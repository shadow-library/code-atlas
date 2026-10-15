use atlas_project::ToolsSetting;

use crate::error::McpError;

pub const SERVER_NAME: &str = "code-atlas";
pub const BLOCK_BEGIN: &str = "<!-- code-atlas:begin -->";
pub const BLOCK_END: &str = "<!-- code-atlas:end -->";

pub const EDIT_TOOL: &str = "atlas_edit";
pub const TOOLS: [&str; 15] = [
  "atlas_api",
  "atlas_callees",
  "atlas_callers",
  "atlas_context",
  "atlas_edit",
  "atlas_explain",
  "atlas_impact",
  "atlas_overview",
  "atlas_patterns",
  "atlas_search",
  "atlas_similar",
  "atlas_status",
  "atlas_symbol",
  "atlas_tests_for",
  "atlas_unused",
];
pub const CORE_TOOLS: [&str; 6] = ["atlas_context", "atlas_edit", "atlas_impact", "atlas_search", "atlas_status", "atlas_symbol"];
pub const READ_ONLY_TOOLS: [&str; 14] = [
  "atlas_api",
  "atlas_callees",
  "atlas_callers",
  "atlas_context",
  "atlas_explain",
  "atlas_impact",
  "atlas_overview",
  "atlas_patterns",
  "atlas_search",
  "atlas_similar",
  "atlas_status",
  "atlas_symbol",
  "atlas_tests_for",
  "atlas_unused",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProfile {
  name: &'static str,
  tools: Vec<&'static str>,
}

impl ToolProfile {
  pub fn full() -> Self {
    Self {
      name: "full",
      tools: TOOLS.to_vec(),
    }
  }

  pub fn from_setting(setting: &ToolsSetting) -> Result<Self, McpError> {
    let (name, tools) = match setting {
      ToolsSetting::Full => return Ok(Self::full()),
      ToolsSetting::Core => ("core", CORE_TOOLS.to_vec()),
      ToolsSetting::ReadOnly => ("read-only", CORE_TOOLS.into_iter().filter(|tool| *tool != EDIT_TOOL).collect()),
      ToolsSetting::Only(names) => ("custom", known_tools(names)?),
    };
    Ok(Self { name, tools })
  }

  pub fn name(&self) -> &'static str {
    self.name
  }

  pub fn tools(&self) -> &[&'static str] {
    &self.tools
  }

  pub fn contains(&self, tool: &str) -> bool {
    self.tools.contains(&tool)
  }

  pub fn read_only_tools(&self) -> Vec<&'static str> {
    READ_ONLY_TOOLS.into_iter().filter(|tool| self.contains(tool)).collect()
  }

  pub fn refusal(&self, tool: &str) -> Option<String> {
    (TOOLS.contains(&tool) && !self.contains(tool)).then(|| format!("{tool} is not in profile {}; enable with [mcp] tools", self.name))
  }
}

fn shown_tool_name(name: &str) -> String {
  let tool_like = name.starts_with("atlas_") && name.len() <= 64 && name.chars().all(|character| character.is_ascii_lowercase() || character == '_');
  match tool_like {
    true => format!(" {name:?}"),
    false => String::new(),
  }
}

fn known_tools(names: &[String]) -> Result<Vec<&'static str>, McpError> {
  let mut tools = Vec::with_capacity(names.len());
  for name in names {
    let Some(tool) = TOOLS.into_iter().find(|tool| *tool == name.as_str()) else {
      return Err(McpError::UnknownTool {
        shown: shown_tool_name(name),
        known: TOOLS.join(", "),
      });
    };
    tools.push(tool);
  }
  tools.sort_unstable();
  tools.dedup();
  Ok(tools)
}

struct Clause {
  needs: &'static [&'static str],
  text: &'static str,
}

const fn clause(needs: &'static [&'static str], text: &'static str) -> Clause {
  Clause { needs, text }
}

type Rule = &'static [&'static [Clause]];

const INTRO: &str = "Code Atlas answers questions about ONE indexed project (a group of repos) from a knowledge graph kept up to date with the working tree.";

const RULES: [Rule; 8] = [
  &[&[clause(&["atlas_context"], "Start a task with atlas_context: a ranked pack of the code to read.")]],
  &[&[
    clause(
      &["atlas_search", "atlas_similar"],
      "Before writing new code, run atlas_search and atlas_similar to find code to reuse.",
    ),
    clause(&["atlas_search"], "Before writing new code, run atlas_search to find code to reuse."),
    clause(&["atlas_similar"], "Before writing new code, run atlas_similar to find code to reuse."),
  ]],
  &[&[
    clause(
      &["atlas_impact", "atlas_tests_for"],
      "Before changing shared code, run atlas_impact on it; use atlas_tests_for to pick tests.",
    ),
    clause(&["atlas_impact"], "Before changing shared code, run atlas_impact on it."),
    clause(&["atlas_tests_for"], "Use atlas_tests_for to pick the tests to run."),
  ]],
  &[
    &[
      clause(
        &["atlas_symbol"],
        "Read only the code the results list (atlas_symbol mode=source only for code you will not edit yourself; read files you will edit with your own tool).",
      ),
      clause(&[], "Read only the code the results list, with your own tool."),
    ],
    &[clause(
      &[EDIT_TOOL],
      "To rename or rewrite a symbol use atlas_edit (exact across files), not find-and-replace; after it changes a file, re-read that file before editing it yourself.",
    )],
  ],
  &[&[clause(
    &[],
    "Untagged entities and [resolved]/[syntactic]/[compiler]/[declared] relations are reliable but not exhaustive: read the caveats, and an empty result is not proof of absence. [heuristic] and [inferred] relations are leads: verify them in the source before relying on them. A tag after an entity name tags the entity; one in parentheses or in a `verb target [tag]` step tags the relation.",
  )]],
  &[
    &[clause(
      &[],
      "An answer starting with DEGRADED came from an index that could not be fully refreshed: verify it against the files.",
    )],
    &[clause(&["atlas_explain"], "atlas_explain shows the evidence of a relation.")],
  ],
  &[&[
    clause(&["atlas_status"], "After editing, call atlas_status with since_run or re-query; there is no reindex tool."),
    clause(&[], "After editing, re-query; there is no reindex tool."),
  ]],
  &[&[clause(&["atlas_search"], "If a target is not found or is ambiguous, use atlas_search.")]],
];

pub const REMINDER: &str = "This repository is indexed by Code Atlas (the code-atlas MCP server). atlas_search finds where a name, text or structure lives across every repo of the project, atlas_symbol answers definitions and references, and atlas_context ranks the code to read for a task; one of them may answer this faster and more completely than a text search. This reminder appears once per session.";

pub fn instructions(profile: &ToolProfile) -> String {
  let mut text = INTRO.to_owned();
  let rules = RULES.iter().map(|rule| render_rule(rule, profile)).filter(|rule| !rule.is_empty());
  for (index, rule) in rules.enumerate() {
    text.push_str(&format!("\n{}. {rule}", index + 1));
  }
  text
}

fn render_rule(rule: Rule, profile: &ToolProfile) -> String {
  let clauses = rule
    .iter()
    .filter_map(|alternatives| alternatives.iter().find(|clause| clause.needs.iter().all(|tool| profile.contains(tool))));
  clauses.map(|clause| clause.text).collect::<Vec<_>>().join(" ")
}

pub fn repo_block(project: &str, profile: &ToolProfile) -> String {
  format!(
    "{BLOCK_BEGIN}\n## Code Atlas\n\nThis repo is indexed by Code Atlas (project `{project}`), served by the `{SERVER_NAME}` MCP server. Prefer it to grep when a question spans files or repos.\n\n{}\n{BLOCK_END}",
    instructions(profile)
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  const AGENT_INTEGRATION_DOC: &str = include_str!("../../../docs/agent-integration.md");

  fn profile(setting: ToolsSetting) -> ToolProfile {
    ToolProfile::from_setting(&setting).unwrap()
  }

  fn built_in_profiles() -> [ToolProfile; 3] {
    [profile(ToolsSetting::Core), profile(ToolsSetting::ReadOnly), ToolProfile::full()]
  }

  fn named_tools(text: &str) -> Vec<&'static str> {
    TOOLS.into_iter().filter(|tool| text.contains(tool)).collect()
  }

  #[test]
  fn should_render_the_server_instructions_inside_the_repo_block() {
    for profile in built_in_profiles() {
      let block = repo_block("acme", &profile);
      assert!(block.starts_with(&format!("{BLOCK_BEGIN}\n## Code Atlas\n")), "{block}");
      assert!(block.ends_with(&format!("\n{}\n{BLOCK_END}", instructions(&profile))), "{block}");
      assert!(block.contains("(project `acme`)"), "{block}");
    }
  }

  #[test]
  fn should_document_exactly_the_rendered_repo_block() {
    let rendered = format!("```markdown\n{}\n```", repo_block("<project>", &ToolProfile::full()));
    assert!(
      AGENT_INTEGRATION_DOC.contains(&rendered),
      "docs/agent-integration.md must contain the rendered guidance block:\n{rendered}"
    );
  }

  #[test]
  fn should_state_the_editing_rules_every_profile_with_the_edit_tool_must_carry() {
    for profile in built_in_profiles() {
      let text = instructions(&profile);
      assert!(text.contains("atlas_symbol mode=source only for code you will not edit yourself"), "{text}");
      let edit_rules = [
        "To rename or rewrite a symbol use atlas_edit",
        "after it changes a file, re-read that file before editing it yourself",
      ];
      for rule in edit_rules {
        assert_eq!(text.contains(rule), profile.contains(EDIT_TOOL), "{}: {rule}", profile.name());
      }
      assert!(text.lines().count() <= 12, "{text}");
    }
  }

  #[test]
  fn should_name_in_every_clause_only_the_tools_it_needs() {
    for clause in RULES.iter().flat_map(|rule| rule.iter()).flat_map(|alternatives| alternatives.iter()) {
      for tool in named_tools(clause.text) {
        assert!(clause.needs.contains(&tool), "{tool} is not among the needs of: {}", clause.text);
      }
    }
  }

  #[test]
  fn should_name_only_tools_of_the_profile_in_its_instructions() {
    let custom = profile(ToolsSetting::Only(vec!["atlas_callers".to_owned()]));
    for profile in built_in_profiles().into_iter().chain([custom]) {
      let text = instructions(&profile);
      for tool in named_tools(&text) {
        assert!(profile.contains(tool), "{} names {tool}: {text}", profile.name());
      }
    }
  }

  #[test]
  fn should_render_each_built_in_profile_exactly() {
    assert_eq!(
      named_tools(&instructions(&ToolProfile::full())),
      [
        "atlas_context",
        "atlas_edit",
        "atlas_explain",
        "atlas_impact",
        "atlas_search",
        "atlas_similar",
        "atlas_status",
        "atlas_symbol",
        "atlas_tests_for"
      ]
    );
    assert_eq!(named_tools(&instructions(&profile(ToolsSetting::Core))), CORE_TOOLS);
    assert_eq!(
      instructions(&profile(ToolsSetting::ReadOnly)),
      instructions(&profile(ToolsSetting::Core)).replace(
        " To rename or rewrite a symbol use atlas_edit (exact across files), not find-and-replace; after it changes a file, re-read that file before editing it yourself.",
        ""
      )
    );
    let custom = instructions(&profile(ToolsSetting::Only(vec!["atlas_tests_for".to_owned()])));
    assert!(
      custom.contains("\n1. Use atlas_tests_for to pick the tests to run.\n2. Read only the code the results list, with your own tool.\n"),
      "{custom}"
    );
    assert!(custom.ends_with("\n5. After editing, re-query; there is no reindex tool."), "{custom}");
  }

  #[test]
  fn should_resolve_the_profiles_to_their_tools() {
    assert_eq!(profile(ToolsSetting::Core).tools(), CORE_TOOLS);
    assert_eq!(
      profile(ToolsSetting::ReadOnly).tools(),
      ["atlas_context", "atlas_impact", "atlas_search", "atlas_status", "atlas_symbol"]
    );
    assert_eq!(ToolProfile::full().tools(), TOOLS);
    let custom = profile(ToolsSetting::Only(vec!["atlas_status".to_owned(), "atlas_edit".to_owned(), "atlas_status".to_owned()]));
    assert_eq!((custom.name(), custom.tools()), ("custom", ["atlas_edit", "atlas_status"].as_slice()));
  }

  #[test]
  fn should_reject_an_unknown_tool_in_a_list() {
    let error = ToolProfile::from_setting(&ToolsSetting::Only(vec!["atlas_read".to_owned()])).unwrap_err();
    assert_eq!(
      error.to_string(),
      format!(
        "unknown tool \"atlas_read\"; expected the profile core, read-only or full, or a list of tools from: {}",
        TOOLS.join(", ")
      )
    );
  }

  #[test]
  fn should_not_echo_an_unknown_tool_name_that_does_not_look_like_a_tool() {
    let error = ToolProfile::from_setting(&ToolsSetting::Only(vec!["sk-FAKEKEY123".to_owned()])).unwrap_err();
    assert_eq!(
      error.to_string(),
      format!("unknown tool; expected the profile core, read-only or full, or a list of tools from: {}", TOOLS.join(", "))
    );
  }

  #[test]
  fn should_refuse_only_known_tools_outside_the_profile() {
    let core = profile(ToolsSetting::Core);
    assert_eq!(
      core.refusal("atlas_similar").as_deref(),
      Some("atlas_similar is not in profile core; enable with [mcp] tools")
    );
    assert_eq!(core.refusal("atlas_search"), None);
    assert_eq!(core.refusal("atlas_nonexistent"), None);
    assert_eq!(ToolProfile::full().refusal("atlas_similar"), None);
  }

  #[test]
  fn should_list_as_read_only_only_tools_of_the_profile_and_never_the_edit_tool() {
    assert!(!READ_ONLY_TOOLS.contains(&EDIT_TOOL));
    assert!(READ_ONLY_TOOLS.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(TOOLS.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(CORE_TOOLS.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(ToolProfile::full().read_only_tools(), READ_ONLY_TOOLS);
    assert_eq!(
      profile(ToolsSetting::Core).read_only_tools(),
      ["atlas_context", "atlas_impact", "atlas_search", "atlas_status", "atlas_symbol"]
    );
  }
}
