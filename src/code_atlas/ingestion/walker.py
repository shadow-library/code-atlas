"""Gitignore-aware repository walker."""

from __future__ import annotations

import os
from collections.abc import Iterator
from pathlib import Path

from pathspec import GitIgnoreSpec

from code_atlas.errors import IngestionError
from code_atlas.utils import get_logger

__all__ = ["walk_repo"]

log = get_logger(__name__)

_BINARY_SNIFF_BYTES = 8192
_BASELINE_IGNORE = (
    ".git/",
    "node_modules/",
    ".venv/",
    "venv/",
    "__pycache__/",
    "dist/",
    "build/",
    "*.lock",
    "*.pyc",
    "*.pyo",
    "*.so",
    "*.dylib",
    "*.dll",
)


def _load_gitignore_lines(dir_path: Path) -> list[str]:
    """Return the raw lines of ``dir_path/.gitignore`` or an empty list."""
    gi = dir_path / ".gitignore"
    if not gi.is_file():
        return []
    text = gi.read_text(encoding="utf-8", errors="replace")
    return text.splitlines()


def _is_binary(path: Path) -> bool:
    """Sniff the first chunk of a file for a null byte; treat unreadable as binary."""
    try:
        with path.open("rb") as handle:
            chunk = handle.read(_BINARY_SNIFF_BYTES)
    except OSError as exc:
        log.debug("walker.read_failed", path=str(path), error=str(exc))
        return True
    return b"\x00" in chunk


def _compose_spec(lines: list[str]) -> GitIgnoreSpec:
    """Build a GitIgnoreSpec from accumulated pattern lines."""
    return GitIgnoreSpec.from_lines(lines)


def _read_git_info_exclude(root: Path) -> list[str]:
    """Read ``<root>/.git/info/exclude`` filtering blanks and comments."""
    exclude = root / ".git" / "info" / "exclude"
    if not exclude.is_file():
        return []
    out: list[str] = []
    for raw in exclude.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        out.append(raw)
    return out


def _walk(root: Path, current_dir: Path, accumulated_lines: list[str]) -> Iterator[Path]:
    """Recurse ``current_dir`` yielding text files not matched by the active spec."""
    dir_lines = _load_gitignore_lines(current_dir)
    active_lines = accumulated_lines + dir_lines
    spec = _compose_spec(active_lines)

    try:
        entries = list(os.scandir(current_dir))
    except OSError as exc:
        log.warning("walker.scandir_failed", path=str(current_dir), error=str(exc))
        return

    for entry in entries:
        entry_path = Path(entry.path)
        try:
            rel = entry_path.relative_to(root).as_posix()
        except ValueError:
            continue
        is_dir = entry.is_dir(follow_symlinks=False)
        match_key = f"{rel}/" if is_dir else rel
        if spec.match_file(match_key):
            continue
        if is_dir:
            yield from _walk(root, entry_path, active_lines)
            continue
        if not entry.is_file(follow_symlinks=False):
            continue
        if _is_binary(entry_path):
            continue
        yield entry_path.resolve()


def walk_repo(root: Path, extra_ignores: list[str] | None = None) -> Iterator[Path]:
    """Yield text files under ``root`` honoring gitignore-style rules."""
    root = root.resolve()
    if not root.is_dir():
        raise IngestionError("walk_repo root is not a directory", context={"root": str(root)})

    initial_lines: list[str] = list(_BASELINE_IGNORE)
    if extra_ignores:
        initial_lines.extend(extra_ignores)
    initial_lines.extend(_read_git_info_exclude(root))

    yield from _walk(root, root, initial_lines)
