use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use crate::error::{LspError, LspResult};
use crate::transport::Transport;

pub const STDERR_TAIL_BYTES: usize = 8 * 1024;
const KILL_REAP_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerCommand {
  pub program: PathBuf,
  pub args: Vec<OsString>,
  pub env: Vec<(OsString, OsString)>,
  pub cwd: Option<PathBuf>,
}

impl ServerCommand {
  pub fn new(program: impl Into<PathBuf>) -> Self {
    Self {
      program: program.into(),
      ..Self::default()
    }
  }

  pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
    self.args.push(arg.into());
    self
  }

  pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
    self.env.push((name.into(), value.into()));
    self
  }

  pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
    self.cwd = Some(cwd.into());
    self
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessExit {
  Code(i32),
  Signal(i32),
  Unknown,
}

impl ProcessExit {
  fn of(status: ExitStatus) -> Self {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
      return Self::Signal(signal);
    }
    status.code().map_or(Self::Unknown, Self::Code)
  }
}

impl fmt::Display for ProcessExit {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Code(code) => write!(formatter, "exited with code {code}"),
      Self::Signal(signal) => write!(formatter, "was killed by signal {signal}"),
      Self::Unknown => formatter.write_str("exited"),
    }
  }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct TailBuffer {
  bytes: VecDeque<u8>,
  closed: bool,
}

#[derive(Default)]
struct TailState {
  buffer: Mutex<TailBuffer>,
  changed: Condvar,
}

#[derive(Clone, Default)]
pub struct StderrTail {
  state: Arc<TailState>,
}

impl StderrTail {
  pub fn capture(mut stream: impl Read + Send + 'static) -> Self {
    let tail = Self::default();
    let writer = tail.clone();
    let spawned = thread::Builder::new().name("lsp-stderr".to_owned()).spawn(move || {
      let mut chunk = [0u8; 4096];
      loop {
        match stream.read(&mut chunk) {
          Ok(0) => break,
          Ok(read) => writer.push(&chunk[..read]),
          Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
          Err(error) => {
            tracing::debug!(%error, "reading language server stderr failed");
            break;
          }
        }
      }
      writer.close();
    });
    if let Err(error) = spawned {
      tracing::warn!(%error, "cannot start the stderr reader thread");
      tail.close();
    }
    tail
  }

  fn push(&self, bytes: &[u8]) {
    let mut buffer = lock(&self.state.buffer);
    buffer.bytes.extend(bytes);
    let excess = buffer.bytes.len().saturating_sub(STDERR_TAIL_BYTES);
    buffer.bytes.drain(..excess);
  }

  fn close(&self) {
    lock(&self.state.buffer).closed = true;
    self.state.changed.notify_all();
  }

  pub fn snapshot(&self) -> String {
    let buffer = lock(&self.state.buffer);
    let (front, back) = buffer.bytes.as_slices();
    String::from_utf8_lossy(&[front, back].concat()).into_owned()
  }

  pub fn wait_closed(&self, timeout: Duration) -> bool {
    let buffer = lock(&self.state.buffer);
    let (buffer, _) = self
      .state
      .changed
      .wait_timeout_while(buffer, timeout, |buffer| !buffer.closed)
      .unwrap_or_else(PoisonError::into_inner);
    buffer.closed
  }
}

type ExitListener = Box<dyn FnOnce(ProcessExit) + Send>;

#[derive(Default)]
struct ExitSlot {
  exit: Option<ProcessExit>,
  listeners: Vec<ExitListener>,
}

#[derive(Default)]
struct ExitState {
  slot: Mutex<ExitSlot>,
  changed: Condvar,
}

impl ExitState {
  fn record(&self, exit: ProcessExit) {
    let listeners = {
      let mut slot = lock(&self.slot);
      slot.exit = Some(exit);
      std::mem::take(&mut slot.listeners)
    };
    self.changed.notify_all();
    for listener in listeners {
      listener(exit);
    }
  }
}

#[derive(Clone)]
pub(crate) struct ExitHandle(Arc<ExitState>);

impl ExitHandle {
  pub(crate) fn get(&self) -> Option<ProcessExit> {
    lock(&self.0.slot).exit
  }

  pub(crate) fn wait(&self, timeout: Duration) -> Option<ProcessExit> {
    let slot = lock(&self.0.slot);
    let (slot, _) = self
      .0
      .changed
      .wait_timeout_while(slot, timeout, |slot| slot.exit.is_none())
      .unwrap_or_else(PoisonError::into_inner);
    slot.exit
  }
}

pub struct ServerProcess {
  pid: u32,
  program: String,
  exit: Arc<ExitState>,
  stderr: StderrTail,
}

impl ServerProcess {
  pub fn spawn(command: &ServerCommand) -> LspResult<(Self, Transport)> {
    let program = command.program.display().to_string();
    let mut process = Command::new(&command.program);
    process
      .args(&command.args)
      .envs(command.env.iter().map(|(name, value)| (name, value)))
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped());
    if let Some(cwd) = &command.cwd {
      process.current_dir(cwd);
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut process, 0);
    let spawn_error = |reason: String| LspError::Spawn { program: program.clone(), reason };
    let mut child = process.spawn().map_err(|error| spawn_error(error.to_string()))?;
    let pid = child.id();
    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
      kill_group(pid);
      let _reaped = child.wait();
      return Err(spawn_error("the child has no piped stdio".to_owned()));
    };
    let exit = Arc::new(ExitState::default());
    let watched = Arc::clone(&exit);
    let watcher = thread::Builder::new().name("lsp-exit".to_owned()).spawn(move || {
      let status = child.wait();
      kill_group(pid);
      watched.record(status.map_or(ProcessExit::Unknown, ProcessExit::of));
    });
    if let Err(error) = watcher {
      kill_group(pid);
      return Err(spawn_error(format!("cannot start the exit watcher: {error}")));
    }
    let process = Self {
      pid,
      program,
      exit,
      stderr: StderrTail::capture(stderr),
    };
    Ok((process, Transport::new(stdout, stdin)))
  }

  pub fn pid(&self) -> u32 {
    self.pid
  }

  pub fn program(&self) -> &str {
    &self.program
  }

  pub fn stderr(&self) -> &StderrTail {
    &self.stderr
  }

  pub fn exit(&self) -> Option<ProcessExit> {
    lock(&self.exit.slot).exit
  }

  pub fn wait_exit(&self, timeout: Duration) -> Option<ProcessExit> {
    self.exit_handle().wait(timeout)
  }

  pub(crate) fn exit_handle(&self) -> ExitHandle {
    ExitHandle(Arc::clone(&self.exit))
  }

  pub fn on_exit(&self, listener: impl FnOnce(ProcessExit) + Send + 'static) {
    let mut slot = lock(&self.exit.slot);
    match slot.exit {
      Some(exit) => {
        drop(slot);
        listener(exit);
      }
      None => slot.listeners.push(Box::new(listener)),
    }
  }

  pub fn kill(&self) -> Option<ProcessExit> {
    if let Some(exit) = self.exit() {
      return Some(exit);
    }
    kill_group(self.pid);
    self.wait_exit(KILL_REAP_WAIT)
  }
}

impl Drop for ServerProcess {
  fn drop(&mut self) {
    if self.exit().is_none() {
      kill_group(self.pid);
    }
  }
}

#[cfg(unix)]
pub(crate) fn kill_group(pid: u32) {
  let group = format!("-{pid}");
  let killed = Command::new("/bin/kill")
    .args(["-s", "KILL", "--", &group])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  if let Err(error) = killed {
    tracing::warn!(%error, group, "failed to signal the process group");
  }
}

#[cfg(not(unix))]
pub(crate) fn kill_group(_pid: u32) {}

#[cfg(test)]
mod tests {
  use std::io::Write;
  use std::time::Instant;

  use super::*;

  fn shell(script: &str) -> ServerCommand {
    ServerCommand::new("sh").arg("-c").arg(script)
  }

  fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
  }

  #[test]
  fn should_report_the_exit_code_and_stderr_of_a_process() {
    let (process, _transport) = ServerProcess::spawn(&shell("echo boom >&2; exit 3")).unwrap();

    assert_eq!(process.wait_exit(Duration::from_secs(10)), Some(ProcessExit::Code(3)));
    assert!(process.stderr().wait_closed(Duration::from_secs(10)));
    assert_eq!(process.stderr().snapshot(), "boom\n");
  }

  #[test]
  fn should_keep_only_the_last_stderr_bytes() {
    let (process, _transport) = ServerProcess::spawn(&shell("head -c 100000 /dev/zero | tr '\\0' 'a' >&2; echo END >&2")).unwrap();

    assert!(process.stderr().wait_closed(Duration::from_secs(10)));
    let tail = process.stderr().snapshot();
    assert_eq!(tail.len(), STDERR_TAIL_BYTES);
    assert!(tail.ends_with("aaaEND\n"));
  }

  #[test]
  fn should_pipe_stdin_and_stdout_through_the_transport() {
    let (process, transport) = ServerProcess::spawn(&shell("read line; echo \"got $line\"")).unwrap();
    let (mut reader, mut writer) = transport.into_parts();
    writer.write_all(b"ping\n").unwrap();
    writer.flush().unwrap();
    let mut out = String::new();
    reader.read_to_string(&mut out).unwrap();

    assert_eq!(out, "got ping\n");
    assert_eq!(process.wait_exit(Duration::from_secs(10)), Some(ProcessExit::Code(0)));
  }

  #[test]
  fn should_kill_the_whole_process_group_on_drop() {
    let marker = tempfile::tempdir().unwrap();
    let pid_file = marker.path().join("grandchild.pid");
    let script = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
    let (process, _transport) = ServerProcess::spawn(&shell(&script)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_file.exists() && Instant::now() < deadline {
      thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(Duration::from_millis(50));
    let grandchild: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    let leader = process.pid();
    drop(process);
    thread::sleep(Duration::from_millis(300));

    assert!(!alive(grandchild), "grandchild {grandchild} survived");
    assert!(!alive(leader), "leader {leader} survived");
  }

  #[test]
  fn should_call_exit_listeners_registered_before_and_after_the_exit() {
    let (process, _transport) = ServerProcess::spawn(&shell("exit 0")).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let early = sender.clone();
    process.on_exit(move |exit| early.send(("early", exit)).unwrap());
    process.wait_exit(Duration::from_secs(10));
    process.on_exit(move |exit| sender.send(("late", exit)).unwrap());

    let mut seen: Vec<_> = receiver.iter().take(2).collect();
    seen.sort_by_key(|(name, _)| *name);
    assert_eq!(seen, vec![("early", ProcessExit::Code(0)), ("late", ProcessExit::Code(0))]);
  }

  #[test]
  fn should_report_a_missing_binary_as_a_spawn_failure() {
    let result = ServerProcess::spawn(&ServerCommand::new("/definitely/missing/atlas-lsp-binary"));

    assert!(matches!(result, Err(LspError::Spawn { ref program, .. }) if program == "/definitely/missing/atlas-lsp-binary"));
  }

  #[test]
  fn should_report_a_killed_process_as_signalled() {
    let (process, _transport) = ServerProcess::spawn(&shell("sleep 60")).unwrap();

    assert_eq!(process.kill(), Some(ProcessExit::Signal(9)));
  }
}
