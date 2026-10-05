use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_engine::Engine;
use atlas_ide::{CrossRepo, DiscoveryLauncher, EditOperation, EditRequest, EditStatus, IdeError, IdeMode, IdeOptions, IdePlan, IdeService, IdeSettings, Launcher, ServerView};
use atlas_project::Project;
use serde::{Deserialize, Serialize};

use super::SetScore;
use crate::quality::corpus::{FixtureGroup, IndexedProject};
use crate::quality::error::QualityResult;
use crate::quality::metrics::{Prohibition, ratio};
use crate::quality::truth::{RenameSetup, RenameSite, RenameTruth};

const READY_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedEdit {
  pub repo: String,
  pub path: String,
  pub line: u32,
  pub column: u32,
  pub on_name: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedReport {
  pub repo: String,
  pub path: String,
  pub line: Option<u32>,
  pub column: Option<u32>,
  pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameObserved {
  pub edits: Vec<ObservedEdit>,
  pub reports: Vec<ObservedReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportScore {
  pub expected: u64,
  pub found: u64,
  pub wrong_reason: u64,
  pub missing: u64,
}

impl ReportScore {
  pub fn recall(&self) -> Option<f64> {
    ratio(self.found, self.expected)
  }

  pub fn absorb(&mut self, other: &ReportScore) {
    self.expected += other.expected;
    self.found += other.found;
    self.wrong_reason += other.wrong_reason;
    self.missing += other.missing;
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameScore {
  pub edit: SetScore,
  pub report: ReportScore,
  pub untouched: Prohibition,
  pub missed_edits: Vec<String>,
  pub unexpected_edits: Vec<String>,
  pub report_problems: Vec<String>,
  pub violations: Vec<String>,
  pub extra_reports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RenameOutcome {
  Scored { score: Box<RenameScore>, caveats: Vec<String> },
  Skipped { reason: String },
  Failed { reason: String, score: Box<RenameScore> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameScenario {
  pub id: String,
  pub fixture: String,
  pub target: String,
  pub new_name: String,
  pub cross_repo: String,
  pub outcome: RenameOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameServer {
  pub server: String,
  pub version: String,
  pub typescript: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameAggregate {
  pub scored: u64,
  pub failed: u64,
  pub skipped: u64,
  pub edit: SetScore,
  pub report: ReportScore,
  pub untouched: Prohibition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RenameReport {
  NotRequested {
    reason: String,
  },
  Run {
    servers: Vec<RenameServer>,
    scenarios: Vec<RenameScenario>,
    aggregate: RenameAggregate,
  },
}

impl Default for RenameReport {
  fn default() -> Self {
    Self::NotRequested {
      reason: "no rename scenarios were run".to_owned(),
    }
  }
}

fn place(repo: &str, path: &str, line: u32) -> String {
  format!("{repo}:{path}:{line}")
}

fn site_place(site: &RenameSite) -> String {
  place(&site.repo, &site.path, site.line)
}

fn same_place(site: &RenameSite, repo: &str, path: &str, line: u32, column: Option<u32>) -> bool {
  site.repo == repo && site.path == path && site.line == line && site.column.is_none_or(|expected| Some(expected) == column)
}

pub fn score_rename(truth: &RenameTruth, observed: &RenameObserved) -> RenameScore {
  let mut score = RenameScore::default();
  let mut taken = vec![false; observed.edits.len()];
  let mut expected: Vec<&RenameSite> = truth.edit.iter().collect();
  expected.sort_by_key(|site| site.column.is_none());
  let mut matched = 0;
  for site in expected {
    let found = observed
      .edits
      .iter()
      .enumerate()
      .find(|(index, edit)| !taken[*index] && edit.on_name && same_place(site, &edit.repo, &edit.path, edit.line, Some(edit.column)));
    match found {
      Some((index, _)) => {
        taken[index] = true;
        matched += 1;
      }
      None => score.missed_edits.push(site_place(site)),
    }
  }
  score.missed_edits.sort();
  score.edit = SetScore {
    expected: truth.edit.len() as u64,
    predicted: observed.edits.len() as u64,
    correct: matched,
  };
  score.unexpected_edits = observed
    .edits
    .iter()
    .zip(&taken)
    .filter(|(_, taken)| !**taken)
    .map(|(edit, _)| format!("{}:{}", place(&edit.repo, &edit.path, edit.line), edit.column))
    .collect();
  score.report.expected = truth.report.len() as u64;
  let mut claimed = BTreeSet::new();
  for site in &truth.report {
    let at_place: Vec<(usize, &ObservedReport)> = observed
      .reports
      .iter()
      .enumerate()
      .filter(|(_, report)| report.line.is_some_and(|line| same_place(site, &report.repo, &report.path, line, report.column)))
      .collect();
    let exact = at_place
      .iter()
      .find(|(index, report)| !claimed.contains(index) && Some(report.reason.as_str()) == site.reason.as_deref());
    match (exact, at_place.first()) {
      (Some((index, _)), _) => {
        claimed.insert(*index);
        score.report.found += 1;
      }
      (None, Some((_, other))) => {
        score.report.wrong_reason += 1;
        score.report_problems.push(format!(
          "{} reported as `{}`, expected `{}`",
          site_place(site),
          other.reason,
          site.reason.as_deref().unwrap_or_default()
        ));
      }
      (None, None) => {
        score.report.missing += 1;
        score.report_problems.push(format!("{} not reported", site_place(site)));
      }
    }
  }
  score.extra_reports = observed
    .reports
    .iter()
    .enumerate()
    .filter(|(index, report)| {
      !claimed.contains(index)
        && !truth
          .report
          .iter()
          .any(|site| report.line.is_some_and(|line| same_place(site, &report.repo, &report.path, line, report.column)))
    })
    .map(|(_, report)| format!("{} ({})", place(&report.repo, &report.path, report.line.unwrap_or(0)), report.reason))
    .collect();
  score.untouched.checked = truth.untouched.len() as u64;
  for site in &truth.untouched {
    if observed.edits.iter().any(|edit| same_place(site, &edit.repo, &edit.path, edit.line, Some(edit.column))) {
      score.untouched.violations += 1;
      score.violations.push(site_place(site));
    }
  }
  score
}

fn unscored(truth: &RenameTruth) -> RenameScore {
  score_rename(truth, &RenameObserved::default())
}

pub fn aggregate_scenarios(scenarios: &[RenameScenario]) -> RenameAggregate {
  let mut aggregate = RenameAggregate::default();
  for scenario in scenarios {
    let score = match &scenario.outcome {
      RenameOutcome::Scored { score, .. } => {
        aggregate.scored += 1;
        score
      }
      RenameOutcome::Failed { score, .. } => {
        aggregate.failed += 1;
        score
      }
      RenameOutcome::Skipped { .. } => {
        aggregate.skipped += 1;
        continue;
      }
    };
    aggregate.edit.absorb(&score.edit);
    aggregate.report.absorb(&score.report);
    aggregate.untouched.absorb(&score.untouched);
  }
  aggregate
}

#[derive(Debug, Clone, Default)]
pub struct RenameOptions {
  pub typescript_server: Option<PathBuf>,
}

impl RenameOptions {
  pub fn from_env() -> Self {
    Self {
      typescript_server: std::env::var_os("ATLAS_TEST_TS_LSP").map(PathBuf::from),
    }
  }
}

fn locate_on_path(program: &str) -> Option<PathBuf> {
  std::env::split_paths(&std::env::var_os("PATH")?)
    .map(|directory| directory.join(program))
    .find(|candidate| candidate.is_file())
}

fn typescript_command(server: &Path) -> Option<Vec<String>> {
  let entry = server.to_string_lossy().into_owned();
  match server.extension().and_then(|extension| extension.to_str()) {
    Some("mjs" | "js" | "cjs") => Some(vec![locate_on_path("node")?.to_string_lossy().into_owned(), entry]),
    _ => Some(vec![entry]),
  }
}

pub fn rename_settings(options: &RenameOptions) -> IdeSettings {
  IdeSettings {
    download: false,
    ready_timeout_secs: READY_TIMEOUT_SECS,
    typescript_command: options.typescript_server.as_deref().and_then(typescript_command),
    ..IdeSettings::default()
  }
}

pub fn run_rename(groups: &[FixtureGroup], options: &RenameOptions) -> QualityResult<RenameReport> {
  run_rename_with(groups, Arc::new(DiscoveryLauncher), rename_settings(options))
}

pub fn run_rename_with(groups: &[FixtureGroup], launcher: Arc<dyn Launcher>, settings: IdeSettings) -> QualityResult<RenameReport> {
  let mut scenarios = Vec::new();
  let mut servers: Vec<RenameServer> = Vec::new();
  for group in groups {
    for truth in &group.truth.rename_sites {
      tracing::info!(scenario = %truth.id, fixture = %group.name, "running rename scenario");
      let run = run_scenario(group, truth, Arc::clone(&launcher), settings.clone())?;
      for server in run.servers {
        if !servers.contains(&server) {
          servers.push(server);
        }
      }
      scenarios.push(run.scenario);
    }
  }
  if scenarios.is_empty() {
    return Ok(RenameReport::NotRequested {
      reason: "no fixture declares [[rename_sites]]".to_owned(),
    });
  }
  let aggregate = aggregate_scenarios(&scenarios);
  Ok(RenameReport::Run { servers, scenarios, aggregate })
}

struct ScenarioRun {
  scenario: RenameScenario,
  servers: Vec<RenameServer>,
}

fn run_scenario(group: &FixtureGroup, truth: &RenameTruth, launcher: Arc<dyn Launcher>, settings: IdeSettings) -> QualityResult<ScenarioRun> {
  let cross_repo = truth.cross_repo.clone().unwrap_or_else(|| "report".to_owned());
  let spec = truth.target.spec().unwrap_or_default();
  let scenario = |outcome: RenameOutcome| RenameScenario {
    id: truth.id.clone(),
    fixture: group.name.clone(),
    target: spec.clone(),
    new_name: truth.new_name.clone(),
    cross_repo: cross_repo.clone(),
    outcome,
  };
  let failed = |reason: String, servers: Vec<RenameServer>| ScenarioRun {
    scenario: scenario(RenameOutcome::Failed {
      reason,
      score: Box::new(unscored(truth)),
    }),
    servers,
  };
  let project = IndexedProject::build(group)?;
  if let Err(reason) = apply_setup(&project, &truth.setup) {
    return Ok(failed(reason, Vec::new()));
  }
  project.engine.ensure_fresh()?;
  let request = DryRunRequest {
    target: spec.clone(),
    new_name: truth.new_name.clone(),
    cross_repo: if cross_repo == "apply" { CrossRepo::Apply } else { CrossRepo::Report },
  };
  let run = match dry_run_rename(&project.engine, launcher, settings, &request) {
    Ok(run) => run,
    Err(DryRunError::NoServer(reason)) => {
      return Ok(ScenarioRun {
        scenario: scenario(RenameOutcome::Skipped {
          reason: format!("no language server: {reason}"),
        }),
        servers: Vec::new(),
      });
    }
    Err(DryRunError::Failed { reason, servers }) => return Ok(failed(reason, servers)),
  };
  let observed = match observe(&project.project, &run.plan, old_name_of(truth).as_deref()) {
    Ok(observed) => observed,
    Err(reason) => return Ok(failed(reason, run.servers)),
  };
  Ok(ScenarioRun {
    scenario: scenario(RenameOutcome::Scored {
      score: Box::new(score_rename(truth, &observed)),
      caveats: run.plan.caveats.clone(),
    }),
    servers: run.servers,
  })
}

#[derive(Debug, Clone)]
pub struct DryRunRequest {
  pub target: String,
  pub new_name: String,
  pub cross_repo: CrossRepo,
}

pub struct DryRun {
  pub plan: IdePlan,
  pub servers: Vec<RenameServer>,
}

#[derive(Debug)]
pub enum DryRunError {
  NoServer(String),
  Failed { reason: String, servers: Vec<RenameServer> },
}

pub fn dry_run_rename(engine: &Arc<Engine>, launcher: Arc<dyn Launcher>, settings: IdeSettings, request: &DryRunRequest) -> Result<DryRun, DryRunError> {
  let failed = |reason: String, servers: Vec<RenameServer>| DryRunError::Failed { reason, servers };
  let cache = tempfile::tempdir().map_err(|error| failed(format!("cannot create the cache directory: {error}"), Vec::new()))?;
  let ide = IdeService::new(Arc::clone(engine), launcher, IdeOptions::new(IdeMode::OneShot, settings, cache.path()));
  let operation = EditOperation::Rename {
    target: request.target.clone(),
    new_name: request.new_name.clone(),
    cross_repo: request.cross_repo,
    allow_unverified: false,
  };
  let response = match ide.edit(&EditRequest::dry_run(operation)) {
    Ok(response) => response,
    Err(error @ (IdeError::Unavailable { .. } | IdeError::Disabled { .. })) => return Err(DryRunError::NoServer(error.to_string())),
    Err(error) => return Err(failed(error.to_string(), Vec::new())),
  };
  let servers: Vec<RenameServer> = response.answer.servers.iter().map(server_of).collect();
  let plan_id = match &response.answer.status {
    EditStatus::DryRun { plan_id, .. } => plan_id.clone(),
    EditStatus::Refused { reason } => return Err(failed(format!("refused: {reason}"), servers)),
    other => return Err(failed(format!("unexpected status {other:?}"), servers)),
  };
  match ide.planned(&plan_id) {
    Some(plan) => Ok(DryRun { plan, servers }),
    None => Err(failed("the dry-run plan is no longer cached".to_owned(), servers)),
  }
}

fn server_of(view: &ServerView) -> RenameServer {
  RenameServer {
    server: view.server.clone(),
    version: view.version.clone(),
    typescript: view.typescript.clone(),
  }
}

fn repo_dir(project: &Project, repo: &str) -> Result<PathBuf, String> {
  project
    .repos
    .iter()
    .find(|candidate| candidate.name.as_str() == repo)
    .map(|candidate| candidate.path.clone())
    .ok_or_else(|| format!("repo `{repo}` is not part of the fixture"))
}

fn apply_setup(project: &IndexedProject, setup: &[RenameSetup]) -> Result<(), String> {
  for step in setup {
    let file = repo_dir(&project.project, &step.repo)?.join(&step.path);
    let text = fs::read_to_string(&file).map_err(|error| format!("setup cannot read {}: {error}", file.display()))?;
    if !text.contains(&step.replace) {
      return Err(format!("setup text {:?} does not occur in {}:{}", step.replace, step.repo, step.path));
    }
    fs::write(&file, text.replace(&step.replace, &step.with)).map_err(|error| format!("setup cannot write {}: {error}", file.display()))?;
  }
  Ok(())
}

fn old_name_of(truth: &RenameTruth) -> Option<String> {
  let symbol = truth.target.symbol.as_deref()?;
  symbol.rsplit(['.', ':']).next().map(str::to_owned)
}

pub fn observe(project: &Project, plan: &IdePlan, old_name: Option<&str>) -> Result<RenameObserved, String> {
  let mut observed = RenameObserved::default();
  for file in &plan.edits.files {
    let path = repo_dir(project, file.file.repo.as_str())?.join(file.file.path.as_str());
    let text = fs::read_to_string(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    for edit in &file.edits {
      let before = text
        .get(..edit.start)
        .ok_or_else(|| format!("edit offset {} is not a character boundary in {}", edit.start, file.file))?;
      let line = before.matches('\n').count() as u32 + 1;
      let column = before.rsplit('\n').next().map_or(0, |tail| tail.chars().count()) as u32 + 1;
      let replaced = text.get(edit.start..edit.end);
      let on_name = old_name.is_none_or(|name| replaced.is_some_and(|text| text == name || text.strip_prefix("r#") == Some(name)));
      observed.edits.push(ObservedEdit {
        repo: file.file.repo.to_string(),
        path: file.file.path.to_string(),
        line,
        column,
        on_name,
      });
    }
  }
  for site in &plan.not_renamed {
    let reason = serde_json::to_value(site.reason)
      .ok()
      .and_then(|value| value.as_str().map(str::to_owned))
      .unwrap_or_else(|| site.reason.as_str().to_owned());
    observed.reports.push(ObservedReport {
      repo: site.repo.clone(),
      path: site.path.clone(),
      line: site.line,
      column: site.column,
      reason,
    });
  }
  Ok(observed)
}

#[cfg(test)]
mod tests {
  use atlas_ide::{IdeResult, LaunchRequest, LaunchSpec};

  use super::*;
  use crate::quality::corpus::discover;
  use crate::quality::truth::parse_truth;

  const TRUTH: &str = r#"
[project]
name = "demo"
repos = ["a"]

[[rename_sites]]
id = "demo"
target = { repo = "a", path = "src/x.ts", symbol = "X.run" }
new_name = "go"
edit = [
  { repo = "a", path = "src/x.ts", line = 3 },
  { repo = "a", path = "src/x.ts", line = 7, column = 12 },
  { repo = "a", path = "src/y.ts", line = 7 },
  { repo = "a", path = "src/y.ts", line = 7 },
]
report = [
  { repo = "a", path = "src/z.spec.ts", line = 5, column = 27, reason = "mock" },
  { repo = "a", path = "src/z.spec.ts", line = 6, reason = "mock" },
  { repo = "a", path = "src/z.spec.ts", line = 8, reason = "mock" },
]
untouched = [
  { repo = "a", path = "src/x.ts", line = 4 },
  { repo = "a", path = "src/y.ts", line = 9 },
]
"#;

  fn truth() -> RenameTruth {
    parse_truth(TRUTH, "demo.toml").unwrap().rename_sites.remove(0)
  }

  fn edit(path: &str, line: u32, column: u32) -> ObservedEdit {
    ObservedEdit {
      repo: "a".to_owned(),
      path: path.to_owned(),
      line,
      column,
      on_name: true,
    }
  }

  fn report(path: &str, line: u32, column: u32, reason: &str) -> ObservedReport {
    ObservedReport {
      repo: "a".to_owned(),
      path: path.to_owned(),
      line: Some(line),
      column: Some(column),
      reason: reason.to_owned(),
    }
  }

  #[test]
  fn should_parse_rename_truth_with_optional_columns_and_reasons() {
    let truth = truth();
    assert_eq!(truth.target.spec().as_deref(), Some("X.run"));
    assert_eq!(truth.edit.len(), 4);
    assert_eq!(truth.edit[1].column, Some(12));
    assert_eq!(truth.report[0].reason.as_deref(), Some("mock"));
    assert_eq!(truth.cross_repo, None);
  }

  #[test]
  fn should_reject_rename_truth_that_contradicts_itself() {
    let duplicate = format!(
      "{TRUTH}\n[[rename_sites]]\nid = \"demo\"\ntarget = {{ repo = \"a\", path = \"p.ts\", symbol = \"S\" }}\nnew_name = \"t\"\nedit = [{{ repo = \"a\", path = \"p.ts\", line = 1 }}]\n"
    );
    assert!(parse_truth(&duplicate, "t.toml").unwrap_err().to_string().contains("duplicate rename_sites id `demo`"));
    let both = TRUTH.replace("line = 4 }", "line = 3 }");
    assert!(parse_truth(&both, "t.toml").unwrap_err().to_string().contains("both a site and untouched"));
    let reasonless = TRUTH.replace(
      ", reason = \"mock\" },\n  { repo = \"a\", path = \"src/z.spec.ts\", line = 6",
      " },\n  { repo = \"a\", path = \"src/z.spec.ts\", line = 6",
    );
    assert!(parse_truth(&reasonless, "t.toml").unwrap_err().to_string().contains("needs a reason"));
    let unknown = TRUTH.replace("new_name = \"go\"", "new_name = \"go\"\ncross_repo = \"always\"");
    assert!(parse_truth(&unknown, "t.toml").unwrap_err().to_string().contains("unknown cross_repo `always`"));
    let targetless = TRUTH.replace(", symbol = \"X.run\"", "");
    assert!(parse_truth(&targetless, "t.toml").unwrap_err().to_string().contains("needs a symbol, key or line"));
  }

  #[test]
  fn should_build_position_and_key_targets() {
    let mut target = truth().target;
    target.line = Some(4);
    assert_eq!(target.spec().as_deref(), Some("a:src/x.ts:4:1"));
    target.column = Some(9);
    assert_eq!(target.spec().as_deref(), Some("a:src/x.ts:4:9"));
    target.key = Some("ts:demo:a:src/x.ts#class".to_owned());
    assert_eq!(target.spec().as_deref(), Some("ts:demo:a:src/x.ts#class"));
  }

  #[test]
  fn should_score_exact_recall_precision_report_recall_and_violations() {
    let observed = RenameObserved {
      edits: vec![
        edit("src/x.ts", 3, 5),
        edit("src/x.ts", 4, 1),
        edit("src/x.ts", 7, 12),
        edit("src/y.ts", 7, 2),
        edit("src/q.ts", 1, 1),
      ],
      reports: vec![
        report("src/z.spec.ts", 5, 27, "mock"),
        report("src/z.spec.ts", 6, 9, "heuristic"),
        report("src/z.spec.ts", 12, 1, "mock"),
      ],
    };

    let score = score_rename(&truth(), &observed);

    assert_eq!(
      score.edit,
      SetScore {
        expected: 4,
        predicted: 5,
        correct: 3
      }
    );
    assert_eq!((score.edit.recall(), score.edit.precision()), (Some(0.75), Some(0.6)));
    assert_eq!(score.missed_edits, vec!["a:src/y.ts:7"]);
    assert_eq!(score.unexpected_edits, vec!["a:src/x.ts:4:1", "a:src/q.ts:1:1"]);
    assert_eq!(
      score.report,
      ReportScore {
        expected: 3,
        found: 1,
        wrong_reason: 1,
        missing: 1
      }
    );
    assert_eq!(score.report.recall(), Some(1.0 / 3.0));
    assert_eq!(
      score.report_problems,
      vec!["a:src/z.spec.ts:6 reported as `heuristic`, expected `mock`", "a:src/z.spec.ts:8 not reported"]
    );
    assert_eq!(
      score.untouched,
      Prohibition {
        checked: 2,
        violations: 1,
        unbound: 0,
        vacuous: 0
      }
    );
    assert_eq!(score.violations, vec!["a:src/x.ts:4"]);
    assert_eq!(score.extra_reports, vec!["a:src/z.spec.ts:12 (mock)"]);
  }

  #[test]
  fn should_match_a_column_only_when_the_truth_gives_one() {
    let observed = RenameObserved {
      edits: vec![edit("src/x.ts", 3, 99), edit("src/x.ts", 7, 13), edit("src/y.ts", 7, 1), edit("src/y.ts", 7, 20)],
      reports: Vec::new(),
    };
    let score = score_rename(&truth(), &observed);
    assert_eq!(score.edit.correct, 3);
    assert_eq!(score.missed_edits, vec!["a:src/x.ts:7"]);
  }

  #[test]
  fn should_not_count_an_edit_on_the_right_line_but_not_on_the_old_name() {
    let mut off_name = edit("src/x.ts", 3, 1);
    off_name.on_name = false;
    let observed = RenameObserved {
      edits: vec![off_name, edit("src/x.ts", 7, 12), edit("src/y.ts", 7, 1), edit("src/y.ts", 7, 5)],
      reports: Vec::new(),
    };
    let score = score_rename(&truth(), &observed);
    assert_eq!(
      score.edit,
      SetScore {
        expected: 4,
        predicted: 4,
        correct: 3
      }
    );
    assert_eq!(score.missed_edits, vec!["a:src/x.ts:3"]);
    assert_eq!(score.unexpected_edits, vec!["a:src/x.ts:3:1"]);
  }

  #[test]
  fn should_need_one_distinct_edit_per_expected_site_on_the_same_line() {
    let observed = RenameObserved {
      edits: vec![edit("src/x.ts", 3, 1), edit("src/x.ts", 7, 12), edit("src/y.ts", 7, 1)],
      reports: Vec::new(),
    };
    let score = score_rename(&truth(), &observed);
    assert_eq!(
      score.edit,
      SetScore {
        expected: 4,
        predicted: 3,
        correct: 3
      }
    );
    assert_eq!(score.missed_edits, vec!["a:src/y.ts:7"]);
  }

  #[test]
  fn should_aggregate_scored_and_failed_scenarios_and_leave_skipped_ones_out() {
    let scenario = |outcome| RenameScenario {
      id: "s".to_owned(),
      fixture: "f".to_owned(),
      target: "T".to_owned(),
      new_name: "n".to_owned(),
      cross_repo: "report".to_owned(),
      outcome,
    };
    let perfect = RenameObserved {
      edits: vec![edit("src/x.ts", 3, 1), edit("src/x.ts", 7, 12), edit("src/y.ts", 7, 1), edit("src/y.ts", 7, 5)],
      reports: vec![
        report("src/z.spec.ts", 5, 27, "mock"),
        report("src/z.spec.ts", 6, 1, "mock"),
        report("src/z.spec.ts", 8, 1, "mock"),
      ],
    };
    let scenarios = vec![
      scenario(RenameOutcome::Scored {
        score: Box::new(score_rename(&truth(), &perfect)),
        caveats: Vec::new(),
      }),
      scenario(RenameOutcome::Failed {
        reason: "refused".to_owned(),
        score: Box::new(unscored(&truth())),
      }),
      scenario(RenameOutcome::Skipped { reason: "none".to_owned() }),
    ];

    let aggregate = aggregate_scenarios(&scenarios);

    assert_eq!((aggregate.scored, aggregate.failed, aggregate.skipped), (1, 1, 1));
    assert_eq!(
      aggregate.edit,
      SetScore {
        expected: 8,
        predicted: 4,
        correct: 4
      }
    );
    assert_eq!((aggregate.edit.recall(), aggregate.edit.precision()), (Some(0.5), Some(1.0)));
    assert_eq!(
      aggregate.report,
      ReportScore {
        expected: 6,
        found: 3,
        wrong_reason: 0,
        missing: 3
      }
    );
    assert_eq!(aggregate.untouched.checked, 4);
    assert_eq!(aggregate.untouched.violations, 0);
  }

  struct Unavailable;

  impl Launcher for Unavailable {
    fn launch(&self, request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
      Err(IdeError::Unavailable {
        server: request.kind.name().to_owned(),
        reason: "not installed".to_owned(),
        alternative: None,
      })
    }
  }

  struct Broken;

  impl Launcher for Broken {
    fn launch(&self, _request: &LaunchRequest<'_>) -> IdeResult<LaunchSpec> {
      Err(IdeError::InvalidRequest("launcher exploded".to_owned()))
    }
  }

  fn trait_dispatch() -> Vec<FixtureGroup> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let groups = discover(&fixtures, &["trait-dispatch-rust".to_owned()]).unwrap();
    assert_eq!(groups.len(), 1);
    groups
  }

  #[test]
  fn should_skip_a_scenario_without_a_language_server_instead_of_scoring_zero() {
    let report = run_rename_with(&trait_dispatch(), Arc::new(Unavailable), IdeSettings::default()).unwrap();

    let RenameReport::Run { servers, scenarios, aggregate } = report else {
      panic!("expected a run");
    };
    assert_eq!(servers, Vec::new());
    assert_eq!(scenarios.len(), 1);
    assert_eq!(scenarios[0].id, "store-get");
    assert!(matches!(&scenarios[0].outcome, RenameOutcome::Skipped { reason } if reason.starts_with("no language server: rust-analyzer is unavailable: not installed")));
    assert_eq!((aggregate.scored, aggregate.failed, aggregate.skipped), (0, 0, 1));
    assert_eq!(aggregate.edit, SetScore::default());
    assert_eq!(aggregate.edit.recall(), None);
  }

  #[test]
  fn should_count_a_failed_scenario_as_missed_sites() {
    let report = run_rename_with(&trait_dispatch(), Arc::new(Broken), IdeSettings::default()).unwrap();

    let RenameReport::Run { scenarios, aggregate, .. } = report else {
      panic!("expected a run");
    };
    let RenameOutcome::Failed { reason, score } = &scenarios[0].outcome else {
      panic!("expected a failure, got {:?}", scenarios[0].outcome);
    };
    assert_eq!(reason, "launcher exploded");
    assert_eq!(score.missed_edits.len(), 7);
    assert_eq!((aggregate.failed, aggregate.edit.expected, aggregate.edit.correct), (1, 7, 0));
    assert_eq!(aggregate.edit.recall(), Some(0.0));
  }

  #[test]
  fn should_report_no_scenarios_when_no_fixture_declares_rename_truth() {
    let report = run_rename_with(&[], Arc::new(Broken), IdeSettings::default()).unwrap();
    assert_eq!(
      report,
      RenameReport::NotRequested {
        reason: "no fixture declares [[rename_sites]]".to_owned()
      }
    );
  }

  #[test]
  fn should_launch_typescript_through_node_for_a_js_entry_and_directly_otherwise() {
    assert_eq!(typescript_command(Path::new("/opt/tls")), Some(vec!["/opt/tls".to_owned()]));
    let script = typescript_command(Path::new("/opt/lib/cli.mjs"));
    assert!(script.is_none_or(|command| command.len() == 2 && command[1] == "/opt/lib/cli.mjs" && command[0].ends_with("node")));
  }
}
