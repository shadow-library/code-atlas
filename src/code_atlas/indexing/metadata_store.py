"""SQLite-backed canonical store of ``CodeChunk`` rows keyed by ``chunk_id``."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from datetime import UTC, datetime
from types import TracebackType
from typing import Any

from sqlalchemy import (
    Column,
    Engine,
    Integer,
    MetaData,
    String,
    Table,
    create_engine,
    delete,
    func,
    select,
)
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.exc import SQLAlchemyError

from code_atlas.domain.chunk import CodeChunk
from code_atlas.errors import IndexingError
from code_atlas.utils import get_logger

__all__ = ["MetadataStore"]

log = get_logger(__name__)

_METADATA = MetaData()
_CHUNKS = Table(
    "chunks",
    _METADATA,
    Column("chunk_id", String, primary_key=True),
    Column("repo_id", String, nullable=False, index=True),
    Column("path", String, nullable=False),
    Column("language", String, nullable=False),
    Column("kind", String, nullable=False),
    Column("symbol", String, nullable=True),
    Column("start_line", Integer, nullable=False),
    Column("end_line", Integer, nullable=False),
    Column("content", String, nullable=False),
    Column("content_hash", String, nullable=False, index=True),
    Column("indexed_at", String, nullable=False),
)


def _row_to_chunk(row: Mapping[str, Any]) -> CodeChunk:
    data = {k: v for k, v in row.items() if k != "indexed_at"}
    return CodeChunk(**data)


class MetadataStore:
    """SQLAlchemy Core wrapper around the canonical ``chunks`` table."""

    def __init__(self, url: str = "sqlite:///:memory:") -> None:
        self._engine: Engine = create_engine(url, future=True)
        _METADATA.create_all(self._engine)

    @property
    def engine(self) -> Engine:
        return self._engine

    def upsert(self, chunk: CodeChunk) -> None:
        vals = chunk.model_dump()
        vals["indexed_at"] = datetime.now(UTC).isoformat()
        stmt = insert(_CHUNKS).values(**vals)
        stmt = stmt.on_conflict_do_update(
            index_elements=["chunk_id"],
            set_={k: v for k, v in vals.items() if k != "chunk_id"},
        )
        try:
            with self._engine.begin() as conn:
                conn.execute(stmt)
        except SQLAlchemyError as exc:
            log.warning("metadata_store.upsert_failed", chunk_id=chunk.chunk_id, error=str(exc))
            raise IndexingError("metadata upsert failed", context={"chunk_id": chunk.chunk_id}) from exc
        log.debug("metadata_store.upsert", chunk_id=chunk.chunk_id)

    def upsert_many(self, chunks: Iterable[CodeChunk]) -> int:
        items = list(chunks)
        if not items:
            return 0
        now = datetime.now(UTC).isoformat()
        rows: list[dict[str, Any]] = []
        for chunk in items:
            vals = chunk.model_dump()
            vals["indexed_at"] = now
            rows.append(vals)
        try:
            with self._engine.begin() as conn:
                for vals in rows:
                    stmt = insert(_CHUNKS).values(**vals)
                    stmt = stmt.on_conflict_do_update(
                        index_elements=["chunk_id"],
                        set_={k: v for k, v in vals.items() if k != "chunk_id"},
                    )
                    conn.execute(stmt)
        except SQLAlchemyError as exc:
            log.warning("metadata_store.upsert_many_failed", count=len(rows), error=str(exc))
            raise IndexingError("metadata upsert failed", context={"count": len(rows)}) from exc
        log.debug("metadata_store.upsert_many", count=len(rows))
        return len(rows)

    def get(self, chunk_id: str) -> CodeChunk | None:
        with self._engine.connect() as conn:
            row = conn.execute(select(_CHUNKS).where(_CHUNKS.c.chunk_id == chunk_id)).mappings().first()
        if row is None:
            return None
        return _row_to_chunk(dict(row))

    def get_many(self, chunk_ids: list[str]) -> list[CodeChunk]:
        if not chunk_ids:
            return []
        with self._engine.connect() as conn:
            result = conn.execute(select(_CHUNKS).where(_CHUNKS.c.chunk_id.in_(chunk_ids))).mappings().all()
        by_id: dict[str, dict[str, Any]] = {row["chunk_id"]: dict(row) for row in result}
        return [_row_to_chunk(by_id[cid]) for cid in chunk_ids if cid in by_id]

    def delete_repo(self, repo_id: str) -> int:
        with self._engine.begin() as conn:
            result = conn.execute(delete(_CHUNKS).where(_CHUNKS.c.repo_id == repo_id))
        deleted = int(result.rowcount or 0)
        log.debug("metadata_store.delete_repo", repo_id=repo_id, deleted=deleted)
        return deleted

    def count(self, *, repo_id: str | None = None) -> int:
        stmt = select(func.count()).select_from(_CHUNKS)
        if repo_id is not None:
            stmt = stmt.where(_CHUNKS.c.repo_id == repo_id)
        with self._engine.connect() as conn:
            value = conn.execute(stmt).scalar_one()
        return int(value)

    def find_by_path(self, repo_id: str, path: str) -> list[CodeChunk]:
        """Return all chunks in ``repo_id`` whose ``path`` matches, ordered by start_line ASC."""
        with self._engine.connect() as conn:
            result = (
                conn.execute(
                    select(_CHUNKS)
                    .where((_CHUNKS.c.repo_id == repo_id) & (_CHUNKS.c.path == path))
                    .order_by(_CHUNKS.c.start_line.asc())
                )
                .mappings()
                .all()
            )
        return [_row_to_chunk(dict(row)) for row in result]

    def close(self) -> None:
        self._engine.dispose()

    def __enter__(self) -> MetadataStore:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.close()
