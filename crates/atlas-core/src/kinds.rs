use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

fn is_custom_kind_name(value: &str) -> bool {
  let mut chars = value.chars();
  matches!(chars.next(), Some('a'..='z')) && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

macro_rules! kind_enum {
  ($name:ident, $what:literal, { $($variant:ident => $text:literal),+ $(,)? }) => {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(try_from = "String", into = "String")]
    #[non_exhaustive]
    pub enum $name {
      $($variant,)+
      Custom(String),
    }

    impl $name {
      pub const BUILTIN: &'static [$name] = &[$($name::$variant),+];

      pub fn as_str(&self) -> &str {
        match self {
          $(Self::$variant => $text,)+
          Self::Custom(name) => name,
        }
      }

      pub fn custom(name: &str) -> Result<Self, CoreError> {
        name.parse()
      }
    }

    impl fmt::Display for $name {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
      }
    }

    impl FromStr for $name {
      type Err = CoreError;

      fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
          $($text => Ok(Self::$variant),)+
          custom if is_custom_kind_name(custom) => Ok(Self::Custom(custom.to_owned())),
          other => Err(CoreError::InvalidName { what: $what, value: other.to_owned(), reason: "custom kinds must be snake_case ASCII" }),
        }
      }
    }

    impl TryFrom<String> for $name {
      type Error = CoreError;

      fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
      }
    }

    impl From<$name> for String {
      fn from(value: $name) -> Self {
        value.as_str().to_owned()
      }
    }
  };
}

kind_enum!(EntityKind, "entity kind", {
  Repository => "repository",
  Package => "package",
  Application => "application",
  Service => "service",
  Module => "module",
  File => "file",
  Function => "function",
  Method => "method",
  Class => "class",
  Struct => "struct",
  Enum => "enum",
  Trait => "trait",
  Interface => "interface",
  TypeAlias => "type_alias",
  Constant => "constant",
  Field => "field",
  ApiEndpoint => "api_endpoint",
  ApiClientCall => "api_client_call",
  AuthScheme => "auth_scheme",
  Permission => "permission",
  ConfigKey => "config_key",
  EnvVar => "env_var",
  Event => "event",
  Queue => "queue",
  Job => "job",
  DbTable => "db_table",
  DbModel => "db_model",
  ExternalService => "external_service",
  Test => "test",
  TestSuite => "test_suite",
  Capability => "capability",
  Doc => "doc",
  Macro => "macro",
  Variant => "variant",
  Variable => "variable",
  Namespace => "namespace",
});

kind_enum!(EdgeKind, "edge kind", {
  Contains => "contains",
  Defines => "defines",
  Exports => "exports",
  Imports => "imports",
  ReExports => "re_exports",
  Calls => "calls",
  References => "references",
  Implements => "implements",
  Extends => "extends",
  Instantiates => "instantiates",
  TypeOf => "type_of",
  DependsOnPackage => "depends_on_package",
  ExposesEndpoint => "exposes_endpoint",
  CallsEndpoint => "calls_endpoint",
  SupportsAuth => "supports_auth",
  UsesAuth => "uses_auth",
  RequiresPermission => "requires_permission",
  ReadsConfig => "reads_config",
  ReadsEnv => "reads_env",
  Publishes => "publishes",
  Subscribes => "subscribes",
  ReadsTable => "reads_table",
  WritesTable => "writes_table",
  Tests => "tests",
  CoversFile => "covers_file",
  SimilarTo => "similar_to",
  EquivalentTo => "equivalent_to",
  RenamedFrom => "renamed_from",
  PartOfApp => "part_of_app",
  MapsToTable => "maps_to_table",
});

impl EntityKind {
  pub fn is_code_symbol(&self) -> bool {
    matches!(
      self,
      Self::Module
        | Self::Function
        | Self::Method
        | Self::Class
        | Self::Struct
        | Self::Enum
        | Self::Trait
        | Self::Interface
        | Self::TypeAlias
        | Self::Constant
        | Self::Field
        | Self::Test
        | Self::TestSuite
        | Self::Macro
        | Self::Variant
        | Self::Variable
        | Self::Namespace
    )
  }
}

impl EdgeKind {
  pub fn is_symmetric(&self) -> bool {
    matches!(self, Self::SimilarTo | Self::EquivalentTo)
  }

  pub fn forward_label(&self) -> Cow<'_, str> {
    match self {
      Self::ReExports => Cow::Borrowed("re-exports"),
      Self::TypeOf => Cow::Borrowed("has type"),
      Self::DependsOnPackage => Cow::Borrowed("depends on package"),
      Self::Tests => Cow::Borrowed("tests"),
      other => Cow::Owned(other.as_str().replace('_', " ")),
    }
  }

  pub fn reverse_label(&self) -> Cow<'_, str> {
    let label = match self {
      Self::Contains => "contained in",
      Self::Defines => "defined in",
      Self::Exports => "exported by",
      Self::Imports => "imported by",
      Self::ReExports => "re-exported by",
      Self::Calls => "called by",
      Self::References => "referenced by",
      Self::Implements => "implemented by",
      Self::Extends => "extended by",
      Self::Instantiates => "instantiated by",
      Self::TypeOf => "type of",
      Self::DependsOnPackage => "package depended on by",
      Self::ExposesEndpoint => "exposed by",
      Self::CallsEndpoint => "endpoint called by",
      Self::SupportsAuth => "auth supported by",
      Self::UsesAuth => "auth used by",
      Self::RequiresPermission => "permission required by",
      Self::ReadsConfig => "config read by",
      Self::ReadsEnv => "env read by",
      Self::Publishes => "published by",
      Self::Subscribes => "subscribed to by",
      Self::ReadsTable => "table read by",
      Self::WritesTable => "table written by",
      Self::Tests => "tested by",
      Self::CoversFile => "covered by",
      Self::SimilarTo => "similar to",
      Self::EquivalentTo => "equivalent to",
      Self::RenamedFrom => "renamed to",
      Self::PartOfApp => "has part",
      Self::MapsToTable => "table mapped by",
      Self::Custom(name) => return Cow::Owned(format!("{} (reverse)", name.replace('_', " "))),
    };
    Cow::Borrowed(label)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_round_trip_every_builtin_entity_kind() {
    for kind in EntityKind::BUILTIN {
      assert_eq!(kind.as_str().parse::<EntityKind>().unwrap(), *kind);
      assert_eq!(serde_json::to_string(kind).unwrap(), format!("\"{}\"", kind.as_str()));
      assert!(!matches!(kind, EntityKind::Custom(_)));
    }
    assert_eq!(EntityKind::BUILTIN.len(), 36);
  }

  #[test]
  fn should_round_trip_every_builtin_edge_kind() {
    for kind in EdgeKind::BUILTIN {
      assert_eq!(kind.as_str().parse::<EdgeKind>().unwrap(), *kind);
      assert_eq!(serde_json::from_str::<EdgeKind>(&serde_json::to_string(kind).unwrap()).unwrap(), *kind);
    }
    assert_eq!(EdgeKind::BUILTIN.len(), 30);
    assert_eq!(EdgeKind::MapsToTable.as_str(), "maps_to_table");
    assert_eq!(EdgeKind::MapsToTable.forward_label(), "maps to table");
  }

  #[test]
  fn should_parse_unknown_snake_case_names_as_custom_kinds() {
    assert_eq!("feature_flag".parse::<EntityKind>().unwrap(), EntityKind::Custom("feature_flag".into()));
    assert_eq!("guards".parse::<EdgeKind>().unwrap(), EdgeKind::Custom("guards".into()));
    assert_eq!(serde_json::from_str::<EntityKind>("\"feature_flag\"").unwrap(), EntityKind::Custom("feature_flag".into()));
  }

  #[test]
  fn should_canonicalise_custom_kinds_that_shadow_builtins() {
    assert_eq!(EntityKind::custom("function").unwrap(), EntityKind::Function);
    assert_eq!(EdgeKind::custom("calls").unwrap(), EdgeKind::Calls);
  }

  #[test]
  fn should_reject_malformed_custom_kinds() {
    for value in ["", "Function", "feature-flag", "1st", "a b", "a:b"] {
      assert!(value.parse::<EntityKind>().is_err(), "{value:?}");
      assert!(value.parse::<EdgeKind>().is_err(), "{value:?}");
    }
  }

  #[test]
  fn should_label_edges_for_both_traversal_directions() {
    assert_eq!(EdgeKind::Calls.forward_label(), "calls");
    assert_eq!(EdgeKind::Calls.reverse_label(), "called by");
    assert_eq!(EdgeKind::ReadsEnv.forward_label(), "reads env");
    assert_eq!(EdgeKind::RenamedFrom.reverse_label(), "renamed to");
    assert_eq!(EdgeKind::Custom("guards".into()).reverse_label(), "guards (reverse)");
    for kind in EdgeKind::BUILTIN {
      assert!(!kind.forward_label().is_empty());
      assert!(!kind.reverse_label().is_empty());
    }
  }

  #[test]
  fn should_mark_only_similarity_edges_as_symmetric() {
    let symmetric: Vec<_> = EdgeKind::BUILTIN.iter().filter(|kind| kind.is_symmetric()).collect();
    assert_eq!(symmetric, [&EdgeKind::SimilarTo, &EdgeKind::EquivalentTo]);
  }
}
