"""Unit tests for OllamaEmbeddingProvider with a mock httpx transport."""

from __future__ import annotations

import asyncio
import json

import httpx
import pytest

from code_atlas.errors import ProviderError
from code_atlas.providers.ollama_embeddings import OllamaEmbeddingProvider, _factory
from code_atlas.providers.registry import register_embedding, registered_embeddings

DIM = 4


def _ok_response(dim: int = DIM) -> httpx.Response:
    return httpx.Response(200, json={"embedding": [0.1] * dim})


def _make_provider(
    handler: httpx.MockTransport,
    *,
    dimension: int = DIM,
    concurrency: int = 4,
) -> tuple[OllamaEmbeddingProvider, httpx.AsyncClient]:
    client = httpx.AsyncClient(base_url="http://test", transport=handler)
    provider = OllamaEmbeddingProvider(
        base_url="http://test",
        model="nomic-embed-text",
        dimension=dimension,
        concurrency=concurrency,
        client=client,
    )
    return provider, client


@pytest.mark.asyncio
async def test_embed_round_trip_two_inputs() -> None:
    seen: list[str] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content.decode("utf-8"))
        seen.append(body["prompt"])
        return _ok_response()

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport)
    try:
        vectors = await provider.embed(["hello", "world"])
    finally:
        await client.aclose()

    assert len(vectors) == 2
    assert all(len(v) == DIM for v in vectors)
    assert sorted(seen) == ["hello", "world"]


@pytest.mark.asyncio
async def test_embed_empty_batch_returns_empty() -> None:
    calls = 0

    async def handler(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return _ok_response()

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport)
    try:
        vectors = await provider.embed([])
    finally:
        await client.aclose()

    assert vectors == []
    assert calls == 0


@pytest.mark.asyncio
async def test_embed_http_error_wraps_provider_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(500, text="boom")

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport)
    try:
        with pytest.raises(ProviderError) as info:
            await provider.embed(["hi"])
    finally:
        await client.aclose()

    assert info.value.context["status_code"] == 500
    assert "boom" in info.value.context["body"]


@pytest.mark.asyncio
async def test_embed_network_error_wraps_provider_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("nope", request=request)

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport)
    try:
        with pytest.raises(ProviderError) as info:
            await provider.embed(["hi"])
    finally:
        await client.aclose()

    assert info.value.context["error_type"] == "ConnectError"


@pytest.mark.asyncio
async def test_embed_malformed_response_raises() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"foo": "bar"})

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport)
    try:
        with pytest.raises(ProviderError) as info:
            await provider.embed(["hi"])
    finally:
        await client.aclose()

    assert "malformed" in info.value.message
    assert info.value.context["keys"] == ["foo"]


@pytest.mark.asyncio
async def test_embed_dimension_mismatch_raises() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"embedding": [0.1, 0.2, 0.3]})

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport, dimension=4)
    try:
        with pytest.raises(ProviderError) as info:
            await provider.embed(["hi"])
    finally:
        await client.aclose()

    assert info.value.context["expected"] == 4
    assert info.value.context["got"] == 3


@pytest.mark.asyncio
async def test_concurrency_cap_respected() -> None:
    in_flight = 0
    max_in_flight = 0
    lock = asyncio.Lock()

    async def handler(request: httpx.Request) -> httpx.Response:
        nonlocal in_flight, max_in_flight
        async with lock:
            in_flight += 1
            if in_flight > max_in_flight:
                max_in_flight = in_flight
        await asyncio.sleep(0.02)
        async with lock:
            in_flight -= 1
        return _ok_response()

    transport = httpx.MockTransport(handler)
    provider, client = _make_provider(transport, concurrency=2)
    try:
        vectors = await provider.embed(["a", "b", "c", "d", "e"])
    finally:
        await client.aclose()

    assert len(vectors) == 5
    assert max_in_flight <= 2


def test_auto_registration() -> None:
    register_embedding("ollama", _factory)
    assert "ollama" in registered_embeddings()


def test_invalid_dimension_or_concurrency_in_ctor_raises() -> None:
    with pytest.raises(ProviderError) as info:
        OllamaEmbeddingProvider(
            base_url="http://test",
            model="m",
            dimension=0,
        )
    assert info.value.context["dimension"] == 0

    with pytest.raises(ProviderError) as info2:
        OllamaEmbeddingProvider(
            base_url="http://test",
            model="m",
            dimension=4,
            concurrency=0,
        )
    assert info2.value.context["concurrency"] == 0


@pytest.mark.asyncio
async def test_aclose_does_not_close_injected_client() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return _ok_response()

    transport = httpx.MockTransport(handler)
    client = httpx.AsyncClient(base_url="http://test", transport=transport)
    provider = OllamaEmbeddingProvider(
        base_url="http://test",
        model="m",
        dimension=DIM,
        client=client,
    )
    await provider.aclose()
    assert client.is_closed is False
    await client.aclose()
