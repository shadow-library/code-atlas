use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use atlas_eval::agent::{
  self, AgentError, BATCH_FILE, BatchManifest, ClaudeConfig, Condition, ConfigIsolation, DEFAULT_JUDGE_MODEL, JudgeMode, JudgeOutcome, JudgeSettings, LoadedTask, PreambleVariant,
  RUN_FILE, RejudgeRequest, RunManifest, RunnerConfig, SCORE_FILE, Score, SurfaceSetting,
};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "0.0.0-fake (Claude Code)"; exit 0; fi
mcp=""
schema=""
model=""
while [ $# -gt 0 ]; do
  case "$1" in
    --model) model="$2"; shift 2 ;;
    --mcp-config) mcp="$2"; shift 2 ;;
    --json-schema) schema="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ ! -f "$mcp" ]; then echo "MCP config file not found: $mcp" >&2; exit 1; fi
if [ -n "$schema" ]; then
  cat > /dev/null
  echo "$model" >> "$(dirname "$0")/judge-models.log"
  if [ -f "$(dirname "$0")/judge-fails" ]; then echo "judge unavailable" >&2; exit 1; fi
  case "$schema" in
    *misleading*) printf '%s\n' '{"type":"result","is_error":false,"structured_output":{"misleading":false,"examples":[],"summary":"fine"},"total_cost_usd":0.02,"modelUsage":{"claude-sonnet-5-5":{}}}' ;;
    *)
      c='{"score":4,"rationale":"ok"}'
      printf '%s\n' "{\"type\":\"result\",\"is_error\":false,\"structured_output\":{\"correctness\":$c,\"completeness\":$c,\"architectural_consistency\":$c,\"appropriate_reuse\":$c,\"blast_radius_handling\":$c},\"total_cost_usd\":0.03,\"modelUsage\":{\"claude-sonnet-5-5\":{}}}" ;;
  esac
  exit 0
fi
servers='[]'
if grep -q code-atlas "$mcp"; then servers='[{"name":"code-atlas","status":"connected"}]'; fi
echo "export function cancelOrder(id: string) { return id; }" >> src/orders.ts
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"$PWD\",\"tools\":[\"Read\",\"Edit\"],\"mcp_servers\":$servers,\"model\":\"claude-haiku-4-5-20251001\",\"permissionMode\":\"bypassPermissions\",\"claude_code_version\":\"0.0.0-fake\"}"
if [ "$servers" != '[]' ]; then
  printf '%s\n' '{"type":"assistant","message":{"id":"m1","model":"claude-haiku-4-5-20251001","content":[{"type":"tool_use","id":"t1","name":"mcp__code-atlas__atlas_context","input":{"task":"cancel an order"}}],"usage":{"input_tokens":1,"output_tokens":1}}}'
  printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"MUST READ\n  tree:src/orders.ts:1 getOrder [resolved]\n"}]}}'
fi
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"duration_ms":10,"num_turns":2,"total_cost_usd":0.01,"usage":{"input_tokens":2,"output_tokens":2}}'
"#;

const FAKE_ATLAS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "catl 0.0.0-fake" ;;
  project) [ -n "$CODE_ATLAS_HOME" ] || exit 9 ;;
  index) echo "indexed" ;;
  *) exit 2 ;;
esac
"#;

const RECORDED_JUDGE_MODEL: &str = "claude-haiku-5-5";

const TASK: &str = r#"
id = "t96-judge"
title = "Judge"
repo = { fixture = "acme" }
base_commit = "e5209b4"
prompt = "Add cancelOrder to the orders API."
timeout_minutes = 1
max_turns = 5
"#;

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

fn relative(path: &Path) -> PathBuf {
  let depth = std::env::current_dir()
    .unwrap()
    .components()
    .filter(|component| matches!(component, Component::Normal(_)))
    .count();
  let mut relative: PathBuf = std::iter::repeat_n("..", depth).collect();
  relative.push(path.strip_prefix("/").unwrap());
  relative
}

struct Fixture {
  root: tempfile::TempDir,
}

impl Fixture {
  fn path(&self, name: &str) -> PathBuf {
    self.root.path().join(name)
  }

  fn claude(&self) -> ClaudeConfig {
    ClaudeConfig {
      binary: self.path("bin/claude"),
      model: "claude-haiku-4-5-20251001".to_owned(),
      judge_model: RECORDED_JUDGE_MODEL.to_owned(),
      config_isolation: ConfigIsolation::User,
    }
  }

  fn config(&self, batch: &str) -> RunnerConfig {
    RunnerConfig {
      repo_root: self.path("source"),
      eval_home: relative(&self.path("eval")),
      batch_dir: relative(&self.path(&format!("eval/runs/{batch}"))),
      tree_root: self.path("trees"),
      keep_trees: false,
      cache_dir: relative(&self.path("eval/runs/.cache")),
      claude: self.claude(),
      atlas_bin: self.path("bin/catl"),
      repo_override: Some(self.path("source")),
      judge: JudgeMode::Claude,
      parallel: 1,
      budget_usd: None,
      validation_timeout: Duration::from_secs(60),
      atlas_timeout: Duration::from_secs(60),
      seed: 3,
      probe: false,
      preamble: PreambleVariant::Standard,
      atlas_config: None,
      atlas_hooks: false,
      atlas_surface: SurfaceSetting::Standard,
      incumbent: None,
    }
  }

  fn run(&self, batch: &str, condition: Condition) -> PathBuf {
    self.path(&format!("eval/runs/{batch}/t96-judge.{condition}.r0"))
  }
}

fn fixture() -> (Fixture, Vec<LoadedTask>) {
  let root = tempfile::tempdir().unwrap();
  write(&root.path().join("eval/tasks/t96-judge/task.toml"), TASK);
  write(&root.path().join("source/src/orders.ts"), "export function getOrder(id: string) { return id; }\n");
  executable(&root.path().join("bin/claude"), FAKE_CLAUDE);
  executable(&root.path().join("bin/catl"), FAKE_ATLAS);
  fs::create_dir_all(root.path().join("trees")).unwrap();
  let tasks = agent::load_tasks(&root.path().join("eval/tasks"), &[]).unwrap();
  (Fixture { root }, tasks)
}

fn judge_of(dir: &Path) -> JudgeOutcome {
  agent::read_json::<Score>(&dir.join(SCORE_FILE)).unwrap().judge
}

fn review_of(dir: &Path) -> JudgeOutcome {
  agent::read_json(&dir.join("atlas-review.json")).unwrap()
}

fn cost_of(dir: &Path) -> f64 {
  agent::read_json::<RunManifest>(&dir.join(RUN_FILE)).unwrap().cost_usd
}

fn close(actual: f64, expected: f64) -> bool {
  (actual - expected).abs() < 1e-9
}

#[test]
fn should_judge_runs_of_a_batch_given_by_relative_paths() {
  let (fixture, tasks) = fixture();
  agent::run_batch(&fixture.config("judged"), &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 3)).unwrap();
  for condition in Condition::ALL {
    let judge = judge_of(&fixture.run("judged", condition));
    assert!(matches!(judge, JudgeOutcome::Scored { .. }), "{condition}: {judge:?}");
  }
  assert!(matches!(review_of(&fixture.run("judged", Condition::Atlas)), JudgeOutcome::Reviewed { .. }));
  assert!(close(cost_of(&fixture.run("judged", Condition::Atlas)), 0.01 + 0.03 + 0.02));
}

#[test]
fn should_rejudge_only_failed_judges_in_place_without_a_session() {
  let (fixture, tasks) = fixture();
  write(&fixture.path("bin/judge-fails"), "");
  agent::run_batch(&fixture.config("failed"), &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 3)).unwrap();
  let (baseline, atlas) = (fixture.run("failed", Condition::Baseline), fixture.run("failed", Condition::Atlas));
  assert!(matches!(judge_of(&baseline), JudgeOutcome::Failed { .. }));
  assert!(matches!(review_of(&atlas), JudgeOutcome::Failed { .. }));
  assert!(close(cost_of(&atlas), 0.01));
  let batch_path = fixture.path("eval/runs/failed").join(BATCH_FILE);
  let spent_before = agent::read_json::<BatchManifest>(&batch_path).unwrap().spent_usd;
  let session_before = fs::read_to_string(atlas.join("transcript.jsonl")).unwrap();
  fs::remove_file(fixture.path("bin/judge-fails")).unwrap();
  fs::remove_file(fixture.path("bin/judge-models.log")).unwrap();

  let batch_dir = fixture.path("eval/runs/failed");
  let recorded = agent::recorded_environment(&batch_dir).unwrap();
  let binary = fixture.path("bin/claude");
  let settings = |judge_model: Option<&'static str>, config_isolation: Option<ConfigIsolation>| JudgeSettings {
    binary: &binary,
    judge_model,
    config_isolation,
  };
  let claude = agent::rejudge_claude(recorded.as_ref(), &settings(None, None)).unwrap();
  assert_ne!(RECORDED_JUDGE_MODEL, DEFAULT_JUDGE_MODEL);
  assert_eq!((claude.judge_model.as_str(), claude.config_isolation), (RECORDED_JUDGE_MODEL, ConfigIsolation::User));
  assert_eq!(
    agent::rejudge_claude(recorded.as_ref(), &settings(Some(RECORDED_JUDGE_MODEL), Some(ConfigIsolation::User))).unwrap(),
    claude
  );
  assert_eq!(
    agent::rejudge_claude(recorded.as_ref(), &settings(Some(DEFAULT_JUDGE_MODEL), None))
      .unwrap_err()
      .to_string(),
    format!("--judge-model {DEFAULT_JUDGE_MODEL} differs from the {RECORDED_JUDGE_MODEL} the batch was judged with; a rejudge uses the batch's judge settings")
  );
  assert!(matches!(
    agent::rejudge_claude(recorded.as_ref(), &settings(None, Some(ConfigIsolation::Isolated))),
    Err(AgentError::JudgeSettingConflict { .. })
  ));
  assert_eq!(agent::rejudge_claude(None, &settings(None, None)).unwrap().judge_model, DEFAULT_JUDGE_MODEL);
  let request = |all: bool, tasks: &[LoadedTask]| {
    agent::rejudge(&RejudgeRequest {
      batch_dir: &relative(&fixture.path("eval/runs/failed")),
      tasks,
      only: &[],
      all,
      claude: &claude,
    })
    .unwrap()
  };
  let held = agent::lock_batch(&batch_dir).unwrap();
  let busy = agent::rejudge(&RejudgeRequest {
    batch_dir: &batch_dir,
    tasks: &tasks,
    only: &[],
    all: false,
    claude: &claude,
  })
  .unwrap_err();
  assert!(matches!(busy, AgentError::BatchInProgress(_)), "{busy}");
  drop(held);

  let summary = request(false, &tasks);
  assert_eq!(
    fs::read_to_string(fixture.path("bin/judge-models.log")).unwrap(),
    format!("{RECORDED_JUDGE_MODEL}\n").repeat(3)
  );
  assert_eq!(
    (summary.rubrics, summary.reviews, summary.still_failed.clone(), summary.skipped.clone()),
    (2, 1, Vec::new(), Vec::new())
  );
  assert!(close(summary.spent_usd, 0.03 + 0.03 + 0.02));
  assert!(matches!(judge_of(&baseline), JudgeOutcome::Scored { .. }));
  assert!(matches!(judge_of(&atlas), JudgeOutcome::Scored { .. }));
  assert!(matches!(review_of(&atlas), JudgeOutcome::Reviewed { .. }));
  assert!(close(cost_of(&baseline), 0.01 + 0.03));
  assert!(close(cost_of(&atlas), 0.01 + 0.03 + 0.02));
  assert!(close(agent::read_json::<BatchManifest>(&batch_path).unwrap().spent_usd, spent_before + 0.08));
  assert_eq!(fs::read_to_string(atlas.join("transcript.jsonl")).unwrap(), session_before);

  let nothing_failed = request(false, &tasks);
  assert_eq!((nothing_failed.rubrics, nothing_failed.reviews), (0, 0));
  let everything = request(true, &tasks);
  assert_eq!((everything.rubrics, everything.reviews), (2, 1));

  let mut edited = tasks.clone();
  edited[0].spec_hash = "edited".to_owned();
  let skipped = request(true, &edited);
  assert_eq!((skipped.rubrics, skipped.skipped.len()), (0, 2));
  assert_eq!(skipped.skipped[0], "t96-judge.atlas.r0: no task spec with id `t96-judge` and the recorded spec hash");
}
