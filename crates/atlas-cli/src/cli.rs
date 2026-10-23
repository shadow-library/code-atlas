use std::path::PathBuf;

use atlas_core::EntityKind;
use atlas_project::ToolsSetting;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (commit ", env!("CODE_ATLAS_BUILD_COMMIT"), ")");

#[derive(Debug, Parser)]
#[command(
  name = "catl",
  version = VERSION,
  about = "Cross-repo code knowledge graph for coding agents",
  after_help = "Exit codes:\n  0  answered with results\n  1  runtime error\n  2  usage or config error (bad flags, ambiguous or no project, invalid values)\n  3  degraded: results were printed but the index is stale, partial or incomplete\n  4  answered, but nothing matched (read commands only: target not found or ambiguous, run or file not found)\n  5  edit refused, nothing written (stale or foreign or unversioned plan, safe-delete blockers, ambiguous target, no or several matches, scope violation, unverified sites, server refusal); every refused edit exits 5, never 4\n  6  edit failed and was rolled back, nothing changed\n  7  edit partially applied: some files changed and the rollback failed; the output lists them (never exits 0)\n  130  interrupted (a write in progress finishes first; if that edit is refused, rolled back or partially applied, its 5, 6 or 7 wins)\n\nAn applied or dry-run edit whose index freshness is degraded exits 3.\n\nIDE commands (outline --source lsp, refs, decl, impls, diagnostics, edit rename|safe-delete, lsp check) start a language server per invocation and stop it on exit; edit replace-body|insert-before|insert-after start one only when one is already warm, which a one-shot process never has, so they start none. Measured: 2-5 s on TypeScript fixtures, 20-65 s cold on Rust (22 s for a 2-crate workspace, 64 s for acme-billing), longer on large workspaces. They suit scripts and CI; the MCP server keeps servers warm."
)]
pub struct Cli {
  #[arg(long, global = true, value_name = "FILTER", help = "Log filter such as `info` or `atlas_engine=debug`; overrides RUST_LOG")]
  pub log_level: Option<String>,
  #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
  pub log_format: LogFormat,
  #[arg(long, short = 'p', global = true, value_name = "NAME", help = "Project name; inferred from the working directory when omitted")]
  pub project: Option<String>,
  #[command(subcommand)]
  pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
  Text,
  Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
  #[command(subcommand, about = "Manage registered projects and their repos")]
  Project(ProjectCommand),
  #[command(subcommand, about = "Show the layered configuration and where each value comes from")]
  Config(ConfigCommand),
  #[command(about = "Pin the current repo to a project with a .code-atlas.toml marker")]
  Init,
  #[command(about = "Index the project now")]
  Index(IndexArgs),
  #[command(about = "Show index health, freshness and spend")]
  Status(StatusArgs),
  #[command(about = "Keep the index fresh in the foreground until interrupted")]
  Watch,
  #[command(about = "Run the opt-in LLM enrichment pass")]
  Enrich(EnrichArgs),
  #[command(about = "Run compiler-grade SCIP indexers now")]
  Scip,
  #[command(subcommand, about = "Query the knowledge graph")]
  Query(QueryCommand),
  #[command(about = "Shortcut for `query search`")]
  Search(SearchArgs),
  #[command(about = "Shortcut for `query impact`")]
  Impact(ImpactArgs),
  #[command(about = "Shortcut for `query context`")]
  Context(ContextArgs),
  #[command(about = "Shortcut for `query callers`")]
  Callers(CallArgs),
  #[command(about = "Shortcut for `query callees`")]
  Callees(CallArgs),
  #[command(about = "Shortcut for `query symbol`")]
  Symbol(SymbolArgs),
  #[command(about = "Shortcut for `query api`")]
  Api(ApiArgs),
  #[command(about = "Shortcut for `query explain`")]
  Explain(ExplainArgs),
  #[command(about = "Shortcut for `query read`")]
  Read(ReadArgs),
  #[command(name = "tests-for", about = "Shortcut for `query tests-for`")]
  TestsFor(TestsForArgs),
  #[command(about = "Shortcut for `query counterparts`")]
  Counterparts(CounterpartsArgs),
  #[command(about = "Shortcut for `query docs`")]
  Docs(DocsArgs),
  #[command(about = "Shortcut for `query path`")]
  Path(PathArgs),
  #[command(subcommand, about = "List the notes under .code-atlas/notes/ with their status, or stamp them with the code they are about")]
  Notes(NotesCommand),
  #[command(about = "Find symbols by name path in the graph")]
  Find(FindArgs),
  #[command(about = "List the symbols of a file from the index (--source lsp for the exact language-server outline)")]
  Outline(OutlineArgs),
  #[command(about = "Exact references to a symbol or position from the language server (--source graph for the graph)")]
  Refs(RefsArgs),
  #[command(about = "Go to the definition and declaration at a position, or at the single regex match in a file")]
  Decl(DeclArgs),
  #[command(about = "Exact implementations of a trait, interface or method from the language server")]
  Impls(ImplsArgs),
  #[command(about = "Show the type hierarchy of a symbol from the graph")]
  Hierarchy(HierarchyArgs),
  #[command(about = "Compiler diagnostics of a file from the language server")]
  Diagnostics(DiagnosticsArgs),
  #[command(subcommand, about = "Exact edits: rename, symbol edits, safe delete, text replacement, plan application")]
  Edit(EditCommand),
  #[command(subcommand, about = "Inspect the language servers")]
  Lsp(LspCommand),
  #[command(subcommand, about = "Wire Code Atlas into Claude Code and Codex as config.toml [agents] asks, and track what was written")]
  Agents(AgentsCommand),
  #[command(subcommand, about = "Wire Code Atlas into Claude Code for this repo as earlier versions did; `agents sync` is the managed way")]
  Connect(ConnectCommand),
  #[command(about = "Answer a Claude Code hook from the JSON on stdin; always exits 0")]
  Hook(HookArgs),
  #[command(about = "Serve the MCP protocol on stdio")]
  Mcp(McpArgs),
  #[command(subcommand, about = "Inspect internals")]
  Debug(DebugCommand),
  #[command(about = "Check tools, registry, databases, locks and language-server discovery")]
  Doctor(DoctorArgs),
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
  Create(CreateArgs),
  AddRepo(AddRepoArgs),
  #[command(about = "Change a repo's include/exclude globs and language filter")]
  SetRepo(SetRepoArgs),
  RemoveRepo(RemoveRepoArgs),
  RenameRepo(RenameRepoArgs),
  List(JsonFlag),
  Show(ShowArgs),
  Delete(DeleteArgs),
  #[command(about = "Show or change a project's settings")]
  Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct JsonFlag {
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
  pub name: String,
  #[arg(long = "repo", value_name = "PATH[=NAME]")]
  pub repos: Vec<String>,
}

#[derive(Debug, Args)]
pub struct AddRepoArgs {
  #[arg(value_name = "PROJECT")]
  pub project_name: String,
  pub path: PathBuf,
  #[arg(long)]
  pub name: Option<String>,
  #[command(flatten)]
  pub filters: RepoFilterArgs,
}

#[derive(Debug, Args)]
pub struct RepoFilterArgs {
  #[arg(long = "include", value_name = "GLOB", help = "Index only paths matching this gitignore-style glob (repeatable)")]
  pub include: Vec<String>,
  #[arg(long = "exclude", value_name = "GLOB", help = "Skip paths matching this gitignore-style glob (repeatable)")]
  pub exclude: Vec<String>,
  #[arg(
    long,
    value_name = "LANGS",
    value_delimiter = ',',
    help = "Index only these languages: rust, ts (typescript, tsx, javascript, jsx), comma separated"
  )]
  pub languages: Option<Vec<String>>,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("change").required(true).multiple(true).args(["include", "exclude", "languages", "clear_filters"])))]
pub struct SetRepoArgs {
  #[arg(value_name = "PROJECT")]
  pub project_name: String,
  pub repo: String,
  #[arg(long = "include", value_name = "GLOB", conflicts_with = "clear_filters", help = "Replace the include globs (repeatable)")]
  pub include: Vec<String>,
  #[arg(long = "exclude", value_name = "GLOB", conflicts_with = "clear_filters", help = "Replace the exclude globs (repeatable)")]
  pub exclude: Vec<String>,
  #[arg(
    long,
    value_name = "LANGS",
    value_delimiter = ',',
    conflicts_with = "clear_filters",
    help = "Replace the language filter: rust, ts, comma separated"
  )]
  pub languages: Option<Vec<String>>,
  #[arg(long, help = "Remove every include glob, exclude glob and language filter")]
  pub clear_filters: bool,
}

#[derive(Debug, Args)]
pub struct RemoveRepoArgs {
  #[arg(value_name = "PROJECT")]
  pub project_name: String,
  pub repo: String,
}

#[derive(Debug, Args)]
pub struct RenameRepoArgs {
  #[arg(value_name = "PROJECT")]
  pub project_name: String,
  pub old: String,
  pub new: String,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
  pub name: String,
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
  pub name: String,
  #[arg(long, help = "Also delete the project's database and caches")]
  pub purge_data: bool,
  #[arg(long, short = 'y', help = "Skip the confirmation prompt")]
  pub yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Toggle {
  On,
  Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LspUnitArg {
  Project,
  Repo,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
  #[arg(value_name = "PROJECT")]
  pub project_name: String,
  #[arg(long, value_enum)]
  pub llm: Option<Toggle>,
  #[arg(long, value_name = "PROVIDER", help = "claude-cli, anthropic, openai-compatible or none")]
  pub llm_provider: Option<String>,
  #[arg(long, value_name = "MODEL")]
  pub llm_model: Option<String>,
  #[arg(long, value_enum, help = "Run SCIP automatically while `watch` or `mcp --watch` is active (explicit `scip` always works)")]
  pub scip: Option<Toggle>,
  #[arg(long, value_name = "SECS", help = "Seconds without file changes before an automatic SCIP run")]
  pub scip_debounce: Option<u64>,
  #[arg(long, value_name = "BYTES")]
  pub max_file_bytes: Option<u64>,
  #[arg(long, value_enum, help = "Allow exact operations to start language servers (default on)")]
  pub lsp: Option<Toggle>,
  #[arg(long, value_enum, help = "Allow fetching typescript-language-server through npx when it is not installed (default on)")]
  pub lsp_download: Option<Toggle>,
  #[arg(
    long,
    value_name = "PATH",
    action = clap::ArgAction::Append,
    help = "Command line of typescript-language-server as argv: repeat the flag per word, the first an absolute path, e.g. --lsp-ts-command /usr/bin/node --lsp-ts-command /opt/ts/cli.mjs (--stdio is added)"
  )]
  pub lsp_ts_command: Vec<String>,
  #[arg(long, conflicts_with = "lsp_ts_command", help = "Forget the configured typescript-language-server command")]
  pub lsp_ts_command_clear: bool,
  #[arg(long, value_name = "SECS", help = "Seconds a warm language server may idle before it is stopped")]
  pub lsp_idle_secs: Option<u64>,
  #[arg(long, value_name = "N", help = "Most language servers running at once")]
  pub lsp_max_servers: Option<usize>,
  #[arg(long, value_enum, help = "One server per project or per repo")]
  pub lsp_unit: Option<LspUnitArg>,
  #[arg(long, value_name = "SECS", help = "Seconds to wait for a server to become ready")]
  pub lsp_ready_timeout_secs: Option<u64>,
  #[arg(long, value_name = "SECS", help = "Seconds `lsp check` and flychecks wait")]
  pub lsp_check_timeout_secs: Option<u64>,
  #[arg(
    long = "lsp-exclude",
    value_name = "GLOB",
    help = "Replace the repo-relative globs of Cargo projects and files the language servers must not load (repeatable)"
  )]
  pub lsp_exclude: Vec<String>,
  #[arg(long, conflicts_with = "lsp_exclude", help = "Remove every lsp exclude glob")]
  pub lsp_exclude_clear: bool,
  #[arg(
    long,
    help = "Forget every setting this project stores in projects.toml (config.toml defaults apply again) before applying the other flags"
  )]
  pub reset: bool,
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct IndexArgs {
  #[arg(long, help = "Re-analyse every file instead of only what changed")]
  pub full: bool,
}

#[derive(Debug, Args)]
pub struct McpArgs {
  #[arg(long, help = "Also watch the repos so answers stay fresh between calls (default: [mcp] watch in config.toml)")]
  pub watch: bool,
  #[arg(long, conflicts_with = "watch", help = "Do not watch, even when config.toml or CODE_ATLAS_MCP_WATCH turns watching on")]
  pub no_watch: bool,
  #[arg(
    long,
    value_name = "PROFILE",
    help = "Tools to serve: core, read-only, full or a comma-separated list of tool names (default: [mcp] tools in config.toml, else full)"
  )]
  pub tools: Option<ToolsSetting>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
  #[command(about = "Print configuration keys with their values; -p or the working directory selects the project")]
  Show(ConfigShowArgs),
}

#[derive(Debug, Args)]
pub struct ConfigShowArgs {
  #[arg(long, help = "Also list keys that still have their built-in value")]
  pub effective: bool,
  #[arg(
    long,
    help = "Prefix each key with the layer it came from: built-in, config.toml, projects.toml, <repo>/.code-atlas.toml, env:<VAR> or flag:<FLAG>"
  )]
  pub origin: bool,
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct EnrichArgs {
  #[arg(long, value_name = "N")]
  pub budget_calls: Option<u32>,
  #[arg(long, value_name = "N")]
  pub budget_tokens: Option<u64>,
  #[arg(long, help = "Also summarise module capabilities, with its own share of the budget")]
  pub summaries: bool,
  #[arg(long, help = "List the candidates each task would see without calling the LLM or writing facts")]
  pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum QueryCommand {
  Overview(OverviewArgs),
  Search(SearchArgs),
  Symbol(SymbolArgs),
  Callers(CallArgs),
  Callees(CallArgs),
  Dependents(TraverseArgs),
  Dependencies(TraverseArgs),
  Impact(ImpactArgs),
  TestsFor(TestsForArgs),
  #[command(about = "Files in other repos or apps that share a file's rare string values and keys, with what only they hold")]
  Counterparts(CounterpartsArgs),
  #[command(about = "Sections of repository docs and notes that match words or mention a target")]
  Docs(DocsArgs),
  #[command(about = "The cheapest path between two targets over graph edges, hops weighted by certainty")]
  Path(PathArgs),
  Api(ApiArgs),
  Similar(SimilarArgs),
  Unused(UnusedArgs),
  Patterns(PatternsArgs),
  Context(ContextArgs),
  Explain(ExplainArgs),
  Read(ReadArgs),
  Changes(ChangesArgs),
}

#[derive(Debug, Args)]
pub struct QueryCommon {
  #[arg(long, help = "Print the full response as JSON")]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct OverviewArgs {
  #[arg(long, help = "Append each repo's README and docs/ headings and its notes count")]
  pub docs: bool,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
  pub text: String,
  #[arg(long = "kind", value_name = "KIND")]
  pub kinds: Vec<EntityKind>,
  #[arg(long, help = "Restrict to one repo, or to a directory: repo:dir or dir")]
  pub repo: Option<String>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, help = "Include stale inferred facts")]
  pub include_stale: bool,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct TargetFilterArgs {
  #[arg(long = "kind", value_name = "KIND", help = "Disambiguate the target by entity kind")]
  pub kinds: Vec<EntityKind>,
  #[arg(long, help = "Disambiguate the target by repo")]
  pub repo: Option<String>,
}

#[derive(Debug, Args)]
pub struct SymbolArgs {
  pub target: String,
  #[arg(long, help = "Include the full body")]
  pub body: bool,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct CallArgs {
  #[command(flatten)]
  pub traverse: TraverseArgs,
  #[arg(
    long,
    help = "Follow HTTP hops: from a handler to the client calls that reach its endpoint (callers) or from a client call to the handler it reaches (callees)"
  )]
  pub via_http: bool,
}

#[derive(Debug, Args)]
pub struct TraverseArgs {
  pub target: String,
  #[arg(long)]
  pub depth: Option<u32>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_confidence)]
  pub min_confidence: Option<f32>,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ImpactArgs {
  #[arg(required = true)]
  pub targets: Vec<String>,
  #[arg(long)]
  pub depth: Option<u32>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_confidence)]
  pub min_confidence: Option<f32>,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct TestsForArgs {
  pub target: String,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_confidence)]
  pub min_confidence: Option<f32>,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct CounterpartsArgs {
  #[arg(help = "A file ([repo:]path) or a symbol, whose file is used")]
  pub target: String,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("docs_subject").required(true).multiple(true).args(["query", "about"])))]
pub struct DocsArgs {
  #[arg(help = "Words to rank sections by")]
  pub query: Option<String>,
  #[arg(long, value_name = "TARGET", help = "Only sections that mention this symbol or file (for a file, any symbol in it)")]
  pub about: Option<String>,
  #[arg(long, help = "Only notes under .code-atlas/notes/")]
  pub notes: bool,
  #[arg(long)]
  pub limit: Option<usize>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct PathArgs {
  pub from: String,
  pub to: String,
  #[arg(long, value_name = "HOPS", help = "Longest path searched, 1-12 (default 6)")]
  pub max_depth: Option<u32>,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Subcommand)]
pub enum NotesCommand {
  #[command(about = "Each note with the status of its anchors: current, stale (the code changed since stamping), unstamped or missing")]
  List(NotesListArgs),
  #[command(about = "Record the hash of the code each anchor names in the note's frontmatter (adds the frontmatter from its mentions when missing)")]
  Stamp(NotesStampArgs),
}

#[derive(Debug, Args)]
pub struct NotesListArgs {
  #[arg(long, help = "Only notes with a stale or missing anchor")]
  pub stale: bool,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct NotesStampArgs {
  #[arg(required = true, value_name = "NOTE", help = "Note files: a path, or [repo:]path inside a repo")]
  pub files: Vec<String>,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
pub struct ApiArgs {
  #[arg(long)]
  pub repo: Option<String>,
  #[arg(long, value_name = "METHOD ROUTE")]
  pub endpoint: Option<String>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_confidence)]
  pub min_confidence: Option<f32>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("subject").required(true).args(["target", "snippet", "code"])))]
pub struct SimilarArgs {
  pub target: Option<String>,
  #[arg(long, value_name = "FILE:START-END", help = "Find code similar to a line range, e.g. src/money.rs:3-12")]
  pub snippet: Option<String>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_unit_interval)]
  pub threshold: Option<f64>,
  #[arg(
    long,
    value_name = "FILE|-",
    requires = "language",
    help = "Find code similar to a draft read from a file or stdin; needs --language"
  )]
  pub code: Option<String>,
  #[arg(long, value_enum, requires = "code", help = "Language of the draft")]
  pub language: Option<CodeLanguage>,
  #[arg(long, help = "Include stale inferred facts")]
  pub include_stale: bool,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CodeLanguage {
  Rust,
  Ts,
  Tsx,
  Js,
}

#[derive(Debug, Args)]
pub struct UnusedArgs {
  #[arg(long = "kind", value_name = "KIND")]
  pub kinds: Vec<EntityKind>,
  #[arg(long)]
  pub repo: Option<String>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct PatternsArgs {
  pub topic: String,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, help = "Omit source excerpts")]
  pub no_excerpts: bool,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ContextArgs {
  pub task: String,
  #[arg(long, value_name = "TOKENS")]
  pub budget: Option<usize>,
  #[arg(long = "seed", value_name = "TARGET", help = "Start from this symbol, file or endpoint instead of searching")]
  pub seeds: Vec<String>,
  #[arg(
    long = "known",
    value_name = "TARGET",
    help = "A file or symbol you already hold: anchors the pack like a seed but is not listed in MUST READ or SKIM"
  )]
  pub known: Vec<String>,
  #[arg(long, value_name = "HOPS", value_parser = parse_context_depth, help = "Graph hops expanded from the seeds, 0 to 3")]
  pub depth: Option<u32>,
  #[arg(long, help = "Show scores, expansions and omissions")]
  pub explain: bool,
  #[arg(long, value_parser = parse_confidence)]
  pub min_confidence: Option<f32>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ExplainArgs {
  pub target: String,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
  pub target: String,
  #[arg(long, help = "Show only the signature")]
  pub signature: bool,
  #[arg(long, value_name = "LINES", default_value_t = 0)]
  pub context: u32,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ChangesArgs {
  #[arg(long, value_name = "RUN")]
  pub since_run: Option<i64>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceArg {
  Lsp,
  Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SeverityArg {
  Error,
  Warning,
  Info,
  Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CrossRepoArg {
  Report,
  Apply,
}

#[derive(Debug, Args)]
pub struct FindArgs {
  #[arg(value_name = "NAME_PATH")]
  pub pattern: String,
  #[arg(long, value_name = "PATH", help = "Restrict to a file or directory, optionally prefixed with the repo")]
  pub path: Option<String>,
  #[arg(long, help = "Include children down to this depth")]
  pub depth: Option<u32>,
  #[arg(long, help = "Include the full body")]
  pub body: bool,
  #[arg(long, help = "Match the last segment as a substring")]
  pub substring: bool,
  #[arg(long = "kind", value_name = "KIND")]
  pub kinds: Vec<EntityKind>,
  #[arg(long = "exclude-kind", value_name = "KIND")]
  pub exclude_kinds: Vec<EntityKind>,
  #[arg(long, value_name = "N")]
  pub max: Option<usize>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct OutlineArgs {
  #[arg(value_name = "[REPO:]PATH")]
  pub file: String,
  #[arg(long)]
  pub depth: Option<u32>,
  #[arg(long, value_enum, default_value_t = SourceArg::Graph)]
  pub source: SourceArg,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct RefsArgs {
  #[arg(value_name = "TARGET", help = "Name path, key, or [REPO:]PATH:LINE[:COL]")]
  pub target: String,
  #[arg(long, value_enum, default_value_t = SourceArg::Lsp)]
  pub source: SourceArg,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("where").required(true).args(["target", "path"])))]
pub struct DeclArgs {
  #[arg(value_name = "[REPO:]PATH:LINE:COL")]
  pub target: Option<String>,
  #[arg(long, value_name = "[REPO:]PATH", requires = "regex")]
  pub path: Option<String>,
  #[arg(
    long,
    value_name = "REGEX",
    requires = "path",
    help = "Regex with one capture group around the name; it must match exactly once in the file"
  )]
  pub regex: Option<String>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct ImplsArgs {
  pub target: String,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct HierarchyArgs {
  pub target: String,
  #[arg(long)]
  pub depth: Option<u32>,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct DiagnosticsArgs {
  #[arg(value_name = "[REPO:]PATH")]
  pub file: String,
  #[arg(long, value_name = "A-B", help = "Only diagnostics on these 1-based lines")]
  pub lines: Option<String>,
  #[arg(long, value_enum, value_name = "SEVERITY", default_value_t = SeverityArg::Hint)]
  pub min_severity: SeverityArg,
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct EditOutput {
  #[arg(long, help = "Show the plan (diffs and plan id) without writing anything")]
  pub dry_run: bool,
  #[arg(
    long,
    value_name = "FILE",
    requires = "dry_run",
    help = "Write the dry-run plan to FILE as JSON; apply it later with `edit apply FILE`"
  )]
  pub plan_out: Option<PathBuf>,
  #[arg(long, help = "Print the full outcome as JSON")]
  pub json: bool,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("replacement").required(true).args(["text", "text_file"])))]
pub struct SymbolEditArgs {
  pub target: String,
  #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
  pub text: Option<String>,
  #[arg(long, value_name = "FILE|-", help = "Read the text from a file or stdin")]
  pub text_file: Option<String>,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
pub struct RenameArgs {
  pub target: String,
  #[arg(value_name = "NEW")]
  pub new_name: String,
  #[arg(long, value_enum, default_value_t = CrossRepoArg::Report, help = "Report uses of the symbol in other repos, or edit the ones the graph proves")]
  pub cross_repo: CrossRepoArg,
  #[arg(long, help = "Apply even when sites the index resolves to the symbol were not renamed by the server; check them first")]
  pub allow_unverified: bool,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
pub struct SafeDeleteArgs {
  pub target: String,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("replacement").required(true).args(["text", "text_file"])))]
pub struct ReplaceArgs {
  #[arg(value_name = "[REPO:]PATH")]
  pub path: String,
  #[arg(long, allow_hyphen_values = true)]
  pub pattern: String,
  #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
  pub text: Option<String>,
  #[arg(long, value_name = "FILE|-", help = "Read the replacement from a file or stdin")]
  pub text_file: Option<String>,
  #[arg(long, help = "Treat the pattern as a regex")]
  pub regex: bool,
  #[arg(long, help = "Replace every match instead of requiring exactly one")]
  pub all: bool,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("replacement").required(true).args(["text", "text_file"])))]
pub struct ReplaceInFilesArgs {
  #[arg(long, allow_hyphen_values = true)]
  pub pattern: String,
  #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
  pub text: Option<String>,
  #[arg(long, value_name = "FILE|-", help = "Read the replacement from a file or stdin")]
  pub text_file: Option<String>,
  #[arg(long, help = "Treat the pattern as a regex")]
  pub regex: bool,
  #[arg(long = "include", value_name = "GLOB")]
  pub include: Vec<String>,
  #[arg(long = "exclude", value_name = "GLOB")]
  pub exclude: Vec<String>,
  #[arg(long = "occurrence", value_name = "ID", help = "Apply only these occurrence ids from the dry run (repeatable)")]
  pub occurrences: Vec<String>,
  #[arg(long, value_name = "N", help = "Apply every match, which must number exactly N")]
  pub expected_count: Option<usize>,
  #[command(flatten)]
  pub output: EditOutput,
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
  #[arg(value_name = "PLAN.json")]
  pub plan: PathBuf,
  #[arg(long, help = "Print the full outcome as JSON")]
  pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum EditCommand {
  #[command(about = "Rename a symbol everywhere the language server finds it")]
  Rename(RenameArgs),
  #[command(about = "Replace a symbol's declaration; restate all decorators/attributes of a group or none")]
  ReplaceBody(SymbolEditArgs),
  #[command(about = "Insert text before a symbol, after its leading docs and attributes")]
  InsertBefore(SymbolEditArgs),
  #[command(about = "Insert text after a symbol")]
  InsertAfter(SymbolEditArgs),
  #[command(about = "Delete a symbol only when nothing references it")]
  SafeDelete(SafeDeleteArgs),
  #[command(about = "Replace text in one file; exactly one match unless --all")]
  Replace(ReplaceArgs),
  #[command(about = "Replace text across files; without --occurrence or --expected-count it is a dry run")]
  ReplaceInFiles(ReplaceInFilesArgs),
  #[command(about = "Apply a plan file made with --dry-run --plan-out; refused when the files changed since")]
  Apply(ApplyArgs),
}

#[derive(Debug, Subcommand)]
pub enum LspCommand {
  #[command(about = "Start each language server once and report discovery, versions, readiness time and workspace health")]
  Check(LspCheckArgs),
}

#[derive(Debug, Args)]
pub struct LspCheckArgs {
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
  #[arg(long, help = "Also start each language server of the working directory's project and report its health (slow)")]
  pub lsp: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConnectCommand {
  Claude(ConnectClaudeArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConnectScope {
  Project,
  User,
}

#[derive(Debug, Args)]
pub struct ConnectClaudeArgs {
  #[arg(long, value_enum, default_value_t = ConnectScope::Project)]
  pub scope: ConnectScope,
  #[arg(long, short = 'y', help = "Apply changes to existing files without asking")]
  pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
  #[command(about = "Write or update the agent wiring config.toml [agents] asks for, and record it in wiring.json")]
  Sync(AgentsSyncArgs),
  #[command(about = "List what agents sync recorded and whether it changed since (exits 3 on drift)")]
  Status(AgentsStatusArgs),
  #[command(about = "Remove exactly what agents sync recorded")]
  Unsync(AgentsUnsyncArgs),
  #[command(about = "Print the Claude Code hook groups agents sync would write, by event; writes nothing")]
  Hooks(AgentsHooksArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentName {
  Claude,
  Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentScope {
  User,
  Local,
  Project,
}

#[derive(Debug, Args)]
pub struct AgentsSyncArgs {
  #[arg(long = "agent", value_enum, help = "Agent to wire (repeatable); defaults to the agents enabled in config.toml")]
  pub agents: Vec<AgentName>,
  #[arg(
    long,
    value_enum,
    help = "Claude Code scope: user (one cwd-routed server), local (per repo, not committed) or project (.mcp.json, committed); defaults to [agents.claude] scope"
  )]
  pub scope: Option<AgentScope>,
  #[arg(
    long = "repo",
    value_name = "PATH",
    help = "Repo to wire for local or project scope and guidance (repeatable); defaults to the repo of the working directory"
  )]
  pub repos: Vec<PathBuf>,
  #[arg(long, help = "Show what would change and write nothing")]
  pub dry_run: bool,
  #[arg(long, help = "Overwrite entries that were changed since the last sync or that agents sync did not write")]
  pub force: bool,
  #[arg(long, help = "Replace entries from an older catl (an absolute command or a fixed --project)")]
  pub migrate: bool,
  #[arg(long, short = 'y', help = "Apply changes to existing files without asking")]
  pub yes: bool,
}

#[derive(Debug, Args)]
pub struct AgentsStatusArgs {
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentsHooksArgs {
  #[arg(long, value_name = "COMMAND", help = "catl command the hooks run; defaults to [agents] binary")]
  pub binary: Option<String>,
  #[arg(long, value_name = "DIR", help = "CODE_ATLAS_HOME the hooks run with; defaults to the CODE_ATLAS_HOME of this shell")]
  pub home: Option<PathBuf>,
  #[arg(long, help = "Print the hooks object of a Claude Code settings file as JSON")]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentsUnsyncArgs {
  #[arg(long = "agent", value_enum)]
  pub agents: Vec<AgentName>,
  #[arg(long, value_enum)]
  pub scope: Option<AgentScope>,
  #[arg(long = "repo", value_name = "PATH")]
  pub repos: Vec<PathBuf>,
  #[arg(long, help = "Show what would be removed and write nothing")]
  pub dry_run: bool,
  #[arg(long, help = "Also remove entries that were changed since the last sync")]
  pub force: bool,
  #[arg(long, short = 'y', help = "Apply changes without asking")]
  pub yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookName {
  Remind,
  Seed,
  Counterpart,
  Notes,
}

#[derive(Debug, Args)]
pub struct HookArgs {
  #[arg(value_enum)]
  pub hook: HookName,
}

#[derive(Debug, Subcommand)]
pub enum DebugCommand {
  Run(DebugRunArgs),
  ExplainContext(DebugExplainContextArgs),
  DumpFile(DebugDumpFileArgs),
}

#[derive(Debug, Args)]
pub struct DebugRunArgs {
  pub id: i64,
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct DebugExplainContextArgs {
  pub task: String,
  #[arg(long)]
  pub json: bool,
}

#[derive(Debug, Args)]
pub struct DebugDumpFileArgs {
  #[arg(value_name = "REPO:PATH")]
  pub file: String,
}

fn parse_confidence(raw: &str) -> Result<f32, String> {
  let value: f32 = raw.parse().map_err(|_| format!("{raw:?} is not a number"))?;
  if (0.0..=1.0).contains(&value) {
    Ok(value)
  } else {
    Err(format!("{raw} is outside 0..=1"))
  }
}

fn parse_context_depth(raw: &str) -> Result<u32, String> {
  let value: u32 = raw.parse().map_err(|_| format!("{raw:?} is not a whole number"))?;
  if value <= atlas_query::MAX_CONTEXT_DEPTH {
    Ok(value)
  } else {
    Err(format!("{raw} is outside 0..={}", atlas_query::MAX_CONTEXT_DEPTH))
  }
}

fn parse_unit_interval(raw: &str) -> Result<f64, String> {
  let value: f64 = raw.parse().map_err(|_| format!("{raw:?} is not a number"))?;
  if (0.0..=1.0).contains(&value) {
    Ok(value)
  } else {
    Err(format!("{raw} is outside 0..=1"))
  }
}
