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
