# Task 027 — Retrieval metrics (recall@k, MRR, nDCG)

**Status:** done
**Deps:** 026 (done)
**Files:** src/code_atlas/evaluation/metrics_retrieval.py (new), src/code_atlas/evaluation/__init__.py (modified), tests/unit/evaluation/test_metrics_retrieval.py (new)

## Apply notes

- Applied sub-agent diff verbatim. Quality gate green first-try: ruff format/check clean, mypy clean (43 source files), **297 passed** (+23 new). No post-write fixes.

## Key decisions (locked)

- Pure, stdlib-only (`math`), binary relevance (file relevant iff in expected set).
- `retrieved_files` order-preserving dedup (`list(dict.fromkeys(...))`) — first-occurrence rank matters for mrr/ndcg. `expected_files` → set.
- `k >= 1` validated for `recall_at_k` + `ndcg_at_k` → `EvaluationError(context={"k": k})`. `mrr` takes no k.
- **Empty-expected conventions**: `recall_at_k → 1.0` (vacuous), `mrr → 0.0`, `ndcg_at_k → 0.0`.
- nDCG denominator at 1-indexed rank i is `log2(i+1)` (rank 1 → 1.0); IDCG capped at `min(k, len(expected))`.
- `mrr` is single-query reciprocal rank; runner (Task 030) means it across cases for MRR.

## Carry-forward

- Task 028 (citation grounding) operates on `Answer.citations` + `MetadataStore`, not these file-level metrics. Task 030's runner will call recall/mrr/ndcg over retrieved-vs-`expected_files` per case and aggregate.

---

## Verbatim sub-agent response (abridged)

### Summary
Three pure retrieval metrics (`recall_at_k`, `mrr`, `ndcg_at_k`) in `metrics_retrieval.py`. Deduped file-path level; `expected` as set. `k >= 1` validated for the `@k` metrics (`EvaluationError(context={"k": k})`). Empty-expected: recall 1.0 (vacuous), mrr 0.0, ndcg 0.0. Re-exported from `evaluation/__init__`. 23-test suite covering all worked examples.

### Next task
028 — Citation grounding / hallucination check.
