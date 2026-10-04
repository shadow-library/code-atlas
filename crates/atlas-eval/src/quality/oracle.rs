use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atlas_core::{Availability, CertaintyClass, Edge, EdgeKind, Entity, EntityId, EntityKey, Owner, ProjectName, RelPath, RepoName};
use atlas_engine::{Engine, EngineOptions};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_scip::{Agreement, Disagreement, Reconciliation, RustAnalyzerScip, ScipIndexer, ScipTypescript, reconcile};
use serde::{Deserialize, Serialize};

use super::corpus::{FixtureGroup, copy_tree};
use super::error::{QualityError, QualityResult};
use super::metrics::{CONFIDENCE_BUCKETS, bucket_index, bucket_label, ratio};
use super::oracle_index::{IndexDigest, PackageLine, RunContext, execute_run, plan};
use super::snapshot::Snapshot;

const COMPARABLE: [EdgeKind; 5] = [EdgeKind::Calls, EdgeKind::References, EdgeKind::Instantiates, EdgeKind::Implements, EdgeKind::Extends];
const SITE_KINDS: [&str; 3] = ["calls", "references", "instantiates"];
const CONFLICT_SAMPLE: usize = 20;
const WORKSPACE_ITEMS: [&str; 5] = ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml"];

#[derive(Debug, Clone)]
pub struct OracleOptions {
  pub work_dir: PathBuf,
  pub workspace_root: Option<PathBuf>,
  pub typescript: bool,
  pub timeout: Duration,
  pub only: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum IndexerState {
  Available { detail: String },
  Unavailable { reason: String },
  Disabled { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
  pub agreed: u64,
  pub agreed_respanned: u64,
  pub ours_only: u64,
  pub ours_only_cross_package: u64,
  pub ours_only_file_level: u64,
  pub scip_only: u64,
  pub conflicting: u64,
  pub scip_conflicting: u64,
}

impl Tally {
  pub fn confirmable(&self) -> u64 {
    self.agreed + self.conflicting + self.ours_only
  }

  pub fn unconfirmable(&self) -> u64 {
    self.ours_only_cross_package + self.ours_only_file_level
  }

  pub fn total_edges(&self) -> u64 {
    self.confirmable() + self.unconfirmable()
  }

  pub fn precision_confirmable(&self) -> Option<f64> {
    ratio(self.agreed, self.confirmable())
  }

  pub fn precision_all_in(&self) -> Option<f64> {
    ratio(self.agreed, self.total_edges())
  }

  pub fn recall(&self) -> Option<f64> {
    ratio(self.agreed, self.agreed + self.scip_only + self.scip_conflicting)
  }

  pub fn recall_within(&self, all: &Tally) -> Option<f64> {
    ratio(self.agreed, all.agreed + all.scip_only + all.scip_conflicting)
  }

  pub fn absorb(&mut self, other: &Tally) {
    self.agreed += other.agreed;
    self.agreed_respanned += other.agreed_respanned;
    self.ours_only += other.ours_only;
    self.ours_only_cross_package += other.ours_only_cross_package;
    self.ours_only_file_level += other.ours_only_file_level;
    self.scip_only += other.scip_only;
    self.conflicting += other.conflicting;
    self.scip_conflicting += other.scip_conflicting;
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassSplit {
  pub all: Tally,
  pub resolved: Tally,
  pub heuristic: Tally,
  pub syntactic: Tally,
}

impl ClassSplit {
  pub fn absorb(&mut self, other: &ClassSplit) {
    self.all.absorb(&other.all);
    self.resolved.absorb(&other.resolved);
    self.heuristic.absorb(&other.heuristic);
    self.syntactic.absorb(&other.syntactic);
  }

  fn bucket(&mut self, class: CertaintyClass) -> Option<&mut Tally> {
    match class {
      CertaintyClass::Resolved => Some(&mut self.resolved),
      CertaintyClass::Heuristic => Some(&mut self.heuristic),
      CertaintyClass::Syntactic => Some(&mut self.syntactic),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidenceBucket {
  pub label: String,
  pub agreed: u64,
  pub conflicting: u64,
  pub unconfirmed: u64,
}

impl ConfidenceBucket {
  pub fn agreement(&self) -> Option<f64> {
    ratio(self.agreed, self.agreed + self.conflicting)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictSample {
  pub target: String,
  pub location: String,
  pub enclosing: String,
  pub edge_kind: String,
  pub ours: Vec<String>,
  pub scip: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetOracle {
  pub name: String,
  pub packages: Vec<PackageLine>,
  pub files_covered: usize,
  pub per_kind: BTreeMap<String, ClassSplit>,
  pub skipped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OracleReport {
  pub rust_analyzer: IndexerState,
  pub scip_typescript: IndexerState,
  pub targets: Vec<TargetOracle>,
  pub per_kind: BTreeMap<String, ClassSplit>,
  pub site_kinds: ClassSplit,
  pub heuristic_buckets: Vec<ConfidenceBucket>,
  pub conflict_sample: Vec<ConflictSample>,
  pub conflicts_total: usize,
  pub unconfirmed_sample: Vec<ConflictSample>,
  pub unconfirmed_total: usize,
  pub scip_only_sample: Vec<ConflictSample>,
  pub scip_only_total: usize,
  pub indexes: Vec<IndexDigest>,
}

impl OracleReport {
  pub fn unavailable(rust_analyzer: IndexerState, scip_typescript: IndexerState) -> Self {
    Self {
      rust_analyzer,
      scip_typescript,
      targets: Vec::new(),
      per_kind: BTreeMap::new(),
      site_kinds: ClassSplit::default(),
      heuristic_buckets: empty_buckets(),
      conflict_sample: Vec::new(),
      conflicts_total: 0,
      unconfirmed_sample: Vec::new(),
      unconfirmed_total: 0,
      scip_only_sample: Vec::new(),
      scip_only_total: 0,
      indexes: Vec::new(),
    }
  }
}

fn empty_buckets() -> Vec<ConfidenceBucket> {
  (0..CONFIDENCE_BUCKETS.len())
    .map(|index| ConfidenceBucket {
      label: bucket_label(index),
      ..ConfidenceBucket::default()
    })
    .collect()
}

fn state_of(availability: Availability, detail: &str) -> IndexerState {
  match availability {
    Availability::Available => IndexerState::Available { detail: detail.to_owned() },
    Availability::Unavailable { reason } => IndexerState::Unavailable { reason },
  }
}

struct Target {
  name: String,
  project: String,
  repos: Vec<(String, PathBuf)>,
  workspace: bool,
}

fn targets(groups: &[FixtureGroup], options: &OracleOptions) -> Vec<Target> {
  let mut targets: Vec<Target> = groups
    .iter()
    .filter(|group| group.name != "adversarial/broken-syntax")
    .map(|group| Target {
      name: group.name.clone(),
      project: group.truth.project.name.clone(),
      repos: group.repos.iter().map(|repo| (repo.name.clone(), repo.dir.clone())).collect(),
      workspace: false,
    })
    .collect();
  if let Some(root) = &options.workspace_root {
    targets.push(Target {
      name: "workspace".to_owned(),
      project: "code-atlas-workspace".to_owned(),
      repos: vec![("code-atlas".to_owned(), root.clone())],
      workspace: true,
    });
  }
  targets.retain(|target| options.only.is_empty() || options.only.iter().any(|filter| target.name.contains(filter.as_str())));
  targets
}

pub fn run_oracle(groups: &[FixtureGroup], options: &OracleOptions) -> QualityResult<OracleReport> {
  let rust = RustAnalyzerScip::new().with_timeout(options.timeout);
  let typescript = ScipTypescript::new().with_timeout(options.timeout);
  let rust_state = state_of(rust.availability(), "rust-analyzer scip");
  let ts_state = match options.typescript {
    false => IndexerState::Disabled {
      reason: "scip-typescript was not requested (--scip-typescript)".to_owned(),
    },
    true => state_of(typescript.availability(), "npx @sourcegraph/scip-typescript"),
  };
  let mut report = OracleReport::unavailable(rust_state.clone(), ts_state.clone());
  let rust_ready = matches!(rust_state, IndexerState::Available { .. });
  let ts_ready = matches!(ts_state, IndexerState::Available { .. });
  if !rust_ready && !ts_ready {
    return Ok(report);
  }
  fs::create_dir_all(&options.work_dir).map_err(|error| QualityError::io(&options.work_dir, error))?;
  let mut conflicts: Vec<ConflictSample> = Vec::new();
  let mut unconfirmed: Vec<ConflictSample> = Vec::new();
  let mut missed: Vec<ConflictSample> = Vec::new();
  for target in targets(groups, options) {
    tracing::info!(target = %target.name, "running the scip oracle");
    let outcome = run_target(
      &target,
      options,
      rust_ready.then_some(&rust as &dyn ScipIndexer),
      ts_ready.then_some(&typescript as &dyn ScipIndexer),
    )?;
    report.indexes.extend(outcome.digests);
    report.targets.push(outcome.summary);
    conflicts.extend(outcome.compared.conflicts);
    unconfirmed.extend(outcome.compared.unconfirmed);
    missed.extend(outcome.compared.missed);
    for (kind, split) in outcome.compared.per_kind {
      report.per_kind.entry(kind).or_default().absorb(&split);
    }
    for (index, bucket) in outcome.compared.buckets.iter().enumerate() {
      report.heuristic_buckets[index].agreed += bucket.agreed;
      report.heuristic_buckets[index].conflicting += bucket.conflicting;
      report.heuristic_buckets[index].unconfirmed += bucket.unconfirmed;
    }
  }
  for (kind, split) in &report.per_kind {
    if SITE_KINDS.contains(&kind.as_str()) {
      report.site_kinds.absorb(split);
    }
  }
  report.conflicts_total = conflicts.len();
  report.conflict_sample = sample(conflicts);
  report.unconfirmed_total = unconfirmed.len();
  report.unconfirmed_sample = sample(unconfirmed);
  report.scip_only_total = missed.len();
  report.scip_only_sample = sample(missed);
  Ok(report)
}

fn sample(mut conflicts: Vec<ConflictSample>) -> Vec<ConflictSample> {
  conflicts.sort_by(|left, right| (&left.target, &left.location).cmp(&(&right.target, &right.location)));
  if conflicts.len() <= CONFLICT_SAMPLE {
    return conflicts;
  }
  let step = conflicts.len() as f64 / CONFLICT_SAMPLE as f64;
  (0..CONFLICT_SAMPLE).map(|index| conflicts[(index as f64 * step) as usize].clone()).collect()
}

struct TargetOutcome {
  summary: TargetOracle,
  digests: Vec<IndexDigest>,
  compared: Compared,
}

fn safe_name(name: &str) -> String {
  name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

fn copy_workspace(root: &Path, to: &Path) -> QualityResult<()> {
  fs::create_dir_all(to).map_err(|error| QualityError::io(to, error))?;
  for item in WORKSPACE_ITEMS {
    let source = root.join(item);
    let target = to.join(item);
    match source.is_dir() {
      true => copy_tree(&source, &target)?,
      false if source.is_file() => {
        fs::copy(&source, &target).map_err(|error| QualityError::io(&source, error))?;
      }
      false => {}
    }
  }
  Ok(())
}

fn run_target(target: &Target, options: &OracleOptions, cargo: Option<&dyn ScipIndexer>, npm: Option<&dyn ScipIndexer>) -> QualityResult<TargetOutcome> {
  let work = options.work_dir.join(safe_name(&target.name));
  let tree = work.join("tree");
  if tree.exists() {
    fs::remove_dir_all(&tree).map_err(|error| QualityError::io(&tree, error))?;
  }
  let mut configs = Vec::new();
  for (name, source) in &target.repos {
    let destination = tree.join(name);
    match target.workspace {
      true => copy_workspace(source, &destination)?,
      false => copy_tree(source, &destination)?,
    }
    configs.push(RepoConfig {
      name: RepoName::new(name).map_err(|error| QualityError::scenario(&target.name, error.to_string()))?,
      path: destination,
      include: Vec::new(),
      exclude: Vec::new(),
      languages: None,
    });
  }
  let project = Project {
    name: ProjectName::new(&target.project).map_err(|error| QualityError::scenario(&target.name, error.to_string()))?,
    settings: ProjectSettings::default(),
    repos: configs,
  };
  let home = work.join("home");
  if home.exists() {
    fs::remove_dir_all(&home).map_err(|error| QualityError::io(&home, error))?;
  }
  let engine = Arc::new(Engine::open(&project, &AtlasHome::rooted_at(&home), EngineOptions::default())?);
  engine.ensure_fresh()?;
  let snapshot = Snapshot::load(&engine, &project)?;
  let cache_dir = work.join("scip-cache");
  let context = RunContext {
    target: &target.name,
    project: &project,
    engine: &engine,
    snapshot: &snapshot,
    cache_dir: &cache_dir,
    cargo,
    npm,
  };
  let mut packages: Vec<PackageLine> = Vec::new();
  let mut digests: Vec<IndexDigest> = Vec::new();
  let mut per_file: BTreeMap<(RepoName, RelPath), Vec<Edge>> = BTreeMap::new();
  for run in plan(&snapshot, &project) {
    let output = execute_run(&run, &context)?;
    packages.push(output.line);
    digests.extend(output.digest);
    per_file.extend(output.per_file);
  }
  let compared = compare(&engine, &snapshot, &project, &per_file, &target.name)?;
  let skipped = per_file
    .is_empty()
    .then(|| "no SCIP index covered an indexed file (no indexer ran for this target)".to_owned());
  Ok(TargetOutcome {
    summary: TargetOracle {
      name: target.name.clone(),
      packages,
      files_covered: per_file.len(),
      per_kind: compared.per_kind.clone(),
      skipped,
    },
    digests,
    compared,
  })
}

type Lookup<'e> = HashMap<(ReconcileKeyHash, EntityId), &'e Edge>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ReconcileKeyHash {
  Site(EntityId, String, u32, u32),
  Relation(String, EntityId),
}

fn keys_of(edge: &Edge) -> Vec<ReconcileKeyHash> {
  let relational = matches!(edge.kind, EdgeKind::Implements | EdgeKind::Extends) || edge.evidence.is_empty();
  if relational {
    return vec![ReconcileKeyHash::Relation(edge.kind.to_string(), edge.from)];
  }
  edge
    .evidence
    .iter()
    .map(|evidence| ReconcileKeyHash::Site(edge.from, evidence.path.to_string(), evidence.span.start_byte, evidence.span.end_byte))
    .collect()
}

fn lookup(edges: &[Edge]) -> Lookup<'_> {
  let mut index = HashMap::new();
  for edge in edges {
    for key in keys_of(edge) {
      index.insert((key, edge.to), edge);
    }
  }
  index
}

fn key_hash(key: &atlas_scip::ReconcileKey) -> ReconcileKeyHash {
  match key {
    atlas_scip::ReconcileKey::Site { from, path, span, .. } => ReconcileKeyHash::Site(*from, path.to_string(), span.start_byte, span.end_byte),
    atlas_scip::ReconcileKey::Relation { kind, from } => ReconcileKeyHash::Relation(kind.to_string(), *from),
  }
}

fn package_of(entity: Option<&Entity>) -> Option<String> {
  let EntityKey::Symbol(key) = &entity?.key else { return None };
  Some(key.package().split('/').next().unwrap_or_default().to_owned())
}

struct Compared {
  per_kind: BTreeMap<String, ClassSplit>,
  buckets: Vec<ConfidenceBucket>,
  conflicts: Vec<ConflictSample>,
  unconfirmed: Vec<ConflictSample>,
  missed: Vec<ConflictSample>,
}

fn compare(engine: &Engine, snapshot: &Snapshot, project: &Project, per_file: &BTreeMap<(RepoName, RelPath), Vec<Edge>>, target_name: &str) -> QualityResult<Compared> {
  let mut ours: Vec<Edge> = Vec::new();
  let mut scip: Vec<Edge> = Vec::new();
  let mut sources = Sources::new(project);
  for ((repo, path), edges) in per_file {
    if !snapshot.files.contains_key(&(repo.clone(), path.clone())) {
      continue;
    }
    let owner = Owner::File {
      repo: repo.clone(),
      path: path.clone(),
    };
    let own = engine.store().read(|tx| tx.owned_facts(&owner))?;
    ours.extend(own.edges.into_iter().filter(|edge| COMPARABLE.contains(&edge.kind) && !edge.certainty.is_llm_derived()));
    scip.extend(edges.iter().cloned());
  }
  let result = reconcile(&scip, &ours);
  let ours_lookup = lookup(&ours);
  let scip_lookup = lookup(&scip);
  let mut per_kind: BTreeMap<String, ClassSplit> = BTreeMap::new();
  let mut buckets = empty_buckets();
  let mut conflicts = Vec::new();
  let mut unconfirmed = Vec::new();
  let mut missed = Vec::new();
  let mut record = |edge: &Edge, apply: &dyn Fn(&mut Tally)| {
    let split = per_kind.entry(edge.kind.to_string()).or_default();
    apply(&mut split.all);
    if let Some(bucket) = split.bucket(edge.certainty.class()) {
      apply(bucket);
    }
  };
  let heuristic_bucket = |edge: &Edge| {
    edge
      .certainty
      .confidence()
      .filter(|_| edge.certainty.class() == CertaintyClass::Heuristic)
      .map(|confidence| bucket_index(f32::from(confidence)))
  };
  let Reconciliation {
    agreed,
    ours_only,
    scip_only,
    conflicting,
  } = result;
  for Agreement { key, target } in &agreed {
    let Some(edge) = ours_lookup.get(&(key_hash(key), *target)) else { continue };
    record(edge, &|tally| tally.agreed += 1);
    if let Some(index) = heuristic_bucket(edge) {
      buckets[index].agreed += 1;
    }
  }
  let (respanned, drifted_conflicts) = pair_sites(&ours_only, &scip_only, snapshot, &mut sources);
  let consumed_ours: HashSet<usize> = respanned.iter().chain(&drifted_conflicts).map(|pair| pair.ours).collect();
  let consumed_scip: HashSet<usize> = respanned.iter().chain(&drifted_conflicts).map(|pair| pair.scip).collect();
  for pair in &respanned {
    let Agreement { key, target } = &ours_only[pair.ours];
    let Some(edge) = ours_lookup.get(&(key_hash(key), *target)) else { continue };
    record(edge, &|tally| {
      tally.agreed += 1;
      tally.agreed_respanned += 1;
    });
    if let Some(index) = heuristic_bucket(edge) {
      buckets[index].agreed += 1;
    }
  }
  for pair in &drifted_conflicts {
    let (
      Agreement {
        key: ours_key,
        target: ours_target,
      },
      Agreement {
        key: scip_key,
        target: scip_target,
      },
    ) = (&ours_only[pair.ours], &scip_only[pair.scip]);
    let (Some(ours_edge), Some(scip_edge)) = (ours_lookup.get(&(key_hash(ours_key), *ours_target)), scip_lookup.get(&(key_hash(scip_key), *scip_target))) else {
      continue;
    };
    record(ours_edge, &|tally| tally.conflicting += 1);
    record(scip_edge, &|tally| tally.scip_conflicting += 1);
    if let Some(index) = heuristic_bucket(ours_edge) {
      buckets[index].conflicting += 1;
    }
    if let atlas_scip::ReconcileKey::Site { from, repo, path, span } = ours_key {
      conflicts.push(ConflictSample {
        target: target_name.to_owned(),
        location: format!("{repo}:{path}:{}", span.start_line),
        enclosing: snapshot.describe(*from),
        edge_kind: ours_edge.kind.to_string(),
        ours: vec![describe_target(snapshot, &ours_lookup, ours_key, *ours_target)],
        scip: vec![snapshot.describe(*scip_target)],
      });
    }
  }
  for (index, Agreement { key, target }) in ours_only.iter().enumerate() {
    if consumed_ours.contains(&index) {
      continue;
    }
    let Some(edge) = ours_lookup.get(&(key_hash(key), *target)) else { continue };
    let unconfirmable = classify_unconfirmable(snapshot, edge);
    record(edge, &|tally| match unconfirmable {
      Some(Unconfirmable::CrossPackage) => tally.ours_only_cross_package += 1,
      Some(Unconfirmable::FileLevel) => tally.ours_only_file_level += 1,
      None => tally.ours_only += 1,
    });
    if let (None, atlas_scip::ReconcileKey::Site { from, repo, path, span }, CertaintyClass::Resolved) = (unconfirmable, key, edge.certainty.class()) {
      unconfirmed.push(ConflictSample {
        target: target_name.to_owned(),
        location: format!("{repo}:{path}:{}", span.start_line),
        enclosing: snapshot.describe(*from),
        edge_kind: edge.kind.to_string(),
        ours: vec![describe_target(snapshot, &ours_lookup, key, edge.to)],
        scip: Vec::new(),
      });
    }
    if let (Some(index), None) = (heuristic_bucket(edge), unconfirmable) {
      buckets[index].unconfirmed += 1;
    }
  }
  for (index, Agreement { key, target }) in scip_only.iter().enumerate() {
    if consumed_scip.contains(&index) {
      continue;
    }
    let Some(edge) = scip_lookup.get(&(key_hash(key), *target)) else { continue };
    record(edge, &|tally| tally.scip_only += 1);
    if let atlas_scip::ReconcileKey::Site { from, repo, path, span } = key {
      missed.push(ConflictSample {
        target: target_name.to_owned(),
        location: format!("{repo}:{path}:{}", span.start_line),
        enclosing: snapshot.describe(*from),
        edge_kind: edge.kind.to_string(),
        ours: Vec::new(),
        scip: vec![snapshot.describe(edge.to)],
      });
    }
  }
  for Disagreement { key, scip_targets, our_targets } in &conflicting {
    let mut kind = String::new();
    for target in our_targets {
      let Some(edge) = ours_lookup.get(&(key_hash(key), *target)) else { continue };
      kind = edge.kind.to_string();
      record(edge, &|tally| tally.conflicting += 1);
      if let Some(index) = heuristic_bucket(edge) {
        buckets[index].conflicting += 1;
      }
    }
    for target in scip_targets {
      let Some(edge) = scip_lookup.get(&(key_hash(key), *target)) else { continue };
      record(edge, &|tally| tally.scip_conflicting += 1);
    }
    if let atlas_scip::ReconcileKey::Site { from, repo, path, span } = key {
      conflicts.push(ConflictSample {
        target: target_name.to_owned(),
        location: format!("{repo}:{path}:{}", span.start_line),
        enclosing: snapshot.describe(*from),
        edge_kind: kind,
        ours: our_targets.iter().map(|id| describe_target(snapshot, &ours_lookup, key, *id)).collect(),
        scip: scip_targets.iter().map(|id| snapshot.describe(*id)).collect(),
      });
    }
  }
  Ok(Compared {
    per_kind,
    buckets,
    conflicts,
    unconfirmed,
    missed,
  })
}

#[derive(Clone, Copy)]
enum Unconfirmable {
  CrossPackage,
  FileLevel,
}

fn classify_unconfirmable(snapshot: &Snapshot, edge: &Edge) -> Option<Unconfirmable> {
  if matches!(snapshot.entity(edge.from).map(|entity| &entity.key), Some(EntityKey::File { .. })) {
    return Some(Unconfirmable::FileLevel);
  }
  let cross = package_of(snapshot.entity(edge.from))
    .zip(package_of(snapshot.entity(edge.to)))
    .is_some_and(|(from, to)| from != to);
  cross.then_some(Unconfirmable::CrossPackage)
}

struct SitePair {
  ours: usize,
  scip: usize,
}

fn site_parts(agreement: &Agreement) -> Option<(EntityId, &RepoName, &RelPath, atlas_core::Span)> {
  match &agreement.key {
    atlas_scip::ReconcileKey::Site { from, repo, path, span } => Some((*from, repo, path, *span)),
    atlas_scip::ReconcileKey::Relation { .. } => None,
  }
}

fn spans_overlap(left: &atlas_core::Span, right: &atlas_core::Span) -> bool {
  left.start_byte < right.end_byte && right.start_byte < left.end_byte
}

struct Sources {
  roots: HashMap<RepoName, PathBuf>,
  cache: HashMap<(RepoName, RelPath), Option<String>>,
}

impl Sources {
  fn new(project: &Project) -> Self {
    Self {
      roots: project.repos.iter().map(|repo| (repo.name.clone(), repo.path.clone())).collect(),
      cache: HashMap::new(),
    }
  }

  fn text(&mut self, repo: &RepoName, path: &RelPath, span: &atlas_core::Span) -> String {
    let roots = &self.roots;
    let source = self
      .cache
      .entry((repo.clone(), path.clone()))
      .or_insert_with(|| roots.get(repo).and_then(|root| fs::read_to_string(root.join(path.as_str())).ok()));
    source
      .as_ref()
      .and_then(|source| source.get(span.start_byte as usize..span.end_byte as usize))
      .unwrap_or_default()
      .to_owned()
  }
}

fn pair_sites(ours_only: &[Agreement], scip_only: &[Agreement], snapshot: &Snapshot, sources: &mut Sources) -> (Vec<SitePair>, Vec<SitePair>) {
  let mut by_site: HashMap<(EntityId, RepoName, RelPath), Vec<usize>> = HashMap::new();
  for (index, theirs) in scip_only.iter().enumerate() {
    if let Some((from, repo, path, _)) = site_parts(theirs) {
      by_site.entry((from, repo.clone(), path.clone())).or_default().push(index);
    }
  }
  let mut taken = vec![false; scip_only.len()];
  let mut consumed = vec![false; ours_only.len()];
  let mut agreed = Vec::new();
  let mut conflicts = Vec::new();
  for agree_pass in [true, false] {
    for (i, ours) in ours_only.iter().enumerate() {
      let Some((from, repo, path, span)) = site_parts(ours) else { continue };
      if consumed[i] {
        continue;
      }
      let callee = snapshot.entity(ours.target).map(|entity| entity.name.clone()).unwrap_or_default();
      let ours_text = sources.text(repo, path, &span);
      let candidates = by_site.get(&(from, repo.clone(), path.clone())).cloned().unwrap_or_default();
      let mut found = None;
      for j in candidates {
        let Some((_, _, _, other_span)) = site_parts(&scip_only[j]) else { continue };
        if taken[j] || (scip_only[j].target == ours.target) != agree_pass {
          continue;
        }
        let touches = match agree_pass {
          true => {
            spans_overlap(&span, &other_span)
              || (span.start_line == other_span.start_line && !callee.is_empty() && ours_text.contains(&callee) && sources.text(repo, path, &other_span).contains(&callee))
          }
          false => spans_overlap(&span, &other_span),
        };
        if touches {
          found = Some(j);
          break;
        }
      }
      let Some(j) = found else { continue };
      taken[j] = true;
      consumed[i] = true;
      match agree_pass {
        true => agreed.push(SitePair { ours: i, scip: j }),
        false => conflicts.push(SitePair { ours: i, scip: j }),
      }
    }
  }
  (agreed, conflicts)
}

fn describe_target(snapshot: &Snapshot, ours: &Lookup<'_>, key: &atlas_scip::ReconcileKey, id: EntityId) -> String {
  let tag = ours.get(&(key_hash(key), id)).map(|edge| edge.certainty.tag()).unwrap_or_default();
  format!("{} {tag}", snapshot.describe(id))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_derive_precision_and_recall_from_a_tally() {
    let tally = Tally {
      agreed: 8,
      agreed_respanned: 2,
      ours_only_file_level: 4,
      ours_only: 1,
      ours_only_cross_package: 5,
      scip_only: 1,
      conflicting: 1,
      scip_conflicting: 1,
    };
    assert_eq!(tally.precision_confirmable(), Some(0.8));
    assert_eq!(tally.precision_all_in(), Some(8.0 / 19.0));
    assert_eq!((tally.confirmable(), tally.total_edges()), (10, 19));
    assert_eq!(tally.recall(), Some(0.8));
    assert_eq!(Tally::default().precision_all_in(), None);
  }

  #[test]
  fn should_sample_conflicts_evenly_and_deterministically() {
    let conflicts: Vec<ConflictSample> = (0..100)
      .map(|index| ConflictSample {
        target: "t".into(),
        location: format!("r:p:{index:03}"),
        enclosing: String::new(),
        edge_kind: "calls".into(),
        ours: vec![],
        scip: vec![],
      })
      .collect();
    let picked = sample(conflicts.clone());
    assert_eq!(picked.len(), 20);
    assert_eq!(picked[0].location, "r:p:000");
    assert_eq!(picked[1].location, "r:p:005");
    assert_eq!(sample(conflicts.into_iter().rev().collect()), picked);
    assert_eq!(sample(vec![]).len(), 0);
  }

  #[test]
  fn should_normalise_target_names_for_directories() {
    assert_eq!(safe_name("adversarial/trait-dispatch-rust"), "adversarial_trait-dispatch-rust");
  }
}
