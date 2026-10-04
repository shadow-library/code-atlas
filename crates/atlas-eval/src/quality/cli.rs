use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;

use super::context::{ContextReport, evaluate_context, load_context_tasks};
use super::corpus::{Corpus, IndexedProject, discover};
use super::error::{QualityError, QualityResult};
use super::evaluate::evaluate_fixture;
use super::metrics::Comparison;
use super::oracle::{IndexerState, OracleOptions, OracleReport, run_oracle};
use super::refactor::{RefactorReport, run_refactors};
use super::report::{Environment, QualityReport, assemble, compare_reports, read_report, render_comparison, write_report};

const DEFAULT_SCIP_TIMEOUT_SECS: u64 = 1_800;

#[derive(Debug, Clone, Args)]
pub struct QualityArgs {
  #[arg(long, help = "Repository root containing fixtures/ (default: the nearest ancestor of the working directory that has them)")]
  pub repo_root: Option<PathBuf>,
  #[arg(long, help = "Output base path; .md and .json are appended (default: eval/reports/<date>/quality)")]
  pub out: Option<PathBuf>,
  #[arg(long, help = "A previous quality.json; prints improved and regressed metrics")]
  pub compare: Option<PathBuf>,
  #[arg(long, help = "Report date (default: today, UTC)")]
  pub date: Option<String>,
  #[arg(long = "only", help = "Restrict to fixture groups whose name contains this text (acme, globex, adversarial/<case>)")]
  pub only: Vec<String>,
  #[arg(long, help = "Skip the SCIP oracle")]
  pub no_scip: bool,
  #[arg(long, help = "Directory for SCIP trees, indexes and caches (default: a temporary directory)")]
  pub scip_work: Option<PathBuf>,
  #[arg(long, help = "Also run scip-typescript on the TypeScript fixtures (needs npx and network or a cached package)")]
  pub scip_typescript: bool,
  #[arg(long, help = "Also run the SCIP oracle on this repository's workspace")]
  pub scip_workspace: bool,
  #[arg(long, help = "Restrict the SCIP oracle to targets whose name contains this text")]
  pub scip_only: Vec<String>,
  #[arg(long, default_value_t = DEFAULT_SCIP_TIMEOUT_SECS)]
  pub scip_timeout_secs: u64,
  #[arg(long, help = "Skip the scripted refactor scenarios")]
  pub no_refactors: bool,
  #[arg(long, help = "Directory of task specs with ground_truth.relevant_files / relevant_symbols; scores the context query")]
  pub context_tasks: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityOutcome {
  pub markdown: PathBuf,
  pub json: PathBuf,
  pub comparison: Option<String>,
  pub regressions: usize,
}

fn locate_repo_root(explicit: Option<&Path>) -> QualityResult<PathBuf> {
  if let Some(root) = explicit {
    return Ok(root.to_owned());
  }
  let cwd = std::env::current_dir().map_err(|error| QualityError::io(".", error))?;
  cwd
    .ancestors()
    .find(|candidate| candidate.join("fixtures").join("expected.schema.md").is_file())
    .map(Path::to_owned)
    .ok_or_else(|| QualityError::InvalidArgument("no fixtures/ directory found above the working directory; pass --repo-root".to_owned()))
}

pub fn run_quality(args: &QualityArgs, repo_root: &Path, date: &str) -> QualityResult<QualityReport> {
  let fixtures = repo_root.join("fixtures");
  let groups = discover(&fixtures, &args.only)?;
  let corpus = Corpus::index(groups)?;
  let mut reports = Vec::new();
  for group in &corpus.groups {
    let Some(project) = corpus.project(&group.truth.project.name) else { continue };
    tracing::info!(fixture = %group.name, "evaluating fixture");
    reports.push(evaluate_fixture(&group.name, &group.truth, project, &corpus)?);
  }
  let mut notes = Vec::new();
  let oracle = oracle_report(args, repo_root, &corpus, &mut notes)?;
  let refactors = match args.no_refactors {
    true => {
      notes.push("refactor scenarios were skipped (--no-refactors)".to_owned());
      RefactorReport { scenarios: Vec::new() }
    }
    false => run_refactors(&fixtures.join("acme"))?,
  };
  let context = context_report(args, &fixtures, &corpus)?;
  let environment = Environment {
    date: date.to_owned(),
    rust_analyzer: oracle.rust_analyzer.clone(),
    scip_typescript: oracle.scip_typescript.clone(),
    notes,
  };
  Ok(assemble(environment, reports, oracle, refactors, context))
}

fn oracle_report(args: &QualityArgs, repo_root: &Path, corpus: &Corpus, notes: &mut Vec<String>) -> QualityResult<OracleReport> {
  if args.no_scip {
    notes.push("the SCIP oracle was skipped (--no-scip)".to_owned());
    let skipped = IndexerState::Disabled { reason: "--no-scip".to_owned() };
    return Ok(OracleReport::unavailable(skipped.clone(), skipped));
  }
  let (work_dir, _keep) = match &args.scip_work {
    Some(dir) => (dir.clone(), None),
    None => {
      let temporary = tempfile::tempdir().map_err(|error| QualityError::io(std::env::temp_dir(), error))?;
      (temporary.path().to_owned(), Some(temporary))
    }
  };
  let options = OracleOptions {
    work_dir,
    workspace_root: args.scip_workspace.then(|| repo_root.to_owned()),
    typescript: args.scip_typescript,
    timeout: Duration::from_secs(args.scip_timeout_secs),
    only: args.scip_only.clone(),
  };
  run_oracle(&corpus.groups, &options)
}

fn context_report(args: &QualityArgs, fixtures: &Path, corpus: &Corpus) -> QualityResult<ContextReport> {
  let Some(dir) = &args.context_tasks else {
    return Ok(ContextReport::NotRequested {
      reason: "no --context-tasks directory was given".to_owned(),
    });
  };
  let (tasks, skipped) = load_context_tasks(dir)?;
  let mut extra: BTreeMap<String, IndexedProject> = BTreeMap::new();
  for task in &tasks {
    if corpus.project(&task.fixture).is_some() || extra.contains_key(&task.fixture) {
      continue;
    }
    if let Some(group) = discover(fixtures, std::slice::from_ref(&task.fixture))?
      .into_iter()
      .find(|group| group.name == task.fixture)
    {
      extra.insert(task.fixture.clone(), IndexedProject::build(&group)?);
    }
  }
  let mut projects: BTreeMap<String, &IndexedProject> = corpus.projects.iter().map(|(name, project)| (name.clone(), project)).collect();
  projects.extend(extra.iter().map(|(name, project)| (name.clone(), project)));
  evaluate_context(&tasks, skipped, &projects)
}

pub fn execute(args: &QualityArgs) -> QualityResult<QualityOutcome> {
  let repo_root = locate_repo_root(args.repo_root.as_deref())?;
  let date = args.date.clone().unwrap_or_else(crate::agent::today_utc);
  let report = run_quality(args, &repo_root, &date)?;
  let comparison: Option<Comparison> = match &args.compare {
    Some(path) => Some(compare_reports(&read_report(path)?, &report)),
    None => None,
  };
  let out = args.out.clone().unwrap_or_else(|| PathBuf::from("eval").join("reports").join(&date).join("quality"));
  let (markdown, json) = write_report(&report, &out, comparison.as_ref())?;
  Ok(QualityOutcome {
    markdown,
    json,
    regressions: comparison.as_ref().map_or(0, |comparison| comparison.regressed.len()),
    comparison: comparison.as_ref().map(render_comparison),
  })
}
