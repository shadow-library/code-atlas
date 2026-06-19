"""Reranker Protocol and a passthrough default implementation."""

from __future__ import annotations

from typing import Protocol

from code_atlas.domain.retrieval import RetrievalResult

__all__ = ["PassthroughReranker", "Reranker"]


class Reranker(Protocol):
    """Re-score retrieved results, possibly via an LLM or cross-encoder."""

    async def rerank(self, query: str, results: list[RetrievalResult]) -> list[RetrievalResult]: ...


class PassthroughReranker:
    """No-op reranker. Returns results in the same order it received them.

    Returns a shallow copy so callers may safely mutate the result without
    affecting the input list.
    """

    async def rerank(self, query: str, results: list[RetrievalResult]) -> list[RetrievalResult]:
        del query
        return list(results)
