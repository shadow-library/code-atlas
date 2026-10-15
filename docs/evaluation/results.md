# Evaluation results

Measured 2026-10-04 to 2026-10-06. Method and harness limits are in [methodology.md](methodology.md). Every number below comes from a committed report under `eval/reports/` or from a commit message, or was derived from the gitignored run artefacts under `eval/runs/<batch>/`, except where a paragraph says it comes from *manual reviewer trials* (independent reviewers' trials during development, with real servers, not reproducible by an `atlas-eval` command; tables checked in as [`review-trials.md`](../../eval/reports/2026-10-06/review-trials.md), raw logs not) or from a review whose numbers are not in that file (the adversarial review of an implementation task, run before its merge; its artefacts are not in the repository). Each table or paragraph names its source. Derived numbers say how they were computed (see [Appendix](#appendix-derived-numbers)).

## Summary

Several evaluations, each asking a different question.

- **Knowledge quality** (`quality.md`). On the hand-written fixtures, Code Atlas found every expected entity (184/184) and edge (98/98), produced none of the 63 checked forbidden edges, and passed 6/6 refactor scenarios with incremental output equal to a full index. Against the compiler oracle (SCIP), `resolved` edges were 97.6% precise over confirmable edges (95.9% all-in) and `heuristic` edges 92.6% (82.9% all-in). Recall is the weak side: our `resolved` edges reproduce 53.3% of SCIP's call, reference and instantiation edges, and all our edges together reproduce 60.8%. Context-pack selection could be scored on one task only, where it found 50% of the relevant files at 10% to 20% precision.
- **Performance** (`perf-full/perf.md`). This repository (116k LOC) indexes cold in about 1.1 s, and a query's freshness barrier costs 6 ms (p50) when nothing changed. Real repositories index in 322 ms (ripgrep) and 421 ms (hono). A 1M LOC synthetic corpus indexes in 5.6 s with 2.21 GiB peak RSS. Mixed read and write concurrency at 1M LOC is weak: 8 to 26 queries per second, with p95 up to 974 ms. The machine was a loaded desktop.
- **Agent A/B, batch 1** (primary, planned in advance in implementation plan §3.3, `agent.md`). Every one of the 44 runs passed its hidden tests in both conditions, so correctness was at ceiling and cannot separate the conditions. The agent ignored the offered tools: there were 2 Code Atlas calls in 22 atlas sessions, both in the smoke task. The atlas condition still used 18.7% more tokens. Nearly all of that is the fixed cost of the larger prompt: about 6,300 extra tokens on every model request, almost all of them the definitions of the 16 MCP tools and the server instructions.
- **Agent A/B, batch 2** (exploratory, post hoc, `agent-directed.md`). With a directive preamble, 21 of 22 atlas sessions called `atlas_context` once. No session called any other Code Atlas tool. Atlas runs read 23% fewer bytes, but used 24.8% more tokens and about 2 more turns. Correctness was again at ceiling. Judge scores and files read differ by about as much as in batch 1's `t01`–`t10`, where the tools went unused. Only the bytes-read reduction exceeds that spread, and with 2 runs per task it is a lead, not a result.

- **Retrieval** (`retrieval.md`, 2026-10-05, dev set, same authors). Deterministic and LLM-free: from the task prompt alone, the MUST READ section held 46.4% of the files to change, the whole pack 68.5%, and any of pack, search and similar 87.0%. See [Retrieval](#retrieval-before-the-agent-edits).
- **IDE operations** (2026-10-05/06). Fixture rename eval: 7/7 scenarios, 41/41 edits, a regression check rather than a held-out result. On hono 4.13.13, 7 of 10 first-round TypeScript renames broke test compilation; after the fixes 19 symbols (plus 2 variants) added 0 `tsc` errors (manual reviewer trial). `similar` on draft code: structural match for 0 of 16 realistic re-implementations. Lexical reuse suggestions from task prompts were dropped (about 5% precision). Details and gaps: [IDE operations](#ide-operations).

**Bottom line.** The knowledge graph is accurate where it makes claims, and its certainty classes separate more precise edges from less precise ones. On these 11 tasks, however, there is **no evidence that Code Atlas helps a coding agent**: correctness was identical, tokens and cost went up, and without explicit instruction the agent did not use the tools. Both A/B batches cost $18.35 in total ($8.53 + $9.82, from the reports). The IDE operations (`atlas_edit` and the exact reads) were built after these batches and have no agent evaluation.

## Knowledge quality

Source: `eval/reports/2026-10-04/quality.md` (schema version 1, generated at `6a2854f`, committed in `14124c2`), with rust-analyzer and scip-typescript available.

### Fixtures

| measure | result |
|---|---|
| required entities | 184/184 (100%) |
| expected edges (bound recall) | 98/98 (100%), 0 wrong certainty or detail, 0 unbound |
| forbidden edges violated | 0/63 checked (+12 vacuous) |
| unresolved references kept unresolved | 14/15; 0 fabricated |
| blast radius `must_include` / `must_not_include` violations | 25/25 / 0/10 |
| isolation leaks (acme vs globex) | 0/18 |
| auth matrix (acme) | 100% precision and recall for supported, used and unused schemes |
| unused (acme) | 9/9 reported; 0/19 `not_unused` violations |
| similarity claims | 7 checked, 0 violated; no LLM verdicts exist in these runs |

These are the expectations the report does not pass:

- `tests_for`: an acme test was linked to `router` at `[heuristic 0.60]`, outside the certainty the fixture accepts (tests-for 8/9).
- `unused`: the globex `api_key` auth scheme was not reported as unused.
- `generated-code`: an `include!` reference was recorded as external `std` instead of `generated_code`.

The fixtures were written by the same developers as the system, so 100% on them shows that the stated behaviour holds, not that it generalises (see the report's *Limits*).

### Compiler oracle (SCIP)

Oracle edges cover `calls`, `references` and `instantiates` in every file an indexer covered: the fixture projects plus this repository (443 Rust files at `6a2854f`). Confirmable precision counts every unconfirmed same-package edge as wrong and ignores unconfirmable edges (cross-package, file-level). All-in precision counts every unconfirmed edge as wrong. Recall is measured against all of SCIP's edges.

| our class | agreed | precision (confirmable) | precision (all-in) | recall vs SCIP |
|---|---|---|---|---|
| resolved | 41,529 | 97.6% | 95.9% | 53.3% |
| heuristic | 5,834 | 92.6% | 82.9% | 7.5% |
| all | 47,363 | 97.0% | 94.1% | 60.8% |

Per kind (all classes):

| kind | precision (confirmable) | precision (all-in) | recall |
|---|---|---|---|
| calls | 96.3% | 93.2% | 90.1% |
| references | 98.0% | 95.0% | 45.5% |
| instantiates | 94.1% | 94.0% | 99.3% |
| implements | 46.9% | 28.4% | 100.0% |

- Low recall for `references` (45.5%) means that "no reference found" is weak evidence. Most misses in the sample are field and type references (`quality.md`, *SCIP-only edges*).
- Low `implements` precision is mostly a granularity difference: SCIP does not model method-level `implements` edges (impl method to trait method), so these always count as ours-only.
- There are 11 conflicting sites, all listed in the report. They fall into three kinds:
  - **Granularity (6).** We point at a class where SCIP points at its constructor, or at a method where SCIP points at the receiver constant (`acme`). These are not wrong targets.
  - **Wrong heuristic name-only candidates (4 sites), at 0.25 to 0.35.** For example, `db_path` resolved to `AtlasHome::db_path` instead of `Engine::db_path` at `[heuristic 0.35]`.
  - **One likely wrong `resolved` edge.** A reference to the free function `classify_failure` in `atlas-scip` `typescript.rs`, where SCIP points at the field `CachedInvocation::classify_failure`.

### Calibration

Fixture-judged edges (`quality.md`, *Certainty calibration*):

| class | judged | correct | overclaimed | false |
|---|---|---|---|---|
| heuristic | 46 | 46 | 0 | 0 |
| resolved | 43 | 43 | 0 | 0 |
| syntactic | 19 | 19 | 0 | 0 |

Heuristic edges against the oracle, by confidence (agreed / (agreed + conflicting)):

| confidence | agreed | conflicting | unconfirmed | agreement |
|---|---|---|---|---|
| [0.0, 0.4) | 668 | 5 | 431 | 99.3% |
| [0.4, 0.6) | 5,155 | 0 | 25 | 100.0% |
| [0.6, 0.8) | 11 | 2 | 2 | 84.6% |
| [0.8, 1.0] | 0 | 2 | 0 | 0.0% |

The fixture sample is small and was written to probe hard cases. On the oracle, the top two buckets hold only 15 judged edges. All 4 of their conflicts are granularity cases: class versus constructor at 0.60, and method versus SCIP's receiver constant at 0.85. The genuinely wrong name-only targets sit at 0.25 to 0.35. **Calibration is therefore neither shown nor refuted by this data.**

### Refactor robustness

All 6 scenarios pass with every check: rename symbol and callers, move file, extract function, delete function, change route literal, swap guard. Each one checks four things:

- the expected graph deltas appear;
- the incremental index equals a full index (fact-set digest);
- no deterministic fact is stale;
- inferred facts on changed evidence are no longer current.

### Context selection

Only `t00-smoke` could be scored, because the other ten tasks are not fixture repositories. On it, MUST READ files reached 20.0% precision and 50.0% recall, and MUST READ plus SKIM files reached 10.0% precision and 50.0% recall. One task is an anecdote, not a measurement. The A/B transcripts below say more.

### Report drift caveat

Against the previous report (`5a4b1d3`), the comparison lists 17 regressed and 30 improved metrics. Commit `14124c2` attributes all 17 regressions to the oracle target moving: this repository grew from 379 to 443 `.rs` files between the two reports. Old and new binaries scored on the same frozen tree (`6a2854f`) agree on every `calls`, `implements` and `instantiates` metric except heuristic calls (−0.0016), where 9 correct edges moved from heuristic to resolved. On the same tree, references recall rose from 0.266 to 0.455. The same-tree artefacts are not committed, so the commit message is the only source for these figures.

## Performance

Sources: `eval/reports/2026-10-04/perf-full/perf.md` (full suite, 1,178 s) and `perf-quick/perf.md` (quick suite, 501 s); per-process RSS from `perf-full/perf.json`. Release build, macOS aarch64, 14 logical CPUs. Commit `0dc8a96`: no agents ran, but desktop apps kept the load average at 4.7 to 8.7 during the runs, so absolute numbers carry that noise. As one measure of run-to-run spread, the `self` cold index took 1,233 ms in the quick suite and 1,074 ms in the full suite, a 15% difference on nearly the same tree (563 and 564 files).

### Cold index (full suite, median of 3)

| corpus | files | LOC | index | peak RSS (MiB) | DB (MiB) |
|---|---|---|---|---|---|
| self | 564 | 115,763 | 1,074 ms | 539.2 | 79.0 |
| ripgrep | 110 | 56,386 | 322 ms | 207.9 | 29.2 |
| hono | 450 | 91,940 | 421 ms | 275.9 | 35.1 |
| synth-100k | 583 | 100,495 | 523 ms | 299.7 | 39.3 |
| synth-1m | 5,511 | 1,001,293 | 5,600 ms | 2,258.9 | 388.3 |

Persisting is the largest phase: 38.8% of the summed phase time on `self` and 55.6% at 1M. In hono, 8 of 450 files parse only partially (the "parse failed/partial" column reads 0/8).

The reports print these sizes as "MB", but the values are MiB: `perf.json` stores the self `peak_rss_bytes` as 565,362,688, which is 539.2 × 2^20. This document uses MiB and GiB throughout.

The whole session worker, which mixes warm open, queries and edits, peaks higher: 1,183 MiB on `self`, 701 MiB on hono and 4,324 MiB (4.22 GiB) at 1M. These are `peak_rss_bytes` of the `session_process` records in `perf.json`.

### Warm open, barrier, incremental

| corpus | `Engine::open` | barrier p50 / p95 | 1-file edit | 100-file edit | rename symbol |
|---|---|---|---|---|---|
| self | 242 ms | 6.0 / 6.1 ms | 23.5 ms | 221 ms | 904 ms (318 files) |
| ripgrep | 77.8 ms | 4.0 / 4.2 ms | 5.4 ms | n/a | 261 ms (44 files) |
| hono | 126 ms | 4.4 / 12.9 ms | 8.6 ms | 146 ms | 273 ms (128 files) |
| synth-1m | 1,135 ms | 12.4 / 14.5 ms | 117 ms | 261 ms | 2,954 ms (1,630 files) |

At 1M LOC, editing 1,000 files takes 1,436 ms and a branch switch over 1,275 files takes 1,979 ms. The barrier figures were measured without a watcher, so every call is a full stat scan.

### Query latency including the barrier (p50 / p95 ms)

| corpus | symbol | callers | impact (depth 3) | context (6,000 tokens) | unused |
|---|---|---|---|---|---|
| self | 6.1 / 6.4 | 6.1 / 9.2 | 7.0 / 11.0 | 11.3 / 21.9 | 260.9 / max 266.1 |
| hono | 4.1 / 4.5 | 4.1 / 5.4 | 4.5 / 6.6 | 5.8 / 34.0 | 91.9 / max 93.0 |
| synth-1m | 15.8 / 65.2 | 14.5 / 46.4 | 16.1 / 90.8 | 74.5 / 135.7 | 1,084.7 / max 1,379.6 |

### Concurrency (qps / p95, median of three 5 s windows)

| corpus | read-only N=1 | read-only N=16 | with edits N=1 | with edits N=4 | with edits N=16 |
|---|---|---|---|---|---|
| self | 133 / 12.3 ms | 1,117 / 19.2 ms | 115 / 19.5 ms | 260 / 28.7 ms | 775 / 40.9 ms |
| hono | 207 / 6.9 ms | 1,695 / 11.0 ms | 185 / 8.9 ms | 387 / 16.4 ms | 1,195 / 42.4 ms |
| synth-1m | 33 / 93.1 ms | 225 / 187.9 ms | 8 / 158.9 ms | 10 / 973.9 ms | 26 / 703.8 ms |

Read-only throughput scales with threads. **The mixed result at 1M is weak**: under a concurrent edit stream, 16 threads reach 26 qps, and p95 is 691 to 974 ms from N=4 up. That is not interactive at that scale. At 100k LOC, mixed throughput still scales (synth-100k: 228 to 1,277 qps), although p95 rises to 61.8 ms at N=16.

In the multi-project scenario, 3 projects index in 527 ms into 3 separate databases with 0 cross-talk violations.

The synthetic corpora are a relative scale test (uniform bodies, fixed vocabulary). ripgrep and hono are the realistic data points.

## Retrieval before the agent edits

Source: `eval/reports/2026-10-05/retrieval.md` (commit `0f3557f`, `atlas-eval retrieval`, deterministic, no LLM). It is a dev set written by the same authors, so these are regression signals, not an estimate for unseen code. Mean over the 11 tasks, queried from the task prompt alone:

| channel | `must_change` recall | `relevant_files` recall | `relevant_files` precision |
|---|---|---|---|
| MUST READ | 0.464 | 0.386 | 0.372 |
| whole pack (6,000-token budget) | 0.685 | 0.535 | 0.232 |
| search | 0.694 | 0.726 | 0.113 |
| similar | 0.439 | 0.365 | 0.186 |
| any of pack, search, similar | 0.870 | 0.842 | 0.097 |

- `reuse_targets` (4 tasks): pack 0.40, search 0.60, similar 0.05, any 0.70. Blast radius (2 tasks): `impact` recall 1.0, but only once the changed symbol is given.
- The MUST READ section holds fewer than half of the files to change. The pack is a starting point; the agent must still search.
- Changes after this report (seed fixes, importability text) moved MUST READ `must_change` recall from 0.464 to 0.555 on this dev set (*review notes, not in `review-trials.md`; no new retrieval report was committed*). The same review found the gain of its first revision did not generalise to held-out tasks, and that lexical reuse suggestions from prompts were not worth keeping. See [IDE operations](#ide-operations) and [trust and limitations](../trust-and-limitations.md#negative-results-and-known-gaps-ide-operations-and-retrieval).
- Serving path, behaviour sites, fan-in discount and sibling routes (2026-10-15, `atlas-eval retrieval` at `40b1d37` against the same commit with the change; no report checked in). On `t00`–`t10` no `must_change` recall moved, in MUST READ or in the whole pack, on any variant (MUST READ: `full` 0.555, `agent` 0.569, `agent-seeded` 0.660, `first-line` 0.658). MUST READ `relevant_files` precision rose on `agent` and `agent-seeded` (0.597 to 0.618, 0.605 to 0.626) and on `first-line` (0.432 to 0.439); MUST READ line F0.5 moved by at most 0.001. Whole-pack `relevant_files` precision moved by at most 0.004 (0.346 to 0.344 on `agent`): `t07`'s second agent query lists the tests of `parseAccept`, now reached as the function that calls two search hits, instead of `serveStatic`'s (0.308 to 0.214), and `t02` is unchanged (0.714). The agent packs grew by 40 tokens on average, most of it `t05` (2011 to 2637 tokens), whose MUST READ gains `traversal` in `traverse.rs`, a file the task lists as relevant. On the invented `t11` the first-line query had no file to change in the pack before; after, the router is in MUST READ (`must_change` recall 0 to 1 with and without the route family; MUST READ `relevant_files` precision 0.333 to 0.600 with it, 0.429 to 0.500 without). Ablations on that query: without the route family only the behaviour-site rule fires, and it alone takes recall to 1.0 (the serving path has no endpoint to start from); with the family, the serving path alone also takes it to 1.0 (MUST READ precision 0.600), and behaviour sites alone take it to 1.0 at precision 0.500. The full prompt found the router lexically before and after (recall 1.0). A first version that added the callers of every single search hit lost `t01`'s file to change on one agent query and cut MUST READ line F0.5 on `agent` from 0.091 to 0.075; requiring two hits removed both effects. A first version of the fan-in discount that counted test callers dropped a `watch.rs` test of `t06` from TESTS and the shared `serialize` from a `hono/cookie` pack; counting only production users restored both.
- Every decorator on the handler, decorator-site exclusion by byte range, fan-in outside the decorator's file, the enum roll-up, and naming the only endpoint of a route mentioned without a method (2026-10-15, `atlas-eval retrieval` at `865f71e` against the same commit with the change; no report checked in). On `t00`–`t10` no `must_change` recall, `relevant_files` precision or line F0.5 moved, in MUST READ or in the whole pack, on any variant (MUST READ `must_change` recall: `full` 0.555, `agent` 0.569, `agent-seeded` 0.660, `first-line` 0.658; MUST READ `relevant_files` precision 0.353, 0.618, 0.626, 0.439). Two `full` packs changed. `t01`'s prompt names `GET /users/{id}`, whose acme handler carries the application's own `@RequirePermission`, so `PermissionsGuard.canActivate` comes in as sharing `PERMISSION_KEY` with that decorator and a SKIM item of `query.rs` gives way (4575 to 4535 tokens). `t03`'s prompt names routes such as `/health` without a method; each has one endpoint, which is now named instead of seeded as "another method", so its sibling routes are listed under PATTERN and MUST READ holds 7 items instead of 8 at the same file precision (2620 to 2600 tokens). `t11` is unchanged with and without its route family. On the invented `t12`, with its route family, the first-line query had no file to change in the pack before and has the invoker builder in MUST READ after (`must_change` recall 0 to 1, MUST READ `relevant_files` precision 0.333 to 0.500, +233 tokens); the full prompt had lost the router from MUST READ to the endpoint's seeds and gets it back (0 to 1, precision 0.429 to 0.571). Without the family `t12` has no endpoint and nothing moved (full prompt 1.0 lexically, first line 0). Ablations on `t12` with the family: without the parameter-decorator walk nothing moved; without the class-decorator walk recall still reaches 1.0 on both queries, at MUST READ precision 0.500 on the full prompt; walking only the route decorator for sibling routes' handlers loses the first-line MUST READ hit (the invoker builder scores 0.96 of the MUST READ floor without the sibling `subscribe` handler's `@Payload()` link). Two variants were measured and dropped: a MUST READ floor set only by items with a body (it grew `t01`'s MUST READ from 24 to 32 items and `t03`'s from 8 to 19, `full` MUST READ precision 0.353 to 0.342, with no recall gain) and that sibling gate. The enum roll-up changed no pack on any suite; it is covered by a fixture test.

## IDE operations

Exact reads and edits through rust-analyzer and typescript-language-server (`atlas_symbol` modes, `atlas_edit`), and the retrieval features built next to them.

### Rename: fixtures

Source: `eval/reports/2026-10-06/rename.md` (`atlas-eval quality`, dry runs through one-shot servers, typescript-language-server 5.1.3 with TypeScript 5.9.3, rust-analyzer 1.97.1). Truth is hand-written `[[rename_sites]]` in the fixtures. **These fixtures were used while rename was built, so this is a regression check, not a held-out evaluation.**

| scenario | fixture | edits found/expected | report sites found/expected | `untouched` lines edited |
|---|---|---|---|---|
| format-money | acme | 6/6 | 0/0 | 0/0 |
| users-service-find-one | acme | 5/5 | 0/0 | 0/4 |
| orders-service-find-one | acme | 2/2 | 3/3 | 0/4 |
| to-cents | acme | 7/7 | 2/2 | 0/1 |
| to-cents-apply | acme | 8/8 | 0/0 | 0/1 |
| store-get | trait-dispatch-rust | 7/7 | 0/0 | 0/1 |
| payment-provider-charge | ts-overrides | 6/6 | 1/1 | 0/2 |
| **all (7 scored, 0 failed, 0 skipped)** | | **41/41** | **6/6** | **0/13** |

A golden test suite (`crates/atlas-ide/tests/golden_rename.rs`: 7 rename scenarios plus 1 safe-delete, with exact expected sites, run against real servers and skipped with a `SKIPPED` line when a server is unavailable) checks the same kind of plans; it is also a regression check, not held-out.

Edit precision is 100% in every scenario. In `to-cents` the server kept the old name as an alias (`toMinorUnits as toCents`), and the plan says so in a caveat.

### Rename: real repositories (manual reviewer trials)

Manual reviewer trials on hono 4.13.13 (compile before and after, build and spec TypeScript projects) and on this repository, run during the review of the rename task (commit `21baaee`, 2026-10-05). Per-symbol tables for all rounds are in [`review-trials.md`](../../eval/reports/2026-10-06/review-trials.md). Baseline `tsc --noEmit`: build 8 errors, spec 11,199; "added" means new errors:

| round | what was measured | result |
|---|---|---|
| 0, before fixes | 10 TypeScript renames | **7 of 10 broke test compilation**: tsserver searched only loaded projects, and a solution-style `tsconfig.json` loads a project only when one of its files is opened, so plans depended on earlier reads |
| implementer re-trial after fixes | 6 renames | 0 added `tsc` errors |
| reviewer re-trial | 19 symbols (the 10 originals plus 9 new random ones), plus 2 variants of refused renames | every applied rename added 0 `tsc` errors in the build and spec projects; 1 nondeterministic plan (an object-literal property); 1 correct refusal (name conflict); 1 false refusal (a local binding); 1 rename refused through a kept alias that should have passed |
| final delta check | all trials | 0 added `tsc` errors |
| Rust, this repository | 5 renames | 5 of 5 pass `cargo check` (with `lsp.exclude` set; rust-analyzer was unhealthy on a full copy without it) |

The review also found writes of broken code (`export const export const` from `replace_body`) and a silent rename onto an existing name; both were fixed before merge. Known gaps after the fixes: rename determinism covers projects in the anchor's repo that contain or reference the anchor, and projects with no link to it are not opened (object-literal sites there can be missed); the clean-file TypeScript diagnostics baseline can time out under heavy load (reported `PARTIAL`); tsconfig `extends` arrays and packages are ignored (safe over-selection); default exports addressed by position are refused.

### `similar` from draft code (held-out, manual reviewer trial)

Manual reviewer trial of `similar --code` (tables in [`review-trials.md`](../../eval/reports/2026-10-06/review-trials.md)): a held-out set of 16 functions sampled with a fixed seed (8 Rust from this repository at `f559c69`, 8 TypeScript from hono 4.13.13) against an index of 23,459 symbols, same-language, limit 10, default threshold 0.5.

| draft | R@1 | R@5 | structural match |
|---|---|---|---|
| original, unchanged | 15/16 | 16/16 | 16/16 |
| light paraphrase | 15/16 | 16/16 | 16/16 |
| realistic re-implementation | 8/16 | 12/16 | **0/16** |
| realistic, threshold 0.3 | 10/16 | 13/16 | 3/16 |

Name-and-signature rows add 1.9 to 3.3 rows per query at 6% to 25% precision (13% / 6% / 25% for original / light / realistic at 0.5), with a floor and cap tuned on this same set (so not held-out). **Structure finds copies and light paraphrases, not independent re-implementations.** No LLM verdicts were produced.

### Reuse suggestions from task prompts: negative result

Matching a task prompt's prose against code to propose reuse candidates in `context` was tried and dropped (summary in [`review-trials.md`](../../eval/reports/2026-10-06/review-trials.md)). On a reviewer-owned held-out set of 17 tasks (the set size is (review notes, not in `review-trials.md`)): loose matching raised reuse recall from 0.54 to 0.70 at about 5% precision (4 of 73 lines were true hits) and fired on why and bug prompts; tight matching added 0 true hits among 7 new lines (0 of 3 on the dev set), with recall back at 0.54. About 780 lines were removed (review notes, not in `review-trials.md`). What shipped instead: seed fixes and importability text on structural REUSE entries (dev set, MUST READ `must_change` recall 0.464 to 0.555; REUSE recall unchanged at 0.4, (review notes, not in `review-trials.md`); no held-out confirmation recorded).

### Unused test-only, mocks, receiver chains (measured in review)

- **Unused, test-only class** (`385216d`): 28 of 28 on this repository and 57 of 57 on hono correct at merge, after a first version in which about 25% of this repository's test-only claims were wrong and a second whose recall collapsed. Those repositories were also the revision targets, so the figures are not held-out. Gaps: multi-line import and export blocks (about 7 hono symbols of recall), undeclared-dependency packages, classes called only through unresolved receivers, rustfmt `* ` continuation lines treated as comments.
- **Mocks** (`6efdf50`): 38 of 38 stored mock edges correct on two real repositories. Gaps: path-insensitive call graph, coarse file-level mocks, `bun:test` and assignment mocks not detected.
- **Rust receiver chains** (`4cedc6a`): declared types only; untyped `let` receivers are not inferred, extension traits on `Option` and `Result` stay `External(std)`, the inherent-versus-trait choice ignores receiver mode, and external globs still over-claim value-namespace names. In the implementer's run on this repository (before review fixes) it added 742 edges, upgraded 45 and lost none, with about 130 hand-checked correct.

### Parse budget (measured in review)

`59e357b` bounds every parse by a CPU-time budget (2 s plus 1 s per 100 KiB, cap 30 s) and retries overruns. Review, not a checked-in report: facts byte-identical on 1,266 files; a repository with 24 hung files had a 100 s first query under the first revision (wall-time budgets, uncapped serial retry) and 4.6 s with the final CPU-time budget and capped retries; under artificial starvation first-pass failures fell from 382 to 19, all recovered on a due retry; about 1% CPU overhead. Latency costs that remain: about 6 to 10 s on the query that first meets hung files, about 10 s for a due retry, and up to 300 s wall for an escalated retry under extreme starvation. Fuzzing (270k, later 736k cases) kept finding hanging inputs, which is why a budget rather than input fixes is the guard.

### Tool surface size

The MCP surface went from 16 to 15 tools and larger `atlas_symbol` and `atlas_edit` schemas. A test pins `tools/list` plus instructions at no more than 1.10 times the pre-change 15,140 bytes. The prompt-token cost per request has not been re-measured; the 6,300-token figure above is for the 16-tool binary `6a2854f`.

## Agent A/B

Setup, from the report environments:

- **Model.** Claude Code 2.1.289 with `claude-sonnet-5-5` as both session and judge model.
- **Code Atlas binary.** Build commit `6a2854f`, blake3 `ddbc1afe…`, identical in both batches.
- **Tasks.** 11 tasks × 2 conditions × 2 runs = 44 sessions per batch. Six tasks run on this repository frozen at `327f4dc`, four on hono at `f23b146`, and `t00-smoke` on the acme fixture (`eval/tasks/*/task.toml`).
- **Contamination.** 0 contaminated runs in either batch.
- **Judge failures.** Four in batch 1 and one in batch 2. The judge scores of those runs are missing; the reports show `n=1 of 2`.

### Batch 1: primary, planned in advance (preamble v1)

Source: `eval/reports/2026-10-04/agent.md` (runs `eval/runs/2026-10-04-seed1/`). The preamble (`STANDARD_PREAMBLE`) has three sentences. It names the tools (context, search, impact, tests-for, explain), says to use them to understand the codebase before reading files, and says that `[heuristic]`/`[inferred]` relations are leads to verify. The batch was planned in advance in implementation plan §3.3; no separate pre-registration document exists.

Means of per-task means over the 11 paired tasks:

| metric | baseline | atlas | atlas vs baseline |
|---|---|---|---|
| hidden tests pass | 1 | 1 | ceiling: 44/44 runs pass |
| regressions | 0 | 0 | |
| judge correctness (1–5) | 4.591 | 4.591 | 0 |
| judge completeness | 4.136 | 4.091 | −0.05 |
| judge architectural consistency | 4.5 | 4.636 | +0.14 |
| judge appropriate reuse | 4.409 | 4.682 | +0.27 |
| judge blast-radius handling | 4.0 | 3.773 | −0.23 |
| must_consider coverage | 0.452 | 0.479 | +0.03 |
| reuse targets referenced (4 tasks) | 1 | 0.75 | −0.25 |
| duplication introduced (5 tasks) | 0 | 0 | none; the earlier 0.2 was a metric artefact (see below) |
| lexical duplicate candidates (5 tasks) | 0 | 0.4 | lead only, a lower bound |
| total tokens | 207,066 | 245,716 | **+18.7%** |
| cache-read tokens | 184,424 | 222,074 | +20.4% |
| cost (USD) | 0.153 | 0.164 | +7% |
| turns | 12.64 | 11.18 | −1.5 |
| files read | 4.27 | 3.55 | −17% |
| bytes read | 14,675 | 13,739 | −6% |
| first edit | 15.9 s (5 tasks) | 29.6 s (5 tasks) | not the same task set; +21% on the 4 shared tasks |
| session time | 136.2 s | 142.5 s | +5% |
| Code Atlas calls per run | 0 | 0.091 | |

**Adoption.** Two Code Atlas calls were made in 22 atlas sessions, both `atlas_context`, one in each `t00-smoke` atlas run. The transcripts' `init` events show that in every atlas session, `t01`–`t10` included, the `code-atlas` server was `connected` and all 16 `atlas_*` tools were in the tool list. The agent chose Grep, Read and Bash instead. In `t01`–`t10` the atlas condition is therefore a near A/A comparison: it differs only by the preamble and the offered tools, so its differences show how large run-to-run noise is. Over those 10 tasks, with no tool calls, judged reuse moved by +0.30, blast radius by −0.10, `must_consider` coverage by −0.02, bytes read by −5.8% and files read by −15.7% (per-task means from `agent.md`).

**Token overhead.** The first model request of every atlas session was 6,322 tokens larger on average than the baseline request for the same task (derived from `metrics.json` `tokens.per_turn`; see the appendix). The preamble is about 50 tokens of that. The rest is the 16 tool definitions and the server instructions, which are part of every request's prompt and are read from cache on each request. Multiplying the extra prefix by the number of requests gives about 58,900 tokens per task. The observed difference is 38,650 tokens, so the prefix explains the whole overhead. Net of the prefix, atlas runs used about 10% fewer tokens, in line with their fewer turns (11.2 vs 12.6). Ten of the eleven tasks are near A/A, so that reduction is noise, not a tool effect.

**Time to first edit.** The aggregate row compares different task sets: baseline has values for {`t00`, `t03`, `t04`, `t05`, `t07`} and atlas for {`t00`, `t01`, `t03`, `t05`, `t07`}. The atlas set includes one `t01` run of 74.2 s. On the 4 tasks with a value in both conditions, the means are 15.3 s against 18.5 s (+21%). Only `t00` involved a tool call, so the difference is noise or prompt length, not tool latency.

**Duplication.** The first published version of these reports showed 0 against 0.2. It came entirely from `t07`, where neither atlas run called Code Atlas, and it was an artefact: the harness compared a lexical BM25 score with the 0.8 Jaccard threshold and labelled it structural (EVAL-7). Commit `88b4b68` fixed the metric and re-scored both batch reports: `duplication_introduced` is 0 in both conditions, and the former findings are listed as `lexical_duplicate_candidates` (batch 1: 0 against 0.4; both `t07` atlas runs wrote a named helper, the baseline runs inlined the logic). Those candidate counts are a lower bound, because the stored run files never kept matches below 0.8. All `t07` runs in both batches referenced the reuse target `parseAccept`.

### Batch 2: exploratory, post hoc (directed preamble v2)

Source: `eval/reports/2026-10-04/agent-directed.md` (runs `eval/runs/2026-10-05-directed/`). This batch was designed after batch 1 had shown the tools unused, so it was not planned in advance. Each task has 2 runs per condition, and no significance is claimed. The preamble told the agent to:

- call `atlas_context` before searching;
- call `atlas_impact` before changing shared code;
- call `atlas_search` or `atlas_similar` before writing a helper;
- read files to edit with its own tool.

| metric | baseline | atlas | atlas vs baseline |
|---|---|---|---|
| hidden tests pass | 1 | 1 | ceiling: 44/44 runs pass |
| judge correctness | 4.545 | 4.591 | +0.05 |
| judge completeness | 4.045 | 4.045 | 0 |
| judge architectural consistency | 4.636 | 4.591 | −0.05 |
| judge appropriate reuse | 4.409 | 4.773 | +0.36 |
| judge blast-radius handling | 4.091 | 3.864 | −0.23 |
| must_consider coverage | 0.475 | 0.581 | +0.11 |
| reuse targets referenced (4 tasks) | 1 | 0.875 | −0.125 |
| duplication introduced (5 tasks) | 0 | 0 | none (re-scored, see batch 1) |
| lexical duplicate candidates (5 tasks) | 0 | 0.2 | lead only, a lower bound (`t07` again) |
| relevant files missed | 1.636 | 1.409 | −0.23 |
| total tokens | 220,890 | 275,587 | **+24.8%** |
| cost (USD) | 0.159 | 0.182 | +14% |
| turns | 12.68 | 14.59 | **+1.9** |
| files read | 4.36 | 3.73 | −15% |
| bytes read | 14,843 | 11,435 | **−23%** |
| lines read | 385 | 297 | −23% |
| search calls | 3.45 | 2.95 | −0.5 |
| first edit | 16.0 s (5 tasks) | 14.3 s (6 tasks) | not the same task set; −13.5% on the 5 shared tasks |
| wall time | 156.7 s | 146.3 s | −7% |
| Code Atlas calls per run | 0 | 0.955 | |
| Code Atlas bytes per run | 0 | 11,198 | |

**Adoption** (from the transcripts):

- 21 of 22 atlas sessions called `atlas_context` exactly once; `t09` r0 made no call.
- None called `atlas_impact`, `atlas_search`, `atlas_similar` or any other tool, although the preamble asked for them.
- In all 21 sessions, the call right after `atlas_context` was a native search or read: Grep 17 times, Glob 2, Read 1 and `cat` 1.

The pack added context but did not replace the agent's own exploration.

**Effect when used.** Atlas runs read 23% fewer bytes. They also read 15% fewer files, but batch 1 shows the same drop without tool use. They took about 2 more turns, though, and the extra prefix (6,392 tokens per request in this batch) was paid again, so tokens rose by 24.8% and cost by 14%. Net of the prefix, tokens were 1.8% lower than baseline.

The following comparison covers `t01`–`t10` only (atlas vs baseline, per-task means from both reports), because in batch 1 the tools were used only in `t00`:

| metric | batch 1 (tools unused) | batch 2 (`atlas_context` used) |
|---|---|---|
| judge appropriate reuse | +0.30 | +0.40 |
| judge blast-radius handling | −0.10 | −0.10 |
| `must_consider` coverage | −0.02 | +0.07 |
| files read | −15.7% | −14.1% |
| bytes read | −5.8% | −22.5% |

- Judged reuse, blast radius and files read moved by about as much without the tools as with them.
- Bytes read and, more weakly, `must_consider` coverage moved beyond the batch 1 spread. With 2 runs per task, these are leads, not results.
- Between the two batches, baseline itself moved: +6.7% tokens and +14% session time, under identical conditions.

**Per-task observations.** Sampled from transcripts and the unblinded atlas reviews (`atlas-review.json`). The "MUST READ" and file lists below come from the `atlas_context` results in `transcript.jsonl`.

- **`t05-mcp-dependency-tools`, useful pack.** The pack (~1,961 tokens) listed `TraversalKind` with its `Dependents`/`Dependencies` variants, the CLI dispatch and `AtlasServer::call`. Both `must_change` files appeared in it. Its REUSE CANDIDATES section listed 6 items (same-shape `AtlasServer::callers`/`callees`/`explain` and small `as_str` methods), none of them a ground-truth reuse target. Three of the five `reuse_targets` (`render::targeted`, `arguments::depth`, `TraverseArgs`) did appear elsewhere in the pack. Atlas runs read 13,655 bytes against 30,614 for baseline, and `must_consider` coverage was 0.75 against 0.5. Tokens still rose (348,206 vs 307,705), and one atlas run took 275 s.
- **`t02-typed-degraded-cause`, accurate pack, more work.** The pack contained 5 of the 6 `must_change` files, and the reviewer found it accurate. Both runs still grepped and read the engine files first. One run took 34 turns, which put the task mean at 23.5 turns against 12 for baseline, and tokens rose by 44% (312,934 vs 218,023).
- **`t03-nested-router-links`, edit target missing.** Both packs pointed at `compose.rs`, the link-side composition step. The r0 pack also listed a NestJS `emit_endpoint` hit, which the reviewer called keyword noise. Neither pack mentioned `express.rs`, the one file that had to change. The agent found it with Grep. Tokens rose by 47% (365,564 vs 248,547).
- **`t07-static-precompressed-quality`, reuse surfaced.** The pack surfaced `parseAccept` and `getNextAcceptValue` from `src/utils/accept.ts`, and both atlas runs reused `parseAccept`. So did every baseline run, without the pack. The file to edit (`serve-static/index.ts`) was not in the pack.
- **`t10-auth-challenge-quoting`, empty pack.** The pack (~200 tokens, 588 bytes) contained no items, only a caveat that 8 hono files parse partially. The agent continued with Grep. The empty pack cost one turn and did no harm.
- **`t00-smoke`, noisy pack.** The seed `User` matched 2 entities and was dropped as ambiguous. `user.entity.ts`, the file to edit, never appeared, while `OrdersController`, `OrdersService` and delete handlers were listed as MUST READ because of keyword hits. The unblinded review flagged this pack as misleading, the only one of 21. The r1 pack is identical apart from the timing in its freshness line, yet its review found nothing misleading. It cost context budget but did not cause a wrong change.

### Threats to validity

- **Sample size.** Each batch has 11 tasks with 2 runs per condition. Per-task means rest on 2 runs, and time and token ranges within one cell often differ by 2× or more (for example, `t04` atlas session time in batch 1 ranged from 56.6 s to 334.4 s). No test of significance was run.
- **Ceiling.** Every run passed its hidden tests in both batches, so the main outcome cannot discriminate. The tasks are too easy for this model, or their hidden tests check too little.
- **Task authorship.** Tasks, ground truth and hidden tests were written by the same developers and tooling that built Code Atlas, so task selection and ground truth may favour what the graph models.
- **Self-repository tasks.** Six tasks run on this repository at `327f4dc`, an older commit than the Code Atlas binary (`6a2854f`, 21 commits later). The binary indexes the frozen tree, so its own code cannot leak an answer, but its tool behaviour and descriptions are newer. The `t05` answer (MCP tools `atlas_dependents` and `atlas_dependencies`) was not in the offered tool list; the `init` events list 16 tools without them. The model may also have seen hono in pre-training.
- **Judge.** The judge is the same model family as the agent (`claude-sonnet-5-5`), and five judge calls failed. Blinding removes tool names and paths, but the judge can still recognise style.
- **Duplication metric.** Fixed in `88b4b68` (EVAL-7): the harness used to compare the lexical BM25 score with the 0.8 Jaccard threshold and label it `structural`. Every earlier finding in both batches was this case, with scores of 5.24 to 8.36, so both reports were re-scored from 0.2 to 0. The metric still inspects only added named functions, so inline code is never flagged, and re-scored lexical candidate counts are a lower bound, not comparable with counts from new runs. `duplication_introduced` is weaker evidence than its name suggests.
- **Atlas reviews.** The "did Code Atlas mislead?" reviews are unblinded and come from the same model family. They are also inconsistent: the two `t00` packs are identical apart from a timing figure, but one was judged misleading and the other was not. The "1 of 21 misleading" count is a weak signal.
- **Claude configuration.** Both batches ran with `claude config = user`, and the isolation probe found 3 built-in plugins (`cc-plugin-agents-md`, `cc-plugin-telemetry`, `cc-plugin-plugin-authoring`) in both conditions. Implementation plan §3.3 specified an isolated config directory. The leak is identical across conditions, but it differs from the planned setup.
- **Harness version.** The batches ran from different evaluation commits (`0dc8a96` and `3c54d92`). The only harness change between them is the preamble option (`--atlas-preamble`); the Code Atlas binary is identical.
- **Contamination.** Detection found 0 contaminated runs in both batches. Bash is not sandboxed, so detection is after the fact and best effort.
- **Load.** The A/B batches ran on the same desktop as the performance runs. Load was logged for the performance runs (4.7 to 8.7, commit `0dc8a96`) but not for the A/B batches, and hidden-test builds share the machine. The time metrics are the noisiest metrics in the reports.
- **Post-hoc design.** Batch 2's preamble was chosen after seeing batch 1. It measures whether the tools help once used, not whether agents use them.

## What the evidence supports, and what it does not

Supported:

- On the fixtures and this repository, Code Atlas's authoritative edges are accurate: 97.6% confirmable and 95.9% all-in precision for `resolved` against SCIP, and 98/98 fixture edges with no forbidden edge.
- The certainty classes separate quality: `resolved` edges are more precise than `heuristic` ones (95.9% vs 82.9% all-in).
- Incremental indexing equals a full index on the scripted refactors, and the freshness barrier is cheap at the scale of real repositories (4 to 6 ms p50).
- With a directive preamble, an agent calls `atlas_context`. In this small sample it read 22.5% fewer bytes on `t01`–`t10`, beyond the batch 1 spread of −5.8%. That is a lead for a larger study, not a result. The 14% to 16% drop in files read appears in both batches, with and without tool use, so it is not attributed to the tool.

Not supported:

- That Code Atlas improves an agent's correctness: both batches are at ceiling, with no difference.
- That it saves tokens or money: the atlas condition used more tokens (+18.7%, +24.8%) and cost more (+7%, +14%) in both batches.
- That agents adopt the tools without being told to: 2 calls in 22 sessions.
- Any benefit from `impact`, `search`, `similar` or the other 12 tools in agent use: they were never called.
- Better reuse or blast-radius handling: the judged differences are within the spread observed with no tool use.
- That the IDE operations help agents: no agent batch has used `atlas_edit` or the exact reads.
- That rename is deterministic everywhere: unlinked TypeScript projects are not opened, and the real-repository trials are review measurements on one TypeScript repository (hono) and this repository.
- That `similar` finds re-implementations: 0 of 16 realistic re-implementations matched structurally.
- That the graph is complete: `resolved` edges reproduce 53.3% of SCIP's edges, and references only 40.4%.
- That heuristic confidence values are calibrated. The fixture sample was 46/46 correct, but on the oracle the upper buckets hold 15 edges, and their conflicts are granularity cases. Calibration is neither shown nor refuted.
- Interactive concurrent use at 1M LOC under edits.

## Future work

Grounded in these findings:

- **Evaluate `atlas_edit` with agents.** Rename and symbol edits have fixture and review evidence only. Measure whether agents use them, and whether re-reading rules and refusals cost or save turns.
- **Held-out evidence for rename and `similar`.** Commit the hono and self trials as a report, and score rename on fixtures that were not used to build it.
- **Harder tasks.** Tasks where baseline agents fail: cross-repo blast radius, larger unfamiliar code bases, tasks whose hidden tests check what was *not* changed. These need repositories and authors independent of Code Atlas, and more runs per cell.
- **Cheaper presence.** Offering the server cost about 6,300 tokens per request before any call (measured for the 16-tool surface; the 15-tool surface with `atlas_edit` has not been re-measured). Exposing fewer tools by default (for example `atlas_context` and `atlas_impact`) or shortening the tool descriptions would cut the fixed cost, which is currently larger than any read savings.
- **Adoption.** With the three-sentence standard preamble, which names the tools and says to use them before reading files, the agent called them in 2 of 22 sessions, both in the smoke task. Integration guidance (`connect claude`) should use directive wording, and the effect of that wording should be measured on tasks off the ceiling.
- **Context packs.**
  - Rank graph neighbours of the seeds above keyword hits. Measured by `must_change`, the packs held none of the files to edit in 4 tasks (`t00`, `t03`, `t07`, and `t10` with its empty pack) and only some of them in 5 (`t02` 5/6, `t04` 2/5, `t06` 3/11, `t08` 5/8, `t09` 2/4). Only `t01` and `t05` held them all.
  - Resolve an ambiguous seed against the task text instead of dropping it (`t00`).
  - Fall back to search hits when the pack would be empty (`t10`).
  - Make packs smaller: the agent re-read files anyway.
- **Evaluation harness.**
  - Score duplication only on structural or inferred evidence, and label the lexical fallback as lexical.
  - Detect inline duplication.
  - Use a judge from another model family.
  - Log load during A/B batches.
  - Make `first_edit_ms` available for every run: it has a value for only 5 or 6 of 11 tasks.
- **Calibration.** Collect more oracle-judged heuristic edges in the upper confidence buckets, and separate granularity conflicts from wrong targets, before relying on confidence values.
- **Scale.** Investigate the mixed read and write throughput at 1M LOC and the 4.22 GiB session peak RSS.

Open backlog (development follow-ups, tracked outside the repository):

- **DISPATCH-1.** Fan-out over `dyn` and generic receivers includes impls from crates that are not dependencies and from other crates' test code. It should be limited to the caller's package and its dependencies, and needs a resolver version bump.
- **MACRO-1.** A crate-local `macro_rules!` macro named like a std macro (`line`) is treated as std.
- **Link pin corpus.** Add a root `test/*.e2e-spec.ts` client call, so that test-code changes in the link pass are pinned.
- **EVAL-6.** The evaluator's SCIP oracle plan diverges from the engine's SCIP units for nested or non-root workspaces. Drive it from the engine's plan.
- **scip-typescript workspaces.** scip-typescript still runs per npm package, so cross-package references inside an npm workspace stay unverified.
- **Analyzer gaps.** `this: T` parameter annotations are ignored, `extends` is missing for local classes, and object-literal members have no symbols. Partial parses of TS files also occur, as in 8 of 450 hono files.
- **PERF-3.** An N+1 owner lookup in `select_entities` and `select_edges`.
- **ENGINE-3.** The store lacks a list-owners API and an API to migrate inferred subjects.
- **QUERY-1.** Assertions carry no per-assertion run id, and there is no API to list edges or unresolved references by reason.
- **EVAL-5.** Check that the evaluator applies the DB-model convention (class symbol plus a `maps_to_table` edge).
- **Hold fingerprint inputs.** The automatic-SCIP failure hold fingerprints file hashes and the package graph, but not lockfiles, `rust-toolchain*` or `.cargo/config*`. A failure fixed by changing only those waits for the hold to expire.

New items from this evaluation:

- **EVAL-7.** Duplication metric. Fixed in `88b4b68` (lexical matches are separate candidates; both batch reports re-scored from 0.2 to 0). Still open: it ignores inline code.
- **CTX-1.** Context packs miss edit targets.
  - `must_change` files were absent in 4 of 11 tasks and partly absent in 5.
  - Ambiguous seeds are dropped (partly addressed: a name narrowed by a named repository is now seeded).
  - Empty packs offer no fallback.
  - The retrieval eval now measures this without agents; the only checked-in report predates the seed fixes ([Retrieval](#retrieval-before-the-agent-edits)).
- **CAL-1.** Calibration needs more oracle-judged heuristic edges in [0.6, 1.0], with granularity conflicts separated from wrong targets.

## Appendix: derived numbers

These figures were computed from the gitignored run artefacts, not read off a report.

- **Code Atlas calls and the offered tool list:** `tool_use` blocks whose name contains `atlas` in `eval/runs/<batch>/*.atlas.r*/transcript.jsonl`, and the `tools` and `mcp_servers` fields of each transcript's `system`/`init` event.
- **Fixed prompt overhead:** for each run, the first entry of `metrics.json` `tokens.per_turn` gives the first prompt size (`input + cache_read + cache_creation`). The per-task difference of the condition means, averaged over tasks, is 6,322 tokens in batch 1 and 6,392 in batch 2. The overhead per task is the mean over atlas runs of (number of `per_turn` entries × that task's difference), averaged over tasks like the report aggregates: 58,907 and 58,690 tokens. "Net of the prefix" subtracts it from the atlas mean of `tokens.total`, and reproduces the report's token means exactly (207,066 and 245,716 in batch 1).
- **Context packs:** the `atlas_context` tool results in the transcripts give the pack sizes (`~N of 6000 tokens` in the header), the section items and the files they name. These were compared with `ground_truth` in `eval/tasks/<task>/task.toml` and with the session's later Read calls. The "next call after `atlas_context`" counts come from the order of `tool_use` blocks.
- **`t01`–`t10` and shared-task figures:** unweighted means of the per-task rows in `agent.md` and `agent-directed.md`. The `t01`–`t10` figures skip `t00`. The shared-task `first_edit_ms` figures use only the tasks with a value in both conditions.
- **Pack coverage by `must_change`:** each `ground_truth.must_change` path was searched for in each run's `atlas_context` result text.
- **Duplication findings:** `score.json` `duplication.findings` in every run of both batches (the scores of 5.24 to 8.36 that identified the EVAL-7 artefact).
- **Session-process RSS:** `perf-full/perf.json`, records with scenario `session_process`, field `peak_rss_bytes`.
