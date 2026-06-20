# Task 032 — Makefile + developer docs (FINAL TASK)

**Status:** done — completes Phase 9 and the entire planned build.
**Files:** `Makefile` (new, root), `README.md` (rewrite), `docs/usage.md` (new), `docs/development.md` (new), `docs/architecture.md` (new)

## Apply notes
- No Python changes → no ruff/mypy/pytest gate impact. Verified the targets + links directly:
  - `make help` lists all targets (proves Makefile recipes are real TABs — make rejects space indentation).
  - `make eval` → `seed dataset OK: 10 cases`.
  - `make test` → 317 passed.
  - Link check: every relative link in README + docs resolves (`docs/usage.md`, `docs/development.md`, `docs/architecture.md`, `ARCHITECTURE.md`, `LICENSE`, and docs-internal `usage.md` / `architecture.md` / `../ARCHITECTURE.md`).
- Confirmed `LICENSE`, `ARCHITECTURE.md`, `.pre-commit-config.yaml`, `eval/datasets/seed.yaml`, `config/costs.yaml` all exist before linking them (no broken links).
- Applied sub-agent diffs verbatim; wrote the Makefile with literal tab indentation.

## Known limitation / follow-up
- `make eval` is an OFFLINE smoke (validates the seed dataset only). There is no `code-atlas eval` CLI subcommand; full grounded eval is the `code_atlas.evaluation` library and needs Ollama + an indexed repo. Documented in `docs/usage.md`. Candidate **Task 033**: add a `code-atlas eval` subcommand.

## Sub-agent response (verbatim excerpt — Summary + State update)

### Summary
Task 032 (the final task) adds developer tooling and docs. Root `Makefile` with a self-documenting `help` default goal and the full target set (`install`, `fmt`, `lint`, `type`, `test`, `test-all`, `eval`, `run-api`, `docker-build`, `docker-up`, `clean`, plus bonus `check`), all `.PHONY`, TAB-indented recipes. Fully rewrote the outdated `README.md` (which falsely claimed the CLI wasn't wired) and created `docs/usage.md` (CLI + HTTP API + eval-as-library), `docs/development.md` (setup, gate, markers, pre-commit, docker, layout, contributing), `docs/architecture.md` (narrative pipeline + ASCII diagram). Every fact verified against source (CLI flags, API endpoints/SSE, Answer shape, EvalRunner/write_report/load_cost_table signatures, CODE_ATLAS_ env scheme, seed dataset 10 cases, compose services, pre-commit). Confirmed NO `code-atlas eval` subcommand — eval make target is an offline dataset-validation smoke. No .py changes.

### State update (sub-agent)
- Makefile: TAB recipes, `.DEFAULT_GOAL := help`, all `.PHONY`. install/fmt/lint/type/test(fast)/test-all/check/eval/run-api/docker-build/docker-up/clean (clean keeps data/ + eval/reports/).
- `make eval` = offline seed-dataset smoke; full grounded eval is the library, needs Ollama + indexed repo.
- README rewritten; docs/usage.md, docs/development.md, docs/architecture.md created; all internal links resolve.

## Next task
None — Task 032 was the final task in TASKS.md. Project feature-complete. Optional follow-up: Task 033 (`code-atlas eval` CLI subcommand).
