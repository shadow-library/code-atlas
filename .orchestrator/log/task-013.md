# Task 013 — Lexical store (SQLite FTS5)

**Status:** done
**Sub-agent:** general-purpose (agentId ae34022bb779e0493)

## Files changed
- src/code_atlas/indexing/__init__.py (modified — re-export LexicalStore)
- src/code_atlas/indexing/lexical_store.py (new)
- tests/unit/indexing/test_lexical_store.py (new)

## Sub-agent response (verbatim)

### Summary
Add `LexicalStore` backed by SQLite FTS5 with BM25 ranking, idempotent upserts via delete+insert, repo-scoped search/delete/count, and IndexingError wrapping. Export from indexing package and ship 13 unit tests.

### State update
- LexicalStore live: FTS5 over (content, symbol) with repo_id/chunk_id UNINDEXED columns.
- Uses stdlib sqlite3, NOT SQLAlchemy. `url` param is raw filename or `:memory:` — diverges from MetadataStore URL form.
- Score convention: `-bm25(...)` returned so higher = better; ORDER BY raw bm25 ASC under the hood.
- Idempotency via DELETE+INSERT per chunk_id (no PK on FTS5 virtual tables).
- None symbol coerced to "" — FTS5 columns reject NULL.
- Empty query treated as malformed by FTS5 itself; `OperationalError` caught and wrapped as IndexingError.

### Next task
Implement Task 014: vector_store.py (LanceDB) — third pillar of the chunk lookup triad alongside metadata + lexical stores.

## Apply notes

- Diff was clean (no HTML entities to decode).
- Applied via Write/Edit (new files + small __init__.py edit) — git apply not needed.
- Test count: sub-agent claimed 13 tests in Summary but diff contains 15 (test_search_returns_empty_list_for_no_matches and test_invalid_k_raises_indexing_error added beyond the "13" count). Acceptance bumped accordingly.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 28 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (18 files, strict)
  - `uv run pytest tests/ -q` → 133 passed (118 prior + 15 new)
- Minor stylistic redundancy left in place: search() originally had two except branches (OperationalError and Error) with identical bodies; collapsed to one `except sqlite3.Error` since OperationalError is a subclass. Same end behavior, less noise.
- Sub-agent's "Next task" matches the planned Task 014 (LanceDB vector store).
