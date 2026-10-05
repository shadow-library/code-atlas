mod e2e_support;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use e2e_support::{FakeClaude, Run, Sandbox, assert_same_answers, shorts, similarity_reply};
use serde_json::Value;

const LLM_ON: [&str; 7] = ["project", "config", "acme", "--llm", "on", "--llm-provider", "claude-cli"];

fn indexed_acme() -> (Sandbox, PathBuf) {
  let sandbox = Sandbox::new();
  let root = sandbox.acme();
  sandbox.run(&root.join("acme-api"), &["index"]).ok();
  (sandbox, root)
}

fn verdict_for(similar: &Value, short: &str) -> String {
  similar["members"]
    .as_array()
    .unwrap()
    .iter()
    .find(|member| member["entity"]["short"] == short)
    .and_then(|member| member["verdict"].as_str())
    .unwrap_or_else(|| panic!("{short} is not a similarity candidate: {similar:#}"))
    .to_owned()
}

fn empty_path(sandbox: &Sandbox) -> PathBuf {
  let dir = sandbox.work().join("empty-bin");
  fs::create_dir_all(&dir).unwrap();
  dir
}

#[test]
fn should_hide_stale_inferred_facts_by_default_and_flag_them_with_include_stale() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let claude = FakeClaude::replying(&sandbox.work(), &similarity_reply("similar_not_equivalent", 0.82, &["rounding differs on half cents"]));
  let path = claude.path();
  let with_fake = [("PATH", path.as_os_str())];
  sandbox.run(&api_repo, &LLM_ON).ok();

  let enriched = sandbox.run_with(&api_repo, &["enrich"], &with_fake).ok();
  let calls = claude.calls();
  assert_eq!(calls, 8, "{enriched}");
  assert!(enriched.stdout.contains("links: completed; 1 provider calls, 0 cache hits"), "{enriched}");
  assert!(enriched.stdout.contains("similarity (structural): completed; 3 provider calls, 0 cache hits"), "{enriched}");
  assert!(enriched.stdout.contains("similarity (lexical): completed; 4 provider calls, 0 cache hits"), "{enriched}");
  let status = sandbox.run(&api_repo, &["status", "--json"]).ok().answer();
  assert_eq!(
    (status["inferred_current"].as_u64(), status["inferred_stale"].as_u64()),
    (Some(calls as u64 - 1), Some(0)),
    "{status:#}"
  );

  let fresh_fact = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser", "--json"]).ok().answer();
  assert_eq!(
    verdict_for(&fresh_fact, "AcmeClient.getOrder"),
    "inferred similar, not equivalent (0.82): rounding differs on half cents"
  );
  let text = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser"]).ok();
  assert!(text.stdout.contains("inferred [inferred 0.82]"), "{text}");

  let client = root.join("acme-sdk/src/client.ts");
  let mut source = fs::read_to_string(&client).unwrap();
  source.push_str("\n// touched by the e2e suite\n");
  fs::write(&client, source).unwrap();

  let hidden = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser", "--json"]).ok().json();
  let stale = hidden["freshness"]["stale_inferred_facts"].as_u64().unwrap();
  assert!(stale >= 1, "{hidden:#}");
  assert!(
    !verdict_for(&hidden["answer"], "AcmeClient.getOrder").contains("inferred"),
    "a stale fact leaked: {hidden:#}"
  );
  let hidden_text = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser"]).ok();
  assert!(
    hidden_text.stdout.contains(&format!("{stale} stale inferred facts (hidden unless include_stale=true)")),
    "{hidden_text}"
  );
  assert!(hidden_text.stdout.contains("inferred facts hidden: 1 stale (set include_stale=true)"), "{hidden_text}");
  assert_eq!(sandbox.run(&api_repo, &["status", "--json"]).ok().answer()["inferred_stale"].as_u64(), Some(stale));

  let shown = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser", "--include-stale"]).ok();
  assert!(shown.stdout.contains("inferred [inferred 0.82 stale]"), "{shown}");
  assert!(
    shown
      .stdout
      .contains("verdict: inferred similar, not equivalent (0.82) [stale: acme-sdk:src/client.ts changed]: rounding differs on half cents"),
    "{shown}"
  );

  let unavailable = empty_path(&sandbox);
  let skipped = sandbox.run_with(&api_repo, &["enrich"], &[("PATH", unavailable.as_os_str())]).expect(3);
  assert!(skipped.stdout.contains("provider unavailable"), "{skipped}");
  assert_eq!(
    sandbox.run(&api_repo, &["status", "--json"]).ok().answer()["inferred_stale"].as_u64(),
    Some(stale),
    "an unavailable provider must leave stale facts stale"
  );

  let revalidated = sandbox.run_with(&api_repo, &["enrich"], &with_fake).ok();
  assert!(revalidated.stdout.contains("revalidation: "), "{revalidated}");
  let after = sandbox.run(&api_repo, &["status", "--json"]).ok().answer();
  assert_eq!(after["inferred_stale"], 0, "revalidation should replace the stale facts: {after:#}\n{revalidated}");
  let current = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser", "--json"]).ok().answer();
  assert_eq!(
    verdict_for(&current, "AcmeClient.getOrder"),
    "inferred similar, not equivalent (0.82): rounding differs on half cents"
  );
}

#[test]
fn should_reject_an_llm_equivalence_claim_that_lists_behavioural_differences() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let claude = FakeClaude::replying(&sandbox.work(), &similarity_reply("equivalent", 0.97, &["getOrder hits a different route"]));
  sandbox.run(&api_repo, &LLM_ON).ok();

  let enriched = sandbox.run_with(&api_repo, &["enrich"], &[("PATH", claude.path().as_os_str())]);

  enriched.assert_no_crash();
  assert!(claude.calls() > 0, "{enriched}");
  assert_eq!(sandbox.run(&api_repo, &["status", "--json"]).ok().answer()["inferred_current"], 0, "{enriched}");
  let similar = sandbox.run(&api_repo, &["query", "similar", "AcmeClient.getUser", "--include-stale"]).ok();
  assert!(!similar.stdout.contains("inferred equivalent"), "{similar}");
}

#[test]
fn should_skip_enrichment_when_the_llm_is_unavailable_and_leave_queries_untouched() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let queries: [&[&str]; 3] = [
    &["api", "--json"],
    &["query", "similar", "AcmeClient.getUser", "--json"],
    &["impact", "formatMoney", "--json"],
  ];
  let before: Vec<Value> = queries.iter().map(|query| sandbox.run(&api_repo, query).ok().answer()).collect();
  sandbox.run(&api_repo, &LLM_ON).ok();

  let enriched = sandbox.run_with(&api_repo, &["enrich"], &[("PATH", empty_path(&sandbox).as_os_str())]).expect(3);

  assert!(enriched.stdout.contains("similarity (structural): provider unavailable (cannot run `claude`"), "{enriched}");
  assert!(enriched.stdout.contains("links: provider unavailable (cannot run `claude`"), "{enriched}");
  assert!(enriched.stdout.contains("0 provider calls"), "{enriched}");
  let after: Vec<Value> = queries.iter().map(|query| sandbox.run(&api_repo, query).ok().answer()).collect();
  assert_eq!(before, after);
  let status = sandbox.run(&api_repo, &["status", "--json"]).ok().answer();
  assert_eq!((status["inferred_current"].as_u64(), status["llm_calls"].as_u64()), (Some(0), Some(0)), "{status:#}");
}

#[test]
fn should_record_skipped_semantic_tasks_in_the_enrich_run() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  sandbox.run(&api_repo, &LLM_ON).ok();
  let enriched = sandbox.run_with(&api_repo, &["enrich"], &[("PATH", empty_path(&sandbox).as_os_str())]).expect(3);
  let run_id = enriched.stdout.split_whitespace().nth(2).unwrap().trim_end_matches(':').to_owned();

  let recorded = sandbox.run(&api_repo, &["debug", "run", &run_id, "--json"]).ok().json();

  let text = recorded.to_string();
  assert!(
    recorded["status"] != "ok" || text.contains("skipped"),
    "the run ledger does not say the semantic tasks were skipped: {recorded:#}"
  );
}

struct Hostile {
  sandbox: Sandbox,
  repo: PathBuf,
  unreadable: bool,
}

fn hostile_repo() -> Hostile {
  let sandbox = Sandbox::new();
  let repo = sandbox.work().join("hostile");
  let src = repo.join("src");
  fs::create_dir_all(src.join("loop")).unwrap();
  fs::write(
    src.join("main.ts"),
    "import { helper } from './helper';\n\nexport function entry(): number {\n  return helper(2);\n}\n",
  )
  .unwrap();
  fs::write(src.join("helper.ts"), "export function helper(value: number): number {\n  return value * 2;\n}\n").unwrap();
  fs::write(src.join("binary.ts"), b"export function broken(): string {\n  return \"\xff\xfe\xc3\x28\";\n}\n").unwrap();
  let huge: String = (0..120_000).map(|index| format!("export function big{index}(): number {{ return {index}; }}\n")).collect();
  assert!(huge.len() > 5_000_000);
  fs::write(src.join("huge.ts"), huge).unwrap();
  symlink("..", src.join("loop/self")).unwrap();
  symlink("b.ts", src.join("a.ts")).unwrap();
  symlink("a.ts", src.join("b.ts")).unwrap();
  let secret = src.join("secret.ts");
  fs::write(&secret, "export function hidden(): number {\n  return 1;\n}\n").unwrap();
  fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
  let unreadable = fs::read(&secret).is_err();
  sandbox.create_project("hostile", std::slice::from_ref(&repo));
  Hostile { sandbox, repo, unreadable }
}

#[test]
fn should_index_a_hostile_repo_partially_with_an_accurate_status() {
  let Hostile { sandbox, repo, unreadable } = hostile_repo();

  let indexed = sandbox.run(&repo, &["index"]).expect(3);

  let failed: Vec<&str> = indexed.stdout.lines().filter(|line| line.trim_start().starts_with("failed ")).collect();
  let mut expected = vec!["failed hostile:src/binary.ts: source is not valid UTF-8"];
  if unreadable {
    expected.insert(0, "failed hostile:src/secret.ts: unreadable: Permission denied (os error 13)");
  }
  assert_eq!(failed.iter().map(|line| line.trim()).collect::<Vec<_>>(), expected, "{indexed}");
  assert!(
    indexed.stdout.contains(&format!("outcome: partial ({} file(s) or repo(s) failed)", expected.len())),
    "{indexed}"
  );

  let status = sandbox.run(&repo, &["status", "--json"]).expect(if unreadable { 3 } else { 0 });
  let answer = status.answer();
  assert_eq!(
    answer["files_by_parse_status"],
    serde_json::json!({ "failed": 1, "ok": 2 + usize::from(!unreadable) }),
    "{status}"
  );
  let callers = sandbox.run(&repo, &["callers", "helper"]);
  assert!([0, 3].contains(&callers.code), "{callers}");
  assert!(callers.stdout.contains("entry function hostile:src/main.ts:3  calls helper [resolved]"), "{callers}");
  assert!(callers.stdout.contains("hostile:src/binary.ts failed: source is not valid UTF-8"), "{callers}");
  for skipped in ["big1", "broken"] {
    sandbox.run(&repo, &["query", "symbol", skipped]).expect(4);
  }
  for run in [&indexed, &status, &callers] {
    run.assert_no_crash();
  }
}

#[test]
fn should_answer_a_query_when_a_file_parse_outlasts_its_budget() {
  let sandbox = Sandbox::new();
  let repo = sandbox.work().join("stalling");
  let src = repo.join("src");
  fs::create_dir_all(&src).unwrap();
  fs::write(repo.join("Cargo.toml"), "[package]\nname = \"stalling\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
  fs::write(
    src.join("lib.rs"),
    "mod hang;\n\npub fn helper() -> u32 {\n  1\n}\n\npub fn entry() -> u32 {\n  helper()\n}\n",
  )
  .unwrap();
  fs::copy(
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../atlas-lang-rust/tests/hangs/error-recovery-1.rs.txt"),
    src.join("hang.rs"),
  )
  .unwrap();
  sandbox.create_project("stalling", std::slice::from_ref(&repo));

  let callers = sandbox.run(&repo, &["callers", "helper"]).expect(3);
  let again = sandbox.run(&repo, &["callers", "helper"]).ok();

  let failure = "stalling:src/hang.rs: parse budget exceeded (2003 ms, 352 bytes)";
  assert!(callers.stdout.contains(&format!("affected {failure}")), "{callers}");
  assert!(callers.stdout.contains("entry function stalling:src/lib.rs:7  calls helper [resolved]"), "{callers}");
  let header = again.stdout.lines().next().unwrap_or_default();
  assert!(header.starts_with("fresh run=1 (+0 files, ") && header.ends_with(" | 1 files failed analysis"), "{again}");
  assert!(again.stdout.contains("stalling:src/hang.rs failed: parse budget exceeded (2003 ms, 352 bytes)"), "{again}");
  assert!(again.stderr.is_empty(), "the unchanged file was analysed again: {again}");
  let status = sandbox.run(&repo, &["status"]).ok();
  assert!(
    status
      .stdout
      .contains("failed stalling:src/hang.rs parse budget exceeded (2003 ms, 352 bytes) (budget retry 2 in "),
    "{status}"
  );
}

#[test]
fn should_report_an_oversized_file_as_skipped() {
  let Hostile { sandbox, repo, .. } = hostile_repo();

  let indexed = sandbox.run(&repo, &["index"]);
  let status = sandbox.run(&repo, &["status"]);

  assert!(
    indexed.stdout.contains("src/huge.ts") || status.stdout.contains("src/huge.ts"),
    "the oversized file is invisible:\n{indexed}\n{status}"
  );
}

#[test]
fn should_render_the_scip_state_in_status_without_debug_formatting() {
  let (sandbox, root) = indexed_acme();

  let status = sandbox.run(&root.join("acme-api"), &["status"]).ok();

  let line = status.stdout.lines().find(|line| line.starts_with("scip")).unwrap();
  assert!(!line.contains("Some(") && !line.contains("NeverRun"), "{line}");
}

#[test]
fn should_survive_files_vanishing_while_indexing() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let churn = api_repo.join("src/churn");
  fs::create_dir_all(&churn).unwrap();
  let done = Arc::new(AtomicBool::new(false));
  let churner = {
    let (done, churn) = (Arc::clone(&done), churn.clone());
    thread::spawn(move || {
      let mut round = 0usize;
      while !done.load(Ordering::SeqCst) {
        let file = churn.join(format!("f{}.ts", round % 40));
        let _ = fs::write(&file, format!("export function churn{}(): number {{\n  return {round};\n}}\n", round % 40));
        let _ = fs::remove_file(churn.join(format!("f{}.ts", (round + 20) % 40)));
        round += 1;
      }
    })
  };

  let runs: Vec<Run> = (0..4).map(|_| sandbox.run(&api_repo, &["index", "--full"])).collect();
  done.store(true, Ordering::SeqCst);
  churner.join().unwrap();
  fs::remove_dir_all(&churn).unwrap();

  for run in &runs {
    run.assert_no_crash();
    assert!([0, 3].contains(&run.code), "{run}");
    assert!(!run.stderr.contains("lock"), "{run}");
  }
  let settled = sandbox.run(&api_repo, &["status", "--json"]).ok().answer();
  assert_eq!(
    settled["failed_files"],
    serde_json::json!([]),
    "files that no longer exist still degrade the index: {settled:#}"
  );
  sandbox.run(&api_repo, &["query", "symbol", "churn0"]).expect(4);
  assert_same_answers(&sandbox, "acme", &[&["api"], &["query", "overview"]], "after files vanished mid-index");
}

#[test]
fn should_report_a_deleted_repo_without_crashing() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  fs::remove_dir_all(root.join("acme-billing")).unwrap();

  let overview = sandbox.run(&api_repo, &["query", "overview", "--json"]).expect(3);
  let indexed = sandbox.run(&api_repo, &["index"]).expect(3);
  let status = sandbox.run(&api_repo, &["status"]).expect(3);
  let doctor = sandbox.run(&api_repo, &["doctor"]);

  assert_eq!(overview.json()["freshness"]["state"], "degraded");
  assert!(overview.stdout.contains("acme-billing"), "{overview}");
  assert!(indexed.stdout.contains("outcome: partial"), "{indexed}");
  assert!(indexed.stdout.lines().any(|line| line.trim_start().starts_with("failed acme-billing")), "{indexed}");
  assert!(status.stdout.starts_with("DEGRADED"), "{status}");
  assert!(doctor.stdout.contains("warn  acme/acme-billing:") && doctor.stdout.contains("is missing"), "{doctor}");
  let gone = sandbox.run(&api_repo, &["query", "symbol", "create_invoice"]);
  assert_eq!(gone.code, 4, "facts of an unavailable repo must not be served as current: {gone}");
  for run in [&overview, &indexed, &status, &doctor, &gone] {
    run.assert_no_crash();
  }
}

#[test]
fn should_fail_clearly_on_a_truncated_database_and_let_doctor_detect_it() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let db = sandbox.db("acme");
  let size = fs::metadata(&db).unwrap().len();
  fs::OpenOptions::new().write(true).open(&db).unwrap().set_len(size / 2).unwrap();
  for suffix in ["-wal", "-shm"] {
    let _ = fs::remove_file(PathBuf::from(format!("{}{suffix}", db.display())));
  }

  let query = sandbox.run(&api_repo, &["query", "overview"]).expect(1);
  let doctor = sandbox.run(&api_repo, &["doctor"]).expect(1);

  assert_eq!(query.stdout, "");
  assert!(query.stderr.starts_with("error: ") && query.stderr.contains("malformed"), "{query}");
  assert!(doctor.stdout.contains("FAIL  acme database: database disk image is malformed"), "{doctor}");
  assert!(doctor.stdout.contains("1 failures"), "{doctor}");
  for run in [&query, &doctor] {
    run.assert_no_crash();
  }
}

#[test]
fn should_report_unavailable_scip_indexers_when_nothing_is_on_the_path() {
  let (sandbox, root) = indexed_acme();
  let billing = root.join("acme-billing");
  let path = empty_path(&sandbox);
  let no_tools = [("PATH", path.as_os_str())];

  let scip = sandbox.run_with(&billing, &["scip"], &no_tools).expect(3);
  let doctor = sandbox.run_with(&billing, &["doctor"], &no_tools);

  assert!(
    scip
      .stdout
      .contains("acme-billing:Cargo.toml [rust-analyzer] indexer unavailable: rust-analyzer is not on PATH and `rustup which rust-analyzer` found nothing"),
    "{scip}"
  );
  for repo in ["acme-api", "acme-sdk", "acme-web"] {
    assert!(
      scip
        .stdout
        .contains(&format!("{repo}:package.json [scip-typescript] indexer unavailable: npx is not usable")),
      "{scip}"
    );
  }
  assert!(doctor.stdout.contains("warn  rust-analyzer: rust-analyzer is not on PATH"), "{doctor}");
  let query = sandbox.run_with(&billing, &["query", "callers", "to_cents", "--json"], &no_tools).ok().json();
  assert_eq!(query["freshness"]["scip"]["state"], "never_run", "{query:#}");
  assert!(shorts(&query["answer"]["groups"]).contains(&"consume_order_created".to_owned()), "{query:#}");
}

fn churn_and_query(sandbox: &Sandbox, repo: &Path, file: &Path, prefix: &str, rounds: usize) -> Vec<Run> {
  let mut runs = Vec::new();
  for round in 0..rounds {
    let mut source = fs::read_to_string(file).unwrap();
    source.push_str(&format!("\nexport function {prefix}{round}(): number {{\n  return {round};\n}}\n"));
    fs::write(file, source).unwrap();
    runs.push(sandbox.run(repo, &["query", "symbol", &format!("{prefix}{round}")]));
  }
  runs
}

#[test]
fn should_let_two_cli_processes_edit_and_query_concurrently_without_lock_errors() {
  let (sandbox, root) = indexed_acme();
  let sandbox = Arc::new(sandbox);
  let api_repo = root.join("acme-api");
  let web_repo = root.join("acme-web");
  let writers: Vec<_> = [
    (api_repo.clone(), api_repo.join("src/common/slug.ts"), "apiHelper"),
    (web_repo.clone(), web_repo.join("src/utils/currency.ts"), "webHelper"),
  ]
  .into_iter()
  .map(|(repo, file, prefix)| {
    let sandbox = Arc::clone(&sandbox);
    thread::spawn(move || churn_and_query(&sandbox, &repo, &file, prefix, 6))
  })
  .collect();
  let indexer = {
    let (sandbox, repo) = (Arc::clone(&sandbox), api_repo.clone());
    thread::spawn(move || (0..3).map(|_| sandbox.run(&repo, &["index"])).collect::<Vec<_>>())
  };

  let runs: Vec<Run> = writers.into_iter().flat_map(|writer| writer.join().unwrap()).collect();
  let index_runs = indexer.join().unwrap();

  for run in runs.iter().chain(&index_runs) {
    run.assert_no_crash();
    assert!(!run.stderr.to_lowercase().contains("lock"), "lock error instead of a wait: {run}");
  }
  for run in &runs {
    assert_eq!(run.code, 0, "an edit made before the query must be visible to it: {run}");
  }
  for run in &index_runs {
    assert_eq!(run.code, 0, "{run}");
  }
  assert_same_answers(
    &sandbox,
    "acme",
    &[&["api"], &["query", "overview"], &["search", "apiHelper"], &["search", "webHelper"]],
    "after concurrent edits",
  );
}

#[test]
fn should_list_enrichment_candidates_in_a_dry_run_without_calling_the_llm() {
  let (sandbox, root) = indexed_acme();
  let api_repo = root.join("acme-api");
  let claude = FakeClaude::replying(&sandbox.work(), &similarity_reply("equivalent", 0.9, &[]));
  sandbox.run(&api_repo, &LLM_ON).ok();

  let planned = sandbox
    .run_with(&api_repo, &["enrich", "--dry-run", "--summaries"], &[("PATH", claude.path().as_os_str())])
    .ok();

  assert_eq!(claude.calls(), 0, "{planned}");
  assert!(planned.stdout.contains("links (1 unmatched calls, 0 without a same-method endpoint)"), "{planned}");
  assert!(
    planned.stdout.contains("GET {}/health from checkHealth: GET /orders/{id} 0.250, GET /users/{id} 0.250"),
    "{planned}"
  );
  assert!(planned.stdout.contains("similarity (lexical) (4)"), "{planned}");
  assert!(
    planned
      .stdout
      .contains("<> ts:acme-sdk:@acme/sdk:src/money::toCents#function (bm25 0.556, related name concepts cent"),
    "{planned}"
  );
  assert!(planned.stdout.contains("summaries ("), "{planned}");
  assert_eq!(sandbox.run(&api_repo, &["status", "--json"]).ok().answer()["llm_calls"], 0);
}
