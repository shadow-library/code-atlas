"""Unit tests for citation grounding against a real in-memory MetadataStore."""

from __future__ import annotations

from collections.abc import Iterator

import pytest

from code_atlas.domain.answer import Answer, Citation
from code_atlas.domain.chunk import CodeChunk
from code_atlas.evaluation import check_grounding
from code_atlas.indexing.metadata_store import MetadataStore


@pytest.fixture
def store() -> Iterator[MetadataStore]:
    s = MetadataStore("sqlite:///:memory:")
    s.upsert(
        CodeChunk(
            chunk_id="c1",
            repo_id="r",
            path="a.py",
            language="python",
            kind="function",
            symbol="foo",
            start_line=1,
            end_line=10,
            content="def foo():\n    return 1\n",
            content_hash="h" * 16,
        )
    )
    yield s
    s.close()


def test_fully_grounded_all_green(store: MetadataStore) -> None:
    answer = Answer(text="answer", citations=[Citation(path="a.py", start_line=1, end_line=10, snippet="def foo()")])
    report = check_grounding(answer, store, repo_id="r")
    assert report.total == 1
    assert report.grounded == 1
    assert report.ungrounded_citations == []
    assert report.is_fully_grounded is True


def test_fabricated_file_flagged(store: MetadataStore) -> None:
    answer = Answer(text="answer", citations=[Citation(path="ghost.py", start_line=1, end_line=2, snippet="")])
    report = check_grounding(answer, store, repo_id="r")
    assert report.grounded == 0
    assert len(report.ungrounded_citations) == 1
    flagged = report.ungrounded_citations[0]
    assert flagged.file_exists is False
    assert "file not indexed" in flagged.reasons


def test_out_of_range_line_flagged(store: MetadataStore) -> None:
    answer = Answer(text="answer", citations=[Citation(path="a.py", start_line=50, end_line=60, snippet="")])
    report = check_grounding(answer, store, repo_id="r")
    assert report.grounded == 0
    flagged = report.ungrounded_citations[0]
    assert flagged.line_range_valid is False
    assert "line range outside known chunks" in flagged.reasons


def test_wrong_snippet_flagged(store: MetadataStore) -> None:
    answer = Answer(text="answer", citations=[Citation(path="a.py", start_line=1, end_line=10, snippet="def bar()")])
    report = check_grounding(answer, store, repo_id="r")
    assert report.grounded == 0
    flagged = report.ungrounded_citations[0]
    assert flagged.file_exists is True
    assert flagged.line_range_valid is True
    assert flagged.snippet_present is False
    assert "snippet not found in cited chunk" in flagged.reasons


def test_empty_snippet_is_vacuously_present(store: MetadataStore) -> None:
    answer = Answer(text="answer", citations=[Citation(path="a.py", start_line=1, end_line=10, snippet="")])
    report = check_grounding(answer, store, repo_id="r")
    assert report.grounded == 1
    assert report.is_fully_grounded is True


def test_empty_citations_is_fully_grounded(store: MetadataStore) -> None:
    report = check_grounding(Answer(text="x"), store, repo_id="r")
    assert report.total == 0
    assert report.grounded == 0
    assert report.is_fully_grounded is True


def test_mixed_counts(store: MetadataStore) -> None:
    answer = Answer(
        text="answer",
        citations=[
            Citation(path="a.py", start_line=1, end_line=10, snippet="def foo()"),
            Citation(path="ghost.py", start_line=1, end_line=2, snippet=""),
        ],
    )
    report = check_grounding(answer, store, repo_id="r")
    assert report.total == 2
    assert report.grounded == 1
    assert len(report.ungrounded_citations) == 1
