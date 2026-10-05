use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Span};
use atlas_lsp::protocol::Location;
use atlas_lsp::{LineIndex, canonical_path, path_from_uri};
use atlas_query::EntityRef;
use serde::Serialize;

use crate::error::IdeResult;
use crate::outline::{OutlineSymbol, document_symbols, innermost};
use crate::service::Session;
use crate::target::{FileRef, read_text};

const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SitePlace {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub repo: Option<String>,
  pub path: String,
  pub line: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub column: Option<u32>,
}

impl fmt::Display for SitePlace {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(repo) = &self.repo {
      write!(formatter, "{repo}:")?;
    }
    write!(formatter, "{}:{}", self.path, self.line)?;
    match self.column {
      Some(column) => write!(formatter, ":{column}"),
      None => Ok(()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Enclosing {
  Graph { symbol: EntityRef },
  Lsp { name: String, kind: String },
}

impl Enclosing {
  pub fn label(&self) -> String {
    match self {
      Self::Graph { symbol } if symbol.kind == "test" || symbol.kind == "test_suite" => format!("{} (test)", symbol.tagged_short()),
      Self::Graph { symbol } => symbol.tagged_short(),
      Self::Lsp { name, kind } => format!("{name} ({kind})"),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Site {
  pub place: SitePlace,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub enclosing: Option<Enclosing>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub unverified: Option<String>,
}

enum Text {
  Verified { text: String, indexed: bool },
  Unverified { reason: String },
}

struct Known {
  file: Option<FileRef>,
  text: Text,
  outline: Option<Vec<OutlineSymbol>>,
}

pub(crate) struct SiteMapper<'s, 'a> {
  session: &'s Session<'a>,
  known: HashMap<PathBuf, Known>,
}

impl<'s, 'a> SiteMapper<'s, 'a> {
  pub(crate) fn new(session: &'s Session<'a>) -> Self {
    Self { session, known: HashMap::new() }
  }

  pub(crate) fn map_all(&mut self, locations: Vec<Location>) -> IdeResult<Vec<Site>> {
    let mut sites = Vec::with_capacity(locations.len());
    for location in locations {
      sites.push(self.map(&location)?);
    }
    sites.sort_by(|left, right| left.place.cmp(&right.place));
    sites.dedup_by(|left, right| left.place == right.place);
    Ok(sites)
  }

  pub(crate) fn map(&mut self, location: &Location) -> IdeResult<Site> {
    let path = match path_from_uri(&location.uri).and_then(|path| canonical_path(&path)) {
      Ok(path) => path,
      Err(error) => {
        return Ok(Site {
          place: SitePlace {
            repo: None,
            path: location.uri.clone(),
            line: location.range.start.line + 1,
            column: None,
          },
          enclosing: None,
          snippet: None,
          unverified: Some(format!("the server answered with a location code-atlas cannot read ({error})")),
        });
      }
    };
    self.learn(&path)?;
    let known = &self.known[&path];
    let repo = known.file.as_ref().map(|file| file.repo.to_string());
    let shown_path = known.file.as_ref().map_or_else(|| path.display().to_string(), |file| file.path.to_string());
    let place = |line: u32, column: Option<u32>| SitePlace {
      repo: repo.clone(),
      path: shown_path.clone(),
      line,
      column,
    };
    let unverified = |reason: String| Site {
      place: place(location.range.start.line + 1, None),
      enclosing: None,
      snippet: None,
      unverified: Some(reason),
    };
    let (start, snippet, indexed) = match &known.text {
      Text::Unverified { reason } => return Ok(unverified(format!("{reason}; line as reported by the server"))),
      Text::Verified { text, indexed } => {
        let index = LineIndex::new(text, self.session.lease.line_breaks());
        match index
          .offset(location.range.start, self.session.lease.encoding())
          .and_then(|offset| index.graph_span(offset..offset))
        {
          Ok(start) => (start, line_text(text, start.start_byte as usize), *indexed),
          Err(error) => return Ok(unverified(format!("the server's position does not fit the file ({error})"))),
        }
      }
    };
    let enclosing = self.enclosing(&path, &start, indexed)?;
    Ok(Site {
      place: place(start.start_line, Some(start.start_col + 1)),
      enclosing,
      snippet,
      unverified: None,
    })
  }

  fn enclosing(&mut self, path: &Path, start: &Span, indexed: bool) -> IdeResult<Option<Enclosing>> {
    let Some(file) = self.known[path].file.clone() else { return Ok(None) };
    if indexed {
      let symbol = self.session.query.innermost_symbol(&file.repo, &file.path, start.start_line, start.start_col + 1)?;
      return Ok(symbol.map(|entity| Enclosing::Graph { symbol: EntityRef::of(&entity) }));
    }
    let Some(known) = self.known.get_mut(path) else { return Ok(None) };
    if known.outline.is_none() {
      let Text::Verified { text, .. } = &known.text else { return Ok(None) };
      let symbols = document_symbols(self.session, path, text).unwrap_or_else(|error| {
        tracing::debug!(%error, path = %path.display(), "no document symbols for an unindexed file; sites there have no enclosing symbol");
        Vec::new()
      });
      known.outline = Some(symbols);
    }
    let outline = known.outline.as_deref().unwrap_or_default();
    Ok(innermost(outline, start.start_byte as usize).map(|symbol| Enclosing::Lsp {
      name: symbol.name.clone(),
      kind: symbol.kind.clone(),
    }))
  }

  fn learn(&mut self, path: &Path) -> IdeResult<()> {
    if self.known.contains_key(path) {
      return Ok(());
    }
    let file = self.session.files.of_absolute(path);
    let indexed = match &file {
      Some(file) => self
        .session
        .query
        .engine()
        .store()
        .read(|tx| tx.file(&file.repo, &file.path))?
        .map(|record| record.content_hash),
      None => None,
    };
    let open = self.session.lease.open_document(path).map(|doc| doc.hash);
    let text = match read_text(path) {
      Err(reason) => Text::Unverified {
        reason: format!("cannot read the file ({reason})"),
      },
      Ok((text, hash)) => verify(text, hash, open, indexed),
    };
    self.known.insert(path.to_path_buf(), Known { file, text, outline: None });
    Ok(())
  }
}

fn verify(text: String, hash: ContentHash, open: Option<ContentHash>, indexed: Option<ContentHash>) -> Text {
  match (open, indexed) {
    (Some(sent), _) if sent != hash => Text::Unverified {
      reason: "the file changed after it was sent to the language server".to_owned(),
    },
    (None, Some(indexed)) if indexed != hash => Text::Unverified {
      reason: "the file changed after indexing".to_owned(),
    },
    (_, indexed) => Text::Verified {
      text,
      indexed: indexed == Some(hash),
    },
  }
}

pub(crate) fn line_text(text: &str, offset: usize) -> Option<String> {
  let start = text.get(..offset)?.rfind('\n').map_or(0, |at| at + 1);
  let end = text[offset..].find('\n').map_or(text.len(), |at| offset + at);
  let content = text[start..end].trim();
  Some(match content.char_indices().nth(SNIPPET_CHARS) {
    Some((cut, _)) => format!("{}…", &content[..cut]),
    None => content.to_owned(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_cut_snippets_at_160_characters_and_trim_them() {
    let long = format!("  {}\r\nnext", "é".repeat(200));

    assert_eq!(line_text("a\n  let x = 1;\r\nb", 4), Some("let x = 1;".to_owned()));
    assert_eq!(line_text(&long, 2), Some(format!("{}…", "é".repeat(160))));
    assert_eq!(line_text("abc", 9), None);
  }

  #[test]
  fn should_trust_only_text_matching_what_the_server_or_the_index_saw() {
    let hash = ContentHash::of(b"x");
    let other = ContentHash::of(b"y");

    assert!(matches!(verify("x".into(), hash, Some(hash), Some(other)), Text::Verified { indexed: false, .. }));
    assert!(matches!(verify("x".into(), hash, Some(other), Some(hash)), Text::Unverified { .. }));
    assert!(matches!(verify("x".into(), hash, None, Some(other)), Text::Unverified { .. }));
    assert!(matches!(verify("x".into(), hash, None, Some(hash)), Text::Verified { indexed: true, .. }));
    assert!(matches!(verify("x".into(), hash, None, None), Text::Verified { indexed: false, .. }));
  }
}
