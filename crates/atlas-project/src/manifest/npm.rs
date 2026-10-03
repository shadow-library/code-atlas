use atlas_core::RelPath;

use super::json::{Json, parse_json};
use super::types::{DependencySection, Ecosystem, LoadContext, NpmDependency, NpmExportTarget, NpmExports, NpmPackage, NpmSpec, PackageInfo, PackageKind, RepoDir, WorkspaceInfo};
use crate::globs::PathGlobs;

struct RawPackage {
  manifest: RelPath,
  dir: RepoDir,
  json: Json,
}

pub(super) fn load(ctx: &mut LoadContext<'_>) -> (Vec<PackageInfo>, Vec<WorkspaceInfo>) {
  let manifests: Vec<RelPath> = ctx.files_named("package.json").cloned().collect();
  let mut raws = Vec::new();
  for manifest in manifests {
    let Some(text) = ctx.read(&manifest) else { continue };
    match parse_json(&text) {
      Ok(json) if json.as_object().is_some() => raws.push(RawPackage {
        dir: RepoDir::of_file(&manifest),
        manifest,
        json,
      }),
      Ok(_) => ctx.warn(&manifest, "package.json must contain a JSON object"),
      Err(error) => ctx.warn(&manifest, format_args!("invalid JSON: {error}")),
    }
  }

  let all_dirs: Vec<&RepoDir> = raws.iter().map(|raw| &raw.dir).collect();
  let workspaces: Vec<WorkspaceInfo> = raws.iter().filter_map(|raw| build_workspace(raw, &all_dirs, ctx)).collect();

  let mut packages = Vec::new();
  for raw in &raws {
    let package = build_package(raw, &workspaces, ctx);
    packages.push(PackageInfo {
      manifest: raw.manifest.clone(),
      root: raw.dir.clone(),
      kind: PackageKind::Npm(package),
    });
  }
  (packages, workspaces)
}

fn workspace_patterns(json: &Json) -> Option<Vec<String>> {
  let declared = json.get("workspaces")?;
  let list = match declared {
    Json::Array(items) => items.as_slice(),
    Json::Object(_) => declared.get("packages")?.as_array()?,
    _ => return None,
  };
  Some(list.iter().filter_map(Json::as_str).map(str::to_owned).collect())
}

fn build_workspace(raw: &RawPackage, all_dirs: &[&RepoDir], ctx: &mut LoadContext<'_>) -> Option<WorkspaceInfo> {
  let patterns = workspace_patterns(&raw.json)?;
  let anchored: Vec<String> = patterns
    .iter()
    .map(|pattern| match pattern.strip_prefix('!') {
      Some(body) => format!("!{}", raw.dir.join_pattern(body.strip_prefix("./").unwrap_or(body))),
      None => raw.dir.join_pattern(pattern.strip_prefix("./").unwrap_or(pattern)),
    })
    .collect();
  let globs = match PathGlobs::anchored(&anchored) {
    Ok(globs) => globs,
    Err(error) => {
      ctx.warn(&raw.manifest, format_args!("invalid workspaces glob: {error}"));
      return None;
    }
  };
  let mut members: Vec<RepoDir> = all_dirs
    .iter()
    .filter(|dir| **dir != &raw.dir && !dir.is_root() && globs.matches(dir.as_str(), true))
    .map(|dir| (*dir).clone())
    .collect();
  members.sort();
  Some(WorkspaceInfo {
    manifest: raw.manifest.clone(),
    root: raw.dir.clone(),
    ecosystem: Ecosystem::Npm,
    is_virtual: raw.json.string_at("name").is_none(),
    member_patterns: patterns.iter().filter(|pattern| !pattern.starts_with('!')).cloned().collect(),
    exclude_patterns: patterns.iter().filter_map(|pattern| pattern.strip_prefix('!')).map(str::to_owned).collect(),
    members,
  })
}

fn build_package(raw: &RawPackage, workspaces: &[WorkspaceInfo], ctx: &mut LoadContext<'_>) -> NpmPackage {
  let json = &raw.json;
  let exports = json.get("exports").and_then(|value| match parse_exports(value) {
    Ok(exports) => Some(exports),
    Err(message) => {
      ctx.warn(&raw.manifest, format_args!("invalid exports: {message}"));
      None
    }
  });
  let mut dependencies = Vec::new();
  for (key, section) in [
    ("dependencies", DependencySection::Normal),
    ("devDependencies", DependencySection::Dev),
    ("peerDependencies", DependencySection::Peer),
  ] {
    let Some(entries) = json.get(key).and_then(Json::as_object) else { continue };
    let mut sorted: Vec<(&String, &Json)> = entries.iter().map(|(name, spec)| (name, spec)).collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (name, spec) in sorted {
      let Some(spec) = spec.as_str() else {
        ctx.warn(&raw.manifest, format_args!("dependency {name:?} in {key} has a non-string spec"));
        continue;
      };
      dependencies.push(NpmDependency {
        name: name.clone(),
        section,
        spec: classify_spec(spec),
      });
    }
  }
  let workspace_root = workspaces
    .iter()
    .find(|workspace| workspace.root == raw.dir)
    .or_else(|| {
      workspaces
        .iter()
        .filter(|workspace| workspace.members.contains(&raw.dir))
        .max_by_key(|workspace| workspace.root.depth())
    })
    .map(|workspace| workspace.root.clone());
  NpmPackage {
    name: json.string_at("name"),
    version: json.string_at("version"),
    main: json.string_at("main"),
    module: json.string_at("module"),
    types: json.string_at("types").or_else(|| json.string_at("typings")),
    exports,
    dependencies,
    workspace_root,
  }
}

pub fn classify_spec(spec: &str) -> NpmSpec {
  let trimmed = spec.trim();
  if let Some(path) = trimmed.strip_prefix("file:") {
    return NpmSpec::File(path.to_owned());
  }
  if let Some(path) = trimmed.strip_prefix("link:") {
    return NpmSpec::Link(path.to_owned());
  }
  if let Some(rest) = trimmed.strip_prefix("workspace:") {
    return NpmSpec::Workspace(rest.to_owned());
  }
  if looks_like_semver_range(trimmed) {
    return NpmSpec::Semver(trimmed.to_owned());
  }
  NpmSpec::Other(trimmed.to_owned())
}

fn looks_like_semver_range(spec: &str) -> bool {
  if spec.is_empty() || spec == "*" {
    return true;
  }
  let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '^' | '~' | '<' | '>' | '=' | '|' | '*' | ' ');
  let starts_like_range = spec.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '^' | '~' | '<' | '>' | '=' | '*' | 'v' | 'x' | 'X'));
  starts_like_range && spec.chars().all(allowed) && !spec.chars().all(|c| c.is_ascii_alphabetic())
}

fn parse_exports(value: &Json) -> Result<NpmExports, String> {
  let Json::Object(entries) = value else {
    return Ok(NpmExports::Root(parse_export_target(value)?));
  };
  let subpaths = entries.iter().filter(|(key, _)| key.starts_with('.')).count();
  if subpaths == 0 {
    return Ok(NpmExports::Root(parse_export_target(value)?));
  }
  if subpaths != entries.len() {
    return Err("keys mix subpaths (starting with '.') and conditions".to_owned());
  }
  let parsed: Result<Vec<_>, String> = entries.iter().map(|(key, target)| Ok((key.clone(), parse_export_target(target)?))).collect();
  Ok(NpmExports::Subpaths(parsed?))
}

fn parse_export_target(value: &Json) -> Result<NpmExportTarget, String> {
  match value {
    Json::String(path) => Ok(NpmExportTarget::Path(path.clone())),
    Json::Null => Ok(NpmExportTarget::Blocked),
    Json::Array(items) => Ok(NpmExportTarget::Fallbacks(items.iter().map(parse_export_target).collect::<Result<_, _>>()?)),
    Json::Object(entries) => Ok(NpmExportTarget::Conditions(
      entries
        .iter()
        .map(|(key, target)| Ok((key.clone(), parse_export_target(target)?)))
        .collect::<Result<_, String>>()?,
    )),
    Json::Bool(_) | Json::Number(_) => Err("export targets must be strings, arrays, objects or null".to_owned()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_classify_dependency_specs() {
    let cases = [
      ("file:../shared", NpmSpec::File("../shared".into())),
      ("link:../shared", NpmSpec::Link("../shared".into())),
      ("workspace:*", NpmSpec::Workspace("*".into())),
      ("workspace:^1.2.0", NpmSpec::Workspace("^1.2.0".into())),
      ("^1.2.3", NpmSpec::Semver("^1.2.3".into())),
      ("~0.4", NpmSpec::Semver("~0.4".into())),
      ("1.2.3-beta.1", NpmSpec::Semver("1.2.3-beta.1".into())),
      (">=1.0.0 <2.0.0", NpmSpec::Semver(">=1.0.0 <2.0.0".into())),
      ("1.x || 2.x", NpmSpec::Semver("1.x || 2.x".into())),
      ("*", NpmSpec::Semver("*".into())),
      ("latest", NpmSpec::Other("latest".into())),
      ("next", NpmSpec::Other("next".into())),
      ("github:user/repo", NpmSpec::Other("github:user/repo".into())),
      ("user/repo", NpmSpec::Other("user/repo".into())),
      ("npm:other@^1", NpmSpec::Other("npm:other@^1".into())),
      ("https://example.com/a.tgz", NpmSpec::Other("https://example.com/a.tgz".into())),
    ];
    for (spec, expected) in cases {
      assert_eq!(classify_spec(spec), expected, "{spec}");
    }
  }
}
