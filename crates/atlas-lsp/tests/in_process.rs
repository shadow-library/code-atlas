mod common;

use std::io::Write;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use atlas_lsp::framing::{FrameReader, write_frame};
use atlas_lsp::protocol::{CONTENT_MODIFIED, GotoResponse, Message, RequestId, ResponseError};
use atlas_lsp::{BoxedReader, BoxedWriter, Client, ClientOptions, Disconnect, LspError, NoopHandler, ServerEvent, ShutdownOutcome, Transport};
use common::Recorder;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

struct Peer {
  reader: FrameReader<BoxedReader>,
  writer: BoxedWriter,
}

impl Peer {
  fn recv(&mut self) -> Message {
    let body = self.reader.read_frame().unwrap().unwrap();
    Message::parse(&body).unwrap()
  }

  fn recv_request(&mut self) -> (RequestId, String, Value) {
    match self.recv() {
      Message::Request { id, method, params } => (id, method, params),
      other => panic!("expected a request, got {other:?}"),
    }
  }

  fn send(&mut self, message: &Message) {
    write_frame(&mut self.writer, &message.to_bytes()).unwrap();
  }

  fn respond(&mut self, id: RequestId, outcome: Result<Value, ResponseError>) {
    self.send(&Message::Response { id: Some(id), outcome });
  }

  fn raw(&mut self, bytes: &[u8]) {
    self.writer.write_all(bytes).unwrap();
    self.writer.flush().unwrap();
  }
}

fn connect(handler: Arc<dyn atlas_lsp::ClientHandler>) -> (Client, Peer) {
  let (client_side, server_side) = Transport::pair().unwrap();
  let client = Client::start(client_side, handler, ClientOptions::default()).unwrap();
  let (reader, writer) = server_side.into_parts();
  (
    client,
    Peer {
      reader: FrameReader::new(reader),
      writer,
    },
  )
}

#[test]
fn should_correlate_out_of_order_responses() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let client = Arc::new(client);
  let callers: Vec<_> = ["a", "b", "c"]
    .into_iter()
    .map(|method| {
      let client = Arc::clone(&client);
      thread::spawn(move || client.request::<Value>(method, json!({"from": method})).unwrap())
    })
    .collect();
  let mut received: Vec<_> = (0..3).map(|_| peer.recv_request()).collect();
  received.sort_by(|left, right| right.1.cmp(&left.1));
  for (id, method, params) in received {
    peer.respond(id, Ok(json!({"answer": method, "echo": params})));
  }

  let answers: Vec<Value> = callers.into_iter().map(|caller| caller.join().unwrap()).collect();
  assert_eq!(
    answers,
    vec![
      json!({"answer": "a", "echo": {"from": "a"}}),
      json!({"answer": "b", "echo": {"from": "b"}}),
      json!({"answer": "c", "echo": {"from": "c"}}),
    ]
  );
}

#[test]
fn should_ignore_responses_to_unknown_ids_and_keep_routing() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    let (id, _, _) = peer.recv_request();
    peer.respond(RequestId::Number(999), Ok(json!("stray")));
    peer.respond(RequestId::String("1".to_owned()), Ok(json!("string id")));
    peer.respond(id, Ok(json!("mine")));
    peer
  });

  assert_eq!(client.request::<String>("ping", Value::Null).unwrap(), "mine");
  server.join().unwrap();
}

#[test]
fn should_return_a_server_error_with_its_code() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    let (id, _, _) = peer.recv_request();
    peer.respond(id, Err(ResponseError::new(CONTENT_MODIFIED, "content modified")));
    peer
  });

  let error = client.request::<Value>("textDocument/rename", json!({})).unwrap_err();
  let LspError::Response { method, error } = error else {
    panic!("expected a response error, got {error:?}");
  };
  assert_eq!(
    (method.as_str(), error.code, error.message.as_str()),
    ("textDocument/rename", CONTENT_MODIFIED, "content modified")
  );
  server.join().unwrap();
}

#[test]
fn should_decode_null_results_and_report_undecodable_ones() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    let (id, _, _) = peer.recv_request();
    peer.respond(id, Ok(Value::Null));
    let (id, _, _) = peer.recv_request();
    peer.respond(id, Ok(json!("not locations")));
    peer
  });

  assert_eq!(client.request::<Option<GotoResponse>>("textDocument/definition", json!({})).unwrap(), None);
  let error = client.request::<GotoResponse>("textDocument/definition", json!({})).unwrap_err();
  assert!(matches!(error, LspError::Decode { ref method, .. } if method == "textDocument/definition"), "{error:?}");
  server.join().unwrap();
}

#[test]
fn should_skip_invalid_json_and_invalid_utf8_bodies_and_keep_reading() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    let (id, _, _) = peer.recv_request();
    peer.raw(b"Content-Length: 9\r\n\r\n{invalid}");
    peer.raw(b"Content-Length: 22\r\n\r\n{\"method\":\"x\xff\xfe\",\"a\":1}");
    peer.raw(b"Content-Length: 2\r\n\r\n[]");
    peer.raw(b"Content-Length: 0\r\n\r\n");
    peer.respond(id, Ok(json!({"ok": true})));
    peer
  });

  assert_eq!(client.request::<Value>("ping", Value::Null).unwrap(), json!({"ok": true}));
  assert!(!client.is_closed());
  server.join().unwrap();
}

#[test]
fn should_fail_every_pending_request_when_the_stream_ends() {
  let recorder = Recorder::new();
  let (client, mut peer) = connect(recorder.clone());
  let client = Arc::new(client);
  let callers: Vec<_> = ["a", "b"]
    .into_iter()
    .map(|method| {
      let client = Arc::clone(&client);
      thread::spawn(move || client.request::<Value>(method, Value::Null))
    })
    .collect();
  peer.recv_request();
  peer.recv_request();
  drop(peer);

  let expected = LspError::ServerExited {
    cause: Disconnect::EndOfStream,
    stderr_tail: String::new(),
  };
  for caller in callers {
    assert_eq!(caller.join().unwrap().unwrap_err().to_string(), expected.to_string());
  }
  assert_eq!(client.request::<Value>("after", Value::Null).unwrap_err().to_string(), expected.to_string());
  let events = recorder.wait_for(WAIT, |events| !events.is_empty());
  assert!(
    matches!(
      &events[..],
      [ServerEvent::Disconnected(LspError::ServerExited {
        cause: Disconnect::EndOfStream,
        ..
      })]
    ),
    "{events:?}"
  );
}

#[test]
fn should_fail_a_pending_request_when_a_frame_header_is_malformed() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    peer.recv_request();
    peer.raw(b"Content-Type: application/json\r\n\r\n{}");
    peer
  });

  let error = client.request::<Value>("ping", Value::Null).unwrap_err();
  assert!(
    matches!(error, LspError::Protocol { ref message, .. } if message == "message header has no Content-Length"),
    "{error:?}"
  );
  assert!(client.is_closed());
  server.join().unwrap();
}

#[test]
fn should_fail_a_pending_request_when_a_message_exceeds_the_size_limit() {
  let (client_side, server_side) = Transport::pair().unwrap();
  let options = ClientOptions {
    max_message_bytes: 1024,
    ..ClientOptions::default()
  };
  let client = Client::start(client_side, Arc::new(NoopHandler), options).unwrap();
  let (reader, writer) = server_side.into_parts();
  let mut peer = Peer {
    reader: FrameReader::new(reader),
    writer,
  };
  let server = thread::spawn(move || {
    peer.recv_request();
    peer.raw(b"Content-Length: 2048\r\n\r\n");
    peer
  });

  let error = client.request::<Value>("ping", Value::Null).unwrap_err();
  assert!(
    matches!(error, LspError::Protocol { ref message, .. } if message == "message of 2048 bytes exceeds the 1024 byte limit"),
    "{error:?}"
  );
  server.join().unwrap();
}

#[test]
fn should_report_a_write_to_a_closed_server_as_an_exit() {
  let (client, peer) = connect(Arc::new(NoopHandler));
  let Peer { reader, writer } = peer;
  drop(reader);

  let request_error = client.request_with_timeout::<Value>("textDocument/hover", json!({}), WAIT).unwrap_err();
  let error = client.closed_error().unwrap();
  assert_eq!(request_error.to_string(), error.to_string());
  assert!(
    matches!(
      error,
      LspError::ServerExited {
        cause: Disconnect::WriteFailed(_),
        ..
      }
    ),
    "{error:?}"
  );
  drop(writer);
}

#[test]
fn should_shut_down_and_refuse_later_requests() {
  let (client, mut peer) = connect(Arc::new(NoopHandler));
  let server = thread::spawn(move || {
    let (id, method, params) = peer.recv_request();
    peer.respond(id, Ok(Value::Null));
    let Message::Notification { method: exit, .. } = peer.recv() else {
      panic!("expected the exit notification");
    };
    let end = peer.reader.read_frame().unwrap();
    (method, params, exit, end)
  });

  assert_eq!(client.shutdown(), ShutdownOutcome::Closed);
  let (method, params, exit, end) = server.join().unwrap();
  assert_eq!((method.as_str(), params, exit.as_str(), end), ("shutdown", Value::Null, "exit", None));
  let error = client.request::<Value>("later", Value::Null).unwrap_err();
  assert!(matches!(error, LspError::ServerExited { cause: Disconnect::ShutDown, .. }), "{error:?}");
}

#[test]
fn should_deliver_exactly_one_disconnected_event_and_nothing_after_it() {
  let recorder = Recorder::new();
  let (client, mut peer) = connect(recorder.clone());
  let server = thread::spawn(move || {
    peer.send(&Message::Notification {
      method: "fake/before".to_owned(),
      params: Value::Null,
    });
    let (id, _, _) = peer.recv_request();
    peer.respond(id, Ok(Value::Null));
    peer.recv();
    peer.send(&Message::Notification {
      method: "fake/after".to_owned(),
      params: Value::Null,
    });
    peer
  });
  recorder.notifications(WAIT, "fake/before", 1);

  assert_eq!(client.shutdown(), ShutdownOutcome::Closed);
  let peer = server.join().unwrap();
  thread::sleep(Duration::from_millis(200));
  drop(client);
  drop(peer);
  thread::sleep(Duration::from_millis(100));

  let events = recorder.events();
  assert_eq!(events.len(), 2, "{events:?}");
  assert!(matches!(&events[0], ServerEvent::Notification { method, .. } if method == "fake/before"));
  assert!(matches!(&events[1], ServerEvent::Disconnected(LspError::ServerExited { cause: Disconnect::ShutDown, .. })));
}
