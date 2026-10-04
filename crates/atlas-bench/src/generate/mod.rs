mod model;
mod rust;
mod ts;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::error::{BenchError, BenchResult};
use crate::rng::Rng;

use model::{Callee, Endpoint, FnRef, FuncSpec, Handler, ModuleSpec, Stmt, TypeSpec, Verb};

pub use model::Lang;

const MODULES_PER_GROUP: usize = 8;
const API_MODULES: usize = 4;
const DEP_CALL_PERCENT: usize = 6;
const DUPLICATE_PERCENT: usize = 4;
const MIN_REGISTRY_FOR_DUPLICATES: usize = 20;
const MAX_CLIENT_CALLS: usize = 10;
const VERBS: [&str; 16] = [
  "compute",
  "parse",
  "build",
  "resolve",
  "validate",
  "render",
  "fetch",
  "merge",
  "apply",
  "reduce",
  "encode",
  "decode",
  "collect",
  "normalize",
  "schedule",
  "dispatch",
];
const NOUNS: [&str; 16] = [
  "order", "user", "invoice", "ledger", "session", "token", "quota", "route", "report", "widget", "cursor", "packet", "record", "policy", "account", "catalog",
];
const METHODS: [&str; 2] = ["apply", "refine"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LangChoice {
  Rust,
  Ts,
  Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenConfig {
  pub loc: u64,
  pub repos: usize,
  pub lang: LangChoice,
  pub seed: u64,
  pub projects: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlantedDuplicate {
  pub repo: String,
  pub path: String,
  pub name: String,
  pub original: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoOutput {
  pub name: String,
  pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOutput {
  pub name: String,
  pub root: PathBuf,
  pub repos: Vec<RepoOutput>,
  pub planted: Vec<PlantedDuplicate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenSummary {
  pub projects: Vec<ProjectOutput>,
  pub files: u64,
  pub loc: u64,
  pub tree_hash: String,
}

#[derive(Debug, Clone)]
struct DepCtx {
  ident: String,
  package: String,
  rel_path: String,
}

#[derive(Debug, Clone)]
struct UnitCtx {
  ident: String,
  package: String,
  slug: String,
  env_prefix: String,
  dep: Option<DepCtx>,
}

#[derive(Debug, Clone)]
struct UnitPlan {
  repo: String,
  subdir: Option<&'static str>,
  lang: Lang,
}

impl UnitPlan {
  fn rel_dir(&self) -> String {
    match self.subdir {
      Some(subdir) => format!("{}/{subdir}", self.repo),
      None => self.repo.clone(),
    }
  }

  fn package(&self) -> String {
    match self.subdir {
      Some(subdir) => format!("{}-{subdir}", self.repo),
      None => self.repo.clone(),
    }
  }
}

struct Writer {
  root: PathBuf,
  files: u64,
  loc: u64,
}

impl Writer {
  fn write(&mut self, rel: &str, content: &str) -> BenchResult<()> {
    let path = self.root.join(rel);
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).map_err(|source| BenchError::io("create", parent, source))?;
    }
    fs::write(&path, content).map_err(|source| BenchError::io("write", &path, source))?;
    self.files += 1;
    self.loc += content.lines().count() as u64;
    Ok(())
  }
}

struct UnitOutput {
  api_fns: Vec<FnRef>,
  endpoints: Vec<Endpoint>,
}

struct UnitState {
  rng: Rng,
  lang: Lang,
  repo: String,
  rel_dir: String,
  registry: Vec<FnRef>,
  specs: Vec<FuncSpec>,
  dep_fns: Vec<FnRef>,
  counter: usize,
  planted: Vec<PlantedDuplicate>,
}

impl UnitState {
  fn function_name(&mut self) -> String {
    self.counter += 1;
    let verb = *self.rng.pick(&VERBS);
    let noun = *self.rng.pick(&NOUNS);
    match self.lang {
      Lang::Rust => format!("{verb}_{noun}_{}", self.counter),
      Lang::Ts => format!("{verb}{}{}{}", noun[..1].to_uppercase(), &noun[1..], self.counter),
    }
  }

  fn type_name(&mut self) -> String {
    self.counter += 1;
    let noun = *self.rng.pick(&NOUNS);
    format!("{}{}{}", noun[..1].to_uppercase(), &noun[1..], self.counter)
  }

  fn pick_callee(&mut self) -> Option<Callee> {
    if !self.dep_fns.is_empty() && self.rng.chance(DEP_CALL_PERCENT) {
      return Some(Callee::Dep(self.rng.pick(&self.dep_fns).clone()));
    }
    if self.registry.is_empty() {
      return None;
    }
    let index = self.rng.skewed(self.registry.len());
    Some(Callee::Local(self.registry[index].clone()))
  }

  fn statement(&mut self) -> Stmt {
    let roll = self.rng.below(100);
    match roll {
      0..45 => match self.pick_callee() {
        Some(callee) => Stmt::Call {
          callee,
          constant: self.rng.constant(),
        },
        None => self.arith(),
      },
      45..70 => self.arith(),
      70..85 => Stmt::Loop {
        count: self.rng.range(2, 9) as u32,
        constant: self.rng.constant(),
      },
      _ => Stmt::Branch {
        modulus: self.rng.range(2, 9) as u32,
        low: self.rng.constant(),
        high: self.rng.constant(),
      },
    }
  }

  fn arith(&mut self) -> Stmt {
    Stmt::Arith {
      mul: self.rng.constant(),
      add: self.rng.constant(),
    }
  }

  fn statements(&mut self, low: usize, high: usize) -> Vec<Stmt> {
    let count = self.rng.range(low, high);
    (0..count).map(|_| self.statement()).collect()
  }

  fn duplicate_of(&mut self) -> Option<FuncSpec> {
    if self.specs.len() < MIN_REGISTRY_FOR_DUPLICATES || !self.rng.chance(DUPLICATE_PERCENT) {
      return None;
    }
    let donor = (0..8).map(|_| self.rng.below(self.specs.len())).find(|index| self.specs[*index].stmts.len() >= 5)?;
    let original = self.specs[donor].clone();
    let mut stmts = original.stmts.clone();
    for stmt in stmts.iter_mut().take(2) {
      match stmt {
        Stmt::Call { constant, .. } | Stmt::Loop { constant, .. } => *constant = constant.wrapping_add(7),
        Stmt::Arith { add, .. } => *add = add.wrapping_add(7),
        Stmt::Branch { high, .. } => *high = high.wrapping_add(7),
      }
    }
    let name = self.function_name();
    self.planted.push(PlantedDuplicate {
      repo: self.repo.clone(),
      path: String::new(),
      name: name.clone(),
      original: original.name,
    });
    Some(FuncSpec { name, stmts })
  }

  fn plan_module(&mut self, group: usize, index: usize) -> ModuleSpec {
    let planted_before = self.planted.len();
    let count = self.rng.range(6, 12);
    let mut funcs = Vec::with_capacity(count);
    for _ in 0..count {
      let func = match self.duplicate_of() {
        Some(duplicate) => duplicate,
        None => {
          let stmts = self.statements(3, 9);
          FuncSpec {
            name: self.function_name(),
            stmts,
          }
        }
      };
      self.registry.push(FnRef {
        name: func.name.clone(),
        group,
        module: index,
      });
      self.specs.push(func.clone());
      funcs.push(func);
    }
    let extension = match self.lang {
      Lang::Rust => "rs",
      Lang::Ts => "ts",
    };
    let path = format!("{}/src/g{group}/m{index}.{extension}", self.rel_dir);
    for planted in &mut self.planted[planted_before..] {
      planted.path = path.clone();
    }
    let methods = METHODS
      .iter()
      .map(|name| FuncSpec {
        name: (*name).to_owned(),
        stmts: self.statements(2, 5),
      })
      .collect();
    ModuleSpec {
      group,
      index,
      funcs,
      ty: TypeSpec { name: self.type_name(), methods },
    }
  }

  fn plan_handlers(&mut self, group: usize, slug: &str) -> Vec<Handler> {
    let noun = *self.rng.pick(&NOUNS);
    let resource = format!("{noun}{group}");
    let mut endpoints = vec![(Verb::Get, false), (Verb::Get, true), (Verb::Post, false)];
    if self.rng.chance(35) {
      endpoints.push((Verb::Delete, true));
    }
    endpoints
      .into_iter()
      .map(|(verb, with_id)| Handler {
        endpoint: Endpoint {
          verb,
          slug: slug.to_owned(),
          resource: resource.clone(),
          with_id,
        },
        callees: (0..2).filter_map(|_| self.pick_callee()).collect(),
      })
      .collect()
  }
}

fn generate_unit(plan: &UnitPlan, ctx: &UnitCtx, budget: u64, state: &mut UnitState, earlier: &[Endpoint], writer: &mut Writer) -> BenchResult<UnitOutput> {
  let dir = plan.rel_dir();
  let started = writer.loc;
  let mut groups: Vec<usize> = Vec::new();
  let mut endpoints: Vec<Endpoint> = Vec::new();
  let mut first_function: Option<String> = None;
  let mut spent = false;
  while !spent {
    let group = groups.len();
    let mut modules = 0;
    for index in 0..MODULES_PER_GROUP {
      let spec = state.plan_module(group, index);
      first_function.get_or_insert_with(|| spec.funcs[0].name.clone());
      match plan.lang {
        Lang::Rust => writer.write(&format!("{dir}/src/g{group}/m{index}.rs"), &rust::module(&spec, ctx))?,
        Lang::Ts => {
          writer.write(&format!("{dir}/src/g{group}/m{index}.ts"), &ts::module(&spec, ctx))?;
          if index % 3 == 0 {
            writer.write(&format!("{dir}/src/g{group}/m{index}.test.ts"), &ts::module_test(&spec))?;
          }
        }
      }
      modules += 1;
      spent = writer.loc - started >= budget;
      if spent {
        break;
      }
    }
    let handlers = state.plan_handlers(group, &ctx.slug);
    endpoints.extend(handlers.iter().map(|handler| handler.endpoint.clone()));
    match plan.lang {
      Lang::Rust => {
        writer.write(&format!("{dir}/src/g{group}/routes.rs"), &rust::routes(&handlers, ctx))?;
        writer.write(&format!("{dir}/src/g{group}/mod.rs"), &rust::group_mod(modules))?;
      }
      Lang::Ts => writer.write(&format!("{dir}/src/g{group}/routes.ts"), &ts::routes(group, &handlers, ctx))?,
    }
    groups.push(modules);
  }
  let client_calls = pick_client_calls(&mut state.rng, earlier);
  let has_client = !client_calls.is_empty();
  match plan.lang {
    Lang::Rust => {
      writer.write(&format!("{dir}/Cargo.toml"), &rust::manifest(ctx))?;
      writer.write(&format!("{dir}/src/lib.rs"), &rust::lib(&groups, has_client))?;
      if has_client {
        writer.write(&format!("{dir}/src/client.rs"), &rust::client(ctx, &client_calls))?;
      }
      writer.write(&format!("{dir}/tests/smoke.rs"), &rust::smoke_test(ctx, first_function.as_deref().unwrap_or("compute")))?;
    }
    Lang::Ts => {
      let api: Vec<(usize, usize)> = (0..groups[0].min(API_MODULES)).map(|module| (0, module)).collect();
      writer.write(&format!("{dir}/package.json"), &ts::manifest(ctx))?;
      writer.write(&format!("{dir}/tsconfig.json"), &ts::tsconfig())?;
      writer.write(&format!("{dir}/src/index.ts"), &ts::barrel(&api, has_client))?;
      writer.write(&format!("{dir}/src/server.ts"), &ts::server(groups.len()))?;
      if has_client {
        writer.write(&format!("{dir}/src/client.ts"), &ts::client(ctx, &client_calls))?;
      }
    }
  }
  let api_fns = state.registry.iter().filter(|func| func.group == 0 && func.module < API_MODULES).cloned().collect();
  Ok(UnitOutput { api_fns, endpoints })
}

fn pick_client_calls(rng: &mut Rng, earlier: &[Endpoint]) -> Vec<Endpoint> {
  if earlier.is_empty() {
    return Vec::new();
  }
  let wanted = earlier.len().min(MAX_CLIENT_CALLS);
  let mut chosen: BTreeSet<usize> = BTreeSet::new();
  while chosen.len() < wanted {
    chosen.insert(rng.below(earlier.len()));
  }
  chosen.into_iter().map(|index| earlier[index].clone()).collect()
}

fn plan_units(cfg: &GenConfig, prefix: &str) -> Vec<UnitPlan> {
  let name = |index: usize| format!("{prefix}repo-{index}");
  if cfg.lang == LangChoice::Mixed && cfg.repos == 1 {
    return vec![
      UnitPlan {
        repo: name(0),
        subdir: Some("server"),
        lang: Lang::Rust,
      },
      UnitPlan {
        repo: name(0),
        subdir: Some("web"),
        lang: Lang::Ts,
      },
    ];
  }
  (0..cfg.repos)
    .map(|index| UnitPlan {
      repo: name(index),
      subdir: None,
      lang: match cfg.lang {
        LangChoice::Rust => Lang::Rust,
        LangChoice::Ts => Lang::Ts,
        LangChoice::Mixed if index % 2 == 0 => Lang::Rust,
        LangChoice::Mixed => Lang::Ts,
      },
    })
    .collect()
}

fn unit_ctx(plans: &[UnitPlan], index: usize, slug_prefix: &str) -> UnitCtx {
  let plan = &plans[index];
  let package = plan.package();
  let ident = package.replace('-', "_");
  let dep = plans[..index].iter().rposition(|candidate| candidate.lang == plan.lang).map(|dep_index| {
    let dep_plan = &plans[dep_index];
    let up = if plan.subdir.is_some() { "../../" } else { "../" };
    let dep_package = dep_plan.package();
    DepCtx {
      ident: dep_package.replace('-', "_"),
      package: match plan.lang {
        Lang::Rust => dep_package,
        Lang::Ts => format!("@gen/{dep_package}"),
      },
      rel_path: format!("{up}{}", dep_plan.rel_dir()),
    }
  });
  UnitCtx {
    package: match plan.lang {
      Lang::Rust => package,
      Lang::Ts => format!("@gen/{package}"),
    },
    env_prefix: ident.to_uppercase(),
    slug: plan.subdir.map(str::to_owned).unwrap_or_else(|| format!("{slug_prefix}r{}", index)),
    ident,
    dep,
  }
}

fn generate_project(cfg: &GenConfig, project_index: usize, root: &Path) -> BenchResult<(ProjectOutput, u64, u64)> {
  let prefix = if cfg.projects > 1 { format!("p{project_index}-") } else { String::new() };
  let slug_prefix = if cfg.projects > 1 { format!("p{project_index}") } else { String::new() };
  let plans = plan_units(cfg, &prefix);
  let mut writer = Writer {
    root: root.to_path_buf(),
    files: 0,
    loc: 0,
  };
  let budget = cfg.loc / plans.len() as u64;
  let project_rng = Rng::new(cfg.seed).derive(project_index as u64);
  let mut outputs: Vec<UnitOutput> = Vec::new();
  let mut planted = Vec::new();
  for (index, plan) in plans.iter().enumerate() {
    let ctx = unit_ctx(&plans, index, &slug_prefix);
    let dep_fns = plans[..index]
      .iter()
      .rposition(|candidate| candidate.lang == plan.lang)
      .map(|dep_index| outputs[dep_index].api_fns.clone())
      .unwrap_or_default();
    let earlier: Vec<Endpoint> = outputs.iter().flat_map(|output| output.endpoints.iter().cloned()).collect();
    let mut state = UnitState {
      rng: project_rng.derive(index as u64 + 1),
      lang: plan.lang,
      repo: plan.repo.clone(),
      rel_dir: plan.rel_dir(),
      registry: Vec::new(),
      specs: Vec::new(),
      dep_fns,
      counter: 0,
      planted: Vec::new(),
    };
    outputs.push(generate_unit(plan, &ctx, budget, &mut state, &earlier, &mut writer)?);
    planted.append(&mut state.planted);
  }
  let repos: Vec<RepoOutput> = {
    let mut names: Vec<String> = plans.iter().map(|plan| plan.repo.clone()).collect();
    names.dedup();
    names.into_iter().map(|name| RepoOutput { path: root.join(&name), name }).collect()
  };
  let manifest = serde_json::json!({
    "seed": cfg.seed,
    "target_loc": cfg.loc,
    "repos": repos.iter().map(|repo| repo.name.clone()).collect::<Vec<_>>(),
    "files": writer.files,
    "loc": writer.loc,
    "planted_duplicates": planted,
  });
  let text = serde_json::to_string_pretty(&manifest).map_err(|source| BenchError::json("generator manifest", source))?;
  writer.write("gen-manifest.json", &text)?;
  let name = if cfg.projects > 1 { format!("synth-p{project_index}") } else { "synth".to_owned() };
  Ok((
    ProjectOutput {
      name,
      root: root.to_path_buf(),
      repos,
      planted,
    },
    writer.files,
    writer.loc,
  ))
}

pub fn generate(cfg: &GenConfig, out: &Path) -> BenchResult<GenSummary> {
  if cfg.repos == 0 || cfg.projects == 0 || cfg.loc == 0 {
    return Err(BenchError::Invalid("loc, repos and projects must be positive".to_owned()));
  }
  let mut projects = Vec::new();
  let (mut files, mut loc) = (0, 0);
  for index in 0..cfg.projects {
    let root = if cfg.projects > 1 { out.join(format!("p{index}")) } else { out.to_path_buf() };
    let (project, project_files, project_loc) = generate_project(cfg, index, &root)?;
    files += project_files;
    loc += project_loc;
    projects.push(project);
  }
  Ok(GenSummary {
    projects,
    files,
    loc,
    tree_hash: hash_tree(out)?,
  })
}

fn collect_files(dir: &Path, found: &mut Vec<PathBuf>) -> BenchResult<()> {
  let mut entries: Vec<PathBuf> = fs::read_dir(dir)
    .map_err(|source| BenchError::io("read", dir, source))?
    .map(|entry| entry.map(|entry| entry.path()).map_err(|source| BenchError::io("read", dir, source)))
    .collect::<BenchResult<_>>()?;
  entries.sort();
  for path in entries {
    if path.is_dir() {
      collect_files(&path, found)?;
    } else {
      found.push(path);
    }
  }
  Ok(())
}

pub fn hash_tree(root: &Path) -> BenchResult<String> {
  let mut files = Vec::new();
  collect_files(root, &mut files)?;
  let mut hasher = blake3::Hasher::new();
  for path in files {
    let content = fs::read(&path).map_err(|source| BenchError::io("read", &path, source))?;
    let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
    hasher.update(relative.as_bytes());
    hasher.update(&(content.len() as u64).to_le_bytes());
    hasher.update(&content);
  }
  Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
  use atlas_engine::IndexMode;

  use super::*;
  use crate::scenario::{open_engine, parse_counts};
  use crate::source::list_sources;
  use crate::suite::synthetic_projects;

  fn config(loc: u64, repos: usize, lang: LangChoice, seed: u64) -> GenConfig {
    GenConfig {
      loc,
      repos,
      lang,
      seed,
      projects: 1,
    }
  }

  fn generated(cfg: &GenConfig) -> (tempfile::TempDir, GenSummary) {
    let dir = tempfile::tempdir().unwrap();
    let summary = generate(cfg, dir.path()).unwrap();
    (dir, summary)
  }

  #[test]
  fn should_produce_the_same_tree_hash_for_the_same_seed() {
    let cfg = config(6_000, 2, LangChoice::Mixed, 11);
    let (first_dir, first) = generated(&cfg);
    let (second_dir, second) = generated(&cfg);
    assert_eq!(first.tree_hash, second.tree_hash);
    assert_eq!(hash_tree(first_dir.path()).unwrap(), hash_tree(second_dir.path()).unwrap());
    assert_eq!((first.files, first.loc), (second.files, second.loc));
  }

  #[test]
  fn should_produce_a_different_tree_for_a_different_seed() {
    let (_first_dir, first) = generated(&config(6_000, 2, LangChoice::Mixed, 11));
    let (_second_dir, second) = generated(&config(6_000, 2, LangChoice::Mixed, 12));
    assert_ne!(first.tree_hash, second.tree_hash);
  }

  #[test]
  fn should_land_within_a_tenth_of_the_requested_loc_per_unit_budget() {
    let (_dir, summary) = generated(&config(20_000, 2, LangChoice::Mixed, 3));
    assert!(summary.loc >= 20_000 && summary.loc < 24_000, "loc={}", summary.loc);
  }

  #[test]
  fn should_plant_near_duplicates_with_their_originals() {
    let (_dir, summary) = generated(&config(20_000, 1, LangChoice::Rust, 5));
    let planted = &summary.projects[0].planted;
    assert!(!planted.is_empty());
    assert!(planted.iter().all(|duplicate| duplicate.path.ends_with(".rs") && duplicate.name != duplicate.original));
  }

  #[test]
  fn should_generate_cross_repo_endpoints_and_clients() {
    let (dir, _) = generated(&config(8_000, 2, LangChoice::Rust, 5));
    let routes = fs::read_to_string(dir.path().join("repo-0/src/g0/routes.rs")).unwrap();
    assert!(routes.contains(".route(\"/api/v1/r0/"));
    let client = fs::read_to_string(dir.path().join("repo-1/src/client.rs")).unwrap();
    assert!(client.contains("format!(\"{}/api/v1/r0/"));
    let manifest = fs::read_to_string(dir.path().join("repo-1/Cargo.toml")).unwrap();
    assert!(manifest.contains("repo-0 = { path = \"../repo-0\" }"));
  }

  #[test]
  fn should_split_a_single_mixed_repo_into_a_server_and_a_web_package() {
    let (dir, summary) = generated(&config(8_000, 1, LangChoice::Mixed, 5));
    assert_eq!(summary.projects[0].repos.len(), 1);
    assert!(dir.path().join("repo-0/server/Cargo.toml").exists());
    assert!(dir.path().join("repo-0/web/package.json").exists());
    let client = fs::read_to_string(dir.path().join("repo-0/web/src/client.ts")).unwrap();
    assert!(client.contains("fetch(`${this.baseUrl}/api/v1/server/"));
  }

  #[test]
  fn should_keep_projects_in_separate_directories_with_distinct_repo_names() {
    let dir = tempfile::tempdir().unwrap();
    let summary = generate(
      &GenConfig {
        projects: 3,
        ..config(6_000, 2, LangChoice::Mixed, 9)
      },
      dir.path(),
    )
    .unwrap();
    let names: Vec<&str> = summary.projects.iter().flat_map(|project| project.repos.iter().map(|repo| repo.name.as_str())).collect();
    assert_eq!(names, vec!["p0-repo-0", "p0-repo-1", "p1-repo-0", "p1-repo-1", "p2-repo-0", "p2-repo-1"]);
    assert!(dir.path().join("p2/p2-repo-1").is_dir());
  }

  #[test]
  fn should_reject_empty_configurations() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(generate(&config(0, 1, LangChoice::Rust, 1), dir.path()), Err(BenchError::Invalid(_))));
    assert!(matches!(generate(&config(100, 0, LangChoice::Rust, 1), dir.path()), Err(BenchError::Invalid(_))));
  }

  fn assert_parses_cleanly(cfg: &GenConfig) {
    let (dir, summary) = generated(cfg);
    let projects = synthetic_projects(&summary.projects).unwrap();
    let home = tempfile::tempdir().unwrap();
    let (engine, _) = open_engine(&projects[0], home.path()).unwrap();
    let run = engine.index(IndexMode::Full).unwrap();
    assert_eq!(run.outcome, atlas_engine::RunOutcomeKind::Ok);
    let parsed = parse_counts(&engine, &projects[0]).unwrap();
    let sources = list_sources(&projects[0]).unwrap();
    assert!(sources.len() > 20);
    assert_eq!(parsed.unclean(), 0, "{:?}", parsed.samples);
    assert_eq!(parsed.ok, sources.len() as u64);
    drop(dir);
  }

  #[test]
  fn should_generate_rust_that_parses_with_zero_failures() {
    assert_parses_cleanly(&config(12_000, 2, LangChoice::Rust, 21));
  }

  #[test]
  fn should_generate_typescript_that_parses_with_zero_failures() {
    assert_parses_cleanly(&config(12_000, 2, LangChoice::Ts, 22));
  }

  #[test]
  fn should_generate_mixed_repos_that_parse_with_zero_failures() {
    assert_parses_cleanly(&config(12_000, 3, LangChoice::Mixed, 23));
    assert_parses_cleanly(&config(12_000, 1, LangChoice::Mixed, 24));
  }
}
