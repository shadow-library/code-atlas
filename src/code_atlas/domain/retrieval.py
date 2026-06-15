"""Retrieval query and result value objects."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field

from code_atlas.domain.chunk import CodeChunk

__all__ = ["RetrievalQuery", "RetrievalResult", "RetrievalSource"]


RetrievalSource = Literal["vector", "lexical", "fused"]


class RetrievalQuery(BaseModel):
    """A natural-language retrieval request with optional metadata filters."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    text: str = Field(min_length=1)
    k: int = Field(default=10, gt=0, le=200)
    filters: dict[str, Any] = Field(default_factory=dict)


class RetrievalResult(BaseModel):
    """A scored chunk returned by a retrieval source."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    chunk: CodeChunk
    score: float = Field(ge=0.0)
    source: RetrievalSource
