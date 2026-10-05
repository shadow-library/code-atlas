use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, RelPath, RepoName, RunId};
use atlas_lsp::{ChangeKind, ChangeList, FileChange, Lease, Settle, SkipReason, SyncReport, canonical_path};
use atlas_store::{ReadTx, RunStatus, Store, StoreError};
use serde_json::Value;

use crate::render::Completeness;
use crate::target::ProjectFiles;

const MAX_LEDGER_RUNS: i64 = 256;
const INDEX_TRIGGERS: [&str; 3] = ["barrier", "index", "edit"];
const PACKAGE_MANIFESTS: [&str; 2] = ["package.json", "Cargo.toml"];
const BUILD_INPUTS: [&str; 3] = ["Cargo.toml", "Cargo.lock", "build.rs"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LedgerChanges {
  pub graphs_changed: Vec<RepoName>,
}

pub(crate) fn change_list(store: &Store, files: &ProjectFiles, since: Option<RunId>, target: Option<RunId>, seen: &mut LedgerChanges) -> ChangeList {
  let (Some(since), Some(target)) = (since, target) else {
    return ChangeList::Unknown;
  };
  if target.0 - since.0 > MAX_LEDGER_RUNS {
    return ChangeList::Unknown;
  }
  let read = store.read(|tx| -> Result<Option<Vec<FileChange>>, StoreError> { ledger_changes(tx, files, since, target, seen) });
  match read {
    Ok(Some(changes)) => ChangeList::Known(changes),
    Ok(None) => ChangeList::Unknown,
    Err(error) => {
      tracing::warn!(%error, since = since.0, target = target.0, "cannot read the run ledger; the language server will be restarted");
      ChangeList::Unknown
    }
  }
}

enum Seen {
  Written,
  Deleted,
}

fn ledger_changes(tx: &ReadTx<'_>, files: &ProjectFiles, since: RunId, target: RunId, ledger: &mut LedgerChanges) -> Result<Option<Vec<FileChange>>, StoreError> {
  let mut seen: BTreeMap<(RepoName, RelPath), Seen> = BTreeMap::new();
  for id in since.0 + 1..=target.0 {
    let Some(run) = tx.run(RunId(id))? else { return Ok(None) };
    if !INDEX_TRIGGERS.contains(&run.trigger.as_str()) {
      continue;
    }
    if !matches!(run.status, RunStatus::Ok | RunStatus::Partial { .. }) {
      return Ok(None);
    }
    let set = &run.report.change_set;
    let (Some(analyzed), Some(deleted), Some(moved)) = (entries(set, "analyzed"), entries(set, "deleted"), entries(set, "moved")) else {
      return Ok(None);
    };
    for label in deleted {
      let Some(key) = file_key(&label) else { return Ok(None) };
      seen.insert(key, Seen::Deleted);
    }
    for label in moved {
      let Some((from, to)) = label.split_once(" -> ").and_then(|(from, to)| Some((file_key(from)?, file_key(to)?))) else {
        return Ok(None);
      };
      seen.insert(from, Seen::Deleted);
      seen.insert(to, Seen::Written);
    }
    for label in analyzed {
      let Some(key) = file_key(&label) else { return Ok(None) };
      seen.insert(key, Seen::Written);
    }
    ledger
      .graphs_changed
      .extend(entries(set, "graphs_changed").unwrap_or_default().iter().filter_map(|repo| RepoName::new(repo).ok()));
  }
  ledger.graphs_changed.sort();
  ledger.graphs_changed.dedup();
  let mut changes = Vec::new();
  for ((repo, path), state) in seen {
    let Some(root) = files.root(&repo) else { continue };
    let absolute = root.join(path.as_str());
    let kind = match (state, tx.file(&repo, &path)?) {
      (Seen::Written, Some(record)) => ChangeKind::Changed(record.content_hash),
      (Seen::Written, None) | (Seen::Deleted, _) => ChangeKind::Deleted,
    };
    changes.push(FileChange::new(absolute, kind));
  }
  Ok(Some(changes))
}

pub(crate) type Manifests = BTreeMap<PathBuf, BTreeMap<PathBuf, ContentHash>>;

pub(crate) fn package_dirs(tx: &ReadTx<'_>, files: &ProjectFiles, roots: &[PathBuf]) -> Result<Vec<PathBuf>, StoreError> {
  let mut dirs: BTreeSet<PathBuf> = roots.iter().cloned().collect();
  for repo in files.repos() {
    let Some(root) = files.root(&repo).filter(|root| roots.iter().any(|unit_root| root.starts_with(unit_root))) else {
      continue;
    };
    let mut candidates = BTreeSet::new();
    for record in tx.files_in_repo(&repo)? {
      let mut dir = Path::new(record.path.as_str()).parent();
      while let Some(current) = dir.filter(|current| !current.as_os_str().is_empty()) {
        if !candidates.insert(current.to_path_buf()) {
          break;
        }
        dir = current.parent();
      }
    }
    dirs.extend(
      candidates
        .into_iter()
        .map(|dir| root.join(dir))
        .filter(|dir| PACKAGE_MANIFESTS.iter().any(|name| dir.join(name).is_file())),
    );
  }
  Ok(dirs.into_iter().collect())
}

pub(crate) fn is_manifest(name: &str) -> bool {
  name == "package.json" || BUILD_INPUTS.contains(&name) || ((name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json"))
}

fn manifests_in(dir: &Path) -> BTreeMap<PathBuf, ContentHash> {
  let listed = |dir: &Path, wanted: &dyn Fn(&str) -> bool| -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
      .flatten()
      .filter(|entry| entry.file_name().to_str().is_some_and(wanted))
      .map(|entry| entry.path())
      .collect()
  };
  let mut paths = listed(dir, &is_manifest);
  paths.extend(listed(&dir.join(".cargo"), &|name| name.starts_with("config")));
  paths
    .into_iter()
    .filter_map(|path| std::fs::read(&path).ok().map(|bytes| (path, ContentHash::of(&bytes))))
    .collect()
}

pub(crate) fn manifests(dirs: &[PathBuf]) -> Manifests {
  dirs.iter().map(|dir| (dir.clone(), manifests_in(dir))).collect()
}

pub(crate) fn manifest_changes(before: &Manifests, now: &Manifests) -> Vec<FileChange> {
  let none = BTreeMap::new();
  let dirs: BTreeSet<&PathBuf> = before.keys().chain(now.keys()).collect();
  let mut changes = Vec::new();
  for dir in dirs {
    let previous = before.get(dir).unwrap_or(&none);
    let current = now.get(dir).unwrap_or(&none);
    for (path, hash) in current {
      if previous.get(path) != Some(hash) {
        changes.push(FileChange::new(path.clone(), ChangeKind::Changed(*hash)));
      }
    }
    for path in previous.keys().filter(|path| !current.contains_key(*path)) {
      changes.push(FileChange::new(path.clone(), ChangeKind::Deleted));
    }
  }
  changes
}

fn entries(change_set: &Value, section: &str) -> Option<Vec<String>> {
  let section = change_set.get(section)?;
  let entries: Vec<String> = section
    .get("entries")?
    .as_array()?
    .iter()
    .map(|entry| entry.as_str().map(str::to_owned))
    .collect::<Option<_>>()?;
  let total = section.get("total")?.as_u64()?;
  (total == entries.len() as u64).then_some(entries)
}

fn file_key(label: &str) -> Option<(RepoName, RelPath)> {
  let (repo, path) = label.split_once(':')?;
  Some((RepoName::new(repo).ok()?, RelPath::new(path).ok()?))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncNotes {
  pub partial: Vec<String>,
  pub caveats: Vec<String>,
  pub warnings: Vec<String>,
  pub drifted: Vec<String>,
}

impl SyncNotes {
  pub(crate) fn of(report: &SyncReport, files: &ProjectFiles) -> Self {
    let mut notes = Self::default();
    match &report.settle {
      Settle::Busy { status } => notes.partial.push(format!("server busy: {status}")),
      Settle::Unconfirmed { paths } => notes.caveats.push(format!(
        "the language server may not have picked up changes to {} (tsserver relies on its own file watchers); results involving them may be stale",
        shown(paths, files)
      )),
      Settle::NotNeeded | Settle::Quiet | Settle::Ready => {}
    }
    if let Some(reason) = &report.restarted {
      notes.warnings.push(format!("the language server was restarted: {reason}"));
    }
    notes.drifted = report.drifted.iter().map(|path| display_path(path, files)).collect();
    if !report.drifted.is_empty() {
      notes.warnings.push(format!(
        "{} changed on disk after indexing; the language server was sent the disk text",
        shown(&report.drifted, files)
      ));
    }
    let skipped: Vec<String> = report
      .skipped
      .iter()
      .filter(|skipped| skipped.reason != SkipReason::UnsupportedLanguage && skipped.reason != SkipReason::Missing)
      .map(|skipped| format!("{} ({})", display_path(&skipped.path, files), skip_text(&skipped.reason)))
      .collect();
    if !skipped.is_empty() {
      notes.warnings.push(format!("not sent to the language server as open documents: {}", skipped.join(", ")));
    }
    notes
  }

  pub(crate) fn lease_state(&mut self, lease: &Lease, files: &ProjectFiles) {
    if let Some(health) = lease.health_error() {
      let partial = format!(
        "{} reports an unhealthy workspace and code-atlas cannot tell whether this file's project loaded ({health})",
        lease.kind().name()
      );
      if !self.partial.contains(&partial) {
        self.partial.push(partial);
      }
    }
    if let Some(reason) = lease.sync_lost() {
      self
        .partial
        .push(format!("document sync was lost ({reason}); the server will be restarted on the next request"));
    }
    let lockfiles = lease.lockfile_changes();
    if !lockfiles.is_empty() {
      self.warnings.push(format!(
        "rust-analyzer rewrote {} since it started (cargo updated the lock file); code-atlas does not restore it",
        shown(&lockfiles, files)
      ));
    }
  }

  pub(crate) fn absorb(&mut self, later: SyncNotes) {
    let lists = [
      (&mut self.partial, later.partial),
      (&mut self.caveats, later.caveats),
      (&mut self.warnings, later.warnings),
      (&mut self.drifted, later.drifted),
    ];
    for (into, from) in lists {
      for note in from {
        if !into.contains(&note) {
          into.push(note);
        }
      }
    }
  }

  pub(crate) fn completeness(&self) -> Completeness {
    match self.partial.is_empty() {
      true => Completeness::Complete,
      false => Completeness::Partial { reason: self.partial.join("; ") },
    }
  }
}

fn skip_text(reason: &SkipReason) -> String {
  match reason {
    SkipReason::UnsupportedLanguage => "not a language of this server".to_owned(),
    SkipReason::Missing => "missing".to_owned(),
    SkipReason::Unreadable(reason) => format!("unreadable: {reason}"),
    SkipReason::NotUtf8 => "not UTF-8".to_owned(),
    SkipReason::OverBudget { bytes } => format!("{bytes} bytes over the overlay budget"),
  }
}

pub(crate) fn display_path(path: &Path, files: &ProjectFiles) -> String {
  let canonical = canonical_path(path).unwrap_or_else(|_| path.to_path_buf());
  files.of_absolute(&canonical).map_or_else(|| path.display().to_string(), |file| file.to_string())
}

fn shown(paths: &[PathBuf], files: &ProjectFiles) -> String {
  const SHOWN: usize = 5;
  let mut names: Vec<String> = paths.iter().take(SHOWN).map(|path| display_path(path, files)).collect();
  if paths.len() > SHOWN {
    names.push(format!("{} more", paths.len() - SHOWN));
  }
  names.join(", ")
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn should_read_complete_change_set_sections_only() {
    let set = json!({
      "analyzed": {"total": 2, "entries": ["api:src/a.ts", "api:src/b.ts"]},
      "deleted": {"total": 1001, "entries": ["api:src/c.ts"]},
    });

    assert_eq!(entries(&set, "analyzed"), Some(vec!["api:src/a.ts".to_owned(), "api:src/b.ts".to_owned()]));
    assert_eq!(entries(&set, "deleted"), None);
    assert_eq!(entries(&set, "moved"), None);
    assert_eq!(entries(&Value::Null, "analyzed"), None);
  }

  #[test]
  fn should_recognise_manifests_and_report_their_changes_across_old_and_new_directories() {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("pkg");
    let fresh = work.path().join("fresh");
    let gone = work.path().join("gone");
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::create_dir_all(&gone).unwrap();
    std::fs::write(gone.join("package.json"), "{}").unwrap();
    std::fs::write(dir.join("tsconfig.build.json"), "{}").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    std::fs::write(dir.join(".cargo/config.toml"), "").unwrap();
    std::fs::write(dir.join("README.md"), "").unwrap();
    let before = manifests(&[dir.clone(), gone]);
    std::fs::write(dir.join("tsconfig.build.json"), "{\"x\": 1}").unwrap();
    std::fs::remove_file(dir.join("Cargo.toml")).unwrap();
    std::fs::write(dir.join("jsconfig.json"), "{}").unwrap();
    std::fs::write(fresh.join("package.json"), "{}").unwrap();

    let changes = manifest_changes(&before, &manifests(&[dir.clone(), fresh]));

    assert_eq!(before[&dir].len(), 3);
    let mut summary: Vec<(String, bool)> = changes
      .iter()
      .map(|change| (change.path.strip_prefix(work.path()).unwrap().display().to_string(), change.kind == ChangeKind::Deleted))
      .collect();
    summary.sort();
    assert_eq!(
      summary,
      vec![
        ("fresh/package.json".to_owned(), false),
        ("gone/package.json".to_owned(), true),
        ("pkg/Cargo.toml".to_owned(), true),
        ("pkg/jsconfig.json".to_owned(), false),
        ("pkg/tsconfig.build.json".to_owned(), false)
      ]
    );
    assert!(is_manifest("tsconfig.app.json") && is_manifest("build.rs") && !is_manifest("tsconfig.txt"));
  }

  #[test]
  fn should_split_change_set_labels_into_repo_and_path() {
    assert_eq!(
      file_key("acme-api:src/users/users.service.ts"),
      Some((RepoName::new("acme-api").unwrap(), RelPath::new("src/users/users.service.ts").unwrap()))
    );
    assert_eq!(file_key("no-separator"), None);
  }
}
