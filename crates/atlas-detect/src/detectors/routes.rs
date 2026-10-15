use atlas_core::{
  ArgSummary, AttributeFact, Attrs, EdgeKind, EntityKey, EntityKind, FileFacts, HttpMethod, LanguageFamily, NodeRef, RefKind, Severity, SymbolFact, normalize_route,
};
use serde_json::{Map, Value, json};

use crate::cert::Cert;
use crate::families::{DecoratorFamily, FamilyOrigin, PathArgument};
use crate::lex::{Token, TokenKind, lex};
use crate::sink::{Sink, attrs};
use crate::strings::{StringEval, evaluate};
use crate::view::{Binding, FileView, Imported, starts_with_call};

const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";
const FAMILY_PREFIX_KIND: &str = "http_family_prefix";
const GUARD_DECORATOR_KIND: &str = "guard_decorator";
const UNREAD_PERMISSION_CONFIDENCE: f32 = 0.6;
const RULE_UNREAD_PERMISSION: &str = "permission-constant-unread";
const BUILT_IN_VALUE_TYPES: [&str; 16] = [
  "Array", "BigInt", "Boolean", "Date", "Error", "Map", "Number", "Object", "Promise", "Proxy", "RegExp", "Set", "String", "Symbol", "WeakMap", "WeakSet",
];

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
  target: GuardTarget<'f>,
}

enum GuardTarget<'f> {
  Class(Vec<String>),
  Decorator { name: &'f str, attribute: &'f AttributeFact },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionSource {
  Literal,
  Constant,
  UnreadConstant,
}

impl PermissionSource {
  fn label(self) -> &'static str {
    match self {
      Self::Literal => "literal",
      Self::Constant => "constant",
      Self::UnreadConstant => "unread_constant",
    }
  }
}

struct PermissionValue {
  name: String,
  cert: Cert,
  source: PermissionSource,
  constant: Option<Vec<String>>,
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
      let level_and_order = [("level", json!(guard.level)), ("order", json!(order))];
      match guard.target {
        GuardTarget::Class(path) => sink.edge(
          EdgeKind::RequiresGuard,
          NodeRef::Key(endpoint.clone()),
          NodeRef::Unresolved { path, from: Some(method.local) },
          decorator.span,
          cert,
          attrs(level_and_order),
        ),
        GuardTarget::Decorator { name, attribute } => {
          let guard = self.decorator_guard(sink, name, attribute);
          let mut edge_attrs = attrs(level_and_order);
          edge_attrs.extend(self.guard_arguments(attribute));
          sink.edge(
            EdgeKind::RequiresGuard,
            NodeRef::Key(endpoint.clone()),
            NodeRef::Key(guard),
            decorator.span,
            cert,
            edge_attrs,
          );
        }
      }
    }
    for (level, owner) in [("class", class), ("method", method)] {
      for attribute in &owner.attributes {
        if self.is_any(attribute, &self.family.permissions) {
          self.declared_permission(sink, method, owner, attribute, level);
        } else if is_permission_decorator(attribute) {
          self.named_permission(sink, method, owner, attribute, level);
        }
      }
    }
  }

  fn named_permission(&self, sink: &mut Sink, method: &SymbolFact, owner: &SymbolFact, attribute: &AttributeFact, level: &str) {
    let view = self.view;
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
      self.non_literal_permission(sink, owner, attribute);
    }
  }

  fn declared_permission(&self, sink: &mut Sink, method: &SymbolFact, owner: &SymbolFact, attribute: &AttributeFact, level: &str) {
    let view = self.view;
    let decorator_name = attribute.path.join(".");
    let args = attribute.args.iter().flat_map(|arg| match arg {
      ArgSummary::Array(items) => items.iter().collect(),
      single => vec![single],
    });
    let mut unreadable = false;
    for arg in args {
      let Some(value) = self.permission_value(arg) else {
        unreadable = true;
        continue;
      };
      let permission = EntityKey::Permission {
        repo: view.repo().clone(),
        name: value.name,
      };
      let constant = value.constant.as_ref().map(|path| json!(self.constant_name(path)));
      sink.entity(
        permission.clone(),
        attribute.span,
        value.cert,
        attrs([
          ("decorator", json!(decorator_name)),
          ("value_source", json!(value.source.label())),
          ("constant", constant.clone().unwrap_or(Value::Null)),
        ]),
      );
      sink.edge(
        EdgeKind::RequiresPermission,
        NodeRef::Local(method.local),
        NodeRef::Key(permission.clone()),
        attribute.span,
        value.cert,
        attrs([("decorator", json!(decorator_name)), ("level", json!(level)), ("constant", constant.unwrap_or(Value::Null))]),
      );
      if let (Some(path), PermissionSource::UnreadConstant) = (value.constant, value.source) {
        sink.edge(
          EdgeKind::References,
          NodeRef::Key(permission),
          NodeRef::Unresolved { path, from: Some(owner.local) },
          attribute.span,
          Cert::Syntactic,
          attrs([("permission_constant", json!(true))]),
        );
      }
    }
    if unreadable {
      self.non_literal_permission(sink, owner, attribute);
    }
  }

  fn permission_value(&self, arg: &ArgSummary) -> Option<PermissionValue> {
    let constant = match arg {
      ArgSummary::Path(path) => Some(path.clone()),
      _ => None,
    };
    if let Some(eval) = evaluate(self.view, None, arg)
      && let Some(name) = eval.static_value()
    {
      let source = if eval.cert == Cert::Syntactic {
        PermissionSource::Literal
      } else {
        PermissionSource::Constant
      };
      return Some(PermissionValue {
        name,
        cert: eval.cert,
        source,
        constant,
      });
    }
    let path = constant?;
    Some(PermissionValue {
      name: self.constant_name(&path),
      cert: Cert::heuristic(UNREAD_PERMISSION_CONFIDENCE, RULE_UNREAD_PERMISSION),
      source: PermissionSource::UnreadConstant,
      constant: Some(path),
    })
  }

  fn constant_name(&self, path: &[String]) -> String {
    let Some((head, rest)) = path.split_first() else { return String::new() };
    match self.view.ts_import(head).map(|import| import.imported) {
      Some(Imported::Namespace) if !rest.is_empty() => rest.join("."),
      Some(Imported::Named(imported)) => std::iter::once(imported).chain(rest.iter().map(String::as_str)).collect::<Vec<_>>().join("."),
      _ => path.join("."),
    }
  }

  fn non_literal_permission(&self, sink: &mut Sink, owner: &SymbolFact, attribute: &AttributeFact) {
    sink.diagnostic(
      Severity::Info,
      "permission-not-literal",
      format!("{} on {} has a non-literal argument", attribute.path.join("."), self.view.qualified_name(owner)),
      Some(attribute.span),
    );
  }

  fn decorator_guard(&self, sink: &mut Sink, name: &str, attribute: &AttributeFact) -> EntityKey {
    let view = self.view;
    let guard = EntityKey::Other {
      kind: EntityKind::Custom(GUARD_DECORATOR_KIND.to_owned()),
      repo: Some(view.repo().clone()),
      name: format!("{}:{name}", self.family.name),
    };
    let scheme = self.family.guard_scheme(name);
    sink.entity(
      guard.clone(),
      attribute.span,
      Cert::Syntactic,
      attrs([
        ("framework", json!(self.family.name)),
        ("decorator", json!(name)),
        ("auth_scheme", scheme.map_or(Value::Null, |scheme| json!(scheme))),
      ]),
    );
    let Some(scheme) = scheme else { return guard };
    let key = EntityKey::AuthScheme {
      repo: view.repo().clone(),
      scheme: scheme.to_owned(),
    };
    let declared = attrs([("guard_kind", json!("declared_decorator")), ("framework", json!(self.family.name))]);
    sink.entity(key.clone(), attribute.span, Cert::Syntactic, declared.clone());
    sink.edge(
      EdgeKind::ImplementsAuth,
      NodeRef::Key(guard.clone()),
      NodeRef::Key(key),
      attribute.span,
      Cert::Syntactic,
      declared,
    );
    guard
  }

  fn guard_arguments(&self, attribute: &AttributeFact) -> Attrs {
    let mut options = Map::new();
    let mut arguments = Vec::new();
    for arg in &attribute.args {
      match arg {
        ArgSummary::Object(_) => {
          if let Some(Value::Object(entries)) = self.static_value(arg) {
            options.extend(entries);
          }
        }
        other => arguments.extend(self.static_value(other)),
      }
    }
    attrs([
      ("options", if options.is_empty() { Value::Null } else { Value::Object(options) }),
      ("arguments", if arguments.is_empty() { Value::Null } else { Value::Array(arguments) }),
    ])
  }

  fn static_value(&self, arg: &ArgSummary) -> Option<Value> {
    match arg {
      ArgSummary::Number(text) => Some(text.parse::<serde_json::Number>().map_or_else(|_| json!(text), Value::Number)),
      ArgSummary::Bool(value) => Some(json!(value)),
      ArgSummary::Null => Some(Value::Null),
      ArgSummary::Array(items) => items.iter().map(|item| self.static_value(item)).collect::<Option<Vec<_>>>().map(Value::Array),
      ArgSummary::Object(entries) => Some(Value::Object(
        entries
          .iter()
          .filter_map(|entry| Some((entry.key.static_name()?.to_owned(), self.static_value(&entry.value)?)))
          .collect(),
      )),
      ArgSummary::Str(_) | ArgSummary::Template(_) | ArgSummary::Concat(_) | ArgSummary::Path(_) => {
        match (evaluate(self.view, None, arg).and_then(|eval| eval.static_value()), arg) {
          (Some(value), _) => Some(json!(value)),
          (None, ArgSummary::Path(path)) => Some(json!({ "binding": path.join(".") })),
          (None, _) => None,
        }
      }
      _ => None,
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

  fn guard_uses<'f>(&'f self, owner: &'f SymbolFact, level: &'static str) -> Vec<GuardUse<'f>> {
    let mut uses = Vec::new();
    for attribute in &owner.attributes {
      let Some(name) = self.family.guards.iter().find(|name| self.is_decorator(attribute, name)) else {
        continue;
      };
      let classes: Vec<Vec<String>> = match self.family.origin {
        FamilyOrigin::BuiltIn => attribute.args.iter().filter_map(guard_path).collect(),
        FamilyOrigin::Declared => attribute.args.iter().filter_map(|arg| self.guard_class(arg)).collect(),
      };
      if classes.is_empty() && self.family.origin == FamilyOrigin::Declared {
        uses.push(GuardUse {
          level,
          target: GuardTarget::Decorator { name, attribute },
        });
        continue;
      }
      uses.extend(classes.into_iter().map(|path| GuardUse {
        level,
        target: GuardTarget::Class(path),
      }));
    }
    uses
  }

  fn guard_class(&self, arg: &ArgSummary) -> Option<Vec<String>> {
    let path = guard_path(arg)?;
    let class = match path.as_slice() {
      [name] => is_class_name(name) && self.may_bind_class(name),
      [namespace, name] => is_class_name(name) && self.view.ts_import(namespace).is_some_and(|import| import.imported == Imported::Namespace),
      _ => false,
    };
    class.then_some(path)
  }

  fn may_bind_class(&self, name: &str) -> bool {
    let Some(symbol) = self.view.top_level_named(name) else { return true };
    match symbol.kind {
      EntityKind::Class => true,
      EntityKind::Constant | EntityKind::Variable => matches!(self.view.resolve_binding(None, name), Some(Binding::Init(tokens)) if creates_class(tokens)),
      _ => false,
    }
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

fn creates_class(initializer: &[Token]) -> bool {
  let (constructor, called) = match initializer.split_first() {
    Some((first, _)) if first.is_ident("class") => return true,
    Some((first, rest)) if first.is_ident("new") => (rest, true),
    _ => (initializer, false),
  };
  let Some((name, end)) = leading_name(constructor) else { return false };
  let called = called || constructor.get(end).is_some_and(|token| token.is_punct("("));
  called && is_class_name(name) && !BUILT_IN_VALUE_TYPES.contains(&name)
}

fn leading_name(tokens: &[Token]) -> Option<(&str, usize)> {
  let mut index = 0;
  loop {
    let name = tokens.get(index).filter(|token| token.kind == TokenKind::Ident)?;
    if !tokens.get(index + 1).is_some_and(|token| token.is_punct(".")) {
      return Some((&name.text, index + 1));
    }
    index += 2;
  }
}

fn is_class_name(name: &str) -> bool {
  name.starts_with(|first: char| first.is_ascii_uppercase()) && name.chars().any(|character| character.is_ascii_lowercase())
}

fn is_permission_decorator(attribute: &AttributeFact) -> bool {
  let Some(name) = attribute.path.last() else { return false };
  let lower = name.to_ascii_lowercase();
  let rest = lower.strip_prefix("requires").or_else(|| lower.strip_prefix("require")).unwrap_or(&lower);
  let rest = rest.strip_suffix('s').unwrap_or(rest);
  matches!(rest, "permission" | "role" | "scope")
}
