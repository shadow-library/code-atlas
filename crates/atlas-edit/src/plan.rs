use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use atlas_core::{ContentHash, RelPath, RepoName};
use serde::{Deserialize, Serialize};

use crate::error::PlanViolation;
use crate::text::bom_len;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileTarget {
  pub repo: RepoName,
  pub path: RelPath,
}

impl FileTarget {
  pub fn new(repo: RepoName, path: RelPath) -> Self {
    Self { repo, path }
  }
}

impl fmt::Display for FileTarget {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}:{}", self.repo, self.path)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
  pub start: usize,
  pub end: usize,
  pub new_text: String,
}

impl TextEdit {
  pub fn replace(range: Range<usize>, new_text: impl Into<String>) -> Self {
    Self {
      start: range.start,
      end: range.end,
      new_text: new_text.into(),
    }
  }

  pub fn insert(at: usize, new_text: impl Into<String>) -> Self {
    Self::replace(at..at, new_text)
  }

  pub fn delete(range: Range<usize>) -> Self {
    Self::replace(range, "")
  }

  pub fn range(&self) -> Range<usize> {
    self.start..self.end
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdits {
  pub file: FileTarget,
  pub expected: ContentHash,
  pub edits: Vec<TextEdit>,
}

impl FileEdits {
  pub fn new(file: FileTarget, expected: ContentHash, mut edits: Vec<TextEdit>) -> Self {
    edits.sort_by_key(|edit| (edit.start, edit.end));
    Self { file, expected, edits }
  }

  pub(crate) fn validate_shape(&self) -> Result<(), PlanViolation> {
    if self.edits.is_empty() {
      return Err(PlanViolation::NoEdits { file: self.file.clone() });
    }
    for (index, edit) in self.edits.iter().enumerate() {
      if edit.start > edit.end {
        return Err(PlanViolation::Inverted {
          file: self.file.clone(),
          start: edit.start,
          end: edit.end,
        });
      }
      let Some(previous) = index.checked_sub(1).map(|before| &self.edits[before]) else {
        continue;
      };
      if (previous.start, previous.end) > (edit.start, edit.end) {
        return Err(PlanViolation::Unsorted { file: self.file.clone(), index });
      }
      if edit.start < previous.end {
        return Err(PlanViolation::Overlap {
          file: self.file.clone(),
          first: (previous.start, previous.end),
          second: (edit.start, edit.end),
        });
      }
    }
    Ok(())
  }

  pub fn check_against(&self, content: &str) -> Result<(), PlanViolation> {
    self.validate_shape()?;
    let bom = bom_len(content);
    for edit in &self.edits {
      if edit.end > content.len() {
        return Err(PlanViolation::OutOfBounds {
          file: self.file.clone(),
          start: edit.start,
          end: edit.end,
          len: content.len(),
        });
      }
      if edit.start < bom {
        return Err(PlanViolation::TouchesBom { file: self.file.clone() });
      }
      if let Some(offset) = [edit.start, edit.end].into_iter().find(|offset| !content.is_char_boundary(*offset)) {
        return Err(PlanViolation::NotCharBoundary { file: self.file.clone(), offset });
      }
    }
    Ok(())
  }

  pub fn apply_to(&self, content: &str) -> Result<String, PlanViolation> {
    self.check_against(content)?;
    Ok(splice(content, &self.edits))
  }
}

pub(crate) fn splice(content: &str, edits: &[TextEdit]) -> String {
  let mut output = String::with_capacity(content.len() + edits.iter().map(|edit| edit.new_text.len()).sum::<usize>());
  let mut cursor = 0;
  for edit in edits {
    output.push_str(&content[cursor..edit.start]);
    output.push_str(&edit.new_text);
    cursor = edit.end;
  }
  output.push_str(&content[cursor..]);
  output
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ResourceOp {
  Create { file: FileTarget, content: String },
  Rename { from: FileTarget, to: FileTarget, expected: ContentHash },
  Delete { file: FileTarget, expected: ContentHash },
}

impl ResourceOp {
  fn paths(&self) -> Vec<&FileTarget> {
    match self {
      Self::Create { file, .. } | Self::Delete { file, .. } => vec![file],
      Self::Rename { from, to, .. } => vec![from, to],
    }
  }
}

impl fmt::Display for ResourceOp {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Create { file, content } => write!(f, "create {file} ({} lines)", content.lines().count()),
      Self::Rename { from, to, .. } => write!(f, "rename {from} -> {to}"),
      Self::Delete { file, .. } => write!(f, "delete {file}"),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditPlan {
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub files: Vec<FileEdits>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub resources: Vec<ResourceOp>,
}

impl EditPlan {
  pub fn is_empty(&self) -> bool {
    self.files.is_empty() && self.resources.is_empty()
  }

  pub fn validate(&self) -> Result<(), PlanViolation> {
    if self.is_empty() {
      return Err(PlanViolation::Empty);
    }
    let mut edited: HashMap<&FileTarget, &ContentHash> = HashMap::new();
    for file in &self.files {
      file.validate_shape()?;
      if edited.insert(&file.file, &file.expected).is_some() {
        return Err(PlanViolation::DuplicateFile { file: file.file.to_string() });
      }
    }
    let mut claimed = HashSet::new();
    for op in &self.resources {
      if let ResourceOp::Rename { from, to, .. } = op
        && from == to
      {
        return Err(conflict(from, "renames a file onto itself (case-only renames are not supported)"));
      }
      for path in op.paths() {
        if !claimed.insert(path) {
          return Err(conflict(path, "appears in more than one resource operation"));
        }
      }
      match op {
        ResourceOp::Create { file, .. } if edited.contains_key(file) => return Err(conflict(file, "a created file cannot also be edited")),
        ResourceOp::Delete { file, .. } if edited.contains_key(file) => return Err(conflict(file, "a deleted file cannot also be edited")),
        ResourceOp::Rename { to, .. } if edited.contains_key(to) => return Err(conflict(to, "a rename destination cannot also be edited")),
        ResourceOp::Rename { from, expected, .. } if edited.get(from).is_some_and(|hash| *hash != expected) => {
          return Err(conflict(from, "the rename and the content edits expect different contents"));
        }
        _ => {}
      }
    }
    Ok(())
  }
}

fn conflict(path: &FileTarget, reason: &'static str) -> PlanViolation {
  PlanViolation::ResourceConflict { path: path.clone(), reason }
}
