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

<!-- Filled by the orchestrator as tasks complete. One section per task. -->

## Considered but deferred
<!-- Architectural suggestions sub-agents raised that we chose not to act on. -->

## Open issues
<!-- Bugs discovered but not yet scheduled. Move to TASKS.md when scheduled. -->
