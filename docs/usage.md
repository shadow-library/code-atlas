# Usage

code-atlas ships two surfaces over the same retrieval-grounded agent: a CLI
(`code-atlas`) and an HTTP API (`uvicorn code_atlas.api.app:app`). Both need a
running [Ollama](https://ollama.com/) for real embeddings and chat, and an indexed
repository to answer against.

## CLI

The console script `code-atlas` maps to `code_atlas.cli:app`.

### `code-atlas init`

Write a default config file (`config/default.yaml`) into the current directory.
Refuses to overwrite an existing file unless `--force` is given.

| Flag | Description |
| --- | --- |
| `--force`, `-f` | Overwrite an existing `config/default.yaml`. |

```bash
code-atlas init
code-atlas init --force
```

### `code-atlas ingest`

Index a repository into the vector, lexical, metadata, and symbol-graph stores.

| Flag | Description |
| --- | --- |
| `--repo <path>` | Path to the repository root to index. |
| `--id <repo_id>` | Stable identifier for this repository (used later by `ask`). |

```bash
code-atlas ingest --repo . --id code-atlas
```

### `code-atlas ask`

Answer a natural-language question against an indexed repository. The question is a
**positional** argument; `--repo-id` selects which indexed repo to query.

| Argument / Flag | Description |
| --- | --- |
| `<question>` | Positional. The natural-language question. |
| `--repo-id <id>` | Identifier of the repository to query (the `--id` used at ingest). |

```bash
code-atlas ask "Where is the hybrid retriever defined?" --repo-id code-atlas
```

Output: the answer text, then a `Citations` block (one line per citation as
`path:start-end (symbol)`), then a latency/token footer:

```
The hybrid retriever is defined in src/code_atlas/retrieval/hybrid.py ...

Citations
  src/code_atlas/retrieval/hybrid.py:42-88 (HybridRetriever)
1234 ms · 1567 tokens
```

## HTTP API

Run it locally with `make run-api` (uvicorn on `http://127.0.0.1:8000`).

### `GET /health`

```bash
curl http://127.0.0.1:8000/health
# {"status":"ok","version":"0.1.0"}
```

### `POST /ingest`

Schedules a background index job and returns its id immediately (`202 Accepted`).

```bash
curl -X POST http://127.0.0.1:8000/ingest \
  -H 'content-type: application/json' \
  -d '{"repo_path": ".", "repo_id": "code-atlas"}'
# 202 -> {"job_id": "<hex>", "status": "accepted"}
```

### `POST /ask`

Returns an `Answer` JSON: `text`, `citations[]`, `trace[]`, `latency_ms`, `token_usage`.

```bash
curl -X POST http://127.0.0.1:8000/ask \
  -H 'content-type: application/json' \
  -d '{"repo_id": "code-atlas", "question": "Where is the hybrid retriever defined?"}'
```

### `GET /ask/stream`

Server-sent events: `data: <token>` events as the answer is produced, then a terminal
`event: done` carrying the full `Answer` JSON.

```bash
curl -N 'http://127.0.0.1:8000/ask/stream?repo_id=code-atlas&question=Where+is+the+hybrid+retriever+defined%3F'
# data: The
# data: hybrid
# ...
# event: done
# data: {"text": "...", "citations": [...], "trace": [...], "latency_ms": 1234, "token_usage": {...}}
```

## Evaluation

There is **no `code-atlas eval` subcommand**. Evaluation is a library
(`code_atlas.evaluation`): `load_dataset`, `EvalRunner`, `write_report`, the metric
functions, and `load_cost_table` / `estimate_cost`. A seed dataset lives at
`eval/datasets/seed.yaml` (10 cases targeting this repo).

To validate a dataset offline (no Ollama, no index needed):

```bash
make eval
# seed dataset OK: 10 cases
```

A full grounded run needs a running Ollama **and** an indexed repo, and is driven
programmatically. Build the agent stack the same way the CLI does, then run the cases:

```python
import asyncio
from pathlib import Path

from code_atlas.agent.qa import QAAgent
from code_atlas.agent.tools import Toolbox
from code_atlas.config import Settings
from code_atlas.evaluation import EvalRunner, load_cost_table, load_dataset, write_report
from code_atlas.indexing.lexical_store import LexicalStore
from code_atlas.indexing.metadata_store import MetadataStore
from code_atlas.indexing.symbol_graph import SymbolGraph
from code_atlas.indexing.vector_store import LanceVectorStore
from code_atlas.providers import make_embedding, make_llm
from code_atlas.retrieval.hybrid import HybridRetriever


async def main() -> None:
    settings = Settings()
    root = Path(settings.storage.root_dir)

    metadata = MetadataStore(f"sqlite:///{Path(settings.storage.sqlite_path).resolve()}")
    lexical = LexicalStore(str((root / "lexical.sqlite").resolve()))
    vector = LanceVectorStore(settings.storage.lance_uri, dimension=settings.embeddings.dimension)
    graph_path = (root / "symbol_graph.json.gz").resolve()
    graph = SymbolGraph.load(graph_path) if graph_path.exists() else SymbolGraph()

    embedder = make_embedding(settings)
    llm = make_llm(settings)
    retriever = HybridRetriever(
        vector_store=vector, lexical_store=lexical, embedder=embedder, metadata_store=metadata
    )

    cases = load_dataset(Path("eval/datasets/seed.yaml"))
    toolbox = Toolbox(metadata_store=metadata, symbol_graph=graph, repo_id="code-atlas")
    agent = QAAgent(retriever=retriever, llm=llm, toolbox=toolbox)

    runner = EvalRunner(
        agent=agent,
        metadata_store=metadata,
        judge_llm=llm,
        cost_table=load_cost_table(Path("config/costs.yaml")),
        provider=settings.chat.provider,
        model=settings.chat.model,
        k=10,
    )
    run = await runner.run(cases)
    json_path, md_path = write_report(run, Path("eval/reports"))
    print(f"wrote {json_path} and {md_path}")


asyncio.run(main())
```

`write_report(run, out_dir)` returns `(json_path, md_path)`. The Markdown report
holds an aggregates table (recall@k, MRR, nDCG@k, grounding rate, correctness,
latency p50/p95, cost) plus a per-case breakdown.
