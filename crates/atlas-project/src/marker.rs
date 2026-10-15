use std::fs;
use std::io;
use std::path::Path;

use atlas_core::Language;
use atlas_detect::check_family_name;
use toml::Value;

use crate::config::{as_words, flatten};
use crate::document::with_line_endings_of;
use crate::error::ProjectError;
use crate::globs::PathGlobs;

pub const MARKER_FILE: &str = ".code-atlas.toml";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoFile {
  pub project: Option<String>,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub languages: Option<Vec<Language>>,
  pub route_families: Vec<String>,
  pub warnings: Vec<String>,
  pub(crate) settings: Vec<(String, Value)>,
}

impl RepoFile {
  pub fn read(path: &Path) -> Result<Option<Self>, ProjectError> {
    if !path.is_file() {
      return Ok(None);
    }
    let text = match fs::read_to_string(path) {
      Ok(text) => text,
      Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(source) => return Err(ProjectError::io("read", path, source)),
    };
    Self::parse(path, &text).map(Some)
  }

  pub fn parse(path: &Path, text: &str) -> Result<Self, ProjectError> {
    let invalid = |message: String| ProjectError::InvalidMarker { path: path.to_owned(), message };
    let table: toml::Table = toml::from_str(text).map_err(|error| invalid(atlas_detect::toml_error_summary(text, &error)))?;
    let mut file = Self::default();
    for (name, value) in table {
      match (name.as_str(), value) {
        ("project", Value::String(project)) => file.project = Some(project),
        ("project", other) => return Err(invalid(format!("`project` must be a string, got {other}"))),
        ("index", Value::Table(index)) => file.read_index(index),
        ("index", _) => file.warnings.push("index ignored: it must be a table".to_owned()),
        ("detect", Value::Table(detect)) => file.read_detect(detect),
        ("detect", _) => file.warnings.push("detect ignored: it must be a table".to_owned()),
        (_, Value::Table(section)) => file.settings.extend(flatten(&name, &section)),
        (_, other) => file.settings.push((name, other)),
      }
    }
    Ok(file)
  }

  fn read_index(&mut self, index: toml::Table) {
    for (name, value) in index {
      let outcome = match name.as_str() {
        "include" => globs(&value).map(|globs| self.include = globs),
        "exclude" => globs(&value).map(|globs| self.exclude = globs),
        "languages" => match value.clone().try_into::<Vec<Language>>() {
          Ok(languages) if languages.is_empty() => Err("an empty list would index nothing; name at least one language".to_owned()),
          Ok(languages) => {
            self.languages = Some(languages);
            Ok(())
          }
          Err(_) => Err(format!("expected language names such as [\"rust\", \"typescript\"], got {value}")),
        },
        _ => Err("unknown key".to_owned()),
      };
      if let Err(message) = outcome {
        self.warnings.push(format!("index.{name} ignored: {message}"));
      }
    }
  }

  fn read_detect(&mut self, detect: toml::Table) {
    for (name, value) in detect {
      let outcome = match name.as_str() {
        "route_families" => family_names(&value).map(|names| self.route_families = names),
        _ => Err("unknown key".to_owned()),
      };
      if let Err(message) = outcome {
        self.warnings.push(format!("detect.{name} ignored: {message}"));
      }
    }
  }
}

fn family_names(value: &Value) -> Result<Vec<String>, String> {
  let names = as_words(value)?;
  names.iter().try_for_each(|name| check_family_name(name))?;
  Ok(names)
}

fn globs(value: &Value) -> Result<Vec<String>, String> {
  let globs = as_words(value)?;
  PathGlobs::new(&globs).map_err(|error| error.to_string())?;
  Ok(globs)
}

pub fn write_marker_project(path: &Path, project: &str) -> Result<(), ProjectError> {
  let invalid = |message: String| ProjectError::InvalidMarker { path: path.to_owned(), message };
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(source) if source.kind() == io::ErrorKind::NotFound => String::new(),
    Err(source) => return Err(ProjectError::io("read", path, source)),
  };
  let mut intended: toml::Table = toml::from_str(&text).map_err(|error: toml::de::Error| invalid(atlas_detect::toml_error_summary(&text, &error)))?;
  intended.insert("project".to_owned(), Value::String(project.to_owned()));
  let mut document: toml_edit::DocumentMut = text
    .parse()
    .map_err(|error: toml_edit::TomlError| invalid(atlas_detect::describe_toml_error(&text, error.message(), error.span())))?;
  document["project"] = toml_edit::value(project);
  let edited = document.to_string();
  let rendered = match toml::from_str::<toml::Table>(&edited) {
    Ok(parsed) if parsed == intended => edited,
    _ => {
      tracing::warn!(path = %path.display(), "keeping comments would have changed the marker; wrote a fresh serialisation without them");
      toml::to_string(&intended).map_err(|error| invalid(error.to_string()))?
    }
  };
  fs::write(path, with_line_endings_of(&text, rendered)).map_err(|source| ProjectError::io("write", path, source))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(text: &str) -> RepoFile {
    RepoFile::parse(Path::new("/r/.code-atlas.toml"), text).unwrap()
  }

  #[test]
  fn should_read_the_pin_and_index_keys_and_warn_on_everything_unknown() {
    let file = parse(
      "project = \"acme\"\nfuture = true\n[index]\nexclude = [\"legacy/**\"]\ninclude = [\"src/**\"]\nlanguages = [\"rust\"]\nshape = 1\n[detect]\nroute_families = [\"acme-http\"]\n[[detect.routes]]\npack = \"x\"\n[lsp]\nmax_servers = 4\n",
    );

    assert_eq!(file.project.as_deref(), Some("acme"));
    assert_eq!(
      (file.include, file.exclude, file.languages),
      (vec!["src/**".to_owned()], vec!["legacy/**".to_owned()], Some(vec![Language::Rust]))
    );
    assert_eq!(file.route_families, ["acme-http"]);
    assert_eq!(file.warnings, ["detect.routes ignored: unknown key", "index.shape ignored: unknown key"]);
    let keys: Vec<&str> = file.settings.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["future", "lsp.max_servers"]);
  }

  #[test]
  fn should_warn_and_drop_malformed_index_values() {
    let file = parse("[index]\nexclude = \"dist\"\ninclude = [\"{a\"]\nlanguages = [\"cobol\"]\n");

    assert_eq!((file.include.len(), file.exclude.len(), file.languages), (0, 0, None));
    assert_eq!(file.warnings.len(), 3);
    assert_eq!(file.warnings[0], "index.exclude ignored: expected an array of strings, got \"dist\"");
    assert!(file.warnings[1].starts_with("index.include ignored: invalid glob \"{a\""), "{:?}", file.warnings);
    assert_eq!(
      file.warnings[2],
      "index.languages ignored: expected language names such as [\"rust\", \"typescript\"], got [\"cobol\"]"
    );
  }

  #[test]
  fn should_warn_and_drop_malformed_route_family_lists() {
    let file = parse("[detect]\nroute_families = [\"Acme Http\"]\n");
    assert!(file.route_families.is_empty());
    assert_eq!(
      file.warnings,
      ["detect.route_families ignored: a route family name is a lowercase letter followed by lowercase letters, digits, - or _ (at most 64 characters), got \"Acme Http\""]
    );
    assert_eq!(parse("detect = 1\n").warnings, ["detect ignored: it must be a table"]);
  }

  #[test]
  fn should_ignore_an_empty_language_list() {
    let file = parse("[index]\nlanguages = []\n");
    assert_eq!(file.languages, None);
    assert_eq!(file.warnings, ["index.languages ignored: an empty list would index nothing; name at least one language"]);
  }

  #[test]
  fn should_accept_a_file_without_a_pin() {
    assert_eq!(parse("[index]\nexclude = [\"a\"]\n").project, None);
  }

  #[test]
  fn should_fail_on_broken_syntax_or_a_non_string_pin() {
    for text in ["project = \n", "project = 3\n"] {
      let error = RepoFile::parse(Path::new("/r/.code-atlas.toml"), text).unwrap_err();
      assert!(matches!(error, ProjectError::InvalidMarker { .. }), "{text}: {error}");
    }
  }

  #[test]
  fn should_set_the_pin_and_keep_every_other_byte_of_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MARKER_FILE);
    write_marker_project(&path, "acme").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "project = \"acme\"\n");

    fs::write(&path, "# team settings\n[index]\nexclude = [\"legacy/**\"] # generated code\n").unwrap();
    write_marker_project(&path, "acme").unwrap();

    assert_eq!(
      fs::read_to_string(&path).unwrap(),
      "project = \"acme\"\n# team settings\n[index]\nexclude = [\"legacy/**\"] # generated code\n"
    );

    fs::write(&path, "project = \"old\"\r\n# team settings\r\n[index]\r\nexclude = [\"legacy/**\"]\r\n").unwrap();
    write_marker_project(&path, "acme").unwrap();
    assert_eq!(
      fs::read_to_string(&path).unwrap(),
      "project = \"acme\"\r\n# team settings\r\n[index]\r\nexclude = [\"legacy/**\"]\r\n"
    );
  }

  #[test]
  fn should_not_echo_the_source_line_of_a_malformed_marker() {
    let error = RepoFile::parse(Path::new("/r/.code-atlas.toml"), "project = \"acme\"\napi_key_env = sk-FAKEKEY123\n")
      .unwrap_err()
      .to_string();

    assert!(!error.contains("FAKEKEY"), "{error}");
    assert_eq!(
      error,
      "invalid project marker /r/.code-atlas.toml: line 2, column 15: string values must be quoted, expected literal string"
    );
  }
}
