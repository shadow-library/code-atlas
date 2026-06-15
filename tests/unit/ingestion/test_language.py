"""Tests for source-file language detection."""

from __future__ import annotations

from pathlib import Path

import pytest

from code_atlas.ingestion import detect_language


@pytest.mark.parametrize(
    ("ext", "expected"),
    [
        (".py", "python"),
        (".pyi", "python"),
        (".js", "javascript"),
        (".mjs", "javascript"),
        (".cjs", "javascript"),
        (".jsx", "javascript"),
        (".ts", "typescript"),
        (".mts", "typescript"),
        (".cts", "typescript"),
        (".tsx", "tsx"),
        (".go", "go"),
        (".java", "java"),
        (".rs", "rust"),
        (".c", "c"),
        (".h", "c"),
        (".cc", "cpp"),
        (".cpp", "cpp"),
        (".cxx", "cpp"),
        (".c++", "cpp"),
        (".hpp", "cpp"),
        (".hh", "cpp"),
        (".hxx", "cpp"),
        (".h++", "cpp"),
    ],
)
def test_extension_table(ext: str, expected: str) -> None:
    assert detect_language(Path(f"src/file{ext}")) == expected


def test_extension_case_insensitive() -> None:
    assert detect_language(Path("src/X.PY")) == "python"
    assert detect_language(Path("src/X.Tsx")) == "tsx"


def test_unknown_extension_returns_none() -> None:
    assert detect_language(Path("src/notes.unknown")) is None


def test_no_extension_no_shebang_returns_none(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("hello\n", encoding="utf-8")
    assert detect_language(target) is None


def test_shebang_env_python(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/env python3\necho hi\n", encoding="utf-8")
    assert detect_language(target) == "python"


def test_shebang_absolute_python(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/python\n", encoding="utf-8")
    assert detect_language(target) == "python"


def test_shebang_env_node(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/env node\n", encoding="utf-8")
    assert detect_language(target) == "javascript"


def test_shebang_with_versioned_interpreter(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/env python3.11\n", encoding="utf-8")
    assert detect_language(target) == "python"


def test_shebang_unknown_interpreter_returns_none(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/env perl\n", encoding="utf-8")
    assert detect_language(target) is None


def test_content_arg_skips_filesystem_read(tmp_path: Path) -> None:
    missing = tmp_path / "does-not-exist"
    assert detect_language(missing, content="#!/usr/bin/env python\n") == "python"


def test_unreadable_file_returns_none() -> None:
    assert detect_language(Path("/no/such/file/anywhere")) is None


def test_extension_wins_over_shebang(tmp_path: Path) -> None:
    target = tmp_path / "script.py"
    target.write_text("#!/usr/bin/env node\nprint('hi')\n", encoding="utf-8")
    assert detect_language(target) == "python"


def test_shebang_with_carriage_return(tmp_path: Path) -> None:
    target = tmp_path / "cmd"
    target.write_text("#!/usr/bin/env python\r\n", encoding="utf-8")
    assert detect_language(target) == "python"
