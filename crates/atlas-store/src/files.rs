use atlas_core::{ContentHash, Language, ParseStatus, Producer, RelPath, RepoName, RunId};
use rusqlite::{OptionalExtension, Row, params};

use crate::codec::{from_json, from_sql_int, to_json, to_sql_int};
use crate::error::{StoreError, StoreResult};
use crate::tx::{ReadTx, WriteTx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
  pub repo: RepoName,
  pub path: RelPath,
  pub language: Option<Language>,
  pub content_hash: ContentHash,
  pub size: u64,
  pub mtime_ns: i64,
  pub inode: Option<u64>,
  pub parse_status: ParseStatus,
  pub analyzer: Option<Producer>,
  pub indexed_run: Option<RunId>,
}

macro_rules! file_columns {
  () => {
    "repo, path, language, content_hash, size, mtime_ns, inode, parse_status, analyzer_name, analyzer_version, indexed_run"
  };
}

struct RawFile {
  repo: String,
  path: String,
  language: Option<String>,
  content_hash: String,
  size: i64,
  mtime_ns: i64,
  inode: Option<i64>,
  parse_status: String,
  analyzer_name: Option<String>,
  analyzer_version: Option<i64>,
  indexed_run: Option<i64>,
}

impl RawFile {
  fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      repo: row.get(0)?,
      path: row.get(1)?,
      language: row.get(2)?,
      content_hash: row.get(3)?,
      size: row.get(4)?,
      mtime_ns: row.get(5)?,
      inode: row.get(6)?,
      parse_status: row.get(7)?,
      analyzer_name: row.get(8)?,
      analyzer_version: row.get(9)?,
      indexed_run: row.get(10)?,
    })
  }

  fn into_record(self) -> StoreResult<FileRecord> {
    let analyzer = match (self.analyzer_name, self.analyzer_version) {
      (Some(name), Some(version)) => {
        let version = u32::try_from(version).map_err(|_| StoreError::corrupt("analyzer version", version.to_string()))?;
        Some(Producer::new(name, version))
      }
      (None, None) => None,
      (name, version) => return Err(StoreError::corrupt("analyzer", format!("name {name:?} without version {version:?}"))),
    };
    Ok(FileRecord {
      repo: RepoName::new(self.repo)?,
      path: RelPath::new(&self.path)?,
      language: self.language.map(|language| language.parse()).transpose()?,
      content_hash: self.content_hash.parse()?,
      size: from_sql_int("file size", self.size)?,
      mtime_ns: self.mtime_ns,
      inode: self.inode.map(|inode| u64::from_le_bytes(inode.to_le_bytes())),
      parse_status: from_json("parse status", &self.parse_status)?,
      analyzer,
      indexed_run: self.indexed_run.map(RunId),
    })
  }
}

impl WriteTx<'_> {
  pub fn upsert_file(&mut self, file: &FileRecord) -> StoreResult<()> {
    self
      .conn()
      .prepare_cached(concat!(
        "INSERT INTO files (",
        file_columns!(),
        ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(repo, path) DO UPDATE SET language = excluded.language, content_hash = excluded.content_hash, size = excluded.size, \
         mtime_ns = excluded.mtime_ns, inode = excluded.inode, parse_status = excluded.parse_status, analyzer_name = excluded.analyzer_name, \
         analyzer_version = excluded.analyzer_version, indexed_run = excluded.indexed_run"
      ))?
      .execute(params![
        file.repo.as_str(),
        file.path.as_str(),
        file.language.map(Language::as_str),
        file.content_hash.to_string(),
        to_sql_int("file size", file.size)?,
        file.mtime_ns,
        file.inode.map(|inode| i64::from_le_bytes(inode.to_le_bytes())),
        to_json(&file.parse_status)?,
        file.analyzer.as_ref().map(|producer| producer.name.as_str()),
        file.analyzer.as_ref().map(|producer| i64::from(producer.version)),
        file.indexed_run.map(|run| run.0),
      ])?;
    Ok(())
  }

  pub fn delete_file(&mut self, repo: &RepoName, path: &RelPath) -> StoreResult<bool> {
    let deleted = self
      .conn()
      .prepare_cached("DELETE FROM files WHERE repo = ?1 AND path = ?2")?
      .execute(params![repo.as_str(), path.as_str()])?;
    Ok(deleted == 1)
  }
}

impl ReadTx<'_> {
  pub fn file(&self, repo: &RepoName, path: &RelPath) -> StoreResult<Option<FileRecord>> {
    let raw = self
      .conn()
      .prepare_cached(concat!("SELECT ", file_columns!(), " FROM files WHERE repo = ?1 AND path = ?2"))?
      .query_row(params![repo.as_str(), path.as_str()], RawFile::from_row)
      .optional()?;
    raw.map(RawFile::into_record).transpose()
  }

  pub fn files_in_repo(&self, repo: &RepoName) -> StoreResult<Vec<FileRecord>> {
    let mut statement = self
      .conn()
      .prepare_cached(concat!("SELECT ", file_columns!(), " FROM files WHERE repo = ?1 ORDER BY path"))?;
    let rows = statement.query_map([repo.as_str()], RawFile::from_row)?;
    rows.map(|raw| raw?.into_record()).collect()
  }
}
