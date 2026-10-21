use std::fmt;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::spec::LoadedTask;

pub const STANDARD_PREAMBLE: &str = "A Code Atlas MCP server is available with tools for repository knowledge (context, search, impact, tests-for, explain). \
Use it to understand the codebase before reading files. Relations tagged [heuristic]/[inferred] are leads to verify.";
pub const DIRECTED_PREAMBLE: &str = "A Code Atlas MCP server is available with tools for repository knowledge. \
Before searching or reading files, call atlas_context with a one-line description of the task and read what it returns. \
Before changing a function or type that other code uses, call atlas_impact on it. \
Before writing a new helper, call atlas_search or atlas_similar to look for an existing one. \
Relations tagged [heuristic]/[inferred] are leads to verify. To edit a file, read it with your own file tool.";
pub const FORCED_PREAMBLE: &str = "A Code Atlas MCP server is available with tools for repository knowledge (context, search, symbol, impact). \
In this session Grep and Glob are not available and Bash search or read commands (grep, rg, find, ls, cat, head, tail, sed, awk) are refused: \
find code with atlas_search or atlas_context, list a file's symbols with atlas_symbol mode=outline, and open files with Read. \
Relations tagged [heuristic]/[inferred] are leads to verify.";
const FORCED_SURFACE_DESCRIPTION: &str = "forced (tools Bash, Edit, Read, Write; MCP core; Bash search guard)";
const CORE_TOOL_PROFILE: &str = "core";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PreambleVariant {
  #[default]
  Standard,
  Directed,
  #[value(skip)]
  Forced,
}

impl PreambleVariant {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Standard => "standard",
      Self::Directed => "directed",
      Self::Forced => "forced",
    }
  }

  pub fn version(self) -> u32 {
    match self {
      Self::Standard => 1,
      Self::Directed => 2,
      Self::Forced => 3,
    }
  }

  pub fn text(self) -> &'static str {
    match self {
      Self::Standard => STANDARD_PREAMBLE,
      Self::Directed => DIRECTED_PREAMBLE,
      Self::Forced => FORCED_PREAMBLE,
    }
  }

  pub fn label(self) -> String {
    match self {
      Self::Standard => self.version().to_string(),
      Self::Directed | Self::Forced => format!("{} ({})", self.version(), self.as_str()),
    }
  }
}

impl fmt::Display for PreambleVariant {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AtlasSurface {
  #[default]
  Standard,
  Forced,
}

impl AtlasSurface {
  pub fn is_standard(&self) -> bool {
    *self == Self::Standard
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Standard => "standard",
      Self::Forced => "forced",
    }
  }

  pub fn describe(self) -> &'static str {
    match self {
      Self::Standard => self.as_str(),
      Self::Forced => FORCED_SURFACE_DESCRIPTION,
    }
  }

  pub fn preamble(self, requested: PreambleVariant) -> PreambleVariant {
    match self {
      Self::Standard => requested,
      Self::Forced => PreambleVariant::Forced,
    }
  }

  pub fn mcp_tool_profile(self) -> Option<&'static str> {
    match self {
      Self::Standard => None,
      Self::Forced => Some(CORE_TOOL_PROFILE),
    }
  }
}

impl fmt::Display for AtlasSurface {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

const ASSUMED_TURN_CAP: u32 = 40;
const ASSUMED_CACHE_READ_PER_TURN: f64 = 40_000.0;
const ASSUMED_CACHE_WRITE_PER_TURN: f64 = 3_000.0;
const ASSUMED_INPUT_PER_TURN: f64 = 50.0;
const ASSUMED_OUTPUT_PER_TURN: f64 = 1_000.0;
const ASSUMED_JUDGE_INPUT: f64 = 30_000.0;
const ASSUMED_JUDGE_OUTPUT: f64 = 1_500.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
  Baseline,
  Atlas,
}

impl Condition {
  pub const ALL: [Condition; 2] = [Condition::Baseline, Condition::Atlas];

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Baseline => "baseline",
      Self::Atlas => "atlas",
    }
  }

  pub fn prompt(self, preamble: PreambleVariant, task_prompt: &str) -> String {
    match self {
      Self::Baseline => task_prompt.to_owned(),
      Self::Atlas => format!("{}\n\n{task_prompt}", preamble.text()),
    }
  }
}

impl fmt::Display for Condition {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedRun {
  pub order: usize,
  pub run_id: String,
  pub task_id: String,
  pub condition: Condition,
  pub run_index: u32,
}

pub fn run_id(task_id: &str, condition: Condition, run_index: u32) -> String {
  format!("{task_id}.{condition}.r{run_index}")
}

pub fn plan_runs(tasks: &[LoadedTask], conditions: &[Condition], runs_per_condition: u32, seed: u64) -> Vec<PlannedRun> {
  let mut runs: Vec<PlannedRun> = Vec::new();
  for task in tasks {
    for condition in conditions {
      for run_index in 0..runs_per_condition {
        runs.push(PlannedRun {
          order: 0,
          run_id: run_id(&task.spec.id, *condition, run_index),
          task_id: task.spec.id.clone(),
          condition: *condition,
          run_index,
        });
      }
    }
  }
  shuffle(&mut runs, seed);
  for (order, run) in runs.iter_mut().enumerate() {
    run.order = order;
  }
  runs
}

struct SplitMix64(u64);

impl SplitMix64 {
  fn next(&mut self) -> u64 {
    self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = self.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
  }

  fn below(&mut self, bound: usize) -> usize {
    (self.next() % bound as u64) as usize
  }
}

fn shuffle<T>(items: &mut [T], seed: u64) {
  let mut rng = SplitMix64(seed);
  for index in (1..items.len()).rev() {
    let other = rng.below(index + 1);
    items.swap(index, other);
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
  pub input: f64,
  pub output: f64,
  pub cache_write: f64,
  pub cache_read: f64,
}

pub fn model_price(model: &str) -> Option<ModelPrice> {
  let price = |input: f64, output: f64, cache_read: f64| ModelPrice {
    input,
    output,
    cache_write: input * 2.0,
    cache_read,
  };
  let known = [
    ("claude-sonnet-5-5", price(2.0, 10.0, 0.20)),
    ("claude-sonnet-5", price(2.0, 10.0, 0.20)),
    ("claude-sonnet-4-6", price(3.0, 15.0, 0.30)),
    ("claude-haiku-4-5", price(1.0, 5.0, 0.10)),
    ("claude-opus-5-5", price(4.0, 20.0, 0.20)),
    ("claude-opus-5", price(5.0, 25.0, 0.50)),
    ("claude-opus-4-8", price(5.0, 25.0, 0.50)),
    ("claude-opus-4-7", price(5.0, 25.0, 0.50)),
    ("claude-opus-4-6", price(5.0, 25.0, 0.50)),
  ];
  known
    .iter()
    .filter(|(prefix, _)| model == *prefix || model.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('-')))
    .max_by_key(|(prefix, _)| prefix.len())
    .map(|(_, price)| *price)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
  pub session_usd: f64,
  pub judge_usd: f64,
}

pub fn estimate_run_cost(model: &str, judge_model: Option<&str>, max_turns: u32, condition: Condition) -> Option<CostEstimate> {
  let price = model_price(model)?;
  let turns = f64::from(max_turns.min(ASSUMED_TURN_CAP));
  let per_million = |tokens: f64, rate: f64| tokens * rate / 1_000_000.0;
  let session_usd = turns
    * (per_million(ASSUMED_INPUT_PER_TURN, price.input)
      + per_million(ASSUMED_OUTPUT_PER_TURN, price.output)
      + per_million(ASSUMED_CACHE_WRITE_PER_TURN, price.cache_write)
      + per_million(ASSUMED_CACHE_READ_PER_TURN, price.cache_read));
  let judge_calls = match condition {
    Condition::Baseline => 1.0,
    Condition::Atlas => 2.0,
  };
  let judge_usd = match judge_model {
    None => 0.0,
    Some(judge) => {
      let judge_price = model_price(judge)?;
      judge_calls * (per_million(ASSUMED_JUDGE_INPUT, judge_price.input) + per_million(ASSUMED_JUDGE_OUTPUT, judge_price.output))
    }
  };
  Some(CostEstimate { session_usd, judge_usd })
}

pub struct DryRunContext<'a> {
  pub tasks: &'a [LoadedTask],
  pub runs: &'a [PlannedRun],
  pub model: &'a str,
  pub judge_model: Option<&'a str>,
  pub seed: u64,
  pub parallel: usize,
  pub budget_usd: Option<f64>,
  pub preamble: PreambleVariant,
  pub notes: &'a [String],
}

pub fn render_dry_run(context: &DryRunContext<'_>) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "planned runs: {} (seed {}, parallel {})", context.runs.len(), context.seed, context.parallel);
  let _ = writeln!(out, "model: {}  judge: {}", context.model, context.judge_model.unwrap_or("off"));
  if context.runs.iter().any(|run| run.condition == Condition::Atlas) {
    let _ = writeln!(out, "atlas preamble: {} (version {})", context.preamble, context.preamble.version());
  }
  let _ = writeln!(out, "order  run_id                                   max_turns  timeout_min  est_usd");
  let mut total: Option<f64> = Some(0.0);
  for run in context.runs {
    let Some(task) = context.tasks.iter().find(|task| task.spec.id == run.task_id) else {
      continue;
    };
    let estimate = estimate_run_cost(context.model, context.judge_model, task.spec.max_turns, run.condition);
    let cost = estimate.map(|estimate| estimate.session_usd + estimate.judge_usd);
    total = total.zip(cost).map(|(sum, cost)| sum + cost);
    let shown = cost.map_or_else(|| "unknown".to_owned(), |cost| format!("{cost:.2}"));
    let _ = writeln!(
      out,
      "{:>5}  {:<40} {:>9}  {:>11}  {:>7}",
      run.order, run.run_id, task.spec.max_turns, task.spec.timeout_minutes, shown
    );
  }
  match total {
    Some(total) => {
      let _ = writeln!(
        out,
        "estimated total: ${total:.2} (assumes up to {ASSUMED_TURN_CAP} turns/run, ~{ASSUMED_CACHE_READ_PER_TURN} cache-read and ~{ASSUMED_CACHE_WRITE_PER_TURN} cache-write tokens/turn at 1h-cache prices; a rough guide, not a bound)"
      );
    }
    None => {
      let _ = writeln!(out, "estimated total: unknown (no price table entry for {} or the judge model)", context.model);
    }
  }
  if let Some(budget) = context.budget_usd {
    let _ = writeln!(out, "budget: stops starting new runs once reported spend exceeds ${budget:.2}");
  }
  for note in context.notes {
    let _ = writeln!(out, "note: {note}");
  }
  out
}

pub fn today_utc() -> String {
  let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0);
  civil_date(seconds / 86_400)
}

pub fn civil_date(days_since_epoch: u64) -> String {
  let z = days_since_epoch as i64 + 719_468;
  let era = z.div_euclid(146_097);
  let day_of_era = z.rem_euclid(146_097);
  let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let month_index = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * month_index + 2) / 5 + 1;
  let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
  let year = year_of_era + era * 400 + i64::from(month <= 2);
  format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::super::spec::parse_task;
  use super::*;

  fn task(id: &str) -> LoadedTask {
    let text = format!("id = \"{id}\"\ntitle = \"t\"\nrepo = \"self\"\nbase_commit = \"abcdef1\"\nprompt = \"p\"\ntimeout_minutes = 5\nmax_turns = 30\n");
    LoadedTask {
      spec: parse_task(&text, std::path::Path::new("task.toml")).unwrap(),
      dir: PathBuf::from(id),
      spec_hash: "h".to_owned(),
    }
  }

  #[test]
  fn should_prefix_only_the_atlas_prompt_with_the_preamble() {
    assert_eq!(Condition::Baseline.prompt(PreambleVariant::Standard, "Fix it."), "Fix it.");
    assert_eq!(Condition::Baseline.prompt(PreambleVariant::Directed, "Fix it."), "Fix it.");
    assert_eq!(Condition::Atlas.prompt(PreambleVariant::Standard, "Fix it."), format!("{STANDARD_PREAMBLE}\n\nFix it."));
    assert_eq!(Condition::Atlas.prompt(PreambleVariant::Directed, "Fix it."), format!("{DIRECTED_PREAMBLE}\n\nFix it."));
  }

  #[test]
  fn should_plan_every_combination_in_a_seeded_deterministic_order() {
    let tasks = vec![task("t01"), task("t02")];
    let first = plan_runs(&tasks, &Condition::ALL, 2, 7);
    let again = plan_runs(&tasks, &Condition::ALL, 2, 7);
    let other = plan_runs(&tasks, &Condition::ALL, 2, 8);
    assert_eq!(first.len(), 8);
    assert_eq!(first, again);
    assert_ne!(
      first.iter().map(|run| &run.run_id).collect::<Vec<_>>(),
      other.iter().map(|run| &run.run_id).collect::<Vec<_>>()
    );
    let mut ids: Vec<&str> = first.iter().map(|run| run.run_id.as_str()).collect();
    ids.sort();
    assert_eq!(
      ids,
      vec![
        "t01.atlas.r0",
        "t01.atlas.r1",
        "t01.baseline.r0",
        "t01.baseline.r1",
        "t02.atlas.r0",
        "t02.atlas.r1",
        "t02.baseline.r0",
        "t02.baseline.r1"
      ]
    );
    assert_eq!(first.iter().map(|run| run.order).collect::<Vec<_>>(), (0..8).collect::<Vec<_>>());
  }

  #[test]
  fn should_price_dated_model_ids_by_their_family() {
    assert_eq!(model_price("claude-haiku-4-5-20251001").map(|price| price.output), Some(5.0));
    assert_eq!(model_price("claude-sonnet-5-5").map(|price| price.input), Some(2.0));
    assert_eq!(model_price("claude-opus-5-5").map(|price| price.input), Some(4.0));
    assert_eq!(model_price("claude-unknown-1"), None);
  }

  #[test]
  fn should_expose_exact_preamble_text_version_and_label_per_variant() {
    assert_eq!(PreambleVariant::default(), PreambleVariant::Standard);
    assert_eq!((PreambleVariant::Standard.version(), PreambleVariant::Standard.label()), (1, "1".to_owned()));
    assert_eq!((PreambleVariant::Directed.version(), PreambleVariant::Directed.label()), (2, "2 (directed)".to_owned()));
    assert_eq!(
      PreambleVariant::Directed.text(),
      "A Code Atlas MCP server is available with tools for repository knowledge. Before searching or reading files, call atlas_context with a one-line description of the task and read what it returns. Before changing a function or type that other code uses, call atlas_impact on it. Before writing a new helper, call atlas_search or atlas_similar to look for an existing one. Relations tagged [heuristic]/[inferred] are leads to verify. To edit a file, read it with your own file tool."
    );
    assert_eq!(
      PreambleVariant::Standard.text(),
      "A Code Atlas MCP server is available with tools for repository knowledge (context, search, impact, tests-for, explain). Use it to understand the codebase before reading files. Relations tagged [heuristic]/[inferred] are leads to verify."
    );
  }

  #[test]
  fn should_replace_any_requested_preamble_with_the_forced_one_under_the_forced_surface() {
    assert_eq!(AtlasSurface::default(), AtlasSurface::Standard);
    assert_eq!(AtlasSurface::Forced.preamble(PreambleVariant::Standard), PreambleVariant::Forced);
    assert_eq!(AtlasSurface::Standard.preamble(PreambleVariant::Directed), PreambleVariant::Directed);
    assert_eq!((PreambleVariant::Forced.version(), PreambleVariant::Forced.label()), (3, "3 (forced)".to_owned()));
    assert_eq!(
      PreambleVariant::Forced.text(),
      "A Code Atlas MCP server is available with tools for repository knowledge (context, search, symbol, impact). In this session Grep and Glob are not available and Bash search or read commands (grep, rg, find, ls, cat, head, tail, sed, awk) are refused: find code with atlas_search or atlas_context, list a file's symbols with atlas_symbol mode=outline, and open files with Read. Relations tagged [heuristic]/[inferred] are leads to verify."
    );
    assert_eq!(
      Condition::Atlas.prompt(AtlasSurface::Forced.preamble(PreambleVariant::Standard), "Fix it."),
      format!("{FORCED_PREAMBLE}\n\nFix it.")
    );
    assert_eq!(AtlasSurface::Forced.describe(), "forced (tools Bash, Edit, Read, Write; MCP core; Bash search guard)");
  }

  #[test]
  fn should_name_only_core_profile_tools_in_the_forced_preamble() {
    let named: Vec<&str> = FORCED_PREAMBLE
      .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
      .filter(|word| word.starts_with("atlas_"))
      .collect();
    assert_eq!(named, vec!["atlas_search", "atlas_context", "atlas_symbol"]);
    assert!(named.iter().all(|tool| super::super::claude::CORE_ATLAS_TOOLS.contains(tool)));
  }

  #[test]
  fn should_not_offer_the_forced_preamble_as_a_preamble_flag() {
    use clap::ValueEnum;
    let offered: Vec<String> = PreambleVariant::value_variants()
      .iter()
      .filter_map(|variant| variant.to_possible_value())
      .map(|value| value.get_name().to_owned())
      .collect();
    assert_eq!(offered, vec!["standard", "directed"]);
  }

  #[test]
  fn should_show_the_preamble_variant_in_the_dry_run_only_when_atlas_runs_are_planned() {
    let tasks = [task("t1")];
    let render = |conditions: &[Condition], preamble| {
      let runs = plan_runs(&tasks, conditions, 1, 1);
      render_dry_run(&DryRunContext {
        tasks: &tasks,
        runs: &runs,
        model: "claude-haiku-4-5",
        judge_model: None,
        seed: 1,
        parallel: 1,
        budget_usd: None,
        preamble,
        notes: &[],
      })
    };
    assert!(render(&Condition::ALL, PreambleVariant::Directed).contains("atlas preamble: directed (version 2)\n"));
    assert!(render(&Condition::ALL, PreambleVariant::Standard).contains("atlas preamble: standard (version 1)\n"));
    assert!(!render(&[Condition::Baseline], PreambleVariant::Directed).contains("atlas preamble"));
  }

  #[test]
  fn should_estimate_cost_from_the_documented_token_profile() {
    let estimate = estimate_run_cost("claude-haiku-4-5-20251001", Some("claude-sonnet-5-5"), 10, Condition::Atlas).unwrap();
    let per_turn = (50.0 * 1.0 + 1_000.0 * 5.0 + 3_000.0 * 2.0 + 40_000.0 * 0.10) / 1_000_000.0;
    assert!((estimate.session_usd - 10.0 * per_turn).abs() < 1e-9);
    let judge = 2.0 * (30_000.0 * 2.0 + 1_500.0 * 10.0) / 1_000_000.0;
    assert!((estimate.judge_usd - judge).abs() < 1e-9);
    assert_eq!(estimate_run_cost("claude-unknown-1", None, 10, Condition::Baseline), None);
  }

  #[test]
  fn should_convert_days_to_civil_dates() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(11_016), "2000-02-29");
    assert_eq!(civil_date(20_730), "2026-10-04");
  }
}
