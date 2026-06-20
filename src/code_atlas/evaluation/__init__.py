"""Evaluation subsystem: datasets, metrics, and the eval runner."""

from code_atlas.evaluation.datasets import EvalCase, load_dataset
from code_atlas.evaluation.metrics_correctness import CorrectnessReport, judge_answer
from code_atlas.evaluation.metrics_grounding import GroundingReport, UngroundedCitation, check_grounding
from code_atlas.evaluation.metrics_retrieval import mrr, ndcg_at_k, recall_at_k

__all__ = [
    "CorrectnessReport",
    "EvalCase",
    "GroundingReport",
    "UngroundedCitation",
    "check_grounding",
    "judge_answer",
    "load_dataset",
    "mrr",
    "ndcg_at_k",
    "recall_at_k",
]
