"""In-memory symbol graph over networkx.MultiDiGraph with gzipped JSON persistence."""

from __future__ import annotations

import gzip
import importlib
import json
from pathlib import Path
from typing import Any, Literal, cast, get_args

from code_atlas.domain.chunk import Symbol
from code_atlas.errors import IndexingError
from code_atlas.utils import get_logger

__all__ = ["EdgeKind", "SymbolGraph"]

EdgeKind = Literal["calls", "imports", "defines", "contained_in"]

_VALID_KINDS: tuple[str, ...] = get_args(EdgeKind)

log = get_logger(__name__)

nx: Any = importlib.import_module("networkx")


NodeKey = tuple[str, str]


def _key(symbol: Symbol) -> NodeKey:
    return (symbol.path, symbol.name)


class SymbolGraph:
    """Typed-edge symbol graph. Sync API; persistence is explicit via save/load."""

    def __init__(self) -> None:
        self._g: Any = nx.MultiDiGraph()

    def add_symbol(self, symbol: Symbol) -> None:
        key = _key(symbol)
        self._g.add_node(key, node_id=list(key), symbol_data=symbol.model_dump())

    def has_symbol(self, symbol: Symbol) -> bool:
        return bool(self._g.has_node(_key(symbol)))

    def add_edge(self, src: Symbol, dst: Symbol, kind: EdgeKind) -> None:
        if kind not in _VALID_KINDS:
            raise IndexingError(
                "symbol_graph: unknown edge kind",
                context={"kind": kind, "valid_kinds": list(_VALID_KINDS)},
            )
        self.add_symbol(src)
        self.add_symbol(dst)
        self._g.add_edge(_key(src), _key(dst), key=kind)

    def _neighbors_calls(self, symbol: Symbol, *, direction: Literal["in", "out"]) -> list[Symbol]:
        key = _key(symbol)
        if not self._g.has_node(key):
            return []
        if direction == "in":
            edge_view = self._g.in_edges(key, keys=True)
            picks = [u for u, _v, k in edge_view if k == "calls"]
        else:
            edge_view = self._g.out_edges(key, keys=True)
            picks = [v for _u, v, k in edge_view if k == "calls"]
        unique = {cast(NodeKey, n) for n in picks}
        ordered = sorted(unique, key=lambda nk: (nk[0], nk[1]))
        return [self._symbol_from_node(nk) for nk in ordered]

    def callers(self, symbol: Symbol) -> list[Symbol]:
        return self._neighbors_calls(symbol, direction="in")

    def callees(self, symbol: Symbol) -> list[Symbol]:
        return self._neighbors_calls(symbol, direction="out")

    def _symbol_from_node(self, key: NodeKey) -> Symbol:
        data = self._g.nodes[key].get("symbol_data")
        if not isinstance(data, dict):
            raise IndexingError(
                "symbol_graph: node missing symbol_data",
                context={"node_key": list(key)},
            )
        return Symbol(**data)

    def __len__(self) -> int:
        return int(self._g.number_of_nodes())

    def edge_count(self) -> int:
        return int(self._g.number_of_edges())

    def save(self, path: Path) -> None:
        target = Path(path)
        try:
            data = nx.node_link_data(self._g, edges="edges")
            payload = json.dumps(data, sort_keys=True).encode("utf-8")
            blob = gzip.compress(payload, compresslevel=9)
            target.write_bytes(blob)
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("symbol_graph.save_failed", path=str(target), error=str(exc))
            raise IndexingError(
                "symbol_graph: save failed",
                context={"path": str(target), "error": str(exc)},
            ) from exc

    @classmethod
    def load(cls, path: Path) -> SymbolGraph:
        source = Path(path)
        try:
            blob = source.read_bytes()
            payload = gzip.decompress(blob)
            data = json.loads(payload.decode("utf-8"))
            raw_graph = nx.node_link_graph(data, edges="edges", directed=True, multigraph=True)
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("symbol_graph.load_failed", path=str(source), error=str(exc))
            raise IndexingError(
                "symbol_graph: load failed",
                context={"path": str(source), "error": str(exc)},
            ) from exc

        rebuilt: Any = nx.MultiDiGraph()
        try:
            for raw_node, attrs in raw_graph.nodes(data=True):
                node_id = attrs.get("node_id", raw_node)
                key = cls._coerce_key(node_id)
                rebuilt.add_node(key, **attrs)
                rebuilt.nodes[key]["node_id"] = list(key)
            for u, v, k, attrs in raw_graph.edges(keys=True, data=True):
                u_attrs = raw_graph.nodes[u]
                v_attrs = raw_graph.nodes[v]
                u_key = cls._coerce_key(u_attrs.get("node_id", u))
                v_key = cls._coerce_key(v_attrs.get("node_id", v))
                rebuilt.add_edge(u_key, v_key, key=k, **attrs)
        except IndexingError:
            raise
        except Exception as exc:
            log.warning("symbol_graph.rebuild_failed", path=str(source), error=str(exc))
            raise IndexingError(
                "symbol_graph: rebuild after load failed",
                context={"path": str(source), "error": str(exc)},
            ) from exc

        out = cls()
        out._g = rebuilt
        return out

    @staticmethod
    def _coerce_key(raw: Any) -> NodeKey:
        if isinstance(raw, tuple) and len(raw) == 2:
            return (str(raw[0]), str(raw[1]))
        if isinstance(raw, list) and len(raw) == 2:
            return (str(raw[0]), str(raw[1]))
        raise IndexingError(
            "symbol_graph: invalid node id on load",
            context={"node_id": raw},
        )
