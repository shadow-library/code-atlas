use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const DRAIN_GRACE: Duration = Duration::from_secs(2);
pub(crate) const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct ProcessOutput {
  pub status: ExitStatus,
  pub stdout: Vec<u8>,
  pub stderr: Vec<u8>,
}

pub(crate) enum ProcessError {
  Spawn(io::Error),
  Io(io::Error),
  TimedOut,
  OutputTooLarge { limit: usize },
}

struct Captured {
  bytes: Vec<u8>,
  truncated: bool,
}

fn drain(mut pipe: impl Read + Send + 'static, limit: usize) -> mpsc::Receiver<Captured> {
  let (sender, receiver) = mpsc::channel();
  thread::spawn(move || {
    let mut captured = Captured {
      bytes: Vec::new(),
      truncated: false,
    };
    let mut chunk = [0u8; 8192];
    loop {
      let read = match pipe.read(&mut chunk) {
        Ok(0) | Err(_) => break,
        Ok(read) => read,
      };
      let room = limit.saturating_sub(captured.bytes.len());
      captured.bytes.extend_from_slice(&chunk[..read.min(room)]);
      captured.truncated |= read > room;
    }
    let _ = sender.send(captured);
  });
  receiver
}

fn kill_tree(child: &mut Child) {
  #[cfg(unix)]
  {
    let _ = Command::new("kill")
      .args(["-s", "KILL", "--"])
      .arg(format!("-{}", child.id()))
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status();
  }
  let _ = child.kill();
  let _ = child.wait();
}

pub(crate) fn run_with_timeout(command: &mut Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<ProcessOutput, ProcessError> {
  command
    .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  #[cfg(unix)]
  std::os::unix::process::CommandExt::process_group(command, 0);
  let mut child = command.spawn().map_err(ProcessError::Spawn)?;
  if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
    let input = input.to_vec();
    thread::spawn(move || {
      let _ = pipe.write_all(&input);
    });
  }
  let stdout = child.stdout.take().map(|pipe| drain(pipe, MAX_CAPTURE_BYTES));
  let stderr = child.stderr.take().map(|pipe| drain(pipe, MAX_CAPTURE_BYTES));
  let deadline = Instant::now() + timeout;
  let status = loop {
    match child.try_wait() {
      Ok(Some(status)) => break status,
      Ok(None) if Instant::now() >= deadline => {
        kill_tree(&mut child);
        return Err(ProcessError::TimedOut);
      }
      Ok(None) => thread::sleep(POLL_INTERVAL),
      Err(error) => {
        kill_tree(&mut child);
        return Err(ProcessError::Io(error));
      }
    }
  };
  let collect = |receiver: Option<mpsc::Receiver<Captured>>| match receiver {
    Some(receiver) => receiver.recv_timeout(DRAIN_GRACE).map_err(|_| ProcessError::TimedOut),
    None => Ok(Captured {
      bytes: Vec::new(),
      truncated: false,
    }),
  };
  let (stdout, stderr) = (collect(stdout)?, collect(stderr)?);
  if stdout.truncated || stderr.truncated {
    return Err(ProcessError::OutputTooLarge { limit: MAX_CAPTURE_BYTES });
  }
  Ok(ProcessOutput {
    status,
    stdout: stdout.bytes,
    stderr: stderr.bytes,
  })
}
