use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ToolStats {
  pub calls: u64,
  pub errors: u64,
  pub total_latency_us: u64,
  pub max_latency_us: u64,
  pub bytes_returned: u64,
  pub truncated_calls: u64,
  pub bytes_truncated: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetrySnapshot {
  pub uptime_secs: u64,
  pub total_calls: u64,
  pub tools: BTreeMap<String, ToolStats>,
}

#[derive(Debug)]
pub(crate) struct Telemetry {
  started: Instant,
  tools: Mutex<BTreeMap<&'static str, ToolStats>>,
}

impl Telemetry {
  pub(crate) fn new() -> Self {
    Self {
      started: Instant::now(),
      tools: Mutex::new(BTreeMap::new()),
    }
  }

  pub(crate) fn record(&self, tool: &'static str, latency: Duration, bytes: usize, failed: bool, truncated: usize) {
    let micros = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
    let mut tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
    let stats = tools.entry(tool).or_default();
    stats.calls += 1;
    stats.errors += u64::from(failed);
    stats.total_latency_us = stats.total_latency_us.saturating_add(micros);
    stats.max_latency_us = stats.max_latency_us.max(micros);
    stats.bytes_returned = stats.bytes_returned.saturating_add(bytes as u64);
    stats.truncated_calls += u64::from(truncated > 0);
    stats.bytes_truncated = stats.bytes_truncated.saturating_add(truncated as u64);
  }

  pub(crate) fn snapshot(&self) -> TelemetrySnapshot {
    let tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
    TelemetrySnapshot {
      uptime_secs: self.started.elapsed().as_secs(),
      total_calls: tools.values().map(|stats| stats.calls).sum(),
      tools: tools.iter().map(|(name, stats)| ((*name).to_owned(), stats.clone())).collect(),
    }
  }
}

impl TelemetrySnapshot {
  pub(crate) fn render_text(&self) -> String {
    let mut text = format!("mcp telemetry: {} calls in {}s (calls made before this one)\n", self.total_calls, self.uptime_secs);
    for (name, stats) in &self.tools {
      let average_ms = stats.total_latency_us as f64 / stats.calls.max(1) as f64 / 1000.0;
      let max_ms = stats.max_latency_us as f64 / 1000.0;
      text.push_str(&format!(
        "  {name} calls={} errors={} avg={average_ms:.1}ms max={max_ms:.1}ms bytes={} truncated={} ({} bytes dropped)\n",
        stats.calls, stats.errors, stats.bytes_returned, stats.truncated_calls, stats.bytes_truncated
      ));
    }
    text
  }
}
