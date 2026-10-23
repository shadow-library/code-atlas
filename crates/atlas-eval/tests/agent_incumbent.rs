use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_eval::agent::{
  self, AgentError, ClaudeConfig, Condition, ConfigIsolation, GRAPHIFY_SECTION, GraphBuild, Graphify, IncumbentProbe, IncumbentTools, JudgeMode, Metrics, PreambleVariant,
  RUN_FILE, RunManifest, RunStatus, RunnerConfig, SERENA_CONFIG, SessionOutcome, StripPlan, SurfaceSetting, TreeSource, graph_cache_key, resolve_binary,
};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "0.0.0-fake (Claude Code)"; exit 0; fi
mcp=""
settings=""
while [ $# -gt 0 ]; do
  case "$1" in
    --mcp-config) mcp="$2"; shift 2 ;;
    --settings) settings="$2"; shift 2 ;;
    *) shift ;;
  esac
done
servers='[]'
if grep -q serena "$mcp"; then servers='[{"name":"serena","status":"connected"}]'; fi
hook=""
if [ -n "$settings" ] && [ ! -f "$PWD/../../../no-hooks" ] && { [ ! -f "$PWD/../../../no-run-hooks" ] || [ ! -d graphify-out ]; }; then
  command=$(sed 's/.*"SessionStart":\[{"hooks":\[{"command":"\([^"]*\)".*/\1/' "$settings")
  context=$(echo '{"session_id":"s1","hook_event_name":"SessionStart"}' | sh -c "$command" | sed 's/"/\\"/g')
  hook="{\"type\":\"system\",\"subtype\":\"hook_response\",\"hook_id\":\"h0\",\"hook_name\":\"SessionStart:startup\",\"hook_event\":\"SessionStart\",\"stdout\":\"$context\",\"stderr\":\"\",\"exit_code\":0,\"outcome\":\"success\"}"
fi
find . -path ./.git -prune -o -type f -print | sort | sed 's/^/FILE:/' >&2
echo "STATUS:$(git status --porcelain --untracked-files=all)" >&2
if [ -d apps/acme-api ]; then
cat > apps/acme-api/src/orders.ts <<'TS'
export function getOrder(id: string) { return id; }
export function cancelOrder(id: string) { return id; }
TS
mkdir -p .serena && echo "project_name: tree" > .serena/project.yml
fi
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"$PWD\",\"tools\":[\"Read\",\"Edit\",\"Bash\"],\"mcp_servers\":$servers,\"model\":\"claude-haiku-4-5-20251001\",\"permissionMode\":\"bypassPermissions\",\"claude_code_version\":\"0.0.0-fake\"}"
if [ -n "$hook" ]; then printf '%s\n' "$hook"; fi
if [ "$servers" != '[]' ]; then
  printf '%s\n' '{"type":"assistant","message":{"id":"m1","model":"claude-haiku-4-5-20251001","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"graphify query \"cancel order\""}}],"usage":{"input_tokens":1,"output_tokens":1}}}'
  printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"NODE cancelPath"}]}}'
  printf '%s\n' '{"type":"assistant","message":{"id":"m2","model":"claude-haiku-4-5-20251001","content":[{"type":"tool_use","id":"t2","name":"mcp__serena__find_symbol","input":{"name_path_pattern":"cancelPath","relative_path":"../linked-view/apps/acme-web/src/api/orders.ts"}}],"usage":{"input_tokens":1,"output_tokens":1}}}'
  printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"cancelPath"}]}}'
fi
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"duration_ms":10,"num_turns":2,"total_cost_usd":0.01,"usage":{"input_tokens":2,"output_tokens":2}}'
"#;

const FAKE_ATLAS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "catl 0.0.0-fake" ;;
  *) exit 2 ;;
esac
"#;

const FAKE_SERENA: &str = r#"#!/bin/sh
echo "SERENA_HOME=$SERENA_HOME HOME=$HOME $*" >> "$(dirname "$0")/serena.log"
case "$1" in
  --version) echo "Serena 0.0.0-fake" ;;
  project) mkdir -p "$SERENA_HOME/language_servers/static/fake-ts" ;;
  *) exit 2 ;;
esac
"#;

const FAKE_SERENA_HOOKS: &str = r#"#!/bin/sh
echo "SERENA_HOME=$SERENA_HOME $*" >> "$(dirname "$0")/serena-hooks.log"
cat > /dev/null
if [ "$1" = "activate" ]; then echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"activate the project"}}'; fi
"#;

const FAKE_GRAPHIFY: &str = r##"#!/bin/sh
echo "HOME=$HOME $*" >> "$(dirname "$0")/graphify.log"
nodes() {
  sed -e 's/^{"nodes":\[//' -e 's/\],"links":\[\]}$//' -e "s/\"id\":\"/\"id\":\"$2::/g" "$1"
}
case "$1" in
  --version) echo "graphify 0.0.0-fake" ;;
  update)
    mkdir -p "$2/graphify-out/cache"
    list=""
    for file in $(cd "$2" && find . -name graphify-out -prune -o -type f -name '*.ts' -print | sed 's|^\./||' | sort); do
      list="$list{\"id\":\"$file\"},"
    done
    printf '{"nodes":[%s],"links":[]}' "${list%,}" > "$2/graphify-out/graph.json"
    echo "$2" > "$2/graphify-out/.graphify_root"
    echo "{}" > "$2/graphify-out/cache/stat-index.json" ;;
  cluster-only) echo "# report of $(basename "$2")" > "$2/graphify-out/GRAPH_REPORT.md" ;;
  merge-graphs)
    first=$(nodes "$2" "$(basename "$(dirname "$(dirname "$2")")")")
    second=$(nodes "$3" "$(basename "$(dirname "$(dirname "$3")")")")
    printf '{"nodes":[%s,%s],"links":[]}' "$first" "$second" > "$5" ;;
  *) exit 2 ;;
esac
"##;

const TASK: &str = r#"
id = "t97-incumbent"
title = "Incumbent"
repo = { fixture = "acme" }
base_commit = "e5209b4"
prompt = "Add cancelOrder to the orders API; the web app already calls it."
timeout_minutes = 1
max_turns = 5
[linked]
paths = ["apps/acme-web"]
patch = "linked.patch"
repo = "web"
[validation]
commands = ["grep -q cancelOrder apps/acme-api/src/orders.ts"]
[ground_truth]
must_change = ["apps/acme-api/src/orders.ts"]
"#;

const LINKED_PATCH: &str = "--- a/apps/acme-web/src/api/orders.ts
+++ b/apps/acme-web/src/api/orders.ts
@@ -1 +1,2 @@
 export const ordersPath = '/orders';
+export const cancelPath = '/orders/:id/cancel';
";

fn write(path: &Path, content: &str) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, content).unwrap();
}

fn executable(path: &Path, content: &str) {
  write(path, content);
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
  }
}

struct Fixture {
  root: tempfile::TempDir,
  tasks: PathBuf,
  source: PathBuf,
}

impl Fixture {
  fn path(&self, name: &str) -> PathBuf {
    self.root.path().join(name)
  }

  fn tools(&self) -> IncumbentTools {
    let bin = self.path("bin");
    IncumbentTools {
      serena: bin.join("serena"),
      serena_version: Some("Serena 0.0.0-fake".to_owned()),
      serena_hooks: bin.join("serena-hooks"),
      graphify: bin.join("graphify"),
      graphify_version: Some("graphify 0.0.0-fake".to_owned()),
      language_servers: self.path("eval/.cache/serena-language-servers"),
      timeout: Duration::from_secs(60),
    }
  }

  fn config(&self, batch: &str) -> RunnerConfig {
    let bin = self.path("bin");
    RunnerConfig {
      repo_root: self.source.clone(),
      eval_home: self.path("eval"),
      batch_dir: self.path(&format!("eval/runs/{batch}")),
      tree_root: self.path("trees"),
      keep_trees: true,
      cache_dir: self.path("eval/.cache"),
      claude: ClaudeConfig {
        binary: bin.join("claude"),
        model: "claude-haiku-4-5-20251001".to_owned(),
        judge_model: "claude-sonnet-5-5".to_owned(),
        config_isolation: ConfigIsolation::User,
      },
      atlas_bin: bin.join("catl"),
      repo_override: Some(self.source.clone()),
      judge: JudgeMode::Off,
      parallel: 1,
      budget_usd: None,
      validation_timeout: Duration::from_secs(60),
      atlas_timeout: Duration::from_secs(60),
      seed: 5,
      probe: true,
      preamble: PreambleVariant::Standard,
      atlas_config: None,
      atlas_hooks: false,
      atlas_surface: SurfaceSetting::Standard,
      incumbent: Some(self.tools()),
    }
  }
}

fn fixture() -> Fixture {
  let root = tempfile::tempdir().unwrap();
  let tasks = root.path().join("tasks");
  write(&tasks.join("t97-incumbent/task.toml"), TASK);
  write(&tasks.join("t97-incumbent/linked.patch"), LINKED_PATCH);
  let source = root.path().join("source");
  write(&source.join("package.json"), "{\"private\": true, \"workspaces\": [\"apps/*\"]}\n");
  write(&source.join("apps/acme-api/src/orders.ts"), "export function getOrder(id: string) { return id; }\n");
  write(&source.join("apps/acme-web/src/api/orders.ts"), "export const ordersPath = '/orders';\n");
  write(&source.join("apps/acme-docs/README.md"), "# docs\n");
  for (name, script) in [
    ("claude", FAKE_CLAUDE),
    ("catl", FAKE_ATLAS),
    ("serena", FAKE_SERENA),
    ("serena-hooks", FAKE_SERENA_HOOKS),
    ("graphify", FAKE_GRAPHIFY),
  ] {
    executable(&root.path().join("bin").join(name), script);
  }
  fs::create_dir_all(root.path().join("trees")).unwrap();
  Fixture { root, tasks, source }
}

fn lines_with(log: &str, prefix: &str) -> Vec<String> {
  log.lines().filter_map(|line| line.strip_prefix(prefix)).map(str::to_owned).collect()
}

fn run_dir(fixture: &Fixture, batch: &str, condition: Condition) -> PathBuf {
  fixture.path(&format!("eval/runs/{batch}/t97-incumbent.{condition}.r0"))
}

#[test]
fn should_give_the_incumbent_arm_serena_and_cached_graphify_graphs_of_the_session_tree_and_the_linked_repo() {
  let fixture = fixture();
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  let conditions = [Condition::Baseline, Condition::Incumbent];
  let manifest = agent::run_batch(&fixture.config("first"), &tasks, agent::plan_runs(&tasks, &conditions, 1, 5)).unwrap();
  let record = manifest.environment.incumbent.clone().unwrap();
  assert_eq!(
    (record.serena_version.as_deref(), record.graphify_version.as_deref(), record.instructions_version),
    (Some("Serena 0.0.0-fake"), Some("graphify 0.0.0-fake"), 2)
  );
  let probe: IncumbentProbe = agent::read_json(&fixture.path("eval/runs/first/_probe/incumbent/probe.json")).unwrap();
  assert_eq!(
    (probe.started, probe.serena_status.as_deref(), probe.session_start_injected),
    (true, Some("connected"), true)
  );
  assert_eq!(probe.hooks_seen, vec!["SessionStart (SessionStart:startup)"]);

  let dir = run_dir(&fixture, "first", Condition::Incumbent);
  let run: RunManifest = agent::read_json(&dir.join(RUN_FILE)).unwrap();
  assert_eq!(
    run.status,
    RunStatus::Completed {
      session: SessionOutcome::Finished { subtype: "success".to_owned() }
    }
  );
  let tree = PathBuf::from(run.tree.clone().unwrap());
  let scratch = tree.parent().unwrap().to_owned();
  let session_log = fs::read_to_string(dir.join("claude.stderr.log")).unwrap();
  assert_eq!(
    lines_with(&session_log, "FILE:"),
    vec![
      "./apps/acme-api/graphify-out/.graphify_root",
      "./apps/acme-api/graphify-out/GRAPH_REPORT.md",
      "./apps/acme-api/graphify-out/graph.json",
      "./apps/acme-api/src/orders.ts",
      "./apps/acme-docs/README.md",
      "./graphify-out/.graphify_root",
      "./graphify-out/GRAPH_REPORT.md",
      "./graphify-out/graph.json",
      "./graphify-out/linked-web.json",
      "./package.json",
    ]
  );
  assert_eq!(lines_with(&session_log, "STATUS:"), vec![""]);
  assert_eq!(
    fs::read_to_string(tree.join(".git/info/exclude")).unwrap(),
    "target/\nnode_modules/\n.code-atlas/\ngraphify-out/\n.serena/\n"
  );
  assert_eq!(fs::read_to_string(tree.join("graphify-out/.graphify_root")).unwrap(), tree.display().to_string());
  assert_eq!(
    agent::read_json::<serde_json::Value>(&tree.join("graphify-out/graph.json")).unwrap(),
    serde_json::json!({"nodes": [{"id": "apps/acme-api/src/orders.ts"}, {"id": "web::apps/acme-web/src/api/orders.ts"}], "links": []})
  );
  assert_eq!(
    fs::read_to_string(tree.join("graphify-out/linked-web.json")).unwrap(),
    "{\"nodes\":[{\"id\":\"apps/acme-web/src/api/orders.ts\"}],\"links\":[]}"
  );
  assert_eq!(fs::read_to_string(tree.join("graphify-out/GRAPH_REPORT.md")).unwrap(), "# report of tree\n");
  let diff = fs::read_to_string(dir.join("diff.patch")).unwrap();
  assert!(
    diff.contains("apps/acme-api/src/orders.ts") && !diff.contains("graphify-out") && !diff.contains(".serena"),
    "{diff}"
  );

  let key = graph_cache_key(&tasks[0]);
  let incumbent = run.incumbent.clone().unwrap();
  assert_eq!(
    (
      incumbent.graph_key.as_str(),
      incumbent.graph_roots.clone(),
      incumbent.graph_nodes,
      incumbent.graph_edges,
      incumbent.excluded.clone()
    ),
    (
      key.as_str(),
      vec![".".to_owned(), "apps/acme-api".to_owned()],
      2,
      0,
      vec!["graphify-out/".to_owned(), ".serena/".to_owned()]
    )
  );
  assert!(incumbent.graph_build_ms > 0);
  assert!(fixture.path(&format!("eval/.cache/graphify/{key}/graphs.json")).is_file());
  assert!(!fixture.path(&format!("eval/.cache/graphify/{key}/graphify-out/cache")).exists());
  assert!(!scratch.join("graphify-build").exists());

  let tools = fixture.tools();
  let home = scratch.join("serena-home");
  assert_eq!(fs::read_to_string(home.join("serena_config.yml")).unwrap(), SERENA_CONFIG);
  assert_eq!(fs::read_link(home.join("language_servers")).unwrap(), tools.language_servers);
  assert!(tools.language_servers.join("static/fake-ts").is_dir(), "the warm-up filled the shared cache");
  assert_eq!(agent::read_json::<serde_json::Value>(&dir.join("mcp.json")).unwrap(), tools.mcp_config(&home));
  assert_eq!(
    agent::read_json::<serde_json::Value>(&dir.join("incumbent-hooks.json")).unwrap(),
    tools.hook_settings(&home)
  );
  let settings = run.args.iter().position(|arg| arg == "--settings").map(|index| run.args[index + 1].clone());
  assert_eq!(settings, Some(scratch.join("incumbent-hooks.json").display().to_string()));
  let allowed = run.args.iter().position(|arg| arg == "--allowedTools").map(|index| run.args[index + 1].clone());
  assert_eq!(allowed.as_deref(), Some("Read,Edit,Write,Glob,Grep,Bash,mcp__serena"));
  let system = run
    .args
    .iter()
    .position(|arg| arg == "--append-system-prompt")
    .map(|index| run.args[index + 1].clone())
    .unwrap();
  assert_eq!(
    system,
    format!(
      "{GRAPHIFY_SECTION}\n\ngraphify-out/graph.json also covers the linked repository `web`, checked out read-only at `{}/`: its nodes are prefixed `web::` and their source paths are relative to that checkout. Its graph alone is graphify-out/linked-web.json (pass it with `--graph`). `graphify update .` rebuilds only this repository, so after it graph.json no longer covers `web`; query graphify-out/linked-web.json for it.",
      scratch.join("linked-view").display()
    )
  );
  assert!(scratch.join("linked-view/apps/acme-web/src/api/orders.ts").is_file());
  assert_eq!(run.env_names.iter().filter(|name| *name == "PATH").count(), 1);
  assert_eq!(fs::read_link(scratch.join("incumbent-bin/graphify")).unwrap(), tools.graphify);
  let hooks_log = fs::read_to_string(fixture.path("bin/serena-hooks.log")).unwrap();
  assert!(
    hooks_log.contains(&format!("SERENA_HOME={} activate --client=claude-code\n", home.display())),
    "{hooks_log}"
  );

  let metrics: Metrics = agent::read_json(&dir.join("metrics.json")).unwrap();
  assert!(!metrics.contaminated, "{:?}", metrics.out_of_tree_accesses);
  let incumbent_metrics = metrics.incumbent.unwrap();
  assert_eq!(
    (incumbent_metrics.graphify_calls, incumbent_metrics.serena_total(), incumbent_metrics.graphify_bytes),
    (1, 1, 15)
  );
  assert_eq!(
    metrics.out_of_tree_accesses.iter().map(|access| (access.tool.as_str(), access.linked)).collect::<Vec<_>>(),
    vec![("mcp__serena__find_symbol", Some(agent::LinkedAccess::Read))]
  );

  let real_home = std::env::var("HOME").unwrap_or_default();
  let serena_log = fs::read_to_string(fixture.path("bin/serena.log")).unwrap();
  let graphify_log = fs::read_to_string(fixture.path("bin/graphify.log")).unwrap();
  let trees = fixture.path("trees");
  for line in serena_log.lines().chain(graphify_log.lines()).filter(|line| !line.ends_with("--version")) {
    let home = line.split("HOME=").last().unwrap().split(' ').next().unwrap();
    assert!(Path::new(home).starts_with(&trees), "{line}");
    assert!(real_home.is_empty() || !line.contains(&format!("HOME={real_home} ")), "{line}");
  }
  assert_eq!(
    lines_with(&serena_log, "SERENA_HOME=")
      .iter()
      .filter(|line| line.contains("project index") && line.ends_with("/serena-warm-up --language typescript --log-level ERROR"))
      .count(),
    1
  );
  assert_eq!(graphify_log.lines().filter(|line| line.contains(" update ")).count(), 3);
  assert_eq!(graphify_log.lines().filter(|line| line.contains(" merge-graphs ")).count(), 1);

  let baseline = run_dir(&fixture, "first", Condition::Baseline);
  let baseline_run: RunManifest = agent::read_json(&baseline.join(RUN_FILE)).unwrap();
  assert_eq!(baseline_run.incumbent, None);
  let baseline_log = fs::read_to_string(baseline.join("claude.stderr.log")).unwrap();
  assert!(!baseline_log.contains("graphify-out"), "{baseline_log}");
  let baseline_tree = PathBuf::from(baseline_run.tree.unwrap());
  assert_eq!(
    fs::read_to_string(baseline_tree.join(".git/info/exclude")).unwrap(),
    "target/\nnode_modules/\n.code-atlas/\n"
  );
  let baseline_metrics: Metrics = agent::read_json(&baseline.join("metrics.json")).unwrap();
  assert_eq!(baseline_metrics.incumbent, None);

  agent::run_batch(&fixture.config("second"), &tasks, agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5)).unwrap();
  let cached: RunManifest = agent::read_json(&run_dir(&fixture, "second", Condition::Incumbent).join(RUN_FILE)).unwrap();
  let cached = cached.incumbent.unwrap();
  assert_eq!((cached.graph_build_ms, cached.graph_nodes), (0, 2));
  let updates = || {
    fs::read_to_string(fixture.path("bin/graphify.log"))
      .unwrap()
      .lines()
      .filter(|line| line.contains(" update "))
      .count()
  };
  assert_eq!(updates(), 3);

  let mut upgraded = fixture.config("third");
  if let Some(tools) = upgraded.incumbent.as_mut() {
    tools.graphify_version = Some("graphify 9.9.9".to_owned());
  }
  agent::run_batch(&upgraded, &tasks, agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5)).unwrap();
  let rebuilt: RunManifest = agent::read_json(&run_dir(&fixture, "third", Condition::Incumbent).join(RUN_FILE)).unwrap();
  assert!(rebuilt.incumbent.unwrap().graph_build_ms > 0, "another graphify version misses the cache");
  assert_eq!(updates(), 6);
  let set: serde_json::Value = agent::read_json(&fixture.path(&format!("eval/.cache/graphify/{key}/graphs.json"))).unwrap();
  assert_eq!(set["inputs"]["graphify_version"], "graphify 9.9.9");
  let resumed = agent::run_batch(
    &RunnerConfig {
      batch_dir: fixture.path("eval/runs/second"),
      ..upgraded
    },
    &tasks,
    agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5),
  )
  .unwrap_err();
  assert!(
    matches!(&resumed, AgentError::IncumbentMismatch { runs, .. } if runs == "t97-incumbent.incumbent.r0"),
    "{resumed}"
  );
}

#[test]
fn should_refuse_the_batch_when_the_probe_shows_no_serena_hook_and_fail_a_run_whose_hooks_did_not_run() {
  let fixture = fixture();
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  write(&fixture.path("no-hooks"), "");
  let refused = agent::run_batch(&fixture.config("refused"), &tasks, agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5)).unwrap_err();
  assert!(
    matches!(&refused, AgentError::IncumbentUnavailable { reason } if reason.starts_with("the incumbent probe session reported no SessionStart hook event")),
    "{refused}"
  );
  assert!(!run_dir(&fixture, "refused", Condition::Incumbent).exists());
  let unprobed = agent::run_batch(
    &RunnerConfig {
      probe: false,
      ..fixture.config("unprobed")
    },
    &tasks,
    agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5),
  )
  .unwrap_err();
  assert!(unprobed.to_string().contains("drop --no-probe"), "{unprobed}");

  fs::remove_file(fixture.path("no-hooks")).unwrap();
  write(&fixture.path("no-run-hooks"), "");
  agent::run_batch(&fixture.config("unhooked"), &tasks, agent::plan_runs(&tasks, &[Condition::Incumbent], 1, 5)).unwrap();
  let run: RunManifest = agent::read_json(&run_dir(&fixture, "unhooked", Condition::Incumbent).join(RUN_FILE)).unwrap();
  assert_eq!(
    run.status,
    RunStatus::SetupFailed {
      stage: "incumbent-hooks".to_owned(),
      reason: "the session reported no SessionStart hook event, so the Serena hooks did not run".to_owned()
    }
  );
}

#[test]
#[ignore = "runs the real graphify found on PATH"]
fn should_merge_the_linked_graph_under_the_linked_repo_name_with_the_real_graphify() {
  let Ok(binary) = resolve_binary(Path::new("graphify"), std::env::var_os("PATH").as_deref()) else {
    return;
  };
  let fixture = fixture();
  let task = agent::load_tasks(&fixture.tasks, &[]).unwrap().remove(0);
  let work = fixture.path("trees/work");
  let graphs = agent::ensure_graphs(&GraphBuild {
    graphify: &Graphify {
      binary,
      timeout: Duration::from_secs(300),
    },
    graphify_version: None,
    task: &task,
    source: &TreeSource::Directory { path: fixture.source.clone() },
    strip: &StripPlan::for_task(&task, true),
    cache_dir: &fixture.path("eval/.cache"),
    work: &work,
  })
  .unwrap();
  assert_eq!(graphs.set.roots, vec![".", "apps/acme-api"]);
  let origins = |graph: &serde_json::Value| -> Vec<(String, String, String)> {
    let mut origins: Vec<(String, String, String)> = graph["nodes"]
      .as_array()
      .unwrap()
      .iter()
      .filter(|node| node["source_file"].as_str().is_some_and(|file| file.ends_with(".ts")))
      .map(|node| {
        let id = node["id"].as_str().unwrap();
        let prefix = id.split_once("::").map_or("", |(prefix, _)| prefix);
        (
          prefix.to_owned(),
          node["repo"].as_str().unwrap_or_default().to_owned(),
          node["source_file"].as_str().unwrap().to_owned(),
        )
      })
      .collect();
    origins.sort();
    origins.dedup();
    origins
  };
  let graph: serde_json::Value = agent::read_json(&graphs.dir.join("graphify-out/graph.json")).unwrap();
  let tree_entry = (String::new(), String::new(), "apps/acme-api/src/orders.ts".to_owned());
  let web_entry = ("web".to_owned(), "web".to_owned(), "apps/acme-web/src/api/orders.ts".to_owned());
  assert_eq!(origins(&graph), vec![tree_entry.clone(), web_entry]);
  let linked: serde_json::Value = agent::read_json(&graphs.dir.join("graphify-out/linked-web.json")).unwrap();
  assert_eq!(origins(&linked), vec![(String::new(), String::new(), "apps/acme-web/src/api/orders.ts".to_owned())]);
  assert!(graphs.dir.join("graphify-out/GRAPH_REPORT.md").is_file());
  assert!(!graphs.dir.join("graphify-out/cache").exists() && !graphs.dir.join("graphify-out/graph.html").exists());
  assert_eq!(fs::read_dir(work.join("home")).unwrap().count(), 0, "graphify wrote under its sandboxed HOME");

  let session = fixture.path("trees/session/tree");
  agent::materialize_session_tree(&task, &TreeSource::Directory { path: fixture.source.clone() }, &session, &StripPlan::for_task(&task, true)).unwrap();
  agent::install_graphs(&graphs, &session).unwrap();
  let before = graph["nodes"].as_array().unwrap().iter().filter(|node| node["repo"].is_null()).count();
  let home = fixture.path("trees/home");
  fs::create_dir_all(&home).unwrap();
  let graphify = |args: &[&str]| {
    let output = std::process::Command::new("graphify").args(args).current_dir(&session).env("HOME", &home).output().unwrap();
    assert!(output.status.success(), "graphify {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
  };
  graphify(&["update", "."]);
  let updated: serde_json::Value = agent::read_json(&session.join("graphify-out/graph.json")).unwrap();
  let ids: Vec<&str> = updated["nodes"].as_array().unwrap().iter().filter_map(|node| node["id"].as_str()).collect();
  let distinct: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
  assert_eq!(distinct.len(), ids.len(), "{ids:?}");
  assert!(ids.iter().all(|id| !id.contains("::")), "the update keeps this repository only: {ids:?}");
  assert_eq!(origins(&updated), vec![tree_entry]);
  assert_eq!(ids.len(), before, "every session tree node was replaced, none duplicated: {ids:?}");
  let answer = graphify(&["query", "orders", "--graph", "graphify-out/linked-web.json"]);
  assert!(answer.contains("apps/acme-web/src/api/orders.ts"), "{answer}");
}
