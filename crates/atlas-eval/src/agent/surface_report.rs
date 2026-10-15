use std::fmt::Write;

use super::surface::{ConfigurationResult, DeltaRange, McpArm, Spread, SurfaceReport, TokenMeasurement, TokenSection, ToolLoading};

pub fn render_surface_markdown(report: &SurfaceReport) -> String {
  let mut text = String::new();
  let bytes = &report.bytes;
  let _ = writeln!(text, "# MCP tool surface cost\n");
  let _ = writeln!(text, "- model: `{}`", report.model);
  let _ = writeln!(text, "- Claude Code: {}", report.claude_code_version.as_deref().unwrap_or("unknown"));
  let _ = writeln!(text, "- catl: {}", report.atlas_version.as_deref().unwrap_or("unknown"));
  let _ = writeln!(text, "- tools: {}", report.tools.as_deref().unwrap_or("the server default (`[mcp] tools`, else full)"));
  let _ = writeln!(
    text,
    "- MCP server: {} {}\n",
    bytes.server_name.as_deref().unwrap_or("unknown"),
    bytes.server_version.as_deref().unwrap_or("")
  );
  let _ = writeln!(text, "## Bytes (deterministic, no session)\n");
  let _ = writeln!(
    text,
    "Measured by starting `catl mcp` on the acme fixture and sending `initialize` and `tools/list`. Sizes are UTF-8 bytes of the compact JSON.\n"
  );
  let _ = writeln!(text, "| item | bytes |\n|---|---:|");
  let _ = writeln!(text, "| `instructions` string | {} |", bytes.instructions_bytes);
  let _ = writeln!(text, "| whole `tools/list` result | {} |", bytes.tools_list_bytes);
  let _ = writeln!(text, "| tool count | {} |", bytes.tool_count);
  let _ = writeln!(
    text,
    "| instructions + tools/list, characters / 4 (rough estimate, not a tokenizer) | {} |\n",
    bytes.estimated_tokens_chars_div4
  );
  let _ = writeln!(text, "| tool | name + description | input schema | whole tool object |\n|---|---:|---:|---:|");
  for tool in &bytes.tools {
    let _ = writeln!(
      text,
      "| `{}` | {} | {} | {} |",
      tool.name, tool.name_description_bytes, tool.input_schema_bytes, tool.total_bytes
    );
  }
  let _ = writeln!(text, "\n## Tokens (measured, tiny sessions)\n");
  match &report.tokens {
    TokenSection::NotMeasured { reason } => {
      let _ = writeln!(text, "not measured: {reason}");
    }
    TokenSection::Measured(measurement) => render_tokens(&mut text, measurement, report),
  }
  text
}

fn render_tokens(text: &mut String, measurement: &TokenMeasurement, report: &SurfaceReport) {
  let _ = writeln!(
    text,
    "Prompt `{}`, `--max-turns {}`, the eval session flags (`--strict-mcp-config`, `--setting-sources \"\"`, the six base tools, plus `ToolSearch` in the allowed mode), {} repeats per configuration; {} sessions started, {} of them retries.\n",
    measurement.prompt, measurement.max_turns, measurement.repeats, measurement.sessions_started, measurement.retries_used
  );
  let _ = writeln!(
    text,
    "Compared fields are those of the first (only) API request, read from its assistant event: `input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`. \
Their sum (`prompt total`) is the size of the prompt whatever the cache state, so it is the number compared; the individual fields only show how that prompt was split between cache writes and cache reads, which depends on what earlier sessions left in the cache.\n"
  );
  let _ = writeln!(
    text,
    "| configuration | run | input | cache creation | cache read | prompt total |\n|---|---:|---:|---:|---:|---:|"
  );
  for result in &measurement.configurations {
    for (index, sample) in result.samples.iter().enumerate() {
      let usage = &sample.first_request;
      let _ = writeln!(
        text,
        "| {} | {} | {} | {} | {} | {} |",
        configuration_label(result),
        index + 1,
        usage.input,
        usage.cache_creation,
        usage.cache_read,
        usage.prompt_total
      );
    }
  }
  let _ = writeln!(
    text,
    "\n| configuration | prompt total min / median / max | cache creation median | cache read median |\n|---|---|---:|---:|"
  );
  for result in &measurement.configurations {
    let _ = writeln!(
      text,
      "| {} | {} | {} | {} |",
      configuration_label(result),
      spread_cell(result.prompt_total),
      result.cache_creation.map_or("-".to_owned(), |spread| spread.median.to_string()),
      result.cache_read.map_or("-".to_owned(), |spread| spread.median.to_string())
    );
  }
  let _ = writeln!(text, "\n### Tokens Code Atlas adds per request\n");
  let _ = writeln!(
    text,
    "Difference of medians, with the extreme range (lowest Code Atlas run minus highest baseline run, highest minus lowest).\n"
  );
  let _ = writeln!(text, "| loading | prompt total | input | cache creation | cache read |\n|---|---|---|---|---|");
  for delta in &measurement.deltas {
    let _ = writeln!(
      text,
      "| {} | {} | {} | {} | {} |",
      loading_label(delta.loading),
      range_cell(delta.prompt_total),
      range_cell(delta.input),
      range_cell(delta.cache_creation),
      range_cell(delta.cache_read)
    );
  }
  render_comparison(text, measurement, report);
  let _ = writeln!(text, "\n### What the init event exposed\n");
  let _ = writeln!(
    text,
    "| configuration | tools | atlas tools | `ToolSearch` tool | MCP servers | identical in every run |\n|---|---:|---:|---|---|---|"
  );
  for result in &measurement.configurations {
    let Some(first) = result.samples.first() else { continue };
    let identical = result.samples.iter().all(|sample| sample.exposure == first.exposure);
    let exposure = &first.exposure;
    let _ = writeln!(
      text,
      "| {} | {} | {} | {} | {} | {} |",
      configuration_label(result),
      exposure.tool_count,
      exposure.atlas_tool_count,
      if exposure.tool_search_tool_present { "present" } else { "absent" },
      if exposure.mcp_servers.is_empty() {
        "none".to_owned()
      } else {
        exposure.mcp_servers.join(", ")
      },
      if identical { "yes" } else { "no" }
    );
  }
  let _ = writeln!(
    text,
    "\n### Limits\n\n- One model and one Claude Code version; the byte numbers do not depend on either.\n- The prompt total is the size of the first request, not its cost: cache reads and cache writes are billed differently.\n- The sessions run in an empty directory with no project instructions, so the baseline prompt (system prompt, six built-in tools) is a floor, not a typical repository session.\n- characters / 4 is a rough estimate, not a tokenizer count."
  );
  let failed: Vec<&String> = measurement.configurations.iter().flat_map(|result| &result.failed_attempts).collect();
  if !failed.is_empty() {
    let _ = writeln!(text, "\n### Failed attempts\n");
    for failure in failed {
      let _ = writeln!(text, "- {failure}");
    }
  }
}

fn render_comparison(text: &mut String, measurement: &TokenMeasurement, report: &SurfaceReport) {
  let _ = writeln!(text);
  for delta in &measurement.deltas {
    let _ = writeln!(text, "- {} adds {} tokens per request.", loading_label(delta.loading), delta.prompt_total.median);
  }
  let upfront = measurement.deltas.iter().find(|delta| delta.loading == ToolLoading::Upfront);
  if let Some(upfront) = upfront {
    let _ = writeln!(
      text,
      "- The byte accounting predicts about {} tokens (characters / 4) if every schema and the instructions were sent; the upfront delta is {:.2} times that.",
      report.bytes.estimated_tokens_chars_div4,
      ratio(upfront.prompt_total.median, report.bytes.estimated_tokens_chars_div4)
    );
  }
  let denied = measurement.deltas.iter().find(|delta| delta.loading == ToolLoading::SearchToolDenied);
  let exposure = measurement
    .configurations
    .iter()
    .find(|result| result.arm == McpArm::CodeAtlas && result.loading == ToolLoading::SearchToolDenied)
    .and_then(|result| result.samples.first())
    .map(|sample| &sample.exposure);
  if let (Some(upfront), Some(denied), Some(exposure)) = (upfront, denied, exposure)
    && denied.prompt_total.median * 10 >= upfront.prompt_total.median * 9
  {
    let _ = writeln!(
      text,
      "- With the `ToolSearch` tool not allowed nothing was deferred (median delta {} against {} upfront): the init event listed {} atlas tools and the `ToolSearch` tool was {}.",
      denied.prompt_total.median,
      upfront.prompt_total.median,
      exposure.atlas_tool_count,
      if exposure.tool_search_tool_present { "present" } else { "absent" }
    );
  }
  if let Some(ledger) = &measurement.extension {
    let _ = writeln!(
      text,
      "- The tool-search-allowed mode was measured in a second run: {} sessions started, {} of them retries.",
      ledger.sessions_started, ledger.retries_used
    );
  }
  if let Some(note) = &measurement.extension_note {
    let _ = writeln!(text, "- {note}");
  }
}

fn ratio(numerator: i64, denominator: u64) -> f64 {
  if denominator == 0 {
    return 0.0;
  }
  numerator as f64 / denominator as f64
}

fn configuration_label(result: &ConfigurationResult) -> String {
  let arm = match result.arm {
    McpArm::NoMcp => "no MCP",
    McpArm::CodeAtlas => "Code Atlas",
  };
  format!("{arm}, {}", loading_label(result.loading))
}

fn loading_label(loading: ToolLoading) -> &'static str {
  match loading {
    ToolLoading::Upfront => "upfront (`ENABLE_TOOL_SEARCH=false`)",
    ToolLoading::SearchToolDenied => "tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`)",
    ToolLoading::SearchAllowed => "tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`)",
  }
}

fn spread_cell(spread: Option<Spread>) -> String {
  spread.map_or("-".to_owned(), |spread| format!("{} / {} / {}", spread.min, spread.median, spread.max))
}

fn range_cell(range: DeltaRange) -> String {
  format!("{:+} ({:+} to {:+})", range.median, range.low, range.high)
}

#[cfg(test)]
mod tests {
  use super::super::surface::{SurfaceBytes, ToolBytes};
  use super::*;

  #[test]
  fn should_render_the_not_measured_reason_after_the_bytes_tables() {
    let report = SurfaceReport {
      model: "m".to_owned(),
      tools: None,
      claude_code_version: None,
      atlas_version: Some("catl 1".to_owned()),
      bytes: SurfaceBytes {
        server_name: Some("code-atlas".to_owned()),
        server_version: Some("1".to_owned()),
        instructions_bytes: 10,
        tools_list_bytes: 20,
        tool_count: 1,
        tools: vec![ToolBytes {
          name: "atlas_status".to_owned(),
          name_description_bytes: 5,
          input_schema_bytes: 6,
          total_bytes: 7,
        }],
        estimated_tokens_chars_div4: 7,
      },
      tokens: TokenSection::NotMeasured {
        reason: "the claude binary is unavailable".to_owned(),
      },
    };
    let text = render_surface_markdown(&report);
    assert!(text.contains("| `instructions` string | 10 |"));
    assert!(text.contains("| `atlas_status` | 5 | 6 | 7 |"));
    assert!(text.ends_with("not measured: the claude binary is unavailable\n"));
  }
}
