use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::ProjectError;

pub(crate) struct PathGlobs {
  matcher: Gitignore,
}

impl PathGlobs {
  pub(crate) fn new<S: AsRef<str>>(patterns: &[S]) -> Result<Self, ProjectError> {
    Self::build(patterns.iter().map(|pattern| pattern.as_ref().to_owned()))
  }

  pub(crate) fn anchored<S: AsRef<str>>(patterns: &[S]) -> Result<Self, ProjectError> {
    Self::build(patterns.iter().map(|pattern| anchor(pattern.as_ref())))
  }

  fn build(lines: impl Iterator<Item = String>) -> Result<Self, ProjectError> {
    let mut builder = GitignoreBuilder::new("");
    for line in lines {
      builder.add_line(None, &line).map_err(|error| ProjectError::InvalidGlob {
        pattern: line.clone(),
        message: error.to_string(),
      })?;
    }
    let matcher = builder.build().map_err(|error| ProjectError::InvalidGlob {
      pattern: String::new(),
      message: error.to_string(),
    })?;
    Ok(Self { matcher })
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.matcher.is_empty()
  }

  pub(crate) fn matches(&self, relative: &str, is_dir: bool) -> bool {
    self.matcher.matched(relative, is_dir).is_ignore()
  }

  pub(crate) fn matches_within(&self, relative: &str, is_dir: bool) -> bool {
    self.matcher.matched_path_or_any_parents(relative, is_dir).is_ignore()
  }
}

fn anchor(pattern: &str) -> String {
  let (negation, body) = match pattern.strip_prefix('!') {
    Some(rest) => ("!", rest),
    None => ("", pattern),
  };
  let body = body.strip_prefix("./").unwrap_or(body).trim_end_matches('/');
  let body = body.strip_prefix('/').unwrap_or(body);
  format!("{negation}/{body}")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_match_single_segment_wildcards_only_one_level_deep_when_anchored() {
    let globs = PathGlobs::anchored(&["crates/*"]).unwrap();
    assert!(globs.matches("crates/core", true));
    assert!(!globs.matches("crates/core/inner", true));
    assert!(!globs.matches("other/crates/core", true));
  }

  #[test]
  fn should_match_recursive_wildcards_when_anchored() {
    let globs = PathGlobs::anchored(&["apps/**"]).unwrap();
    assert!(globs.matches("apps/web", true));
    assert!(globs.matches("apps/web/nested", true));
  }

  #[test]
  fn should_let_negations_remove_earlier_matches() {
    let globs = PathGlobs::anchored(&["packages/*", "!packages/legacy"]).unwrap();
    assert!(globs.matches("packages/a", true));
    assert!(!globs.matches("packages/legacy", true));
  }

  #[test]
  fn should_reject_malformed_patterns() {
    assert!(matches!(PathGlobs::new(&["{a"]), Err(ProjectError::InvalidGlob { pattern, .. }) if pattern == "{a"));
  }

  #[test]
  fn should_match_unanchored_patterns_at_any_depth() {
    let globs = PathGlobs::new(&["*.generated.ts"]).unwrap();
    assert!(globs.matches("src/deep/a.generated.ts", false));
    assert!(!globs.matches("src/a.ts", false));
  }

  #[test]
  fn should_match_files_below_a_matching_directory() {
    let globs = PathGlobs::new(&["legacy/"]).unwrap();
    assert!(globs.matches_within("legacy/a/b.ts", false));
    assert!(!globs.matches_within("modern/a.ts", false));
  }
}
