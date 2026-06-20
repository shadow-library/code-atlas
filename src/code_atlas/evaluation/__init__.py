"""Evaluation subsystem: datasets, metrics, and the eval runner."""

from code_atlas.evaluation.datasets import EvalCase, load_dataset
from code_atlas.evaluation.metrics_correctness import CorrectnessReport, judge_answer
from code_atlas.evaluation.metrics_cost import CostRate, CostTable, estimate_cost, load_cost_table
from code_atlas.evaluation.metrics_grounding import GroundingReport, UngroundedCitation, check_grounding
from code_atlas.evaluation.metrics_retrieval import mrr, ndcg_at_k, recall_at_k
from code_atlas.evaluation.report import render_markdown, write_report
from code_atlas.evaluation.runner import Agent, CaseResult, EvalAggregates, EvalRun, EvalRunner

__all__ = [
    "Agent",
    "CaseResult",
    "CorrectnessReport",
    "CostRate",
    "CostTable",
    "EvalAggregates",
    "EvalCase",
    "EvalRun",
    "EvalRunner",
    "GroundingReport",
    "UngroundedCitation",
    "check_grounding",
    "estimate_cost",
    "judge_answer",
    "load_cost_table",
    "load_dataset",
    "mrr",
    "ndcg_at_k",
    "recall_at_k",
    "render_markdown",
    "write_report",
]
