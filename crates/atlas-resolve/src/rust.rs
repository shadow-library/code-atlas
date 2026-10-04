use atlas_core::{EntityKind, LanguageFamily, ReceiverHint, RefKind, ReferenceFact, RustTarget, Span, Visibility};

use std::collections::{HashSet, VecDeque};

use crate::engine::{
  DISPATCH_CONFIDENCE, FIELD_NAME_CONFIDENCE, FN_LOCAL_ITEM_CONFIDENCE, Found, Guard, Heuristic, Hit, ImplTargets, Lookup, MACRO_SCOPE_CONFIDENCE, MAX_DISPATCH_CANDIDATES, Miss,
  Outcome, RULE_DYN_DISPATCH, RULE_FIELD_NAME, RULE_FN_LOCAL_ITEM, RULE_MACRO_SCOPE, RULE_OVERRIDE_DISPATCH, Recv, RecvType, Resolver, unique, weakest,
};
use crate::layout::{ExternCrate, PackageKey};
use crate::surface::{SymSurface, UseScope, UseSurface};
use crate::table::{FileIdx, ModuleId, SymRef};
use crate::typetext::{TypeShape, ValueType, is_deref_wrapper, rust_generic_param, rust_outer_path, rust_type_shape};

const RUST_PRELUDE: [&str; 46] = [
  "Option",
  "Some",
  "None",
  "Result",
  "Ok",
  "Err",
  "Vec",
  "String",
  "Box",
  "ToString",
  "ToOwned",
  "Clone",
  "Copy",
  "Default",
  "Drop",
  "Eq",
  "PartialEq",
  "Ord",
  "PartialOrd",
  "Fn",
  "FnMut",
  "FnOnce",
  "From",
  "Into",
  "TryFrom",
  "TryInto",
  "Iterator",
  "IntoIterator",
  "Extend",
  "DoubleEndedIterator",
  "ExactSizeIterator",
  "AsRef",
  "AsMut",
  "Send",
  "Sync",
  "Sized",
  "Unpin",
  "drop",
  "FromIterator",
  "Self",
  "size_of",
  "size_of_val",
  "align_of",
  "align_of_val",
  "Future",
  "IntoFuture",
];
const STD_BLANKET_METHODS: [&str; 11] = [
  "into",
  "try_into",
  "from",
  "try_from",
  "to_owned",
  "to_string",
  "borrow",
  "borrow_mut",
  "type_id",
  "as_ref",
  "as_mut",
];
const DERIVED_METHODS: [(&str, &str, &str); 15] = [
  ("default", "Default", "std"),
  ("clone", "Clone", "std"),
  ("clone_from", "Clone", "std"),
  ("fmt", "Debug", "std"),
  ("eq", "PartialEq", "std"),
  ("ne", "PartialEq", "std"),
  ("cmp", "Ord", "std"),
  ("max", "Ord", "std"),
  ("min", "Ord", "std"),
  ("clamp", "Ord", "std"),
  ("partial_cmp", "PartialOrd", "std"),
  ("hash", "Hash", "std"),
  ("serialize", "Serialize", "serde"),
  ("deserialize", "Deserialize", "serde"),
  ("deserialize_in_place", "Deserialize", "serde"),
];
const PRIMITIVES: [&str; 17] = [
  "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];
const COMMON_STD_FIELDS: [&str; 5] = ["start", "end", "status", "stdout", "stderr"];
const STD_CRATES: [&str; 5] = ["std", "core", "alloc", "proc_macro", "test"];
const STD_MACROS: [&str; 38] = [
  "println",
  "print",
  "eprintln",
  "eprint",
  "format",
  "format_args",
  "write",
  "writeln",
  "vec",
  "panic",
  "assert",
  "assert_eq",
  "assert_ne",
  "debug_assert",
  "debug_assert_eq",
  "debug_assert_ne",
  "todo",
  "unimplemented",
  "unreachable",
  "matches",
  "dbg",
  "include",
  "include_str",
  "include_bytes",
  "concat",
  "stringify",
  "env",
  "option_env",
  "cfg",
  "line",
  "column",
  "file",
  "module_path",
  "compile_error",
  "thread_local",
  "concat_idents",
  "trace_macros",
  "log_syntax",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ns {
  Type,
  Value,
  Macro,
}

impl Ns {
  fn admits(self, kind: &EntityKind) -> bool {
    match self {
      Self::Macro => *kind == EntityKind::Macro,
      Self::Type => !matches!(
        kind,
        EntityKind::Function | EntityKind::Method | EntityKind::Constant | EntityKind::Variable | EntityKind::Test | EntityKind::Macro | EntityKind::Field
      ),
      Self::Value => matches!(
        kind,
        EntityKind::Function | EntityKind::Method | EntityKind::Constant | EntityKind::Variable | EntityKind::Test | EntityKind::Struct | EntityKind::Variant
      ),
    }
  }

  fn tag(self) -> &'static str {
    match self {
      Self::Type => "t",
      Self::Value => "v",
      Self::Macro => "m",
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) struct RustScope {
  pub file: FileIdx,
  pub module: ModuleId,
  pub fns: Vec<u32>,
  pub from: Option<u32>,
  pub local_types: Vec<String>,
  pub may_declare_types: bool,
  pub generics: Vec<(String, Vec<String>)>,
}

impl RustScope {
  fn at_module(file: FileIdx, module: ModuleId) -> Self {
    Self {
      file,
      module,
      fns: Vec::new(),
      from: None,
      local_types: Vec::new(),
      may_declare_types: false,
      generics: Vec::new(),
    }
  }

  fn use_target_scope(&self) -> Self {
    Self {
      may_declare_types: self.may_declare_types,
      ..Self::at_module(self.file, self.module.clone())
    }
  }
}

const WRAPPER_METHODS: [&str; 28] = [
  "clone",
  "clone_from",
  "as_ref",
  "as_mut",
  "borrow",
  "borrow_mut",
  "deref",
  "deref_mut",
  "eq",
  "ne",
  "cmp",
  "partial_cmp",
  "hash",
  "fmt",
  "to_string",
  "into",
  "downgrade",
  "upgrade",
  "strong_count",
  "weak_count",
  "get_mut",
  "make_mut",
  "ptr_eq",
  "into_raw",
  "as_ptr",
  "try_unwrap",
  "into_inner",
  "unwrap_or_clone",
];

fn is_type_like(kind: Option<&EntityKind>) -> bool {
  matches!(
    kind,
    Some(EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::TypeAlias | EntityKind::Variant | EntityKind::Module)
  )
}

fn qualified_trait(segment: &str) -> Option<Vec<String>> {
  let inner = segment.strip_prefix('<')?.strip_suffix('>')?;
  let head = inner.split('<').next().unwrap_or(inner);
  let path: Vec<String> = head.split("::").map(|part| part.trim().to_owned()).collect();
  path.iter().all(|part| !part.is_empty()).then_some(path)
}

fn is_prelude_name(name: &str, ns: Ns) -> bool {
  match ns {
    Ns::Macro => STD_MACROS.contains(&name),
    _ => RUST_PRELUDE.contains(&name) || PRIMITIVES.contains(&name) || STD_CRATES.contains(&name),
  }
}

fn ends_in_annotation(symbol: &SymSurface, at: Option<Span>) -> bool {
  matches!((symbol.annotation_end_byte, at), (Some(end), Some(at)) if at.end_byte <= end)
}

fn has_declared_type(receiver: Option<&ReceiverHint>) -> bool {
  matches!(receiver, Some(ReceiverHint::SelfValue | ReceiverHint::TypeHint(_) | ReceiverHint::Declared { .. }))
}

fn is_dynamic_segment(segment: &str) -> bool {
  segment.starts_with('[') || segment.starts_with('(')
}

impl Resolver<'_> {
  pub(crate) fn rust_scope(&self, file: FileIdx, from: Option<u32>) -> Option<RustScope> {
    self.rust_scope_at(file, from, None)
  }

  fn rust_scope_at(&self, file: FileIdx, from: Option<u32>, at: Option<Span>) -> Option<RustScope> {
    let surface = self.table.file(file)?;
    let suffix: Vec<String> = match from.and_then(|local| surface.symbol(local)) {
      None => Vec::new(),
      Some(symbol) if symbol.kind == EntityKind::Module => {
        let inline = surface.rust.module_decls.iter().any(|declaration| declaration.inline == from);
        match (inline, symbol.module_suffix.split_last()) {
          (false, Some((_, parent))) => parent.to_vec(),
          _ => symbol.module_suffix.clone(),
        }
      }
      Some(symbol) => symbol.module_suffix.clone(),
    };
    let fns: Vec<u32> = from
      .map(|local| {
        surface
          .ancestors(local)
          .filter(|ancestor| surface.symbol(*ancestor).is_some_and(SymSurface::is_function_like))
          .collect()
      })
      .unwrap_or_default();
    let mut local_types = Vec::new();
    let mut may_declare_types = false;
    let mut generics = Vec::new();
    for function in &fns {
      let Some(symbol) = surface.symbol(*function) else { continue };
      may_declare_types |= symbol.declares_local_types;
      generics.extend(symbol.generics.iter().cloned());
      local_types.extend(
        surface
          .rust
          .impls
          .iter()
          .filter(|implementation| implementation.scope == Some(*function))
          .filter_map(|implementation| match implementation.self_type.segments.as_slice() {
            [single] => Some(single.clone()),
            _ => None,
          }),
      );
    }
    if let Some(local) = from {
      generics.extend(self.enclosing_type_generics(file, local));
      may_declare_types |= surface.ancestors(local).any(|ancestor| {
        surface
          .symbol(ancestor)
          .is_some_and(|symbol| symbol.declares_local_types && !(ancestor == local && ends_in_annotation(symbol, at)))
      });
    }
    Some(RustScope {
      file,
      module: self.table.rust_module_of(file, &suffix)?,
      fns,
      from,
      local_types,
      may_declare_types,
      generics,
    })
  }

  fn enclosing_type_generics(&self, file: FileIdx, local: u32) -> Vec<(String, Vec<String>)> {
    let Some(surface) = self.table.file(file) else { return Vec::new() };
    for ancestor in surface.ancestors(local) {
      let Some(symbol) = surface.symbol(ancestor) else { continue };
      if symbol.kind == EntityKind::Trait {
        return symbol.generics.clone();
      }
      let Some(impl_of) = &symbol.impl_of else { continue };
      return surface
        .rust
        .impls
        .iter()
        .find(|implementation| {
          implementation.self_type == impl_of.self_type && implementation.trait_ref == impl_of.trait_ref && implementation.module_suffix == symbol.module_suffix
        })
        .map(|implementation| implementation.generic_params.iter().map(|name| (name.clone(), Vec::new())).collect())
        .unwrap_or_default();
    }
    Vec::new()
  }

  fn symbol_scope(&self, symbol: SymRef) -> Option<RustScope> {
    let scope = self.rust_scope(symbol.file, Some(symbol.local))?;
    Some(RustScope::at_module(scope.file, scope.module))
  }

  fn visible(&self, visibility: &Visibility, owner: &ModuleId, from: &ModuleId) -> bool {
    if from.is_within(owner) {
      return true;
    }
    match visibility {
      Visibility::Private => false,
      Visibility::Crate | Visibility::Restricted(_) => from.same_crate(owner),
      _ => true,
    }
  }

  pub(crate) fn rust_reference(&self, reference: &ReferenceFact) -> Outcome {
    let Some(scope) = self.rust_scope_at(self.file, reference.from.map(|local| local.0), Some(reference.span)) else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    let path = &reference.path;
    if path.is_empty() {
      return Outcome::Unresolved(Miss::NotFound);
    }
    if path.iter().any(|segment| is_dynamic_segment(segment)) {
      return Outcome::Unresolved(Miss::Dynamic);
    }
    let mut guard = Guard::default();
    match reference.kind {
      RefKind::MethodCall => self.rust_method_call(&scope, reference),
      RefKind::MacroCall => self.rust_macro(&scope, reference, &mut guard),
      RefKind::TypeRef | RefKind::Instantiation | RefKind::Inherit => Outcome::from_lookup(self.rust_resolve_path(&scope, path, Ns::Type, &mut guard)),
      RefKind::ValueRef if reference.receiver.is_some() => self.rust_field_access(&scope, reference),
      RefKind::Call | RefKind::ValueRef => {
        let lookup = self.rust_resolve_path(&scope, path, Ns::Value, &mut guard);
        match lookup {
          Err(Miss::NotFound) if self.rust_macro_may_generate(&scope, path, &mut guard) => Outcome::Unresolved(Miss::MacroGenerated),
          Ok(hit) if reference.kind == RefKind::Call && self.is_trait_method(&hit) => {
            let extra = hit
              .symbol()
              .map(|declaration| self.rust_dispatch_candidates(declaration, RULE_OVERRIDE_DISPATCH))
              .unwrap_or_default();
            Outcome::Targets { primary: hit, extra }
          }
          other => Outcome::from_lookup(other),
        }
      }
      _ => Outcome::Unresolved(Miss::Unsupported("reference kind has no rust resolution rule")),
    }
  }

  pub(crate) fn rust_type_anchor(&self, reference: &ReferenceFact) -> Option<Hit> {
    if !matches!(reference.kind, RefKind::Call | RefKind::ValueRef) || reference.path.len() < 2 || reference.receiver.is_some() {
      return None;
    }
    let scope = self.rust_scope_at(self.file, reference.from.map(|local| local.0), Some(reference.span))?;
    let path = &reference.path;
    let qualified_at = path.iter().position(|segment| qualified_trait(segment).is_some());
    let type_path = &path[..qualified_at.unwrap_or(path.len() - 1)];
    let head = type_path.first()?;
    if matches!(head.as_str(), "Self" | "crate" | "self" | "super" | "$crate" | "_") {
      return None;
    }
    if type_path.len() == 1 && scope.generics.iter().any(|(generic, _)| generic == head) {
      return None;
    }
    let hit = self.rust_resolve_path(&scope, type_path, Ns::Type, &mut Guard::default()).ok()?;
    let symbol = hit.symbol()?;
    matches!(
      self.sym_kind(symbol),
      Some(EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::TypeAlias)
    )
    .then_some(hit)
  }

  pub(crate) fn rust_resolve_path(&self, scope: &RustScope, segments: &[String], ns: Ns, guard: &mut Guard) -> Lookup {
    let Some((first, rest)) = segments.split_first() else { return Err(Miss::NotFound) };
    if segments.iter().any(|segment| is_dynamic_segment(segment)) {
      return Err(Miss::Dynamic);
    }
    let start = match first.as_str() {
      "crate" | "$crate" => Hit::exact(Found::Module(scope.module.root())),
      "self" => Hit::exact(Found::Module(scope.module.clone())),
      "super" => Hit::exact(Found::Module(scope.module.parent().ok_or(Miss::NotFound)?)),
      "Self" => self.rust_self_type(scope)?,
      "_" => return Err(Miss::Unsupported("inferred type placeholder")),
      name => self.rust_lookup_name(scope, name, if rest.is_empty() { ns } else { Ns::Type }, guard)?,
    };
    self.rust_walk(scope, start, rest, ns, guard)
  }

  fn rust_walk(&self, scope: &RustScope, start: Hit, rest: &[String], ns: Ns, guard: &mut Guard) -> Lookup {
    let mut current = start;
    let mut index = 0;
    while index < rest.len() {
      let segment = &rest[index];
      let segment_ns = if index + 1 == rest.len() { ns } else { Ns::Type };
      let carried = current.heuristic;
      current = match current.found.clone() {
        Found::External(_) => return Ok(current),
        Found::File(_) => return Err(Miss::NotFound),
        Found::Module(module) => match segment.as_str() {
          "super" => Hit::exact(Found::Module(module.parent().ok_or(Miss::NotFound)?)),
          "self" => current,
          _ => match self.rust_lookup_in_module(&module, segment, segment_ns, &scope.module, guard) {
            Err(Miss::NotFound) if self.module_has_item_macros(&module) => return Err(Miss::MacroGenerated),
            other => other?,
          },
        },
        Found::Symbol(symbol) => match qualified_trait(segment) {
          Some(trait_path) => {
            let Some(method) = rest.get(index + 1) else { return Err(Miss::NotFound) };
            index += 1;
            self.rust_qualified(scope, symbol, &trait_path, method, guard)?
          }
          None => self.rust_associated(symbol, segment)?,
        },
      }
      .weaken(carried);
      index += 1;
    }
    Ok(current)
  }

  pub(crate) fn rust_lookup_name(&self, scope: &RustScope, name: &str, ns: Ns, guard: &mut Guard) -> Lookup {
    self.note(name);
    if ns != Ns::Macro && scope.local_types.iter().any(|local| local == name) {
      return Err(Miss::Unsupported("fn-local item"));
    }
    let result = match self.rust_lookup_scoped(scope, name, ns, guard) {
      Err(Miss::NotFound) => self.rust_prelude(scope, name, ns),
      Err(Miss::MacroGenerated) => self.rust_prelude(scope, name, ns).or(Err(Miss::MacroGenerated)),
      other => other,
    };
    match result {
      Ok(hit) if scope.may_declare_types && ns != Ns::Macro && self.is_type_like_hit(&hit) => Ok(hit.weaken(Some(Heuristic {
        confidence: FN_LOCAL_ITEM_CONFIDENCE,
        rule: RULE_FN_LOCAL_ITEM,
      }))),
      other => other,
    }
  }

  fn is_type_like_hit(&self, hit: &Hit) -> bool {
    match &hit.found {
      Found::Module(_) => true,
      Found::Symbol(symbol) => is_type_like(self.sym_kind(*symbol)),
      _ => false,
    }
  }

  fn rust_against_module_binding(&self, scope: &RustScope, name: &str, ns: Ns, hit: Hit, guard: &mut Guard) -> Lookup {
    let outer = match self.rust_lookup_in_module(&scope.module, name, ns, &scope.module, guard) {
      Err(Miss::NotFound) => self.rust_prelude(scope, name, ns),
      other => other,
    };
    match outer {
      Ok(binding) if binding.found != hit.found => Err(Miss::Ambiguous(vec![hit, binding])),
      _ => Ok(hit),
    }
  }

  fn rust_lookup_scoped(&self, scope: &RustScope, name: &str, ns: Ns, guard: &mut Guard) -> Lookup {
    let Some(surface) = self.table.file(scope.file) else { return Err(Miss::NotFound) };
    for function in &scope.fns {
      let use_scope = UseScope::Function(*function);
      let mut generated = false;
      let mut explicit: Vec<Hit> = Vec::new();
      for entry in surface.uses_in(&use_scope).filter(|entry| entry.bound_name() == Some(name)) {
        match self.rust_use_target(&scope.use_target_scope(), entry, ns, guard) {
          Ok(hit) => explicit.push(hit),
          Err(Miss::MacroGenerated) => generated = true,
          Err(_) => {}
        }
      }
      match unique(explicit) {
        Err(Miss::NotFound) if generated => return Err(Miss::MacroGenerated),
        Err(Miss::NotFound) => {}
        Ok(hit) => return self.rust_against_module_binding(scope, name, ns, hit, guard),
        other => return other,
      }
      let globs: Vec<(RustScope, &UseSurface)> = surface
        .uses_in(&use_scope)
        .filter(|entry| entry.bound_name().is_none())
        .map(|entry| (scope.use_target_scope(), entry))
        .collect();
      if globs.iter().any(|(module_scope, glob)| self.rust_use_target(module_scope, glob, Ns::Type, guard).is_err()) {
        return Err(Miss::Unsupported("fn-local item"));
      }
      match self.rust_globs(globs, name, ns, guard) {
        Err(Miss::NotFound) => {}
        Ok(hit) => return self.rust_against_module_binding(scope, name, ns, hit, guard),
        other => return other,
      }
    }
    self.rust_lookup_in_module(&scope.module, name, ns, &scope.module, guard)
  }

  fn rust_use_target(&self, scope: &RustScope, entry: &UseSurface, ns: Ns, guard: &mut Guard) -> Lookup {
    let previous = guard.macro_items;
    guard.macro_items = true;
    let result = self.rust_resolve_path(scope, entry.target_segments(), ns, guard);
    guard.macro_items = previous;
    result
  }

  pub(crate) fn rust_lookup_in_module(&self, module: &ModuleId, name: &str, ns: Ns, from: &ModuleId, guard: &mut Guard) -> Lookup {
    self.note(name);
    let key = format!(
      "{}:{}:{}::{}#{}<{}:{}:{}",
      module.repo,
      module.package,
      module.path.join("/"),
      name,
      ns.tag(),
      from.repo,
      from.package,
      from.path.join("/")
    );
    let memoize = guard.is_top_level();
    if memoize && let Some(cached) = self.module_lookups.borrow().get(&key) {
      return cached.clone();
    }
    if !guard.enter(key.clone()) {
      return Err(Miss::NotFound);
    }
    let result = match self.rust_direct_in_module(module, name, ns, from, guard) {
      Err(Miss::NotFound) => self.rust_globs(self.module_globs(module, from), name, ns, guard),
      other => other,
    };
    guard.exit();
    if memoize {
      self.module_lookups.borrow_mut().insert(key, result.clone());
    }
    result
  }

  fn module_globs(&self, module: &ModuleId, from: &ModuleId) -> Vec<(RustScope, &UseSurface)> {
    let Some(entry) = self.table.module_entry(module) else { return Vec::new() };
    let mut globs = Vec::new();
    for location in entry.effective_locs() {
      let Some(file) = self.table.file(location.file) else { continue };
      for glob in file.uses_in(&UseScope::Module(location.suffix.clone())) {
        if glob.bound_name().is_none() && self.visible(&glob.visibility, module, from) {
          globs.push((RustScope::at_module(location.file, module.clone()), glob));
        }
      }
    }
    globs
  }

  fn is_braced_struct(&self, symbol: SymRef) -> bool {
    let Some(file) = self.table.file(symbol.file) else { return false };
    file.children(symbol.local).iter().any(|child| {
      file
        .symbol(*child)
        .is_some_and(|field| field.kind == EntityKind::Field && !field.name.chars().all(|c| c.is_ascii_digit()))
    })
  }

  fn rust_direct_in_module(&self, module: &ModuleId, name: &str, ns: Ns, from: &ModuleId, guard: &mut Guard) -> Lookup {
    let Some(entry) = self.table.module_entry(module) else { return Err(Miss::NotFound) };
    let mut hits = Vec::new();
    let mut generated = false;
    for location in entry.effective_locs() {
      let Some(file) = self.table.file(location.file) else { continue };
      for local in file.items_named(&location.suffix, name) {
        let Some(symbol) = file.symbol(*local) else { continue };
        let reference = SymRef {
          file: location.file,
          local: *local,
        };
        let admitted = match (ns, &symbol.kind) {
          (Ns::Macro, EntityKind::Macro) => symbol.exported_macro || guard.macro_items,
          (Ns::Value, EntityKind::Struct) => !self.is_braced_struct(reference),
          _ => ns.admits(&symbol.kind),
        };
        if admitted && self.visible(&symbol.visibility, module, from) {
          hits.push(Hit::exact(Found::Symbol(reference)));
        }
      }
      if ns == Ns::Type && file.module_decls_in(&location.suffix).any(|declaration| declaration.name == name) {
        hits.push(Hit::exact(Found::Module(module.child(name))));
      }
      if ns == Ns::Type && location.suffix.is_empty() {
        for (alias, crate_name) in &file.rust.extern_crates {
          if alias != name {
            continue;
          }
          match crate_name.as_str() {
            "self" => hits.push(Hit::exact(Found::Module(module.root()))),
            _ => match self.rust_extern_crate(location.file, crate_name) {
              Some(Ok(hit)) => hits.push(hit),
              Some(Err(Miss::Ambiguous(ambiguous))) => hits.extend(ambiguous),
              _ => {}
            },
          }
        }
      }
      let scope = RustScope::at_module(location.file, module.clone());
      for entry in file.uses_in(&UseScope::Module(location.suffix.clone())) {
        if entry.bound_name() != Some(name) || !self.visible(&entry.visibility, module, from) {
          continue;
        }
        match self.rust_use_target(&scope, entry, ns, guard) {
          Ok(hit) => hits.push(hit),
          Err(Miss::Ambiguous(ambiguous)) => hits.extend(ambiguous),
          Err(Miss::MacroGenerated) => generated = true,
          Err(_) => {}
        }
      }
    }
    if ns == Ns::Macro && module.path.is_empty() {
      hits.extend(self.rust_exported_macros(module, name).into_iter().map(|symbol| Hit::exact(Found::Symbol(symbol))));
    }
    if !hits.is_empty() {
      return unique(hits);
    }
    if generated {
      return Err(Miss::MacroGenerated);
    }
    Err(Miss::NotFound)
  }

  fn rust_globs<'s>(&'s self, globs: Vec<(RustScope, &'s UseSurface)>, name: &str, ns: Ns, guard: &mut Guard) -> Lookup {
    let mut hits = Vec::new();
    let mut external = None;
    let mut generated = false;
    let mut visited: HashSet<ModuleId> = HashSet::new();
    let mut queue: VecDeque<(RustScope, &UseSurface, Option<Heuristic>, ModuleId)> = globs
      .into_iter()
      .map(|(scope, glob)| {
        let importer = scope.module.clone();
        (scope, glob, None, importer)
      })
      .collect();
    while let Some((scope, glob, carried, importer)) = queue.pop_front() {
      match self.rust_use_target(&scope, glob, Ns::Type, guard) {
        Ok(Hit {
          found: Found::Module(target),
          heuristic,
        }) => {
          if !visited.insert(target.clone()) {
            continue;
          }
          let carried = weakest(carried, heuristic);
          match self.rust_direct_in_module(&target, name, ns, &importer, guard) {
            Ok(hit) => hits.push(hit.weaken(carried)),
            Err(Miss::Ambiguous(ambiguous)) => hits.extend(ambiguous),
            Err(Miss::MacroGenerated) => generated = true,
            Err(Miss::NotFound) => {
              generated |= self.module_has_item_macros(&target);
              queue.extend(
                self
                  .module_globs(&target, &importer)
                  .into_iter()
                  .map(|(next, nested)| (next, nested, carried, importer.clone())),
              );
            }
            Err(_) => {}
          }
        }
        Ok(Hit {
          found: Found::Symbol(enumeration),
          ..
        }) if self.sym_kind(enumeration) == Some(&EntityKind::Enum) && ns != Ns::Macro => {
          hits.extend(self.child_named(enumeration, name).map(|variant| Hit::exact(Found::Symbol(variant))));
        }
        Ok(Hit {
          found: Found::External(package), ..
        }) => {
          external.get_or_insert(package);
        }
        _ => {}
      }
    }
    if !hits.is_empty() {
      return unique(hits);
    }
    if is_prelude_name(name, ns) {
      return Err(Miss::NotFound);
    }
    match (external, generated) {
      (Some(package), _) => Ok(Hit::exact(Found::External(package))),
      (None, true) => Err(Miss::MacroGenerated),
      (None, false) => Err(Miss::NotFound),
    }
  }

  fn rust_std_fallback(&self, owner: SymRef, method: &str) -> Option<String> {
    if STD_BLANKET_METHODS.contains(&method) {
      return Some("std".to_owned());
    }
    let derives = &self.table.sym(owner)?.derives;
    DERIVED_METHODS
      .iter()
      .find(|(name, derive, _)| *name == method && derives.iter().any(|derived| derived == derive))
      .map(|(_, _, package)| (*package).to_owned())
  }

  fn rust_method_or_std(&self, owner: SymRef, name: &str) -> Lookup {
    match self.rust_method_on_type(owner, name) {
      Err(Miss::NotFound) => {
        if let Some(constant) = self.rust_assoc_const(owner, name) {
          return Ok(Hit::exact(Found::Symbol(constant)));
        }
        match self.rust_std_fallback(owner, name) {
          Some(package) => Ok(Hit::exact(Found::External(package))),
          None => Err(Miss::NotFound),
        }
      }
      other => other,
    }
  }

  fn rust_assoc_const(&self, owner: SymRef, name: &str) -> Option<SymRef> {
    let owner_surface = self.table.sym(owner)?;
    let module = self.table.rust_module_of(owner.file, &owner_surface.module_suffix)?;
    let wanted = [owner_surface.name.clone(), name.to_owned()];
    self.table.module_entry(&module)?.effective_locs().find_map(|location| {
      let file = self.table.file(location.file)?;
      file
        .symbols
        .iter()
        .position(|symbol| symbol.kind == EntityKind::Constant && symbol.module_suffix == location.suffix && symbol.item_path == wanted)
        .map(|local| SymRef {
          file: location.file,
          local: local as u32,
        })
    })
  }

  fn rust_exported_macros(&self, root: &ModuleId, name: &str) -> Vec<SymRef> {
    let Some(macros) = self.table.macros_by_name.get(name) else { return Vec::new() };
    macros
      .iter()
      .copied()
      .filter(|symbol| self.table.sym(*symbol).is_some_and(|surface| surface.exported_macro))
      .filter(|symbol| self.table.rust_module_of(symbol.file, &[]).is_some_and(|module| module.same_crate(root)))
      .collect()
  }

  fn rust_extern_crate(&self, file: FileIdx, name: &str) -> Option<Lookup> {
    let package = self.table.file_package(file)?;
    match self.table.layout.extern_crate(package, name)? {
      ExternCrate::InProject(target) => self.crate_root(&target).map(|root| Ok(Hit::exact(Found::Module(root)))),
      ExternCrate::Ambiguous(targets) => Some(Err(Miss::Ambiguous(
        targets
          .iter()
          .filter_map(|target| self.crate_root(target))
          .map(|root| Hit::exact(Found::Module(root)))
          .collect(),
      ))),
      ExternCrate::External(package) => Some(Ok(Hit::exact(Found::External(package)))),
    }
  }

  fn crate_root(&self, package: &PackageKey) -> Option<ModuleId> {
    let cargo = self.table.layout.cargo(package)?;
    cargo.lib()?;
    Some(ModuleId {
      repo: package.repo.clone(),
      package: RustTarget::Lib.package_field(&cargo.name),
      path: Vec::new(),
    })
  }

  fn rust_prelude(&self, scope: &RustScope, name: &str, ns: Ns) -> Lookup {
    if ns == Ns::Macro {
      return match STD_MACROS.contains(&name) {
        true => Ok(Hit::exact(Found::External("std".to_owned()))),
        false => Err(Miss::NotFound),
      };
    }
    if let Some(result) = self.rust_extern_crate(scope.file, name) {
      return result;
    }
    let root = scope.module.root();
    let root_alias = self.table.module_entry(&root).and_then(|entry| {
      entry.effective_locs().filter(|location| location.suffix.is_empty()).find_map(|location| {
        let file = self.table.file(location.file)?;
        let (_, crate_name) = file.rust.extern_crates.iter().find(|(alias, _)| alias == name)?;
        match crate_name.as_str() {
          "self" => Some(Ok(Hit::exact(Found::Module(root.clone())))),
          _ => self.rust_extern_crate(location.file, crate_name),
        }
      })
    });
    if let Some(result) = root_alias {
      return result;
    }
    if STD_CRATES.contains(&name) {
      return Ok(Hit::exact(Found::External(name.to_owned())));
    }
    if RUST_PRELUDE.contains(&name) || PRIMITIVES.contains(&name) {
      return Ok(Hit::exact(Found::External("std".to_owned())));
    }
    Err(Miss::NotFound)
  }

  fn rust_associated(&self, owner: SymRef, segment: &str) -> Lookup {
    match self.sym_kind(owner) {
      Some(EntityKind::Trait) => self.child_named(owner, segment).map(|child| Hit::exact(Found::Symbol(child))).ok_or(Miss::NotFound),
      Some(EntityKind::Enum) => match self.child_named(owner, segment) {
        Some(variant) => Ok(Hit::exact(Found::Symbol(variant))),
        None => self.rust_method_or_std(owner, segment),
      },
      Some(EntityKind::Struct) => self.rust_method_or_std(owner, segment),
      Some(EntityKind::TypeAlias) => Err(Miss::Unsupported("associated items through a type alias")),
      _ => Err(Miss::NotFound),
    }
  }

  fn rust_qualified(&self, scope: &RustScope, owner: SymRef, trait_path: &[String], method: &str, guard: &mut Guard) -> Lookup {
    let target = self.rust_resolve_path(scope, trait_path, Ns::Type, guard)?;
    let Found::Symbol(trait_symbol) = target.found else { return Ok(target) };
    if self.sym_kind(trait_symbol) != Some(&EntityKind::Trait) {
      return Err(Miss::NotFound);
    }
    let implementations: Vec<Hit> = self
      .rust_impl_methods(owner, method)
      .into_iter()
      .filter(|candidate| self.rust_method_trait(*candidate) == Some(Found::Symbol(trait_symbol)))
      .map(|candidate| Hit::exact(Found::Symbol(candidate)))
      .collect();
    match unique(implementations) {
      Err(Miss::NotFound) => self.child_named(trait_symbol, method).map(|child| Hit::exact(Found::Symbol(child))).ok_or(Miss::NotFound),
      other => other,
    }
  }

  fn rust_impl_methods(&self, owner: SymRef, name: &str) -> Vec<SymRef> {
    let Some(owner_name) = self.table.sym(owner).map(|symbol| symbol.name.clone()) else {
      return Vec::new();
    };
    let Some(methods) = self.table.methods_by_name.get(name) else { return Vec::new() };
    let mut matching: Vec<SymRef> = methods
      .iter()
      .copied()
      .filter(|method| {
        self.table.file(method.file).is_some_and(|file| file.family == LanguageFamily::Rust)
          && self
            .table
            .sym(*method)
            .and_then(|symbol| symbol.impl_of.as_ref())
            .is_some_and(|impl_of| impl_of.self_type.name() == Some(owner_name.as_str()))
      })
      .filter(|method| self.rust_method_self(*method) == Some(Found::Symbol(owner)))
      .collect();
    matching.sort();
    matching
  }

  pub(crate) fn rust_method_on_type(&self, owner: SymRef, name: &str) -> Lookup {
    self.note(name);
    if let Some(owner_surface) = self.table.sym(owner) {
      self.note(&owner_surface.name);
    }
    let candidates = self.rust_impl_methods(owner, name);
    let (inherent, trait_impls): (Vec<SymRef>, Vec<SymRef>) = candidates.into_iter().partition(|method| {
      self
        .table
        .sym(*method)
        .and_then(|symbol| symbol.impl_of.as_ref())
        .is_some_and(|impl_of| impl_of.trait_ref.is_none())
    });
    for group in [inherent, trait_impls] {
      if !group.is_empty() {
        return unique(group.into_iter().map(|method| Hit::exact(Found::Symbol(method))).collect());
      }
    }
    let Some(owner_name) = self.table.sym(owner).map(|symbol| symbol.name.clone()) else {
      return Err(Miss::NotFound);
    };
    let mut defaults = Vec::new();
    for (file, position) in self.table.rust_impls_by_self.get(&owner_name).map(Vec::as_slice).unwrap_or_default() {
      let (self_found, trait_found) = self.rust_impl_targets(*file, *position);
      if self_found != Some(Found::Symbol(owner)) {
        continue;
      }
      if let Some(Found::Symbol(trait_symbol)) = trait_found
        && let Some(method) = self.child_named(trait_symbol, name)
      {
        defaults.push(Hit::exact(Found::Symbol(method)));
      }
    }
    unique(defaults)
  }

  pub(crate) fn rust_method_self(&self, method: SymRef) -> Option<Found> {
    if let Some(cached) = self.self_types.borrow().get(&method) {
      return cached.clone();
    }
    let resolved = self.table.sym(method).and_then(|symbol| symbol.impl_of.clone()).and_then(|impl_of| {
      let scope = self.symbol_scope(method)?;
      self
        .rust_resolve_path(&scope, &impl_of.self_type.segments, Ns::Type, &mut Guard::default())
        .ok()
        .map(|hit| hit.found)
    });
    self.self_types.borrow_mut().insert(method, resolved.clone());
    resolved
  }

  pub(crate) fn rust_method_trait(&self, method: SymRef) -> Option<Found> {
    let trait_ref = self.table.sym(method)?.impl_of.as_ref()?.trait_ref.clone()?;
    let scope = self.symbol_scope(method)?;
    self
      .rust_resolve_path(&scope, &trait_ref.segments, Ns::Type, &mut Guard::default())
      .ok()
      .map(|hit| hit.found)
  }

  pub(crate) fn rust_impl_targets(&self, file: FileIdx, position: u32) -> ImplTargets {
    if let Some(cached) = self.impl_targets.borrow().get(&(file, position)) {
      return cached.clone();
    }
    let resolved = self.rust_impl_targets_uncached(file, position).unwrap_or((None, None));
    self.impl_targets.borrow_mut().insert((file, position), resolved.clone());
    resolved
  }

  fn rust_impl_targets_uncached(&self, file: FileIdx, position: u32) -> Option<ImplTargets> {
    let surface = self.table.file(file)?;
    let implementation = surface.rust.impls.get(position as usize)?;
    let scope = match implementation.scope {
      Some(local) if surface.symbol(local).is_some_and(SymSurface::is_function_like) => self.rust_scope(file, Some(local))?,
      _ => RustScope::at_module(file, self.table.rust_module_of(file, &implementation.module_suffix)?),
    };
    let self_found = self
      .rust_resolve_path(&scope, &implementation.self_type.segments, Ns::Type, &mut Guard::default())
      .ok()
      .map(|hit| hit.found);
    let trait_found = implementation
      .trait_ref
      .as_ref()
      .and_then(|trait_ref| self.rust_resolve_path(&scope, &trait_ref.segments, Ns::Type, &mut Guard::default()).ok())
      .map(|hit| hit.found);
    Some((self_found, trait_found))
  }

  fn rust_self_type(&self, scope: &RustScope) -> Lookup {
    let Some(surface) = self.table.file(scope.file) else { return Err(Miss::NotFound) };
    let Some(from) = scope.from else {
      return Err(Miss::Unsupported("Self outside an impl or trait"));
    };
    for ancestor in surface.ancestors(from) {
      let Some(symbol) = surface.symbol(ancestor) else { continue };
      let reference = SymRef {
        file: scope.file,
        local: ancestor,
      };
      if symbol.impl_of.is_some() {
        return self.rust_method_self(reference).map(Hit::exact).ok_or(Miss::NotFound);
      }
      if symbol.kind == EntityKind::Trait {
        return Ok(Hit::exact(Found::Symbol(reference)));
      }
    }
    Err(Miss::Unsupported("Self outside an impl or trait"))
  }

  fn rust_method_call(&self, scope: &RustScope, reference: &ReferenceFact) -> Outcome {
    let path = &reference.path;
    let Some((method, receiver_chain)) = path.split_last() else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    if receiver_chain.is_empty() {
      return Outcome::Unresolved(Miss::NotFound);
    }
    let caller = scope.from.map(|local| SymRef { file: scope.file, local });
    let receiver = self.rust_chain_recv(scope, reference.receiver.as_ref(), receiver_chain);
    self.rust_method_on_recv(receiver, method, caller)
  }

  fn rust_chain_recv(&self, scope: &RustScope, hint: Option<&ReceiverHint>, chain: &[String]) -> Recv {
    let Some((head, fields)) = chain.split_first() else { return Recv::unknown() };
    let mut receiver = match hint {
      Some(ReceiverHint::SelfValue) => self.rust_self_recv(scope),
      Some(ReceiverHint::TypeHint(hint)) => self.rust_hint_recv(scope, head, hint),
      Some(ReceiverHint::Declared { type_text, .. }) => self.recv_from_text(scope, type_text, &scope.generics),
      Some(ReceiverHint::Expr(name)) if name == head => self.rust_generic_recv(scope, head),
      Some(ReceiverHint::Identifier(name)) if name == head => self.rust_identifier_recv(scope, name),
      _ => Recv::unknown(),
    };
    for field in fields {
      receiver = self.rust_field_type(receiver, field);
    }
    receiver
  }

  fn rust_field_access(&self, scope: &RustScope, reference: &ReferenceFact) -> Outcome {
    let Some((field, receiver_chain)) = reference.path.split_last() else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    if receiver_chain.is_empty() {
      return Outcome::Unresolved(Miss::NotFound);
    }
    let receiver = self.rust_chain_recv(scope, reference.receiver.as_ref(), receiver_chain);
    match receiver.ty {
      RecvType::Concrete(owner) => match self.rust_field_of(owner, field) {
        Some(member) => Outcome::Targets {
          primary: Hit::exact(Found::Symbol(member)).weaken(receiver.heuristic),
          extra: Vec::new(),
        },
        None => Outcome::Unresolved(Miss::NotFound),
      },
      RecvType::External(package) => Outcome::External(package),
      RecvType::Unknown if has_declared_type(reference.receiver.as_ref()) => Outcome::Unresolved(Miss::NotFound),
      RecvType::Unknown => self.rust_field_name_candidates(field),
      RecvType::Traits { .. } | RecvType::Static(_) | RecvType::Namespace(_) => Outcome::Unresolved(Miss::NotFound),
    }
  }

  fn rust_field_of(&self, owner: SymRef, name: &str) -> Option<SymRef> {
    self.note(name);
    let file = self.table.file(owner.file)?;
    file
      .children(owner.local)
      .iter()
      .find(|child| file.symbol(**child).is_some_and(|symbol| symbol.kind == EntityKind::Field && symbol.name == name))
      .map(|child| SymRef { file: owner.file, local: *child })
  }

  fn rust_field_name_candidates(&self, name: &str) -> Outcome {
    self.note(name);
    let Some(package) = self.table.file_package(self.file) else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    let mut candidates: Vec<SymRef> = self
      .table
      .rust_fields_by_name
      .get(name)
      .map(|fields| {
        fields
          .iter()
          .copied()
          .filter(|field| self.is_visible(field.file))
          .filter(|field| self.table.file_package(field.file) == Some(package) || self.table.sym(*field).is_some_and(|surface| surface.visibility == Visibility::Public))
          .collect()
      })
      .unwrap_or_default();
    candidates.sort();
    match candidates.as_slice() {
      [] => Outcome::Unresolved(Miss::NotFound),
      [single] if !COMMON_STD_FIELDS.contains(&name) => Outcome::Candidates(vec![Hit::heuristic(Found::Symbol(*single), FIELD_NAME_CONFIDENCE, RULE_FIELD_NAME)]),
      _ => Outcome::Unresolved(Miss::NameOnly(candidates.into_iter().map(|candidate| Hit::exact(Found::Symbol(candidate))).collect())),
    }
  }

  fn rust_method_on_recv(&self, receiver: Recv, method: &str, caller: Option<SymRef>) -> Outcome {
    self.note(method);
    let carried = receiver.heuristic;
    let wrapper = receiver.wrapper;
    if let Some(package) = &wrapper
      && WRAPPER_METHODS.contains(&method)
    {
      return Outcome::External(package.clone());
    }
    match receiver.ty {
      RecvType::Concrete(owner) => match self.rust_method_or_std(owner, method) {
        Ok(Hit {
          found: Found::External(package), ..
        }) => Outcome::External(package),
        Ok(hit) => Outcome::Targets {
          primary: hit.weaken(carried),
          extra: Vec::new(),
        },
        Err(Miss::NotFound) => match wrapper {
          Some(package) => Outcome::External(package),
          None if self.rust_type_may_have_generated_items(owner) => Outcome::Unresolved(Miss::MacroGenerated),
          None => Outcome::Unresolved(Miss::NotFound),
        },
        Err(miss) => Outcome::Unresolved(miss),
      },
      RecvType::Traits { traits, external } => {
        let hits: Vec<Hit> = traits
          .iter()
          .filter_map(|owner| self.child_named(*owner, method))
          .map(|found| Hit::exact(Found::Symbol(found)))
          .collect();
        match unique(hits) {
          Ok(primary) => {
            let extra = primary
              .symbol()
              .map(|declaration| self.rust_dispatch_candidates(declaration, RULE_DYN_DISPATCH))
              .unwrap_or_default();
            Outcome::Targets {
              primary: primary.weaken(carried),
              extra,
            }
          }
          Err(Miss::NotFound) => match external.or(wrapper) {
            Some(package) => Outcome::External(package),
            None => Outcome::Unresolved(Miss::NotFound),
          },
          Err(miss) => Outcome::Unresolved(miss),
        }
      }
      RecvType::External(package) => Outcome::External(package),
      RecvType::Unknown => self.method_name_candidates(method, caller),
      RecvType::Static(_) | RecvType::Namespace(_) => Outcome::Unresolved(Miss::NotFound),
    }
  }

  fn rust_dispatch_candidates(&self, declaration: SymRef, rule: &'static str) -> Vec<Hit> {
    let Some(surface) = self.table.sym(declaration) else { return Vec::new() };
    let Some(trait_symbol) = surface.parent.map(|parent| SymRef {
      file: declaration.file,
      local: parent,
    }) else {
      return Vec::new();
    };
    let Some(trait_name) = self.table.sym(trait_symbol).map(|symbol| symbol.name.clone()) else {
      return Vec::new();
    };
    self.note(&trait_name);
    let mut candidates = Vec::new();
    for (file, position) in self.table.rust_impls_by_trait.get(&trait_name).map(Vec::as_slice).unwrap_or_default() {
      let (_, trait_found) = self.rust_impl_targets(*file, *position);
      if trait_found != Some(Found::Symbol(trait_symbol)) {
        continue;
      }
      let Some(impl_file) = self.table.file(*file) else { continue };
      let Some(implementation) = impl_file.rust.impls.get(*position as usize) else { continue };
      let method = impl_file.symbols.iter().position(|symbol| {
        symbol.kind == EntityKind::Method
          && symbol.name == surface.name
          && symbol.module_suffix == implementation.module_suffix
          && symbol
            .impl_of
            .as_ref()
            .is_some_and(|impl_of| impl_of.self_type == implementation.self_type && impl_of.trait_ref == implementation.trait_ref)
      });
      if let Some(local) = method {
        candidates.push(SymRef { file: *file, local: local as u32 });
      }
    }
    candidates.sort_by_key(|candidate| self.table.symbol_entity(*candidate));
    candidates.dedup();
    candidates.truncate(MAX_DISPATCH_CANDIDATES);
    candidates
      .into_iter()
      .map(|candidate| Hit::heuristic(Found::Symbol(candidate), DISPATCH_CONFIDENCE, rule))
      .collect()
  }

  fn rust_self_recv(&self, scope: &RustScope) -> Recv {
    match self.rust_self_type(scope) {
      Ok(hit) => self.recv_from_type_hit(hit),
      Err(_) => Recv::unknown(),
    }
  }

  fn recv_from_type_hit(&self, hit: Hit) -> Recv {
    let ty = match hit.found {
      Found::Symbol(symbol) => match self.sym_kind(symbol) {
        Some(EntityKind::Trait) => RecvType::Traits {
          traits: vec![symbol],
          external: None,
        },
        Some(EntityKind::Struct | EntityKind::Enum) => RecvType::Concrete(symbol),
        _ => RecvType::Unknown,
      },
      Found::External(package) => RecvType::External(package),
      _ => RecvType::Unknown,
    };
    Recv::of(ty, hit.heuristic)
  }

  fn param_type<'s>(&'s self, scope: &RustScope, name: &str) -> Option<(u32, &'s str)> {
    let surface = self.table.file(scope.file)?;
    scope.fns.iter().find_map(|function| {
      let symbol = surface.symbol(*function)?;
      let (_, text) = symbol.params.iter().find(|(param, _)| param == name)?;
      Some((*function, text.as_deref()?))
    })
  }

  fn rust_hint_recv(&self, scope: &RustScope, head: &str, hint: &[String]) -> Recv {
    if !is_deref_wrapper(hint) {
      return self.recv_from_shape(scope, TypeShape::Path(hint.to_vec()), &scope.generics);
    }
    let Some((_, text)) = self.param_type(scope, head) else { return Recv::unknown() };
    let same_wrapper = rust_outer_path(text).is_some_and(|outer| outer.last() == hint.last());
    if !same_wrapper {
      return Recv::unknown();
    }
    self.recv_from_text(scope, text, &scope.generics).recovered_from_param()
  }

  fn rust_generic_recv(&self, scope: &RustScope, head: &str) -> Recv {
    let Some((function, text)) = self.param_type(scope, head) else { return Recv::unknown() };
    let Some(generic) = rust_generic_param(text) else { return Recv::unknown() };
    let Some(bounds) = self
      .table
      .file(scope.file)
      .and_then(|surface| surface.symbol(function))
      .and_then(|symbol| symbol.generics.iter().find(|(name, _)| name == generic).map(|(_, bounds)| bounds.clone()))
    else {
      return Recv::unknown();
    };
    let paths: Vec<Vec<String>> = bounds.iter().filter_map(|bound| rust_outer_path(bound)).collect();
    self.recv_from_shape(scope, TypeShape::Bounds(paths), &[]).recovered_from_param()
  }

  fn rust_identifier_recv(&self, scope: &RustScope, name: &str) -> Recv {
    let Ok(hit) = self.rust_lookup_name(scope, name, Ns::Value, &mut Guard::default()) else {
      return Recv::unknown();
    };
    match hit.found {
      Found::Symbol(symbol) => match self.table.sym(symbol) {
        Some(surface) if surface.kind == EntityKind::Struct => Recv::of(RecvType::Concrete(symbol), hit.heuristic),
        Some(surface) => match (&surface.value_type, self.symbol_scope(symbol)) {
          (Some(ValueType::Declared(text)), Some(owner_scope)) => self.recv_from_text(&owner_scope, text, &[]),
          _ => Recv::unknown(),
        },
        None => Recv::unknown(),
      },
      Found::External(package) => Recv::of(RecvType::External(package), None),
      _ => Recv::unknown(),
    }
  }

  fn rust_field_type(&self, receiver: Recv, field: &str) -> Recv {
    match receiver.ty {
      RecvType::Concrete(owner) => {
        let Some(member) = self.child_named(owner, field) else { return Recv::unknown() };
        let Some(surface) = self.table.sym(member) else { return Recv::unknown() };
        let (EntityKind::Field, Some(ValueType::Declared(text))) = (&surface.kind, &surface.value_type) else {
          return Recv::unknown();
        };
        let Some(scope) = self.symbol_scope(member) else { return Recv::unknown() };
        let owner_generics = self.table.sym(owner).map(|symbol| symbol.generics.clone()).unwrap_or_default();
        let mut resolved = self.recv_from_text(&scope, text, &owner_generics);
        resolved.heuristic = resolved.heuristic.or(receiver.heuristic);
        resolved
      }
      RecvType::External(package) => Recv::of(RecvType::External(package), receiver.heuristic),
      _ => Recv::unknown(),
    }
  }

  fn recv_from_shape(&self, scope: &RustScope, shape: TypeShape, generics: &[(String, Vec<String>)]) -> Recv {
    match shape {
      TypeShape::Path(path) => {
        if let [single] = path.as_slice()
          && let Some((_, bounds)) = generics.iter().find(|(generic, _)| generic == single)
        {
          if bounds.is_empty() {
            return Recv::unknown();
          }
          let paths: Vec<Vec<String>> = bounds.iter().filter_map(|bound| rust_outer_path(bound)).collect();
          return self.recv_from_shape(scope, TypeShape::Bounds(paths), &[]);
        }
        match self.rust_resolve_path(scope, &path, Ns::Type, &mut Guard::default()) {
          Ok(hit) => self.recv_from_type_hit(hit),
          Err(_) => Recv::unknown(),
        }
      }
      TypeShape::Bounds(paths) => {
        let mut traits = Vec::new();
        let mut external = None;
        for path in paths {
          match self.rust_resolve_path(scope, &path, Ns::Type, &mut Guard::default()).map(|hit| hit.found) {
            Ok(Found::Symbol(symbol)) if self.sym_kind(symbol) == Some(&EntityKind::Trait) => traits.push(symbol),
            Ok(Found::External(package)) => {
              external.get_or_insert(package);
            }
            _ => return Recv::unknown(),
          }
        }
        match (traits.is_empty(), external) {
          (true, Some(package)) => Recv::of(RecvType::External(package), None),
          (true, None) => Recv::unknown(),
          (false, external) => Recv::of(RecvType::Traits { traits, external }, None),
        }
      }
      TypeShape::Unknown => Recv::unknown(),
    }
  }

  fn rust_macro(&self, scope: &RustScope, reference: &ReferenceFact, guard: &mut Guard) -> Outcome {
    let path = &reference.path;
    let [name] = path.as_slice() else {
      return Outcome::from_lookup(self.rust_resolve_path(scope, path, Ns::Macro, guard));
    };
    self.note(name);
    if let Some(textual) = self.rust_textual_macro(scope, name, reference.span.start_byte) {
      return Outcome::from_lookup(Ok(Hit::exact(Found::Symbol(textual))));
    }
    match self.rust_lookup_scoped(scope, name, Ns::Macro, guard) {
      Err(Miss::NotFound) => {}
      other => return Outcome::from_lookup(other),
    }
    let same_crate: Vec<Hit> = self
      .table
      .macros_by_name
      .get(name)
      .map(|macros| {
        macros
          .iter()
          .copied()
          .filter(|symbol| symbol.file != scope.file)
          .filter(|symbol| self.table.rust_module_of(symbol.file, &[]).is_some_and(|module| module.same_crate(&scope.module)))
          .map(|symbol| Hit::heuristic(Found::Symbol(symbol), MACRO_SCOPE_CONFIDENCE, RULE_MACRO_SCOPE))
          .collect()
      })
      .unwrap_or_default();
    match unique(same_crate) {
      Err(Miss::NotFound) => Outcome::from_lookup(self.rust_prelude(scope, name, Ns::Macro)),
      Ok(hit) => Outcome::Candidates(vec![hit]),
      Err(miss) => Outcome::Unresolved(miss),
    }
  }

  fn rust_textual_macro(&self, scope: &RustScope, name: &str, before: u32) -> Option<SymRef> {
    let surface = self.table.file(scope.file)?;
    let reference_suffix = scope
      .from
      .and_then(|local| surface.symbol(local))
      .map(|symbol| symbol.module_suffix.clone())
      .unwrap_or_default();
    surface
      .symbols
      .iter()
      .enumerate()
      .filter(|(_, symbol)| symbol.kind == EntityKind::Macro && symbol.name == name && symbol.span.start_byte < before && reference_suffix.starts_with(&symbol.module_suffix))
      .max_by_key(|(_, symbol)| symbol.span.start_byte)
      .map(|(local, _)| SymRef {
        file: scope.file,
        local: local as u32,
      })
  }

  fn rust_macro_may_generate(&self, scope: &RustScope, path: &[String], guard: &mut Guard) -> bool {
    if let Some((_, owner_path)) = path.split_last()
      && !owner_path.is_empty()
    {
      return match self.rust_resolve_path(scope, owner_path, Ns::Type, guard).map(|hit| hit.found) {
        Ok(Found::Symbol(owner)) => self.rust_type_may_have_generated_items(owner),
        Ok(Found::Module(module)) => self.module_has_item_macros(&module),
        _ => false,
      };
    }
    self.module_has_item_macros(&scope.module)
  }

  fn module_has_item_macros(&self, module: &ModuleId) -> bool {
    self.table.module_entry(module).is_some_and(|entry| {
      entry
        .effective_locs()
        .any(|location| self.table.file(location.file).is_some_and(|file| file.rust.item_macro_calls))
    })
  }

  fn rust_type_may_have_generated_items(&self, owner: SymRef) -> bool {
    let Some(name) = self.table.sym(owner).map(|symbol| symbol.name.clone()) else {
      return false;
    };
    let defining = self.table.file(owner.file).is_some_and(|file| file.rust.item_macro_calls);
    defining
      || self.table.rust_impls_by_self.get(&name).is_some_and(|impls| {
        impls.iter().any(|(file, position)| {
          self.table.file(*file).is_some_and(|surface| surface.rust.item_macro_calls) && self.rust_impl_targets(*file, *position).0 == Some(Found::Symbol(owner))
        })
      })
  }

  pub(crate) fn rust_use_targets(&self, entry: &UseSurface) -> Vec<Hit> {
    let module_suffix = match &entry.scope {
      UseScope::Module(suffix) => suffix.clone(),
      UseScope::Function(local) => self.surface.symbol(*local).map(|symbol| symbol.module_suffix.clone()).unwrap_or_default(),
    };
    let Some(module) = self.table.rust_module_of(self.file, &module_suffix) else {
      return Vec::new();
    };
    let scope = RustScope::at_module(self.file, module);
    let namespaces: &[Ns] = match entry.binding {
      atlas_core::UseBinding::Glob => &[Ns::Type],
      _ => &[Ns::Type, Ns::Value, Ns::Macro],
    };
    let mut hits: Vec<Hit> = Vec::new();
    for ns in namespaces {
      if let Ok(hit) = self.rust_resolve_path(&scope, entry.target_segments(), *ns, &mut Guard::default())
        && !hits.iter().any(|existing| existing.found == hit.found)
      {
        hits.push(hit);
      }
    }
    hits
  }

  fn is_trait_method(&self, hit: &Hit) -> bool {
    let Some(symbol) = hit.symbol() else { return false };
    let Some(parent) = self.table.sym(symbol).and_then(|surface| surface.parent) else {
      return false;
    };
    self.sym_kind(SymRef { file: symbol.file, local: parent }) == Some(&EntityKind::Trait)
  }

  pub(crate) fn implements_project_trait_or_inherent(&self, method: SymRef) -> bool {
    match self.table.sym(method).and_then(|symbol| symbol.impl_of.as_ref()) {
      Some(impl_of) if impl_of.trait_ref.is_some() => matches!(self.rust_method_trait(method), Some(Found::Symbol(_))),
      _ => true,
    }
  }

  fn recv_from_text(&self, scope: &RustScope, text: &str, generics: &[(String, Vec<String>)]) -> Recv {
    let mut receiver = self.recv_from_shape(scope, rust_type_shape(text), generics);
    if rust_outer_path(text).is_some_and(|outer| is_deref_wrapper(&outer)) {
      receiver.wrapper = Some("std".to_owned());
    }
    receiver
  }
}
