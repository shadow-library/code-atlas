# Task 031 — Docker + docker-compose (Ollama)

**Status:** done (Phase 9 starts)
**Files:** `docker/Dockerfile` (new), `docker/docker-compose.yml` (new), `.dockerignore` (new, repo root)

## Apply notes
- No Python changes → no ruff/mypy/pytest gate. Verified instead against a LIVE Docker daemon (Docker 28.5.1, Compose v2.40):
  - `docker compose -f docker/docker-compose.yml config` → VALID.
  - `docker compose -f docker/docker-compose.yml build code-atlas` → succeeded; image `docker-code-atlas:latest`, 1.04 GB (lancedb/pyarrow/numpy dominate).
  - `docker run --rm docker-code-atlas:latest code-atlas --help` → exit 0, printed init/ingest/ask command list.
- Applied sub-agent diff verbatim (no edits). Built image left in local Docker (not removed).

## Sub-agent response (verbatim)

All facts verified: module-level `app = create_app()` at line 85, `config/` has YAML files (keep), `eval/` exists with only `datasets` (the task excludes `eval/reports/` specifically). README.md present, uv.lock present. python:3.12-slim satisfies `requires-python >=3.11`.

### Summary
Three new infra files for containerized deployment, no Python changes:
- **docker/Dockerfile** — multi-stage. `builder` copies the static `uv` binary from `ghcr.io/astral-sh/uv:latest`, layer-caches deps via `uv sync --frozen --no-dev --no-install-project` (only pyproject/uv.lock/README.md copied first — README required because `pyproject.readme = "README.md"`), then copies `src/` and `uv sync --frozen --no-dev`. `runtime` is clean `python:3.12-slim` + non-root `appuser` (uid 1000), prebuilt `/app/.venv` + `/app/src` copied across, `/app/.venv/bin` on PATH.
- **CMD-vs-ENTRYPOINT**: server via `CMD ["uvicorn", "code_atlas.api.app:app", ...]`, NOT ENTRYPOINT — acceptance runs `docker run <img> code-atlas --help`; ENTRYPOINT would append the arg to uvicorn and fail. CMD lets default run start the API and the trailing command override it.
- **Editable-install gotcha**: `uv sync` editable `.pth` points at `/app/src` → runtime MUST copy `/app/src` to the same absolute path or imports break.
- **docker/docker-compose.yml** — `ollama` (ollama/ollama:latest, 11434, vol ollama-models:/root/.ollama) + `code-atlas` (`context: ..` = repo root, `dockerfile: docker/Dockerfile`, 8000, depends_on ollama, env `CODE_ATLAS_OLLAMA__BASE_URL=http://ollama:11434`). No obsolete `version:`.
- **.dockerignore** (repo root) keeps src/pyproject/uv.lock/README.md/config; excludes .git/.venv/caches/tests/eval reports/data/.orchestrator/.github/docs.

### Files changed
- `docker/Dockerfile` (new)
- `docker/docker-compose.yml` (new)
- `.dockerignore` (new, repo root)

### Next task
Task 032 — Makefile + developer docs (deps 031 ✓).
