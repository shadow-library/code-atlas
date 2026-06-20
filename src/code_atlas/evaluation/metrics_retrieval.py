"""Retrieval metrics: recall@k, MRR, nDCG@k over deduped file paths (binary relevance)."""

from __future__ import annotations

import math

from code_atlas.errors import EvaluationError

__all__ = ["mrr", "ndcg_at_k", "recall_at_k"]


def _dedup(items: list[str]) -> list[str]:
    """Order-preserving dedup: keeps each path at its first-occurrence rank."""
    return list(dict.fromkeys(items))


def recall_at_k(retrieved_files: list[str], expected_files: list[str], k: int) -> float:
    """Fraction of expected files appearing in the top-k retrieved (deduped). Empty expected → 1.0 (vacuous)."""
    if k < 1:
        raise EvaluationError("k must be >= 1", context={"k": k})
    expected = set(expected_files)
    if not expected:
        return 1.0
    topk = _dedup(retrieved_files)[:k]
    return len(set(topk) & expected) / len(expected)


def mrr(retrieved_files: list[str], expected_files: list[str]) -> float:
    """Reciprocal rank of the first relevant file (1-indexed, deduped). No relevant or empty expected → 0.0."""
    expected = set(expected_files)
    if not expected:
        return 0.0
    for rank, file in enumerate(_dedup(retrieved_files), start=1):
        if file in expected:
            return 1.0 / rank
    return 0.0


def ndcg_at_k(retrieved_files: list[str], expected_files: list[str], k: int) -> float:
    """Normalized DCG over top-k (deduped), binary gains. Empty expected → 0.0."""
    if k < 1:
        raise EvaluationError("k must be >= 1", context={"k": k})
    expected = set(expected_files)
    if not expected:
        return 0.0
    topk = _dedup(retrieved_files)[:k]
    dcg = sum(1.0 / math.log2(i + 1) for i, f in enumerate(topk, start=1) if f in expected)
    idcg = sum(1.0 / math.log2(i + 1) for i in range(1, min(k, len(expected)) + 1))
    return dcg / idcg
