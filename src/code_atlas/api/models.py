"""Pydantic request/response models for the HTTP API.

The ``/ask`` response reuses the domain :class:`~code_atlas.domain.answer.Answer`
directly, so only the request bodies and the lightweight status responses live here.
"""

from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

__all__ = ["AskRequest", "HealthResponse", "IngestRequest", "IngestResponse"]


class HealthResponse(BaseModel):
    """Liveness probe payload."""

    status: str
    version: str


class IngestRequest(BaseModel):
    """Request to index a repository at ``repo_path`` under ``repo_id``."""

    model_config = ConfigDict(extra="forbid")

    repo_path: str = Field(min_length=1)
    repo_id: str = Field(min_length=1)


class IngestResponse(BaseModel):
    """Acknowledgement that an ingest job was accepted."""

    job_id: str
    status: str


class AskRequest(BaseModel):
    """A natural-language question scoped to one indexed repository."""

    model_config = ConfigDict(extra="forbid")

    repo_id: str = Field(min_length=1)
    question: str = Field(min_length=1)
