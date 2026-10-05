# atlas-lsp

Blocking stdio JSON-RPC client for language servers, hand-written lenient LSP types, positions, file URIs and server processes. Depends on `atlas-core` only.

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

## Testing

- `atlas-fake-lsp` (a real `[[bin]]`, `CARGO_BIN_EXE_atlas-fake-lsp`) replays a JSON-lines script: `recv`, `recv_all`, `reply`, `recv_reply`, `notify`, `request`, `canned`, `sleep_ms`, `stderr`, `exit`, `raw`, `raw_bytes`, `sleeper`, `hang`. A script mismatch exits 97 with the reason on stderr.
- The real rust-analyzer test runs when rust-analyzer is found on PATH or through `rustup which`, and prints `SKIPPED` otherwise.
