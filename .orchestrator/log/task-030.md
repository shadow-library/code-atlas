# Task 030 — Latency + cost tracking, eval runner, report writer

**Status:** done (Phase 8 complete)
**Files:** `src/code_atlas/evaluation/metrics_cost.py` (new), `src/code_atlas/evaluation/runner.py` (new), `src/code_atlas/evaluation/report.py` (new), `config/costs.yaml` (new), `src/code_atlas/evaluation/__init__.py` (re-exports), `tests/integration/evaluation/test_runner.py` (new)

## Apply notes
- Sub-agent did NOT write files this time (unlike Task 029) — applied all diffs via Write/Edit myself.
- Post-write fixes: (1) reordered `runner.py` imports — sub-agent placed `from code_atlas.domain.answer import TokenUsage` after the `evaluation.*` block; moved it before (isort: domain < evaluation). (2) `ruff format` wrapped 4 long `EvaluationError(...)` calls in `metrics_cost.py`. (3) `ruff check --fix` confirmed `__all__` ASCII/case-sensitive sort (no changes needed beyond what I wrote).
- Gate clean after fixes: ruff format ✓, ruff check ✓, mypy ✓ (48 source files), pytest ✓ (317 passed, 5 new).

## Sub-agent response (verbatim)

RUF022 sorts case-sensitively (uppercase before lowercase, ASCII order) — matching the existing `__init__.py` which already has uppercase names before lowercase. The ascii sort is correct. The full `__all__` is the ascii ordering above (including `load_dataset`).

I have everything needed. Here are the diffs.

### Summary

Closes Phase 8 by adding three evaluation modules, a cost rate-card config, and an integration test:

- **metrics_cost.py** — `CostRate`/`CostTable`, `load_cost_table` (mirrors `datasets.py` error contract), and `estimate_cost` (per-1k prompt/completion math with `default`-model fallback and a `cost.rate_missing` warning → `0.0` on a missing provider/model).
- **runner.py** — the `Agent` structural `Protocol` (no `agent` import), `CaseResult`/`EvalAggregates`/`EvalRun` records, and `EvalRunner.run` which drives one case at a time (errors propagate), computes all four metric families, and aggregates with pure helpers (`_mean`, `_grounding_rate`, `_percentile` with linear interpolation). Filesystem-safe `run_id`.
- **report.py** — `render_markdown` (exact `# Eval report:` / `## Aggregates` / `## Per-case results` headers, Markdown tables) and `write_report` (JSON + MD, returns both paths).
- **config/costs.yaml** — illustrative rate cards (ollama `default` → 0.0; openai/anthropic real-ish rates).
- **evaluation/__init__.py** — re-exports the new public names; `__all__` re-sorted by ruff RUF022 (ASCII: uppercase before lowercase).
- **tests/integration/evaluation/test_runner.py** — stub agent + stub judge, real in-memory `MetadataStore`, inline cost table; runs 2 cases, asserts aggregates/cost math, report files + content, and direct `estimate_cost`/`load_cost_table` round-trip against the shipped yaml.

Pydantic footgun handled: `GroundingReport`, `CorrectnessReport`, `TokenUsage` are runtime imports in `runner.py` (pydantic field annotations); `Answer`/`EvalCase`/`MetadataStore`/`LLMProvider`/`CostTable`/`Path` stay under `TYPE_CHECKING`.

### State update (sub-agent)
- metrics_cost.py: `CostRate` frozen, `CostTable` alias, `load_cost_table` (datasets-style error contract), `estimate_cost` (`default`-model fallback; `cost.rate_missing` warning → 0.0).
- runner.py: `Agent` Protocol (no agent import), `CaseResult`/`EvalAggregates`/`EvalRun`, `EvalRunner.run` (errors propagate). Helpers `_mean`, `_grounding_rate` (vacuous 1.0), `_percentile` (linear interpolation). `run_id` = `{UTC %Y%m%dT%H%M%S}-{uuid4 hex8}`. Pydantic-field types runtime-imported.
- report.py: exact headers; `write_report` → `(json_path, md_path)`.
- config/costs.yaml: ollama default→0.0; openai/anthropic rates.
- Gotcha: `estimate_cost` uses `models.get(model) or models.get("default")` — relies on a `CostRate` instance always being truthy.

### Next task (sub-agent)
Phase 8 closed. Next is the first `[pending]` task of Phase 9 — Task 031 (Docker + docker-compose).
