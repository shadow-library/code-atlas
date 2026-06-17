"""SQLite FTS5-backed lexical store for ``CodeChunk`` full-text search."""

from __future__ import annotations

import sqlite3
from collections.abc import Iterable
from types import TracebackType

from code_atlas.domain.chunk import CodeChunk
from code_atlas.errors import IndexingError
from code_atlas.utils import get_logger

__all__ = ["LexicalStore"]

log = get_logger(__name__)

_CREATE_SQL = """
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
  content,
  symbol,
  repo_id UNINDEXED,
  chunk_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);
"""


class LexicalStore:
    """FTS5 index over chunk ``content`` and ``symbol`` with BM25 ranking.

    The ``url`` argument is a raw SQLite filename (or ``":memory:"``), NOT a
    SQLAlchemy URL — this store uses stdlib ``sqlite3`` because FTS5 virtual
    tables and ``bm25()`` don't fit SQLAlchemy's schema model cleanly. Contrast
    with ``MetadataStore`` which accepts ``sqlite:///...`` SQLAlchemy URLs.
    """

    def __init__(self, url: str = ":memory:") -> None:
        try:
            self._conn = sqlite3.connect(url, check_same_thread=False)
            with self._conn:
                self._conn.execute(_CREATE_SQL)
        except sqlite3.Error as exc:
            log.warning("lexical_store.init_failed", url=url, error=str(exc))
            raise IndexingError("lexical store init failed", context={"url": url}) from exc

    @property
    def connection(self) -> sqlite3.Connection:
        return self._conn

    def upsert(self, chunk: CodeChunk) -> None:
        try:
            with self._conn:
                self._conn.execute("DELETE FROM chunks_fts WHERE chunk_id = ?", (chunk.chunk_id,))
                self._conn.execute(
                    "INSERT INTO chunks_fts (content, symbol, repo_id, chunk_id) VALUES (?, ?, ?, ?)",
                    (chunk.content, chunk.symbol or "", chunk.repo_id, chunk.chunk_id),
                )
        except sqlite3.Error as exc:
            log.warning("lexical_store.upsert_failed", chunk_id=chunk.chunk_id, error=str(exc))
            raise IndexingError("lexical upsert failed", context={"chunk_id": chunk.chunk_id}) from exc

    def upsert_many(self, chunks: Iterable[CodeChunk]) -> int:
        count = 0
        try:
            with self._conn:
                for chunk in chunks:
                    self._conn.execute("DELETE FROM chunks_fts WHERE chunk_id = ?", (chunk.chunk_id,))
                    self._conn.execute(
                        "INSERT INTO chunks_fts (content, symbol, repo_id, chunk_id) VALUES (?, ?, ?, ?)",
                        (chunk.content, chunk.symbol or "", chunk.repo_id, chunk.chunk_id),
                    )
                    count += 1
        except sqlite3.Error as exc:
            log.warning("lexical_store.upsert_many_failed", error=str(exc))
            raise IndexingError("lexical upsert_many failed", context={"inserted": count}) from exc
        return count

    def search(self, query: str, k: int = 10, repo_id: str | None = None) -> list[tuple[str, float]]:
        """Run BM25 FTS5 search.

        SQLite's ``bm25(table)`` returns negative values where lower = better
        match. We negate it so callers see higher score = more relevant; rows
        are ordered by raw ``bm25`` ascending (best first).
        """
        if k < 1:
            raise IndexingError("k must be >= 1", context={"k": k})
        sql = "SELECT chunk_id, -bm25(chunks_fts) AS score FROM chunks_fts WHERE chunks_fts MATCH ?"
        params: list[object] = [query]
        if repo_id is not None:
            sql += " AND repo_id = ?"
            params.append(repo_id)
        sql += " ORDER BY bm25(chunks_fts) ASC LIMIT ?"
        params.append(k)
        try:
            cur = self._conn.execute(sql, params)
            rows = cur.fetchall()
        except sqlite3.Error as exc:
            log.warning("lexical_store.search_failed", query=query, error=str(exc))
            raise IndexingError("lexical search failed", context={"query": query}) from exc
        return [(str(cid), float(score)) for cid, score in rows]

    def delete_repo(self, repo_id: str) -> int:
        try:
            with self._conn:
                cur = self._conn.execute("DELETE FROM chunks_fts WHERE repo_id = ?", (repo_id,))
                return int(cur.rowcount)
        except sqlite3.Error as exc:
            log.warning("lexical_store.delete_repo_failed", repo_id=repo_id, error=str(exc))
            raise IndexingError("lexical delete_repo failed", context={"repo_id": repo_id}) from exc

    def count(self, *, repo_id: str | None = None) -> int:
        sql = "SELECT COUNT(*) FROM chunks_fts"
        params: tuple[object, ...] = ()
        if repo_id is not None:
            sql += " WHERE repo_id = ?"
            params = (repo_id,)
        try:
            cur = self._conn.execute(sql, params)
            row = cur.fetchone()
        except sqlite3.Error as exc:
            log.warning("lexical_store.count_failed", repo_id=repo_id, error=str(exc))
            raise IndexingError("lexical count failed", context={"repo_id": repo_id}) from exc
        return int(row[0]) if row else 0

    def close(self) -> None:
        try:
            self._conn.close()
        except sqlite3.Error as exc:
            log.warning("lexical_store.close_failed", error=str(exc))
            raise IndexingError("lexical close failed", context={}) from exc

    def __enter__(self) -> LexicalStore:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        self.close()
