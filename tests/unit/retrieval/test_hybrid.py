"""Unit tests for HybridRetriever with fake stores and a real in-memory MetadataStore."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from code_atlas.domain.chunk import CodeChunk
from code_atlas.domain.retrieval import RetrievalQuery
from code_atlas.errors import RetrievalError
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.retrieval.hybrid import HybridRetriever


def _chunk(cid: str, repo_id: str = "r", text: str = "x") -> CodeChunk:
    return CodeChunk(
        chunk_id=cid,
        repo_id=repo_id,
        path="p.py",
        language="python",
        kind="function",
        symbol=cid,
        start_line=1,
        end_line=2,
        content=text,
        content_hash="h" * 16,
    )


class FakeVectorStore:
    dimension = 4

    def __init__(self, results: list[tuple[str, float]]) -> None:
        self._results = results
        self.last_filters: dict[str, Any] | None = None
        self.last_k: int | None = None

    def search(
        self,
        vector: list[float],
        k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> list[tuple[str, float]]:
        self.last_filters = filters
        self.last_k = k
        return list(self._results[:k])


class FakeLexicalStore:
    def __init__(self, results: list[tuple[str, float]]) -> None:
        self._results = results
        self.last_repo_id: str | None = None
        self.last_k: int | None = None

    def search(
        self,
        query: str,
        k: int = 10,
        repo_id: str | None = None,
    ) -> list[tuple[str, float]]:
        self.last_repo_id = repo_id
        self.last_k = k
        return list(self._results[:k])


class FakeEmbedder:
    model = "fake"
    dimension = 4

    def __init__(self) -> None:
        self.call_count = 0
        self.last_texts: list[str] | None = None

    async def embed(self, texts: list[str]) -> list[list[float]]:
        self.call_count += 1
        self.last_texts = list(texts)
        return [[float(len(t))] * self.dimension for t in texts]


@pytest.fixture
def meta_store(tmp_path: Path) -> Iterator[MetadataStore]:
    # File-backed SQLite (not :memory:) so that asyncio.to_thread workers see the same DB.
    store = MetadataStore(f"sqlite:///{tmp_path / 'meta.sqlite'}")
    yield store
    store.close()


def _seed(meta: MetadataStore, ids: list[str]) -> None:
    for cid in ids:
        meta.upsert(_chunk(cid))


def _build(
    vec: list[tuple[str, float]],
    lex: list[tuple[str, float]],
    meta: MetadataStore,
    *,
    oversample: int = 2,
) -> tuple[HybridRetriever, FakeVectorStore, FakeLexicalStore, FakeEmbedder]:
    fv, fl, fe = FakeVectorStore(vec), FakeLexicalStore(lex), FakeEmbedder()
    r = HybridRetriever(
        vector_store=fv,
        lexical_store=fl,
        embedder=fe,
        metadata_store=meta,
        oversample=oversample,
    )
    return r, fv, fl, fe


@pytest.mark.asyncio
async def test_rrf_fuses_two_rankings_deterministically(meta_store: MetadataStore) -> None:
    # vec ranks A>B>C; lex ranks B>D>A.
    # k=60: A = 1/61 + 1/63, B = 1/62 + 1/61, C = 1/63, D = 1/62.
    # B (~0.03252) > A (~0.03227) > D (1/62) > C (1/63).
    _seed(meta_store, ["A", "B", "C", "D"])
    vec = [("A", 9.0), ("B", 8.0), ("C", 7.0)]
    lex = [("B", 9.0), ("D", 8.0), ("A", 7.0)]
    r, *_ = _build(vec, lex, meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=4))
    ids = [res.chunk.chunk_id for res in out]
    assert ids == ["B", "A", "D", "C"]


@pytest.mark.asyncio
async def test_repo_id_filter_passed_to_both_stores(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, fv, fl, _ = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    await r.retrieve(RetrievalQuery(text="q", k=1, filters={"repo_id": "r1"}))
    assert fv.last_filters == {"repo_id": "r1"}
    assert fl.last_repo_id == "r1"


@pytest.mark.asyncio
async def test_no_repo_id_passes_none_to_stores(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, fv, fl, _ = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    await r.retrieve(RetrievalQuery(text="q", k=1))
    assert fv.last_filters is None
    assert fl.last_repo_id is None


@pytest.mark.asyncio
async def test_unknown_filter_keys_are_silently_ignored(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, fv, fl, _ = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=1, filters={"language": "py"}))
    assert fv.last_filters is None
    assert fl.last_repo_id is None
    assert len(out) == 1


@pytest.mark.asyncio
async def test_invalid_repo_id_type_raises_retrieval_error(meta_store: MetadataStore) -> None:
    r, *_ = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    with pytest.raises(RetrievalError) as exc:
        await r.retrieve(RetrievalQuery(text="q", k=1, filters={"repo_id": 42}))
    assert exc.value.context["field"] == "repo_id"
    assert exc.value.context["got_type"] == "int"


@pytest.mark.asyncio
async def test_returns_top_k_only(meta_store: MetadataStore) -> None:
    ids = [f"c{i}" for i in range(10)]
    _seed(meta_store, ids)
    vec = [(cid, 1.0) for cid in ids]
    lex = [(cid, 1.0) for cid in ids]
    r, *_ = _build(vec, lex, meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=3))
    assert len(out) == 3


@pytest.mark.asyncio
async def test_dedup_by_chunk_id(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, *_ = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=5))
    assert len(out) == 1
    assert out[0].chunk.chunk_id == "A"
    # rank 0 in both lists -> 2 * 1/61.
    assert out[0].score == pytest.approx(2.0 / 61.0)


@pytest.mark.asyncio
async def test_hydration_skips_missing_metadata(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A", "B"])  # C missing on purpose
    vec = [("A", 1.0), ("B", 1.0), ("C", 1.0)]
    lex = [("A", 1.0), ("B", 1.0), ("C", 1.0)]
    r, *_ = _build(vec, lex, meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=3))
    ids = [res.chunk.chunk_id for res in out]
    assert ids == ["A", "B"]


@pytest.mark.asyncio
async def test_source_is_fused(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A", "B"])
    r, *_ = _build([("A", 1.0), ("B", 1.0)], [("B", 1.0), ("A", 1.0)], meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=2))
    assert out and all(res.source == "fused" for res in out)


@pytest.mark.asyncio
async def test_oversample_pulls_more_from_stores(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, fv, fl, _ = _build([("A", 1.0)], [("A", 1.0)], meta_store, oversample=3)
    await r.retrieve(RetrievalQuery(text="q", k=2))
    assert fv.last_k == 6
    assert fl.last_k == 6


@pytest.mark.asyncio
async def test_embedder_called_once_with_query_text(meta_store: MetadataStore) -> None:
    _seed(meta_store, ["A"])
    r, _, _, fe = _build([("A", 1.0)], [("A", 1.0)], meta_store)
    await r.retrieve(RetrievalQuery(text="hello", k=1))
    assert fe.call_count == 1
    assert fe.last_texts == ["hello"]


@pytest.mark.asyncio
async def test_returns_empty_when_both_stores_empty(meta_store: MetadataStore) -> None:
    r, *_ = _build([], [], meta_store)
    out = await r.retrieve(RetrievalQuery(text="q", k=5))
    assert out == []
