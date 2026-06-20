"""Cost estimation from token usage against a provider/model rate table loaded from YAML."""

from __future__ import annotations

from typing import TYPE_CHECKING

import yaml
from pydantic import BaseModel, ConfigDict, ValidationError

from code_atlas.errors import EvaluationError
from code_atlas.utils import get_logger

if TYPE_CHECKING:
    from pathlib import Path

    from code_atlas.domain.answer import TokenUsage

__all__ = ["CostRate", "CostTable", "estimate_cost", "load_cost_table"]

log = get_logger(__name__)

CostTable = dict[str, dict[str, "CostRate"]]


class CostRate(BaseModel):
    """USD per 1k tokens for prompt and completion on one provider/model."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    prompt_per_1k: float = 0.0
    completion_per_1k: float = 0.0


def load_cost_table(path: Path) -> CostTable:
    """Load a ``{provider: {model: {prompt_per_1k, completion_per_1k}}}`` rate table from YAML."""
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise EvaluationError("costs: cannot read file", context={"path": str(path)}) from exc

    try:
        data = yaml.safe_load(raw)
    except yaml.YAMLError as exc:
        raise EvaluationError("costs: invalid YAML", context={"path": str(path)}) from exc

    if not isinstance(data, dict):
        raise EvaluationError(
            "costs: invalid rate table", context={"path": str(path), "error": "top-level is not a mapping"}
        )

    table: CostTable = {}
    for provider, models in data.items():
        if not isinstance(models, dict):
            raise EvaluationError(
                "costs: invalid rate table",
                context={"path": str(path), "error": f"provider {provider!r} is not a mapping"},
            )
        rates: dict[str, CostRate] = {}
        for model, rate in models.items():
            if not isinstance(rate, dict):
                raise EvaluationError(
                    "costs: invalid rate table",
                    context={"path": str(path), "error": f"rate for {provider!r}/{model!r} is not a mapping"},
                )
            try:
                rates[str(model)] = CostRate.model_validate(rate)
            except ValidationError as exc:
                raise EvaluationError(
                    "costs: invalid rate table", context={"path": str(path), "error": str(exc)}
                ) from exc
        table[str(provider)] = rates

    log.info("costs.loaded", providers=len(table))
    return table


def estimate_cost(usage: TokenUsage, *, provider: str, model: str, table: CostTable) -> float:
    """Estimate USD cost for one call; fall back to the provider's ``default`` model, else warn and return 0.0."""
    models = table.get(provider)
    rate = models.get(model) or models.get("default") if models else None
    if rate is None:
        log.warning("cost.rate_missing", provider=provider, model=model)
        return 0.0
    return usage.prompt / 1000 * rate.prompt_per_1k + usage.completion / 1000 * rate.completion_per_1k
