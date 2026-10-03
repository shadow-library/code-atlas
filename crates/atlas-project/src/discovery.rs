use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use atlas_core::{Language, RelPath};
use ignore::{DirEntry, WalkBuilder, WalkState};

use crate::error::ProjectError;
use crate::globs::PathGlobs;
use crate::registry::RepoConfig;

const DEFAULT_EXCLUDES: [&str; 8] = ["node_modules", "target", "dist", "build", ".git", ".next", "coverage", "vendor"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
  pub path: RelPath,
  pub language: Option<Language>,
  pub size: u64,
  pub mtime_ns: i64,
  pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
  TooLarge { size: u64, limit: u64 },
  NonUtf8Path,
  Symlink,
  Unreadable(String),
}

#[derive(Debug, Default)]
pub struct DiscoveryResult {
  pub files: Vec<DiscoveredFile>,
  pub skipped: Vec<(RelPath, SkipReason)>,
  pub walk_errors: Vec<String>,
}

pub fn is_manifest_name(file_name: &str) -> bool {
  matches!(file_name, "Cargo.toml" | "package.json" | "jsconfig.json") || (file_name.starts_with("tsconfig") && file_name.ends_with(".json"))
}

struct WalkFilter {
  root: std::path::PathBuf,
  overridden_defaults: HashSet<&'static str>,
  exclude: PathGlobs,
}

impl WalkFilter {
  fn admits(&self, entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
      return true;
    }
    let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir());
    let Some(name) = entry.file_name().to_str() else { return true };
    if is_dir && DEFAULT_EXCLUDES.contains(&name) && !self.overridden_defaults.contains(name) {
      return false;
    }
    if self.exclude.is_empty() {
      return true;
    }
    let Some(relative) = entry.path().strip_prefix(&self.root).ok().and_then(Path::to_str) else {
      return true;
    };
    !self.exclude.matches(relative, is_dir)
  }
}

#[derive(Default)]
struct Collected {
  files: Vec<DiscoveredFile>,
  skipped: Vec<(RelPath, SkipReason)>,
  walk_errors: Vec<String>,
}

pub fn discover(repo: &RepoConfig, max_file_bytes: u64) -> Result<DiscoveryResult, ProjectError> {
  fs::metadata(&repo.path).map_err(|source| ProjectError::io("read repository root", &repo.path, source))?;
  let include = PathGlobs::new(&repo.include)?;
  let overridden_defaults = DEFAULT_EXCLUDES
    .into_iter()
    .filter(|name| {
      repo
        .include
        .iter()
        .filter(|pattern| !pattern.starts_with('!'))
        .any(|pattern| pattern.split('/').any(|segment| segment == *name))
    })
    .collect();
  let filter = Arc::new(WalkFilter {
    root: repo.path.clone(),
    overridden_defaults,
    exclude: PathGlobs::new(&repo.exclude)?,
  });

  let walker = {
    let filter = Arc::clone(&filter);
    WalkBuilder::new(&repo.path)
      .hidden(true)
      .ignore(true)
      .git_ignore(true)
      .git_global(true)
      .git_exclude(true)
      .parents(false)
      .require_git(false)
      .follow_links(false)
      .filter_entry(move |entry| filter.admits(entry))
      .build_parallel()
  };

  let collected = Mutex::new(Collected::default());
  walker.run(|| {
    let collected = &collected;
    let include = &include;
    Box::new(move |result| {
      let outcome = match result {
        Ok(entry) => classify(&entry, repo, include, max_file_bytes),
        Err(error) => Outcome::WalkError(error.to_string()),
      };
      let Ok(mut sink) = collected.lock() else { return WalkState::Quit };
      match outcome {
        Outcome::Ignored => {}
        Outcome::File(file) => sink.files.push(file),
        Outcome::Skipped(path, reason) => sink.skipped.push((path, reason)),
        Outcome::WalkError(message) => sink.walk_errors.push(message),
      }
      WalkState::Continue
    })
  });

  let mut collected = collected.into_inner().unwrap_or_default();
  collected.files.sort_by(|a, b| a.path.cmp(&b.path));
  collected.skipped.sort();
  collected.walk_errors.sort();
  Ok(DiscoveryResult {
    files: collected.files,
    skipped: collected.skipped,
    walk_errors: collected.walk_errors,
  })
}

enum Outcome {
  Ignored,
  File(DiscoveredFile),
  Skipped(RelPath, SkipReason),
  WalkError(String),
}

fn classify(entry: &DirEntry, repo: &RepoConfig, include: &PathGlobs, max_file_bytes: u64) -> Outcome {
  let Some(file_type) = entry.file_type() else { return Outcome::Ignored };
  if file_type.is_dir() || entry.depth() == 0 {
    return Outcome::Ignored;
  }
  let Ok(relative_os) = entry.path().strip_prefix(&repo.path) else {
    return Outcome::Ignored;
  };
  let (relative, utf8) = match relative_os.to_str() {
    Some(text) => (text.to_owned(), true),
    None => (relative_os.to_string_lossy().into_owned(), false),
  };
  let file_name = relative.rsplit(['/', '\\']).next().unwrap_or(&relative);
  let language = Language::from_path(&relative);
  let manifest = is_manifest_name(file_name);
  if language.is_none() && !manifest {
    return Outcome::Ignored;
  }
  if let (Some(language), Some(filter)) = (language, &repo.languages)
    && !filter.contains(&language)
  {
    return Outcome::Ignored;
  }
  if !manifest && !include.is_empty() && !include.matches_within(&relative, false) {
    return Outcome::Ignored;
  }
  let Ok(path) = RelPath::new(&relative) else {
    tracing::warn!(path = %relative, "skipping path that cannot be represented as a repository-relative path");
    return Outcome::Ignored;
  };
  if !utf8 {
    return Outcome::Skipped(path, SkipReason::NonUtf8Path);
  }
  if file_type.is_symlink() {
    return Outcome::Skipped(path, SkipReason::Symlink);
  }
  let metadata = match entry.metadata() {
    Ok(metadata) => metadata,
    Err(error) => return Outcome::Skipped(path, SkipReason::Unreadable(error.to_string())),
  };
  if metadata.len() > max_file_bytes {
    return Outcome::Skipped(
      path,
      SkipReason::TooLarge {
        size: metadata.len(),
        limit: max_file_bytes,
      },
    );
  }
  Outcome::File(DiscoveredFile {
    path,
    language,
    size: metadata.len(),
    mtime_ns: mtime_ns(&metadata),
    inode: inode(&metadata),
  })
}

pub(crate) fn mtime_ns(metadata: &Metadata) -> i64 {
  let Ok(modified) = metadata.modified() else { return 0 };
  match modified.duration_since(UNIX_EPOCH) {
    Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
    Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |nanos| -nanos),
  }
}

#[cfg(unix)]
pub(crate) fn inode(metadata: &Metadata) -> u64 {
  std::os::unix::fs::MetadataExt::ino(metadata)
}

#[cfg(not(unix))]
pub(crate) fn inode(_metadata: &Metadata) -> u64 {
  0
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

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

    fn paths(&self, limit: u64) -> Vec<String> {
      discover(&self.config, limit).unwrap().files.iter().map(|file| file.path.to_string()).collect()
    }
  }

  const NO_LIMIT: u64 = 1 << 20;

  #[test]
  fn should_return_supported_sources_and_manifests_only() {
    let repo = Repo::new();
    for name in [
      "src/lib.rs",
      "src/app.ts",
      "src/App.tsx",
      "src/a.js",
      "src/b.jsx",
      "README.md",
      "src/data.json",
      "Cargo.toml",
      "package.json",
      "tsconfig.json",
      "tsconfig.build.json",
      "jsconfig.json",
    ] {
      repo.write(name, "x");
    }
    assert_eq!(
      repo.paths(NO_LIMIT),
      [
        "Cargo.toml",
        "jsconfig.json",
        "package.json",
        "src/App.tsx",
        "src/a.js",
        "src/app.ts",
        "src/b.jsx",
        "src/lib.rs",
        "tsconfig.build.json",
        "tsconfig.json"
      ]
    );
  }

  #[test]
  fn should_tag_languages_and_leave_manifests_untagged() {
    let repo = Repo::new();
    repo.write("a.rs", "fn main() {}");
    repo.write("package.json", "{}");
    let result = discover(&repo.config, NO_LIMIT).unwrap();
    let languages: Vec<_> = result.files.iter().map(|file| (file.path.as_str(), file.language)).collect();
    assert_eq!(languages, [("a.rs", Some(Language::Rust)), ("package.json", None)]);
    assert_eq!(result.files[0].size, 12);
    assert!(result.files[0].mtime_ns > 0);
  }

  #[test]
  fn should_honour_gitignore_without_a_git_directory() {
    let repo = Repo::new();
    repo.write(".gitignore", "ignored.ts\ngenerated/\n");
    repo.write("ignored.ts", "x");
    repo.write("generated/a.ts", "x");
    repo.write("kept.ts", "x");
    assert_eq!(repo.paths(NO_LIMIT), ["kept.ts"]);
  }

  #[test]
  fn should_honour_dot_ignore_files() {
    let repo = Repo::new();
    repo.write(".ignore", "scratch.ts\n");
    repo.write("scratch.ts", "x");
    repo.write("kept.ts", "x");
    assert_eq!(repo.paths(NO_LIMIT), ["kept.ts"]);
  }

  #[test]
  fn should_skip_hidden_files_and_directories() {
    let repo = Repo::new();
    repo.write(".hidden/a.ts", "x");
    repo.write(".secret.ts", "x");
    repo.write("visible.ts", "x");
    assert_eq!(repo.paths(NO_LIMIT), ["visible.ts"]);
  }

  #[test]
  fn should_apply_default_excludes_at_any_depth() {
    let repo = Repo::new();
    for dir in ["node_modules", "target", "dist", "build", ".next", "coverage", "vendor", "pkg/node_modules", "pkg/dist"] {
      repo.write(&format!("{dir}/x.ts"), "x");
    }
    repo.write("pkg/src/ok.ts", "x");
    repo.write("builder/ok.ts", "x");
    assert_eq!(repo.paths(NO_LIMIT), ["builder/ok.ts", "pkg/src/ok.ts"]);
  }

  #[test]
  fn should_let_include_globs_override_default_excludes() {
    let mut repo = Repo::new();
    repo.write("vendor/lib/a.ts", "x");
    repo.write("vendor/other/b.ts", "x");
    repo.write("src/c.ts", "x");
    repo.config.include = vec!["vendor/lib/**".to_owned()];
    assert_eq!(repo.paths(NO_LIMIT), ["vendor/lib/a.ts"]);
  }

  #[test]
  fn should_restrict_to_include_globs_but_keep_manifests() {
    let mut repo = Repo::new();
    repo.write("src/a.ts", "x");
    repo.write("scripts/b.ts", "x");
    repo.write("package.json", "{}");
    repo.config.include = vec!["src/".to_owned()];
    assert_eq!(repo.paths(NO_LIMIT), ["package.json", "src/a.ts"]);
  }

  #[test]
  fn should_apply_exclude_globs_to_files_and_directories() {
    let mut repo = Repo::new();
    repo.write("src/a.ts", "x");
    repo.write("src/a.generated.ts", "x");
    repo.write("legacy/b.ts", "x");
    repo.write("src/legacy/c.ts", "x");
    repo.config.exclude = vec!["*.generated.ts".to_owned(), "legacy/".to_owned()];
    assert_eq!(repo.paths(NO_LIMIT), ["src/a.ts"]);
  }

  #[test]
  fn should_filter_by_language_but_keep_manifests() {
    let mut repo = Repo::new();
    repo.write("a.rs", "x");
    repo.write("b.ts", "x");
    repo.write("Cargo.toml", "x");
    repo.config.languages = Some(vec![Language::Rust]);
    assert_eq!(repo.paths(NO_LIMIT), ["Cargo.toml", "a.rs"]);
  }

  #[test]
  fn should_skip_files_above_the_size_limit_and_record_why() {
    let repo = Repo::new();
    repo.write("small.ts", "12345");
    repo.write("big.ts", "123456");
    let result = discover(&repo.config, 5).unwrap();
    assert_eq!(result.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["small.ts"]);
    assert_eq!(result.skipped, [(RelPath::new("big.ts").unwrap(), SkipReason::TooLarge { size: 6, limit: 5 })]);
  }

  #[test]
  fn should_not_follow_symlinks_and_should_report_symlinked_sources() {
    let repo = Repo::new();
    repo.write("real/a.ts", "x");
    repo.write("outside/secret.ts", "x");
    let root = &repo.config.path;
    std::os::unix::fs::symlink(root.join("outside"), root.join("linked-dir")).unwrap();
    std::os::unix::fs::symlink(root.join("real/a.ts"), root.join("linked-file.ts")).unwrap();
    let result = discover(&repo.config, NO_LIMIT).unwrap();
    let paths: Vec<_> = result.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["outside/secret.ts", "real/a.ts"]);
    assert_eq!(result.skipped, [(RelPath::new("linked-file.ts").unwrap(), SkipReason::Symlink)]);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn should_record_non_utf8_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let repo = Repo::new();
    let name = OsString::from_vec(b"bad-\xff.ts".to_vec());
    fs::write(repo.config.path.join(name), "x").unwrap();
    repo.write("good.ts", "x");
    let result = discover(&repo.config, NO_LIMIT).unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].1, SkipReason::NonUtf8Path);
  }

  #[test]
  fn should_return_deterministically_sorted_output() {
    let repo = Repo::new();
    for index in (0..60).rev() {
      repo.write(&format!("d{}/f{}.ts", index % 7, index), "x");
    }
    let first = repo.paths(NO_LIMIT);
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted);
    assert_eq!(first.len(), 60);
    for _ in 0..5 {
      assert_eq!(repo.paths(NO_LIMIT), first);
    }
  }

  #[test]
  fn should_fail_for_a_missing_repository_root() {
    let mut repo = Repo::new();
    repo.config.path = PathBuf::from("/definitely/not/here");
    assert!(matches!(discover(&repo.config, NO_LIMIT), Err(ProjectError::Io { .. })));
  }
}
