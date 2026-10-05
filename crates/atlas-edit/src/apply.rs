use std::collections::HashMap;
use std::fs::{self, Permissions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_core::ContentHash;
use atlas_project::DEFAULT_EXCLUDES;
use ignore::WalkBuilder;
use tempfile::{Builder, TempPath};

use crate::error::{EditError, EditResult, NotTextReason};
use crate::lock::EditLock;
use crate::plan::{EditPlan, FileEdits, FileTarget, ResourceOp};
use crate::scope::{EditScope, ScopedPath, is_vcs_dir};

pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
pub const TEMP_PREFIX: &str = ".atlas-edit.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOptions {
  pub lock_path: PathBuf,
  pub lock_timeout: Duration,
}

impl ApplyOptions {
  pub fn new(lock_path: impl Into<PathBuf>) -> Self {
    Self {
      lock_path: lock_path.into(),
      lock_timeout: DEFAULT_LOCK_TIMEOUT,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedChange {
  Modified {
    file: FileTarget,
    before: ContentHash,
    after: ContentHash,
    edits: usize,
  },
  Created {
    file: FileTarget,
    after: ContentHash,
  },
  Renamed {
    from: FileTarget,
    to: FileTarget,
  },
  Deleted {
    file: FileTarget,
    before: ContentHash,
  },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
  pub changes: Vec<AppliedChange>,
  pub swept: Vec<PathBuf>,
}

pub fn apply(plan: &EditPlan, scope: &EditScope, options: &ApplyOptions) -> EditResult<ApplyReport> {
  apply_with(plan, scope, options, &NoFaults)
}

pub fn temp_prefix(lock_path: &Path) -> String {
  let canonical = fs::canonicalize(lock_path).unwrap_or_else(|_| lock_path.to_path_buf());
  let id = blake3::hash(canonical.as_os_str().as_encoded_bytes()).to_hex();
  format!("{TEMP_PREFIX}{}.", &id.as_str()[..8])
}

pub fn sweep_stale_temps(scope: &EditScope, options: &ApplyOptions) -> EditResult<Vec<PathBuf>> {
  let mut lock = EditLock::open(&options.lock_path)?;
  let prefix = temp_prefix(&options.lock_path);
  lock.hold(options.lock_timeout, || {
    let mut swept = Vec::new();
    for repo in scope.repos() {
      let filter = repo.filter.clone();
      let walker = WalkBuilder::new(&repo.root)
        .standard_filters(false)
        .follow_links(false)
        .filter_entry(move |entry| {
          let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir());
          let Some(name) = entry.file_name().to_str() else { return true };
          entry.depth() == 0 || !is_dir || !(is_vcs_dir(name) || (DEFAULT_EXCLUDES.contains(&name) && !filter.overrides_default(name)))
        })
        .build();
      let directories = walker
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|file_type| file_type.is_dir()))
        .map(|entry| entry.into_path());
      for directory in directories {
        swept.extend(sweep_directory(&directory, &prefix));
      }
    }
    swept
  })
}

fn sweep_directory(directory: &Path, prefix: &str) -> Vec<PathBuf> {
  let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
  let mut swept = Vec::new();
  for entry in entries.flatten() {
    let is_temp = entry.file_name().to_str().is_some_and(|name| name.starts_with(prefix));
    if !is_temp || !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
      continue;
    }
    let path = entry.path();
    match fs::remove_file(&path) {
      Ok(()) => {
        tracing::info!(path = %path.display(), "removed a temporary file left by an interrupted edit");
        swept.push(path);
      }
      Err(error) => tracing::warn!(path = %path.display(), %error, "cannot remove a temporary file left by an interrupted edit"),
    }
  }
  swept
}

pub(crate) trait Faults {
  fn before_commit(&self, _step: usize) -> io::Result<()> {
    Ok(())
  }

  fn before_restore(&self, _step: usize) -> io::Result<()> {
    Ok(())
  }
}

struct NoFaults;

impl Faults for NoFaults {}

pub(crate) fn apply_with(plan: &EditPlan, scope: &EditScope, options: &ApplyOptions, faults: &dyn Faults) -> EditResult<ApplyReport> {
  plan.validate()?;
  let mut lock = EditLock::open(&options.lock_path)?;
  let prefix = temp_prefix(&options.lock_path);
  lock.hold(options.lock_timeout, || Transaction::prepare(plan, scope, &prefix)?.commit(faults))?
}

struct ContentStep {
  target: ScopedPath,
  before: ContentHash,
  after: ContentHash,
  edits: usize,
  original: Vec<u8>,
  permissions: Permissions,
  temp: TempPath,
}

enum ResourceStep {
  Create { target: ScopedPath, after: ContentHash, temp: TempPath },
  Rename { from: ScopedPath, to: ScopedPath, expected: ContentHash },
  Delete { target: ScopedPath, expected: ContentHash },
}

enum Undo {
  Restore { target: ScopedPath, original: Vec<u8>, permissions: Permissions },
  RemoveCreated { target: ScopedPath },
  Unrename { from: ScopedPath, to: ScopedPath },
  Recreate { target: ScopedPath, original: Vec<u8>, permissions: Permissions },
}

struct Transaction {
  contents: Vec<ContentStep>,
  resources: Vec<ResourceStep>,
  swept: Vec<PathBuf>,
  prefix: String,
}

impl Transaction {
  fn prepare(plan: &EditPlan, scope: &EditScope, prefix: &str) -> EditResult<Self> {
    let plan = canonical_plan(plan, scope)?;
    let mut stale = Vec::new();
    let mut originals = HashMap::new();
    for file in &plan.files {
      let target = locate(scope, &file.file)?;
      ensure_writable(&target)?;
      let original = read_existing(&target)?;
      if ContentHash::of(&original) != file.expected {
        stale.push(file.file.to_string());
      }
      originals.insert(file.file.clone(), (target, original));
    }
    for op in &plan.resources {
      match op {
        ResourceOp::Create { file, .. } => ensure_absent(&locate(scope, file)?)?,
        ResourceOp::Rename { from, to, expected } => {
          ensure_absent(&locate(scope, to)?)?;
          ensure_writable(&locate(scope, from)?)?;
          if !originals.contains_key(from) && ContentHash::of(&read_existing(&locate(scope, from)?)?) != *expected {
            stale.push(from.to_string());
          }
        }
        ResourceOp::Delete { file, expected } => {
          ensure_writable(&locate(scope, file)?)?;
          if ContentHash::of(&read_existing(&locate(scope, file)?)?) != *expected {
            stale.push(file.to_string());
          }
        }
      }
    }
    if !stale.is_empty() {
      return Err(EditError::Stale { paths: stale });
    }
    let swept = sweep_touched_directories(&plan, scope, prefix);

    let mut contents = Vec::new();
    let mut committed_hash = HashMap::new();
    for file in &plan.files {
      let Some((target, original)) = originals.remove(&file.file) else { continue };
      if let Some(step) = prepare_content(file, target, original, prefix)? {
        committed_hash.insert(file.file.clone(), step.after);
        contents.push(step);
      }
    }
    let mut resources = Vec::new();
    for op in &plan.resources {
      resources.push(match op {
        ResourceOp::Create { file, content } => {
          let target = locate(scope, file)?;
          let temp = write_temp(&target.absolute, content.as_bytes(), new_file_permissions().as_ref(), prefix)?;
          ResourceStep::Create {
            target,
            after: ContentHash::of(content.as_bytes()),
            temp,
          }
        }
        ResourceOp::Rename { from, to, expected } => ResourceStep::Rename {
          from: locate(scope, from)?,
          to: locate(scope, to)?,
          expected: committed_hash.get(from).copied().unwrap_or(*expected),
        },
        ResourceOp::Delete { file, expected } => ResourceStep::Delete {
          target: locate(scope, file)?,
          expected: *expected,
        },
      });
    }
    Ok(Self {
      contents,
      resources,
      swept,
      prefix: prefix.to_owned(),
    })
  }

  fn commit(self, faults: &dyn Faults) -> EditResult<ApplyReport> {
    let mut journal = Vec::new();
    let mut report = ApplyReport {
      changes: Vec::new(),
      swept: self.swept,
    };
    let steps = self.contents.into_iter().map(Step::Content).chain(self.resources.into_iter().map(Step::Resource));
    for (index, step) in steps.enumerate() {
      let outcome = faults
        .before_commit(index)
        .map_err(|source| EditError::io("commit", step.path(), source))
        .and_then(|()| step.run(&mut journal));
      match outcome {
        Ok(change) => report.changes.push(change),
        Err(cause) => return Err(rollback(journal, cause, faults, &self.prefix)),
      }
    }
    Ok(report)
  }
}

enum Step {
  Content(ContentStep),
  Resource(ResourceStep),
}

impl Step {
  fn path(&self) -> &Path {
    match self {
      Self::Content(step) => &step.target.absolute,
      Self::Resource(ResourceStep::Create { target, .. } | ResourceStep::Delete { target, .. }) => &target.absolute,
      Self::Resource(ResourceStep::Rename { from, .. }) => &from.absolute,
    }
  }

  fn run(self, journal: &mut Vec<Undo>) -> EditResult<AppliedChange> {
    match self {
      Self::Content(step) => {
        verify_unchanged(&step.target, step.before)?;
        step
          .temp
          .persist(&step.target.absolute)
          .map_err(|error| EditError::io("replace", &step.target.absolute, error.error))?;
        journal.push(Undo::Restore {
          target: step.target.clone(),
          original: step.original,
          permissions: step.permissions,
        });
        Ok(AppliedChange::Modified {
          file: step.target.file,
          before: step.before,
          after: step.after,
          edits: step.edits,
        })
      }
      Self::Resource(ResourceStep::Create { target, after, temp }) => {
        if let Err(failure) = move_no_clobber(&temp, &target.absolute) {
          if failure.destination_left {
            journal.push(Undo::RemoveCreated { target: target.clone() });
          }
          return Err(create_error(&target, failure.error));
        }
        keep(temp);
        journal.push(Undo::RemoveCreated { target: target.clone() });
        Ok(AppliedChange::Created { file: target.file, after })
      }
      Self::Resource(ResourceStep::Rename { from, to, expected }) => {
        verify_unchanged(&from, expected)?;
        if let Err(failure) = move_no_clobber(&from.absolute, &to.absolute) {
          if failure.destination_left {
            journal.push(Undo::RemoveCreated { target: to.clone() });
          }
          return Err(create_error(&to, failure.error));
        }
        journal.push(Undo::Unrename {
          from: from.clone(),
          to: to.clone(),
        });
        Ok(AppliedChange::Renamed { from: from.file, to: to.file })
      }
      Self::Resource(ResourceStep::Delete { target, expected }) => {
        let original = read_existing(&target)?;
        if ContentHash::of(&original) != expected {
          return Err(EditError::Stale {
            paths: vec![target.file.to_string()],
          });
        }
        let permissions = fs::metadata(&target.absolute)
          .map_err(|error| EditError::io("stat", &target.absolute, error))?
          .permissions();
        fs::remove_file(&target.absolute).map_err(|error| EditError::io("delete", &target.absolute, error))?;
        journal.push(Undo::Recreate {
          target: target.clone(),
          original,
          permissions,
        });
        Ok(AppliedChange::Deleted {
          file: target.file,
          before: expected,
        })
      }
    }
  }
}

impl Undo {
  fn label(&self) -> String {
    match self {
      Self::Restore { target, .. } | Self::RemoveCreated { target } | Self::Recreate { target, .. } => target.file.to_string(),
      Self::Unrename { from, to } => format!("{} -> {}", from.file, to.file),
    }
  }

  fn run(&self, prefix: &str) -> EditResult<()> {
    match self {
      Self::Restore { target, original, permissions } => {
        let temp = write_temp(&target.absolute, original, Some(permissions), prefix)?;
        temp.persist(&target.absolute).map_err(|error| EditError::io("restore", &target.absolute, error.error))
      }
      Self::RemoveCreated { target } => fs::remove_file(&target.absolute).map_err(|error| EditError::io("remove created file", &target.absolute, error)),
      Self::Unrename { from, to } => move_no_clobber(&to.absolute, &from.absolute).map_err(|failure| EditError::io("restore renamed file", &from.absolute, failure.error)),
      Self::Recreate { target, original, permissions } => {
        let temp = write_temp(&target.absolute, original, Some(permissions), prefix)?;
        move_no_clobber(&temp, &target.absolute).map_err(|failure| EditError::io("recreate deleted file", &target.absolute, failure.error))?;
        keep(temp);
        Ok(())
      }
    }
  }
}

fn rollback(journal: Vec<Undo>, cause: EditError, faults: &dyn Faults, prefix: &str) -> EditError {
  tracing::warn!(%cause, steps = journal.len(), "edit failed during commit; rolling back");
  let mut applied = Vec::new();
  let mut failed = Vec::new();
  for (index, undo) in journal.iter().rev().enumerate() {
    let outcome = faults
      .before_restore(index)
      .map_err(|source| EditError::io("roll back", PathBuf::from(undo.label()), source))
      .and_then(|()| undo.run(prefix));
    if let Err(error) = outcome {
      tracing::error!(change = %undo.label(), %error, "rollback step failed");
      applied.push(undo.label());
      failed.push(format!("{}: {error}", undo.label()));
    }
  }
  let cause = Box::new(cause);
  if failed.is_empty() {
    EditError::RolledBack { cause }
  } else {
    EditError::PartiallyApplied { cause, applied, failed }
  }
}

fn canonical_plan(plan: &EditPlan, scope: &EditScope) -> EditResult<EditPlan> {
  let mut targets = plan.files.iter().map(|file| &file.file).collect::<Vec<_>>();
  for op in &plan.resources {
    match op {
      ResourceOp::Create { file, .. } | ResourceOp::Delete { file, .. } => targets.push(file),
      ResourceOp::Rename { from, to, .. } => targets.extend([from, to]),
    }
  }
  let resolved = scope.locate_all(targets.iter().copied())?;
  let canonical: HashMap<&FileTarget, &FileTarget> = targets.iter().copied().zip(resolved.iter().map(|path| &path.file)).collect();
  let rename = |target: &FileTarget| canonical.get(target).map_or_else(|| target.clone(), |resolved| (*resolved).clone());
  let canonical_plan = EditPlan {
    files: plan
      .files
      .iter()
      .map(|file| FileEdits {
        file: rename(&file.file),
        expected: file.expected,
        edits: file.edits.clone(),
      })
      .collect(),
    resources: plan
      .resources
      .iter()
      .map(|op| match op {
        ResourceOp::Create { file, content } => ResourceOp::Create {
          file: rename(file),
          content: content.clone(),
        },
        ResourceOp::Rename { from, to, expected } => ResourceOp::Rename {
          from: rename(from),
          to: rename(to),
          expected: *expected,
        },
        ResourceOp::Delete { file, expected } => ResourceOp::Delete {
          file: rename(file),
          expected: *expected,
        },
      })
      .collect(),
  };
  canonical_plan.validate()?;
  Ok(canonical_plan)
}

fn locate(scope: &EditScope, file: &FileTarget) -> EditResult<ScopedPath> {
  scope.locate(file).map_err(|refusal| EditError::OutOfScope { refused: vec![refusal] })
}

fn prepare_content(file: &FileEdits, target: ScopedPath, original: Vec<u8>, prefix: &str) -> EditResult<Option<ContentStep>> {
  let text = std::str::from_utf8(&original).map_err(|_| EditError::NotText {
    path: file.file.to_string(),
    reason: NotTextReason::NotUtf8,
  })?;
  let updated = file.apply_to(text)?;
  if updated.as_bytes() == original.as_slice() {
    tracing::debug!(file = %file.file, "edits leave the file unchanged; skipping it");
    return Ok(None);
  }
  let permissions = fs::metadata(&target.absolute)
    .map_err(|error| EditError::io("stat", &target.absolute, error))?
    .permissions();
  let temp = write_temp(&target.absolute, updated.as_bytes(), Some(&permissions), prefix)?;
  Ok(Some(ContentStep {
    before: file.expected,
    after: ContentHash::of(updated.as_bytes()),
    edits: file.edits.len(),
    target,
    original,
    permissions,
    temp,
  }))
}

fn read_existing(target: &ScopedPath) -> EditResult<Vec<u8>> {
  fs::read(&target.absolute).map_err(|error| match error.kind() {
    ErrorKind::NotFound => EditError::Missing { path: target.file.to_string() },
    _ => EditError::io("read", &target.absolute, error),
  })
}

fn ensure_absent(target: &ScopedPath) -> EditResult<()> {
  match fs::symlink_metadata(&target.absolute) {
    Ok(_) => Err(EditError::AlreadyExists { path: target.file.to_string() }),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
    Err(error) => Err(EditError::io("stat", &target.absolute, error)),
  }
}

fn verify_unchanged(target: &ScopedPath, expected: ContentHash) -> EditResult<()> {
  if ContentHash::of(&read_existing(target)?) == expected {
    return Ok(());
  }
  Err(EditError::Stale {
    paths: vec![target.file.to_string()],
  })
}

fn ensure_writable(target: &ScopedPath) -> EditResult<()> {
  match fs::metadata(&target.absolute) {
    Ok(metadata) if metadata.permissions().readonly() => Err(EditError::ReadOnly { path: target.file.to_string() }),
    _ => Ok(()),
  }
}

fn sweep_touched_directories(plan: &EditPlan, scope: &EditScope, prefix: &str) -> Vec<PathBuf> {
  let mut targets: Vec<&FileTarget> = plan.files.iter().map(|file| &file.file).collect();
  for op in &plan.resources {
    match op {
      ResourceOp::Create { file, .. } | ResourceOp::Delete { file, .. } => targets.push(file),
      ResourceOp::Rename { from, to, .. } => targets.extend([from, to]),
    }
  }
  let mut directories: Vec<PathBuf> = targets
    .into_iter()
    .filter_map(|target| scope.locate(target).ok())
    .filter_map(|located| located.absolute.parent().map(Path::to_path_buf))
    .collect();
  directories.sort();
  directories.dedup();
  directories.iter().flat_map(|directory| sweep_directory(directory, prefix)).collect()
}

fn keep(temp: TempPath) {
  if let Err(error) = temp.keep() {
    tracing::debug!(error = %error.error, "temporary path already moved");
  }
}

struct MoveFailure {
  error: io::Error,
  destination_left: bool,
}

impl From<io::Error> for MoveFailure {
  fn from(error: io::Error) -> Self {
    Self { error, destination_left: false }
  }
}

fn move_no_clobber(from: &Path, to: &Path) -> Result<(), MoveFailure> {
  #[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android"))]
  match rustix::fs::renameat_with(rustix::fs::CWD, from, rustix::fs::CWD, to, rustix::fs::RenameFlags::NOREPLACE) {
    Ok(()) => return Ok(()),
    Err(errno) if is_unsupported(errno) => {}
    Err(errno) => return Err(io::Error::from(errno).into()),
  }
  match fs::hard_link(from, to) {
    Ok(()) => return unlink_source(from, to, &|path| fs::remove_file(path)),
    Err(error) if !rustix::io::Errno::from_io_error(&error).is_some_and(is_unsupported) => return Err(error.into()),
    Err(_) => {}
  }
  tracing::debug!(from = %from.display(), to = %to.display(), "no atomic no-replace move on this filesystem; checking the destination under the edit lock");
  match fs::symlink_metadata(to) {
    Ok(_) => Err(io::Error::from(ErrorKind::AlreadyExists).into()),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(fs::rename(from, to)?),
    Err(error) => Err(error.into()),
  }
}

fn unlink_source(from: &Path, to: &Path, remove: &dyn Fn(&Path) -> io::Result<()>) -> Result<(), MoveFailure> {
  let Err(error) = remove(from) else { return Ok(()) };
  let destination_left = remove(to)
    .inspect_err(|cleanup| tracing::error!(path = %to.display(), error = %cleanup, "cannot remove the new name after a failed move"))
    .is_err();
  Err(MoveFailure { error, destination_left })
}

fn is_unsupported(errno: rustix::io::Errno) -> bool {
  use rustix::io::Errno;
  [Errno::NOSYS, Errno::INVAL, Errno::NOTSUP, Errno::OPNOTSUPP, Errno::PERM].contains(&errno)
}

fn create_error(target: &ScopedPath, error: io::Error) -> EditError {
  match error.kind() {
    ErrorKind::AlreadyExists => EditError::AlreadyExists { path: target.file.to_string() },
    _ => EditError::io("create", &target.absolute, error),
  }
}

fn write_temp(path: &Path, bytes: &[u8], permissions: Option<&Permissions>, prefix: &str) -> EditResult<TempPath> {
  let directory = path.parent().unwrap_or(Path::new("."));
  let mut temp = Builder::new()
    .prefix(prefix)
    .tempfile_in(directory)
    .map_err(|error| EditError::io("create temporary file in", directory, error))?;
  temp.write_all(bytes).map_err(|error| EditError::io("write temporary file", temp.path(), error))?;
  if let Some(permissions) = permissions {
    temp
      .as_file()
      .set_permissions(permissions.clone())
      .map_err(|error| EditError::io("set permissions on", temp.path(), error))?;
  }
  temp.as_file().sync_all().map_err(|error| EditError::io("sync", temp.path(), error))?;
  Ok(temp.into_temp_path())
}

#[cfg(unix)]
fn new_file_permissions() -> Option<Permissions> {
  Some(std::os::unix::fs::PermissionsExt::from_mode(0o644))
}

#[cfg(not(unix))]
fn new_file_permissions() -> Option<Permissions> {
  None
}

#[cfg(test)]
mod tests;
