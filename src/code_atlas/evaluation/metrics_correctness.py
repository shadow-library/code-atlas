"""Answer correctness via LLM-as-judge: per-trait booleans from the LLM, the score computed by us."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any

from pydantic import BaseModel, ConfigDict

from code_atlas.providers.base import ChatMessage
from code_atlas.utils import get_logger

if TYPE_CHECKING:
    from code_atlas.domain.answer import Answer
    from code_atlas.providers.base import LLMProvider

__all__ = ["CorrectnessReport", "judge_answer"]

log = get_logger(__name__)

_JUDGE_SYSTEM_PROMPT = (
    "You are a strict answer-correctness evaluator. You are given an answer and a list of expected "
    "traits the answer should exhibit. For EACH trait, independently decide whether the answer satisfies "
    "it: true if the answer clearly exhibits the trait, false otherwise.\n\n"
    "Respond with ONLY a JSON object of this exact shape and nothing else (no prose, no markdown):\n"
    '{"per_trait": {<trait text>: <bool>, ...}, "rationale": <string>}\n\n'
    "Use each trait's EXACT text as its key. The rationale is a brief explanation of your judgments."
)


class CorrectnessReport(BaseModel):
    """Per-trait correctness judgment and the fraction of traits satisfied."""

    model_config = ConfigDict(frozen=True)

    score: float
    per_trait: dict[str, bool]
    rationale: str


def _build_messages(answer: Answer, expected_traits: list[str]) -> list[ChatMessage]:
    """Assemble the judge prompt: system instructions plus the answer and numbered expected traits."""
    numbered = "\n".join(f"{i}. {trait}" for i, trait in enumerate(expected_traits, start=1))
    user = f"ANSWER:\n{answer.text}\n\nEXPECTED TRAITS:\n{numbered}"
    return [
        ChatMessage(role="system", content=_JUDGE_SYSTEM_PROMPT),
        ChatMessage(role="user", content=user),
    ]


def _coerce_bool(v: Any) -> bool:
    """Normalize a judge value to bool; LLMs frequently emit stringified booleans."""
    if isinstance(v, bool):
        return v
    if isinstance(v, str):
        return v.strip().lower() in {"true", "yes", "1"}
    return bool(v)


def _parse_judge(content: str, expected_traits: list[str]) -> CorrectnessReport:
    """Parse the judge's JSON, aligning per-trait keys to the requested traits; soft-fail on malformed output."""
    parsed: Any = None
    try:
        parsed = json.loads(content)
    except json.JSONDecodeError:
        start = content.find("{")
        end = content.rfind("}")
        if start != -1 and end > start:
            try:
                parsed = json.loads(content[start : end + 1])
            except json.JSONDecodeError:
                parsed = None

    if not isinstance(parsed, dict) or not isinstance(parsed.get("per_trait"), dict):
        log.warning("correctness.judge_malformed", content=content[:200])
        return CorrectnessReport(score=0.0, per_trait={}, rationale="judge returned malformed output")

    raw: dict[str, Any] = parsed["per_trait"]
    per_trait = {t: _coerce_bool(raw.get(t, False)) for t in expected_traits}
    rationale = str(parsed.get("rationale", ""))
    score = sum(per_trait.values()) / len(expected_traits)
    return CorrectnessReport(score=score, per_trait=per_trait, rationale=rationale)


async def judge_answer(answer: Answer, expected_traits: list[str], llm: LLMProvider) -> CorrectnessReport:
    """Judge an answer against expected traits; score is the fraction of traits the LLM marks satisfied."""
    if not expected_traits:
        return CorrectnessReport(score=1.0, per_trait={}, rationale="no traits to evaluate")

    response = await llm.chat(_build_messages(answer, expected_traits))
    return _parse_judge(response.content, expected_traits)
