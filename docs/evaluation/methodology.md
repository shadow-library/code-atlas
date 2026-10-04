# Evaluation methodology

Three harnesses measure three different things. None replaces the others.

| Harness | Question | Binary | Output |
|---|---|---|---|
| Quality eval | Is the knowledge graph correct, and is its certainty honest? | `atlas-eval quality` | `eval/reports/<date>/quality.{md,json}` |
| Performance bench | How fast and how large, and does it regress? | `atlas-bench` | `perf.{md,json}` in `--out` |
| Agent A/B | Does an agent with Code Atlas do better work than one without? | `atlas-eval agent` | `eval/reports/<date>/agent.{md,json}` |

Measured results are in [results.md](results.md). This document covers method and limits only.

## Quality eval

**Fixtures.** `fixtures/acme` (4 repos: NestJS API, TS SDK, React web, Rust axum billing) and `fixtures/globex` (project isolation twin with the same names), plus 10 adversarial cases under `fixtures/adversarial/` (shadowing, re-export chain, duplicate names, trait dispatch, dynamic TS, computed routes, generated code, similar-not-equivalent, broken syntax, macro-heavy Rust). Each has an `expected.toml` written by hand from runtime semantics, not from tool output; the format is `fixtures/expected.schema.md`.

**Ground truth sections.** Required entities, required edges with accepted certainty classes and minimum confidence, forbidden edges (optionally "never above heuristic"), unresolved references that must stay unresolved, unmatched client calls, auth matrices, endpoint auth modes, unused and not-unused symbols, similarity pairs with the expected verdict and whether a deterministic claim is allowed, test-of relations, file flags, blast radii (must include, may include, must not include), isolation (entities that must never appear in a project's results).

**Procedure.** Each fixture project is indexed into a temporary home with the production engine; selectors are bound to entities (a selector must match exactly one entity or it is a *binding error*, excluded from recall and reported); expectations are judged *found*, *wrong certainty*, *wrong detail*, *missing* or *unbound*. Queries (`api`, `unused`, `similar`, `tests-for`, `impact`) are run through the real query layer for the corresponding sections.

**Oracle.** Unless `--no-scip` is given, SCIP (rust-analyzer when available; `--scip-typescript` adds scip-typescript) indexes the same trees and `atlas_scip::reconcile` compares `calls`, `references`, `instantiates`, `implements`, `extends` per site (enclosing symbol, file, span). Outcomes: agreed, ours-only, scip-only, conflicting; unconfirmable edges (cross-package, file-level) are separated from same-package unconfirmed ones. Precision is reported two ways (confirmable-only and all-in) and recall against SCIP's edges; `--scip-workspace` also runs the oracle over this repository.

**Metrics.** Entity and edge recall (bound and all), per kind; forbidden-edge violations; fabrication checks on unresolved references; `not_unused` violations; `must_not_include` and isolation leaks; certainty calibration (per class and per heuristic confidence bucket: correct, overclaimed, underclaimed, false); oracle precision and recall per edge kind and class; refactor robustness (six scripted edits on a copy of acme: rename, move file, extract function, delete function, change route literal, swap guard; each checks expected graph deltas, incremental equals full by fact-set digest, no stale deterministic fact, inferred facts on changed evidence no longer current); context selection precision and recall against task `ground_truth` (`--context-tasks <dir>`).

**Limits (also printed in every report).** Small hand-written fixtures plus one real workspace: recall and precision say little about arbitrary code, and the ground truth was written alongside the system. SCIP covers five edge kinds in two languages, only in indexed files, and disagrees with us on granularity. Heuristic calibration rests on a few dozen judged edges. No fixture declares an exhaustive edge set, so precision against unlisted edges is not measured. Without an LLM no inferred verdicts exist, so verdict accuracy is not measured. Nothing is tuned between the run and the report.

**Re-run and compare.**

```sh
cargo build --release
target/release/atlas-eval quality --out eval/reports/$(date +%F)/quality
target/release/atlas-eval quality --compare eval/reports/2026-10-04/quality.json     # prints improved and regressed metrics
target/release/atlas-eval quality --only acme --no-scip --no-refactors --out /tmp/q   # fast subset, about 1 s
```

Other flags: `--repo-root`, `--date`, `--scip-work`, `--scip-only`, `--scip-timeout-secs` (1800), `--context-tasks`. The full run with SCIP needs `rust-analyzer`, `npx` and network or a warm npm cache, and takes minutes.

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

**Scoring.** Deterministic first: hidden tests (commands must pass), regressions (failures not already failing at base), `must_change` and `must_consider` coverage, reuse of `reuse_targets`, duplication introduced (post-session index and `query similar --snippet` for every added function; a match with score 0.8 or more, or an inferred-equivalent verdict, against a reuse target or forbidden duplicate counts; failure to run is `unknown`, never `false`). Then an LLM judge (default `--judge claude`, model set with `--judge-model`) scores five criteria 1 to 5 (correctness, completeness, architectural consistency, appropriate reuse, blast-radius handling) from a **blinded** prompt: the diff, test results and files read, with Code Atlas tool names replaced by a neutral name and no paths. Atlas runs additionally get an unblinded review ("did Code Atlas mislead the agent?") that is never part of the blinded scores.

**Aggregation.** Means of per-task means over tasks scored in both conditions (paired); per-metric sample size is shown when below the run count; outcome counts (completed, max turns, error, timeout, no result, setup failed) are reported per task, and a higher atlas rate of timeouts or max-turn hits is listed as a negative result. Setup failures are excluded and listed by stage.

**Contamination detection.** Every tool call is scanned for paths outside the tree (path fields and, best effort, absolute, `~` and `..` paths in Bash), ignoring toolchain and system locations. A run that touched the repository root, the eval home or another run's scratch directory is `contaminated`: listed at the top of the report, counted per condition and excluded from all statistics. Bash is not sandboxed, so detection is after the fact.

**Version drift.** Self tasks freeze the tree at `base_commit`, but the atlas binary does not. The batch records the evaluation repository commit, the binary's build commit (`code-atlas --version`) and its blake3 hash (`atlas_bin_blake3`). A binary built after a task's feature landed can leak the answer (t05 asks for the `atlas_dependents`/`atlas_dependencies` MCP tools; t01 asks for cwd-relative CLI targets). Compare the build commit with each self task's `base_commit`; prefer a binary built from `base_commit` for those tasks. `--repo-override` marks a batch not reproducible from a commit. Always pass `--atlas-bin` explicitly.

**Limits.** Few runs per condition and few tasks: differences are noisy and the judge is a model. Tasks are authored by the system's developers. Sessions are cost-bounded (`--budget-usd`, and `max_turns` and `timeout_minutes` set per task in `task.toml`). Metrics have documented caveats (partial output-token counts per turn, best-effort Bash read parsing, heuristic density, atlas relations reported as unverified without a SCIP reconciliation helper).

**Re-run and compare.**

```sh
cargo build --release
target/release/atlas-eval agent run --dry-run [--task t00-smoke]          # planned order and rough cost
target/release/atlas-eval agent run --task t00-smoke --runs-per-condition 1 \
  --atlas-bin target/release/code-atlas --budget-usd 2                    # resumable per batch
target/release/atlas-eval agent report --runs eval/runs/<batch> --out eval/reports/<date>/agent \
  --compare eval/reports/<previous>/agent.json
target/release/atlas-eval agent probe --out /tmp/probe
```

Other `run` flags: `--tasks`, `--eval-home`, `--batch` (default `<date>-seed<seed>`), `--condition`, `--seed`, `--parallel` (2), `--judge claude|off`, `--atlas-preamble standard|directed`, `--no-probe`, `--validation-timeout-secs`, `--atlas-timeout-secs`, `--tree-root`, `--keep-trees`, `--claude-bin`, `--model`, `--judge-model`, `--claude-config user|isolated`. A batch is resumable: runs with a `score.json` are reused. Artifacts per run (`eval/runs/<batch>/<task>.<condition>.r<i>/`): `run.json`, `transcript.jsonl`, `diff.patch`, `metrics.json`, `score.json`, `atlas-review.json`. External tasks need network once for the pnpm install step; real runs need the `claude` CLI.
