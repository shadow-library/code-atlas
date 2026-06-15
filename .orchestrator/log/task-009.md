# Task 009 — Language detection

**Status:** done
**Sub-agent:** general-purpose (agentId a04eeea422a7f7b14)

## Files changed
- src/code_atlas/ingestion/__init__.py (modified — re-export detect_language)
- src/code_atlas/ingestion/language.py (new)
- tests/unit/ingestion/test_language.py (new)

## Sub-agent response (verbatim)

### Summary
Implements language detection in `src/code_atlas/ingestion/language.py`, re-exports from `ingestion/__init__.py`, and adds parametrized unit tests covering extension mapping, case-insensitivity, shebang resolution (env/absolute, versioned interpreter, unknown), content-only fast path, unreadable files, extension-wins precedence, and Windows CRLF.

### State update
Task 009 done. `detect_language` lives in `ingestion/language.py`. Ext map case-insens. Shebang fallback: env-aware, versioned interpreter trim (`python3.11` → `python`), CRLF safe, OSError swallowed. Re-exported via `ingestion/__init__`. 13 unit tests added.

### Next task
Task 010: tree-sitter parser bootstrap — load grammars from `tree-sitter-language-pack` for the names returned by `detect_language`.

## Apply notes

- HTML-entity decoding across `->`, `<`, `>=`.
- All four checks green on first run — no post-write fixes needed.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → OK (19 files)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (13 files, strict)
  - `uv run pytest tests/ -q` → 86 passed (51 prior + 35 new; the parametrized extension table expands to 23 distinct cases)
- Sub-agent's "Next task" matches the planned task 010 (tree-sitter chunker).
