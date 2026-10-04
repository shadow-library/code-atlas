use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_eval::agent::{
  self, ClaudeConfig, Condition, ConfigIsolation, DuplicationCheck, JudgeMode, Metrics, RUN_FILE, RunManifest, RunStatus, RunnerConfig, SCORE_FILE, Score, SessionOutcome,
  TargetRole,
};

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
git rev-parse --show-toplevel >&2
pwd -P >&2
if grep -q code-atlas "$mcp"; then servers='[{"name":"code-atlas","status":"connected"}]'; fi
cat > acme-api/src/users/user.entity.ts <<'TS'
export class User {
  id!: string;

  @DeleteDateColumn({ name: 'deleted_at', nullable: true })
  deletedAt!: Date | null;
}
TS
cat > acme-api/src/users/users.service.ts <<'TS'
export class UsersService {
  async findOne(id: string): Promise<User> {
    const user = await this.users.findOneBy({ id });
    return user;
  }

  async findActive(id: string): Promise<User> {
    const user = await this.users.findOneBy({ id });
    return user;
  }
}
TS
entity="$PWD/acme-api/src/users/user.entity.ts"
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"$PWD\",\"tools\":[\"Read\",\"Edit\"],\"mcp_servers\":$servers,\"model\":\"claude-haiku-4-5-20251001\",\"permissionMode\":\"bypassPermissions\",\"claude_code_version\":\"0.0.0-fake\"}"
printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"claude-haiku-4-5-20251001\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Read\",\"input\":{\"file_path\":\"$entity\"}}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":20}}}"
printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"1\texport class User {\n2\t}"}]}}'
printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"model\":\"claude-haiku-4-5-20251001\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"Edit\",\"input\":{\"file_path\":\"$entity\"}}],\"usage\":{\"input_tokens\":12,\"output_tokens\":40,\"cache_read_input_tokens\":120,\"cache_creation_input_tokens\":0}}}"
printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"duration_ms":1500,"num_turns":3,"total_cost_usd":0.01,"usage":{"input_tokens":22,"output_tokens":45,"cache_read_input_tokens":220,"cache_creation_input_tokens":20},"modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":22,"outputTokens":45,"cacheReadInputTokens":220,"cacheCreationInputTokens":20,"costUSD":0.01}}}'
"#;

const FAKE_ATLAS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "code-atlas 0.0.0-fake" ;;
  project) [ -n "$CODE_ATLAS_HOME" ] || exit 9 ;;
  index)
    [ -n "$CODE_ATLAS_HOME" ] || exit 9
    echo '{"files":4}' > "$CODE_ATLAS_HOME/index-run.json"
    echo data > "$CODE_ATLAS_HOME/atlas.db"
    echo "indexed 4 files"
    ;;
  query) echo '{"matches":[{"key":"ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method","structural_score":0.93,"location":"acme-api:src/users/users.service.ts:2"}]}' ;;
  *) exit 2 ;;
esac
"#;

const TASK: &str = r#"
id = "t99-pipeline"
title = "Pipeline"
repo = { fixture = "acme" }
base_commit = "e5209b4"
prompt = "Add deletedAt to User."
timeout_minutes = 1
max_turns = 5
atlas_repos = ["acme-api"]
[validation]
hidden_tests = ["hidden/acme-api/check.sh"]
commands = ["sh acme-api/check.sh"]
regression = ["true"]
[ground_truth]
relevant_files = ["acme-api/src/users/user.entity.ts", "acme-api/src/users/users.service.ts"]
must_change = ["acme-api/src/users/user.entity.ts"]
must_consider = ["acme-api/src/users/users.controller.ts"]
forbidden_duplicates = ["UsersService.findOne"]
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

struct Fixture {
  _root: tempfile::TempDir,
  tasks: PathBuf,
  source: PathBuf,
  bin: PathBuf,
  eval_home: PathBuf,
  batch: PathBuf,
  trees: PathBuf,
}

impl Fixture {
  fn config(&self, parallel: usize, budget_usd: Option<f64>) -> RunnerConfig {
    RunnerConfig {
      repo_root: self.source.clone(),
      eval_home: self.eval_home.clone(),
      batch_dir: self.batch.clone(),
      tree_root: self.trees.clone(),
      keep_trees: false,
      cache_dir: self.eval_home.join(".cache"),
      claude: ClaudeConfig {
        binary: self.bin.join("claude"),
        model: "claude-haiku-4-5-20251001".to_owned(),
        judge_model: "claude-sonnet-5-5".to_owned(),
        config_isolation: ConfigIsolation::User,
      },
      atlas_bin: self.bin.join("code-atlas"),
      repo_override: Some(self.source.clone()),
      judge: JudgeMode::Off,
      parallel,
      budget_usd,
      validation_timeout: Duration::from_secs(60),
      atlas_timeout: Duration::from_secs(60),
      seed: 3,
      probe: false,
    }
  }
}

fn fixture() -> Fixture {
  let root = tempfile::tempdir().unwrap();
  let tasks = root.path().join("tasks");
  write(&tasks.join("t99-pipeline/task.toml"), TASK);
  write(
    &tasks.join("t99-pipeline/hidden/acme-api/check.sh"),
    "grep -q \"name: 'deleted_at'\" acme-api/src/users/user.entity.ts\n",
  );
  let source = root.path().join("source");
  write(&source.join("acme-api/src/users/user.entity.ts"), "export class User {\n  id!: string;\n}\n");
  write(
    &source.join("acme-api/src/users/users.service.ts"),
    "export class UsersService {\n  async findOne(id: string): Promise<User> {\n    const user = await this.users.findOneBy({ id });\n    return user;\n  }\n}\n",
  );
  write(&source.join("acme-api/src/users/users.controller.ts"), "export class UsersController {}\n");
  write(&source.join("acme-sdk/src/index.ts"), "export {};\n");
  write(&source.join("CLAUDE.md"), "Use two-space indentation.\n");
  let bin = root.path().join("bin");
  executable(&bin.join("claude"), FAKE_CLAUDE);
  executable(&bin.join("code-atlas"), FAKE_ATLAS);
  let eval_home = root.path().join("eval");
  let batch = eval_home.join("runs/batch");
  let trees = root.path().join("trees");
  fs::create_dir_all(&trees).unwrap();
  Fixture {
    tasks,
    source,
    bin,
    eval_home,
    batch,
    trees,
    _root: root,
  }
}

#[test]
fn should_run_score_and_report_both_conditions_with_fake_binaries() {
  let fixture = fixture();
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  let planned = agent::plan_runs(&tasks, &Condition::ALL, 1, 3);
  let config = fixture.config(2, None);
  let manifest = agent::run_batch(&config, &tasks, planned).unwrap();
  assert_eq!(manifest.spent_usd, 0.02);
  assert_eq!(manifest.environment.code_atlas_version.as_deref(), Some("code-atlas 0.0.0-fake"));

  for condition in Condition::ALL {
    let dir = fixture.batch.join(format!("t99-pipeline.{condition}.r0"));
    let run: RunManifest = agent::read_json(&dir.join(RUN_FILE)).unwrap();
    assert_eq!(
      run.status,
      RunStatus::Completed {
        session: SessionOutcome::Finished { subtype: "success".to_owned() }
      },
      "{condition}"
    );
    assert_eq!(run.models_reported, vec!["claude-haiku-4-5-20251001"]);
    let tree = PathBuf::from(run.tree.clone().unwrap());
    let rendered = tree.display().to_string();
    assert!(!rendered.contains("atlas") && !rendered.contains("baseline") && !rendered.contains("/eval/"), "{rendered}");
    assert!(!tree.exists(), "scratch tree must be removed after scoring");
    let seen = fs::read_to_string(dir.join("claude.stderr.log")).unwrap();
    let mut lines = seen.lines();
    assert_eq!(lines.next(), lines.next(), "git toplevel must be the fresh tree repo");
    assert_eq!(run.project_instructions, vec!["CLAUDE.md"]);
    if condition == Condition::Atlas {
      let mcp: serde_json::Value = agent::read_json(&dir.join("mcp.json")).unwrap();
      let home = PathBuf::from(mcp["mcpServers"]["code-atlas"]["env"]["CODE_ATLAS_HOME"].as_str().unwrap());
      assert_eq!(home.parent(), tree.parent(), "the atlas home must sit beside the tree in the scratch dir");
      assert!(!home.display().to_string().contains("/eval/"));
      assert!(dir.join("atlas-home/index-run.json").is_file());
      assert!(!dir.join("atlas-home/atlas.db").exists());
      assert!(fs::read_to_string(dir.join("atlas-index.log")).unwrap().starts_with("indexed 4 files"));
    }
    assert!(dir.join("atlas-post-home/index-run.json").is_file());
    let position = run.args.iter().position(|arg| arg == "--append-system-prompt").unwrap();
    assert!(run.args[position + 1].contains("Use two-space indentation."));
    assert_eq!(run.atlas_index_ms.is_some(), condition == Condition::Atlas);
    let score: Score = agent::read_json(&dir.join(SCORE_FILE)).unwrap();
    assert!(score.hidden_tests.as_ref().unwrap().passed, "{condition}");
    assert_eq!(score.regression_count, Some(0));
    assert_eq!(score.must_change_coverage.ratio, Some(1.0));
    assert_eq!(score.must_consider_coverage.ratio, Some(0.0));
    assert_eq!(score.duplication_introduced, Some(true));
    let DuplicationCheck::Checked { new_functions, findings, .. } = &score.duplication else {
      panic!("duplication was not checked: {:?}", score.duplication);
    };
    assert_eq!(
      new_functions.iter().map(|function| function.item_path.as_str()).collect::<Vec<_>>(),
      vec!["UsersService::findActive"]
    );
    assert_eq!(findings[0].role, TargetRole::ForbiddenDuplicate);
    assert_eq!(score.atlas_accuracy.is_some(), condition == Condition::Atlas);
    let metrics: Metrics = agent::read_json(&dir.join("metrics.json")).unwrap();
    assert_eq!(metrics.reads.files, vec!["acme-api/src/users/user.entity.ts"]);
    assert_eq!(metrics.out_of_tree_accesses, Vec::<agent::OutOfTreeAccess>::new());
    assert!(!metrics.contaminated);
    assert_eq!(metrics.relevance.relevant_missed, Vec::<String>::new());
    assert_eq!(metrics.tokens.total.total(), 22 + 45 + 220 + 20);
    assert!(metrics.timing.first_edit_ms.is_some());
    assert_eq!(
      metrics.relevance.files_changed,
      vec!["acme-api/src/users/user.entity.ts", "acme-api/src/users/users.service.ts"]
    );
  }

  let base: agent::BaseCheck = agent::read_json(&fixture.batch.join("_base/t99-pipeline/base.json")).unwrap();
  assert!(!base.validation.all_passed());

  let (batch, records) = agent::load_runs(&fixture.batch).unwrap();
  let report = agent::build_report(batch.as_ref(), &records, None);
  assert_eq!(report.tasks.len(), 1);
  assert_eq!(report.tasks[0].conditions.len(), 2);
  let (markdown, json) = agent::write_report(&report, &fixture.batch.join("report/agent")).unwrap();
  assert!(fs::read_to_string(markdown).unwrap().contains("### t99-pipeline"));
  assert!(json.is_file());
}

#[test]
fn should_reuse_scored_runs_when_resumed() {
  let fixture = fixture();
  let tasks = agent::load_tasks(&fixture.tasks, &[]).unwrap();
  let config = fixture.config(1, Some(0.005));
  let first = agent::run_batch(&config, &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 3)).unwrap();
  assert_eq!(first.skipped_for_budget.len(), 1);
  let resumed = agent::run_batch(&RunnerConfig { budget_usd: None, ..config }, &tasks, agent::plan_runs(&tasks, &Condition::ALL, 1, 3)).unwrap();
  assert_eq!(resumed.skipped_for_budget, Vec::<String>::new());
  assert_eq!(resumed.spent_usd, 0.02);
}
