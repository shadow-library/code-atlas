"""API routes plus the dependency providers tests override for offline runs.

The dependency providers (:func:`get_agent_factory`, :func:`get_ingest_runner`) are
the *only* request-time touch points for ``app.state`` and real I/O. Overriding both
lets the route handlers run without the lifespan having opened any real store. They
live here (not in ``app.py``) so routes never import ``app``: the app imports the
router, routes read ``request.app.state`` lazily at request time.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Callable
from pathlib import Path
from typing import Annotated
from uuid import uuid4

from fastapi import APIRouter, BackgroundTasks, Depends, Request
from fastapi.responses import StreamingResponse

from code_atlas import __version__
from code_atlas.agent.qa import QAAgent
from code_atlas.agent.tools import Toolbox
from code_atlas.api.models import AskRequest, HealthResponse, IngestRequest, IngestResponse
from code_atlas.config import Settings
from code_atlas.domain.answer import Answer
from code_atlas.indexing.indexer import Indexer
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.indexing.vector_store import LanceVectorStore
from code_atlas.providers import make_embedding

__all__ = ["AgentFactory", "IngestRunner", "get_agent_factory", "get_ingest_runner", "router"]

router = APIRouter()

AgentFactory = Callable[[str], QAAgent]
IngestRunner = Callable[[str, str], None]


def get_agent_factory(request: Request) -> AgentFactory:
    """Build per-request :class:`QAAgent` instances from the long-lived ``app.state``."""
    state = request.app.state

    def _make(repo_id: str) -> QAAgent:
        toolbox = Toolbox(metadata_store=state.metadata, symbol_graph=state.graph, repo_id=repo_id)
        return QAAgent(retriever=state.retriever, llm=state.llm, toolbox=toolbox)

    return _make


def get_ingest_runner(request: Request) -> IngestRunner:
    """Return a self-contained index job, mirroring ``cli.py``'s ``ingest``.

    It runs in a threadpool background-task thread with no event loop, so it owns its
    own embedder, four stores, and one persistent loop for the sync embed shim. The
    in-memory ``app.state.graph`` goes stale after this writes a fresh graph to disk
    (real-time index updates are out of scope); a restart reloads it.
    """
    settings: Settings = request.app.state.settings

    def _run(repo_path: str, repo_id: str) -> None:
        root_dir = Path(settings.storage.root_dir)
        sqlite_path = Path(settings.storage.sqlite_path)
        root_dir.mkdir(parents=True, exist_ok=True)
        sqlite_path.resolve().parent.mkdir(parents=True, exist_ok=True)
        lexical_path = str((root_dir / "lexical.sqlite").resolve())
        graph_path = (root_dir / "symbol_graph.json.gz").resolve()

        embedder = make_embedding(settings)
        loop = asyncio.new_event_loop()
        metadata = MetadataStore(f"sqlite:///{sqlite_path.resolve()}")
        lexical = LexicalStore(lexical_path)
        vector = LanceVectorStore(settings.storage.lance_uri, dimension=settings.embeddings.dimension)
        graph = SymbolGraph()
        try:

            def _embed(texts: list[str]) -> list[list[float]]:
                return loop.run_until_complete(embedder.embed(texts))

            indexer = Indexer(
                metadata_store=metadata,
                lexical_store=lexical,
                vector_store=vector,
                symbol_graph=graph,
                embed=_embed,
                batch_size=settings.embeddings.batch_size,
            )
            indexer.index_repo(
                Path(repo_path),
                repo_id,
                extra_ignores=settings.ingestion.ignore_patterns,
                max_chunk_lines=settings.ingestion.max_chunk_lines,
            )
            graph.save(graph_path)
        finally:
            aclose = getattr(embedder, "aclose", None)
            if aclose is not None:
                loop.run_until_complete(aclose())
            loop.close()
            metadata.close()
            lexical.close()
            vector.close()

    return _run


async def _event_stream(agent: QAAgent, question: str) -> AsyncIterator[str]:
    """Replay a computed :class:`Answer` as SSE.

    ``QAAgent`` is non-streaming, so the full answer is computed first and replayed as
    whitespace-delimited token events followed by a terminal ``done`` event carrying the
    serialized answer. True incremental streaming is deferred: it needs a streaming path
    in ``QAAgent`` that streams only the final post-tool turn.
    """
    answer = await agent.ask(question)
    tokens = answer.text.split() or [answer.text]
    for token in tokens:
        yield f"data: {token}\n\n"
    yield f"event: done\ndata: {answer.model_dump_json()}\n\n"


@router.get("/health", response_model=HealthResponse)
async def health() -> HealthResponse:
    """Liveness probe; reads no state."""
    return HealthResponse(status="ok", version=__version__)


@router.post("/ask", response_model=Answer)
async def ask(body: AskRequest, make_agent: Annotated[AgentFactory, Depends(get_agent_factory)]) -> Answer:
    """Answer a question against an indexed repository."""
    agent = make_agent(body.repo_id)
    return await agent.ask(body.question)


@router.post("/ingest", status_code=202, response_model=IngestResponse)
async def ingest(
    body: IngestRequest,
    background_tasks: BackgroundTasks,
    runner: Annotated[IngestRunner, Depends(get_ingest_runner)],
) -> IngestResponse:
    """Schedule a background index job and return its job id immediately."""
    job_id = uuid4().hex
    background_tasks.add_task(runner, body.repo_path, body.repo_id)
    return IngestResponse(job_id=job_id, status="accepted")


@router.get("/ask/stream")
async def ask_stream(
    repo_id: str,
    question: str,
    make_agent: Annotated[AgentFactory, Depends(get_agent_factory)],
) -> StreamingResponse:
    """Stream an answer as server-sent events (replayed from the computed answer)."""
    agent = make_agent(repo_id)
    return StreamingResponse(_event_stream(agent, question), media_type="text/event-stream")
