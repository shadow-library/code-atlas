# Task 020 — Hybrid retrieval (vector + lexical, RRF)

**Status:** done
**Sub-agent:** general-purpose (agentId a428252894092115b)

## Files changed
- src/code_atlas/retrieval/__init__.py (new)
- src/code_atlas/retrieval/hybrid.py (new)
- tests/unit/retrieval/test_hybrid.py (new)
- CLAUDE.md (added gotcha: SQLite :memory: + asyncio.to_thread)

## Sub-agent response (verbatim)

### Summary
HybridRetriever. Parallel vec+lex via gather. Embed gates vec path. RRF k=60 fuse rank pos (ignore raw scores). Oversample k*2 pull bigger pool. Hydrate via metadata_store.get_many, drop missing. Filter extract repo_id only, bad type raise RetrievalError, unknown keys log+ignore. Errors from embedder/stores propagate as-is. 11 tests with fake stores + real in-mem MetadataStore.

### State update
- Added `code_atlas.retrieval` package; entrypoint `HybridRetriever` plus public constant `RRF_K_DEFAULT=60`.
- Hybrid pipeline = embed -> gather(vector_search, lexical_search) -> RRF fuse (k=60 default, rank-only) -> metadata hydrate -> top-k of `source="fused"`.
- Oversample knob (default 2) widens candidate pool: stores called with `k = query.k * oversample`.
- Filter contract v1: only `repo_id` (str) is honored; unknown keys debug-logged and ignored; non-str `repo_id` -> `RetrievalError(context={"field": "repo_id", "got_type": ...})`.
- Error propagation: embedder errors (ProviderError) and store errors (IndexingError) bubble AS-IS so callers can distinguish failure source; only filter validation raises `RetrievalError` directly.
- Hydration drops chunk_ids missing from `MetadataStore` (with warning log); rank order preserved.

### Next task
Task 021: Reranker (cross-encoder rerank stage that consumes list[RetrievalResult] from HybridRetriever).

## Apply notes

- Diff was mostly clean. Two issues fixed inline:
  1. **Sub-agent shipped `tests/unit/retrieval/__init__.py`**. Project convention has NO `__init__.py` in `tests/` (pytest discovers via `pythonpath` + `rootdir`, verified with `find tests -name __init__.py` returning nothing). **Skipped that file** during apply.
  2. **Fixture used `sqlite:///:memory:`** for MetadataStore. SQLite `:memory:` is per-connection — when `asyncio.to_thread` runs `get_many` on a worker thread, that thread sees an empty DB and fails with `OperationalError: no such table: chunks`. **All 10 hybrid retrieve tests failed**. Fix: switched fixture to `sqlite:///{tmp_path}/meta.sqlite` (file-backed; tmp_path is auto-cleaned by pytest). Documented in CLAUDE.md as a carry-forward gotcha.
- Also post-write: **RUF022** flagged `__all__` ordering in both `__init__.py` and `hybrid.py`. Auto-fixed via `ruff check --fix` — ruff's RUF022 uses case-sensitive natural sort, which puts all-caps `RRF_K_DEFAULT` before mixed-case `HybridRetriever` (despite ASCII H < R, the rule treats "all-caps" group as a sort class).
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 46 files already formatted
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (29 source files, strict)
  - `uv run pytest tests/ -q` → 220 passed (208 prior + 12 new — sub-agent said 11; actual was 12)
- Sub-agent's "Next task" suggestion (`Reranker`) aligns with TASKS.md Task 021 (Citation hydration + Reranker Protocol). The framing differs slightly: Task 021 ships both `Citation` hydration helper AND a `Reranker` Protocol with passthrough default. Cross-encoder rerank is post-v1 work.
