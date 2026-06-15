"""Core code-chunk and symbol value objects."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

__all__ = ["ChunkKind", "CodeChunk", "Symbol", "SymbolKind"]


SymbolKind = Literal["function", "method", "class", "module", "variable", "constant", "other"]
ChunkKind = Literal["file", "class", "function", "method", "block"]


class Symbol(BaseModel):
    """A named code entity (function, class, method, ...) at a source location."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    name: str = Field(min_length=1)
    kind: SymbolKind
    path: str = Field(min_length=1)
    line: int = Field(ge=1)
    parent: str | None = None


class CodeChunk(BaseModel):
    """A bounded slice of source identified by content hash and source range."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    chunk_id: str = Field(min_length=1)
    repo_id: str = Field(min_length=1)
    path: str = Field(min_length=1)
    language: str = Field(min_length=1)
    kind: ChunkKind
    symbol: str | None = None
    start_line: int = Field(ge=1)
    end_line: int = Field(ge=1)
    content: str = Field(min_length=1)
    content_hash: str = Field(min_length=8)

    @model_validator(mode="after")
    def _check_line_range(self) -> CodeChunk:
        if self.end_line < self.start_line:
            raise ValueError("end_line must be >= start_line")
        return self
