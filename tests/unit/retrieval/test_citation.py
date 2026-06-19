"""Unit tests for to_citation."""

from __future__ import annotations

import pytest

from code_atlas.domain.chunk import CodeChunk
from code_atlas.retrieval.citation import DEFAULT_SNIPPET_MAX_CHARS, to_citation


def _chunk(
    cid: str = "c1",
    *,
    path: str = "p.py",
    symbol: str | None = "foo",
    start: int = 1,
    end: int = 2,
    content: str = "x",
) -> CodeChunk:
    return CodeChunk(
        chunk_id=cid,
        repo_id="r",
        path=path,
        language="python",
        kind="function",
        symbol=symbol,
        start_line=start,
        end_line=end,
        content=content,
        content_hash="h" * 16,
    )


def test_to_citation_extracts_basic_fields() -> None:
    chunk = _chunk(symbol="foo", start=10, end=25, path="src/a.py", content="body")

    citation = to_citation(chunk)

    assert citation.path == "src/a.py"
    assert citation.start_line == 10
    assert citation.end_line == 25
    assert citation.symbol == "foo"
    assert citation.snippet == "body"


def test_to_citation_with_none_symbol() -> None:
    citation = to_citation(_chunk(symbol=None))

    assert citation.symbol is None


def test_to_citation_snippet_truncation() -> None:
    content = "a" * 2000
    citation = to_citation(_chunk(content=content), max_snippet_chars=100)

    assert len(citation.snippet) == 100
    assert citation.snippet == content[:100]


def test_to_citation_full_content_when_under_limit() -> None:
    content = "b" * 50
    citation = to_citation(_chunk(content=content), max_snippet_chars=DEFAULT_SNIPPET_MAX_CHARS)

    assert citation.snippet == content


def test_to_citation_zero_max_chars_yields_empty_snippet() -> None:
    assert to_citation(_chunk(content="abc"), max_snippet_chars=0).snippet == ""


def test_to_citation_negative_max_chars_raises() -> None:
    with pytest.raises(ValueError, match="max_snippet_chars must be >= 0"):
        to_citation(_chunk(), max_snippet_chars=-1)
