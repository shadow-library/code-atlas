"""HTTP API package: FastAPI app exposing health, ingest, and Q&A endpoints."""

from __future__ import annotations

from code_atlas.api.app import app, create_app

__all__ = ["app", "create_app"]
