use super::metrics::{Metrics, TokenMetrics};
use super::plan::{Condition, PreambleVariant};
use super::record::{CostSource, RunManifest, RunStatus, SessionOutcome};
use super::report::RunRecord;
use super::transcript::TokenUsage;
use super::workspace::TreeSource;

pub(super) const TREE_PARENT: &str = "/var/folders/x";

pub(super) fn scratch_of(run_id: &str) -> String {
  format!("{TREE_PARENT}/ae-{run_id}")
}

pub(super) fn run_record(task: &str, condition: Condition, index: u32, total: TokenUsage) -> RunRecord {
  let run_id = format!("{task}.{condition}.r{index}");
  let scratch = scratch_of(&run_id);
  let atlas = condition == Condition::Atlas;
  let manifest = RunManifest {
    run_id: run_id.clone(),
    task_id: task.to_owned(),
    condition,
    run_index: index,
    order: 0,
    spec_hash: format!("spec-{task}"),
    source: TreeSource::GitArchive {
      repo: "/repo".into(),
      commit: "c0ffee".to_owned(),
      subdir: None,
    },
    base_sha: Some(format!("base-{task}")),
    tree: Some(format!("{scratch}/tree")),
    project_instructions: vec!["CLAUDE.md".to_owned()],
    model_requested: "claude-sonnet-5-5".to_owned(),
    models_reported: vec!["claude-sonnet-5-5".to_owned()],
    claude_code_version: Some("2.1.289".to_owned()),
    preamble_version: atlas.then_some(1),
    preamble_variant: atlas.then_some(PreambleVariant::Standard),
    atlas_config_blake3: None,
    args: [
      "--mcp-config",
      &format!("{scratch}/mcp.json"),
      "--add-dir",
      &format!("/private{scratch}/tree"),
      "--append-system-prompt",
      "Read CLAUDE.md first.",
      "-p",
      "Fix it.",
    ]
    .map(str::to_owned)
    .to_vec(),
    env_names: vec!["HOME".to_owned()],
    atlas_project: None,
    atlas_index_ms: None,
    wall_ms: Some(60_000),
    session_cost_usd: Some(0.5),
    cost_source: CostSource::Reported,
    cost_usd: 0.5,
    status: RunStatus::Completed {
      session: SessionOutcome::Finished { subtype: "success".to_owned() },
    },
  };
  let metrics = Metrics {
    tokens: TokenMetrics { total, ..TokenMetrics::default() },
    ..Metrics::default()
  };
  RunRecord {
    manifest,
    metrics: Some(metrics),
    score: None,
    transcript: format!("{run_id}/transcript.jsonl"),
  }
}
