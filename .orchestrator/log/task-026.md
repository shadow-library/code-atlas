# Task 026 — Eval dataset format + seed dataset (starts Phase 8)

**Status:** done
**Deps:** 007 (done)
**Files:** src/code_atlas/evaluation/__init__.py (new), src/code_atlas/evaluation/datasets.py (new), eval/datasets/seed.yaml (new), tests/unit/evaluation/test_datasets.py (new)

## Apply notes

- Applied sub-agent diff with two small adjustments:
  1. `evaluation/__init__.py` uses an ABSOLUTE import (`from code_atlas.evaluation.datasets import ...`) rather than the sub-agent's relative `from .datasets import ...`, per the codebase convention (domain/agent/providers all use absolute intra-package imports).
  2. Used the sub-agent's own cleaned-up `test_datasets.py` (it flagged that its first version had a needless `TYPE_CHECKING` Path alias and supplied the simplified final form).
- `evaluation` depends on `errors` + `utils`. The brief excerpt said "depends only on errors", but the loader emits a `dataset.loaded` info log via `get_logger`; `utils` is a leaf with no upward deps, so strict layering holds and this matches every other subsystem. Kept the logger (it is used, not dead).
- Quality gate green first-try: ruff format/check clean, mypy clean (42 source files), **274 passed** (6 new). No post-write fixes.

## Key decisions (locked)

- **Dataset YAML shape**: a top-level mapping with a `cases:` list (not a bare list) — leaves room for future dataset-level metadata without a format break.
- **`EvalCase`**: frozen + `extra="forbid"`; `case_id`/`repo_id`/`question` required (`min_length=1`); the three `expected_*` lists default to `[]`.
- **Error contract**: every failure path raises `EvaluationError` with a helpful `context` dict (path / index+error / case_id); wraps the cause via `from exc`. Duplicate `case_id` is enforced at load time.
- **Seed targets code-atlas itself** (10 cases, `repo_id: code-atlas`); file paths + symbols vetted against the current tree. Loaded cwd-independently via `Path(__file__).resolve().parents[3]`.

## Carry-forward

- Future metrics tasks (027 recall@k/MRR/nDCG over `expected_files`; 028 grounding; 029 LLM-judge over `expected_answer_traits`) consume `EvalCase`. The seed's `expected_symbols`/`expected_files` are the ground truth those metrics score against — keep them accurate if files move.

---

## Verbatim sub-agent response (abridged)

### Summary
Adds the `evaluation/` subsystem starting Phase 8: a frozen Pydantic v2 `EvalCase` model and a `load_dataset(path) -> list[EvalCase]` YAML loader with a typed-`EvaluationError` contract (unreadable file, invalid YAML, malformed `cases`, per-case validation, duplicate `case_id`). Ships a 10-case seed dataset (`eval/datasets/seed.yaml`) targeting code-atlas itself — all paths and symbols vetted via grep. Adds 6 unit tests.

### Model / loader / error contract
(see STATE.md Task 026 block — applied verbatim aside from the absolute-import tweak.)

### Next task
027 — Retrieval metrics (recall@k, MRR, nDCG over EvalCase.expected_files).
