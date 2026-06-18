"""Integration tests for the Indexer orchestrator over a tiny fixture repo."""

from __future__ import annotations

import math
from collections.abc import Iterator
from pathlib import Path

import pytest

from code_atlas.errors import IndexingError
from code_atlas.indexing import (
    Indexer,
    LanceVectorStore,
    LexicalStore,
    MetadataStore,
    SymbolGraph,
)

ALPHA_V1 = 'def hello():\n    return "hi"\n\ndef world():\n    return "wo"\n'
ALPHA_V2 = 'def hello():\n    return "HI!"\n\ndef world():\n    return "WO!"\n'
BETA_SRC = (
    "class Greeter:\n"
    "    def greet(self):\n"
    "        return hello()\n"
    "\n"
    "def main():\n"
    "    g = Greeter()\n"
    "    return g.greet()\n"
)


def _build_repo(root: Path, *, alpha: str = ALPHA_V1) -> None:
    src = root / "src"
    src.mkdir(parents=True, exist_ok=True)
    (src / "alpha.py").write_text(alpha, encoding="utf-8")
    (src / "beta.py").write_text(BETA_SRC, encoding="utf-8")


class FakeEmbedder:
    def __init__(self, dim: int = 4) -> None:
        self.dim = dim
        self.call_count = 0
        self.texts_seen: list[str] = []

    def __call__(self, texts: list[str]) -> list[list[float]]:
        self.call_count += 1
        self.texts_seen.extend(texts)
        return [[float(i + 1) for i in range(self.dim)] for _ in texts]


class BadCountEmbedder:
    def __init__(self, dim: int = 4) -> None:
        self.dim = dim

    def __call__(self, texts: list[str]) -> list[list[float]]:
        return [[float(i + 1) for i in range(self.dim)] for _ in texts[:-1]]


class BadDimEmbedder:
    def __init__(self, dim: int = 4) -> None:
        self.dim = dim

    def __call__(self, texts: list[str]) -> list[list[float]]:
        return [[1.0, 2.0] for _ in texts]


@pytest.fixture
def stores(tmp_path: Path) -> Iterator[tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph]]:
    meta = MetadataStore("sqlite:///:memory:")
    lex = LexicalStore(":memory:")
    vec = LanceVectorStore(str(tmp_path / "lance"), dimension=4)
    graph = SymbolGraph()
    try:
        yield meta, lex, vec, graph
    finally:
        meta.close()
        lex.close()
        vec.close()


def test_index_repo_writes_all_four_stores(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=FakeEmbedder(dim=4),
    )

    result = indexer.index_repo(tmp_path, "repo1")

    assert result.chunks_seen > 0
    assert result.chunks_indexed == result.chunks_seen
    assert meta.count(repo_id="repo1") == result.chunks_indexed
    assert lex.count(repo_id="repo1") == result.chunks_indexed
    assert vec.count(repo_id="repo1") == result.chunks_indexed
    assert graph.edge_count() > 0


def test_index_repo_is_idempotent(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    embedder = FakeEmbedder(dim=4)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=embedder,
    )

    first = indexer.index_repo(tmp_path, "repo1")
    embedder.call_count = 0
    embedder.texts_seen.clear()

    second = indexer.index_repo(tmp_path, "repo1")

    assert second.embed_calls == 0
    assert second.embed_batches == 0
    assert second.chunks_skipped_cached == first.chunks_indexed
    assert second.chunks_indexed == 0
    assert embedder.call_count == 0
    assert meta.count(repo_id="repo1") == first.chunks_indexed
    assert lex.count(repo_id="repo1") == first.chunks_indexed
    assert vec.count(repo_id="repo1") == first.chunks_indexed


def test_index_repo_re_embeds_on_content_change(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    embedder = FakeEmbedder(dim=4)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=embedder,
    )

    indexer.index_repo(tmp_path, "repo1")
    embedder.call_count = 0
    embedder.texts_seen.clear()

    (tmp_path / "src" / "alpha.py").write_text(ALPHA_V2, encoding="utf-8")

    result = indexer.index_repo(tmp_path, "repo1")

    assert result.embed_calls > 0
    for text in embedder.texts_seen:
        assert "Greeter" not in text
        assert "main" not in text


def test_index_repo_batches_embed_calls(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    embedder = FakeEmbedder(dim=4)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=embedder,
        batch_size=2,
    )

    result = indexer.index_repo(tmp_path, "repo1")

    assert result.chunks_indexed >= 4
    expected_batches = math.ceil(result.chunks_indexed / 2)
    assert result.embed_batches == expected_batches
    assert embedder.call_count == expected_batches


def test_index_repo_python_edges_extracted(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=FakeEmbedder(dim=4),
    )

    indexer.index_repo(tmp_path, "repo1")

    inner = graph._g  # type: ignore[attr-defined]
    edge_kinds = {k for _u, _v, k in inner.edges(keys=True)}
    assert "defines" in edge_kinds
    assert "contained_in" in edge_kinds

    contained = [(u, v) for u, v, k in inner.edges(keys=True) if k == "contained_in"]
    assert any(u[1] == "Greeter" and v[1] == "greet" for u, v in contained)


def test_index_repo_embed_dimension_mismatch_raises(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=BadDimEmbedder(dim=4),
    )
    with pytest.raises(IndexingError):
        indexer.index_repo(tmp_path, "repo1")


def test_index_repo_embed_batch_len_mismatch_raises(
    tmp_path: Path,
    stores: tuple[MetadataStore, LexicalStore, LanceVectorStore, SymbolGraph],
) -> None:
    meta, lex, vec, graph = stores
    _build_repo(tmp_path)
    indexer = Indexer(
        metadata_store=meta,
        lexical_store=lex,
        vector_store=vec,
        symbol_graph=graph,
        embed=BadCountEmbedder(dim=4),
    )
    with pytest.raises(IndexingError):
        indexer.index_repo(tmp_path, "repo1")
