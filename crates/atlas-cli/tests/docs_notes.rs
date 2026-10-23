use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use atlas_test_lsp::hermetic::hermetic_catl;
use serde_json::{Value, json};
use tempfile::TempDir;

const REPO: &str = "shelf-web";
const RETRIES: &str = ".code-atlas/notes/retries.md";

struct Sandbox {
  root: TempDir,
}

impl Sandbox {
  fn new() -> Self {
    let sandbox = Self {
      root: tempfile::tempdir().unwrap(),
    };
    for directory in ["atlas/fake-bin", "atlas/claude", "atlas/codex", "work"] {
      fs::create_dir_all(sandbox.path(directory)).unwrap();
    }
    sandbox
  }

  fn path(&self, relative: &str) -> PathBuf {
    fs::canonicalize(self.root.path()).unwrap().join(relative)
  }

  fn command(&self, cwd: &Path, args: &[&str]) -> Command {
    let mut command = hermetic_catl(env!("CARGO_BIN_EXE_catl"), &self.path("atlas"));
    command.args(args).current_dir(cwd);
    command
  }

  fn run(&self, cwd: &Path, args: &[&str]) -> Output {
    self.command(cwd, args).stdin(Stdio::null()).output().unwrap()
  }

  fn hook(&self, cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut arguments = args.to_vec();
    arguments.extend(["hook", "notes"]);
    let mut child = self
      .command(cwd, &arguments)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
  }

  fn shelf(&self) -> PathBuf {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-query/tests/docs-corpus").join(REPO);
    let root = self.path("work").join(REPO);
    copy_dir(&corpus, &root);
    let path = root.display().to_string();
    assert_ok(&self.run(&root, &["project", "create", "shelf", "--repo", &path]));
    assert_ok(&self.run(&root, &["index"]));
    root
  }
}

fn copy_dir(from: &Path, to: &Path) {
  fs::create_dir_all(to).unwrap();
  for entry in fs::read_dir(from).unwrap() {
    let entry = entry.unwrap();
    let target = to.join(entry.file_name());
    match entry.file_type().unwrap().is_dir() {
      true => copy_dir(&entry.path(), &target),
      false => drop(fs::copy(entry.path(), &target).unwrap()),
    }
  }
}

fn stdout(output: &Output) -> String {
  String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
  String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
  output.status.code().unwrap()
}

fn assert_ok(output: &Output) {
  assert_eq!(code(output), 0, "stdout: {}\nstderr: {}", stdout(output), stderr(output));
}

fn without_freshness(text: &str) -> String {
  text.lines().skip(1).map(|line| format!("{line}\n")).collect()
}

type EventFor = Box<dyn Fn(&str) -> String>;

fn post_tool_use(session: &str, cwd: &Path, file: &Path) -> String {
  json!({
    "session_id": session,
    "cwd": cwd,
    "hook_event_name": "PostToolUse",
    "tool_name": "Read",
    "tool_input": {"file_path": file},
    "tool_response": {"type": "text"}
  })
  .to_string()
}

#[test]
fn should_print_docs_as_text_and_json_and_exit_4_when_nothing_matches() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();
  let readme = root.join("README.md");

  let text = sandbox.run(&root, &["docs", "retries backoff", "--limit", "1"]);
  let json = sandbox.run(&root, &["query", "docs", "--about", "backoffDelay", "--json"]);
  let none = sandbox.run(&root, &["docs", "zebra"]);
  let neither = sandbox.run(&root, &["docs"]);

  assert_ok(&text);
  assert_eq!(
    without_freshness(&stdout(&text)),
    format!(
      "docs matching \"retries backoff\" (1 of 3 sections)
  shelf-web:README.md > Shelf web > Retries 5-9 [doc]
    file: {}
    `createShelfClient` builds the client; failed reads wait `backoffDelay()` before the next attempt. See [the backoff helper](src/lib/backoff.ts).
    mentions `createShelfClient` -> createShelfClient function shelf-web:src/lib/shelf-client.ts:6 [heuristic 0.70]
    mentions `backoffDelay` -> backoffDelay function shelf-web:src/lib/backoff.ts:3 [heuristic 0.70]
    mentions `src/lib/backoff.ts` -> src/lib/backoff.ts file shelf-web:src/lib/backoff.ts [heuristic 0.70]
",
      readme.display()
    )
  );
  assert_ok(&json);
  let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
  let sections: Vec<(&str, &str, &str)> = document["answer"]["sections"]
    .as_array()
    .unwrap()
    .iter()
    .map(|section| {
      (
        section["path"].as_str().unwrap(),
        section["kind"].as_str().unwrap(),
        section["note_status"].as_str().unwrap_or("-"),
      )
    })
    .collect();
  assert_eq!(sections, [(RETRIES, "note", "unstamped"), ("README.md", "doc", "-")]);
  assert_eq!(document["answer"]["about"]["short"], "backoffDelay");
  assert_eq!(document["answer"]["sections"][1]["mentions"][0]["certainty"]["tag"], "[heuristic 0.70]");
  assert_eq!(code(&none), 4);
  assert_eq!(
    without_freshness(&stdout(&none)),
    "docs matching \"zebra\" (0 of 0 sections)\n  none: no section of a repository doc or note matches\n"
  );
  assert_eq!(code(&neither), 2);
}

#[test]
fn should_list_notes_stamp_one_and_list_it_as_stale_after_its_code_changes() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();
  let note = root.join(RETRIES);

  let listed = sandbox.run(&root, &["notes", "list"]);
  let none_stale = sandbox.run(&root, &["notes", "list", "--stale"]);
  let stamped = sandbox.run(&root, &["notes", "stamp", RETRIES]);
  let restamped = sandbox.run(&root, &["notes", "stamp", "shelf-web:.code-atlas/notes/retries.md"]);
  let refused = sandbox.run(&root, &["notes", "stamp", "README.md"]);
  let backoff = root.join("src/lib/backoff.ts");
  fs::write(&backoff, fs::read_to_string(&backoff).unwrap().replace("30000", "60000")).unwrap();
  let stale = sandbox.run(&root, &["notes", "list", "--stale", "--json"]);

  assert_ok(&listed);
  assert_eq!(
    without_freshness(&stdout(&listed)),
    format!(
      "notes (2)
  shelf-web:.code-atlas/notes/client.md \"Client construction\" [unstamped]
    file: {}
    anchor shelf-web:src/lib/shelf-client.ts#createShelfClient [unstamped] -> createShelfClient function shelf-web:src/lib/shelf-client.ts:6 (from a mention [heuristic 0.70])
  shelf-web:.code-atlas/notes/retries.md \"Retry budget\" [unstamped]
    file: {}
    anchor shelf-web:src/lib/backoff.ts#backoffDelay [unstamped] -> backoffDelay function shelf-web:src/lib/backoff.ts:3
",
      root.join(".code-atlas/notes/client.md").display(),
      note.display()
    )
  );
  assert_eq!(code(&none_stale), 4);
  assert_eq!(
    without_freshness(&stdout(&none_stale)),
    "stale notes (0)\n  none: every note under .code-atlas/notes/ matches the code it is anchored to\n"
  );
  assert_ok(&stamped);
  assert!(
    stdout(&stamped).contains("  applied\nchanged 1 file(s), 1 edit(s):\n  shelf-web:.code-atlas/notes/retries.md  1\n"),
    "{}",
    stdout(&stamped)
  );
  assert_eq!(
    fs::read_to_string(&note).unwrap(),
    "---\nanchors:\n  - symbol: shelf-web:src/lib/backoff.ts#backoffDelay\n    body: bb6f402ed561\n---\n# Retry budget\n\n`backoffDelay` caps the wait at thirty seconds; keep the cap when changing the curve.\n"
  );
  assert_ok(&restamped);
  assert!(
    stdout(&restamped).contains("every anchor already carries the hash of its current code; nothing was written"),
    "{}",
    stdout(&restamped)
  );
  assert_eq!(code(&refused), 5);
  assert!(
    stdout(&refused).contains("shelf-web:README.md is not a note: notes live under .code-atlas/notes/; nothing was written"),
    "{}",
    stdout(&refused)
  );
  assert_ok(&stale);
  let document: Value = serde_json::from_str(&stdout(&stale)).unwrap();
  assert_eq!(document["answer"]["notes"].as_array().unwrap().len(), 1);
  assert_eq!(document["answer"]["notes"][0]["path"], RETRIES);
  assert_eq!(document["answer"]["notes"][0]["status"], "stale");
  assert_eq!(document["answer"]["notes"][0]["anchors"][0]["status"], "stale");
}

#[test]
fn should_print_the_cheapest_path_and_exit_4_when_none_is_within_the_depth() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();

  let found = sandbox.run(&root, &["path", "loadWithBackoff", "ShelfClient"]);
  let shallow = sandbox.run(&root, &["query", "path", "loadWithBackoff", "ShelfClient", "--max-depth", "1"]);

  assert_ok(&found);
  assert_eq!(
    without_freshness(&stdout(&found)),
    "path between loadWithBackoff function shelf-web:src/lib/backoff.ts:7 and ShelfClient class shelf-web:src/lib/shelf-client.ts:10: 2 hops, cost 2
  loadWithBackoff function shelf-web:src/lib/backoff.ts:7
  calls createShelfClient function shelf-web:src/lib/shelf-client.ts:6 ([resolved])
  instantiates ShelfClient class shelf-web:src/lib/shelf-client.ts:10 ([resolved])
"
  );
  assert_eq!(code(&shallow), 4);
  assert_eq!(
    without_freshness(&stdout(&shallow)),
    "no path within 1 hops between loadWithBackoff function shelf-web:src/lib/backoff.ts:7 and ShelfClient class shelf-web:src/lib/shelf-client.ts:10 (3 entities visited)\n"
  );
}

#[test]
fn should_append_the_doc_map_to_the_overview_with_docs() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();

  let mapped = sandbox.run(&root, &["query", "overview", "--docs"]);

  assert_ok(&mapped);
  assert!(
    stdout(&mapped).ends_with(
      "docs\n  shelf-web: 2 mapped docs, 2 notes (0 stale)\n    README.md\n      Shelf web\n      Shelf web > Retries\n      Shelf web > Labels\n    docs/design.md\n      Design\n      Design > Paging\n"
    ),
    "{}",
    stdout(&mapped)
  );
}

#[test]
fn should_note_the_notes_anchored_to_a_read_file_once_per_session_and_say_when_they_are_stale() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();
  let backoff = root.join("src/lib/backoff.ts");
  let note = root.join(RETRIES);
  let note_for = |status: &str| {
    format!(
      "Code Atlas: notes earlier sessions left about shelf-web:src/lib/backoff.ts:
  shelf-web:.code-atlas/notes/retries.md — {}
    Retry budget [{status}]: `backoffDelay` caps the wait at thirty seconds; keep the cap when changing the curve.
Read the note before changing the code it describes; a stale note may describe code that is gone.",
      note.display()
    )
  };

  let first = sandbox.hook(&root, &[], &post_tool_use("session-a", &root, &backoff));
  let again = sandbox.hook(&root, &["--log-level", "debug"], &post_tool_use("session-a", &root, &backoff));
  assert_ok(&sandbox.run(&root, &["notes", "stamp", RETRIES]));
  fs::write(&backoff, fs::read_to_string(&backoff).unwrap().replace("30000", "60000")).unwrap();
  assert_ok(&sandbox.run(&root, &["status"]));
  let stale = sandbox.hook(&root, &[], &post_tool_use("session-b", &root, &backoff));

  assert_ok(&first);
  assert_eq!(
    serde_json::from_str::<Value>(&stdout(&first)).unwrap(),
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": note_for("unstamped")}})
  );
  assert_eq!((code(&again), stdout(&again)), (0, String::new()));
  assert!(stderr(&again).contains("reason=this file was already noted in this session"), "{}", stderr(&again));
  let context = serde_json::from_str::<Value>(&stdout(&stale)).unwrap()["hookSpecificOutput"]["additionalContext"].clone();
  assert_eq!(context, json!(note_for("stale: the code changed after this note was written")));
  assert!(!stdout(&first).contains("atlas_"));
}

#[test]
fn should_stay_silent_and_exit_0_for_bad_input_unanchored_files_and_note_files() {
  let sandbox = Sandbox::new();
  let root = sandbox.shelf();
  let cwd = root.clone();
  let event = |file: PathBuf| {
    let cwd = cwd.clone();
    move |session: &str| post_tool_use(session, &cwd, &file)
  };
  let cases: [(EventFor, &str); 4] = [
    (Box::new(|_| "not json".to_owned()), "the hook input is not JSON"),
    (Box::new(event(root.join("src/ui/format.ts"))), "reason=no note is anchored to the file"),
    (Box::new(event(root.join(RETRIES))), "reason=the file is a doc or not a source or manifest file"),
    (Box::new(event(PathBuf::from("src/lib/backoff.ts"))), "no notes: not a PostToolUse event"),
  ];

  for (input, reason) in &cases {
    let quiet = sandbox.hook(&root, &[], &input("quiet"));
    assert_eq!((code(&quiet), stdout(&quiet), stderr(&quiet)), (0, String::new(), String::new()), "{reason}");
    let logged = sandbox.hook(&root, &["--log-level", "debug"], &input("logged"));
    assert_eq!((code(&logged), stdout(&logged)), (0, String::new()), "{reason}");
    assert!(stderr(&logged).contains(reason), "{reason}:\n{}", stderr(&logged));
  }
}
