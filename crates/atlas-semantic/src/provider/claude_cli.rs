use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use atlas_core::{Availability, LlmError, LlmProvider, LlmRequest, LlmResponse, TokenUsage};
use serde_json::Value;

use super::process::{ProcessError, ProcessOutput, run_with_timeout};

const PROVIDER_ID: &str = "claude-cli";
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const STDERR_EXCERPT_CHARS: usize = 400;

#[derive(Debug, Clone)]
pub struct ClaudeCliProvider {
  binary: PathBuf,
  model: String,
}

impl ClaudeCliProvider {
  pub fn new(model: impl Into<String>) -> Self {
    Self {
      binary: PathBuf::from("claude"),
      model: model.into(),
    }
  }

  pub fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
    self.binary = binary.into();
    self
  }

  fn command(&self, request: &LlmRequest) -> Command {
    let mut command = Command::new(&self.binary);
    command.args([
      "-p",
      "--output-format",
      "json",
      "--no-session-persistence",
      "--disable-slash-commands",
      "--strict-mcp-config",
    ]);
    command.args(["--tools", "", "--setting-sources", ""]);
    command.arg("--system-prompt").arg(&request.system);
    command.arg("--model").arg(&self.model);
    if let Some(schema) = &request.json_schema {
      command.arg("--json-schema").arg(schema.to_string());
    }
    command.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
    command.env("CLAUDE_CODE_MAX_OUTPUT_TOKENS", request.max_output_tokens.to_string());
    command
  }
}

impl LlmProvider for ClaudeCliProvider {
  fn id(&self) -> &str {
    PROVIDER_ID
  }

  fn model(&self) -> &str {
    &self.model
  }

  fn availability(&self) -> Availability {
    let outcome = run_with_timeout(Command::new(&self.binary).arg("--version"), None, VERSION_TIMEOUT);
    match outcome {
      Ok(output) if output.status.success() => Availability::Available,
      Ok(output) => Availability::Unavailable {
        reason: format!("`{} --version` exited with {}", self.binary.display(), output.status),
      },
      Err(ProcessError::Spawn(error)) => Availability::Unavailable {
        reason: format!("cannot run `{}`: {error}", self.binary.display()),
      },
      Err(ProcessError::TimedOut) => Availability::Unavailable {
        reason: format!("`{} --version` timed out", self.binary.display()),
      },
      Err(ProcessError::Io(error)) => Availability::Unavailable {
        reason: format!("`{} --version` failed: {error}", self.binary.display()),
      },
      Err(ProcessError::OutputTooLarge { .. }) => Availability::Unavailable {
        reason: format!("`{} --version` printed too much output", self.binary.display()),
      },
    }
  }

  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
    let workdir = tempfile::tempdir().map_err(|error| LlmError::Provider(format!("cannot create a scratch directory: {error}")))?;
    let mut command = self.command(request);
    command.current_dir(workdir.path());
    let output = match run_with_timeout(&mut command, Some(request.prompt.as_bytes()), request.timeout) {
      Ok(output) => output,
      Err(ProcessError::TimedOut) => return Err(LlmError::Timeout(request.timeout)),
      Err(ProcessError::OutputTooLarge { limit }) => return Err(LlmError::Provider(format!("claude output exceeded the {limit} byte capture limit"))),
      Err(ProcessError::Spawn(error)) if error.kind() == std::io::ErrorKind::NotFound => {
        return Err(LlmError::Unavailable(format!("`{}` was not found", self.binary.display())));
      }
      Err(ProcessError::Spawn(error) | ProcessError::Io(error)) => return Err(LlmError::Provider(format!("claude process failed: {error}"))),
    };
    self.parse(&output)
  }
}

impl ClaudeCliProvider {
  fn parse(&self, output: &ProcessOutput) -> Result<LlmResponse, LlmError> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    if !output.status.success() {
      let detail = document
        .as_ref()
        .and_then(|document| document.get("result"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
          let stderr = String::from_utf8_lossy(&output.stderr);
          stderr.trim().chars().take(STDERR_EXCERPT_CHARS).collect()
        });
      return Err(LlmError::Provider(format!("claude exited with {}: {detail}", output.status)));
    }
    let document = document.ok_or_else(|| LlmError::InvalidResponse("claude did not print a JSON document".to_owned()))?;
    let result = document.get("result").and_then(Value::as_str).unwrap_or_default();
    if document.get("is_error").and_then(Value::as_bool) == Some(true) {
      return Err(LlmError::Provider(format!("claude reported an error: {result}")));
    }
    let text = match document.get("structured_output") {
      Some(structured) if !structured.is_null() => structured.to_string(),
      _ => result.to_owned(),
    };
    if text.trim().is_empty() {
      return Err(LlmError::InvalidResponse("claude returned an empty result".to_owned()));
    }
    let tokens = |field: &str| document.pointer(&format!("/usage/{field}")).and_then(Value::as_u64).unwrap_or(0);
    let model = document
      .get("modelUsage")
      .and_then(Value::as_object)
      .and_then(|models| models.keys().next().cloned())
      .unwrap_or_else(|| self.model().to_owned());
    Ok(LlmResponse {
      text,
      model,
      usage: TokenUsage {
        input: tokens("input_tokens"),
        output: tokens("output_tokens"),
        cache_read: tokens("cache_read_input_tokens"),
        cache_write: tokens("cache_creation_input_tokens"),
      },
    })
  }
}

#[cfg(all(test, unix))]
mod tests {
  use std::fs;
  use std::os::unix::fs::PermissionsExt;
  use std::path::Path;

  use super::*;

  fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("claude");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
  }

  fn request(timeout: Duration) -> LlmRequest {
    LlmRequest {
      system: "sys".into(),
      prompt: "hello".into(),
      max_output_tokens: 256,
      json_schema: Some(serde_json::json!({"type":"object"})),
      timeout,
    }
  }

  #[test]
  fn should_parse_text_usage_and_model_from_the_json_result() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"cat >/dev/null; echo '{"type":"result","is_error":false,"result":"{\"ok\":true}","usage":{"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":2},"modelUsage":{"claude-haiku-4-5-20251001":{}}}'"#;
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), body));
    let response = provider.complete(&request(Duration::from_secs(10))).unwrap();
    assert_eq!(response.text, r#"{"ok":true}"#);
    assert_eq!(response.model, "claude-haiku-4-5-20251001");
    assert_eq!(
      response.usage,
      TokenUsage {
        input: 11,
        output: 7,
        cache_read: 3,
        cache_write: 2
      }
    );
  }

  #[test]
  fn should_prefer_structured_output_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"cat >/dev/null; echo '{"is_error":false,"result":"","structured_output":{"verdict":"unrelated"},"usage":{}}'"#;
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), body));
    let response = provider.complete(&request(Duration::from_secs(10))).unwrap();
    assert_eq!(response.text, r#"{"verdict":"unrelated"}"#);
    assert_eq!(response.model, "haiku");
  }

  #[test]
  fn should_pass_the_prompt_on_stdin_and_lock_the_session_down() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let body = format!(
      r#"printf '%s\n' "$@" > '{log}'; pwd >> '{log}'; cat >> '{log}'; echo '{{"is_error":false,"result":"x","usage":{{}}}}'"#,
      log = log.display()
    );
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), &body));
    provider.complete(&request(Duration::from_secs(10))).unwrap();
    let logged = fs::read_to_string(log).unwrap();
    let lines: Vec<&str> = logged.lines().collect();
    assert_eq!(
      &lines[..16],
      [
        "-p",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--system-prompt",
        "sys",
        "--model",
        "haiku",
        "--json-schema",
        r#"{"type":"object"}"#
      ]
    );
    assert!(!lines[16].contains("code-atlas"));
    assert_eq!(lines[17], "hello");
  }

  #[test]
  fn should_kill_the_process_and_report_a_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), "exec sleep 30"));
    let started = std::time::Instant::now();
    let error = provider.complete(&request(Duration::from_millis(200))).unwrap_err();
    assert_eq!(error, LlmError::Timeout(Duration::from_millis(200)));
    assert!(started.elapsed() < Duration::from_secs(10));
  }

  #[test]
  fn should_kill_the_whole_process_group_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let body = format!("sleep 30 &\necho $! > '{}'\nwait", pid_file.display());
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), &body));
    assert_eq!(provider.complete(&request(Duration::from_secs(3))).unwrap_err(), LlmError::Timeout(Duration::from_secs(3)));
    let pid = fs::read_to_string(&pid_file).unwrap().trim().to_owned();
    let alive = || Command::new("kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().unwrap().success();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while alive() && std::time::Instant::now() < deadline {
      std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive());
  }

  #[test]
  fn should_fail_explicitly_when_output_exceeds_the_capture_limit() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), "cat >/dev/null; head -c 5000000 /dev/zero"));
    let error = provider.complete(&request(Duration::from_secs(20))).unwrap_err();
    assert_eq!(error, LlmError::Provider("claude output exceeded the 4194304 byte capture limit".to_owned()));
  }

  #[test]
  fn should_surface_cli_errors_as_provider_errors() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), r#"cat >/dev/null; echo '{"is_error":true,"result":"rate limit hit"}'"#));
    assert_eq!(
      provider.complete(&request(Duration::from_secs(10))).unwrap_err(),
      LlmError::Provider("claude reported an error: rate limit hit".to_owned())
    );
  }

  #[test]
  fn should_report_availability_from_the_version_probe() {
    let dir = tempfile::tempdir().unwrap();
    let working = ClaudeCliProvider::new("haiku").with_binary(script(dir.path(), "echo 1.0.0"));
    assert_eq!(working.availability(), Availability::Available);
    let missing = ClaudeCliProvider::new("haiku").with_binary(dir.path().join("absent"));
    assert!(matches!(missing.availability(), Availability::Unavailable { .. }));
    assert!(matches!(missing.complete(&request(Duration::from_secs(1))), Err(LlmError::Unavailable(_))));
  }
}
