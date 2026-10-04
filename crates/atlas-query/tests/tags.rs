mod common;

use atlas_query::{
  ApiRequest, ChangesRequest, ContextRequest, ExplainRequest, ImpactRequest, PatternsRequest, QueryResult, ReadRequest, RenderText, Response, SearchRequest, SimilarRequest,
  SymbolRequest, TestsForRequest, TraversalKind, TraverseRequest, UnusedRequest, render_text,
};
use std::collections::{BTreeMap, BTreeSet};

use common::Fixture;
use serde::Serialize;
use serde_json::Value;

struct Rendered {
  name: String,
  text: String,
  json: Value,
}

fn rendered<T: RenderText + Serialize>(name: impl Into<String>, response: QueryResult<Response<T>>) -> Rendered {
  let response = response.unwrap();
  Rendered {
    name: name.into(),
    text: render_text(&response),
    json: serde_json::to_value(&response).unwrap(),
  }
}

fn traverse(fixture: &Fixture, kind: TraversalKind, target: &str) -> Rendered {
  let request = TraverseRequest {
    depth: 2,
    min_confidence: Some(0.0),
    ..TraverseRequest::new(target)
  };
  rendered(format!("{} {target}", kind.as_str()), fixture.query.traverse(kind, &request))
}

fn battery(fixture: &Fixture) -> Vec<Rendered> {
  let query = &fixture.query;
  let mut all = vec![
    rendered("overview", query.overview()),
    rendered("search", query.search(&SearchRequest::new("convert currency string to cents"))),
    rendered("search endpoint", query.search(&SearchRequest::new("orders"))),
    rendered(
      "api",
      query.api(&ApiRequest {
        min_confidence: Some(0.0),
        ..ApiRequest::default()
      }),
    ),
    rendered("unused", query.unused(&UnusedRequest::default())),
    rendered("patterns endpoint", query.patterns(&PatternsRequest::new("endpoint"))),
    rendered("patterns test", query.patterns(&PatternsRequest::new("test"))),
    rendered("context", query.context(&ContextRequest::new("add a DELETE /orders/:id endpoint with api-key auth"))),
    rendered("changes", query.changes(&ChangesRequest::default())),
    rendered("similar", query.similar(&SimilarRequest::new("toCents"))),
  ];
  for target in ["UsersService.findOne", "GET /orders/:id", "toCents", "UsersController"] {
    all.push(rendered(format!("symbol {target}"), query.symbol(&SymbolRequest::new(target))));
    all.push(rendered(format!("explain {target}"), query.explain(&ExplainRequest::new(target))));
    all.push(rendered(format!("read {target}"), query.read_symbol(&ReadRequest::new(target))));
    all.push(rendered(
      format!("impact {target}"),
      query.impact(&ImpactRequest {
        min_confidence: Some(0.0),
        ..ImpactRequest::new([target])
      }),
    ));
    all.push(rendered(format!("tests-for {target}"), query.tests_for(&TestsForRequest::new(target))));
    for kind in [TraversalKind::Callers, TraversalKind::Callees, TraversalKind::Dependents, TraversalKind::Dependencies] {
      all.push(traverse(fixture, kind, target));
    }
  }
  all
}

fn walk<'a>(value: &'a Value, found: &mut Vec<&'a serde_json::Map<String, Value>>) {
  match value {
    Value::Object(map) => {
      found.push(map);
      map.values().for_each(|child| walk(child, found));
    }
    Value::Array(items) => items.iter().for_each(|child| walk(child, found)),
    _ => {}
  }
}

fn tag_of(value: &Value) -> Option<&str> {
  let certainty = value.get("certainty")?.as_object()?;
  if certainty.get("authoritative")?.as_bool()? {
    return None;
  }
  certainty.get("tag")?.as_str()
}

const LISTING_PREFIXES: [&str; 11] = [
  "handler ",
  "guard ",
  "consumer ",
  "callers ",
  "auth used ",
  "permissions ",
  "via ",
  "implemented by ",
  "auth ",
  ", ",
  "used by ",
];

fn is_word_boundary(c: Option<char>) -> bool {
  c.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'))
}

fn listing_occurrences<'a>(line: &'a str, name: &str, phrases: &BTreeSet<String>) -> Vec<&'a str> {
  let mut after = Vec::new();
  for (start, _) in line.match_indices(name) {
    let end = start + name.len();
    if !is_word_boundary(line[..start].chars().next_back()) || !is_word_boundary(line[end..].chars().next()) {
      continue;
    }
    let before = &line[..start];
    if phrases.iter().any(|phrase| before.ends_with(&format!("{phrase} "))) {
      continue;
    }
    if before.trim().is_empty() || LISTING_PREFIXES.iter().any(|prefix| before.ends_with(prefix)) {
      after.push(&line[end..]);
    }
  }
  after
}

fn first_bracket_group(text: &str) -> Option<&str> {
  let open = text.find('[')?;
  let close = text[open..].find(']')? + open;
  Some(&text[open..=close])
}

const PROSE_PREFIXES: [&str; 5] = ["sig ", "doc ", "matched ", "why:", "attr "];

fn is_prose(line: &str) -> bool {
  let trimmed = line.trim_start();
  let numbered = trimmed
    .split_once('|')
    .is_some_and(|(number, _)| !number.trim().is_empty() && number.trim().chars().all(|c| c.is_ascii_digit()));
  numbered || PROSE_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
}

fn untagged_lines(output: &Rendered) -> Vec<String> {
  let mut objects = Vec::new();
  walk(&output.json, &mut objects);
  let mut tags: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
  let mut authoritative: BTreeSet<(String, String)> = BTreeSet::new();
  let mut hop_tags: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
  let mut phrases: BTreeSet<String> = BTreeSet::new();
  for map in &objects {
    let value = Value::Object((*map).clone());
    if let (Some(phrase), Some(entity)) = (map.get("phrase").and_then(Value::as_str), map.get("entity").and_then(Value::as_str)) {
      phrases.insert(phrase.to_owned());
      if let Some(tag) = tag_of(&value) {
        hop_tags.entry(format!("{phrase} {entity}")).or_default().insert(tag.to_owned());
      }
      continue;
    }
    if map.contains_key("owner") {
      continue;
    }
    let named = ["short", "event"].iter().find_map(|key| map.get(*key).and_then(Value::as_str));
    if let Some(name) = named {
      let kind = map.get("kind").and_then(Value::as_str).unwrap_or("event").to_owned();
      match tag_of(&value) {
        Some(tag) => tags.entry((name.to_owned(), kind)).or_default().insert(tag.to_owned()),
        None => authoritative.insert((name.to_owned(), kind)),
      };
    }
    for (key, by_name) in map.iter().filter(|(key, _)| key.ends_with("_certainty")) {
      let kind = key.trim_end_matches("_certainty").trim_end_matches('s').to_owned();
      for (name, certainty) in by_name.as_object().into_iter().flatten() {
        let tag = certainty.get("tag").and_then(Value::as_str).unwrap_or_default();
        tags.entry((name.clone(), kind.clone())).or_default().insert(tag.to_owned());
      }
    }
  }
  let required: BTreeMap<&str, BTreeSet<&str>> = tags
    .iter()
    .filter(|(entity, _)| !authoritative.contains(*entity))
    .fold(BTreeMap::new(), |mut by_name, ((name, _), found)| {
      by_name.entry(name.as_str()).or_default().extend(found.iter().map(String::as_str));
      by_name
    });
  let mut problems = Vec::new();
  for line in output.text.lines().filter(|line| !is_prose(line)) {
    for (name, expected) in &required {
      for after in listing_occurrences(line, name, &phrases) {
        if !first_bracket_group(after).is_some_and(|group| expected.contains(group)) {
          problems.push(format!("{name} should be followed by one of {expected:?}: {line}"));
        }
      }
    }
  }
  for (head, expected) in &hop_tags {
    let tagged = |line: &str| expected.iter().any(|tag| line.contains(&format!("{head} {tag}")));
    problems.extend(output.text.lines().filter(|line| line.contains(head.as_str()) && !tagged(line)).map(str::to_owned));
  }
  problems.sort();
  problems.dedup();
  problems
}

#[test]
fn should_tag_every_non_authoritative_entity_and_edge_in_every_query_text_over_acme() {
  let fixture = common::acme();

  let mut failures = Vec::new();
  let mut tagged_entities = 0;
  for output in battery(&fixture) {
    let mut objects = Vec::new();
    walk(&output.json, &mut objects);
    tagged_entities += objects
      .iter()
      .filter(|map| map.contains_key("short") && tag_of(&Value::Object((**map).clone())).is_some())
      .count();
    failures.extend(untagged_lines(&output).into_iter().map(|line| format!("{}: {line}", output.name)));
  }

  assert!(tagged_entities > 0, "acme must contain non-authoritative entities for this test to mean anything");
  assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn should_omit_the_entity_certainty_for_authoritative_entities_in_json() {
  let fixture = common::acme();

  let output = rendered("symbol", fixture.query.symbol(&SymbolRequest::new("UsersService.findOne")));

  assert!(output.json["answer"]["entity"].get("certainty").is_none(), "{:#}", output.json);
}
