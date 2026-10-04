use std::path::PathBuf;

use atlas_core::EntityKind;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (commit ", env!("CODE_ATLAS_BUILD_COMMIT"), ")");

#[derive(Debug, Parser)]
#[command(
  name = "code-atlas",
  version = VERSION,
  about = "Cross-repo code knowledge graph for coding agents",
  after_help = "Exit codes:\n  0  answered with results\n  1  runtime error\n  2  usage or config error (bad flags, ambiguous or no project, invalid values)\n  3  degraded: results were printed but the index is stale, partial or incomplete\n  4  answered, but nothing matched (target not found or ambiguous, run or file not found)"
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
  Callers(TraverseArgs),
  #[command(about = "Shortcut for `query callees`")]
  Callees(TraverseArgs),
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
  #[command(subcommand, about = "Wire Code Atlas into an agent client")]
  Connect(ConnectCommand),
  #[command(about = "Serve the MCP protocol on stdio")]
  Mcp(McpArgs),
  #[command(subcommand, about = "Inspect internals")]
  Debug(DebugCommand),
  #[command(about = "Check tools, registry, databases and locks")]
  Doctor,
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
  #[arg(long, help = "Restore every setting to its default before applying the other flags")]
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
  #[arg(long, help = "Also watch the repos so answers stay fresh between calls")]
  pub watch: bool,
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
  Callers(TraverseArgs),
  Callees(TraverseArgs),
  Dependents(TraverseArgs),
  Dependencies(TraverseArgs),
  Impact(ImpactArgs),
  TestsFor(TestsForArgs),
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
  #[command(flatten)]
  pub common: QueryCommon,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
  pub text: String,
  #[arg(long = "kind", value_name = "KIND")]
  pub kinds: Vec<EntityKind>,
  #[arg(long)]
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
#[command(group(ArgGroup::new("subject").required(true).args(["target", "snippet"])))]
pub struct SimilarArgs {
  pub target: Option<String>,
  #[arg(long, value_name = "FILE:START-END", help = "Find code similar to a line range, e.g. src/money.rs:3-12")]
  pub snippet: Option<String>,
  #[arg(long)]
  pub limit: Option<usize>,
  #[arg(long, value_parser = parse_unit_interval)]
  pub threshold: Option<f64>,
  #[arg(long, help = "Include stale inferred facts")]
  pub include_stale: bool,
  #[command(flatten)]
  pub filter: TargetFilterArgs,
  #[command(flatten)]
  pub common: QueryCommon,
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

fn parse_unit_interval(raw: &str) -> Result<f64, String> {
  let value: f64 = raw.parse().map_err(|_| format!("{raw:?} is not a number"))?;
  if (0.0..=1.0).contains(&value) {
    Ok(value)
  } else {
    Err(format!("{raw} is outside 0..=1"))
  }
}
