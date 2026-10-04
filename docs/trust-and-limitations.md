# Trust and limitations

This document states what Code Atlas can and cannot conclude. Command output below was produced on the fixtures at commit `a03744e` (2026-10-04); the fixtures are in `fixtures/` and their expected results in `fixtures/*/expected.toml`.

## Principles

1. Every fact carries one certainty class. Only `compiler_verified`, `syntactic`, `resolved` and `declared` facts are stated as known; everything else is tagged and hedged. See the [knowledge model](knowledge-model.md#certainty-classes).
2. "Authoritative" is not "complete". A `resolved` relation is exact under the language rules Code Atlas emulates. The set of relations is never claimed to be exhaustive.
3. A derived fact is never stronger than its weakest input. Absence ("unused", "no consumers", "unmatched") is capped at `heuristic`.
4. Uncertainty is surfaced, not hidden: unresolved references that could reach a target, parse failures, skipped files, hidden low-confidence relations and degraded freshness appear as caveats or headers.
5. LLM output is never mixed with analysis output. It is stored as `inferred`, with provider, model, rationale and evidence, and goes stale when the evidence changes.

## Deterministic, heuristic or inferred, per feature

| Feature | Class of the answer | Basis |
|---|---|---|
| Symbols, signatures, docs, visibility, containment | deterministic (`syntactic`) | tree-sitter parse of one file |
| Imports, re-exports, module tree, `implements`/`extends` | deterministic (`resolved`) | emulated Rust and TS resolution rules; `heuristic` through `dist-to-src` |
| Calls and references with a unique target | deterministic (`resolved`) | scope, import and type resolution |
| Calls through trait objects, generics without bounds, interfaces, overrides | heuristic (0.3 candidates) | dispatch candidates |
| Method calls on receivers of unknown type | heuristic (0.25 to 0.5) or unresolved | name-only matching; ubiquitous names (`push`, `map`, `then`) stay `ambiguous` |
| Calls inside macros | heuristic (0.6) or unresolved | token scan; macro-generated names are never resolved |
| SCIP-confirmed edges | deterministic (`compiler_verified`) | rust-analyzer / scip-typescript on identical file contents |
| Endpoints with literal routes | deterministic (`syntactic`/`resolved`) | decorators and registration calls |
| Endpoints with constant-folded or concatenated routes | heuristic | string evaluation; confidence reflects the folding |
| Endpoint composition across routers, prefixes, mounts | heuristic (link pass) | `mounts` edges plus prefix composition |
| Client call to endpoint (`calls_endpoint`), auth support and use | heuristic | route matching, base-URL evidence from names, guard and header classification |
| Env var and config reads | deterministic for literal names | syntax; getter-based config reads are heuristic |
| Tests for a symbol | direct: `resolved` calls; file coverage: heuristic 0.6 | call graph plus naming convention |
| Impact, callers, callees | as strong as the edges traversed; uncertain paths flagged `UNCERTAIN` | graph traversal with per-hop certainty |
| Unused code | heuristic (`likely` / `possibly`) | absence of inbound edges, with exclusions and caveats |
| Events, jobs, DB tables and queries | heuristic, experimental | bus-word and library heuristics, SQL literal parsing |
| Search | lexical ranking (BM25, field-weighted) | scores are deterministic, relevance is a lead |
| Near-duplicates (`similar`) | deterministic score, no conclusion | MinHash/LSH over normalized tokens; "behaviour unverified" |
| Equivalence verdicts | inferred | only an LLM pair classification speaks to behaviour |
| Lineage (`renamed_from`) | heuristic | body hashes and similarity |
| Context packs, patterns | ranking heuristics | selection, not claims; every item states why it was included |

## Where Code Atlas can and cannot conclude

Each case maps to an adversarial fixture under `fixtures/adversarial/`, whose `expected.toml` pins the required behaviour and which the quality evaluation scores.

### Dynamic dispatch (`dynamic-ts`, `trait-dispatch-rust`)

`table[methodName]()`, `target[name]()`, `require(variable)`, template-literal `import()` and `eval` are not resolved and no edge is invented; each is stored as an unresolved `dynamic` reference. Callers queries name them:

```
$ code-atlas -p adv callers stop
callers of stop function dynamic-ts:src/handlers.ts:5 depth=1
  none found over calls, instantiates
caveats:
  2 dynamic member calls could reach this (computed property access)
    dynamic-ts:src/dispatch.ts:5 table.[methodName]
    dynamic-ts:src/dispatch.ts:9 target.[methodName]
```

Conclusion allowed: "no static caller". Not allowed: "no caller". For Rust, generic bounds, `impl Trait` and `dyn Trait` receivers resolve to the trait method (`resolved`) because the call target is the trait method; which implementation runs is not decided, and implementation candidates are `heuristic` at 0.3.

### Macros (`macro-heavy-rust`)

Functions generated by `macro_rules!` (`define_ops! { add => + }`, `getter!(x, i64)`) have no symbol, so calls to them stay `unsupported: macro_generated` and are never resolved. Macro bodies that do not parse as expressions are token-scanned and their references are capped at `heuristic` 0.6. Callers and impact results can therefore miss callers inside macros and callees that macros define. The `callers` query lists unresolved references with the target's name; `callees` and `impact` do not yet list the macro-generated references of the target itself (see [known limitations](#search-context-queries)).

### Generated code (`generated-code`)

Files with an `@generated` or `Code generated ... DO NOT EDIT` comment near the top, or a `.generated.` or `.g.` segment in the file name (`api.generated.ts`, `parser.g.rs`), are indexed and flagged `generated = true` (they are not skipped, because agents ask about them). Rust `include!(concat!(env!("OUT_DIR"), "/x.rs"))` is not followed: the included items do not exist in the graph. The quality evaluation records this reference as external `std` rather than `generated_code` (one non-passing expectation of 15), which is a classification detail, not a fabricated edge.

### Computed routes and URLs (`computed-routes-ts`)

Constant propagation and string concatenation produce endpoints at `heuristic` certainty (`POST /api/items [heuristic 0.60]`); literal routes are `resolved` or `syntactic`. Fully dynamic routes produce no endpoint, only a `route-dynamic` diagnostic. Fully dynamic client URLs produce no client call, only an `http.dynamic_url` diagnostic with the call span. Routes registered on a router that is not mounted in the same file are `router_relative` until the link pass composes them through `mounts` edges; clients match only composed or absolute routes.

### Cross-repo HTTP (`acme`, `globex`)

`calls_endpoint`, `supports_auth` and `uses_auth` are always `heuristic`. A client is matched to a repo only when every non-generic token of one of the repo's names appears in the base-URL source (env var, config key, identifier or host); generic tokens (`api`, `web`, `service`, `url`, `v1`) never name a repo. A direct `fetch` whose path matches a parameterized route only at runtime is a weak candidate:

```
consumer useMe function acme-web:src/hooks/useMe.ts:7  possibly calls endpoint GET /users/{id} [heuristic 0.47]
```

(`useMe` fetches `/users/me`, which is the same route template; whether the server treats `me` as an id is a runtime fact.) Calls that match no endpoint are listed as unmatched at heuristic 0.5. Clients outside the project (browsers, other services, mobile apps) are invisible, so "no consumers" always carries "external clients may exist". Environment definitions (`.env`, compose, Helm) are not indexed, so env values and unused-env detection are unavailable.

### Unused-code caveats

`query unused` returns candidates only. A symbol is "likely unused" when it has no inbound edge at any certainty and is not exported beyond its application package; "possibly unused" when external use is plausible (public API of a library package, endpoints with no in-project consumer, auth schemes no client sends, event handlers a message runtime may invoke). Constructors, `main`, route handlers, trait and interface implementations and framework-invoked methods are excluded and counted. Reflection, dependency-injection containers, serialization, build scripts and consumers outside the project can hide uses. Verify before deleting.

### Similarity is not equivalence (`similar-not-equivalent`)

The fixtures hold pairs with the same shape and different behaviour (`roundHalfUp`/`roundHalfEven` differ on exact .5 ties; `isEvenMod`/`isEvenBit` differ on non-integers because `n & 1` truncates to int32), the same name and different behaviour (`normalize` for user input vs paths), and different shape with the same behaviour (`sumLoop`/`sumReduce`; `toCents`/`parseMoney` in the acme fixture). Deterministic similarity can only say what matches in structure and vocabulary:

```
roundHalfEven function similar-not-equivalent:src/rounding.ts:6 [ts]
  evidence: lexical 6.80 (round half valu numb), same signature (number) -> number
  verdict: same signature shape and related name, behaviour unverified
caveats:
  similarity is evidence, not a decision: only an inferred equivalence verdict speaks to behaviour
```

Deterministic analysis never emits `equivalent_to`. Small bodies (fewer than 40 tokens) are held to a minimum structural score of 0.85, and about a third of small-body structural pairs are trivial (enum match tables, constructors, projections), so a match is a candidate for review, never proof of reuse. Behaviourally equivalent but structurally different pairs are not found by structure at all; only an LLM verdict can relate them, and no verdicts exist in the published fixture runs.

## Known limitations

Concrete items, grouped by area. Source: reviewer findings accepted as known gaps, the `atlas-resolve` contract notes, and the follow-up backlog. Items marked *fixed* in the backlog are not listed.

### Resolution (Rust)

- Function-local items (`struct`, `enum`, `trait`, `type`, `mod`, `fn` declared inside a function body) are invisible to the analyzer. A name that may be shadowed by such an item resolves at most `heuristic 0.5` (`fn-local-item-possible`), and a local type in a body longer than 4,096 tokens is not detected. Two narrow variants of wrong resolution were accepted as residual gaps after review.
- `let x = T::new()` leaves `x` with an unknown type (analyzer emits no hint), and method chains on expressions (`(expr)` heads) are `dynamic`. Rust method-call recall is the weakest edge class; improvement via return-type propagation is not implemented.
- `self` and `Self` inside an `impl` nested in a method body are unknown, so those calls and field accesses are at most name-only heuristics at 0.5 and may pick the outer type's member (backlog RESOLVE-4, open).
- Impl-level generic bounds are not recorded (names only), so such receivers are unknown. Bindings in fn items nested in a body keep capped hints (`param-type-recovery` 0.7).
- Field accesses through `Deref` impls and tuple-struct fields (`.0`) are not resolved. Methods of derived std or serde traits on project types are `external`.
- Token scanning of macro bodies drops calls on the left of `=>` guards in `select!` arms (RECALL-1).
- Resolution is not rustc: `cfg` gating is recorded but not evaluated, `#[path]` and multi-crate reachability follow documented rules only, and proc macros are opaque.

### Resolution (TypeScript and JavaScript)

- `this` inside object-literal methods and `function` expressions is treated as the enclosing class until the analyzer marks it unknown.
- Declaration files (`.d.ts`), `node_modules` types and path aliases outside `tsconfig` are not resolved through the compiler; external types are `external`.
- Without `node_modules` (or `tsconfig.json`), SCIP external types resolve poorly, local edges are unaffected. A hoisted monorepo `node_modules` above a package without `tsconfig.json` is not seen by the SCIP overlay (SCIP-1).
- Dynamic `require` and `import()` are unresolved by design.

### Detectors

- Frameworks other than those in [languages and frameworks](languages-and-frameworks.md) produce no endpoints, clients or auth schemes; unknown HTTP clients are invisible.
- `SELECT * FROM a, b` reports only `b` as a table (DETECT-1).
- Events assume one bus and DB tables one database per project. Event and job detection are heuristic and flagged experimental.
- Auth classification is by library, header and name conventions; a guard named unconventionally is "unclassified" with `no-detected-auth-scheme` 0.5.

### Cross-repo linking

- Matching depends on repo, package and application names appearing in base-URL evidence; a base URL taken from a config value that is not in the index (env files, secrets, service discovery) cannot be matched.
- Only HTTP, package dependencies, auth, events and config are linked. gRPC, GraphQL, message schemas and shared database tables are not.

### SCIP

- Production SCIP runs rust-analyzer once per Cargo package manifest, not once per workspace, so cross-crate edges between workspace members may not be `compiler_verified` (SCIP-2). The quality evaluation indexes workspaces at the root to confirm cross-crate edges, which production does not yet do.
- rust-analyzer builds the workspace (build scripts): about 2 GB RSS and tens of seconds cold. scip-typescript needs `npx` and either a cached package or network access.
- SCIP covers `calls`, `references`, `instantiates`, `implements` and `extends` only, and only in files the indexer covered. A file changed after the last SCIP run is lagging and its edges stay at `resolved`. `scip.enabled` and `scip.debounce_secs` are stored by `project config` but nothing reads them: SCIP runs only on `code-atlas scip` regardless of `--scip off`, and never automatically.

### Similarity and LLM

- Structural similarity misses equivalent code of a different shape; the LLM candidate generator only receives structural clusters of 0.6 or more, so those pairs never get a verdict (SEM-1). Lexical or signature based candidate generation is not implemented.
- `enrich` runs similarity-pair classification and revalidation. Capability summaries and client-call linking exist as task types in `atlas-semantic` but are not wired into `enrich`, and the watcher does not trigger enrichment.
- Cost figures are estimates from a built-in price table of unverified list prices (Sonnet and Haiku families); unknown models have no estimate.

### Search, context, queries

- Search is lexical. A comment-free code base gives BM25 only identifiers and signatures, so conceptual queries ("detect racily clean files") may rank vocabulary matches above the real logic (QUERY-4 improved grouping; recall depends on naming).
- Context packs are budgeted selections. Items "likely to edit" show only an outline; read those files directly.
- `callees` and `impact` do not list the target's own unresolved outgoing references. On `macro-heavy-rust`, `callees Point::manhattan` prints `none found` with no caveat, although the index stores three `macro_generated` references from that method (`add`, `self.x`, `self.y`). `callers` does list unresolved references that carry the target's name.
- The search group heading "exact name" over-claims: hits that match only one query token (for example `Order.userId` for "user lookup") appear under it.
- Entity listings carry no certainty tag, so the guidance "untagged results are authoritative" does not cover them; only relation lines are tagged.
- The rendered tag for compiler-verified edges is `[compiler]`, while the MCP server instructions say `[compiler_verified]` (the JSON `class` is `compiler_verified`).
- `atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not MCP tools at this commit.

### Indexing scale and cost (measured before the performance round)

- Evidence carries a per-edge content hash, so any byte change in a file rewrites every edge it owns (tens to hundreds of edges per one-file edit); a package-graph change re-resolves and re-persists every owner.
- Database size (about 250 KB per file at last measurement) and per-file artifact blobs grow linearly; the similarity index costs about 300 MiB per 100k symbols.
- Persist dominates cold indexing. A cross-process generation change forces a full state reload in every other open engine.
- The persisted skipped-file list can be stale after a crash (report-only, ENGINE-6).

Current numbers are in `docs/evaluation/results.md` once published.

### Unsupported

- Languages other than Rust, TypeScript, JavaScript, TSX and JSX (extensions `rs ts mts cts tsx js mjs cjs jsx`). No Python, Go, Java, C#, C/C++, PHP, Ruby.
- Projects of more than the repos you register; there is no remote or hosted index. Hidden files, `.gitignore`d files, symlinks, files over `max_file_bytes`, and the directories `node_modules target dist build .git .next coverage vendor` are not indexed unless an `include` pattern in `projects.toml` names a default-excluded directory.
- Windows: not supported in practice. `watch` and the SCIP and LLM subprocess handling use unix signals and process groups, and nothing is tested on Windows.
- Runtime behaviour: no profiling, tracing or coverage data; no environment values; no infrastructure (Terraform, Kubernetes) analysis.

## Failure modes and how they surface

| Situation | Where it shows | Effect on answers |
|---|---|---|
| Index could not be brought up to date | first line `DEGRADED: <reason> run=...` with `affected <repo:path>: <reason>` lines; CLI exit code 3; JSON `freshness.state = "degraded"` with `failed_files` | answers printed from the last consistent state; verify against files |
| File unreadable, not UTF-8, or analyzer failure | `DEGRADED` as above; file's facts are removed (queries return "not found" for its symbols) | relations through that file are missing |
| Syntax error | `parse ok N, partial M` in `status`; caveat `N files in scope parsed partially or failed; their relations may be missing` on queries that touch the file | facts around the error are extracted; some relations may be missing |
| File skipped (too large, symlink, non-UTF-8 path) | header `skipped N files (see status)`; `status` lists each with its reason | file absent from the graph; raise `--max-file-bytes` via `project config` if intended |
| Partial run (project pass failed or panicked, dependency rounds did not converge) | `status` recent runs show `partial (<reason>)`; next run re-resolves everything | pass-owned facts (link, structure) missing until the next successful run |
| Project configuration changed under a running engine | `DEGRADED: project configuration changed; reopen the engine` | `watch` and `mcp` reload automatically; one-shot CLI reopens |
| Crashed run | recovered at next lock: orphaned runs marked `failed`, all files re-resolved | none after recovery |
| SCIP behind | `status`: `scip: current (47/58 files covered)` or behind N; JSON `scip.state` | edges of lagging files stay `resolved`/`heuristic` |
| SCIP indexer unavailable, timed out, or network failure | `code-atlas scip` reports per package; `doctor` shows tool availability | no `compiler_verified` upgrade for that package |
| LLM provider unavailable or over budget | `enrich` prints skipped task counts, run recorded `partial`, exit code 3; `status` shows the run | no new inferred facts |
| Stale inferred facts | tag suffix `stale` or `needs-revalidation`; `status` counts; hidden unless `--include-stale` | excluded from default results |
| Target not found or ambiguous | `not found: "<target>" (...)` or candidate list; exit code 4 | try `search`; an empty result is not proof of absence |
| Query returned nothing | caveats listing unresolved references, dynamic accesses and problem files that could hide relations | read the caveats before concluding |

Exit code 4 takes precedence in the CLI when nothing matched, even if the index is also degraded; read the `DEGRADED` line in the output.
