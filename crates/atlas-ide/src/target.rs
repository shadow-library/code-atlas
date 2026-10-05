use std::fmt;
use std::path::{Component, Path, PathBuf};

use atlas_core::{ContentHash, Entity, EntityKey, EntityKind, LanguageFamily, RelPath, RepoName, Span};
use atlas_engine::EngineError;
use atlas_lsp::canonical_path;
use atlas_project::Project;
use atlas_query::{EntityRef, Freshness, NameSpan, QueryEngine, SymbolTarget, Targeted};
use serde::Serialize;

use crate::error::{IdeError, IdeResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileRef {
  pub repo: RepoName,
  pub path: RelPath,
  #[serde(skip)]
  pub absolute: PathBuf,
}

impl fmt::Display for FileRef {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}:{}", self.repo, self.path)
  }
}

#[derive(Debug, Clone)]
struct RepoRoot {
  name: RepoName,
  root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectFiles {
  repos: Vec<RepoRoot>,
}

impl ProjectFiles {
  pub(crate) fn new(project: &Project) -> Self {
    let repos = project
      .repos
      .iter()
      .map(|repo| RepoRoot {
        name: repo.name.clone(),
        root: canonical_path(&repo.path).unwrap_or_else(|_| repo.path.clone()),
      })
      .collect();
    Self { repos }
  }

  pub(crate) fn repos(&self) -> Vec<RepoName> {
    self.repos.iter().map(|repo| repo.name.clone()).collect()
  }

  pub(crate) fn names(&self) -> Vec<String> {
    self.repos.iter().map(|repo| repo.name.to_string()).collect()
  }

  pub(crate) fn root(&self, repo: &RepoName) -> Option<&Path> {
    self.repos.iter().find(|candidate| candidate.name == *repo).map(|candidate| candidate.root.as_path())
  }

  fn roots(&self) -> Vec<String> {
    self.repos.iter().map(|repo| format!("{} ({})", repo.name, repo.root.display())).collect()
  }

  fn outside(&self, path: impl Into<String>) -> IdeError {
    IdeError::OutsideProject {
      path: path.into(),
      repos: self.roots(),
    }
  }

  pub(crate) fn file(&self, repo: &RepoName, path: &RelPath) -> IdeResult<FileRef> {
    let root = self.root(repo).ok_or_else(|| IdeError::UnknownRepo {
      repo: repo.to_string(),
      repos: self.names(),
    })?;
    let absolute = canonical_path(&root.join(path.as_str())).map_err(atlas_lsp::LspError::from)?;
    match self.of_absolute(&absolute) {
      Some(file) if file.repo == *repo => Ok(file),
      _ => Err(self.outside(format!("{repo}:{path}"))),
    }
  }

  pub(crate) fn of_absolute(&self, canonical: &Path) -> Option<FileRef> {
    let (repo, rest) = self
      .repos
      .iter()
      .filter_map(|repo| canonical.strip_prefix(&repo.root).ok().map(|rest| (repo, rest)))
      .min_by_key(|(_, rest)| rest.components().count())?;
    let parts: Option<Vec<&str>> = rest
      .components()
      .map(|component| match component {
        Component::Normal(part) => part.to_str(),
        _ => None,
      })
      .collect();
    let path = RelPath::new(&parts?.join("/")).ok()?;
    Some(FileRef {
      repo: repo.name.clone(),
      path,
      absolute: canonical.to_path_buf(),
    })
  }

  pub(crate) fn locate_spec(&self, spec: &str) -> IdeResult<FileRef> {
    let spec = spec.trim();
    match spec.split_once(':') {
      Some((repo, path)) if self.repos.iter().any(|candidate| candidate.name.as_str() == repo) => self.locate(Some(repo), path),
      _ => self.locate(None, spec),
    }
  }

  pub(crate) fn locate(&self, repo: Option<&str>, path: &str) -> IdeResult<FileRef> {
    let raw = Path::new(path);
    if raw.is_absolute() {
      let canonical = canonical_path(raw).map_err(|_| self.outside(path))?;
      return self.of_absolute(&canonical).ok_or_else(|| self.outside(path));
    }
    let relative = RelPath::new(path).map_err(|_| self.outside(path))?;
    if let Some(repo) = repo {
      let name = self.repos.iter().find(|candidate| candidate.name.as_str() == repo).ok_or_else(|| IdeError::UnknownRepo {
        repo: repo.to_owned(),
        repos: self.names(),
      })?;
      return self.file(&name.name, &relative);
    }
    let existing: Vec<&RepoRoot> = self.repos.iter().filter(|candidate| candidate.root.join(relative.as_str()).is_file()).collect();
    match existing.as_slice() {
      [] => Err(IdeError::FileNotFound {
        path: path.to_owned(),
        repos: self.names(),
      }),
      [only] => self.file(&only.name, &relative),
      many => Err(IdeError::AmbiguousFile {
        path: path.to_owned(),
        repos: many.iter().map(|candidate| candidate.name.to_string()).collect(),
      }),
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) enum Target {
  Symbol {
    entity: Box<Entity>,
    file: FileRef,
    name: Span,
    indexed: ContentHash,
  },
  Position {
    file: FileRef,
    line: u32,
    column: u32,
  },
}

#[derive(Debug, Clone)]
pub(crate) struct Anchor {
  pub file: FileRef,
  pub text: String,
  pub hash: ContentHash,
  pub offset: usize,
}

pub(crate) struct Resolver<'a> {
  pub query: &'a QueryEngine,
  pub files: &'a ProjectFiles,
}

enum Attempt<T> {
  Done(T),
  SourceChanged(String),
}

impl Resolver<'_> {
  pub(crate) fn resolve(&self, text: &str, freshness: &mut Freshness) -> IdeResult<Targeted<Target>> {
    let mut resynced = false;
    loop {
      match self.attempt(text)? {
        Attempt::Done(target) => return Ok(target),
        Attempt::SourceChanged(file) if resynced => return Err(IdeError::SourceChanged { file }),
        Attempt::SourceChanged(file) => {
          tracing::info!(file, "target file changed since indexing; re-running the freshness barrier");
          resynced = true;
          resync(self.query, freshness)?;
        }
      }
    }
  }

  fn attempt(&self, text: &str) -> IdeResult<Attempt<Targeted<Target>>> {
    let parsed = SymbolTarget::parse(text)?;
    match &parsed {
      SymbolTarget::Position { repo, path, line, column } => {
        let file = self.files.locate(repo.as_deref(), path)?;
        return Ok(Attempt::Done(Targeted::Found(Target::Position {
          file,
          line: *line,
          column: *column,
        })));
      }
      SymbolTarget::File { repo, path } => {
        self.files.locate(repo.as_deref(), path)?;
        return Err(IdeError::NotASymbol {
          spec: text.to_owned(),
          kind: "file".to_owned(),
        });
      }
      SymbolTarget::Line { repo, path, .. } => {
        self.files.locate(repo.as_deref(), path)?;
      }
      SymbolTarget::Key(_) | SymbolTarget::Id(_) | SymbolTarget::NamePath(_) => {}
    }
    let entity = match self.query.resolve_target(text)? {
      Targeted::Found(entity) => entity,
      Targeted::NotFound { spec, hint } => return Ok(Attempt::Done(Targeted::NotFound { spec, hint })),
      Targeted::Ambiguous { spec, candidates } => match self.first_overload(&candidates)? {
        Some(entity) => entity,
        None => return Ok(Attempt::Done(Targeted::Ambiguous { spec, candidates })),
      },
    };
    self.symbol(text, entity)
  }

  fn first_overload(&self, candidates: &[EntityRef]) -> IdeResult<Option<Entity>> {
    let keys: Vec<EntityKey> = candidates.iter().filter_map(|candidate| candidate.key.parse().ok()).collect();
    let symbols: Vec<_> = keys.iter().filter_map(EntityKey::as_symbol).collect();
    let Some(first) = symbols.iter().min_by_key(|symbol| symbol.disambiguator()) else {
      return Ok(None);
    };
    let base = (*first).clone().with_disambiguator(0);
    let same_overload_set = symbols.len() == candidates.len() && symbols.iter().all(|symbol| (*symbol).clone().with_disambiguator(0) == base);
    if !same_overload_set || first.family() == LanguageFamily::Rust {
      return Ok(None);
    }
    match self.query.resolve_target(&EntityKey::Symbol((*first).clone()).to_string())? {
      Targeted::Found(entity) => Ok(Some(entity)),
      Targeted::NotFound { .. } | Targeted::Ambiguous { .. } => Ok(None),
    }
  }

  fn symbol(&self, text: &str, entity: Entity) -> IdeResult<Attempt<Targeted<Target>>> {
    if entity.kind == EntityKind::ApiEndpoint {
      return Err(IdeError::EndpointTarget { spec: text.to_owned() });
    }
    if !entity.kind.is_code_symbol() {
      return Err(IdeError::NotASymbol {
        spec: text.to_owned(),
        kind: entity.kind.to_string(),
      });
    }
    let (Some(repo), Some(path)) = (&entity.repo, &entity.path) else {
      return Err(IdeError::NoNameToken {
        target: entity.key.to_string(),
        reason: "the symbol has no file".to_owned(),
      });
    };
    let file = self.files.file(repo, path)?;
    let indexed = || -> IdeResult<Option<ContentHash>> { Ok(self.query.engine().store().read(|tx| tx.file(repo, path))?.map(|record| record.content_hash)) };
    let before = indexed()?;
    let name = self.query.name_span(&entity)?;
    let after = indexed()?;
    let (Some(indexed), true) = (before, before == after) else {
      return Ok(Attempt::SourceChanged(file.to_string()));
    };
    match name {
      NameSpan::Found { span } => Ok(Attempt::Done(Targeted::Found(Target::Symbol {
        entity: Box::new(entity),
        file,
        name: span,
        indexed,
      }))),
      NameSpan::SourceChanged => Ok(Attempt::SourceChanged(file.to_string())),
      NameSpan::Unavailable { reason } => Err(IdeError::NoNameToken {
        target: entity.key.to_string(),
        reason,
      }),
    }
  }

  pub(crate) fn anchor(&self, target: &Target) -> IdeResult<Anchor> {
    match target {
      Target::Symbol { file, name, indexed, .. } => {
        let (text, hash) = match self.query.engine().read_source(&file.repo, &file.path) {
          Ok(read) => read,
          Err(EngineError::SourceChanged { .. }) => return Err(IdeError::SourceChanged { file: file.to_string() }),
          Err(error) => return Err(error.into()),
        };
        if hash != *indexed {
          return Err(IdeError::SourceChanged { file: file.to_string() });
        }
        Ok(Anchor {
          file: file.clone(),
          text,
          hash,
          offset: name.start_byte as usize,
        })
      }
      Target::Position { file, line, column } => {
        let (text, hash) = read_text(&file.absolute).map_err(|reason| IdeError::InvalidPosition {
          file: file.to_string(),
          line: *line,
          column: *column,
          reason,
        })?;
        let offset = offset_of(&text, *line, *column).map_err(|reason| IdeError::InvalidPosition {
          file: file.to_string(),
          line: *line,
          column: *column,
          reason,
        })?;
        Ok(Anchor {
          file: file.clone(),
          text,
          hash,
          offset,
        })
      }
    }
  }
}

pub(crate) fn resync(query: &QueryEngine, freshness: &mut Freshness) -> IdeResult<()> {
  let later = query.fresh()?;
  freshness.notes.push(format!(
    "a source file changed while answering; the index was re-synced to run {}",
    later.run.map_or_else(|| "none".to_owned(), |run| run.to_string())
  ));
  if later.is_degraded() {
    freshness.state = later.state;
  }
  freshness.run = later.run;
  Ok(())
}

pub(crate) fn read_text(path: &Path) -> Result<(String, ContentHash), String> {
  let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
  let hash = ContentHash::of(&bytes);
  String::from_utf8(bytes).map(|text| (text, hash)).map_err(|_| "the file is not UTF-8".to_owned())
}

pub(crate) fn offset_of(text: &str, line: u32, column: u32) -> Result<usize, String> {
  if line == 0 || column == 0 {
    return Err("lines and columns are 1-based".to_owned());
  }
  let mut start = 0;
  for _ in 1..line {
    match text[start..].find('\n') {
      Some(at) => start += at + 1,
      None => return Err(format!("the file has only {} lines", text.split('\n').count())),
    }
  }
  let end = text[start..].find('\n').map_or(text.len(), |at| start + at);
  let content_end = if text[start..end].ends_with('\r') { end - 1 } else { end };
  let offset = start + (column as usize - 1);
  if offset > content_end {
    return Err(format!("line {line} has {} bytes", content_end - start));
  }
  if !text.is_char_boundary(offset) {
    return Err(format!("column {column} is inside a multi-byte character (columns count UTF-8 bytes)"));
  }
  Ok(offset)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_convert_one_based_byte_columns_to_offsets() {
    let text = "ab\r\né = 1;\nlast";

    assert_eq!(offset_of(text, 1, 1), Ok(0));
    assert_eq!(offset_of(text, 1, 3), Ok(2));
    assert_eq!(offset_of(text, 2, 1), Ok(4));
    assert_eq!(offset_of(text, 2, 4), Ok(7));
    assert_eq!(offset_of(text, 3, 5), Ok(16));
    assert_eq!(offset_of(text, 1, 4), Err("line 1 has 2 bytes".to_owned()));
    assert_eq!(
      offset_of(text, 2, 2),
      Err("column 2 is inside a multi-byte character (columns count UTF-8 bytes)".to_owned())
    );
    assert_eq!(offset_of(text, 4, 1), Err("the file has only 3 lines".to_owned()));
  }
}
