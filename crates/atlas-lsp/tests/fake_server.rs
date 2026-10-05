mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use atlas_lsp::protocol::{ClientCapabilities, Health, InitializeParams, NumberOrString, ServerStatusParams, WorkDoneProgress, WorkDoneProgressBegin};
use atlas_lsp::{Client, ClientHandler, ClientOptions, Disconnect, LspError, NoopHandler, PositionEncoding, ProcessExit, ServerCommand, ServerEvent, ShutdownOutcome};
use common::Recorder;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

struct Scenario {
  dir: tempfile::TempDir,
}

impl Scenario {
  fn new(steps: &[Value]) -> Self {
    let scenario = Self::empty();
    scenario.write(steps);
    scenario
  }

  fn empty() -> Self {
    Self {
      dir: tempfile::tempdir().unwrap(),
    }
  }

  fn write(&self, steps: &[Value]) {
    let lines: Vec<String> = steps.iter().map(Value::to_string).collect();
    fs::write(self.path("script.jsonl"), lines.join("\n")).unwrap();
  }

  fn path(&self, name: &str) -> PathBuf {
    self.dir.path().join(name)
  }

  fn command(&self) -> ServerCommand {
    ServerCommand::new(env!("CARGO_BIN_EXE_atlas-fake-lsp")).arg(self.path("script.jsonl"))
  }

  fn spawn(&self, handler: Arc<dyn ClientHandler>, options: ClientOptions) -> Client {
    Client::spawn(&self.command(), handler, options).unwrap()
  }
}

fn initialize_steps(encoding: Option<&str>) -> Vec<Value> {
  let capabilities = match encoding {
    Some(encoding) => json!({"positionEncoding": encoding, "renameProvider": {"prepareProvider": true}}),
    None => json!({"renameProvider": true}),
  };
  vec![
    json!({"recv": {"method": "initialize", "params": {"processId": std::process::id(), "capabilities": {"general": {"positionEncodings": ["utf-8", "utf-16"]}}}}}),
    json!({"reply": {"method": "initialize", "result": {"capabilities": capabilities, "serverInfo": {"name": "fake", "version": "0.1"}}}}),
    json!({"recv": {"method": "initialized"}}),
  ]
}

fn initialize_params() -> InitializeParams {
  InitializeParams {
    process_id: Some(std::process::id()),
    capabilities: ClientCapabilities::standard(vec![PositionEncoding::Utf8, PositionEncoding::Utf16]),
    ..InitializeParams::default()
  }
}

fn read_pid(path: &Path) -> u32 {
  let deadline = Instant::now() + WAIT;
  loop {
    if let Ok(text) = fs::read_to_string(path)
      && let Ok(pid) = text.trim().parse()
    {
      return pid;
    }
    assert!(Instant::now() < deadline, "no pid in {}", path.display());
    thread::sleep(Duration::from_millis(10));
  }
}

fn alive(pid: u32) -> bool {
  Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
}

fn wait_dead(pid: u32) -> bool {
  let deadline = Instant::now() + WAIT;
  while alive(pid) {
    if Instant::now() > deadline {
      return false;
    }
    thread::sleep(Duration::from_millis(20));
  }
  true
}

fn concurrent(client: &Arc<Client>, methods: &[&'static str]) -> Vec<thread::JoinHandle<Result<Value, LspError>>> {
  methods
    .iter()
    .map(|method| {
      let client = Arc::clone(client);
      let method = *method;
      thread::spawn(move || client.request::<Value>(method, json!({})))
    })
    .collect()
}

#[test]
fn should_negotiate_every_position_encoding_with_the_fake_server() {
  let cases = [
    (None, PositionEncoding::Utf16),
    (Some("utf-8"), PositionEncoding::Utf8),
    (Some("utf-16"), PositionEncoding::Utf16),
    (Some("utf-32"), PositionEncoding::Utf32),
  ];
  for (chosen, expected) in cases {
    let scenario = Scenario::new(&initialize_steps(chosen));
    let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

    let initialized = client.initialize(&initialize_params()).unwrap();

    assert_eq!(initialized.encoding, expected, "{chosen:?}");
    assert_eq!(client.position_encoding(), Some(expected));
    assert_eq!(initialized.result.server_info.map(|info| info.name).as_deref(), Some("fake"));
    assert_eq!(initialized.result.capabilities.supports_prepare_rename(), chosen.is_some());
    assert_eq!(client.shutdown(), ShutdownOutcome::Exited(ProcessExit::Code(0)), "{chosen:?}");
  }
}

#[test]
fn should_refuse_an_unknown_position_encoding() {
  let scenario = Scenario::new(&initialize_steps(Some("utf-7")));
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

  let error = client.initialize(&initialize_params()).unwrap_err();

  assert!(matches!(error, LspError::UnsupportedEncoding { ref encoding } if encoding == "utf-7"), "{error:?}");
  assert_eq!(client.position_encoding(), None);
}

#[test]
fn should_correlate_concurrent_requests_answered_out_of_order() {
  let scenario = Scenario::new(&[
    json!({"recv_all": ["a", "b", "c"]}),
    json!({"reply": {"method": "c", "result": "for c"}}),
    json!({"sleep_ms": 20}),
    json!({"reply": {"method": "a", "result": "for a"}}),
    json!({"reply": {"method": "b", "error": {"code": -32801, "message": "content modified"}}}),
  ]);
  let client = Arc::new(scenario.spawn(Arc::new(NoopHandler), ClientOptions::default()));

  let results: Vec<_> = concurrent(&client, &["a", "b", "c"]).into_iter().map(|caller| caller.join().unwrap()).collect();

  assert_eq!(results[0].as_ref().unwrap(), &json!("for a"));
  assert!(
    matches!(&results[1], Err(LspError::Response { method, error }) if method == "b" && error.code == -32801),
    "{:?}",
    results[1]
  );
  assert_eq!(results[2].as_ref().unwrap(), &json!("for c"));
}

#[test]
fn should_time_out_a_request_and_send_cancel_request() {
  let scenario = Scenario::new(&[
    json!({"recv": {"method": "slow"}}),
    json!({"recv": {"method": "$/cancelRequest", "params": {"id": 1}}}),
    json!({"reply": {"method": "slow", "error": {"code": -32800, "message": "cancelled"}}}),
    json!({"recv": {"method": "ping"}}),
    json!({"reply": {"method": "ping", "result": "pong"}}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());
  let started = Instant::now();

  let error = client.request_with_timeout::<Value>("slow", json!({}), Duration::from_millis(200)).unwrap_err();

  assert!(started.elapsed() < WAIT);
  assert_eq!(error.to_string(), "slow timed out after 0.2s");
  assert!(matches!(error, LspError::Timeout { ref method, after } if method == "slow" && after == Duration::from_millis(200)));
  assert_eq!(client.request::<String>("ping", json!({})).unwrap(), "pong");
}

#[test]
fn should_answer_server_initiated_requests() {
  struct Settings;
  impl ClientHandler for Settings {
    fn configuration(&self, item: &atlas_lsp::protocol::ConfigurationItem) -> Value {
      json!({"section": item.section})
    }
  }
  let recorder = Recorder::new();
  struct Both(Arc<Recorder>);
  impl ClientHandler for Both {
    fn configuration(&self, item: &atlas_lsp::protocol::ConfigurationItem) -> Value {
      Settings.configuration(item)
    }
    fn on_event(&self, event: ServerEvent) {
      self.0.on_event(event);
    }
  }
  let scenario = Scenario::new(&[
    json!({"request": {"id": "cfg", "method": "workspace/configuration", "params": {"items": [{"section": "rust-analyzer"}, {}]}}}),
    json!("recv_reply"),
    json!({"request": {"id": 7, "method": "workspace/applyEdit", "params": {"edit": {"changes": {}}}}}),
    json!("recv_reply"),
    json!({"request": {"id": 8, "method": "client/registerCapability", "params": {"registrations": []}}}),
    json!("recv_reply"),
    json!({"request": {"id": 9, "method": "custom/unknown"}}),
    json!("recv_reply"),
  ]);
  let _client = scenario.spawn(Arc::new(Both(recorder.clone())), ClientOptions::default());

  let replies = recorder.notifications(WAIT, "fake/reply", 4);

  assert_eq!(
    replies,
    vec![
      json!({"id": "cfg", "result": [{"section": "rust-analyzer"}, {"section": null}]}),
      json!({"id": 7, "result": {"applied": false, "failureReason": "code-atlas applies edits itself; language servers never write"}}),
      json!({"id": 8, "result": null}),
      json!({"id": 9, "error": {"code": -32601, "message": "code-atlas does not handle custom/unknown"}}),
    ]
  );
}

#[test]
fn should_capture_progress_status_and_diagnostics_events_in_order() {
  let recorder = Recorder::new();
  let scenario = Scenario::new(&[
    json!({"request": {"id": 1, "method": "window/workDoneProgress/create", "params": {"token": "rustAnalyzer/Indexing"}}}),
    json!("recv_reply"),
    json!({"notify": {"method": "$/progress", "params": {"token": "rustAnalyzer/Indexing", "value": {"kind": "begin", "title": "Indexing", "percentage": 0}}}}),
    json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": false}}}),
    json!({"notify": {"method": "window/logMessage", "params": {"type": 4, "message": "noise"}}}),
    json!({"notify": {"method": "$/progress", "params": {"token": "rustAnalyzer/Indexing", "value": {"kind": "end"}}}}),
    json!({"notify": {"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": true}}}),
    json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {"uri": "file:///a.rs", "version": 2, "diagnostics": []}}}),
    json!({"notify": {"method": "$/typescriptVersion", "params": {"version": "5.9.3"}}}),
  ]);
  let _client = scenario.spawn(recorder.clone(), ClientOptions::default());

  let events = recorder.wait_for(WAIT, |events| events.len() >= 8);

  let indexing = NumberOrString::String("rustAnalyzer/Indexing".to_owned());
  assert!(matches!(&events[0], ServerEvent::ProgressCreated(token) if *token == indexing), "{events:?}");
  let ServerEvent::Notification { method, .. } = &events[1] else {
    panic!("expected the echoed reply, got {:?}", events[1]);
  };
  assert_eq!(method, "fake/reply");
  let ServerEvent::Progress(begin) = &events[2] else {
    panic!("expected progress, got {:?}", events[2]);
  };
  assert_eq!(
    begin.work_done(),
    Some(WorkDoneProgress::Begin(WorkDoneProgressBegin {
      title: "Indexing".to_owned(),
      percentage: Some(0),
      ..WorkDoneProgressBegin::default()
    }))
  );
  let statuses: Vec<_> = events
    .iter()
    .filter_map(|event| match event {
      ServerEvent::ServerStatus(status) => Some(status.clone()),
      _ => None,
    })
    .collect();
  assert_eq!(
    statuses,
    vec![
      ServerStatusParams {
        health: Health::Ok,
        quiescent: false,
        message: None
      },
      ServerStatusParams {
        health: Health::Ok,
        quiescent: true,
        message: None
      },
    ]
  );
  assert!(matches!(&events[3], ServerEvent::ServerStatus(_)));
  assert!(matches!(&events[4], ServerEvent::Progress(end) if matches!(end.work_done(), Some(WorkDoneProgress::End(_)))));
  assert!(matches!(&events[6], ServerEvent::Diagnostics(params) if params.uri == "file:///a.rs" && params.version == Some(2)));
  assert!(matches!(&events[7], ServerEvent::Notification { method, params } if method == "$/typescriptVersion" && params["version"] == "5.9.3"));
}

#[test]
fn should_fail_pending_requests_when_the_server_exits() {
  let scenario = Scenario::new(&[json!({"recv_all": ["a", "b"]}), json!({"stderr": "fatal: index corrupted\n"}), json!({"exit": 3})]);
  let client = Arc::new(scenario.spawn(Arc::new(NoopHandler), ClientOptions::default()));

  let results: Vec<_> = concurrent(&client, &["a", "b"]).into_iter().map(|caller| caller.join().unwrap()).collect();

  for result in results {
    let error = result.unwrap_err();
    assert!(
      matches!(&error, LspError::ServerExited { cause: Disconnect::ProcessExited(ProcessExit::Code(3)), stderr_tail } if stderr_tail == "fatal: index corrupted\n"),
      "{error:?}"
    );
  }
  let after = client.request::<Value>("c", json!({})).unwrap_err();
  assert!(
    matches!(
      after,
      LspError::ServerExited {
        cause: Disconnect::ProcessExited(ProcessExit::Code(3)),
        ..
      }
    ),
    "{after:?}"
  );
}

#[test]
fn should_report_the_stderr_tail_of_a_crashed_server() {
  let noise = "x".repeat(20_000);
  let scenario = Scenario::new(&[
    json!({"recv": {"method": "initialize"}}),
    json!({"stderr": noise}),
    json!({"stderr": "\nthread 'main' panicked at src/main.rs:1:1:\nboom\n"}),
    json!({"exit": 101}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

  let error = client.initialize(&initialize_params()).unwrap_err();

  let LspError::ServerExited { cause, stderr_tail } = &error else {
    panic!("expected an exit, got {error:?}");
  };
  assert_eq!(*cause, Disconnect::ProcessExited(ProcessExit::Code(101)));
  assert_eq!(stderr_tail.len(), atlas_lsp::STDERR_TAIL_BYTES);
  assert!(stderr_tail.ends_with("xxx\nthread 'main' panicked at src/main.rs:1:1:\nboom\n"), "{stderr_tail}");
  assert!(error.to_string().starts_with("language server exited (the process exited with code 101); stderr tail:\n"));
  assert!(error.to_string().ends_with("panicked at src/main.rs:1:1:\nboom"));
}

#[test]
fn should_kill_grandchildren_holding_the_output_when_the_server_exits() {
  let scenario = Scenario::empty();
  let pid_file = scenario.path("sleeper.pid");
  scenario.write(&[
    json!({"sleeper": {"pid_file": pid_file, "inherit_stdout": true}}),
    json!({"recv": {"method": "a"}}),
    json!({"exit": 4}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());
  let sleeper = read_pid(&pid_file);

  let error = client.request::<Value>("a", json!({})).unwrap_err();

  assert!(
    matches!(
      error,
      LspError::ServerExited {
        cause: Disconnect::ProcessExited(ProcessExit::Code(4)),
        ..
      }
    ),
    "{error:?}"
  );
  assert!(wait_dead(sleeper), "sleeper {sleeper} survived the server exit");
  assert!(client.is_closed());
}

#[test]
fn should_kill_the_server_on_a_framing_error() {
  let scenario = Scenario::new(&[
    json!({"recv": {"method": "ping"}}),
    json!({"raw": "Server listening on stdio\r\n\r\n"}),
    json!({"sleep_ms": 60000}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());
  let pid = client.pid().unwrap();

  let error = client.request::<Value>("ping", json!({})).unwrap_err();

  assert!(
    matches!(error, LspError::Protocol { ref message, .. } if message == "malformed header line \"Server listening on stdio\""),
    "{error:?}"
  );
  assert!(wait_dead(pid), "server {pid} survived a protocol violation");
}

#[test]
fn should_skip_invalid_json_from_the_server() {
  let scenario = Scenario::new(&[
    json!({"recv": {"method": "ping"}}),
    json!({"raw": "Content-Length: 9\r\n\r\n{invalid}"}),
    json!({"raw_bytes": [67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32, 50, 13, 10, 13, 10, 255, 254]}),
    json!({"reply": {"method": "ping", "result": "pong"}}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

  assert_eq!(client.request::<String>("ping", json!({})).unwrap(), "pong");
  assert!(!client.is_closed());
}

#[test]
fn should_shut_down_gracefully() {
  let scenario = Scenario::new(&initialize_steps(Some("utf-16")));
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());
  client.initialize(&initialize_params()).unwrap();

  assert_eq!(client.shutdown(), ShutdownOutcome::Exited(ProcessExit::Code(0)));
  let error = client.request::<Value>("later", json!({})).unwrap_err();
  assert!(matches!(error, LspError::ServerExited { cause: Disconnect::ShutDown, .. }), "{error:?}");
}

#[test]
fn should_kill_the_process_group_on_shutdown_timeout() {
  let scenario = Scenario::empty();
  let pid_file = scenario.path("sleeper.pid");
  scenario.write(&[json!({"sleeper": {"pid_file": pid_file}}), json!("hang")]);
  let options = ClientOptions {
    shutdown_timeout: Duration::from_millis(300),
    exit_grace: Duration::from_millis(300),
    ..ClientOptions::default()
  };
  let client = scenario.spawn(Arc::new(NoopHandler), options);
  let sleeper = read_pid(&pid_file);
  let server = client.pid().unwrap();
  let started = Instant::now();

  let outcome = client.shutdown();

  assert_eq!(outcome, ShutdownOutcome::Killed(Some(ProcessExit::Signal(9))));
  assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
  assert!(wait_dead(server), "server {server} survived");
  assert!(wait_dead(sleeper), "sleeper {sleeper} survived");
}

#[test]
fn should_kill_the_process_group_when_the_client_is_dropped() {
  let scenario = Scenario::empty();
  let pid_file = scenario.path("sleeper.pid");
  scenario.write(&[json!({"sleeper": {"pid_file": pid_file}})]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());
  let sleeper = read_pid(&pid_file);
  let server = client.pid().unwrap();

  drop(client);

  assert!(wait_dead(server), "server {server} survived");
  assert!(wait_dead(sleeper), "sleeper {sleeper} survived");
}

#[test]
fn should_report_a_script_mismatch_as_an_exit() {
  let scenario = Scenario::new(&[json!({"recv": {"method": "initialize"}})]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

  let error = client.request::<Value>("other", json!({})).unwrap_err();

  assert!(
    matches!(&error, LspError::ServerExited { cause: Disconnect::ProcessExited(ProcessExit::Code(97)), stderr_tail } if stderr_tail.contains("expected [\"initialize\"], got other")),
    "{error:?}"
  );
}

#[test]
fn should_serve_canned_responses_after_the_script_ends() {
  let scenario = Scenario::new(&[
    json!({"canned": {"method": "textDocument/references", "result": [{"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}}}]}}),
    json!({"canned": {"method": "textDocument/rename", "error": {"code": -32803, "message": "cannot rename a library item"}}}),
  ]);
  let client = scenario.spawn(Arc::new(NoopHandler), ClientOptions::default());

  let first = client.request::<Value>("textDocument/references", json!({})).unwrap();
  let second = client.request::<Value>("textDocument/references", json!({})).unwrap();
  let refused = client.request::<Value>("textDocument/rename", json!({})).unwrap_err();
  let unknown = client.request::<Value>("textDocument/hover", json!({})).unwrap_err();

  assert_eq!(first, second);
  assert_eq!(first[0]["range"]["start"], json!({"line": 1, "character": 4}));
  assert!(matches!(refused, LspError::Response { ref error, .. } if error.code == -32803 && error.message == "cannot rename a library item"));
  assert!(matches!(unknown, LspError::Response { ref error, .. } if error.code == -32601));
  assert_eq!(client.shutdown(), ShutdownOutcome::Exited(ProcessExit::Code(0)));
}

#[test]
fn should_answer_a_server_request_that_arrives_while_a_client_request_waits_for_it() {
  let recorder = Recorder::new();
  let scenario = Scenario::new(&[
    json!({"recv": {"method": "textDocument/rename"}}),
    json!({"request": {"id": "cfg", "method": "workspace/configuration", "params": {"items": [{"section": "typescript"}]}}}),
    json!("recv_reply"),
    json!({"reply": {"method": "textDocument/rename", "result": {"changes": {}}}}),
  ]);
  let client = scenario.spawn(recorder.clone(), ClientOptions::default());

  let renamed = client.request::<Value>("textDocument/rename", json!({})).unwrap();

  assert_eq!(renamed, json!({"changes": {}}));
  assert_eq!(recorder.notifications(WAIT, "fake/reply", 1), vec![json!({"id": "cfg", "result": [null]})]);
}

#[test]
fn should_answer_a_burst_of_configuration_requests_in_order() {
  let recorder = Recorder::new();
  let requests = (0..20).map(|id| json!({"request": {"id": id, "method": "workspace/configuration", "params": {"items": [{"section": "rust-analyzer"}]}}}));
  let replies = (0..20).map(|_| json!("recv_reply"));
  let scenario = Scenario::new(&requests.chain(replies).collect::<Vec<_>>());
  let _client = scenario.spawn(recorder.clone(), ClientOptions::default());

  let answered = recorder.notifications(WAIT, "fake/reply", 20);

  let expected: Vec<Value> = (0..20).map(|id| json!({"id": id, "result": [null]})).collect();
  assert_eq!(answered, expected);
}

fn hung_server_options() -> ClientOptions {
  ClientOptions {
    shutdown_timeout: Duration::from_millis(300),
    exit_grace: Duration::from_millis(300),
    ..ClientOptions::default()
  }
}

#[test]
fn should_time_out_a_request_whose_payload_the_server_never_reads() {
  let scenario = Scenario::new(&[json!("hang")]);
  let client = scenario.spawn(Arc::new(NoopHandler), hung_server_options());
  let payload = json!({"text": "x".repeat(4 * 1024 * 1024)});
  let started = Instant::now();

  let error = client
    .request_with_timeout::<Value>("textDocument/didOpenLike", payload, Duration::from_millis(500))
    .unwrap_err();

  assert!(matches!(error, LspError::Timeout { ref method, .. } if method == "textDocument/didOpenLike"), "{error:?}");
  assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
  let started = Instant::now();
  assert_eq!(client.shutdown(), ShutdownOutcome::Killed(Some(ProcessExit::Signal(9))));
  assert!(started.elapsed() < Duration::from_millis(1600), "{:?}", started.elapsed());
}

#[test]
fn should_shut_down_from_another_thread_while_a_write_is_blocked() {
  let scenario = Scenario::new(&[json!("hang")]);
  let client = Arc::new(scenario.spawn(Arc::new(NoopHandler), hung_server_options()));
  let writer = {
    let client = Arc::clone(&client);
    thread::spawn(move || {
      let payload = json!({"text": "x".repeat(4 * 1024 * 1024)});
      client.request_with_timeout::<Value>("textDocument/didOpenLike", payload, Duration::from_secs(60))
    })
  };
  thread::sleep(Duration::from_millis(300));
  let started = Instant::now();

  let outcome = client.shutdown();

  assert_eq!(outcome, ShutdownOutcome::Killed(Some(ProcessExit::Signal(9))));
  assert!(started.elapsed() < Duration::from_millis(1600), "{:?}", started.elapsed());
  let error = writer.join().unwrap().unwrap_err();
  assert!(matches!(error, LspError::ServerExited { cause: Disconnect::ShutDown, .. }), "{error:?}");
}
