macro_rules! certainty {
  () => {
    "Relations carry a certainty tag: [heuristic]/[inferred] ones are leads to verify in the source, not facts."
  };
}

pub(crate) const CONTEXT: &str = concat!(
  "START HERE for a task. Returns a ranked, budgeted pack of the code to read: full bodies for the top items, signatures for the next, one-liners for the rest, each with its line range and why it was included, plus what was omitted and why. ",
  "Items marked \"named in the task: read the file directly\" show an outline and the lines around the key site instead of the body; for a rename or symbol rewrite use atlas_edit. ",
  "Seeds are optional symbols or files to anchor on. Pass files or symbols you already read as known: they anchor the pack but are not listed again, so it shows what is missing. ",
  certainty!()
);
pub(crate) const SEARCH: &str = "Find where a capability is implemented or whether similar code already exists (names, text and structure). Use before writing new code. Returns ranked hits by match strength; a hit is a candidate, not proof of equivalence.";
pub(crate) const SYMBOL: &str = "Symbol lookup and IDE navigation. mode: info (default: definition, signature, docs, relation counts), find (name-path search like 'UsersService/findOne'), outline (a file's symbols from the index; source='lsp' for the exact language-server outline), source (code from disk), references | declaration | implementations (exact, from rust-analyzer or typescript-language-server), hierarchy (supertypes/subtypes from the graph), diagnostics (compiler errors for a file). Exact modes fail with an install hint when no server is available; for references, source='graph' returns tagged graph results instead.";
pub(crate) const CALLERS: &str = concat!(
  "Who calls this function or method, grouped by repo, up to depth hops (default 1). Misses dynamic or macro-generated calls; see the caveats. via_http includes HTTP callers from other repos. ",
  certainty!()
);
pub(crate) const CALLEES: &str = concat!(
  "What this function or method calls, grouped by repo, up to depth hops (default 1). Misses dynamic or macro-generated calls; see the caveats. via_http follows HTTP calls across repos. ",
  certainty!()
);
pub(crate) const IMPACT: &str = concat!(
  "Blast radius of changing one or more targets: direct and transitive dependents grouped by repo, endpoints, cross-repo consumers, tests and config, with the path explaining each. Use before changing shared code. Depth defaults to 3. ",
  "Unresolved references can hide dependents; the caveats say where. ",
  certainty!()
);
pub(crate) const TESTS_FOR: &str = concat!(
  "Tests relevant to a target: direct, via the call graph, or via file coverage heuristics. When no test reaches a method, a separate hedged method-name fallback lists tests that call a method of that name on an untyped value; it is not proof of coverage. Use to choose which tests to run. ",
  certainty!()
);
pub(crate) const API: &str = concat!(
  "HTTP endpoints with handlers, consumers across repos and auth schemes supported vs used. Filter by repo or endpoint ('GET /users/{id}'). ",
  certainty!()
);
pub(crate) const SIMILAR: &str = concat!(
  "Near-duplicates and equivalents of a symbol (target), of a line range inside an indexed symbol (snippet, e.g. 'repo:src/money.ts:10-30') or of draft code you have not saved (code with language rust|ts|tsx|js), ",
  "with consumers, tests and a likely shared home. Provide exactly one of target, snippet or code. Use before adding a helper. Matches are leads, not proof of equivalence."
);
pub(crate) const UNUSED: &str = "Candidate unused functions, endpoints, auth modes and events, plus test-only symbols (used only by tests), with evidence and caveats. These are candidates only: dynamic dispatch, reflection and external consumers can hide uses; verify before deleting.";
pub(crate) const PATTERNS: &str =
  "How this codebase does something (e.g. 'error handling', 'pagination'): exemplar implementations ranked by prevalence. Use to match local conventions.";
pub(crate) const EXPLAIN: &str =
  "Provenance of a symbol or relation: certainty, evidence spans, producing run, staleness and history. Use to decide whether to trust a [heuristic]/[inferred] fact.";
pub(crate) const OVERVIEW: &str = "Project map: repos, packages, applications, entrypoints, key modules and detected domains. Use to orient in an unfamiliar project.";
pub(crate) const STATUS: &str = "Index health: freshness, parse failures, SCIP coverage, LLM status, language servers and this server's per-tool call telemetry. Use when answers look incomplete or stale. since_run also lists what the index changed in newer runs (files analysed, deleted, moved, renamed symbols); use it after editing.";

pub(crate) const UNBOUND_INSTRUCTIONS: &str = "Code Atlas is not bound to a project here: the server started in a directory that no registered project contains, so it serves only atlas_status, which says how to register the repo. Use your own tools for this repo.";
pub(crate) const UNBOUND_STATUS: &str = "Why Code Atlas answers nothing here (no registered project contains this directory) and how to register the repo.";

pub(crate) fn edit() -> String {
  format!(
    "Edit code safely. operation: rename (exact, every file the language server sees; sites it cannot see are listed to verify; apply refuses when sites the server should have renamed are unverified, set allow_unverified only after checking them), replace_body / insert_before / insert_after (a symbol's definition), safe_delete (refuses and lists references), replace_content (one file), replace_in_files, apply (a plan id). dry_run=true returns diffs and a plan id. Refuses if a file changed since planning. Re-read changed files before editing them with your own tool. {}. {}. {}.",
    atlas_ide::REPLACE_BODY_NOTE,
    atlas_ide::REGEX_NOTE,
    atlas_ide::REPLACE_IN_FILES_NOTE
  )
}
