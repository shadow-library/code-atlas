"""Q&A orchestrator: retrieval + LLM tool-use loop + grounded citations into an Answer."""

from __future__ import annotations

import json
import time
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any, Literal

from code_atlas.agent.prompts import DECLINE_MESSAGE, SYSTEM_PROMPT, format_user_prompt
from code_atlas.agent.tools import Toolbox
from code_atlas.domain.answer import Answer, Citation, TokenUsage
from code_atlas.domain.retrieval import RetrievalQuery, RetrievalResult
from code_atlas.errors import AgentError
from code_atlas.providers.base import ChatMessage, ChatResponse, LLMProvider, ToolCall
from code_atlas.retrieval.citation import to_citation
from code_atlas.retrieval.hybrid import HybridRetriever
from code_atlas.utils import get_logger

__all__ = ["QAAgent", "StreamEvent"]

log = get_logger(__name__)


@dataclass(frozen=True)
class StreamEvent:
    """One event from ``QAAgent.ask_stream``: a content ``token`` delta, or the terminal ``done`` answer."""

    type: Literal["token", "done"]
    text: str = ""
    answer: Answer | None = None


class QAAgent:
    """Answer a question by retrieving context and driving a bounded LLM tool-use loop."""

    def __init__(
        self,
        *,
        retriever: HybridRetriever,
        llm: LLMProvider,
        toolbox: Toolbox,
        max_tool_iters: int = 4,
        retrieval_k: int = 8,
    ) -> None:
        if max_tool_iters < 1:
            raise AgentError("qa: max_tool_iters must be >= 1", context={"max_tool_iters": max_tool_iters})
        if retrieval_k < 1:
            raise AgentError("qa: retrieval_k must be >= 1", context={"retrieval_k": retrieval_k})
        self._retriever = retriever
        self._llm = llm
        self._toolbox = toolbox
        self._max_tool_iters = max_tool_iters
        self._retrieval_k = retrieval_k

    async def ask(self, question: str) -> Answer:
        if not question.strip():
            raise AgentError("qa: question is required", context={"question": question})

        start = time.perf_counter()
        log.info("qa.ask", question_len=len(question))

        results = await self._retriever.retrieve(RetrievalQuery(text=question, k=self._retrieval_k))

        messages: list[ChatMessage] = [
            ChatMessage(role="system", content=SYSTEM_PROMPT),
            ChatMessage(role="user", content=format_user_prompt(question, results)),
        ]

        prompt_tokens = 0
        completion_tokens = 0
        trace: list[dict[str, Any]] = [{"step": "retrieve", "results": len(results)}]

        response: ChatResponse | None = None
        for iteration in range(self._max_tool_iters + 1):
            response = await self._llm.chat(messages, tools=self._toolbox.specs)
            prompt_tokens += response.usage.prompt
            completion_tokens += response.usage.completion
            trace.append(
                {
                    "step": "llm",
                    "iter": iteration,
                    "tool_calls": [tc.name for tc in response.tool_calls],
                    "finish_reason": response.finish_reason,
                }
            )
            if not response.tool_calls:
                break
            if iteration == self._max_tool_iters:
                break  # cap reached; stop executing further tools
            messages.append(ChatMessage(role="assistant", content=response.content))
            for tc in response.tool_calls:
                result = self._run_tool(tc, trace)
                messages.append(ChatMessage(role="tool", content=json.dumps(result), tool_call_id=tc.id, name=tc.name))
        assert response is not None  # range(max_tool_iters + 1) runs at least once

        text = response.content.strip() or DECLINE_MESSAGE
        citations = self._build_citations(results)
        latency_ms = int((time.perf_counter() - start) * 1000)
        token_usage = TokenUsage(prompt=prompt_tokens, completion=completion_tokens)

        n_iters = sum(1 for entry in trace if entry["step"] == "llm")
        log.info("qa.answer", n_citations=len(citations), n_iters=n_iters, latency_ms=latency_ms)

        return Answer(
            text=text,
            citations=citations,
            trace=trace,
            latency_ms=latency_ms,
            token_usage=token_usage,
        )

    async def ask_stream(self, question: str) -> AsyncIterator[StreamEvent]:
        if not question.strip():
            raise AgentError("qa: question is required", context={"question": question})

        start = time.perf_counter()
        log.info("qa.ask_stream", question_len=len(question))

        results = await self._retriever.retrieve(RetrievalQuery(text=question, k=self._retrieval_k))

        messages: list[ChatMessage] = [
            ChatMessage(role="system", content=SYSTEM_PROMPT),
            ChatMessage(role="user", content=format_user_prompt(question, results)),
        ]

        prompt_tokens = 0
        completion_tokens = 0
        trace: list[dict[str, Any]] = [{"step": "retrieve", "results": len(results)}]
        final_content = ""

        for iteration in range(self._max_tool_iters + 1):
            content_parts: list[str] = []
            tool_calls: list[ToolCall] = []
            usage: TokenUsage | None = None
            finish_reason: str | None = None
            # Tokens stream every turn; Ollama tool-call turns carry empty content, so only the final turn surfaces.
            async for chunk in self._llm.chat_stream(messages, tools=self._toolbox.specs):
                if chunk.content_delta:
                    content_parts.append(chunk.content_delta)
                    yield StreamEvent(type="token", text=chunk.content_delta)
                if chunk.tool_call_delta is not None:
                    tool_calls.append(chunk.tool_call_delta)
                if chunk.usage is not None:
                    usage = chunk.usage
                if chunk.finish_reason is not None:
                    finish_reason = chunk.finish_reason
            if usage is not None:
                prompt_tokens += usage.prompt
                completion_tokens += usage.completion
            content = "".join(content_parts)
            trace.append(
                {
                    "step": "llm",
                    "iter": iteration,
                    "tool_calls": [tc.name for tc in tool_calls],
                    "finish_reason": finish_reason,
                }
            )
            if not tool_calls or iteration == self._max_tool_iters:
                final_content = content
                break
            messages.append(ChatMessage(role="assistant", content=content))
            for tc in tool_calls:
                result = self._run_tool(tc, trace)
                messages.append(ChatMessage(role="tool", content=json.dumps(result), tool_call_id=tc.id, name=tc.name))

        # The terminal `done` event's Answer is authoritative; Answer.text is the final turn's content.
        text = final_content.strip() or DECLINE_MESSAGE
        citations = self._build_citations(results)
        latency_ms = int((time.perf_counter() - start) * 1000)
        token_usage = TokenUsage(prompt=prompt_tokens, completion=completion_tokens)

        n_iters = sum(1 for entry in trace if entry["step"] == "llm")
        log.info("qa.answer_stream", n_citations=len(citations), n_iters=n_iters, latency_ms=latency_ms)

        yield StreamEvent(
            type="done",
            answer=Answer(text=text, citations=citations, trace=trace, latency_ms=latency_ms, token_usage=token_usage),
        )

    def _run_tool(self, tc: ToolCall, trace: list[dict[str, Any]]) -> dict[str, Any]:
        trace.append({"step": "tool", "name": tc.name, "arguments": tc.arguments})
        try:
            return self._toolbox.call(tc.name, tc.arguments)
        except AgentError as exc:
            # Feed tool errors back to the LLM instead of raising, keeping the loop robust.
            return {"error": str(exc)}

    def _build_citations(self, results: list[RetrievalResult]) -> list[Citation]:
        seen: set[tuple[str, int, int]] = set()
        citations: list[Citation] = []
        for result in results:
            citation = to_citation(result.chunk)
            key = (citation.path, citation.start_line, citation.end_line)
            if key in seen:
                continue
            seen.add(key)
            citations.append(citation)
        return citations
