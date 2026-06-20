"""Render and persist an eval run as JSON plus a human-readable Markdown report."""

from __future__ import annotations

from typing import TYPE_CHECKING

from code_atlas.utils import get_logger

if TYPE_CHECKING:
    from pathlib import Path

    from code_atlas.evaluation.runner import EvalRun

__all__ = ["render_markdown", "write_report"]

log = get_logger(__name__)


def render_markdown(run: EvalRun) -> str:
    """Render an eval run as Markdown: a title, an aggregates table, and a per-case table."""
    a = run.aggregates
    lines: list[str] = [
        f"# Eval report: {run.run_id}",
        "",
        "## Aggregates",
        "",
        "| metric | value |",
        "| --- | --- |",
        f"| n_cases | {a.n_cases} |",
        f"| k | {a.k} |",
        f"| mean_recall@k | {a.mean_recall_at_k:.4f} |",
        f"| mean_mrr | {a.mean_mrr:.4f} |",
        f"| mean_ndcg@k | {a.mean_ndcg_at_k:.4f} |",
        f"| mean_grounding_rate | {a.mean_grounding_rate:.4f} |",
        f"| mean_correctness | {a.mean_correctness:.4f} |",
        f"| latency_p50_ms | {a.latency_p50_ms:.4f} |",
        f"| latency_p95_ms | {a.latency_p95_ms:.4f} |",
        f"| total_cost_usd | {a.total_cost_usd:.6f} |",
        f"| mean_cost_usd | {a.mean_cost_usd:.6f} |",
        "",
        "## Per-case results",
        "",
        "| case_id | recall@k | mrr | ndcg@k | grounded | correctness | latency_ms | cost_usd |",
        "| --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for c in run.cases:
        grounded = f"{c.grounding.grounded}/{c.grounding.total}"
        lines.append(
            f"| {c.case_id} | {c.recall_at_k:.4f} | {c.mrr:.4f} | {c.ndcg_at_k:.4f} | "
            f"{grounded} | {c.correctness.score:.4f} | {c.latency_ms} | {c.cost_usd:.6f} |"
        )
    return "\n".join(lines) + "\n"


def write_report(run: EvalRun, out_dir: Path) -> tuple[Path, Path]:
    """Write the run's JSON and Markdown reports into ``out_dir``; return ``(json_path, md_path)``."""
    out_dir.mkdir(parents=True, exist_ok=True)
    json_path = out_dir / f"{run.run_id}.json"
    md_path = out_dir / f"{run.run_id}.md"
    json_path.write_text(run.model_dump_json(indent=2), encoding="utf-8")
    md_path.write_text(render_markdown(run), encoding="utf-8")
    log.info("eval.report.written", json=str(json_path), md=str(md_path))
    return json_path, md_path
