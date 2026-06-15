"""Typed exception hierarchy for code-atlas."""

from __future__ import annotations

from typing import Any

__all__ = [
    "AgentError",
    "CodeAtlasError",
    "ConfigError",
    "EvaluationError",
    "IndexingError",
    "IngestionError",
    "ProviderError",
    "RepositoryNotIndexed",
    "RetrievalError",
]


class CodeAtlasError(Exception):
    """Base class for all code-atlas errors; carries optional context."""

    def __init__(self, message: str, *, context: dict[str, Any] | None = None) -> None:
        self.message = message
        self.context: dict[str, Any] = dict(context) if context else {}
        super().__init__(message)

    def __str__(self) -> str:
        if not self.context:
            return self.message
        return f"{self.message} | context={self.context!r}"

    def __repr__(self) -> str:
        return f"{type(self).__name__}(message={self.message!r}, context={self.context!r})"


class ConfigError(CodeAtlasError):
    """Invalid or missing configuration."""


class IngestionError(CodeAtlasError):
    """Repo walking, language detection, or parsing failure."""


class IndexingError(CodeAtlasError):
    """Write failure in vector, lexical, metadata, or graph stores."""


class RepositoryNotIndexed(IndexingError):
    """Operation requires an indexed repo that is missing."""


class ProviderError(CodeAtlasError):
    """LLM or embedding provider failure."""


class RetrievalError(CodeAtlasError):
    """Retrieval pipeline failure."""


class AgentError(CodeAtlasError):
    """Q&A agent failure."""


class EvaluationError(CodeAtlasError):
    """Evaluation runner or metrics failure."""
