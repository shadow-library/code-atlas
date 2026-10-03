use std::collections::BTreeMap;

use atlas_core::{EdgeKind, EntityKey, EntityKind, FileFacts, LanguageFamily, NodeRef, RefKind, Span, SymbolFact};
use serde_json::json;

use super::detector;
use super::headers::is_api_key_header;
use crate::cert::Cert;
use crate::lex::{Token, TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::view::{FileView, Imported};

pub struct AuthSchemeDetector;

detector!(AuthSchemeDetector, "auth-schemes", 1);

const PASSPORT: [&str; 1] = ["@nestjs/passport"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardKind {
  NestGuard,
  PassportGuard,
  PassportStrategy,
  NestMiddleware,
  ExpressMiddleware,
  AxumMiddleware,
  RustExtractor,
}

impl GuardKind {
  fn label(self) -> &'static str {
    match self {
      Self::NestGuard => "nest_guard",
      Self::PassportGuard => "passport_guard",
      Self::PassportStrategy => "passport_strategy",
      Self::NestMiddleware => "nest_middleware",
      Self::ExpressMiddleware => "express_middleware",
      Self::AxumMiddleware => "axum_middleware",
      Self::RustExtractor => "rust_extractor",
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
struct Scheme {
  id: &'static str,
  confidence: f32,
  detail: Option<(&'static str, String)>,
}

impl AuthSchemeDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    match facts.file.language.family() {
      LanguageFamily::Ts => facts
        .symbols
        .iter()
        .any(|symbol| !symbol.implements.is_empty() || symbol.signature.as_deref().is_some_and(|signature| signature.contains(" extends ")) || symbol.params.len() >= 3),
      LanguageFamily::Rust => facts.symbols.iter().any(|symbol| !symbol.implements.is_empty() || symbol.params.len() >= 2),
      _ => false,
    }
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    for symbol in &view.facts.symbols {
      let Some(kind) = guard_kind(view, symbol) else { continue };
      let scopes = scope_spans(view, symbol, kind);
      let delegates = delegate_guards(view, symbol);
      for delegate in &delegates {
        sink.edge(
          EdgeKind::References,
          NodeRef::Local(symbol.local),
          NodeRef::Unresolved {
            path: vec![delegate.clone()],
            from: Some(symbol.local),
          },
          symbol.name_span,
          Cert::heuristic(0.7, "composite-guard"),
          attrs([("delegates_to", json!(true)), ("guard_kind", json!(kind.label()))]),
        );
      }
      if !delegates.is_empty() {
        continue;
      }
      let schemes = match kind {
        GuardKind::PassportGuard | GuardKind::PassportStrategy => passport_scheme(view, symbol).into_iter().collect(),
        _ => schemes_from_tokens(&scopes.iter().flat_map(|span| view.span_tokens(*span).iter().cloned()).collect::<Vec<_>>()),
      };
      for scheme in schemes {
        let key = EntityKey::AuthScheme {
          repo: view.repo().clone(),
          scheme: scheme.id.to_owned(),
        };
        let cert = Cert::heuristic(scheme.confidence, "auth-signal");
        let detail = scheme.detail.as_ref().map(|(name, value)| ((*name).to_owned(), json!(value)));
        let mut entity_attrs = attrs([("guard_kind", json!(kind.label()))]);
        entity_attrs.extend(detail.clone());
        sink.entity(key.clone(), symbol.name_span, cert, entity_attrs.clone());
        sink.edge(
          EdgeKind::ImplementsAuth,
          NodeRef::Local(symbol.local),
          NodeRef::Key(key),
          symbol.name_span,
          cert,
          entity_attrs,
        );
      }
    }
  }
}

fn guard_kind(view: &FileView<'_>, symbol: &SymbolFact) -> Option<GuardKind> {
  match (view.family(), &symbol.kind) {
    (LanguageFamily::Ts, EntityKind::Class) => {
      if implements(symbol, "CanActivate") {
        return Some(GuardKind::NestGuard);
      }
      if implements(symbol, "NestMiddleware") {
        return Some(GuardKind::NestMiddleware);
      }
      match extends_call(symbol)?.as_str() {
        name if is_passport_import(view, name, "AuthGuard") => Some(GuardKind::PassportGuard),
        name if is_passport_import(view, name, "PassportStrategy") => Some(GuardKind::PassportStrategy),
        _ => None,
      }
    }
    (LanguageFamily::Ts, EntityKind::Function) => is_express_middleware(view, symbol).then_some(GuardKind::ExpressMiddleware),
    (LanguageFamily::Rust, EntityKind::Struct | EntityKind::Enum) => {
      let extractor = (implements(symbol, "FromRequestParts") || implements(symbol, "FromRequest")) && (view.rust_uses_crate("axum") || view.rust_uses_crate("actix_web"));
      extractor.then_some(GuardKind::RustExtractor)
    }
    (LanguageFamily::Rust, EntityKind::Function) => is_axum_middleware(view, symbol).then_some(GuardKind::AxumMiddleware),
    _ => None,
  }
}

fn implements(symbol: &SymbolFact, name: &str) -> bool {
  symbol.implements.iter().any(|path| path.last().is_some_and(|last| last == name))
}

fn extends_call(symbol: &SymbolFact) -> Option<String> {
  let tokens = lex(symbol.signature.as_deref()?, LanguageFamily::Ts);
  let index = tokens.iter().position(|token| token.is_ident("extends"))?;
  match tokens.get(index + 1..index + 3)? {
    [name, open] if name.kind == TokenKind::Ident && open.is_punct("(") => Some(name.text.clone()),
    _ => None,
  }
}

fn is_passport_import(view: &FileView<'_>, local: &str, name: &str) -> bool {
  view.imports_named(local, &PASSPORT, name)
}

fn is_express_middleware(view: &FileView<'_>, symbol: &SymbolFact) -> bool {
  let [request, response, next] = symbol.params.as_slice() else { return false };
  let named = |param: &atlas_core::ParamFact, names: &[&str], types: &[&str]| {
    names.contains(&param.name.as_str()) || param.type_text.as_deref().is_some_and(|type_text| types.contains(&type_text.trim()))
  };
  let typed_from_express =
    next.type_text.as_deref().is_some_and(|type_text| type_text.trim() == "NextFunction") && view.ts_import("NextFunction").is_some_and(|import| import.specifier == "express");
  let shaped = named(request, &["req", "request"], &["Request"]) && named(response, &["res", "response", "reply"], &["Response"]) && next.name == "next";
  typed_from_express || (shaped && view.ts_specifiers().any(|specifier| matches!(specifier, "express" | "koa" | "fastify")))
}

fn is_axum_middleware(view: &FileView<'_>, symbol: &SymbolFact) -> bool {
  symbol.params.iter().any(|param| {
    param.type_text.as_deref().is_some_and(|type_text| {
      let base = type_text.split('<').next().unwrap_or(type_text).trim();
      base == "axum::middleware::Next" || (base == "Next" && view.rust_path_is(&["Next".to_owned()], &["axum", "middleware", "Next"]))
    })
  })
}

fn scope_spans(view: &FileView<'_>, symbol: &SymbolFact, kind: GuardKind) -> Vec<Span> {
  if kind != GuardKind::RustExtractor {
    return vec![symbol.span];
  }
  view
    .facts
    .symbols
    .iter()
    .filter(|candidate| {
      candidate.item_path.first() == symbol.item_path.first()
        && candidate.module_suffix == symbol.module_suffix
        && candidate.item_path.get(1).is_some_and(|segment| segment.starts_with("<FromRequest"))
    })
    .map(|candidate| candidate.span)
    .collect()
}

fn delegate_guards(view: &FileView<'_>, symbol: &SymbolFact) -> Vec<String> {
  if symbol.kind != EntityKind::Class {
    return Vec::new();
  }
  let mut delegates = Vec::new();
  for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::MethodCall) {
    let [this, field, method] = reference.path.as_slice() else { continue };
    if this != "this" || method != "canActivate" || !reference.from.and_then(|local| view.class_of(local)).is_some_and(|class| class.local == symbol.local) {
      continue;
    }
    let Some(type_text) = view.field_type(symbol, field) else { continue };
    if !delegates.contains(&type_text) {
      delegates.push(type_text);
    }
  }
  delegates
}

fn passport_scheme(view: &FileView<'_>, symbol: &SymbolFact) -> Option<Scheme> {
  let tokens = lex(symbol.signature.as_deref()?, LanguageFamily::Ts);
  let open = tokens.iter().position(|token| token.is_punct("("))?;
  let args: Vec<&Token> = tokens.get(open + 1..)?.iter().take_while(|token| !token.is_punct(")")).collect();
  let strategy_name = args.iter().rev().find(|token| token.kind == TokenKind::Str).map(|token| token.text.to_ascii_lowercase());
  let strategy_module = args
    .iter()
    .find(|token| token.kind == TokenKind::Ident)
    .and_then(|token| view.ts_import(&token.text))
    .filter(|import| matches!(import.imported, Imported::Named(_) | Imported::Default))
    .map(|import| import.specifier.to_owned());
  let id = match (strategy_module.as_deref(), strategy_name.as_deref()) {
    (Some("passport-jwt" | "passport-http-bearer"), _) | (_, Some("jwt" | "bearer")) => "bearer",
    (Some("passport-headerapikey"), _) | (_, Some("api-key" | "apikey" | "headerapikey")) => "api_key",
    (Some("passport-cookie"), _) | (_, Some("cookie" | "session")) => "session_cookie",
    (Some("passport-http"), _) | (_, Some("basic")) => "basic",
    _ => return None,
  };
  Some(Scheme {
    id,
    confidence: 0.75,
    detail: strategy_name.map(|name| ("strategy", name)),
  })
}

fn schemes_from_tokens(tokens: &[Token]) -> Vec<Scheme> {
  let mut authorization = false;
  let mut bearer = false;
  let mut basic = false;
  let mut api_keys = Vec::new();
  let mut cookies = Vec::new();
  for (index, token) in tokens.iter().enumerate() {
    let previous = index.checked_sub(1).and_then(|previous| tokens.get(previous));
    match token.kind {
      TokenKind::Str | TokenKind::TemplateChunk => {
        let lower = token.text.to_ascii_lowercase();
        if lower == "authorization" {
          authorization = true;
        } else if lower.starts_with("bearer") {
          bearer = true;
        } else if lower.starts_with("basic ") || lower == "basic" {
          basic = true;
        } else if token.kind == TokenKind::Str && is_api_key_header(&lower) && !lower.contains(' ') && !api_keys.contains(&lower) {
          api_keys.push(lower);
        }
      }
      TokenKind::Ident => match token.text.as_str() {
        "AUTHORIZATION" => authorization = true,
        "authorization" if previous.is_some_and(|previous| previous.is_punct(".") || previous.is_punct("?.")) => authorization = true,
        "Bearer" => bearer = true,
        "Basic" => basic = true,
        "cookies" | "signedCookies" | "CookieJar" | "cookie_jar" => {
          if let Some(name) = cookie_name(tokens, index)
            && !cookies.contains(&name)
          {
            cookies.push(name);
          }
        }
        _ => {}
      },
      _ => {}
    }
  }
  let mut schemes = Vec::new();
  if bearer {
    schemes.push(Scheme {
      id: "bearer",
      confidence: if authorization { 0.85 } else { 0.6 },
      detail: None,
    });
  }
  if basic && authorization {
    schemes.push(Scheme {
      id: "basic",
      confidence: 0.8,
      detail: None,
    });
  }
  for header in api_keys {
    schemes.push(Scheme {
      id: "api_key",
      confidence: 0.8,
      detail: Some(("header", header)),
    });
  }
  for cookie in cookies {
    schemes.push(Scheme {
      id: "session_cookie",
      confidence: 0.8,
      detail: Some(("cookie", cookie)),
    });
  }
  dedupe(schemes)
}

fn cookie_name(tokens: &[Token], index: usize) -> Option<String> {
  let mut cursor = index + 1;
  if tokens.get(cursor).is_some_and(|token| token.is_punct("?.")) {
    cursor += 1;
  }
  let token = tokens.get(cursor)?;
  if token.is_punct("[") {
    return tokens.get(cursor + 1).filter(|name| name.kind == TokenKind::Str).map(|name| name.text.clone());
  }
  if token.is_punct(".") || token.is_punct("?.") {
    let name = tokens.get(cursor + 1)?;
    if name.is_ident("get") && tokens.get(cursor + 2).is_some_and(|open| open.is_punct("(")) {
      return tokens.get(cursor + 3).filter(|name| name.kind == TokenKind::Str).map(|name| name.text.clone());
    }
    return (name.kind == TokenKind::Ident).then(|| name.text.clone());
  }
  None
}

fn dedupe(schemes: Vec<Scheme>) -> Vec<Scheme> {
  let mut by_id: BTreeMap<&'static str, Scheme> = BTreeMap::new();
  for scheme in schemes {
    by_id.entry(scheme.id).or_insert(scheme);
  }
  by_id.into_values().collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn ids(source: &str) -> Vec<(&'static str, Option<String>)> {
    schemes_from_tokens(&lex(source, LanguageFamily::Ts))
      .into_iter()
      .map(|scheme| (scheme.id, scheme.detail.map(|(_, value)| value)))
      .collect()
  }

  #[test]
  fn should_classify_schemes_from_what_a_guard_reads() {
    assert_eq!(ids("const h = req.headers['authorization']; if (!h?.startsWith('Bearer ')) throw x;"), [("bearer", None)]);
    assert_eq!(ids("const key = request.headers['x-api-key'];"), [("api_key", Some("x-api-key".to_owned()))]);
    assert_eq!(ids("const sid = request.cookies?.['acme_session'];"), [("session_cookie", Some("acme_session".to_owned()))]);
    assert_eq!(
      ids("if (req.header('x-globex-key') !== process.env.GLOBEX_KEY) {}"),
      [("api_key", Some("x-globex-key".to_owned()))]
    );
    assert_eq!(ids("const user = context.switchToHttp().getRequest().user;"), []);
  }
}
