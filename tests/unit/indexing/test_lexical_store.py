"""Tests for the SQLite FTS5 lexical store."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from code_atlas.domain import CodeChunk
from code_atlas.errors import IndexingError
from code_atlas.indexing import LexicalStore


def _chunk(chunk_id: str, **overrides: object) -> CodeChunk:
    defaults: dict[str, object] = {
        "chunk_id": chunk_id,
        "repo_id": "repo1",
        "path": "src/x.py",
        "language": "python",
        "kind": "function",
        "symbol": "foo",
        "start_line": 1,
        "end_line": 10,
        "content": "hello world",
        "content_hash": "deadbeefcafef00d",
    }
    defaults.update(overrides)
    return CodeChunk(**defaults)  # type: ignore[arg-type]


@pytest.fixture
def store() -> Iterator[LexicalStore]:
    s = LexicalStore(":memory:")
    try:
        yield s
    finally:
        s.close()


def test_round_trip_search_returns_positive_score(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="hello world"))
    results = store.search("hello")
    assert len(results) == 1
    chunk_id, score = results[0]
    assert chunk_id == "c1"
    assert score > 0.0


def test_upsert_is_idempotent_on_chunk_id(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="hello world"))
    store.upsert(_chunk("c1", content="hello brave new world"))
    assert store.count() == 1
    results = store.search("brave")
    assert len(results) == 1
    assert results[0][0] == "c1"


def test_upsert_many_returns_count(store: LexicalStore) -> None:
    chunks = [_chunk(f"c{i}", content=f"alpha beta {i}") for i in range(5)]
    n = store.upsert_many(chunks)
    assert n == 5
    assert store.count() == 5


def test_upsert_many_empty_batch_returns_zero(store: LexicalStore) -> None:
    n = store.upsert_many([])
    assert n == 0
    assert store.count() == 0


def test_repo_id_filter_isolates_results(store: LexicalStore) -> None:
    store.upsert(_chunk("a1", repo_id="repoA", content="shared keyword"))
    store.upsert(_chunk("b1", repo_id="repoB", content="shared keyword"))
    a_results = store.search("shared", repo_id="repoA")
    b_results = store.search("shared", repo_id="repoB")
    assert [cid for cid, _ in a_results] == ["a1"]
    assert [cid for cid, _ in b_results] == ["b1"]
    all_results = store.search("shared")
    assert {cid for cid, _ in all_results} == {"a1", "b1"}


def test_delete_repo_removes_only_target_repo(store: LexicalStore) -> None:
    store.upsert(_chunk("a1", repo_id="repoA", content="alpha"))
    store.upsert(_chunk("a2", repo_id="repoA", content="alpha"))
    store.upsert(_chunk("b1", repo_id="repoB", content="alpha"))
    removed = store.delete_repo("repoA")
    assert removed == 2
    assert store.count(repo_id="repoA") == 0
    assert store.count(repo_id="repoB") == 1


def test_count_by_repo(store: LexicalStore) -> None:
    store.upsert(_chunk("a1", repo_id="repoA", content="alpha"))
    store.upsert(_chunk("a2", repo_id="repoA", content="beta"))
    store.upsert(_chunk("b1", repo_id="repoB", content="gamma"))
    assert store.count() == 3
    assert store.count(repo_id="repoA") == 2
    assert store.count(repo_id="repoB") == 1
    assert store.count(repo_id="missing") == 0


def test_on_disk_persistence_via_context_manager(tmp_path: Path) -> None:
    db = tmp_path / "lex.db"
    with LexicalStore(str(db)) as s:
        s.upsert(_chunk("c1", content="persistent hello"))
    with LexicalStore(str(db)) as s:
        results = s.search("persistent")
        assert [cid for cid, _ in results] == ["c1"]


def test_bm25_ranking_orders_by_relevance(store: LexicalStore) -> None:
    store.upsert(_chunk("rare", content="needle in haystack"))
    store.upsert(_chunk("dense", content="needle needle needle needle in haystack"))
    results = store.search("needle")
    assert [cid for cid, _ in results] == ["dense", "rare"]
    dense_score = results[0][1]
    rare_score = results[1][1]
    assert dense_score >= rare_score


def test_score_is_positive_after_negation(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="quick brown fox"))
    results = store.search("fox")
    assert all(score > 0.0 for _, score in results)


def test_malformed_query_raises_indexing_error(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="hello world"))
    with pytest.raises(IndexingError) as exc_info:
        store.search('"unterminated')
    assert exc_info.value.context == {"query": '"unterminated'}


def test_empty_query_raises_indexing_error(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="hello world"))
    with pytest.raises(IndexingError):
        store.search("")


def test_none_symbol_coerced_to_empty_string(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", symbol=None, content="body text"))
    assert store.count() == 1
    results = store.search("body")
    assert [cid for cid, _ in results] == ["c1"]


def test_search_returns_empty_list_for_no_matches(store: LexicalStore) -> None:
    store.upsert(_chunk("c1", content="hello world"))
    assert store.search("zzznonexistent") == []


def test_invalid_k_raises_indexing_error(store: LexicalStore) -> None:
    with pytest.raises(IndexingError):
        store.search("hello", k=0)
