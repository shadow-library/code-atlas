# Dogfooding log

Curated findings from using Code Atlas on its own repository and fixtures. Entries are dated; "status" says what happened afterwards, checked at commit `a03744e` unless noted. This is anecdotal evidence from single sessions, not a measurement; measured results belong in `results.md`.

## 2026-10-04 first self-index (commit `327f4dc`)

Positive

- Indexed 452 files in 1.1 s, 430 MB peak RSS (release build). `status` shows a certainty histogram per edge kind, which made it obvious which relation classes were exact and which heuristic.
- `impact Certainty::is_authoritative` returned 3 resolved direct callers and 7 heuristic name-only callers (confidence 0.5) flagged `UNCERTAIN`, plus transitive dependents with the path explaining each. Useful: the uncertain callers were visibly separated from the certain ones.

Negative

- `search "detect racily clean files"` put single-token name matches (`RegressionVerdict::Clean`, `Plan::Clean`) in the top "exact name" group and ranked the real logic (`atlas-project` `changes.rs`, `Tree::detect`) third. The code base has almost no comments, so BM25 sees only identifiers. Status: the grouping was tightened in `35ea452`; at `a03744e` the same query returns a "related" group (`Engine::settle_clean`, `Plan::Clean`, `WatcherClean`, ...). Recall for conceptual queries still depends on identifier naming.
- The self repo includes `fixtures/` (acme and globex TS), so endpoints and clients appear in the code-atlas project graph and add noise to `api` and `overview`. Accepted: the tests use them. Exclude `fixtures/**` in `projects.toml` for a cleaner view.

## 2026-10-04 agent harness smoke run (`t00-smoke`, haiku, one run per condition, about $0.59)

Positive

- The pipeline worked end to end: isolated trees, MCP connected, hidden tests, regression check, metrics, blinded judge, atlas review.
- The atlas run made 3 Code Atlas calls (overview, context, read; 10.8 KB) and read `users.controller`, the file the hidden check depended on (`must_consider` coverage 1.0 against 0.5 for baseline).

Negative

- Both conditions failed the same hidden check (a column name `deleted_at` not set): model capability, not the index.
- The atlas run used more turns and about 5x the tokens of baseline (including cache reads) on a one-file task, and ran `npm install` on its own.
- The agent called `atlas_read` on a file and then the native `Read` on the same file, because Claude Code requires a read before an edit. Context was duplicated, and `context` MUST READ bodies duplicated the following `Read` for files being edited. Status: fixed in `35ea452` (descriptions of `atlas_read`, `atlas_context` and the server instructions now say to read files you will edit natively; "likely to edit" items show an outline).

## 2026-10-04 end-to-end and adversarial suite findings

Negative (all fixed before this documentation pass)

- `callers` and `impact` gave no caveat when the target was reachable through `table[methodName]()`; computed member accesses never name-match. Status: fixed in `35ea452`; `callers stop` on `dynamic-ts` now lists the two dynamic member calls.
- `api` listed router-relative fragments (`POST /`, `GET /{id}`) beside composed routes. Fixed in `35ea452`.
- Oversized files were silently dropped; `enrich` without an LLM recorded an `ok` run; `status` leaked Rust `Debug` formatting (`Some(NeverRun)`). Fixed in `83c4b1d`: skipped files are listed, enrich records `partial`, text is rendered.
- Rust field accesses were not references (27% reference recall against SCIP) and trait-dispatch receivers through generic bounds were capped at heuristic 0.7. Fixed in `924e9e1` (field references, declared generic hints); the quality evaluation's `trait-dispatch-rust` case went from 6/8 to 8/8 expected edges.
- Small helpers were invisible to similarity (bodies under 40 tokens). Addressed in `8f87e00` with a stricter minimum score (0.85) and a "small body" caveat; about 31% of small-body pairs are trivial.

## 2026-10-04 documentation verification run (`a03744e`, `fixtures/acme`, scratch `CODE_ATLAS_HOME`)

Positive

- Self-index of the repository: 530 files in 1.7 s, 528 MB peak RSS (release build). The impact query on `Certainty::is_authoritative` again split resolved callers from `UNCERTAIN` name-only ones.
- `api` on acme renders every cross-repo link hedged (`possibly calls endpoint GET /orders/{id} [heuristic 0.77]`) and the weakest one honestly low: `useMe`, whose runtime path `/users/me` matches `GET /users/{id}` only structurally, scored `[heuristic 0.47]`.
- After `code-atlas scip`, the same `callers UsersService.findOne` query went from `[resolved]` to `[compiler]` on all four callers (acme: 63 `compiler_verified` calls edges).
- Degraded paths surface: with `max_file_bytes` lowered, the header said `skipped 1 files (see status)` and `status` named the file and sizes; an unreadable file produced `DEGRADED: 1 file(s) or repo(s) failed` with the reason and its symbols disappeared rather than going stale.
- `query similar roundHalfUp` returned `behaviour unverified` and a caveat that similarity is evidence, not a decision.

Negative (status rechecked at `6e68c91`)

- `callees Point::manhattan` on `macro-heavy-rust` printed `none found` without a caveat, although three `macro_generated` unresolved references from that method are stored (`debug dump-file`). `callers` does list unresolved references that carry the target's name; `callees` and `impact` do not list the target's own. Status: fixed in `bbbd680`; `callees`, `dependencies` and `impact` now list the three names as possibly macro-generated.
- `search "user lookup"` on acme returned nine "exact name" hits that matched only the token `user` (fields like `Order.userId`, a hook `useUser`). The group heading overstates the match. Status: fixed in `bbbd680` (whole-token equality required; the same query now shows a "strong" group).
- `project config --scip off` is accepted and stored, but `code-atlas scip` still ran. `scip.enabled` and `scip.debounce_secs` are not read by anything. Status: fixed in `4a3dc42`; `scip.enabled` is read, opt-in and default off, and applies to automatic runs only (explicit `code-atlas scip` always runs).
- The committed quality report (`eval/reports/2026-10-04/quality.md`, 96/98 edges) is older than the code; re-running at `a03744e` gives 98/98. Regenerate before quoting. Status: regenerated in `14124c2` (98/98).
- The `connect claude` guidance block says "untagged results are authoritative", while some lines (entity listings) carry no tag at all, and `[compiler]` is the rendered tag whereas the MCP server instructions refer to `[compiler_verified]`. Wording to reconcile. Status: fixed in `bbbd680`; every uncertain entity is tagged, tags render `[compiler]` everywhere, and the guidance explains entity versus relation tags.

## 2026-10-04 to 2026-10-05 evaluation phase (`6a2854f` to `14d2b44`)

Measured results are in [results.md](results.md). These are the lessons from running the evaluation.

Negative

- **Producer versions were not bumped.** While preparing the evaluation, we found that the resolver, analyzers and detectors had changed their output over many commits without a version bump. A database built by an older binary therefore kept stale facts until each file changed, and its incremental output no longer matched a full index. Project passes had no version at all. Status: fixed in `6a2854f`.
  - The resolver, both analyzers, `config-env` and `link` were bumped.
  - Project passes are now versioned.
  - Each producer's output is pinned over fixtures and in-crate corpora, so output drift over unchanged inputs fails a test.
  - The lesson: "incremental equals full" was tested within one binary, never across binaries.
- **Report drift.** Against the previous report, the regenerated quality report listed 17 regressed SCIP-oracle metrics. All 17 came from the oracle target, which is this repository: it grew from 379 to 443 `.rs` files in between. Scored on the same frozen tree, old and new binaries were identical except heuristic calls (−0.0016, commit `14124c2`). Report comparisons against a moving tree need a same-tree check before anyone reads them as regressions.
- **Adoption.** In the primary A/B batch, planned in advance in implementation plan §3.3, the agent called Code Atlas twice in 22 sessions, both times in the smoke task. The server was connected and all 16 tools were listed in every session, but the agent used Grep and Read instead. A three-sentence preamble that names the tools and says to use them before reading files is not enough.
- **Fixed cost of the server.** Offering the server adds about 6,300 prompt tokens to every model request, almost all of it tool definitions and server instructions. That cost alone explains the atlas condition's +18.7% tokens in batch 1, where the tools were unused in `t01`–`t10`.
- **Ceiling.** All 88 A/B sessions across both batches passed their hidden tests. The tasks did not separate the conditions on correctness.
- **Context packs.** With a directed preamble, 21 of 22 sessions called `atlas_context`. Every one of them then searched or read natively as its next step.
  - In `t00`, the seed `User` was dropped as ambiguous, and keyword hits (`OrdersController`, delete handlers) filled MUST READ.
  - In `t03` and `t07`, the file to edit was not in the pack.
  - In `t10`, the pack was empty apart from a parse caveat.
  - Measured by `must_change`, packs held all the files to edit only in `t01` and `t05`, and some of them in five other tasks.
- **Duplication metric.** The A/B duplication metric falls back to a lexical BM25 score and compares it with a Jaccard threshold. Every finding is a lexical score of 5.2 to 8.4 reported as structural. They come from 4 runs, all in `t07`.

Positive

- **Accurate packs.** When a pack was relevant (`t05`, `t02`), the unblinded reviews found it accurate, and atlas runs read fewer bytes (`t05`: 13,655 against 30,614). Only 1 of 21 reviewed packs was judged misleading, for noise rather than wrong facts. These reviews are noisy, though: the identical `t00` r1 pack was judged not misleading.
- **Clean harness runs.** The harness ran 88 sessions with 0 contaminated runs and recorded the binary's build commit and hash in each report.
