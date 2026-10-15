use std::collections::BTreeSet;
use std::sync::LazyLock;

use atlas_core::{ContentHash, HttpMethod, ImportFact};
use serde::Serialize;
use thiserror::Error;
use toml::Value;

const SCHEMA_VERSION: i64 = 1;
const MAX_NAME_LEN: usize = 64;
const BUILT_IN_FRAMEWORKS: [&str; 7] = ["nestjs", "express", "fastify", "koa", "axum", "actix", "rocket"];
const DEFAULT_PATH_KEY: &str = "path";
const NEST_ROUTES: [(&str, HttpMethod); 8] = [
  ("Get", HttpMethod::Get),
  ("Post", HttpMethod::Post),
  ("Put", HttpMethod::Put),
  ("Patch", HttpMethod::Patch),
  ("Delete", HttpMethod::Delete),
  ("All", HttpMethod::Any),
  ("Head", HttpMethod::Head),
  ("Options", HttpMethod::Options),
];

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{key}: {message}")]
pub struct FamilyError {
  pub key: String,
  pub message: String,
}

impl FamilyError {
  fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
    Self {
      key: key.into(),
      message: message.into(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportScope {
  Prefix(String),
  Packages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FamilyOrigin {
  BuiltIn,
  Declared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PathArgument {
  pub index: usize,
  pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RouteDecorator {
  pub decorator: String,
  pub method: HttpMethod,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct ParamDecorators {
  pub path: Vec<String>,
  pub headers: Vec<String>,
  pub body: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlobalPrefix {
  pub factory: Vec<String>,
  pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecoratorFamily {
  pub(crate) name: String,
  pub(crate) packages: Vec<String>,
  pub(crate) scope: ImportScope,
  pub(crate) controllers: Vec<String>,
  pub(crate) controller_path: PathArgument,
  pub(crate) routes: Vec<RouteDecorator>,
  pub(crate) route_path: PathArgument,
  pub(crate) guards: Vec<String>,
  pub(crate) params: ParamDecorators,
  pub(crate) global_prefix: Option<GlobalPrefix>,
  pub(crate) origin: FamilyOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFamily {
  pub family: DecoratorFamily,
  pub warnings: Vec<String>,
}

pub(crate) static NEST: LazyLock<DecoratorFamily> = LazyLock::new(|| DecoratorFamily {
  name: "nestjs".to_owned(),
  packages: vec!["@nestjs/common".to_owned()],
  scope: ImportScope::Prefix("@nestjs/".to_owned()),
  controllers: vec!["Controller".to_owned()],
  controller_path: PathArgument {
    index: 0,
    key: Some(DEFAULT_PATH_KEY.to_owned()),
  },
  routes: NEST_ROUTES
    .iter()
    .map(|(decorator, method)| RouteDecorator {
      decorator: (*decorator).to_owned(),
      method: *method,
    })
    .collect(),
  route_path: PathArgument { index: 0, key: None },
  guards: vec!["UseGuards".to_owned()],
  params: ParamDecorators {
    path: vec!["Param".to_owned()],
    headers: vec!["Headers".to_owned()],
    body: vec!["Body".to_owned()],
  },
  global_prefix: Some(GlobalPrefix {
    factory: vec!["NestFactory".to_owned(), "create".to_owned()],
    method: "setGlobalPrefix".to_owned(),
  }),
  origin: FamilyOrigin::BuiltIn,
});

impl DecoratorFamily {
  pub fn name(&self) -> &str {
    &self.name
  }

  pub fn parse(name: &str, text: &str) -> Result<ParsedFamily, FamilyError> {
    check_family_name(name).map_err(|message| FamilyError::new("name", message))?;
    let table: toml::Table = toml::from_str(text).map_err(|error| FamilyError::new("syntax", crate::toml_error_summary(text, &error)))?;
    let mut reader = Reader::default();
    let family = reader.family(name, &table)?;
    Ok(ParsedFamily {
      family,
      warnings: reader.warnings,
    })
  }

  pub(crate) fn applies_to(&self, imports: &[ImportFact]) -> bool {
    imports.iter().any(|import| {
      let ImportFact::TsImport { specifier, .. } = import else { return false };
      match &self.scope {
        ImportScope::Prefix(prefix) => specifier.starts_with(prefix.as_str()),
        ImportScope::Packages => self.packages.iter().any(|package| is_package_or_subpath(specifier, package)),
      }
    })
  }

  pub(crate) fn dynamic_route_rule(&self) -> &'static str {
    match self.origin {
      FamilyOrigin::BuiltIn => "nest-dynamic-route",
      FamilyOrigin::Declared => "declared-dynamic-route",
    }
  }

  pub(crate) fn exports(&self, specifier: &str) -> bool {
    self.packages.iter().any(|package| package == specifier)
  }
}

pub fn check_family_name(name: &str) -> Result<(), String> {
  let mut characters = name.chars();
  let well_formed = characters.next().is_some_and(|first| first.is_ascii_lowercase())
    && characters.all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_'))
    && name.len() <= MAX_NAME_LEN;
  if !well_formed {
    return Err(format!(
      "a route family name is a lowercase letter followed by lowercase letters, digits, - or _ (at most {MAX_NAME_LEN} characters), got {name:?}"
    ));
  }
  match BUILT_IN_FRAMEWORKS.contains(&name) {
    true => Err(format!("{name:?} names a built-in framework; choose another name")),
    false => Ok(()),
  }
}

pub fn families_digest(families: &[DecoratorFamily]) -> ContentHash {
  ContentHash::of(serde_json::to_string(families).unwrap_or_default().as_bytes())
}

fn is_package_or_subpath(specifier: &str, package: &str) -> bool {
  specifier.strip_prefix(package).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[derive(Default)]
struct Reader {
  warnings: Vec<String>,
  decorators: BTreeSet<String>,
}

impl Reader {
  fn family(&mut self, name: &str, table: &toml::Table) -> Result<DecoratorFamily, FamilyError> {
    let known = ["version", "packages", "controller", "routes", "guards", "params", "global_prefix"];
    self.warn_unknown("", table, &known);
    match table.get("version") {
      None | Some(Value::Integer(SCHEMA_VERSION)) => {}
      Some(other) => {
        return Err(FamilyError::new(
          "version",
          format!("unsupported version {other}; this catl reads version {SCHEMA_VERSION}"),
        ));
      }
    }
    let packages = required(table, "packages", "")?;
    let packages = package_list(packages, "packages")?;
    let controller = section(table, "controller")?.ok_or_else(|| FamilyError::new("controller", "missing; name the controller decorators"))?;
    self.warn_unknown("controller.", controller, &["decorators", "path"]);
    let controllers = self.decorator_list(required(controller, "decorators", "controller.")?, "controller.decorators")?;
    let controller_path = self.path_argument(controller, "controller.")?;
    let routes_section = section(table, "routes")?.ok_or_else(|| FamilyError::new("routes", "missing; map route decorators to HTTP methods under [routes.methods]"))?;
    self.warn_unknown("routes.", routes_section, &["methods", "path"]);
    let routes = self.route_decorators(routes_section)?;
    let route_path = self.path_argument(routes_section, "routes.")?;
    let guards = match section(table, "guards")? {
      None => Vec::new(),
      Some(guards) => {
        self.warn_unknown("guards.", guards, &["decorators"]);
        self.optional_decorators(guards, "decorators", "guards.")?
      }
    };
    let params = match section(table, "params")? {
      None => ParamDecorators::default(),
      Some(params) => {
        self.warn_unknown("params.", params, &["path", "headers", "body"]);
        ParamDecorators {
          path: self.optional_decorators(params, "path", "params.")?,
          headers: self.optional_decorators(params, "headers", "params.")?,
          body: self.optional_decorators(params, "body", "params.")?,
        }
      }
    };
    let global_prefix = section(table, "global_prefix")?.map(|prefix| self.global_prefix(prefix)).transpose()?;
    Ok(DecoratorFamily {
      name: name.to_owned(),
      packages,
      scope: ImportScope::Packages,
      controllers,
      controller_path,
      routes,
      route_path,
      guards,
      params,
      global_prefix,
      origin: FamilyOrigin::Declared,
    })
  }

  fn warn_unknown(&mut self, prefix: &str, table: &toml::Table, known: &[&str]) {
    for name in table.keys().filter(|name| !known.contains(&name.as_str())) {
      self.warnings.push(format!("unknown key {prefix}{name} ignored"));
    }
  }

  fn route_decorators(&mut self, routes: &toml::Table) -> Result<Vec<RouteDecorator>, FamilyError> {
    let methods = match required(routes, "methods", "routes.")? {
      Value::Table(methods) if !methods.is_empty() => methods,
      Value::Table(_) => return Err(FamilyError::new("routes.methods", "must map at least one decorator to an HTTP method")),
      other => return Err(FamilyError::new("routes.methods", format!("expected a table of decorator = \"METHOD\", got {other}"))),
    };
    let mut decorators = Vec::with_capacity(methods.len());
    for (decorator, value) in methods {
      let key = format!("routes.methods.{decorator}");
      self.claim(decorator, &key)?;
      let method = value
        .as_str()
        .map(str::to_ascii_uppercase)
        .and_then(|method| method.parse::<HttpMethod>().ok())
        .filter(|method| *method != HttpMethod::Unknown)
        .ok_or_else(|| FamilyError::new(&key, format!("expected an HTTP method such as \"GET\", \"POST\" or \"ANY\", got {value}")))?;
      decorators.push(RouteDecorator {
        decorator: decorator.clone(),
        method,
      });
    }
    Ok(decorators)
  }

  fn path_argument(&mut self, slot: &toml::Table, prefix: &str) -> Result<PathArgument, FamilyError> {
    let default = PathArgument {
      index: 0,
      key: Some(DEFAULT_PATH_KEY.to_owned()),
    };
    let key = format!("{prefix}path");
    let path = match slot.get("path") {
      None => return Ok(default),
      Some(Value::Table(path)) => path,
      Some(other) => return Err(FamilyError::new(key, format!("expected a table such as {{ argument = 0, key = \"path\" }}, got {other}"))),
    };
    self.warn_unknown(&format!("{key}."), path, &["argument", "key"]);
    let index = match path.get("argument") {
      None => 0,
      Some(value) => value
        .as_integer()
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| FamilyError::new(format!("{key}.argument"), format!("expected a non-negative argument position, got {value}")))?,
    };
    let path_key = match path.get("key") {
      None => default.key,
      Some(value) => Some(identifier(value, &format!("{key}.key"))?),
    };
    Ok(PathArgument { index, key: path_key })
  }

  fn global_prefix(&mut self, prefix: &toml::Table) -> Result<GlobalPrefix, FamilyError> {
    self.warn_unknown("global_prefix.", prefix, &["factory", "method"]);
    let factory = required(prefix, "factory", "global_prefix.")?;
    let segments: Vec<String> = factory
      .as_str()
      .map(|text| text.split('.').map(str::to_owned).collect())
      .filter(|segments: &Vec<String>| segments.iter().all(|segment| is_identifier(segment)))
      .ok_or_else(|| FamilyError::new("global_prefix.factory", format!("expected a call path such as \"AppFactory.create\", got {factory}")))?;
    let method = identifier(required(prefix, "method", "global_prefix.")?, "global_prefix.method")?;
    Ok(GlobalPrefix { factory: segments, method })
  }

  fn optional_decorators(&mut self, table: &toml::Table, name: &str, prefix: &str) -> Result<Vec<String>, FamilyError> {
    match table.get(name) {
      None => Ok(Vec::new()),
      Some(value) => self.decorator_list(value, &format!("{prefix}{name}")),
    }
  }

  fn decorator_list(&mut self, value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
    let names = strings(value, key)?;
    if names.is_empty() {
      return Err(FamilyError::new(key, "must name at least one decorator"));
    }
    for name in &names {
      if !is_identifier(name) {
        return Err(FamilyError::new(key, format!("expected decorator names such as \"Controller\", got {name:?}")));
      }
      self.claim(name, key)?;
    }
    Ok(names)
  }

  fn claim(&mut self, decorator: &str, key: &str) -> Result<(), FamilyError> {
    if !is_identifier(decorator) {
      return Err(FamilyError::new(key, format!("{decorator:?} is not a decorator name")));
    }
    match self.decorators.insert(decorator.to_owned()) {
      true => Ok(()),
      false => Err(FamilyError::new(key, format!("{decorator} is listed more than once; each decorator has one role"))),
    }
  }
}

fn required<'t>(table: &'t toml::Table, name: &str, prefix: &str) -> Result<&'t Value, FamilyError> {
  table.get(name).ok_or_else(|| FamilyError::new(format!("{prefix}{name}"), "missing"))
}

fn section<'t>(table: &'t toml::Table, name: &str) -> Result<Option<&'t toml::Table>, FamilyError> {
  match table.get(name) {
    None => Ok(None),
    Some(Value::Table(section)) => Ok(Some(section)),
    Some(_) => Err(FamilyError::new(name, "must be a table")),
  }
}

fn strings(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let invalid = || FamilyError::new(key, format!("expected an array of strings, got {value}"));
  value
    .as_array()
    .ok_or_else(invalid)?
    .iter()
    .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
    .collect()
}

fn package_list(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let packages = strings(value, key)?;
  if packages.is_empty() {
    return Err(FamilyError::new(key, "must name at least one package, such as \"@acme/http\""));
  }
  if let Some(package) = packages
    .iter()
    .find(|package| package.is_empty() || package.chars().any(char::is_whitespace) || package.ends_with('/'))
  {
    return Err(FamilyError::new(key, format!("expected import specifiers such as \"@acme/http\", got {package:?}")));
  }
  Ok(packages)
}

fn identifier(value: &Value, key: &str) -> Result<String, FamilyError> {
  value
    .as_str()
    .filter(|text| is_identifier(text))
    .map(str::to_owned)
    .ok_or_else(|| FamilyError::new(key, format!("expected an identifier, got {value}")))
}

fn is_identifier(text: &str) -> bool {
  let mut characters = text.chars();
  characters.next().is_some_and(|first| first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
    && characters.all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
}

#[cfg(test)]
mod tests {
  use super::*;

  const ACME: &str = "version = 1\npackages = [\"@acme/http\"]\n\n[controller]\ndecorators = [\"Api\"]\n\n[routes.methods]\nRead = \"GET\"\nWrite = \"post\"\n\n[guards]\ndecorators = [\"Guarded\"]\n\n[params]\npath = [\"PathParam\"]\nheaders = [\"Header\"]\nbody = [\"Payload\"]\n\n[global_prefix]\nfactory = \"AcmeFactory.create\"\nmethod = \"setGlobalPrefix\"\n";

  fn parse(text: &str) -> Result<ParsedFamily, String> {
    DecoratorFamily::parse("acme-http", text).map_err(|error| error.to_string())
  }

  #[test]
  fn should_parse_every_section_of_a_family_file() {
    let parsed = parse(ACME).unwrap();
    let family = parsed.family;

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!((family.name.as_str(), family.packages.as_slice()), ("acme-http", ["@acme/http".to_owned()].as_slice()));
    assert_eq!(family.controllers, ["Api"]);
    assert_eq!(
      family.controller_path,
      PathArgument {
        index: 0,
        key: Some("path".to_owned())
      }
    );
    assert_eq!(
      family.routes,
      [
        RouteDecorator {
          decorator: "Read".to_owned(),
          method: HttpMethod::Get
        },
        RouteDecorator {
          decorator: "Write".to_owned(),
          method: HttpMethod::Post
        }
      ]
    );
    assert_eq!(family.guards, ["Guarded"]);
    assert_eq!(
      family.params,
      ParamDecorators {
        path: vec!["PathParam".to_owned()],
        headers: vec!["Header".to_owned()],
        body: vec!["Payload".to_owned()],
      }
    );
    assert_eq!(
      family.global_prefix,
      Some(GlobalPrefix {
        factory: vec!["AcmeFactory".to_owned(), "create".to_owned()],
        method: "setGlobalPrefix".to_owned(),
      })
    );
  }

  #[test]
  fn should_read_custom_path_arguments_and_keys() {
    let text = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\npath = { argument = 1, key = \"prefix\" }\n[routes]\npath = { argument = 0 }\n[routes.methods]\nRead = \"GET\"\n";
    let family = parse(text).unwrap().family;
    assert_eq!(
      (family.controller_path, family.route_path),
      (
        PathArgument {
          index: 1,
          key: Some("prefix".to_owned())
        },
        PathArgument {
          index: 0,
          key: Some("path".to_owned())
        }
      )
    );
  }

  #[test]
  fn should_warn_about_unknown_keys_at_every_level() {
    let text = format!("future = 1\n{ACME}[controller.path]\nargument = 0\nstyle = \"x\"\n[client]\nbase = 1\n");
    let warnings = parse(&text).unwrap().warnings;
    assert_eq!(
      warnings,
      ["unknown key client ignored", "unknown key future ignored", "unknown key controller.path.style ignored"]
    );
  }

  #[test]
  fn should_reject_invalid_values_naming_the_key() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";
    for (text, message) in [
      ("version = 2\n", "version: unsupported version 2; this catl reads version 1"),
      ("[controller]\ndecorators = [\"Api\"]\n", "packages: missing"),
      ("packages = []\n", "packages: must name at least one package, such as \"@acme/http\""),
      (
        "packages = [\"@acme/ http\"]\n",
        "packages: expected import specifiers such as \"@acme/http\", got \"@acme/ http\"",
      ),
      ("packages = [\"@acme/http\"]\n", "controller: missing; name the controller decorators"),
      ("packages = [\"@acme/http\"]\ncontroller = 1\n", "controller: must be a table"),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n",
        "routes: missing; map route decorators to HTTP methods under [routes.methods]",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"FETCH\"\n",
        "routes.methods.Read: expected an HTTP method such as \"GET\", \"POST\" or \"ANY\", got \"FETCH\"",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\n",
        "routes.methods: must map at least one decorator to an HTTP method",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nApi = \"GET\"\n",
        "routes.methods.Api: Api is listed more than once; each decorator has one role",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"api.v1\"]\n",
        "controller.decorators: expected decorator names such as \"Controller\", got \"api.v1\"",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\npath = { argument = -1 }\n",
        "controller.path.argument: expected a non-negative argument position, got -1",
      ),
    ] {
      assert_eq!(parse(text), Err(message.to_owned()), "{text}");
    }
    for (extra, message) in [
      ("[guards]\ndecorators = \"Guarded\"\n", "guards.decorators: expected an array of strings, got \"Guarded\""),
      (
        "[global_prefix]\nfactory = \"Acme Factory\"\nmethod = \"setGlobalPrefix\"\n",
        "global_prefix.factory: expected a call path such as \"AppFactory.create\", got \"Acme Factory\"",
      ),
      ("[global_prefix]\nfactory = \"AcmeFactory.create\"\n", "global_prefix.method: missing"),
    ] {
      assert_eq!(parse(&format!("{base}{extra}")), Err(message.to_owned()), "{extra}");
    }
    assert!(parse("packages = [").unwrap_err().starts_with("syntax: "));
  }

  #[test]
  fn should_reject_malformed_and_built_in_family_names() {
    assert_eq!(check_family_name("acme-http"), Ok(()));
    assert_eq!(check_family_name("nestjs"), Err("\"nestjs\" names a built-in framework; choose another name".to_owned()));
    assert!(check_family_name("Acme").is_err());
    assert!(check_family_name("9acme").is_err());
    assert!(check_family_name("").is_err());
    assert_eq!(DecoratorFamily::parse("express", ACME).map_err(|error| error.key), Err("name".to_owned()));
  }

  #[test]
  fn should_scope_declared_families_to_their_packages_and_subpaths() {
    let family = parse(ACME).unwrap().family;
    let import = |specifier: &str| ImportFact::TsImport {
      specifier: specifier.to_owned(),
      bindings: Vec::new(),
      type_only: false,
      span: atlas_core::Span::from_zero_based_rows(0, 0, 0, 0, 0, 0),
    };
    assert!(family.applies_to(&[import("@acme/http")]));
    assert!(family.applies_to(&[import("@acme/http/testing")]));
    assert!(!family.applies_to(&[import("@acme/http-client")]));
    assert!(!family.applies_to(&[import("@nestjs/common")]));
  }

  #[test]
  fn should_change_the_digest_with_any_definition_change() {
    let one = parse(ACME).unwrap().family;
    let mut other = one.clone();
    other.routes[0].method = HttpMethod::Put;
    assert_eq!(families_digest(std::slice::from_ref(&one)), families_digest(std::slice::from_ref(&one)));
    assert_ne!(families_digest(std::slice::from_ref(&one)), families_digest(&[other]));
  }

  #[test]
  fn should_not_echo_the_source_line_of_a_malformed_family_file() {
    let error = DecoratorFamily::parse("acme-http", "version = 1\ncontroller = sk-FAKEKEY123\n").unwrap_err();

    assert_eq!(error.key, "syntax");
    assert!(!error.message.contains("FAKEKEY"), "{}", error.message);
    assert!(error.message.starts_with("line 2, column 14: "), "{}", error.message);
  }
}
