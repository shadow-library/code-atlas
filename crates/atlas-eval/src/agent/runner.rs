use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::access::{AccessPolicy, LinkedPolicy, LinkedReads, contaminated, out_of_tree_accesses};
use super::accuracy::atlas_accuracy;
use super::atlas_cli::{AtlasCli, RepoGlobs};
use super::atlas_config::AtlasConfig;
use super::bash_guard::{bash_guard_denials, check_bash_guard};
use super::claude::{
  ATLAS_SERVER_NAME, ClaudeConfig, EMPTY_MCP_CONFIG, HookWiring, SERENA_SERVER_NAME, SessionRequest, SessionTools, host_env, project_instructions, session_invocation,
};
use super::diff::{Diff, FileChange};
use super::error::{AgentError, AgentResult};
use super::graphify::{GraphBuild, Graphify, ensure_graphs, exclude_tool_dirs, install_graphs};
use super::hooks::{AtlasHooks, AtlasHooksRecord, USER_PROMPT_SUBMIT, hook_metrics, session_settings};
use super::incumbent::{INCUMBENT_HOOKS_FILE, IncumbentRecord, IncumbentRun, IncumbentTools, incumbent_instructions, incumbent_instructions_digest};
use super::incumbent_probe::{INCUMBENT_PROBE_DIR, INCUMBENT_PROBE_FILE, IncumbentProbe, IncumbentProbeRequest, incumbent_hooks_problem, run_incumbent_probe};
use super::judge::{JudgeOutcome, judge_atlas_review, judge_rubric};
use super::metrics::{LinkedInput, LinkedRestore, Metrics, MetricsInput, compute_metrics, incumbent_metrics, linked_metrics};
use super::plan::{AtlasSurface, Condition, PlannedRun, PreambleVariant, model_price, today_utc};
use super::probe::{HookedProbe, HookedProbeRequest, ProbeReport, default_user_claude_md, run_hooked_probe, run_probe};
use super::process::{ProcessExit, run_captured, run_streaming};
use super::record::{
  ATLAS_REVIEW_FILE, BASE_DIR, BASE_FILE, BATCH_FILE, BaseCheck, BatchManifest, CostSource, Coverage, DIFF_FILE, DuplicationCheck, Environment, HiddenTests, METRICS_FILE,
  OFFSETS_FILE, RUN_FILE, RunManifest, RunStatus, SCORE_FILE, Score, SessionOutcome, TRANSCRIPT_FILE, read_json, write_json,
};
use super::report::HELD_OUT_CATEGORY;
use super::reuse::{analyze_source, duplication_findings, new_functions, reuse_references, same_name_as_targets};
use super::spec::{LinkedSpec, LoadedTask, SESSION_REPO};
use super::transcript::{TokenUsage, Transcript};
use super::validation::{SuiteResult, regressions, run_command, run_suite};
use super::workspace::{
  LinkedLayout, Restored, StripPlan, TreeSource, apply_linked_patch, base_file, check_isolated_location, copy_hidden, diff_against, head_commit, init_base, local_source_path,
  materialize_task, resolve_source, scratch_dir,
};

const MAX_TURNS_SUBTYPE: &str = "error_max_turns";
const ATLAS_HOME: &str = "atlas-home";
const ATLAS_POST_HOME: &str = "atlas-post-home";
const ATLAS_INDEX_LOG: &str = "atlas-index.log";
const ATLAS_HOOKS_FILE: &str = "atlas-hooks.json";
const ATLAS_HOOKS_USER: &str = "atlas-hooks-user";
const ATLAS_HOOKS_STAGE: &str = "atlas-hooks";
const PROBE_DIR: &str = "_probe";
const HOOKED_PROBE_DIR: &str = "hooks";
const FORCED_PROBE_PROJECT: &str = "_probe/forced";
const FORCED_PROBE_SOURCE: (&str, &str) = ("probe.ts", "export const probe = \"ATLAS-EVAL-GUARD-PROBE\";\n");
const CARGO_TARGET: &str = "cargo-target";
const BATCH_LOCK_FILE: &str = ".batch.lock";
const BATCH_LOCK_GRACE: Duration = Duration::from_secs(5);
const BATCH_LOCK_POLL: Duration = Duration::from_millis(25);
const INCUMBENT_WARM_UP: &str = "_incumbent/warm-up";
const GRAPH_BUILD_DIR: &str = "graphify-build";
const INCUMBENT_GRAPHIFY_STAGE: &str = "incumbent-graphify";
const INCUMBENT_SERENA_STAGE: &str = "incumbent-serena";
const INCUMBENT_HOOKS_STAGE: &str = "incumbent-hooks";
const INDEX_DATA_SUFFIXES: [&str; 6] = [".db", ".db-wal", ".db-shm", ".sqlite", ".sqlite-wal", ".sqlite-shm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum JudgeMode {
  Claude,
  Off,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SurfaceSetting {
  #[default]
  Standard,
  Forced {
    bash_guard: PathBuf,
  },
}

impl SurfaceSetting {
  pub fn resolve(surface: AtlasSurface) -> AgentResult<Self> {
    match surface {
      AtlasSurface::Standard => Ok(Self::Standard),
      AtlasSurface::Forced => std::env::current_exe()
        .map(|bash_guard| Self::Forced { bash_guard })
        .map_err(|error| AgentError::AtlasSurfaceUnavailable {
          reason: format!("cannot locate the running atlas-eval binary for the Bash guard: {error}"),
        }),
    }
  }

  pub fn surface(&self) -> AtlasSurface {
    match self {
      Self::Standard => AtlasSurface::Standard,
      Self::Forced { .. } => AtlasSurface::Forced,
    }
  }

  pub fn bash_guard(&self) -> Option<&Path> {
    match self {
      Self::Standard => None,
      Self::Forced { bash_guard } => Some(bash_guard),
    }
  }
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
  pub preamble: PreambleVariant,
  pub atlas_config: Option<AtlasConfig>,
  pub atlas_hooks: bool,
  pub atlas_surface: SurfaceSetting,
  pub incumbent: Option<IncumbentTools>,
}

struct RunContext<'a> {
  config: &'a RunnerConfig,
  hooks: Option<&'a AtlasHooksRecord>,
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
    atlas_bin_blake3: fs::read(&config.atlas_bin).ok().map(|bytes| blake3::hash(&bytes).to_hex().to_string()),
    atlas_bin: config.atlas_bin.display().to_string(),
    preamble_version: config.preamble.version(),
    preamble_variant: config.preamble,
    repo_override: config.repo_override.as_ref().map(|path| path.display().to_string()),
    atlas_config: config.atlas_config.as_ref().map(AtlasConfig::record),
    atlas_hooks: None,
    atlas_surface: config.atlas_surface.surface(),
    probe: None,
    linked_paths: BTreeMap::new(),
    incumbent: None,
  }
}

pub fn absolute_config(config: &RunnerConfig) -> AgentResult<RunnerConfig> {
  let absolute = |path: &Path| std::path::absolute(path).map_err(|error| AgentError::io(path, error));
  let mut resolved = config.clone();
  resolved.repo_root = absolute(&config.repo_root)?;
  resolved.eval_home = absolute(&config.eval_home)?;
  resolved.batch_dir = absolute(&config.batch_dir)?;
  resolved.tree_root = absolute(&config.tree_root)?;
  resolved.cache_dir = absolute(&config.cache_dir)?;
  if config.claude.binary.components().count() > 1 {
    resolved.claude.binary = absolute(&config.claude.binary)?;
  }
  Ok(resolved)
}

pub fn lock_batch(batch_dir: &Path) -> AgentResult<fs::File> {
  fs::create_dir_all(batch_dir).map_err(|error| AgentError::io(batch_dir, error))?;
  let path = batch_dir.join(BATCH_LOCK_FILE);
  let file = fs::File::create(&path).map_err(|error| AgentError::io(&path, error))?;
  let deadline = std::time::Instant::now() + BATCH_LOCK_GRACE;
  loop {
    match file.try_lock() {
      Ok(()) => return Ok(file),
      // A child forked by another thread holds every open descriptor until it execs, so a lock this process just released can look held for a moment.
      Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => thread::sleep(BATCH_LOCK_POLL),
      Err(fs::TryLockError::WouldBlock) => return Err(AgentError::BatchInProgress(batch_dir.to_owned())),
      Err(fs::TryLockError::Error(error)) => return Err(AgentError::io(&path, error)),
    }
  }
}

pub fn run_batch(config: &RunnerConfig, tasks: &[LoadedTask], planned: Vec<PlannedRun>) -> AgentResult<BatchManifest> {
  let config = &absolute_config(config)?;
  let _lock = lock_batch(&config.batch_dir)?;
  ensure_same_atlas_config(config, &planned)?;
  ensure_surface_preamble(config)?;
  ensure_same_surface(config, &planned)?;
  if let Some(atlas_config) = config.atlas_config.as_ref().filter(|_| !planned.is_empty()) {
    validate_atlas_config(config, atlas_config)?;
  }
  let hooks = expected_hooks(config, &planned)?;
  ensure_same_hooks(config, &planned, hooks.as_ref())?;
  let bash_guard = checked_bash_guard(config, &planned)?;
  let incumbent = planned_incumbent(config, &planned)?;
  ensure_same_incumbent(config, &planned, incumbent)?;
  let mut environment = environment(config);
  if planned.iter().any(|run| run.condition == Condition::Atlas) {
    environment.atlas_hooks = hooks.clone();
  } else {
    let recorded = recorded_environment(config)?;
    environment.atlas_hooks = recorded.as_ref().and_then(|recorded| recorded.atlas_hooks.clone());
    environment.atlas_surface = recorded.map_or(AtlasSurface::Standard, |recorded| recorded.atlas_surface);
  }
  let recorded = recorded_batch(config)?;
  environment.incumbent = match incumbent {
    Some(tools) => Some(warmed_incumbent(config, tools)?),
    None => recorded.as_ref().and_then(|recorded| recorded.environment.incumbent.clone()),
  };
  let incumbent_probe_cost = match incumbent {
    Some(tools) => incumbent_probe(config, tools)?.cost_usd.unwrap_or(0.0),
    None => 0.0,
  };
  if config.probe {
    environment.probe = Some(probe(config)?);
  }
  let mut hooked_probe_cost = 0.0;
  if hooks.is_some() || bash_guard.is_some() {
    if hooks.is_some()
      && let Some(probe) = environment.probe.as_ref().filter(|probe| probe.saw_hook(USER_PROMPT_SUBMIT))
    {
      return Err(AgentError::AtlasHooksUnavailable {
        reason: format!(
          "the isolation probe already reports a {USER_PROMPT_SUBMIT} hook without --settings ({}), for example from managed settings; the atlas seed hook could not be told apart from it, so the hooked probe could pass for the wrong reason; disable that hook for eval runs (or drop --atlas-hooks), then delete <batch>/_probe/probe.json or start a new --batch so the isolation probe runs again",
          probe.hooks.join(", ")
        ),
      });
    }
    let hooked = hooked_probe(config, hooks.as_ref(), bash_guard)?;
    hooked_probe_cost = hooked.cost_usd.unwrap_or(0.0);
    if let Some(probe) = environment.probe.as_mut() {
      probe.atlas_hooks_seen = Some(hooked.hooks_seen);
      probe.atlas_surface_seen = hooked.surface_seen;
    }
  }
  environment.linked_paths = tasks
    .iter()
    .filter_map(|task| Some((task.spec.id.clone(), task.spec.linked.as_ref()?.paths.clone())))
    .collect();
  let mut sources = BTreeMap::new();
  let mut bases = BTreeMap::new();
  for task in tasks {
    let source = resolve_source(task, &config.repo_root, config.repo_override.as_deref(), &config.cache_dir)?;
    let strip = StripPlan::for_task(task, config.repo_override.is_some());
    bases.insert(task.spec.id.clone(), base_check(config, task, &source, &strip)?);
    sources.insert(task.spec.id.clone(), (source, strip));
  }
  let initial_spent = environment.probe.as_ref().and_then(|probe| probe.cost_usd).unwrap_or(0.0) + hooked_probe_cost + incumbent_probe_cost;
  let queue = Mutex::new(planned.iter().cloned().collect::<VecDeque<_>>());
  let spent = Mutex::new(initial_spent);
  let skipped = Mutex::new(Vec::new());
  let hooks_failed = AtomicBool::new(false);
  let skipped_after_hooks = Mutex::new(Vec::new());
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
          if run.condition == Condition::Atlas && hooks_failed.load(Ordering::SeqCst) {
            tracing::warn!(run = run.run_id, "an earlier atlas session of this batch failed its hooks; not starting run");
            skipped_after_hooks.lock().unwrap_or_else(PoisonError::into_inner).push(run.run_id.clone());
            continue;
          }
          let (Some(task), Some((source, strip))) = (tasks.iter().find(|task| task.spec.id == run.task_id), sources.get(&run.task_id)) else {
            continue;
          };
          let context = RunContext {
            config,
            hooks: hooks.as_ref(),
            task,
            source,
            strip,
            base: bases.get(&run.task_id).and_then(Option::as_ref),
            planned: &run,
            dir: config.batch_dir.join(&run.run_id),
          };
          let reused = context.dir.join(SCORE_FILE).is_file();
          let cost = execute_run(&context).unwrap_or_else(|error| {
            tracing::error!(run = run.run_id, %error, "run failed before it was scored; it will be retried on resume");
            read_json::<RunManifest>(&context.dir.join(RUN_FILE)).map_or(0.0, |manifest| manifest.cost_usd)
          });
          *spent.lock().unwrap_or_else(PoisonError::into_inner) += cost;
          if !reused && failed_hooks(&context.dir) {
            hooks_failed.store(true, Ordering::SeqCst);
          }
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
    skipped_after_hooks_failure: skipped_after_hooks.into_inner().unwrap_or_else(PoisonError::into_inner),
    spent_usd: spent.into_inner().unwrap_or_else(PoisonError::into_inner),
    instruction_sections_dropped: tasks
      .iter()
      .filter(|task| !task.spec.drop_instruction_sections.is_empty())
      .map(|task| (task.spec.id.clone(), task.spec.drop_instruction_sections.clone()))
      .collect(),
    held_out: held_out(tasks, recorded.as_ref()),
  };
  write_json(&config.batch_dir.join(BATCH_FILE), &manifest)?;
  Ok(manifest)
}

fn planned_incumbent<'a>(config: &'a RunnerConfig, planned: &[PlannedRun]) -> AgentResult<Option<&'a IncumbentTools>> {
  if !planned.iter().any(|run| run.condition == Condition::Incumbent) {
    return Ok(None);
  }
  config.incumbent.as_ref().map(Some).ok_or_else(|| AgentError::IncumbentUnavailable {
    reason: "incumbent runs are planned but no Serena and graphify binaries were resolved".to_owned(),
  })
}

fn held_out(tasks: &[LoadedTask], recorded: Option<&BatchManifest>) -> Vec<String> {
  let declared = tasks
    .iter()
    .filter(|task| task.spec.category.iter().any(|category| category == HELD_OUT_CATEGORY))
    .map(|task| task.spec.id.clone());
  let earlier = recorded.map(|recorded| recorded.held_out.clone()).unwrap_or_default();
  let union: BTreeSet<String> = declared.chain(earlier).collect();
  union.into_iter().collect()
}

fn recorded_batch(config: &RunnerConfig) -> AgentResult<Option<BatchManifest>> {
  let path = config.batch_dir.join(BATCH_FILE);
  path.is_file().then(|| read_json::<BatchManifest>(&path)).transpose()
}

fn ensure_same_incumbent(config: &RunnerConfig, planned: &[PlannedRun], tools: Option<&IncumbentTools>) -> AgentResult<()> {
  let Some(tools) = tools else {
    return Ok(());
  };
  let expected = tools.setup();
  let mut differing = Vec::new();
  let mut recorded = BTreeSet::new();
  for run in planned.iter().filter(|run| run.condition == Condition::Incumbent) {
    let dir = config.batch_dir.join(&run.run_id);
    if !dir.join(SCORE_FILE).is_file() {
      continue;
    }
    let manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    let setup = manifest.incumbent.as_ref().map_or_else(|| "none".to_owned(), IncumbentRun::setup);
    if setup != expected {
      recorded.insert(setup);
      differing.push(run.run_id.clone());
    }
  }
  if differing.is_empty() {
    return Ok(());
  }
  Err(AgentError::IncumbentMismatch {
    runs: differing.join(", "),
    recorded: recorded.into_iter().collect::<Vec<_>>().join("; "),
    expected,
  })
}

fn incumbent_probe(config: &RunnerConfig, tools: &IncumbentTools) -> AgentResult<IncumbentProbe> {
  if !config.probe {
    return Err(AgentError::IncumbentUnavailable {
      reason: "a probe session must show that Claude Code runs the Serena hooks under the eval flags before any incumbent session; drop --no-probe".to_owned(),
    });
  }
  let out_dir = config.batch_dir.join(PROBE_DIR).join(INCUMBENT_PROBE_DIR);
  if let Ok(probe) = read_json::<IncumbentProbe>(&out_dir.join(INCUMBENT_PROBE_FILE))
    && probe.tools_blake3 == tools.probe_key()
    && probe.problem(&out_dir).is_none()
  {
    return Ok(probe);
  }
  remove_dir(&out_dir)?;
  fs::create_dir_all(&out_dir).map_err(|error| AgentError::io(&out_dir, error))?;
  let scratch = isolated_scratch(config, &format!("{PROBE_DIR}/{INCUMBENT_PROBE_DIR}"))?;
  let probe = incumbent_session_env(tools, &scratch).and_then(|tree_env| {
    run_incumbent_probe(&IncumbentProbeRequest {
      tools,
      claude: &config.claude,
      out_dir: &out_dir,
      scratch: &scratch,
      tree_env,
    })
  });
  release_scratch(config, &scratch);
  let probe = probe?;
  match probe.problem(&out_dir) {
    Some(reason) => Err(AgentError::IncumbentUnavailable { reason }),
    None => Ok(probe),
  }
}

fn incumbent_session_env(tools: &IncumbentTools, scratch: &Path) -> AgentResult<Vec<(String, String)>> {
  let mut env = tree_env(scratch);
  if let Some(path) = tools.session_path(scratch, std::env::var_os("PATH").as_deref())? {
    env.push(("PATH".to_owned(), path));
  }
  Ok(env)
}

fn warmed_incumbent(config: &RunnerConfig, tools: &IncumbentTools) -> AgentResult<IncumbentRecord> {
  let scratch = isolated_scratch(config, INCUMBENT_WARM_UP)?;
  let warmed = tools.warm_up(&scratch);
  release_scratch(config, &scratch);
  warmed?;
  Ok(tools.record())
}

fn validate_atlas_config(config: &RunnerConfig, atlas_config: &AtlasConfig) -> AgentResult<()> {
  fs::create_dir_all(&config.tree_root).map_err(|error| AgentError::io(&config.tree_root, error))?;
  let probe = tempfile::Builder::new()
    .prefix("ae-config-probe-")
    .tempdir_in(&config.tree_root)
    .map_err(|error| AgentError::io(&config.tree_root, error))?;
  check_isolated_location(probe.path(), &[&config.repo_root, &config.eval_home])?;
  atlas_config.validate(&config.atlas_bin, probe.path(), config.atlas_timeout)
}

fn ensure_same_atlas_config(config: &RunnerConfig, planned: &[PlannedRun]) -> AgentResult<()> {
  let expected = config.atlas_config.as_ref().map(AtlasConfig::digest);
  let mut differing = Vec::new();
  let mut recorded = BTreeSet::new();
  for run in planned {
    let dir = config.batch_dir.join(&run.run_id);
    if !dir.join(SCORE_FILE).is_file() {
      continue;
    }
    let manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    if manifest.atlas_config_blake3 != expected {
      recorded.insert(manifest.atlas_config_blake3.unwrap_or_else(|| "none".to_owned()));
      differing.push(run.run_id.clone());
    }
  }
  if differing.is_empty() {
    return Ok(());
  }
  Err(AgentError::AtlasConfigMismatch {
    runs: differing.join(", "),
    recorded: recorded.into_iter().collect::<Vec<_>>().join(", "),
    expected: expected.unwrap_or_else(|| "none".to_owned()),
  })
}

fn expected_hooks(config: &RunnerConfig, planned: &[PlannedRun]) -> AgentResult<Option<AtlasHooksRecord>> {
  if !config.atlas_hooks || !planned.iter().any(|run| run.condition == Condition::Atlas) {
    return Ok(None);
  }
  if !config.probe {
    return Err(AgentError::AtlasHooksUnavailable {
      reason: "a hooked probe session must confirm that Claude Code runs the hooks under the eval flags before any atlas session; drop --no-probe".to_owned(),
    });
  }
  fs::create_dir_all(&config.tree_root).map_err(|error| AgentError::io(&config.tree_root, error))?;
  let probe = tempfile::Builder::new()
    .prefix("ae-hooks-probe-")
    .tempdir_in(&config.tree_root)
    .map_err(|error| AgentError::io(&config.tree_root, error))?;
  check_isolated_location(probe.path(), &[&config.repo_root, &config.eval_home])?;
  let home = probe.path().join("home");
  fs::create_dir_all(&home).map_err(|error| AgentError::io(&home, error))?;
  if let Some(atlas_config) = &config.atlas_config {
    atlas_config.apply(&home)?;
  }
  let atlas = AtlasCli::new(&config.atlas_bin, &home, config.atlas_timeout).sandboxed(probe.path().join(ATLAS_HOOKS_USER));
  Ok(Some(AtlasHooks::render(&atlas, probe.path())?.record().clone()))
}

fn recorded_environment(config: &RunnerConfig) -> AgentResult<Option<Environment>> {
  let path = config.batch_dir.join(BATCH_FILE);
  if !path.is_file() {
    return Ok(None);
  }
  Ok(Some(read_json::<BatchManifest>(&path)?.environment))
}

fn ensure_surface_preamble(config: &RunnerConfig) -> AgentResult<()> {
  let surface = config.atlas_surface.surface();
  if (surface == AtlasSurface::Forced) == (config.preamble == PreambleVariant::Forced) {
    return Ok(());
  }
  Err(AgentError::InvalidArgument(format!(
    "atlas surface {surface} runs with the {} preamble, not {}",
    surface.preamble(PreambleVariant::Standard),
    config.preamble
  )))
}

fn ensure_same_surface(config: &RunnerConfig, planned: &[PlannedRun]) -> AgentResult<()> {
  let expected = config.atlas_surface.surface();
  let mut differing = Vec::new();
  let mut recorded = BTreeSet::new();
  for run in planned.iter().filter(|run| run.condition == Condition::Atlas) {
    let dir = config.batch_dir.join(&run.run_id);
    if !dir.join(SCORE_FILE).is_file() {
      continue;
    }
    let manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    if manifest.atlas_surface != expected {
      recorded.insert(manifest.atlas_surface);
      differing.push(run.run_id.clone());
    }
  }
  if differing.is_empty() {
    return Ok(());
  }
  Err(AgentError::AtlasSurfaceMismatch {
    runs: differing.join(", "),
    recorded: recorded.iter().map(|surface| surface.as_str()).collect::<Vec<_>>().join(", "),
    expected: expected.to_string(),
  })
}

fn checked_bash_guard<'a>(config: &'a RunnerConfig, planned: &[PlannedRun]) -> AgentResult<Option<&'a Path>> {
  let Some(bash_guard) = config.atlas_surface.bash_guard() else {
    return Ok(None);
  };
  if !planned.iter().any(|run| run.condition == Condition::Atlas) {
    return Ok(None);
  }
  if !config.probe {
    return Err(AgentError::AtlasSurfaceUnavailable {
      reason: "a probe session must confirm the tool surface and that Claude Code honours the Bash guard before any atlas session; drop --no-probe".to_owned(),
    });
  }
  check_bash_guard(bash_guard)?;
  Ok(Some(bash_guard))
}

fn failed_hooks(run_dir: &Path) -> bool {
  read_json::<RunManifest>(&run_dir.join(RUN_FILE)).is_ok_and(|manifest| matches!(&manifest.status, RunStatus::SetupFailed { stage, .. } if stage == ATLAS_HOOKS_STAGE))
}

fn hooked_probe(config: &RunnerConfig, expected: Option<&AtlasHooksRecord>, bash_guard: Option<&Path>) -> AgentResult<HookedProbe> {
  let surface = probed_surface(bash_guard);
  let out_dir = config.batch_dir.join(PROBE_DIR).join(HOOKED_PROBE_DIR);
  let recorded = out_dir.join("probe.json");
  if let Ok(probe) = read_json::<HookedProbe>(&recorded)
    && probe.atlas_hooks_blake3.as_deref() == expected.map(|expected| expected.blake3.as_str())
    && probe.atlas_surface == surface
    && hooked_probe_problem(&probe, expected.is_some(), &out_dir).is_none()
  {
    return Ok(probe);
  }
  if let Some(expected) = expected.filter(|expected| !expected.installs(USER_PROMPT_SUBMIT)) {
    return Err(AgentError::AtlasHooksUnavailable {
      reason: format!(
        "the hooks {} have no {USER_PROMPT_SUBMIT} group, which the probe and every session need to prove the hooks ran; enable `seed` in [agents.claude] hooks",
        expected.describe()
      ),
    });
  }
  remove_dir(&out_dir)?;
  fs::create_dir_all(&out_dir).map_err(|error| AgentError::io(&out_dir, error))?;
  let scratch = isolated_scratch(config, &format!("{PROBE_DIR}/{HOOKED_PROBE_DIR}"))?;
  let probe = run_hooked_probe_in(config, expected, bash_guard, &out_dir, &scratch);
  release_scratch(config, &scratch);
  let probe = probe?;
  match hooked_probe_problem(&probe, expected.is_some(), &out_dir) {
    Some(error) => Err(error),
    None => Ok(probe),
  }
}

fn probed_surface(bash_guard: Option<&Path>) -> AtlasSurface {
  match bash_guard {
    Some(_) => AtlasSurface::Forced,
    None => AtlasSurface::Standard,
  }
}

fn hooked_probe_problem(probe: &HookedProbe, hooks: bool, out_dir: &Path) -> Option<AgentError> {
  let hooks_seen = !hooks || probe.user_prompt_submit_seen;
  let surface_problem = match (probe.atlas_surface, &probe.surface_seen) {
    (AtlasSurface::Standard, _) => None,
    (AtlasSurface::Forced, None) => Some("the forced probe recorded no tool surface".to_owned()),
    (AtlasSurface::Forced, Some(seen)) => seen.problem(),
  };
  if hooks_seen && surface_problem.is_none() {
    return None;
  }
  let logs = format!("see {} and {}", out_dir.join("claude.stderr.log").display(), out_dir.join(TRANSCRIPT_FILE).display());
  let unavailable = |reason: String| {
    if hooks_seen {
      AgentError::AtlasSurfaceUnavailable { reason }
    } else {
      AgentError::AtlasHooksUnavailable { reason }
    }
  };
  let shown = if hooks_seen { "the forced tool surface" } else { "whether the hooks run" };
  if !probe.started {
    return Some(unavailable(format!(
      "the hooked probe session did not start (no init event, exit {:?}), so it could not show {shown}; {logs}",
      probe.exit
    )));
  }
  if !probe.exit.succeeded() && probe.model_turns == 0 {
    return Some(unavailable(format!(
      "the hooked probe session failed before its first model request (exit {:?}); {logs}",
      probe.exit
    )));
  }
  if !hooks_seen {
    return Some(unavailable(format!(
      "the hooked probe session started and reached the model but reported no {USER_PROMPT_SUBMIT} hook event, so Claude Code does not run --settings hooks under the eval flags (exit {:?}); {logs}",
      probe.exit
    )));
  }
  surface_problem.map(|problem| unavailable(format!("{problem}; {logs}")))
}

fn run_hooked_probe_in(config: &RunnerConfig, expected: Option<&AtlasHooksRecord>, bash_guard: Option<&Path>, out_dir: &Path, scratch: &Path) -> AgentResult<HookedProbe> {
  let home = isolated_atlas_home(config, scratch, ATLAS_HOME)?;
  let atlas = AtlasCli::new(&config.atlas_bin, &home, config.atlas_timeout);
  let hooks = match expected {
    None => None,
    Some(expected) => {
      let hooks = AtlasHooks::render(&atlas.clone().sandboxed(scratch.join(ATLAS_HOOKS_USER)), scratch)?;
      if hooks.record() != expected {
        return Err(AgentError::AtlasHooksUnavailable {
          reason: format!("the probe home renders hooks {} but the batch installs {}", hooks.record().describe(), expected.describe()),
        });
      }
      Some(hooks)
    }
  };
  let surface = probed_surface(bash_guard);
  let mcp_config = match surface {
    AtlasSurface::Standard => EMPTY_MCP_CONFIG.to_owned(),
    AtlasSurface::Forced => forced_probe_project(config, &atlas, scratch)?,
  };
  let settings = session_settings(hooks.as_ref(), bash_guard)?.map(|settings| settings.to_string()).unwrap_or_default();
  let request = HookedProbeRequest {
    out_dir,
    scratch,
    settings: &settings,
    mcp_config: &mcp_config,
    atlas_hooks_blake3: expected.map(|expected| expected.blake3.as_str()),
    surface,
    tree_env: tree_env(scratch),
  };
  run_hooked_probe(&request, &config.claude)
}

fn forced_probe_project(config: &RunnerConfig, atlas: &AtlasCli, scratch: &Path) -> AgentResult<String> {
  let tree = scratch.join("tree");
  let (name, content) = FORCED_PROBE_SOURCE;
  fs::create_dir_all(&tree).map_err(|error| AgentError::io(&tree, error))?;
  fs::write(tree.join(name), content).map_err(|error| AgentError::io(tree.join(name), error))?;
  let project = project_name(&config.batch_dir, FORCED_PROBE_PROJECT);
  atlas.project_create(&project, std::slice::from_ref(&tree), &tree)?;
  atlas.index(&project, &tree)?;
  Ok(atlas.mcp_config(&project, AtlasSurface::Forced.mcp_tool_profile()).to_string())
}

fn ensure_same_hooks(config: &RunnerConfig, planned: &[PlannedRun], expected: Option<&AtlasHooksRecord>) -> AgentResult<()> {
  let digest = |record: Option<&AtlasHooksRecord>| record.map_or_else(|| "none".to_owned(), |record| record.blake3.clone());
  let mut differing = Vec::new();
  let mut recorded = BTreeSet::new();
  for run in planned.iter().filter(|run| run.condition == Condition::Atlas) {
    let dir = config.batch_dir.join(&run.run_id);
    if !dir.join(SCORE_FILE).is_file() {
      continue;
    }
    let manifest: RunManifest = read_json(&dir.join(RUN_FILE))?;
    if digest(manifest.atlas_hooks.as_ref()) != digest(expected) {
      recorded.insert(digest(manifest.atlas_hooks.as_ref()));
      differing.push(run.run_id.clone());
    }
  }
  if differing.is_empty() {
    return Ok(());
  }
  Err(AgentError::AtlasHooksMismatch {
    runs: differing.join(", "),
    recorded: recorded.into_iter().collect::<Vec<_>>().join(", "),
    expected: digest(expected),
  })
}

fn install_settings(context: &RunContext<'_>, atlas: &AtlasCli, scratch: &Path, manifest: &mut RunManifest) -> AgentResult<()> {
  let hooks = match context.hooks {
    None => None,
    Some(expected) => {
      let session_atlas = AtlasCli::new(atlas.binary(), atlas.home(), context.config.atlas_timeout).sandboxed(scratch.join(ATLAS_HOOKS_USER));
      let hooks = AtlasHooks::render(&session_atlas, scratch)?;
      if hooks.record() != expected {
        return Err(AgentError::AtlasHooksUnavailable {
          reason: format!(
            "the session home renders hooks {} but the batch installs {}",
            hooks.record().describe(),
            expected.describe()
          ),
        });
      }
      Some(hooks)
    }
  };
  let Some(settings) = session_settings(hooks.as_ref(), context.config.atlas_surface.bash_guard())? else {
    return Ok(());
  };
  let settings = settings.to_string();
  for path in [scratch.join(ATLAS_HOOKS_FILE), context.dir.join(ATLAS_HOOKS_FILE)] {
    fs::write(&path, &settings).map_err(|error| AgentError::io(&path, error))?;
  }
  manifest.atlas_hooks = hooks.map(|hooks| hooks.record().clone());
  Ok(())
}

fn probe(config: &RunnerConfig) -> AgentResult<ProbeReport> {
  let dir = config.batch_dir.join(PROBE_DIR);
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
  if let Some(atlas_config) = &config.atlas_config {
    atlas_config.apply(&home)?;
  }
  Ok(home)
}

fn preserve_atlas_artifacts(config: &RunnerConfig, scratch: &Path, run_dir: &Path) {
  let seeded = config.atlas_config.as_ref().map(AtlasConfig::seeded_paths).unwrap_or_default();
  for home in [ATLAS_HOME, ATLAS_POST_HOME] {
    let source = scratch.join(home);
    if !source.is_dir() {
      continue;
    }
    if let Err(error) = copy_small_artifacts(&source, &source, &run_dir.join(home), &seeded) {
      tracing::warn!(home, %error, "cannot preserve Code Atlas artifacts");
    }
  }
}

fn copy_small_artifacts(root: &Path, from: &Path, to: &Path, skip: &BTreeSet<PathBuf>) -> std::io::Result<()> {
  fs::create_dir_all(to)?;
  for entry in fs::read_dir(from)? {
    let entry = entry?;
    let path = entry.path();
    let name = entry.file_name().to_string_lossy().into_owned();
    if entry.file_type()?.is_dir() {
      copy_small_artifacts(root, &path, &to.join(&name), skip)?;
    } else if path.strip_prefix(root).is_ok_and(|relative| skip.contains(relative)) {
      continue;
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
  let env = tree_env(&scratch);
  build_base_tree(task, source, strip, &scratch, &env, config.validation_timeout).map_err(|(_, error)| error)?;
  if let Some(linked) = LinkedLayout::of(task, &scratch) {
    let restored = linked.restore(&tree)?;
    if !restored.missing.is_empty() {
      return Err(AgentError::InvalidArgument(format!(
        "the held linked paths {:?} vanished before the base check restored them",
        restored.missing
      )));
    }
  }
  copy_hidden(task, &tree)?;
  let check = BaseCheck {
    task_id: task.spec.id.clone(),
    validation: run_suite(&validation.commands, &tree, &env, config.validation_timeout),
    regression: run_suite(&validation.regression, &tree, &env, config.validation_timeout),
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
  let is_atlas = context.planned.condition == Condition::Atlas;
  let surface = context.config.atlas_surface.surface();
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
    preamble_version: is_atlas.then(|| context.config.preamble.version()),
    preamble_variant: is_atlas.then_some(context.config.preamble),
    atlas_config_blake3: context.config.atlas_config.as_ref().map(AtlasConfig::digest),
    atlas_hooks: None,
    atlas_surface: if is_atlas { surface } else { AtlasSurface::Standard },
    incumbent: None,
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
        preserve_atlas_artifacts(context.config, scratch, dir);
        release_scratch(context.config, scratch);
      }
      return Ok(0.0);
    }
  };
  let tree = scratch.join("tree");
  let scored = score_session(context, &scratch, &tree, &base_sha, &mut manifest);
  preserve_atlas_artifacts(context.config, &scratch, dir);
  release_scratch(context.config, &scratch);
  scored
}

fn score_session(context: &RunContext<'_>, scratch: &Path, tree: &Path, base_sha: &str, manifest: &mut RunManifest) -> AgentResult<f64> {
  let dir = &context.dir;
  let linked = LinkedLayout::of(context.task, scratch);
  let held = linked.as_ref().map(LinkedLayout::snapshot);
  let session = run_session(context, scratch, tree, manifest)?;
  let linked_writes = match (&linked, &held) {
    (Some(linked), Some(held)) => held.changes(&linked.snapshot()),
    _ => Vec::new(),
  };
  let accesses = out_of_tree_accesses(&session, &access_policy(context, scratch, tree, &session));
  let excluded = linked.as_ref().map(|linked| linked.spec.paths.clone()).unwrap_or_default();
  let diff_text = diff_against(tree, base_sha, &excluded)?;
  fs::write(dir.join(DIFF_FILE), &diff_text).map_err(|error| AgentError::io(dir.join(DIFF_FILE), error))?;
  let diff = Diff::parse(&diff_text);
  let duplication = duplication_check(context, scratch, tree, base_sha, &diff);
  let roots = linked.as_ref().map(linked_roots).unwrap_or_default();
  let restored = match &linked {
    Some(linked) => linked.restore(tree)?,
    None => Restored::default(),
  };
  if !restored.conflicts.is_empty() {
    tracing::warn!(run = context.planned.run_id, paths = ?restored.conflicts, "the session created files at linked paths; moved them aside before restoring the linked code");
  }
  let spec = &context.task.spec;
  let env = tree_env(scratch);
  let (validation, regression) = match restored.missing.is_empty() {
    true => {
      copy_hidden(context.task, tree)?;
      (
        run_suite(&spec.validation.commands, tree, &env, context.config.validation_timeout),
        run_suite(&spec.validation.regression, tree, &env, context.config.validation_timeout),
      )
    }
    false => {
      tracing::warn!(run = context.planned.run_id, paths = ?restored.missing, "the session moved held linked paths away; validation does not run");
      (SuiteResult::default(), SuiteResult::default())
    }
  };
  let changed = diff.changed_files();
  let mut metrics = compute_metrics(&MetricsInput {
    transcript: &session,
    tree,
    view: linked.as_ref().map(|linked| linked.view.as_path()),
    ground_truth: &spec.ground_truth,
    changed_files: &changed,
    wall_ms: manifest.wall_ms,
  });
  metrics.out_of_tree_accesses = accesses;
  metrics.incumbent = (context.planned.condition == Condition::Incumbent).then(|| incumbent_metrics(&session));
  metrics.contaminated = contaminated(&metrics.out_of_tree_accesses);
  if manifest.atlas_surface == AtlasSurface::Forced {
    metrics.atlas_surface_denials = Some(bash_guard_denials(&session));
  }
  if let Some(linked) = &linked {
    let linked = linked_metrics(
      &session,
      &LinkedInput {
        roots: &roots,
        spec: linked.spec,
        path_created: !restored.conflicts.is_empty(),
        writes: linked_writes,
        restore: match restored.missing.is_empty() {
          true => LinkedRestore::Restored,
          false => LinkedRestore::HoldMissing { paths: restored.missing },
        },
      },
    );
    metrics.contaminated |= linked.contaminates();
    metrics.linked = Some(linked);
  }
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

fn source_roots(task: &LoadedTask, source: &TreeSource, cache_dir: &Path) -> Vec<PathBuf> {
  let archived_from = match source {
    TreeSource::GitArchive { repo, .. } => Some(repo.canonicalize().unwrap_or_else(|_| repo.clone())),
    TreeSource::Directory { path } => Some(path.canonicalize().unwrap_or_else(|_| path.clone())),
  };
  local_source_path(&task.spec.repo, cache_dir).into_iter().chain(archived_from).collect()
}

fn access_policy(context: &RunContext<'_>, scratch: &Path, tree: &Path, session: &Transcript) -> AccessPolicy {
  let config = context.config;
  let mut trees = vec![tree.to_owned(), scratch.join(CARGO_TARGET)];
  trees.extend(tree.canonicalize().ok());
  trees.extend(session.init.as_ref().and_then(|init| init.cwd.as_deref()).map(PathBuf::from));
  AccessPolicy {
    trees,
    home: std::env::var_os("HOME").map(PathBuf::from),
    repo_root: config.repo_root.canonicalize().unwrap_or_else(|_| config.repo_root.clone()),
    eval_home: config.eval_home.canonicalize().unwrap_or_else(|_| config.eval_home.clone()),
    scratch_root: config.tree_root.canonicalize().unwrap_or_else(|_| config.tree_root.clone()),
    own_scratch: scratch.canonicalize().unwrap_or_else(|_| scratch.to_owned()),
    source_roots: source_roots(context.task, context.source, &config.cache_dir),
    linked: LinkedLayout::of(context.task, scratch).map(|linked| LinkedPolicy {
      hold: with_canonical(&linked.hold),
      view: with_canonical(&linked.view),
      view_reads: match context.planned.condition {
        Condition::Atlas | Condition::Incumbent => LinkedReads::Allowed,
        Condition::Baseline => LinkedReads::Contaminating,
      },
    }),
  }
}

fn linked_roots(linked: &LinkedLayout<'_>) -> Vec<PathBuf> {
  [&linked.hold, &linked.view].into_iter().flat_map(|root| with_canonical(root)).collect()
}

fn with_canonical(path: &Path) -> Vec<PathBuf> {
  let mut paths = vec![path.to_owned()];
  paths.extend(path.canonicalize().ok().filter(|canonical| canonical != path));
  paths
}

fn prepare_tree(context: &RunContext<'_>, scratch: &Path, manifest: &mut RunManifest) -> Result<String, (&'static str, AgentError)> {
  let tree = &scratch.join("tree");
  manifest.tree = Some(tree.display().to_string());
  let base_sha = build_base_tree(context.task, context.source, context.strip, scratch, &tree_env(scratch), context.config.validation_timeout)?;
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
      if let Some(linked) = LinkedLayout::of(context.task, scratch) {
        linked
          .materialize_view(context.task, context.source, context.strip)
          .map_err(|error| ("linked-view", error))?;
        let filters = RepoGlobs {
          include: linked.spec.include_globs(),
          exclude: linked.view_excludes().map_err(|error| ("linked-view", error))?,
        };
        atlas
          .add_repo(&project, &linked.view, &linked.spec.repo, &filters, tree)
          .map_err(|error| ("atlas-project", error))?;
      }
      let index = atlas.index(&project, tree).map_err(|error| ("atlas-index", error))?;
      let log = context.dir.join(ATLAS_INDEX_LOG);
      fs::write(&log, format!("{}\n--- stderr ---\n{}", index.stdout, index.stderr)).map_err(|error| ("atlas-index", AgentError::io(&log, error)))?;
      manifest.atlas_index_ms = Some(millis(index.duration));
      manifest.atlas_project = Some(project.clone());
      install_settings(context, &atlas, scratch, manifest).map_err(|error| (ATLAS_HOOKS_STAGE, error))?;
      atlas.mcp_config(&project, context.config.atlas_surface.surface().mcp_tool_profile()).to_string()
    }
    Condition::Incumbent => prepare_incumbent(context, scratch, tree, manifest)?,
  };
  for path in [mcp_path, context.dir.join("mcp.json")] {
    fs::write(&path, &mcp_config).map_err(|error| ("mcp-config", AgentError::io(&path, error)))?;
  }
  Ok(base_sha)
}

fn prepare_incumbent(context: &RunContext<'_>, scratch: &Path, tree: &Path, manifest: &mut RunManifest) -> Result<String, (&'static str, AgentError)> {
  let config = context.config;
  let tools = config.incumbent.as_ref().ok_or_else(|| {
    (
      INCUMBENT_SERENA_STAGE,
      AgentError::IncumbentUnavailable {
        reason: "no Serena and graphify binaries were resolved for this batch".to_owned(),
      },
    )
  })?;
  if let Some(linked) = LinkedLayout::of(context.task, scratch) {
    linked
      .materialize_view(context.task, context.source, context.strip)
      .map_err(|error| ("linked-view", error))?;
  }
  let graphify = Graphify {
    binary: tools.graphify.clone(),
    timeout: tools.timeout,
  };
  let work = scratch.join(GRAPH_BUILD_DIR);
  let graphs = ensure_graphs(&GraphBuild {
    graphify: &graphify,
    graphify_version: tools.graphify_version.as_deref(),
    task: context.task,
    source: context.source,
    strip: context.strip,
    cache_dir: &config.cache_dir,
    work: &work,
  });
  remove_dir(&work).map_err(|error| (INCUMBENT_GRAPHIFY_STAGE, error))?;
  let graphs = graphs.map_err(|error| (INCUMBENT_GRAPHIFY_STAGE, error))?;
  install_graphs(&graphs, tree).map_err(|error| (INCUMBENT_GRAPHIFY_STAGE, error))?;
  let excluded = exclude_tool_dirs(tree, &graphs.set.roots).map_err(|error| (INCUMBENT_GRAPHIFY_STAGE, error))?;
  let home = tools.serena_home(scratch).map_err(|error| (INCUMBENT_SERENA_STAGE, error))?;
  let settings = tools.hook_settings(&home).to_string();
  for path in [scratch.join(INCUMBENT_HOOKS_FILE), context.dir.join(INCUMBENT_HOOKS_FILE)] {
    fs::write(&path, &settings).map_err(|error| (INCUMBENT_SERENA_STAGE, AgentError::io(&path, error)))?;
  }
  manifest.incumbent = Some(IncumbentRun {
    serena_version: tools.serena_version.clone(),
    graphify_version: tools.graphify_version.clone(),
    graph_key: graphs.set.key.clone(),
    graph_roots: graphs.set.roots.clone(),
    graph_build_ms: if graphs.built_now { graphs.set.build_ms } else { 0 },
    graph_nodes: graphs.set.nodes,
    graph_edges: graphs.set.edges,
    excluded,
    instructions_blake3: incumbent_instructions_digest(),
  });
  Ok(tools.mcp_config(&home).to_string())
}

fn tree_env(scratch: &Path) -> Vec<(String, String)> {
  vec![("CARGO_TARGET_DIR".to_owned(), scratch.join(CARGO_TARGET).display().to_string())]
}

fn build_base_tree(
  task: &LoadedTask,
  source: &TreeSource,
  strip: &StripPlan,
  scratch: &Path,
  env: &[(String, String)],
  timeout: Duration,
) -> Result<String, (&'static str, AgentError)> {
  let tree = &scratch.join("tree");
  materialize_task(task, source, tree, strip).map_err(|error| ("materialize", error))?;
  apply_linked_patch(task, tree).map_err(|error| ("materialize", error))?;
  run_setup(&task.spec.setup, tree, env, timeout).map_err(|error| ("setup", error))?;
  if let Some(linked) = LinkedLayout::of(task, scratch) {
    linked.hide(tree).map_err(|error| ("linked-hide", error))?;
  }
  init_base(tree).map_err(|error| ("git-base", error))
}

fn run_setup(commands: &[String], tree: &Path, env: &[(String, String)], timeout: Duration) -> AgentResult<()> {
  for command in commands {
    let result = run_command(command, tree, env, timeout);
    if !result.passed() {
      return Err(AgentError::Command {
        command: result.command,
        detail: format!("{:?}: {}", result.exit, result.output_tail),
      });
    }
  }
  Ok(())
}

pub fn atlas_repos(task: &LoadedTask, tree: &Path) -> Vec<PathBuf> {
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
  let condition = context.planned.condition;
  let incumbent = (condition == Condition::Incumbent).then(|| {
    let linked = LinkedLayout::of(context.task, scratch);
    incumbent_instructions(linked.as_ref().map(|linked| (linked.spec.repo.as_str(), linked.view.as_path())))
  });
  let system_prompt = match (instructions.map(|(text, _)| text), incumbent) {
    (Some(project), Some(incumbent)) => Some(format!("{project}\n{incumbent}")),
    (project, incumbent) => project.or(incumbent),
  };
  let session_env = match context.config.incumbent.as_ref().filter(|_| condition == Condition::Incumbent) {
    Some(tools) => incumbent_session_env(tools, scratch)?,
    None => tree_env(scratch),
  };
  let request = SessionRequest {
    prompt: condition.prompt(context.config.preamble, &spec.prompt),
    tree: tree.to_owned(),
    mcp_config: scratch.join("mcp.json"),
    max_turns: spec.max_turns,
    tools: SessionTools::of(condition, manifest.atlas_surface),
    config_dir: scratch.join("claude-config"),
    project_instructions: system_prompt,
    tree_env: session_env,
    hooks: session_hooks(condition, manifest, scratch),
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
  let problem = mcp_server_problem(condition, &transcript)
    .or_else(|| hooks_problem(manifest.atlas_hooks.as_ref(), &transcript).map(|reason| (ATLAS_HOOKS_STAGE, reason)))
    .or_else(|| {
      (condition == Condition::Incumbent)
        .then(|| incumbent_hooks_problem(&transcript))
        .flatten()
        .map(|reason| (INCUMBENT_HOOKS_STAGE, reason))
    });
  manifest.status = match problem {
    Some((stage, reason)) => RunStatus::SetupFailed { stage: stage.to_owned(), reason },
    None => RunStatus::Completed {
      session: session_outcome(&transcript, &output.exit),
    },
  };
  write_json(&dir.join(RUN_FILE), manifest)?;
  Ok(transcript)
}

fn session_hooks(condition: Condition, manifest: &RunManifest, scratch: &Path) -> HookWiring {
  if condition == Condition::Incumbent {
    return HookWiring::Installed {
      settings: scratch.join(INCUMBENT_HOOKS_FILE),
    };
  }
  if manifest.atlas_hooks.is_some() || manifest.atlas_surface == AtlasSurface::Forced {
    return HookWiring::Installed {
      settings: scratch.join(ATLAS_HOOKS_FILE),
    };
  }
  HookWiring::Unchanged
}

fn mcp_server_problem(condition: Condition, transcript: &Transcript) -> Option<(&'static str, String)> {
  let (stage, name) = match condition {
    Condition::Baseline => return None,
    Condition::Atlas => ("atlas-mcp", ATLAS_SERVER_NAME),
    Condition::Incumbent => ("incumbent-mcp", SERENA_SERVER_NAME),
  };
  let servers = transcript.init.as_ref().map(|init| init.mcp_servers.as_slice()).unwrap_or_default();
  let reason = match servers.iter().find(|server| server.name == name) {
    Some(server) if server.status == "connected" => return None,
    Some(server) => format!("the {name} MCP server reported status `{}`", server.status),
    None => format!("the session did not list the {name} MCP server"),
  };
  Some((stage, reason))
}

fn hooks_problem(installed: Option<&AtlasHooksRecord>, transcript: &Transcript) -> Option<String> {
  let installed = installed.filter(|record| record.installs(USER_PROMPT_SUBMIT))?;
  transcript.init.as_ref()?;
  let metrics = hook_metrics(transcript);
  if !metrics.ran(USER_PROMPT_SUBMIT) {
    return Some(format!(
      "the session installed hooks {} but reported no {USER_PROMPT_SUBMIT} hook event, so they did not run",
      installed.describe()
    ));
  }
  metrics.only_failed(USER_PROMPT_SUBMIT).then(|| {
    format!("every {USER_PROMPT_SUBMIT} hook of the session wrote to stderr and none added context, so the hook command is broken (see its hook_response stderr in the transcript)")
  })
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
      lexical_candidates: Vec::new(),
      failed_queries: Vec::new(),
    };
  }
  let home = match isolated_atlas_home(context.config, scratch, ATLAS_POST_HOME) {
    Ok(home) => home,
    Err(error) => return DuplicationCheck::Unavailable { reason: error.to_string() },
  };
  let atlas = AtlasCli::new(&context.config.atlas_bin, home, context.config.atlas_timeout);
  let project = format!("{}-post", project_name(&context.config.batch_dir, &context.planned.run_id));
  let linked_excludes = context.task.spec.linked.as_ref().map(LinkedSpec::exclude_globs).unwrap_or_default();
  let indexed = atlas
    .project_create(&project, &atlas_repos(context.task, tree), tree)
    .and_then(|()| match linked_excludes.is_empty() {
      true => Ok(()),
      false => atlas.set_repo_excludes(&project, SESSION_REPO, &linked_excludes, tree),
    })
    .and_then(|()| atlas.index(&project, tree).map(|_| ()));
  if let Err(error) = indexed {
    return DuplicationCheck::Unavailable { reason: error.to_string() };
  }
  let mut findings = Vec::new();
  let mut lexical_candidates = Vec::new();
  let mut failed_queries = Vec::new();
  for function in &added {
    match atlas.similar_snippet(&function.path, function.start_line, function.end_line, tree) {
      Ok(matches) => {
        let scored = duplication_findings(function, &matches, &truth.reuse_targets, &truth.forbidden_duplicates);
        findings.extend(scored.findings);
        lexical_candidates.extend(scored.lexical_candidates);
      }
      Err(error) => failed_queries.push(format!("{}:{}: {error}", function.path, function.start_line)),
    }
  }
  DuplicationCheck::Checked {
    new_functions: added,
    findings,
    lexical_candidates,
    failed_queries,
  }
}

fn judge_run(context: &RunContext<'_>, diff: &str, validation: &SuiteResult, regression: &SuiteResult, metrics: &Metrics) -> JudgeOutcome {
  if context.config.judge == JudgeMode::Off {
    return JudgeOutcome::Skipped {
      reason: "judge disabled".to_owned(),
    };
  }
  judge_rubric(&context.config.claude, &context.task.spec.prompt, diff, validation, regression, metrics, &context.dir)
}

fn review_atlas(context: &RunContext<'_>, transcript: &Transcript, diff: &str) -> JudgeOutcome {
  if context.config.judge == JudgeMode::Off {
    return JudgeOutcome::Skipped {
      reason: "judge disabled".to_owned(),
    };
  }
  judge_atlas_review(&context.config.claude, &context.task.spec.prompt, transcript, diff, &context.dir)
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
