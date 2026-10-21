use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::atlas_cli::{AtlasCli, isolated_command};
use super::claude::{ATLAS_SERVER_NAME, ClaudeConfig, EMPTY_MCP_CONFIG, HookWiring, SessionRequest, SessionTools, host_env, session_invocation};
use super::error::{AgentError, AgentResult};
use super::plan::AtlasSurface;
use super::process::{ProcessExit, run_captured, run_streaming};
use super::record::write_json;
use super::surface::{
  ConfigurationResult, EXTENSION_RETRY_CAP, EXTENSION_SESSION_CAP, LoadingDelta, McpArm, SURFACE_MAX_TURNS, SURFACE_PROMPT, SURFACE_REPEATS, SURFACE_RETRY_CAP,
  SURFACE_SESSION_CAP, SessionLedger, SessionSample, SurfaceReport, TOOL_SEARCH_TOOL, TOOL_SEARCH_VAR, TokenMeasurement, TokenSection, ToolLoading, loading_delta,
  measure_surface_bytes, sample_from_transcript, search_allowed_configurations, surface_configurations,
};
use super::transcript::Transcript;
use super::workspace::{check_isolated_location, copy_dir};

const PROJECT_NAME: &str = "surface";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MCP_TIMEOUT: Duration = Duration::from_secs(60);
const CLI_TIMEOUT: Duration = Duration::from_secs(600);
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct SurfaceOptions {
  pub atlas_bin: PathBuf,
  pub claude: ClaudeConfig,
  pub fixture: PathBuf,
  pub work_dir: PathBuf,
  pub sessions: bool,
  pub tools: Option<String>,
}

pub fn run_surface(options: &SurfaceOptions) -> AgentResult<SurfaceReport> {
  let (cli, fixture) = prepare(options)?;
  let (initialize, tools_list) = fetch_mcp_surface(&options.atlas_bin, cli.home(), &mcp_arguments(options), &fixture)?;
  let bytes = measure_surface_bytes(&initialize, &tools_list);
  let claude_code_version = claude_version(&options.claude.binary);
  let tokens = match (options.sessions, &claude_code_version) {
    (false, _) => TokenSection::NotMeasured {
      reason: "sessions were not requested".to_owned(),
    },
    (true, None) => TokenSection::NotMeasured {
      reason: "the claude binary is unavailable".to_owned(),
    },
    (true, Some(_)) => measure_tokens(options, &cli)?,
  };
  Ok(SurfaceReport {
    model: options.claude.model.clone(),
    tools: options.tools.clone(),
    claude_code_version,
    atlas_version: cli.version(),
    bytes,
    tokens,
  })
}

fn prepare(options: &SurfaceOptions) -> AgentResult<(AtlasCli, PathBuf)> {
  fs::create_dir_all(&options.work_dir).map_err(|error| AgentError::io(&options.work_dir, error))?;
  let fixture = options.work_dir.join("fixture");
  fs::create_dir_all(&fixture).map_err(|error| AgentError::io(&fixture, error))?;
  copy_dir(&options.fixture, &fixture)?;
  let repos = repositories(&fixture)?;
  let cli = AtlasCli::new(&options.atlas_bin, options.work_dir.join("atlas-home"), CLI_TIMEOUT);
  cli.project_create(PROJECT_NAME, &repos, &fixture)?;
  cli.index(PROJECT_NAME, &fixture)?;
  Ok((cli, fixture))
}

fn repositories(fixture: &Path) -> AgentResult<Vec<PathBuf>> {
  let entries = fs::read_dir(fixture).map_err(|error| AgentError::io(fixture, error))?;
  let mut repos: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
  repos.sort();
  Ok(repos)
}

fn claude_version(binary: &Path) -> Option<String> {
  let mut command = Command::new(binary);
  command.arg("--version");
  let output = run_captured(command, None, Duration::from_secs(30));
  output.exit.succeeded().then(|| output.stdout.trim().to_owned())
}

fn mcp_arguments(options: &SurfaceOptions) -> Vec<String> {
  let mut arguments = vec!["mcp".to_owned(), "--project".to_owned(), PROJECT_NAME.to_owned()];
  if let Some(tools) = &options.tools {
    arguments.extend(["--tools".to_owned(), tools.clone()]);
  }
  arguments
}

pub fn fetch_mcp_surface(binary: &Path, home: &Path, arguments: &[String], cwd: &Path) -> AgentResult<(Value, Value)> {
  let mut child = isolated_command(binary)
    .args(arguments)
    .current_dir(cwd)
    .env("CODE_ATLAS_HOME", home)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|error| AgentError::Command {
      command: "catl mcp".to_owned(),
      detail: format!("cannot start: {error}"),
    })?;
  let outcome = exchange(&mut child);
  let _ = child.kill();
  let _ = child.wait();
  outcome
}

fn exchange(child: &mut Child) -> AgentResult<(Value, Value)> {
  let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
    return Err(mcp_error("the server has no stdio pipes".to_owned()));
  };
  let (sender, lines) = mpsc::channel();
  thread::spawn(move || {
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
      if sender.send(line).is_err() {
        break;
      }
    }
  });
  let capabilities = json!({"protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "atlas-eval-surface", "version": "0"}});
  let initialize = request(&mut stdin, &lines, 1, "initialize", capabilities)?;
  notify(&mut stdin, "notifications/initialized")?;
  let tools_list = request(&mut stdin, &lines, 2, "tools/list", json!({}))?;
  if tools_list.get("nextCursor").is_some() {
    return Err(mcp_error("tools/list is paginated; the byte accounting expects one page".to_owned()));
  }
  Ok((initialize, tools_list))
}

fn request(stdin: &mut ChildStdin, lines: &Receiver<String>, id: u64, method: &str, params: Value) -> AgentResult<Value> {
  send(stdin, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
  let deadline = Instant::now() + MCP_TIMEOUT;
  loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let line = lines.recv_timeout(remaining).map_err(|_| mcp_error(format!("no response to {method}")))?;
    let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
    if message.get("id").and_then(Value::as_u64) != Some(id) {
      continue;
    }
    if let Some(error) = message.get("error") {
      return Err(mcp_error(format!("{method} failed: {error}")));
    }
    return message.get("result").cloned().ok_or_else(|| mcp_error(format!("{method} returned no result")));
  }
}

fn notify(stdin: &mut ChildStdin, method: &str) -> AgentResult<()> {
  send(stdin, &json!({"jsonrpc": "2.0", "method": method}))
}

fn send(stdin: &mut ChildStdin, message: &Value) -> AgentResult<()> {
  writeln!(stdin, "{message}")
    .and_then(|()| stdin.flush())
    .map_err(|error| mcp_error(format!("cannot write to the server: {error}")))
}

fn mcp_error(detail: String) -> AgentError {
  AgentError::Command {
    command: "catl mcp".to_owned(),
    detail,
  }
}

struct Budget {
  session_cap: u32,
  retry_cap: u32,
  started: u32,
  retries: u32,
}

impl Budget {
  fn new(session_cap: u32, retry_cap: u32) -> Self {
    Self {
      session_cap,
      retry_cap,
      started: 0,
      retries: 0,
    }
  }

  fn may_start(&self, retry: bool) -> bool {
    self.started < self.session_cap + self.retry_cap && (!retry || self.retries < self.retry_cap)
  }

  fn ledger(&self) -> SessionLedger {
    SessionLedger {
      sessions_started: self.started,
      retries_used: self.retries,
    }
  }
}

struct Workbench {
  tree: PathBuf,
  sessions_dir: PathBuf,
  empty_config: PathBuf,
  atlas_config: PathBuf,
}

struct Matrix {
  samples: Vec<Vec<SessionSample>>,
  failures: Vec<Vec<String>>,
  budget: Budget,
  note: Option<String>,
}

fn workbench(options: &SurfaceOptions, cli: &AtlasCli) -> AgentResult<Workbench> {
  let tree = options.work_dir.join("session-tree");
  fs::create_dir_all(&tree).map_err(|error| AgentError::io(&tree, error))?;
  check_isolated_location(&tree, &[])?;
  let sessions_dir = options.work_dir.join("sessions");
  fs::create_dir_all(&sessions_dir).map_err(|error| AgentError::io(&sessions_dir, error))?;
  let empty_config = options.work_dir.join("empty-mcp.json");
  fs::write(&empty_config, EMPTY_MCP_CONFIG).map_err(|error| AgentError::io(&empty_config, error))?;
  let atlas_config = options.work_dir.join("atlas-mcp.json");
  let mut config = cli.mcp_config(PROJECT_NAME, None);
  config["mcpServers"][ATLAS_SERVER_NAME]["args"] = Value::from(mcp_arguments(options));
  write_json(&atlas_config, &config)?;
  Ok(Workbench {
    tree,
    sessions_dir,
    empty_config,
    atlas_config,
  })
}

fn run_matrix(
  options: &SurfaceOptions,
  bench: &Workbench,
  plan: &[(McpArm, ToolLoading)],
  mut budget: Budget,
  mut samples: Vec<Vec<SessionSample>>,
  deferral_ceiling: Option<u64>,
) -> Result<Matrix, String> {
  let mut failures: Vec<Vec<String>> = plan.iter().map(|_| Vec::new()).collect();
  for repeat in 1..=SURFACE_REPEATS {
    for (index, (arm, loading)) in plan.iter().enumerate() {
      let mcp_config = if *arm == McpArm::CodeAtlas { &bench.atlas_config } else { &bench.empty_config };
      let mut attempt = 0;
      while samples[index].len() < repeat as usize && budget.may_start(attempt > 0) {
        if attempt > 0 {
          budget.retries += 1;
        }
        budget.started += 1;
        attempt += 1;
        let label = format!("{}-{}-r{repeat}-a{attempt}", arm_label(*arm), loading_label(*loading));
        match run_session(options, bench, mcp_config, *arm, *loading, &label) {
          Ok(sample) => {
            let first_ever = samples.iter().all(Vec::is_empty);
            let not_deferred = deferral_ceiling.is_some_and(|ceiling| !sample.exposure.tool_search_tool_present || sample.first_request.prompt_total >= ceiling);
            samples[index].push(sample);
            if first_ever && not_deferred {
              let note =
                "the verification session lacked the ToolSearch tool or its prompt was not smaller than the non-deferred one; the remaining sessions were not run".to_owned();
              return Ok(Matrix {
                samples,
                failures,
                budget,
                note: Some(note),
              });
            }
            break;
          }
          Err(reason) => {
            let reason = scrub(&reason, &options.work_dir);
            tracing::warn!(%label, %reason, "surface session failed");
            let first_ever = samples.iter().all(Vec::is_empty);
            failures[index].push(format!("{label}: {reason}"));
            if first_ever {
              return Err(format!("the first session failed: {reason}"));
            }
          }
        }
      }
    }
  }
  Ok(Matrix {
    samples,
    failures,
    budget,
    note: None,
  })
}

fn results_of(plan: &[(McpArm, ToolLoading)], matrix: Matrix) -> (Vec<ConfigurationResult>, Budget, Option<String>) {
  let results = plan
    .iter()
    .zip(matrix.samples)
    .zip(matrix.failures)
    .map(|(((arm, loading), samples), failed)| ConfigurationResult::new(*arm, *loading, samples, failed))
    .filter(|result| !result.samples.is_empty() || !result.failed_attempts.is_empty())
    .collect();
  (results, matrix.budget, matrix.note)
}

fn deltas_of(results: &[ConfigurationResult], loadings: &[ToolLoading]) -> Vec<LoadingDelta> {
  loadings
    .iter()
    .filter_map(|loading| {
      let pick = |arm: McpArm| results.iter().find(|result| result.arm == arm && result.loading == *loading);
      loading_delta(*loading, pick(McpArm::NoMcp)?, pick(McpArm::CodeAtlas)?)
    })
    .collect()
}

fn measure_tokens(options: &SurfaceOptions, cli: &AtlasCli) -> AgentResult<TokenSection> {
  let bench = workbench(options, cli)?;
  let plan = surface_configurations();
  let matrix = match run_matrix(
    options,
    &bench,
    &plan,
    Budget::new(SURFACE_SESSION_CAP, SURFACE_RETRY_CAP),
    plan.iter().map(|_| Vec::new()).collect(),
    None,
  ) {
    Ok(matrix) => matrix,
    Err(reason) => return Ok(TokenSection::NotMeasured { reason }),
  };
  let (configurations, budget, _) = results_of(&plan, matrix);
  let ledger = budget.ledger();
  let deltas = deltas_of(&configurations, &[ToolLoading::Upfront, ToolLoading::SearchToolDenied]);
  Ok(TokenSection::Measured(Box::new(TokenMeasurement {
    prompt: SURFACE_PROMPT.to_owned(),
    max_turns: SURFACE_MAX_TURNS,
    repeats: SURFACE_REPEATS,
    sessions_started: ledger.sessions_started,
    retries_used: ledger.retries_used,
    configurations,
    deltas,
    extension: None,
    extension_note: None,
  })))
}

pub fn extend_surface(options: &SurfaceOptions, mut report: SurfaceReport) -> AgentResult<SurfaceReport> {
  let invalid = |reason: &str| AgentError::InvalidArgument(reason.to_owned());
  let TokenSection::Measured(measurement) = &mut report.tokens else {
    return Err(invalid("the recorded report has no measured token section to extend"));
  };
  if report.model != options.claude.model {
    return Err(invalid("the recorded report used another model"));
  }
  if report.tools != options.tools {
    return Err(invalid("the recorded report used another tool profile"));
  }
  let version = claude_version(&options.claude.binary);
  if version != report.claude_code_version {
    return Err(invalid("the claude binary is not the version of the recorded report"));
  }
  let (cli, _) = prepare(options)?;
  let bench = workbench(options, &cli)?;
  let plan = search_allowed_configurations();
  let ceiling = measurement
    .configurations
    .iter()
    .find(|result| result.arm == McpArm::CodeAtlas && result.loading == ToolLoading::SearchToolDenied)
    .and_then(|result| result.prompt_total)
    .map(|spread| spread.min);
  let seed: Vec<Vec<SessionSample>> = plan
    .iter()
    .map(|(arm, loading)| {
      let recorded = measurement.configurations.iter().find(|result| result.arm == *arm && result.loading == *loading);
      recorded.map(|result| result.samples.clone()).unwrap_or_default()
    })
    .collect();
  let resumed = measurement.extension.unwrap_or(SessionLedger {
    sessions_started: 0,
    retries_used: 0,
  });
  let deferral_ceiling = if seed.iter().all(Vec::is_empty) { ceiling } else { None };
  let mut budget = Budget::new(EXTENSION_SESSION_CAP, EXTENSION_RETRY_CAP);
  budget.started = resumed.sessions_started;
  budget.retries = resumed.retries_used;
  let matrix = run_matrix(options, &bench, &plan, budget, seed, deferral_ceiling).map_err(|reason| AgentError::Command {
    command: "claude".to_owned(),
    detail: reason,
  })?;
  let (results, budget, note) = results_of(&plan, matrix);
  measurement.configurations.retain(|result| result.loading != ToolLoading::SearchAllowed);
  measurement.deltas.retain(|delta| delta.loading != ToolLoading::SearchAllowed);
  measurement.configurations.extend(results);
  let added = deltas_of(&measurement.configurations, &[ToolLoading::SearchAllowed]);
  measurement.deltas.extend(added);
  measurement.extension = Some(budget.ledger());
  measurement.extension_note = note;
  Ok(report)
}

fn run_session(options: &SurfaceOptions, bench: &Workbench, mcp_config: &Path, arm: McpArm, loading: ToolLoading, label: &str) -> Result<SessionSample, String> {
  let sessions_dir = &bench.sessions_dir;
  let request = SessionRequest {
    prompt: SURFACE_PROMPT.to_owned(),
    tree: bench.tree.clone(),
    mcp_config: mcp_config.to_owned(),
    max_turns: SURFACE_MAX_TURNS,
    tools: match arm {
      McpArm::NoMcp => SessionTools::Baseline,
      McpArm::CodeAtlas => SessionTools::Atlas(AtlasSurface::Standard),
    },
    config_dir: options.work_dir.join("claude-config"),
    project_instructions: None,
    tree_env: Vec::new(),
    hooks: HookWiring::Unchanged,
  };
  let mut invocation = session_invocation(&options.claude, &request, &host_env());
  for (key, value) in &mut invocation.env {
    if key == TOOL_SEARCH_VAR {
      *value = loading.tool_search_value().to_owned();
    }
  }
  if loading.allows_tool_search_tool() {
    allow_tool_search_tool(&mut invocation.args);
  }
  let transcript_path = sessions_dir.join(format!("{label}.jsonl"));
  let output = run_streaming(invocation.command(), SESSION_TIMEOUT, &transcript_path, &sessions_dir.join(format!("{label}.stderr.log")))
    .map_err(|error| format!("cannot record the session: {error}"))?;
  let text = fs::read_to_string(&transcript_path).map_err(|error| format!("cannot read the transcript: {error}"))?;
  let transcript = Transcript::parse(&text, &output.line_offsets_ms);
  let exit = match &output.exit {
    ProcessExit::SpawnFailed { .. } => ProcessExit::SpawnFailed {
      reason: "cannot start the claude binary".to_owned(),
    },
    other => other.clone(),
  };
  sample_from_transcript(&exit, &transcript)
}

fn allow_tool_search_tool(args: &mut [String]) {
  for index in 1..args.len() {
    if args[index - 1] == "--tools" || args[index - 1] == "--allowedTools" {
      args[index] = format!("{},{TOOL_SEARCH_TOOL}", args[index]);
    }
  }
}

fn arm_label(arm: McpArm) -> &'static str {
  match arm {
    McpArm::NoMcp => "no-mcp",
    McpArm::CodeAtlas => "atlas",
  }
}

fn loading_label(loading: ToolLoading) -> &'static str {
  match loading {
    ToolLoading::Upfront => "upfront",
    ToolLoading::SearchToolDenied => "search-denied",
    ToolLoading::SearchAllowed => "search-allowed",
  }
}

fn scrub(text: &str, work_dir: &Path) -> String {
  let mut scrubbed = text.replace(&work_dir.display().to_string(), "<work>");
  if let Some(home) = std::env::var_os("HOME") {
    scrubbed = scrubbed.replace(&home.to_string_lossy().into_owned(), "~");
  }
  scrubbed
}
