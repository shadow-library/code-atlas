use std::fs;
use std::path::Path;

use atlas_core::RelPath;
use ignore::{DirEntry, WalkBuilder};

use crate::discovery::{WalkFilter, mtime_ns};
use crate::error::ProjectError;
use crate::globs::PathGlobs;
use crate::registry::RepoConfig;

pub const NOTES_DIR: &str = ".code-atlas/notes";
pub const MAX_DOC_BYTES: u64 = 512 * 1024;
const MARKER_DIR: &str = ".code-atlas";
const NOTES_NAME: &str = "notes";
const DOC_EXTENSIONS: [&str; 2] = ["md", "mdx"];
const CHANGELOG_STEM: &str = "changelog";
const ALWAYS_EXCLUDED_DIR: &str = "node_modules";
const EXCLUDED_SEGMENTS: [&str; 2] = ["tests", "fixtures"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocScope {
  All,
  Notes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDoc {
  pub path: RelPath,
  pub size: u64,
  pub mtime_ns: i64,
  pub note: bool,
}

pub fn is_note_path(path: &str) -> bool {
  path.strip_prefix(NOTES_DIR).is_some_and(|rest| rest.starts_with('/'))
}

pub fn is_doc_name(file_name: &str) -> bool {
  file_name
    .rsplit_once('.')
    .is_some_and(|(stem, extension)| !stem.is_empty() && DOC_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

pub fn discover_docs(repo: &RepoConfig, scope: DocScope, max_file_bytes: u64) -> Result<Vec<DiscoveredDoc>, ProjectError> {
  fs::metadata(&repo.path).map_err(|source| ProjectError::io("read repository root", &repo.path, source))?;
  let include = PathGlobs::new(&repo.include)?;
  let filter = WalkFilter::new(repo.path.clone(), &repo.include, &repo.exclude)?;
  let walker = WalkBuilder::new(&repo.path)
    .hidden(false)
    .ignore(true)
    .git_ignore(true)
    .git_global(true)
    .git_exclude(true)
    .parents(false)
    .require_git(false)
    .follow_links(false)
    .filter_entry(move |entry| admits(entry, scope) && filter.admits(entry))
    .build();
  let limit = max_file_bytes.min(MAX_DOC_BYTES);
  let mut docs: Vec<DiscoveredDoc> = walker.flatten().filter_map(|entry| classify(&entry, repo, &include, limit)).collect();
  docs.sort_by(|left, right| left.path.cmp(&right.path));
  Ok(docs)
}

fn admits(entry: &DirEntry, scope: DocScope) -> bool {
  if entry.depth() == 0 {
    return true;
  }
  let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir());
  let Some(name) = entry.file_name().to_str() else { return false };
  let in_marker = entry
    .path()
    .parent()
    .and_then(Path::file_name)
    .is_some_and(|parent| parent == MARKER_DIR && entry.depth() == 2);
  match entry.depth() {
    1 if name == MARKER_DIR => is_dir,
    1 if scope == DocScope::Notes => false,
    2 if in_marker => is_dir && name == NOTES_NAME,
    _ => !name.starts_with('.') && !(is_dir && name == ALWAYS_EXCLUDED_DIR),
  }
}

fn classify(entry: &DirEntry, repo: &RepoConfig, include: &PathGlobs, limit: u64) -> Option<DiscoveredDoc> {
  let file_type = entry.file_type()?;
  if !file_type.is_file() || entry.depth() == 0 {
    return None;
  }
  let relative = entry.path().strip_prefix(&repo.path).ok()?.to_str()?;
  let name = relative.rsplit('/').next().unwrap_or(relative);
  if !is_doc_name(name) {
    return None;
  }
  let note = is_note_path(relative);
  if !note && excluded_by_name(relative, name) {
    return None;
  }
  if !note && !include.is_empty() && !include.matches_within(relative, false) {
    return None;
  }
  let path = RelPath::new(relative).ok()?;
  let metadata = entry.metadata().ok()?;
  (metadata.len() <= limit).then(|| DiscoveredDoc {
    path,
    size: metadata.len(),
    mtime_ns: mtime_ns(&metadata),
    note,
  })
}

fn excluded_by_name(relative: &str, name: &str) -> bool {
  let changelog = name.rsplit_once('.').is_some_and(|(stem, _)| stem.eq_ignore_ascii_case(CHANGELOG_STEM));
  changelog || relative.split('/').any(|segment| EXCLUDED_SEGMENTS.contains(&segment))
}

#[cfg(test)]
mod tests {
  use tempfile::TempDir;

  use super::*;

  struct Repo {
    _dir: TempDir,
    config: RepoConfig,
  }

  impl Repo {
    fn new() -> Self {
      let dir = tempfile::tempdir().unwrap();
      let path = fs::canonicalize(dir.path()).unwrap();
      let config = RepoConfig {
        name: atlas_core::RepoName::new("repo").unwrap(),
        path,
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      };
      Self { _dir: dir, config }
    }

    fn write(&self, relative: &str, content: &str) {
      let path = self.config.path.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(path, content).unwrap();
    }

    fn paths(&self, scope: DocScope) -> Vec<(String, bool)> {
      discover_docs(&self.config, scope, 1 << 20)
        .unwrap()
        .into_iter()
        .map(|doc| (doc.path.to_string(), doc.note))
        .collect()
    }
  }

  #[test]
  fn should_find_markdown_docs_and_notes_with_the_index_ignore_rules() {
    let repo = Repo::new();
    for path in [
      "README.md",
      "docs/design.mdx",
      "docs/CHANGELOG.md",
      "CHANGELOG.md",
      "src/notes.txt",
      "src/lib.rs",
      "node_modules/pkg/README.md",
      "packages/kit/node_modules/x/README.md",
      "tests/README.md",
      "src/fixtures/a.md",
      ".hidden/a.md",
      ".code-atlas/other.md",
      ".code-atlas/notes/retries.md",
      ".code-atlas/notes/.draft.md",
      ".code-atlas/notes/tests/a.md",
      "ignored/a.md",
    ] {
      repo.write(path, "# Title\n");
    }
    repo.write(".gitignore", "ignored/\n");
    assert_eq!(
      repo.paths(DocScope::All),
      [
        (".code-atlas/notes/retries.md".to_owned(), true),
        (".code-atlas/notes/tests/a.md".to_owned(), true),
        ("README.md".to_owned(), false),
        ("docs/design.mdx".to_owned(), false),
      ]
    );
    assert_eq!(
      repo.paths(DocScope::Notes),
      [(".code-atlas/notes/retries.md".to_owned(), true), (".code-atlas/notes/tests/a.md".to_owned(), true)]
    );
  }

  #[test]
  fn should_apply_include_globs_to_docs_but_not_to_notes_and_exclude_globs_to_both() {
    let mut repo = Repo::new();
    for path in ["README.md", "src/guide.md", ".code-atlas/notes/a.md", ".code-atlas/notes/old/b.md"] {
      repo.write(path, "# Title\n");
    }
    repo.config.include = vec!["src/".to_owned()];
    repo.config.exclude = vec![".code-atlas/notes/old/".to_owned()];
    assert_eq!(repo.paths(DocScope::All), [(".code-atlas/notes/a.md".to_owned(), true), ("src/guide.md".to_owned(), false)]);
  }

  #[test]
  fn should_skip_docs_over_the_size_cap() {
    let repo = Repo::new();
    repo.write("small.md", "12345");
    repo.write("big.md", "123456");
    let paths: Vec<String> = discover_docs(&repo.config, DocScope::All, 5).unwrap().into_iter().map(|doc| doc.path.to_string()).collect();
    assert_eq!(paths, ["small.md"]);
  }

  #[test]
  fn should_recognise_note_paths_and_doc_names() {
    assert!(is_note_path(".code-atlas/notes/a.md"));
    assert!(!is_note_path(".code-atlas/notesx/a.md"));
    assert!(!is_note_path("docs/.code-atlas/notes/a.md"));
    assert!(is_doc_name("README.MD") && is_doc_name("a.mdx"));
    assert!(!is_doc_name(".md") && !is_doc_name("a.markdown") && !is_doc_name("md"));
  }
}
