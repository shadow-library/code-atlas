#![allow(dead_code)]

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use atlas_lsp::{ClientHandler, ServerEvent};

#[derive(Default)]
pub struct Recorder {
  events: Mutex<Vec<ServerEvent>>,
  changed: Condvar,
}

impl Recorder {
  pub fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  pub fn events(&self) -> Vec<ServerEvent> {
    self.events.lock().unwrap().clone()
  }

  pub fn wait_for(&self, timeout: Duration, done: impl Fn(&[ServerEvent]) -> bool) -> Vec<ServerEvent> {
    let events = self.events.lock().unwrap();
    let (events, _) = self.changed.wait_timeout_while(events, timeout, |events| !done(events)).unwrap();
    events.clone()
  }

  pub fn notifications(&self, timeout: Duration, method: &str, count: usize) -> Vec<serde_json::Value> {
    let matching = |events: &[ServerEvent]| -> Vec<serde_json::Value> {
      events
        .iter()
        .filter_map(|event| match event {
          ServerEvent::Notification { method: seen, params } if seen == method => Some(params.clone()),
          _ => None,
        })
        .collect()
    };
    matching(&self.wait_for(timeout, |events| matching(events).len() >= count))
  }
}

impl ClientHandler for Recorder {
  fn on_event(&self, event: ServerEvent) {
    self.events.lock().unwrap().push(event);
    self.changed.notify_all();
  }
}

pub mod fake;
