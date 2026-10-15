use std::collections::BTreeSet;
use std::sync::LazyLock;

use atlas_core::{ContentHash, HttpMethod, ImportFact};
use serde::Serialize;
use thiserror::Error;
use toml::Value;

const SCHEMA_VERSION: i64 = 1;
const MAX_NAME_LEN: usize = 64;
const BUILT_IN_FRAMEWORKS: [&str; 12] = [
  "nestjs",
  "express",
  "fastify",
  "koa",
  "axum",
  "actix",
  "rocket",
  "fetch",
  "axios",
  "nest_http",
  "openapi-fetch",
  "reqwest",
];
const DEFAULT_PATH_KEY: &str = "path";
const CLIENT_METHODS: [HttpMethod; 7] = [
  HttpMethod::Get,
  HttpMethod::Post,
  HttpMethod::Put,
  HttpMethod::Patch,
  HttpMethod::Delete,
  HttpMethod::Head,
  HttpMethod::Options,
];
const SERVER_SECTIONS: [&str; 7] = ["packages", "controller", "routes", "guards", "permissions", "params", "global_prefix"];
const AUTH_SCHEMES: [&str; 4] = ["bearer", "api_key", "session_cookie", "basic"];
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GuardScheme {
  pub decorator: String,
  pub scheme: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PathStyle {
  Openapi,
  Express,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientExports {
  All,
  Only(Vec<String>),
  None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClientMethod {
  pub name: String,
  pub method: HttpMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClientShape {
  pub modules: Vec<String>,
  pub files: Vec<String>,
  pub exports: ClientExports,
  pub factories: Vec<String>,
  pub methods: Vec<ClientMethod>,
  pub path: PathArgument,
  pub style: PathStyle,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub target_package: Option<String>,
}

impl ClientShape {
  pub(crate) fn method(&self, name: &str) -> Option<HttpMethod> {
    self.methods.iter().find(|method| method.name == name).map(|method| method.method)
  }

  pub(crate) fn is_client_export(&self, imported: &str) -> bool {
    if self.factories.iter().any(|factory| factory == imported) {
      return false;
    }
    match &self.exports {
      ClientExports::All => true,
      ClientExports::Only(names) => names.iter().any(|name| name == imported),
      ClientExports::None => false,
    }
  }

  pub(crate) fn is_factory(&self, imported: &str) -> bool {
    self.factories.iter().any(|factory| factory == imported)
  }
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
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub(crate) guard_schemes: Vec<GuardScheme>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub(crate) permissions: Vec<String>,
  pub(crate) params: ParamDecorators,
  pub(crate) global_prefix: Option<GlobalPrefix>,
  pub(crate) origin: FamilyOrigin,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) client: Option<ClientShape>,
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
  guard_schemes: Vec::new(),
  permissions: Vec::new(),
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
  client: None,
});

pub(crate) static OPENAPI_FETCH: LazyLock<ClientShape> = LazyLock::new(|| ClientShape {
  modules: vec!["openapi-fetch".to_owned()],
  files: Vec::new(),
  exports: ClientExports::None,
  factories: vec!["default".to_owned(), "createClient".to_owned()],
  methods: CLIENT_METHODS
    .iter()
    .map(|method| ClientMethod {
      name: method.as_str().to_owned(),
      method: *method,
    })
    .collect(),
  path: PathArgument { index: 0, key: None },
  style: PathStyle::Openapi,
  target_package: None,
});

impl DecoratorFamily {
  pub fn name(&self) -> &str {
    &self.name
  }

  pub fn has_routes(&self) -> bool {
    !self.routes.is_empty()
  }

  pub fn has_client(&self) -> bool {
    self.client.is_some()
  }

  pub(crate) fn client(&self) -> Option<&ClientShape> {
    self.client.as_ref()
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

  pub(crate) fn guard_scheme(&self, decorator: &str) -> Option<&str> {
    self.guard_schemes.iter().find(|guard| guard.decorator == decorator).map(|guard| guard.scheme.as_str())
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
    let known = ["version", "packages", "controller", "routes", "guards", "permissions", "params", "global_prefix", "client"];
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
    let client_section = section(table, "client")?;
    if let Some(client) = client_section
      && !SERVER_SECTIONS.iter().any(|name| table.contains_key(*name))
    {
      return Ok(client_only(name, self.client(client)?));
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
    let (guards, guard_schemes) = match section(table, "guards")? {
      None => (Vec::new(), Vec::new()),
      Some(guards) => self.guards(guards)?,
    };
    let permissions = match section(table, "permissions")? {
      None => Vec::new(),
      Some(permissions) => self.permissions(permissions)?,
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
    let client = client_section.map(|client| self.client(client)).transpose()?;
    Ok(DecoratorFamily {
      name: name.to_owned(),
      packages,
      scope: ImportScope::Packages,
      controllers,
      controller_path,
      routes,
      route_path,
      guards,
      guard_schemes,
      permissions,
      params,
      global_prefix,
      origin: FamilyOrigin::Declared,
      client,
    })
  }

  fn client(&mut self, client: &toml::Table) -> Result<ClientShape, FamilyError> {
    self.warn_unknown(
      "client.",
      client,
      &["modules", "files", "exports", "factories", "methods", "path", "style", "target_package"],
    );
    let modules = match client.get("modules") {
      None => Vec::new(),
      Some(value) => package_list(value, "client.modules")?,
    };
    let files = match client.get("files") {
      None => Vec::new(),
      Some(value) => file_globs(value, "client.files")?,
    };
    if modules.is_empty() && files.is_empty() {
      return Err(FamilyError::new("client", "name the modules a client is imported from in client.modules or client.files"));
    }
    let exports = match client.get("exports") {
      None => ClientExports::All,
      Some(value) => ClientExports::Only(identifiers(value, "client.exports")?),
    };
    let factories = match client.get("factories") {
      None => Vec::new(),
      Some(value) => identifiers(value, "client.factories")?,
    };
    let methods = match client.get("methods") {
      None => CLIENT_METHODS
        .iter()
        .flat_map(|method| [method.as_str().to_owned(), method.as_str().to_ascii_lowercase()].map(|name| ClientMethod { name, method: *method }))
        .collect(),
      Some(value) => client_methods(value)?,
    };
    let path = self.path_argument(client, "client.")?;
    let style = match client.get("style") {
      None => PathStyle::Openapi,
      Some(Value::String(style)) if style == "openapi" => PathStyle::Openapi,
      Some(Value::String(style)) if style == "express" => PathStyle::Express,
      Some(other) => return Err(FamilyError::new("client.style", format!("expected \"openapi\" or \"express\", got {other}"))),
    };
    let target_package = client.get("target_package").map(|value| package_name(value, "client.target_package")).transpose()?;
    Ok(ClientShape {
      modules,
      files,
      exports,
      factories,
      methods,
      path,
      style,
      target_package,
    })
  }

  fn guards(&mut self, guards: &toml::Table) -> Result<(Vec<String>, Vec<GuardScheme>), FamilyError> {
    self.warn_unknown("guards.", guards, &["decorators", "auth_schemes"]);
    let decorators = self.optional_decorators(guards, "decorators", "guards.")?;
    let schemes = match guards.get("auth_schemes") {
      None => return Ok((decorators, Vec::new())),
      Some(Value::Table(schemes)) => schemes,
      Some(other) => {
        return Err(FamilyError::new("guards.auth_schemes", format!("expected a table of decorator = \"scheme\", got {other}")));
      }
    };
    let mut guard_schemes = Vec::with_capacity(schemes.len());
    for (decorator, value) in schemes {
      let key = format!("guards.auth_schemes.{decorator}");
      if !decorators.contains(decorator) {
        return Err(FamilyError::new(key, "names a decorator that is not listed in guards.decorators"));
      }
      let scheme = value.as_str().filter(|scheme| AUTH_SCHEMES.contains(scheme)).ok_or_else(|| {
        FamilyError::new(
          &key,
          format!("expected one of {}, got {value}", AUTH_SCHEMES.map(|scheme| format!("\"{scheme}\"")).join(", ")),
        )
      })?;
      guard_schemes.push(GuardScheme {
        decorator: decorator.clone(),
        scheme: scheme.to_owned(),
      });
    }
    Ok((decorators, guard_schemes))
  }

  fn permissions(&mut self, permissions: &toml::Table) -> Result<Vec<String>, FamilyError> {
    self.warn_unknown("permissions.", permissions, &["decorators"]);
    self.decorator_list(required(permissions, "decorators", "permissions.")?, "permissions.decorators")
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

fn client_only(name: &str, client: ClientShape) -> DecoratorFamily {
  DecoratorFamily {
    name: name.to_owned(),
    packages: Vec::new(),
    scope: ImportScope::Packages,
    controllers: Vec::new(),
    controller_path: PathArgument {
      index: 0,
      key: Some(DEFAULT_PATH_KEY.to_owned()),
    },
    routes: Vec::new(),
    route_path: PathArgument {
      index: 0,
      key: Some(DEFAULT_PATH_KEY.to_owned()),
    },
    guards: Vec::new(),
    guard_schemes: Vec::new(),
    permissions: Vec::new(),
    params: ParamDecorators::default(),
    global_prefix: None,
    origin: FamilyOrigin::Declared,
    client: Some(client),
  }
}

fn package_name(value: &Value, key: &str) -> Result<String, FamilyError> {
  value
    .as_str()
    .filter(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
    .map(str::to_owned)
    .ok_or_else(|| FamilyError::new(key, format!("expected a package name such as \"@acme/books-api\", got {value}")))
}

fn client_methods(value: &Value) -> Result<Vec<ClientMethod>, FamilyError> {
  let names = strings(value, "client.methods")?;
  if names.is_empty() {
    return Err(FamilyError::new("client.methods", "must name at least one method, such as \"get\""));
  }
  names
    .into_iter()
    .map(|name| {
      let method = HttpMethod::parse_lenient(&name)
        .filter(|method| !matches!(method, HttpMethod::Unknown | HttpMethod::Any))
        .ok_or_else(|| FamilyError::new("client.methods", format!("expected HTTP method names such as \"get\" or \"GET\", got {name:?}")))?;
      Ok(ClientMethod { name, method })
    })
    .collect()
}

fn identifiers(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let names = strings(value, key)?;
  match names.iter().find(|name| !is_identifier(name)) {
    Some(name) => Err(FamilyError::new(key, format!("expected export names such as \"booksApi\" or \"default\", got {name:?}"))),
    None => Ok(names),
  }
}

fn file_globs(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let globs = strings(value, key)?;
  let invalid = globs
    .iter()
    .find(|glob| glob.is_empty() || glob.starts_with('/') || glob.starts_with("./") || glob.split('/').any(|segment| segment.is_empty() || segment == ".."));
  match invalid {
    Some(glob) => Err(FamilyError::new(
      key,
      format!("expected repository-relative paths such as \"apps/web/src/api/*.ts\", got {glob:?}"),
    )),
    None => Ok(globs),
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
    let text = format!("future = 1\n{ACME}[controller.path]\nargument = 0\nstyle = \"x\"\n[client]\nmodules = [\"@acme/books-client\"]\nbase = 1\n[server]\nport = 1\n");
    let warnings = parse(&text).unwrap().warnings;
    assert_eq!(
      warnings,
      [
        "unknown key future ignored",
        "unknown key server ignored",
        "unknown key controller.path.style ignored",
        "unknown key client.base ignored"
      ]
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
  fn should_parse_a_client_only_family_with_every_client_key() {
    let text = "[client]\nmodules = [\"@acme/books-client\"]\nfiles = [\"apps/web/src/api/*.ts\"]\nexports = [\"booksApi\", \"default\"]\nfactories = [\"createBooksClient\"]\nmethods = [\"get\", \"POST\"]\npath = { argument = 1 }\nstyle = \"express\"\ntarget_package = \"@acme/books-api\"\n";
    let parsed = parse(text).unwrap();
    let family = parsed.family;

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert!(!family.has_routes() && family.has_client());
    assert!(!family.applies_to(&[]));
    assert_eq!(
      family.client,
      Some(ClientShape {
        modules: vec!["@acme/books-client".to_owned()],
        files: vec!["apps/web/src/api/*.ts".to_owned()],
        exports: ClientExports::Only(vec!["booksApi".to_owned(), "default".to_owned()]),
        factories: vec!["createBooksClient".to_owned()],
        methods: vec![
          ClientMethod {
            name: "get".to_owned(),
            method: HttpMethod::Get
          },
          ClientMethod {
            name: "POST".to_owned(),
            method: HttpMethod::Post
          },
        ],
        path: PathArgument {
          index: 1,
          key: Some("path".to_owned())
        },
        style: PathStyle::Express,
        target_package: Some("@acme/books-api".to_owned()),
      })
    );
  }

  #[test]
  fn should_default_client_methods_to_both_cases_and_keep_a_server_side_optional() {
    let client = parse("[client]\nfiles = [\"src/api/**\"]\n").unwrap().family.client.unwrap();
    let names: Vec<&str> = client.methods.iter().map(|method| method.name.as_str()).collect();
    assert_eq!(
      names,
      [
        "GET", "get", "POST", "post", "PUT", "put", "PATCH", "patch", "DELETE", "delete", "HEAD", "head", "OPTIONS", "options"
      ]
    );
    assert_eq!((client.exports, client.style, client.target_package), (ClientExports::All, PathStyle::Openapi, None));
    let both = parse(&format!("{ACME}[client]\nmodules = [\"@acme/http/client\"]\n")).unwrap().family;
    assert!(both.has_routes() && both.has_client());
    assert_eq!(
      parse("packages = [\"@acme/http\"]\n[client]\nmodules = [\"x\"]\n"),
      Err("controller: missing; name the controller decorators".to_owned())
    );
  }

  #[test]
  fn should_reject_invalid_client_values_naming_the_key() {
    for (text, message) in [
      (
        "[client]\nexports = [\"api\"]\n",
        "client: name the modules a client is imported from in client.modules or client.files",
      ),
      ("[client]\nmodules = []\n", "client.modules: must name at least one package, such as \"@acme/http\""),
      (
        "[client]\nfiles = [\"../web/api.ts\"]\n",
        "client.files: expected repository-relative paths such as \"apps/web/src/api/*.ts\", got \"../web/api.ts\"",
      ),
      (
        "[client]\nfiles = [\"/abs/api.ts\"]\n",
        "client.files: expected repository-relative paths such as \"apps/web/src/api/*.ts\", got \"/abs/api.ts\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nmethods = [\"fetch\"]\n",
        "client.methods: expected HTTP method names such as \"get\" or \"GET\", got \"fetch\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nmethods = []\n",
        "client.methods: must name at least one method, such as \"get\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nstyle = \"rails\"\n",
        "client.style: expected \"openapi\" or \"express\", got \"rails\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nexports = [\"books.api\"]\n",
        "client.exports: expected export names such as \"booksApi\" or \"default\", got \"books.api\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\ntarget_package = \"\"\n",
        "client.target_package: expected a package name such as \"@acme/books-api\", got \"\"",
      ),
      ("client = 1\n", "client: must be a table"),
    ] {
      assert_eq!(parse(text), Err(message.to_owned()), "{text}");
    }
  }

  #[test]
  fn should_keep_the_digest_of_a_family_without_a_client_section() {
    let family = parse(ACME).unwrap().family;
    assert!(!serde_json::to_string(&family).unwrap().contains("client"));
    let with_client = parse(&format!("{ACME}[client]\nmodules = [\"@acme/http/client\"]\n")).unwrap().family;
    assert_ne!(families_digest(std::slice::from_ref(&family)), families_digest(&[with_client]));
  }

  #[test]
  fn should_parse_guard_auth_schemes_and_permission_decorators() {
    let text = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Guarded\", \"Signed\"]\nauth_schemes = { Signed = \"bearer\" }\n[permissions]\ndecorators = [\"Permit\", \"BotPermission\"]\n";
    let parsed = parse(text).unwrap();

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(parsed.family.guards, ["Guarded", "Signed"]);
    assert_eq!(
      parsed.family.guard_schemes,
      [GuardScheme {
        decorator: "Signed".to_owned(),
        scheme: "bearer".to_owned()
      }]
    );
    assert_eq!((parsed.family.guard_scheme("Signed"), parsed.family.guard_scheme("Guarded")), (Some("bearer"), None));
    assert_eq!(parsed.family.permissions, ["Permit", "BotPermission"]);
  }

  #[test]
  fn should_reject_invalid_guard_and_permission_sections_naming_the_key() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";
    for (extra, message) in [
      (
        "[guards]\ndecorators = [\"Guarded\"]\nauth_schemes = { Signed = \"bearer\" }\n",
        "guards.auth_schemes.Signed: names a decorator that is not listed in guards.decorators",
      ),
      (
        "[guards]\ndecorators = [\"Guarded\"]\nauth_schemes = { Guarded = \"oauth\" }\n",
        "guards.auth_schemes.Guarded: expected one of \"bearer\", \"api_key\", \"session_cookie\", \"basic\", got \"oauth\"",
      ),
      (
        "[guards]\ndecorators = [\"Guarded\"]\nauth_schemes = [\"bearer\"]\n",
        "guards.auth_schemes: expected a table of decorator = \"scheme\", got [\"bearer\"]",
      ),
      (
        "[guards]\ndecorators = [\"Guarded\"]\n[permissions]\ndecorators = [\"Guarded\"]\n",
        "permissions.decorators: Guarded is listed more than once; each decorator has one role",
      ),
      ("[permissions]\n", "permissions.decorators: missing"),
      ("[permissions]\ndecorators = []\n", "permissions.decorators: must name at least one decorator"),
    ] {
      assert_eq!(parse(&format!("{base}{extra}")), Err(message.to_owned()), "{extra}");
    }
    assert_eq!(parse(&format!("permissions = 1\n{base}")), Err("permissions: must be a table".to_owned()));
    assert_eq!(
      parse("[permissions]\ndecorators = [\"Permit\"]\n[client]\nmodules = [\"x\"]\n"),
      Err("packages: missing".to_owned())
    );
    let warnings = parse(&format!(
      "{base}[guards]\ndecorators = [\"Guarded\"]\nschemes = 1\n[permissions]\ndecorators = [\"Permit\"]\nconstants = 1\n"
    ))
    .unwrap()
    .warnings;
    assert_eq!(warnings, ["unknown key guards.schemes ignored", "unknown key permissions.constants ignored"]);
  }

  #[test]
  fn should_keep_the_digest_of_a_family_without_auth_schemes_or_permissions() {
    let family = parse(ACME).unwrap().family;
    let serialized = serde_json::to_string(&family).unwrap();
    assert!(!serialized.contains("guard_schemes") && !serialized.contains("permissions"), "{serialized}");
    let with_permissions = parse(&format!("{ACME}[permissions]\ndecorators = [\"Permit\"]\n")).unwrap().family;
    assert_ne!(families_digest(std::slice::from_ref(&family)), families_digest(&[with_permissions]));
  }

  #[test]
  fn should_reject_malformed_and_built_in_family_names() {
    assert_eq!(check_family_name("acme-http"), Ok(()));
    assert_eq!(check_family_name("nestjs"), Err("\"nestjs\" names a built-in framework; choose another name".to_owned()));
    assert_eq!(
      check_family_name("openapi-fetch"),
      Err("\"openapi-fetch\" names a built-in framework; choose another name".to_owned())
    );
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
