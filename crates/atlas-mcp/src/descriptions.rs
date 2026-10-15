pub(crate) const CONTEXT: &str = concat!(
  "START HERE for a task: a ranked, budgeted pack of the code to read across repos (bodies, then signatures, then one-liners), each with its line range and reason, plus what was left out. ",
  "Pass what you already read as known. Files named in the task get an outline and key lines, not a body. ",
  "The pack is seeded from the task's words and can miss the file to change: search when it is not there."
);
pub(crate) const SEARCH: &str = "Ranked search across every repo for where a capability, name or text lives (names, text and structure). Use instead of grep when you do not know the identifier, and before writing code to find something to reuse. Hits are candidates.";
pub(crate) const SYMBOL: &str = concat!(
  "One symbol or file. Use instead of grep or reading whole files for a definition, its references or a file's structure. ",
  "mode: info (default: definition, signature, docs, relation counts), find (name-path search), outline (a file's symbols), source (code from disk), ",
  "references | declaration | implementations (exact, from rust-analyzer or typescript-language-server; without one they fail with an install hint, and references source=graph answers from the index), ",
  "hierarchy (supertypes and subtypes), diagnostics (compiler errors)."
);
pub(crate) const CALLERS: &str = "Who calls a function or method, grouped by repo, up to depth hops; via_http adds HTTP callers in other repos. Unlike grep it resolves imports and methods. Misses dynamic and macro-generated calls (see the caveats).";
pub(crate) const CALLEES: &str =
  "What a function or method calls, grouped by repo, up to depth hops; via_http follows HTTP calls into other repos. Misses dynamic and macro-generated calls (see the caveats).";
pub(crate) const IMPACT: &str = "Blast radius of changing targets: dependents (direct and transitive, by repo), endpoints, cross-repo consumers, tests and config, each with the path that explains it. Use before changing shared code. Unresolved references can hide dependents (see the caveats).";
pub(crate) const TESTS_FOR: &str = "Tests that reach a target: directly, through the call graph or by file-coverage heuristics. Use to choose tests to run. When none reaches a method, a labelled fallback lists tests calling a method of that name on an untyped value: not proof of coverage.";
pub(crate) const API: &str = "HTTP endpoints with handlers, consumers in other repos and auth schemes supported vs used. Routes come only from NestJS, Express, axum and declared route families, so a missing route is not proof of absence.";
pub(crate) const SIMILAR: &str = "Near-duplicates of a symbol (target), of a line range inside an indexed symbol (snippet) or of unsaved code (code + language), with consumers, tests and a likely shared home; give exactly one. Use before adding a helper. Matches are leads, not proof of equivalence.";
pub(crate) const UNUSED: &str = "Candidate dead code: functions, endpoints, auth modes and events with no use found, plus symbols used only by tests. Dynamic dispatch, reflection and external consumers can hide uses: verify before deleting.";
pub(crate) const PATTERNS: &str =
  "How this codebase usually does something (a topic such as error handling or pagination): exemplar implementations ranked by prevalence. Use to match local conventions.";
pub(crate) const EXPLAIN: &str =
  "Provenance of a symbol or relation: certainty, evidence spans, producing run, staleness and history. Use to decide whether to trust a [heuristic]/[inferred] fact.";
pub(crate) const OVERVIEW: &str = "Project map: repos, packages, applications, entrypoints, key modules and detected domains. Use to orient in an unfamiliar project.";
pub(crate) const STATUS: &str = "Index health: freshness, parse failures, SCIP coverage, LLM status, language servers and per-tool call telemetry. Use when answers look incomplete or stale; after editing, since_run lists what the index changed.";

pub(crate) const UNBOUND_INSTRUCTIONS: &str = "Code Atlas is not bound to a project here: the server started in a directory that no registered project contains, so it serves only atlas_status, which says how to register the repo. Use your own tools for this repo.";
pub(crate) const UNBOUND_STATUS: &str = "Why Code Atlas answers nothing here (no registered project contains this directory) and how to register the repo.";

pub(crate) fn edit() -> String {
  format!(
    "Edit code exactly: rename across files through the language server (other sites are listed; apply refuses while sites it should have renamed are unverified, so set allow_unverified only after checking them), rewrite or insert around a symbol's definition, safe_delete (refuses while referenced), replace text in one file or many. dry_run=true returns diffs and a plan id for apply. Refuses if a file changed since planning. Re-read changed files before editing them with your own tool. {}. {}. {}.",
    atlas_ide::REPLACE_BODY_NOTE,
    atlas_ide::REGEX_NOTE,
    atlas_ide::REPLACE_IN_FILES_NOTE
  )
}
