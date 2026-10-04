use std::collections::{BTreeMap, BTreeSet};

macro_rules! line {
  ($out:expr) => {
    $out.push('\n')
  };
  ($out:expr, $($arg:tt)*) => {{
    $out.push_str(&format!($($arg)*));
    $out.push('\n');
  }};
}
pub(crate) use line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
  Rust,
  Ts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnRef {
  pub name: String,
  pub group: usize,
  pub module: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
  Local(FnRef),
  Dep(FnRef),
}

impl Callee {
  pub fn target(&self) -> &FnRef {
    match self {
      Self::Local(target) | Self::Dep(target) => target,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
  Call { callee: Callee, constant: u32 },
  Arith { mul: u32, add: u32 },
  Loop { count: u32, constant: u32 },
  Branch { modulus: u32, low: u32, high: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncSpec {
  pub name: String,
  pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSpec {
  pub name: String,
  pub methods: Vec<FuncSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSpec {
  pub group: usize,
  pub index: usize,
  pub funcs: Vec<FuncSpec>,
  pub ty: TypeSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
  Get,
  Post,
  Delete,
}

impl Verb {
  pub fn upper(self) -> &'static str {
    match self {
      Self::Get => "GET",
      Self::Post => "POST",
      Self::Delete => "DELETE",
    }
  }

  pub fn lower(self) -> &'static str {
    match self {
      Self::Get => "get",
      Self::Post => "post",
      Self::Delete => "delete",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
  pub verb: Verb,
  pub slug: String,
  pub resource: String,
  pub with_id: bool,
}

impl Endpoint {
  pub fn path(&self, param: impl Fn() -> String) -> String {
    let base = format!("/api/v1/{}/{}", self.slug, self.resource);
    if self.with_id { format!("{base}/{}", param()) } else { base }
  }

  pub fn handler_name(&self, lang: Lang) -> String {
    let verb = match self.verb {
      Verb::Get if self.with_id => "get",
      Verb::Get => "list",
      Verb::Post => "create",
      Verb::Delete => "remove",
    };
    match lang {
      Lang::Rust => format!("{verb}_{}", self.resource),
      Lang::Ts => format!("{verb}{}{}", self.resource[..1].to_uppercase(), &self.resource[1..]),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
  pub endpoint: Endpoint,
  pub callees: Vec<Callee>,
}

pub type ImportMap = BTreeMap<(Dependency, usize, usize), BTreeSet<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependency {
  Local,
  Dep,
}

pub fn imports_of<'a>(callees: impl Iterator<Item = &'a Callee>, own: Option<(usize, usize)>) -> ImportMap {
  let mut imports = ImportMap::new();
  for callee in callees {
    let (origin, target) = match callee {
      Callee::Local(target) => (Dependency::Local, target),
      Callee::Dep(target) => (Dependency::Dep, target),
    };
    if origin == Dependency::Local && own == Some((target.group, target.module)) {
      continue;
    }
    imports.entry((origin, target.group, target.module)).or_default().insert(target.name.clone());
  }
  imports
}

impl ModuleSpec {
  pub fn callees(&self) -> impl Iterator<Item = &Callee> {
    self.funcs.iter().chain(self.ty.methods.iter()).flat_map(|func| {
      func.stmts.iter().filter_map(|stmt| match stmt {
        Stmt::Call { callee, .. } => Some(callee),
        _ => None,
      })
    })
  }

  pub fn imports(&self) -> ImportMap {
    imports_of(self.callees(), Some((self.group, self.index)))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn call(name: &str, group: usize, module: usize, dep: bool) -> Stmt {
    let target = FnRef {
      name: name.to_owned(),
      group,
      module,
    };
    Stmt::Call {
      callee: if dep { Callee::Dep(target) } else { Callee::Local(target) },
      constant: 100,
    }
  }

  #[test]
  fn should_skip_imports_for_callees_in_the_same_module() {
    let spec = ModuleSpec {
      group: 1,
      index: 2,
      funcs: vec![FuncSpec {
        name: "own".to_owned(),
        stmts: vec![call("sibling", 1, 2, false), call("other", 0, 3, false), call("remote", 0, 1, true)],
      }],
      ty: TypeSpec {
        name: "Thing".to_owned(),
        methods: Vec::new(),
      },
    };
    let imports = spec.imports();
    assert_eq!(imports.len(), 2);
    assert!(imports[&(Dependency::Local, 0, 3)].contains("other"));
    assert!(imports[&(Dependency::Dep, 0, 1)].contains("remote"));
  }

  #[test]
  fn should_render_endpoint_paths_and_handler_names() {
    let endpoint = Endpoint {
      verb: Verb::Get,
      slug: "r0".to_owned(),
      resource: "orders3".to_owned(),
      with_id: true,
    };
    assert_eq!(endpoint.path(|| ":id".to_owned()), "/api/v1/r0/orders3/:id");
    assert_eq!(endpoint.handler_name(Lang::Rust), "get_orders3");
    assert_eq!(endpoint.handler_name(Lang::Ts), "getOrders3");
  }
}
