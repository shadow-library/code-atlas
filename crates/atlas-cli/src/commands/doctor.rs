use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use atlas_project::{Project, ProjectError, Registry, UserConfig};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::cli::DoctorArgs;
use crate::commands::connect::{SERVER_NAME, claude_config_path};
use crate::commands::ide::{check_outcome, check_servers, resolved_text, unit_detail};
use crate::commands::lsp_probe::{self, Resolved};
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
  lines: Vec<String>,
}

impl Check {
  fn new(level: Level, name: impl Into<String>, detail: impl Into<String>) -> Self {
    Self {
      level,
      name: name.into(),
      detail: detail.into(),
      lines: Vec::new(),
    }
  }

  fn with_lines(mut self, lines: Vec<String>) -> Self {
    self.lines = lines;
    self
  }
}

pub fn run(ctx: &Context, args: DoctorArgs) -> CliResult<Outcome> {
  let mut checks = vec![Check::new(
    Level::Ok,
    "home",
    format!("config {}, data {}", ctx.home.config_dir().display(), ctx.home.data_dir().display()),
  )];
  checks.push(user_config(ctx));
  let mut focus = None;
  match Registry::load(&ctx.home) {
    Err(error) => checks.push(Check::new(Level::Fail, "registry", error.to_string())),
    Ok(registry) => {
      focus = registry.resolve_project(&ctx.cwd, ctx.project.as_deref()).ok().cloned();
      checks.push(Check::new(
        Level::Ok,
        "registry",
        format!("{} projects in {}", registry.list().len(), ctx.home.registry_path().display()),
      ));
      checks.push(cwd_project(ctx, &registry));
      checks.extend(config_warnings(&registry, focus.as_ref()));
      for project in registry.list() {
        checks.extend(project_checks(ctx, project));
      }
    }
  }
  checks.extend(claude_user_scope());
  checks.push(sqlite_build());
  checks.extend(language_servers(focus.as_ref()));
  if args.lsp {
    checks.extend(live_servers(ctx, focus.as_ref()));
  }
  checks.push(tool("npx", "npx", &["--version"], "needed for TypeScript SCIP indexing"));
  checks.push(tool("claude", "claude", &["--version"], "needed to connect Claude Code and for LLM enrichment through it"));
  for check in &checks {
    let label = match check.level {
      Level::Ok => "ok  ",
      Level::Warn => "warn",
      Level::Fail => "FAIL",
    };
    emit_line(format!("{label}  {}: {}", check.name, check.detail))?;
    for line in &check.lines {
      emit_line(format!("      {line}"))?;
    }
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

fn user_config(ctx: &Context) -> Check {
  match UserConfig::load(&ctx.home) {
    Ok(config) if config.exists() => Check::new(Level::Ok, "config", format!("{} loaded", config.path().display())),
    Ok(config) => Check::new(Level::Ok, "config", format!("{} absent; built-in defaults apply", config.path().display())),
    Err(error) => Check::new(Level::Fail, "config", error.to_string()),
  }
}

fn cwd_project(ctx: &Context, registry: &Registry) -> Check {
  let trace = registry.trace(&ctx.cwd, None);
  let check = match &trace.outcome {
    Ok(resolution) => Check::new(Level::Ok, "working directory", format!("belongs to project {}", resolution.project.name)),
    Err(ProjectError::NotInProject { .. }) => Check::new(Level::Ok, "working directory", "not inside any project"),
    Err(error) => Check::new(Level::Warn, "working directory", error.to_string()),
  };
  check.with_lines(trace.steps)
}

fn config_warnings(registry: &Registry, focus: Option<&Project>) -> Option<Check> {
  let warnings = registry.warnings(focus.map(|project| project.name.as_str()));
  let count = warnings.len();
  (count > 0).then(|| Check::new(Level::Warn, "configuration", format!("{count} ignored values; see `catl config show --origin`")).with_lines(warnings))
}

fn claude_user_scope() -> Option<Check> {
  let path = claude_config_path()?;
  let text = fs::read_to_string(&path).ok()?;
  let name = "claude user scope";
  let entry = match serde_json::from_str::<Value>(&text) {
    Ok(document) => document.get("mcpServers")?.get(SERVER_NAME)?.clone(),
    Err(error) => return Some(Check::new(Level::Ok, name, format!("cannot read {}: {error}", path.display()))),
  };
  let arguments: Vec<&str> = entry.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
  let pinned = arguments.iter().position(|argument| *argument == "--project").and_then(|index| arguments.get(index + 1));
  Some(match pinned {
    Some(project) => Check::new(
      Level::Warn,
      name,
      format!(
        "the user-scope {SERVER_NAME} server in {} pins --project {project}, so every repo without its own .mcp.json talks to {project}; rerun `catl connect claude --scope user`",
        path.display()
      ),
    ),
    None => Check::new(Level::Ok, name, format!("{SERVER_NAME} resolves the project from the session's working directory")),
  })
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

fn language_servers(project: Option<&Project>) -> Vec<Check> {
  let mut checks = Vec::new();
  let defaults = atlas_project::LspSettings::default();
  let settings = project.map_or(&defaults, |project| &project.settings.lsp);
  let scope = project.map_or_else(|| "no project in scope; default settings".to_owned(), |project| format!("project {}", project.name));
  if !settings.enabled {
    checks.push(Check::new(
      Level::Warn,
      "language servers",
      format!("disabled for {scope} (lsp.enabled = false); exact operations refuse until `catl project config <project> --lsp on`"),
    ));
  }
  match lsp_probe::probe_rust_analyzer().result {
    Resolved::Found { summary } => checks.push(Check::new(
      Level::Ok,
      "rust-analyzer",
      format!("{summary}; used for exact Rust operations and Rust SCIP indexing"),
    )),
    Resolved::Unavailable { reason } => checks.push(Check::new(
      Level::Warn,
      "rust-analyzer",
      format!("{reason}; exact Rust operations and Rust SCIP indexing are unavailable (`rustup component add rust-analyzer`)"),
    )),
  }
  let roots = project.map(lsp_probe::typescript_roots).unwrap_or_default();
  let probe = lsp_probe::probe_typescript(settings, &roots, false);
  let lines: Vec<String> = probe
    .steps
    .iter()
    .map(|step| match settings.download && step.label.starts_with("download (") {
      true => format!(
        "{}: skipped by doctor (no network); lsp.download = on; `catl lsp check` probes the npm registry",
        step.label
      ),
      false => step.text(),
    })
    .collect();
  checks.push(
    match probe.result {
      Resolved::Found { summary } => Check::new(Level::Ok, "typescript-language-server", summary),
      Resolved::Unavailable { .. } if settings.download => Check::new(
        Level::Ok,
        "typescript-language-server",
        "not installed; it is fetched through npx on first use (lsp.download is on)",
      ),
      Resolved::Unavailable { reason } => Check::new(Level::Warn, "typescript-language-server", reason),
    }
    .with_lines(lines),
  );
  for project in project.into_iter() {
    for (repo, version) in lsp_probe::typescript_seven_repos(project) {
      checks.push(Check::new(
        Level::Warn,
        format!("{}/{repo} typescript", project.name),
        format!(
          "node_modules holds TypeScript {version}, which has no tsserver; exact TypeScript operations need typescript@5 beside the server or the one bundled with it, or they fail as unavailable"
        ),
      ));
    }
  }
  checks
}

fn live_servers(ctx: &Context, project: Option<&Project>) -> Vec<Check> {
  if project.is_none() {
    return vec![Check::new(Level::Warn, "language server health", "no project in scope; run from a repo or pass --project")];
  }
  match check_servers(ctx) {
    Err(error) => vec![Check::new(Level::Warn, "language server health", error.to_string())],
    Ok((_, report)) => {
      let mut checks = Vec::new();
      for unit in &report.units {
        let (detail, notes) = unit_detail(unit, &project.map(|project| project.settings.lsp.exclude.clone()).unwrap_or_default());
        let level = match check_outcome(std::slice::from_ref(unit)) {
          Outcome::Complete => Level::Ok,
          Outcome::Failed => Level::Fail,
          _ => Level::Warn,
        };
        checks.push(Check::new(level, format!("{} ({})", unit.server, unit.scope), detail).with_lines(notes));
      }
      if report.units.is_empty() {
        checks.push(Check::new(
          Level::Ok,
          "language server health",
          format!("no Rust or TypeScript files indexed; typescript {}", resolved_text(&report.typescript.result)),
        ));
      }
      checks
    }
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
