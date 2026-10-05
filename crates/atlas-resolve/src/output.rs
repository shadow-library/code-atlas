use std::collections::{BTreeMap, HashMap};

use atlas_core::{
  ArgSummary, Certainty, Confidence, ContentHash, Edge, EdgeId, EdgeKind, EntityId, EntityKind, Evidence, ExportFact, FileFacts, ImportBinding, ImportFact, LanguageFamily,
  LocalSymbolId, Producer, ReceiverHint, RefKind, RefOrigin, ReferenceFact, Span, UnresolvedReason, UnresolvedRef, Visibility,
};

use crate::engine::{Found, Guard, Hit, MACRO_TOKEN_SCAN_CONFIDENCE, MAX_AMBIGUOUS_CANDIDATES, Miss, Outcome, RULE_MACRO_TOKEN_SCAN, Resolver};
use crate::error::ResolveError;
use crate::surface::FileSurface;
use crate::table::{FileIdx, LookupSource, ProjectSymbolTable, Reachability, SymRef};
use crate::ts::{TsNs, specifier_display};

pub const RESOLVER_NAME: &str = "atlas-resolve";
pub const RESOLVER_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
  Resolved(EntityId, Certainty),
  Candidates(Vec<(EntityId, Confidence)>),
  Unresolved(UnresolvedReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefOutcome {
  Resolved,
  Heuristic,
  Unresolved,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutcomeCounts {
  pub resolved: u32,
  pub heuristic: u32,
  pub unresolved: u32,
}

impl OutcomeCounts {
  pub fn total(&self) -> u32 {
    self.resolved + self.heuristic + self.unresolved
  }

  fn record(&mut self, outcome: RefOutcome) {
    match outcome {
      RefOutcome::Resolved => self.resolved += 1,
      RefOutcome::Heuristic => self.heuristic += 1,
      RefOutcome::Unresolved => self.unresolved += 1,
    }
  }

  pub fn add(&mut self, other: &OutcomeCounts) {
    self.resolved += other.resolved;
    self.heuristic += other.heuristic;
    self.unresolved += other.unresolved;
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveStats {
  pub by_kind: BTreeMap<RefKind, OutcomeCounts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFile {
  pub module_path_override: Option<Vec<String>>,
  pub package_override: Option<String>,
  pub reachability: Reachability,
  pub edges: Vec<Edge>,
  pub unresolved: Vec<UnresolvedRef>,
  pub structural: Vec<Edge>,
  pub stats: ResolveStats,
}

fn checked_surface<'t>(facts: &FileFacts, table: &'t ProjectSymbolTable) -> Result<(FileIdx, &'t FileSurface), ResolveError> {
  if !table.is_fresh() {
    return Err(ResolveError::StaleTable);
  }
  let not_indexed = || ResolveError::NotIndexed {
    repo: facts.file.repo.clone(),
    path: facts.file.path.clone(),
  };
  let index = table.file_index(&facts.file.repo, &facts.file.path).ok_or_else(not_indexed)?;
  let surface = table.file(index).ok_or_else(not_indexed)?;
  if surface.content_hash != facts.file.content_hash {
    return Err(ResolveError::StaleFile {
      repo: facts.file.repo.clone(),
      path: facts.file.path.clone(),
    });
  }
  Ok((index, surface))
}

pub fn resolve_file(facts: &FileFacts, table: &ProjectSymbolTable) -> Result<ResolvedFile, ResolveError> {
  let (index, surface) = checked_surface(facts, table)?;
  let resolver = Resolver::new(table, index, surface);
  let mut emitter = Emitter::new(&resolver, facts.file.content_hash);
  for reference in &facts.references {
    let outcome = cap_origin(resolver.resolve_reference(reference), reference.origin);
    let anchor = resolver.type_anchor(reference, &outcome).map(|hit| match reference.origin {
      RefOrigin::Syntax => hit,
      _ => cap_hit(hit),
    });
    emitter.reference(reference, outcome);
    if let Some(anchor) = anchor {
      emitter.anchor(reference, &anchor);
    }
  }
  emitter.containment();
  match surface.family {
    LanguageFamily::Rust => emitter.rust_structure(),
    _ => emitter.ts_structure(facts),
  }
  table.record_lookups(index, resolver.take_lookups(), LookupSource::File);
  let placement = table.effective_placement(index);
  let (module_path_override, package_override) = match surface.family {
    LanguageFamily::Rust => (
      (placement.context.module_path != surface.context.module_path).then(|| placement.context.module_path.clone()),
      (placement.context.package != surface.context.package).then(|| placement.context.package.clone()),
    ),
    _ => (None, None),
  };
  Ok(ResolvedFile {
    module_path_override,
    package_override,
    reachability: placement.reachability,
    edges: finish_edges(emitter.edges),
    unresolved: emitter.unresolved,
    structural: finish_edges(emitter.structural),
    stats: emitter.stats,
  })
}

pub fn resolve_path_in_file(file: &FileFacts, path: &[String], from: Option<LocalSymbolId>, table: &ProjectSymbolTable) -> Result<Resolution, ResolveError> {
  let (index, surface) = checked_surface(file, table)?;
  let resolver = Resolver::new(table, index, surface);
  let receiver = (surface.family == LanguageFamily::Ts && path.first().is_some_and(|head| head == "this")).then_some(ReceiverHint::This);
  let reference = ReferenceFact {
    from,
    kind: RefKind::ValueRef,
    path: path.to_vec(),
    receiver,
    args: Vec::new(),
    span: Span::from_zero_based_rows(0, 0, 0, 0, 0, 0),
    origin: RefOrigin::Syntax,
  };
  let resolution = match resolver.resolve_reference(&reference) {
    Outcome::Targets { primary, .. } => match (resolver.found_entity(&primary.found), certainty_of(&primary)) {
      (Some(entity), Some(certainty)) => Resolution::Resolved(entity, certainty),
      _ => Resolution::Unresolved(UnresolvedReason::NotFound),
    },
    Outcome::Candidates(hits) => Resolution::Candidates(
      hits
        .iter()
        .filter_map(|hit| Some((resolver.found_entity(&hit.found)?, Confidence::new(hit.heuristic?.confidence).ok()?)))
        .collect(),
    ),
    Outcome::External(package) => Resolution::Unresolved(UnresolvedReason::External { package }),
    Outcome::Unresolved(miss) => Resolution::Unresolved(resolver.reason(miss)),
  };
  table.record_lookups(index, resolver.take_lookups(), LookupSource::Detector);
  Ok(resolution)
}

fn cap_hit(hit: Hit) -> Hit {
  match hit.heuristic {
    None => Hit::heuristic(hit.found, MACRO_TOKEN_SCAN_CONFIDENCE, RULE_MACRO_TOKEN_SCAN),
    Some(_) => hit,
  }
}

fn cap_origin(outcome: Outcome, origin: RefOrigin) -> Outcome {
  if origin == RefOrigin::Syntax {
    return outcome;
  }
  match outcome {
    Outcome::Targets { primary, extra } => Outcome::Candidates(std::iter::once(cap_hit(primary)).chain(extra).collect()),
    Outcome::Candidates(hits) => Outcome::Candidates(hits.into_iter().map(cap_hit).collect()),
    other => other,
  }
}

fn certainty_of(hit: &Hit) -> Option<Certainty> {
  match hit.heuristic {
    None => Some(Certainty::Resolved),
    Some(heuristic) => Some(Certainty::heuristic(Confidence::new(heuristic.confidence).ok()?, heuristic.rule)),
  }
}

fn finish_edges(edges: HashMap<EdgeId, Edge>) -> Vec<Edge> {
  let mut edges: Vec<Edge> = edges.into_values().collect();
  edges.sort_by_key(|edge| edge.id);
  edges
}

impl Resolver<'_> {
  pub(crate) fn resolve_reference(&self, reference: &ReferenceFact) -> Outcome {
    let outcome = match self.surface.family {
      LanguageFamily::Rust => self.rust_reference(reference),
      _ => self.ts_reference(reference),
    };
    self.collapse_same_entity(outcome)
  }

  pub(crate) fn type_anchor(&self, reference: &ReferenceFact, outcome: &Outcome) -> Option<Hit> {
    match outcome {
      Outcome::External(_) | Outcome::Unresolved(Miss::NotFound | Miss::MacroGenerated | Miss::Ambiguous(_) | Miss::NameOnly(_) | Miss::Unsupported(_)) => {}
      _ => return None,
    }
    match self.surface.family {
      LanguageFamily::Rust => self.rust_type_anchor(reference),
      _ => self.ts_type_anchor(reference),
    }
  }

  fn collapse_same_entity(&self, outcome: Outcome) -> Outcome {
    let Outcome::Unresolved(Miss::Ambiguous(hits)) = &outcome else { return outcome };
    if hits.len() < 2 {
      return outcome;
    }
    let entities: Vec<Option<EntityId>> = hits.iter().map(|hit| self.found_entity(&hit.found)).collect();
    let same = matches!(entities.first(), Some(Some(first)) if entities.iter().all(|entity| entity.as_ref() == Some(first)));
    if !same {
      return outcome;
    }
    match hits.iter().find(|hit| hit.heuristic.is_none()).or(hits.first()) {
      Some(primary) => Outcome::Targets {
        primary: primary.clone(),
        extra: Vec::new(),
      },
      None => outcome,
    }
  }

  pub(crate) fn found_entity(&self, found: &Found) -> Option<EntityId> {
    match found {
      Found::Symbol(symbol) => self.table.symbol_entity(*symbol),
      Found::Module(module) => self.table.module_entity(module),
      Found::File(file) => self.table.file_entity(*file),
      Found::External(_) => None,
    }
  }

  pub(crate) fn reason(&self, miss: Miss) -> UnresolvedReason {
    match miss {
      Miss::NotFound => UnresolvedReason::NotFound,
      Miss::Dynamic => UnresolvedReason::Dynamic,
      Miss::MacroGenerated => UnresolvedReason::Unsupported {
        detail: "macro_generated".to_owned(),
      },
      Miss::Unsupported(detail) => UnresolvedReason::Unsupported { detail: detail.to_owned() },
      Miss::Ambiguous(hits) | Miss::NameOnly(hits) => {
        let mut candidates: Vec<EntityId> = hits.iter().filter_map(|hit| self.found_entity(&hit.found)).collect();
        candidates.sort();
        candidates.dedup();
        candidates.truncate(MAX_AMBIGUOUS_CANDIDATES);
        UnresolvedReason::Ambiguous { candidates }
      }
    }
  }
}

struct Emitter<'r, 't> {
  resolver: &'r Resolver<'t>,
  content_hash: ContentHash,
  producer: Producer,
  edges: HashMap<EdgeId, Edge>,
  structural: HashMap<EdgeId, Edge>,
  unresolved: Vec<UnresolvedRef>,
  stats: ResolveStats,
}

#[derive(Clone, Copy)]
enum Bucket {
  Reference,
  Structural,
}

impl<'r, 't> Emitter<'r, 't> {
  fn new(resolver: &'r Resolver<'t>, content_hash: ContentHash) -> Self {
    Self {
      resolver,
      content_hash,
      producer: Producer::new(RESOLVER_NAME, RESOLVER_VERSION),
      edges: HashMap::new(),
      structural: HashMap::new(),
      unresolved: Vec::new(),
      stats: ResolveStats::default(),
    }
  }

  fn file(&self) -> FileIdx {
    self.resolver.file
  }

  fn local_entity(&self, local: Option<u32>) -> Option<EntityId> {
    match local {
      Some(local) => self.resolver.table.symbol_entity(SymRef { file: self.file(), local }),
      None => self.resolver.table.file_entity(self.file()),
    }
  }

  fn evidence(&self, span: Span) -> Evidence {
    Evidence {
      repo: self.resolver.surface.repo.clone(),
      path: self.resolver.surface.path.clone(),
      span,
      content_hash: self.content_hash,
      producer: self.producer.clone(),
      note: None,
    }
  }

  fn add(&mut self, bucket: Bucket, kind: EdgeKind, from: EntityId, to: EntityId, certainty: Certainty, span: Span) {
    if from == to && kind != EdgeKind::Calls {
      return;
    }
    let evidence = self.evidence(span);
    let edges = match bucket {
      Bucket::Reference => &mut self.edges,
      Bucket::Structural => &mut self.structural,
    };
    let edge = Edge::new(kind, from, to, "", certainty).with_evidence(evidence.clone());
    match edges.get_mut(&edge.id) {
      None => {
        edges.insert(edge.id, edge);
      }
      Some(existing) => match edge.certainty.cmp_strength(&existing.certainty) {
        std::cmp::Ordering::Greater => *existing = edge,
        std::cmp::Ordering::Equal if !existing.evidence.contains(&evidence) => existing.evidence.push(evidence),
        _ => {}
      },
    }
  }

  fn add_hit(&mut self, bucket: Bucket, kind: EdgeKind, from: EntityId, hit: &Hit, span: Span) -> bool {
    let (Some(to), Some(certainty)) = (self.resolver.found_entity(&hit.found), certainty_of(hit)) else {
      return false;
    };
    self.add(bucket, kind, from, to, certainty, span);
    true
  }

  fn target_kind(&self, hit: &Hit) -> Option<EntityKind> {
    match &hit.found {
      Found::Symbol(symbol) => self.resolver.sym_kind(*symbol).cloned(),
      Found::Module(_) => Some(EntityKind::Module),
      Found::File(_) => Some(EntityKind::File),
      Found::External(_) => None,
    }
  }

  fn edge_kind(&self, reference: &ReferenceFact, hit: &Hit) -> EdgeKind {
    let target = self.target_kind(hit);
    let from_kind = reference.from.and_then(|local| self.resolver.surface.symbol(local.0)).map(|symbol| &symbol.kind);
    match reference.kind {
      RefKind::Call | RefKind::MethodCall => match target {
        Some(EntityKind::Struct | EntityKind::Variant | EntityKind::Class) => EdgeKind::Instantiates,
        Some(EntityKind::Function | EntityKind::Method | EntityKind::Test | EntityKind::Field | EntityKind::Constant | EntityKind::Variable) => EdgeKind::Calls,
        _ => EdgeKind::References,
      },
      RefKind::Instantiation => EdgeKind::Instantiates,
      RefKind::Decorator => EdgeKind::Calls,
      RefKind::Inherit => EdgeKind::Extends,
      RefKind::JsxElement => match target {
        Some(EntityKind::Class) => EdgeKind::Instantiates,
        _ => EdgeKind::Calls,
      },
      RefKind::TypeRef => match from_kind {
        Some(EntityKind::Field | EntityKind::Constant | EntityKind::Variable) => EdgeKind::TypeOf,
        _ => EdgeKind::References,
      },
      _ => EdgeKind::References,
    }
  }

  fn reference(&mut self, reference: &ReferenceFact, outcome: Outcome) {
    let Some(from) = self.local_entity(reference.from.map(|local| local.0)) else { return };
    let class = match outcome {
      Outcome::Targets { primary, extra } => {
        let kind = self.edge_kind(reference, &primary);
        let class = match primary.heuristic {
          None => RefOutcome::Resolved,
          Some(_) => RefOutcome::Heuristic,
        };
        let emitted = self.add_hit(Bucket::Reference, kind.clone(), from, &primary, reference.span);
        for hit in &extra {
          self.add_hit(Bucket::Reference, kind.clone(), from, hit, reference.span);
        }
        match emitted {
          true => class,
          false => self.unresolved(reference, from, UnresolvedReason::NotFound),
        }
      }
      Outcome::Candidates(hits) => {
        let mut emitted = false;
        for hit in &hits {
          let kind = self.edge_kind(reference, hit);
          emitted |= self.add_hit(Bucket::Reference, kind, from, hit, reference.span);
        }
        match emitted {
          true => RefOutcome::Heuristic,
          false => self.unresolved(reference, from, UnresolvedReason::NotFound),
        }
      }
      Outcome::External(package) => self.unresolved(reference, from, UnresolvedReason::External { package }),
      Outcome::Unresolved(miss) => {
        let reason = self.resolver.reason(miss);
        self.unresolved(reference, from, reason)
      }
    };
    self.stats.by_kind.entry(reference.kind).or_default().record(class);
  }

  fn anchor(&mut self, reference: &ReferenceFact, hit: &Hit) {
    let Some(from) = self.local_entity(reference.from.map(|local| local.0)) else { return };
    self.add_hit(Bucket::Reference, EdgeKind::References, from, hit, reference.span);
  }

  fn unresolved(&mut self, reference: &ReferenceFact, from: EntityId, reason: UnresolvedReason) -> RefOutcome {
    self.unresolved.push(UnresolvedRef {
      from,
      path: reference.path.clone(),
      kind: reference.kind,
      receiver: reference.receiver.clone(),
      reason,
      repo: self.resolver.surface.repo.clone(),
      file: self.resolver.surface.path.clone(),
      span: reference.span,
    });
    RefOutcome::Unresolved
  }

  fn containment(&mut self) {
    let Some(file_entity) = self.local_entity(None) else { return };
    let surface = self.resolver.surface;
    for (local, symbol) in surface.symbols.iter().enumerate() {
      let Some(entity) = self.local_entity(Some(local as u32)) else { continue };
      let container = match symbol.parent {
        None => Some(file_entity),
        Some(parent) => self.local_entity(Some(parent)),
      };
      if let Some(container) = container {
        self.add(Bucket::Structural, EdgeKind::Contains, container, entity, Certainty::Syntactic, symbol.span);
      }
    }
  }

  fn rust_structure(&mut self) {
    let resolver = self.resolver;
    let surface = resolver.surface;
    let file = self.file();
    if let Some(module) = resolver.table.rust_module_of(file, &[])
      && !module.path.is_empty()
      && let Some(parent_module) = resolver
        .table
        .module_entry(&module)
        .and_then(|entry| entry.decl)
        .filter(|declaration| declaration.file != file)
      && let Some(container) = resolver.table.symbol_entity(parent_module)
    {
      for (local, symbol) in surface.symbols.iter().enumerate() {
        if symbol.kind == EntityKind::Module
          && symbol.parent.is_none()
          && symbol.module_suffix.len() == 1
          && let Some(child) = self.local_entity(Some(local as u32))
        {
          self.add(Bucket::Structural, EdgeKind::Contains, container, child, Certainty::Resolved, symbol.span);
        }
      }
    }
    for (local, symbol) in surface.symbols.iter().enumerate() {
      let Some(impl_of) = &symbol.impl_of else { continue };
      let method = SymRef { file, local: local as u32 };
      let Some(method_entity) = resolver.table.symbol_entity(method) else { continue };
      if let Some(Found::Symbol(owner)) = resolver.rust_method_self(method)
        && let Some(owner_entity) = resolver.table.symbol_entity(owner)
      {
        self.add(Bucket::Structural, EdgeKind::Contains, owner_entity, method_entity, Certainty::Resolved, symbol.span);
      }
      if impl_of.trait_ref.is_some()
        && let Some(Found::Symbol(trait_symbol)) = resolver.rust_method_trait(method)
        && let Some(declaration) = resolver.child_named(trait_symbol, &symbol.name)
        && let Some(declaration_entity) = resolver.table.symbol_entity(declaration)
      {
        self.add(
          Bucket::Structural,
          EdgeKind::Implements,
          method_entity,
          declaration_entity,
          Certainty::Resolved,
          symbol.span,
        );
      }
    }
    for (position, implementation) in surface.rust.impls.iter().enumerate() {
      if implementation.trait_ref.is_none() {
        continue;
      }
      let (Some(Found::Symbol(owner)), Some(Found::Symbol(trait_symbol))) = resolver.rust_impl_targets(file, position as u32) else {
        continue;
      };
      if resolver.sym_kind(trait_symbol) != Some(&EntityKind::Trait) {
        continue;
      }
      if let (Some(owner_entity), Some(trait_entity)) = (resolver.table.symbol_entity(owner), resolver.table.symbol_entity(trait_symbol)) {
        self.add(
          Bucket::Structural,
          EdgeKind::Implements,
          owner_entity,
          trait_entity,
          Certainty::Resolved,
          implementation.span,
        );
      }
    }
    let Some(file_entity) = self.local_entity(None) else { return };
    for entry in &surface.rust.uses {
      for hit in resolver.rust_use_targets(entry) {
        if matches!(hit.found, Found::External(_)) {
          continue;
        }
        self.add_hit(Bucket::Structural, EdgeKind::Imports, file_entity, &hit, entry.span);
        if entry.visibility != Visibility::Private {
          self.add_hit(Bucket::Structural, EdgeKind::ReExports, file_entity, &hit, entry.span);
        }
      }
    }
  }

  fn ts_structure(&mut self, facts: &FileFacts) {
    let resolver = self.resolver;
    let file = self.file();
    for (local, symbol) in resolver.surface.symbols.iter().enumerate() {
      let reference = SymRef { file, local: local as u32 };
      let Some(entity) = self.local_entity(Some(local as u32)) else { continue };
      let heritage = symbol
        .extends
        .iter()
        .map(|path| (EdgeKind::Extends, path))
        .chain(symbol.implements.iter().map(|path| (EdgeKind::Implements, path)));
      for (kind, path) in heritage {
        if let Ok(hit) = resolver.ts_resolve_heritage(reference, path)
          && matches!(hit.found, Found::Symbol(_))
        {
          self.add_hit(Bucket::Structural, kind, entity, &hit, symbol.span);
        }
      }
    }
    let Some(file_entity) = self.local_entity(None) else { return };
    for import in &facts.imports {
      match import {
        ImportFact::TsImport { specifier, bindings, span, .. } => self.ts_import(file_entity, specifier, bindings, *span),
        ImportFact::TsRequire { specifier, bindings, scope, span } => match specifier.as_static_str() {
          Some(literal) => self.ts_import(file_entity, &literal, bindings, *span),
          None => self.dynamic_import("require", specifier, scope.map(|local| local.0), *span),
        },
        ImportFact::TsDynamicImport { specifier, scope, span } => match specifier.as_static_str() {
          Some(literal) => {
            let from = self.local_entity(scope.map(|local| local.0)).unwrap_or(file_entity);
            if let Ok(hit) = resolver.ts_specifier(file, &literal) {
              self.add_hit(Bucket::Structural, EdgeKind::Imports, from, &hit, *span);
            }
          }
          None => self.dynamic_import("import", specifier, scope.map(|local| local.0), *span),
        },
        _ => {}
      }
    }
    for export in &facts.exports {
      match export {
        ExportFact::ReExport { specifier, bindings, span, .. } => {
          let Ok(target) = resolver.ts_specifier(file, specifier) else { continue };
          let Found::File(module) = target.found else { continue };
          for binding in bindings {
            for hit in resolver.ts_export_both(module, &binding.imported) {
              self.add_hit(Bucket::Structural, EdgeKind::ReExports, file_entity, &hit.weaken(target.heuristic), *span);
            }
          }
        }
        ExportFact::ReExportAll { specifier, span, .. } => {
          if let Ok(target) = resolver.ts_specifier(file, specifier) {
            self.add_hit(Bucket::Structural, EdgeKind::ReExports, file_entity, &target, *span);
          }
        }
        ExportFact::Named { local, span, .. } => {
          let is_declared_here = resolver.surface.items_named(&[], local).iter().next().is_some();
          if !is_declared_here && let Ok(hit) = resolver.ts_lookup_name(file, None, local, TsNs::Value, &mut Guard::default()) {
            self.add_hit(Bucket::Structural, EdgeKind::ReExports, file_entity, &hit, *span);
          }
        }
        _ => {}
      }
    }
  }

  fn ts_import(&mut self, file_entity: EntityId, specifier: &str, bindings: &[ImportBinding], span: Span) {
    let resolver = self.resolver;
    let Ok(target) = resolver.ts_specifier(self.file(), specifier) else { return };
    let Found::File(module) = target.found else { return };
    self.add_hit(Bucket::Structural, EdgeKind::Imports, file_entity, &target, span);
    for binding in bindings {
      let name = match binding {
        ImportBinding::Named { imported, .. } => imported.as_str(),
        ImportBinding::Default { .. } => "default",
        ImportBinding::Namespace { .. } => continue,
      };
      for hit in resolver.ts_export_both(module, name) {
        self.add_hit(Bucket::Structural, EdgeKind::Imports, file_entity, &hit.weaken(target.heuristic), span);
      }
    }
  }

  fn dynamic_import(&mut self, callee: &str, specifier: &ArgSummary, scope: Option<u32>, span: Span) {
    let Some(from) = self.local_entity(scope) else { return };
    self.unresolved.push(UnresolvedRef {
      from,
      path: vec![callee.to_owned(), specifier_display(specifier)],
      kind: RefKind::Call,
      receiver: None,
      reason: UnresolvedReason::Dynamic,
      repo: self.resolver.surface.repo.clone(),
      file: self.resolver.surface.path.clone(),
      span,
    });
  }
}
