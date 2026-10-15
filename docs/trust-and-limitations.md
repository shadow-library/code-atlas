# Trust and limitations

This document states what Code Atlas can and cannot conclude. Command output below was produced on the fixtures at commit `a03744e` (2026-10-04); the fixtures are in `fixtures/` and their expected results in `fixtures/*/expected.toml`. Where a number comes from a review rather than a checked-in report, it says so.

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
| Exact references, declaration, implementations, diagnostics, and outline with `source=lsp` (`atlas_symbol`, `refs`, ...) | `[compiler]`, live, never stored | a language server's answer for the configuration it loaded; `PARTIAL` when its view is incomplete |
| Rename, safe delete | exact for what the server sees; the rest is listed, not edited | language-server edit plus graph sites the server did not return |
| Symbol edits (`replace_body`, `insert_*`) | syntactic spans bound to the indexed hash | tree-sitter spans plus a leading-doc and attribute scanner; no server |
| Endpoints with literal routes | deterministic (`syntactic`/`resolved`) | decorators and registration calls |
| Endpoints with constant-folded or concatenated routes | heuristic | string evaluation; confidence reflects the folding |
| Endpoint composition across routers, prefixes, mounts | heuristic (link pass) | `mounts` edges plus prefix composition |
| Client call to endpoint (`calls_endpoint`), auth support and use | heuristic | route matching, base-URL evidence from names, guard and header classification |
| Env var and config reads | deterministic for literal names | syntax; getter-based config reads are heuristic |
| Tests for a symbol | direct: `resolved` calls; file coverage: heuristic 0.6 | call graph plus naming convention |
| Impact, callers, callees | as strong as the edges traversed; uncertain paths flagged `UNCERTAIN` | graph traversal with per-hop certainty |
| Unused code | heuristic (`likely` / `possibly`); test-only symbols are a third class | absence of inbound edges, with exclusions and caveats; config keys and env vars are not checked |
| Tests that mock a symbol | heuristic (0.6) | `jest.fn()`/`vi.fn()`/`createMock`/`spyOn` patterns; a mock edge is never counted as production use |
| Events, jobs, DB tables and queries | heuristic, experimental | bus-word and library heuristics, SQL literal parsing |
| Search | lexical ranking (BM25, field-weighted) | scores are deterministic, relevance is a lead |
| Near-duplicates (`similar`), including from draft code | deterministic score, no conclusion | MinHash/LSH over normalized tokens; "behaviour unverified"; finds copies, not re-implementations (see below) |
| Equivalence verdicts | inferred | only an LLM pair classification speaks to behaviour |
| Lineage (`renamed_from`) | heuristic | body hashes and similarity |
| Context packs, patterns | ranking heuristics | selection, not claims; every item states why it was included |

## Where Code Atlas can and cannot conclude

Each case maps to an adversarial fixture under `fixtures/adversarial/`, whose `expected.toml` pins the required behaviour and which the quality evaluation scores.

### Dynamic dispatch (`dynamic-ts`, `trait-dispatch-rust`)

`table[methodName]()`, `target[name]()`, `require(variable)`, template-literal `import()` and `eval` are not resolved and no edge is invented; each is stored as an unresolved `dynamic` reference. Callers queries name them:

```
$ catl -p adv callers stop
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

(`useMe` fetches `/users/me`, which is the same route template; whether the server treats `me` as an id is a runtime fact.) A URL whose head is dynamic (`${prefix}/versions/latest`) matches a route in full, or else with up to two leading static route segments taken by the head (rule `base-path-prefix`), and only when the call's base is known and static (a declared `base` or the base of its client definition), any number of leading segments after that base, static or parameter, taken by the head when the route starts with the base's segments and at least two static segments match after the head (rule `dynamic-head-prefix`, confidence ×0.6). Only candidates of the first rule that finds any are kept, and equally good candidates share the call. A head with an unknown base, such as ``fetch(`${process.env.PAYMENTS_URL}/oauth/token`)``, never skips more than two static segments, and a one-segment static tail (`${base}/chapters`) stays unmatched. Calls that match no endpoint are listed as unmatched at heuristic 0.5. Clients outside the project (browsers, other services, mobile apps) are invisible, so "no consumers" always carries "external clients may exist". Environment definitions (`.env`, compose, Helm) are not indexed, so env values and unused-env detection are unavailable.

### Unused-code caveats

`query unused` returns candidates only, in three classes. A symbol is "likely unused" when it has no inbound edge at any certainty and is not exported beyond its application package; "possibly unused" when external use is plausible (public API of a library package, endpoints with no in-project consumer, auth schemes no client sends, event handlers a message runtime may invoke); "used only by tests" when every use is from test code (mock-only references do not count as use). Config keys and env vars are not checked, because `.env` and config files are not indexed. Constructors, `main`, route handlers, trait and interface implementations and framework-invoked methods are excluded and counted. Reflection, dependency-injection containers, serialization, build scripts and consumers outside the project can hide uses. Verify before deleting.

### Similarity is not equivalence (`similar-not-equivalent`)

The fixtures hold pairs with the same shape and different behaviour (`roundHalfUp`/`roundHalfEven` differ on exact .5 ties; `isEvenMod`/`isEvenBit` differ on non-integers because `n & 1` truncates to int32), the same name and different behaviour (`normalize` for user input vs paths), and different shape with the same behaviour (`sumLoop`/`sumReduce`; `toCents`/`parseMoney` in the acme fixture). Deterministic similarity can only say what matches in structure and vocabulary:

```
roundHalfEven function similar-not-equivalent:src/rounding.ts:6 [ts]
  evidence: lexical 6.80 (round half valu numb), same signature (number) -> number
  verdict: same signature shape and related name, behaviour unverified
caveats:
  similarity is evidence, not a decision: only an inferred equivalence verdict speaks to behaviour
```

Draft code you have not saved can be compared too (`similar --code`, `atlas_similar code`). It finds copies and light paraphrases, not independent re-implementations (measured below). Deterministic analysis never emits `equivalent_to`. Small bodies (fewer than 40 tokens) are held to a minimum structural score of 0.85, and about a third of small-body structural pairs are trivial (enum match tables, constructors, projections), so a match is a candidate for review, never proof of reuse. Behaviourally equivalent but structurally different pairs are not found by structure at all; only an LLM verdict can relate them, and no verdicts exist in the published fixture runs.

### Exact operations and edits (language servers)

Reads from rust-analyzer and typescript-language-server (`references`, `declaration`, `implementations`, `outline` with `source=lsp`, `diagnostics`) are stated as `[compiler]` because the compiler front end computed them, but that claim is scoped:

- **Valid for the configuration the server loaded.** A server sees one set of features, targets, `cfg` flags and loaded projects. Code gated out of that configuration, or in a TypeScript project the server did not load, is invisible. They are live answers and are never stored: the graph keeps its own certainties.
- **A partial view says so.** A busy server, a lost document sync, unsettled diagnostics, an incomplete flycheck or a rust-analyzer workspace in `health = error` makes the answer `PARTIAL: ...`. Edits refuse in the last case.
- **Graph sites outside the server's view are listed, not merged.** References add `outside the language server's view` (other repos, mocks, same-repo sites outside the server's project, HTTP consumers), each with its own graph tag.
- **No silent fallback.** With no server the exact modes fail with an install hint. `source="graph"` is explicit and returns tagged, non-exhaustive graph results.
- **Diagnostics after an edit are native errors in the edited files, compared with a baseline.** For Rust that is rust-analyzer's own diagnostics: `cargo check` is not run, and the text never claims a clean project. Files without a settled baseline are reported `PARTIAL`, never clean.
- **TypeScript 7 is not supported** (no `tsserver.js`).
- **Rename determinism is scoped.** For projects in the anchor's repo that contain or reference the anchor file, the plan does not depend on earlier reads (the server is made to load them). TypeScript projects with no link to the anchor's project are not opened, so sites there, for example object-literal properties, can be missed. Sites the server cannot see are listed (`not renamed, verify`, `UNVERIFIED:`, `UNLOADED:`), and apply refuses while same-repo ones remain unless `allow_unverified` is set.
- **Cross-repo rename applies only authoritative graph sites** whose text checks out on disk, all or nothing per file; re-exports, shorthand properties and property keys are never edited. When the server keeps the old name as an alias (`export { new as old }`), importers through the alias are left alone because they keep compiling.
- **Edits are atomic per plan, not durable against a crash.** There is no journal: a process killed mid-commit can leave a mix of old and new files plus `.atlas-edit.*` temp files that the next apply sweeps. A failed write rolls back (exit 6); a failed rollback is reported as partially applied (exit 7) with the files. `rust-analyzer` may rewrite `Cargo.lock`; it is reported, not restored.

Measured evidence is under [Negative results and known gaps](#negative-results-and-known-gaps-ide-operations-and-retrieval) below.

## Known limitations

Concrete items, grouped by area. Source: reviewer findings accepted as known gaps, the `atlas-resolve` contract notes, and the follow-up backlog. Items marked *fixed* in the backlog are not listed.

### Resolution (Rust)

- Function-local items (`struct`, `enum`, `trait`, `type`, `mod`, `fn` declared inside a function body) are invisible to the analyzer. A name that may be shadowed by such an item resolves at most `heuristic 0.5` (`fn-local-item-possible`), and a local type in a body longer than 4,096 tokens is not detected. Two narrow variants of wrong resolution were accepted as residual gaps after review.
- `let x = T::new()` leaves `x` with an unknown type (analyzer emits no hint). Method-call chains over a declared-type head (a typed binding, `self`, or a path call such as `f(..)` or `T::new(..)`, at most 8 steps) are resolved with receiver chains; chains over any other head (index, deref, closure call, literal) and over untyped `let` receivers stay `dynamic`. Rust method-call recall is still the weakest edge class.
- Receiver chains use declared types only. A chain ending in a method that a project extension trait adds to `Option` or `Result` stays `External(std)`; choosing between an inherent and a trait method ignores receiver mode (an inherent `fn m(self)` beats a trait `fn m(&self)` on `&T`), and a chain carries that choice into the next hop; an external glob (`use rayon::prelude::*`) still claims value-namespace names (a unit-struct head) when they are not found. Source: `crates/atlas-resolve/CLAUDE.md`. In the implementer's run on this repository, before the review fixes, the resolver added 742 edges and upgraded 45 with 0 lost, about 130 of them hand-checked correct; review found wrong `resolved` edges from `Rc<T>.borrow()` and `Borrow` import forms, which were fixed (not a checked-in report; final counts were not recorded).
- `self` and `Self` inside a fn, impl or trait item nested in a body stay unresolved (`Unsupported: self in nested item`, no candidates), even when the nested impl is for a module-level type, because a name match could pick the outer type's same-named member. Calls and field accesses through them are missing, not guessed.
- Impl-level generic bounds are not recorded (names only), so such receivers are unknown. Bindings in fn items nested in a body keep capped hints (`param-type-recovery` 0.7).
- Field accesses through `Deref` impls and tuple-struct fields (`.0`) are not resolved. Methods of derived std or serde traits on project types are `external`.
- Token scanning of macro bodies drops calls on the left of `=>` guards in `select!` arms (RECALL-1).
- Resolution is not rustc: `cfg` gating is recorded but not evaluated, `#[path]` and multi-crate reachability follow documented rules only, and proc macros are opaque.

### Resolution (TypeScript and JavaScript)

- A rebound `this` inside a class (local classes, object-literal methods, non-arrow functions) is `Unsupported: rebound this` with no candidates, including functions invoked with the instance as `this` (`forEach(fn, this)`, `fn.call(this)`). Outside a class it keeps name-only candidates. Explicit `this: T` parameter annotations are ignored, so such receivers are treated as rebound `this`.
- Declaration files (`.d.ts`), `node_modules` types and path aliases outside `tsconfig` are not resolved through the compiler; external types are `external`.
- Without `node_modules` (or `tsconfig.json`), SCIP external types resolve poorly, local edges are unaffected. A hoisted monorepo `node_modules` above a package without `tsconfig.json` is not seen by the SCIP overlay (SCIP-1).
- Dynamic `require` and `import()` are unresolved by design.

### Detectors

- Frameworks other than those in [languages and frameworks](languages-and-frameworks.md) produce no endpoints, clients or auth schemes; unknown HTTP clients are invisible.
- `SELECT * FROM a, b` reports only `b` as a table (DETECT-1).
- Events assume one bus and DB tables one database per project. Event and job detection are heuristic and flagged experimental.
- Auth classification is by library, header and name conventions; a guard named unconventionally is "unclassified" with `no-detected-auth-scheme` 0.5. In a declared route family, public and optional routes are known only through `[access]` markers: an unlisted `@Public()` stays an unknown guard, and a listed marker is trusted to bypass the family's own guard decorators, which may in fact ignore it; guard classes named in arguments are kept and decide the requirement. A class-level marker on a method with its own family guard decorator is ambiguous (the decorator may or may not reset the public metadata): the method's guards decide, and the requirement is capped at `heuristic 0.50` (`class-marker-method-guard`). A family can declare `class_marker = "method_wins"` for decorators whose method use replaces the class's access metadata, which removes that cap; the declaration is trusted, not checked. On one level, a marker next to `method_wins` guards is decided by decorator order (the topmost wins, as TypeScript applies decorators bottom-up); next to guards of which only some are `method_wins` it is contested. An option marker (`@Auth({ public: true })`) is read as metadata of its own decorator only: it suppresses that decorator's guard and never another one's, so a framework whose guards share one public flag across decorators is under-reported as requiring login (the safe direction). Class and method options of one guard decorator follow override semantics (the method's replace the class's) unless the family declares `options = "deep_merge"`; a framework whose merge differs from both (lodash-style index-wise arrays, a custom reducer) is misread, and options the detector cannot read (a spread, a named options object) leave a merged class marker contested and merged class permissions at `heuristic 0.50`. Schemes chosen by options (`[guards.option_schemes]`) are exact only for a literal options object setting listed keys that map to the same schemes; keys mapping to different schemes or unreadable options give the union of the candidate schemes at `heuristic 0.50`, and a key set to a binding whose value is falsy at runtime is still read as set.

### Cross-repo linking

- Matching depends on repo, package and application names appearing in base-URL evidence; a base URL taken from a config value that is not in the index (env files, secrets, service discovery) cannot be matched.
- Only HTTP, package dependencies, auth, events and config are linked. gRPC, GraphQL, message schemas and shared database tables are not.

### SCIP

- Cargo workspace members are indexed as one rust-analyzer run at the workspace root, so cross-crate edges can be `compiler_verified`. scip-typescript still runs per npm package: cross-package references inside an npm workspace stay unverified. Standalone crates run per package.
- rust-analyzer builds the workspace (build scripts): about 2 GB RSS and tens of seconds cold. scip-typescript needs `npx` and either a cached package or network access.
- SCIP covers `calls`, `references`, `instantiates`, `implements` and `extends` only, and only in files the indexer covered. A file changed after the last SCIP run is lagging and its edges stay at `resolved`. Automatic SCIP is opt-in (`project config <p> --scip on`) and runs only while a watcher is active, after the debounce; it starts rust-analyzer or `npx scip-typescript` unasked, and a failed package is held for up to an hour unless its inputs change (changes to lockfiles, `rust-toolchain*` or `.cargo/config*` alone do not lift the hold). Without a watcher SCIP runs only on `catl scip`.

### Similarity and LLM

- Structural similarity misses equivalent code of a different shape. `enrich` now also proposes lexical candidates (shared name words, signatures, BM25 neighbours), but verdicts need an LLM and are budget-bound, so many pairs get none.
- `enrich` shares one budget across phases (links up to 1/5, structural pairs next (no fixed share), lexical pairs up to 2/5, summaries up to 1/5 when `--summaries` is given, revalidation the rest). LLM links for unmatched client calls are `inferred` consumers, shown tagged in `api`, `impact`, `unused` and `explain`; they go stale when the client file or the candidate set changes. Only same-method endpoints are offered as candidates. The watcher never triggers enrichment.
- Cost figures are estimates from a built-in price table of unverified list prices (Sonnet and Haiku families); unknown models have no estimate.

### Search, context, queries

- Search is lexical. A comment-free code base gives BM25 only identifiers and signatures, so conceptual queries ("detect racily clean files") may rank vocabulary matches above the real logic (QUERY-4 improved grouping; recall depends on naming).
- Context packs are budgeted selections. Items "named in the task: read the file directly" show only an outline and the lines around the key site; read those files directly. The key site of an item without a relation site is the body line that matches the most task or search terms, a lexical guess. The pack expands at most three graph hops from its seeds (`--depth` in the CLI narrows it), so code further away appears only through search; enclosing types, endpoint handlers and guards, and auth-scheme guards are added without counting as a hop. Line ranges did not make the packs materially more line-precise: line F0.5 stays below 0.1 for MUST READ and below 0.02 for the whole pack on the dev set.
- The serving path of a named endpoint is found only through graph facts, and only when the route decorator's implementation is indexed: the handler's call to the decorator that sits at the endpoint's own site (other decorators on the handler, such as a role check, are not followed), the constants, functions and types that implementation uses inside its own package (three hops), and the other functions that use one of them. Code under `node_modules` (NestJS itself) is not indexed, so a Nest route yields no serving path. A constant or helper with more than six production users (test code and mock edges are not counted) is treated as shared plumbing, not as the decorator's metadata; functions in the decorator's own file and functions that are themselves applied as decorators are skipped. Shared constants and functions count four times as much as shared types, a function's shares are summed, and the three heaviest are added per decorator. Every hop's certainty, the decorator call included, is multiplied into the weight, and a chain with any non-authoritative edge is listed as uncertain. The reason line states the shared symbol and the edge (`references ROUTE_METADATA [resolved]`), not that the function is the registrar. No edge records "registers the routes of" or "wraps the reply of" today; the pack infers that shape from shared references at query time.
- A function joins the pack as "the code that applies the behaviour" only when it calls at least two search hits (each with at most four production callers; only callers through authoritative edges, `syntactic`, `declared`, `resolved` or `compiler`, are added). A single hit's callers are never added, so a cross-cutting change whose primitives are not lexical hits, or are called from many places, still needs search.
- The fan-in discount multiplies a callee's score by `sqrt(4 / users)` when more than four production symbols call, instantiate or reference it (test code and mock edges are not counted; counting stops at 256), and passes that factor on to whatever is reached through it. It applies after ranking, so it removes no relation, but a discounted item can fall below the MUST READ relevance floor (into SKIM) or below the low-relevance cut (into OMITTED as low relevance), and the tests that reach it can lose their slot in TESTS.
- Once the named route exists, at most five other routes of its resource or shape are listed under PATTERN.
- `callees`, `dependencies` and `impact` name unresolved references in the target's own body that may be macro-generated (`callees Point::manhattan` lists `add`, `self.x`, `self.y`), but they cannot say what those names would resolve to.
- Certainty tags: every non-authoritative entity is tagged after its name, relation tags are in parentheses or after the target, and `[compiler]` is the only spelling. JSON carries `class`, `confidence`, `freshness`, `authoritative` and `tag`.
- `atlas_dependents` and `atlas_dependencies` exist in the CLI (`query dependents`, `query dependencies`) but are not MCP tools.
- Mocks: the call graph is path-insensitive, so a controller built with a mocked service shows `[via call graph]` plus a note rather than a verdict; file-level mocks are coarse and reported hedged (`its scope mocks it`); `bun:test` and assignment mocks (`x.m = jest.fn()`) are not detected, so such a mock is under-claimed as spied.
- Unused, test-only class: gaps are multi-line import and export blocks (about 7 hono symbols of recall), a package with an undeclared dependency whose only use is unresolved can list a production-used symbol, classes whose methods are only called through unresolved receivers, and rustfmt `* ` continuation lines are treated as comments.

### Indexing scale and cost

The first bullet's figures come from the schema-v2 change (ccbf4b4). They were measured during development with release builds on a machine loaded by concurrent builds (load average 11 to 19). Measured numbers are in [results.md](evaluation/results.md#performance).

- Rows are compact and the content hash is stored once per owner, so a body edit that keeps spans rewrites no edge (it was 43 to 172 at 1M LOC). At 1M synthetic LOC the cold index went from 24.3 s to 7.8 s and the database from 827 MB to 407 MB. The 2026-10-04 report's 388.3 "MB" is MiB, about 407 MB: the same size in a different unit, not a further reduction. Run-ledger fact deltas now count rewritten rows, not semantic changes, so a span shift counts as a change.
- A migrated v1 database keeps its size until a re-index or `VACUUM`. A legacy row that cannot be decoded fails the migration (delete the database and re-index). An older binary that re-adopts a project can still write legacy rows; they are read, and rewritten compactly on the next replace.
- A package-graph change still re-resolves and re-persists every owner. The similarity index costs about 300 MiB per 100k symbols (earlier measurement, not re-measured). A cross-process generation change forces a full state reload in every other open engine.
- The persisted skipped-file list can be stale after a crash (report-only, ENGINE-6).
- At 1M synthetic LOC, the full performance suite (2026-10-04) measured the following:
  - Queries under a concurrent edit stream reach only 8 to 26 qps, with p95 up to 974 ms.
  - `unused` takes about 1.1 s.
  - The long-running session process peaks at about 4.22 GiB RSS.

  Read-only workloads and repositories of 100k LOC or less are not affected to this degree.

### Findings from the agent evaluation

From the A/B batches of 2026-10-04 and 2026-10-05 ([results](evaluation/results.md#agent-ab)):

- **No measured benefit.** On 11 tasks with 2 runs per condition, there is no evidence that Code Atlas improves an agent's correctness, and the atlas condition used more tokens and cost more in both batches. All runs passed their hidden tests, so the tasks were too easy to separate the conditions.
- **Fixed cost.** Connecting the MCP server added about 6,300 prompt tokens to every model request (16 tool definitions plus server instructions, binary `6a2854f`), whether or not a tool is called. On short tasks this costs more than any reduction in file reads saves. The current server has 15 tools, but `atlas_symbol` and the new `atlas_edit` carry many more parameters. With shorter descriptions `tools/list` plus the instructions is 14,243 bytes with all tools, 8,829 with the `core` [profile](agent-integration.md#tool-profiles) and 6,347 with `read-only`; a test pins each under a byte budget. The per-request token cost has not been re-measured in sessions since, and no agent evaluation has run on the new surface: whether `atlas_edit`, the shorter descriptions or a smaller profile help or harm agents is unmeasured.
- **Adoption needs direction.** With the three-sentence standard preamble, which names the tools and says to use them before reading files, the agent called them in 2 of 22 sessions, both in the smoke task. Told when to call them, it called `atlas_context` in 21 of 22, but never `atlas_impact`, `atlas_search` or `atlas_similar`.
- **Context packs are keyword-seeded and can miss the edit target.**
  - Measured by `must_change`, the pack held none of the files to edit in 4 of 11 tasks (`t00`, `t03`, `t07`, and `t10` with its empty pack) and only some of them in 5 more (`t02`, `t04`, `t06`, `t08`, `t09`). It held them all only in `t01` and `t05`.
  - An ambiguous seed name (`User`, matching 2 entities) is dropped rather than resolved against the task text.
  - A pack can be empty apart from caveats (`t10`).

  Treat a pack as a starting point and confirm edit targets with search.
- **Partial parses.** 8 of 450 files in hono at `f23b146` parse only partially. Relations near the parse errors may be missing, and queries that touch these files say so in a caveat.
- **Recall.** Against SCIP, `resolved` edges reproduce 53.3% of the compiler's call, reference and instantiation edges, and references only 40.4%. An empty `callers` or `impact` result is weaker evidence than its certainty tag suggests; read the caveats.
- **Heuristic confidence.** Against SCIP, the confidence buckets at 0.6 and above hold only 15 edges. All 4 of their conflicts are granularity differences (class versus constructor, method versus receiver constant), while the wrong name-only targets sit at 0.25 to 0.35. Calibration of the confidence values is neither shown nor refuted.

### Negative results and known gaps (IDE operations and retrieval)

Measured 2026-10-05 and 2026-10-06. Numbers marked *review* come from the adversarial review of that task, run before merge. Where a number is tabulated in [`eval/reports/2026-10-06/review-trials.md`](../eval/reports/2026-10-06/review-trials.md) (rename trials, held-out `similar`, the headline reuse figures; manual trials with real servers, not reproducible by an `atlas-eval` command, raw logs not in the repository) it says so; any number tagged "review notes, not in `review-trials.md`", and the unused, mocks, resolver and parse-budget figures, have no checked-in table. Numbers marked *report* are checked in.

**Reuse suggestions from task prompts did not work (dropped).**

- Tried: lexical matching of a task prompt's prose against code, to list reuse candidates in `context`. Review on a reviewer-owned held-out set of 17 tasks (the implementer had not seen it; the set size is (review notes, not in `review-trials.md`)): with loose matching, reuse recall rose from 0.54 to 0.70 but only about 5% of the suggested lines were true hits (4 of 73); it also fired on why and bug prompts and repeated MUST READ items. With tight matching, 0 of 7 new lines on held-out prompts were true hits (0 of 3 on the dev set) and recall fell back to the baseline 0.54. About 780 lines of this path were removed (review notes, not in `review-trials.md`).
- Shipped instead: seed fixes (backticked identifiers seed only when capitalised or written as calls, platform types never seed alone, an ambiguous name narrowed by a named repo is seeded as named) and importability text on structural REUSE entries. On the dev set, `must_change` recall in MUST READ moved from 0.464 to 0.555 (the baseline is `eval/reports/2026-10-05/retrieval.md` at `0f3557f`; 0.555 is reproduced at `1c1232d` in `eval/reports/2026-10-12/retrieval.md`, which also shows it falling to 0.287 for the short queries agents actually sent). **Held-out check:** for the first revision the same review found the `must_change` gain did not generalise to held-out tasks (review notes, not in `review-trials.md`); no held-out confirmation of the shipped seed fixes is recorded. REUSE recall on the dev set did not change (0.4, (review notes, not in `review-trials.md`)).

**`similar` from draft code finds copies, not independent re-implementations.** Review of `similar --code` on a held-out set of 16 functions (8 Rust from this repository, 8 TypeScript from hono) rewritten at three levels, default threshold 0.5, same-language only (review at `cd9fd97`):

| draft | R@1 | R@5 | structural match |
|---|---|---|---|
| original, unchanged | 15/16 | 16/16 | 16/16 |
| light paraphrase (renames, reordering) | 15/16 | 16/16 | 16/16 |
| realistic re-implementation | 8/16 | 12/16 | **0/16** |
| realistic, threshold 0.3 | 10/16 | 13/16 | 3/16 |

- Name-and-signature rows, listed apart as low precision, add 1.9 to 3.3 rows per query at 6% to 25% precision (13% / 6% / 25% for original / light / realistic at 0.5; before the noise rule 6.4 to 7.7 rows at 2.8% relevance). The floor and cap that produce them were tuned on this same 16-function set, so those figures are not held-out and claim no gain.
- No structural match was found for any realistic re-implementation at the default threshold: structure detects copies. A conceptually equivalent rewrite is found only through the weak name route, or not at all. Cross-language matches: 0 rows (review notes, not in `review-trials.md`).
- No LLM verdicts exist in any published run, so nothing speaks to behaviour.

**Rename.**

- Fixtures, *report*: `eval/reports/2026-10-06/rename.md` scores 7 scenarios (acme, `trait-dispatch-rust`, `ts-overrides`): 41/41 edits found, 100% precision, 6/6 not-renamed sites listed with the expected reason, 0 of 13 `untouched` lines edited, with typescript-language-server 5.1.3 and rust-analyzer 1.97.1. These fixtures were used while building rename, so this is a regression check, not a held-out measurement.
- Real code, *review* of the hono trials (hono 4.13.13; build and spec TypeScript projects) and this repository:
  - Round 0: **7 of 10 TypeScript renames broke test compilation.** tsserver searched only the projects it had loaded, and a solution-style `tsconfig.json` loads a project only when one of its files is opened, so plans depended on earlier reads. Rename now opens the graph's candidate files and one file per containing tsconfig project (caps 200 and 16).
  - After the fixes, across 19 symbols (the 10 originals plus 9 new random ones, plus 2 variants of refused renames), every applied rename added 0 `tsc` errors in the build and spec projects. The same review still found 1 nondeterministic plan (an object-literal property), 2 false refusals (one for a local binding, one through a kept alias that should have passed) and a `replace_body` that wrote `export const export const`. Later revisions narrowed the binding check to ignore comments and strings, excluded kept aliases and fixed the keyword handling; the later hono trials added 0 `tsc` errors, but the exact local-binding case was not re-run. Rust: 5 of 5 renames on this repository pass `cargo check`, with `lsp.exclude` set.
  - Determinism is scoped to projects in the anchor's repo that contain or reference the anchor. Projects with no link to the anchor's project are not opened, so object-literal sites there can be missed.
- `replace_body` writes exactly the text the caller supplies: in one trial that text itself introduced type errors (`x2`, a `declare const` with a type the reviewer wrote), and the post-edit diagnostics delta reports such errors only when a server is warm. Known gaps: the clean-file TypeScript diagnostics baseline can time out under heavy load (the result is reported `PARTIAL`, never clean); a tsconfig `extends` that is an array or a package is ignored (this over-selects projects, which is safe); default exports are refused when addressed by position; insert spacing around `export const` is cosmetic; the refusal wording for an unhealthy rust-analyzer says "after the edit" even when no edit ran. These come from review notes and were not all re-verified for this page.

**Unused: test-only symbols.** Precision at merge (*review*, commit `385216d`): 28 of 28 on this repository, 57 of 57 on hono. The first review found 25% of this repository's test-only claims wrong, and a later revision's recall collapsed (about 119 correct hono symbols dropped); both were fixed, on the same repositories the revisions were developed against, so those precision figures are not held-out. Gaps: multi-line import and export blocks (about 7 hono symbols of recall), a package with an undeclared dependency whose only use is unresolved can list a production-used symbol, classes whose methods are only called through unresolved receivers, and rustfmt `* ` continuation lines count as comments.

**Mocks.** 38 of 38 stored mock edges were correct on two real repositories (*review*, commit `6efdf50`, a small count). Gaps: the call graph is path-insensitive (a controller built with a mocked service shows `[via call graph]` plus a note), file-level mocks are coarse (reported hedged), and `bun:test` and assignment mocks (`x.m = jest.fn()`) are not detected, which under-claims them as spied.

**Parse budget.** tree-sitter error recovery can loop forever on some malformed Rust and TSX; the analyzers had no time limit until `59e357b`. A budget-failed file is a failed file (`DEGRADED`) until a retry succeeds: one same-run retry at 2x, then retries after 1 minute, 10 minutes and 1 hour. Latency costs: a run that finds hung files adds about 6 to 10 s to the query that triggers it; a due retry adds about 10 s; under extreme starvation an escalated retry can take up to 300 s wall (10x backstop on a 30 s CPU cap). Review: first query on a repository with 24 hung files fell from 100 s (first revision, uncapped serial retries) to 4.6 s (final); starvation first-pass failures fell from 382 to 19, all recovered on a due retry (*review*). Fuzzing (270k, later 736k cases) kept finding further hanging inputs, which is why a time budget, not input fixes, is the guard.

**LSP configuration freshness.** Manifest edits (`Cargo.toml`, `package.json`, `tsconfig*.json`) are not in the engine's change set. `atlas-ide` compares the root and package manifests on every sync as a workaround; the proper fix (ENGINE-1) is in the backlog.

**No agent evaluation of the new surface.** The A/B batches predate `atlas_edit` and the folded tools; whether agents use them well, or whether the changed instructions help, is unmeasured.

### Unsupported

- Languages other than Rust, TypeScript, JavaScript, TSX and JSX (extensions `rs ts mts cts tsx js mjs cjs jsx`). No Python, Go, Java, C#, C/C++, PHP, Ruby.
- Projects of more than the repos you register; there is no remote or hosted index. Hidden files, `.gitignore`d files, symlinks, files over `max_file_bytes`, and the directories `node_modules target dist build .git .next coverage vendor` are not indexed unless an `include` pattern (`project add-repo --include`, `project set-repo`) names a default-excluded directory.
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
| SCIP indexer unavailable, timed out, or network failure | `catl scip` reports per package; `doctor` shows tool availability | no `compiler_verified` upgrade for that package |
| LLM provider unavailable or over budget | `enrich` prints skipped task counts, run recorded `partial`, exit code 3; `status` shows the run | no new inferred facts |
| Stale inferred facts | tag suffix `stale` or `needs-revalidation`; `status` counts; hidden unless `--include-stale` | excluded from default results |
| Inferred client-call link whose candidate endpoints changed | queries (`api`, `impact`, `unused`, `explain`) recompute the candidate set live and tag the link `needs-revalidation`; `status` counts it as current until the next `enrich` marks it | hidden from default results; the next `enrich` re-asks the model |
| Language server unavailable, disabled or TypeScript 7 only | exact operations fail with an install hint (`rustup component add rust-analyzer`, `npm i -g typescript-language-server typescript@5`, `--lsp on`); exit 1 | no exact answer and no silent fallback; `source=graph` for references and for an `outline` asked with `source=lsp` (outline reads the graph by default) |
| Language server view incomplete (busy, lost sync, workspace `health = error`, diagnostics not settled) | `PARTIAL: ...` in the answer; rename and safe delete refuse | treat as a lower bound; edits are not applied |
| Edit refused (stale plan, blockers, unverified sites, scope violation) | `atlas_edit` tool error / CLI exit 5, nothing written | re-plan after reading the listed sites |
| Edit write failed | `FAILED:` first line, exit 6, nothing changed (rolled back); or `DEGRADED:` first line, exit 7, with the files still changed | check the listed files; the index is refreshed in both cases |
| File over its parse budget | `DEGRADED` with `parse budget exceeded (...)`, file's facts removed, `status` shows the next retry | relations through the file are missing until a retry succeeds |
| Target not found or ambiguous | `not found: "<target>" (...)` or candidate list; exit code 4 | try `search`; an empty result is not proof of absence |
| Query returned nothing | caveats listing unresolved references, dynamic accesses and problem files that could hide relations | read the caveats before concluding |

Exit code 4 takes precedence in the CLI when nothing matched, even if the index is also degraded; read the `DEGRADED` line in the output. Refused edits exit 5, never 4.
