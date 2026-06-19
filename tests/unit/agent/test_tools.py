"""Unit tests for the agent Toolbox."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from code_atlas.agent.tools import Toolbox
from code_atlas.domain.chunk import CodeChunk, Symbol
from code_atlas.errors import AgentError
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph


def _chunk(
    cid: str,
    *,
    repo_id: str = "r1",
    path: str = "a.py",
    start: int = 1,
    end: int = 5,
    symbol: str | None = None,
    kind: str = "function",
    content: str = "body",
) -> CodeChunk:
    return CodeChunk(
        chunk_id=cid,
        repo_id=repo_id,
        path=path,
        language="python",
        kind=kind,  # type: ignore[arg-type]
        symbol=symbol,
        start_line=start,
        end_line=end,
        content=content,
        content_hash="h" * 16,
    )


def _sym(name: str, *, path: str = "a.py", kind: str = "function", line: int = 1) -> Symbol:
    return Symbol(name=name, kind=kind, path=path, line=line)  # type: ignore[arg-type]


@pytest.fixture
def store(tmp_path: Path) -> Iterator[MetadataStore]:
    s = MetadataStore(f"sqlite:///{tmp_path / 'm.sqlite'}")
    try:
        yield s
    finally:
        s.close()


@pytest.fixture
def graph() -> SymbolGraph:
    return SymbolGraph()


@pytest.fixture
def toolbox(store: MetadataStore, graph: SymbolGraph) -> Toolbox:
    return Toolbox(metadata_store=store, symbol_graph=graph, repo_id="r1")


def test_open_file_returns_overlapping_chunks(store: MetadataStore, toolbox: Toolbox) -> None:
    store.upsert_many(
        [
            _chunk("c1", start=1, end=10),
            _chunk("c2", start=11, end=20),
            _chunk("c3", start=21, end=30),
        ]
    )
    result = toolbox.open_file(path="a.py", start_line=8, end_line=15)
    assert result["path"] == "a.py"
    assert result["start_line"] == 8
    assert result["end_line"] == 15
    assert [c["chunk_id"] for c in result["chunks"]] == ["c1", "c2"]


def test_open_file_invalid_range_raises_agent_error(toolbox: Toolbox) -> None:
    with pytest.raises(AgentError) as exc:
        toolbox.open_file(path="a.py", start_line=10, end_line=5)
    assert exc.value.context.get("start_line") == 10
    assert exc.value.context.get("end_line") == 5


def test_open_file_empty_path_raises(toolbox: Toolbox) -> None:
    with pytest.raises(AgentError):
        toolbox.open_file(path="", start_line=1, end_line=5)


def test_open_file_no_match_returns_empty_chunks(toolbox: Toolbox) -> None:
    result = toolbox.open_file(path="ghost.py", start_line=1, end_line=10)
    assert result == {"path": "ghost.py", "start_line": 1, "end_line": 10, "chunks": []}


def test_open_file_respects_repo_id_binding(store: MetadataStore, toolbox: Toolbox) -> None:
    store.upsert(_chunk("foreign", repo_id="r2", path="a.py", start=1, end=5))
    result = toolbox.open_file(path="a.py", start_line=1, end_line=10)
    assert result["chunks"] == []


def test_find_symbol_round_trip(graph: SymbolGraph, toolbox: Toolbox) -> None:
    graph.add_symbol(_sym("foo", path="a.py", line=10))
    result = toolbox.find_symbol(name="foo")
    assert result["name"] == "foo"
    assert result["results"] == [{"name": "foo", "kind": "function", "path": "a.py", "line": 10, "parent": None}]


def test_find_symbol_kind_filter(graph: SymbolGraph, toolbox: Toolbox) -> None:
    graph.add_symbol(_sym("foo", path="a.py", kind="function", line=1))
    graph.add_symbol(_sym("foo", path="b.py", kind="class", line=2))
    result = toolbox.find_symbol(name="foo", kind="class")
    assert [r["kind"] for r in result["results"]] == ["class"]
    assert [r["path"] for r in result["results"]] == ["b.py"]


def test_find_symbol_unknown_returns_empty_no_error(toolbox: Toolbox) -> None:
    assert toolbox.find_symbol(name="ghost") == {"name": "ghost", "results": []}


def test_find_symbol_empty_name_raises(toolbox: Toolbox) -> None:
    with pytest.raises(AgentError):
        toolbox.find_symbol(name="")


def test_list_callers_round_trip(graph: SymbolGraph, toolbox: Toolbox) -> None:
    f1 = _sym("f1", path="a.py", line=1)
    f2 = _sym("f2", path="b.py", line=2)
    graph.add_edge(f1, f2, "calls")
    result = toolbox.list_callers(symbol_name="f2")
    assert result["symbol"] == "f2"
    assert [(r["name"], r["path"]) for r in result["results"]] == [("f1", "a.py")]


def test_list_callees_round_trip(graph: SymbolGraph, toolbox: Toolbox) -> None:
    f1 = _sym("f1", path="a.py", line=1)
    f2 = _sym("f2", path="b.py", line=2)
    graph.add_edge(f1, f2, "calls")
    result = toolbox.list_callees(symbol_name="f1")
    assert result["symbol"] == "f1"
    assert [(r["name"], r["path"]) for r in result["results"]] == [("f2", "b.py")]


def test_list_callers_unknown_symbol_returns_empty(toolbox: Toolbox) -> None:
    assert toolbox.list_callers(symbol_name="ghost") == {"symbol": "ghost", "results": []}
    assert toolbox.list_callees(symbol_name="ghost") == {"symbol": "ghost", "results": []}


def test_list_callers_dedupes_across_overloads(graph: SymbolGraph, toolbox: Toolbox) -> None:
    target_a = _sym("target", path="a.py", line=1)
    target_b = _sym("target", path="b.py", line=1)
    caller = _sym("caller", path="c.py", line=1)
    graph.add_edge(caller, target_a, "calls")
    graph.add_edge(caller, target_b, "calls")
    result = toolbox.list_callers(symbol_name="target")
    assert [(r["name"], r["path"]) for r in result["results"]] == [("caller", "c.py")]


def test_call_dispatches_by_name(store: MetadataStore, toolbox: Toolbox) -> None:
    store.upsert(_chunk("c1", start=1, end=5))
    direct = toolbox.open_file(path="a.py", start_line=1, end_line=5)
    via_call = toolbox.call("open_file", {"path": "a.py", "start_line": 1, "end_line": 5})
    assert via_call == direct


def test_call_unknown_tool_raises(toolbox: Toolbox) -> None:
    with pytest.raises(AgentError) as exc:
        toolbox.call("xyz", {})
    assert exc.value.context.get("name") == "xyz"
    assert set(exc.value.context.get("available", [])) == {
        "open_file",
        "find_symbol",
        "list_callers",
        "list_callees",
    }


def test_call_invalid_arguments_raises(toolbox: Toolbox) -> None:
    with pytest.raises(AgentError) as exc:
        toolbox.call("open_file", {"wrong_arg": 1})
    assert exc.value.context.get("tool") == "open_file"
    assert "error" in exc.value.context


def test_specs_returns_four_tool_specs(toolbox: Toolbox) -> None:
    specs = toolbox.specs
    assert len(specs) == 4
    assert {s.name for s in specs} == {"open_file", "find_symbol", "list_callers", "list_callees"}
    for s in specs:
        assert s.parameters.get("type") == "object"
        assert "properties" in s.parameters
        assert "required" in s.parameters
        assert s.description


def test_constructor_requires_repo_id(store: MetadataStore, graph: SymbolGraph) -> None:
    with pytest.raises(AgentError):
        Toolbox(metadata_store=store, symbol_graph=graph, repo_id="")
