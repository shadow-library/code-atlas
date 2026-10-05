use std::cell::OnceCell;

use atlas_core::{
  ArgSummary, DetectInput, EntityKey, EntityKind, FileFacts, ImportBinding, ImportFact, LanguageFamily, LocalSymbolId, NodeRef, ParamFact, RepoName, Span, SymbolFact, UseBinding,
  is_test_path,
};

use crate::lex::{Token, TokenKind, lex};

pub(crate) struct FileView<'a> {
  pub facts: &'a FileFacts,
  pub source: &'a str,
  tokens: OnceCell<Vec<Token>>,
  line_starts: OnceCell<Vec<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Imported<'a> {
  Named(&'a str),
  Default,
  Namespace,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TsImportRef<'a> {
  pub specifier: &'a str,
  pub imported: Imported<'a>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Binding<'v> {
  Param(&'v ParamFact),
  Init(&'v [Token]),
}

impl<'a> FileView<'a> {
  pub fn new(input: &DetectInput<'a>) -> Self {
    Self {
      facts: input.facts,
      source: input.source,
      tokens: OnceCell::new(),
      line_starts: OnceCell::new(),
    }
  }

  pub fn family(&self) -> LanguageFamily {
    self.facts.file.language.family()
  }

  pub fn repo(&self) -> &RepoName {
    &self.facts.file.repo
  }

  pub fn tokens(&self) -> &[Token] {
    self.tokens.get_or_init(|| lex(self.source, self.family()))
  }

  pub fn tokens_in(&self, start: u32, end: u32) -> &[Token] {
    let tokens = self.tokens();
    let first = tokens.partition_point(|token| token.start < start);
    let last = tokens.partition_point(|token| token.start < end);
    tokens.get(first..last.max(first)).unwrap_or_default()
  }

  pub fn span_tokens(&self, span: Span) -> &[Token] {
    self.tokens_in(span.start_byte, span.end_byte)
  }

  pub fn token_span(&self, token: &Token) -> Option<Span> {
    self.span_of(token.start as usize, token.end as usize)
  }

  pub fn span_of(&self, start: usize, end: usize) -> Option<Span> {
    if start > end || end > self.source.len() {
      return None;
    }
    let line_starts = self.line_starts.get_or_init(|| line_starts(self.source));
    let position = |byte: usize| {
      let line = line_starts.partition_point(|line_start| *line_start <= byte);
      let line_start = line.checked_sub(1).and_then(|index| line_starts.get(index)).copied().unwrap_or(0);
      (u32::try_from(line).ok(), u32::try_from(byte - line_start).ok())
    };
    let ((Some(start_line), Some(start_col)), (Some(end_line), Some(end_col))) = (position(start), position(end)) else {
      return None;
    };
    Some(Span {
      start_byte: u32::try_from(start).ok()?,
      end_byte: u32::try_from(end).ok()?,
      start_line,
      start_col,
      end_line,
      end_col,
    })
  }

  pub fn symbol(&self, local: LocalSymbolId) -> Option<&'a SymbolFact> {
    self.facts.symbol(local)
  }

  pub fn file_key(&self) -> EntityKey {
    EntityKey::File {
      repo: self.repo().clone(),
      path: self.facts.file.path.clone(),
    }
  }

  pub fn node(&self, from: Option<LocalSymbolId>) -> NodeRef {
    match from {
      Some(local) => NodeRef::Local(local),
      None => NodeRef::Key(self.file_key()),
    }
  }

  pub fn qualified_name(&self, symbol: &SymbolFact) -> String {
    let separator = match self.family() {
      LanguageFamily::Rust => "::",
      _ => ".",
    };
    symbol.module_suffix.iter().chain(&symbol.item_path).cloned().collect::<Vec<_>>().join(separator)
  }

  pub fn innermost_symbol(&self, byte: u32) -> Option<LocalSymbolId> {
    self
      .facts
      .symbols
      .iter()
      .filter(|symbol| symbol.span.contains_byte(byte))
      .min_by_key(|symbol| symbol.span.len())
      .map(|symbol| symbol.local)
  }

  pub fn top_level_named(&self, name: &str) -> Option<&'a SymbolFact> {
    let mut matches = self
      .facts
      .symbols
      .iter()
      .filter(|symbol| symbol.parent.is_none() && symbol.item_path.len() == 1 && symbol.name == name);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
  }

  pub fn ts_import(&self, local: &str) -> Option<TsImportRef<'a>> {
    self.facts.imports.iter().find_map(|import| {
      let (specifier, bindings) = match import {
        ImportFact::TsImport { specifier, bindings, .. } => (specifier.as_str(), bindings),
        ImportFact::TsRequire {
          specifier: ArgSummary::Str(specifier),
          bindings,
          ..
        } => (specifier.as_str(), bindings),
        _ => return None,
      };
      bindings.iter().find(|binding| binding.local() == local).map(|binding| TsImportRef {
        specifier,
        imported: match binding {
          ImportBinding::Named { imported, .. } => Imported::Named(imported.as_str()),
          ImportBinding::Default { .. } => Imported::Default,
          ImportBinding::Namespace { .. } => Imported::Namespace,
        },
      })
    })
  }

  pub fn imports_named(&self, local: &str, specifiers: &[&str], name: &str) -> bool {
    self
      .ts_import(local)
      .is_some_and(|import| specifiers.contains(&import.specifier) && import.imported == Imported::Named(name))
  }

  pub fn imports_module(&self, local: &str, specifiers: &[&str]) -> bool {
    self
      .ts_import(local)
      .is_some_and(|import| specifiers.contains(&import.specifier) && matches!(import.imported, Imported::Default | Imported::Namespace))
  }

  pub fn ts_specifiers(&self) -> impl Iterator<Item = &'a str> {
    self.facts.imports.iter().filter_map(|import| match import {
      ImportFact::TsImport { specifier, .. } => Some(specifier.as_str()),
      ImportFact::TsRequire {
        specifier: ArgSummary::Str(specifier),
        ..
      } => Some(specifier.as_str()),
      _ => None,
    })
  }

  pub fn rust_use(&self, local: &str) -> Option<&'a [String]> {
    self.facts.imports.iter().find_map(|import| {
      let ImportFact::RustUse { paths, .. } = import else { return None };
      paths.iter().find_map(|path| {
        let bound = match &path.binding {
          UseBinding::Name => path.segments.last().map(String::as_str),
          UseBinding::Alias(alias) => Some(alias.as_str()),
          UseBinding::Glob => None,
        };
        (bound == Some(local)).then_some(path.segments.as_slice())
      })
    })
  }

  pub fn rust_uses_crate(&self, crate_name: &str) -> bool {
    self.facts.imports.iter().any(|import| match import {
      ImportFact::RustUse { paths, .. } => paths.iter().any(|path| path.segments.first().is_some_and(|first| first == crate_name)),
      ImportFact::RustExternCrate { name, .. } => name == crate_name,
      _ => false,
    })
  }

  pub fn rust_path_is(&self, path: &[String], full: &[&str]) -> bool {
    let Some((head, rest)) = path.split_first() else { return false };
    if path.iter().map(String::as_str).eq(full.iter().copied()) {
      return true;
    }
    let Some(expanded) = self.rust_use(head) else { return false };
    expanded.iter().chain(rest).map(String::as_str).eq(full.iter().copied())
  }

  pub fn module_const_string(&self, name: &str) -> Option<String> {
    let symbol = self.top_level_named(name).filter(|symbol| symbol.kind == EntityKind::Constant)?;
    let tokens = self.span_tokens(symbol.span);
    let after_name = match self.family() {
      LanguageFamily::Rust => {
        let index = tokens.iter().position(|token| token.is_ident("const") || token.is_ident("static"))?;
        tokens.get(index + 1).filter(|token| token.is_ident(name))?;
        index + 2
      }
      _ => {
        let signature = symbol.signature.as_deref().unwrap_or_default();
        if !signature.starts_with("const ") && !signature.starts_with("export const ") {
          return None;
        }
        tokens.first().filter(|token| token.is_ident(name))?;
        1
      }
    };
    let value_start = initializer_start(tokens, after_name)?;
    single_string_literal(tokens.get(value_start..)?)
  }

  pub fn resolve_binding(&self, scope: Option<LocalSymbolId>, name: &str) -> Option<Binding<'_>> {
    if let Some(symbol) = scope.and_then(|local| self.symbol(local)) {
      if let Some(param) = symbol.params.iter().find(|param| param.name == name) {
        return Some(Binding::Param(param));
      }
      if let Some(init) = self.local_initializer(symbol.span, name) {
        return Some(Binding::Init(init));
      }
    }
    let symbol = self
      .top_level_named(name)
      .filter(|symbol| matches!(symbol.kind, EntityKind::Constant | EntityKind::Variable))?;
    let tokens = self.span_tokens(symbol.span);
    let after_name = tokens.iter().position(|token| token.is_ident(name))? + 1;
    let start = initializer_start(tokens, after_name)?;
    Some(Binding::Init(tokens.get(start..)?))
  }

  pub fn local_initializer(&self, scope: Span, name: &str) -> Option<&[Token]> {
    let tokens = self.span_tokens(scope);
    let mut found = None;
    let mut count = 0usize;
    for (index, token) in tokens.iter().enumerate() {
      if !matches!(token.text.as_str(), "const" | "let" | "var") || token.kind != TokenKind::Ident {
        continue;
      }
      let mut cursor = index + 1;
      if tokens.get(cursor).is_some_and(|token| token.is_ident("mut")) {
        cursor += 1;
      }
      if !tokens.get(cursor).is_some_and(|token| token.is_ident(name)) {
        continue;
      }
      count += 1;
      found = initializer_start(tokens, cursor + 1).map(|start| (index, start));
    }
    let (declaration, start) = found.filter(|_| count == 1)?;
    let reassigned = tokens.iter().enumerate().any(|(index, token)| {
      index > declaration + 1
        && token.is_ident(name)
        && tokens.get(index + 1).is_some_and(|next| next.is_punct("="))
        && !index
          .checked_sub(1)
          .and_then(|previous| tokens.get(previous))
          .is_some_and(|previous| previous.is_punct(".") || previous.is_punct("?."))
    });
    if reassigned {
      return None;
    }
    tokens.get(start..)
  }

  pub fn class_of(&self, local: LocalSymbolId) -> Option<&'a SymbolFact> {
    let symbol = self.symbol(local)?;
    match self.family() {
      LanguageFamily::Rust => {
        let type_name = symbol.item_path.first()?;
        self
          .facts
          .symbols
          .iter()
          .find(|candidate| candidate.parent.is_none() && candidate.item_path.len() == 1 && candidate.item_path[0] == *type_name && candidate.module_suffix == symbol.module_suffix)
      }
      _ => {
        let mut current = symbol;
        while let Some(parent) = current.parent.and_then(|parent| self.symbol(parent)) {
          if parent.kind == EntityKind::Class {
            return Some(parent);
          }
          current = parent;
        }
        None
      }
    }
  }

  pub fn field_type(&self, class: &SymbolFact, field: &str) -> Option<String> {
    let children = self.facts.children(class.local);
    let mut constructor_type = None;
    for child in children {
      if child.kind == EntityKind::Field && child.name == field {
        let declared = child.signature.as_deref().and_then(|signature| field_type_from_signature(signature, field));
        if declared.is_some() {
          return declared;
        }
      }
      if child.name == "constructor" {
        constructor_type = child.params.iter().find(|param| param.name == field).and_then(|param| param.type_text.clone());
      }
    }
    constructor_type
  }

  pub fn is_test_context(&self, scope: Option<LocalSymbolId>) -> bool {
    if is_test_path(self.facts.file.path.as_str()) {
      return true;
    }
    let mut current = scope.and_then(|local| self.symbol(local));
    while let Some(symbol) = current {
      if symbol.test.is_some() {
        return true;
      }
      current = symbol.parent.and_then(|parent| self.symbol(parent));
    }
    false
  }

  pub fn enclosing_scope_contains_binding(&self, scope: Option<LocalSymbolId>, name: &str) -> bool {
    let Some(symbol) = scope.and_then(|local| self.symbol(local)) else { return false };
    if symbol.params.iter().any(|param| param.name == name) {
      return true;
    }
    let tokens = self.span_tokens(symbol.span);
    tokens
      .windows(2)
      .any(|pair| matches!(pair[0].text.as_str(), "const" | "let" | "var") && pair[1].is_ident(name))
  }
}

fn line_starts(source: &str) -> Vec<usize> {
  std::iter::once(0)
    .chain(source.bytes().enumerate().filter(|(_, byte)| *byte == b'\n').map(|(index, _)| index + 1))
    .collect()
}

fn field_type_from_signature(signature: &str, field: &str) -> Option<String> {
  let after_name = signature.split_once(field)?.1.trim_start();
  let after_name = after_name.strip_prefix('?').unwrap_or(after_name);
  let type_text = after_name.strip_prefix(':')?;
  let type_text = type_text.split('=').next().unwrap_or(type_text).trim();
  (!type_text.is_empty()).then(|| type_text.to_owned())
}

pub(crate) fn initializer_start(tokens: &[Token], after_name: usize) -> Option<usize> {
  let first = tokens.get(after_name)?;
  if first.is_punct("=") {
    return Some(after_name + 1);
  }
  if !first.is_punct(":") {
    return None;
  }
  let mut depth = 0i32;
  for (index, token) in tokens.iter().enumerate().skip(after_name + 1) {
    if token.kind != TokenKind::Punct {
      continue;
    }
    match token.text.as_str() {
      "<" | "(" | "[" | "{" => depth += 1,
      ">" | ")" | "]" | "}" => depth -= 1,
      "=" if depth <= 0 => return Some(index + 1),
      ";" | "," if depth <= 0 => return None,
      _ => {}
    }
  }
  None
}

fn single_string_literal(tokens: &[Token]) -> Option<String> {
  let (value, consumed) = match tokens {
    [first, ..] if first.kind == TokenKind::Str => (first.text.clone(), 1),
    [open, chunk, close, ..] if open.is_punct("`") && chunk.kind == TokenKind::TemplateChunk && close.is_punct("`") => (chunk.text.clone(), 3),
    [open, close, ..] if open.is_punct("`") && close.is_punct("`") => (String::new(), 2),
    _ => return None,
  };
  match tokens.get(consumed..)? {
    [] => Some(value),
    [semicolon] if semicolon.is_punct(";") => Some(value),
    [as_token, const_token, rest @ ..] if as_token.is_ident("as") && const_token.is_ident("const") && rest.iter().all(|token| token.is_punct(";")) => Some(value),
    _ => None,
  }
}

pub(crate) fn starts_with_call(tokens: &[Token], path: &[&str], separator: &str) -> bool {
  let tokens = strip_leading(tokens, &["await", "new"]);
  let mut cursor = 0usize;
  for (index, segment) in path.iter().enumerate() {
    if index > 0 {
      if !tokens.get(cursor).is_some_and(|token| token.is_punct(separator)) {
        return false;
      }
      cursor += 1;
    }
    if !tokens.get(cursor).is_some_and(|token| token.is_ident(segment)) {
      return false;
    }
    cursor = skip_turbofish(tokens, cursor + 1);
  }
  tokens.get(cursor).is_some_and(|token| token.is_punct("("))
}

fn skip_turbofish(tokens: &[Token], cursor: usize) -> usize {
  let open = match (tokens.get(cursor), tokens.get(cursor + 1)) {
    (Some(colons), Some(angle)) if colons.is_punct("::") && angle.is_punct("<") => cursor + 1,
    (Some(angle), _) if angle.is_punct("<") => cursor,
    _ => return cursor,
  };
  let mut depth = 0i32;
  for (index, token) in tokens.iter().enumerate().skip(open) {
    if token.is_punct("<") {
      depth += 1;
    } else if token.is_punct(">") {
      depth -= 1;
      if depth == 0 {
        return index + 1;
      }
    } else if token.is_punct("(") || token.is_punct(";") {
      return cursor;
    }
  }
  cursor
}

fn strip_leading<'t>(mut tokens: &'t [Token], words: &[&str]) -> &'t [Token] {
  while let Some((first, rest)) = tokens.split_first() {
    if first.kind == TokenKind::Ident && words.contains(&first.text.as_str()) {
      tokens = rest;
    } else {
      break;
    }
  }
  tokens
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tokens(source: &str) -> Vec<Token> {
    lex(source, LanguageFamily::Ts)
  }

  #[test]
  fn should_find_the_initializer_after_an_optional_type_annotation() {
    let typed = tokens("x: Record<string, number> = 1");
    assert_eq!(initializer_start(&typed, 1), Some(9));
    assert_eq!(initializer_start(&tokens("x = 1"), 1), Some(2));
    assert_eq!(initializer_start(&tokens("x; y = 1"), 1), None);
  }

  #[test]
  fn should_accept_only_a_single_string_literal_initializer() {
    assert_eq!(single_string_literal(&tokens("'orders'")).as_deref(), Some("orders"));
    assert_eq!(single_string_literal(&tokens("`orders`;")).as_deref(), Some("orders"));
    assert_eq!(single_string_literal(&tokens("'orders' as const")).as_deref(), Some("orders"));
    assert_eq!(single_string_literal(&tokens("'a' + b")), None);
    assert_eq!(single_string_literal(&tokens("`a${b}`")), None);
    assert_eq!(single_string_literal(&tokens("compute()")), None);
  }

  #[test]
  fn should_match_call_shapes_with_generics_and_await() {
    assert!(starts_with_call(&tokens("await NestFactory.create(AppModule)"), &["NestFactory", "create"], "."));
    assert!(starts_with_call(&tokens("express()"), &["express"], "."));
    assert!(!starts_with_call(&tokens("express.json()"), &["express"], "."));
    assert!(starts_with_call(&lex("Router::<AppState>::new()", LanguageFamily::Rust), &["Router", "new"], "::"));
    assert!(starts_with_call(&tokens("axios.create<Api>({})"), &["axios", "create"], "."));
    assert!(!starts_with_call(&tokens("map.get('x')"), &["axios", "get"], "."));
  }

  #[test]
  fn should_compute_spans_like_span_of_text_from_a_line_index() {
    let source = "a\nbc\n\nd\u{e9}f\n";
    let file = atlas_core::FileRef::detect(RepoName::new("r").unwrap(), atlas_core::RelPath::new("a.ts").unwrap(), atlas_core::ContentHash::of(b"")).unwrap();
    let facts = FileFacts::new(file, atlas_core::Producer::new("test", 1));
    let input = DetectInput {
      facts: &facts,
      source,
      package: None,
    };
    let view = FileView::new(&input);
    for start in 0..=source.len() {
      for end in start..=source.len() {
        assert_eq!(view.span_of(start, end), Span::of_text(source, start, end), "{start}..{end}");
      }
    }
    assert_eq!(view.span_of(3, 2), None);
    assert_eq!(view.span_of(0, source.len() + 1), None);
  }

  #[test]
  fn should_extract_declared_field_types_from_signatures() {
    assert_eq!(field_type_from_signature("private readonly http: HttpService", "http").as_deref(), Some("HttpService"));
    assert_eq!(field_type_from_signature("http: reqwest::Client", "http").as_deref(), Some("reqwest::Client"));
    assert_eq!(field_type_from_signature("private readonly pool = new Pool()", "pool"), None);
  }
}
