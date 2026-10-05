use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use atlas_core::{
  ArgSummary, AttributeFact, ContentHash, EntityKind, ExportFact, FileFacts, ImplInfo, ImportFact, KeyContext, LanguageFamily, Modifier, Modifiers, ModuleBody, NormToken, RefKind,
  RelPath, RepoName, Span, TypePath, UseBinding, Visibility, token_code,
};

use crate::error::ResolveError;
use crate::typetext::{ValueType, rust_alias_target, rust_declared_type, rust_generic_bounds, rust_return_type, ts_return_type, ts_type_params, ts_value_type};

const LOCAL_ITEM_KEYWORDS: [&str; 9] = ["struct", "enum", "union", "trait", "type", "mod", "fn", "impl", "macro_rules!"];
const MIN_ITEM_BLOCK_BYTES: usize = "{mod m{}}".len();
const INITIALIZER_FRAME_BYTES: usize = " = ;".len();

#[derive(Debug, Clone)]
pub(crate) struct SymSurface {
  pub kind: EntityKind,
  pub name: String,
  pub parent: Option<u32>,
  pub module_suffix: Vec<String>,
  pub item_path: Vec<String>,
  pub disambiguator: u32,
  pub visibility: Visibility,
  pub modifiers: Modifiers,
  pub impl_of: Option<ImplInfo>,
  pub extends: Vec<Vec<String>>,
  pub implements: Vec<Vec<String>>,
  pub params: Vec<(String, Option<String>)>,
  pub value_type: Option<ValueType>,
  pub return_type: Option<String>,
  pub alias_target: Option<String>,
  pub generics: Vec<(String, Vec<String>)>,
  pub exported_macro: bool,
  pub derives: Vec<String>,
  pub has_body: bool,
  pub declares_local_types: bool,
  pub annotation_end_byte: Option<u32>,
  pub span: Span,
}

impl SymSurface {
  pub fn is_function_like(&self) -> bool {
    matches!(self.kind, EntityKind::Function | EntityKind::Method | EntityKind::Test)
  }

  pub fn is_getter(&self) -> bool {
    self.modifiers.contains(Modifier::Getter)
  }

  pub fn is_static(&self) -> bool {
    self.modifiers.contains(Modifier::Static)
  }

  pub fn generic_names(&self) -> impl Iterator<Item = &str> {
    self.generics.iter().map(|(name, _)| name.as_str())
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum UseScope {
  Module(Vec<String>),
  Function(u32),
}

#[derive(Debug, Clone)]
pub(crate) struct UseSurface {
  pub scope: UseScope,
  pub visibility: Visibility,
  pub segments: Vec<String>,
  pub binding: UseBinding,
  pub span: Span,
}

impl UseSurface {
  pub fn bound_name(&self) -> Option<&str> {
    match &self.binding {
      UseBinding::Name => self.segments.iter().rev().find(|segment| *segment != "self").map(String::as_str),
      UseBinding::Alias(alias) => Some(alias),
      UseBinding::Glob => None,
    }
  }

  pub fn target_segments(&self) -> &[String] {
    match self.segments.split_last() {
      Some((last, rest)) if last == "self" => rest,
      _ => &self.segments,
    }
  }
}

#[derive(Debug, Clone)]
pub(crate) struct ModDeclSurface {
  pub name: String,
  pub module_suffix: Vec<String>,
  pub inline: Option<u32>,
  pub path_attr: Option<String>,
  pub cfg: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImplSurface {
  pub self_type: TypePath,
  pub trait_ref: Option<TypePath>,
  pub generic_params: Vec<String>,
  pub module_suffix: Vec<String>,
  pub scope: Option<u32>,
  pub span: Span,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RustSurface {
  pub uses: Vec<UseSurface>,
  pub extern_crates: Vec<(String, String)>,
  pub module_decls: Vec<ModDeclSurface>,
  pub impls: Vec<ImplSurface>,
  pub item_macro_calls: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TsSurface {
  pub imports: Vec<ImportFact>,
  pub exports: Vec<ExportFact>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileSurface {
  pub repo: RepoName,
  pub path: RelPath,
  pub family: LanguageFamily,
  pub content_hash: ContentHash,
  pub context: KeyContext,
  pub symbols: Vec<SymSurface>,
  pub rust: RustSurface,
  pub ts: TsSurface,
  items: HashMap<(Vec<String>, String), Vec<u32>>,
  children: HashMap<u32, Vec<u32>>,
}

impl FileSurface {
  pub fn from_facts(facts: &FileFacts, context: &KeyContext) -> Result<Self, ResolveError> {
    let keys = facts.symbol_keys(context)?;
    let family = facts.file.language.family();
    let symbols: Vec<SymSurface> = facts
      .symbols
      .iter()
      .zip(&keys)
      .map(|(symbol, key)| symbol_surface(family, symbol, key.disambiguator()))
      .collect();
    let mut items: HashMap<(Vec<String>, String), Vec<u32>> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (index, symbol) in symbols.iter().enumerate() {
      let index = index as u32;
      if let Some(parent) = symbol.parent {
        children.entry(parent).or_default().push(index);
      }
      if is_scope_item(family, symbol, &symbols) {
        items.entry((symbol.module_suffix.clone(), symbol.name.clone())).or_default().push(index);
      }
    }
    Ok(Self {
      repo: facts.file.repo.clone(),
      path: facts.file.path.clone(),
      family,
      content_hash: facts.file.content_hash,
      context: context.clone(),
      rust: rust_surface(facts, &symbols),
      ts: ts_surface(facts),
      symbols,
      items,
      children,
    })
  }

  pub fn symbol(&self, local: u32) -> Option<&SymSurface> {
    self.symbols.get(local as usize)
  }

  pub fn items_named(&self, suffix: &[String], name: &str) -> &[u32] {
    self.items.get(&(suffix.to_vec(), name.to_owned())).map_or(&[], Vec::as_slice)
  }

  pub fn children(&self, parent: u32) -> &[u32] {
    self.children.get(&parent).map_or(&[], Vec::as_slice)
  }

  pub fn ancestors(&self, local: u32) -> impl Iterator<Item = u32> + '_ {
    std::iter::successors(Some(local), |current| self.symbol(*current).and_then(|symbol| symbol.parent))
  }

  pub fn module_decls_in(&self, suffix: &[String]) -> impl Iterator<Item = &ModDeclSurface> {
    let suffix = suffix.to_vec();
    self.rust.module_decls.iter().filter(move |declaration| declaration.module_suffix == suffix)
  }

  pub fn uses_in(&self, scope: &UseScope) -> impl Iterator<Item = &UseSurface> {
    let scope = scope.clone();
    self.rust.uses.iter().filter(move |entry| entry.scope == scope)
  }

  pub fn use_module_suffix(&self, entry: &UseSurface) -> Vec<String> {
    match &entry.scope {
      UseScope::Module(suffix) => suffix.clone(),
      UseScope::Function(local) => self.symbol(*local).map(|symbol| symbol.module_suffix.clone()).unwrap_or_default(),
    }
  }

  pub fn textual_macro(&self, reference_suffix: &[String], name: &str, before: u32) -> Option<u32> {
    self
      .symbols
      .iter()
      .enumerate()
      .filter(|(_, symbol)| symbol.kind == EntityKind::Macro && symbol.name == name && symbol.span.start_byte < before && reference_suffix.starts_with(&symbol.module_suffix))
      .max_by_key(|(_, symbol)| symbol.span.start_byte)
      .map(|(local, _)| local as u32)
  }

  pub fn use_binds_textual_macro(&self, entry: &UseSurface) -> bool {
    match entry.target_segments() {
      [name] => self.textual_macro(&self.use_module_suffix(entry), name, entry.span.start_byte).is_some(),
      _ => false,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Signature {
  pub names: BTreeMap<String, u64>,
  pub structural: u64,
}

impl Signature {
  pub fn changed_names(&self, other: &Signature) -> BTreeSet<String> {
    let mut changed: BTreeSet<String> = BTreeSet::new();
    for (name, digest) in &self.names {
      if other.names.get(name) != Some(digest) {
        changed.insert(name.clone());
      }
    }
    for name in other.names.keys() {
      if !self.names.contains_key(name) {
        changed.insert(name.clone());
      }
    }
    changed
  }
}

fn digest(value: impl std::fmt::Debug) -> u64 {
  let mut hasher = DefaultHasher::new();
  format!("{value:?}").hash(&mut hasher);
  hasher.finish()
}

impl FileSurface {
  pub fn signature(&self) -> Signature {
    let mut names: BTreeMap<String, u64> = BTreeMap::new();
    let mut add = |name: &str, value: u64| {
      let entry = names.entry(name.to_owned()).or_insert(0);
      *entry = entry.rotate_left(5) ^ value;
    };
    for symbol in &self.symbols {
      let parent = symbol.parent.and_then(|parent| self.symbol(parent)).map(|parent| parent.name.as_str());
      let value = digest((
        (
          &symbol.kind,
          &symbol.module_suffix,
          &symbol.item_path,
          symbol.disambiguator,
          &symbol.visibility,
          symbol.modifiers,
          &symbol.impl_of,
        ),
        (
          &symbol.extends,
          &symbol.implements,
          &symbol.params,
          &symbol.value_type,
          &symbol.return_type,
          &symbol.alias_target,
          &symbol.generics,
        ),
        (symbol.exported_macro, &symbol.derives, symbol.has_body, parent),
      ));
      add(&symbol.name, value);
      for heritage in symbol.extends.iter().chain(&symbol.implements) {
        if let Some(name) = heritage.last() {
          add(name, value);
        }
      }
      if let Some(impl_of) = &symbol.impl_of {
        for owner in impl_of
          .self_type
          .name()
          .into_iter()
          .chain(impl_of.trait_ref.as_ref().and_then(|trait_ref| trait_ref.name()))
        {
          add(owner, value);
        }
      }
    }
    let mut structural: Vec<u64> = Vec::new();
    for entry in &self.rust.uses {
      let value = digest((&entry.scope, &entry.visibility, &entry.segments, &entry.binding, self.use_binds_textual_macro(entry)));
      match entry.bound_name() {
        Some(name) => add(name, value),
        None => structural.push(value),
      }
    }
    for implementation in &self.rust.impls {
      let value = digest((
        &implementation.self_type,
        &implementation.trait_ref,
        &implementation.module_suffix,
        implementation.scope,
        &implementation.generic_params,
      ));
      for owner in implementation
        .self_type
        .name()
        .into_iter()
        .chain(implementation.trait_ref.as_ref().and_then(|trait_ref| trait_ref.name()))
      {
        add(owner, value);
      }
    }
    structural.push(digest((
      self
        .rust
        .module_decls
        .iter()
        .map(|declaration| (&declaration.name, &declaration.module_suffix, declaration.inline, &declaration.path_attr, &declaration.cfg))
        .collect::<Vec<_>>(),
      &self.rust.extern_crates,
      self.rust.item_macro_calls,
    )));
    for import in &self.ts.imports {
      let value = digest(import_shape(import));
      match import {
        ImportFact::TsImport { bindings, .. } | ImportFact::TsRequire { bindings, .. } => bindings.iter().for_each(|binding| add(binding.local(), value)),
        _ => {}
      }
    }
    for export in &self.ts.exports {
      let value = digest(export_shape(export));
      match export {
        ExportFact::Named { local, exported, .. } => {
          add(exported, value);
          add(local, value);
        }
        ExportFact::Default { .. } => add("default", value),
        ExportFact::ReExport { bindings, .. } => bindings.iter().for_each(|binding| add(&binding.exported, value)),
        ExportFact::ReExportAll { namespace: Some(namespace), .. } => add(namespace, value),
        ExportFact::CommonJs { exported: Some(exported), .. } => add(exported, value),
        _ => structural.push(value),
      }
    }
    Signature {
      names,
      structural: digest(structural),
    }
  }
}

fn import_shape(import: &ImportFact) -> String {
  match import {
    ImportFact::TsImport {
      specifier, bindings, type_only, ..
    } => format!("import {specifier} {bindings:?} {type_only}"),
    ImportFact::TsRequire { specifier, bindings, scope, .. } => format!("require {specifier:?} {bindings:?} {scope:?}"),
    _ => String::new(),
  }
}

fn export_shape(export: &ExportFact) -> String {
  match export {
    ExportFact::Named { local, exported, type_only, .. } => format!("named {local} {exported} {type_only}"),
    ExportFact::Default { target, .. } => format!("default {target:?}"),
    ExportFact::ReExport {
      specifier, bindings, type_only, ..
    } => format!("reexport {specifier} {bindings:?} {type_only}"),
    ExportFact::ReExportAll {
      specifier, namespace, type_only, ..
    } => format!("star {specifier} {namespace:?} {type_only}"),
    ExportFact::CommonJs { exported, value, .. } => format!("commonjs {exported:?} {value:?}"),
    _ => String::new(),
  }
}

fn is_scope_item(family: LanguageFamily, symbol: &SymSurface, symbols: &[SymSurface]) -> bool {
  match family {
    LanguageFamily::Rust => {
      symbol.item_path.len() == 1
        && symbol
          .parent
          .is_none_or(|parent| symbols.get(parent as usize).is_some_and(|parent| parent.kind == EntityKind::Module))
    }
    _ => symbol.parent.is_none() && symbol.item_path.len() == 1 && !matches!(symbol.kind, EntityKind::Test | EntityKind::TestSuite),
  }
}

fn initializer_fits_item_block(symbol: &atlas_core::SymbolFact, signature: &str) -> bool {
  let span_bytes = (symbol.span.end_byte - symbol.span.start_byte) as usize;
  span_bytes.saturating_sub(signature.len() + INITIALIZER_FRAME_BYTES) >= MIN_ITEM_BLOCK_BYTES
}

fn annotation_tail<'s>(symbol: &atlas_core::SymbolFact, signature: &'s str) -> Option<&'s str> {
  if symbol.kind != EntityKind::Constant {
    return None;
  }
  let name = &symbol.name;
  let name_end = ["const ", "static ", "mut "].iter().find_map(|keyword| {
    let needle = format!("{keyword}{name}");
    signature.find(&needle).map(|at| at + needle.len())
  })?;
  signature.get(name_end..)
}

fn annotation_declares_items(tail: &str) -> bool {
  tail.contains('{')
    && tail
      .split(|character: char| !(character.is_alphanumeric() || character == '_'))
      .any(|word| LOCAL_ITEM_KEYWORDS.iter().any(|keyword| keyword.trim_end_matches('!') == word))
}

fn annotation_end_byte(symbol: &atlas_core::SymbolFact, signature: &str) -> Option<u32> {
  let tail = annotation_tail(symbol, signature)?;
  if symbol.body.is_none() || tail.ends_with('…') || annotation_declares_items(tail) {
    return None;
  }
  Some(symbol.name_span.end_byte + tail.len() as u32)
}

fn symbol_surface(family: LanguageFamily, symbol: &atlas_core::SymbolFact, disambiguator: u32) -> SymSurface {
  let signature = symbol.signature.as_deref().unwrap_or_default();
  let is_value_holder = matches!(symbol.kind, EntityKind::Field | EntityKind::Constant | EntityKind::Variable);
  let is_callable = matches!(symbol.kind, EntityKind::Function | EntityKind::Method | EntityKind::Test);
  let is_generic_owner = is_callable
    || matches!(
      symbol.kind,
      EntityKind::Struct | EntityKind::Enum | EntityKind::Trait | EntityKind::TypeAlias | EntityKind::Class | EntityKind::Interface
    );
  let (value_type, return_type, generics) = match family {
    LanguageFamily::Rust => (
      is_value_holder.then(|| rust_declared_type(signature).map(ValueType::Declared)).flatten(),
      is_callable.then(|| rust_return_type(signature)).flatten(),
      if is_generic_owner { rust_generic_bounds(signature, &symbol.name) } else { Vec::new() },
    ),
    _ => (
      is_value_holder.then(|| ts_value_type(signature)).flatten(),
      is_callable.then(|| ts_return_type(signature)).flatten(),
      if is_generic_owner {
        ts_type_params(signature, &symbol.name).into_iter().map(|name| (name, Vec::new())).collect()
      } else {
        Vec::new()
      },
    ),
  };
  let keyword_codes: Vec<u32> = LOCAL_ITEM_KEYWORDS.iter().map(|keyword| token_code(NormToken::Keyword(keyword))).collect();
  let is_initialized_item = matches!(symbol.kind, EntityKind::Constant | EntityKind::Variable);
  let declares_local_types = family == LanguageFamily::Rust
    && match (&symbol.body, is_callable || is_initialized_item) {
      (Some(body), true) => {
        body.token_count as usize > body.normalized_tokens.len()
          || body.normalized_tokens.iter().any(|code| keyword_codes.contains(code))
          || annotation_tail(symbol, signature).is_some_and(annotation_declares_items)
      }
      (None, true) => is_initialized_item && (symbol.span.start_line != symbol.span.end_line || initializer_fits_item_block(symbol, signature)),
      (_, false) => false,
    };
  SymSurface {
    kind: symbol.kind.clone(),
    name: symbol.name.clone(),
    parent: symbol.parent.map(|parent| parent.0),
    module_suffix: symbol.module_suffix.clone(),
    item_path: symbol.item_path.clone(),
    disambiguator,
    visibility: symbol.visibility.clone(),
    modifiers: symbol.modifiers,
    impl_of: symbol.impl_of.clone(),
    extends: symbol.extends.clone(),
    implements: symbol.implements.clone(),
    params: symbol.params.iter().map(|param| (param.name.clone(), param.type_text.clone())).collect(),
    value_type,
    return_type,
    alias_target: (family == LanguageFamily::Rust && symbol.kind == EntityKind::TypeAlias && symbol.impl_of.is_none())
      .then(|| rust_alias_target(signature))
      .flatten(),
    generics,
    exported_macro: symbol.attributes.iter().any(|attribute| attribute.is("macro_export")),
    derives: symbol
      .attributes
      .iter()
      .filter(|attribute| attribute.is("derive"))
      .flat_map(|attribute| attribute.args.iter())
      .filter_map(|argument| match argument {
        ArgSummary::Path(path) => path.last().cloned(),
        _ => None,
      })
      .collect(),
    has_body: symbol.body.is_some(),
    declares_local_types,
    annotation_end_byte: annotation_end_byte(symbol, signature),
    span: symbol.span,
  }
}

fn rust_surface(facts: &FileFacts, symbols: &[SymSurface]) -> RustSurface {
  let mut surface = RustSurface::default();
  for import in &facts.imports {
    match import {
      ImportFact::RustUse { paths, visibility, scope, span } => {
        let scope = match scope.and_then(|local| symbols.get(local.index()).map(|symbol| (local.0, symbol))) {
          Some((_, symbol)) if symbol.kind == EntityKind::Module => UseScope::Module(symbol.module_suffix.clone()),
          Some((local, _)) => UseScope::Function(local),
          None => UseScope::Module(Vec::new()),
        };
        surface.uses.extend(paths.iter().map(|path| UseSurface {
          scope: scope.clone(),
          visibility: visibility.clone(),
          segments: path.segments.clone(),
          binding: path.binding.clone(),
          span: *span,
        }));
      }
      ImportFact::RustExternCrate { name, alias, .. } => surface.extern_crates.push((alias.clone().unwrap_or_else(|| name.clone()), name.clone())),
      _ => {}
    }
  }
  surface.module_decls = facts
    .module_decls
    .iter()
    .map(|declaration| ModDeclSurface {
      name: declaration.name.clone(),
      module_suffix: declaration.module_suffix.clone(),
      inline: match declaration.body {
        ModuleBody::Inline(local) => Some(local.0),
        ModuleBody::External => None,
      },
      path_attr: declaration.path_attr.clone(),
      cfg: cfg_text(&declaration.attributes),
    })
    .collect();
  surface.impls = facts
    .impls
    .iter()
    .map(|implementation| ImplSurface {
      self_type: implementation.self_type.clone(),
      trait_ref: implementation.trait_ref.clone(),
      generic_params: implementation.generic_params.clone(),
      module_suffix: implementation.module_suffix.clone(),
      scope: implementation.scope.map(|local| local.0),
      span: implementation.span,
    })
    .collect();
  surface.item_macro_calls = facts.references.iter().any(|reference| {
    reference.kind == RefKind::MacroCall
      && reference
        .from
        .is_none_or(|local| symbols.get(local.index()).is_some_and(|symbol| symbol.kind == EntityKind::Module))
  });
  surface
}

fn ts_surface(facts: &FileFacts) -> TsSurface {
  TsSurface {
    imports: facts
      .imports
      .iter()
      .filter(|import| matches!(import, ImportFact::TsImport { .. } | ImportFact::TsRequire { .. }))
      .cloned()
      .collect(),
    exports: facts.exports.clone(),
  }
}

pub(crate) fn cfg_text(attributes: &[AttributeFact]) -> Option<String> {
  let conditions: Vec<String> = attributes
    .iter()
    .filter(|attribute| attribute.is("cfg"))
    .flat_map(|attribute| attribute.args.iter().map(render_cfg))
    .collect();
  match conditions.as_slice() {
    [] => None,
    [single] => Some(single.clone()),
    _ => Some(format!("all({})", conditions.join(", "))),
  }
}

fn render_cfg(argument: &ArgSummary) -> String {
  match argument {
    ArgSummary::Path(segments) => segments.join("::"),
    ArgSummary::Str(value) => format!("{value:?}"),
    ArgSummary::Named { name, value } => format!("{name} = {}", render_cfg(value)),
    ArgSummary::Call { callee, args } => format!("{}({})", callee.join("::"), args.iter().map(render_cfg).collect::<Vec<_>>().join(", ")),
    ArgSummary::Other(text) => text.clone(),
    _ => "?".to_owned(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn attribute(args: Vec<ArgSummary>) -> AttributeFact {
    AttributeFact {
      path: vec!["cfg".into()],
      args,
      span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
    }
  }

  #[test]
  fn should_render_cfg_predicates_compactly() {
    assert_eq!(cfg_text(&[attribute(vec![ArgSummary::Path(vec!["test".into()])])]).as_deref(), Some("test"));
    let any = ArgSummary::Call {
      callee: vec!["any".into()],
      args: vec![
        ArgSummary::Path(vec!["unix".into()]),
        ArgSummary::Named {
          name: "feature".into(),
          value: Box::new(ArgSummary::Str("x".into())),
        },
      ],
    };
    assert_eq!(cfg_text(&[attribute(vec![any])]).as_deref(), Some("any(unix, feature = \"x\")"));
    assert_eq!(cfg_text(&[]), None);
  }
}
