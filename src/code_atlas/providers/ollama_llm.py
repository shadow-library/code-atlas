"""Ollama chat provider: async ``/api/chat`` with NDJSON streaming and tool calls."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator, Iterator, Sequence
from typing import Any

import httpx

from code_atlas.config import Settings
from code_atlas.domain.answer import TokenUsage
from code_atlas.errors import ProviderError
from code_atlas.providers.base import ChatChunk, ChatMessage, ChatResponse, ToolCall, ToolSpec
from code_atlas.providers.registry import register_llm
from code_atlas.utils import get_logger

__all__ = ["OllamaLLMProvider"]

log = get_logger(__name__)

_CHAT_PATH = "/api/chat"


class OllamaLLMProvider:
    """Chat via Ollama's ``/api/chat``, with NDJSON streaming and tool calls."""

    def __init__(
        self,
        *,
        base_url: str,
        model: str,
        temperature: float = 0.2,
        max_tokens: int = 2048,
        timeout_s: float = 60.0,
        client: httpx.AsyncClient | None = None,
    ) -> None:
        if not model or not model.strip():
            raise ProviderError("ollama llm: model must be non-empty", context={"model": model})
        if temperature < 0.0 or temperature > 2.0:
            raise ProviderError(
                "ollama llm: temperature out of range",
                context={"temperature": temperature},
            )
        if max_tokens < 1:
            raise ProviderError(
                "ollama llm: max_tokens must be >= 1",
                context={"max_tokens": max_tokens},
            )

        self.model = model
        self._base_url = base_url
        self._temperature = temperature
        self._max_tokens = max_tokens
        self._timeout_s = timeout_s

        if client is None:
            self._client = httpx.AsyncClient(base_url=base_url, timeout=timeout_s)
            self._owns_client = True
        else:
            self._client = client
            self._owns_client = False

    async def chat(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> ChatResponse:
        if not messages:
            raise ProviderError("ollama llm: messages must be non-empty")

        body = self._build_payload(messages, tools, stream=False)
        log.info(
            "ollama_llm.chat",
            model=self.model,
            message_count=len(messages),
            has_tools=bool(tools),
        )

        try:
            resp = await self._client.post(_CHAT_PATH, json=body)
            resp.raise_for_status()
        except httpx.HTTPStatusError as exc:
            log.warning(
                "ollama_llm.failed",
                error=str(exc),
                status_code=exc.response.status_code,
            )
            raise ProviderError(
                "ollama llm: HTTP error",
                context={
                    "status_code": exc.response.status_code,
                    "url": str(exc.request.url),
                    "body": exc.response.text[:200],
                },
            ) from exc
        except httpx.RequestError as exc:
            log.warning(
                "ollama_llm.failed",
                error=str(exc),
                error_type=type(exc).__name__,
            )
            raise ProviderError(
                "ollama llm: network error",
                context={
                    "error_type": type(exc).__name__,
                    "url": str(exc.request.url) if exc.request is not None else None,
                },
            ) from exc

        try:
            payload = resp.json()
        except (json.JSONDecodeError, ValueError) as exc:
            log.warning("ollama_llm.failed", error=str(exc), status_code=resp.status_code)
            raise ProviderError(
                "ollama llm: invalid JSON response",
                context={"status_code": resp.status_code},
            ) from exc

        if not isinstance(payload, dict) or "message" not in payload:
            keys = list(payload.keys()) if isinstance(payload, dict) else []
            raise ProviderError("ollama llm: malformed response", context={"keys": keys})

        message = payload["message"] or {}
        raw_tool_calls = message.get("tool_calls") or []
        tool_calls = [_to_tool_call(idx, tc) for idx, tc in enumerate(raw_tool_calls)]

        return ChatResponse(
            content=message.get("content", "") or "",
            tool_calls=tool_calls,
            usage=TokenUsage(
                prompt=int(payload.get("prompt_eval_count", 0) or 0),
                completion=int(payload.get("eval_count", 0) or 0),
            ),
            model=payload.get("model", self.model),
            finish_reason=payload.get("done_reason"),
        )

    async def chat_stream(
        self,
        messages: Sequence[ChatMessage],
        *,
        tools: Sequence[ToolSpec] | None = None,
    ) -> AsyncIterator[ChatChunk]:
        if not messages:
            raise ProviderError("ollama llm: messages must be non-empty")

        body = self._build_payload(messages, tools, stream=True)
        log.info(
            "ollama_llm.stream",
            model=self.model,
            message_count=len(messages),
            has_tools=bool(tools),
        )

        try:
            async with self._client.stream("POST", _CHAT_PATH, json=body) as resp:
                resp.raise_for_status()
                async for line in resp.aiter_lines():
                    if not line.strip():
                        continue
                    try:
                        chunk_data = json.loads(line)
                    except (json.JSONDecodeError, ValueError) as exc:
                        log.warning("ollama_llm.failed", error=str(exc), line=line[:200])
                        raise ProviderError(
                            "ollama llm: invalid JSON response",
                            context={"line": line[:200]},
                        ) from exc

                    for chunk in _emit_line(chunk_data):
                        yield chunk
        except httpx.HTTPStatusError as exc:
            log.warning(
                "ollama_llm.failed",
                error=str(exc),
                status_code=exc.response.status_code,
            )
            raise ProviderError(
                "ollama llm: HTTP error",
                context={
                    "status_code": exc.response.status_code,
                    "url": str(exc.request.url),
                    "body": exc.response.text[:200],
                },
            ) from exc
        except httpx.RequestError as exc:
            log.warning(
                "ollama_llm.failed",
                error=str(exc),
                error_type=type(exc).__name__,
            )
            raise ProviderError(
                "ollama llm: network error",
                context={
                    "error_type": type(exc).__name__,
                    "url": str(exc.request.url) if exc.request is not None else None,
                },
            ) from exc

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()

    def _build_payload(
        self,
        messages: Sequence[ChatMessage],
        tools: Sequence[ToolSpec] | None,
        *,
        stream: bool,
    ) -> dict[str, Any]:
        wire_messages: list[dict[str, Any]] = []
        for msg in messages:
            entry: dict[str, Any] = {"role": msg.role, "content": msg.content}
            if msg.name is not None:
                entry["name"] = msg.name
            if msg.tool_call_id is not None:
                entry["tool_call_id"] = msg.tool_call_id
            wire_messages.append(entry)

        payload: dict[str, Any] = {
            "model": self.model,
            "messages": wire_messages,
            "stream": stream,
            "options": {"temperature": self._temperature, "num_predict": self._max_tokens},
        }

        if tools:
            payload["tools"] = [
                {
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": spec.parameters,
                    },
                }
                for spec in tools
            ]

        return payload


def _to_tool_call(idx: int, raw: dict[str, Any]) -> ToolCall:
    fn = raw.get("function") or {}
    return ToolCall(
        id=f"call_{idx}",
        name=fn.get("name", ""),
        arguments=dict(fn.get("arguments") or {}),
    )


def _emit_line(chunk_data: dict[str, Any]) -> Iterator[ChatChunk]:
    is_final = bool(chunk_data.get("done"))
    message = chunk_data.get("message") or {}
    content_delta = message.get("content", "") or ""
    raw_tool_calls = message.get("tool_calls") or []
    tool_calls = [_to_tool_call(idx, tc) for idx, tc in enumerate(raw_tool_calls)]

    if content_delta and not is_final:
        yield ChatChunk(content_delta=content_delta, done=False)
        content_delta = ""

    if is_final:
        final_tool = tool_calls[-1] if tool_calls else None
        leading = tool_calls[:-1] if tool_calls else []
        for tc in leading:
            yield ChatChunk(content_delta="", tool_call_delta=tc, done=False)
        yield ChatChunk(
            content_delta=content_delta,
            tool_call_delta=final_tool,
            done=True,
            usage=TokenUsage(
                prompt=int(chunk_data.get("prompt_eval_count", 0) or 0),
                completion=int(chunk_data.get("eval_count", 0) or 0),
            ),
            finish_reason=chunk_data.get("done_reason"),
        )
        return

    for tc in tool_calls:
        yield ChatChunk(content_delta="", tool_call_delta=tc, done=False)


def _factory(settings: Settings) -> OllamaLLMProvider:
    return OllamaLLMProvider(
        base_url=settings.ollama.base_url,
        model=settings.chat.model,
        temperature=settings.chat.temperature,
        max_tokens=settings.chat.max_tokens,
        timeout_s=settings.ollama.timeout_s,
    )


register_llm("ollama", _factory)
