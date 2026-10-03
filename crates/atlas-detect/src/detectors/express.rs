use std::collections::HashMap;

use atlas_core::{ArgSummary, Attrs, EdgeKind, EntityKey, FileFacts, HttpMethod, LanguageFamily, LocalSymbolId, NodeRef, RefKind, ReferenceFact, Severity, normalize_route};
use serde_json::json;

use super::{detector, handler_node};
use crate::cert::Cert;
use crate::lex::{TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::evaluate;
use crate::view::{Binding, FileView, Imported, starts_with_call};

pub struct ExpressEndpointDetector;

detector!(ExpressEndpointDetector, "http-endpoints-express", 1);

const SERVER_PACKAGES: [&str; 4] = ["express", "fastify", "koa-router", "@koa/router"];
const ROUTE_METHODS: [(&str, HttpMethod); 9] = [
  ("get", HttpMethod::Get),
  ("post", HttpMethod::Post),
  ("put", HttpMethod::Put),
  ("patch", HttpMethod::Patch),
  ("delete", HttpMethod::Delete),
  ("del", HttpMethod::Delete),
  ("head", HttpMethod::Head),
  ("options", HttpMethod::Options),
  ("all", HttpMethod::Any),
];
const MAX_MOUNT_DEPTH: usize = 4;
pub(crate) const MOUNTS_EDGE: &str = "mounts";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
  Absolute,
  Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReceiverKind {
  framework: &'static str,
  scope: Scope,
}

struct Call<'f> {
  receiver: String,
  extra_prefix: Option<String>,
  method: String,
  kind: ReceiverKind,
  reference: &'f ReferenceFact,
}

struct Mount {
  parent: String,
  parent_kind: ReceiverKind,
  prefix: String,
  child: String,
}

#[derive(Debug, Clone)]
struct Chain {
  prefix: String,
  cert: Cert,
  relative_to: Option<String>,
}

enum Handler {
  Named(NodeRef, String),
  Inline,
}

impl ExpressEndpointDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.file.language.family() == LanguageFamily::Ts
      && facts.imports.iter().any(|import| match import {
        atlas_core::ImportFact::TsImport { specifier, .. } => SERVER_PACKAGES.contains(&specifier.as_str()),
        atlas_core::ImportFact::TsRequire {
          specifier: ArgSummary::Str(specifier),
          ..
        } => SERVER_PACKAGES.contains(&specifier.as_str()),
        _ => false,
      })
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    let mut kinds: HashMap<(Option<LocalSymbolId>, String), Option<ReceiverKind>> = HashMap::new();
    let mut classify_cached = |scope: Option<LocalSymbolId>, name: &str| *kinds.entry((scope, name.to_owned())).or_insert_with(|| classify(view, scope, name));
    let mut calls = Vec::new();
    let mut mounts = Vec::new();
    for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::MethodCall) {
      let Some((receiver, extra_prefix, method)) = split_receiver(reference) else { continue };
      let Some(kind) = classify_cached(reference.from, &receiver) else { continue };
      if method == "use" {
        let (prefix, routers) = match reference.args.split_first() {
          Some((ArgSummary::Str(prefix), rest)) => (prefix.clone(), rest),
          _ => ("/".to_owned(), reference.args.as_slice()),
        };
        for router in routers {
          let ArgSummary::Path(path) = router else { continue };
          let [child] = path.as_slice() else { continue };
          if classify_cached(reference.from, child).is_some_and(|child_kind| child_kind.scope == Scope::Relative) {
            mounts.push(Mount {
              parent: receiver.clone(),
              parent_kind: kind,
              prefix: prefix.clone(),
              child: child.clone(),
            });
          } else if view.ts_import(child).is_some() {
            emit_cross_file_mount(view, sink, reference, kind, &receiver, &prefix, child);
          }
        }
        continue;
      }
      if ROUTE_METHODS.iter().any(|(name, _)| *name == method) {
        calls.push(Call {
          receiver,
          extra_prefix,
          method,
          kind,
          reference,
        });
      }
    }
    for call in &calls {
      self.emit_registration(view, sink, call, &mounts);
    }
  }

  fn emit_registration(&self, view: &FileView<'_>, sink: &mut Sink, call: &Call<'_>, mounts: &[Mount]) {
    let reference = call.reference;
    let Some(http_method) = ROUTE_METHODS.iter().find(|(name, _)| *name == call.method).map(|(_, method)| *method) else {
      return;
    };
    let chained_route = call.extra_prefix.as_ref().map(|prefix| ArgSummary::Str(prefix.clone()));
    let (routes, rest): (Vec<&ArgSummary>, &[ArgSummary]) = match (&chained_route, reference.args.split_first()) {
      (Some(route), _) => (vec![route], reference.args.as_slice()),
      (None, Some((ArgSummary::Array(items), rest))) => (items.iter().collect(), rest),
      (None, Some((single, rest))) => (vec![single], rest),
      (None, None) => return,
    };
    let handlers: Vec<&ArgSummary> = rest
      .iter()
      .flat_map(|arg| match arg {
        ArgSummary::Array(items) => items.iter().collect::<Vec<_>>(),
        other => vec![other],
      })
      .filter(|arg| !matches!(arg, ArgSummary::Object(_)))
      .collect();
    let Some((handler_arg, middleware)) = handlers.split_last() else { return };
    let handler = classify_handler(view, reference.from, handler_arg);
    let (handler_node_ref, handler_cert, handler_name) = match &handler {
      Handler::Named(node, name) => (node.clone(), handler_cert(node), name.clone()),
      Handler::Inline => (view.node(reference.from), Cert::Syntactic, "<inline>".to_owned()),
    };
    let chains = chains_for(&call.receiver, call.kind, mounts, 0);
    for route_arg in routes {
      let Some(eval) = evaluate(view, reference.from, route_arg) else {
        sink.diagnostic(
          Severity::Warning,
          "route-dynamic",
          format!("{}.{} registers a route that cannot be determined statically", call.receiver, call.method),
          Some(reference.span),
        );
        continue;
      };
      let (local_route, route_cert, route_source) = match eval.static_value() {
        Some(value) => (value, eval.cert, if eval.cert == Cert::Syntactic { "literal" } else { "constant" }),
        None if eval.has_literal_text() => (eval.pattern("{}"), eval.cert.capped(0.4, "express-dynamic-route"), "dynamic"),
        None => {
          sink.diagnostic(
            Severity::Warning,
            "route-dynamic",
            format!("{}.{} registers a route without static segments", call.receiver, call.method),
            Some(reference.span),
          );
          continue;
        }
      };
      for chain in &chains {
        let route = normalize_route(&format!("{}/{}", chain.prefix, local_route));
        let (cert, scope) = match &chain.relative_to {
          Some(_) => (route_cert.weaker(chain.cert).capped(0.5, "express-router-unmounted"), "router_relative"),
          None => (route_cert.weaker(chain.cert), "absolute"),
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
            ("framework", json!(call.kind.framework)),
            ("handler", json!(handler_name)),
            ("route_source", json!(route_source)),
            ("route_scope", json!(scope)),
            ("router", chain.relative_to.as_ref().map_or(json!(null), |router| json!(router))),
          ]),
        );
        sink.edge(
          EdgeKind::ExposesEndpoint,
          handler_node_ref.clone(),
          NodeRef::Key(endpoint.clone()),
          reference.span,
          cert.weaker(handler_cert),
          Attrs::new(),
        );
        for (order, guard) in middleware.iter().enumerate() {
          let Some((node, extra)) = middleware_node(view, reference.from, guard) else { continue };
          let mut edge_attrs = attrs([("level", json!("route")), ("order", json!(order))]);
          edge_attrs.extend(extra);
          sink.edge(EdgeKind::RequiresGuard, NodeRef::Key(endpoint.clone()), node, reference.span, cert, edge_attrs);
        }
      }
    }
  }
}

fn split_receiver(reference: &ReferenceFact) -> Option<(String, Option<String>, String)> {
  let [receiver, method] = reference.path.as_slice() else { return None };
  if !receiver.starts_with('(') {
    return Some((receiver.clone(), None, method.clone()));
  }
  let inner = receiver.strip_prefix('(')?.strip_suffix(')')?;
  let tokens = lex(inner, LanguageFamily::Ts);
  match tokens.as_slice() {
    [name, dot, route, open, path, close]
      if name.kind == TokenKind::Ident && dot.is_punct(".") && route.is_ident("route") && open.is_punct("(") && path.kind == TokenKind::Str && close.is_punct(")") =>
    {
      Some((name.text.clone(), Some(path.text.clone()), method.clone()))
    }
    _ => None,
  }
}

fn classify(view: &FileView<'_>, scope: Option<LocalSymbolId>, name: &str) -> Option<ReceiverKind> {
  match view.resolve_binding(scope, name)? {
    Binding::Param(param) => classify_type(view, param.type_text.as_deref()?),
    Binding::Init(tokens) => {
      let head = tokens.iter().find(|token| !token.is_ident("await") && !token.is_ident("new"))?;
      let is_new = tokens.first().is_some_and(|token| token.is_ident("new"));
      let import = view.ts_import(&head.text)?;
      let callee_is = |path: &[&str]| starts_with_call(tokens, path, ".");
      match (import.specifier, import.imported) {
        ("express", Imported::Default | Imported::Namespace) if callee_is(&[&head.text]) => Some(absolute("express")),
        ("express", Imported::Default | Imported::Namespace) if callee_is(&[&head.text, "Router"]) => Some(relative("express")),
        ("express", Imported::Named("Router")) if callee_is(&[&head.text]) => Some(relative("express")),
        ("fastify", Imported::Default | Imported::Namespace | Imported::Named("fastify" | "Fastify")) if callee_is(&[&head.text]) => Some(absolute("fastify")),
        ("koa-router" | "@koa/router", Imported::Default | Imported::Named("Router")) if is_new && callee_is(&[&head.text]) => Some(absolute("koa")),
        _ => None,
      }
    }
  }
}

fn classify_type(view: &FileView<'_>, type_text: &str) -> Option<ReceiverKind> {
  let type_text = type_text.trim();
  let (qualifier, name) = match type_text.rsplit_once('.') {
    Some((qualifier, name)) => (Some(qualifier), name),
    None => (None, type_text),
  };
  let name = name.split('<').next().unwrap_or(name).trim();
  let from = |specifier: &str| match qualifier {
    Some(qualifier) => view.imports_module(qualifier, &[specifier]),
    None => view.imports_named(name, &[specifier], name),
  };
  match name {
    "Express" | "Application" if from("express") => Some(absolute("express")),
    "Router" if from("express") => Some(relative("express")),
    "FastifyInstance" if from("fastify") => Some(absolute("fastify")),
    _ => None,
  }
}

fn absolute(framework: &'static str) -> ReceiverKind {
  ReceiverKind {
    framework,
    scope: Scope::Absolute,
  }
}

fn relative(framework: &'static str) -> ReceiverKind {
  ReceiverKind {
    framework,
    scope: Scope::Relative,
  }
}

fn chains_for(receiver: &str, kind: ReceiverKind, mounts: &[Mount], depth: usize) -> Vec<Chain> {
  if kind.scope == Scope::Absolute {
    return vec![Chain {
      prefix: String::new(),
      cert: Cert::Syntactic,
      relative_to: None,
    }];
  }
  let parents: Vec<&Mount> = mounts.iter().filter(|mount| mount.child == receiver).collect();
  if parents.is_empty() || depth >= MAX_MOUNT_DEPTH {
    return vec![Chain {
      prefix: String::new(),
      cert: Cert::Syntactic,
      relative_to: Some(receiver.to_owned()),
    }];
  }
  parents
    .into_iter()
    .flat_map(|mount| {
      chains_for(&mount.parent, mount.parent_kind, mounts, depth + 1).into_iter().map(move |chain| Chain {
        prefix: format!("{}/{}", chain.prefix, mount.prefix),
        cert: chain.cert.weaker(Cert::Resolved),
        relative_to: chain.relative_to,
      })
    })
    .collect()
}

fn classify_handler(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary) -> Handler {
  match arg {
    ArgSummary::Path(path) => Handler::Named(handler_node(view, path, scope), path.join(".")),
    _ => Handler::Inline,
  }
}

fn handler_cert(node: &NodeRef) -> Cert {
  match node {
    NodeRef::Unresolved { .. } => Cert::Resolved,
    _ => Cert::Syntactic,
  }
}

fn middleware_node(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary) -> Option<(NodeRef, Attrs)> {
  match arg {
    ArgSummary::Path(path) => Some((handler_node(view, path, scope), Attrs::new())),
    ArgSummary::Call { callee, args } => {
      let strategy = match args.first() {
        Some(ArgSummary::Str(strategy)) if callee.last().is_some_and(|name| name == "authenticate") => json!(strategy),
        _ => json!(null),
      };
      Some((
        NodeRef::Unresolved {
          path: callee.clone(),
          from: scope,
        },
        attrs([("strategy", strategy)]),
      ))
    }
    _ => None,
  }
}

fn emit_cross_file_mount(view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact, kind: ReceiverKind, parent: &str, prefix: &str, child: &str) {
  let Ok(edge_kind) = MOUNTS_EDGE.parse::<EdgeKind>() else { return };
  let scope = match kind.scope {
    Scope::Absolute => "absolute",
    Scope::Relative => "router_relative",
  };
  sink.edge(
    edge_kind,
    view.node(reference.from),
    NodeRef::Unresolved {
      path: vec![child.to_owned()],
      from: reference.from,
    },
    reference.span,
    Cert::heuristic(0.6, "express-cross-file-mount"),
    attrs([
      ("framework", json!(kind.framework)),
      ("mount_prefix", json!(normalize_route(prefix))),
      ("mounted_on", json!(parent)),
      ("mounted_on_scope", json!(scope)),
    ]),
  );
}
