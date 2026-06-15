"""Tests for code_atlas.domain value objects."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from code_atlas.domain import (
    Answer,
    Citation,
    CodeChunk,
    RetrievalQuery,
    RetrievalResult,
    Symbol,
    TokenUsage,
)


def make_chunk(**overrides: object) -> CodeChunk:
    defaults: dict[str, object] = {
        "chunk_id": "c1",
        "repo_id": "repo1",
        "path": "src/x.py",
        "language": "python",
        "kind": "function",
        "symbol": "foo",
        "start_line": 1,
        "end_line": 10,
        "content": "def foo():\n    pass\n",
        "content_hash": "deadbeef",
    }
    defaults.update(overrides)
    return CodeChunk(**defaults)  # type: ignore[arg-type]


def test_symbol_round_trip() -> None:
    s = Symbol(name="foo", kind="function", path="src/x.py", line=3, parent=None)
    data = s.model_dump()
    assert Symbol.model_validate(data) == s


def test_symbol_rejects_empty_name() -> None:
    with pytest.raises(ValidationError):
        Symbol(name="", kind="function", path="src/x.py", line=1)


def test_chunk_round_trip_via_json() -> None:
    chunk = make_chunk()
    raw = chunk.model_dump_json()
    assert CodeChunk.model_validate_json(raw) == chunk


def test_chunk_rejects_negative_lines() -> None:
    with pytest.raises(ValidationError):
        make_chunk(start_line=0)
    with pytest.raises(ValidationError):
        make_chunk(start_line=-1)


def test_chunk_rejects_empty_content() -> None:
    with pytest.raises(ValidationError):
        make_chunk(content="")


def test_chunk_rejects_end_before_start() -> None:
    with pytest.raises(ValidationError) as exc:
        make_chunk(start_line=10, end_line=5)
    assert "end_line" in str(exc.value)


def test_chunk_is_frozen() -> None:
    chunk = make_chunk()
    with pytest.raises(ValidationError):
        chunk.path = "other.py"  # type: ignore[misc]


def test_chunk_rejects_extras() -> None:
    with pytest.raises(ValidationError):
        CodeChunk(  # type: ignore[call-arg]
            chunk_id="c1",
            repo_id="repo1",
            path="src/x.py",
            language="python",
            kind="function",
            start_line=1,
            end_line=2,
            content="x",
            content_hash="deadbeef",
            extra_field="x",
        )


def test_retrieval_query_defaults() -> None:
    q = RetrievalQuery(text="x")
    assert q.k == 10
    assert q.filters == {}


def test_retrieval_query_k_bounds() -> None:
    with pytest.raises(ValidationError):
        RetrievalQuery(text="x", k=0)
    with pytest.raises(ValidationError):
        RetrievalQuery(text="x", k=201)


def test_retrieval_query_text_required_non_empty() -> None:
    with pytest.raises(ValidationError):
        RetrievalQuery(text="")


def test_retrieval_result_round_trip() -> None:
    result = RetrievalResult(chunk=make_chunk(), score=0.42, source="vector")
    raw = result.model_dump_json()
    parsed = RetrievalResult.model_validate_json(raw)
    assert parsed == result
    assert parsed.score == 0.42


def test_retrieval_result_rejects_negative_score() -> None:
    with pytest.raises(ValidationError):
        RetrievalResult(chunk=make_chunk(), score=-0.1, source="vector")


def test_retrieval_result_invalid_source() -> None:
    with pytest.raises(ValidationError):
        RetrievalResult(chunk=make_chunk(), score=0.1, source="other")  # type: ignore[arg-type]


def test_citation_rejects_end_before_start() -> None:
    with pytest.raises(ValidationError):
        Citation(path="src/x.py", start_line=10, end_line=5)


def test_token_usage_total_autocompute() -> None:
    usage = TokenUsage(prompt=10, completion=5)
    assert usage.total == 15


def test_token_usage_rejects_inconsistent_total() -> None:
    with pytest.raises(ValidationError):
        TokenUsage(prompt=10, completion=5, total=10)


def test_token_usage_zero_default() -> None:
    usage = TokenUsage()
    assert usage.prompt == 0
    assert usage.completion == 0
    assert usage.total == 0


def test_answer_defaults() -> None:
    answer = Answer(text="hi")
    assert answer.citations == []
    assert answer.trace == []
    assert answer.latency_ms == 0
    assert answer.token_usage.total == 0


def test_answer_round_trip_with_citations() -> None:
    c1 = Citation(path="src/a.py", start_line=1, end_line=2, snippet="x")
    c2 = Citation(path="src/b.py", start_line=5, end_line=9, symbol="bar", snippet="y")
    answer = Answer(
        text="result",
        citations=[c1, c2],
        trace=[{"step": "retrieve", "k": 5}],
        latency_ms=42,
        token_usage=TokenUsage(prompt=3, completion=2),
    )
    raw = answer.model_dump_json()
    parsed = Answer.model_validate_json(raw)
    assert parsed == answer
    assert parsed.citations[1].symbol == "bar"
    assert parsed.token_usage.total == 5
