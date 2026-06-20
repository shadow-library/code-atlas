"""FastAPI application: lifespan-managed composition root for the long-lived service.

Unlike the CLI (one-shot, sync-shimmed), the API is long-lived and async-native, so it
owns its own composition in :func:`lifespan` rather than sharing a builder with the CLI.
Extracting a shared composition root is deferred until the lifecycles converge.
"""

from __future__ import annotations

from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI

from code_atlas import __version__
from code_atlas.api.routes import router
from code_atlas.config import Settings
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.indexing.vector_store import LanceVectorStore
from code_atlas.providers import make_embedding, make_llm
from code_atlas.retrieval.hybrid import HybridRetriever
from code_atlas.utils import configure_logging, get_logger

__all__ = ["app", "create_app"]

log = get_logger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    """Open the stores, providers, and retriever once and stash them on ``app.state``."""
    settings = Settings()
    configure_logging(settings.app.log_level, json=settings.app.log_json)

    root_dir = Path(settings.storage.root_dir)
    sqlite_path = Path(settings.storage.sqlite_path)
    metadata = MetadataStore(f"sqlite:///{sqlite_path.resolve()}")
    lexical = LexicalStore(str((root_dir / "lexical.sqlite").resolve()))
    vector = LanceVectorStore(settings.storage.lance_uri, dimension=settings.embeddings.dimension)
    graph_path = (root_dir / "symbol_graph.json.gz").resolve()
    graph = SymbolGraph.load(graph_path) if graph_path.exists() else SymbolGraph()

    embedder = make_embedding(settings)
    llm = make_llm(settings)
    retriever = HybridRetriever(
        vector_store=vector,
        lexical_store=lexical,
        embedder=embedder,
        metadata_store=metadata,
    )

    app.state.settings = settings
    app.state.metadata = metadata
    app.state.lexical = lexical
    app.state.vector = vector
    app.state.graph = graph
    app.state.embedder = embedder
    app.state.llm = llm
    app.state.retriever = retriever

    log.info("api.started", version=__version__)
    try:
        yield
    finally:
        for provider in (embedder, llm):
            aclose = getattr(provider, "aclose", None)
            if aclose is not None:
                await aclose()
        metadata.close()
        lexical.close()
        vector.close()
        log.info("api.stopped")


def create_app() -> FastAPI:
    """Construct the FastAPI app with the lifespan-managed composition and routes."""
    app = FastAPI(title="code-atlas", version=__version__, lifespan=lifespan)
    app.include_router(router)
    return app


app = create_app()
