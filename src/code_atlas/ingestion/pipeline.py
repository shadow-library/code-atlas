"""Compose walker, language detection, and parser into a lazy chunk stream."""

from __future__ import annotations

import os
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

from code_atlas.domain.chunk import CodeChunk
from code_atlas.errors import IngestionError
from code_atlas.ingestion.language import detect_language
from code_atlas.ingestion.parser import chunk_file
from code_atlas.ingestion.walker import walk_repo
from code_atlas.utils import get_logger

__all__ = ["IngestStats", "ingest_repo"]

log = get_logger(__name__)

_FileStamp = tuple[float, int]  # (mtime, size)


@dataclass(slots=True)
class IngestStats:
    """Counters updated by ``ingest_repo``; mutated in place."""

    files_seen: int = 0
    files_skipped_no_language: int = 0
    files_skipped_unreadable: int = 0
    files_skipped_unchanged: int = 0
    files_chunked: int = 0
    chunks_emitted: int = 0


def _resolve_root(root: Path) -> Path:
    resolved = root.resolve()
    if not resolved.is_dir():
        raise IngestionError("ingest_repo root is not a directory", context={"root": str(resolved)})
    return resolved


def _stamp(p: Path) -> _FileStamp:
    st = os.stat(p)
    return (st.st_mtime, st.st_size)


def ingest_repo(
    root: Path,
    repo_id: str,
    *,
    extra_ignores: list[str] | None = None,
    max_chunk_lines: int = 200,
    mtime_cache: dict[str, _FileStamp] | None = None,
    stats: IngestStats | None = None,
) -> Iterator[CodeChunk]:
    """Walk ``root``, detect languages, chunk eligible files, and yield ``CodeChunk``s lazily."""
    if not repo_id or not repo_id.strip():
        raise IngestionError("ingest_repo requires non-empty repo_id")
    resolved_root = _resolve_root(root)
    counters = stats if stats is not None else IngestStats()
    return _iter(
        root=resolved_root,
        repo_id=repo_id,
        extra_ignores=extra_ignores,
        max_chunk_lines=max_chunk_lines,
        mtime_cache=mtime_cache,
        stats=counters,
    )


def _iter(
    *,
    root: Path,
    repo_id: str,
    extra_ignores: list[str] | None,
    max_chunk_lines: int,
    mtime_cache: dict[str, _FileStamp] | None,
    stats: IngestStats,
) -> Iterator[CodeChunk]:
    for p in walk_repo(root, extra_ignores=extra_ignores):
        stats.files_seen += 1
        rel = p.relative_to(root).as_posix()

        try:
            current_stamp = _stamp(p)
        except OSError as exc:
            stats.files_skipped_unreadable += 1
            log.warning("pipeline.stat_failed", path=rel, error=str(exc))
            continue

        if mtime_cache is not None and mtime_cache.get(rel) == current_stamp:
            stats.files_skipped_unchanged += 1
            log.debug("pipeline.skip_unchanged", path=rel)
            continue

        language = detect_language(p)
        if language is None:
            stats.files_skipped_no_language += 1
            log.debug("pipeline.skip_no_language", path=rel)
            continue

        try:
            content = p.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            stats.files_skipped_unreadable += 1
            log.warning("pipeline.read_failed", path=rel, error=str(exc))
            continue

        if not content.strip():
            if mtime_cache is not None:
                mtime_cache[rel] = current_stamp
            continue

        chunks = chunk_file(
            path=rel,
            repo_id=repo_id,
            language=language,
            content=content,
            max_chunk_lines=max_chunk_lines,
        )
        if chunks:
            stats.files_chunked += 1
            for chunk in chunks:
                stats.chunks_emitted += 1
                yield chunk
        if mtime_cache is not None:
            mtime_cache[rel] = current_stamp

    log.info(
        "pipeline.completed",
        files_seen=stats.files_seen,
        files_chunked=stats.files_chunked,
        chunks_emitted=stats.chunks_emitted,
        files_skipped_no_language=stats.files_skipped_no_language,
        files_skipped_unreadable=stats.files_skipped_unreadable,
        files_skipped_unchanged=stats.files_skipped_unchanged,
    )
