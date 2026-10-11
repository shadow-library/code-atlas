use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::patch::{OldLineChange, PatchFiles};
use super::score::{BaseFile, F_BETA, counted_overlap, f_beta};
use crate::quality::SetOverlap;

pub type LineSet = BTreeSet<(String, u32)>;

pub fn line_count(bytes: &[u8]) -> u32 {
  let terminated = bytes.iter().filter(|byte| **byte == b'\n').count();
  let unterminated_tail = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
  u32::try_from(terminated + unterminated_tail).unwrap_or(u32::MAX)
}

pub fn gold_lines(changes: &BTreeSet<OldLineChange>, line_count: u32) -> BTreeSet<u32> {
  let removed: BTreeSet<u32> = changes
    .iter()
    .filter_map(|change| match change {
      OldLineChange::Removed { line } => Some(*line),
      OldLineChange::InsertedBetween { .. } => None,
    })
    .collect();
  let anchors = changes
    .iter()
    .filter_map(|change| match *change {
      OldLineChange::InsertedBetween { before, after } if !removed.contains(&before) && !removed.contains(&after) => Some([before, after]),
      _ => None,
    })
    .flatten();
  removed.iter().copied().chain(anchors).filter(|line| (1..=line_count).contains(line)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldFile {
  pub path: String,
  pub lines: usize,
  pub indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LineGoldSummary {
  Unavailable {
    reason: String,
  },
  Available {
    lines: usize,
    lines_outside_index: usize,
    files: Vec<GoldFile>,
    added_files: Vec<String>,
    absent_files: Vec<String>,
  },
}

pub fn line_gold(patch: &PatchFiles, base: impl Fn(&str) -> (BaseFile, u32)) -> (LineSet, LineGoldSummary) {
  let mut gold = LineSet::new();
  let mut files = Vec::new();
  let mut absent_files = Vec::new();
  for (path, changes) in &patch.existing {
    let (state, line_count) = base(path);
    if state == BaseFile::Absent {
      absent_files.push(path.clone());
      continue;
    }
    let lines = gold_lines(changes, line_count);
    files.push(GoldFile {
      path: path.clone(),
      lines: lines.len(),
      indexed: state == BaseFile::Indexed,
    });
    gold.extend(lines.into_iter().map(|line| (path.clone(), line)));
  }
  let summary = LineGoldSummary::Available {
    lines: gold.len(),
    lines_outside_index: files.iter().filter(|file| !file.indexed).map(|file| file.lines).sum(),
    files,
    added_files: patch.added.iter().cloned().collect(),
    absent_files,
  };
  (gold, summary)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemSpans {
  pub lines: LineSet,
  pub items: usize,
  pub without_span: usize,
}

impl ItemSpans {
  pub fn add(&mut self, span: Option<(String, u32, u32)>) {
    self.items += 1;
    match span {
      Some((file, start, end)) => self.lines.extend((start..=end).map(|line| (file.clone(), line))),
      None => self.without_span += 1,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LineChannel {
  pub items: usize,
  pub items_without_span: usize,
  pub lines: SetOverlap,
  pub f05: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LineScores {
  pub must_read: LineChannel,
  pub pack: LineChannel,
}

pub fn score_lines(spans: &ItemSpans, gold: &LineSet) -> LineChannel {
  let lines = counted_overlap(spans.lines.len(), gold.len(), spans.lines.intersection(gold).count());
  LineChannel {
    items: spans.items,
    items_without_span: spans.without_span,
    f05: f_beta(&lines, F_BETA),
    lines,
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;

  use super::*;

  fn removed(line: u32) -> OldLineChange {
    OldLineChange::Removed { line }
  }

  fn inserted(before: u32, after: u32) -> OldLineChange {
    OldLineChange::InsertedBetween { before, after }
  }

  fn spans(items: &[Option<(&str, u32, u32)>]) -> ItemSpans {
    let mut spans = ItemSpans::default();
    for item in items {
      spans.add(item.map(|(file, start, end)| (file.to_owned(), start, end)));
    }
    spans
  }

  #[test]
  fn should_count_a_last_line_without_a_newline() {
    assert_eq!(line_count(b""), 0);
    assert_eq!(line_count(b"a\nb\n"), 2);
    assert_eq!(line_count(b"a\nb"), 2);
    assert_eq!(line_count(b"\n"), 1);
  }

  #[test]
  fn should_take_removed_lines_and_the_neighbours_of_pure_insertions_as_gold() {
    let changes = BTreeSet::from([removed(10), removed(11), inserted(11, 12), inserted(30, 31)]);
    assert_eq!(gold_lines(&changes, 100).into_iter().collect::<Vec<_>>(), vec![10, 11, 30, 31]);
  }

  #[test]
  fn should_drop_insertion_neighbours_outside_the_file() {
    let changes = BTreeSet::from([inserted(0, 1), inserted(5, 6)]);
    assert_eq!(gold_lines(&changes, 5).into_iter().collect::<Vec<_>>(), vec![1, 5]);
    assert_eq!(gold_lines(&BTreeSet::from([inserted(0, 1)]), 0), BTreeSet::new());
  }

  #[test]
  fn should_build_gold_from_base_files_and_set_added_and_absent_files_apart() {
    let patch = PatchFiles {
      existing: BTreeMap::from([
        ("src/a.rs".to_owned(), BTreeSet::from([removed(3), removed(4)])),
        ("CLAUDE.md".to_owned(), BTreeSet::from([removed(7)])),
        ("eval/gone.rs".to_owned(), BTreeSet::from([removed(1)])),
      ]),
      added: BTreeSet::from(["src/new.rs".to_owned()]),
    };
    let (gold, summary) = line_gold(&patch, |path| match path {
      "src/a.rs" => (BaseFile::Indexed, 10),
      "CLAUDE.md" => (BaseFile::Unindexable, 20),
      _ => (BaseFile::Absent, 0),
    });
    assert_eq!(
      gold.into_iter().collect::<Vec<_>>(),
      vec![("CLAUDE.md".to_owned(), 7), ("src/a.rs".to_owned(), 3), ("src/a.rs".to_owned(), 4)]
    );
    assert_eq!(
      summary,
      LineGoldSummary::Available {
        lines: 3,
        lines_outside_index: 1,
        files: vec![
          GoldFile {
            path: "CLAUDE.md".to_owned(),
            lines: 1,
            indexed: false
          },
          GoldFile {
            path: "src/a.rs".to_owned(),
            lines: 2,
            indexed: true
          },
        ],
        added_files: vec!["src/new.rs".to_owned()],
        absent_files: vec!["eval/gone.rs".to_owned()],
      }
    );
  }

  #[test]
  fn should_score_the_union_of_item_spans_against_gold_lines() {
    let gold: LineSet = [("a.rs", 10), ("a.rs", 11), ("a.rs", 12), ("b.rs", 5)]
      .into_iter()
      .map(|(file, line)| (file.to_owned(), line))
      .collect();
    let retrieved = spans(&[Some(("a.rs", 11, 14)), Some(("a.rs", 12, 16)), Some(("c.rs", 1, 2)), None]);
    let channel = score_lines(&retrieved, &gold);
    assert_eq!((channel.items, channel.items_without_span), (4, 1));
    assert_eq!((channel.lines.retrieved, channel.lines.relevant, channel.lines.hits), (8, 4, 2));
    assert_eq!((channel.lines.precision, channel.lines.recall), (Some(0.25), Some(0.5)));
    assert_eq!(channel.f05, Some(1.25 * 2.0 / (0.25 * 4.0 + 8.0)));
  }

  #[test]
  fn should_score_zero_for_a_pack_without_spans_and_nothing_without_gold() {
    let gold: LineSet = [("a.rs".to_owned(), 1)].into();
    let empty = score_lines(&spans(&[None]), &gold);
    assert_eq!((empty.lines.precision, empty.lines.recall, empty.f05), (None, Some(0.0), Some(0.0)));
    let no_gold = score_lines(&spans(&[Some(("a.rs", 1, 3))]), &LineSet::new());
    assert_eq!((no_gold.lines.precision, no_gold.lines.recall, no_gold.f05), (None, None, None));
  }
}
