use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::target::Target;
use crate::error::{CliError, CliResult};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Agent {
  Claude,
  Codex,
}

impl Agent {
  pub(super) fn name(self) -> &'static str {
    match self {
      Self::Claude => "claude",
      Self::Codex => "codex",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
  User,
  Local,
  Project,
}

impl Scope {
  pub(super) fn name(self) -> &'static str {
    match self {
      Self::User => "user",
      Self::Local => "local",
      Self::Project => "project",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Record {
  pub(super) agent: Agent,
  pub(super) scope: Scope,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub(super) repo: Option<PathBuf>,
  pub(super) target: Target,
  pub(super) hash: String,
  #[serde(default, skip_serializing_if = "std::ops::Not::not")]
  pub(super) created_file: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Document {
  version: u32,
  entries: Vec<Record>,
}

pub(super) struct Manifest {
  path: PathBuf,
  pub(super) records: Vec<Record>,
}

impl Manifest {
  pub(super) fn locked<T>(path: PathBuf, work: impl FnOnce(&mut Manifest) -> CliResult<T>) -> CliResult<T> {
    let directory = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory).map_err(|source| CliError::io("create", directory, source))?;
    let lock_path = path.with_extension("json.lock");
    let file: File = OpenOptions::new()
      .create(true)
      .truncate(false)
      .write(true)
      .open(&lock_path)
      .map_err(|source| CliError::io("open", &lock_path, source))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write().map_err(|source| CliError::io("lock", &lock_path, source))?;
    let records = Self::read(&path)?;
    work(&mut Manifest { path, records })
  }

  pub(super) fn read(path: &Path) -> CliResult<Vec<Record>> {
    let text = match fs::read_to_string(path) {
      Ok(text) => text,
      Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
      Err(source) => return Err(CliError::io("read", path, source)),
    };
    let invalid = |message: String| CliError::InvalidManifest {
      path: path.to_path_buf(),
      message,
    };
    let document: Document = serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?;
    if document.version != VERSION {
      return Err(invalid(format!("unsupported version {}; this catl reads version {VERSION}", document.version)));
    }
    Ok(document.entries)
  }

  pub(super) fn find(&self, target: &Target) -> Option<&Record> {
    self.records.iter().find(|record| &record.target == target)
  }

  pub(super) fn put(&mut self, record: Record) -> CliResult<()> {
    match self.records.iter_mut().find(|existing| existing.target == record.target) {
      Some(existing) => *existing = record,
      None => self.records.push(record),
    }
    self.save()
  }

  pub(super) fn forget(&mut self, target: &Target) -> CliResult<()> {
    self.records.retain(|record| &record.target != target);
    self.save()
  }

  fn save(&self) -> CliResult<()> {
    let document = Document {
      version: VERSION,
      entries: self.records.clone(),
    };
    let mut text = serde_json::to_string_pretty(&document)?;
    text.push('\n');
    let directory = self.path.parent().unwrap_or(Path::new("."));
    let io = |source| CliError::io("write", &self.path, source);
    let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(io)?;
    temp.write_all(text.as_bytes()).map_err(io)?;
    temp.persist(&self.path).map_err(|error| io(error.error))?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn record(path: &str) -> Record {
    Record {
      agent: Agent::Claude,
      scope: Scope::Local,
      repo: Some(PathBuf::from("/r")),
      target: Target::Guidance { path: PathBuf::from(path) },
      hash: "blake3:00".to_owned(),
      created_file: true,
    }
  }

  #[test]
  fn should_persist_every_change_and_replace_records_by_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data/wiring.json");
    Manifest::locked(path.clone(), |manifest| {
      manifest.put(record("/r/CLAUDE.local.md"))?;
      manifest.put(Record {
        hash: "blake3:11".to_owned(),
        ..record("/r/CLAUDE.local.md")
      })?;
      manifest.put(record("/r/AGENTS.md"))?;
      manifest.forget(&Target::Guidance {
        path: PathBuf::from("/r/AGENTS.md"),
      })
    })
    .unwrap();

    let records = Manifest::read(&path).unwrap();
    assert_eq!(
      records,
      [Record {
        hash: "blake3:11".to_owned(),
        ..record("/r/CLAUDE.local.md")
      }]
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("{\n  \"version\": 1,\n  \"entries\": [\n"), "{text}");
    assert!(text.contains("\"kind\": \"guidance\""), "{text}");
  }

  #[test]
  fn should_refuse_a_manifest_it_cannot_read_instead_of_starting_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wiring.json");
    fs::write(&path, "{\"version\": 2, \"entries\": []}").unwrap();
    let error = Manifest::locked(path.clone(), |_| Ok(())).unwrap_err();
    assert_eq!(
      error.to_string(),
      format!("invalid wiring manifest {}: unsupported version 2; this catl reads version 1", path.display())
    );
  }
}
