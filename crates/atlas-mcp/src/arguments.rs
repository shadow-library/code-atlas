use atlas_core::EntityKind;
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
      #[schemars(description = "Output format: 'text' (compact, default) or 'json' (structured)")]
      pub format: Option<Format>,
    }
  };
}

arguments!(ContextArgs {
  #[schemars(description = "What you are about to do, in your own words")]
  task: String,
  #[schemars(description = "Approximate size budget of the pack in tokens, 500-32000; larger packs include more bodies")]
  budget: Option<usize>,
  #[schemars(description = "Optional symbols, files or keys to anchor the pack on")]
  seeds: Option<Vec<String>>,
});

arguments!(SearchArgs {
  #[schemars(description = "Natural-language or identifier text, e.g. 'convert currency string to cents'")]
  query: String,
  #[schemars(description = "Restrict to an entity kind such as function, method, class, struct, trait, api_endpoint")]
  kind: Option<String>,
  #[schemars(description = "Restrict to one repo of the project")]
  repo: Option<String>,
  #[schemars(description = "Maximum hits per section, 1-200")]
  limit: Option<usize>,
});

arguments!(SymbolArgs {
  #[schemars(description = "Name (UsersService.findOne), [repo:]path[:line], 'GET /users/{id}' or full key")]
  target: String,
  #[schemars(description = "Include the symbol body (default false)")]
  body: Option<bool>,
});

arguments!(TraverseArgs {
  #[schemars(description = "Function or method: name, [repo:]path[:line] or full key")]
  target: String,
  #[schemars(description = "Hops to follow, 1-6 (default 1)")]
  depth: Option<u32>,
});

arguments!(ImpactArgs {
  #[schemars(description = "One or more symbols, files, endpoints or keys that will change")]
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
  #[schemars(description = "Line range inside an indexed symbol: '[repo:]path:START-END' (not code text)")]
  snippet: Option<String>,
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

arguments!(ReadArgs {
  #[schemars(description = "Symbol: name, [repo:]path[:line] or full key")]
  target: String,
  #[schemars(description = "Extra lines before and after the symbol, 0-100 (default 0)")]
  context_lines: Option<u32>,
});

arguments!(NoArgs {});

arguments!(ChangesArgs {
  #[schemars(description = "Only runs newer than this run id; default is the latest run")]
  since_run: Option<i64>,
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
