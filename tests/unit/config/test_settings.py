"""Tests for the layered Settings loader (yaml + .env + env vars)."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import ValidationError

from code_atlas.config import Settings, load_settings

ENV_KEYS = (
    "CODE_ATLAS_APP__LOG_LEVEL",
    "CODE_ATLAS_APP__LOG_JSON",
    "CODE_ATLAS_CHAT__TEMPERATURE",
    "CODE_ATLAS_OLLAMA__BASE_URL",
)


def _clear_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in ENV_KEYS:
        monkeypatch.delenv(key, raising=False)


def test_defaults_when_no_yaml_no_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    s = load_settings()
    assert s.app.log_level == "INFO"
    assert s.ollama.base_url == "http://localhost:11434"
    assert isinstance(s, Settings)


def test_yaml_overrides_defaults(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    yaml_path = tmp_path / "config.yaml"
    yaml_path.write_text("chat:\n  temperature: 0.7\n", encoding="utf-8")
    s = load_settings(yaml_path=yaml_path)
    assert s.chat.temperature == 0.7
    assert s.chat.provider == "ollama"


def test_env_overrides_yaml(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    yaml_path = tmp_path / "config.yaml"
    yaml_path.write_text("app:\n  log_level: DEBUG\n", encoding="utf-8")
    monkeypatch.setenv("CODE_ATLAS_APP__LOG_LEVEL", "ERROR")
    s = load_settings(yaml_path=yaml_path)
    assert s.app.log_level == "ERROR"


def test_dotenv_overrides_yaml_but_env_wins(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    yaml_path = tmp_path / "config.yaml"
    yaml_path.write_text("app:\n  log_level: DEBUG\n", encoding="utf-8")
    env_path = tmp_path / "custom.env"
    env_path.write_text("CODE_ATLAS_APP__LOG_LEVEL=WARNING\n", encoding="utf-8")

    s = load_settings(yaml_path=yaml_path, env_file=env_path)
    assert s.app.log_level == "WARNING"

    monkeypatch.setenv("CODE_ATLAS_APP__LOG_LEVEL", "ERROR")
    s2 = load_settings(yaml_path=yaml_path, env_file=env_path)
    assert s2.app.log_level == "ERROR"


def test_init_overrides_beat_everything(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    yaml_path = tmp_path / "config.yaml"
    yaml_path.write_text("app:\n  log_level: DEBUG\n", encoding="utf-8")
    monkeypatch.setenv("CODE_ATLAS_APP__LOG_LEVEL", "ERROR")
    s = load_settings(yaml_path=yaml_path, app={"log_level": "CRITICAL"})
    assert s.app.log_level == "CRITICAL"


def test_invalid_type_raises_validation_error(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    monkeypatch.setenv("CODE_ATLAS_CHAT__TEMPERATURE", "not-a-number")
    with pytest.raises(ValidationError):
        load_settings()


def test_out_of_range_raises_validation_error(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    _clear_env(monkeypatch)
    monkeypatch.setenv("CODE_ATLAS_CHAT__TEMPERATURE", "5.0")
    with pytest.raises(ValidationError):
        load_settings()
