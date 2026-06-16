# Task 011 — Ingestion pipeline

**Status:** done
**Sub-agent:** general-purpose (agentId a6ed777e401fc6052)

## Files changed
- src/code_atlas/ingestion/__init__.py (modified — re-export IngestStats + ingest_repo)
- src/code_atlas/ingestion/pipeline.py (new)
- tests/integration/ingestion/test_pipeline.py (new — first integration test)

## Sub-agent response (verbatim)

### Summary
Implemented `ingest_repo` in `src/code_atlas/ingestion/pipeline.py`: composes `walk_repo` → `detect_language` → `chunk_file` into a lazy `Iterator[CodeChunk]`. Eager validation of `root` and `repo_id` happens in the outer function; the inner `_iter` generator performs the per-file work. Introduces `IngestStats` dataclass (slots) for caller-mutable counters and supports optional `mtime_cache: dict[str, tuple[float, int]]` for incremental re-ingestion (cache updated only after successful processing). Updated the package `__init__.py` to re-export `IngestStats` and `ingest_repo`. Added integration tests covering chunk emission, repo_id propagation, relative paths, stats counters, mtime-cache skip/refresh behavior, eager error raising, and lazy iteration.

### State update
Phase 2 done. Pipeline live: walker → detect → chunk. IngestStats counters. mtime cache: stamp = (mtime,size); update only after chunk success. Validate eager (bad root + empty repo_id raise sans iter); emit lazy via _iter gen.

### Next task
Phase 3 kickoff: persistence layer. metadata + lexical writers consuming `Iterator[CodeChunk]`; defer vector embeddings until provider config is wired.

## Apply notes

- HTML-entity decoding across `->`, `<`, `>=`.
- Post-write fix: `test_stats_counters` expected `files_skipped_no_language == 2` (notes.txt + README.md), but `.gitignore` is also a text file with no extension/shebang → counts as no-language. Updated to `== 3` with explanatory inline comment.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → OK (23 files)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (15 files, strict)
  - `uv run pytest tests/ -q` → 106 passed (97 prior + 9 new)
- Phase 2 (Ingestion) complete. Next phase: persistence/indexing layer.
- Sub-agent's "Next task" suggested deferring vector embeddings until providers are wired. Per TASKS.md plan, the order is: metadata store (012) → lexical (013) → vector + Protocol (014) → symbol graph (015) → indexer (016). Providers come in Phase 4 (017–019). So metadata + lexical land before any vector code, which matches the suggestion.
