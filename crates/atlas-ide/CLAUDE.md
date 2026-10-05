# atlas-ide

`IdeService`: exact read operations over language servers (references, declaration, implementations, outline, diagnostics) plus graph extras, and pass-throughs to the graph modes (`find`, `hierarchy`, `source = graph` outline and references). Shared by CLI and MCP.

## Service

- One `IdeService` per (process, project). It owns its `QueryEngine` (built from the shared `Arc<Engine>`), one `LspPool` (`IdeMode::Warm` for MCP, `IdeMode::OneShot` for CLI) and a `Launcher`.
- `reload(project)` adopts new `lsp` settings: a changed pool configuration (or disabling) replaces the pool and shuts the old one down; the discovery cache is cleared either way. Units whose configuration changed are replaced by the pool on their next use. Callers still call `Engine::reload_project` themselves.
- `shutdown()` (also on drop) shuts every server down; MCP must call it before `persist_similarity`.
- Discovery sits behind `Launcher`: `DiscoveryLauncher` (production, atlas-lsp discovery + `atlas_scip::locate_rust_analyzer`) or a test launcher. Successful launch specs are cached per (kind, scope, folders); failures are not, so an install is picked up on the next call.

## Exact operations

- Order is fixed: barrier (`QueryEngine::fresh`) → target resolution → unit plan (`Engine::language_units()`, taken before the unit lock) → per-unit mutex → `pool.sync` (one lease for sync and request) → requests. Reads use `SyncDemand::Best`; B7 must pass `SyncDemand::Exact` for rename and safe_delete.
- Lock order: the per-unit mutex may be held while the engine mutex is taken (store reads, `write_then_refresh`), never the reverse. Nothing called with the engine mutex held may take a unit lock.
- Answers are never written to the store. They carry `ServerView`: server, version (reported by the server, discovery's value only as fallback), reported TypeScript version, certainty `compiler_verified` / `[compiler]` with `source = "lsp:<server> <version>"`, completeness, caveats and warnings.
- Never present a partial answer as complete: `Settle::Busy`, a lost document sync, unsettled diagnostics and an incomplete flycheck make the answer `Partial` (rendered `PARTIAL: …`). `Settle::Unconfirmed` is a caveat naming the paths; drift, skipped overlays, restarts and rewritten `Cargo.lock` files are warnings.
- No fallback: disabled or unavailable servers are typed errors (`Disabled`, `Unavailable`, `ServerFailed` for spawn, crash-loop, readiness and pool failures) with the install hint; outline and references add "retry with source='graph'". Graph answers are only returned when `source = graph` is asked for, and keep their tags.
- A TypeScript server that reports TypeScript ≥ 7 is retired and the call fails `Unavailable`; that unit refuses without spawning for 60 s. A reported version that contradicts a workspace tsserver choice is a warning.
- Change sets for sync come from the run ledger (`barrier`/`index`/`edit` runs between the server's generation and the target). Any gap (missing run, failed or running run, capped section, more than 256 runs) is `ChangeList::Unknown`, which restarts the server. A non-empty `graphs_changed` in range adds a caveat. Manifests (`package.json`, `tsconfig*.json`, `jsconfig*.json`, `Cargo.toml`, `Cargo.lock`, `build.rs`, `.cargo/config*`) in the unit roots, Cargo directories and every package directory (a directory of indexed files holding `package.json` or `Cargo.toml`, recomputed per generation) are compared against a per-server snapshot, because the ledger does not list manifest edits.

## Targets and positions

- Targets: key or id, `[repo:]path:LINE:COL` (1-based, UTF-8 byte column), `[repo:]path:LINE`, name paths. Endpoints, files and non-code entities are refused with typed errors; paths outside every repo root (after canonicalisation) are `OutsideProject`. A TypeScript overload set (keys equal up to `~n`) is queried at its first signature; anything else ambiguous is `Targeted::Ambiguous`.
- A symbol position is its declared name token. The indexed hash is read before and after `name_span`; if it moved, the barrier runs once and resolution repeats, then `SourceChanged`. The anchor text must hash to that indexed value, and the server's open document must hash to the anchor text, or the call fails `SourceChanged`.
- Every LSP position is converted with `LineIndex` over text proven to be what the server saw: the open overlay's hash, or for unopened files the indexed hash. A site in a file that changed, cannot be read or does not fit the position keeps the server's line, no column, and an `unverified` reason. Displayed columns are 1-based UTF-8 byte columns on `\n`-counted lines.
- Enclosing symbols come from the graph (`innermost_symbol`) only when the file's text equals the indexed text; otherwise from the server's document symbols.

## References extras

- `outside the language server's view` lists, for symbol targets only, inbound graph usages the server did not return (matched by repo, path, line and column, line only when the graph has no column): other repos, mock references (`mock` attr), and same-repo sites with authoritative certainty (`not in the server's project`, e.g. files outside the tsconfig `include`). Unreturned heuristic same-repo sites are counted in a caveat, not listed. Unresolved same-name sites are tagged `[unresolved]`. HTTP consumers (`calls_endpoint`, inferred links excluded) of endpoints the target exposes are listed once per client call site (evidence noted `endpoint` is skipped). Each keeps its graph certainty tag.
- A position target's own site is kept; only a symbol target's declaration is dropped from the server's list.

## Diagnostics

- Pull (`textDocument/diagnostic`) when advertised; otherwise the open document is re-sent under a new version (`Lease::touch_document`) and the latest publish with `version >= v` (or no version) after that point wins once 300 ms pass without a newer one, at most 10 s. A timeout, with or without a publish, is `Partial`; without one the answer says nothing is known and never reports a clean file. A quiet window cut short by the deadline is a timeout, not settled.
- rust-analyzer: `rust-analyzer/runFlycheck` (works with checkOnSave off), begin within 5 s or `not_run`, end within `check_timeout_secs` or `timed_out`. Unless complete, the answer is `Partial` and only `source = "rust-analyzer"` diagnostics are kept, so earlier cargo check results never pass as current.
- TS2307 adds the caveat that references through the unresolved module are invisible to exact operations.

## Testing

- `atlas-ide-fake-lsp` is atlas-lsp's fake server compiled again (`include!`), because Cargo exposes `CARGO_BIN_EXE_*` only to the defining package's tests.
- Real-server tests (`tests/lsp_real.rs`) run with `ATLAS_TEST_TS_LSP`, a server on PATH or in the npx cache, and rust-analyzer on PATH or via rustup; otherwise they print `SKIPPED`. They never download.
