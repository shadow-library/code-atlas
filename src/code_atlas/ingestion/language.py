"""Language detection for source files via extension and shebang heuristics."""

from __future__ import annotations

from pathlib import Path

__all__ = ["detect_language"]

_EXTENSION_TO_LANGUAGE: dict[str, str] = {
    ".py": "python",
    ".pyi": "python",
    ".js": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".jsx": "javascript",
    ".ts": "typescript",
    ".mts": "typescript",
    ".cts": "typescript",
    ".tsx": "tsx",
    ".go": "go",
    ".java": "java",
    ".rs": "rust",
    ".c": "c",
    ".h": "c",
    ".cc": "cpp",
    ".cpp": "cpp",
    ".cxx": "cpp",
    ".c++": "cpp",
    ".hpp": "cpp",
    ".hh": "cpp",
    ".hxx": "cpp",
    ".h++": "cpp",
}

_SHEBANG_INTERPRETER_TO_LANGUAGE: dict[str, str] = {
    "python": "python",
    "python2": "python",
    "python3": "python",
    "node": "javascript",
    "ts-node": "typescript",
    "deno": "typescript",
    "bun": "typescript",
    "go": "go",
    "java": "java",
    "rustc": "rust",
}


def _parse_shebang(line: str) -> str | None:
    """Return the interpreter basename from a shebang line, or None if invalid."""
    stripped = line.rstrip("\r").strip()
    if not stripped.startswith("#!"):
        return None
    remainder = stripped[2:].strip()
    if not remainder:
        return None
    tokens = remainder.split()
    first = tokens[0]
    first_base = first.rsplit("/", 1)[-1]
    if first_base == "env":
        if len(tokens) < 2:
            return None
        interpreter = tokens[1]
        return interpreter.rsplit("/", 1)[-1]
    return first_base


def _lookup_interpreter(basename: str) -> str | None:
    """Resolve a shebang interpreter basename to a language name, if known."""
    if basename in _SHEBANG_INTERPRETER_TO_LANGUAGE:
        return _SHEBANG_INTERPRETER_TO_LANGUAGE[basename]
    trimmed = basename.rstrip("0123456789.")
    if trimmed and trimmed != basename and trimmed in _SHEBANG_INTERPRETER_TO_LANGUAGE:
        return _SHEBANG_INTERPRETER_TO_LANGUAGE[trimmed]
    return None


def _first_nonempty_line(text: str) -> str | None:
    """Return the first non-empty stripped line of the given text, or None."""
    for raw in text.splitlines():
        stripped = raw.strip()
        if stripped:
            return raw
    return None


def detect_language(path: Path, content: str | None = None) -> str | None:
    """Detect a tree-sitter-language-pack name for the file, or None if unknown.

    Resolution order:
    1. If the path has a known extension (case-insensitive), return that mapping.
    2. Otherwise, examine the shebang line: from ``content`` if provided,
       else by reading the first line of the file.
    3. Otherwise return None.
    """
    suffix = path.suffix.lower()
    if suffix in _EXTENSION_TO_LANGUAGE:
        return _EXTENSION_TO_LANGUAGE[suffix]

    if content is None:
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None

    first_line = _first_nonempty_line(content)
    if first_line is None:
        return None
    basename = _parse_shebang(first_line)
    if basename is None:
        return None
    return _lookup_interpreter(basename)
