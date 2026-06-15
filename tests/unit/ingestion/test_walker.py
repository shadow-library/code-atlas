"""Tests for the gitignore-aware repo walker."""

from __future__ import annotations

from pathlib import Path

import pytest

from code_atlas.errors import IngestionError
from code_atlas.ingestion import walk_repo


def _make_tree(root: Path, layout: dict[str, str | bytes]) -> None:
    for rel, content in layout.items():
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8")


def _rel_set(root: Path, paths: list[Path]) -> set[str]:
    base = root.resolve()
    return {p.relative_to(base).as_posix() for p in paths}


def test_yields_only_text_files(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "a.py": "x = 1",
            "b.bin": b"binary\x00data\x00here",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {"a.py"}


def test_gitignore_excludes_listed_files(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            ".gitignore": "skip.txt\n",
            "keep.py": "x",
            "skip.txt": "ignored",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {".gitignore", "keep.py"}


def test_nested_gitignore_honored(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "keep.py": "x",
            "sub/.gitignore": "inner_skip.txt\n",
            "sub/inner_keep.py": "y",
            "sub/inner_skip.txt": "nope",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {"keep.py", "sub/.gitignore", "sub/inner_keep.py"}


def test_baseline_ignores_node_modules(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "node_modules/lib.js": "console.log(1)",
            "src/app.py": "x = 1",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {"src/app.py"}


def test_baseline_ignores_pycache(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "__pycache__/x.pyc": "compiled",
            "m.py": "x = 1",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {"m.py"}


def test_extra_ignores_applied(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "a.py": "x = 1",
            "b.py": "y = 2",
        },
    )
    result = list(walk_repo(tmp_path, extra_ignores=["b.py"]))
    assert _rel_set(tmp_path, result) == {"a.py"}


def test_git_info_exclude_honored(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            ".git/info/exclude": "secret.py\n",
            "secret.py": "shh = 1",
            "ok.py": "x = 1",
        },
    )
    result = list(walk_repo(tmp_path))
    assert _rel_set(tmp_path, result) == {"ok.py"}


def test_missing_root_raises_ingestion_error(tmp_path: Path) -> None:
    with pytest.raises(IngestionError):
        list(walk_repo(tmp_path / "does_not_exist"))


def test_root_is_file_raises_ingestion_error(tmp_path: Path) -> None:
    target = tmp_path / "file.txt"
    target.write_text("hi", encoding="utf-8")
    with pytest.raises(IngestionError):
        list(walk_repo(target))


def test_yields_absolute_paths(tmp_path: Path) -> None:
    _make_tree(
        tmp_path,
        {
            "a.py": "x = 1",
        },
    )
    result = list(walk_repo(tmp_path))
    assert result, "expected at least one path"
    for path in result:
        assert path.is_absolute(), f"not absolute: {path}"
