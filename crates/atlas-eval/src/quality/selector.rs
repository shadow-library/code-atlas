use atlas_core::{Entity, EntityId, EntityKey, EntityKind, HttpMethod, LanguageFamily, RelPath, Visibility, normalize_route};
use serde::{Deserialize, Serialize};

use super::snapshot::Snapshot;
use super::truth::Selector;

pub fn anonymous_route(route: &str) -> String {
  normalize_route(route)
    .split('/')
    .map(|segment| match segment.starts_with('{') && segment.ends_with('}') {
      true => "{}",
      false => segment,
    })
    .collect::<Vec<_>>()
    .join("/")
}

pub fn client_target_matches(target: &str, route: &str) -> bool {
  let wanted = anonymous_route(route);
  let actual = anonymous_route(target);
  actual == wanted || actual.strip_prefix("/{}").is_some_and(|rest| format!("/{}", rest.trim_start_matches('/')) == wanted)
}

pub fn rust_file_module_depth(path: &str) -> usize {
  let segments: Vec<&str> = path.split('/').collect();
  let Some(src) = segments.iter().rposition(|segment| *segment == "src") else { return 0 };
  let Some((file, dirs)) = segments[src + 1..].split_last() else { return 0 };
  let stem = file.trim_end_matches(".rs");
  let is_root = matches!(stem, "lib" | "main" | "mod");
  match dirs.split_first() {
    Some((&"bin", [])) => 0,
    Some((&"bin", inside)) => (inside.len() - 1) + usize::from(!matches!(stem, "main" | "mod")),
    _ => dirs.len() + usize::from(!is_root),
  }
}

pub fn symbol_form(entity: &Entity) -> Option<String> {
  let EntityKey::Symbol(key) = &entity.key else { return None };
  let items = key.item_path();
  if items.is_empty() {
    return None;
  }
  if key.family() != LanguageFamily::Rust {
    return Some(items.join("."));
  }
  let depth = entity.path.as_ref().map_or(0, |path| rust_file_module_depth(path.as_str()));
  let inline_modules = key.module_path().get(depth..).unwrap_or_default();
  let path: Vec<&str> = inline_modules.iter().chain(items).map(String::as_str).collect();
  let trait_position = path
    .iter()
    .position(|segment| segment.starts_with('<') && segment.ends_with('>'))
    .filter(|position| *position > 0);
  Some(match trait_position {
    Some(position) => {
      let trait_name = &path[position][1..path[position].len() - 1];
      format!("<{} as {}>::{}", path[..position].join("::"), trait_name, path[position + 1..].join("::"))
    }
    None => path.join("::"),
  })
}

fn kind_alias(kind: &str) -> &str {
  match kind {
    "db_model" => "db_table",
    other => other,
  }
}

fn name_of(entity: &Entity) -> String {
  entity.key.display_name()
}

fn project_scoped(key: &EntityKey) -> bool {
  matches!(
    key,
    EntityKey::DbTable { .. } | EntityKey::Event { .. } | EntityKey::Queue { .. } | EntityKey::ExternalService { .. } | EntityKey::Capability { .. }
  )
}

fn repo_matches(entity: &Entity, repo: &str) -> bool {
  if project_scoped(&entity.key) {
    return true;
  }
  entity.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo) || entity.attrs.get("repo").and_then(serde_json::Value::as_str) == Some(repo)
}

pub fn matches(entity: &Entity, selector: &Selector) -> bool {
  if let Some(repo) = &selector.repo
    && !repo_matches(entity, repo)
  {
    return false;
  }
  if let Some(kind) = &selector.kind {
    if entity.kind.as_str() != kind_alias(kind) {
      return false;
    }
    return match kind.as_str() {
      "api_endpoint" => endpoint_matches(entity, selector),
      "auth_scheme" => selector.scheme.as_ref().is_none_or(|scheme| name_of(entity) == *scheme),
      _ => selector.name.as_ref().is_none_or(|name| name_of(entity) == *name),
    };
  }
  let Some(path) = &selector.path else { return false };
  let located = match &entity.key {
    EntityKey::File { path, .. } => Some(path.as_str()),
    _ => entity.path.as_ref().map(RelPath::as_str),
  };
  if located != Some(path.as_str()) {
    return false;
  }
  if let Some(suite) = &selector.suite {
    return entity.kind == EntityKind::TestSuite
      && matches!(&entity.key, EntityKey::Symbol(key) if key.item_path().last().is_some_and(|label| label == suite) || key.item_path().join(".") == *suite);
  }
  match &selector.symbol {
    Some(symbol) => symbol_matches(entity, symbol),
    None => matches!(entity.key, EntityKey::File { .. }),
  }
}

fn symbol_matches(entity: &Entity, symbol: &str) -> bool {
  if symbol_form(entity).is_some_and(|form| form == symbol) {
    return true;
  }
  symbol == "default" && matches!(entity.key, EntityKey::Symbol(_)) && entity.visibility == Some(Visibility::ExportedDefault)
}

fn endpoint_matches(entity: &Entity, selector: &Selector) -> bool {
  let EntityKey::Endpoint { method, route, .. } = &entity.key else { return false };
  let method_ok = selector.method.as_ref().is_none_or(|wanted| HttpMethod::parse_lenient(wanted) == Some(*method));
  let route_ok = selector.route.as_ref().is_none_or(|wanted| anonymous_route(wanted) == anonymous_route(route));
  method_ok && route_ok
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum BindingProblem {
  NoMatch { hint: String },
  Ambiguous { candidates: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingError {
  pub fixture: String,
  pub section: String,
  pub selector: String,
  #[serde(flatten)]
  pub problem: BindingProblem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
  One(EntityId),
  Many(Vec<EntityId>),
  Foreign,
  Failed(BindingProblem),
}

impl Binding {
  pub fn ids(&self) -> Vec<EntityId> {
    match self {
      Self::One(id) => vec![*id],
      Self::Many(ids) => ids.clone(),
      Self::Foreign | Self::Failed(_) => Vec::new(),
    }
  }
}

pub fn is_foreign(snapshot: &Snapshot, selector: &Selector) -> bool {
  selector.repo.as_ref().is_some_and(|repo| !snapshot.repos.contains(repo))
}

pub fn bind(snapshot: &Snapshot, selector: &Selector, expected_kind: Option<&str>) -> Binding {
  if is_foreign(snapshot, selector) {
    return Binding::Foreign;
  }
  let found: Vec<&Entity> = snapshot.entities.values().filter(|entity| matches(entity, selector)).collect();
  if selector.is_wildcard() {
    return Binding::Many(found.iter().map(|entity| entity.id).collect());
  }
  let narrowed = match (found.len(), expected_kind) {
    (count, Some(kind)) if count > 1 => {
      let same_kind: Vec<&Entity> = found.iter().copied().filter(|entity| entity.kind.as_str() == kind_alias(kind)).collect();
      if same_kind.is_empty() { found } else { same_kind }
    }
    _ => found,
  };
  match narrowed.as_slice() {
    [only] => Binding::One(only.id),
    [] => Binding::Failed(BindingProblem::NoMatch {
      hint: nearest_candidates(snapshot, selector),
    }),
    many => Binding::Failed(BindingProblem::Ambiguous {
      candidates: many.iter().map(|entity| entity.key.to_string()).collect(),
    }),
  }
}

fn nearest_candidates(snapshot: &Snapshot, selector: &Selector) -> String {
  let (Some(path), Some(symbol)) = (&selector.path, selector.symbol.as_ref().or(selector.suite.as_ref())) else {
    return "no entity of that shape exists in the index".to_owned();
  };
  let in_file: Vec<String> = snapshot
    .entities
    .values()
    .filter(|entity| entity.path.as_ref().map(|candidate| candidate.as_str()) == Some(path.as_str()) && matches!(entity.key, EntityKey::Symbol(_)))
    .filter_map(|entity| symbol_form(entity).map(|form| (form, entity.kind.as_str().to_owned())))
    .filter(|(form, _)| {
      let last = symbol.rsplit([':', '.']).next().unwrap_or(symbol);
      form.contains(last) || symbol.contains(form.rsplit([':', '.']).next().unwrap_or(form.as_str()))
    })
    .map(|(form, kind)| format!("{form} ({kind})"))
    .take(5)
    .collect();
  match in_file.is_empty() {
    true => format!("no symbol like `{symbol}` is indexed in {path}"),
    false => format!("similar symbols in {path}: {}", in_file.join(", ")),
  }
}

#[cfg(test)]
mod tests {
  use std::collections::{BTreeMap, BTreeSet};

  use atlas_core::{Certainty, EntityKey, LanguageFamily, RelPath, RepoName, Span, SymbolKey};

  use super::*;

  fn symbol(repo: &str, path: &str, family: LanguageFamily, items: &[&str], kind: EntityKind, disambiguator: u32) -> Entity {
    let module: Vec<String> = path
      .trim_start_matches("src/")
      .trim_end_matches(".ts")
      .trim_end_matches(".rs")
      .split('/')
      .map(str::to_owned)
      .collect();
    let key = SymbolKey::new(
      family,
      RepoName::new(repo).unwrap(),
      "pkg",
      module,
      items.iter().map(|item| (*item).to_owned()).collect(),
      kind,
      disambiguator,
    )
    .unwrap();
    Entity::new(EntityKey::Symbol(key), Certainty::Syntactic).located(RelPath::new(path).unwrap(), Span::from_zero_based_rows(0, 1, 0, 0, 0, 1))
  }

  fn endpoint(repo: &str, method: HttpMethod, route: &str) -> Entity {
    Entity::new(
      EntityKey::Endpoint {
        repo: RepoName::new(repo).unwrap(),
        method,
        route: route.to_owned(),
      },
      Certainty::Syntactic,
    )
  }

  fn snapshot(entities: Vec<Entity>) -> Snapshot {
    Snapshot::assemble(
      BTreeSet::from(["a".to_owned(), "b".to_owned()]),
      entities.into_iter().map(|entity| (entity.id, entity)).collect(),
      Vec::new(),
      Vec::new(),
      BTreeMap::new(),
      Vec::new(),
    )
  }

  fn symbol_selector(repo: &str, path: &str, name: &str) -> Selector {
    Selector {
      repo: Some(repo.into()),
      path: Some(path.into()),
      symbol: Some(name.into()),
      ..Selector::default()
    }
  }

  #[test]
  fn should_render_rust_trait_impl_methods_in_the_schema_form() {
    let entity = symbol("a", "src/store.rs", LanguageFamily::Rust, &["PgStore", "<InvoiceStore>", "insert"], EntityKind::Method, 0);
    assert_eq!(symbol_form(&entity).as_deref(), Some("<PgStore as InvoiceStore>::insert"));
    let inherent = symbol("a", "src/store.rs", LanguageFamily::Rust, &["PgStore", "new"], EntityKind::Method, 0);
    assert_eq!(symbol_form(&inherent).as_deref(), Some("PgStore::new"));
    let ts = symbol("a", "src/x.ts", LanguageFamily::Ts, &["UsersService", "findOne"], EntityKind::Method, 0);
    assert_eq!(symbol_form(&ts).as_deref(), Some("UsersService.findOne"));
  }

  #[test]
  fn should_count_the_file_module_depth_of_rust_sources() {
    assert_eq!(rust_file_module_depth("src/lib.rs"), 0);
    assert_eq!(rust_file_module_depth("src/main.rs"), 0);
    assert_eq!(rust_file_module_depth("src/handlers.rs"), 1);
    assert_eq!(rust_file_module_depth("src/a/mod.rs"), 1);
    assert_eq!(rust_file_module_depth("src/a/b.rs"), 2);
    assert_eq!(rust_file_module_depth("crate/src/lib.rs"), 0);
    assert_eq!(rust_file_module_depth("tests/invoices.rs"), 0);
    assert_eq!(rust_file_module_depth("build.rs"), 0);
    assert_eq!(rust_file_module_depth("src/bin/tool.rs"), 0);
    assert_eq!(rust_file_module_depth("src/bin/tool/helpers.rs"), 1);
  }

  #[test]
  fn should_include_inline_modules_in_rust_symbol_forms() {
    let mut entity = symbol("a", "src/consumer.rs", LanguageFamily::Rust, &["explicit_beats_glob"], EntityKind::Function, 0);
    let EntityKey::Symbol(key) = &entity.key else { panic!() };
    let nested = SymbolKey::new(
      LanguageFamily::Rust,
      RepoName::new("a").unwrap(),
      "pkg",
      vec!["consumer".into(), "glob_explicit".into()],
      key.item_path().to_vec(),
      EntityKind::Function,
      0,
    )
    .unwrap();
    entity.key = EntityKey::Symbol(nested);
    assert_eq!(symbol_form(&entity).as_deref(), Some("glob_explicit::explicit_beats_glob"));
  }

  #[test]
  fn should_bind_a_symbol_selector_to_exactly_one_entity() {
    let found = symbol("a", "src/x.ts", LanguageFamily::Ts, &["Users", "find"], EntityKind::Method, 0);
    let id = found.id;
    let snapshot = snapshot(vec![found, symbol("a", "src/y.ts", LanguageFamily::Ts, &["Users", "find"], EntityKind::Method, 0)]);
    assert_eq!(bind(&snapshot, &symbol_selector("a", "src/x.ts", "Users.find"), None), Binding::One(id));
  }

  #[test]
  fn should_report_an_ambiguity_when_two_entities_match_one_selector() {
    let snapshot = snapshot(vec![
      symbol("a", "src/x.ts", LanguageFamily::Ts, &["parse"], EntityKind::Function, 0),
      symbol("a", "src/x.ts", LanguageFamily::Ts, &["parse"], EntityKind::Function, 1),
    ]);
    let Binding::Failed(BindingProblem::Ambiguous { candidates }) = bind(&snapshot, &symbol_selector("a", "src/x.ts", "parse"), None) else {
      panic!("expected an ambiguity");
    };
    assert_eq!(candidates.len(), 2);
  }

  #[test]
  fn should_disambiguate_by_expected_kind_before_reporting_an_ambiguity() {
    let class = symbol("a", "src/x.ts", LanguageFamily::Ts, &["Thing"], EntityKind::Class, 0);
    let interface = symbol("a", "src/x.ts", LanguageFamily::Ts, &["Thing"], EntityKind::Interface, 0);
    let class_id = class.id;
    let snapshot = snapshot(vec![class, interface]);
    let selector = symbol_selector("a", "src/x.ts", "Thing");
    assert!(matches!(bind(&snapshot, &selector, None), Binding::Failed(BindingProblem::Ambiguous { .. })));
    assert_eq!(bind(&snapshot, &selector, Some("class")), Binding::One(class_id));
  }

  #[test]
  fn should_report_no_match_with_nearby_symbols() {
    let snapshot = snapshot(vec![symbol("a", "src/x.ts", LanguageFamily::Ts, &["Users", "findAll"], EntityKind::Method, 0)]);
    let Binding::Failed(BindingProblem::NoMatch { hint }) = bind(&snapshot, &symbol_selector("a", "src/x.ts", "Users.find"), None) else {
      panic!("expected no match");
    };
    assert!(hint.contains("Users.findAll"), "{hint}");
  }

  #[test]
  fn should_treat_selectors_naming_other_projects_as_foreign() {
    let snapshot = snapshot(Vec::new());
    assert_eq!(bind(&snapshot, &symbol_selector("elsewhere", "src/x.ts", "f"), None), Binding::Foreign);
  }

  #[test]
  fn should_match_endpoints_ignoring_parameter_names_and_normalising_routes_on_both_sides() {
    let users = endpoint("a", HttpMethod::Get, "/users/{userId}");
    let id = users.id;
    let snapshot = snapshot(vec![users, endpoint("a", HttpMethod::Post, "/users")]);
    let selector = Selector {
      kind: Some("api_endpoint".into()),
      repo: Some("a".into()),
      method: Some("GET".into()),
      route: Some("/users/:id".into()),
      ..Selector::default()
    };
    assert_eq!(bind(&snapshot, &selector, None), Binding::One(id));
    let wrong_method = Selector {
      method: Some("DELETE".into()),
      ..selector
    };
    assert!(matches!(bind(&snapshot, &wrong_method, None), Binding::Failed(BindingProblem::NoMatch { .. })));
  }

  #[test]
  fn should_bind_project_scoped_entities_even_when_the_selector_names_a_repo() {
    let table = Entity::new(EntityKey::DbTable { name: "users".into() }, Certainty::Syntactic);
    let event = Entity::new(EntityKey::Event { name: "order.created".into() }, Certainty::Syntactic);
    let (table_id, event_id) = (table.id, event.id);
    let snapshot = snapshot(vec![table, event]);
    let model = Selector {
      kind: Some("db_model".into()),
      repo: Some("a".into()),
      name: Some("users".into()),
      ..Selector::default()
    };
    let published = Selector {
      kind: Some("event".into()),
      repo: Some("b".into()),
      name: Some("order.created".into()),
      ..Selector::default()
    };
    assert_eq!(bind(&snapshot, &model, Some("db_model")), Binding::One(table_id));
    assert_eq!(bind(&snapshot, &published, None), Binding::One(event_id));
  }

  #[test]
  fn should_expand_wildcard_selectors_to_every_entity_of_the_kind() {
    let snapshot = snapshot(vec![
      endpoint("a", HttpMethod::Get, "/x"),
      endpoint("a", HttpMethod::Get, "/y"),
      symbol("a", "src/x.ts", LanguageFamily::Ts, &["f"], EntityKind::Function, 0),
    ]);
    let selector = Selector {
      kind: Some("api_endpoint".into()),
      ..Selector::default()
    };
    assert_eq!(bind(&snapshot, &selector, None).ids().len(), 2);
  }

  #[test]
  fn should_select_a_default_export_by_visibility() {
    let mut entity = symbol("a", "src/alpha.ts", LanguageFamily::Ts, &["alpha"], EntityKind::Function, 0);
    entity.visibility = Some(Visibility::ExportedDefault);
    let id = entity.id;
    let snapshot = snapshot(vec![entity]);
    assert_eq!(bind(&snapshot, &symbol_selector("a", "src/alpha.ts", "default"), None), Binding::One(id));
  }

  #[test]
  fn should_match_client_targets_with_an_unknown_base() {
    assert!(client_target_matches("{}/pets/{}", "/pets/:id"));
    assert!(client_target_matches("/api/items/{}", "/api/items/:id"));
    assert!(!client_target_matches("{}/pets/{}", "/dogs/:id"));
  }
}
