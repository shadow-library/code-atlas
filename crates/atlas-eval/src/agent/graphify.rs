use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{AgentError, AgentResult};
use super::incumbent::{cache_lock, linked_graph_file};
use super::process::{ProcessExit, run_captured, tail};
use super::record::{read_json, write_json};
use super::spec::{LinkedSpec, LoadedTask};
use super::workspace::{StripPlan, TreeSource, apply_linked_patch, git_command, materialize_session_tree, materialize_task};

pub const GRAPHIFY_OUT: &str = "graphify-out";
pub const SERENA_PROJECT_DIR: &str = ".serena";
pub const GRAPH_ROOT: &str = ".";
const IGNORE_PROBE: &str = "atlas-eval-probe";
const GRAPH_CACHE: &str = "graphify";
const GRAPHS_FILE: &str = "graphs.json";
const GRAPH_FILE: &str = "graph.json";
// `graphify update` without a path rebuilds the directory `.graphify_root` names; `cache/` and `manifest.json` hold absolute paths of the build tree.
const BUILD_ONLY: [&str; 3] = [".graphify_root", "cache", "manifest.json"];
const ROOT_MARKER: &str = ".graphify_root";
const WORKSPACE_PARENTS: [&str; 2] = ["apps", "packages"];
const STAGE_TREE: &str = "tree";
const STAGE_VIEW: &str = "linked-view";
const STAGE_HOME: &str = "home";
const SPEC_HASH_CHARS: usize = 12;
const SKIPPED_DIRS: [&str; 6] = [".git", "node_modules", "target", "dist", GRAPHIFY_OUT, SERENA_PROJECT_DIR];
const SOURCE_EXTENSIONS: [&str; 28] = [
  "rs", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "rb", "php", "cs", "swift", "c", "h", "cc", "cpp", "hpp", "scala", "vue", "svelte",
  "dart", "lua",
];
const OUTPUT_TAIL_BYTES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphInputs {
  pub graphify_version: Option<String>,
  pub tree: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub linked_view: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSet {
  pub key: String,
  #[serde(default)]
  pub inputs: Option<GraphInputs>,
  pub roots: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub linked_repo: Option<String>,
  pub nodes: u64,
  pub edges: u64,
  pub build_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedGraphs {
  pub set: GraphSet,
  pub dir: PathBuf,
  pub built_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graphify {
  pub binary: PathBuf,
  pub timeout: Duration,
}

impl Graphify {
  fn run(&self, args: &[&str], cwd: &Path, home: &Path) -> AgentResult<()> {
    let mut command = Command::new(&self.binary);
    command.args(args).current_dir(cwd).env("HOME", home);
    let output = run_captured(command, None, self.timeout);
    if output.exit.succeeded() {
      return Ok(());
    }
    Err(AgentError::Command {
      command: format!("graphify {}", args.join(" ")),
      detail: format!("{:?}: {}", output.exit, tail(&format!("{}{}", output.stdout, output.stderr), OUTPUT_TAIL_BYTES)),
    })
  }

  fn build(&self, dir: &Path, cwd: &Path, home: &Path) -> AgentResult<()> {
    let dir = dir.display().to_string();
    self.run(&["update", &dir], cwd, home)?;
    self.run(&["cluster-only", &dir, "--no-viz"], cwd, home)
  }
}

pub struct GraphBuild<'a> {
  pub graphify: &'a Graphify,
  pub graphify_version: Option<&'a str>,
  pub task: &'a LoadedTask,
  pub source: &'a TreeSource,
  pub strip: &'a StripPlan,
  pub cache_dir: &'a Path,
  pub work: &'a Path,
}

pub fn graph_cache_key(task: &LoadedTask) -> String {
  let hash: String = task.spec_hash.chars().take(SPEC_HASH_CHARS).collect();
  format!("{}-{hash}", task.spec.id)
}

pub fn ensure_graphs(build: &GraphBuild<'_>) -> AgentResult<CachedGraphs> {
  let cache = build.cache_dir.join(GRAPH_CACHE);
  let key = graph_cache_key(build.task);
  let target = cache.join(&key);
  let tree = build.work.join(STAGE_TREE);
  materialize_session_tree(build.task, build.source, &tree, build.strip)?;
  let view = match &build.task.spec.linked {
    Some(linked) => {
      let view = build.work.join(STAGE_VIEW);
      materialize_task(build.task, build.source, &view, build.strip)?;
      apply_linked_patch(build.task, &view)?;
      Some((linked, view))
    }
    None => None,
  };
  let inputs = GraphInputs {
    graphify_version: build.graphify_version.map(str::to_owned),
    tree: content_identity(&tree, &[String::new()])?,
    linked_view: view.as_ref().map(|(linked, view)| content_identity(view, &linked.paths)).transpose()?,
  };
  if let Some(set) = cached(&target, &inputs)? {
    return Ok(CachedGraphs {
      set,
      dir: target,
      built_now: false,
    });
  }
  let _lock = cache_lock(&target)?;
  if let Some(set) = cached(&target, &inputs)? {
    return Ok(CachedGraphs {
      set,
      dir: target,
      built_now: false,
    });
  }
  let started = Instant::now();
  let home = build.work.join(STAGE_HOME);
  fs::create_dir_all(&home).map_err(|error| AgentError::io(&home, error))?;
  let roots = graph_roots(&tree)?;
  for root in &roots {
    build.graphify.build(&tree.join(root), build.work, &home)?;
  }
  if let Some((linked, view)) = &view {
    merge_linked(build, linked, view, &tree, &home)?;
  }
  let (nodes, edges) = graph_size(&tree.join(GRAPHIFY_OUT).join(GRAPH_FILE))?;
  let set = GraphSet {
    key: key.clone(),
    inputs: Some(inputs),
    roots: roots.clone(),
    linked_repo: build.task.spec.linked.as_ref().map(|linked| linked.repo.clone()),
    nodes,
    edges,
    build_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
  };
  let staging = cache.join(format!("{key}.staging"));
  remove_dir(&staging)?;
  for root in &roots {
    copy_dir(&tree.join(root).join(GRAPHIFY_OUT), &staging.join(root).join(GRAPHIFY_OUT), &BUILD_ONLY)?;
  }
  write_json(&staging.join(GRAPHS_FILE), &set)?;
  remove_dir(&target)?;
  fs::rename(&staging, &target).map_err(|error| AgentError::io(&staging, error))?;
  Ok(CachedGraphs {
    set,
    dir: target,
    built_now: true,
  })
}

fn merge_linked(build: &GraphBuild<'_>, linked: &LinkedSpec, view: &Path, tree: &Path, home: &Path) -> AgentResult<()> {
  let repo = build.work.join(&linked.repo);
  for path in &linked.paths {
    let destination = repo.join(path);
    if let Some(parent) = destination.parent() {
      fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
    }
    fs::rename(view.join(path), &destination).map_err(|error| AgentError::io(view.join(path), error))?;
  }
  let repo_arg = repo.display().to_string();
  build.graphify.run(&["update", &repo_arg], build.work, home)?;
  let root_graph = tree.join(GRAPHIFY_OUT).join(GRAPH_FILE);
  let linked_graph = repo.join(GRAPHIFY_OUT).join(GRAPH_FILE);
  let (root_arg, linked_arg) = (root_graph.display().to_string(), linked_graph.display().to_string());
  build.graphify.run(&["merge-graphs", &root_arg, &linked_arg, "--out", &root_arg], build.work, home)?;
  unprefix_session_tree(&root_graph)?;
  let alone = tree.join(GRAPHIFY_OUT).join(linked_graph_file(&linked.repo));
  fs::copy(&linked_graph, &alone).map_err(|error| AgentError::io(&linked_graph, error))?;
  build.graphify.run(&["cluster-only", &tree.display().to_string(), "--no-viz"], build.work, home)
}

pub fn unprefix_session_tree(graph_path: &Path) -> AgentResult<()> {
  let mut graph: Value = read_json(graph_path)?;
  let prefix = format!("{STAGE_TREE}::");
  let unprefixed = |value: &Value| value.as_str().and_then(|id| id.strip_prefix(prefix.as_str())).map(|id| Value::String(id.to_owned()));
  for node in graph.get_mut("nodes").and_then(Value::as_array_mut).into_iter().flatten() {
    let Some(fields) = node.as_object_mut() else {
      continue;
    };
    let Some(id) = fields.get("id").and_then(unprefixed) else {
      continue;
    };
    fields.insert("id".to_owned(), id);
    fields.remove("repo");
    fields.remove("local_id");
  }
  for key in ["links", "edges"] {
    for link in graph.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
      for end in ["source", "target"] {
        if let Some(id) = link.get(end).and_then(unprefixed) {
          link[end] = id;
        }
      }
    }
  }
  if let Some(hyperedges) = graph.get_mut("hyperedges") {
    unprefix_strings(hyperedges, &prefix);
  }
  write_json(graph_path, &graph)
}

fn unprefix_strings(value: &mut Value, prefix: &str) {
  match value {
    Value::String(text) => {
      if let Some(rest) = text.strip_prefix(prefix) {
        *text = rest.to_owned();
      }
    }
    Value::Array(items) => items.iter_mut().for_each(|item| unprefix_strings(item, prefix)),
    Value::Object(fields) => fields.values_mut().for_each(|item| unprefix_strings(item, prefix)),
    _ => {}
  }
}

fn cached(target: &Path, inputs: &GraphInputs) -> AgentResult<Option<GraphSet>> {
  let path = target.join(GRAPHS_FILE);
  if !path.is_file() {
    return Ok(None);
  }
  let set: GraphSet = read_json(&path)?;
  Ok((set.inputs.as_ref() == Some(inputs)).then_some(set))
}

pub fn content_identity(root: &Path, paths: &[String]) -> AgentResult<String> {
  let mut hasher = blake3::Hasher::new();
  for path in paths {
    hash_entry(root, Path::new(path), &mut hasher)?;
  }
  Ok(hasher.finalize().to_hex().to_string())
}

fn hash_entry(root: &Path, relative: &Path, hasher: &mut blake3::Hasher) -> AgentResult<()> {
  let absolute = root.join(relative);
  let kind = fs::symlink_metadata(&absolute).map_err(|error| AgentError::io(&absolute, error))?.file_type();
  if kind.is_dir() {
    let mut names: Vec<std::ffi::OsString> = fs::read_dir(&absolute)
      .map_err(|error| AgentError::io(&absolute, error))?
      .map(|entry| entry.map(|entry| entry.file_name()))
      .collect::<Result<_, _>>()
      .map_err(|error| AgentError::io(&absolute, error))?;
    names.sort();
    for name in names {
      hash_entry(root, &relative.join(name), hasher)?;
    }
    return Ok(());
  }
  hasher.update(relative.to_string_lossy().as_bytes());
  hasher.update(b"\0");
  if kind.is_symlink() {
    let target = fs::read_link(&absolute).map_err(|error| AgentError::io(&absolute, error))?;
    hasher.update(b"link:");
    hasher.update(target.to_string_lossy().as_bytes());
  } else {
    let bytes = fs::read(&absolute).map_err(|error| AgentError::io(&absolute, error))?;
    hasher.update(blake3::hash(&bytes).as_bytes());
  }
  hasher.update(b"\n");
  Ok(())
}

pub fn graph_roots(tree: &Path) -> AgentResult<Vec<String>> {
  let mut roots = vec![GRAPH_ROOT.to_owned()];
  for parent in WORKSPACE_PARENTS {
    let dir = tree.join(parent);
    if !dir.is_dir() {
      continue;
    }
    let mut members = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| AgentError::io(&dir, error))? {
      let entry = entry.map_err(|error| AgentError::io(&dir, error))?;
      let is_dir = entry.file_type().map_err(|error| AgentError::io(entry.path(), error))?.is_dir();
      if is_dir && holds_source(&entry.path())? {
        members.push(format!("{parent}/{}", entry.file_name().to_string_lossy()));
      }
    }
    members.sort();
    roots.extend(members);
  }
  Ok(roots)
}

fn holds_source(dir: &Path) -> AgentResult<bool> {
  for entry in fs::read_dir(dir).map_err(|error| AgentError::io(dir, error))? {
    let entry = entry.map_err(|error| AgentError::io(dir, error))?;
    let kind = entry.file_type().map_err(|error| AgentError::io(entry.path(), error))?;
    let name = entry.file_name().to_string_lossy().into_owned();
    let found = match (kind.is_dir(), kind.is_file()) {
      (true, _) if SKIPPED_DIRS.contains(&name.as_str()) => false,
      (true, _) => holds_source(&entry.path())?,
      (false, true) => name
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && SOURCE_EXTENSIONS.contains(&extension)),
      (false, false) => false,
    };
    if found {
      return Ok(true);
    }
  }
  Ok(false)
}

pub fn graph_size(path: &Path) -> AgentResult<(u64, u64)> {
  let graph: Value = read_json(path)?;
  let count = |field: &str| graph.get(field).and_then(Value::as_array).map(|items| items.len() as u64);
  let nodes = count("nodes").ok_or_else(|| AgentError::Parse {
    path: path.to_owned(),
    reason: "no `nodes` array".to_owned(),
  })?;
  Ok((nodes, count("links").or_else(|| count("edges")).unwrap_or(0)))
}

pub fn install_graphs(cached: &CachedGraphs, tree: &Path) -> AgentResult<()> {
  for root in &cached.set.roots {
    let destination = tree.join(root).join(GRAPHIFY_OUT);
    remove_dir(&destination)?;
    clone_dir(&cached.dir.join(root).join(GRAPHIFY_OUT), &destination)?;
    let marker = destination.join(ROOT_MARKER);
    let build_root = if root == GRAPH_ROOT { tree.to_owned() } else { tree.join(root) };
    fs::write(&marker, build_root.display().to_string()).map_err(|error| AgentError::io(&marker, error))?;
  }
  Ok(())
}

pub fn exclude_tool_dirs(tree: &Path, graph_roots: &[String]) -> AgentResult<Vec<String>> {
  let graph_probes: Vec<String> = graph_roots
    .iter()
    .map(|root| match root.as_str() {
      GRAPH_ROOT => format!("{GRAPHIFY_OUT}/{IGNORE_PROBE}"),
      root => format!("{root}/{GRAPHIFY_OUT}/{IGNORE_PROBE}"),
    })
    .collect();
  let serena_probe = vec![format!("{SERENA_PROJECT_DIR}/{IGNORE_PROBE}")];
  let mut added = Vec::new();
  for (dir, probes) in [(GRAPHIFY_OUT, &graph_probes), (SERENA_PROJECT_DIR, &serena_probe)] {
    for probe in probes {
      if !ignored(tree, probe)? {
        added.push(format!("{dir}/"));
        break;
      }
    }
  }
  if added.is_empty() {
    return Ok(added);
  }
  let exclude = tree.join(".git").join("info").join("exclude");
  let mut text = fs::read_to_string(&exclude).unwrap_or_default();
  if !text.is_empty() && !text.ends_with('\n') {
    text.push('\n');
  }
  for line in &added {
    text.push_str(line);
    text.push('\n');
  }
  fs::write(&exclude, text).map_err(|error| AgentError::io(&exclude, error))?;
  Ok(added)
}

fn ignored(tree: &Path, path: &str) -> AgentResult<bool> {
  let mut command = git_command(tree);
  command.args(["check-ignore", "-q", "--no-index", path]);
  let output = run_captured(command, None, Duration::from_secs(60));
  match output.exit {
    ProcessExit::Exited { code: Some(0) } => Ok(true),
    ProcessExit::Exited { code: Some(1) } => Ok(false),
    _ => Err(AgentError::Command {
      command: format!("git check-ignore {path}"),
      detail: format!("{:?}: {}", output.exit, output.stderr),
    }),
  }
}

fn clone_dir(from: &Path, to: &Path) -> AgentResult<()> {
  if let Some(parent) = to.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let mut command = Command::new("cp");
  if cfg!(target_os = "macos") {
    command.arg("-cR");
  } else {
    command.args(["--reflink=auto", "-R"]);
  }
  command.arg(from).arg(to);
  if run_captured(command, None, Duration::from_secs(600)).exit.succeeded() {
    return Ok(());
  }
  remove_dir(to)?;
  copy_dir(from, to, &[])
}

fn copy_dir(from: &Path, to: &Path, skip: &[&str]) -> AgentResult<()> {
  fs::create_dir_all(to).map_err(|error| AgentError::io(to, error))?;
  for entry in fs::read_dir(from).map_err(|error| AgentError::io(from, error))? {
    let entry = entry.map_err(|error| AgentError::io(from, error))?;
    let name = entry.file_name();
    if skip.iter().any(|skipped| name == *skipped) {
      continue;
    }
    let kind = entry.file_type().map_err(|error| AgentError::io(entry.path(), error))?;
    if kind.is_dir() {
      copy_dir(&entry.path(), &to.join(&name), &[])?;
    } else if kind.is_file() {
      fs::copy(entry.path(), to.join(&name)).map_err(|error| AgentError::io(entry.path(), error))?;
    }
  }
  Ok(())
}

fn remove_dir(path: &Path) -> AgentResult<()> {
  match fs::remove_dir_all(path) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(AgentError::io(path, error)),
    _ => Ok(()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  #[test]
  fn should_find_the_root_and_every_workspace_directory_that_holds_source() {
    let tree = tempfile::tempdir().unwrap();
    write(&tree.path().join("apps/acme-api/src/main.ts"), "");
    write(&tree.path().join("apps/acme-docs/README.md"), "");
    write(&tree.path().join("apps/acme-deps/node_modules/x/index.js"), "");
    write(&tree.path().join("apps/acme-deps/package.json"), "{}");
    write(&tree.path().join("packages/acme-core/crates/x/src/lib.rs"), "");
    write(&tree.path().join("packages/.hidden.ts"), "");
    write(&tree.path().join("services/acme-jobs/src/main.ts"), "");
    assert_eq!(graph_roots(tree.path()).unwrap(), vec![".", "apps/acme-api", "packages/acme-core"]);
  }

  #[test]
  fn should_key_the_cache_by_task_id_and_twelve_characters_of_the_spec_hash() {
    let spec = super::super::spec::parse_task(
      "id = \"t07-acme\"\ntitle = \"t\"\nrepo = \"self\"\nbase_commit = \"abcdef1\"\nprompt = \"p\"\ntimeout_minutes = 5\nmax_turns = 30\n",
      Path::new("task.toml"),
    )
    .unwrap();
    let task = LoadedTask {
      spec,
      dir: PathBuf::from("t07-acme"),
      spec_hash: "0123456789abcdef0123".to_owned(),
    };
    assert_eq!(graph_cache_key(&task), "t07-acme-0123456789ab");
  }

  #[test]
  fn should_count_nodes_and_links_of_a_graph() {
    let dir = tempfile::tempdir().unwrap();
    write(
      &dir.path().join("graph.json"),
      r#"{"nodes": [{"id": "a"}, {"id": "b"}], "links": [{"source": "a", "target": "b"}]}"#,
    );
    assert_eq!(graph_size(&dir.path().join("graph.json")).unwrap(), (2, 1));
    write(&dir.path().join("old.json"), r#"{"nodes": [{"id": "a"}], "edges": []}"#);
    assert_eq!(graph_size(&dir.path().join("old.json")).unwrap(), (1, 0));
  }

  fn git_tree() -> tempfile::TempDir {
    let tree = tempfile::tempdir().unwrap();
    super::super::workspace::init_base(tree.path()).unwrap();
    tree
  }

  #[test]
  fn should_exclude_the_tool_directories_only_when_the_tree_does_not_ignore_them() {
    let roots = [".".to_owned(), "apps/acme-api".to_owned()];
    let plain = git_tree();
    assert_eq!(exclude_tool_dirs(plain.path(), &roots).unwrap(), vec!["graphify-out/", ".serena/"]);
    assert_eq!(
      fs::read_to_string(plain.path().join(".git/info/exclude")).unwrap(),
      "target/\nnode_modules/\n.code-atlas/\ngraphify-out/\n.serena/\n"
    );
    let ignoring = git_tree();
    write(&ignoring.path().join(".gitignore"), "graphify-out/\n/.serena\n");
    assert_eq!(exclude_tool_dirs(ignoring.path(), &roots).unwrap(), Vec::<String>::new());
    assert_eq!(
      fs::read_to_string(ignoring.path().join(".git/info/exclude")).unwrap(),
      "target/\nnode_modules/\n.code-atlas/\n"
    );
    let partly = git_tree();
    write(&partly.path().join(".gitignore"), "graphify-out\n");
    assert_eq!(exclude_tool_dirs(partly.path(), &roots).unwrap(), vec![".serena/"]);
    let root_only = git_tree();
    write(&root_only.path().join(".gitignore"), "/graphify-out/\n.serena/\n");
    assert_eq!(exclude_tool_dirs(root_only.path(), &[".".to_owned()]).unwrap(), Vec::<String>::new());
    assert_eq!(exclude_tool_dirs(root_only.path(), &roots).unwrap(), vec!["graphify-out/"]);
  }

  #[test]
  fn should_give_merged_session_tree_nodes_their_unprefixed_ids_and_keep_the_linked_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.json");
    let merged = serde_json::json!({
      "directed": false,
      "nodes": [
        {"id": "tree::src_orders_getorder", "repo": "tree", "local_id": "src_orders_getorder", "source_file": "apps/acme-api/src/orders.ts"},
        {"id": "web::src_client_fetchorder", "repo": "web", "local_id": "src_client_fetchorder", "source_file": "apps/acme-web/src/client.ts"}
      ],
      "links": [{"source": "tree::src_orders_getorder", "target": "web::src_client_fetchorder", "relation": "calls"}],
      "hyperedges": [{"nodes": ["tree::src_orders_getorder", "web::src_client_fetchorder"]}]
    });
    write_json(&path, &merged).unwrap();
    unprefix_session_tree(&path).unwrap();
    assert_eq!(
      read_json::<Value>(&path).unwrap(),
      serde_json::json!({
        "directed": false,
        "nodes": [
          {"id": "src_orders_getorder", "source_file": "apps/acme-api/src/orders.ts"},
          {"id": "web::src_client_fetchorder", "repo": "web", "local_id": "src_client_fetchorder", "source_file": "apps/acme-web/src/client.ts"}
        ],
        "links": [{"source": "src_orders_getorder", "target": "web::src_client_fetchorder", "relation": "calls"}],
        "hyperedges": [{"nodes": ["src_orders_getorder", "web::src_client_fetchorder"]}]
      })
    );
  }

  #[test]
  fn should_change_the_content_identity_with_any_file_of_the_hashed_paths_only() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("apps/acme-web/src/a.ts"), "a");
    write(&root.path().join("apps/acme-api/src/b.ts"), "b");
    let whole = content_identity(root.path(), &[String::new()]).unwrap();
    let linked = content_identity(root.path(), &["apps/acme-web".to_owned()]).unwrap();
    write(&root.path().join("apps/acme-api/src/b.ts"), "changed");
    assert_ne!(content_identity(root.path(), &[String::new()]).unwrap(), whole);
    assert_eq!(content_identity(root.path(), &["apps/acme-web".to_owned()]).unwrap(), linked);
    fs::rename(root.path().join("apps/acme-web/src/a.ts"), root.path().join("apps/acme-web/src/c.ts")).unwrap();
    assert_ne!(content_identity(root.path(), &["apps/acme-web".to_owned()]).unwrap(), linked);
  }

  #[test]
  fn should_install_each_cached_graph_with_the_session_tree_as_its_build_root() {
    let cache = tempfile::tempdir().unwrap();
    write(&cache.path().join("graphify-out/graph.json"), "{\"nodes\": []}");
    write(&cache.path().join("graphify-out/GRAPH_REPORT.md"), "# report");
    write(&cache.path().join("apps/acme-api/graphify-out/graph.json"), "{\"nodes\": []}");
    let tree = tempfile::tempdir().unwrap();
    write(&tree.path().join("apps/acme-api/graphify-out/stale.json"), "stale");
    let cached = CachedGraphs {
      set: GraphSet {
        key: "t1-abc".to_owned(),
        inputs: None,
        roots: vec![".".to_owned(), "apps/acme-api".to_owned()],
        linked_repo: None,
        nodes: 0,
        edges: 0,
        build_ms: 0,
      },
      dir: cache.path().to_owned(),
      built_now: false,
    };
    install_graphs(&cached, tree.path()).unwrap();
    assert_eq!(fs::read_to_string(tree.path().join("graphify-out/GRAPH_REPORT.md")).unwrap(), "# report");
    assert_eq!(
      fs::read_to_string(tree.path().join("graphify-out/.graphify_root")).unwrap(),
      tree.path().display().to_string()
    );
    assert_eq!(
      fs::read_to_string(tree.path().join("apps/acme-api/graphify-out/.graphify_root")).unwrap(),
      tree.path().join("apps/acme-api").display().to_string()
    );
    assert!(!tree.path().join("apps/acme-api/graphify-out/stale.json").exists());
    assert!(tree.path().join("apps/acme-api/graphify-out/graph.json").is_file());
  }
}
