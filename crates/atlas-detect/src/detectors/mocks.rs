use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{
  ATTR_MOCK, ATTR_MOCK_TYPE, ArgSummary, EdgeKind, EntityKey, EntityKind, FileFacts, ImportBinding, ImportFact, LanguageFamily, LocalSymbolId, NodeRef, RefKind, ReferenceFact,
  RelPath, Severity, Span, SymbolFact,
};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::lex::{Token, TokenKind};
use crate::sink::{Sink, attrs};
use crate::view::{FileView, Imported};

pub struct MockDetector;

detector!(MockDetector, "test-mocks", 2);

const MOCK_LIBRARIES: [&str; 2] = ["jest", "vi"];
const MOCK_WRAPPERS: [&[&str]; 9] = [
  &["jest", "Mocked"],
  &["jest", "MockedObject"],
  &["jest", "MockedObjectDeep"],
  &["Mocked"],
  &["MockedObject"],
  &["DeepMocked"],
  &["MockProxy"],
  &["DeepMockProxy"],
  &["Partial"],
];
const MOCK_FACTORIES: [&str; 3] = ["createMock", "mock", "mockDeep"];
const CALL_MATCHERS: [&str; 9] = [
  "toHaveBeenCalled",
  "toHaveBeenLastCalledWith",
  "toHaveBeenNthCalledWith",
  "toBeCalled",
  "lastCalledWith",
  "nthCalledWith",
  "toHaveReturned",
  "toHaveLastReturnedWith",
  "toHaveNthReturnedWith",
];
const REPLACING_SETUPS: [&str; 5] = ["mockImplementation", "mockReturnValue", "mockResolvedValue", "mockRejectedValue", "mockReturnThis"];
const DECLARATION_KEYWORDS: [&str; 3] = ["const", "let", "var"];
const ATTR_MOCK_SITE: &str = "mock_site";
const ERASING_CASTS: [&str; 2] = ["unknown", "any"];
const PROTOTYPE: &str = "prototype";
const BUN_TEST: &str = "bun:test";
const MOCK_MODULE_RULE: &str = "bun-mock-module";
const DYNAMIC_MOCK_MODULE: &str = "mock.dynamic_module";
const SOURCE_EXTENSIONS: [&str; 4] = [".ts", ".tsx", ".js", ".jsx"];
const INDEX_EXTENSIONS: [&str; 3] = ["/index.ts", "/index.tsx", "/index.js"];
const RUNTIME_EXTENSIONS: [(&str, &[&str]); 8] = [
  (".ts", &[".ts"]),
  (".tsx", &[".tsx"]),
  (".mts", &[".mts"]),
  (".cts", &[".cts"]),
  (".js", &[".js", ".ts", ".tsx"]),
  (".jsx", &[".jsx", ".tsx"]),
  (".mjs", &[".mjs", ".mts"]),
  (".cjs", &[".cjs", ".cts"]),
];
const NON_EXPORT_KEYS: [&str; 2] = ["default", "__esModule"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MockSite {
  FnProperty,
  MockSetup,
  CallAssertion,
  SpyOn,
  Module,
}

impl MockSite {
  fn as_str(self) -> &'static str {
    match self {
      Self::FnProperty => "fn_property",
      Self::MockSetup => "mock_setup",
      Self::CallAssertion => "call_assertion",
      Self::SpyOn => "spy_on",
      Self::Module => "module",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacement {
  Replaced,
  Spied,
}

impl Replacement {
  fn as_str(self) -> &'static str {
    match self {
      Self::Replaced => "replaced",
      Self::Spied => "spied",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TypeRule {
  Cast,
  Provider,
  MockedType,
  CreateMock,
  Annotation,
  Instantiation,
  Prototype,
  Class,
  Naming,
}

impl TypeRule {
  fn as_str(self) -> &'static str {
    match self {
      Self::Cast => "cast",
      Self::Provider => "provider",
      Self::MockedType => "mocked_type",
      Self::CreateMock => "create_mock",
      Self::Annotation => "annotation",
      Self::Instantiation => "instantiation",
      Self::Prototype => "prototype",
      Self::Class => "class",
      Self::Naming => "naming",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockedType {
  path: Vec<String>,
  rule: TypeRule,
}

impl MockedType {
  fn new(path: Vec<String>, rule: TypeRule) -> Self {
    Self { path, rule }
  }

  fn replaces_members(&self) -> bool {
    matches!(self.rule, TypeRule::MockedType | TypeRule::CreateMock)
  }

  fn member_path(&self, member: &str) -> Vec<String> {
    let mut path = self.path.clone();
    if self.rule != TypeRule::Class {
      path.push(PROTOTYPE.to_owned());
    }
    path.push(member.to_owned());
    path
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeExpr {
  path: Vec<String>,
  args: Vec<TypeExpr>,
}

impl TypeExpr {
  fn unwrap_mocked(self) -> (Vec<String>, bool) {
    let mut current = self;
    let mut wrapped = false;
    while current.args.len() == 1 && MOCK_WRAPPERS.iter().any(|wrapper| current.path.iter().map(String::as_str).eq(wrapper.iter().copied())) {
      current = current.args.remove(0);
      wrapped = true;
    }
    (current.path, wrapped)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleTarget {
  Export,
  File,
}

impl ModuleTarget {
  fn as_str(self) -> &'static str {
    match self {
      Self::Export => "module_export",
      Self::File => "module_file",
    }
  }
}

#[derive(Debug, Default)]
struct BunTest {
  mocks: Vec<String>,
  spies: Vec<String>,
  namespaces: Vec<String>,
}

impl BunTest {
  fn of(facts: &FileFacts) -> Self {
    let mut bun = Self::default();
    for bindings in facts.imports.iter().filter_map(bun_bindings) {
      for binding in bindings {
        match binding {
          ImportBinding::Named { imported, local, .. } if imported == "mock" => bun.mocks.push(local.clone()),
          ImportBinding::Named { imported, local, .. } if imported == "spyOn" => bun.spies.push(local.clone()),
          ImportBinding::Namespace { local } => bun.namespaces.push(local.clone()),
          _ => {}
        }
      }
    }
    bun
  }

  fn is_imported(facts: &FileFacts) -> bool {
    facts.imports.iter().any(|import| bun_bindings(import).is_some())
  }

  fn binds(&self, name: &String) -> bool {
    self.mocks.contains(name) || self.spies.contains(name) || self.namespaces.contains(name)
  }

  fn is_mock(&self, path: &[String]) -> bool {
    match path {
      [name] => self.mocks.contains(name),
      [namespace, function] => self.namespaces.contains(namespace) && function == "mock",
      _ => false,
    }
  }

  fn is_mock_module(&self, path: &[String]) -> bool {
    match path {
      [mock @ .., module] if module == "module" => self.is_mock(mock),
      _ => false,
    }
  }

  fn is_spy(&self, path: &[String]) -> bool {
    match path {
      [name] => self.spies.contains(name),
      [namespace, function] => self.namespaces.contains(namespace) && function == "spyOn",
      _ => false,
    }
  }

  fn spy_head_prefixes(&self) -> Vec<String> {
    let jest = MOCK_LIBRARIES.iter().map(|library| format!("({library}.spyOn("));
    let bare = self.spies.iter().map(|spy| format!("({spy}("));
    let namespaced = self.namespaces.iter().map(|namespace| format!("({namespace}.spyOn("));
    jest.chain(bare).chain(namespaced).collect()
  }
}

fn bun_bindings(import: &ImportFact) -> Option<&[ImportBinding]> {
  match import {
    ImportFact::TsImport { specifier, bindings, .. } if specifier == BUN_TEST => Some(bindings),
    ImportFact::TsRequire {
      specifier: ArgSummary::Str(specifier),
      bindings,
      ..
    } if specifier == BUN_TEST => Some(bindings),
    _ => None,
  }
}

enum Evidence {
  Typed(MockedType),
  Conflicting,
  Absent,
}

#[derive(Debug, Clone, Copy)]
struct Declaration {
  keyword: usize,
  name: usize,
  owner: Option<LocalSymbolId>,
}

enum MockUse<'r> {
  Member {
    head: &'r str,
    member: &'r str,
    site: MockSite,
    replacement: Replacement,
  },
  Spy {
    target: &'r [String],
    member: &'r str,
    replacement: Replacement,
  },
}

struct Owners {
  starts: Vec<u32>,
  owners: Vec<Option<LocalSymbolId>>,
}

impl Owners {
  fn new(symbols: &[SymbolFact]) -> Self {
    let mut sorted: Vec<&SymbolFact> = symbols.iter().collect();
    sorted.sort_by_key(|symbol| (symbol.span.start_byte, Reverse(symbol.span.end_byte)));
    let mut owners = Self {
      starts: vec![0],
      owners: vec![None],
    };
    let mut open: Vec<(u32, LocalSymbolId)> = Vec::new();
    for symbol in sorted {
      owners.close_until(&mut open, symbol.span.start_byte);
      open.push((symbol.span.end_byte, symbol.local));
      owners.push(symbol.span.start_byte, Some(symbol.local));
    }
    owners.close_until(&mut open, u32::MAX);
    owners
  }

  fn close_until(&mut self, open: &mut Vec<(u32, LocalSymbolId)>, position: u32) {
    while let Some(&(end, _)) = open.last()
      && end <= position
    {
      open.pop();
      self.push(end, open.last().map(|(_, local)| *local));
    }
  }

  fn push(&mut self, start: u32, owner: Option<LocalSymbolId>) {
    let start = start.max(self.starts.last().copied().unwrap_or(0));
    self.starts.push(start);
    self.owners.push(owner);
  }

  fn at(&self, byte: u32) -> Option<LocalSymbolId> {
    let index = self.starts.partition_point(|start| *start <= byte);
    index.checked_sub(1).and_then(|index| self.owners[index])
  }
}

struct Brackets {
  enclosing: Vec<Option<usize>>,
  closing: Vec<Option<usize>>,
}

impl Brackets {
  fn new(tokens: &[Token]) -> Self {
    let mut enclosing = Vec::with_capacity(tokens.len());
    let mut closing = vec![None; tokens.len()];
    let mut open: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
      let is_punct = token.kind == TokenKind::Punct;
      if is_punct && matches!(token.text.as_str(), "}" | ")" | "]") {
        if let Some(opened) = open.pop() {
          closing[opened] = Some(index);
        }
        enclosing.push(open.last().copied());
        continue;
      }
      enclosing.push(open.last().copied());
      if is_punct && matches!(token.text.as_str(), "{" | "(" | "[") {
        open.push(index);
      }
    }
    Self { enclosing, closing }
  }

  fn enclosing_brace(&self, tokens: &[Token], index: usize) -> Option<usize> {
    self.enclosing.get(index).copied().flatten().filter(|open| tokens[*open].is_punct("{"))
  }

  fn close_of(&self, open: usize) -> Option<usize> {
    self.closing.get(open).copied().flatten()
  }
}

type BindingCache = BTreeMap<(String, Option<usize>), Option<MockedType>>;

struct Scan<'v, 'a> {
  view: &'v FileView<'a>,
  tokens: &'v [Token],
  owners: Owners,
  brackets: Brackets,
  declarations: BTreeMap<&'v str, Vec<Declaration>>,
  cache: BindingCache,
  bun: BunTest,
}

impl MockDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.file.language.family() == LanguageFamily::Ts
      && facts.references.iter().any(|reference| match reference.path.as_slice() {
        [library, function] if MOCK_LIBRARIES.contains(&library.as_str()) && matches!(function.as_str(), "fn" | "spyOn") => true,
        [.., last] => is_mock_setup(last) || is_call_matcher(last),
        [] => false,
      })
      || BunTest::is_imported(facts)
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    let tokens = view.tokens();
    let owners = Owners::new(&view.facts.symbols);
    let declarations = declarations(tokens, &owners);
    let mut scan = Scan {
      view,
      tokens,
      owners,
      brackets: Brackets::new(tokens),
      declarations,
      cache: BindingCache::new(),
      bun: BunTest::of(view.facts),
    };
    scan.fn_properties(sink);
    let chains = Chains::of(&view.facts.references, &scan.bun);
    for reference in &view.facts.references {
      if scan.shadows_bun_binding(reference) {
        continue;
      }
      if scan.bun.is_mock_module(&reference.path) && view.is_test_context(reference.from) {
        scan.mock_module(sink, reference);
        continue;
      }
      let Some(usage) = mock_use(reference, &chains, &scan.bun) else { continue };
      if !view.is_test_context(reference.from) {
        continue;
      }
      let (mocked, member, span, site, replacement) = match usage {
        MockUse::Member { head, member, site, replacement } => (
          scan.binding_type(head, reference.span.start_byte, reference.from),
          member,
          scan.member_span(reference.span, head, member),
          site,
          replacement,
        ),
        MockUse::Spy { target, member, replacement } => (
          scan.spied_type(target, reference),
          member,
          scan.string_span(reference.span, member),
          MockSite::SpyOn,
          replacement,
        ),
      };
      let Some(mocked) = mocked else { continue };
      let replacement = if mocked.replaces_members() { Replacement::Replaced } else { replacement };
      let edge = MockEdge {
        scope: reference.from,
        member,
        span,
        site,
        replacement,
      };
      emit(view, sink, &mocked, edge);
    }
  }
}

struct Chains {
  matchers: BTreeSet<u32>,
  replaced_spies: BTreeSet<u32>,
}

impl Chains {
  fn of(references: &[ReferenceFact], bun: &BunTest) -> Self {
    let spy_heads = bun.spy_head_prefixes();
    let mut chains = Self {
      matchers: BTreeSet::new(),
      replaced_spies: BTreeSet::new(),
    };
    for reference in references.iter().filter(|reference| reference.kind == RefKind::MethodCall) {
      let (Some(head), Some(name)) = (reference.path.first(), reference.name()) else {
        continue;
      };
      if head.starts_with("(expect(") && is_call_matcher(name) {
        chains.matchers.insert(reference.span.start_byte);
      }
      if spy_heads.iter().any(|spy_head| head.starts_with(spy_head)) && is_replacing_setup(name) {
        chains.replaced_spies.insert(reference.span.start_byte);
      }
    }
    chains
  }
}

fn mock_use<'r>(reference: &'r ReferenceFact, chains: &Chains, bun: &BunTest) -> Option<MockUse<'r>> {
  match (reference.kind, reference.path.as_slice(), reference.args.as_slice()) {
    (RefKind::MethodCall, [head, member, setup], _) if is_mock_setup(setup) && is_plain_name(head) && is_plain_name(member) => Some(MockUse::Member {
      head,
      member,
      site: MockSite::MockSetup,
      replacement: if is_replacing_setup(setup) { Replacement::Replaced } else { Replacement::Spied },
    }),
    (RefKind::Call, [expect], [ArgSummary::Path(path)]) if expect == "expect" && chains.matchers.contains(&reference.span.start_byte) => match path.as_slice() {
      [head, member] if is_plain_name(head) && is_plain_name(member) => Some(MockUse::Member {
        head,
        member,
        site: MockSite::CallAssertion,
        replacement: Replacement::Spied,
      }),
      _ => None,
    },
    (RefKind::MethodCall | RefKind::Call, path, [ArgSummary::Path(target), ArgSummary::Str(member), ..]) if is_spy_callee(reference.kind, path, bun) => Some(MockUse::Spy {
      target,
      member,
      replacement: match chains.replaced_spies.contains(&reference.span.start_byte) {
        true => Replacement::Replaced,
        false => Replacement::Spied,
      },
    }),
    _ => None,
  }
}

fn is_spy_callee(kind: RefKind, path: &[String], bun: &BunTest) -> bool {
  match (kind, path) {
    (RefKind::MethodCall, [library, spy]) if MOCK_LIBRARIES.contains(&library.as_str()) && spy == "spyOn" => true,
    _ => bun.is_spy(path),
  }
}

fn declarations<'t>(tokens: &'t [Token], owners: &Owners) -> BTreeMap<&'t str, Vec<Declaration>> {
  let mut declarations: BTreeMap<&str, Vec<Declaration>> = BTreeMap::new();
  for (keyword, pair) in tokens.windows(2).enumerate() {
    if pair[0].kind == TokenKind::Ident && DECLARATION_KEYWORDS.contains(&pair[0].text.as_str()) && pair[1].kind == TokenKind::Ident {
      declarations.entry(pair[1].text.as_str()).or_default().push(Declaration {
        keyword,
        name: keyword + 1,
        owner: owners.at(pair[0].start),
      });
    }
  }
  declarations
}

impl Scan<'_, '_> {
  fn fn_properties(&mut self, sink: &mut Sink) {
    for index in 1..self.tokens.len() {
      let Some(member) = self.mock_fn_property(index) else { continue };
      let key = &self.tokens[index];
      let scope = self.owners.at(key.start);
      if !self.view.is_test_context(scope) {
        continue;
      }
      let Some(open) = self.brackets.enclosing_brace(self.tokens, index) else { continue };
      let Some(mocked) = self.object_type(open, scope) else { continue };
      let Some(span) = inner_string_span(self.view, key) else { continue };
      let edge = MockEdge {
        scope,
        member: &member,
        span,
        site: MockSite::FnProperty,
        replacement: Replacement::Replaced,
      };
      emit(self.view, sink, &mocked, edge);
    }
  }

  fn shadows_bun_binding(&self, reference: &ReferenceFact) -> bool {
    let Some(head) = reference.path.first() else { return false };
    self.bun.binds(head) && self.is_shadowed(head, reference.span.start_byte)
  }

  fn mock_module(&self, sink: &mut Sink, reference: &ReferenceFact) {
    let tokens = self.view.span_tokens(reference.span);
    let Some(open) = tokens.iter().position(|token| token.is_punct("(")) else { return };
    let Some(specifier) = tokens.get(open + 1).filter(|token| token.kind == TokenKind::Str) else {
      sink.diagnostic(
        Severity::Info,
        DYNAMIC_MOCK_MODULE,
        "mock.module call with a non-literal specifier was not analysed".to_owned(),
        Some(reference.span),
      );
      return;
    };
    let specifier_span = inner_string_span(self.view, specifier).unwrap_or(reference.span);
    let scope = reference.from;
    for path in relative_module_files(&self.view.facts.file.path, &specifier.text) {
      let key = EntityKey::File {
        repo: self.view.repo().clone(),
        path,
      };
      self.module_edge(sink, scope, NodeRef::Key(key), specifier_span, ModuleTarget::File);
    }
    for export in factory_exports(&tokens[open + 2..]) {
      let Some(span) = inner_string_span(self.view, export) else { continue };
      let target = NodeRef::Unresolved {
        path: vec![export.text.clone()],
        from: scope,
      };
      self.module_edge(sink, scope, target, span, ModuleTarget::Export);
    }
  }

  fn module_edge(&self, sink: &mut Sink, scope: Option<LocalSymbolId>, to: NodeRef, span: Span, target: ModuleTarget) {
    sink.edge(
      EdgeKind::References,
      self.view.node(scope),
      to,
      span,
      Cert::heuristic(0.6, MOCK_MODULE_RULE),
      attrs([
        (ATTR_MOCK, json!(Replacement::Replaced.as_str())),
        (ATTR_MOCK_TYPE, json!(target.as_str())),
        (ATTR_MOCK_SITE, json!(MockSite::Module.as_str())),
      ]),
    );
  }

  fn mock_fn_property(&self, index: usize) -> Option<String> {
    let tokens = self.tokens;
    let key = &tokens[index];
    if !matches!(key.kind, TokenKind::Ident | TokenKind::Str) || !tokens.get(index - 1).is_some_and(|before| before.is_punct("{") || before.is_punct(",")) {
      return None;
    }
    let (colon, callee) = tokens.get(index + 1..)?.split_first()?;
    let shadowed = callee.first().is_some_and(|name| self.bun.binds(&name.text) && self.is_shadowed(&name.text, name.start));
    (colon.is_punct(":") && !shadowed && self.is_mock_fn_call(callee))
      .then(|| key.text.clone())
      .filter(|name| is_plain_name(name))
  }

  fn is_mock_fn_call(&self, callee: &[Token]) -> bool {
    match callee {
      [library, dot, function, open, ..] if library.kind == TokenKind::Ident && dot.is_punct(".") && function.kind == TokenKind::Ident && open.is_punct("(") => {
        let path = [library.text.clone(), function.text.clone()];
        (MOCK_LIBRARIES.contains(&library.text.as_str()) && function.text == "fn") || self.bun.is_mock(&path)
      }
      [name, open, ..] if name.kind == TokenKind::Ident && open.is_punct("(") => self.bun.is_mock(std::slice::from_ref(&name.text)),
      _ => false,
    }
  }

  fn object_type(&mut self, open: usize, scope: Option<LocalSymbolId>) -> Option<MockedType> {
    let close = self.brackets.close_of(open)?;
    if let Some(path) = self.cast_at(close + 1) {
      return Some(MockedType::new(path, TypeRule::Cast));
    }
    let before = open.checked_sub(1).and_then(|index| self.tokens.get(index))?;
    if before.is_punct(":") && open >= 2 && self.tokens[open - 2].is_ident("useValue") {
      return self.provided_type(open - 2).map(|path| MockedType::new(path, TypeRule::Provider));
    }
    if !before.is_punct("=") {
      return None;
    }
    let name = assigned_name(self.tokens, open - 1)?;
    self.binding_type(&name, self.tokens[open].start, scope)
  }

  fn binding_type(&mut self, name: &str, position: u32, scope: Option<LocalSymbolId>) -> Option<MockedType> {
    let declaration = self.visible_declaration(name, position, scope);
    let key = (name.to_owned(), declaration.map(|declaration| declaration.keyword));
    if let Some(cached) = self.cache.get(&key) {
      return cached.clone();
    }
    let declared = match &declaration {
      Some(declaration) => self.declared_type(name, declaration),
      None if self.known_type(&[name.to_owned()]) => Evidence::Typed(MockedType::new(vec![name.to_owned()], TypeRule::Class)),
      None => Evidence::Absent,
    };
    let inferred = match declared {
      Evidence::Typed(mocked) => Some(mocked),
      Evidence::Conflicting => None,
      Evidence::Absent => self.named_type(name),
    };
    self.cache.insert(key, inferred.clone());
    inferred
  }

  fn declared_type(&self, name: &str, declaration: &Declaration) -> Evidence {
    let tokens = self.tokens;
    let mut cursor = declaration.name + 1;
    if tokens.get(cursor).is_some_and(|token| token.is_punct(":")) {
      let Some((annotation, next)) = parse_type(tokens, cursor + 1) else {
        return Evidence::Absent;
      };
      let (path, wrapped) = annotation.unwrap_mocked();
      if self.known_type(&path) {
        return Evidence::Typed(MockedType::new(path, if wrapped { TypeRule::MockedType } else { TypeRule::Annotation }));
      }
      cursor = next;
    }
    let mut evidence: Vec<MockedType> = Vec::new();
    if tokens.get(cursor).is_some_and(|token| token.is_punct("=")) {
      evidence.extend(self.initializer_type(cursor + 1));
    }
    let (start, end) = self.region(declaration);
    for index in start..end {
      let token = &tokens[index];
      let before = index.checked_sub(1).and_then(|before| tokens.get(before));
      let is_other_binding = before.is_some_and(|before| before.is_punct(".") || before.is_punct("?.") || DECLARATION_KEYWORDS.iter().any(|keyword| before.is_ident(keyword)));
      if !token.is_ident(name) || index == declaration.name || is_other_binding {
        continue;
      }
      let next = tokens.get(index + 1);
      if next.is_some_and(|next| next.is_punct("=")) {
        evidence.extend(self.initializer_type(index + 2));
      } else if next.is_some_and(|next| next.is_ident("as")) {
        evidence.extend(self.cast_at(index + 1).map(|path| MockedType::new(path, TypeRule::Cast)));
      } else if index >= 2 && tokens[index - 1].is_punct(":") && tokens[index - 2].is_ident("useValue") && next.is_some_and(|next| next.is_punct(",") || next.is_punct("}")) {
        evidence.extend(self.provided_type(index - 2).map(|path| MockedType::new(path, TypeRule::Provider)));
      }
    }
    let types: BTreeSet<&Vec<String>> = evidence.iter().map(|mocked| &mocked.path).collect();
    match types.len() {
      0 => Evidence::Absent,
      1 => evidence.into_iter().min_by_key(|mocked| mocked.rule).map_or(Evidence::Absent, Evidence::Typed),
      _ => Evidence::Conflicting,
    }
  }

  fn initializer_type(&self, start: usize) -> Option<MockedType> {
    let tokens = self.tokens;
    let start = start + usize::from(tokens.get(start).is_some_and(|token| token.is_ident("await")));
    let first = tokens.get(start)?;
    if first.is_punct("{") {
      let close = self.brackets.close_of(start)?;
      return self.cast_at(close + 1).map(|path| MockedType::new(path, TypeRule::Cast));
    }
    if first.is_ident("new") {
      let (constructed, _) = parse_type(tokens, start + 1)?;
      return self.known_type(&constructed.path).then(|| MockedType::new(constructed.path, TypeRule::Instantiation));
    }
    if first.kind == TokenKind::Ident && MOCK_FACTORIES.contains(&first.text.as_str()) && tokens.get(start + 1).is_some_and(|token| token.is_punct("<")) {
      let (argument, close) = parse_type(tokens, start + 2)?;
      if !tokens.get(close).is_some_and(|token| token.is_punct(">")) || !tokens.get(close + 1).is_some_and(|token| token.is_punct("(")) {
        return None;
      }
      let (path, _) = argument.unwrap_mocked();
      return self.known_type(&path).then(|| MockedType::new(path, TypeRule::CreateMock));
    }
    None
  }

  fn cast_at(&self, index: usize) -> Option<Vec<String>> {
    let tokens = self.tokens;
    if !tokens.get(index)?.is_ident("as") {
      return None;
    }
    let mut target = index + 1;
    if tokens.get(target).is_some_and(|token| ERASING_CASTS.iter().any(|erasing| token.is_ident(erasing))) && tokens.get(target + 1).is_some_and(|token| token.is_ident("as")) {
      target += 2;
    }
    let (cast, _) = parse_type(tokens, target)?;
    let (path, _) = cast.unwrap_mocked();
    self.known_type(&path).then_some(path)
  }

  fn provided_type(&self, use_value: usize) -> Option<Vec<String>> {
    let tokens = self.tokens;
    let open = self.brackets.enclosing_brace(tokens, use_value)?;
    let close = self.brackets.close_of(open)?;
    let mut depth = 0i32;
    for index in open + 1..close {
      let token = &tokens[index];
      match token.text.as_str() {
        "{" | "(" | "[" if token.kind == TokenKind::Punct => depth += 1,
        "}" | ")" | "]" if token.kind == TokenKind::Punct => depth -= 1,
        "provide" if depth == 0 && token.kind == TokenKind::Ident && tokens.get(index + 1).is_some_and(|next| next.is_punct(":")) => {
          let (provided, _) = parse_type(tokens, index + 2)?;
          return (provided.args.is_empty() && self.known_type(&provided.path)).then_some(provided.path);
        }
        _ => {}
      }
    }
    None
  }

  fn spied_type(&mut self, target: &[String], reference: &ReferenceFact) -> Option<MockedType> {
    match target {
      [class, prototype] if prototype == PROTOTYPE => {
        let path = vec![class.clone()];
        self.known_type(&path).then(|| MockedType::new(path, TypeRule::Prototype))
      }
      [name] if is_plain_name(name) => self.binding_type(name, reference.span.start_byte, reference.from),
      _ => None,
    }
  }

  fn named_type(&self, name: &str) -> Option<MockedType> {
    if !name.starts_with(char::is_lowercase) {
      return None;
    }
    let stem = name
      .strip_prefix("mocked")
      .or_else(|| name.strip_prefix("mock"))
      .filter(|stem| stem.starts_with(char::is_uppercase))
      .unwrap_or(name);
    let stem = stem.strip_suffix("Mock").filter(|stem| !stem.is_empty()).unwrap_or(stem);
    let mut chars = stem.chars();
    let type_name: String = chars.next()?.to_uppercase().chain(chars).collect();
    let imported = self.view.ts_import(&type_name).is_some_and(|import| import.imported != Imported::Namespace);
    imported.then(|| MockedType::new(vec![type_name], TypeRule::Naming))
  }

  fn known_type(&self, path: &[String]) -> bool {
    let (Some(head), Some(last)) = (path.first(), path.last()) else { return false };
    if !last.starts_with(char::is_uppercase) {
      return false;
    }
    self.view.ts_import(head).is_some()
      || self
        .view
        .top_level_named(head)
        .is_some_and(|symbol| matches!(symbol.kind, EntityKind::Class | EntityKind::Interface | EntityKind::TypeAlias | EntityKind::Namespace))
  }

  fn is_shadowed(&self, name: &str, position: u32) -> bool {
    let Some(candidates) = self.declarations.get(name) else { return false };
    candidates.iter().any(|declaration| {
      let keyword = &self.tokens[declaration.keyword];
      keyword.start < position && self.scope_end(declaration.keyword).is_none_or(|end| position < end)
    })
  }

  fn scope_end(&self, keyword: usize) -> Option<u32> {
    let enclosing = self.brackets.enclosing.get(keyword).copied().flatten()?;
    let close = self.brackets.close_of(enclosing)?;
    let body_close = match self.tokens[enclosing].is_punct("(") && self.tokens.get(close + 1).is_some_and(|next| next.is_punct("{")) {
      true => self.brackets.close_of(close + 1)?,
      false => close,
    };
    Some(self.tokens[body_close].end)
  }

  fn visible_declaration(&self, name: &str, position: u32, scope: Option<LocalSymbolId>) -> Option<Declaration> {
    let mut ancestors = BTreeSet::new();
    let mut current = scope;
    while let Some(local) = current {
      ancestors.insert(local);
      current = self.view.symbol(local).and_then(|symbol| symbol.parent);
    }
    let candidates = self.declarations.get(name)?;
    let visible_before = candidates.partition_point(|declaration| self.tokens[declaration.keyword].start < position);
    candidates[..visible_before]
      .iter()
      .rev()
      .find(|declaration| declaration.owner.is_none_or(|owner| ancestors.contains(&owner)))
      .copied()
  }

  fn region(&self, declaration: &Declaration) -> (usize, usize) {
    let Some(owner) = declaration.owner.and_then(|local| self.view.symbol(local)) else {
      return (0, self.tokens.len());
    };
    let first = self.tokens.partition_point(|token| token.start < owner.span.start_byte);
    let last = self.tokens.partition_point(|token| token.start < owner.span.end_byte);
    (first, last)
  }

  fn member_span(&self, within: Span, head: &str, member: &str) -> Span {
    self
      .view
      .span_tokens(within)
      .windows(3)
      .find(|window| window[0].is_ident(head) && (window[1].is_punct(".") || window[1].is_punct("?.")) && window[2].is_ident(member))
      .and_then(|window| self.view.token_span(&window[2]))
      .unwrap_or(within)
  }

  fn string_span(&self, within: Span, member: &str) -> Span {
    self
      .view
      .span_tokens(within)
      .iter()
      .find(|token| token.kind == TokenKind::Str && token.text == member)
      .and_then(|token| inner_string_span(self.view, token))
      .unwrap_or(within)
  }
}

struct MockEdge<'m> {
  scope: Option<LocalSymbolId>,
  member: &'m str,
  span: Span,
  site: MockSite,
  replacement: Replacement,
}

fn emit(view: &FileView<'_>, sink: &mut Sink, mocked: &MockedType, edge: MockEdge<'_>) {
  sink.edge(
    EdgeKind::References,
    view.node(edge.scope),
    NodeRef::Unresolved {
      path: mocked.member_path(edge.member),
      from: edge.scope,
    },
    edge.span,
    Cert::heuristic(0.6, "jest-mock-member"),
    attrs([
      (ATTR_MOCK, json!(edge.replacement.as_str())),
      (ATTR_MOCK_TYPE, json!(mocked.rule.as_str())),
      (ATTR_MOCK_SITE, json!(edge.site.as_str())),
    ]),
  );
}

fn relative_module_files(importer: &RelPath, specifier: &str) -> Vec<RelPath> {
  if !specifier.starts_with("./") && !specifier.starts_with("../") {
    return Vec::new();
  }
  let Ok(module) = importer.resolve_relative(specifier) else { return Vec::new() };
  let module = module.as_str();
  let candidates: Vec<String> = match RUNTIME_EXTENSIONS.iter().find(|(extension, _)| module.ends_with(extension)) {
    Some((extension, sources)) => {
      let stem = &module[..module.len() - extension.len()];
      sources.iter().map(|source| format!("{stem}{source}")).collect()
    }
    None => SOURCE_EXTENSIONS.iter().chain(&INDEX_EXTENSIONS).map(|suffix| format!("{module}{suffix}")).collect(),
  };
  candidates.iter().filter_map(|candidate| RelPath::new(candidate).ok()).collect()
}

fn factory_exports(tokens: &[Token]) -> Vec<&Token> {
  let Some(arrow) = tokens.iter().position(|token| token.is_punct("=>")) else {
    return Vec::new();
  };
  let open = match &tokens[arrow + 1..] {
    [paren, brace, ..] if paren.is_punct("(") && brace.is_punct("{") => arrow + 2,
    [brace, ret, object, ..] if brace.is_punct("{") && ret.is_ident("return") && object.is_punct("{") => arrow + 3,
    _ => return Vec::new(),
  };
  let mut exports = Vec::new();
  let mut depth = 1i32;
  for (index, token) in tokens.iter().enumerate().skip(open + 1) {
    if token.kind == TokenKind::Punct {
      match token.text.as_str() {
        "{" | "(" | "[" => depth += 1,
        "}" | ")" | "]" => depth -= 1,
        _ => {}
      }
      if depth == 0 {
        break;
      }
      continue;
    }
    let after_separator = tokens.get(index - 1).is_some_and(|before| before.is_punct("{") || before.is_punct(","));
    let is_key = matches!(token.kind, TokenKind::Ident | TokenKind::Str)
      && depth == 1
      && after_separator
      && tokens.get(index + 1).is_some_and(|next| [":", ",", "}", "("].iter().any(|punct| next.is_punct(punct)));
    if is_key && is_plain_name(&token.text) && !NON_EXPORT_KEYS.contains(&token.text.as_str()) {
      exports.push(token);
    }
  }
  exports
}

fn inner_string_span(view: &FileView<'_>, token: &Token) -> Option<Span> {
  if token.kind != TokenKind::Str || token.end < token.start + 2 {
    return view.token_span(token);
  }
  view.span_of(token.start as usize + 1, token.end as usize - 1)
}

fn is_plain_name(name: &str) -> bool {
  let mut chars = name.chars();
  chars.next().is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$')
    && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    && !matches!(name, "this" | "super")
}

fn is_mock_setup(name: &str) -> bool {
  name.strip_prefix("mock").is_some_and(|rest| rest.starts_with(char::is_uppercase))
}

fn is_replacing_setup(name: &str) -> bool {
  REPLACING_SETUPS.iter().any(|setup| name.starts_with(setup))
}

fn is_call_matcher(name: &str) -> bool {
  CALL_MATCHERS.iter().any(|matcher| name.starts_with(matcher))
}

fn assigned_name(tokens: &[Token], equals: usize) -> Option<String> {
  let mut index = equals;
  while index > 0 {
    index -= 1;
    let token = &tokens[index];
    if token.kind == TokenKind::Ident && DECLARATION_KEYWORDS.contains(&token.text.as_str()) {
      return tokens.get(index + 1).filter(|name| name.kind == TokenKind::Ident).map(|name| name.text.clone());
    }
    if [";", "{", "}", "(", ")", "=>"].iter().any(|stop| token.is_punct(stop)) {
      break;
    }
  }
  let name = tokens.get(equals.checked_sub(1)?)?;
  let before = equals.checked_sub(2).and_then(|index| tokens.get(index));
  let is_member_or_annotation = before.is_some_and(|before| before.is_punct(".") || before.is_punct("?.") || before.is_punct(":"));
  (name.kind == TokenKind::Ident && !is_member_or_annotation).then(|| name.text.clone())
}

fn parse_type(tokens: &[Token], start: usize) -> Option<(TypeExpr, usize)> {
  let first = tokens.get(start).filter(|token| token.kind == TokenKind::Ident)?;
  let mut path = vec![first.text.clone()];
  let mut cursor = start + 1;
  while tokens.get(cursor).is_some_and(|token| token.is_punct(".")) {
    let Some(segment) = tokens.get(cursor + 1).filter(|token| token.kind == TokenKind::Ident) else {
      break;
    };
    path.push(segment.text.clone());
    cursor += 2;
  }
  let mut args = Vec::new();
  if tokens.get(cursor).is_some_and(|token| token.is_punct("<")) {
    cursor += 1;
    loop {
      let (argument, next) = parse_type(tokens, cursor)?;
      args.push(argument);
      cursor = next;
      match tokens.get(cursor) {
        Some(token) if token.is_punct(",") => cursor += 1,
        Some(token) if token.is_punct(">") => {
          cursor += 1;
          break;
        }
        _ => return None,
      }
    }
  }
  Some((TypeExpr { path, args }, cursor))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::lex::lex;

  #[test]
  fn should_parse_nested_generic_types_and_unwrap_mock_wrappers() {
    let tokens = lex("jest.Mocked<Partial<OrdersService>> = x", LanguageFamily::Ts);
    let (parsed, next) = parse_type(&tokens, 0).unwrap();
    assert_eq!(next, 9);
    assert_eq!(parsed.unwrap_mocked(), (vec!["OrdersService".to_owned()], true));
    let plain = lex("api.Client<T, U>", LanguageFamily::Ts);
    let (parsed, _) = parse_type(&plain, 0).unwrap();
    assert_eq!(parsed.clone().unwrap_mocked(), (vec!["api".to_owned(), "Client".to_owned()], false));
    assert_eq!(parsed.args.len(), 2);
    assert_eq!(parse_type(&lex("Foo<Bar", LanguageFamily::Ts), 0), None);
  }

  #[test]
  fn should_name_the_assigned_binding_of_an_initializer() {
    let declared = lex("const users: Users = {", LanguageFamily::Ts);
    assert_eq!(assigned_name(&declared, 4).as_deref(), Some("users"));
    let assigned = lex("beforeEach(() => { users = {", LanguageFamily::Ts);
    assert_eq!(assigned_name(&assigned, 7).as_deref(), Some("users"));
    let member = lex("this.users = {", LanguageFamily::Ts);
    assert_eq!(assigned_name(&member, 3), None);
  }

  #[test]
  fn should_find_the_same_owner_as_the_innermost_symbol_scan() {
    let source = "describe('a', () => {\n  const x = 1;\n  it('b', () => {\n    const y = 2;\n  });\n  describe('c', () => {\n    it('d', () => {});\n  });\n});\nfunction helper() { return 3; }\nclass K { m() { return 4; } }\n";
    let file = atlas_core::FileRef::detect(
      atlas_core::RepoName::new("r").unwrap(),
      atlas_core::RelPath::new("a.spec.ts").unwrap(),
      atlas_core::ContentHash::of(source.as_bytes()),
    )
    .unwrap();
    let facts = atlas_core::LanguageAnalyzer::analyze(&atlas_lang_ts::TsAnalyzer::new(), &atlas_core::AnalysisInput { file: &file, source }).unwrap();
    let input = atlas_core::DetectInput {
      facts: &facts,
      source,
      package: None,
    };
    let view = FileView::new(&input);
    let owners = Owners::new(&facts.symbols);
    assert!(facts.symbols.len() >= 6);
    for byte in 0..=u32::try_from(source.len()).unwrap() {
      assert_eq!(owners.at(byte), view.innermost_symbol(byte), "byte {byte}");
    }
  }

  #[test]
  fn should_recognise_mock_setup_and_call_matcher_names() {
    assert!(is_mock_setup("mockResolvedValueOnce"));
    assert!(is_replacing_setup("mockImplementationOnce"));
    assert!(!is_replacing_setup("mockClear"));
    assert!(!is_mock_setup("mocked"));
    assert!(is_call_matcher("toHaveBeenCalledWith"));
    assert!(is_call_matcher("toBeCalledTimes"));
    assert!(!is_call_matcher("toBe"));
  }
}
