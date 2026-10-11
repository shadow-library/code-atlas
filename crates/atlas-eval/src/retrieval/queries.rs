use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{RetrievalError, RetrievalResult};

pub const QUERIES_FILE: &str = "queries.toml";
pub const NO_RECORDED_QUERY: &str = "no recorded `atlas_context` call: queries.toml is missing or lists none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryVariant {
  Full,
  Agent,
  AgentSeeded,
  FirstLine,
}

impl QueryVariant {
  pub const ALL: [Self; 4] = [Self::Full, Self::Agent, Self::AgentSeeded, Self::FirstLine];

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Full => "full",
      Self::Agent => "agent",
      Self::AgentSeeded => "agent-seeded",
      Self::FirstLine => "first-line",
    }
  }

  pub fn definition(self) -> &'static str {
    match self {
      Self::Full => "the whole task prompt",
      Self::Agent => "the `task` argument of each distinct recorded `atlas_context` call, without its seeds",
      Self::AgentSeeded => "each distinct recorded `atlas_context` call as sent: the `task` argument with its `seeds`",
      Self::FirstLine => "the first sentence of the prompt's first paragraph",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
  pub batch: String,
  pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedQuery {
  pub task: String,
  #[serde(default)]
  pub seeds: Vec<String>,
  pub sent_by: Vec<Provenance>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedQueries {
  #[serde(default)]
  pub no_call: Vec<Provenance>,
  #[serde(default, rename = "query")]
  pub queries: Vec<RecordedQuery>,
}

pub fn parse_recorded_queries(text: &str) -> Result<RecordedQueries, String> {
  let recorded: RecordedQueries = toml::from_str(text).map_err(|error| error.to_string())?;
  let mut seen = BTreeSet::new();
  for query in &recorded.queries {
    if query.task.trim().is_empty() {
      return Err("a query has an empty task".to_owned());
    }
    if query.seeds.iter().any(|seed| seed.trim().is_empty()) {
      return Err(format!("query `{}` has an empty seed", query.task));
    }
    if query.sent_by.is_empty() {
      return Err(format!("query `{}` names no run in sent_by", query.task));
    }
    if !seen.insert((&query.task, &query.seeds)) {
      return Err(format!("query `{}` is listed twice with the same seeds", query.task));
    }
  }
  let runs = recorded.queries.iter().flat_map(|query| &query.sent_by).chain(&recorded.no_call);
  if let Some(run) = runs.into_iter().find(|run| run.batch.trim().is_empty() || run.run.trim().is_empty()) {
    return Err(format!("a run needs a batch and a run id, got batch `{}` and run `{}`", run.batch, run.run));
  }
  Ok(recorded)
}

pub fn load_recorded_queries(task_dir: &Path) -> RetrievalResult<RecordedQueries> {
  let path = task_dir.join(QUERIES_FILE);
  let text = match fs::read_to_string(&path) {
    Ok(text) => text,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(RecordedQueries::default()),
    Err(error) => return Err(RetrievalError::io(&path, error)),
  };
  parse_recorded_queries(&text).map_err(|reason| RetrievalError::InvalidQueries { path, reason })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantQuery {
  pub text: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub seeds: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub sent_by: Vec<Provenance>,
}

impl VariantQuery {
  fn derived(text: String) -> Self {
    Self {
      text,
      seeds: Vec::new(),
      sent_by: Vec::new(),
    }
  }
}

pub fn first_sentence(prompt: &str) -> String {
  let paragraph = prompt.trim().lines().map(str::trim).take_while(|line| !line.is_empty()).collect::<Vec<_>>().join(" ");
  let mut in_code_span = false;
  let mut characters = paragraph.char_indices().peekable();
  while let Some((at, character)) = characters.next() {
    let ends_sentence = matches!(character, '.' | '?' | '!') && !in_code_span && characters.peek().is_none_or(|(_, next)| next.is_whitespace());
    if character == '`' {
      in_code_span = !in_code_span;
    }
    let sentence = paragraph[..at].trim();
    if ends_sentence && sentence.chars().any(char::is_alphanumeric) {
      return sentence.to_owned();
    }
  }
  paragraph
}

pub fn variant_queries(variant: QueryVariant, prompt: &str, recorded: &RecordedQueries) -> Vec<VariantQuery> {
  match variant {
    QueryVariant::Full => vec![VariantQuery::derived(prompt.to_owned())],
    QueryVariant::FirstLine => vec![VariantQuery::derived(first_sentence(prompt))],
    QueryVariant::AgentSeeded => recorded
      .queries
      .iter()
      .map(|query| VariantQuery {
        text: query.task.clone(),
        seeds: query.seeds.clone(),
        sent_by: sorted_runs(query.sent_by.iter()),
      })
      .collect(),
    QueryVariant::Agent => {
      let mut texts: Vec<&str> = Vec::new();
      for query in &recorded.queries {
        if !texts.contains(&query.task.as_str()) {
          texts.push(&query.task);
        }
      }
      texts
        .into_iter()
        .map(|text| VariantQuery {
          text: text.to_owned(),
          seeds: Vec::new(),
          sent_by: sorted_runs(recorded.queries.iter().filter(|query| query.task == text).flat_map(|query| &query.sent_by)),
        })
        .collect()
    }
  }
}

fn sorted_runs<'a>(runs: impl Iterator<Item = &'a Provenance>) -> Vec<Provenance> {
  runs.cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  const RECORDED: &str = r#"
no_call = [{ batch = "b1", run = "t.atlas.r2" }]

[[query]]
task = "Replace the reason String"
sent_by = [{ batch = "b1", run = "t.atlas.r1" }]

[[query]]
task = "Add the tools"
seeds = ["atlas_callers", "crates/atlas-mcp"]
sent_by = [{ batch = "b1", run = "t.atlas.r0" }]

[[query]]
task = "Replace the reason String"
seeds = ["FreshnessReport"]
sent_by = [{ batch = "b2", run = "t.atlas.r0" }, { batch = "b1", run = "t.atlas.r3" }]
"#;

  fn run(batch: &str, run: &str) -> Provenance {
    Provenance {
      batch: batch.to_owned(),
      run: run.to_owned(),
    }
  }

  #[test]
  fn should_parse_recorded_queries_with_their_seeds_and_runs() {
    let recorded = parse_recorded_queries(RECORDED).unwrap();
    assert_eq!(recorded.no_call, vec![run("b1", "t.atlas.r2")]);
    assert_eq!(recorded.queries.len(), 3);
    assert_eq!(
      recorded.queries[1],
      RecordedQuery {
        task: "Add the tools".to_owned(),
        seeds: vec!["atlas_callers".to_owned(), "crates/atlas-mcp".to_owned()],
        sent_by: vec![run("b1", "t.atlas.r0")],
      }
    );
    assert_eq!(recorded.queries[0].seeds, Vec::<String>::new());
  }

  #[test]
  fn should_reject_malformed_query_files() {
    let reject = |text: &str| parse_recorded_queries(text).unwrap_err();
    assert_eq!(reject("[[query]]\ntask = \" \"\nsent_by = [{ batch = \"b\", run = \"r\" }]\n"), "a query has an empty task");
    assert_eq!(reject("[[query]]\ntask = \"q\"\nsent_by = []\n"), "query `q` names no run in sent_by");
    assert_eq!(
      reject("[[query]]\ntask = \"q\"\nseeds = [\"\"]\nsent_by = [{ batch = \"b\", run = \"r\" }]\n"),
      "query `q` has an empty seed"
    );
    assert_eq!(
      reject("[[query]]\ntask = \"q\"\nsent_by = [{ batch = \"b\", run = \"r\" }]\n[[query]]\ntask = \"q\"\nsent_by = [{ batch = \"b\", run = \"s\" }]\n"),
      "query `q` is listed twice with the same seeds"
    );
    assert_eq!(
      reject("no_call = [{ batch = \"\", run = \"r\" }]\n"),
      "a run needs a batch and a run id, got batch `` and run `r`"
    );
    assert!(reject("[[query]]\ntask = \"q\"\nbudget = 1\nsent_by = [{ batch = \"b\", run = \"r\" }]\n").contains("unknown field `budget`"));
  }

  #[test]
  fn should_treat_a_missing_query_file_as_no_recorded_query_and_report_a_broken_one() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load_recorded_queries(dir.path()).unwrap(), RecordedQueries::default());
    fs::write(dir.path().join(QUERIES_FILE), "[[query]]\ntask = \"\"\nsent_by = []\n").unwrap();
    let error = load_recorded_queries(dir.path()).unwrap_err();
    assert_eq!(error.to_string(), format!("invalid {}: a query has an empty task", dir.path().join(QUERIES_FILE).display()));
  }

  #[test]
  fn should_build_no_agent_query_without_a_recorded_call() {
    let recorded = parse_recorded_queries("no_call = [{ batch = \"b1\", run = \"t.atlas.r0\" }]\n").unwrap();
    assert_eq!(variant_queries(QueryVariant::Agent, "Prompt. More.", &recorded), Vec::new());
    assert_eq!(variant_queries(QueryVariant::AgentSeeded, "Prompt. More.", &recorded), Vec::new());
    assert_eq!(
      variant_queries(QueryVariant::Full, "Prompt. More.", &recorded),
      vec![VariantQuery::derived("Prompt. More.".to_owned())]
    );
    assert_eq!(
      variant_queries(QueryVariant::FirstLine, "Prompt. More.", &recorded),
      vec![VariantQuery::derived("Prompt".to_owned())]
    );
  }

  #[test]
  fn should_merge_agent_queries_by_task_text_and_keep_seeded_calls_apart() {
    let recorded = parse_recorded_queries(RECORDED).unwrap();
    assert_eq!(
      variant_queries(QueryVariant::Agent, "p", &recorded),
      vec![
        VariantQuery {
          text: "Replace the reason String".to_owned(),
          seeds: Vec::new(),
          sent_by: vec![run("b1", "t.atlas.r1"), run("b1", "t.atlas.r3"), run("b2", "t.atlas.r0")],
        },
        VariantQuery {
          text: "Add the tools".to_owned(),
          seeds: Vec::new(),
          sent_by: vec![run("b1", "t.atlas.r0")],
        },
      ]
    );
    let seeded = variant_queries(QueryVariant::AgentSeeded, "p", &recorded);
    assert_eq!(seeded.len(), 3);
    assert_eq!(
      seeded[2],
      VariantQuery {
        text: "Replace the reason String".to_owned(),
        seeds: vec!["FreshnessReport".to_owned()],
        sent_by: vec![run("b1", "t.atlas.r3"), run("b2", "t.atlas.r0")],
      }
    );
  }

  #[test]
  fn should_cut_the_first_paragraph_at_its_first_sentence_end_outside_code_spans() {
    assert_eq!(
      first_sentence("`UsersService.findOne` answers \"not found\". Every other query works.\n\nSecond paragraph."),
      "`UsersService.findOne` answers \"not found\""
    );
    assert_eq!(
      first_sentence("Agents cannot ask \"what uses this?\" or \"what does this use?\".\n\nThe CLI answers both."),
      "Agents cannot ask \"what uses this?\" or \"what does this use?\""
    );
    assert_eq!(
      first_sentence("Write `--snippet money.rs:1-15. ` and it works! Then more."),
      "Write `--snippet money.rs:1-15. ` and it works"
    );
    assert_eq!(
      first_sentence("\n  Bug report: routes are\n  never linked\n\nDetails. More."),
      "Bug report: routes are never linked"
    );
    assert_eq!(first_sentence("Version 1.2 ships today"), "Version 1.2 ships today");
    assert_eq!(first_sentence("... then it broke. Later."), "... then it broke");
  }
}
