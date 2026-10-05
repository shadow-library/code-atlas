use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::Duration;

use atlas_lsp::framing::{FrameReader, write_frame};
use atlas_lsp::protocol::{Message, RequestId, ResponseError};
use serde::Deserialize;
use serde_json::{Value, json};

const SCRIPT_MISMATCH: i32 = 97;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Step {
  Recv {
    method: String,
    #[serde(default)]
    params: Option<Value>,
  },
  RecvAll(Vec<String>),
  Reply {
    method: String,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<ResponseError>,
  },
  RecvReply,
  Notify {
    method: String,
    #[serde(default)]
    params: Value,
  },
  Request {
    id: RequestId,
    method: String,
    #[serde(default)]
    params: Value,
  },
  SleepMs(u64),
  Stderr(String),
  Exit(i32),
  Raw(String),
  RawBytes(Vec<u8>),
  Sleeper {
    pid_file: PathBuf,
    #[serde(default)]
    inherit_stdout: bool,
  },
  Hang,
  Canned {
    method: String,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<ResponseError>,
  },
}

struct Server {
  input: FrameReader<io::Stdin>,
  pending: VecDeque<(RequestId, String)>,
  canned: HashMap<String, Result<Value, ResponseError>>,
}

fn main() {
  let Some(path) = std::env::args_os().nth(1) else {
    fail("usage: atlas-fake-lsp <script.jsonl>");
  };
  let script = fs::read_to_string(&path).unwrap_or_else(|error| fail(&format!("cannot read the script: {error}")));
  let steps: Vec<Step> = script
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(|line| serde_json::from_str(line).unwrap_or_else(|error| fail(&format!("bad script line {line}: {error}"))))
    .collect();
  let mut server = Server {
    input: FrameReader::new(io::stdin()),
    pending: VecDeque::new(),
    canned: HashMap::new(),
  };
  for step in steps {
    server.run(step);
  }
  server.serve_until_exit();
}

impl Server {
  fn run(&mut self, step: Step) {
    match step {
      Step::Recv { method, params } => self.expect(&[method], params.as_ref()),
      Step::RecvAll(methods) => self.expect(&methods, None),
      Step::Reply { method, result, error } => self.reply(&method, error.map_or(Ok(result), Err)),
      Step::RecvReply => self.recv_reply(),
      Step::Notify { method, params } => send(&Message::Notification { method, params }),
      Step::Request { id, method, params } => send(&Message::Request { id, method, params }),
      Step::SleepMs(millis) => thread::sleep(Duration::from_millis(millis)),
      Step::Stderr(text) => {
        let mut stderr = io::stderr();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
      }
      Step::Exit(code) => process::exit(code),
      Step::Raw(text) => write_raw(text.as_bytes()),
      Step::RawBytes(bytes) => write_raw(&bytes),
      Step::Sleeper { pid_file, inherit_stdout } => spawn_sleeper(&pid_file, inherit_stdout),
      Step::Canned { method, result, error } => {
        self.canned.insert(method, error.map_or(Ok(result), Err));
      }
      Step::Hang => loop {
        thread::sleep(Duration::from_secs(3600));
      },
    }
  }

  fn next(&mut self) -> Option<Message> {
    match self.input.read_frame() {
      Ok(Some(body)) => Some(Message::parse(&body).unwrap_or_else(|error| fail(&format!("client sent a malformed message: {error}")))),
      Ok(None) => None,
      Err(error) => fail(&format!("client framing error: {error}")),
    }
  }

  fn expect(&mut self, methods: &[String], params: Option<&Value>) {
    let mut remaining: Vec<&String> = methods.iter().collect();
    while !remaining.is_empty() {
      let Some(message) = self.next() else {
        fail(&format!("client closed the stream while {remaining:?} were expected"));
      };
      let (method, actual, id) = match message {
        Message::Request { id, method, params } => (method, params, Some(id)),
        Message::Notification { method, params } => (method, params, None),
        Message::Response { .. } => fail(&format!("expected {remaining:?}, got a response")),
      };
      let Some(index) = remaining.iter().position(|expected| **expected == method) else {
        fail(&format!("expected {remaining:?}, got {method}"));
      };
      if let Some(expected) = params
        && !contains(&actual, expected)
      {
        fail(&format!("{method} params {actual} do not contain {expected}"));
      }
      remaining.remove(index);
      if let Some(id) = id {
        self.pending.push_back((id, method));
      }
    }
  }

  fn reply(&mut self, method: &str, outcome: Result<Value, ResponseError>) {
    let Some(index) = self.pending.iter().position(|(_, pending)| pending == method) else {
      fail(&format!("no pending {method} request to reply to"));
    };
    let Some((id, _)) = self.pending.remove(index) else {
      fail("pending queue changed underneath");
    };
    send(&Message::Response { id: Some(id), outcome });
  }

  fn recv_reply(&mut self) {
    match self.next() {
      Some(Message::Response { id, outcome }) => {
        let echoed = match outcome {
          Ok(result) => json!({"id": id, "result": result}),
          Err(error) => json!({"id": id, "error": {"code": error.code, "message": error.message}}),
        };
        send(&Message::Notification {
          method: "fake/reply".to_owned(),
          params: echoed,
        });
      }
      other => fail(&format!("expected a response, got {other:?}")),
    }
  }

  fn serve_until_exit(&mut self) -> ! {
    while let Some(message) = self.next() {
      match message {
        Message::Request { id, method, .. } if method == "shutdown" => send(&Message::Response {
          id: Some(id),
          outcome: Ok(Value::Null),
        }),
        Message::Request { id, method, .. } => {
          let outcome = self.canned.get(&method).cloned();
          send(&Message::Response {
            id: Some(id),
            outcome: outcome.unwrap_or_else(|| Err(ResponseError::new(-32601, format!("fake server has no {method}")))),
          });
        }
        Message::Notification { method, .. } if method == "exit" => process::exit(0),
        _ => {}
      }
    }
    process::exit(0)
  }
}

fn contains(actual: &Value, expected: &Value) -> bool {
  match (actual, expected) {
    (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| actual.get(key).is_some_and(|present| contains(present, value))),
    _ => actual == expected,
  }
}

fn send(message: &Message) {
  let mut stdout = io::stdout().lock();
  if write_frame(&mut stdout, &message.to_bytes()).is_err() {
    process::exit(0);
  }
}

fn write_raw(bytes: &[u8]) {
  let mut stdout = io::stdout().lock();
  if stdout.write_all(bytes).and_then(|()| stdout.flush()).is_err() {
    process::exit(0);
  }
}

fn spawn_sleeper(pid_file: &Path, inherit_stdout: bool) {
  let stdout = match inherit_stdout {
    true => Stdio::inherit(),
    false => Stdio::null(),
  };
  let mut child = Command::new("sleep")
    .arg("600")
    .stdin(Stdio::null())
    .stdout(stdout)
    .stderr(Stdio::null())
    .spawn()
    .unwrap_or_else(|error| fail(&format!("cannot spawn the sleeper: {error}")));
  fs::write(pid_file, child.id().to_string()).unwrap_or_else(|error| fail(&format!("cannot write the sleeper pid: {error}")));
  thread::spawn(move || child.wait());
}

fn fail(message: &str) -> ! {
  let _ = writeln!(io::stderr(), "atlas-fake-lsp: {message}");
  process::exit(SCRIPT_MISMATCH)
}
