use std::collections::{HashMap, HashSet, VecDeque};

use atlas_core::{KeyContext, RelPath, RustTarget};
use atlas_project::RepoDir;

use crate::layout::PackageKey;
use crate::surface::{FileSurface, ModDeclSurface};
use crate::table::{FileIdx, FilePlacement, ProjectSymbolTable, Reachability};

const MAX_MODULE_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirMode {
  ModRs,
  NonModRs,
}

struct Visit {
  file: FileIdx,
  module_path: Vec<String>,
  mode: DirMode,
  cfg: Vec<String>,
}

fn target_rank(target: &RustTarget) -> (u8, String) {
  match target {
    RustTarget::Lib => (0, String::new()),
    RustTarget::Bin(name) => (1, name.clone()),
    RustTarget::Test(name) => (2, name.clone()),
    RustTarget::Example(name) => (3, name.clone()),
    RustTarget::Bench(name) => (4, name.clone()),
    RustTarget::BuildScript => (5, String::new()),
  }
}

fn reach_rank(reachability: &Reachability) -> u8 {
  match reachability {
    Reachability::TargetRoot => 0,
    Reachability::Reachable => 1,
    Reachability::CfgGated { .. } => 2,
    Reachability::Unreachable | Reachability::Unpackaged | Reachability::NotApplicable => 3,
  }
}

pub(crate) fn place_package(table: &ProjectSymbolTable, package: &PackageKey, files: &[FileIdx]) -> Vec<(FileIdx, FilePlacement)> {
  let by_path: HashMap<&RelPath, FileIdx> = files.iter().filter_map(|index| table.file(*index).map(|file| (&file.path, *index))).collect();
  let mut placed: HashMap<FileIdx, FilePlacement> = HashMap::new();
  if let Some(cargo) = table.layout.cargo(package) {
    let mut targets: Vec<_> = cargo.targets.iter().collect();
    targets.sort_by_key(|target| target_rank(&target.target));
    for target in targets {
      let Some(root) = by_path.get(&target.root_file).copied() else { continue };
      let package_field = target.target.package_field(&cargo.name);
      walk_target(table, &by_path, root, &package_field, &mut placed);
    }
  }
  files
    .iter()
    .filter_map(|index| {
      if let Some(placement) = placed.remove(index) {
        return Some((*index, placement));
      }
      let file = table.file(*index)?;
      let reachability = if table.layout.cargo(package).is_some() {
        Reachability::Unreachable
      } else {
        Reachability::Unpackaged
      };
      Some((
        *index,
        FilePlacement {
          context: file.context.clone(),
          reachability,
          also_in: Vec::new(),
        },
      ))
    })
    .collect()
}

fn walk_target(table: &ProjectSymbolTable, by_path: &HashMap<&RelPath, FileIdx>, root: FileIdx, package_field: &str, placed: &mut HashMap<FileIdx, FilePlacement>) {
  let mut queue = VecDeque::from([Visit {
    file: root,
    module_path: Vec::new(),
    mode: DirMode::ModRs,
    cfg: Vec::new(),
  }]);
  let mut visited: HashSet<(FileIdx, Vec<String>)> = HashSet::new();
  while let Some(visit) = queue.pop_front() {
    if visit.module_path.len() > MAX_MODULE_DEPTH || !visited.insert((visit.file, visit.module_path.clone())) {
      continue;
    }
    let Some(file) = table.file(visit.file) else { continue };
    let reachability = match (visit.file == root, visit.cfg.as_slice()) {
      (true, _) => Reachability::TargetRoot,
      (false, []) => Reachability::Reachable,
      (false, [single]) => Reachability::CfgGated { cfg: single.clone() },
      (false, many) => Reachability::CfgGated {
        cfg: format!("all({})", many.join(", ")),
      },
    };
    let context = KeyContext {
      package: package_field.to_owned(),
      module_path: visit.module_path.clone(),
    };
    match placed.get_mut(&visit.file) {
      None => {
        placed.insert(
          visit.file,
          FilePlacement {
            context,
            reachability,
            also_in: Vec::new(),
          },
        );
      }
      Some(existing) if reach_rank(&reachability) < reach_rank(&existing.reachability) => {
        let previous = std::mem::replace(&mut existing.context, context);
        existing.reachability = reachability;
        existing.also_in.push(previous);
      }
      Some(existing) => {
        if existing.context != context && !existing.also_in.contains(&context) {
          existing.also_in.push(context);
        }
      }
    }
    for declaration in file.rust.module_decls.iter().filter(|declaration| declaration.inline.is_none()) {
      let Some((child, mode)) = child_candidates(&file.path, visit.mode, declaration)
        .into_iter()
        .find_map(|(path, mode)| Some((*by_path.get(&path)?, mode)))
      else {
        continue;
      };
      let mut cfg = visit.cfg.clone();
      cfg.extend(enclosing_cfgs(file, declaration));
      let mut module_path = visit.module_path.clone();
      module_path.extend(declaration.module_suffix.iter().cloned());
      module_path.push(declaration.name.clone());
      queue.push_back(Visit {
        file: child,
        module_path,
        mode,
        cfg,
      });
    }
  }
}

fn enclosing_cfgs(file: &FileSurface, declaration: &ModDeclSurface) -> Vec<String> {
  let mut cfgs = Vec::new();
  for depth in 0..declaration.module_suffix.len() {
    let (parent_suffix, name) = (&declaration.module_suffix[..depth], &declaration.module_suffix[depth]);
    if let Some(cfg) = file
      .module_decls_in(parent_suffix)
      .find(|inline| inline.inline.is_some() && inline.name == *name)
      .and_then(|inline| inline.cfg.clone())
    {
      cfgs.push(cfg);
    }
  }
  cfgs.extend(declaration.cfg.clone());
  cfgs
}

fn child_candidates(parent: &RelPath, mode: DirMode, declaration: &ModDeclSurface) -> Vec<(RelPath, DirMode)> {
  let parent_dir = RepoDir::of_file(parent);
  let stem = parent.file_name().strip_suffix(".rs").unwrap_or(parent.file_name());
  let children_dir = match mode {
    DirMode::ModRs => Some(parent_dir.clone()),
    DirMode::NonModRs => parent_dir.resolve(stem),
  };
  let inline_dir = children_dir.and_then(|dir| match declaration.module_suffix.is_empty() {
    true => Some(dir),
    false => dir.resolve(&declaration.module_suffix.join("/")),
  });
  if let Some(path_attr) = &declaration.path_attr {
    let base = if declaration.module_suffix.is_empty() { Some(parent_dir) } else { inline_dir };
    return base.and_then(|base| base.file(path_attr)).map(|path| (path, DirMode::ModRs)).into_iter().collect();
  }
  let Some(dir) = inline_dir else { return Vec::new() };
  [
    (dir.file(&format!("{}.rs", declaration.name)), DirMode::NonModRs),
    (dir.file(&format!("{}/mod.rs", declaration.name)), DirMode::ModRs),
  ]
  .into_iter()
  .filter_map(|(path, mode)| Some((path?, mode)))
  .collect()
}
