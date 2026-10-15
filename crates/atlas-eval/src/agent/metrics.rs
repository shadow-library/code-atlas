use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::access::OutOfTreeAccess;
use super::bash::analyze_bash;
use super::claude::ATLAS_TOOL_PREFIX;
use super::hooks::{HookMetrics, hook_metrics};
use super::spec::GroundTruth;
use super::transcript::{TokenUsage, ToolCall, Transcript};

const EDIT_TOOLS: [&str; 2] = ["Edit", "Write"];
const SEARCH_TOOLS: [&str; 2] = ["Grep", "Glob"];
const SOURCE_EXTENSIONS: [&str; 12] = ["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "toml", "json", "md", "yaml", "yml"];
const ATLAS_SECTIONS: [&str; 7] = ["MUST READ", "SKIM", "REUSE CANDIDATES", "IMPACT", "TESTS", "OMITTED", "UNCERTAIN"];
const MUST_READ: &str = "MUST READ";

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
}

pub struct MetricsInput<'a> {
  pub transcript: &'a Transcript,
  pub tree: &'a Path,
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
  let roots = tree_roots(input.tree, transcript.init.as_ref().and_then(|init| init.cwd.as_deref()));
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
  }
}

fn tool_metrics(transcript: &Transcript) -> ToolMetrics {
  let mut metrics = ToolMetrics::default();
  for call in &transcript.tool_calls {
    *metrics.by_name.entry(call.name.clone()).or_default() += 1;
    metrics.total += 1;
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

fn tree_roots(tree: &Path, reported_cwd: Option<&str>) -> Vec<PathBuf> {
  let mut roots = vec![tree.to_owned()];
  if let Ok(canonical) = tree.canonicalize() {
    roots.push(canonical);
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
    .find(|section| trimmed.starts_with(*section) && trimmed[section.len()..].chars().all(|c| !c.is_alphabetic()))
    .copied()
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
}
