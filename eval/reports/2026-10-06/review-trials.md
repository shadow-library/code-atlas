# Review trials (IDE operations and retrieval)

- These are manual trials run by independent reviewers during development (2026-10-05/06), on copies of the named repositories, with real language servers.
- They are not reproducible by an `atlas-eval` command.
- Raw logs were kept in the session scratch space and are not part of the repository.

## 1. Rename trials, hono 4.13.13

typescript-language-server 5.1.3, TypeScript 5.9.3. The baseline is `tsc --noEmit` with `types: []` overrides: build 8 errors, spec 11,199. "Added" means new errors after the trial.

### Round 0 (first implementation)

| # | symbol (kind) | edits | files | verify sites | tsc build | tsc spec |
|---|---|---|---|---|---|---|
| t01 | handleMiddleware (fn) | 2 | 2 | 12 | 0 | +16 |
| t02 | deepMerge (fn) | 4 | 2 | 3 | 0 | +1 |
| t03 | wrapTime (fn) | 2 | 2 | 3 | 0 | 0 |
| t04 | HonoRequest (class) | 16 | 4 | 40 | 0 | +1 |
| t05 | JSXFragmentNode (class) | 4 | 2 | 0 | 0 | 0 |
| t06 | DEFAULT_OPTIONS (const) | 3 | 1 | 3 | 0 | +1 |
| t07 | SSEStreamingApi.writeSSE (method) | 2 | 1 | 24 heuristic | 0 | +23 |
| t08 | HonoRequest.query (overloads) | 22 | 11 | 6 | 0 | 0 (confounded) |
| t09 | DetectorOptions.order (interface member) | 4 | 1 | 0 | 0 | +1 |
| t10 | useRef (overload set) | 12 | 3 | 0 (false "(none)") | 0 | +1 |

Result: 7 of 10 broke test-file compilation. The cause was that tsserver only searched loaded projects. Also, safe_delete `StreamingApi.sleep` left 7 TS2339 errors.

### Revision 1

Fresh copy; 10 original plus 9 new random symbols (n01–n09), plus 2 variants of refused renames (t08b, n06b) and 1 of a refused safe_delete (n09b). Determinism compares a fresh server against a server after unrelated reads.

| # | symbol | outcome | edits/files | tsc build | tsc spec | deterministic |
|---|---|---|---|---|---|---|
| t01 | handleMiddleware | applied | 14/3 | 0 | 0 | same |
| t02 | deepMerge | applied | 7/3 | 0 | 0 | same |
| t03 | wrapTime | refused (3 unverified via alias); applied with allow_unverified | 2/2 | 0 | 0 | same |
| t04 | HonoRequest | applied | 56/5 | 0 | 0 | same |
| t05 | JSXFragmentNode | applied | 4/2 | 0 | 0 | same |
| t06 | DEFAULT_OPTIONS | applied | 6/2 | 0 | 0 | same |
| t07 | SSEStreamingApi.writeSSE | applied | 25/3 | 0 | 0 | same |
| t08 | HonoRequest.query→searchParam | refused (false positive) | – | – | – | same |
| t08b | HonoRequest.query→readQuery | applied | 25/13 | 0 | 0 | – |
| t09 | DetectorOptions.order | applied | 5/2 | 0 | 0 | **different** (fresh: 1 file, misses a test) |
| t10 | useRef | applied | 25/4 | 0 | 0 | same |
| n01 | HTTPException | applied | 64/21 | 0 | 0 | same |
| n02 | Router.add (6 implementers) | applied | 189/17 | 0 | 0 | same |
| n03 | JSONPrimitive (type alias) | applied | 6/1 | 0 | 0 | same |
| n04 | WSContext.send (generic class) | applied | 9/4 | 0 | 0 | same |
| n05 | toSSG (re-exported) | applied | 48/5 | 0 | 0 | – |
| n06 | buildStyleString→styleString | refused (correct conflict) | – | – | – | same |
| n06b | buildStyleString→composeStyle | applied | 3/1 | 0 | 0 | – |
| n07 | Trie | applied | 5/2 | 0 | 0 | same |
| n08 | bufferToString | applied | 4/2 | 0 | 0 | same |
| n09 | safe_delete IS_CSS_ESCAPED | refused (bug: export const span) | – | – | – | – |
| n09b | safe_delete DragEvent | applied | 1/1 | 0 | 0 | – |
| extra | safe_delete StreamingApi.sleep | refused, 7 blockers | – | – | – | – |

### Revision 2 (fixes for the remaining refusals and nondeterminism)

| # | operation | outcome | edits/files | tsc build | tsc spec | deterministic |
|---|---|---|---|---|---|---|
| rb1 | replace_body bufferToString, full declaration | applied | 1/1 | 0 | 0 | – |
| rb2 | replace_body buildStyleString, full declaration | applied | 1/1 | 0 | 0 | – |
| sd1 | safe_delete IS_CSS_ESCAPED | applied | 1/1 | 0 | 0 | – |
| t03 | rename wrapTime | applied (alias sites informational) | 2/2 | 0 | 0 | – |
| t08 | rename HonoRequest.query→searchParam | applied | 25/13 | 0 | 0 | – |
| t09 | rename DetectorOptions.order | applied | 5/2 | 0 | 0 | same |
| x1 | rename isOldBunVersion (export let) | applied | 6/3 | 0 | 0 | – |
| x3 | rename rawCssString | applied | 3/3 | 0 | 0 | – |
| x2 | replace_body `__STATIC_CONTENT_MANIFEST` (declare const) | applied with the reviewer's own text | 1/1 | +1 | +1 | – |

The x2 errors come from the type the reviewer wrote, not from the edit. A later narrow fix made three more cases refuse instead of writing broken code:

- a test project including the anchor through `../`;
- a destructured const;
- a declaration keyword on the previous line.

## 2. Rename trials, this repository

rust-analyzer 1.97.1; `lsp.exclude` = `fixtures/**,crates/*/tests/pin-corpus/**,eval/**`; baseline `cargo check --workspace --all-targets` clean.

| # | symbol (kind) | edits | files | cargo check |
|---|---|---|---|---|
| r1 | SimilarTruth (struct) | 4 | 2 | pass |
| r2 | LlmProvider::complete (trait method + 5 impls) | 28 | 7 | pass |
| r3 | inferred_edge (pub fn, cross-crate) | 7 | 3 | pass |
| r4 | UseBinding::Alias (enum variant) | 7 | 5 | pass |
| r5 | UnusedScore.reported_possibly (field) | 5 | 3 | pass |

Without `lsp.exclude`, rust-analyzer reports unhealthy (fixture crates fail `cargo metadata`). Reads are then marked PARTIAL and edits refuse. That is why the `eval/**` glob is listed; it is harmless if nothing matches.

## 3. `similar` from draft code, held-out

16 functions sampled with a fixed seed: 8 Rust from code-atlas f559c69 and 8 TypeScript from hono 4.13.13 `src/`. Index of 23,459 symbols, limit 10. Numbers after revision 1:

| Draft | Threshold | R@1 | R@5 | Original found by structure | Name-only rows/query | Name-only precision |
|---|---|---|---|---|---|---|
| original | 0.5 | 15/16 | 16/16 | 16/16 | 1.94 | 13% |
| original | 0.3 | 15/16 | 16/16 | 16/16 | 1.94 | 13% |
| light paraphrase | 0.5 | 15/16 | 16/16 | 16/16 | 3.06 | 6% |
| light paraphrase | 0.3 | 15/16 | 16/16 | 16/16 | 3.06 | 6% |
| realistic re-implementation | 0.5 | 8/16 | 12/16 | 0/16 | 3.25 | 25% |
| realistic re-implementation | 0.3 | 10/16 | 13/16 | 3/16 | 3.12 | 22% |

Before the name-only noise rule, name-only rows were 6.4 / 6.9 / 7.7 per query at 2.8% relevance beyond the original. The rule (floor 0.6 × best, at most 5) was tuned on this same set, so the name-only rows above are not a held-out measure of the rule. The structural columns are held-out.

## 4. Reuse suggestions from task prompts (dropped)

- Round 0, loose matching: reuse recall 0.537 → 0.704 on held-out, at about 5% precision (4 of 73 lines).
- Revision 1, tight matching: 0 of 7 new lines were true hits on held-out, and 0 of 3 on dev.
- This path was removed.
