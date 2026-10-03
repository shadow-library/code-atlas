use std::collections::BTreeMap;

use atlas_core::{ContentHash, EntityId, EntityKey, EntityKind, FileFacts, RefKind, RelPath, RepoName, Span, SymbolKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipSymbol {
  pub key: SymbolKey,
  pub entity: EntityId,
  pub kind: EntityKind,
  pub name: String,
  pub span: Span,
  pub name_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipReference {
  pub kind: RefKind,
  pub name: Option<String>,
  pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipTraitImpl {
  pub span: Span,
  pub self_name: String,
  pub trait_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipFileContext {
  pub content_hash: ContentHash,
  pub source: String,
  pub symbols: Vec<ScipSymbol>,
  pub references: Vec<ScipReference>,
  pub trait_impls: Vec<ScipTraitImpl>,
}

impl ScipFileContext {
  pub fn from_facts(facts: &FileFacts, keys: Vec<SymbolKey>, source: impl Into<String>) -> Self {
    let symbols = facts
      .symbols
      .iter()
      .zip(keys)
      .map(|(symbol, key)| ScipSymbol {
        entity: EntityId::of(&EntityKey::Symbol(key.clone())),
        key,
        kind: symbol.kind.clone(),
        name: symbol.name.clone(),
        span: symbol.span,
        name_span: symbol.name_span,
      })
      .collect();
    let references = facts
      .references
      .iter()
      .map(|reference| ScipReference {
        kind: reference.kind,
        name: reference.name().map(str::to_owned),
        span: reference.span,
      })
      .collect();
    let trait_impls = facts
      .impls
      .iter()
      .filter_map(|implementation| {
        Some(ScipTraitImpl {
          span: implementation.span,
          self_name: implementation.self_type.segments.last()?.clone(),
          trait_name: implementation.trait_ref.as_ref()?.segments.last()?.clone(),
        })
      })
      .collect();
    Self {
      content_hash: facts.file.content_hash,
      source: source.into(),
      symbols,
      references,
      trait_impls,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipIngestContext {
  pub repo: RepoName,
  pub package_dir: Option<RelPath>,
  pub files: BTreeMap<RelPath, ScipFileContext>,
}

impl ScipIngestContext {
  pub fn new(repo: RepoName) -> Self {
    Self {
      repo,
      package_dir: None,
      files: BTreeMap::new(),
    }
  }

  pub fn in_package_dir(mut self, package_dir: RelPath) -> Self {
    self.package_dir = Some(package_dir);
    self
  }

  pub fn with_file(mut self, path: RelPath, file: ScipFileContext) -> Self {
    self.files.insert(path, file);
    self
  }

  pub(crate) fn repo_path(&self, document_path: &str) -> Option<RelPath> {
    let relative = RelPath::new(document_path).ok()?;
    match &self.package_dir {
      Some(dir) => dir.join(relative.as_str()).ok(),
      None => Some(relative),
    }
  }
}
