mod cargo;
mod json;
mod npm;
mod tsconfig;
mod types;

use std::path::Path;

use atlas_core::{Language, LanguageFamily, RelPath, RustTarget};

pub use self::json::{Json, jsonc_to_json, parse_jsonc};
pub use self::npm::classify_spec;
pub use self::types::*;
use crate::discovery::DiscoveredFile;

#[derive(Debug, Default)]
pub struct PackageGraph {
  pub packages: Vec<PackageInfo>,
  pub workspaces: Vec<WorkspaceInfo>,
  pub tsconfigs: Vec<TsConfigInfo>,
  pub warnings: Vec<ManifestWarning>,
}

impl PackageGraph {
  pub fn load(repo_root: &Path, files: &[DiscoveredFile]) -> Self {
    let root = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let mut ctx = LoadContext {
      root: &root,
      files,
      warnings: Vec::new(),
    };
    let (mut packages, mut workspaces) = cargo::load(&mut ctx);
    let (npm_packages, npm_workspaces) = npm::load(&mut ctx);
    packages.extend(npm_packages);
    workspaces.extend(npm_workspaces);
    let tsconfigs = tsconfig::load(&mut ctx);
    let mut warnings = ctx.warnings;
    warnings.sort_by(|a, b| (&a.file, &a.error).cmp(&(&b.file, &b.error)));
    warnings.dedup();
    Self {
      packages,
      workspaces,
      tsconfigs,
      warnings,
    }
  }

  pub fn package_for_file(&self, file: &RelPath) -> Option<&PackageInfo> {
    let wanted = Language::from_path(file.as_str()).map(Language::family);
    self
      .packages
      .iter()
      .filter(|package| package.root.contains(file))
      .filter(|package| {
        matches!(
          (wanted, package.ecosystem()),
          (Some(LanguageFamily::Rust), Ecosystem::Cargo) | (Some(LanguageFamily::Ts), Ecosystem::Npm) | (None, _)
        )
      })
      .max_by(|a, b| a.root.depth().cmp(&b.root.depth()).then_with(|| b.manifest.cmp(&a.manifest)))
  }

  pub fn rust_targets_for_file(&self, file: &RelPath) -> Vec<RustFileTarget> {
    let Some(cargo) = self.package_for_file(file).and_then(PackageInfo::as_cargo) else {
      return Vec::new();
    };
    let roots: Vec<&RustTargetInfo> = cargo.targets.iter().filter(|target| target.root_file == *file).collect();
    let candidates: Vec<&RustTargetInfo> = if roots.is_empty() {
      let containing: Vec<&RustTargetInfo> = cargo
        .targets
        .iter()
        .filter(|target| target.target != RustTarget::BuildScript && target.module_dir().contains(file))
        .collect();
      let deepest = containing.iter().map(|target| target.module_dir().depth()).max();
      containing.into_iter().filter(|target| Some(target.module_dir().depth()) == deepest).collect()
    } else {
      roots
    };
    candidates
      .into_iter()
      .filter_map(|target| {
        Some(RustFileTarget {
          target: target.target.clone(),
          module_path: default_rust_module_path(target, file)?,
        })
      })
      .collect()
  }

  pub fn tsconfig_for_file(&self, file: &RelPath) -> Option<&TsConfigInfo> {
    self
      .tsconfigs
      .iter()
      .filter(|config| config.is_primary() && config.dir.contains(file))
      .max_by_key(|config| (config.dir.depth(), config.path.file_name() == "tsconfig.json"))
  }
}

#[cfg(test)]
mod tests;
