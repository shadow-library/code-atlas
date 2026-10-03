use atlas_core::{
  ArgSummary, Attrs, EdgeKind, EntityKey, FileFacts, HttpMethod, LanguageFamily, LocalSymbolId, NodeRef, RefKind, ReferenceFact, Severity, Span, SymbolFact, normalize_route,
};
use serde_json::json;

use super::express::MOUNTS_EDGE;
use super::{detector, handler_node};
use crate::cert::Cert;
use crate::lex::{Token, TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::evaluate;
use crate::view::{Binding, FileView, starts_with_call};

pub struct AxumEndpointDetector;

detector!(AxumEndpointDetector, "http-endpoints-axum", 1);

const SERVER_CRATES: [&str; 3] = ["axum", "actix_web", "rocket"];
const ATTRIBUTE_CRATES: [&str; 2] = ["actix_web", "rocket"];
const METHOD_FNS: [(&str, HttpMethod); 9] = [
  ("get", HttpMethod::Get),
  ("post", HttpMethod::Post),
  ("put", HttpMethod::Put),
  ("patch", HttpMethod::Patch),
  ("delete", HttpMethod::Delete),
  ("head", HttpMethod::Head),
  ("options", HttpMethod::Options),
  ("trace", HttpMethod::Trace),
  ("any", HttpMethod::Any),
];
const MAX_NEST_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
  Builder,
  Parameter,
}

#[derive(Debug, Clone, PartialEq)]
enum HandlerArg {
  Path(Vec<String>),
  Inline,
}

struct Nest {
  owner: Option<LocalSymbolId>,
  prefix: String,
  target: String,
}

struct AttributeRoute<'f> {
  symbol: &'f SymbolFact,
  span: Span,
  method: HttpMethod,
  route: &'f str,
  framework: &'static str,
}

struct Chain {
  prefix: String,
  cert: Cert,
}

impl AxumEndpointDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    if facts.file.language.family() != LanguageFamily::Rust {
      return false;
    }
    let uses_server_crate = facts.imports.iter().any(|import| match import {
      atlas_core::ImportFact::RustUse { paths, .. } => paths.iter().any(|path| path.segments.first().is_some_and(|first| SERVER_CRATES.contains(&first.as_str()))),
      _ => false,
    });
    uses_server_crate
      || facts
        .references
        .iter()
        .any(|reference| reference.path.first().is_some_and(|first| SERVER_CRATES.contains(&first.as_str())))
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    let mut routes = Vec::new();
    let mut nests = Vec::new();
    for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::MethodCall) {
      let Some(method) = reference.path.last() else { continue };
      if !matches!(method.as_str(), "route" | "route_service" | "nest") {
        continue;
      }
      let Some(anchor) = router_anchor(view, reference) else { continue };
      if method == "nest" {
        self.record_nest(view, sink, reference, &mut nests);
      } else {
        routes.push((reference, anchor));
      }
    }
    for (reference, anchor) in routes {
      self.emit_route(view, sink, reference, anchor, &nests);
    }
    self.emit_attribute_routes(view, sink);
  }

  fn record_nest(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact, nests: &mut Vec<Nest>) {
    let [ArgSummary::Str(prefix), ArgSummary::Call { callee, .. }, ..] = reference.args.as_slice() else {
      return;
    };
    if let [name] = callee.as_slice()
      && view.top_level_named(name).is_some_and(|symbol| symbol.kind == atlas_core::EntityKind::Function)
    {
      nests.push(Nest {
        owner: reference.from,
        prefix: prefix.clone(),
        target: name.clone(),
      });
      return;
    }
    let Ok(edge_kind) = MOUNTS_EDGE.parse::<EdgeKind>() else { return };
    sink.edge(
      edge_kind,
      view.node(reference.from),
      NodeRef::Unresolved {
        path: callee.clone(),
        from: reference.from,
      },
      reference.span,
      Cert::heuristic(0.6, "axum-cross-file-nest"),
      attrs([("framework", json!("axum")), ("mount_prefix", json!(normalize_route(prefix)))]),
    );
  }

  fn emit_route(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact, anchor: Anchor, nests: &[Nest]) {
    let Some(path_arg) = reference.args.first() else { return };
    let Some(eval) = evaluate(view, reference.from, path_arg) else {
      sink.diagnostic(
        Severity::Warning,
        "route-dynamic",
        "axum route path cannot be determined statically".to_owned(),
        Some(reference.span),
      );
      return;
    };
    let Some(local_route) = eval.static_value() else {
      sink.diagnostic(Severity::Warning, "route-dynamic", "axum route path has dynamic segments".to_owned(), Some(reference.span));
      return;
    };
    let method_routes = match reference.path.last().map(String::as_str) {
      Some("route_service") => vec![(HttpMethod::Any, handler_arg(reference.args.get(1)))],
      _ => reference.args.get(1).map(|arg| method_routes(view, arg)).unwrap_or_default(),
    };
    let owner_name = reference.from.and_then(|local| view.symbol(local)).map(|symbol| symbol.name.clone());
    let chains = chains_for(owner_name.as_deref(), nests, view, 0);
    for (http_method, handler) in method_routes {
      let (node, handler_cert, handler_name) = match &handler {
        HandlerArg::Path(path) => {
          let node = handler_node(view, path, reference.from);
          let cert = if matches!(node, NodeRef::Unresolved { .. }) { Cert::Resolved } else { Cert::Syntactic };
          (node, cert, path.join("::"))
        }
        HandlerArg::Inline => (view.node(reference.from), Cert::Syntactic, "<inline>".to_owned()),
      };
      for chain in &chains {
        let route = normalize_route(&format!("{}/{}", chain.prefix, local_route));
        let mut cert = eval.cert.weaker(chain.cert);
        let scope = match anchor {
          Anchor::Builder => "absolute",
          Anchor::Parameter => {
            cert = cert.capped(0.5, "axum-router-parameter");
            "router_relative"
          }
        };
        let endpoint = EntityKey::Endpoint {
          repo: view.repo().clone(),
          method: http_method,
          route,
        };
        sink.entity(
          endpoint.clone(),
          reference.span,
          cert,
          attrs([
            ("framework", json!("axum")),
            ("handler", json!(handler_name)),
            ("route_scope", json!(scope)),
            ("route_source", json!(if eval.cert == Cert::Syntactic { "literal" } else { "constant" })),
          ]),
        );
        sink.edge(
          EdgeKind::ExposesEndpoint,
          node.clone(),
          NodeRef::Key(endpoint),
          reference.span,
          cert.weaker(handler_cert),
          Attrs::new(),
        );
      }
    }
  }

  fn emit_attribute_routes(&self, view: &FileView<'_>, sink: &mut Sink) {
    for symbol in view
      .facts
      .symbols
      .iter()
      .filter(|symbol| matches!(symbol.kind, atlas_core::EntityKind::Function | atlas_core::EntityKind::Method))
    {
      for attribute in &symbol.attributes {
        let full: Vec<String> = match attribute.path.as_slice() {
          [single] => match view.rust_use(single) {
            Some(expanded) => expanded.to_vec(),
            None => continue,
          },
          path => path.to_vec(),
        };
        let (Some(krate), Some(name)) = (full.first(), full.last()) else { continue };
        if full.len() != 2 || !ATTRIBUTE_CRATES.contains(&krate.as_str()) {
          continue;
        }
        let methods: Vec<HttpMethod> = match name.as_str() {
          "route" => attribute
            .args
            .iter()
            .filter_map(|arg| match arg {
              ArgSummary::Named { name, value } if name == "method" => match value.as_ref() {
                ArgSummary::Str(method) => HttpMethod::parse_lenient(method),
                _ => None,
              },
              _ => None,
            })
            .collect(),
          other => METHOD_FNS
            .iter()
            .filter(|(fn_name, _)| *fn_name == other && *fn_name != "any")
            .map(|(_, method)| *method)
            .collect(),
        };
        let Some(ArgSummary::Str(raw_route)) = attribute.args.first() else { continue };
        let framework = if krate == "rocket" { "rocket" } else { "actix" };
        for method in methods {
          emit_attribute_endpoint(
            view,
            sink,
            &AttributeRoute {
              symbol,
              span: attribute.span,
              method,
              route: raw_route,
              framework,
            },
          );
        }
      }
    }
  }
}

fn emit_attribute_endpoint(view: &FileView<'_>, sink: &mut Sink, route: &AttributeRoute<'_>) {
  let endpoint = EntityKey::Endpoint {
    repo: view.repo().clone(),
    method: route.method,
    route: normalize_route(&strip_param_patterns(route.route)),
  };
  sink.entity(
    endpoint.clone(),
    route.span,
    Cert::Syntactic,
    attrs([
      ("framework", json!(route.framework)),
      ("handler", json!(view.qualified_name(route.symbol))),
      ("route_scope", json!("absolute")),
      ("route_source", json!("literal")),
    ]),
  );
  sink.edge(
    EdgeKind::ExposesEndpoint,
    NodeRef::Local(route.symbol.local),
    NodeRef::Key(endpoint),
    route.span,
    Cert::Syntactic,
    Attrs::new(),
  );
}

fn router_anchor(view: &FileView<'_>, reference: &ReferenceFact) -> Option<Anchor> {
  let [receiver, _method] = reference.path.as_slice() else { return None };
  if let Some(inner) = receiver.strip_prefix('(').and_then(|rest| rest.strip_suffix(')')) {
    let tokens = lex(inner, LanguageFamily::Rust);
    return is_router_new(view, &tokens).then_some(Anchor::Builder);
  }
  match view.resolve_binding(reference.from, receiver)? {
    Binding::Init(tokens) => is_router_new(view, tokens).then_some(Anchor::Builder),
    Binding::Param(param) => {
      let type_text = param.type_text.as_deref()?.trim_start_matches(['&', ' ']).trim_start_matches("mut ");
      let base = type_text.split('<').next().unwrap_or(type_text).trim();
      let is_axum = base == "axum::Router" || (base == "Router" && view.rust_path_is(&["Router".to_owned()], &["axum", "Router"]));
      is_axum.then_some(Anchor::Parameter)
    }
  }
}

fn is_router_new(view: &FileView<'_>, tokens: &[Token]) -> bool {
  if starts_with_call(tokens, &["axum", "Router", "new"], "::") {
    return true;
  }
  starts_with_call(tokens, &["Router", "new"], "::") && view.rust_path_is(&["Router".to_owned()], &["axum", "Router"])
}

fn method_fn(view: &FileView<'_>, callee: &[String]) -> Option<HttpMethod> {
  let name = callee.last()?;
  let method = METHOD_FNS.iter().find(|(fn_name, _)| fn_name == name)?.1;
  let full: Vec<String> = match callee {
    [single] => view.rust_use(single)?.to_vec(),
    [head, rest @ ..] => match view.rust_use(head) {
      Some(expanded) => expanded.iter().chain(rest).cloned().collect(),
      None => callee.to_vec(),
    },
    [] => return None,
  };
  (full.len() == 3 && full[0] == "axum" && full[1] == "routing").then_some(method)
}

fn method_routes(view: &FileView<'_>, arg: &ArgSummary) -> Vec<(HttpMethod, HandlerArg)> {
  match arg {
    ArgSummary::Call { callee, args } => method_fn(view, callee).map(|method| vec![(method, handler_arg(args.first()))]).unwrap_or_default(),
    ArgSummary::Other(text) => parse_method_chain(view, text),
    _ => Vec::new(),
  }
}

fn handler_arg(arg: Option<&ArgSummary>) -> HandlerArg {
  match arg {
    Some(ArgSummary::Path(path)) => HandlerArg::Path(path.clone()),
    _ => HandlerArg::Inline,
  }
}

fn parse_method_chain(view: &FileView<'_>, text: &str) -> Vec<(HttpMethod, HandlerArg)> {
  let tokens = lex(text, LanguageFamily::Rust);
  let mut routes = Vec::new();
  let mut cursor = 0usize;
  let mut first = true;
  loop {
    let mut callee = Vec::new();
    while let Some(token) = tokens.get(cursor).filter(|token| token.kind == TokenKind::Ident) {
      callee.push(token.text.clone());
      cursor += 1;
      if !tokens.get(cursor).is_some_and(|token| token.is_punct("::")) {
        break;
      }
      cursor += 1;
    }
    if callee.is_empty() || !tokens.get(cursor).is_some_and(|token| token.is_punct("(")) {
      break;
    }
    let Some(close) = matching_paren(&tokens, cursor) else { break };
    let method = if first {
      method_fn(view, &callee)
    } else {
      callee
        .last()
        .and_then(|name| METHOD_FNS.iter().find(|(fn_name, _)| fn_name == name).map(|(_, method)| *method))
    };
    if first && method.is_none() {
      return Vec::new();
    }
    if let Some(method) = method {
      routes.push((method, first_argument(tokens.get(cursor + 1..close).unwrap_or_default())));
    }
    first = false;
    cursor = close + 1;
    if !tokens.get(cursor).is_some_and(|token| token.is_punct(".")) {
      break;
    }
    cursor += 1;
  }
  routes
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
  let mut depth = 0i32;
  for (index, token) in tokens.iter().enumerate().skip(open) {
    if token.is_punct("(") {
      depth += 1;
    } else if token.is_punct(")") {
      depth -= 1;
      if depth == 0 {
        return Some(index);
      }
    }
  }
  None
}

fn first_argument(tokens: &[Token]) -> HandlerArg {
  let end = tokens.iter().position(|token| token.is_punct(",")).unwrap_or(tokens.len());
  let argument = tokens.get(..end).unwrap_or_default();
  let is_path = !argument.is_empty()
    && argument
      .iter()
      .enumerate()
      .all(|(index, token)| if index % 2 == 0 { token.kind == TokenKind::Ident } else { token.is_punct("::") });
  if !is_path {
    return HandlerArg::Inline;
  }
  HandlerArg::Path(argument.iter().filter(|token| token.kind == TokenKind::Ident).map(|token| token.text.clone()).collect())
}

fn chains_for(owner: Option<&str>, nests: &[Nest], view: &FileView<'_>, depth: usize) -> Vec<Chain> {
  let parents: Vec<&Nest> = match owner {
    Some(owner) if depth < MAX_NEST_DEPTH => nests.iter().filter(|nest| nest.target == owner).collect(),
    _ => Vec::new(),
  };
  if parents.is_empty() {
    return vec![Chain {
      prefix: String::new(),
      cert: Cert::Syntactic,
    }];
  }
  parents
    .into_iter()
    .flat_map(|nest| {
      let parent_name = nest.owner.and_then(|local| view.symbol(local)).map(|symbol| symbol.name.as_str());
      chains_for(parent_name, nests, view, depth + 1).into_iter().map(move |chain| Chain {
        prefix: format!("{}/{}", chain.prefix, nest.prefix),
        cert: chain.cert.weaker(Cert::Resolved),
      })
    })
    .collect()
}

fn strip_param_patterns(route: &str) -> String {
  route
    .split('/')
    .map(|segment| match segment.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
      Some(inner) => format!("{{{}}}", inner.split(':').next().unwrap_or(inner)),
      None => segment.to_owned(),
    })
    .collect::<Vec<_>>()
    .join("/")
}
