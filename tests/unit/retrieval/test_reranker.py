"""Unit tests for PassthroughReranker."""

from __future__ import annotations

import pytest

from code_atlas.domain.chunk import CodeChunk
from code_atlas.domain.retrieval import RetrievalResult
from code_atlas.retrieval.reranker import PassthroughReranker


def _chunk(cid: str) -> CodeChunk:
    return CodeChunk(
        chunk_id=cid,
        repo_id="r",
        path="p.py",
        language="python",
        kind="function",
        symbol=cid,
        start_line=1,
        end_line=2,
        content="x",
        content_hash="h" * 16,
    )


def _result(cid: str, *, score: float = 1.0) -> RetrievalResult:
    return RetrievalResult(chunk=_chunk(cid), score=score, source="fused")


@pytest.mark.asyncio
async def test_passthrough_preserves_order() -> None:
    inputs = [_result("a"), _result("b"), _result("c")]

    out = await PassthroughReranker().rerank("query", inputs)

    assert [r.chunk.chunk_id for r in out] == ["a", "b", "c"]


@pytest.mark.asyncio
async def test_passthrough_returns_copy_not_same_list() -> None:
    inputs = [_result("a"), _result("b")]

    out = await PassthroughReranker().rerank("query", inputs)

    assert out is not inputs
    assert out == inputs


@pytest.mark.asyncio
async def test_passthrough_returns_empty_for_empty_input() -> None:
    assert await PassthroughReranker().rerank("q", []) == []


@pytest.mark.asyncio
async def test_passthrough_ignores_query() -> None:
    inputs = [_result("a"), _result("b")]
    reranker = PassthroughReranker()

    assert await reranker.rerank("q1", inputs) == await reranker.rerank("q2", inputs)
