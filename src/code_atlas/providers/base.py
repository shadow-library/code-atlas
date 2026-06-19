"""Provider Protocols and frozen pydantic records for chat I/O."""

from __future__ import annotations

from collections.abc import AsyncIterator, Sequence
from typing import Any, Literal, Protocol

from pydantic import BaseModel, ConfigDict, Field

from code_atlas.domain.answer import TokenUsage

__all__ = [
    "ChatChunk",
    "ChatMessage",
    "ChatResponse",
    "ChatRole",
    "EmbeddingProvider",
    "LLMProvider",
    "ToolCall",
    "ToolSpec",
]

ChatRole = Literal["system", "user", "assistant", "tool"]


class ChatMessage(BaseModel):
    """One turn in a chat conversation."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    role: ChatRole
    content: str
    tool_call_id: str | None = None
    name: str | None = None


class ToolSpec(BaseModel):
    """JSON-schema-style description of a callable tool exposed to the LLM."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    name: str = Field(min_length=1)
    description: str = ""
    parameters: dict[str, Any] = Field(default_factory=dict)


class ToolCall(BaseModel):
    """A single tool invocation requested by the LLM."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    id: str = Field(min_length=1)
    name: str = Field(min_length=1)
    arguments: dict[str, Any] = Field(default_factory=dict)


class ChatResponse(BaseModel):
    """Non-streamed chat completion."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    content: str = ""
    tool_calls: list[ToolCall] = Field(default_factory=list)
    usage: TokenUsage = Field(default_factory=TokenUsage)
    model: str = ""
    finish_reason: str | None = None


class ChatChunk(BaseModel):
    """One delta in a streamed chat response. Final chunk: done=True."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    content_delta: str = ""
    tool_call_delta: ToolCall | None = None
    done: bool = False
    usage: TokenUsage | None = None
    finish_reason: str | None = None


class EmbeddingProvider(Protocol):
    """Async batch embedding."""

    model: str
    dimension: int

    async def embed(self, texts: list[str]) -> list[list[float]]: ...


class LLMProvider(Protocol):
    """Async chat with optional tools, plus a streaming variant."""

    model: str

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse: ...

    def chat_stream(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> AsyncIterator[ChatChunk]: ...
