use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use atlas_core::{EntityKind, LanguageFamily, RepoName};

use crate::layout::PackageKey;
use crate::surface::FileSurface;
use crate::table::{FileIdx, ModuleId, ProjectSymbolTable, Reachability, SymRef};

pub(crate) const MAX_DEPTH: usize = 48;
pub(crate) const MAX_UNKNOWN_RECEIVER_CANDIDATES: usize = 3;
pub(crate) const MAX_AMBIGUOUS_CANDIDATES: usize = 32;
pub(crate) const MAX_DISPATCH_CANDIDATES: usize = 64;
pub(crate) const DISPATCH_CONFIDENCE: f32 = 0.3;
pub(crate) const DIST_TO_SRC_CONFIDENCE: f32 = 0.85;
pub(crate) const MACRO_SCOPE_CONFIDENCE: f32 = 0.6;
pub(crate) const MACRO_TOKEN_SCAN_CONFIDENCE: f32 = 0.6;
pub(crate) const PARAM_TYPE_CONFIDENCE: f32 = 0.7;
pub(crate) const FN_LOCAL_ITEM_CONFIDENCE: f32 = 0.5;
pub(crate) const FIELD_NAME_CONFIDENCE: f32 = 0.5;

const COMMON_RUST_METHODS: [&str; 98] = [
  "iter",
  "kind",
  "iter_mut",
  "into_iter",
  "push",
  "push_str",
  "pop",
  "first",
  "last",
  "get",
  "get_mut",
  "insert",
  "remove",
  "contains",
  "contains_key",
  "len",
  "is_empty",
  "join",
  "map",
  "map_err",
  "and_then",
  "or_else",
  "filter",
  "filter_map",
  "flat_map",
  "collect",
  "clone",
  "cloned",
  "copied",
  "to_string",
  "to_owned",
  "as_str",
  "as_ref",
  "as_mut",
  "as_slice",
  "unwrap",
  "unwrap_or",
  "unwrap_or_default",
  "unwrap_or_else",
  "expect",
  "ok",
  "err",
  "next",
  "extend",
  "retain",
  "sort",
  "sort_by",
  "sort_by_key",
  "sort_unstable",
  "dedup",
  "keys",
  "values",
  "entry",
  "or_insert",
  "or_default",
  "read",
  "write",
  "path",
  "text",
  "split",
  "trim",
  "starts_with",
  "ends_with",
  "find",
  "position",
  "any",
  "all",
  "count",
  "sum",
  "min",
  "max",
  "chars",
  "bytes",
  "lines",
  "parse",
  "replace",
  "take",
  "skip",
  "clear",
  "append",
  "drain",
  "truncate",
  "chain",
  "zip",
  "rev",
  "enumerate",
  "fold",
  "lock",
  "send",
  "recv",
  "fmt",
  "eq",
  "cmp",
  "hash",
  "flush",
  "is_some",
  "is_none",
  "is_ok",
];
const COMMON_TS_METHODS: [&str; 64] = [
  "map",
  "filter",
  "reduce",
  "forEach",
  "push",
  "pop",
  "shift",
  "unshift",
  "slice",
  "splice",
  "join",
  "split",
  "trim",
  "includes",
  "indexOf",
  "find",
  "findIndex",
  "some",
  "every",
  "concat",
  "keys",
  "values",
  "entries",
  "get",
  "set",
  "has",
  "delete",
  "add",
  "clear",
  "then",
  "catch",
  "finally",
  "toString",
  "toLowerCase",
  "toUpperCase",
  "replace",
  "startsWith",
  "endsWith",
  "json",
  "text",
  "sort",
  "reverse",
  "flat",
  "flatMap",
  "at",
  "fill",
  "apply",
  "call",
  "bind",
  "emit",
  "on",
  "log",
  "error",
  "warn",
  "info",
  "debug",
  "next",
  "send",
  "write",
  "read",
  "close",
  "open",
  "match",
  "test",
];

pub(crate) const RULE_DYN_DISPATCH: &str = "dyn-dispatch-candidate";
pub(crate) const RULE_INTERFACE_DISPATCH: &str = "interface-dispatch-candidate";
pub(crate) const RULE_METHOD_NAME: &str = "method-name-only";
pub(crate) const RULE_DIST_TO_SRC: &str = "dist-to-src";
pub(crate) const RULE_MACRO_SCOPE: &str = "macro-name-in-crate";
pub(crate) const RULE_MACRO_TOKEN_SCAN: &str = "macro-token-scan";
pub(crate) const RULE_PARAM_TYPE: &str = "param-type-recovery";
pub(crate) const RULE_FN_LOCAL_ITEM: &str = "fn-local-item-possible";
pub(crate) const RULE_OVERRIDE_DISPATCH: &str = "override-dispatch-candidate";
pub(crate) const RULE_FIELD_NAME: &str = "field-name-only";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Found {
  Symbol(SymRef),
  Module(ModuleId),
  File(FileIdx),
  External(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Heuristic {
  pub confidence: f32,
  pub rule: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hit {
  pub found: Found,
  pub heuristic: Option<Heuristic>,
}

impl Hit {
  pub fn exact(found: Found) -> Self {
    Self { found, heuristic: None }
  }

  pub fn heuristic(found: Found, confidence: f32, rule: &'static str) -> Self {
    Self {
      found,
      heuristic: Some(Heuristic { confidence, rule }),
    }
  }

  pub fn weaken(mut self, other: Option<Heuristic>) -> Self {
    self.heuristic = weakest(self.heuristic, other);
    self
  }

  pub fn symbol(&self) -> Option<SymRef> {
    match self.found {
      Found::Symbol(symbol) => Some(symbol),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Miss {
  NotFound,
  Ambiguous(Vec<Hit>),
  NameOnly(Vec<Hit>),
  Dynamic,
  MacroGenerated,
  Unsupported(&'static str),
}

pub(crate) type Lookup = Result<Hit, Miss>;

pub(crate) fn weakest(left: Option<Heuristic>, right: Option<Heuristic>) -> Option<Heuristic> {
  match (left, right) {
    (Some(left), Some(right)) => Some(if right.confidence < left.confidence { right } else { left }),
    (left, None) => left,
    (None, right) => right,
  }
}

pub(crate) fn unique(hits: Vec<Hit>) -> Lookup {
  let mut distinct: Vec<Hit> = Vec::new();
  for hit in hits {
    match distinct.iter_mut().find(|existing| existing.found == hit.found) {
      Some(existing) if existing.heuristic.is_some() && hit.heuristic.is_none() => *existing = hit,
      Some(_) => {}
      None => distinct.push(hit),
    }
  }
  match distinct.len() {
    0 => Err(Miss::NotFound),
    1 => Ok(distinct.remove(0)),
    _ => Err(Miss::Ambiguous(distinct)),
  }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecvType {
  Concrete(SymRef),
  Static(SymRef),
  Traits { traits: Vec<SymRef>, external: Option<String> },
  Namespace(FileIdx),
  External(String),
  Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recv {
  pub ty: RecvType,
  pub heuristic: Option<Heuristic>,
  pub wrapper: Option<String>,
}

impl Recv {
  pub fn unknown() -> Self {
    Self::of(RecvType::Unknown, None)
  }

  pub fn of(ty: RecvType, heuristic: Option<Heuristic>) -> Self {
    Self { ty, heuristic, wrapper: None }
  }

  pub fn weaken(mut self, heuristic: Heuristic) -> Self {
    self.heuristic = weakest(self.heuristic, Some(heuristic));
    self
  }

  pub fn recovered_from_param(self) -> Self {
    self.weaken(Heuristic {
      confidence: PARAM_TYPE_CONFIDENCE,
      rule: RULE_PARAM_TYPE,
    })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
  Targets { primary: Hit, extra: Vec<Hit> },
  Candidates(Vec<Hit>),
  Unresolved(Miss),
  External(String),
}

impl Outcome {
  pub fn from_lookup(lookup: Lookup) -> Self {
    match lookup {
      Ok(Hit {
        found: Found::External(package), ..
      }) => Self::External(package),
      Ok(primary) => Self::Targets { primary, extra: Vec::new() },
      Err(miss) => Self::Unresolved(miss),
    }
  }
}

pub(crate) type ImplTargets = (Option<Found>, Option<Found>);

#[derive(Debug, Default)]
pub(crate) struct Guard {
  stack: Vec<String>,
  pub macro_items: bool,
}

impl Guard {
  pub fn enter(&mut self, key: String) -> bool {
    if self.stack.len() >= MAX_DEPTH || self.stack.contains(&key) {
      return false;
    }
    self.stack.push(key);
    true
  }

  pub fn exit(&mut self) {
    self.stack.pop();
  }

  pub fn is_top_level(&self) -> bool {
    self.stack.is_empty() && !self.macro_items
  }
}

enum CandidateScope {
  Packages(HashSet<PackageKey>),
  Repo(RepoName),
}

pub(crate) struct Resolver<'t> {
  pub table: &'t ProjectSymbolTable,
  pub file: FileIdx,
  pub surface: &'t FileSurface,
  visible: CandidateScope,
  pub(crate) self_types: RefCell<HashMap<SymRef, Option<Found>>>,
  pub(crate) impl_targets: RefCell<HashMap<(FileIdx, u32), ImplTargets>>,
  pub(crate) module_lookups: RefCell<HashMap<String, Lookup>>,
  lookups: RefCell<BTreeSet<String>>,
}

impl<'t> Resolver<'t> {
  pub fn new(table: &'t ProjectSymbolTable, file: FileIdx, surface: &'t FileSurface) -> Self {
    let visible = match table.file_package(file) {
      Some(package) => {
        let mut packages: HashSet<PackageKey> = table.layout.dependency_packages(package).into_iter().collect();
        packages.insert(package.clone());
        CandidateScope::Packages(packages)
      }
      None => CandidateScope::Repo(surface.repo.clone()),
    };
    Self {
      table,
      file,
      surface,
      visible,
      self_types: RefCell::new(HashMap::new()),
      impl_targets: RefCell::new(HashMap::new()),
      module_lookups: RefCell::new(HashMap::new()),
      lookups: RefCell::new(BTreeSet::new()),
    }
  }

  pub fn note(&self, name: &str) {
    if !self.lookups.borrow().contains(name) {
      self.lookups.borrow_mut().insert(name.to_owned());
    }
  }

  pub fn take_lookups(&self) -> BTreeSet<String> {
    self.lookups.take()
  }

  pub fn sym_kind(&self, symbol: SymRef) -> Option<&EntityKind> {
    self.table.sym(symbol).map(|surface| &surface.kind)
  }

  pub fn method_name_candidates(&self, name: &str, caller: Option<SymRef>) -> Outcome {
    self.note(name);
    let family = self.surface.family;
    let mut candidates: Vec<SymRef> = self
      .table
      .methods_by_name
      .get(name)
      .map(|methods| {
        methods
          .iter()
          .copied()
          .filter(|method| Some(*method) != caller)
          .filter(|method| self.table.file(method.file).is_some_and(|file| file.family == family) && self.is_visible(method.file))
          .filter(|method| self.table.file_package(method.file) == self.table.file_package(self.file) || !self.is_test_only(*method))
          .filter(|method| family != LanguageFamily::Rust || self.implements_project_trait_or_inherent(*method))
          .collect()
      })
      .unwrap_or_default();
    candidates.sort();
    if candidates.is_empty() {
      return Outcome::Unresolved(Miss::NotFound);
    }
    let common = match family {
      LanguageFamily::Rust => COMMON_RUST_METHODS.contains(&name),
      _ => COMMON_TS_METHODS.contains(&name),
    };
    if common || candidates.len() > MAX_UNKNOWN_RECEIVER_CANDIDATES {
      return Outcome::Unresolved(Miss::NameOnly(candidates.into_iter().map(|candidate| Hit::exact(Found::Symbol(candidate))).collect()));
    }
    let confidence = match candidates.len() {
      1 => 0.5,
      2 => 0.35,
      _ => 0.25,
    };
    Outcome::Candidates(
      candidates
        .into_iter()
        .map(|candidate| Hit::heuristic(Found::Symbol(candidate), confidence, RULE_METHOD_NAME))
        .collect(),
    )
  }

  fn is_test_only(&self, symbol: SymRef) -> bool {
    let gated = |cfg: &str| cfg.split(|c: char| !c.is_alphanumeric() && c != '_').any(|word| word == "test");
    if let Reachability::CfgGated { cfg } = self.table.effective_placement(symbol.file).reachability
      && gated(&cfg)
    {
      return true;
    }
    let (Some(file), Some(surface)) = (self.table.file(symbol.file), self.table.sym(symbol)) else {
      return false;
    };
    file.rust.module_decls.iter().any(|declaration| {
      declaration.inline.is_some()
        && declaration.cfg.as_deref().is_some_and(gated)
        && surface.module_suffix.len() > declaration.module_suffix.len()
        && surface.module_suffix.starts_with(&declaration.module_suffix)
        && surface.module_suffix[declaration.module_suffix.len()] == declaration.name
    })
  }

  pub(crate) fn is_visible(&self, file: FileIdx) -> bool {
    match &self.visible {
      CandidateScope::Packages(packages) => self.table.file_package(file).is_some_and(|package| packages.contains(package)),
      CandidateScope::Repo(repo) => self.table.file(file).is_some_and(|surface| surface.repo == *repo),
    }
  }

  pub(crate) fn child_named(&self, parent: SymRef, name: &str) -> Option<SymRef> {
    self.note(name);
    let file = self.table.file(parent.file)?;
    file
      .children(parent.local)
      .iter()
      .find(|child| file.symbol(**child).is_some_and(|symbol| symbol.name == name))
      .map(|child| SymRef { file: parent.file, local: *child })
  }
}
