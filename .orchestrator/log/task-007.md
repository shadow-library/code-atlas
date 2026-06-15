# Task 007 — Domain types

**Status:** done
**Sub-agent:** general-purpose (agentId ad227ceacd8a55b97)

## Files changed
- src/code_atlas/domain/__init__.py (new)
- src/code_atlas/domain/chunk.py (new)
- src/code_atlas/domain/retrieval.py (new)
- src/code_atlas/domain/answer.py (new)
- tests/unit/domain/test_types.py (new)

## Sub-agent response (verbatim)

### Summary
Added the seven core domain value objects (`Symbol`, `CodeChunk`, `RetrievalQuery`, `RetrievalResult`, `Citation`, `TokenUsage`, `Answer`) as frozen pydantic v2 models with `extra="forbid"`, paired Literal kind/source types, model-validator invariants (line ranges, token totals), and 20 unit tests. The `domain` package has zero internal deps and exports everything through `code_atlas.domain`.

### State update
- New pkg `code_atlas.domain`: 4 modules, zero internal deps.
- All models frozen + extra forbid. Paths str (JSON stable).
- `chunk.py`: Symbol, CodeChunk. End>=start invariant.
- `retrieval.py`: RetrievalSource Literal, RetrievalQuery (k 1..200), RetrievalResult (score >= 0).
- `answer.py`: Citation (snippet max 4096), TokenUsage (auto-fill or reject inconsistent), Answer.
- Frozen-field write in validator via `object.__setattr__`.
- 20 tests in `tests/unit/domain/test_types.py`. No `__init__.py`.
- Tests not mypy-checked (files = src only).

### Next task
Task 008 — per TASKS.md. Domain layer ready; downstream can consume types without circular deps.

## Apply notes

- HTML-entity decoding across all `->`, `<`, `>=` operators.
- All four checks green on first run — no post-write fixes needed (cleanest task so far).
- Acceptance verified:
  - `uv run ruff format --check src tests` → OK (14 files)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (10 files, strict)
  - `uv run pytest tests/ -q` → 41 passed (21 prior + 20 new)
- Confirmed correct behavior: pydantic v2 raises `ValidationError` on frozen-attr assignment (not `TypeError`).
