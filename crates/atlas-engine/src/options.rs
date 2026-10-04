use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atlas_core::{RelPath, RepoName};
use atlas_detect::DiagnosingDetector;
use atlas_semantic::TaskProviders;
use atlas_similarity::SimilarityConfig;
use atlas_store::StoreOptions;

use crate::passes::ProjectPass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMode {
  Incremental,
  Full,
}

type FaultPredicate = dyn Fn(&RepoName, &RelPath) -> bool + Send + Sync;

#[derive(Clone, Default)]
pub struct FaultInjection {
  panic_on: Option<Arc<FaultPredicate>>,
  crash_after_persist: Option<Arc<AtomicBool>>,
  before_index_lock: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl FaultInjection {
  pub fn panic_when(predicate: impl Fn(&RepoName, &RelPath) -> bool + Send + Sync + 'static) -> Self {
    Self {
      panic_on: Some(Arc::new(predicate)),
      ..Self::default()
    }
  }

  pub fn crash_after_persist(armed: Arc<AtomicBool>) -> Self {
    Self {
      crash_after_persist: Some(armed),
      ..Self::default()
    }
  }

  pub fn before_index_lock(hook: impl Fn() + Send + Sync + 'static) -> Self {
    Self {
      before_index_lock: Some(Arc::new(hook)),
      ..Self::default()
    }
  }

  pub(crate) fn pause_before_index_lock(&self) {
    if let Some(hook) = &self.before_index_lock {
      hook();
    }
  }

  pub(crate) fn should_panic(&self, repo: &RepoName, path: &RelPath) -> bool {
    self.panic_on.as_ref().is_some_and(|predicate| predicate(repo, path))
  }

  pub(crate) fn crash_point(&self) {
    if self.crash_after_persist.as_ref().is_some_and(|armed| armed.load(Ordering::SeqCst)) {
      panic!("injected crash after file owners were committed");
    }
  }
}

impl fmt::Debug for FaultInjection {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("FaultInjection")
      .field("panic_on", &self.panic_on.is_some())
      .field("crash_after_persist", &self.crash_after_persist.is_some())
      .field("before_index_lock", &self.before_index_lock.is_some())
      .finish()
  }
}

#[derive(Clone)]
pub struct EngineOptions {
  pub store: StoreOptions,
  pub batch_max_owners: usize,
  pub batch_max_duration: Duration,
  pub full_scan_interval: Duration,
  pub watch_debounce: Duration,
  pub similarity: SimilarityConfig,
  pub passes: Vec<Arc<dyn ProjectPass>>,
  pub llm_providers: Option<Arc<TaskProviders>>,
  pub similarity_persist_idle: Duration,
  #[doc(hidden)]
  pub extra_detectors: Vec<Arc<dyn DiagnosingDetector>>,
  #[doc(hidden)]
  pub fault_injection: FaultInjection,
}

impl Default for EngineOptions {
  fn default() -> Self {
    Self {
      store: StoreOptions::default(),
      batch_max_owners: 200,
      batch_max_duration: Duration::from_millis(50),
      full_scan_interval: Duration::from_secs(60),
      watch_debounce: Duration::from_millis(200),
      similarity: SimilarityConfig::default(),
      passes: Vec::new(),
      llm_providers: None,
      similarity_persist_idle: Duration::from_secs(30),
      extra_detectors: Vec::new(),
      fault_injection: FaultInjection::default(),
    }
  }
}

impl fmt::Debug for EngineOptions {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("EngineOptions")
      .field("store", &self.store)
      .field("batch_max_owners", &self.batch_max_owners)
      .field("batch_max_duration", &self.batch_max_duration)
      .field("full_scan_interval", &self.full_scan_interval)
      .field("watch_debounce", &self.watch_debounce)
      .field("similarity", &self.similarity)
      .field("passes", &self.passes.iter().map(|pass| pass.name()).collect::<Vec<_>>())
      .field("llm_providers", &self.llm_providers.is_some())
      .field("similarity_persist_idle", &self.similarity_persist_idle)
      .field("extra_detectors", &self.extra_detectors.len())
      .field("fault_injection", &self.fault_injection)
      .finish()
  }
}
