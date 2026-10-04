use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use atlas_eval::agent::{
  self, AgentError, AgentReport, ClaudeConfig, Condition, ConfigIsolation, DEFAULT_JUDGE_MODEL, DEFAULT_SESSION_MODEL, DryRunContext, JudgeMode, LoadedTask, RepoSource,
  RunnerConfig,
};

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
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
  #[command(about = "Run the planned sessions, score them, and write run artifacts under <eval-home>/runs/<batch>")]
  Run(Box<RunArgs>),
  #[command(about = "Aggregate a batch of runs into agent.md and agent.json")]
  Report(ReportArgs),
  #[command(about = "Run one tiny session with the baseline flags and record which isolation they achieve")]
  Probe(ProbeArgs),
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
  #[arg(long)]
  atlas_bin: Option<PathBuf>,
  #[arg(long)]
  repo_override: Option<PathBuf>,
  #[arg(long, value_enum, default_value_t = JudgeMode::Claude)]
  judge: JudgeMode,
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
struct ProbeArgs {
  #[arg(long)]
  out: PathBuf,
  #[command(flatten)]
  claude: ClaudeArgs,
}

fn main() -> ExitCode {
  tracing_subscriber::fmt()
    .with_writer(std::io::stderr)
    .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
    .init();
  let cli = Cli::parse();
  let outcome = match cli.command {
    Command::Agent(AgentCommand::Run(args)) => run(&args),
    Command::Agent(AgentCommand::Report(args)) => report(&args),
    Command::Agent(AgentCommand::Probe(args)) => probe(&args),
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
    .map(|profile| repo_root.join("target").join(profile).join("code-atlas"))
    .find(|path| path.is_file());
  built.or_else(|| {
    std::env::var_os("PATH")
      .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join("code-atlas")).collect::<Vec<_>>())
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
  let planned = agent::plan_runs(&tasks, &conditions, args.runs_per_condition, args.seed);
  let atlas_bin = locate_atlas_bin(args.atlas_bin.as_deref(), &repo_root);
  let judge_model = (args.judge == JudgeMode::Claude).then_some(args.claude.judge_model.as_str());
  if args.dry_run {
    let mut notes: Vec<String> = tasks.iter().map(|task| source_note(task, args.repo_override.as_deref())).collect();
    if conditions.contains(&Condition::Atlas) {
      notes.push(match &atlas_bin {
        Some(path) if path.is_file() => format!("code-atlas binary: {}", path.display()),
        Some(path) => format!("code-atlas binary {} does not exist yet; atlas runs would fail at setup", path.display()),
        None => "no code-atlas binary found (target/release, target/debug, PATH); atlas runs would fail at setup — pass --atlas-bin".to_owned(),
      });
    }
    let context = DryRunContext {
      tasks: &tasks,
      runs: &planned,
      model: &args.claude.model,
      judge_model,
      seed: args.seed,
      parallel: args.parallel,
      budget_usd: args.budget_usd,
      notes: &notes,
    };
    print!("{}", agent::render_dry_run(&context));
    return Ok(());
  }
  let eval_home = args.eval_home.clone().unwrap_or_else(|| repo_root.join("eval"));
  let batch = args.batch.clone().unwrap_or_else(|| format!("{}-seed{}", agent::today_utc(), args.seed));
  let config = RunnerConfig {
    repo_root,
    batch_dir: eval_home.join("runs").join(batch),
    cache_dir: eval_home.join("runs").join(".cache"),
    eval_home,
    tree_root: args.tree_root.clone().unwrap_or_else(std::env::temp_dir),
    keep_trees: args.keep_trees,
    claude: args.claude.config(),
    atlas_bin: atlas_bin.unwrap_or_else(|| PathBuf::from("code-atlas")),
    repo_override: args.repo_override.clone(),
    judge: args.judge,
    parallel: args.parallel,
    budget_usd: args.budget_usd,
    validation_timeout: Duration::from_secs(args.validation_timeout_secs),
    atlas_timeout: Duration::from_secs(args.atlas_timeout_secs),
    seed: args.seed,
    probe: !args.no_probe,
  };
  let manifest = agent::run_batch(&config, &tasks, planned)?;
  tracing::info!(
    batch = %config.batch_dir.display(),
    spent_usd = manifest.spent_usd,
    skipped = manifest.skipped_for_budget.len(),
    "batch finished; run `atlas-eval agent report --runs <batch>` to aggregate it"
  );
  Ok(())
}

fn report(args: &ReportArgs) -> Result<(), AgentError> {
  let (batch, records) = agent::load_runs(&args.runs)?;
  let previous: Option<AgentReport> = args.compare.as_deref().map(agent::read_json).transpose()?;
  let report = agent::build_report(batch.as_ref(), &records, previous.as_ref());
  let date = batch.as_ref().map_or_else(agent::today_utc, |batch| batch.environment.date.clone());
  let out = args.out.clone().unwrap_or_else(|| PathBuf::from("eval").join("reports").join(date).join("agent"));
  let base = match out.extension().and_then(|extension| extension.to_str()) {
    Some("md" | "json") => out.with_extension(""),
    _ => out,
  };
  let (markdown, json) = agent::write_report(&report, &base)?;
  tracing::info!(markdown = %markdown.display(), json = %json.display(), runs = records.len(), "report written");
  Ok(())
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
