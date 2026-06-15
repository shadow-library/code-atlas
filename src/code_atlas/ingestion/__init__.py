"""Repo ingestion: walking, language detection, parsing, pipeline."""

from code_atlas.ingestion.language import detect_language
from code_atlas.ingestion.walker import walk_repo

__all__ = ["detect_language", "walk_repo"]
