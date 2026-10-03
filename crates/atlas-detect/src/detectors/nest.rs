use atlas_core::{
  ArgSummary, AttributeFact, Attrs, EdgeKind, EntityKey, EntityKind, FileFacts, HttpMethod, ImportFact, LanguageFamily, NodeRef, RefKind, Severity, SymbolFact, normalize_route,
};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::lex::{TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::{StringEval, evaluate};
use crate::view::{Binding, FileView, Imported, starts_with_call};

pub struct NestEndpointDetector;

detector!(NestEndpointDetector, "http-endpoints-nest", 1);

const NEST_COMMON: [&str; 1] = ["@nestjs/common"];
const ROUTE_DECORATORS: [(&str, HttpMethod); 8] = [
  ("Get", HttpMethod::Get),
  ("Post", HttpMethod::Post),
  ("Put", HttpMethod::Put),
  ("Patch", HttpMethod::Patch),
  ("Delete", HttpMethod::Delete),
  ("All", HttpMethod::Any),
  ("Head", HttpMethod::Head),
  ("Options", HttpMethod::Options),
];
const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";

struct Route {
  route: String,
  cert: Cert,
  source: &'static str,
}

struct Handler<'f> {
  class: &'f SymbolFact,
  method: &'f SymbolFact,
  decorator: &'f AttributeFact,
  http_method: HttpMethod,
}

struct GuardUse<'f> {
  level: &'static str,
  arg: &'f ArgSummary,
}

impl NestEndpointDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.file.language.family() == LanguageFamily::Ts
      && facts
        .imports
        .iter()
        .any(|import| matches!(import, ImportFact::TsImport { specifier, .. } if specifier.starts_with("@nestjs/")))
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    for class in view.facts.symbols.iter().filter(|symbol| symbol.kind == EntityKind::Class) {
      let Some(controller) = class.attributes.iter().find(|attribute| is_nest_decorator(view, attribute, "Controller")) else {
        continue;
      };
      let Some(prefixes) = controller_prefixes(view, class, controller, sink) else { continue };
      for method in view.facts.children(class.local).filter(|symbol| symbol.kind == EntityKind::Method) {
        for decorator in &method.attributes {
          let Some(http_method) = ROUTE_DECORATORS
            .iter()
            .find(|(name, _)| is_nest_decorator(view, decorator, name))
            .map(|(_, method)| *method)
          else {
            continue;
          };
          let handler = Handler {
            class,
            method,
            decorator,
            http_method,
          };
          for path in decorator_paths(view, method, decorator, sink) {
            for prefix in &prefixes {
              self.emit_endpoint(view, sink, &handler, prefix, &path);
            }
          }
        }
      }
    }
    self.detect_global_prefix(view, sink);
  }

  fn emit_endpoint(&self, view: &FileView<'_>, sink: &mut Sink, handler: &Handler<'_>, prefix: &Route, path: &Route) {
    let Handler {
      class,
      method,
      decorator,
      http_method,
    } = *handler;
    let route = normalize_route(&format!("{}/{}", prefix.route, path.route));
    let cert = prefix.cert.weaker(path.cert);
    let source = if prefix.source == "dynamic" || path.source == "dynamic" {
      "dynamic"
    } else if prefix.source == "constant" || path.source == "constant" {
      "constant"
    } else {
      "literal"
    };
    let endpoint = EntityKey::Endpoint {
      repo: view.repo().clone(),
      method: http_method,
      route,
    };
    let strategies = passport_strategies(class, method);
    sink.entity(
      endpoint.clone(),
      decorator.span,
      cert,
      attrs([
        ("framework", json!("nestjs")),
        ("handler", json!(view.qualified_name(method))),
        ("controller", json!(class.name)),
        ("route_source", json!(source)),
        ("params", param_names(view, method, "Param")),
        ("headers", param_names(view, method, "Headers")),
        ("body_type", body_type(view, method)),
        ("passport_strategies", if strategies.is_empty() { json!(null) } else { json!(strategies) }),
      ]),
    );
    sink.edge(
      EdgeKind::ExposesEndpoint,
      NodeRef::Local(method.local),
      NodeRef::Key(endpoint.clone()),
      decorator.span,
      cert,
      Attrs::new(),
    );
    let guards = guard_uses(view, class, "class").into_iter().chain(guard_uses(view, method, "method"));
    for (order, guard) in guards.enumerate() {
      let Some(path) = guard_path(guard.arg) else { continue };
      sink.edge(
        EdgeKind::RequiresGuard,
        NodeRef::Key(endpoint.clone()),
        NodeRef::Unresolved { path, from: Some(method.local) },
        decorator.span,
        cert,
        attrs([("level", json!(guard.level)), ("order", json!(order))]),
      );
    }
    for (level, owner) in [("class", class), ("method", method)] {
      for attribute in owner.attributes.iter().filter(|attribute| is_permission_decorator(attribute)) {
        for name in attribute.args.iter().filter_map(|arg| match arg {
          ArgSummary::Str(value) => Some(value.clone()),
          _ => None,
        }) {
          let permission = EntityKey::Permission { repo: view.repo().clone(), name };
          let decorator_name = attribute.path.join(".");
          sink.entity(permission.clone(), attribute.span, Cert::Syntactic, attrs([("decorator", json!(decorator_name))]));
          sink.edge(
            EdgeKind::RequiresPermission,
            NodeRef::Local(method.local),
            NodeRef::Key(permission),
            attribute.span,
            Cert::Syntactic,
            attrs([("decorator", json!(decorator_name)), ("level", json!(level))]),
          );
        }
        if attribute.args.iter().any(|arg| !matches!(arg, ArgSummary::Str(_))) {
          sink.diagnostic(
            Severity::Info,
            "permission-not-literal",
            format!("{} on {} has a non-literal argument", attribute.path.join("."), view.qualified_name(owner)),
            Some(attribute.span),
          );
        }
      }
    }
  }

  fn detect_global_prefix(&self, view: &FileView<'_>, sink: &mut Sink) {
    let Ok(kind) = GLOBAL_PREFIX_KIND.parse::<EntityKind>() else { return };
    for reference in &view.facts.references {
      let [receiver, method] = reference.path.as_slice() else { continue };
      if reference.kind != RefKind::MethodCall || method != "setGlobalPrefix" {
        continue;
      }
      let Some(ArgSummary::Str(prefix)) = reference.args.first() else { continue };
      let anchored = matches!(view.resolve_binding(reference.from, receiver), Some(Binding::Init(tokens)) if starts_with_call(tokens, &["NestFactory", "create"], "."));
      if !anchored {
        continue;
      }
      sink.entity(
        EntityKey::Other {
          kind: kind.clone(),
          repo: Some(view.repo().clone()),
          name: normalize_route(prefix),
        },
        reference.span,
        Cert::Syntactic,
        attrs([("framework", json!("nestjs"))]),
      );
    }
  }
}

pub(crate) fn is_nest_decorator(view: &FileView<'_>, attribute: &AttributeFact, name: &str) -> bool {
  match attribute.path.as_slice() {
    [local] => view.imports_named(local, &NEST_COMMON, name),
    [namespace, member] => {
      member == name
        && view
          .ts_import(namespace)
          .is_some_and(|import| NEST_COMMON.contains(&import.specifier) && import.imported == Imported::Namespace)
    }
    _ => false,
  }
}

fn controller_prefixes(view: &FileView<'_>, class: &SymbolFact, controller: &AttributeFact, sink: &mut Sink) -> Option<Vec<Route>> {
  let routes: Vec<Route> = match controller.args.first() {
    None => vec![literal_route("")],
    Some(ArgSummary::Object(entries)) => match entries.iter().find(|entry| entry.key.static_name() == Some("path")) {
      Some(entry) => route_values(view, class, &entry.value, controller, sink)?,
      None => vec![literal_route("")],
    },
    Some(arg) => route_values(view, class, arg, controller, sink)?,
  };
  Some(routes)
}

fn decorator_paths(view: &FileView<'_>, method: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Vec<Route> {
  match decorator.args.first() {
    None => vec![literal_route("")],
    Some(arg) => route_values(view, method, arg, decorator, sink).unwrap_or_default(),
  }
}

fn route_values(view: &FileView<'_>, owner: &SymbolFact, arg: &ArgSummary, decorator: &AttributeFact, sink: &mut Sink) -> Option<Vec<Route>> {
  if let ArgSummary::Array(items) = arg {
    let routes: Vec<Route> = items.iter().filter_map(|item| route_value(view, owner, item, decorator, sink)).collect();
    return (!routes.is_empty()).then_some(routes);
  }
  route_value(view, owner, arg, decorator, sink).map(|route| vec![route])
}

fn route_value(view: &FileView<'_>, owner: &SymbolFact, arg: &ArgSummary, decorator: &AttributeFact, sink: &mut Sink) -> Option<Route> {
  let Some(eval) = evaluate(view, None, arg) else {
    sink.diagnostic(
      Severity::Warning,
      "route-dynamic",
      format!(
        "@{} on {} has a route that cannot be determined statically",
        decorator.path.join("."),
        view.qualified_name(owner)
      ),
      Some(decorator.span),
    );
    return None;
  };
  route_of(&eval, view, owner, decorator, sink)
}

fn route_of(eval: &StringEval, view: &FileView<'_>, owner: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Option<Route> {
  if let Some(value) = eval.static_value() {
    let source = if eval.cert == Cert::Syntactic { "literal" } else { "constant" };
    return Some(Route {
      route: value,
      cert: eval.cert,
      source,
    });
  }
  sink.diagnostic(
    Severity::Info,
    "route-partially-dynamic",
    format!("@{} on {} has dynamic route segments", decorator.path.join("."), view.qualified_name(owner)),
    Some(decorator.span),
  );
  eval.has_literal_text().then(|| Route {
    route: eval.pattern("{}"),
    cert: eval.cert.capped(0.4, "nest-dynamic-route"),
    source: "dynamic",
  })
}

fn literal_route(value: &str) -> Route {
  Route {
    route: value.to_owned(),
    cert: Cert::Syntactic,
    source: "literal",
  }
}

fn guard_uses<'f>(view: &FileView<'_>, owner: &'f SymbolFact, level: &'static str) -> Vec<GuardUse<'f>> {
  owner
    .attributes
    .iter()
    .filter(|attribute| is_nest_decorator(view, attribute, "UseGuards"))
    .flat_map(|attribute| attribute.args.iter())
    .map(|arg| GuardUse { level, arg })
    .collect()
}

fn guard_path(arg: &ArgSummary) -> Option<Vec<String>> {
  match arg {
    ArgSummary::Path(path) => Some(path.clone()),
    ArgSummary::Call { callee, .. } => Some(callee.clone()),
    ArgSummary::Other(text) => {
      let tokens = lex(text, LanguageFamily::Ts);
      match tokens.as_slice() {
        [new, name, ..] if new.is_ident("new") && name.kind == TokenKind::Ident => Some(vec![name.text.clone()]),
        _ => None,
      }
    }
    _ => None,
  }
}

fn passport_strategies(class: &SymbolFact, method: &SymbolFact) -> Vec<String> {
  [class, method]
    .iter()
    .flat_map(|owner| owner.attributes.iter())
    .filter(|attribute| attribute.is("UseGuards"))
    .flat_map(|attribute| attribute.args.iter())
    .filter_map(|arg| match arg {
      ArgSummary::Call { callee, args } if callee.last().is_some_and(|name| name == "AuthGuard") => match args.first() {
        Some(ArgSummary::Str(strategy)) => Some(strategy.clone()),
        _ => None,
      },
      _ => None,
    })
    .collect()
}

pub(crate) fn is_permission_decorator(attribute: &AttributeFact) -> bool {
  let Some(name) = attribute.path.last() else { return false };
  let lower = name.to_ascii_lowercase();
  let rest = lower.strip_prefix("requires").or_else(|| lower.strip_prefix("require")).unwrap_or(&lower);
  let rest = rest.strip_suffix('s').unwrap_or(rest);
  matches!(rest, "permission" | "role" | "scope")
}

fn param_names(view: &FileView<'_>, method: &SymbolFact, decorator: &str) -> serde_json::Value {
  let names: Vec<&str> = method
    .params
    .iter()
    .flat_map(|param| param.attributes.iter())
    .filter(|attribute| is_nest_decorator(view, attribute, decorator))
    .filter_map(|attribute| match attribute.args.first() {
      Some(ArgSummary::Str(name)) => Some(name.as_str()),
      _ => None,
    })
    .collect();
  if names.is_empty() { json!(null) } else { json!(names) }
}

fn body_type(view: &FileView<'_>, method: &SymbolFact) -> serde_json::Value {
  method
    .params
    .iter()
    .find(|param| param.attributes.iter().any(|attribute| is_nest_decorator(view, attribute, "Body")))
    .and_then(|param| param.type_text.clone())
    .map_or(json!(null), |type_text| json!(type_text))
}
