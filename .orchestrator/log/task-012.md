# Task 012 — Metadata store (SQLite + SQLAlchemy Core)

**Status:** done
**Sub-agent:** general-purpose (agentId acdbafe2de355ac58)

## Files changed
- pyproject.toml (modified — added sqlalchemy)
- src/code_atlas/indexing/__init__.py (new)
- src/code_atlas/indexing/metadata_store.py (new)
- tests/unit/indexing/test_metadata_store.py (new)
- uv.lock (regenerated)

## Sub-agent response (verbatim)

### Summary
Implement Task 012: SQLite-backed `MetadataStore` for canonical `CodeChunk` rows, kicking off Phase 3 (indexing). Adds the `code_atlas.indexing` subpackage with a SQLAlchemy Core-only store supporting upsert (single + batched), order-preserving multi-get, repo-scoped delete, count, and context-manager lifecycle. Wraps all SQLAlchemy failures as `IndexingError`. Adds 12 unit tests covering round-trip, idempotency, update, batch insert, order preservation, repo isolation, and on-disk persistence.

### State update
Phase 3 kick. New pkg `code_atlas.indexing`. `MetadataStore` SQLite + SQLAlchemy Core, no ORM. Table `chunks` PK `chunk_id`, idx `repo_id`+`content_hash`, `indexed_at` ISO UTC. Ops: upsert, upsert_many, get, get_many (order-preserve, skip miss), delete_repo, count(repo_id?), close, ctx-mgr. SQLAlchemyError → IndexingError. Dep `sqlalchemy>=2.0,<3.0` added.

### Next task
Task 013: lexical store (likely SQLite FTS5 or Tantivy).

## Apply notes

- HTML-entity decoding across `->`, `<`, `>=`.
- Two post-write fixes:
  1. Ruff UP017: replaced `from datetime import datetime, timezone` + `datetime.now(timezone.utc)` with `from datetime import UTC, datetime` + `datetime.now(UTC)` (Python 3.11+ preferred form).
  2. mypy strict: SQLAlchemy 2.0's `RowMapping` is not assignable to `Mapping[str, Any]` (key type is wider). Wrapped `row` with `dict(row)` at call sites in `get()` and `get_many()` to coerce to a plain dict. The helper `_row_to_chunk(row: Mapping[str, Any])` signature stayed intact.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → OK (26 files)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (17 files, strict)
  - `uv run pytest tests/ -q` → 118 passed (106 prior + 12 new)
- Sub-agent's "Next task" matches the planned task 013 (lexical store via SQLite FTS5).
