use crate::graph::{ATTR_TEST, Entity};
use crate::kinds::EntityKind;
use crate::lang::LanguageFamily;

const TEST_DIRECTORIES: [&str; 3] = ["__tests__", "test", "tests"];
const TEST_MARKERS: [&str; 2] = ["spec", "test"];
const RUST_TEST_MODULES: [&str; 2] = ["tests", "test"];

pub fn is_test_path(path: &str) -> bool {
  let mut segments: Vec<&str> = path.split('/').collect();
  let file_name = segments.pop().unwrap_or_default();
  if segments.iter().any(|segment| TEST_DIRECTORIES.contains(segment)) || file_name.ends_with("_test.rs") {
    return true;
  }
  let stem = file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem);
  stem.split('.').skip(1).any(|part| {
    let marker = part.rsplit(['-', '_']).next().unwrap_or(part);
    TEST_MARKERS.contains(&marker)
  })
}

pub fn is_test_code(entity: &Entity) -> bool {
  if matches!(entity.kind, EntityKind::Test | EntityKind::TestSuite) || entity.attrs.contains_key(ATTR_TEST) {
    return true;
  }
  let in_test_module = entity
    .key
    .as_symbol()
    .is_some_and(|symbol| symbol.family() == LanguageFamily::Rust && symbol.module_path().iter().any(|segment| RUST_TEST_MODULES.contains(&segment.as_str())));
  in_test_module || entity.path.as_ref().is_some_and(|path| is_test_path(path.as_str()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_recognise_test_file_conventions_of_both_languages() {
    for path in [
      "src/users.spec.ts",
      "src/money.test.tsx",
      "test/app.e2e-spec.ts",
      "src/__tests__/x.ts",
      "tests/a.js",
      "a.int_test.mjs",
      "tests/invoices.rs",
      "crates/x/tests/y.rs",
      "src/money_test.rs",
    ] {
      assert!(is_test_path(path), "{path}");
    }
    for path in [
      "src/users.ts",
      "src/testing/helpers.ts",
      "src/spec.ts",
      "src/contest.ts",
      "src/latest.ts",
      "src/attest.rs",
      "src/test.rs",
    ] {
      assert!(!is_test_path(path), "{path}");
    }
  }
}
