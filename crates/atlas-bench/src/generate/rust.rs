use super::UnitCtx;
use super::model::{Callee, Dependency, Endpoint, FuncSpec, Handler, ImportMap, ModuleSpec, Stmt, Verb, imports_of, line};

fn render_imports(out: &mut String, imports: &ImportMap, ctx: &UnitCtx) {
  for ((origin, group, module), names) in imports {
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let root = match origin {
      Dependency::Local => "crate".to_owned(),
      Dependency::Dep => ctx.dep.as_ref().map(|dep| dep.ident.clone()).unwrap_or_default(),
    };
    line!(out, "use {root}::g{group}::m{module}::{{{}}};", names.join(", "));
  }
}

fn render_stmts(out: &mut String, stmts: &[Stmt], indent: &str) {
  for (index, stmt) in stmts.iter().enumerate() {
    match stmt {
      Stmt::Call { callee, constant } => {
        line!(out, "{indent}let v{index} = {}(acc.wrapping_add({constant}));", callee.target().name);
        line!(out, "{indent}acc = acc.wrapping_mul(31).wrapping_add(v{index});");
      }
      Stmt::Arith { mul, add } => line!(out, "{indent}acc = acc.wrapping_mul({mul}).wrapping_add({add});"),
      Stmt::Loop { count, constant } => {
        line!(out, "{indent}for i in 0..{count} {{");
        line!(out, "{indent}  acc = acc.wrapping_add(i64::from(i) ^ {constant});");
        line!(out, "{indent}}}");
      }
      Stmt::Branch { modulus, low, high } => {
        line!(out, "{indent}if acc % {modulus} == 0 {{");
        line!(out, "{indent}  acc = acc.wrapping_sub({low});");
        line!(out, "{indent}}} else {{");
        line!(out, "{indent}  acc = acc.wrapping_add({high});");
        line!(out, "{indent}}}");
      }
    }
  }
}

fn render_function(out: &mut String, func: &FuncSpec) {
  line!(out, "pub fn {}(input: i64) -> i64 {{", func.name);
  line!(out, "  let mut acc = input;");
  render_stmts(out, &func.stmts, "  ");
  line!(out, "  acc");
  line!(out, "}}");
}

pub fn module(spec: &ModuleSpec, ctx: &UnitCtx) -> String {
  let mut out = String::new();
  let imports = spec.imports();
  render_imports(&mut out, &imports, ctx);
  if !imports.is_empty() {
    line!(out);
  }
  let ty = &spec.ty.name;
  line!(out, "#[derive(Debug, Clone, PartialEq)]");
  line!(out, "pub struct {ty} {{");
  line!(out, "  pub id: u64,");
  line!(out, "  pub seed: i64,");
  line!(out, "}}");
  line!(out);
  line!(out, "impl {ty} {{");
  line!(out, "  pub fn new(id: u64, seed: i64) -> Self {{");
  line!(out, "    Self {{ id, seed }}");
  line!(out, "  }}");
  line!(out);
  line!(out, "  pub fn describe(&self) -> String {{");
  line!(out, "    format!(\"{ty}#{{}}\", self.id)");
  line!(out, "  }}");
  for method in &spec.ty.methods {
    line!(out);
    line!(out, "  pub fn {}(&self, input: i64) -> i64 {{", method.name);
    line!(out, "    let mut acc = input ^ self.seed;");
    render_stmts(&mut out, &method.stmts, "    ");
    line!(out, "    acc");
    line!(out, "  }}");
  }
  line!(out, "}}");
  for func in &spec.funcs {
    line!(out);
    render_function(&mut out, func);
  }
  line!(out);
  line!(out, "#[cfg(test)]");
  line!(out, "mod tests {{");
  line!(out, "  use super::*;");
  for func in spec.funcs.iter().take(2) {
    line!(out);
    line!(out, "  #[test]");
    line!(out, "  fn should_be_deterministic_for_{}() {{", func.name);
    line!(out, "    assert_eq!({0}(7), {0}(7));", func.name);
    line!(out, "  }}");
  }
  line!(out, "}}");
  out
}

pub fn group_mod(modules: usize) -> String {
  let mut out = String::new();
  for index in 0..modules {
    line!(out, "pub mod m{index};");
  }
  line!(out, "pub mod routes;");
  out
}

fn handler_body(out: &mut String, handler: &Handler) {
  let calls: Vec<String> = handler.callees.iter().map(|callee| callee.target().name.clone()).collect();
  let (first, second) = (
    calls.first().map(String::as_str).unwrap_or("i64::from"),
    calls.get(1).map(String::as_str).unwrap_or("i64::from"),
  );
  match (handler.endpoint.verb, handler.endpoint.with_id) {
    (Verb::Get, false) => {
      line!(out, "  let total = {first}(3).wrapping_add({second}(5));");
      line!(out, "  Json(total)");
    }
    (_, true) => {
      line!(out, "  let value = {first}(id).wrapping_add({second}(id));");
      line!(out, "  Json(value)");
    }
    (_, false) => {
      line!(out, "  let created = {first}(body);");
      line!(out, "  Json(created)");
    }
  }
}

pub fn routes(handlers: &[Handler], ctx: &UnitCtx) -> String {
  let mut out = String::new();
  line!(out, "use axum::extract::Path;");
  line!(out, "use axum::routing::{{delete, get, post}};");
  line!(out, "use axum::{{Json, Router}};");
  let callees: Vec<&Callee> = handlers.iter().flat_map(|handler| handler.callees.iter()).collect();
  let imports = imports_of(callees.into_iter(), None);
  line!(out);
  render_imports(&mut out, &imports, ctx);
  for handler in handlers {
    let endpoint = &handler.endpoint;
    line!(out);
    let name = endpoint.handler_name(super::model::Lang::Rust);
    match (endpoint.verb, endpoint.with_id) {
      (_, true) => line!(out, "pub async fn {name}(Path(id): Path<i64>) -> Json<i64> {{"),
      (Verb::Get, false) => line!(out, "pub async fn {name}() -> Json<i64> {{"),
      (_, false) => line!(out, "pub async fn {name}(Json(body): Json<i64>) -> Json<i64> {{"),
    }
    handler_body(&mut out, handler);
    line!(out, "}}");
  }
  line!(out);
  line!(out, "pub fn router() -> Router {{");
  line!(out, "  Router::new()");
  for handler in handlers {
    let endpoint = &handler.endpoint;
    line!(
      out,
      "    .route(\"{}\", {}({}))",
      endpoint.path(|| "{id}".to_owned()),
      endpoint.verb.lower(),
      endpoint.handler_name(super::model::Lang::Rust)
    );
  }
  line!(out, "}}");
  out
}

pub fn lib(groups: &[usize], has_client: bool) -> String {
  let mut out = String::new();
  if has_client {
    line!(out, "pub mod client;");
  }
  for group in 0..groups.len() {
    line!(out, "pub mod g{group};");
  }
  line!(out);
  line!(out, "use axum::Router;");
  line!(out);
  line!(out, "pub fn router() -> Router {{");
  line!(out, "  Router::new()");
  for group in 0..groups.len() {
    line!(out, "    .merge(g{group}::routes::router())");
  }
  line!(out, "}}");
  out
}

pub fn client(ctx: &UnitCtx, endpoints: &[Endpoint]) -> String {
  let mut out = String::new();
  line!(out, "#[derive(Clone)]");
  line!(out, "pub struct Client {{");
  line!(out, "  http: reqwest::Client,");
  line!(out, "  base_url: String,");
  line!(out, "}}");
  line!(out);
  line!(out, "impl Client {{");
  line!(out, "  pub fn new(base_url: String) -> Self {{");
  line!(out, "    Self {{ http: reqwest::Client::new(), base_url }}");
  line!(out, "  }}");
  line!(out);
  line!(out, "  pub fn from_env() -> Result<Self, std::env::VarError> {{");
  line!(out, "    let base_url = std::env::var(\"{}_API_URL\")?;", ctx.env_prefix);
  line!(out, "    Ok(Self::new(base_url))");
  line!(out, "  }}");
  for endpoint in endpoints {
    let name = format!("{}_{}", endpoint.handler_name(super::model::Lang::Rust), endpoint.slug.replace('-', "_"));
    let args = match (endpoint.verb, endpoint.with_id) {
      (_, true) => ", id: &str",
      (Verb::Post, false) => ", body: &str",
      _ => "",
    };
    line!(out);
    line!(out, "  pub async fn {name}(&self{args}) -> Result<String, reqwest::Error> {{");
    let template = endpoint.path(|| "{}".to_owned());
    match endpoint.with_id {
      true => line!(out, "    let url = format!(\"{{}}{template}\", self.base_url, id);"),
      false => line!(out, "    let url = format!(\"{{}}{template}\", self.base_url);"),
    }
    match (endpoint.verb, endpoint.with_id) {
      (Verb::Post, false) => line!(out, "    let response = self.http.post(url).body(body.to_owned()).send().await?.error_for_status()?;"),
      (verb, _) => line!(out, "    let response = self.http.{}(url).send().await?.error_for_status()?;", verb.lower()),
    }
    line!(out, "    response.text().await");
    line!(out, "  }}");
  }
  line!(out, "}}");
  out
}

pub fn manifest(ctx: &UnitCtx) -> String {
  let mut out = String::new();
  line!(out, "[package]");
  line!(out, "name = \"{}\"", ctx.package);
  line!(out, "version = \"0.1.0\"");
  line!(out, "edition = \"2021\"");
  line!(out);
  line!(out, "[workspace]");
  line!(out);
  line!(out, "[lib]");
  line!(out, "name = \"{}\"", ctx.ident);
  line!(out, "path = \"src/lib.rs\"");
  line!(out);
  line!(out, "[dependencies]");
  line!(out, "axum = \"0.8\"");
  line!(out, "reqwest = {{ version = \"0.12\", default-features = false, features = [\"json\", \"rustls-tls\"] }}");
  line!(out, "serde_json = \"1\"");
  line!(out, "tokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}");
  if let Some(dep) = &ctx.dep {
    line!(out, "{} = {{ path = \"{}\" }}", dep.package, dep.rel_path);
  }
  out
}

pub fn smoke_test(ctx: &UnitCtx, first: &str) -> String {
  let mut out = String::new();
  line!(out, "use {}::g0::m0::{first};", ctx.ident);
  line!(out);
  line!(out, "#[test]");
  line!(out, "fn should_compute_{first}_deterministically() {{");
  line!(out, "  assert_eq!({first}(1), {first}(1));");
  line!(out, "}}");
  out
}
