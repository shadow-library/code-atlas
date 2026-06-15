# Architecture

> This document is the contract. Sub-agents read excerpts of it. If a task reveals
> something is wrong, stop and amend it deliberately — do not let sub-agents drift it.

## Product

`code-atlas` is a CLI + library + HTTP API that indexes large polyglot code
repositories into a hybrid (semantic + lexical + symbol-graph) store and answers
natural-language questions about them with file/function-level citations. It targets
architecture exploration, debugging assistance, navigation, and (later) code-change
suggestions. Provider-agnostic: any LLM/embedding stack (Ollama, OpenAI, Anthropic,
local sentence-transformers) plugs in behind protocols.

## Stack

- Language: Python 3.11+
- Package manager: `uv` (lockfile, venv, install)
- Lint + format: `ruff` (replaces black / isort / flake8)
- Type check: `mypy --strict` over `src/code_atlas/`
- Tests: `pytest`, `pytest-cov`, `pytest-asyncio`
- CLI: `typer`
- HTTP API: `FastAPI` + `uvicorn`
- Config: `pydantic-settings` (env + YAML)
- Logging: `structlog` (JSON in prod, console in dev)
- Vector store: LanceDB (embedded, file-based)
- Lexical store: SQLite FTS5
- Metadata: SQLite (SQLAlchemy Core, no ORM)
- Source parsing: `tree-sitter` + `tree-sitter-language-pack`
- LLM / embeddings: pluggable behind `Protocol` classes. Default: Ollama.
- HTTP client: `httpx` (async)
- Docker: `python:3.12-slim` base + non-root user
- Out of scope (v1): code edits, IDE plugin, distributed indexing, multi-tenant auth.

## Structure

```
code-atlas/
├── src/code_atlas/
│   ├── cli.py            — typer CLI entry, thin glue
│   ├── api/              — FastAPI app, request/response models
│   ├── config/           — pydantic-settings, YAML loader
│   ├── domain/           — pure types: Chunk, Symbol, Citation, Answer
│   ├── errors.py         — typed exception hierarchy
│   ├── ingestion/        — repo walker, language detection, tree-sitter chunking
│   ├── indexing/         — vector / lexical / symbol-graph / metadata stores
│   ├── providers/        — Protocols + adapters for LLM + embeddings
│   ├── retrieval/        — hybrid retrieval, RRF, citation hydration, reranker
│   ├── agent/            — Q&A orchestrator, prompts, LLM-callable tools
│   ├── evaluation/       — datasets, metrics, runner, report writer
│   └── utils/            — logging, token counting, path helpers
├── tests/                — unit + integration, mirrors src tree
├── eval/                 — golden datasets + reports (reports gitignored)
├── docs/                 — usage, architecture (user-facing), development
├── docker/               — Dockerfile + docker-compose.yml
├── .github/workflows/    — CI: lint, type, test, build
└── pyproject.toml
```

Layering rule (strict): `domain` has zero internal deps. `indexing` and `providers`
depend on `domain` only. `retrieval` depends on `indexing` + `providers`. `agent`
depends on `retrieval` + `providers`. `cli` / `api` depend on everything below.
Tests mirror the same boundaries.

## Conventions

- **Errors:** typed exceptions in `code_atlas.errors` (e.g., `RepositoryNotIndexed`,
  `ProviderError`, `IngestionError`). Carry context as attributes. No bare `Exception`.
- **Logging:** `structlog`. Per module `log = structlog.get_logger(__name__)`.
  Levels: `debug` traces, `info` lifecycle, `warning` recoverable, `error` failures.
  Never `print`.
- **Configuration:** single `Settings` (pydantic-settings) loaded from
  `config/default.yaml` + `.env` + env vars; env wins. Inject `Settings` into
  constructors. No module-level globals reading env.
- **Naming:** snake_case files / functions, PascalCase classes, UPPER_SNAKE constants.
  File name matches the primary exported type when there is one.
- **Tests:** `tests/unit/...` mirrors `src/...`; `tests/integration/...` for cross-
  module flows. Markers: `slow`, `network`, `requires_ollama`.
- **Types:** `from __future__ import annotations` everywhere. `Protocol` over ABCs
  for adapter seams. `mypy --strict` must pass.
- **Async:** I/O paths async-first (providers, API). CPU-bound (parsing, indexing)
  sync, called via `asyncio.to_thread` from async callers.
- **Commits:** Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, `test:`,
  `refactor:`, `build:`, `ci:`). No AI co-authors. Atomic, at green checkpoints.
- **Line width:** 120 chars max. Indent 2 spaces is the user's preference but Python
  community standard is 4 — code uses **4 spaces** (ruff default). Configs / YAML
  use 2.

## Subsystems

### Ingestion (`ingestion/`)
- Files: `walker.py` (gitignore-aware), `language.py` (ext → lang), `parser.py`
  (tree-sitter AST chunker), `pipeline.py` (walk → detect → parse → emit chunks).
- Output: `Iterable[CodeChunk]` with `path, start_line, end_line, symbol, kind,
  language, content, content_hash`.
- Chunking strategy: AST-aware — one chunk per top-level function / class / method.
  Bodies over `MAX_CHUNK_LINES` split on nearest inner block. Fallback fixed-window
  for unparseable / unsupported files.

### Indexing (`indexing/`)
- `metadata_store.py`: SQLite canonical `Chunk` rows; both vector + lexical reference
  by `chunk_id` (UUIDv7).
- `lexical_store.py`: SQLite FTS5 over `content + symbol`. BM25 ranking.
- `vector_store.py`: `VectorStore` Protocol + `LanceVectorStore` impl. Stores
  `(chunk_id, vector, metadata)`. Hybrid filter on metadata supported.
- `symbol_graph.py`: NetworkX DiGraph persisted to disk (pickle + JSON sidecar).
  Edges: `calls`, `imports`, `defines`, `contained_in`.
- `indexer.py`: orchestrates chunk → metadata + lexical + vector + graph writes.
  Idempotent on `content_hash`.

### Providers (`providers/`)
- `base.py`: `LLMProvider` (chat + stream), `EmbeddingProvider` (embed batch).
  Both `Protocol`s, async.
- `registry.py`: resolves provider by name from config. New provider = new file +
  one-line registration.
- `ollama.py`: default impl for both. Talks to local Ollama HTTP via `httpx`.
  Configurable model per role (`chat.model`, `embeddings.model`).
- Adding OpenAI / Anthropic / Voyage = new file under `providers/`. The rest of
  the system never imports a concrete provider.

### Retrieval (`retrieval/`)
- `hybrid.py`: runs vector + lexical in parallel via `asyncio.gather`. Fuses with
  Reciprocal Rank Fusion (RRF). Returns ranked `RetrievalResult` list.
- `reranker.py`: `Reranker` Protocol + passthrough default. Pluggable for cross-
  encoder later.
- `citation.py`: hydrates retrieved chunks back to file:line + nearest enclosing
  symbol, ready for `Citation` rendering.

### Agent (`agent/`)
- `qa.py`: takes question + indexed repo, runs retrieval, formats context for the
  LLM, drives tool-use loop, returns `Answer{ text, citations, trace }`.
- `tools.py`: LLM-callable tools — `open_file(path, start, end)`, `find_symbol(name)`,
  `list_callers(symbol)`, `list_callees(symbol)`. Tool args + return shapes are typed.
- `prompts.py`: system prompts. Hard requirements: every claim cites file:line;
  decline if no supporting chunk; never invent file paths.

### Evaluation (`evaluation/`)
- `datasets.py`: `EvalCase{ repo_id, question, expected_files, expected_symbols,
  expected_answer_traits }`. Seed sets shipped under `eval/datasets/`.
- `metrics.py`:
  - **Retrieval:** recall@k, MRR, nDCG@k on `expected_files`.
  - **Citation grounding (hallucination):** every cited (file, line range) must
    exist and the cited text must appear in that range.
  - **Answer correctness:** LLM-as-judge against `expected_answer_traits` rubric.
  - **Latency:** end-to-end p50 / p95.
  - **Cost:** tokens × per-provider rate card; rate cards in `config/costs.yaml`.
- `runner.py`: runs cases, writes JSON + Markdown report to `eval/reports/`.

## Out of scope for v1

- Code edits / refactors / PR generation.
- Cross-repo retrieval (one indexed repo per agent instance).
- Multi-tenant auth on the HTTP API.
- Real-time file-watch index updates.

## Open questions

- Reranker default: ship passthrough vs simple lexical rerank? (Decide in retrieval phase.)
- API auth model for v1: bearer token from env, or no auth? (Decide before API ships.)

---

*Sub-agents see slices of this file. Each task brief includes the structure block, the
relevant subsystem section, and the relevant convention bullets — typically 10–40 lines.*
