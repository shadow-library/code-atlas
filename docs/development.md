# Development

## Local setup

```bash
uv sync --all-extras   # or: make install
```

This installs both runtime and dev dependencies (`ruff`, `mypy`, `pytest`,
`pytest-asyncio`, `pre-commit`, ...), declared under
`[project.optional-dependencies].dev` in `pyproject.toml`.

## Quality gate

Every commit must pass all four checks:

```bash
uv run ruff format --check src tests
uv run ruff check src tests
uv run mypy src/code_atlas
uv run pytest tests/ -q
```

The `make` equivalents:

| Command | Runs |
| --- | --- |
| `make fmt` | `ruff format src tests` (writes changes) |
| `make lint` | `ruff check src tests` |
| `make type` | `mypy src/code_atlas` |
| `make test` | `pytest -q -m "not slow and not requires_ollama"` (fast subset) |
| `make test-all` | `pytest -q` (all markers) |
| `make check` | format-check + lint + type + fast tests in one go |

## Test markers

Tests carry markers declared in `pyproject.toml`: `slow`, `network`,
`requires_ollama`. CI runs `-m "not slow and not requires_ollama"`. Run subsets with
`-m`:

```bash
uv run pytest tests/ -q -m "not slow and not requires_ollama"   # fast (CI default)
uv run pytest tests/ -q -m "requires_ollama"                    # only Ollama-dependent
uv run pytest tests/ -q                                          # everything
```

## Pre-commit

A `.pre-commit-config.yaml` runs ruff-format, ruff (with `--fix`), a set of
pre-commit-hooks checks, and mypy. Run it across the tree:

```bash
uv run pre-commit run --all-files
```

## Eval smoke

```bash
make eval
# seed dataset OK: 10 cases
```

This validates `eval/datasets/seed.yaml` offline. A full grounded eval run needs
Ollama + an indexed repo and is driven programmatically — see
[Usage › Evaluation](usage.md#evaluation).

## Docker dev

```bash
make docker-build   # docker compose -f docker/docker-compose.yml build
make docker-up      # ollama on 11434, api on 8000
```

## Project layout

```
code-atlas/
├── src/code_atlas/
│   ├── cli.py          typer CLI (composition root)
│   ├── ingestion/      repo walk, language detection, tree-sitter chunking
│   ├── indexing/       vector / lexical / metadata / symbol-graph stores + indexer
│   ├── providers/      LLM + embedding Protocols, registry, Ollama default
│   ├── retrieval/      hybrid retrieval, RRF fusion, citation hydration, reranker
│   ├── agent/          QA orchestrator, prompts, LLM-callable tools
│   ├── evaluation/     datasets, metrics, runner, report writer
│   └── api/            FastAPI app, routes, request/response models
├── tests/              mirrors the src tree
├── eval/datasets/      seed eval dataset
├── config/             default.yaml, costs.yaml
└── docker/             Dockerfile + docker-compose.yml
```

## Contributing

- **Commits:** Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, ...).
- **Types:** `mypy --strict` must pass; `from __future__ import annotations` everywhere.
- **Style:** 120-column lines; `ruff` is the single source of formatting and lint truth.
- **Tests:** mirror `src/` under `tests/`; mark Ollama/slow cases appropriately.

The authoritative internal contract is [`ARCHITECTURE.md`](../ARCHITECTURE.md).
