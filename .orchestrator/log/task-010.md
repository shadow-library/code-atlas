# Task 010 — Tree-sitter AST chunker

**Status:** done
**Sub-agent:** general-purpose (agentId a825879a6233f6a56)

## Files changed
- pyproject.toml (modified — added tree-sitter + tree-sitter-language-pack)
- src/code_atlas/ingestion/__init__.py (modified — re-export chunk_file)
- src/code_atlas/ingestion/parser.py (new)
- tests/unit/ingestion/test_parser.py (new)
- uv.lock (regenerated)

## Sub-agent response (verbatim)

### Summary
Adds Python AST-aware chunker `chunk_file` with fixed-window fallback in `src/code_atlas/ingestion/parser.py`, wires it through the `ingestion` package `__init__`, declares the two tree-sitter runtime dependencies, and ships unit tests covering Python AST extraction (functions, classes + methods, decorated defs), empty/whitespace inputs, no-def fallback, fixed-window math, and hash stability. Python is the only language using AST in v1; everything else (and Python parser failures) falls back to a 50-line/5-overlap fixed window. Tree-sitter handles are typed as `Any` (no stubs ship for `tree-sitter-language-pack`); the import is done via `importlib.import_module` to keep `mypy --strict` clean without `# type: ignore` lines that would conflict with `warn_unused_ignores`. Body-splitting for oversized defs is intentionally deferred.

### State update
task 010 done. parser.py makes AST chunks for python only. other lang → fixed-window 50 lines, 5 overlap. python parse fail → warn log, fall back to whole-file chunk. tree-sitter handle = `Any`, import via importlib (no `# type: ignore` to fight `warn_unused_ignores`). mypy strict clean. ruff format and check clean. fixed-window math verified at runtime: 100 lines → 3 chunks at [1,46,91], 120 lines → 3 chunks ending at 120. body-split for oversized def DEFERRED. non-python AST extractor DEFERRED. nested-class methods DEFERRED.

### Next task
Task 011: body-splitting for oversized Python defs; OR Task 012: AST extractors for JS/TS/Go/Java/Rust.

## Apply notes

- HTML-entity decoding across `->`, `<`, `>=`.
- All four checks green on first run — no post-write fixes needed.
- `uv sync --extra dev` pulled tree-sitter 0.25.2 + tree-sitter-language-pack 0.13.0 (plus 30+ bundled grammar packages, sizeable install but transparent).
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → OK (21 files)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (14 files, strict)
  - `uv run pytest tests/ -q` → 97 passed (86 prior + 11 new)
- Sub-agent's choice to use `importlib.import_module` instead of `# type: ignore` for the untyped tree-sitter API is clean — avoids the `warn_unused_ignores` trap and keeps strict mypy happy without disabling rules.
- Sub-agent's "Next task" offered two paths (body-splitting vs more languages). Per TASKS.md plan, next is task 011 (ingestion pipeline composing walker → detect → chunker).
