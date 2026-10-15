use atlas_core::{EntityKind, Language};
use atlas_ide::{CrossRepo, DeclarationRequest, DiagnosticsRequest, EditOperation, EditRequest, LineRange, OutlineRequest, ReferencesRequest};
use atlas_query::{FindRequest, HierarchyRequest, NameMatch, ReadRequest, SimilarRequest, SymbolRequest};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

use crate::error::ToolError;

const MAX_LIMIT: usize = 200;
const MAX_DEPTH: u32 = 6;
const MIN_BUDGET: usize = 500;
const MAX_BUDGET: usize = 32_000;
const MAX_CONTEXT_LINES: u32 = 100;

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "rmcp::schemars")]
pub enum Format {
  #[default]
  Text,
  Json,
}

macro_rules! arguments {
  ($(#[$meta:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
    $(#[$meta])*
    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[schemars(crate = "rmcp::schemars")]
    pub struct $name {
      $($(#[$field_meta])* pub $field: $ty,)*
      pub format: Option<Format>,
    }
  };
}

arguments!(ContextArgs {
  #[schemars(description = "What you are about to do, in your own words")]
  task: String,
  #[schemars(description = "Pack size in tokens, 500-32000; larger packs hold more bodies")]
  budget: Option<usize>,
  #[schemars(description = "Symbols, files or keys to anchor the pack on")]
  seeds: Option<Vec<String>>,
  #[schemars(description = "Files or symbols you already hold: they anchor the pack but are not listed again")]
  known: Option<Vec<String>>,
});

arguments!(SearchArgs {
  #[schemars(description = "Words, a phrase or an identifier")]
  query: String,
  #[schemars(description = "Only this entity kind, e.g. function, class, api_endpoint")]
  kind: Option<String>,
  #[schemars(description = "Restrict to one repo of the project")]
  repo: Option<String>,
  #[schemars(description = "Maximum hits per section, 1-200")]
  limit: Option<usize>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum SymbolMode {
  Info,
  Find,
  Outline,
  Source,
  References,
  Declaration,
  Implementations,
  Hierarchy,
  Diagnostics,
}

impl SymbolMode {
  fn name(self) -> &'static str {
    match self {
      Self::Info => "info",
      Self::Find => "find",
      Self::Outline => "outline",
      Self::Source => "source",
      Self::References => "references",
      Self::Declaration => "declaration",
      Self::Implementations => "implementations",
      Self::Hierarchy => "hierarchy",
      Self::Diagnostics => "diagnostics",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum ReadSource {
  Lsp,
  Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum Severity {
  Error,
  Warning,
  Info,
  Hint,
}

arguments!(SymbolArgs {
  #[schemars(description = "Name, name path (A/b, b[1]), [repo:]path[:line[:col]], 'GET /users/{id}' or key")]
  target: Option<String>,
  mode: Option<SymbolMode>,
  #[schemars(description = "find: scope; outline/diagnostics: the file; declaration: file for regex")]
  path: Option<String>,
  #[schemars(description = "find/outline 0-3, hierarchy 1-5")]
  depth: Option<u32>,
  #[schemars(description = "info/find: include bodies")]
  body: Option<bool>,
  #[schemars(description = "find: substring match on the last segment")]
  substring: Option<bool>,
  #[schemars(description = "find: kinds to include")]
  kinds: Option<Vec<String>>,
  exclude_kinds: Option<Vec<String>>,
  #[schemars(description = "find: 1-200")]
  max_matches: Option<usize>,
  #[schemars(description = "declaration: capture group around the name in path")]
  regex: Option<String>,
  #[schemars(description = "outline: graph (default, the index) or lsp (exact); references: lsp (default) or graph")]
  source: Option<ReadSource>,
  #[schemars(description = "source: extra lines, 0-100")]
  context_lines: Option<u32>,
  #[schemars(description = "diagnostics: START-END")]
  lines: Option<String>,
  #[schemars(description = "diagnostics: lowest severity")]
  min_severity: Option<Severity>,
});

arguments!(TraverseArgs {
  #[schemars(description = "Function or method: name, [repo:]path[:line] or full key")]
  target: String,
  #[schemars(description = "Hops to follow, 1-6 (default 1)")]
  depth: Option<u32>,
  #[schemars(description = "Also follow HTTP calls across repos")]
  via_http: Option<bool>,
});

arguments!(ImpactArgs {
  #[schemars(description = "Symbols, files, endpoints or keys that will change")]
  targets: Vec<String>,
  #[schemars(description = "Hops to follow, 1-6 (default 3)")]
  depth: Option<u32>,
});

arguments!(TargetArgs {
  #[schemars(description = "Name (UsersService.findOne), [repo:]path[:line], 'GET /users/{id}' or full key")]
  target: String,
});

arguments!(ApiArgs {
  #[schemars(description = "Restrict to one repo of the project")]
  repo: Option<String>,
  #[schemars(description = "One endpoint, e.g. 'GET /users/{id}'")]
  endpoint: Option<String>,
});

arguments!(SimilarArgs {
  #[schemars(description = "Symbol: name, [repo:]path[:line] or full key")]
  target: Option<String>,
  #[schemars(description = "Line range inside an indexed symbol: '[repo:]path:START-END'")]
  snippet: Option<String>,
  #[schemars(description = "Unsaved draft code to compare; needs language")]
  code: Option<String>,
  #[schemars(description = "rust, ts, tsx or js")]
  language: Option<String>,
});

arguments!(UnusedArgs {
  #[schemars(description = "Restrict to an entity kind such as function, api_endpoint, event, config_key")]
  kind: Option<String>,
  #[schemars(description = "Restrict to one repo of the project")]
  repo: Option<String>,
});

arguments!(PatternsArgs {
  #[schemars(description = "What to look for, e.g. 'error handling', 'pagination', 'auth guard'")]
  topic: String,
});

arguments!(NoArgs {});

arguments!(StatusArgs {
  #[schemars(description = "Also list index changes in runs newer than this run id (0 = recent runs)")]
  since_run: Option<i64>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum EditKind {
  Rename,
  ReplaceBody,
  InsertBefore,
  InsertAfter,
  SafeDelete,
  ReplaceContent,
  ReplaceInFiles,
  Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum CrossRepoMode {
  Report,
  Apply,
}

arguments!(EditArgs {
  operation: EditKind,
  #[schemars(description = "Symbol name, name path or position")]
  target: Option<String>,
  #[schemars(description = "replace_content: [repo:]file")]
  path: Option<String>,
  new_name: Option<String>,
  #[schemars(description = "New body, inserted code or replacement ($1 with regex)")]
  text: Option<String>,
  #[schemars(description = "replace_content/replace_in_files: text to find")]
  pattern: Option<String>,
  regex: Option<bool>,
  #[schemars(description = "replace_content: allow several matches")]
  all: Option<bool>,
  #[schemars(description = "replace_in_files: globs, optional repo: prefix")]
  include: Option<Vec<String>>,
  exclude: Option<Vec<String>>,
  occurrences: Option<Vec<String>>,
  expected_count: Option<usize>,
  #[schemars(description = "rename: report (default) or apply authoritative other-repo sites")]
  cross_repo: Option<CrossRepoMode>,
  allow_unverified: Option<bool>,
  #[schemars(description = "apply: plan id from a dry run")]
  plan: Option<String>,
  dry_run: Option<bool>,
});

pub(crate) fn limit(value: Option<usize>) -> Result<Option<usize>, ToolError> {
  let Some(value) = value else { return Ok(None) };
  if (1..=MAX_LIMIT).contains(&value) {
    return Ok(Some(value));
  }
  Err(ToolError::invalid(format!("limit must be between 1 and {MAX_LIMIT}, got {value}")))
}

pub(crate) fn budget(value: Option<usize>) -> Result<Option<usize>, ToolError> {
  let Some(value) = value else { return Ok(None) };
  if (MIN_BUDGET..=MAX_BUDGET).contains(&value) {
    return Ok(Some(value));
  }
  Err(ToolError::invalid(format!("budget must be between {MIN_BUDGET} and {MAX_BUDGET} tokens, got {value}")))
}

pub(crate) fn depth(value: Option<u32>, default: u32) -> Result<u32, ToolError> {
  let Some(value) = value else { return Ok(default) };
  if (1..=MAX_DEPTH).contains(&value) {
    return Ok(value);
  }
  Err(ToolError::invalid(format!("depth must be between 1 and {MAX_DEPTH}, got {value}")))
}

pub(crate) fn context_lines(value: Option<u32>) -> Result<u32, ToolError> {
  let value = value.unwrap_or(0);
  if value <= MAX_CONTEXT_LINES {
    return Ok(value);
  }
  Err(ToolError::invalid(format!("context_lines must be at most {MAX_CONTEXT_LINES}, got {value}")))
}

pub(crate) fn kinds(value: Option<&str>) -> Result<Vec<EntityKind>, ToolError> {
  let Some(value) = value else { return Ok(Vec::new()) };
  value
    .parse::<EntityKind>()
    .map(|kind| vec![kind])
    .map_err(|_| ToolError::invalid(format!("unknown kind {value:?}; kinds are snake_case, e.g. function, method, class, api_endpoint")))
}

pub(crate) fn non_empty(name: &str, value: &str) -> Result<(), ToolError> {
  if value.trim().is_empty() {
    return Err(ToolError::invalid(format!("{name} must not be empty")));
  }
  Ok(())
}

pub(crate) enum SymbolPlan {
  Info(SymbolRequest),
  Find(FindRequest),
  Outline(OutlineRequest),
  Source(ReadRequest),
  References(ReferencesRequest),
  Declaration(DeclarationRequest),
  Implementations(String),
  Hierarchy(HierarchyRequest),
  Diagnostics(DiagnosticsRequest),
}

pub(crate) enum SimilarPlan {
  Indexed(SimilarRequest),
  Draft(SimilarRequest),
}

type Given<'a> = &'a [(&'static str, bool)];

fn takes(subject: &str, required: &[&str], optional: &[&str], given: Given<'_>) -> Result<(), ToolError> {
  check(subject, &listing(&required.join(", "), optional), required, &[required, optional].concat(), given)
}

fn takes_either(subject: &str, either: [&str; 2], optional: &[&str], given: Given<'_>) -> Result<(), ToolError> {
  check(subject, &listing(&either.join(" or "), optional), &[], &[&either, optional].concat(), given)
}

fn listing(required: &str, optional: &[&str]) -> String {
  match optional.is_empty() {
    true => required.to_owned(),
    false => format!("{required} (optional: {})", optional.join(", ")),
  }
}

fn check(subject: &str, listing: &str, required: &[&str], allowed: &[&str], given: Given<'_>) -> Result<(), ToolError> {
  if let Some((name, _)) = given.iter().find(|(name, present)| *present && !allowed.contains(name)) {
    return Err(ToolError::invalid(format!("{subject} takes {listing}; {name} is not allowed")));
  }
  if let Some(missing) = required.iter().find(|name| !given.iter().any(|(candidate, present)| *present && candidate == *name)) {
    return Err(ToolError::invalid(format!("{subject} takes {listing}; {missing} is missing")));
  }
  Ok(())
}

fn one_of(subject: &str, first: (&str, Option<String>), second: (&str, Option<String>)) -> Result<String, ToolError> {
  match (first.1, second.1) {
    (Some(value), None) | (None, Some(value)) => {
      non_empty(first.0, &value)?;
      Ok(value)
    }
    (None, None) => Err(ToolError::invalid(format!("{subject} needs {} or {}", first.0, second.0))),
    (Some(_), Some(_)) => Err(ToolError::invalid(format!("{subject} takes {} or {}, not both", first.0, second.0))),
  }
}

fn entity_kinds(values: Option<Vec<String>>) -> Result<Vec<EntityKind>, ToolError> {
  values.unwrap_or_default().iter().map(|value| kinds(Some(value)).map(|mut kind| kind.remove(0))).collect()
}

fn required(name: &str, value: Option<String>) -> Result<String, ToolError> {
  let value = value.ok_or_else(|| ToolError::invalid(format!("{name} is missing")))?;
  non_empty(name, &value)?;
  Ok(value)
}

impl SymbolArgs {
  fn given(&self) -> [(&'static str, bool); 13] {
    [
      ("target", self.target.is_some()),
      ("path", self.path.is_some()),
      ("depth", self.depth.is_some()),
      ("body", self.body.is_some()),
      ("substring", self.substring.is_some()),
      ("kinds", self.kinds.is_some()),
      ("exclude_kinds", self.exclude_kinds.is_some()),
      ("max_matches", self.max_matches.is_some()),
      ("regex", self.regex.is_some()),
      ("source", self.source.is_some()),
      ("context_lines", self.context_lines.is_some()),
      ("lines", self.lines.is_some()),
      ("min_severity", self.min_severity.is_some()),
    ]
  }
}

pub(crate) fn symbol_plan(args: SymbolArgs) -> Result<SymbolPlan, ToolError> {
  let mode = args.mode.unwrap_or(SymbolMode::Info);
  let subject = format!("mode {}", mode.name());
  let given = args.given();
  match mode {
    SymbolMode::Info => {
      takes(&subject, &["target"], &["body"], &given)?;
      let mut request = SymbolRequest::new(required("target", args.target)?);
      request.body = args.body.unwrap_or(false);
      Ok(SymbolPlan::Info(request))
    }
    SymbolMode::Find => {
      takes(
        &subject,
        &["target"],
        &["path", "depth", "body", "substring", "kinds", "exclude_kinds", "max_matches"],
        &given,
      )?;
      let mut request = FindRequest::new(required("target", args.target)?);
      request.path = args.path;
      request.depth = args.depth.unwrap_or(request.depth);
      request.body = args.body.unwrap_or(false);
      if args.substring == Some(true) {
        request.name_match = NameMatch::Substring;
      }
      request.kinds = entity_kinds(args.kinds)?;
      request.exclude_kinds = entity_kinds(args.exclude_kinds)?;
      request.max_matches = args.max_matches.unwrap_or(request.max_matches);
      Ok(SymbolPlan::Find(request))
    }
    SymbolMode::Outline => {
      takes_either(&subject, ["path", "target"], &["depth", "source"], &given)?;
      let mut request = OutlineRequest::new(one_of(&subject, ("path", args.path), ("target", args.target))?);
      request.depth = args.depth.unwrap_or(request.depth);
      request.source = read_source(args.source, atlas_ide::ReadSource::Graph);
      Ok(SymbolPlan::Outline(request))
    }
    SymbolMode::Source => {
      takes(&subject, &["target"], &["context_lines"], &given)?;
      let mut request = ReadRequest::new(required("target", args.target)?);
      request.context = context_lines(args.context_lines)?;
      Ok(SymbolPlan::Source(request))
    }
    SymbolMode::References => {
      takes(&subject, &["target"], &["source"], &given)?;
      let mut request = ReferencesRequest::new(required("target", args.target)?);
      request.source = read_source(args.source, atlas_ide::ReadSource::Lsp);
      Ok(SymbolPlan::References(request))
    }
    SymbolMode::Declaration => match args.regex {
      Some(regex) => {
        takes(&subject, &["path", "regex"], &[], &given)?;
        non_empty("regex", &regex)?;
        Ok(SymbolPlan::Declaration(DeclarationRequest::Regex {
          path: required("path", args.path)?,
          regex,
        }))
      }
      None => {
        takes(&subject, &["target"], &[], &given).map_err(|error| {
          let detail = error.to_string();
          ToolError::invalid(format!(
            "mode declaration takes target, or path with regex; {}",
            detail.split_once("; ").map_or("", |(_, rest)| rest)
          ))
        })?;
        Ok(SymbolPlan::Declaration(DeclarationRequest::Position {
          target: required("target", args.target)?,
        }))
      }
    },
    SymbolMode::Implementations => {
      takes(&subject, &["target"], &[], &given)?;
      Ok(SymbolPlan::Implementations(required("target", args.target)?))
    }
    SymbolMode::Hierarchy => {
      takes(&subject, &["target"], &["depth"], &given)?;
      let mut request = HierarchyRequest::new(required("target", args.target)?);
      request.depth = args.depth.unwrap_or(request.depth);
      Ok(SymbolPlan::Hierarchy(request))
    }
    SymbolMode::Diagnostics => {
      takes_either(&subject, ["path", "target"], &["lines", "min_severity"], &given)?;
      let mut request = DiagnosticsRequest::new(one_of(&subject, ("path", args.path), ("target", args.target))?);
      request.lines = args.lines.as_deref().map(LineRange::parse).transpose()?;
      if let Some(severity) = args.min_severity {
        request.min_severity = severity.into();
      }
      Ok(SymbolPlan::Diagnostics(request))
    }
  }
}

fn read_source(source: Option<ReadSource>, default: atlas_ide::ReadSource) -> atlas_ide::ReadSource {
  match source {
    Some(ReadSource::Graph) => atlas_ide::ReadSource::Graph,
    Some(ReadSource::Lsp) => atlas_ide::ReadSource::Lsp,
    None => default,
  }
}

impl From<Severity> for atlas_ide::Severity {
  fn from(severity: Severity) -> Self {
    match severity {
      Severity::Error => Self::Error,
      Severity::Warning => Self::Warning,
      Severity::Info => Self::Info,
      Severity::Hint => Self::Hint,
    }
  }
}

impl EditArgs {
  fn given(&self) -> [(&'static str, bool); 15] {
    [
      ("target", self.target.is_some()),
      ("path", self.path.is_some()),
      ("new_name", self.new_name.is_some()),
      ("text", self.text.is_some()),
      ("pattern", self.pattern.is_some()),
      ("regex", self.regex.is_some()),
      ("all", self.all.is_some()),
      ("include", self.include.is_some()),
      ("exclude", self.exclude.is_some()),
      ("occurrences", self.occurrences.is_some()),
      ("expected_count", self.expected_count.is_some()),
      ("cross_repo", self.cross_repo.is_some()),
      ("allow_unverified", self.allow_unverified.is_some()),
      ("plan", self.plan.is_some()),
      ("dry_run", self.dry_run.is_some()),
    ]
  }
}

pub(crate) fn edit_request(args: EditArgs) -> Result<EditRequest, ToolError> {
  let given = args.given();
  let (name, required_fields, optional_fields): (&str, &[&str], &[&str]) = match args.operation {
    EditKind::Rename => ("rename", &["target", "new_name"], &["cross_repo", "allow_unverified", "dry_run"]),
    EditKind::ReplaceBody => ("replace_body", &["target", "text"], &["dry_run"]),
    EditKind::InsertBefore => ("insert_before", &["target", "text"], &["dry_run"]),
    EditKind::InsertAfter => ("insert_after", &["target", "text"], &["dry_run"]),
    EditKind::SafeDelete => ("safe_delete", &["target"], &["dry_run"]),
    EditKind::ReplaceContent => ("replace_content", &["path", "pattern", "text"], &["regex", "all", "dry_run"]),
    EditKind::ReplaceInFiles => (
      "replace_in_files",
      &["pattern", "text"],
      &["regex", "include", "exclude", "occurrences", "expected_count", "dry_run"],
    ),
    EditKind::Apply => ("apply", &["plan"], &[]),
  };
  takes(&format!("operation {name}"), required_fields, optional_fields, &given)?;
  let dry_run = args.dry_run.unwrap_or(false);
  let operation = match args.operation {
    EditKind::Rename => {
      let new_name = required("new_name", args.new_name)?;
      EditOperation::Rename {
        target: required("target", args.target)?,
        new_name,
        cross_repo: match args.cross_repo {
          Some(CrossRepoMode::Apply) => CrossRepo::Apply,
          Some(CrossRepoMode::Report) | None => CrossRepo::Report,
        },
        allow_unverified: args.allow_unverified.unwrap_or(false),
      }
    }
    EditKind::ReplaceBody => EditOperation::ReplaceBody {
      target: required("target", args.target)?,
      text: text_of(args.text)?,
    },
    EditKind::InsertBefore => EditOperation::InsertBefore {
      target: required("target", args.target)?,
      text: text_of(args.text)?,
    },
    EditKind::InsertAfter => EditOperation::InsertAfter {
      target: required("target", args.target)?,
      text: text_of(args.text)?,
    },
    EditKind::SafeDelete => EditOperation::SafeDelete {
      target: required("target", args.target)?,
    },
    EditKind::ReplaceContent => EditOperation::ReplaceContent {
      path: required("path", args.path)?,
      pattern: required("pattern", args.pattern)?,
      text: text_of(args.text)?,
      regex: args.regex.unwrap_or(false),
      all: args.all.unwrap_or(false),
    },
    EditKind::ReplaceInFiles => EditOperation::ReplaceInFiles {
      pattern: required("pattern", args.pattern)?,
      text: text_of(args.text)?,
      regex: args.regex.unwrap_or(false),
      include: args.include.unwrap_or_default(),
      exclude: args.exclude.unwrap_or_default(),
      occurrences: args.occurrences,
      expected_count: args.expected_count,
    },
    EditKind::Apply => EditOperation::Apply {
      plan_id: required("plan", args.plan)?,
    },
  };
  Ok(match dry_run {
    true => EditRequest::dry_run(operation),
    false => EditRequest::new(operation),
  })
}

fn text_of(text: Option<String>) -> Result<String, ToolError> {
  text.ok_or_else(|| ToolError::invalid("text is missing"))
}

fn draft_language(value: &str) -> Result<Language, ToolError> {
  match value {
    "rust" => Ok(Language::Rust),
    "ts" => Ok(Language::TypeScript),
    "tsx" => Ok(Language::Tsx),
    "js" => Ok(Language::JavaScript),
    other => Err(ToolError::invalid(format!("language must be rust, ts, tsx or js, got {other:?}"))),
  }
}

pub(crate) fn similar_plan(args: SimilarArgs) -> Result<SimilarPlan, ToolError> {
  let provided = [args.target.is_some(), args.snippet.is_some(), args.code.is_some()]
    .into_iter()
    .filter(|given| *given)
    .count();
  if provided != 1 {
    return Err(ToolError::invalid("provide exactly one of target, snippet or code"));
  }
  match (args.target, args.snippet, args.code) {
    (Some(spec), None, None) | (None, Some(spec), None) => {
      if args.language.is_some() {
        return Err(ToolError::invalid("language only goes with code"));
      }
      non_empty("target or snippet", &spec)?;
      Ok(SimilarPlan::Indexed(SimilarRequest::new(spec)))
    }
    (None, None, Some(code)) => {
      non_empty("code", &code)?;
      let language = args.language.ok_or_else(|| ToolError::invalid("code needs language: rust, ts, tsx or js"))?;
      Ok(SimilarPlan::Draft(SimilarRequest::from_code(draft_language(&language)?, code)))
    }
    _ => Err(ToolError::invalid("provide exactly one of target, snippet or code")),
  }
}
