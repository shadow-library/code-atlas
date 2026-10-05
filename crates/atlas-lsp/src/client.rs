use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Disconnect, LspError, LspResult};
use crate::framing::{FrameReader, FramingError, MAX_MESSAGE_BYTES, encode_frame};
use crate::position::{PositionEncoding, negotiate_encoding};
use crate::process::{ExitHandle, ProcessExit, ServerCommand, ServerProcess, StderrTail, kill_group};
use crate::protocol::{
  ApplyWorkspaceEditResult, ConfigurationItem, ConfigurationParams, INTERNAL_ERROR, INVALID_PARAMS, InitializeParams, InitializeResult, METHOD_NOT_FOUND, Message, ProgressParams,
  ProgressToken, PublishDiagnosticsParams, RequestId, ResponseError, ServerStatusParams, ShowMessageParams, WorkDoneProgressCreateParams, WorkspaceFolder,
};
use crate::transport::{BoxedReader, BoxedWriter, Transport};

const STDERR_GRACE: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const OUTBOX_CAPACITY: usize = 64;
const APPLY_EDIT_REFUSAL: &str = "code-atlas applies edits itself; language servers never write";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOptions {
  pub request_timeout: Duration,
  pub initialize_timeout: Duration,
  pub shutdown_timeout: Duration,
  pub exit_grace: Duration,
  pub max_message_bytes: usize,
}

impl Default for ClientOptions {
  fn default() -> Self {
    Self {
      request_timeout: Duration::from_secs(60),
      initialize_timeout: Duration::from_secs(60),
      shutdown_timeout: Duration::from_secs(5),
      exit_grace: Duration::from_secs(2),
      max_message_bytes: MAX_MESSAGE_BYTES,
    }
  }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ServerEvent {
  Progress(ProgressParams),
  ProgressCreated(ProgressToken),
  ServerStatus(ServerStatusParams),
  Diagnostics(PublishDiagnosticsParams),
  ShowMessage(ShowMessageParams),
  Notification { method: String, params: Value },
  Disconnected(LspError),
}

/// `on_event` runs on the reader thread under the event gate: it must only record state and notify, never block or call the client.
pub trait ClientHandler: Send + Sync + 'static {
  fn configuration(&self, _item: &ConfigurationItem) -> Value {
    Value::Null
  }

  fn workspace_folders(&self) -> Option<Vec<WorkspaceFolder>> {
    None
  }

  fn on_event(&self, _event: ServerEvent) {}
}

pub struct NoopHandler;

impl ClientHandler for NoopHandler {}

#[derive(Debug, Clone)]
pub struct Initialized {
  pub result: InitializeResult,
  pub encoding: PositionEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
  Exited(ProcessExit),
  Killed(Option<ProcessExit>),
  Closed,
}

struct Pending {
  method: String,
  sender: SyncSender<LspResult<Value>>,
}

#[derive(Default)]
enum Phase {
  #[default]
  Open,
  ShuttingDown,
  Closed(LspError),
}

#[derive(Default)]
struct State {
  pending: HashMap<i64, Pending>,
  phase: Phase,
}

impl State {
  fn closed(&self) -> Option<&LspError> {
    match &self.phase {
      Phase::Closed(error) => Some(error),
      Phase::Open | Phase::ShuttingDown => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboxPhase {
  Open,
  Draining,
  Closed,
}

enum EnqueueError {
  Full,
  Closed,
}

struct OutboxState {
  frames: VecDeque<Vec<u8>>,
  phase: OutboxPhase,
}

struct Outbox {
  state: Mutex<OutboxState>,
  changed: Condvar,
}

impl Outbox {
  fn new() -> Self {
    Self {
      state: Mutex::new(OutboxState {
        frames: VecDeque::new(),
        phase: OutboxPhase::Open,
      }),
      changed: Condvar::new(),
    }
  }

  fn push(&self, frame: Vec<u8>, deadline: Instant) -> Result<(), EnqueueError> {
    let mut state = lock(&self.state);
    loop {
      if state.phase != OutboxPhase::Open {
        return Err(EnqueueError::Closed);
      }
      if state.frames.len() < OUTBOX_CAPACITY {
        state.frames.push_back(frame);
        self.changed.notify_all();
        return Ok(());
      }
      let now = Instant::now();
      if now >= deadline {
        return Err(EnqueueError::Full);
      }
      state = self.changed.wait_timeout(state, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
    }
  }

  fn next(&self) -> Option<Vec<u8>> {
    let mut state = lock(&self.state);
    loop {
      if let Some(frame) = state.frames.pop_front() {
        self.changed.notify_all();
        return Some(frame);
      }
      if state.phase != OutboxPhase::Open {
        state.phase = OutboxPhase::Closed;
        return None;
      }
      state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
    }
  }

  fn drain_and_close(&self) {
    let mut state = lock(&self.state);
    if state.phase == OutboxPhase::Open {
      state.phase = OutboxPhase::Draining;
    }
    self.changed.notify_all();
  }

  fn abort(&self) {
    let mut state = lock(&self.state);
    state.phase = OutboxPhase::Closed;
    state.frames.clear();
    self.changed.notify_all();
  }
}

struct ProcessLink {
  pid: u32,
  exit: ExitHandle,
  stderr: StderrTail,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventGate {
  Open,
  Closed,
}

struct Shared {
  state: Mutex<State>,
  closed: Condvar,
  outbox: Outbox,
  events: Mutex<EventGate>,
  handler: Arc<dyn ClientHandler>,
  process: Option<ProcessLink>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
  fn is_closed(&self) -> bool {
    lock(&self.state).closed().is_some()
  }

  fn closed_error(&self) -> LspError {
    lock(&self.state).closed().cloned().unwrap_or_else(shut_down)
  }

  fn wait_closed(&self, timeout: Duration) {
    let state = lock(&self.state);
    let _unused = self.closed.wait_timeout_while(state, timeout, |state| state.closed().is_none());
  }

  fn stderr_tail(&self) -> String {
    let Some(process) = &self.process else {
      return String::new();
    };
    process.stderr.wait_closed(STDERR_GRACE);
    process.stderr.snapshot()
  }

  fn disconnect(&self, cause: Disconnect) {
    let shutting_down = match &lock(&self.state).phase {
      Phase::Closed(_) => return,
      Phase::ShuttingDown => true,
      Phase::Open => false,
    };
    if shutting_down {
      return self.close(shut_down());
    }
    let cause = match (&cause, &self.process) {
      (Disconnect::EndOfStream | Disconnect::Truncated, Some(process)) => process.exit.wait(OUTPUT_DRAIN_GRACE).map_or(cause, Disconnect::ProcessExited),
      _ => cause,
    };
    let stderr_tail = self.stderr_tail();
    self.close(LspError::ServerExited { cause, stderr_tail });
  }

  fn protocol_failure(&self, message: String) {
    if let Some(process) = &self.process
      && process.exit.get().is_none()
    {
      kill_group(process.pid);
    }
    let stderr_tail = self.stderr_tail();
    self.close(LspError::Protocol { message, stderr_tail });
  }

  fn close(&self, error: LspError) {
    let pending = {
      let mut state = lock(&self.state);
      if state.closed().is_some() {
        return;
      }
      state.phase = Phase::Closed(error.clone());
      std::mem::take(&mut state.pending)
    };
    self.closed.notify_all();
    self.outbox.drain_and_close();
    tracing::debug!(%error, pending = pending.len(), "language server connection closed");
    for (_, pending) in pending {
      let _unused = pending.sender.try_send(Err(error.clone()));
    }
    let mut gate = lock(&self.events);
    *gate = EventGate::Closed;
    self.handler.on_event(ServerEvent::Disconnected(error));
  }

  fn emit(&self, event: ServerEvent) {
    let gate = lock(&self.events);
    if *gate == EventGate::Open {
      self.handler.on_event(event);
    }
  }

  fn enqueue(&self, method: &str, message: &Message, deadline: Instant, waited: Duration) -> LspResult<()> {
    match self.outbox.push(encode_frame(&message.to_bytes()), deadline) {
      Ok(()) => Ok(()),
      Err(EnqueueError::Closed) => Err(self.closed_error()),
      Err(EnqueueError::Full) => Err(LspError::Timeout {
        method: method.to_owned(),
        after: waited,
      }),
    }
  }

  fn write_loop(&self, mut writer: BoxedWriter) {
    while let Some(frame) = self.outbox.next() {
      if let Err(error) = writer.write_all(&frame).and_then(|()| writer.flush()) {
        self.outbox.abort();
        self.disconnect(Disconnect::WriteFailed(error.to_string()));
        return;
      }
    }
  }

  fn read_loop(&self, reader: BoxedReader, limit: usize, requests: &Sender<(RequestId, String, Value)>) {
    let mut frames = FrameReader::with_limit(reader, limit);
    loop {
      let body = match frames.read_frame() {
        Ok(Some(body)) => body,
        Ok(None) => return self.disconnect(Disconnect::EndOfStream),
        Err(FramingError::Truncated) => return self.disconnect(Disconnect::Truncated),
        Err(FramingError::Io(error)) => return self.disconnect(Disconnect::ReadFailed(error.to_string())),
        Err(error) => return self.protocol_failure(error.to_string()),
      };
      match Message::parse(&body) {
        Ok(Message::Response { id, outcome }) => self.route(id, outcome),
        Ok(Message::Notification { method, params }) => self.notification(method, params),
        Ok(Message::Request { id, method, params }) => {
          if method == "window/workDoneProgress/create"
            && let Ok(created) = serde_json::from_value::<WorkDoneProgressCreateParams>(params.clone())
          {
            self.emit(ServerEvent::ProgressCreated(created.token));
          }
          let _unused = requests.send((id, method, params));
        }
        Err(error) => tracing::warn!(%error, bytes = body.len(), "skipping a malformed message from the language server"),
      }
    }
  }

  fn route(&self, id: Option<RequestId>, outcome: Result<Value, ResponseError>) {
    let pending = match id {
      Some(RequestId::Number(id)) => lock(&self.state).pending.remove(&id),
      _ => None,
    };
    let Some(pending) = pending else {
      tracing::debug!(?id, "dropping a response nobody is waiting for");
      return;
    };
    let result = outcome.map_err(|error| LspError::Response { method: pending.method, error });
    let _unused = pending.sender.try_send(result);
  }

  fn notification(&self, method: String, params: Value) {
    let event = match method.as_str() {
      "$/progress" => serde_json::from_value(params.clone()).map(ServerEvent::Progress).ok(),
      "experimental/serverStatus" => serde_json::from_value(params.clone()).map(ServerEvent::ServerStatus).ok(),
      "textDocument/publishDiagnostics" => serde_json::from_value(params.clone()).map(ServerEvent::Diagnostics).ok(),
      "window/showMessage" => serde_json::from_value(params.clone()).map(ServerEvent::ShowMessage).ok(),
      "window/logMessage" => {
        let text = params.get("message").and_then(Value::as_str).unwrap_or_default();
        tracing::debug!(text, "language server log");
        return;
      }
      "$/logTrace" | "$/cancelRequest" => return,
      _ => None,
    };
    self.emit(event.unwrap_or(ServerEvent::Notification { method, params }));
  }

  fn answer_loop(&self, requests: &Receiver<(RequestId, String, Value)>, reply_timeout: Duration) {
    for (id, method, params) in requests {
      let outcome = answer_server_request(self.handler.as_ref(), &method, params);
      if let Err(error) = &outcome {
        tracing::debug!(method, code = error.code, "refusing a language server request");
      }
      let reply = Message::Response { id: Some(id), outcome };
      if let Err(error) = self.enqueue(&method, &reply, Instant::now() + reply_timeout, reply_timeout) {
        tracing::debug!(%error, method, "cannot answer a language server request");
      }
    }
  }
}

fn shut_down() -> LspError {
  LspError::ServerExited {
    cause: Disconnect::ShutDown,
    stderr_tail: String::new(),
  }
}

fn answer_server_request(handler: &dyn ClientHandler, method: &str, params: Value) -> Result<Value, ResponseError> {
  match method {
    "workspace/configuration" => {
      let params: ConfigurationParams = serde_json::from_value(params).map_err(|error| ResponseError::new(INVALID_PARAMS, error.to_string()))?;
      Ok(Value::Array(params.items.iter().map(|item| handler.configuration(item)).collect()))
    }
    "workspace/workspaceFolders" => handler.workspace_folders().map_or(Ok(Value::Null), reply),
    "workspace/applyEdit" => reply(ApplyWorkspaceEditResult {
      applied: false,
      failure_reason: Some(APPLY_EDIT_REFUSAL.to_owned()),
    }),
    "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" | "window/showMessageRequest" => Ok(Value::Null),
    refresh if refresh.starts_with("workspace/") && refresh.ends_with("/refresh") => Ok(Value::Null),
    _ => Err(ResponseError::new(METHOD_NOT_FOUND, format!("code-atlas does not handle {method}"))),
  }
}

fn reply(result: impl Serialize) -> Result<Value, ResponseError> {
  serde_json::to_value(result).map_err(|error| ResponseError::new(INTERNAL_ERROR, error.to_string()))
}

pub struct Client {
  shared: Arc<Shared>,
  next_id: AtomicI64,
  encoding: OnceLock<PositionEncoding>,
  options: ClientOptions,
  process: Option<ServerProcess>,
}

impl Client {
  pub fn start(transport: Transport, handler: Arc<dyn ClientHandler>, options: ClientOptions) -> LspResult<Self> {
    Self::connect(transport, handler, options, None)
  }

  pub fn spawn(command: &ServerCommand, handler: Arc<dyn ClientHandler>, options: ClientOptions) -> LspResult<Self> {
    let (process, transport) = ServerProcess::spawn(command)?;
    let mut client = Self::connect(transport, handler, options, Some(&process))?;
    let shared = Arc::clone(&client.shared);
    process.on_exit(move |exit| {
      shared.wait_closed(OUTPUT_DRAIN_GRACE);
      shared.disconnect(Disconnect::ProcessExited(exit));
    });
    client.process = Some(process);
    Ok(client)
  }

  fn connect(transport: Transport, handler: Arc<dyn ClientHandler>, options: ClientOptions, process: Option<&ServerProcess>) -> LspResult<Self> {
    let (reader, writer) = transport.into_parts();
    let shared = Arc::new(Shared {
      state: Mutex::new(State::default()),
      closed: Condvar::new(),
      outbox: Outbox::new(),
      events: Mutex::new(EventGate::Open),
      handler,
      process: process.map(|process| ProcessLink {
        pid: process.pid(),
        exit: process.exit_handle(),
        stderr: process.stderr().clone(),
      }),
    });
    let writing = Arc::clone(&shared);
    spawn_thread("lsp-writer", move || writing.write_loop(writer))?;
    let (requests, incoming) = mpsc::channel();
    let reading = Arc::clone(&shared);
    let limit = options.max_message_bytes;
    spawn_thread("lsp-reader", move || reading.read_loop(reader, limit, &requests))?;
    let answering = Arc::clone(&shared);
    let reply_timeout = options.request_timeout;
    spawn_thread("lsp-responder", move || answering.answer_loop(&incoming, reply_timeout))?;
    Ok(Self {
      shared,
      next_id: AtomicI64::new(1),
      encoding: OnceLock::new(),
      options,
      process: None,
    })
  }

  pub fn options(&self) -> &ClientOptions {
    &self.options
  }

  pub fn pid(&self) -> Option<u32> {
    self.process.as_ref().map(ServerProcess::pid)
  }

  pub fn stderr_tail(&self) -> String {
    self.process.as_ref().map(|process| process.stderr().snapshot()).unwrap_or_default()
  }

  pub fn is_closed(&self) -> bool {
    self.shared.is_closed()
  }

  pub fn closed_error(&self) -> Option<LspError> {
    lock(&self.shared.state).closed().cloned()
  }

  pub fn position_encoding(&self) -> Option<PositionEncoding> {
    self.encoding.get().copied()
  }

  pub fn initialize(&self, params: &InitializeParams) -> LspResult<Initialized> {
    let result: InitializeResult = self.request_with_timeout("initialize", params, self.options.initialize_timeout)?;
    let encoding = negotiate_encoding(&params.capabilities.general.position_encodings, result.capabilities.position_encoding.as_deref())?;
    self.notify("initialized", json!({}))?;
    let encoding = *self.encoding.get_or_init(|| encoding);
    Ok(Initialized { result, encoding })
  }

  pub fn request<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> LspResult<R> {
    self.request_with_timeout(method, params, self.options.request_timeout)
  }

  pub fn request_with_timeout<R: DeserializeOwned>(&self, method: &str, params: impl Serialize, timeout: Duration) -> LspResult<R> {
    let params = encode(method, params)?;
    let value = self.request_value(method, params, timeout)?;
    serde_json::from_value(value).map_err(|error| LspError::Decode {
      method: method.to_owned(),
      message: error.to_string(),
    })
  }

  pub fn request_value(&self, method: &str, params: Value, timeout: Duration) -> LspResult<Value> {
    let deadline = Instant::now() + timeout;
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = mpsc::sync_channel(1);
    {
      let mut state = lock(&self.shared.state);
      if let Some(error) = state.closed() {
        return Err(error.clone());
      }
      state.pending.insert(
        id,
        Pending {
          method: method.to_owned(),
          sender,
        },
      );
    }
    let request = Message::Request {
      id: RequestId::Number(id),
      method: method.to_owned(),
      params,
    };
    if let Err(error) = self.shared.enqueue(method, &request, deadline, timeout) {
      lock(&self.shared.state).pending.remove(&id);
      return Err(error);
    }
    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
      Ok(result) => result,
      Err(RecvTimeoutError::Disconnected) => Err(self.shared.closed_error()),
      Err(RecvTimeoutError::Timeout) => self.expire(id, method, timeout, &receiver),
    }
  }

  fn expire(&self, id: i64, method: &str, timeout: Duration, receiver: &Receiver<LspResult<Value>>) -> LspResult<Value> {
    let still_pending = lock(&self.shared.state).pending.remove(&id).is_some();
    if !still_pending {
      return receiver.recv().unwrap_or_else(|_| Err(self.shared.closed_error()));
    }
    let cancel = Message::Notification {
      method: "$/cancelRequest".to_owned(),
      params: json!({ "id": id }),
    };
    if let Err(error) = self.shared.enqueue("$/cancelRequest", &cancel, Instant::now(), Duration::ZERO) {
      tracing::debug!(%error, method, "cannot cancel a timed-out request");
    }
    Err(LspError::Timeout {
      method: method.to_owned(),
      after: timeout,
    })
  }

  pub fn notify(&self, method: &str, params: impl Serialize) -> LspResult<()> {
    self.notify_with_timeout(method, params, self.options.request_timeout)
  }

  pub fn notify_with_timeout(&self, method: &str, params: impl Serialize, timeout: Duration) -> LspResult<()> {
    let params = encode(method, params)?;
    if let Some(error) = self.closed_error() {
      return Err(error);
    }
    let notification = Message::Notification {
      method: method.to_owned(),
      params,
    };
    self.shared.enqueue(method, &notification, Instant::now() + timeout, timeout)
  }

  pub fn shutdown(&self) -> ShutdownOutcome {
    let deadline = Instant::now() + self.options.shutdown_timeout;
    if let Err(error) = self.request_with_timeout::<Value>("shutdown", Value::Null, self.options.shutdown_timeout) {
      tracing::debug!(%error, "language server did not acknowledge shutdown");
    }
    {
      let mut state = lock(&self.shared.state);
      if let Phase::Open = state.phase {
        state.phase = Phase::ShuttingDown;
      }
    }
    let exit = Message::Notification {
      method: "exit".to_owned(),
      params: Value::Null,
    };
    if let Err(error) = self.shared.enqueue("exit", &exit, deadline, self.options.shutdown_timeout) {
      tracing::debug!(%error, "cannot send exit to the language server");
    }
    self.shared.outbox.drain_and_close();
    let outcome = match &self.process {
      None => ShutdownOutcome::Closed,
      Some(process) => match process.wait_exit(self.options.exit_grace) {
        Some(exit) => ShutdownOutcome::Exited(exit),
        None => ShutdownOutcome::Killed(process.kill()),
      },
    };
    self.shared.close(shut_down());
    outcome
  }
}

impl Drop for Client {
  fn drop(&mut self) {
    self.shared.close(shut_down());
  }
}

fn encode(method: &str, params: impl Serialize) -> LspResult<Value> {
  serde_json::to_value(params).map_err(|error| LspError::Encode {
    method: method.to_owned(),
    message: error.to_string(),
  })
}

fn spawn_thread(name: &str, body: impl FnOnce() + Send + 'static) -> LspResult<()> {
  thread::Builder::new().name(name.to_owned()).spawn(body).map(drop).map_err(|error| LspError::Spawn {
    program: name.to_owned(),
    reason: error.to_string(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  struct Settings;

  impl ClientHandler for Settings {
    fn configuration(&self, item: &ConfigurationItem) -> Value {
      match item.section.as_deref() {
        Some("rust-analyzer") => json!({"linkedProjects": ["/w/Cargo.toml"]}),
        _ => Value::Null,
      }
    }

    fn workspace_folders(&self) -> Option<Vec<WorkspaceFolder>> {
      Some(vec![WorkspaceFolder {
        uri: "file:///w".to_owned(),
        name: "w".to_owned(),
      }])
    }
  }

  #[test]
  fn should_answer_workspace_configuration_with_the_unit_settings() {
    let answer = answer_server_request(
      &Settings,
      "workspace/configuration",
      json!({"items": [{"section": "rust-analyzer"}, {"section": "editor"}]}),
    );

    assert_eq!(answer, Ok(json!([{"linkedProjects": ["/w/Cargo.toml"]}, null])));
  }

  #[test]
  fn should_answer_workspace_folders_and_acknowledge_registrations_and_refreshes() {
    assert_eq!(
      answer_server_request(&Settings, "workspace/workspaceFolders", Value::Null),
      Ok(json!([{"uri": "file:///w", "name": "w"}]))
    );
    assert_eq!(answer_server_request(&NoopHandler, "workspace/workspaceFolders", Value::Null), Ok(Value::Null));
    for method in [
      "window/workDoneProgress/create",
      "client/registerCapability",
      "client/unregisterCapability",
      "window/showMessageRequest",
      "workspace/semanticTokens/refresh",
      "workspace/diagnostic/refresh",
    ] {
      assert_eq!(answer_server_request(&NoopHandler, method, json!({})), Ok(Value::Null), "{method}");
    }
  }

  #[test]
  fn should_refuse_workspace_apply_edit() {
    let answer = answer_server_request(&NoopHandler, "workspace/applyEdit", json!({"edit": {"changes": {}}}));

    assert_eq!(answer, Ok(json!({"applied": false, "failureReason": APPLY_EDIT_REFUSAL})));
  }

  #[test]
  fn should_answer_unknown_server_requests_with_method_not_found() {
    let answer = answer_server_request(&NoopHandler, "custom/thing", Value::Null);

    assert_eq!(answer.map_err(|error| error.code), Err(METHOD_NOT_FOUND));
  }

  #[test]
  fn should_reject_malformed_configuration_params() {
    let answer = answer_server_request(&NoopHandler, "workspace/configuration", json!({"items": 3}));

    assert_eq!(answer.map_err(|error| error.code), Err(INVALID_PARAMS));
  }

  #[test]
  fn should_return_a_response_routed_while_the_request_was_timing_out() {
    let (client_side, _server_side) = Transport::pair().unwrap();
    let client = Client::start(client_side, Arc::new(NoopHandler), ClientOptions::default()).unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    let router = thread::spawn(move || {
      thread::sleep(Duration::from_millis(100));
      sender.send(Ok(json!("routed"))).unwrap();
    });

    let result = client.expire(7, "slow", Duration::from_millis(1), &receiver);

    assert_eq!(result.unwrap(), json!("routed"));
    router.join().unwrap();
  }
}
