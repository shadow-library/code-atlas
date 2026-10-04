use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChange {
  Added,
  Modified,
  Deleted,
  Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedLine {
  pub line: u32,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
  pub path: String,
  pub old_path: Option<String>,
  pub change: FileChange,
  pub binary: bool,
  pub added: Vec<AddedLine>,
  pub removed_lines: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
  pub files: Vec<FileDiff>,
}

impl Diff {
  pub fn parse(text: &str) -> Self {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut next_line: u32 = 0;
    let mut in_hunk = false;
    for line in text.lines() {
      if let Some(header) = line.strip_prefix("diff --git ") {
        files.push(new_file(header));
        in_hunk = false;
        continue;
      }
      let Some(file) = files.last_mut() else {
        continue;
      };
      if let Some(hunk) = line.strip_prefix("@@ ") {
        next_line = hunk_start(hunk);
        in_hunk = true;
      } else if in_hunk {
        absorb_hunk_line(file, line, &mut next_line);
      } else {
        absorb_header_line(file, line);
      }
    }
    for file in &mut files {
      if file.old_path.as_deref() == Some(file.path.as_str()) || file.change == FileChange::Added {
        file.old_path = None;
      }
    }
    Self { files }
  }

  pub fn changed_files(&self) -> BTreeSet<String> {
    self.files.iter().map(|file| file.path.clone()).collect()
  }

  pub fn added_line_count(&self) -> u64 {
    self.files.iter().map(|file| file.added.len() as u64).sum()
  }

  pub fn removed_line_count(&self) -> u64 {
    self.files.iter().map(|file| u64::from(file.removed_lines)).sum()
  }

  pub fn added_lines(&self) -> impl Iterator<Item = (&FileDiff, &AddedLine)> {
    self.files.iter().flat_map(|file| file.added.iter().map(move |line| (file, line)))
  }
}

fn absorb_hunk_line(file: &mut FileDiff, line: &str, next_line: &mut u32) {
  if let Some(added) = line.strip_prefix('+') {
    file.added.push(AddedLine {
      line: *next_line,
      text: added.to_owned(),
    });
    *next_line += 1;
  } else if line.starts_with('-') {
    file.removed_lines += 1;
  } else if line.starts_with(' ') || line.is_empty() {
    *next_line += 1;
  }
}

fn absorb_header_line(file: &mut FileDiff, line: &str) {
  if line.starts_with("new file mode") {
    file.change = FileChange::Added;
  } else if line.starts_with("deleted file mode") {
    file.change = FileChange::Deleted;
  } else if let Some(from) = line.strip_prefix("rename from ") {
    file.old_path = Some(from.to_owned());
    file.change = FileChange::Renamed;
  } else if let Some(to) = line.strip_prefix("rename to ") {
    file.path = to.to_owned();
  } else if line.starts_with("Binary files ") {
    file.binary = true;
  } else if let Some(path) = line.strip_prefix("+++ b/") {
    file.path = path.to_owned();
  } else if let Some(path) = line.strip_prefix("--- a/") {
    file.old_path.get_or_insert_with(|| path.to_owned());
  }
}

fn new_file(header: &str) -> FileDiff {
  let path = header
    .strip_prefix("a/")
    .and_then(|rest| rest.split_once(" b/"))
    .map_or_else(|| header.to_owned(), |(_, new)| new.to_owned());
  FileDiff {
    path,
    old_path: None,
    change: FileChange::Modified,
    binary: false,
    added: Vec::new(),
    removed_lines: 0,
  }
}

fn hunk_start(hunk: &str) -> u32 {
  hunk
    .split_whitespace()
    .find_map(|part| part.strip_prefix('+'))
    .and_then(|range| range.split(',').next())
    .and_then(|start| start.parse().ok())
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;

  const SAMPLE: &str = "\
diff --git a/src/money.rs b/src/money.rs
index 1111111..2222222 100644
--- a/src/money.rs
+++ b/src/money.rs
@@ -1,3 +1,5 @@
 pub fn a() {}
-pub fn b() {}
+pub fn b() -> u8 { 1 }
+
+pub fn cents(x: f64) -> i64 { to_cents(x) }
 pub fn c() {}
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+fn helper() {}
+fn other() {}
diff --git a/src/gone.rs b/src/gone.rs
deleted file mode 100644
--- a/src/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-fn gone() {}
diff --git a/src/old_name.rs b/src/new_name.rs
similarity index 90%
rename from src/old_name.rs
rename to src/new_name.rs
diff --git a/logo.png b/logo.png
new file mode 100644
Binary files /dev/null and b/logo.png differ
";

  #[test]
  fn should_parse_statuses_paths_and_line_numbers() {
    let diff = Diff::parse(SAMPLE);
    let summary: Vec<(&str, FileChange, Option<&str>)> = diff.files.iter().map(|file| (file.path.as_str(), file.change, file.old_path.as_deref())).collect();
    assert_eq!(
      summary,
      vec![
        ("src/money.rs", FileChange::Modified, None),
        ("src/new.rs", FileChange::Added, None),
        ("src/gone.rs", FileChange::Deleted, None),
        ("src/new_name.rs", FileChange::Renamed, Some("src/old_name.rs")),
        ("logo.png", FileChange::Added, None),
      ]
    );
    let money = &diff.files[0];
    let added: Vec<(u32, &str)> = money.added.iter().map(|line| (line.line, line.text.as_str())).collect();
    assert_eq!(added, vec![(2, "pub fn b() -> u8 { 1 }"), (3, ""), (4, "pub fn cents(x: f64) -> i64 { to_cents(x) }")]);
    assert_eq!(money.removed_lines, 1);
    assert_eq!(diff.files[1].added[1].line, 2);
    assert!(diff.files[4].binary);
    assert_eq!(diff.added_line_count(), 5);
    assert_eq!(diff.removed_line_count(), 2);
  }

  #[test]
  fn should_treat_dash_dash_content_inside_a_hunk_as_a_removed_line() {
    let diff = Diff::parse("diff --git a/q.sql b/q.sql\n--- a/q.sql\n+++ b/q.sql\n@@ -1,2 +1,2 @@\n--- old comment\n+++ new comment\n select 1;\n");
    assert_eq!(diff.files[0].removed_lines, 1);
    assert_eq!(diff.files[0].added[0].text, "++ new comment");
    assert_eq!(diff.files[0].path, "q.sql");
  }

  #[test]
  fn should_list_changed_files_once_each() {
    let diff = Diff::parse(SAMPLE);
    assert_eq!(
      diff.changed_files().into_iter().collect::<Vec<_>>(),
      vec!["logo.png", "src/gone.rs", "src/money.rs", "src/new.rs", "src/new_name.rs"]
    );
  }
}
