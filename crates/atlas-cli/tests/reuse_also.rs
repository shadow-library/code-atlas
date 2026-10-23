use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use atlas_test_lsp::hermetic::hermetic_catl;
use serde_json::{Value, json};
use tempfile::TempDir;

const BACKOFF_COPY: &str = "export function pauseAfterFailure(problem: unknown, count: number): number {
  const low = 250;
  const high = 30_000;
  if (typeof problem === 'object' && problem !== null && 'retryAfter' in problem) {
    const wanted = Number((problem as { retryAfter: unknown }).retryAfter);
    if (Number.isFinite(wanted) && wanted > 0) {
      return Math.min(wanted * 1000, high);
    }
  }
  const doubled = low * 2 ** Math.max(0, count - 1);
  return Math.min(doubled, high);
}
";
const SMALL: &str = "export function sumOf(values: number[]): number {\n  return values.reduce((total, value) => total + value, 0);\n}\n";

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

  fn piped(&self, cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut child = self.command(cwd, args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
  }

  fn shelf(&self) -> PathBuf {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-query/tests/reuse-corpus/shelf-app");
    let repo = self.path("work").join("shelf-app");
    copy_dir(&corpus, &repo);
    let root = repo.display().to_string();
    assert_ok(&self.run(&repo, &["project", "create", "shelf", "--repo", &root]));
    assert_ok(&self.run(&repo, &["index"]));
    repo
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
fn should_print_reuse_candidates_for_text_on_stdin_and_exit_4_when_there_are_none() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();

  let found = sandbox.piped(&repo, &["reuse-check", "src/ui/retry-badge.ts", "--stdin"], BACKOFF_COPY);
  let json = sandbox.piped(&repo, &["query", "reuse-check", "shelf-app:src/ui/retry-badge.ts", "--stdin", "--json"], BACKOFF_COPY);
  let none = sandbox.piped(&repo, &["reuse-check", "src/ui/sum.ts", "--stdin"], SMALL);
  let bad_range = sandbox.run(&repo, &["reuse-check", "src/lib/backoff.ts", "--lines", "9-2"]);

  assert_ok(&found);
  assert_eq!(
    without_freshness(&stdout(&found)),
    "reuse check of shelf-app:src/ui/retry-badge.ts (1 function checked, 2 candidates)
  pauseAfterFailure (new) 1-12, 102 tokens
    [heuristic 1.00] waitBeforeRetry shelf-app:src/lib/backoff.ts:1-12 — near duplicate 1.00
      function waitBeforeRetry(error: unknown, attempt: number): number
    [heuristic 1.00] requeueDelay shelf-app:src/jobs/requeue.ts:1-12 — near duplicate 1.00
      function requeueDelay(reason: unknown, tries: number): number
"
  );
  assert_ok(&json);
  let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
  assert_eq!(document["answer"]["index_state"], "not_indexed");
  assert_eq!(
    document["answer"]["units"][0]["candidates"][0]["reason"],
    json!({"signal": "near_duplicate", "jaccard": 1.0})
  );
  assert_eq!(code(&none), 4);
  assert_eq!(
    without_freshness(&stdout(&none)),
    "reuse check of shelf-app:src/ui/sum.ts (1 function checked, 0 candidates)\n  sumOf (new) 1-3, 20 tokens\n  none: no existing function looks like the checked code\n"
  );
  assert_eq!(code(&bad_range), 2);
}

#[test]
fn should_list_the_files_that_may_also_change_and_exit_4_when_there_are_none() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();

  let found = sandbox.run(&repo, &["also-change", "src/api/client.ts"]);
  let limited = sandbox.run(&repo, &["query", "also-change", "shelf-app:src/api/client.ts", "--limit", "1", "--json"]);
  let none = sandbox.run(&repo, &["also-change", "src/jobs/requeue.ts"]);

  assert_ok(&found);
  assert_eq!(
    without_freshness(&stdout(&found)),
    "also change with shelf-app:src/api/client.ts (3)
  shelf-app:src/state/shelf-store.ts score=1.00 — calls changed function fetchShelf
  shelf-app:src/lib/backoff.ts score=0.36 — called by changed function fetchShelf
  shelf-app:src/ui/shelf-view.ts score=0.27 — via shelf-app:src/state/shelf-store.ts, 2 hops from a changed file
"
  );
  assert_ok(&limited);
  let document: Value = serde_json::from_str(&stdout(&limited)).unwrap();
  assert_eq!(document["answer"]["files"].as_array().unwrap().len(), 1);
  assert_eq!(document["answer"]["files"][0]["certainty"]["tag"], "[resolved]");
  assert_eq!(code(&none), 4);
}

fn edit_event(name: &str, session: &str, cwd: &Path, tool: &str, input: Value) -> String {
  json!({
    "session_id": session,
    "transcript_path": "/tmp/t.jsonl",
    "cwd": cwd,
    "permission_mode": "default",
    "hook_event_name": name,
    "tool_name": tool,
    "tool_input": input,
    "tool_use_id": "toolu_1"
  })
  .to_string()
}

fn write_event(session: &str, cwd: &Path, file: &Path, content: &str) -> String {
  edit_event("PreToolUse", session, cwd, "Write", json!({"file_path": file, "content": content}))
}

fn context(output: &Output) -> String {
  let document: Value = serde_json::from_str(&stdout(output)).unwrap();
  assert_eq!(document["hookSpecificOutput"].get("permissionDecision"), None);
  document["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned()
}

#[test]
fn should_note_reuse_candidates_before_a_write_once_per_session_and_candidate() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();
  let badge = repo.join("src/ui/retry-badge.ts");

  let first = sandbox.piped(&repo, &["hook", "reuse"], &write_event("session-a", &repo, &badge, BACKOFF_COPY));
  let again = sandbox.piped(&repo, &["--log-level", "info", "hook", "reuse"], &write_event("session-a", &repo, &badge, BACKOFF_COPY));
  let other = sandbox.piped(&repo, &["hook", "reuse"], &write_event("session-b", &repo, &badge, BACKOFF_COPY));

  assert_ok(&first);
  let note = context(&first);
  assert_eq!(
    note,
    "Code Atlas: the code about to be written looks like existing code:
  [heuristic 1.00] `pauseAfterFailure` (new) looks like `waitBeforeRetry` — src/lib/backoff.ts:1-12 `function waitBeforeRetry(error: unknown, attempt: number): number`
  [heuristic 1.00] `pauseAfterFailure` (new) looks like `requeueDelay` — src/jobs/requeue.ts:1-12 `function requeueDelay(reason: unknown, tries: number): number`
Reuse or extend it where it fits."
  );
  assert!(note.chars().count() <= 600 && !note.contains("atlas_"));
  assert_eq!(serde_json::from_str::<Value>(&stdout(&first)).unwrap()["hookSpecificOutput"]["hookEventName"], "PreToolUse");
  assert_eq!((code(&again), stdout(&again)), (0, String::new()));
  assert!(stderr(&again).contains("reason=nothing to note"), "{}", stderr(&again));
  assert_eq!(context(&other), note);
}

#[test]
fn should_stay_silent_and_exit_0_whenever_it_cannot_note_reuse() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();
  let elsewhere = sandbox.path("work");
  let badge = repo.join("src/ui/retry-badge.ts");
  let silent: [(String, &str); 6] = [
    ("not json".to_owned(), "the hook input is not JSON"),
    (
      write_event("s", &repo, &badge, BACKOFF_COPY).replace("PreToolUse", "PostToolUse"),
      "no reuse note: not a PreToolUse event",
    ),
    (
      write_event("s", &repo, &repo.join("README.md"), BACKOFF_COPY),
      "reason=the file is not a Rust or TypeScript source file",
    ),
    (
      write_event("s", &repo, &repo.join("src/ui/retry-badge.test.ts"), BACKOFF_COPY),
      "or is a test or generated file",
    ),
    (write_event("s", &repo, &repo.join("src/ui/sum.ts"), SMALL), "reason=nothing to note"),
    (
      write_event("s", &elsewhere, &elsewhere.join("loose.ts"), BACKOFF_COPY),
      "reason=no project resolves for the working directory",
    ),
  ];
  for (index, (input, reason)) in silent.iter().enumerate() {
    let input = input.replace("\"s\"", &format!("\"quiet-{index}\""));
    let quiet = sandbox.piped(&repo, &["hook", "reuse"], &input);
    assert_eq!((code(&quiet), stdout(&quiet), stderr(&quiet)), (0, String::new(), String::new()), "{reason}");
    let logged = sandbox.piped(&repo, &["--log-level", "debug", "hook", "reuse"], &input.replace("quiet-", "logged-"));
    assert_eq!((code(&logged), stdout(&logged)), (0, String::new()), "{reason}");
    assert!(stderr(&logged).contains(reason), "{reason}:\n{}", stderr(&logged));
    assert!(!stderr(&logged).contains("retryAfter"), "code leaked into the log:\n{}", stderr(&logged));
  }
}

#[test]
fn should_answer_reuse_from_an_index_behind_on_other_files_but_not_on_the_edited_one() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();
  let client = repo.join("src/api/client.ts");
  let added = format!("\n{BACKOFF_COPY}");
  let edit = |session: &str| {
    let input = json!({"file_path": client, "old_string": "export interface Shelf {", "new_string": format!("{added}\nexport interface Shelf {{")});
    sandbox.piped(&repo, &["--log-level", "info", "hook", "reuse"], &edit_event("PreToolUse", session, &repo, "Edit", input))
  };

  fs::write(repo.join("src/ui/count-label.ts"), "export const LABEL = 'items';\n").unwrap();
  let behind_elsewhere = edit("session-c");
  fs::write(&client, fs::read_to_string(&client).unwrap().replace("attempt < 3", "attempt < 4")).unwrap();
  let behind_here = edit("session-d");

  assert_ok(&behind_elsewhere);
  assert!(
    context(&behind_elsewhere).contains("`pauseAfterFailure` (new) looks like `waitBeforeRetry`"),
    "{}",
    stdout(&behind_elsewhere)
  );
  assert_eq!((code(&behind_here), stdout(&behind_here)), (0, String::new()));
  assert!(
    stderr(&behind_here).contains("reason=the edited file changed since the last index run"),
    "{}",
    stderr(&behind_here)
  );
}

fn post_edit(session: &str, cwd: &Path, file: &Path) -> String {
  edit_event("PostToolUse", session, cwd, "Edit", json!({"file_path": file, "old_string": "a", "new_string": "b"}))
}

#[test]
fn should_note_the_files_that_may_also_change_once_per_session_and_file_leaving_out_edited_ones() {
  let sandbox = Sandbox::new();
  let repo = sandbox.shelf();
  let client = repo.join("src/api/client.ts");
  let store = repo.join("src/state/shelf-store.ts");
  fs::write(&client, fs::read_to_string(&client).unwrap().replace("attempt < 3", "attempt < 4")).unwrap();
  let hook = |input: String| sandbox.piped(&repo, &["--log-level", "info", "hook", "also-change"], &input);

  let first = hook(post_edit("session-e", &repo, &client));
  let again = hook(post_edit("session-e", &repo, &client));
  let store_first = hook(post_edit("session-f", &repo, &store));
  let then_client = hook(post_edit("session-f", &repo, &client));

  assert_ok(&first);
  let note = context(&first);
  assert_eq!(
    note,
    "Code Atlas: files close to shelf-app:src/api/client.ts in the code graph, which often change with it (leads to check, not certain):
  shelf-app:src/state/shelf-store.ts — calls changed function fetchShelf
  shelf-app:src/lib/backoff.ts — called by changed function fetchShelf
  shelf-app:src/ui/shelf-view.ts — via shelf-app:src/state/shelf-store.ts, 2 hops from a changed file"
  );
  assert!(note.chars().count() <= 600 && !note.contains("atlas_"));
  assert_eq!(
    serde_json::from_str::<Value>(&stdout(&first)).unwrap()["hookSpecificOutput"]["hookEventName"],
    "PostToolUse"
  );
  assert_eq!((code(&again), stdout(&again)), (0, String::new()));
  assert!(stderr(&again).contains("reason=this file was already noted in this session"), "{}", stderr(&again));
  assert_ok(&store_first);
  let lines: Vec<String> = context(&then_client).lines().skip(1).map(str::to_owned).collect();
  assert_eq!(
    lines,
    [
      "  shelf-app:src/lib/backoff.ts — called by changed function fetchShelf",
      "  shelf-app:src/ui/shelf-view.ts — via shelf-app:src/state/shelf-store.ts, 2 hops from a changed file"
    ]
  );
  assert_eq!(fs::read_dir(sandbox.path("atlas/data/hooks/also-change")).unwrap().count(), 6);
}

#[test]
fn should_record_an_edit_the_hook_could_not_answer_and_leave_that_file_out_later() {
  let sandbox = Sandbox::new();
  let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-query/tests/reuse-corpus/shelf-app");
  let repo = sandbox.path("work").join("shelf-app");
  copy_dir(&corpus, &repo);
  let root = repo.display().to_string();
  assert_ok(&sandbox.run(&repo, &["project", "create", "shelf", "--repo", &root]));
  let hook = |file: &str| sandbox.piped(&repo, &["--log-level", "info", "hook", "also-change"], &post_edit("session-g", &repo, &repo.join(file)));

  let unanswered = hook("src/state/shelf-store.ts");
  assert_ok(&sandbox.run(&repo, &["index"]));
  let answered = hook("src/api/client.ts");

  assert_eq!((code(&unanswered), stdout(&unanswered)), (0, String::new()));
  assert!(stderr(&unanswered).contains("reason=the project has never been indexed"), "{}", stderr(&unanswered));
  let lines: Vec<String> = context(&answered).lines().skip(1).map(str::to_owned).collect();
  assert_eq!(
    lines,
    [
      "  shelf-app:src/lib/backoff.ts — called by changed function fetchShelf",
      "  shelf-app:src/ui/shelf-view.ts — via shelf-app:src/state/shelf-store.ts, 2 hops from a changed file"
    ]
  );
}
