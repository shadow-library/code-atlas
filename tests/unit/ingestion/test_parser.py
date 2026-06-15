"""Tests for the tree-sitter AST chunker with fixed-window fallback."""

from __future__ import annotations

import textwrap

from code_atlas.domain import CodeChunk
from code_atlas.ingestion import chunk_file


def test_python_two_functions_yields_two_chunks() -> None:
    src = textwrap.dedent(
        """\
        def foo() -> int:
            return 1


        def bar() -> str:
            return "x"
        """
    )
    chunks = chunk_file(path="src/x.py", repo_id="repo1", language="python", content=src)
    symbols = sorted(c.symbol for c in chunks if c.symbol)
    assert symbols == ["bar", "foo"]
    assert all(c.kind == "function" for c in chunks)
    assert all(c.language == "python" for c in chunks)


def test_python_class_with_methods_yields_class_and_methods() -> None:
    src = textwrap.dedent(
        """\
        class Foo:
            def a(self):
                pass

            def b(self):
                pass
        """
    )
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    kinds = sorted(c.kind for c in chunks)
    symbols = sorted(c.symbol for c in chunks if c.symbol)
    assert "class" in kinds
    assert kinds.count("method") == 2
    assert "Foo" in symbols
    assert "a" in symbols and "b" in symbols


def test_python_decorated_function_uses_inner_identifier() -> None:
    src = textwrap.dedent(
        """\
        import functools

        @functools.lru_cache
        def cached() -> int:
            return 42
        """
    )
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    assert any(c.symbol == "cached" for c in chunks)


def test_python_no_definitions_yields_whole_file_chunk() -> None:
    src = "print('hi')\n"
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    assert len(chunks) == 1
    assert chunks[0].kind == "file"
    assert chunks[0].symbol is None
    assert chunks[0].start_line == 1


def test_unknown_language_uses_fixed_window() -> None:
    src = "\n".join(f"line {i}" for i in range(1, 121)) + "\n"
    chunks = chunk_file(path="src/x.hs", repo_id="r", language="haskell", content=src)
    assert chunks, "expected at least one chunk"
    assert all(c.kind == "block" for c in chunks)
    assert chunks[0].start_line == 1
    assert chunks[-1].end_line == 120


def test_fixed_window_overlap_is_5_default() -> None:
    src = "\n".join(f"line {i}" for i in range(1, 101)) + "\n"
    chunks = chunk_file(path="src/x.unknown", repo_id="r", language="unknown", content=src)
    assert len(chunks) == 3
    assert [c.start_line for c in chunks] == [1, 46, 91]


def test_empty_content_yields_no_chunks() -> None:
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content="")
    assert chunks == []


def test_whitespace_only_content_yields_no_chunks() -> None:
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content="   \n   \n")
    assert chunks == []


def test_content_hash_stable_across_runs() -> None:
    src = textwrap.dedent(
        """\
        def foo() -> int:
            return 1
        """
    )
    first = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    second = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    assert [c.chunk_id for c in first] == [c.chunk_id for c in second]
    assert [c.content_hash for c in first] == [c.content_hash for c in second]


def test_python_chunk_content_matches_source_slice() -> None:
    src = textwrap.dedent(
        """\
        def foo() -> int:
            return 1


        def bar() -> str:
            return "x"
        """
    )
    lines = src.splitlines()
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    assert chunks
    for chunk in chunks:
        expected = "\n".join(lines[chunk.start_line - 1 : chunk.end_line])
        assert chunk.content.rstrip("\n") == expected.rstrip("\n")


def test_chunks_are_code_chunk_instances() -> None:
    src = "def f():\n    return 0\n"
    chunks = chunk_file(path="src/x.py", repo_id="r", language="python", content=src)
    assert chunks
    assert all(isinstance(c, CodeChunk) for c in chunks)
