# Evaluation methodology

Four harnesses measure four different things, plus a rename scenario run inside the quality eval. None replaces the others.

| Harness | Question | Binary | Output |
|---|---|---|---|
| Quality eval | Is the knowledge graph correct, and is its certainty honest? | `atlas-eval quality` | `eval/reports/<date>/quality.{md,json}` |
| Retrieval eval | Before an agent edits anything, does Code Atlas surface what the task needs? | `atlas-eval retrieval` | `eval/reports/<date>/retrieval.{md,json}` |
| Performance bench | How fast and how large, and does it regress? | `atlas-bench` | `perf.{md,json}` in `--out` |
| Agent A/B | Does an agent with Code Atlas do better work than one without? | `atlas-eval agent` | `eval/reports/<date>/agent.{md,json}` |

Measured results are in [results.md](results.md). This document covers method and limits only.

## Quality eval

**Fixtures.** `fixtures/acme` (4 repos: NestJS API, TS SDK, React web, Rust axum billing) and `fixtures/globex` (project isolation twin with the same names), plus 10 adversarial cases under `fixtures/adversarial/` (shadowing, re-export chain, duplicate names, trait dispatch, dynamic TS, computed routes, generated code, similar-not-equivalent, broken syntax, macro-heavy Rust). Each has an `expected.toml` written by hand from runtime semantics, not from tool output; the format is `fixtures/expected.schema.md`.

**Ground truth sections.** Required entities, required edges with accepted certainty classes and minimum confidence, forbidden edges (optionally "never above heuristic"), unresolved references that must stay unresolved, unmatched client calls, auth matrices, endpoint auth modes, unused and not-unused symbols, similarity pairs with the expected verdict and whether a deterministic claim is allowed, test-of relations, file flags, blast radii (must include, may include, must not include), isolation (entities that must never appear in a project's results).

**Procedure.** Each fixture project is indexed into a temporary home with the production engine; selectors are bound to entities (a selector must match exactly one entity or it is a *binding error*, excluded from recall and reported); expectations are judged *found*, *wrong certainty*, *wrong detail*, *missing* or *unbound*. Queries (`api`, `unused`, `similar`, `tests-for`, `impact`) are run through the real query layer for the corresponding sections.

**Oracle.** Unless `--no-scip` is given, SCIP (rust-analyzer when available; `--scip-typescript` adds scip-typescript) indexes the same trees and `atlas_scip::reconcile` compares `calls`, `references`, `instantiates`, `implements`, `extends` per site (enclosing symbol, file, span). Outcomes: agreed, ours-only, scip-only, conflicting; unconfirmable edges (cross-package, file-level) are separated from same-package unconfirmed ones. Precision is reported two ways (confirmable-only and all-in) and recall against SCIP's edges; `--scip-workspace` also runs the oracle over this repository.

**Metrics.** Entity and edge recall (bound and all), per kind; forbidden-edge violations; fabrication checks on unresolved references; `not_unused` violations; `must_not_include` and isolation leaks; certainty calibration (per class and per heuristic confidence bucket: correct, overclaimed, underclaimed, false); oracle precision and recall per edge kind and class; refactor robustness (six scripted edits on a copy of acme: rename, move file, extract function, delete function, change route literal, swap guard; each checks expected graph deltas, incremental equals full by fact-set digest, no stale deterministic fact, inferred facts on changed evidence no longer current); context selection precision and recall against task `ground_truth` (`--context-tasks <dir>`; fixture tasks only, so `t00`; the retrieval eval below covers every task).

**Limits (also printed in every report).** Small hand-written fixtures plus one real workspace: recall and precision say little about arbitrary code, and the ground truth was written alongside the system. SCIP covers five edge kinds in two languages, only in indexed files, and disagrees with us on granularity. Heuristic calibration rests on a few dozen judged edges. No fixture declares an exhaustive edge set, so precision against unlisted edges is not measured. Without an LLM no inferred verdicts exist, so verdict accuracy is not measured. Nothing is tuned between the run and the report.

**Re-run and compare.**

```sh
cargo build --release
target/release/atlas-eval quality --out eval/reports/$(date +%F)/quality
target/release/atlas-eval quality --compare eval/reports/2026-10-04/quality.json     # prints improved and regressed metrics
target/release/atlas-eval quality --only acme --no-scip --no-refactors --out /tmp/q   # fast subset, about 1 s
```

Other flags: `--repo-root`, `--date`, `--scip-work`, `--scip-only`, `--scip-timeout-secs` (1800), `--context-tasks`. The full run with SCIP needs `rust-analyzer`, `npx` and network or a warm npm cache, and takes minutes.

## Rename eval (inside the quality eval)

**Question.** Does a rename plan edit exactly the sites that reference the symbol, and list the ones it cannot see?

**Method.** `[[rename_sites]]` tables in a fixture's `expected.toml` (format in `fixtures/expected.schema.md`) give, per scenario, the symbol, the new name, every site the plan must edit, the sites it must list as not renamed with a reason, and lines that mention the name but must not be edited. The evaluator copies the fixture to a temporary directory, indexes it, plans the rename as a dry run through `IdeService` with one-shot language servers, and scores edit recall, edit precision, report recall and violations. A planned edit whose original text is not the old name never counts as a hit. A scenario without a language server is skipped, never scored as zero; a failed scenario counts its expected sites as missed. `atlas-eval quality` runs it by default (`--no-rename` skips, `--rename-only` writes `rename.md` and `rename.json`); a test that uses the real rust-analyzer runs only with `ATLAS_TEST_RENAME_REAL=1`.

**Limits.**

- The fixtures were used while rename was built, so the scores are a regression check, not a held-out evaluation. The report says so at the top.
- It scores plans, not applied edits, and does not compile the result.
- Real-repository evidence (hono, this repository; compile before and after) comes from review trials, not from this harness, and has no checked-in report.

## Retrieval eval

**Question.** Before the agent edits anything, does Code Atlas surface the files, symbols and lines the task needs, from the full prompt and from the short query an agent really sends? It is deterministic: no LLM and no agent sessions.

**Trees.** For every task in `eval/tasks` (`t00` to `t10`), the base tree is built the way the agent harness builds it: `git archive` at `base_commit` (or the pinned hono commit, from the clone cache shared with `agent run`, `eval/runs/.cache`), the same stripping, then `base.patch`. `setup` commands are skipped because they only install dependencies, and discovery excludes `node_modules` anyway. The tree is indexed in process with the production engine and the repos the harness registers (`atlas_repos`, or the tree root, named `tree`).

**Query variants.** Each task is queried once per variant, never from ground truth:

- `full`: the whole task prompt. This is what reports before 2026-10-12 scored, and their unprefixed metric names are today's `full.*`.
- `agent`: the `task` argument of every distinct `atlas_context` call recorded for the task, without its seeds.
- `agent-seeded`: every distinct recorded call as it was sent, the `task` argument with its `seeds`. Seeds reach `context` only; `search` and `similar` get the same text as `agent`. On a task whose recorded calls carry no seeds it equals `agent`.
- `first-line`: the first sentence of the prompt, so a task without transcripts still has a short query. The rule: take the prompt's first paragraph (the text before the first blank line, lines joined by a space) and cut it before the first `.`, `?` or `!` that follows a letter or digit, lies outside a backtick code span and is followed by whitespace or the end of the paragraph. Without such a character the whole paragraph is the query. An abbreviation such as `e.g.` followed by a space ends the sentence early.

A task with several queries for a variant is scored on each and the scores are averaged per task. A task with no recorded call is `n/a` for `agent` and `agent-seeded` and leaves their means; it is never scored 0.

**Recorded calls.** `eval/tasks/<task>/queries.toml` (format in `eval/README.md`) is extracted from `eval/runs/<batch>/<task>.atlas.r<i>/transcript.jsonl`: every `tool_use` block whose tool name ends in `atlas_context` contributes its `task` and `seeds` arguments verbatim. Calls with the same task text and seeds are merged into one entry that lists every run that sent it (`batch` is the batch directory name, `run` the run id). Atlas runs of the batch without such a call are listed under `no_call`. The checked-in files come from the batch `2026-10-05-directed`, in which 21 of 22 atlas runs made exactly one call and none passed another argument. The queries were written by agents talking to an older build; replaying them shows what today's build returns for the same words, not what those agents saw.

**Pipeline per query** (text = the variant's query):

1. `context(text)` at the default budget (6000 tokens; the CLI and MCP defaults are the same), with the recorded seeds for `agent-seeded`.
2. `search` (default limit 10) with the whole text, then once per significant phrase:
   - code spans: up to 8 backtick spans in order of appearance, at least 3 characters, case-insensitive duplicates dropped, fenced code blocks ignored;
   - prose phrases: up to 4. The text outside code is cut at punctuation and at a fixed stopword list. Runs of at least two remaining words are kept, and a longer run keeps its last four words.
3. `similar` on the first function or method hit of each search (exact name, then strong, then related), up to 5 distinct seeds. The prompts contain no in-tree code range, so `similar --snippet` does not apply.
4. `context(text)` again at budgets 4000, 8000 and 16000 for the budgeted yield.

**Scoring.**

- Every returned entity is mapped to a tree-relative file. The channels are:
  - the MUST READ section;
  - the whole pack: MUST READ, SKIM, PATTERN, REUSE CANDIDATES, IMPACT, TESTS and UNCERTAIN (omitted items do not count);
  - search;
  - similar;
  - any, the union of pack, search and similar.
- Per channel: recall of `must_change`, recall and precision of `relevant_files`, recall of `must_consider`. A ground-truth file that does not exist in the base tree, or exists but is never indexed (for example `CLAUDE.md`, or a file that failed analysis, which the report's degraded line would show), is listed separately as absent or unindexable and left out of the denominator. The blast-radius expectation leaves them out too. Precision is undefined for a channel that returned no file, so an empty MUST READ section leaves the precision mean instead of lowering it; the report counts the queries with an empty MUST READ and an empty pack per variant.
- F0.5 per channel: `1.25 x hits / (0.25 x gold + retrieved)`, which is `(1 + b^2) x P x R / (b^2 x P + R)` with `b = 0.5`, so precision weighs twice as much as recall. It is 0 when no gold file is retrieved, an empty channel included, and undefined only when the gold set is empty. Precision and recall need one gold set: it is `relevant_files` (those present in the index), the only labelled set these reports already score for both, so F0.5 follows from the two `relevant_files` rows. `must_change` is a subset of `relevant_files` in ten tasks; `t06` lists eight `must_change` files, all of them test files, outside `relevant_files`, and retrieving one of them counts as a false positive, in F0.5 as in the precision row. The aggregate also carries `must_change.f05.*`, the strict reading with `must_change` as gold.
- Line F0.5, for MUST READ and the whole pack:
  - Gold is the set of old-side (file, line) pairs `reference.patch` changes in files of the base tree: every removed line, and for an insertion that replaces nothing the old line directly before it and the one directly after it (those inside the file). An insertion next to a removed line is a replacement and adds nothing.
  - Files the patch adds have no base lines; they are left out and listed. A file the patch changes that is missing from the base tree is listed too.
  - Gold lines in base files the index does not hold (`CLAUDE.md` in `t02`) stay in the gold set, because the change needs them, and their count is shown per task. `t00` has no `reference.patch` and no line score.
  - Retrieved is the union of the line ranges the pack items state (`lines`). This is the range an item points the agent at, not the lines the pack prints: MUST READ bodies are cut at 80 lines, SKIM items show a signature, a doc line, a member outline and at most a 5-line excerpt. MUST READ, SKIM and TESTS items state their symbol's indexed span (`start_line` to `end_line`). A one-line mention (IMPACT, PATTERN, REUSE, UNCERTAIN) states the span of the reference that put it in the pack (a call, reference, instantiation, env or config read, endpoint call, request or auth use) when that reference lies inside the item, such as a caller's call site, and its span otherwise. An item that states no range retrieves its entity's indexed span (reports before 2026-10-15 scored every item this way); an item whose entity has no span retrieves no line and is counted.
  - Precision, recall and F0.5 are over (file, line) pairs with the same formula. A changed line between two symbols can only be covered by an item whose span contains the gap.
  - Effect of the stated-range rule (2026-10-15, same product at depth 3, old rule against new): MUST READ is unchanged; whole-pack line F0.5 rises by at most 0.0004 (0.0001 on the agent variants) and whole-pack line recall falls by 0.0065 on `full`, `agent` and `agent-seeded` (0 on `first-line`), because IMPACT callers now point at their call site while some gold lines sit elsewhere in the caller (`t02`).
  - Excerpts (the up to 5 lines a SKIM or outlined MUST READ item prints around its key site) are not scored: they cost tokens and bytes but retrieve no line. Line precision did not improve materially with line ranges and excerpts: line F0.5 stays below 0.1 for MUST READ and below 0.02 for the whole pack.
  - Expansion depth (graph hops from the seeds; enclosing types, endpoint handlers and guards, and auth-scheme guards are added without a hop) was measured at 0 to 3. Depths 1 to 3 have the same `must_change` recall on every task, in MUST READ and in the whole pack, for all four variants. On the agent variants MUST READ line F0.5 is 0.091 at depth 3, 0.083 at depth 2, 0.087 at depth 1 and 0.067 at depth 0; whole-pack line F0.5 is 0.015, 0.015, 0.018 and 0.023; packs at depth 1 are about 7% smaller. Depth 0 loses MUST READ recall on `full` (0.555 to 0.454). The default is 3: it has the best MUST READ line F0.5 on `agent`, `agent-seeded` and `first-line`, and depth 1 turns change sites two hops from the seeds into one-line IMPACT mentions (`t02`: `Engine::degraded`, MUST READ line recall 0.194 to 0.032). At any depth, direct dependents of a named seed stay one-line IMPACT mentions unless a deeper path reaches them.
- Budgeted yield: recall of `must_change` in MUST READ and in the whole pack when `context` runs with `budget` 4000, 8000 and 16000. The budget is the query's own parameter, so the product's token estimate decides what fits; the report shows the estimate and the rendered bytes at every budget, and counts the queries for which the budget changes the pack or the recall.
- `reuse_targets` recall per channel: a target is found when a returned entity key ends with the target's last two path segments, the matcher the agent harness uses for duplication.
- Pack size: bytes of the rendered text answer (without the freshness header, which carries timings) and the product's own token estimate.
- Blast radius, for tasks in the `blast-radius` category (`t02`, `t06`). The seeds are the innermost indexed symbols (function, method, type, variant, constant or macro) in the first `must_change` file, the definition site, that enclose a line `reference.patch` removes or an insertion it makes. `impact` runs once on all seeds, at the default group limit (what an agent sees) and unlimited, and recall is over the other `must_change` files. The report also shows how many files each view retrieved and the precision against `must_change`, which is low by design because `impact` lists every dependent. Here ground truth chooses the query input: this measures how complete `impact` is once the target is known, not whether an agent would find the target. It does not depend on the query variant.
- Aggregates are means over the tasks that define each metric, named `<variant>.<metric>`, each with its task count. The headline table condenses them for MUST READ and the whole pack. The comparison of each short variant with `full` is paired: it uses only the tasks that define the metric for both and counts the tasks on which the variant is higher, equal and lower. A task that cannot be materialised or indexed is reported as not scored, with its stage and error, and is excluded; the command still writes the report, then exits non-zero.

**Determinism.** Each task uses its own scratch directory under `--work-dir`, named by process id and a per-process sequence so concurrent runs never share or delete each other's trees, and removed afterwards. The output holds no timings, dates or absolute paths. The report records the repository HEAD and says when product sources have uncommitted changes. Two runs produce byte-identical files.

**Limits.**

- Dev set, same authors: printed at the top of every report.
- Eleven tasks. The paired counts show direction; they are not a significance test.
- The agent queries come from one batch, one model and one preamble (`directed`), two runs per task.
- The phrase extraction is a heuristic without part-of-speech tagging; environment notes in the hono prompts become search phrases too.
- "Any" assumes the agent reads every hit, so its precision is low by design.
- Line gold comes from one reference solution; another correct fix could change other lines. Pure insertions count as two lines however much they add.
- Blast-radius seeds rely on task authors listing the definition file first in `must_change`.

**Re-run.**

```sh
cargo build --release -p atlas-eval
target/release/atlas-eval retrieval --out eval/reports/$(date +%F)/retrieval     # all tasks, about 15 s; hono needs the cached clone or network
target/release/atlas-eval retrieval --task t00-smoke --out /tmp/retrieval          # about 1 s
```

Other flags: `--repo-root`, `--tasks`, `--cache-dir` (`runs/.cache` beside the task directory, so `eval/runs/.cache`), `--work-dir` (system temp dir). `--out` defaults to `reports/<date>/retrieval` beside the task directory.

## Performance bench

**Corpora** (`atlas-bench run --suite quick|full`):

| Corpus | Source | Suites |
|---|---|---|
| `self` | copy of this repository | both |
| `fixtures-acme`, `fixtures-globex` | the fixtures | both |
| `synth-100k` (2 repos), `synth-multi-project` (3 projects, 30k LOC) | generated, seed 20260504 | both |
| `synth-10k`, `synth-100k-5-repos`, `synth-1m` | generated | full |
| `ripgrep`, `hono` | pinned upstream commits fetched with `atlas-bench fetch` | full |

`atlas-bench gen --loc N --repos R --lang rust|ts|mixed --seed S --projects P --out DIR` produces a synthetic corpus: uniform arithmetic-shaped bodies, names from a fixed word list, 4% planted near-duplicates.

**Scenarios.** Each corpus runs in a fresh worker process (resource usage from `/usr/bin/time -l`), with a fresh home.

| Scenario | Measures |
|---|---|
| `cold_index` | `IndexMode::Full` on an empty database: time, files/s, LOC/s, peak RSS, CPU, DB size, similarity blob, parse failures. Median of 3 |
| `warm_startup`, `no_change_barrier` | engine open on an existing index; barrier latency with nothing changed (50 iterations) |
| `query_latency` | p50/p95/max for symbol, callers, impact (depth 3), search, similar, context (6,000 token budget), api, unused (heavy kinds are sampled less and time-boxed) |
| `concurrency`, `concurrency_with_edits` | mixed query workload at 1, 4, 8, 16 threads for 5 s windows (median of 3), optionally with a concurrent edit stream |
| `incremental` | 1, 10, 100, 1000 body edits: latency, entities and edges changed, edges rewritten |
| `rename_symbol`, `file_move`, `branch_switch`, `delete_directory` | structural change latency, rename detection |
| `multi_project` | footprint of several projects open together |

**Limits.** Synthetic corpora are a relative scale test, not realistic code (`synth-1m` is not representative of a real 1M LOC code base); ripgrep and hono are the realistic points. "Cold" means a fresh process and empty database but a warm OS page cache. The session worker's peak RSS mixes warm-open, query and edit work. Results depend on the machine; reports record OS, arch, logical CPUs and build profile, and comparisons refuse to run across differing environments.

**Re-run and compare.**

```sh
target/release/atlas-bench fetch                                  # full suite only
target/release/atlas-bench run --suite quick --out bench/new      # writes bench/new/perf.{md,json}
target/release/atlas-bench run --suite quick --out bench/new --compare bench/old/perf.json --fail-on-regression
```

`--only <corpus>` (repeatable) restricts corpora (a quick single-corpus run takes about two minutes because of repeats and concurrency windows), `--profile` adds profiling output, `--work-dir` and `--corpora-dir` relocate scratch and corpora. A metric regresses when it worsens by more than 10%, the change exceeds a 2.0 noise floor, and it exceeds the observed min-max spread of either run; `_max_ms` metrics are never compared. Run on a quiet machine in release mode.

## Agent A/B harness

Full reference: `eval/README.md`; task catalogue: `eval/tasks/README.md`; hidden-test verification: `eval/tasks/VERIFICATION.md`.

**Design.** Headless Claude Code sessions run the same task prompt in two conditions: `baseline`, with no Code Atlas, and `atlas`, with the Code Atlas MCP server and a preamble prepended to the prompt. Run order is a seeded shuffle, and `--runs-per-condition` defaults to 2.

**Preamble variants** (`PreambleVariant` in `crates/atlas-eval/src/agent/plan.rs`, chosen with `--atlas-preamble`):

| Variant | Version | Text (summary) |
|---|---|---|
| `standard` (default) | 1 | `STANDARD_PREAMBLE`, three sentences: a Code Atlas server with tools for context, search, impact, tests-for and explain is available; use it to understand the codebase before reading files; `[heuristic]`/`[inferred]` relations are leads to verify |
| `directed` | 2 | `DIRECTED_PREAMBLE`: call `atlas_context` before searching or reading; call `atlas_impact` before changing shared code; call `atlas_search` or `atlas_similar` before writing a helper; read files you edit with your own tool |

Each run records its variant, and the report shows it in the environment table (`preamble version`). `agent report` refuses a batch that mixes variants.

**Batches run so far.**

- Batch 1 (`eval/runs/2026-10-04-seed1`, `standard`) is the primary comparison, planned in advance (implementation plan §3.3).
- Batch 2 (`eval/runs/2026-10-05-directed`, `directed`) is exploratory. It was designed after batch 1 showed that the agent almost never called the tools, so it measures what the tools do once used, not whether agents use them unprompted. Because the design is post hoc, its report (`agent-directed.md`) is kept separate and makes no significance claims.

Both batches used the same tasks, model, seed, Code Atlas binary and 2 runs per condition. The harness commits differ (`0dc8a96` and `3c54d92`); the only change between them is the preamble option.

**Tasks.** Eleven: `t00-smoke` (harness check) and ten tasks across categories (find and extend, cross-crate blast radius, planted bug, refactor, architecture-consistent addition, config impact, reuse trap, test discovery, cross-package consistency). Six are on this repository frozen at a base commit, four on `honojs/hono`. Each has hidden tests that fail at base and pass with a reference patch, regression commands, and manually derived ground truth (`relevant_files`, `must_change`, `must_consider`, `reuse_targets`, `forbidden_duplicates`).

**Isolation.**

- Each run gets a fresh tree built with `git archive` at the base commit (no history or future commits reachable), committed as `eval-base` with a fixed identity; `eval/`, `scratch-pad/`, `docs/plan/`, hidden test destinations and (for fixture trees) `expected.toml` files are stripped. Optional `base.patch` plants a bug and `setup` commands run before the base commit.
- Tree paths are under a neutral temp root (`ae-<hash>/tree`) and the harness refuses ancestors that are git repositories, so nothing reveals the task, condition or repository. Each run has its own `CARGO_TARGET_DIR`.
- Both conditions use identical flags except the MCP config and the preamble: strict MCP config, no setting sources, no slash commands, no session persistence, tools limited to Read, Edit, Write, Glob, Grep, Bash, bypass-permissions mode, cleared environment rebuilt from an allowlist. The tree's root `CLAUDE.md` and `AGENTS.md` are injected identically into both conditions.
- Each batch runs a probe session recording which user config, MCP servers, skills and plugins leaked in. With OAuth login the Claude config directory cannot be isolated (`--claude-config isolated` needs an API key or token); isolation then rests on flags and the environment allowlist.

**Metrics** (per run, from the transcript): token totals and cost (reported, or estimated from per-turn usage and flagged), turns, tool calls by name, files read (Read plus simple `cat`/`head`/`tail`/`sed -n` in Bash; others counted as unparsed), bytes read, relevant files read and missed, irrelevant files read, time to first edit and session time (indexing excluded, reported as `atlas_index_ms`), information density (useful bytes over context bytes, a heuristic), Code Atlas calls and bytes, and `MUST READ` coverage credited as reading.

**Scoring.** Deterministic first: hidden tests (commands must pass), regressions (failures not already failing at base), `must_change` and `must_consider` coverage, reuse of `reuse_targets`, duplication introduced (post-session index and `query similar --snippet` for every added function; a structural Jaccard of 0.8 or more, or an inferred-equivalent verdict, against a reuse target or forbidden duplicate counts; a match with a lexical (BM25) score but no qualifying structural Jaccard (≥ 0.8) is recorded as a lexical candidate and reported as `lexical_duplicate_candidates`, never as duplication; that count is unknown when any query failed; failure to run is `unknown`, never `false`). Run files written before this split are re-scored by `agent report`: a stored score above 1 cannot be a Jaccard and becomes a lexical candidate. Those files never stored matches below 0.8, so their re-scored candidate counts are a lower bound and are not comparable with new runs. Then an LLM judge (default `--judge claude`, model set with `--judge-model`) scores five criteria 1 to 5 (correctness, completeness, architectural consistency, appropriate reuse, blast-radius handling) from a **blinded** prompt: the diff, test results and files read, with Code Atlas tool names replaced by a neutral name and no paths. Atlas runs additionally get an unblinded review ("did Code Atlas mislead the agent?") that is never part of the blinded scores.

**Aggregation.** Means of per-task means over tasks scored in both conditions (paired); per-metric sample size is shown when below the run count; outcome counts (completed, max turns, error, timeout, no result, setup failed) are reported per task, and a higher atlas rate of timeouts or max-turn hits is listed as a negative result. Setup failures are excluded and listed by stage.

**Contamination detection.** Every tool call is scanned for paths outside the tree (path fields and, best effort, absolute, `~` and `..` paths in Bash), ignoring toolchain and system locations. A run that touched the repository root, the eval home, another run's scratch directory or the repository its own tree was archived from (a local `repo.git` directory, the clone, an override directory) is `contaminated`: listed at the top of the report, counted per condition and excluded from all statistics. Bash is not sandboxed, so detection is after the fact.

**Version drift.** Self tasks freeze the tree at `base_commit`, but the atlas binary does not. The batch records the evaluation repository commit, the binary's build commit (`catl --version`) and its blake3 hash (`atlas_bin_blake3`). A binary built after a task's feature landed can leak the answer (t05 asks for the `atlas_dependents`/`atlas_dependencies` MCP tools; t01 asks for cwd-relative CLI targets). Compare the build commit with each self task's `base_commit`; prefer a binary built from `base_commit` for those tasks. `--repo-override` marks a batch not reproducible from a commit. Always pass `--atlas-bin` explicitly.

**Limits.** Few runs per condition and few tasks: differences are noisy and the judge is a model. Tasks are authored by the system's developers. Sessions are cost-bounded (`--budget-usd`, and `max_turns` and `timeout_minutes` set per task in `task.toml`). Metrics have documented caveats (partial output-token counts per turn, best-effort Bash read parsing, heuristic density, atlas relations reported as unverified without a SCIP reconciliation helper).

**Re-run and compare.**

```sh
cargo build --release
target/release/atlas-eval agent run --dry-run [--task t00-smoke]          # planned order and rough cost
target/release/atlas-eval agent run --task t00-smoke --runs-per-condition 1 \
  --atlas-bin target/release/catl --budget-usd 2                    # resumable per batch
target/release/atlas-eval agent report --runs eval/runs/<batch> --out eval/reports/<date>/agent \
  --compare eval/reports/<previous>/agent.json
target/release/atlas-eval agent probe --out /tmp/probe
```

Other `run` flags: `--tasks`, `--eval-home`, `--batch` (default `<date>-seed<seed>`), `--condition`, `--seed`, `--parallel` (2), `--judge claude|off`, `--atlas-preamble standard|directed`, `--atlas-config <dir>`, `--no-probe`, `--validation-timeout-secs`, `--atlas-timeout-secs`, `--tree-root`, `--keep-trees`, `--claude-bin`, `--model`, `--judge-model`, `--claude-config user|isolated`. A batch is resumable: runs with a `score.json` are reused. Artifacts per run (`eval/runs/<batch>/<task>.<condition>.r<i>/`): `run.json`, `transcript.jsonl`, `diff.patch`, `metrics.json`, `score.json`, `atlas-review.json`. External tasks need network once for the pnpm install step; real runs need the `claude` CLI.

**Atlas configuration (`--atlas-config <dir>`).** Each atlas session gets an empty `CODE_ATLAS_HOME`, so a project that needs route-family definitions would run the atlas arm without them. The flag copies `<dir>/config.toml` and `<dir>/detectors/*.toml` (nothing else; symbolic links are skipped, never followed) to `config/config.toml` and `config/detectors/` of the Code Atlas homes the run creates, which are the session home (`atlas-home`) of every atlas run and the duplication-scoring home (`atlas-post-home`) of every run, baseline included. The scoring home gets it in both arms so the duplication check indexes both arms alike; a baseline session never sees a Code Atlas home. Without the flag nothing changes.

Before any session starts, whenever at least one run is planned, the run is refused (no batch written) when `<dir>` is not a directory, holds neither file kind, has a `config.toml` that is not valid TOML, or does not load. Loading is checked in a probe home: the files are applied, a project is created over an empty repository, and the run is refused if `catl config show` prints any `warning:` or `error:` line or exits non-zero, or if `catl doctor` reports a `FAIL` line (for example `[mcp] tools` naming an unknown tool, which stops `catl mcp` from starting) or exits with a code other than 0 or 3. `warn` lines of `doctor` are ignored because they depend on the machine (PATH, language servers). An invalid family file that is enabled is an `error:` with exit 2; a family file that is not valid, or an enabled family without a file, is only a `warning:` and exit 0, which is why warnings refuse too.

`batch.json` `environment.atlas_config` holds the blake3 digest of the copied files (sorted relative path and bytes), their names and the dotted names of the keys `config.toml` sets (`defaults.detect.route_families`), never any value; the report's environment table shows them. Every `run.json` records the digest as `atlas_config_blake3`, and a batch is refused with `AtlasConfigMismatch` when a scored run was produced with another digest, or with none, than the invocation resuming it. The seeded files are left out of the `atlas-home` and `atlas-post-home` artifacts preserved in each run directory. `--dry-run` prints where the configuration is applied, the digest, the names and the key names.

Residual risk: the seeded files sit in the atlas session's scratch home, a sibling of the tree, and Bash is not sandboxed, so an atlas agent that reads them puts their contents into `transcript.jsonl` (the harness flags the access as out of tree, but does not prevent it). Keep family definitions that are private out of any batch whose transcripts are shared.

### Private task sets

The eleven tasks above were written by the developers of Code Atlas, mostly on its own repository, and every session passes them. A task set drawn from merged work in a repository the tool was not written for can exist beside them without being published: it lives in `eval/private/<name>/` (git-ignored) with its own `tasks/`, `runs/` and `reports/`. Nothing derived from that repository appears in a tracked file; tracked docs say only that such a set can exist, how one is built and how to point the harness at it (`eval/README.md`, "Private task sets").

- **Where results go.** Every command derives the eval home from its input (the parent of the task directory, or the directory two levels above a batch) and writes runs, the clone cache and reports there. A command refuses to write what derives from a git-ignored task set to a path git would track, and treats a path inside a repository as private or tracked when git cannot answer. A set or batch outside any repository is not recognised as private; `eval/README.md` lists exactly what is refused.
- **What both arms do not see.** A task can remove files or directories from its tree (`strip`) and a section from an instruction file (`drop_instruction_sections`), in both conditions and in the tree the deterministic commands read. The first is for text in the base tree that states the diagnosis, the second for project instructions that direct the agent to a tool no session has, which would otherwise weigh differently on the two arms. `batch.json` records the dropped sections.
- **How a task is built.** From one merged commit: the base is its parent, the reference patch is the change minus the hidden tests, and the hidden tests are the cases the commit added, moved into new files and reduced to what the prompt states. `agent run --base-only` verifies it without a session: validation fails and regression passes at the base, and both pass on a copy whose base patch is the reference.
- **Classes.** `feature` (at least 3 source files in at least 2 packages), `bugfix` (a failing test before the fix and a failure output that can be quoted), `refactor` (a rename, move or signature change with uses in at least 4 files, with a check for leftovers) and `control` (a correct single-file change, where a context tool should not help and only its overhead shows).
- **Prompts and the leak audit.** Prompts state the behaviour wanted or the symptom seen in product terms. `agent leak-audit` then searches every prompt term in the base tree and classes the task `handed`, `guided` or `open` by whether a term returns a file the task must change, or a file that names one, and among how many files. Labels and messages are quoted in the prompt so that they count as terms. The class is recorded in the task (`prompt_leak`), so results can be read per class; a realistic prompt is kept even when it is `handed`.
- **Limits.** A hidden test taken from a commit can pin names the commit introduced; such assertions are dropped or rewritten against code that exists at the base, which makes the hidden test weaker than the original. The leak class measures the distance from the prompt to the first file, not to every file a change needs. The audit does not see what a command named in the prompt would print. The guard knows a set is private only through git's ignore rules.

### Headroom from recorded sessions

`atlas-eval agent headroom --runs <batch>...` measures, without running anything, when a recorded session reaches the files its task must change and where it got its entry point from. It describes the sessions it reads; whether a context tool would change them is a separate question that it cannot answer.

- **Units.** A turn is one model response (one assistant `message.id`); the tool-call ordinal is the position of a call in the transcript. An event belongs to the call that issued it. The `num_turns` shown by `agent report` is on the tool-call scale.
- **Events per file.** Named: the path appears in the result of a Grep, Glob, Bash or MCP call whose input did not name the file. Searched: a search addressed to that one file. Read: Read, or `cat`, `head`, `tail`, `sed -n` in Bash. Edited: Edit or Write, or in Bash `sed -i`, `perl -i`, a redirect, `tee`, or a Python script that writes and names the file. Touched: the earliest of searched, read and edited. A result that names a file can also carry lines of it (grep output with context); that is still a naming, not a read.
- **Files.** `must_change` and `relevant_files` of the task spec. Files absent from the base tree are reported separately and excluded.
- **Bash.** Commands are interpreted lexically with the working directory followed across calls; the report lists the recognised forms, the unrecognised ones, and three checks (mentions in unrecognised forms, `cd` failures, edit events against the final diff).
- **First file and every file.** For a file set the report gives the first event over its files (the entry point) and, where every file has the event, the event of the last file (the whole change set located), plus counts pooled over files. Seen is the earliest of read and shown.
- **Entry point.** Terms of the prompt and of the injected project instructions (code spans, identifier-shaped tokens, quoted phrases) are grepped as fixed strings in the base tree; a term reaches a file that contains it. Per session, the text of the search that first named a `must_change` file, or the path opened without one, is compared with both texts: whole words first, then parts of words, prompt before instructions.
- **Output.** Per session, per task and overall: median, range and n of each event on both scales, the share of sessions with the first file and with every file read, touched or seen by turn 1, 2, 3 and 5, the pooled share of files, the share of the session before the first read and before the last file's first touch, files edited without an earlier read, and files with no recognised touch. Baseline sessions are pooled over batches; atlas sessions are grouped by preamble variant.
- **Limits.** Runs of one task are repetitions, so the task is the unit; prompts written by the authors of a fix tend to name the code; injected project instructions and public repositories give the model more to go on. The report prints these limits with the counts of the batches it read.

### Noise floor from recorded runs

`atlas-eval agent noise --runs <batch>... [--ab-report <agent.json>...]` is an A/A analysis of existing data: the difference between runs of the same condition, for every metric of the agent report and for `cache_weighted_tokens` (input + 1.25 × cache-creation + 0.1 × cache-read tokens).

- **Same baseline?** Before comparing baselines across batches it compares what defines a baseline session: model, CLI version, config isolation, judge model and probe per batch, and spec hash, tree source, base commit, injected instructions, command line (scratch directory masked) and environment names per run. A differing session field cancels the across-batch comparison; a differing task is left out of it. The eval repository commit is reported but cannot be checked from the run records.
- **A/A differences.** Second run minus first run of a task within one batch and condition, and baseline per-task mean of one batch minus the other. Per comparison: pairs, median absolute difference, median relative difference, range, and the paired statistic the A/B report uses (difference of the means of per-task means over the paired tasks).
- **Noise bound.** The largest of 2 × the replicate standard error of the batch, 2 × the pooled replicate standard error, and the largest paired A/A difference observed for the metric on the scale of an A/B effect: a repetition difference compares single runs, so it is multiplied by √(mean of 1 / runs per cell), which is 1/√2 with two runs. The replicate standard error is √(mean over tasks of (variance of the baseline per-task mean + variance of the atlas per-task mean) / tasks), with each variance estimated from the repetitions of that cell.
- **Robustness.** Every effect is printed with its size in standard errors of the batch and pooled, and with the number of tasks whose removal, one at a time, moves it across the bound.
- **Classes.** An A/B effect is `no difference`, `inside` or `outside` the bound, or `undetermined` without repetitions. `outside` is a lead, not a significance claim: each variance rests on one difference, a batch classifies some thirty correlated metrics, and about one pure-noise effect in twenty exceeds 2 standard errors.
- **Scope.** The result is the noise floor of the task set that was run. It does not replace an A/A run on a harder task set, whose variance will differ.
