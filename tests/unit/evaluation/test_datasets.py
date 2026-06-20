"""Tests for the eval dataset schema and YAML loader."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

from code_atlas.errors import EvaluationError
from code_atlas.evaluation import EvalCase, load_dataset

SEED_PATH = Path(__file__).resolve().parents[3] / "eval" / "datasets" / "seed.yaml"


def test_seed_dataset_parses() -> None:
    cases = load_dataset(SEED_PATH)
    assert len(cases) >= 6
    assert all(isinstance(c, EvalCase) for c in cases)
    assert all(c.repo_id == "code-atlas" for c in cases)
    assert len({c.case_id for c in cases}) == len(cases)


def test_load_validates_required_fields(tmp_path: Path) -> None:
    path = tmp_path / "missing_question.yaml"
    path.write_text(yaml.safe_dump({"cases": [{"case_id": "a", "repo_id": "code-atlas"}]}), encoding="utf-8")
    with pytest.raises(EvaluationError):
        load_dataset(path)


def test_duplicate_case_id_raises(tmp_path: Path) -> None:
    path = tmp_path / "dupe.yaml"
    case = {"case_id": "dup", "repo_id": "code-atlas", "question": "q?"}
    path.write_text(yaml.safe_dump({"cases": [case, dict(case)]}), encoding="utf-8")
    with pytest.raises(EvaluationError) as exc_info:
        load_dataset(path)
    assert "case_id" in exc_info.value.context


def test_missing_cases_key_raises(tmp_path: Path) -> None:
    path = tmp_path / "no_cases.yaml"
    path.write_text(yaml.safe_dump({"not_cases": []}), encoding="utf-8")
    with pytest.raises(EvaluationError):
        load_dataset(path)


def test_missing_file_raises(tmp_path: Path) -> None:
    with pytest.raises(EvaluationError):
        load_dataset(tmp_path / "nope.yaml")


def test_defaults_for_optional_lists(tmp_path: Path) -> None:
    path = tmp_path / "minimal.yaml"
    path.write_text(
        yaml.safe_dump({"cases": [{"case_id": "m", "repo_id": "code-atlas", "question": "q?"}]}),
        encoding="utf-8",
    )
    cases = load_dataset(path)
    assert len(cases) == 1
    case = cases[0]
    assert case.expected_files == []
    assert case.expected_symbols == []
    assert case.expected_answer_traits == []
