use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::ProjectName;
use atlas_project::{MARKER_FILE, ProjectError, RepoFile, RepoSpec, slugify, write_marker_project};

use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit_line};

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let root = repo_root(&ctx.cwd)?;
  let marker = root.join(MARKER_FILE);
  let pinned = RepoFile::read(&marker)?.and_then(|file| file.project);
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
    write_marker_project(&marker, &name)?;
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
