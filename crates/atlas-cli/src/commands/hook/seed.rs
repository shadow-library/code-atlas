use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use atlas_core::RunId;
use atlas_engine::{BarrierMode, Engine, EngineOptions, ExistingBinding, bind_existing_project};
use atlas_mcp::guidance::{SEED_INTRO, ToolProfile, seed_footer};
use atlas_project::{AtlasHome, ConfigEnv, McpFlags, Registry, resolve_mcp};
use atlas_query::{ContextAnswer, ContextItem, ContextRequest, FreshnessState, QueryEngine, context_item_line};
use atlas_store::StoreOptions;
use serde_json::{Value, json};

use crate::commands::mcp::tool_profile;

const USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";
const TIME_LIMIT: Duration = Duration::from_secs(3);
const WAIT_MARGIN: Duration = Duration::from_millis(250);
const CHARS_PER_TOKEN: usize = 4;
const MAX_CONTEXT_CHARS: usize = 9_000;
const GATE_ITEMS: usize = 3;
const GATE_TERMS: usize = 2;
const MIN_PROMPT_CHARS: usize = 16;
const MIN_CONTENT_WORDS: usize = 3;
const MIN_CONTENT_LETTERS: usize = 3;
const PASTE_MARKERS: [&str; 2] = ["<pasted_content", "</pasted_content"];
const CHATTER: &str = "about after afterwards again all also and any approach are before but can commit continue could did does done even for from good great has have \
   heading her his how into just keep last let like look looks made make merge message more much need next nice not now okay only open our out perfect \
   please push revert run short should some sounds step still summarise summarize sure tell thank thanks that the their them then there these they think \
   this today want was were what when where which who why will with would yes you your";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
  Code,
  Plain,
}

#[derive(Debug, PartialEq, Eq)]
struct Prompt {
  kind: PromptKind,
  content: BTreeSet<String>,
  names: BTreeSet<String>,
}

struct Job {
  home: AtlasHome,
  cwd: PathBuf,
  project: Option<String>,
  prompt: String,
  terms: Prompt,
  deadline: Instant,
}

enum Silence {
  NoProject(String),
  OtherCheckout,
  OverlayNotSeeded(String),
  NeverIndexed,
  Stale(String),
  Resynced,
  RunInProgress,
  EmptyPack,
  Unrelated,
  Failed(String),
}

impl Silence {
  fn reason(&self) -> String {
    match self {
      Self::NoProject(reason) => format!("no project: {reason}"),
      Self::OtherCheckout => "the working directory is answered from another checkout".to_owned(),
      Self::OverlayNotSeeded(label) => format!("the worktree overlay {label} is not seeded yet"),
      Self::NeverIndexed => "the project has never been indexed".to_owned(),
      Self::Stale(reason) => format!("the index is not fresh: {reason}"),
      Self::Resynced => "the index changed while the pack was built".to_owned(),
      Self::RunInProgress => "an index run started while the pack was built".to_owned(),
      Self::EmptyPack => "the pack is empty".to_owned(),
      Self::Unrelated => format!("no item is named in the prompt and none of the top {GATE_ITEMS} matches {GATE_TERMS} of its words"),
      Self::Failed(reason) => format!("failed: {reason}"),
    }
  }
}

pub(super) struct SessionMarkers<Seen, Claim> {
  pub seen: Seen,
  pub claim: Claim,
}

pub(super) fn seed(home: &AtlasHome, project: Option<&str>, event: &Value, markers: &SessionMarkers<impl Fn(&str) -> bool, impl Fn(&str) -> bool>) -> Option<Value> {
  let started = Instant::now();
  let Some((prompt, cwd)) = user_prompt(event) else {
    tracing::debug!("no seed: not a UserPromptSubmit event with a prompt and an absolute cwd");
    return None;
  };
  let prompt_chars = prompt.chars().count();
  let Some(terms) = classify(prompt) else {
    tracing::debug!(prompt_chars, "no seed: the prompt is too short or has no code-like content");
    return None;
  };
  let session = event.get("session_id").and_then(Value::as_str).filter(|session| !session.is_empty());
  if terms.kind == PromptKind::Plain && session.is_none_or(&markers.seen) {
    tracing::debug!(prompt_chars, "no seed: a plain-English prompt after the first of its session");
    return None;
  }
  let job = Job {
    home: home.clone(),
    cwd,
    project: project.map(str::to_owned),
    prompt: prompt.to_owned(),
    terms,
    deadline: started + TIME_LIMIT,
  };
  let (sender, receiver) = mpsc::channel();
  let kind = job.terms.kind;
  let spawned = thread::Builder::new().name("seed".to_owned()).spawn(move || {
    let _ = sender.send(job.run());
  });
  if let Err(error) = spawned {
    tracing::debug!(prompt_chars, error = %error, "no seed: cannot start the worker");
    return None;
  }
  let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
  match receiver.recv_timeout(TIME_LIMIT) {
    Ok(Ok(context)) => {
      let claimed = session.is_some_and(&markers.claim);
      if kind == PromptKind::Plain && !claimed {
        tracing::debug!(prompt_chars, "no seed: another hook claimed the first plain-English prompt of the session");
        return None;
      }
      tracing::info!(prompt_chars, elapsed_ms = elapsed_ms(), context_chars = context.len(), "seeded the prompt");
      Some(json!({ "hookSpecificOutput": { "hookEventName": USER_PROMPT_SUBMIT, "additionalContext": context } }))
    }
    Ok(Err(silence)) => {
      tracing::info!(prompt_chars, elapsed_ms = elapsed_ms(), reason = %silence.reason(), "no seed");
      None
    }
    Err(error) => {
      tracing::info!(prompt_chars, elapsed_ms = elapsed_ms(), limit_ms = TIME_LIMIT.as_millis() as u64, error = %error, "no seed: the time limit passed");
      None
    }
  }
}

fn user_prompt(event: &Value) -> Option<(&str, PathBuf)> {
  (event.get("hook_event_name")?.as_str()? == USER_PROMPT_SUBMIT).then_some(())?;
  let prompt = event.get("prompt")?.as_str()?;
  let cwd = PathBuf::from(event.get("cwd")?.as_str()?);
  cwd.is_absolute().then_some((prompt, cwd))
}

fn classify(prompt: &str) -> Option<Prompt> {
  let prompt = without_paste_markers(prompt);
  let prompt = prompt.trim();
  if prompt.chars().count() < MIN_PROMPT_CHARS {
    return None;
  }
  let content = content_words(prompt);
  let names = code_names(prompt);
  let code = prompt.contains('`')
    || prompt
      .split_whitespace()
      .map(|word| word.trim_matches(|c: char| ",;:!?\"'<>[]{}".contains(c)))
      .any(code_like);
  match (code, content.len() >= MIN_CONTENT_WORDS) {
    (true, _) => Some(Prompt {
      kind: PromptKind::Code,
      content,
      names,
    }),
    (false, true) => Some(Prompt {
      kind: PromptKind::Plain,
      content,
      names,
    }),
    (false, false) => None,
  }
}

fn code_names(prompt: &str) -> BTreeSet<String> {
  let spans = prompt.split('`').skip(1).step_by(2);
  let words = prompt
    .split_whitespace()
    .map(|word| word.trim_matches(|c: char| ",;:!?\"'<>[]{}`".contains(c)))
    .filter(|word| code_like(word));
  spans
    .chain(words)
    .map(|token| token.trim().trim_end_matches(['.', ',']).trim_end_matches("()"))
    .filter(|token| !token.is_empty())
    .map(str::to_owned)
    .collect()
}

fn segments(path: &str) -> Vec<String> {
  path
    .replace("->", "::")
    .replace('.', "::")
    .split("::")
    .filter(|segment| !segment.is_empty())
    .map(str::to_lowercase)
    .collect()
}

fn specific_name(name: &str) -> bool {
  let chars: Vec<char> = name.chars().collect();
  let snake = chars
    .windows(3)
    .any(|window| window[1] == '_' && window[0].is_alphanumeric() && window[2].is_alphanumeric());
  let interior_upper = chars.iter().skip(1).any(|c| c.is_uppercase()) && chars.iter().any(|c| c.is_lowercase());
  snake || interior_upper
}

fn names_item(names: &BTreeSet<String>, short: &str) -> bool {
  let item = segments(short);
  let Some((last, item_qualifier)) = item.split_last() else { return false };
  names.iter().any(|token| {
    let path = segments(token);
    let Some((name, qualifier)) = path.split_last() else { return false };
    name == last
      && match qualifier.is_empty() {
        true => specific_name(token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')),
        false => !item_qualifier.is_empty() && (item_qualifier.ends_with(qualifier) || qualifier.ends_with(item_qualifier)),
      }
  })
}

fn content_words(prompt: &str) -> BTreeSet<String> {
  prompt
    .split(|c: char| !c.is_alphanumeric())
    .flat_map(split_camel_case)
    .map(|word| word.to_lowercase())
    .filter(|word| word.chars().count() >= MIN_CONTENT_LETTERS && word.chars().all(char::is_alphabetic) && !CHATTER.split_whitespace().any(|chatter| chatter == word))
    .collect()
}

fn split_camel_case(word: &str) -> Vec<&str> {
  let mut parts = Vec::new();
  let mut start = 0;
  let mut previous: Option<char> = None;
  for (index, character) in word.char_indices() {
    if character.is_uppercase() && previous.is_some_and(char::is_lowercase) {
      parts.push(&word[start..index]);
      start = index;
    }
    previous = Some(character);
  }
  parts.push(&word[start..]);
  parts
}

fn without_paste_markers(prompt: &str) -> String {
  prompt
    .lines()
    .filter(|line| !PASTE_MARKERS.iter().any(|marker| line.trim_start().starts_with(marker)))
    .collect::<Vec<_>>()
    .join("\n")
}

fn code_like(word: &str) -> bool {
  let word = word.trim_end_matches(['.', ',']);
  let chars: Vec<char> = word.chars().collect();
  let between = |separator: char| {
    chars
      .windows(3)
      .any(|window| window[1] == separator && window[0].is_alphanumeric() && window[2].is_alphanumeric())
  };
  let dotted = word.split('.').filter(|part| part.chars().any(char::is_alphanumeric)).collect::<Vec<_>>();
  let qualified = dotted.len() > 1 && dotted.iter().all(|part| part.chars().count() > 1);
  let path = word.matches('/').count() >= 2 || (word.contains('/') && qualified);
  let camel = chars.windows(2).any(|pair| pair[0].is_lowercase() && pair[1].is_uppercase());
  let call = chars.windows(2).any(|pair| pair[0].is_alphanumeric() && pair[1] == '(');
  between('_') || word.contains("::") || word.contains("->") || qualified || path || camel || call
}

impl Job {
  fn run(self) -> Result<String, Silence> {
    let registry = Registry::load(&self.home).map_err(|error| Silence::Failed(error.to_string()))?;
    let store = StoreOptions {
      lock_timeout: self.remaining(),
      busy_timeout: self.remaining(),
      ..StoreOptions::default()
    };
    let bound = match bind_existing_project(&self.home, &registry, &self.cwd, self.project.as_deref(), &store) {
      Ok(ExistingBinding::Bound(bound)) => *bound,
      Ok(ExistingBinding::OverlayNotSeeded { label }) => return Err(Silence::OverlayNotSeeded(label)),
      Err(error) => return Err(Silence::NoProject(error.to_string())),
    };
    if bound.binding.caveat().is_some() {
      return Err(Silence::OtherCheckout);
    }
    if !bound.engine_home.db_path(&bound.project.name).is_file() {
      return Err(Silence::NeverIndexed);
    }
    let config = registry.user_config();
    let profile = tool_profile(&resolve_mcp(config, &ConfigEnv::default(), McpFlags::default())).map_err(|error| Silence::Failed(error.to_string()))?;
    let options = EngineOptions {
      store: StoreOptions {
        lock_timeout: self.remaining(),
        busy_timeout: self.remaining(),
        ..store
      },
      barrier: BarrierMode::CheckOnly,
      ..EngineOptions::default()
    };
    let engine = Arc::new(Engine::open(&bound.project, &bound.engine_home, options).map_err(|error| Silence::Failed(error.to_string()))?);
    let response = QueryEngine::new(Arc::clone(&engine))
      .context(&ContextRequest::new(self.prompt))
      .map_err(|error| Silence::Failed(error.to_string()))?;
    let freshness = &response.freshness;
    let Some(run) = freshness.run else { return Err(Silence::NeverIndexed) };
    if let FreshnessState::Degraded { reason, .. } = &freshness.state {
      return Err(Silence::Stale(reason.clone()));
    }
    let generation = engine.persisted_generation().map_err(|error| Silence::Failed(error.to_string()))?;
    if !freshness.notes.is_empty() || generation != Some(RunId(run)) {
      return Err(Silence::Resynced);
    }
    if engine.index_run_in_progress().map_err(|error| Silence::Failed(error.to_string()))? {
      return Err(Silence::RunInProgress);
    }
    relevant(&response.answer, &self.terms)?;
    render(&response.answer, config.hooks().seed_budget, &profile).ok_or(Silence::EmptyPack)
  }

  fn remaining(&self) -> Duration {
    self.deadline.saturating_duration_since(Instant::now()).saturating_sub(WAIT_MARGIN)
  }
}

fn relevant(answer: &ContextAnswer, prompt: &Prompt) -> Result<(), Silence> {
  let code: Vec<&ContextItem> = answer.must_read.iter().chain(&answer.skim).collect();
  if code.is_empty() && answer.tests.is_empty() {
    return Err(Silence::EmptyPack);
  }
  if code.iter().any(|item| item.named) {
    return Ok(());
  }
  let names_prompt = |item: &ContextItem| prompt.kind == PromptKind::Code && names_item(&prompt.names, &item.entity.short);
  let matches_prompt = |item: &ContextItem| {
    let matched = item.search_terms.iter().filter(|term| prompt.content.iter().any(|word| word.starts_with(term.as_str())));
    matched.collect::<BTreeSet<_>>().len() >= GATE_TERMS
  };
  match code.iter().take(GATE_ITEMS).any(|item| names_prompt(item) || matches_prompt(item)) {
    true => Ok(()),
    false => Err(Silence::Unrelated),
  }
}

fn render(answer: &ContextAnswer, budget_tokens: usize, profile: &ToolProfile) -> Option<String> {
  let limit = budget_tokens.saturating_mul(CHARS_PER_TOKEN).min(MAX_CONTEXT_CHARS);
  let footer = seed_footer(profile).map(|footer| format!("\n{footer}")).unwrap_or_default();
  let mut text = SEED_INTRO.to_owned();
  let mut listed = 0;
  let sections: [(&str, &[ContextItem]); 3] = [("Read first:", &answer.must_read), ("Also relevant:", &answer.skim), ("Tests:", &answer.tests)];
  'sections: for (title, items) in sections {
    for (index, item) in items.iter().enumerate() {
      let heading = if index == 0 { format!("\n{title}") } else { String::new() };
      let line = format!("{heading}\n- {}", context_item_line(item));
      if text.len() + line.len() + footer.len() > limit {
        break 'sections;
      }
      text.push_str(&line);
      listed += 1;
    }
  }
  (listed > 0).then(|| text + &footer)
}

#[cfg(test)]
mod tests {
  use atlas_project::ToolsSetting;

  use super::*;

  #[test]
  fn should_read_only_user_prompt_events_with_a_prompt_and_an_absolute_cwd() {
    let event = |name: &str, prompt: Value, cwd: Value| json!({"session_id": "s", "hook_event_name": name, "prompt": prompt, "cwd": cwd});
    assert_eq!(
      user_prompt(&event("UserPromptSubmit", json!("fix it"), json!("/src/app"))),
      Some(("fix it", PathBuf::from("/src/app")))
    );
    assert_eq!(user_prompt(&event("PreToolUse", json!("fix it"), json!("/src/app"))), None);
    assert_eq!(user_prompt(&event("UserPromptSubmit", json!(3), json!("/src/app"))), None);
    assert_eq!(user_prompt(&event("UserPromptSubmit", json!("fix it"), json!("src/app"))), None);
    assert_eq!(user_prompt(&json!({"hook_event_name": "UserPromptSubmit", "cwd": "/src"})), None);
  }

  fn kind(prompt: &str) -> Option<PromptKind> {
    classify(prompt).map(|prompt| prompt.kind)
  }

  #[test]
  fn should_tell_code_prompts_from_plain_english_ones_and_skip_chatter() {
    for prompt in [
      "Fix the bug in UsersService.findOne",
      "why does first_in_session fire twice",
      "rename `answer` everywhere",
      "look at crates/atlas-cli/src/main.rs please",
      "what calls parse_args() here?",
      "make Engine::open fail on a bad schema",
    ] {
      assert_eq!(kind(prompt), Some(PromptKind::Code), "{prompt}");
    }
    for prompt in [
      "Add rate limiting to the login endpoint",
      "the app crashes on startup",
      "<pasted_content id=\"1\">\nError: refund total is negative\n</pasted_content id=\"1\">",
    ] {
      assert_eq!(kind(prompt), Some(PromptKind::Plain), "{prompt}");
    }
    for prompt in [
      "yes go ahead",
      "continue with the next step",
      "looks good, thanks! now run the tests",
      "why does the build fail?",
      "e.g. like this, ok?",
      "                 ",
      "look at this:\n<pasted_content id=\"7\">\nok sure\n</pasted_content id=\"7\">",
    ] {
      assert_eq!(kind(prompt), None, "{prompt}");
    }
  }

  #[test]
  fn should_split_identifiers_into_content_words() {
    let words: Vec<String> = content_words("Fix UsersService.findOne in the parse_order helper, ok?").into_iter().collect();
    assert_eq!(words, ["find", "fix", "helper", "one", "order", "parse", "service", "users"]);
  }

  #[test]
  fn should_pass_a_pack_that_names_prompt_code_or_whose_top_items_match_two_prompt_words() {
    let hit = |terms: &[&str], named: bool| ContextItem {
      named,
      search_terms: terms.iter().map(|term| (*term).to_owned()).collect(),
      ..item("f", "src/a.rs", (1, 2), "search hit")
    };
    let content = classify("users get logged out after five minutes").unwrap();
    let pass = |skim: Vec<ContextItem>| relevant(&answer(Vec::new(), skim), &content).is_ok();
    assert!(pass(vec![hit(&[], true)]));
    assert!(pass(vec![hit(&["user", "log"], false)]));
    assert!(!pass(vec![hit(&["user"], false), hit(&["minut"], false)]));
    assert!(!pass(vec![
      hit(&["user"], false),
      hit(&["log"], false),
      hit(&["five"], false),
      hit(&["user", "log"], false)
    ]));
    assert!(!pass(vec![hit(&["user", "hour"], false)]));
    assert!(matches!(relevant(&answer(Vec::new(), Vec::new()), &content), Err(Silence::EmptyPack)));
  }

  fn search_hit(short: &str) -> ContextItem {
    ContextItem {
      search_terms: vec!["unrel".to_owned()],
      ..item(short, "src/a.rs", (1, 2), "search hit on unrelated")
    }
  }

  #[test]
  fn should_pass_a_code_prompt_whose_token_equals_the_short_name_of_a_top_three_item() {
    let pack = |names: &[&str]| {
      answer(
        names.iter().take(1).map(|name| search_hit(name)).collect(),
        names.iter().skip(1).map(|name| search_hit(name)).collect(),
      )
    };
    let passes = |prompt: &str, names: &[&str]| relevant(&pack(names), &classify(prompt).unwrap()).is_ok();
    let flush = ["Cache::flush_pending", "Cache::len", "Cache::clear"];
    assert!(passes("what does `flush_pending` return when nothing changed?", &flush));
    assert!(passes("what does flush_pending() return when nothing changed?", &flush));
    assert!(passes("what does Cache::flush_pending return when nothing changed?", &flush));
    assert!(passes("what does `FLUSH_PENDING` return when nothing changed?", &flush));
    assert!(passes("where is `REFRESH_DEFERRED` checked, exactly?", &["REFRESH_DEFERRED", "Cache::len"]));
    assert!(passes("where is REFRESH_DEFERRED checked, exactly?", &["Cache::len", "Cache::clear", "REFRESH_DEFERRED"]));
    assert!(!passes(
      "where is REFRESH_DEFERRED checked, exactly?",
      &["Cache::len", "Cache::clear", "Cache::drop", "REFRESH_DEFERRED"]
    ));
    assert!(!passes("what does `flush_pending_all` return when nothing changed?", &flush));
    assert!(!passes("what does the flush_pending_x helper return?", &flush));
    assert!(!passes("what does `Cache` do when the flush pending list is empty?", &["Registry::len", "Registry::clear"]));
  }

  #[test]
  fn should_pass_only_specific_identifiers_and_qualified_names_never_a_common_word() {
    let passes = |prompt: &str, short: &str| relevant(&answer(vec![search_hit(short)], Vec::new()), &classify(prompt).unwrap()).is_ok();
    for (prompt, short) in [
      ("use `map` instead", "map"),
      ("can you call `run()` again", "run"),
      ("rename it to `main` please", "main"),
      ("remove the `fmt` step", "fmt"),
      ("should it be `default()` instead", "default"),
      ("should it be `default()` instead", "Config::default"),
      ("can you call `Cache` again", "Cache"),
      ("use `Other::flush` instead", "Cache::flush"),
    ] {
      assert!(!passes(prompt, short), "{prompt} / {short}");
    }
    for (prompt, short) in [
      ("what does `Cache::flush` do now", "Cache::flush"),
      ("what does `Cache.flush` do now", "Cache::flush"),
      ("what does `store::Cache::flush` do now", "Cache::flush"),
      ("what does `flushPending` do now", "flushPending"),
      ("what does `flush_pending` do now", "Cache::flush_pending"),
      ("what does `persist_similarity` do now", "Engine::persist_similarity"),
    ] {
      assert!(passes(prompt, short), "{prompt} / {short}");
    }
  }

  #[test]
  fn should_not_let_a_plain_english_word_equal_to_an_item_name_pass_the_gate() {
    let pack = answer(vec![search_hit("flush")], Vec::new());
    assert!(matches!(
      relevant(&pack, &classify("please flush the pending entries when the cache closes").unwrap()),
      Err(Silence::Unrelated)
    ));
  }

  fn item(name: &str, path: &str, lines: (u32, u32), why: &str) -> ContextItem {
    ContextItem {
      entity: atlas_query::EntityRef {
        key: format!("sym:r:{path}#{name}"),
        id: "0".to_owned(),
        short: name.to_owned(),
        kind: "function".to_owned(),
        location: Some(atlas_query::Location {
          repo: "r".to_owned(),
          path: path.to_owned(),
          line: Some(lines.0),
          column: None,
        }),
        certainty: None,
      },
      score: 1.0,
      why: why.to_owned(),
      certainty: None,
      via: None,
      uncertain: false,
      signature: None,
      doc: None,
      skeleton: Vec::new(),
      lines: Some(atlas_query::LineRange {
        start: lines.0,
        end: lines.1,
        basis: atlas_query::RangeBasis::Symbol,
      }),
      excerpt: None,
      body: None,
      read_directly: false,
      run: None,
      named: false,
      search_terms: Vec::new(),
    }
  }

  fn answer(must_read: Vec<ContextItem>, skim: Vec<ContextItem>) -> ContextAnswer {
    ContextAnswer {
      task: String::new(),
      budget: 0,
      estimated_tokens: 0,
      must_read,
      skim,
      patterns: Vec::new(),
      reuse: Vec::new(),
      impact: Vec::new(),
      tests: Vec::new(),
      uncertain: Vec::new(),
      summaries: Vec::new(),
      omitted: atlas_query::Listing::default(),
      omitted_by_reason: std::collections::BTreeMap::new(),
      explain: None,
      caveats: Vec::new(),
    }
  }

  #[test]
  fn should_render_a_ranked_list_with_line_ranges_reasons_and_the_profile_footer() {
    let pack = answer(
      vec![item("findOne", "src/users.ts", (31, 35), "named in the task")],
      vec![item("toCents", "src/money.ts", (1, 7), "called by findOne [resolved]")],
    );
    let full = ToolProfile::full();
    let expected = format!(
      "{SEED_INTRO}\nRead first:\n- findOne function r:src/users.ts:31-35  why: named in the task\nAlso relevant:\n- toCents function r:src/money.ts:1-7  why: called by findOne [resolved]\n{}",
      seed_footer(&full).unwrap()
    );
    assert_eq!(render(&pack, 1_200, &full).as_deref(), Some(expected.as_str()));
    let status_only = ToolProfile::from_setting(&ToolsSetting::Only(vec!["atlas_status".to_owned()])).unwrap();
    assert!(!render(&pack, 1_200, &status_only).unwrap().contains("atlas_"));
  }

  #[test]
  fn should_stop_at_the_budget_and_say_nothing_for_an_empty_pack() {
    let many: Vec<ContextItem> = (0..200).map(|index| item(&format!("f{index}"), "src/a.rs", (1, 2), "search hit")).collect();
    let rendered = render(&answer(many, Vec::new()), 200, &ToolProfile::full()).unwrap();
    assert!(rendered.len() <= 800, "{}", rendered.len());
    assert!(rendered.contains("- f0 function"), "{rendered}");
    let largest = render(
      &answer((0..2_000).map(|index| item(&format!("f{index}"), "src/a.rs", (1, 2), "x")).collect(), Vec::new()),
      2_000,
      &ToolProfile::full(),
    )
    .unwrap();
    assert!(largest.len() <= MAX_CONTEXT_CHARS, "{}", largest.len());
    assert_eq!(render(&answer(Vec::new(), Vec::new()), 1_200, &ToolProfile::full()), None);
  }
}
