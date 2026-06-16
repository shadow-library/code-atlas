"""Repo ingestion: walking, language detection, parsing, pipeline."""

from code_atlas.ingestion.language import detect_language
from code_atlas.ingestion.parser import chunk_file
from code_atlas.ingestion.pipeline import IngestStats, ingest_repo
from code_atlas.ingestion.walker import walk_repo

__all__ = ["IngestStats", "chunk_file", "detect_language", "ingest_repo", "walk_repo"]
