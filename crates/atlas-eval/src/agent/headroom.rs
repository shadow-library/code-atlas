use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::diff::Diff;
use super::error::{AgentError, AgentResult};
use super::plan::Condition;
use super::provenance::{EntrySource, SourceReach, TaskReach, Wording, provenance, source_reach};
use super::record::{DIFF_FILE, OFFSETS_FILE, read_json, write_json_dense};
use super::report::{RunRecord, is_scored, load_runs, number};
use super::spec::{GroundTruth, LoadedTask, RepoSource, load_tasks};
use super::touch::{FileTouches, Ordinal, Presence, SessionTouches, TouchDiagnostics, TouchEvent, session_touches};
use super::transcript::Transcript;
use super::tree_index::TreeIndex;
use super::workspace::{StripPlan, TreeSource, materialize_task};

pub const WITHIN_LIMITS: [u32; 4] = [1, 2, 3, 5];
const EARLY_READ_TURN: u32 = 2;
const BASELINE_GROUP: &str = "baseline";
const GROUP_ORDER: [&str; 3] = [BASELINE_GROUP, "atlas (standard)", "atlas (directed)"];
const INSTRUCTIONS_FLAG: &str = "--append-system-prompt";
const PROMPT_FLAG: &str = "-p";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SessionShare {
  pub turns: f64,
  pub tool_calls: f64,
  pub time: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetHeadroom {
  pub files_at_base: usize,
  pub created: Vec<String>,
  pub first_named: Option<Ordinal>,
  pub first_addressed: Option<Ordinal>,
  pub first_seen: Option<Ordinal>,
  pub first_searched: Option<Ordinal>,
  pub first_read: Option<Ordinal>,
  pub first_touched: Option<Ordinal>,
  pub first_edited: Option<Ordinal>,
  pub first_read_via: Option<String>,
  pub before_first_read: Option<SessionShare>,
  pub all_read: Option<Ordinal>,
  pub all_touched: Option<Ordinal>,
  pub turns_before_all_touched: Option<f64>,
  #[serde(skip)]
  pub file_reads: Vec<Ordinal>,
  #[serde(skip)]
  pub file_touches: Vec<Ordinal>,
  pub edited_without_read: Vec<String>,
  pub edited_unseen: Vec<String>,
  pub without_read: Vec<String>,
  pub without_touch: Vec<String>,
}

impl SetHeadroom {
  fn of(paths: &[String], touches: &SessionTouches, session_ms: Option<u64>) -> Self {
    let files: Vec<&FileTouches> = touches.files.iter().filter(|file| paths.contains(&file.path)).collect();
    let at_base: Vec<&FileTouches> = files.iter().copied().filter(|file| file.presence == Presence::AtBase).collect();
    let events = |pick: fn(&FileTouches) -> Option<&TouchEvent>| at_base.iter().filter_map(|file| pick(file)).collect::<Vec<_>>();
    let first = |pick: fn(&FileTouches) -> Option<&TouchEvent>| events(pick).into_iter().min_by_key(|event| event.at);
    let every = |pick: fn(&FileTouches) -> Option<&TouchEvent>| {
      let found = events(pick);
      (found.len() == at_base.len()).then(|| found.into_iter().map(|event| event.at).max()).flatten()
    };
    let ordinals = |pick: fn(&FileTouches) -> Option<&TouchEvent>| events(pick).into_iter().map(|event| event.at).collect::<Vec<_>>();
    let listed = |keep: fn(&FileTouches) -> bool| at_base.iter().filter(|file| keep(file)).map(|file| file.path.clone()).collect::<Vec<_>>();
    let turn_share = |ordinal: Ordinal| f64::from(ordinal.turn.saturating_sub(1)) / f64::from(touches.turns.max(1));
    let first_read = first(|file| file.read.as_ref());
    let before_first_read = first_read.map(|event| SessionShare {
      turns: turn_share(event.at),
      tool_calls: f64::from(event.at.call.saturating_sub(1)) / f64::from(touches.tool_calls.max(1)),
      time: event
        .elapsed_ms
        .zip(session_ms.filter(|total| *total > 0))
        .map(|(elapsed, total)| (elapsed as f64 / total as f64).min(1.0)),
    });
    let first_addressed = first(FileTouches::addressed).map(|event| event.at);
    let first_named = first(FileTouches::named_before_addressed).map(|event| event.at);
    let all_touched = every(FileTouches::touched);
    Self {
      files_at_base: at_base.len(),
      created: files.iter().filter(|file| file.presence == Presence::Created).map(|file| file.path.clone()).collect(),
      first_named: first_named.filter(|named| first_addressed.is_none_or(|addressed| *named < addressed)),
      first_addressed,
      first_seen: first(FileTouches::seen).map(|event| event.at),
      first_searched: first(|file| file.searched.as_ref()).map(|event| event.at),
      first_read: first_read.map(|event| event.at),
      first_touched: first(FileTouches::touched).map(|event| event.at),
      first_edited: first(|file| file.edited.as_ref()).map(|event| event.at),
      first_read_via: first_read.map(|event| event.via.clone()),
      before_first_read,
      all_read: every(|file| file.read.as_ref()),
      all_touched,
      turns_before_all_touched: all_touched.map(turn_share),
      file_reads: ordinals(|file| file.read.as_ref()),
      file_touches: ordinals(FileTouches::touched),
      edited_without_read: listed(|file| edited_before(file, &[&file.read])),
      edited_unseen: listed(|file| edited_before(file, &[&file.read, &file.searched])),
      without_read: listed(|file| file.read.is_none()),
      without_touch: listed(|file| file.touched().is_none()),
    }
  }

  fn addressed_before_named(&self) -> bool {
    self.first_addressed.is_some() && self.first_named.is_none()
  }
}

fn edited_before(file: &FileTouches, sightings: &[&Option<TouchEvent>]) -> bool {
  let Some(edited) = &file.edited else {
    return false;
  };
  !sightings.iter().any(|sighting| sighting.as_ref().is_some_and(|event| event.at.call < edited.at.call))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditCheck {
  pub edited_but_unchanged: Vec<String>,
  pub changed_without_edit_event: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
  Search,
  Opened,
}

impl EntryKind {
  fn label(self) -> &'static str {
    match self {
      Self::Search => "a search that named it, using",
      Self::Opened => "opening it with no earlier naming, its path containing",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
  #[serde(flatten)]
  pub at: Ordinal,
  pub kind: EntryKind,
  pub source: EntrySource,
  pub shared: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionHeadroom {
  pub batch: String,
  pub run_id: String,
  pub task_id: String,
  pub group: String,
  pub run_index: u32,
  pub turns: u32,
  pub tool_calls: u32,
  pub reported_num_turns: Option<u64>,
  pub session_ms: Option<u64>,
  pub must_change: SetHeadroom,
  pub relevant: SetHeadroom,
  pub entry: Option<Entry>,
  pub files: Vec<FileTouches>,
  pub diagnostics: TouchDiagnostics,
  pub edit_check: EditCheck,
}

pub struct SessionInput<'a> {
  pub batch: &'a str,
  pub record: &'a RunRecord,
  pub transcript: &'a Transcript,
  pub tree: TreeIndex,
  pub truth: &'a GroundTruth,
  pub changed_files: &'a BTreeSet<String>,
}

pub fn group_of(record: &RunRecord) -> String {
  match record.manifest.condition {
    Condition::Baseline => BASELINE_GROUP.to_owned(),
    Condition::Atlas => format!("atlas ({})", record.manifest.preamble_variant.unwrap_or_default()),
  }
}

fn argument<'a>(record: &'a RunRecord, flag: &str) -> &'a str {
  let args = &record.manifest.args;
  args.iter().position(|arg| arg == flag).and_then(|index| args.get(index + 1)).map_or("", String::as_str)
}

fn entry_of(set: &SetHeadroom, paths: &[String], touches: &SessionTouches, record: &RunRecord) -> Option<Entry> {
  let (at, kind, query) = match (set.first_named, set.first_addressed) {
    (Some(named), _) => {
      let query = touches.queries.iter().find(|query| query.call == named.call).map(|query| query.text.clone());
      (named, EntryKind::Search, query.unwrap_or_default())
    }
    (None, Some(addressed)) => {
      let opened = touches
        .files
        .iter()
        .find(|file| paths.contains(&file.path) && file.addressed().is_some_and(|event| event.at == addressed));
      (addressed, EntryKind::Opened, opened.map(|file| file.path.clone()).unwrap_or_default())
    }
    (None, None) => return None,
  };
  let (source, shared) = provenance(&query, &Wording::of(argument(record, PROMPT_FLAG)), &Wording::of(argument(record, INSTRUCTIONS_FLAG)));
  Some(Entry { at, kind, source, shared })
}

pub fn analyze_session(input: SessionInput<'_>) -> SessionHeadroom {
  let manifest = &input.record.manifest;
  let mut tracked = input.truth.must_change.clone();
  tracked.extend(input.truth.relevant_files.iter().filter(|path| !input.truth.must_change.contains(path)).cloned());
  let touches = session_touches(input.transcript, input.tree, &tracked);
  let session_ms = input.transcript.result.as_ref().and_then(|result| result.duration_ms);
  let edited: BTreeSet<&String> = touches.files.iter().filter(|file| file.edited.is_some()).map(|file| &file.path).collect();
  let edit_check = EditCheck {
    edited_but_unchanged: tracked
      .iter()
      .filter(|path| edited.contains(path) && !input.changed_files.contains(*path))
      .cloned()
      .collect(),
    changed_without_edit_event: tracked
      .iter()
      .filter(|path| !edited.contains(path) && input.changed_files.contains(*path))
      .cloned()
      .collect(),
  };
  let must_change = SetHeadroom::of(&input.truth.must_change, &touches, session_ms);
  SessionHeadroom {
    batch: input.batch.to_owned(),
    run_id: manifest.run_id.clone(),
    task_id: manifest.task_id.clone(),
    group: group_of(input.record),
    run_index: manifest.run_index,
    turns: touches.turns,
    tool_calls: touches.tool_calls,
    reported_num_turns: input.transcript.result.as_ref().and_then(|result| result.num_turns),
    session_ms,
    entry: entry_of(&must_change, &input.truth.must_change, &touches, input.record),
    must_change,
    relevant: SetHeadroom::of(&input.truth.relevant_files, &touches, session_ms),
    diagnostics: touches.diagnostics,
    files: touches.files,
    edit_check,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Summary {
  pub median: f64,
  pub min: f64,
  pub max: f64,
  pub n: usize,
}

impl Summary {
  pub fn of(values: &[f64]) -> Option<Self> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let (first, last) = (*sorted.first()?, *sorted.last()?);
    let middle = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
      sorted[middle]
    } else {
      (sorted[middle - 1] + sorted[middle]) / 2.0
    };
    Some(Self {
      median,
      min: first,
      max: last,
      n: sorted.len(),
    })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
  Ordinal,
  Share,
}

struct Measure {
  name: &'static str,
  label: &'static str,
  unit: Unit,
  extract: fn(&SetHeadroom) -> Option<f64>,
}

fn turn(ordinal: Option<Ordinal>) -> Option<f64> {
  ordinal.map(|ordinal| f64::from(ordinal.turn))
}

fn call(ordinal: Option<Ordinal>) -> Option<f64> {
  ordinal.map(|ordinal| f64::from(ordinal.call))
}

const MEASURES: [Measure; 20] = [
  Measure {
    name: "first_named_turn",
    label: "first file named in a result, turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_named),
  },
  Measure {
    name: "first_named_tool_call",
    label: "first file named in a result, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.first_named),
  },
  Measure {
    name: "first_seen_turn",
    label: "first lines of a file seen (read, or shown in a result), turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_seen),
  },
  Measure {
    name: "first_searched_turn",
    label: "first search addressed to a file, turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_searched),
  },
  Measure {
    name: "first_searched_tool_call",
    label: "first search addressed to a file, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.first_searched),
  },
  Measure {
    name: "first_read_turn",
    label: "first file read, turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_read),
  },
  Measure {
    name: "first_read_tool_call",
    label: "first file read, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.first_read),
  },
  Measure {
    name: "first_touched_turn",
    label: "first file touched (search, read or edit), turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_touched),
  },
  Measure {
    name: "first_touched_tool_call",
    label: "first file touched (search, read or edit), tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.first_touched),
  },
  Measure {
    name: "first_edited_turn",
    label: "first file edited, turn",
    unit: Unit::Ordinal,
    extract: |set| turn(set.first_edited),
  },
  Measure {
    name: "first_edited_tool_call",
    label: "first file edited, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.first_edited),
  },
  Measure {
    name: "all_read_turn",
    label: "every file read, turn of the last one's first read",
    unit: Unit::Ordinal,
    extract: |set| turn(set.all_read),
  },
  Measure {
    name: "all_read_tool_call",
    label: "every file read, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.all_read),
  },
  Measure {
    name: "all_touched_turn",
    label: "every file touched, turn of the last one's first touch",
    unit: Unit::Ordinal,
    extract: |set| turn(set.all_touched),
  },
  Measure {
    name: "all_touched_tool_call",
    label: "every file touched, tool call",
    unit: Unit::Ordinal,
    extract: |set| call(set.all_touched),
  },
  Measure {
    name: "share_before_first_read_turns",
    label: "share of the session before the first read, turns",
    unit: Unit::Share,
    extract: |set| set.before_first_read.map(|share| share.turns),
  },
  Measure {
    name: "share_before_first_read_tool_calls",
    label: "share of the session before the first read, tool calls",
    unit: Unit::Share,
    extract: |set| set.before_first_read.map(|share| share.tool_calls),
  },
  Measure {
    name: "share_before_first_read_time",
    label: "share of the session before the first read, time",
    unit: Unit::Share,
    extract: |set| set.before_first_read.and_then(|share| share.time),
  },
  Measure {
    name: "share_before_all_touched_turns",
    label: "share of the session before the last file's first touch, turns",
    unit: Unit::Share,
    extract: |set| set.turns_before_all_touched,
  },
  Measure {
    name: "files_at_base",
    label: "files of the set that exist at base",
    unit: Unit::Ordinal,
    extract: |set| Some(set.files_at_base as f64),
  },
];

const FIRST_READ_TURN: &str = "first_read_turn";
const ALL_READ_TURN: &str = "all_read_turn";
const ALL_TOUCHED_TURN: &str = "all_touched_turn";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Within {
  pub limit: u32,
  pub by_turn: usize,
  pub by_tool_call: usize,
}

fn within(ordinals: &[Option<Ordinal>]) -> Vec<Within> {
  WITHIN_LIMITS
    .iter()
    .map(|limit| Within {
      limit: *limit,
      by_turn: ordinals.iter().flatten().filter(|ordinal| ordinal.turn <= *limit).count(),
      by_tool_call: ordinals.iter().flatten().filter(|ordinal| ordinal.call <= *limit).count(),
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetAggregate {
  pub sessions: usize,
  pub sessions_with_several_files: usize,
  pub measures: BTreeMap<String, Summary>,
  pub read_within: Vec<Within>,
  pub touched_within: Vec<Within>,
  pub seen_within: Vec<Within>,
  pub all_read_within: Vec<Within>,
  pub all_touched_within: Vec<Within>,
  pub files: usize,
  pub files_read: usize,
  pub files_touched: usize,
  pub files_read_within: Vec<Within>,
  pub files_touched_within: Vec<Within>,
  pub sessions_without_read: usize,
  pub sessions_without_touch: usize,
  pub sessions_addressing_before_named: usize,
  pub sessions_editing_without_read: usize,
  pub sessions_editing_unseen: usize,
  pub sessions_with_files_without_touch: usize,
}

impl SetAggregate {
  fn of(sets: &[&SetHeadroom]) -> Self {
    let sets: Vec<&SetHeadroom> = sets.iter().copied().filter(|set| set.files_at_base > 0).collect();
    let count = |keep: fn(&SetHeadroom) -> bool| sets.iter().filter(|set| keep(set)).count();
    let per_session = |pick: fn(&SetHeadroom) -> Option<Ordinal>| within(&sets.iter().map(|set| pick(set)).collect::<Vec<_>>());
    let per_file = |pick: fn(&SetHeadroom) -> &Vec<Ordinal>| sets.iter().flat_map(|set| pick(set).iter().copied().map(Some)).collect::<Vec<_>>();
    let (reads, touches) = (per_file(|set| &set.file_reads), per_file(|set| &set.file_touches));
    Self {
      sessions: sets.len(),
      sessions_with_several_files: count(|set| set.files_at_base > 1),
      measures: MEASURES
        .iter()
        .filter_map(|measure| {
          let values: Vec<f64> = sets.iter().filter_map(|set| (measure.extract)(set)).collect();
          Summary::of(&values).map(|summary| (measure.name.to_owned(), summary))
        })
        .collect(),
      read_within: per_session(|set| set.first_read),
      touched_within: per_session(|set| set.first_touched),
      seen_within: per_session(|set| set.first_seen),
      all_read_within: per_session(|set| set.all_read),
      all_touched_within: per_session(|set| set.all_touched),
      files: sets.iter().map(|set| set.files_at_base).sum(),
      files_read: reads.len(),
      files_touched: touches.len(),
      files_read_within: within(&reads),
      files_touched_within: within(&touches),
      sessions_without_read: count(|set| set.first_read.is_none()),
      sessions_without_touch: count(|set| set.first_touched.is_none()),
      sessions_addressing_before_named: count(SetHeadroom::addressed_before_named),
      sessions_editing_without_read: count(|set| !set.edited_without_read.is_empty()),
      sessions_editing_unseen: count(|set| !set.edited_unseen.is_empty()),
      sessions_with_files_without_touch: count(|set| !set.without_touch.is_empty()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryCount {
  pub kind: EntryKind,
  pub source: EntrySource,
  pub sessions: usize,
  pub reading_early: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupHeadroom {
  pub group: String,
  pub sessions: usize,
  pub turns: Option<Summary>,
  pub tool_calls: Option<Summary>,
  pub must_change: SetAggregate,
  pub relevant: SetAggregate,
  pub entries: Vec<EntryCount>,
  pub sessions_without_entry: usize,
  pub sessions_reading_early: usize,
}

fn reads_early(session: &SessionHeadroom) -> bool {
  session.must_change.first_read.is_some_and(|read| read.turn <= EARLY_READ_TURN)
}

impl GroupHeadroom {
  fn of(group: &str, sessions: &[&SessionHeadroom]) -> Self {
    let totals = |pick: fn(&SessionHeadroom) -> u32| Summary::of(&sessions.iter().map(|session| f64::from(pick(session))).collect::<Vec<_>>());
    let mut entries: BTreeMap<(EntryKind, EntrySource), (usize, usize)> = BTreeMap::new();
    for (session, entry) in sessions.iter().filter_map(|session| session.entry.as_ref().map(|entry| (session, entry))) {
      let counts = entries.entry((entry.kind, entry.source)).or_default();
      counts.0 += 1;
      counts.1 += usize::from(reads_early(session));
    }
    Self {
      group: group.to_owned(),
      sessions: sessions.len(),
      turns: totals(|session| session.turns),
      tool_calls: totals(|session| session.tool_calls),
      must_change: SetAggregate::of(&sessions.iter().map(|session| &session.must_change).collect::<Vec<_>>()),
      relevant: SetAggregate::of(&sessions.iter().map(|session| &session.relevant).collect::<Vec<_>>()),
      entries: entries
        .into_iter()
        .map(|((kind, source), (sessions, reading_early))| EntryCount {
          kind,
          source,
          sessions,
          reading_early,
        })
        .collect(),
      sessions_without_entry: sessions.iter().filter(|session| session.entry.is_none()).count(),
      sessions_reading_early: sessions.iter().filter(|session| reads_early(session)).count(),
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFacts {
  pub repository: String,
  pub reach: TaskReach,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskHeadroom {
  pub task_id: String,
  pub facts: Option<TaskFacts>,
  pub groups: Vec<GroupHeadroom>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadroomBatch {
  pub name: String,
  pub date: Option<String>,
  pub sessions: usize,
  pub excluded: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskShare {
  pub limit: u32,
  pub first_read: usize,
  pub all_read: usize,
  pub all_touched: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeadroomReport {
  pub batches: Vec<HeadroomBatch>,
  pub overall: Vec<GroupHeadroom>,
  pub tasks: Vec<TaskHeadroom>,
  pub baseline_tasks: usize,
  pub baseline_tasks_with_median_within: Vec<TaskShare>,
  pub sessions: Vec<SessionHeadroom>,
  pub notes: Vec<String>,
}

fn groups_of(sessions: &[&SessionHeadroom]) -> Vec<GroupHeadroom> {
  let rank = |name: &str| GROUP_ORDER.iter().position(|known| *known == name).unwrap_or(GROUP_ORDER.len());
  let mut names: Vec<&str> = sessions.iter().map(|session| session.group.as_str()).collect();
  names.sort_by_key(|name| (rank(name), *name));
  names.dedup();
  names
    .into_iter()
    .map(|name| GroupHeadroom::of(name, &sessions.iter().copied().filter(|session| session.group == name).collect::<Vec<_>>()))
    .collect()
}

fn median_within(group: &GroupHeadroom, measure: &str, limit: u32) -> bool {
  let every_session = group.must_change.measures.get(measure).is_some_and(|summary| summary.n == group.must_change.sessions);
  every_session && group.must_change.measures.get(measure).is_some_and(|summary| summary.median <= f64::from(limit))
}

pub fn build_headroom(batches: Vec<HeadroomBatch>, sessions: Vec<SessionHeadroom>, facts: &BTreeMap<String, TaskFacts>, notes: Vec<String>) -> HeadroomReport {
  let all: Vec<&SessionHeadroom> = sessions.iter().collect();
  let task_ids: BTreeSet<&str> = sessions.iter().map(|session| session.task_id.as_str()).collect();
  let tasks: Vec<TaskHeadroom> = task_ids
    .into_iter()
    .map(|task_id| TaskHeadroom {
      task_id: task_id.to_owned(),
      facts: facts.get(task_id).cloned(),
      groups: groups_of(&all.iter().copied().filter(|session| session.task_id == task_id).collect::<Vec<_>>()),
    })
    .collect();
  let baseline: Vec<&GroupHeadroom> = tasks.iter().filter_map(|task| task.groups.iter().find(|group| group.group == BASELINE_GROUP)).collect();
  let tasks_within = |measure: &str, limit: u32| baseline.iter().filter(|group| median_within(group, measure, limit)).count();
  let within = WITHIN_LIMITS
    .iter()
    .map(|limit| TaskShare {
      limit: *limit,
      first_read: tasks_within(FIRST_READ_TURN, *limit),
      all_read: tasks_within(ALL_READ_TURN, *limit),
      all_touched: tasks_within(ALL_TOUCHED_TURN, *limit),
    })
    .collect();
  HeadroomReport {
    batches,
    overall: groups_of(&all),
    baseline_tasks: baseline.len(),
    baseline_tasks_with_median_within: within,
    tasks,
    sessions,
    notes,
  }
}

pub(super) fn read_tree(dir: &Path, prefix: &str, contents: &mut BTreeMap<String, String>) -> AgentResult<()> {
  let entries = fs::read_dir(dir).map_err(|error| AgentError::io(dir, error))?;
  for entry in entries {
    let entry = entry.map_err(|error| AgentError::io(dir, error))?;
    let name = entry.file_name().to_string_lossy().into_owned();
    let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
    let kind = entry.file_type().map_err(|error| AgentError::io(entry.path(), error))?;
    if kind.is_dir() {
      read_tree(&entry.path(), &relative, contents)?;
    } else if kind.is_file() {
      let bytes = fs::read(entry.path()).map_err(|error| AgentError::io(entry.path(), error))?;
      contents.insert(relative, String::from_utf8_lossy(&bytes).into_owned());
    }
  }
  Ok(())
}

fn base_contents(record: &RunRecord, task: &LoadedTask) -> AgentResult<BTreeMap<String, String>> {
  let scratch = tempfile::tempdir().map_err(|error| AgentError::io(std::env::temp_dir(), error))?;
  let tree = scratch.path().join("tree");
  let overridden = matches!(record.manifest.source, TreeSource::Directory { .. });
  materialize_task(task, &record.manifest.source, &tree, &StripPlan::for_task(task, overridden))?;
  let mut contents = BTreeMap::new();
  read_tree(&tree, "", &mut contents)?;
  Ok(contents)
}

fn repository_of(task: &LoadedTask) -> String {
  match &task.spec.repo {
    RepoSource::SelfRepo => "this repository".to_owned(),
    RepoSource::Fixture { name } => format!("fixture `{name}` of this repository"),
    RepoSource::Git { url, .. } => url.clone(),
  }
}

pub fn task_facts(record: &RunRecord, repository: String, contents: &BTreeMap<String, String>, truth: &GroundTruth) -> TaskFacts {
  let instructions = argument(record, INSTRUCTIONS_FLAG);
  TaskFacts {
    repository,
    reach: TaskReach {
      prompt: source_reach(argument(record, PROMPT_FLAG), contents, &truth.must_change),
      instructions: (!instructions.is_empty()).then(|| source_reach(instructions, contents, &truth.must_change)),
    },
  }
}

pub fn load_headroom(batch_dirs: &[PathBuf], tasks_dir: &Path) -> AgentResult<HeadroomReport> {
  let tasks = load_tasks(tasks_dir, &[])?;
  let mut batches = Vec::new();
  let mut sessions = Vec::new();
  let mut notes = BTreeSet::new();
  let mut trees: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
  let mut described: BTreeMap<String, (RunRecord, (String, String))> = BTreeMap::new();
  for batch_dir in batch_dirs {
    let name = batch_dir.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned();
    let (manifest, records) = load_runs(batch_dir)?;
    let mut excluded = Vec::new();
    let before = sessions.len();
    for record in &records {
      if !is_scored(record) {
        excluded.push(record.manifest.run_id.clone());
        continue;
      }
      let task = tasks
        .iter()
        .find(|task| task.spec.id == record.manifest.task_id)
        .ok_or_else(|| AgentError::UnknownTask(record.manifest.task_id.clone()))?;
      if matches!(record.manifest.source, TreeSource::Directory { .. }) {
        notes.insert(format!(
          "Batch `{name}` copied its trees from an override directory; the base tree is read from that directory as it is now, not as it was when the batch ran."
        ));
      }
      if task.spec_hash != record.manifest.spec_hash {
        notes.insert(format!(
          "`{}`: the task spec changed since batch `{name}` ran (spec hash {} then, {} now); ground truth is read from the current spec.",
          task.spec.id, record.manifest.spec_hash, task.spec_hash
        ));
      }
      let recorded = manifest.as_ref().and_then(|batch| batch.truths.iter().find(|(id, _)| *id == task.spec.id));
      if recorded.is_some_and(|(_, truth)| truth.must_change != task.spec.ground_truth.must_change || truth.relevant_files != task.spec.ground_truth.relevant_files) {
        notes.insert(format!(
          "`{}`: `must_change` or `relevant_files` differ from the ground truth recorded in batch `{name}`.",
          task.spec.id
        ));
      }
      let run_dir = batch_dir.join(&record.transcript).parent().map(Path::to_owned).unwrap_or_else(|| batch_dir.clone());
      let transcript_path = batch_dir.join(&record.transcript);
      let text = fs::read_to_string(&transcript_path).map_err(|error| AgentError::io(&transcript_path, error))?;
      let offsets_path = run_dir.join(OFFSETS_FILE);
      let offsets: Vec<u64> = if offsets_path.is_file() { read_json(&offsets_path)? } else { Vec::new() };
      let transcript = Transcript::parse(&text, &offsets);
      let diff_path = run_dir.join(DIFF_FILE);
      let changed_files = fs::read_to_string(&diff_path).map(|text| Diff::parse(&text).changed_files()).unwrap_or_default();
      let source = serde_json::to_string(&record.manifest.source).unwrap_or_default();
      let key = (task.spec.id.clone(), source);
      if !trees.contains_key(&key) {
        trees.insert(key.clone(), base_contents(record, task)?);
      }
      let contents = &trees[&key];
      let unprefixed = record.manifest.condition == Condition::Baseline;
      if !described
        .get(&task.spec.id)
        .is_some_and(|(chosen, _)| chosen.manifest.condition == Condition::Baseline || !unprefixed)
      {
        described.insert(task.spec.id.clone(), (record.clone(), key.clone()));
      }
      let roots = record.manifest.tree.iter().cloned().chain(transcript.init.as_ref().and_then(|init| init.cwd.clone()));
      sessions.push(analyze_session(SessionInput {
        batch: &name,
        record,
        transcript: &transcript,
        tree: TreeIndex::new(roots, contents.keys().cloned()),
        truth: &task.spec.ground_truth,
        changed_files: &changed_files,
      }));
    }
    batches.push(HeadroomBatch {
      name,
      date: manifest.map(|batch| batch.environment.date),
      sessions: sessions.len() - before,
      excluded,
    });
  }
  let facts: BTreeMap<String, TaskFacts> = tasks
    .iter()
    .filter_map(|task| {
      let (record, key) = described.get(&task.spec.id)?;
      Some((task.spec.id.clone(), task_facts(record, repository_of(task), trees.get(key)?, &task.spec.ground_truth)))
    })
    .collect();
  Ok(build_headroom(batches, sessions, &facts, notes.into_iter().collect()))
}

fn percent(value: f64) -> String {
  format!("{}%", number((value * 1000.0).round() / 10.0))
}

fn ordinal_cell(summary: Option<&Summary>, sessions: usize, unit: Unit) -> String {
  let show = |value: f64| match unit {
    Unit::Ordinal => number(value),
    Unit::Share => percent(value),
  };
  match summary {
    None => format!("– (n=0 of {sessions})"),
    Some(summary) if summary.n == sessions => format!("{} ({}–{}, n={})", show(summary.median), show(summary.min), show(summary.max), summary.n),
    Some(summary) => format!("{} ({}–{}, n={} of {sessions})", show(summary.median), show(summary.min), show(summary.max), summary.n),
  }
}

fn share_cell(count: usize, of: usize) -> String {
  if of == 0 {
    return "–".to_owned();
  }
  format!("{count}/{of} ({})", percent(count as f64 / of as f64))
}

fn pair(ordinal: Option<Ordinal>) -> String {
  ordinal.map_or_else(|| "–".to_owned(), |ordinal| format!("{}/{}", ordinal.turn, ordinal.call))
}

const SUPPORTS: &str = "\
- **First-file rows** say when a session has an entry point: one file of the change set has been read or touched. They say nothing about the rest of the change set. For a task with one `must_change` file they are the whole answer; for the others they are the lower end.
- **Every-file rows** say when the whole change set has been located: the turn at which the last `must_change` file is first read, or first touched. **Pooled file rows** count files instead of sessions, so a task with many files weighs more.
- **Seen** is wider than read: it also counts lines of a file that came back in a search result. Where a session edits from such lines it never issues a read, and the read rows understate what it saw.
- **Where the entry point comes from** limits what an early first read can be credited to. When the first search uses a word the prompt wrote down, the early read shows that the task hands over its entry point, not that the agent located code quickly.
- None of these rows shows whether a context tool would shorten a session, and none shows that it could not. They show how much of a session lies before the entry point and before the complete change set in sessions that had no such tool.";

const DEFINITIONS: &str = "\
- **Session.** One recorded run (`<task>.<condition>.r<i>`) that completed and is not contaminated, the same filter `agent report` applies.
- **Turn.** One model response: one distinct assistant `message.id` in `transcript.jsonl`. A response may issue several tool calls in parallel; all of them carry that turn's ordinal. Turn 1 is the first response after the prompt, and the final answer is a turn too. Results of calls issued in turn N reach the model before turn N+1.
- **Tool-call ordinal.** The 1-based position of a tool call in the transcript. The `num_turns` that `agent report` shows as \"turns\" is on this scale, not on the turn scale (see the note under the headline).
- **Event ordinals.** An event belongs to the call that issued it. \"By turn N\" means that call was issued in turn N or earlier, so the model has its result from turn N+1 on.
- **(a) Named.** The file's path appears in the result of a Grep, Glob, Bash or MCP call whose own input did not name the file. Results of Read, Edit and Write are not searched (they are file content or echo the path the agent gave). A path in a result counts when it resolves to the file from the tree root, from a working directory of the shell during that call, from a directory passed to `ls`, or from a `dir:` header line of the output. Per file, a naming is reported only if it comes before the agent first addressed that file; for a file set, only if it comes before the agent first addressed any file of the set.
- **Shown.** A result carries lines of the file: the path is followed by a line number (`path:12:` or `path-12-`, the form of grep output with content), or the search was addressed to the file and asked for content. For Code Atlas results every located item counts, whether or not its body was included.
- **Searched.** A search addressed to that one file: the Grep tool with `path` equal to the file, or `grep`, `egrep`, `fgrep`, `rg`, `ag`, `ack` or `git grep` in Bash with the file written as an operand. A search over a directory, or over a glob that the shell expands, is not addressed to one file; it can only name or show it.
- **(b) Read.** A Read call that did not fail, or `cat`, `head`, `tail` or `sed -n` in Bash with the file as an operand, written out or matched by a glob. These are the forms `agent report` counts as files read (`agent/bash.rs`); unlike that metric, the shell's working directory is followed across calls here.
- **Seen.** The earliest of shown and read: the first time any line of the file reached the model.
- **(c) Edited.** An Edit, Write, MultiEdit or NotebookEdit call that did not fail, or in Bash: `sed -i` (not counted when sed rejected the command, which BSD sed does with GNU syntax), `perl -i … -e`, output redirected or piped through `tee` into the file, or a Python script (`python3 - <<EOF` or `python3 -c`) that writes files and names the file in a string literal.
- **Touched.** The earliest of searched, read and edited: the first call the agent addressed to the file in a recognised form. A file with no touch has no recognised touch; the session may still have changed it in a form this analysis does not recognise, and the detection checks list those files.
- **Mentioned.** The call's input names the file (an operand that resolves to it, or its shortest path suffix that is unique in the base tree) in a form not listed above, for example as a test-runner argument, in the arguments of an MCP call, or in a call that failed. It is a diagnostic and never counts as a touch. Addressed is the earliest of mentioned and touched.
- **File sets.** `must_change` and `relevant_files` from `eval/tasks/<task>/task.toml`. A file absent from the base tree (the task creates it) has nothing to find: it is listed separately and excluded from every statistic. The base tree is the archived tree of the run with the stripped paths removed and the task's `base.patch` applied.
- **First file and every file.** For a file set, a first event is the earliest over its files. \"Every file read\" is defined only when each file of the set was read, and is the first read of the last of them; \"every file touched\" likewise, so it is the turn of the last file's first touch. Pooled file counts take each file of each session once.
- **Share of the session before an event.** By turns: (turn − 1) / turns; by tool calls: (call − 1) / tool calls; by time: the transcript offset of the call divided by the session duration.
- **Terms that reach a file.** From the prompt of a baseline session (an atlas prompt carries the preamble as well), and separately from the project instructions the harness injects with `--append-system-prompt`, these terms are taken: each code span, each identifier-shaped token (an underscore, an inner capital, `::`, `/` or a hyphen) and its parts, each word of four or more characters inside a code span or block, and each double-quoted phrase. A term reaches a file when the file contains it at the base commit as a fixed string, the result of one `grep -F`. Reported per task: how many terms were checked, how many return at least one `must_change` file, and for the most selective ones how many files they return.
- **Entry point of a session.** The call that first named a `must_change` file, or, when the agent addressed one before any result named it, that first address. Its text is the search pattern (Grep `pattern` and `glob`, the Glob pattern, the pattern of a `grep` in Bash or else the whole command, the arguments of an MCP call) or the path the agent opened. The text is compared with the prompt and with the injected instructions, case-insensitively, in this order: a whole word or identifier (letters, digits, underscore; at least four characters; language keywords and a few generic words such as `index` or `tests` excluded) shared with the prompt; with the instructions; a part of such a word (split at underscores and inner capitals) shared with the prompt; with the instructions; neither. The first that holds is the source. This is a lexical comparison, so it shows that the words were available, not what the model did with them.";

const BASH_LIMITS: &str = "\
Bash commands are interpreted lexically, never executed. Recognised: `;`, `&&`, `||`, `|`, newlines, subshells, quoting, output redirects, here-documents, `for x in a b; do …; done` over literal lists, variables assigned in the same command, brace lists, and globs expanded against the base tree plus the files the session created with Write, a redirect, `tee` or `touch`.

The working directory is followed the way Claude Code keeps it. A command's final directory carries over to the next call only if the command exits 0; the transcript records that per call (an error result, a `returnCodeInterpretation` such as \"No matches found\", or a command moved to the background, whose `backgroundCwdHint` states the directory). A `Shell cwd was reset to …` notice and the directory named in a failed Edit are honoured. A `cd` into a directory that the tree model does not know is taken to have failed when the output says so, or when the command hid its errors; the commands chained to it with `&&` are then skipped. Otherwise the directory is taken to exist (build output, installed packages).

Not recognised, so a touch made only this way is missed or shows up as a mention: scripts run from a file (`python3 fix.py a b`), `awk`, `less`, `nl`, `wc`, `diff`, `cp`, `mv`, `rm`, `patch`, `git checkout`/`restore`/`apply`/`stash`, formatters that rewrite files (`cargo fmt`, `prettier --write`), command substitution, parameter expansion with operators (`${f%:*}`), variables set in an earlier call, `xargs`, `find -exec`, `while read` loops, and Python that builds a path at run time or only reads a file. A Python script that writes is taken to edit every existing file that one of its string literals names, also as a whitespace-separated word, relative to the shell's directory or to a literal `os.chdir`. A failing command inside an `&&` chain stops the chain only when it is a rejected `sed -i` or a failed `cd`.";

fn baseline(report: &HeadroomReport) -> Option<&GroupHeadroom> {
  report.overall.iter().find(|group| group.group == BASELINE_GROUP)
}

fn render_within(out: &mut String, set: &SetAggregate) {
  let _ = writeln!(
    out,
    "| by turn | first file read | first file touched | lines of a file seen | every file read | every file touched | files read (pooled) | files touched (pooled) |"
  );
  let _ = writeln!(out, "|---|---|---|---|---|---|---|---|");
  for (index, limit) in WITHIN_LIMITS.iter().enumerate() {
    let sessions = |rows: &[Within]| share_cell(rows[index].by_turn, set.sessions);
    let files = |rows: &[Within]| share_cell(rows[index].by_turn, set.files);
    let _ = writeln!(
      out,
      "| {limit} | {} | {} | {} | {} | {} | {} | {} |",
      sessions(&set.read_within),
      sessions(&set.touched_within),
      sessions(&set.seen_within),
      sessions(&set.all_read_within),
      sessions(&set.all_touched_within),
      files(&set.files_read_within),
      files(&set.files_touched_within)
    );
  }
  let _ = writeln!(
    out,
    "| ever | {} | {} | {} | {} | {} | {} | {} |\n",
    share_cell(set.sessions - set.sessions_without_read, set.sessions),
    share_cell(set.sessions - set.sessions_without_touch, set.sessions),
    share_cell(set.measures.get("first_seen_turn").map_or(0, |summary| summary.n), set.sessions),
    share_cell(set.measures.get(ALL_READ_TURN).map_or(0, |summary| summary.n), set.sessions),
    share_cell(set.measures.get(ALL_TOUCHED_TURN).map_or(0, |summary| summary.n), set.sessions),
    share_cell(set.files_read, set.files),
    share_cell(set.files_touched, set.files)
  );
}

fn entry_label(kind: EntryKind, source: EntrySource) -> String {
  format!("{} {}", kind.label(), source.label())
}

fn render_headline(out: &mut String, report: &HeadroomReport) {
  let Some(baseline) = baseline(report) else {
    let _ = writeln!(out, "No baseline sessions were analysed.\n");
    return;
  };
  let set = &baseline.must_change;
  let _ = writeln!(
    out,
    "Baseline sessions (no Code Atlas): {} over {} tasks. {} of them belong to tasks with more than one `must_change` file; counting each file once per session there are {} files. Turn scale; cells are sessions or files out of the total.\n",
    set.sessions, report.baseline_tasks, set.sessions_with_several_files, set.files
  );
  render_within(out, set);
  let measure = |name: &str, unit: Unit| ordinal_cell(set.measures.get(name), set.sessions, unit);
  let _ = writeln!(
    out,
    "- **First file.** First read at turn {}; first touch at turn {}; first lines seen at turn {}; first edit at turn {}. Before the first read lie {} of the turns and {} of the time.",
    measure(FIRST_READ_TURN, Unit::Ordinal),
    measure("first_touched_turn", Unit::Ordinal),
    measure("first_seen_turn", Unit::Ordinal),
    measure("first_edited_turn", Unit::Ordinal),
    measure("share_before_first_read_turns", Unit::Share),
    measure("share_before_first_read_time", Unit::Share)
  );
  let _ = writeln!(
    out,
    "- **Every file.** The last `must_change` file is first read at turn {} and first touched at turn {}; before that first touch lie {} of the turns. {} of {} files were read at some point and {} touched.",
    measure(ALL_READ_TURN, Unit::Ordinal),
    measure(ALL_TOUCHED_TURN, Unit::Ordinal),
    measure("share_before_all_touched_turns", Unit::Share),
    set.files_read,
    set.files,
    set.files_touched
  );
  let _ = writeln!(
    out,
    "- **First naming.** A result named a `must_change` file before the agent addressed one at turn {}; in {} of {} sessions the agent addressed one before any result had named one.",
    measure("first_named_turn", Unit::Ordinal),
    set.sessions_addressing_before_named,
    set.sessions
  );
  let entries: Vec<String> = baseline
    .entries
    .iter()
    .map(|entry| {
      format!(
        "{}: {} sessions, {} of them reading one by turn {EARLY_READ_TURN}",
        entry_label(entry.kind, entry.source),
        entry.sessions,
        entry.reading_early
      )
    })
    .collect();
  let _ = writeln!(
    out,
    "- **Entry point.** {} sessions read a `must_change` file by turn {EARLY_READ_TURN}. The first one was reached by {}.",
    baseline.sessions_reading_early,
    entries.join("; by ")
  );
  let per_task = |pick: fn(&TaskShare) -> usize| {
    report
      .baseline_tasks_with_median_within
      .iter()
      .map(|share| format!("≤ {}: {}", share.limit, pick(share)))
      .collect::<Vec<_>>()
      .join(", ")
  };
  let _ = writeln!(
    out,
    "- **Per task** ({} tasks; a task counts when every baseline session of it has the event and their median turn is within the limit). First file read: {}. Every file read: {}. Every file touched: {}.",
    report.baseline_tasks,
    per_task(|share| share.first_read),
    per_task(|share| share.all_read),
    per_task(|share| share.all_touched)
  );
  let on_call_scale = report
    .sessions
    .iter()
    .filter(|session| session.reported_num_turns == Some(u64::from(session.tool_calls) + 1))
    .count();
  let _ = writeln!(
    out,
    "\nNote on scales: in {on_call_scale} of {} sessions the `num_turns` of the result event equals the tool-call count plus one, so the \"turns\" of the agent report are tool calls, not model responses. Baseline sessions here took {} turns and {} tool calls.\n",
    report.sessions.len(),
    ordinal_cell(baseline.turns.as_ref(), baseline.sessions, Unit::Ordinal),
    ordinal_cell(baseline.tool_calls.as_ref(), baseline.sessions, Unit::Ordinal)
  );
}

fn render_limits(out: &mut String, report: &HeadroomReport) {
  let Some(baseline) = baseline(report) else {
    return;
  };
  let per_task: BTreeSet<usize> = report
    .tasks
    .iter()
    .filter_map(|task| task.groups.iter().find(|group| group.group == BASELINE_GROUP))
    .map(|group| group.sessions)
    .collect();
  let runs = per_task.iter().map(usize::to_string).collect::<Vec<_>>().join(" or ");
  let _ = writeln!(
    out,
    "- **Few independent observations.** The {} baseline sessions are {} tasks with {runs} runs each of the same prompt. Runs of one task are repetitions that mostly do the same thing, so the unit to read is the task, and there are {} of them.",
    baseline.sessions, report.baseline_tasks, report.baseline_tasks
  );
  let _ = writeln!(
    out,
    "- **Same authors.** The tasks under `eval/tasks`, their prompts and their `must_change` lists were written by the developers of Code Atlas. A prompt written by someone who knows the fix tends to name the code involved."
  );
  let mut repositories: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
  for facts in report.tasks.iter().filter_map(|task| task.facts.as_ref()) {
    let counts = repositories.entry(facts.repository.as_str()).or_default();
    counts.0 += 1;
    counts.1 += usize::from(facts.reach.instructions.is_some());
  }
  let listed: Vec<String> = repositories
    .iter()
    .map(|(repository, (tasks, guided))| format!("{tasks} on {repository}, {guided} of them with project instructions injected into every session"))
    .collect();
  let _ = writeln!(
    out,
    "- **Repositories.** {}. On this repository the injected instructions are an architecture guide that names crates, files and identifiers. A repository given by URL is public, so the model may know its layout from training.",
    if listed.is_empty() { "Unknown".to_owned() } else { listed.join("; ") }
  );
  let _ = writeln!(
    out,
    "- **So the result describes these tasks.** It does not carry over to tasks whose prompts do not name code, to repositories without such a guide, or to code the model has not seen. An early first read here is what such a prompt yields."
  );
  let _ = writeln!(
    out,
    "- **Detection.** Touches are recognised from a fixed set of forms; the last two sections say which, and what was missed in these sessions.\n"
  );
}

fn reach_cell(reach: &SourceReach) -> String {
  let listed: Vec<String> = reach
    .most_selective
    .iter()
    .map(|term| format!("`{}` ({} of {} files)", term.term.replace('|', "\\|"), term.must_change, term.files))
    .collect();
  match listed.is_empty() {
    true => format!("0 of {}", reach.terms),
    false => format!("{} of {}: {}", reach.reaching, reach.terms, listed.join(", ")),
  }
}

fn render_entry(out: &mut String, report: &HeadroomReport) {
  let _ = writeln!(
    out,
    "Terms taken from the prompt and from the injected project instructions, and what one fixed-string grep for each returns at the base commit. A cell reads `terms that return a must_change file of terms checked`, then the most selective of them as `term (must_change files among the files returned of files returned)`.\n"
  );
  let _ = writeln!(out, "| task | repository | `must_change` files | prompt | injected instructions |");
  let _ = writeln!(out, "|---|---|---|---|---|");
  let files_of = |task: &TaskHeadroom| {
    task
      .groups
      .first()
      .and_then(|group| group.must_change.measures.get("files_at_base"))
      .map_or(0.0, |summary| summary.max)
  };
  for task in &report.tasks {
    let Some(facts) = &task.facts else {
      continue;
    };
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} |",
      task.task_id,
      facts.repository,
      number(files_of(task)),
      reach_cell(&facts.reach.prompt),
      facts.reach.instructions.as_ref().map_or_else(|| "none injected".to_owned(), reach_cell)
    );
  }
  let with_facts: Vec<&TaskFacts> = report.tasks.iter().filter_map(|task| task.facts.as_ref()).collect();
  let _ = writeln!(
    out,
    "\nPrompts with at least one term that returns a `must_change` file: {} of {}. Tasks whose injected instructions have one: {} of {}.\n",
    with_facts.iter().filter(|facts| facts.reach.prompt.reaching > 0).count(),
    with_facts.len(),
    with_facts
      .iter()
      .filter(|facts| facts.reach.instructions.as_ref().is_some_and(|reach| reach.reaching > 0))
      .count(),
    with_facts.iter().filter(|facts| facts.reach.instructions.is_some()).count()
  );
  let _ = writeln!(
    out,
    "How each session reached its first `must_change` file. Cells are `sessions (of which read one by turn {EARLY_READ_TURN})`.\n"
  );
  let groups = &report.overall;
  let kinds: BTreeSet<(EntryKind, EntrySource)> = groups.iter().flat_map(|group| group.entries.iter().map(|entry| (entry.kind, entry.source))).collect();
  let header: Vec<String> = groups
    .iter()
    .map(|group| {
      format!(
        "{} ({} sessions, {} reading by turn {EARLY_READ_TURN})",
        group.group, group.sessions, group.sessions_reading_early
      )
    })
    .collect();
  let _ = writeln!(out, "| the first file was reached by | {} |", header.join(" | "));
  let _ = writeln!(out, "|---|{}", "---|".repeat(groups.len()));
  for (kind, source) in kinds {
    let cells: Vec<String> = groups
      .iter()
      .map(|group| match group.entries.iter().find(|entry| entry.kind == kind && entry.source == source) {
        Some(entry) => format!("{} ({})", entry.sessions, entry.reading_early),
        None => "0 (0)".to_owned(),
      })
      .collect();
    let _ = writeln!(out, "| {} | {} |", entry_label(kind, source), cells.join(" | "));
  }
  let none: Vec<String> = groups.iter().map(|group| format!("{} (0)", group.sessions_without_entry)).collect();
  let _ = writeln!(out, "| no `must_change` file named or addressed | {} |\n", none.join(" | "));
}

type SessionCount = (&'static str, fn(&SetAggregate) -> usize);

const SESSION_COUNTS: [SessionCount; 7] = [
  ("sessions with more than one file in the set", |set| set.sessions_with_several_files),
  ("sessions with no recognised read of any file", |set| set.sessions_without_read),
  ("sessions with no recognised touch of any file", |set| set.sessions_without_touch),
  ("sessions addressing a file before any result named one", |set| set.sessions_addressing_before_named),
  ("sessions editing a file with no earlier read of it", |set| set.sessions_editing_without_read),
  ("sessions editing a file with no earlier read or search of it", |set| set.sessions_editing_unseen),
  ("sessions with at least one file without a recognised touch", |set| set.sessions_with_files_without_touch),
];

type WithinRows = (&'static str, fn(&SetAggregate) -> (&Vec<Within>, usize));

const WITHIN_ROWS: [WithinRows; 7] = [
  ("first file read", |set| (&set.read_within, set.sessions)),
  ("first file touched", |set| (&set.touched_within, set.sessions)),
  ("lines of a file seen", |set| (&set.seen_within, set.sessions)),
  ("every file read", |set| (&set.all_read_within, set.sessions)),
  ("every file touched", |set| (&set.all_touched_within, set.sessions)),
  ("files read, pooled", |set| (&set.files_read_within, set.files)),
  ("files touched, pooled", |set| (&set.files_touched_within, set.files)),
];

struct FileList {
  title: &'static str,
  meaning: &'static str,
  pick: fn(&SetHeadroom) -> &Vec<String>,
}

const FILE_LISTS: [FileList; 4] = [
  FileList {
    title: "Edited with no earlier read",
    meaning: "The file was edited and no read of it (as defined above) was issued in an earlier tool call. Lines of it may still have been shown in a search result.",
    pick: |set| &set.edited_without_read,
  },
  FileList {
    title: "Edited with no earlier read or search",
    meaning: "As above, and no search addressed to the file came earlier either.",
    pick: |set| &set.edited_unseen,
  },
  FileList {
    title: "No recognised touch",
    meaning: "No search, read or edit addressed to the file was recognised in the session. A file marked as changed was changed all the same, in a form this analysis does not recognise (see the detection checks).",
    pick: |set| &set.without_touch,
  },
  FileList {
    title: "Created by the task",
    meaning: "Absent from the base tree, so there was nothing to find; excluded from the statistics.",
    pick: |set| &set.created,
  },
];

fn render_groups(out: &mut String, groups: &[GroupHeadroom], pick: fn(&GroupHeadroom) -> &SetAggregate) {
  let header: Vec<String> = groups.iter().map(|group| format!("{} ({} sessions)", group.group, pick(group).sessions)).collect();
  let _ = writeln!(out, "| measure | {} |", header.join(" | "));
  let _ = writeln!(out, "|---|{}", "---|".repeat(groups.len()));
  let mut row = |label: &str, cells: Vec<String>| {
    let _ = writeln!(out, "| {label} | {} |", cells.join(" | "));
  };
  for measure in &MEASURES {
    row(
      measure.label,
      groups
        .iter()
        .map(|group| ordinal_cell(pick(group).measures.get(measure.name), pick(group).sessions, measure.unit))
        .collect(),
    );
  }
  for (label, rows) in WITHIN_ROWS {
    for (index, limit) in WITHIN_LIMITS.iter().enumerate() {
      row(
        &format!("{label} by turn {limit}"),
        groups
          .iter()
          .map(|group| {
            let (within, of) = rows(pick(group));
            share_cell(within[index].by_turn, of)
          })
          .collect(),
      );
    }
  }
  row(
    "files read at some point, pooled",
    groups.iter().map(|group| share_cell(pick(group).files_read, pick(group).files)).collect(),
  );
  row(
    "files touched at some point, pooled",
    groups.iter().map(|group| share_cell(pick(group).files_touched, pick(group).files)).collect(),
  );
  for (label, count) in SESSION_COUNTS {
    row(label, groups.iter().map(|group| share_cell(count(pick(group)), pick(group).sessions)).collect());
  }
  row(
    "total turns",
    groups.iter().map(|group| ordinal_cell(group.turns.as_ref(), group.sessions, Unit::Ordinal)).collect(),
  );
  row(
    "total tool calls",
    groups.iter().map(|group| ordinal_cell(group.tool_calls.as_ref(), group.sessions, Unit::Ordinal)).collect(),
  );
  out.push('\n');
}

fn render_tasks(out: &mut String, report: &HeadroomReport) {
  let groups: Vec<&str> = report.overall.iter().map(|group| group.group.as_str()).collect();
  let must_change: fn(&GroupHeadroom) -> &SetAggregate = |group| &group.must_change;
  let relevant: fn(&GroupHeadroom) -> &SetAggregate = |group| &group.relevant;
  let _ = writeln!(out, "### Baseline, `must_change`: first file and every file\n");
  let _ = writeln!(
    out,
    "| task | files | sessions | first file read, turn | every file read, turn | every file touched, turn (the last file's first touch) | total turns | files read / touched at some point |"
  );
  let _ = writeln!(out, "|---|---|---|---|---|---|---|---|");
  for task in &report.tasks {
    let Some(group) = task.groups.iter().find(|group| group.group == BASELINE_GROUP) else {
      continue;
    };
    let set = &group.must_change;
    let cell = |name: &str| ordinal_cell(set.measures.get(name), set.sessions, Unit::Ordinal);
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} | {} | {} | {} | {} / {} of {} |",
      task.task_id,
      set.measures.get("files_at_base").map_or_else(|| "–".to_owned(), |summary| number(summary.max)),
      set.sessions,
      cell(FIRST_READ_TURN),
      cell(ALL_READ_TURN),
      cell(ALL_TOUCHED_TURN),
      ordinal_cell(group.turns.as_ref(), group.sessions, Unit::Ordinal),
      set.files_read,
      set.files_touched,
      set.files
    );
  }
  out.push('\n');
  for (title, pick, measure, unit) in [
    ("First read of a `must_change` file, turn", must_change, FIRST_READ_TURN, Unit::Ordinal),
    ("Every `must_change` file read, turn", must_change, ALL_READ_TURN, Unit::Ordinal),
    (
      "Every `must_change` file touched, turn (the last file's first touch)",
      must_change,
      ALL_TOUCHED_TURN,
      Unit::Ordinal,
    ),
    (
      "Share of the turns before the last `must_change` file's first touch",
      must_change,
      "share_before_all_touched_turns",
      Unit::Share,
    ),
    ("First result naming a `must_change` file, turn", must_change, "first_named_turn", Unit::Ordinal),
    ("First lines of a `must_change` file seen, turn", must_change, "first_seen_turn", Unit::Ordinal),
    ("First touch of a `must_change` file, turn", must_change, "first_touched_turn", Unit::Ordinal),
    ("First edit of a `must_change` file, turn", must_change, "first_edited_turn", Unit::Ordinal),
    (
      "Share of the turns before the first read of a `must_change` file",
      must_change,
      "share_before_first_read_turns",
      Unit::Share,
    ),
    ("First read of a `relevant_files` member, turn", relevant, FIRST_READ_TURN, Unit::Ordinal),
    ("Every `relevant_files` member read, turn", relevant, ALL_READ_TURN, Unit::Ordinal),
  ] {
    let _ = writeln!(out, "### {title}\n");
    let _ = writeln!(out, "| task | {} |", groups.join(" | "));
    let _ = writeln!(out, "|---|{}", "---|".repeat(groups.len()));
    for task in &report.tasks {
      let cells: Vec<String> = groups
        .iter()
        .map(|name| match task.groups.iter().find(|group| group.group == *name) {
          Some(group) => ordinal_cell(pick(group).measures.get(measure), pick(group).sessions, unit),
          None => "no sessions".to_owned(),
        })
        .collect();
      let _ = writeln!(out, "| {} | {} |", task.task_id, cells.join(" | "));
    }
    out.push('\n');
  }
  let _ = writeln!(out, "### Total turns and tool calls\n");
  let _ = writeln!(
    out,
    "| task | {} |",
    groups.iter().map(|group| format!("{group}: turns | {group}: tool calls")).collect::<Vec<_>>().join(" | ")
  );
  let _ = writeln!(out, "|---|{}", "---|---|".repeat(groups.len()));
  for task in &report.tasks {
    let cells: Vec<String> = groups
      .iter()
      .map(|name| match task.groups.iter().find(|group| group.group == *name) {
        Some(group) => format!(
          "{} | {}",
          ordinal_cell(group.turns.as_ref(), group.sessions, Unit::Ordinal),
          ordinal_cell(group.tool_calls.as_ref(), group.sessions, Unit::Ordinal)
        ),
        None => "no sessions | no sessions".to_owned(),
      })
      .collect();
    let _ = writeln!(out, "| {} | {} |", task.task_id, cells.join(" | "));
  }
  out.push('\n');
}

fn render_sessions(out: &mut String, report: &HeadroomReport) {
  let _ = writeln!(
    out,
    "Ordinals are `turn/tool call`; `–` means no such event. Columns refer to `must_change` files that exist at base. The entry column gives the source of the entry point and the words it shares with it.\n"
  );
  let _ = writeln!(
    out,
    "| session | batch | group | turns | tool calls | files | named | seen | first read | every file read | first touch | every file touched | first edit | first read via | entry point |"
  );
  let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
  for session in &report.sessions {
    let set = &session.must_change;
    let entry = session.entry.as_ref().map_or_else(
      || "–".to_owned(),
      |entry| {
        let how = match entry.kind {
          EntryKind::Search => "search",
          EntryKind::Opened => "opened",
        };
        format!(
          "{how}; {}: {}",
          entry.source.label(),
          if entry.shared.is_empty() { "–".to_owned() } else { entry.shared.join(", ") }
        )
      },
    );
    let _ = writeln!(
      out,
      "| {}.r{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
      session.task_id,
      session.run_index,
      session.batch,
      session.group,
      session.turns,
      session.tool_calls,
      set.files_at_base,
      pair(set.first_named),
      pair(set.first_seen),
      pair(set.first_read),
      pair(set.all_read),
      pair(set.first_touched),
      pair(set.all_touched),
      pair(set.first_edited),
      set.first_read_via.as_deref().unwrap_or("–"),
      entry
    );
  }
  out.push('\n');
}

fn render_file_lists(out: &mut String, report: &HeadroomReport) {
  for FileList { title, meaning, pick } in FILE_LISTS {
    let _ = writeln!(out, "### {title}\n\n{meaning}\n");
    let rows: Vec<String> = report
      .sessions
      .iter()
      .filter(|session| !pick(&session.must_change).is_empty())
      .map(|session| {
        let files: Vec<String> = pick(&session.must_change)
          .iter()
          .map(|path| match session.edit_check.changed_without_edit_event.contains(path) {
            true => format!("`{path}` (changed in the final diff)"),
            false => format!("`{path}`"),
          })
          .collect();
        format!("- `{}` ({}, {}): {}", session.run_id, session.batch, session.group, files.join(", "))
      })
      .collect();
    match rows.is_empty() {
      true => out.push_str("None.\n\n"),
      false => {
        let _ = writeln!(out, "{}\n", rows.join("\n"));
      }
    }
  }
}

fn code_list(paths: &[String]) -> String {
  paths.iter().map(|path| format!("`{path}`")).collect::<Vec<_>>().join(", ")
}

fn render_seen_without_read(out: &mut String, report: &HeadroomReport) {
  let rows: Vec<String> = report
    .sessions
    .iter()
    .filter(|session| session.must_change.files_at_base > 0 && session.must_change.first_read.is_none())
    .map(|session| {
      let set = &session.must_change;
      format!(
        "`{}` ({}, {}): lines first shown at {}, first edit at {}",
        session.run_id,
        session.batch,
        session.group,
        pair(set.first_seen),
        pair(set.first_edited)
      )
    })
    .collect();
  let _ = writeln!(
    out,
    "- Sessions with no recognised read of a `must_change` file: {}. For each, the turn/tool call at which a result first carried lines of one, and the first edit: {}.",
    rows.len(),
    if rows.is_empty() { "none".to_owned() } else { rows.join("; ") }
  );
}

fn render_checks(out: &mut String, report: &HeadroomReport) {
  let sessions = &report.sessions;
  let total = |pick: fn(&SessionHeadroom) -> u32| sessions.iter().map(|session| u64::from(pick(session))).sum::<u64>();
  let having = |keep: fn(&SessionHeadroom) -> bool| sessions.iter().filter(|session| keep(session)).count();
  let mentioned_first = sessions
    .iter()
    .filter(|session| {
      session
        .must_change
        .first_addressed
        .is_some_and(|addressed| session.must_change.first_touched.is_none_or(|touched| addressed < touched))
    })
    .map(|session| {
      let mention = session
        .files
        .iter()
        .filter_map(|file| file.mentioned.as_ref())
        .find(|event| Some(event.at) == session.must_change.first_addressed);
      format!(
        "`{}` ({}, via {})",
        session.run_id,
        session.batch,
        mention.map_or("an unknown form", |event| event.via.as_str())
      )
    })
    .collect::<Vec<_>>();
  let _ = writeln!(out, "- Sessions analysed: {}.", sessions.len());
  render_seen_without_read(out, report);
  let _ = writeln!(
    out,
    "- Sessions whose first agent-side reference to a `must_change` file is a mention, earlier than the first touch: {}{}. For these the first touch may be later than the truth.",
    mentioned_first.len(),
    if mentioned_first.is_empty() {
      String::new()
    } else {
      format!(": {}", mentioned_first.join(", "))
    }
  );
  let _ = writeln!(
    out,
    "- `cd` commands whose output reports that the directory does not exist (the directory is kept): {}. `cd` commands into a directory the tree model does not know, with errors hidden (assumed to have failed): {} in {} sessions. Working directories stated by the harness itself (a failed Edit's note, a backgrounded command's hint): {}, of which {} contradict the tracked directory.",
    total(|session| session.diagnostics.failed_cds),
    total(|session| session.diagnostics.unverified_cds),
    having(|session| session.diagnostics.unverified_cds > 0),
    total(|session| session.diagnostics.cwd_notes),
    total(|session| session.diagnostics.cwd_notes_contradicting)
  );
  let _ = writeln!(
    out,
    "- Read commands with operands that could not be resolved (dynamic or unmatched glob): {}. `sed -i` commands rejected by sed and not counted as edits: {} file operands. Failed Read/Edit/Write calls: {}. Shell working-directory resets: {}.",
    total(|session| session.diagnostics.unparsed_bash_reads),
    total(|session| session.diagnostics.rejected_sed_edits),
    total(|session| session.diagnostics.errored_file_calls),
    total(|session| session.diagnostics.cwd_resets)
  );
  let unchanged: usize = sessions.iter().map(|session| session.edit_check.edited_but_unchanged.len()).sum();
  let missed: usize = sessions.iter().map(|session| session.edit_check.changed_without_edit_event.len()).sum();
  let _ = writeln!(
    out,
    "- Edit detection against each session's final diff, over `must_change` and `relevant_files`: {missed} file(s) changed in the diff without a recognised edit event; {unchanged} file(s) with an edit event that the diff does not show as changed (the edit was reverted, rewrote the file without changing it, or was misdetected)."
  );
  for session in sessions.iter().filter(|session| session.edit_check != EditCheck::default()) {
    let check = &session.edit_check;
    let mut parts = Vec::new();
    if !check.changed_without_edit_event.is_empty() {
      parts.push(format!("changed without an edit event: {}", code_list(&check.changed_without_edit_event)));
    }
    if !check.edited_but_unchanged.is_empty() {
      parts.push(format!("edit event but unchanged: {}", code_list(&check.edited_but_unchanged)));
    }
    let _ = writeln!(out, "  - `{}` ({}): {}", session.run_id, session.batch, parts.join("; "));
  }
  for note in &report.notes {
    let _ = writeln!(out, "- {note}");
  }
  out.push('\n');
}

pub fn render_headroom_markdown(report: &HeadroomReport) -> String {
  let mut out = String::new();
  let _ = writeln!(out, "# Headroom: when does an agent reach the files it must change?\n");
  let _ = writeln!(
    out,
    "Computed from recorded sessions only; no session was run for this report. For every session it gives the turn at which the first file the task must change is reached, the turn at which every such file has been reached, and where the session got its entry point from. Read the headline together with the two sections after it: they say what each number supports and how far these sessions can be generalised.\n"
  );
  let _ = writeln!(out, "| batch | date recorded | sessions analysed | excluded |");
  let _ = writeln!(out, "|---|---|---|---|");
  for batch in &report.batches {
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} |",
      batch.name,
      batch.date.as_deref().unwrap_or("unknown"),
      batch.sessions,
      if batch.excluded.is_empty() { "none".to_owned() } else { batch.excluded.join(", ") }
    );
  }
  let _ = writeln!(out, "\nBaseline sessions of all batches are pooled; atlas sessions are grouped by preamble variant.\n");
  let _ = writeln!(out, "## Headline\n");
  render_headline(&mut out, report);
  let _ = writeln!(out, "## What each measure supports\n\n{SUPPORTS}\n");
  let _ = writeln!(out, "## Limits\n");
  render_limits(&mut out, report);
  let _ = writeln!(out, "## Where the entry point comes from\n");
  render_entry(&mut out, report);
  let _ = writeln!(out, "## Definitions\n\n{DEFINITIONS}\n");
  let _ = writeln!(
    out,
    "## Overall, `must_change`\n\nCells are `median (min–max, n)` over sessions; `n` is given as `n of N` when some sessions have no such event. Shares are sessions, or files where the row says pooled.\n"
  );
  render_groups(&mut out, &report.overall, |group| &group.must_change);
  let _ = writeln!(out, "## Overall, `relevant_files`\n");
  render_groups(&mut out, &report.overall, |group| &group.relevant);
  let _ = writeln!(out, "## Per task\n\nCells are `median (min–max, n)` over the sessions of the task in that group.\n");
  render_tasks(&mut out, report);
  let _ = writeln!(out, "## Per session\n");
  render_sessions(&mut out, report);
  let _ = writeln!(out, "## `must_change` files by session\n");
  render_file_lists(&mut out, report);
  let _ = writeln!(out, "## How Bash is mapped to files\n\n{BASH_LIMITS}\n");
  let _ = writeln!(out, "## Detection checks\n");
  render_checks(&mut out, report);
  out
}

pub fn write_headroom(report: &HeadroomReport, out_base: &Path) -> AgentResult<(PathBuf, PathBuf)> {
  if let Some(parent) = out_base.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let markdown = out_base.with_extension("md");
  let json = out_base.with_extension("json");
  fs::write(&markdown, render_headroom_markdown(report)).map_err(|error| AgentError::io(&markdown, error))?;
  write_json_dense(&json, report)?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use serde_json::{Value, json};

  use super::super::provenance::TermReach;
  use super::super::testing::{run_record, scratch_of};
  use super::super::transcript::{AssistantTurn, ResultInfo, TokenUsage, ToolCall, ToolResult};
  use super::*;

  const ENTITY: &str = "api/src/user.entity.ts";
  const SERVICE: &str = "api/src/users.service.ts";
  const MAPPER: &str = "api/src/user.mapper.ts";
  const SEEDS: &str = "api/test/seeds.ts";
  const CONTROLLER: &str = "api/src/users.controller.ts";
  const PROMPT: &str = "Add `deletedAt` to the User entity.";
  const INSTRUCTIONS: &str = "UsersService lives in users.service.ts";

  struct Step {
    turn: u32,
    name: &'static str,
    input: Value,
    output: &'static str,
  }

  fn step(turn: u32, name: &'static str, input: Value, output: &'static str) -> Step {
    Step { turn, name, input, output }
  }

  fn transcript(turns: u32, steps: Vec<Step>) -> Transcript {
    Transcript {
      turns: (0..turns)
        .map(|index| AssistantTurn {
          message_id: format!("m{index}"),
          model: None,
          usage: TokenUsage::default(),
          elapsed_ms: None,
        })
        .collect(),
      tool_calls: steps
        .into_iter()
        .enumerate()
        .map(|(index, step)| ToolCall {
          id: format!("t{index}"),
          name: step.name.to_owned(),
          input: step.input,
          elapsed_ms: Some((index as u64 + 1) * 1000),
          result: Some(ToolResult {
            text: step.output.to_owned(),
            ..ToolResult::default()
          }),
          turn: step.turn,
        })
        .collect(),
      result: Some(ResultInfo {
        duration_ms: Some(10_000),
        num_turns: Some(7),
        ..ResultInfo::default()
      }),
      ..Transcript::default()
    }
  }

  fn truth(must_change: &[&str]) -> GroundTruth {
    GroundTruth {
      must_change: must_change.iter().map(|path| (*path).to_owned()).collect(),
      relevant_files: [ENTITY, CONTROLLER].map(str::to_owned).to_vec(),
      ..GroundTruth::default()
    }
  }

  fn analyze(run: (&str, Condition, u32), transcript: &Transcript, changed: &[&str], must_change: &[&str]) -> SessionHeadroom {
    let mut record = run_record(run.0, run.1, run.2, TokenUsage::default());
    record.manifest.args = ["--append-system-prompt", INSTRUCTIONS, "-p", PROMPT].map(str::to_owned).to_vec();
    let root = format!("{}/tree", scratch_of(&record.manifest.run_id));
    let tree = TreeIndex::new([root], [ENTITY, SERVICE, SEEDS, CONTROLLER, "api/src/main.ts"].map(str::to_owned));
    analyze_session(SessionInput {
      batch: "b1",
      record: &record,
      transcript,
      tree,
      truth: &truth(must_change),
      changed_files: &changed.iter().map(|path| (*path).to_owned()).collect(),
    })
  }

  fn path(run_id: &str, file: &str) -> String {
    format!("/private{}/tree/{file}", scratch_of(run_id))
  }

  fn explored() -> SessionHeadroom {
    let run = "t01.baseline.r0";
    let steps = vec![
      step(
        1,
        "Grep",
        json!({"pattern": "deletedAt", "output_mode": "files_with_matches"}),
        "Found 2 files\napi/src/users.controller.ts\napi/src/user.entity.ts",
      ),
      step(2, "Read", json!({"file_path": path(run, CONTROLLER)}), "1\tclass UsersController {}"),
      step(3, "Read", json!({"file_path": path(run, ENTITY)}), "1\tclass User {}"),
      step(3, "Bash", json!({"command": "grep -n findOne api/src/users.service.ts"}), "18:  findOne() {"),
      step(4, "Edit", json!({"file_path": path(run, ENTITY)}), "updated"),
      step(4, "Bash", json!({"command": "sed -i '' 's/a/b/' api/test/seeds.ts api/src/users.service.ts"}), ""),
      step(5, "Write", json!({"file_path": path(run, MAPPER), "content": "export {}"}), "created"),
    ];
    analyze(
      ("t01", Condition::Baseline, 0),
      &transcript(6, steps),
      &[ENTITY, SEEDS, MAPPER, CONTROLLER],
      &[ENTITY, MAPPER, SEEDS, SERVICE],
    )
  }

  fn ordinal(turn: u32, call: u32) -> Option<Ordinal> {
    Some(Ordinal { call, turn })
  }

  #[test]
  fn should_report_the_first_file_and_every_file_of_a_set() {
    let session = explored();
    let set = &session.must_change;
    assert_eq!((session.turns, session.tool_calls, session.reported_num_turns), (6, 7, Some(7)));
    assert_eq!(session.group, "baseline");
    assert_eq!((set.files_at_base, set.created.clone()), (3, vec![MAPPER.to_owned()]));
    assert_eq!(set.first_named, ordinal(1, 1));
    assert_eq!(set.first_addressed, ordinal(3, 3));
    assert_eq!(set.first_seen, ordinal(3, 3));
    assert_eq!(set.first_read, ordinal(3, 3));
    assert_eq!(set.first_read_via.as_deref(), Some("Read"));
    assert_eq!(set.first_searched, ordinal(3, 4));
    assert_eq!(set.first_touched, ordinal(3, 3));
    assert_eq!(set.first_edited, ordinal(4, 5));
    assert_eq!(
      set.before_first_read,
      Some(SessionShare {
        turns: 2.0 / 6.0,
        tool_calls: 2.0 / 7.0,
        time: Some(0.3)
      })
    );
    assert_eq!(set.all_read, None);
    assert_eq!(set.all_touched, ordinal(4, 6));
    assert_eq!(set.turns_before_all_touched, Some(0.5));
    assert_eq!(set.file_reads, vec![Ordinal { call: 3, turn: 3 }]);
    assert_eq!(
      set.file_touches,
      vec![Ordinal { call: 3, turn: 3 }, Ordinal { call: 6, turn: 4 }, Ordinal { call: 4, turn: 3 }]
    );
    assert_eq!(set.edited_without_read, vec![SEEDS, SERVICE]);
    assert_eq!(set.edited_unseen, vec![SEEDS]);
    assert_eq!(set.without_read, vec![SEEDS, SERVICE]);
    assert_eq!(set.without_touch, Vec::<String>::new());
    assert_eq!(
      (session.relevant.first_read, session.relevant.all_read, session.relevant.all_touched),
      (ordinal(2, 2), ordinal(3, 3), ordinal(3, 3))
    );
    assert_eq!(session.relevant.first_edited, ordinal(4, 5));
    assert_eq!(session.edit_check.edited_but_unchanged, vec![SERVICE]);
    assert_eq!(session.edit_check.changed_without_edit_event, vec![CONTROLLER]);
    assert_eq!(
      session.entry,
      Some(Entry {
        at: Ordinal { call: 1, turn: 1 },
        kind: EntryKind::Search,
        source: EntrySource::PromptIdentifier,
        shared: vec!["deletedat".to_owned()]
      })
    );
    let service = session.files.iter().find(|file| file.path == SERVICE).unwrap();
    assert_eq!(
      service.shown.as_ref().map(|event| (event.at.turn, event.at.call, event.via.as_str())),
      Some((3, 4, "Bash: grep"))
    );
  }

  #[test]
  fn should_treat_an_address_before_any_naming_as_an_opened_entry_point() {
    let run = "t02.baseline.r0";
    let steps = vec![
      step(1, "Read", json!({"file_path": path(run, SERVICE)}), "1\tclass UsersService {}"),
      step(2, "Grep", json!({"pattern": "deletedAt", "output_mode": "content"}), "api/src/user.entity.ts:4:  deletedAt"),
    ];
    let session = analyze(("t02", Condition::Baseline, 0), &transcript(3, steps), &[], &[ENTITY, MAPPER, SEEDS, SERVICE]);
    let set = &session.must_change;
    assert_eq!((set.first_addressed, set.first_named), (ordinal(1, 1), None));
    assert!(set.addressed_before_named());
    assert_eq!(set.before_first_read.map(|share| share.turns), Some(0.0));
    assert_eq!((set.first_seen, set.all_read, set.all_touched), (ordinal(1, 1), None, None));
    assert_eq!(set.without_touch, vec![ENTITY, SEEDS]);
    assert_eq!(
      session.entry,
      Some(Entry {
        at: Ordinal { call: 1, turn: 1 },
        kind: EntryKind::Opened,
        source: EntrySource::InstructionsIdentifier,
        shared: vec!["service".to_owned(), "users".to_owned()]
      })
    );
    let entity = session.files.iter().find(|file| file.path == ENTITY).unwrap();
    assert_eq!(entity.seen().map(|event| event.at), ordinal(2, 2));
    assert_eq!(entity.read, None);
  }

  fn reading(run: (&str, Condition, u32), reads: &[(&str, u32)], turns: u32, must_change: &[&str]) -> SessionHeadroom {
    let run_id = format!("{}.{}.r{}", run.0, run.1, run.2);
    let mut steps: Vec<Step> = reads
      .iter()
      .map(|(file, turn)| step(*turn, "Read", json!({"file_path": path(&run_id, file)}), "1\tcontent"))
      .collect();
    if steps.is_empty() {
      steps.push(step(1, "Bash", json!({"command": "true"}), ""));
    }
    analyze(run, &transcript(turns, steps), &[], must_change)
  }

  fn report() -> HeadroomReport {
    let several = [ENTITY, SEEDS, SERVICE];
    let sessions = vec![
      reading(("t01", Condition::Baseline, 0), &[(ENTITY, 1)], 4, &[ENTITY]),
      reading(("t01", Condition::Baseline, 1), &[(ENTITY, 2)], 8, &[ENTITY]),
      reading(("t02", Condition::Baseline, 0), &[(ENTITY, 2), (SEEDS, 2), (SERVICE, 6)], 10, &several),
      reading(("t02", Condition::Baseline, 1), &[(SEEDS, 3), (ENTITY, 5)], 10, &several),
      reading(("t03", Condition::Baseline, 0), &[], 6, &[ENTITY]),
      reading(("t03", Condition::Baseline, 1), &[(ENTITY, 3)], 6, &[ENTITY]),
      reading(("t01", Condition::Atlas, 0), &[(ENTITY, 4)], 8, &[ENTITY]),
    ];
    let batch = HeadroomBatch {
      name: "b1".to_owned(),
      date: Some("2026-10-04".to_owned()),
      sessions: sessions.len(),
      excluded: Vec::new(),
    };
    let facts = TaskFacts {
      repository: "this repository".to_owned(),
      reach: TaskReach {
        prompt: SourceReach {
          terms: 3,
          reaching: 1,
          most_selective: vec![TermReach {
            term: "deletedAt".to_owned(),
            files: 2,
            must_change: 1,
          }],
        },
        instructions: None,
      },
    };
    build_headroom(vec![batch], sessions, &BTreeMap::from([("t01".to_owned(), facts)]), Vec::new())
  }

  fn summary(median: f64, min: f64, max: f64, n: usize) -> Option<Summary> {
    Some(Summary { median, min, max, n })
  }

  fn counts(rows: &[Within]) -> Vec<(usize, usize)> {
    rows.iter().map(|row| (row.by_turn, row.by_tool_call)).collect()
  }

  #[test]
  fn should_aggregate_first_file_every_file_and_pooled_file_rows() {
    let report = report();
    assert_eq!(
      report.overall.iter().map(|group| group.group.as_str()).collect::<Vec<_>>(),
      vec!["baseline", "atlas (standard)"]
    );
    let baseline = &report.overall[0];
    let set = &baseline.must_change;
    assert_eq!((baseline.sessions, set.sessions, set.sessions_with_several_files), (6, 6, 2));
    assert_eq!(set.measures.get("first_read_turn").copied(), summary(2.0, 1.0, 3.0, 5));
    assert_eq!(set.measures.get("all_read_turn").copied(), summary(2.5, 1.0, 6.0, 4));
    assert_eq!(set.measures.get("all_touched_turn").copied(), summary(2.5, 1.0, 6.0, 4));
    assert_eq!(set.measures.get("all_read_tool_call").copied(), summary(1.0, 1.0, 3.0, 4));
    assert_eq!(set.measures.get("share_before_first_read_turns").copied(), summary(0.125, 0.0, 1.0 / 3.0, 5));
    assert_eq!(set.measures.get("share_before_all_touched_turns").copied(), summary((0.125 + 1.0 / 3.0) / 2.0, 0.0, 0.5, 4));
    assert_eq!(counts(&set.read_within), vec![(1, 5), (3, 5), (5, 5), (5, 5)]);
    assert_eq!(counts(&set.seen_within), vec![(1, 5), (3, 5), (5, 5), (5, 5)]);
    assert_eq!(counts(&set.all_read_within), vec![(1, 3), (2, 3), (3, 4), (3, 4)]);
    assert_eq!(counts(&set.all_touched_within), vec![(1, 3), (2, 3), (3, 4), (3, 4)]);
    assert_eq!((set.files, set.files_read, set.files_touched), (10, 8, 8));
    assert_eq!(counts(&set.files_read_within), vec![(1, 5), (4, 7), (6, 8), (7, 8)]);
    assert_eq!(counts(&set.files_touched_within), vec![(1, 5), (4, 7), (6, 8), (7, 8)]);
    assert_eq!((set.sessions_without_read, set.sessions_without_touch, set.sessions_addressing_before_named), (1, 1, 5));
    assert_eq!(set.sessions_with_files_without_touch, 2);
    assert_eq!(baseline.turns, summary(7.0, 4.0, 10.0, 6));
    assert_eq!(
      baseline.entries,
      vec![
        EntryCount {
          kind: EntryKind::Opened,
          source: EntrySource::PromptIdentifier,
          sessions: 4,
          reading_early: 3
        },
        EntryCount {
          kind: EntryKind::Opened,
          source: EntrySource::Neither,
          sessions: 1,
          reading_early: 0
        }
      ]
    );
    assert_eq!((baseline.sessions_without_entry, baseline.sessions_reading_early), (1, 3));
    assert_eq!(report.baseline_tasks, 3);
    let shares: Vec<(u32, usize, usize, usize)> = report
      .baseline_tasks_with_median_within
      .iter()
      .map(|share| (share.limit, share.first_read, share.all_read, share.all_touched))
      .collect();
    assert_eq!(shares, vec![(1, 0, 0, 0), (2, 1, 1, 1), (3, 2, 1, 1), (5, 2, 1, 1)]);
    let atlas = &report.tasks[0].groups[1];
    assert_eq!((atlas.group.as_str(), atlas.must_change.measures["first_read_turn"].median), ("atlas (standard)", 4.0));
    assert_eq!(report.tasks[0].facts.as_ref().map(|facts| facts.reach.prompt.reaching), Some(1));
    assert_eq!(report.tasks[1].facts, None);
  }

  #[test]
  fn should_render_both_headline_views_the_entry_sources_and_the_limits() {
    let markdown = render_headroom_markdown(&report());
    for expected in [
      "Baseline sessions (no Code Atlas): 6 over 3 tasks. 2 of them belong to tasks with more than one `must_change` file; counting each file once per session there are 10 files.",
      "| 2 | 3/6 (50%) | 3/6 (50%) | 3/6 (50%) | 2/6 (33.3%) | 2/6 (33.3%) | 4/10 (40%) | 4/10 (40%) |",
      "| ever | 5/6 (83.3%) | 5/6 (83.3%) | 5/6 (83.3%) | 4/6 (66.7%) | 4/6 (66.7%) | 8/10 (80%) | 8/10 (80%) |",
      "- **Every file.** The last `must_change` file is first read at turn 2.5 (1–6, n=4 of 6) and first touched at turn 2.5 (1–6, n=4 of 6);",
      "- **Entry point.** 3 sessions read a `must_change` file by turn 2. The first one was reached by opening it with no earlier naming, its path containing an identifier or word written in the prompt: 4 sessions, 3 of them reading one by turn 2; by opening it with no earlier naming, its path containing neither: 1 sessions, 0 of them reading one by turn 2.",
      "First file read: ≤ 1: 0, ≤ 2: 1, ≤ 3: 2, ≤ 5: 2. Every file read: ≤ 1: 0, ≤ 2: 1, ≤ 3: 1, ≤ 5: 1.",
      "- **Few independent observations.** The 6 baseline sessions are 3 tasks with 2 runs each of the same prompt.",
      "- **Repositories.** 1 on this repository, 0 of them with project instructions injected into every session.",
      "| t01 | this repository | 1 | 1 of 3: `deletedAt` (1 of 2 files) | none injected |",
      "| opening it with no earlier naming, its path containing an identifier or word written in the prompt | 4 (3) | 1 (0) |",
      "| t02 | 3 | 2 | 2.5 (2–3, n=2) | 6 (6–6, n=1 of 2) | 6 (6–6, n=1 of 2) | 10 (10–10, n=2) | 5 / 5 of 6 |",
      "| t03 | 3 (3–3, n=1 of 2) | no sessions |",
      "- Sessions with no recognised read of a `must_change` file: 1. For each, the turn/tool call at which a result first carried lines of one, and the first edit: `t03.baseline.r0` (b1, baseline): lines first shown at –, first edit at –.",
      "- **Turn.** One model response",
    ] {
      assert!(markdown.contains(expected), "missing `{expected}` in\n{markdown}");
    }
    assert!(!markdown.contains("/var/folders"), "{markdown}");
  }

  #[test]
  fn should_mark_untouched_files_that_the_final_diff_shows_as_changed() {
    let session = reading(("t02", Condition::Baseline, 0), &[(ENTITY, 2)], 4, &[ENTITY, SEEDS, SERVICE]);
    let mut report = build_headroom(Vec::new(), vec![session], &BTreeMap::new(), Vec::new());
    report.sessions[0].edit_check.changed_without_edit_event = vec![SEEDS.to_owned()];
    let markdown = render_headroom_markdown(&report);
    assert!(
      markdown.contains("- `t02.baseline.r0` (b1, baseline): `api/test/seeds.ts` (changed in the final diff), `api/src/users.service.ts`"),
      "{markdown}"
    );
  }

  #[test]
  fn should_take_the_middle_of_an_even_sample_as_the_median() {
    assert_eq!(Summary::of(&[4.0, 1.0, 3.0, 2.0]), summary(2.5, 1.0, 4.0, 4));
    assert_eq!(Summary::of(&[]), None);
  }
}
