use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::claude::ATLAS_TOOL_PREFIX;
use super::transcript::Transcript;

pub const UNVERIFIED_REASON: &str = "no SCIP reconciliation helper is available to the agent harness yet";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationLine {
  pub tool: String,
  pub line: String,
  pub certainty: String,
  pub confidence: Option<f64>,
  pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verification {
  Unverified { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtlasAccuracy {
  pub responses: u64,
  pub relations: Vec<RelationLine>,
  pub by_certainty: BTreeMap<String, u64>,
  pub stale_tags: u64,
  pub degraded_headers: u64,
  pub stale_headers: u64,
  pub verification: Verification,
}

pub fn atlas_accuracy(transcript: &Transcript) -> AtlasAccuracy {
  let mut accuracy = AtlasAccuracy {
    responses: 0,
    relations: Vec::new(),
    by_certainty: BTreeMap::new(),
    stale_tags: 0,
    degraded_headers: 0,
    stale_headers: 0,
    verification: Verification::Unverified {
      reason: UNVERIFIED_REASON.to_owned(),
    },
  };
  for call in transcript.tool_calls.iter().filter(|call| call.name.starts_with(ATLAS_TOOL_PREFIX)) {
    let Some(result) = &call.result else {
      continue;
    };
    accuracy.responses += 1;
    let tool = call.name.trim_start_matches(ATLAS_TOOL_PREFIX).to_owned();
    let header = result.text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default();
    accuracy.degraded_headers += u64::from(header.trim_start().starts_with("DEGRADED"));
    accuracy.stale_headers += u64::from(header.contains("stale"));
    for line in result.text.lines() {
      let Some(relation) = relation_line(&tool, line) else {
        continue;
      };
      *accuracy.by_certainty.entry(relation.certainty.clone()).or_default() += 1;
      accuracy.stale_tags += u64::from(relation.stale);
      accuracy.relations.push(relation);
    }
  }
  accuracy
}

pub fn relation_line(tool: &str, line: &str) -> Option<RelationLine> {
  let trimmed = line.trim_end();
  let inner = trimmed.strip_suffix(']')?;
  let open = inner.rfind('[')?;
  let mut words = inner[open + 1..].split_whitespace();
  let certainty = words.next()?;
  if !certainty.chars().all(|c| c.is_ascii_lowercase() || c == '_') || certainty.is_empty() {
    return None;
  }
  let rest: Vec<&str> = words.collect();
  let confidence = rest.first().and_then(|word| word.parse::<f64>().ok());
  let stale = rest.contains(&"stale");
  if rest.iter().any(|word| *word != "stale" && word.parse::<f64>().is_err()) {
    return None;
  }
  Some(RelationLine {
    tool: tool.to_owned(),
    line: trimmed.trim_start().to_owned(),
    certainty: certainty.to_owned(),
    confidence,
    stale,
  })
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::transcript::{ToolCall, ToolResult};
  use super::*;

  fn atlas_call(name: &str, text: &str) -> ToolCall {
    ToolCall {
      id: name.to_owned(),
      name: format!("{ATLAS_TOOL_PREFIX}{name}"),
      input: json!({}),
      elapsed_ms: None,
      result: Some(ToolResult {
        text: text.to_owned(),
        is_error: false,
        ..ToolResult::default()
      }),
      turn: 1,
    }
  }

  #[test]
  fn should_extract_tagged_relation_lines_and_header_states() {
    let transcript = Transcript {
      tool_calls: vec![
        atlas_call(
          "atlas_callers",
          "fresh run=3 (+0 files, 2ms)\n  a::b -> c::d [resolved]\n  a::b -> e::f [heuristic 0.72]\n  note [not a tag here]\n",
        ),
        atlas_call("atlas_explain", "DEGRADED: parse failed in src/x.rs\n  x equivalent_to y [inferred 0.81 stale]\n"),
        atlas_call("atlas_status", "fresh run=3, 2 stale inferred facts\n"),
      ],
      ..Transcript::default()
    };
    let accuracy = atlas_accuracy(&transcript);
    assert_eq!(accuracy.responses, 3);
    let tags: Vec<(&str, Option<f64>, bool)> = accuracy
      .relations
      .iter()
      .map(|relation| (relation.certainty.as_str(), relation.confidence, relation.stale))
      .collect();
    assert_eq!(tags, vec![("resolved", None, false), ("heuristic", Some(0.72), false), ("inferred", Some(0.81), true)]);
    assert_eq!(accuracy.by_certainty.get("heuristic"), Some(&1));
    assert_eq!(accuracy.stale_tags, 1);
    assert_eq!(accuracy.degraded_headers, 1);
    assert_eq!(accuracy.stale_headers, 1);
    assert_eq!(accuracy.relations[0].tool, "atlas_callers");
  }
}
