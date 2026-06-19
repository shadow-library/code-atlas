"""Hybrid retriever: parallel vector + lexical, RRF fusion, metadata hydration."""

from __future__ import annotations

import asyncio
from typing import Any

from code_atlas.domain.retrieval import RetrievalQuery, RetrievalResult
from code_atlas.errors import RetrievalError
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.vector_store import VectorStore
from code_atlas.providers.base import EmbeddingProvider
from code_atlas.utils import get_logger

__all__ = ["RRF_K_DEFAULT", "HybridRetriever"]

log = get_logger(__name__)

RRF_K_DEFAULT = 60
_KNOWN_FILTER_KEYS = frozenset({"repo_id"})


class HybridRetriever:
    """Hybrid retrieval: parallel vector + lexical, RRF fusion, metadata hydration."""

    def __init__(
        self,
        *,
        vector_store: VectorStore,
        lexical_store: LexicalStore,
        embedder: EmbeddingProvider,
        metadata_store: MetadataStore,
        rrf_k: int = RRF_K_DEFAULT,
        oversample: int = 2,
    ) -> None:
        if rrf_k < 1:
            raise RetrievalError("hybrid: rrf_k must be >= 1", context={"rrf_k": rrf_k})
        if oversample < 1:
            raise RetrievalError("hybrid: oversample must be >= 1", context={"oversample": oversample})
        self._vector = vector_store
        self._lexical = lexical_store
        self._embedder = embedder
        self._metadata = metadata_store
        self._rrf_k = rrf_k
        self._oversample = oversample

    async def retrieve(self, query: RetrievalQuery) -> list[RetrievalResult]:
        repo_id = self._extract_repo_id(query.filters)
        unknown = set(query.filters) - _KNOWN_FILTER_KEYS
        if unknown:
            log.debug("hybrid.unknown_filters", keys=sorted(unknown))

        oversampled_k = query.k * self._oversample
        vec_filters: dict[str, Any] | None = {"repo_id": repo_id} if repo_id else None

        log.info(
            "hybrid.retrieve",
            query_len=len(query.text),
            k=query.k,
            has_filters=bool(query.filters),
        )

        async def _vector_path() -> list[tuple[str, float]]:
            vectors = await self._embedder.embed([query.text])
            if not vectors:
                return []
            vec = vectors[0]
            return await asyncio.to_thread(self._vector.search, vec, oversampled_k, vec_filters)

        async def _lexical_path() -> list[tuple[str, float]]:
            return await asyncio.to_thread(self._lexical.search, query.text, oversampled_k, repo_id)

        vec_results, lex_results = await asyncio.gather(_vector_path(), _lexical_path())

        fused = _rrf_fuse(vec_results, lex_results, k=self._rrf_k)
        top_pairs = fused[: query.k]

        if not top_pairs:
            log.info("hybrid.fused", vec=len(vec_results), lex=len(lex_results), unique=0, returned=0)
            return []

        chunk_ids = [cid for cid, _ in top_pairs]
        chunks = await asyncio.to_thread(self._metadata.get_many, chunk_ids)
        chunks_by_id = {chunk.chunk_id: chunk for chunk in chunks}

        out: list[RetrievalResult] = []
        for cid, score in top_pairs:
            chunk = chunks_by_id.get(cid)
            if chunk is None:
                log.warning("hybrid.missing_metadata", chunk_id=cid)
                continue
            out.append(RetrievalResult(chunk=chunk, score=score, source="fused"))

        log.info(
            "hybrid.fused",
            vec=len(vec_results),
            lex=len(lex_results),
            unique=len(fused),
            returned=len(out),
        )
        return out

    @staticmethod
    def _extract_repo_id(filters: dict[str, Any]) -> str | None:
        raw = filters.get("repo_id")
        if raw is None:
            return None
        if not isinstance(raw, str):
            raise RetrievalError(
                "hybrid: filter type invalid",
                context={"field": "repo_id", "got_type": type(raw).__name__},
            )
        return raw


def _rrf_fuse(
    vec_results: list[tuple[str, float]],
    lex_results: list[tuple[str, float]],
    *,
    k: int,
) -> list[tuple[str, float]]:
    """Reciprocal rank fusion. Returns [(chunk_id, fused_score)] sorted desc."""
    scores: dict[str, float] = {}
    for rank, (cid, _) in enumerate(vec_results):
        scores[cid] = scores.get(cid, 0.0) + 1.0 / (k + rank + 1)
    for rank, (cid, _) in enumerate(lex_results):
        scores[cid] = scores.get(cid, 0.0) + 1.0 / (k + rank + 1)
    return sorted(scores.items(), key=lambda kv: kv[1], reverse=True)
