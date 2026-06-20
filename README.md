# code-atlas

Grounded AI coding assistant for large, polyglot repositories — ask questions, get answers with `file:line` citations.

## Features

- Hybrid retrieval: semantic (vector) + lexical (BM25) + symbol-graph over large polyglot repos.
- Grounded answers: every claim carries a `file:line` citation; the agent declines when nothing supports it.
- Provider-agnostic: LLM and embedding backends sit behind `Protocol`s; Ollama is the default.
- Two surfaces: a `typer` CLI and a `FastAPI` HTTP API (including SSE streaming).
- AST-aware chunking via tree-sitter, with a fixed-window fallback for unparseable files.
- Built-in evaluation harness: retrieval metrics, citation grounding, LLM-as-judge correctness, latency, and token cost.

## Requirements

- Python 3.11+
- [uv](https://docs.astral.sh/uv/) for dependency and environment management
- A running [Ollama](https://ollama.com/) for real embeddings and chat (default provider)

## Install

```bash
uv sync --all-extras   # or: make install
```

## Quickstart

These commands assume a running Ollama with the default models pulled
(`ollama pull llama3.1` and `ollama pull nomic-embed-text`).

```bash
# 1. Write config/default.yaml into the current directory.
code-atlas init

# 2. Index a repository (here, this repo itself).
code-atlas ingest --repo . --id code-atlas

# 3. Ask a question about the indexed repo.
code-atlas ask "Where is the hybrid retriever defined?" --repo-id code-atlas
```

## Run the API

```bash
make run-api   # uvicorn on http://127.0.0.1:8000
```

```bash
curl http://127.0.0.1:8000/health
# {"status":"ok","version":"0.1.0"}

curl -X POST http://127.0.0.1:8000/ask \
  -H 'content-type: application/json' \
  -d '{"repo_id": "code-atlas", "question": "Where is the hybrid retriever defined?"}'
```

## Docker

Brings up Ollama (port 11434) and the API (port 8000); the app reaches Ollama at
`http://ollama:11434`.

```bash
make docker-up   # docker compose -f docker/docker-compose.yml up
```

## Configuration

Settings load from `config/default.yaml`, then `.env`, then environment variables —
**environment variables win**. Env vars are prefixed `CODE_ATLAS_`, with `__` as the
nested delimiter:

```bash
export CODE_ATLAS_OLLAMA__BASE_URL=http://localhost:11434
export CODE_ATLAS_CHAT__MODEL=llama3.1
export CODE_ATLAS_EMBEDDINGS__MODEL=nomic-embed-text
export CODE_ATLAS_EMBEDDINGS__DIMENSION=768
```

Defaults: chat model `llama3.1`, embeddings model `nomic-embed-text` (dimension 768),
provider `ollama`.

## Documentation

- [Usage](docs/usage.md) — CLI and HTTP API reference, plus running the eval harness.
- [Development](docs/development.md) — local setup, the quality gate, and project layout.
- [Architecture](docs/architecture.md) — how the ingestion → retrieval → agent pipeline fits together.

For the authoritative internal contract, see [`ARCHITECTURE.md`](ARCHITECTURE.md).

## License

MIT — see [`LICENSE`](LICENSE).
