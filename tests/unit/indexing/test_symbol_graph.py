"""Unit tests for SymbolGraph."""

from __future__ import annotations

from pathlib import Path

import pytest

from code_atlas.domain.chunk import Symbol
from code_atlas.errors import IndexingError
from code_atlas.indexing.symbol_graph import SymbolGraph


def _sym(name: str, path: str = "pkg/mod.py", kind: str = "function", line: int = 1) -> Symbol:
    return Symbol(name=name, kind=kind, path=path, line=line)  # type: ignore[arg-type]


def test_add_symbol_roundtrip_and_has_symbol() -> None:
    g = SymbolGraph()
    s = _sym("foo")
    g.add_symbol(s)
    assert g.has_symbol(s)
    assert len(g) == 1


def test_add_symbol_idempotent() -> None:
    g = SymbolGraph()
    s = _sym("foo")
    g.add_symbol(s)
    g.add_symbol(s)
    g.add_symbol(s)
    assert len(g) == 1


def test_add_edge_auto_adds_endpoints() -> None:
    g = SymbolGraph()
    a, b = _sym("a"), _sym("b")
    g.add_edge(a, b, "calls")
    assert g.has_symbol(a)
    assert g.has_symbol(b)
    assert g.edge_count() == 1


def test_add_edge_all_kinds_between_same_pair() -> None:
    g = SymbolGraph()
    a, b = _sym("a"), _sym("b")
    for k in ("calls", "imports", "defines", "contained_in"):
        g.add_edge(a, b, k)  # type: ignore[arg-type]
    assert g.edge_count() == 4


def test_add_edge_same_kind_idempotent() -> None:
    g = SymbolGraph()
    a, b = _sym("a"), _sym("b")
    g.add_edge(a, b, "calls")
    g.add_edge(a, b, "calls")
    g.add_edge(a, b, "calls")
    assert g.edge_count() == 1


def test_add_edge_unknown_kind_raises() -> None:
    g = SymbolGraph()
    a, b = _sym("a"), _sym("b")
    with pytest.raises(IndexingError) as exc_info:
        g.add_edge(a, b, "frobnicates")  # type: ignore[arg-type]
    assert exc_info.value.context.get("kind") == "frobnicates"
    assert "calls" in exc_info.value.context.get("valid_kinds", [])


def test_callers_returns_only_calls_sources() -> None:
    g = SymbolGraph()
    a, b, c = _sym("a"), _sym("b"), _sym("c")
    g.add_edge(a, b, "calls")
    g.add_edge(c, b, "imports")
    callers = g.callers(b)
    assert callers == [a]


def test_callees_returns_only_calls_targets() -> None:
    g = SymbolGraph()
    a, b, c = _sym("a"), _sym("b"), _sym("c")
    g.add_edge(a, b, "calls")
    g.add_edge(a, c, "imports")
    callees = g.callees(a)
    assert callees == [b]


def test_callers_callees_missing_symbol_returns_empty() -> None:
    g = SymbolGraph()
    ghost = _sym("ghost", path="nope.py")
    assert g.callers(ghost) == []
    assert g.callees(ghost) == []


def test_callers_callees_sort_order_deterministic() -> None:
    g = SymbolGraph()
    target = _sym("target", path="z/t.py")
    c1 = _sym("alpha", path="a/x.py")
    c2 = _sym("beta", path="a/x.py")
    c3 = _sym("gamma", path="b/y.py")
    g.add_edge(c3, target, "calls")
    g.add_edge(c1, target, "calls")
    g.add_edge(c2, target, "calls")
    callers = g.callers(target)
    assert [(s.path, s.name) for s in callers] == [
        ("a/x.py", "alpha"),
        ("a/x.py", "beta"),
        ("b/y.py", "gamma"),
    ]


def test_save_creates_gzipped_file(tmp_path: Path) -> None:
    g = SymbolGraph()
    g.add_edge(_sym("a"), _sym("b"), "calls")
    target = tmp_path / "graph.json.gz"
    g.save(target)
    blob = target.read_bytes()
    assert blob[:2] == b"\x1f\x8b"


def test_edge_count_and_len_totals() -> None:
    g = SymbolGraph()
    a, b, c = _sym("a"), _sym("b"), _sym("c")
    g.add_edge(a, b, "calls")
    g.add_edge(a, b, "imports")
    g.add_edge(b, c, "defines")
    assert len(g) == 3
    assert g.edge_count() == 3


def test_save_to_missing_parent_dir_raises(tmp_path: Path) -> None:
    g = SymbolGraph()
    g.add_symbol(_sym("a"))
    bad = tmp_path / "does_not_exist" / "graph.json.gz"
    with pytest.raises(IndexingError):
        g.save(bad)


def test_load_corrupted_file_raises(tmp_path: Path) -> None:
    bad = tmp_path / "bad.json.gz"
    bad.write_bytes(b"this is not gzip")
    with pytest.raises(IndexingError):
        SymbolGraph.load(bad)


def test_save_load_roundtrip_preserves_everything(tmp_path: Path) -> None:
    g = SymbolGraph()
    a = _sym("a", path="p/a.py", line=10)
    b = _sym("b", path="p/b.py", line=20)
    g.add_edge(a, b, "calls")
    g.add_edge(a, b, "imports")
    g.add_symbol(_sym("solo", path="p/solo.py"))
    target = tmp_path / "graph.json.gz"
    g.save(target)
    loaded = SymbolGraph.load(target)
    assert len(loaded) == len(g) == 3
    assert loaded.edge_count() == g.edge_count() == 2
    assert loaded.has_symbol(a)
    assert loaded.has_symbol(b)
    assert loaded.callees(a) == [b]
    assert loaded.callers(b) == [a]
    restored = loaded.callees(a)[0]
    assert restored.line == 20
    assert restored.path == "p/b.py"


def test_acceptance_five_symbols_six_edges_roundtrip(tmp_path: Path) -> None:
    g = SymbolGraph()
    mod = _sym("mod", path="pkg/m.py", kind="module", line=1)
    cls = _sym("Cls", path="pkg/m.py", kind="class", line=5)
    f1 = _sym("alpha", path="pkg/m.py", kind="function", line=10)
    f2 = _sym("beta", path="pkg/m.py", kind="function", line=20)
    f3 = _sym("gamma", path="pkg/other.py", kind="function", line=3)

    g.add_edge(mod, cls, "defines")
    g.add_edge(mod, f1, "defines")
    g.add_edge(cls, f2, "contained_in")
    g.add_edge(f1, f2, "calls")
    g.add_edge(f1, f3, "calls")
    g.add_edge(mod, f3, "imports")

    before_len = len(g)
    before_edges = g.edge_count()
    before_callees_f1 = g.callees(f1)
    before_callers_f2 = g.callers(f2)
    before_callers_f3 = g.callers(f3)

    assert before_len == 5
    assert before_edges == 6

    target = tmp_path / "acceptance.json.gz"
    g.save(target)
    loaded = SymbolGraph.load(target)

    assert len(loaded) == before_len
    assert loaded.edge_count() == before_edges
    assert loaded.callees(f1) == before_callees_f1
    assert loaded.callers(f2) == before_callers_f2
    assert loaded.callers(f3) == before_callers_f3
    assert [s.name for s in loaded.callees(f1)] == ["beta", "gamma"]


def test_find_by_name_returns_matches_sorted() -> None:
    # Node key is (path, name), so each foo must live in a distinct path to coexist.
    g = SymbolGraph()
    g.add_symbol(_sym("foo", path="z/last.py", line=5))
    g.add_symbol(_sym("foo", path="m/mid.py", line=10))
    g.add_symbol(_sym("foo", path="a/first.py", line=2))
    g.add_symbol(_sym("other", path="a/first.py", line=1))
    matches = g.find_by_name("foo")
    assert [(s.path, s.line) for s in matches] == [
        ("a/first.py", 2),
        ("m/mid.py", 10),
        ("z/last.py", 5),
    ]


def test_find_by_name_filters_by_kind() -> None:
    g = SymbolGraph()
    g.add_symbol(_sym("foo", path="a.py", kind="function", line=1))
    g.add_symbol(_sym("foo", path="b.py", kind="class", line=2))
    g.add_symbol(_sym("foo", path="c.py", kind="method", line=3))
    matches = g.find_by_name("foo", kind="class")
    assert [(s.path, s.kind) for s in matches] == [("b.py", "class")]
    assert g.find_by_name("foo", kind="constant") == []
