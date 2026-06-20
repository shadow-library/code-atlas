# code-atlas developer tasks. Run `make help` to list targets.
# Recipe lines are TAB-indented (GNU make requirement).

.DEFAULT_GOAL := help

.PHONY: help install fmt lint type test test-all check eval run-api docker-build docker-up clean

help:  ## List available targets
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

install:  ## Install all deps (runtime + dev) via uv
	uv sync --all-extras

fmt:  ## Format the code (ruff format)
	uv run ruff format src tests

lint:  ## Lint the code (ruff check)
	uv run ruff check src tests

type:  ## Type-check the package (mypy --strict)
	uv run mypy src/code_atlas

test:  ## Run the fast test suite (skips slow + requires_ollama)
	uv run pytest tests/ -q -m "not slow and not requires_ollama"

test-all:  ## Run the full test suite (all markers)
	uv run pytest tests/ -q

check:  ## Run the full quality gate: format-check, lint, type, fast tests
	uv run ruff format --check src tests
	uv run ruff check src tests
	uv run mypy src/code_atlas
	uv run pytest tests/ -q -m "not slow and not requires_ollama"

eval:  ## Smoke-validate the seed eval dataset offline (a full grounded run needs Ollama + an indexed repo; see docs/usage.md)
	uv run python -c "from pathlib import Path; from code_atlas.evaluation import load_dataset; cases = load_dataset(Path('eval/datasets/seed.yaml')); print(f'seed dataset OK: {len(cases)} cases')"

run-api:  ## Run the HTTP API with autoreload on 127.0.0.1:8000
	uv run uvicorn code_atlas.api.app:app --reload --host 127.0.0.1 --port 8000

docker-build:  ## Build the Docker images (app + ollama)
	docker compose -f docker/docker-compose.yml build

docker-up:  ## Start the stack (ollama on 11434, api on 8000)
	docker compose -f docker/docker-compose.yml up

clean:  ## Remove tooling caches and build artifacts (keeps data/ and eval/reports/)
	rm -rf .mypy_cache .ruff_cache .pytest_cache
	rm -rf dist build .coverage htmlcov
	rm -rf src/*.egg-info *.egg-info
	find . -type d -name __pycache__ -prune -exec rm -rf {} +
