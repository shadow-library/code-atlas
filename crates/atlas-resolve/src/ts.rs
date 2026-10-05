use std::collections::{HashSet, VecDeque};

use atlas_core::{
  ArgSummary, DefaultExportTarget, EntityKind, ExportFact, ImportBinding, ImportFact, Modifier, ReceiverHint, RefKind, ReferenceFact, RelPath, RepoName, Visibility,
};
use atlas_project::{NpmExportTarget, NpmExports, TsConfigInfo};

use crate::engine::{
  DISPATCH_CONFIDENCE, DIST_TO_SRC_CONFIDENCE, DispatchReach, DispatchScope, Found, Guard, Heuristic, Hit, Lookup, MAX_DISPATCH_CANDIDATES, Miss, Outcome, RULE_DIST_TO_SRC,
  RULE_INTERFACE_DISPATCH, RULE_OVERRIDE_DISPATCH, Recv, RecvType, Resolver, unique, weakest,
};
use crate::layout::PackageKey;
use crate::surface::SymSurface;
use crate::table::{FileIdx, SymRef};
use crate::typetext::{ValueType, ts_type_head};

const PROBE_EXTENSIONS: [&str; 9] = [".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"];
const SOURCE_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"];
const BUILD_EXTENSIONS: [&str; 7] = [".d.ts", ".d.mts", ".d.cts", ".js", ".mjs", ".cjs", ".jsx"];
const BUILD_DIRS: [&str; 8] = ["dist", "build", "lib", "out", "esm", "cjs", "es", "types"];
const EXPORT_CONDITIONS: [&str; 7] = ["types", "import", "module", "default", "require", "node", "browser"];
const MAX_HERITAGE_DEPTH: usize = 32;
const DYNAMIC_GLOBALS: [&str; 2] = ["eval", "Function"];
pub(crate) const GLOBAL_PACKAGE: &str = "global";
const NODE_BUILTINS: [&str; 27] = [
  "assert",
  "buffer",
  "child_process",
  "cluster",
  "crypto",
  "dns",
  "events",
  "fs",
  "http",
  "http2",
  "https",
  "module",
  "net",
  "os",
  "path",
  "perf_hooks",
  "process",
  "querystring",
  "readline",
  "stream",
  "string_decoder",
  "timers",
  "tls",
  "url",
  "util",
  "worker_threads",
  "zlib",
];
const GLOBALS: [&str; 81] = [
  "Math",
  "JSON",
  "Object",
  "Array",
  "String",
  "Number",
  "Boolean",
  "Symbol",
  "BigInt",
  "Date",
  "RegExp",
  "Error",
  "TypeError",
  "RangeError",
  "SyntaxError",
  "Promise",
  "Map",
  "Set",
  "WeakMap",
  "WeakSet",
  "Proxy",
  "Reflect",
  "Intl",
  "console",
  "process",
  "Buffer",
  "globalThis",
  "window",
  "document",
  "navigator",
  "location",
  "fetch",
  "setTimeout",
  "setInterval",
  "clearTimeout",
  "clearInterval",
  "queueMicrotask",
  "structuredClone",
  "parseInt",
  "parseFloat",
  "isNaN",
  "isFinite",
  "encodeURIComponent",
  "decodeURIComponent",
  "URL",
  "URLSearchParams",
  "Headers",
  "Request",
  "Response",
  "AbortController",
  "TextEncoder",
  "TextDecoder",
  "crypto",
  "performance",
  "localStorage",
  "sessionStorage",
  "Uint8Array",
  "ArrayBuffer",
  "Record",
  "Partial",
  "Pick",
  "Omit",
  "Readonly",
  "Required",
  "ReturnType",
  "Parameters",
  "Awaited",
  "NonNullable",
  "Exclude",
  "Extract",
  "describe",
  "it",
  "expect",
  "test",
  "jest",
  "vi",
  "beforeEach",
  "afterEach",
  "beforeAll",
  "afterAll",
  "require",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TsNs {
  Value,
  Type,
}

impl TsNs {
  pub(crate) fn other(self) -> Self {
    match self {
      Self::Value => Self::Type,
      Self::Type => Self::Value,
    }
  }

  fn tag(self) -> &'static str {
    match self {
      Self::Value => "v",
      Self::Type => "t",
    }
  }

  fn admits(self, kind: &EntityKind) -> bool {
    match self {
      Self::Value => !matches!(kind, EntityKind::Interface | EntityKind::TypeAlias),
      Self::Type => matches!(
        kind,
        EntityKind::Class | EntityKind::Interface | EntityKind::TypeAlias | EntityKind::Enum | EntityKind::Namespace
      ),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
  Instance,
  Static,
  Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverHead {
  This,
  Super,
  Other,
}

enum Member {
  Found(SymRef, Option<Heuristic>),
  External(String),
  Missing,
}

fn has_source_extension(path: &str) -> bool {
  SOURCE_EXTENSIONS.iter().any(|extension| path.ends_with(extension))
}

fn split_package(specifier: &str) -> (&str, &str) {
  let mut boundary = 0;
  let segments = if specifier.starts_with('@') { 2 } else { 1 };
  for (seen, (index, _)) in specifier.match_indices('/').enumerate() {
    if seen + 1 == segments {
      boundary = index;
      break;
    }
  }
  match boundary {
    0 => (specifier, ""),
    index => (&specifier[..index], &specifier[index + 1..]),
  }
}

fn export_paths(target: &NpmExportTarget, out: &mut Vec<String>) {
  match target {
    NpmExportTarget::Path(path) => out.push(path.clone()),
    NpmExportTarget::Blocked => {}
    NpmExportTarget::Fallbacks(targets) => targets.iter().for_each(|target| export_paths(target, out)),
    NpmExportTarget::Conditions(conditions) => {
      let mut ordered: Vec<&(String, NpmExportTarget)> = conditions.iter().collect();
      ordered.sort_by_key(|(condition, _)| EXPORT_CONDITIONS.iter().position(|known| known == condition).unwrap_or(EXPORT_CONDITIONS.len()));
      for (_, target) in ordered {
        export_paths(target, out);
      }
    }
  }
}

fn match_subpath(pattern: &str, wanted: &str) -> Option<String> {
  match pattern.split_once('*') {
    None => (pattern == wanted).then(String::new),
    Some((prefix, suffix)) => {
      let captured = wanted.strip_prefix(prefix)?.strip_suffix(suffix)?;
      Some(captured.to_owned())
    }
  }
}

pub(crate) fn specifier_display(specifier: &ArgSummary) -> String {
  match specifier {
    ArgSummary::Path(path) => path.join("."),
    ArgSummary::Template(parts) => {
      let names: Vec<String> = parts
        .iter()
        .filter_map(|part| match part {
          atlas_core::TemplatePart::Expr(ArgSummary::Path(path)) => Some(path.join(".")),
          _ => None,
        })
        .collect();
      match names.as_slice() {
        [single] => single.clone(),
        _ => specifier.to_pattern("${…}").unwrap_or_else(|| "<dynamic>".to_owned()),
      }
    }
    other => other.to_pattern("${…}").unwrap_or_else(|| "<dynamic>".to_owned()),
  }
}

impl Resolver<'_> {
  pub(crate) fn ts_reference(&self, reference: &ReferenceFact) -> Outcome {
    let path = &reference.path;
    if path.is_empty() {
      return Outcome::Unresolved(Miss::NotFound);
    }
    if path.iter().any(|segment| segment.starts_with('[') || segment.starts_with('(')) {
      return Outcome::Unresolved(Miss::Dynamic);
    }
    let from = reference.from.map(|local| local.0);
    let member_access = (path.len() > 1 && reference.receiver.is_some()) || reference.kind == RefKind::MethodCall;
    if member_access {
      return self.ts_member_call(from, reference);
    }
    let ns = match reference.kind {
      RefKind::TypeRef | RefKind::Inherit => TsNs::Type,
      RefKind::Call | RefKind::ValueRef | RefKind::Decorator | RefKind::JsxElement | RefKind::Instantiation => TsNs::Value,
      _ => return Outcome::Unresolved(Miss::Unsupported("reference kind has no typescript resolution rule")),
    };
    if ns == TsNs::Type && path.len() == 1 && self.ts_is_type_param(self.file, from, &path[0]) {
      return Outcome::Unresolved(Miss::Unsupported("generic type parameter"));
    }
    Outcome::from_lookup(self.ts_static_path(self.file, from, path, ns, &mut Guard::default()))
  }

  pub(crate) fn ts_type_anchor(&self, reference: &ReferenceFact) -> Option<Hit> {
    let (_, chain) = reference.path.split_last()?;
    let head = chain.first()?;
    if matches!(head.as_str(), "this" | "super") {
      return None;
    }
    match &reference.receiver {
      None => {}
      Some(ReceiverHint::Identifier(name)) if name == head => {}
      Some(_) => return None,
    }
    let from = reference.from.map(|local| local.0);
    if chain.len() == 1 && self.ts_is_type_param(self.file, from, head) {
      return None;
    }
    let hit = self.ts_static_path(self.file, from, chain, TsNs::Value, &mut Guard::default()).ok()?;
    let symbol = hit.symbol()?;
    matches!(self.sym_kind(symbol), Some(EntityKind::Class | EntityKind::Enum)).then_some(hit)
  }

  fn ts_is_type_param(&self, file: FileIdx, from: Option<u32>, name: &str) -> bool {
    let (Some(surface), Some(from)) = (self.table.file(file), from) else { return false };
    surface
      .ancestors(from)
      .any(|ancestor| surface.symbol(ancestor).is_some_and(|symbol| symbol.generic_names().any(|generic| generic == name)))
  }

  pub(crate) fn ts_static_path(&self, file: FileIdx, from: Option<u32>, path: &[String], ns: TsNs, guard: &mut Guard) -> Lookup {
    let Some((head, rest)) = path.split_first() else { return Err(Miss::NotFound) };
    let mut current = self.ts_lookup_name(file, from, head, if rest.is_empty() { ns } else { TsNs::Value }, guard);
    if !rest.is_empty() && current == Err(Miss::NotFound) {
      current = self.ts_lookup_name(file, from, head, TsNs::Type, guard);
    }
    let mut current = current?;
    for (index, segment) in rest.iter().enumerate() {
      let segment_ns = if index + 1 == rest.len() { ns } else { TsNs::Value };
      let carried = current.heuristic;
      current = self.ts_static_member(current, segment, segment_ns, guard)?.weaken(carried);
    }
    Ok(current)
  }

  fn ts_static_member(&self, owner: Hit, segment: &str, ns: TsNs, guard: &mut Guard) -> Lookup {
    match owner.found {
      Found::File(file) => match self.ts_export(file, segment, ns, guard) {
        Err(Miss::NotFound) => self.ts_export(file, segment, ns.other(), guard),
        other => other,
      },
      Found::External(_) => Ok(owner),
      Found::Module(_) => Err(Miss::NotFound),
      Found::Symbol(symbol) => match self.sym_kind(symbol) {
        Some(EntityKind::Class) => self.ts_member_hit(symbol, segment, Side::Static),
        Some(EntityKind::Interface) => self.ts_member_hit(symbol, segment, Side::Any),
        Some(EntityKind::Namespace | EntityKind::Enum) => self.child_named(symbol, segment).map(|child| Hit::exact(Found::Symbol(child))).ok_or(Miss::NotFound),
        Some(EntityKind::Constant | EntityKind::Variable | EntityKind::Field) => {
          let receiver = self.ts_value_recv(symbol);
          match self.ts_member_of_recv(&receiver, segment) {
            Some(Ok((member, heuristic))) => Ok(Hit::exact(Found::Symbol(member)).weaken(heuristic).weaken(receiver.heuristic)),
            Some(Err(package)) => Ok(Hit::exact(Found::External(package))),
            None => Err(Miss::NotFound),
          }
        }
        _ => Err(Miss::NotFound),
      },
    }
  }

  fn ts_member_hit(&self, owner: SymRef, name: &str, side: Side) -> Lookup {
    match self.ts_member_lookup(owner, name, side) {
      Member::Found(member, heuristic) => Ok(Hit::exact(Found::Symbol(member)).weaken(heuristic)),
      Member::External(package) => Ok(Hit::exact(Found::External(package))),
      Member::Missing => Err(Miss::NotFound),
    }
  }

  pub(crate) fn ts_lookup_name(&self, file: FileIdx, from: Option<u32>, name: &str, ns: TsNs, guard: &mut Guard) -> Lookup {
    self.note(name);
    if let Some(local) = self.ts_local(file, name, ns) {
      return Ok(Hit::exact(Found::Symbol(local)));
    }
    let Some(surface) = self.table.file(file) else { return Err(Miss::NotFound) };
    let enclosing: HashSet<u32> = from.map(|local| surface.ancestors(local).collect()).unwrap_or_default();
    for import in &surface.ts.imports {
      match import {
        ImportFact::TsImport { specifier, bindings, .. } => {
          if let Some(binding) = bindings.iter().find(|binding| binding.local() == name) {
            return self.ts_import_binding(file, specifier, binding, ns, guard);
          }
        }
        ImportFact::TsRequire { specifier, bindings, scope, .. } => {
          let in_scope = scope.is_none_or(|scope| enclosing.contains(&scope.0));
          let Some(binding) = bindings.iter().find(|binding| binding.local() == name).filter(|_| in_scope) else {
            continue;
          };
          return match specifier.as_static_str() {
            Some(literal) => self.ts_import_binding(file, &literal, binding, ns, guard),
            None => Err(Miss::Dynamic),
          };
        }
        _ => {}
      }
    }
    if DYNAMIC_GLOBALS.contains(&name) {
      return Err(Miss::Dynamic);
    }
    if GLOBALS.contains(&name) {
      return Ok(Hit::exact(Found::External(GLOBAL_PACKAGE.to_owned())));
    }
    Err(Miss::NotFound)
  }

  fn ts_pick(&self, file: FileIdx, candidates: Vec<u32>) -> Option<SymRef> {
    let surface = self.table.file(file)?;
    let overloads = candidates.len() > 1
      && candidates
        .iter()
        .all(|local| surface.symbol(*local).is_some_and(|symbol| symbol.kind == EntityKind::Function));
    let chosen = match overloads {
      true => candidates
        .iter()
        .find(|local| surface.symbol(**local).is_some_and(|symbol| symbol.has_body))
        .or(candidates.first()),
      false => candidates.first(),
    };
    chosen.map(|local| SymRef { file, local: *local })
  }

  fn ts_local(&self, file: FileIdx, name: &str, ns: TsNs) -> Option<SymRef> {
    let surface = self.table.file(file)?;
    let candidates: Vec<u32> = surface
      .items_named(&[], name)
      .iter()
      .copied()
      .filter(|local| surface.symbol(*local).is_some_and(|symbol| ns.admits(&symbol.kind)))
      .collect();
    self.ts_pick(file, candidates)
  }

  fn ts_import_binding(&self, file: FileIdx, specifier: &str, binding: &ImportBinding, ns: TsNs, guard: &mut Guard) -> Lookup {
    let target = self.ts_specifier(file, specifier)?;
    let Found::File(module) = target.found else { return Ok(target) };
    let carried = target.heuristic;
    match binding {
      ImportBinding::Named { imported, .. } => self.ts_export(module, imported, ns, guard).map(|hit| hit.weaken(carried)),
      ImportBinding::Default { .. } => self.ts_export(module, "default", ns, guard).map(|hit| hit.weaken(carried)),
      ImportBinding::Namespace { .. } => Ok(target),
    }
  }

  pub(crate) fn ts_specifier(&self, file: FileIdx, specifier: &str) -> Lookup {
    let Some(surface) = self.table.file(file) else { return Err(Miss::NotFound) };
    let repo = &surface.repo;
    if specifier.starts_with("./") || specifier.starts_with("../") || specifier == "." || specifier == ".." {
      let base = surface.path.resolve_relative(specifier).map_err(|_| Miss::NotFound)?;
      return self.ts_probe(repo, base.as_str()).map(|found| Hit::exact(Found::File(found))).ok_or(Miss::NotFound);
    }
    if let Some(config) = self.table.layout.tsconfig_for_file(repo, &surface.path) {
      if let Some(found) = self.ts_paths(config, repo, specifier) {
        return Ok(Hit::exact(Found::File(found)));
      }
      if let Some(found) = config
        .base_url
        .as_ref()
        .and_then(|base| base.file(specifier))
        .and_then(|base| self.ts_probe(repo, base.as_str()))
      {
        return Ok(Hit::exact(Found::File(found)));
      }
    }
    let (package, subpath) = split_package(specifier);
    if package.starts_with("node:") || NODE_BUILTINS.contains(&package) {
      return Ok(Hit::exact(Found::External(package.to_owned())));
    }
    match self.table.layout.npm_package(repo, package) {
      Some(key) => self.ts_package_entry(&key, subpath),
      None => Ok(Hit::exact(Found::External(package.to_owned()))),
    }
  }

  fn ts_exists(&self, repo: &RepoName, path: &str) -> Option<FileIdx> {
    let path = RelPath::new(path).ok()?;
    self.table.file_index(repo, &path)
  }

  pub(crate) fn ts_probe(&self, repo: &RepoName, base: &str) -> Option<FileIdx> {
    if has_source_extension(base) {
      let rewrites: &[(&str, &[&str])] = &[
        (".js", &[".ts", ".tsx", ".d.ts"]),
        (".jsx", &[".tsx", ".d.ts"]),
        (".mjs", &[".mts", ".d.mts"]),
        (".cjs", &[".cts", ".d.cts"]),
      ];
      for (from, targets) in rewrites {
        if let Some(stem) = base.strip_suffix(from)
          && let Some(found) = targets.iter().find_map(|target| self.ts_exists(repo, &format!("{stem}{target}")))
        {
          return Some(found);
        }
      }
      if let Some(found) = self.ts_exists(repo, base) {
        return Some(found);
      }
    }
    PROBE_EXTENSIONS
      .iter()
      .find_map(|extension| self.ts_exists(repo, &format!("{base}{extension}")))
      .or_else(|| PROBE_EXTENSIONS.iter().find_map(|extension| self.ts_exists(repo, &format!("{base}/index{extension}"))))
  }

  fn ts_paths(&self, config: &TsConfigInfo, repo: &RepoName, specifier: &str) -> Option<FileIdx> {
    let (mapping, captured) = config
      .paths
      .iter()
      .filter_map(|mapping| {
        let captured = match_subpath(&mapping.pattern, specifier)?;
        let score = match mapping.pattern.split_once('*') {
          None => usize::MAX,
          Some((prefix, _)) => prefix.len(),
        };
        Some((score, mapping, captured))
      })
      .max_by_key(|(score, _, _)| *score)
      .map(|(_, mapping, captured)| (mapping, captured))?;
    let base = config.paths_base.as_ref().or(config.base_url.as_ref()).unwrap_or(&config.dir);
    mapping.targets.iter().find_map(|target| {
      let substituted = target.replacen('*', &captured, 1);
      let path = base.file(&substituted)?;
      self.ts_probe(repo, path.as_str())
    })
  }

  fn ts_package_entry(&self, key: &PackageKey, subpath: &str) -> Lookup {
    let (Some(package), Some(npm)) = (self.table.layout.package(key), self.table.layout.npm(key)) else {
      return Err(Miss::NotFound);
    };
    let mut declared: Vec<String> = Vec::new();
    match &npm.exports {
      Some(NpmExports::Root(target)) if subpath.is_empty() => export_paths(target, &mut declared),
      Some(NpmExports::Root(_)) => return Err(Miss::NotFound),
      Some(NpmExports::Subpaths(entries)) => {
        let wanted = if subpath.is_empty() { ".".to_owned() } else { format!("./{subpath}") };
        let best = entries
          .iter()
          .filter_map(|(pattern, target)| match_subpath(pattern, &wanted).map(|captured| (pattern.split('*').next().map_or(0, str::len), target, captured)))
          .max_by_key(|(score, _, _)| *score);
        let Some((_, target, captured)) = best else { return Err(Miss::NotFound) };
        let mut paths = Vec::new();
        export_paths(target, &mut paths);
        declared.extend(paths.into_iter().map(|path| path.replacen('*', &captured, 1)));
      }
      None if subpath.is_empty() => {
        declared.extend([&npm.types, &npm.module, &npm.main].into_iter().flatten().cloned());
        declared.push("index".to_owned());
      }
      None => declared.push(subpath.to_owned()),
    }
    let candidates: Vec<RelPath> = declared.iter().filter_map(|path| package.root.file(path)).collect();
    for candidate in &candidates {
      if let Some(found) = self
        .ts_exists(&key.repo, candidate.as_str())
        .filter(|found| self.table.file(*found).is_some_and(|file| !is_declaration(&file.path)))
      {
        return Ok(Hit::exact(Found::File(found)));
      }
      if !has_source_extension(candidate.as_str())
        && let Some(found) = self.ts_probe(&key.repo, candidate.as_str())
      {
        return Ok(Hit::exact(Found::File(found)));
      }
    }
    for candidate in &candidates {
      if let Some(found) = self.ts_dist_to_src(key, package.root.as_rel_path(), candidate) {
        return Ok(Hit::heuristic(Found::File(found), DIST_TO_SRC_CONFIDENCE, RULE_DIST_TO_SRC));
      }
    }
    Err(Miss::NotFound)
  }

  fn ts_dist_to_src(&self, key: &PackageKey, root: Option<&RelPath>, candidate: &RelPath) -> Option<FileIdx> {
    let within = match root {
      Some(root) => candidate.strip_dir_prefix(root)?,
      None => candidate.as_str(),
    };
    let (first, rest) = within.split_once('/')?;
    if !BUILD_DIRS.contains(&first) {
      return None;
    }
    let stem = BUILD_EXTENSIONS.iter().find_map(|extension| rest.strip_suffix(extension)).unwrap_or(rest);
    let source = match root {
      Some(root) => format!("{}/src/{stem}", root.as_str()),
      None => format!("src/{stem}"),
    };
    self.ts_probe(&key.repo, &source)
  }

  pub(crate) fn ts_export(&self, file: FileIdx, name: &str, ns: TsNs, guard: &mut Guard) -> Lookup {
    self.note(name);
    let key = format!("ts:{}:{name}:{}", file.0, ns.tag());
    let memoize = guard.is_top_level();
    if memoize && let Some(cached) = self.module_lookups.borrow().get(&key) {
      return cached.clone();
    }
    if !guard.enter(key.clone()) {
      return Err(Miss::NotFound);
    }
    let result = match self.ts_export_direct(file, name, ns, guard) {
      Some(result) => result,
      None if name == "default" => Err(Miss::NotFound),
      None => self.ts_star_exports(file, name, ns, guard),
    };
    guard.exit();
    if memoize {
      self.module_lookups.borrow_mut().insert(key, result.clone());
    }
    result
  }

  pub(crate) fn ts_export_both(&self, file: FileIdx, name: &str) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for ns in [TsNs::Value, TsNs::Type] {
      if let Ok(hit) = self.ts_export(file, name, ns, &mut Guard::default())
        && !hits.iter().any(|existing| existing.found == hit.found)
      {
        hits.push(hit);
      }
    }
    hits
  }

  fn ts_export_direct(&self, file: FileIdx, name: &str, ns: TsNs, guard: &mut Guard) -> Option<Lookup> {
    let surface = self.table.file(file)?;
    let declared: Vec<u32> = surface
      .symbols
      .iter()
      .enumerate()
      .filter(|(_, symbol)| {
        symbol.parent.is_none()
          && ns.admits(&symbol.kind)
          && match name {
            "default" => symbol.visibility == Visibility::ExportedDefault,
            _ => symbol.visibility == Visibility::Exported && symbol.name == name,
          }
      })
      .map(|(local, _)| local as u32)
      .collect();
    if let Some(symbol) = self.ts_pick(file, declared) {
      return Some(Ok(Hit::exact(Found::Symbol(symbol))));
    }
    for export in &surface.ts.exports {
      let result = match export {
        ExportFact::Named { local, exported, .. } if exported == name => self.ts_lookup_name(file, None, local, ns, guard),
        ExportFact::Default {
          target: DefaultExportTarget::Identifier(local),
          ..
        } if name == "default" => self.ts_lookup_name(file, None, local, ns, guard),
        ExportFact::ReExport { specifier, bindings, .. } => {
          let Some(binding) = bindings.iter().find(|binding| binding.exported == name) else {
            continue;
          };
          match self.ts_specifier(file, specifier) {
            Ok(Hit {
              found: Found::File(module),
              heuristic,
            }) => self.ts_export(module, &binding.imported, ns, guard).map(|hit| hit.weaken(heuristic)),
            other => other,
          }
        }
        ExportFact::ReExportAll {
          specifier,
          namespace: Some(namespace),
          ..
        } if namespace == name => self.ts_specifier(file, specifier),
        ExportFact::CommonJs {
          exported: Some(exported),
          value: ArgSummary::Path(path),
          ..
        } if exported == name && path.len() == 1 && ns == TsNs::Value => self.ts_lookup_name(file, None, &path[0], ns, guard),
        ExportFact::CommonJs {
          exported: None,
          value: ArgSummary::Path(path),
          ..
        } if name == "default" && path.len() == 1 && ns == TsNs::Value => self.ts_lookup_name(file, None, &path[0], ns, guard),
        ExportFact::CommonJs {
          exported: None,
          value: ArgSummary::Object(entries),
          ..
        } if ns == TsNs::Value => {
          let entry = entries.iter().find(|entry| entry.key.static_name() == Some(name));
          match entry.map(|entry| &entry.value) {
            Some(ArgSummary::Path(path)) if path.len() == 1 => self.ts_lookup_name(file, None, &path[0], ns, guard),
            _ => continue,
          }
        }
        _ => continue,
      };
      match result {
        Err(Miss::NotFound) => continue,
        other => return Some(other),
      }
    }
    None
  }

  fn ts_star_exports(&self, file: FileIdx, name: &str, ns: TsNs, guard: &mut Guard) -> Lookup {
    let mut hits = Vec::new();
    let mut external = None;
    let mut visited: HashSet<FileIdx> = HashSet::from([file]);
    let mut queue: VecDeque<(FileIdx, Option<Heuristic>)> = VecDeque::from([(file, None)]);
    while let Some((current, carried)) = queue.pop_front() {
      let Some(surface) = self.table.file(current) else { continue };
      for export in &surface.ts.exports {
        let ExportFact::ReExportAll { specifier, namespace: None, .. } = export else {
          continue;
        };
        match self.ts_specifier(current, specifier) {
          Ok(Hit {
            found: Found::File(module),
            heuristic,
          }) => {
            if !visited.insert(module) {
              continue;
            }
            let carried = weakest(carried, heuristic);
            match self.ts_export_direct(module, name, ns, guard) {
              Some(Ok(hit)) => hits.push(hit.weaken(carried)),
              Some(Err(Miss::Ambiguous(ambiguous))) => hits.extend(ambiguous),
              Some(Err(_)) => {}
              None => queue.push_back((module, carried)),
            }
          }
          Ok(Hit {
            found: Found::External(package), ..
          }) => {
            external.get_or_insert(package);
          }
          _ => {}
        }
      }
    }
    match (hits.is_empty(), external) {
      (false, _) => unique(hits),
      (true, Some(package)) => Ok(Hit::exact(Found::External(package))),
      (true, None) => Err(Miss::NotFound),
    }
  }

  fn ts_member_call(&self, from: Option<u32>, reference: &ReferenceFact) -> Outcome {
    let path = &reference.path;
    let Some((member, chain)) = path.split_last() else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    let Some(head) = chain.first() else {
      return Outcome::Unresolved(Miss::NotFound);
    };
    let mut head_kind = ReceiverHead::Other;
    let receiver = match &reference.receiver {
      Some(ReceiverHint::This) if head == "this" => match self.ts_this(from) {
        Some(receiver) => {
          if chain.len() == 1 {
            head_kind = ReceiverHead::This;
          }
          receiver
        }
        None => return Outcome::Unresolved(Miss::Unsupported("this outside a class")),
      },
      Some(ReceiverHint::Expr(expression)) if head == "this" && expression == "this" && self.ts_this(from).is_some() => {
        return Outcome::Unresolved(Miss::Unsupported("rebound this"));
      }
      Some(ReceiverHint::Expr(expression)) if head == "super" && expression == "super" => match self.ts_this(from) {
        Some(receiver) => {
          head_kind = ReceiverHead::Super;
          self.ts_base_recv(receiver)
        }
        None => return Outcome::Unresolved(Miss::Unsupported("super outside a class")),
      },
      Some(ReceiverHint::TypeHint(hint)) => self.ts_type_recv(self.file, from, hint),
      Some(ReceiverHint::Identifier(name)) if name == head => match self.ts_lookup_name(self.file, from, head, TsNs::Value, &mut Guard::default()) {
        Ok(hit) => self.ts_recv_of_found(hit),
        Err(miss) => return Outcome::Unresolved(miss),
      },
      None => match self.ts_lookup_name(self.file, from, head, TsNs::Value, &mut Guard::default()) {
        Ok(hit) => self.ts_recv_of_found(hit),
        Err(miss) => return Outcome::Unresolved(miss),
      },
      _ => Recv::unknown(),
    };
    let mut receiver = receiver;
    for segment in &chain[1..] {
      receiver = self.ts_member_type(receiver, segment);
    }
    self.ts_method_on(receiver, member, from, head_kind)
  }

  fn ts_this(&self, from: Option<u32>) -> Option<Recv> {
    let from = from?;
    let mut below_class: Option<u32> = None;
    for ancestor in self.surface.ancestors(from) {
      let symbol = self.surface.symbol(ancestor)?;
      if symbol.kind == EntityKind::Class {
        let class = SymRef { file: self.file, local: ancestor };
        let static_context = match below_class {
          None => true,
          Some(member) => self.surface.symbol(member).is_some_and(|member| member.is_static()),
        };
        let ty = if static_context { RecvType::Static(class) } else { RecvType::Concrete(class) };
        return Some(Recv::of(ty, None));
      }
      below_class = Some(ancestor);
    }
    None
  }

  fn ts_base_recv(&self, receiver: Recv) -> Recv {
    let (class, static_side) = match receiver.ty {
      RecvType::Concrete(class) => (class, false),
      RecvType::Static(class) => (class, true),
      _ => return Recv::unknown(),
    };
    let Some(base) = self.table.sym(class).and_then(|symbol| symbol.extends.first().cloned()) else {
      return Recv::unknown();
    };
    let base = self.ts_type_recv(class.file, None, &base);
    match (base.ty, static_side) {
      (RecvType::Concrete(parent), true) => Recv::of(RecvType::Static(parent), base.heuristic),
      (ty, _) => Recv::of(ty, base.heuristic),
    }
  }

  fn ts_type_recv(&self, file: FileIdx, from: Option<u32>, path: &[String]) -> Recv {
    if let [single] = path
      && self.ts_is_type_param(file, from, single)
    {
      return Recv::unknown();
    }
    match self.ts_static_path(file, from, path, TsNs::Type, &mut Guard::default()) {
      Ok(hit) => {
        let heuristic = hit.heuristic;
        let ty = match hit.found {
          Found::Symbol(symbol) => match self.sym_kind(symbol) {
            Some(EntityKind::Class) => RecvType::Concrete(symbol),
            Some(EntityKind::Interface) => RecvType::Traits {
              traits: vec![symbol],
              external: None,
            },
            Some(EntityKind::Enum | EntityKind::Namespace) => RecvType::Static(symbol),
            _ => RecvType::Unknown,
          },
          Found::External(package) => RecvType::External(package),
          _ => RecvType::Unknown,
        };
        Recv::of(ty, heuristic)
      }
      Err(_) => Recv::unknown(),
    }
  }

  fn ts_recv_of_found(&self, hit: Hit) -> Recv {
    let heuristic = hit.heuristic;
    let receiver = match hit.found {
      Found::Symbol(symbol) => match self.sym_kind(symbol) {
        Some(EntityKind::Class | EntityKind::Enum | EntityKind::Namespace) => Recv::of(RecvType::Static(symbol), None),
        Some(EntityKind::Constant | EntityKind::Variable | EntityKind::Field) => self.ts_value_recv(symbol),
        _ => Recv::unknown(),
      },
      Found::File(module) => Recv::of(RecvType::Namespace(module), None),
      Found::External(package) => Recv::of(RecvType::External(package), None),
      Found::Module(_) => Recv::unknown(),
    };
    let weakened = weakest(receiver.heuristic, heuristic);
    Recv::of(receiver.ty, weakened)
  }

  fn ts_value_recv(&self, holder: SymRef) -> Recv {
    let Some(surface) = self.table.sym(holder) else { return Recv::unknown() };
    let scope = Some(holder.local);
    match &surface.value_type {
      Some(ValueType::Declared(text)) => match ts_type_head(text) {
        Some(head) => self.ts_type_recv(holder.file, scope, &head),
        None => Recv::unknown(),
      },
      Some(ValueType::Constructed(path)) => match self.ts_static_path(holder.file, surface.parent, path, TsNs::Value, &mut Guard::default()) {
        Ok(Hit {
          found: Found::Symbol(class),
          heuristic,
        }) if self.sym_kind(class) == Some(&EntityKind::Class) => Recv::of(RecvType::Concrete(class), heuristic),
        Ok(Hit {
          found: Found::External(package), ..
        }) => Recv::of(RecvType::External(package), None),
        _ => Recv::unknown(),
      },
      None if surface.is_getter() => match surface.return_type.as_deref().and_then(ts_type_head) {
        Some(head) => self.ts_type_recv(holder.file, scope, &head),
        None => Recv::unknown(),
      },
      None => Recv::unknown(),
    }
  }

  fn ts_member_of_recv(&self, receiver: &Recv, name: &str) -> Option<Result<(SymRef, Option<Heuristic>), String>> {
    let found = |member: Member| match member {
      Member::Found(symbol, heuristic) => Some(Ok((symbol, heuristic))),
      Member::External(package) => Some(Err(package)),
      Member::Missing => None,
    };
    match &receiver.ty {
      RecvType::Concrete(owner) => found(self.ts_member_lookup(*owner, name, Side::Instance)),
      RecvType::Static(owner) => {
        let side = if self.sym_kind(*owner) == Some(&EntityKind::Class) { Side::Static } else { Side::Any };
        found(self.ts_member_lookup(*owner, name, side))
      }
      RecvType::Traits { traits, .. } => traits.iter().find_map(|owner| found(self.ts_member_lookup(*owner, name, Side::Any))),
      RecvType::External(package) => Some(Err(package.clone())),
      RecvType::Namespace(_) | RecvType::Unknown => None,
    }
  }

  fn ts_member_type(&self, receiver: Recv, segment: &str) -> Recv {
    let carried = receiver.heuristic;
    let next = match &receiver.ty {
      RecvType::Namespace(module) => match self.ts_export(*module, segment, TsNs::Value, &mut Guard::default()) {
        Ok(hit) => self.ts_recv_of_found(hit),
        Err(_) => Recv::unknown(),
      },
      RecvType::External(package) => Recv::of(RecvType::External(package.clone()), None),
      RecvType::Unknown => Recv::unknown(),
      _ => match self.ts_member_of_recv(&receiver, segment) {
        Some(Ok((member, heuristic))) => {
          let next = match self.sym_kind(member) {
            Some(EntityKind::Class | EntityKind::Enum | EntityKind::Namespace) => Recv::of(RecvType::Static(member), None),
            _ => self.ts_value_recv(member),
          };
          let weakened = weakest(next.heuristic, heuristic);
          Recv::of(next.ty, weakened)
        }
        Some(Err(package)) => Recv::of(RecvType::External(package), None),
        None => Recv::unknown(),
      },
    };
    let weakened = weakest(next.heuristic, carried);
    Recv::of(next.ty, weakened)
  }

  fn ts_method_on(&self, receiver: Recv, member: &str, from: Option<u32>, head: ReceiverHead) -> Outcome {
    let carried = receiver.heuristic;
    let caller = from.map(|local| SymRef { file: self.file, local });
    match &receiver.ty {
      RecvType::Namespace(module) => Outcome::from_lookup(self.ts_export(*module, member, TsNs::Value, &mut Guard::default()).map(|hit| hit.weaken(carried))),
      RecvType::External(package) => Outcome::External(package.clone()),
      RecvType::Unknown => self.method_name_candidates(member, caller),
      RecvType::Concrete(class) | RecvType::Static(class) => match self.ts_member_of_recv(&receiver, member) {
        Some(Ok((found, heuristic))) => {
          let extra = match (&receiver.ty, head) {
            (RecvType::Concrete(_), ReceiverHead::This) => self.ts_override_candidates(*class, found, member, &self.dispatch_scope(DispatchReach::Implementers, caller)),
            (RecvType::Concrete(_), ReceiverHead::Other) => self.ts_override_candidates(*class, found, member, &self.dispatch_scope(DispatchReach::Linked, caller)),
            _ => Vec::new(),
          };
          Outcome::Targets {
            primary: Hit::exact(Found::Symbol(found)).weaken(heuristic).weaken(carried),
            extra,
          }
        }
        Some(Err(package)) => Outcome::External(package),
        None => Outcome::Unresolved(Miss::NotFound),
      },
      RecvType::Traits { traits, .. } => {
        let hits: Vec<(SymRef, SymRef, Option<Heuristic>)> = traits
          .iter()
          .filter_map(|owner| match self.ts_member_lookup(*owner, member, Side::Any) {
            Member::Found(found, heuristic) => Some((*owner, found, heuristic)),
            _ => None,
          })
          .collect();
        match hits.as_slice() {
          [] => Outcome::Unresolved(Miss::NotFound),
          [(interface, found, heuristic), ..] => Outcome::Targets {
            primary: Hit::exact(Found::Symbol(*found)).weaken(*heuristic).weaken(carried),
            extra: self.ts_dispatch_candidates(*interface, member, &self.dispatch_scope(DispatchReach::Linked, caller)),
          },
        }
      }
    }
  }

  fn ts_own_member(&self, owner: SymRef, name: &str, side: Side) -> Option<SymRef> {
    self.note(name);
    let file = self.table.file(owner.file)?;
    let filtered = self.sym_kind(owner) == Some(&EntityKind::Class) && side != Side::Any;
    file
      .children(owner.local)
      .iter()
      .find(|child| {
        file.symbol(**child).is_some_and(|symbol| {
          symbol.name == name
            && match (filtered, side) {
              (false, _) | (_, Side::Any) => true,
              (true, Side::Static) => symbol.is_static(),
              (true, Side::Instance) => !symbol.is_static(),
            }
        })
      })
      .map(|child| SymRef { file: owner.file, local: *child })
  }

  fn ts_member_lookup(&self, owner: SymRef, name: &str, side: Side) -> Member {
    let mut visited: HashSet<SymRef> = HashSet::new();
    let mut pending: Vec<(SymRef, Option<Heuristic>)> = vec![(owner, None)];
    let mut external = None;
    while let Some((current, carried)) = pending.pop() {
      if !visited.insert(current) || visited.len() > MAX_HERITAGE_DEPTH {
        continue;
      }
      if let Some(surface) = self.table.sym(current) {
        self.note(&surface.name);
      }
      if let Some(member) = self.ts_own_member(current, name, side) {
        return Member::Found(member, carried);
      }
      let Some(surface) = self.table.sym(current) else { continue };
      for base in surface.extends.iter().rev() {
        match self.ts_resolve_heritage(current, base) {
          Ok(Hit {
            found: Found::Symbol(parent),
            heuristic,
          }) => pending.push((parent, weakest(carried, heuristic))),
          Ok(Hit {
            found: Found::External(package), ..
          }) => {
            external.get_or_insert(package);
          }
          _ => {}
        }
      }
    }
    match external {
      Some(package) => Member::External(package),
      None => Member::Missing,
    }
  }

  fn ts_override_candidates(&self, class: SymRef, declared: SymRef, member: &str, scope: &DispatchScope) -> Vec<Hit> {
    if self.table.sym(declared).is_none_or(|symbol| symbol.is_static()) {
      return Vec::new();
    }
    let mut candidates: Vec<SymRef> = Vec::new();
    let mut visited: HashSet<SymRef> = HashSet::from([class]);
    let mut pending = vec![class];
    while let Some(base) = pending.pop() {
      let Some(base_name) = self.table.sym(base).map(|symbol| symbol.name.clone()) else {
        continue;
      };
      self.note(&base_name);
      for subclass in self.table.ts_subclasses.get(&base_name).map(Vec::as_slice).unwrap_or_default() {
        if visited.len() > MAX_DISPATCH_CANDIDATES || visited.contains(subclass) {
          continue;
        }
        let extends_base = self
          .table
          .sym(*subclass)
          .and_then(|symbol| symbol.extends.first().cloned())
          .is_some_and(|path| self.ts_resolve_heritage(*subclass, &path).map(|hit| hit.found) == Ok(Found::Symbol(base)));
        if !extends_base {
          continue;
        }
        visited.insert(*subclass);
        pending.push(*subclass);
        if !self.may_dispatch_into(scope, subclass.file) || !self.may_dispatch_to(scope, *subclass) {
          continue;
        }
        if let Some(own) = self.ts_own_member(*subclass, member, Side::Instance)
          && own != declared
        {
          candidates.push(own);
        }
      }
    }
    candidates.sort_by_key(|candidate| self.table.symbol_entity(*candidate));
    candidates.dedup();
    candidates.truncate(MAX_DISPATCH_CANDIDATES);
    candidates
      .into_iter()
      .map(|candidate| Hit::heuristic(Found::Symbol(candidate), DISPATCH_CONFIDENCE, RULE_OVERRIDE_DISPATCH))
      .collect()
  }

  fn ts_dispatch_candidates(&self, interface: SymRef, member: &str, scope: &DispatchScope) -> Vec<Hit> {
    let Some(name) = self.table.sym(interface).map(|symbol| symbol.name.clone()) else {
      return Vec::new();
    };
    self.note(&name);
    let mut candidates: Vec<SymRef> = Vec::new();
    for class in self.table.ts_implementers.get(&name).map(Vec::as_slice).unwrap_or_default() {
      if !self.may_dispatch_into(scope, class.file) || !self.may_dispatch_to(scope, *class) {
        continue;
      }
      let Some(surface) = self.table.sym(*class) else { continue };
      let implements_it = surface
        .implements
        .iter()
        .any(|path| self.ts_resolve_heritage(*class, path).map(|hit| hit.found) == Ok(Found::Symbol(interface)));
      if !implements_it {
        continue;
      }
      if let Member::Found(found, _) = self.ts_member_lookup(*class, member, Side::Instance)
        && found != interface
      {
        candidates.push(found);
      }
    }
    candidates.sort_by_key(|candidate| self.table.symbol_entity(*candidate));
    candidates.dedup();
    candidates.truncate(MAX_DISPATCH_CANDIDATES);
    candidates
      .into_iter()
      .map(|candidate| Hit::heuristic(Found::Symbol(candidate), DISPATCH_CONFIDENCE, RULE_INTERFACE_DISPATCH))
      .collect()
  }

  pub(crate) fn ts_resolve_heritage(&self, class: SymRef, path: &[String]) -> Lookup {
    self.ts_static_path(class.file, None, path, TsNs::Type, &mut Guard::default())
  }

  pub(crate) fn ts_heritage_chains(&self, class: SymRef) -> Vec<Vec<(SymRef, Option<Heuristic>)>> {
    let Some(owner) = self.table.sym(class) else { return Vec::new() };
    owner
      .extends
      .iter()
      .chain(&owner.implements)
      .filter_map(|path| match self.ts_resolve_heritage(class, path) {
        Ok(Hit {
          found: Found::Symbol(target),
          heuristic,
        }) => Some(self.ts_ancestors(target, heuristic)),
        _ => None,
      })
      .collect()
  }

  fn ts_ancestors(&self, start: SymRef, heuristic: Option<Heuristic>) -> Vec<(SymRef, Option<Heuristic>)> {
    let mut visited: HashSet<SymRef> = HashSet::new();
    let mut ancestors = Vec::new();
    let mut pending: VecDeque<(SymRef, Option<Heuristic>)> = VecDeque::from([(start, heuristic)]);
    while let Some((current, carried)) = pending.pop_front() {
      if !visited.insert(current) || visited.len() > MAX_HERITAGE_DEPTH {
        continue;
      }
      let Some(surface) = self.table.sym(current) else { continue };
      self.note(&surface.name);
      if !matches!(surface.kind, EntityKind::Class | EntityKind::Interface) {
        continue;
      }
      ancestors.push((current, carried));
      for base in &surface.extends {
        if let Ok(Hit {
          found: Found::Symbol(parent),
          heuristic,
        }) = self.ts_resolve_heritage(current, base)
        {
          pending.push_back((parent, weakest(carried, heuristic)));
        }
      }
    }
    ancestors
  }

  pub(crate) fn ts_overridden_members(&self, chain: &[(SymRef, Option<Heuristic>)], method: SymRef) -> Vec<(Hit, Override)> {
    let Some(source) = self.table.sym(method) else { return Vec::new() };
    if source.name == "constructor" || source.visibility == Visibility::Private {
      return Vec::new();
    }
    let side = if source.is_static() { Side::Static } else { Side::Instance };
    let accessor = Accessor::of(source);
    for (owner, carried) in chain {
      let kind = self.sym_kind(*owner);
      if side == Side::Static && kind != Some(&EntityKind::Class) {
        continue;
      }
      let members = self.ts_override_targets(*owner, &source.name, side, accessor);
      if members.is_empty() {
        continue;
      }
      let private = members
        .iter()
        .any(|member| self.table.sym(*member).is_none_or(|symbol| symbol.visibility == Visibility::Private));
      if private || members.contains(&method) {
        return Vec::new();
      }
      let via = match kind {
        Some(EntityKind::Interface) => Override::Interface,
        _ => Override::Class,
      };
      return members.into_iter().map(|member| (Hit::exact(Found::Symbol(member)).weaken(*carried), via)).collect();
    }
    Vec::new()
  }

  fn ts_override_targets(&self, owner: SymRef, name: &str, side: Side, accessor: Accessor) -> Vec<SymRef> {
    self.note(name);
    let Some(file) = self.table.file(owner.file) else { return Vec::new() };
    let filtered = self.sym_kind(owner) == Some(&EntityKind::Class);
    let members: Vec<(u32, Accessor)> = file
      .children(owner.local)
      .iter()
      .filter_map(|child| {
        let symbol = file.symbol(*child)?;
        let admitted = symbol.name == name && matches!(symbol.kind, EntityKind::Method | EntityKind::Field) && (!filtered || symbol.is_static() == (side == Side::Static));
        admitted.then_some((*child, Accessor::of(symbol)))
      })
      .collect();
    let same_accessor = members.iter().any(|(_, kind)| *kind == accessor);
    members
      .into_iter()
      .filter(|(_, kind)| !same_accessor || *kind == accessor)
      .map(|(local, _)| SymRef { file: owner.file, local })
      .collect()
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Override {
  Interface,
  Class,
}

impl Override {
  pub(crate) fn as_str(self) -> &'static str {
    match self {
      Self::Interface => "interface",
      Self::Class => "class",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accessor {
  Getter,
  Setter,
  Plain,
}

impl Accessor {
  fn of(symbol: &SymSurface) -> Self {
    match (symbol.is_getter(), symbol.modifiers.contains(Modifier::Setter)) {
      (true, _) => Self::Getter,
      (_, true) => Self::Setter,
      _ => Self::Plain,
    }
  }
}

fn is_declaration(path: &RelPath) -> bool {
  atlas_core::Language::is_declaration_file(path.as_str())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_split_scoped_and_plain_package_specifiers() {
    assert_eq!(split_package("@acme/sdk"), ("@acme/sdk", ""));
    assert_eq!(split_package("@acme/sdk/money"), ("@acme/sdk", "money"));
    assert_eq!(split_package("react-dom/client"), ("react-dom", "client"));
    assert_eq!(split_package("react"), ("react", ""));
  }

  #[test]
  fn should_match_export_subpath_patterns() {
    assert_eq!(match_subpath("./*", "./utils/x").as_deref(), Some("utils/x"));
    assert_eq!(match_subpath("./money", "./money").as_deref(), Some(""));
    assert_eq!(match_subpath("./money", "./other"), None);
    assert_eq!(match_subpath("@app/*", "@app/users/service").as_deref(), Some("users/service"));
  }

  #[test]
  fn should_order_export_conditions_by_source_preference() {
    let target = NpmExportTarget::Conditions(vec![
      ("require".into(), NpmExportTarget::Path("./dist/index.cjs".into())),
      ("types".into(), NpmExportTarget::Path("./dist/index.d.ts".into())),
      ("import".into(), NpmExportTarget::Path("./dist/index.js".into())),
    ]);
    let mut paths = Vec::new();
    export_paths(&target, &mut paths);
    assert_eq!(paths, ["./dist/index.d.ts", "./dist/index.js", "./dist/index.cjs"]);
  }

  #[test]
  fn should_display_dynamic_specifiers_by_their_variable() {
    assert_eq!(specifier_display(&ArgSummary::Path(vec!["modulePath".into()])), "modulePath");
    let template = ArgSummary::Template(vec![
      atlas_core::TemplatePart::Lit("./plugins/".into()),
      atlas_core::TemplatePart::Expr(ArgSummary::Path(vec!["name".into()])),
    ]);
    assert_eq!(specifier_display(&template), "name");
  }
}
