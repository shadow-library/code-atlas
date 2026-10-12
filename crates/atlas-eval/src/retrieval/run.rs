use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use atlas_core::{Entity, EntityId, EntityKind, RelPath, RepoName};
use atlas_engine::{Engine, EngineOptions, FreshnessReport};
use atlas_ide::{CrossRepo, DiscoveryLauncher};
use atlas_project::{AtlasHome, Project, Registry, RepoSpec};
use atlas_query::{
  ContextAnswer, ContextItem, ContextRequest, EntityRef, ImpactAnswer, ImpactRequest, QueryEngine, RenderText, SearchAnswer, SearchRequest, SimilarRequest, Targeted, TextOut,
};
use atlas_store::EntityFilter;

use super::error::{RetrievalError, RetrievalResult};
use super::lines::{ItemSpans, LineGoldSummary, LineScores, LineSet, line_count, line_gold, score_lines};
use super::patch::{OldLineChange, old_side_changes, patch_files, renamed_old_lines};
use super::phrases::{MAX_CODE_SPANS, MAX_PROSE_PHRASES, significant_phrases};
use super::queries::{NO_RECORDED_QUERY, QueryVariant, RecordedQueries, VariantQuery, load_recorded_queries, variant_queries};
use super::report::{
  BlastRadius, IndexSummary, PackSize, QueryLog, QueryScore, RenameRetrieval, RetrievalReport, RetrievalSettings, SeedOutcome, SimilarSeed, TaskEntry, TaskOutcome, TaskRetrieval,
  VariantOutcome, VariantResult,
};
use super::score::{BaseFile, PackCost, Retrieved, YIELD_BUDGETS, budget_yield, score_files, score_rename_sites, score_reuse, tree_path};
use crate::agent::{LoadedTask, RenameSpec, StripPlan, atlas_repos, load_tasks, materialize_task, resolve_source};
use crate::quality::{DryRunError, DryRunRequest, RenameOptions, dry_run_rename, observe, overlap, rename_settings};

pub const MAX_SIMILAR_SEEDS: usize = 5;
pub const BLAST_RADIUS_CATEGORY: &str = "blast-radius";
const REFERENCE_PATCH: &str = "reference.patch";
const PROJECT_NAME: &str = "retrieval";
const SCRATCH_PREFIX: &str = "atlas-retrieval-";
static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SEED_KINDS: [EntityKind; 11] = [
  EntityKind::Function,
  EntityKind::Method,
  EntityKind::Class,
  EntityKind::Struct,
  EntityKind::Enum,
  EntityKind::Trait,
  EntityKind::Interface,
  EntityKind::TypeAlias,
  EntityKind::Constant,
  EntityKind::Variant,
  EntityKind::Macro,
];
const PRODUCT_PATHS: [&str; 4] = ["crates", "Cargo.toml", "Cargo.lock", ":(exclude)crates/atlas-eval"];

#[derive(Debug, Clone)]
pub struct RetrievalOptions {
  pub repo_root: PathBuf,
  pub tasks_dir: PathBuf,
  pub only: Vec<String>,
  pub cache_dir: PathBuf,
  pub work_dir: PathBuf,
}

pub fn settings() -> RetrievalSettings {
  RetrievalSettings {
    context_budget: ContextRequest::new("").budget,
    search_limit: SearchRequest::new("").limit,
    max_code_spans: MAX_CODE_SPANS,
    max_prose_phrases: MAX_PROSE_PHRASES,
    max_similar_seeds: MAX_SIMILAR_SEEDS,
    impact_group_limit: ImpactRequest::new([""]).limit,
    yield_budgets: YIELD_BUDGETS.to_vec(),
  }
}

pub fn run_retrieval(options: &RetrievalOptions) -> RetrievalResult<RetrievalReport> {
  let tasks = load_tasks(&options.tasks_dir, &options.only)?;
  let mut entries = Vec::new();
  for task in &tasks {
    tracing::info!(task = %task.spec.id, "evaluating retrieval");
    let outcome = match evaluate_task(task, options) {
      Ok(retrieval) => TaskOutcome::Scored(Box::new(retrieval)),
      Err(failure) => {
        tracing::warn!(task = %task.spec.id, stage = failure.stage, error = %failure.error, "retrieval evaluation failed");
        TaskOutcome::Failed {
          stage: failure.stage.to_owned(),
          reason: failure.error.to_string(),
        }
      }
    };
    entries.push(TaskEntry {
      id: task.spec.id.clone(),
      title: task.spec.title.clone(),
      categories: task.spec.category.clone(),
      outcome,
    });
  }
  Ok(RetrievalReport::new(product_revision(&options.repo_root), settings(), entries))
}

struct StageFailure {
  stage: &'static str,
  error: RetrievalError,
}

trait AtStage<T> {
  fn at(self, stage: &'static str) -> Result<T, StageFailure>;
}

impl<T, E: Into<RetrievalError>> AtStage<T> for Result<T, E> {
  fn at(self, stage: &'static str) -> Result<T, StageFailure> {
    self.map_err(|error| StageFailure { stage, error: error.into() })
  }
}

struct Scratch {
  dir: PathBuf,
}

impl Scratch {
  fn create(dir: PathBuf) -> RetrievalResult<Self> {
    remove_dir(&dir)?;
    fs::create_dir_all(&dir).map_err(|error| RetrievalError::io(&dir, error))?;
    Ok(Self { dir })
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    if let Err(error) = remove_dir(&self.dir) {
      tracing::warn!(dir = %self.dir.display(), %error, "cannot remove the retrieval scratch directory");
    }
  }
}

fn remove_dir(dir: &Path) -> RetrievalResult<()> {
  match fs::remove_dir_all(dir) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(RetrievalError::io(dir, error)),
    _ => Ok(()),
  }
}

struct Indexed {
  project: Project,
  engine: Arc<Engine>,
  query: QueryEngine,
  prefixes: BTreeMap<String, String>,
  tree: PathBuf,
}

fn evaluate_task(task: &LoadedTask, options: &RetrievalOptions) -> Result<TaskRetrieval, StageFailure> {
  let digest = blake3::hash(task.spec.id.as_bytes()).to_hex();
  let unique = format!("{}-{}", std::process::id(), SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed));
  let recorded = load_recorded_queries(&task.dir).at("queries")?;
  let scratch = Scratch::create(options.work_dir.join(format!("{SCRATCH_PREFIX}{unique}-{}", &digest[..12]))).at("scratch")?;
  let tree = scratch.dir.join("tree");
  let source = resolve_source(task, &options.repo_root, None, &options.cache_dir).at("source")?;
  materialize_task(task, &source, &tree, &StripPlan::for_task(task, false)).at("materialize")?;
  let indexed = index(task, &scratch.dir, &tree).at("index")?;
  let (index, files) = index_summary(&indexed).at("index")?;
  query_task(task, &indexed, index, &files, &recorded).at("query")
}

fn index(task: &LoadedTask, scratch: &Path, tree: &Path) -> RetrievalResult<Indexed> {
  let home = AtlasHome::rooted_at(scratch.join("home"));
  let mut registry = Registry::load(&home)?;
  let specs = atlas_repos(task, tree).into_iter().map(RepoSpec::new).collect();
  let project = registry.create_project(PROJECT_NAME, specs)?.clone();
  let tree = tree.canonicalize().map_err(|error| RetrievalError::io(tree, error))?;
  let prefixes = project
    .repos
    .iter()
    .filter_map(|repo| {
      let relative = repo.path.strip_prefix(&tree).ok()?.to_str()?.replace('\\', "/");
      Some((repo.name.to_string(), relative))
    })
    .collect();
  let engine = Arc::new(Engine::open(&project, &home, EngineOptions::default())?);
  let query = QueryEngine::new(Arc::clone(&engine));
  Ok(Indexed {
    project,
    engine,
    query,
    prefixes,
    tree,
  })
}

fn index_summary(indexed: &Indexed) -> RetrievalResult<(IndexSummary, BTreeSet<String>)> {
  let degraded = match indexed.engine.ensure_fresh()? {
    FreshnessReport::Fresh(_) => None,
    FreshnessReport::Degraded { reason, failed_files, .. } => Some(format!("{reason} ({} failed files)", failed_files.len())),
  };
  let files = indexed.engine.store().read(|tx| -> RetrievalResult<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    for repo in &indexed.project.repos {
      let records = tx.files_in_repo(&repo.name)?;
      files.extend(records.iter().filter_map(|record| tree_path(&indexed.prefixes, record.repo.as_str(), record.path.as_str())));
    }
    Ok(files)
  })?;
  let summary = IndexSummary {
    repos: indexed.project.repos.iter().map(|repo| repo.name.to_string()).collect(),
    files: files.len(),
    degraded,
  };
  Ok((summary, files))
}

fn classify(indexed: &Indexed, files: &BTreeSet<String>, path: &str) -> BaseFile {
  if files.contains(path) {
    return BaseFile::Indexed;
  }
  match indexed.tree.join(path).is_file() {
    true => BaseFile::Unindexable,
    false => BaseFile::Absent,
  }
}

struct TaskTruth<'a> {
  task: &'a LoadedTask,
  indexed: &'a Indexed,
  files: &'a BTreeSet<String>,
  gold_lines: Option<LineSet>,
}

fn query_task(task: &LoadedTask, indexed: &Indexed, index: IndexSummary, files: &BTreeSet<String>, recorded: &RecordedQueries) -> RetrievalResult<TaskRetrieval> {
  let (gold_lines, line_gold) = task_line_gold(task, indexed, files);
  let truth = TaskTruth { task, indexed, files, gold_lines };
  let mut variants = Vec::new();
  for variant in QueryVariant::ALL {
    let queries = variant_queries(variant, &task.spec.prompt, recorded);
    let outcome = match queries.is_empty() {
      true => VariantOutcome::Unavailable {
        reason: NO_RECORDED_QUERY.to_owned(),
      },
      false => VariantOutcome::Scored {
        queries: queries.into_iter().map(|query| score_query(&truth, query)).collect::<RetrievalResult<_>>()?,
      },
    };
    variants.push(VariantResult { variant, outcome });
  }
  let blast_radius = task
    .spec
    .category
    .iter()
    .any(|category| category == BLAST_RADIUS_CATEGORY)
    .then(|| blast_radius(task, indexed, files))
    .transpose()?;
  Ok(TaskRetrieval {
    index,
    line_gold,
    no_call: recorded.no_call.clone(),
    variants,
    blast_radius,
    rename: task.spec.rename.as_ref().map(|rename| rename_retrieval(task, rename, indexed)),
  })
}

fn task_line_gold(task: &LoadedTask, indexed: &Indexed, files: &BTreeSet<String>) -> (Option<LineSet>, LineGoldSummary) {
  let Ok(patch) = fs::read_to_string(task.dir.join(REFERENCE_PATCH)) else {
    let reason = format!("the task has no readable {REFERENCE_PATCH}");
    return (None, LineGoldSummary::Unavailable { reason });
  };
  let (gold, summary) = line_gold(&patch_files(&patch), |path| {
    let state = classify(indexed, files, path);
    let lines = fs::read(indexed.tree.join(path)).map(|bytes| line_count(&bytes)).unwrap_or(0);
    (state, lines)
  });
  (Some(gold), summary)
}

fn score_query(truth: &TaskTruth<'_>, query: VariantQuery) -> RetrievalResult<QueryScore> {
  let indexed = truth.indexed;
  let request = ContextRequest {
    seeds: query.seeds.clone(),
    ..ContextRequest::new(query.text.as_str())
  };
  let context = indexed.query.context(&request)?.answer;
  let mut retrieved = Retrieved::default();
  collect_pack(indexed, &context, &mut retrieved);
  let phrases = significant_phrases(&query.text);
  let texts = std::iter::once(query.text.clone()).chain(phrases.iter().map(|phrase| phrase.text.clone()));
  let mut seeds: Vec<String> = Vec::new();
  for text in texts {
    let answer = indexed.query.search(&SearchRequest::new(text))?.answer;
    for entity in search_entities(&answer) {
      collect(indexed, entity, &mut retrieved.search_files, &mut retrieved.search_symbols);
    }
    if let Some(seed) = similar_seed(&answer).filter(|seed| !seeds.contains(seed)) {
      seeds.push(seed);
    }
  }
  seeds.truncate(MAX_SIMILAR_SEEDS);
  let mut similar_seeds = Vec::new();
  for key in seeds {
    let outcome = match indexed.query.similar(&SimilarRequest::new(key.clone()))?.answer {
      Targeted::Found(answer) => {
        for entity in answer
          .members
          .iter()
          .map(|member| &member.entity)
          .chain(answer.shape_only.iter().map(|shape| &shape.entity))
        {
          collect(indexed, entity, &mut retrieved.similar_files, &mut retrieved.similar_symbols);
        }
        SeedOutcome::Found {
          members: answer.members.len(),
          shape_only: answer.shape_only.len(),
        }
      }
      Targeted::NotFound { .. } => SeedOutcome::NotFound,
      Targeted::Ambiguous { candidates, .. } => SeedOutcome::Ambiguous { candidates: candidates.len() },
    };
    similar_seeds.push(SimilarSeed { key, outcome });
  }
  let labelled = &truth.task.spec.ground_truth;
  let base_file = |path: &str| classify(indexed, truth.files, path);
  let must_change = score_files(&retrieved, &labelled.must_change, base_file);
  let must_change_at_base: BTreeSet<String> = must_change.expected.iter().cloned().collect();
  let mut yields = Vec::new();
  for budget in YIELD_BUDGETS {
    let answer = indexed.query.context(&ContextRequest { budget, ..request.clone() })?.answer;
    let mut at_budget = Retrieved::default();
    collect_pack(indexed, &answer, &mut at_budget);
    let size = pack_size(&answer);
    let cost = PackCost {
      budget,
      estimated_tokens: size.estimated_tokens,
      bytes: size.bytes,
    };
    yields.push(budget_yield(cost, &at_budget, &must_change_at_base));
  }
  let lines = match &truth.gold_lines {
    None => None,
    Some(gold) => Some(LineScores {
      must_read: score_lines(&item_spans(indexed, context.must_read.iter())?, gold),
      pack: score_lines(&item_spans(indexed, pack_sections(&context).into_iter().flatten())?, gold),
    }),
  };
  Ok(QueryScore {
    query,
    searches: QueryLog {
      search_phrases: phrases,
      similar_seeds,
    },
    pack: pack_size(&context),
    must_change,
    relevant_files: score_files(&retrieved, &labelled.relevant_files, base_file),
    must_consider: score_files(&retrieved, &labelled.must_consider, base_file),
    reuse: (!labelled.reuse_targets.is_empty()).then(|| score_reuse(&retrieved, &labelled.reuse_targets)),
    lines,
    yields,
  })
}

fn item_spans<'a>(indexed: &Indexed, items: impl Iterator<Item = &'a ContextItem>) -> RetrievalResult<ItemSpans> {
  indexed.engine.store().read(|tx| -> RetrievalResult<ItemSpans> {
    let mut spans = ItemSpans::default();
    for item in items {
      let entity = match item.entity.id.parse::<EntityId>() {
        Ok(id) => tx.entity(id)?,
        Err(_) => None,
      };
      spans.add(entity.as_ref().and_then(|entity| entity_span(indexed, entity)));
    }
    Ok(spans)
  })
}

fn entity_span(indexed: &Indexed, entity: &Entity) -> Option<(String, u32, u32)> {
  let span = entity.span?;
  let file = tree_path(&indexed.prefixes, entity.repo.as_ref()?.as_str(), entity.path.as_ref()?.as_str())?;
  Some((file, span.start_line, span.end_line))
}

fn rename_retrieval(task: &LoadedTask, rename: &RenameSpec, indexed: &Indexed) -> RenameRetrieval {
  let unavailable = |reason: String| RenameRetrieval::Unavailable { reason };
  let patch_path = task.dir.join(REFERENCE_PATCH);
  let Ok(patch) = fs::read_to_string(&patch_path) else {
    return unavailable(format!("the task has no readable {REFERENCE_PATCH}"));
  };
  let expected = renamed_old_lines(&patch, rename.old_name(), &rename.new_name);
  if expected.is_empty() {
    return unavailable(format!("{REFERENCE_PATCH} replaces `{}` by `{}` on no line", rename.old_name(), rename.new_name));
  }
  let request = DryRunRequest {
    target: rename.symbol.clone(),
    new_name: rename.new_name.clone(),
    cross_repo: CrossRepo::Report,
  };
  let settings = rename_settings(&RenameOptions::from_env());
  let run = match dry_run_rename(&indexed.engine, Arc::new(DiscoveryLauncher), settings, &request) {
    Ok(run) => run,
    Err(DryRunError::NoServer(reason)) => return unavailable(format!("no language server: {reason}")),
    Err(DryRunError::Failed { reason, .. }) => return unavailable(format!("the dry run failed: {reason}")),
  };
  let observed = match observe(&indexed.project, &run.plan, Some(rename.old_name())) {
    Ok(observed) => observed,
    Err(reason) => return unavailable(reason),
  };
  let planned: BTreeSet<(String, u32)> = observed
    .edits
    .iter()
    .filter_map(|edit| Some((tree_path(&indexed.prefixes, &edit.repo, &edit.path)?, edit.line)))
    .collect();
  RenameRetrieval::Scored {
    target: rename.symbol.clone(),
    new_name: rename.new_name.clone(),
    servers: run.servers.iter().map(|server| format!("{} {}", server.server, server.version)).collect(),
    score: score_rename_sites(&expected, &planned),
  }
}

fn pack_sections(context: &ContextAnswer) -> [&[ContextItem]; 7] {
  [
    &context.must_read,
    &context.skim,
    &context.patterns,
    &context.reuse,
    &context.impact,
    &context.tests,
    &context.uncertain,
  ]
}

fn collect_pack(indexed: &Indexed, context: &ContextAnswer, retrieved: &mut Retrieved) {
  let mut must_read_symbols = BTreeSet::new();
  for item in &context.must_read {
    collect(indexed, &item.entity, &mut retrieved.must_read_files, &mut must_read_symbols);
  }
  for item in pack_sections(context).into_iter().flatten() {
    collect(indexed, &item.entity, &mut retrieved.pack_files, &mut retrieved.pack_symbols);
  }
}

fn collect(indexed: &Indexed, entity: &EntityRef, files: &mut BTreeSet<String>, symbols: &mut BTreeSet<String>) {
  symbols.insert(entity.key.clone());
  if let Some(path) = entity.location.as_ref().and_then(|location| tree_path(&indexed.prefixes, &location.repo, &location.path)) {
    files.insert(path);
  }
}

fn search_entities(answer: &SearchAnswer) -> impl Iterator<Item = &EntityRef> {
  answer
    .exact_name
    .iter()
    .chain(&answer.strong)
    .chain(&answer.related)
    .map(|hit| &hit.entity)
    .chain(answer.structurally_similar.iter().map(|hit| &hit.entity))
    .chain(answer.inferred_capabilities.iter().map(|hit| &hit.entity))
}

fn similar_seed(answer: &SearchAnswer) -> Option<String> {
  answer
    .exact_name
    .iter()
    .chain(&answer.strong)
    .chain(&answer.related)
    .find(|hit| matches!(hit.entity.kind.parse::<EntityKind>(), Ok(EntityKind::Function | EntityKind::Method)))
    .map(|hit| hit.entity.key.clone())
}

fn pack_size(context: &ContextAnswer) -> PackSize {
  let mut out = TextOut::default();
  context.render_text(&mut out);
  PackSize {
    bytes: out.finish().len(),
    estimated_tokens: context.estimated_tokens,
    budget: context.budget,
    must_read: context.must_read.len(),
    skim: context.skim.len(),
    patterns: context.patterns.len(),
    reuse: context.reuse.len(),
    impact: context.impact.len(),
    tests: context.tests.len(),
    uncertain: context.uncertain.len(),
    omitted: context.omitted.total,
  }
}

fn blast_radius(task: &LoadedTask, indexed: &Indexed, files: &BTreeSet<String>) -> RetrievalResult<BlastRadius> {
  let unavailable = |reason: String| Ok(BlastRadius::Unavailable { reason });
  let truth = &task.spec.ground_truth;
  let Some(definition) = truth.must_change.first() else {
    return unavailable("the task lists no must_change files".to_owned());
  };
  let patch_path = task.dir.join(REFERENCE_PATCH);
  let Ok(patch) = fs::read_to_string(&patch_path) else {
    return unavailable(format!("the task has no readable {REFERENCE_PATCH}"));
  };
  let changes = old_side_changes(&patch, definition);
  if changes.is_empty() {
    return unavailable(format!("{REFERENCE_PATCH} does not change `{definition}`"));
  }
  let Some((repo, path)) = repo_location(&indexed.prefixes, definition) else {
    return unavailable(format!("`{definition}` is outside every indexed repository"));
  };
  let seeds = seed_symbols(indexed, &repo, &path, &changes)?;
  if seeds.is_empty() {
    return unavailable(format!("no indexed symbol encloses the changes {REFERENCE_PATCH} makes to `{definition}`"));
  }
  let expected: BTreeSet<String> = truth.must_change.iter().filter(|file| *file != definition && files.contains(*file)).cloned().collect();
  let visible_request = ImpactRequest::new(seeds.iter().cloned());
  let full_request = ImpactRequest {
    limit: usize::MAX,
    ..visible_request.clone()
  };
  let (visible, collapsed_visible) = match impact_files(indexed, &visible_request)? {
    Ok(found) => found,
    Err(reason) => return unavailable(reason),
  };
  let (full, _) = match impact_files(indexed, &full_request)? {
    Ok(found) => found,
    Err(reason) => return unavailable(reason),
  };
  Ok(BlastRadius::Scored {
    definition_file: definition.clone(),
    seeds,
    missed: expected.difference(&full).cloned().collect(),
    visible: overlap(&visible, &expected),
    full: overlap(&full, &expected),
    expected: expected.into_iter().collect(),
    collapsed_visible,
  })
}

fn repo_location(prefixes: &BTreeMap<String, String>, file: &str) -> Option<(RepoName, RelPath)> {
  let (repo, prefix) = prefixes
    .iter()
    .filter(|(_, prefix)| prefix.is_empty() || file.starts_with(&format!("{prefix}/")))
    .max_by_key(|(_, prefix)| prefix.len())?;
  let relative = match prefix.is_empty() {
    true => file,
    false => &file[prefix.len() + 1..],
  };
  Some((RepoName::new(repo).ok()?, RelPath::new(relative).ok()?))
}

fn seed_symbols(indexed: &Indexed, repo: &RepoName, path: &RelPath, changes: &BTreeSet<OldLineChange>) -> RetrievalResult<Vec<String>> {
  let entities = indexed.engine.store().read(|tx| tx.entities_in_file(repo, path, &EntityFilter::default()))?;
  let candidates: Vec<(u32, String, u32, u32)> = entities
    .iter()
    .filter(|entity| SEED_KINDS.contains(&entity.kind))
    .filter_map(|entity| {
      let span = entity.span?;
      Some((span.end_line.saturating_sub(span.start_line), entity.key.to_string(), span.start_line, span.end_line))
    })
    .collect();
  let mut seeds = BTreeSet::new();
  for change in changes {
    let innermost = candidates
      .iter()
      .filter(|(_, _, start, end)| change.enclosed_by(*start, *end))
      .min_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    if let Some((_, key, _, _)) = innermost {
      seeds.insert(key.clone());
    }
  }
  Ok(seeds.into_iter().collect())
}

fn impact_files(indexed: &Indexed, request: &ImpactRequest) -> RetrievalResult<Result<(BTreeSet<String>, usize), String>> {
  let answer: ImpactAnswer = match indexed.query.impact(request)?.answer {
    Targeted::Found(answer) => answer,
    Targeted::NotFound { spec, .. } => return Ok(Err(format!("impact cannot resolve `{spec}`"))),
    Targeted::Ambiguous { spec, .. } => return Ok(Err(format!("impact finds `{spec}` ambiguous"))),
  };
  let groups = [
    &answer.direct,
    &answer.transitive,
    &answer.endpoints,
    &answer.consumers,
    &answer.inferred_consumers,
    &answer.tests,
    &answer.config,
  ];
  let mut files = BTreeSet::new();
  let mut symbols = BTreeSet::new();
  for item in groups.iter().flat_map(|group| &group.items) {
    collect(indexed, &item.entity, &mut files, &mut symbols);
  }
  Ok(Ok((files, groups.iter().map(|group| group.collapsed).sum())))
}

fn product_revision(repo_root: &Path) -> Option<String> {
  let git = |args: &[&str]| -> Option<String> {
    let output = Command::new("git").arg("-C").arg(repo_root).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
  };
  let head = git(&["rev-parse", "HEAD"])?;
  let mut status = vec!["status", "--porcelain", "--"];
  status.extend(PRODUCT_PATHS);
  match git(&status) {
    Some(changes) if changes.is_empty() => Some(head),
    Some(_) => Some(format!("{head} with uncommitted product changes")),
    None => Some(format!("{head} (working tree state unknown)")),
  }
}
