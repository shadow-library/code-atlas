"""Ollama embedding provider: async httpx client, single-input API fanned out concurrently."""

from __future__ import annotations

import asyncio
import json

import httpx

from code_atlas.config import Settings
from code_atlas.errors import ProviderError
from code_atlas.providers.registry import register_embedding
from code_atlas.utils import get_logger

__all__ = ["OllamaEmbeddingProvider"]

log = get_logger(__name__)

_EMBED_PATH = "/api/embeddings"


class OllamaEmbeddingProvider:
    """Embeddings via Ollama's single-input ``/api/embeddings`` endpoint, fanned out concurrently."""

    def __init__(
        self,
        *,
        base_url: str,
        model: str,
        dimension: int,
        timeout_s: float = 60.0,
        concurrency: int = 4,
        client: httpx.AsyncClient | None = None,
    ) -> None:
        if dimension < 1:
            raise ProviderError(
                "ollama embeddings: dimension must be >= 1",
                context={"dimension": dimension},
            )
        if concurrency < 1:
            raise ProviderError(
                "ollama embeddings: concurrency must be >= 1",
                context={"concurrency": concurrency},
            )

        self.model = model
        self.dimension = dimension
        self._base_url = base_url
        self._timeout_s = timeout_s
        self._concurrency = concurrency

        if client is None:
            self._client = httpx.AsyncClient(base_url=base_url, timeout=timeout_s)
            self._owns_client = True
        else:
            self._client = client
            self._owns_client = False

    async def embed(self, texts: list[str]) -> list[list[float]]:
        if not texts:
            return []

        log.info("ollama_embeddings.batch", count=len(texts), concurrency=self._concurrency)
        semaphore = asyncio.Semaphore(self._concurrency)
        return await asyncio.gather(*[self._embed_one(text, semaphore) for text in texts])

    async def _embed_one(self, text: str, semaphore: asyncio.Semaphore) -> list[float]:
        async with semaphore:
            log.debug("ollama_embeddings.request", model=self.model, text_len=len(text))
            try:
                resp = await self._client.post(
                    _EMBED_PATH,
                    json={"model": self.model, "prompt": text},
                )
                resp.raise_for_status()
            except httpx.HTTPStatusError as exc:
                log.warning(
                    "ollama_embeddings.failed",
                    error=str(exc),
                    status_code=exc.response.status_code,
                )
                raise ProviderError(
                    "ollama embeddings: HTTP error",
                    context={
                        "status_code": exc.response.status_code,
                        "url": str(exc.request.url),
                        "body": exc.response.text[:200],
                    },
                ) from exc
            except httpx.RequestError as exc:
                log.warning(
                    "ollama_embeddings.failed",
                    error=str(exc),
                    error_type=type(exc).__name__,
                )
                raise ProviderError(
                    "ollama embeddings: network error",
                    context={
                        "error_type": type(exc).__name__,
                        "url": str(exc.request.url) if exc.request is not None else None,
                    },
                ) from exc

            try:
                payload = resp.json()
            except (json.JSONDecodeError, ValueError) as exc:
                log.warning("ollama_embeddings.failed", error=str(exc), status_code=resp.status_code)
                raise ProviderError(
                    "ollama embeddings: invalid JSON response",
                    context={"status_code": resp.status_code},
                ) from exc

            if not isinstance(payload, dict):
                raise ProviderError(
                    "ollama embeddings: malformed response",
                    context={"type": type(payload).__name__},
                )

            vec = payload.get("embedding")
            if not isinstance(vec, list):
                raise ProviderError(
                    "ollama embeddings: malformed response",
                    context={"keys": list(payload.keys())},
                )

            if len(vec) != self.dimension:
                raise ProviderError(
                    "ollama embeddings: dimension mismatch",
                    context={
                        "expected": self.dimension,
                        "got": len(vec),
                        "model": self.model,
                    },
                )

            return [float(x) for x in vec]

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()


def _factory(settings: Settings) -> OllamaEmbeddingProvider:
    return OllamaEmbeddingProvider(
        base_url=settings.ollama.base_url,
        model=settings.embeddings.model,
        dimension=settings.embeddings.dimension,
        timeout_s=settings.ollama.timeout_s,
        concurrency=settings.ollama.concurrency,
    )


register_embedding("ollama", _factory)
