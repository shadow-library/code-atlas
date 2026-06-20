# Task 029 — Answer correctness (LLM-as-judge)

**Status:** done
**Files:** `src/code_atlas/evaluation/metrics_correctness.py` (new), `src/code_atlas/evaluation/__init__.py` (re-exports), `tests/unit/evaluation/test_metrics_correctness.py` (new)

## Apply notes
- Diff applied verbatim — no post-write fixes. Quality gate clean first-try: ruff format ✓, ruff check ✓, mypy ✓ (45 source files), pytest ✓ (312 passed, 8 new).
- Harness note: Write/Edit reported stale "file not read / modified since read" errors but the writes actually landed (verified by re-reading each file); re-ran the gate against the on-disk content.

## Sub-agent response (verbatim)

All four quality gates pass: format clean, lint clean, mypy clean (45 source files), and all 312 tests pass (8 new). The warnings are pre-existing noise (lancedb `table_names()`, pytest-asyncio deprecations) unrelated to this change.

### Summary

Added the LLM-as-judge answer-correctness metric. New module `metrics_correctness.py` exposes `CorrectnessReport` (frozen pydantic: `score`, `per_trait`, `rationale`) and async `judge_answer(answer, expected_traits, llm)`. The LLM returns per-trait booleans + a rationale; **we** compute `score = (# True) / len(traits)`. Key behaviors: vacuous short-circuit for empty traits (score 1.0, no LLM call); robust JSON parsing with outermost-brace extraction for markdown-fenced output; soft-fail (score 0.0 + `log.warning`) on malformed judge output; trait alignment so `per_trait` keys always equal the requested traits (missing → False, hallucinated extras dropped); and `_coerce_bool` for stringified booleans. Re-exported both names from `evaluation/__init__.py` in RUF022 sort order. 8 unit tests added covering all spec cases. Quality gate fully green; `judge_answer` does not catch `ProviderError` (bubbles per codebase policy).

### Files changed
- `src/code_atlas/evaluation/metrics_correctness.py` (new)
- `src/code_atlas/evaluation/__init__.py` (re-exports)
- `tests/unit/evaluation/test_metrics_correctness.py` (new)

### State update

**Task 029 — done.** New `evaluation/metrics_correctness.py`: `CorrectnessReport` (frozen: `score`, `per_trait: dict[str,bool]`, `rationale`) + async `judge_answer(answer, expected_traits, llm)`. LLM returns per-trait booleans + rationale; score computed by us as `sum(True)/len(traits)`. Empty traits → vacuous 1.0 with no LLM call. Parser handles raw JSON and outermost-brace extraction (markdown fences), coerces stringified booleans, aligns keys to requested traits (missing→False, extras dropped), and soft-fails malformed output to score 0.0 + `log.warning("correctness.judge_malformed", ...)`. `ProviderError` bubbles (uncaught) per codebase policy. No tools passed to `chat`. Cost/token capture deferred to Task 030. Re-exported from `evaluation/__init__.py` in RUF022 order. 8 unit tests. Quality gate green: 312 tests pass, mypy/ruff clean.

### Next task
Task 030 — latency + cost tracking, eval runner, report writer (deps 027 ✓, 028 ✓, 029 ✓ — all met).
