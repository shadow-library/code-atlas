use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use atlas_core::{ContentHash, Entity, EntityKind, RunId};
use atlas_edit::{EditScope, FileTarget, LeadingRegion, Nesting, SymbolOp, SymbolPlan, SymbolTarget, Syntax, plan_symbol_edit};
use atlas_engine::EngineError;
use atlas_lsp::protocol::{DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoResponse, ReferenceContext, ReferenceParams, TextDocumentIdentifier};
use atlas_lsp::{LineIndex, SyncDemand, uri_from_path};
use atlas_query::{EntityRef, NameSpan, Response, Targeted};
use atlas_store::EntityFilter;

use super::extras::{GraphSites, Reach, VerifyReason, VerifySite};
use super::plans::{PlanDraft, PlanGuard};
use super::rename::{partial_view, refuse, untargeted};
use super::{DryRunReason, EditOutcome, OperationView, changed_files, planning_error, respond, sync_refusal};
use crate::error::{IdeError, IdeResult};
use crate::map::SiteMapper;
use crate::references::{anchor_position, unsupported};
use crate::service::{IdeService, Session, guard};
use crate::target::{Anchor, FileRef, Resolver, Target, offset_of};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SymbolEdit {
  ReplaceBody(String),
  InsertBefore(String),
  InsertAfter(String),
}

impl SymbolEdit {
  fn view(&self, target: String) -> OperationView {
    match self {
      Self::ReplaceBody(_) => OperationView::ReplaceBody { target },
      Self::InsertBefore(_) => OperationView::InsertBefore { target },
      Self::InsertAfter(_) => OperationView::InsertAfter { target },
    }
  }

  fn op(self) -> SymbolOp {
    match self {
      Self::ReplaceBody(text) => SymbolOp::ReplaceBody(text),
      Self::InsertBefore(text) => SymbolOp::InsertBefore(text),
      Self::InsertAfter(text) => SymbolOp::InsertAfter(text),
    }
  }
}

pub(crate) struct SymbolSite {
  entity: Entity,
  file: FileRef,
  text: String,
  hash: ContentHash,
  name: Range<usize>,
  target: SymbolTarget,
}

impl SymbolSite {
  fn anchor(&self) -> Anchor {
    Anchor {
      file: self.file.clone(),
      text: self.text.clone(),
      hash: self.hash,
      offset: self.name.start,
    }
  }

  fn short(&self) -> String {
    EntityRef::of(&self.entity).short
  }
}

impl IdeService {
  pub(crate) fn symbol_edit(&self, spec: &str, edit: SymbolEdit, dry_run: bool) -> IdeResult<Response<EditOutcome>> {
    let mut operation = edit.view(spec.to_owned());
    let files = self.files();
    let mut freshness = self.query.fresh()?;
    let resolver = Resolver {
      query: &self.query,
      files: &files,
    };
    let target = match resolver.resolve(spec, &mut freshness)? {
      Targeted::Found(target) => target,
      other => return untargeted(freshness, operation, other),
    };
    let site = match self.symbol_site(&target)? {
      Ok(site) => site,
      Err(reason) => return respond(freshness, EditOutcome::refused(operation, reason)),
    };
    operation = edit.view(site.short());
    if let Some(reason) = self.overload_refusal(&site)? {
      return respond(freshness, EditOutcome::refused(operation, reason));
    }
    let project = self.query.engine().project();
    let scope = EditScope::new(&project)?;
    if let Some(reason) = unrestated_keyword(&site, &edit) {
      return respond(freshness, EditOutcome::refused(operation, reason));
    }
    let planned = match plan_symbol_edit(&scope, &site.target, &edit.op()) {
      Ok(planned) => planned,
      Err(error) => return respond(freshness, planning_error(&operation, error)?),
    };
    let mut caveats = Vec::new();
    let mut sessions = Vec::new();
    let warm = self.warm_unit(&site.file);
    let lock = warm.as_ref().map(|unit| Arc::clone(&unit.lock));
    let _held = lock.as_ref().map(|lock| guard(lock));
    match &warm {
      Some(unit) => match self.connect_live(unit, freshness.run.map(RunId), &files, std::slice::from_ref(&site.file.absolute)) {
        Ok(session) => {
          if let Some(reason) = cross_check(&session, &site, &planned.region)? {
            return refuse(freshness, operation, session, reason);
          }
          sessions.push(session);
        }
        Err(error) => caveats.push(format!(
          "the running {} was not consulted ({error}); the declaration span comes from the index only",
          unit.kind.name()
        )),
      },
      None => caveats
        .push("no language server is running for this file: the declaration span comes from the index alone, without a cross-check, and diagnostics are not checked".to_owned()),
    }
    let originals = BTreeMap::from([(site.target.file.clone(), site.text.clone())]);
    let plan = self.symbol_plan(operation, PlanGuard::Files, planned, &originals, caveats)?;
    self.conclude(freshness, plan, sessions, dry_run.then_some(DryRunReason::Requested))
  }

  fn symbol_plan(
    &self,
    operation: OperationView,
    guard: PlanGuard,
    planned: SymbolPlan,
    originals: &BTreeMap<FileTarget, String>,
    caveats: Vec<String>,
  ) -> IdeResult<super::IdePlan> {
    let draft = PlanDraft {
      project: self.query.engine().project().name.to_string(),
      operation,
      guard,
      files: changed_files(&planned.plan, originals, std::slice::from_ref(&planned.diff))?,
      resources: Vec::new(),
      edits: planned.plan,
      not_renamed: Vec::new(),
      applied_from_graph: Vec::new(),
      caveats,
      unverified: Vec::new(),
      unloaded_projects: Vec::new(),
      allow_unverified: false,
    };
    Ok(draft.seal())
  }

  pub(crate) fn safe_delete(&self, spec: &str, dry_run: bool) -> IdeResult<Response<EditOutcome>> {
    let mut operation = OperationView::SafeDelete { target: spec.to_owned() };
    self.ensure_enabled()?;
    let files = self.files();
    let mut freshness = self.query.fresh()?;
    let resolver = Resolver {
      query: &self.query,
      files: &files,
    };
    let target = match resolver.resolve(spec, &mut freshness)? {
      Targeted::Found(target) => target,
      other => return untargeted(freshness, operation, other),
    };
    let site = match self.symbol_site(&target)? {
      Ok(site) => site,
      Err(reason) => return respond(freshness, EditOutcome::refused(operation, reason)),
    };
    operation = OperationView::SafeDelete { target: site.short() };
    if let Some(reason) = self.overload_refusal(&site)? {
      return respond(freshness, EditOutcome::refused(operation, reason));
    }
    let graph = self.graph_sites(&site.entity, Reach::TargetOnly)?;
    let unit = self.unit(&site.file)?;
    let candidates = self.exact_candidates(&files, &graph.sites, &site.file, |path| unit.covers(path))?;
    let lock = Arc::clone(&unit.lock);
    let _serialised = guard(&lock);
    let mut session = match self.connect(&unit, freshness.run.map(RunId), &files, SyncDemand::Exact, &candidates.open) {
      Ok(session) => session,
      Err(error) => return respond(freshness, EditOutcome::refused(operation, sync_refusal(error)?)),
    };
    if let Err(error) = session.settle_projects() {
      return refuse(
        freshness,
        operation,
        session,
        format!("the language server did not finish loading the projects of the files that use the symbol ({error}); nothing was deleted"),
      );
    }
    if let Some(reason) = partial_view(&session) {
      return refuse(freshness, operation, session, reason);
    }
    if !session.capabilities().references_provider.is_supported() {
      return Err(unsupported(&session, "textDocument/references"));
    }
    let project = self.query.engine().project();
    let scope = EditScope::new(&project)?;
    let planned = match plan_symbol_edit(&scope, &site.target, &SymbolOp::SafeDelete) {
      Ok(planned) => planned,
      Err(error) => {
        let mut outcome = planning_error(&operation, error)?;
        outcome.servers = vec![session.finish()];
        return respond(freshness, outcome);
      }
    };
    if let Some(reason) = cross_check(&session, &site, &planned.region)? {
      return refuse(freshness, operation, session, reason);
    }
    let removed = planned.plan.files.first().and_then(|file| file.edits.first()).map(|edit| edit.range()).unwrap_or_default();
    if !candidates.unloaded.is_empty() {
      return refuse(
        freshness,
        operation,
        session,
        format!(
          "the language server could not load {} (unreadable tsconfig, no file to open), so references there cannot be ruled out; nothing was deleted",
          candidates.unloaded.join(", ")
        ),
      );
    }
    let (blockers, mut caveats) = self.delete_blockers(&mut session, &site, &removed, graph)?;
    caveats.extend(candidates.caveats);
    if !blockers.is_empty() {
      let count = blockers.len();
      let mut outcome = EditOutcome::refused(operation, format!("{count} reference(s) to {} would break; nothing was deleted", site.short()));
      outcome.blockers = blockers;
      outcome.caveats = caveats;
      outcome.servers = vec![session.finish()];
      return respond(freshness, outcome);
    }
    let originals = BTreeMap::from([(site.target.file.clone(), site.text.clone())]);
    let plan = self.symbol_plan(operation, PlanGuard::Project { run: freshness.run }, planned, &originals, caveats)?;
    self.conclude(freshness, plan, vec![session], dry_run.then_some(DryRunReason::Requested))
  }

  fn delete_blockers(&self, session: &mut Session<'_>, site: &SymbolSite, removed: &Range<usize>, graph: GraphSites) -> IdeResult<(Vec<VerifySite>, Vec<String>)> {
    let anchor = site.anchor();
    let params = ReferenceParams {
      position: atlas_lsp::protocol::TextDocumentPositionParams {
        text_document: TextDocumentIdentifier {
          uri: uri_from_path(&anchor.file.absolute).map_err(atlas_lsp::LspError::from)?,
        },
        position: anchor_position(session, &anchor)?,
      },
      context: ReferenceContext { include_declaration: false },
    };
    let locations: GotoResponse = session.request("textDocument/references", params)?;
    let sites = SiteMapper::new(session).map_all(locations.into_locations())?;
    let inside = |repo: Option<&str>, path: &str, line: u32, column: Option<u32>| {
      repo == Some(site.file.repo.as_str())
        && path == site.file.path.as_str()
        && column
          .and_then(|column| offset_of(&site.text, line, column).ok())
          .is_some_and(|offset| removed.contains(&offset))
    };
    let mut blockers: Vec<VerifySite> = sites
      .iter()
      .filter(|found| !inside(found.place.repo.as_deref(), &found.place.path, found.place.line, found.place.column))
      .map(|found| VerifySite {
        reason: VerifyReason::LanguageServer,
        repo: found.place.repo.clone().unwrap_or_else(|| "outside the project".to_owned()),
        path: found.place.path.clone(),
        line: Some(found.place.line),
        column: found.place.column,
        from: found.enclosing.as_ref().map(crate::map::Enclosing::label),
        edge_kind: None,
        certainty: Some("[compiler]".to_owned()),
        authoritative: true,
        snippet: found.snippet.clone(),
        note: found.unverified.clone(),
      })
      .collect();
    let mut caveats = graph.caveats;
    let returned = |candidate: &VerifySite| {
      sites.iter().any(|found| {
        found.place.repo.as_deref() == Some(candidate.repo.as_str())
          && found.place.path == candidate.path
          && Some(found.place.line) == candidate.line
          && (candidate.column.is_none() || found.place.column.is_none() || found.place.column == candidate.column)
      })
    };
    let mut heuristic = 0;
    for candidate in graph.sites {
      if returned(&candidate) || candidate.line.is_some_and(|line| inside(Some(&candidate.repo), &candidate.path, line, candidate.column)) {
        continue;
      }
      let seen = session
        .files
        .root(&atlas_core::RepoName::new(&candidate.repo).map_err(|error| IdeError::InvalidRequest(error.to_string()))?)
        .is_some_and(|root| session.lease.open_document(&root.join(&candidate.path)).is_some());
      match (candidate.reason, seen) {
        (VerifyReason::Heuristic, true) => heuristic += 1,
        (VerifyReason::Heuristic, false) => blockers.push(candidate.with_note("the language server never loaded this file, so it cannot rule this site out")),
        _ => blockers.push(candidate),
      }
    }
    if heuristic > 0 {
      caveats.push(format!(
        "{heuristic} heuristic graph site(s) the language server did not return were not counted as references"
      ));
    }
    Ok((blockers, caveats))
  }

  fn overload_refusal(&self, site: &SymbolSite) -> IdeResult<Option<String>> {
    let overloads = self.overloads(&site.entity)?;
    if overloads.is_empty() {
      return Ok(None);
    }
    let mut lines: Vec<u32> = overloads
      .iter()
      .chain(std::iter::once(&site.entity))
      .filter_map(|entity| entity.span.map(|span| span.start_line))
      .collect();
    lines.sort_unstable();
    Ok(Some(format!(
      "{} is one of {} overload signatures (lines {}); a symbol edit would change only one of them, so edit the set with replace_content; nothing was written",
      site.entity.name,
      lines.len(),
      lines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )))
  }

  pub(crate) fn symbol_site(&self, target: &Target) -> IdeResult<Result<SymbolSite, String>> {
    let (entity, file, name, indexed) = match target {
      Target::Symbol { entity, file, name, indexed } => (entity.as_ref().clone(), file.clone(), *name, *indexed),
      Target::Position { file, line, column } => {
        let Some(entity) = self.query.innermost_symbol(&file.repo, &file.path, *line, *column)? else {
          return Ok(Err(format!("{file}:{line}:{column} is not inside an indexed symbol; give a symbol name or key")));
        };
        let span = match self.query.name_span(&entity)? {
          NameSpan::Found { span } if span.start_line == *line && span.start_col + 1 == *column => span,
          NameSpan::SourceChanged => return Err(IdeError::SourceChanged { file: file.to_string() }),
          _ => return Ok(Err(format!("{file}:{line}:{column} is not on the name of {}; give a symbol name or key", entity.name))),
        };
        let indexed = self
          .query
          .engine()
          .store()
          .read(|tx| tx.file(&file.repo, &file.path))?
          .map(|record| record.content_hash)
          .ok_or_else(|| IdeError::SourceChanged { file: file.to_string() })?;
        (entity, file.clone(), span, indexed)
      }
    };
    let Some(span) = entity.span else {
      return Ok(Err(format!("{} has no declaration span in the index", entity.key)));
    };
    let (text, hash) = match self.query.engine().read_source(&file.repo, &file.path) {
      Ok(read) => read,
      Err(EngineError::SourceChanged { .. }) => return Err(IdeError::SourceChanged { file: file.to_string() }),
      Err(error) => return Err(error.into()),
    };
    if hash != indexed {
      return Err(IdeError::SourceChanged { file: file.to_string() });
    }
    let syntax = match file.absolute.extension().and_then(|extension| extension.to_str()) {
      Some("rs") => Syntax::Rust,
      _ => Syntax::TypeScript,
    };
    let declarator = span.start_byte as usize..span.end_byte as usize;
    let declaration = match syntax {
      Syntax::TypeScript => match declaration_statement(&text, declarator, &entity.kind) {
        Ok(statement) => statement,
        Err(reason) => return Ok(Err(format!("{}{reason}; edit it with replace_content; nothing was written", entity.name))),
      },
      Syntax::Rust => declarator,
    };
    let name_range = name.start_byte as usize..name.end_byte as usize;
    if name_range.start < declaration.start || name_range.end > declaration.end {
      return Ok(Err(format!(
        "the index's name and declaration spans of {} disagree; edit it with replace_content",
        entity.name
      )));
    }
    if let Some(reason) = self.shared_declaration(&entity, &file, &declaration)? {
      return Ok(Err(reason));
    }
    let (floor, nesting) = self.floor(&entity, &file, &declaration)?;
    let target = SymbolTarget {
      file: FileTarget::new(file.repo.clone(), file.path.clone()),
      expected: hash,
      span: declaration,
      floor,
      syntax,
      nesting,
    };
    Ok(Ok(SymbolSite {
      entity,
      file,
      text,
      hash,
      name: name_range,
      target,
    }))
  }

  fn shared_declaration(&self, entity: &Entity, file: &FileRef, declaration: &Range<usize>) -> IdeResult<Option<String>> {
    let others = self
      .query
      .engine()
      .store()
      .read(|tx| tx.entities_in_file(&file.repo, &file.path, &EntityFilter::default()))?;
    let shared = others.into_iter().filter(|other| other.id != entity.id && other.kind.is_code_symbol()).find(|other| {
      let Some(span) = other.span else { return false };
      let range = span.start_byte as usize..span.end_byte as usize;
      let overlaps = range.start < declaration.end && declaration.start < range.end;
      let inside = declaration.start <= range.start && range.end <= declaration.end && range != *declaration;
      let around = range.start <= declaration.start && declaration.end <= range.end && range.len() > declaration.len();
      overlaps && !inside && !around
    });
    Ok(shared.map(|other| {
      format!(
        "{} shares its declaration with {}; a symbol edit would change both, so edit it with replace_content; nothing was written",
        entity.name, other.name
      )
    }))
  }

  fn floor(&self, entity: &Entity, file: &FileRef, declaration: &Range<usize>) -> IdeResult<(usize, Nesting)> {
    let symbols: Vec<Entity> = self
      .query
      .engine()
      .store()
      .read(|tx| tx.entities_in_file(&file.repo, &file.path, &EntityFilter::default()))?
      .into_iter()
      .filter(|other| other.id != entity.id && other.kind.is_code_symbol())
      .filter(|other| other.repo.as_ref() == Some(&file.repo) && other.path.as_ref() == Some(&file.path))
      .collect();
    let bytes = |other: &Entity| other.span.map(|span| span.start_byte as usize..span.end_byte as usize);
    let parent = symbols
      .iter()
      .filter_map(|other| bytes(other).map(|range| (other, range)))
      .filter(|(_, range)| range.start <= declaration.start && declaration.end <= range.end && range.len() > declaration.len())
      .min_by_key(|(_, range)| range.len());
    let within = parent.as_ref().map_or(0..usize::MAX, |(_, range)| range.clone());
    let previous = symbols
      .iter()
      .filter_map(bytes)
      .filter(|range| range.end <= declaration.start && within.start <= range.start && range.end <= within.end)
      .map(|range| range.end)
      .max();
    let parent_body = match &parent {
      Some((parent, range)) => match self.query.name_span(parent)? {
        NameSpan::Found { span } => Some(span.end_byte as usize),
        NameSpan::SourceChanged | NameSpan::Unavailable { .. } => Some(range.start),
      },
      None => None,
    };
    let floor = previous.into_iter().chain(parent_body).max().unwrap_or(0);
    let nesting = match parent {
      Some(_) => Nesting::Nested,
      None => Nesting::TopLevel,
    };
    Ok((floor, nesting))
  }
}

fn unrestated_keyword(site: &SymbolSite, edit: &SymbolEdit) -> Option<String> {
  let SymbolEdit::ReplaceBody(text) = edit else { return None };
  let head = site.text[site.target.span.start..site.name.start].split_whitespace().collect::<Vec<_>>().join(" ");
  let code = text
    .lines()
    .map(str::trim)
    .find(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('*') && !line.starts_with("/*") && !line.starts_with('@'))?;
  let spans_keywords = declaration_keywords(&head);
  let written = code
    .split_whitespace()
    .take_while(|word| matches!(*word, "export" | "declare" | "const" | "let" | "var"))
    .collect::<Vec<_>>()
    .join(" ");
  match (spans_keywords, written == head) {
    (true, false) => Some(format!(
      "replace_body replaces the whole `{head} {}` statement; restate it from `{head}`; nothing was written",
      site.entity.name
    )),
    (false, _) if declaration_keywords(&written) => Some(format!(
      "the new text starts with `{written}`, which the declaration of {} does not contain; nothing was written",
      site.entity.name
    )),
    _ => None,
  }
}

fn declaration_keywords(words: &str) -> bool {
  let words: Vec<&str> = words.split_whitespace().collect();
  let Some((last, modifiers)) = words.split_last() else { return false };
  matches!(*last, "const" | "let" | "var") && modifiers.iter().all(|word| matches!(*word, "export" | "declare"))
}

fn declaration_statement(text: &str, declarator: Range<usize>, kind: &EntityKind) -> Result<Range<usize>, &'static str> {
  let line_start = text[..declarator.start].rfind('\n').map_or(0, |at| at + 1);
  let prefix = &text[line_start..declarator.start];
  let keyword = prefix.trim_start();
  let variable = matches!(kind, EntityKind::Constant | EntityKind::Variable);
  let together = " is declared together with other variables in one statement";
  let after = text[declarator.end..].trim_start_matches([' ', '\t']);
  if prefix.trim_end().ends_with(',') || (variable && after.starts_with(',')) {
    return Err(together);
  }
  let start = match keyword.is_empty() {
    true => {
      let before = text[..line_start].trim_end();
      let previous_start = before.rfind('\n').map_or(0, |at| at + 1);
      let previous = &before[previous_start..];
      match declaration_keywords(previous) {
        true => previous_start + (previous.len() - previous.trim_start().len()),
        false if variable && before.ends_with(',') => return Err(together),
        false => return Ok(declarator),
      }
    }
    false if declaration_keywords(keyword) && keyword.ends_with(char::is_whitespace) => line_start + (prefix.len() - keyword.len()),
    false => return Ok(declarator),
  };
  if text[declarator.clone()].starts_with(['{', '[']) {
    return Err(" is bound by a destructuring pattern together with other bindings");
  }
  if after.starts_with(',') {
    return Err(together);
  }
  let end = match after.strip_prefix(';') {
    Some(_) => declarator.end + (text[declarator.end..].len() - after.len()) + 1,
    None => declarator.end,
  };
  Ok(start..end)
}

struct Outlined {
  range: Range<usize>,
  selection: Range<usize>,
}

fn outlined(symbols: &[DocumentSymbol], index: &LineIndex<'_>, encoding: atlas_lsp::PositionEncoding, into: &mut Vec<Outlined>) {
  for symbol in symbols {
    if let (Ok(range), Ok(selection)) = (index.offsets(symbol.range, encoding), index.offsets(symbol.selection_range, encoding)) {
      into.push(Outlined { range, selection });
    }
    outlined(&symbol.children, index, encoding, into);
  }
}

fn cross_check(session: &Session<'_>, site: &SymbolSite, region: &LeadingRegion) -> IdeResult<Option<String>> {
  let at = format!("{}:{}", site.file, site.text[..site.name.start].matches('\n').count() + 1);
  match session.lease.open_document(&site.file.absolute) {
    Some(document) if document.hash == site.hash => {}
    _ => return Ok(Some(format!("{} changed while the edit was planned; nothing was written", site.file))),
  }
  if !session.capabilities().document_symbol_provider.is_supported() {
    return Ok(None);
  }
  let response: DocumentSymbolResponse = session.request(
    "textDocument/documentSymbol",
    DocumentSymbolParams {
      text_document: TextDocumentIdentifier {
        uri: uri_from_path(&site.file.absolute).map_err(atlas_lsp::LspError::from)?,
      },
    },
  )?;
  let index = LineIndex::new(&site.text, session.lease.line_breaks());
  let encoding = session.lease.encoding();
  let mut symbols = Vec::new();
  match &response {
    DocumentSymbolResponse::Nested(nested) => outlined(nested, &index, encoding, &mut symbols),
    DocumentSymbolResponse::Flat(flat) => symbols.extend(
      flat
        .iter()
        .filter_map(|symbol| index.offsets(symbol.location.range, encoding).ok())
        .map(|range| Outlined { selection: range.clone(), range }),
    ),
  }
  let Some(server) = symbols
    .iter()
    .filter(|symbol| symbol.selection.start <= site.name.start && site.name.start < symbol.selection.end.max(symbol.selection.start + 1))
    .min_by_key(|symbol| symbol.range.len())
  else {
    return Ok(Some(format!(
      "server and index disagree at {at}: {} has no symbol named there; nothing was written",
      session.server_name()
    )));
  };
  let declaration = &site.target.span;
  let gap = site
    .text
    .get(declaration.end.min(server.range.end)..declaration.end.max(server.range.end))
    .unwrap_or_default();
  let starts_inside = region.start <= server.range.start && server.range.start <= site.name.start;
  let ends_together = gap.trim().trim_matches([';', ',']).is_empty();
  match starts_inside && ends_together {
    true => Ok(None),
    false => Ok(Some(format!(
      "server and index disagree about the extent of {} at {at}: the index spans bytes {}..{}, {} spans {}..{}; nothing was written",
      site.entity.name,
      declaration.start,
      declaration.end,
      session.server_name(),
      server.range.start,
      server.range.end
    ))),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_widen_a_lone_declarator_to_its_statement_and_refuse_shared_ones() {
    let widen = |text: &str, name: &str, kind: EntityKind| {
      let start = text.find(name).unwrap();
      let end = text[start..].find([',', ';', '\n']).map_or(text.len(), |at| start + at);
      declaration_statement(text, start..end, &kind).map(|range| text[range].to_owned())
    };

    assert_eq!(widen("export const a = 1;\n", "a =", EntityKind::Constant), Ok("export const a = 1;".to_owned()));
    assert_eq!(widen("  const a = () => 1\n", "a =", EntityKind::Function), Ok("const a = () => 1".to_owned()));
    assert_eq!(
      widen("const a = 1, b = 2;\n", "a =", EntityKind::Constant),
      Err(" is declared together with other variables in one statement")
    );
    assert_eq!(
      widen("const a = 1, b = 2;\n", "b =", EntityKind::Constant),
      Err(" is declared together with other variables in one statement")
    );
    assert_eq!(
      widen("const a = 1,\n  b = 2;\n", "b =", EntityKind::Constant),
      Err(" is declared together with other variables in one statement")
    );
    assert_eq!(widen("enum E {\n  A = 1,\n  B = 2\n}\n", "B =", EntityKind::Field), Ok("B = 2".to_owned()));
    assert_eq!(
      widen("export const\n  split = 3\n", "split =", EntityKind::Constant),
      Ok("export const\n  split = 3".to_owned())
    );
    let destructured = "export const { alpha, beta } = { alpha: 1, beta: 2 }\n";
    assert_eq!(
      declaration_statement(destructured, 13..52, &EntityKind::Constant),
      Err(" is bound by a destructuring pattern together with other bindings")
    );
  }

  #[test]
  fn should_flatten_nested_document_symbols_into_byte_ranges() {
    let symbol = |name: &str, range: ((u32, u32), (u32, u32)), selection: ((u32, u32), (u32, u32)), children: Vec<DocumentSymbol>| DocumentSymbol {
      name: name.to_owned(),
      detail: None,
      kind: atlas_lsp::protocol::SymbolKind(12),
      tags: Vec::new(),
      deprecated: None,
      range: lsp_range(range),
      selection_range: lsp_range(selection),
      children,
    };
    let text = "class A {\n  run() {}\n}\n";
    let index = LineIndex::new(text, atlas_lsp::LineBreaks::Ecma);
    let mut flat = Vec::new();

    outlined(
      &[symbol(
        "A",
        ((0, 0), (2, 1)),
        ((0, 6), (0, 7)),
        vec![symbol("run", ((1, 2), (1, 10)), ((1, 2), (1, 5)), Vec::new())],
      )],
      &index,
      atlas_lsp::PositionEncoding::Utf16,
      &mut flat,
    );

    let ranges: Vec<(Range<usize>, Range<usize>)> = flat.into_iter().map(|symbol| (symbol.range, symbol.selection)).collect();
    assert_eq!(ranges, vec![(0..22, 6..7), (12..20, 12..15)]);
  }

  fn lsp_range(range: ((u32, u32), (u32, u32))) -> atlas_lsp::protocol::Range {
    atlas_lsp::protocol::Range {
      start: atlas_lsp::protocol::Position {
        line: range.0.0,
        character: range.0.1,
      },
      end: atlas_lsp::protocol::Position {
        line: range.1.0,
        character: range.1.1,
      },
    }
  }
}
