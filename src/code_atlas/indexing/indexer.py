"""Indexer orchestrator: composes ingest_repo with metadata/lexical/vector/graph stores."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from code_atlas.domain.chunk import CodeChunk
from code_atlas.errors import IndexingError
from code_atlas.indexing.edge_extractor import extract_python_edges
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.indexing.vector_store import VectorItem, VectorStore
from code_atlas.ingestion.pipeline import IngestStats, ingest_repo
from code_atlas.utils import get_logger

__all__ = ["EmbedFunc", "IndexResult", "Indexer"]

log = get_logger(__name__)

EmbedFunc = Callable[[list[str]], list[list[float]]]


@dataclass(slots=True)
class IndexResult:
    """Counters returned by ``Indexer.index_repo``."""

    chunks_seen: int = 0
    chunks_indexed: int = 0
    chunks_skipped_cached: int = 0
    embed_batches: int = 0
    embed_calls: int = 0
    edges_added: int = 0
    ingest_stats: IngestStats = field(default_factory=IngestStats)


class Indexer:
    """Single-pass batched orchestrator over the four indexing stores."""

    def __init__(
        self,
        *,
        metadata_store: MetadataStore,
        lexical_store: LexicalStore,
        vector_store: VectorStore,
        symbol_graph: SymbolGraph,
        embed: EmbedFunc,
        batch_size: int = 64,
    ) -> None:
        if batch_size < 1:
            raise IndexingError("batch_size must be >= 1", context={"batch_size": batch_size})
        self._metadata = metadata_store
        self._lexical = lexical_store
        self._vector = vector_store
        self._graph = symbol_graph
        self._embed = embed
        self._batch_size = batch_size

    def index_repo(
        self,
        root: Path,
        repo_id: str,
        *,
        extra_ignores: list[str] | None = None,
        max_chunk_lines: int = 200,
        mtime_cache: dict[str, tuple[float, int]] | None = None,
    ) -> IndexResult:
        result = IndexResult(ingest_stats=IngestStats())
        log.info("indexer.index_repo.start", repo_id=repo_id, root=str(root), batch_size=self._batch_size)

        per_file: dict[str, list[CodeChunk]] = {}
        batch: list[CodeChunk] = []

        for chunk in ingest_repo(
            root,
            repo_id,
            extra_ignores=extra_ignores,
            max_chunk_lines=max_chunk_lines,
            mtime_cache=mtime_cache,
            stats=result.ingest_stats,
        ):
            result.chunks_seen += 1
            per_file.setdefault(chunk.path, []).append(chunk)
            batch.append(chunk)
            if len(batch) >= self._batch_size:
                self._flush(batch, result)
                batch = []

        if batch:
            self._flush(batch, result)

        self._build_symbol_graph(per_file, result)

        log.info(
            "indexer.index_repo.completed",
            repo_id=repo_id,
            chunks_seen=result.chunks_seen,
            chunks_indexed=result.chunks_indexed,
            chunks_skipped_cached=result.chunks_skipped_cached,
            embed_batches=result.embed_batches,
            embed_calls=result.embed_calls,
            edges_added=result.edges_added,
        )
        return result

    def _flush(self, batch: list[CodeChunk], result: IndexResult) -> None:
        existing_rows = self._metadata.get_many([c.chunk_id for c in batch])
        existing: dict[str, str] = {row.chunk_id: row.content_hash for row in existing_rows}

        to_index: list[CodeChunk] = []
        cached = 0
        for chunk in batch:
            if existing.get(chunk.chunk_id) == chunk.content_hash:
                cached += 1
            else:
                to_index.append(chunk)

        result.chunks_skipped_cached += cached

        if not to_index:
            log.info("indexer.batch.flushed", to_index=0, cached=cached)
            return

        vectors = self._embed_texts([c.content for c in to_index])
        self._validate_vectors(vectors, expected_count=len(to_index))

        result.embed_batches += 1
        result.embed_calls += len(to_index)

        self._metadata.upsert_many(to_index)
        self._lexical.upsert_many(to_index)
        items = [
            VectorItem(
                chunk_id=chunk.chunk_id,
                repo_id=chunk.repo_id,
                vector=vec,
                metadata={"path": chunk.path, "language": chunk.language, "kind": chunk.kind},
            )
            for chunk, vec in zip(to_index, vectors, strict=True)
        ]
        self._vector.upsert(items)

        result.chunks_indexed += len(to_index)
        log.info("indexer.batch.flushed", to_index=len(to_index), cached=cached)

    def _embed_texts(self, texts: list[str]) -> list[list[float]]:
        try:
            return self._embed(texts)
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("indexer.embed_failed", batch_size=len(texts), error=str(exc))
            raise IndexingError(
                "indexer: embed failed",
                context={"batch_size": len(texts)},
            ) from exc

    def _validate_vectors(self, vectors: list[list[float]], *, expected_count: int) -> None:
        if len(vectors) != expected_count:
            log.warning(
                "indexer.embed_count_mismatch",
                expected=expected_count,
                got=len(vectors),
            )
            raise IndexingError(
                "indexer: embed returned wrong number of vectors",
                context={"expected": expected_count, "got": len(vectors)},
            )
        dim = self._vector.dimension
        for idx, vec in enumerate(vectors):
            if len(vec) != dim:
                log.warning(
                    "indexer.embed_dim_mismatch",
                    expected=dim,
                    got=len(vec),
                    index=idx,
                )
                raise IndexingError(
                    "indexer: embedding dimension mismatch",
                    context={"expected": dim, "got": len(vec), "index": idx},
                )

    def _build_symbol_graph(self, per_file: dict[str, list[CodeChunk]], result: IndexResult) -> None:
        files_processed = 0
        for chunks in per_file.values():
            if not chunks or chunks[0].language != "python":
                continue
            edges = extract_python_edges(chunks)
            files_processed += 1
            for src, dst, kind in edges:
                self._graph.add_edge(src, dst, kind)
                result.edges_added += 1
        log.info(
            "indexer.symbol_graph.built",
            files=files_processed,
            edges=result.edges_added,
        )
