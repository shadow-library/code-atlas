use std::collections::{BTreeSet, HashMap};
use std::mem;
use std::sync::{Mutex, MutexGuard};

use atlas_core::{EntityId, EntityKey, EntityKind, FileFacts, KeyContext, LanguageFamily, RelPath, RepoName, SymbolKey};
use atlas_project::PackageGraph;
use tracing::debug;

use crate::error::ResolveError;
use crate::layout::{Layout, PackageKey};
use crate::modtree::place_package;
use crate::surface::FileSurface;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FileIdx(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SymRef {
  pub file: FileIdx,
  pub local: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ModuleId {
  pub repo: RepoName,
  pub package: String,
  pub path: Vec<String>,
}

impl ModuleId {
  pub fn parent(&self) -> Option<ModuleId> {
    let (_, parent) = self.path.split_last()?;
    Some(ModuleId {
      repo: self.repo.clone(),
      package: self.package.clone(),
      path: parent.to_vec(),
    })
  }

  pub fn child(&self, name: &str) -> ModuleId {
    let mut path = self.path.clone();
    path.push(name.to_owned());
    ModuleId {
      repo: self.repo.clone(),
      package: self.package.clone(),
      path,
    }
  }

  pub fn root(&self) -> ModuleId {
    ModuleId {
      repo: self.repo.clone(),
      package: self.package.clone(),
      path: Vec::new(),
    }
  }

  pub fn same_crate(&self, other: &ModuleId) -> bool {
    self.repo == other.repo && self.package == other.package
  }

  pub fn is_within(&self, ancestor: &ModuleId) -> bool {
    self.same_crate(ancestor) && self.path.starts_with(&ancestor.path)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleLoc {
  pub file: FileIdx,
  pub suffix: Vec<String>,
  pub weak: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleEntry {
  pub locs: Vec<ModuleLoc>,
  pub decl: Option<SymRef>,
}

impl ModuleEntry {
  pub fn effective_locs(&self) -> impl Iterator<Item = &ModuleLoc> {
    let has_strong = self.locs.iter().any(|location| !location.weak);
    self.locs.iter().filter(move |location| !has_strong || !location.weak)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reachability {
  TargetRoot,
  Reachable,
  CfgGated { cfg: String },
  Unreachable,
  Unpackaged,
  NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlacement {
  pub context: KeyContext,
  pub reachability: Reachability,
  pub also_in: Vec<KeyContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceChange {
  Added,
  Removed,
  Changed,
  Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDelta {
  pub repo: RepoName,
  pub path: RelPath,
  pub change: SurfaceChange,
  pub names: BTreeSet<String>,
  pub broad: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LookupSource {
  File,
  Detector,
}

#[derive(Debug, Default)]
struct LookupIndex {
  by_file: HashMap<FileIdx, BTreeSet<String>>,
  by_detector: HashMap<FileIdx, BTreeSet<String>>,
  by_name: HashMap<String, BTreeSet<FileIdx>>,
}

impl LookupIndex {
  fn unlink(&mut self, index: FileIdx, names: impl IntoIterator<Item = String>) {
    for name in names {
      let still_used = self.by_file.get(&index).is_some_and(|set| set.contains(&name)) || self.by_detector.get(&index).is_some_and(|set| set.contains(&name));
      if still_used {
        continue;
      }
      if let Some(files) = self.by_name.get_mut(&name) {
        files.remove(&index);
        if files.is_empty() {
          self.by_name.remove(&name);
        }
      }
    }
  }

  fn forget(&mut self, index: FileIdx) {
    let names: Vec<String> = self.by_file.remove(&index).into_iter().chain(self.by_detector.remove(&index)).flatten().collect();
    self.unlink(index, names);
  }

  fn record(&mut self, index: FileIdx, names: BTreeSet<String>, source: LookupSource) {
    for name in &names {
      self.by_name.entry(name.clone()).or_default().insert(index);
    }
    match source {
      LookupSource::File => {
        let previous = self.by_file.insert(index, names).unwrap_or_default();
        self.unlink(index, previous);
      }
      LookupSource::Detector => self.by_detector.entry(index).or_default().extend(names),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
  pub repo: RepoName,
  pub path: RelPath,
  pub from: KeyContext,
  pub to: KeyContext,
}

#[derive(Debug, Default)]
pub struct ProjectSymbolTable {
  pub(crate) layout: Layout,
  files: Vec<Option<FileSurface>>,
  free: Vec<u32>,
  file_ids: HashMap<(RepoName, RelPath), FileIdx>,
  file_packages: HashMap<FileIdx, PackageKey>,
  package_files: HashMap<PackageKey, BTreeSet<FileIdx>>,
  placements: HashMap<FileIdx, FilePlacement>,
  pub(crate) modules: HashMap<ModuleId, ModuleEntry>,
  pub(crate) methods_by_name: HashMap<String, Vec<SymRef>>,
  pub(crate) macros_by_name: HashMap<String, Vec<SymRef>>,
  pub(crate) rust_impls_by_trait: HashMap<String, Vec<(FileIdx, u32)>>,
  pub(crate) rust_impls_by_self: HashMap<String, Vec<(FileIdx, u32)>>,
  pub(crate) ts_implementers: HashMap<String, Vec<SymRef>>,
  pub(crate) ts_subclasses: HashMap<String, Vec<SymRef>>,
  dirty_packages: BTreeSet<PackageKey>,
  dirty_repos: BTreeSet<RepoName>,
  lookups: Mutex<LookupIndex>,
  removed_packages: HashMap<(RepoName, RelPath), PackageKey>,
}

impl ProjectSymbolTable {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn set_repo_packages(&mut self, repo: RepoName, graph: PackageGraph) {
    self.layout.set_repo(repo.clone(), graph);
    self.dirty_repos.insert(repo);
  }

  pub fn remove_repo_packages(&mut self, repo: &RepoName) {
    if self.layout.remove_repo(repo) {
      self.dirty_repos.insert(repo.clone());
    }
  }

  pub fn is_fresh(&self) -> bool {
    self.dirty_packages.is_empty() && self.dirty_repos.is_empty()
  }

  pub fn file_count(&self) -> usize {
    self.file_ids.len()
  }

  pub fn contains_file(&self, repo: &RepoName, path: &RelPath) -> bool {
    self.file_ids.contains_key(&(repo.clone(), path.clone()))
  }

  pub fn upsert_file(&mut self, facts: &FileFacts, context: &KeyContext) -> Result<SurfaceDelta, ResolveError> {
    let surface = FileSurface::from_facts(facts, context)?;
    let key = (surface.repo.clone(), surface.path.clone());
    let signature = surface.signature();
    if let Some(existing) = self.file_ids.get(&key).copied() {
      let previous_signature = self.file(existing).map(FileSurface::signature).unwrap_or_default();
      let previous_context = self.file(existing).map(|file| file.context.clone());
      self.detach(existing);
      self.attach(existing, surface);
      let names = previous_signature.changed_names(&signature);
      let broad = previous_signature.structural != signature.structural || previous_context.as_ref() != Some(context);
      let change = if names.is_empty() && !broad { SurfaceChange::Unchanged } else { SurfaceChange::Changed };
      return Ok(SurfaceDelta {
        repo: key.0,
        path: key.1,
        change,
        names,
        broad,
      });
    }
    let index = match self.free.pop() {
      Some(slot) => FileIdx(slot),
      None => {
        self.files.push(None);
        FileIdx((self.files.len() - 1) as u32)
      }
    };
    self.file_ids.insert(key.clone(), index);
    self.attach(index, surface);
    Ok(SurfaceDelta {
      repo: key.0,
      path: key.1,
      change: SurfaceChange::Added,
      names: signature.names.into_keys().collect(),
      broad: true,
    })
  }

  pub fn remove_file(&mut self, repo: &RepoName, path: &RelPath) -> Option<SurfaceDelta> {
    let index = self.file_ids.remove(&(repo.clone(), path.clone()))?;
    let names: BTreeSet<String> = self.file(index).map(|file| file.signature().names.into_keys().collect()).unwrap_or_default();
    let package = self.file_packages.get(&index).cloned();
    self.detach(index);
    self.files[index.0 as usize] = None;
    self.free.push(index.0);
    self.lookup_index().forget(index);
    if let Some(package) = package {
      self.removed_packages.insert((repo.clone(), path.clone()), package);
    }
    Some(SurfaceDelta {
      repo: repo.clone(),
      path: path.clone(),
      change: SurfaceChange::Removed,
      names,
      broad: true,
    })
  }

  fn lookup_index(&self) -> MutexGuard<'_, LookupIndex> {
    match self.lookups.lock() {
      Ok(guard) => guard,
      Err(poisoned) => poisoned.into_inner(),
    }
  }

  pub(crate) fn record_lookups(&self, index: FileIdx, names: BTreeSet<String>, source: LookupSource) {
    self.lookup_index().record(index, names, source);
  }

  pub fn affected_files(&self, deltas: &[SurfaceDelta], relocations: &[Relocation]) -> BTreeSet<(RepoName, RelPath)> {
    let mut affected: BTreeSet<FileIdx> = BTreeSet::new();
    let mut broad_packages: BTreeSet<PackageKey> = BTreeSet::new();
    let mut broad_repos: BTreeSet<RepoName> = BTreeSet::new();
    {
      let lookups = self.lookup_index();
      for delta in deltas.iter().filter(|delta| delta.change != SurfaceChange::Unchanged) {
        for name in &delta.names {
          affected.extend(lookups.by_name.get(name).into_iter().flatten().copied());
        }
      }
    }
    let broad_sources = deltas
      .iter()
      .filter(|delta| delta.broad)
      .map(|delta| (delta.repo.clone(), delta.path.clone()))
      .chain(relocations.iter().map(|relocation| (relocation.repo.clone(), relocation.path.clone())));
    for (repo, path) in broad_sources {
      let package = self
        .file_index(&repo, &path)
        .and_then(|index| self.file_packages.get(&index).cloned())
        .or_else(|| self.removed_packages.get(&(repo.clone(), path.clone())).cloned())
        .or_else(|| self.layout.package_for_file(&repo, &path));
      match package {
        Some(package) => {
          broad_packages.insert(package);
        }
        None => {
          broad_repos.insert(repo);
        }
      }
    }
    if !broad_packages.is_empty() {
      let dependents: Vec<PackageKey> = self
        .package_files
        .keys()
        .filter(|candidate| self.layout.dependency_packages(candidate).iter().any(|dependency| broad_packages.contains(dependency)))
        .cloned()
        .collect();
      broad_packages.extend(dependents);
    }
    for package in &broad_packages {
      affected.extend(self.package_files.get(package).into_iter().flatten().copied());
    }
    for repo in &broad_repos {
      affected.extend(self.file_ids.iter().filter(|((file_repo, _), _)| file_repo == repo).map(|(_, index)| *index));
    }
    let changed: BTreeSet<(RepoName, RelPath)> = deltas.iter().map(|delta| (delta.repo.clone(), delta.path.clone())).collect();
    affected
      .into_iter()
      .filter_map(|index| self.file(index).map(|file| (file.repo.clone(), file.path.clone())))
      .filter(|key| !changed.contains(key))
      .collect()
  }

  pub fn refresh(&mut self) -> Vec<Relocation> {
    let mut packages = mem::take(&mut self.dirty_packages);
    let mut relocations = Vec::new();
    for repo in mem::take(&mut self.dirty_repos) {
      let indices: Vec<FileIdx> = self.file_ids.iter().filter(|((file_repo, _), _)| *file_repo == repo).map(|(_, index)| *index).collect();
      for index in indices {
        if let Some(old) = self.unassign_package(index) {
          packages.insert(old);
        }
        match self.assign_package(index) {
          Some(package) => {
            packages.insert(package);
          }
          None => relocations.extend(self.place_unpackaged(index)),
        }
      }
      packages.extend(self.layout.cargo_packages().filter(|package| package.repo == repo));
    }
    for package in packages {
      let files: Vec<FileIdx> = self.package_files.get(&package).map(|files| files.iter().copied().collect()).unwrap_or_default();
      let rust_files: Vec<FileIdx> = files
        .into_iter()
        .filter(|index| self.file(*index).is_some_and(|file| file.family == LanguageFamily::Rust))
        .collect();
      let placements = place_package(self, &package, &rust_files);
      for (index, placement) in placements {
        relocations.extend(self.set_placement(index, placement));
      }
    }
    relocations
  }

  pub fn placement(&self, repo: &RepoName, path: &RelPath) -> Option<FilePlacement> {
    let index = self.file_ids.get(&(repo.clone(), path.clone()))?;
    Some(self.effective_placement(*index))
  }

  pub(crate) fn effective_placement(&self, index: FileIdx) -> FilePlacement {
    if let Some(placement) = self.placements.get(&index) {
      return placement.clone();
    }
    let (context, reachability) = match self.file(index) {
      Some(file) if file.family == LanguageFamily::Rust => (file.context.clone(), Reachability::Unpackaged),
      Some(file) => (file.context.clone(), Reachability::NotApplicable),
      None => (
        KeyContext {
          package: String::new(),
          module_path: Vec::new(),
        },
        Reachability::NotApplicable,
      ),
    };
    FilePlacement {
      context,
      reachability,
      also_in: Vec::new(),
    }
  }

  pub(crate) fn file_index(&self, repo: &RepoName, path: &RelPath) -> Option<FileIdx> {
    self.file_ids.get(&(repo.clone(), path.clone())).copied()
  }

  pub(crate) fn file(&self, index: FileIdx) -> Option<&FileSurface> {
    self.files.get(index.0 as usize)?.as_ref()
  }

  pub(crate) fn sym(&self, symbol: SymRef) -> Option<&crate::surface::SymSurface> {
    self.file(symbol.file)?.symbol(symbol.local)
  }

  pub(crate) fn file_package(&self, index: FileIdx) -> Option<&PackageKey> {
    self.file_packages.get(&index)
  }

  pub(crate) fn module_entry(&self, module: &ModuleId) -> Option<&ModuleEntry> {
    self.modules.get(module)
  }

  pub(crate) fn rust_module_of(&self, index: FileIdx, suffix: &[String]) -> Option<ModuleId> {
    let file = self.file(index)?;
    let placement = self.effective_placement(index);
    let mut path = placement.context.module_path;
    path.extend(suffix.iter().cloned());
    Some(ModuleId {
      repo: file.repo.clone(),
      package: placement.context.package,
      path,
    })
  }

  pub(crate) fn symbol_key(&self, symbol: SymRef) -> Option<SymbolKey> {
    let file = self.file(symbol.file)?;
    let surface = file.symbol(symbol.local)?;
    let context = self.effective_placement(symbol.file).context;
    let mut module_path = context.module_path;
    module_path.extend(surface.module_suffix.iter().cloned());
    SymbolKey::new(
      file.family,
      file.repo.clone(),
      context.package,
      module_path,
      surface.item_path.clone(),
      surface.kind.clone(),
      surface.disambiguator,
    )
    .ok()
  }

  pub(crate) fn symbol_entity(&self, symbol: SymRef) -> Option<EntityId> {
    Some(EntityId::of(&EntityKey::Symbol(self.symbol_key(symbol)?)))
  }

  pub(crate) fn file_entity(&self, index: FileIdx) -> Option<EntityId> {
    let file = self.file(index)?;
    Some(EntityId::of(&EntityKey::File {
      repo: file.repo.clone(),
      path: file.path.clone(),
    }))
  }

  pub(crate) fn module_entity(&self, module: &ModuleId) -> Option<EntityId> {
    let entry = self.modules.get(module)?;
    match entry.decl {
      Some(declaration) => self.symbol_entity(declaration),
      None => self.file_entity(entry.effective_locs().next()?.file),
    }
  }

  fn attach(&mut self, index: FileIdx, surface: FileSurface) {
    for (local, symbol) in surface.symbols.iter().enumerate() {
      let reference = SymRef { file: index, local: local as u32 };
      match symbol.kind {
        EntityKind::Method => self.methods_by_name.entry(symbol.name.clone()).or_default().push(reference),
        EntityKind::Macro => self.macros_by_name.entry(symbol.name.clone()).or_default().push(reference),
        EntityKind::Class => {
          for implemented in &symbol.implements {
            if let Some(name) = implemented.last() {
              self.ts_implementers.entry(name.clone()).or_default().push(reference);
            }
          }
          for base in &symbol.extends {
            if let Some(name) = base.last() {
              self.ts_subclasses.entry(name.clone()).or_default().push(reference);
            }
          }
        }
        _ => {}
      }
    }
    for (position, implementation) in surface.rust.impls.iter().enumerate() {
      let entry = (index, position as u32);
      if let Some(name) = implementation.trait_ref.as_ref().and_then(|trait_ref| trait_ref.name()) {
        self.rust_impls_by_trait.entry(name.to_owned()).or_default().push(entry);
      }
      if let Some(name) = implementation.self_type.name() {
        self.rust_impls_by_self.entry(name.to_owned()).or_default().push(entry);
      }
    }
    let is_rust = surface.family == LanguageFamily::Rust;
    self.files[index.0 as usize] = Some(surface);
    match self.assign_package(index) {
      Some(package) if is_rust => {
        self.dirty_packages.insert(package);
      }
      Some(_) => {}
      None if is_rust => {
        self.place_unpackaged(index);
      }
      None => {}
    }
  }

  fn detach(&mut self, index: FileIdx) {
    if let Some(package) = self.unassign_package(index)
      && self.file(index).is_some_and(|file| file.family == LanguageFamily::Rust)
    {
      self.dirty_packages.insert(package);
    }
    self.unindex_modules(index);
    self.placements.remove(&index);
    let Some(surface) = self.files[index.0 as usize].take() else { return };
    let mut names: BTreeSet<(&str, EntityKind)> = BTreeSet::new();
    for symbol in &surface.symbols {
      names.insert((symbol.name.as_str(), symbol.kind.clone()));
      for implemented in &symbol.implements {
        if let Some(name) = implemented.last() {
          retain_other_files(self.ts_implementers.get_mut(name), index, |reference| reference.file);
        }
      }
      for base in &symbol.extends {
        if let Some(name) = base.last() {
          retain_other_files(self.ts_subclasses.get_mut(name), index, |reference| reference.file);
        }
      }
    }
    for (name, kind) in names {
      match kind {
        EntityKind::Method => retain_other_files(self.methods_by_name.get_mut(name), index, |reference| reference.file),
        EntityKind::Macro => retain_other_files(self.macros_by_name.get_mut(name), index, |reference| reference.file),
        _ => {}
      }
    }
    for implementation in &surface.rust.impls {
      if let Some(name) = implementation.trait_ref.as_ref().and_then(|trait_ref| trait_ref.name()) {
        retain_other_files(self.rust_impls_by_trait.get_mut(name), index, |entry| entry.0);
      }
      if let Some(name) = implementation.self_type.name() {
        retain_other_files(self.rust_impls_by_self.get_mut(name), index, |entry| entry.0);
      }
    }
  }

  fn assign_package(&mut self, index: FileIdx) -> Option<PackageKey> {
    let file = self.file(index)?;
    let package = self.layout.package_for_file(&file.repo, &file.path)?;
    self.file_packages.insert(index, package.clone());
    self.package_files.entry(package.clone()).or_default().insert(index);
    Some(package)
  }

  fn unassign_package(&mut self, index: FileIdx) -> Option<PackageKey> {
    let package = self.file_packages.remove(&index)?;
    if let Some(files) = self.package_files.get_mut(&package) {
      files.remove(&index);
      if files.is_empty() {
        self.package_files.remove(&package);
      }
    }
    Some(package)
  }

  fn place_unpackaged(&mut self, index: FileIdx) -> Option<Relocation> {
    let file = self.file(index)?;
    if file.family != LanguageFamily::Rust {
      return None;
    }
    let placement = FilePlacement {
      context: file.context.clone(),
      reachability: Reachability::Unpackaged,
      also_in: Vec::new(),
    };
    self.set_placement(index, placement)
  }

  fn set_placement(&mut self, index: FileIdx, placement: FilePlacement) -> Option<Relocation> {
    let previous = self.placements.get(&index).map(|placement| placement.context.clone());
    self.unindex_modules(index);
    let file = self.file(index)?;
    let baseline = previous.unwrap_or_else(|| file.context.clone());
    let relocation = (baseline != placement.context).then(|| Relocation {
      repo: file.repo.clone(),
      path: file.path.clone(),
      from: baseline,
      to: placement.context.clone(),
    });
    if let Some(relocation) = &relocation {
      debug!(repo = %relocation.repo, path = %relocation.path, package = %relocation.to.package, module = ?relocation.to.module_path, "rust file relocated by the module tree");
    }
    self.placements.insert(index, placement);
    self.index_modules(index);
    relocation
  }

  fn module_slots(&self, index: FileIdx) -> Vec<(ModuleId, Option<ModuleLoc>, Option<SymRef>)> {
    let Some(file) = self.file(index) else { return Vec::new() };
    let placement = self.effective_placement(index);
    let weak = placement.reachability == Reachability::Unreachable;
    let mut slots = Vec::new();
    for (position, context) in std::iter::once(&placement.context).chain(&placement.also_in).enumerate() {
      let primary = position == 0;
      let base = ModuleId {
        repo: file.repo.clone(),
        package: context.package.clone(),
        path: context.module_path.clone(),
      };
      slots.push((
        base.clone(),
        Some(ModuleLoc {
          file: index,
          suffix: Vec::new(),
          weak,
        }),
        None,
      ));
      for declaration in &file.rust.module_decls {
        let mut suffix = declaration.module_suffix.clone();
        suffix.push(declaration.name.clone());
        let symbol = declaration.inline.or_else(|| {
          file
            .symbols
            .iter()
            .position(|symbol| symbol.kind == EntityKind::Module && symbol.item_path.is_empty() && symbol.module_suffix == suffix)
            .map(|position| position as u32)
        });
        let mut module = base.clone();
        module.path.extend(suffix.iter().cloned());
        let location = declaration.inline.map(|_| ModuleLoc { file: index, suffix, weak });
        let declaration = symbol.filter(|_| primary).map(|local| SymRef { file: index, local });
        slots.push((module, location, declaration));
      }
    }
    slots
  }

  fn index_modules(&mut self, index: FileIdx) {
    for (module, location, declaration) in self.module_slots(index) {
      let entry = self.modules.entry(module).or_default();
      if let Some(location) = location {
        entry.locs.push(location);
        entry.locs.sort_by_key(|location| location.file);
      }
      if entry.decl.is_none() {
        entry.decl = declaration;
      }
    }
  }

  fn unindex_modules(&mut self, index: FileIdx) {
    if !self.placements.contains_key(&index) {
      return;
    }
    for (module, _, _) in self.module_slots(index) {
      let Some(entry) = self.modules.get_mut(&module) else { continue };
      entry.locs.retain(|location| location.file != index);
      if entry.decl.is_some_and(|declaration| declaration.file == index) {
        entry.decl = None;
      }
      if entry.locs.is_empty() && entry.decl.is_none() {
        self.modules.remove(&module);
      }
    }
  }
}

fn retain_other_files<T>(entries: Option<&mut Vec<T>>, index: FileIdx, file_of: impl Fn(&T) -> FileIdx) {
  if let Some(entries) = entries {
    entries.retain(|entry| file_of(entry) != index);
  }
}
