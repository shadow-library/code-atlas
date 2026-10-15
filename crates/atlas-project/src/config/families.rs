use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use atlas_detect::{DecoratorFamily, check_family_name};

use crate::error::ProjectError;

const FAMILY_EXTENSION: &str = "toml";

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrokenFamily {
  path: PathBuf,
  message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FamilyCatalog {
  dir: PathBuf,
  families: BTreeMap<String, DecoratorFamily>,
  broken: BTreeMap<String, BrokenFamily>,
}

impl FamilyCatalog {
  pub(crate) fn load(dir: &Path, warnings: &mut Vec<String>) -> Result<Self, ProjectError> {
    let mut catalog = Self {
      dir: dir.to_owned(),
      ..Self::default()
    };
    let entries = match fs::read_dir(dir) {
      Ok(entries) => entries,
      Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(catalog),
      Err(source) => return Err(ProjectError::io("read", dir, source)),
    };
    let mut paths: Vec<PathBuf> = entries
      .map(|entry| entry.map(|entry| entry.path()).map_err(|source| ProjectError::io("read", dir, source)))
      .collect::<Result<_, _>>()?;
    paths.sort();
    for path in paths.into_iter().filter(|path| is_family_file(path)) {
      catalog.read(&path, warnings);
    }
    Ok(catalog)
  }

  pub(crate) fn get(&self, name: &str) -> Option<&DecoratorFamily> {
    self.families.get(name)
  }

  pub(crate) fn path_of(&self, name: &str) -> PathBuf {
    self.dir.join(format!("{name}.{FAMILY_EXTENSION}"))
  }

  pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
    self.families.keys().map(String::as_str)
  }

  pub(crate) fn check_enabled<'n>(&self, mut enabled: impl Iterator<Item = &'n str>) -> Result<(), ProjectError> {
    match enabled.find_map(|name| self.broken.get(name)) {
      Some(broken) => Err(ProjectError::ConfigParse {
        path: broken.path.clone(),
        message: broken.message.clone(),
      }),
      None => Ok(()),
    }
  }

  fn read(&mut self, path: &Path, warnings: &mut Vec<String>) {
    let file = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_owned();
    if let Err(message) = check_family_name(&name) {
      warnings.push(format!("detectors/{file} ignored: file name: {message}"));
      return;
    }
    let parsed = fs::read_to_string(path)
      .map_err(|error| format!("cannot read it: {error}"))
      .and_then(|text| DecoratorFamily::parse(&name, &text).map_err(|error| error.to_string()));
    match parsed {
      Ok(parsed) => {
        warnings.extend(parsed.warnings.iter().map(|warning| format!("detectors/{file}: {warning}")));
        self.families.insert(name, parsed.family);
      }
      Err(message) => {
        warnings.push(format!("detectors/{file} ignored: {message}"));
        self.broken.insert(name, BrokenFamily { path: path.to_owned(), message });
      }
    }
  }
}

fn is_family_file(path: &Path) -> bool {
  let visible = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| !name.starts_with('.'));
  visible && path.extension().is_some_and(|extension| extension == FAMILY_EXTENSION) && path.is_file()
}

#[cfg(test)]
mod tests {
  use super::*;

  const ACME: &str = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";

  fn load(files: &[(&str, &str)]) -> (tempfile::TempDir, FamilyCatalog, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
      fs::write(dir.path().join(name), text).unwrap();
    }
    let mut warnings = Vec::new();
    let catalog = FamilyCatalog::load(dir.path(), &mut warnings).unwrap();
    (dir, catalog, warnings)
  }

  #[test]
  fn should_treat_a_missing_directory_as_no_families() {
    let mut warnings = Vec::new();
    let catalog = FamilyCatalog::load(Path::new("/nonexistent/code-atlas/detectors"), &mut warnings).unwrap();
    assert_eq!(catalog.names().count(), 0);
    assert!(warnings.is_empty());
  }

  #[test]
  fn should_load_toml_files_by_stem_and_skip_everything_else() {
    let (_dir, catalog, warnings) = load(&[
      ("acme-http.toml", &format!("shape = 1\n{ACME}")),
      ("notes.md", "not a family"),
      (".hidden.toml", "broken ="),
    ]);
    assert_eq!(catalog.names().collect::<Vec<_>>(), ["acme-http"]);
    assert_eq!(catalog.get("acme-http").map(DecoratorFamily::name), Some("acme-http"));
    assert_eq!(warnings, ["detectors/acme-http.toml: unknown key shape ignored"]);
  }

  #[test]
  fn should_skip_a_file_whose_name_is_no_family_name_with_a_warning() {
    let (_dir, catalog, warnings) = load(&[("Acme Http.toml", ACME), ("acme-http.toml", ACME)]);
    assert_eq!(catalog.names().collect::<Vec<_>>(), ["acme-http"]);
    assert_eq!(catalog.check_enabled(["Acme Http"].into_iter()).map_err(|error| error.to_string()), Ok(()));
    assert_eq!(
      warnings,
      ["detectors/Acme Http.toml ignored: file name: a route family name is a lowercase letter followed by lowercase letters, digits, - or _ (at most 64 characters)"]
    );
  }

  #[test]
  fn should_warn_about_an_invalid_family_and_fail_only_where_it_is_enabled() {
    let broken = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = 1\n";
    let (dir, catalog, warnings) = load(&[("acme-http.toml", broken), ("acme-rpc.toml", ACME)]);
    let message = "routes.methods.Read: expected an HTTP method such as \"GET\", \"POST\" or \"ANY\", got 1";

    assert_eq!(catalog.names().collect::<Vec<_>>(), ["acme-rpc"]);
    assert_eq!(warnings, [format!("detectors/acme-http.toml ignored: {message}")]);
    assert!(catalog.check_enabled(["acme-rpc"].into_iter()).is_ok());
    assert_eq!(
      catalog.check_enabled(["acme-rpc", "acme-http"].into_iter()).map_err(|error| error.to_string()),
      Err(format!("invalid config file {}: {message}", dir.path().join("acme-http.toml").display()))
    );
  }
}
