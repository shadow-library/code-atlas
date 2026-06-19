# State

> Caveman-style fact sheet. Bullets, not prose. Append per-task sections under
> "Capabilities" as tasks complete. Compact when this file exceeds ~300 lines.

## Stack
- Python 3.11+. uv-managed. src/ layout.
- Lint+format: ruff. Type: mypy --strict. Tests: pytest.
- CLI: typer. API: FastAPI + uvicorn. Config: pydantic-settings + YAML.
- Logging: structlog (JSON in prod, console in dev).
- Vector: LanceDB embedded. Lexical: SQLite FTS5. Metadata: SQLite (SQLAlchemy Core).
- Parsing: tree-sitter + tree-sitter-language-pack.
- Providers: Protocols. Default = Ollama (chat + embeddings). httpx async.
- Docker: python:3.12-slim multi-stage, non-root.

## Conventions
- Errors: typed in code_atlas.errors. Carry context dict. No bare Exception.
- Logging: structlog per-module logger. Never print. Levels: debug/info/warning/error.
- Config: single Settings via pydantic-settings. yaml → .env → env (env wins).
  Injected; no module-global env reads.
- Naming: snake_case files/funcs, PascalCase classes, UPPER_SNAKE constants.
- Tests mirror src tree. Markers: slow, network, requires_ollama.
- Types: `from __future__ import annotations`. Protocol over ABC for seams.
- Async: I/O async-first; CPU sync via asyncio.to_thread.
- Commits: Conventional Commits, no AI co-authors, small + atomic at green.
- Line width 120. Indent: 4 spaces Python, 2 spaces yaml/json/md.
- Layering: domain → indexing/providers → retrieval → agent → cli/api.

## Capabilities (by task)

### Task 020 — Hybrid retrieval (RRF) — Phase 5 starts
- New pkg `code_atlas.retrieval`. Re-exports `HybridRetriever` + `RRF_K_DEFAULT` (=60).
- `HybridRetriever(*, vector_store, lexical_store, embedder, metadata_store, rrf_k=60, oversample=2)`. All store deps are SYNC; embedder is ASYNC. Sync calls wrapped in `asyncio.to_thread` from inside the async `retrieve`.
- `async retrieve(query: RetrievalQuery) -> list[RetrievalResult]`. Pipeline:
  1. Embed `[query.text]` (async).
  2. `asyncio.gather` the vector-search and lexical-search paths. Stores are called with `k = query.k * oversample` (default 2x) for a wider fusion pool.
  3. **RRF fusion** (k=60): score per chunk_id = `Σ 1 / (rrf_k + rank + 1)` across the two ranked lists. Rank is 0-indexed in code, +1 normalization for the 1-indexed RRF convention. Underlying scores are **discarded** — only rank position counts.
  4. Take top `query.k` (chunk_id, score) pairs, hydrate via `metadata_store.get_many`, emit `RetrievalResult(chunk=..., score=fused_score, source="fused")`.
- **Concurrency**: `_vector_path` chains `embed → vector.search` sequentially, but `asyncio.gather(_vector_path(), _lexical_path())` runs lexical concurrently with that chain. Tested at the surface (gather invocation, embedder call count) — not wall-clock.
- **Filter contract v1**: ONLY `repo_id` (str) is honored. Unknown keys (`language`, `kind`, etc.) silently ignored with a `hybrid.unknown_filters` debug log — forward-friendly for future expansion. Non-str `repo_id` → `RetrievalError(context={"field": "repo_id", "got_type": ...})`.
- Hydration drops chunk_ids missing from metadata store (with `hybrid.missing_metadata` warning log). Rank order preserved.
- **Error propagation policy**: embedder failures (`ProviderError`) and store failures (`IndexingError`) bubble AS-IS — callers can distinguish failure source. Only filter-validation issues raise `RetrievalError` directly. Empty fused list returns `[]` (no error).
- Ctor validation: `rrf_k < 1` or `oversample < 1` → `RetrievalError`.
- 12 unit tests in `tests/unit/retrieval/test_hybrid.py` using `FakeVectorStore`/`FakeLexicalStore`/`FakeEmbedder` + real `MetadataStore`. Coverage: deterministic RRF order (vec A>B>C, lex B>D>A → B>A>D>C), repo_id passthrough to both stores, no-filter → None to both, unknown filter keys ignored, bad repo_id type raises, top-k limit, dedup by chunk_id with summed score, hydration skip on missing, `source="fused"`, oversample multiplies k, embedder called once with `[query.text]`, both-empty returns `[]`.
- **Test gotcha discovered**: `MetadataStore("sqlite:///:memory:")` is NOT shareable across threads. `asyncio.to_thread` schedules `get_many` on a worker, and SQLite `:memory:` is per-connection, so the worker sees an empty DB ("no such table: chunks"). **Fix**: use `sqlite:///{tmp_path}/meta.sqlite` (file-backed) in tests that exercise async-wrapped store calls. Alternative would be `poolclass=StaticPool + check_same_thread=False`, but file-based is simpler. **Carry forward**: any future test that calls a SQLAlchemy store via `asyncio.to_thread` must use a file URL, NOT `:memory:`.
- Two post-write fixes:
  1. **RUF022**: ruff isort wants `["RRF_K_DEFAULT", "HybridRetriever"]` (R before H — appears to use a case-sensitive natural sort where all-caps tokens sort before mixed-case). Auto-fixed.
  2. **Test fixture URL**: switched `:memory:` → file-backed (see above).
- Did NOT ship `tests/unit/retrieval/__init__.py` despite sub-agent including it — project convention is no `__init__.py` in tests/ (pytest discovers via `pythonpath` + `rootdir`).

### Task 019 — Ollama LLM provider — Phase 4 complete
- New module `code_atlas.providers.ollama_llm` (re-exports `OllamaLLMProvider` from `code_atlas.providers`). Importing triggers `register_llm("ollama", _factory)` side-effect at module bottom.
- `OllamaLLMProvider(*, base_url, model, temperature=0.2, max_tokens=2048, timeout_s=60.0, client=None)`. Implements `LLMProvider` Protocol structurally.
- Endpoint: `POST {base_url}/api/chat`. Same `client` ownership pattern as Task 018 (`_owns_client` flag; injected clients survive `aclose()`).
- **Request payload (`_build_payload`)**:
  - `messages` → `[{"role", "content"}]`; `name`/`tool_call_id` included only when not None.
  - `tools` (if non-empty) → `[{"type": "function", "function": {"name", "description", "parameters"}}]`. Omitted entirely when no tools passed.
  - `options: {"temperature": ..., "num_predict": max_tokens}` always present.
  - `stream: true|false` set per method.
- **Non-streamed response → `ChatResponse`**:
  - `content` = `payload["message"]["content"]` (defensive `or ""` for None).
  - `tool_calls`: list comprehension over `message.tool_calls`, IDs synthesized as `f"call_{idx}"` (Ollama doesn't emit IDs). `arguments=dict(fn.get("arguments") or {})`.
  - `usage` = `TokenUsage(prompt=prompt_eval_count, completion=eval_count)`; total auto-fills.
  - `finish_reason` = `done_reason` (may be None on older Ollama).
- **Streaming via `httpx.AsyncClient.stream("POST", ...)`** as async context manager. Iterate `resp.aiter_lines()`, skip blank lines, JSON-decode each. Helper `_emit_line(chunk_data) -> Iterator[ChatChunk]` handles all four streaming cases: content-only intermediate, tool-call-only intermediate, content+tools combo, and final `done: true` line (folds last tool call into terminal chunk, emits preceding tool calls as their own chunks first).
- Async-generator `chat_stream` uses outer `try/except` wrapping the `async with stream(...)` block — captures both pre-iteration `raise_for_status()` failures AND mid-stream `RequestError` disconnects. Wraps to `ProviderError` consistently.
- Validation in `__init__`: empty `model`, `temperature` outside `[0, 2]`, `max_tokens < 1` → `ProviderError`. Empty `messages` in `chat`/`chat_stream` → `ProviderError`.
- Error wrapping mirrors Task 018: `HTTPStatusError` → `{status_code, url, body[:200]}`, `RequestError` → `{error_type, url}`, JSON decode → `{status_code}` for non-stream, `{line}` for stream, malformed payload (missing `message` key) → `{keys}`.
- 14 unit tests in `tests/unit/providers/test_ollama_llm.py`: plain chat content+usage+finish_reason+model passthrough, tool-call extraction (`call_0` id), payload tools-list shape (present when given, absent when not), options/temperature/max_tokens passthrough, multi-turn message serialization (system/user/tool roles incl. tool_call_id + name), HTTP error wrap, network error wrap, malformed response, streaming chunks (2 deltas + 1 done), streaming tool_call_delta (final chunk), streaming HTTP error, empty messages raises, invalid ctor args, auto-registration.
- Quality gate clean first try (no post-write fixes).
- **Phase 4 (Providers) complete.** Both Ollama providers (embeddings + LLM) wired; registry has `make_embedding(settings)` + `make_llm(settings)` resolving the default `"ollama"` provider. Phase 5 (Retrieval) unblocks next.

### Task 018 — Ollama embedding provider
- New runtime dep: `httpx>=0.27,<1.0` (installed 0.28.1 + anyio/h11/httpcore/certifi/idna).
- New module `code_atlas.providers.ollama_embeddings`. Re-exported as `OllamaEmbeddingProvider` from `code_atlas.providers`. **Importing the module triggers `register_embedding("ollama", _factory)` at module bottom** — side-effect registration. `providers/__init__.py` imports the class, which is enough to run the side-effect.
- `OllamaEmbeddingProvider(*, base_url, model, dimension, timeout_s=60.0, concurrency=4, client=None)`. Implements `EmbeddingProvider` Protocol structurally (`model`, `dimension`, `async embed`).
- Endpoint: `POST {base_url}/api/embeddings` with body `{"model": ..., "prompt": <single text>}`. Single-input API → batches via `asyncio.gather` with a **per-call `asyncio.Semaphore(concurrency)`** (per-call avoids event-loop binding bugs). Response: `{"embedding": [...]}`.
- Score / dim validation: returned vector len must equal `self.dimension`; mismatch → `ProviderError` with `{"expected", "got", "model"}`. Validates on every response (not just first).
- Client ownership tracked by `_owns_client: bool`. `client=None` → builds `httpx.AsyncClient(base_url=base_url, timeout=timeout_s)`, owns it (closed by `aclose()`). Injected client → not closed by `aclose()` (caller-owned, dependency-injection pattern for tests).
- Error wrapping: `HTTPStatusError` (4xx/5xx) → `ProviderError` with `{status_code, url, body[:200]}`. `RequestError` (connect/timeout/etc.) → `ProviderError` with `{error_type, url}`. JSON decode error → `ProviderError` with `{status_code}`. Non-dict payload or missing `embedding` key → `ProviderError` with `{keys}` or `{type}`. All wrap via `from exc`.
- `dimension < 1` or `concurrency < 1` in `__init__` → `ProviderError`. Empty `texts` → `[]`, no HTTP calls.
- `_factory(settings)` reads `settings.ollama.{base_url, timeout_s, concurrency}` + `settings.embeddings.{model, dimension}`. Note: cross-group composition (embedding model lives under `embeddings`, transport lives under `ollama`).
- Logging: `info` on `ollama_embeddings.batch` (once per `embed()`), `debug` on `.request` (per text — debug to avoid log floods), `warning` on `.failed`.
- 10 unit tests in `tests/unit/providers/test_ollama_embeddings.py` using `httpx.MockTransport(async_handler)`. Coverage: round-trip 2 inputs, empty batch → 0 calls, HTTP error wrap, network error wrap (`httpx.ConnectError`), malformed response, dim mismatch, concurrency cap (5 inputs / concurrency=2 → max-in-flight ≤ 2), auto-registration, invalid ctor args, injected-client not closed by `aclose()`.
- Test hygiene: NO `clear_registry` autouse in this file (would defeat the auto-registration test). The `test_auto_registration` test re-registers explicitly via the imported `_factory` reference, which is robust to test ordering.
- Quality gate clean first-try — no post-write fixes.

### Task 017 — Provider Protocols + registry — Phase 4 starts
- New pkg `code_atlas.providers` (depends on domain/errors/utils/config only — strict layering).
- `base.py` ships two Protocols and five frozen pydantic records:
  - `EmbeddingProvider` Protocol: `model: str`, `dimension: int`, `async embed(texts: list[str]) -> list[list[float]]`.
  - `LLMProvider` Protocol: `model: str`, `async chat(messages, *, tools=None) -> ChatResponse`, `chat_stream(messages, *, tools=None) -> AsyncIterator[ChatChunk]`.
  - `ChatRole = Literal["system", "user", "assistant", "tool"]`.
  - `ChatMessage{role, content, tool_call_id?, name?}` — `content` may be empty (for assistant turns that are tool-only).
  - `ToolSpec{name, description, parameters: dict}` — `parameters` is free-form JSON schema, NOT validated by us.
  - `ToolCall{id, name, arguments: dict}`.
  - `ChatResponse{content, tool_calls, usage: TokenUsage, model, finish_reason}`.
  - `ChatChunk{content_delta, tool_call_delta?, done, usage?, finish_reason?}` — final chunk has `done=True`; `usage` populated on final chunk only (consumer-checked, not model-enforced).
- **`chat_stream` Protocol signature is `def ... -> AsyncIterator[ChatChunk]` (NOT `async def`).** Concrete impls use `async def chat_stream(...) -> AsyncIterator[ChatChunk]` with `yield` (async-generator function). When called, this returns an `AsyncIterator` synchronously — no `await` on the call site, only on the `async for`. This is the standard Protocol+async-gen idiom; both signatures are compatible.
- `registry.py` — two module-level dicts `_EMBEDDINGS`, `_LLMS`. Public API: `register_embedding`, `register_llm`, `make_embedding`, `make_llm`, `clear_registry`, `registered_embeddings`, `registered_llms`. Aliases: `EmbeddingFactory = Callable[[Settings], EmbeddingProvider]`, `LLMFactory = Callable[[Settings], LLMProvider]`.
- Registry semantics:
  - Empty/whitespace name on register → `ProviderError`.
  - Re-register same name **silently overwrites** (so concrete providers can re-register on import without failing on test re-imports).
  - `make_embedding(settings)` reads `settings.embeddings.provider`; `make_llm(settings)` reads `settings.chat.provider`. Unknown name → `ProviderError("... not registered", context={"name": ..., "available": sorted(...)})`.
  - Factory exception → wrapped as `ProviderError(..., context={"name": name}) from exc` (original in `__cause__`).
  - Info log on register (`provider.embedding_registered`/`provider.llm_registered`), warn log on factory fail.
- **Registry ships empty.** Concrete providers will auto-register on import (Task 018: ollama embeddings, Task 019: ollama LLM).
- 10 unit tests in `tests/unit/providers/test_registry.py`. Uses `autouse` `_reset_registry` fixture that calls `clear_registry()` before AND after each test (state never leaks).
- Test fakes (`_FakeEmbedder`, `_FakeLLM`) are minimal Protocol-conformant classes inside the test module — duck-typed against the Protocols (no explicit `class ... (EmbeddingProvider):` inheritance needed since Protocols are structural).
- Phase 3 (Indexing) complete; Phase 4 (Providers) seeded.

### Task 016 — Indexer orchestrator — Phase 3 complete
- New module `code_atlas.indexing.indexer` (re-exports `Indexer`, `IndexResult`, `EmbedFunc`).
- New helper `code_atlas.indexing.edge_extractor.extract_python_edges(chunks) -> list[(Symbol, Symbol, EdgeKind)]`. Derives `defines` (module → top-level class/function) + `contained_in` (class → method, line-range nesting) from CodeChunk metadata alone — NO tree-sitter re-parse. Module Symbol synthesized as `Symbol(name=path.stem, kind="module", path=path, line=1)`.
- `EmbedFunc = Callable[[list[str]], list[list[float]]]` — **sync** boundary. Async embedders adapt via shim at caller boundary (Task 020 territory). Decision: indexer stays sync because all four stores are sync; wrapping every DB call in `asyncio.to_thread` would be over-engineering.
- `Indexer(*, metadata_store, lexical_store, vector_store, symbol_graph, embed, batch_size=64)`. Stores are caller-owned — `Indexer` NEVER closes them.
- `index_repo(root, repo_id, *, extra_ignores=None, max_chunk_lines=200, mtime_cache=None) -> IndexResult`.
- `IndexResult` (slotted dataclass): `chunks_seen, chunks_indexed, chunks_skipped_cached, embed_batches, embed_calls, edges_added, ingest_stats`.
- **Idempotency strategy (critical)**: per-batch, `metadata_store.get_many([c.chunk_id for c in batch])` → `{chunk_id: content_hash}`. Partition into `cached` (matching hash → skip embed + skip writes) vs `to_index` (new or hash-changed). Only `to_index` chunks pass to embedder + store upserts. Re-running `index_repo` on unchanged tree → `embed_calls == 0`, `embed_batches == 0`.
- **Batching**: stream from `ingest_repo`, accumulate `batch: list[CodeChunk]` to `batch_size`, flush. Final partial batch flushes at end. Embedder called once per non-empty `to_index` partition.
- **Symbol-graph build is per-file** (not per-batch), executed AFTER the chunk stream is exhausted. Maintains `per_file: dict[str, list[CodeChunk]]` for whole-run accumulation. **Memory note**: buffers all chunks in memory; acceptable v1, will need streaming on very large repos (deferred).
- Vector items get `metadata={"path": ..., "language": ..., "kind": ...}` (JSON-encoded by LanceVectorStore on persist).
- Error handling: embedder exception → `IndexingError("indexer: embed failed", context={"batch_size": N})`. Vector count mismatch → `IndexingError(..., {"expected": N, "got": M})`. Vector dim mismatch → `IndexingError(..., {"expected": dim, "got": got, "index": idx})`. Stores' own IndexingErrors propagate untouched.
- Info-level milestones: `indexer.index_repo.start`, `indexer.batch.flushed`, `indexer.symbol_graph.built`, `indexer.index_repo.completed`.
- `batch_size < 1` → `IndexingError` in `__init__`.
- 7 integration tests in `tests/integration/indexing/test_indexer.py`: 4-store fan-out, idempotency (re-run = 0 embed calls), re-embed on content change (only changed file's chunks), batching (small batch_size → ceil(n/2) batches), Python edges extracted (defines + contained_in for Greeter→greet), embed dim mismatch raises, embed count mismatch raises.
- Phase 3 (Indexing) is now complete. Phase 4 (Providers) unblocks next.

### Task 015 — Symbol graph (NetworkX MultiDiGraph)
- New runtime dep: `networkx>=3.2,<4.0` (installed: 3.6.1, ~2MB).
- New module `code_atlas.indexing.symbol_graph`. Re-exports `SymbolGraph` + `EdgeKind` (Literal) from `code_atlas.indexing`.
- Backed by `networkx.MultiDiGraph` (not plain DiGraph) so multiple edge kinds between same `(src, dst)` are independent. `key=kind` on `add_edge` makes same-kind re-adds idempotent.
- **Node key strategy**: tuple `(symbol.path, symbol.name)` as the networkx node id (hashable). Full `Symbol.model_dump()` stored as node attr `symbol_data`. Additional attr `node_id=[path, name]` (list copy) is the canonical key for save/load round-trip.
- `EdgeKind = Literal["calls", "imports", "defines", "contained_in"]`. Unknown kind on `add_edge` → `IndexingError` with `{"kind": ..., "valid_kinds": [...]}`. `get_args(EdgeKind)` drives the validation set.
- Public API: `add_symbol`, `has_symbol`, `add_edge`, `callers`, `callees`, `save`, `load` (classmethod), `__len__`, `edge_count`. Sync (matches sibling stores).
- `callers` / `callees` filter on `key == "calls"` only (NOT all edge kinds). Dedup + sort by `(path, name)` for deterministic order. Missing symbol → `[]`, not error.
- **Persistence**: `nx.node_link_data(g, edges="edges")` → `json.dumps(sort_keys=True)` → `gzip.compress(level=9)` → `path.write_bytes(...)`. `edges="edges"` (vs default "links") dodges nx 3.x FutureWarning. Pairs symmetric in `load`.
- **Round-trip gotcha**: tuple node ids serialize to JSON lists. `_coerce_key` accepts both list and tuple shapes; load rebuilds the graph fresh, keying nodes by the stored `node_id` attribute (NOT the raw `nx.node_link_graph` node id, which may be coerced inconsistently across nx versions).
- Errors: `save` / `load` wrap `Exception` → `IndexingError` with `{"path": ..., "error": str(exc)}`. `IndexingError` re-raised untouched.
- Mypy strict: `networkx` has no shipped stubs (mypy hint suggests `types-networkx`). Imported via `importlib.import_module("networkx")` + typed `Any`, matching tree-sitter-language-pack + lancedb precedent. Module-level `nx: Any` plus `self._g: Any`. `bool(...)` wrap on `has_node` return.
- No async, no context manager, no close — graph is in-memory; persistence is explicit.
- 16 unit tests cover: round-trip + has_symbol, idempotent add_symbol, add_edge auto-endpoints, all-4-kinds same pair, same-kind idempotent, unknown kind raises, callers/callees filter to "calls" only, missing symbol returns [], deterministic sort order, gzip magic bytes on save, edge_count + len totals, save to missing parent dir raises, load corrupted file raises, full save/load round-trip preserves Symbol payload (incl. line numbers), **acceptance test: 5 symbols + 6 edges across all 4 kinds, save → load, all queries identical**.

### Task 014 — Vector store (LanceDB + Protocol)
- New runtime dep: `lancedb>=0.13,<1.0`. Pulls `pyarrow`, `numpy`, `tqdm`, `urllib3`, etc. (~80MB on install). Heavy but locked-in architectural choice.
- New module `code_atlas.indexing.vector_store`. Re-exports `VectorStore` (Protocol), `VectorItem` (pydantic), `LanceVectorStore` from `code_atlas.indexing`.
- `VectorItem` (frozen pydantic, `extra="forbid"`): `chunk_id`, `repo_id`, `vector: list[float]` (min_length=1), `metadata: dict[str, Any]` (defaults to `{}`). Lives in `vector_store.py`, NOT in `domain/` (persistence record, not pure domain).
- `VectorStore` Protocol: `dimension: int` attr + `upsert(items) -> int`, `search(vector, k=10, filters=None) -> list[(chunk_id, score)]`, `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`. **Sync** API — LanceDB embedded is local file ops; callers wrap in `asyncio.to_thread`.
- `LanceVectorStore(uri, *, table_name="chunks_vec", dimension=768)`. `uri` is a directory path (NOT a SQLAlchemy URL). Creates/opens table on construction.
- PyArrow schema: `chunk_id: string, repo_id: string, vector: list_<float32, dimension>, metadata: string` (JSON-encoded via `json.dumps(..., sort_keys=True)`).
- Idempotency: `tbl.merge_insert("chunk_id").when_matched_update_all().when_not_matched_insert_all().execute(arrow_table)`. Re-upsert keeps row count at 1.
- Search: `tbl.search(vector).metric("cosine").limit(k)` + optional `.where("repo_id = '...'")`. Score = `1.0 - _distance` so higher = more relevant. For exact-match vectors, score ≈ 1.0.
- Filters dict: ONLY `repo_id: str` supported. Unknown keys → `IndexingError`. `repo_id` value containing single-quote → `IndexingError` (no SQL escape logic; reject the input instead). Empty filters dict treated as None.
- `delete_repo` counts BEFORE deleting (LanceDB's delete doesn't return rowcount reliably).
- `count(*, repo_id=...)` uses `tbl.count_rows(filter=...)` (available in lancedb >= 0.6).
- `dimension < 1`, `k < 1`, vector-length mismatch (upsert & search) → `IndexingError` with context.
- LanceDB/PyArrow exceptions wrapped via bare `except Exception` (LanceDB error hierarchy is messy across versions); IndexingErrors we raise ourselves are re-raised untouched.
- Mypy strict: `lancedb` and `pyarrow` imported via `importlib.import_module(...)` and stored as `Any` to dodge stub mismatches under `warn_unused_ignores`. Matches the tree-sitter-language-pack precedent from Task 010. `cast(list[str], ...)` for arrow result extraction.
- **lancedb 0.33 deprecation note**: `table_names()` emits a `DeprecationWarning` recommending `list_tables()`, but lancedb 0.33's `list_tables()` returns `(namespace, name)` tuples containing unhashable lists — not a string list. Don't chase the warning without checking the API; the swap is NOT a drop-in. Kept `table_names()`; warning is noise, not a bug.
- 16 unit tests cover round-trip + positive score, idempotent upsert, empty batch returns 0, repo_id filter isolation in search, delete_repo isolation, delete_repo no-match returns 0, count by repo, on-disk persistence via ctx-mgr, wrong-dim upsert + search, unknown filter key, k<1, single-quote rejected on delete/search/count, score positive for self-match, search k-limit, metadata JSON round-trip.

### Task 013 — Lexical store (SQLite FTS5)
- New module `code_atlas.indexing.lexical_store`; `LexicalStore` re-exported from `code_atlas.indexing`.
- Uses **stdlib `sqlite3`**, NOT SQLAlchemy. Diverges from `MetadataStore`: `url` is a raw filename or `":memory:"` (e.g. `LexicalStore("/tmp/lex.db")`), not a `sqlite:///...` URL. Two stores share no connection.
- Single FTS5 virtual table:
  ```
  CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content, symbol, repo_id UNINDEXED, chunk_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
  );
  ```
- Public API: `upsert(chunk)`, `upsert_many(chunks) -> int`, `search(query, k=10, repo_id=None) -> list[(chunk_id, score)]`, `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`, ctx-mgr, `connection` property.
- **Score convention**: returns `-bm25(chunks_fts)` so higher score = more relevant; rows ordered by raw `bm25 ASC` under the hood. Callers can compare scores naturally (`>`).
- Idempotency: no PK on FTS5 virtual tables, so `upsert` does `DELETE WHERE chunk_id=? + INSERT`. Re-upserting same chunk_id keeps row count at 1.
- `chunk.symbol` is `str | None`; FTS5 columns reject NULL → coerced to `""` on insert.
- All `sqlite3.Error` (incl. `OperationalError` for malformed MATCH) wrapped as `IndexingError` with context (`chunk_id`, `query`, or `repo_id`). Empty/malformed FTS query → `IndexingError`.
- `k < 1` → `IndexingError` (validated before SQL).
- 15 unit tests cover round-trip, idempotency, batch + empty, repo isolation, delete_repo, count-by-repo, on-disk persistence, BM25 multi-term-repetition ordering, positive score, malformed + empty query, k<1, None symbol coercion, no-match empty list.

### Task 012 — Metadata store (SQLite + SQLAlchemy Core) — Phase 3 starts
- New runtime dep: `sqlalchemy>=2.0,<3.0`.
- New pkg `code_atlas.indexing`. Re-exports `MetadataStore`.
- SQLAlchemy Core only (no ORM models). Module-level `MetaData` + single `chunks` table:
  - Columns: `chunk_id PK`, `repo_id` (indexed), `path`, `language`, `kind`, `symbol` (nullable), `start_line`, `end_line`, `content`, `content_hash` (indexed), `indexed_at` (ISO 8601 UTC).
- `MetadataStore(url="sqlite:///:memory:")` constructor creates engine + tables.
- Methods: `upsert(chunk)`, `upsert_many(chunks) -> int`, `get(chunk_id) -> CodeChunk | None`, `get_many(chunk_ids) -> list[CodeChunk]` (preserves input order, skips missing), `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`, `__enter__`/`__exit__`, `engine` property.
- Idempotency via SQLite-dialect `insert(...).on_conflict_do_update(index_elements=["chunk_id"], set_={...})`. Re-upserting same chunk_id updates fields (incl. `indexed_at`), preserves row count.
- All SQLAlchemy errors wrapped as `IndexingError` with context (chunk_id or batch count).
- `RowMapping` ≠ `Mapping[str, Any]` for mypy strict — call sites convert via `dict(row)`.
- Use `datetime.UTC` alias (Python 3.11+, per ruff UP017).
- 12 unit tests cover round-trip, idempotent upsert, update semantics, batch + empty batch, get_many order + missing-skip, delete_repo isolation, count-by-repo, on-disk persistence via context manager, engine property.

### Task 011 — Ingestion pipeline — Phase 2 complete
- `code_atlas.ingestion.pipeline.ingest_repo(root, repo_id, *, extra_ignores=None, max_chunk_lines=200, mtime_cache=None, stats=None) -> Iterator[CodeChunk]`.
- Re-exported from `code_atlas.ingestion` along with `IngestStats`.
- Composes walker → language detection → chunker into a lazy chunk stream.
- **Eager validation**: bad root (non-dir) or empty repo_id raise `IngestionError` BEFORE iteration (uses an inner `_iter` generator + outer factory function).
- `IngestStats` (slotted dataclass): counters for `files_seen`, `files_skipped_no_language`, `files_skipped_unreadable`, `files_skipped_unchanged`, `files_chunked`, `chunks_emitted`. Caller can pass an instance; mutated in place.
- `mtime_cache: dict[str, tuple[float, int]]` — caller-managed dict mapping `rel_path` → `(mtime, size)`. Updated only after a file is fully processed (transient parse errors don't poison the cache). On match: skip + bump `files_skipped_unchanged`.
- Per-file flow: walker yields → stat → cache check → language detect → read text → skip empty/whitespace → chunk → yield. OSError on stat or read → warn + skip (counted as `files_skipped_unreadable`).
- `pipeline.completed` info log at end with all counters.
- Non-detectable files (`.txt`, `.md`, `.gitignore`) → `files_skipped_no_language`.
- 9 integration tests in `tests/integration/ingestion/test_pipeline.py` cover: chunk emission, repo_id propagation, relative-path output, all stats counters, mtime-cache skip + refresh on `os.utime` bump, eager errors for bad root + empty repo_id, lazy `next()` consumption.

### Task 010 — Tree-sitter AST chunker
- New runtime deps: `tree-sitter>=0.23,<1.0`, `tree-sitter-language-pack>=0.10,<1.0`.
- `code_atlas.ingestion.parser.chunk_file(*, path, repo_id, language, content, max_chunk_lines=200) -> list[CodeChunk]`. Keyword-only args.
- Re-exported from `code_atlas.ingestion`.
- AST chunking implemented for **python only** in v1. All other languages → fixed-window fallback. Note in `parser.no_ast_extractor` debug log.
- Python AST extraction:
  - One chunk per top-level `function_definition` (kind=function), `class_definition` (kind=class), `decorated_definition` (uses inner def's kind/symbol).
  - One chunk per first-level method inside a class body (kind=method).
  - Decorated classes also emit their nested methods.
  - Symbol extracted from first `identifier` child (UTF-8 sliced from source bytes).
- Empty / whitespace-only content → `[]`.
- Python file with no defs → single `kind="file"` chunk covering whole content.
- Python parser exception → warning log + whole-file chunk.
- Fixed-window fallback: 50-line windows, 5-line overlap. `kind="block"`, `symbol=None`. Last window may be shorter. 100 lines → 3 windows starting at [1, 46, 91].
- `content_hash` = sha256 hex (64 chars) of chunk content.
- `chunk_id` = first 32 chars of `sha256("{repo_id}\n{path}\n{start}-{end}\n{content_hash}")`. Deterministic across runs.
- Tree-sitter handles typed as `Any` (no stubs). `importlib.import_module("tree_sitter_language_pack")` avoids stub-mismatch issues with `mypy --strict + warn_unused_ignores`.
- DEFERRED (follow-up tasks): body-splitting for oversized defs (parameter accepted but unused, marked `_ = max_chunk_lines`); per-language AST extractors for JS/TS/Go/Java/Rust/C/C++; nested classes deeper than one level.
- 11 tests cover Python def extraction, class+methods, decorated, no-defs whole-file, fixed-window math (length + start offsets), empty/whitespace, hash stability, content slicing, isinstance check.

### Task 009 — Language detection
- `code_atlas.ingestion.language.detect_language(path, content=None) -> str | None`.
- Re-exported from `code_atlas.ingestion`.
- Resolution order: extension table (case-insensitive) → shebang line (from `content` if given, else `path.read_text(errors="replace")`) → `None`.
- Extension table maps to tree-sitter-language-pack names: `python` (`.py`, `.pyi`), `javascript` (`.js`, `.mjs`, `.cjs`, `.jsx`), `typescript` (`.ts`, `.mts`, `.cts`), `tsx` (`.tsx`), `go`, `java`, `rust`, `c` (`.c`, `.h`), `cpp` (`.cc`, `.cpp`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++`).
- Shebang interpreter map: `python`/`python2`/`python3` → python, `node` → javascript, `ts-node`/`deno`/`bun` → typescript, `go`/`java`/`rustc` → respective.
- Shebang handles `#!/usr/bin/env <interp>` and absolute paths. Strips trailing digits/dots so `python3.11` → `python` after lookup miss.
- CRLF tolerated. `OSError` on read → `None`.
- Extension wins over shebang (e.g., `script.py` with `#!/usr/bin/env node` → `python`).
- 35 tests (incl. 23 parametrized table cases) all green.

### Task 008 — Repo walker (gitignore-aware)
- New pkg `code_atlas.ingestion`. Re-exports `walk_repo`.
- Runtime dep added: `pathspec>=0.12,<1.0` (uses `GitIgnoreSpec.from_lines`).
- `walk_repo(root: Path, extra_ignores: list[str] | None = None) -> Iterator[Path]`.
  - Resolves root, raises `IngestionError` if not a directory.
  - Yields absolute resolved Path objects.
- Baseline ignore list (always applied): `.git/`, `node_modules/`, `.venv/`, `venv/`, `__pycache__/`, `dist/`, `build/`, `*.lock`, `*.pyc`, `*.pyo`, `*.so`, `*.dylib`, `*.dll`.
- Honors root + nested `.gitignore` (nested patterns scoped to their subtree), `.git/info/exclude` (blanks/comments stripped), and caller-supplied `extra_ignores`.
- Binary detection: null-byte scan in first 8 KB; unreadable files treated as binary (skipped) with debug log.
- `.gitignore` files themselves ARE yielded (matches git semantics — they're tracked).
- Validation is lazy because `walk_repo` is a generator; errors raise on first iteration.
- 10 tests cover: text-only filtering, root + nested `.gitignore`, baseline + extra ignores, `.git/info/exclude`, missing-root + file-root error paths, absolute-path output.

### Task 007 — Domain types (frozen pydantic v2 models)
- New pkg `code_atlas.domain`; 4 modules, zero internal deps. Pure types.
- All models `ConfigDict(frozen=True, extra="forbid")`. Paths are `str` (JSON-stable, cross-platform).
- `chunk.py`: Literal aliases `SymbolKind` (function/method/class/module/variable/constant/other) and `ChunkKind` (file/class/function/method/block). `Symbol{name,kind,path,line,parent}`. `CodeChunk{chunk_id,repo_id,path,language,kind,symbol,start_line,end_line,content,content_hash}`. Chunk `model_validator(after)` raises `ValueError` when `end_line < start_line`.
- `retrieval.py`: Literal `RetrievalSource` (vector/lexical/fused). `RetrievalQuery{text,k=10,filters={}}` with `1 <= k <= 200`. `RetrievalResult{chunk,score>=0,source}`.
- `answer.py`: `Citation{path,start_line,end_line,symbol?,snippet<=4096}` with end>=start invariant. `TokenUsage{prompt,completion,total}` auto-fills `total = prompt + completion` when `total == 0`, else rejects `total < prompt + completion`. `Answer{text,citations=[],trace=[],latency_ms=0,token_usage=TokenUsage()}`.
- Frozen-field write inside validator uses `object.__setattr__(self, ...)` — required pydantic pattern.
- `domain/__init__.py` re-exports 10 names. Absolute imports inside the package.
- 20 unit tests cover round-trip (dict + JSON), validation rejects (empty/negative/out-of-range/extras), frozen-assignment, line-range invariant, k bounds, token-usage reconciliation, citation invariant, Answer defaults, nested round-trip.
- Tests not mypy-checked (config files = src only), so test `# type: ignore` comments are cosmetic-only.

### Task 006 — Config (pydantic-settings + YAML)
- New runtime deps: `pydantic>=2.8`, `pydantic-settings>=2.4`, `pyyaml>=6.0`. Dev: `types-pyyaml`.
- `code_atlas.config` exports `Settings` + `load_settings`.
- Top-level `Settings(BaseSettings)` holds 7 nested `BaseModel` groups: `app`, `ingestion`, `storage`, `embeddings`, `chat`, `ollama`, `eval`.
- Env prefix `CODE_ATLAS_`, nested delimiter `__`, default yaml `config/default.yaml`, default env `.env`, `extra="ignore"`.
- Source precedence (highest first): init kwargs → process env → .env → yaml → secrets. First source with a value wins.
- `load_settings(yaml_path=None, env_file=None, **overrides)`: builds a one-off `_Scoped(Settings)` subclass when paths override; else plain `Settings(**overrides)`. Used by tests; production code calls `Settings()` directly.
- Missing yaml/env paths are silently empty (pydantic-settings default).
- `config/default.yaml` mirrors field defaults verbatim. `.env.example` shows one var per section with `__` nesting + env-wins comment.
- Field-level validation: temperature bounded `[0.0, 2.0]`, integers gt=0 / gte=0 where appropriate.
- Mypy strict required `cast("SettingsConfigDict", merged_dict)` for the `_Scoped` model_config (TypedDict can't accept `**dict[str, Any]`).
- 7 unit tests pass: defaults, yaml override, env-beats-yaml, dotenv layering, init-beats-all, bad-type validation, out-of-range validation. All tests use `monkeypatch.chdir(tmp_path)` + `delenv` for hermeticity.

### Task 005 — Logging setup (structlog)
- Runtime dep: `structlog>=24.1,<26.0` (first non-dev dep).
- `code_atlas.utils` package; re-exports `configure_logging`, `get_logger`.
- `configure_logging(level, *, json=True, stream=None)`:
  - level via `logging.getLevelNamesMapping()`.
  - Resets stdlib root handler so reconfigure is idempotent; attaches `StreamHandler(stream or sys.stderr)`.
  - Processor chain: `merge_contextvars` → `add_log_level` → `StackInfoRenderer()` → (`set_exc_info` only when console) → `TimeStamper(fmt="iso", utc=True)` → `JSONRenderer()` or `ConsoleRenderer(colors=False)`.
  - `wrapper_class=make_filtering_bound_logger(level)`. `logger_factory=PrintLoggerFactory(file=target)`. `cache_logger_on_first_use=True`.
- `get_logger(name=None)` → `structlog.stdlib.BoundLogger` (via `cast`).
- ConsoleRenderer colors=False so test output is stable. Re-enable via env when wiring dev.
- 6 tests pass: JSON parseable, console human, ISO timestamp, bound-logger shape, level filter drops, reconfigure swaps stream.

### Task 004 — Errors module (typed exception hierarchy)
- `src/code_atlas/errors.py`. Base `CodeAtlasError(Exception)` carries `message: str` and `context: dict[str, Any]` (per-instance, defaults to `{}`).
- `__str__`: bare message when context empty; `"{message} | context={ctx!r}"` otherwise.
- `__repr__`: `f"{type(self).__name__}(message=..., context=...)"`.
- 8 subclasses extend the base (one-line docstring each): `ConfigError`, `IngestionError`, `IndexingError`, `ProviderError`, `RetrievalError`, `AgentError`, `EvaluationError`.
- `RepositoryNotIndexed` extends `IndexingError` (not base) so callers catch either.
- `__all__` sorted alphabetically (RUF022).
- `tests/unit/test_errors.py`: 8 tests cover str/repr, inheritance, raise/except context survival, per-instance defaults, repr-form in str.
- `tests/` tree exists; no `__init__.py` per convention (pytest discovers via `pythonpath` + rootdir).
- Verified locally: ruff format/check, mypy strict, 8/8 pytest pass.

### Task 003 — CI pipeline (GitHub Actions)
- `.github/workflows/ci.yml`. Triggers: push + pull_request, all branches.
- Concurrency group cancels in-progress on same ref. Permissions: contents:read.
- Three jobs: `lint`, `type`, `test`. `test` needs `[lint, type]`.
- `lint` + `type` pinned to Python 3.11. `test` matrix on 3.11/3.12, fail-fast off.
- Setup chain per job: checkout@v4 → setup-uv@v3 (uv cache keyed on uv.lock) → `uv python install <ver>` → `uv sync --extra dev`.
- Test step tolerates ONLY pytest exit 5 (no-tests-collected) via shell wrapper.
- Coverage artifact `coverage-py<ver>` per matrix entry; `if-no-files-found: ignore`.

### Task 002 — Dev tooling (ruff, mypy, pytest, pre-commit, editorconfig)
- Dev extras pinned: ruff>=0.6, mypy>=1.11, pytest>=8, pytest-cov>=5, pytest-asyncio>=0.23, pre-commit>=3.7.
- Ruff: line-length 120, py311, select E/F/I/UP/B/SIM/RUF, format double quotes + lf.
- `[tool.ruff.lint.per-file-ignores]` empty placeholder (forward-friendly).
- Mypy strict over `src/code_atlas`. warn_unused_ignores + warn_redundant_casts on.
- Pytest: testpaths `tests`, pythonpath `src`, asyncio_mode auto, markers slow/network/requires_ollama, strict-markers + strict-config.
- Coverage: branch on, source `code_atlas`, excludes pragma + TYPE_CHECKING + NotImplementedError.
- Pre-commit hooks: ruff-format, ruff --fix, generic hygiene (trailing-ws, eof-fixer, check-yaml/toml/merge-conflict), local mypy via `uv run mypy`.
- Editorconfig: utf-8/lf root; py 4-space cap 120; yaml/json/md/toml 2-space; Makefile tabs.
- `uv sync --extra dev` resolves cleanly. Lockfile (`uv.lock`) committed.
- Verified locally: `ruff format --check`, `ruff check`, `mypy src/code_atlas` all pass.

### Task 001 — Project scaffolding (uv + src layout)
- Hatchling backend. src layout via `packages = ["src/code_atlas"]`.
- `pyproject.toml`: name `code-atlas`, version `0.1.0`, `requires-python >=3.11`, license MIT.
- Console script `code-atlas = code_atlas.cli:app` stubbed; `cli` module lands in task 024.
- `dev` extra empty placeholder; ruff/mypy/pytest deferred to task 002.
- `[tool.uv] package = true` set.
- `src/code_atlas/__init__.py` exports `__version__`.
- `src/code_atlas/py.typed` present (PEP 561 typed marker).
- README: title, tagline, install stub.
- `.gitignore` extended with project-specific section (orchestrator backup, data, *.lance, eval/reports, config/local.yaml, .uv-cache).

## Considered but deferred
<!-- Architectural suggestions sub-agents raised that we chose not to act on. -->

## Open issues
<!-- Bugs discovered but not yet scheduled. Move to TASKS.md when scheduled. -->
