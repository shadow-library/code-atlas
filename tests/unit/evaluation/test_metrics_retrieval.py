"""Tests for retrieval metrics: recall@k, MRR, nDCG@k."""

from __future__ import annotations

import math

import pytest

from code_atlas.errors import EvaluationError
from code_atlas.evaluation.metrics_retrieval import mrr, ndcg_at_k, recall_at_k

RETRIEVED = ["a.py", "b.py", "c.py", "d.py"]
EXPECTED = ["b.py", "d.py"]
PERFECT = ["b.py", "d.py", "a.py"]


def test_recall_partial() -> None:
    assert recall_at_k(RETRIEVED, EXPECTED, 2) == 0.5


def test_recall_full() -> None:
    assert recall_at_k(RETRIEVED, EXPECTED, 4) == 1.0


def test_recall_zero() -> None:
    assert recall_at_k(RETRIEVED, EXPECTED, 1) == 0.0


def test_recall_k_truncation_excludes_beyond_k() -> None:
    # d.py (relevant) is at rank 4; k=3 must exclude it, leaving only b.py.
    assert recall_at_k(RETRIEVED, EXPECTED, 3) == 0.5


def test_recall_empty_expected_is_vacuous_one() -> None:
    assert recall_at_k(RETRIEVED, [], 2) == 1.0


def test_recall_empty_retrieved() -> None:
    assert recall_at_k([], EXPECTED, 2) == 0.0


def test_recall_dedup_counts_once() -> None:
    # Duplicate relevant collapses to one rank; topk=2 over deduped ["a","b"].
    assert recall_at_k(["a.py", "a.py", "b.py"], ["b.py"], 2) == 1.0


def test_mrr_first_relevant() -> None:
    assert mrr(RETRIEVED, EXPECTED) == 0.5


def test_mrr_perfect() -> None:
    assert mrr(PERFECT, EXPECTED) == 1.0


def test_mrr_miss() -> None:
    assert mrr(["x.py", "y.py"], EXPECTED) == 0.0


def test_mrr_empty_expected() -> None:
    assert mrr(RETRIEVED, []) == 0.0


def test_mrr_empty_retrieved() -> None:
    assert mrr([], EXPECTED) == 0.0


def test_mrr_dedup_affects_rank() -> None:
    # ["a","a","b"] dedups to ["a","b"]; first relevant b.py at rank 2.
    assert mrr(["a.py", "a.py", "b.py"], ["b.py"]) == 0.5


def test_ndcg_perfect() -> None:
    assert ndcg_at_k(PERFECT, EXPECTED, 2) == 1.0


def test_ndcg_partial() -> None:
    dcg = 1.0 / math.log2(3) + 1.0 / math.log2(5)
    idcg = 1.0 / math.log2(2) + 1.0 / math.log2(3)
    assert ndcg_at_k(RETRIEVED, EXPECTED, 4) == pytest.approx(dcg / idcg)


def test_ndcg_miss() -> None:
    assert ndcg_at_k(["x.py", "y.py"], EXPECTED, 2) == 0.0


def test_ndcg_empty_expected() -> None:
    assert ndcg_at_k(RETRIEVED, [], 2) == 0.0


def test_ndcg_empty_retrieved() -> None:
    assert ndcg_at_k([], EXPECTED, 2) == 0.0


def test_ndcg_k_truncation() -> None:
    # d.py (relevant) at rank 4 excluded by k=3; only b.py at rank 2 contributes.
    dcg = 1.0 / math.log2(3)
    idcg = 1.0 / math.log2(2) + 1.0 / math.log2(3)
    assert ndcg_at_k(RETRIEVED, EXPECTED, 3) == pytest.approx(dcg / idcg)


@pytest.mark.parametrize("bad_k", [0, -1])
def test_recall_invalid_k_raises(bad_k: int) -> None:
    with pytest.raises(EvaluationError) as exc:
        recall_at_k(RETRIEVED, EXPECTED, bad_k)
    assert exc.value.context == {"k": bad_k}


@pytest.mark.parametrize("bad_k", [0, -1])
def test_ndcg_invalid_k_raises(bad_k: int) -> None:
    with pytest.raises(EvaluationError) as exc:
        ndcg_at_k(RETRIEVED, EXPECTED, bad_k)
    assert exc.value.context == {"k": bad_k}
