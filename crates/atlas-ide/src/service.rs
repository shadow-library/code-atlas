use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use atlas_core::RunId;
use atlas_engine::Engine;
use atlas_lsp::protocol::ServerCapabilities;
use atlas_lsp::{ChangeList, Lease, LspError, LspPool, RestartReason, ServerKind, SyncDemand, SyncLimits, UnitKey, UnitPhase, UnitScope, resident_kib};
use atlas_project::{AtlasHome, Project};
use atlas_query::{FindAnswer, FindRequest, Freshness, HierarchyAnswer, HierarchyRequest, QueryEngine, Response, Targeted};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{IdeError, IdeResult};
use crate::render::{ServerView, StatusAnswer, UnitView};
use crate::servers::{IdeMode, IdeSettings, LaunchRequest, LaunchSpec, Launcher, TsServer, TsServerOrigin, UnitInputs, UnitMode, UnitPlan, kind_of, unavailable};
use crate::sync::{LedgerChanges, Manifests, SyncNotes, change_list, display_path, manifest_changes, manifests, package_dirs};
use crate::target::{FileRef, ProjectFiles};

const TYPESCRIPT_7: &str = "TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one";
const REFUSAL_MEMORY: Duration = Duration::from_secs(60);
const CONTENT_MODIFIED: i64 = -32801;
const REQUEST_CANCELLED: i64 = -32800;

pub struct IdeOptions {
  pub mode: IdeMode,
  pub settings: IdeSettings,
  pub cache_dir: PathBuf,
  pub sync: SyncLimits,
}

impl IdeOptions {
  pub fn for_project(project: &Project, home: &AtlasHome, mode: IdeMode) -> Self {
    Self::new(mode, IdeSettings::from(&project.settings.lsp), home.project_data_dir(&project.name))
  }

  pub fn new(mode: IdeMode, settings: IdeSettings, cache_dir: impl Into<PathBuf>) -> Self {
    Self {
      mode,
      settings,
      cache_dir: cache_dir.into(),
      sync: SyncLimits::default(),
    }
  }
}

struct Config {
  settings: IdeSettings,
  pool: Arc<LspPool>,
}

type PackageDirs = (Option<RunId>, Vec<PathBuf>);
type LaunchKey = (ServerKind, UnitScope, Vec<PathBuf>);

pub struct IdeService {
  pub(crate) query: QueryEngine,
  launcher: Arc<dyn Launcher>,
  mode: IdeMode,
  sync: SyncLimits,
  cache_dir: PathBuf,
  config: RwLock<Config>,
  unit_locks: Mutex<HashMap<UnitKey, Arc<Mutex<()>>>>,
  launches: Mutex<HashMap<LaunchKey, LaunchSpec>>,
  refusals: Mutex<HashMap<LaunchKey, Instant>>,
  lockfiles: Mutex<BTreeMap<UnitKey, Vec<String>>>,
  manifests: Mutex<HashMap<UnitKey, (Option<u32>, Manifests)>>,
  package_dirs: Mutex<HashMap<UnitKey, PackageDirs>>,
}

pub(crate) struct Session<'a> {
  pub lease: Lease,
  pub kind: ServerKind,
  pub query: &'a QueryEngine,
  pub files: &'a ProjectFiles,
  pub notes: SyncNotes,
  pub settings: IdeSettings,
  launch: LaunchSpec,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Session<'_> {
  pub(crate) fn capabilities(&self) -> &ServerCapabilities {
    &self.lease.initialize_result().capabilities
  }

  pub(crate) fn request<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> IdeResult<R> {
    let params = serde_json::to_value(params).map_err(|error| LspError::Encode {
      method: method.to_owned(),
      message: error.to_string(),
    })?;
    match self.lease.request(method, params.clone()) {
      Err(LspError::Response { error, .. }) if error.code == CONTENT_MODIFIED || error.code == REQUEST_CANCELLED => {
        tracing::debug!(method, code = error.code, "the language server dropped the request while its state changed; retrying once");
        Ok(self.lease.request(method, params)?)
      }
      result => Ok(result?),
    }
  }

  pub(crate) fn server_name(&self) -> &'static str {
    self.kind.name()
  }

  fn view(&mut self) -> ServerView {
    self.notes.lease_state(&self.lease, self.files);
    let version = self
      .lease
      .server_info()
      .and_then(|info| info.version.clone())
      .unwrap_or_else(|| self.launch.server_version.clone());
    let typescript = match self.kind {
      ServerKind::TypeScript => self
        .lease
        .typescript_version()
        .map(|announced| announced.version)
        .filter(|version| !version.is_empty())
        .or_else(|| self.launch.tsserver.as_ref().map(|tsserver| tsserver.version.clone())),
      ServerKind::RustAnalyzer => None,
    };
    ServerView::new(self.kind.name(), version, typescript, &self.notes)
  }
}

impl IdeService {
  pub fn new(engine: Arc<Engine>, launcher: Arc<dyn Launcher>, options: IdeOptions) -> Self {
    let pool = Arc::new(LspPool::new(options.mode.pool_options(&options.settings, options.sync)));
    Self {
      query: QueryEngine::new(engine),
      launcher,
      mode: options.mode,
      sync: options.sync,
      cache_dir: options.cache_dir,
      config: RwLock::new(Config { settings: options.settings, pool }),
      unit_locks: Mutex::new(HashMap::new()),
      launches: Mutex::new(HashMap::new()),
      refusals: Mutex::new(HashMap::new()),
      lockfiles: Mutex::new(BTreeMap::new()),
      manifests: Mutex::new(HashMap::new()),
      package_dirs: Mutex::new(HashMap::new()),
    }
  }

  pub fn query(&self) -> &QueryEngine {
    &self.query
  }

  pub fn settings(&self) -> IdeSettings {
    self.config.read().unwrap_or_else(PoisonError::into_inner).settings.clone()
  }

  fn pool(&self) -> Arc<LspPool> {
    Arc::clone(&self.config.read().unwrap_or_else(PoisonError::into_inner).pool)
  }

  pub fn reload(&self, project: &Project) {
    self.reload_settings(IdeSettings::from(&project.settings.lsp));
  }

  pub fn reload_settings(&self, settings: IdeSettings) {
    let retired = {
      let mut config = self.config.write().unwrap_or_else(PoisonError::into_inner);
      let pool_changed = self.mode.pool_options(&config.settings, self.sync) != self.mode.pool_options(&settings, self.sync);
      let disabled = config.settings.enabled && !settings.enabled;
      config.settings = settings;
      match pool_changed || disabled {
        true => {
          let fresh = Arc::new(LspPool::new(self.mode.pool_options(&config.settings, self.sync)));
          Some(std::mem::replace(&mut config.pool, fresh))
        }
        false => None,
      }
    };
    guard(&self.launches).clear();
    guard(&self.refusals).clear();
    if let Some(pool) = retired {
      pool.shutdown();
    }
  }

  pub fn shutdown(&self) {
    self.pool().shutdown();
  }

  pub fn find(&self, request: &FindRequest) -> IdeResult<Response<FindAnswer>> {
    Ok(self.query.find(request)?)
  }

  pub fn hierarchy(&self, request: &HierarchyRequest) -> IdeResult<Response<Targeted<HierarchyAnswer>>> {
    Ok(self.query.hierarchy(request)?)
  }

  pub(crate) fn files(&self) -> ProjectFiles {
    ProjectFiles::new(&self.query.engine().project())
  }

  pub(crate) fn ensure_enabled(&self) -> IdeResult<IdeSettings> {
    let settings = self.settings();
    match settings.enabled {
      true => Ok(settings),
      false => Err(IdeError::Disabled {
        project: self.query.engine().project().name.to_string(),
        alternative: None,
      }),
    }
  }

  pub(crate) fn exact<T>(
    &self,
    freshness: &Freshness,
    files: &ProjectFiles,
    file: &FileRef,
    demand: SyncDemand,
    open: &[PathBuf],
    op: impl FnOnce(&mut Session<'_>) -> IdeResult<T>,
  ) -> IdeResult<(ServerView, T)> {
    let settings = self.ensure_enabled()?;
    let kind = kind_of(&file.absolute).ok_or_else(|| IdeError::UnsupportedFile { path: file.to_string() })?;
    let scope = match settings.unit {
      UnitMode::Project => UnitScope::Project,
      UnitMode::Repo => UnitScope::Repo(file.repo.to_string()),
    };
    let (plan, launch, launch_key) = self.plan(kind, &scope, &settings)?;
    if guard(&self.refusals).get(&launch_key).is_some_and(|refused| refused.elapsed() < REFUSAL_MEMORY) {
      return Err(unavailable(kind, TYPESCRIPT_7));
    }
    plan.covers(&file.absolute).map_err(|reason| IdeError::NotCovered {
      path: file.to_string(),
      server: kind.name().to_owned(),
      reason,
    })?;
    let unit_lock = self.unit_lock(&plan.unit.key());
    let _serialised = guard(&unit_lock);
    let generation = freshness.run.map(RunId);
    let store = self.query.engine().store();
    let pool = self.pool();
    let key = plan.unit.key();
    let manifest_dirs = self.manifest_dirs(&key, generation, files, &plan)?;
    let pid = pool.get(&plan.unit, generation)?.pid();
    let baseline = guard(&self.manifests).get(&key).filter(|(known, _)| *known == pid).map(|(_, manifests)| manifests.clone());
    let current = manifests(&manifest_dirs);
    let announced = Cell::new(false);
    let ledger = RefCell::new(LedgerChanges::default());
    let changes = |since| {
      announced.set(true);
      match (change_list(store, files, since, generation, &mut ledger.borrow_mut()), &baseline) {
        (ChangeList::Known(mut changes), Some(baseline)) => {
          changes.extend(manifest_changes(baseline, &current));
          ChangeList::Known(changes)
        }
        (list, _) => list,
      }
    };
    let synced = pool.sync(&plan.unit, generation, demand, changes, open)?;
    let restarted = synced.lease.pid() != pid;
    let snapshot = match (baseline, announced.get() || restarted) {
      (Some(baseline), false) => baseline,
      _ => current,
    };
    guard(&self.manifests).insert(key, (synced.lease.pid(), snapshot));
    let mut notes = SyncNotes::of(&synced.report, files);
    let graphs_changed = ledger.into_inner().graphs_changed;
    if !graphs_changed.is_empty() {
      notes.caveats.push(format!(
        "project configuration changed in {} since the server last synced; the server reloads it on its own and may not have finished",
        graphs_changed.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
      ));
    }
    match typescript_report(&synced.lease, &launch) {
      TypeScriptReport::Supported => {}
      TypeScriptReport::Unreported => notes
        .caveats
        .push("the server did not report its TypeScript version; the header shows the version discovery expected".to_owned()),
      TypeScriptReport::Contradicts(warning) => notes.warnings.push(warning),
      TypeScriptReport::Unsupported(version) => {
        tracing::warn!(version, "the TypeScript language server loaded TypeScript 7 or later; retiring it");
        pool.retire(
          &plan.unit,
          synced.lease,
          RestartReason::Refused {
            reason: format!("it loaded TypeScript {version}"),
          },
        );
        guard(&self.refusals).insert(launch_key, Instant::now());
        return Err(unavailable(kind, TYPESCRIPT_7));
      }
    }
    let mut session = Session {
      notes,
      lease: synced.lease,
      kind,
      query: &self.query,
      files,
      settings,
      launch,
    };
    let result = op(&mut session)?;
    let view = session.view();
    let lockfiles = session.lease.lockfile_changes().iter().map(|path| display_path(path, files)).collect();
    self.remember_lockfiles(&plan.unit.key(), lockfiles);
    Ok((view, result))
  }

  fn plan(&self, kind: ServerKind, scope: &UnitScope, settings: &IdeSettings) -> IdeResult<(UnitPlan, LaunchSpec, LaunchKey)> {
    let units = self.query.engine().language_units()?;
    let project = self.query.engine().project().name.to_string();
    let inputs = UnitInputs {
      project: &project,
      kind,
      scope,
      units: &units,
      settings,
      cache_dir: &self.cache_dir,
    };
    let roots: Vec<PathBuf> = inputs.folders().into_iter().map(|folder| folder.root).collect();
    let key = (kind, scope.clone(), roots.clone());
    let cached = guard(&self.launches).get(&key).cloned();
    let launch = match cached {
      Some(launch) => launch,
      None => {
        let launch = self.launcher.launch(&LaunchRequest {
          kind,
          repo_roots: &roots,
          settings,
        })?;
        guard(&self.launches).insert(key.clone(), launch.clone());
        launch
      }
    };
    Ok((inputs.plan(&launch)?, launch, key))
  }

  fn manifest_dirs(&self, key: &UnitKey, generation: Option<RunId>, files: &ProjectFiles, plan: &UnitPlan) -> IdeResult<Vec<PathBuf>> {
    if let Some((_, dirs)) = guard(&self.package_dirs).get(key).filter(|(known, _)| *known == generation) {
      return Ok(dirs.clone());
    }
    let mut dirs = self.query.engine().store().read(|tx| package_dirs(tx, files, &plan.roots))?;
    dirs.extend(plan.cargo_dirs.iter().cloned());
    dirs.sort();
    dirs.dedup();
    guard(&self.package_dirs).insert(key.clone(), (generation, dirs.clone()));
    Ok(dirs)
  }

  fn unit_lock(&self, key: &UnitKey) -> Arc<Mutex<()>> {
    Arc::clone(guard(&self.unit_locks).entry(key.clone()).or_default())
  }

  fn remember_lockfiles(&self, key: &UnitKey, changed: Vec<String>) {
    let mut lockfiles = guard(&self.lockfiles);
    match changed.is_empty() {
      true => lockfiles.remove(key),
      false => lockfiles.insert(key.clone(), changed),
    };
  }

  pub fn status(&self) -> StatusAnswer {
    let settings = self.settings();
    let lockfiles = guard(&self.lockfiles).clone();
    let units = self
      .pool()
      .status()
      .into_iter()
      .map(|status| {
        let warnings = lockfiles.get(&status.key).cloned().unwrap_or_default();
        let rss = match &status.phase {
          UnitPhase::Warming(summary) | UnitPhase::Running(summary) => summary.pid.and_then(resident_kib),
          UnitPhase::Starting | UnitPhase::Backoff { .. } => None,
        };
        UnitView::of(&status, rss, &warnings)
      })
      .collect();
    StatusAnswer {
      enabled: settings.enabled,
      mode: self.mode,
      units,
    }
  }
}

enum TypeScriptReport {
  Supported,
  Unreported,
  Contradicts(String),
  Unsupported(String),
}

fn typescript_report(lease: &Lease, launch: &LaunchSpec) -> TypeScriptReport {
  if lease.kind() != ServerKind::TypeScript {
    return TypeScriptReport::Supported;
  }
  let Some(reported) = lease.typescript_version().map(|announced| announced.version).filter(|version| !version.is_empty()) else {
    return TypeScriptReport::Unreported;
  };
  if major(&reported).is_some_and(|major| major >= 7) {
    return TypeScriptReport::Unsupported(reported);
  }
  match &launch.tsserver {
    Some(TsServer {
      version,
      origin: TsServerOrigin::Workspace { repo_root },
      ..
    }) if *version != reported => TypeScriptReport::Contradicts(format!(
      "discovery chose the workspace TypeScript {version} in {}, but the server loaded TypeScript {reported}",
      repo_root.display()
    )),
    _ => TypeScriptReport::Supported,
  }
}

fn major(version: &str) -> Option<u32> {
  version.trim().split('.').next()?.parse().ok()
}

impl Drop for IdeService {
  fn drop(&mut self) {
    self.shutdown();
  }
}
