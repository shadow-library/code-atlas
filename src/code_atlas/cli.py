"""Typer CLI: the composition root that wires providers, stores, and the agent.

``cli.py`` owns assembly only. The async embedder/LLM providers are adapted to
the sync ``Indexer`` at this caller boundary (see ``ingest``'s persistent loop).
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Annotated, Any

import typer
import yaml
from rich.console import Console

from code_atlas.agent.qa import QAAgent
from code_atlas.agent.tools import Toolbox
from code_atlas.config import Settings
from code_atlas.config.settings import (
    AppSettings,
    ChatSettings,
    EmbeddingsSettings,
    EvalSettings,
    IngestionSettings,
    OllamaSettings,
    StorageSettings,
)
from code_atlas.domain.answer import Answer
from code_atlas.errors import CodeAtlasError
from code_atlas.indexing.indexer import Indexer
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.indexing.vector_store import LanceVectorStore
from code_atlas.providers import make_embedding, make_llm
from code_atlas.retrieval.hybrid import HybridRetriever
from code_atlas.utils import configure_logging

app = typer.Typer(
    name="code-atlas",
    help="AI coding assistant for large polyglot repositories.",
    no_args_is_help=True,
    add_completion=False,
)
console = Console()
err_console = Console(stderr=True)

_CONFIG_PATH = Path("config/default.yaml")


class _StorePaths:
    """Resolved on-disk locations derived from ``settings.storage``."""

    def __init__(self, settings: Settings) -> None:
        storage = settings.storage
        self.root_dir = Path(storage.root_dir)
        self.sqlite_path = Path(storage.sqlite_path)
        self.metadata_url = f"sqlite:///{self.sqlite_path.resolve()}"
        self.lexical_path = str((self.root_dir / "lexical.sqlite").resolve())
        self.vector_uri = storage.lance_uri
        self.graph_path = (self.root_dir / "symbol_graph.json.gz").resolve()


def _default_config() -> dict[str, Any]:
    """Default config built from the plain nested models (no env/.env reads)."""
    return {
        "app": AppSettings().model_dump(mode="json"),
        "ingestion": IngestionSettings().model_dump(mode="json"),
        "storage": StorageSettings().model_dump(mode="json"),
        "embeddings": EmbeddingsSettings().model_dump(mode="json"),
        "chat": ChatSettings().model_dump(mode="json"),
        "ollama": OllamaSettings().model_dump(mode="json"),
        "eval": EvalSettings().model_dump(mode="json"),
    }


@app.command()
def init(
    force: Annotated[bool, typer.Option("--force", "-f", help="Overwrite an existing config file.")] = False,
) -> None:
    """Write a default config file (config/default.yaml) to the current directory."""
    if _CONFIG_PATH.exists() and not force:
        err_console.print(f"[yellow]{_CONFIG_PATH} already exists; pass --force to overwrite.[/yellow]")
        raise typer.Exit(code=1)

    _CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
    _CONFIG_PATH.write_text(yaml.safe_dump(_default_config(), sort_keys=False), encoding="utf-8")
    console.print(f"[green]Wrote default config to {_CONFIG_PATH}[/green]")


@app.command()
def ingest(
    repo: Annotated[Path, typer.Option("--repo", help="Path to the repository root to index.")],
    repo_id: Annotated[str, typer.Option("--id", help="Stable identifier for this repository.")],
) -> None:
    """Index a repository into the vector, lexical, metadata, and graph stores."""
    settings = Settings()
    configure_logging(settings.app.log_level, json=settings.app.log_json)
    paths = _StorePaths(settings)

    paths.root_dir.mkdir(parents=True, exist_ok=True)
    paths.sqlite_path.resolve().parent.mkdir(parents=True, exist_ok=True)
    Path(paths.lexical_path).parent.mkdir(parents=True, exist_ok=True)

    embedder = make_embedding(settings)
    loop = asyncio.new_event_loop()
    metadata = MetadataStore(paths.metadata_url)
    lexical = LexicalStore(paths.lexical_path)
    vector = LanceVectorStore(paths.vector_uri, dimension=settings.embeddings.dimension)
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
        result = indexer.index_repo(
            repo,
            repo_id,
            extra_ignores=settings.ingestion.ignore_patterns,
            max_chunk_lines=settings.ingestion.max_chunk_lines,
        )
        graph.save(paths.graph_path)

        console.print(f"[bold]Indexed[/bold] {repo_id} ([dim]{repo}[/dim])")
        console.print(f"  chunks seen:    {result.chunks_seen}")
        console.print(f"  chunks indexed: {result.chunks_indexed}")
        console.print(f"  cached skipped: {result.chunks_skipped_cached}")
        console.print(f"  edges added:    {result.edges_added}")
    except CodeAtlasError as exc:
        err_console.print(f"[red]error:[/red] {exc}")
        raise typer.Exit(code=1) from exc
    finally:
        aclose = getattr(embedder, "aclose", None)
        if aclose is not None:
            loop.run_until_complete(aclose())
        loop.close()
        metadata.close()
        lexical.close()
        vector.close()


@app.command()
def ask(
    question: Annotated[str, typer.Argument(help="Natural-language question about the indexed repo.")],
    repo_id: Annotated[str, typer.Option("--repo-id", help="Identifier of the repository to query.")],
) -> None:
    """Answer a question against an indexed repository."""
    settings = Settings()
    configure_logging(settings.app.log_level, json=settings.app.log_json)
    paths = _StorePaths(settings)

    embedder = make_embedding(settings)
    llm = make_llm(settings)
    metadata = MetadataStore(paths.metadata_url)
    lexical = LexicalStore(paths.lexical_path)
    vector = LanceVectorStore(paths.vector_uri, dimension=settings.embeddings.dimension)
    graph = SymbolGraph.load(paths.graph_path) if paths.graph_path.exists() else SymbolGraph()

    retriever = HybridRetriever(
        vector_store=vector,
        lexical_store=lexical,
        embedder=embedder,
        metadata_store=metadata,
    )
    toolbox = Toolbox(metadata_store=metadata, symbol_graph=graph, repo_id=repo_id)
    agent = QAAgent(retriever=retriever, llm=llm, toolbox=toolbox)

    async def _run() -> Answer:
        try:
            return await agent.ask(question)
        finally:
            for provider in (embedder, llm):
                aclose = getattr(provider, "aclose", None)
                if aclose is not None:
                    await aclose()

    try:
        answer = asyncio.run(_run())
    except CodeAtlasError as exc:
        err_console.print(f"[red]error:[/red] {exc}")
        raise typer.Exit(code=1) from exc
    finally:
        metadata.close()
        lexical.close()
        vector.close()

    console.print(answer.text)
    if answer.citations:
        console.print("\n[bold]Citations[/bold]")
        for citation in answer.citations:
            line = f"  {citation.path}:{citation.start_line}-{citation.end_line}"
            if citation.symbol:
                line += f" ({citation.symbol})"
            console.print(line)
    console.print(f"[dim]{answer.latency_ms} ms · {answer.token_usage.total} tokens[/dim]")
