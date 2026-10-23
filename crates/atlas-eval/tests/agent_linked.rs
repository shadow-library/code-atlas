use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_core::{EdgeKind, EntityKey, RepoName};
use atlas_engine::{Engine, EngineOptions};
use atlas_eval::agent::{
  self, AtlasSurface, BaseCheck, ClaudeConfig, Condition, ConfigIsolation, JudgeMode, LinkedLayout, LinkedMetrics, LinkedRestore, Metrics, PreambleVariant, RUN_FILE, RepoGlobs,
  RunManifest, RunStatus, RunnerConfig, SCORE_FILE, Score, SessionOutcome, StripPlan, SurfaceSetting, TreeSource,
};
use atlas_project::{AtlasHome, Registry};
use atlas_store::EdgeFilter;
use atlas_test_lsp::hermetic::{catl_binary, hermetic_catl};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "0.0.0-fake (Claude Code)"; exit 0; fi
mcp=""
while [ $# -gt 0 ]; do
  case "$1" in
    --mcp-config) mcp="$2"; shift 2 ;;
    *) shift ;;
  esac
done
servers='[]'
if grep -q code-atlas "$mcp"; then servers='[{"name":"code-atlas","status":"connected"}]'; fi
find . -path ./.git -prune -o -type f -print | sort | sed 's/^/FILE:/' >&2
echo "STATUS:$(git status --porcelain --untracked-files=all)" >&2
cat > apps/acme-api/src/orders.ts <<'TS'
export function getOrder(id: string) { return id; }
export function cancelOrder(id: string) { return id; }
TS
mkdir -p apps/acme-web/src
echo "export const guessed = '/orders/cancel';" > apps/acme-web/src/guess.ts
view="$(dirname "$PWD")/linked-view/apps/acme-web/src/api/orders.ts"
if [ -f "$PWD/../../../tamper" ]; then
  mv "$(dirname "$PWD")/linked-hold/apps/acme-web" "$(dirname "$PWD")/moved-web"
  [ -f "$view" ] && echo "export const edited = 1;" >> "$view"
fi
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"$PWD\",\"tools\":[\"Read\",\"Edit\"],\"mcp_servers\":$servers,\"model\":\"claude-haiku-4-5-20251001\",\"permissionMode\":\"bypassPermissions\",\"claude_code_version\":\"0.0.0-fake\"}"
if [ "$servers" != '[]' ]; then
  printf '%s\n' '{"type":"assistant","message":{"id":"m1","model":"claude-haiku-4-5-20251001","content":[{"type":"tool_use","id":"t1","name":"mcp__code-atlas__atlas_context","input":{"task":"cancel an order"}}],"usage":{"input_tokens":1,"output_tokens":1}}}'
  printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"MUST READ\n  web:apps/acme-web/src/api/orders.ts:2 cancelPath [resolved]\n"}]}}'
  printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"model\":\"claude-haiku-4-5-20251001\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"Read\",\"input\":{\"file_path\":\"$view\"}}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}"
  printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"1\texport const ordersPath"}]}}'
fi
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"duration_ms":10,"num_turns":2,"total_cost_usd":0.01,"usage":{"input_tokens":2,"output_tokens":2}}'
"#;

const FAKE_ATLAS: &str = r#"#!/bin/sh
[ -n "$CODE_ATLAS_HOME" ] && echo "$*" >> "$CODE_ATLAS_HOME/catl-args.txt"
case "$1" in
  --version) echo "catl 0.0.0-fake" ;;
  project) [ -n "$CODE_ATLAS_HOME" ] || exit 9 ;;
  index) echo "indexed" ;;
  *) exit 2 ;;
esac
"#;

const TASK: &str = r#"
id = "t98-linked"
title = "Linked"
repo = { fixture = "acme" }
base_commit = "e5209b4"
prompt = "Add cancelOrder to the orders API; the web app already calls it."
timeout_minutes = 1
max_turns = 5
setup = ["mkdir -p apps/acme-web/node_modules && echo built > apps/acme-web/node_modules/marker"]
[linked]
paths = ["apps/acme-web"]
patch = "linked.patch"
repo = "web"
[validation]
hidden_tests = ["hidden/apps/acme-api/check.sh"]
commands = ["grep -q /cancel apps/acme-web/src/api/orders.ts && test -f apps/acme-web/node_modules/marker", "sh apps/acme-api/check.sh"]
regression = ["true"]
[ground_truth]
relevant_files = ["apps/acme-api/src/orders.ts", "apps/acme-web/src/api/orders.ts"]
must_change = ["apps/acme-api/src/orders.ts"]
must_consider = ["apps/acme-web/src/api/orders.ts"]
"#;

const LINKED_PATCH: &str = "--- a/apps/acme-web/src/api/orders.ts
+++ b/apps/acme-web/src/api/orders.ts
@@ -1 +1,2 @@
 export const ordersPath = '/orders';
+export const cancelPath = '/orders/:id/cancel';
";

const CHECK: &str = "grep -q cancelOrder apps/acme-api/src/orders.ts && test ! -e apps/acme-web/src/guess.ts\n";

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

  fn config(&self) -> RunnerConfig {
    let bin = self.path("bin");
    RunnerConfig {
      repo_root: self.source.clone(),
      eval_home: self.path("eval"),
      batch_dir: self.path("eval/runs/batch"),
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
      probe: false,
      preamble: PreambleVariant::Standard,
      atlas_config: None,
      atlas_hooks: false,
      atlas_surface: SurfaceSetting::Standard,
    }
  }
}

fn fixture() -> Fixture {
  let root = tempfile::tempdir().unwrap();
  let tasks = root.path().join("tasks");
  write(&tasks.join("t98-linked/task.toml"), TASK);
  write(&tasks.join("t98-linked/linked.patch"), LINKED_PATCH);
  write(&tasks.join("t98-linked/hidden/apps/acme-api/check.sh"), CHECK);
  let source = root.path().join("source");
  write(&source.join("package.json"), "{\"private\": true, \"workspaces\": [\"apps/*\"]}\n");
  write(&source.join("apps/acme-api/src/orders.ts"), "export function getOrder(id: string) { return id; }\n");
  write(&source.join("apps/acme-web/src/api/orders.ts"), "export const ordersPath = '/orders';\n");
  executable(&root.path().join("bin/claude"), FAKE_CLAUDE);
  executable(&root.path().join("bin/catl"), FAKE_ATLAS);
  fs::create_dir_all(root.path().join("trees")).unwrap();
  Fixture { root, tasks, source }
}

fn lines_with(log: &str, prefix: &str) -> Vec<String> {
  log.lines().filter_map(|line| line.strip_prefix(prefix)).map(str::to_owned).collect()
}

#[test]
fn should_hide_linked_paths_from_both_sessions_index_them_for_atlas_and_restore_them_for_validation() {
  let fixture = fixture();
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  let manifest = agent::run_batch(&fixture.config(), &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 5)).unwrap();
  assert_eq!(
    manifest.environment.linked_paths,
    BTreeMap::from([("t98-linked".to_owned(), vec!["apps/acme-web".to_owned()])])
  );

  let base: BaseCheck = agent::read_json(&fixture.path("eval/runs/batch/_base/t98-linked/base.json")).unwrap();
  assert_eq!(
    base.validation.passed_commands(),
    1,
    "the base check must see the restored linked code: {:?}",
    base.validation
  );
  assert!(!base.validation.all_passed());

  for condition in Condition::ALL {
    let dir = fixture.path(&format!("eval/runs/batch/t98-linked.{condition}.r0"));
    let run: RunManifest = agent::read_json(&dir.join(RUN_FILE)).unwrap();
    assert_eq!(
      run.status,
      RunStatus::Completed {
        session: SessionOutcome::Finished { subtype: "success".to_owned() }
      },
      "{condition}"
    );
    let session_log = fs::read_to_string(dir.join("claude.stderr.log")).unwrap();
    assert_eq!(lines_with(&session_log, "FILE:"), vec!["./apps/acme-api/src/orders.ts", "./package.json"], "{condition}");
    assert_eq!(lines_with(&session_log, "STATUS:"), vec![""], "{condition}");

    let tree = PathBuf::from(run.tree.clone().unwrap());
    let scratch = tree.parent().unwrap();
    assert_eq!(
      fs::read_to_string(tree.join("apps/acme-web/src/api/orders.ts")).unwrap(),
      "export const ordersPath = '/orders';\nexport const cancelPath = '/orders/:id/cancel';\n"
    );
    assert_eq!(fs::read_to_string(tree.join("apps/acme-web/node_modules/marker")).unwrap(), "built\n");
    assert!(!tree.join("apps/acme-web/src/guess.ts").exists());
    assert_eq!(fs::read_to_string(tree.join(".git/info/exclude")).unwrap(), "target/\nnode_modules/\n.code-atlas/\n");
    let diff = fs::read_to_string(dir.join("diff.patch")).unwrap();
    assert!(diff.contains("apps/acme-api/src/orders.ts") && !diff.contains("acme-web"), "{condition}: {diff}");
    assert_eq!(
      fs::read_to_string(scratch.join("linked-conflict/apps/acme-web/src/guess.ts")).unwrap(),
      "export const guessed = '/orders/cancel';\n"
    );
    assert_eq!(scratch.join("linked-view/package.json").is_file(), condition == Condition::Atlas, "{condition}");

    let score: Score = agent::read_json(&dir.join(SCORE_FILE)).unwrap();
    assert!(score.hidden_tests.as_ref().unwrap().passed, "{condition}: {:?}", score.validation);
    let metrics: Metrics = agent::read_json(&dir.join("metrics.json")).unwrap();
    assert_eq!(metrics.relevance.files_changed, vec!["apps/acme-api/src/orders.ts"], "{condition}");
    assert_eq!(
      metrics.relevance.relevant_read,
      if condition == Condition::Atlas {
        vec!["apps/acme-web/src/api/orders.ts".to_owned()]
      } else {
        Vec::new()
      },
      "{condition}"
    );
    assert!(!metrics.contaminated, "{condition}: {:?}", metrics.out_of_tree_accesses);
    let atlas = condition == Condition::Atlas;
    assert_eq!(
      metrics.linked,
      Some(LinkedMetrics {
        linked_reads: u64::from(atlas),
        linked_source_calls: u64::from(atlas),
        linked_path_created: true,
        ..LinkedMetrics::default()
      }),
      "{condition}"
    );
    assert_eq!(
      metrics.out_of_tree_accesses.iter().map(|access| access.linked).collect::<Vec<_>>(),
      if atlas { vec![Some(agent::LinkedAccess::Read)] } else { Vec::new() },
      "{condition}"
    );
    if atlas {
      let project = run.atlas_project.clone().unwrap();
      let view = scratch.join("linked-view");
      assert_eq!(
        fs::read_to_string(dir.join("atlas-home/catl-args.txt")).unwrap(),
        format!(
          "project create {project} --repo {}\nproject add-repo {project} {} --name web --include apps/acme-web/** --exclude /apps/acme-api\nindex --project {project}\n",
          tree.display(),
          view.display()
        )
      );
      assert!(!view.join("apps/acme-web/node_modules").exists(), "setup never runs in the view");
      assert_eq!(run.atlas_surface, AtlasSurface::Standard);
    }
  }
}

#[test]
fn should_score_a_run_that_moved_the_held_linked_paths_or_wrote_the_view_as_contaminated_without_validation() {
  let fixture = fixture();
  write(&fixture.path("tamper"), "");
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  agent::run_batch(&fixture.config(), &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 5)).unwrap();

  for condition in Condition::ALL {
    let dir = fixture.path(&format!("eval/runs/batch/t98-linked.{condition}.r0"));
    let run: RunManifest = agent::read_json(&dir.join(RUN_FILE)).unwrap();
    assert!(matches!(run.status, RunStatus::Completed { .. }), "{condition}: {:?}", run.status);
    let score: Score = agent::read_json(&dir.join(SCORE_FILE)).unwrap();
    assert_eq!(
      (score.validation.commands.len(), score.regression.commands.len(), score.hidden_tests),
      (0, 0, None),
      "{condition}"
    );
    let metrics: Metrics = agent::read_json(&dir.join("metrics.json")).unwrap();
    assert!(metrics.contaminated, "{condition}");
    let linked = metrics.linked.unwrap();
    let mut writes = vec!["linked-hold/apps/acme-web".to_owned()];
    if condition == Condition::Atlas {
      writes.push("linked-view/apps/acme-web/src/api/orders.ts".to_owned());
    }
    assert_eq!(linked.linked_writes, writes, "{condition}");
    assert_eq!(
      linked.restore,
      LinkedRestore::HoldMissing {
        paths: vec!["apps/acme-web".to_owned()]
      },
      "{condition}"
    );
    let tree = PathBuf::from(run.tree.unwrap());
    assert!(!tree.join("apps/acme-web/src/api/orders.ts").exists(), "{condition}");
  }
}

#[test]
fn should_index_the_linked_paths_from_the_view_and_resolve_their_imports_into_workspace_packages_of_the_tree() {
  let fixture = fixture();
  write(&fixture.source.join("package.json"), "{\"private\": true, \"workspaces\": [\"apps/*\", \"packages/*\"]}\n");
  write(
    &fixture.source.join("apps/acme-web/package.json"),
    "{\"name\": \"@acme/web\", \"dependencies\": {\"@acme/shared\": \"workspace:*\"}}\n",
  );
  write(
    &fixture.source.join("apps/acme-web/src/api/format.ts"),
    "import { formatOrder } from '@acme/shared';\n\nexport const label = formatOrder('1');\n",
  );
  write(
    &fixture.source.join("packages/acme-shared/package.json"),
    "{\"name\": \"@acme/shared\", \"main\": \"src/index.ts\"}\n",
  );
  write(
    &fixture.source.join("packages/acme-shared/src/index.ts"),
    "export function formatOrder(id: string) { return `#${id}`; }\n",
  );
  let task = agent::load_tasks(&fixture.tasks, &[]).unwrap().remove(0);
  let scratch = tempfile::tempdir().unwrap();
  let source = TreeSource::Directory { path: fixture.source.clone() };
  let strip = StripPlan::for_task(&task, true);
  let tree = scratch.path().join("tree");
  agent::materialize_task(&task, &source, &tree, &strip).unwrap();
  let linked = LinkedLayout::of(&task, scratch.path()).unwrap();
  linked.hide(&tree).unwrap();
  linked.materialize_view(&task, &source, &strip).unwrap();
  write(&tree.join("apps/acme-web/src/guess.ts"), "export const guessed = 1;\n");
  let globs = RepoGlobs {
    include: linked.spec.include_globs(),
    exclude: linked.view_excludes().unwrap(),
  };
  assert_eq!(globs.exclude, vec!["/packages", "/apps/acme-api"]);
  let held = linked.snapshot();
  let home = scratch.path().join("atlas-home");
  let catl = |args: &[String]| {
    let output = hermetic_catl(catl_binary(), &home).args(args).current_dir(&tree).output().unwrap();
    assert!(output.status.success(), "catl {args:?}: {}", String::from_utf8_lossy(&output.stderr));
  };

  catl(
    &["project", "create", "linked", "--repo"]
      .map(str::to_owned)
      .into_iter()
      .chain([tree.display().to_string()])
      .collect::<Vec<_>>(),
  );
  catl(&agent::set_repo_exclude_args("linked", "tree", &linked.spec.exclude_globs()));
  catl(&agent::add_repo_args("linked", &linked.view, &linked.spec.repo, &globs));

  let atlas_home = AtlasHome::rooted_at(&home);
  let project = Registry::load(&atlas_home).unwrap().project("linked").unwrap().clone();
  let repos: Vec<(String, Vec<String>, Vec<String>)> = project
    .repos
    .iter()
    .map(|repo| (repo.name.to_string(), repo.include.clone(), repo.exclude.clone()))
    .collect();
  assert_eq!(
    repos,
    vec![
      ("tree".to_owned(), Vec::new(), vec!["/apps/acme-web".to_owned()]),
      (
        "web".to_owned(),
        vec!["apps/acme-web/**".to_owned()],
        vec!["/packages".to_owned(), "/apps/acme-api".to_owned()]
      ),
    ]
  );
  let engine = Engine::open(&project, &atlas_home, EngineOptions::default()).unwrap();
  engine.ensure_fresh().unwrap();
  let files = |repo: &str| -> Vec<String> {
    let repo = RepoName::new(repo).unwrap();
    engine
      .store()
      .read(|tx| tx.files_in_repo(&repo))
      .unwrap()
      .into_iter()
      .map(|record| record.path.to_string())
      .collect()
  };
  assert_eq!(
    files("web"),
    vec![
      "apps/acme-web/package.json",
      "apps/acme-web/src/api/format.ts",
      "apps/acme-web/src/api/orders.ts",
      "package.json"
    ]
  );
  assert_eq!(
    files("tree"),
    vec![
      "apps/acme-api/src/orders.ts",
      "package.json",
      "packages/acme-shared/package.json",
      "packages/acme-shared/src/index.ts"
    ]
  );
  let file_of = |key: &EntityKey| match key {
    EntityKey::File { repo, path } => Some(format!("{repo}:{path}")),
    _ => None,
  };
  let imports: Vec<(String, String)> = engine
    .store()
    .read(|tx| {
      let mut imports = Vec::new();
      for link in tx.edge_links_of_kind(&EdgeKind::Imports, &EdgeFilter::default())? {
        let (Some(from), Some(to)) = (tx.entity(link.from)?, tx.entity(link.to)?) else {
          continue;
        };
        if let (Some(from), Some(to)) = (file_of(&from.key), to.repo.zip(to.path).map(|(repo, path)| format!("{repo}:{path}"))) {
          imports.push((from, to));
        }
      }
      Ok::<_, atlas_store::StoreError>(imports)
    })
    .unwrap();
  assert!(
    imports.contains(&("web:apps/acme-web/src/api/format.ts".to_owned(), "tree:packages/acme-shared/src/index.ts".to_owned())),
    "{imports:?}"
  );
  assert_eq!(
    held.changes(&linked.snapshot()),
    Vec::<String>::new(),
    "indexing must write nothing into the view or the hold"
  );
}
