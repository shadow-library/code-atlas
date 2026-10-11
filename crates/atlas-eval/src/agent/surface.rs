use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::claude::ATLAS_TOOL_PREFIX;
use super::process::ProcessExit;
use super::transcript::{TokenUsage, Transcript};

pub const SURFACE_PROMPT: &str = "Reply with the single word ok.";
pub const SURFACE_MAX_TURNS: u32 = 1;
pub const SURFACE_REPEATS: u32 = 3;
pub const SURFACE_SESSION_CAP: u32 = 12;
pub const SURFACE_RETRY_CAP: u32 = 4;
pub const SURFACE_DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";
pub const TOOL_SEARCH_VAR: &str = "ENABLE_TOOL_SEARCH";
pub const TOOL_SEARCH_ON: &str = "true";
pub const TOOL_SEARCH_OFF: &str = "false";
pub const TOOL_SEARCH_TOOL: &str = "ToolSearch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBytes {
  pub name: String,
  pub name_description_bytes: u64,
  pub input_schema_bytes: u64,
  pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceBytes {
  pub server_name: Option<String>,
  pub server_version: Option<String>,
  pub instructions_bytes: u64,
  pub tools_list_bytes: u64,
  pub tool_count: u64,
  pub tools: Vec<ToolBytes>,
  pub estimated_tokens_chars_div4: u64,
}

pub fn measure_surface_bytes(initialize: &Value, tools_list: &Value) -> SurfaceBytes {
  let instructions = initialize.get("instructions").and_then(Value::as_str).unwrap_or_default();
  let tools_list_text = compact(tools_list);
  let mut tools: Vec<ToolBytes> = tools_list
    .get("tools")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .map(|tool| {
      let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
      let description = tool.get("description").and_then(Value::as_str).unwrap_or_default();
      ToolBytes {
        name: name.to_owned(),
        name_description_bytes: (name.len() + description.len()) as u64,
        input_schema_bytes: tool.get("inputSchema").map_or(0, |schema| compact(schema).len() as u64),
        total_bytes: compact(tool).len() as u64,
      }
    })
    .collect();
  tools.sort_by(|left, right| right.total_bytes.cmp(&left.total_bytes).then_with(|| left.name.cmp(&right.name)));
  let chars = instructions.chars().count() + tools_list_text.chars().count();
  SurfaceBytes {
    server_name: initialize.pointer("/serverInfo/name").and_then(Value::as_str).map(str::to_owned),
    server_version: initialize.pointer("/serverInfo/version").and_then(Value::as_str).map(str::to_owned),
    instructions_bytes: instructions.len() as u64,
    tools_list_bytes: tools_list_text.len() as u64,
    tool_count: tools.len() as u64,
    tools,
    estimated_tokens_chars_div4: (chars / 4) as u64,
  }
}

fn compact(value: &Value) -> String {
  serde_json::to_string(value).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpArm {
  NoMcp,
  CodeAtlas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLoading {
  Upfront,
  #[serde(alias = "deferred")]
  SearchToolDenied,
  SearchAllowed,
}

impl ToolLoading {
  pub fn tool_search_value(self) -> &'static str {
    match self {
      Self::Upfront => TOOL_SEARCH_OFF,
      Self::SearchToolDenied | Self::SearchAllowed => TOOL_SEARCH_ON,
    }
  }

  pub fn allows_tool_search_tool(self) -> bool {
    self == Self::SearchAllowed
  }
}

pub fn surface_configurations() -> Vec<(McpArm, ToolLoading)> {
  let mut configurations = Vec::new();
  for loading in [ToolLoading::Upfront, ToolLoading::SearchToolDenied] {
    for arm in [McpArm::NoMcp, McpArm::CodeAtlas] {
      configurations.push((arm, loading));
    }
  }
  configurations
}

pub fn search_allowed_configurations() -> Vec<(McpArm, ToolLoading)> {
  vec![(McpArm::CodeAtlas, ToolLoading::SearchAllowed), (McpArm::NoMcp, ToolLoading::SearchAllowed)]
}

pub const EXTENSION_SESSION_CAP: u32 = 6;
pub const EXTENSION_RETRY_CAP: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptUsage {
  pub input: u64,
  pub cache_creation: u64,
  pub cache_read: u64,
  pub prompt_total: u64,
}

impl PromptUsage {
  pub fn of(usage: &TokenUsage) -> Self {
    Self {
      input: usage.input,
      cache_creation: usage.cache_creation,
      cache_read: usage.cache_read,
      prompt_total: usage.input + usage.cache_creation + usage.cache_read,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exposure {
  pub tool_count: u64,
  pub atlas_tool_count: u64,
  pub tool_search_tool_present: bool,
  pub mcp_servers: Vec<String>,
  pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSample {
  pub first_request: PromptUsage,
  pub result_usage: PromptUsage,
  pub output_tokens: u64,
  pub num_turns: Option<u64>,
  pub cost_usd: Option<f64>,
  pub exposure: Exposure,
}

pub fn first_request_usage(transcript: &Transcript) -> Option<TokenUsage> {
  transcript.turns.first().map(|turn| turn.usage)
}

pub fn sample_from_transcript(exit: &ProcessExit, transcript: &Transcript) -> Result<SessionSample, String> {
  if !exit.succeeded() {
    return Err(format!("process did not exit cleanly: {exit:?}"));
  }
  let Some(result) = &transcript.result else {
    return Err("the stream has no result event".to_owned());
  };
  if result.is_error {
    return Err(format!("the result is an error: {}", result.text.clone().unwrap_or_default()));
  }
  let Some(first) = first_request_usage(transcript) else {
    return Err("the stream has no assistant message".to_owned());
  };
  let init = transcript.init.clone().unwrap_or_default();
  let mut tools = init.tools.clone();
  tools.sort();
  Ok(SessionSample {
    first_request: PromptUsage::of(&first),
    result_usage: PromptUsage::of(&result.usage),
    output_tokens: first.output,
    num_turns: result.num_turns,
    cost_usd: result.total_cost_usd,
    exposure: Exposure {
      tool_count: tools.len() as u64,
      atlas_tool_count: tools.iter().filter(|tool| tool.starts_with(ATLAS_TOOL_PREFIX)).count() as u64,
      tool_search_tool_present: tools.iter().any(|tool| tool == TOOL_SEARCH_TOOL),
      mcp_servers: init.mcp_servers.iter().map(|server| format!("{} ({})", server.name, server.status)).collect(),
      tools,
    },
  })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spread {
  pub min: u64,
  pub median: u64,
  pub max: u64,
}

impl Spread {
  pub fn of(values: &[u64]) -> Option<Self> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let (min, max) = (*sorted.first()?, *sorted.last()?);
    Some(Self {
      min,
      median: sorted[(sorted.len() - 1) / 2],
      max,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationResult {
  pub arm: McpArm,
  pub loading: ToolLoading,
  pub samples: Vec<SessionSample>,
  pub failed_attempts: Vec<String>,
  pub prompt_total: Option<Spread>,
  pub input: Option<Spread>,
  pub cache_creation: Option<Spread>,
  pub cache_read: Option<Spread>,
}

impl ConfigurationResult {
  pub fn new(arm: McpArm, loading: ToolLoading, samples: Vec<SessionSample>, failed_attempts: Vec<String>) -> Self {
    let spread = |field: fn(&PromptUsage) -> u64| Spread::of(&samples.iter().map(|sample| field(&sample.first_request)).collect::<Vec<_>>());
    Self {
      arm,
      loading,
      prompt_total: spread(|usage| usage.prompt_total),
      input: spread(|usage| usage.input),
      cache_creation: spread(|usage| usage.cache_creation),
      cache_read: spread(|usage| usage.cache_read),
      samples,
      failed_attempts,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaRange {
  pub median: i64,
  pub low: i64,
  pub high: i64,
}

impl DeltaRange {
  pub fn between(with: &Spread, without: &Spread) -> Self {
    let signed = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    Self {
      median: signed(with.median) - signed(without.median),
      low: signed(with.min) - signed(without.max),
      high: signed(with.max) - signed(without.min),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadingDelta {
  pub loading: ToolLoading,
  pub prompt_total: DeltaRange,
  pub input: DeltaRange,
  pub cache_creation: DeltaRange,
  pub cache_read: DeltaRange,
}

pub fn loading_delta(loading: ToolLoading, without: &ConfigurationResult, with: &ConfigurationResult) -> Option<LoadingDelta> {
  let range = |field: fn(&ConfigurationResult) -> Option<Spread>| Some(DeltaRange::between(&field(with)?, &field(without)?));
  Some(LoadingDelta {
    loading,
    prompt_total: range(|result| result.prompt_total)?,
    input: range(|result| result.input)?,
    cache_creation: range(|result| result.cache_creation)?,
    cache_read: range(|result| result.cache_read)?,
  })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLedger {
  pub sessions_started: u32,
  pub retries_used: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenMeasurement {
  pub prompt: String,
  pub max_turns: u32,
  pub repeats: u32,
  pub sessions_started: u32,
  pub retries_used: u32,
  pub configurations: Vec<ConfigurationResult>,
  pub deltas: Vec<LoadingDelta>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub extension: Option<SessionLedger>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub extension_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TokenSection {
  Measured(Box<TokenMeasurement>),
  NotMeasured { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceReport {
  pub model: String,
  pub claude_code_version: Option<String>,
  pub atlas_version: Option<String>,
  pub bytes: SurfaceBytes,
  pub tokens: TokenSection,
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn canned_initialize() -> Value {
    json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "code-atlas", "version": "0.1.0"}, "instructions": "Use the graph. é"})
  }

  fn canned_tools_list() -> Value {
    json!({"tools": [
      {"name": "a", "description": "dd", "inputSchema": {"type": "object"}},
      {"name": "bb", "description": "d", "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}}, "annotations": {"readOnlyHint": true}}
    ]})
  }

  #[test]
  fn should_count_instruction_tool_and_schema_bytes_and_sort_tools_largest_first() {
    let bytes = measure_surface_bytes(&canned_initialize(), &canned_tools_list());
    assert_eq!(bytes.server_name.as_deref(), Some("code-atlas"));
    assert_eq!(bytes.server_version.as_deref(), Some("0.1.0"));
    assert_eq!(bytes.instructions_bytes, 17);
    assert_eq!(bytes.tool_count, 2);
    assert_eq!(bytes.tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), vec!["bb", "a"]);
    assert_eq!(
      bytes.tools[0],
      ToolBytes {
        name: "bb".to_owned(),
        name_description_bytes: 3,
        input_schema_bytes: 54,
        total_bytes: 136
      }
    );
    assert_eq!(
      bytes.tools[1],
      ToolBytes {
        name: "a".to_owned(),
        name_description_bytes: 3,
        input_schema_bytes: 17,
        total_bytes: 63
      }
    );
    assert_eq!(bytes.tools_list_bytes, 212);
    assert_eq!(bytes.estimated_tokens_chars_div4, (16 + 212) / 4);
  }

  #[test]
  fn should_report_zero_bytes_for_an_empty_surface() {
    let bytes = measure_surface_bytes(&json!({}), &json!({"tools": []}));
    assert_eq!(
      (bytes.instructions_bytes, bytes.tools_list_bytes, bytes.tool_count, bytes.estimated_tokens_chars_div4),
      (0, 12, 0, 3)
    );
  }

  const CANNED_STREAM: &str = concat!(
    r#"{"type":"system","subtype":"init","tools":["Bash","mcp__code-atlas__atlas_status","mcp__code-atlas__atlas_search","ToolSearch"],"mcp_servers":[{"name":"code-atlas","status":"connected"}]}"#,
    "\n",
    r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":4,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":2000}}}"#,
    "\n",
    r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.002,"result":"ok","usage":{"input_tokens":6,"output_tokens":7,"cache_read_input_tokens":110,"cache_creation_input_tokens":2100}}"#,
    "\n"
  );

  #[test]
  fn should_read_the_first_request_usage_and_exposure_from_a_stream_json_result() {
    let transcript = Transcript::parse(CANNED_STREAM, &[]);
    let sample = sample_from_transcript(&ProcessExit::Exited { code: Some(0) }, &transcript).unwrap();
    assert_eq!(
      sample.first_request,
      PromptUsage {
        input: 4,
        cache_creation: 2000,
        cache_read: 100,
        prompt_total: 2104
      }
    );
    assert_eq!(sample.result_usage.prompt_total, 2216);
    assert_eq!(sample.output_tokens, 5);
    assert_eq!(sample.num_turns, Some(1));
    assert_eq!(sample.cost_usd, Some(0.002));
    assert_eq!(sample.exposure.tool_count, 4);
    assert_eq!(sample.exposure.atlas_tool_count, 2);
    assert!(sample.exposure.tool_search_tool_present);
    assert_eq!(sample.exposure.mcp_servers, vec!["code-atlas (connected)"]);
  }

  #[test]
  fn should_reject_sessions_without_a_usable_result() {
    let exited = ProcessExit::Exited { code: Some(0) };
    assert_eq!(
      sample_from_transcript(&ProcessExit::TimedOut, &Transcript::parse(CANNED_STREAM, &[])),
      Err("process did not exit cleanly: TimedOut".to_owned())
    );
    assert_eq!(sample_from_transcript(&exited, &Transcript::default()), Err("the stream has no result event".to_owned()));
    let errored = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#;
    assert_eq!(
      sample_from_transcript(&exited, &Transcript::parse(errored, &[])),
      Err("the result is an error: Not logged in".to_owned())
    );
    let no_turn = r#"{"type":"result","subtype":"success","is_error":false}"#;
    assert_eq!(
      sample_from_transcript(&exited, &Transcript::parse(no_turn, &[])),
      Err("the stream has no assistant message".to_owned())
    );
  }

  #[test]
  fn should_summarise_spreads_and_deltas_between_arms() {
    assert_eq!(Spread::of(&[30, 10, 20]), Some(Spread { min: 10, median: 20, max: 30 }));
    assert_eq!(Spread::of(&[1, 4]), Some(Spread { min: 1, median: 1, max: 4 }));
    assert_eq!(Spread::of(&[]), None);
    let delta = DeltaRange::between(
      &Spread {
        min: 900,
        median: 1000,
        max: 1100,
      },
      &Spread { min: 80, median: 100, max: 120 },
    );
    assert_eq!(
      delta,
      DeltaRange {
        median: 900,
        low: 780,
        high: 1020
      }
    );
  }

  fn configuration(arm: McpArm, totals: [u64; 3]) -> ConfigurationResult {
    let sample = |total: u64| SessionSample {
      first_request: PromptUsage {
        input: 1,
        cache_creation: total - 1,
        cache_read: 0,
        prompt_total: total,
      },
      result_usage: PromptUsage::of(&TokenUsage::default()),
      output_tokens: 1,
      num_turns: Some(1),
      cost_usd: None,
      exposure: Exposure {
        tool_count: 0,
        atlas_tool_count: 0,
        tool_search_tool_present: false,
        mcp_servers: Vec::new(),
        tools: Vec::new(),
      },
    };
    ConfigurationResult::new(arm, ToolLoading::Upfront, totals.iter().map(|total| sample(*total)).collect(), Vec::new())
  }

  #[test]
  fn should_compute_the_loading_delta_from_medians_with_the_extreme_range() {
    let without = configuration(McpArm::NoMcp, [100, 110, 105]);
    let with = configuration(McpArm::CodeAtlas, [7000, 7010, 7020]);
    let delta = loading_delta(ToolLoading::Upfront, &without, &with).unwrap();
    assert_eq!(
      delta.prompt_total,
      DeltaRange {
        median: 6905,
        low: 6890,
        high: 6920
      }
    );
    assert_eq!(delta.input, DeltaRange { median: 0, low: 0, high: 0 });
    assert_eq!(
      delta.cache_creation,
      DeltaRange {
        median: 6905,
        low: 6890,
        high: 6920
      }
    );
  }

  #[test]
  fn should_enumerate_the_four_configurations_with_loading_as_the_outer_axis() {
    assert_eq!(
      surface_configurations(),
      vec![
        (McpArm::NoMcp, ToolLoading::Upfront),
        (McpArm::CodeAtlas, ToolLoading::Upfront),
        (McpArm::NoMcp, ToolLoading::SearchToolDenied),
        (McpArm::CodeAtlas, ToolLoading::SearchToolDenied),
      ]
    );
    assert_eq!(
      search_allowed_configurations(),
      vec![(McpArm::CodeAtlas, ToolLoading::SearchAllowed), (McpArm::NoMcp, ToolLoading::SearchAllowed)]
    );
    assert_eq!(ToolLoading::Upfront.tool_search_value(), "false");
    assert_eq!(ToolLoading::SearchToolDenied.tool_search_value(), "true");
    assert_eq!(ToolLoading::SearchAllowed.tool_search_value(), "true");
    assert!(ToolLoading::SearchAllowed.allows_tool_search_tool());
    assert!(!ToolLoading::SearchToolDenied.allows_tool_search_tool());
  }

  #[test]
  fn should_read_the_legacy_deferred_loading_name() {
    assert_eq!(serde_json::from_str::<ToolLoading>("\"deferred\"").unwrap(), ToolLoading::SearchToolDenied);
  }
}
