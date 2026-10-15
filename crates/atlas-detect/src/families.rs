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
const SERVER_SECTIONS: [&str; 8] = ["packages", "controller", "routes", "guards", "permissions", "access", "params", "global_prefix"];
const AUTH_SCHEMES: [&str; 4] = ["bearer", "api_key", "session_cookie", "basic"];
pub(crate) const UNREAD_OPTIONS: &str = "unread";
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
  pub schemes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OptionKeys {
  pub decorator: String,
  pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OptionScheme {
  pub option: String,
  pub schemes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OptionSchemes {
  pub decorator: String,
  pub options: Vec<OptionScheme>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub default: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OptionMerge {
  #[default]
  Override,
  DeepMerge,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClassMarker {
  #[default]
  Contested,
  MethodWins,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Semantics {
  pub options: OptionMerge,
  pub class_marker: ClassMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DecoratorSemantics {
  pub decorator: String,
  pub semantics: Semantics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct GuardSemantics {
  pub default: Semantics,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub decorators: Vec<DecoratorSemantics>,
}

impl GuardSemantics {
  fn is_default(&self) -> bool {
    *self == Self::default()
  }

  pub(crate) fn of(&self, decorator: &str) -> Semantics {
    self
      .decorators
      .iter()
      .find(|entry| entry.decorator == decorator)
      .map_or(self.default, |entry| entry.semantics)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccessMode {
  Public,
  Optional,
}

impl AccessMode {
  pub(crate) const ALL: [AccessMode; 2] = [AccessMode::Public, AccessMode::Optional];

  pub(crate) fn as_str(self) -> &'static str {
    match self {
      Self::Public => "public",
      Self::Optional => "optional",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AccessOption {
  pub decorator: String,
  pub mode: AccessMode,
  pub keys: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct AccessMarkers {
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub public: Vec<String>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub optional: Vec<String>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub options: Vec<AccessOption>,
}

impl AccessMarkers {
  fn is_empty(&self) -> bool {
    self.public.is_empty() && self.optional.is_empty() && self.options.is_empty()
  }

  pub(crate) fn decorators(&self, mode: AccessMode) -> &[String] {
    match mode {
      AccessMode::Public => &self.public,
      AccessMode::Optional => &self.optional,
    }
  }
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
  #[serde(skip_serializing_if = "Option::is_none")]
  pub base: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub surfaces: Option<SurfaceMap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SurfaceMap {
  pub modules: Vec<String>,
  pub factory: String,
  pub option: String,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub containers: Vec<String>,
}

impl SurfaceMap {
  pub(crate) fn follows_members_of(&self, export: &str) -> bool {
    self.containers.is_empty() || self.containers.iter().any(|container| container == export)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RouteOptions {
  pub decorators: Vec<String>,
  pub method: String,
  pub path: String,
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

  pub(crate) fn surface_factory(&self, specifier: &str, imported: &str) -> Option<&SurfaceMap> {
    self
      .surfaces
      .as_ref()
      .filter(|surfaces| surfaces.factory == imported && surfaces.modules.iter().any(|module| module == specifier))
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
  pub(crate) guard_permissions: Vec<OptionKeys>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub(crate) option_schemes: Vec<OptionSchemes>,
  #[serde(skip_serializing_if = "GuardSemantics::is_default")]
  pub(crate) semantics: GuardSemantics,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub(crate) permissions: Vec<String>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub(crate) permission_schemes: Vec<GuardScheme>,
  #[serde(skip_serializing_if = "AccessMarkers::is_empty")]
  pub(crate) access: AccessMarkers,
  pub(crate) params: ParamDecorators,
  pub(crate) global_prefix: Option<GlobalPrefix>,
  pub(crate) origin: FamilyOrigin,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) client: Option<ClientShape>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) route_options: Option<RouteOptions>,
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
  guard_permissions: Vec::new(),
  option_schemes: Vec::new(),
  semantics: GuardSemantics::default(),
  permissions: Vec::new(),
  permission_schemes: Vec::new(),
  access: AccessMarkers::default(),
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
  route_options: None,
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
  base: None,
  surfaces: None,
});

impl DecoratorFamily {
  pub fn name(&self) -> &str {
    &self.name
  }

  pub fn has_routes(&self) -> bool {
    !self.routes.is_empty() || self.route_options.is_some()
  }

  pub fn has_client(&self) -> bool {
    self.client.is_some()
  }

  pub(crate) fn client(&self) -> Option<&ClientShape> {
    self.client.as_ref()
  }

  pub(crate) fn route_options(&self) -> Option<&RouteOptions> {
    self.route_options.as_ref()
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

  pub(crate) fn decorator_schemes(&self, decorator: &str) -> &[String] {
    self
      .guard_schemes
      .iter()
      .chain(&self.permission_schemes)
      .find(|guard| guard.decorator == decorator)
      .map_or(&[], |guard| guard.schemes.as_slice())
  }

  pub(crate) fn permission_schemes(&self) -> &[GuardScheme] {
    &self.permission_schemes
  }

  pub(crate) fn guard_permission_keys(&self, decorator: &str) -> &[String] {
    self
      .guard_permissions
      .iter()
      .find(|options| options.decorator == decorator)
      .map_or(&[], |options| options.keys.as_slice())
  }

  pub(crate) fn access(&self) -> &AccessMarkers {
    &self.access
  }

  pub(crate) fn option_schemes(&self, decorator: &str) -> Option<&OptionSchemes> {
    self.option_schemes.iter().find(|mapping| mapping.decorator == decorator)
  }

  pub(crate) fn semantics(&self, decorator: &str) -> Semantics {
    self.semantics.of(decorator)
  }
}

pub fn check_family_name(name: &str) -> Result<(), String> {
  let mut characters = name.chars();
  let well_formed = characters.next().is_some_and(|first| first.is_ascii_lowercase())
    && characters.all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_'))
    && name.len() <= MAX_NAME_LEN;
  if !well_formed {
    return Err(format!(
      "a route family name is a lowercase letter followed by lowercase letters, digits, - or _ (at most {MAX_NAME_LEN} characters)"
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

#[derive(Default)]
struct GuardSection {
  decorators: Vec<String>,
  schemes: Vec<GuardScheme>,
  permission_options: Vec<OptionKeys>,
  option_schemes: Vec<OptionSchemes>,
}

impl Reader {
  fn family(&mut self, name: &str, table: &toml::Table) -> Result<DecoratorFamily, FamilyError> {
    let known = [
      "version",
      "packages",
      "controller",
      "routes",
      "guards",
      "permissions",
      "access",
      "params",
      "global_prefix",
      "client",
    ];
    self.warn_unknown("", table, &known);
    match table.get("version") {
      None | Some(Value::Integer(SCHEMA_VERSION)) => {}
      Some(other) => {
        return Err(FamilyError::new(
          "version",
          format!("unsupported version {}; this catl reads version {SCHEMA_VERSION}", crate::describe_value(other)),
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
    self.warn_unknown("routes.", routes_section, &["methods", "path", "options"]);
    let options_only = !routes_section.contains_key("methods") && routes_section.contains_key("options");
    let routes = if options_only { Vec::new() } else { self.route_decorators(routes_section)? };
    let route_options = self.route_options(routes_section)?;
    let route_path = self.path_argument(routes_section, "routes.")?;
    let guards_section = section(table, "guards")?;
    let guards = match guards_section {
      None => GuardSection::default(),
      Some(guards) => self.guards(guards)?,
    };
    let (permissions, permission_schemes) = match section(table, "permissions")? {
      None => (Vec::new(), Vec::new()),
      Some(permissions) => self.permissions(permissions)?,
    };
    let semantics = match guards_section.and_then(|guards| guards.get("semantics")) {
      None => GuardSemantics::default(),
      Some(value) => self.semantics(value, &guards.decorators, &permission_schemes)?,
    };
    let access = match section(table, "access")? {
      None => AccessMarkers::default(),
      Some(access) => self.access(access, &guards)?,
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
      guards: guards.decorators,
      guard_schemes: guards.schemes,
      guard_permissions: guards.permission_options,
      option_schemes: guards.option_schemes,
      semantics,
      permissions,
      permission_schemes,
      access,
      params,
      global_prefix,
      origin: FamilyOrigin::Declared,
      client,
      route_options,
    })
  }

  fn client(&mut self, client: &toml::Table) -> Result<ClientShape, FamilyError> {
    self.warn_unknown(
      "client.",
      client,
      &["modules", "files", "exports", "factories", "methods", "path", "style", "target_package", "base", "surfaces"],
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
      Some(other) => {
        return Err(FamilyError::new(
          "client.style",
          format!("expected \"openapi\" or \"express\", got {}", crate::describe_value(other)),
        ));
      }
    };
    let target_package = client.get("target_package").map(|value| package_name(value, "client.target_package")).transpose()?;
    let base = client.get("base").map(client_base).transpose()?;
    let surfaces = self.surfaces(client)?;
    Ok(ClientShape {
      modules,
      files,
      exports,
      factories,
      methods,
      path,
      style,
      target_package,
      base,
      surfaces,
    })
  }

  fn guards(&mut self, guards: &toml::Table) -> Result<GuardSection, FamilyError> {
    self.warn_unknown("guards.", guards, &["decorators", "auth_schemes", "permission_options", "option_schemes", "semantics"]);
    let decorators = self.optional_decorators(guards, "decorators", "guards.")?;
    let schemes = decorator_schemes(guards.get("auth_schemes"), "guards", &decorators)?;
    let mut option_schemes = Vec::new();
    for (decorator, key, value) in per_decorator(guards.get("option_schemes"), "guards.option_schemes", "guards", &decorators)? {
      if schemes.iter().any(|scheme| scheme.decorator == decorator) {
        return Err(FamilyError::new(key, "names a decorator that guards.auth_schemes also maps; give its schemes in one place"));
      }
      option_schemes.push(option_scheme_map(decorator, &key, value)?);
    }
    let permission_options = per_decorator(guards.get("permission_options"), "guards.permission_options", "guards", &decorators)?
      .into_iter()
      .map(|(decorator, key, value)| {
        Ok(OptionKeys {
          decorator,
          keys: option_keys(value, &key)?,
        })
      })
      .collect::<Result<_, FamilyError>>()?;
    Ok(GuardSection {
      decorators,
      schemes,
      permission_options,
      option_schemes,
    })
  }

  fn semantics(&mut self, value: &Value, guards: &[String], permission_schemes: &[GuardScheme]) -> Result<GuardSemantics, FamilyError> {
    const KEY: &str = "guards.semantics";
    let Value::Table(entries) = value else {
      return Err(FamilyError::new(KEY, format!("must be a table, got {}", crate::describe_value(value))));
    };
    let mut semantics = GuardSemantics {
      default: match entries.get("default") {
        None => Semantics::default(),
        Some(value) => self.semantics_entry(value, &format!("{KEY}.default"), Semantics::default(), true)?,
      },
      decorators: Vec::new(),
    };
    for (decorator, value) in entries.iter().filter(|(name, _)| name.as_str() != "default") {
      let key = format!("{KEY}.{decorator}");
      let is_guard = guards.contains(decorator);
      if !is_guard && !permission_schemes.iter().any(|scheme| &scheme.decorator == decorator) {
        return Err(FamilyError::new(
          key,
          "names a decorator that is neither in guards.decorators nor a permissions.decorators entry with auth_schemes",
        ));
      }
      let entry = self.semantics_entry(value, &key, semantics.default, is_guard)?;
      semantics.decorators.push(DecoratorSemantics {
        decorator: decorator.clone(),
        semantics: entry,
      });
    }
    Ok(semantics)
  }

  fn semantics_entry(&mut self, value: &Value, key: &str, inherited: Semantics, reads_options: bool) -> Result<Semantics, FamilyError> {
    let Value::Table(fields) = value else {
      return Err(FamilyError::new(
        key,
        format!(
          "expected a table such as {{ options = \"deep_merge\", class_marker = \"method_wins\" }}, got {}",
          crate::describe_value(value)
        ),
      ));
    };
    self.warn_unknown(&format!("{key}."), fields, &["options", "class_marker"]);
    if !fields.contains_key("options") && !fields.contains_key("class_marker") {
      return Err(FamilyError::new(key, "must set options or class_marker"));
    }
    let mut semantics = inherited;
    if let Some(options) = fields.get("options") {
      if !reads_options {
        return Err(FamilyError::new(
          format!("{key}.options"),
          "applies only to guards.decorators, whose options the framework reads",
        ));
      }
      semantics.options = match options.as_str() {
        Some("override") => OptionMerge::Override,
        Some("deep_merge") => OptionMerge::DeepMerge,
        _ => {
          return Err(FamilyError::new(
            format!("{key}.options"),
            format!("expected \"override\" or \"deep_merge\", got {}", crate::describe_value(options)),
          ));
        }
      };
    }
    if let Some(marker) = fields.get("class_marker") {
      semantics.class_marker = match marker.as_str() {
        Some("contested") => ClassMarker::Contested,
        Some("method_wins") => ClassMarker::MethodWins,
        _ => {
          return Err(FamilyError::new(
            format!("{key}.class_marker"),
            format!("expected \"contested\" or \"method_wins\", got {}", crate::describe_value(marker)),
          ));
        }
      };
    }
    Ok(semantics)
  }

  fn permissions(&mut self, permissions: &toml::Table) -> Result<(Vec<String>, Vec<GuardScheme>), FamilyError> {
    self.warn_unknown("permissions.", permissions, &["decorators", "auth_schemes"]);
    let decorators = self.decorator_list(required(permissions, "decorators", "permissions.")?, "permissions.decorators")?;
    let schemes = decorator_schemes(permissions.get("auth_schemes"), "permissions", &decorators)?;
    Ok((decorators, schemes))
  }

  fn access(&mut self, access: &toml::Table, guards: &GuardSection) -> Result<AccessMarkers, FamilyError> {
    self.warn_unknown("access.", access, &["public", "optional", "options"]);
    let public = self.optional_decorators(access, "public", "access.")?;
    let optional = self.optional_decorators(access, "optional", "access.")?;
    let mut options = Vec::new();
    for (decorator, key, value) in per_decorator(access.get("options"), "access.options", "guards", &guards.decorators)? {
      let Value::Table(modes) = value else {
        return Err(FamilyError::new(
          key,
          format!(
            "expected a table such as {{ public = \"public\", optional = \"optional\" }}, got {}",
            crate::describe_value(value)
          ),
        ));
      };
      self.warn_unknown(&format!("{key}."), modes, &["public", "optional"]);
      let mut claimed: BTreeSet<String> = BTreeSet::new();
      let permission_keys = guards
        .permission_options
        .iter()
        .find(|options| options.decorator == decorator)
        .map_or(&[][..], |options| options.keys.as_slice());
      for mode in AccessMode::ALL {
        let Some(value) = modes.get(mode.as_str()) else { continue };
        let mode_key = format!("{key}.{}", mode.as_str());
        let keys = option_keys(value, &mode_key)?;
        if keys.iter().any(|option| permission_keys.contains(option)) {
          return Err(FamilyError::new(
            mode_key,
            format!("names an option key that guards.permission_options.{decorator} also lists"),
          ));
        }
        if !keys.iter().all(|option| claimed.insert(option.clone())) {
          return Err(FamilyError::new(mode_key, "names an option key that also marks the other mode"));
        }
        options.push(AccessOption {
          decorator: decorator.clone(),
          mode,
          keys,
        });
      }
      if claimed.is_empty() {
        return Err(FamilyError::new(key, "must map public or optional to an option key"));
      }
    }
    Ok(AccessMarkers { public, optional, options })
  }

  fn surfaces(&mut self, client: &toml::Table) -> Result<Option<SurfaceMap>, FamilyError> {
    let surfaces = match client.get("surfaces") {
      None => return Ok(None),
      Some(Value::Table(surfaces)) => surfaces,
      Some(_) => return Err(FamilyError::new("client.surfaces", "must be a table")),
    };
    self.warn_unknown("client.surfaces.", surfaces, &["modules", "factory", "option", "containers"]);
    Ok(Some(SurfaceMap {
      modules: package_list(required(surfaces, "modules", "client.surfaces.")?, "client.surfaces.modules")?,
      factory: identifier(required(surfaces, "factory", "client.surfaces.")?, "client.surfaces.factory")?,
      option: identifier(required(surfaces, "option", "client.surfaces.")?, "client.surfaces.option")?,
      containers: match surfaces.get("containers") {
        None => Vec::new(),
        Some(value) => identifiers(value, "client.surfaces.containers")?,
      },
    }))
  }

  fn route_options(&mut self, routes: &toml::Table) -> Result<Option<RouteOptions>, FamilyError> {
    let options = match routes.get("options") {
      None => return Ok(None),
      Some(Value::Table(options)) => options,
      Some(_) => return Err(FamilyError::new("routes.options", "must be a table")),
    };
    self.warn_unknown("routes.options.", options, &["decorators", "method", "path"]);
    let decorators = self.decorator_list(required(options, "decorators", "routes.options.")?, "routes.options.decorators")?;
    let key = |name: &str| match options.get(name) {
      None => Ok(name.to_owned()),
      Some(value) => identifier(value, &format!("routes.options.{name}")),
    };
    Ok(Some(RouteOptions {
      decorators,
      method: key("method")?,
      path: key("path")?,
    }))
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
      other => {
        return Err(FamilyError::new(
          "routes.methods",
          format!("expected a table of decorator = \"METHOD\", got {}", crate::describe_value(other)),
        ));
      }
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
        .ok_or_else(|| {
          FamilyError::new(
            &key,
            format!("expected an HTTP method such as \"GET\", \"POST\" or \"ANY\", got {}", crate::describe_value(value)),
          )
        })?;
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
      Some(other) => {
        return Err(FamilyError::new(
          key,
          format!("expected a table such as {{ argument = 0, key = \"path\" }}, got {}", crate::describe_value(other)),
        ));
      }
    };
    self.warn_unknown(&format!("{key}."), path, &["argument", "key"]);
    let index = match path.get("argument") {
      None => 0,
      Some(value) => value.as_integer().and_then(|index| usize::try_from(index).ok()).ok_or_else(|| {
        FamilyError::new(
          format!("{key}.argument"),
          format!("expected a non-negative argument position, got {}", crate::describe_value(value)),
        )
      })?,
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
      .ok_or_else(|| {
        FamilyError::new(
          "global_prefix.factory",
          format!("expected a call path such as \"AppFactory.create\", got {}", crate::describe_value(factory)),
        )
      })?;
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
        return Err(FamilyError::new(key, "expected decorator names such as \"Controller\""));
      }
      self.claim(name, key)?;
    }
    Ok(names)
  }

  fn claim(&mut self, decorator: &str, key: &str) -> Result<(), FamilyError> {
    if !is_identifier(decorator) {
      return Err(FamilyError::new(key, "expected a decorator name".to_owned()));
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
    guard_permissions: Vec::new(),
    option_schemes: Vec::new(),
    semantics: GuardSemantics::default(),
    permissions: Vec::new(),
    permission_schemes: Vec::new(),
    access: AccessMarkers::default(),
    params: ParamDecorators::default(),
    global_prefix: None,
    origin: FamilyOrigin::Declared,
    client: Some(client),
    route_options: None,
  }
}

fn per_decorator<'t>(value: Option<&'t Value>, key: &str, section: &str, decorators: &[String]) -> Result<Vec<(String, String, &'t Value)>, FamilyError> {
  let table = match value {
    None => return Ok(Vec::new()),
    Some(Value::Table(table)) => table,
    Some(other) => {
      return Err(FamilyError::new(
        key,
        format!("expected a table keyed by decorator name, got {}", crate::describe_value(other)),
      ));
    }
  };
  table
    .iter()
    .map(|(decorator, value)| {
      let entry = format!("{key}.{decorator}");
      match decorators.contains(decorator) {
        true => Ok((decorator.clone(), entry, value)),
        false => Err(FamilyError::new(entry, format!("names a decorator that is not listed in {section}.decorators"))),
      }
    })
    .collect()
}

fn decorator_schemes(value: Option<&Value>, section: &str, decorators: &[String]) -> Result<Vec<GuardScheme>, FamilyError> {
  per_decorator(value, &format!("{section}.auth_schemes"), section, decorators)?
    .into_iter()
    .map(|(decorator, key, value)| {
      Ok(GuardScheme {
        decorator,
        schemes: scheme_list(value, &key)?,
      })
    })
    .collect()
}

fn option_scheme_map(decorator: String, key: &str, value: &Value) -> Result<OptionSchemes, FamilyError> {
  let Value::Table(entries) = value else {
    return Err(FamilyError::new(
      key,
      format!(
        "expected a table such as {{ default = \"session_cookie\", service = \"bearer\" }}, got {}",
        crate::describe_value(value)
      ),
    ));
  };
  let mut mapping = OptionSchemes {
    decorator,
    options: Vec::new(),
    default: Vec::new(),
  };
  for (option, schemes) in entries {
    let entry = format!("{key}.{option}");
    if option == "default" {
      mapping.default = scheme_list(schemes, &entry)?;
      continue;
    }
    if !is_identifier(option) {
      return Err(FamilyError::new(key, "expected option keys such as \"service\""));
    }
    if option == UNREAD_OPTIONS {
      return Err(FamilyError::new(entry, "is reserved for options that cannot be read; rename the key"));
    }
    mapping.options.push(OptionScheme {
      option: option.clone(),
      schemes: scheme_list(schemes, &entry)?,
    });
  }
  match mapping.options.is_empty() {
    true => Err(FamilyError::new(key, "must map at least one option key to a scheme")),
    false => Ok(mapping),
  }
}

fn scheme_list(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let invalid = || {
    FamilyError::new(
      key,
      format!(
        "expected one of {} or a non-empty array of them, got {}",
        AUTH_SCHEMES.map(|scheme| format!("\"{scheme}\"")).join(", "),
        crate::describe_value(value)
      ),
    )
  };
  let schemes = one_or_many(value).ok_or_else(invalid)?;
  if !schemes.iter().all(|scheme| AUTH_SCHEMES.contains(&scheme.as_str())) {
    return Err(invalid());
  }
  unique(schemes, key, "lists a scheme more than once")
}

fn option_keys(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let invalid = || {
    FamilyError::new(
      key,
      format!(
        "expected an option key such as \"permission\" or a non-empty array of them, got {}",
        crate::describe_value(value)
      ),
    )
  };
  let keys = one_or_many(value).ok_or_else(invalid)?;
  if !keys.iter().all(|option| is_identifier(option)) {
    return Err(invalid());
  }
  unique(keys, key, "lists an option key more than once")
}

fn one_or_many(value: &Value) -> Option<Vec<String>> {
  match value {
    Value::String(single) => Some(vec![single.clone()]),
    Value::Array(items) if !items.is_empty() => items.iter().map(|item| item.as_str().map(str::to_owned)).collect(),
    _ => None,
  }
}

fn unique(values: Vec<String>, key: &str, message: &str) -> Result<Vec<String>, FamilyError> {
  let distinct: BTreeSet<&String> = values.iter().collect();
  match distinct.len() == values.len() {
    true => Ok(values),
    false => Err(FamilyError::new(key, message)),
  }
}

fn client_base(value: &Value) -> Result<String, FamilyError> {
  let invalid = || {
    FamilyError::new(
      "client.base",
      format!(
        "expected a base path such as \"/api/v1\" or an origin such as \"https://api.acme.test/v1\", got {}",
        crate::describe_value(value)
      ),
    )
  };
  let base = value.as_str().ok_or_else(invalid)?;
  let well_formed = (base.starts_with('/') || base.starts_with("https://") || base.starts_with("http://"))
    && !base.chars().any(|character| character.is_whitespace() || matches!(character, '{' | '}' | '?' | '#'));
  let trimmed = base.trim_end_matches('/');
  match well_formed && !trimmed.is_empty() {
    true => Ok(trimmed.to_owned()),
    false => Err(invalid()),
  }
}

fn package_name(value: &Value, key: &str) -> Result<String, FamilyError> {
  value
    .as_str()
    .filter(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
    .map(str::to_owned)
    .ok_or_else(|| FamilyError::new(key, format!("expected a package name such as \"@acme/books-api\", got {}", crate::describe_value(value))))
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
        .ok_or_else(|| FamilyError::new("client.methods", "expected HTTP method names such as \"get\" or \"GET\""))?;
      Ok(ClientMethod { name, method })
    })
    .collect()
}

fn identifiers(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let names = strings(value, key)?;
  match names.iter().all(|name| is_identifier(name)) {
    true => Ok(names),
    false => Err(FamilyError::new(key, "expected export names such as \"booksApi\" or \"default\"")),
  }
}

fn file_globs(value: &Value, key: &str) -> Result<Vec<String>, FamilyError> {
  let globs = strings(value, key)?;
  let invalid = globs
    .iter()
    .any(|glob| glob.is_empty() || glob.starts_with('/') || glob.starts_with("./") || glob.split('/').any(|segment| segment.is_empty() || segment == ".."));
  match invalid {
    true => Err(FamilyError::new(key, "expected repository-relative paths such as \"apps/web/src/api/*.ts\"")),
    false => Ok(globs),
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
  let invalid = || FamilyError::new(key, format!("expected an array of strings, got {}", crate::describe_value(value)));
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
  if packages
    .iter()
    .any(|package| package.is_empty() || package.chars().any(char::is_whitespace) || package.ends_with('/'))
  {
    return Err(FamilyError::new(key, "expected import specifiers such as \"@acme/http\""));
  }
  Ok(packages)
}

fn identifier(value: &Value, key: &str) -> Result<String, FamilyError> {
  value
    .as_str()
    .filter(|text| is_identifier(text))
    .map(str::to_owned)
    .ok_or_else(|| FamilyError::new(key, format!("expected an identifier, got {}", crate::describe_value(value))))
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
    let text = format!("future = 1\n{ACME}[controller.path]\nargument = 0\nstyle = \"x\"\n[client]\nmodules = [\"@acme/books-client\"]\nretries = 1\n[server]\nport = 1\n");
    let warnings = parse(&text).unwrap().warnings;
    assert_eq!(
      warnings,
      [
        "unknown key future ignored",
        "unknown key server ignored",
        "unknown key controller.path.style ignored",
        "unknown key client.retries ignored"
      ]
    );
  }

  #[test]
  fn should_reject_invalid_values_naming_the_key() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";
    for (text, message) in [
      ("version = 2\n", "version: unsupported version 2; this catl reads version 1"),
      ("version = \"sk-FAKEKEY123\"\n", "version: unsupported version a string; this catl reads version 1"),
      ("[controller]\ndecorators = [\"Api\"]\n", "packages: missing"),
      ("packages = []\n", "packages: must name at least one package, such as \"@acme/http\""),
      ("packages = [\"@acme/ http\"]\n", "packages: expected import specifiers such as \"@acme/http\""),
      ("packages = [\"@acme/http\"]\n", "controller: missing; name the controller decorators"),
      ("packages = [\"@acme/http\"]\ncontroller = 1\n", "controller: must be a table"),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n",
        "routes: missing; map route decorators to HTTP methods under [routes.methods]",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"FETCH\"\n",
        "routes.methods.Read: expected an HTTP method such as \"GET\", \"POST\" or \"ANY\", got a string",
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
        "controller.decorators: expected decorator names such as \"Controller\"",
      ),
      (
        "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\npath = { argument = -1 }\n",
        "controller.path.argument: expected a non-negative argument position, got -1",
      ),
    ] {
      assert_eq!(parse(text), Err(message.to_owned()), "{text}");
    }
    for (extra, message) in [
      ("[guards]\ndecorators = \"Guarded\"\n", "guards.decorators: expected an array of strings, got a string"),
      (
        "[global_prefix]\nfactory = \"Acme Factory\"\nmethod = \"setGlobalPrefix\"\n",
        "global_prefix.factory: expected a call path such as \"AppFactory.create\", got a string",
      ),
      ("[global_prefix]\nfactory = \"AcmeFactory.create\"\n", "global_prefix.method: missing"),
    ] {
      assert_eq!(parse(&format!("{base}{extra}")), Err(message.to_owned()), "{extra}");
    }
    assert!(parse("packages = [").unwrap_err().starts_with("syntax: "));
  }

  #[test]
  fn should_parse_a_client_only_family_with_every_client_key() {
    let text = "[client]\nmodules = [\"@acme/books-client\"]\nfiles = [\"apps/web/src/api/*.ts\"]\nexports = [\"booksApi\", \"default\"]\nfactories = [\"createBooksClient\"]\nmethods = [\"get\", \"POST\"]\npath = { argument = 1 }\nstyle = \"express\"\ntarget_package = \"@acme/books-api\"\nbase = \"/api/v1/\"\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createApiClient\"\noption = \"surfaces\"\ncontainers = [\"apiClient\"]\n";
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
        base: Some("/api/v1".to_owned()),
        surfaces: Some(SurfaceMap {
          modules: vec!["@acme/http-kit".to_owned()],
          factory: "createApiClient".to_owned(),
          option: "surfaces".to_owned(),
          containers: vec!["apiClient".to_owned()],
        }),
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
        "client.files: expected repository-relative paths such as \"apps/web/src/api/*.ts\"",
      ),
      (
        "[client]\nfiles = [\"/abs/api.ts\"]\n",
        "client.files: expected repository-relative paths such as \"apps/web/src/api/*.ts\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nmethods = [\"fetch\"]\n",
        "client.methods: expected HTTP method names such as \"get\" or \"GET\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nmethods = []\n",
        "client.methods: must name at least one method, such as \"get\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\nstyle = \"rails\"\n",
        "client.style: expected \"openapi\" or \"express\", got a string",
      ),
      (
        "[client]\nmodules = [\"x\"]\nexports = [\"books.api\"]\n",
        "client.exports: expected export names such as \"booksApi\" or \"default\"",
      ),
      (
        "[client]\nmodules = [\"x\"]\ntarget_package = \"\"\n",
        "client.target_package: expected a package name such as \"@acme/books-api\", got a string",
      ),
      ("client = 1\n", "client: must be a table"),
      (
        "[client]\nmodules = [\"x\"]\nbase = \"api/v1\"\n",
        "client.base: expected a base path such as \"/api/v1\" or an origin such as \"https://api.acme.test/v1\", got a string",
      ),
      (
        "[client]\nmodules = [\"x\"]\nbase = \"/api/{version}\"\n",
        "client.base: expected a base path such as \"/api/v1\" or an origin such as \"https://api.acme.test/v1\", got a string",
      ),
      (
        "[client]\nmodules = [\"x\"]\nbase = \"/\"\n",
        "client.base: expected a base path such as \"/api/v1\" or an origin such as \"https://api.acme.test/v1\", got a string",
      ),
      ("[client]\nmodules = [\"x\"]\nsurfaces = 1\n", "client.surfaces: must be a table"),
      (
        "[client]\nmodules = [\"x\"]\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\noption = \"surfaces\"\n",
        "client.surfaces.factory: missing",
      ),
      (
        "[client]\nmodules = [\"x\"]\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createApiClient\"\noption = \"surfaces.map\"\n",
        "client.surfaces.option: expected an identifier, got a string",
      ),
      (
        "[client]\nmodules = [\"x\"]\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createApiClient\"\noption = \"surfaces\"\ncontainers = [\"api.client\"]\n",
        "client.surfaces.containers: expected export names such as \"booksApi\" or \"default\"",
      ),
    ] {
      assert_eq!(parse(text), Err(message.to_owned()), "{text}");
    }
  }

  #[test]
  fn should_parse_option_route_decorators_with_default_and_custom_keys() {
    let family = parse(&format!("{ACME}[routes.options]\ndecorators = [\"Route\"]\n")).unwrap().family;
    assert_eq!(
      family.route_options,
      Some(RouteOptions {
        decorators: vec!["Route".to_owned()],
        method: "method".to_owned(),
        path: "path".to_owned(),
      })
    );
    let only = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.options]\ndecorators = [\"Route\"]\nmethod = \"verb\"\npath = \"url\"\n";
    let family = parse(only).unwrap().family;
    assert!(family.has_routes() && family.routes.is_empty());
    assert_eq!(
      family.route_options.map(|options| (options.method, options.path)),
      Some(("verb".to_owned(), "url".to_owned()))
    );
  }

  #[test]
  fn should_reject_invalid_option_route_values_naming_the_key() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n";
    for (extra, message) in [
      ("[routes]\noptions = 1\n", "routes.options: must be a table"),
      ("[routes.options]\nmethod = \"verb\"\n", "routes.options.decorators: missing"),
      (
        "[routes.methods]\nRoute = \"GET\"\n[routes.options]\ndecorators = [\"Route\"]\n",
        "routes.options.decorators: Route is listed more than once; each decorator has one role",
      ),
      (
        "[routes.options]\ndecorators = [\"Route\"]\npath = \"options.path\"\n",
        "routes.options.path: expected an identifier, got a string",
      ),
      ("[routes]\npath = { argument = 0 }\n", "routes.methods: missing"),
    ] {
      assert_eq!(parse(&format!("{base}{extra}")), Err(message.to_owned()), "{extra}");
    }
  }

  #[test]
  fn should_keep_the_digest_of_a_family_without_a_client_section() {
    let family = parse(ACME).unwrap().family;
    let serialized = serde_json::to_string(&family).unwrap();
    assert!(!serialized.contains("client") && !serialized.contains("route_options"), "{serialized}");
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
        schemes: vec!["bearer".to_owned()]
      }]
    );
    assert_eq!(
      (parsed.family.decorator_schemes("Signed"), parsed.family.decorator_schemes("Guarded")),
      (["bearer".to_owned()].as_slice(), [].as_slice())
    );
    assert_eq!(parsed.family.permissions, ["Permit", "BotPermission"]);
  }

  #[test]
  fn should_parse_scheme_arrays_permission_schemes_guard_permission_options_and_access_markers() {
    let text = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Auth\", \"Signed\"]\nauth_schemes = { Auth = [\"bearer\", \"session_cookie\"], Signed = \"api_key\" }\npermission_options = { Auth = [\"permission\", \"bot\"] }\n[permissions]\ndecorators = [\"Permit\", \"RequireScope\"]\nauth_schemes = { RequireScope = \"bearer\" }\n[access]\npublic = [\"Public\"]\noptional = [\"OptionalAuth\"]\noptions = { Auth = { public = \"public\", optional = [\"optional\", \"anonymous\"] } }\n";
    let parsed = parse(text).unwrap();
    let family = parsed.family;

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(family.decorator_schemes("Auth"), ["bearer", "session_cookie"]);
    assert_eq!(family.decorator_schemes("Signed"), ["api_key"]);
    assert_eq!(family.decorator_schemes("RequireScope"), ["bearer"]);
    assert_eq!(family.guard_permission_keys("Auth"), ["permission", "bot"]);
    assert_eq!(family.guard_permission_keys("Signed"), Vec::<String>::new());
    assert_eq!(
      family.access,
      AccessMarkers {
        public: vec!["Public".to_owned()],
        optional: vec!["OptionalAuth".to_owned()],
        options: vec![
          AccessOption {
            decorator: "Auth".to_owned(),
            mode: AccessMode::Public,
            keys: vec!["public".to_owned()],
          },
          AccessOption {
            decorator: "Auth".to_owned(),
            mode: AccessMode::Optional,
            keys: vec!["optional".to_owned(), "anonymous".to_owned()],
          },
        ],
      }
    );
    let access_only = parse("[access]\npublic = [\"Public\"]\n").unwrap_err();
    assert_eq!(access_only, "packages: missing");
  }

  #[test]
  fn should_reject_invalid_access_scheme_and_option_values_naming_the_key_without_echoing_them() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n";
    let schemes = "expected one of \"bearer\", \"api_key\", \"session_cookie\", \"basic\" or a non-empty array of them";
    for (extra, message) in [
      (
        "[guards]\ndecorators = [\"Auth\"]\nauth_schemes = { Auth = [] }\n".to_owned(),
        format!("guards.auth_schemes.Auth: {schemes}, got an array"),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\nauth_schemes = { Auth = [\"bearer\", \"sk-FAKEKEY123\"] }\n".to_owned(),
        format!("guards.auth_schemes.Auth: {schemes}, got an array"),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\nauth_schemes = { Auth = [\"bearer\", \"bearer\"] }\n".to_owned(),
        "guards.auth_schemes.Auth: lists a scheme more than once".to_owned(),
      ),
      (
        "[permissions]\ndecorators = [\"Permit\"]\nauth_schemes = { RequireScope = \"bearer\" }\n".to_owned(),
        "permissions.auth_schemes.RequireScope: names a decorator that is not listed in permissions.decorators".to_owned(),
      ),
      (
        "[permissions]\ndecorators = [\"Permit\"]\nauth_schemes = \"bearer\"\n".to_owned(),
        "permissions.auth_schemes: expected a table keyed by decorator name, got a string".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\npermission_options = { Guarded = \"permission\" }\n".to_owned(),
        "guards.permission_options.Guarded: names a decorator that is not listed in guards.decorators".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\npermission_options = { Auth = \"sk-FAKEKEY123\" }\n".to_owned(),
        "guards.permission_options.Auth: expected an option key such as \"permission\" or a non-empty array of them, got a string".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\n[access]\npublic = [\"Auth\"]\n".to_owned(),
        "access.public: Auth is listed more than once; each decorator has one role".to_owned(),
      ),
      (
        "[permissions]\ndecorators = [\"Permit\"]\n[access]\noptional = [\"Permit\"]\n".to_owned(),
        "access.optional: Permit is listed more than once; each decorator has one role".to_owned(),
      ),
      (
        "[access]\noptions = { Auth = { public = \"public\" } }\n".to_owned(),
        "access.options.Auth: names a decorator that is not listed in guards.decorators".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\n[access]\noptions = { Auth = \"public\" }\n".to_owned(),
        "access.options.Auth: expected a table such as { public = \"public\", optional = \"optional\" }, got a string".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\n[access]\noptions = { Auth = {} }\n".to_owned(),
        "access.options.Auth: must map public or optional to an option key".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\n[access]\noptions = { Auth = { public = \"open\", optional = [\"open\"] } }\n".to_owned(),
        "access.options.Auth.optional: names an option key that also marks the other mode".to_owned(),
      ),
      (
        "[guards]\ndecorators = [\"Auth\"]\npermission_options = { Auth = \"permission\" }\n[access]\noptions = { Auth = { public = \"permission\" } }\n".to_owned(),
        "access.options.Auth.public: names an option key that guards.permission_options.Auth also lists".to_owned(),
      ),
    ] {
      let error = parse(&format!("{base}{extra}")).unwrap_err();
      assert!(!error.contains("FAKEKEY"), "{error}");
      assert_eq!(error, message, "{extra}");
    }
    assert_eq!(parse(&format!("access = 1\n{base}")), Err("access: must be a table".to_owned()));
    let warnings = parse(&format!(
      "{base}[guards]\ndecorators = [\"Auth\"]\n[access]\npublic = [\"Public\"]\nmarkers = 1\noptions = {{ Auth = {{ public = \"public\", anonymous = \"anonymous\" }} }}\n"
    ))
    .unwrap()
    .warnings;
    assert_eq!(warnings, ["unknown key access.markers ignored", "unknown key access.options.Auth.anonymous ignored"]);
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
        "guards.auth_schemes.Guarded: expected one of \"bearer\", \"api_key\", \"session_cookie\", \"basic\" or a non-empty array of them, got a string",
      ),
      (
        "[guards]\ndecorators = [\"Guarded\"]\nauth_schemes = [\"bearer\"]\n",
        "guards.auth_schemes: expected a table keyed by decorator name, got an array",
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
    for absent in ["guard_schemes", "guard_permissions", "permissions", "permission_schemes", "access"] {
      assert!(!serialized.contains(absent), "{serialized}");
    }
    let with_permissions = parse(&format!("{ACME}[permissions]\ndecorators = [\"Permit\"]\n")).unwrap().family;
    assert_ne!(families_digest(std::slice::from_ref(&family)), families_digest(&[with_permissions]));
  }

  #[test]
  fn should_parse_guard_semantics_with_a_family_default_and_option_schemes() {
    let text = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Auth\", \"Authenticated\"]\n[guards.option_schemes]\nAuth = { default = \"session_cookie\", service = \"bearer\", bot = [\"bearer\", \"api_key\"] }\n[guards.semantics]\ndefault = { class_marker = \"method_wins\" }\nAuth = { options = \"deep_merge\" }\nRequireScope = { class_marker = \"contested\" }\n[permissions]\ndecorators = [\"RequireScope\"]\nauth_schemes = { RequireScope = \"bearer\" }\n";
    let parsed = parse(text).unwrap();
    let family = parsed.family;

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(
      family.option_schemes("Auth"),
      Some(&OptionSchemes {
        decorator: "Auth".to_owned(),
        options: vec![
          OptionScheme {
            option: "bot".to_owned(),
            schemes: vec!["bearer".to_owned(), "api_key".to_owned()],
          },
          OptionScheme {
            option: "service".to_owned(),
            schemes: vec!["bearer".to_owned()],
          },
        ],
        default: vec!["session_cookie".to_owned()],
      })
    );
    let semantics = |decorator: &str| family.semantics(decorator);
    assert_eq!(
      (semantics("Auth"), semantics("Authenticated"), semantics("RequireScope")),
      (
        Semantics {
          options: OptionMerge::DeepMerge,
          class_marker: ClassMarker::MethodWins,
        },
        Semantics {
          options: OptionMerge::Override,
          class_marker: ClassMarker::MethodWins,
        },
        Semantics {
          options: OptionMerge::Override,
          class_marker: ClassMarker::Contested,
        },
      )
    );
  }

  #[test]
  fn should_keep_the_digest_and_defaults_of_a_family_without_or_with_only_default_semantics() {
    let family = parse(ACME).unwrap().family;
    let serialized = serde_json::to_string(&family).unwrap();
    for absent in ["semantics", "option_schemes"] {
      assert!(!serialized.contains(absent), "{serialized}");
    }
    assert_eq!(family.semantics("Guarded"), Semantics::default());
    let explicit = parse(&format!("{ACME}[guards.semantics]\ndefault = {{ options = \"override\", class_marker = \"contested\" }}\n"))
      .unwrap()
      .family;
    assert_eq!(families_digest(std::slice::from_ref(&family)), families_digest(&[explicit]));
    let merging = parse(&format!("{ACME}[guards.semantics]\nGuarded = {{ options = \"deep_merge\" }}\n")).unwrap().family;
    assert_ne!(families_digest(std::slice::from_ref(&family)), families_digest(&[merging]));
  }

  #[test]
  fn should_reject_invalid_semantics_and_option_schemes_naming_the_key_without_echoing_values() {
    let base = "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Auth\"]\n";
    let schemes = "expected one of \"bearer\", \"api_key\", \"session_cookie\", \"basic\" or a non-empty array of them";
    for (extra, message) in [
      ("semantics = \"deep_merge\"\n".to_owned(), "guards.semantics: must be a table, got a string".to_owned()),
      (
        "[guards.semantics]\nAuth = \"deep_merge\"\n".to_owned(),
        "guards.semantics.Auth: expected a table such as { options = \"deep_merge\", class_marker = \"method_wins\" }, got a string".to_owned(),
      ),
      (
        "[guards.semantics]\nAuth = { options = \"sk-FAKEKEY123\" }\n".to_owned(),
        "guards.semantics.Auth.options: expected \"override\" or \"deep_merge\", got a string".to_owned(),
      ),
      (
        "[guards.semantics]\ndefault = { class_marker = true }\n".to_owned(),
        "guards.semantics.default.class_marker: expected \"contested\" or \"method_wins\", got true".to_owned(),
      ),
      (
        "[guards.semantics]\nAuth = {}\n".to_owned(),
        "guards.semantics.Auth: must set options or class_marker".to_owned(),
      ),
      (
        "[guards.semantics]\nPermit = { class_marker = \"method_wins\" }\n[permissions]\ndecorators = [\"Permit\"]\n".to_owned(),
        "guards.semantics.Permit: names a decorator that is neither in guards.decorators nor a permissions.decorators entry with auth_schemes".to_owned(),
      ),
      (
        "[guards.semantics]\nPermit = { options = \"deep_merge\" }\n[permissions]\ndecorators = [\"Permit\"]\nauth_schemes = { Permit = \"bearer\" }\n".to_owned(),
        "guards.semantics.Permit.options: applies only to guards.decorators, whose options the framework reads".to_owned(),
      ),
      (
        "option_schemes = { Auth = \"bearer\" }\n".to_owned(),
        "guards.option_schemes.Auth: expected a table such as { default = \"session_cookie\", service = \"bearer\" }, got a string".to_owned(),
      ),
      (
        "option_schemes = { Signed = { service = \"bearer\" } }\n".to_owned(),
        "guards.option_schemes.Signed: names a decorator that is not listed in guards.decorators".to_owned(),
      ),
      (
        "option_schemes = { Auth = { default = \"bearer\" } }\n".to_owned(),
        "guards.option_schemes.Auth: must map at least one option key to a scheme".to_owned(),
      ),
      (
        "option_schemes = { Auth = { service = \"sk-FAKEKEY123\" } }\n".to_owned(),
        format!("guards.option_schemes.Auth.service: {schemes}, got a string"),
      ),
      (
        "option_schemes = { Auth = { \"service.name\" = \"bearer\" } }\n".to_owned(),
        "guards.option_schemes.Auth: expected option keys such as \"service\"".to_owned(),
      ),
      (
        "option_schemes = { Auth = { unread = \"bearer\" } }\n".to_owned(),
        "guards.option_schemes.Auth.unread: is reserved for options that cannot be read; rename the key".to_owned(),
      ),
      (
        "auth_schemes = { Auth = \"bearer\" }\noption_schemes = { Auth = { service = \"bearer\" } }\n".to_owned(),
        "guards.option_schemes.Auth: names a decorator that guards.auth_schemes also maps; give its schemes in one place".to_owned(),
      ),
    ] {
      let error = parse(&format!("{base}{extra}")).unwrap_err();
      assert!(!error.contains("FAKEKEY"), "{error}");
      assert_eq!(error, message, "{extra}");
    }
    let warnings = parse(&format!("{base}[guards.semantics]\nAuth = {{ options = \"deep_merge\", arrays = \"concat\" }}\n"))
      .unwrap()
      .warnings;
    assert_eq!(warnings, ["unknown key guards.semantics.Auth.arrays ignored"]);
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
