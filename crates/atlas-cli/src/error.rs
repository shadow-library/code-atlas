use std::io::{self, Write};
use std::path::PathBuf;

use atlas_core::CoreError;
use atlas_edit::EditError;
use atlas_engine::EngineError;
use atlas_ide::IdeError;
use atlas_mcp::McpError;
use atlas_project::ProjectError;
use atlas_query::QueryError;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
  Success,
  Failure,
  Usage,
  Degraded,
  NoMatch,
  Refused,
  RolledBack,
  PartiallyApplied,
}

impl Exit {
  pub fn code(self) -> u8 {
    match self {
      Self::Success => 0,
      Self::Failure => 1,
      Self::Usage => 2,
      Self::Degraded => 3,
      Self::NoMatch => 4,
      Self::Refused => 5,
      Self::RolledBack => 6,
      Self::PartiallyApplied => 7,
    }
  }
}

#[derive(Debug, Error)]
pub enum CliError {
  #[error(transparent)]
  Project(#[from] ProjectError),
  #[error(transparent)]
  Engine(#[from] EngineError),
  #[error(transparent)]
  Query(#[from] QueryError),
  #[error(transparent)]
  Core(#[from] CoreError),
  #[error("{operation} {}: {source}", .path.display())]
  Io {
    operation: &'static str,
    path: PathBuf,
    #[source]
    source: io::Error,
  },
  #[error("cannot encode output as JSON: {0}")]
  Encode(#[from] serde_json::Error),
  #[error("invalid log filter {filter:?}: {message}")]
  InvalidLogFilter { filter: String, message: String },
  #[error("invalid --repo value {0:?}")]
  InvalidRepoSpec(String),
  #[error("invalid --snippet value {0:?}; expected FILE:START-END")]
  InvalidSnippet(String),
  #[error("invalid target {0:?}; expected REPO:PATH")]
  InvalidFileTarget(String),
  #[error("invalid wiring manifest {}: {message}", .path.display())]
  InvalidManifest { path: PathBuf, message: String },
  #[error("refusing to change {}; nothing was written", .targets.join(", "))]
  WiringRefused { targets: Vec<String> },
  #[error("{target}: {detail}")]
  WiringNotVisible { target: String, detail: String },
  #[error(
    "refusing to change agent configuration from a Cargo build or test (CARGO_MANIFEST_DIR is set) while {missing} is unset, because it would change the real user configuration"
  )]
  UnsandboxedAgentWiring { missing: String },
  #[error("{} pins project {existing:?}, not {requested:?}", .path.display())]
  MarkerConflict { path: PathBuf, existing: String, requested: String },
  #[error("cannot derive a project name from {}", .0.display())]
  ProjectNameUnderivable(PathBuf),
  #[error("{what} needs confirmation, and stdin is not a terminal")]
  ConfirmationRequired { what: String },
  #[error("{0}")]
  InvalidSetting(String),
  #[error(transparent)]
  Ide(#[from] IdeError),
  #[error(transparent)]
  EditRefused(Box<CliError>),
  #[error("{0}")]
  EditOutsideIndexedCheckout(String),
  #[error("invalid plan file {}: {message}", .path.display())]
  InvalidPlan { path: PathBuf, message: String },
  #[error("{0}")]
  InvalidInput(String),
  #[error("the `claude` CLI is not available: {reason}")]
  ClaudeUnavailable { reason: String, command: String },
  #[error("aborted")]
  Declined,
  #[error(transparent)]
  Mcp(McpError),
  #[error("cannot install the signal handler: {0}")]
  Signal(#[source] io::Error),
  #[error("run {0} does not exist")]
  RunNotFound(i64),
  #[error("{what} is not indexed")]
  FileNotIndexed { what: String },
  #[error("refusing to delete {}: it is outside the data directory", .0.display())]
  UnsafePurge(PathBuf),
}

impl CliError {
  pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
    Self::Io {
      operation,
      path: path.into(),
      source,
    }
  }

  pub fn for_edit(self) -> Self {
    let unmatched = match &self {
      Self::Ide(IdeError::FileNotFound { .. } | IdeError::EndpointTarget { .. } | IdeError::NotASymbol { .. }) => true,
      Self::Ide(IdeError::Edit(EditError::Io { source, .. })) => source.kind() == io::ErrorKind::NotFound,
      _ => false,
    };
    match unmatched {
      true => Self::EditRefused(Box::new(self)),
      false => self,
    }
  }

  pub fn exit(&self) -> Exit {
    match self {
      Self::Project(error) => project_exit(error),
      Self::Engine(EngineError::Project(error)) => project_exit(error),
      Self::Query(QueryError::Invalid(_)) | Self::Core(_) => Exit::Usage,
      Self::InvalidLogFilter { .. }
      | Self::InvalidRepoSpec(_)
      | Self::InvalidSnippet(_)
      | Self::InvalidFileTarget(_)
      | Self::WiringRefused { .. }
      | Self::UnsandboxedAgentWiring { .. }
      | Self::MarkerConflict { .. }
      | Self::ProjectNameUnderivable(_)
      | Self::ConfirmationRequired { .. }
      | Self::InvalidSetting(_)
      | Self::InvalidPlan { .. }
      | Self::InvalidInput(_)
      | Self::ClaudeUnavailable { .. } => Exit::Usage,
      Self::Ide(error) => ide_exit(error),
      Self::EditRefused(_) | Self::EditOutsideIndexedCheckout(_) => Exit::Refused,
      Self::RunNotFound(_) | Self::FileNotIndexed { .. } => Exit::NoMatch,
      _ => Exit::Failure,
    }
  }

  fn hint(&self) -> Option<&'static str> {
    match self {
      Self::Project(ProjectError::ProjectNotFound(_)) => Some("list registered projects with `catl project list`"),
      Self::Project(ProjectError::Ambiguous { .. }) => Some("pass --project <name>, or run `catl init --project <name>` in the repo to pin one"),
      Self::Project(ProjectError::NotInProject { .. }) => Some("run `catl init` in the repo, or `catl project create <name> --repo <path>`, or pass --project <name>"),
      Self::Project(ProjectError::InvalidGlob { .. }) => {
        Some("globs use gitignore syntax, e.g. 'src/**', 'legacy/' or '*.generated.ts'; quote them so the shell does not expand them")
      }
      Self::Project(ProjectError::RepoNotFound { .. }) => Some("list a project's repos with `catl project show <name>`"),
      Self::Engine(EngineError::IndexLockTimeout { .. }) => Some("another index run holds the lock; retry when it finishes"),
      Self::Engine(EngineError::ProjectConfigChanged) => Some("another process changed the project; retry the command"),
      Self::ConfirmationRequired { .. } => Some("pass --yes to proceed without a prompt"),
      Self::ClaudeUnavailable { .. } => None,
      Self::Mcp(_) => Some("the MCP client must speak JSON-RPC over this process's stdin and stdout"),
      Self::RunNotFound(_) => Some("list recent runs with `catl status`"),
      Self::FileNotIndexed { .. } => Some("check the repo name with `catl project show` and the path relative to the repo root"),
      Self::Ide(error) => ide_hint(error),
      Self::EditRefused(error) => error.hint(),
      Self::InvalidPlan { .. } => Some("make a plan with `catl edit <operation> --dry-run --plan-out plan.json`"),
      Self::UnsandboxedAgentWiring { .. } => Some("point CLAUDE_CONFIG_DIR and CODEX_HOME at a temporary directory"),
      Self::WiringRefused { .. } => Some("each refused entry above says why and which flag overrides it; `catl agents status` lists drift"),
      Self::InvalidManifest { .. } => Some("fix or move the file; `catl agents status` and `unsync` rely on it to know what they wrote"),
      _ => None,
    }
  }

  pub fn report(&self) {
    let mut stderr = io::stderr().lock();
    let _ = match self {
      Self::Project(ProjectError::Ambiguous { cwd, candidates }) => {
        let mut text = format!("error: {} is inside several projects:\n", cwd.display());
        for candidate in candidates {
          text.push_str(&format!("  {candidate}\n"));
        }
        write!(stderr, "{text}")
      }
      other => writeln!(stderr, "error: {other}"),
    };
    if let Self::ClaudeUnavailable { command, .. } = self {
      let _ = writeln!(stderr, "hint: install Claude Code, or register the server yourself with:\n  {command}");
    }
    if let Some(hint) = self.hint() {
      let _ = writeln!(stderr, "hint: {hint}");
    }
  }
}

fn ide_exit(error: &IdeError) -> Exit {
  match error {
    IdeError::InvalidRequest(_)
    | IdeError::Regex { .. }
    | IdeError::RegexGroups { .. }
    | IdeError::RegexMatches { .. }
    | IdeError::InvalidPosition { .. }
    | IdeError::UnsupportedFile { .. }
    | IdeError::UnknownRepo { .. }
    | IdeError::AmbiguousFile { .. }
    | IdeError::OutsideProject { .. } => Exit::Usage,
    IdeError::FileNotFound { .. } | IdeError::EndpointTarget { .. } | IdeError::NotASymbol { .. } => Exit::NoMatch,
    IdeError::PlanExpired { .. } | IdeError::UnknownPlan { .. } => Exit::Refused,
    IdeError::Query(QueryError::Invalid(_)) => Exit::Usage,
    IdeError::Edit(error) => edit_exit(error),
    _ => Exit::Failure,
  }
}

fn edit_exit(error: &EditError) -> Exit {
  match error {
    EditError::EmptyPattern
    | EditError::InvalidRegex { .. }
    | EditError::UnknownCaptureGroup { .. }
    | EditError::InvalidOccurrenceId(_)
    | EditError::InvalidGlob { .. }
    | EditError::UnknownRepo(_)
    | EditError::EmptySelection
    | EditError::EmptyText => Exit::Usage,
    EditError::RolledBack { .. } => Exit::RolledBack,
    EditError::PartiallyApplied { .. } => Exit::PartiallyApplied,
    EditError::Io { .. } | EditError::LockTimeout { .. } | EditError::Project(_) => Exit::Failure,
    _ => Exit::Refused,
  }
}

fn ide_hint(error: &IdeError) -> Option<&'static str> {
  match error {
    IdeError::Unavailable { alternative: Some(_), .. } | IdeError::Disabled { alternative: Some(_), .. } | IdeError::ServerFailed { alternative: Some(_), .. } => {
      Some("pass --source graph for tagged graph results that need no language server")
    }
    IdeError::Unavailable { .. } | IdeError::ServerFailed { .. } => Some("check the language servers with `catl lsp check` and `catl doctor`"),
    _ => None,
  }
}

fn project_exit(error: &ProjectError) -> Exit {
  match error {
    ProjectError::Io { .. } | ProjectError::HomeNotFound | ProjectError::RegistrySerialize(_) => Exit::Failure,
    _ => Exit::Usage,
  }
}

impl From<McpError> for CliError {
  fn from(error: McpError) -> Self {
    match error {
      McpError::Project(error) => Self::Project(error),
      McpError::Engine(error) => Self::Engine(error),
      unknown @ McpError::UnknownTool { .. } => Self::InvalidSetting(unknown.to_string()),
      other => Self::Mcp(other),
    }
  }
}

pub type CliResult<T> = Result<T, CliError>;
