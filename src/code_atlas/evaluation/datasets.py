"""Eval-case schema and YAML dataset loader."""

from __future__ import annotations

from typing import TYPE_CHECKING

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from code_atlas.errors import EvaluationError
from code_atlas.utils import get_logger

if TYPE_CHECKING:
    from pathlib import Path

__all__ = ["EvalCase", "load_dataset"]

log = get_logger(__name__)


class EvalCase(BaseModel):
    """A single grounded question with expected files, symbols, and answer traits."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    case_id: str = Field(min_length=1)
    repo_id: str = Field(min_length=1)
    question: str = Field(min_length=1)
    expected_files: list[str] = Field(default_factory=list)
    expected_symbols: list[str] = Field(default_factory=list)
    expected_answer_traits: list[str] = Field(default_factory=list)


def load_dataset(path: Path) -> list[EvalCase]:
    """Load and validate an eval dataset from a YAML file.

    The file is a mapping with a ``cases:`` list of case dicts; the mapping shape
    leaves room for future dataset-level metadata without breaking the format.
    """
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise EvaluationError("dataset: cannot read file", context={"path": str(path)}) from exc

    try:
        data = yaml.safe_load(raw)
    except yaml.YAMLError as exc:
        raise EvaluationError("dataset: invalid YAML", context={"path": str(path)}) from exc

    if not isinstance(data, dict):
        raise EvaluationError("dataset: 'cases' must be a list", context={"path": str(path)})
    cases_raw = data.get("cases")
    if not isinstance(cases_raw, list):
        raise EvaluationError("dataset: 'cases' must be a list", context={"path": str(path)})

    cases: list[EvalCase] = []
    seen: set[str] = set()
    for i, item in enumerate(cases_raw):
        try:
            case = EvalCase.model_validate(item)
        except ValidationError as exc:
            raise EvaluationError("dataset: invalid case", context={"index": i, "error": str(exc)}) from exc
        if case.case_id in seen:
            raise EvaluationError("dataset: duplicate case_id", context={"case_id": case.case_id})
        seen.add(case.case_id)
        cases.append(case)

    log.info("dataset.loaded", count=len(cases))
    return cases
