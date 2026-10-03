use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{RelPath, RepoName};
use crate::kinds::EntityKind;
use crate::lang::LanguageFamily;

const SEGMENT_RESERVED: &[char] = &['%', ':', '/', '#', '~'];
const FIELD_RESERVED: &[char] = &['%', ':', '#', '~'];
const TAIL_RESERVED: &[char] = &['%'];
const TS_SOURCE_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

pub const UNPACKAGED: &str = "_";

fn escape<'a>(value: &'a str, reserved: &[char]) -> Cow<'a, str> {
  let needs_escape = |c: char| c.is_control() || reserved.contains(&c);
  if !value.chars().any(needs_escape) {
    return Cow::Borrowed(value);
  }
  let mut escaped = String::with_capacity(value.len() + 8);
  for c in value.chars() {
    if !needs_escape(c) {
      escaped.push(c);
      continue;
    }
    let mut buffer = [0u8; 4];
    for byte in c.encode_utf8(&mut buffer).bytes() {
      escaped.push_str(&format!("%{byte:02X}"));
    }
  }
  Cow::Owned(escaped)
}

fn unescape(value: &str) -> Option<String> {
  let bytes = value.as_bytes();
  let mut decoded = Vec::with_capacity(bytes.len());
  let mut index = 0;
  while index < bytes.len() {
    if bytes[index] != b'%' {
      decoded.push(bytes[index]);
      index += 1;
      continue;
    }
    let hex = value.get(index + 1..index + 3)?;
    decoded.push(u8::from_str_radix(hex, 16).ok()?);
    index += 3;
  }
  String::from_utf8(decoded).ok()
}

fn unescape_field(key: &str, value: &str) -> Result<String, CoreError> {
  unescape(value).ok_or_else(|| CoreError::invalid_key(key, "malformed percent escape"))
}

fn unescape_segments(key: &str, joined: &str, separator: &str) -> Result<Vec<String>, CoreError> {
  if joined.is_empty() {
    return Ok(Vec::new());
  }
  joined.split(separator).map(|segment| unescape_field(key, segment)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SymbolKey {
  family: LanguageFamily,
  repo: RepoName,
  package: String,
  module_path: Vec<String>,
  item_path: Vec<String>,
  kind: EntityKind,
  disambiguator: u32,
}

impl SymbolKey {
  pub fn new(
    family: LanguageFamily,
    repo: RepoName,
    package: impl Into<String>,
    module_path: Vec<String>,
    item_path: Vec<String>,
    kind: EntityKind,
    disambiguator: u32,
  ) -> Result<Self, CoreError> {
    let package = package.into();
    let key = Self {
      family,
      repo,
      package,
      module_path,
      item_path,
      kind,
      disambiguator,
    };
    if key.package.is_empty() {
      return Err(CoreError::invalid_key(&key.to_string(), "package must not be empty; use UNPACKAGED"));
    }
    if key.module_path.iter().chain(&key.item_path).any(String::is_empty) {
      return Err(CoreError::invalid_key(&key.to_string(), "path segments must not be empty"));
    }
    Ok(key)
  }

  pub fn family(&self) -> LanguageFamily {
    self.family
  }

  pub fn repo(&self) -> &RepoName {
    &self.repo
  }

  pub fn package(&self) -> &str {
    &self.package
  }

  pub fn module_path(&self) -> &[String] {
    &self.module_path
  }

  pub fn item_path(&self) -> &[String] {
    &self.item_path
  }

  pub fn kind(&self) -> &EntityKind {
    &self.kind
  }

  pub fn disambiguator(&self) -> u32 {
    self.disambiguator
  }

  pub fn with_disambiguator(mut self, disambiguator: u32) -> Self {
    self.disambiguator = disambiguator;
    self
  }

  pub fn name(&self) -> &str {
    self.item_path.last().or_else(|| self.module_path.last()).map_or(&self.package, String::as_str)
  }
}

impl fmt::Display for SymbolKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}:{}:{}:", self.family, self.repo, escape(&self.package, FIELD_RESERVED))?;
    for (index, segment) in self.module_path.iter().enumerate() {
      if index > 0 {
        f.write_str("/")?;
      }
      f.write_str(&escape(segment, SEGMENT_RESERVED))?;
    }
    f.write_str("::")?;
    for (index, segment) in self.item_path.iter().enumerate() {
      if index > 0 {
        f.write_str("::")?;
      }
      f.write_str(&escape(segment, SEGMENT_RESERVED))?;
    }
    write!(f, "#{}", escape(self.kind.as_str(), SEGMENT_RESERVED))?;
    if self.disambiguator > 0 {
      write!(f, "~{}", self.disambiguator)?;
    }
    Ok(())
  }
}

impl FromStr for SymbolKey {
  type Err = CoreError;

  fn from_str(key: &str) -> Result<Self, Self::Err> {
    let invalid = |reason: &str| CoreError::invalid_key(key, reason);
    let (body, disambiguator) = match key.rsplit_once('~') {
      Some((body, digits)) => (body, digits.parse::<u32>().map_err(|_| invalid("disambiguator must be a number"))?),
      None => (key, 0),
    };
    let (body, kind) = body.rsplit_once('#').ok_or_else(|| invalid("missing #kind"))?;
    let mut fields = body.splitn(4, ':');
    let (Some(family), Some(repo), Some(package), Some(rest)) = (fields.next(), fields.next(), fields.next(), fields.next()) else {
      return Err(invalid("expected <lang>:<repo>:<package>:<module>::<items>"));
    };
    let (module, items) = rest.split_once("::").ok_or_else(|| invalid("missing '::' between module path and item path"))?;
    let parsed = Self::new(
      family.parse()?,
      repo.parse()?,
      unescape_field(key, package)?,
      unescape_segments(key, module, "/")?,
      unescape_segments(key, items, "::")?,
      unescape_field(key, kind)?.parse()?,
      disambiguator,
    )?;
    if parsed.to_string() != key {
      return Err(invalid("not in canonical form"));
    }
    Ok(parsed)
  }
}

impl TryFrom<String> for SymbolKey {
  type Error = CoreError;

  fn try_from(value: String) -> Result<Self, Self::Error> {
    value.parse()
  }
}

impl From<SymbolKey> for String {
  fn from(value: SymbolKey) -> Self {
    value.to_string()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RustTarget {
  Lib,
  Bin(String),
  Test(String),
  Example(String),
  Bench(String),
  BuildScript,
}

impl RustTarget {
  pub fn package_field(&self, cargo_package: &str) -> String {
    match self {
      Self::Lib => cargo_package.to_owned(),
      Self::Bin(name) => format!("{cargo_package}/bin/{name}"),
      Self::Test(name) => format!("{cargo_package}/test/{name}"),
      Self::Example(name) => format!("{cargo_package}/example/{name}"),
      Self::Bench(name) => format!("{cargo_package}/bench/{name}"),
      Self::BuildScript => format!("{cargo_package}/build"),
    }
  }
}

pub fn ts_module_path(file: &RelPath, package_dir: Option<&RelPath>) -> Option<Vec<String>> {
  let within_package = match package_dir {
    Some(dir) => file.strip_dir_prefix(dir)?,
    None => file.as_str(),
  };
  let stem = TS_SOURCE_EXTENSIONS
    .iter()
    .find_map(|extension| within_package.strip_suffix(extension))
    .unwrap_or(within_package);
  Some(stem.split('/').map(str::to_owned).collect())
}

pub fn normalize_route(raw: &str) -> String {
  let path = raw.split(['?', '#']).next().unwrap_or_default();
  let segments: Vec<String> = path.split('/').filter(|segment| !segment.is_empty()).map(normalize_route_segment).collect();
  format!("/{}", segments.join("/"))
}

fn normalize_route_segment(segment: &str) -> String {
  let parameter = segment
    .strip_prefix(':')
    .or_else(|| segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
    .or_else(|| segment.strip_prefix('<').and_then(|s| s.strip_suffix('>')).map(|s| s.rsplit(':').next().unwrap_or(s)))
    .or_else(|| segment.strip_prefix('[').and_then(|s| s.strip_suffix(']')).map(|s| s.trim_start_matches("...")));
  match parameter {
    Some(name) => format!("{{{}}}", name.split('(').next().unwrap_or(name)),
    None => segment.to_owned(),
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
pub enum HttpMethod {
  Get,
  Post,
  Put,
  Patch,
  Delete,
  Head,
  Options,
  Trace,
  Connect,
  Any,
  Unknown,
}

impl HttpMethod {
  pub const ALL: [HttpMethod; 11] = [
    Self::Get,
    Self::Post,
    Self::Put,
    Self::Patch,
    Self::Delete,
    Self::Head,
    Self::Options,
    Self::Trace,
    Self::Connect,
    Self::Any,
    Self::Unknown,
  ];

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Get => "GET",
      Self::Post => "POST",
      Self::Put => "PUT",
      Self::Patch => "PATCH",
      Self::Delete => "DELETE",
      Self::Head => "HEAD",
      Self::Options => "OPTIONS",
      Self::Trace => "TRACE",
      Self::Connect => "CONNECT",
      Self::Any => "ANY",
      Self::Unknown => "UNKNOWN",
    }
  }

  pub fn parse_lenient(value: &str) -> Option<Self> {
    Self::ALL.into_iter().find(|method| method.as_str().eq_ignore_ascii_case(value))
  }
}

impl fmt::Display for HttpMethod {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl FromStr for HttpMethod {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    Self::ALL.into_iter().find(|method| method.as_str() == value).ok_or_else(|| CoreError::UnknownVariant {
      what: "http method",
      value: value.to_owned(),
    })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PackageEcosystem {
  Cargo,
  Npm,
}

impl PackageEcosystem {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Cargo => "cargo",
      Self::Npm => "npm",
    }
  }
}

impl fmt::Display for PackageEcosystem {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl FromStr for PackageEcosystem {
  type Err = CoreError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    match value {
      "cargo" => Ok(Self::Cargo),
      "npm" => Ok(Self::Npm),
      other => Err(CoreError::UnknownVariant {
        what: "package ecosystem",
        value: other.to_owned(),
      }),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[non_exhaustive]
pub enum EntityKey {
  Symbol(SymbolKey),
  Repository { repo: RepoName },
  File { repo: RepoName, path: RelPath },
  Package { ecosystem: PackageEcosystem, name: String },
  Application { repo: RepoName, name: String },
  Service { repo: RepoName, name: String },
  Endpoint { repo: RepoName, method: HttpMethod, route: String },
  ClientCall { repo: RepoName, method: HttpMethod, target: String },
  AuthScheme { repo: RepoName, scheme: String },
  Permission { repo: RepoName, name: String },
  ConfigKey { repo: RepoName, key: String },
  EnvVar { repo: RepoName, name: String },
  Event { name: String },
  Queue { name: String },
  Job { repo: RepoName, name: String },
  DbTable { name: String },
  ExternalService { name: String },
  Capability { name: String },
  Other { kind: EntityKind, repo: Option<RepoName>, name: String },
}

impl EntityKey {
  pub fn kind(&self) -> EntityKind {
    match self {
      Self::Symbol(key) => key.kind().clone(),
      Self::Repository { .. } => EntityKind::Repository,
      Self::File { .. } => EntityKind::File,
      Self::Package { .. } => EntityKind::Package,
      Self::Application { .. } => EntityKind::Application,
      Self::Service { .. } => EntityKind::Service,
      Self::Endpoint { .. } => EntityKind::ApiEndpoint,
      Self::ClientCall { .. } => EntityKind::ApiClientCall,
      Self::AuthScheme { .. } => EntityKind::AuthScheme,
      Self::Permission { .. } => EntityKind::Permission,
      Self::ConfigKey { .. } => EntityKind::ConfigKey,
      Self::EnvVar { .. } => EntityKind::EnvVar,
      Self::Event { .. } => EntityKind::Event,
      Self::Queue { .. } => EntityKind::Queue,
      Self::Job { .. } => EntityKind::Job,
      Self::DbTable { .. } => EntityKind::DbTable,
      Self::ExternalService { .. } => EntityKind::ExternalService,
      Self::Capability { .. } => EntityKind::Capability,
      Self::Other { kind, .. } => kind.clone(),
    }
  }

  pub fn repo(&self) -> Option<&RepoName> {
    match self {
      Self::Symbol(key) => Some(key.repo()),
      Self::Repository { repo }
      | Self::File { repo, .. }
      | Self::Application { repo, .. }
      | Self::Service { repo, .. }
      | Self::Endpoint { repo, .. }
      | Self::ClientCall { repo, .. }
      | Self::AuthScheme { repo, .. }
      | Self::Permission { repo, .. }
      | Self::ConfigKey { repo, .. }
      | Self::EnvVar { repo, .. }
      | Self::Job { repo, .. } => Some(repo),
      Self::Other { repo, .. } => repo.as_ref(),
      Self::Package { .. } | Self::Event { .. } | Self::Queue { .. } | Self::DbTable { .. } | Self::ExternalService { .. } | Self::Capability { .. } => None,
    }
  }

  pub fn display_name(&self) -> String {
    match self {
      Self::Symbol(key) => key.name().to_owned(),
      Self::Repository { repo } => repo.to_string(),
      Self::File { path, .. } => path.to_string(),
      Self::Endpoint { method, route, .. } => format!("{method} {route}"),
      Self::ClientCall { method, target, .. } => format!("{method} {target}"),
      Self::AuthScheme { scheme, .. } => scheme.clone(),
      Self::ConfigKey { key, .. } => key.clone(),
      Self::Package { name, .. }
      | Self::Application { name, .. }
      | Self::Service { name, .. }
      | Self::Permission { name, .. }
      | Self::EnvVar { name, .. }
      | Self::Event { name }
      | Self::Queue { name }
      | Self::Job { name, .. }
      | Self::DbTable { name }
      | Self::ExternalService { name }
      | Self::Capability { name }
      | Self::Other { name, .. } => name.clone(),
    }
  }

  pub fn as_symbol(&self) -> Option<&SymbolKey> {
    match self {
      Self::Symbol(key) => Some(key),
      _ => None,
    }
  }
}

impl From<SymbolKey> for EntityKey {
  fn from(key: SymbolKey) -> Self {
    Self::Symbol(key)
  }
}

impl fmt::Display for EntityKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let tail = |value: &str| escape(value, TAIL_RESERVED).into_owned();
    match self {
      Self::Symbol(key) => write!(f, "{key}"),
      Self::Repository { repo } => write!(f, "repo:{repo}"),
      Self::File { repo, path } => write!(f, "file:{repo}:{}", tail(path.as_str())),
      Self::Package { ecosystem, name } => write!(f, "pkg:{ecosystem}:{}", tail(name)),
      Self::Application { repo, name } => write!(f, "app:{repo}:{}", tail(name)),
      Self::Service { repo, name } => write!(f, "service:{repo}:{}", tail(name)),
      Self::Endpoint { repo, method, route } => write!(f, "endpoint:{repo}:{method} {}", tail(route)),
      Self::ClientCall { repo, method, target } => write!(f, "client:{repo}:{method} {}", tail(target)),
      Self::AuthScheme { repo, scheme } => write!(f, "auth:{repo}:{}", tail(scheme)),
      Self::Permission { repo, name } => write!(f, "permission:{repo}:{}", tail(name)),
      Self::ConfigKey { repo, key } => write!(f, "config:{repo}:{}", tail(key)),
      Self::EnvVar { repo, name } => write!(f, "env:{repo}:{}", tail(name)),
      Self::Event { name } => write!(f, "event:{}", tail(name)),
      Self::Queue { name } => write!(f, "queue:{}", tail(name)),
      Self::Job { repo, name } => write!(f, "job:{repo}:{}", tail(name)),
      Self::DbTable { name } => write!(f, "table:{}", tail(name)),
      Self::ExternalService { name } => write!(f, "external:{}", tail(name)),
      Self::Capability { name } => write!(f, "capability:{}", tail(name)),
      Self::Other { kind, repo, name } => {
        let repo = repo.as_ref().map(RepoName::as_str).unwrap_or_default();
        write!(f, "other:{}:{repo}:{}", escape(kind.as_str(), FIELD_RESERVED), tail(name))
      }
    }
  }
}

fn parse_domain_key(key: &str, prefix: &str, rest: &str) -> Result<EntityKey, CoreError> {
  let invalid = |reason: &str| CoreError::invalid_key(key, reason);
  let repo_and_tail = || -> Result<(RepoName, String), CoreError> {
    let (repo, tail) = rest.split_once(':').ok_or_else(|| invalid("expected <repo>:<name>"))?;
    Ok((repo.parse()?, unescape_field(key, tail)?))
  };
  let name = || unescape_field(key, rest);
  let method_and_tail = |tail: String| -> Result<(HttpMethod, String), CoreError> {
    let (method, value) = tail.split_once(' ').ok_or_else(|| invalid("expected <METHOD> <route>"))?;
    Ok((method.parse()?, value.to_owned()))
  };
  Ok(match prefix {
    "repo" => EntityKey::Repository { repo: rest.parse()? },
    "file" => {
      let (repo, path) = repo_and_tail()?;
      EntityKey::File { repo, path: RelPath::new(&path)? }
    }
    "pkg" => {
      let (ecosystem, name) = rest.split_once(':').ok_or_else(|| invalid("expected <ecosystem>:<name>"))?;
      EntityKey::Package {
        ecosystem: ecosystem.parse()?,
        name: unescape_field(key, name)?,
      }
    }
    "app" => repo_and_tail().map(|(repo, name)| EntityKey::Application { repo, name })?,
    "service" => repo_and_tail().map(|(repo, name)| EntityKey::Service { repo, name })?,
    "endpoint" => {
      let (repo, tail) = repo_and_tail()?;
      let (method, route) = method_and_tail(tail)?;
      EntityKey::Endpoint { repo, method, route }
    }
    "client" => {
      let (repo, tail) = repo_and_tail()?;
      let (method, target) = method_and_tail(tail)?;
      EntityKey::ClientCall { repo, method, target }
    }
    "auth" => repo_and_tail().map(|(repo, scheme)| EntityKey::AuthScheme { repo, scheme })?,
    "permission" => repo_and_tail().map(|(repo, name)| EntityKey::Permission { repo, name })?,
    "config" => repo_and_tail().map(|(repo, key)| EntityKey::ConfigKey { repo, key })?,
    "env" => repo_and_tail().map(|(repo, name)| EntityKey::EnvVar { repo, name })?,
    "event" => EntityKey::Event { name: name()? },
    "queue" => EntityKey::Queue { name: name()? },
    "job" => repo_and_tail().map(|(repo, name)| EntityKey::Job { repo, name })?,
    "table" => EntityKey::DbTable { name: name()? },
    "external" => EntityKey::ExternalService { name: name()? },
    "capability" => EntityKey::Capability { name: name()? },
    "other" => {
      let mut fields = rest.splitn(3, ':');
      let (Some(kind), Some(repo), Some(name)) = (fields.next(), fields.next(), fields.next()) else {
        return Err(invalid("expected other:<kind>:<repo?>:<name>"));
      };
      let repo = if repo.is_empty() { None } else { Some(repo.parse()?) };
      EntityKey::Other {
        kind: unescape_field(key, kind)?.parse()?,
        repo,
        name: unescape_field(key, name)?,
      }
    }
    _ => return Err(invalid("unknown key prefix")),
  })
}

impl FromStr for EntityKey {
  type Err = CoreError;

  fn from_str(key: &str) -> Result<Self, Self::Err> {
    let (prefix, rest) = key.split_once(':').ok_or_else(|| CoreError::invalid_key(key, "missing key prefix"))?;
    let parsed = match prefix.parse::<LanguageFamily>() {
      Ok(_) => EntityKey::Symbol(key.parse()?),
      Err(_) => parse_domain_key(key, prefix, rest)?,
    };
    if parsed.to_string() != key {
      return Err(CoreError::invalid_key(key, "not in canonical form"));
    }
    Ok(parsed)
  }
}

impl TryFrom<String> for EntityKey {
  type Error = CoreError;

  fn try_from(value: String) -> Result<Self, Self::Error> {
    value.parse()
  }
}

impl From<EntityKey> for String {
  fn from(value: EntityKey) -> Self {
    value.to_string()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn repo(name: &str) -> RepoName {
    RepoName::new(name).unwrap()
  }

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
  }

  fn symbol(family: LanguageFamily, package: &str, module: &[&str], items: &[&str], kind: EntityKind, n: u32) -> SymbolKey {
    SymbolKey::new(family, repo("backend"), package, strings(module), strings(items), kind, n).unwrap()
  }

  #[test]
  fn should_render_the_documented_canonical_forms() {
    let rust = SymbolKey::new(
      LanguageFamily::Rust,
      repo("atlas"),
      "atlas-store",
      strings(&["graph"]),
      strings(&["GraphStore", "neighbors"]),
      EntityKind::Method,
      0,
    )
    .unwrap();
    assert_eq!(rust.to_string(), "rust:atlas:atlas-store:graph::GraphStore::neighbors#method");
    let ts = symbol(
      LanguageFamily::Ts,
      "@acme/api",
      &["src", "users", "users.controller"],
      &["UsersController", "findOne"],
      EntityKind::Method,
      0,
    );
    assert_eq!(ts.to_string(), "ts:backend:@acme/api:src/users/users.controller::UsersController::findOne#method");
    let overload = symbol(LanguageFamily::Ts, "pkg", &["index"], &["parse"], EntityKind::Function, 2);
    assert_eq!(overload.to_string(), "ts:backend:pkg:index::parse#function~2");
  }

  #[test]
  fn should_render_crate_roots_and_modules_with_empty_paths() {
    let crate_root = symbol(LanguageFamily::Rust, "atlas-core", &[], &[], EntityKind::Module, 0);
    assert_eq!(crate_root.to_string(), "rust:backend:atlas-core:::#module");
    let module = symbol(LanguageFamily::Rust, "atlas-core", &["keys", "tests"], &[], EntityKind::Module, 0);
    assert_eq!(module.to_string(), "rust:backend:atlas-core:keys/tests::#module");
    let root_fn = symbol(LanguageFamily::Rust, "atlas-cli/bin/code-atlas", &[], &["main"], EntityKind::Function, 0);
    assert_eq!(root_fn.to_string(), "rust:backend:atlas-cli/bin/code-atlas:::main#function");
  }

  #[test]
  fn should_escape_separator_characters_inside_segments() {
    let key = symbol(
      LanguageFamily::Ts,
      "we:ird#pkg~",
      &["a/b", "c:d"],
      &["x#y", "p~q", "100%", "line\nbreak"],
      EntityKind::Method,
      0,
    );
    assert_eq!(key.to_string(), "ts:backend:we%3Aird%23pkg%7E:a%2Fb/c%3Ad::x%23y::p%7Eq::100%25::line%0Abreak#method");
  }

  #[test]
  fn should_round_trip_tricky_symbol_keys() {
    let trait_method = symbol(LanguageFamily::Rust, "app", &["error"], &["AppError", "<From<io::Error>>", "from"], EntityKind::Method, 0);
    let cases = vec![
      trait_method,
      symbol(LanguageFamily::Ts, "@scope/pkg", &["src", "index"], &["default"], EntityKind::Function, 0),
      symbol(LanguageFamily::Ts, "@scope/pkg", &["index"], &["$store", "_private$"], EntityKind::Field, 0),
      symbol(LanguageFamily::Ts, "pkg", &["src", "types.d"], &["Foo"], EntityKind::Interface, 0),
      symbol(LanguageFamily::Ts, "pkg", &["src", "ユーザー"], &["取得", "ünïcödé"], EntityKind::Function, 0),
      symbol(
        LanguageFamily::Ts,
        "pkg",
        &["users.spec"],
        &["UsersService", "should find a user by id"],
        EntityKind::Test,
        0,
      ),
      symbol(LanguageFamily::Ts, "pkg", &["routes"], &["GET /users/:id"], EntityKind::Custom("route_handler".into()), 0),
      symbol(LanguageFamily::Ts, "pkg", &[".", ".."], &["..."], EntityKind::Variable, 7),
      symbol(LanguageFamily::Ts, UNPACKAGED, &["scripts", "build"], &["run"], EntityKind::Function, 0),
      symbol(
        LanguageFamily::Rust,
        "a",
        &["::"],
        &[":", "::", "%3A", "~1", "#method"],
        EntityKind::Constant,
        4_000_000_000,
      ),
      symbol(LanguageFamily::Rust, "a", &[], &["tab\there", "\u{7f}", " "], EntityKind::Constant, 1),
    ];
    for key in cases {
      let rendered = key.to_string();
      let parsed: SymbolKey = rendered.parse().unwrap_or_else(|error| panic!("{rendered}: {error}"));
      assert_eq!(parsed, key, "{rendered}");
      assert_eq!(parsed.to_string(), rendered);
      let entity: EntityKey = rendered.parse().unwrap();
      assert_eq!(entity, EntityKey::Symbol(key.clone()));
      assert_eq!(serde_json::from_str::<SymbolKey>(&serde_json::to_string(&key).unwrap()).unwrap(), key);
    }
  }

  const HOSTILE_ALPHABET: [&str; 16] = ["a", "Z", "_", "$", ":", "::", "/", "#", "~", "%", "%2F", ".", " ", "é", "\n", "@"];

  struct XorShift(u64);

  impl XorShift {
    fn below(&mut self, bound: usize) -> usize {
      self.0 ^= self.0 << 13;
      self.0 ^= self.0 >> 7;
      self.0 ^= self.0 << 17;
      usize::try_from(self.0 % u64::try_from(bound).unwrap()).unwrap()
    }

    fn segment(&mut self) -> String {
      let length = 1 + self.below(4);
      (0..length).map(|_| HOSTILE_ALPHABET[self.below(HOSTILE_ALPHABET.len())]).collect()
    }

    fn segments(&mut self, max: usize) -> Vec<String> {
      let count = self.below(max + 1);
      (0..count).map(|_| self.segment()).collect()
    }
  }

  #[test]
  fn should_round_trip_generated_segments_over_a_hostile_alphabet() {
    let mut random = XorShift(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2_000 {
      let module = random.segments(2);
      let items = random.segments(3);
      let package = random.segment();
      let disambiguator = u32::try_from(random.below(3)).unwrap();
      let key = SymbolKey::new(LanguageFamily::Ts, repo("r"), package, module, items, EntityKind::Function, disambiguator).unwrap();
      let rendered = key.to_string();
      assert_eq!(rendered.parse::<SymbolKey>().unwrap_or_else(|error| panic!("{rendered:?}: {error}")), key);
    }
  }

  #[test]
  fn should_keep_kind_in_identity() {
    let class = symbol(LanguageFamily::Ts, "pkg", &["a"], &["Foo"], EntityKind::Class, 0);
    let interface = symbol(LanguageFamily::Ts, "pkg", &["a"], &["Foo"], EntityKind::Interface, 0);
    assert_ne!(class.to_string(), interface.to_string());
    let inherent = symbol(LanguageFamily::Rust, "pkg", &["a"], &["Foo", "fmt"], EntityKind::Method, 0);
    let trait_impl = symbol(LanguageFamily::Rust, "pkg", &["a"], &["Foo", "<Display>", "fmt"], EntityKind::Method, 0);
    assert_ne!(inherent.to_string(), trait_impl.to_string());
  }

  #[test]
  fn should_reject_malformed_symbol_keys() {
    let cases = [
      "",
      "rust:atlas:pkg:mod::item",
      "rust:atlas:pkg:mod#function",
      "rust:atlas:pkg#function",
      "go:atlas:pkg:mod::item#function",
      "rust:bad repo:pkg:mod::item#function",
      "rust:atlas::mod::item#function",
      "rust:atlas:pkg:a//b::item#function",
      "rust:atlas:pkg:mod::a::::b#function",
      "rust:atlas:pkg:mod::item#function~0",
      "rust:atlas:pkg:mod::item#function~01",
      "rust:atlas:pkg:mod::item#function~x",
      "rust:atlas:pkg:mod::item#Function",
      "rust:atlas:pkg:mod::it%6Dem#function",
      "rust:atlas:pkg:mod::it%3aem#function",
      "rust:atlas:pkg:mod::bad%ZZ#function",
      "rust:atlas:pkg:mod::bad%FF#function",
    ];
    for key in cases {
      assert!(key.parse::<SymbolKey>().is_err(), "{key:?} must be rejected");
    }
  }

  #[test]
  fn should_reject_empty_segments_and_packages_at_construction() {
    assert!(SymbolKey::new(LanguageFamily::Rust, repo("a"), "", vec![], vec!["x".into()], EntityKind::Function, 0).is_err());
    assert!(SymbolKey::new(LanguageFamily::Rust, repo("a"), "p", vec![String::new()], vec![], EntityKind::Module, 0).is_err());
    assert!(SymbolKey::new(LanguageFamily::Rust, repo("a"), "p", vec![], vec!["a".into(), String::new()], EntityKind::Function, 0).is_err());
  }

  #[test]
  fn should_name_symbols_by_their_innermost_segment() {
    assert_eq!(symbol(LanguageFamily::Rust, "p", &["m"], &["T", "f"], EntityKind::Method, 0).name(), "f");
    assert_eq!(symbol(LanguageFamily::Rust, "p", &["m", "inner"], &[], EntityKind::Module, 0).name(), "inner");
    assert_eq!(symbol(LanguageFamily::Rust, "p", &[], &[], EntityKind::Module, 0).name(), "p");
  }

  #[test]
  fn should_round_trip_every_domain_key_variant() {
    let cases = vec![
      (EntityKey::Repository { repo: repo("acme-api") }, "repo:acme-api"),
      (
        EntityKey::File {
          repo: repo("acme-api"),
          path: RelPath::new("src/main.ts").unwrap(),
        },
        "file:acme-api:src/main.ts",
      ),
      (
        EntityKey::File {
          repo: repo("acme-api"),
          path: RelPath::new("odd/100%:x.ts").unwrap(),
        },
        "file:acme-api:odd/100%25:x.ts",
      ),
      (
        EntityKey::Package {
          ecosystem: PackageEcosystem::Npm,
          name: "@acme/sdk".into(),
        },
        "pkg:npm:@acme/sdk",
      ),
      (
        EntityKey::Package {
          ecosystem: PackageEcosystem::Cargo,
          name: "atlas-core".into(),
        },
        "pkg:cargo:atlas-core",
      ),
      (
        EntityKey::Application {
          repo: repo("acme-web"),
          name: "web".into(),
        },
        "app:acme-web:web",
      ),
      (
        EntityKey::Service {
          repo: repo("acme-api"),
          name: "users".into(),
        },
        "service:acme-api:users",
      ),
      (
        EntityKey::Endpoint {
          repo: repo("acme-api"),
          method: HttpMethod::Get,
          route: "/users/{id}".into(),
        },
        "endpoint:acme-api:GET /users/{id}",
      ),
      (
        EntityKey::Endpoint {
          repo: repo("acme-api"),
          method: HttpMethod::Any,
          route: "/a b:c".into(),
        },
        "endpoint:acme-api:ANY /a b:c",
      ),
      (
        EntityKey::ClientCall {
          repo: repo("acme-web"),
          method: HttpMethod::Unknown,
          target: "{}/users/{}".into(),
        },
        "client:acme-web:UNKNOWN {}/users/{}",
      ),
      (
        EntityKey::AuthScheme {
          repo: repo("acme-api"),
          scheme: "bearer-jwt".into(),
        },
        "auth:acme-api:bearer-jwt",
      ),
      (
        EntityKey::Permission {
          repo: repo("acme-api"),
          name: "users:read".into(),
        },
        "permission:acme-api:users:read",
      ),
      (
        EntityKey::ConfigKey {
          repo: repo("acme-api"),
          key: "database.url".into(),
        },
        "config:acme-api:database.url",
      ),
      (
        EntityKey::EnvVar {
          repo: repo("acme-api"),
          name: "DATABASE_URL".into(),
        },
        "env:acme-api:DATABASE_URL",
      ),
      (EntityKey::Event { name: "user.created".into() }, "event:user.created"),
      (EntityKey::Queue { name: "emails".into() }, "queue:emails"),
      (
        EntityKey::Job {
          repo: repo("acme-billing"),
          name: "nightly-invoice".into(),
        },
        "job:acme-billing:nightly-invoice",
      ),
      (EntityKey::DbTable { name: "users".into() }, "table:users"),
      (EntityKey::ExternalService { name: "api.stripe.com".into() }, "external:api.stripe.com"),
      (EntityKey::Capability { name: "money formatting".into() }, "capability:money formatting"),
      (
        EntityKey::Other {
          kind: EntityKind::Custom("feature_flag".into()),
          repo: None,
          name: "new-ui".into(),
        },
        "other:feature_flag::new-ui",
      ),
      (
        EntityKey::Other {
          kind: EntityKind::Doc,
          repo: Some(repo("acme-api")),
          name: "README.md\n".into(),
        },
        "other:doc:acme-api:README.md%0A",
      ),
      (
        EntityKey::EnvVar {
          repo: repo("acme-api"),
          name: String::new(),
        },
        "env:acme-api:",
      ),
    ];
    for (key, expected) in cases {
      assert_eq!(key.to_string(), expected);
      assert_eq!(expected.parse::<EntityKey>().unwrap(), key, "{expected}");
      assert_eq!(serde_json::from_str::<EntityKey>(&serde_json::to_string(&key).unwrap()).unwrap(), key);
    }
  }

  #[test]
  fn should_reject_malformed_domain_keys() {
    let cases = [
      "nothing",
      "unknown:x",
      "repo:bad repo",
      "file:acme:../escape.ts",
      "file:acme:./src/a.ts",
      "pkg:pip:requests",
      "endpoint:acme:GET",
      "endpoint:acme:get /users",
      "env:DATABASE_URL",
      "env:acme:A%2",
      "env:acme:%41",
      "other:feature_flag:new-ui",
      "other:Bad:acme:x",
    ];
    for key in cases {
      assert!(key.parse::<EntityKey>().is_err(), "{key:?} must be rejected");
    }
  }

  #[test]
  fn should_expose_kind_repo_and_display_name_of_keys() {
    let endpoint = EntityKey::Endpoint {
      repo: repo("acme-api"),
      method: HttpMethod::Post,
      route: "/users".into(),
    };
    assert_eq!(endpoint.kind(), EntityKind::ApiEndpoint);
    assert_eq!(endpoint.repo().map(RepoName::as_str), Some("acme-api"));
    assert_eq!(endpoint.display_name(), "POST /users");
    let env = EntityKey::EnvVar {
      repo: repo("acme-api"),
      name: "PORT".into(),
    };
    assert_eq!(env.kind(), EntityKind::EnvVar);
    assert_eq!(env.repo().map(RepoName::as_str), Some("acme-api"));
    assert_eq!(env.display_name(), "PORT");
    let event = EntityKey::Event { name: "user.created".into() };
    assert_eq!(event.repo(), None);
    let method = symbol(LanguageFamily::Rust, "p", &["m"], &["T", "f"], EntityKind::Method, 0);
    assert_eq!(EntityKey::from(method.clone()).kind(), EntityKind::Method);
    assert_eq!(EntityKey::from(method).display_name(), "f");
  }

  #[test]
  fn should_name_rust_target_packages_without_colliding_with_cargo_names() {
    assert_eq!(RustTarget::Lib.package_field("atlas-cli"), "atlas-cli");
    assert_eq!(RustTarget::Bin("code-atlas".into()).package_field("atlas-cli"), "atlas-cli/bin/code-atlas");
    assert_eq!(RustTarget::Test("e2e".into()).package_field("atlas-cli"), "atlas-cli/test/e2e");
    assert_eq!(RustTarget::Example("demo".into()).package_field("x"), "x/example/demo");
    assert_eq!(RustTarget::Bench("hot".into()).package_field("x"), "x/bench/hot");
    assert_eq!(RustTarget::BuildScript.package_field("x"), "x/build");
  }

  #[test]
  fn should_derive_ts_module_paths_relative_to_the_package() {
    let file = |path: &str| RelPath::new(path).unwrap();
    let dir = file("packages/api");
    assert_eq!(
      ts_module_path(&file("packages/api/src/users/users.controller.ts"), Some(&dir)).unwrap(),
      ["src", "users", "users.controller"]
    );
    assert_eq!(ts_module_path(&file("packages/api/index.tsx"), Some(&dir)).unwrap(), ["index"]);
    assert_eq!(ts_module_path(&file("packages/api/types.d.ts"), Some(&dir)).unwrap(), ["types.d"]);
    assert_eq!(ts_module_path(&file("lib/util.mjs"), None).unwrap(), ["lib", "util"]);
    assert_eq!(ts_module_path(&file("README.md"), None).unwrap(), ["README.md"]);
    assert_eq!(ts_module_path(&file("packages/web/a.ts"), Some(&dir)), None);
  }

  #[test]
  fn should_normalise_route_templates_across_frameworks() {
    let cases = [
      ("/users/:id", "/users/{id}"),
      ("/users/{id}", "/users/{id}"),
      ("users/<int:id>/", "/users/{id}"),
      ("/blog/[slug]", "/blog/{slug}"),
      ("/docs/[...path]", "/docs/{path}"),
      ("//a///b/", "/a/b"),
      ("/users/:id(\\d+)", "/users/{id}"),
      ("/search?q=x#top", "/search"),
      ("/", "/"),
      ("", "/"),
      ("/files/*", "/files/*"),
      ("/Users/Me", "/Users/Me"),
    ];
    for (raw, expected) in cases {
      assert_eq!(normalize_route(raw), expected, "{raw}");
    }
  }

  #[test]
  fn should_parse_http_methods_strictly_and_leniently() {
    for method in HttpMethod::ALL {
      assert_eq!(method.as_str().parse::<HttpMethod>().unwrap(), method);
    }
    assert!("get".parse::<HttpMethod>().is_err());
    assert_eq!(HttpMethod::parse_lenient("get"), Some(HttpMethod::Get));
    assert_eq!(HttpMethod::parse_lenient("PROPFIND"), None);
  }
}
