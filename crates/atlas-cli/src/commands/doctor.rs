use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use atlas_core::Availability;
use atlas_project::Registry;
use atlas_scip::{RustAnalyzerScip, ScipIndexer};
use rusqlite::{Connection, OpenFlags};

use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line};

const INTEGRITY_ROWS_SHOWN: usize = 3;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
  Ok,
  Warn,
  Fail,
}

struct Check {
  level: Level,
  name: String,
  detail: String,
}

impl Check {
  fn new(level: Level, name: impl Into<String>, detail: impl Into<String>) -> Self {
    Self {
      level,
      name: name.into(),
      detail: detail.into(),
    }
  }
}

pub fn run(ctx: &Context) -> CliResult<Outcome> {
  let mut checks = vec![Check::new(
    Level::Ok,
    "home",
    format!("config {}, data {}", ctx.home.config_dir().display(), ctx.home.data_dir().display()),
  )];
  match Registry::load(&ctx.home) {
    Err(error) => checks.push(Check::new(Level::Fail, "registry", error.to_string())),
    Ok(registry) => {
      checks.push(Check::new(
        Level::Ok,
        "registry",
        format!("{} projects in {}", registry.list().len(), ctx.home.registry_path().display()),
      ));
      checks.push(cwd_project(ctx, &registry));
      for project in registry.list() {
        checks.extend(project_checks(ctx, project));
      }
    }
  }
  checks.push(sqlite_build());
  checks.push(rust_analyzer());
  checks.push(tool("npx", "npx", &["--version"], "needed for TypeScript SCIP indexing"));
  checks.push(tool("claude", "claude", &["--version"], "needed to connect Claude Code and for LLM enrichment through it"));
  for check in &checks {
    let label = match check.level {
      Level::Ok => "ok  ",
      Level::Warn => "warn",
      Level::Fail => "FAIL",
    };
    emit_line(format!("{label}  {}: {}", check.name, check.detail))?;
  }
  let failures = checks.iter().filter(|check| check.level == Level::Fail).count();
  let warnings = checks.iter().filter(|check| check.level == Level::Warn).count();
  emit_line(format!("{failures} failures, {warnings} warnings"))?;
  Ok(match (failures, warnings) {
    (0, 0) => Outcome::Complete,
    (0, _) => Outcome::Degraded,
    _ => Outcome::Failed,
  })
}

fn cwd_project(ctx: &Context, registry: &Registry) -> Check {
  match registry.resolve_project(&ctx.cwd, None) {
    Ok(project) => Check::new(Level::Ok, "working directory", format!("belongs to project {}", project.name)),
    Err(atlas_project::ProjectError::NotInProject { .. }) => Check::new(Level::Ok, "working directory", "not inside any project"),
    Err(error) => Check::new(Level::Warn, "working directory", error.to_string()),
  }
}

fn project_checks(ctx: &Context, project: &atlas_project::Project) -> Vec<Check> {
  let mut checks = Vec::new();
  for repo in &project.repos {
    let name = format!("{}/{}", project.name, repo.name);
    match repo.path.is_dir() {
      true => checks.push(Check::new(Level::Ok, name, repo.path.display().to_string())),
      false => checks.push(Check::new(Level::Warn, name, format!("{} is missing", repo.path.display()))),
    }
  }
  let db = ctx.home.db_path(&project.name);
  let name = format!("{} database", project.name);
  if !db.exists() {
    checks.push(Check::new(Level::Ok, name, "not indexed yet"));
    return checks;
  }
  checks.push(match integrity(&db) {
    Ok(()) => Check::new(Level::Ok, name, "integrity_check passed"),
    Err(reason) => Check::new(Level::Fail, name, reason),
  });
  checks.push(lock_state(project.name.as_str(), &db));
  checks
}

fn integrity(db: &Path) -> Result<(), String> {
  let connection =
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|error| format!("cannot open {}: {error}", db.display()))?;
  connection.busy_timeout(BUSY_TIMEOUT).map_err(|error| error.to_string())?;
  let mut statement = connection.prepare("PRAGMA integrity_check").map_err(|error| error.to_string())?;
  let rows = statement
    .query_map([], |row| row.get::<_, String>(0))
    .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    .map_err(|error| format!("integrity_check failed: {error}"))?;
  if rows == ["ok"] {
    return Ok(());
  }
  let shown: Vec<&str> = rows.iter().take(INTEGRITY_ROWS_SHOWN).map(String::as_str).collect();
  Err(format!("integrity_check reported {} problems: {}", rows.len(), shown.join("; ")))
}

fn lock_state(project: &str, db: &Path) -> Check {
  let mut lock_path = db.as_os_str().to_owned();
  lock_path.push(".index.lock");
  let name = format!("{project} index lock");
  let Ok(file) = File::open(&lock_path) else {
    return Check::new(Level::Ok, name, "no lock file; nothing has indexed yet");
  };
  match file.try_lock_shared() {
    Ok(()) => Check::new(Level::Ok, name, "free"),
    Err(fs::TryLockError::WouldBlock) => Check::new(Level::Ok, name, "held; an index run is in progress"),
    Err(fs::TryLockError::Error(error)) => Check::new(Level::Warn, name, format!("cannot inspect: {error}")),
  }
}

fn sqlite_build() -> Check {
  let options = Connection::open_in_memory().and_then(|connection| {
    let mut statement = connection.prepare("PRAGMA compile_options")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>()
  });
  match options {
    Err(error) => Check::new(Level::Warn, "sqlite build", format!("cannot read compile options: {error}")),
    Ok(options) => sqlite_build_check(&options),
  }
}

fn sqlite_build_check(options: &[String]) -> Check {
  match options.iter().any(|option| option == "ENABLE_MEMORY_MANAGEMENT") {
    true => Check::new(
      Level::Warn,
      "sqlite build",
      "compiled with ENABLE_MEMORY_MANAGEMENT: one process-wide page-cache mutex serialises concurrent MCP queries; build from a checkout (cargo install --path) or set LIBSQLITE3_FLAGS",
    ),
    false => Check::new(Level::Ok, "sqlite build", "per-connection page caches; concurrent queries scale"),
  }
}

fn rust_analyzer() -> Check {
  match RustAnalyzerScip::new().availability() {
    Availability::Available => Check::new(Level::Ok, "rust-analyzer", "available for Rust SCIP indexing"),
    Availability::Unavailable { reason } => Check::new(Level::Warn, "rust-analyzer", format!("{reason}; Rust edges stay at resolver certainty")),
  }
}

fn tool(name: &str, program: &str, args: &[&str], purpose: &str) -> Check {
  let output = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output();
  match output {
    Ok(output) if output.status.success() => {
      let version = String::from_utf8_lossy(&output.stdout);
      Check::new(Level::Ok, name, version.lines().next().unwrap_or("available").trim().to_owned())
    }
    Ok(output) => Check::new(Level::Warn, name, format!("`{program} {}` exited with {}; {purpose}", args.join(" "), output.status)),
    Err(_) => Check::new(Level::Warn, name, format!("not found on PATH; {purpose}")),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_warn_when_sqlite_shares_one_page_cache_across_connections() {
    let shared = sqlite_build_check(&["THREADSAFE=1".to_owned(), "ENABLE_MEMORY_MANAGEMENT".to_owned()]);
    let separate = sqlite_build_check(&["THREADSAFE=1".to_owned(), "DEFAULT_MEMSTATUS=0".to_owned()]);
    assert_eq!(shared.level, Level::Warn);
    assert_eq!(separate.level, Level::Ok);
  }

  #[test]
  fn should_report_the_workspace_build_without_the_shared_page_cache() {
    assert_eq!(sqlite_build().level, Level::Ok, "{}", sqlite_build().detail);
  }
}
