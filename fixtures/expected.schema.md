# `expected.toml` format

Every fixture project (`acme/`, `globex/`) and every adversarial case (`adversarial/<case>/`) carries one `expected.toml` with the ground truth a correct analyzer/linker/query engine must reproduce. Selectors never contain SymbolKey strings; the evaluator maps selectors onto whatever key format the engine uses.

## Conventions

- All TOML. Inline tables are used for selectors. Unknown keys must be ignored by readers.
- `repo` is the repository directory name. In `acme/` and `globex/` it is the directory under the project; in `adversarial/<case>/` it is the case directory name itself and `path` is relative to the case directory.
- `path` is relative to the repo root, forward slashes.
- Route templates are normalised to `:param` style (`{id}` in axum and `${id}` in template literals both become `:id`). Methods are upper case.
- Certainty classes: `compiler_verified`, `syntactic`, `resolved`, `heuristic`, `inferred`, `declared` (plan section 1.3). A `certainty` list means "the fact must exist and carry one of these classes". `min_confidence` applies only to `heuristic` / `inferred` and is the lowest acceptable confidence.
- Ground truth encodes runtime-correct semantics, not naive string matching. Where the right answer is uncertain the expected class is `heuristic` (or the item is `unresolved`), never `resolved`.

## Symbol naming (`symbol`)

| Language | Form | Example |
|---|---|---|
| TS/JS | `functionName`, `ClassName`, `ClassName.method`, `default`, `constName` | `UsersController.findOne` |
| Rust | `fn_name`, `Type`, `Type::method`, `Trait::method` | `AcmeApiClient::get_order` |
| Rust nested inline module | `module::item`, relative to the file's own module | `tests::converts_whole_and_fractional_amounts` |
| Rust trait impl method | `<Type as Trait>::method` | `<PgInvoiceStore as InvoiceStore>::insert` |
| File | omit `symbol` | `{ repo = "acme-sdk", path = "src/index.ts" }` |

Module-level constant initialisers (`export const X = process.env.Y`, `const acme = new AcmeClient(...)`) are attributed to the const symbol, never to the file; the const is a `constant` entity. Facts at module level that are not inside a named declaration (for example a bare `app.use(...)` statement or a `include!` item) are attributed to the file.

Anonymous inline callbacks (Express handlers, closures) are attributed to the nearest enclosing named symbol; with none (module level) they are attributed to the file, i.e. the selector has no `symbol`.

Rust files are modules by path, so `src/handlers.rs` + `create_invoice` means `handlers::create_invoice`; only modules declared inline inside the file appear in `symbol`.

## Selectors

| Selector | Shape | Notes |
|---|---|---|
| symbol / file | `{ repo, path, symbol? }` | no `symbol` selects the file entity |
| test suite | `{ repo, path, suite }` | `describe('<suite>')` block in the file |
| endpoint | `{ kind = "api_endpoint", repo?, method, route }` | omit `method`/`route` to wildcard |
| other entity | `{ kind, repo?, name }` | `env_var` and `config_key` are scoped per repo; `event` is scoped per project; `auth_scheme` uses `scheme` in place of `name` (`bearer`, `api_key`, `session_cookie`); `permission`, `db_model` (table name), `package` use `name` |
| wildcard | `{ kind }` | any entity of that kind |

A selector with `repo` naming a repo of another project (for example `globex-api` inside `acme/expected.toml`) is legal and is used for isolation traps.

## Sections

### `[project]`
`name`, `repos = [...]`, `unused_exhaustive = false` (when false, symbols not listed in `[[unused]]` / `[[not_unused]]` / `[[ambiguous_unused]]` are ignored when scoring unused-code precision; recall is scored on `[[unused]]` only).

### `[[entity]]` - must exist
Entity `kind` values used by the fixtures: `api_endpoint`, `auth_scheme`, `class`, `config_key`, `constant`, `db_model`, `env_var`, `event`, `function`, `interface`, `macro` (Rust `macro_rules!`; a custom kind), `method`, `package`, `permission`, `struct`, `test`, `trait`. `db_model` carries both `name` (table) and `sym` (the model class). `test` is a single test function (`#[test]`, `it(...)`); suites are selected with `suite`.

`kind` plus either `sym = {...}` (symbols) or the selector fields inline (endpoints, other entities). Optional: `handler = {...}` (endpoints), `certainty = [...]` and `min_confidence` (derived entities such as endpoints whose class is `heuristic`), `best_effort = true` (extraction is desirable but not required), `note`.

### `[[edge]]` - must exist
`kind` (EdgeKind string; used: `calls`, `calls_endpoint`, `depends_on_package`, `equivalent_to`, `exposes_endpoint`, `implements`, `implements_auth`, `imports`, `instantiates`, `publishes`, `re_exports`, `reads_config`, `reads_env`, `references`, `requires_permission`, `subscribes`, `supports_auth`, `uses_auth`), `from`, `to` (selectors; `from` is usually a symbol, a file selector for file-level edges such as `imports`/`re_exports`; `imports` may also have a symbol-level `from`, meaning an import issued from inside that function, such as a dynamic `import()`; `supports_auth` has an endpoint selector as `from`), `certainty = [...]`, optional `min_confidence`, optional `required = false` (the edge is a tolerated candidate: allowed with the stated certainty, absence is not a failure), optional `occurrences` (exact number of distinct call-site/evidence spans backing the edge), `note`. For `calls` edges to a trait method, `to` names the trait method.

Auth edges: `implements_auth` runs from the guard, strategy or middleware symbol that checks a credential to the `auth_scheme` (detector output). `supports_auth` runs from an `api_endpoint` to each `auth_scheme` it accepts, derived through the endpoint's guards (`requires_guard` -> `implements_auth`, with composite guards followed through their delegates); it mirrors `[[endpoint_auth]]`. `uses_auth` runs from the client-side symbol that sends the credential to the scheme of the API it calls. Manifest `depends_on_package` edges are `syntactic` (read from the manifest); the cross-repo resolution adds attributes and a repo-to-repo edge, so expectations accept `["syntactic", "resolved"]`.

### `[[forbidden_edge]]` - must NOT exist
Same shape as `[[edge]]`. Optional `certainty = [...]` restricts the prohibition to those classes only (for example "may exist as heuristic, never as resolved"). Without `certainty` any edge of that kind between the selectors is a violation. `to = { kind = "..." }` with no other fields forbids every edge of that kind from `from`.

### `[[unresolved]]` - references that must stay unresolved
`from`, `ref_kind` (`call` | `reference` | `import` | `require` | `include` | `route`), `reason` (`dynamic` | `generated_code` | `macro_generated` | `ambiguous` | `external`), optional `name`, `candidates = [selectors]` (the only targets a `heuristic` edge may point to), `unless_certainty = [...]` (certainty classes that are correct for this reference and lift the no-edge/no-resolved expectation, for example `["compiler_verified"]` when a SCIP index resolves a macro-generated function), `max_certainty` (`heuristic` means a heuristic edge to a listed candidate is tolerated, absent means no edge at all), `note`. No `resolved` / `syntactic` edge may exist for these references, and no `compiler_verified` edge unless listed in `unless_certainty`.

### `[[unmatched_call]]`
A client call that must produce an `api_client_call` entity but no `calls_endpoint` edge to any endpoint. `from`, `method`, `route`, `note`.

### `[[auth]]`
`api_repo` is the repo exposing the API. Per API repo: `supported = [...]` (schemes the API accepts), `used = { scheme = [client repos] }` (schemes some client in the project actually sends), `unused = [...]` (supported but never used by any client).

### `[[endpoint_auth]]`
`method`, `route`, `schemes = [...]`, `mode = "all_of" | "any_of"`, `permission?`, `certainty`.

### `[[unused]]` / `[[ambiguous_unused]]` / `[[not_unused]]`
`sym = {...}` or `endpoint = { method, route }`. `unused`: must be reported unused (no project-internal consumer). `not_unused`: must NOT be reported unused (reachable via decorator, trait dispatch, re-export, route registration or test-only use; `reason` explains). `ambiguous_unused`: public-API surface with no internal caller; ignored by scoring.

### `[[similar]]`
`a`, `b` (symbol selectors), `expected = "equivalent" | "similar_not_equivalent" | "unrelated"`, `acceptable = [...]` (other verdicts accepted in addition to `expected`, for genuinely debatable pairs), `deterministic_claim_allowed = false` (deterministic analysis must not emit `equivalent_to`; `similar_to` with `heuristic` certainty is allowed for `similar_not_equivalent`), `note`. `equivalent` is only ever an `inferred` fact.

### `[[test_of]]`
`test` (suite or symbol selector), `subject`, `certainty = [...]`, `note`.

### `[[file_flag]]`
`repo`, `path`, `flag` (`generated` | `partial_syntax`), `note`. The file is still indexed.

### `[[blast_radius]]`
`change` (selector), `must_include`, `may_include`, `must_not_include` (lists of selectors), `note`. Transitive impact through calls, references, endpoint edges and tests.

### `[[isolation]]`
`project` (the project a query is scoped to), `must_not_return = [selectors]`, `note`. Queries scoped to `project` must never surface these, even for identical names, routes or env vars.

### `[[rename_sites]]`
Eval truth for rename dry runs (`atlas-eval quality`; no producer reads it, so no pin changes). One scenario per table.

- `id` (unique per file), `new_name`, `note`.
- `target = { repo, path, symbol }` names the symbol to rename, using the symbol forms above. When a name is ambiguous, give `line` (and optionally `column`) for a position target, or `key` for an entity key; `key` wins over `line`, which wins over `symbol`.
- `cross_repo = "report" | "apply"` (default `report`) is passed to the rename.
- `setup = [{ repo, path, replace, with }]` rewrites the evaluator's fixture copy before indexing (every occurrence of `replace`; a missing `replace` text fails the scenario). Lines in the truth below refer to the files after the setup.
- `edit = [{ repo, path, line, column? }]` lists every site the plan must edit. Lines and columns are 1-based. Without `column` a site matches any edit on that line; two entries on one line need two distinct edits. Each edit must be backed by a read of the file: the line holds the old name as a reference to the target.
- `report = [{ repo, path, line, column?, reason }]` lists sites the plan must list as not renamed. `reason` is one of `language_server`, `outside_language_server`, `mock`, `heuristic`, `http`, `dynamic`, `token_mismatch`.
- `untouched = [{ repo, path, line, column? }]` lists lines that mention the name (a string, an unrelated method of the same name, a longer identifier) and must not be edited. An `untouched` line may not also be an `edit` or `report` site.

Scores per scenario and in total: edit recall (expected sites planned / expected), edit precision (planned edits that are expected / planned), report recall (expected report sites listed with the expected reason / expected) and violations (`untouched` lines edited). A scenario without a language server is `skipped`, never zero; a scenario that fails counts its expected sites as missed.

## Validation

Every `expected.toml` must parse with `tomllib`, every selector `path` must exist under its repo, and every `symbol` (or `suite`) must textually occur in that file. Selectors in `forbidden_edge` / `isolation` / `blast_radius` may point at other repos of the fixture tree (for example `globex-api` from `acme/expected.toml`).
