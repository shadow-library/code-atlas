use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use atlas_core::{RelPath, RepoName};
use atlas_project::{DEFAULT_EXCLUDES, PathGlobs, Project, ProjectError, RepoConfig, WalkFilter};
use ignore::Match;
use ignore::gitignore::Gitignore;

use crate::error::{EditError, EditResult};
use crate::plan::FileTarget;

pub(crate) const VCS_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];
const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
  NotAbsolute,
  InvalidPath,
  NotUtf8,
  MissingParent,
  OutsideRepos,
  UnknownRepo,
  BackslashInName,
  VcsMetadata(String),
  DefaultExcluded(String),
  Symlink,
  Gitignored,
  NotAFile,
  Unreadable(String),
}

impl fmt::Display for RefusalReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::NotAbsolute => f.write_str("not an absolute path"),
      Self::InvalidPath => f.write_str("not a file path"),
      Self::NotUtf8 => f.write_str("not valid UTF-8"),
      Self::MissingParent => f.write_str("its directory does not exist"),
      Self::OutsideRepos => f.write_str("outside every registered repo"),
      Self::UnknownRepo => f.write_str("unknown repo"),
      Self::BackslashInName => f.write_str("a backslash in a file name"),
      Self::VcsMetadata(dir) => write!(f, "inside version-control metadata {dir}"),
      Self::DefaultExcluded(dir) => write!(f, "inside excluded directory {dir}"),
      Self::Symlink => f.write_str("a symlink"),
      Self::Gitignored => f.write_str("gitignored"),
      Self::NotAFile => f.write_str("not a regular file"),
      Self::Unreadable(message) => write!(f, "unreadable: {message}"),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
  pub path: PathBuf,
  pub reason: RefusalReason,
}

impl fmt::Display for Refusal {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{} ({})", self.path.display(), self.reason)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedPath {
  pub file: FileTarget,
  pub absolute: PathBuf,
}

pub(crate) fn globs<S: AsRef<str>>(patterns: &[S]) -> EditResult<PathGlobs> {
  PathGlobs::new(patterns).map_err(project_error)
}

pub(crate) fn project_error(error: ProjectError) -> EditError {
  match error {
    ProjectError::InvalidGlob { pattern, message } => EditError::InvalidGlob { pattern, message },
    other => EditError::Project(other),
  }
}

pub(crate) fn is_vcs_dir(name: &str) -> bool {
  VCS_DIRS.iter().any(|vcs| vcs.eq_ignore_ascii_case(name))
}

pub(crate) struct ScopeRepo {
  pub(crate) name: RepoName,
  pub(crate) root: PathBuf,
  pub(crate) include: PathGlobs,
  pub(crate) filter: WalkFilter,
}

impl ScopeRepo {
  fn new(config: &RepoConfig) -> EditResult<Option<Self>> {
    let root = match fs::canonicalize(&config.path) {
      Ok(root) => root,
      Err(error) => {
        tracing::warn!(repo = %config.name, path = %config.path.display(), %error, "repo root is unavailable; its files cannot be edited");
        return Ok(None);
      }
    };
    Ok(Some(Self {
      name: config.name.clone(),
      include: globs(&config.include)?,
      filter: WalkFilter::new(root.clone(), &config.include, &config.exclude).map_err(project_error)?,
      root,
    }))
  }

  fn excluded_directory<'a>(&self, directories: &[&'a str], relative: &str) -> Option<&'a str> {
    directories
      .iter()
      .find(|name| DEFAULT_EXCLUDES.contains(name) && !(self.filter.overrides_default(name) && self.include.matches_within(relative, false)))
      .copied()
  }

  fn is_gitignored(&self, global: &Gitignore, relative: &str) -> bool {
    let segments: Vec<&str> = relative.split('/').collect();
    let ancestors = (1..segments.len()).map(|depth| (segments[..depth].join("/"), true));
    for (path, is_dir) in ancestors.chain(std::iter::once((relative.to_owned(), false))) {
      if self.ignored_by_stack(global, &path, is_dir) {
        return true;
      }
    }
    false
  }

  fn ignored_by_stack(&self, global: &Gitignore, path: &str, is_dir: bool) -> bool {
    let mut ignored = verdict(global.matched(path, is_dir), false);
    let exclude = self.root.join(".git").join("info").join("exclude");
    if exclude.is_file() {
      ignored = verdict(load_ignore(&exclude).matched(path, is_dir), ignored);
    }
    let segments: Vec<&str> = path.split('/').collect();
    for depth in 0..segments.len() {
      let directory = segments[..depth].join("/");
      let below = segments[depth..].join("/");
      for name in IGNORE_FILES {
        let file = if directory.is_empty() {
          self.root.join(name)
        } else {
          self.root.join(&directory).join(name)
        };
        if file.is_file() {
          ignored = verdict(load_ignore(&file).matched(&below, is_dir), ignored);
        }
      }
    }
    ignored
  }
}

fn verdict<T>(matched: Match<T>, previous: bool) -> bool {
  match matched {
    Match::None => previous,
    Match::Ignore(_) => true,
    Match::Whitelist(_) => false,
  }
}

fn load_ignore(path: &Path) -> Gitignore {
  let (matcher, error) = Gitignore::new(path);
  if let Some(error) = error {
    tracing::debug!(path = %path.display(), %error, "ignore file is partially invalid");
  }
  matcher
}

pub struct EditScope {
  repos: Vec<ScopeRepo>,
  global_ignore: Gitignore,
  max_file_bytes: u64,
}

impl EditScope {
  pub fn new(project: &Project) -> EditResult<Self> {
    let mut repos = Vec::with_capacity(project.repos.len());
    for config in &project.repos {
      if let Some(repo) = ScopeRepo::new(config)? {
        repos.push(repo);
      }
    }
    let (global_ignore, error) = Gitignore::global();
    if let Some(error) = error {
      tracing::debug!(%error, "global gitignore is partially invalid");
    }
    Ok(Self {
      repos,
      global_ignore,
      max_file_bytes: project.settings.max_file_bytes,
    })
  }

  pub(crate) fn repos(&self) -> &[ScopeRepo] {
    &self.repos
  }

  pub(crate) fn max_file_bytes(&self) -> u64 {
    self.max_file_bytes
  }

  pub fn repo_root(&self, repo: &RepoName) -> Option<&Path> {
    self.repos.iter().find(|candidate| &candidate.name == repo).map(|candidate| candidate.root.as_path())
  }

  pub fn locate(&self, file: &FileTarget) -> Result<ScopedPath, Refusal> {
    let Some(root) = self.repo_root(&file.repo) else {
      return Err(Refusal {
        path: PathBuf::from(file.to_string()),
        reason: RefusalReason::UnknownRepo,
      });
    };
    self.resolve(&root.join(file.path.as_str()))
  }

  pub fn locate_all<'a>(&self, files: impl IntoIterator<Item = &'a FileTarget>) -> EditResult<Vec<ScopedPath>> {
    collect(files.into_iter().map(|file| self.locate(file)))
  }

  pub fn resolve_all<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) -> EditResult<Vec<ScopedPath>> {
    collect(paths.into_iter().map(|path| self.resolve(path)))
  }

  pub fn resolve(&self, path: &Path) -> Result<ScopedPath, Refusal> {
    let refuse = |reason| Refusal { path: path.to_path_buf(), reason };
    if !path.is_absolute() {
      return Err(refuse(RefusalReason::NotAbsolute));
    }
    let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
      return Err(refuse(RefusalReason::InvalidPath));
    };
    let parent = fs::canonicalize(parent).map_err(|_| refuse(RefusalReason::MissingParent))?;
    let mut absolute = parent.join(name);
    match fs::symlink_metadata(&absolute) {
      Ok(metadata) if metadata.file_type().is_symlink() => return Err(refuse(RefusalReason::Symlink)),
      Ok(_) => absolute = fs::canonicalize(&absolute).map_err(|error| refuse(RefusalReason::Unreadable(error.to_string())))?,
      Err(error) if error.kind() == ErrorKind::NotFound => {}
      Err(error) => return Err(refuse(RefusalReason::Unreadable(error.to_string()))),
    }
    let Some(repo) = self
      .repos
      .iter()
      .filter(|repo| absolute.starts_with(&repo.root))
      .max_by_key(|repo| repo.root.components().count())
    else {
      return Err(refuse(RefusalReason::OutsideRepos));
    };
    let Ok(relative) = absolute.strip_prefix(&repo.root) else {
      return Err(refuse(RefusalReason::OutsideRepos));
    };
    let Some(relative) = relative.to_str() else {
      return Err(refuse(RefusalReason::NotUtf8));
    };
    if relative.contains('\\') {
      return Err(refuse(RefusalReason::BackslashInName));
    }
    let relative = RelPath::new(relative).map_err(|_| refuse(RefusalReason::NotAFile))?;
    let segments: Vec<&str> = relative.segments().collect();
    if let Some(vcs) = segments.iter().find(|segment| is_vcs_dir(segment)) {
      return Err(refuse(RefusalReason::VcsMetadata((*vcs).to_owned())));
    }
    if let Some(excluded) = repo.excluded_directory(&segments[..segments.len() - 1], relative.as_str()) {
      return Err(refuse(RefusalReason::DefaultExcluded(excluded.to_owned())));
    }
    if fs::metadata(&absolute).is_ok_and(|metadata| !metadata.is_file()) {
      return Err(refuse(RefusalReason::NotAFile));
    }
    if repo.is_gitignored(&self.global_ignore, relative.as_str()) {
      return Err(refuse(RefusalReason::Gitignored));
    }
    Ok(ScopedPath {
      file: FileTarget::new(repo.name.clone(), relative),
      absolute,
    })
  }
}

fn collect(results: impl Iterator<Item = Result<ScopedPath, Refusal>>) -> EditResult<Vec<ScopedPath>> {
  let mut resolved = Vec::new();
  let mut refused = Vec::new();
  for result in results {
    match result {
      Ok(path) => resolved.push(path),
      Err(refusal) => refused.push(refusal),
    }
  }
  if refused.is_empty() { Ok(resolved) } else { Err(EditError::OutOfScope { refused }) }
}
