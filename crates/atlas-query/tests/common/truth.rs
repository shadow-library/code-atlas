use std::fs;

use atlas_query::EntityRef;
use toml::Value;

use super::fixtures_root;

pub fn expected(group: &str) -> Value {
  let text = fs::read_to_string(fixtures_root().join(group).join("expected.toml")).unwrap();
  Value::Table(toml::from_str::<toml::Table>(&text).unwrap())
}

pub fn section<'v>(truth: &'v Value, name: &str) -> &'v [Value] {
  truth.get(name).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn field<'v>(selector: &'v Value, name: &str) -> Option<&'v str> {
  selector.get(name).and_then(Value::as_str)
}

fn anonymous_route(route: &str) -> String {
  route
    .split('/')
    .map(|segment| match segment.starts_with(':') || (segment.starts_with('{') && segment.ends_with('}')) {
      true => "{}",
      false => segment,
    })
    .collect::<Vec<_>>()
    .join("/")
}

fn symbol_matches(short: &str, symbol: &str) -> bool {
  let normalised = match symbol.strip_prefix('<').and_then(|rest| rest.split_once(">::")) {
    Some((inner, tail)) => match inner.split_once(" as ") {
      Some((self_type, trait_name)) => format!("{self_type}::<{trait_name}>::{tail}"),
      None => symbol.to_owned(),
    },
    None => symbol.to_owned(),
  };
  if short == normalised {
    return true;
  }
  let segments: Vec<&str> = normalised.split("::").collect();
  (1..segments.len()).any(|start| short == segments[start..].join("::"))
}

pub fn matches(entity: &EntityRef, selector: &Value) -> bool {
  let location = entity.location.as_ref();
  let repo_ok = field(selector, "repo").is_none_or(|repo| location.is_some_and(|location| location.repo == repo) || entity.key.contains(&format!(":{repo}:")));
  if !repo_ok {
    return false;
  }
  if let Some(kind) = field(selector, "kind") {
    if entity.kind != kind {
      return false;
    }
    return match kind {
      "api_endpoint" => match (field(selector, "method"), field(selector, "route")) {
        (Some(method), Some(route)) => {
          let (entity_method, entity_route) = entity.short.split_once(' ').unwrap_or_default();
          entity_method == method && anonymous_route(entity_route) == anonymous_route(route)
        }
        _ => true,
      },
      "auth_scheme" => field(selector, "scheme").is_none_or(|scheme| entity.short == scheme),
      _ => field(selector, "name").is_none_or(|name| entity.short == name),
    };
  }
  let Some(location) = location else { return false };
  if field(selector, "path").is_some_and(|path| location.path != path) {
    return false;
  }
  if let Some(suite) = field(selector, "suite") {
    let suite_name = entity.short.rsplit('.').next().unwrap_or_default();
    let in_suite = entity.short.starts_with(&format!("{suite}.")) || entity.short.contains(&format!(".{suite}."));
    return (entity.kind == "test_suite" && suite_name == suite) || (entity.kind == "test" && in_suite);
  }
  match field(selector, "symbol") {
    Some(symbol) => symbol_matches(&entity.short, symbol),
    None => entity.kind == "file",
  }
}

pub fn any_matches<'e>(entities: impl IntoIterator<Item = &'e EntityRef>, selector: &Value) -> bool {
  entities.into_iter().any(|entity| matches(entity, selector))
}

pub fn label(selector: &Value) -> String {
  selector.to_string()
}
