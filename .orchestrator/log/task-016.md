# Task 016 — Indexer orchestrator

**Status:** done
**Sub-agent:** general-purpose (agentId a314ef2a2950ff612)

## Files changed
- src/code_atlas/indexing/edge_extractor.py (new)
- src/code_atlas/indexing/indexer.py (new)
- src/code_atlas/indexing/__init__.py (modified — re-exports `EmbedFunc`, `Indexer`, `IndexResult`)
- tests/integration/indexing/test_indexer.py (new)

## Sub-agent response (verbatim)

### Summary
Added `Indexer` orchestrator with batched embedding, content-hash idempotency via metadata pre-check, and per-file Python symbol-edge extraction (defines + contained_in) derived from `CodeChunk` metadata alone. New `edge_extractor.extract_python_edges` plus integration tests covering 4-store fan-out, no-op re-runs, selective re-embedding, batching, edge extraction, and dim/count error paths.

### State update
- `Indexer` sync, batched (default 64), idempotent via `metadata.get_many` content_hash pre-check before embed.
- Per-batch flush writes only `to_index` to all three persistence stores; symbol_graph built once at end from `per_file` accumulator.
- `extract_python_edges` derives `defines` (module→top-level class/function) + `contained_in` (class→method, line-range nest) — no tree-sitter; pure metadata.
- Memory note: full-run `per_file: dict[str, list[CodeChunk]]` buffer; acceptable v1, may need streaming on huge repos.
- Indexer never closes stores — caller-owned lifecycle.
- Embed failures, dim mismatch, count mismatch all wrap as `IndexingError` with context.

### Next task
Task 017: write async wrapper / adapter so async embedders (Ollama, OpenAI) can be plugged into the sync `EmbedFunc` boundary without blocking the event loop — referenced as Task 020 in arch excerpt.

## Apply notes

- Diff was clean (no HTML entities to decode). Applied via Write for new files, Edit for `__init__.py`.
- Two post-write lint fixes:
  1. **SIM102** in `edge_extractor.py`: nested `if cls_chunk.start_line <= ... : if enclosing is None or ...:`. Collapsed into single `if A and B and (C or D):` per ruff guidance. Cleaner.
  2. **I001** in `test_indexer.py`: ruff isort wanted single blank line (not double) between import block and module-level constants. Auto-fixed via `ruff check --fix`.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 35 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (22 source files, strict)
  - `uv run pytest tests/ -q` → 172 passed (165 prior + 7 new)
- Sub-agent's "Next task" suggestion drifts: it conflates Task 017 (Provider Protocols + registry) with an async adapter (which doesn't have a dedicated task yet — was hand-waved as "caller-side shim" in Task 016's brief). Not adopting verbatim. The actual next task in TASKS.md is Task 017 (Provider Protocols + registry), which lays the seam that Task 018 (Ollama embeddings) plugs into. The async-to-sync adapter is a 3-line shim, not a task on its own.
- One quirk in the integration test: it reaches into `graph._g` (private attr) to inspect edges by kind, since `SymbolGraph` exposes only `callers`/`callees` (calls only). For an integration test inspecting graph internals across edge kinds, this is acceptable; a follow-up could add a public `edges(kind=None)` accessor if more code needs this.
