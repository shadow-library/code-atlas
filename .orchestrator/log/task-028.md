# Task 028 — Citation grounding (hallucination check)

**Status:** done
**Deps:** 026, 012 (both done)
**Files:** src/code_atlas/evaluation/metrics_grounding.py (new), src/code_atlas/evaluation/__init__.py (modified), tests/unit/evaluation/test_metrics_grounding.py (new)

## Apply notes

- Applied sub-agent diff verbatim except: made `__init__.__all__` multiline (sub-agent had it as one ~115-char line; multiline is clearer and sidesteps any near-limit reflow). RUF022 order unchanged.
- Quality gate green first-try: ruff format/check clean, mypy clean (44 source files), **304 passed** (+7). No post-write fixes.

## Key decisions (locked)

- `check_grounding(answer, metadata_store, *, repo_id) -> GroundingReport` — sync; `find_by_path` called directly (no threads, so `:memory:` store is fine in tests).
- Three independent binary checks per citation, each with a fixed reason string: file_exists ("file not indexed"), line_range_valid ("line range outside known chunks" — ∃ containing chunk), snippet_present ("snippet not found in cited chunk" — substring of a containing chunk).
- Empty snippet → vacuously present. Empty citations → vacuously fully grounded. No EvaluationError paths.
- `evaluation`→`indexing` dependency is annotation-only (MetadataStore under TYPE_CHECKING); `Citation` imported at runtime (pydantic field). Keeps sqlalchemy out of the evaluation import path.

## Carry-forward

- Task 030's runner will call `check_grounding(answer, metadata_store, repo_id=case.repo_id)` per case and fold `GroundingReport` into the per-case eval record. `is_fully_grounded` / `grounded`/`total` are the aggregate signals.

---

## Verbatim sub-agent response (abridged)

### Summary
Added `metrics_grounding.py` with frozen `UngroundedCitation` + `GroundingReport` and `check_grounding(answer, metadata_store, *, repo_id)`. Three per-citation binary checks (file existence, line-range containment, snippet presence; empty snippet vacuously true) aggregated into a report with an `is_fully_grounded` property. `Citation` imported at runtime; `Answer`/`CodeChunk`/`MetadataStore` under TYPE_CHECKING to keep sqlalchemy out of evaluation's import path. 7 tests against a real in-memory MetadataStore.

### Next task
029 — Answer correctness / LLM-as-judge.
