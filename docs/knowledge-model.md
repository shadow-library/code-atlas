# Knowledge model

The graph is a set of entities and edges. Every entity and edge is asserted by an *owner* (a file, or a project pass), carries exactly one certainty, and cites evidence. Definitions below come from `atlas-core` (`kinds.rs`, `keys.rs`, `provenance.rs`, `graph.rs`) and `atlas-store`.

## Entities

36 built-in kinds plus snake_case custom kinds (`EntityKind::Custom`, used for example for `macro`-like or pass-status entities; `Other` keys carry them).

| Group | Kinds | Key form (rendered) |
|---|---|---|
| Code symbols | `module function method class struct enum trait interface type_alias constant field variant variable namespace test test_suite macro` | `<lang>:<repo>:<package>:<module path>::<item path>#<kind>[~n]` |
| Containers | `repository package application service file` | `repo:<repo>`, `pkg:<npm\|cargo>:<name>`, `app:<repo>:<name>`, `service:<repo>:<name>`, `file:<repo>:<path>` |
| HTTP and auth | `api_endpoint api_client_call auth_scheme permission` | `endpoint:<repo>:<METHOD> <route>`, `client:<repo>:<METHOD> <target>`, `auth:<repo>:<scheme>`, `permission:<repo>:<name>` |
| Config | `config_key env_var` | `config:<repo>:<key>`, `env:<repo>:<NAME>` |
| Messaging, jobs, data (experimental) | `event queue job db_table db_model` | `event:<name>`, `queue:<name>`, `job:<repo>:<name>`, `table:<name>` |
| Other | `external_service capability doc` | `external:<name>`, `capability:<name>`, `other:<kind>:<repo>:<name>` |

Notes:

- `~n` is a per-file disambiguator in source order for same-named symbols (overloads), so reordering overloads changes keys.
- Symbol keys percent-escape `% : / # ~` in segments. The language prefix is `ts` for TS/JS/TSX/JSX and `rust` for Rust. Keys are parsed back and must be in canonical form.
- Routes are normalised to `{param}` form (`/users/{id}`); optional params stay `{id?}`. A Nest global prefix is never part of an endpoint key; it is recorded as `other:http_global_prefix:<repo>:<prefix>` and applied by the link pass, which emits composed endpoints.
- `EntityId` is a 64-bit truncated keyed blake3 hash of the rendered key and is pinned by tests. Changing key rendering is a breaking change.
- A DB model is a normal class/struct symbol with a `maps_to_table` edge to the `db_table`; the `db_model` kind exists only for `Other` keys.
- Scoping: env vars, config keys, endpoints, auth schemes, permissions and jobs are repo-scoped; events, queues and tables are project-scoped.

## Edges

33 built-in kinds plus custom kinds (the Express and axum detectors emit a custom `mounts` edge). One edge exists per `(kind, from, to, discriminator)`; the discriminator is empty unless parallel edges mean different things (for example two `calls_endpoint` edges with different auth headers). Evidence from several spans is merged into one edge.

| Edge kind | Produced by | Certainty in practice |
|---|---|---|
| `contains` | analyzers (file to symbol, parent to child), resolver (module tree, type to impl method), manifests, structure pass | `syntactic`, `resolved` |
| `imports`, `re_exports` | resolver | `resolved`; `heuristic` when a hop went through `dist-to-src` |
| `calls`, `references`, `instantiates`, `type_of` | resolver | `resolved` for unique results; `heuristic` for dispatch candidates, name-only matches, macro token scans; `compiler_verified` after SCIP |
| `implements`, `extends` | resolver | `resolved`; `compiler_verified` after SCIP |
| `references` with attr `mock` (`replaced` or `spied`) | `test-mocks` detector, from test code to the mocked member | `heuristic 0.60`; queries never count a mock edge as production use or as a test exercising its target |
| `depends_on_package` | manifests; link pass adds `cross_repo`/`target_repo` | `syntactic`, `resolved` |
| `exposes_endpoint` | endpoint detectors plus handler resolution | `syntactic`, `resolved`, `heuristic` (computed routes) |
| `requires_guard`, `implements_auth`, `makes_request` | detectors only; consumed by the link pass | `resolved`, `heuristic` |
| `calls_endpoint`, `supports_auth`, `uses_auth` | link pass | `heuristic` (never above; derived from route and auth matching) |
| `requires_permission` | detectors (from handler decorators) | `syntactic` |
| `reads_env`, `reads_config` | config detector | `syntactic` for literal names, `heuristic` for config getters |
| `publishes`, `subscribes` | events detector (experimental) | `heuristic` |
| `reads_table`, `writes_table`, `maps_to_table` | db detector (experimental) | `syntactic` for explicit table names, `heuristic` otherwise |
| `defines` | jobs detector (experimental) | `heuristic` |
| `covers_file` | test detector | `syntactic` (Rust `#[cfg(test)]` module), `heuristic 0.60` (file naming) |
| `part_of_app` | structure pass | `syntactic` (manifest), `heuristic 0.70` (server bootstrap) |
| `renamed_from` | lineage | `heuristic`, see below |
| `similar_to`, `equivalent_to` | LLM enrichment only, stored as inferred facts | `inferred` |
| `tests`, `exports` | declared in the kind list; no production stage emits them (test relations come from `calls` and `covers_file`) | not produced |

Similarity search results (`query similar`, `search`) are computed from an in-memory index at query time. Deterministic analysis never stores `equivalent_to`, and a fixture asserts it must not.

## Certainty classes

Every edge and derived entity has exactly one `Certainty`:

| Class | Tag | Meaning | Authoritative |
|---|---|---|---|
| `compiler_verified` | `[compiler]` | Confirmed by a SCIP index built from the same file contents (rust-analyzer or scip-typescript). Never emitted by the resolver | yes |
| `syntactic` | `[syntactic]` | Read directly from syntax or a manifest: containment, declared routes, decorator arguments, env var names, manifest dependencies | yes |
| `resolved` | `[resolved]` | A unique result under emulated language rules (scope, module, import, trait and type resolution). Not compiler-verified | yes |
| `declared` | `[declared]` | Asserted by configuration rather than code: registered repos, the link pass status entity | yes |
| `heuristic` | `[heuristic 0.72]` | A named rule with a confidence in 0..1: dispatch candidates, name-only matches, macro token scans, route and base-URL matching, naming conventions, absence ("no detected client") | no |
| `inferred` | `[inferred 0.81]` | Produced by an LLM with provider, model, rationale and evidence. Can go stale | no |

The same `[compiler]` tag is rendered on live language-server answers (`atlas_symbol` references, declaration, implementations, diagnostics; `atlas_edit` plans), with `source = "lsp:<server> <version>"`. Those answers are not graph facts: they are never stored and are valid for the configuration the server loaded (features, target, loaded projects). A partial one is marked `PARTIAL`. Graph sites that the server did not return keep their own graph tags beside it.

`is_authoritative()` is true for the four exact classes. "Authoritative" means exact under the class's derivation, not exhaustive. `is_llm_derived()` is true only for `inferred`.

Strength order (used to pick the winning assertion): `compiler_verified` > `syntactic` > `declared` > `resolved` > `heuristic` (by confidence, then rule name) > `inferred` (by confidence). Ties are broken deterministically.

Heuristic rules defined by the resolver include `dyn-dispatch-candidate` (0.3), `interface-dispatch-candidate` (0.3), `override-dispatch-candidate` (0.3), `method-name-only` (0.5 for one candidate, 0.35 for two, 0.25 for three), `field-name-only` (0.5), `macro-name-in-crate` (0.6), `macro-token-scan` (0.6), `param-type-recovery` (0.7), `dist-to-src` (0.85, applied per hop), `fn-local-item-possible` (0.5). Absence rules from the link pass use a fixed 0.5 (`no-detected-auth-scheme`, `no-detected-client`, `no-detected-counterpart`, `no-matching-route`). Heuristic certainty is carried through every hop (re-exports, extends chains, globs), so a path is as weak as its weakest edge.

Rendering rules: a fact is stated plainly only if authoritative. Every non-authoritative entity carries its tag after its name; relation tags go in parentheses (`X [heuristic 0.80] ([heuristic 0.70])`) or after the target of a `verb target [tag]` step. Heuristic relations are hedged in prose (`possibly calls endpoint GET /orders/{id} [heuristic 0.77]`), inferred ones as `likely ...`. Default confidence floors for heuristic relations: 0.25 for callers, callees, dependents and dependencies; 0.3 for context packs; 0.0 (nothing hidden) for impact, tests-for, api, unused, explain, symbol and similar. `--min-confidence` overrides the floor where the command has it, and hidden relations are counted in the caveats.

## Provenance and evidence

`Evidence` = `{ repo, path, span, content_hash, producer, note }`. The `content_hash` is the hash of the file when the fact was produced, which is what makes staleness checkable. `Producer` = `{ name, version }` (for example `atlas-lang-ts@1`, `atlas-detect.events@1`, `atlas-resolve@1`, `scip:<tool>`).

`code-atlas query explain <target>` shows the merged fact, every assertion with its owner, certainty and evidence spans, and the file's last indexed run. Example (acme, `GET /users/{id}`): the endpoint is `[syntactic]` from the file owner, while the `link` pass asserts a `[heuristic 0.50]` annotation on the same key.

## Per-attribute provenance

The store keeps one assertion per owner and key, and the merged view unions attributes across owners. A heuristic link-pass attribute on a syntactic endpoint must not be rendered as syntactic, so `entity_with_sources` / `edge_with_sources` return, for every attribute, the owner and certainty of the assertion whose value was kept. Renderers must tag each attribute with its own source certainty, never with the entity's.

In `explain` output each attribute reads `attr <name>: <value> [<class>, <owner kind>]`, for example `attr framework: nestjs [syntactic, file]` and `attr auth_requirement: {"all_of":["bearer","authorization_only:PermissionsGuard"]} [heuristic 0.50, link]`.

Pass-owned attributes on shared keys (from `atlas-link`): `AuthScheme` (`consumers`, `used`, `evidence_count`, `supported_by_endpoints`), `Endpoint` (`auth_requirement`, `auth_combinator`, `unclassified_guards`, `authorization_guards`), `Event` (`producers`, `consumers`, `cross_repo`, `orphan`), `ClientCall` (`unmatched`, `unmatched_reason`), manifest `depends_on_package` (`cross_repo`, `target_repo`). An annotation entity has one certainty for all its attributes: the weakest of every input behind them. Negative attributes ("unused", "orphan", "unmatched") are always capped at `heuristic 0.5`, because absence of evidence is not evidence of absence.

## Unresolved references

References that do not resolve are stored with a reason and shown only as caveats:

| Reason | Meaning |
|---|---|
| `not_found` | no candidate in scope |
| `ambiguous` | several candidates; none is picked as `resolved` |
| `dynamic` | path has an expression head or computed segment (`table[name]()`, `require(x)`, template `import()`), so no candidates exist |
| `external` | resolved to a package outside the project (npm name, Cargo crate, `std`, Node builtin, `global`) |
| `unsupported` | with a detail such as `macro_generated` (a name a project macro may define) or `fn-local item` |

## Inferred facts, staleness and revalidation

Inferred facts live in `inferred_facts` with subjects, evidence, provider, model, rationale and a payload. Relations in use: `equivalent_to`, `similar_to`, `calls_endpoint`, `capability_summary`. `enrich` produces similarity-pair classifications, `calls_endpoint` links for client calls the link pass could not match (shown only as tagged inferred consumers, never merged into deterministic edges), and, with `--summaries`, capability summaries.

Freshness is computed from evidence hashes (`Freshness::of_evidence`):

| State | Tag suffix | Cause |
|---|---|---|
| `current` | none | every evidence file still has its recorded hash |
| `stale` | `stale` | an evidence file changed or no longer exists |
| `needs_revalidation` | `needs-revalidation` | a subject was renamed or moved (lineage); an inferred client-call link also needs revalidation when the client file or the candidate endpoint set changes (queries recompute the set live; `explain` says why a link is settled or stale and, if a static link now exists, its tag) |

Stale and low-confidence (below 0.5) facts are excluded from default query results; `--include-stale` shows stale ones. Deterministic facts are never stale: they are re-derived or deleted when their file changes. `code-atlas enrich` re-runs due facts through the provider (cached by prompt hash).

## Lineage

The `lineage` owner holds `renamed_from` edges (new symbol to old symbol), produced when symbols disappear and appear in the same update. It is history, so it cannot be recomputed from the tree and is excluded from the incremental-equals-full check.

| Rule | Condition |
|---|---|
| `moved-file` | same kind, the file moved, and the item path is unchanged (confidence 1.0) |
| `identical-body` | same normalized token body; at least 12 tokens when in the same or the moved file, at least 30 otherwise |
| `similar-body` | MinHash near-duplicate with Jaccard of at least 0.85 (not identical) on bodies of 40 or more tokens |

A pair is recorded only if it is the unique best match in both directions (a tie leaves both symbols unmatched). Kind must match. Bodies under 30 tokens whose names share no meaningful token are capped at confidence 0.6. Up to 8 near-duplicate candidates are considered per symbol and at most 10,000 lineage edges are kept. Inferred facts migrate to the new subject only for lineage with confidence 0.85 or higher; others become `needs-revalidation`.
