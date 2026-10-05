use std::rc::Rc;

use atlas_core::{ChainHead, ChainStep, EntityKind, Modifier, ReceiverChain, ReceiverHint};

use crate::engine::{
  CONSTRUCTOR_CONFIDENCE, DEREF_GUARD_CONFIDENCE, DispatchReach, Found, Guard, Heuristic, Hit, Miss, Outcome, RULE_CONSTRUCTOR, RULE_UNWRAP_OPTION, RULE_UNWRAP_RESULT, Recv,
  RecvType, Resolver, UNWRAP_CONFIDENCE, unique, weakest,
};
use crate::rust::{Ns, RustScope, qualified_trait};
use crate::table::SymRef;
use crate::typetext::{TypeShape, is_deref_wrapper, rust_type_parts, rust_type_shape, strip_reference};

const MAX_TYPE_DEPTH: usize = 8;
const UNWRAPPING_METHODS: [&str; 5] = ["unwrap", "expect", "unwrap_or", "unwrap_or_else", "unwrap_or_default"];
const WRAPPER_KEEPING_METHODS: [&str; 4] = ["as_ref", "as_mut", "cloned", "copied"];
const SELF_RETURNING_STD_METHODS: [&str; 2] = ["clone", "to_owned"];
const DEREF_KEEPING_METHODS: [&str; 2] = ["clone", "as_ref"];
const BORROW_TRAITS: [(&str, &str); 2] = [("borrow", "Borrow"), ("borrow_mut", "BorrowMut")];
const STD_ROOTS: [&str; 3] = ["std", "core", "alloc"];
const LOCK_GUARD_PACKAGES: [&str; 5] = ["std", "core", "alloc", "parking_lot", "tokio"];
const CELL_GUARD_PACKAGES: [&str; 3] = ["std", "core", "alloc"];
const GUARDS: [(&str, &str, &[&str]); 5] = [
  ("MutexGuard", "deref:MutexGuard", &LOCK_GUARD_PACKAGES),
  ("RwLockReadGuard", "deref:RwLockReadGuard", &LOCK_GUARD_PACKAGES),
  ("RwLockWriteGuard", "deref:RwLockWriteGuard", &LOCK_GUARD_PACKAGES),
  ("Ref", "deref:Ref", &CELL_GUARD_PACKAGES),
  ("RefMut", "deref:RefMut", &CELL_GUARD_PACKAGES),
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SelfType {
  Scope,
  Concrete(SymRef),
  Opaque,
}

#[derive(Debug, Clone)]
enum Generics {
  Bounded(Vec<(String, Vec<String>)>),
  Opaque(Vec<String>),
}

impl Generics {
  fn contains(&self, name: &str) -> bool {
    match self {
      Self::Bounded(generics) => generics.iter().any(|(generic, _)| generic == name),
      Self::Opaque(names) => names.iter().any(|generic| generic == name),
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) struct TypeText {
  text: String,
  scope: RustScope,
  generics: Generics,
  bound: Rc<Vec<(String, TypeText)>>,
  self_type: SelfType,
}

impl TypeText {
  fn declared(text: &str, scope: &RustScope) -> Self {
    Self {
      text: text.to_owned(),
      scope: scope.clone(),
      generics: Generics::Bounded(scope.generics.clone()),
      bound: Rc::new(Vec::new()),
      self_type: SelfType::Scope,
    }
  }

  fn with_text(&self, text: &str) -> Self {
    Self {
      text: text.to_owned(),
      ..self.clone()
    }
  }

  fn names_generic(&self, name: &str) -> bool {
    self.generics.contains(name) || self.bound.iter().any(|(bound, _)| bound == name)
  }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Wrapper {
  Option,
  Result,
  Guard(&'static str),
  Future,
}

#[derive(Debug, Clone)]
pub(crate) enum Cursor {
  Value(Recv),
  Wrapped {
    wrapper: Wrapper,
    package: String,
    inner: Box<TypeText>,
    heuristic: Option<Heuristic>,
  },
}

impl Cursor {
  fn unknown() -> Self {
    Self::Value(Recv::unknown())
  }

  fn weaken(self, heuristic: Option<Heuristic>) -> Self {
    match self {
      Self::Value(mut receiver) => {
        receiver.heuristic = weakest(receiver.heuristic, heuristic);
        Self::Value(receiver)
      }
      Self::Wrapped {
        wrapper,
        package,
        inner,
        heuristic: carried,
      } => Self::Wrapped {
        wrapper,
        package,
        inner,
        heuristic: weakest(carried, heuristic),
      },
    }
  }
}

fn rule(confidence: f32, rule: &'static str) -> Option<Heuristic> {
  Some(Heuristic { confidence, rule })
}

fn unwrap_rule(wrapper: Wrapper) -> Option<Heuristic> {
  match wrapper {
    Wrapper::Option => rule(UNWRAP_CONFIDENCE, RULE_UNWRAP_OPTION),
    _ => rule(UNWRAP_CONFIDENCE, RULE_UNWRAP_RESULT),
  }
}

fn unify(param: &str, arg: &str, generics: &[&str], bindings: &mut Vec<(String, String)>, conflicts: &mut Vec<String>, depth: usize) {
  if depth > MAX_TYPE_DEPTH {
    return;
  }
  let (param, arg) = (strip_reference(param), strip_reference(arg));
  if generics.contains(&param) {
    match bindings.iter().find(|(name, _)| name == param) {
      Some((_, existing)) if existing != arg => conflicts.push(param.to_owned()),
      Some(_) => {}
      None => bindings.push((param.to_owned(), arg.to_owned())),
    }
    return;
  }
  let (Some((param_path, param_args)), Some((arg_path, arg_args))) = (rust_type_parts(param), rust_type_parts(arg)) else {
    return;
  };
  if param_path.last() != arg_path.last() || param_args.len() != arg_args.len() {
    return;
  }
  for (param, arg) in param_args.iter().zip(&arg_args) {
    unify(param, arg, generics, bindings, conflicts, depth + 1);
  }
}

impl Resolver<'_> {
  pub(crate) fn rust_chain_call(&self, scope: &RustScope, chain: &ReceiverChain, method: &str, caller: Option<SymRef>) -> Outcome {
    let mut cursor = self.chain_head(scope, &chain.head);
    for step in &chain.steps {
      cursor = self.chain_step(scope, cursor, step);
    }
    let receiver = match self.auto_deref(cursor) {
      Cursor::Value(receiver) => receiver,
      Cursor::Wrapped { package, .. } => return Outcome::External(package),
    };
    if receiver.ty == RecvType::Unknown {
      return Outcome::Unresolved(Miss::Dynamic);
    }
    self.rust_method_on_recv(receiver, method, caller, DispatchReach::Linked)
  }

  fn chain_head(&self, scope: &RustScope, head: &ChainHead) -> Cursor {
    match head {
      ChainHead::Binding { name, hint } => match hint {
        ReceiverHint::SelfValue => Cursor::Value(self.rust_self_recv(scope)),
        ReceiverHint::Declared { type_text, .. } => self.classify(&TypeText::declared(type_text, scope), 0),
        ReceiverHint::TypeHint(path) => Cursor::Value(self.rust_hint_recv(scope, name, path)),
        ReceiverHint::Identifier(identifier) if identifier == name => Cursor::Value(self.rust_identifier_recv(scope, name)),
        _ => Cursor::unknown(),
      },
      ChainHead::Call { path, args } => self.chain_call_head(scope, path, args),
      _ => Cursor::unknown(),
    }
  }

  fn chain_call_head(&self, scope: &RustScope, path: &[String], args: &[Option<String>]) -> Cursor {
    let Ok(hit) = self.rust_resolve_path(scope, path, Ns::Value, &mut Guard::default()) else {
      return Cursor::unknown();
    };
    let Found::Symbol(callee) = hit.found else { return Cursor::unknown() };
    let type_end = path.iter().position(|segment| qualified_trait(segment).is_some()).unwrap_or(path.len().saturating_sub(1));
    let owner = match path.split_at_checked(type_end) {
      Some((prefix, _)) if !prefix.is_empty() => self
        .rust_resolve_path(scope, prefix, Ns::Type, &mut Guard::default())
        .ok()
        .and_then(|owner| owner.symbol())
        .filter(|owner| {
          matches!(
            self.sym_kind(*owner),
            Some(EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::TypeAlias)
          )
        }),
      _ => None,
    };
    let cursor = match self.sym_kind(callee) {
      Some(EntityKind::Function | EntityKind::Method) => {
        let self_type = match (self.is_trait_member(callee), owner) {
          (false, _) => SelfType::Scope,
          (true, Some(owner)) if matches!(self.sym_kind(owner), Some(EntityKind::Struct | EntityKind::Enum)) => SelfType::Concrete(owner),
          (true, _) => SelfType::Opaque,
        };
        let bindings = self.unify_arguments(callee, args, scope);
        let returned = self.return_cursor(callee, self_type, bindings);
        match owner {
          Some(_) => returned.weaken(rule(CONSTRUCTOR_CONFIDENCE, RULE_CONSTRUCTOR)),
          None => returned,
        }
      }
      Some(EntityKind::Struct) => Cursor::Value(Recv::of(RecvType::Concrete(callee), None)),
      Some(EntityKind::Variant) => match self.table.sym(callee).and_then(|variant| variant.parent) {
        Some(parent) => Cursor::Value(Recv::of(RecvType::Concrete(SymRef { file: callee.file, local: parent }), None)),
        None => Cursor::unknown(),
      },
      _ => Cursor::unknown(),
    };
    cursor.weaken(hit.heuristic)
  }

  fn unify_arguments(&self, callee: SymRef, args: &[Option<String>], scope: &RustScope) -> Vec<(String, TypeText)> {
    let Some(surface) = self.table.sym(callee) else { return Vec::new() };
    let generics: Vec<&str> = surface.generic_names().collect();
    if generics.is_empty() || surface.params.len() != args.len() {
      return Vec::new();
    }
    let mut bindings = Vec::new();
    let mut conflicts = Vec::new();
    for ((_, param), arg) in surface.params.iter().zip(args) {
      if let (Some(param), Some(arg)) = (param, arg) {
        unify(param, arg, &generics, &mut bindings, &mut conflicts, 0);
      }
    }
    bindings
      .into_iter()
      .filter(|(name, _)| !conflicts.contains(name))
      .map(|(name, text)| (name, TypeText::declared(&text, scope)))
      .collect()
  }

  fn is_trait_member(&self, symbol: SymRef) -> bool {
    self
      .table
      .sym(symbol)
      .and_then(|surface| surface.parent)
      .is_some_and(|parent| self.sym_kind(SymRef { file: symbol.file, local: parent }) == Some(&EntityKind::Trait))
  }

  fn signature_scope(&self, function: SymRef) -> Option<RustScope> {
    let scope = self.rust_scope(function.file, Some(function.local))?;
    if scope.fns.len() > 1 {
      return Some(scope);
    }
    Some(RustScope {
      fns: Vec::new(),
      local_types: Vec::new(),
      may_declare_types: false,
      ..scope
    })
  }

  fn return_cursor(&self, function: SymRef, self_type: SelfType, bound: Vec<(String, TypeText)>) -> Cursor {
    let memo_key = bound.is_empty().then(|| (function, self_type.clone()));
    if let Some(key) = &memo_key
      && let Some(cached) = self.return_cursors.borrow().get(key)
    {
      return cached.clone();
    }
    let cursor = self.return_cursor_uncached(function, self_type, bound);
    if let Some(key) = memo_key {
      self.return_cursors.borrow_mut().insert(key, cursor.clone());
    }
    cursor
  }

  fn return_cursor_uncached(&self, function: SymRef, self_type: SelfType, bound: Vec<(String, TypeText)>) -> Cursor {
    let (Some(surface), Some(scope)) = (self.table.sym(function), self.signature_scope(function)) else {
      return Cursor::unknown();
    };
    let returned = TypeText {
      text: surface.return_type.clone().unwrap_or_else(|| "()".to_owned()),
      generics: Generics::Opaque(scope.generics.iter().map(|(name, _)| name.clone()).collect()),
      scope,
      bound: Rc::new(bound),
      self_type,
    };
    match surface.modifiers.contains(Modifier::Async) {
      true => Cursor::Wrapped {
        wrapper: Wrapper::Future,
        package: "std".to_owned(),
        inner: Box::new(returned),
        heuristic: None,
      },
      false => self.classify(&returned, 0),
    }
  }

  fn chain_step(&self, scope: &RustScope, cursor: Cursor, step: &ChainStep) -> Cursor {
    match step {
      ChainStep::Field(name) => self.field_step(self.auto_deref(cursor), name),
      ChainStep::Method(name) => self.method_step(scope, cursor, name),
      ChainStep::Try => match cursor {
        Cursor::Wrapped {
          wrapper: wrapper @ (Wrapper::Option | Wrapper::Result),
          inner,
          heuristic,
          ..
        } => self.classify(&inner, 0).weaken(heuristic).weaken(unwrap_rule(wrapper)),
        _ => Cursor::unknown(),
      },
      ChainStep::Await => match cursor {
        Cursor::Wrapped {
          wrapper: Wrapper::Future,
          inner,
          heuristic,
          ..
        } => self.classify(&inner, 0).weaken(heuristic),
        _ => Cursor::unknown(),
      },
      _ => Cursor::unknown(),
    }
  }

  fn auto_deref(&self, cursor: Cursor) -> Cursor {
    match cursor {
      Cursor::Wrapped {
        wrapper: Wrapper::Guard(name),
        inner,
        heuristic,
        ..
      } => self.classify(&inner, 0).weaken(heuristic).weaken(rule(DEREF_GUARD_CONFIDENCE, name)),
      other => other,
    }
  }

  fn field_step(&self, cursor: Cursor, name: &str) -> Cursor {
    let Cursor::Value(receiver) = cursor else { return Cursor::unknown() };
    let RecvType::Concrete(owner) = receiver.ty else { return Cursor::unknown() };
    let Some(member) = self.child_named(owner, name) else { return Cursor::unknown() };
    let memo_key = (member, SelfType::Concrete(owner));
    if let Some(cached) = self.return_cursors.borrow().get(&memo_key) {
      return cached.clone().weaken(receiver.heuristic);
    }
    let cursor = self.field_cursor(owner, member);
    self.return_cursors.borrow_mut().insert(memo_key, cursor.clone());
    cursor.weaken(receiver.heuristic)
  }

  fn field_cursor(&self, owner: SymRef, member: SymRef) -> Cursor {
    let Some(surface) = self.table.sym(member) else { return Cursor::unknown() };
    let (EntityKind::Field, Some(crate::typetext::ValueType::Declared(text))) = (&surface.kind, &surface.value_type) else {
      return Cursor::unknown();
    };
    let Some(scope) = self.symbol_scope(member) else { return Cursor::unknown() };
    let owner_generics = self.table.sym(owner).map(|symbol| symbol.generics.clone()).unwrap_or_default();
    let declared = TypeText {
      text: text.clone(),
      scope,
      generics: Generics::Bounded(owner_generics),
      bound: Rc::new(Vec::new()),
      self_type: SelfType::Concrete(owner),
    };
    self.classify(&declared, 0)
  }

  fn method_step(&self, scope: &RustScope, cursor: Cursor, name: &str) -> Cursor {
    match cursor {
      Cursor::Wrapped {
        wrapper: wrapper @ (Wrapper::Option | Wrapper::Result),
        package,
        inner,
        heuristic,
      } => {
        if UNWRAPPING_METHODS.contains(&name) {
          return self.classify(&inner, 0).weaken(heuristic).weaken(unwrap_rule(wrapper));
        }
        let wrapper = match (wrapper, name) {
          (_, name) if WRAPPER_KEEPING_METHODS.contains(&name) => wrapper,
          (Wrapper::Result, "ok") => Wrapper::Option,
          (Wrapper::Option, "ok_or" | "ok_or_else") => Wrapper::Result,
          _ => return Cursor::unknown(),
        };
        Cursor::Wrapped {
          wrapper,
          package,
          inner,
          heuristic,
        }
      }
      wrapped @ Cursor::Wrapped { wrapper: Wrapper::Guard(_), .. } => self.method_step(scope, self.auto_deref(wrapped), name),
      Cursor::Wrapped { .. } => Cursor::unknown(),
      Cursor::Value(receiver) => self.method_on_value(scope, receiver, name),
    }
  }

  fn method_on_value(&self, scope: &RustScope, receiver: Recv, name: &str) -> Cursor {
    if receiver.wrapper.is_some() {
      if DEREF_KEEPING_METHODS.contains(&name) {
        return Cursor::Value(receiver);
      }
      if name == "as_mut" || self.imports_borrow_trait(scope, name) {
        return Cursor::unknown();
      }
    }
    match &receiver.ty {
      RecvType::Concrete(owner) => match self.rust_method_or_std(*owner, name) {
        Ok(Hit {
          found: Found::Symbol(method),
          heuristic,
        }) if matches!(self.sym_kind(method), Some(EntityKind::Method | EntityKind::Function)) => {
          let self_type = match self.is_trait_member(method) {
            true => SelfType::Concrete(*owner),
            false => SelfType::Scope,
          };
          self.return_cursor(method, self_type, Vec::new()).weaken(heuristic).weaken(receiver.heuristic)
        }
        Ok(Hit { found: Found::External(_), .. }) if SELF_RETURNING_STD_METHODS.contains(&name) => Cursor::Value(Recv::of(RecvType::Concrete(*owner), receiver.heuristic)),
        _ => Cursor::unknown(),
      },
      RecvType::Traits { traits, .. } => {
        let declarations: Vec<Hit> = traits
          .iter()
          .filter_map(|owner| self.child_named(*owner, name))
          .map(|declaration| Hit::exact(Found::Symbol(declaration)))
          .collect();
        match unique(declarations).map(|hit| hit.found) {
          Ok(Found::Symbol(declaration)) => self.return_cursor(declaration, SelfType::Opaque, Vec::new()).weaken(receiver.heuristic),
          _ => Cursor::unknown(),
        }
      }
      _ => Cursor::unknown(),
    }
  }

  fn imports_borrow_trait(&self, scope: &RustScope, method: &str) -> bool {
    let Some((_, trait_name)) = BORROW_TRAITS.iter().find(|(name, _)| *name == method) else {
      return false;
    };
    let imported = self.surface.rust.uses.iter().any(|entry| match (entry.bound_name(), entry.target_segments()) {
      (Some(_), [root, .., module, name]) => STD_ROOTS.contains(&root.as_str()) && module == "borrow" && name == trait_name,
      (None, [.., module]) => module == "borrow",
      _ => false,
    });
    imported || !matches!(self.rust_lookup_name(scope, trait_name, Ns::Type, &mut Guard::default()), Err(Miss::NotFound))
  }

  fn classify(&self, declared: &TypeText, depth: usize) -> Cursor {
    if depth > MAX_TYPE_DEPTH {
      return Cursor::unknown();
    }
    let text = strip_reference(&declared.text);
    if text.starts_with("dyn ") || text.starts_with("impl ") {
      return Cursor::Value(self.recv_from_shape(&declared.scope, rust_type_shape(text), &[]));
    }
    let Some((path, args)) = rust_type_parts(text) else { return Cursor::unknown() };
    if let [single] = path.as_slice() {
      if let Some((_, bound)) = declared.bound.iter().find(|(name, _)| name == single) {
        return self.classify(bound, depth + 1);
      }
      match (&declared.self_type, single.as_str()) {
        (SelfType::Concrete(owner), "Self") => return Cursor::Value(Recv::of(RecvType::Concrete(*owner), None)),
        (SelfType::Opaque, "Self") => return Cursor::unknown(),
        _ => {}
      }
      match &declared.generics {
        Generics::Bounded(generics) if declared.generics.contains(single) => {
          return Cursor::Value(self.recv_from_shape(&declared.scope, TypeShape::Path(path), generics));
        }
        Generics::Opaque(_) if declared.generics.contains(single) => return Cursor::unknown(),
        _ => {}
      }
    }
    if path.len() > 1 && (path[0] == "Self" || declared.names_generic(&path[0])) {
      return Cursor::unknown();
    }
    if is_deref_wrapper(&path) {
      let Some(inner) = args.first() else { return Cursor::unknown() };
      return match self.classify(&declared.with_text(inner), depth + 1) {
        Cursor::Value(mut receiver) => {
          receiver.wrapper = Some("std".to_owned());
          Cursor::Value(receiver)
        }
        wrapped => wrapped,
      };
    }
    let Ok(hit) = self.rust_resolve_path(&declared.scope, &path, Ns::Type, &mut Guard::default()) else {
      return Cursor::unknown();
    };
    let cursor = match hit.found {
      Found::Symbol(symbol) => match self.sym_kind(symbol) {
        Some(EntityKind::Struct | EntityKind::Enum) => Cursor::Value(Recv::of(RecvType::Concrete(symbol), None)),
        Some(EntityKind::Trait) => Cursor::Value(Recv::of(
          RecvType::Traits {
            traits: vec![symbol],
            external: None,
          },
          None,
        )),
        Some(EntityKind::TypeAlias) => self.expand_alias(symbol, declared, &args, depth),
        _ => Cursor::unknown(),
      },
      Found::External(package) => self.external_type(package, &path, &args, declared),
      _ => Cursor::unknown(),
    };
    cursor.weaken(hit.heuristic)
  }

  fn expand_alias(&self, alias: SymRef, declared: &TypeText, args: &[&str], depth: usize) -> Cursor {
    let Some(surface) = self.table.sym(alias) else { return Cursor::unknown() };
    let module_level = surface
      .parent
      .is_none_or(|parent| self.sym_kind(SymRef { file: alias.file, local: parent }) == Some(&EntityKind::Module));
    let (true, Some(target), Some(scope)) = (module_level, &surface.alias_target, self.symbol_scope(alias)) else {
      return Cursor::unknown();
    };
    let params: Vec<String> = surface.generic_names().map(str::to_owned).collect();
    let bound: Vec<(String, TypeText)> = params.iter().zip(args).map(|(param, arg)| (param.clone(), declared.with_text(arg))).collect();
    let expanded = TypeText {
      text: target.clone(),
      scope,
      generics: Generics::Opaque(params),
      bound: Rc::new(bound),
      self_type: SelfType::Opaque,
    };
    self.classify(&expanded, depth + 1)
  }

  fn external_type(&self, package: String, path: &[String], args: &[&str], declared: &TypeText) -> Cursor {
    let Some(name) = path.last().map(String::as_str) else { return Cursor::unknown() };
    let Some(inner) = args.first() else {
      return Cursor::Value(Recv::of(RecvType::External(package), None));
    };
    let wrapper = match name {
      "Option" => Wrapper::Option,
      "Result" => Wrapper::Result,
      _ => match GUARDS.iter().find(|(guard, _, packages)| *guard == name && packages.contains(&package.as_str())) {
        Some((_, rule, _)) => Wrapper::Guard(rule),
        None => return Cursor::Value(Recv::of(RecvType::External(package), None)),
      },
    };
    Cursor::Wrapped {
      wrapper,
      package,
      inner: Box::new(declared.with_text(inner)),
      heuristic: None,
    }
  }
}
