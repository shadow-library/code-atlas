# Architecture

A user-facing walkthrough of how code-atlas turns a repository into grounded,
cited answers. For the authoritative internal contract, see
[`ARCHITECTURE.md`](../ARCHITECTURE.md).

## The pipeline

```
  repo
   │
   ▼
 Ingestion ── walk → detect language → tree-sitter AST chunk
   │                                    (fixed-window fallback)
   ▼
 Indexing ── LanceDB vectors · SQLite FTS5 (BM25) · SQLite metadata · NetworkX symbol graph
   │
   ▼
 Retrieval ── vector ∥ lexical → Reciprocal Rank Fusion → citation hydration → rerank
   │
   ▼
 Agent ────── retrieval-grounded QA + bounded LLM tool-use loop
   │
   ▼
 Answer ───── text + file:line citations + trace + latency + token usage
```

## Ingestion

The repo is walked (gitignore-aware), each file's language is detected by extension,
and source is split into chunks. Chunking is AST-aware via tree-sitter — roughly one
chunk per top-level function, class, or method — with oversized bodies split on inner
blocks. Files that can't be parsed (unsupported languages, malformed source) fall back
to fixed-window chunks so nothing is dropped.

## Indexing

Each chunk is written to four stores, keyed by a stable `chunk_id`:

- **Vectors** — LanceDB (embedded, file-based) holds chunk embeddings for semantic search.
- **Lexical** — SQLite FTS5 over content + symbol, ranked by BM25, for exact-term search.
- **Metadata** — SQLite (SQLAlchemy Core) is the canonical chunk table; both other
  stores reference it by id. Indexing is idempotent on content hash, so re-indexing an
  unchanged repo is cheap.
- **Symbol graph** — a NetworkX directed graph of `calls` / `imports` / `defines` /
  `contained_in` edges, persisted to disk.

## Retrieval

On a query, the vector and lexical searches run **in parallel** and their ranked
results are fused with **Reciprocal Rank Fusion (RRF)** — a rank-based merge that needs
no score calibration between the two very different scoring schemes. Fused hits are
hydrated back to `file:line` plus the nearest enclosing symbol, ready to render as
citations. A reranker seam sits at the end (passthrough by default, pluggable for a
cross-encoder later).

## Agent

The QA agent grounds its answer in retrieved context and runs a **bounded LLM tool-use
loop**. The LLM can call typed tools to navigate the codebase:

- `open_file(path, start, end)` — read a specific span.
- `find_symbol(name)` — locate a symbol's definition.
- `list_callers(symbol)` / `list_callees(symbol)` — traverse the symbol graph.

Every claim must cite `file:line`; the agent declines rather than invent a path when no
chunk supports the answer. It returns an `Answer` carrying the text, citations, a trace
of the tool calls, latency, and token usage.

## Providers

LLM and embedding backends sit behind async `Protocol`s resolved by name from config.
Ollama is the default for both. Adding OpenAI, Anthropic, or another backend is a new
file plus a one-line registration — nothing else in the system imports a concrete
provider.

## Surfaces

Two thin surfaces wrap the same agent stack: a `typer` CLI (`code-atlas`) for one-shot
use, and a `FastAPI` service for long-lived serving (including SSE streaming). See
[Usage](usage.md) for both.

## Evaluation harness

The harness scores the agent end-to-end: retrieval quality (recall@k, MRR, nDCG@k),
citation grounding (every cited span must exist and contain the cited text),
answer correctness via LLM-as-judge, latency (p50/p95), and token cost against a rate
card. It is a library, not a CLI subcommand — see
[Usage › Evaluation](usage.md#evaluation).
