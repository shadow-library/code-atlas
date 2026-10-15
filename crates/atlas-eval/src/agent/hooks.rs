use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::atlas_cli::AtlasCli;
use super::error::{AgentError, AgentResult};
use super::transcript::{HookPhase, Transcript};

pub const ATLAS_HOME_TOKEN: &str = "<atlas-home>";
pub const ATLAS_BIN_TOKEN: &str = "<atlas-bin>";
pub const USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";
pub const PRE_TOOL_USE: &str = "PreToolUse";
const SUCCESS_OUTCOME: &str = "success";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMetrics {
  pub started: BTreeMap<String, u64>,
  pub finished: BTreeMap<String, u64>,
  #[serde(default)]
  pub with_stderr: BTreeMap<String, u64>,
  pub seed_injections: u64,
  pub reminder_injections: u64,
}

impl HookMetrics {
  pub fn ran(&self, event: &str) -> bool {
    self.started.contains_key(event) || self.finished.contains_key(event)
  }

  pub fn finished(&self, event: &str) -> u64 {
    self.finished.get(event).copied().unwrap_or(0)
  }

  pub fn injections(&self, event: &str) -> u64 {
    match event {
      USER_PROMPT_SUBMIT => self.seed_injections,
      PRE_TOOL_USE => self.reminder_injections,
      _ => 0,
    }
  }

  pub fn silent(&self, event: &str) -> u64 {
    self.finished(event).saturating_sub(self.injections(event))
  }

  pub fn only_failed(&self, event: &str) -> bool {
    let finished = self.finished(event);
    finished > 0 && self.with_stderr.get(event).copied().unwrap_or(0) == finished && self.injections(event) == 0
  }
}

pub fn hook_metrics(transcript: &Transcript) -> HookMetrics {
  let mut metrics = HookMetrics::default();
  for hook in &transcript.hooks {
    let HookPhase::Finished {
      outcome,
      exit_code,
      stdout,
      stderr,
    } = &hook.phase
    else {
      *metrics.started.entry(hook.event.clone()).or_default() += 1;
      continue;
    };
    *metrics.finished.entry(hook.event.clone()).or_default() += 1;
    if !stderr.trim().is_empty() {
      *metrics.with_stderr.entry(hook.event.clone()).or_default() += 1;
    }
    let succeeded = exit_code.map_or(outcome.as_deref() == Some(SUCCESS_OUTCOME), |code| code == 0);
    if !succeeded || !injects_context(&hook.event, stdout) {
      continue;
    }
    match hook.event.as_str() {
      USER_PROMPT_SUBMIT => metrics.seed_injections += 1,
      PRE_TOOL_USE => metrics.reminder_injections += 1,
      _ => {}
    }
  }
  metrics
}

fn injects_context(event: &str, stdout: &str) -> bool {
  let stdout = stdout.trim();
  match serde_json::from_str::<Value>(stdout) {
    Ok(Value::Object(output)) => output
      .get("hookSpecificOutput")
      .and_then(|specific| specific.get("additionalContext"))
      .and_then(Value::as_str)
      .is_some_and(|context| !context.trim().is_empty()),
    _ => event == USER_PROMPT_SUBMIT && !stdout.is_empty(),
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasHooksRecord {
  pub blake3: String,
  pub groups: BTreeMap<String, usize>,
}

impl AtlasHooksRecord {
  pub fn installs(&self, event: &str) -> bool {
    self.groups.get(event).is_some_and(|groups| *groups > 0)
  }

  pub fn describe(&self) -> String {
    let events: Vec<String> = self.groups.iter().map(|(event, groups)| format!("{event} ({groups} group(s))")).collect();
    format!("blake3 {}: {}", self.blake3, events.join(", "))
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AtlasHooks {
  groups: Map<String, Value>,
  record: AtlasHooksRecord,
}

impl AtlasHooks {
  pub fn render(atlas: &AtlasCli, cwd: &Path) -> AgentResult<Self> {
    let output = atlas.agents_hooks(cwd).map_err(|error| AgentError::AtlasHooksUnavailable { reason: error.to_string() })?;
    Self::parse(&output.stdout, &atlas.binary().display().to_string(), &atlas.home().display().to_string()).map_err(|reason| AgentError::AtlasHooksUnavailable {
      reason: format!("`catl agents hooks --json` printed {reason}"),
    })
  }

  pub fn parse(stdout: &str, binary: &str, home: &str) -> Result<Self, String> {
    let document: Value = serde_json::from_str(stdout.trim()).map_err(|error| format!("no JSON ({error})"))?;
    let Value::Object(groups) = document else {
      return Err("JSON that is not an object of hook events".to_owned());
    };
    let mut counts = BTreeMap::new();
    for (event, entries) in &groups {
      let Some(entries) = entries.as_array().filter(|entries| entries.iter().all(Value::is_object)) else {
        return Err(format!("`{event}` without an array of hook groups"));
      };
      counts.insert(event.clone(), entries.len());
    }
    if counts.values().all(|groups| *groups == 0) {
      return Err("no hook group, so [agents.claude] hooks of the session home installs none".to_owned());
    }
    let normalized = canonical(&normalize(&Value::Object(groups.clone()), home, binary));
    Ok(Self {
      record: AtlasHooksRecord {
        blake3: blake3::hash(normalized.to_string().as_bytes()).to_hex().to_string(),
        groups: counts,
      },
      groups,
    })
  }

  pub fn record(&self) -> &AtlasHooksRecord {
    &self.record
  }

  pub fn settings(&self) -> Value {
    json!({ "hooks": Value::Object(self.groups.clone()) })
  }
}

fn normalize(value: &Value, home: &str, binary: &str) -> Value {
  match value {
    Value::String(text) => Value::String(masked(&masked(text, home, ATLAS_HOME_TOKEN), binary, ATLAS_BIN_TOKEN)),
    Value::Array(items) => Value::Array(items.iter().map(|item| normalize(item, home, binary)).collect()),
    Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), normalize(value, home, binary))).collect()),
    scalar => scalar.clone(),
  }
}

fn masked(text: &str, needle: &str, token: &str) -> String {
  if needle.is_empty() {
    return text.to_owned();
  }
  text.replace(needle, token)
}

fn canonical(value: &Value) -> Value {
  match value {
    Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
    Value::Object(fields) => {
      let sorted: BTreeMap<&String, Value> = fields.iter().map(|(key, value)| (key, canonical(value))).collect();
      Value::Object(sorted.into_iter().map(|(key, value)| (key.clone(), value)).collect())
    }
    scalar => scalar.clone(),
  }
}

#[cfg(test)]
mod tests {
  use super::super::transcript::HookEvent;
  use super::*;

  const SYNTHETIC: &str = include_str!("../../tests/fixtures/hooks-transcript.jsonl");

  #[test]
  fn should_count_the_injections_of_a_recorded_hooks_transcript() {
    let transcript = Transcript::parse(SYNTHETIC, &[]);
    let metrics = hook_metrics(&transcript);
    assert_eq!(
      metrics,
      HookMetrics {
        started: BTreeMap::from([("PreToolUse".to_owned(), 2), ("UserPromptSubmit".to_owned(), 1)]),
        finished: BTreeMap::from([("PreToolUse".to_owned(), 2), ("UserPromptSubmit".to_owned(), 1)]),
        with_stderr: BTreeMap::new(),
        seed_injections: 1,
        reminder_injections: 1,
      }
    );
    assert_eq!(transcript.hooks[1].name, "UserPromptSubmit");
    assert!(metrics.ran(USER_PROMPT_SUBMIT) && !metrics.ran("SessionStart"));
    assert_eq!((metrics.silent(USER_PROMPT_SUBMIT), metrics.silent(PRE_TOOL_USE)), (0, 1));
    assert!(!metrics.only_failed(USER_PROMPT_SUBMIT));
    assert_eq!(transcript.tool_calls.len(), 2);
  }

  fn finished(event: &str, outcome: Option<&str>, exit_code: Option<i64>, stdout: &str) -> HookEvent {
    failed(event, outcome, exit_code, stdout, "")
  }

  fn failed(event: &str, outcome: Option<&str>, exit_code: Option<i64>, stdout: &str, stderr: &str) -> HookEvent {
    HookEvent {
      event: event.to_owned(),
      name: event.to_owned(),
      phase: HookPhase::Finished {
        outcome: outcome.map(str::to_owned),
        exit_code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
      },
    }
  }

  #[test]
  fn should_tell_a_hook_that_only_failed_from_one_that_stayed_silent() {
    let broken = Transcript {
      hooks: vec![failed(USER_PROMPT_SUBMIT, Some("success"), Some(0), "", "sh: /opt/catl: not found\n")],
      ..Transcript::default()
    };
    let silent = Transcript {
      hooks: vec![finished(USER_PROMPT_SUBMIT, Some("success"), Some(0), "")],
      ..Transcript::default()
    };
    let warned_but_seeded = Transcript {
      hooks: vec![failed(USER_PROMPT_SUBMIT, Some("success"), Some(0), "pack", "warning: x")],
      ..Transcript::default()
    };
    let broken = hook_metrics(&broken);
    assert_eq!((broken.with_stderr.get(USER_PROMPT_SUBMIT), broken.silent(USER_PROMPT_SUBMIT)), (Some(&1), 1));
    assert!(broken.only_failed(USER_PROMPT_SUBMIT));
    assert!(!hook_metrics(&silent).only_failed(USER_PROMPT_SUBMIT));
    assert!(!hook_metrics(&warned_but_seeded).only_failed(USER_PROMPT_SUBMIT));
  }

  #[test]
  fn should_count_only_output_claude_code_adds_to_the_context() {
    let context = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"use the index"}}"#;
    let transcript = Transcript {
      hooks: vec![
        finished(PRE_TOOL_USE, Some("success"), Some(0), context),
        finished(PRE_TOOL_USE, Some("error"), Some(1), context),
        finished(PRE_TOOL_USE, Some("success"), None, context),
        finished(PRE_TOOL_USE, Some("success"), Some(0), "plain text is only logged for PreToolUse"),
        finished(PRE_TOOL_USE, Some("success"), Some(0), r#"{"hookSpecificOutput":{"additionalContext":"  "}}"#),
        finished(USER_PROMPT_SUBMIT, None, Some(0), "plain text is context for UserPromptSubmit"),
        finished(USER_PROMPT_SUBMIT, Some("success"), Some(0), "\n"),
        finished(USER_PROMPT_SUBMIT, Some("cancelled"), None, "late"),
        finished("SessionStart", Some("success"), Some(0), "context of another event"),
      ],
      ..Transcript::default()
    };
    let metrics = hook_metrics(&transcript);
    assert_eq!((metrics.seed_injections, metrics.reminder_injections), (1, 2));
    assert_eq!(metrics.finished.get(USER_PROMPT_SUBMIT), Some(&3));
    assert!(metrics.started.is_empty());
  }

  fn printed(home: &str) -> String {
    json!({
      "PreToolUse": [{"matcher": "Grep|Glob|Bash", "hooks": [{"type": "command", "command": format!("CODE_ATLAS_HOME={home} /opt/catl hook remind || true"), "timeout": 10}]}],
      "UserPromptSubmit": [{"hooks": [{"type": "command", "command": format!("CODE_ATLAS_HOME={home} /opt/catl hook seed || true"), "timeout": 10}]}]
    })
    .to_string()
  }

  #[test]
  fn should_digest_the_groups_without_the_session_home_or_the_binary_path() {
    let first = AtlasHooks::parse(&printed("/tmp/ae-1/atlas-home"), "/opt/catl", "/tmp/ae-1/atlas-home").unwrap();
    let second = AtlasHooks::parse(&printed("/tmp/ae-2/atlas-home"), "/opt/catl", "/tmp/ae-2/atlas-home").unwrap();
    assert_eq!(first.record(), second.record());
    assert_eq!(first.record().groups, BTreeMap::from([("PreToolUse".to_owned(), 1), ("UserPromptSubmit".to_owned(), 1)]));
    assert!(!serde_json::to_string(first.record()).unwrap().contains("/tmp/"));
    assert_ne!(first.settings(), second.settings());
    assert_eq!(
      first.settings()["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
      "CODE_ATLAS_HOME=/tmp/ae-1/atlas-home /opt/catl hook seed || true"
    );
  }

  #[test]
  fn should_digest_a_changed_group_differently() {
    let seed_only = json!({"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "CODE_ATLAS_HOME=/h /opt/catl hook seed || true", "timeout": 10}]}]});
    let both = AtlasHooks::parse(&printed("/h"), "/opt/catl", "/h").unwrap();
    let one = AtlasHooks::parse(&seed_only.to_string(), "/opt/catl", "/h").unwrap();
    assert_ne!(both.record().blake3, one.record().blake3);
    assert!(one.record().installs(USER_PROMPT_SUBMIT) && !one.record().installs(PRE_TOOL_USE));
  }

  #[test]
  fn should_refuse_output_that_installs_nothing_or_is_not_hook_groups() {
    assert_eq!(
      AtlasHooks::parse("{}", "catl", "/h").unwrap_err(),
      "no hook group, so [agents.claude] hooks of the session home installs none"
    );
    assert_eq!(AtlasHooks::parse("[]", "catl", "/h").unwrap_err(), "JSON that is not an object of hook events");
    assert_eq!(
      AtlasHooks::parse("{\"PreToolUse\": 3}", "catl", "/h").unwrap_err(),
      "`PreToolUse` without an array of hook groups"
    );
    assert!(AtlasHooks::parse("PreToolUse: catl hook remind", "catl", "/h").unwrap_err().starts_with("no JSON"));
  }

  #[test]
  fn should_hash_independently_of_key_order() {
    let reordered = r#"{"UserPromptSubmit":[{"hooks":[{"timeout":10,"command":"CODE_ATLAS_HOME=/h /opt/catl hook seed || true","type":"command"}]}],"PreToolUse":[{"hooks":[{"command":"CODE_ATLAS_HOME=/h /opt/catl hook remind || true","timeout":10,"type":"command"}],"matcher":"Grep|Glob|Bash"}]}"#;
    let ordered = AtlasHooks::parse(&printed("/h"), "/opt/catl", "/h").unwrap();
    assert_eq!(AtlasHooks::parse(reordered, "/opt/catl", "/h").unwrap().record(), ordered.record());
  }
}
