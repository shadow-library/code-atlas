pub(crate) const INSTRUCTIONS: &str = "\
Code Atlas answers questions about ONE indexed project (a group of repos) from a knowledge graph; every call first brings the index up to date with the working tree.
1. Start a task with atlas_context: a ranked, budgeted pack of code to read, with reasons and omissions.
2. Before writing new code, run atlas_search and atlas_similar to find something to reuse.
3. Before changing shared code, run atlas_impact on it; use atlas_tests_for to pick tests.
4. Read only the code the results list. To edit a file, read it with your own file-reading tool; use atlas_read for code you only need to view.
5. [resolved]/[syntactic]/[compiler_verified]/[declared] relations are reliable but not exhaustive; always read the caveats. [heuristic] and [inferred] relations are leads: verify them in the source before relying on them.
6. An empty result is not proof of absence: read the caveats (unresolved references, files that failed to parse, hidden low-confidence relations).
7. After editing, call atlas_changes or re-query; freshness is automatic, there is no reindex tool.
8. Targets: a name (UsersService.findOne), a [repo:]path[:line], 'GET /users/{id}' or a full key. If not found or ambiguous, use atlas_search.
Add format \"json\" to any tool for structured output.";

macro_rules! certainty {
  () => {
    "Relations carry a certainty tag: [heuristic]/[inferred] ones are leads to verify in the source, not facts."
  };
}

pub(crate) const CONTEXT: &str = concat!(
  "START HERE for a task. Returns a ranked, budgeted pack of the code to read: full bodies for the top items, signatures for the next, one-liners for the rest, each with why it was included, plus what was omitted and why. ",
  "Items marked \"likely to edit: read the file directly\" show only a signature outline. To edit a file, read it with your own file-reading tool; use atlas_read for code you only need to view. ",
  "Seeds are optional symbols or files to anchor on. ",
  certainty!()
);
pub(crate) const SEARCH: &str = "Find where a capability is implemented or whether similar code already exists (names, text and structure). Use before writing new code. Returns ranked hits by match strength; a hit is a candidate, not proof of equivalence.";
pub(crate) const SYMBOL: &str =
  "Definition, signature, docs, location and relation counts for one symbol; the body only if body=true. Use atlas_read for the source of a single symbol.";
pub(crate) const CALLERS: &str = concat!(
  "Who calls this function or method, grouped by repo, up to depth hops (default 1). Misses dynamic or macro-generated calls; see the caveats. ",
  certainty!()
);
pub(crate) const CALLEES: &str = concat!(
  "What this function or method calls, grouped by repo, up to depth hops (default 1). Misses dynamic or macro-generated calls; see the caveats. ",
  certainty!()
);
pub(crate) const IMPACT: &str = concat!(
  "Blast radius of changing one or more targets: direct and transitive dependents grouped by repo, endpoints, cross-repo consumers, tests and config, with the path explaining each. Use before changing shared code. Depth defaults to 3. ",
  "Unresolved references can hide dependents; the caveats say where. ",
  certainty!()
);
pub(crate) const TESTS_FOR: &str = concat!(
  "Tests relevant to a target: direct, via the call graph, or via file coverage heuristics. Use to choose which tests to run. ",
  certainty!()
);
pub(crate) const API: &str = concat!(
  "HTTP endpoints with handlers, consumers across repos and auth schemes supported vs used. Filter by repo or endpoint ('GET /users/{id}'). ",
  certainty!()
);
pub(crate) const SIMILAR: &str = concat!(
  "Near-duplicates and equivalents of a symbol (target) or of a line range inside an indexed symbol (snippet, e.g. 'repo:src/money.ts:10-30'; code text is not accepted), ",
  "with consumers, tests and a likely shared home. Provide exactly one of target or snippet. Use before adding a helper. Matches are leads, not proof of equivalence."
);
pub(crate) const UNUSED: &str = "Candidate unused functions, endpoints, auth modes, events and config with evidence and caveats. These are candidates only: dynamic dispatch, reflection and external consumers can hide uses; verify before deleting.";
pub(crate) const PATTERNS: &str =
  "How this codebase does something (e.g. 'error handling', 'pagination'): exemplar implementations ranked by prevalence. Use to match local conventions.";
pub(crate) const EXPLAIN: &str =
  "Provenance of a symbol or relation: certainty, evidence spans, producing run, staleness and history. Use to decide whether to trust a [heuristic]/[inferred] fact.";
pub(crate) const READ: &str = "Source of exactly one symbol, re-read from disk (never a cache), with optional extra context lines. To edit a file, read it with your own file-reading tool; use atlas_read for code you only need to view.";
pub(crate) const OVERVIEW: &str = "Project map: repos, packages, applications, entrypoints, key modules and detected domains. Use to orient in an unfamiliar project.";
pub(crate) const STATUS: &str =
  "Index health: freshness, parse failures, SCIP coverage, LLM status and this server's per-tool call telemetry. Use when answers look incomplete or stale.";
pub(crate) const CHANGES: &str =
  "What the index changed in recent runs (files analysed, deleted, moved, renamed symbols). Use after editing to confirm the index saw it; since_run limits to newer runs.";
