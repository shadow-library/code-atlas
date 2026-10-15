# atlas-link

Cross-repo linking for one project, exposed as `link(tx, project) -> LinkOutput`. The engine runs it by default as the `ProjectPass` named `link`, right after `structure`; atlas-link cannot implement `ProjectPass` itself because atlas-engine depends on this crate.

## Inputs

- The pass reads only assertions owned by files and by earlier passes (`repos`, `structure`). It never reads its own owner, `lineage` or `scip:*` (`facts.rs`), so a previous run cannot feed back into the next one.
- Routes come from `Endpoint` entities. Router-relative Express routes, axum functions mounted with `.nest`, and Nest controllers in a repo with `setGlobalPrefix` are composed into pass-owned endpoints (`composed_from`). Clients match only composed or absolute routes.
- `normalize_route` keeps optional params as `{id?}` (Express `:id?`, Next `[[...slug]]`); the matcher treats them as zero-or-one segment.

## Pass-owned attrs on shared keys

The store unions attrs across assertions (see `crates/atlas-store/CLAUDE.md`), so these attrs are visible through `entity()` and `edge()`; the pass's own assertion stays available through `entity_assertions`, `edge_assertions` and `owned_facts(Project { pass: "link" })`:

- `AuthScheme`: `consumers`, `used`, `evidence_count`, `supported_by_endpoints`
- `Endpoint` (detector-owned keys): `auth_requirement`, `auth_combinator`, `unclassified_guards`, `authorization_guards`, `packages`
- `Event`: `producers`, `consumers`, `producer_repos`, `consumer_repos`, `cross_repo`, `orphan`
- `ClientCall`: `unmatched`, `unmatched_reason`, `packages`
- manifest `depends_on_package` edges: `cross_repo`, `target_repo`

Pass-owned table accesses (`tables.rs`): for every `maps_to_table` edge into a `DbTable`, each `reads_table`/`writes_table` edge into its source (a Drizzle constant) yields the same kind from the accessor to the table, at the weaker certainty of the access and the mapping, with the access evidence relabelled, `via` (sorted source keys) and `orm` (only when every merged access agrees). Accesses of one function to several constants of one table merge into one edge whose certainty is the strongest of them, independent of read order. `link_status` counts them as `table_accesses`.

The store keeps one assertion per owner and key, so each annotation entity has one certainty for all its attrs. That certainty is the weakest, by `cmp_strength`, of every input behind any of its attrs (heuristic becomes `Heuristic { rule: "link-derived", confidence: min }`), never stronger than the entity itself:

- Endpoint: `requires_guard`, `implements_auth`, `delegates_to` and, for authorization-only guards, `requires_permission`. An `Unknown` guard adds the absence input `no-detected-auth-scheme` (0.5); an `AuthorizationOnly` guard adds `guard-name-classification` (0.5), since it is a name match. A composed endpoint keeps the weaker of its route-chain certainty and this one, so its own certainty can drop (one assertion per owner and key).
- Annotations assert only the attrs the pass derives; detector attrs are not copied, so they keep detector provenance.
- AuthScheme: `uses_auth` and `supports_auth` edges. A scheme with no `uses_auth` edge is also capped at `Heuristic { rule: "no-detected-client", confidence: 0.5 }`, because `used: false` is absence of evidence.
- Event: `publishes` and `subscribes` edges. An event with no producer or no consumer (`orphan`, zero counts) is also capped at `Heuristic { rule: "no-detected-counterpart", confidence: 0.5 }`.
- `depends_on_package` `cross_repo`/`target_repo`: `Resolved` at most, from manifests.
- ClientCall `unmatched`: capped at `Heuristic { rule: "no-matching-route", confidence: 0.5 }`.
- `packages` (Endpoint, ClientCall): the nearest package manifest of every file asserting the key (`scopes.rs`, built from file-owned `Package` entities whose `manifest` attr is their owner's path), so at most `syntactic`. A composed endpoint carries the packages its base shares with the prefix that composed it (the base's when the prefix has none or shares none). Annotated only in repos with more than one manifest; placement uses packages in every repo.
- Absence caps use a fixed 0.5 confidence; only the heuristic class matters, and no negative attr may render under a syntactic tag.

The merged entity keeps the strongest certainty across owners, so only the per-attr source certainty from `entity_with_sources` drops.

`uses_auth` never comes from test code: `atlas_core::is_test_code`, or a Rust item inside a module entity that carries `ATTR_TEST` (`#[cfg(test)] mod fixtures`). Any change to this pass's output bumps `LINK_VERSION` and its pin in `atlas-engine/tests/version_pin.rs`.

Tests assert the `[[auth]]` summary through `entity()` and the rest through the pass's own facts.

## Package placement

- When some global prefix of the repo shares a package with an endpoint, only those prefixes are composed onto it: prefixes of other packages no longer are. When none does, every prefix of the repo applies as before, so an endpoint never loses all of its compositions.
- A site whose `makes_request` carries `client_module`/`client_export` (a declared `files` client imported by a relative specifier) is joined to the file-owned `other:typed_client` entity whose `modules` and `export` match in the same repo: the site matches `<base><target>` for a relative target, inherits the definition's `base_source`, `types_module` and `types_path` when it has none, and records `client_defined_in` plus the definition's evidence. A client is `unmatched` only when no site found a candidate.
- A client site's candidates are restricted to one package, first match wins, and only if a candidate lives there: the package of `types_path` (nearest manifest in the caller's repo) or of `types_module` (bare or scoped package name that is an in-project package), then `target_package` (must be an in-project package), then the caller's package when the URL is relative. Restricting changes shares, never route confidence; excluded candidates stay in `alternatives` with `linked: false`.

## Base-URL evidence

- A repo is named by a base source (env var, config key, identifier, literal host) only if all non-generic tokens of one of its names (repo, package, application) appear in the source. Generic tokens (`api`, `web`, `service`, `url`, `key`, `v1`, numbers, ...) only rank repos that are already named.
- A repo whose name tokens are all generic is never named. A host that matches no repo is evidence of nothing.
- A route with extra leading static segments (`/v1/users/{id}` for `{}/users/{}`) is a candidate only if no repo has a full match.
