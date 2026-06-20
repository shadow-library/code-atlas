# Task 025 — FastAPI app (/health, /ingest, /ask, SSE stream) — closes Phase 7

**Status:** done
**Deps:** 016, 023 (both done)
**Files:** pyproject.toml (modified — fastapi+uvicorn), src/code_atlas/api/{__init__,app,routes,models}.py (new), tests/unit/api/test_app.py (new)

## Apply notes / post-write fixes

1. **B008 (caught before applying):** sub-agent used `make_agent: AgentFactory = Depends(get_agent_factory)` — ruff B008 flags `Depends()` in argument defaults (FastAPI's `Depends` is not in ruff's immutable-calls allowlist, same as `typer.Option`). Applied with route params converted to `Annotated[T, Depends(...)]` (modern FastAPI idiom; no ruff-config change). Added `from typing import Annotated` to routes.py.
2. Ran `uv sync --extra dev` (installed fastapi/starlette 1.3.1, uvicorn 0.49.0, uvloop, httptools, watchfiles, websockets).
3. Ruff format reflowed `test_app.py` (the multi-line `Citation(...)` ctor + a dependency-override lambda). Auto-fixed.

Quality gate green: ruff format/check clean, mypy clean (40 source files), **268 passed** (5 new). The real `app` object is exercised end-to-end via `TestClient` (routes register, `response_model=Answer` serializes).

## Key decisions (locked)

- **API does its OWN composition** (lifespan-managed stores on `app.state`), NOT a shared builder with the CLI — lifecycles differ (CLI one-shot/sync-shimmed vs API long-lived/async-native). Shared-builder extraction deferred.
- **Offline testability via `dependency_overrides`** + `TestClient(app)` **without** the `with` context-manager (so the lifespan never opens real stores). The two dependency providers (`get_agent_factory`, `get_ingest_runner`) are the only request-time touch points for `app.state`/real I/O.
- Dependency providers live in `routes.py` (not `app.py`) to avoid an app↔routes import cycle.
- `/ask` returns the domain `Answer` directly as `response_model`.
- `/ask/stream` REPLAYS the computed `Answer` as SSE (QAAgent is non-streaming): `data: <token>\n\n` per whitespace token + terminal `event: done\ndata: <json>\n\n`. True streaming deferred.
- `/ingest` default runner mirrors CLI ingest (own embedder + 4 stores + persistent loop shim, runs in threadpool background-task thread).

## Carry-forward gotchas

- `Depends`/`Query`/`typer.Option` → always use `Annotated[...]` (ruff B008).
- `TestClient(app)` without `with` skips the lifespan — the offline-test lever for FastAPI apps that open real resources at startup.
- **v1 limitation**: in-memory `app.state.graph` goes stale after a background ingest writes a new graph to disk; restart reloads it (real-time index updates out of scope).
- `get_ingest_runner._run` does not catch `CodeAtlasError` (no client to return it to in a background task); failures propagate to the threadpool and are logged by the framework. Add an explicit try/except if failure visibility is needed later.

---

## Verbatim sub-agent response (abridged — full diff applied with the Annotated fix)

## Summary

Implements Task 025 — the FastAPI HTTP API that closes Phase 7. Adds a new `code_atlas.api` package with four modules (`models.py`, `routes.py`, `app.py`, `__init__.py`) exposing a long-lived async service with `/health`, `/ask`, `/ingest`, and `/ask/stream` (SSE) endpoints. The API does its own lifespan-managed composition (4 stores + embedder + llm + retriever on `app.state`), mirroring `cli.py`'s store-path derivation and the persistent-event-loop sync embed shim. Offline testability via two dependency providers overridden in tests + `TestClient(app)` without `with`. Adds `fastapi` + `uvicorn[standard]` and 5 offline tests.

## Files changed

- pyproject.toml (fastapi, uvicorn[standard])
- src/code_atlas/api/__init__.py (re-export app, create_app)
- src/code_atlas/api/models.py (HealthResponse, IngestRequest, IngestResponse, AskRequest)
- src/code_atlas/api/routes.py (router, get_agent_factory, get_ingest_runner, _event_stream, 4 routes)
- src/code_atlas/api/app.py (lifespan, create_app, app)
- tests/unit/api/test_app.py (5 offline tests)

## Next task

026 — Eval dataset format + seed dataset.
