use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const DRAIN_GRACE: Duration = Duration::from_secs(2);
pub(crate) const STDERR_TAIL_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub(crate) enum ProcessOutcome {
  Exited {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
  },
  TimedOut {
    stderr: String,
  },
  SpawnFailed(String),
}

pub(crate) fn run_with_timeout(mut command: Command, timeout: Duration) -> ProcessOutcome {
  command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
  #[cfg(unix)]
  std::os::unix::process::CommandExt::process_group(&mut command, 0);
  let mut child = match command.spawn() {
    Ok(child) => child,
    Err(error) => return ProcessOutcome::SpawnFailed(error.to_string()),
  };
  let stdout = drain_tail(child.stdout.take());
  let stderr = drain_tail(child.stderr.take());
  let deadline = Instant::now() + timeout;
  loop {
    match child.try_wait() {
      Ok(Some(status)) => {
        kill_group(child.id());
        return ProcessOutcome::Exited {
          success: status.success(),
          code: status.code(),
          stdout: collect(&stdout),
          stderr: collect(&stderr),
        };
      }
      Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
      Ok(None) => {
        terminate(&mut child);
        return ProcessOutcome::TimedOut { stderr: collect(&stderr) };
      }
      Err(error) => {
        terminate(&mut child);
        return ProcessOutcome::SpawnFailed(format!("waiting on the child failed: {error}"));
      }
    }
  }
}

fn terminate(child: &mut Child) {
  kill_group(child.id());
  if let Err(error) = child.kill() {
    tracing::debug!(%error, "child already gone when killing");
  }
  if let Err(error) = child.wait() {
    tracing::warn!(%error, "failed to reap killed child");
  }
}

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

fn drain_tail(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
  let (sender, receiver) = mpsc::channel();
  let Some(mut pipe) = pipe else {
    return receiver;
  };
  thread::spawn(move || {
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(read) = pipe.read(&mut chunk) {
      if read == 0 {
        break;
      }
      tail.extend_from_slice(&chunk[..read]);
      if tail.len() > STDERR_TAIL_BYTES * 2 {
        tail.drain(..tail.len() - STDERR_TAIL_BYTES);
      }
    }
    let start = tail.len().saturating_sub(STDERR_TAIL_BYTES);
    let _ = sender.send(String::from_utf8_lossy(&tail[start..]).into_owned());
  });
  receiver
}

fn collect(receiver: &mpsc::Receiver<String>) -> String {
  receiver.recv_timeout(DRAIN_GRACE).unwrap_or_default()
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
  fn should_capture_stdout_and_stderr_of_a_finished_process() {
    let outcome = run_with_timeout(shell("echo out; echo err >&2; exit 3"), Duration::from_secs(10));
    let ProcessOutcome::Exited { success, code, stdout, stderr } = outcome else {
      panic!("expected exit, got {outcome:?}");
    };
    assert!(!success);
    assert_eq!(code, Some(3));
    assert_eq!(stdout.trim(), "out");
    assert_eq!(stderr.trim(), "err");
  }

  #[test]
  fn should_keep_only_the_stderr_tail() {
    let outcome = run_with_timeout(shell("head -c 100000 /dev/zero | tr '\\0' 'a' >&2; echo END >&2"), Duration::from_secs(10));
    let ProcessOutcome::Exited { stderr, .. } = outcome else {
      panic!("expected exit, got {outcome:?}");
    };
    assert!(stderr.len() <= STDERR_TAIL_BYTES);
    assert!(stderr.trim_end().ends_with("aEND"));
  }

  #[test]
  fn should_time_out_and_kill_the_whole_process_group() {
    let marker = tempfile::tempdir().unwrap();
    let pid_file = marker.path().join("grandchild.pid");
    let script = format!("sleep 60 & echo $! > {}; wait", pid_file.display());
    let started = Instant::now();
    let outcome = run_with_timeout(shell(&script), Duration::from_millis(500));
    assert!(matches!(outcome, ProcessOutcome::TimedOut { .. }), "got {outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let grandchild: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    thread::sleep(Duration::from_millis(300));
    let alive = Command::new("kill").args(["-0", &grandchild.to_string()]).stderr(Stdio::null()).status().unwrap().success();
    assert!(!alive, "grandchild {grandchild} survived the group kill");
  }

  #[test]
  fn should_kill_children_that_outlive_a_normally_exiting_process() {
    let marker = tempfile::tempdir().unwrap();
    let pid_file = marker.path().join("child.pid");
    let script = format!("sleep 60 >/dev/null 2>&1 & echo $! > {}; exit 0", pid_file.display());
    let outcome = run_with_timeout(shell(&script), Duration::from_secs(10));
    assert!(matches!(outcome, ProcessOutcome::Exited { success: true, .. }), "got {outcome:?}");
    let child: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    thread::sleep(Duration::from_millis(300));
    let alive = Command::new("kill").args(["-0", &child.to_string()]).stderr(Stdio::null()).status().unwrap().success();
    assert!(!alive, "child {child} survived a normal exit");
  }

  #[test]
  fn should_report_a_missing_binary_as_a_spawn_failure() {
    let outcome = run_with_timeout(Command::new("atlas-scip-definitely-missing-binary"), Duration::from_secs(1));
    assert!(matches!(outcome, ProcessOutcome::SpawnFailed(_)));
  }
}
