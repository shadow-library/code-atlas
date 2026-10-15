use std::path::{Path, PathBuf};
use std::time::Instant;

use atlas_core::RepoName;
use atlas_project::{
  AtlasHome, Binding, OverlayDerivation, OverlayError, OverlayLease, OverlayPlan, Overlays, Project, ProjectError, Registry, Resolution, SeedSource, SeedSummary, WorktreeMode,
  now_ms,
};
use atlas_store::{Store, StoreOptions, backup_database};

use crate::error::{EngineError, EngineResult};
use crate::state::{EngineMeta, META_BLOB, META_BLOB_VERSION};

const SEED_SPACE_FACTOR: u64 = 2;

#[derive(Debug)]
pub struct BoundProject {
  pub project: Project,
  pub binding: Binding,
  pub engine_home: AtlasHome,
  pub warnings: Vec<String>,
  pub overlay: Option<OverlayLease>,
  pub base_db: Option<PathBuf>,
}

impl BoundProject {
  fn on_registered(resolution: Resolution, home: &AtlasHome) -> Self {
    Self {
      project: resolution.project,
      binding: resolution.binding,
      engine_home: home.clone(),
      warnings: resolution.warnings,
      overlay: None,
      base_db: None,
    }
  }
}

pub fn bind_project(home: &AtlasHome, registry: &Registry, cwd: &Path, explicit: Option<&str>, store: &StoreOptions) -> Result<BoundProject, ProjectError> {
  let resolution = registry.resolve(cwd, explicit)?;
  let mode = registry.user_config().worktrees().mode;
  let worktree = match &resolution.binding {
    Binding::Direct | Binding::Overlay(_) => {
      collect_stale_overlays(home, registry);
      return Ok(BoundProject::on_registered(resolution, home));
    }
    Binding::UnresolvedWorktree(unresolved) => (unresolved.root.clone(), None),
    Binding::Worktree(worktree) => (worktree.worktree.clone(), Some(worktree.clone())),
  };
  match (mode, worktree) {
    (WorktreeMode::Off, (root, _)) => Err(ProjectError::WorktreeIgnored {
      cwd: cwd.to_path_buf(),
      worktree: root,
      project: resolution.project.name.to_string(),
    }),
    (WorktreeMode::Main, _) | (WorktreeMode::Overlay, (_, None)) => Ok(BoundProject::on_registered(resolution, home)),
    (WorktreeMode::Overlay, (_, Some(worktree))) => Ok(match registry.derive_overlay(&worktree) {
      Ok(OverlayDerivation::Eligible(plan)) => match prepare_overlay(home, registry, &plan, store) {
        Ok(lease) => on_overlay(&plan, resolution.warnings, lease, home.db_path(&plan.base)),
        Err(error) => fall_back(resolution, home, format!("could not prepare the worktree overlay {}: {error}", plan.label)),
      },
      Ok(OverlayDerivation::Ineligible { reason }) => fall_back(resolution, home, format!("no worktree overlay: {reason}")),
      Err(error) => fall_back(resolution, home, format!("no worktree overlay: {error}")),
    }),
  }
}

pub fn current_overlay(registry: &Registry, cwd: &Path, explicit: Option<&str>) -> Result<Option<OverlayPlan>, ProjectError> {
  let Binding::Worktree(worktree) = registry.resolve(cwd, explicit)?.binding else {
    return Ok(None);
  };
  match registry.derive_overlay(&worktree)? {
    OverlayDerivation::Eligible(plan) => Ok(Some(*plan)),
    OverlayDerivation::Ineligible { .. } => Ok(None),
  }
}

fn on_overlay(plan: &OverlayPlan, mut warnings: Vec<String>, lease: OverlayLease, base_db: PathBuf) -> BoundProject {
  warnings.extend(plan.warnings.iter().cloned());
  BoundProject {
    project: plan.project.clone(),
    binding: plan.binding(),
    engine_home: lease.home.clone(),
    warnings,
    overlay: Some(lease),
    base_db: Some(base_db),
  }
}

fn fall_back(resolution: Resolution, home: &AtlasHome, warning: String) -> BoundProject {
  tracing::debug!(warning = %warning, "answering from the indexed checkout; edits are refused");
  let mut bound = BoundProject::on_registered(resolution, home);
  bound.warnings.push(format!("{warning}; answering from the indexed checkout, and edits are refused here"));
  bound
}

pub fn prepare_overlay(home: &AtlasHome, registry: &Registry, plan: &OverlayPlan, store: &StoreOptions) -> Result<OverlayLease, OverlayError> {
  let overlays = Overlays::new(home);
  let base_db = home.db_path(&plan.base);
  let lease = overlays.acquire(plan, store.lock_timeout, |staging| seed_overlay(&base_db, staging, &plan.repos, store))?;
  if let Some(seed) = &lease.seeded {
    tracing::info!(overlay = %plan.label, worktree = %plan.worktree.display(), source = ?seed.source, bytes = seed.bytes, millis = seed.millis, "seeded a worktree overlay");
  }
  collect_stale_overlays(home, registry);
  Ok(lease)
}

fn collect_stale_overlays(home: &AtlasHome, registry: &Registry) {
  let settings = registry.user_config().worktrees();
  match Overlays::new(home).collect_garbage_if_due(&settings, |name| registry.project(name.as_str()).is_ok(), now_ms()) {
    Ok(collected) => {
      for removed in collected {
        tracing::info!(overlay = %removed.record.label, reason = %removed.reason.describe(), "collected a worktree overlay");
      }
    }
    Err(error) => tracing::warn!(error = %error, "could not collect stale worktree overlays"),
  }
}

pub fn seed_overlay(base_db: &Path, staging: &Path, remapped: &[RepoName], options: &StoreOptions) -> EngineResult<SeedSummary> {
  seed_overlay_with(base_db, staging, remapped, options, free_space)
}

pub fn seed_overlay_with(
  base_db: &Path,
  staging: &Path,
  remapped: &[RepoName],
  options: &StoreOptions,
  free_space: impl Fn(&Path) -> std::io::Result<u64>,
) -> EngineResult<SeedSummary> {
  let started = Instant::now();
  let source = match base_db.exists() {
    true => {
      ensure_room_for_copy(base_db, staging, free_space)?;
      backup_database(base_db, staging, options.lock_timeout)?;
      SeedSource::Base
    }
    false => SeedSource::Empty,
  };
  let store = Store::open_with(staging, StoreOptions { readers: 1, ..options.clone() })?;
  forget_scan_times(&store, remapped)?;
  drop(store);
  let bytes = std::fs::metadata(staging).map_err(|source| EngineError::io(staging, source))?.len();
  Ok(SeedSummary {
    source,
    bytes,
    millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
  })
}

fn ensure_room_for_copy(base_db: &Path, staging: &Path, free_space: impl Fn(&Path) -> std::io::Result<u64>) -> EngineResult<()> {
  let size = |path: &Path| std::fs::metadata(path).map_or(0, |metadata| metadata.len());
  let mut wal = base_db.as_os_str().to_owned();
  wal.push("-wal");
  let needed = (size(base_db) + size(Path::new(&wal))).saturating_mul(SEED_SPACE_FACTOR);
  let directory = staging.parent().unwrap_or(Path::new("."));
  let available = free_space(directory).map_err(|source| EngineError::io(directory, source))?;
  match available >= needed {
    true => Ok(()),
    false => Err(EngineError::InsufficientSpace {
      path: directory.to_path_buf(),
      needed,
      available,
    }),
  }
}

#[cfg(unix)]
fn free_space(directory: &Path) -> std::io::Result<u64> {
  let stats = rustix::fs::statvfs(directory)?;
  Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(not(unix))]
fn free_space(_: &Path) -> std::io::Result<u64> {
  Ok(u64::MAX)
}

// Without a scan time every stamp counts as racy, so the first run re-hashes each file of a remapped repo instead of trusting the base checkout's stat.
fn forget_scan_times(store: &Store, remapped: &[RepoName]) -> EngineResult<()> {
  let Some(mut meta) = store.read(EngineMeta::read)? else { return Ok(()) };
  for repo in remapped {
    if let Some(repo_meta) = meta.repos.get_mut(repo.as_str()) {
      repo_meta.scan_started_ns = None;
    }
  }
  let bytes = serde_json::to_vec(&meta).map_err(|error| EngineError::codec("engine meta", error))?;
  store.write(|tx| -> EngineResult<()> {
    tx.put_blob(META_BLOB, META_BLOB_VERSION, &bytes)?;
    Ok(())
  })
}
