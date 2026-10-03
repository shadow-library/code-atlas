use std::collections::HashSet;

use atlas_core::{RelPath, RustTarget};
use toml::{Table, Value};

use super::types::{
  CargoDependency, CargoDependencySource, CargoPackage, DependencySection, Ecosystem, LoadContext, PackageInfo, PackageKind, RepoDir, RustTargetInfo, WorkspaceInfo,
};
use crate::globs::PathGlobs;

struct RawManifest {
  manifest: RelPath,
  dir: RepoDir,
  table: Table,
}

impl RawManifest {
  fn workspace(&self) -> Option<&Table> {
    self.table.get("workspace")?.as_table()
  }

  fn package(&self) -> Option<&Table> {
    self.table.get("package")?.as_table()
  }
}

pub(super) fn load(ctx: &mut LoadContext<'_>) -> (Vec<PackageInfo>, Vec<WorkspaceInfo>) {
  let manifests: Vec<RelPath> = ctx.files_named("Cargo.toml").cloned().collect();
  let mut raws = Vec::new();
  for manifest in manifests {
    let Some(text) = ctx.read(&manifest) else { continue };
    match text.parse::<Table>() {
      Ok(table) => raws.push(RawManifest {
        dir: RepoDir::of_file(&manifest),
        manifest,
        table,
      }),
      Err(error) => ctx.warn(&manifest, format_args!("invalid TOML: {error}")),
    }
  }
  raws.retain(|raw| {
    let usable = raw.package().is_some() || raw.workspace().is_some();
    if !usable {
      ctx.warn(&raw.manifest, "manifest defines neither [package] nor [workspace]");
    }
    usable
  });

  let package_dirs: Vec<&RepoDir> = raws.iter().filter(|raw| raw.package().is_some()).map(|raw| &raw.dir).collect();
  let workspaces: Vec<WorkspaceInfo> = raws.iter().filter_map(|raw| build_workspace(raw, &package_dirs, ctx)).collect();

  let rust_files: Vec<&RelPath> = ctx.files.iter().map(|file| &file.path).filter(|path| path.extension() == Some("rs")).collect();
  let file_set: HashSet<&str> = rust_files.iter().map(|path| path.as_str()).collect();

  let mut packages = Vec::new();
  for raw in &raws {
    let Some(package_table) = raw.package() else { continue };
    let workspace = owning_workspace(&raw.dir, &workspaces);
    let workspace_raw = workspace.and_then(|info| raws.iter().find(|candidate| candidate.manifest == info.manifest));
    let Some(name) = package_table.get("name").and_then(Value::as_str) else {
      ctx.warn(&raw.manifest, "[package] has no string name");
      continue;
    };
    let package = CargoPackage {
      name: name.to_owned(),
      version: package_version(package_table, workspace_raw),
      targets: collect_targets(raw, name, &rust_files, &file_set, ctx),
      dependencies: collect_dependencies(raw, workspace_raw, ctx),
      workspace_root: workspace.map(|info| info.root.clone()),
    };
    packages.push(PackageInfo {
      manifest: raw.manifest.clone(),
      root: raw.dir.clone(),
      kind: PackageKind::Cargo(package),
    });
  }
  (packages, workspaces)
}

fn string_list(table: &Table, key: &str) -> Vec<String> {
  table
    .get(key)
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default()
}

fn build_workspace(raw: &RawManifest, package_dirs: &[&RepoDir], ctx: &mut LoadContext<'_>) -> Option<WorkspaceInfo> {
  let workspace = raw.workspace()?;
  let member_patterns = string_list(workspace, "members");
  let exclude_patterns = string_list(workspace, "exclude");
  let anchored = |patterns: &[String]| -> Vec<String> { patterns.iter().map(|pattern| raw.dir.join_pattern(pattern.strip_prefix("./").unwrap_or(pattern))).collect() };
  let (members, excluded) = match (PathGlobs::anchored(&anchored(&member_patterns)), PathGlobs::anchored(&anchored(&exclude_patterns))) {
    (Ok(members), Ok(excluded)) => (members, excluded),
    (Err(error), _) | (_, Err(error)) => {
      ctx.warn(&raw.manifest, format_args!("invalid workspace glob: {error}"));
      return None;
    }
  };
  let mut resolved: Vec<RepoDir> = package_dirs
    .iter()
    .filter(|dir| **dir != &raw.dir && !dir.is_root() && members.matches(dir.as_str(), true) && !excluded.matches(dir.as_str(), true))
    .map(|dir| (*dir).clone())
    .collect();
  resolved.sort();
  for pattern in &member_patterns {
    let literal = !pattern.contains(['*', '?', '[']);
    let target = raw.dir.resolve(pattern);
    let present = target.as_ref().is_some_and(|dir| *dir == raw.dir || resolved.contains(dir) || package_dirs.contains(&dir));
    if literal && !present {
      ctx.warn(&raw.manifest, format_args!("workspace member {pattern:?} has no Cargo.toml in the repository"));
    }
  }
  Some(WorkspaceInfo {
    manifest: raw.manifest.clone(),
    root: raw.dir.clone(),
    ecosystem: Ecosystem::Cargo,
    is_virtual: raw.package().is_none(),
    member_patterns,
    exclude_patterns,
    members: resolved,
  })
}

fn owning_workspace<'a>(package_dir: &RepoDir, workspaces: &'a [WorkspaceInfo]) -> Option<&'a WorkspaceInfo> {
  if let Some(own) = workspaces.iter().find(|workspace| workspace.root == *package_dir) {
    return Some(own);
  }
  workspaces
    .iter()
    .filter(|workspace| workspace.members.contains(package_dir))
    .max_by_key(|workspace| workspace.root.depth())
}

fn package_version(package: &Table, workspace: Option<&RawManifest>) -> Option<String> {
  match package.get("version")? {
    Value::String(version) => Some(version.clone()),
    Value::Table(inherit) if inherit.get("workspace").and_then(Value::as_bool) == Some(true) => {
      workspace?.workspace()?.get("package")?.as_table()?.get("version")?.as_str().map(str::to_owned)
    }
    _ => None,
  }
}

fn collect_targets(raw: &RawManifest, package_name: &str, rust_files: &[&RelPath], file_set: &HashSet<&str>, ctx: &mut LoadContext<'_>) -> Vec<RustTargetInfo> {
  let package = raw.package();
  let flag = |key: &str| package.and_then(|table| table.get(key)).and_then(Value::as_bool) != Some(false);
  let exists = |path: &RelPath| file_set.contains(path.as_str());
  let mut targets: Vec<RustTargetInfo> = Vec::new();
  let lib_table = raw.table.get("lib").and_then(Value::as_table);
  let lib_path = lib_table.and_then(|lib| lib.get("path")).and_then(Value::as_str).unwrap_or("src/lib.rs");
  if let Some(root_file) = raw.dir.file(lib_path)
    && (lib_table.is_some() || exists(&root_file))
  {
    let name = lib_table.and_then(|lib| lib.get("name")).and_then(Value::as_str).unwrap_or(package_name).replace('-', "_");
    push_target(&mut targets, RustTarget::Lib, name, root_file);
  }

  for (key, kind, directory, auto) in [
    ("bin", RustTarget::Bin as fn(String) -> RustTarget, "src/bin", flag("autobins")),
    ("test", RustTarget::Test, "tests", flag("autotests")),
    ("example", RustTarget::Example, "examples", flag("autoexamples")),
    ("bench", RustTarget::Bench, "benches", flag("autobenches")),
  ] {
    let explicit = raw.table.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    for entry in explicit.iter().filter_map(Value::as_table) {
      let Some(name) = entry.get("name").and_then(Value::as_str).or((key == "bin").then_some(package_name)) else {
        ctx.warn(&raw.manifest, format_args!("[[{key}]] entry has no name"));
        continue;
      };
      let root = match entry.get("path").and_then(Value::as_str) {
        Some(path) => raw.dir.file(path),
        None => default_target_root(raw, key, directory, name, package_name, &exists),
      };
      match root {
        Some(root_file) => push_target(&mut targets, kind(name.to_owned()), name.to_owned(), root_file),
        None => ctx.warn(&raw.manifest, format_args!("[[{key}]] {name:?} path escapes the repository")),
      }
    }
    if !auto {
      continue;
    }
    if key == "bin"
      && let Some(main) = raw.dir.file("src/main.rs")
      && exists(&main)
    {
      push_target(&mut targets, kind(package_name.to_owned()), package_name.to_owned(), main);
    }
    let Some(directory) = raw.dir.file(directory) else { continue };
    for path in rust_files {
      let Some(within) = path.strip_dir_prefix(&directory) else { continue };
      let name = match within.split('/').collect::<Vec<_>>().as_slice() {
        [file] => file.strip_suffix(".rs"),
        [dir, "main.rs"] => Some(*dir),
        _ => None,
      };
      if let Some(name) = name {
        push_target(&mut targets, kind(name.to_owned()), name.to_owned(), (*path).clone());
      }
    }
  }

  let build_path = match package.and_then(|table| table.get("build")) {
    Some(Value::Boolean(false)) => None,
    Some(Value::String(path)) => raw.dir.file(path),
    _ => raw.dir.file("build.rs").filter(|path| exists(path)),
  };
  if let Some(root_file) = build_path {
    push_target(&mut targets, RustTarget::BuildScript, "build-script-build".to_owned(), root_file);
  }
  targets
}

fn push_target(targets: &mut Vec<RustTargetInfo>, target: RustTarget, name: String, root_file: RelPath) {
  if targets.iter().any(|existing| existing.target == target && existing.name == name) {
    return;
  }
  targets.push(RustTargetInfo { target, name, root_file });
}

fn default_target_root(raw: &RawManifest, key: &str, directory: &str, name: &str, package_name: &str, exists: &dyn Fn(&RelPath) -> bool) -> Option<RelPath> {
  if key == "bin" && name == package_name {
    let main = raw.dir.file("src/main.rs")?;
    if exists(&main) {
      return Some(main);
    }
  }
  let flat = raw.dir.file(&format!("{directory}/{name}.rs"))?;
  if exists(&flat) {
    return Some(flat);
  }
  let nested = raw.dir.file(&format!("{directory}/{name}/main.rs"))?;
  Some(if exists(&nested) { nested } else { flat })
}

fn collect_dependencies(raw: &RawManifest, workspace: Option<&RawManifest>, ctx: &mut LoadContext<'_>) -> Vec<CargoDependency> {
  let workspace_dependencies = workspace.and_then(|ws| ws.workspace()?.get("dependencies")?.as_table().map(|table| (table, &ws.dir)));
  let mut dependencies = Vec::new();
  let sections = [
    ("dependencies", DependencySection::Normal),
    ("dev-dependencies", DependencySection::Dev),
    ("build-dependencies", DependencySection::Build),
  ];
  for (key, section) in sections {
    if let Some(table) = raw.table.get(key).and_then(Value::as_table) {
      read_dependency_table(raw, table, section, None, workspace_dependencies, ctx, &mut dependencies);
    }
  }
  if let Some(targets) = raw.table.get("target").and_then(Value::as_table) {
    for (cfg, target) in targets {
      for (key, section) in sections {
        if let Some(table) = target.get(key).and_then(Value::as_table) {
          read_dependency_table(raw, table, section, Some(cfg), workspace_dependencies, ctx, &mut dependencies);
        }
      }
    }
  }
  dependencies
}

struct DependencyFields {
  source: CargoDependencySource,
  version: Option<String>,
  package: Option<String>,
}

fn dependency_fields(value: &Value, base: &RepoDir) -> DependencyFields {
  let Some(table) = value.as_table() else {
    return DependencyFields {
      source: CargoDependencySource::Registry,
      version: value.as_str().map(str::to_owned),
      package: None,
    };
  };
  let source = if let Some(path) = table.get("path").and_then(Value::as_str) {
    CargoDependencySource::Path {
      raw: path.to_owned(),
      resolved: base.resolve(path),
    }
  } else if let Some(url) = table.get("git").and_then(Value::as_str) {
    CargoDependencySource::Git { url: url.to_owned() }
  } else {
    CargoDependencySource::Registry
  };
  DependencyFields {
    source,
    version: table.get("version").and_then(Value::as_str).map(str::to_owned),
    package: table.get("package").and_then(Value::as_str).map(str::to_owned),
  }
}

fn read_dependency_table(
  raw: &RawManifest,
  table: &Table,
  section: DependencySection,
  target_cfg: Option<&str>,
  workspace_dependencies: Option<(&Table, &RepoDir)>,
  ctx: &mut LoadContext<'_>,
  out: &mut Vec<CargoDependency>,
) {
  for (local_name, value) in table {
    let inherits = value.as_table().and_then(|entry| entry.get("workspace")).and_then(Value::as_bool) == Some(true);
    let local = value.as_table();
    let optional = local.and_then(|entry| entry.get("optional")).and_then(Value::as_bool) == Some(true);
    let fields = if inherits {
      match workspace_dependencies.and_then(|(table, dir)| table.get(local_name).map(|entry| (entry, dir))) {
        Some((entry, dir)) => {
          let mut inherited = dependency_fields(entry, dir);
          if let Some(package) = local.and_then(|entry| entry.get("package")).and_then(Value::as_str) {
            inherited.package = Some(package.to_owned());
          }
          inherited
        }
        None => {
          ctx.warn(&raw.manifest, format_args!("dependency {local_name:?} uses workspace = true but no workspace declares it"));
          DependencyFields {
            source: CargoDependencySource::UnresolvedWorkspace,
            version: None,
            package: None,
          }
        }
      }
    } else {
      dependency_fields(value, &raw.dir)
    };
    out.push(CargoDependency {
      local_name: local_name.clone(),
      package_name: fields.package.unwrap_or_else(|| local_name.clone()),
      section,
      source: fields.source,
      version: fields.version,
      optional,
      target_cfg: target_cfg.map(str::to_owned),
    });
  }
}
