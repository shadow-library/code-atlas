pub const SERVER_NAME: &str = "code-atlas";
pub const BLOCK_BEGIN: &str = "<!-- code-atlas:begin -->";
pub const BLOCK_END: &str = "<!-- code-atlas:end -->";

pub const EDIT_TOOL: &str = "atlas_edit";
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

const INTRO: &str =
  "Code Atlas answers questions about ONE indexed project (a group of repos) from a knowledge graph; every call first brings the index up to date with the working tree.";

const RULES: [&str; 8] = [
  "Start a task with atlas_context: a ranked, budgeted pack of code to read, with reasons and omissions.",
  "Before writing new code, run atlas_search and atlas_similar to find something to reuse.",
  "Before changing shared code, run atlas_impact on it; use atlas_tests_for to pick tests.",
  "Read only the code the results list (atlas_symbol mode=source only for code you will not edit yourself; read files you will edit with your own tool). To rename or rewrite a symbol use atlas_edit (exact across files), not find-and-replace; after it changes a file, re-read that file before editing it yourself.",
  "Untagged entities and [resolved]/[syntactic]/[compiler]/[declared] relations are reliable but not exhaustive; always read the caveats. An empty result is not proof of absence (unresolved references, files that failed to parse, hidden low-confidence relations). [heuristic] and [inferred] relations are leads: verify them in the source before relying on them; entities carry the same tags. A tag after an entity name tags the entity; a tag in parentheses or in a `verb target [tag]` step tags the relation.",
  "An answer whose first line starts with DEGRADED came from an index that could not be fully refreshed: verify it against the files. Run atlas_explain on a relation that drives a decision to see its evidence.",
  "After editing, call atlas_status with since_run or re-query; freshness is automatic, there is no reindex tool.",
  "Targets: a name (UsersService.findOne), a [repo:]path[:line], 'GET /users/{id}' or a full key. If not found or ambiguous, use atlas_search.",
];

const OUTRO: &str = "Add format \"json\" to any tool for structured output.";

pub const REMINDER: &str = "This repository is indexed by Code Atlas (the code-atlas MCP server). atlas_search finds where a name, text or structure lives across every repo of the project, atlas_symbol answers definitions and references, and atlas_context ranks the code to read for a task; one of them may answer this faster and more completely than a text search. This reminder appears once per session.";

pub fn instructions() -> String {
  let mut text = INTRO.to_owned();
  for (index, rule) in RULES.iter().enumerate() {
    text.push_str(&format!("\n{}. {rule}", index + 1));
  }
  text.push('\n');
  text.push_str(OUTRO);
  text
}

pub fn repo_block(project: &str) -> String {
  format!(
    "{BLOCK_BEGIN}\n## Code Atlas\n\nThis repo is indexed by Code Atlas (project `{project}`), served by the `{SERVER_NAME}` MCP server. Prefer it to grep when a question spans files or repos.\n\n{}\n{BLOCK_END}",
    instructions()
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  const AGENT_INTEGRATION_DOC: &str = include_str!("../../../docs/agent-integration.md");

  #[test]
  fn should_render_the_server_instructions_inside_the_repo_block() {
    let block = repo_block("acme");
    assert!(block.starts_with(&format!("{BLOCK_BEGIN}\n## Code Atlas\n")), "{block}");
    assert!(block.ends_with(&format!("\n{}\n{BLOCK_END}", instructions())), "{block}");
    assert!(block.contains("(project `acme`)"), "{block}");
  }

  #[test]
  fn should_document_exactly_the_rendered_repo_block() {
    let rendered = format!("```markdown\n{}\n```", repo_block("<project>"));
    assert!(
      AGENT_INTEGRATION_DOC.contains(&rendered),
      "docs/agent-integration.md must contain the rendered guidance block:\n{rendered}"
    );
  }

  #[test]
  fn should_state_the_editing_rules_every_rendering_must_carry() {
    let text = instructions();
    for rule in [
      "atlas_symbol mode=source only for code you will not edit yourself",
      "To rename or rewrite a symbol use atlas_edit",
      "after it changes a file, re-read that file before editing it yourself",
    ] {
      assert!(text.contains(rule), "{rule}");
    }
    assert!(text.lines().count() <= 12, "{text}");
  }

  #[test]
  fn should_never_list_the_edit_tool_as_read_only() {
    assert!(!READ_ONLY_TOOLS.contains(&EDIT_TOOL));
    assert!(READ_ONLY_TOOLS.windows(2).all(|pair| pair[0] < pair[1]));
  }
}
