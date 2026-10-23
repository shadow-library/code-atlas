use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use atlas_test_lsp::hermetic::hermetic_catl;
use serde_json::{Value, json};
use tempfile::TempDir;

const REPOS: [&str; 2] = ["notes-web", "notes-server"];
const OP_SWITCH: &str = "src/ops/op-switch.ts";
const SYNC_JOBS: &str = "src/sync/sync-jobs.ts";

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

  fn repo(&self, name: &str) -> PathBuf {
    self.path("work").join(name)
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
    arguments.extend(["hook", "counterpart"]);
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

  fn notes(&self) -> PathBuf {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-query/tests/counterpart-corpus");
    let mut arguments = vec!["project".to_owned(), "create".to_owned(), "notes".to_owned()];
    for name in REPOS {
      copy_dir(&corpus.join(name), &self.repo(name));
      arguments.extend(["--repo".to_owned(), self.repo(name).display().to_string()]);
    }
    let web = self.repo("notes-web");
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    assert_ok(&self.run(&web, &arguments));
    assert_ok(&self.run(&web, &["index"]));
    web
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

#[test]
fn should_print_the_counterparts_of_a_file_as_text_and_json_and_exit_4_when_there_are_none() {
  let sandbox = Sandbox::new();
  let web = sandbox.notes();
  let specs = sandbox.repo("notes-server").join("src/ops/op-specs.ts");

  let text = sandbox.run(&web, &["counterparts", "notes-web:src/ops/op-switch.ts"]);
  let json = sandbox.run(&web, &["query", "counterparts", "notes-web:src/ops/op-switch.ts", "--json"]);
  let none = sandbox.run(&web, &["counterparts", "notes-web:src/ui/format.ts"]);
  let none_json = sandbox.run(&web, &["counterparts", "notes-web:src/ui/format.ts", "--json"]);

  assert_ok(&text);
  assert_eq!(
    without_freshness(&stdout(&text)),
    format!(
      "counterparts of notes-web:src/ops/op-switch.ts (1)
  notes-server:src/ops/op-specs.ts [heuristic 0.44] score=31.45
    file: {}
    shared values (14), rarest: 'draft_slug', 'revision_mark', 'section_anchor', 'page.archive_draft', 'page.create_draft'
    most shared in DECLARED_OP_SPECS 6-20, WIRE_FIELDS 22
    only on that side, next to shared values:
      14  'page.merge_drafts': {{ required: {{ source_slug: 'string', target_slug: 'string' }}, optional: {{}} }},
      17  'page.tag_note': {{ required: {{ draft_slug: 'string', tag: 'string' }}, optional: {{}} }},
      22  export const WIRE_FIELDS = ['draft_slug', 'section_anchor', 'revision_mark', 'merge_receipt'];
",
      specs.display()
    )
  );
  assert_ok(&json);
  let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
  let counterpart = &document["answer"]["counterparts"][0];
  assert_eq!(document["answer"]["outcome"], "found");
  assert_eq!(document["answer"]["file"], json!({"repo": "notes-web", "path": OP_SWITCH, "line": null}));
  assert_eq!(
    (&counterpart["repo"], &counterpart["path"], &counterpart["absolute_path"], &counterpart["score"]),
    (&json!("notes-server"), &json!("src/ops/op-specs.ts"), &json!(specs.display().to_string()), &json!(31.45))
  );
  assert_eq!(counterpart["certainty"]["tag"], "[heuristic 0.44]");
  assert_eq!(counterpart["shared"]["count"], 14);
  assert_eq!(
    counterpart["symbols"][0],
    json!({"name": "DECLARED_OP_SPECS", "kind": "constant", "start_line": 6, "end_line": 20, "shared_lines": 12})
  );
  assert_eq!(counterpart["only_there"][0]["token"], "page.merge_drafts");
  assert_eq!(code(&none), 4);
  assert_eq!(
    without_freshness(&stdout(&none)),
    "counterparts of notes-web:src/ui/format.ts (0)\n  none: no file in another repo or package shares enough rare string values and keys with it\n"
  );
  assert_eq!(code(&none_json), 4);
  assert_eq!(serde_json::from_str::<Value>(&stdout(&none_json)).unwrap()["answer"]["counterparts"], json!([]));
}

type EventFor<'e> = &'e dyn Fn(&str) -> String;

fn post_tool_use(session: &str, cwd: &Path, file: &Path) -> String {
  json!({
    "session_id": session,
    "transcript_path": "/tmp/t.jsonl",
    "cwd": cwd,
    "permission_mode": "default",
    "hook_event_name": "PostToolUse",
    "tool_name": "Read",
    "tool_input": {"file_path": file},
    "tool_response": {"type": "text"},
    "tool_use_id": "toolu_1"
  })
  .to_string()
}

#[test]
fn should_note_the_counterpart_of_a_read_file_once_per_session_and_file() {
  let sandbox = Sandbox::new();
  let web = sandbox.notes();
  let jobs = web.join(SYNC_JOBS);
  let registry = sandbox.repo("notes-server").join("src/jobs/sync-registry.ts");

  let first = sandbox.hook(&web, &[], &post_tool_use("session-a", &web, &jobs));
  let again = sandbox.hook(&web, &["--log-level", "debug"], &post_tool_use("session-a", &web, &jobs));
  let other_session = sandbox.hook(&web, &[], &post_tool_use("session-b", &web, &jobs));

  assert_ok(&first);
  let note = format!(
    "Code Atlas [heuristic 0.64]: a linked repo holds the other side of notes-web:src/sync/sync-jobs.ts (28 shared values such as 'conflict_hint', 'outline_cursor'):
  notes-server:src/jobs/sync-registry.ts — {}
    most shared in SYNC_FIELDS 1-28
    only on that side, next to shared values:
      6  'sync.hydrate_outline': ['outline_cursor', 'hydration_window'],
      11  'sync.evict_shelf': ['shelf_token', 'eviction_reason'],
Read that file before changing what this one sends, reads or names.",
    registry.display()
  );
  assert_eq!(
    serde_json::from_str::<Value>(&stdout(&first)).unwrap(),
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": note}})
  );
  assert!(note.chars().count() <= 1_200);
  assert_eq!((code(&again), stdout(&again)), (0, String::new()));
  assert!(stderr(&again).contains("reason=this file was already noted in this session"), "{}", stderr(&again));
  assert_eq!(stdout(&other_session), stdout(&first));
  assert_eq!(fs::read_dir(sandbox.path("atlas/data/hooks/counterpart")).unwrap().count(), 4);
}

#[test]
fn should_answer_a_file_below_the_hook_floor_once_per_session_and_index_generation() {
  let sandbox = Sandbox::new();
  let web = sandbox.notes();
  let switch = web.join(OP_SWITCH);
  let read = |session: &str| sandbox.hook(&web, &["--log-level", "info"], &post_tool_use(session, &web, &switch));

  let first = read("session-f");
  let again = read("session-f");
  fs::write(web.join("src/ui/format.ts"), "export const CHANGED = 1;\n").unwrap();
  assert_ok(&sandbox.run(&web, &["status"]));
  let next_generation = read("session-f");

  for (output, reason) in [
    (&first, "reason=every counterpart is below the hook's confidence floor of 0.60"),
    (&again, "reason=this file was already answered in this session at this index generation"),
    (&next_generation, "reason=every counterpart is below the hook's confidence floor of 0.60"),
  ] {
    assert_eq!((code(output), stdout(output)), (0, String::new()), "{reason}");
    assert!(stderr(output).contains(reason), "{reason}:\n{}", stderr(output));
  }
}

#[test]
fn should_stay_silent_and_exit_0_whenever_it_cannot_note_a_counterpart_from_a_fresh_index() {
  let sandbox = Sandbox::new();
  let web = sandbox.notes();
  let elsewhere = sandbox.path("work");
  let outside = elsewhere.join("elsewhere.ts");
  fs::write(&outside, "export const OTHER = 'page.create_draft';\n").unwrap();
  let event = |session: &str, file: &Path| post_tool_use(session, &web, file);

  let silent: [(EventFor, &str); 9] = [
    (&|_| "not json".to_owned(), "the hook input is not JSON"),
    (
      &|session| event(session, &web.join(SYNC_JOBS)).replace("PostToolUse", "PreToolUse"),
      "no counterpart note: not a PostToolUse event",
    ),
    (
      &|session| event(session, Path::new("src/sync/sync-jobs.ts")),
      "no counterpart note: not a PostToolUse event",
    ),
    (&|session| event(session, &web.join("README.md")), "reason=the file is not a Rust or TypeScript source file"),
    (
      &|session| event(session, &web.join("src/ops/op-switch.test.ts")),
      "reason=the file is a test or generated file",
    ),
    (&|session| event(session, &web.join("tests/sync-jobs.ts")), "reason=the file is a test or generated file"),
    (&|session| event(session, &outside), "reason=the file is in no repo of the project"),
    (
      &|session| post_tool_use(session, &elsewhere, &outside),
      "reason=no project resolves for the working directory",
    ),
    (&|session| event(session, &web.join("src/ui/format.ts")), "reason=the file has no counterpart"),
  ];
  fs::create_dir_all(web.join("tests")).unwrap();
  fs::copy(web.join(SYNC_JOBS), web.join("tests/sync-jobs.ts")).unwrap();
  assert_ok(&sandbox.run(&web, &["status"]));
  for (index, (input, reason)) in silent.iter().enumerate() {
    let quiet = sandbox.hook(&web, &[], &input(&format!("quiet-{index}")));
    assert_eq!((code(&quiet), stdout(&quiet), stderr(&quiet)), (0, String::new(), String::new()), "{reason}");
    let logged = sandbox.hook(&web, &["--log-level", "debug"], &input(&format!("logged-{index}")));
    assert_eq!((code(&logged), stdout(&logged)), (0, String::new()), "{reason}");
    assert!(stderr(&logged).contains(reason), "{reason}:\n{}", stderr(&logged));
    assert!(!stderr(&logged).contains("page.create_draft"), "file text leaked into the log:\n{}", stderr(&logged));
    let info = sandbox.hook(&web, &["--log-level", "info"], &input(&format!("info-{index}")));
    assert!(!stderr(&info).contains(&*elsewhere.to_string_lossy()), "a path leaked into the log:\n{}", stderr(&info));
  }
  let markers = || fs::read_dir(sandbox.path("atlas/data/hooks/counterpart")).map_or(0, Iterator::count);
  let answered = markers();

  fs::write(web.join("src/ui/format.ts"), "export const CHANGED = 1;\n").unwrap();
  let stale = sandbox.hook(&web, &["--log-level", "info"], &event("session-s", &web.join(SYNC_JOBS)));
  assert_eq!((code(&stale), stdout(&stale)), (0, String::new()));
  assert!(stderr(&stale).contains("reason=the index is not fresh"), "{}", stderr(&stale));
  assert_eq!(markers(), answered);
  assert_ok(&sandbox.run(&web, &["status"]));
  let fresh = sandbox.hook(&web, &[], &event("session-s", &web.join(SYNC_JOBS)));
  assert!(stdout(&fresh).contains("notes-server:src/jobs/sync-registry.ts"), "{}", stdout(&fresh));
}
