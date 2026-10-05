use std::fmt;

use crate::error::PlanViolation;
use crate::occurrence::{Occurrence, OccurrenceId};
use crate::plan::{FileTarget, TextEdit};
use crate::text::LineIndex;

const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
  Context,
  Removed,
  Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
  pub kind: LineKind,
  pub text: String,
  pub occurrences: Vec<OccurrenceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
  pub old_start: usize,
  pub old_lines: usize,
  pub new_start: usize,
  pub new_lines: usize,
  pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
  pub file: FileTarget,
  pub hunks: Vec<Hunk>,
}

impl fmt::Display for FileDiff {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(f, "--- {}", self.file)?;
    writeln!(f, "+++ {}", self.file)?;
    for hunk in &self.hunks {
      writeln!(f, "{hunk}")?;
    }
    Ok(())
  }
}

impl fmt::Display for Hunk {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let start = |first: usize, count: usize| if count == 0 { first.saturating_sub(1) } else { first };
    write!(
      f,
      "@@ -{},{} +{},{} @@",
      start(self.old_start, self.old_lines),
      self.old_lines,
      start(self.new_start, self.new_lines),
      self.new_lines
    )?;
    for line in &self.lines {
      let marker = match line.kind {
        LineKind::Context => ' ',
        LineKind::Removed => '-',
        LineKind::Added => '+',
      };
      write!(f, "\n{marker}{}", line.text)?;
      if !line.occurrences.is_empty() {
        let ids = line.occurrences.iter().map(OccurrenceId::as_str).collect::<Vec<_>>().join(" ");
        write!(f, "  [{ids}]")?;
      }
    }
    Ok(())
  }
}

struct Change {
  old_from: usize,
  removed: usize,
  added: Vec<String>,
}

pub fn file_diff(file: &FileTarget, original: &str, edits: &[TextEdit], occurrences: &[Occurrence]) -> Result<FileDiff, PlanViolation> {
  check(file, original, edits)?;
  let index = LineIndex::new(original);
  let lines: Vec<&str> = original.split_inclusive('\n').collect();
  let changes = changes(original, &index, &lines, edits);
  let annotations: Vec<(usize, &OccurrenceId)> = occurrences.iter().map(|occurrence| (index.line_of(occurrence.start), &occurrence.id)).collect();
  let mut hunks = Vec::new();
  let mut current: Option<HunkBuilder> = None;
  let mut delta = 0isize;
  for change in changes {
    let merge = current.as_ref().is_some_and(|hunk| change.old_from <= hunk.cursor + 2 * CONTEXT);
    if !merge {
      if let Some(hunk) = current.take() {
        hunks.push(hunk.finish(&lines));
      }
      let lead = change.old_from.saturating_sub(CONTEXT);
      current = Some(HunkBuilder::new(lead, lead.saturating_add_signed(delta)));
    }
    let Some(hunk) = current.as_mut() else { continue };
    hunk.context(&lines, change.old_from);
    hunk.change(&lines, &change, &annotations);
    delta += change.added.len() as isize - change.removed as isize;
  }
  if let Some(hunk) = current {
    hunks.push(hunk.finish(&lines));
  }
  Ok(FileDiff { file: file.clone(), hunks })
}

fn check(file: &FileTarget, original: &str, edits: &[TextEdit]) -> Result<(), PlanViolation> {
  let mut previous_end = 0;
  for (position, edit) in edits.iter().enumerate() {
    if edit.start > edit.end || edit.end > original.len() {
      return Err(PlanViolation::OutOfBounds {
        file: file.clone(),
        start: edit.start,
        end: edit.end,
        len: original.len(),
      });
    }
    if edit.start < previous_end {
      return Err(PlanViolation::Unsorted {
        file: file.clone(),
        index: position,
      });
    }
    if let Some(offset) = [edit.start, edit.end].into_iter().find(|offset| !original.is_char_boundary(*offset)) {
      return Err(PlanViolation::NotCharBoundary { file: file.clone(), offset });
    }
    previous_end = edit.end;
  }
  Ok(())
}

fn changes(original: &str, index: &LineIndex, lines: &[&str], edits: &[TextEdit]) -> Vec<Change> {
  let line_span = |edit: &TextEdit| {
    let first = index.line_of(edit.start);
    let last = if edit.end > edit.start { index.line_of(edit.end - 1) } else { first };
    (first, last)
  };
  let mut groups: Vec<(usize, usize, Vec<&TextEdit>)> = Vec::new();
  for edit in edits {
    let (first, last) = line_span(edit);
    match groups.last_mut() {
      Some((_, group_last, members)) if first <= *group_last => {
        *group_last = (*group_last).max(last);
        members.push(edit);
      }
      _ => groups.push((first, last, vec![edit])),
    }
  }
  groups
    .into_iter()
    .filter_map(|(first, last, members)| {
      let segment_start = index.start(first);
      let segment_end = if last + 1 < index.line_count() { index.start(last + 1) } else { original.len() };
      let mut replaced = String::new();
      let mut cursor = segment_start;
      for edit in members {
        replaced.push_str(&original[cursor..edit.start]);
        replaced.push_str(&edit.new_text);
        cursor = edit.end;
      }
      replaced.push_str(&original[cursor..segment_end]);
      let old: &[&str] = &lines[first.min(lines.len())..(last + 1).min(lines.len())];
      let new: Vec<&str> = replaced.split_inclusive('\n').collect();
      let prefix = old.iter().zip(&new).take_while(|(left, right)| left == right).count();
      let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(left, right)| left == right).count();
      let removed = old.len() - prefix - suffix;
      let added: Vec<String> = new[prefix..new.len() - suffix].iter().map(|line| (*line).to_owned()).collect();
      (removed > 0 || !added.is_empty()).then_some(Change {
        old_from: first + prefix,
        removed,
        added,
      })
    })
    .collect()
}

struct HunkBuilder {
  old_from: usize,
  new_from: usize,
  cursor: usize,
  old_lines: usize,
  new_lines: usize,
  lines: Vec<DiffLine>,
}

impl HunkBuilder {
  fn new(old_from: usize, new_from: usize) -> Self {
    Self {
      old_from,
      new_from,
      cursor: old_from,
      old_lines: 0,
      new_lines: 0,
      lines: Vec::new(),
    }
  }

  fn context(&mut self, lines: &[&str], until: usize) {
    for line in &lines[self.cursor.min(lines.len())..until.min(lines.len())] {
      self.push(LineKind::Context, line, Vec::new());
    }
    self.cursor = self.cursor.max(until);
  }

  fn change(&mut self, lines: &[&str], change: &Change, annotations: &[(usize, &OccurrenceId)]) {
    for (line_number, line) in lines.iter().enumerate().skip(change.old_from).take(change.removed) {
      let ids = annotations.iter().filter(|(annotated, _)| *annotated == line_number).map(|(_, id)| (*id).clone()).collect();
      self.push(LineKind::Removed, line, ids);
    }
    for line in &change.added {
      self.push(LineKind::Added, line, Vec::new());
    }
    self.cursor = change.old_from + change.removed;
  }

  fn push(&mut self, kind: LineKind, text: &str, occurrences: Vec<OccurrenceId>) {
    match kind {
      LineKind::Context => {
        self.old_lines += 1;
        self.new_lines += 1;
      }
      LineKind::Removed => self.old_lines += 1,
      LineKind::Added => self.new_lines += 1,
    }
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    self.lines.push(DiffLine {
      kind,
      text: text.to_owned(),
      occurrences,
    });
  }

  fn finish(mut self, lines: &[&str]) -> Hunk {
    let trailing = (self.cursor + CONTEXT).min(lines.len());
    self.context(lines, trailing);
    Hunk {
      old_start: self.old_from + 1,
      old_lines: self.old_lines,
      new_start: self.new_from + 1,
      new_lines: self.new_lines,
      lines: self.lines,
    }
  }
}
