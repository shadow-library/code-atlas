"""Offline CLI tests: exercise only --help and the `init` command (no network)."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml
from typer.testing import CliRunner

from code_atlas.cli import app

runner = CliRunner()


def test_help_lists_three_commands() -> None:
    result = runner.invoke(app, ["--help"])
    assert result.exit_code == 0
    assert "init" in result.output
    assert "ingest" in result.output
    assert "ask" in result.output


def test_init_help_shows_force() -> None:
    result = runner.invoke(app, ["init", "--help"])
    assert result.exit_code == 0
    assert "--force" in result.output


def test_ingest_help_shows_flags() -> None:
    result = runner.invoke(app, ["ingest", "--help"])
    assert result.exit_code == 0
    assert "--repo" in result.output
    assert "--id" in result.output


def test_ask_help_shows_repo_id() -> None:
    result = runner.invoke(app, ["ask", "--help"])
    assert result.exit_code == 0
    assert "--repo-id" in result.output


def test_init_writes_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    result = runner.invoke(app, ["init"])
    assert result.exit_code == 0
    target = Path("config/default.yaml")
    assert target.exists()
    data = yaml.safe_load(target.read_text())
    for key in ("app", "storage", "embeddings", "chat", "ollama"):
        assert key in data
    assert data["embeddings"]["provider"] == "ollama"


def test_init_refuses_existing_without_force(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    target = Path("config/default.yaml")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("sentinel: true\n")
    result = runner.invoke(app, ["init"])
    assert result.exit_code != 0
    assert yaml.safe_load(target.read_text()) == {"sentinel": True}


def test_init_force_overwrites(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    target = Path("config/default.yaml")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("sentinel: true\n")
    result = runner.invoke(app, ["init", "--force"])
    assert result.exit_code == 0
    data = yaml.safe_load(target.read_text())
    assert "sentinel" not in data
    assert data["embeddings"]["provider"] == "ollama"
