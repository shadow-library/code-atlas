#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atlas_core::{
  AnalysisInput, CertaintyClass, ContentHash, Edge, EdgeKind, EntityId, EntityKey, EntityKind, FileFacts, FileRef, KeyContext, Language, LanguageAnalyzer, LanguageFamily, RelPath,
  RepoName, SymbolKey, UNPACKAGED, UnresolvedReason, UnresolvedRef, ts_module_path,
};
use atlas_lang_rust::RustAnalyzer;
use atlas_lang_ts::TsAnalyzer;
use atlas_project::{PackageGraph, RepoConfig, discover};
use atlas_resolve::{ProjectSymbolTable, ResolvedFile, resolve_file};

pub fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn repo(name: &str) -> RepoName {
  RepoName::new(name).unwrap()
}

pub fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_owned()).collect()
}

pub struct RepoSource {
  pub name: RepoName,
  pub root: PathBuf,
  pub graph: PackageGraph,
  pub files: Vec<(FileFacts, KeyContext)>,
}

pub fn analyze(repo: &RepoName, path: &str, source: &str) -> FileFacts {
  let file = FileRef::detect(repo.clone(), RelPath::new(path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
  let input = AnalysisInput { file: &file, source };
  let facts = match file.language.family() {
    LanguageFamily::Rust => RustAnalyzer::new().analyze(&input).unwrap(),
    _ => TsAnalyzer::new().analyze(&input).unwrap(),
  };
  facts.check_invariants(source.len()).unwrap();
  facts
}

pub fn key_context(graph: &PackageGraph, path: &RelPath) -> KeyContext {
  let family = Language::from_path(path.as_str()).map(Language::family);
  if family == Some(LanguageFamily::Rust) {
    let targets = graph.rust_targets_for_file(path);
    if let Some(target) = targets.first() {
      let cargo = graph.package_for_file(path).and_then(|package| package.as_cargo()).unwrap();
      return KeyContext {
        package: target.target.package_field(&cargo.name),
        module_path: target.module_path.clone(),
      };
    }
    let stem = path.as_str().strip_suffix(".rs").unwrap_or(path.as_str());
    return KeyContext {
      package: UNPACKAGED.to_owned(),
      module_path: stem.split('/').map(str::to_owned).collect(),
    };
  }
  match graph.package_for_file(path).and_then(|package| Some((package.name()?.to_owned(), package.root.clone()))) {
    Some((name, root)) => KeyContext {
      package: name,
      module_path: ts_module_path(path, root.as_rel_path()).unwrap(),
    },
    None => KeyContext {
      package: UNPACKAGED.to_owned(),
      module_path: ts_module_path(path, None).unwrap(),
    },
  }
}

pub fn load_repo(name: &str, root: &Path) -> RepoSource {
  let name = repo(name);
  let config = RepoConfig {
    name: name.clone(),
    path: root.to_path_buf(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  };
  let discovered = discover(&config, 2 * 1024 * 1024).unwrap();
  let graph = PackageGraph::load(root, &discovered.files);
  let files = discovered
    .files
    .iter()
    .filter(|file| file.language.is_some())
    .map(|file| {
      let source = std::fs::read_to_string(root.join(file.path.as_str())).unwrap();
      let facts = analyze(&name, file.path.as_str(), &source);
      let context = key_context(&graph, &file.path);
      (facts, context)
    })
    .collect();
  RepoSource {
    name,
    root: root.to_path_buf(),
    graph,
    files,
  }
}

pub struct Project {
  pub table: ProjectSymbolTable,
  pub repos: Vec<RepoSource>,
  pub results: BTreeMap<(String, String), ResolvedFile>,
}

impl Project {
  pub fn load(repos: &[(&str, PathBuf)]) -> Self {
    let repos: Vec<RepoSource> = repos.iter().map(|(name, root)| load_repo(name, root)).collect();
    let mut table = ProjectSymbolTable::new();
    for source in &repos {
      table.set_repo_packages(source.name.clone(), PackageGraph::load(&source.root, &discover_files(source)));
      for (facts, context) in &source.files {
        table.upsert_file(facts, context).unwrap();
      }
    }
    table.refresh();
    let mut project = Self {
      table,
      repos,
      results: BTreeMap::new(),
    };
    project.resolve_all();
    project
  }

  pub fn resolve_all(&mut self) {
    self.results.clear();
    for source in &self.repos {
      for (facts, _) in &source.files {
        let resolved = resolve_file(facts, &self.table).unwrap();
        self.results.insert((source.name.to_string(), facts.file.path.to_string()), resolved);
      }
    }
  }

  pub fn adversarial(case: &str) -> Self {
    Self::load(&[(case, fixtures_root().join("adversarial").join(case))])
  }

  pub fn facts(&self, repo: &str, path: &str) -> &FileFacts {
    self
      .repos
      .iter()
      .find(|source| source.name.as_str() == repo)
      .and_then(|source| source.files.iter().find(|(facts, _)| facts.file.path.as_str() == path))
      .map(|(facts, _)| facts)
      .unwrap_or_else(|| panic!("no file {repo}:{path}"))
  }

  pub fn result(&self, repo: &str, path: &str) -> &ResolvedFile {
    self
      .results
      .get(&(repo.to_owned(), path.to_owned()))
      .unwrap_or_else(|| panic!("no result for {repo}:{path}"))
  }

  pub fn all_edges(&self) -> impl Iterator<Item = &Edge> {
    self.results.values().flat_map(|result| result.edges.iter().chain(&result.structural))
  }

  pub fn all_unresolved(&self) -> impl Iterator<Item = &UnresolvedRef> {
    self.results.values().flat_map(|result| result.unresolved.iter())
  }

  pub fn edges_between(&self, kind: EdgeKind, from: EntityId, to: EntityId) -> Vec<&Edge> {
    self.all_edges().filter(|edge| edge.kind == kind && edge.from == from && edge.to == to).collect()
  }

  pub fn assert_edge(&self, kind: EdgeKind, from: EntityId, to: EntityId, classes: &[CertaintyClass]) -> &Edge {
    let edges = self.edges_between(kind.clone(), from, to);
    let [edge] = edges.as_slice() else {
      panic!("expected exactly one {kind} edge {from} -> {to}, found {}: {edges:#?}", edges.len());
    };
    assert!(
      classes.contains(&edge.certainty.class()),
      "{kind} edge {from} -> {to} has certainty {:?}, expected one of {classes:?}",
      edge.certainty
    );
    assert!(!edge.evidence.is_empty(), "edge without evidence: {edge:#?}");
    edge
  }

  pub fn assert_no_edge(&self, kind: EdgeKind, from: EntityId, to: EntityId) {
    let edges = self.edges_between(kind.clone(), from, to);
    assert!(edges.is_empty(), "forbidden {kind} edge {from} -> {to}: {edges:#?}");
  }

  pub fn assert_no_edge_of_class(&self, kind: EdgeKind, from: EntityId, to: EntityId, classes: &[CertaintyClass]) {
    let edges: Vec<&Edge> = self
      .edges_between(kind.clone(), from, to)
      .into_iter()
      .filter(|edge| classes.contains(&edge.certainty.class()))
      .collect();
    assert!(edges.is_empty(), "forbidden {kind} edge {from} -> {to} with class in {classes:?}: {edges:#?}");
  }

  pub fn edges_from(&self, kind: EdgeKind, from: EntityId) -> Vec<&Edge> {
    self.all_edges().filter(|edge| edge.kind == kind && edge.from == from).collect()
  }

  pub fn unresolved_named(&self, from: EntityId, name: &str) -> Vec<&UnresolvedRef> {
    self
      .all_unresolved()
      .filter(|unresolved| unresolved.from == from && unresolved.path.last().is_some_and(|last| last == name))
      .collect()
  }

  pub fn assert_unresolved(&self, from: EntityId, name: &str, expected: &UnresolvedReason) {
    let found = self.unresolved_named(from, name);
    assert!(!found.is_empty(), "no unresolved reference named {name} from {from}");
    for unresolved in found {
      assert_eq!(&unresolved.reason, expected, "{unresolved:#?}");
    }
  }
}

fn discover_files(source: &RepoSource) -> Vec<atlas_project::DiscoveredFile> {
  let config = RepoConfig {
    name: source.name.clone(),
    path: source.root.clone(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  };
  discover(&config, 2 * 1024 * 1024).unwrap().files
}

pub fn symbol(language: LanguageFamily, repo_name: &str, package: &str, module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  let key = SymbolKey::new(language, repo(repo_name), package, strings(module), strings(items), kind, 0).unwrap();
  EntityId::of(&EntityKey::Symbol(key))
}

pub fn rust(repo_name: &str, package: &str, module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  symbol(LanguageFamily::Rust, repo_name, package, module, items, kind)
}

pub fn ts(repo_name: &str, package: &str, module: &[&str], items: &[&str], kind: EntityKind) -> EntityId {
  symbol(LanguageFamily::Ts, repo_name, package, module, items, kind)
}

pub fn file(repo_name: &str, path: &str) -> EntityId {
  EntityId::of(&EntityKey::File {
    repo: repo(repo_name),
    path: RelPath::new(path).unwrap(),
  })
}

impl Project {
  pub fn names(&self) -> std::collections::HashMap<EntityId, String> {
    let mut names = std::collections::HashMap::new();
    for source in &self.repos {
      for (facts, _) in &source.files {
        let placement = self.table.placement(&facts.file.repo, &facts.file.path).unwrap();
        names.insert(file(source.name.as_str(), facts.file.path.as_str()), format!("file:{}", facts.file.path));
        for key in facts.symbol_keys(&placement.context).unwrap() {
          names.insert(EntityId::of(&EntityKey::Symbol(key.clone())), key.to_string());
        }
      }
    }
    names
  }

  pub fn dump(&self) -> String {
    let names = self.names();
    let name = |id: &EntityId| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let mut out = String::new();
    for ((repo, path), result) in &self.results {
      out.push_str(&format!("== {repo}:{path} override={:?} {:?}\n", result.module_path_override, result.reachability));
      for edge in result.edges.iter().chain(&result.structural) {
        out.push_str(&format!("  {} {} -> {} {}\n", edge.kind, name(&edge.from), name(&edge.to), edge.certainty.tag()));
      }
      for unresolved in &result.unresolved {
        out.push_str(&format!(
          "  ?? {} {:?} {:?} {:?}\n",
          name(&unresolved.from),
          unresolved.kind,
          unresolved.path,
          unresolved.reason
        ));
      }
    }
    out
  }
}

pub struct Synthetic {
  pub dirs: Vec<tempfile::TempDir>,
  pub project: Project,
}

pub fn write_repo(files: &[(&str, &str)]) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  for (path, content) in files {
    let target = dir.path().join(path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, content).unwrap();
  }
  dir
}

pub fn synthetic(repos: &[(&str, &[(&str, &str)])]) -> Synthetic {
  let dirs: Vec<tempfile::TempDir> = repos.iter().map(|(_, files)| write_repo(files)).collect();
  let roots: Vec<(&str, PathBuf)> = repos.iter().zip(&dirs).map(|((name, _), dir)| (*name, dir.path().to_path_buf())).collect();
  let project = Project::load(&roots);
  Synthetic { dirs, project }
}

pub fn single(name: &str, files: &[(&str, &str)]) -> Synthetic {
  synthetic(&[(name, files)])
}

pub fn symbol_n(language: LanguageFamily, repo_name: &str, package: &str, module: &[&str], items: &[&str], kind: EntityKind, n: u32) -> EntityId {
  let key = SymbolKey::new(language, repo(repo_name), package, strings(module), strings(items), kind, n).unwrap();
  EntityId::of(&EntityKey::Symbol(key))
}

impl Project {
  pub fn edges_to(&self, kind: EdgeKind, to: EntityId) -> Vec<&Edge> {
    self.all_edges().filter(|edge| edge.kind == kind && edge.to == to).collect()
  }

  pub fn assert_certainty(&self, kind: EdgeKind, from: EntityId, to: EntityId, certainty: &atlas_core::Certainty) {
    let edges = self.edges_between(kind.clone(), from, to);
    let [edge] = edges.as_slice() else {
      panic!("expected exactly one {kind} edge {from} -> {to}, found {edges:#?}");
    };
    assert_eq!(&edge.certainty, certainty, "{kind} edge {from} -> {to}");
  }
}
