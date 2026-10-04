use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::access::{AccessPolicy, contaminated, out_of_tree_accesses};
use super::accuracy::atlas_accuracy;
use super::atlas_cli::AtlasCli;
use super::claude::{ATLAS_SERVER_NAME, ATLAS_TOOL_PREFIX, ClaudeConfig, EMPTY_MCP_CONFIG, SessionRequest, host_env, project_instructions, session_invocation};
use super::diff::{Diff, FileChange};
use super::error::{AgentError, AgentResult};
use super::judge::{JudgeInput, JudgeKind, JudgeOutcome, atlas_review_prompt, blinded_prompt, run_judge};
use super::metrics::{Metrics, MetricsInput, compute_metrics};
use super::plan::{Condition, PREAMBLE_VERSION, PlannedRun, model_price, today_utc};
use super::probe::{ProbeReport, default_user_claude_md, run_probe};
use super::process::{ProcessExit, run_captured, run_streaming};
use super::record::{
  ATLAS_REVIEW_FILE, BASE_DIR, BASE_FILE, BATCH_FILE, BaseCheck, BatchManifest, CostSource, Coverage, DIFF_FILE, DuplicationCheck, Environment, HiddenTests, METRICS_FILE,
  OFFSETS_FILE, RUN_FILE, RunManifest, RunStatus, SCORE_FILE, Score, SessionOutcome, TRANSCRIPT_FILE, read_json, write_json,
};
use super::reuse::{analyze_source, duplication_findings, new_functions, reuse_references, same_name_as_targets};
use super::spec::LoadedTask;
use super::transcript::{TokenUsage, Transcript};
use super::validation::{SuiteResult, regressions, run_suite};
use super::workspace::{StripPlan, TreeSource, base_file, check_isolated_location, copy_hidden, diff_against, head_commit, init_base, materialize, resolve_source, scratch_dir};

const MAX_TURNS_SUBTYPE: &str = "error_max_turns";
const ATLAS_HOME: &str = "atlas-home";
const ATLAS_POST_HOME: &str = "atlas-post-home";
const ATLAS_INDEX_LOG: &str = "atlas-index.log";
const INDEX_DATA_SUFFIXES: [&str; 6] = [".db", ".db-wal", ".db-shm", ".sqlite", ".sqlite-wal", ".sqlite-shm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum JudgeMode {
  Claude,
  Off,
}

#[derive(Debug, Clone)]
pub struct RunnerConfig {
  pub repo_root: PathBuf,
  pub eval_home: PathBuf,
  pub batch_dir: PathBuf,
  pub tree_root: PathBuf,
  pub keep_trees: bool,
  pub cache_dir: PathBuf,
  pub claude: ClaudeConfig,
  pub atlas_bin: PathBuf,
  pub repo_override: Option<PathBuf>,
  pub judge: JudgeMode,
  pub parallel: usize,
  pub budget_usd: Option<f64>,
  pub validation_timeout: Duration,
  pub atlas_timeout: Duration,
  pub seed: u64,
  pub probe: bool,
}

struct RunContext<'a> {
  config: &'a RunnerConfig,
  task: &'a LoadedTask,
  source: &'a TreeSource,
  strip: &'a StripPlan,
  base: Option<&'a BaseCheck>,
  planned: &'a PlannedRun,
  dir: PathBuf,
}

pub fn environment(config: &RunnerConfig) -> Environment {
  let claude_version = {
    let mut command = Command::new(&config.claude.binary);
    command.arg("--version");
    let output = run_captured(command, None, Duration::from_secs(30));
    output.exit.succeeded().then(|| output.stdout.trim().to_owned())
  };
  Environment {
    date: today_utc(),
    seed: config.seed,
    claude_version,
    model: config.claude.model.clone(),
    judge_model: (config.judge == JudgeMode::Claude).then(|| config.claude.judge_model.clone()),
    config_isolation: config.claude.config_isolation,
    code_atlas_commit: head_commit(&config.repo_root),
    code_atlas_version: AtlasCli::new(&config.atlas_bin, config.cache_dir.join("atlas-version-home"), Duration::from_secs(10)).version(),
    atlas_bin: config.atlas_bin.display().to_string(),
    preamble_version: PREAMBLE_VERSION,
    repo_override: config.repo_override.as_ref().map(|path| path.display().to_string()),
    probe: None,
  }
}

pub fn run_batch(config: &RunnerConfig, tasks: &[LoadedTask], planned: Vec<PlannedRun>) -> AgentResult<BatchManifest> {
  fs::create_dir_all(&config.batch_dir).map_err(|error| AgentError::io(&config.batch_dir, error))?;
  let mut environment = environment(config);
  if config.probe {
    environment.probe = Some(probe(config)?);
  }
  let mut sources = BTreeMap::new();
  let mut bases = BTreeMap::new();
  for task in tasks {
    let source = resolve_source(task, &config.repo_root, config.repo_override.as_deref(), &config.cache_dir)?;
    let strip = StripPlan::for_task(task, config.repo_override.is_some());
    bases.insert(task.spec.id.clone(), base_check(config, task, &source, &strip)?);
    sources.insert(task.spec.id.clone(), (source, strip));
  }
  let initial_spent = environment.probe.as_ref().and_then(|probe| probe.cost_usd).unwrap_or(0.0);
  let queue = Mutex::new(planned.iter().cloned().collect::<VecDeque<_>>());
  let spent = Mutex::new(initial_spent);
  let skipped = Mutex::new(Vec::new());
  thread::scope(|scope| {
    for _ in 0..config.parallel.max(1) {
      scope.spawn(|| {
        loop {
          let Some(run) = queue.lock().unwrap_or_else(PoisonError::into_inner).pop_front() else {
            break;
          };
          let spent_so_far = *spent.lock().unwrap_or_else(PoisonError::into_inner);
          if config.budget_usd.is_some_and(|budget| spent_so_far > budget) {
            tracing::warn!(run = run.run_id, spent_so_far, "budget exceeded; not starting run");
            skipped.lock().unwrap_or_else(PoisonError::into_inner).push(run.run_id.clone());
            continue;
          }
          let (Some(task), Some((source, strip))) = (tasks.iter().find(|task| task.spec.id == run.task_id), sources.get(&run.task_id)) else {
            continue;
          };
          let context = RunContext {
            config,
            task,
            source,
            strip,
            base: bases.get(&run.task_id).and_then(Option::as_ref),
            planned: &run,
            dir: config.batch_dir.join(&run.run_id),
          };
          let cost = execute_run(&context).unwrap_or_else(|error| {
            tracing::error!(run = run.run_id, %error, "run failed before it was scored; it will be retried on resume");
            read_json::<RunManifest>(&context.dir.join(RUN_FILE)).map_or(0.0, |manifest| manifest.cost_usd)
          });
          *spent.lock().unwrap_or_else(PoisonError::into_inner) += cost;
        }
      });
    }
  });
  let truths = tasks.iter().map(|task| (task.spec.id.clone(), task.spec.ground_truth.clone())).collect();
  let manifest = BatchManifest {
    environment,
    tasks: tasks.iter().map(|task| task.spec.id.clone()).collect(),
    truths,
    planned,
    skipped_for_budget: skipped.into_inner().unwrap_or_else(PoisonError::into_inner),
    spent_usd: spent.into_inner().unwrap_or_else(PoisonError::into_inner),
  };
  write_json(&config.batch_dir.join(BATCH_FILE), &manifest)?;
  Ok(manifest)
}

fn probe(config: &RunnerConfig) -> AgentResult<ProbeReport> {
  let dir = config.batch_dir.join("_probe");
  let recorded = dir.join("probe.json");
  if recorded.is_file() {
    return read_json(&recorded);
  }
  run_probe(&dir, &config.claude, default_user_claude_md().as_deref())
}

fn isolated_scratch(config: &RunnerConfig, key: &str) -> AgentResult<PathBuf> {
  let scratch = scratch_dir(&config.tree_root, &config.batch_dir, key);
  remove_dir(&scratch)?;
  fs::create_dir_all(&scratch).map_err(|error| AgentError::io(&scratch, error))?;
  check_isolated_location(&scratch, &[&config.repo_root, &config.eval_home])?;
  Ok(scratch)
}

fn isolated_atlas_home(config: &RunnerConfig, scratch: &Path, name: &str) -> AgentResult<PathBuf> {
  let home = scratch.join(name);
  fs::create_dir_all(&home).map_err(|error| AgentError::io(&home, error))?;
  check_isolated_location(&home, &[&config.repo_root, &config.eval_home])?;
  Ok(home)
}

fn preserve_atlas_artifacts(scratch: &Path, run_dir: &Path) {
  for home in [ATLAS_HOME, ATLAS_POST_HOME] {
    let source = scratch.join(home);
    if !source.is_dir() {
      continue;
    }
    if let Err(error) = copy_small_artifacts(&source, &run_dir.join(home)) {
      tracing::warn!(home, %error, "cannot preserve Code Atlas artifacts");
    }
  }
}

fn copy_small_artifacts(from: &Path, to: &Path) -> std::io::Result<()> {
  fs::create_dir_all(to)?;
  for entry in fs::read_dir(from)? {
    let entry = entry?;
    let path = entry.path();
    let name = entry.file_name().to_string_lossy().into_owned();
    if entry.file_type()?.is_dir() {
      copy_small_artifacts(&path, &to.join(&name))?;
    } else if !INDEX_DATA_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
      fs::copy(&path, to.join(&name))?;
    }
  }
  Ok(())
}

fn release_scratch(config: &RunnerConfig, scratch: &Path) {
  if config.keep_trees {
    return;
  }
  if let Err(error) = fs::remove_dir_all(scratch) {
    tracing::warn!(scratch = %scratch.display(), %error, "cannot remove a scratch tree");
  }
}

fn base_check(config: &RunnerConfig, task: &LoadedTask, source: &TreeSource, strip: &StripPlan) -> AgentResult<Option<BaseCheck>> {
  let validation = &task.spec.validation;
  if validation.commands.is_empty() && validation.regression.is_empty() {
    return Ok(None);
  }
  let dir = config.batch_dir.join(BASE_DIR).join(&task.spec.id);
  let path = dir.join(BASE_FILE);
  if path.is_file() {
    return read_json(&path).map(Some);
  }
  remove_dir(&dir)?;
  fs::create_dir_all(&dir).map_err(|error| AgentError::io(&dir, error))?;
  let scratch = isolated_scratch(config, &format!("{BASE_DIR}/{}", task.spec.id))?;
  let tree = scratch.join("tree");
  materialize(source, &tree, strip)?;
  init_base(&tree)?;
  copy_hidden(task, &tree)?;
  let check = BaseCheck {
    task_id: task.spec.id.clone(),
    validation: run_suite(&validation.commands, &tree, config.validation_timeout),
    regression: run_suite(&validation.regression, &tree, config.validation_timeout),
  };
  release_scratch(config, &scratch);
  if check.validation.all_passed() && !check.validation.commands.is_empty() {
    tracing::warn!(task = task.spec.id, "hidden validation already passes on the base tree; the task does not discriminate");
  }
  write_json(&path, &check)?;
  Ok(Some(check))
}

fn remove_dir(dir: &Path) -> AgentResult<()> {
  match fs::remove_dir_all(dir) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(AgentError::io(dir, error)),
    _ => Ok(()),
  }
}

fn execute_run(context: &RunContext<'_>) -> AgentResult<f64> {
  let dir = &context.dir;
  if dir.join(SCORE_FILE).is_file() {
    let manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    tracing::info!(run = manifest.run_id, "run already scored; reusing it");
    return Ok(manifest.cost_usd);
  }
  remove_dir(dir)?;
  fs::create_dir_all(dir).map_err(|error| AgentError::io(dir, error))?;
  let mut manifest = RunManifest {
    run_id: context.planned.run_id.clone(),
    task_id: context.task.spec.id.clone(),
    condition: context.planned.condition,
    run_index: context.planned.run_index,
    order: context.planned.order,
    spec_hash: context.task.spec_hash.clone(),
    source: context.source.clone(),
    base_sha: None,
    tree: None,
    project_instructions: Vec::new(),
    model_requested: context.config.claude.model.clone(),
    models_reported: Vec::new(),
    claude_code_version: None,
    preamble_version: (context.planned.condition == Condition::Atlas).then_some(PREAMBLE_VERSION),
    args: Vec::new(),
    env_names: Vec::new(),
    atlas_project: None,
    atlas_index_ms: None,
    wall_ms: None,
    session_cost_usd: None,
    cost_source: CostSource::Unknown,
    cost_usd: 0.0,
    status: RunStatus::SetupFailed {
      stage: "prepare".to_owned(),
      reason: "not started".to_owned(),
    },
  };
  let prepared = isolated_scratch(context.config, &context.planned.run_id)
    .map_err(|error| ("isolation", error))
    .and_then(|scratch| prepare_tree(context, &scratch, &mut manifest).map(|base_sha| (scratch, base_sha)));
  let (scratch, base_sha) = match prepared {
    Ok(prepared) => prepared,
    Err((stage, error)) => {
      manifest.status = RunStatus::SetupFailed {
        stage: stage.to_owned(),
        reason: error.to_string(),
      };
      write_json(&dir.join(RUN_FILE), &manifest)?;
      if let Some(scratch) = manifest.tree.as_deref().and_then(|tree| Path::new(tree).parent()) {
        preserve_atlas_artifacts(scratch, dir);
        release_scratch(context.config, scratch);
      }
      return Ok(0.0);
    }
  };
  let tree = scratch.join("tree");
  let scored = score_session(context, &scratch, &tree, &base_sha, &mut manifest);
  preserve_atlas_artifacts(&scratch, dir);
  release_scratch(context.config, &scratch);
  scored
}

fn score_session(context: &RunContext<'_>, scratch: &Path, tree: &Path, base_sha: &str, manifest: &mut RunManifest) -> AgentResult<f64> {
  let dir = &context.dir;
  let session = run_session(context, scratch, tree, manifest)?;
  let diff_text = diff_against(tree, base_sha)?;
  fs::write(dir.join(DIFF_FILE), &diff_text).map_err(|error| AgentError::io(dir.join(DIFF_FILE), error))?;
  let diff = Diff::parse(&diff_text);
  let duplication = duplication_check(context, scratch, tree, base_sha, &diff);
  copy_hidden(context.task, tree)?;
  let spec = &context.task.spec;
  let validation = run_suite(&spec.validation.commands, tree, context.config.validation_timeout);
  let regression = run_suite(&spec.validation.regression, tree, context.config.validation_timeout);
  let changed = diff.changed_files();
  let mut metrics = compute_metrics(&MetricsInput {
    transcript: &session,
    tree,
    ground_truth: &spec.ground_truth,
    changed_files: &changed,
    wall_ms: manifest.wall_ms,
  });
  metrics.out_of_tree_accesses = out_of_tree_accesses(&session, &access_policy(context.config, scratch, tree, &session));
  metrics.contaminated = contaminated(&metrics.out_of_tree_accesses);
  if metrics.contaminated {
    tracing::warn!(
      run = context.planned.run_id,
      accesses = metrics.out_of_tree_accesses.len(),
      "the session touched the repository, the eval home or another run"
    );
  }
  write_json(&dir.join(METRICS_FILE), &metrics)?;
  let judge = judge_run(context, &diff_text, &validation, &regression, &metrics);
  let atlas_review = (context.planned.condition == Condition::Atlas).then(|| review_atlas(context, &session, &diff_text));
  if let Some(review) = &atlas_review {
    write_json(&dir.join(ATLAS_REVIEW_FILE), review)?;
  }
  let outcomes = Outcomes {
    validation,
    regression,
    duplication,
    judge: judge.clone(),
  };
  let score = build_score(context, &session, &diff, &metrics, outcomes);
  write_json(&dir.join(SCORE_FILE), &score)?;
  manifest.cost_usd += judge.cost_usd().unwrap_or(0.0) + atlas_review.as_ref().and_then(JudgeOutcome::cost_usd).unwrap_or(0.0);
  write_json(&dir.join(RUN_FILE), manifest)?;
  Ok(manifest.cost_usd)
}

fn access_policy(config: &RunnerConfig, scratch: &Path, tree: &Path, session: &Transcript) -> AccessPolicy {
  let mut trees = vec![tree.to_owned()];
  trees.extend(tree.canonicalize().ok());
  trees.extend(session.init.as_ref().and_then(|init| init.cwd.as_deref()).map(PathBuf::from));
  AccessPolicy {
    trees,
    home: std::env::var_os("HOME").map(PathBuf::from),
    repo_root: config.repo_root.canonicalize().unwrap_or_else(|_| config.repo_root.clone()),
    eval_home: config.eval_home.canonicalize().unwrap_or_else(|_| config.eval_home.clone()),
    scratch_root: config.tree_root.canonicalize().unwrap_or_else(|_| config.tree_root.clone()),
    own_scratch: scratch.canonicalize().unwrap_or_else(|_| scratch.to_owned()),
  }
}

fn prepare_tree(context: &RunContext<'_>, scratch: &Path, manifest: &mut RunManifest) -> Result<String, (&'static str, AgentError)> {
  let tree = &scratch.join("tree");
  manifest.tree = Some(tree.display().to_string());
  materialize(context.source, tree, context.strip).map_err(|error| ("materialize", error))?;
  let base_sha = init_base(tree).map_err(|error| ("git-base", error))?;
  manifest.base_sha = Some(base_sha.clone());
  let mcp_path = scratch.join("mcp.json");
  let mcp_config = match context.planned.condition {
    Condition::Baseline => EMPTY_MCP_CONFIG.to_owned(),
    Condition::Atlas => {
      let home = isolated_atlas_home(context.config, scratch, ATLAS_HOME).map_err(|error| ("isolation", error))?;
      let atlas = AtlasCli::new(&context.config.atlas_bin, home, context.config.atlas_timeout);
      let project = project_name(&context.config.batch_dir, &context.planned.run_id);
      atlas
        .project_create(&project, &atlas_repos(context.task, tree), tree)
        .map_err(|error| ("atlas-project", error))?;
      let index = atlas.index(&project, tree).map_err(|error| ("atlas-index", error))?;
      let log = context.dir.join(ATLAS_INDEX_LOG);
      fs::write(&log, format!("{}\n--- stderr ---\n{}", index.stdout, index.stderr)).map_err(|error| ("atlas-index", AgentError::io(&log, error)))?;
      manifest.atlas_index_ms = Some(millis(index.duration));
      manifest.atlas_project = Some(project.clone());
      atlas.mcp_config(&project).to_string()
    }
  };
  for path in [mcp_path, context.dir.join("mcp.json")] {
    fs::write(&path, &mcp_config).map_err(|error| ("mcp-config", AgentError::io(&path, error)))?;
  }
  Ok(base_sha)
}

fn atlas_repos(task: &LoadedTask, tree: &Path) -> Vec<PathBuf> {
  if task.spec.atlas_repos.is_empty() {
    return vec![tree.to_owned()];
  }
  task.spec.atlas_repos.iter().map(|repo| tree.join(repo)).collect()
}

fn project_name(batch_dir: &Path, run_id: &str) -> String {
  let digest = blake3::hash(format!("{}/{run_id}", batch_dir.display()).as_bytes()).to_hex();
  format!("eval-{}", &digest[..12])
}

fn run_session(context: &RunContext<'_>, scratch: &Path, tree: &Path, manifest: &mut RunManifest) -> AgentResult<Transcript> {
  let dir = &context.dir;
  let spec = &context.task.spec;
  let instructions = project_instructions(tree);
  manifest.project_instructions = instructions.as_ref().map(|(_, names)| names.clone()).unwrap_or_default();
  let request = SessionRequest {
    prompt: context.planned.condition.prompt(&spec.prompt),
    tree: tree.to_owned(),
    mcp_config: scratch.join("mcp.json"),
    max_turns: spec.max_turns,
    with_atlas_tools: context.planned.condition == Condition::Atlas,
    config_dir: scratch.join("claude-config"),
    project_instructions: instructions.map(|(text, _)| text),
  };
  let invocation = session_invocation(&context.config.claude, &request, &host_env());
  manifest.args = invocation.args.clone();
  manifest.env_names = invocation.env_names();
  let transcript_path = dir.join(TRANSCRIPT_FILE);
  let timeout = Duration::from_secs(spec.timeout_minutes * 60);
  tracing::info!(run = context.planned.run_id, "starting claude session");
  let output = run_streaming(invocation.command(), timeout, &transcript_path, &dir.join("claude.stderr.log")).map_err(|error| AgentError::io(&transcript_path, error))?;
  write_json(&dir.join(OFFSETS_FILE), &output.line_offsets_ms)?;
  let text = fs::read_to_string(&transcript_path).map_err(|error| AgentError::io(&transcript_path, error))?;
  let transcript = Transcript::parse(&text, &output.line_offsets_ms);
  manifest.wall_ms = Some(millis(output.duration));
  manifest.models_reported = transcript.reported_models();
  manifest.claude_code_version = transcript.init.as_ref().and_then(|init| init.claude_code_version.clone());
  let (session_cost, cost_source) = session_cost(&transcript, &context.config.claude.model);
  manifest.session_cost_usd = session_cost;
  manifest.cost_source = cost_source;
  manifest.cost_usd = session_cost.unwrap_or(0.0);
  manifest.status = match atlas_server_problem(context.planned.condition, &transcript) {
    Some(reason) => RunStatus::SetupFailed {
      stage: "atlas-mcp".to_owned(),
      reason,
    },
    None => RunStatus::Completed {
      session: session_outcome(&transcript, &output.exit),
    },
  };
  write_json(&dir.join(RUN_FILE), manifest)?;
  Ok(transcript)
}

fn atlas_server_problem(condition: Condition, transcript: &Transcript) -> Option<String> {
  if condition != Condition::Atlas {
    return None;
  }
  let servers = transcript.init.as_ref().map(|init| init.mcp_servers.as_slice()).unwrap_or_default();
  match servers.iter().find(|server| server.name == ATLAS_SERVER_NAME) {
    Some(server) if server.status == "connected" => None,
    Some(server) => Some(format!("the {ATLAS_SERVER_NAME} MCP server reported status `{}`", server.status)),
    None => Some(format!("the session did not list the {ATLAS_SERVER_NAME} MCP server")),
  }
}

fn session_outcome(transcript: &Transcript, exit: &ProcessExit) -> SessionOutcome {
  match (&transcript.result, exit) {
    (Some(result), _) if result.subtype.as_deref() == Some(MAX_TURNS_SUBTYPE) => SessionOutcome::MaxTurns,
    (Some(result), _) if result.is_error => SessionOutcome::ReportedError {
      subtype: result.subtype.clone().unwrap_or_default(),
    },
    (Some(result), _) => SessionOutcome::Finished {
      subtype: result.subtype.clone().unwrap_or_default(),
    },
    (None, ProcessExit::TimedOut) => SessionOutcome::TimedOut,
    (None, exit) => SessionOutcome::NoResultEvent { exit: exit.clone() },
  }
}

fn session_cost(transcript: &Transcript, model: &str) -> (Option<f64>, CostSource) {
  if let Some(cost) = transcript.result.as_ref().and_then(|result| result.total_cost_usd) {
    return (Some(cost), CostSource::Reported);
  }
  let usage: TokenUsage = transcript.turn_usage_total();
  let Some(price) = model_price(model) else {
    tracing::warn!(model, "no reported cost and no price for the model; the budget cannot count this run");
    return (None, CostSource::Unknown);
  };
  tracing::warn!(model, "no result event; estimating the session cost from per-turn usage");
  let cost = (usage.input as f64 * price.input + usage.output as f64 * price.output + usage.cache_read as f64 * price.cache_read + usage.cache_creation as f64 * price.cache_write)
    / 1_000_000.0;
  (Some(cost), CostSource::EstimatedFromUsage)
}

fn duplication_check(context: &RunContext<'_>, scratch: &Path, tree: &Path, base_sha: &str, diff: &Diff) -> DuplicationCheck {
  let truth = &context.task.spec.ground_truth;
  if truth.reuse_targets.is_empty() && truth.forbidden_duplicates.is_empty() {
    return DuplicationCheck::NotApplicable {
      reason: "the task lists no reuse targets or forbidden duplicates".to_owned(),
    };
  }
  let mut added = Vec::new();
  for file in diff.files.iter().filter(|file| file.change != FileChange::Deleted && !file.binary) {
    let Ok(source) = fs::read_to_string(tree.join(&file.path)) else {
      continue;
    };
    let Some(post) = analyze_source(&file.path, &source) else {
      continue;
    };
    let base_source = match file.change {
      FileChange::Added => None,
      _ => base_file(tree, base_sha, file.old_path.as_deref().unwrap_or(&file.path)),
    };
    let base = base_source.and_then(|source| analyze_source(&file.path, &source));
    added.extend(new_functions(&file.path, base.as_ref(), &post));
  }
  if added.is_empty() {
    return DuplicationCheck::Checked {
      new_functions: added,
      findings: Vec::new(),
      failed_queries: Vec::new(),
    };
  }
  let home = match isolated_atlas_home(context.config, scratch, ATLAS_POST_HOME) {
    Ok(home) => home,
    Err(error) => return DuplicationCheck::Unavailable { reason: error.to_string() },
  };
  let atlas = AtlasCli::new(&context.config.atlas_bin, home, context.config.atlas_timeout);
  let project = format!("{}-post", project_name(&context.config.batch_dir, &context.planned.run_id));
  let indexed = atlas
    .project_create(&project, &atlas_repos(context.task, tree), tree)
    .and_then(|()| atlas.index(&project, tree).map(|_| ()));
  if let Err(error) = indexed {
    return DuplicationCheck::Unavailable { reason: error.to_string() };
  }
  let mut findings = Vec::new();
  let mut failed_queries = Vec::new();
  for function in &added {
    match atlas.similar_snippet(&function.path, function.start_line, function.end_line, tree) {
      Ok(matches) => findings.extend(duplication_findings(function, &matches, &truth.reuse_targets, &truth.forbidden_duplicates)),
      Err(error) => failed_queries.push(format!("{}:{}: {error}", function.path, function.start_line)),
    }
  }
  DuplicationCheck::Checked {
    new_functions: added,
    findings,
    failed_queries,
  }
}

fn judge_run(context: &RunContext<'_>, diff: &str, validation: &SuiteResult, regression: &SuiteResult, metrics: &Metrics) -> JudgeOutcome {
  if context.config.judge == JudgeMode::Off {
    return JudgeOutcome::Skipped {
      reason: "judge disabled".to_owned(),
    };
  }
  let prompt = blinded_prompt(&JudgeInput {
    task_prompt: &context.task.spec.prompt,
    diff,
    validation,
    regression,
    files_read: &metrics.reads.files,
    files_changed: &metrics.relevance.files_changed,
  });
  run_judge(&context.config.claude, JudgeKind::BlindedRubric, &prompt, &context.dir.join("judge"))
}

fn review_atlas(context: &RunContext<'_>, transcript: &Transcript, diff: &str) -> JudgeOutcome {
  if context.config.judge == JudgeMode::Off {
    return JudgeOutcome::Skipped {
      reason: "judge disabled".to_owned(),
    };
  }
  let excerpts: Vec<(String, String, String)> = transcript
    .tool_calls
    .iter()
    .filter(|call| call.name.starts_with(ATLAS_TOOL_PREFIX))
    .map(|call| {
      let response = call.result.as_ref().map(|result| result.text.clone()).unwrap_or_default();
      (call.name.trim_start_matches(ATLAS_TOOL_PREFIX).to_owned(), call.input.to_string(), response)
    })
    .collect();
  if excerpts.is_empty() {
    return JudgeOutcome::Skipped {
      reason: "the session made no Code Atlas calls".to_owned(),
    };
  }
  let prompt = atlas_review_prompt(&context.task.spec.prompt, &excerpts, diff);
  run_judge(&context.config.claude, JudgeKind::AtlasReview, &prompt, &context.dir.join("atlas-judge"))
}

struct Outcomes {
  validation: SuiteResult,
  regression: SuiteResult,
  duplication: DuplicationCheck,
  judge: JudgeOutcome,
}

fn build_score(context: &RunContext<'_>, transcript: &Transcript, diff: &Diff, metrics: &Metrics, outcomes: Outcomes) -> Score {
  let Outcomes {
    validation,
    regression,
    duplication,
    judge,
  } = outcomes;
  let truth = &context.task.spec.ground_truth;
  let changed: BTreeSet<String> = diff.changed_files();
  let changed_refs: BTreeSet<&String> = changed.iter().collect();
  let considered: BTreeSet<&String> = changed.iter().chain(&metrics.reads.files).chain(&metrics.atlas.must_read_files).collect();
  let new_functions = match &duplication {
    DuplicationCheck::Checked { new_functions, .. } => new_functions.clone(),
    _ => Vec::new(),
  };
  let targets: Vec<String> = truth.reuse_targets.iter().chain(&truth.forbidden_duplicates).cloned().collect();
  let found = regressions(context.base.map(|base| &base.regression), &regression);
  let hidden_tests = (!validation.commands.is_empty()).then(|| HiddenTests {
    passed: validation.all_passed(),
    commands_passed: validation.passed_commands(),
    commands_total: validation.commands.len(),
    counts: validation.counts(),
  });
  Score {
    hidden_tests,
    regression_count: found.count(),
    regressions: found,
    validation,
    regression,
    must_change_coverage: Coverage::of(&truth.must_change, &changed_refs),
    must_consider_coverage: Coverage::of(&truth.must_consider, &considered),
    reuse_references: reuse_references(diff, &truth.reuse_targets),
    duplication_introduced: duplication.introduced(),
    duplication,
    new_functions_named_like_targets: same_name_as_targets(&new_functions, &targets),
    judge,
    atlas_accuracy: (context.planned.condition == Condition::Atlas).then(|| atlas_accuracy(transcript)),
  }
}

fn millis(duration: Duration) -> u64 {
  u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
