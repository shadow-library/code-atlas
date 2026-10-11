use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchFiles {
  pub existing: BTreeMap<String, BTreeSet<OldLineChange>>,
  pub added: BTreeSet<String>,
}

enum Section {
  Outside,
  Existing(String),
  Added,
}

pub fn patch_files(patch: &str) -> PatchFiles {
  let mut files = PatchFiles::default();
  let mut section = Section::Outside;
  let mut old_line: Option<u32> = None;
  for line in patch.lines() {
    if line.starts_with("diff --git ") {
      section = Section::Outside;
      old_line = None;
      continue;
    }
    if let Some(old_path) = line.strip_prefix("--- ").filter(|_| old_line.is_none()) {
      section = match old_path.strip_prefix("a/") {
        Some(path) => {
          files.existing.entry(path.to_owned()).or_default();
          Section::Existing(path.to_owned())
        }
        None if old_path == "/dev/null" => Section::Added,
        None => Section::Outside,
      };
      continue;
    }
    if let Some(new_path) = line.strip_prefix("+++ ").filter(|_| old_line.is_none()) {
      if let (Section::Added, Some(path)) = (&section, new_path.strip_prefix("b/")) {
        files.added.insert(path.to_owned());
      }
      continue;
    }
    if let Some(header) = line.strip_prefix("@@ ") {
      old_line = old_hunk_start(header);
      continue;
    }
    let (Some(current), Section::Existing(path)) = (old_line.as_mut(), &section) else {
      continue;
    };
    let Some(changes) = files.existing.get_mut(path) else {
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
  files
}

pub fn old_side_changes(patch: &str, path: &str) -> BTreeSet<OldLineChange> {
  patch_files(patch).existing.remove(path).unwrap_or_default()
}

fn is_identifier_char(character: char) -> bool {
  character.is_alphanumeric() || character == '_' || character == '$'
}

fn word_spans<'a>(text: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
  text.match_indices(word).map(|(at, _)| at).filter(move |at| {
    let before = text[..*at].chars().next_back();
    let after = text[at + word.len()..].chars().next();
    !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
  })
}

fn has_word(text: &str, word: &str) -> bool {
  word_spans(text, word).next().is_some()
}

fn replace_word(text: &str, old: &str, new: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut cursor = 0;
  for at in word_spans(text, old) {
    out.push_str(&text[cursor..at]);
    out.push_str(new);
    cursor = at + old.len();
  }
  out.push_str(&text[cursor..]);
  out
}

pub fn renamed_old_lines(patch: &str, old: &str, new: &str) -> BTreeSet<(String, u32)> {
  let mut sites = BTreeSet::new();
  let mut path: Option<String> = None;
  let mut old_line: Option<u32> = None;
  let mut removed: Vec<(u32, &str)> = Vec::new();
  let mut added: Vec<&str> = Vec::new();
  let mut flush = |path: &Option<String>, removed: &mut Vec<(u32, &str)>, added: &mut Vec<&str>| {
    let mut used = vec![false; added.len()];
    for (index, (line, text)) in removed.iter().enumerate() {
      if !has_word(text, old) {
        continue;
      }
      let rewritten = replace_word(text, old, new);
      let exact = added.iter().enumerate().position(|(at, candidate)| !used[at] && **candidate == *rewritten);
      let positional = Some(index).filter(|at| added.get(*at).is_some_and(|candidate| !used[*at] && has_word(candidate, new) && !has_word(candidate, old)));
      if let (Some(at), Some(path)) = (exact.or(positional), path) {
        used[at] = true;
        sites.insert((path.clone(), *line));
      }
    }
    removed.clear();
    added.clear();
  };
  for line in patch.lines() {
    if line.starts_with("diff --git ") {
      flush(&path, &mut removed, &mut added);
      path = None;
      old_line = None;
      continue;
    }
    if let Some(old_path) = line.strip_prefix("--- ").filter(|_| old_line.is_none()) {
      path = old_path.strip_prefix("a/").map(str::to_owned);
      continue;
    }
    if line.starts_with("+++ ") && old_line.is_none() {
      continue;
    }
    if let Some(header) = line.strip_prefix("@@ ") {
      flush(&path, &mut removed, &mut added);
      old_line = old_hunk_start(header);
      continue;
    }
    let Some(current) = old_line.as_mut() else {
      continue;
    };
    match line.chars().next() {
      Some('-') => {
        removed.push((*current, &line[1..]));
        *current += 1;
      }
      Some('+') => added.push(&line[1..]),
      Some(' ') | None => {
        flush(&path, &mut removed, &mut added);
        *current += 1;
      }
      _ => {}
    }
  }
  flush(&path, &mut removed, &mut added);
  sites
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

  const ADDING_PATCH: &str = "diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 0000000..ad7fd14
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+pub fn fresh() {}
+--- not a header
diff --git a/src/kept.rs b/src/kept.rs
--- a/src/kept.rs
+++ b/src/kept.rs
@@ -7,3 +7,2 @@
 keep
-gone
 keep
diff --git a/src/mode.rs b/src/mode.rs
old mode 100644
new mode 100755
";

  #[test]
  fn should_list_added_files_apart_from_the_old_side_changes_of_existing_ones() {
    let files = patch_files(ADDING_PATCH);
    assert_eq!(files.added, BTreeSet::from(["src/new.rs".to_owned()]));
    assert_eq!(
      files.existing,
      BTreeMap::from([("src/kept.rs".to_owned(), BTreeSet::from([OldLineChange::Removed { line: 8 }]))])
    );
    let both = patch_files(PATCH);
    assert_eq!(both.existing.keys().collect::<Vec<_>>(), vec!["src/other.rs", "src/report.rs"]);
    assert_eq!(both.added, BTreeSet::new());
  }

  const RENAME_PATCH: &str = "diff --git a/src/a.ts b/src/a.ts
--- a/src/a.ts
+++ b/src/a.ts
@@ -5,6 +5,6 @@
 import x from 'x';
-import { OldName } from './b';
+import { NewName } from './b';
 const keep = OldNameExtra;
-const a = new OldName(), b = OldName.make();
+const a = new NewName(), b = NewName.make();
-const c = OldName;
+const c = somethingElse;
 end
diff --git a/src/b.ts b/src/b.ts
--- a/src/b.ts
+++ b/src/b.ts
@@ -1,3 +1,4 @@
-export class OldName {}
-// OldName is gone
+export class NewName {}
+// the old class is gone
+export const extra = 1;
";

  #[test]
  fn should_keep_only_old_side_lines_where_the_old_identifier_becomes_the_new_one() {
    let sites: Vec<(String, u32)> = renamed_old_lines(RENAME_PATCH, "OldName", "NewName").into_iter().collect();
    assert_eq!(sites, vec![("src/a.ts".to_owned(), 6), ("src/a.ts".to_owned(), 8), ("src/b.ts".to_owned(), 1)]);
  }

  #[test]
  fn should_not_match_longer_identifiers_or_other_files() {
    assert_eq!(renamed_old_lines(RENAME_PATCH, "Old", "New"), BTreeSet::new());
    assert_eq!(renamed_old_lines(RENAME_PATCH, "Missing", "NewName"), BTreeSet::new());
  }

  #[test]
  fn should_replace_whole_words_only() {
    assert_eq!(replace_word("OldName OldNameExtra _OldName OldName.x", "OldName", "N"), "N OldNameExtra _OldName N.x");
  }

  #[test]
  fn should_require_an_insertion_to_lie_strictly_inside_a_span() {
    let inserted = OldLineChange::InsertedBetween { before: 27, after: 28 };
    assert!(inserted.enclosed_by(20, 28));
    assert!(!inserted.enclosed_by(26, 27));
    assert!(OldLineChange::Removed { line: 44 }.enclosed_by(42, 46));
  }
}
