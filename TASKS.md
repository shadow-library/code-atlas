# Tasks

> Format per task:
> ```
> ## {ID} — {title} [{status}]
> **deps:** {comma-separated IDs or "none"}
> **files:** {comma-separated paths}
> **desc:** {1–4 sentences}
> **accept:** {concrete acceptance criteria}
> ```
> Statuses: `pending`, `in-progress`, `done`, `blocked`, `cancelled`.

---

## Phase 1 — Foundation

## 001 — Project scaffolding (uv + src layout) [done]
**deps:** none
**files:** pyproject.toml, src/code_atlas/__init__.py, src/code_atlas/py.typed, README.md, .gitignore
**desc:** Create a uv-managed Python 3.11+ project with src/ layout. `pyproject.toml` declares package name `code-atlas`, version `0.1.0`, deps placeholder (empty `dependencies = []` + dev-deps section), and console script `code-atlas = code_atlas.cli:app`. Append project-specific lines to `.gitignore` (uv venv, lance dirs, eval reports). README skeleton with title, one-line tagline, and an "Install" stub.
**accept:** `uv sync` works on a clean checkout. `python -c "import code_atlas"` succeeds. `code-atlas --help` fails *only* because cli.py does not yet exist (that's the next task's job) — `pyproject.toml` resolves cleanly.

## 002 — Dev tooling (ruff, mypy, pre-commit) [done]
**deps:** 001
**files:** pyproject.toml, .pre-commit-config.yaml, .editorconfig
**desc:** Add `ruff` (format + lint), `mypy --strict`, and `pre-commit` to dev-deps in `pyproject.toml`. Configure ruff in `pyproject.toml` (line-length 120, target py311, select sensible rule sets: E,F,I,UP,B,SIM,RUF). Configure mypy strict over `src/code_atlas`. `.pre-commit-config.yaml` runs ruff format + check + mypy. `.editorconfig` enforces LF, utf-8, final newline, 4-space Python, 2-space yaml/json/md.
**accept:** `uv run ruff check src tests` exits 0 (on empty tree). `uv run mypy src/code_atlas` exits 0. `uv run pre-commit run --all-files` passes.

## 003 — CI pipeline (GitHub Actions) [done]
**deps:** 002
**files:** .github/workflows/ci.yml
**desc:** Workflow on push + PR. Matrix on python 3.11/3.12. Steps: checkout, install uv, `uv sync --all-extras`, `uv run ruff format --check`, `uv run ruff check`, `uv run mypy src/code_atlas`, `uv run pytest -m "not slow and not requires_ollama" --cov=code_atlas --cov-report=xml`. Upload coverage artifact.
**accept:** workflow file is valid (parses, action-lint clean). Job graph: `lint`, `type`, `test` — `test` depends on `lint` + `type`.

## 004 — Errors module (typed exception hierarchy) [done]
**deps:** 001
**files:** src/code_atlas/errors.py, tests/unit/test_errors.py
**desc:** Hierarchy rooted at `CodeAtlasError`. Subclasses: `ConfigError`, `IngestionError`, `IndexingError`, `RepositoryNotIndexed`, `ProviderError`, `RetrievalError`, `AgentError`, `EvaluationError`. Each can carry an optional `context: dict[str, Any]`. `__str__` includes context summary.
**accept:** All classes importable. Test verifies inheritance chain and that context survives `raise` / `except`.

## 005 — Logging setup (structlog) [done]
**deps:** 001
**files:** pyproject.toml, src/code_atlas/utils/__init__.py, src/code_atlas/utils/logging.py, tests/unit/utils/test_logging.py
**desc:** `configure_logging(level, json: bool)` sets up structlog with stdlib bridge, ISO timestamps, log level filter, JSON renderer in prod / ConsoleRenderer with colors in dev. Module-level `get_logger(name)` re-exports `structlog.get_logger`.
**accept:** Test calls `configure_logging`, logs a message, asserts JSON output when `json=True` and human-readable when `json=False`.

## 006 — Config (pydantic-settings + YAML) [done]
**deps:** 001
**files:** pyproject.toml, src/code_atlas/config/__init__.py, src/code_atlas/config/settings.py, config/default.yaml, .env.example, tests/unit/config/test_settings.py
**desc:** `Settings` (pydantic-settings) with nested groups: `app` (log_level, log_json), `ingestion` (max_chunk_lines, ignore_patterns), `storage` (root_dir, lance_uri), `embeddings` (provider, model, dimension), `chat` (provider, model, temperature), `ollama` (base_url, timeout_s), `eval` (cost_rates_path). Loads from `config/default.yaml` first, then `.env`, then env vars (env wins). `.env.example` shows the env var names.
**accept:** Loading `Settings()` in a tmpdir with custom yaml + .env reads the override correctly. Env var override beats yaml. Missing required fields surface a clear validation error.

## 007 — Domain types [done]
**deps:** 001
**files:** src/code_atlas/domain/__init__.py, src/code_atlas/domain/chunk.py, src/code_atlas/domain/retrieval.py, src/code_atlas/domain/answer.py, tests/unit/domain/test_types.py
**desc:** Pydantic models (frozen + slots where supported). `CodeChunk{chunk_id, repo_id, path, language, kind, symbol, start_line, end_line, content, content_hash}`. `Symbol{name, kind, path, line, parent}`. `RetrievalQuery{text, k, filters}`. `RetrievalResult{chunk, score, source}` (`source` = "vector" | "lexical" | "fused"). `Citation{path, start_line, end_line, symbol, snippet}`. `Answer{text, citations, trace, latency_ms, token_usage}`. `TokenUsage{prompt, completion, total}`. All have JSON serialization round-trip.
**accept:** Each type serializes → deserializes losslessly. Validation rejects negative lines or empty content. Mypy strict passes.

---

## Phase 2 — Ingestion

## 008 — Repo walker (gitignore-aware) [done]
**deps:** 007
**files:** pyproject.toml, src/code_atlas/ingestion/__init__.py, src/code_atlas/ingestion/walker.py, tests/unit/ingestion/test_walker.py
**desc:** `walk_repo(root: Path, extra_ignores: list[str]) -> Iterable[Path]`. Honors `.gitignore`, `.git/info/exclude`, plus a hardcoded baseline (node_modules, .venv, __pycache__, dist, build, *.lock). Returns absolute paths to text files only (skip binaries via null-byte sniff).
**accept:** Test with a tmpdir containing a .gitignore yields exactly the expected files. Binary file (random bytes) is skipped. Nested .gitignore honored.

## 009 — Language detection [done]
**deps:** 008
**files:** src/code_atlas/ingestion/__init__.py, src/code_atlas/ingestion/language.py, tests/unit/ingestion/test_language.py
**desc:** `detect_language(path: Path, content: str | None = None) -> str | None`. Extension table first (py, js, ts, tsx, jsx, go, java, rs, c, cc, cpp, h, hpp). Shebang fallback for extension-less files. Returns tree-sitter-language-pack-compatible language name or `None`.
**accept:** Table-driven test covers all initial languages + a None case.

## 010 — Tree-sitter AST chunker [done]
**deps:** 007, 009
**files:** pyproject.toml, src/code_atlas/ingestion/__init__.py, src/code_atlas/ingestion/parser.py, tests/unit/ingestion/test_parser.py
**desc:** `chunk_file(path: Path, language: str, content: str) -> list[CodeChunk]`. Uses `tree-sitter-language-pack` to get parsers. Walks AST, emits one chunk per top-level function / class / method. For bodies over `max_chunk_lines`, split on inner block boundaries. `content_hash` = sha256 of normalized content. Falls back to fixed-window chunks (50 lines, 5 overlap) for unsupported languages or parser failures.
**accept:** Python source with two functions → two chunks with correct symbol names and line ranges. Unknown-language file → fixed-window chunks. Hash is stable across runs.

## 011 — Ingestion pipeline [done]
**deps:** 008, 009, 010
**files:** src/code_atlas/ingestion/__init__.py, src/code_atlas/ingestion/pipeline.py, tests/integration/ingestion/test_pipeline.py
**desc:** `ingest_repo(root: Path, repo_id: str) -> Iterator[CodeChunk]`. Composes walker → detect → chunk. Skips unchanged files via mtime + size cache (in-memory only for now). Emits chunks lazily.
**accept:** Integration test over a tiny fixture repo yields expected chunk count and a chunk for a named function.

---

## Phase 3 — Indexing

## 012 — Metadata store (SQLite + SQLAlchemy Core) [done]
**deps:** 007
**files:** pyproject.toml, src/code_atlas/indexing/__init__.py, src/code_atlas/indexing/metadata_store.py, tests/unit/indexing/test_metadata_store.py
**desc:** SQLite-backed `MetadataStore` with table `chunks(chunk_id PK, repo_id, path, language, kind, symbol, start_line, end_line, content, content_hash, indexed_at)`. SQLAlchemy Core (no ORM). Methods: `upsert(chunk)`, `get(chunk_id)`, `get_many(ids)`, `delete_repo(repo_id)`. Idempotent on `(repo_id, path, content_hash)`.
**accept:** Upsert is idempotent (no duplicate rows on re-upsert). `get_many` preserves input order.

## 013 — Lexical store (SQLite FTS5) [done]
**deps:** 012
**files:** src/code_atlas/indexing/lexical_store.py, tests/unit/indexing/test_lexical_store.py
**desc:** Separate FTS5 virtual table `chunks_fts(content, symbol, repo_id UNINDEXED, chunk_id UNINDEXED)`. `LexicalStore.upsert(chunk)`, `LexicalStore.search(query, k, repo_id) -> list[(chunk_id, score)]`. BM25 ranking. Tokenizer: `unicode61 remove_diacritics 2`.
**accept:** Inserting a chunk containing "hello world" and searching `"hello"` returns it with non-zero score. Score order matches BM25 expectation across multiple chunks.

## 014 — Vector store (LanceDB + Protocol) [done]
**deps:** 007
**files:** src/code_atlas/indexing/vector_store.py, tests/unit/indexing/test_vector_store.py
**desc:** `VectorStore` Protocol: `upsert(items: Iterable[VectorItem])`, `search(vector, k, filters) -> list[(chunk_id, score)]`, `delete_repo(repo_id)`. `VectorItem{chunk_id, repo_id, vector, metadata}`. `LanceVectorStore` impl writes to a configurable LanceDB URI. Uses cosine similarity.
**accept:** Round-trip: upsert 3 items, search with the embedding of one, top-1 result is that item.

## 015 — Symbol graph [done]
**deps:** 007
**files:** src/code_atlas/indexing/symbol_graph.py, tests/unit/indexing/test_symbol_graph.py
**desc:** `SymbolGraph` wraps `networkx.DiGraph`. Nodes: `Symbol`. Edges typed: `calls`, `imports`, `defines`, `contained_in`. Methods: `add_symbol`, `add_edge`, `callers(symbol)`, `callees(symbol)`, `save(path)`, `load(path)`. Persist as gzipped JSON (node-link format) for portability.
**accept:** Build graph with 5 symbols and 6 edges, save + load, all queries return same results.

## 016 — Indexer orchestrator [done]
**deps:** 011, 012, 013, 014, 015
**files:** src/code_atlas/indexing/indexer.py, tests/integration/indexing/test_indexer.py
**desc:** `Indexer` composes ingestion + the four stores. `index_repo(root, repo_id, embedder)` walks chunks through metadata + lexical + vector + symbol graph in one pass. Batches embedding calls (configurable batch size). Idempotent on `content_hash`. Symbol graph edges populated only when language has a defined extractor (tree-sitter queries per language; ship Python extractor in this task, others as follow-ups).
**accept:** Integration test over the test fixture: re-running `index_repo` is a no-op (no extra embedding calls, no duplicate rows).

---

## Phase 4 — Providers

## 017 — Provider Protocols + registry [done]
**deps:** 006, 007
**files:** src/code_atlas/providers/__init__.py, src/code_atlas/providers/base.py, src/code_atlas/providers/registry.py, tests/unit/providers/test_registry.py
**desc:** `EmbeddingProvider` Protocol: `async embed(texts: list[str]) -> list[list[float]]`, plus `dimension: int`, `model: str`. `LLMProvider` Protocol: `async chat(messages, tools=None) -> ChatResponse`, `async chat_stream(messages, tools=None) -> AsyncIterator[ChatChunk]`. `ChatResponse{content, tool_calls, usage}`. Registry maps name → factory: `register_embedding("name", factory)`, `register_llm("name", factory)`, `make_embedding(settings) -> EmbeddingProvider`, `make_llm(settings) -> LLMProvider`.
**accept:** Test registers a fake provider, resolves it through `make_*`, calls it, returns expected shape. Unknown name → `ProviderError`.

## 018 — Ollama embedding provider [done]
**deps:** 017
**files:** src/code_atlas/providers/ollama_embeddings.py, tests/unit/providers/test_ollama_embeddings.py
**desc:** `OllamaEmbeddingProvider(base_url, model, timeout_s)` calls `POST {base_url}/api/embeddings`. Batches by sequential calls (Ollama embeddings API is single-input; concurrency via `asyncio.gather` capped at N). Auto-registers under `"ollama"` on import.
**accept:** With a mock httpx transport, `embed(["hello", "world"])` returns two vectors of declared `dimension`. Network errors → `ProviderError`.

## 019 — Ollama LLM provider (chat + stream + tools) [done]
**deps:** 017
**files:** src/code_atlas/providers/ollama_llm.py, tests/unit/providers/test_ollama_llm.py
**desc:** `OllamaLLMProvider(base_url, model, temperature, timeout_s)` talks to `POST {base_url}/api/chat`. Supports `tools` (OpenAI-style function schema, translated to Ollama's format). `chat_stream` consumes NDJSON stream. Token usage extracted from `prompt_eval_count` + `eval_count`. Auto-registers under `"ollama"`.
**accept:** Mock-transport tests cover: plain chat returns content + usage; stream yields chunks then a final stop chunk; tool call returns `tool_calls` field.

---

## Phase 5 — Retrieval

## 020 — Hybrid retrieval (vector + lexical, RRF) [done]
**deps:** 013, 014, 017
**files:** src/code_atlas/retrieval/__init__.py, src/code_atlas/retrieval/hybrid.py, tests/unit/retrieval/test_hybrid.py
**desc:** `HybridRetriever(vector_store, lexical_store, embedder, metadata_store)`. `async retrieve(query: RetrievalQuery) -> list[RetrievalResult]`. Runs vector + lexical in parallel via `asyncio.gather`. Fuses ranks via Reciprocal Rank Fusion (k=60). Returns top-`query.k` deduped by `chunk_id`, hydrated with `CodeChunk` via metadata store.
**accept:** Synthetic test: 5 chunks where vector ranks A>B>C and lexical ranks B>A>D — RRF yields A & B at top, C and D follow. Test asserts deterministic order.

## 021 — Citation hydration + Reranker Protocol [done]
**deps:** 020
**files:** src/code_atlas/retrieval/citation.py, src/code_atlas/retrieval/reranker.py, tests/unit/retrieval/test_citation.py
**desc:** `to_citation(chunk: CodeChunk) -> Citation` extracts nearest enclosing symbol (already on chunk) and a snippet (configurable max chars). `Reranker` Protocol with `async rerank(query, results) -> results`. Default `PassthroughReranker` returns input as-is.
**accept:** Citation includes correct path/lines/symbol. Passthrough rerank preserves order.

---

## Phase 6 — Agent

## 022 — Agent tools (file + symbol lookups) [done]
**deps:** 012, 015
**files:** src/code_atlas/agent/__init__.py, src/code_atlas/agent/tools.py, tests/unit/agent/test_tools.py
**desc:** Tool implementations + JSON schemas for LLM: `open_file(path, start_line, end_line)`, `find_symbol(name, kind=None)`, `list_callers(symbol)`, `list_callees(symbol)`. All take a `repo_id` from a bound context (no LLM hallucinating repo IDs). `Toolbox(metadata_store, symbol_graph, repo_id)` exposes a dict `name -> callable` and a list of JSON schemas.
**accept:** Each tool returns a typed dict with deterministic shape. Unknown symbol returns `{"results": []}`, never raises.

## 023 — Prompts + QA agent [pending]
**deps:** 020, 021, 022, 019
**files:** src/code_atlas/agent/prompts.py, src/code_atlas/agent/qa.py, tests/integration/agent/test_qa.py
**desc:** `prompts.py` holds system prompt with hard rules: cite file:line for every claim; decline if no supporting chunk; never invent paths. `QAAgent(retriever, llm, toolbox, max_tool_iters=4)`. `async ask(question) -> Answer`. Loop: format context, call LLM with tools, if tool calls present, execute and feed back, otherwise extract text + citations. Latency + token usage captured.
**accept:** Integration test with a fake LLM (canned tool-call then final answer) and a fixture index returns an Answer with at least one citation matching the retrieved chunk.

---

## Phase 7 — CLI + API

## 024 — CLI (typer): init / ingest / ask [pending]
**deps:** 016, 023
**files:** src/code_atlas/cli.py, tests/unit/test_cli.py
**desc:** `typer` app `code-atlas`. Commands: `init` (write a default config to cwd), `ingest --repo <path> --id <repo_id>` (run Indexer), `ask "<question>" --repo-id <id>` (run QAAgent, print Answer with citations rendered as `path:start-end`). Reads Settings via env/yaml. Pretty output via `rich`.
**accept:** `code-atlas --help` lists three commands. Invoking each with `--help` shows flags. CliRunner test for `init` writes the file.

## 025 — FastAPI app (/health, /ingest, /ask, SSE stream) [pending]
**deps:** 016, 023
**files:** src/code_atlas/api/__init__.py, src/code_atlas/api/app.py, src/code_atlas/api/routes.py, src/code_atlas/api/models.py, tests/unit/api/test_app.py
**desc:** FastAPI app. `GET /health` returns `{status: "ok", version}`. `POST /ingest` body `{repo_path, repo_id}` triggers indexing (background task), returns 202 + job id. `POST /ask` body `{repo_id, question}` returns `Answer` JSON. `GET /ask/stream?repo_id&question` returns SSE stream of chat tokens followed by a final `event: done` carrying the full Answer.
**accept:** `TestClient` covers all four endpoints with mocked indexer/agent. SSE stream test asserts at least one data event and a terminal done event.

---

## Phase 8 — Evaluation

## 026 — Eval dataset format + seed dataset [pending]
**deps:** 007
**files:** src/code_atlas/evaluation/__init__.py, src/code_atlas/evaluation/datasets.py, eval/datasets/seed.yaml, tests/unit/evaluation/test_datasets.py
**desc:** `EvalCase` pydantic model: `case_id, repo_id, question, expected_files (list[str]), expected_symbols (list[str]), expected_answer_traits (list[str])`. `load_dataset(path) -> list[EvalCase]` reads YAML. Seed dataset: 6–10 cases targeting *this* repo (code-atlas itself) — questions like "Where is the hybrid retriever defined?", "What protocol does an embedding provider implement?".
**accept:** Seed YAML parses. Loader validates required fields. Case `case_id`s are unique.

## 027 — Retrieval metrics (recall@k, MRR, nDCG) [pending]
**deps:** 026
**files:** src/code_atlas/evaluation/metrics_retrieval.py, tests/unit/evaluation/test_metrics_retrieval.py
**desc:** Pure functions: `recall_at_k(retrieved_files, expected_files, k)`, `mrr(retrieved_files, expected_files)`, `ndcg_at_k(retrieved_files, expected_files, k)`. Operate on file-path level (deduped).
**accept:** Tests cover exact known inputs/outputs for each metric, including ties and empty cases.

## 028 — Citation grounding (hallucination check) [pending]
**deps:** 026, 012
**files:** src/code_atlas/evaluation/metrics_grounding.py, tests/unit/evaluation/test_metrics_grounding.py
**desc:** Given an `Answer` + `MetadataStore`, verify each citation: (a) file exists in the repo index, (b) the line range is within the file's known chunk ranges, (c) the citation `snippet` (if non-empty) is a substring of the chunk content. Return `GroundingReport{total, grounded, ungrounded_citations}`.
**accept:** Tests: fully grounded answer → all green; fabricated citation → flagged; out-of-range line → flagged; correct file but wrong snippet → flagged.

## 029 — Answer correctness (LLM-as-judge) [pending]
**deps:** 026, 019
**files:** src/code_atlas/evaluation/metrics_correctness.py, tests/unit/evaluation/test_metrics_correctness.py
**desc:** `judge_answer(answer, expected_traits, llm) -> CorrectnessReport{score: 0..1, per_trait: dict[str, bool], rationale: str}`. Prompt instructs judge LLM to evaluate each trait independently. Uses the same provider abstraction.
**accept:** With a fake LLM returning canned JSON, returns the parsed report shape; malformed judge output → score 0 + warning.

## 030 — Latency + cost tracking, eval runner, report writer [pending]
**deps:** 027, 028, 029
**files:** src/code_atlas/evaluation/metrics_cost.py, src/code_atlas/evaluation/runner.py, src/code_atlas/evaluation/report.py, config/costs.yaml, tests/integration/evaluation/test_runner.py
**desc:** `metrics_cost.py` reads `config/costs.yaml` (per-provider per-model USD per 1k tokens for prompt + completion), turns `TokenUsage` into USD. `runner.py`: iterate cases, run agent per case, collect retrieval / grounding / correctness / latency / cost, emit `EvalRun{cases, aggregates}`. `report.py` writes both `eval/reports/{run_id}.json` and a Markdown summary table.
**accept:** Integration test runs the runner over 2 cases with a stubbed agent, produces both report files, JSON parses, Markdown has expected sections.

---

## Phase 9 — Infrastructure & Docs

## 031 — Docker + docker-compose (Ollama) [pending]
**deps:** 024, 025
**files:** docker/Dockerfile, docker/docker-compose.yml, .dockerignore
**desc:** Multi-stage Dockerfile: builder (uv install) → runtime (python:3.12-slim, non-root user, copy installed venv + src, expose 8000). Entrypoint runs `uvicorn code_atlas.api.app:app`. `docker-compose.yml` defines two services: `ollama` (ollama/ollama image, volume for models, exposes 11434) and `code-atlas` (build context root) depending on ollama. `.dockerignore` excludes tests, eval reports, .venv, .git.
**accept:** `docker compose build` succeeds. `docker compose config` validates. Image runs (assert via `docker run --rm <img> code-atlas --help`).

## 032 — Makefile + developer docs [pending]
**deps:** 031
**files:** Makefile, README.md, docs/usage.md, docs/development.md, docs/architecture.md
**desc:** Makefile targets: `install`, `fmt`, `lint`, `type`, `test`, `test-all`, `eval`, `run-api`, `docker-build`, `docker-up`, `clean`. README expanded: install + quickstart (ingest + ask), pointers to docs. `docs/usage.md` covers CLI + API examples. `docs/development.md` covers local setup, tests, eval, contributing. `docs/architecture.md` is a user-facing summary of the system (not a copy of ARCHITECTURE.md).
**accept:** `make install` resolves; `make test` runs the suite; docs are linked from README; no broken internal links.

---

<!-- Add new tasks below this line as they emerge. Use IDs 033+. -->
