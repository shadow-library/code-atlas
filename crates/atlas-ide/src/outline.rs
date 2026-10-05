use std::ops::Range;
use std::path::Path;

use atlas_lsp::protocol::{DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, SymbolInformation, TextDocumentIdentifier};
use atlas_lsp::{LineIndex, PositionEncoding, PositionError, SyncDemand, canonical_path, path_from_uri, uri_from_path};
use atlas_query::{OutlineAnswer, Response};
use serde::Serialize;

use crate::error::{IdeError, IdeResult};
use crate::render::ServerView;
use crate::service::{IdeService, Session};
use crate::target::{FileRef, read_text};

pub const MAX_OUTLINE_DEPTH: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
  Lsp,
  Graph,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutlineSymbol {
  pub name: String,
  pub kind: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
  pub line: u32,
  pub column: u32,
  pub end_line: u32,
  pub children: Vec<OutlineSymbol>,
  #[serde(skip)]
  pub(crate) bytes: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExactOutline {
  pub file: FileRef,
  pub source: ReadSource,
  pub server: ServerView,
  pub symbols: Vec<OutlineSymbol>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outline {
  Exact(Box<ExactOutline>),
  Graph(atlas_query::Targeted<OutlineAnswer>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineRequest {
  pub file: String,
  pub depth: u32,
  pub source: ReadSource,
}

impl OutlineRequest {
  pub fn new(file: impl Into<String>) -> Self {
    Self {
      file: file.into(),
      depth: 1,
      source: ReadSource::Lsp,
    }
  }
}

impl IdeService {
  pub fn outline(&self, request: &OutlineRequest) -> IdeResult<Response<Outline>> {
    validate_depth(request.depth)?;
    if request.source == ReadSource::Graph {
      let response = self.query.outline(&atlas_query::OutlineRequest {
        file: request.file.clone(),
        depth: request.depth,
      })?;
      return Ok(Response {
        freshness: response.freshness,
        answer: Outline::Graph(response.answer),
      });
    }
    self.ensure_enabled().map_err(IdeError::with_graph_alternative)?;
    let files = self.files();
    let freshness = self.query.fresh()?;
    let file = files.locate_spec(&request.file)?;
    let (server, mut symbols) = self
      .exact(&freshness, &files, &file, SyncDemand::Best, std::slice::from_ref(&file.absolute), |session| {
        let (text, hash) = read_text(&file.absolute).map_err(|reason| IdeError::NotOpened {
          file: file.to_string(),
          server: session.server_name().to_owned(),
          reason,
        })?;
        match session.lease.open_document(&file.absolute) {
          None => {
            return Err(IdeError::NotOpened {
              file: file.to_string(),
              server: session.server_name().to_owned(),
              reason: "it is not open after syncing (too large or unreadable)".to_owned(),
            });
          }
          Some(open) if open.hash != hash => return Err(IdeError::SourceChanged { file: file.to_string() }),
          Some(_) => {}
        }
        document_symbols(session, &file.absolute, &text)
      })
      .map_err(IdeError::with_graph_alternative)?;
    truncate(&mut symbols, request.depth);
    Ok(Response {
      freshness,
      answer: Outline::Exact(Box::new(ExactOutline {
        file,
        source: ReadSource::Lsp,
        server,
        symbols,
      })),
    })
  }
}

pub(crate) fn validate_depth(depth: u32) -> IdeResult<()> {
  match depth <= MAX_OUTLINE_DEPTH {
    true => Ok(()),
    false => Err(IdeError::InvalidRequest(format!("depth must be between 0 and {MAX_OUTLINE_DEPTH}"))),
  }
}

pub(crate) fn document_symbols(session: &Session<'_>, path: &Path, text: &str) -> IdeResult<Vec<OutlineSymbol>> {
  let response: DocumentSymbolResponse = session.request(
    "textDocument/documentSymbol",
    DocumentSymbolParams {
      text_document: TextDocumentIdentifier {
        uri: uri_from_path(path).map_err(atlas_lsp::LspError::from)?,
      },
    },
  )?;
  let index = LineIndex::new(text, session.lease.line_breaks());
  let encoding = session.lease.encoding();
  let symbols = match response {
    DocumentSymbolResponse::Nested(symbols) => symbols.iter().map(|symbol| nested(symbol, &index, encoding)).collect::<Result<Vec<_>, _>>(),
    DocumentSymbolResponse::Flat(symbols) => flat(&symbols, path, &index, encoding),
  };
  let mut symbols = symbols.map_err(atlas_lsp::LspError::from)?;
  sort(&mut symbols);
  Ok(symbols)
}

fn node(name: &str, kind: &str, detail: Option<String>, range: Range<usize>, selection: usize, index: &LineIndex<'_>) -> Result<OutlineSymbol, PositionError> {
  let start = index.graph_span(selection..selection)?;
  let whole = index.graph_span(range.clone())?;
  Ok(OutlineSymbol {
    name: name.to_owned(),
    kind: kind.to_owned(),
    detail: detail.filter(|detail| !detail.is_empty()),
    line: start.start_line,
    column: start.start_col + 1,
    end_line: whole.end_line,
    children: Vec::new(),
    bytes: range,
  })
}

fn nested(symbol: &DocumentSymbol, index: &LineIndex<'_>, encoding: PositionEncoding) -> Result<OutlineSymbol, PositionError> {
  let range = index.offsets(symbol.range, encoding)?;
  let selection = index.offset(symbol.selection_range.start, encoding)?;
  let mut node = node(&symbol.name, symbol.kind.name(), symbol.detail.clone(), range, selection, index)?;
  node.children = symbol.children.iter().map(|child| nested(child, index, encoding)).collect::<Result<_, _>>()?;
  Ok(node)
}

fn flat(symbols: &[SymbolInformation], path: &Path, index: &LineIndex<'_>, encoding: PositionEncoding) -> Result<Vec<OutlineSymbol>, PositionError> {
  let in_file = |symbol: &&SymbolInformation| {
    path_from_uri(&symbol.location.uri)
      .ok()
      .and_then(|uri_path| canonical_path(&uri_path).ok())
      .is_some_and(|uri_path| uri_path == path)
  };
  let mut nodes = Vec::new();
  for symbol in symbols.iter().filter(in_file) {
    let range = index.offsets(symbol.location.range, encoding)?;
    let start = range.start;
    nodes.push(node(
      &symbol.name,
      symbol.kind.name(),
      symbol.container_name.clone().map(|container| format!("in {container}")),
      range,
      start,
      index,
    )?);
  }
  nodes.sort_by(|left, right| left.bytes.start.cmp(&right.bytes.start).then(right.bytes.end.cmp(&left.bytes.end)));
  let mut roots: Vec<OutlineSymbol> = Vec::new();
  for node in nodes {
    insert(&mut roots, node);
  }
  Ok(roots)
}

fn insert(siblings: &mut Vec<OutlineSymbol>, node: OutlineSymbol) {
  match siblings.last_mut() {
    Some(last) if last.bytes.start <= node.bytes.start && node.bytes.end <= last.bytes.end && last.bytes != node.bytes => insert(&mut last.children, node),
    _ => siblings.push(node),
  }
}

fn sort(symbols: &mut [OutlineSymbol]) {
  symbols.sort_by(|left, right| left.bytes.start.cmp(&right.bytes.start).then(right.bytes.end.cmp(&left.bytes.end)));
  for symbol in symbols {
    sort(&mut symbol.children);
  }
}

pub(crate) fn truncate(symbols: &mut [OutlineSymbol], depth: u32) {
  for symbol in symbols {
    match depth {
      0 => symbol.children.clear(),
      _ => truncate(&mut symbol.children, depth - 1),
    }
  }
}

pub(crate) fn innermost(symbols: &[OutlineSymbol], offset: usize) -> Option<&OutlineSymbol> {
  let found = symbols.iter().find(|symbol| symbol.bytes.start <= offset && offset < symbol.bytes.end)?;
  innermost(&found.children, offset).or(Some(found))
}

#[cfg(test)]
mod tests {
  use atlas_lsp::LineBreaks;
  use atlas_lsp::protocol::{Location, Position, Range as LspRange, SymbolKind};

  use super::*;

  fn range(start: (u32, u32), end: (u32, u32)) -> LspRange {
    LspRange {
      start: Position {
        line: start.0,
        character: start.1,
      },
      end: Position { line: end.0, character: end.1 },
    }
  }

  fn names(symbols: &[OutlineSymbol]) -> Vec<String> {
    symbols
      .iter()
      .map(|symbol| match symbol.children.is_empty() {
        true => format!("{}@{}:{}", symbol.name, symbol.line, symbol.column),
        false => format!("{}@{}:{}[{}]", symbol.name, symbol.line, symbol.column, names(&symbol.children).join(" ")),
      })
      .collect()
  }

  #[test]
  fn should_nest_flat_symbol_information_by_range_containment() {
    let text = "class Ü {\n  a() {}\n  b() {}\n}\nfunction f() {}\n";
    let index = LineIndex::new(text, LineBreaks::Ecma);
    let path = Path::new("/w/a.ts");
    let info = |name: &str, kind: u32, range: LspRange| SymbolInformation {
      name: name.to_owned(),
      kind: SymbolKind(kind),
      location: Location {
        uri: "file:///w/a.ts".to_owned(),
        range,
      },
      ..SymbolInformation::default()
    };
    let other_file = SymbolInformation {
      location: Location {
        uri: "file:///w/b.ts".to_owned(),
        range: range((0, 0), (0, 1)),
      },
      ..info("elsewhere", 12, range((0, 0), (0, 1)))
    };
    let symbols = [
      info("f", 12, range((4, 0), (4, 15))),
      info("b", 6, range((2, 2), (2, 8))),
      info("Ü", 5, range((0, 0), (3, 1))),
      info("a", 6, range((1, 2), (1, 8))),
      other_file,
    ];

    let mut outline = flat(&symbols, path, &index, PositionEncoding::Utf16).unwrap();

    assert_eq!(names(&outline), vec!["Ü@1:1[a@2:3 b@3:3]", "f@5:1"]);
    assert_eq!(innermost(&outline, text.find("b()").unwrap()).map(|symbol| symbol.name.as_str()), Some("b"));
    assert_eq!(innermost(&outline, text.find("function").unwrap()).map(|symbol| symbol.name.as_str()), Some("f"));
    truncate(&mut outline, 0);
    assert_eq!(names(&outline), vec!["Ü@1:1", "f@5:1"]);
  }

  #[test]
  fn should_read_nested_symbols_at_their_selection_in_utf8_columns() {
    let text = "/* é */ struct Store {\n  fn get() {}\n}\n";
    let index = LineIndex::new(text, LineBreaks::Lf);
    let symbol = DocumentSymbol {
      name: "Store".to_owned(),
      kind: SymbolKind(23),
      range: range((0, 8), (2, 1)),
      selection_range: range((0, 15), (0, 20)),
      children: vec![DocumentSymbol {
        name: "get".to_owned(),
        kind: SymbolKind(6),
        detail: Some("fn()".to_owned()),
        range: range((1, 2), (1, 13)),
        selection_range: range((1, 5), (1, 8)),
        ..DocumentSymbol::default()
      }],
      ..DocumentSymbol::default()
    };

    let node = nested(&symbol, &index, PositionEncoding::Utf16).unwrap();

    assert_eq!((node.line, node.column, node.end_line, node.kind.as_str()), (1, 17, 3, "struct"));
    assert_eq!((node.children[0].line, node.children[0].column, node.children[0].detail.as_deref()), (2, 6, Some("fn()")));
  }
}
