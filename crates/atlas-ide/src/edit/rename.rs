use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_core::{ContentHash, Entity, RunId};
use atlas_edit::{EditPlan, EditScope, FileEdits, FileTarget, ResourceOp, ScopedPath, TextEdit};
use atlas_lsp::protocol::{DocumentChange, PrepareRenameResult, RenameParams, WorkspaceEdit};
use atlas_lsp::{LineIndex, LspError, ServerKind, SyncDemand, canonical_path, path_from_uri};
use atlas_query::{EntityRef, Freshness, Response, Targeted};
use atlas_store::EntityFilter;

use super::extras::{Coverage, GraphRename, Reach, VerifyReason, VerifySite, is_identifier_char, reads_token};
use super::plans::{PlanDraft, PlanGuard};
use super::{CrossRepo, DryRunReason, EditOutcome, OperationView, changed_files, resource_labels, respond, sync_refusal};
use crate::error::{IdeError, IdeResult};
use crate::references::{position_params, unsupported};
use crate::servers::kind_of;
use crate::service::{IdeService, Session, guard};
use crate::target::{Anchor, Resolver};

const RENAME: &str = "textDocument/rename";
const PREPARE_RENAME: &str = "textDocument/prepareRename";

pub(crate) struct Converted {
  pub plan: EditPlan,
  pub originals: BTreeMap<FileTarget, String>,
}

pub(crate) enum Conversion {
  Ready(Converted),
  Stale { reason: String, paths: Vec<PathBuf> },
  Refused(String),
}

pub(crate) struct Expectation<'a> {
  pub old_name: &'a str,
  pub new_name: &'a str,
}

pub(crate) struct RenameCall<'a> {
  pub spec: &'a str,
  pub new_name: &'a str,
  pub cross_repo: CrossRepo,
  pub allow_unverified: bool,
  pub dry_run: bool,
}

impl IdeService {
  pub(crate) fn rename(&self, call: &RenameCall<'_>) -> IdeResult<Response<EditOutcome>> {
    let (new_name, cross_repo) = (call.new_name, call.cross_repo);
    let mut operation = OperationView::Rename {
      target: call.spec.to_owned(),
      new_name: new_name.to_owned(),
      cross_repo,
    };
    self.ensure_enabled()?;
    let files = self.files();
    let mut freshness = self.query.fresh()?;
    let resolver = Resolver {
      query: &self.query,
      files: &files,
    };
    let target = match resolver.resolve(call.spec, &mut freshness)? {
      Targeted::Found(target) => target,
      other => return untargeted(freshness, operation, other),
    };
    let anchor = resolver.anchor(&target)?;
    let mut caveats = Vec::new();
    let entity = self.graph_target(&target, &mut caveats)?;
    if let Some(entity) = &entity {
      operation = OperationView::Rename {
        target: EntityRef::of(entity).short,
        new_name: new_name.to_owned(),
        cross_repo,
      };
    }
    let kind = kind_of(&anchor.file.absolute).ok_or_else(|| IdeError::UnsupportedFile { path: anchor.file.to_string() })?;
    validate_name(new_name, kind)?;
    if let Some(entity) = &entity
      && let Some(conflict) = self.name_conflict(entity, new_name)?
    {
      return respond(freshness, EditOutcome::refused(operation, conflict));
    }
    let graph = entity.as_ref().map(|entity| self.graph_sites(entity, Reach::Family)).transpose()?;
    let unit = self.unit(&anchor.file)?;
    let candidates = self.exact_candidates(&files, graph.as_ref().map_or(&[], |graph| &graph.sites), &anchor.file, |path| unit.covers(path))?;
    caveats.extend(candidates.caveats.iter().cloned());
    let lock = Arc::clone(&unit.lock);
    let _serialised = guard(&lock);
    let generation = freshness.run.map(RunId);
    let mut session = match self.connect(&unit, generation, &files, SyncDemand::Exact, &candidates.open) {
      Ok(session) => session,
      Err(error) => return respond(freshness, EditOutcome::refused(operation, sync_refusal(error)?)),
    };
    if let Err(error) = session.settle_projects() {
      return refuse(
        freshness,
        operation,
        session,
        format!("the language server did not finish loading the projects of the files to rename ({error}); nothing was written"),
      );
    }
    if let Some(reason) = partial_view(&session) {
      return refuse(freshness, operation, session, reason);
    }
    if !session.capabilities().rename_provider.is_supported() {
      return Err(unsupported(&session, RENAME));
    }
    let old_name = match prepare(&session, &anchor, entity.as_ref())? {
      Ok(name) => name,
      Err(reason) => return refuse(freshness, operation, session, reason),
    };
    if old_name == new_name || old_name.strip_prefix("r#") == Some(new_name) {
      return refuse(freshness, operation, session, format!("{old_name} is already named {new_name}"));
    }
    let project = self.query.engine().project();
    let scope = EditScope::new(&project)?;
    let expectation = Expectation {
      old_name: old_name.strip_prefix("r#").unwrap_or(&old_name),
      new_name,
    };
    let mut resynced = false;
    let converted = loop {
      let edit = match request_rename(&session, &anchor, new_name)? {
        Ok(edit) => edit,
        Err(reason) => return refuse(freshness, operation, session, reason),
      };
      match convert(&session, &scope, edit, &expectation)? {
        Conversion::Ready(converted) => break converted,
        Conversion::Refused(reason) => return refuse(freshness, operation, session, reason),
        Conversion::Stale { reason, .. } if resynced => {
          return refuse(
            freshness,
            operation,
            session,
            format!("the language server's view still disagrees with the files on disk after re-syncing them ({reason}); nothing was written"),
          );
        }
        Conversion::Stale { reason, mut paths } => {
          tracing::info!(%reason, files = paths.len(), "the rename does not match the files on disk; re-syncing them and asking again");
          resynced = true;
          paths.extend(candidates.open.iter().cloned());
          if let Err(error) = session.resync(generation, SyncDemand::Exact, &paths) {
            return refuse(freshness, operation, session, sync_refusal(error)?);
          }
          if let Some(reason) = partial_view(&session) {
            return refuse(freshness, operation, session, reason);
          }
        }
      }
    };
    if !covers_anchor(&converted.plan, &anchor) {
      return refuse(
        freshness,
        operation,
        session,
        format!(
          "the server's rename does not change the name at {}; refusing a rename that leaves its own target untouched",
          place(&anchor)
        ),
      );
    }
    let Converted { mut plan, mut originals } = converted;
    let member = entity.as_ref().and_then(|entity| entity.key.as_symbol()).is_some_and(|key| key.item_path().len() > 1);
    if let Some(conflict) = (!member).then(|| shadowed_binding(&plan, &originals, new_name, kind)).flatten() {
      return refuse(freshness, operation, session, conflict);
    }
    let aliases = kept_aliases(&plan, &originals, &expectation);
    if !aliases.is_empty() {
      caveats.push(format!(
        "the language server kept the old name {} at {}; code that reaches it through those places keeps working under the old name",
        expectation.old_name,
        aliases.iter().map(|alias| alias.label.clone()).collect::<Vec<_>>().join(", ")
      ));
    }
    let target_repo = anchor.file.repo.as_str();
    let (not_renamed, applied_from_graph, unverified) = match graph {
      Some(graph) => {
        caveats.extend(graph.caveats);
        let coverage = Coverage::of_plan(&plan, &originals);
        let alias_files: Vec<&FileTarget> = aliases.iter().map(|alias| &alias.file).collect();
        let (via_alias, unmatched): (Vec<_>, Vec<_>) = graph
          .sites
          .into_iter()
          .filter(|site| !coverage.covers(site))
          .partition(|site| reaches_through(&files, &originals, site, expectation.old_name, &alias_files));
        let alias_note = format!(
          "via kept alias {}; it still compiles through the alias and was not changed",
          aliases
            .iter()
            .map(|alias| format!("`{}`", alias.text))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
        );
        let (mut remaining, applied) = match cross_repo {
          CrossRepo::Report => (unmatched, Vec::new()),
          CrossRepo::Apply => {
            let rename = GraphRename {
              files: &files,
              scope: &scope,
              old_name: expectation.old_name,
              new_name,
            };
            let applied = self.apply_graph_sites(&rename, &mut plan, &mut originals, unmatched)?;
            if let Err(violation) = plan.validate() {
              return refuse(freshness, operation, session, format!("the graph sites conflict with the server's edits: {violation}"));
            }
            (applied.remaining, applied.applied)
          }
        };
        let unverified: Vec<VerifySite> = remaining
          .iter()
          .filter(|site| site.reason == VerifyReason::OutsideLanguageServer && site.authoritative && site.repo == target_repo)
          .cloned()
          .collect();
        remaining.extend(via_alias.into_iter().map(|site| site.with_note(alias_note.clone())));
        remaining.sort_by_key(VerifySite::sort_key);
        (remaining, applied, unverified)
      }
      None => (Vec::new(), Vec::new(), Vec::new()),
    };
    let draft = PlanDraft {
      project: project.name.to_string(),
      operation,
      guard: PlanGuard::Project { run: freshness.run },
      files: changed_files(&plan, &originals, &[])?,
      resources: resource_labels(&plan.resources),
      edits: plan,
      not_renamed,
      applied_from_graph,
      caveats,
      unverified,
      unloaded_projects: candidates.unloaded.clone(),
      allow_unverified: call.allow_unverified,
    };
    self.conclude(freshness, draft.seal(), vec![session], call.dry_run.then_some(DryRunReason::Requested))
  }
}

impl IdeService {
  fn name_conflict(&self, entity: &Entity, new_name: &str) -> IdeResult<Option<String>> {
    let Some(key) = entity.key.as_symbol() else { return Ok(None) };
    let Some((_, parent)) = key.item_path().split_last() else { return Ok(None) };
    let bare = new_name.strip_prefix("r#").unwrap_or(new_name);
    let others = self.query.engine().store().read(|tx| tx.entities_by_name(bare, &EntityFilter::default()))?;
    let clash = others.into_iter().filter(|other| other.id != entity.id && other.kind.is_code_symbol()).find(|other| {
      other.key.as_symbol().is_some_and(|other| {
        other.family() == key.family()
          && other.repo() == key.repo()
          && other.package() == key.package()
          && other.module_path() == key.module_path()
          && other.item_path().split_last().is_some_and(|(_, other_parent)| other_parent == parent)
      })
    });
    Ok(clash.map(|other| {
      let shown = EntityRef::of(&other);
      let place = shown.location.map_or_else(String::new, |location| format!(" at {location}"));
      format!(
        "{bare} already exists in the same scope ({} {}{place}); the rename would clash with it; nothing was written",
        shown.kind, shown.short
      )
    }))
  }
}

pub(crate) fn untargeted<T>(freshness: Freshness, operation: OperationView, answer: Targeted<T>) -> IdeResult<Response<EditOutcome>> {
  let outcome = match answer {
    Targeted::Found(_) => EditOutcome::refused(operation, "the target was found but not used"),
    Targeted::NotFound { spec, hint } => EditOutcome::refused(operation, format!("no symbol matches {spec:?} ({hint}); nothing was written")),
    Targeted::Ambiguous { spec, candidates } => {
      let mut outcome = EditOutcome::refused(
        operation,
        format!(
          "{spec:?} matches {} symbols; give a full key or [repo:]path:line:col; nothing was written",
          candidates.len()
        ),
      );
      outcome.candidates = candidates;
      outcome
    }
  };
  respond(freshness, outcome)
}

pub(crate) fn refuse(freshness: Freshness, operation: OperationView, session: Session<'_>, reason: String) -> IdeResult<Response<EditOutcome>> {
  let mut outcome = EditOutcome::refused(operation, reason);
  outcome.servers = vec![session.finish()];
  respond(freshness, outcome)
}

pub(crate) fn partial_view(session: &Session<'_>) -> Option<String> {
  let mut reasons = session.notes.partial.clone();
  if let Some(lost) = session.lease.sync_lost() {
    reasons.push(format!("document sync was lost ({lost})"));
  }
  if let Some(health) = session.lease.health_error() {
    reasons.push(format!(
      "{} reports an unhealthy workspace ({health}); fix the failing projects or exclude them with lsp.exclude",
      session.server_name()
    ));
  }
  if !session.notes.drifted.is_empty() {
    reasons.push(format!(
      "{} changed on disk after indexing, so the index and the server disagree; retry once the index has caught up",
      session.notes.drifted.join(", ")
    ));
  }
  match reasons.is_empty() {
    true => None,
    false => Some(format!(
      "the language server's view is incomplete ({}); an edit never proceeds on a partial view; nothing was written",
      reasons.join("; ")
    )),
  }
}

fn validate_name(name: &str, kind: ServerKind) -> IdeResult<()> {
  let invalid = |language: &str| IdeError::InvalidRequest(format!("{name:?} is not a valid {language} identifier"));
  match kind {
    ServerKind::TypeScript => {
      let mut chars = name.chars();
      let first = chars.next().is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$');
      match first && chars.all(is_identifier_char) {
        true => Ok(()),
        false => Err(invalid("TypeScript")),
      }
    }
    ServerKind::RustAnalyzer => {
      let bare = name.strip_prefix("r#").unwrap_or(name);
      let mut chars = bare.chars();
      let first = chars.next().is_some_and(|first| first.is_alphabetic() || first == '_');
      match first && chars.all(|character| character.is_alphanumeric() || character == '_') && bare != "_" {
        true => Ok(()),
        false => Err(invalid("Rust")),
      }
    }
  }
}

fn place(anchor: &Anchor) -> String {
  let line_start = anchor.text[..anchor.offset].rfind('\n').map_or(0, |at| at + 1);
  let line = anchor.text[..anchor.offset].matches('\n').count() + 1;
  format!("{}:{line}:{}", anchor.file, anchor.offset - line_start + 1)
}

fn identifier_at(text: &str, offset: usize) -> Option<String> {
  let rest = text.get(offset..)?;
  let length: usize = rest.chars().take_while(|character| is_identifier_char(*character)).map(char::len_utf8).sum();
  (length > 0).then(|| rest[..length].to_owned())
}

fn prepare(session: &Session<'_>, anchor: &Anchor, entity: Option<&Entity>) -> IdeResult<Result<String, String>> {
  let at = place(anchor);
  let fallback = || match entity {
    Some(entity) => Ok(entity.name.clone()),
    None => identifier_at(&anchor.text, anchor.offset).ok_or_else(|| format!("there is no identifier at {at}")),
  };
  if !session.capabilities().supports_prepare_rename() {
    return Ok(fallback());
  }
  let prepared = match session.request::<Option<PrepareRenameResult>>(PREPARE_RENAME, position_params(session, anchor)?) {
    Ok(prepared) => prepared,
    Err(IdeError::Lsp(LspError::Response { error, .. })) => {
      return Ok(Err(format!("{} cannot rename at {at}: {}", session.server_name(), error.message)));
    }
    Err(error) => return Err(error),
  };
  let range = match prepared {
    None => return Ok(Err(format!("{} cannot rename at {at}: it found nothing renameable there", session.server_name()))),
    Some(PrepareRenameResult::DefaultBehavior) => return Ok(fallback()),
    Some(PrepareRenameResult::Range(range) | PrepareRenameResult::RangeWithPlaceholder { range, .. }) => range,
  };
  let index = LineIndex::new(&anchor.text, session.lease.line_breaks());
  let Ok(bytes) = index.offsets_strict(range, session.lease.encoding()) else {
    return Ok(Err(format!("server and index disagree at {at}: the server's range does not fit the file")));
  };
  let text = &anchor.text[bytes.clone()];
  if !(bytes.start <= anchor.offset && anchor.offset <= bytes.end) {
    return Ok(Err(format!("server and index disagree at {at}: the server would rename {text:?}, which is elsewhere")));
  }
  match entity {
    Some(entity) if text != entity.name && text.strip_prefix("r#") != Some(entity.name.as_str()) => Ok(Err(format!(
      "server and index disagree at {at}: the server would rename {text:?}, the index names {:?}",
      entity.name
    ))),
    _ if text.is_empty() => Ok(Err(format!("server and index disagree at {at}: the server's range is empty"))),
    _ => Ok(Ok(text.to_owned())),
  }
}

fn request_rename(session: &Session<'_>, anchor: &Anchor, new_name: &str) -> IdeResult<Result<WorkspaceEdit, String>> {
  let params = RenameParams {
    position: position_params(session, anchor)?,
    new_name: new_name.to_owned(),
  };
  match session.request::<Option<WorkspaceEdit>>(RENAME, params) {
    Ok(Some(edit)) => Ok(Ok(edit)),
    Ok(None) => Ok(Err(format!("{} returned no edits for this rename; nothing was written", session.server_name()))),
    Err(IdeError::Lsp(LspError::Response { error, .. })) => Ok(Err(format!("{} refused the rename: {}; nothing was written", session.server_name(), error.message))),
    Err(IdeError::Lsp(LspError::Decode { message, .. })) => Ok(Err(format!("{}'s rename cannot be applied safely ({message}); nothing was written", session.server_name()))),
    Err(error) => Err(error),
  }
}

pub(crate) struct KeptAlias {
  pub file: FileTarget,
  pub label: String,
  pub text: String,
}

fn kept_aliases(plan: &EditPlan, originals: &BTreeMap<FileTarget, String>, expected: &Expectation<'_>) -> Vec<KeptAlias> {
  let mut kept = Vec::new();
  for file in &plan.files {
    let Some(text) = originals.get(&file.file) else { continue };
    for edit in &file.edits {
      let keeps_old = edit.new_text != expected.new_name
        && edit
          .new_text
          .match_indices(expected.old_name)
          .any(|(offset, _)| reads_token(&edit.new_text, offset, expected.old_name));
      if !keeps_old {
        continue;
      }
      let line = text[..edit.start].matches('\n').count() + 1;
      let column = edit.start - text[..edit.start].rfind('\n').map_or(0, |at| at + 1) + 1;
      kept.push(KeptAlias {
        text: edit.new_text.clone(),
        file: file.file.clone(),
        label: format!("{}:{line}:{column} ({:?})", file.file, edit.new_text),
      });
    }
  }
  kept
}

fn module_specifiers(text: &str, name: &str) -> Vec<String> {
  let Ok(pattern) = regex::Regex::new(&format!(
    r#"(?s)\b(?:import|export)\b(?:[^;'"`]|'[^']*'|"[^"]*")*?\b{}\b(?:[^;'"`]|'[^']*'|"[^"]*")*?\bfrom\s*['"]([^'"]+)['"]"#,
    regex::escape(name)
  )) else {
    return Vec::new();
  };
  pattern
    .captures_iter(text)
    .filter_map(|captures| captures.get(1))
    .map(|found| found.as_str().to_owned())
    .collect()
}

fn resolves_to(from: &Path, specifier: &str, candidates: &[PathBuf]) -> bool {
  let Some(directory) = from.parent() else { return false };
  let base = directory.join(specifier);
  let stem = base.to_string_lossy().trim_end_matches(".js").to_owned();
  let tries = [
    base.clone(),
    PathBuf::from(format!("{stem}.ts")),
    PathBuf::from(format!("{stem}.tsx")),
    PathBuf::from(format!("{stem}.d.ts")),
    PathBuf::from(format!("{stem}.mts")),
    base.join("index.ts"),
    base.join("index.tsx"),
  ];
  tries.iter().filter_map(|path| canonical_path(path).ok()).any(|path| candidates.contains(&path))
}

fn reaches_through(files: &crate::target::ProjectFiles, originals: &BTreeMap<FileTarget, String>, site: &VerifySite, old_name: &str, aliases: &[&FileTarget]) -> bool {
  if aliases.is_empty() {
    return false;
  }
  let Some(root) = atlas_core::RepoName::new(&site.repo).ok().and_then(|repo| files.root(&repo).map(Path::to_path_buf)) else {
    return true;
  };
  let path = root.join(&site.path);
  if kind_of(&path) != Some(ServerKind::TypeScript) {
    return true;
  }
  let text = originals
    .iter()
    .find(|(file, _)| file.repo.as_str() == site.repo && file.path.as_str() == site.path)
    .map(|(_, text)| text.clone())
    .or_else(|| std::fs::read_to_string(&path).ok());
  let Some(text) = text else { return true };
  let alias_paths: Vec<PathBuf> = aliases
    .iter()
    .filter_map(|file| files.root(&file.repo).and_then(|root| canonical_path(&root.join(file.path.as_str())).ok()))
    .collect();
  module_specifiers(&text, old_name)
    .iter()
    .any(|specifier| !specifier.starts_with('.') || resolves_to(&path, specifier, &alias_paths))
}

fn shadowed_binding(plan: &EditPlan, originals: &BTreeMap<FileTarget, String>, new_name: &str, kind: ServerKind) -> Option<String> {
  let bare = new_name.strip_prefix("r#").unwrap_or(new_name);
  let name = regex::escape(bare);
  let patterns = match kind {
    ServerKind::TypeScript => vec![
      format!(r"\bimport\s+(?:type\s+)?\{{[^}}]*\b{name}\b[^}}]*\}}"),
      format!(r"\bimport\s+(?:type\s+)?{name}\b"),
      format!(r"\bimport\s+\*\s+as\s+{name}\b"),
      format!(r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?(?:const|let|var|function\*?|class|interface|type|enum|namespace)\s+{name}\b"),
      format!(r"\b(?:const|let|var)\s*[{{\[][^}}\]]*\b{name}\b[^}}\]]*[}}\]]\s*="),
    ],
    ServerKind::RustAnalyzer => vec![
      format!(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:fn|struct|enum|trait|type|const|static|mod|union|macro_rules!)\s+{name}\b"),
      format!(r"\blet\s+(?:mut\s+)?{name}\b"),
      format!(r"\buse\b[^;]*\b{name}\b[^;]*;"),
    ],
  };
  let compiled: Vec<regex::Regex> = patterns.iter().filter_map(|pattern| regex::Regex::new(pattern).ok()).collect();
  let Ok(word) = regex::Regex::new(&format!(r"\b{name}\b")) else { return None };
  for file in &plan.files {
    let Some(text) = originals.get(&file.file) else { continue };
    let code = mask_code(text, kind);
    let edited: Vec<usize> = file.edits.iter().map(|edit| edit.start).collect();
    let mut bound: Vec<usize> = compiled
      .iter()
      .flat_map(|pattern| {
        pattern
          .find_iter(&code)
          .filter_map(|found| word.find(found.as_str()).map(|inner| found.start() + inner.start()))
          .collect::<Vec<_>>()
      })
      .filter(|at| {
        let (start, end) = enclosing_block(&code, *at);
        edited.iter().any(|edit| start <= *edit && *edit <= end)
      })
      .collect();
    bound.sort_unstable();
    if let Some(at) = bound.first() {
      let line = text[..*at].matches('\n').count() + 1;
      let line_start = text[..*at].rfind('\n').map_or(0, |found| found + 1);
      return Some(format!(
        "{bare} is already bound in {}:{line} ({}), in a scope the rename edits; the rename would clash with or shadow it; nothing was written",
        file.file,
        text[line_start..].lines().next().unwrap_or_default().trim()
      ));
    }
  }
  None
}

fn enclosing_block(code: &str, at: usize) -> (usize, usize) {
  let bytes = code.as_bytes();
  let mut depth = 0usize;
  let mut start = None;
  for index in (0..at.min(bytes.len())).rev() {
    match bytes[index] {
      b'}' => depth += 1,
      b'{' if depth == 0 => {
        start = Some(index);
        break;
      }
      b'{' => depth -= 1,
      _ => {}
    }
  }
  let Some(start) = start else { return (0, code.len()) };
  let mut depth = 0usize;
  for (index, byte) in bytes.iter().enumerate().skip(start) {
    match byte {
      b'{' => depth += 1,
      b'}' => {
        depth -= 1;
        if depth == 0 {
          return (start, index);
        }
      }
      _ => {}
    }
  }
  (start, code.len())
}

fn mask_code(text: &str, kind: ServerKind) -> String {
  let bytes = text.as_bytes();
  let mut masked = bytes.to_vec();
  let blank = |from: usize, to: usize, masked: &mut Vec<u8>| {
    for byte in masked.iter_mut().take(to.min(bytes.len())).skip(from) {
      if *byte != b'\n' {
        *byte = b' ';
      }
    }
  };
  let mut index = 0;
  while index < bytes.len() {
    let rest = &bytes[index..];
    let end = if rest.starts_with(b"//") {
      rest.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |at| index + at)
    } else if rest.starts_with(b"/*") {
      text[index + 2..].find("*/").map_or(bytes.len(), |at| index + 2 + at + 2)
    } else if kind == ServerKind::RustAnalyzer && (rest.starts_with(b"r#") || rest.starts_with(b"r\"")) && !preceded_by_word(bytes, index) {
      let hashes = rest[1..].iter().take_while(|byte| **byte == b'#').count();
      if rest.get(1 + hashes) == Some(&b'"') {
        let closing = format!("\"{}", "#".repeat(hashes));
        text[index + 2 + hashes..].find(&closing).map_or(bytes.len(), |at| index + 2 + hashes + at + closing.len())
      } else {
        index + 1
      }
    } else if rest[0] == b'"' || (kind == ServerKind::TypeScript && (rest[0] == b'\'' || rest[0] == b'`')) {
      quoted_end(bytes, index)
    } else if kind == ServerKind::RustAnalyzer && rest[0] == b'\'' {
      char_literal_end(bytes, index).unwrap_or(index + 1)
    } else {
      index + 1
    };
    if end > index + 1 || rest.starts_with(b"//") {
      blank(index, end, &mut masked);
    }
    index = end.max(index + 1);
  }
  String::from_utf8(masked).unwrap_or_else(|_| text.to_owned())
}

fn preceded_by_word(bytes: &[u8], index: usize) -> bool {
  index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_')
}

fn quoted_end(bytes: &[u8], start: usize) -> usize {
  let quote = bytes[start];
  let mut index = start + 1;
  while index < bytes.len() {
    match bytes[index] {
      b'\\' => index += 2,
      byte if byte == quote => return index + 1,
      b'\n' if quote != b'`' && quote != b'"' => return index,
      _ => index += 1,
    }
  }
  bytes.len()
}

fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
  let body = bytes.get(start + 1..)?;
  if body.first() == Some(&b'\\') {
    return body.iter().skip(1).position(|byte| *byte == b'\'').map(|at| start + 1 + 1 + at + 1);
  }
  let width = std::str::from_utf8(&body[..body.len().min(4)])
    .ok()
    .and_then(|text| text.chars().next())
    .map_or(1, char::len_utf8);
  (body.get(width) == Some(&b'\'')).then_some(start + 1 + width + 1)
}

fn covers_anchor(plan: &EditPlan, anchor: &Anchor) -> bool {
  plan
    .files
    .iter()
    .any(|file| file.file.repo == anchor.file.repo && file.file.path == anchor.file.path && file.edits.iter().any(|edit| edit.start <= anchor.offset && anchor.offset <= edit.end))
}

fn normalize(path: &Path) -> PathBuf {
  if let Ok(canonical) = canonical_path(path) {
    return canonical;
  }
  match (path.parent().and_then(|parent| canonical_path(parent).ok()), path.file_name()) {
    (Some(parent), Some(name)) => parent.join(name),
    _ => path.to_path_buf(),
  }
}

fn acceptable(original: &str, text: &str, start: usize, old_name: &str) -> bool {
  let raw = format!("r#{old_name}");
  if original == old_name || original == raw {
    return true;
  }
  original.is_empty() && (reads_token(text, start, old_name) || reads_token(text, start, &raw) || text[..start].ends_with(old_name))
}

struct Pending {
  versions: BTreeSet<Option<i32>>,
  edits: Vec<atlas_lsp::protocol::TextEdit>,
}

pub(crate) fn convert(session: &Session<'_>, scope: &EditScope, edit: WorkspaceEdit, expected: &Expectation<'_>) -> IdeResult<Conversion> {
  if edit.requires_confirmation() {
    let labels: Vec<String> = edit.change_annotations.values().map(|annotation| annotation.label.clone()).collect();
    return Ok(Conversion::Refused(format!(
      "the server marked this rename as needing confirmation ({}); code-atlas never applies such edits automatically; nothing was written",
      labels.join(", ")
    )));
  }
  let mut pending: BTreeMap<PathBuf, Pending> = BTreeMap::new();
  let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
  let mut created = Vec::new();
  let mut deleted = Vec::new();
  let mut moved_to: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
  let mut moved_away: BTreeSet<PathBuf> = BTreeSet::new();
  for change in edit.into_document_changes() {
    match change {
      DocumentChange::Edit(document) => {
        let path = normalize(&path_from_uri(&document.text_document.uri).map_err(LspError::from)?);
        if moved_away.contains(&path) {
          return Ok(Conversion::Refused(format!(
            "the server edits {} after renaming it away; nothing was written",
            path.display()
          )));
        }
        let source = moved_to.get(&path).cloned().unwrap_or(path);
        let entry = pending.entry(source).or_insert_with(|| Pending {
          versions: BTreeSet::new(),
          edits: Vec::new(),
        });
        entry.versions.insert(document.text_document.version);
        entry.edits.extend(document.edits);
      }
      DocumentChange::Rename(rename) => {
        if rename.options.overwrite {
          return Ok(Conversion::Refused(format!("the server's rename would overwrite {}; nothing was written", rename.new_uri)));
        }
        let from = normalize(&path_from_uri(&rename.old_uri).map_err(LspError::from)?);
        let to = normalize(&path_from_uri(&rename.new_uri).map_err(LspError::from)?);
        moved_to.insert(to.clone(), from.clone());
        moved_away.insert(from.clone());
        renames.push((from, to));
      }
      DocumentChange::Create(create) => created.push(normalize(&path_from_uri(&create.uri).map_err(LspError::from)?)),
      DocumentChange::Delete(delete) => deleted.push(normalize(&path_from_uri(&delete.uri).map_err(LspError::from)?)),
    }
  }
  if !created.is_empty() || !deleted.is_empty() {
    let shown: Vec<String> = created
      .iter()
      .map(|path| format!("create {}", path.display()))
      .chain(deleted.iter().map(|path| format!("delete {}", path.display())))
      .collect();
    return Ok(Conversion::Refused(format!(
      "the server's rename would also {}; code-atlas applies renames that edit text and rename files only; nothing was written",
      shown.join(", ")
    )));
  }
  let mut every: Vec<&Path> = pending.keys().map(PathBuf::as_path).collect();
  every.extend(renames.iter().flat_map(|(from, to)| [from.as_path(), to.as_path()]));
  let mut scoped: BTreeMap<PathBuf, ScopedPath> = BTreeMap::new();
  let mut refused = Vec::new();
  for path in every {
    match scope.resolve(path) {
      Ok(found) => {
        scoped.insert(path.to_path_buf(), found);
      }
      Err(refusal) => refused.push(refusal.to_string()),
    }
  }
  if !refused.is_empty() {
    refused.sort();
    refused.dedup();
    return Ok(Conversion::Refused(format!(
      "the rename touches files outside the editable scope: {}; nothing was written",
      refused.join(", ")
    )));
  }
  let mut plan = EditPlan::default();
  let mut originals = BTreeMap::new();
  let mut stale = Vec::new();
  let mut stale_paths = Vec::new();
  for (path, pending) in pending {
    let Some(target) = scoped.get(&path) else { continue };
    match file_edits(session, target, &pending, expected)? {
      FileOutcome::Ready(edits, text) => {
        originals.insert(target.file.clone(), text);
        plan.files.push(edits);
      }
      FileOutcome::Stale(reason) => {
        stale.push(reason);
        stale_paths.push(path);
      }
      FileOutcome::Refused(reason) => return Ok(Conversion::Refused(reason)),
    }
  }
  for (from, to) in &renames {
    let (Some(source), Some(destination)) = (scoped.get(from), scoped.get(to)) else {
      continue;
    };
    match std::fs::read(&source.absolute) {
      Ok(bytes) => plan.resources.push(ResourceOp::Rename {
        from: source.file.clone(),
        to: destination.file.clone(),
        expected: ContentHash::of(&bytes),
      }),
      Err(error) => {
        stale.push(format!("{} cannot be read ({error})", source.file));
        stale_paths.push(from.clone());
      }
    }
  }
  if !stale.is_empty() {
    return Ok(Conversion::Stale {
      reason: stale.join("; "),
      paths: stale_paths,
    });
  }
  if plan.is_empty() {
    return Ok(Conversion::Refused(format!("{} returned an empty rename; nothing was written", session.server_name())));
  }
  if let Err(violation) = plan.validate() {
    return Ok(Conversion::Refused(format!("the server's rename is not a valid edit ({violation}); nothing was written")));
  }
  Ok(Conversion::Ready(Converted { plan, originals }))
}

enum FileOutcome {
  Ready(FileEdits, String),
  Stale(String),
  Refused(String),
}

fn file_edits(session: &Session<'_>, target: &ScopedPath, pending: &Pending, expected: &Expectation<'_>) -> IdeResult<FileOutcome> {
  let file = &target.file;
  let bytes = match std::fs::read(&target.absolute) {
    Ok(bytes) => bytes,
    Err(error) => return Ok(FileOutcome::Stale(format!("{file} cannot be read ({error})"))),
  };
  let hash = ContentHash::of(&bytes);
  let Ok(text) = String::from_utf8(bytes) else {
    return Ok(FileOutcome::Refused(format!("{file} is not UTF-8; nothing was written")));
  };
  let open = session.lease.open_document(&target.absolute);
  let versioned: Vec<i32> = pending.versions.iter().flatten().copied().collect();
  match &open {
    Some(document) if document.hash != hash => return Ok(FileOutcome::Stale(format!("{file} changed after it was sent to the language server"))),
    Some(document) if versioned.iter().any(|version| *version != document.version) => {
      return Ok(FileOutcome::Stale(format!(
        "the server edits {file} at version {versioned:?}, but the open document is at version {}",
        document.version
      )));
    }
    None if !versioned.is_empty() => {
      return Ok(FileOutcome::Stale(format!(
        "the server edits {file} at version {versioned:?}, but code-atlas never opened it"
      )));
    }
    None => {
      let indexed = session.query.engine().store().read(|tx| tx.file(&file.repo, &file.path))?.map(|record| record.content_hash);
      if indexed.is_some_and(|indexed| indexed != hash) {
        return Ok(FileOutcome::Stale(format!("{file} changed since it was indexed")));
      }
    }
    Some(_) => {}
  }
  let index = LineIndex::new(&text, session.lease.line_breaks());
  let mut edits = Vec::with_capacity(pending.edits.len());
  for edit in &pending.edits {
    let Ok(range) = index.offsets_strict(edit.range, session.lease.encoding()) else {
      return Ok(FileOutcome::Stale(format!(
        "the server's position {}:{} does not fit {file}",
        edit.range.start.line + 1,
        edit.range.start.character + 1
      )));
    };
    let at = index
      .graph_span(range.start..range.start)
      .map_or_else(|_| format!("{file}@{}", range.start), |span| format!("{file}:{}:{}", span.start_line, span.start_col + 1));
    if edit.new_text.contains(['\n', '\r']) {
      return Ok(FileOutcome::Refused(format!("the server's rename writes a line break at {at}; nothing was written")));
    }
    if !edit.new_text.contains(expected.new_name) {
      return Ok(FileOutcome::Refused(format!(
        "the server's rename writes {:?} at {at}, which does not contain {}; nothing was written",
        edit.new_text, expected.new_name
      )));
    }
    let original = &text[range.clone()];
    if !acceptable(original, &text, range.start, expected.old_name) {
      return Ok(FileOutcome::Stale(format!("at {at} the server would replace {original:?}, not {}", expected.old_name)));
    }
    edits.push(TextEdit::replace(range, edit.new_text.clone()));
  }
  edits.sort_by(|left, right| (left.start, left.end, &left.new_text).cmp(&(right.start, right.end, &right.new_text)));
  edits.dedup();
  let edits = FileEdits::new(file.clone(), hash, edits);
  if let Err(violation) = edits.check_against(&text) {
    return Ok(FileOutcome::Refused(format!("the server's rename is not a valid edit ({violation}); nothing was written")));
  }
  Ok(FileOutcome::Ready(edits, text))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_accept_only_the_old_name_its_raw_form_or_an_adjacent_insertion() {
    let text = "let findOne = 1; r#type; x";

    assert!(acceptable("findOne", text, 4, "findOne"));
    assert!(acceptable("r#type", text, 17, "type"));
    assert!(acceptable("", text, 4, "findOne"));
    assert!(acceptable("", text, 11, "findOne"));
    assert!(!acceptable("", text, 25, "findOne"));
    assert!(!acceptable("findOn", text, 4, "findOne"));
    assert!(!acceptable("let", text, 0, "findOne"));
  }

  #[test]
  fn should_blank_comments_and_strings_but_keep_offsets_and_lifetimes() {
    let ts = "const a = 'x // y' // note b\n/* c */ let d = `e`;\n";
    let rust = "fn f<'a>(x: &'a str) -> char { let c = 'z'; r#\"q\"#; x.len(); c }\n";

    assert_eq!(mask_code(ts, ServerKind::TypeScript), format!("const a ={}\n        let d =    ;\n", " ".repeat(19)));
    assert_eq!(
      mask_code(rust, ServerKind::RustAnalyzer),
      "fn f<'a>(x: &'a str) -> char { let c =    ;       ; x.len(); c }\n"
    );
    assert_eq!(enclosing_block("a { b { c } d } e", 12), (2, 14));
    assert_eq!(enclosing_block("a { b } c", 8), (0, 9));
  }

  #[test]
  fn should_validate_identifiers_per_language() {
    assert!(validate_name("getById", ServerKind::TypeScript).is_ok());
    assert!(validate_name("$élan_2", ServerKind::TypeScript).is_ok());
    assert!(validate_name("get by id", ServerKind::TypeScript).is_err());
    assert!(validate_name("2x", ServerKind::TypeScript).is_err());
    assert!(validate_name("r#type", ServerKind::RustAnalyzer).is_ok());
    assert!(validate_name("fetch_one", ServerKind::RustAnalyzer).is_ok());
    assert!(validate_name("$x", ServerKind::RustAnalyzer).is_err());
    assert!(validate_name("_", ServerKind::RustAnalyzer).is_err());
    assert!(validate_name("", ServerKind::RustAnalyzer).is_err());
  }
}
