"""Vector store: VectorStore Protocol and LanceDB-backed implementation."""

from __future__ import annotations

import importlib
import json
from collections.abc import Iterable
from types import TracebackType
from typing import Any, Protocol, cast

from pydantic import BaseModel, ConfigDict, Field

from code_atlas.errors import IndexingError
from code_atlas.utils import get_logger

log = get_logger(__name__)

_TABLE_NAME_DEFAULT = "chunks_vec"


class VectorItem(BaseModel):
    """Persistence record for a single vector row."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    chunk_id: str = Field(min_length=1)
    repo_id: str = Field(min_length=1)
    vector: list[float] = Field(min_length=1)
    metadata: dict[str, Any] = Field(default_factory=dict)


class VectorStore(Protocol):
    """Protocol every vector backend must satisfy."""

    dimension: int

    def upsert(self, items: Iterable[VectorItem]) -> int: ...

    def search(
        self,
        vector: list[float],
        k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> list[tuple[str, float]]: ...

    def delete_repo(self, repo_id: str) -> int: ...

    def count(self, *, repo_id: str | None = None) -> int: ...

    def close(self) -> None: ...


def _reject_quote(value: str, *, field: str) -> None:
    if "'" in value:
        raise IndexingError(
            "single-quote not allowed in filter value",
            context={"field": field, "value": value},
        )


class LanceVectorStore:
    """LanceDB-backed VectorStore (embedded, local file ops, sync)."""

    def __init__(
        self,
        uri: str,
        *,
        table_name: str = _TABLE_NAME_DEFAULT,
        dimension: int = 768,
    ) -> None:
        if dimension < 1:
            raise IndexingError("dimension must be >= 1", context={"dimension": dimension})

        self.dimension = dimension
        self._uri = uri
        self._table_name = table_name

        lancedb = importlib.import_module("lancedb")
        pa = importlib.import_module("pyarrow")

        self._pa = pa
        self._schema = pa.schema(
            [
                pa.field("chunk_id", pa.string()),
                pa.field("repo_id", pa.string()),
                pa.field("vector", pa.list_(pa.float32(), list_size=dimension)),
                pa.field("metadata", pa.string()),
            ]
        )

        try:
            self._db: Any = lancedb.connect(uri)
            existing = set(self._db.table_names())
            if table_name in existing:
                self._tbl: Any = self._db.open_table(table_name)
            else:
                self._tbl = self._db.create_table(table_name, schema=self._schema)
        except Exception as exc:
            log.warning("vector_store.init_failed", uri=uri, error=str(exc))
            raise IndexingError(
                "vector store init failed",
                context={"uri": uri, "table_name": table_name},
            ) from exc

    def upsert(self, items: Iterable[VectorItem]) -> int:
        rows: list[dict[str, Any]] = []
        for item in items:
            if len(item.vector) != self.dimension:
                raise IndexingError(
                    "vector dimension mismatch",
                    context={
                        "chunk_id": item.chunk_id,
                        "got_dim": len(item.vector),
                        "expected_dim": self.dimension,
                    },
                )
            rows.append(
                {
                    "chunk_id": item.chunk_id,
                    "repo_id": item.repo_id,
                    "vector": [float(x) for x in item.vector],
                    "metadata": json.dumps(item.metadata, sort_keys=True),
                }
            )

        if not rows:
            return 0

        try:
            arrow_table = self._pa.Table.from_pylist(rows, schema=self._schema)
            (
                self._tbl.merge_insert("chunk_id")
                .when_matched_update_all()
                .when_not_matched_insert_all()
                .execute(arrow_table)
            )
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("vector_store.upsert_failed", count=len(rows), error=str(exc))
            raise IndexingError(
                "vector upsert failed",
                context={"count": len(rows)},
            ) from exc

        return len(rows)

    def search(
        self,
        vector: list[float],
        k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> list[tuple[str, float]]:
        if k < 1:
            raise IndexingError("k must be >= 1", context={"k": k})
        if len(vector) != self.dimension:
            raise IndexingError(
                "vector dimension mismatch",
                context={"got_dim": len(vector), "expected_dim": self.dimension},
            )

        where_clause: str | None = None
        if filters:
            unknown = [key for key in filters if key != "repo_id"]
            if unknown:
                raise IndexingError(
                    "unknown filter keys",
                    context={"unknown_filter_keys": sorted(unknown)},
                )
            repo_id = filters.get("repo_id")
            if repo_id is not None:
                if not isinstance(repo_id, str):
                    raise IndexingError(
                        "repo_id filter must be a string",
                        context={"type": type(repo_id).__name__},
                    )
                _reject_quote(repo_id, field="repo_id")
                where_clause = f"repo_id = '{repo_id}'"

        try:
            query = self._tbl.search(vector).metric("cosine").limit(k)
            if where_clause is not None:
                query = query.where(where_clause)
            arrow_result = query.to_arrow()
            chunk_ids = cast(list[str], arrow_result.column("chunk_id").to_pylist())
            distances = cast(list[float], arrow_result.column("_distance").to_pylist())
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("vector_store.search_failed", k=k, error=str(exc))
            raise IndexingError(
                "vector search failed",
                context={"k": k, "filters": filters},
            ) from exc

        return [(cid, 1.0 - float(dist)) for cid, dist in zip(chunk_ids, distances, strict=True)]

    def delete_repo(self, repo_id: str) -> int:
        _reject_quote(repo_id, field="repo_id")
        matched = self.count(repo_id=repo_id)
        if matched == 0:
            return 0
        try:
            self._tbl.delete(f"repo_id = '{repo_id}'")
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("vector_store.delete_failed", repo_id=repo_id, error=str(exc))
            raise IndexingError(
                "vector delete failed",
                context={"repo_id": repo_id},
            ) from exc
        return matched

    def count(self, *, repo_id: str | None = None) -> int:
        filter_clause: str | None = None
        if repo_id is not None:
            _reject_quote(repo_id, field="repo_id")
            filter_clause = f"repo_id = '{repo_id}'"

        try:
            if filter_clause is not None:
                return int(self._tbl.count_rows(filter=filter_clause))
            return int(self._tbl.count_rows())
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("vector_store.count_failed", repo_id=repo_id, error=str(exc))
            try:
                df = self._tbl.to_pandas()
                if repo_id is None:
                    return len(df)
                return int((df["repo_id"] == repo_id).sum())
            except Exception as exc2:
                raise IndexingError(
                    "vector count failed",
                    context={"repo_id": repo_id, "error": str(exc)},
                ) from exc2

    def close(self) -> None:
        self._tbl = None
        self._db = None

    def __enter__(self) -> LanceVectorStore:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        self.close()
