use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OldLineChange {
  Removed { line: u32 },
  InsertedBetween { before: u32, after: u32 },
}

impl OldLineChange {
  pub fn enclosed_by(&self, start_line: u32, end_line: u32) -> bool {
    match *self {
      Self::Removed { line } => start_line <= line && line <= end_line,
      Self::InsertedBetween { before, after } => start_line <= before && after <= end_line,
    }
  }
}

pub fn old_side_changes(patch: &str, path: &str) -> BTreeSet<OldLineChange> {
  let mut changes = BTreeSet::new();
  let mut in_file = false;
  let mut old_line: Option<u32> = None;
  for line in patch.lines() {
    if line.starts_with("diff --git ") {
      in_file = false;
      old_line = None;
      continue;
    }
    if let Some(old_path) = line.strip_prefix("--- ").filter(|_| old_line.is_none()) {
      in_file = old_path.strip_prefix("a/") == Some(path);
      continue;
    }
    if !in_file {
      continue;
    }
    if let Some(header) = line.strip_prefix("@@ ") {
      old_line = old_hunk_start(header);
      continue;
    }
    let Some(current) = old_line.as_mut() else {
      continue;
    };
    match line.chars().next() {
      Some('-') => {
        changes.insert(OldLineChange::Removed { line: *current });
        *current += 1;
      }
      Some('+') => {
        changes.insert(OldLineChange::InsertedBetween {
          before: current.saturating_sub(1),
          after: *current,
        });
      }
      Some(' ') | None => *current += 1,
      _ => {}
    }
  }
  changes
}

fn old_hunk_start(header: &str) -> Option<u32> {
  let old = header.strip_prefix('-')?.split_whitespace().next()?;
  old.split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
  use super::*;

  const PATCH: &str = "diff --git a/src/report.rs b/src/report.rs
index 201d1ef..ad7fd14 100644
--- a/src/report.rs
+++ b/src/report.rs
@@ -1,4 +1,5 @@
 use std::collections::BTreeMap;
+use std::fmt;
 use std::time::Duration;

 use atlas_core::{RelPath, RepoName, RunId};
@@ -42,4 +43,4 @@ pub enum FreshnessReport {
   Degraded {
     header: FreshnessHeader,
-    reason: String,
+    cause: DegradedCause,
   },
diff --git a/src/other.rs b/src/other.rs
--- a/src/other.rs
+++ b/src/other.rs
@@ -3,2 +3,2 @@
-x
--- a removed SQL comment
+y
+y
";

  #[test]
  fn should_map_removed_and_inserted_lines_to_old_line_numbers_of_one_file() {
    let changes: Vec<OldLineChange> = old_side_changes(PATCH, "src/report.rs").into_iter().collect();
    assert_eq!(
      changes,
      vec![
        OldLineChange::Removed { line: 44 },
        OldLineChange::InsertedBetween { before: 1, after: 2 },
        OldLineChange::InsertedBetween { before: 44, after: 45 },
      ]
    );
  }

  #[test]
  fn should_ignore_files_the_patch_does_not_touch() {
    assert_eq!(old_side_changes(PATCH, "src/missing.rs"), BTreeSet::new());
    assert_eq!(
      old_side_changes(PATCH, "src/other.rs").into_iter().collect::<Vec<_>>(),
      vec![
        OldLineChange::Removed { line: 3 },
        OldLineChange::Removed { line: 4 },
        OldLineChange::InsertedBetween { before: 4, after: 5 },
      ]
    );
  }

  #[test]
  fn should_require_an_insertion_to_lie_strictly_inside_a_span() {
    let inserted = OldLineChange::InsertedBetween { before: 27, after: 28 };
    assert!(inserted.enclosed_by(20, 28));
    assert!(!inserted.enclosed_by(26, 27));
    assert!(OldLineChange::Removed { line: 44 }.enclosed_by(42, 46));
  }
}
