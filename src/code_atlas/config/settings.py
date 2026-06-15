"""Typed settings: nested pydantic groups loaded from yaml + .env + env vars.

Precedence (highest first): init kwargs > process env vars > .env file > YAML file.
Pydantic-settings takes the first source that yields a value for each field, so
the source ordering in ``settings_customise_sources`` encodes the precedence.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, cast

from pydantic import BaseModel, Field
from pydantic_settings import (
    BaseSettings,
    PydanticBaseSettingsSource,
    SettingsConfigDict,
    YamlConfigSettingsSource,
)

DEFAULT_YAML_PATH = Path("config/default.yaml")

__all__ = [
    "AppSettings",
    "ChatSettings",
    "EmbeddingsSettings",
    "EvalSettings",
    "IngestionSettings",
    "OllamaSettings",
    "Settings",
    "StorageSettings",
    "load_settings",
]


class AppSettings(BaseModel):
    """Process-wide app knobs (logging, etc.)."""

    log_level: str = "INFO"
    log_json: bool = True


class IngestionSettings(BaseModel):
    """File-walk and chunking parameters."""

    max_chunk_lines: int = Field(default=200, gt=0)
    ignore_patterns: list[str] = Field(
        default_factory=lambda: ["node_modules", ".venv", "__pycache__", "dist", "build", ".git"],
    )


class StorageSettings(BaseModel):
    """On-disk locations for vector, lexical, and metadata stores."""

    root_dir: Path = Path("./data")
    lance_uri: str = "./data/lancedb"
    sqlite_path: Path = Path("./data/code_atlas.sqlite")


class EmbeddingsSettings(BaseModel):
    """Embedding-provider selection and request shape."""

    provider: str = "ollama"
    model: str = "nomic-embed-text"
    dimension: int = Field(default=768, gt=0)
    batch_size: int = Field(default=32, gt=0)


class ChatSettings(BaseModel):
    """Chat-provider selection and decoding parameters."""

    provider: str = "ollama"
    model: str = "llama3.1"
    temperature: float = Field(default=0.2, ge=0.0, le=2.0)
    max_tokens: int = Field(default=2048, gt=0)


class OllamaSettings(BaseModel):
    """Connection to the local Ollama daemon."""

    base_url: str = "http://localhost:11434"
    timeout_s: float = Field(default=60.0, gt=0.0)
    concurrency: int = Field(default=4, gt=0)


class EvalSettings(BaseModel):
    """Eval harness paths."""

    cost_rates_path: Path = Path("config/costs.yaml")
    reports_dir: Path = Path("eval/reports")


class Settings(BaseSettings):
    """Top-level settings object. Inject this; never read env directly elsewhere."""

    app: AppSettings = Field(default_factory=AppSettings)
    ingestion: IngestionSettings = Field(default_factory=IngestionSettings)
    storage: StorageSettings = Field(default_factory=StorageSettings)
    embeddings: EmbeddingsSettings = Field(default_factory=EmbeddingsSettings)
    chat: ChatSettings = Field(default_factory=ChatSettings)
    ollama: OllamaSettings = Field(default_factory=OllamaSettings)
    eval: EvalSettings = Field(default_factory=EvalSettings)

    model_config = SettingsConfigDict(
        env_prefix="CODE_ATLAS_",
        env_nested_delimiter="__",
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
        yaml_file=str(DEFAULT_YAML_PATH),
    )

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        return (
            init_settings,
            env_settings,
            dotenv_settings,
            YamlConfigSettingsSource(settings_cls),
            file_secret_settings,
        )


def load_settings(yaml_path: Path | None = None, env_file: Path | None = None, **overrides: Any) -> Settings:
    """Build a :class:`Settings` with optional yaml/env-file overrides for tests."""
    if yaml_path is None and env_file is None:
        return Settings(**overrides)

    overrides_cfg: dict[str, Any] = {}
    if yaml_path is not None:
        overrides_cfg["yaml_file"] = str(yaml_path)
    if env_file is not None:
        overrides_cfg["env_file"] = str(env_file)

    class _Scoped(Settings):
        model_config = cast("SettingsConfigDict", {**Settings.model_config, **overrides_cfg})

    return _Scoped(**overrides)
