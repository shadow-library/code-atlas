use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::ProjectName;
use atlas_project::{ProjectError, RepoSpec, slugify};

use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit_line};

const MARKER_FILE: &str = ".code-atlas.toml";

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let root = repo_root(&ctx.cwd)?;
  let marker = root.join(MARKER_FILE);
  let pinned = read_marker(&marker)?;
  if let (Some(existing), Some(requested)) = (&pinned, &ctx.project)
    && existing != requested
  {
    return Err(CliError::MarkerConflict {
      path: marker,
      existing: existing.clone(),
      requested: requested.clone(),
    });
  }
  let name = match ctx.project.clone().or(pinned.clone()) {
    Some(name) => name,
    None => derive_name(&root)?,
  };
  ProjectName::new(name.as_str())?;
  let mut registry = ctx.registry()?;
  let registration = match registry.project(&name) {
    Ok(project) if project.repos.iter().any(|repo| repo.path == root) => "repo already registered",
    Ok(_) => {
      registry.add_repo(&name, RepoSpec::new(&root))?;
      "repo added"
    }
    Err(ProjectError::ProjectNotFound(_)) => {
      registry.create_project(&name, vec![RepoSpec::new(&root)])?;
      "project created"
    }
    Err(error) => return Err(error.into()),
  };
  let pin = if pinned.as_deref() == Some(name.as_str()) {
    "unchanged"
  } else {
    fs::write(&marker, format!("project = \"{name}\"\n")).map_err(|source| CliError::io("write", &marker, source))?;
    "written"
  };
  emit_line(format!("project {name}: {registration} ({})", root.display()))?;
  emit_line(format!("{}: {pin}", marker.display()))?;
  Ok(Outcome::Complete)
}

pub fn repo_root(cwd: &Path) -> CliResult<PathBuf> {
  let canonical = fs::canonicalize(cwd).map_err(|source| CliError::io("canonicalize", cwd, source))?;
  let root = canonical.ancestors().find(|directory| directory.join(".git").exists()).unwrap_or(&canonical);
  Ok(root.to_path_buf())
}

fn derive_name(root: &Path) -> CliResult<String> {
  root
    .file_name()
    .and_then(|name| name.to_str())
    .and_then(slugify)
    .ok_or_else(|| CliError::ProjectNameUnderivable(root.to_path_buf()))
}

fn read_marker(marker: &Path) -> CliResult<Option<String>> {
  let text = match fs::read_to_string(marker) {
    Ok(text) => text,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(source) => return Err(CliError::io("read", marker, source)),
  };
  let table: toml::Table = toml::from_str(&text).map_err(|error| CliError::InvalidConfig {
    path: marker.to_path_buf(),
    message: error.to_string(),
  })?;
  let project = table.get("project").and_then(toml::Value::as_str).map(str::to_owned);
  project.map(Some).ok_or_else(|| CliError::InvalidConfig {
    path: marker.to_path_buf(),
    message: "missing string key `project`".to_owned(),
  })
}
