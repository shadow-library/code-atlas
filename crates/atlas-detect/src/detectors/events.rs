use atlas_core::{ArgSummary, EdgeKind, EntityKey, FileFacts, LanguageFamily, NodeRef, ReceiverHint, RefKind, ReferenceFact};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::sink::{Sink, attrs};
use crate::view::FileView;

pub struct EventDetector;

detector!(EventDetector, "events", 1);

const BUS_WORDS: [&str; 8] = ["events", "emitter", "bus", "broker", "queue", "producer", "consumer", "pubsub"];
const PUBLISH_METHODS: [&str; 4] = ["emit", "emitAsync", "publish", "send"];
const SUBSCRIBE_METHODS: [&str; 5] = ["on", "once", "subscribe", "addListener", "listen"];
const EMITTER_TYPES: [&str; 2] = ["EventEmitter", "EventEmitter2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
  Publish,
  Subscribe,
}

impl EventDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.references.iter().any(|reference| {
      reference
        .path
        .last()
        .is_some_and(|name| PUBLISH_METHODS.contains(&name.as_str()) || SUBSCRIBE_METHODS.contains(&name.as_str()) || name == "OnEvent")
    })
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    for reference in &view.facts.references {
      match reference.kind {
        RefKind::MethodCall => self.method_call(view, sink, reference),
        RefKind::Decorator if view.family() == LanguageFamily::Ts => self.on_event(view, sink, reference),
        _ => {}
      }
    }
  }

  fn method_call(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact) {
    let Some((method, receiver)) = reference.path.split_last() else { return };
    let direction = if PUBLISH_METHODS.contains(&method.as_str()) {
      Direction::Publish
    } else if SUBSCRIBE_METHODS.contains(&method.as_str()) {
      Direction::Subscribe
    } else {
      return;
    };
    let Some(confidence) = bus_confidence(view, reference, receiver) else { return };
    for name in topic_names(method, reference.args.first()) {
      emit_event(view, sink, reference, &name, direction, Cert::heuristic(confidence, "event-bus-call"));
    }
  }

  fn on_event(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact) {
    let [name] = reference.path.as_slice() else { return };
    if !view.imports_named(name, &["@nestjs/event-emitter"], "OnEvent") {
      return;
    }
    let Some(ArgSummary::Str(event)) = reference.args.first() else { return };
    emit_event(view, sink, reference, event, Direction::Subscribe, Cert::heuristic(0.85, "nest-on-event"));
  }
}

fn emit_event(view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact, name: &str, direction: Direction, cert: Cert) {
  let event = EntityKey::Event { name: name.to_owned() };
  let edge_kind = match direction {
    Direction::Publish => EdgeKind::Publishes,
    Direction::Subscribe => EdgeKind::Subscribes,
  };
  sink.entity(event.clone(), reference.span, cert, attrs([("experimental", json!(true))]));
  sink.edge(
    edge_kind,
    view.node(reference.from),
    NodeRef::Key(event),
    reference.span,
    cert,
    attrs([("experimental", json!(true))]),
  );
}

fn bus_confidence(view: &FileView<'_>, reference: &ReferenceFact, receiver: &[String]) -> Option<f32> {
  let name = receiver.last()?;
  if name.starts_with('(') {
    return None;
  }
  let typed = match &reference.receiver {
    Some(ReceiverHint::TypeHint(path)) => path.last().cloned(),
    _ => None,
  }
  .or_else(|| match receiver {
    [this, field] if this == "this" || this == "self" => reference.from.and_then(|local| view.class_of(local)).and_then(|class| view.field_type(class, field)),
    _ => None,
  });
  let type_suggests_bus = typed.as_deref().is_some_and(|type_text| {
    let base = type_text.trim_start_matches(['&', ' ']).trim_start_matches("mut ").trim_start_matches("dyn ").trim();
    EMITTER_TYPES.contains(&base) || is_bus_word(base)
  });
  match (type_suggests_bus, is_bus_word(name)) {
    (true, _) => Some(0.8),
    (false, true) => Some(0.7),
    (false, false) => None,
  }
}

fn is_bus_word(name: &str) -> bool {
  let words = identifier_words(name);
  let Some(last) = words.last() else { return false };
  let pair = words.len().checked_sub(2).map(|index| words[index..].concat());
  BUS_WORDS.contains(&last.as_str()) || pair.is_some_and(|pair| pair == "pubsub")
}

fn identifier_words(name: &str) -> Vec<String> {
  let mut words: Vec<String> = Vec::new();
  let mut current = String::new();
  let mut previous_lower = false;
  for c in name.chars() {
    if !c.is_alphanumeric() {
      if !current.is_empty() {
        words.push(std::mem::take(&mut current));
      }
      previous_lower = false;
      continue;
    }
    if c.is_uppercase() && previous_lower && !current.is_empty() {
      words.push(std::mem::take(&mut current));
    }
    previous_lower = c.is_lowercase() || c.is_ascii_digit();
    current.extend(c.to_lowercase());
  }
  if !current.is_empty() {
    words.push(current);
  }
  words
    .into_iter()
    .map(|word| word.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned())
    .filter(|word| !word.is_empty())
    .collect()
}

fn topic_names(method: &str, first: Option<&ArgSummary>) -> Vec<String> {
  match first {
    Some(ArgSummary::Str(name)) if !name.is_empty() => vec![name.clone()],
    Some(ArgSummary::Object(entries)) if matches!(method, "send" | "subscribe") => entries
      .iter()
      .filter_map(|entry| match (entry.key.static_name(), &entry.value) {
        (Some("topic"), ArgSummary::Str(topic)) => Some(vec![topic.clone()]),
        (Some("topics"), ArgSummary::Array(topics)) => Some(
          topics
            .iter()
            .filter_map(|topic| match topic {
              ArgSummary::Str(topic) => Some(topic.clone()),
              _ => None,
            })
            .collect(),
        ),
        _ => None,
      })
      .flatten()
      .collect(),
    _ => Vec::new(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_match_bus_words_only_as_the_trailing_identifier_word() {
    for name in [
      "events",
      "orderEvents",
      "eventEmitter",
      "EventEmitter2",
      "MessageQueue",
      "event_bus",
      "kafkaProducer",
      "pubSub",
      "PUBSUB",
    ] {
      assert!(is_bus_word(name), "{name}");
    }
    for name in ["busStop", "cabus", "queueItems", "eventsCount", "stream", "socket", "subscriber_list"] {
      assert!(!is_bus_word(name), "{name}");
    }
  }
}
