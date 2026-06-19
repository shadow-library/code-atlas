"""Unit tests for the providers registry and chat-record types."""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator, Sequence

import pytest

from code_atlas.config import Settings
from code_atlas.config.settings import ChatSettings, EmbeddingsSettings
from code_atlas.domain.answer import TokenUsage
from code_atlas.errors import ProviderError
from code_atlas.providers import (
    ChatChunk,
    ChatMessage,
    ChatResponse,
    ToolCall,
    ToolSpec,
    clear_registry,
    make_embedding,
    make_llm,
    register_embedding,
    register_llm,
    registered_embeddings,
    registered_llms,
)


@pytest.fixture(autouse=True)
def _reset_registry() -> Iterator[None]:
    clear_registry()
    yield
    clear_registry()


class _FakeEmbedder:
    """Minimal EmbeddingProvider Protocol implementation."""

    def __init__(self, model: str = "fake-embed", dimension: int = 4) -> None:
        self.model = model
        self.dimension = dimension

    async def embed(self, texts: list[str]) -> list[list[float]]:
        return [[float(len(t))] * self.dimension for t in texts]


class _FakeLLM:
    """Minimal LLMProvider Protocol implementation."""

    def __init__(self, model: str = "fake-chat") -> None:
        self.model = model

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse:
        _ = tools
        last = messages[-1].content if messages else ""
        return ChatResponse(
            content=f"echo:{last}",
            usage=TokenUsage(prompt=3, completion=5),
            model=self.model,
            finish_reason="stop",
        )

    async def chat_stream(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> AsyncIterator[ChatChunk]:
        _ = tools
        last = messages[-1].content if messages else ""
        yield ChatChunk(content_delta=f"echo:{last[:2]}")
        yield ChatChunk(content_delta=last[2:])
        yield ChatChunk(done=True, usage=TokenUsage(prompt=3, completion=5), finish_reason="stop")


def _settings(*, embed: str = "fake", chat: str = "fake") -> Settings:
    return Settings(
        embeddings=EmbeddingsSettings(provider=embed),
        chat=ChatSettings(provider=chat),
    )


@pytest.mark.asyncio
async def test_register_and_make_embedding_round_trip() -> None:
    register_embedding("fake", lambda _s: _FakeEmbedder(dimension=3))
    provider = make_embedding(_settings())
    assert provider.model == "fake-embed"
    assert provider.dimension == 3
    vectors = await provider.embed(["hi", "yo!"])
    assert vectors == [[2.0, 2.0, 2.0], [3.0, 3.0, 3.0]]


@pytest.mark.asyncio
async def test_register_and_make_llm_round_trip() -> None:
    register_llm("fake", lambda _s: _FakeLLM())
    provider = make_llm(_settings())
    response = await provider.chat([ChatMessage(role="user", content="ping")])
    assert isinstance(response, ChatResponse)
    assert response.content == "echo:ping"
    assert response.usage.total == 8
    assert response.finish_reason == "stop"
    assert response.model == "fake-chat"


def test_unknown_embedding_provider_raises_with_available() -> None:
    register_embedding("alpha", lambda _s: _FakeEmbedder())
    register_embedding("beta", lambda _s: _FakeEmbedder())
    with pytest.raises(ProviderError) as info:
        make_embedding(_settings(embed="missing"))
    assert info.value.context["name"] == "missing"
    assert info.value.context["available"] == ["alpha", "beta"]


def test_unknown_llm_provider_raises_with_available() -> None:
    register_llm("alpha", lambda _s: _FakeLLM())
    with pytest.raises(ProviderError) as info:
        make_llm(_settings(chat="missing"))
    assert info.value.context["name"] == "missing"
    assert info.value.context["available"] == ["alpha"]


@pytest.mark.parametrize("bad", ["", "   ", "\t"])
def test_empty_name_on_register_raises(bad: str) -> None:
    with pytest.raises(ProviderError):
        register_embedding(bad, lambda _s: _FakeEmbedder())
    with pytest.raises(ProviderError):
        register_llm(bad, lambda _s: _FakeLLM())


@pytest.mark.asyncio
async def test_reregister_overwrites_without_error() -> None:
    register_embedding("fake", lambda _s: _FakeEmbedder(model="first", dimension=2))
    register_embedding("fake", lambda _s: _FakeEmbedder(model="second", dimension=2))
    provider = make_embedding(_settings())
    assert provider.model == "second"

    register_llm("fake", lambda _s: _FakeLLM(model="first"))
    register_llm("fake", lambda _s: _FakeLLM(model="second"))
    llm = make_llm(_settings())
    assert llm.model == "second"


def test_factory_exception_wraps_as_provider_error() -> None:
    sentinel = RuntimeError("boom")

    def bad_factory(_s: Settings) -> _FakeEmbedder:
        raise sentinel

    register_embedding("fake", bad_factory)
    with pytest.raises(ProviderError) as info:
        make_embedding(_settings())
    assert info.value.context["name"] == "fake"
    assert info.value.__cause__ is sentinel


@pytest.mark.asyncio
async def test_chat_stream_round_trip() -> None:
    register_llm("fake", lambda _s: _FakeLLM())
    provider = make_llm(_settings())
    chunks: list[ChatChunk] = []
    async for chunk in provider.chat_stream([ChatMessage(role="user", content="hello")]):
        chunks.append(chunk)

    assert len(chunks) == 3
    assert chunks[0].content_delta == "echo:he"
    assert chunks[1].content_delta == "llo"
    final = chunks[-1]
    assert final.done is True
    assert final.usage is not None
    assert final.usage.total == 8
    assert final.finish_reason == "stop"


def test_records_json_round_trip() -> None:
    msg = ChatMessage(role="tool", content="result", tool_call_id="call_1", name="lookup")
    assert ChatMessage.model_validate_json(msg.model_dump_json()) == msg

    call = ToolCall(id="call_1", name="lookup", arguments={"q": "x"})
    assert ToolCall.model_validate_json(call.model_dump_json()) == call

    response = ChatResponse(
        content="hi",
        tool_calls=[call],
        usage=TokenUsage(prompt=1, completion=2),
        model="m",
        finish_reason="stop",
    )
    assert ChatResponse.model_validate_json(response.model_dump_json()) == response

    chunk = ChatChunk(content_delta="", done=True, usage=TokenUsage(prompt=1, completion=2), finish_reason="stop")
    assert ChatChunk.model_validate_json(chunk.model_dump_json()) == chunk

    spec = ToolSpec(name="lookup", description="search", parameters={"type": "object"})
    assert ToolSpec.model_validate_json(spec.model_dump_json()) == spec


def test_registered_listings_are_sorted() -> None:
    assert registered_embeddings() == []
    assert registered_llms() == []

    register_embedding("zeta", lambda _s: _FakeEmbedder())
    register_embedding("alpha", lambda _s: _FakeEmbedder())
    register_embedding("mike", lambda _s: _FakeEmbedder())
    assert registered_embeddings() == ["alpha", "mike", "zeta"]

    register_llm("zeta", lambda _s: _FakeLLM())
    register_llm("alpha", lambda _s: _FakeLLM())
    assert registered_llms() == ["alpha", "zeta"]
