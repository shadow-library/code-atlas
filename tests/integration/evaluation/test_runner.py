"""Integration tests for the eval runner, cost estimation, and report writer."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator, Sequence
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from code_atlas.domain.answer import Answer, Citation, TokenUsage
from code_atlas.domain.chunk import CodeChunk
from code_atlas.evaluation.datasets import EvalCase
from code_atlas.evaluation.metrics_cost import CostRate, estimate_cost, load_cost_table
from code_atlas.evaluation.report import write_report
from code_atlas.evaluation.runner import EvalRunner
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.providers.base import ChatMessage, ChatResponse, ToolSpec

if TYPE_CHECKING:
    from collections.abc import Iterator

REPO_ID = "repo1"
PATH_A = "src/a.py"
PATH_B = "src/b.py"


class StubAgent:
    """Returns a canned ``Answer`` per question."""

    def __init__(self, answers: dict[str, Answer]) -> None:
        self._answers = answers

    async def ask(self, question: str) -> Answer:
        return self._answers[question]


class StubJudge:
    """Canned LLM judge: marks every requested trait satisfied."""

    model = "fake"

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse:
        user = messages[-1].content
        traits = [line.split(". ", 1)[1] for line in user.splitlines() if line[:1].isdigit() and ". " in line]
        per_trait = {t: True for t in traits}
        payload = json.dumps({"per_trait": per_trait, "rationale": "all traits satisfied"})
        return ChatResponse(content=payload, model=self.model)

    def chat_stream(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> AsyncIterator[object]:
        raise NotImplementedError


@pytest.fixture
def store() -> Iterator[MetadataStore]:
    s = MetadataStore("sqlite:///:memory:")
    s.upsert(
        CodeChunk(
            chunk_id="c-a",
            repo_id=REPO_ID,
            path=PATH_A,
            language="python",
            kind="function",
            symbol="alpha",
            start_line=1,
            end_line=20,
            content="def alpha():\n    return 'alpha body'\n",
            content_hash="hash-alpha",
        )
    )
    s.upsert(
        CodeChunk(
            chunk_id="c-b",
            repo_id=REPO_ID,
            path=PATH_B,
            language="python",
            kind="function",
            symbol="beta",
            start_line=1,
            end_line=20,
            content="def beta():\n    return 'beta body'\n",
            content_hash="hash-beta",
        )
    )
    yield s
    s.close()


def _answers() -> dict[str, Answer]:
    return {
        "What is alpha?": Answer(
            text="alpha returns 'alpha body'",
            citations=[Citation(path=PATH_A, start_line=1, end_line=2, symbol="alpha", snippet="alpha body")],
            latency_ms=100,
            token_usage=TokenUsage(prompt=1000, completion=500),
        ),
        "What is beta?": Answer(
            text="beta returns 'beta body'",
            citations=[Citation(path=PATH_B, start_line=1, end_line=2, symbol="beta", snippet="beta body")],
            latency_ms=300,
            token_usage=TokenUsage(prompt=2000, completion=1000),
        ),
    }


def _cases() -> list[EvalCase]:
    return [
        EvalCase(
            case_id="case-a",
            repo_id=REPO_ID,
            question="What is alpha?",
            expected_files=[PATH_A],
            expected_symbols=["alpha"],
            expected_answer_traits=["mentions alpha"],
        ),
        EvalCase(
            case_id="case-b",
            repo_id=REPO_ID,
            question="What is beta?",
            expected_files=[PATH_B],
            expected_symbols=["beta"],
            expected_answer_traits=["mentions beta"],
        ),
    ]


_RATE = CostRate(prompt_per_1k=0.00015, completion_per_1k=0.0006)
_TABLE = {"openai": {"gpt-4o-mini": _RATE}}


async def test_run_produces_populated_eval_run(store: MetadataStore) -> None:
    runner = EvalRunner(
        agent=StubAgent(_answers()),
        metadata_store=store,
        judge_llm=StubJudge(),
        cost_table=_TABLE,
        provider="openai",
        model="gpt-4o-mini",
        k=5,
    )
    run = await runner.run(_cases())

    assert len(run.cases) == 2
    agg = run.aggregates
    assert agg.n_cases == 2
    assert agg.k == 5
    assert 0.0 <= agg.mean_recall_at_k <= 1.0
    assert 0.0 <= agg.mean_mrr <= 1.0
    assert 0.0 <= agg.mean_ndcg_at_k <= 1.0
    assert 0.0 <= agg.mean_grounding_rate <= 1.0
    assert 0.0 <= agg.mean_correctness <= 1.0
    assert agg.mean_recall_at_k == pytest.approx(1.0)
    assert agg.mean_grounding_rate == pytest.approx(1.0)
    assert agg.mean_correctness == pytest.approx(1.0)
    assert isinstance(agg.latency_p50_ms, float)
    assert isinstance(agg.latency_p95_ms, float)
    assert agg.latency_p50_ms == pytest.approx(200.0)

    cost_a = 1000 / 1000 * 0.00015 + 500 / 1000 * 0.0006
    cost_b = 2000 / 1000 * 0.00015 + 1000 / 1000 * 0.0006
    assert agg.total_cost_usd == pytest.approx(cost_a + cost_b)
    assert agg.mean_cost_usd == pytest.approx((cost_a + cost_b) / 2)


async def test_write_report_emits_json_and_markdown(store: MetadataStore, tmp_path: Path) -> None:
    runner = EvalRunner(
        agent=StubAgent(_answers()),
        metadata_store=store,
        judge_llm=StubJudge(),
        cost_table=_TABLE,
        provider="openai",
        model="gpt-4o-mini",
        k=5,
    )
    run = await runner.run(_cases())
    json_path, md_path = write_report(run, tmp_path / "reports")

    assert json_path.exists()
    assert md_path.exists()
    loaded = json.loads(json_path.read_text(encoding="utf-8"))
    assert loaded["run_id"] == run.run_id
    assert "aggregates" in loaded
    assert len(loaded["cases"]) == 2

    md = md_path.read_text(encoding="utf-8")
    assert "## Aggregates" in md
    assert "## Per-case results" in md
    assert "case-a" in md
    assert "case-b" in md


def test_estimate_cost_known_usage() -> None:
    usage = TokenUsage(prompt=1000, completion=500)
    cost = estimate_cost(usage, provider="openai", model="gpt-4o-mini", table=_TABLE)
    assert cost == pytest.approx(0.00015 + 0.0003)


def test_estimate_cost_missing_provider_returns_zero() -> None:
    usage = TokenUsage(prompt=1000, completion=500)
    assert estimate_cost(usage, provider="nope", model="gpt-4o-mini", table=_TABLE) == 0.0


def test_load_cost_table_roundtrip_from_shipped_yaml() -> None:
    path = Path(__file__).resolve().parents[3] / "config" / "costs.yaml"
    table = load_cost_table(path)

    usage = TokenUsage(prompt=5000, completion=5000)
    assert estimate_cost(usage, provider="ollama", model="llama3", table=table) == 0.0
    assert table["openai"]["gpt-4o-mini"].prompt_per_1k == pytest.approx(0.00015)
    assert table["openai"]["gpt-4o-mini"].completion_per_1k == pytest.approx(0.0006)
