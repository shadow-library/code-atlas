use std::collections::BTreeSet;

use atlas_core::{
  ArgSummary, EdgeKind, EntityKey, EntityKind, FileFacts, HttpMethod, ImportFact, LanguageFamily, LocalSymbolId, NodeRef, ObjectEntry, RefKind, ReferenceFact, Severity, SymbolFact,
};
use serde_json::json;

use super::headers::{HeaderValue, classify_header, hints_from_object, hints_from_tokens};
use super::{detector, same_statement, strip_ws};
use crate::cert::Cert;
use crate::lex::{Token, TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::{Piece, StringEval, evaluate};
use crate::view::{Binding, FileView, starts_with_call};

pub struct HttpClientDetector;

detector!(HttpClientDetector, "http-clients", 1);

const AXIOS: [&str; 1] = ["axios"];
pub(crate) const DYNAMIC_URL: &str = "http.dynamic_url";
const BODYLESS_METHODS: [&str; 4] = ["get", "delete", "head", "options"];
const BODY_METHODS: [&str; 3] = ["post", "put", "patch"];
const REQWEST_METHODS: [&str; 6] = ["get", "post", "put", "patch", "delete", "head"];

#[derive(Debug, Clone, PartialEq)]
enum MethodSource {
  Literal(HttpMethod),
  Default,
  Param(String),
  Unknown,
}

impl MethodSource {
  fn method(&self) -> HttpMethod {
    match self {
      Self::Literal(method) => *method,
      Self::Default => HttpMethod::Get,
      Self::Param(_) | Self::Unknown => HttpMethod::Unknown,
    }
  }

  fn label(&self) -> &'static str {
    match self {
      Self::Literal(_) => "literal",
      Self::Default => "default",
      Self::Param(_) | Self::Unknown => "dynamic",
    }
  }
}

#[derive(Debug, Clone)]
struct Request<'f> {
  reference: &'f ReferenceFact,
  library: &'static str,
  method: MethodSource,
  url: Option<StringEval>,
  url_arg: Option<ArgSummary>,
  base: Option<String>,
  base_pieces: Vec<Piece>,
  hints: BTreeSet<String>,
  hint_source: Option<String>,
  cap: Option<Cert>,
}

impl HttpClientDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    match facts.file.language.family() {
      LanguageFamily::Ts => {
        facts.references.iter().any(|reference| {
          reference.path.last().is_some_and(|name| name == "fetch")
            || (reference.kind == RefKind::Instantiation && reference.args.iter().any(|arg| matches!(arg, ArgSummary::Object(_))))
        }) || facts
          .imports
          .iter()
          .any(|import| matches!(import, ImportFact::TsImport { specifier, .. } if specifier == "axios" || specifier == "@nestjs/axios"))
      }
      LanguageFamily::Rust => {
        facts
          .imports
          .iter()
          .any(|import| matches!(import, ImportFact::RustUse { paths, .. } if paths.iter().any(|path| path.segments.first().is_some_and(|first| first == "reqwest"))))
          || facts.references.iter().any(|reference| reference.path.first().is_some_and(|first| first == "reqwest"))
      }
      _ => false,
    }
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    let requests: Vec<Request<'_>> = view
      .facts
      .references
      .iter()
      .filter_map(|reference| match view.family() {
        LanguageFamily::Rust => rust_request(view, reference),
        _ => ts_request(view, reference),
      })
      .collect();
    let mut wrapped = BTreeSet::new();
    if view.family() == LanguageFamily::Ts {
      for request in &requests {
        let synthesized = wrapper_calls(view, request);
        if !synthesized.is_empty() {
          wrapped.insert(request.reference.span.start_byte);
        }
        for (caller, call) in synthesized {
          emit(view, sink, &call, caller);
        }
      }
      sdk_auth_options(view, sink);
    }
    for request in requests.iter().filter(|request| !wrapped.contains(&request.reference.span.start_byte)) {
      emit(view, sink, request, request.reference.from);
    }
  }
}

fn emit(view: &FileView<'_>, sink: &mut Sink, request: &Request<'_>, from: Option<LocalSymbolId>) {
  let mut pieces = request.base_pieces.clone();
  let mut cert = Cert::Syntactic;
  match &request.url {
    Some(url) => {
      cert = cert.weaker(url.cert);
      pieces.extend(url.pieces.iter().cloned());
    }
    None => {
      cert = cert.capped(0.3, "client-dynamic-url");
      pieces.push(Piece::Dyn(request.url_arg.clone().unwrap_or(ArgSummary::Null)));
    }
  }
  let combined = StringEval { pieces, cert };
  if !combined.has_literal_text() {
    let caller = from
      .and_then(|local| view.symbol(local))
      .map_or_else(|| "<file>".to_owned(), |symbol| view.qualified_name(symbol));
    sink.diagnostic(
      Severity::Info,
      DYNAMIC_URL,
      format!("{} call in {caller} has a fully dynamic URL and was not analysed", request.library),
      Some(request.reference.span),
    );
    return;
  }
  let target = combined.pattern("{}");
  let base = request.base.clone().or_else(|| base_source(view, from, &combined));
  if combined.leading_dynamic().is_some() {
    cert = cert.capped(0.7, "client-dynamic-base");
  }
  if request.method.method() == HttpMethod::Unknown {
    cert = cert.capped(0.5, "client-unknown-method");
  }
  if let Some(cap) = request.cap {
    cert = cert.weaker(cap);
  }
  let client = EntityKey::ClientCall {
    repo: view.repo().clone(),
    method: request.method.method(),
    target,
  };
  sink.entity(client.clone(), request.reference.span, cert, attrs([("library", json!(request.library))]));
  let hints: Vec<&String> = request.hints.iter().collect();
  sink.edge(
    EdgeKind::MakesRequest,
    view.node(from),
    NodeRef::Key(client),
    request.reference.span,
    cert,
    attrs([
      ("library", json!(request.library)),
      ("method_source", json!(request.method.label())),
      ("base_source", base.map_or(json!(null), |base| json!(base))),
      ("auth_hint", if hints.is_empty() { json!(null) } else { json!(hints) }),
      ("auth_hint_source", request.hint_source.as_ref().map_or(json!(null), |source| json!(source))),
    ]),
  );
}

fn ts_request<'f>(view: &FileView<'_>, reference: &'f ReferenceFact) -> Option<Request<'f>> {
  let scope = reference.from;
  match (reference.kind, reference.path.as_slice()) {
    (RefKind::Call, [name]) if name == "fetch" && is_global(view, scope, "fetch") => fetch_request(view, reference),
    (RefKind::MethodCall, [global, name]) if name == "fetch" && matches!(global.as_str(), "window" | "globalThis" | "self") => fetch_request(view, reference),
    (RefKind::Call, [name]) if view.imports_module(name, &AXIOS) => axios_config_request(view, reference, reference.args.first()?, reference.args.get(1)),
    (RefKind::MethodCall, [receiver, method]) if view.imports_module(receiver, &AXIOS) => match method.as_str() {
      "request" => axios_config_request(view, reference, &ArgSummary::Null, reference.args.first()),
      method => axios_method_request(view, reference, method, "axios", None),
    },
    (RefKind::MethodCall, [receiver, method]) if is_http_method(method) && view.ts_specifiers().any(|specifier| AXIOS.contains(&specifier)) => {
      let base = axios_instance_base(view, scope, receiver)?;
      axios_method_request(view, reference, method, "axios", Some(base))
    }
    (RefKind::MethodCall, [this, field, method]) if this == "this" && is_http_method(method) && is_nest_http_service(view, scope, field) => {
      axios_method_request(view, reference, method, "nest_http", None)
    }
    _ => None,
  }
}

fn is_http_method(method: &str) -> bool {
  BODYLESS_METHODS.contains(&method) || BODY_METHODS.contains(&method)
}

fn is_global(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str) -> bool {
  view.ts_import(name).is_none() && view.top_level_named(name).is_none() && !view.enclosing_scope_contains_binding(scope, name)
}

fn fetch_request<'f>(view: &FileView<'_>, reference: &'f ReferenceFact) -> Option<Request<'f>> {
  let scope = reference.from;
  let url = evaluate(view, scope, reference.args.first()?);
  let mut request = Request {
    reference,
    library: "fetch",
    method: MethodSource::Default,
    url,
    url_arg: reference.args.first().cloned(),
    base: None,
    base_pieces: Vec::new(),
    hints: BTreeSet::new(),
    hint_source: None,
    cap: None,
  };
  match reference.args.get(1) {
    None if request.url.is_none() && !is_string_param(view, scope, reference.args.first()) => request.method = MethodSource::Unknown,
    None => {}
    Some(ArgSummary::Object(entries)) => {
      request.method = method_from_entries(view, scope, entries);
      apply_header_options(view, scope, entries, &mut request);
    }
    Some(_) => request.method = MethodSource::Unknown,
  }
  Some(request)
}

fn axios_method_request<'f>(
  view: &FileView<'_>,
  reference: &'f ReferenceFact,
  method: &str,
  library: &'static str,
  base: Option<(Option<String>, Vec<Piece>)>,
) -> Option<Request<'f>> {
  let config_index = if BODYLESS_METHODS.contains(&method) {
    1
  } else if BODY_METHODS.contains(&method) {
    2
  } else {
    return None;
  };
  let http_method = HttpMethod::parse_lenient(method)?;
  let scope = reference.from;
  let (base_label, base_pieces) = base.unwrap_or_default();
  let mut request = Request {
    reference,
    library,
    method: MethodSource::Literal(http_method),
    url: evaluate(view, scope, reference.args.first()?),
    url_arg: reference.args.first().cloned(),
    base: base_label,
    base_pieces,
    hints: BTreeSet::new(),
    hint_source: None,
    cap: None,
  };
  if let Some(ArgSummary::Object(entries)) = reference.args.get(config_index) {
    apply_header_options(view, scope, entries, &mut request);
  }
  Some(request)
}

fn axios_config_request<'f>(view: &FileView<'_>, reference: &'f ReferenceFact, first: &ArgSummary, second: Option<&ArgSummary>) -> Option<Request<'f>> {
  let scope = reference.from;
  let (url_arg, config) = match (first, second) {
    (ArgSummary::Object(entries), _) => (entries.iter().find(|entry| entry.key.static_name() == Some("url")).map(|entry| &entry.value), Some(entries)),
    (ArgSummary::Null, Some(ArgSummary::Object(entries))) => (entries.iter().find(|entry| entry.key.static_name() == Some("url")).map(|entry| &entry.value), Some(entries)),
    (url, Some(ArgSummary::Object(entries))) => (Some(url), Some(entries)),
    (url, _) => (Some(url), None),
  };
  let mut request = Request {
    reference,
    library: "axios",
    method: MethodSource::Default,
    url: url_arg.and_then(|url| evaluate(view, scope, url)),
    url_arg: url_arg.cloned(),
    base: None,
    base_pieces: Vec::new(),
    hints: BTreeSet::new(),
    hint_source: None,
    cap: None,
  };
  if let Some(entries) = config {
    request.method = method_from_entries(view, scope, entries);
    apply_header_options(view, scope, entries, &mut request);
  }
  Some(request)
}

fn method_from_entries(view: &FileView<'_>, scope: Option<LocalSymbolId>, entries: &[ObjectEntry]) -> MethodSource {
  match entries.iter().find(|entry| entry.key.static_name() == Some("method")) {
    Some(entry) => match &entry.value {
      ArgSummary::Str(method) => HttpMethod::parse_lenient(method).map_or(MethodSource::Unknown, MethodSource::Literal),
      ArgSummary::Path(path) => match path.as_slice() {
        [name] if is_param(view, scope, name) => MethodSource::Param(name.clone()),
        _ => MethodSource::Unknown,
      },
      _ => MethodSource::Unknown,
    },
    None if entries.iter().any(|entry| matches!(entry.key, atlas_core::ObjectKey::Spread)) => MethodSource::Unknown,
    None => MethodSource::Default,
  }
}

fn apply_header_options(view: &FileView<'_>, scope: Option<LocalSymbolId>, entries: &[ObjectEntry], request: &mut Request<'_>) {
  for entry in entries {
    match (entry.key.static_name(), &entry.value) {
      (Some("headers"), ArgSummary::Object(headers)) => {
        let (hints, dynamic) = hints_from_object(headers);
        request.hints.extend(hints);
        if dynamic && request.hints.is_empty() {
          request.hints.insert("unknown".to_owned());
        }
      }
      (Some("headers"), ArgSummary::Call { callee, .. }) => match helper_hints(view, scope, callee) {
        Some((hints, source)) => {
          request.hints.extend(hints);
          request.hint_source = Some(source);
        }
        None => {
          request.hints.insert("unknown".to_owned());
        }
      },
      (Some("headers"), _) => {
        request.hints.insert("unknown".to_owned());
      }
      (Some("credentials"), ArgSummary::Str(value)) if value == "include" => {
        request.hints.insert("cookie:*".to_owned());
      }
      (Some("withCredentials"), ArgSummary::Bool(true)) => {
        request.hints.insert("cookie:*".to_owned());
      }
      (Some("auth"), ArgSummary::Object(_)) => {
        request.hints.insert("basic".to_owned());
      }
      _ => {}
    }
  }
}

fn helper_hints(view: &FileView<'_>, scope: Option<LocalSymbolId>, callee: &[String]) -> Option<(BTreeSet<String>, String)> {
  let helper = match callee {
    [this, method] if this == "this" => {
      let class = view.class_of(scope?)?;
      view.facts.children(class.local).find(|child| child.kind == EntityKind::Method && child.name == *method)?
    }
    [function] => view.top_level_named(function).filter(|symbol| symbol.kind == EntityKind::Function)?,
    _ => return None,
  };
  Some((hints_from_tokens(view.span_tokens(helper.span)), view.qualified_name(helper)))
}

fn axios_instance_base(view: &FileView<'_>, scope: Option<LocalSymbolId>, receiver: &str) -> Option<(Option<String>, Vec<Piece>)> {
  let Binding::Init(tokens) = view.resolve_binding(scope, receiver)? else { return None };
  let head = tokens.first()?;
  if !view.imports_module(&head.text, &AXIOS) || !starts_with_call(tokens, &[&head.text, "create"], ".") {
    return None;
  }
  let create = view
    .facts
    .references
    .iter()
    .find(|reference| reference.span.start_byte == head.start && reference.path.last().is_some_and(|name| name == "create"))?;
  let Some(ArgSummary::Object(entries)) = create.args.first() else {
    return Some((None, Vec::new()));
  };
  let Some(base) = entries.iter().find(|entry| entry.key.static_name() == Some("baseURL")) else {
    return Some((None, Vec::new()));
  };
  match evaluate(view, create.from, &base.value) {
    Some(eval) if eval.static_value().is_some() => Some((None, eval.pieces)),
    _ => Some((Some(classify_base_arg(view, create.from, &base.value)), vec![Piece::Dyn(base.value.clone())])),
  }
}

fn is_nest_http_service(view: &FileView<'_>, scope: Option<LocalSymbolId>, field: &str) -> bool {
  let Some(class) = scope.and_then(|local| view.class_of(local)) else { return false };
  view.field_type(class, field).is_some_and(|type_text| type_text == "HttpService") && view.imports_named("HttpService", &["@nestjs/axios"], "HttpService")
}

fn is_string_param(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: Option<&ArgSummary>) -> bool {
  let Some(ArgSummary::Path(path)) = arg else { return false };
  let [name] = path.as_slice() else { return false };
  scope
    .and_then(|local| view.symbol(local))
    .and_then(|symbol| symbol.params.iter().find(|param| param.name == *name))
    .and_then(|param| param.type_text.as_deref())
    .is_some_and(|type_text| matches!(type_text.trim(), "string" | "URL"))
}

fn is_param(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str) -> bool {
  scope
    .and_then(|local| view.symbol(local))
    .is_some_and(|symbol| symbol.params.iter().any(|param| param.name == name))
}

fn base_source(view: &FileView<'_>, scope: Option<LocalSymbolId>, url: &StringEval) -> Option<String> {
  if let Some(leading) = url.leading_dynamic() {
    return Some(classify_base_arg(view, scope, leading));
  }
  let Some(Piece::Lit(prefix)) = url.pieces.first() else { return None };
  let rest = prefix.strip_prefix("https://").or_else(|| prefix.strip_prefix("http://"))?;
  let origin_len = prefix.len() - rest.len() + rest.find('/').unwrap_or(rest.len());
  Some(format!("literal:{}", &prefix[..origin_len]))
}

fn classify_base_arg(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary) -> String {
  match arg {
    ArgSummary::Path(path) => match path.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
      ["process", "env", name] | ["import", "meta", "env", name] => format!("env:{name}"),
      ["this" | "self", ..] => "field".to_owned(),
      [name] if is_param(view, scope, name) => "param".to_owned(),
      [name] => format!("ident:{name}"),
      _ => "expr".to_owned(),
    },
    ArgSummary::Call { callee, args } => match (callee.last().map(String::as_str), args.first()) {
      (Some("var" | "var_os"), Some(ArgSummary::Str(name))) if callee.iter().any(|segment| segment == "env") => format!("env:{name}"),
      (Some("get" | "getOrThrow"), Some(ArgSummary::Str(key))) if callee.iter().any(|segment| segment.to_ascii_lowercase().contains("config")) => format!("config:{key}"),
      _ => "expr".to_owned(),
    },
    _ => "expr".to_owned(),
  }
}

fn wrapper_calls<'f>(view: &FileView<'f>, request: &Request<'f>) -> Vec<(Option<LocalSymbolId>, Request<'f>)> {
  let Some(wrapper) = request.reference.from.and_then(|local| view.symbol(local)) else {
    return Vec::new();
  };
  if !matches!(wrapper.kind, EntityKind::Function | EntityKind::Method) {
    return Vec::new();
  }
  let params: Vec<&str> = wrapper.params.iter().map(|param| param.name.as_str()).collect();
  let url_uses_param = request.url.as_ref().is_some_and(|url| {
    url
      .pieces
      .iter()
      .any(|piece| matches!(piece, Piece::Dyn(ArgSummary::Path(path)) if path.len() == 1 && params.contains(&path[0].as_str())))
  });
  let method_uses_param = matches!(&request.method, MethodSource::Param(name) if params.contains(&name.as_str()));
  if !url_uses_param && !method_uses_param {
    return Vec::new();
  }
  view
    .facts
    .references
    .iter()
    .filter(|caller| caller.from != Some(wrapper.local) && is_call_to(view, caller, wrapper))
    .filter_map(|caller| bind_wrapper(view, request, wrapper, &params, caller).map(|call| (caller.from, call)))
    .collect()
}

fn is_call_to(view: &FileView<'_>, caller: &ReferenceFact, wrapper: &SymbolFact) -> bool {
  match (caller.kind, caller.path.as_slice()) {
    (RefKind::Call, [name]) => wrapper.kind == EntityKind::Function && wrapper.parent.is_none() && *name == wrapper.name,
    (RefKind::MethodCall, [this, name]) if this == "this" && *name == wrapper.name && wrapper.kind == EntityKind::Method => {
      let caller_class = caller.from.and_then(|local| view.class_of(local)).map(|class| class.local);
      caller_class.is_some() && caller_class == wrapper.parent
    }
    _ => false,
  }
}

fn bind_wrapper<'f>(view: &FileView<'f>, request: &Request<'f>, wrapper: &SymbolFact, params: &[&str], caller: &'f ReferenceFact) -> Option<Request<'f>> {
  let argument = |name: &str| params.iter().position(|param| *param == name).and_then(|index| caller.args.get(index));
  let url = request.url.as_ref().map(|url| {
    let mut eval = StringEval {
      pieces: Vec::new(),
      cert: url.cert,
    };
    for piece in &url.pieces {
      match piece {
        Piece::Dyn(ArgSummary::Path(path)) if path.len() == 1 && argument(&path[0]).is_some() => {
          let Some(arg) = argument(&path[0]) else { continue };
          match evaluate(view, caller.from, arg) {
            Some(bound) => {
              eval.cert = eval.cert.weaker(bound.cert);
              for bound_piece in bound.pieces {
                push_piece(&mut eval, bound_piece);
              }
            }
            None => eval.pieces.push(Piece::Dyn(arg.clone())),
          }
        }
        other => push_piece(&mut eval, other.clone()),
      }
    }
    eval
  });
  if !url.as_ref().is_some_and(StringEval::has_literal_text) {
    return None;
  }
  let method = match &request.method {
    MethodSource::Param(name) => match argument(name) {
      Some(ArgSummary::Str(method)) => HttpMethod::parse_lenient(method).map_or(MethodSource::Unknown, MethodSource::Literal),
      _ => MethodSource::Unknown,
    },
    other => other.clone(),
  };
  Some(Request {
    reference: caller,
    library: request.library,
    method,
    url,
    url_arg: None,
    base: request.base.clone(),
    base_pieces: request.base_pieces.clone(),
    hints: request.hints.clone(),
    hint_source: Some(request.hint_source.clone().unwrap_or_else(|| view.qualified_name(wrapper))),
    cap: Some(Cert::heuristic(0.6, "same-file-wrapper")),
  })
}

fn push_piece(eval: &mut StringEval, piece: Piece) {
  match (eval.pieces.last_mut(), piece) {
    (Some(Piece::Lit(previous)), Piece::Lit(value)) => previous.push_str(&value),
    (_, piece) => eval.pieces.push(piece),
  }
}

fn sdk_auth_options(view: &FileView<'_>, sink: &mut Sink) {
  for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::Instantiation) {
    for arg in &reference.args {
      let ArgSummary::Object(entries) = arg else { continue };
      let Some(ArgSummary::Object(auth)) = entries.iter().find(|entry| entry.key.static_name() == Some("auth")).map(|entry| &entry.value) else {
        continue;
      };
      let Some((field, ArgSummary::Str(raw))) = auth
        .iter()
        .find(|entry| matches!(entry.key.static_name(), Some("type" | "scheme" | "kind")))
        .and_then(|entry| entry.key.static_name().map(|field| (field, &entry.value)))
      else {
        continue;
      };
      sink.edge(
        EdgeKind::References,
        view.node(reference.from),
        NodeRef::Unresolved {
          path: reference.path.clone(),
          from: reference.from,
        },
        reference.span,
        Cert::heuristic(0.6, "sdk-auth-option"),
        attrs([
          ("uses_auth_option", json!(auth_option_scheme(raw))),
          ("option", json!(format!("auth.{field}"))),
          ("option_value", json!(raw)),
        ]),
      );
    }
  }
}

fn auth_option_scheme(raw: &str) -> String {
  let normalized: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
  match normalized.as_str() {
    "bearer" | "jwt" | "token" | "bearertoken" => "bearer".to_owned(),
    "apikey" | "key" => "api_key".to_owned(),
    "basic" => "basic".to_owned(),
    "cookie" | "session" | "sessioncookie" => "session_cookie".to_owned(),
    _ => normalized,
  }
}

fn rust_request<'f>(view: &FileView<'_>, reference: &'f ReferenceFact) -> Option<Request<'f>> {
  let scope = reference.from;
  let (method, url_arg) = match reference.kind {
    RefKind::Call if view.rust_path_is(&reference.path, &["reqwest", "get"]) => (MethodSource::Literal(HttpMethod::Get), reference.args.first()?),
    RefKind::MethodCall => {
      let (method_name, receiver) = reference.path.split_last()?;
      if !(REQWEST_METHODS.contains(&method_name.as_str()) || method_name == "request") || !is_reqwest_receiver(view, scope, receiver) {
        return None;
      }
      if method_name == "request" {
        let method = match reference.args.first() {
          Some(ArgSummary::Path(path)) => path
            .last()
            .and_then(|name| HttpMethod::parse_lenient(name))
            .map_or(MethodSource::Unknown, MethodSource::Literal),
          _ => MethodSource::Unknown,
        };
        (method, reference.args.get(1)?)
      } else {
        (MethodSource::Literal(HttpMethod::parse_lenient(method_name)?), reference.args.first()?)
      }
    }
    _ => return None,
  };
  let (url, cap) = rust_url(view, scope, url_arg);
  let mut request = Request {
    reference,
    library: "reqwest",
    method,
    url,
    url_arg: Some(url_arg.clone()),
    base: None,
    base_pieces: Vec::new(),
    hints: BTreeSet::new(),
    hint_source: None,
    cap,
  };
  rust_chain_hints(view, reference, &mut request.hints);
  Some(request)
}

fn is_reqwest_receiver(view: &FileView<'_>, scope: Option<LocalSymbolId>, receiver: &[String]) -> bool {
  match receiver {
    [this, field] if this == "self" => scope
      .and_then(|local| view.class_of(local))
      .and_then(|class| view.field_type(class, field))
      .is_some_and(|type_text| is_reqwest_client_type(view, &type_text)),
    [expr] if expr.starts_with('(') => {
      let inner = expr.trim_start_matches('(').trim_end_matches(')');
      is_reqwest_client_init(view, &lex(inner, LanguageFamily::Rust))
    }
    [name] => match view.resolve_binding(scope, name) {
      Some(Binding::Param(param)) => param.type_text.as_deref().is_some_and(|type_text| is_reqwest_client_type(view, type_text)),
      Some(Binding::Init(tokens)) => is_reqwest_client_init(view, tokens),
      None => false,
    },
    _ => false,
  }
}

fn is_reqwest_client_type(view: &FileView<'_>, type_text: &str) -> bool {
  let trimmed = type_text.trim_start_matches(['&', ' ']).trim_start_matches("mut ").trim();
  if matches!(trimmed, "reqwest::Client" | "reqwest::blocking::Client") {
    return true;
  }
  trimmed == "Client" && (view.rust_path_is(&["Client".to_owned()], &["reqwest", "Client"]) || view.rust_path_is(&["Client".to_owned()], &["reqwest", "blocking", "Client"]))
}

fn is_reqwest_client_init(view: &FileView<'_>, tokens: &[Token]) -> bool {
  let qualified = [["reqwest", "Client", "new"], ["reqwest", "Client", "builder"]];
  if qualified.iter().any(|path| starts_with_call(tokens, path, "::")) {
    return true;
  }
  let imported = view.rust_path_is(&["Client".to_owned()], &["reqwest", "Client"]);
  imported && (starts_with_call(tokens, &["Client", "new"], "::") || starts_with_call(tokens, &["Client", "builder"], "::"))
}

fn rust_url(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary) -> (Option<StringEval>, Option<Cert>) {
  match arg {
    ArgSummary::Call { callee, args } if callee.last().is_some_and(|name| name == "format!") => (format_eval(args), None),
    ArgSummary::Path(path) if path.len() == 1 => {
      let Some(Binding::Init(tokens)) = view.resolve_binding(scope, &path[0]) else {
        return (None, None);
      };
      let cap = Some(Cert::heuristic(0.7, "local-url-binding"));
      match tokens {
        [literal, ..] if literal.kind == TokenKind::Str => (
          Some(StringEval {
            pieces: vec![Piece::Lit(literal.text.clone())],
            cert: Cert::Syntactic,
          }),
          cap,
        ),
        [format, bang, ..] if format.is_ident("format") && bang.is_punct("!") => {
          let macro_ref = view
            .facts
            .references
            .iter()
            .find(|reference| reference.kind == RefKind::MacroCall && reference.span.start_byte == format.start);
          (macro_ref.and_then(|reference| format_eval(&reference.args)), cap)
        }
        _ => (None, None),
      }
    }
    other => (evaluate(view, scope, other), None),
  }
}

fn format_eval(args: &[ArgSummary]) -> Option<StringEval> {
  let (ArgSummary::Str(template), rest) = args.split_first()? else { return None };
  let mut eval = StringEval {
    pieces: Vec::new(),
    cert: Cert::Syntactic,
  };
  let mut positional = rest.iter().filter(|arg| !matches!(arg, ArgSummary::Named { .. }));
  let mut chars = template.chars().peekable();
  let mut literal = String::new();
  while let Some(c) = chars.next() {
    match (c, chars.peek()) {
      ('{', Some('{')) | ('}', Some('}')) => {
        literal.push(c);
        chars.next();
      }
      ('{', _) => {
        let mut spec = String::new();
        for inner in chars.by_ref() {
          if inner == '}' {
            break;
          }
          spec.push(inner);
        }
        push_piece(&mut eval, Piece::Lit(std::mem::take(&mut literal)));
        let name = spec.split(':').next().unwrap_or_default();
        let value = if name.is_empty() {
          positional.next().cloned()
        } else if let Ok(index) = name.parse::<usize>() {
          rest.get(index).cloned()
        } else {
          Some(ArgSummary::Path(vec![name.to_owned()]))
        };
        match value {
          Some(ArgSummary::Str(text)) => push_piece(&mut eval, Piece::Lit(text)),
          Some(value) => eval.pieces.push(Piece::Dyn(value)),
          None => eval.pieces.push(Piece::Dyn(ArgSummary::Null)),
        }
      }
      _ => literal.push(c),
    }
  }
  push_piece(&mut eval, Piece::Lit(literal));
  eval.pieces.retain(|piece| !matches!(piece, Piece::Lit(value) if value.is_empty()));
  Some(eval)
}

fn rust_chain_hints(view: &FileView<'_>, request: &ReferenceFact, hints: &mut BTreeSet<String>) {
  let receiver: Vec<&str> = request.path.iter().map(String::as_str).collect();
  let prefix = format!("{}(", receiver.join("."));
  for link in view.facts.references.iter().filter(|link| {
    link.kind == RefKind::MethodCall
      && link.from == request.from
      && link.span.start_byte > request.span.start_byte
      && link
        .path
        .first()
        .is_some_and(|head| head.starts_with('(') && strip_ws(head.trim_start_matches('(')).starts_with(&strip_ws(&prefix)))
      && same_statement(view, request.span, link.span)
  }) {
    let Some(name) = link.path.last() else { continue };
    match name.as_str() {
      "bearer_auth" => {
        hints.insert("bearer".to_owned());
      }
      "basic_auth" => {
        hints.insert("basic".to_owned());
      }
      "headers" => {
        hints.insert("unknown".to_owned());
      }
      "header" => {
        let header_name = match link.args.first() {
          Some(ArgSummary::Str(name)) => Some(name.clone()),
          Some(ArgSummary::Path(path)) => path.last().map(|name| name.replace('_', "-").to_ascii_lowercase()),
          _ => None,
        };
        let value = link.args.get(1).map_or(HeaderValue::Dynamic, HeaderValue::of_arg);
        match header_name {
          Some(header_name) => hints.extend(classify_header(&header_name, &value)),
          None => {
            hints.insert("unknown".to_owned());
          }
        }
      }
      _ => {}
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_split_format_templates_into_literal_and_dynamic_pieces() {
    let eval = format_eval(&[
      ArgSummary::Str("{}/orders/{}?x={{1}}".into()),
      ArgSummary::Path(vec!["self".into(), "base_url".into()]),
      ArgSummary::Path(vec!["id".into()]),
    ])
    .unwrap();
    assert_eq!(eval.pattern("{}"), "{}/orders/{}?x={1}");
    assert_eq!(eval.leading_dynamic(), Some(&ArgSummary::Path(vec!["self".into(), "base_url".into()])));
    let named = format_eval(&[ArgSummary::Str("{base}/users/{id:?}".into())]).unwrap();
    assert_eq!(named.pattern("{}"), "{}/users/{}");
  }

  #[test]
  fn should_map_sdk_auth_option_values_to_schemes() {
    assert_eq!(auth_option_scheme("bearer"), "bearer");
    assert_eq!(auth_option_scheme("apiKey"), "api_key");
    assert_eq!(auth_option_scheme("api-key"), "api_key");
    assert_eq!(auth_option_scheme("oauth2"), "oauth2");
  }
}
