"""Unit tests for OllamaLLMProvider with a mock httpx transport."""

from __future__ import annotations

import json
from typing import Any

import httpx
import pytest

from code_atlas.errors import ProviderError
from code_atlas.providers.base import ChatMessage, ToolSpec
from code_atlas.providers.ollama_llm import OllamaLLMProvider, _factory
from code_atlas.providers.registry import register_llm, registered_llms


def _ok_chat_response(content: str = "hi", tool_calls: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    msg: dict[str, Any] = {"role": "assistant", "content": content}
    if tool_calls is not None:
        msg["tool_calls"] = tool_calls
    return {
        "model": "llama3.1",
        "message": msg,
        "done": True,
        "done_reason": "stop",
        "prompt_eval_count": 7,
        "eval_count": 11,
    }


def _ndjson_bytes(lines: list[dict[str, Any]]) -> bytes:
    return ("\n".join(json.dumps(line) for line in lines)).encode("utf-8")


def _make_provider(
    handler: httpx.MockTransport,
    *,
    temperature: float = 0.2,
    max_tokens: int = 2048,
) -> tuple[OllamaLLMProvider, httpx.AsyncClient]:
    client = httpx.AsyncClient(base_url="http://test", transport=handler)
    provider = OllamaLLMProvider(
        base_url="http://test",
        model="llama3.1",
        temperature=temperature,
        max_tokens=max_tokens,
        client=client,
    )
    return provider, client


@pytest.mark.asyncio
async def test_chat_plain_returns_content_and_usage() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=_ok_chat_response(content="Hello"))

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        resp = await provider.chat([ChatMessage(role="user", content="hi")])
    finally:
        await client.aclose()

    assert resp.content == "Hello"
    assert resp.model == "llama3.1"
    assert resp.finish_reason == "stop"
    assert resp.usage.prompt == 7
    assert resp.usage.completion == 11
    assert resp.usage.total == 18
    assert resp.tool_calls == []


@pytest.mark.asyncio
async def test_chat_with_tools_extracts_tool_calls() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json=_ok_chat_response(
                content="",
                tool_calls=[{"function": {"name": "lookup", "arguments": {"q": "x"}}}],
            ),
        )

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        resp = await provider.chat([ChatMessage(role="user", content="lookup x")])
    finally:
        await client.aclose()

    assert len(resp.tool_calls) == 1
    tc = resp.tool_calls[0]
    assert tc.id == "call_0"
    assert tc.name == "lookup"
    assert tc.arguments == {"q": "x"}


@pytest.mark.asyncio
async def test_chat_sends_tools_in_payload() -> None:
    seen: dict[str, Any] = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content.decode("utf-8")))
        return httpx.Response(200, json=_ok_chat_response())

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        await provider.chat(
            [ChatMessage(role="user", content="hi")],
            tools=[ToolSpec(name="lookup", description="search", parameters={"type": "object"})],
        )
        assert "tools" in seen
        assert seen["tools"][0]["type"] == "function"
        assert seen["tools"][0]["function"]["name"] == "lookup"
        assert seen["tools"][0]["function"]["parameters"] == {"type": "object"}

        seen.clear()
        await provider.chat([ChatMessage(role="user", content="hi")])
        assert "tools" not in seen
    finally:
        await client.aclose()


@pytest.mark.asyncio
async def test_chat_options_carry_temperature_and_max_tokens() -> None:
    seen: dict[str, Any] = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content.decode("utf-8")))
        return httpx.Response(200, json=_ok_chat_response())

    provider, client = _make_provider(httpx.MockTransport(handler), temperature=0.3, max_tokens=256)
    try:
        await provider.chat([ChatMessage(role="user", content="hi")])
    finally:
        await client.aclose()

    assert seen["options"] == {"temperature": 0.3, "num_predict": 256}
    assert seen["stream"] is False
    assert seen["model"] == "llama3.1"


@pytest.mark.asyncio
async def test_chat_messages_serialized_correctly() -> None:
    seen: dict[str, Any] = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content.decode("utf-8")))
        return httpx.Response(200, json=_ok_chat_response())

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        await provider.chat(
            [
                ChatMessage(role="system", content="be helpful"),
                ChatMessage(role="user", content="hi"),
                ChatMessage(role="tool", content="result=42", tool_call_id="call_0", name="lookup"),
            ],
        )
    finally:
        await client.aclose()

    msgs = seen["messages"]
    assert msgs[0] == {"role": "system", "content": "be helpful"}
    assert msgs[1] == {"role": "user", "content": "hi"}
    assert msgs[2]["role"] == "tool"
    assert msgs[2]["content"] == "result=42"
    assert msgs[2]["name"] == "lookup"
    assert msgs[2]["tool_call_id"] == "call_0"


@pytest.mark.asyncio
async def test_chat_http_error_wraps_provider_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(500, text="boom")

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        with pytest.raises(ProviderError) as info:
            await provider.chat([ChatMessage(role="user", content="hi")])
    finally:
        await client.aclose()

    assert info.value.context["status_code"] == 500
    assert "boom" in info.value.context["body"]


@pytest.mark.asyncio
async def test_chat_network_error_wraps_provider_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("nope", request=request)

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        with pytest.raises(ProviderError) as info:
            await provider.chat([ChatMessage(role="user", content="hi")])
    finally:
        await client.aclose()

    assert info.value.context["error_type"] == "ConnectError"


@pytest.mark.asyncio
async def test_chat_malformed_response_raises() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"foo": "bar"})

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        with pytest.raises(ProviderError) as info:
            await provider.chat([ChatMessage(role="user", content="hi")])
    finally:
        await client.aclose()

    assert "malformed" in info.value.message
    assert info.value.context["keys"] == ["foo"]


@pytest.mark.asyncio
async def test_chat_stream_yields_chunks_then_done() -> None:
    body = _ndjson_bytes(
        [
            {"model": "llama3.1", "message": {"role": "assistant", "content": "He"}, "done": False},
            {"model": "llama3.1", "message": {"role": "assistant", "content": "llo"}, "done": False},
            {
                "model": "llama3.1",
                "message": {"role": "assistant", "content": ""},
                "done": True,
                "done_reason": "stop",
                "prompt_eval_count": 7,
                "eval_count": 11,
            },
        ],
    )

    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=body)

    provider, client = _make_provider(httpx.MockTransport(handler))
    chunks = []
    try:
        async for chunk in provider.chat_stream([ChatMessage(role="user", content="hi")]):
            chunks.append(chunk)
    finally:
        await client.aclose()

    assert len(chunks) == 3
    assert chunks[0].content_delta == "He"
    assert chunks[0].done is False
    assert chunks[1].content_delta == "llo"
    assert chunks[1].done is False
    assert chunks[2].done is True
    assert chunks[2].finish_reason == "stop"
    assert chunks[2].usage is not None
    assert chunks[2].usage.prompt == 7
    assert chunks[2].usage.completion == 11


@pytest.mark.asyncio
async def test_chat_stream_emits_tool_call_delta() -> None:
    body = _ndjson_bytes(
        [
            {
                "model": "llama3.1",
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{"function": {"name": "lookup", "arguments": {"q": "x"}}}],
                },
                "done": True,
                "done_reason": "stop",
                "prompt_eval_count": 4,
                "eval_count": 2,
            },
        ],
    )

    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=body)

    provider, client = _make_provider(httpx.MockTransport(handler))
    chunks = []
    try:
        async for chunk in provider.chat_stream([ChatMessage(role="user", content="hi")]):
            chunks.append(chunk)
    finally:
        await client.aclose()

    assert len(chunks) == 1
    assert chunks[0].done is True
    assert chunks[0].tool_call_delta is not None
    assert chunks[0].tool_call_delta.id == "call_0"
    assert chunks[0].tool_call_delta.name == "lookup"
    assert chunks[0].tool_call_delta.arguments == {"q": "x"}


@pytest.mark.asyncio
async def test_chat_stream_http_error_wraps_provider_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(500, text="boom")

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        with pytest.raises(ProviderError) as info:
            async for _ in provider.chat_stream([ChatMessage(role="user", content="hi")]):
                pass
    finally:
        await client.aclose()

    assert info.value.context["status_code"] == 500


@pytest.mark.asyncio
async def test_chat_empty_messages_raises() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=_ok_chat_response())

    provider, client = _make_provider(httpx.MockTransport(handler))
    try:
        with pytest.raises(ProviderError):
            await provider.chat([])
    finally:
        await client.aclose()


def test_invalid_ctor_args_raise() -> None:
    with pytest.raises(ProviderError) as info:
        OllamaLLMProvider(base_url="http://test", model="m", temperature=-1.0)
    assert info.value.context["temperature"] == -1.0

    with pytest.raises(ProviderError) as info2:
        OllamaLLMProvider(base_url="http://test", model="m", max_tokens=0)
    assert info2.value.context["max_tokens"] == 0

    with pytest.raises(ProviderError):
        OllamaLLMProvider(base_url="http://test", model="")


def test_auto_registration() -> None:
    register_llm("ollama", _factory)
    assert "ollama" in registered_llms()
