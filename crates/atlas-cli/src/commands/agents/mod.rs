mod claude;
mod codex;
mod guidance;
mod manifest;
mod plan;
mod target;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atlas_mcp::guidance::{SERVER_NAME, ToolProfile, repo_block};
use atlas_project::{AgentsSettings, Binding, ClaudeHook, ClaudeScope, ConfigEnv, GuidanceMode, HooksSource, McpFlags, ProjectError, Registry, Resolution, resolve_mcp};
use serde_json::{Value, json};

use self::claude::{HOME_VAR, HookEvent, RegistrationScope};
use self::guidance::ExcludeLocation;
use self::manifest::{Agent, Manifest, Record, Scope};
use self::plan::{Bookkeeping, Change, Desired, Flags, Owner, Redo, Step, decide_removal, decide_write};
use self::target::{Content, Current, Effect, Target};
use crate::cli::{AgentName, AgentScope, AgentsCommand, AgentsHooksArgs, AgentsStatusArgs, AgentsSyncArgs, AgentsUnsyncArgs, ConnectCommand, ConnectScope};
use crate::commands::mcp::tool_profile;
use crate::confirm::confirm;
use crate::context::Context;
use crate::diff::unified;
use crate::error::{CliError, CliResult};
use crate::output::{Outcome, emit, emit_json, emit_line, note};

pub(crate) use self::claude::claude_config_path;

const DEFAULT_COMMAND: &str = "catl";
const DEFAULT_SEED_NOTE: &str = "new: the seed hook is on by default; it adds a short Code Atlas context pack to each prompt you submit in a registered project (UserPromptSubmit). To turn it off, set [agents.claude] hooks = [\"auto-approve\"] in config.toml and rerun `catl agents sync`";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
  Sync,
  Connect,
}

struct SyncRequest {
  style: Style,
  agents: Vec<Agent>,
  scope: Option<Scope>,
  repos: Vec<PathBuf>,
  flags: Flags,
  dry_run: bool,
  yes: bool,
}

struct RepoRoot {
  root: PathBuf,
  project: String,
}

#[derive(Default)]
struct Notes(Vec<String>);

impl Notes {
  fn push(&mut self, note: String) {
    if !self.0.contains(&note) {
      self.0.push(note);
    }
  }
}

pub fn run(ctx: &Context, command: AgentsCommand) -> CliResult<Outcome> {
  match command {
    AgentsCommand::Sync(args) => sync(ctx, args.into()),
    AgentsCommand::Status(args) => status(ctx, args),
    AgentsCommand::Unsync(args) => unsync(ctx, args),
    AgentsCommand::Hooks(args) => hooks(ctx, args),
  }
}

pub fn connect(ctx: &Context, command: ConnectCommand) -> CliResult<Outcome> {
  let ConnectCommand::Claude(args) = command;
  sync(
    ctx,
    SyncRequest {
      style: Style::Connect,
      agents: vec![Agent::Claude],
      scope: Some(match args.scope {
        ConnectScope::User => Scope::User,
        ConnectScope::Project => Scope::Project,
      }),
      repos: Vec::new(),
      flags: Flags { force: true, migrate: false },
      dry_run: false,
      yes: args.yes,
    },
  )
}

impl From<AgentsSyncArgs> for SyncRequest {
  fn from(args: AgentsSyncArgs) -> Self {
    Self {
      style: Style::Sync,
      agents: args.agents.iter().copied().map(agent_of).collect(),
      scope: args.scope.map(scope_of),
      repos: args.repos,
      flags: Flags {
        force: args.force,
        migrate: args.migrate,
      },
      dry_run: args.dry_run,
      yes: args.yes,
    }
  }
}

fn agent_of(name: AgentName) -> Agent {
  match name {
    AgentName::Claude => Agent::Claude,
    AgentName::Codex => Agent::Codex,
  }
}

fn scope_of(scope: AgentScope) -> Scope {
  match scope {
    AgentScope::User => Scope::User,
    AgentScope::Local => Scope::Local,
    AgentScope::Project => Scope::Project,
  }
}

const SANDBOX_VARS: [&str; 3] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME", HOME_VAR];

fn refuse_unsandboxed_test() -> CliResult<()> {
  if std::env::var_os("CARGO_MANIFEST_DIR").is_none_or(|value| value.is_empty()) {
    return Ok(());
  }
  let temp = fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
  let unsafe_vars = unsandboxed_vars(|name| std::env::var_os(name), &temp);
  match unsafe_vars.is_empty() {
    true => Ok(()),
    false => Err(CliError::UnsandboxedAgentWiring {
      variables: unsafe_vars.join(", "),
    }),
  }
}

fn unsandboxed_vars(get: impl Fn(&str) -> Option<OsString>, temp: &Path) -> Vec<&'static str> {
  SANDBOX_VARS
    .into_iter()
    .filter(|name| !get(name).filter(|value| !value.is_empty()).is_some_and(|value| is_under(Path::new(&value), temp)))
    .collect()
}

fn is_under(path: &Path, root: &Path) -> bool {
  if !path.is_absolute() || path.components().any(|component| component == std::path::Component::ParentDir) {
    return false;
  }
  let mut existing = path;
  let mut rest = PathBuf::new();
  let canonical = loop {
    if let Ok(found) = fs::canonicalize(existing) {
      break found;
    }
    let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
      return false;
    };
    rest = Path::new(name).join(&rest);
    existing = parent;
  };
  canonical.join(rest).starts_with(root)
}

fn sync(ctx: &Context, request: SyncRequest) -> CliResult<Outcome> {
  refuse_unsandboxed_test()?;
  let registry = ctx.registry()?;
  let settings = registry.user_config().agents();
  let profile = tool_profile(&resolve_mcp(registry.user_config(), &ConfigEnv::default(), McpFlags::default()))?;
  let mut notes = Notes::default();
  let agents = select_agents(&request.agents, &settings)?;
  let scope = request.scope.unwrap_or(match settings.claude.scope {
    ClaudeScope::User => Scope::User,
    ClaudeScope::Local => Scope::Local,
    ClaudeScope::Project => Scope::Project,
  });
  if agents.contains(&Agent::Codex) && scope != Scope::User && request.scope.is_some() {
    notes.push("Codex has one user-level configuration; --scope applies to Claude Code only".to_owned());
  }
  let repos_required = agents.contains(&Agent::Claude) && scope != Scope::User;
  let guidance = match request.style {
    Style::Sync => effective_guidance(settings.guidance),
    Style::Connect => GuidanceMode::Tracked,
  };
  let repos = resolve_repos(ctx, &registry, &request.repos, repos_required, guidance != GuidanceMode::None, &mut notes)?;
  let wiring = Wiring {
    style: request.style,
    scope,
    settings: &settings,
    profile: &profile,
    guidance,
  };
  let desired = desired_wiring(&wiring, &agents, &repos, &mut notes)?;
  Manifest::locked(ctx.home.wiring_path(), |manifest| {
    let selection: Vec<(Agent, Scope)> = agents.iter().map(|agent| (*agent, if *agent == Agent::Claude { scope } else { Scope::User })).collect();
    let repo_roots: Vec<&Path> = repos.iter().map(|repo| repo.root.as_path()).collect();
    let desired_targets: BTreeSet<Target> = desired.iter().map(|item| item.target.clone()).collect();
    let stale: Vec<&Record> = manifest
      .records
      .iter()
      .filter(|_| request.style == Style::Sync)
      .filter(|record| !matches!(record.target, Target::Exclude { .. }))
      .filter(|record| selection.contains(&(record.agent, record.scope)))
      .filter(|record| record.repo.as_deref().is_none_or(|repo| repo_roots.contains(&repo)))
      .filter(|record| !desired_targets.contains(&record.target))
      .collect();
    let stale_targets: BTreeSet<&Target> = stale.iter().map(|record| &record.target).collect();
    let surviving = manifest
      .records
      .iter()
      .map(|record| &record.target)
      .filter(|target| !stale_targets.contains(target))
      .chain(desired_targets.iter());
    let needed = needed_excludes(surviving);
    let mut steps: Vec<Step> = desired
      .into_iter()
      .map(|item| {
        let recorded = manifest.find(&item.target);
        let file_created_by_sync = manifest.records.iter().any(|record| record.created_file && record.target.file() == item.target.file());
        decide_write(item, recorded, file_created_by_sync, request.flags)
      })
      .collect();
    steps.extend(stale.iter().map(|record| decide_removal(record, request.flags)));
    steps.extend(orphan_excludes(&manifest.records, &needed, &desired_targets).map(|record| decide_removal(record, request.flags)));
    if settings.claude.hooks_source == HooksSource::BuiltIn && steps.iter().any(creates_seed_hook) {
      notes.push(DEFAULT_SEED_NOTE.to_owned());
    }
    let user_scope_sync = request.style == Style::Sync && agents.contains(&Agent::Claude) && scope == Scope::User;
    if user_scope_sync {
      steps.extend(migrations(manifest, request.flags, &mut notes));
    }
    for repo in repos.iter().filter(|_| user_scope_sync) {
      let mcp_json = Target::McpJson {
        path: repo.root.join(".mcp.json"),
      };
      if matches!(mcp_json.current(), Current::Present { .. }) && manifest.find(&mcp_json).is_none() {
        notes.push(format!(
          "{} also registers {SERVER_NAME} for this repo and wins over the user scope; remove it or rerun with --scope project",
          mcp_json.label()
        ));
      }
    }
    execute(manifest, steps, &notes, request.dry_run, request.yes)
  })
}

fn creates_seed_hook(step: &Step) -> bool {
  step.change == Change::Created
    && matches!(
      step.target,
      Target::Hooks {
        event: HookEvent::UserPromptSubmit,
        ..
      }
    )
}

fn effective_guidance(mode: GuidanceMode) -> GuidanceMode {
  match mode {
    GuidanceMode::Auto => GuidanceMode::None,
    other => other,
  }
}

fn select_agents(requested: &[Agent], settings: &AgentsSettings) -> CliResult<Vec<Agent>> {
  let explicit = !requested.is_empty();
  let mut agents: Vec<Agent> = match explicit {
    true => requested.to_vec(),
    false => [(Agent::Claude, settings.claude.enabled), (Agent::Codex, settings.codex.enabled)]
      .into_iter()
      .filter_map(|(agent, enabled)| enabled.then_some(agent))
      .collect(),
  };
  agents.sort();
  agents.dedup();
  if agents.contains(&Agent::Codex) && codex::codex_home().is_none_or(|home| !home.is_dir()) {
    let home = codex::codex_home().map_or_else(|| "~/.codex".to_owned(), |home| home.display().to_string());
    if explicit {
      return Err(CliError::InvalidInput(format!("Codex is not set up here: {home} does not exist (CODEX_HOME or ~/.codex)")));
    }
    note(format!("skipped Codex: {home} does not exist"));
    agents.retain(|agent| *agent != Agent::Codex);
  }
  if agents.is_empty() {
    return Err(CliError::InvalidInput("no agent to wire: enable one in config.toml [agents] or pass --agent".to_owned()));
  }
  Ok(agents)
}

fn resolve_repos(ctx: &Context, registry: &Registry, paths: &[PathBuf], required: bool, wanted: bool, notes: &mut Notes) -> CliResult<Vec<RepoRoot>> {
  if !paths.is_empty() {
    let mut roots: Vec<RepoRoot> = Vec::new();
    for path in paths {
      let path = ctx.cwd.join(path);
      let resolution = registry.resolve(&path, ctx.project.as_deref())?;
      for warning in &resolution.warnings {
        notes.push(format!("warning: {warning}"));
      }
      let root = repo_root(&path, &resolution)?;
      if !roots.iter().any(|existing| existing.root == root.root) {
        roots.push(root);
      }
    }
    return Ok(roots);
  }
  if !required && !wanted {
    return Ok(Vec::new());
  }
  match ctx.resolution() {
    Ok(resolution) => Ok(vec![repo_root(&ctx.cwd, &resolution)?]),
    Err(error) if required => Err(error),
    Err(CliError::Project(ProjectError::NotInProject { .. })) => {
      notes.push("no guidance written: the working directory is in no registered project; pass --repo".to_owned());
      Ok(Vec::new())
    }
    Err(error) => Err(error),
  }
}

fn repo_root(path: &Path, resolution: &Resolution) -> CliResult<RepoRoot> {
  if let Some(caveat) = resolution.binding.caveat() {
    note(caveat);
  }
  Ok(RepoRoot {
    root: connection_root(path, resolution)?,
    project: resolution.project.name.to_string(),
  })
}

fn connection_root(path: &Path, resolution: &Resolution) -> CliResult<PathBuf> {
  if let Binding::UnresolvedWorktree(unresolved) = &resolution.binding {
    return Ok(unresolved.root.clone());
  }
  if let Binding::Worktree(worktree) = &resolution.binding {
    let counterpart = resolution
      .project
      .repo(worktree.repo.as_str())
      .and_then(|repo| repo.path.strip_prefix(&worktree.indexed_checkout).ok())
      .map(|relative| worktree.worktree.join(relative))
      .filter(|path| path.is_dir());
    return Ok(counterpart.unwrap_or_else(|| worktree.worktree.clone()));
  }
  let path = fs::canonicalize(path).map_err(|source| CliError::io("canonicalize", path, source))?;
  let repo = resolution
    .project
    .repos
    .iter()
    .filter(|repo| path.starts_with(&repo.path))
    .max_by_key(|repo| repo.path.components().count());
  Ok(repo.map_or(path, |repo| repo.path.clone()))
}

struct Wiring<'a> {
  style: Style,
  scope: Scope,
  settings: &'a AgentsSettings,
  profile: &'a ToolProfile,
  guidance: GuidanceMode,
}

fn desired_wiring(wiring: &Wiring<'_>, agents: &[Agent], repos: &[RepoRoot], notes: &mut Notes) -> CliResult<Vec<Desired>> {
  let home = std::env::var_os(HOME_VAR).filter(|value| !value.is_empty());
  let scope = wiring.scope;
  let mut items: Vec<Desired> = Vec::new();
  for agent in agents {
    match agent {
      Agent::Claude if wiring.style == Style::Connect => connect_wiring(scope, home.as_ref(), repos, notes, &mut items)?,
      Agent::Claude => claude_wiring(scope, wiring.settings, wiring.profile, home.as_ref(), repos, notes, &mut items)?,
      Agent::Codex => codex_wiring(wiring.settings, home.as_ref(), notes, &mut items)?,
    }
    let owner_scope = if *agent == Agent::Claude { scope } else { Scope::User };
    for repo in repos {
      guidance_wiring(*agent, owner_scope, wiring, repo, notes, &mut items);
    }
  }
  let mut excludes = Vec::new();
  for item in &items {
    if !is_local_file(&item.target) {
      continue;
    }
    match guidance::exclude_for(item.target.file()) {
      ExcludeLocation::Excluded { path, line } => excludes.push(Desired {
        owner: item.owner.clone(),
        target: Target::Exclude { path, line },
        content: Content::Line,
        guard: None,
      }),
      ExcludeLocation::NotInGit => {}
      ExcludeLocation::Unresolved(root) => notes.push(format!(
        "warning: {} is not excluded from git: the .git entry at {} does not resolve",
        item.target.file().display(),
        root.display()
      )),
    }
  }
  items.extend(excludes);
  let mut seen = BTreeSet::new();
  items.retain(|item| seen.insert(item.target.clone()));
  Ok(items)
}

fn claude_wiring(
  scope: Scope,
  settings: &AgentsSettings,
  profile: &ToolProfile,
  home: Option<&OsString>,
  repos: &[RepoRoot],
  notes: &mut Notes,
  items: &mut Vec<Desired>,
) -> CliResult<()> {
  let home = home.map(OsString::as_os_str);
  let settings_items = |path: PathBuf| {
    let mut contents = Vec::new();
    if settings.claude.hooks.contains(&ClaudeHook::AutoApprove) {
      contents.push((Target::Permissions { path: path.clone() }, Content::Entries(claude::permission_rules(profile))));
    }
    for event in hook_events(settings) {
      let target = Target::Hooks { path: path.clone(), event };
      contents.push((target, Content::Entries(claude::hook_groups(event, &settings.binary, home))));
    }
    contents
  };
  let mut push = |repo: Option<&RepoRoot>, target: Target, content: Content| {
    items.push(Desired {
      owner: Owner {
        agent: Agent::Claude,
        scope,
        repo: repo.map(|repo| repo.root.clone()),
      },
      target,
      content,
      guard: None,
    })
  };
  match scope {
    Scope::User => {
      check_binary(&settings.binary, notes);
      let target = Target::Registration {
        scope: RegistrationScope::User,
        config: claude_config()?,
        cwd: None,
      };
      push(None, target, Content::Server(claude::server_entry(&settings.binary, home)));
      let path = claude::user_settings_path().ok_or_else(|| CliError::InvalidInput("cannot locate ~/.claude/settings.json: HOME is not set".to_owned()))?;
      for (target, content) in settings_items(path) {
        push(None, target, content);
      }
    }
    Scope::Local => {
      check_binary(&settings.binary, notes);
      for repo in repos {
        let target = Target::Registration {
          scope: RegistrationScope::Local,
          config: claude_config()?,
          cwd: Some(repo.root.clone()),
        };
        push(Some(repo), target, Content::Server(claude::server_entry(&settings.binary, home)));
        for (target, content) in settings_items(claude::local_settings_path(&repo.root)) {
          push(Some(repo), target, content);
        }
      }
    }
    Scope::Project => {
      check_binary(DEFAULT_COMMAND, notes);
      notes.push(format!(
        "warning: .mcp.json is meant to be committed: everyone who opens the repo in Claude Code gets the {SERVER_NAME} server and needs `{DEFAULT_COMMAND}` on PATH"
      ));
      warn_home_left_out(home, notes);
      if !settings.claude.hooks.is_empty() {
        notes.push("permission rules and hooks go into .claude/settings.local.json (excluded from git), not the committed .claude/settings.json".to_owned());
      }
      for repo in repos {
        let mcp_json = Target::McpJson {
          path: repo.root.join(".mcp.json"),
        };
        push(Some(repo), mcp_json, Content::Server(claude::server_entry(DEFAULT_COMMAND, None)));
        for (target, content) in settings_items(claude::local_settings_path(&repo.root)) {
          push(Some(repo), target, content);
        }
      }
    }
  }
  Ok(())
}

fn hook_events(settings: &AgentsSettings) -> Vec<HookEvent> {
  [
    (ClaudeHook::Remind, HookEvent::PreToolUse),
    (ClaudeHook::Seed, HookEvent::UserPromptSubmit),
    (ClaudeHook::Counterpart, HookEvent::PostToolUse),
    (ClaudeHook::Notes, HookEvent::PostToolUseNotes),
  ]
  .into_iter()
  .filter(|(hook, _)| settings.claude.hooks.contains(hook))
  .map(|(_, event)| event)
  .collect()
}

fn hooks(ctx: &Context, args: AgentsHooksArgs) -> CliResult<Outcome> {
  let settings = ctx.registry()?.user_config().agents();
  let binary = args.binary.unwrap_or_else(|| settings.binary.clone());
  let home = args.home.map(OsString::from).or_else(|| std::env::var_os(HOME_VAR).filter(|value| !value.is_empty()));
  let mut groups = serde_json::Map::new();
  for event in hook_events(&settings) {
    let entry = groups.entry(event.name().to_owned()).or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(entries) = entry {
      entries.extend(claude::hook_groups(event, &binary, home.as_deref()));
    }
  }
  if args.json {
    emit_json(&Value::Object(groups))?;
    return Ok(Outcome::Complete);
  }
  if groups.is_empty() {
    note("no hook is enabled in config.toml [agents.claude] hooks");
  }
  for (event, entries) in &groups {
    for command in entries.as_array().into_iter().flatten().filter_map(|group| group["hooks"][0]["command"].as_str()) {
      emit_line(format!("{event}: {command}"))?;
    }
  }
  Ok(Outcome::Complete)
}

fn connect_wiring(scope: Scope, home: Option<&OsString>, repos: &[RepoRoot], notes: &mut Notes, items: &mut Vec<Desired>) -> CliResult<()> {
  let executable = std::env::current_exe()
    .and_then(fs::canonicalize)
    .map_err(|source| CliError::io("locate", "the catl executable", source))?;
  let owner = |repo: Option<&RepoRoot>| Owner {
    agent: Agent::Claude,
    scope,
    repo: repo.map(|repo| repo.root.clone()),
  };
  if scope == Scope::User {
    let entry = claude::server_entry(&executable.display().to_string(), home.map(OsString::as_os_str));
    let target = Target::Registration {
      scope: RegistrationScope::User,
      config: claude_config()?,
      cwd: None,
    };
    items.push(Desired {
      owner: owner(None),
      target,
      content: Content::Server(entry),
      guard: None,
    });
    return Ok(());
  }
  warn_home_left_out(home.map(OsString::as_os_str), notes);
  let command = project_command(&executable, notes);
  for repo in repos {
    let entry = json!({ "type": "stdio", "command": command, "args": ["mcp", "--project", repo.project, "--watch"] });
    items.push(Desired {
      owner: owner(Some(repo)),
      target: Target::McpJson {
        path: repo.root.join(".mcp.json"),
      },
      content: Content::Server(entry),
      guard: None,
    });
  }
  Ok(())
}

fn project_command(executable: &Path, notes: &mut Notes) -> String {
  if find_on_path(DEFAULT_COMMAND).and_then(|found| fs::canonicalize(found).ok()).as_deref() == Some(executable) {
    return DEFAULT_COMMAND.to_owned();
  }
  notes.push(format!(
    "warning: `{DEFAULT_COMMAND}` on PATH is missing or is not this binary, so .mcp.json names {} directly, which makes the file machine-specific; put this {DEFAULT_COMMAND} on PATH and rerun before committing it",
    executable.display()
  ));
  executable.display().to_string()
}

fn warn_home_left_out(home: Option<&std::ffi::OsStr>, notes: &mut Notes) {
  if home.is_some() {
    notes.push(format!(
      "warning: {HOME_VAR} is set but was left out of .mcp.json because that file is usually committed; the server will use the default home"
    ));
  }
}

fn claude_config() -> CliResult<PathBuf> {
  claude_config_path().ok_or_else(|| CliError::InvalidInput("cannot locate ~/.claude.json: neither CLAUDE_CONFIG_DIR nor HOME is set".to_owned()))
}

fn codex_wiring(settings: &AgentsSettings, home: Option<&OsString>, notes: &mut Notes, items: &mut Vec<Desired>) -> CliResult<()> {
  let codex_home = codex::codex_home().ok_or_else(|| CliError::InvalidInput("cannot locate the Codex home: neither CODEX_HOME nor HOME is set".to_owned()))?;
  check_binary(&settings.binary, notes);
  items.push(Desired {
    owner: Owner {
      agent: Agent::Codex,
      scope: Scope::User,
      repo: None,
    },
    target: Target::Codex {
      path: codex::config_path(&codex_home),
    },
    content: Content::Server(codex::server_entry(&settings.binary, home.map(OsString::as_os_str))),
    guard: None,
  });
  Ok(())
}

fn guidance_wiring(agent: Agent, scope: Scope, wiring: &Wiring<'_>, repo: &RepoRoot, notes: &mut Notes, items: &mut Vec<Desired>) {
  let owner = Owner {
    agent,
    scope,
    repo: Some(repo.root.clone()),
  };
  let block = Content::Block(repo_block(&repo.project, wiring.profile));
  match (wiring.guidance, agent) {
    (GuidanceMode::None | GuidanceMode::Auto, _) => {}
    (GuidanceMode::Local, Agent::Codex) => notes.push("Codex has no git-excluded instruction file; guidance = \"local\" applies to Claude Code only".to_owned()),
    (GuidanceMode::Local, Agent::Claude) => {
      let guard = guidance::hides_agents_md(&repo.root).then(|| {
        format!(
          "{} would stop Claude Code from reading AGENTS.md in {} (it reads AGENTS.md only without a CLAUDE.md or CLAUDE.local.md); use guidance = \"tracked\", or set Claude Code's project instructions to claude-md-and-agents-md",
          guidance::LOCAL_FILE,
          repo.root.display()
        )
      });
      items.push(Desired {
        owner,
        target: Target::Guidance {
          path: repo.root.join(guidance::LOCAL_FILE),
        },
        content: block,
        guard,
      });
    }
    (GuidanceMode::Tracked, _) => match guidance::tracked_path(&repo.root) {
      Ok(path) => items.push(Desired {
        owner,
        target: Target::Guidance { path },
        content: block,
        guard: None,
      }),
      Err(reason) => notes.push(format!("warning: no guidance written in {}: {reason}", repo.root.display())),
    },
  }
}

fn is_local_file(target: &Target) -> bool {
  match target {
    Target::Guidance { path } => guidance::is_local_file(path),
    Target::Hooks { path, .. } | Target::Permissions { path } => claude::is_local_settings(path),
    _ => false,
  }
}

fn needed_excludes<'a>(targets: impl Iterator<Item = &'a Target>) -> BTreeSet<Target> {
  targets
    .filter(|target| is_local_file(target))
    .filter_map(|target| match guidance::exclude_for(target.file()) {
      ExcludeLocation::Excluded { path, line } => Some(Target::Exclude { path, line }),
      ExcludeLocation::NotInGit | ExcludeLocation::Unresolved(_) => None,
    })
    .collect()
}

fn orphan_excludes<'a>(records: &'a [Record], needed: &'a BTreeSet<Target>, desired: &'a BTreeSet<Target>) -> impl Iterator<Item = &'a Record> {
  records
    .iter()
    .filter(move |record| matches!(record.target, Target::Exclude { .. }) && !needed.contains(&record.target) && !desired.contains(&record.target))
}

fn migrations(manifest: &Manifest, flags: Flags, notes: &mut Notes) -> Vec<Step> {
  let Some(config) = claude_config_path() else { return Vec::new() };
  let recorded = |cwd: &PathBuf| {
    manifest
      .find(&Target::Registration {
        scope: RegistrationScope::Local,
        config: config.clone(),
        cwd: Some(cwd.clone()),
      })
      .is_some()
  };
  let legacy: Vec<(PathBuf, Value)> = claude::legacy_local_registrations(&config).into_iter().filter(|(cwd, _)| !recorded(cwd)).collect();
  if legacy.is_empty() {
    return Vec::new();
  }
  if !flags.migrate {
    let paths: Vec<String> = legacy.iter().map(|(path, _)| path.display().to_string()).collect();
    notes.push(format!(
      "{} local-scope {SERVER_NAME} entries from an older catl win over the user scope in {}; rerun with --migrate to remove them",
      legacy.len(),
      paths.join(", ")
    ));
    return Vec::new();
  }
  legacy
    .into_iter()
    .map(|(cwd, entry)| Step {
      owner: Owner {
        agent: Agent::Claude,
        scope: Scope::Local,
        repo: Some(cwd.clone()),
      },
      target: Target::Registration {
        scope: RegistrationScope::Local,
        config: config.clone(),
        cwd: Some(cwd.clone()),
      },
      change: Change::Removed,
      effect: Some(Effect::Unregister {
        scope: RegistrationScope::Local,
        cwd: Some(cwd),
        before: Some(entry),
      }),
      redo: Redo::AsPlanned,
      confirm: Some("remove a local-scope entry from an older catl that wins over the user scope".to_owned()),
      reason: None,
      bookkeeping: Bookkeeping::Forget,
    })
    .collect()
}

fn check_binary(binary: &str, notes: &mut Notes) {
  let executable = std::env::current_exe().and_then(fs::canonicalize).ok();
  if Path::new(binary).is_absolute() {
    if !Path::new(binary).is_file() {
      notes.push(format!("warning: [agents] binary {binary} does not exist; agents will fail to start the server"));
    }
    return;
  }
  match find_on_path(binary).and_then(|found| fs::canonicalize(found).ok()) {
    Some(found) if Some(&found) == executable.as_ref() => {}
    Some(found) => notes.push(format!("warning: `{binary}` on PATH is {}, not this binary; agents will start that one", found.display())),
    None => notes.push(format!(
      "warning: `{binary}` is not on PATH, so agents cannot start the server; put catl on PATH or set [agents] binary to an absolute path in config.toml{}",
      executable.map(|path| format!(" (this binary is {})", path.display())).unwrap_or_default()
    )),
  }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
  let path = std::env::var_os("PATH")?;
  std::env::split_paths(&path)
    .filter(|directory| directory.is_absolute())
    .map(|directory| directory.join(name))
    .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
  use std::os::unix::fs::PermissionsExt;
  fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
  path.is_file()
}

fn execute(manifest: &mut Manifest, steps: Vec<Step>, notes: &Notes, dry_run: bool, yes: bool) -> CliResult<Outcome> {
  for note_text in &notes.0 {
    note(note_text);
  }
  for step in &steps {
    show(step)?;
  }
  let refused: Vec<String> = steps.iter().filter(|step| step.change == Change::Refused).map(|step| step.target.label()).collect();
  if !refused.is_empty() {
    return Err(CliError::WiringRefused { targets: refused });
  }
  if dry_run {
    emit_line("dry run: nothing written")?;
    return Ok(Outcome::Complete);
  }
  let asked: Vec<String> = steps
    .iter()
    .filter_map(|step| step.confirm.as_ref().map(|why| format!("{} ({why})", step.target.label())))
    .collect();
  if !asked.is_empty() {
    confirm(&format!("apply these changes to {}", asked.join(", ")), yes)?;
  }
  let registrations: Vec<&Effect> = steps
    .iter()
    .filter_map(|step| step.effect.as_ref())
    .filter(|effect| matches!(effect, Effect::Register { .. } | Effect::Unregister { .. }))
    .collect();
  if let Some(first) = registrations.first() {
    claude::probe(match first {
      Effect::Register { scope, cwd, entry, .. } => Some((*scope, cwd.as_deref(), entry)),
      _ => None,
    })?;
  }
  for step in steps {
    if let Some(planned) = &step.effect {
      let effect = current_effect(&step, planned)?;
      if let Err(error) = apply(&effect).and_then(|()| verify(&step, &effect)) {
        reconcile(manifest, &step.target)?;
        return Err(error);
      }
      if let Effect::Register { scope, .. } = effect {
        emit_line(format!("registered {SERVER_NAME} in the Claude Code {} scope", scope_name(scope)))?;
      }
    }
    match step.bookkeeping {
      Bookkeeping::Keep => {}
      Bookkeeping::Put(record) => manifest.put(record)?,
      Bookkeeping::Forget => manifest.forget(&step.target)?,
    }
  }
  Ok(Outcome::Complete)
}

fn verify(step: &Step, effect: &Effect) -> CliResult<()> {
  let current = step.target.current();
  let missing = |detail: String| CliError::WiringNotVisible {
    target: step.target.label(),
    detail,
  };
  match (effect, &step.bookkeeping, current) {
    (Effect::Register { .. }, Bookkeeping::Put(record), Current::Present { hash, .. }) if hash == record.hash => Ok(()),
    (Effect::Register { .. }, _, current) => Err(missing(format!(
      "`claude mcp add-json` reported success, but the entry Code Atlas reads back is {}; nothing was recorded, check `claude mcp list`",
      describe(&current)
    ))),
    (Effect::Unregister { .. }, _, Current::Absent) => Ok(()),
    (Effect::Unregister { .. }, _, current) => Err(missing(format!("`claude mcp remove` reported success, but the entry is {}", describe(&current)))),
    _ => Ok(()),
  }
}

fn describe(current: &Current) -> String {
  match current {
    Current::Absent => "absent".to_owned(),
    Current::Present { .. } => "different from what was asked for".to_owned(),
    Current::Unreadable(reason) => format!("unreadable ({reason})"),
  }
}

fn reconcile(manifest: &mut Manifest, target: &Target) -> CliResult<()> {
  match (target.current(), manifest.find(target)) {
    (Current::Absent, Some(_)) => manifest.forget(target),
    _ => Ok(()),
  }
}

fn current_effect(step: &Step, planned: &Effect) -> CliResult<Effect> {
  let fresh = match &step.redo {
    Redo::AsPlanned => return Ok(planned.clone()),
    Redo::Write(content) => step.target.write(content).map(Some),
    Redo::Remove { created_file } => step.target.remove(*created_file),
  };
  let unsupported = |reason: String| CliError::InvalidInput(format!("{} changed while syncing: {reason}; nothing more was written", step.target.label()));
  fresh.map_err(unsupported)?.ok_or_else(|| unsupported("the entry is already gone".to_owned()))
}

fn scope_name(scope: RegistrationScope) -> &'static str {
  match scope {
    RegistrationScope::User => "user",
    RegistrationScope::Local => "local",
  }
}

fn show(step: &Step) -> CliResult<()> {
  let label = step.target.label();
  match &step.reason {
    Some(reason) => emit_line(format!("{label}: {}: {reason}", step.change.label()))?,
    None => emit_line(format!("{label}: {}", step.change.label()))?,
  }
  let Some(effect) = &step.effect else { return Ok(()) };
  let diff = match effect {
    Effect::Write { path, before, after } => unified(path, before.as_deref(), after),
    Effect::Delete { path, before } => unified(path, Some(before), ""),
    Effect::Register { before, entry, .. } => unified(Path::new(&label), before.as_ref().map(pretty).as_deref(), &pretty(entry)),
    Effect::Unregister { before, .. } => unified(Path::new(&label), before.as_ref().map(pretty).as_deref(), ""),
  };
  emit(&diff)
}

fn pretty(value: &Value) -> String {
  serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()) + "\n"
}

fn apply(effect: &Effect) -> CliResult<()> {
  match effect {
    Effect::Write { path, before, after } => write_file(path, before.is_some(), after),
    Effect::Delete { path, .. } => fs::remove_file(path).map_err(|source| CliError::io("delete", path, source)),
    Effect::Register { scope, cwd, entry, before } => claude::register(*scope, cwd.as_deref(), entry, before.as_ref()),
    Effect::Unregister { scope, cwd, .. } => claude::unregister(*scope, cwd.as_deref()),
  }
}

fn write_file(path: &Path, existed: bool, text: &str) -> CliResult<()> {
  if !existed {
    if let Some(directory) = path.parent() {
      fs::create_dir_all(directory).map_err(|source| CliError::io("create", directory, source))?;
    }
    return fs::write(path, text).map_err(|source| CliError::io("write", path, source));
  }
  let target = fs::canonicalize(path).map_err(|source| CliError::io("canonicalize", path, source))?;
  let directory = target.parent().unwrap_or(Path::new("."));
  let io = |source| CliError::io("write", &target, source);
  let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(io)?;
  temp.write_all(text.as_bytes()).map_err(io)?;
  let permissions = fs::metadata(&target).map_err(io)?.permissions();
  fs::set_permissions(temp.path(), permissions).map_err(io)?;
  temp.persist(&target).map_err(|error| io(error.error))?;
  Ok(())
}

fn status(ctx: &Context, args: AgentsStatusArgs) -> CliResult<Outcome> {
  let path = ctx.home.wiring_path();
  let records = Manifest::read(&path)?;
  let states: Vec<(&Record, &'static str, Option<String>)> = records
    .iter()
    .map(|record| match record.target.current() {
      Current::Present { hash, .. } if hash == record.hash => (record, "ok", None),
      Current::Present { .. } => (record, "changed", Some("changed since the last sync".to_owned())),
      Current::Absent => (record, "removed", Some("removed since the last sync".to_owned())),
      Current::Unreadable(reason) => (record, "unreadable", Some(reason)),
    })
    .collect();
  let drifted = states.iter().any(|(_, state, _)| *state != "ok");
  if args.json {
    let entries: Vec<Value> = states
      .iter()
      .map(|(record, state, detail)| {
        json!({
          "agent": record.agent.name(),
          "scope": record.scope.name(),
          "repo": record.repo,
          "target": record.target,
          "label": record.target.label(),
          "state": state,
          "detail": detail,
        })
      })
      .collect();
    emit_json(&json!({ "manifest": path, "entries": entries }))?;
    return Ok(Outcome::degraded_if(drifted));
  }
  if records.is_empty() {
    note(format!("nothing recorded in {}; run `catl agents sync`", path.display()));
  }
  for (record, state, detail) in &states {
    let detail = detail.as_ref().map(|detail| format!(" ({detail})")).unwrap_or_default();
    emit_line(format!("{} {} {}: {state}{detail}", record.agent.name(), record.scope.name(), record.target.label()))?;
  }
  if drifted {
    note("drift: `catl agents sync` refuses to overwrite changed entries without --force; `catl agents unsync` removes only unchanged ones");
  }
  Ok(Outcome::degraded_if(drifted))
}

fn unsync(ctx: &Context, args: AgentsUnsyncArgs) -> CliResult<Outcome> {
  refuse_unsandboxed_test()?;
  let agents: Vec<Agent> = args.agents.iter().copied().map(agent_of).collect();
  let scope = args.scope.map(scope_of);
  let repos: Vec<PathBuf> = args
    .repos
    .iter()
    .map(|path| fs::canonicalize(ctx.cwd.join(path)).unwrap_or_else(|_| ctx.cwd.join(path)))
    .collect();
  let flags = Flags {
    force: args.force,
    migrate: false,
  };
  Manifest::locked(ctx.home.wiring_path(), |manifest| {
    let selected: Vec<&Record> = manifest
      .records
      .iter()
      .filter(|record| !matches!(record.target, Target::Exclude { .. }))
      .filter(|record| agents.is_empty() || agents.contains(&record.agent))
      .filter(|record| scope.is_none_or(|scope| scope == record.scope))
      .filter(|record| repos.is_empty() || record.repo.as_ref().is_some_and(|repo| repos.iter().any(|path| path.starts_with(repo))))
      .collect();
    let removed: BTreeSet<&Target> = selected.iter().map(|record| &record.target).collect();
    let needed = needed_excludes(manifest.records.iter().map(|record| &record.target).filter(|target| !removed.contains(target)));
    let none = BTreeSet::new();
    let mut steps: Vec<Step> = selected.iter().map(|record| decide_removal(record, flags)).collect();
    steps.extend(orphan_excludes(&manifest.records, &needed, &none).map(|record| decide_removal(record, flags)));
    if steps.is_empty() {
      note("nothing recorded matches; see `catl agents status`");
    }
    execute(manifest, steps, &Notes::default(), args.dry_run, args.yes)
  })
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use super::*;

  fn vars(entries: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: HashMap<String, OsString> = entries.iter().map(|(name, path)| ((*name).to_owned(), path.as_os_str().to_owned())).collect();
    move |name| map.get(name).cloned()
  }

  #[test]
  fn should_accept_variables_that_lie_under_the_temporary_directory_even_when_not_created_yet() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let claude = root.join("claude");
    fs::create_dir(&claude).unwrap();
    let codex = root.join("codex/not/yet");
    let get = vars(&[("CLAUDE_CONFIG_DIR", &claude), ("CODEX_HOME", &codex), (HOME_VAR, temp.path())]);

    assert!(unsandboxed_vars(get, &root).is_empty());
  }

  #[test]
  fn should_flag_missing_empty_outside_relative_and_escaping_variables() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let escaping = root.join("a/../../elsewhere");
    let get = vars(&[("CLAUDE_CONFIG_DIR", outside.path()), ("CODEX_HOME", Path::new("relative/dir")), (HOME_VAR, &escaping)]);
    let empty = vars(&[("CLAUDE_CONFIG_DIR", Path::new(""))]);

    assert_eq!(unsandboxed_vars(get, &root), ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "CODE_ATLAS_HOME"]);
    assert_eq!(unsandboxed_vars(empty, &root), ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "CODE_ATLAS_HOME"]);
  }

  #[test]
  fn should_follow_symlinks_out_of_the_temporary_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = root.join("link");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let get = vars(&[("CLAUDE_CONFIG_DIR", &link), ("CODEX_HOME", &root), (HOME_VAR, &root)]);

    assert_eq!(unsandboxed_vars(get, &root), ["CLAUDE_CONFIG_DIR"]);
  }
}
