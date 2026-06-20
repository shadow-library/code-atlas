"""Integration tests for QAAgent: real retriever + metadata store + symbol graph + toolbox."""

from __future__ import annotations

from collections.abc import AsyncIterator, Sequence
from pathlib import Path
from typing import Any

import pytest

from code_atlas.agent.prompts import DECLINE_MESSAGE
from code_atlas.agent.qa import QAAgent
from code_atlas.agent.tools import Toolbox
from code_atlas.domain.answer import TokenUsage
from code_atlas.domain.chunk import CodeChunk, Symbol
from code_atlas.errors import AgentError
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.providers.base import ChatChunk, ChatMessage, ChatResponse, ToolCall, ToolSpec
from code_atlas.retrieval.hybrid import HybridRetriever

_HYBRID_PATH = "src/code_atlas/retrieval/hybrid.py"


class FakeVectorStore:
    dimension = 4

    def __init__(self, results: list[tuple[str, float]]) -> None:
        self._results = results

    def search(
        self,
        vector: list[float],
        k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> list[tuple[str, float]]:
        return list(self._results[:k])


class FakeLexicalStore:
    def __init__(self, results: list[tuple[str, float]]) -> None:
        self._results = results

    def search(
        self,
        query: str,
        k: int = 10,
        repo_id: str | None = None,
    ) -> list[tuple[str, float]]:
        return list(self._results[:k])


class FakeEmbedder:
    model = "fake"
    dimension = 4

    async def embed(self, texts: list[str]) -> list[list[float]]:
        return [[float(len(t))] * self.dimension for t in texts]


class FakeLLM:
    model = "fake"

    def __init__(self, responses: list[ChatResponse]) -> None:
        self._responses = responses
        self.calls: list[tuple[list[ChatMessage], Sequence[ToolSpec] | None]] = []

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse:
        self.calls.append((list(messages), tools))
        return self._responses[len(self.calls) - 1]

    async def chat_stream(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> AsyncIterator[ChatChunk]:
        if False:  # pragma: no cover - unused minimal stub
            yield ChatChunk()


def _seed_chunk(meta: MetadataStore) -> None:
    meta.upsert(
        CodeChunk(
            chunk_id="c1",
            repo_id="r",
            path=_HYBRID_PATH,
            language="python",
            kind="class",
            symbol="HybridRetriever",
            start_line=24,
            end_line=102,
            content="class HybridRetriever:\n    ...\n",
            content_hash="h" * 16,
        )
    )


@pytest.fixture
def meta_store(tmp_path: Path) -> MetadataStore:
    # File-backed SQLite (not :memory:) so asyncio.to_thread workers share the same DB.
    store = MetadataStore(f"sqlite:///{tmp_path / 'meta.sqlite'}")
    _seed_chunk(store)
    return store


@pytest.fixture
def symbol_graph() -> SymbolGraph:
    graph = SymbolGraph()
    graph.add_symbol(Symbol(name="HybridRetriever", kind="class", path=_HYBRID_PATH, line=24))
    return graph


def _retriever(meta: MetadataStore) -> HybridRetriever:
    return HybridRetriever(
        vector_store=FakeVectorStore([("c1", 0.9)]),
        lexical_store=FakeLexicalStore([("c1", 0.8)]),
        embedder=FakeEmbedder(),
        metadata_store=meta,
    )


def _tool_then_answer() -> list[ChatResponse]:
    return [
        ChatResponse(
            content="",
            tool_calls=[ToolCall(id="call_0", name="find_symbol", arguments={"name": "HybridRetriever"})],
            usage=TokenUsage(prompt=10, completion=5),
        ),
        ChatResponse(
            content=f"The hybrid retriever is defined in {_HYBRID_PATH}:24-102.",
            usage=TokenUsage(prompt=12, completion=8),
        ),
    ]


async def test_ask_returns_grounded_answer(meta_store: MetadataStore, symbol_graph: SymbolGraph) -> None:
    fake_llm = FakeLLM(_tool_then_answer())
    agent = QAAgent(
        retriever=_retriever(meta_store),
        llm=fake_llm,
        toolbox=Toolbox(metadata_store=meta_store, symbol_graph=symbol_graph, repo_id="r"),
        max_tool_iters=4,
    )
    answer = await agent.ask("Where is the hybrid retriever defined?")
    assert answer.text == f"The hybrid retriever is defined in {_HYBRID_PATH}:24-102."
    assert len(answer.citations) >= 1
    assert any(c.path == _HYBRID_PATH for c in answer.citations)
    assert answer.token_usage.total == 35
    assert len(fake_llm.calls) == 2


async def test_tool_call_executed(meta_store: MetadataStore, symbol_graph: SymbolGraph) -> None:
    fake_llm = FakeLLM(_tool_then_answer())
    agent = QAAgent(
        retriever=_retriever(meta_store),
        llm=fake_llm,
        toolbox=Toolbox(metadata_store=meta_store, symbol_graph=symbol_graph, repo_id="r"),
        max_tool_iters=4,
    )
    answer = await agent.ask("Where is the hybrid retriever defined?")
    second_call_messages, _ = fake_llm.calls[1]
    assert any(m.role == "tool" and m.name == "find_symbol" for m in second_call_messages)
    assert any(e.get("step") == "tool" and e.get("name") == "find_symbol" for e in answer.trace)


async def test_decline_when_no_content(meta_store: MetadataStore, symbol_graph: SymbolGraph) -> None:
    fake_llm = FakeLLM([ChatResponse(content="")])
    agent = QAAgent(
        retriever=_retriever(meta_store),
        llm=fake_llm,
        toolbox=Toolbox(metadata_store=meta_store, symbol_graph=symbol_graph, repo_id="r"),
        max_tool_iters=4,
    )
    answer = await agent.ask("Where is the hybrid retriever defined?")
    assert answer.text == DECLINE_MESSAGE


async def test_empty_question_raises(meta_store: MetadataStore, symbol_graph: SymbolGraph) -> None:
    agent = QAAgent(
        retriever=_retriever(meta_store),
        llm=FakeLLM(_tool_then_answer()),
        toolbox=Toolbox(metadata_store=meta_store, symbol_graph=symbol_graph, repo_id="r"),
        max_tool_iters=4,
    )
    with pytest.raises(AgentError):
        await agent.ask("   ")
