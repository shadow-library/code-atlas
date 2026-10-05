use std::time::{Duration, Instant};

use atlas_lsp::{RestartReason, ServerKind, UnitScope};
use serde::Serialize;

use crate::error::IdeResult;
use crate::servers::{IdeSettings, UnitMode};
use crate::service::{IdeService, TypeScriptReport, typescript_report, unit_scope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UnitCheckState {
  Ready {
    ready_ms: u64,
    server_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    typescript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_error: Option<String>,
  },
  Failed {
    reason: String,
  },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitCheck {
  pub server: String,
  pub scope: String,
  #[serde(flatten)]
  pub state: UnitCheckState,
}

impl IdeService {
  pub fn check_servers(&self) -> IdeResult<Vec<UnitCheck>> {
    let settings = self.ensure_enabled()?;
    self.query.fresh()?;
    let units = self.query.engine().language_units()?;
    let mut checks = Vec::new();
    for (kind, repos) in [
      (ServerKind::RustAnalyzer, units.rust.map(|unit| unit.folders)),
      (ServerKind::TypeScript, units.typescript.map(|unit| unit.folders)),
    ] {
      let Some(folders) = repos else { continue };
      let scopes: Vec<UnitScope> = match settings.unit {
        UnitMode::Project => vec![UnitScope::Project],
        UnitMode::Repo => folders.iter().map(|folder| unit_scope(&settings, &folder.repo)).collect(),
      };
      for scope in scopes {
        let state = match self.check_unit(kind, &scope, &settings) {
          Ok(state) => state,
          Err(error) => UnitCheckState::Failed { reason: error.to_string() },
        };
        checks.push(UnitCheck {
          server: kind.name().to_owned(),
          scope: match &scope {
            UnitScope::Project => "project".to_owned(),
            UnitScope::Repo(repo) => format!("repo {repo}"),
          },
          state,
        });
      }
    }
    Ok(checks)
  }

  fn check_unit(&self, kind: ServerKind, scope: &UnitScope, settings: &IdeSettings) -> IdeResult<UnitCheckState> {
    let (mut plan, launch, _) = self.plan(kind, scope, settings)?;
    let timeout = Duration::from_secs(settings.check_timeout_secs);
    plan.unit.timeouts.ready = timeout;
    let pool = self.pool();
    let started = Instant::now();
    let lease = pool.get(&plan.unit, None)?;
    let ready = lease.wait_ready(timeout);
    let ready_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let server_version = lease.server_info().and_then(|info| info.version.clone()).unwrap_or_else(|| launch.server_version.clone());
    let typescript = lease
      .typescript_version()
      .map(|announced| announced.version)
      .filter(|version| !version.is_empty())
      .or_else(|| launch.tsserver.as_ref().map(|tsserver| tsserver.version.clone()));
    let health_error = lease.health_error();
    let unsupported = match typescript_report(&lease, &launch) {
      TypeScriptReport::Unsupported(version) => Some(version),
      _ => None,
    };
    pool.retire(
      &plan.unit,
      lease,
      RestartReason::Refused {
        reason: "lsp check finished".to_owned(),
      },
    );
    ready?;
    if let Some(version) = unsupported {
      return Ok(UnitCheckState::Failed {
        reason: format!("the server loaded TypeScript {version}; TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one"),
      });
    }
    Ok(UnitCheckState::Ready {
      ready_ms,
      server_version,
      typescript,
      health_error,
    })
  }
}
