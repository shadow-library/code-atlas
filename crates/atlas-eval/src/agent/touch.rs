use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::claude::ATLAS_TOOL_PREFIX;
use super::incumbent::serena_edit;
use super::metrics::locations_in;
use super::shell::{AccessOutcome, ShellAccessKind, ShellForm, trace_bash};
use super::transcript::{Completion, ToolCall, ToolResult, Transcript};
use super::tree_index::{Cwd, TreeIndex};

const READ_TOOL: &str = "Read";
const WRITE_TOOL: &str = "Write";
const EDIT_TOOLS: [&str; 4] = ["Edit", WRITE_TOOL, "MultiEdit", "NotebookEdit"];
const SEARCH_TOOLS: [&str; 2] = ["Grep", "Glob"];
const BASH_TOOL: &str = "Bash";
const FILE_FIELDS: [&str; 2] = ["file_path", "notebook_path"];
const SERENA_PATH_FIELD: &str = "relative_path";
const CWD_RESET_NOTICE: &str = "Shell cwd was reset to ";
const CWD_ERROR_NOTE: &str = "your current working directory is ";
const CWD_HINT: &str = "Session cwd remains ";
const GREP_CONTENT_MODE: &str = "content";
const DIFF_SIDE_PREFIXES: [&str; 2] = ["a/", "b/"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchKind {
  Named,
  Shown,
  Mentioned,
  Searched,
  Read,
  Edited,
}

// `call` comes first so that the derived ordering is the order of the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ordinal {
  pub call: u32,
  pub turn: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchEvent {
  #[serde(flatten)]
  pub at: Ordinal,
  pub via: String,
  #[serde(skip)]
  pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
  AtBase,
  Created,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTouches {
  pub path: String,
  pub presence: Presence,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub named: Option<TouchEvent>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub shown: Option<TouchEvent>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mentioned: Option<TouchEvent>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub searched: Option<TouchEvent>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub read: Option<TouchEvent>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub edited: Option<TouchEvent>,
}

impl FileTouches {
  pub fn touched(&self) -> Option<&TouchEvent> {
    earliest([&self.searched, &self.read, &self.edited])
  }

  pub fn seen(&self) -> Option<&TouchEvent> {
    earliest([&self.shown, &self.read])
  }

  pub fn addressed(&self) -> Option<&TouchEvent> {
    earliest([&self.mentioned, &self.searched, &self.read, &self.edited])
  }

  pub fn named_before_addressed(&self) -> Option<&TouchEvent> {
    let named = self.named.as_ref()?;
    self.addressed().is_none_or(|addressed| named.at < addressed.at).then_some(named)
  }

  fn slot(&mut self, kind: TouchKind) -> &mut Option<TouchEvent> {
    match kind {
      TouchKind::Named => &mut self.named,
      TouchKind::Shown => &mut self.shown,
      TouchKind::Mentioned => &mut self.mentioned,
      TouchKind::Searched => &mut self.searched,
      TouchKind::Read => &mut self.read,
      TouchKind::Edited => &mut self.edited,
    }
  }
}

fn earliest<const N: usize>(events: [&Option<TouchEvent>; N]) -> Option<&TouchEvent> {
  events.into_iter().flatten().min_by_key(|event| event.at)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchDiagnostics {
  pub failed_cds: u32,
  pub unverified_cds: u32,
  pub unparsed_bash_reads: u32,
  pub rejected_sed_edits: u32,
  pub cwd_resets: u32,
  pub errored_file_calls: u32,
  pub cwd_notes: u32,
  pub cwd_notes_contradicting: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
  pub call: u32,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTouches {
  pub turns: u32,
  pub tool_calls: u32,
  pub files: Vec<FileTouches>,
  pub queries: Vec<SearchQuery>,
  pub diagnostics: TouchDiagnostics,
}

pub fn session_touches(transcript: &Transcript, tree: TreeIndex, tracked: &[String]) -> SessionTouches {
  let files = tracked
    .iter()
    .map(|path| FileTouches {
      path: path.clone(),
      presence: if tree.is_file(path) { Presence::AtBase } else { Presence::Created },
      named: None,
      shown: None,
      mentioned: None,
      searched: None,
      read: None,
      edited: None,
    })
    .collect();
  let mut tracker = Tracker {
    suffixes: tracked.iter().map(|path| tree.distinguishing_suffix(path)).collect(),
    tree,
    cwd: Cwd::root(),
    files,
    queries: Vec::new(),
    diagnostics: TouchDiagnostics::default(),
  };
  for (index, call) in transcript.tool_calls.iter().enumerate() {
    tracker.observe(call, index as u32 + 1);
  }
  SessionTouches {
    turns: transcript.turns.len() as u32,
    tool_calls: transcript.tool_calls.len() as u32,
    files: tracker.files,
    queries: tracker.queries,
    diagnostics: tracker.diagnostics,
  }
}

#[derive(Default)]
struct Observed {
  input: BTreeMap<usize, BTreeMap<TouchKind, String>>,
  naming_bases: Option<Vec<String>>,
  query: Option<String>,
}

impl Observed {
  fn add(&mut self, file: usize, kind: TouchKind, via: impl Into<String>) {
    self.input.entry(file).or_default().entry(kind).or_insert_with(|| via.into());
  }
}

struct Tracker {
  tree: TreeIndex,
  cwd: Cwd,
  suffixes: Vec<String>,
  files: Vec<FileTouches>,
  queries: Vec<SearchQuery>,
  diagnostics: TouchDiagnostics,
}

impl Tracker {
  fn observe(&mut self, call: &ToolCall, ordinal: u32) {
    let errored = call.result.as_ref().is_none_or(|result| result.is_error);
    let output = call.result.as_ref().map_or("", |result| result.text.as_str());
    let name = call.name.as_str();
    let mut observed = Observed::default();
    let input_text = match name {
      READ_TOOL => self.observe_file_tool(call, TouchKind::Read, errored, &mut observed),
      _ if EDIT_TOOLS.contains(&name) => self.observe_file_tool(call, TouchKind::Edited, errored, &mut observed),
      _ if serena_edit(name) && call.input_str(SERENA_PATH_FIELD).is_some() => self.observe_serena_edit(call, errored, &mut observed),
      _ if SEARCH_TOOLS.contains(&name) => self.observe_search_tool(call, errored, &mut observed),
      BASH_TOOL => self.observe_bash(call, &mut observed),
      _ => {
        observed.naming_bases = Some(vec![String::new()]);
        observed.query = Some(string_values(&call.input).join(" "));
        call.input.to_string()
      }
    };
    if let Some(text) = observed.query.take() {
      self.queries.push(SearchQuery { call: ordinal, text });
    }
    for file in 0..self.files.len() {
      if !observed.input.contains_key(&file) && mentions(&input_text, &self.suffixes[file]) {
        observed.add(file, TouchKind::Mentioned, format!("{name} input"));
      }
    }
    let named = match &observed.naming_bases {
      Some(bases) if call.result.is_some() => self.named_in(name, output, bases),
      _ => BTreeMap::new(),
    };
    let at = Ordinal { call: ordinal, turn: call.turn };
    for (file, lines) in named {
      if lines == ResultLines::Carried {
        self.record(file, TouchKind::Shown, at, format!("{name} result"), call.elapsed_ms);
      }
      if !observed.input.contains_key(&file) {
        self.record(file, TouchKind::Named, at, format!("{name} result"), call.elapsed_ms);
      }
    }
    for (file, kinds) in observed.input {
      for (kind, via) in kinds {
        self.record(file, kind, at, via, call.elapsed_ms);
      }
    }
  }

  fn record(&mut self, file: usize, kind: TouchKind, at: Ordinal, via: String, elapsed_ms: Option<u64>) {
    self.files[file].slot(kind).get_or_insert(TouchEvent { at, via, elapsed_ms });
  }

  fn tracked(&self, path: &str) -> Option<usize> {
    self.files.iter().position(|file| file.path == path)
  }

  fn noted_cwd(&self, text: &str, marker: &str) -> Option<Cwd> {
    let noted = text.split(marker).nth(1)?.split([';', '<', '\n']).next()?.trim().trim_end_matches('.');
    Some(self.tree.resolve(&Cwd::Outside, noted).map_or(Cwd::Outside, Cwd::InTree))
  }

  fn observe_file_tool(&mut self, call: &ToolCall, kind: TouchKind, errored: bool, observed: &mut Observed) -> String {
    let Some(path) = FILE_FIELDS.iter().find_map(|field| call.input_str(field)).and_then(|raw| self.tree.resolve(&self.cwd, raw)) else {
      return String::new();
    };
    if errored {
      self.diagnostics.errored_file_calls += 1;
      let output = call.result.as_ref().map_or("", |result| result.text.as_str());
      let tracked = self.cwd.clone();
      self.cwd = self.checked_note(self.noted_cwd(output, CWD_ERROR_NOTE), tracked);
    } else if call.name == WRITE_TOOL {
      self.tree.add_file(&path);
    }
    if let Some(file) = self.tracked(&path) {
      match errored {
        true => observed.add(file, TouchKind::Mentioned, format!("{} (failed)", call.name)),
        false => observed.add(file, kind, call.name.clone()),
      }
    }
    String::new()
  }

  fn observe_serena_edit(&mut self, call: &ToolCall, errored: bool, observed: &mut Observed) -> String {
    let Some(path) = call.input_str(SERENA_PATH_FIELD).and_then(|raw| self.tree.resolve(&Cwd::root(), raw)) else {
      return call.input.to_string();
    };
    self.diagnostics.errored_file_calls += u32::from(errored);
    if let Some(file) = self.tracked(&path) {
      match errored {
        true => observed.add(file, TouchKind::Mentioned, format!("{} (failed)", call.name)),
        false => observed.add(file, TouchKind::Edited, call.name.clone()),
      }
    }
    String::new()
  }

  fn observe_search_tool(&mut self, call: &ToolCall, errored: bool, observed: &mut Observed) -> String {
    let mut bases = vec![String::new()];
    bases.extend(self.cwd.in_tree().map(str::to_owned));
    let scope = call.input_str("path").unwrap_or_default();
    for base in [Cwd::root(), self.cwd.clone()] {
      let Some(path) = self.tree.resolve(&base, scope) else {
        continue;
      };
      if self.tree.is_dir(&path) {
        bases.push(path);
      } else if let Some(file) = self.tracked(&path).filter(|_| self.tree.is_file(&path) && !errored) {
        observed.add(file, TouchKind::Searched, call.name.clone());
        if call.input_str("output_mode") == Some(GREP_CONTENT_MODE) {
          observed.add(file, TouchKind::Shown, call.name.clone());
        }
      }
    }
    observed.naming_bases = Some(bases);
    observed.query = Some(["pattern", "glob"].iter().filter_map(|field| call.input_str(field)).collect::<Vec<_>>().join(" "));
    let mut query = call.input.clone();
    if let Some(fields) = query.as_object_mut() {
      fields.remove("path");
    }
    query.to_string()
  }

  fn observe_bash(&mut self, call: &ToolCall, observed: &mut Observed) -> String {
    let command = call.input_str("command").unwrap_or_default();
    let result = call.result.clone().unwrap_or_default();
    let before = self.cwd.clone();
    let trace = trace_bash(command, &result.text, &mut self.cwd, &mut self.tree);
    self.diagnostics.failed_cds += trace.failed_cds;
    self.diagnostics.unverified_cds += trace.unverified_cds;
    self.diagnostics.unparsed_bash_reads += trace.unparsed_reads;
    for access in &trace.accesses {
      let failed = access.outcome == AccessOutcome::Rejected;
      self.diagnostics.rejected_sed_edits += u32::from(failed);
      let Some(file) = self.tracked(&access.path) else {
        continue;
      };
      let kind = match access.kind {
        _ if failed => TouchKind::Mentioned,
        ShellAccessKind::Search => TouchKind::Searched,
        ShellAccessKind::Read => TouchKind::Read,
        ShellAccessKind::Edit => TouchKind::Edited,
      };
      let via = if failed {
        format!("Bash: {} (rejected)", access.form.label())
      } else {
        format!("Bash: {}", access.form.label())
      };
      if access.form == ShellForm::Search && !failed {
        observed.add(file, TouchKind::Shown, via.clone());
      }
      observed.add(file, kind, via);
    }
    for path in &trace.operands {
      if let Some(file) = self.tracked(path).filter(|file| !observed.input.contains_key(file)) {
        observed.add(file, TouchKind::Mentioned, "Bash operand");
      }
    }
    self.cwd = self.cwd_after(&result, before);
    let mut bases = trace.working_dirs;
    bases.extend(trace.listed_dirs);
    observed.naming_bases = Some(bases);
    observed.query = Some(if trace.search_patterns.is_empty() {
      command.to_owned()
    } else {
      trace.search_patterns.join(" ")
    });
    command.to_owned()
  }

  // Claude Code 2.1.289 keeps the directory a command ends in only when the command exits 0.
  fn cwd_after(&mut self, result: &ToolResult, before: Cwd) -> Cwd {
    if let Some(reset) = self.noted_cwd(&result.text, CWD_RESET_NOTICE) {
      self.diagnostics.cwd_resets += 1;
      return reset;
    }
    match &result.completion {
      Completion::Backgrounded { cwd_hint } => {
        let noted = cwd_hint.as_deref().and_then(|hint| self.noted_cwd(hint, CWD_HINT));
        self.checked_note(noted, before)
      }
      Completion::Finished { exit_note: Some(_) } => before,
      Completion::Finished { exit_note: None } | Completion::Unreported if result.is_error => before,
      Completion::Finished { exit_note: None } | Completion::Unreported => self.cwd.clone(),
    }
  }

  fn checked_note(&mut self, noted: Option<Cwd>, tracked: Cwd) -> Cwd {
    let Some(noted) = noted else {
      return tracked;
    };
    self.diagnostics.cwd_notes += 1;
    self.diagnostics.cwd_notes_contradicting += u32::from(noted != tracked);
    noted
  }

  fn named_in(&self, tool: &str, output: &str, bases: &[String]) -> BTreeMap<usize, ResultLines> {
    let mut bases: Vec<Cwd> = bases.iter().cloned().map(Cwd::InTree).collect();
    let headers: Vec<Cwd> = output
      .lines()
      .filter_map(|line| line.trim().strip_suffix(':'))
      .flat_map(|header| bases.iter().filter_map(|base| self.tree.resolve(base, header)).collect::<Vec<_>>())
      .filter(|dir| self.tree.is_dir(dir))
      .map(Cwd::InTree)
      .collect();
    for header in headers {
      if !bases.contains(&header) {
        bases.push(header);
      }
    }
    let basenames: Vec<&str> = self.files.iter().map(|file| file.path.rsplit('/').next().unwrap_or(&file.path)).collect();
    let mut named: BTreeMap<usize, ResultLines> = BTreeMap::new();
    let mut note = |file: usize, lines: ResultLines| {
      let known = named.entry(file).or_insert(lines);
      *known = (*known).max(lines);
    };
    for token in output.split(|c: char| c.is_whitespace() || matches!(c, ':' | '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '=' | '<' | '>' | '|' | '*')) {
      let token = token.trim_end_matches(['.', ',']);
      if !basenames.iter().any(|basename| token.ends_with(basename)) {
        continue;
      }
      let stripped = DIFF_SIDE_PREFIXES.iter().find_map(|prefix| token.strip_prefix(prefix));
      for candidate in std::iter::once(token).chain(stripped) {
        let lines = if carries_lines(output, candidate) { ResultLines::Carried } else { ResultLines::Absent };
        for file in bases.iter().filter_map(|base| self.tree.resolve(base, candidate)).filter_map(|path| self.tracked(&path)) {
          note(file, lines);
        }
      }
    }
    if tool.starts_with(ATLAS_TOOL_PREFIX) {
      for location in output.lines().flat_map(locations_in) {
        let in_repo = location.repo.as_ref().map(|repo| format!("{repo}/{}", location.path));
        let lines = if location.line.is_some() { ResultLines::Carried } else { ResultLines::Absent };
        for file in in_repo.iter().chain(std::iter::once(&location.path)).filter_map(|path| self.tracked(path)) {
          note(file, lines);
        }
      }
    }
    named
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResultLines {
  Absent,
  Carried,
}

fn carries_lines(output: &str, path: &str) -> bool {
  output.match_indices(path).any(|(start, _)| {
    let mut after = output[start + path.len()..].chars();
    matches!(after.next(), Some(':' | '-')) && after.next().is_some_and(|c| c.is_ascii_digit())
  })
}

fn string_values(value: &serde_json::Value) -> Vec<String> {
  match value {
    serde_json::Value::String(text) => vec![text.clone()],
    serde_json::Value::Array(items) => items.iter().flat_map(string_values).collect(),
    serde_json::Value::Object(fields) => fields.values().flat_map(string_values).collect(),
    _ => Vec::new(),
  }
}

pub(super) fn mentions(text: &str, suffix: &str) -> bool {
  let joins_name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-');
  text.match_indices(suffix).any(|(start, _)| {
    let before = text[..start].chars().next_back();
    let mut after = text[start + suffix.len()..].chars();
    let ends = match after.next() {
      Some('.') => !after.next().is_some_and(char::is_alphanumeric),
      Some(next) => !joins_name(next),
      None => true,
    };
    ends && !before.is_some_and(|c| joins_name(c) || c == '.')
  })
}

#[cfg(test)]
mod tests {
  use serde_json::{Value, json};

  use super::super::transcript::{AssistantTurn, TokenUsage};
  use super::*;

  const ROOT: &str = "/private/var/folders/x/ae-1/tree";
  const EXPRESS: &str = "crates/detect/src/detectors/express.rs";
  const VIEW: &str = "crates/detect/src/view.rs";
  const ENDPOINTS: &str = "crates/detect/tests/endpoints.rs";
  const IMPORTS: &str = "crates/detect/src/imports.rs";

  fn tree() -> TreeIndex {
    TreeIndex::new(
      [ROOT.to_owned()],
      [EXPRESS, VIEW, ENDPOINTS, "crates/detect/src/lib.rs", "crates/link/src/view.rs", "CLAUDE.md"].map(str::to_owned),
    )
  }

  struct Step {
    turn: u32,
    name: &'static str,
    input: Value,
    output: &'static str,
    is_error: bool,
    completion: Completion,
  }

  fn step(turn: u32, name: &'static str, input: Value, output: &'static str) -> Step {
    Step {
      turn,
      name,
      input,
      output,
      is_error: false,
      completion: Completion::default(),
    }
  }

  fn failed(step: Step) -> Step {
    Step { is_error: true, ..step }
  }

  fn completed(step: Step, completion: Completion) -> Step {
    Step { completion, ..step }
  }

  fn bash(turn: u32, command: &str, output: &'static str) -> Step {
    step(turn, "Bash", json!({ "command": command }), output)
  }

  fn file(path: &str) -> String {
    format!("{ROOT}/{path}")
  }

  fn transcript(steps: Vec<Step>) -> Transcript {
    let turns = steps.iter().map(|step| step.turn).max().unwrap_or(0) + 1;
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
          elapsed_ms: Some(index as u64 * 1000),
          result: Some(ToolResult {
            text: step.output.to_owned(),
            is_error: step.is_error,
            completion: step.completion,
          }),
          turn: step.turn,
        })
        .collect(),
      ..Transcript::default()
    }
  }

  fn touches(steps: Vec<Step>, tracked: &[&str]) -> SessionTouches {
    let tracked: Vec<String> = tracked.iter().map(|path| (*path).to_owned()).collect();
    session_touches(&transcript(steps), tree(), &tracked)
  }

  fn at(event: &Option<TouchEvent>) -> Option<(u32, u32)> {
    event.as_ref().map(|event| (event.at.turn, event.at.call))
  }

  fn via(event: &Option<TouchEvent>) -> Option<&str> {
    event.as_ref().map(|event| event.via.as_str())
  }

  #[test]
  fn should_separate_a_search_hit_from_a_targeted_search_a_read_and_an_edit() {
    let session = touches(
      vec![
        step(
          1,
          "Grep",
          json!({"pattern": "mounted_on", "path": "crates", "output_mode": "files_with_matches"}),
          "Found 2 files\ncrates/detect/src/detectors/express.rs\ncrates/link/src/compose.rs",
        ),
        step(2, "Read", json!({"file_path": file("crates/link/src/compose.rs")}), "1\tuse x;"),
        step(
          2,
          "Grep",
          json!({"pattern": "mounted_on", "path": EXPRESS, "output_mode": "content", "-C": 6}),
          "369:      (\"mount_prefix\", x),",
        ),
        step(3, "Read", json!({"file_path": file(EXPRESS), "offset": 230, "limit": 150}), "230\t    {"),
        step(4, "Edit", json!({"file_path": file(EXPRESS), "old_string": "a", "new_string": "b"}), "updated"),
      ],
      &[EXPRESS],
    );
    let express = &session.files[0];
    assert_eq!((session.turns, session.tool_calls), (5, 5));
    assert_eq!(express.presence, Presence::AtBase);
    assert_eq!(at(&express.named), Some((1, 1)));
    assert_eq!(via(&express.named), Some("Grep result"));
    assert_eq!(at(&express.searched), Some((2, 3)));
    assert_eq!(at(&express.read), Some((3, 4)));
    assert_eq!(at(&express.edited), Some((4, 5)));
    assert_eq!(express.mentioned, None);
    assert_eq!(express.touched().map(|event| event.at.call), Some(3));
    assert_eq!(express.named_before_addressed().map(|event| event.at.call), Some(1));
    assert_eq!(express.read.as_ref().and_then(|event| event.elapsed_ms), Some(3000));
  }

  #[test]
  fn should_not_count_a_result_as_naming_a_file_the_call_itself_addressed() {
    let session = touches(
      vec![
        bash(
          1,
          "grep -n mounted crates/detect/src/detectors/express.rs crates/detect/src/view.rs",
          "crates/detect/src/detectors/express.rs:12:x",
        ),
        step(
          2,
          "Edit",
          json!({"file_path": file(VIEW)}),
          "The file /private/var/folders/x/ae-1/tree/crates/detect/src/view.rs has been updated",
        ),
        step(3, "Read", json!({"file_path": file("CLAUDE.md")}), "see crates/detect/tests/endpoints.rs"),
      ],
      &[EXPRESS, VIEW, ENDPOINTS],
    );
    assert_eq!(session.files[0].named, None);
    assert_eq!(at(&session.files[0].searched), Some((1, 1)));
    assert_eq!(session.files[0].named_before_addressed(), None);
    assert_eq!(via(&session.files[1].searched), Some("Bash: grep"));
    assert_eq!(at(&session.files[1].edited), Some((2, 2)));
    assert_eq!(session.files[1].named, None);
    assert_eq!(session.files[2].named, None);
  }

  #[test]
  fn should_map_bash_forms_with_a_directory_that_persists_between_calls() {
    let session = touches(
      vec![
        bash(
          1,
          "cd crates/detect && ls src src/detectors",
          "src:\ndetectors\nlib.rs\nview.rs\n\nsrc/detectors:\nexpress.rs",
        ),
        bash(2, "cd src/detectors && sed -n 60,95p express.rs", "fn run() {}"),
        bash(
          3,
          "cd .. && grep -rn ImportFact . | head",
          "./view.rs:10:use ImportFact;\n./detectors/express.rs:3:use ImportFact;",
        ),
        bash(
          4,
          "python3 - <<'E'\np='view.rs'\ns=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nE\ncd ../../.. && cargo fmt",
          "",
        ),
        bash(
          5,
          "sed -i 's/a/b/' crates/detect/tests/endpoints.rs",
          "sed: 1: \"crates/detect/tests/e ...\": command c expects \\ followed by text",
        ),
        bash(6, "sed -i '' 's/a/b/' crates/detect/tests/endpoints.rs && cargo test", "ok"),
      ],
      &[EXPRESS, VIEW, ENDPOINTS],
    );
    let (express, view, endpoints) = (&session.files[0], &session.files[1], &session.files[2]);
    assert_eq!(at(&express.named), Some((1, 1)));
    assert_eq!(via(&express.named), Some("Bash result"));
    assert_eq!(at(&express.read), Some((2, 2)));
    assert_eq!(via(&express.read), Some("Bash: sed -n"));
    assert_eq!(at(&view.named), Some((1, 1)));
    assert_eq!(view.read, None);
    assert_eq!(at(&view.edited), Some((4, 4)));
    assert_eq!(via(&view.edited), Some("Bash: python script"));
    assert_eq!(at(&endpoints.mentioned), Some((5, 5)));
    assert_eq!(via(&endpoints.mentioned), Some("Bash: sed -i (rejected)"));
    assert_eq!(at(&endpoints.edited), Some((6, 6)));
    assert_eq!(session.diagnostics.rejected_sed_edits, 1);
    assert_eq!(session.diagnostics.unverified_cds, 0);
  }

  #[test]
  fn should_report_created_files_and_files_never_touched() {
    let session = touches(
      vec![
        step(
          1,
          "Glob",
          json!({"pattern": "**/view.rs"}),
          "/private/var/folders/x/ae-1/tree/crates/detect/src/view.rs\n/private/var/folders/x/ae-1/tree/crates/link/src/view.rs",
        ),
        step(2, "Write", json!({"file_path": file(IMPORTS), "content": "pub struct Imports;"}), "created"),
        bash(3, "cd crates/detect/src && cat imports.rs && ./node_modules/.bin/vitest run view.rs", "pub struct Imports;"),
      ],
      &[IMPORTS, VIEW, ENDPOINTS],
    );
    let (imports, view, endpoints) = (&session.files[0], &session.files[1], &session.files[2]);
    assert_eq!(imports.presence, Presence::Created);
    assert_eq!(at(&imports.edited), Some((2, 2)));
    assert_eq!(at(&imports.read), Some((3, 3)));
    assert_eq!(view.presence, Presence::AtBase);
    assert_eq!(at(&view.named), Some((1, 1)));
    assert_eq!(at(&view.mentioned), Some((3, 3)));
    assert_eq!(via(&view.mentioned), Some("Bash operand"));
    assert_eq!(view.touched(), None);
    assert_eq!((endpoints.named.clone(), endpoints.addressed()), (None, None));
  }

  #[test]
  fn should_not_count_failed_file_tool_calls_as_reads_or_edits() {
    let session = touches(
      vec![
        failed(step(1, "Read", json!({"file_path": file(VIEW)}), "<tool_use_error>File does not exist.</tool_use_error>")),
        failed(step(
          2,
          "Edit",
          json!({"file_path": file(VIEW)}),
          "<tool_use_error>File has not been read yet.</tool_use_error>",
        )),
        step(3, "Read", json!({"file_path": "/etc/hosts"}), "127.0.0.1"),
      ],
      &[VIEW],
    );
    assert_eq!(at(&session.files[0].mentioned), Some((1, 1)));
    assert_eq!(via(&session.files[0].mentioned), Some("Read (failed)"));
    assert_eq!((session.files[0].read.clone(), session.files[0].edited.clone()), (None, None));
    assert_eq!(session.diagnostics.errored_file_calls, 2);
  }

  #[test]
  fn should_count_a_serena_edit_by_its_project_relative_path_as_an_edit() {
    let session = touches(
      vec![
        bash(1, "cd crates", "x"),
        step(
          2,
          "mcp__serena__replace_symbol_body",
          json!({"name_path": "render", "relative_path": "crates/detect/src/view.rs", "body": "fn render() {}"}),
          "OK",
        ),
        failed(step(
          3,
          "mcp__serena__insert_after_symbol",
          json!({"name_path": "x", "relative_path": "crates/detect/src/detectors/express.rs", "body": "y"}),
          "error",
        )),
      ],
      &[VIEW, EXPRESS],
    );
    assert_eq!(
      (at(&session.files[0].edited), via(&session.files[0].edited)),
      (Some((2, 2)), Some("mcp__serena__replace_symbol_body"))
    );
    assert_eq!(session.files[1].edited, None);
    assert_eq!(via(&session.files[1].mentioned), Some("mcp__serena__insert_after_symbol (failed)"));
    assert_eq!(session.diagnostics.errored_file_calls, 1);
  }

  #[test]
  fn should_follow_a_cwd_reset_notice_and_name_files_from_atlas_results() {
    let session = touches(
      vec![
        bash(1, "cd /tmp && ls", "x\nShell cwd was reset to /private/var/folders/x/ae-1/tree"),
        bash(2, "cat crates/detect/src/view.rs", "x"),
        step(
          3,
          "mcp__code-atlas__atlas_context",
          json!({"task": "fix mounts"}),
          "MUST READ\n  tree:crates/detect/src/detectors/express.rs:108 emit_cross_file_mount [resolved]",
        ),
      ],
      &[VIEW, EXPRESS],
    );
    assert_eq!(at(&session.files[0].read), Some((2, 2)));
    assert_eq!(at(&session.files[1].named), Some((3, 3)));
    assert_eq!(via(&session.files[1].named), Some("mcp__code-atlas__atlas_context result"));
    assert_eq!(session.diagnostics.cwd_resets, 1);
  }

  #[test]
  fn should_keep_the_previous_directory_unless_the_command_exited_cleanly() {
    let quiet = Completion::Finished {
      exit_note: Some("No matches found".to_owned()),
    };
    let background = Completion::Backgrounded {
      cwd_hint: Some(format!(
        "Session cwd remains {ROOT}/crates/detect; directory changes made by the backgrounded command do not apply."
      )),
    };
    let session = touches(
      vec![
        completed(bash(1, "cd crates/detect/src && grep -rn nothing .", ""), quiet),
        bash(2, "cat view.rs", "cat: view.rs: No such file or directory"),
        failed(bash(3, "cd crates/detect/src && false", "Exit code 1")),
        bash(4, "cat view.rs; cd crates/detect && true", ""),
        completed(bash(5, "cd src && sleep 500", "moved to the background"), background),
        bash(6, "cat src/view.rs", "x"),
        bash(7, "cd nowhere; cat src/lib.rs", "(eval):cd:1: no such file or directory: nowhere"),
        bash(8, "cat src/view.rs", "x"),
      ],
      &[VIEW, "crates/detect/src/lib.rs"],
    );
    assert_eq!(at(&session.files[0].read), Some((6, 6)));
    assert_eq!(at(&session.files[1].read), Some((7, 7)));
    assert_eq!((session.diagnostics.cwd_notes, session.diagnostics.cwd_notes_contradicting), (1, 0));
    assert_eq!((session.diagnostics.failed_cds, session.diagnostics.unverified_cds), (1, 0));
  }

  #[test]
  fn should_count_a_harness_note_that_contradicts_the_tracked_directory() {
    let session = touches(
      vec![
        bash(1, "cd crates/detect && true", ""),
        failed(step(
          2,
          "Edit",
          json!({"file_path": file("src/view.rs")}),
          "<tool_use_error>File does not exist. Note: your current working directory is /private/var/folders/x/ae-1/tree.</tool_use_error>",
        )),
        bash(3, "cat crates/detect/src/view.rs", "x"),
      ],
      &[VIEW],
    );
    assert_eq!(at(&session.files[0].read), Some((3, 3)));
    assert_eq!((session.diagnostics.cwd_notes, session.diagnostics.cwd_notes_contradicting), (1, 1));
  }

  #[test]
  fn should_match_a_mention_only_at_path_boundaries() {
    assert!(mentions("vitest run src/detectors/express.rs --coverage", "detectors/express.rs"));
    assert!(mentions("see express.rs.", "express.rs"));
    assert!(!mentions("cat myexpress.rs", "express.rs"));
    assert!(!mentions("cat express.rs.bak express.rsx", "express.rs"));
  }
}
