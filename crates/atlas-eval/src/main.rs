use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use atlas_eval::agent::{
  self, AgentError, AgentReport, ClaudeConfig, Condition, ConfigIsolation, DEFAULT_JUDGE_MODEL, DEFAULT_SESSION_MODEL, DryRunContext, EvalHome, JudgeMode, LoadedTask,
  PreambleVariant, RepoSource, RunnerConfig,
};
use atlas_eval::quality::{self, QualityArgs};
use atlas_eval::retrieval::{self, RetrievalArgs};

#[derive(Debug, Parser)]
#[command(name = "atlas-eval", about = "Code Atlas evaluation harness")]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  #[command(subcommand, about = "Controlled Claude Code A/B comparison: baseline vs Code Atlas MCP")]
  Agent(AgentCommand),
  #[command(about = "Knowledge-quality evaluation: ground truth, SCIP oracle, refactor robustness and context selection")]
  Quality(Box<QualityArgs>),
  #[command(about = "Deterministic retrieval evaluation: does Code Atlas surface each task's ground truth from the prompt alone?")]
  Retrieval(Box<RetrievalArgs>),
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
  #[command(about = "Run the planned sessions, score them, and write run artifacts under <eval-home>/runs/<batch>")]
  Run(Box<RunArgs>),
  #[command(about = "Aggregate a batch of runs into agent.md and agent.json")]
  Report(ReportArgs),
  #[command(about = "Run one tiny session with the baseline flags and record which isolation they achieve")]
  Probe(ProbeArgs),
  #[command(about = "Measure what the MCP tool surface costs per request: bytes of tools/list and instructions, and tokens in tiny sessions")]
  Surface(Box<SurfaceArgs>),
  #[command(about = "From recorded sessions: the turn at which each session first sees, reads and edits the files its task must change")]
  Headroom(HeadroomArgs),
  #[command(about = "From recorded runs: how much every report metric differs between runs of the same condition, next to the A/B effects")]
  Noise(NoiseArgs),
  #[command(about = "Per task, without a session: which prompt terms return a file the task must change, and the leak class that follows")]
  LeakAudit(LeakAuditArgs),
}

#[derive(Debug, Args)]
struct ClaudeArgs {
  #[arg(long, default_value = "claude")]
  claude_bin: PathBuf,
  #[arg(long, default_value = DEFAULT_SESSION_MODEL)]
  model: String,
  #[arg(long, default_value = DEFAULT_JUDGE_MODEL)]
  judge_model: String,
  #[arg(long, value_enum, default_value_t = ConfigIsolation::User)]
  claude_config: ConfigIsolation,
}

impl ClaudeArgs {
  fn config(&self) -> ClaudeConfig {
    ClaudeConfig {
      binary: self.claude_bin.clone(),
      model: self.model.clone(),
      judge_model: self.judge_model.clone(),
      config_isolation: self.claude_config,
    }
  }
}

#[derive(Debug, Args)]
struct RunArgs {
  #[arg(long)]
  repo_root: Option<PathBuf>,
  #[arg(long)]
  tasks: Option<PathBuf>,
  #[arg(long = "task")]
  only: Vec<String>,
  #[arg(long)]
  eval_home: Option<PathBuf>,
  #[arg(long)]
  batch: Option<String>,
  #[arg(long, value_enum, num_args = 1.., default_values_t = Condition::ALL)]
  condition: Vec<Condition>,
  #[arg(long, default_value_t = 2)]
  runs_per_condition: u32,
  #[arg(long, default_value_t = 1)]
  seed: u64,
  #[arg(long, default_value_t = 2)]
  parallel: usize,
  #[arg(long)]
  budget_usd: Option<f64>,
  #[arg(long)]
  dry_run: bool,
  #[arg(
    long,
    conflicts_with = "dry_run",
    help = "Run the base check of every task (setup, hidden tests, regression on the untouched base tree) and start no session"
  )]
  base_only: bool,
  #[arg(long)]
  atlas_bin: Option<PathBuf>,
  #[arg(long)]
  repo_override: Option<PathBuf>,
  #[arg(
    long,
    value_name = "DIR",
    help = "Seed every isolated Code Atlas home of the atlas arm with DIR/config.toml and DIR/detectors/*.toml (refused when DIR is missing or the configuration does not load)"
  )]
  atlas_config: Option<PathBuf>,
  #[arg(long, value_enum, default_value_t = JudgeMode::Claude)]
  judge: JudgeMode,
  #[arg(long, value_enum, default_value_t = PreambleVariant::Standard)]
  atlas_preamble: PreambleVariant,
  #[arg(long)]
  no_probe: bool,
  #[arg(long, default_value_t = 900)]
  validation_timeout_secs: u64,
  #[arg(long, default_value_t = 900)]
  atlas_timeout_secs: u64,
  #[arg(long)]
  tree_root: Option<PathBuf>,
  #[arg(long)]
  keep_trees: bool,
  #[command(flatten)]
  claude: ClaudeArgs,
}

#[derive(Debug, Args)]
struct ReportArgs {
  #[arg(long)]
  runs: PathBuf,
  #[arg(long)]
  out: Option<PathBuf>,
  #[arg(long)]
  compare: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct HeadroomArgs {
  #[arg(long, required = true, num_args = 1..)]
  runs: Vec<PathBuf>,
  #[arg(long)]
  out: Option<PathBuf>,
  #[arg(long)]
  repo_root: Option<PathBuf>,
  #[arg(long)]
  tasks: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct NoiseArgs {
  #[arg(long, required = true, num_args = 1..)]
  runs: Vec<PathBuf>,
  #[arg(long, num_args = 1..)]
  ab_report: Vec<PathBuf>,
  #[arg(long)]
  out: Option<PathBuf>,
  #[arg(long)]
  repo_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct LeakAuditArgs {
  #[arg(
    long,
    help = "Repository root holding eval/tasks and the task base commits (default: the git top level of the working directory)"
  )]
  repo_root: Option<PathBuf>,
  #[arg(long, help = "Task directory (default: <repo-root>/eval/tasks)")]
  tasks: Option<PathBuf>,
  #[arg(long = "task", help = "Only audit this task id (repeatable)")]
  only: Vec<String>,
  #[arg(long, help = "Output base path; .md and .json are appended (default: reports/<date>/leak-audit beside the task directory)")]
  out: Option<PathBuf>,
  #[arg(
    long,
    help = "Clone cache for external task repositories (default: runs/.cache beside the task directory, shared with `agent run`)"
  )]
  cache_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ProbeArgs {
  #[arg(long)]
  out: PathBuf,
  #[command(flatten)]
  claude: ClaudeArgs,
}

#[derive(Debug, Args)]
struct SurfaceArgs {
  #[arg(long, required_unless_present = "render_from")]
  atlas_bin: Option<PathBuf>,
  #[arg(long, conflicts_with_all = ["atlas_bin", "no_sessions", "work_dir", "extend_from"])]
  render_from: Option<PathBuf>,
  #[arg(long, requires = "atlas_bin", conflicts_with = "no_sessions")]
  extend_from: Option<PathBuf>,
  #[arg(long, default_value = "eval/reports/surface")]
  out: PathBuf,
  #[arg(long, default_value = agent::SURFACE_DEFAULT_MODEL)]
  model: String,
  #[arg(long, default_value = "claude")]
  claude_bin: PathBuf,
  #[arg(long)]
  repo_root: Option<PathBuf>,
  #[arg(long)]
  work_dir: Option<PathBuf>,
  #[arg(long)]
  no_sessions: bool,
  #[arg(
    long,
    value_name = "PROFILE",
    conflicts_with = "render_from",
    help = "Pass --tools PROFILE to catl mcp (core, read-only, full or name,name)"
  )]
  tools: Option<String>,
}

fn main() -> ExitCode {
  tracing_subscriber::fmt()
    .with_writer(std::io::stderr)
    .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
    .init();
  let cli = Cli::parse();
  let outcome: Result<(), Box<dyn std::error::Error>> = match cli.command {
    Command::Agent(AgentCommand::Run(args)) => run(&args).map_err(Into::into),
    Command::Agent(AgentCommand::Report(args)) => report(&args).map_err(Into::into),
    Command::Agent(AgentCommand::Probe(args)) => probe(&args).map_err(Into::into),
    Command::Agent(AgentCommand::Surface(args)) => surface(&args).map_err(Into::into),
    Command::Agent(AgentCommand::Headroom(args)) => headroom(&args).map_err(Into::into),
    Command::Agent(AgentCommand::Noise(args)) => noise(&args).map_err(Into::into),
    Command::Agent(AgentCommand::LeakAudit(args)) => leak_audit(&args).map_err(Into::into),
    Command::Quality(args) => run_quality(&args).map_err(Into::into),
    Command::Retrieval(args) => run_retrieval(&args),
  };
  match outcome {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      tracing::error!(%error, "atlas-eval failed");
      ExitCode::FAILURE
    }
  }
}

fn repo_root(explicit: Option<&Path>) -> Result<PathBuf, AgentError> {
  if let Some(path) = explicit {
    return Ok(path.to_owned());
  }
  let output = ProcessCommand::new("git")
    .args(["rev-parse", "--show-toplevel"])
    .output()
    .map_err(|error| AgentError::Command {
      command: "git rev-parse --show-toplevel".to_owned(),
      detail: error.to_string(),
    })?;
  if !output.status.success() {
    return Err(AgentError::InvalidArgument("not inside a git repository; pass --repo-root".to_owned()));
  }
  Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

fn locate_atlas_bin(explicit: Option<&Path>, repo_root: &Path) -> Option<PathBuf> {
  if let Some(path) = explicit {
    return Some(path.to_owned());
  }
  let built = ["release", "debug"]
    .iter()
    .map(|profile| repo_root.join("target").join(profile).join("catl"))
    .find(|path| path.is_file());
  built.or_else(|| {
    std::env::var_os("PATH")
      .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join("catl")).collect::<Vec<_>>())
      .unwrap_or_default()
      .into_iter()
      .find(|path| path.is_file())
  })
}

fn source_note(task: &LoadedTask, repo_override: Option<&Path>) -> String {
  if let Some(path) = repo_override {
    return format!("{}: tree copied from --repo-override {} (not reproducible from a commit)", task.spec.id, path.display());
  }
  let base = task.spec.base_commit.as_deref().unwrap_or("?");
  match &task.spec.repo {
    RepoSource::SelfRepo => format!("{}: this repository at {base}", task.spec.id),
    RepoSource::Fixture { name } => format!("{}: fixtures/{name} at {base}", task.spec.id),
    RepoSource::Git { url, commit } => format!("{}: {url} at {commit}", task.spec.id),
  }
}

fn run(args: &RunArgs) -> Result<(), AgentError> {
  let repo_root = repo_root(args.repo_root.as_deref())?;
  let tasks_dir = args.tasks.clone().unwrap_or_else(|| repo_root.join("eval").join("tasks"));
  let tasks = agent::load_tasks(&tasks_dir, &args.only)?;
  if let Some(path) = &args.repo_override
    && !path.is_dir()
  {
    return Err(AgentError::InvalidArgument(format!("--repo-override {} is not a directory", path.display())));
  }
  if args.runs_per_condition == 0 {
    return Err(AgentError::InvalidArgument("--runs-per-condition must be positive".to_owned()));
  }
  let mut conditions = args.condition.clone();
  conditions.sort();
  conditions.dedup();
  let atlas_config = args.atlas_config.as_deref().map(agent::AtlasConfig::load).transpose()?;
  let planned = planned_runs(args, &tasks, &conditions);
  let atlas_bin = locate_atlas_bin(args.atlas_bin.as_deref(), &repo_root);
  let judge_model = (args.judge == JudgeMode::Claude).then_some(args.claude.judge_model.as_str());
  if args.dry_run {
    let mut notes: Vec<String> = tasks.iter().map(|task| source_note(task, args.repo_override.as_deref())).collect();
    if conditions.contains(&Condition::Atlas) {
      notes.push(match &atlas_bin {
        Some(path) if path.is_file() => format!("catl binary: {}", path.display()),
        Some(path) => format!("catl binary {} does not exist yet; atlas runs would fail at setup", path.display()),
        None => "no catl binary found (target/release, target/debug, PATH); atlas runs would fail at setup — pass --atlas-bin".to_owned(),
      });
    }
    if let Some(atlas_config) = &atlas_config {
      notes.push(format!(
        "atlas config {} is copied into the session home of each atlas run and the duplication-scoring home (atlas-post-home) of every run, baseline included; the baseline session sees none of it; keys set: {}",
        atlas_config.describe(),
        atlas_config.keys().join(", ")
      ));
    }
    let context = DryRunContext {
      tasks: &tasks,
      runs: &planned,
      model: &args.claude.model,
      judge_model,
      seed: args.seed,
      parallel: args.parallel,
      budget_usd: args.budget_usd,
      preamble: args.atlas_preamble,
      notes: &notes,
    };
    print!("{}", agent::render_dry_run(&context));
    return Ok(());
  }
  let home = match &args.eval_home {
    Some(path) => EvalHome::new(path),
    None => EvalHome::of_tasks(&tasks_dir)?,
  };
  agent::keep_private_untracked(&[&tasks_dir], &[home.root()])?;
  let config = runner_config(args, repo_root, &home, atlas_bin, atlas_config);
  let manifest = agent::run_batch(&config, &tasks, planned)?;
  tracing::info!(
    batch = %config.batch_dir.display(),
    spent_usd = manifest.spent_usd,
    skipped = manifest.skipped_for_budget.len(),
    "batch finished; run `atlas-eval agent report --runs <batch>` to aggregate it"
  );
  Ok(())
}

fn planned_runs(args: &RunArgs, tasks: &[LoadedTask], conditions: &[Condition]) -> Vec<agent::PlannedRun> {
  if args.base_only {
    return Vec::new();
  }
  agent::plan_runs(tasks, conditions, args.runs_per_condition, args.seed)
}

fn runner_config(args: &RunArgs, repo_root: PathBuf, home: &EvalHome, atlas_bin: Option<PathBuf>, atlas_config: Option<agent::AtlasConfig>) -> RunnerConfig {
  let batch = args.batch.clone().unwrap_or_else(|| format!("{}-seed{}", agent::today_utc(), args.seed));
  RunnerConfig {
    repo_root,
    batch_dir: home.batch(&batch),
    cache_dir: home.clone_cache(),
    eval_home: home.root().to_owned(),
    tree_root: args.tree_root.clone().unwrap_or_else(std::env::temp_dir),
    keep_trees: args.keep_trees,
    claude: args.claude.config(),
    atlas_bin: atlas_bin.unwrap_or_else(|| PathBuf::from("catl")),
    repo_override: args.repo_override.clone(),
    judge: args.judge,
    parallel: args.parallel,
    budget_usd: args.budget_usd,
    validation_timeout: Duration::from_secs(args.validation_timeout_secs),
    atlas_timeout: Duration::from_secs(args.atlas_timeout_secs),
    seed: args.seed,
    probe: !args.no_probe && !args.base_only,
    preamble: args.atlas_preamble,
    atlas_config,
  }
}

fn report(args: &ReportArgs) -> Result<(), AgentError> {
  let (batch, records) = agent::load_runs(&args.runs)?;
  agent::ensure_single_preamble(batch.as_ref(), &records)?;
  let previous: Option<AgentReport> = args.compare.as_deref().map(agent::read_json).transpose()?;
  let report = agent::build_report(batch.as_ref(), &records, previous.as_ref());
  let date = batch.as_ref().map_or_else(agent::today_utc, |batch| batch.environment.date.clone());
  let base = agent::batch_report_base(std::slice::from_ref(&args.runs), &[], args.out.as_deref(), &date, "agent")?;
  let (markdown, json) = agent::write_report(&report, &base)?;
  tracing::info!(markdown = %markdown.display(), json = %json.display(), runs = records.len(), "report written");
  Ok(())
}

fn headroom_tasks(args: &HeadroomArgs) -> Result<PathBuf, AgentError> {
  if let Some(path) = &args.tasks {
    return Ok(path.clone());
  }
  let beside_runs = args.runs.first().and_then(|batch| EvalHome::of_batch(batch).ok()).map(|home| home.tasks());
  match beside_runs.filter(|tasks| args.repo_root.is_none() && tasks.is_dir()) {
    Some(tasks) => Ok(tasks),
    None => Ok(repo_root(args.repo_root.as_deref())?.join("eval").join("tasks")),
  }
}

fn headroom(args: &HeadroomArgs) -> Result<(), AgentError> {
  let tasks_dir = headroom_tasks(args)?;
  let base = agent::batch_report_base(&args.runs, &[&tasks_dir], args.out.as_deref(), &agent::today_utc(), "headroom")?;
  let report = agent::load_headroom(&args.runs, &tasks_dir)?;
  let (markdown, json) = agent::write_headroom(&report, &base)?;
  tracing::info!(markdown = %markdown.display(), json = %json.display(), sessions = report.sessions.len(), "headroom report written");
  Ok(())
}

fn noise(args: &NoiseArgs) -> Result<(), AgentError> {
  let compared: Vec<&Path> = args.ab_report.iter().map(PathBuf::as_path).collect();
  let base = agent::batch_report_base(&args.runs, &compared, args.out.as_deref(), &agent::today_utc(), "noise-floor")?;
  let report = agent::load_noise(&args.runs, &args.ab_report, repo_root(args.repo_root.as_deref()).ok().as_deref())?;
  let (markdown, json) = agent::write_noise(&report, &base)?;
  tracing::info!(markdown = %markdown.display(), json = %json.display(), metrics = report.metrics.len(), "noise-floor report written");
  Ok(())
}

fn leak_audit(args: &LeakAuditArgs) -> Result<(), AgentError> {
  let repo_root = repo_root(args.repo_root.as_deref())?;
  let tasks_dir = args.tasks.clone().unwrap_or_else(|| repo_root.join("eval").join("tasks"));
  let cache_dir = match &args.cache_dir {
    Some(path) => path.clone(),
    None => EvalHome::of_tasks(&tasks_dir)?.clone_cache(),
  };
  let base = agent::task_report_base(&tasks_dir, args.out.as_deref(), &[&cache_dir], &agent::today_utc(), "leak-audit")?;
  let report = agent::audit_leaks(&agent::load_tasks(&tasks_dir, &args.only)?, &repo_root, &cache_dir)?;
  let (markdown, json) = agent::write_leak_report(&report, &base)?;
  tracing::info!(markdown = %markdown.display(), json = %json.display(), tasks = report.tasks.len(), "leak audit written");
  match report.tasks.iter().filter(|task| task.disagrees()).count() {
    0 => Ok(()),
    count => Err(AgentError::LeakClassMismatch { count, report: markdown }),
  }
}

fn run_quality(args: &QualityArgs) -> Result<(), quality::QualityError> {
  let outcome = quality::execute(args)?;
  tracing::info!(markdown = %outcome.markdown.display(), json = %outcome.json.display(), "quality report written");
  if let Some(comparison) = &outcome.comparison {
    print!("{comparison}");
  }
  Ok(())
}

fn run_retrieval(args: &RetrievalArgs) -> Result<(), Box<dyn std::error::Error>> {
  let repo_root = repo_root(args.repo_root.as_deref())?;
  let outcome = retrieval::execute_retrieval(args, &repo_root)?;
  tracing::info!(markdown = %outcome.markdown.display(), json = %outcome.json.display(), "retrieval report written");
  Ok(())
}

fn surface(args: &SurfaceArgs) -> Result<(), AgentError> {
  let report = match (&args.render_from, &args.atlas_bin) {
    (Some(path), _) => agent::read_json(path)?,
    (None, Some(atlas_bin)) => measure_surface(args, atlas_bin)?,
    (None, None) => return Err(AgentError::InvalidArgument("--atlas-bin is required".to_owned())),
  };
  std::fs::create_dir_all(&args.out).map_err(|error| AgentError::io(&args.out, error))?;
  agent::write_json(&args.out.join("surface.json"), &report)?;
  let markdown_path = args.out.join("surface.md");
  std::fs::write(&markdown_path, agent::render_surface_markdown(&report)).map_err(|error| AgentError::io(&markdown_path, error))
}

fn measure_surface(args: &SurfaceArgs, atlas_bin: &Path) -> Result<agent::SurfaceReport, AgentError> {
  let repo_root = repo_root(args.repo_root.as_deref())?;
  let scratch = tempfile::tempdir().map_err(|error| AgentError::io(std::env::temp_dir(), error))?;
  let options = agent::SurfaceOptions {
    atlas_bin: atlas_bin.canonicalize().unwrap_or_else(|_| atlas_bin.to_owned()),
    claude: ClaudeConfig {
      binary: args.claude_bin.clone(),
      model: args.model.clone(),
      judge_model: DEFAULT_JUDGE_MODEL.to_owned(),
      config_isolation: ConfigIsolation::User,
    },
    fixture: repo_root.join("fixtures").join("acme"),
    work_dir: args.work_dir.clone().unwrap_or_else(|| scratch.path().to_owned()),
    sessions: !args.no_sessions,
    tools: args.tools.clone(),
  };
  match &args.extend_from {
    Some(path) => agent::extend_surface(&options, agent::read_json(path)?),
    None => agent::run_surface(&options),
  }
}

fn probe(args: &ProbeArgs) -> Result<(), AgentError> {
  let report = agent::run_probe(&args.out, &args.claude.config(), agent::default_user_claude_md().as_deref())?;
  let rendered = serde_json::to_string_pretty(&report).map_err(|error| AgentError::Parse {
    path: args.out.join("probe.json"),
    reason: error.to_string(),
  })?;
  println!("{rendered}");
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn run_args(flags: &[&str]) -> RunArgs {
    let cli = Cli::try_parse_from(["atlas-eval", "agent", "run"].iter().chain(flags)).unwrap();
    match cli.command {
      Command::Agent(AgentCommand::Run(args)) => *args,
      other => panic!("{other:?}"),
    }
  }

  fn config(flags: &[&str]) -> RunnerConfig {
    runner_config(&run_args(flags), PathBuf::from("/work"), &EvalHome::new("/work/eval"), None, None)
  }

  #[test]
  fn should_pass_the_atlas_config_directory_through_the_flag() {
    assert_eq!(run_args(&[]).atlas_config, None);
    assert_eq!(run_args(&["--atlas-config", "/cfg"]).atlas_config, Some(PathBuf::from("/cfg")));
  }

  #[test]
  fn should_start_no_probe_and_plan_no_run_for_a_base_only_batch() {
    let base_only = run_args(&["--base-only", "--batch", "verify"]);
    assert_eq!(planned_runs(&base_only, &[], &Condition::ALL), Vec::new());
    let checked = config(&["--base-only", "--batch", "verify"]);
    assert_eq!((checked.probe, checked.batch_dir), (false, PathBuf::from("/work/eval/runs/verify")));
    assert_eq!(checked.cache_dir, PathBuf::from("/work/eval/runs/.cache"));
    assert!(config(&[]).probe);
    assert!(!config(&["--no-probe"]).probe);
    assert!(Cli::try_parse_from(["atlas-eval", "agent", "run", "--base-only", "--dry-run"]).is_err());
  }
}
