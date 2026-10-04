use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use toml::Value as Toml;

use super::{Found, Run, Sandbox, entity_refs};

const AUTHORITATIVE: [&str; 3] = ["syntactic", "resolved", "compiler_verified"];

pub fn load(path: &Path) -> Toml {
  toml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

pub fn tables<'a>(document: &'a Toml, section: &str) -> Vec<&'a Toml> {
  document.get(section).and_then(Toml::as_array).map(|items| items.iter().collect()).unwrap_or_default()
}

fn text(value: &Toml, field: &str) -> Option<String> {
  value.get(field).and_then(Toml::as_str).map(str::to_owned)
}

fn strings(value: &Toml, field: &str) -> Vec<String> {
  value
    .get(field)
    .and_then(Toml::as_array)
    .map(|items| items.iter().filter_map(Toml::as_str).map(str::to_owned).collect())
    .unwrap_or_default()
}

fn number(value: &Toml, field: &str) -> Option<f64> {
  value
    .get(field)
    .and_then(|value| value.as_float().or_else(|| value.as_integer().map(|integer| integer as f64)))
}

pub fn braces(route: &str) -> String {
  route
    .split('/')
    .map(|segment| match segment.strip_prefix(':') {
      Some(name) => format!("{{{name}}}"),
      None => segment.to_owned(),
    })
    .collect::<Vec<_>>()
    .join("/")
}

#[derive(Debug, Clone)]
pub enum Selector {
  Symbol {
    repo: String,
    path: String,
    symbol: Option<String>,
    suite: Option<String>,
  },
  Endpoint {
    repo: Option<String>,
    method: Option<String>,
    route: Option<String>,
  },
  Named {
    kind: String,
    repo: Option<String>,
    name: String,
  },
  Kind {
    kind: String,
  },
}

impl Selector {
  pub fn parse(value: &Toml) -> Self {
    match (text(value, "kind"), text(value, "path")) {
      (_, Some(path)) => Self::Symbol {
        repo: text(value, "repo").unwrap(),
        path,
        symbol: text(value, "symbol"),
        suite: text(value, "suite"),
      },
      (Some(kind), None) if kind == "api_endpoint" => Self::Endpoint {
        repo: text(value, "repo"),
        method: text(value, "method"),
        route: text(value, "route").map(|route| braces(&route)),
      },
      (Some(kind), None) => match text(value, "name").or_else(|| text(value, "scheme")) {
        Some(name) => Self::Named {
          kind,
          repo: text(value, "repo"),
          name,
        },
        None => Self::Kind { kind },
      },
      (None, None) => panic!("unsupported selector {value}"),
    }
  }

  pub fn field(table: &Toml, field: &str) -> Self {
    Self::parse(table.get(field).unwrap_or_else(|| panic!("missing {field} in {table}")))
  }

  pub fn repo(&self) -> Option<&str> {
    match self {
      Self::Symbol { repo, .. } => Some(repo),
      Self::Endpoint { repo, .. } | Self::Named { repo, .. } => repo.as_deref(),
      Self::Kind { .. } => None,
    }
  }

  pub fn matches(&self, found: &Found) -> bool {
    match self {
      Self::Symbol { repo, path, symbol, suite } => {
        let located = found.repo.as_deref() == Some(repo) && found.path.as_deref() == Some(path);
        located
          && match (symbol, suite) {
            (_, Some(suite)) => found.kind == "test_suite" && found.short == *suite,
            (Some(symbol), None) => found.kind != "file" && same_symbol(&found.short, symbol),
            (None, None) => found.kind == "file",
          }
      }
      Self::Endpoint { repo, method, route } => {
        let (found_method, found_route) = found.short.split_once(' ').unwrap_or(("", found.short.as_str()));
        found.kind == "api_endpoint"
          && repo.as_ref().is_none_or(|repo| found.repo.as_ref() == Some(repo))
          && method.as_ref().is_none_or(|method| method == found_method)
          && route.as_ref().is_none_or(|route| route == found_route)
      }
      Self::Named { kind, repo, name } => found.kind == *kind && found.short == *name && repo.as_ref().is_none_or(|repo| found.repo.as_ref() == Some(repo)),
      Self::Kind { kind } => found.kind == *kind,
    }
  }

  pub fn describe(&self) -> String {
    match self {
      Self::Symbol { repo, path, symbol, suite } => format!("{repo}:{path}{}", symbol.as_ref().or(suite.as_ref()).map(|name| format!("#{name}")).unwrap_or_default()),
      Self::Endpoint { repo, method, route } => format!(
        "endpoint {} {} {}",
        repo.as_deref().unwrap_or("*"),
        method.as_deref().unwrap_or("*"),
        route.as_deref().unwrap_or("*")
      ),
      Self::Named { kind, repo, name } => format!("{kind} {}:{name}", repo.as_deref().unwrap_or("*")),
      Self::Kind { kind } => format!("any {kind}"),
    }
  }
}

fn item_path(symbol: &str) -> String {
  match symbol.strip_prefix('<').and_then(|rest| rest.split_once(">::")) {
    Some((inner, method)) => match inner.split_once(" as ") {
      Some((self_type, trait_name)) => format!("{self_type}::<{trait_name}>::{method}"),
      None => symbol.to_owned(),
    },
    None => symbol.replace('.', "::"),
  }
}

fn same_symbol(short: &str, symbol: &str) -> bool {
  let wanted = item_path(symbol);
  let short = item_path(short);
  short == wanted || wanted.ends_with(&format!("::{short}")) || short.ends_with(&format!("::{wanted}"))
}

#[derive(Debug, Clone)]
pub struct Relation {
  pub other: Found,
  pub kind: String,
  pub class: String,
  pub confidence: Option<f64>,
}

impl Relation {
  fn of(item: &Value) -> Option<Self> {
    let relation = &item["relation"];
    let certainty = &relation["certainty"];
    Some(Self {
      other: Found::of(&item["entity"])?,
      kind: relation["kind"].as_str()?.to_owned(),
      class: certainty["class"].as_str()?.to_owned(),
      confidence: certainty["confidence"].as_f64(),
    })
  }

  fn describe(&self) -> String {
    let confidence = self.confidence.map(|value| format!(" {value:.2}")).unwrap_or_default();
    format!(
      "{} -> {} {}:{} [{}{confidence}]",
      self.kind,
      self.other.short,
      self.other.repo.as_deref().unwrap_or("-"),
      self.other.path.as_deref().unwrap_or("-"),
      self.class
    )
  }
}

pub struct Atlas<'a> {
  pub sandbox: &'a Sandbox,
  pub cwd: PathBuf,
  pub project: String,
}

impl Atlas<'_> {
  pub fn run(&self, args: &[&str]) -> Run {
    let mut args = args.to_vec();
    args.extend(["--project", self.project.as_str()]);
    let run = self.sandbox.run(&self.cwd, &args);
    run.assert_no_crash();
    run
  }

  pub fn answer(&self, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    let run = self.run(&args);
    assert!([0, 3, 4].contains(&run.code), "{run}");
    run.answer()
  }

  pub fn dump(&self, repo: &str, path: &str) -> Option<Value> {
    let run = self.run(&["debug", "dump-file", &format!("{repo}:{path}")]);
    match run.code {
      4 => None,
      _ => Some(run.json()),
    }
  }

  pub fn diagnostics(&self, repo: &str, path: &str) -> String {
    let explained = self.run(&["query", "explain", &format!("{repo}:{path}")]);
    explained
      .stdout
      .lines()
      .filter(|line| line.trim_start().starts_with("attr detector_diagnostics:"))
      .collect::<Vec<_>>()
      .join("\n")
  }

  pub fn endpoints(&self) -> Vec<Value> {
    self.answer(&["query", "api", "--limit", "1000", "--min-confidence", "0"])["endpoints"]["items"]
      .as_array()
      .cloned()
      .unwrap_or_default()
  }

  pub fn resolve(&self, selector: &Selector) -> Option<Found> {
    match selector {
      Selector::Symbol { repo, path, symbol, suite } => self.resolve_symbol(repo, path, symbol.as_deref(), suite.as_deref()),
      Selector::Endpoint { .. } => self
        .endpoints()
        .iter()
        .filter_map(|item| Found::of(&item["endpoint"]))
        .find(|found| selector.matches(found)),
      Selector::Named { kind, repo, name } => {
        let mut args = vec!["query", "symbol", name.as_str(), "--kind", kind.as_str()];
        if let Some(repo) = repo {
          args.extend(["--repo", repo.as_str()]);
        }
        let answer = self.answer(&args);
        entity_refs(&answer).into_iter().find(|found| selector.matches(found))
      }
      Selector::Kind { .. } => None,
    }
  }

  fn resolve_symbol(&self, repo: &str, path: &str, symbol: Option<&str>, suite: Option<&str>) -> Option<Found> {
    let dump = self.dump(repo, path)?;
    let entities: Vec<(String, Found)> = dump["entities"]
      .as_array()?
      .iter()
      .filter_map(|entity| {
        let found = Found {
          id: entity["id"].as_str()?.to_owned(),
          short: entity["name"].as_str()?.to_owned(),
          kind: entity["kind"].as_str()?.to_owned(),
          repo: Some(repo.to_owned()),
          path: Some(path.to_owned()),
        };
        Some((entity["key"].as_str()?.to_owned(), found))
      })
      .collect();
    let wanted = match (symbol, suite) {
      (None, None) => return entities.into_iter().map(|(_, found)| found).find(|found| found.kind == "file"),
      (_, Some(suite)) => (item_path(suite), Some("test_suite")),
      (Some(symbol), None) => (item_path(symbol), None),
    };
    let by_key: Vec<Found> = entities
      .iter()
      .filter(|(key, found)| {
        let item = key.split('#').next().unwrap_or(key).replace('/', "::");
        let kind_ok = wanted.1.is_none_or(|kind| found.kind == kind) && found.kind != "file";
        kind_ok && (item == wanted.0 || item.ends_with(&format!(":{}", wanted.0)))
      })
      .map(|(_, found)| found.clone())
      .collect();
    if let Some(found) = by_key.iter().find(|found| !matches!(found.kind.as_str(), "test" | "test_suite")).or(by_key.first()) {
      return Some(found.clone());
    }
    let code: Vec<Found> = entities.into_iter().map(|(_, found)| found).filter(|found| found.kind != "file").collect();
    match (symbol, code.as_slice()) {
      (Some("default"), [only]) => Some(only.clone()),
      _ => None,
    }
  }

  pub fn relations(&self, of: &Found, direction: &str) -> Vec<Relation> {
    let answer = self.answer(&["query", direction, &of.id, "--depth", "1", "--limit", "1000", "--min-confidence", "0"]);
    answer["groups"]
      .as_array()
      .into_iter()
      .flatten()
      .flat_map(|group| group["items"].as_array().cloned().unwrap_or_default())
      .filter_map(|item| Relation::of(&item))
      .collect()
  }

  pub fn edges(&self, from: &Selector, to: &Selector, kind: &str) -> Result<Vec<Relation>, String> {
    let source = self.resolve(from).ok_or_else(|| format!("cannot resolve {}", from.describe()))?;
    if source.kind != "file" {
      return Ok(
        self
          .relations(&source, "dependencies")
          .into_iter()
          .filter(|relation| relation.kind == kind && to.matches(&relation.other))
          .collect(),
      );
    }
    if let Selector::Kind { kind } = to {
      return Err(format!("wildcard target {kind} from a file selector is not supported"));
    }
    let Some(target) = self.resolve(to) else { return Ok(Vec::new()) };
    Ok(
      self
        .relations(&target, "dependents")
        .into_iter()
        .filter(|relation| relation.kind == kind && relation.other.id == source.id)
        .map(|relation| Relation {
          other: target.clone(),
          ..relation
        })
        .collect(),
    )
  }
}

pub struct Report {
  pub violations: Vec<String>,
  pub notes: Vec<String>,
}

impl Report {
  pub fn assert_clean(&self, case: &str) {
    assert!(
      self.violations.is_empty(),
      "{case}: {} expectation(s) violated through the CLI:\n  {}\nnotes:\n  {}",
      self.violations.len(),
      self.violations.join("\n  "),
      self.notes.join("\n  ")
    );
  }
}

pub fn evaluate(atlas: &Atlas<'_>, expected: &Toml) -> Report {
  let mut report = new_report();
  check_entities(atlas, expected, &mut report);
  check_edges(atlas, expected, &mut report);
  check_forbidden(atlas, expected, &mut report);
  check_unresolved(atlas, expected, &mut report);
  check_similar(atlas, expected, &mut report);
  check_file_flags(atlas, expected, &mut report);
  check_unmatched_calls(atlas, expected, &mut report);
  check_not_unused(atlas, expected, &mut report);
  check_blast_radius(atlas, expected, &mut report);
  report
}

fn certainty_ok(table: &Toml, class: &str, confidence: Option<f64>) -> Result<(), String> {
  let allowed = strings(table, "certainty");
  if !allowed.is_empty() && !allowed.iter().any(|allowed| allowed == class) {
    return Err(format!("certainty {class} is not one of {allowed:?}"));
  }
  match (number(table, "min_confidence"), confidence) {
    (Some(minimum), Some(actual)) if matches!(class, "heuristic" | "inferred") && actual + 1e-9 < minimum => Err(format!("confidence {actual:.2} is below {minimum:.2}")),
    _ => Ok(()),
  }
}

fn check_entities(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for entity in tables(expected, "entity") {
    if entity.get("best_effort").and_then(Toml::as_bool) == Some(true) {
      continue;
    }
    let kind = text(entity, "kind").unwrap();
    if kind == "api_endpoint" {
      check_endpoint(atlas, entity, report);
      continue;
    }
    let Some(selector) = entity.get("sym").map(Selector::parse) else {
      report.notes.push(format!("entity {kind} without sym is not checked: {entity}"));
      continue;
    };
    match atlas.resolve(&selector) {
      None => report.violations.push(format!("entity {kind} {} is not visible", selector.describe())),
      Some(found) if found.kind != kind => report.violations.push(format!("entity {} has kind {}, expected {kind}", selector.describe(), found.kind)),
      Some(_) => {}
    }
  }
}

fn check_endpoint(atlas: &Atlas<'_>, entity: &Toml, report: &mut Report) {
  let selector = Selector::parse(entity);
  let endpoints = atlas.endpoints();
  let Some(item) = endpoints.iter().find(|item| Found::of(&item["endpoint"]).is_some_and(|found| selector.matches(&found))) else {
    report.violations.push(format!("{} is missing from `api`", selector.describe()));
    return;
  };
  let certainty = &item["certainty"];
  if let Err(problem) = certainty_ok(entity, certainty["class"].as_str().unwrap_or("?"), certainty["confidence"].as_f64()) {
    report.violations.push(format!("{}: {problem}", selector.describe()));
  }
  let Some(handler) = entity.get("handler").map(Selector::parse) else { return };
  match Found::of(&item["handler"]["entity"]) {
    Some(found) if handler.matches(&found) => {}
    other => report
      .violations
      .push(format!("{} handler is {other:?}, expected {}", selector.describe(), handler.describe())),
  }
}

fn check_edges(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for edge in tables(expected, "edge") {
    let kind = text(edge, "kind").unwrap();
    let (from, to) = (Selector::field(edge, "from"), Selector::field(edge, "to"));
    let required = edge.get("required").and_then(Toml::as_bool) != Some(false);
    let label = format!("{kind} {} -> {}", from.describe(), to.describe());
    let relations = match atlas.edges(&from, &to, &kind) {
      Ok(relations) => relations,
      Err(problem) => {
        report.violations.push(format!("{label}: {problem}"));
        continue;
      }
    };
    if relations.is_empty() && required {
      report.violations.push(format!("{label}: missing"));
    }
    for relation in &relations {
      if let Err(problem) = certainty_ok(edge, &relation.class, relation.confidence) {
        report.violations.push(format!("{label}: {problem}"));
      }
    }
  }
}

pub fn check_forbidden(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for edge in tables(expected, "forbidden_edge") {
    let kind = text(edge, "kind").unwrap();
    let (from, to) = (Selector::field(edge, "from"), Selector::field(edge, "to"));
    let restricted = strings(edge, "certainty");
    let label = format!("forbidden {kind} {} -> {}", from.describe(), to.describe());
    if kind == "equivalent_to" {
      check_no_equivalence(atlas, &from, &to, &restricted, &label, report);
      continue;
    }
    match atlas.edges(&from, &to, &kind) {
      Err(problem) => report.violations.push(format!("{label}: {problem}")),
      Ok(relations) => {
        for relation in relations.iter().filter(|relation| restricted.is_empty() || restricted.contains(&relation.class)) {
          report.violations.push(format!("{label}: present as {}", relation.describe()));
        }
      }
    }
  }
}

fn similar_members(atlas: &Atlas<'_>, of: &Found) -> Vec<Value> {
  atlas.answer(&["query", "similar", &of.id, "--limit", "200", "--threshold", "0"])["members"]
    .as_array()
    .cloned()
    .unwrap_or_default()
}

fn claimed_equivalence(member: &Value) -> Option<String> {
  let verdict = member["verdict"].as_str().unwrap_or_default();
  let claims = verdict.contains("equivalent") && !verdict.contains("not equivalent");
  claims.then(|| verdict.to_owned())
}

fn check_no_equivalence(atlas: &Atlas<'_>, from: &Selector, to: &Selector, restricted: &[String], label: &str, report: &mut Report) {
  let Some(source) = atlas.resolve(from) else {
    report.violations.push(format!("{label}: cannot resolve {}", from.describe()));
    return;
  };
  for member in similar_members(atlas, &source) {
    let Some(found) = Found::of(&member["entity"]) else { continue };
    if !to.matches(&found) {
      continue;
    }
    let Some(verdict) = claimed_equivalence(&member) else { continue };
    let class = if verdict.starts_with("inferred") { "inferred" } else { "deterministic" };
    let forbidden = restricted.is_empty() || (class == "deterministic" && restricted.iter().any(|allowed| AUTHORITATIVE.contains(&allowed.as_str()) || allowed == "heuristic"));
    if forbidden {
      report.violations.push(format!("{label}: presented as {verdict:?}"));
    }
  }
}

fn reason_matches(expected: &str, actual: &Value) -> bool {
  let reason = actual["reason"].as_str().unwrap_or_default();
  let detail = actual["detail"].as_str().unwrap_or_default();
  match expected {
    "macro_generated" | "generated_code" => reason == "unsupported" && detail == "macro_generated",
    other => reason == other,
  }
}

fn ref_kind_matches(expected: &str, reference: &Value) -> bool {
  let kind = reference["kind"].as_str().unwrap_or_default();
  let head = reference["path"][0].as_str().unwrap_or_default();
  match expected {
    "call" => matches!(kind, "call" | "method_call"),
    "reference" => matches!(kind, "value_ref" | "type_ref"),
    "require" => kind == "call" && head == "require",
    "import" => kind == "call" && head == "import",
    _ => false,
  }
}

fn names(reference: &Value, name: &str) -> bool {
  reference["path"]
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .any(|segment| segment == name || segment == format!("[{name}]"))
}

fn check_unresolved(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for unresolved in tables(expected, "unresolved") {
    let from = Selector::field(unresolved, "from");
    let ref_kind = text(unresolved, "ref_kind").unwrap();
    let reason = text(unresolved, "reason").unwrap();
    let name = text(unresolved, "name");
    let label = format!("unresolved {ref_kind} {} from {}", name.as_deref().unwrap_or("-"), from.describe());
    let Some(source) = atlas.resolve(&from) else {
      report.violations.push(format!("{label}: cannot resolve the source"));
      continue;
    };
    let tolerated = strings(unresolved, "unless_certainty");
    let heuristic_ok = text(unresolved, "max_certainty").as_deref() == Some("heuristic");
    let candidates: Vec<Selector> = unresolved
      .get("candidates")
      .and_then(Toml::as_array)
      .map(|items| items.iter().map(Selector::parse).collect())
      .unwrap_or_default();
    if source.kind != "file" {
      for relation in atlas.relations(&source, "dependencies") {
        let to_candidate = candidates.iter().any(|candidate| candidate.matches(&relation.other));
        let named = name.as_deref().is_some_and(|name| same_symbol(&relation.other.short, name));
        if !to_candidate && !named {
          continue;
        }
        let allowed = tolerated.contains(&relation.class) || (heuristic_ok && to_candidate && relation.class == "heuristic");
        if !allowed {
          report.violations.push(format!("{label}: linked as {}", relation.describe()));
        }
      }
    }
    let (repo, path) = (source.repo.clone().unwrap(), source.path.clone().unwrap());
    let diagnostics = atlas.diagnostics(&repo, &path);
    if ref_kind == "route" {
      if !diagnostics.contains("route-dynamic") {
        report.violations.push(format!("{label}: no route-dynamic diagnostic on {repo}:{path}"));
      }
      continue;
    }
    if !matches!(ref_kind.as_str(), "call" | "reference" | "require" | "import") {
      report.notes.push(format!(
        "{label}: {ref_kind} references have no user-visible unresolved record; only the absence of edges was checked"
      ));
      continue;
    }
    let dump = atlas.dump(&repo, &path).unwrap();
    let recorded = dump["unresolved"].as_array().into_iter().flatten().any(|reference| {
      reference["from"].as_str() == Some(source.id.as_str())
        && ref_kind_matches(&ref_kind, reference)
        && reason_matches(&reason, &reference["reason"])
        && name.as_deref().is_none_or(|name| names(reference, name))
    });
    let dynamic_url = reason == "dynamic" && diagnostics.contains("http.dynamic_url") && diagnostics.contains(&format!(" in {} ", source.short));
    if !recorded && !dynamic_url {
      report.violations.push(format!(
        "{label}: no unresolved {reason} record in `debug dump-file {repo}:{path}` and no matching diagnostic"
      ));
    }
  }
}

fn check_similar(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for pair in tables(expected, "similar") {
    if pair.get("deterministic_claim_allowed").and_then(Toml::as_bool) != Some(false) {
      continue;
    }
    let (a, b) = (Selector::field(pair, "a"), Selector::field(pair, "b"));
    let label = format!("similar {} ~ {}", a.describe(), b.describe());
    let (Some(left), Some(right)) = (atlas.resolve(&a), atlas.resolve(&b)) else {
      report.violations.push(format!("{label}: cannot resolve both symbols"));
      continue;
    };
    for (source, other) in [(&left, &b), (&right, &a)] {
      for member in similar_members(atlas, source) {
        let matches_other = Found::of(&member["entity"]).is_some_and(|found| other.matches(&found));
        if let (true, Some(verdict)) = (matches_other, claimed_equivalence(&member))
          && !verdict.starts_with("inferred")
        {
          report.violations.push(format!("{label}: deterministic output claims {verdict:?}"));
        }
      }
    }
  }
}

fn check_file_flags(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for flag in tables(expected, "file_flag") {
    let (repo, path, kind) = (text(flag, "repo").unwrap(), text(flag, "path").unwrap(), text(flag, "flag").unwrap());
    let label = format!("file_flag {kind} {repo}:{path}");
    let Some(dump) = atlas.dump(&repo, &path) else {
      report.violations.push(format!("{label}: file is not indexed"));
      continue;
    };
    let ok = match kind.as_str() {
      "partial_syntax" => dump["file"]["parse_status"]["status"] == "partial",
      "generated" => atlas.run(&["query", "explain", &format!("{repo}:{path}")]).stdout.contains("attr generated: true"),
      other => {
        report.notes.push(format!("{label}: unknown flag {other}"));
        true
      }
    };
    if !ok {
      report.violations.push(format!("{label}: not visible"));
    }
  }
}

fn check_unmatched_calls(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  let unmatched = tables(expected, "unmatched_call");
  if unmatched.is_empty() {
    return;
  }
  let answer = atlas.answer(&["query", "api", "--limit", "1000", "--min-confidence", "0"]);
  for call in unmatched {
    let from = Selector::field(call, "from");
    let method = text(call, "method").unwrap();
    let label = format!("unmatched {method} call from {}", from.describe());
    let listed =
      answer["unmatched_calls"]["items"].as_array().into_iter().flatten().any(|item| {
        Found::of(&item["caller"]["entity"]).is_some_and(|found| from.matches(&found)) && item["call"]["short"].as_str().is_some_and(|short| short.starts_with(&method))
      });
    if !listed {
      report.violations.push(format!("{label}: not listed under unmatched client call sites"));
    }
    if let Ok(relations) = atlas.edges(&from, &Selector::Kind { kind: "api_endpoint".to_owned() }, "calls_endpoint") {
      for relation in relations {
        report.violations.push(format!("{label}: linked as {}", relation.describe()));
      }
    }
  }
}

fn check_not_unused(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  let keep = tables(expected, "not_unused");
  if keep.is_empty() {
    return;
  }
  let answer = atlas.answer(&["query", "unused", "--limit", "1000"]);
  let likely = entity_refs(&answer["likely_unused"]);
  for entry in keep {
    let Some(selector) = entry.get("sym").map(Selector::parse) else { continue };
    let resolved = atlas.resolve(&selector);
    for found in &likely {
      if selector.matches(found) || resolved.as_ref().is_some_and(|resolved| resolved.id == found.id) {
        report.violations.push(format!("not_unused {} is reported as likely unused", selector.describe()));
      }
    }
  }
}

pub fn check_blast_radius(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  for radius in tables(expected, "blast_radius") {
    let change = Selector::field(radius, "change");
    let label = format!("blast radius of {}", change.describe());
    let Some(target) = atlas.resolve(&change) else {
      report.violations.push(format!("{label}: cannot resolve the change"));
      continue;
    };
    let answer = atlas.answer(&["query", "impact", &target.id, "--limit", "1000", "--min-confidence", "0"]);
    let reached: Vec<Found> = entity_refs(&answer).into_iter().filter(|found| found.id != target.id).collect();
    for wanted in radius.get("must_include").and_then(Toml::as_array).into_iter().flatten().map(Selector::parse) {
      if !reached.iter().any(|found| wanted.matches(found)) {
        report.violations.push(format!("{label}: missing {}", wanted.describe()));
      }
    }
    for unwanted in radius.get("must_not_include").and_then(Toml::as_array).into_iter().flatten().map(Selector::parse) {
      if let Some(found) = reached.iter().find(|found| unwanted.matches(found)) {
        report.violations.push(format!("{label}: includes {} ({})", unwanted.describe(), found.short));
      }
    }
  }
}

pub fn repo_set(values: &[String]) -> BTreeSet<String> {
  values.iter().cloned().collect()
}

pub fn check_auth(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  let answer = atlas.answer(&["query", "api", "--limit", "1000", "--min-confidence", "0"]);
  for auth in tables(expected, "auth") {
    let repo = text(auth, "api_repo").unwrap();
    let Some(matrix) = answer["auth_matrix"].as_array().into_iter().flatten().find(|matrix| matrix["repo"] == repo.as_str()) else {
      report.violations.push(format!("auth matrix for {repo} is missing"));
      continue;
    };
    let schemes: Vec<&Value> = matrix["schemes"].as_array().into_iter().flatten().collect();
    let scheme = |name: &str| schemes.iter().find(|scheme| scheme["scheme"]["short"] == name).copied();
    let supported: BTreeSet<String> = schemes
      .iter()
      .filter(|scheme| scheme["supported"]["endpoints"].as_u64().unwrap_or(0) > 0)
      .filter_map(|scheme| scheme["scheme"]["short"].as_str().map(str::to_owned))
      .collect();
    if supported != repo_set(&strings(auth, "supported")) {
      report
        .violations
        .push(format!("{repo}: supported schemes {supported:?}, expected {:?}", strings(auth, "supported")));
    }
    if let Some(used) = auth.get("used").and_then(Toml::as_table) {
      for (name, clients) in used {
        let wanted: BTreeSet<String> = clients.as_array().into_iter().flatten().filter_map(Toml::as_str).map(str::to_owned).collect();
        let actual: BTreeSet<String> = scheme(name)
          .map(|scheme| {
            scheme["used_by_repos"]
              .as_array()
              .into_iter()
              .flatten()
              .filter_map(Value::as_str)
              .map(str::to_owned)
              .collect()
          })
          .unwrap_or_default();
        if actual != wanted {
          report.violations.push(format!("{repo}: {name} used by {actual:?}, expected {wanted:?}"));
        }
      }
    }
    for name in strings(auth, "unused") {
      match scheme(&name) {
        Some(scheme) if scheme["used"] == "used" => report.violations.push(format!("{repo}: {name} is reported as used")),
        Some(_) => {}
        None => report.violations.push(format!("{repo}: {name} is missing from the auth matrix")),
      }
    }
  }
}

pub fn check_endpoint_auth(atlas: &Atlas<'_>, expected: &Toml, report: &mut Report) {
  let endpoints = atlas.endpoints();
  for rule in tables(expected, "endpoint_auth") {
    let method = text(rule, "method").unwrap();
    let route = braces(&text(rule, "route").unwrap());
    let label = format!("auth of {method} {route}");
    let Some(item) = endpoints.iter().find(|item| item["method"] == method.as_str() && item["route"] == route.as_str()) else {
      report.violations.push(format!("{label}: endpoint missing"));
      continue;
    };
    let auth: Vec<&Value> = item["auth"].as_array().into_iter().flatten().collect();
    let schemes: BTreeSet<String> = auth.iter().filter_map(|entry| entry["scheme"]["entity"]["short"].as_str().map(str::to_owned)).collect();
    if schemes != repo_set(&strings(rule, "schemes")) {
      report.violations.push(format!("{label}: schemes {schemes:?}, expected {:?}", strings(rule, "schemes")));
    }
    let mode = text(rule, "mode").unwrap();
    for entry in &auth {
      if entry["combinator"]["value"] != mode.as_str() {
        report.violations.push(format!("{label}: combinator {}, expected {mode}", entry["combinator"]["value"]));
      }
      let certainty = &entry["scheme"]["relation"]["certainty"];
      if let Err(problem) = certainty_ok(rule, certainty["class"].as_str().unwrap_or("?"), certainty["confidence"].as_f64()) {
        report.violations.push(format!("{label}: {problem}"));
      }
    }
    if let Some(permission) = text(rule, "permission") {
      let permissions: Vec<&str> = item["permissions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["entity"]["short"].as_str())
        .collect();
      if permissions != [permission.as_str()] {
        report.violations.push(format!("{label}: permissions {permissions:?}, expected [{permission}]"));
      }
    }
  }
}

pub fn new_report() -> Report {
  Report {
    violations: Vec::new(),
    notes: Vec::new(),
  }
}
