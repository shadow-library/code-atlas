"""Answer, citation, and token-usage value objects."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel, ConfigDict, Field, model_validator

__all__ = ["Answer", "Citation", "TokenUsage"]


class Citation(BaseModel):
    """A pointer back to a source location supporting an answer."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    path: str = Field(min_length=1)
    start_line: int = Field(ge=1)
    end_line: int = Field(ge=1)
    symbol: str | None = None
    snippet: str = Field(default="", max_length=4096)

    @model_validator(mode="after")
    def _check_line_range(self) -> Citation:
        if self.end_line < self.start_line:
            raise ValueError("end_line must be >= start_line")
        return self


class TokenUsage(BaseModel):
    """Prompt/completion/total token counts for a single LLM call."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    prompt: int = Field(default=0, ge=0)
    completion: int = Field(default=0, ge=0)
    total: int = Field(default=0, ge=0)

    @model_validator(mode="after")
    def _reconcile_total(self) -> TokenUsage:
        summed = self.prompt + self.completion
        if self.total == 0 and summed > 0:
            object.__setattr__(self, "total", summed)
            return self
        if self.total > 0 and self.total < summed:
            raise ValueError("total must be >= prompt + completion")
        return self


class Answer(BaseModel):
    """A grounded answer with citations, trace, latency, and token usage."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    text: str = Field(min_length=1)
    citations: list[Citation] = Field(default_factory=list)
    trace: list[dict[str, Any]] = Field(default_factory=list)
    latency_ms: int = Field(default=0, ge=0)
    token_usage: TokenUsage = Field(default_factory=TokenUsage)
