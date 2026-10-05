mod e2e_support;

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use e2e_support::{ACME_REPOS, Run, Sandbox, copy_dir, executable, fixtures};
use serde_json::{Value, json};

const FAKE: &str = env!("CARGO_BIN_EXE_atlas-cli-fake-lsp");
const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const USE: &str = "import { toCents } from './money';\nexport function total(): number {\n  return toCents('1') + toCents('2');\n}\n";

fn canned(method: &str, result: Value) -> Value {
  json!({"canned": {"method": method, "result": result}})
}

fn range(start: (u32, u32), end: (u32, u32)) -> Value {
  json!({"start": {"line": start.0, "character": start.1}, "end": {"line": end.0, "character": end.1}})
}

fn text_edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> Value {
  json!({"range": range(start, end), "newText": new_text})
}

fn ts_ready() -> Vec<Value> {
  vec![
    json!({"recv": {"method": "initialize"}}),
    json!({"reply": {"method": "initialize", "result": {
      "capabilities": {
        "textDocumentSync": 1,
        "referencesProvider": true,
        "definitionProvider": true,
        "implementationProvider": true,
        "documentSymbolProvider": true,
        "renameProvider": {"prepareProvider": true},
        "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}
      },
      "serverInfo": {"name": "typescript-language-server", "version": "5.1.3"}
    }}}),
    json!({"recv": {"method": "initialized"}}),
    json!({"notify": {"method": "$/typescriptVersion", "params": {"version": "5.9.3", "source": "bundled"}}}),
  ]
}

struct Demo {
  sandbox: Sandbox,
  root: PathBuf,
  scratch: PathBuf,
}

impl Demo {
  fn new() -> Self {
    let sandbox = Sandbox::new();
    let root = sandbox.work().join("web");
    for (path, content) in [
      ("package.json", "{\"name\": \"web\"}\n"),
      ("tsconfig.json", "{}\n"),
      ("src/money.ts", MONEY),
      ("src/use.ts", USE),
    ] {
      let target = root.join(path);
      fs::create_dir_all(target.parent().unwrap()).unwrap();
      fs::write(target, content).unwrap();
    }
    let scratch = sandbox.work().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let spec = format!("{}=web", root.display());
    sandbox.run(&sandbox.work(), &["project", "create", "demo", "--repo", &spec]).ok();
    Self { sandbox, root, scratch }
  }

  fn run(&self, args: &[&str]) -> Run {
    self.sandbox.run(&self.root, args)
  }

  fn uri(&self, path: &str) -> String {
    format!("file://{}", self.root.join(path).display())
  }

  fn serve(&self, steps: &[Value]) {
    let lines: Vec<String> = steps.iter().map(Value::to_string).collect();
    let script = self.scratch.join("ts.jsonl");
    fs::write(&script, lines.join("\n")).unwrap();
    let wrapper = self.scratch.join("ts-server.sh");
    let body = format!(
      "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 5.1.3; exit 0; fi\necho $$ > '{}'\nexec '{FAKE}' '{}'\n",
      self.scratch.join("ts.pid").display(),
      script.display()
    );
    executable(&wrapper, &body);
    assert!(std::process::Command::new(&wrapper).arg("--version").output().unwrap().status.success());
    let wrapper = wrapper.display().to_string();
    self.run(&["project", "config", "demo", "--lsp-ts-command", &wrapper, "--lsp-download", "off"]).ok();
  }

  fn serve_rename(&self, edits: Value) {
    let mut steps = ts_ready();
    steps.extend([
      canned("textDocument/diagnostic", json!({"kind": "full", "items": []})),
      canned("textDocument/documentSymbol", json!([])),
      canned("textDocument/references", json!([])),
      canned("textDocument/prepareRename", json!({"range": range((0, 16), (0, 23)), "placeholder": "toCents"})),
      canned("textDocument/rename", json!({"changes": edits})),
    ]);
    self.serve(&steps);
  }

  fn full_rename_edits(&self, new_name: &str) -> Value {
    json!({
      self.uri("src/money.ts"): [text_edit((0, 16), (0, 23), new_name)],
      self.uri("src/use.ts"): [text_edit((0, 9), (0, 16), new_name), text_edit((2, 9), (2, 16), new_name), text_edit((2, 24), (2, 31), new_name)],
    })
  }

  fn read(&self, path: &str) -> String {
    fs::read_to_string(self.root.join(path)).unwrap()
  }
}

fn id_of(plan: &Path) -> String {
  let plan: Value = serde_json::from_str(&fs::read_to_string(plan).unwrap()).unwrap();
  plan["id"].as_str().unwrap().to_owned()
}

#[test]
fn should_dry_run_a_rename_to_a_plan_file_and_apply_it_from_a_new_process() {
  let demo = Demo::new();
  demo.serve_rename(demo.full_rename_edits("cents"));
  let plan = demo.scratch.join("plan.json");
  let plan_arg = plan.display().to_string();

  let dry = demo.run(&["edit", "rename", "toCents", "cents", "--dry-run", "--plan-out", &plan_arg]).ok();

  assert!(dry.stdout.contains("rename toCents -> cents  dry run, plan p-"), "{dry}");
  assert!(dry.stdout.contains("would change 2 file(s), 4 edit(s):"), "{dry}");
  assert!(dry.stdout.contains("dry run, plan p-"), "{dry}");
  assert!(dry.stdout.contains(&format!("(plan written to {plan_arg}; valid until the files change)")), "{dry}");
  assert!(!dry.stdout.contains("apply within"), "{dry}");
  assert_eq!(dry.stderr, "");
  assert_eq!(demo.read("src/money.ts"), MONEY);
  let written: Value = serde_json::from_str(&fs::read_to_string(&plan).unwrap()).unwrap();
  assert_eq!(written["project"], "demo");
  assert_eq!(written["version"], 1);
  assert_eq!(written["operation"]["operation"], "rename");
  assert_eq!(written["files"].as_array().unwrap().len(), 2);
  assert_eq!(written["guard"]["kind"], "project");

  let applied = demo.run(&["edit", "apply", &plan_arg, "--json"]).ok();

  let outcome = applied.answer();
  assert_eq!(outcome["status"]["state"], "applied");
  assert_eq!(outcome["files"].as_array().unwrap().len(), 2);
  assert_eq!(demo.read("src/money.ts"), MONEY.replace("toCents", "cents"));
  assert_eq!(demo.read("src/use.ts"), USE.replace("toCents", "cents"));
  let found = demo.run(&["find", "cents", "--json"]).ok();
  assert_eq!(found.answer()["matches"]["total"], 1);
  demo.run(&["find", "toCents"]).expect(4);

  let again = demo.run(&["edit", "apply", &plan_arg]).expect(5);
  assert!(again.stdout.contains("REFUSED: rename toCents -> cents:"), "{again}");
}

#[test]
fn should_exit_5_and_write_nothing_when_a_plan_went_stale() {
  let demo = Demo::new();
  demo.serve_rename(demo.full_rename_edits("cents"));
  let plan = demo.scratch.join("plan.json");
  let plan_arg = plan.display().to_string();
  demo.run(&["edit", "rename", "toCents", "cents", "--dry-run", "--plan-out", &plan_arg]).ok();
  let external = "export function toCents(amount: string): number {\n  return 1;\n}\n";
  fs::write(demo.root.join("src/money.ts"), external).unwrap();

  let refused = demo.run(&["edit", "apply", &plan_arg]).expect(5);

  assert!(refused.stdout.contains("1 files changed since plan"), "{refused}");
  assert!(refused.stdout.contains("nothing was written"), "{refused}");
  assert_eq!(demo.read("src/money.ts"), external);
  assert_eq!(demo.read("src/use.ts"), USE);
}

#[test]
fn should_refuse_a_text_plan_whose_file_was_written_after_the_dry_run() {
  let demo = Demo::new();
  let plan = demo.scratch.join("text-plan.json");
  let plan_arg = plan.display().to_string();
  demo
    .run(&[
      "edit",
      "replace",
      "src/use.ts",
      "--pattern",
      "toCents('1')",
      "--text",
      "toCents('9')",
      "--dry-run",
      "--plan-out",
      &plan_arg,
    ])
    .ok();
  let external = USE.replace("total", "sum");
  fs::write(demo.root.join("src/use.ts"), &external).unwrap();

  let refused = demo.run(&["edit", "apply", &plan_arg]).expect(5);

  assert!(refused.stdout.contains("REFUSED:"), "{refused}");
  assert_eq!(demo.read("src/use.ts"), external);
}

#[test]
fn should_refuse_a_rename_that_leaves_graph_sites_with_exit_5_unless_allow_unverified() {
  let demo = Demo::new();
  demo.serve_rename(json!({
    demo.uri("src/money.ts"): [text_edit((0, 16), (0, 23), "cents")],
    demo.uri("src/use.ts"): [text_edit((0, 9), (0, 16), "cents")],
  }));

  let refused = demo.run(&["edit", "rename", "toCents", "cents"]).expect(5);

  assert!(
    refused
      .stdout
      .contains("REFUSED: rename toCents -> cents: 2 site(s) the index resolves to this symbol in the same repo were not renamed"),
    "{refused}"
  );
  assert_eq!(demo.read("src/money.ts"), MONEY);

  let allowed = demo.run(&["edit", "rename", "toCents", "cents", "--allow-unverified"]).ok();

  assert!(allowed.stdout.contains("rename toCents -> cents  applied"), "{allowed}");
  assert_eq!(demo.read("src/money.ts"), MONEY.replace("toCents", "cents"));
}

#[test]
fn should_refuse_replace_unless_exactly_one_match_or_all() {
  let demo = Demo::new();

  let many = demo.run(&["edit", "replace", "src/use.ts", "--pattern", "toCents", "--text", "cents"]).expect(5);

  assert!(
    many.stdout.contains("REFUSED: replace_content web:src/use.ts: 3 matches; pass all or narrow the pattern"),
    "{many}"
  );
  assert!(many.stdout.contains("nothing was written"), "{many}");
  assert_eq!(demo.read("src/use.ts"), USE);

  let none = demo.run(&["edit", "replace", "src/use.ts", "--pattern", "absent", "--text", "x"]).expect(5);
  assert!(none.stdout.contains("REFUSED: replace_content web:src/use.ts: no match in"), "{none}");
  let as_json = demo.run(&["edit", "replace", "src/use.ts", "--pattern", "absent", "--text", "x", "--json"]).expect(5);
  assert_eq!(as_json.answer()["status"]["state"], "refused");

  let all = demo.run(&["edit", "replace", "src/use.ts", "--pattern", "toCents", "--text", "cents", "--all"]).ok();
  assert!(all.stdout.contains("applied"), "{all}");
  assert_eq!(demo.read("src/use.ts"), USE.replace("toCents", "cents"));

  let one = demo
    .run(&["edit", "replace", "src/money.ts", "--pattern", "Math\\.round\\(", "--text", "Math.floor(", "--regex"])
    .ok();
  assert!(one.stdout.contains("applied"), "{one}");
  assert!(demo.read("src/money.ts").contains("Math.floor("));
}

#[test]
fn should_dry_run_replace_in_files_until_an_occurrence_selection_or_count_is_named() {
  let demo = Demo::new();
  let args = |extra: &[&'static str]| -> Vec<&'static str> {
    let mut args = vec!["edit", "replace-in-files", "--pattern", "toCents\\('(\\d)'\\)", "--text", "toCents('${1}0')", "--regex"];
    args.extend(extra);
    args
  };

  let implicit = demo.run(&args(&[])).ok();

  assert!(
    implicit
      .stdout
      .contains("2 matches; this call was a dry run because it named neither occurrences nor expected_count"),
    "{implicit}"
  );
  assert_eq!(demo.read("src/use.ts"), USE);

  let wrong = demo.run(&args(&["--expected-count", "3"])).expect(5);
  assert!(wrong.stdout.contains("REFUSED: replace_in_files"), "{wrong}");
  assert!(wrong.stdout.contains("expected 3 matches, found 2"), "{wrong}");
  assert_eq!(demo.read("src/use.ts"), USE);

  let applied = demo.run(&args(&["--expected-count", "2"])).ok();
  assert!(applied.stdout.contains("applied"), "{applied}");
  assert_eq!(demo.read("src/use.ts"), USE.replace("'1'", "'10'").replace("'2'", "'20'"));
}

#[test]
fn should_read_the_replacement_for_a_symbol_edit_from_stdin() {
  let demo = Demo::new();
  let replacement = "export function toCents(amount: string): number {\n  return 7;\n}\n";
  let mut command = demo.sandbox.command(&demo.root, &["edit", "replace-body", "toCents", "--text-file", "-"]);
  let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
  child.stdin.take().unwrap().write_all(replacement.as_bytes()).unwrap();

  let output = child.wait_with_output().unwrap();

  assert_eq!(
    output.status.code(),
    Some(0),
    "{}{}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr)
  );
  assert_eq!(demo.read("src/money.ts"), replacement);
}

#[test]
fn should_refuse_safe_delete_with_exit_5_while_references_remain() {
  let demo = Demo::new();
  let mut steps = ts_ready();
  steps.extend([
    canned("textDocument/references", json!([{"uri": demo.uri("src/use.ts"), "range": range((2, 9), (2, 16))}])),
    canned(
      "textDocument/documentSymbol",
      json!([{"name": "toCents", "kind": 12, "range": range((0, 0), (2, 1)), "selectionRange": range((0, 16), (0, 23))}]),
    ),
  ]);
  demo.serve(&steps);

  let refused = demo.run(&["edit", "safe-delete", "toCents"]).expect(5);

  assert!(
    refused
      .stdout
      .contains("REFUSED: safe_delete toCents: 3 reference(s) to toCents would break; nothing was deleted"),
    "{refused}"
  );
  assert_eq!(demo.read("src/money.ts"), MONEY);
}

#[test]
fn should_refuse_an_ambiguous_edit_target_with_exit_5() {
  let demo = Demo::new();
  fs::write(demo.root.join("src/a.ts"), "export function helper() {}\n").unwrap();
  fs::write(demo.root.join("src/b.ts"), "export function helper() {}\n").unwrap();

  let refused = demo.run(&["edit", "replace-body", "helper", "--text", "export function helper() { return 1; }"]).expect(5);

  assert!(
    refused
      .stdout
      .contains("\"helper\" matches 2 symbols; give a full key or [repo:]path:line:col; nothing was written"),
    "{refused}"
  );
}

#[test]
fn should_print_outline_and_references_with_the_documented_json_shape() {
  let demo = Demo::new();
  let mut steps = ts_ready();
  steps.extend([
    canned(
      "textDocument/documentSymbol",
      json!([{"name": "toCents", "kind": 12, "range": range((0, 0), (2, 1)), "selectionRange": range((0, 16), (0, 23))}]),
    ),
    canned("textDocument/references", json!([{"uri": demo.uri("src/use.ts"), "range": range((2, 9), (2, 16))}])),
  ]);
  demo.serve(&steps);

  let outline = demo.run(&["outline", "web:src/money.ts", "--json"]).ok();
  let refs = demo.run(&["refs", "toCents", "--json"]).ok();
  let text = demo.run(&["outline", "web:src/money.ts"]).ok();

  let answer = outline.answer();
  assert_eq!(answer["server"]["server"], "typescript-language-server");
  assert_eq!(answer["server"]["version"], "5.1.3");
  assert_eq!(answer["server"]["certainty"]["tag"], "[compiler]");
  assert_eq!(answer["symbols"][0]["name"], "toCents");
  assert_eq!(answer["symbols"][0]["kind"], "function");
  assert_eq!(answer["symbols"][0]["line"], 1);
  let answer = refs.answer();
  assert_eq!(answer["outcome"], "found");
  assert_eq!(answer["server"]["certainty"]["tag"], "[compiler]");
  assert_eq!(answer["groups"][0]["repo"], "web");
  assert_eq!(answer["groups"][0]["symbols"][0]["sites"][0]["place"]["path"], "src/use.ts");
  assert!(text.stdout.contains("(typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]"), "{text}");
}

#[test]
fn should_stop_the_language_server_and_exit_130_when_interrupted() {
  let demo = Demo::new();
  let mut steps = ts_ready()[..3].to_vec();
  steps.push(json!("hang"));
  demo.serve(&steps);
  let mut command = demo.sandbox.command(&demo.root, &["outline", "web:src/money.ts"]);
  let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
  let pid_file = demo.scratch.join("ts.pid");
  let deadline = Instant::now() + Duration::from_secs(30);
  while !pid_file.exists() && Instant::now() < deadline && child.try_wait().unwrap().is_none() {
    std::thread::sleep(Duration::from_millis(50));
  }
  if !pid_file.exists() {
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    panic!(
      "the language server never started: exit {:?}\n{}",
      output.status.code(),
      String::from_utf8_lossy(&output.stderr)
    );
  }
  let server = fs::read_to_string(&pid_file).unwrap().trim().to_owned();
  std::thread::sleep(Duration::from_millis(500));

  std::process::Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap();
  let output = child.wait_with_output().unwrap();

  assert_eq!(output.status.code(), Some(130), "{}", String::from_utf8_lossy(&output.stderr));
  let gone = Instant::now() + Duration::from_secs(10);
  let alive = || std::process::Command::new("kill").args(["-0", &server]).stderr(Stdio::null()).status().unwrap().success();
  while alive() && Instant::now() < gone {
    std::thread::sleep(Duration::from_millis(50));
  }
  assert!(!alive(), "the language server {server} outlived the interrupted command");
}

fn typescript_command() -> Option<(String, String)> {
  let cli = PathBuf::from(env::var_os("ATLAS_TEST_TS_LSP")?);
  let node = env::split_paths(&env::var_os("PATH")?).map(|dir| dir.join("node")).find(|candidate| candidate.is_file())?;
  Some((node.display().to_string(), cli.display().to_string()))
}

fn rust_analyzer_available() -> bool {
  std::process::Command::new("rust-analyzer")
    .arg("--version")
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .is_ok_and(|status| status.success())
}

fn real_acme(test: &str) -> Option<(Sandbox, PathBuf)> {
  let Some((node, cli)) = typescript_command() else {
    eprintln!("SKIPPED {test}: set ATLAS_TEST_TS_LSP to the typescript-language-server cli.mjs and put node on PATH");
    return None;
  };
  let sandbox = Sandbox::new();
  let root = sandbox.work().join("acme");
  for repo in ACME_REPOS {
    copy_dir(&fixtures().join("acme").join(repo), &root.join(repo));
  }
  let specs: Vec<String> = ACME_REPOS.iter().map(|repo| root.join(repo).display().to_string()).collect();
  let mut args = vec!["project", "create", "acme"];
  for spec in &specs {
    args.extend(["--repo", spec]);
  }
  sandbox.run(&root, &args).ok();
  sandbox
    .run(
      &root,
      &["project", "config", "acme", "--lsp-ts-command", &node, "--lsp-ts-command", &cli, "--lsp-download", "off"],
    )
    .ok();
  Some((sandbox, root))
}

#[test]
fn should_rename_to_cents_on_acme_through_the_real_typescript_server_from_a_plan_file() {
  let Some((sandbox, root)) = real_acme("should_rename_to_cents_on_acme_through_the_real_typescript_server_from_a_plan_file") else {
    return;
  };
  let api = root.join("acme-api");
  let plan = root.join("plan.json");
  let plan_arg = plan.display().to_string();

  let dry = sandbox
    .run(&api, &["edit", "rename", "acme-sdk:src/money.ts:1:17", "toCentsX", "--dry-run", "--plan-out", &plan_arg])
    .ok();

  assert!(dry.stdout.contains("would change 3 file(s), 7 edit(s):"), "{dry}");
  assert!(dry.stdout.contains("acme-web:src/components/OrderForm.tsx"), "{dry}");
  assert!(id_of(&plan).starts_with("p-"));
  assert!(fs::read_to_string(root.join("acme-sdk/src/money.ts")).unwrap().contains("export function toCents("));

  let applied = sandbox.run(&api, &["edit", "apply", &plan_arg]).ok();

  assert!(applied.stdout.contains("rename toCents -> toCentsX  applied"), "{applied}");
  assert!(fs::read_to_string(root.join("acme-sdk/src/money.ts")).unwrap().contains("export function toCentsX("));
  assert!(fs::read_to_string(root.join("acme-sdk/src/money.test.ts")).unwrap().contains("toCentsX('12.34')"));
  sandbox.run(&api, &["find", "toCentsX"]).ok();
}

#[test]
fn should_start_each_real_language_server_once_in_lsp_check() {
  let test = "should_start_each_real_language_server_once_in_lsp_check";
  let Some((sandbox, root)) = real_acme(test) else {
    return;
  };
  if !rust_analyzer_available() {
    eprintln!("SKIPPED {test}: rust-analyzer is not on PATH");
    return;
  }

  let api = root.join("acme-api");
  let checked = sandbox.run(&api, &["lsp", "check", "--json"]);

  assert!([0, 3].contains(&checked.code), "{checked}");
  let report = checked.json();
  let units = report["units"].as_array().unwrap();
  let servers: Vec<&str> = units.iter().map(|unit| unit["server"].as_str().unwrap()).collect();
  assert_eq!(servers, ["rust-analyzer", "typescript-language-server"], "{checked}");
  assert!(units.iter().all(|unit| unit["state"] == "ready" && unit["ready_ms"].as_u64().is_some()), "{checked}");
  assert_eq!(units[1]["typescript"], "5.9.3");
  assert_eq!(report["typescript"]["result"]["state"], "found");
  assert!(
    report["typescript"]["steps"]
      .as_array()
      .unwrap()
      .iter()
      .any(|step| step["label"] == "lsp.typescript_command" && step["status"] == "found"),
    "{checked}"
  );
  let text = sandbox.run(&api, &["lsp", "check"]);
  assert!([0, 3].contains(&text.code), "{text}");
  assert!(text.stdout.contains("ready in"), "{text}");
}

#[test]
fn should_report_readiness_and_versions_of_the_typescript_server_in_lsp_check() {
  let demo = Demo::new();
  demo.serve(&ts_ready());

  let checked = demo.run(&["lsp", "check", "--json"]).ok();

  let report = checked.json();
  assert_eq!(report["units"].as_array().unwrap().len(), 1, "{checked}");
  let unit = &report["units"][0];
  assert_eq!(
    (unit["server"].as_str(), unit["scope"].as_str(), unit["state"].as_str()),
    (Some("typescript-language-server"), Some("project"), Some("ready"))
  );
  assert_eq!((unit["server_version"].as_str(), unit["typescript"].as_str()), (Some("5.1.3"), Some("5.9.3")));
  assert!(unit["ready_ms"].as_u64().is_some());
  assert!(unit.get("health_error").is_none());
  let steps = report["typescript"]["steps"].as_array().unwrap();
  assert_eq!((steps[0]["label"].as_str(), steps[0]["status"].as_str()), (Some("lsp.typescript_command"), Some("found")));
  let text = demo.run(&["lsp", "check"]).ok();
  assert!(text.stdout.contains("ok    typescript-language-server (project): ready in "), "{text}");
  assert!(text.stdout.contains("version 5.1.3, TypeScript 5.9.3"), "{text}");
}

#[test]
fn should_exit_3_and_suggest_lsp_exclude_when_rust_analyzer_reports_an_unhealthy_workspace() {
  let sandbox = Sandbox::new();
  let root = sandbox.work().join("core");
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("Cargo.toml"), "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
  fs::write(root.join("src/lib.rs"), "pub fn answer() -> u32 {\n  42\n}\n").unwrap();
  let spec = format!("{}=core", root.display());
  sandbox.run(&sandbox.work(), &["project", "create", "demo", "--repo", &spec]).ok();
  let bin = sandbox.work().join("bin");
  fs::create_dir_all(&bin).unwrap();
  let mut steps = vec![
    json!({"recv": {"method": "initialize"}}),
    json!({"reply": {"method": "initialize", "result": {"capabilities": {"textDocumentSync": 1}, "serverInfo": {"name": "rust-analyzer", "version": "1.2.3"}}}}),
    json!({"recv": {"method": "initialized"}}),
  ];
  steps.push(json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "error", "quiescent": true, "message": "Failed to load workspaces: fixtures/broken/Cargo.toml"}}}));
  let script = bin.join("ra.jsonl");
  fs::write(&script, steps.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
  let wrapper = bin.join("rust-analyzer");
  executable(
    &wrapper,
    &format!(
      "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'rust-analyzer 1.2.3 (fake)'; exit 0; fi\nexec '{FAKE}' '{}'\n",
      script.display()
    ),
  );
  assert!(std::process::Command::new(&wrapper).arg("--version").output().unwrap().status.success());
  let path = bin.as_os_str();

  let checked = sandbox.run_with(&root, &["lsp", "check"], &[("PATH", path)]).expect(3);

  assert!(checked.stdout.contains("warn  rust-analyzer (project): ready in "), "{checked}");
  assert!(
    checked.stdout.contains("workspace health error: Failed to load workspaces: fixtures/broken/Cargo.toml"),
    "{checked}"
  );
  assert!(checked.stdout.contains("--lsp-exclude <glob>"), "{checked}");
  assert!(
    checked.stdout.contains("rust-analyzer: ") && checked.stdout.contains("rust-analyzer 1.2.3 (fake)"),
    "{checked}"
  );
  let json = sandbox.run_with(&root, &["lsp", "check", "--json"], &[("PATH", path)]).expect(3);
  assert_eq!(json.json()["units"][0]["health_error"], "Failed to load workspaces: fixtures/broken/Cargo.toml");
}

#[test]
fn should_note_in_the_dry_run_header_that_a_plan_was_not_saved() {
  let demo = Demo::new();

  let dry = demo.run(&["edit", "replace", "src/money.ts", "--pattern", "100", "--text", "1000", "--dry-run"]).ok();

  assert!(dry.stdout.contains("(plan not saved; pass --plan-out to apply later)"), "{dry}");
  assert!(!dry.stdout.contains("apply within"), "{dry}");
}

#[test]
fn should_refuse_plans_with_a_missing_a_different_or_a_foreign_version_or_project_with_exit_5() {
  let demo = Demo::new();
  let plan = demo.scratch.join("text-plan.json");
  let plan_arg = plan.display().to_string();
  demo
    .run(&[
      "edit",
      "replace",
      "src/money.ts",
      "--pattern",
      "100",
      "--text",
      "1000",
      "--dry-run",
      "--plan-out",
      &plan_arg,
    ])
    .ok();
  let original: Value = serde_json::from_str(&fs::read_to_string(&plan).unwrap()).unwrap();
  assert_eq!(original["version"], 1);
  let variant = |name: &str, change: &dyn Fn(&mut Value)| {
    let mut plan = original.clone();
    change(&mut plan);
    let path = demo.scratch.join(name);
    fs::write(&path, plan.to_string()).unwrap();
    path.display().to_string()
  };
  let missing = variant("missing.json", &|plan| {
    plan.as_object_mut().unwrap().remove("version");
  });
  let newer = variant("newer.json", &|plan| plan["version"] = json!(2));
  let foreign = variant("foreign.json", &|plan| plan["project"] = json!("other"));

  let refused_missing = demo.run(&["edit", "apply", &missing]).expect(5);
  let refused_newer = demo.run(&["edit", "apply", &newer]).expect(5);
  let refused_foreign = demo.run(&["edit", "apply", &foreign]).expect(5);

  assert!(
    refused_missing
      .stdout
      .contains("has no version; this code-atlas reads plan version 1; run the dry run again; nothing was written"),
    "{refused_missing}"
  );
  assert!(refused_newer.stdout.contains("has version 2; this code-atlas reads plan version 1"), "{refused_newer}");
  assert!(
    refused_foreign.stdout.contains("was made for project other, not demo; nothing was written"),
    "{refused_foreign}"
  );
  assert_eq!(demo.read("src/money.ts"), MONEY);
  demo.run(&["edit", "apply", &plan_arg]).ok();
  assert_eq!(demo.read("src/money.ts"), MONEY.replace("100", "1000"));
}
