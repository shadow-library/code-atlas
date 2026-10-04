use super::UnitCtx;
use super::model::{Callee, Dependency, Endpoint, FuncSpec, Handler, ImportMap, Lang, ModuleSpec, Stmt, Verb, imports_of, line};

const MODULUS: u32 = 1_000_003;

fn render_imports(out: &mut String, imports: &ImportMap, ctx: &UnitCtx, own_group: usize) {
  for ((origin, group, module), names) in imports {
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let from = match origin {
      Dependency::Dep => ctx.dep.as_ref().map(|dep| dep.package.clone()).unwrap_or_default(),
      Dependency::Local if *group == own_group => format!("./m{module}"),
      Dependency::Local => format!("../g{group}/m{module}"),
    };
    line!(out, "import {{ {} }} from '{from}';", names.join(", "));
  }
}

fn render_stmts(out: &mut String, stmts: &[Stmt], indent: &str) {
  for (index, stmt) in stmts.iter().enumerate() {
    match stmt {
      Stmt::Call { callee, constant } => {
        line!(out, "{indent}const v{index} = {}(acc + {constant});", callee.target().name);
        line!(out, "{indent}acc = (acc * 31 + v{index}) % {MODULUS};");
      }
      Stmt::Arith { mul, add } => line!(out, "{indent}acc = (acc * {mul} + {add}) % {MODULUS};"),
      Stmt::Loop { count, constant } => {
        line!(out, "{indent}for (let i = 0; i < {count}; i++) {{");
        line!(out, "{indent}  acc = (acc + (i ^ {constant})) % {MODULUS};");
        line!(out, "{indent}}}");
      }
      Stmt::Branch { modulus, low, high } => {
        line!(out, "{indent}if (acc % {modulus} === 0) {{");
        line!(out, "{indent}  acc -= {low};");
        line!(out, "{indent}}} else {{");
        line!(out, "{indent}  acc += {high};");
        line!(out, "{indent}}}");
      }
    }
  }
}

fn render_function(out: &mut String, func: &FuncSpec) {
  line!(out, "export function {}(input: number): number {{", func.name);
  line!(out, "  let acc = input;");
  render_stmts(out, &func.stmts, "  ");
  line!(out, "  return acc;");
  line!(out, "}}");
}

pub fn module(spec: &ModuleSpec, ctx: &UnitCtx) -> String {
  let mut out = String::new();
  let imports = spec.imports();
  render_imports(&mut out, &imports, ctx, spec.group);
  if !imports.is_empty() {
    line!(out);
  }
  let ty = &spec.ty.name;
  line!(out, "export class {ty} {{");
  line!(out, "  constructor(");
  line!(out, "    readonly id: number,");
  line!(out, "    private readonly seed: number,");
  line!(out, "  ) {{}}");
  line!(out);
  line!(out, "  describe(): string {{");
  line!(out, "    return `{ty}#${{this.id}}`;");
  line!(out, "  }}");
  for method in &spec.ty.methods {
    line!(out);
    line!(out, "  {}(input: number): number {{", method.name);
    line!(out, "    let acc = input ^ this.seed;");
    render_stmts(&mut out, &method.stmts, "    ");
    line!(out, "    return acc;");
    line!(out, "  }}");
  }
  line!(out, "}}");
  for func in &spec.funcs {
    line!(out);
    render_function(&mut out, func);
  }
  out
}

pub fn module_test(spec: &ModuleSpec) -> String {
  let mut out = String::new();
  let names: Vec<&str> = spec.funcs.iter().take(2).map(|func| func.name.as_str()).collect();
  line!(out, "import {{ describe, expect, it }} from 'vitest';");
  line!(out);
  line!(out, "import {{ {} }} from './m{}';", names.join(", "), spec.index);
  line!(out);
  line!(out, "describe('m{}', () => {{", spec.index);
  for name in names {
    line!(out, "  it('should be deterministic for {name}', () => {{");
    line!(out, "    expect({name}(7)).toBe({name}(7));");
    line!(out, "  }});");
  }
  line!(out, "}});");
  out
}

pub fn routes(group: usize, handlers: &[Handler], ctx: &UnitCtx) -> String {
  let mut out = String::new();
  line!(out, "import {{ type Express }} from 'express';");
  let callees: Vec<&Callee> = handlers.iter().flat_map(|handler| handler.callees.iter()).collect();
  let imports = imports_of(callees.into_iter(), None);
  line!(out);
  render_imports(&mut out, &imports, ctx, usize::MAX);
  line!(out);
  line!(out, "export function registerG{group}Routes(app: Express): void {{");
  for (index, handler) in handlers.iter().enumerate() {
    if index > 0 {
      line!(out);
    }
    let endpoint = &handler.endpoint;
    let calls: Vec<String> = handler.callees.iter().map(|callee| callee.target().name.clone()).collect();
    let (first, second) = (calls.first().cloned().unwrap_or_default(), calls.get(1).cloned().unwrap_or_default());
    line!(out, "  app.{}('{}', (req, res) => {{", endpoint.verb.lower(), endpoint.path(|| ":id".to_owned()));
    match (endpoint.verb, endpoint.with_id) {
      (Verb::Get, false) => {
        line!(out, "    const total = {first}(3) + {second}(5);");
        line!(out, "    return res.json({{ total }});");
      }
      (_, true) => {
        line!(out, "    const value = {first}(Number(req.params.id)) + {second}(1);");
        line!(out, "    return res.json({{ value }});");
      }
      (_, false) => {
        line!(out, "    const created = {first}(Number(req.body.amount));");
        line!(out, "    return res.status(201).json({{ created }});");
      }
    }
    line!(out, "  }});");
  }
  line!(out, "}}");
  out
}

pub fn server(groups: usize) -> String {
  let mut out = String::new();
  line!(out, "import express from 'express';");
  line!(out);
  for group in 0..groups {
    line!(out, "import {{ registerG{group}Routes }} from './g{group}/routes';");
  }
  line!(out);
  line!(out, "const app = express();");
  line!(out, "app.use(express.json());");
  line!(out);
  for group in 0..groups {
    line!(out, "registerG{group}Routes(app);");
  }
  line!(out);
  line!(out, "app.listen(Number(process.env.PORT ?? 3000));");
  out
}

pub fn barrel(api_modules: &[(usize, usize)], has_client: bool) -> String {
  let mut out = String::new();
  for (group, module) in api_modules {
    line!(out, "export * from './g{group}/m{module}';");
  }
  if has_client {
    line!(out, "export * from './client';");
  }
  out
}

pub fn client(ctx: &UnitCtx, endpoints: &[Endpoint]) -> String {
  let mut out = String::new();
  line!(out, "export class Client {{");
  line!(out, "  constructor(private readonly baseUrl: string = process.env.{}_API_URL ?? '') {{}}", ctx.env_prefix);
  for endpoint in endpoints {
    let name = format!("{}{}", endpoint.handler_name(Lang::Ts), endpoint.slug.to_uppercase());
    let args = match (endpoint.verb, endpoint.with_id) {
      (_, true) => "id: string",
      (Verb::Post, false) => "body: unknown",
      _ => "",
    };
    line!(out);
    line!(out, "  async {name}({args}): Promise<unknown> {{");
    let path = endpoint.path(|| "${id}".to_owned());
    let options = match (endpoint.verb, endpoint.with_id) {
      (Verb::Post, false) => "{ method: 'POST', body: JSON.stringify(body) }".to_owned(),
      (verb, _) => format!("{{ method: '{}' }}", verb.upper()),
    };
    line!(out, "    const response = await fetch(`${{this.baseUrl}}{path}`, {options});");
    line!(out, "    if (!response.ok) throw new Error(`request failed with ${{response.status}}`);");
    line!(out, "    return response.json();");
    line!(out, "  }}");
  }
  line!(out, "}}");
  out
}

pub fn manifest(ctx: &UnitCtx) -> String {
  let mut deps = vec!["    \"express\": \"^4.19.2\"".to_owned()];
  if let Some(dep) = &ctx.dep {
    deps.push(format!("    \"{}\": \"file:{}\"", dep.package, dep.rel_path));
  }
  let mut out = String::new();
  line!(out, "{{");
  line!(out, "  \"name\": \"{}\",", ctx.package);
  line!(out, "  \"version\": \"0.1.0\",");
  line!(out, "  \"private\": true,");
  line!(out, "  \"type\": \"module\",");
  line!(out, "  \"main\": \"src/index.ts\",");
  line!(out, "  \"scripts\": {{");
  line!(out, "    \"build\": \"tsc -p .\",");
  line!(out, "    \"test\": \"vitest run\"");
  line!(out, "  }},");
  line!(out, "  \"dependencies\": {{");
  line!(out, "{}", deps.join(",\n"));
  line!(out, "  }},");
  line!(out, "  \"devDependencies\": {{");
  line!(out, "    \"typescript\": \"^5.5.0\",");
  line!(out, "    \"vitest\": \"^2.0.5\"");
  line!(out, "  }}");
  line!(out, "}}");
  out
}

pub fn tsconfig() -> String {
  let mut out = String::new();
  line!(out, "{{");
  line!(out, "  \"compilerOptions\": {{");
  line!(out, "    \"module\": \"ESNext\",");
  line!(out, "    \"moduleResolution\": \"Bundler\",");
  line!(out, "    \"target\": \"ES2022\",");
  line!(out, "    \"strict\": true,");
  line!(out, "    \"skipLibCheck\": true");
  line!(out, "  }},");
  line!(out, "  \"include\": [\"src\"]");
  line!(out, "}}");
  out
}
