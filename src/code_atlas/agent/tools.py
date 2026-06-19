"""LLM-callable tools bound to a single repo_id, plus JSON schemas for the LLM."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from code_atlas.domain.chunk import Symbol
from code_atlas.errors import AgentError
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.providers.base import ToolSpec
from code_atlas.utils import get_logger

__all__ = ["ToolResult", "Toolbox"]

log = get_logger(__name__)

ToolResult = dict[str, Any]

_OPEN_FILE_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "path": {"type": "string", "description": "Repo-relative posix path of the source file."},
        "start_line": {"type": "integer", "minimum": 1, "description": "First line to include (1-indexed)."},
        "end_line": {"type": "integer", "minimum": 1, "description": "Last line to include (inclusive)."},
    },
    "required": ["path", "start_line", "end_line"],
    "additionalProperties": False,
}

_FIND_SYMBOL_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "name": {"type": "string", "description": "Symbol name to find (case-sensitive)."},
        "kind": {
            "type": "string",
            "enum": ["function", "method", "class", "module", "variable", "constant", "other"],
            "description": "Optional kind filter.",
        },
    },
    "required": ["name"],
    "additionalProperties": False,
}

_LIST_CALLERS_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "symbol_name": {"type": "string", "description": "Symbol name to find callers of."},
    },
    "required": ["symbol_name"],
    "additionalProperties": False,
}

_LIST_CALLEES_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "symbol_name": {"type": "string", "description": "Symbol name to find callees of."},
    },
    "required": ["symbol_name"],
    "additionalProperties": False,
}


def _symbol_to_dict(s: Symbol) -> dict[str, Any]:
    return {"name": s.name, "kind": s.kind, "path": s.path, "line": s.line, "parent": s.parent}


class Toolbox:
    """Bundle of LLM-callable tools bound to a specific repo_id."""

    def __init__(
        self,
        *,
        metadata_store: MetadataStore,
        symbol_graph: SymbolGraph,
        repo_id: str,
    ) -> None:
        if not repo_id:
            raise AgentError("toolbox: repo_id is required", context={"repo_id": repo_id})
        self._metadata = metadata_store
        self._graph = symbol_graph
        self._repo_id = repo_id

    @property
    def specs(self) -> list[ToolSpec]:
        return [
            ToolSpec(
                name="open_file",
                description=(
                    "Read a slice of an indexed source file. Returns chunks overlapping "
                    "the given line range, with content, kind, and symbol."
                ),
                parameters=_OPEN_FILE_SCHEMA,
            ),
            ToolSpec(
                name="find_symbol",
                description=(
                    "Find symbols (functions, classes, methods, etc.) by name across the "
                    "indexed repository. Optionally filter by kind."
                ),
                parameters=_FIND_SYMBOL_SCHEMA,
            ),
            ToolSpec(
                name="list_callers",
                description=(
                    "List all symbols that call the named symbol (across all files). "
                    "Returns empty list if symbol not in graph."
                ),
                parameters=_LIST_CALLERS_SCHEMA,
            ),
            ToolSpec(
                name="list_callees",
                description=("List all symbols called by the named symbol. Returns empty list if symbol not in graph."),
                parameters=_LIST_CALLEES_SCHEMA,
            ),
        ]

    @property
    def callables(self) -> dict[str, Callable[..., dict[str, Any]]]:
        return {
            "open_file": self.open_file,
            "find_symbol": self.find_symbol,
            "list_callers": self.list_callers,
            "list_callees": self.list_callees,
        }

    def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        """Dispatch a tool by name. Unknown name or bad args raise AgentError."""
        log.info("toolbox.call", tool=name, repo_id=self._repo_id)
        funcs = self.callables
        func = funcs.get(name)
        if func is None:
            raise AgentError(
                "toolbox: unknown tool",
                context={"name": name, "available": sorted(funcs)},
            )
        try:
            return func(**arguments)
        except AgentError:
            raise
        except TypeError as exc:
            raise AgentError(
                "toolbox: invalid arguments",
                context={"tool": name, "error": str(exc)},
            ) from exc

    def open_file(self, *, path: str, start_line: int, end_line: int) -> dict[str, Any]:
        if not path:
            raise AgentError("open_file: path is required", context={"path": path})
        if start_line < 1 or end_line < start_line:
            raise AgentError(
                "open_file: invalid line range",
                context={"start_line": start_line, "end_line": end_line},
            )
        log.debug("toolbox.open_file", repo_id=self._repo_id, path=path, start_line=start_line, end_line=end_line)
        chunks = self._metadata.find_by_path(self._repo_id, path)
        matched = [c for c in chunks if c.start_line <= end_line and c.end_line >= start_line]
        return {
            "path": path,
            "start_line": start_line,
            "end_line": end_line,
            "chunks": [
                {
                    "chunk_id": c.chunk_id,
                    "start_line": c.start_line,
                    "end_line": c.end_line,
                    "kind": c.kind,
                    "symbol": c.symbol,
                    "content": c.content,
                }
                for c in matched
            ],
        }

    def find_symbol(self, *, name: str, kind: str | None = None) -> dict[str, Any]:
        if not name:
            raise AgentError("find_symbol: name is required", context={"name": name})
        log.debug("toolbox.find_symbol", repo_id=self._repo_id, name=name, kind=kind)
        matches = self._graph.find_by_name(name, kind=kind)
        return {"name": name, "results": [_symbol_to_dict(s) for s in matches]}

    def list_callers(self, *, symbol_name: str) -> dict[str, Any]:
        if not symbol_name:
            raise AgentError("list_callers: symbol_name is required", context={"symbol_name": symbol_name})
        log.debug("toolbox.list_callers", repo_id=self._repo_id, symbol_name=symbol_name)
        return {"symbol": symbol_name, "results": self._aggregate_neighbors(symbol_name, direction="in")}

    def list_callees(self, *, symbol_name: str) -> dict[str, Any]:
        if not symbol_name:
            raise AgentError("list_callees: symbol_name is required", context={"symbol_name": symbol_name})
        log.debug("toolbox.list_callees", repo_id=self._repo_id, symbol_name=symbol_name)
        return {"symbol": symbol_name, "results": self._aggregate_neighbors(symbol_name, direction="out")}

    def _aggregate_neighbors(self, symbol_name: str, *, direction: str) -> list[dict[str, Any]]:
        anchors = self._graph.find_by_name(symbol_name)
        if not anchors:
            return []
        seen: dict[tuple[str, str], Symbol] = {}
        for anchor in anchors:
            neighbors = self._graph.callers(anchor) if direction == "in" else self._graph.callees(anchor)
            for n in neighbors:
                seen.setdefault((n.path, n.name), n)
        ordered = sorted(seen.values(), key=lambda s: (s.path, s.name))
        return [_symbol_to_dict(s) for s in ordered]
