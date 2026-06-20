"""System prompts and context rendering for the Q&A agent."""

from __future__ import annotations

from code_atlas.domain.retrieval import RetrievalResult

__all__ = ["DECLINE_MESSAGE", "SYSTEM_PROMPT", "format_context", "format_user_prompt"]

SYSTEM_PROMPT = (
    "You are a code assistant answering questions about a single indexed repository.\n"
    "\n"
    "Rules:\n"
    "1. Ground every claim in the provided context or in tool results. Cite the supporting\n"
    "   location inline as `path:start-end` (for example `src/app/main.py:10-42`).\n"
    "2. Never invent file paths, symbols, or line numbers. Only reference paths that appear\n"
    "   in the context or are returned by a tool.\n"
    "3. If the context and tools are insufficient to answer, say so plainly and do not guess.\n"
    "\n"
    "You may call tools to read specific file ranges or to look up symbols, callers, and\n"
    "callees before answering. Keep answers concise and grounded."
)

DECLINE_MESSAGE = "I couldn't find supporting code in the index to answer that question."


def format_context(results: list[RetrievalResult]) -> str:
    """Render retrieved chunks into a context block for the LLM."""
    if not results:
        return "No indexed code was retrieved for this question."
    blocks: list[str] = []
    for result in results:
        chunk = result.chunk
        symbol = chunk.symbol or "-"
        header = f"[{chunk.path}:{chunk.start_line}-{chunk.end_line}] symbol={symbol}"
        blocks.append(f"{header}\n{chunk.content}")
    return "\n\n".join(blocks)


def format_user_prompt(question: str, results: list[RetrievalResult]) -> str:
    """Combine the question with the rendered retrieval context into a user turn."""
    return f"{question}\n\nContext:\n{format_context(results)}"
