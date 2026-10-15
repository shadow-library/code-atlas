use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atlas_project::AtlasHome;
use serde::{Deserialize, Serialize};

use super::atlas_cli::AtlasCli;
use super::error::{AgentError, AgentResult};
use super::process::ProcessExit;
use super::spec::LoadedTask;

const CONFIG_FILE: &str = "config.toml";
const DETECTORS_DIR: &str = "detectors";
const TOML_EXTENSION: &str = "toml";
const PROBE_PROJECT: &str = "config-probe";
const DOCTOR_FAILURE_PREFIX: &str = "FAIL";
const DOCTOR_EXIT_CODES: [i32; 2] = [0, 3];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasConfigRecord {
  pub blake3: String,
  pub files: Vec<String>,
  #[serde(default)]
  pub keys: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum Seeded {
  Config,
  Detector(String),
}

impl Seeded {
  fn name(&self) -> String {
    match self {
      Self::Config => CONFIG_FILE.to_owned(),
      Self::Detector(file) => format!("{DETECTORS_DIR}/{file}"),
    }
  }

  fn target(&self, home: &AtlasHome) -> PathBuf {
    match self {
      Self::Config => home.config_path(),
      Self::Detector(file) => home.detectors_dir().join(file),
    }
  }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AtlasConfig {
  dir: PathBuf,
  files: Vec<(Seeded, Vec<u8>)>,
  keys: Vec<String>,
}

impl fmt::Debug for AtlasConfig {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter
      .debug_struct("AtlasConfig")
      .field("dir", &self.dir)
      .field("files", &self.names())
      .field("keys", &self.keys)
      .finish()
  }
}

impl AtlasConfig {
  pub fn load(dir: &Path) -> AgentResult<Self> {
    if !dir.is_dir() {
      return Err(AgentError::AtlasConfigMissing(dir.to_owned()));
    }
    let mut files = Vec::new();
    let mut keys = Vec::new();
    if let Some(bytes) = read_regular(&dir.join(CONFIG_FILE))? {
      keys = key_paths(&bytes).map_err(|reason| AgentError::AtlasConfigRejected {
        dir: dir.to_owned(),
        reason: format!("{CONFIG_FILE}: {reason}"),
      })?;
      files.push((Seeded::Config, bytes));
    }
    files.extend(detector_files(&dir.join(DETECTORS_DIR))?);
    if files.is_empty() {
      return Err(AgentError::AtlasConfigEmpty(dir.to_owned()));
    }
    files.sort_by_key(|(seeded, _)| seeded.name());
    Ok(Self { dir: dir.to_owned(), files, keys })
  }

  pub fn names(&self) -> Vec<String> {
    self.files.iter().map(|(seeded, _)| seeded.name()).collect()
  }

  pub fn keys(&self) -> &[String] {
    &self.keys
  }

  pub fn digest(&self) -> String {
    let mut hasher = blake3::Hasher::new();
    for (seeded, bytes) in &self.files {
      let name = seeded.name();
      hasher.update(&(name.len() as u64).to_le_bytes());
      hasher.update(name.as_bytes());
      hasher.update(&(bytes.len() as u64).to_le_bytes());
      hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
  }

  pub fn record(&self) -> AtlasConfigRecord {
    AtlasConfigRecord {
      blake3: self.digest(),
      files: self.names(),
      keys: self.keys.clone(),
    }
  }

  pub fn describe(&self) -> String {
    format!(
      "{} file(s) from {} (blake3 {}): {}",
      self.files.len(),
      self.dir.display(),
      self.digest(),
      self.names().join(", ")
    )
  }

  pub fn apply(&self, root: &Path) -> AgentResult<()> {
    let home = AtlasHome::rooted_at(root);
    for (seeded, bytes) in &self.files {
      let target = seeded.target(&home);
      if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
      }
      fs::write(&target, bytes).map_err(|error| AgentError::io(&target, error))?;
    }
    Ok(())
  }

  pub fn seeded_paths(&self) -> BTreeSet<PathBuf> {
    let home = AtlasHome::rooted_at("");
    self.files.iter().map(|(seeded, _)| seeded.target(&home)).collect()
  }

  pub fn validate(&self, atlas_bin: &Path, probe_dir: &Path, timeout: Duration) -> AgentResult<()> {
    let rejected = |reason: String| AgentError::AtlasConfigRejected { dir: self.dir.clone(), reason };
    let home = probe_dir.join("home");
    let repo = probe_dir.join("repo");
    for directory in [&home, &repo] {
      fs::create_dir_all(directory).map_err(|error| AgentError::io(directory, error))?;
    }
    self.apply(&home)?;
    let atlas = AtlasCli::new(atlas_bin, &home, timeout).sandboxed(probe_dir.join("user"));
    atlas
      .project_create(PROBE_PROJECT, std::slice::from_ref(&repo), &repo)
      .map_err(|error| rejected(error.to_string()))?;
    let shown = atlas.config_show(PROBE_PROJECT, &repo).map_err(|error| rejected(error.to_string()))?;
    let mut problems: Vec<String> = [shown.stdout.as_str(), shown.stderr.as_str()]
      .into_iter()
      .flat_map(str::lines)
      .map(str::trim_start)
      .filter(|line| line.starts_with("warning:") || line.starts_with("error:"))
      .map(str::to_owned)
      .collect();
    let doctor = atlas.doctor(PROBE_PROJECT, &repo).map_err(|error| rejected(error.to_string()))?;
    problems.extend(doctor.stdout.lines().filter(|line| line.starts_with(DOCTOR_FAILURE_PREFIX)).map(str::to_owned));
    if problems.is_empty()
      && let ProcessExit::Exited { code } = doctor.exit
      && !code.is_some_and(|code| DOCTOR_EXIT_CODES.contains(&code))
    {
      problems.push(format!("catl doctor exited with {code:?}"));
    }
    if problems.is_empty() {
      return Ok(());
    }
    Err(rejected(problems.join("; ")))
  }
}

pub fn atlas_config_mismatch(task: &LoadedTask, used: Option<&AtlasConfig>) -> AgentResult<Option<String>> {
  let Some(dir) = task.atlas_config_dir() else { return Ok(None) };
  let wanted = AtlasConfig::load(&dir)?;
  Ok(match used {
    Some(used) if used.digest() == wanted.digest() => None,
    Some(used) => Some(format!(
      "task {} declares the Code Atlas configuration {} (blake3 {}), but this run uses blake3 {}",
      task.spec.id,
      dir.display(),
      wanted.digest(),
      used.digest()
    )),
    None => Some(format!(
      "task {} declares the Code Atlas configuration {}, but this run seeds none; pass --atlas-config {}",
      task.spec.id,
      dir.display(),
      dir.display()
    )),
  })
}

fn key_paths(bytes: &[u8]) -> Result<Vec<String>, String> {
  let text = std::str::from_utf8(bytes).map_err(|_| "is not UTF-8".to_owned())?;
  let table: toml::Table = text.parse().map_err(|error: toml::de::Error| error.message().to_owned())?;
  let mut keys = Vec::new();
  collect_keys(&table, "", &mut keys);
  keys.sort();
  Ok(keys)
}

fn collect_keys(table: &toml::Table, prefix: &str, keys: &mut Vec<String>) {
  for (key, value) in table {
    let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
    match value {
      toml::Value::Table(nested) if !nested.is_empty() => collect_keys(nested, &path, keys),
      _ => keys.push(path),
    }
  }
}

fn read_regular(path: &Path) -> AgentResult<Option<Vec<u8>>> {
  let metadata = match fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(AgentError::io(path, error)),
  };
  if !metadata.file_type().is_file() {
    tracing::warn!(path = %path.display(), "not a regular file; not copied");
    return Ok(None);
  }
  fs::read(path).map(Some).map_err(|error| AgentError::io(path, error))
}

fn detector_files(directory: &Path) -> AgentResult<Vec<(Seeded, Vec<u8>)>> {
  match fs::symlink_metadata(directory) {
    Ok(metadata) if metadata.file_type().is_dir() => {}
    Ok(_) => {
      tracing::warn!(path = %directory.display(), "not a directory; its files are not copied");
      return Ok(Vec::new());
    }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(AgentError::io(directory, error)),
  }
  let mut files = Vec::new();
  for entry in fs::read_dir(directory).map_err(|error| AgentError::io(directory, error))? {
    let entry = entry.map_err(|error| AgentError::io(directory, error))?;
    let path = entry.path();
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str().filter(|_| path.extension().is_some_and(|extension| extension == TOML_EXTENSION)) else {
      continue;
    };
    if let Some(bytes) = read_regular(&path)? {
      files.push((Seeded::Detector(name.to_owned()), bytes));
    }
  }
  Ok(files)
}

#[cfg(test)]
mod tests {
  use std::os::unix::fs::symlink;

  use super::*;

  fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn seeded() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("config.toml"), "[defaults.detect]\nroute_families = [\"acme-http\"]\n");
    write(&dir.path().join("detectors/acme-http.toml"), "version = 1\n");
    write(&dir.path().join("detectors/zeta.toml"), "version = 1\n");
    write(&dir.path().join("detectors/notes.md"), "private\n");
    write(&dir.path().join("projects.toml"), "ignored\n");
    dir
  }

  #[test]
  fn should_refuse_a_directory_that_does_not_exist() {
    let missing = std::env::temp_dir().join("ae-no-such-atlas-config-dir");
    assert!(matches!(AtlasConfig::load(&missing), Err(AgentError::AtlasConfigMissing(path)) if path == missing));
  }

  #[test]
  fn should_refuse_a_directory_with_nothing_to_copy() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("projects.toml"), "x\n");
    assert!(matches!(AtlasConfig::load(dir.path()), Err(AgentError::AtlasConfigEmpty(_))));
  }

  #[test]
  fn should_copy_only_config_toml_and_detector_toml_files_to_where_catl_reads_them() {
    let source = seeded();
    let config = AtlasConfig::load(source.path()).unwrap();
    let home = tempfile::tempdir().unwrap();
    config.apply(home.path()).unwrap();

    assert_eq!(config.names(), ["config.toml", "detectors/acme-http.toml", "detectors/zeta.toml"]);
    assert_eq!(
      fs::read_to_string(home.path().join("config/config.toml")).unwrap(),
      "[defaults.detect]\nroute_families = [\"acme-http\"]\n"
    );
    assert_eq!(fs::read_to_string(home.path().join("config/detectors/zeta.toml")).unwrap(), "version = 1\n");
    assert!(!home.path().join("config/detectors/notes.md").exists());
    assert!(!home.path().join("config/projects.toml").exists());
  }

  #[test]
  fn should_not_follow_symbolic_links_out_of_the_directory() {
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("secret.toml"), "secret = true\n");
    write(&outside.path().join("stolen/family.toml"), "version = 1\n");
    let source = seeded();
    symlink(outside.path().join("secret.toml"), source.path().join("detectors/leak.toml")).unwrap();
    let linked = tempfile::tempdir().unwrap();
    write(&linked.path().join("real.toml"), "version = 1\n");
    symlink(outside.path().join("secret.toml"), linked.path().join("config.toml")).unwrap();
    symlink(outside.path().join("stolen"), linked.path().join("detectors")).unwrap();

    let config = AtlasConfig::load(source.path()).unwrap();
    assert_eq!(config.names(), ["config.toml", "detectors/acme-http.toml", "detectors/zeta.toml"]);
    assert!(matches!(AtlasConfig::load(linked.path()), Err(AgentError::AtlasConfigEmpty(_))));
  }

  #[test]
  fn should_hash_the_sorted_paths_and_bytes_independently_of_the_source_directory() {
    let first = AtlasConfig::load(seeded().path()).unwrap();
    let second = AtlasConfig::load(seeded().path()).unwrap();
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.digest().len(), 64);

    let changed = seeded();
    write(&changed.path().join("detectors/zeta.toml"), "version = 2\n");
    assert_ne!(AtlasConfig::load(changed.path()).unwrap().digest(), first.digest());

    let renamed = seeded();
    fs::rename(renamed.path().join("detectors/zeta.toml"), renamed.path().join("detectors/omega.toml")).unwrap();
    assert_ne!(AtlasConfig::load(renamed.path()).unwrap().digest(), first.digest());
  }

  #[test]
  fn should_keep_file_contents_out_of_the_record_and_the_debug_output() {
    let source = seeded();
    write(&source.path().join("detectors/zeta.toml"), "private_marker = 1\n");
    let config = AtlasConfig::load(source.path()).unwrap();
    let record = serde_json::to_string(&config.record()).unwrap();
    assert!(!record.contains("private_marker") && !format!("{config:?}").contains("private_marker"));
    assert!(record.contains("detectors/zeta.toml"));
  }

  #[test]
  fn should_record_the_dotted_names_of_the_keys_config_toml_sets_and_never_their_values() {
    let source = tempfile::tempdir().unwrap();
    write(
      &source.path().join("config.toml"),
      "[defaults.detect]\nroute_families = [\"secret-family\"]\n[mcp]\ntools = \"core\"\n",
    );
    let record = AtlasConfig::load(source.path()).unwrap().record();
    assert_eq!(record.keys, ["defaults.detect.route_families", "mcp.tools"]);
    assert!(!serde_json::to_string(&record).unwrap().contains("secret-family"));
  }

  #[test]
  fn should_refuse_a_config_toml_that_is_not_valid_toml_without_echoing_its_text() {
    let source = tempfile::tempdir().unwrap();
    write(&source.path().join("config.toml"), "token = \"private_marker\n");
    let error = AtlasConfig::load(source.path()).unwrap_err();
    assert!(
      matches!(&error, AgentError::AtlasConfigRejected { .. }) && !error.to_string().contains("private_marker"),
      "{error}"
    );
  }

  #[test]
  fn should_place_the_files_at_the_paths_the_atlas_home_reads() {
    let source = seeded();
    let config = AtlasConfig::load(source.path()).unwrap();
    let home = AtlasHome::rooted_at("");
    assert_eq!(
      config.seeded_paths().into_iter().collect::<Vec<_>>(),
      [home.config_path(), home.detectors_dir().join("acme-http.toml"), home.detectors_dir().join("zeta.toml")]
    );
  }
}
