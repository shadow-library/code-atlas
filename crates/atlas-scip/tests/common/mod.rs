#![allow(dead_code)]

use atlas_core::{ContentHash, EntityId, EntityKey, EntityKind, LanguageFamily, RefKind, RelPath, RepoName, Span, SymbolKey};
use atlas_scip::{ScipFileContext, ScipIngestContext, ScipReference, ScipSymbol, ScipTraitImpl};
use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, Relationship, SymbolInformation, SymbolRole, ToolInfo};

pub fn repo() -> RepoName {
  RepoName::new("demo").unwrap()
}

pub fn rel(path: &str) -> RelPath {
  RelPath::new(path).unwrap()
}

pub fn span_of(source: &str, needle: &str, nth: usize) -> Span {
  let start = source.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle:?} #{nth} not in source")).0;
  Span::of_text(source, start, start + needle.len()).unwrap()
}

pub fn symbol(source: &str, kind: EntityKind, item_path: &[&str], whole: &str, name: &str) -> ScipSymbol {
  symbol_nth(source, kind, item_path, whole, 0, name)
}

pub fn symbol_nth(source: &str, kind: EntityKind, item_path: &[&str], whole: &str, nth: usize, name: &str) -> ScipSymbol {
  let whole_span = span_of(source, whole, nth);
  let name_start = whole_span.start_byte as usize + whole.find(name).unwrap();
  let name_span = Span::of_text(source, name_start, name_start + name.len()).unwrap();
  let key = SymbolKey::new(
    LanguageFamily::Ts,
    repo(),
    "demo-pkg",
    vec!["a".to_owned()],
    item_path.iter().map(|segment| (*segment).to_owned()).collect(),
    kind.clone(),
    0,
  )
  .unwrap();
  ScipSymbol {
    entity: EntityId::of(&EntityKey::Symbol(key.clone())),
    key,
    kind,
    name: name.to_owned(),
    span: whole_span,
    name_span,
  }
}

pub fn reference(kind: RefKind, name: &str, span: Span) -> ScipReference {
  ScipReference {
    kind,
    name: Some(name.to_owned()),
    span,
  }
}

pub fn file(source: &str, symbols: Vec<ScipSymbol>, references: Vec<ScipReference>) -> ScipFileContext {
  ScipFileContext {
    content_hash: ContentHash::of(source.as_bytes()),
    source: source.to_owned(),
    symbols,
    references,
    trait_impls: Vec::new(),
  }
}

pub fn context(files: Vec<(&str, ScipFileContext)>) -> ScipIngestContext {
  files
    .into_iter()
    .fold(ScipIngestContext::new(repo()), |context, (path, file)| context.with_file(rel(path), file))
}

pub fn definition(symbol: &str, range: Vec<i32>) -> Occurrence {
  let mut occurrence = Occurrence::new();
  occurrence.symbol = symbol.to_owned();
  occurrence.range = range;
  occurrence.symbol_roles = SymbolRole::Definition as i32;
  occurrence
}

pub fn usage(symbol: &str, range: Vec<i32>) -> Occurrence {
  let mut occurrence = Occurrence::new();
  occurrence.symbol = symbol.to_owned();
  occurrence.range = range;
  occurrence
}

pub fn document(path: &str, encoding: PositionEncoding, occurrences: Vec<Occurrence>) -> Document {
  let mut document = Document::new();
  document.relative_path = path.to_owned();
  document.position_encoding = EnumOrUnknown::new(encoding);
  document.occurrences = occurrences;
  document
}

pub fn implementation(symbol: &str, implements: &str) -> SymbolInformation {
  let mut relationship = Relationship::new();
  relationship.symbol = implements.to_owned();
  relationship.is_implementation = true;
  let mut information = SymbolInformation::new();
  information.symbol = symbol.to_owned();
  information.relationships.push(relationship);
  information
}

pub fn index_bytes(tool: &str, documents: Vec<Document>) -> Vec<u8> {
  let mut tool_info = ToolInfo::new();
  tool_info.name = tool.to_owned();
  let mut metadata = Metadata::new();
  metadata.tool_info = Some(tool_info).into();
  let mut index = Index::new();
  index.metadata = Some(metadata).into();
  index.documents = documents;
  index.write_to_bytes().unwrap()
}

pub fn with_trait_impl(mut file: ScipFileContext, source: &str, whole: &str, self_name: &str, trait_name: &str) -> ScipFileContext {
  file.trait_impls.push(ScipTraitImpl {
    span: span_of(source, whole, 0),
    self_name: self_name.to_owned(),
    trait_name: trait_name.to_owned(),
  });
  file
}
