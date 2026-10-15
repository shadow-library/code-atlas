use std::collections::{BTreeMap, BTreeSet};

use atlas_core::{ArgSummary, Attrs, EntityKey, EntityKind, FileFacts, ImportFact, LocalSymbolId, ObjectEntry, RefKind, ReferenceFact, Span, SymbolFact, Visibility};
use serde_json::json;

use super::clients::{MethodSource, Request, apply_header_hints, apply_header_options, classify_base_arg};
use crate::cert::Cert;
use crate::families::{ClientExports, ClientShape, PathArgument, PathStyle};
use crate::lex::{Token, TokenKind};
use crate::objects::const_object;
use crate::sink::Sink;
use crate::strings::{Piece, StringEval, evaluate};
use crate::view::{Binding, FileView, Imported, starts_with_call};

const BASE_URL_KEYS: [&str; 2] = ["baseUrl", "baseURL"];
const SOURCE_EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
const INDEX_FILES: [&str; 4] = ["index.ts", "index.tsx", "index.js", "index.jsx"];
const EMITTED_TS_EXTENSIONS: [(&str, &str); 4] = [(".js", ".ts"), (".jsx", ".tsx"), (".mjs", ".mts"), (".cjs", ".cts")];
const STRIPPED_EXTENSIONS: [&str; 9] = [".d.ts", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];
const PLACEHOLDER: &str = "{}";
const CLIENT_DEFINITION_KIND: &str = "typed_client";

pub(super) struct TypedShape<'s> {
  pub library: &'s str,
  pub shape: &'s ClientShape,
}

#[derive(Default)]
struct ClientBinding {
  base: Option<String>,
  base_pieces: Vec<Piece>,
  hints: BTreeSet<String>,
  hint_source: Option<String>,
  extra: Attrs,
}

pub(super) fn shape_applies(facts: &FileFacts, shape: &ClientShape) -> bool {
  facts.imports.iter().any(|import| match import {
    ImportFact::TsImport { specifier, .. }
    | ImportFact::TsRequire {
      specifier: ArgSummary::Str(specifier),
      ..
    } => module_matches(shape, facts.file.path.as_str(), specifier) || imports_surface_factory(shape, specifier),
    _ => false,
  })
}

fn imports_surface_factory(shape: &ClientShape, specifier: &str) -> bool {
  shape.surfaces.as_ref().is_some_and(|surfaces| surfaces.modules.iter().any(|module| module == specifier))
}

pub(super) fn emit_client_definitions(view: &FileView<'_>, sink: &mut Sink, typed: &TypedShape<'_>) {
  for symbol in exported_bindings(view) {
    if let Some(client) = factory_client(view, None, &symbol.name, typed.shape) {
      emit_definition(view, sink, typed, &symbol.name, symbol.span, client);
    }
  }
}

pub(super) fn emit_surface_definitions(view: &FileView<'_>, sink: &mut Sink, typed: &TypedShape<'_>) {
  if typed.shape.surfaces.is_none() {
    return;
  }
  for symbol in exported_bindings(view) {
    match surface_container(view, None, &symbol.name, typed.shape) {
      Some(surfaces) => {
        for (member, client) in surfaces {
          emit_definition(view, sink, typed, &format!("{}.{member}", symbol.name), symbol.span, client);
        }
      }
      None => {
        if let Some(client) = surface_member(view, None, &symbol.name, typed.shape) {
          emit_definition(view, sink, typed, &symbol.name, symbol.span, client);
        }
      }
    }
  }
}

fn exported_bindings<'v>(view: &FileView<'v>) -> impl Iterator<Item = &'v SymbolFact> {
  view.facts.symbols.iter().filter(|symbol| {
    symbol.parent.is_none() && symbol.item_path.len() == 1 && matches!(symbol.kind, EntityKind::Constant | EntityKind::Variable) && symbol.visibility == Visibility::Exported
  })
}

fn emit_definition(view: &FileView<'_>, sink: &mut Sink, typed: &TypedShape<'_>, export: &str, span: Span, client: ClientBinding) {
  let Ok(kind) = CLIENT_DEFINITION_KIND.parse::<EntityKind>() else { return };
  let path = view.facts.file.path.as_str();
  let stem = module_stem(path);
  let mut modules = vec![stem.to_owned()];
  modules.extend(stem.strip_suffix("/index").map(str::to_owned));
  let mut attrs = client.extra;
  attrs.insert("library".to_owned(), json!(typed.library));
  attrs.insert("export".to_owned(), json!(export));
  attrs.insert("modules".to_owned(), json!(modules));
  if !client.base_pieces.is_empty() {
    let base = StringEval {
      pieces: client.base_pieces,
      cert: Cert::Syntactic,
    };
    attrs.insert("base".to_owned(), json!(base.pattern(PLACEHOLDER)));
  }
  if let Some(source) = client.base {
    attrs.insert("base_source".to_owned(), json!(source));
  }
  let key = EntityKey::Other {
    kind,
    repo: Some(view.repo().clone()),
    name: format!("{path}#{export}"),
  };
  sink.entity(key, span, Cert::Syntactic, attrs);
}

pub(super) fn typed_request<'f>(view: &FileView<'_>, reference: &'f ReferenceFact, typed: &TypedShape<'_>) -> Option<Request<'f>> {
  if !matches!(reference.kind, RefKind::MethodCall | RefKind::Call) {
    return None;
  }
  let (method_name, receiver) = reference.path.split_last()?;
  let method = typed.shape.method(method_name)?;
  let client = client_binding(view, reference.from, receiver, typed.shape)?;
  let (url_arg, options) = path_argument(&reference.args, &typed.shape.path)?;
  let scope = reference.from;
  let mut request = Request::new(reference, typed.library, MethodSource::Literal(method));
  request.url = evaluate(view, scope, url_arg);
  request.url_arg = Some(url_arg.clone());
  request.base = client.base;
  request.base_pieces = client.base_pieces;
  request.hints = client.hints;
  request.hint_source = client.hint_source;
  request.style = Some(typed.shape.style);
  request.extra = client.extra;
  if let Some(target) = &typed.shape.target_package {
    request.extra.insert("target_package".to_owned(), json!(target));
  }
  if let (true, Some(base)) = (request.base_pieces.is_empty(), &typed.shape.base) {
    request.extra.insert("client_base".to_owned(), json!(base));
  }
  if let Some(entries) = options {
    apply_header_options(view, scope, entries, &mut request);
  }
  Some(request)
}

fn path_argument<'a>(args: &'a [ArgSummary], path: &PathArgument) -> Option<(&'a ArgSummary, Option<&'a [ObjectEntry]>)> {
  let argument = args.get(path.index)?;
  if let (ArgSummary::Object(entries), Some(key)) = (argument, &path.key)
    && let Some(entry) = entries.iter().find(|entry| entry.key.static_name() == Some(key.as_str()))
  {
    return Some((&entry.value, Some(entries)));
  }
  let options = match args.get(path.index + 1) {
    Some(ArgSummary::Object(entries)) => Some(entries.as_slice()),
    _ => None,
  };
  Some((argument, options))
}

fn client_binding(view: &FileView<'_>, scope: Option<LocalSymbolId>, receiver: &[String], shape: &ClientShape) -> Option<ClientBinding> {
  match receiver {
    [name] if is_shadowed(view, scope, name) => factory_client(view, scope, name, shape).or_else(|| surface_member(view, scope, name, shape)),
    [name] => imported_client(view, name, shape),
    [container, member] if shape.surfaces.is_some() && is_shadowed(view, scope, container) => surface_container(view, scope, container, shape)?.remove(member.as_str()),
    [namespace, export] if !is_shadowed(view, scope, namespace) => {
      let import = view.ts_import(namespace)?;
      if import.imported != Imported::Namespace {
        return imported_member_client(view, namespace, export, shape);
      }
      let accepted = module_matches(shape, view.facts.file.path.as_str(), import.specifier) && shape.is_client_export(export);
      accepted.then(|| imported_binding(view, import.specifier, Some(export)))
    }
    [namespace, container, member] if !is_shadowed(view, scope, namespace) => {
      let surfaces = shape.surfaces.as_ref()?;
      let import = view.ts_import(namespace)?;
      let accepted = import.imported == Imported::Namespace
        && module_matches(shape, view.facts.file.path.as_str(), import.specifier)
        && shape.is_client_export(container)
        && surfaces.follows_members_of(container);
      accepted.then(|| imported_binding(view, import.specifier, Some(&format!("{container}.{member}"))))
    }
    _ => None,
  }
}

fn imported_member_client(view: &FileView<'_>, name: &str, member: &str, shape: &ClientShape) -> Option<ClientBinding> {
  let surfaces = shape.surfaces.as_ref()?;
  let import = view.ts_import(name)?;
  let container = match import.imported {
    Imported::Named(imported) => imported,
    Imported::Default => "default",
    Imported::Namespace => return None,
  };
  let accepted = module_matches(shape, view.facts.file.path.as_str(), import.specifier) && shape.is_client_export(container) && surfaces.follows_members_of(container);
  accepted.then(|| imported_binding(view, import.specifier, Some(&format!("{container}.{member}"))))
}

fn surface_container(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str, shape: &ClientShape) -> Option<BTreeMap<String, ClientBinding>> {
  let Binding::Init(tokens) = view.resolve_binding(scope, name)? else { return None };
  let tokens = skip_await(tokens);
  let head = tokens.first()?;
  if !starts_with_call(tokens, &[&head.text], ".") || is_shadowed(view, scope, &head.text) {
    return None;
  }
  let import = view.ts_import(&head.text)?;
  let imported = match import.imported {
    Imported::Named(imported) => imported,
    Imported::Default => "default",
    Imported::Namespace => return None,
  };
  let surfaces = shape.surface_factory(import.specifier, imported)?;
  let factory = view
    .facts
    .references
    .iter()
    .find(|reference| reference.kind == RefKind::Call && reference.span.start_byte == head.start && reference.path.last() == Some(&head.text))?;
  let Some(ArgSummary::Object(options)) = factory.args.first() else { return None };
  let map = options.iter().find(|entry| entry.key.static_name() == Some(surfaces.option.as_str()))?;
  let entries = match &map.value {
    ArgSummary::Object(entries) => entries.clone(),
    ArgSummary::Path(path) => match path.as_slice() {
      [constant] => const_object(view, factory.from, constant)?,
      _ => return None,
    },
    _ => return None,
  };
  let bases = entries.iter().filter_map(|entry| {
    let member = entry.key.static_name()?;
    let (base, base_pieces) = base_of(view, factory.from, &entry.value);
    let mut client = ClientBinding {
      base,
      base_pieces,
      ..ClientBinding::default()
    };
    client.extra.insert("surface".to_owned(), json!(member));
    Some((member.to_owned(), client))
  });
  Some(bases.collect())
}

fn surface_member(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str, shape: &ClientShape) -> Option<ClientBinding> {
  shape.surfaces.as_ref()?;
  let Binding::Init(tokens) = view.resolve_binding(scope, name)? else { return None };
  let [container, dot, member, rest @ ..] = tokens else { return None };
  let ends = match rest.first() {
    None => true,
    Some(next) => next.is_punct(";") || next.is_ident("as") || next.is_ident("satisfies"),
  };
  if !ends || !dot.is_punct(".") || container.kind != TokenKind::Ident || member.kind != TokenKind::Ident {
    return None;
  }
  surface_container(view, scope, &container.text, shape)?.remove(member.text.as_str())
}

fn is_shadowed(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str) -> bool {
  view.enclosing_scope_contains_binding(scope, name) || view.resolve_binding(scope, name).is_some()
}

fn imported_client(view: &FileView<'_>, name: &str, shape: &ClientShape) -> Option<ClientBinding> {
  let import = view.ts_import(name)?;
  if !module_matches(shape, view.facts.file.path.as_str(), import.specifier) {
    return None;
  }
  let (accepted, export) = match import.imported {
    Imported::Named(imported) => (shape.is_client_export(imported), Some(imported)),
    Imported::Default => (shape.is_client_export("default"), Some("default")),
    Imported::Namespace => (shape.exports == ClientExports::All, None),
  };
  accepted.then(|| imported_binding(view, import.specifier, export))
}

fn imported_binding(view: &FileView<'_>, specifier: &str, export: Option<&str>) -> ClientBinding {
  let mut client = ClientBinding::default();
  if let (Some(module), Some(export)) = (resolve_relative(view.facts.file.path.as_str(), specifier), export) {
    client.extra.insert("client_module".to_owned(), json!(module_stem(&module)));
    client.extra.insert("client_export".to_owned(), json!(export));
  }
  client
}

fn factory_client(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str, shape: &ClientShape) -> Option<ClientBinding> {
  let Binding::Init(tokens) = view.resolve_binding(scope, name)? else { return None };
  let tokens = skip_await(tokens);
  let head = tokens.first()?;
  if !starts_with_call(tokens, &[&head.text], ".") || is_shadowed(view, scope, &head.text) {
    return None;
  }
  let import = view.ts_import(&head.text)?;
  let imported = match import.imported {
    Imported::Named(imported) => imported,
    Imported::Default => "default",
    Imported::Namespace => return None,
  };
  if !shape.is_factory(imported) || !module_matches(shape, view.facts.file.path.as_str(), import.specifier) {
    return None;
  }
  let mut client = ClientBinding::default();
  if let Some(factory) = view
    .facts
    .references
    .iter()
    .find(|reference| reference.kind == RefKind::Call && reference.span.start_byte == head.start && reference.path.last() == Some(&head.text))
  {
    (client.base, client.base_pieces) = factory_base(view, factory);
    if let Some(ArgSummary::Object(entries)) = factory.args.first() {
      apply_header_hints(view, factory.from, entries, &mut client.hints, &mut client.hint_source);
    }
  }
  if let Some(types) = type_argument_head(tokens) {
    types_origin(view, types, &mut client.extra);
  }
  Some(client)
}

fn factory_base(view: &FileView<'_>, factory: &ReferenceFact) -> (Option<String>, Vec<Piece>) {
  let Some(ArgSummary::Object(entries)) = factory.args.first() else {
    return (None, Vec::new());
  };
  let Some(base) = entries.iter().find(|entry| entry.key.static_name().is_some_and(|name| BASE_URL_KEYS.contains(&name))) else {
    return (None, Vec::new());
  };
  base_of(view, factory.from, &base.value)
}

fn base_of(view: &FileView<'_>, scope: Option<LocalSymbolId>, value: &ArgSummary) -> (Option<String>, Vec<Piece>) {
  let mut pieces = evaluate(view, scope, value).map_or_else(|| vec![Piece::Dyn(value.clone())], |eval| eval.pieces);
  trim_trailing_slash(&mut pieces);
  let source = match pieces.first() {
    Some(Piece::Dyn(leading)) => Some(classify_base_arg(view, scope, leading)),
    _ => None,
  };
  (source, pieces)
}

fn module_stem(path: &str) -> &str {
  STRIPPED_EXTENSIONS.iter().find_map(|extension| path.strip_suffix(extension)).unwrap_or(path)
}

fn trim_trailing_slash(pieces: &mut Vec<Piece>) {
  let Some(Piece::Lit(last)) = pieces.last_mut() else { return };
  let trimmed = last.trim_end_matches('/').len();
  last.truncate(trimmed);
  if last.is_empty() {
    pieces.pop();
  }
}

fn skip_await(tokens: &[Token]) -> &[Token] {
  match tokens.split_first() {
    Some((first, rest)) if first.is_ident("await") => rest,
    _ => tokens,
  }
}

fn type_argument_head(tokens: &[Token]) -> Option<&str> {
  let [_, open, head, ..] = tokens else { return None };
  (open.is_punct("<") && head.kind == TokenKind::Ident).then_some(head.text.as_str())
}

fn types_origin(view: &FileView<'_>, types: &str, extra: &mut Attrs) {
  let Some(import) = view.ts_import(types) else { return };
  extra.insert("types_module".to_owned(), json!(import.specifier));
  if let Some(path) = resolve_relative(view.facts.file.path.as_str(), import.specifier) {
    extra.insert("types_path".to_owned(), json!(path));
  }
}

fn module_matches(shape: &ClientShape, file: &str, specifier: &str) -> bool {
  if shape.modules.iter().any(|module| module == specifier) {
    return true;
  }
  if shape.files.is_empty() {
    return false;
  }
  let Some(base) = resolve_relative(file, specifier) else { return false };
  module_candidates(&base)
    .iter()
    .any(|candidate| shape.files.iter().any(|glob| glob_matches(glob, candidate)))
}

pub(super) fn resolve_relative(file: &str, specifier: &str) -> Option<String> {
  if !(specifier == "." || specifier == ".." || specifier.starts_with("./") || specifier.starts_with("../")) {
    return None;
  }
  let mut segments: Vec<&str> = file.split('/').collect();
  segments.pop();
  for segment in specifier.split('/') {
    match segment {
      "" | "." => {}
      ".." => {
        segments.pop()?;
      }
      segment => segments.push(segment),
    }
  }
  (!segments.is_empty()).then(|| segments.join("/"))
}

fn module_candidates(base: &str) -> Vec<String> {
  let mut candidates = vec![base.to_owned()];
  candidates.extend(SOURCE_EXTENSIONS.iter().map(|extension| format!("{base}.{extension}")));
  candidates.extend(INDEX_FILES.iter().map(|index| format!("{base}/{index}")));
  candidates.extend(
    EMITTED_TS_EXTENSIONS
      .iter()
      .filter_map(|(emitted, source)| base.strip_suffix(emitted).map(|stem| format!("{stem}{source}"))),
  );
  candidates
}

fn glob_matches(pattern: &str, path: &str) -> bool {
  let pattern: Vec<&str> = pattern.split('/').collect();
  let path: Vec<&str> = path.split('/').collect();
  segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
  match pattern.split_first() {
    None => path.is_empty(),
    Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
    Some((first, rest)) => path
      .split_first()
      .is_some_and(|(segment, tail)| segment_matches(first, segment) && segments_match(rest, tail)),
  }
}

fn segment_matches(pattern: &str, text: &str) -> bool {
  let pattern: Vec<char> = pattern.chars().collect();
  let text: Vec<char> = text.chars().collect();
  let (mut p, mut t) = (0usize, 0usize);
  let mut star: Option<(usize, usize)> = None;
  while t < text.len() {
    match pattern.get(p) {
      Some('*') => {
        star = Some((p, t));
        p += 1;
      }
      Some(&c) if c == '?' || c == text[t] => {
        p += 1;
        t += 1;
      }
      _ => match star {
        Some((star_p, star_t)) => {
          p = star_p + 1;
          t = star_t + 1;
          star = Some((star_p, star_t + 1));
        }
        None => return false,
      },
    }
  }
  pattern[p..].iter().all(|c| *c == '*')
}

pub(super) fn rewrite_template(target: &str, style: PathStyle) -> String {
  let end = target.find(['?', '#']).unwrap_or(target.len());
  let (path, rest) = target.split_at(end);
  let segments: Vec<&str> = path
    .split('/')
    .map(|segment| match is_template_segment(segment, style) {
      true => PLACEHOLDER,
      false => segment,
    })
    .collect();
  format!("{}{rest}", segments.join("/"))
}

fn is_template_segment(segment: &str, style: PathStyle) -> bool {
  match style {
    PathStyle::Openapi => segment
      .strip_prefix('{')
      .and_then(|rest| rest.strip_suffix('}'))
      .is_some_and(|name| !name.is_empty() && !name.contains(['{', '}'])),
    PathStyle::Express => segment
      .strip_prefix(':')
      .is_some_and(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_rewrite_whole_template_segments_of_the_path_only() {
    assert_eq!(rewrite_template("/books/{id}/reviews/{reviewId}", PathStyle::Openapi), "/books/{}/reviews/{}");
    assert_eq!(rewrite_template("{}/books/{book-id}?expand={fields}", PathStyle::Openapi), "{}/books/{}?expand={fields}");
    assert_eq!(rewrite_template("/files/{name}.json", PathStyle::Openapi), "/files/{name}.json");
    assert_eq!(rewrite_template("/books/:id/:slug?page=:n", PathStyle::Express), "/books/{}/{}?page=:n");
    assert_eq!(
      rewrite_template("https://books.example:8443/books/:id", PathStyle::Express),
      "https://books.example:8443/books/{}"
    );
    assert_eq!(rewrite_template("/books/{}", PathStyle::Openapi), "/books/{}");
  }

  #[test]
  fn should_resolve_relative_specifiers_inside_the_repository_only() {
    assert_eq!(resolve_relative("apps/web/src/books.ts", "./api/client"), Some("apps/web/src/api/client".to_owned()));
    assert_eq!(
      resolve_relative("apps/web/src/books.ts", "../../books-api/src/openapi"),
      Some("apps/books-api/src/openapi".to_owned())
    );
    assert_eq!(resolve_relative("src/books.ts", "../../outside"), None);
    assert_eq!(resolve_relative("src/books.ts", "@acme/books-api"), None);
  }

  #[test]
  fn should_match_globs_by_segment_with_recursive_wildcards() {
    assert!(glob_matches("apps/web/src/api/*.ts", "apps/web/src/api/client.ts"));
    assert!(!glob_matches("apps/web/src/api/*.ts", "apps/web/src/api/v2/client.ts"));
    assert!(glob_matches("apps/*/src/**/client.ts", "apps/web/src/api/v2/client.ts"));
    assert!(glob_matches("apps/web/src/api/clie?t*", "apps/web/src/api/client"));
    assert!(!glob_matches("apps/web/src/api", "apps/web/src/api/client"));
  }

  #[test]
  fn should_try_source_files_behind_extensionless_and_emitted_specifiers() {
    let candidates = module_candidates("apps/web/src/api/client.js");
    assert!(candidates.contains(&"apps/web/src/api/client.ts".to_owned()));
    assert!(module_candidates("apps/web/src/api").contains(&"apps/web/src/api/index.ts".to_owned()));
  }
}
