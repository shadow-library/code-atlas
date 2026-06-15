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
