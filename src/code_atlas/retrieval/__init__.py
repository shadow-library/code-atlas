"""Retrieval subsystem: hybrid (vector + lexical) search with RRF fusion."""

from __future__ import annotations

from code_atlas.retrieval.citation import DEFAULT_SNIPPET_MAX_CHARS, to_citation
from code_atlas.retrieval.hybrid import RRF_K_DEFAULT, HybridRetriever
from code_atlas.retrieval.reranker import PassthroughReranker, Reranker

__all__ = [
    "DEFAULT_SNIPPET_MAX_CHARS",
    "RRF_K_DEFAULT",
    "HybridRetriever",
    "PassthroughReranker",
    "Reranker",
    "to_citation",
]
