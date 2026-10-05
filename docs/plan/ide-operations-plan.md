# Code Atlas: IDE Operations Plan

Status: **approved** (2026-10-05)
Prerequisites: branch **R2** (`impl/R2-resolver-precision`, uncommitted in `.worktrees/R2`, sets `RESOLVER_VERSION = 3`) and branch **E2** (`impl/E2-retrieval-eval`, uncommitted in `.worktrees/E2`, adds `atlas-eval retrieval`) are merged before tasks A1, A5 and D2 start.

## Approved decisions (user, 2026-10-05)

- Q1 `lsp.enabled`: **on by default, downloads allowed** — exact operations may start rust-analyzer / typescript-language-server, and a missing TS server is fetched via npx (`typescript-language-server` + `typescript@5`). `project config <p> --lsp off` / `--lsp-download off` disable it. (Overrides the "downloads opt-in" row in §0 and §3.2.)
- Q2 MCP folding: **fold `atlas_read` → `atlas_symbol mode=source` and `atlas_changes` → `atlas_status since_run`** (16 → 15 tools); `atlas_callees`/`atlas_explain` stay.
- Q7 cross-repo rename sites: **report by default; `cross_repo: "apply"` edits only authoritative graph sites** (as §7.3).
- Plan defaults taken for the rest: Q3 native diagnostics only in `atlas_edit` answers (flycheck only in `mode=diagnostics`); Q4 one server per (project, language); Q5 warn on `Cargo.lock` change; Q6 no shared daemon in v1; Q8 `tsgo --lsp` later.

---

## 0. Decisions

| Area | Decision |
|---|---|
| Exact operations | Come from language servers only: rename, references, declaration, implementations, document symbols, diagnostics and safe delete. If no server is available they fail with an install hint; there is no silent fallback. |
| Graph's role | Cross-repo, HTTP, mocks, reuse, impact, unused, freshness and provenance. LSP answers are **never persisted** into the graph. |
| New crates | `atlas-lsp` (protocol, client, process, pool, sync), `atlas-edit` (plans, safe application, text operations, symbol regions), `atlas-ide` (orchestration shared by CLI and MCP) |
| JSON-RPC and types | Hand-written stdio JSON-RPC and hand-written LSP types (about 30 structs). No `lsp-types`, no async framework. |
| Server unit | One server per (project, language). rust-analyzer gets `linkedProjects` = every Cargo workspace or standalone manifest in the project. typescript-language-server gets `workspaceFolders` = every TS repo root. |
| Warm servers | At most 2 per process (default): one rust-analyzer and one TS server. LRU eviction, 10 min idle shutdown, no prewarm. |
| TS server consent | Use an installed or cached server only. Downloading via `npx` is opt-in (`project config <p> --lsp-download on`), like auto-SCIP. `typescript@5` is pinned, because `typescript@latest` is 7.x and ships no `tsserver.js`. |
| Symbol-edit spans | Graph spans (tree-sitter, bound to a content hash) plus a leading-doc/attribute scanner. They are cross-checked against LSP `documentSymbol` when a server is already warm. replace_body and insert work without a server; rename and safe_delete need one. |
| Cross-repo rename sites | Default: reported as "not renamed: verify" and never edited. With `cross_repo: "apply"`, only **authoritative** graph sites whose name token checks out on disk are edited. |
| Positions | Exact operations take line:col from the LSP. Graph references get a name-token column computed at query time inside their (expression-level) span. The stored format and the analyzers do not change. |
| MCP | `atlas_symbol` gains `mode`. A new `atlas_edit` has an `operation` field and `dry_run`. `atlas_read` folds into `atlas_symbol mode=source` and `atlas_changes` into `atlas_status since_run`. Result: 15 tools, about +8–13% schema tokens. |

---

## 1. Findings that change the brief (please confirm)

1. **Fixture facts differ from the brief.**
   - `formatMoney` exists only in `acme-api`: `src/common/money.ts`, used by `orders.service.ts` and `reports.service.ts`. It is not in acme-web or acme-sdk.
   - The cross-repo SDK case is `toCents`: `acme-sdk/src/money.ts`, re-exported from `src/index.ts`, used by `acme-web/src/components/OrderForm.tsx` via `@acme/sdk`. The truth file allows `resolved`/`heuristic`. acme-web has no `node_modules`, so tsserver cannot see the link. This is exactly the graph-steps-in case.
   - The jest mock of a `findOne` lives in `acme-api/src/orders/orders.controller.spec.ts`. It mocks `OrdersService.findOne` through `{ findOne: jest.fn() }` and `ordersService.findOne.mockResolvedValue`, and the cast `as unknown as OrdersService` breaks the type link.
   - `users.service.spec.ts` calls `UsersService.findOne` for real; it mocks `repository.findOneBy`.
   - The golden scenarios below use the real sites.
2. **"Evidence lacks column spans" is only half true.**
   - Stored `Span` keeps byte columns: `start_col`/`end_col` in `atlas-core/src/span.rs`, preserved by `atlas-store/src/stored.rs`.
   - But reference spans cover the whole call or member expression. For example `atlas-lang-ts/src/extract/references.rs` passes the call span, not the name token.
   - Query `Location` (`atlas-query/src/model.rs`) drops columns.
   - Decision in §5.4.
3. **`TargetSpec` collides with Serena name paths.**
   - `TargetSpec::parse` treats a leading `/` as an endpoint.
   - `parse_file` treats any text containing `/` as a file path, so `Class/method` becomes a `File` target.
   - Name-path parsing therefore gets its own parser, used by the new modes and `atlas_edit` (§5.1). The existing `info` mode is unchanged, except for one retry described there.
4. **TypeScript 7.**
   - The local npx cache has `typescript@latest` = 7.0.2 with only `tsc` and no `tsserver.js`.
   - typescript-language-server 5.x needs a TS ≤ 6 tsserver.
   - Discovery must pin `typescript@5` and refuse TS 7 with a clear message.
   - TS 7's native LSP (`tsgo --lsp`) is a possible later backend (open question Q8).
5. **The current agent guidance contradicts editing through Code Atlas.**
   - "Edit using your own tools" appears in `docs/agent-integration.md` step 5, in `atlas-mcp/src/descriptions.rs` INSTRUCTIONS item 4, and in the protocol test `should_tell_agents_to_read_files_they_will_edit_with_their_own_tool`.
   - All three must change.
   - New rule: use `atlas_edit` for renames and symbol-level edits. After `atlas_edit` changes a file, re-read it with your own tool before editing it natively, because the harness's read-before-edit state goes stale.
6. **CLAUDE.md additions** (no breaking change to core contracts):
   - The crate list goes from 17 to 20.
   - New contract: LSP answers render as `[compiler]` (`certainty.class = compiler_verified`, `source = "lsp:<server> <version>"`), valid for the configuration the server loaded, and are never written to the store. This keeps `Certainty` unchanged and incremental ≡ full untouched.
   - New exit code `5` (edit refused).
   - The MCP server is no longer read-only.
7. **rust-analyzer as an LSP can write `Cargo.lock`**, as it does in the IDE, when `cargo metadata` finds the lock missing or stale.
   - The SCIP runner snapshots and restores the lock.
   - A long-lived server cannot restore it, so the pool records the lock hash at spawn and reports a change as a warning in the next answer, without restoring it (open question Q5).
   - The cargo target dir is set outside the repo, as `atlas-scip` does.
8. **Nothing in `atlas-core` changes.**
   - Method-level overrides reuse `EdgeKind::Implements`, with attr `via = "interface" | "class"`, matching the Rust convention where a trait method impl `implements` the trait method.
   - Mock references reuse `EdgeKind::References`, with attr `mock = "<rule>"`.

---

## 2. Architecture

### 2.1 Crates and dependencies

```
atlas-core
  └─ atlas-lsp      (core, serde, serde_json, url, thiserror, tracing)          no atlas-project, no engine
  └─ atlas-edit     (core, project, blake3, regex, ignore, tempfile, fd-lock, serde, thiserror, tracing)
atlas-engine        (+ public hooks: language_units, write_then_refresh, analyze_text)
atlas-query         (+ graph symbol surface: find, graph outline, graph references, hierarchy, name-token positions)
atlas-ide           (core, project, engine, query, scip [rust-analyzer locator], lsp, edit, serde, thiserror, tracing)
atlas-mcp, atlas-cli → atlas-ide
```

- **`atlas-lsp`**
  - Contents: framing, a blocking client (reader thread plus writer mutex), server-request handling, the progress and status trackers, `LineIndex` and position encodings, file URIs, process spawn in its own process group, `ServerSpec` for rust-analyzer and TypeScript, `LspPool`, and `OpenDocs` sync.
  - It receives a ready `ServerCommand { program, args, env, cwd }` and does no project discovery, except the TS command resolution in `discover.rs`, which takes plain inputs.
  - The kill-group helper (about 15 lines) is copied from `atlas-scip/src/process.rs`, not shared, so the crate does not depend on atlas-scip.
- **`atlas-edit`**
  - Contents: the `EditPlan` model, `EditScope` (the path guard), hashing, atomic application with rollback, resource operations, the cross-process edit lock, occurrence ids, `replace_content`/`replace_in_files` planning, symbol regions (leading docs and attributes, indentation), insert/replace/delete planning, and hunk rendering.
  - It has no LSP and no engine dependency.
- **`atlas-ide`**
  - `IdeService`: target and name-path resolution, mapping LSP results to (repo, path, line, col, enclosing symbol), turning a `WorkspaceEdit` into an `EditPlan`, graph extras, the plan cache, and the post-edit barrier, LSP notify and diagnostics.
  - Request and answer types implement `atlas_query::RenderText`, so CLI and MCP render identically.

### 2.2 Third-party dependencies (`[workspace.dependencies]`)

| Crate | Why | Cost |
|---|---|---|
| `url = "2"` | Correct `file://` URI conversion (`Url::from_file_path` / `to_file_path`, percent-encoding). LSP URIs with spaces or unicode break hand-rolled encoders. | Already in `Cargo.lock` (2.5.8, via `ureq`): no new compile |
| `regex = "1"` | Regex mode of `replace_content`/`replace_in_files` and declaration lookup by regex | Already in `Cargo.lock` (1.13.1): no new compile |

Rejected:
- `lsp-types`: it carries a union-heavy, strictly typed surface, and we need lenient `#[serde(default)]` parsing of server quirks, for example `DocumentSymbol[]` vs `SymbolInformation[]` and `null` results.
- `tower-lsp`, `async-lsp`: async servers or frameworks; we are a blocking client.
- Any diff crate: hunks are rendered directly from the known edit ranges.

### 2.3 Flow of an exact operation

```
MCP/CLI ─▶ IdeService
   1 barrier: QueryEngine::fresh() (Engine::ensure_fresh)
   2 resolve target: graph entity + name_span (artifact) ──hash check vs disk
   3 pool.get(unit) ─▶ spawn/initialize/ready (quiescent) if cold
   4 sync: change set since server's synced generation ─▶ didOpen/didChange overlays | watched files | restart
   5 LSP request (position in negotiated encoding)
   6 map results ─▶ (repo,path,line,col) + enclosing graph symbol + snippet; add graph-only extras (tagged)
 edit ops additionally:
   7 WorkspaceEdit ─▶ EditPlan (validate scope, ranges, old text)       [dry_run stops here: diffs + plan id]
   8 Engine::write_then_refresh(paths, || atlas_edit::apply(plan))      (index lock held during writes)
   9 pool notify (didChange/didOpen, watched files, didSave) ─▶ diagnostics delta
```

---

## 3. Language-server lifecycle (`atlas-lsp`, `atlas-ide/src/servers.rs`)

### 3.1 Units

- `UnitKey { project, language: ServerKind::{RustAnalyzer, TypeScript} }`.
- Roots come from a new public `Engine::language_units() -> Vec<LanguageUnit { repo, ecosystem, root: PathBuf, manifest: RelPath }>`. It reuses `scip_units`/`indexing_unit` (`atlas-engine/src/scip.rs`): one entry per Cargo workspace (members folded in), per standalone crate and per npm package.
- rust-analyzer: `workspaceFolders` = repo roots that contain Rust; settings `linkedProjects` = absolute manifest paths of every Cargo unit. One process serves cross-repo path dependencies exactly, as a VS Code multi-root workspace does.
- TypeScript: `workspaceFolders` = every TS repo root. tsserver creates configured projects per tsconfig when a file opens.
- Opt-out for huge projects: `lsp.unit = "repo"` (one server per (repo, language)).
- The unit fingerprint is the sorted roots plus settings. `Session::sync_registry` → `IdeService::reload(project)` restarts units whose fingerprint changed.

### 3.2 Discovery and consent (`atlas-lsp/src/discover.rs`, `atlas-ide/src/servers.rs`)

**rust-analyzer:**
- Make `atlas_scip::RustAnalyzerScip::resolve` public as `pub fn locate_rust_analyzer() -> Result<(PathBuf, String), String>`, which keeps the PATH, then `rustup which`, then `--version` probe order.
- No network is involved.
- Spawning executes build scripts and proc macros, as opening the project in an IDE does. This is gated by `lsp.enabled` (default **on**, see Q1).

**typescript-language-server**, resolved in this order. Each candidate is probed with `--version` (5 s timeout):
1. Project setting `lsp.typescript_command` (absolute path, or an argv list).
2. `typescript-language-server` on `PATH`.
3. `<ts repo>/node_modules/.bin/typescript-language-server`, for TS repos in registry order.
4. npx cache without network: `npx --no-install typescript-language-server --version`.
5. Only if `lsp.download = on` (default **off**): `npx --yes -p typescript-language-server@5 -p typescript@5 typescript-language-server --stdio`. `npm ping` must answer first, as in `atlas-scip/src/typescript.rs`.
6. Otherwise `LspError::Unavailable` with the exact hint:
   > `typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5), set code-atlas project config <p> --lsp-ts-command <path>, or allow download with --lsp-download on.`

**tsserver selection:**
- Use `<repo>/node_modules/typescript/lib/tsserver.js` when present and its version is below 7. This matches the IDE's "workspace version", which is deterministic for agents.
- Otherwise use the server's own `typescript@5`.
- It is passed explicitly as `initializationOptions.tsserver.path`.
- A TS ≥ 7 workspace install gives `Unavailable("TypeScript 7 has no tsserver; install typescript@5 alongside or use the bundled one")`.
- The chosen versions (server, TypeScript) appear in every answer header.

`code-atlas doctor` prints each step's result. `atlas_status` shows the state of the servers.

### 3.3 Initialization and readiness

Client capabilities:
- `general.positionEncodings: ["utf-8","utf-16"]`
- `workspace.{workspaceFolders, configuration, didChangeWatchedFiles{dynamicRegistration:false}, workspaceEdit{documentChanges, resourceOperations:[create,rename,delete], failureHandling:"abort"}}`
- `textDocument.{rename{prepareSupport}, documentSymbol{hierarchicalDocumentSymbolSupport}, publishDiagnostics{versionSupport}, diagnostic{}, references, definition, declaration, implementation}`
- `window.workDoneProgress: true`
- `experimental.serverStatusNotification: true`
- `processId` = our pid, so servers exit when we die.

Settings for rust-analyzer: initialization options plus `workspace/configuration` replies for section `rust-analyzer`:

```json
{ "linkedProjects": ["…/Cargo.toml"], "cargo": { "targetDir": "<cache>/lsp-target/<hash(unit)>", "buildScripts": { "enable": true } },
  "procMacro": { "enable": true }, "checkOnSave": true, "check": { "command": "check" },
  "files": { "watcher": "server" }, "cachePriming": { "enable": true } }
```

Settings for TypeScript (initialization options):

```json
{ "hostInfo": "code-atlas", "tsserver": { "path": "…", "logVerbosity": "off" }, "maxTsServerMemory": 3072,
  "preferences": { "providePrefixAndSuffixTextForRename": true, "allowRenameOfImportPath": false } }
```

Readiness (`ServerState`: `Starting → Indexing { message } → Ready | Exited { code, stderr_tail } | Failed { reason }`):
- **rust-analyzer:** ready when `experimental/serverStatus` reports `quiescent: true` and `health != "error"`.
  - `$/progress` tokens (`rustAnalyzer/Fetching`, `…/Indexing`, `…/Building`) feed the `Indexing` message.
  - Exact requests **wait for quiescence**, because rust-analyzer answers partially while loading.
  - Timeout `lsp.ready_timeout_secs` (default 300) → `LspError::NotReady { waited, status }`.
- **TypeScript:** ready after the `initialize` response.
  - Before each request the target file is opened. tsserver blocks until its project loads, so the first request gets the long timeout (120 s).
  - `$/typescriptVersion` is recorded.

Server-to-client requests answered:
- `workspace/configuration`: the unit settings per section.
- `window/workDoneProgress/create`, `client/(un)registerCapability`, `workspace/*/refresh`, `window/showMessageRequest`: `null`.
- `workspace/workspaceFolders`: the folders.
- `workspace/applyEdit`: `{applied:false}`. Servers never write; all writes go through `atlas-edit`.
- Anything else: `-32601`.
- `window/logMessage` → `tracing::debug!`.

### 3.4 Timeouts, crashes, restarts

- Timeouts:
  - `initialize`: 60 s.
  - Requests: 60 s by default; on expiry send `$/cancelRequest` and return `LspError::Timeout { method, after }`.
  - `shutdown`: 5 s.
- When the reader sees EOF or a framing error, every pending request fails with `ServerExited { stderr_tail }` (8 KiB ring) and the state becomes `Exited`.
- On the next `pool.get`, the server restarts. After 3 exits within 10 min the unit becomes `Unavailable` for 10 min with the last stderr tail. This mirrors auto-SCIP's backoff.
- Messages over 64 MiB, or a missing `Content-Length`, give `LspError::Protocol` and kill the server.

### 3.5 Process groups and shutdown

- Spawn with `CommandExt::process_group(0)`. This is safe std, so `unsafe_code = "forbid"` holds.
- Shutdown sequence: `shutdown` → `exit` → wait 2 s → `kill -s KILL -- -pgid` → reap. This also kills rust-analyzer's `proc-macro-srv` child.
- `LspPool` implements `Drop`. `atlas-mcp/src/session.rs::shut_down` calls `ide.shutdown()` before `persist_similarity`.
- If the parent crashes, `processId` lets servers notice and exit, and stdin EOF ends both servers.
- Unix-only, like SCIP.

### 3.6 Memory and pool limits

- rust-analyzer uses about 2–3 GB RSS on large workspaces (comparable to the SCIP run). tsserver uses 0.3–1.5 GB, capped by `maxTsServerMemory`.
- Defaults: `lsp.max_servers = 2` (one per language for the bound project), `lsp.idle_secs = 600`, `lsp.prewarm = off`.
- A reaper thread (every 30 s) shuts down idle servers. Over the limit, the least-recently-used server is evicted (graceful shutdown).
- `atlas_status` shows pid, state, uptime and RSS (`ps -o rss= -p <pid>`), plus the versions.
- Several MCP processes for the same project each own their servers: N × about 3 GB. This is documented; a shared daemon is open question Q6.
- rust-analyzer as LSP and as SCIP indexer use different target dirs (`lsp-target/` vs `rust-target/`), so neither blocks the other on cargo locks, at the price of double build-script compilation (documented).

### 3.7 CLI one-shot vs MCP warm

- **MCP:** servers live in `Session` (an `IdeService` per session) and stay warm between calls.
- **CLI:** every IDE command spawns, waits for readiness, runs, and shuts down. Documented expectations:
  - TS fixtures: 2–5 s.
  - rust-analyzer on fixtures: 5–15 s.
  - rust-analyzer on this workspace: 30–90 s cold. Build scripts (including the tree-sitter C grammars) are cached in `lsp-target/`, so later runs take 10–25 s.
- `code-atlas lsp check` spawns each unit once and reports readiness time. The CLI one-shot mode is meant for scripts and CI, not interactive loops.

---

## 4. Document sync and freshness (`atlas-lsp/src/sync.rs`, `atlas-ide/src/sync.rs`)

Invariant: **before any LSP request, the server's view equals disk, which equals what the barrier indexed.**

- **Per-server state**:
  - `OpenDocs { path → (version, ContentHash) }`.
  - `synced_generation: Option<RunId>`, the engine generation the server reflects. At spawn it is the generation after the barrier that preceded the spawn; the server reads disk itself.
- **Before each IDE operation**:
  1. Run the barrier (`Engine::ensure_fresh`).
  2. If the generation moved past `synced_generation`, read the change set from the store's run ledger since that run (the same data as `ChangesAnswer`: analysed, deleted and moved files).
     - **≤ 64 changed files:** for each, `didOpen` (or `didChange`, full text, `version += 1`) with the disk text, so the overlay equals disk and is applied synchronously. Deleted files get `didClose` plus a `Deleted` watched-file event.
     - **> 64 and ≤ 1000:** send `workspace/didChangeWatchedFiles` for all. rust-analyzer then waits for quiescence (≤ 30 s). For TypeScript, the files needed by the request are additionally opened as overlays.
     - **> 1000** (branch switch): restart the unit.
  3. For every open document, compare the cached `(len, mtime)`, hash on change, then `didChange` or `didClose`.
- **Edits by Code Atlas.** `Engine::write_then_refresh` applies the writes, then `IdeService` sends `didChange` (open documents) or `didOpen` (changed files, ≤ 64) with the new text, plus `Created`/`Deleted`/`Changed` watched-file events. It then sends `didSave` for changed Rust files only when diagnostics are requested, which triggers flycheck.
- **External edits** (the agent's own Edit tool, the user's editor) are caught by the barrier change set on the next call. Without `--watch` the barrier stat-scans; with a watcher the residual undelivered-event risk is the same as for queries.
- **New engine API**: `pub fn write_then_refresh<R>(&self, files: &[(RepoName, RelPath)], write: impl FnOnce() -> R) -> EngineResult<(R, FreshnessReport)>`.
  - It takes the inner mutex and then `<db>.index.lock` (the same order as the barrier), so no barrier in any process indexes a half-applied multi-file edit.
  - It runs `write`, marks the watcher dirty (`WatchSignals::mark_event`), releases, then calls `ensure_fresh()`. Single-flight guarantees a scan launched after the writes.
  - SCIP needs no lock: its input verification already rejects files changed during a run.

---

## 5. Targets, name paths and positions

### 5.1 `SymbolTarget` parser (`atlas-ide/src/target.rs`)

Order of attempts:
1. Full key or id: `EntityKey`/`EntityId` parse, unchanged.
2. Position, `[repo:]path:LINE:COL` (1-based line, 1-based UTF-8 column in the display, converted to the server's encoding). This is new; `TargetSpec` cannot express it.
3. File line, `[repo:]path:LINE`: innermost symbol, as `file_targets` does.
4. **Name path**:
   - `A/b/c`, `/A/b` (absolute, from the file's top level), `b[i]` (0-based index among same-named siblings in source order, so `~n` keys map to it).
   - Rust impl members match with `<Trait>` segments skipped unless named (`qualifier_matches` rule).
   - Dotted and `::` names (`UsersService.findOne`, `Store::get`) are accepted as name paths too.
5. In `info` mode `TargetSpec` semantics stay unchanged. A `File` result that is not found, for text without a file extension, is retried as a name path, so `UsersService/findOne` works everywhere.

Endpoints are not valid targets for IDE modes or edits; the error says "endpoints have no LSP symbol; use atlas_api".

### 5.2 Name path to entity to LSP position

- **Graph first** (all repos, fast): `entities_by_name(last)` → name-path filter on `SymbolKey` module and item path → `path` restriction (a file, or a directory prefix with an optional `repo:`) → kind include/exclude → overload index.
- The position is the entity's **name token**: the `SymbolFact.name_span` from the file artifact (`atlas_engine::artifact_file_facts`). `Entity` stores only the full `span`.
- `Engine::read_source` checks that the disk hash equals the indexed hash. On mismatch, the barrier runs once (like `SourceReader`), then the operation errors.
- **Ambiguity:**
  - Read modes return `Targeted::Ambiguous` with keys.
  - Edits refuse with the candidate keys and never guess.
- **Files outside the project:** a path outside every registered repo root (after canonicalisation) gives `IdeError::OutsideProject { path, repos }`. Inside a repo but not indexed (gitignored, default-excluded): reads through the LSP are allowed (outline, diagnostics), but edits are refused (§11).

### 5.3 Overloads and duplicates

- TS overload signatures and Rust `#[cfg]` duplicates are separate entities (`~n`). `[i]` selects by source order.
- LSP rename at any overload renames all of them; this is reported.

### 5.4 Reference positions (audit item 6)

- **Exact operations:** line:col from the LSP, converted with `LineIndex` (UTF-8, UTF-16 or UTF-32 as negotiated) over the exact text the server saw.
- **Graph references:** `atlas-query/src/queries/positions.rs::name_token(source, span, name) -> Option<Span>`.
  - It finds the last path segment inside the reference span. It must occur exactly once at an identifier boundary.
  - It runs at query time on hash-verified disk text.
  - It is used for "not renamed: verify" sites and for `cross_repo: "apply"`.
  - On failure the site shows line only, with `column: null`.
- No analyzer bump, no store change. Option B, storing `name_span` in `ReferenceFact` (serde-default field plus analyzer bumps), is deferred.
- `Location` gains `column: Option<u32>` (`#[serde(skip_serializing_if = "Option::is_none")]`) and renders `repo:path:line:col`.

---

## 6. Retrieval modes

| Mode | Serena equivalent | Source | Notes |
|---|---|---|---|
| `info` (default) | none | graph | today's `atlas_symbol` |
| `find` | `find_symbol` | graph (`syntactic`) | name-path pattern, `path`, `depth` (children via `contains`), `body`, `substring`, `kinds`/`exclude_kinds`, `max_matches` (default 20); signature always shown |
| `outline` | `get_symbols_overview` | LSP `documentSymbol` (`source=graph` for the tagged graph outline) | `depth` default 1 |
| `source` | none (replaces `atlas_read`) | disk | `context_lines` |
| `references` | `find_referencing_symbols` | LSP `references` (`includeDeclaration:false`), **plus** graph-only extras | grouped by enclosing symbol (innermost graph symbol at the position; LSP outline when the file is not indexed), one-line snippet ≤ 160 chars |
| `declaration` | `find_declaration` | LSP `definition` (and `declaration` if supported) | target `path:line:col`, or `path` + `regex` with exactly one capture group matching exactly once |
| `implementations` | `find_implementations` | LSP `implementation` | |
| `hierarchy` | none | graph `extends`/`implements` up and down, including TS method overrides (A1) | tagged; rust-analyzer and the TS server lack type hierarchy |
| `diagnostics` | `get_diagnostics_for_file` | LSP | §10 |

Graph-only extras in `references` cover sites the server returned no location for:
- inbound usage edges from other repos,
- HTTP consumers when the target exposes an endpoint,
- mock references,
- the `unresolved_risk` caveat.

They are listed in a separate block, `outside the language server's view:`, with certainty tags and reasons (§7.3).

Text output for references:

```
references UsersService.findOne method acme-api:src/users/users.service.ts:18  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]
acme-api (3)
  UsersController.findOne  src/users/users.controller.ts:21:30  return this.usersService.findOne(id);
  findOne (test)           src/users/users.service.spec.ts:15:28  await expect(service.findOne('u1'))…
  …
outside the language server's view:
  (none)
```

---

## 7. Rename

### 7.1 Algorithm (`atlas-ide/src/edit/rename.rs`)

1. Take the IDE edit mutex (one mutation in flight per process) and run the barrier. Resolve the target (§5) and get the server (wait for readiness). Sync (§4) and open the target file.
2. Send `prepareRename` (if `prepareProvider`). Its range text must equal the entity name; otherwise refuse with `"server and index disagree at <loc>"`. A server error (for example "cannot rename a library item") becomes `Refused { reason }`.
3. Send `rename(newName)` and receive a `WorkspaceEdit` (`changes` or `documentChanges`, including `RenameFile`/`CreateFile`/`DeleteFile`).
4. Convert it to an `EditPlan`:
   - URI → canonical path → `EditScope::resolve`. Any file outside the editable scope makes the **whole** rename refused, listing the files.
   - Read every file and hash it; this hash is `expected`.
   - Versioned edits must match `OpenDocs` versions.
   - Ranges must be in bounds and non-overlapping.
   - **Old-text check:** each edit's original text must be `old_name`, `r#old_name`, or empty (an insertion adjacent to an `old_name` occurrence, as with TS prefix/suffix aliasing). Anything else means the server view is stale: resync by opening the files, retry once, then refuse.
5. Compute the graph extras (§7.3) against the pre-edit graph.
6. `dry_run=true`: return diffs, extras and `plan_id`, and stop.
7. Otherwise `Engine::write_then_refresh(paths, || atlas_edit::apply(&plan, &scope))`, then LSP notify, then the diagnostics delta (new errors only, server-native, settle ≤ 10 s).
8. The response always lists the files changed, the edits per file, the extras, the freshness header and the server versions.

### 7.2 Atomic application (`atlas-edit/src/apply.rs`)

- Take `<db>.edit.lock` (`fd-lock`, cross-process) inside the IDE mutex.
- For each file: read, check the hash against `expected` (else `EditError::Stale` and nothing is written), compute the new bytes in memory, write a `tempfile::NamedTempFile::new_in(parent)`, `sync_all`, copy permissions.
- Commit the files one by one: re-check the hash immediately before `persist` (rename over the target), and keep the original bytes in memory.
- Resource operations come after content edits:
  - `Rename`: the target must not exist unless `overwrite`, which we never request.
  - `Create`: the path must not exist.
  - `Delete`: a file only.
- On any failure, roll back in reverse order (originals written back through temp + rename, renames reversed) and return `EditError::RolledBack { cause }`. If the rollback itself fails, return `EditError::PartiallyApplied { applied, failed }`, rendered as a `DEGRADED`-style first line.
- Line endings and BOM are preserved. Inserted agent text has `\n` converted to the file's dominant line ending.

### 7.3 Graph extras: "not renamed: verify"

- **Candidates:**
  - inbound usage edges to the target and to its `implements`/override family (calls, references, instantiates, type_of, imports, re_exports, mock references),
  - `calls_endpoint` consumers when the target exposes an endpoint,
  - `unresolved_risk` sites with the same name.
- **Selection:** a site is listed when the plan has no edit at its name-token position.
- **Reasons** (`VerifyReason`):
  - `OutsideLanguageServer`: another repo or package the server did not see (the `toCents` case),
  - `Mock`: heuristic jest/vitest mock,
  - `Heuristic`: name-only candidate,
  - `Http`: the client call names a route, not a symbol,
  - `Dynamic`: unresolved dynamic member,
  - `TokenMismatch`: alias, for example `import { x as y }`; the site needs no change and is shown for completeness.
- **Never auto-edited by default.** `cross_repo: "apply"` edits only sites where all of these hold:
  - the edge is `Certainty::is_authoritative()`,
  - the reason is `OutsideLanguageServer`,
  - `name_token` finds exactly the old name,
  - the file hash matches.
  These include import bindings, located inside the import statement span. Edited sites are listed under `applied from the graph [resolved]`. Mocks, HTTP and heuristic sites are never edited.

```
rename UsersService.findOne -> getById  applied  (typescript-language-server 5.1.3, TypeScript 5.9.3)
changed 3 files, 4 edits:
  acme-api:src/users/users.service.ts        1
  acme-api:src/users/users.controller.ts     1
  acme-api:src/users/users.service.spec.ts   2
not renamed, verify:
  (none)
diagnostics after edit: no new errors (settled)
fresh run=57 files_updated=3
```

---

## 8. Symbol edits (`atlas-edit/src/symbol.rs`)

**Spans:**
- The declaration span is the graph `span`: tree-sitter item node, hash-bound.
- The **leading region** is scanned upward from the declaration's first line. It is the maximal run of lines that are doc or line comments (`///`, `//`, `/* … */`, `/** … */`, `*` continuation lines), Rust outer attributes (`#[…]` with bracket balance) or TS decorators (`@x(…)` with paren balance), and it stops at a blank line or code.
- When a warm server's `documentSymbol` disagrees with the graph (the symbol whose `selectionRange` contains our `name_span` start does not have a `range` covering our declaration span), the edit is refused. A server is never spawned just for this check.

**Indentation:**
- `base` is the leading whitespace of the declaration's first line.
- Strip `base` from the new text's first line if present.
- If every non-blank following line already starts with `base`, keep them; otherwise prefix `base` to each.

**Operations:**
- `replace_body`: replaces the declaration span. If the new text starts with doc or attribute lines, the leading region is replaced too; otherwise it is kept.
- `insert_before`: inserts above the leading region, re-indented, followed by a newline plus one blank line when the symbol is a top-level item or its preceding line is blank.
- `insert_after`: inserts after the declaration's last line, preceded by a newline (plus a blank line, by the same rule).
- `safe_delete`:
  - Requires a server.
  - Blockers are LSP references (excluding those inside the symbol's own region), plus graph-only extras (cross-repo, HTTP consumers of exposed endpoints, mocks, dynamic same-name sites).
  - Any blocker means `Refused`, with the full list; nothing is deleted.
  - Otherwise it deletes the leading region, the declaration and its line terminator, and collapses one of two adjacent blank lines.

replace_body and insert work without a language server; they are syntactic operations on graph spans. Post-edit diagnostics are attached only when a server is warm.

---

## 9. Text edits (`atlas-edit/src/text.rs`): pure filesystem operations

- **Occurrence id:** `o-` followed by the first 10 hex characters of `blake3(repo ‖ path ‖ file content hash ‖ start_byte ‖ end_byte)`.
  - It is stable for unchanged content and invalid after any change to the file, by design.
  - Ids are checked for uniqueness within a call.
- **`replace_content { path, pattern, text, regex, all }`:**
  - Literal or regex. Regex uses the `regex` crate with multi-line and `.` matching newlines; replacement uses `$1`/`${name}`.
  - Zero matches → `NoMatch`. More than one match without `all` → `MultipleMatches { count, ids }`.
- **`replace_in_files { pattern, text, regex, include, exclude, occurrences, expected_count, dry_run }`:**
  - Files: walk every repo root with `ignore::WalkBuilder` (respecting `.gitignore`, default excludes, the repo's include/exclude from `atlas-project` globs, `max_file_bytes`), skipping binaries (NUL in the first 8 KiB). Optional `include`/`exclude` globs may be prefixed `repo:`.
  - **Apply rule:**
    - Without `occurrences` or `expected_count`, a non-dry-run call is treated as a dry run and says so.
    - `expected_count` must equal the match count.
    - `occurrences` must all exist and are the only ones replaced.
  - Dry run returns per-file hunks with the occurrence id at each change line.
- Apply goes through the same `EditPlan` and `apply` path as everything else.

---

## 10. Diagnostics (`atlas-ide/src/diagnostics.rs`)

- **Pull** (`textDocument/diagnostic`) when the server advertises `diagnosticProvider`. Otherwise **push**: open or update the file at version v, then wait until a `publishDiagnostics` for that URI arrives that has `version ≥ v` (or no version) and was received after the change, and no newer one arrives for 300 ms. Maximum 10 s.
- **rust-analyzer flycheck** (cargo check, push-only) is what the IDE shows on save.
  - `mode=diagnostics` sends `didSave`, waits for the flycheck `$/progress` end (≤ `lsp.check_timeout_secs`, 120), and merges native and flycheck results.
  - Post-edit deltas in `atlas_edit` use native diagnostics only. The answer carries the label `flycheck: complete | timed_out | not_run`.
- **Grouping:** severity (error > warning > info > hint) → innermost graph symbol at the start position (`<file>` when none) → `line:col code message`. Optional `lines: "START-END"` and `min_severity`.
- **Caveat:** if TS reports `TS2307 Cannot find module` on import lines, add "the language server cannot resolve <module>; references through it are invisible to exact operations". This is the `node_modules`-missing case.

---

## 11. Safety

| Rule | Mechanism |
|---|---|
| Edits only inside registered repos | `EditScope::resolve(path)` canonicalises and requires the path to be under a repo root. It refuses symlinks (`symlink_metadata`), `.git/`, default-excluded directories (`node_modules target dist build .next coverage vendor`), and gitignored files (an `ignore` matcher per repo). One refused file refuses the whole plan. |
| Every response lists the files changed | `EditOutcome.files: Vec<ChangedFile { repo, path, edits, before, after }>`, also on dry runs (as "would change") |
| Dry run for every operation | `dry_run: true` returns diffs and `plan_id`. MCP plan cache: 32 plans, 15 min TTL. CLI: `--plan-out plan.json`, then `edit apply plan.json`. |
| Refuse stale plans | Plans carry `expected: ContentHash` per file, re-checked at apply and again immediately before each rename |
| Concurrency | In-process IDE mutex; `<db>.edit.lock` across processes; writes under the engine index lock (`write_then_refresh`); SCIP protected by its own input verification; LSP sync serialised per server |
| Residual risk | An external editor writing between the final hash check and the rename (a microsecond window); documented in trust-and-limitations |
| No formatting | Edits do not run rustfmt or prettier; documented |

---

## 12. Graph-side work (audit gaps and earlier decisions)

- **A1: TS method-level overrides and the heritage label bug** (`atlas-resolve/src/output.rs::ts_structure`).
  - For each class method `m` of class C with a resolved heritage target T (interface or class), find the nearest member named `m` in T or its resolved ancestors (bounded by a guard).
  - Emit `implements` from `C.m` to `T.m` with attr `via`.
  - Certainty: `resolved`, weakened to `heuristic` through `dist-to-src` hits.
  - Drop the `references` edge that `inherited()` in `atlas-lang-ts/src/extract/classes.rs` produces through `TypeRef` when the same (from, to) gets a heritage edge. This is the bug where dependents and dependencies label heritage as `references`.
  - Bump `RESOLVER_VERSION` (3 after R2 → **4**) and re-pin the resolver, detector and engine pins.
  - New fixture `fixtures/adversarial/ts-overrides/` (with `tsconfig.json` and `package.json`): interface `PaymentProvider`, abstract `BaseProvider`, `StripeProvider extends BaseProvider implements PaymentProvider`, `PaypalProvider implements PaymentProvider`, a `checkout.ts` calling through the interface, and a spec with `{ charge: jest.fn() } as unknown as PaymentProvider`.
  - `unused` excludes methods with an outbound method-level `implements` (precise instead of class-level).
- **A2: `unused` honesty.**
  - `--kind config_key` (and `env_var`) adds `Caveat::Note("config keys and env vars are not checked for use: their definitions (.env, config files) are not indexed")` instead of an empty silent answer.
  - New `UnusedClass::TestOnly` and `UnusedAnswer.test_only: Listing<…>`: every inbound usage edge comes from test entities or files (`is_test`), mocks included.
  - The MCP description is fixed in C1.
- **A3: jest/vitest mocks.** New detector `detector!(MockDetector, "test-mocks", 1)` in `atlas-detect/src/detectors/mocks.rs`.
  - Patterns: `{ m: jest.fn() | vi.fn() }`, `x.m.mockResolvedValue|mockReturnValue|mockImplementation…(`, `expect(x.m).toHaveBeenCalled…`, `jest.spyOn(obj, 'm')`.
  - Type of `x`:
    - a cast `as T` / `as unknown as T`,
    - `{ provide: T, useValue: x }`,
    - `jest.Mocked<T>` / `createMock<T>()`,
    - otherwise the naming convention `fooService` → imported `FooService`.
  - Emits a `references` edge from the enclosing test symbol to `NodeRef::Unresolved { path: [T, m], from }`, `heuristic(0.6, "jest-mock-member")`, attr `mock`.
  - `tests_for`: new `TestReach::Mocked` ("mocks it (does not exercise it)").
- **A4: macro hint.** In `target.rs::not_found_hint`, a name with `unresolved_by_names` references of reason `macro_generated` gets "`<name>` looks macro-generated (N macro call sites, e.g. <loc>); macro-generated items have no symbol: search for the macro definition".
- **A5: CTX-1, context reuse.**
  - Extract `lexical_and_signature_candidates(graph, index, text, signature)` from `similar.rs` into `queries/reuse.rs`.
  - In `context.rs`, when the task text contains an "add/create/implement/introduce/new/write" intent, add these candidates (lexical hits on name fields at or above `LEXICAL_FRACTION` of the top score, plus a signature-shape match) to the REUSE section, with `why` naming the signal.
  - Measured by `atlas-eval retrieval` (reuse recall, must_change hit rate).
- **A6: `similar` from code text.**
  - `Engine::analyze_text(language, text) -> EngineResult<FileFacts>` uses the toolchain analyzer in memory, with no store writes.
  - The structural path uses `SimilarityIndex::near_duplicates_of_tokens` (already present); the lexical path uses symbol names and signatures.
  - `SimilarRequest.code: Option<(Language, String)>`.
- **A7: `via_http`.** `TraverseRequest.via_http: bool`.
  - Callers of H: walk `exposes_endpoint` to E, then the inverse of `calls_endpoint` to F, plus `inferred` links.
  - Callees: the reverse.
  - The hop renders `via HTTP GET /users/{id}`, keeps the edge certainty (always heuristic or inferred), and makes the item `uncertain`.
- **A8:** `Location.column` and the `name_token` locator (§5.4).

---

## 13. MCP surface

### 13.1 Tools (16 → 15)

- New: `atlas_edit`.
- Removed: `atlas_read` (→ `atlas_symbol mode=source`, `context_lines`) and `atlas_changes` (→ `atlas_status since_run`).
- Optional further offset (Q3): fold `atlas_callees` into `atlas_callers direction`.

### 13.2 `atlas_symbol`

```rust
arguments!(SymbolArgs {
  target: Option<String>,          // name | name path (A/b, /A/b, b[1]) | [repo:]path[:line[:col]] | key. Required except outline/diagnostics given `path`
  mode: Option<SymbolMode>,        // info|find|outline|source|references|declaration|implementations|hierarchy|diagnostics
  path: Option<String>,            // find: restrict to [repo:]file or dir; outline/diagnostics: the file; declaration: file searched by regex
  depth: Option<u32>,              // find/outline/hierarchy 0-3
  body: Option<bool>,              // info/find
  substring: Option<bool>,         // find: last segment substring match
  kinds: Option<Vec<String>>,      // find: include kinds (function, method, class, …)
  exclude_kinds: Option<Vec<String>>,
  max_matches: Option<usize>,      // find, 1-200
  regex: Option<String>,           // declaration: one capture group locating the usage in `path`
  source: Option<ReadSource>,      // lsp (default) | graph — outline/references only
  context_lines: Option<u32>,      // source 0-100
  lines: Option<String>,           // diagnostics "START-END"
  min_severity: Option<Severity>,  // diagnostics: error|warning|info|hint
});
```

`SymbolMode`, `ReadSource` and `Severity` are unit enums **without doc comments**. schemars then emits a flat `{"type":"string","enum":[…]}`; doc comments would become `oneOf` with descriptions and cost tokens.

Description (about 95 tokens):

> Symbol lookup and IDE navigation. mode: info (default: definition, signature, docs, relation counts), find (name-path search like 'UsersService/findOne', substring, kinds), outline (a file's symbols), source (code from disk), references | declaration | implementations (exact, from rust-analyzer or typescript-language-server), hierarchy (supertypes/subtypes from the graph), diagnostics (compiler errors for a file). Exact modes fail with an install hint when no server is available; source='graph' returns tagged graph results instead.

### 13.3 `atlas_edit` (`annotations(destructive_hint = true, read_only_hint = false, idempotent_hint = false)`)

```rust
arguments!(EditArgs {
  operation: EditOperation,        // rename|replace_body|insert_before|insert_after|safe_delete|replace_content|replace_in_files|apply
  target: Option<String>,          // symbol operations (same forms as atlas_symbol)
  path: Option<String>,            // replace_content: [repo:]file
  new_name: Option<String>,        // rename
  text: Option<String>,            // new body, inserted code, or replacement (supports $1 with regex)
  pattern: Option<String>,         // replace_content / replace_in_files
  regex: Option<bool>,             // pattern is a regex (default literal)
  all: Option<bool>,               // replace_content: allow several matches
  include: Option<Vec<String>>,    // replace_in_files globs, optional repo: prefix
  exclude: Option<Vec<String>>,
  occurrences: Option<Vec<String>>,// ids from a dry run
  expected_count: Option<usize>,   // replace_in_files guard
  cross_repo: Option<CrossRepo>,   // rename: report (default) | apply (authoritative graph sites only)
  plan: Option<String>,            // apply: plan id from a dry run
  dry_run: Option<bool>,           // default false; replace_in_files without occurrences/expected_count is always a dry run
});
```

Per-operation validation (`atlas-mcp/src/arguments.rs::edit_request`) lists the required and forbidden fields per operation and errors with "operation X takes Y". For example, `rename` needs `target` and `new_name`, and forbids `pattern`.

Description (about 100 tokens):

> Edit code safely. operation: rename (exact, every file the language server sees; sites it cannot see are listed to verify), replace_body / insert_before / insert_after (a symbol's definition), safe_delete (refuses and lists references), replace_content (one file, literal or regex), replace_in_files (apply needs occurrences or expected_count from a dry run), apply (a plan id). dry_run=true returns diffs and a plan id. Refuses if a file changed since planning. Lists the files changed and refreshes the index. Re-read changed files before editing them with your own tool.

### 13.4 Other argument changes

- `TraverseArgs.via_http: Option<bool>` on callers and callees.
- `SimilarArgs` gains `code: Option<String>` and `language: Option<String>` (`rust|ts|tsx|js`), with exactly one of target, snippet or code.
- `NoArgs` for status becomes `StatusArgs { since_run }`.
- The `UNUSED` description drops "config": "Candidate unused functions, endpoints, auth modes and events, plus test-only symbols…".

### 13.5 Instructions (about 430 → about 330 tokens)

- Merge items 5–6.
- Replace item 4 with: "To rename or rewrite a symbol, use atlas_edit (exact across files) instead of find-and-replace; after it changes a file, re-read that file before editing it yourself."
- Replace item 7 with: "after editing, call atlas_status since_run …".

### 13.6 Token estimate (method: serialized `tools/list` + instructions, bytes ÷ about 3.6)

| Change | Δ tokens |
|---|---|
| `atlas_symbol`: mode enum + 13 fields + longer description | +520 |
| `atlas_edit`: new (15 fields) | +700 |
| `via_http` × 2, `similar code/language` | +110 |
| remove `atlas_read`, `atlas_changes` (status +since_run) | −350 |
| instructions rewrite, shorter descriptions | −150 |
| **Net** | **about +830 (+13%)** |
| optional: fold `atlas_callees` (−200) and `atlas_explain` into `atlas_symbol mode=explain` (−150) | **about +480 (+8%)** |

C1 measures the real value before and after. It adds `should_keep_tool_schemas_within_the_budget`, which pins the `tools/list` + instructions byte length at ≤ 1.10 × the pre-epic measurement.

---

## 14. CLI (`atlas-cli/src/cli.rs`, `commands/ide.rs`, `commands/edit.rs`)

```
code-atlas find <NAME_PATH> [--path P] [--depth N] [--body] [--substring] [--kind K]… [--exclude-kind K]… [--max N] [--json]
code-atlas outline <[repo:]PATH> [--depth N] [--source graph]
code-atlas refs <TARGET> [--source graph]
code-atlas decl <[repo:]PATH:LINE:COL> | decl --path P --regex R
code-atlas impls <TARGET>
code-atlas hierarchy <TARGET> [--depth N]
code-atlas diagnostics <[repo:]PATH> [--lines A-B] [--min-severity warning]
code-atlas edit rename <TARGET> <NEW> [--cross-repo report|apply]
code-atlas edit replace-body|insert-before|insert-after <TARGET> (--text T | --text-file F|-)
code-atlas edit safe-delete <TARGET>
code-atlas edit replace <[repo:]PATH> --pattern P --text T [--regex] [--all]
code-atlas edit replace-in-files --pattern P --text T [--regex] [--include G]… [--exclude G]… [--occurrence ID]… [--expected-count N]
code-atlas edit apply <PLAN.json>                    # every edit subcommand: --dry-run, --plan-out F, --json
code-atlas lsp check                                 # spawn, ready, report per unit
code-atlas query callers|callees … --via-http ;  query similar --code - --language ts
code-atlas project config <p> --lsp on|off --lsp-download on|off --lsp-ts-command PATH --lsp-idle-secs N --lsp-max-servers N --lsp-unit project|repo
```

- Exit code `5`: edit refused (stale plan, references block safe_delete, ambiguous target, scope violation).
- A language server that is unavailable gives `1` with the hint.
- Help text states that one-shot IDE commands start a server per invocation (§3.7).
- Settings: `atlas-project/src/registry.rs` gains `LspSettings { enabled: true, download: false, typescript_command: None, idle_secs: 600, max_servers: 2, ready_timeout_secs: 300, check_timeout_secs: 120, unit: Project }` in `ProjectSettings`, using `#[serde(default)]`, so old registries parse unchanged.

---

## 15. No-fallback behaviour

| Request | Server available | No server |
|---|---|---|
| references, declaration, implementations, outline, diagnostics | exact, `[compiler]`, server versions in the header | `LspError::Unavailable` with the install hint and "retry with source='graph' for tagged graph results" (outline and references only) |
| `source='graph'` (outline, references) | graph answer, every relation tagged, caveat "graph references are not exhaustive" | same |
| find, info, source, hierarchy | graph (never needs a server) | same |
| rename, safe_delete | exact | refused with the hint; nothing written |
| replace_body, insert_* | syntactic graph spans, diagnostics delta | same, diagnostics `unavailable` |
| replace_content, replace_in_files | filesystem | same |
| Existing graph tools (callers, impact, unused, …) | unchanged | unchanged |

---

## 16. Testing

**Conventions:** `should_*` names, exact asserts, fixtures copied into `tempfile` directories, never edited in place.

- **Protocol (in-process).** `atlas-lsp` is generic over a `Transport { reader: impl Read, writer: impl Write }`. Tests drive a scripted in-memory server thread. Names:
  - `should_frame_a_message_with_content_length_in_bytes`
  - `should_read_two_messages_from_one_chunk`
  - `should_reject_a_header_without_content_length`
  - `should_correlate_out_of_order_responses`
  - `should_answer_workspace_configuration_with_the_unit_settings`
  - `should_answer_unknown_server_requests_with_method_not_found`
  - `should_refuse_workspace_apply_edit`
  - `should_convert_utf16_columns_around_astral_characters`
  - `should_round_trip_file_uris_with_spaces_and_unicode`
- **Fake server process.**
  - *As built:* the binary lives in the unpublished test crate `atlas-test-lsp`, and tests locate it with `atlas_test_lsp::fake_lsp_path()` instead of `CARGO_BIN_EXE_atlas-fake-lsp`.
  - `crates/atlas-lsp/src/bin/atlas-fake-lsp.rs`, driven by a JSON scenario: canned responses per method, delays, `crash_after`, progress and status sequences, a grandchild sleeper.
  - It is a real `[[bin]]`, so integration tests get `CARGO_BIN_EXE_atlas-fake-lsp`. libtest helpers cannot be used: they print to stdout and would corrupt the framing.
  - Names:
    - `should_fail_pending_requests_when_the_server_exits`
    - `should_time_out_a_request_and_send_cancel_request`
    - `should_kill_the_process_group_on_shutdown_timeout`
    - `should_report_the_stderr_tail_of_a_crashed_server`
    - `should_wait_for_rust_analyzer_quiescence_before_ready`
    - `should_restart_a_crashed_server_on_next_use`
    - `should_give_up_after_three_crashes_in_ten_minutes`
    - `should_shut_down_idle_servers`
    - `should_evict_the_least_recently_used_server_over_the_limit`
    - `should_open_changed_files_as_overlays_up_to_the_limit`
    - `should_restart_the_unit_when_a_change_set_exceeds_the_threshold`
- **atlas-edit:**
  - `should_refuse_a_plan_when_a_file_changed_since_planning`
  - `should_roll_back_every_file_when_a_commit_fails_midway` (fault hook: fail the Nth persist)
  - `should_refuse_paths_outside_registered_repos`
  - `should_refuse_symlinks_and_gitignored_files`
  - `should_derive_stable_occurrence_ids_from_file_hash_and_offset`
  - `should_apply_only_selected_occurrences`
  - `should_refuse_when_expected_count_differs`
  - `should_refuse_multiple_matches_without_all`
  - `should_expand_regex_capture_groups`
  - `should_keep_crlf_line_endings_and_bom`
  - `should_reindent_a_flush_left_body_to_the_symbol_indent`
  - `should_replace_leading_docs_when_the_new_body_starts_with_a_doc_comment`
  - `should_delete_a_symbol_with_its_docs_and_attributes_and_one_blank_line`
  - `should_render_hunks_with_occurrence_ids`
- **Real rust-analyzer.** Gated like `atlas-scip/tests/rust_analyzer.rs`: run if `locate_rust_analyzer()` succeeds, else `eprintln!("SKIPPED …")` and return. It is available on this machine (1.97.1).
- **Real typescript-language-server.**
  - Not on PATH and not in the npx cache here. A copy exists at `~/.serena/language_servers/static/TypeScriptLanguageServer/ts-lsp/node_modules/typescript-language-server` (5.1.3, TypeScript 5.9.3).
  - Gate: run when `ATLAS_TEST_TS_LSP` (path to `lib/cli.mjs` or a binary) is set or a server is on PATH, else SKIPPED.
  - A separate `#[ignore = "needs network: npx typescript-language-server@5"]` test covers the download path.
- **Golden rename scenarios** (`crates/atlas-ide/tests/golden_rename.rs`):
  - `should_rename_format_money_across_acme_api`: 6 sites in 3 files (definition, 2 imports, 3 calls).
  - `should_rename_users_service_find_one_including_its_spec_calls`: service, controller and spec. The spec's `service.findOne` sites are renamed.
  - `should_report_orders_service_find_one_jest_mocks_as_not_renamed`: 3 mock sites in `orders.controller.spec.ts` reported `Mock [heuristic 0.60]`. `controller.findOne('o1')` is untouched (a precision check).
  - `should_rename_store_get_and_every_impl_with_rust_analyzer` (`trait-dispatch-rust`): the trait declaration, 2 impls, and calls in `read_generic`/`read_dyn`/`read_impl`/`read_mem`.
  - `should_rename_a_ts_interface_method_and_its_implementations` (`ts-overrides`).
  - `should_report_to_cents_consumers_in_acme_web_as_not_renamed`, and with `cross_repo=apply`, `should_apply_authoritative_to_cents_sites_in_acme_web` (import binding + call in `OrderForm.tsx`).
  - `should_refuse_safe_delete_of_format_money_and_list_references`.
- **After an edit:**
  - `atlas-engine/tests/incremental_equivalence.rs::should_match_a_full_index_after_atlas_edits`: apply plans from `atlas-edit` in the seeded edit sequence.
  - `atlas-ide/tests/freshness.rs::should_reflect_a_rename_in_the_next_query`, with the watcher on and the barrier forced by `write_then_refresh`.
  - `atlas-engine::should_force_a_scan_after_write_then_refresh_even_with_a_clean_watcher`.
- **MCP:**
  - `should_list_fifteen_tools_with_schemas`
  - `should_keep_tool_schemas_within_the_budget`
  - `should_mark_atlas_edit_destructive`
  - `should_rename_through_atlas_edit_with_the_fake_server`
  - `should_return_diffs_and_a_plan_id_on_dry_run`
  - `should_apply_a_plan_id_once`
  - Rewrite `should_tell_agents_to_read_files_they_will_edit_with_their_own_tool` as `should_tell_agents_to_reread_files_after_atlas_edit`.
- **CLI:** `should_exit_5_when_an_edit_is_refused`, `should_write_a_plan_and_apply_it`, `should_read_code_from_stdin_for_similar`.

---

## 17. Evaluation

- **Quality** (`atlas-eval quality`, `crates/atlas-eval/src/quality/evaluate/rename.rs`).
  - New truth section, documented in `fixtures/expected.schema.md`. It is eval truth, so pins are unaffected:

```toml
[[rename_sites]]
id = "format-money"
target = { repo = "acme-api", path = "src/common/money.ts", symbol = "formatMoney" }
new_name = "formatCents"
edit   = [ { repo = "acme-api", path = "src/orders/orders.service.ts", line = 26 }, … ]
report = [ { repo = "acme-api", path = "src/orders/orders.controller.spec.ts", line = 5, reason = "mock" } ]
untouched = [ { repo = "acme-api", path = "src/orders/orders.controller.spec.ts", line = 12 } ]
```

  - Metrics per scenario and in aggregate:
    - rename-site recall = expected edit sites edited ÷ expected,
    - precision = edited sites that are expected ÷ edited,
    - report recall = expected report sites listed,
    - violations = `untouched` sites edited.
  - The scenarios run as dry runs. Without a server the row reads `skipped: no language server`, never zero.
- **Retrieval** (`atlas-eval retrieval`, E2).
  - A task TOML may carry `[rename] symbol = "...", new_name = "..."`.
  - Expected sites = old-side lines of the gold patch where the old identifier is replaced by the new one, reusing E2's `retrieval/patch.rs`.
  - Recall and precision of the dry-run plan against those sites.
  - CTX-1 (A5) is tracked by E2's existing reuse and must_change metrics.
- **Agent task (optional)** `t11-rename-target-filter`:
  - Rename `atlas_query::TargetFilter` to `TargetConstraint`: 45 occurrences in 15 files across atlas-query and atlas-cli.
  - It must **not** touch `TargetFilterArgs`, which is the precision trap.
  - Hidden checks: `cargo check --workspace`, no `\bTargetFilter\b` left, `TargetFilterArgs` unchanged.
  - Measures whether agents adopt `atlas_edit` and what it costs.

---

## 18. Task decomposition

Tier → model: **low** = sonnet/standard, **medium** = sonnet/high, **high** = opus/high.
Tasks in one batch touch disjoint files. MCP edits are concentrated in C1 and CLI edits in C2, so query-layer tasks never conflict on `atlas-mcp/src/*` or `atlas-cli/src/cli.rs`.

| Batch | Task | Scope (files) | Tier | Depends | Size (prod/test LOC) |
|---|---|---|---|---|---|
| 0 | **T0** scaffold | root `Cargo.toml` (members, `url`, `regex` workspace deps), stub `crates/atlas-lsp`, `atlas-edit`, `atlas-ide` (Cargo.toml, lib.rs, error.rs), `Cargo.lock` | low | — | 120 / 0 |
| 1 | **B1** LSP core | `atlas-lsp/src/{protocol,framing,client,transport,position,uri,process,error}.rs`, `src/bin/atlas-fake-lsp.rs` | high | T0 | 1,400 / 650 |
| 1 | **B5** atlas-edit | `atlas-edit/src/{plan,scope,apply,lock,text,occurrence,symbol,region,indent,diff}.rs` | high | T0 | 1,600 / 900 |
| 1 | **A1** TS overrides and heritage label | `atlas-resolve/src/output.rs` (+`ts.rs` lookup helper), resolver/detector/engine `version_pin.rs`, `fixtures/adversarial/ts-overrides/**`, `atlas-resolve/CLAUDE.md`, query test for the dependents label | high | R2 merged | 350 / 400 |
| 1 | **A4** macro hint | `atlas-query/src/target.rs`, `tests/macro_gap.rs` | low | — | 80 / 60 |
| 1 | **A8** positions | `atlas-query/src/model.rs` (`Location.column`), new `queries/positions.rs` | low | — | 200 / 150 |
| 2 | **B2** servers, pool, sync | `atlas-lsp/src/{spec,rust_analyzer,typescript,ready,pool,sync,docs}.rs` | high | B1 | 1,500 / 700 |
| 2 | **B4** engine hooks | `atlas-engine/src/{engine.rs, scip.rs, units.rs(new), analyze_text.rs(new)}`, `watch.rs` (`mark_event` visibility), engine tests | medium | — | 250 / 250 |
| 2 | **A3** mock detector | `atlas-detect/src/detectors/mocks.rs` + registry, detect pin corpus, `atlas-query/src/queries/tests_for.rs`, `text/tests_for.rs`, `fixtures/acme/expected.toml` edges | medium | A1 (pins) | 450 / 250 |
| 2 | **A2** unused honesty | `atlas-query/src/queries/unused.rs`, `text/unused.rs`, `tests/unused.rs` | medium | — | 250 / 150 |
| 2 | **B8** graph symbol surface | new `atlas-query/src/queries/{symbols.rs, hierarchy.rs}`, `text/symbols.rs`, `queries/mod.rs`, `lib.rs`; pub APIs `fresh`, `resolve_target`, `innermost_symbol`, `graph_references`, `name_span` | medium | A8 | 700 / 400 |
| 3 | **B3** settings and discovery | `atlas-project/src/registry.rs` (`LspSettings`), `atlas-scip/src/rust_analyzer.rs` (pub locator), `atlas-lsp/src/discover.rs` | medium | B2 | 400 / 250 |
| 3 | **B6** IDE reads | `atlas-ide/src/{service,target,servers,sync,map,references,declaration,outline,diagnostics,render}.rs` | high | B2, B4, B8 | 1,800 / 600 |
| 3 | **A5** context reuse (CTX-1) | `atlas-query/src/queries/{context.rs, similar.rs, reuse.rs(new)}` | medium | E2 merged | 300 / 200 |
| 4 | **B7** IDE edits | `atlas-ide/src/edit/{mod,rename,symbol_ops,text_ops,extras,plans,notify}.rs` | high | B5, B6 | 1,700 / 800 |
| 4 | **A6** similar from code | `atlas-query/src/queries/similar.rs`, `text/similar.rs` | medium | B4, A5 | 300 / 200 |
| 4 | **A7** via_http | `atlas-query/src/queries/traverse.rs`, `graph.rs`, `text/traverse.rs` | medium | — | 350 / 250 |
| 5 | **C1** MCP | `atlas-mcp/src/{arguments,descriptions,server,session,render}.rs`, `tests/protocol.rs`, `tests/common` | medium | B3, B6, B7, A2, A6, A7 | 900 / 500 |
| 5 | **C2** CLI | `atlas-cli/src/{cli.rs, commands/{ide,edit,query,project,doctor}.rs, error.rs}`, `tests/cli.rs`, new `tests/e2e_edit.rs` | medium | B3, B6, B7, A6, A7 | 900 / 500 |
| 6 | **D1** golden and integration | `atlas-ide/tests/{golden_rename,freshness,lsp_real}.rs`, `atlas-engine/tests/incremental_equivalence.rs`, ts-overrides `tsconfig` if needed | high | C1, C2 | 0 / 1,200 |
| 6 | **D2** evaluation | `atlas-eval/src/quality/evaluate/rename.rs`, `quality/truth.rs`, `retrieval/score.rs`, `[[rename_sites]]` in fixture truth, `fixtures/expected.schema.md`, optional `eval/tasks/t11-rename-target-filter` | medium | B7, E2 merged | 700 / 300 |
| 7 | **D3** docs and contracts | `CLAUDE.md`, new `crates/{atlas-lsp,atlas-edit,atlas-ide}/CLAUDE.md`, `docs/{architecture,agent-integration,trust-and-limitations,operations,knowledge-model}.md`, `connect` guidance block in `atlas-cli/src/commands/connect.rs` | medium | all | docs about 600 lines |

**Total:** 22 tasks in 8 batches (0–7); about 15k production lines and about 9k test lines. The critical path is T0 → B1 → B2 → B6 → B7 → C1/C2 → D1.

Serial constraints:
- A1 → A3 (both re-pin).
- A5 → A6 (`similar.rs`).
- Everything touching `atlas-mcp` is in C1 and everything touching `atlas-cli/src/cli.rs` is in C2.
- R2 must merge before A1 (both edit `atlas-resolve/src/output.rs`).

---

## 19. Risks and open questions

**Questions for you:**
- **Q1.** Should `lsp.enabled` default to on? It spawns rust-analyzer, which runs build scripts and proc macros, whenever an agent asks for an exact operation. The proposal is on, with downloads off. Auto-SCIP is off by default; the difference is that these operations are explicitly requested.
- **Q2.** Is it acceptable to fold `atlas_read` into `atlas_symbol` and `atlas_changes` into `atlas_status`? This is a breaking change to tool names, eval preambles, `connect` guidance and reports. Should `atlas_callees` and `atlas_explain` also be folded to reach about +8% schema tokens?
- **Q3.** Should the post-edit flycheck (cargo check) stay off in `atlas_edit` answers, with native diagnostics only? Full flycheck can take tens of seconds per edit.
- **Q4.** Is one server per project (cross-repo Rust path dependencies renamed exactly, more memory per process) the right default over one per repo?
- **Q5.** rust-analyzer may create or update `Cargo.lock`. Should we warn only (proposed), or refuse to start the server when the lock is missing or stale?
- **Q6.** Should warm servers be shared across processes (a `code-atlas lsp daemon` or the `watch` process)? Not in scope; it would fix N × 3 GB with several agent sessions.
- **Q7.** Is `cross_repo: "apply"` limited to authoritative graph sites acceptable, or should cross-repo sites never be editable in v1?
- **Q8.** Should a TypeScript 7 native LSP backend (`tsgo --lsp`) be added later as the TS server once TS 7 is mainstream?

**Risks:**
- Agents may ignore `atlas_edit` like the other tools. Instruction item 4 and the rename agent task measure this.
- The harness's read-before-edit state goes stale after `atlas_edit`. The tool description tells agents to re-read; the native Edit tool fails loudly otherwise.
- rust-analyzer readiness on very large workspaces can exceed the 300 s ready timeout; the error says so and suggests `lsp check`.
- tsserver without `node_modules` misses cross-package links. The TS2307 caveat (§10) and the graph extras (§7.3) surface this.
- WorkspaceEdits that touch excluded files (a `.d.ts` in `dist`, a vendored copy) refuse the whole rename. This is correct but may frustrate; the refusal lists the files.
- A TOCTOU window against external editors between the last hash check and the rename (§11).
- Memory: one rust-analyzer plus one tsserver can reach about 4.5 GB per MCP process, on top of the engine's own footprint (about 4.2 GiB at 1M LOC).
- There is no Windows support, matching SCIP and `watch`.

---

### Critical files for implementation
- /Users/leander-paul/repositories/shadow-library/code-atlas/crates/atlas-engine/src/engine.rs
- /Users/leander-paul/repositories/shadow-library/code-atlas/crates/atlas-query/src/target.rs
- /Users/leander-paul/repositories/shadow-library/code-atlas/crates/atlas-mcp/src/server.rs
- /Users/leander-paul/repositories/shadow-library/code-atlas/crates/atlas-resolve/src/output.rs
- /Users/leander-paul/repositories/shadow-library/code-atlas/crates/atlas-scip/src/process.rs
