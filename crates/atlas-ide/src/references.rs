use atlas_core::{EdgeKind, Entity, RepoName};
use atlas_lsp::protocol::{GotoResponse, Position, ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams};
use atlas_lsp::{LineIndex, SyncDemand, uri_from_path};
use atlas_query::{CertaintyTag, EntityRef, GraphReference, GraphReferences, Location, NameSpan, ReferenceSide, Response, Targeted};
use atlas_store::{EdgeFilter, InferredPolicy};
use serde::Serialize;

use crate::edit::Reach;
use crate::error::{IdeError, IdeResult};
use crate::map::{Enclosing, Site, SiteMapper, SitePlace, line_text};
use crate::outline::ReadSource;
use crate::render::ServerView;
use crate::service::{IdeService, Session};
use crate::target::{Anchor, Resolver, Target, read_text};

const ENDPOINT_EVIDENCE: &str = "endpoint";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencesRequest {
  pub target: String,
  pub source: ReadSource,
}

impl ReferencesRequest {
  pub fn new(target: impl Into<String>) -> Self {
    Self {
      target: target.into(),
      source: ReadSource::Lsp,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetView {
  Symbol { symbol: EntityRef, name: SitePlace },
  Position { position: SitePlace },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolGroup {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub enclosing: Option<Enclosing>,
  pub sites: Vec<Site>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoGroup {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub repo: Option<String>,
  pub total: usize,
  pub symbols: Vec<SymbolGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ExtraReason {
  OtherRepo,
  NotInServerProject,
  Mock { rule: String },
  Http { endpoint: String },
  Unresolved { detail: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphExtra {
  #[serde(flatten)]
  pub reason: ExtraReason,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub location: Option<Location>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub from: Option<EntityRef>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub edge_kind: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub certainty: Option<CertaintyTag>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExactReferences {
  pub target: TargetView,
  pub source: ReadSource,
  pub server: ServerView,
  pub total: usize,
  pub groups: Vec<RepoGroup>,
  pub outside_server_view: Vec<GraphExtra>,
  pub caveats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum References {
  Exact(Box<Targeted<ExactReferences>>),
  Graph(Box<Targeted<GraphReferences>>),
}

impl IdeService {
  pub fn references(&self, request: &ReferencesRequest) -> IdeResult<Response<References>> {
    if request.source == ReadSource::Graph {
      let response = self.query.symbol_references(&atlas_query::ReferencesRequest::new(&request.target))?;
      return Ok(Response {
        freshness: response.freshness,
        answer: References::Graph(Box::new(response.answer)),
      });
    }
    self.ensure_enabled().map_err(IdeError::with_graph_alternative)?;
    let files = self.files();
    let mut freshness = self.query.fresh()?;
    let resolver = Resolver {
      query: &self.query,
      files: &files,
    };
    let target = match resolver.resolve(&request.target, &mut freshness)? {
      Targeted::Found(target) => target,
      Targeted::NotFound { spec, hint } => return Ok(not_targeted(freshness, Targeted::NotFound { spec, hint })),
      Targeted::Ambiguous { spec, candidates } => return Ok(not_targeted(freshness, Targeted::Ambiguous { spec, candidates })),
    };
    let anchor = resolver.anchor(&target)?;
    let mut caveats = Vec::new();
    let graph_target = self.graph_target(&target, &mut caveats)?;
    let sites = match &graph_target {
      Some(entity) => self.graph_sites(entity, Reach::TargetOnly)?.sites,
      None => Vec::new(),
    };
    let candidates = self.exact_candidates(&files, &sites, &anchor.file, |_| true)?;
    caveats.extend(candidates.caveats);
    let open = candidates.open;
    let (server, sites) = self
      .exact(&freshness, &files, &anchor.file, SyncDemand::Best, &open, |session| {
        if let Err(error) = session.settle_projects() {
          session.notes.partial.push(format!("the language server was still loading projects ({error})"));
        }
        if !session.capabilities().references_provider.is_supported() {
          return Err(unsupported(session, "textDocument/references"));
        }
        let params = ReferenceParams {
          position: position_params(session, &anchor)?,
          context: ReferenceContext { include_declaration: false },
        };
        let locations: GotoResponse = session.request("textDocument/references", params)?;
        let sites = SiteMapper::new(session).map_all(locations.into_locations())?;
        Ok(match &target {
          Target::Symbol { .. } => without_anchor(sites, &anchor),
          Target::Position { .. } => sites,
        })
      })
      .map_err(IdeError::with_graph_alternative)?;
    let mut outside_server_view = Vec::new();
    if let Some(entity) = &graph_target {
      let graph = self.query.graph_references(entity)?;
      let outside = outside_view(&anchor.file.repo, &graph, &sites);
      if outside.heuristic_hidden > 0 {
        caveats.push(format!(
          "{} heuristic graph site(s) in {} that the language server did not return are not shown",
          outside.heuristic_hidden, anchor.file.repo
        ));
      }
      outside_server_view = outside.extras;
      outside_server_view.extend(self.http_consumers(entity)?);
      if !graph.unverified_files.is_empty() {
        caveats.push(format!(
          "{} graph site(s) are in files that changed since indexing and were not checked",
          graph.unverified_files.len()
        ));
      }
    }
    let answer = ExactReferences {
      target: target_view(&target, &anchor),
      source: ReadSource::Lsp,
      server,
      total: sites.len(),
      groups: group(sites),
      outside_server_view,
      caveats,
    };
    Ok(Response {
      freshness,
      answer: References::Exact(Box::new(Targeted::Found(answer))),
    })
  }

  pub(crate) fn graph_target(&self, target: &Target, caveats: &mut Vec<String>) -> IdeResult<Option<Entity>> {
    let (file, line, column) = match target {
      Target::Symbol { entity, .. } => return Ok(Some(entity.as_ref().clone())),
      Target::Position { file, line, column } => (file, *line, *column),
    };
    let on_name = match self.query.innermost_symbol(&file.repo, &file.path, line, column)? {
      Some(entity) => match self.query.name_span(&entity)? {
        NameSpan::Found { span } if span.start_line == line && span.start_col + 1 == column => Some(entity),
        NameSpan::Found { .. } | NameSpan::SourceChanged | NameSpan::Unavailable { .. } => None,
      },
      None => None,
    };
    if on_name.is_none() {
      caveats.push("sites outside the language server's view are only listed for a symbol target (the position is not on an indexed symbol's name)".to_owned());
    }
    Ok(on_name)
  }

  pub(crate) fn http_consumers(&self, target: &Entity) -> IdeResult<Vec<GraphExtra>> {
    let exposes = EdgeFilter {
      kinds: vec![EdgeKind::ExposesEndpoint],
      ..EdgeFilter::default()
    };
    let calls = EdgeFilter {
      kinds: vec![EdgeKind::CallsEndpoint],
      inferred: InferredPolicy::Exclude,
      ..EdgeFilter::default()
    };
    let files = self.files();
    let found = self.query.engine().store().read(|tx| -> IdeResult<Vec<GraphExtra>> {
      let mut extras = Vec::new();
      for exposed in tx.edges_from(target.id, &exposes)? {
        let Some(endpoint) = tx.entity(exposed.to)? else { continue };
        for call in tx.edges_to(endpoint.id, &calls)? {
          let from = tx.entity(call.from)?;
          let sites = call.evidence.iter().filter(|evidence| evidence.note.as_deref() != Some(ENDPOINT_EVIDENCE));
          for evidence in sites {
            let snippet = files.root(&evidence.repo).and_then(|root| {
              let (text, hash) = read_text(&root.join(evidence.path.as_str())).ok()?;
              (hash == evidence.content_hash).then(|| line_text(&text, evidence.span.start_byte as usize)).flatten()
            });
            extras.push(GraphExtra {
              reason: ExtraReason::Http {
                endpoint: endpoint.key.display_name(),
              },
              location: Some(Location {
                repo: evidence.repo.to_string(),
                path: evidence.path.to_string(),
                line: Some(evidence.span.start_line),
                column: None,
              }),
              from: from.as_ref().map(EntityRef::of),
              edge_kind: Some(call.kind.as_str().to_owned()),
              certainty: Some(CertaintyTag::of(&call.certainty)),
              snippet,
            });
          }
        }
      }
      Ok(extras)
    })?;
    let mut found = found;
    found.sort_by(|left, right| left.location.cmp(&right.location));
    Ok(found)
  }
}

fn not_targeted(freshness: atlas_query::Freshness, answer: Targeted<ExactReferences>) -> Response<References> {
  Response {
    freshness,
    answer: References::Exact(Box::new(answer)),
  }
}

pub(crate) fn unsupported(session: &Session<'_>, capability: &str) -> IdeError {
  IdeError::Unsupported {
    server: session.server_name().to_owned(),
    capability: capability.to_owned(),
  }
}

pub(crate) fn position_params(session: &Session<'_>, anchor: &Anchor) -> IdeResult<TextDocumentPositionParams> {
  Ok(TextDocumentPositionParams {
    text_document: TextDocumentIdentifier {
      uri: uri_from_path(&anchor.file.absolute).map_err(atlas_lsp::LspError::from)?,
    },
    position: anchor_position(session, anchor)?,
  })
}

pub(crate) fn anchor_position(session: &Session<'_>, anchor: &Anchor) -> IdeResult<Position> {
  let Some(open) = session.lease.open_document(&anchor.file.absolute) else {
    return Err(IdeError::NotOpened {
      file: anchor.file.to_string(),
      server: session.server_name().to_owned(),
      reason: "it is not open after syncing (too large or unreadable)".to_owned(),
    });
  };
  if open.hash != anchor.hash {
    return Err(IdeError::SourceChanged { file: anchor.file.to_string() });
  }
  let index = LineIndex::new(&anchor.text, session.lease.line_breaks());
  Ok(index.position(anchor.offset, session.lease.encoding()).map_err(atlas_lsp::LspError::from)?)
}

pub(crate) fn anchor_place(anchor: &Anchor) -> SitePlace {
  let before = &anchor.text[..anchor.offset];
  let line_start = before.rfind('\n').map_or(0, |at| at + 1);
  SitePlace {
    repo: Some(anchor.file.repo.to_string()),
    path: anchor.file.path.to_string(),
    line: u32::try_from(before.matches('\n').count() + 1).unwrap_or(u32::MAX),
    column: u32::try_from(anchor.offset - line_start + 1).ok(),
  }
}

fn without_anchor(sites: Vec<Site>, anchor: &Anchor) -> Vec<Site> {
  let place = anchor_place(anchor);
  sites.into_iter().filter(|site| site.place != place).collect()
}

pub(crate) fn target_view(target: &Target, anchor: &Anchor) -> TargetView {
  match target {
    Target::Symbol { entity, .. } => TargetView::Symbol {
      symbol: EntityRef::of(entity),
      name: anchor_place(anchor),
    },
    Target::Position { .. } => TargetView::Position { position: anchor_place(anchor) },
  }
}

fn enclosing_key(enclosing: &Option<Enclosing>) -> String {
  match enclosing {
    Some(Enclosing::Graph { symbol }) => symbol.key.clone(),
    Some(Enclosing::Lsp { name, kind }) => format!("{kind} {name}"),
    None => String::new(),
  }
}

pub(crate) fn group(sites: Vec<Site>) -> Vec<RepoGroup> {
  let mut groups: Vec<RepoGroup> = Vec::new();
  for site in sites {
    let repo = site.place.repo.clone();
    if groups.last().is_none_or(|last| last.repo != repo) {
      groups.push(RepoGroup {
        repo: repo.clone(),
        total: 0,
        symbols: Vec::new(),
      });
    }
    let Some(current) = groups.last_mut() else { continue };
    current.total += 1;
    let key = enclosing_key(&site.enclosing);
    match current.symbols.iter_mut().find(|symbol| enclosing_key(&symbol.enclosing) == key) {
      Some(symbol) => symbol.sites.push(site),
      None => current.symbols.push(SymbolGroup {
        enclosing: site.enclosing.clone(),
        sites: vec![site],
      }),
    }
  }
  groups
}

fn matched_by(location: &Location, sites: &[Site]) -> bool {
  sites.iter().any(|site| {
    site.place.repo.as_deref() == Some(location.repo.as_str())
      && site.place.path == location.path
      && Some(site.place.line) == location.line
      && (location.column.is_none() || site.place.column.is_none() || site.place.column == location.column)
  })
}

pub(crate) struct OutsideView {
  pub extras: Vec<GraphExtra>,
  pub heuristic_hidden: usize,
}

pub(crate) fn outside_view(target_repo: &RepoName, graph: &GraphReferences, sites: &[Site]) -> OutsideView {
  let unmatched = graph
    .references
    .iter()
    .filter(|row| row.side == ReferenceSide::Inbound)
    .filter(|row| row.location.as_ref().is_none_or(|location| !matched_by(location, sites)));
  let mut extras = Vec::new();
  let mut heuristic_hidden = 0;
  for row in unmatched {
    match extra_for(target_repo, row) {
      Some(extra) => extras.push(extra),
      None => heuristic_hidden += 1,
    }
  }
  extras.extend(graph.unresolved.iter().filter(|site| !matched_by(&site.location, sites)).map(|site| GraphExtra {
    reason: ExtraReason::Unresolved {
      detail: format!("{} ({})", site.path, site.reason),
    },
    location: Some(site.location.clone()),
    from: None,
    edge_kind: None,
    certainty: None,
    snippet: None,
  }));
  OutsideView { extras, heuristic_hidden }
}

fn extra_for(target_repo: &RepoName, row: &GraphReference) -> Option<GraphExtra> {
  let site_repo = row.location.as_ref().map_or_else(|| row.other.repo().to_owned(), |location| location.repo.clone());
  let reason = match &row.mock {
    Some(rule) => ExtraReason::Mock { rule: rule.clone() },
    None if site_repo != target_repo.as_str() => ExtraReason::OtherRepo,
    None if row.certainty.authoritative => ExtraReason::NotInServerProject,
    None => return None,
  };
  Some(GraphExtra {
    reason,
    location: row.location.clone(),
    from: Some(row.other.clone()),
    edge_kind: Some(row.edge_kind.clone()),
    certainty: Some(row.certainty.clone()),
    snippet: row.snippet.clone(),
  })
}

#[cfg(test)]
mod tests {
  use atlas_query::SymbolSource;

  use super::*;

  fn entity(key: &str, repo: &str) -> EntityRef {
    EntityRef {
      key: key.to_owned(),
      id: "1".to_owned(),
      short: key.to_owned(),
      kind: "function".to_owned(),
      location: Some(Location {
        repo: repo.to_owned(),
        path: "src/a.ts".to_owned(),
        line: Some(1),
        column: None,
      }),
      certainty: None,
    }
  }

  fn tag(text: &str, authoritative: bool) -> CertaintyTag {
    CertaintyTag {
      class: "resolved".to_owned(),
      confidence: None,
      freshness: None,
      authoritative,
      tag: text.to_owned(),
    }
  }

  fn row(repo: &str, line: u32, column: Option<u32>, side: ReferenceSide, mock: Option<&str>) -> GraphReference {
    GraphReference {
      location: Some(Location {
        repo: repo.to_owned(),
        path: "src/a.ts".to_owned(),
        line: Some(line),
        column,
      }),
      side,
      edge_kind: "calls".to_owned(),
      edge_ids: vec!["e".to_owned()],
      phrase: "called by".to_owned(),
      other: entity(&format!("{repo}-caller-{line}"), repo),
      enclosing: None,
      certainty: tag(if mock.is_some() { "[heuristic 0.60]" } else { "[resolved]" }, mock.is_none()),
      overload: None,
      mock: mock.map(str::to_owned),
      snippet: Some(format!("line {line}")),
    }
  }

  fn heuristic(row: GraphReference) -> GraphReference {
    GraphReference {
      certainty: tag("[heuristic 0.70]", false),
      ..row
    }
  }

  fn site(repo: &str, line: u32, column: u32) -> Site {
    Site {
      place: SitePlace {
        repo: Some(repo.to_owned()),
        path: "src/a.ts".to_owned(),
        line,
        column: Some(column),
      },
      enclosing: None,
      snippet: None,
      unverified: None,
    }
  }

  #[test]
  fn should_list_unreturned_authoritative_other_repo_and_mock_sites_outside_the_server_view() {
    let graph = GraphReferences {
      target: entity("target", "api"),
      overloads: Vec::new(),
      source: SymbolSource::Graph,
      references: vec![
        row("api", 3, Some(5), ReferenceSide::Inbound, None),
        row("api", 4, Some(5), ReferenceSide::Inbound, None),
        heuristic(row("api", 5, Some(5), ReferenceSide::Inbound, None)),
        row("api", 7, Some(9), ReferenceSide::Inbound, Some("jest-mock-member")),
        row("api", 8, Some(9), ReferenceSide::Inbound, Some("jest-mock-member")),
        row("web", 10, Some(2), ReferenceSide::Inbound, None),
        row("web", 11, None, ReferenceSide::Inbound, None),
        row("web", 12, Some(2), ReferenceSide::Inbound, None),
        row("web", 20, Some(1), ReferenceSide::Heritage, None),
      ],
      unresolved: vec![atlas_query::UnresolvedSite {
        location: Location {
          repo: "api".to_owned(),
          path: "src/a.ts".to_owned(),
          line: Some(30),
          column: Some(4),
        },
        path: "this.[name]".to_owned(),
        reason: "dynamic".to_owned(),
      }],
      unverified_files: Vec::new(),
      caveats: Vec::new(),
    };
    let sites = vec![site("api", 3, 5), site("api", 8, 9), site("web", 11, 7), site("web", 12, 2)];

    let outside = outside_view(&RepoName::new("api").unwrap(), &graph, &sites);
    let extras = outside.extras;

    let summary: Vec<String> = extras
      .iter()
      .map(|extra| format!("{:?} {}", extra.reason, extra.location.as_ref().map_or_else(String::new, ToString::to_string)))
      .collect();
    assert_eq!(
      summary,
      vec![
        "NotInServerProject api:src/a.ts:4:5".to_owned(),
        "Mock { rule: \"jest-mock-member\" } api:src/a.ts:7:9".to_owned(),
        "OtherRepo web:src/a.ts:10:2".to_owned(),
        "Unresolved { detail: \"this.[name] (dynamic)\" } api:src/a.ts:30:4".to_owned(),
      ]
    );
    assert_eq!(extras[1].certainty.as_ref().map(|certainty| certainty.tag.as_str()), Some("[heuristic 0.60]"));
    assert_eq!(outside.heuristic_hidden, 1);
  }

  #[test]
  fn should_group_sites_by_repo_and_enclosing_symbol_in_order() {
    let with = |repo: &str, line: u32, name: &str| Site {
      enclosing: Some(Enclosing::Lsp {
        name: name.to_owned(),
        kind: "function".to_owned(),
      }),
      ..site(repo, line, 1)
    };

    let groups = group(vec![with("api", 1, "a"), with("api", 2, "b"), with("api", 3, "a"), with("web", 1, "c")]);

    let shape: Vec<(Option<String>, usize, Vec<usize>)> = groups
      .iter()
      .map(|group| (group.repo.clone(), group.total, group.symbols.iter().map(|symbol| symbol.sites.len()).collect()))
      .collect();
    assert_eq!(shape, vec![(Some("api".to_owned()), 3, vec![2, 1]), (Some("web".to_owned()), 1, vec![1])]);
  }
}
