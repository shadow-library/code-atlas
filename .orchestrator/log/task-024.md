# Task 024 — CLI (typer): init / ingest / ask (starts Phase 7)

**Status:** done
**Deps:** 016, 023 (both done)
**Files:** pyproject.toml (modified — typer+rich deps), src/code_atlas/cli.py (new), tests/unit/test_cli.py (new)

## Apply notes / post-write fixes

The sub-agent diff applied with FOUR orchestrator corrections:

1. **ImportError risk (caught before applying):** sub-agent imported the nested settings groups (`AppSettings`, `ChatSettings`, …) from `code_atlas.config`, but the package `__init__` only re-exports `Settings` + `load_settings`. Switched those imports to `code_atlas.config.settings` (the submodule, where they live and are in `__all__`). Avoided touching `config/__init__.py` (out of task scope). **Carry-forward gotcha.**
2. **Ruff B008:** bare `typer.Option(...)` in argument defaults trips B008. Converted all three command signatures to the modern `Annotated[T, typer.Option(...)]` style (no lint suppression). Required params = Annotated without default; `--force` = `Annotated[bool, typer.Option("--force","-f")] = False`.
3. **typer 0.26.7 CliRunner has no `isolated_filesystem()`** (this typer does NOT depend on click; it ships its own rich-based CliRunner). Rewrote the 3 `init` tests to use `tmp_path` + `monkeypatch.chdir(tmp_path)` (the pattern already used in `test_settings.py`).
4. Craft cleanups: removed the redundant `_store_paths` wrapper (call `_StorePaths(settings)` directly); tightened `_run() -> Answer` (imported `Answer`) instead of `-> Any`; removed RST double-backticks from the `init` help docstring (they rendered literally in CLI help).

Ran `uv sync --extra dev` (installed typer 0.26.7, rich 14.x + annotated-doc/shellingham/markdown-it-py/mdurl). Quality gate green: ruff format/check clean, mypy clean (36 source files), **263 passed** (7 new). Smoke-tested `uv run code-atlas --help` — three commands listed, Ollama auto-registers via package import.

## Key decisions (locked)

- **Composition root lives in `cli.py`** (inline wiring + `_StorePaths` helper). No separate composition module yet — Task 025 (FastAPI) will reuse the same wiring; extract a shared builder only if duplication bites.
- **Async→sync embed shim via ONE persistent event loop** in `ingest` (NOT `asyncio.run` per batch — would break httpx client reuse across loops). `ask` uses one `asyncio.run` over the whole retrieval+chat coroutine.
- **Providers imported from the `code_atlas.providers` package** to trigger Ollama auto-registration side effects.
- **`init` builds defaults from plain nested BaseModels**, not `Settings()`, to avoid env/.env leakage into the written file.
- Did NOT modify `settings.py`/`default.yaml`; lexical + graph paths derived from `storage.root_dir`.

## Carry-forward gotchas

- Nested `*Settings` groups: import from `code_atlas.config.settings`, NOT `code_atlas.config`.
- This `typer` ships no `click` and its `CliRunner` lacks `isolated_filesystem()` — use `tmp_path` + `monkeypatch.chdir`.
- Use `Annotated[...]` for all typer params (ruff B008).
- Provider auto-register logs print at import time before `configure_logging` — benign stderr noise on `--help`.

---

## Verbatim sub-agent response

## Summary

Created `src/code_atlas/cli.py` — the composition root: a `typer.Typer` app (`app`) with three commands (`init`, `ingest`, `ask`) that wires the Ollama provider stack, the four indexing stores, the `Indexer`, `HybridRetriever`, `Toolbox`, and `QAAgent`. The async embedder is bridged to the sync `Indexer.embed` via a single persistent event loop in `ingest`; `ask` runs one `asyncio.run` over the whole retrieval+chat coroutine for httpx-client reuse. Added `typer` and `rich` runtime deps to `pyproject.toml` and offline unit tests in `tests/unit/test_cli.py` exercising only `--help` and `init`.

## Files changed

- new `src/code_atlas/cli.py`
- new `tests/unit/test_cli.py`
- modified `pyproject.toml` (add `typer`, `rich` runtime deps)

## State update

(see STATE.md Task 024 block; the orchestrator expanded it with the post-write fixes)

## Next task

025 — FastAPI app (HTTP surface over the same composition: health + `/ask` endpoint).
