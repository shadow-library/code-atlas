use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const DRAIN_GRACE: Duration = Duration::from_secs(5);
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessExit {
  Exited { code: Option<i32> },
  TimedOut,
  SpawnFailed { reason: String },
}

impl ProcessExit {
  pub fn succeeded(&self) -> bool {
    matches!(self, Self::Exited { code: Some(0) })
  }
}

#[derive(Debug, Clone)]
pub struct CapturedOutput {
  pub exit: ProcessExit,
  pub stdout: String,
  pub stderr: String,
  pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct StreamedOutput {
  pub exit: ProcessExit,
  pub line_offsets_ms: Vec<u64>,
  pub duration: Duration,
}

pub fn run_captured(mut command: Command, stdin: Option<&[u8]>, timeout: Duration) -> CapturedOutput {
  command
    .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  isolate_group(&mut command);
  let started = Instant::now();
  let mut child = match command.spawn() {
    Ok(child) => child,
    Err(error) => {
      return CapturedOutput {
        exit: ProcessExit::SpawnFailed { reason: error.to_string() },
        stdout: String::new(),
        stderr: String::new(),
        duration: started.elapsed(),
      };
    }
  };
  if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
    let input = input.to_vec();
    thread::spawn(move || {
      let _ = pipe.write_all(&input);
    });
  }
  let stdout = drain(child.stdout.take());
  let stderr = drain(child.stderr.take());
  let exit = wait(&mut child, started + timeout);
  CapturedOutput {
    exit,
    stdout: stdout.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
    stderr: stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
    duration: started.elapsed(),
  }
}

pub fn run_streaming(mut command: Command, timeout: Duration, stdout_path: &Path, stderr_path: &Path) -> std::io::Result<StreamedOutput> {
  let stdout_file = File::create(stdout_path)?;
  let stderr_file = File::create(stderr_path)?;
  command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::from(stderr_file));
  isolate_group(&mut command);
  let started = Instant::now();
  let mut child = match command.spawn() {
    Ok(child) => child,
    Err(error) => {
      return Ok(StreamedOutput {
        exit: ProcessExit::SpawnFailed { reason: error.to_string() },
        line_offsets_ms: Vec::new(),
        duration: started.elapsed(),
      });
    }
  };
  let (sender, receiver) = mpsc::channel();
  if let Some(pipe) = child.stdout.take() {
    thread::spawn(move || {
      let _ = sender.send(copy_lines(pipe, stdout_file, started));
    });
  }
  let exit = wait(&mut child, started + timeout);
  let line_offsets_ms = match receiver.recv_timeout(DRAIN_GRACE) {
    Ok(Ok(offsets)) => offsets,
    Ok(Err(error)) => return Err(error),
    Err(_) => Vec::new(),
  };
  Ok(StreamedOutput {
    exit,
    line_offsets_ms,
    duration: started.elapsed(),
  })
}

fn copy_lines(pipe: impl Read, mut sink: File, started: Instant) -> std::io::Result<Vec<u64>> {
  let mut reader = BufReader::new(pipe);
  let mut offsets = Vec::new();
  let mut line = Vec::new();
  loop {
    line.clear();
    if reader.read_until(b'\n', &mut line)? == 0 {
      break;
    }
    offsets.push(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    sink.write_all(&line)?;
    if !line.ends_with(b"\n") {
      sink.write_all(b"\n")?;
    }
  }
  sink.flush()?;
  Ok(offsets)
}

fn wait(child: &mut Child, deadline: Instant) -> ProcessExit {
  loop {
    match child.try_wait() {
      Ok(Some(status)) => {
        kill_group(child.id());
        return ProcessExit::Exited { code: status.code() };
      }
      Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
      Ok(None) => {
        terminate(child);
        return ProcessExit::TimedOut;
      }
      Err(error) => {
        terminate(child);
        return ProcessExit::SpawnFailed {
          reason: format!("waiting on the child failed: {error}"),
        };
      }
    }
  }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
  let (sender, receiver) = mpsc::channel();
  let Some(mut pipe) = pipe else {
    return receiver;
  };
  thread::spawn(move || {
    let mut captured: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(read) = pipe.read(&mut chunk) {
      if read == 0 {
        break;
      }
      let room = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
      captured.extend_from_slice(&chunk[..read.min(room)]);
    }
    let _ = sender.send(String::from_utf8_lossy(&captured).into_owned());
  });
  receiver
}

fn terminate(child: &mut Child) {
  kill_group(child.id());
  if let Err(error) = child.kill() {
    tracing::debug!(%error, "child already gone when killing");
  }
  if let Err(error) = child.wait() {
    tracing::warn!(%error, "failed to reap a killed child");
  }
}

#[cfg(unix)]
fn isolate_group(command: &mut Command) {
  std::os::unix::process::CommandExt::process_group(command, 0);
}

#[cfg(not(unix))]
fn isolate_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_group(pid: u32) {
  let group = format!("-{pid}");
  let killed = Command::new("kill")
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
fn kill_group(_pid: u32) {}

pub fn tail(text: &str, max_bytes: usize) -> String {
  if text.len() <= max_bytes {
    return text.to_owned();
  }
  let mut start = text.len() - max_bytes;
  while !text.is_char_boundary(start) {
    start += 1;
  }
  text[start..].to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
  }

  #[test]
  fn should_capture_output_and_exit_code() {
    let output = run_captured(shell("echo out; echo err >&2; exit 3"), None, Duration::from_secs(10));
    assert_eq!(output.exit, ProcessExit::Exited { code: Some(3) });
    assert_eq!(output.stdout, "out\n");
    assert_eq!(output.stderr, "err\n");
  }

  #[test]
  fn should_feed_stdin_to_the_child() {
    let output = run_captured(Command::new("cat"), Some(b"piped"), Duration::from_secs(10));
    assert_eq!(output.stdout, "piped");
  }

  #[test]
  fn should_stream_lines_to_a_file_with_one_offset_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.jsonl");
    let err = dir.path().join("err.log");
    let output = run_streaming(shell("printf 'a\\nb\\nc'; echo boom >&2"), Duration::from_secs(10), &out, &err).unwrap();
    assert!(output.exit.succeeded());
    assert_eq!(output.line_offsets_ms.len(), 3);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "a\nb\nc\n");
    assert_eq!(std::fs::read_to_string(&err).unwrap(), "boom\n");
  }

  #[test]
  fn should_kill_the_process_group_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = format!("sleep 60 & echo $! > {}; wait", pid_file.display());
    let output = run_captured(shell(&script), None, Duration::from_millis(500));
    assert_eq!(output.exit, ProcessExit::TimedOut);
    let grandchild = std::fs::read_to_string(&pid_file).unwrap().trim().to_owned();
    thread::sleep(Duration::from_millis(300));
    let alive = Command::new("kill").args(["-0", &grandchild]).stderr(Stdio::null()).status().unwrap().success();
    assert!(!alive, "grandchild {grandchild} survived the group kill");
  }

  #[test]
  fn should_report_a_missing_binary_as_a_spawn_failure() {
    let output = run_captured(Command::new("atlas-eval-definitely-missing-binary"), None, Duration::from_secs(1));
    assert!(matches!(output.exit, ProcessExit::SpawnFailed { .. }));
  }

  #[test]
  fn should_keep_the_tail_on_a_char_boundary() {
    assert_eq!(tail("abcdef", 3), "def");
    assert_eq!(tail("aé", 1), "");
    assert_eq!(tail("short", 10), "short");
  }
}
