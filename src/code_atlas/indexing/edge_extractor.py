"""Derive symbol-graph edges from CodeChunk metadata (no re-parsing)."""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import PurePosixPath

from code_atlas.domain.chunk import CodeChunk, Symbol
from code_atlas.indexing.symbol_graph import EdgeKind

__all__ = ["extract_python_edges"]


def _module_symbol(path: str) -> Symbol:
    stem = PurePosixPath(path).stem or path
    return Symbol(name=stem, kind="module", path=path, line=1)


def _chunk_symbol_kind(chunk: CodeChunk) -> str | None:
    if chunk.kind == "class":
        return "class"
    if chunk.kind == "function":
        return "function"
    if chunk.kind == "method":
        return "method"
    return None


def extract_python_edges(chunks: Iterable[CodeChunk]) -> list[tuple[Symbol, Symbol, EdgeKind]]:
    """Return ``defines`` + ``contained_in`` edges for a single Python file's chunks.

    Skips chunks without symbol names. Non-python inputs yield ``[]``.
    """
    items = [c for c in chunks if c.symbol]
    if not items:
        return []
    if any(c.language != "python" for c in items):
        return []

    path = items[0].path
    module = _module_symbol(path)

    edges: list[tuple[Symbol, Symbol, EdgeKind]] = []

    classes: list[tuple[CodeChunk, Symbol]] = []
    for c in items:
        if c.kind in {"class", "function"}:
            sym_kind = _chunk_symbol_kind(c)
            if sym_kind is None:
                continue
            dst = Symbol(
                name=c.symbol or "",
                kind=sym_kind,  # type: ignore[arg-type]
                path=c.path,
                line=c.start_line,
            )
            edges.append((module, dst, "defines"))
            if c.kind == "class":
                classes.append((c, dst))

    for c in items:
        if c.kind != "method":
            continue
        method_sym = Symbol(
            name=c.symbol or "",
            kind="method",
            path=c.path,
            line=c.start_line,
            parent=None,
        )
        enclosing: tuple[CodeChunk, Symbol] | None = None
        for cls_chunk, cls_sym in classes:
            if (
                cls_chunk.start_line <= c.start_line
                and c.end_line <= cls_chunk.end_line
                and (enclosing is None or cls_chunk.start_line > enclosing[0].start_line)
            ):
                enclosing = (cls_chunk, cls_sym)
        if enclosing is not None:
            edges.append((enclosing[1], method_sym, "contained_in"))

    return edges
