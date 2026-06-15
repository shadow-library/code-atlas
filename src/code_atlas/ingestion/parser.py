"""Tree-sitter AST chunker with a fixed-window fallback for unsupported languages."""

from __future__ import annotations

import hashlib
import importlib
from typing import Any

from code_atlas.domain.chunk import ChunkKind, CodeChunk
from code_atlas.utils import get_logger

__all__ = ["chunk_file"]

log = get_logger(__name__)

_FALLBACK_WINDOW_LINES = 50
_FALLBACK_OVERLAP_LINES = 5
_DEFAULT_MAX_CHUNK_LINES = 200


def _content_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _chunk_id(repo_id: str, path: str, start: int, end: int, content_hash: str) -> str:
    raw = f"{repo_id}\n{path}\n{start}-{end}\n{content_hash}".encode()
    return hashlib.sha256(raw).hexdigest()[:32]


def _slice_content(lines: list[str], start_1: int, end_1: int) -> str:
    return "\n".join(lines[start_1 - 1 : end_1]) + "\n"


def _clamp_range(start_0: int, end_0: int, total: int) -> tuple[int, int]:
    """Convert tree-sitter 0-indexed rows to 1-indexed inclusive lines, clamped to file extent."""
    last = max(1, total)
    start_1 = min(max(1, start_0 + 1), last)
    end_1 = min(max(start_1, end_0 + 1), last)
    return start_1, end_1


# Tree-sitter's Python bindings ship no type stubs; we narrow handles with `Any`.
def _node_identifier(node: Any, source: bytes) -> str | None:
    for child in node.children:
        if child.type == "identifier":
            return source[child.start_byte : child.end_byte].decode("utf-8")
    return None


def _decorated_inner(node: Any) -> Any | None:
    for child in node.children:
        if child.type in {"function_definition", "class_definition"}:
            return child
    return None


def _class_body(node: Any) -> Any | None:
    for child in node.children:
        if child.type == "block":
            return child
    return None


def _make_chunk(
    *,
    repo_id: str,
    path: str,
    language: str,
    kind: ChunkKind,
    symbol: str | None,
    start_line: int,
    end_line: int,
    chunk_content: str,
) -> CodeChunk:
    content_hash = _content_hash(chunk_content)
    return CodeChunk(
        chunk_id=_chunk_id(repo_id, path, start_line, end_line, content_hash),
        repo_id=repo_id,
        path=path,
        language=language,
        kind=kind,
        symbol=symbol,
        start_line=start_line,
        end_line=end_line,
        content=chunk_content,
        content_hash=content_hash,
    )


def _emit_def_chunk(
    node: Any,
    kind: ChunkKind,
    *,
    repo_id: str,
    path: str,
    language: str,
    source: bytes,
    lines: list[str],
) -> CodeChunk:
    symbol = _node_identifier(node, source)
    start_line, end_line = _clamp_range(int(node.start_point[0]), int(node.end_point[0]), len(lines))
    chunk_content = _slice_content(lines, start_line, end_line)
    return _make_chunk(
        repo_id=repo_id,
        path=path,
        language=language,
        kind=kind,
        symbol=symbol,
        start_line=start_line,
        end_line=end_line,
        chunk_content=chunk_content,
    )


def _emit_method_chunks(
    class_node: Any,
    *,
    repo_id: str,
    path: str,
    language: str,
    source: bytes,
    lines: list[str],
) -> list[CodeChunk]:
    body = _class_body(class_node)
    if body is None:
        return []
    out: list[CodeChunk] = []
    for child in body.children:
        if child.type == "function_definition":
            out.append(
                _emit_def_chunk(
                    child,
                    "method",
                    repo_id=repo_id,
                    path=path,
                    language=language,
                    source=source,
                    lines=lines,
                )
            )
    return out


def _chunk_python(
    *,
    path: str,
    repo_id: str,
    language: str,
    content: str,
    lines: list[str],
) -> list[CodeChunk]:
    pack: Any = importlib.import_module("tree_sitter_language_pack")
    parser: Any = pack.get_parser("python")
    source: bytes = content.encode("utf-8")
    tree: Any = parser.parse(source)
    root: Any = tree.root_node

    chunks: list[CodeChunk] = []
    for child in root.children:
        node_type: str = child.type
        if node_type == "function_definition":
            chunks.append(
                _emit_def_chunk(
                    child,
                    "function",
                    repo_id=repo_id,
                    path=path,
                    language=language,
                    source=source,
                    lines=lines,
                )
            )
        elif node_type == "class_definition":
            chunks.append(
                _emit_def_chunk(
                    child,
                    "class",
                    repo_id=repo_id,
                    path=path,
                    language=language,
                    source=source,
                    lines=lines,
                )
            )
            chunks.extend(
                _emit_method_chunks(
                    child,
                    repo_id=repo_id,
                    path=path,
                    language=language,
                    source=source,
                    lines=lines,
                )
            )
        elif node_type == "decorated_definition":
            inner = _decorated_inner(child)
            if inner is None:
                continue
            inner_kind: ChunkKind = "function" if inner.type == "function_definition" else "class"
            symbol = _node_identifier(inner, source)
            start_line, end_line = _clamp_range(int(child.start_point[0]), int(child.end_point[0]), len(lines))
            chunk_content = _slice_content(lines, start_line, end_line)
            chunks.append(
                _make_chunk(
                    repo_id=repo_id,
                    path=path,
                    language=language,
                    kind=inner_kind,
                    symbol=symbol,
                    start_line=start_line,
                    end_line=end_line,
                    chunk_content=chunk_content,
                )
            )
            if inner.type == "class_definition":
                chunks.extend(
                    _emit_method_chunks(
                        inner,
                        repo_id=repo_id,
                        path=path,
                        language=language,
                        source=source,
                        lines=lines,
                    )
                )
    return chunks


def _whole_file_chunk(
    *,
    repo_id: str,
    path: str,
    language: str,
    content: str,
    total: int,
) -> CodeChunk:
    body = content if content else "\n"
    return _make_chunk(
        repo_id=repo_id,
        path=path,
        language=language,
        kind="file",
        symbol=None,
        start_line=1,
        end_line=total or 1,
        chunk_content=body,
    )


def _chunk_fixed_window(
    *,
    path: str,
    repo_id: str,
    language: str,
    lines: list[str],
    window: int,
    overlap: int,
) -> list[CodeChunk]:
    total = len(lines)
    if total == 0:
        return []
    step = max(1, window - overlap)
    chunks: list[CodeChunk] = []
    start_1 = 1
    while start_1 <= total:
        end_1 = min(total, start_1 + window - 1)
        chunk_content = _slice_content(lines, start_1, end_1)
        chunks.append(
            _make_chunk(
                repo_id=repo_id,
                path=path,
                language=language,
                kind="block",
                symbol=None,
                start_line=start_1,
                end_line=end_1,
                chunk_content=chunk_content,
            )
        )
        if end_1 >= total:
            break
        start_1 += step
    return chunks


def chunk_file(
    *,
    path: str,
    repo_id: str,
    language: str,
    content: str,
    max_chunk_lines: int = _DEFAULT_MAX_CHUNK_LINES,
) -> list[CodeChunk]:
    """Chunk a file into ``CodeChunk``s; AST-aware for Python, fixed-window otherwise.

    ``max_chunk_lines`` is reserved for future body-splitting of oversized defs.
    """
    _ = max_chunk_lines
    if not content or not content.strip():
        return []
    lines = content.splitlines()
    total = len(lines)

    if language == "python":
        try:
            ast_chunks = _chunk_python(
                path=path,
                repo_id=repo_id,
                language=language,
                content=content,
                lines=lines,
            )
        except Exception as exc:
            log.warning("parser.ast_failed", language=language, path=path, error=str(exc))
            ast_chunks = []
        if ast_chunks:
            return ast_chunks
        return [_whole_file_chunk(repo_id=repo_id, path=path, language=language, content=content, total=total)]

    log.debug("parser.no_ast_extractor", language=language)
    return _chunk_fixed_window(
        path=path,
        repo_id=repo_id,
        language=language,
        lines=lines,
        window=_FALLBACK_WINDOW_LINES,
        overlap=_FALLBACK_OVERLAP_LINES,
    )
