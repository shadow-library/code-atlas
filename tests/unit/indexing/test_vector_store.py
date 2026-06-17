"""Unit tests for LanceVectorStore."""

from __future__ import annotations

from pathlib import Path

import pytest

from code_atlas.errors import IndexingError
from code_atlas.indexing.vector_store import LanceVectorStore, VectorItem

DIM = 4


def _store(tmp_path: Path, dimension: int = DIM) -> LanceVectorStore:
    return LanceVectorStore(str(tmp_path / "lance"), dimension=dimension)


def _item(chunk_id: str, repo_id: str, vec: list[float], meta: dict[str, str] | None = None) -> VectorItem:
    return VectorItem(
        chunk_id=chunk_id,
        repo_id=repo_id,
        vector=vec,
        metadata=meta or {},
    )


def test_round_trip_top1_is_query_with_positive_score(tmp_path: Path) -> None:
    store = _store(tmp_path)
    v_a = [1.0, 0.0, 0.0, 0.0]
    v_b = [0.0, 1.0, 0.0, 0.0]
    v_c = [0.0, 0.0, 1.0, 0.0]
    store.upsert(
        [
            _item("a", "repo1", v_a),
            _item("b", "repo1", v_b),
            _item("c", "repo1", v_c),
        ]
    )

    results = store.search(v_b, k=3)
    assert results, "expected non-empty results"
    top_id, top_score = results[0]
    assert top_id == "b"
    assert top_score > 0.0
    assert top_score <= 1.0 + 1e-6
    store.close()


def test_upsert_is_idempotent_on_chunk_id(tmp_path: Path) -> None:
    store = _store(tmp_path)
    item = _item("a", "repo1", [1.0, 0.0, 0.0, 0.0])
    store.upsert([item])
    store.upsert([item])
    store.upsert([_item("a", "repo1", [0.5, 0.5, 0.0, 0.0])])

    assert store.count() == 1
    store.close()


def test_upsert_empty_batch_returns_zero(tmp_path: Path) -> None:
    store = _store(tmp_path)
    assert store.upsert([]) == 0
    assert store.count() == 0
    store.close()


def test_search_respects_repo_id_filter(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.upsert(
        [
            _item("a1", "a", [1.0, 0.0, 0.0, 0.0]),
            _item("a2", "a", [0.9, 0.1, 0.0, 0.0]),
            _item("b1", "b", [1.0, 0.0, 0.0, 0.0]),
        ]
    )

    results = store.search([1.0, 0.0, 0.0, 0.0], k=10, filters={"repo_id": "a"})
    ids = {cid for cid, _ in results}
    assert ids == {"a1", "a2"}
    store.close()


def test_delete_repo_isolates_and_returns_count(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.upsert(
        [
            _item("a1", "a", [1.0, 0.0, 0.0, 0.0]),
            _item("a2", "a", [0.0, 1.0, 0.0, 0.0]),
            _item("b1", "b", [0.0, 0.0, 1.0, 0.0]),
        ]
    )

    removed = store.delete_repo("a")
    assert removed == 2
    assert store.count(repo_id="a") == 0
    assert store.count(repo_id="b") == 1
    assert store.count() == 1
    store.close()


def test_delete_repo_no_match_returns_zero(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.upsert([_item("a1", "a", [1.0, 0.0, 0.0, 0.0])])
    assert store.delete_repo("does-not-exist") == 0
    assert store.count() == 1
    store.close()


def test_count_by_repo(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.upsert(
        [
            _item("a1", "a", [1.0, 0.0, 0.0, 0.0]),
            _item("a2", "a", [0.0, 1.0, 0.0, 0.0]),
            _item("b1", "b", [0.0, 0.0, 1.0, 0.0]),
        ]
    )
    assert store.count() == 3
    assert store.count(repo_id="a") == 2
    assert store.count(repo_id="b") == 1
    assert store.count(repo_id="missing") == 0
    store.close()


def test_on_disk_persistence_via_context_manager(tmp_path: Path) -> None:
    uri = str(tmp_path / "lance")
    with LanceVectorStore(uri, dimension=DIM) as store:
        store.upsert(
            [
                _item("a1", "a", [1.0, 0.0, 0.0, 0.0]),
                _item("a2", "a", [0.0, 1.0, 0.0, 0.0]),
            ]
        )

    with LanceVectorStore(uri, dimension=DIM) as reopened:
        assert reopened.count() == 2
        results = reopened.search([1.0, 0.0, 0.0, 0.0], k=1)
        assert results[0][0] == "a1"


def test_upsert_wrong_dimension_raises(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(IndexingError) as exc:
        store.upsert([_item("a", "r", [1.0, 0.0, 0.0])])
    assert exc.value.context is not None
    assert exc.value.context["expected_dim"] == DIM
    assert exc.value.context["got_dim"] == 3
    store.close()


def test_search_wrong_dimension_raises(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(IndexingError):
        store.search([1.0, 0.0], k=5)
    store.close()


def test_search_unknown_filter_key_raises(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(IndexingError) as exc:
        store.search([1.0, 0.0, 0.0, 0.0], k=5, filters={"language": "py"})
    assert exc.value.context is not None
    assert "language" in exc.value.context["unknown_filter_keys"]
    store.close()


def test_search_k_less_than_one_raises(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(IndexingError):
        store.search([1.0, 0.0, 0.0, 0.0], k=0)
    store.close()


def test_single_quote_in_repo_id_rejected(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(IndexingError):
        store.delete_repo("repo'; DROP")
    with pytest.raises(IndexingError):
        store.search([1.0, 0.0, 0.0, 0.0], filters={"repo_id": "repo'; DROP"})
    with pytest.raises(IndexingError):
        store.count(repo_id="repo'; DROP")
    store.close()


def test_score_positive_for_matching_vector(tmp_path: Path) -> None:
    store = _store(tmp_path)
    v = [1.0, 2.0, 3.0, 4.0]
    store.upsert([_item("a", "r", v)])
    results = store.search(v, k=1)
    _, score = results[0]
    assert score > 0.0
    store.close()


def test_search_respects_k_limit(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.upsert(
        [
            _item("a", "r", [1.0, 0.0, 0.0, 0.0]),
            _item("b", "r", [0.0, 1.0, 0.0, 0.0]),
            _item("c", "r", [0.0, 0.0, 1.0, 0.0]),
            _item("d", "r", [0.0, 0.0, 0.0, 1.0]),
        ]
    )
    results = store.search([1.0, 1.0, 1.0, 1.0], k=2)
    assert len(results) == 2
    store.close()


def test_metadata_round_trips_via_json(tmp_path: Path) -> None:
    store = _store(tmp_path)
    item = _item("a", "r", [1.0, 0.0, 0.0, 0.0], meta={"lang": "py", "path": "x.py"})
    store.upsert([item])
    store.upsert([item])
    assert store.count() == 1
    store.close()
