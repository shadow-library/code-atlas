use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{
  BodyFingerprintInput, CoreError, EdgeKind, Entity, EntityId, EntityKey, EntityKind, ExportFact, FileFacts, ImportFact, Language, LanguageFamily, ParamFact, ParseStatus, RelPath,
  RepoName, SymbolFact, is_test_code,
};
use atlas_engine::EngineError;
use atlas_semantic::{RELATION_EQUIVALENT_TO, RELATION_SIMILAR_TO};
use atlas_similarity::{SMALL_BODY_TOKENS, SMALL_PAIR_MIN_JACCARD, SMALL_SHINGLE_SIZE, SimilarityIndex, StructuralMatch};
use atlas_store::{EntityFilter, FreshnessPolicy, InferredQuery};
use serde::Serialize;

use crate::QueryEngine;
use crate::error::{QueryError, QueryResult};
use crate::graph::Graph;
use crate::model::{Caveat, CertaintyTag, EntityRef, Response, Targeted};
use crate::policy::{Direction, InferredAdmission, Policy};
use crate::queries::common::{is_test, language, package_key, resolve_one};
use crate::queries::reuse::{CandidateQuery, CandidateScope, access, lexical_and_signature_candidates, shape_of, signature_shape};
use crate::queries::search::rounded;
use crate::queries::tests_for::direct_tests;
use crate::target::TargetFilter;

const DEFAULT_THRESHOLD: f64 = 0.5;
const IDENTICAL_JACCARD: f64 = 0.95;
const STRUCTURAL_CANDIDATE_FACTOR: usize = 4;
const SHAPE_ONLY_LIMIT: usize = 5;
const SHAPE_ONLY_NOTE: &str = "shape-only (small body, likely boilerplate)";
const MAX_DRAFT_BYTES: usize = 64 * 1024;
const MAX_DRAFT_SYMBOLS: usize = 8;
const WRAPPER_NAME: &str = "__draft";
const FRAGMENT_NAME: &str = "(fragment)";
const NAME_ONLY_FLOOR: f64 = 0.6;
const NAME_ONLY_LIMIT: usize = 5;
const DECLARATION_KEYWORDS: [&str; 3] = ["const", "let", "var"];
const RESIDUE_KEYWORDS: [&str; 5] = ["export", "default", "declare", "pub", "async"];
const SHARED_SEGMENTS: [&str; 6] = ["shared", "common", "util", "utils", "lib", "core"];

#[derive(Debug, Clone, PartialEq)]
pub struct SimilarRequest {
  pub target: String,
  pub code: Option<CodeQuery>,
  pub filter: TargetFilter,
  pub limit: usize,
  pub threshold: f64,
  pub include_stale: bool,
}

impl SimilarRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      code: None,
      filter: TargetFilter::default(),
      limit: 10,
      threshold: DEFAULT_THRESHOLD,
      include_stale: false,
    }
  }

  pub fn from_code(language: Language, text: impl Into<String>) -> Self {
    Self {
      code: Some(CodeQuery { language, text: text.into() }),
      ..Self::new(String::new())
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeQuery {
  pub language: Language,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum SimilaritySignal {
  Structural {
    jaccard: f64,
  },
  SmallStructural {
    jaccard: f64,
  },
  Lexical {
    score: f64,
    terms: Vec<String>,
  },
  SignatureShape {
    shape: String,
  },
  Inferred {
    verdict: String,
    rationale: String,
    differences: Vec<String>,
    certainty: CertaintyTag,
  },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsumerSummary {
  pub callers: usize,
  pub uncertain: usize,
  pub repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClusterMember {
  pub entity: EntityRef,
  pub language: String,
  pub signals: Vec<SimilaritySignal>,
  pub verdict: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub importability: Option<String>,
  pub consumers: ConsumerSummary,
  pub tests: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedHome {
  pub language: String,
  pub package: Option<String>,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShapeOnlyMatch {
  pub entity: EntityRef,
  pub jaccard: f64,
  pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimilarAnswer {
  pub target: ClusterMember,
  pub members: Vec<ClusterMember>,
  pub shape_only: Vec<ShapeOnlyMatch>,
  pub shared_home: Vec<SharedHome>,
  pub coupling_risks: Vec<String>,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftOrigin {
  Declared,
  WrappedFragment,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DraftSymbol {
  pub name: String,
  pub kind: EntityKind,
  pub lines: u32,
  pub tokens: u32,
  pub origin: DraftOrigin,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchScope {
  pub symbols: usize,
  pub repos: Vec<String>,
  pub languages: Vec<String>,
  pub threshold: f64,
  pub min_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DraftMatches {
  pub draft: DraftSymbol,
  pub members: Vec<ClusterMember>,
  pub related: Vec<ClusterMember>,
  pub shape_only: Vec<ShapeOnlyMatch>,
  pub searched: SearchScope,
  pub caveats: Vec<Caveat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DraftSimilarAnswer {
  pub language: String,
  pub lines: u32,
  pub drafts: Vec<DraftMatches>,
  pub caveats: Vec<Caveat>,
}

enum Mode<'r> {
  Target,
  Code(&'r CodeQuery),
}

impl SimilarRequest {
  fn mode(&self) -> QueryResult<Mode<'_>> {
    let target = self.target.trim();
    match (&self.code, target.is_empty()) {
      (Some(_), false) => Err(QueryError::invalid("give either a target or code to compare, not both")),
      (Some(code), true) => Ok(Mode::Code(code)),
      (None, false) => Ok(Mode::Target),
      (None, true) => Err(QueryError::invalid("give a target symbol or code to compare")),
    }
  }
}

struct DraftSource {
  family: LanguageFamily,
  name: String,
  kind: EntityKind,
  signature: Option<String>,
  params: Vec<ParamFact>,
  lines: u32,
  body: Option<BodyFingerprintInput>,
  origin: DraftOrigin,
}

struct DraftAnalysis {
  sources: Vec<DraftSource>,
  caveats: Vec<Caveat>,
  test_code: bool,
}

struct DraftContext {
  repo: Option<String>,
  allow_tests: bool,
}

enum Admission {
  Admit,
  TestCode,
  Unrelated,
}

fn note(message: impl Into<String>) -> Caveat {
  Caveat::Note { message: message.into() }
}

fn wrap_fragment(language: Language, text: &str) -> String {
  match language.family() {
    LanguageFamily::Rust => format!("fn {WRAPPER_NAME}() {{\n{text}\n}}\n"),
    _ => format!("async function {WRAPPER_NAME}() {{\n{text}\n}}\n"),
  }
}

fn analyse_text(query: &QueryEngine, language: Language, text: &str) -> QueryResult<FileFacts> {
  query.engine().analyze_text(language, text).map_err(|error| match error {
    EngineError::Core(CoreError::UnsupportedPath(_)) => QueryError::invalid(format!("no analyzer for language {language}")),
    other => QueryError::from(other),
  })
}

fn syntax_errors(facts: &FileFacts) -> Option<u32> {
  match &facts.parse_status {
    ParseStatus::Partial { error_count, .. } => Some(*error_count),
    _ => None,
  }
}

fn strip_comments(text: &str) -> String {
  let mut kept = String::with_capacity(text.len());
  let mut chars = text.chars().peekable();
  while let Some(c) = chars.next() {
    match (c, chars.peek()) {
      ('/', Some('/')) => while chars.next_if(|next| *next != '\n').is_some() {},
      ('/', Some('*')) => {
        chars.next();
        let mut previous = ' ';
        for next in chars.by_ref() {
          if previous == '*' && next == '/' {
            break;
          }
          previous = next;
        }
        kept.push(' ');
      }
      _ => kept.push(c),
    }
  }
  kept
}

fn leading_keywords_start(bytes: &[u8], mut start: usize) -> usize {
  loop {
    let word_end = bytes[..start].iter().rposition(|byte| !byte.is_ascii_whitespace()).map_or(0, |last| last + 1);
    let word_start = bytes[..word_end]
      .iter()
      .rposition(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
      .map_or(0, |last| last + 1);
    let word = std::str::from_utf8(&bytes[word_start..word_end]).unwrap_or_default();
    if word.is_empty() || !(RESIDUE_KEYWORDS.contains(&word) || DECLARATION_KEYWORDS.contains(&word)) {
      return start;
    }
    start = word_start;
  }
}

fn declaration_ranges(text: &str, facts: &FileFacts) -> Vec<(usize, usize)> {
  let bytes = text.as_bytes();
  let spans = facts
    .symbols
    .iter()
    .filter(|symbol| symbol.parent.is_none() && !matches!(symbol.kind, EntityKind::Constant | EntityKind::Variable))
    .map(|symbol| symbol.span)
    .chain(facts.symbols.iter().flat_map(|symbol| symbol.attributes.iter().map(|attribute| attribute.span)))
    .chain(facts.imports.iter().map(ImportFact::span))
    .chain(facts.exports.iter().map(ExportFact::span));
  spans
    .map(|span| {
      let mut end = (span.end_byte as usize).min(bytes.len());
      if bytes.get(end) == Some(&b';') {
        end += 1;
      }
      (leading_keywords_start(bytes, (span.start_byte as usize).min(bytes.len())), end)
    })
    .collect()
}

fn blank_bytes(text: &str, ranges: &[(usize, usize)], keep_ranges: bool) -> Option<String> {
  let mut inside = vec![false; text.len()];
  for (start, end) in ranges {
    inside[*start.min(&text.len())..*end.min(&text.len())].fill(true);
  }
  let bytes = text
    .bytes()
    .zip(inside)
    .map(|(byte, inside)| if byte == b'\n' || inside == keep_ranges { byte } else { b' ' })
    .collect();
  String::from_utf8(bytes).ok()
}

struct Residue {
  text: String,
  lines: u32,
}

fn statement_residue(text: &str, ranges: &[(usize, usize)]) -> Option<Residue> {
  let residue = blank_bytes(text, ranges, false)?;
  let stripped = strip_comments(&residue);
  let has_statements = stripped
    .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
    .any(|word| !word.is_empty() && !RESIDUE_KEYWORDS.contains(&word));
  let lines = u32::try_from(stripped.lines().filter(|line| !line.trim().is_empty()).count()).unwrap_or(u32::MAX);
  has_statements.then_some(Residue { text: residue, lines })
}

fn analyse_draft(query: &QueryEngine, code: &CodeQuery, min_tokens: usize) -> QueryResult<DraftAnalysis> {
  let facts = analyse_text(query, code.language, &code.text)?;
  if let ParseStatus::Failed { reason } = &facts.parse_status {
    return Err(QueryError::Analysis(reason.clone()));
  }
  let mut sources = draft_sources(&facts);
  let declared = sources.len();
  let mut errors = syntax_errors(&facts);
  let mut caveats = Vec::new();
  if declared > MAX_DRAFT_SYMBOLS {
    caveats.push(note(format!("the draft declares {declared} functions; only the first {MAX_DRAFT_SYMBOLS} were compared")));
    sources.truncate(MAX_DRAFT_SYMBOLS);
  }
  let ranges = declaration_ranges(&code.text, &facts);
  if let Some(residue) = statement_residue(&code.text, &ranges) {
    if declared > 0 && errors.is_some() {
      let declared_text = blank_bytes(&code.text, &ranges, true).unwrap_or_default();
      errors = syntax_errors(&analyse_text(query, code.language, &declared_text)?);
    }
    let wrapped = analyse_text(query, code.language, &wrap_fragment(code.language, &residue.text))?;
    let source = match wrapped.parse_status {
      ParseStatus::Failed { .. } => None,
      _ => top_level_callables(&wrapped)
        .into_iter()
        .find(|symbol| symbol.name == WRAPPER_NAME)
        .map(|symbol| DraftSource {
          name: FRAGMENT_NAME.to_owned(),
          lines: residue.lines,
          origin: DraftOrigin::WrappedFragment,
          ..draft_source(&wrapped, symbol)
        })
        .filter(|source| declared == 0 || source.body.as_ref().is_some_and(|body| body.normalized_tokens.len() >= min_tokens)),
    };
    let fragment_errors = syntax_errors(&wrapped);
    errors = match declared {
      0 => fragment_errors,
      _ => Some(errors.unwrap_or(0) + fragment_errors.unwrap_or(0)).filter(|count| *count > 0),
    };
    if let Some(source) = source {
      match declared {
        0 => caveats.push(note(
          "the draft declares no function; its statements were compared as the body of a function, by structure only",
        )),
        _ => caveats.push(note(
          "statements outside the declared functions were also compared as the body of a function, by structure only",
        )),
      }
      sources.push(source);
    }
  }
  if let Some(count) = errors {
    caveats.push(note(format!("the draft has syntax errors ({count}); only the parts that parse were compared")));
  }
  if sources.is_empty() {
    caveats = vec![note("no function, method or statement body was found in the draft, so nothing was compared")];
  }
  Ok(DraftAnalysis {
    sources,
    caveats,
    test_code: facts.symbols.iter().any(|symbol| symbol.test.is_some()),
  })
}

fn top_level_callables(facts: &FileFacts) -> Vec<&SymbolFact> {
  let nested_in_callable = |symbol: &SymbolFact| {
    let mut parent = symbol.parent;
    while let Some(id) = parent {
      let Some(owner) = facts.symbols.iter().find(|candidate| candidate.local == id) else {
        return false;
      };
      if callable(&owner.kind) {
        return true;
      }
      parent = owner.parent;
    }
    false
  };
  facts
    .symbols
    .iter()
    .filter(|symbol| callable(&symbol.kind) && symbol.test.is_none() && !nested_in_callable(symbol))
    .collect()
}

fn draft_source(facts: &FileFacts, symbol: &SymbolFact) -> DraftSource {
  DraftSource {
    family: facts.file.language.family(),
    name: symbol.name.clone(),
    kind: symbol.kind.clone(),
    signature: symbol.signature.clone(),
    params: symbol.params.clone(),
    lines: symbol.span.end_line.saturating_sub(symbol.span.start_line) + 1,
    body: symbol.body.clone(),
    origin: DraftOrigin::Declared,
  }
}

fn draft_sources(facts: &FileFacts) -> Vec<DraftSource> {
  top_level_callables(facts).into_iter().map(|symbol| draft_source(facts, symbol)).collect()
}

impl QueryEngine {
  pub fn similar_to_code(&self, request: &SimilarRequest) -> QueryResult<Response<DraftSimilarAnswer>> {
    let code = match request.mode()? {
      Mode::Code(code) => code,
      Mode::Target => return Err(QueryError::invalid("no code given; use `similar` to compare an indexed symbol")),
    };
    if !request.filter.kinds.is_empty() {
      return Err(QueryError::invalid("the kinds filter does not apply to code drafts; only the repo filter does"));
    }
    let repos: Vec<String> = self.repos().iter().map(ToString::to_string).collect();
    if let Some(repo) = request.filter.repo.as_ref().filter(|repo| !repos.contains(repo)) {
      return Err(QueryError::invalid(format!("unknown repo {repo:?}; this project has {}", repos.join(", "))));
    }
    if code.text.trim().is_empty() {
      return Err(QueryError::invalid("the code to compare is empty"));
    }
    if code.text.len() > MAX_DRAFT_BYTES {
      return Err(QueryError::invalid(format!(
        "the code to compare is {} bytes, over the {MAX_DRAFT_BYTES} byte limit; pass the function or fragment, not the whole file",
        code.text.len()
      )));
    }
    let freshness = self.barrier()?;
    let index = self.engine().similarity();
    let analysis = analyse_draft(self, code, index.config().min_tokens.max(SMALL_SHINGLE_SIZE))?;
    let text_lines = u32::try_from(code.text.lines().count()).unwrap_or(u32::MAX);
    let policy = Policy {
      min_confidence: 0.0,
      include_stale: request.include_stale,
    };
    let scope = SearchScope {
      symbols: index.len(),
      repos: request.filter.repo.clone().map_or(repos, |repo| vec![repo]),
      languages: Vec::new(),
      threshold: request.threshold,
      min_tokens: index.config().min_tokens,
    };
    let context = DraftContext {
      repo: request.filter.repo.clone(),
      allow_tests: analysis.test_code,
    };
    let drafts = self.read(policy, |graph| {
      let mut drafts = analysis
        .sources
        .iter()
        .map(|source| draft_matches(graph, &index, request, source, &context, &scope))
        .collect::<QueryResult<Vec<_>>>()?;
      let empty = |draft: &DraftMatches| draft.members.is_empty() && draft.related.is_empty() && draft.shape_only.is_empty();
      if drafts.iter().any(empty) {
        for draft in drafts.iter_mut().filter(|draft| empty(draft)) {
          draft.searched.languages = indexed_languages(graph, &draft.searched.repos, analysis.sources[0].family)?;
        }
      }
      Ok(drafts)
    })?;
    let answer = DraftSimilarAnswer {
      language: code.language.to_string(),
      lines: text_lines,
      drafts,
      caveats: analysis.caveats,
    };
    Ok(Response { freshness, answer })
  }

  pub fn similar(&self, request: &SimilarRequest) -> QueryResult<Response<Targeted<SimilarAnswer>>> {
    if matches!(request.mode()?, Mode::Code(_)) {
      return Err(QueryError::invalid("code drafts are answered by `similar_to_code`"));
    }
    let freshness = self.barrier()?;
    let index = self.engine().similarity();
    let policy = Policy {
      min_confidence: 0.0,
      include_stale: request.include_stale,
    };
    let answer = self.read(policy, |graph| -> QueryResult<Targeted<SimilarAnswer>> {
      let target = match snippet_target(self, graph, &request.target)? {
        Some(target) => target,
        None => match resolve_one(self, graph, &request.target, &request.filter)? {
          Ok(target) => target,
          Err(outcome) => return Ok(outcome),
        },
      };
      similar(graph, &index, &target, request).map(Targeted::Found)
    })?;
    Ok(Response { freshness, answer })
  }
}

fn snippet_target(query: &QueryEngine, graph: &Graph<'_, '_>, spec: &str) -> QueryResult<Option<Entity>> {
  let Some((location, range)) = spec.rsplit_once(':') else { return Ok(None) };
  let Some((start, end)) = range.split_once('-') else { return Ok(None) };
  let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
    return Ok(None);
  };
  let (repo, path) = match location.split_once(':') {
    Some((repo, path)) => (Some(repo), path),
    None => (None, location),
  };
  let path = RelPath::new(path).map_err(|_| QueryError::invalid(format!("invalid snippet path {path:?}")))?;
  let mut best: Option<Entity> = None;
  for candidate in query.repos().iter().filter(|candidate| repo.is_none_or(|repo| candidate.as_str() == repo)) {
    for entity in graph.tx().entities_in_file(candidate, &path, &EntityFilter::default())? {
      let Some(span) = entity.span.filter(|span| entity.kind.is_code_symbol() && span.start_line <= start && end <= span.end_line) else {
        continue;
      };
      let size = span.end_byte - span.start_byte;
      if best.as_ref().and_then(|best| best.span).is_none_or(|current| size < current.end_byte - current.start_byte) {
        best = Some(entity);
      }
    }
  }
  best
    .map(Some)
    .ok_or_else(|| QueryError::invalid(format!("no indexed symbol encloses {spec}; snippets must lie inside a function or type")))
}

fn callable(kind: &EntityKind) -> bool {
  matches!(kind, EntityKind::Function | EntityKind::Method)
}

fn comparable(target: &Entity, candidate: &Entity) -> bool {
  candidate.id != target.id && comparable_to(callable(&target.kind), candidate)
}

fn comparable_kind(subject_callable: bool, candidate: &Entity) -> bool {
  subject_callable == callable(&candidate.kind) && candidate.kind.is_code_symbol() && candidate.kind != EntityKind::Field
}

fn comparable_to(subject_callable: bool, candidate: &Entity) -> bool {
  !is_test(candidate) && comparable_kind(subject_callable, candidate)
}

fn collect_structural(found: Vec<StructuralMatch>, signals: &mut BTreeMap<EntityId, Vec<SimilaritySignal>>) -> BTreeMap<EntityId, f64> {
  let mut small_found = BTreeMap::new();
  for found in found {
    if found.small {
      small_found.insert(found.entity, rounded(found.exact_jaccard));
      continue;
    }
    signals.entry(found.entity).or_default().push(SimilaritySignal::Structural {
      jaccard: rounded(found.exact_jaccard),
    });
  }
  small_found
}

fn corroborate_small(
  graph: &Graph<'_, '_>,
  shape: Option<&String>,
  small_found: BTreeMap<EntityId, f64>,
  signals: &mut BTreeMap<EntityId, Vec<SimilaritySignal>>,
  comparable: impl Fn(&Entity) -> bool,
) -> QueryResult<Vec<ShapeOnlyMatch>> {
  let mut shape_only = Vec::new();
  for (id, jaccard) in small_found {
    let corroborated = signals.get(&id).is_some_and(|found| {
      found
        .iter()
        .any(|signal| matches!(signal, SimilaritySignal::Lexical { .. } | SimilaritySignal::Inferred { .. }))
    });
    if !corroborated {
      let Some(entity) = graph.entity(id)? else { continue };
      if comparable(&entity) {
        shape_only.push(ShapeOnlyMatch {
          entity: EntityRef::of(&entity),
          jaccard,
          note: SHAPE_ONLY_NOTE.to_owned(),
        });
      }
      continue;
    }
    let found = signals.entry(id).or_default();
    found.push(SimilaritySignal::SmallStructural { jaccard });
    add_signature_shape(graph, shape, id, found)?;
  }
  shape_only.sort_by(|left, right| right.jaccard.total_cmp(&left.jaccard).then_with(|| left.entity.id.cmp(&right.entity.id)));
  shape_only.truncate(SHAPE_ONLY_LIMIT);
  Ok(shape_only)
}

fn structural_reason(status: &atlas_similarity::StructuralStatus) -> String {
  match status {
    atlas_similarity::StructuralStatus::Unknown => "the target is not in the structural index".to_owned(),
    atlas_similarity::StructuralStatus::NoBody => "the target has no body".to_owned(),
    atlas_similarity::StructuralStatus::TooSmall { tokens } => format!("too small for structural comparison ({tokens} tokens)"),
    atlas_similarity::StructuralStatus::LowEntropy { tokens } => format!("too repetitive for structural comparison ({tokens} tokens, few distinct tokens)"),
    atlas_similarity::StructuralStatus::Indexed { tokens } => format!("indexed ({tokens} tokens)"),
  }
}

fn similar(graph: &Graph<'_, '_>, index: &SimilarityIndex, target: &Entity, request: &SimilarRequest) -> QueryResult<SimilarAnswer> {
  let mut signals: BTreeMap<EntityId, Vec<SimilaritySignal>> = BTreeMap::new();
  let mut caveats = Vec::new();
  let small_found = collect_structural(
    index.near_duplicates(target.id, request.threshold, request.limit * STRUCTURAL_CANDIDATE_FACTOR),
    &mut signals,
  );
  let structural = index.structural_status(target.id);
  if let atlas_similarity::StructuralStatus::Indexed { tokens } = structural
    && tokens < SMALL_BODY_TOKENS
    && request.threshold < SMALL_PAIR_MIN_JACCARD
  {
    caveats.push(Caveat::Note {
      message: format!(
        "the target is a small body ({tokens} tokens): small-body structural matches are held to a minimum score of {SMALL_PAIR_MIN_JACCARD:.2}, overriding the requested threshold {:.2}",
        request.threshold
      ),
    });
  }
  if !matches!(structural, atlas_similarity::StructuralStatus::Indexed { .. }) {
    caveats.push(Caveat::Note {
      message: format!(
        "no structural fingerprint for the target: {}; only lexical and inferred signals apply",
        structural_reason(&structural)
      ),
    });
  }
  let text = format!("{} {}", target.name, target.signature.clone().unwrap_or_default());
  let target_shape = signature_shape(target);
  let query = CandidateQuery {
    text: &text,
    shape: target_shape.as_deref(),
    exclude: Some(target.id),
    fetch: request.limit * 3,
    scope: CandidateScope::default(),
  };
  for candidate in lexical_and_signature_candidates(graph, index, &query)?.found {
    let found = signals.entry(candidate.entity.id).or_default();
    found.push(SimilaritySignal::Lexical {
      score: rounded(candidate.score),
      terms: candidate.terms,
    });
    if let Some(shape) = target_shape.clone().filter(|_| candidate.same_shape) {
      found.push(SimilaritySignal::SignatureShape { shape });
    }
  }
  for (id, found) in signals.iter_mut() {
    add_signature_shape(graph, target_shape.as_ref(), *id, found)?;
  }
  let (hidden_stale, hidden_low) = inferred_signals(graph, target, &mut signals)?;
  let shape_only = corroborate_small(graph, target_shape.as_ref(), small_found, &mut signals, |entity| comparable(target, entity))?;
  if hidden_stale + hidden_low > 0 {
    caveats.push(Caveat::InferredHidden {
      stale: hidden_stale,
      low_confidence: hidden_low,
    });
  }
  let mut members = Vec::new();
  for (id, signals) in signals {
    let Some(entity) = graph.entity(id)? else { continue };
    if !comparable(target, &entity) && !signals.iter().any(|signal| matches!(signal, SimilaritySignal::Inferred { .. })) {
      continue;
    }
    members.push((entity, signals));
  }
  members.sort_by(|(left, left_signals), (right, right_signals)| strength(right_signals).total_cmp(&strength(left_signals)).then_with(|| left.key.cmp(&right.key)));
  members.truncate(request.limit);
  let target_member = member(graph, target, Vec::new())?;
  let mut views = Vec::new();
  for (entity, signals) in &members {
    views.push(member(graph, entity, signals.clone())?);
  }
  let mut cluster: Vec<&Entity> = vec![target];
  cluster.extend(
    members
      .iter()
      .filter(|(entity, signals)| consolidation_candidate(target, entity, signals))
      .map(|(entity, _)| entity),
  );
  let (shared_home, coupling_risks) = consolidation(graph, &cluster)?;
  caveats.push(Caveat::Note {
    message: "similarity is evidence, not a decision: only an inferred equivalence verdict speaks to behaviour".to_owned(),
  });
  Ok(SimilarAnswer {
    target: target_member,
    members: views,
    shape_only,
    shared_home,
    coupling_risks,
    caveats,
  })
}

fn indexed_languages(graph: &Graph<'_, '_>, repos: &[String], family: LanguageFamily) -> QueryResult<Vec<String>> {
  let mut languages = BTreeSet::new();
  for repo in repos.iter().filter_map(|repo| RepoName::new(repo).ok()) {
    let files = graph.tx().files_in_repo(&repo)?;
    languages.extend(
      files
        .into_iter()
        .filter_map(|file| file.language)
        .filter(|language| language.family() == family)
        .map(|language| language.to_string()),
    );
  }
  Ok(languages.into_iter().collect())
}

fn admission(source: &DraftSource, context: &DraftContext, entity: &Entity) -> Admission {
  let other_repo = context.repo.as_deref().is_some_and(|repo| entity.repo.as_ref().is_none_or(|found| found.as_str() != repo));
  if !comparable_kind(callable(&source.kind), entity) || language(entity) != Some(source.family) || other_repo {
    return Admission::Unrelated;
  }
  match !context.allow_tests && is_test_code(entity) {
    true => Admission::TestCode,
    false => Admission::Admit,
  }
}

fn structural_score(signals: &[SimilaritySignal]) -> Option<f64> {
  signals.iter().find_map(|signal| match signal {
    SimilaritySignal::Structural { jaccard } | SimilaritySignal::SmallStructural { jaccard } => Some(*jaccard),
    _ => None,
  })
}

fn lexical_score(signals: &[SimilaritySignal]) -> f64 {
  signals
    .iter()
    .filter_map(|signal| match signal {
      SimilaritySignal::Lexical { score, .. } => Some(*score),
      _ => None,
    })
    .fold(0.0, f64::max)
}

fn draft_view(graph: &Graph<'_, '_>, entity: &Entity, signals: Vec<SimilaritySignal>) -> QueryResult<ClusterMember> {
  let mut view = member(graph, entity, signals)?;
  view.importability = Some(format!("{}; behaviour unverified", access(graph, entity, None)?));
  Ok(view)
}

fn draft_matches(
  graph: &Graph<'_, '_>,
  index: &SimilarityIndex,
  request: &SimilarRequest,
  source: &DraftSource,
  context: &DraftContext,
  scope: &SearchScope,
) -> QueryResult<DraftMatches> {
  let mut signals: BTreeMap<EntityId, Vec<SimilaritySignal>> = BTreeMap::new();
  let mut caveats = Vec::new();
  let excluded: RefCell<BTreeSet<EntityId>> = RefCell::new(BTreeSet::new());
  let mut lexical_excluded: Vec<(EntityId, f64)> = Vec::new();
  let min_tokens = scope.min_tokens.max(SMALL_SHINGLE_SIZE);
  let lexical_only = match source.origin {
    DraftOrigin::Declared => "; only lexical signals apply",
    DraftOrigin::WrappedFragment => "",
  };
  let mut small_found = BTreeMap::new();
  match &source.body {
    None => caveats.push(note(format!("the draft symbol has no body, so no structural fingerprint exists{lexical_only}"))),
    Some(body) if body.normalized_tokens.len() < min_tokens => caveats.push(note(format!(
      "the draft body is too small for structural comparison ({} tokens, minimum {min_tokens}){lexical_only}",
      body.token_count
    ))),
    Some(body) => {
      let found = index.near_duplicates_of_tokens(&body.normalized_tokens, request.threshold, request.limit * STRUCTURAL_CANDIDATE_FACTOR);
      small_found = collect_structural(found, &mut signals);
      if body.normalized_tokens.len() < SMALL_BODY_TOKENS && request.threshold < SMALL_PAIR_MIN_JACCARD {
        caveats.push(note(format!(
          "the draft is a small body ({} tokens): small-body structural matches are held to a minimum score of {SMALL_PAIR_MIN_JACCARD:.2}, overriding the requested threshold {:.2}",
          body.token_count, request.threshold
        )));
      }
    }
  }
  let shape = source.signature.as_deref().and_then(|signature| shape_of(signature, &source.params, Some(source.family)));
  if source.origin == DraftOrigin::Declared {
    let text = format!("{} {}", source.name, source.signature.clone().unwrap_or_default());
    let query = CandidateQuery {
      text: &text,
      shape: shape.as_deref(),
      exclude: None,
      fetch: request.limit * 3,
      scope: CandidateScope {
        family: Some(source.family),
        repo: context.repo.as_deref(),
        kinds: &[EntityKind::Function, EntityKind::Method],
        exclude_tests: !context.allow_tests,
      },
    };
    let candidates = lexical_and_signature_candidates(graph, index, &query)?;
    lexical_excluded = candidates.excluded_tests;
    for candidate in candidates.found {
      let found = signals.entry(candidate.entity.id).or_default();
      found.push(SimilaritySignal::Lexical {
        score: rounded(candidate.score),
        terms: candidate.terms,
      });
      if let Some(shape) = shape.clone().filter(|_| candidate.same_shape) {
        found.push(SimilaritySignal::SignatureShape { shape });
      }
    }
  }
  for (id, found) in signals.iter_mut() {
    add_signature_shape(graph, shape.as_ref(), *id, found)?;
  }
  let admit = |entity: &Entity| match admission(source, context, entity) {
    Admission::Admit => true,
    Admission::TestCode => {
      excluded.borrow_mut().insert(entity.id);
      false
    }
    Admission::Unrelated => false,
  };
  let shape_only = corroborate_small(graph, shape.as_ref(), small_found, &mut signals, admit)?;
  let mut structural_rows = Vec::new();
  let mut name_rows = Vec::new();
  for (id, found) in signals {
    let Some(entity) = graph.entity(id)? else { continue };
    if !admit(&entity) {
      continue;
    }
    match structural_score(&found) {
      Some(_) => structural_rows.push((entity, found)),
      None => name_rows.push((entity, found)),
    }
  }
  let best_lexical = structural_rows.iter().chain(&name_rows).map(|(_, found)| lexical_score(found)).fold(0.0, f64::max);
  structural_rows.sort_by(|(left, left_signals), (right, right_signals)| {
    structural_score(right_signals)
      .unwrap_or_default()
      .total_cmp(&structural_score(left_signals).unwrap_or_default())
      .then_with(|| lexical_score(right_signals).total_cmp(&lexical_score(left_signals)))
      .then_with(|| left.key.cmp(&right.key))
  });
  structural_rows.truncate(request.limit);
  name_rows.retain(|(_, found)| lexical_score(found) >= NAME_ONLY_FLOOR * best_lexical);
  name_rows.sort_by(|(left, left_signals), (right, right_signals)| lexical_score(right_signals).total_cmp(&lexical_score(left_signals)).then_with(|| left.key.cmp(&right.key)));
  name_rows.truncate(NAME_ONLY_LIMIT);
  let mut members = Vec::new();
  for (entity, found) in structural_rows {
    members.push(draft_view(graph, &entity, found)?);
  }
  let mut related = Vec::new();
  for (entity, found) in name_rows {
    related.push(draft_view(graph, &entity, found)?);
  }
  let best_with_excluded = lexical_excluded.iter().map(|(_, score)| rounded(*score)).fold(best_lexical, f64::max);
  excluded.borrow_mut().extend(
    lexical_excluded
      .iter()
      .filter(|(_, score)| rounded(*score) >= NAME_ONLY_FLOOR * best_with_excluded)
      .map(|(id, _)| *id),
  );
  let excluded = excluded.into_inner().len();
  if excluded > 0 {
    caveats.push(note(format!("{excluded} test helpers excluded")));
  }
  Ok(DraftMatches {
    draft: DraftSymbol {
      name: source.name.clone(),
      kind: source.kind.clone(),
      lines: source.lines,
      tokens: source.body.as_ref().map_or(0, |body| body.token_count),
      origin: source.origin,
    },
    members,
    related,
    shape_only,
    searched: scope.clone(),
    caveats,
  })
}

fn add_signature_shape(graph: &Graph<'_, '_>, target_shape: Option<&String>, id: EntityId, found: &mut Vec<SimilaritySignal>) -> QueryResult<()> {
  if found.iter().any(|signal| matches!(signal, SimilaritySignal::SignatureShape { .. })) {
    return Ok(());
  }
  let Some(entity) = graph.entity(id)? else { return Ok(()) };
  if let Some(shape) = target_shape.filter(|shape| signature_shape(&entity).as_ref() == Some(*shape)) {
    found.push(SimilaritySignal::SignatureShape { shape: shape.clone() });
  }
  Ok(())
}

fn consolidation_candidate(target: &Entity, entity: &Entity, signals: &[SimilaritySignal]) -> bool {
  let strong = signals.iter().any(|signal| {
    matches!(
      signal,
      SimilaritySignal::Structural { .. } | SimilaritySignal::SmallStructural { .. } | SimilaritySignal::SignatureShape { .. }
    ) || matches!(signal, SimilaritySignal::Inferred { certainty, .. } if certainty.freshness.is_none())
  });
  strong || language(entity) != language(target)
}

fn strength(signals: &[SimilaritySignal]) -> f64 {
  signals
    .iter()
    .map(|signal| match signal {
      SimilaritySignal::Inferred { certainty, verdict, .. } => 3.0 + f64::from(certainty.confidence.unwrap_or(0.0)) - if verdict == "equivalent" { 0.0 } else { 0.5 },
      SimilaritySignal::Structural { jaccard } => 2.0 + jaccard,
      SimilaritySignal::SmallStructural { jaccard } => 1.0 + jaccard,
      SimilaritySignal::Lexical { score, .. } => score.min(10.0) / 10.0,
      SimilaritySignal::SignatureShape { .. } => 1.0,
    })
    .sum()
}

fn inferred_signals(graph: &Graph<'_, '_>, target: &Entity, signals: &mut BTreeMap<EntityId, Vec<SimilaritySignal>>) -> QueryResult<(usize, usize)> {
  let (mut stale, mut low) = (0, 0);
  for relation in [RELATION_EQUIVALENT_TO, RELATION_SIMILAR_TO] {
    let facts = graph.tx().inferred_facts(&InferredQuery {
      subject: Some(target.id),
      relation: Some(relation.to_owned()),
      freshness: FreshnessPolicy::IncludeStale,
    })?;
    for fact in facts {
      match graph.policy.inferred_admission(&fact) {
        InferredAdmission::StaleHidden => {
          stale += 1;
          continue;
        }
        InferredAdmission::LowConfidence => {
          low += 1;
          continue;
        }
        InferredAdmission::Current | InferredAdmission::StaleShown => {}
      }
      let Some(other) = fact.subjects.iter().copied().find(|subject| *subject != target.id) else {
        continue;
      };
      let text = |name: &str| fact.payload.get(name).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
      let differences = fact
        .payload
        .get("behavioural_differences")
        .and_then(serde_json::Value::as_array)
        .map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
      signals.entry(other).or_default().push(SimilaritySignal::Inferred {
        verdict: text("verdict"),
        rationale: text("rationale"),
        differences,
        certainty: CertaintyTag::inferred(&fact.certainty, &fact.freshness),
      });
    }
  }
  Ok((stale, low))
}

fn verdict(signals: &[SimilaritySignal]) -> String {
  for signal in signals {
    if let SimilaritySignal::Inferred {
      verdict,
      rationale,
      differences,
      certainty,
    } = signal
    {
      let confidence = certainty.confidence.map(|value| format!("{value:.2}")).unwrap_or_default();
      let stale = certainty.freshness.as_ref().map(|state| format!(" [{state}]")).unwrap_or_default();
      return match verdict.as_str() {
        "equivalent" => format!("inferred equivalent ({confidence}){stale}: {rationale}"),
        _ => format!("inferred similar, not equivalent ({confidence}){stale}: {}", differences.join("; ")),
      };
    }
  }
  let jaccard = signals.iter().find_map(|signal| match signal {
    SimilaritySignal::Structural { jaccard } => Some(*jaccard),
    _ => None,
  });
  let same_signature = signals.iter().any(|signal| matches!(signal, SimilaritySignal::SignatureShape { .. }));
  if jaccard.is_none() && signals.iter().any(|signal| matches!(signal, SimilaritySignal::SmallStructural { .. })) {
    return "small bodies with a related name and similar token structure, behaviour unverified".to_owned();
  }
  match (jaccard, same_signature) {
    (Some(jaccard), _) if jaccard >= IDENTICAL_JACCARD => "structurally identical, behaviour unverified".to_owned(),
    (Some(_), _) => "same shape, behaviour unverified".to_owned(),
    (None, true) => "same signature shape and related name, behaviour unverified".to_owned(),
    (None, false) => "related by name only, behaviour unverified".to_owned(),
  }
}

fn language_label(entity: &Entity) -> String {
  match language(entity) {
    Some(LanguageFamily::Rust) => "rust".to_owned(),
    Some(_) => "ts".to_owned(),
    None => "unknown".to_owned(),
  }
}

fn member(graph: &Graph<'_, '_>, entity: &Entity, signals: Vec<SimilaritySignal>) -> QueryResult<ClusterMember> {
  let mut hidden = 0;
  let mut repos = BTreeSet::new();
  let (mut callers, mut uncertain) = (0, 0);
  for edge in graph.admitted(entity.id, Direction::Reverse, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References], &mut hidden)? {
    let Some(caller) = graph.entity(edge.from)? else { continue };
    if is_test(&caller) {
      continue;
    }
    callers += 1;
    uncertain += usize::from(!edge.certainty.is_authoritative());
    repos.extend(caller.repo.as_ref().map(ToString::to_string));
  }
  Ok(ClusterMember {
    entity: EntityRef::of(entity),
    language: language_label(entity),
    verdict: verdict(&signals),
    importability: None,
    signals,
    consumers: ConsumerSummary {
      callers,
      uncertain,
      repos: repos.into_iter().collect(),
    },
    tests: direct_tests(graph, entity)?.len(),
  })
}

fn package_dependencies(graph: &Graph<'_, '_>, package: EntityId) -> QueryResult<BTreeSet<EntityId>> {
  Ok(graph.edges_out(package, &[EdgeKind::DependsOnPackage])?.into_iter().map(|edge| edge.to).collect())
}

fn consolidation(graph: &Graph<'_, '_>, cluster: &[&Entity]) -> QueryResult<(Vec<SharedHome>, Vec<String>)> {
  let mut by_language: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
  for entity in cluster {
    by_language.entry(language_label(entity)).or_default().push(entity);
  }
  let mut homes = Vec::new();
  let mut risks = Vec::new();
  if by_language.len() > 1 {
    let languages: Vec<&str> = by_language.keys().map(String::as_str).collect();
    risks.push(format!(
      "members span languages ({}); they cannot share one implementation without a service boundary or code generation",
      languages.join(", ")
    ));
  }
  for (language, members) in &by_language {
    if members.len() < 2 {
      continue;
    }
    let mut consumer_packages: BTreeSet<EntityId> = BTreeSet::new();
    let mut member_packages: BTreeMap<EntityId, String> = BTreeMap::new();
    for member in members {
      if let Some(package) = package_key(member) {
        member_packages.insert(EntityId::of(&package), package_name(&package));
      }
      for edge in graph.edges_in(member.id, &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References])? {
        let Some(caller) = graph.entity(edge.from)? else { continue };
        if let Some(package) = package_key(&caller) {
          consumer_packages.insert(EntityId::of(&package));
        }
      }
    }
    let mut best: Option<(usize, EntityId, String)> = None;
    for (package, name) in &member_packages {
      let mut reaches = 0;
      for consumer in consumer_packages.iter().chain(member_packages.keys()) {
        if consumer == package || package_dependencies(graph, *consumer)?.contains(package) {
          reaches += 1;
        }
      }
      let shared = members
        .iter()
        .filter(|member| package_key(member).is_some_and(|key| EntityId::of(&key) == *package))
        .any(|member| member.path.as_ref().is_some_and(|path| path.segments().any(|segment| SHARED_SEGMENTS.contains(&segment))));
      let rank = reaches * 2 + usize::from(shared);
      if best
        .as_ref()
        .is_none_or(|(current, _, current_name)| rank > *current || (rank == *current && name < current_name))
      {
        best = Some((rank, *package, name.clone()));
      }
    }
    let Some((_, home, name)) = best else { continue };
    let total = consumer_packages.union(&member_packages.keys().copied().collect()).count();
    let mut unreachable = Vec::new();
    for consumer in consumer_packages.iter().chain(member_packages.keys()).collect::<BTreeSet<_>>() {
      if *consumer != home && !package_dependencies(graph, *consumer)?.contains(&home) {
        let label = graph.entity(*consumer)?.map_or_else(|| consumer.to_string(), |entity| entity.name);
        unreachable.push(label);
      }
    }
    let reason = match unreachable.is_empty() {
      true => format!("every consumer and member package ({total}) already depends on {name} or lives in it"),
      false => format!("{name} already hosts a member and is reachable from {} of {total} packages", total - unreachable.len()),
    };
    for package in &unreachable {
      risks.push(format!("consolidating into {name} adds a dependency {package} -> {name} that does not exist yet"));
    }
    homes.push(SharedHome {
      language: language.clone(),
      package: Some(name),
      reason,
    });
  }
  Ok((homes, risks))
}

fn package_name(key: &EntityKey) -> String {
  key.display_name()
}
