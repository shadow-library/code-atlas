"""Project ``CodeChunk`` records into ``Citation`` records for ``Answer`` output."""

from __future__ import annotations

from code_atlas.domain.answer import Citation
from code_atlas.domain.chunk import CodeChunk

__all__ = ["DEFAULT_SNIPPET_MAX_CHARS", "to_citation"]

DEFAULT_SNIPPET_MAX_CHARS = 800


def to_citation(chunk: CodeChunk, *, max_snippet_chars: int = DEFAULT_SNIPPET_MAX_CHARS) -> Citation:
    """Project a CodeChunk into a Citation for inclusion in an Answer.

    The snippet is a raw prefix of ``chunk.content`` (no trailing ellipsis) so
    downstream tools can re-grep it exactly against the source file.
    """
    if max_snippet_chars < 0:
        raise ValueError(f"max_snippet_chars must be >= 0, got {max_snippet_chars}")

    snippet = chunk.content[:max_snippet_chars]

    return Citation(
        path=chunk.path,
        start_line=chunk.start_line,
        end_line=chunk.end_line,
        symbol=chunk.symbol,
        snippet=snippet,
    )
