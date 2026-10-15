use std::collections::BTreeSet;

use atlas_core::{
  ArgSummary, AttributeFact, Attrs, EdgeKind, EntityKey, EntityKind, FileFacts, HttpMethod, LanguageFamily, NodeRef, ObjectEntry, ObjectKey, RefKind, Severity, SymbolFact,
  normalize_route,
};
use serde_json::{Map, Value, json};

use crate::cert::Cert;
use crate::families::{AccessMode, ClassMarker, DecoratorFamily, FamilyOrigin, OptionMerge, OptionScheme, OptionSchemes, PathArgument, UNREAD_OPTIONS};
use crate::lex::{Token, TokenKind, lex};
use crate::objects::const_object;
use crate::sink::{Sink, attrs};
use crate::strings::{StringEval, evaluate};
use crate::view::{Binding, FileView, Imported, starts_with_call};

const GLOBAL_PREFIX_KIND: &str = "http_global_prefix";
const FAMILY_PREFIX_KIND: &str = "http_family_prefix";
const GUARD_DECORATOR_KIND: &str = "guard_decorator";
const UNREAD_PERMISSION_CONFIDENCE: f32 = 0.6;
const RULE_UNREAD_PERMISSION: &str = "permission-constant-unread";
const OVERRIDABLE_OPTION_CONFIDENCE: f32 = 0.5;
const RULE_PERMISSION_OPTION_SPREAD: &str = "permission-option-spread";
const RULE_PERMISSION_MERGE_UNREAD: &str = "permission-option-merge-unread";
const UNCERTAIN_SCHEME_CONFIDENCE: f32 = 0.5;
const RULE_OPTION_SCHEME_UNREAD: &str = "option-scheme-unread";
const RULE_OPTION_SCHEME_SEVERAL: &str = "option-scheme-several-keys";
const DEFAULT_OPTIONS: &str = "default";
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
  position: usize,
  target: GuardTarget<'f>,
  enforcement: Enforcement,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enforcement {
  Required,
  Optional,
}

impl<'f> GuardUse<'f> {
  fn required(level: &'static str, position: usize, target: GuardTarget<'f>) -> Self {
    Self {
      level,
      position,
      target,
      enforcement: Enforcement::Required,
    }
  }

  fn is_family_auth(&self) -> bool {
    !matches!(self.target, GuardTarget::Class(_))
  }

  fn family_decorator(&self) -> Option<&'f str> {
    match self.target {
      GuardTarget::Class(_) => None,
      GuardTarget::Decorator { name, .. } | GuardTarget::ImpliedByPermission { name, .. } => Some(name),
    }
  }

  fn under_marker(self, marked: bool, mode: Option<AccessMode>) -> Option<Self> {
    if !marked || !self.is_family_auth() {
      return Some(self);
    }
    match mode {
      Some(AccessMode::Optional) => Some(Self {
        enforcement: Enforcement::Optional,
        ..self
      }),
      Some(AccessMode::Public) | None => None,
    }
  }
}

enum GuardTarget<'f> {
  Class(Vec<String>),
  Decorator { name: &'f str, attribute: &'f AttributeFact },
  ImpliedByPermission { name: &'f str, attribute: &'f AttributeFact },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
  Own,
  Merged,
}

struct Access<'f> {
  mode: AccessMode,
  decorator: &'f str,
  option: Option<&'f str>,
  level: &'static str,
  via: Via,
  position: usize,
}

impl Access<'_> {
  fn marker(&self) -> Value {
    let mut marker = json!({ "decorator": self.decorator, "level": self.level });
    if let Some(option) = self.option {
      marker["option"] = json!(option);
    }
    if self.via == Via::Merged {
      marker["merged"] = json!(true);
    }
    marker
  }

  fn unapplied(&self, reason: &str, value: &str) -> Value {
    let mut marker = self.marker();
    marker["mode"] = json!(self.mode.as_str());
    marker["applied"] = json!(false);
    marker[reason] = json!(value);
    marker
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause {
  Method,
  DecoratorOrder,
}

impl Cause {
  fn as_str(self) -> &'static str {
    match self {
      Self::Method => "method",
      Self::DecoratorOrder => "decorator_order",
    }
  }
}

enum Marking<'f> {
  Unmarked,
  Applied(Access<'f>),
  Contested(Access<'f>, Cause),
  Overridden(Access<'f>, Cause),
  Limited(Access<'f>),
}

enum LevelMark<'f> {
  Unmarked,
  Marked(Access<'f>),
  ContestedByOrder(Access<'f>),
  OverriddenByOrder(Access<'f>),
}

enum OptionMark<'f> {
  Marked(Access<'f>),
  Uncertain(Access<'f>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
  Nothing,
  ClassGuards,
  AllGuards,
}

impl Reach {
  fn covers(self, level: &str) -> bool {
    match self {
      Self::Nothing => false,
      Self::ClassGuards => level == "class",
      Self::AllGuards => true,
    }
  }
}

impl Marking<'_> {
  fn mode(&self) -> Option<AccessMode> {
    match self {
      Self::Applied(access) => Some(access.mode),
      Self::Unmarked | Self::Contested(..) | Self::Overridden(..) | Self::Limited(_) => None,
    }
  }

  fn reach(&self) -> Reach {
    match self {
      Self::Applied(access) if access.level == "method" => Reach::AllGuards,
      Self::Applied(_) | Self::Contested(_, Cause::Method) => Reach::ClassGuards,
      Self::Contested(_, Cause::DecoratorOrder) | Self::Overridden(..) | Self::Limited(_) | Self::Unmarked => Reach::Nothing,
    }
  }

  fn marker(&self) -> Value {
    match self {
      Self::Unmarked => Value::Null,
      Self::Applied(access) => access.marker(),
      Self::Contested(access, cause) => access.unapplied("contested_by", cause.as_str()),
      Self::Overridden(access, cause) => access.unapplied("overridden_by", cause.as_str()),
      Self::Limited(access) => access.unapplied("scope", "decorator"),
    }
  }
}

#[derive(Clone, Copy)]
enum ClassOption {
  Kept,
  Replaced,
  Unread,
}

enum KeyState<'f> {
  Unset,
  Unknown,
  Set { values: Vec<&'f ArgSummary>, exact: bool },
}

struct SchemeChoice {
  suffix: String,
  option: Value,
  schemes: Vec<String>,
  cert: Cert,
}

impl SchemeChoice {
  fn unread(mapping: &OptionSchemes) -> Self {
    let schemes = mapping.options.iter().flat_map(|option| &option.schemes).chain(&mapping.default);
    Self {
      suffix: UNREAD_OPTIONS.to_owned(),
      option: json!(UNREAD_OPTIONS),
      schemes: sorted_unique(schemes),
      cert: Cert::heuristic(UNCERTAIN_SCHEME_CONFIDENCE, RULE_OPTION_SCHEME_UNREAD),
    }
  }

  fn of(mapping: &OptionSchemes, present: &[&OptionScheme]) -> Self {
    match present {
      [] => Self {
        suffix: DEFAULT_OPTIONS.to_owned(),
        option: json!(DEFAULT_OPTIONS),
        schemes: mapping.default.clone(),
        cert: Cert::Syntactic,
      },
      [one] => Self {
        suffix: one.option.clone(),
        option: json!(one.option),
        schemes: one.schemes.clone(),
        cert: Cert::Syntactic,
      },
      several => {
        let keys: Vec<&str> = several.iter().map(|option| option.option.as_str()).collect();
        let first = sorted_unique(several[0].schemes.iter());
        let agree = several.iter().all(|option| sorted_unique(option.schemes.iter()) == first);
        Self {
          suffix: keys.join("+"),
          option: json!(keys),
          schemes: sorted_unique(several.iter().flat_map(|option| &option.schemes)),
          cert: match agree {
            true => Cert::Syntactic,
            false => Cert::heuristic(UNCERTAIN_SCHEME_CONFIDENCE, RULE_OPTION_SCHEME_SEVERAL),
          },
        }
      }
    }
  }
}

struct PermissionArg<'a> {
  arg: &'a ArgSummary,
  cap: Option<Cert>,
}

#[derive(Clone, Copy)]
struct PermissionSite<'f> {
  method: &'f SymbolFact,
  owner: &'f SymbolFact,
  attribute: &'f AttributeFact,
  level: &'f str,
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
          let Some((http_method, paths)) = self.handler_routes(method, decorator, sink) else {
            continue;
          };
          let handler = Handler {
            class,
            method,
            decorator,
            http_method,
          };
          for path in paths {
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
    let (marking, guards) = self.access_and_guards(class, method);
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
        ("auth", marking.mode().map_or(Value::Null, |mode| json!(mode.as_str()))),
        ("auth_marker", marking.marker()),
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
    for (order, guard) in guards.into_iter().enumerate() {
      let enforcement = match guard.enforcement {
        Enforcement::Required => Value::Null,
        Enforcement::Optional => json!(AccessMode::Optional.as_str()),
      };
      let level_and_order = [("level", json!(guard.level)), ("order", json!(order)), ("auth", enforcement)];
      let (guard, edge_attrs) = match guard.target {
        GuardTarget::Class(path) => (NodeRef::Unresolved { path, from: Some(method.local) }, attrs(level_and_order)),
        GuardTarget::Decorator { name, attribute } => {
          let mut edge_attrs = attrs(level_and_order);
          edge_attrs.extend(self.guard_arguments(attribute));
          let choice = self.family.option_schemes(name).map(|mapping| self.scheme_choice(mapping, class, method));
          (NodeRef::Key(self.decorator_guard(sink, name, attribute, None, choice)), edge_attrs)
        }
        GuardTarget::ImpliedByPermission { name, attribute } => (NodeRef::Key(self.decorator_guard(sink, name, attribute, Some("permission"), None)), attrs(level_and_order)),
      };
      sink.edge(EdgeKind::RequiresGuard, NodeRef::Key(endpoint.clone()), guard, decorator.span, cert, edge_attrs);
    }
    for (level, owner) in [("class", class), ("method", method)] {
      for attribute in &owner.attributes {
        if self.is_any(attribute, &self.family.permissions) {
          self.declared_permission(sink, method, owner, attribute, level);
        } else if let Some((name, keys)) = self.guard_permission_keys(attribute) {
          let site = PermissionSite { method, owner, attribute, level };
          let inherited = level == "class" && self.carries(method, name);
          match (inherited, self.family.semantics(name).options) {
            (false, _) => self.option_permissions(sink, &site, keys, |_, _| ClassOption::Kept),
            (true, OptionMerge::Override) => {}
            (true, OptionMerge::DeepMerge) => self.option_permissions(sink, &site, keys, |key, value| self.merged_permission(method, name, key, value)),
          }
        } else if is_permission_decorator(attribute) {
          self.named_permission(sink, method, owner, attribute, level);
        }
      }
    }
  }

  fn access_and_guards<'f>(&'f self, class: &'f SymbolFact, method: &'f SymbolFact) -> (Marking<'f>, Vec<GuardUse<'f>>) {
    let class_guards = self.guard_uses(class, "class");
    let method_guards = self.guard_uses(method, "method");
    let options = self.option_marks(class, method);
    let option_mode = |decorator: &str| {
      options
        .iter()
        .filter_map(|mark| match mark {
          OptionMark::Marked(access) if access.decorator == decorator => Some(access.mode),
          _ => None,
        })
        .min()
    };
    let self_marked = |guard: &GuardUse<'_>| guard.family_decorator().and_then(option_mode).is_some();
    let route = match self.level_mark(method, "method", &method_guards, &self_marked) {
      LevelMark::Marked(access) => Marking::Applied(access),
      LevelMark::ContestedByOrder(access) => Marking::Contested(access, Cause::DecoratorOrder),
      LevelMark::OverriddenByOrder(access) => Marking::Overridden(access, Cause::DecoratorOrder),
      LevelMark::Unmarked => match self.level_mark(class, "class", &class_guards, &self_marked) {
        LevelMark::Marked(access) => self.class_marking(access, &method_guards, &self_marked),
        LevelMark::ContestedByOrder(access) => Marking::Contested(access, Cause::DecoratorOrder),
        LevelMark::OverriddenByOrder(access) => Marking::Overridden(access, Cause::DecoratorOrder),
        LevelMark::Unmarked => Marking::Unmarked,
      },
    };
    let reach = route.reach();
    let mode = route.mode();
    let guards: Vec<GuardUse<'f>> = class_guards
      .into_iter()
      .chain(method_guards)
      .filter_map(|guard| {
        let covered = reach.covers(guard.level);
        let guard = guard.under_marker(covered, mode)?;
        match guard.family_decorator().and_then(option_mode) {
          Some(own) => guard.under_marker(true, Some(own)),
          None => Some(guard),
        }
      })
      .collect();
    (recorded_marking(route, options, &guards), guards)
  }

  fn class_marking<'f>(&self, access: Access<'f>, method_guards: &[GuardUse<'_>], self_marked: &dyn Fn(&GuardUse<'_>) -> bool) -> Marking<'f> {
    let family_guards: Vec<&str> = method_guards.iter().filter(|guard| !self_marked(guard)).filter_map(GuardUse::family_decorator).collect();
    if family_guards.is_empty() {
      return Marking::Applied(access);
    }
    match family_guards.iter().any(|decorator| self.method_wins(decorator)) {
      true => Marking::Overridden(access, Cause::Method),
      false => Marking::Contested(access, Cause::Method),
    }
  }

  fn method_wins(&self, decorator: &str) -> bool {
    self.family.semantics(decorator).class_marker == ClassMarker::MethodWins
  }

  fn level_mark<'f>(&'f self, owner: &'f SymbolFact, level: &'static str, guards: &[GuardUse<'_>], self_marked: &dyn Fn(&GuardUse<'_>) -> bool) -> LevelMark<'f> {
    let marks: Vec<Access<'f>> = owner
      .attributes
      .iter()
      .enumerate()
      .flat_map(|(position, attribute)| self.decorator_marks(attribute, level, position))
      .collect();
    let family_guards: Vec<&GuardUse<'_>> = guards.iter().filter(|guard| guard.family_decorator().is_some() && !self_marked(guard)).collect();
    let winning = family_guards
      .iter()
      .filter(|guard| guard.family_decorator().is_some_and(|decorator| self.method_wins(decorator)))
      .count();
    if winning == 0 {
      return marks.into_iter().min_by_key(|access| access.mode).map_or(LevelMark::Unmarked, LevelMark::Marked);
    }
    let Some(topmost) = marks.into_iter().min_by_key(|access| access.position) else {
      return LevelMark::Unmarked;
    };
    if winning < family_guards.len() {
      return LevelMark::ContestedByOrder(topmost);
    }
    match family_guards.iter().all(|guard| topmost.position < guard.position) {
      true => LevelMark::Marked(topmost),
      false => LevelMark::OverriddenByOrder(topmost),
    }
  }

  fn carries(&self, owner: &SymbolFact, decorator: &str) -> bool {
    owner.attributes.iter().any(|attribute| self.is_decorator(attribute, decorator))
  }

  fn decorator_marks<'f>(&'f self, attribute: &AttributeFact, level: &'static str, position: usize) -> Vec<Access<'f>> {
    let markers = self.family.access();
    AccessMode::ALL
      .into_iter()
      .filter_map(|mode| {
        let decorator = markers.decorators(mode).iter().find(|name| self.is_decorator(attribute, name))?;
        Some(Access {
          mode,
          decorator,
          option: None,
          level,
          via: Via::Own,
          position,
        })
      })
      .collect()
  }

  fn option_marks<'f>(&'f self, class: &SymbolFact, method: &SymbolFact) -> Vec<OptionMark<'f>> {
    let mut marks = Vec::new();
    for option in &self.family.access().options {
      for key in &option.keys {
        let access = |level: &'static str, via: Via| Access {
          mode: option.mode,
          decorator: &option.decorator,
          option: Some(key),
          level,
          via,
          position: 0,
        };
        let on_method = self.carries(method, &option.decorator);
        if on_method && self.sets_marker(method, &option.decorator, key) {
          marks.push(OptionMark::Marked(access("method", Via::Own)));
          continue;
        }
        if !self.sets_marker(class, &option.decorator, key) {
          continue;
        }
        if !on_method {
          marks.push(OptionMark::Marked(access("class", Via::Own)));
          continue;
        }
        if self.family.semantics(&option.decorator).options == OptionMerge::Override {
          continue;
        }
        match self.option_state(method, &option.decorator, key) {
          KeyState::Unset => marks.push(OptionMark::Marked(access("class", Via::Merged))),
          KeyState::Unknown => marks.push(OptionMark::Uncertain(access("class", Via::Own))),
          KeyState::Set { .. } => {}
        }
      }
    }
    marks
  }

  fn sets_marker(&self, owner: &SymbolFact, decorator: &str, key: &str) -> bool {
    owner
      .attributes
      .iter()
      .filter(|attribute| self.is_decorator(attribute, decorator))
      .flat_map(|attribute| &attribute.args)
      .any(|arg| matches!(arg, ArgSummary::Object(entries) if matches!(OptionKey::of(entries, key), OptionKey::Fixed(ArgSummary::Bool(true)))))
  }

  fn option_state<'f>(&self, owner: &'f SymbolFact, decorator: &str, key: &str) -> KeyState<'f> {
    let mut values = Vec::new();
    let mut exact = true;
    let mut unknown = false;
    let args = owner
      .attributes
      .iter()
      .filter(|attribute| self.is_decorator(attribute, decorator))
      .flat_map(|attribute| &attribute.args);
    for arg in args {
      match arg {
        ArgSummary::Object(entries) => match OptionKey::of(entries, key) {
          OptionKey::Fixed(value) => values.push(value),
          OptionKey::Overridable(value) => {
            values.push(value);
            exact = false;
          }
          OptionKey::SpreadOnly => unknown = true,
          OptionKey::Absent => {}
        },
        ArgSummary::Path(_) | ArgSummary::Call { .. } | ArgSummary::Other(_) | ArgSummary::Truncated => unknown = true,
        _ => {}
      }
    }
    match (values.is_empty(), unknown) {
      (true, false) => KeyState::Unset,
      (true, true) => KeyState::Unknown,
      (false, _) => KeyState::Set { values, exact: exact && !unknown },
    }
  }

  fn effective_state<'f>(&self, decorator: &str, key: &str, class: &'f SymbolFact, method: &'f SymbolFact) -> KeyState<'f> {
    if !self.carries(method, decorator) {
      return self.option_state(class, decorator, key);
    }
    let method_state = self.option_state(method, decorator, key);
    let merges = self.carries(class, decorator) && self.family.semantics(decorator).options == OptionMerge::DeepMerge;
    match method_state {
      KeyState::Unset if merges => self.option_state(class, decorator, key),
      state => state,
    }
  }

  fn scheme_choice(&self, mapping: &OptionSchemes, class: &SymbolFact, method: &SymbolFact) -> SchemeChoice {
    let mut present = Vec::new();
    for option in &mapping.options {
      match self.effective_state(&mapping.decorator, &option.option, class, method) {
        KeyState::Unset => {}
        KeyState::Set { values, exact: true } => {
          if values.last().is_some_and(|value| is_truthy(value)) {
            present.push(option);
          }
        }
        KeyState::Set { exact: false, .. } | KeyState::Unknown => return SchemeChoice::unread(mapping),
      }
    }
    SchemeChoice::of(mapping, &present)
  }

  fn merged_permission(&self, method: &SymbolFact, decorator: &str, key: &str, class_value: &ArgSummary) -> ClassOption {
    match self.option_state(method, decorator, key) {
      KeyState::Unset => ClassOption::Kept,
      KeyState::Unknown => ClassOption::Unread,
      KeyState::Set { values, .. } => {
        let concatenated = matches!(class_value, ArgSummary::Array(_)) && values.iter().all(|value| matches!(value, ArgSummary::Array(_)));
        match concatenated {
          true => ClassOption::Kept,
          false => ClassOption::Replaced,
        }
      }
    }
  }

  fn guard_permission_keys(&self, attribute: &AttributeFact) -> Option<(&str, &[String])> {
    let name = self.family.guards.iter().find(|name| self.is_decorator(attribute, name))?;
    let keys = self.family.guard_permission_keys(name);
    (!keys.is_empty()).then_some((name.as_str(), keys))
  }

  fn option_permissions(&self, sink: &mut Sink, site: &PermissionSite<'_>, keys: &[String], inherit: impl Fn(&str, &ArgSummary) -> ClassOption) {
    for entries in site.attribute.args.iter().filter_map(|arg| match arg {
      ArgSummary::Object(entries) => Some(entries),
      _ => None,
    }) {
      for key in keys {
        let (value, spread) = match OptionKey::of(entries, key) {
          OptionKey::Fixed(value) => (value, None),
          OptionKey::Overridable(value) => (value, Some(Cert::heuristic(OVERRIDABLE_OPTION_CONFIDENCE, RULE_PERMISSION_OPTION_SPREAD))),
          OptionKey::Absent | OptionKey::SpreadOnly => continue,
        };
        let merge = match inherit(key, value) {
          ClassOption::Replaced => continue,
          ClassOption::Kept => None,
          ClassOption::Unread => Some(Cert::heuristic(OVERRIDABLE_OPTION_CONFIDENCE, RULE_PERMISSION_MERGE_UNREAD)),
        };
        let cap = match (spread, merge) {
          (Some(spread), Some(merge)) => Some(spread.weaker(merge)),
          (spread, merge) => spread.or(merge),
        };
        let args = flattened(value).into_iter().map(|arg| PermissionArg { arg, cap }).collect();
        self.emit_permissions(sink, site, args, Some(key));
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
    let args = attribute.args.iter().flat_map(flattened).map(|arg| PermissionArg { arg, cap: None }).collect();
    let site = PermissionSite { method, owner, attribute, level };
    self.emit_permissions(sink, &site, args, None);
  }

  fn emit_permissions(&self, sink: &mut Sink, site: &PermissionSite<'_>, args: Vec<PermissionArg<'_>>, option: Option<&str>) {
    let view = self.view;
    let PermissionSite { method, owner, attribute, level } = *site;
    let decorator_name = attribute.path.join(".");
    let option = option.map_or(Value::Null, |option| json!(option));
    let mut unreadable = false;
    for PermissionArg { arg, cap } in args {
      let Some(value) = self.permission_value(arg) else {
        unreadable = true;
        continue;
      };
      let cert = cap.map_or(value.cert, |cap| value.cert.weaker(cap));
      let permission = EntityKey::Permission {
        repo: view.repo().clone(),
        name: value.name,
      };
      let constant = value.constant.as_ref().map(|path| json!(self.constant_name(path)));
      sink.entity(
        permission.clone(),
        attribute.span,
        cert,
        attrs([
          ("decorator", json!(decorator_name)),
          ("option", option.clone()),
          ("value_source", json!(value.source.label())),
          ("constant", constant.clone().unwrap_or(Value::Null)),
        ]),
      );
      sink.edge(
        EdgeKind::RequiresPermission,
        NodeRef::Local(method.local),
        NodeRef::Key(permission.clone()),
        attribute.span,
        cert,
        attrs([
          ("decorator", json!(decorator_name)),
          ("option", option.clone()),
          ("level", json!(level)),
          ("constant", constant.unwrap_or(Value::Null)),
        ]),
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

  fn decorator_guard(&self, sink: &mut Sink, name: &str, attribute: &AttributeFact, role: Option<&str>, choice: Option<SchemeChoice>) -> EntityKey {
    let view = self.view;
    let (key_name, schemes, implements, option) = match choice {
      None => (
        format!("{}:{name}", self.family.name),
        self.family.decorator_schemes(name).to_vec(),
        Cert::Syntactic,
        Value::Null,
      ),
      Some(choice) => (format!("{}:{name}:{}", self.family.name, choice.suffix), choice.schemes, choice.cert, choice.option),
    };
    let guard = EntityKey::Other {
      kind: EntityKind::Custom(GUARD_DECORATOR_KIND.to_owned()),
      repo: Some(view.repo().clone()),
      name: key_name,
    };
    sink.entity(
      guard.clone(),
      attribute.span,
      Cert::Syntactic,
      attrs([
        ("framework", json!(self.family.name)),
        ("decorator", json!(name)),
        ("auth_schemes", if schemes.is_empty() { Value::Null } else { json!(schemes) }),
        ("role", role.map_or(Value::Null, |role| json!(role))),
        ("scheme_option", option),
      ]),
    );
    for scheme in schemes {
      let key = EntityKey::AuthScheme {
        repo: view.repo().clone(),
        scheme,
      };
      let declared = attrs([("guard_kind", json!("declared_decorator")), ("framework", json!(self.family.name))]);
      sink.entity(key.clone(), attribute.span, Cert::Syntactic, declared.clone());
      sink.edge(
        EdgeKind::ImplementsAuth,
        NodeRef::Key(guard.clone()),
        NodeRef::Key(key),
        attribute.span,
        implements,
        declared,
      );
    }
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

  fn handler_routes(&self, method: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Option<(HttpMethod, Vec<Route>)> {
    match self.route_method(decorator) {
      Some(http_method) => Some((http_method, self.decorator_paths(method, decorator, sink))),
      None => self.option_route(method, decorator, sink),
    }
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
    for (position, attribute) in owner.attributes.iter().enumerate() {
      if let Some(implied) = self.family.permission_schemes().iter().find(|implied| self.is_decorator(attribute, &implied.decorator)) {
        uses.push(GuardUse::required(
          level,
          position,
          GuardTarget::ImpliedByPermission {
            name: &implied.decorator,
            attribute,
          },
        ));
        continue;
      }
      let Some(name) = self.family.guards.iter().find(|name| self.is_decorator(attribute, name)) else {
        continue;
      };
      let classes: Vec<Vec<String>> = match self.family.origin {
        FamilyOrigin::BuiltIn => attribute.args.iter().filter_map(guard_path).collect(),
        FamilyOrigin::Declared => attribute.args.iter().filter_map(|arg| self.guard_class(arg)).collect(),
      };
      if classes.is_empty() && self.family.origin == FamilyOrigin::Declared {
        uses.push(GuardUse::required(level, position, GuardTarget::Decorator { name, attribute }));
        continue;
      }
      uses.extend(classes.into_iter().map(|path| GuardUse::required(level, position, GuardTarget::Class(path))));
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

impl Routes<'_, '_> {
  fn option_route(&self, owner: &SymbolFact, decorator: &AttributeFact, sink: &mut Sink) -> Option<(HttpMethod, Vec<Route>)> {
    let options = self.family.route_options()?;
    if !self.is_any(decorator, &options.decorators) {
      return None;
    }
    let Some((entries, object_cert)) = self.option_entries(decorator) else {
      self.option_diagnostic(
        sink,
        owner,
        decorator,
        "takes options that are neither an object literal nor a const object literal declared in this file",
      );
      return None;
    };
    let method = OptionKey::of(&entries, &options.method);
    let Some((http_method, method_cert)) = method.value().and_then(|arg| self.option_method(arg)) else {
      let message = format!("has no HTTP method that can be determined statically under `{}`", options.method);
      self.option_diagnostic(sink, owner, decorator, &message);
      return None;
    };
    let path = OptionKey::of(&entries, &options.path);
    let overridable: Vec<&str> = [(&method, options.method.as_str()), (&path, options.path.as_str())]
      .into_iter()
      .filter(|(key, _)| key.is_overridable())
      .map(|(_, name)| name)
      .collect();
    let mut cap = object_cert.weaker(method_cert);
    if !overridable.is_empty() {
      cap = cap.capped(0.5, "option-route-spread");
      sink.diagnostic(
        Severity::Info,
        "route-options-spread",
        format!(
          "@{} on {} takes `{}` from options a spread may override",
          decorator.path.join("."),
          self.view.qualified_name(owner),
          overridable.join("`, `")
        ),
        Some(decorator.span),
      );
    }
    let routes = match path.value() {
      None => vec![literal_route("")],
      Some(arg) => self.route_values(owner, arg, decorator, sink)?,
    };
    Some((http_method, routes.into_iter().map(|route| capped_route(route, object_cert, cap)).collect()))
  }

  fn option_entries(&self, decorator: &AttributeFact) -> Option<(Vec<ObjectEntry>, Cert)> {
    match decorator.args.first()? {
      ArgSummary::Object(entries) => Some((entries.clone(), Cert::Syntactic)),
      ArgSummary::Path(path) => match path.as_slice() {
        [name] => const_object(self.view, None, name).map(|entries| (entries, Cert::Resolved)),
        _ => None,
      },
      _ => None,
    }
  }

  fn option_method(&self, arg: &ArgSummary) -> Option<(HttpMethod, Cert)> {
    let parse = |text: &str| HttpMethod::parse_lenient(text).filter(|method| *method != HttpMethod::Unknown);
    match arg {
      ArgSummary::Str(text) => parse(text).map(|method| (method, Cert::Syntactic)),
      ArgSummary::Path(path) => match path.as_slice() {
        [name] => parse(&self.view.module_const_string(name)?).map(|method| (method, Cert::Resolved)),
        [.., member] => parse(member).map(|method| (method, Cert::heuristic(0.8, "option-route-method-member"))),
        [] => None,
      },
      _ => None,
    }
  }

  fn option_diagnostic(&self, sink: &mut Sink, owner: &SymbolFact, decorator: &AttributeFact, problem: &str) {
    sink.diagnostic(
      Severity::Warning,
      "route-options-unresolved",
      format!("@{} on {} {problem}", decorator.path.join("."), self.view.qualified_name(owner)),
      Some(decorator.span),
    );
  }
}

enum OptionKey<'e> {
  Fixed(&'e ArgSummary),
  Overridable(&'e ArgSummary),
  Absent,
  SpreadOnly,
}

impl<'e> OptionKey<'e> {
  fn of(entries: &'e [ObjectEntry], key: &str) -> Self {
    let last = entries.iter().rposition(|entry| entry.key.static_name() == Some(key));
    let spread_after = |from: usize| entries[from..].iter().any(|entry| entry.key == ObjectKey::Spread);
    match last {
      Some(index) if spread_after(index + 1) => Self::Overridable(&entries[index].value),
      Some(index) => Self::Fixed(&entries[index].value),
      None if spread_after(0) => Self::SpreadOnly,
      None => Self::Absent,
    }
  }

  fn value(&self) -> Option<&'e ArgSummary> {
    match self {
      Self::Fixed(value) | Self::Overridable(value) => Some(value),
      Self::Absent | Self::SpreadOnly => None,
    }
  }

  fn is_overridable(&self) -> bool {
    matches!(self, Self::Overridable(_) | Self::SpreadOnly)
  }
}

fn capped_route(route: Route, object_cert: Cert, cap: Cert) -> Route {
  let source = match (route.source, object_cert) {
    ("literal", Cert::Syntactic) => "literal",
    ("literal", _) => "constant",
    (source, _) => source,
  };
  Route {
    route: route.route,
    cert: route.cert.weaker(cap),
    source,
  }
}

fn recorded_marking<'f>(route: Marking<'f>, options: Vec<OptionMark<'f>>, guards: &[GuardUse<'_>]) -> Marking<'f> {
  if matches!(route, Marking::Applied(_) | Marking::Contested(..)) {
    return route;
  }
  let mut marked = Vec::new();
  for mark in options {
    match mark {
      OptionMark::Uncertain(access) => return Marking::Contested(access, Cause::Method),
      OptionMark::Marked(access) => marked.push(access),
    }
  }
  let required = guards.iter().any(|guard| guard.enforcement == Enforcement::Required);
  let optional = guards.iter().any(|guard| guard.enforcement == Enforcement::Optional);
  let strongest = match optional {
    true => marked.into_iter().max_by_key(|access| access.mode),
    false => marked.into_iter().min_by_key(|access| access.mode),
  };
  match (strongest, required) {
    (None, _) => route,
    (Some(access), false) => Marking::Applied(access),
    (Some(_), true) if matches!(route, Marking::Overridden(..)) => route,
    (Some(access), true) => Marking::Limited(access),
  }
}

fn sorted_unique<'s>(schemes: impl Iterator<Item = &'s String>) -> Vec<String> {
  schemes.cloned().collect::<BTreeSet<String>>().into_iter().collect()
}

fn is_truthy(value: &ArgSummary) -> bool {
  match value {
    ArgSummary::Bool(value) => *value,
    ArgSummary::Null => false,
    ArgSummary::Str(text) => !text.is_empty(),
    ArgSummary::Number(text) => text.parse::<f64>().ok().is_none_or(|number| number != 0.0),
    ArgSummary::Path(path) => !matches!(path.as_slice(), [name] if name == "undefined"),
    _ => true,
  }
}

fn flattened(arg: &ArgSummary) -> Vec<&ArgSummary> {
  match arg {
    ArgSummary::Array(items) => items.iter().collect(),
    single => vec![single],
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
