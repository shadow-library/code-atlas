# atlas-store contracts

- Edge targets may be dangling; the engine re-resolves via `owners_referencing` and drops dangling detector edges.
- Run lifetime: `begin_run` and `finish_run` go in their own write transactions, and every write transaction in between calls `resume_run(id)`, because the active run belongs to one `WriteTx`. `begin_run` fails with `RunAlreadyActive` when a run is already active in that transaction, and so does `resume_run` with a different id.
- `replace_owner` / `remove_owner` run inside a savepoint: on error they leave no partial state, even when the caller swallows the error and commits.
- Re-entering the store from a thread that already holds one of its transactions fails with `StoreError::Reentrant`; use the `ReadTx`/`WriteTx` already in hand. A `read` inside a file-backed `write` is allowed and sees the pre-write snapshot.
