# Task 033 — `code-atlas eval` CLI subcommand

**Status:** done (Phase 10 follow-up — was proposed after the planned 32-task build completed)
**Files:** `src/code_atlas/cli.py` (new `run_eval` command + import), `tests/unit/test_cli.py` (offline `--help` test + help-list rename), `docs/usage.md` (Evaluation section rewrite)

## Apply notes
- Applied sub-agent diff. Two deviations: (1) renamed `test_help_lists_three_commands` → `test_help_lists_commands` (the assert set grew to 4 commands; old name was misleading). (2) `ruff format` reflowed the long `--dataset`/`--out` `Annotated` option lines in `run_eval` (>120 cols) — one post-write format pass.
- Gate clean after format: ruff format ✓, ruff check ✓, mypy ✓ (48 source files), pytest ✓ (318 passed, +1).
- Live smoke: `uv run code-atlas eval --help` renders Usage + all four flags (`--repo-id [required]`, `--dataset` default `eval/datasets/seed.yaml`, `--k` default 10, `--out`).

## Sub-agent response (verbatim — Summary + State update)

### Summary
Adds the `code-atlas eval` CLI subcommand that runs the evaluation harness end-to-end (loading the dataset + cost table, building the full agent stack like `ask`, running `EvalRunner`, writing JSON + Markdown reports, and printing an aggregates summary). The dataset/cost-table load happens inside a guard *before* any store is opened, so bad input fails clean without leaking handles. Also adds an offline `--help` unit test and updates `docs/usage.md` to document the new command as the primary path (keeping the programmatic library snippet reframed as the alternative).

### State update
- Added `run_eval` (`@app.command(name="eval")`) to `cli.py`: loads dataset + cost table under a `CodeAtlasError` guard before opening any store, then mirrors `ask`'s stack assembly and the `asyncio.run(_run())` async→sync shim (nested `_run` annotated `-> EvalRun`, providers `aclose`d in `finally`, 3 stores closed in outer `finally`).
- `EvalRunner` driven with `provider=settings.chat.provider`, `model=settings.chat.model`, `k`; `write_report(run, out_dir)` returns `(json_path, md_path)`; rich aggregates summary printed.
- New import `from code_atlas.evaluation import EvalRun, EvalRunner, load_cost_table, load_dataset, write_report`.
- Tests: `test_eval_help_shows_flags`; `test_help_lists_*` also asserts `eval`.
- Docs: `docs/usage.md` `## Evaluation` documents the `eval` subcommand first; programmatic snippet reframed as the alternative.
- `--repo-id` binds the Toolbox/index; per-case grounding still uses each `EvalCase.repo_id`. CLI composition duplicated across `ask`/`eval` per the established pattern.

## Next task
None outstanding. Project + this follow-up complete. Future candidates if desired: extract the shared CLI composition root (now 3 duplicated call-sites), true incremental SSE streaming, or additional language edge extractors.
