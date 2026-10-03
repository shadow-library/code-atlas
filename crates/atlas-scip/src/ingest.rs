use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use atlas_core::{Certainty, Edge, EdgeId, EdgeKind, EntityId, EntityKind, Evidence, Producer, RefKind, RelPath, Span};
use protobuf::Message;
use scip::symbol::is_local_symbol;
use scip::types::{Document, Index, SymbolRole};

use crate::context::{ScipFileContext, ScipIngestContext, ScipReference, ScipSymbol};
use crate::error::ScipError;
use crate::facts::{ScipFacts, UnmappedDefinition};
use crate::impl_header::parse_impl_header;
use crate::positions::{ColumnEncoding, LineIndex, ScipRange};

const PRODUCER_VERSION: u32 = 1;
const MAX_WARNINGS: usize = 50;
const ROLE_DEFINITION: i32 = SymbolRole::Definition as i32;
const ROLE_FORWARD_DEFINITION: i32 = SymbolRole::ForwardDefinition as i32;

pub fn ingest(index_bytes: &[u8], context: &ScipIngestContext) -> Result<ScipFacts, ScipError> {
  let index = Index::parse_from_bytes(index_bytes)?;
  Ok(Ingestor::new(&index, context).run())
}

struct DocumentView<'a> {
  document: &'a Document,
  path: RelPath,
  file: &'a ScipFileContext,
  lines: LineIndex<'a>,
  encoding: ColumnEncoding,
}

impl DocumentView<'_> {
  fn locate(&self, occurrence: &scip::types::Occurrence) -> Option<(u32, u32)> {
    let range = ScipRange::of(occurrence)?;
    let (start, end) = self.lines.byte_range(range, self.encoding)?;
    Some((u32::try_from(start).ok()?, u32::try_from(end).ok()?))
  }

  fn evidence_span(&self, start: u32, end: u32) -> Option<Span> {
    Span::of_text(&self.file.source, start as usize, end as usize)
  }
}

#[derive(Debug, Clone)]
struct Definition {
  entity: EntityId,
  kind: EntityKind,
  path: RelPath,
  span: Span,
}

enum Target<'a> {
  Mapped(&'a Definition),
  ProjectUnmapped,
  External,
}

struct Ingestor<'a> {
  index: &'a Index,
  context: &'a ScipIngestContext,
  producer: Producer,
  views: Vec<DocumentView<'a>>,
  definitions: HashMap<String, Definition>,
  project_defined: HashSet<String>,
  ambiguous: HashSet<String>,
  matched: HashSet<EntityId>,
  edges: BTreeMap<RelPath, EdgeAccumulator>,
  facts: ScipFacts,
}

#[derive(Default)]
struct EdgeAccumulator {
  edges: Vec<Edge>,
  positions: HashMap<EdgeId, usize>,
}

impl EdgeAccumulator {
  fn add(&mut self, edge: Edge) {
    let Some(&position) = self.positions.get(&edge.id) else {
      self.positions.insert(edge.id, self.edges.len());
      self.edges.push(edge);
      return;
    };
    let existing = &mut self.edges[position];
    for evidence in edge.evidence {
      if !existing.evidence.contains(&evidence) {
        existing.evidence.push(evidence);
      }
    }
  }
}

impl<'a> Ingestor<'a> {
  fn new(index: &'a Index, context: &'a ScipIngestContext) -> Self {
    let tool = index.metadata.tool_info.name.as_str();
    let producer = Producer::new(if tool.is_empty() { "scip".to_owned() } else { format!("scip:{tool}") }, PRODUCER_VERSION);
    Self {
      index,
      context,
      producer,
      views: Vec::new(),
      definitions: HashMap::new(),
      project_defined: HashSet::new(),
      ambiguous: HashSet::new(),
      matched: HashSet::new(),
      edges: BTreeMap::new(),
      facts: ScipFacts::default(),
    }
  }

  fn run(mut self) -> ScipFacts {
    self.open_documents();
    self.collect_definitions();
    self.collect_references();
    self.collect_relationships();
    self.derive_trait_impl_relationships();
    self.report_unmapped_definitions();
    self.finish()
  }

  fn warn(&mut self, message: String) {
    tracing::warn!(%message, "scip ingestion warning");
    if self.facts.warnings.len() < MAX_WARNINGS {
      self.facts.warnings.push(message);
    }
  }

  fn open_documents(&mut self) {
    let tool = self.index.metadata.tool_info.name.as_str();
    let mut warned_encoding = false;
    for document in &self.index.documents {
      let Some(path) = self.context.repo_path(&document.relative_path) else {
        self.facts.stats.documents_outside_project += 1;
        continue;
      };
      let Some(file) = self.context.files.get(&path) else {
        self.facts.stats.documents_outside_project += 1;
        continue;
      };
      let declared = document.position_encoding.enum_value_or_default();
      if !warned_encoding && !ColumnEncoding::is_declared(declared) && !file.source.is_ascii() {
        warned_encoding = true;
        let message = format!("document {path} does not declare a position encoding; assuming the default for {tool:?}");
        tracing::warn!(%message, "scip ingestion warning");
        self.facts.warnings.push(message);
      }
      self.facts.stats.documents += 1;
      self.facts.per_file.entry((self.context.repo.clone(), path.clone())).or_default();
      self.views.push(DocumentView {
        document,
        path,
        file,
        lines: LineIndex::new(&file.source),
        encoding: ColumnEncoding::of_document(declared, tool),
      });
    }
  }

  fn collect_definitions(&mut self) {
    let mut pending_warnings = Vec::new();
    for view in &self.views {
      for occurrence in view.document.occurrences.iter().filter(|occurrence| occurrence.symbol_roles & ROLE_DEFINITION != 0) {
        let symbol = occurrence.symbol.as_str();
        if symbol.is_empty() || is_local_symbol(symbol) {
          continue;
        }
        self.project_defined.insert(symbol.to_owned());
        let Some((start, end)) = view.locate(occurrence) else {
          pending_warnings.push(format!("definition of {symbol} in {} has an out-of-range position", view.path));
          self.facts.stats.definitions_unmapped += 1;
          continue;
        };
        let Some(ours) = defining_symbol(&view.file.symbols, start, end) else {
          self.facts.stats.definitions_unmapped += 1;
          continue;
        };
        let Some(span) = view.evidence_span(start, end) else {
          self.facts.stats.definitions_unmapped += 1;
          continue;
        };
        self.facts.stats.definitions_mapped += 1;
        self.matched.insert(ours.entity);
        let definition = Definition {
          entity: ours.entity,
          kind: ours.kind.clone(),
          path: view.path.clone(),
          span,
        };
        match self.definitions.get(symbol) {
          Some(existing) if existing.entity != definition.entity => {
            self.ambiguous.insert(symbol.to_owned());
          }
          Some(_) => {}
          None => {
            self.definitions.insert(symbol.to_owned(), definition);
          }
        }
      }
    }
    for symbol in &self.ambiguous {
      self.definitions.remove(symbol);
    }
    self.facts.stats.definitions_ambiguous = self.ambiguous.len() as u64;
    let ambiguous: Vec<String> = self
      .ambiguous
      .iter()
      .map(|symbol| format!("scip symbol {symbol} is defined at several of our symbols; its references are not mapped"))
      .collect();
    for message in pending_warnings.into_iter().chain(ambiguous) {
      self.warn(message);
    }
  }

  fn target(&self, symbol: &str) -> Target<'_> {
    if let Some(definition) = self.definitions.get(symbol) {
      return Target::Mapped(definition);
    }
    if self.project_defined.contains(symbol) {
      Target::ProjectUnmapped
    } else {
      Target::External
    }
  }

  fn collect_references(&mut self) {
    let mut pending_warnings = Vec::new();
    let mut staged: Vec<(RelPath, Edge)> = Vec::new();
    let mut external: BTreeMap<String, u64> = BTreeMap::new();
    let mut stats = self.facts.stats.clone();
    for view in &self.views {
      for occurrence in &view.document.occurrences {
        let symbol = occurrence.symbol.as_str();
        let roles = occurrence.symbol_roles;
        if symbol.is_empty() || is_local_symbol(symbol) || roles & (ROLE_DEFINITION | ROLE_FORWARD_DEFINITION) != 0 {
          continue;
        }
        let target = match self.target(symbol) {
          Target::External => {
            *external.entry(package_of(symbol).to_owned()).or_default() += 1;
            stats.references_external += 1;
            continue;
          }
          Target::ProjectUnmapped => {
            stats.references_unmapped += 1;
            continue;
          }
          Target::Mapped(definition) => definition,
        };
        let Some((start, end)) = view.locate(occurrence) else {
          pending_warnings.push(format!("reference to {symbol} in {} has an out-of-range position", view.path));
          stats.references_unmapped += 1;
          continue;
        };
        let (Some(enclosing), Some(span)) = (innermost_enclosing(&view.file.symbols, start, end), view.evidence_span(start, end)) else {
          stats.references_unmapped += 1;
          continue;
        };
        let kind = edge_kind(matching_reference(&view.file.references, &span, view.file.source.get(start as usize..end as usize)).map(|reference| reference.kind));
        let evidence = Evidence {
          repo: self.context.repo.clone(),
          path: view.path.clone(),
          span,
          content_hash: view.file.content_hash,
          producer: self.producer.clone(),
          note: None,
        };
        stats.references_mapped += 1;
        staged.push((
          view.path.clone(),
          Edge::new(kind, enclosing.entity, target.entity, "", Certainty::CompilerVerified).with_evidence(evidence),
        ));
      }
    }
    self.facts.stats = stats;
    self.facts.external_references = external;
    for (path, edge) in staged {
      self.edges.entry(path).or_default().add(edge);
    }
    for message in pending_warnings {
      self.warn(message);
    }
  }

  fn collect_relationships(&mut self) {
    let mut staged: Vec<(RelPath, Edge)> = Vec::new();
    for view in &self.views {
      for information in &view.document.symbols {
        for relationship in information.relationships.iter().filter(|relationship| relationship.is_implementation) {
          let (from, to) = (information.symbol.as_str(), relationship.symbol.as_str());
          if is_local_symbol(from) || is_local_symbol(to) {
            continue;
          }
          let (Some(source), Some(target)) = (self.definitions.get(from), self.definitions.get(to)) else {
            self.facts.stats.relationships_unmapped += 1;
            continue;
          };
          let Some(owner) = self.views.iter().find(|candidate| candidate.path == source.path) else {
            self.facts.stats.relationships_unmapped += 1;
            continue;
          };
          let evidence = Evidence {
            repo: self.context.repo.clone(),
            path: source.path.clone(),
            span: source.span,
            content_hash: owner.file.content_hash,
            producer: self.producer.clone(),
            note: Some(format!("scip relationship {from} -> {to}")),
          };
          self.facts.stats.relationships_mapped += 1;
          staged.push((
            source.path.clone(),
            Edge::new(relationship_kind(&source.kind, &target.kind), source.entity, target.entity, "", Certainty::CompilerVerified).with_evidence(evidence),
          ));
        }
      }
    }
    for (path, edge) in staged {
      self.edges.entry(path).or_default().add(edge);
    }
  }

  fn derive_trait_impl_relationships(&mut self) {
    let mut staged: Vec<(RelPath, Edge)> = Vec::new();
    let mut unmapped = 0;
    for view in &self.views {
      let mut occurrences: Vec<(u32, u32, &str)> = view
        .document
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.symbol_roles & ROLE_DEFINITION == 0 && !occurrence.symbol.is_empty() && !is_local_symbol(&occurrence.symbol))
        .filter_map(|occurrence| view.locate(occurrence).map(|(start, end)| (start, end, occurrence.symbol.as_str())))
        .collect();
      occurrences.sort_unstable();
      for implementation in &view.file.trait_impls {
        let Some(header) = parse_impl_header(&view.file.source, implementation.span.start_byte as usize) else {
          continue;
        };
        let named = |range: &Range<usize>, name: &str| -> Vec<(u32, u32, &str)> {
          occurrences
            .iter()
            .filter(|(start, end, _)| range.start <= *start as usize && *end as usize <= range.end && view.file.source.get(*start as usize..*end as usize) == Some(name))
            .copied()
            .collect()
        };
        let (self_hits, trait_hits) = (named(&header.self_path, &implementation.self_name), named(&header.trait_path, &implementation.trait_name));
        let (Some(&(self_start, self_end, self_symbol)), Some(&(_, _, trait_symbol))) = (self_hits.first(), trait_hits.first()) else {
          continue;
        };
        if self_hits.iter().any(|(_, _, symbol)| *symbol != self_symbol) || trait_hits.iter().any(|(_, _, symbol)| *symbol != trait_symbol) {
          continue;
        }
        let (Some(source), Some(target)) = (self.definitions.get(self_symbol), self.definitions.get(trait_symbol)) else {
          if self.project_defined.contains(trait_symbol) || self.project_defined.contains(self_symbol) {
            unmapped += 1;
          }
          continue;
        };
        if !matches!(target.kind, EntityKind::Trait | EntityKind::Interface) || matches!(source.kind, EntityKind::Trait | EntityKind::Interface) {
          continue;
        }
        let Some(span) = view.evidence_span(self_start, self_end) else {
          continue;
        };
        let evidence = Evidence {
          repo: self.context.repo.clone(),
          path: view.path.clone(),
          span,
          content_hash: view.file.content_hash,
          producer: self.producer.clone(),
          note: Some(format!("scip impl header {self_symbol} : {trait_symbol}")),
        };
        staged.push((
          view.path.clone(),
          Edge::new(EdgeKind::Implements, source.entity, target.entity, "", Certainty::CompilerVerified).with_evidence(evidence),
        ));
        self.facts.stats.relationships_mapped += 1;
      }
    }
    self.facts.stats.relationships_unmapped += unmapped;
    for (path, edge) in staged {
      self.edges.entry(path).or_default().add(edge);
    }
  }

  fn report_unmapped_definitions(&mut self) {
    for view in &self.views {
      for symbol in view.file.symbols.iter().filter(|symbol| !self.matched.contains(&symbol.entity)) {
        self.facts.unmapped_definitions.push(UnmappedDefinition {
          repo: self.context.repo.clone(),
          path: view.path.clone(),
          entity: symbol.entity,
          name: symbol.name.clone(),
        });
      }
    }
  }

  fn finish(mut self) -> ScipFacts {
    for (path, accumulator) in self.edges {
      self.facts.per_file.insert((self.context.repo.clone(), path), accumulator.edges);
    }
    self.facts
  }
}

fn defining_symbol(symbols: &[ScipSymbol], start: u32, end: u32) -> Option<&ScipSymbol> {
  symbols
    .iter()
    .filter(|symbol| symbol.name_span.start_byte <= start && end <= symbol.name_span.end_byte)
    .min_by_key(|symbol| symbol.name_span.len())
}

fn innermost_enclosing(symbols: &[ScipSymbol], start: u32, end: u32) -> Option<&ScipSymbol> {
  symbols
    .iter()
    .rev()
    .filter(|symbol| symbol.span.start_byte <= start && end <= symbol.span.end_byte)
    .min_by_key(|symbol| symbol.span.len())
}

fn matching_reference<'a>(references: &'a [ScipReference], span: &Span, text: Option<&str>) -> Option<&'a ScipReference> {
  if let Some(exact) = references
    .iter()
    .find(|reference| reference.span.start_byte == span.start_byte && reference.span.end_byte == span.end_byte)
  {
    return Some(exact);
  }
  let text = text?;
  references
    .iter()
    .filter(|reference| reference.span.contains(span) && reference.name.as_deref() == Some(text))
    .min_by_key(|reference| reference.span.len())
}

fn edge_kind(reference: Option<RefKind>) -> EdgeKind {
  match reference {
    Some(RefKind::Call | RefKind::MethodCall | RefKind::MacroCall) => EdgeKind::Calls,
    Some(RefKind::Instantiation) => EdgeKind::Instantiates,
    _ => EdgeKind::References,
  }
}

fn relationship_kind(from: &EntityKind, to: &EntityKind) -> EdgeKind {
  match (from, to) {
    (EntityKind::Class, EntityKind::Class) | (EntityKind::Interface, EntityKind::Interface) | (EntityKind::Trait, EntityKind::Trait) => EdgeKind::Extends,
    _ => EdgeKind::Implements,
  }
}

fn package_of(symbol: &str) -> &str {
  symbol.split(' ').nth(2).filter(|name| *name != ".").unwrap_or("unknown")
}
