"""Eval runner: drive an agent over eval cases, compute per-case metrics, and aggregate."""

from __future__ import annotations

import math
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Protocol
from uuid import uuid4

from pydantic import BaseModel, ConfigDict

from code_atlas.domain.answer import TokenUsage
from code_atlas.evaluation.metrics_correctness import CorrectnessReport, judge_answer
from code_atlas.evaluation.metrics_cost import estimate_cost
from code_atlas.evaluation.metrics_grounding import GroundingReport, check_grounding
from code_atlas.evaluation.metrics_retrieval import mrr, ndcg_at_k, recall_at_k
from code_atlas.utils import get_logger

if TYPE_CHECKING:
    from code_atlas.domain.answer import Answer
    from code_atlas.evaluation.datasets import EvalCase
    from code_atlas.evaluation.metrics_cost import CostTable
    from code_atlas.indexing.metadata_store import MetadataStore
    from code_atlas.providers.base import LLMProvider

__all__ = ["Agent", "CaseResult", "EvalAggregates", "EvalRun", "EvalRunner"]

log = get_logger(__name__)


class Agent(Protocol):
    """Structural view of the production QA agent the runner drives."""

    async def ask(self, question: str) -> Answer: ...


class CaseResult(BaseModel):
    """Per-case metric bundle: retrieval, grounding, correctness, latency, and cost."""

    model_config = ConfigDict(frozen=True)

    case_id: str
    recall_at_k: float
    mrr: float
    ndcg_at_k: float
    grounding: GroundingReport
    correctness: CorrectnessReport
    latency_ms: int
    cost_usd: float
    token_usage: TokenUsage


class EvalAggregates(BaseModel):
    """Run-level rollup: means, latency percentiles, and cost totals."""

    model_config = ConfigDict(frozen=True)

    n_cases: int
    k: int
    mean_recall_at_k: float
    mean_mrr: float
    mean_ndcg_at_k: float
    mean_grounding_rate: float
    mean_correctness: float
    latency_p50_ms: float
    latency_p95_ms: float
    total_cost_usd: float
    mean_cost_usd: float


class EvalRun(BaseModel):
    """A complete eval run: its cases and aggregates under a filesystem-safe run id."""

    model_config = ConfigDict(frozen=True)

    run_id: str
    k: int
    cases: list[CaseResult]
    aggregates: EvalAggregates


def _mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def _grounding_rate(r: GroundingReport) -> float:
    # No citations is a vacuous pass: there is nothing that could be ungrounded.
    return r.grounded / r.total if r.total else 1.0


def _percentile(values: list[int], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return float(ordered[0])
    rank = pct / 100 * (len(ordered) - 1)
    lo = math.floor(rank)
    hi = math.ceil(rank)
    if lo == hi:
        return float(ordered[lo])
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (rank - lo)


class EvalRunner:
    """Run an agent over eval cases and produce an ``EvalRun`` with per-case and aggregate metrics."""

    def __init__(
        self,
        *,
        agent: Agent,
        metadata_store: MetadataStore,
        judge_llm: LLMProvider,
        cost_table: CostTable,
        provider: str,
        model: str,
        k: int = 10,
    ) -> None:
        self._agent = agent
        self._metadata_store = metadata_store
        self._judge_llm = judge_llm
        self._cost_table = cost_table
        self._provider = provider
        self._model = model
        self._k = k

    async def run(self, cases: list[EvalCase], *, run_id: str | None = None) -> EvalRun:
        """Evaluate each case in order, aggregate, and return the run; per-case errors propagate."""
        if run_id is None:
            run_id = f"{datetime.now(UTC):%Y%m%dT%H%M%S}-{uuid4().hex[:8]}"

        results: list[CaseResult] = []
        for case in cases:
            answer = await self._agent.ask(case.question)
            retrieved = [c.path for c in answer.citations]
            grounding = check_grounding(answer, self._metadata_store, repo_id=case.repo_id)
            correctness = await judge_answer(answer, case.expected_answer_traits, self._judge_llm)
            cost = estimate_cost(answer.token_usage, provider=self._provider, model=self._model, table=self._cost_table)
            results.append(
                CaseResult(
                    case_id=case.case_id,
                    recall_at_k=recall_at_k(retrieved, case.expected_files, self._k),
                    mrr=mrr(retrieved, case.expected_files),
                    ndcg_at_k=ndcg_at_k(retrieved, case.expected_files, self._k),
                    grounding=grounding,
                    correctness=correctness,
                    latency_ms=answer.latency_ms,
                    cost_usd=cost,
                    token_usage=answer.token_usage,
                )
            )

        latencies = [r.latency_ms for r in results]
        costs = [r.cost_usd for r in results]
        aggregates = EvalAggregates(
            n_cases=len(results),
            k=self._k,
            mean_recall_at_k=_mean([r.recall_at_k for r in results]),
            mean_mrr=_mean([r.mrr for r in results]),
            mean_ndcg_at_k=_mean([r.ndcg_at_k for r in results]),
            mean_grounding_rate=_mean([_grounding_rate(r.grounding) for r in results]),
            mean_correctness=_mean([r.correctness.score for r in results]),
            latency_p50_ms=_percentile(latencies, 50),
            latency_p95_ms=_percentile(latencies, 95),
            total_cost_usd=sum(costs),
            mean_cost_usd=_mean(costs),
        )

        log.info("eval.run.completed", run_id=run_id, n_cases=len(results))
        return EvalRun(run_id=run_id, k=self._k, cases=results, aggregates=aggregates)
