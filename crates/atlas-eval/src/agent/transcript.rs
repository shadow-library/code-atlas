use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
  pub input: u64,
  pub output: u64,
  pub cache_read: u64,
  pub cache_creation: u64,
}

impl TokenUsage {
  fn from_api(usage: &Value) -> Self {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Self {
      input: field("input_tokens"),
      output: field("output_tokens"),
      cache_read: field("cache_read_input_tokens"),
      cache_creation: field("cache_creation_input_tokens"),
    }
  }

  fn from_model_usage(usage: &Value) -> Self {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Self {
      input: field("inputTokens"),
      output: field("outputTokens"),
      cache_read: field("cacheReadInputTokens"),
      cache_creation: field("cacheCreationInputTokens"),
    }
  }

  pub fn total(&self) -> u64 {
    self.input + self.output + self.cache_read + self.cache_creation
  }

  pub fn add(&self, other: &Self) -> Self {
    Self {
      input: self.input + other.input,
      output: self.output + other.output,
      cache_read: self.cache_read + other.cache_read,
      cache_creation: self.cache_creation + other.cache_creation,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
  pub name: String,
  pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitInfo {
  pub model: Option<String>,
  pub cwd: Option<String>,
  pub permission_mode: Option<String>,
  pub claude_code_version: Option<String>,
  pub api_key_source: Option<String>,
  pub tools: Vec<String>,
  pub mcp_servers: Vec<McpServerStatus>,
  pub slash_commands: Vec<String>,
  pub skills: Vec<String>,
  pub plugins: Vec<String>,
  pub agents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantTurn {
  pub message_id: String,
  pub model: Option<String>,
  pub usage: TokenUsage,
  pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Completion {
  #[default]
  Unreported,
  Finished {
    exit_note: Option<String>,
  },
  Backgrounded {
    cwd_hint: Option<String>,
  },
}

impl Completion {
  fn from_tool_use_result(report: &Value) -> Self {
    let text = |field: &str| report.get(field).and_then(Value::as_str).map(str::to_owned);
    if !report.is_object() {
      return Self::Unreported;
    }
    if report.get("backgroundTaskId").is_some() {
      return Self::Backgrounded {
        cwd_hint: text("backgroundCwdHint"),
      };
    }
    Self::Finished {
      exit_note: text("returnCodeInterpretation"),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
  pub text: String,
  pub is_error: bool,
  #[serde(default)]
  pub completion: Completion,
}

impl ToolResult {
  pub fn bytes(&self) -> u64 {
    self.text.len() as u64
  }

  pub fn lines(&self) -> u64 {
    self.text.lines().count() as u64
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
  pub id: String,
  pub name: String,
  pub input: Value,
  pub elapsed_ms: Option<u64>,
  pub result: Option<ToolResult>,
  #[serde(default)]
  pub turn: u32,
}

impl ToolCall {
  pub fn input_str(&self, field: &str) -> Option<&str> {
    self.input.get(field).and_then(Value::as_str)
  }

  pub fn result_bytes(&self) -> u64 {
    self.result.as_ref().map_or(0, ToolResult::bytes)
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
  pub usage: TokenUsage,
  pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultInfo {
  pub subtype: Option<String>,
  pub is_error: bool,
  pub duration_ms: Option<u64>,
  pub num_turns: Option<u64>,
  pub total_cost_usd: Option<f64>,
  pub usage: TokenUsage,
  pub model_usage: BTreeMap<String, ModelUsage>,
  pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum HookPhase {
  Started,
  Finished {
    outcome: Option<String>,
    exit_code: Option<i64>,
    stdout: String,
    #[serde(default)]
    stderr: String,
  },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
  pub event: String,
  pub name: String,
  #[serde(flatten)]
  pub phase: HookPhase,
}

impl HookPhase {
  fn finished(event: &Value) -> Self {
    let text = |field: &str| event.get(field).and_then(Value::as_str).map(str::to_owned);
    Self::Finished {
      outcome: text("outcome"),
      exit_code: event.get("exit_code").and_then(Value::as_i64),
      stdout: text("stdout").or_else(|| text("output")).unwrap_or_default(),
      stderr: text("stderr").unwrap_or_default(),
    }
  }
}

impl HookEvent {
  fn parse(event: &Value, phase: HookPhase) -> Self {
    let text = |field: &str| event.get(field).and_then(Value::as_str).unwrap_or_default().to_owned();
    Self {
      event: text("hook_event"),
      name: text("hook_name"),
      phase,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
  pub init: Option<InitInfo>,
  pub turns: Vec<AssistantTurn>,
  pub tool_calls: Vec<ToolCall>,
  pub assistant_text: Vec<String>,
  pub result: Option<ResultInfo>,
  #[serde(default)]
  pub hooks: Vec<HookEvent>,
  pub event_counts: BTreeMap<String, u64>,
  pub unparsed_lines: u64,
  pub orphan_tool_results: u64,
}

impl Transcript {
  pub fn parse(text: &str, offsets_ms: &[u64]) -> Self {
    let mut transcript = Self::default();
    for (index, line) in text.lines().enumerate() {
      let line = line.trim();
      if line.is_empty() {
        continue;
      }
      let Ok(event) = serde_json::from_str::<Value>(line) else {
        transcript.unparsed_lines += 1;
        continue;
      };
      transcript.absorb(&event, offsets_ms.get(index).copied());
    }
    transcript
  }

  pub fn turn_usage_total(&self) -> TokenUsage {
    self.turns.iter().fold(TokenUsage::default(), |total, turn| total.add(&turn.usage))
  }

  pub fn usage_total(&self) -> TokenUsage {
    match &self.result {
      Some(result) if result.usage.total() > 0 => result.usage,
      _ => self.turn_usage_total(),
    }
  }

  pub fn reported_models(&self) -> Vec<String> {
    let mut models: Vec<String> = self.result.iter().flat_map(|result| result.model_usage.keys().cloned()).collect();
    if models.is_empty() {
      models = self.turns.iter().filter_map(|turn| turn.model.clone()).collect();
      models.sort();
      models.dedup();
    }
    models
  }

  fn absorb(&mut self, event: &Value, elapsed_ms: Option<u64>) {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let subtype = event.get("subtype").and_then(Value::as_str);
    let label = subtype.map_or_else(|| kind.to_owned(), |subtype| format!("{kind}/{subtype}"));
    *self.event_counts.entry(label).or_default() += 1;
    match (kind, subtype) {
      ("system", Some("init")) => self.init = Some(parse_init(event)),
      ("system", Some("hook_started")) => self.hooks.push(HookEvent::parse(event, HookPhase::Started)),
      ("system", Some("hook_response")) => self.hooks.push(HookEvent::parse(event, HookPhase::finished(event))),
      ("assistant", _) => self.absorb_assistant(event, elapsed_ms),
      ("user", _) => self.absorb_user(event),
      ("result", _) => self.result = Some(parse_result(event)),
      _ => {}
    }
  }

  fn absorb_assistant(&mut self, event: &Value, elapsed_ms: Option<u64>) {
    let Some(message) = event.get("message") else {
      return;
    };
    let message_id = message.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let usage = message.get("usage").map(TokenUsage::from_api).unwrap_or_default();
    let model = message.get("model").and_then(Value::as_str).map(str::to_owned);
    let turn = match self.turns.iter().position(|turn| !message_id.is_empty() && turn.message_id == message_id) {
      Some(index) => {
        if usage.output >= self.turns[index].usage.output {
          self.turns[index].usage = usage;
        }
        index
      }
      None => {
        self.turns.push(AssistantTurn {
          message_id,
          model,
          usage,
          elapsed_ms,
        });
        self.turns.len() - 1
      }
    };
    for block in content_blocks(message) {
      match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => self.tool_calls.push(ToolCall {
          id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
          name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
          input: block.get("input").cloned().unwrap_or(Value::Null),
          elapsed_ms,
          result: None,
          turn: turn as u32 + 1,
        }),
        Some("text") => {
          if let Some(text) = block.get("text").and_then(Value::as_str) {
            self.assistant_text.push(text.to_owned());
          }
        }
        _ => {}
      }
    }
  }

  fn absorb_user(&mut self, event: &Value) {
    let Some(message) = event.get("message") else {
      return;
    };
    let results = content_blocks(message).filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"));
    let report = event.get("tool_use_result").filter(|_| results.clone().count() == 1);
    for block in results {
      let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
      let result = ToolResult {
        text: result_text(block.get("content")),
        is_error: block.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        completion: report.map(Completion::from_tool_use_result).unwrap_or_default(),
      };
      match self.tool_calls.iter_mut().rev().find(|call| call.id == id) {
        Some(call) => call.result = Some(result),
        None => self.orphan_tool_results += 1,
      }
    }
  }
}

fn content_blocks(message: &Value) -> impl Iterator<Item = &Value> + Clone {
  message.get("content").and_then(Value::as_array).into_iter().flatten()
}

fn result_text(content: Option<&Value>) -> String {
  match content {
    Some(Value::String(text)) => text.clone(),
    Some(Value::Array(blocks)) => blocks.iter().filter_map(|block| block.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
    _ => String::new(),
  }
}

fn string_list(event: &Value, field: &str) -> Vec<String> {
  event
    .get(field)
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|item| match item {
      Value::String(name) => Some(name.clone()),
      Value::Object(object) => object.get("name").and_then(Value::as_str).map(str::to_owned),
      _ => None,
    })
    .collect()
}

fn parse_init(event: &Value) -> InitInfo {
  let text = |field: &str| event.get(field).and_then(Value::as_str).map(str::to_owned);
  let mcp_servers = event
    .get("mcp_servers")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .map(|server| McpServerStatus {
      name: server.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
      status: server.get("status").and_then(Value::as_str).unwrap_or_default().to_owned(),
    })
    .collect();
  InitInfo {
    model: text("model"),
    cwd: text("cwd"),
    permission_mode: text("permissionMode"),
    claude_code_version: text("claude_code_version"),
    api_key_source: text("apiKeySource"),
    tools: string_list(event, "tools"),
    mcp_servers,
    slash_commands: string_list(event, "slash_commands"),
    skills: string_list(event, "skills"),
    plugins: string_list(event, "plugins"),
    agents: string_list(event, "agents"),
  }
}

fn parse_result(event: &Value) -> ResultInfo {
  let model_usage = event
    .get("modelUsage")
    .and_then(Value::as_object)
    .into_iter()
    .flatten()
    .map(|(model, usage)| {
      let entry = ModelUsage {
        usage: TokenUsage::from_model_usage(usage),
        cost_usd: usage.get("costUSD").and_then(Value::as_f64),
      };
      (model.clone(), entry)
    })
    .collect();
  ResultInfo {
    subtype: event.get("subtype").and_then(Value::as_str).map(str::to_owned),
    is_error: event.get("is_error").and_then(Value::as_bool).unwrap_or(false),
    duration_ms: event.get("duration_ms").and_then(Value::as_u64),
    num_turns: event.get("num_turns").and_then(Value::as_u64),
    total_cost_usd: event.get("total_cost_usd").and_then(Value::as_f64),
    usage: event.get("usage").map(TokenUsage::from_api).unwrap_or_default(),
    model_usage,
    text: event.get("result").and_then(Value::as_str).map(str::to_owned),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const PROBE: &str = include_str!("../../tests/fixtures/probe-transcript.jsonl");

  #[test]
  fn should_parse_the_recorded_probe_transcript() {
    let offsets: Vec<u64> = (0..13).map(|index| index * 100).collect();
    let transcript = Transcript::parse(PROBE, &offsets);
    let init = transcript.init.clone().unwrap();
    assert_eq!(init.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(init.permission_mode.as_deref(), Some("bypassPermissions"));
    assert_eq!(init.tools, vec!["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
    assert_eq!(init.mcp_servers, Vec::<McpServerStatus>::new());
    assert_eq!(init.skills, Vec::<String>::new());
    assert_eq!(init.plugins, vec!["cc-plugin-agents-md", "cc-plugin-telemetry", "cc-plugin-plugin-authoring"]);
    assert_eq!(transcript.turns.len(), 2);
    assert_eq!(transcript.turns[0].elapsed_ms, Some(400));
    assert_eq!(
      transcript.turns[0].usage,
      TokenUsage {
        input: 10,
        output: 4,
        cache_read: 8346,
        cache_creation: 1965
      }
    );
    assert_eq!(transcript.tool_calls.len(), 1);
    let write = &transcript.tool_calls[0];
    assert_eq!(write.name, "Write");
    assert_eq!(write.input_str("file_path"), Some("/eval/runs/probe/tree/hello.txt"));
    assert_eq!(write.elapsed_ms, Some(500));
    assert_eq!(write.turn, 1);
    assert!(write.result.as_ref().unwrap().text.starts_with("File created successfully"));
    let result = transcript.result.clone().unwrap();
    assert_eq!(result.subtype.as_deref(), Some("success"));
    assert_eq!(result.num_turns, Some(2));
    assert_eq!(
      result.usage,
      TokenUsage {
        input: 18,
        output: 531,
        cache_read: 18657,
        cache_creation: 2482
      }
    );
    assert_eq!(result.total_cost_usd, Some(0.0095027));
    assert_eq!(transcript.reported_models(), vec!["claude-haiku-4-5-20251001"]);
    assert_eq!(transcript.usage_total().total(), 18 + 531 + 18657 + 2482);
    assert_eq!(transcript.event_counts.get("system/thinking_tokens"), Some(&5));
    assert_eq!(transcript.unparsed_lines, 0);
  }

  #[test]
  fn should_merge_split_assistant_events_and_keep_the_largest_usage() {
    let text = [
      r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"a"}],"usage":{"input_tokens":5,"output_tokens":1}}}"#,
      r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/x"}}],"usage":{"input_tokens":5,"output_tokens":9}}}"#,
      r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"l1\nl2"}]}]}}"#,
      r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"missing","content":"x","is_error":true}]}}"#,
      "not json",
    ]
    .join("\n");
    let transcript = Transcript::parse(&text, &[]);
    assert_eq!(transcript.turns.len(), 1);
    assert_eq!(transcript.turns[0].usage.output, 9);
    assert_eq!(transcript.tool_calls[0].turn, 1);
    assert_eq!(transcript.tool_calls[0].result_bytes(), 5);
    assert_eq!(transcript.tool_calls[0].result.as_ref().unwrap().lines(), 2);
    assert_eq!(transcript.orphan_tool_results, 1);
    assert_eq!(transcript.unparsed_lines, 1);
    assert_eq!(transcript.usage_total().output, 9);
  }

  #[test]
  fn should_read_how_a_command_completed_from_the_structured_tool_result() {
    let call = |id: &str| format!(r#"{{"type":"assistant","message":{{"id":"m-{id}","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{}}}}]}}}}"#);
    let result =
      |id: &str, report: &str| format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"x"}}]}},"tool_use_result":{report}}}"#);
    let text = [
      call("a"),
      result("a", r#"{"stdout":"x","stderr":"","interrupted":false}"#),
      call("b"),
      result("b", r#"{"stdout":"","returnCodeInterpretation":"No matches found"}"#),
      call("c"),
      result(
        "c",
        r#"{"stdout":"","backgroundTaskId":"b1","backgroundCwdHint":"Session cwd remains /tree/src; directory changes do not apply."}"#,
      ),
      call("d"),
      result("d", r#""Error: Exit code 1""#),
      call("e"),
      r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e","content":"x"}]}}"#.to_owned(),
    ]
    .join("\n");
    let transcript = Transcript::parse(&text, &[]);
    let completions: Vec<Completion> = transcript.tool_calls.iter().map(|call| call.result.clone().unwrap().completion).collect();
    assert_eq!(
      completions,
      vec![
        Completion::Finished { exit_note: None },
        Completion::Finished {
          exit_note: Some("No matches found".to_owned())
        },
        Completion::Backgrounded {
          cwd_hint: Some("Session cwd remains /tree/src; directory changes do not apply.".to_owned())
        },
        Completion::Unreported,
        Completion::Unreported,
      ]
    );
    assert_eq!(transcript.tool_calls.iter().map(|call| call.turn).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
  }
}
