# atlas-link

Cross-repo linking for one project, exposed as `link(tx, project) -> LinkOutput`. The engine runs it by default as the `ProjectPass` named `link`, right after `structure`; atlas-link cannot implement `ProjectPass` itself because atlas-engine depends on this crate.

## Inputs

- The pass reads only assertions owned by files and by earlier passes (`repos`, `structure`). It never reads its own owner, `lineage` or `scip:*` (`facts.rs`), so a previous run cannot feed back into the next one.
- Routes come from `Endpoint` entities. Router-relative Express routes, axum functions mounted with `.nest`, and Nest controllers in a repo with `setGlobalPrefix` are composed into pass-owned endpoints (`composed_from`). Clients match only composed or absolute routes.
- `normalize_route` keeps optional params as `{id?}` (Express `:id?`, Next `[[...slug]]`); the matcher treats them as zero-or-one segment.

## Pass-owned attrs on shared keys

The store unions attrs across assertions (see `crates/atlas-store/CLAUDE.md`), so these attrs are visible through `entity()` and `edge()`; the pass's own assertion stays available through `entity_assertions`, `edge_assertions` and `owned_facts(Project { pass: "link" })`:

- `AuthScheme`: `consumers`, `used`, `evidence_count`, `supported_by_endpoints`
- `Endpoint` (detector-owned keys): `auth_requirement`, `auth_combinator`, `unclassified_guards`, `authorization_guards`
- `Event`: `producers`, `consumers`, `producer_repos`, `consumer_repos`, `cross_repo`, `orphan`
- `ClientCall`: `unmatched`, `unmatched_reason`
- manifest `depends_on_package` edges: `cross_repo`, `target_repo`

Tests assert the `[[auth]]` summary through `entity()` and the rest through the pass's own facts.

## Base-URL evidence

- A repo is named by a base source (env var, config key, identifier, literal host) only if all non-generic tokens of one of its names (repo, package, application) appear in the source. Generic tokens (`api`, `web`, `service`, `url`, `key`, `v1`, numbers, ...) only rank repos that are already named.
- A repo whose name tokens are all generic is never named. A host that matches no repo is evidence of nothing.
- A route with extra leading static segments (`/v1/users/{id}` for `{}/users/{}`) is a candidate only if no repo has a full match.
