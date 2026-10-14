use std::fs;
use std::path::{Component, Path, PathBuf};

use atlas_core::RelPath;

use super::json::{Json, parse_json, parse_jsonc};
use super::types::{LoadContext, RepoDir, TsConfigInfo, TsPathMapping};

const CONFIG_DIR: &str = "${configDir}";

#[derive(Clone)]
enum ConfigPath {
  Fixed(PathBuf),
  ConfigDir(String),
}

impl ConfigPath {
  fn parse(dir: &Path, value: &str) -> Self {
    match value.strip_prefix(CONFIG_DIR) {
      Some(rest) => Self::ConfigDir(rest.trim_start_matches(['/', '\\']).to_owned()),
      None => Self::Fixed(normalize(&dir.join(value))),
    }
  }
}

#[derive(Default, Clone)]
struct Effective {
  extends: Vec<PathBuf>,
  base_url: Option<PathBuf>,
  paths: Option<(Vec<TsPathMapping>, PathBuf)>,
  root_dir: Option<ConfigPath>,
  out_dir: Option<ConfigPath>,
  declaration_dir: Option<ConfigPath>,
  composite: Option<bool>,
  declaration: Option<bool>,
  emit_declaration_only: Option<bool>,
  no_emit: Option<bool>,
  include: Option<(Vec<String>, PathBuf)>,
}

impl Effective {
  fn overlay(&mut self, upper: Effective) {
    self.extends.extend(upper.extends);
    self.base_url = upper.base_url.or(self.base_url.take());
    self.paths = upper.paths.or(self.paths.take());
    self.root_dir = upper.root_dir.or(self.root_dir.take());
    self.out_dir = upper.out_dir.or(self.out_dir.take());
    self.declaration_dir = upper.declaration_dir.or(self.declaration_dir.take());
    self.composite = upper.composite.or(self.composite.take());
    self.declaration = upper.declaration.or(self.declaration.take());
    self.emit_declaration_only = upper.emit_declaration_only.or(self.emit_declaration_only.take());
    self.no_emit = upper.no_emit.or(self.no_emit.take());
    self.include = upper.include.or(self.include.take());
  }
}

pub(super) fn is_ts_config_name(file_name: &str) -> bool {
  file_name == "jsconfig.json" || (file_name.starts_with("tsconfig") && file_name.ends_with(".json"))
}

pub(super) fn load(ctx: &mut LoadContext<'_>) -> Vec<TsConfigInfo> {
  let configs: Vec<RelPath> = ctx
    .files
    .iter()
    .map(|file| &file.path)
    .filter(|path| is_ts_config_name(path.file_name()))
    .cloned()
    .collect();
  let mut infos = Vec::new();
  for config in configs {
    let absolute = ctx.root.join(config.as_str());
    let mut stack = Vec::new();
    let effective = resolve(ctx, &config, &absolute, &mut stack);
    infos.push(finish(ctx, &config, effective));
  }
  infos
}

fn finish(ctx: &mut LoadContext<'_>, config: &RelPath, effective: Effective) -> TsConfigInfo {
  let to_repo = |ctx: &mut LoadContext<'_>, label: &str, absolute: &Path| -> Option<RepoDir> {
    let dir = repo_dir_of(ctx.root, absolute);
    if dir.is_none() {
      ctx.warn(config, format_args!("{label} {} resolves outside the repository", absolute.display()));
    }
    dir
  };
  let config_dir = RepoDir::of_file(config);
  let to_repo_path = |ctx: &mut LoadContext<'_>, label: &str, path: &ConfigPath| -> Option<RepoDir> {
    match path {
      ConfigPath::Fixed(absolute) => to_repo(ctx, label, absolute),
      ConfigPath::ConfigDir(rest) => {
        let dir = config_dir.resolve(rest);
        if dir.is_none() {
          ctx.warn(config, format_args!("{label} {CONFIG_DIR}/{rest} resolves outside the repository"));
        }
        dir
      }
    }
  };
  let base_url = effective.base_url.as_deref().and_then(|dir| to_repo(ctx, "baseUrl", dir));
  let root_dir = effective.root_dir.as_ref().and_then(|dir| to_repo_path(ctx, "rootDir", dir));
  let out_dir = effective.out_dir.as_ref().and_then(|dir| to_repo_path(ctx, "outDir", dir));
  let declaration_dir = effective.declaration_dir.as_ref().and_then(|dir| to_repo_path(ctx, "declarationDir", dir));
  let (paths, paths_base) = match effective.paths {
    Some((mappings, origin)) if effective.base_url.is_some() => (mappings, base_url.clone().filter(|_| origin.is_absolute())),
    Some((mappings, origin)) => (mappings, repo_dir_of(ctx.root, &origin)),
    None => (Vec::new(), None),
  };
  let include = match effective.include {
    Some((patterns, origin)) => {
      let origin_dir = repo_dir_of(ctx.root, &origin);
      let resolved: Option<Vec<String>> = patterns
        .iter()
        .map(|pattern| match pattern.strip_prefix(CONFIG_DIR) {
          Some(rest) => Some(config_dir.join_pattern(rest.trim_start_matches(['/', '\\']))),
          None => origin_dir.as_ref().map(|dir| dir.join_pattern(pattern.strip_prefix("./").unwrap_or(pattern))),
        })
        .collect();
      resolved.unwrap_or_else(|| {
        ctx.warn(config, format_args!("include declared in {} lies outside the repository", origin.display()));
        Vec::new()
      })
    }
    None => Vec::new(),
  };
  TsConfigInfo {
    path: config.clone(),
    dir: RepoDir::of_file(config),
    extends: effective.extends,
    base_url,
    paths_base,
    paths,
    root_dir,
    include,
    out_dir,
    declaration_dir,
    composite: effective.composite.unwrap_or(false),
    declaration: effective.declaration.unwrap_or(false),
    emit_declaration_only: effective.emit_declaration_only.unwrap_or(false),
    no_emit: effective.no_emit.unwrap_or(false),
  }
}

fn repo_dir_of(root: &Path, absolute: &Path) -> Option<RepoDir> {
  let relative = absolute.strip_prefix(root).ok()?;
  let mut segments = Vec::new();
  for component in relative.components() {
    match component {
      Component::Normal(segment) => segments.push(segment.to_str()?),
      Component::CurDir => {}
      _ => return None,
    }
  }
  RepoDir::root().resolve(&segments.join("/"))
}

fn normalize(path: &Path) -> PathBuf {
  let mut normalized = PathBuf::new();
  for component in path.components() {
    match component {
      Component::ParentDir => {
        normalized.pop();
      }
      Component::CurDir => {}
      other => normalized.push(other.as_os_str()),
    }
  }
  normalized
}

fn resolve(ctx: &mut LoadContext<'_>, report: &RelPath, file: &Path, stack: &mut Vec<PathBuf>) -> Effective {
  let identity = fs::canonicalize(file).unwrap_or_else(|_| normalize(file));
  if stack.contains(&identity) {
    ctx.warn(report, format_args!("extends cycle through {}", identity.display()));
    return Effective::default();
  }
  let text = match fs::read_to_string(&identity) {
    Ok(text) => text,
    Err(error) => {
      ctx.warn(report, format_args!("cannot read {}: {error}", identity.display()));
      return Effective::default();
    }
  };
  let json = match parse_jsonc(&text) {
    Ok(json) => json,
    Err(error) => {
      ctx.warn(report, format_args!("invalid JSONC in {}: {error}", identity.display()));
      return Effective::default();
    }
  };
  let dir = identity.parent().map(Path::to_path_buf).unwrap_or_default();
  stack.push(identity.clone());

  let mut effective = Effective::default();
  for specifier in extends_specifiers(ctx, report, &json) {
    let Some(target) = resolve_extends(&specifier, &dir, ctx.root) else {
      ctx.warn(report, format_args!("extends target {specifier:?} from {} was not found", identity.display()));
      continue;
    };
    let mut parent = resolve(ctx, report, &target, stack);
    parent.extends.insert(0, fs::canonicalize(&target).unwrap_or(target));
    effective.overlay(parent);
  }
  effective.overlay(own_options(ctx, report, &json, &dir));
  stack.pop();
  effective
}

fn extends_specifiers(ctx: &mut LoadContext<'_>, report: &RelPath, json: &Json) -> Vec<String> {
  match json.get("extends") {
    None => Vec::new(),
    Some(Json::String(spec)) => vec![spec.clone()],
    Some(Json::Array(items)) => items.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
    Some(_) => {
      ctx.warn(report, "extends must be a string or an array of strings");
      Vec::new()
    }
  }
}

fn own_options(ctx: &mut LoadContext<'_>, report: &RelPath, json: &Json, dir: &Path) -> Effective {
  let mut own = Effective::default();
  if let Some(options) = json.get("compilerOptions") {
    own.base_url = options.get("baseUrl").and_then(Json::as_str).map(|value| normalize(&dir.join(value)));
    own.root_dir = options.get("rootDir").and_then(Json::as_str).map(|value| ConfigPath::parse(dir, value));
    own.out_dir = options.get("outDir").and_then(Json::as_str).map(|value| ConfigPath::parse(dir, value));
    own.declaration_dir = options.get("declarationDir").and_then(Json::as_str).map(|value| ConfigPath::parse(dir, value));
    own.composite = options.get("composite").and_then(Json::as_bool);
    own.declaration = options.get("declaration").and_then(Json::as_bool);
    own.emit_declaration_only = options.get("emitDeclarationOnly").and_then(Json::as_bool);
    own.no_emit = options.get("noEmit").and_then(Json::as_bool);
    match options.get("paths") {
      None => {}
      Some(Json::Object(entries)) => {
        let mut mappings: Vec<TsPathMapping> = entries
          .iter()
          .map(|(pattern, targets)| TsPathMapping {
            pattern: pattern.clone(),
            targets: targets
              .as_array()
              .map(|items| items.iter().filter_map(Json::as_str).map(str::to_owned).collect())
              .unwrap_or_default(),
          })
          .collect();
        mappings.sort_by(|a, b| a.pattern.cmp(&b.pattern));
        own.paths = Some((mappings, dir.to_path_buf()));
      }
      Some(_) => ctx.warn(report, "compilerOptions.paths must be an object"),
    }
  }
  if let Some(include) = json.get("include").and_then(Json::as_array) {
    own.include = Some((include.iter().filter_map(Json::as_str).map(str::to_owned).collect(), dir.to_path_buf()));
  }
  own
}

fn resolve_extends(specifier: &str, from_dir: &Path, root: &Path) -> Option<PathBuf> {
  let is_relative = specifier == "." || specifier == ".." || specifier.starts_with("./") || specifier.starts_with("../") || Path::new(specifier).is_absolute();
  if is_relative {
    return file_or_json(&from_dir.join(specifier));
  }
  let mut directory = from_dir;
  loop {
    let module = directory.join("node_modules").join(specifier);
    if let Some(found) = file_or_json(&module).or_else(|| package_tsconfig(&module)) {
      return Some(found);
    }
    if directory == root {
      return None;
    }
    directory = directory.parent()?;
  }
}

fn file_or_json(base: &Path) -> Option<PathBuf> {
  if base.is_file() {
    return Some(base.to_path_buf());
  }
  let mut with_extension = base.as_os_str().to_owned();
  with_extension.push(".json");
  let with_extension = PathBuf::from(with_extension);
  with_extension.is_file().then_some(with_extension)
}

fn package_tsconfig(package_dir: &Path) -> Option<PathBuf> {
  if !package_dir.is_dir() {
    return None;
  }
  let declared = fs::read_to_string(package_dir.join("package.json"))
    .ok()
    .and_then(|text| parse_json(&text).ok())
    .and_then(|json| json.string_at("tsconfig"));
  if let Some(relative) = declared
    && let Some(found) = file_or_json(&package_dir.join(relative))
  {
    return Some(found);
  }
  let default = package_dir.join("tsconfig.json");
  default.is_file().then_some(default)
}
