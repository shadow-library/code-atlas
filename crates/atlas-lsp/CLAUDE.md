# atlas-lsp

Blocking stdio JSON-RPC client for language servers, hand-written lenient LSP types, positions, file URIs, server processes, server specs, the server pool and document sync. Depends on `atlas-core` only: it never discovers projects or reads the engine.

## Positions

- Byte offsets are the only common ground between a language server and the graph. Every LSP position is converted to a byte offset with `LineIndex` over the exact text the server saw, and every displayed or stored position is derived from bytes via `LineIndex::graph_span` / `LineIndex::span`, which equal `Span::of_text` (1-based lines counted on `\n`, 0-based byte columns). Never compare LSP line numbers with graph line numbers.
- `LineIndex` takes the server's line-break convention: `LineBreaks::Lf` for rust-analyzer (CRLF is one break, a lone `\r` is a character), `LineBreaks::Ecma` for tsserver (`\n`, `\r\n`, `\r`, U+2028, U+2029).
- Reads use `offset`/`offsets`, which clamp a character past the line end as the spec says. Edits use `offset_strict`/`offsets_strict`, which refuse it, so a stale position is never applied.
- The encoding is negotiated at `initialize` (`negotiate_encoding`): no answer means utf-16; an encoding the client did not offer is accepted with a warning; an unknown one is an error.

## Protocol types

- Leniency is limited to genuine server quirks: `null` arrays, capability shapes, unknown progress values and status strings.
- Anything that becomes an edit or a reported fact is strict: `range`, `newText`, `uri`, `start`/`end`, `line`/`character`, symbol ranges and diagnostic ranges are required. Snippet text edits, unknown text-edit fields and unknown document-change kinds are errors, never dropped or defaulted.
- `WorkspaceEdit::requires_confirmation` is true when any edit or resource operation carries an annotation that needs confirmation or is undeclared; such edits must never be applied automatically.

## Client

- One reader thread, one writer thread fed by a bounded outbox (64 frames), and one responder thread for server requests. A request's timeout covers enqueueing and waiting; on expiry it sends `$/cancelRequest` and returns `Timeout`.
- `shutdown()` never waits on the writer: `shutdown` and `exit` are enqueued within `shutdown_timeout`, then the process gets `exit_grace` before its group is killed, which also unblocks a stuck write with EPIPE.
- A disconnect fails every pending request with one error carrying the stderr tail (8 KiB). Framing errors are `Protocol` and kill the group; end of stream, process exit and write failures are `ServerExited`.
- `ClientHandler::on_event` runs on the reader thread under the event gate: it must only record state and notify waiters. Exactly one `Disconnected` is delivered, and nothing after it.
- `workspace/applyEdit` is always refused: servers never write, all writes go through `atlas-edit`.

## Processes

- Servers run in their own process group. The exit watcher kills the group right after reaping the leader (stragglers such as `proc-macro-srv` keep the pgid valid), and `Drop` kills the group only while no exit is recorded, so a reused pgid is never signalled.

## Units and specs

- A `ServerUnit` is plain data: project, folders, `UnitSettings`, a ready `ServerCommand`, `Launch` and `Timeouts`. atlas-ide builds it from `Engine::language_units()`.
- rust-analyzer settings: `linkedProjects`, a cargo target dir that must be absolute and outside every folder, check-on-save off by default. They are sent as initialization options and as the `rust-analyzer` section of `workspace/configuration`.
- TypeScript settings: `tsserver.path`, `maxTsServerMemory`, rename preferences and `tsserver.useSyntaxServer = "never"`. With "auto", a partial-semantic syntax server answers while the project loads, and references then cover only open files.
- `LineBreaks` per kind is binding (`Lf` rust-analyzer, `Ecma` TypeScript); use `Lease::line_breaks()`.
- `Launch::Download` (an npx-resolved command) raises the initialize timeout to 300 s to cover the first download.

## Readiness

- rust-analyzer is ready when `experimental/serverStatus` is quiescent, health is not `error`, and no `$/progress` token is active apart from flycheck and cache priming. A quiescent `error` fails fast (`Unhealthy`). After 300 s, `NotReady` reports the last status and says how to check.
- TypeScript is ready once `$/typescriptVersion` arrives (or `Timeouts::announcement` after initialize) and no progress is active. The first request gets `first_request` (120 s), because tsserver loads the project then.
- The tracker (`ClientHandler`) only records state and notifies its condvar. It never takes the pool or sync lock, and the pool never holds its lock while shutting down or dropping a server.

## Pool

- One server per `UnitKey` (project, kind, scope). `UnitScope::Repo` units (for `lsp.unit = "repo"`) never replace each other. `get` is single-flight per unit, and waiters share the start result, including an error. A pending restart cause is kept per unit, so every caller sees it.
- On the next `get`, a server is replaced if it exited, if its sync was lost, or if the unit configuration changed (command, sorted folders, settings; timeouts excluded).
- Three failures within 10 min put the unit in backoff for 10 min (`CrashLoop`). Failures are crashes and failed starts; spawn and invalid-unit errors do not count.
- `max_servers` (2) is a hard cap over starting, warming, live and retiring servers. Over the cap, `get` evicts the least recently used live server without a `Lease` (then a warming one), or waits on the pool condvar for a lease to drop, for up to `busy_timeout` (30 s) counted from when that wait starts, then fails with `PoolBusy`. Leases are counted under the pool lock, so a server is never evicted or reaped between `plan` and the caller.
- `PoolMode::Warm` reaps servers idle for 10 min every 30 s. A server that misses the ready timeout stays in a `Warming` slot: the call fails with `NotReady`, and the next `get` waits for the same process instead of respawning it. A server still warming after 3× the ready timeout is recycled and counted as a failure (`RestartReason::NeverReady`), so `CrashLoop` eventually stops it. There is no prewarm.
- `PoolMode::OneShot` (CLI) has no reaper, and a missed ready timeout shuts the server down. A command spawns, waits for readiness, answers and shuts down with the pool. Expected times: 2–5 s for TS fixtures, 5–15 s for rust-analyzer on fixtures, and 30–90 s cold on this workspace (10–25 s once build scripts are cached in the LSP target dir). One-shot mode is for scripts and CI, not interactive loops.
- `shutdown()` also kills servers that are still starting, so their callers return `PoolShutDown` promptly.
- Memory: rust-analyzer takes 2–3 GB RSS on large workspaces, tsserver 0.3–1.5 GB (capped by `maxTsServerMemory`). Each MCP process owns its servers, so N sessions cost N times that (no shared daemon). `resident_kib` sums RSS over the server's process group (tsserver children, proc-macro-srv).
- `Lease::lockfile_changes()` lists `Cargo.lock` files rewritten since rust-analyzer started. Callers warn; nothing restores them.

## Document sync

- `notify` returning Ok means enqueued, not delivered. The outbox is FIFO, so any later response proves earlier notifications were delivered, and a request timeout is not a sync loss. Only a notification that cannot be enqueued loses sync; the server is then restarted on the next `get`.
- `LspPool::sync(unit, generation, demand, changes_since, open)` asks for the change set since the server's generation.
  - The generation is compared and set under the sync lock. A server already at or past `generation` gets no changes and never moves back; a change set computed from a generation that another sync has since moved is recomputed. Each retry is closer to the target, so contention never restarts the server.
  - Unknown changes, or more than 1000 changed files inside the unit's folders, restart the server.
  - `open` paths are synced first and their drift against the change set is reported. Up to 64 changed files of the server's language become full-text overlays (`didOpen`/`didChange`; versions come from one counter per server). Above 64, only documents that are already open are refreshed.
  - Every change is also sent in one `didChangeWatchedFiles` batch.
- Each sync has an overlay byte budget (8 MiB; 64 MiB for `Exact`), because the outbox bounds frames, not bytes. A document over the budget, not UTF-8 or unreadable is closed if open (a stale overlay would mask the disk) and announced as changed.
- Open documents whose `(len, mtime)` changed outside the change set are refreshed. Beyond 64 documents or 32 MiB, the least recently used are closed, never the requested ones.
- `SyncReport.drifted` lists files whose disk hash differs from the change set; the disk text is sent. `Settle` states what is known:
  - rust-analyzer waits for quiescence (≤ 30 s): `Ready`, `Quiet` or `Busy`. The 500 ms window for the server to react is skipped when every announced file was overlaid and none is `Cargo.toml`, `Cargo.lock`, `build.rs` or `.cargo/config*`.
  - typescript-language-server 5.x ignores `didChangeWatchedFiles`; tsserver relies on its own watchers. Announced files of the server's language and `tsconfig*.json`/`jsconfig*.json`/`package.json` that were not overlaid stay in a per-server unconfirmed set until they are overlaid or the server restarts, and every sync reports them as `Unconfirmed { paths }`. rust-analyzer never fills this set.
  - A deleted TypeScript file is confirmed by `didOpen` (empty text) then `didClose`: tsserver's `closeOpenFile` sees the file is gone and drops the script at once (`handleDeletedFile`). Project configuration changes stay unconfirmed; atlas-ide restarts the TS server on a sticky `Unconfirmed` when it needs exact answers.
- `SyncDemand::Exact` is for rename and safe_delete (atlas-ide must use it for both). Every changed file of the server's language is overlaid regardless of the 64-file limit.
  - TypeScript: if anything stays unconfirmed (project configuration, files over the budget), the server is restarted and re-synced (`RestartReason::Unconfirmed`).
  - rust-analyzer: the sync fails with `SyncIncomplete` when the server is not quiescent afterwards or a changed `.rs` file could not be overlaid.

## Testing

- `atlas-fake-lsp` (a real `[[bin]]`, `CARGO_BIN_EXE_atlas-fake-lsp`) replays a JSON-lines script: `recv`, `recv_all`, `reply`, `recv_reply`, `notify`, `request`, `canned`, `sleep_ms`, `stderr`, `exit`, `raw`, `raw_bytes`, `sleeper`, `hang`, `record` (append every later client message as a JSON line to a file). A script mismatch exits 97 with the reason on stderr.
- The real rust-analyzer tests run when rust-analyzer is found on PATH or through `rustup which`, and print `SKIPPED` otherwise.
- The real typescript-language-server test runs with `ATLAS_TEST_TS_LSP` (a binary or `lib/cli.mjs`), a server on PATH, or one cached under `~/.npm/_npx`. It never downloads, and prints `SKIPPED` otherwise.
