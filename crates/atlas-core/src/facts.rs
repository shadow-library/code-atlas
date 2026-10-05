use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::args::ArgSummary;
use crate::error::CoreError;
use crate::ids::{ContentHash, RelPath, RepoName};
use crate::keys::SymbolKey;
use crate::kinds::EntityKind;
use crate::lang::Language;
use crate::provenance::Producer;
use crate::span::Span;

pub const MAX_REPORTED_PARSE_ERRORS: usize = 8;
pub const MAX_BODY_TOKENS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRef {
  pub repo: RepoName,
  pub path: RelPath,
  pub language: Language,
  pub content_hash: ContentHash,
  pub is_declaration_file: bool,
}

impl FileRef {
  pub fn detect(repo: RepoName, path: RelPath, content_hash: ContentHash) -> Result<Self, CoreError> {
    let language = Language::from_path(path.as_str()).ok_or_else(|| CoreError::UnsupportedPath(path.to_string()))?;
    let is_declaration_file = Language::is_declaration_file(path.as_str());
    Ok(Self {
      repo,
      path,
      language,
      content_hash,
      is_declaration_file,
    })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocalSymbolId(pub u32);

impl LocalSymbolId {
  pub fn index(self) -> usize {
    self.0 as usize
  }
}

impl fmt::Display for LocalSymbolId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "#{}", self.0)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ParseStatus {
  Ok,
  Partial { error_count: u32, first_errors: Vec<ParseErrorFact> },
  Failed { reason: String },
}

impl ParseStatus {
  // Matched by reason prefix: stored artifacts serialize `ParseStatus`, so a new variant would change their encoding.
  pub fn is_budget_exceeded(&self) -> bool {
    matches!(self, Self::Failed { reason } if reason.starts_with(crate::PARSE_BUDGET_EXCEEDED))
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParseErrorFact {
  pub span: Span,
  pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
  Info,
  Warning,
  Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Diagnostic {
  pub severity: Severity,
  pub code: String,
  pub message: String,
  pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Visibility {
  Public,
  Crate,
  Restricted(String),
  Protected,
  Private,
  Exported,
  ExportedDefault,
  Module,
}

impl Visibility {
  pub fn is_externally_visible(&self) -> bool {
    matches!(self, Self::Public | Self::Exported | Self::ExportedDefault)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Modifier {
  Async,
  Static,
  Abstract,
  Const,
  Unsafe,
  Generic,
  Override,
  Readonly,
  Extern,
  Declare,
  Optional,
  Getter,
  Setter,
  Generator,
  Mutable,
  Default,
}

impl Modifier {
  pub const ALL: [Modifier; 16] = [
    Self::Async,
    Self::Static,
    Self::Abstract,
    Self::Const,
    Self::Unsafe,
    Self::Generic,
    Self::Override,
    Self::Readonly,
    Self::Extern,
    Self::Declare,
    Self::Optional,
    Self::Getter,
    Self::Setter,
    Self::Generator,
    Self::Mutable,
    Self::Default,
  ];

  fn bit(self) -> u32 {
    1 << (self as u32)
  }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(from = "Vec<Modifier>", into = "Vec<Modifier>")]
pub struct Modifiers(u32);

impl Modifiers {
  pub fn empty() -> Self {
    Self(0)
  }

  pub fn insert(&mut self, modifier: Modifier) {
    self.0 |= modifier.bit();
  }

  pub fn with(mut self, modifier: Modifier) -> Self {
    self.insert(modifier);
    self
  }

  pub fn contains(self, modifier: Modifier) -> bool {
    self.0 & modifier.bit() != 0
  }

  pub fn is_empty(self) -> bool {
    self.0 == 0
  }

  pub fn iter(self) -> impl Iterator<Item = Modifier> {
    Modifier::ALL.into_iter().filter(move |modifier| self.contains(*modifier))
  }
}

impl fmt::Debug for Modifiers {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_set().entries(self.iter()).finish()
  }
}

impl FromIterator<Modifier> for Modifiers {
  fn from_iter<I: IntoIterator<Item = Modifier>>(modifiers: I) -> Self {
    modifiers.into_iter().fold(Self::empty(), Self::with)
  }
}

impl From<Vec<Modifier>> for Modifiers {
  fn from(modifiers: Vec<Modifier>) -> Self {
    modifiers.into_iter().collect()
  }
}

impl From<Modifiers> for Vec<Modifier> {
  fn from(modifiers: Modifiers) -> Self {
    modifiers.iter().collect()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttributeFact {
  pub path: Vec<String>,
  pub args: Vec<ArgSummary>,
  pub span: Span,
}

impl AttributeFact {
  pub fn is(&self, name: &str) -> bool {
    self.path.last().is_some_and(|last| last == name)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParamFact {
  pub name: String,
  pub type_text: Option<String>,
  pub attributes: Vec<AttributeFact>,
  pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
  Unit,
  Integration,
  Suite,
  Case,
  Bench,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TestInfo {
  pub framework: String,
  pub kind: TestKind,
  pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypePath {
  pub segments: Vec<String>,
  pub generic_args: Option<String>,
}

impl TypePath {
  pub fn new(segments: Vec<String>) -> Self {
    Self { segments, generic_args: None }
  }

  pub fn name(&self) -> Option<&str> {
    self.segments.last().map(String::as_str)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImplInfo {
  pub self_type: TypePath,
  pub trait_ref: Option<TypePath>,
}

impl ImplInfo {
  pub fn item_prefix(&self) -> Vec<String> {
    let self_name = self.self_type.name().unwrap_or("_").to_owned();
    let Some(trait_ref) = &self.trait_ref else { return vec![self_name] };
    let trait_name = trait_ref.name().unwrap_or("_");
    let trait_segment = match &trait_ref.generic_args {
      Some(args) => format!("<{trait_name}<{args}>>"),
      None => format!("<{trait_name}>"),
    };
    vec![self_name, trait_segment]
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BodyFingerprintInput {
  pub normalized_tokens: Vec<u32>,
  pub token_count: u32,
  pub line_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormToken<'a> {
  Identifier,
  StringLiteral,
  NumberLiteral,
  Keyword(&'a str),
  Punctuation(&'a str),
}

const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;
const TOKEN_IDENTIFIER: u32 = 1;
const TOKEN_STRING: u32 = 2;
const TOKEN_NUMBER: u32 = 3;
const RESERVED_TOKEN_CODES: u32 = 16;

pub fn token_code(token: NormToken<'_>) -> u32 {
  let (tag, text) = match token {
    NormToken::Identifier => return TOKEN_IDENTIFIER,
    NormToken::StringLiteral => return TOKEN_STRING,
    NormToken::NumberLiteral => return TOKEN_NUMBER,
    NormToken::Keyword(text) => (b'k', text),
    NormToken::Punctuation(text) => (b'p', text),
  };
  let hash = std::iter::once(tag)
    .chain(text.bytes())
    .fold(FNV_OFFSET, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(FNV_PRIME));
  hash.max(RESERVED_TOKEN_CODES)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolFact {
  pub local: LocalSymbolId,
  pub parent: Option<LocalSymbolId>,
  pub kind: EntityKind,
  pub name: String,
  pub module_suffix: Vec<String>,
  pub item_path: Vec<String>,
  pub span: Span,
  pub name_span: Span,
  pub signature: Option<String>,
  pub doc: Option<String>,
  pub visibility: Visibility,
  pub modifiers: Modifiers,
  pub attributes: Vec<AttributeFact>,
  pub params: Vec<ParamFact>,
  pub test: Option<TestInfo>,
  pub impl_of: Option<ImplInfo>,
  pub extends: Vec<Vec<String>>,
  pub implements: Vec<Vec<String>>,
  pub body: Option<BodyFingerprintInput>,
}

impl SymbolFact {
  pub fn new(local: LocalSymbolId, kind: EntityKind, name: impl Into<String>, item_path: Vec<String>, span: Span, name_span: Span, visibility: Visibility) -> Self {
    Self {
      local,
      parent: None,
      kind,
      name: name.into(),
      module_suffix: Vec::new(),
      item_path,
      span,
      name_span,
      signature: None,
      doc: None,
      visibility,
      modifiers: Modifiers::empty(),
      attributes: Vec::new(),
      params: Vec::new(),
      test: None,
      impl_of: None,
      extends: Vec::new(),
      implements: Vec::new(),
      body: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsePath {
  pub segments: Vec<String>,
  pub binding: UseBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum UseBinding {
  Name,
  Alias(String),
  Glob,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImportBinding {
  Named { imported: String, local: String, type_only: bool },
  Default { local: String },
  Namespace { local: String },
}

impl ImportBinding {
  pub fn local(&self) -> &str {
    match self {
      Self::Named { local, .. } | Self::Default { local } | Self::Namespace { local } => local,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportFact {
  RustUse {
    paths: Vec<UsePath>,
    visibility: Visibility,
    scope: Option<LocalSymbolId>,
    span: Span,
  },
  RustExternCrate {
    name: String,
    alias: Option<String>,
    span: Span,
  },
  TsImport {
    specifier: String,
    bindings: Vec<ImportBinding>,
    type_only: bool,
    span: Span,
  },
  TsRequire {
    specifier: ArgSummary,
    bindings: Vec<ImportBinding>,
    scope: Option<LocalSymbolId>,
    span: Span,
  },
  TsDynamicImport {
    specifier: ArgSummary,
    scope: Option<LocalSymbolId>,
    span: Span,
  },
}

impl ImportFact {
  pub fn span(&self) -> Span {
    match self {
      Self::RustUse { span, .. } | Self::RustExternCrate { span, .. } | Self::TsImport { span, .. } | Self::TsRequire { span, .. } | Self::TsDynamicImport { span, .. } => *span,
    }
  }

  pub fn is_re_export(&self) -> bool {
    matches!(self, Self::RustUse { visibility, .. } if *visibility != Visibility::Private)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReExportBinding {
  pub imported: String,
  pub exported: String,
  pub type_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DefaultExportTarget {
  Identifier(String),
  Expression(ArgSummary),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportFact {
  Named {
    local: String,
    exported: String,
    type_only: bool,
    span: Span,
  },
  Default {
    target: DefaultExportTarget,
    span: Span,
  },
  ReExport {
    specifier: String,
    bindings: Vec<ReExportBinding>,
    type_only: bool,
    span: Span,
  },
  ReExportAll {
    specifier: String,
    namespace: Option<String>,
    type_only: bool,
    span: Span,
  },
  CommonJs {
    exported: Option<String>,
    value: ArgSummary,
    span: Span,
  },
}

impl ExportFact {
  pub fn span(&self) -> Span {
    match self {
      Self::Named { span, .. } | Self::Default { span, .. } | Self::ReExport { span, .. } | Self::ReExportAll { span, .. } | Self::CommonJs { span, .. } => *span,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RefKind {
  Call,
  MethodCall,
  MacroCall,
  TypeRef,
  Instantiation,
  ValueRef,
  Decorator,
  Inherit,
  JsxElement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclaredBinding {
  Param,
  Let,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReceiverHint {
  SelfValue,
  This,
  Identifier(String),
  Expr(String),
  TypeHint(Vec<String>),
  Declared { type_text: String, binding: DeclaredBinding },
  Chain(Box<ReceiverChain>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReceiverChain {
  pub head: ChainHead,
  pub steps: Vec<ChainStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChainHead {
  Binding { name: String, hint: ReceiverHint },
  Call { path: Vec<String>, args: Vec<Option<String>> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChainStep {
  Field(String),
  Method(String),
  Try,
  Await,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RefOrigin {
  #[default]
  Syntax,
  MacroTokenScan,
}

impl RefOrigin {
  pub fn is_syntax(&self) -> bool {
    *self == Self::Syntax
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReferenceFact {
  pub from: Option<LocalSymbolId>,
  pub kind: RefKind,
  pub path: Vec<String>,
  pub receiver: Option<ReceiverHint>,
  pub args: Vec<ArgSummary>,
  pub span: Span,
  #[serde(default, skip_serializing_if = "RefOrigin::is_syntax")]
  pub origin: RefOrigin,
}

impl ReferenceFact {
  pub fn name(&self) -> Option<&str> {
    self.path.last().map(String::as_str)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ModuleBody {
  External,
  Inline(LocalSymbolId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModuleDeclFact {
  pub name: String,
  pub body: ModuleBody,
  pub path_attr: Option<String>,
  pub module_suffix: Vec<String>,
  pub visibility: Visibility,
  pub attributes: Vec<AttributeFact>,
  pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImplFact {
  pub self_type: TypePath,
  pub trait_ref: Option<TypePath>,
  pub generic_params: Vec<String>,
  pub module_suffix: Vec<String>,
  pub scope: Option<LocalSymbolId>,
  pub attributes: Vec<AttributeFact>,
  pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyContext {
  pub package: String,
  pub module_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFacts {
  pub file: FileRef,
  pub parse_status: ParseStatus,
  pub symbols: Vec<SymbolFact>,
  pub imports: Vec<ImportFact>,
  pub exports: Vec<ExportFact>,
  pub references: Vec<ReferenceFact>,
  pub module_decls: Vec<ModuleDeclFact>,
  pub diagnostics: Vec<Diagnostic>,
  pub analyzer: Producer,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub impls: Vec<ImplFact>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub file_doc: Option<String>,
}

impl FileFacts {
  pub fn new(file: FileRef, analyzer: Producer) -> Self {
    Self {
      file,
      parse_status: ParseStatus::Ok,
      symbols: Vec::new(),
      imports: Vec::new(),
      exports: Vec::new(),
      references: Vec::new(),
      module_decls: Vec::new(),
      diagnostics: Vec::new(),
      analyzer,
      impls: Vec::new(),
      file_doc: None,
    }
  }

  pub fn failed(file: FileRef, analyzer: Producer, reason: impl Into<String>) -> Self {
    Self {
      parse_status: ParseStatus::Failed { reason: reason.into() },
      ..Self::new(file, analyzer)
    }
  }

  pub fn next_local_id(&self) -> LocalSymbolId {
    LocalSymbolId(u32::try_from(self.symbols.len()).unwrap_or(u32::MAX))
  }

  pub fn symbol(&self, local: LocalSymbolId) -> Option<&SymbolFact> {
    self.symbols.get(local.index())
  }

  pub fn children(&self, parent: LocalSymbolId) -> impl Iterator<Item = &SymbolFact> {
    self.symbols.iter().filter(move |symbol| symbol.parent == Some(parent))
  }

  pub fn symbol_keys(&self, context: &KeyContext) -> Result<Vec<SymbolKey>, CoreError> {
    let family = self.file.language.family();
    let mut occurrences: HashMap<(Vec<String>, &[String], &EntityKind), u32> = HashMap::new();
    self
      .symbols
      .iter()
      .map(|symbol| {
        let module_path: Vec<String> = context.module_path.iter().chain(&symbol.module_suffix).cloned().collect();
        let seen = occurrences.entry((module_path.clone(), &symbol.item_path, &symbol.kind)).or_insert(0);
        let disambiguator = *seen;
        *seen += 1;
        SymbolKey::new(
          family,
          self.file.repo.clone(),
          context.package.clone(),
          module_path,
          symbol.item_path.clone(),
          symbol.kind.clone(),
          disambiguator,
        )
      })
      .collect()
  }

  pub fn check_invariants(&self, source_len: usize) -> Result<(), CoreError> {
    let violation = |message: String| Err(CoreError::InvalidFacts(format!("{}: {message}", self.file.path)));
    let source_len = u32::try_from(source_len).unwrap_or(u32::MAX);
    let span_ok = |span: &Span| span.is_well_formed() && span.end_byte <= source_len;
    let known = |local: LocalSymbolId| local.index() < self.symbols.len();
    for (index, symbol) in self.symbols.iter().enumerate() {
      if symbol.local.index() != index {
        return violation(format!("symbol {index} carries local id {}", symbol.local));
      }
      if symbol.parent.is_some_and(|parent| parent.index() >= index) {
        return violation(format!("symbol {index} has a parent that is not declared before it"));
      }
      if symbol.item_path.iter().chain(&symbol.module_suffix).any(String::is_empty) {
        return violation(format!("symbol {index} has an empty path segment"));
      }
      if symbol.item_path.is_empty() && symbol.kind != EntityKind::Module {
        return violation(format!("symbol {index} is not a module but has an empty item path"));
      }
      if !span_ok(&symbol.span) || !span_ok(&symbol.name_span) || !symbol.span.contains(&symbol.name_span) {
        return violation(format!("symbol {index} has a malformed span or a name span outside its span"));
      }
    }
    for reference in &self.references {
      if reference.from.is_some_and(|local| !known(local)) || !span_ok(&reference.span) {
        return violation(format!("reference {:?} points at an unknown symbol or has a malformed span", reference.path));
      }
    }
    for import in &self.imports {
      let scope = match import {
        ImportFact::RustUse { scope, .. } | ImportFact::TsRequire { scope, .. } | ImportFact::TsDynamicImport { scope, .. } => *scope,
        _ => None,
      };
      if scope.is_some_and(|local| !known(local)) || !span_ok(&import.span()) {
        return violation("import has an unknown scope or a malformed span".to_owned());
      }
    }
    for export in &self.exports {
      if !span_ok(&export.span()) {
        return violation("export has a malformed span".to_owned());
      }
    }
    for implementation in &self.impls {
      if implementation.scope.is_some_and(|local| !known(local)) || !span_ok(&implementation.span) || implementation.module_suffix.iter().any(String::is_empty) {
        return violation("impl has an unknown scope, a malformed span or an empty module segment".to_owned());
      }
    }
    for declaration in &self.module_decls {
      if matches!(declaration.body, ModuleBody::Inline(local) if !known(local)) || !span_ok(&declaration.span) {
        return violation(format!("module declaration {} has an unknown body symbol or a malformed span", declaration.name));
      }
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::args::{ObjectEntry, ObjectKey, TemplatePart};

  fn span(start: u32, end: u32) -> Span {
    Span::from_zero_based_rows(start, end, 0, start, 0, end)
  }

  fn file() -> FileRef {
    FileRef::detect(
      RepoName::new("acme-api").unwrap(),
      RelPath::new("src/users/users.controller.ts").unwrap(),
      ContentHash::of(b"source"),
    )
    .unwrap()
  }

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
  }

  fn sample_facts() -> FileFacts {
    let mut facts = FileFacts::new(file(), Producer::new("atlas-lang-ts", 1));
    let mut class = SymbolFact::new(
      LocalSymbolId(0),
      EntityKind::Class,
      "UsersController",
      strings(&["UsersController"]),
      span(0, 90),
      span(13, 28),
      Visibility::Exported,
    );
    class.attributes.push(AttributeFact {
      path: strings(&["Controller"]),
      args: vec![ArgSummary::Str("users".into())],
      span: span(0, 12),
    });
    class.implements.push(strings(&["OnModuleInit"]));
    let mut method = SymbolFact::new(
      LocalSymbolId(1),
      EntityKind::Method,
      "findOne",
      strings(&["UsersController", "findOne"]),
      span(30, 80),
      span(40, 47),
      Visibility::Public,
    );
    method.parent = Some(LocalSymbolId(0));
    method.modifiers = Modifiers::empty().with(Modifier::Async);
    method.signature = Some("async findOne(id: string): Promise<User>".into());
    method.params.push(ParamFact {
      name: "id".into(),
      type_text: Some("string".into()),
      attributes: vec![AttributeFact {
        path: strings(&["Param"]),
        args: vec![ArgSummary::Str("id".into())],
        span: span(48, 59),
      }],
      span: span(48, 70),
    });
    method.body = Some(BodyFingerprintInput {
      normalized_tokens: vec![token_code(NormToken::Identifier), token_code(NormToken::Keyword("return"))],
      token_count: 2,
      line_count: 3,
    });
    facts.symbols = vec![class, method];
    facts.imports.push(ImportFact::TsImport {
      specifier: "@nestjs/common".into(),
      bindings: vec![ImportBinding::Named {
        imported: "Get".into(),
        local: "Get".into(),
        type_only: false,
      }],
      type_only: false,
      span: span(0, 5),
    });
    facts.imports.push(ImportFact::TsDynamicImport {
      specifier: ArgSummary::Path(strings(&["modulePath"])),
      scope: Some(LocalSymbolId(1)),
      span: span(60, 70),
    });
    facts.exports.push(ExportFact::ReExportAll {
      specifier: "./dto".into(),
      namespace: None,
      type_only: false,
      span: span(85, 90),
    });
    facts.references.push(ReferenceFact {
      from: Some(LocalSymbolId(1)),
      kind: RefKind::MethodCall,
      path: strings(&["this", "http", "get"]),
      receiver: Some(ReceiverHint::This),
      args: vec![
        ArgSummary::Template(vec![TemplatePart::Expr(ArgSummary::Path(strings(&["BASE"]))), TemplatePart::Lit("/users".into())]),
        ArgSummary::Object(vec![ObjectEntry {
          key: ObjectKey::Ident("headers".into()),
          value: ArgSummary::Object(vec![]),
        }]),
      ],
      span: span(50, 75),
      origin: RefOrigin::Syntax,
    });
    facts
  }

  #[test]
  fn should_omit_empty_impls_file_doc_and_syntax_origin_from_serialized_output() {
    let facts = sample_facts();
    let json = serde_json::to_value(&facts).unwrap();
    let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
      keys,
      [
        "analyzer",
        "diagnostics",
        "exports",
        "file",
        "imports",
        "module_decls",
        "parse_status",
        "references",
        "symbols"
      ]
    );
    let reference = serde_json::to_value(&facts.references[0]).unwrap();
    assert!(reference.get("origin").is_none());
    let legacy = serde_json::to_string(&facts).unwrap();
    assert_eq!(serde_json::from_str::<FileFacts>(&legacy).unwrap(), facts);
  }

  #[test]
  fn should_serialise_declared_receivers_beside_the_existing_hint_shapes() {
    let declared = ReceiverHint::Declared {
      type_text: "Box<dyn Store>".into(),
      binding: DeclaredBinding::Param,
    };
    let json = serde_json::to_value(&declared).unwrap();
    assert_eq!(
      json,
      serde_json::json!({ "type": "declared", "value": { "type_text": "Box<dyn Store>", "binding": "param" } })
    );
    assert_eq!(serde_json::from_value::<ReceiverHint>(json).unwrap(), declared);
    assert_eq!(
      serde_json::to_value(ReceiverHint::TypeHint(strings(&["MemStore"]))).unwrap(),
      serde_json::json!({ "type": "type_hint", "value": ["MemStore"] })
    );
  }

  #[test]
  fn should_serialise_receiver_chains_with_call_heads_and_try_steps() {
    let chain = ReceiverHint::Chain(Box::new(ReceiverChain {
      head: ChainHead::Call {
        path: strings(&["lock"]),
        args: vec![Some("&Mutex<Budget>".into()), None],
      },
      steps: vec![ChainStep::Method("tx".into()), ChainStep::Try, ChainStep::Field("inner".into())],
    }));
    let json = serde_json::to_value(&chain).unwrap();
    assert_eq!(
      json,
      serde_json::json!({ "type": "chain", "value": {
        "head": { "type": "call", "value": { "path": ["lock"], "args": ["&Mutex<Budget>", null] } },
        "steps": [{ "type": "method", "value": "tx" }, { "type": "try" }, { "type": "field", "value": "inner" }]
      } })
    );
    assert_eq!(serde_json::from_value::<ReceiverHint>(json).unwrap(), chain);
    let binding = ReceiverHint::Chain(Box::new(ReceiverChain {
      head: ChainHead::Binding {
        name: "graph".into(),
        hint: ReceiverHint::SelfValue,
      },
      steps: vec![ChainStep::Await],
    }));
    let json = serde_json::to_string(&binding).unwrap();
    assert_eq!(serde_json::from_str::<ReceiverHint>(&json).unwrap(), binding);
  }

  #[test]
  fn should_round_trip_impls_file_docs_and_macro_scan_origins() {
    let mut facts = sample_facts();
    facts.file_doc = Some("Users API.".into());
    facts.references[0].origin = RefOrigin::MacroTokenScan;
    facts.impls.push(ImplFact {
      self_type: TypePath::new(strings(&["UsersController"])),
      trait_ref: Some(TypePath::new(strings(&["std", "error", "Error"]))),
      generic_params: strings(&["T"]),
      module_suffix: strings(&["inner"]),
      scope: Some(LocalSymbolId(1)),
      attributes: vec![],
      span: span(0, 10),
    });
    facts.check_invariants(100).unwrap();
    let json = serde_json::to_value(&facts).unwrap();
    assert_eq!(json["file_doc"], "Users API.");
    assert_eq!(json["references"][0]["origin"], "macro_token_scan");
    assert_eq!(serde_json::from_value::<FileFacts>(json).unwrap(), facts);
    facts.impls[0].scope = Some(LocalSymbolId(7));
    assert!(facts.check_invariants(100).is_err());
  }

  #[test]
  fn should_detect_file_language_and_declaration_flag() {
    let repo = RepoName::new("acme").unwrap();
    let hash = ContentHash::of(b"");
    let declaration = FileRef::detect(repo.clone(), RelPath::new("types/index.d.ts").unwrap(), hash).unwrap();
    assert_eq!((declaration.language, declaration.is_declaration_file), (Language::TypeScript, true));
    let rust = FileRef::detect(repo.clone(), RelPath::new("src/lib.rs").unwrap(), hash).unwrap();
    assert_eq!((rust.language, rust.is_declaration_file), (Language::Rust, false));
    assert!(matches!(
      FileRef::detect(repo, RelPath::new("README.md").unwrap(), hash),
      Err(CoreError::UnsupportedPath(_))
    ));
  }

  #[test]
  fn should_round_trip_file_facts_through_json() {
    let mut facts = sample_facts();
    facts.parse_status = ParseStatus::Partial {
      error_count: 3,
      first_errors: vec![ParseErrorFact {
        span: span(5, 6),
        message: "unexpected token".into(),
      }],
    };
    facts.diagnostics.push(Diagnostic {
      severity: Severity::Warning,
      code: "ts.unsupported".into(),
      message: "decorator metadata".into(),
      span: None,
    });
    let json = serde_json::to_string(&facts).unwrap();
    assert_eq!(serde_json::from_str::<FileFacts>(&json).unwrap(), facts);
  }

  #[test]
  fn should_round_trip_rust_specific_facts_through_json() {
    let file = FileRef::detect(RepoName::new("atlas").unwrap(), RelPath::new("src/lib.rs").unwrap(), ContentHash::of(b"x")).unwrap();
    let mut facts = FileFacts::new(file, Producer::new("atlas-lang-rust", 1));
    let mut module = SymbolFact::new(LocalSymbolId(0), EntityKind::Module, "tests", vec![], span(0, 50), span(4, 9), Visibility::Private);
    module.module_suffix = strings(&["tests"]);
    let mut method = SymbolFact::new(
      LocalSymbolId(1),
      EntityKind::Method,
      "fmt",
      strings(&["Error", "<Display>", "fmt"]),
      span(10, 40),
      span(13, 16),
      Visibility::Private,
    );
    method.impl_of = Some(ImplInfo {
      self_type: TypePath::new(strings(&["Error"])),
      trait_ref: Some(TypePath::new(strings(&["std", "fmt", "Display"]))),
    });
    method.test = Some(TestInfo {
      framework: "rust".into(),
      kind: TestKind::Unit,
      display_name: None,
    });
    facts.symbols = vec![module, method];
    facts.imports.push(ImportFact::RustUse {
      paths: vec![
        UsePath {
          segments: strings(&["crate", "store", "open"]),
          binding: UseBinding::Name,
        },
        UsePath {
          segments: strings(&["std", "io", "Result"]),
          binding: UseBinding::Alias("IoResult".into()),
        },
        UsePath {
          segments: strings(&["super"]),
          binding: UseBinding::Glob,
        },
      ],
      visibility: Visibility::Restricted("super".into()),
      scope: Some(LocalSymbolId(0)),
      span: span(20, 30),
    });
    facts.imports.push(ImportFact::RustExternCrate {
      name: "serde".into(),
      alias: None,
      span: span(0, 3),
    });
    facts.module_decls.push(ModuleDeclFact {
      name: "tests".into(),
      body: ModuleBody::Inline(LocalSymbolId(0)),
      path_attr: Some("tests_impl.rs".into()),
      module_suffix: vec![],
      visibility: Visibility::Private,
      attributes: vec![AttributeFact {
        path: strings(&["cfg"]),
        args: vec![ArgSummary::Path(strings(&["test"]))],
        span: span(0, 3),
      }],
      span: span(0, 50),
    });
    facts.check_invariants(64).unwrap();
    let json = serde_json::to_string(&facts).unwrap();
    assert_eq!(serde_json::from_str::<FileFacts>(&json).unwrap(), facts);
    assert!(facts.imports[0].is_re_export());
  }

  #[test]
  fn should_build_symbol_keys_with_inline_modules_and_ordered_disambiguators() {
    let file = FileRef::detect(RepoName::new("atlas").unwrap(), RelPath::new("src/graph.rs").unwrap(), ContentHash::of(b"x")).unwrap();
    let mut facts = FileFacts::new(file, Producer::new("atlas-lang-rust", 1));
    let mut tests_module = SymbolFact::new(LocalSymbolId(0), EntityKind::Module, "tests", vec![], span(0, 90), span(4, 9), Visibility::Private);
    tests_module.module_suffix = strings(&["tests"]);
    let mut helper = SymbolFact::new(
      LocalSymbolId(1),
      EntityKind::Function,
      "helper",
      strings(&["helper"]),
      span(10, 20),
      span(13, 19),
      Visibility::Private,
    );
    helper.parent = Some(LocalSymbolId(0));
    helper.module_suffix = strings(&["tests"]);
    let first = SymbolFact::new(
      LocalSymbolId(2),
      EntityKind::Function,
      "imp",
      strings(&["imp"]),
      span(30, 40),
      span(33, 36),
      Visibility::Private,
    );
    let second = SymbolFact::new(
      LocalSymbolId(3),
      EntityKind::Function,
      "imp",
      strings(&["imp"]),
      span(50, 60),
      span(53, 56),
      Visibility::Private,
    );
    let constant = SymbolFact::new(
      LocalSymbolId(4),
      EntityKind::Constant,
      "imp",
      strings(&["imp"]),
      span(70, 80),
      span(73, 76),
      Visibility::Private,
    );
    facts.symbols = vec![tests_module, helper, first, second, constant];
    facts.check_invariants(100).unwrap();
    let context = KeyContext {
      package: "atlas-store".into(),
      module_path: strings(&["graph"]),
    };
    let keys: Vec<String> = facts.symbol_keys(&context).unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(
      keys,
      [
        "rust:atlas:atlas-store:graph/tests::#module",
        "rust:atlas:atlas-store:graph/tests::helper#function",
        "rust:atlas:atlas-store:graph::imp#function",
        "rust:atlas:atlas-store:graph::imp#function~1",
        "rust:atlas:atlas-store:graph::imp#constant",
      ]
    );
  }

  #[test]
  fn should_reject_non_module_symbols_without_an_item_path() {
    let mut facts = sample_facts();
    facts.symbols[1].item_path.clear();
    assert!(facts.check_invariants(100).is_err());
  }

  #[test]
  fn should_accept_well_formed_facts() {
    let facts = sample_facts();
    facts.check_invariants(100).unwrap();
    assert_eq!(facts.children(LocalSymbolId(0)).map(|symbol| symbol.name.as_str()).collect::<Vec<_>>(), ["findOne"]);
    assert_eq!(facts.next_local_id(), LocalSymbolId(2));
    assert_eq!(facts.symbol(LocalSymbolId(1)).map(|symbol| symbol.name.as_str()), Some("findOne"));
  }

  #[test]
  fn should_reject_facts_that_break_invariants() {
    let mut out_of_order = sample_facts();
    out_of_order.symbols.swap(0, 1);
    assert!(out_of_order.check_invariants(100).is_err());

    let mut forward_parent = sample_facts();
    forward_parent.symbols[0].parent = Some(LocalSymbolId(1));
    assert!(forward_parent.check_invariants(100).is_err());

    let mut dangling_reference = sample_facts();
    dangling_reference.references[0].from = Some(LocalSymbolId(9));
    assert!(dangling_reference.check_invariants(100).is_err());

    let mut escaping_name = sample_facts();
    escaping_name.symbols[1].name_span = span(10, 20);
    assert!(escaping_name.check_invariants(100).is_err());

    let mut empty_segment = sample_facts();
    empty_segment.symbols[1].item_path.push(String::new());
    assert!(empty_segment.check_invariants(100).is_err());

    assert!(sample_facts().check_invariants(50).is_err());
  }

  #[test]
  fn should_store_modifiers_as_a_set_and_serialise_them_as_names() {
    let modifiers: Modifiers = [Modifier::Static, Modifier::Async, Modifier::Async].into_iter().collect();
    assert!(modifiers.contains(Modifier::Async));
    assert!(!modifiers.contains(Modifier::Const));
    assert_eq!(modifiers.iter().collect::<Vec<_>>(), [Modifier::Async, Modifier::Static]);
    assert_eq!(serde_json::to_string(&modifiers).unwrap(), r#"["async","static"]"#);
    assert_eq!(serde_json::from_str::<Modifiers>(r#"["static","async"]"#).unwrap(), modifiers);
    assert!(Modifiers::empty().is_empty());
  }

  #[test]
  fn should_build_impl_item_prefixes_that_separate_inherent_and_trait_methods() {
    let inherent = ImplInfo {
      self_type: TypePath::new(strings(&["store", "GraphStore"])),
      trait_ref: None,
    };
    assert_eq!(inherent.item_prefix(), ["GraphStore"]);
    let display = ImplInfo {
      self_type: TypePath::new(strings(&["GraphStore"])),
      trait_ref: Some(TypePath::new(strings(&["std", "fmt", "Display"]))),
    };
    assert_eq!(display.item_prefix(), ["GraphStore", "<Display>"]);
    let from_io = ImplInfo {
      self_type: TypePath::new(strings(&["AppError"])),
      trait_ref: Some(TypePath {
        segments: strings(&["From"]),
        generic_args: Some("io::Error".into()),
      }),
    };
    assert_eq!(from_io.item_prefix(), ["AppError", "<From<io::Error>>"]);
  }

  #[test]
  fn should_normalise_tokens_into_stable_codes() {
    assert_eq!(token_code(NormToken::Identifier), 1);
    assert_eq!(token_code(NormToken::StringLiteral), 2);
    assert_eq!(token_code(NormToken::NumberLiteral), 3);
    let keyword = token_code(NormToken::Keyword("return"));
    assert_eq!(keyword, token_code(NormToken::Keyword("return")));
    assert_ne!(keyword, token_code(NormToken::Punctuation("return")));
    assert_ne!(token_code(NormToken::Punctuation("+")), token_code(NormToken::Punctuation("-")));
    assert!(keyword >= 16);
    assert_eq!(token_code(NormToken::Punctuation("{")), 1_447_977_468);
  }

  #[test]
  fn should_classify_visibility_for_external_use() {
    assert!(Visibility::Public.is_externally_visible());
    assert!(Visibility::Exported.is_externally_visible());
    assert!(Visibility::ExportedDefault.is_externally_visible());
    assert!(!Visibility::Crate.is_externally_visible());
    assert!(!Visibility::Module.is_externally_visible());
    assert!(!Visibility::Restricted("super".into()).is_externally_visible());
  }
}
