use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::access::OutOfTreeAccess;
use super::bash::analyze_bash;
use super::bash_guard::denied_by_bash_guard;
use super::claude::ATLAS_TOOL_PREFIX;
use super::hooks::{HookMetrics, hook_metrics};
use super::spec::{GroundTruth, LinkedSpec};
use super::transcript::{TokenUsage, ToolCall, Transcript};

const EDIT_TOOLS: [&str; 2] = ["Edit", "Write"];
const SEARCH_TOOLS: [&str; 2] = ["Grep", "Glob"];
const SOURCE_EXTENSIONS: [&str; 12] = ["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "toml", "json", "md", "yaml", "yml"];
const ATLAS_SECTIONS: [&str; 7] = ["MUST READ", "SKIM", "REUSE CANDIDATES", "IMPACT", "TESTS", "OMITTED", "UNCERTAIN"];
const MUST_READ: &str = "MUST READ";
const ATLAS_CONTEXT_TOOL: &str = "atlas_context";
const ATLAS_SYMBOL_TOOL: &str = "atlas_symbol";
const SOURCE_MODE: &str = "source";
const OMITTED: &str = "OMITTED";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenMetrics {
  pub per_turn: Vec<TokenUsage>,
  pub turns_sum: TokenUsage,
  pub total: TokenUsage,
  pub cost_usd: Option<f64>,
  pub num_turns: Option<u64>,
  pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetrics {
  pub by_name: BTreeMap<String, u64>,
  pub total: u64,
  pub search_calls: u64,
  pub errors: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMetrics {
  pub files: Vec<String>,
  pub outside_tree: Vec<String>,
  pub read_calls: u64,
  pub bash_read_calls: u64,
  pub unparsed_bash_reads: u64,
  pub bytes: u64,
  pub lines: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasMetrics {
  pub calls: u64,
  pub bytes: u64,
  pub must_read_files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelevanceMetrics {
  pub relevant_files: u64,
  pub relevant_read: Vec<String>,
  pub irrelevant_read: Vec<String>,
  pub relevant_missed: Vec<String>,
  pub files_changed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingMetrics {
  pub first_edit_ms: Option<u64>,
  pub session_ms: Option<u64>,
  pub wall_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DensityMetrics {
  pub useful_bytes: u64,
  pub context_bytes: u64,
  pub ratio: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
  #[serde(default)]
  pub out_of_tree_accesses: Vec<OutOfTreeAccess>,
  #[serde(default)]
  pub contaminated: bool,
  pub tokens: TokenMetrics,
  pub tools: ToolMetrics,
  pub reads: ReadMetrics,
  pub atlas: AtlasMetrics,
  pub relevance: RelevanceMetrics,
  pub timing: TimingMetrics,
  pub density: DensityMetrics,
  #[serde(default)]
  pub hooks: HookMetrics,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub atlas_surface_denials: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub linked: Option<LinkedMetrics>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedMetrics {
  pub linked_reads: u64,
  pub linked_source_calls: u64,
  pub linked_path_created: bool,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub linked_writes: Vec<String>,
  #[serde(default, skip_serializing_if = "LinkedRestore::is_restored")]
  pub restore: LinkedRestore,
}

impl LinkedMetrics {
  pub fn contaminates(&self) -> bool {
    !self.linked_writes.is_empty() || !self.restore.is_restored()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum LinkedRestore {
  #[default]
  Restored,
  HoldMissing {
    paths: Vec<String>,
  },
}

impl LinkedRestore {
  pub fn is_restored(&self) -> bool {
    *self == Self::Restored
  }
}

pub struct LinkedInput<'a> {
  pub roots: &'a [PathBuf],
  pub spec: &'a LinkedSpec,
  pub path_created: bool,
  pub writes: Vec<String>,
  pub restore: LinkedRestore,
}

pub struct MetricsInput<'a> {
  pub transcript: &'a Transcript,
  pub tree: &'a Path,
  pub view: Option<&'a Path>,
  pub ground_truth: &'a GroundTruth,
  pub changed_files: &'a BTreeSet<String>,
  pub wall_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct ReadLedger {
  files: BTreeSet<String>,
  outside: BTreeSet<String>,
  read_calls: u64,
  bash_read_calls: u64,
  unparsed: u64,
  bytes: u64,
  lines: u64,
  useful_bytes: u64,
}

pub fn compute_metrics(input: &MetricsInput<'_>) -> Metrics {
  let transcript = input.transcript;
  let roots = tree_roots(input.tree, input.view, transcript.init.as_ref().and_then(|init| init.cwd.as_deref()));
  let relevant: BTreeSet<String> = input.ground_truth.relevant_files.iter().cloned().collect();
  let mut reads = ReadLedger::default();
  let mut atlas = AtlasMetrics::default();
  let mut must_read = BTreeSet::new();
  let mut atlas_useful = 0;
  for call in &transcript.tool_calls {
    if call.name.starts_with(ATLAS_TOOL_PREFIX) {
      let text = call.result.as_ref().map(|result| result.text.as_str()).unwrap_or_default();
      atlas.calls += 1;
      atlas.bytes += call.result_bytes();
      let items = atlas_items(text);
      must_read.extend(
        items
          .iter()
          .filter(|item| item.section.as_deref() == Some(MUST_READ))
          .map(|item| tree_path_for(&item.location, input.tree)),
      );
      atlas_useful += atlas_useful_bytes(&items, &relevant, input.tree);
      continue;
    }
    absorb_read(call, &roots, &relevant, &mut reads);
  }
  atlas.must_read_files = must_read.iter().cloned().collect();
  let tools = tool_metrics(transcript);
  let context_bytes: u64 = transcript.tool_calls.iter().map(ToolCall::result_bytes).sum();
  let useful_bytes = reads.useful_bytes + atlas_useful;
  let covered: BTreeSet<&String> = reads.files.iter().chain(input.changed_files).chain(&must_read).collect();
  Metrics {
    out_of_tree_accesses: Vec::new(),
    contaminated: false,
    tokens: TokenMetrics {
      per_turn: transcript.turns.iter().map(|turn| turn.usage).collect(),
      turns_sum: transcript.turn_usage_total(),
      total: transcript.usage_total(),
      cost_usd: transcript.result.as_ref().and_then(|result| result.total_cost_usd),
      num_turns: transcript.result.as_ref().and_then(|result| result.num_turns),
      duration_ms: transcript.result.as_ref().and_then(|result| result.duration_ms),
    },
    tools,
    relevance: RelevanceMetrics {
      relevant_files: relevant.len() as u64,
      relevant_read: reads.files.intersection(&relevant).cloned().collect(),
      irrelevant_read: reads.files.difference(&relevant).cloned().collect(),
      relevant_missed: relevant.iter().filter(|file| !covered.contains(file)).cloned().collect(),
      files_changed: input.changed_files.iter().cloned().collect(),
    },
    reads: ReadMetrics {
      files: reads.files.into_iter().collect(),
      outside_tree: reads.outside.into_iter().collect(),
      read_calls: reads.read_calls,
      bash_read_calls: reads.bash_read_calls,
      unparsed_bash_reads: reads.unparsed,
      bytes: reads.bytes,
      lines: reads.lines,
    },
    atlas,
    timing: TimingMetrics {
      first_edit_ms: transcript
        .tool_calls
        .iter()
        .find(|call| EDIT_TOOLS.contains(&call.name.as_str()))
        .and_then(|call| call.elapsed_ms),
      session_ms: transcript.result.as_ref().and_then(|result| result.duration_ms),
      wall_ms: input.wall_ms,
    },
    density: DensityMetrics {
      useful_bytes,
      context_bytes,
      ratio: (context_bytes > 0).then(|| useful_bytes as f64 / context_bytes as f64),
    },
    hooks: hook_metrics(transcript),
    atlas_surface_denials: None,
    linked: None,
  }
}

pub fn linked_metrics(transcript: &Transcript, input: &LinkedInput<'_>) -> LinkedMetrics {
  let mut read = BTreeSet::new();
  let mut source_calls = 0;
  for call in transcript.tool_calls.iter().filter(|call| !denied_by_bash_guard(call)) {
    if let Some(tool) = call.name.strip_prefix(ATLAS_TOOL_PREFIX) {
      let reads_source = tool == ATLAS_CONTEXT_TOOL || (tool == ATLAS_SYMBOL_TOOL && call.input_str("mode") == Some(SOURCE_MODE));
      let text = call.result.as_ref().map(|result| result.text.as_str()).unwrap_or_default();
      let names_linked = text
        .lines()
        .flat_map(locations_in)
        .any(|location| location.repo.as_deref() == Some(input.spec.repo.as_str()));
      source_calls += u64::from(reads_source && names_linked);
      continue;
    }
    let targets = match call.name.as_str() {
      "Read" => call.input_str("file_path").map(str::to_owned).into_iter().collect(),
      "Bash" => analyze_bash(call.input_str("command").unwrap_or_default()).read_files,
      _ => continue,
    };
    let linked = targets
      .iter()
      .filter(|target| Path::new(target).is_absolute())
      .filter_map(|target| normalize_path(target, input.roots).ok())
      .filter(|relative| input.spec.contains(relative));
    read.extend(linked);
  }
  LinkedMetrics {
    linked_reads: read.len() as u64,
    linked_source_calls: source_calls,
    linked_path_created: input.path_created,
    linked_writes: input.writes.clone(),
    restore: input.restore.clone(),
  }
}

fn tool_metrics(transcript: &Transcript) -> ToolMetrics {
  let mut metrics = ToolMetrics::default();
  for call in &transcript.tool_calls {
    *metrics.by_name.entry(call.name.clone()).or_default() += 1;
    metrics.total += 1;
    if denied_by_bash_guard(call) {
      continue;
    }
    metrics.errors += u64::from(call.result.as_ref().is_some_and(|result| result.is_error));
    let bash_search = call.name == "Bash" && call.input_str("command").is_some_and(|command| analyze_bash(command).search);
    metrics.search_calls += u64::from(SEARCH_TOOLS.contains(&call.name.as_str()) || bash_search);
  }
  metrics
}

fn absorb_read(call: &ToolCall, roots: &[PathBuf], relevant: &BTreeSet<String>, ledger: &mut ReadLedger) {
  let targets: Vec<String> = match call.name.as_str() {
    "Read" => {
      ledger.read_calls += 1;
      call.input_str("file_path").map(str::to_owned).into_iter().collect()
    }
    "Bash" if denied_by_bash_guard(call) => return,
    "Bash" => {
      let analysis = analyze_bash(call.input_str("command").unwrap_or_default());
      ledger.unparsed += u64::from(analysis.unparsed_reads);
      if analysis.read_files.is_empty() {
        return;
      }
      ledger.bash_read_calls += 1;
      analysis.read_files
    }
    _ => return,
  };
  if targets.is_empty() {
    return;
  }
  let normalized: Vec<Result<String, String>> = targets.iter().map(|target| normalize_path(target, roots)).collect();
  let in_tree_relevant = normalized.iter().filter(|path| path.as_ref().is_ok_and(|path| relevant.contains(path))).count() as u64;
  let bytes = call.result_bytes();
  ledger.bytes += bytes;
  ledger.lines += call.result.as_ref().map_or(0, |result| result.lines());
  ledger.useful_bytes += bytes * in_tree_relevant / normalized.len() as u64;
  for path in normalized {
    match path {
      Ok(path) => ledger.files.insert(path),
      Err(outside) => ledger.outside.insert(outside),
    };
  }
}

fn tree_roots(tree: &Path, view: Option<&Path>, reported_cwd: Option<&str>) -> Vec<PathBuf> {
  let mut roots = Vec::new();
  for root in std::iter::once(tree).chain(view) {
    roots.push(root.to_owned());
    roots.extend(root.canonicalize().ok());
  }
  if let Some(cwd) = reported_cwd {
    roots.push(PathBuf::from(cwd));
  }
  roots.sort();
  roots.dedup();
  roots
}

pub fn normalize_path(raw: &str, roots: &[PathBuf]) -> Result<String, String> {
  let path = Path::new(raw);
  let relative = if path.is_absolute() {
    let lexical = lexical(path);
    match roots.iter().find_map(|root| lexical.strip_prefix(lexical_root(root)).ok()) {
      Some(relative) => relative.to_owned(),
      None => return Err(lexical.display().to_string()),
    }
  } else {
    path.to_owned()
  };
  let normalized = lexical(&relative);
  if normalized.as_os_str().is_empty() || normalized.starts_with("..") {
    return Err(raw.to_owned());
  }
  Ok(normalized.display().to_string())
}

fn lexical_root(root: &Path) -> PathBuf {
  lexical(root)
}

fn lexical(path: &Path) -> PathBuf {
  let mut result = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        if !result.pop() {
          result.push("..");
        }
      }
      other => result.push(other.as_os_str()),
    }
  }
  result
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasLocation {
  pub repo: Option<String>,
  pub path: String,
  pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AtlasItem {
  section: Option<String>,
  location: AtlasLocation,
  bytes: u64,
}

fn atlas_items(text: &str) -> Vec<AtlasItem> {
  let mut items: Vec<AtlasItem> = Vec::new();
  let mut section: Option<String> = None;
  for line in text.lines() {
    let bytes = line.len() as u64 + 1;
    if let Some(header) = section_header(line) {
      section = Some(header.to_owned());
      continue;
    }
    match locations_in(line).into_iter().next() {
      Some(location) => items.push(AtlasItem {
        section: section.clone(),
        location,
        bytes,
      }),
      None => {
        if let Some(item) = items.last_mut() {
          item.bytes += bytes;
        }
      }
    }
  }
  items
}

fn section_header(line: &str) -> Option<&'static str> {
  let trimmed = line.trim().trim_start_matches(['#', '=', '-', ' ']).trim_end_matches([':', '=', '-', ' ']);
  ATLAS_SECTIONS
    .iter()
    .find(|section| {
      trimmed
        .strip_prefix(**section)
        .is_some_and(|rest| rest.chars().all(|c| !c.is_alphabetic()) || (**section == OMITTED && is_reason_counts(rest)))
    })
    .copied()
}

fn is_reason_counts(rest: &str) -> bool {
  let Some((count, _)) = rest.strip_prefix(" (").and_then(|rest| rest.split_once("): ")) else {
    return false;
  };
  !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
}

pub fn locations_in(line: &str) -> Vec<AtlasLocation> {
  line
    .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ',' | '`' | '"' | '\''))
    .filter_map(parse_location)
    .collect()
}

fn parse_location(token: &str) -> Option<AtlasLocation> {
  let parts: Vec<&str> = token.split(':').collect();
  let path_index = parts.iter().position(|part| has_source_extension(part))?;
  let line = parts.get(path_index + 1).and_then(|part| part.split('-').next()).and_then(|part| part.parse().ok());
  let repo = path_index
    .checked_sub(1)
    .map(|index| parts[index])
    .filter(|repo| !repo.is_empty() && !repo.contains('/'))
    .map(str::to_owned);
  Some(AtlasLocation {
    repo,
    path: parts[path_index].trim_start_matches("./").to_owned(),
    line,
  })
}

fn has_source_extension(part: &str) -> bool {
  part
    .rsplit_once('.')
    .is_some_and(|(stem, extension)| !stem.is_empty() && SOURCE_EXTENSIONS.contains(&extension))
}

fn tree_path_for(location: &AtlasLocation, tree: &Path) -> String {
  match &location.repo {
    Some(repo) if tree.join(repo).join(&location.path).is_file() => format!("{repo}/{}", location.path),
    _ => location.path.clone(),
  }
}

fn atlas_useful_bytes(items: &[AtlasItem], relevant: &BTreeSet<String>, tree: &Path) -> u64 {
  items
    .iter()
    .filter(|item| {
      let path = tree_path_for(&item.location, tree);
      relevant.contains(&path) || relevant.contains(&item.location.path)
    })
    .map(|item| item.bytes)
    .sum()
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::transcript::{ResultInfo, ToolResult};
  use super::*;

  fn call(name: &str, input: serde_json::Value, result: &str, elapsed_ms: u64) -> ToolCall {
    ToolCall {
      id: format!("t{elapsed_ms}"),
      name: name.to_owned(),
      input,
      elapsed_ms: Some(elapsed_ms),
      result: Some(ToolResult {
        text: result.to_owned(),
        is_error: false,
        ..ToolResult::default()
      }),
      turn: 1,
    }
  }

  fn transcript() -> Transcript {
    let atlas_response = "fresh run=4 (+0 files, 3ms)\nMUST READ\n  acme-api:src/users/users.service.ts:16 UsersService.findOne [resolved]\n    async findOne(id) {\n    }\nSKIM\n  acme-api:src/users/users.controller.ts:14 findOne [resolved]\n";
    Transcript {
      tool_calls: vec![
        call("mcp__code-atlas__atlas_context", json!({"task": "x"}), atlas_response, 10),
        call("Read", json!({"file_path": "/runs/r/tree/acme-api/src/users/user.entity.ts"}), "1\tline\n2\tline", 20),
        call("Read", json!({"file_path": "/runs/r/tree/acme-api/src/main.ts"}), "1\tmain", 30),
        call("Bash", json!({"command": "cat acme-api/src/users/user.entity.ts acme-api/package.json"}), "aaaa", 40),
        call("Bash", json!({"command": "grep -rn deleted ."}), "x", 50),
        call("Grep", json!({"pattern": "x"}), "", 55),
        call("Read", json!({"file_path": "/etc/hosts"}), "h", 60),
        call("Edit", json!({"file_path": "/runs/r/tree/acme-api/src/users/user.entity.ts"}), "ok", 70),
      ],
      result: Some(ResultInfo {
        total_cost_usd: Some(0.5),
        num_turns: Some(9),
        duration_ms: Some(900),
        ..ResultInfo::default()
      }),
      ..Transcript::default()
    }
  }

  fn truth() -> GroundTruth {
    GroundTruth {
      relevant_files: vec![
        "acme-api/src/users/user.entity.ts".to_owned(),
        "acme-api/src/users/users.service.ts".to_owned(),
        "acme-api/src/users/users.module.ts".to_owned(),
      ],
      ..GroundTruth::default()
    }
  }

  #[test]
  fn should_compute_reads_relevance_and_timing() {
    let tree = tempfile::tempdir().unwrap();
    let tree_path = Path::new("/runs/r/tree");
    std::fs::create_dir_all(tree.path().join("acme-api/src/users")).unwrap();
    let changed: BTreeSet<String> = ["acme-api/src/users/user.entity.ts".to_owned()].into();
    let transcript = transcript();
    let metrics = compute_metrics(&MetricsInput {
      transcript: &transcript,
      view: None,
      tree: tree_path,
      ground_truth: &truth(),
      changed_files: &changed,
      wall_ms: Some(1000),
    });
    assert_eq!(
      metrics.reads.files,
      vec!["acme-api/package.json", "acme-api/src/main.ts", "acme-api/src/users/user.entity.ts"]
    );
    assert_eq!(metrics.reads.outside_tree, vec!["/etc/hosts"]);
    assert_eq!((metrics.reads.read_calls, metrics.reads.bash_read_calls), (3, 1));
    assert_eq!(metrics.reads.bytes, 13 + 6 + 4 + 1);
    assert_eq!(metrics.reads.lines, 2 + 1 + 1 + 1);
    assert_eq!(metrics.tools.search_calls, 2);
    assert_eq!(metrics.tools.total, 8);
    assert_eq!(metrics.atlas.calls, 1);
    assert_eq!(metrics.atlas.must_read_files, vec!["src/users/users.service.ts"]);
    assert_eq!(metrics.relevance.irrelevant_read, vec!["acme-api/package.json", "acme-api/src/main.ts"]);
    assert_eq!(metrics.relevance.relevant_read, vec!["acme-api/src/users/user.entity.ts"]);
    assert_eq!(
      metrics.relevance.relevant_missed,
      vec!["acme-api/src/users/users.module.ts", "acme-api/src/users/users.service.ts"]
    );
    assert_eq!(metrics.timing.first_edit_ms, Some(70));
    assert_eq!(metrics.timing.session_ms, Some(900));
    assert_eq!(metrics.tokens.cost_usd, Some(0.5));
  }

  #[test]
  fn should_credit_atlas_must_read_files_when_they_exist_under_the_repo_directory() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("acme-api/src/users")).unwrap();
    std::fs::write(tree.path().join("acme-api/src/users/users.service.ts"), "x").unwrap();
    let changed = BTreeSet::new();
    let transcript = transcript();
    let metrics = compute_metrics(&MetricsInput {
      transcript: &transcript,
      view: None,
      tree: tree.path(),
      ground_truth: &truth(),
      changed_files: &changed,
      wall_ms: None,
    });
    assert_eq!(metrics.atlas.must_read_files, vec!["acme-api/src/users/users.service.ts"]);
    assert_eq!(metrics.relevance.relevant_missed, vec!["acme-api/src/users/users.module.ts"]);
  }

  #[test]
  fn should_split_bash_bytes_between_relevant_and_irrelevant_targets_for_density() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("acme-api/src/users")).unwrap();
    std::fs::write(tree.path().join("acme-api/src/users/users.service.ts"), "x").unwrap();
    let transcript = transcript();
    let metrics = compute_metrics(&MetricsInput {
      transcript: &transcript,
      view: None,
      tree: tree.path(),
      ground_truth: &truth(),
      changed_files: &BTreeSet::new(),
      wall_ms: None,
    });
    let atlas_bytes_about_service =
      ("  acme-api:src/users/users.service.ts:16 UsersService.findOne [resolved]".len() + 1 + "    async findOne(id) {".len() + 1 + "    }".len() + 1) as u64;
    assert_eq!(metrics.density.useful_bytes, 4 / 2 + atlas_bytes_about_service);
    let context: u64 = transcript.tool_calls.iter().map(ToolCall::result_bytes).sum();
    assert_eq!(metrics.density.context_bytes, context);
  }

  #[test]
  fn should_count_no_read_for_a_bash_call_the_guard_refused() {
    let transcript = Transcript::parse(include_str!("../../tests/fixtures/forced-transcript.jsonl"), &[]);
    let metrics = compute_metrics(&MetricsInput {
      transcript: &transcript,
      view: None,
      tree: Path::new("/eval/runs/r/tree"),
      ground_truth: &GroundTruth::default(),
      changed_files: &BTreeSet::new(),
      wall_ms: None,
    });
    assert_eq!(metrics.reads.files, vec!["src/users/user.entity.ts"]);
    assert_eq!((metrics.reads.read_calls, metrics.reads.bash_read_calls, metrics.reads.unparsed_bash_reads), (1, 0, 0));
    assert_eq!(
      (metrics.tools.by_name.get("Bash"), metrics.tools.total, metrics.tools.errors, metrics.tools.search_calls),
      (Some(&3), 4, 0, 0)
    );
    assert_eq!(metrics.atlas_surface_denials, None);
    assert_eq!(serde_json::to_value(&metrics).unwrap().get("atlas_surface_denials"), None);
  }

  #[test]
  fn should_count_distinct_linked_files_read_and_atlas_calls_that_show_linked_source() {
    let context = "MUST READ\n  web:apps/acme-web/src/api/client.ts:3 createClient [resolved]\n  tree:acme-api/src/main.ts:1 main\n";
    let tree_only = "MUST READ\n  tree:acme-api/src/main.ts:1 main\n";
    let source = "web:apps/acme-web/src/api/client.ts:3-9\nexport function createClient() {}\n";
    let transcript = Transcript {
      tool_calls: vec![
        call("mcp__code-atlas__atlas_context", json!({"task": "x"}), context, 1),
        call("mcp__code-atlas__atlas_context", json!({"task": "y"}), tree_only, 2),
        call("mcp__code-atlas__atlas_symbol", json!({"symbol": "createClient", "mode": "source"}), source, 3),
        call("mcp__code-atlas__atlas_symbol", json!({"symbol": "createClient", "mode": "outline"}), source, 4),
        call("mcp__code-atlas__atlas_search", json!({"query": "client"}), source, 5),
        call("Read", json!({"file_path": "/s/ae-1/linked-view/apps/acme-web/src/api/client.ts"}), "1\tx", 6),
        call("Read", json!({"file_path": "/private/s/ae-1/linked-view/apps/acme-web/src/api/client.ts"}), "1\tx", 7),
        call("Bash", json!({"command": "cd /s/ae-1/linked-hold && cat apps/acme-web/src/config.ts package.json"}), "x", 8),
        call("Read", json!({"file_path": "/s/ae-1/linked-view/acme-api/src/main.ts"}), "1\tx", 9),
        call("Read", json!({"file_path": "apps/acme-web/src/api/session.ts"}), "1\tx", 10),
      ],
      ..Transcript::default()
    };
    let spec = LinkedSpec {
      paths: vec!["apps/acme-web".to_owned()],
      patch: None,
      repo: "web".to_owned(),
    };
    let roots = [
      PathBuf::from("/s/ae-1/linked-hold"),
      PathBuf::from("/s/ae-1/linked-view"),
      PathBuf::from("/private/s/ae-1/linked-view"),
    ];
    let input = LinkedInput {
      roots: &roots,
      spec: &spec,
      path_created: true,
      writes: Vec::new(),
      restore: LinkedRestore::Restored,
    };
    let metrics = linked_metrics(&transcript, &input);
    assert_eq!(
      metrics,
      LinkedMetrics {
        linked_reads: 2,
        linked_source_calls: 2,
        linked_path_created: true,
        linked_writes: Vec::new(),
        restore: LinkedRestore::Restored,
      }
    );
    assert!(!metrics.contaminates());
    assert_eq!(
      serde_json::to_value(&metrics).unwrap(),
      json!({"linked_reads": 2, "linked_source_calls": 2, "linked_path_created": true})
    );
    let truth = GroundTruth {
      relevant_files: vec!["apps/acme-web/src/api/client.ts".to_owned()],
      ..GroundTruth::default()
    };
    let unchanged = compute_metrics(&MetricsInput {
      transcript: &transcript,
      view: Some(Path::new("/s/ae-1/linked-view")),
      tree: Path::new("/s/ae-1/tree"),
      ground_truth: &truth,
      changed_files: &BTreeSet::new(),
      wall_ms: None,
    });
    assert_eq!(unchanged.relevance.relevant_read, vec!["apps/acme-web/src/api/client.ts"]);
    assert_eq!(serde_json::to_value(&unchanged).unwrap().get("linked"), None);
  }

  #[test]
  fn should_mark_linked_metrics_contaminating_when_the_session_changed_or_moved_the_linked_copies() {
    let spec = LinkedSpec {
      paths: vec!["apps/acme-web".to_owned()],
      patch: None,
      repo: "web".to_owned(),
    };
    let metrics = |writes: Vec<String>, restore: LinkedRestore| {
      linked_metrics(
        &Transcript::default(),
        &LinkedInput {
          roots: &[],
          spec: &spec,
          path_created: false,
          writes,
          restore,
        },
      )
    };
    let written = metrics(vec!["linked-view/apps/acme-web/src/a.ts".to_owned()], LinkedRestore::Restored);
    let moved = metrics(
      vec!["linked-hold/apps/acme-web".to_owned()],
      LinkedRestore::HoldMissing {
        paths: vec!["apps/acme-web".to_owned()],
      },
    );
    assert!(written.contaminates());
    assert!(moved.contaminates());
    assert_eq!(
      serde_json::to_value(&moved).unwrap(),
      json!({
        "linked_reads": 0,
        "linked_source_calls": 0,
        "linked_path_created": false,
        "linked_writes": ["linked-hold/apps/acme-web"],
        "restore": {"outcome": "hold_missing", "paths": ["apps/acme-web"]}
      })
    );
  }

  #[test]
  fn should_normalize_paths_against_any_tree_root() {
    let roots = vec![PathBuf::from("/var/t/tree"), PathBuf::from("/private/var/t/tree")];
    assert_eq!(normalize_path("/private/var/t/tree/src/../src/a.rs", &roots), Ok("src/a.rs".to_owned()));
    assert_eq!(normalize_path("./src/a.rs", &roots), Ok("src/a.rs".to_owned()));
    assert_eq!(normalize_path("../outside.rs", &roots), Err("../outside.rs".to_owned()));
    assert_eq!(normalize_path("/etc/passwd", &roots), Err("/etc/passwd".to_owned()));
  }

  #[test]
  fn should_parse_atlas_locations_with_and_without_repo_prefixes() {
    assert_eq!(
      locations_in("  billing:src/money.rs:12-20 to_cents [heuristic 0.72]"),
      vec![AtlasLocation {
        repo: Some("billing".to_owned()),
        path: "src/money.rs".to_owned(),
        line: Some(12)
      }]
    );
    assert_eq!(
      locations_in("(crates/atlas-core/src/lib.rs:3)"),
      vec![AtlasLocation {
        repo: None,
        path: "crates/atlas-core/src/lib.rs".to_owned(),
        line: Some(3)
      }]
    );
    assert_eq!(locations_in("UsersService.findOne returns a user"), Vec::<AtlasLocation>::new());
  }

  #[test]
  fn should_attribute_omitted_bytes_to_omitted_in_the_listed_and_the_collapsed_form() {
    let listed = "SKIM (1)\n  render function web:src/view.ts:3-9  why: called\nOMITTED (2)\n  price function web:src/price.ts:4 [budget]\n  total function web:src/total.ts:8 [low relevance]\n";
    let collapsed = "SKIM (1)\n  render function web:src/view.ts:3-9  why: called\nOMITTED (16): 11 low relevance; 2 budget (scores with explain=true)\n  rates function web:src/rates.ts:1 as rates (3 pack items) [already held by the caller]\n";
    let sections = |text: &str| -> Vec<(Option<String>, String, u64)> { atlas_items(text).into_iter().map(|item| (item.section, item.location.path, item.bytes)).collect() };
    assert_eq!(
      sections(listed),
      [
        (Some("SKIM".to_owned()), "src/view.ts".to_owned(), 51),
        (Some("OMITTED".to_owned()), "src/price.ts".to_owned(), 45),
        (Some("OMITTED".to_owned()), "src/total.ts".to_owned(), 52),
      ]
    );
    assert_eq!(
      sections(collapsed),
      [
        (Some("SKIM".to_owned()), "src/view.ts".to_owned(), 51),
        (Some("OMITTED".to_owned()), "src/rates.ts".to_owned(), 89),
      ]
    );
    assert_eq!(section_header("OMITTED (16): 11 low relevance; 2 budget (scores with explain=true)"), Some("OMITTED"));
    assert_eq!(section_header("OMITTED (3)"), Some("OMITTED"));
    assert_eq!(section_header("TESTS (2): run them"), None);
  }
}
