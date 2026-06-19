"""Tests for the SQLite-backed metadata store."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from code_atlas.domain import CodeChunk
from code_atlas.indexing import MetadataStore


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
        "content": "def foo():\n    return 1\n",
        "content_hash": "deadbeefcafef00d",
    }
    defaults.update(overrides)
    return CodeChunk(**defaults)  # type: ignore[arg-type]


@pytest.fixture
def store() -> Iterator[MetadataStore]:
    s = MetadataStore("sqlite:///:memory:")
    try:
        yield s
    finally:
        s.close()


def test_upsert_and_get_round_trip(store: MetadataStore) -> None:
    chunk = _chunk("c1")
    store.upsert(chunk)
    got = store.get("c1")
    assert got == chunk


def test_get_missing_returns_none(store: MetadataStore) -> None:
    assert store.get("nope") is None


def test_upsert_is_idempotent(store: MetadataStore) -> None:
    chunk = _chunk("c1")
    store.upsert(chunk)
    store.upsert(chunk)
    assert store.count() == 1


def test_upsert_updates_existing(store: MetadataStore) -> None:
    original = _chunk("c1", content="def foo():\n    return 1\n", content_hash="aaaaaaaaaaaaaaaa")
    store.upsert(original)
    updated = _chunk("c1", content="def foo():\n    return 2\n", content_hash="bbbbbbbbbbbbbbbb")
    store.upsert(updated)
    got = store.get("c1")
    assert got is not None
    assert got.content == "def foo():\n    return 2\n"
    assert got.content_hash == "bbbbbbbbbbbbbbbb"
    assert store.count() == 1


def test_upsert_many_returns_count(store: MetadataStore) -> None:
    chunks = [_chunk(f"c{i}") for i in range(1, 4)]
    assert store.upsert_many(chunks) == 3
    assert store.count() == 3


def test_upsert_many_empty_returns_zero(store: MetadataStore) -> None:
    assert store.upsert_many([]) == 0
    assert store.count() == 0


def test_get_many_preserves_input_order(store: MetadataStore) -> None:
    store.upsert_many([_chunk(f"c{i}") for i in range(1, 5)])
    result = store.get_many(["c3", "c1", "c4", "c2"])
    assert [c.chunk_id for c in result] == ["c3", "c1", "c4", "c2"]


def test_get_many_skips_missing(store: MetadataStore) -> None:
    store.upsert_many([_chunk("c1"), _chunk("c2")])
    result = store.get_many(["c1", "missing", "c2"])
    assert [c.chunk_id for c in result] == ["c1", "c2"]


def test_delete_repo_only_targets_one_repo(store: MetadataStore) -> None:
    store.upsert_many(
        [
            _chunk("a1", repo_id="a"),
            _chunk("a2", repo_id="a"),
            _chunk("b1", repo_id="b"),
        ]
    )
    assert store.delete_repo("a") == 2
    assert store.count() == 1
    remaining = store.get("b1")
    assert remaining is not None
    assert remaining.repo_id == "b"


def test_count_filter_by_repo(store: MetadataStore) -> None:
    store.upsert_many(
        [
            _chunk("a1", repo_id="a"),
            _chunk("a2", repo_id="a"),
            _chunk("b1", repo_id="b"),
        ]
    )
    assert store.count(repo_id="a") == 2
    assert store.count(repo_id="b") == 1
    assert store.count(repo_id="missing") == 0
    assert store.count() == 3


def test_context_manager_closes(tmp_path: Path) -> None:
    db_path = tmp_path / "db.sqlite"
    url = "sqlite:///" + str(db_path)
    with MetadataStore(url) as s:
        s.upsert(_chunk("c1"))
    reopened = MetadataStore(url)
    try:
        got = reopened.get("c1")
        assert got is not None
        assert got.chunk_id == "c1"
    finally:
        reopened.close()


def test_engine_property_returns_engine(store: MetadataStore) -> None:
    assert store.engine is not None


def test_find_by_path_returns_chunks_sorted_by_start_line(store: MetadataStore) -> None:
    store.upsert_many(
        [
            _chunk("c50", start_line=50, end_line=60),
            _chunk("c1", start_line=1, end_line=10),
            _chunk("c25", start_line=25, end_line=30),
        ]
    )
    chunks = store.find_by_path("repo1", "src/x.py")
    assert [c.chunk_id for c in chunks] == ["c1", "c25", "c50"]


def test_find_by_path_filters_by_repo_id(store: MetadataStore) -> None:
    store.upsert_many(
        [
            _chunk("a1", repo_id="a", path="shared.py"),
            _chunk("a2", repo_id="a", path="shared.py", start_line=20, end_line=30),
            _chunk("b1", repo_id="b", path="shared.py"),
        ]
    )
    only_a = store.find_by_path("a", "shared.py")
    assert [c.chunk_id for c in only_a] == ["a1", "a2"]
    only_b = store.find_by_path("b", "shared.py")
    assert [c.chunk_id for c in only_b] == ["b1"]
    assert store.find_by_path("a", "missing.py") == []
