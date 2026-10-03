use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Language {
  Rust,
  #[serde(rename = "typescript")]
  TypeScript,
  Tsx,
  #[serde(rename = "javascript")]
  JavaScript,
  Jsx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LanguageFamily {
  Rust,
  Ts,
}

const DECLARATION_SUFFIXES: [&str; 3] = [".d.ts", ".d.mts", ".d.cts"];

impl Language {
  pub const ALL: [Language; 5] = [Language::Rust, Language::TypeScript, Language::Tsx, Language::JavaScript, Language::Jsx];

  pub fn from_extension(extension: &str) -> Option<Self> {
    match extension.to_ascii_lowercase().as_str() {
      "rs" => Some(Self::Rust),
      "ts" | "mts" | "cts" => Some(Self::TypeScript),
      "tsx" => Some(Self::Tsx),
      "js" | "mjs" | "cjs" => Some(Self::JavaScript),
      "jsx" => Some(Self::Jsx),
      _ => None,
    }
  }

  pub fn from_path(path: &str) -> Option<Self> {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, extension) = file_name.rsplit_once('.')?;
    if stem.is_empty() {
      return None;
    }
    Self::from_extension(extension)
  }

  pub fn is_declaration_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    DECLARATION_SUFFIXES.iter().any(|suffix| lower.len() > suffix.len() && lower.ends_with(suffix))
  }

  pub fn family(self) -> LanguageFamily {
    match self {
      Self::Rust => LanguageFamily::Rust,
      Self::TypeScript | Self::Tsx | Self::JavaScript | Self::Jsx => LanguageFamily::Ts,
    }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Rust => "rust",
      Self::TypeScript => "typescript",
      Self::Tsx => "tsx",
      Self::JavaScript => "javascript",
      Self::Jsx => "jsx",
    }
  }
}

impl fmt::Display for Language {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl FromStr for Language {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    Self::ALL.into_iter().find(|language| language.as_str() == value).ok_or_else(|| CoreError::UnknownVariant {
      what: "language",
      value: value.to_owned(),
    })
  }
}

impl LanguageFamily {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Rust => "rust",
      Self::Ts => "ts",
    }
  }
}

impl fmt::Display for LanguageFamily {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl FromStr for LanguageFamily {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    match value {
      "rust" => Ok(Self::Rust),
      "ts" => Ok(Self::Ts),
      other => Err(CoreError::UnknownVariant {
        what: "language family",
        value: other.to_owned(),
      }),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_detect_language_from_path() {
    let cases = [
      ("src/lib.rs", Some(Language::Rust)),
      ("a.ts", Some(Language::TypeScript)),
      ("a.mts", Some(Language::TypeScript)),
      ("a.cts", Some(Language::TypeScript)),
      ("types/index.d.ts", Some(Language::TypeScript)),
      ("App.tsx", Some(Language::Tsx)),
      ("a.js", Some(Language::JavaScript)),
      ("a.mjs", Some(Language::JavaScript)),
      ("a.cjs", Some(Language::JavaScript)),
      ("Button.jsx", Some(Language::Jsx)),
      ("LOUD.TS", Some(Language::TypeScript)),
      ("src\\win.rs", Some(Language::Rust)),
      ("README.md", None),
      ("Makefile", None),
      (".rs", None),
      ("dir.rs/file", None),
    ];
    for (path, expected) in cases {
      assert_eq!(Language::from_path(path), expected, "{path}");
    }
  }

  #[test]
  fn should_flag_declaration_files() {
    assert!(Language::is_declaration_file("types/index.d.ts"));
    assert!(Language::is_declaration_file("x.d.mts"));
    assert!(Language::is_declaration_file("x.d.cts"));
    assert!(!Language::is_declaration_file("index.ts"));
    assert!(!Language::is_declaration_file("dts.ts"));
    assert!(!Language::is_declaration_file(".d.ts"));
  }

  #[test]
  fn should_map_languages_to_families() {
    assert_eq!(Language::Rust.family(), LanguageFamily::Rust);
    for language in [Language::TypeScript, Language::Tsx, Language::JavaScript, Language::Jsx] {
      assert_eq!(language.family(), LanguageFamily::Ts);
    }
  }

  #[test]
  fn should_round_trip_language_strings() {
    for language in Language::ALL {
      assert_eq!(language.as_str().parse::<Language>().unwrap(), language);
      assert_eq!(serde_json::to_string(&language).unwrap(), format!("\"{}\"", language.as_str()));
    }
    assert_eq!("ts".parse::<LanguageFamily>().unwrap(), LanguageFamily::Ts);
    assert!("python".parse::<Language>().is_err());
  }
}
