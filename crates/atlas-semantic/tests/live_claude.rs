use std::time::Duration;

use atlas_core::{ContentHash, EntityId, RelPath, RepoName, Span};
use atlas_semantic::{ClaudeCliProvider, SemanticConfig, SemanticEngine, SemanticTask, SimilarityPair, SourceRef, SymbolSnippet, TaskOutcome};
use atlas_store::{FreshnessPolicy, InferredQuery, Store};

const TO_CENTS: &str = include_str!("../../../fixtures/acme/acme-sdk/src/money.ts");
const PARSE_MONEY: &str = include_str!("../../../fixtures/acme/acme-web/src/utils/currency.ts");

fn snippet(id: u64, repo: &str, path: &str, name: &str, text: &str) -> SymbolSnippet {
  let span = Span::of_text(text, 0, text.len()).unwrap();
  SymbolSnippet {
    entity: EntityId::from_raw(id),
    key: format!("ts:{repo}:{name}"),
    signature: Some(format!("export function {name}({}: string): number", if name == "toCents" { "amount" } else { "input" })),
    snippet: text.to_owned(),
    source: SourceRef {
      repo: RepoName::new(repo).unwrap(),
      path: RelPath::new(path).unwrap(),
      span,
      file_hash: ContentHash::of(text.as_bytes()),
    },
    callers: None,
    test_names: Vec::new(),
  }
}

#[test]
#[ignore = "calls the real `claude` CLI and spends tokens"]
fn live_claude_cli_classifies_to_cents_against_parse_money() {
  let store = Store::open_in_memory().unwrap();
  let provider = ClaudeCliProvider::new("claude-sonnet-5-5");
  let config = SemanticConfig {
    call_timeout: Duration::from_secs(180),
    ..SemanticConfig::default()
  };
  let engine = SemanticEngine::new(&provider, &store, config);
  let pair = SimilarityPair::new(
    snippet(1, "acme-sdk", "src/money.ts", "toCents", TO_CENTS),
    snippet(2, "acme-web", "src/utils/currency.ts", "parseMoney", PARSE_MONEY),
  )
  .unwrap();
  let run = store.write(|tx| tx.begin_run("live")).unwrap();
  let report = engine.run(Some(run), vec![SemanticTask::ClassifySimilarityPair(pair)]).unwrap();
  eprintln!("status: {:?}\nusage: {:?}\ncost: {:?}", report.status, report.usage, report.cost_estimate_usd);
  eprintln!("outcome: {:?}", report.tasks[0].outcome);
  let facts = store
    .read(|tx| {
      tx.inferred_facts(&InferredQuery {
        subject: None,
        relation: None,
        freshness: FreshnessPolicy::IncludeStale,
      })
    })
    .unwrap();
  for fact in &facts {
    eprintln!(
      "fact: {} {:?}\npayload: {}",
      fact.relation,
      fact.certainty,
      serde_json::to_string_pretty(&fact.payload).unwrap()
    );
  }
  assert!(
    matches!(report.tasks[0].outcome, TaskOutcome::Inferred { .. } | TaskOutcome::NoRelation),
    "unexpected outcome {:?}",
    report.tasks[0].outcome
  );
}
