# Code Atlas — Implementation Plan

Status: **awaiting approval**
Decisions locked with the user (2026-10-04):

| Decision | Choice |
|---|---|
| Languages (v1, in depth) | Rust + TypeScript/TSX/JS |
| Symbol resolution | Hybrid: tree-sitter always-on + SCIP ingestion (rust-analyzer, scip-typescript) when available |
| LLM backend | Pluggable `LlmProvider`; default = `claude -p` subprocess; Anthropic/OpenAI-compatible HTTP when a key is set; everything works with no LLM |
| Agent comparison | 10 tasks × 2 conditions × 2 runs = 40 headless sessions |
| Orchestration | `plan-orchestrator`, parallel mode (≤5 worktrees per batch) |

---

## 1. Architecture

### 1.1 Shape

A Cargo workspace. One binary, `code-atlas`, exposes a CLI and an MCP stdio server. Knowledge lives in one SQLite database **per project group**, which gives isolation between groups by construction. A repo listed in two groups is indexed into both databases. That duplicates work but cannot leak between groups.

```
crates/
  atlas-core        domain model: ids, kinds, provenance, evidence, FileFacts IR, errors, traits
  atlas-store       SQLite persistence, schema migrations, fact ownership, graph traversal primitives
  atlas-project     project registry, repo membership, discovery (ignore), hashing, stat cache, manifests
  atlas-lang-rust   tree-sitter Rust analyzer  -> FileFacts
  atlas-lang-ts     tree-sitter TS/TSX/JS analyzer -> FileFacts
  atlas-resolve     name/import/module resolution -> resolved edges
  atlas-detect      framework detectors: HTTP endpoints/clients, auth, config/env, tests, events, jobs, db
  atlas-scip        SCIP generation + ingestion -> compiler-verified edges
  atlas-similarity  structural fingerprints (MinHash/LSH), BM25 lexical index
  atlas-semantic    LlmProvider trait, claude-cli + http backends, cache, semantic tasks
  atlas-link        cross-repo linking: packages, API<->client, auth usage, events, config
  atlas-engine      indexing pipeline, incremental updates, rename/move lineage, freshness, watcher, telemetry
  atlas-query       query engine: search, impact, context packs, unused, duplicates, explain
  atlas-mcp         MCP server (tools) on top of atlas-query
  atlas-cli         `code-atlas` binary
  atlas-bench       performance harness + synthetic repo generator
  atlas-eval        knowledge-quality eval + agent A/B harness
fixtures/           realistic multi-repo fixture systems + adversarial cases + ground truth
eval/               agent task specs, run outputs, reports
docs/               architecture, knowledge model, provenance, agent integration, evaluation, limitations
```

Dependency direction: `core` ← everything. `store` ← `engine`, `query`. Each language analyzer depends on `core` alone. `engine` composes every analysis crate. `query` depends on `store`, `similarity` and `engine`. The engine dependency is for the freshness barrier. `mcp` and `cli` depend on `query` and `engine`.

### 1.2 Third-party dependencies (pre-declared in `[workspace.dependencies]` by T01)

Only mature, actively maintained crates are allowed:
`tree-sitter`, `tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-javascript`, `rusqlite` (bundled), `serde`, `serde_json`, `toml`, `clap`, `thiserror`, `tracing` and `tracing-subscriber`, `rayon`, `ignore`, `blake3`, `notify`, `scip` and `protobuf`, `rmcp` (the official MCP Rust SDK), `tokio`, `ureq` (blocking HTTP for the LLM backend, which avoids pulling a second async stack into analysis), `fd-lock`, `tempfile`, `insta` (dev), `criterion` (dev).

The MinHash and BM25 code is implemented in-house. Each is about 200 lines, and the available crates are either unmaintained or don't expose the normalisation hooks this design needs. If an implementer needs a dependency that isn't on the list, they must justify it in their task summary.

### 1.3 Knowledge model

**Stable identity.** IDs never contain line numbers. A symbol's key is built from:

```
<lang>:<repo>:<package>:<module-path>::<item-path>[#<kind>][~<n>]
e.g. rust:atlas:atlas-store:graph::GraphStore::neighbors#method
     ts:backend:@acme/api:src/users/users.controller.ts::UsersController.findOne#method
```

`~n` is a disambiguator, used only when sibling keys collide, such as TS overloads or a `#[cfg]` duplicate. Spans (byte range, line and column) live in a separate `occurrences` table and are rewritten on every reindex of their file. The integer id of a symbol row is a hash of its key, so the same key always maps to the same row across rebuilds.

**Entities.** `EntityKind` is a closed core enum serialised as strings, plus `Custom(String)`, so new knowledge types never need a schema change. The core kinds are:

- `repository`, `package`, `application`, `service`, `module`, `file`
- `function`, `method`, `class`, `struct`, `enum`, `trait`, `interface`, `type_alias`, `constant`, `field`
- `api_endpoint`, `api_client_call`, `auth_scheme`, `permission`, `config_key`, `env_var`
- `event`, `queue`, `job`, `db_table`, `db_model`, `external_service`
- `test`, `test_suite`, `capability`, `doc`

**Edges.** `EdgeKind` is a closed enum plus `Custom`:

- **Structure:** `contains`, `defines`, `exports`, `imports`, `re_exports`
- **Code relations:** `calls`, `references`, `implements`, `extends`, `instantiates`, `type_of`
- **Packages:** `depends_on_package`
- **APIs and auth:** `exposes_endpoint`, `calls_endpoint`, `supports_auth`, `uses_auth`, `requires_permission`
- **Config:** `reads_config`, `reads_env`
- **Events and data:** `publishes`, `subscribes`, `reads_table`, `writes_table`
- **Tests:** `tests` (test → subject), `covers_file`
- **Similarity and lineage:** `similar_to`, `equivalent_to`, `renamed_from`
- **Application structure:** `part_of_app`

**Provenance.** Every edge and every derived entity carries exactly one `Certainty`. It is a discriminated union, never parallel booleans:

| Certainty | Meaning | Example |
|---|---|---|
| `compiler_verified` | Came from a SCIP index produced by the language's compiler front-end | rust-analyzer resolved `foo()` → `crate::a::foo` |
| `syntactic` | Directly present in syntax, no resolution needed | `fn` X is defined in file Y; `#[get("/users")]` literal route |
| `resolved` | Unique result of emulated language resolution rules over the indexed code | `use crate::a::foo; foo()` resolved by our Rust resolver |
| `heuristic{confidence, rule}` | Ambiguous or pattern-based; several candidates or string matching | route `"/users/" + id` matched to `GET /users/:id` |
| `inferred{confidence, provider, model, rationale}` | LLM judgement over evidence | "`parseMoney` ≡ `toCents`: both normalise currency strings…" |
| `declared` | User configuration | repo X belongs to project A; manual link |

Every fact also has these fields:

- `evidence[]`: file id, byte span, the file's content hash at derivation time, and the producer name and version.
- `run_id`: the index run that derived it.

**Fact ownership.** Every deterministic fact is owned by the file or files that produced it. On reindexing a file, its facts are deleted and re-inserted in one transaction. Deterministic facts can therefore never go stale; they are always current or absent.

Cross-file resolved edges are owned by the referencing file. They are also registered as dependent on the target key. When a target key disappears, the dependents are re-resolved in the same update.

**Inferred facts and staleness.** Inferred facts are stored with evidence hashes. Any mismatch with current content marks the fact `stale`. Stale facts are excluded from default query results, shown with a `stale` flag when explicitly requested, and queued for revalidation. Every LLM call is cached by `blake3(task kind + prompt version + evidence contents)`, so unchanged inputs never trigger a repeat call.

**Rename and move lineage.** Within one update batch, a removed symbol and an added symbol with the same normalised body fingerprint get a `renamed_from` edge. The certainty is `heuristic`, with confidence equal to the fingerprint similarity, and the match must also be the unique best one. Inferred knowledge attached to the old key is migrated to the new key but marked `needs_revalidation`, never silently trusted.

### 1.4 Analysis pipeline

1. **Discover.** Walk repos with `ignore`, honouring `.gitignore`, and apply project include/exclude rules. Each file gets a language, its size, a stat-cache hit or miss, and a `blake3` hash when the stat changed.
2. **Parse and extract.** This runs in parallel with `rayon`. Each analyzer turns one file into `FileFacts`:
   - symbols with kind, key, span, signature, doc and visibility
   - imports and exports
   - unresolved references and calls, with the syntactic receiver and path
   - string literals at interesting call sites
   - decorators and attributes
   - a normalised token stream for fingerprints
   - parse diagnostics. Files with partial parse errors still yield facts, and the file is flagged `parse_status=partial`.
3. **Detect.** Detectors run over the tree-sitter tree and `FileFacts`. They emit domain entities (endpoints, client calls, auth schemes, env reads and so on) with certainty.
4. **Resolve.** These are project-wide passes:
   - the Rust module tree from `mod` declarations
   - `use` paths, crate dependencies from Cargo workspaces, `impl` blocks and trait methods
   - TS relative imports, `tsconfig` `paths` and `baseUrl`, package.json `exports`/`main`, and workspace and cross-repo package-name links
   - call targets: unique → `resolved`, multiple → `heuristic`, none → kept as an `unresolved` reference with a reason
5. **SCIP (optional).** When an indexer is available, the engine generates or ingests SCIP for each package. It matches occurrences to our symbols by (file, span). Matching edges are upgraded to `compiler_verified`, and the disagreements are recorded and counted as a quality metric. SCIP is regenerated lazily, with a configurable debounce, because it is slow. Between SCIP runs the tree-sitter facts stay authoritative for freshness, and SCIP-derived edges on changed files are dropped. That makes freshness win over precision on edited files, rather than serving stale compiler data.
6. **Link.** Cross-repo and cross-package linking:
   - npm and Cargo package dependencies are mapped to the repo that defines that package
   - API endpoints are matched to client calls by normalised method and route template. Base URLs are taken from config and env literals where they are statically visible.
   - auth schemes are matched to how each client authenticates (headers, token providers, SDK options)
   - event names are matched between publishers and subscribers
   - config and env keys are matched between readers and definitions
7. **Similarity.** MinHash of normalised AST token shingles (identifiers and literals abstracted) feeds LSH bucketing to find near-duplicates, which are scored. A BM25 index over names, paths, docs and identifier sub-tokens handles "does X exist" search.
8. **Semantic (optional).** The LLM never sees the whole repo. Semantic tasks run only over shortlisted candidates:
   - labelling candidate near-duplicate pairs as `equivalent`, `similar_but_different` or `unrelated`, with a rationale
   - optional capability summaries per module
   - proposing links for client calls left unresolved
9. **Persist.** One write transaction per update batch. The index run records counts, timings per phase, and the files that were added, changed, deleted or renamed.

### 1.5 Freshness contract

Every query runs a **freshness barrier** first:

1. Stat-scan the project's repos (mtime, size and inode). Hash only files whose stat changed.
2. If anything changed, run an incremental update before answering. Incremental cost is proportional to the changed files plus their dependents.
3. The response carries a `freshness` header: index run id, update duration, files updated, and whether SCIP lags behind.

When the optional `code-atlas watch` daemon is running and reports a clean state, the barrier skips the scan. Branch switches need no special handling: they look like many file changes, and content hashing makes this correct.

Correctness rule: **no query ever answers from an index known to be older than the working tree.** If the update fails, the query returns an explicit error or a `degraded` marker naming the affected files. It never silently returns old data.

### 1.6 Concurrency and reliability

- SQLite runs in WAL mode with one writer and many readers. An `fd-lock` on `<db>.lock` serialises writers across processes, so a CLI and an MCP server can share a database safely. `busy_timeout` is set.
- Analyzer panics are caught per file with `catch_unwind`. The file is recorded as `parse_status=failed` with the error, and the rest of the index proceeds.
- Schema migrations are versioned. A mismatched analyzer version triggers a re-derive of only the facts that analyzer produced.
- LLM is unavailable → semantic tasks are skipped and recorded as `skipped` in the run. Inferred facts already stored keep their stored certainty and staleness. They are never upgraded.

### 1.7 Query surface (shared by CLI and MCP)

Output is a compact, token-efficient text format by default, with `--json` for machines. Each relation line ends with a certainty tag, for example `[resolved]`, `[heuristic 0.72]` or `[inferred 0.81 stale]`.

| Tool / command | Answers |
|---|---|
| `overview` | Project map: repos, packages, applications, services, entrypoints, key modules, detected patterns |
| `search <text>` | "Where is this capability implemented / does something like it exist?" BM25 combined with structural similarity |
| `symbol <name|key>` | Definition, signature, doc and location (no body unless asked) |
| `callers` / `callees` / `dependents` / `dependencies` | Depth- and kind-filtered traversals across repos |
| `impact <targets>` | Blast radius: grouped direct/transitive by repo, app, API, test and config, ranked, with certainty and path explanations |
| `tests-for <target>` | Relevant tests (direct, via call graph, via file coverage heuristics) |
| `api` | Endpoints, consumers, auth schemes supported vs used, with evidence |
| `similar <target|snippet>` | Near-duplicates and equivalents, plus a consolidation report: consumers, tests, likely shared home, coupling risk |
| `unused` | Candidate unused functions, endpoints, auth modes, events and config, with evidence and caveats |
| `patterns <topic>` | How the codebase does X (exemplar implementations ranked by prevalence) |
| `context <task text> [--budget N]` | Minimal context pack: ranked must-read code, signatures to skim, why each was included, and what was omitted and why |
| `explain <edge|entity>` | Provenance: certainty, evidence spans, run, staleness, history |
| `status` | Index health, freshness, parse failures, SCIP coverage, LLM status, costs |
| `read <key> [--body]` | Source of exactly one symbol, re-read from disk (never from a cache) |

**Context pack algorithm** (`atlas-query`):

1. Seeds come from search hits on the task text, plus explicit symbols and files.
2. Do a typed-weight neighbourhood expansion: definitions, callers, callees, implementors, tests, config, API consumers and similar code.
3. Rank with personalised PageRank.
4. Fill the budget. Full bodies go to the top items, signatures to the next tier, and a one-line mention to the rest.
5. Correctness override: high-impact dependents of the seeds that rest on uncertain edges are always listed and flagged, never dropped to save budget.
6. Emit `why` for each item and an `omitted` list with reasons.

### 1.8 Developer experience

```
code-atlas project create acme --repo ~/src/acme-web --repo ~/src/acme-api --repo ~/src/acme-sdk
code-atlas project add-repo acme ~/src/acme-infra
code-atlas index                    # project inferred from cwd; --project to override
code-atlas query impact UsersService.findOne
code-atlas connect claude           # writes .mcp.json + agent guidance snippet (AGENTS.md / CLAUDE.md)
code-atlas mcp                      # stdio MCP server, project inferred from cwd or --project
code-atlas watch                    # optional low-latency freshness daemon
```

- The registry is stored at `$XDG_CONFIG_HOME/code-atlas/projects.toml`. Data goes to `$XDG_DATA_HOME/code-atlas/<project>/atlas.db`.
- The project is inferred from the cwd: the cwd must sit inside exactly one project's repo. If it sits inside two or more, the command errors and lists them; it never guesses.
- Cross-project queries require an explicit `--project`.

### 1.9 Observability

- `tracing` spans per phase.
- An `index_runs` table holding a per-phase timing, counts and the change set.
- `code-atlas status`, plus `code-atlas debug run <id>`, which shows what changed in that run and which facts were added or removed.
- `explain` for any fact.
- `context --explain`, which shows the scores, expansions and omissions.
- An LLM call ledger recording tokens, cache hits and cost.

---

## 2. Testing strategy

- **Unit tests** in every crate, named `should_*`. Analyzer tests use `insta` snapshots of `FileFacts` for real code samples.
- **Fixture systems** (`fixtures/`):
  - `acme` (project A):
    - `acme-web`: TS frontend using the SDK
    - `acme-api`: TS NestJS-style backend with three auth schemes — bearer JWT, API key and session cookie — guards and permissions, endpoints, events and env config
    - `acme-sdk`: a shared TS client with two of the three auth modes used
    - `acme-billing`: a Rust axum service that consumes `acme-api` over HTTP and publishes events
  - `globex` (project B): an unrelated TS + Rust pair that deliberately shares names and routes with `acme`. This is the isolation trap.
  - `adversarial/` contains:
    - ambiguous and duplicate names
    - shadowing and re-exports
    - trait objects and dynamic dispatch
    - computed routes and string-built URLs
    - generated code, macros and `eval`-style dynamism
    - similar-but-not-equivalent functions, for example the same shape but different rounding
    - partially broken syntax
    - files that vanish mid-index
  - Every fixture ships `expected.toml` ground truth: required edges, forbidden edges, and the expected certainty class.
- **Integration and E2E** (`atlas-engine`, `atlas-cli` tests) exercise:
  - indexing multiple repos
  - project isolation
  - modify, rename, move and delete followed by a query, then asserting exact graph deltas
  - a branch switch, by checking out another commit in a temp clone
  - stale inferred facts being excluded
  - LLM unavailable, using a fake provider that fails or times out
  - SCIP missing or failing
  - concurrent CLI and MCP writers
  - an MCP protocol round-trip
- **Property tests**, kept limited: reindexing unchanged content must be a no-op (fact-set identity); incremental and full index must produce identical fact sets after random edit sequences. This **incremental ≡ full** invariant is the central correctness test.

---

## 3. Evaluation

### 3.1 Knowledge quality (`atlas-eval quality`)

- Precision and recall per edge kind against the fixture `expected.toml`. Forbidden edges count as false-positive checks.
- Compare the heuristic resolver with SCIP as the oracle on Code Atlas itself and on an external repo. That gives precision and recall for `resolved` and `heuristic` edges, measured rather than asserted.
- Correctness after refactors: apply scripted refactors (rename, move, extract, delete) to fixtures. Verify that the graph deltas match expectations and that incremental ≡ full.
- Context-selection precision and recall against task ground truth from §3.3.
- Blast-radius accuracy against the known-affected sets for fixture changes.
- Measure the stale-fact rate after edit sequences. The target is 0 deterministic and 0 unflagged inferred.

### 3.2 Performance (`atlas-bench`)

- Corpora:
  - Code Atlas itself
  - ripgrep, a mid-size Rust workspace
  - a mid-size TS monorepo, chosen in T21 (candidates: `honojs/hono`, `nestjs/nest`)
  - synthetic generated repos at 10k, 100k and 1M LOC, both multi-repo and multi-project
- Metrics:
  - cold index time and throughput (LOC/s)
  - warm startup
  - freshness-barrier cost with no changes
  - incremental update latency for 1, 10, 100 and 1000 changed files, a large rename, and a branch switch
  - query latency (p50/p95) per tool
  - concurrent query throughput
  - peak RSS, CPU time, and database size
- Bottlenecks are identified by profiling (`cargo flamegraph` / `samply` if available, otherwise per-phase timing) and fixed in the iteration tasks.

### 3.3 Agent A/B comparison (`atlas-eval agent`)

- **Tasks:** 10 total. Six run against Code Atlas itself, pinned to a frozen commit. Four run against the external repo. The mix covers:
  - finding an implementation
  - adding a feature where reuse is possible
  - a refactor with cross-crate blast radius
  - fixing a bug that requires a dependency trace
  - a test-location task
  - a cross-package change
- **Task spec:** each task's TOML holds:
  - the prompt
  - the base commit
  - hidden validation tests, which the agent never sees
  - the full-suite regression command
  - the ground-truth relevant files
  - the expected reuse targets
  - the expected blast-radius set
  - forbidden duplicates
- **Isolation:**
  - Every run starts in a fresh `git worktree` at the base commit.
  - Runs use `claude -p --output-format stream-json --model <pinned> --strict-mcp-config --mcp-config <empty|atlas.json> --setting-sources ""` plus an isolated config dir. This keeps the user's Serena and graphify servers, CLAUDE.md files and memories out of both conditions.
  - Both conditions get identical prompts. The Code Atlas condition adds only the MCP server and a three-line hint that it exists.
- **Metrics:** extracted from the stream-json events:
  - input, output, cache-read and cache-creation tokens
  - tool calls by type
  - files read, plus bytes and lines read; with the ground truth these give irrelevant files read and relevant files missed
  - search calls
  - time to first edit, used as a proxy for "time to sufficient context"
  - wall time
  - Code Atlas responses and their bytes
- **Scoring:** deterministic checks come first:
  - hidden tests passing
  - regression suite passing
  - duplication: the similarity engine is run over the diff against the forbidden duplicates and reuse targets
  - blast-radius coverage: did the diff touch or consider the required dependents
  - Then a blinded LLM judge (it doesn't see the condition) scores architectural consistency and completeness against a rubric, and its rationale is stored.
  - Graph errors that were encountered are logged by replaying the Code Atlas responses against ground truth.
- **Reporting:** `eval/reports/<date>/report.md` gives per-task and aggregate tables, mean and spread across the two runs, and a negative-results section. Re-running the harness after code changes produces a diff against the previous report.

### 3.4 Dogfooding log

`docs/evaluation/dogfooding.md` records concrete instances, positive and negative, observed while Code Atlas is used during its own later development tasks (T22+).

---

## 4. Task breakdown (orchestrator input)

Tier → model: **low** = sonnet/standard, **medium** = sonnet/high, **high** = opus/high.
Batches run in parallel worktrees, at most five at a time. Tasks within a batch touch disjoint crates or directories.

| Batch | Task | Scope | Tier |
|---|---|---|---|
| 1 | **T01** Workspace scaffold + `atlas-core` | Creates every crate as a compiling stub, sets `[workspace.dependencies]`, commits `Cargo.lock`, adds rustfmt/clippy config and `scripts/check.sh`. `atlas-core` gets: IDs/keys, `EntityKind`, `EdgeKind`, `Certainty`, `Evidence`, the `FileFacts` IR, the `LanguageAnalyzer` / `Detector` / `LlmProvider` trait contracts, and the error types | high |
| 2 | **T02** `atlas-store` | Schema and migrations, fact ownership and atomic per-file replace, dependent registration, traversal primitives, run ledger, WAL, cross-process lock | high |
| 2 | **T03** `atlas-lang-rust` | Tree-sitter Rust → `FileFacts` covering items, impls, traits, `use`, `mod`, calls, macros (as opaque references), attributes, tests, docs, partial-parse handling and the token stream | high |
| 2 | **T04** `atlas-lang-ts` | TS/TSX/JS → `FileFacts` covering functions, classes, methods, interfaces, types, imports/exports (incl. re-exports and default), calls, decorators, call-site literals, tests (`describe`/`it`) and partial parses | high |
| 2 | **T05** `atlas-project` | Project registry, cwd → project inference, discovery, hashing, stat cache, manifest parsing (Cargo workspaces, package.json workspaces, tsconfig) | medium |
| 2 | **T06** fixtures | `acme`, `globex` and `adversarial` fixture repos + `expected.toml` ground truth (no Rust code) | medium |
| 3 | **T07** `atlas-resolve` | Rust and TS resolution (§1.4 step 4) with certainty assignment and unresolved reasons | high |
| 3 | **T08** `atlas-detect` | Detectors for HTTP endpoints/clients, auth schemes and usage, permissions, config/env, tests→subjects, events, jobs and db tables. Endpoints, auth, env and tests are core; events, jobs and db are experimental | high |
| 3 | **T09** `atlas-similarity` | MinHash/LSH fingerprints, near-duplicate clusters, BM25 index with identifier sub-tokenisation | medium |
| 3 | **T10** `atlas-scip` | Runs rust-analyzer scip / scip-typescript with timeouts, parses SCIP, maps by span, produces upgrade and disagreement records | medium |
| 3 | **T11** `atlas-semantic` | `LlmProvider` backends (claude-cli, http, fake), call cache and ledger, the three semantic tasks with versioned prompts, and storage of inferred facts with evidence hashes | medium |
| 4 | **T12** `atlas-engine` | Full and incremental pipeline, dependent re-resolution, rename/move lineage, stale invalidation, freshness barrier, watcher, run telemetry, failure isolation, and the incremental ≡ full property test | high |
| 4 | **T13** `atlas-link` | Cross-repo package links, API↔client matching, auth supported-vs-used, events, config | high |
| 5 | **T14** `atlas-query` | Every query in §1.7, including the context-pack algorithm, impact, unused, consolidation report, patterns and explain | high |
| 6 | **T15** `atlas-cli` | All commands, `connect claude`, the text and JSON output formats | medium |
| 6 | **T16** `atlas-mcp` | MCP tools mirroring the query surface, tool descriptions written for agents, and freshness headers | medium |
| 6 | **T17** `atlas-bench` | Perf harness, synthetic repo generator, corpus fetch script | medium |
| 7 | **T18** E2E + adversarial suite | Every scenario in §2, run against the CLI binary and the MCP protocol | high |
| 7 | **T19** `atlas-eval quality` | §3.1 metrics and report | medium |
| 7 | **T20** `atlas-eval agent` | §3.3 harness: runner, isolation, stream-json metrics, scoring, judge, report | high |
| 8 | **T21** Eval task authoring | 10 tasks with ground truth and hidden tests; freeze base commits; choose the external repo | high |
| 9+ | **T22…** Run & iterate | Orchestrator self-indexes, runs the quality eval, perf bench and 40 agent sessions, triages failures and dispatches fix and optimisation tasks; dogfoods via MCP | dynamic |
| last | **T9x** Docs | README, architecture, knowledge model and provenance, agent integration, limitations and unsupported cases, evaluation methodology and results, future work | medium |

### Contracts that make parallel batches safe

- T01 defines every cross-crate type and trait. Later tasks may extend `atlas-core` only by adding items, never by changing existing signatures. If an implementer needs a breaking change, they report it instead of making it.
- T01 pre-declares all expected third-party dependencies and commits `Cargo.lock`.

### Deviations from the stock `plan-orchestrator` (need your approval)

1. **`Cargo.lock`-only rebase conflicts.** The skill says to stop on any rebase conflict. A conflict confined to `Cargo.lock` is mechanical, so I will resolve it by regenerating the lockfile (`cargo update --workspace`, followed by `cargo check`) and record that in the state file. A conflict in any other file still stops the run.
2. **Sub-agent types.** The skill's `plan-implementer-*` and `plan-task-reviewer` agents aren't registered as agent types in this environment. I will dispatch `general-purpose` agents with an explicit model override (sonnet/opus) and paste the corresponding agent instructions into each prompt.
3. **Batches 9+ are not fixed in advance.** They are generated from measured results: eval failures, profiling and dogfooding. Each one is added to the state file before it is dispatched.

---

## 5. Definition of done

- `scripts/check.sh` passes: `cargo fmt --check`, `clippy -D warnings`, and all tests.
- Code Atlas indexes itself, the fixtures and the external corpora. Incremental ≡ full holds on scripted edit sequences.
- Project isolation is proven by tests on the `acme` / `globex` name collisions.
- The cross-repo API, auth supported-vs-used and SDK linkage in `acme` is detected with correct certainty classes.
- The quality report, perf report and agent A/B report are committed, with negative results included.
- The docs clearly separate implemented, experimental and unsupported features, and deterministic from inferred knowledge.
