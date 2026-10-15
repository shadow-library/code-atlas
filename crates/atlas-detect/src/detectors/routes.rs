use atlas_core::{
  ArgSummary, AttributeFact, Attrs, EdgeKind, EntityKey, EntityKind, FileFacts, HttpMethod, LanguageFamily, NodeRef, RefKind, Severity, SymbolFact, normalize_route,
};
use serde_json::json;

use crate::cert::Cert;
use crate::families::{DecoratorFamily, FamilyOrigin, PathArgument};
use crate::lex::{TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::{StringEval, evaluate};
use crate::view::{Binding, FileView, Imported, starts_with_call};

const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";
const FAMILY_PREFIX_KIND: &str = "http_family_prefix";

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

struct Routes<'a, 'v> {
  family: &'a DecoratorFamily,
  view: &'a FileView<'v>,
}

pub(crate) fn family_applies(family: &DecoratorFamily, facts: &FileFacts) -> bool {
  facts.file.language.family() == LanguageFamily::Ts && family.applies_to(&facts.imports)
}

pub(crate) fn detect_family_routes(family: &DecoratorFamily, view: &FileView<'_>, sink: &mut Sink) {
  let routes = Routes { family, view };
  routes.run(sink);
}

impl Routes<'_, '_> {
  fn run(&self, sink: &mut Sink) {
    let view = self.view;
    for class in view.facts.symbols.iter().filter(|symbol| symbol.kind == EntityKind::Class) {
      let Some(controller) = class.attributes.iter().find(|attribute| self.is_any(attribute, &self.family.controllers)) else {
        continue;
      };
      let Some(prefixes) = self.controller_prefixes(class, controller, sink) else { continue };
      for method in view.facts.children(class.local).filter(|symbol| symbol.kind == EntityKind::Method) {
        for decorator in &method.attributes {
          let Some(http_method) = self.route_method(decorator) else { continue };
          let handler = Handler {
            class,
            method,
            decorator,
            http_method,
          };
          for path in self.decorator_paths(method, decorator, sink) {
            for prefix in &prefixes {
              self.emit_endpoint(sink, &handler, prefix, &path);
            }
          }
        }
      }
    }
    self.detect_global_prefix(sink);
  }

  fn emit_endpoint(&self, sink: &mut Sink, handler: &Handler<'_>, prefix: &Route, path: &Route) {
    let view = self.view;
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
    let strategies = self.passport_strategies(class, method);
    let params = &self.family.params;
    sink.entity(
      endpoint.clone(),
      decorator.span,
      cert,
      attrs([
        ("framework", json!(self.family.name)),
        ("handler", json!(view.qualified_name(method))),
        ("controller", json!(class.name)),
        ("route_source", json!(source)),
        ("params", self.param_names(method, &params.path)),
        ("headers", self.param_names(method, &params.headers)),
        ("body_type", self.body_type(method, &params.body)),
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
    let guards = self.guard_uses(class, "class").into_iter().chain(self.guard_uses(method, "method"));
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

  fn detect_global_prefix(&self, sink: &mut Sink) {
    let view = self.view;
    let Some(global) = &self.family.global_prefix else { return };
    let kind_name = match self.family.origin {
      FamilyOrigin::BuiltIn => GLOBAL_PREFIX_KIND,
      FamilyOrigin::Declared => FAMILY_PREFIX_KIND,
    };
    let Ok(kind) = kind_name.parse::<EntityKind>() else { return };
    let factory: Vec<&str> = global.factory.iter().map(String::as_str).collect();
    for reference in &view.facts.references {
      let [receiver, method] = reference.path.as_slice() else { continue };
      if reference.kind != RefKind::MethodCall || *method != global.method {
        continue;
      }
      let Some(ArgSummary::Str(prefix)) = reference.args.first() else { continue };
      let anchored = matches!(view.resolve_binding(reference.from, receiver), Some(Binding::Init(tokens)) if starts_with_call(tokens, &factory, "."));
      if !anchored {
        continue;
      }
      sink.entity(
        EntityKey::Other {
          kind: kind.clone(),
          repo: Some(view.repo().clone()),
          name: self.prefix_name(prefix),
        },
        reference.span,
        Cert::Syntactic,
        attrs([("framework", json!(self.family.name))]),
      );
    }
  }

  fn prefix_name(&self, prefix: &str) -> String {
    match self.family.origin {
      FamilyOrigin::BuiltIn => normalize_route(prefix),
      FamilyOrigin::Declared => format!("{}:{}", self.family.name, normalize_route(prefix)),
    }
  }

  fn is_decorator(&self, attribute: &AttributeFact, name: &str) -> bool {
    match attribute.path.as_slice() {
      [local] => self
        .view
        .ts_import(local)
        .is_some_and(|import| self.family.exports(import.specifier) && import.imported == Imported::Named(name)),
      [namespace, member] => {
        member == name
          && self
            .view
            .ts_import(namespace)
            .is_some_and(|import| self.family.exports(import.specifier) && import.imported == Imported::Namespace)
      }
      _ => false,
    }
  }

  fn is_any(&self, attribute: &AttributeFact, names: &[String]) -> bool {
    names.iter().any(|name| self.is_decorator(attribute, name))
  }

  fn route_method(&self, decorator: &AttributeFact) -> Option<HttpMethod> {
    self
      .family
      .routes
      .iter()
      .find(|route| self.is_decorator(decorator, &route.decorator))
      .map(|route| route.method)
  }

  fn controller_prefixes(&self, class: &SymbolFact, controller: &AttributeFact, sink: &mut Sink) -> Option<Vec<Route>> {
    self.path_routes(class, controller, &self.family.controller_path, sink)
  }

  fn decorator_paths(&self, method: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Vec<Route> {
    self.path_routes(method, decorator, &self.family.route_path, sink).unwrap_or_default()
  }

  fn path_routes(&self, owner: &SymbolFact, decorator: &AttributeFact, path: &PathArgument, sink: &mut Sink) -> Option<Vec<Route>> {
    match (decorator.args.get(path.index), &path.key) {
      (None, _) => Some(vec![literal_route("")]),
      (Some(ArgSummary::Object(entries)), Some(key)) => match entries.iter().find(|entry| entry.key.static_name() == Some(key.as_str())) {
        Some(entry) => self.route_values(owner, &entry.value, decorator, sink),
        None => Some(vec![literal_route("")]),
      },
      (Some(arg), _) => self.route_values(owner, arg, decorator, sink),
    }
  }

  fn route_values(&self, owner: &SymbolFact, arg: &ArgSummary, decorator: &AttributeFact, sink: &mut Sink) -> Option<Vec<Route>> {
    if let ArgSummary::Array(items) = arg {
      let routes: Vec<Route> = items.iter().filter_map(|item| self.route_value(owner, item, decorator, sink)).collect();
      return (!routes.is_empty()).then_some(routes);
    }
    self.route_value(owner, arg, decorator, sink).map(|route| vec![route])
  }

  fn route_value(&self, owner: &SymbolFact, arg: &ArgSummary, decorator: &AttributeFact, sink: &mut Sink) -> Option<Route> {
    let view = self.view;
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
    self.route_of(&eval, owner, decorator, sink)
  }

  fn route_of(&self, eval: &StringEval, owner: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Option<Route> {
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
      format!("@{} on {} has dynamic route segments", decorator.path.join("."), self.view.qualified_name(owner)),
      Some(decorator.span),
    );
    eval.has_literal_text().then(|| Route {
      route: eval.pattern("{}"),
      cert: eval.cert.capped(0.4, self.family.dynamic_route_rule()),
      source: "dynamic",
    })
  }

  fn guard_uses<'f>(&self, owner: &'f SymbolFact, level: &'static str) -> Vec<GuardUse<'f>> {
    owner
      .attributes
      .iter()
      .filter(|attribute| self.is_any(attribute, &self.family.guards))
      .flat_map(|attribute| attribute.args.iter())
      .map(|arg| GuardUse { level, arg })
      .collect()
  }

  fn passport_strategies(&self, class: &SymbolFact, method: &SymbolFact) -> Vec<String> {
    [class, method]
      .iter()
      .flat_map(|owner| owner.attributes.iter())
      .filter(|attribute| self.family.guards.iter().any(|guard| attribute.is(guard)))
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

  fn param_names(&self, method: &SymbolFact, decorators: &[String]) -> serde_json::Value {
    let names: Vec<&str> = method
      .params
      .iter()
      .flat_map(|param| param.attributes.iter())
      .filter(|attribute| self.is_any(attribute, decorators))
      .filter_map(|attribute| match attribute.args.first() {
        Some(ArgSummary::Str(name)) => Some(name.as_str()),
        _ => None,
      })
      .collect();
    if names.is_empty() { json!(null) } else { json!(names) }
  }

  fn body_type(&self, method: &SymbolFact, decorators: &[String]) -> serde_json::Value {
    method
      .params
      .iter()
      .find(|param| param.attributes.iter().any(|attribute| self.is_any(attribute, decorators)))
      .and_then(|param| param.type_text.clone())
      .map_or(json!(null), |type_text| json!(type_text))
  }
}

fn literal_route(value: &str) -> Route {
  Route {
    route: value.to_owned(),
    cert: Cert::Syntactic,
    source: "literal",
  }
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

fn is_permission_decorator(attribute: &AttributeFact) -> bool {
  let Some(name) = attribute.path.last() else { return false };
  let lower = name.to_ascii_lowercase();
  let rest = lower.strip_prefix("requires").or_else(|| lower.strip_prefix("require")).unwrap_or(&lower);
  let rest = rest.strip_suffix('s').unwrap_or(rest);
  matches!(rest, "permission" | "role" | "scope")
}
