"""Unit tests for the LLM-as-judge answer-correctness metric."""

from __future__ import annotations

from collections.abc import Sequence

import pytest

from code_atlas.domain.answer import Answer
from code_atlas.evaluation import CorrectnessReport, judge_answer
from code_atlas.providers.base import ChatMessage, ChatResponse, ToolSpec


class FakeLLM:
    model = "fake"

    def __init__(self, content: str) -> None:
        self._content = content
        self.calls: list[Sequence[ChatMessage]] = []

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse:
        self.calls.append(messages)
        return ChatResponse(content=self._content)


async def test_all_traits_true() -> None:
    fake = FakeLLM('{"per_trait": {"t1": true, "t2": true}, "rationale": "good"}')
    report = await judge_answer(Answer(text="x"), ["t1", "t2"], fake)
    assert report == CorrectnessReport(score=1.0, per_trait={"t1": True, "t2": True}, rationale="good")


async def test_partial_score() -> None:
    fake = FakeLLM('{"per_trait": {"t1": true, "t2": false, "t3": true}, "rationale": "mixed"}')
    report = await judge_answer(Answer(text="x"), ["t1", "t2", "t3"], fake)
    assert report.score == pytest.approx(2 / 3)
    assert report.per_trait == {"t1": True, "t2": False, "t3": True}


async def test_missing_trait_coerced_false() -> None:
    fake = FakeLLM('{"per_trait": {"t1": true}, "rationale": "partial"}')
    report = await judge_answer(Answer(text="x"), ["t1", "t2"], fake)
    assert report.per_trait == {"t1": True, "t2": False}
    assert report.score == 0.5


async def test_stringified_booleans_coerced() -> None:
    fake = FakeLLM('{"per_trait": {"t1": "true", "t2": "false"}, "rationale": "strings"}')
    report = await judge_answer(Answer(text="x"), ["t1", "t2"], fake)
    assert report.per_trait == {"t1": True, "t2": False}


async def test_malformed_non_json() -> None:
    fake = FakeLLM("totally not json")
    report = await judge_answer(Answer(text="x"), ["t1", "t2"], fake)
    assert report.score == 0.0
    assert report.per_trait == {}
    assert report.rationale == "judge returned malformed output"


async def test_malformed_shape() -> None:
    fake = FakeLLM('{"per_trait": "nope"}')
    report = await judge_answer(Answer(text="x"), ["t1", "t2"], fake)
    assert report.score == 0.0
    assert report.per_trait == {}


async def test_markdown_fenced_json_extracted() -> None:
    fake = FakeLLM('```json\n{"per_trait": {"t1": true}, "rationale": "ok"}\n```')
    report = await judge_answer(Answer(text="x"), ["t1"], fake)
    assert report.per_trait == {"t1": True}
    assert report.score == 1.0


async def test_empty_traits_vacuous() -> None:
    fake = FakeLLM('{"per_trait": {}, "rationale": "unused"}')
    report = await judge_answer(Answer(text="x"), [], fake)
    assert report == CorrectnessReport(score=1.0, per_trait={}, rationale="no traits to evaluate")
    assert fake.calls == []
