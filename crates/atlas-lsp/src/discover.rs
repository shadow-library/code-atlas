use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::{LspError, LspResult};
use crate::process::{ServerCommand, kill_group};
use crate::spec::Launch;

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
const NPM_PING_FETCH_TIMEOUT_MS: &str = "5000";
const PROBE_OUTPUT_LIMIT: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const SERVER_BIN: &str = "typescript-language-server";
const DOWNLOAD_PACKAGES: [&str; 2] = ["typescript-language-server@5", "typescript@5"];
const PINNED_TYPESCRIPT: &str = "typescript@5 (pinned by the download command)";
const UNVERIFIED_TYPESCRIPT: &str = "bundled typescript (version not verified)";
const NOT_FOUND_HINT: &str = "typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5), set catl project config <p> --lsp-ts-command <path>, or allow download with --lsp-download on.";
const TYPESCRIPT_7_HINT: &str = "TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one";

#[derive(Debug, Clone, Copy)]
pub struct TypeScriptDiscovery<'a> {
  pub command: Option<&'a [String]>,
  pub repo_roots: &'a [PathBuf],
  pub download: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
  pub command: ServerCommand,
  pub launch: Launch,
  pub server_version: String,
  pub tsserver: Option<TsServerChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsServerChoice {
  pub path: Option<PathBuf>,
  pub version: String,
  pub source: TsServerSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsServerSource {
  Workspace { repo_root: PathBuf },
  Bundled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryStep {
  pub label: String,
  pub outcome: StepOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
  Found { detail: String },
  Missing { reason: String },
  Skipped { reason: String },
}

#[derive(Debug, Clone)]
pub struct Discovery {
  pub steps: Vec<DiscoveryStep>,
  pub result: LspResult<Discovered>,
}

#[derive(Debug, Clone)]
pub struct Probe {
  pub path: Option<OsString>,
  pub version_timeout: Duration,
  pub ping_timeout: Duration,
}

impl Default for Probe {
  fn default() -> Self {
    Self {
      path: None,
      version_timeout: VERSION_TIMEOUT,
      ping_timeout: PING_TIMEOUT,
    }
  }
}

impl Probe {
  fn search_path(&self) -> Option<OsString> {
    self.path.clone().or_else(|| env::var_os("PATH"))
  }

  fn locate(&self, name: &str) -> Option<PathBuf> {
    env::split_paths(&self.search_path()?)
      .filter(|directory| directory.is_absolute())
      .map(|directory| directory.join(name))
      .find(|candidate| is_executable(candidate))
  }

  fn command(&self, program: impl Into<PathBuf>) -> ServerCommand {
    let command = ServerCommand::new(program);
    match &self.path {
      Some(path) => command.env("PATH", path),
      None => command,
    }
  }
}

pub fn rust_analyzer_server(binary: PathBuf, version: String) -> Discovered {
  Discovered {
    command: ServerCommand::new(binary),
    launch: Launch::Installed,
    server_version: version,
    tsserver: None,
  }
}

pub fn discover_typescript(input: &TypeScriptDiscovery<'_>) -> Discovery {
  discover_typescript_with(input, &Probe::default())
}

pub fn discover_typescript_with(input: &TypeScriptDiscovery<'_>, probe: &Probe) -> Discovery {
  let roots = unique_roots(input.repo_roots);
  let mut steps = Vec::new();
  let server = match find_server(input, &roots, probe, &mut steps) {
    Ok(Some(server)) => server,
    Ok(None) => {
      return Discovery {
        steps,
        result: Err(LspError::Unavailable(NOT_FOUND_HINT.to_owned())),
      };
    }
    Err(error) => return Discovery { steps, result: Err(error) },
  };
  let result = select_tsserver(&roots, &server, &mut steps).map(|tsserver| Discovered {
    command: server.command,
    launch: server.launch,
    server_version: server.version,
    tsserver: Some(tsserver),
  });
  Discovery { steps, result }
}

struct Server {
  command: ServerCommand,
  launch: Launch,
  version: String,
}

enum Attempt {
  Found { server: Server, detail: String },
  Missing(String),
  Skipped(String),
}

enum Candidate<'a> {
  Setting(Option<&'a [String]>),
  Path,
  NodeModules(&'a Path),
  NoRoots,
  NpxCache,
  Download(bool),
}

impl Candidate<'_> {
  fn label(&self) -> String {
    match self {
      Self::Setting(_) => "lsp.typescript_command".to_owned(),
      Self::Path => format!("{SERVER_BIN} on PATH"),
      Self::NodeModules(root) => root.join("node_modules/.bin").join(SERVER_BIN).display().to_string(),
      Self::NoRoots => "node_modules/.bin of the TypeScript repos".to_owned(),
      Self::NpxCache => format!("npx cache (npx --no-install {SERVER_BIN})"),
      Self::Download(_) => format!("download ({})", DOWNLOAD_PACKAGES.join(", ")),
    }
  }

  fn attempt(&self, probe: &Probe) -> Attempt {
    match self {
      Self::Setting(None) => Attempt::Skipped("lsp.typescript_command is not set".to_owned()),
      Self::Setting(Some(argv)) => from_setting(argv, probe),
      Self::Path => match probe.locate(SERVER_BIN) {
        Some(program) => from_installed(program, probe),
        None => Attempt::Missing(format!("{SERVER_BIN} is not on PATH")),
      },
      Self::NodeModules(root) => {
        let program = root.join("node_modules/.bin").join(SERVER_BIN);
        match is_executable(&program) {
          true => from_installed(program, probe),
          false => Attempt::Missing("no executable there".to_owned()),
        }
      }
      Self::NoRoots => Attempt::Skipped("the project has no TypeScript repos".to_owned()),
      Self::NpxCache => from_npx_cache(probe),
      Self::Download(false) => Attempt::Skipped("download is off (lsp.download = false)".to_owned()),
      Self::Download(true) => from_download(probe),
    }
  }
}

fn find_server(input: &TypeScriptDiscovery<'_>, roots: &[PathBuf], probe: &Probe, steps: &mut Vec<DiscoveryStep>) -> LspResult<Option<Server>> {
  let mut candidates = vec![Candidate::Setting(input.command), Candidate::Path];
  match roots.is_empty() {
    true => candidates.push(Candidate::NoRoots),
    false => candidates.extend(roots.iter().map(|root| Candidate::NodeModules(root))),
  }
  candidates.extend([Candidate::NpxCache, Candidate::Download(input.download)]);
  let mut found = None;
  let mut fatal: Option<LspError> = None;
  for candidate in candidates {
    let label = candidate.label();
    let skip_reason = match (&fatal, &found) {
      (Some(_), _) => Some("lsp.typescript_command is set but unusable"),
      (None, Some(_)) => Some("an earlier candidate was found"),
      (None, None) => None,
    };
    if let Some(reason) = skip_reason {
      steps.push(step(label, StepOutcome::Skipped { reason: reason.to_owned() }));
      continue;
    }
    let outcome = match candidate.attempt(probe) {
      Attempt::Found { server, detail } => {
        found = Some(server);
        StepOutcome::Found { detail }
      }
      Attempt::Missing(reason) => {
        if let Candidate::Setting(Some(argv)) = &candidate {
          let program = argv.first().map_or("", String::as_str);
          fatal = Some(LspError::Unavailable(format!("lsp.typescript_command ({program}): {reason}")));
        }
        StepOutcome::Missing { reason }
      }
      Attempt::Skipped(reason) => StepOutcome::Skipped { reason },
    };
    steps.push(step(label, outcome));
  }
  match fatal {
    Some(error) => Err(error),
    None => Ok(found),
  }
}

fn step(label: String, outcome: StepOutcome) -> DiscoveryStep {
  DiscoveryStep { label, outcome }
}

fn from_setting(argv: &[String], probe: &Probe) -> Attempt {
  let Some((program, rest)) = argv.split_first() else {
    return Attempt::Missing("the command is empty".to_owned());
  };
  let program = PathBuf::from(program);
  if !program.is_absolute() {
    return Attempt::Missing(format!("{} is not an absolute path", program.display()));
  }
  let probe_args: Vec<OsString> = rest.iter().filter(|arg| *arg != "--stdio").map(OsString::from).collect();
  let mut command = probe.command(&program);
  command.args = rest.iter().map(OsString::from).collect();
  if !rest.iter().any(|arg| arg == "--stdio") {
    command = command.arg("--stdio");
  }
  found_by_version(command, Launch::Installed, &probe_args, probe)
}

fn from_installed(program: PathBuf, probe: &Probe) -> Attempt {
  let command = probe.command(&program).arg("--stdio");
  found_by_version(command, Launch::Installed, &[], probe)
}

fn from_npx_cache(probe: &Probe) -> Attempt {
  let Some(npx) = probe.locate("npx") else {
    return Attempt::Missing("npx is not on PATH".to_owned());
  };
  let command = probe.command(npx).arg("--no-install").arg(SERVER_BIN).arg("--stdio").cwd(env::temp_dir());
  let probe_args = [OsString::from("--no-install"), OsString::from(SERVER_BIN)];
  found_by_version(command, Launch::Installed, &probe_args, probe)
}

fn found_by_version(command: ServerCommand, launch: Launch, leading_args: &[OsString], probe: &Probe) -> Attempt {
  let mut args = leading_args.to_vec();
  args.push(OsString::from("--version"));
  match probe_version(&command, &args, probe) {
    Ok(version) => Attempt::Found {
      detail: format!("{} (version {version})", command.program.display()),
      server: Server { command, launch, version },
    },
    Err(reason) => Attempt::Missing(reason),
  }
}

fn from_download(probe: &Probe) -> Attempt {
  let Some(npx) = probe.locate("npx") else {
    return Attempt::Missing("download is on but npx is not on PATH".to_owned());
  };
  let Some(npm) = probe.locate("npm") else {
    return Attempt::Missing("download is on but npm is not on PATH".to_owned());
  };
  let ping = [
    OsString::from("ping"),
    OsString::from("--fetch-timeout"),
    OsString::from(NPM_PING_FETCH_TIMEOUT_MS),
    OsString::from("--fetch-retries"),
    OsString::from("0"),
  ];
  match run(&probe.command(npm), &ping, probe.ping_timeout) {
    Ran::Exited { success: true, .. } => {}
    Ran::Exited { code, stderr, .. } => {
      return Attempt::Missing(format!(
        "download is on but the npm registry is unreachable (npm ping {}{})",
        exit_text(code),
        stderr_suffix(&stderr)
      ));
    }
    Ran::TimedOut => {
      return Attempt::Missing(format!(
        "download is on but the npm registry did not answer npm ping within {}s",
        seconds(probe.ping_timeout)
      ));
    }
    Ran::Failed(reason) => return Attempt::Missing(format!("download is on but npm ping could not run: {reason}")),
  }
  let mut command = probe.command(npx).arg("--yes");
  for package in DOWNLOAD_PACKAGES {
    command = command.arg("-p").arg(package);
  }
  let command = command.arg(SERVER_BIN).arg("--stdio").cwd(env::temp_dir());
  Attempt::Found {
    detail: "the npm registry answers; npx fetches the server on first start".to_owned(),
    server: Server {
      command,
      launch: Launch::Download,
      version: format!("{} (fetched on first start)", DOWNLOAD_PACKAGES[0]),
    },
  }
}

fn probe_version(command: &ServerCommand, args: &[OsString], probe: &Probe) -> Result<String, String> {
  let probed = ServerCommand {
    args: args.to_vec(),
    ..command.clone()
  };
  match run(&probed, &[], probe.version_timeout) {
    Ran::Exited { success: true, stdout, .. } => {
      let line = stdout.lines().map(str::trim).find(|line| !line.is_empty());
      match line {
        None => Err("--version printed nothing".to_owned()),
        Some(line) => parse_version(line).ok_or_else(|| format!("--version printed no version number: {:?}", truncated(line))),
      }
    }
    Ran::Exited { code, stderr, .. } => Err(format!("--version {}{}", exit_text(code), stderr_suffix(&stderr))),
    Ran::TimedOut => Err(format!("--version did not answer within {}s", seconds(probe.version_timeout))),
    Ran::Failed(reason) => Err(reason),
  }
}

fn parse_version(line: &str) -> Option<String> {
  line
    .split_whitespace()
    .map(|token| token.trim_start_matches('v'))
    .find(|token| token.starts_with(|c: char| c.is_ascii_digit()) && token.contains('.'))
    .map(str::to_owned)
}

fn exit_text(code: Option<i32>) -> String {
  code.map_or_else(|| "was killed by a signal".to_owned(), |code| format!("exited with code {code}"))
}

fn stderr_suffix(stderr: &str) -> String {
  match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
    Some(line) => format!(": {}", truncated(line)),
    None => String::new(),
  }
}

fn truncated(line: &str) -> String {
  line.chars().take(200).collect()
}

fn seconds(duration: Duration) -> f64 {
  duration.as_secs_f64()
}

enum Ran {
  Exited {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
  },
  TimedOut,
  Failed(String),
}

fn run(command: &ServerCommand, extra_args: &[OsString], timeout: Duration) -> Ran {
  let mut process = Command::new(&command.program);
  process
    .args(&command.args)
    .args(extra_args)
    .envs(command.env.iter().map(|(name, value)| (name, value)))
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  if let Some(cwd) = &command.cwd {
    process.current_dir(cwd);
  }
  #[cfg(unix)]
  std::os::unix::process::CommandExt::process_group(&mut process, 0);
  let mut child = match process.spawn() {
    Ok(child) => child,
    Err(error) => return Ran::Failed(format!("cannot start {}: {error}", command.program.display())),
  };
  let pid = child.id();
  let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
    terminate(&mut child, pid);
    return Ran::Failed("the child has no piped stdio".to_owned());
  };
  thread::scope(|scope| {
    let readers = (|| -> io::Result<_> {
      let out = thread::Builder::new().name("lsp-probe-out".to_owned()).spawn_scoped(scope, move || drain(stdout))?;
      let err = thread::Builder::new().name("lsp-probe-err".to_owned()).spawn_scoped(scope, move || drain(stderr))?;
      Ok((out, err))
    })();
    let (out, err) = match readers {
      Ok(readers) => readers,
      Err(error) => {
        terminate(&mut child, pid);
        return Ran::Failed(format!("cannot start the output reader: {error}"));
      }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
      match child.try_wait() {
        Ok(Some(status)) => break Ok(Some(status)),
        Ok(None) if Instant::now() >= deadline => break Ok(None),
        Ok(None) => thread::sleep(POLL_INTERVAL),
        Err(error) => break Err(error),
      }
    };
    match status {
      Ok(Some(status)) => {
        kill_group(pid);
        Ran::Exited {
          success: status.success(),
          code: status.code(),
          stdout: out.join().unwrap_or_default(),
          stderr: err.join().unwrap_or_default(),
        }
      }
      Ok(None) => {
        terminate(&mut child, pid);
        Ran::TimedOut
      }
      Err(error) => {
        terminate(&mut child, pid);
        Ran::Failed(format!("cannot wait for {}: {error}", command.program.display()))
      }
    }
  })
}

fn terminate(child: &mut std::process::Child, pid: u32) {
  kill_group(pid);
  let _killed = child.kill();
  let _reaped = child.wait();
}

fn drain(stream: impl Read) -> String {
  let mut kept = Vec::new();
  let mut limited = stream.take(PROBE_OUTPUT_LIMIT);
  if limited.read_to_end(&mut kept).is_err() {
    return String::from_utf8_lossy(&kept).into_owned();
  }
  let mut rest = limited.into_inner();
  let _drained = io::copy(&mut rest, &mut io::sink());
  String::from_utf8_lossy(&kept).into_owned()
}

fn is_executable(path: &Path) -> bool {
  let Ok(metadata) = fs::metadata(path) else {
    return false;
  };
  if !metadata.is_file() {
    return false;
  }
  #[cfg(unix)]
  return std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0;
  #[cfg(not(unix))]
  true
}

fn unique_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
  let mut unique: Vec<PathBuf> = Vec::new();
  for root in roots {
    if !unique.contains(root) {
      unique.push(root.clone());
    }
  }
  unique
}

enum Installed {
  Absent(String),
  Unknown(String),
  TooNew(String),
  Usable { tsserver: PathBuf, version: String },
}

#[derive(Deserialize)]
struct PackageManifest {
  version: Option<String>,
}

fn inspect_typescript(dir: &Path) -> Installed {
  let manifest = dir.join("package.json");
  let tsserver = dir.join("lib/tsserver.js");
  let version = fs::read_to_string(&manifest)
    .ok()
    .and_then(|text| serde_json::from_str::<PackageManifest>(&text).ok())
    .and_then(|manifest| manifest.version)
    .filter(|version| !version.trim().is_empty());
  let Some(version) = version else {
    return match manifest.exists() || tsserver.is_file() {
      true => Installed::Unknown("typescript is installed but its package.json has no readable version".to_owned()),
      false => Installed::Absent("typescript is not installed there".to_owned()),
    };
  };
  match version.split('.').next().and_then(|major| major.parse::<u32>().ok()) {
    None => Installed::Unknown(format!("typescript version {version:?} cannot be read as a version number")),
    Some(major) if major >= 7 => Installed::TooNew(version),
    Some(_) if !tsserver.is_file() => Installed::Absent(format!("typescript {version} has no lib/tsserver.js")),
    Some(_) => Installed::Usable { tsserver, version },
  }
}

fn bundled_typescript(command: &ServerCommand) -> Option<(PathBuf, String)> {
  let entries = std::iter::once(command.program.clone()).chain(command.args.iter().map(PathBuf::from).filter(|arg| arg.is_absolute() && arg.is_file()));
  for entry in entries {
    let Ok(real) = fs::canonicalize(&entry) else {
      continue;
    };
    for ancestor in real.ancestors().skip(1) {
      let mut dirs = vec![ancestor.join("node_modules/typescript")];
      if ancestor.file_name().is_some_and(|name| name == "node_modules") {
        dirs.push(ancestor.join("typescript"));
      }
      for dir in dirs {
        match inspect_typescript(&dir) {
          Installed::Absent(_) if !dir.join("package.json").exists() => continue,
          Installed::Usable { tsserver, version } => return Some((tsserver, version)),
          _ => return None,
        }
      }
    }
  }
  None
}

fn select_tsserver(roots: &[PathBuf], server: &Server, steps: &mut Vec<DiscoveryStep>) -> LspResult<TsServerChoice> {
  let mut chosen = None;
  let mut too_new = false;
  let mut skipped_installs = false;
  for root in roots {
    let label = format!("tsserver in {}", root.display());
    if chosen.is_some() {
      steps.push(step(
        label,
        StepOutcome::Skipped {
          reason: "an earlier repo supplies tsserver".to_owned(),
        },
      ));
      continue;
    }
    let outcome = match inspect_typescript(&root.join("node_modules/typescript")) {
      Installed::Usable { tsserver, version } => {
        let detail = format!("typescript {version} at {}", tsserver.display());
        chosen = Some(TsServerChoice {
          path: Some(tsserver),
          version,
          source: TsServerSource::Workspace { repo_root: root.clone() },
        });
        StepOutcome::Found { detail }
      }
      Installed::TooNew(version) => {
        too_new = true;
        StepOutcome::Skipped {
          reason: format!("typescript {version} has no tsserver"),
        }
      }
      Installed::Unknown(reason) => {
        skipped_installs = true;
        StepOutcome::Skipped { reason }
      }
      Installed::Absent(reason) => StepOutcome::Missing { reason },
    };
    steps.push(step(label, outcome));
  }
  if let Some(choice) = chosen {
    return Ok(choice);
  }
  let label = "tsserver bundled with the server".to_owned();
  if server.launch == Launch::Download {
    steps.push(step(
      label,
      StepOutcome::Found {
        detail: "the download command pins typescript@5".to_owned(),
      },
    ));
    return Ok(bundled(None, PINNED_TYPESCRIPT));
  }
  match bundled_typescript(&server.command) {
    Some((tsserver, version)) => {
      steps.push(step(
        label,
        StepOutcome::Found {
          detail: format!("typescript {version} at {}", tsserver.display()),
        },
      ));
      let path = (too_new || skipped_installs).then_some(tsserver);
      Ok(bundled(path, &version))
    }
    None if too_new => {
      let reason = format!("no typescript below 7 resolves from {}", server.command.program.display());
      steps.push(step(label, StepOutcome::Missing { reason }));
      Err(LspError::Unavailable(TYPESCRIPT_7_HINT.to_owned()))
    }
    None => {
      steps.push(step(
        label,
        StepOutcome::Found {
          detail: "the server's own typescript; its version could not be read".to_owned(),
        },
      ));
      Ok(bundled(None, UNVERIFIED_TYPESCRIPT))
    }
  }
}

fn bundled(path: Option<PathBuf>, version: &str) -> TsServerChoice {
  TsServerChoice {
    path,
    version: version.to_owned(),
    source: TsServerSource::Bundled,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_read_a_version_number_from_typical_version_output() {
    assert_eq!(parse_version("5.1.3"), Some("5.1.3".to_owned()));
    assert_eq!(parse_version("typescript-language-server v4.3.4"), Some("4.3.4".to_owned()));
    assert_eq!(parse_version("rust-analyzer 1.85.0 (4d91de4e 2025-02-17)"), Some("1.85.0".to_owned()));
    assert_eq!(parse_version("segfault"), None);
    assert_eq!(parse_version("7"), None);
  }
}
