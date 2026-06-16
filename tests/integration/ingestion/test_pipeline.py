"""Integration tests for the ingest_repo pipeline."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from code_atlas.errors import IngestionError
from code_atlas.ingestion import IngestStats, ingest_repo


def _make_tree(root: Path, layout: dict[str, str | bytes]) -> None:
    for rel, content in layout.items():
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8")


def _simple_repo(root: Path) -> None:
    _make_tree(
        root,
        {
            "src/a.py": "def alpha():\n    return 1\n",
            "src/b.py": "def beta():\n    return 2\n\n\ndef gamma():\n    return 3\n",
            "src/notes.txt": "plain notes — no language\n",
            "README.md": "# repo\n",
            ".gitignore": "*.lock\n",
        },
    )


def test_emits_chunks_for_python_files(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    chunks = list(ingest_repo(tmp_path, "repo1"))
    assert len(chunks) == 3
    symbols = {chunk.symbol for chunk in chunks}
    assert symbols == {"alpha", "beta", "gamma"}
    assert all(chunk.language == "python" for chunk in chunks)


def test_repo_id_propagates(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    chunks = list(ingest_repo(tmp_path, "myrepo"))
    assert chunks
    assert all(chunk.repo_id == "myrepo" for chunk in chunks)


def test_relative_paths(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    chunks = list(ingest_repo(tmp_path, "repo1"))
    assert chunks
    for chunk in chunks:
        assert chunk.path.startswith("src/")
        assert not chunk.path.startswith("/")


def test_stats_counters(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    stats = IngestStats()
    list(ingest_repo(tmp_path, "repo1", stats=stats))
    assert stats.files_seen >= 4
    assert stats.files_chunked == 2
    assert stats.chunks_emitted == 3
    assert stats.files_skipped_no_language == 3  # notes.txt + README.md + .gitignore
    assert stats.files_skipped_unreadable == 0
    assert stats.files_skipped_unchanged == 0


def test_mtime_cache_skips_unchanged(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    cache: dict[str, tuple[float, int]] = {}
    list(ingest_repo(tmp_path, "repo1", mtime_cache=cache))
    assert cache

    second_stats = IngestStats()
    second = list(ingest_repo(tmp_path, "repo1", mtime_cache=cache, stats=second_stats))
    assert second == []
    assert second_stats.chunks_emitted == 0
    assert second_stats.files_skipped_unchanged >= 2


def test_mtime_cache_picks_up_changes(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    cache: dict[str, tuple[float, int]] = {}
    list(ingest_repo(tmp_path, "repo1", mtime_cache=cache))

    target = tmp_path / "src" / "a.py"
    target.write_text("def alpha():\n    return 42\n", encoding="utf-8")
    bumped = target.stat().st_mtime + 5
    os.utime(target, (bumped, bumped))

    second_stats = IngestStats()
    second = list(ingest_repo(tmp_path, "repo1", mtime_cache=cache, stats=second_stats))
    paths = {chunk.path for chunk in second}
    assert "src/a.py" in paths
    assert "src/b.py" not in paths


def test_missing_root_raises_ingestion_error_eagerly(tmp_path: Path) -> None:
    with pytest.raises(IngestionError):
        ingest_repo(tmp_path / "nope", "r")


def test_empty_repo_id_raises_ingestion_error(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    with pytest.raises(IngestionError):
        ingest_repo(tmp_path, "")


def test_lazy_iteration(tmp_path: Path) -> None:
    _simple_repo(tmp_path)
    iterator = iter(ingest_repo(tmp_path, "repo1"))
    first = next(iterator)
    assert first.repo_id == "repo1"
