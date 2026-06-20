# State

> Caveman-style fact sheet. Bullets, not prose. Append per-task sections under
> "Capabilities" as tasks complete. Compact when this file exceeds ~300 lines.

## Stack
- Python 3.11+. uv-managed. src/ layout.
- Lint+format: ruff. Type: mypy --strict. Tests: pytest.
- CLI: typer. API: FastAPI + uvicorn. Config: pydantic-settings + YAML.
- Logging: structlog (JSON in prod, console in dev).
- Vector: LanceDB embedded. Lexical: SQLite FTS5. Metadata: SQLite (SQLAlchemy Core).
- Parsing: tree-sitter + tree-sitter-language-pack.
- Providers: Protocols. Default = Ollama (chat + embeddings). httpx async.
- Docker: python:3.12-slim multi-stage, non-root.

## Conventions
- Errors: typed in code_atlas.errors. Carry context dict. No bare Exception.
- Logging: structlog per-module logger. Never print. Levels: debug/info/warning/error.
- Config: single Settings via pydantic-settings. yaml → .env → env (env wins).
  Injected; no module-global env reads.
- Naming: snake_case files/funcs, PascalCase classes, UPPER_SNAKE constants.
- Tests mirror src tree. Markers: slow, network, requires_ollama.
- Types: `from __future__ import annotations`. Protocol over ABC for seams.
- Async: I/O async-first; CPU sync via asyncio.to_thread.
- Commits: Conventional Commits, no AI co-authors, small + atomic at green.
- Line width 120. Indent: 4 spaces Python, 2 spaces yaml/json/md.
- Layering: domain → indexing/providers → retrieval → agent → cli/api.

## Capabilities (by task)

### Task 035 — True incremental SSE streaming (`QAAgent.ask_stream`)
- **`QAAgent.ask_stream(question) -> AsyncIterator[StreamEvent]`** (new async generator; `ask` UNCHANGED, still uses non-streaming `chat`). Mirrors `ask`'s retrieve + bounded tool-use loop but drives `self._llm.chat_stream(...)`, yielding `StreamEvent(type="token", text=delta)` per `content_delta` as it arrives, then a terminal `StreamEvent(type="done", answer=Answer(...))`.
- **`StreamEvent`** (module-level frozen `@dataclass` in `agent/qa.py`): `type: Literal["token","done"]`, `text: str = ""`, `answer: Answer | None = None`. Exported from `agent.qa` + `agent` (`__all__` sorted). Plain dataclass → field annotations NOT resolved at runtime (`Answer` already runtime-imported anyway; see Task 034 carry-forward).
- **Per-turn accumulation**: each loop turn collects `content_parts`/`tool_calls`/`usage`/`finish_reason` from the chunk stream; usage summed across turns (Ollama puts usage on the final `done=True` chunk). `tool_call_delta` chunks appended to `tool_calls`. After stream: if `not tool_calls or iteration == max_tool_iters` → `final_content = content`, break; else append assistant turn + execute tools (reuses `_run_tool`) + append tool messages.
- **Token-leak note (documented)**: deltas stream for EVERY turn; in practice Ollama tool-call turns carry empty `content_delta`, so only the FINAL turn's tokens surface. The terminal `done` `Answer` is authoritative — `Answer.text` = final turn's content (not the cross-turn token concatenation). Can't know a turn is terminal until its stream ends, so this is the pragmatic correct-enough contract (not buffering, which would kill incrementality).
- **API `_event_stream` rewrite** (`api/routes.py`): consumes `agent.ask_stream`; new `_sse_data(text)` helper frames multi-line payloads SSE-correctly (`"".join(f"data: {line}\n" for line in text.split("\n")) + "\n"`) — fixes embedded-newline framing the old raw `data: <token>` had; terminal `event: done\ndata: <answer json>\n\n`; `CodeAtlasError` mid-stream → `event: error\ndata: {json}\n\n`. Added `import json` + `from code_atlas.errors import CodeAtlasError`. Route handler `ask_stream` + `StreamingResponse(..., media_type="text/event-stream")` unchanged.
- **Supersedes the Task 025 deferral** ("QAAgent has no streaming method, SSE replays the computed Answer"). Now genuinely incremental.
- Tests: `StreamingFakeLLM` (canned per-turn `list[list[ChatChunk]]`, `chat` raises NotImplementedError) + 3 cases in `test_qa.py` (single-turn: tokens concat == answer text + done total 20; tool-then-answer: final-turn text + total 35 + `find_symbol` trace entry; blank-question raises on first `__anext__` — forced via list-comp). `test_app.py` `FakeAgent` gained `ask_stream` (split-word tokens + done); existing `test_ask_stream_sse` still green. Gate clean (no post-write fixes): 48 source files, **321 tests** (+3).
- Carry-forward: an async-generator's pre-yield `raise` surfaces on first `__anext__` — tests must force iteration (`[e async for e in gen]`) to trigger it.

### Task 034 — Extract shared CLI composition root (refactor)
- **Behavior-preserving** de-dup in `cli.py`. Added `_QAStack` dataclass + `_build_qa_stack(settings, paths, repo_id) -> _QAStack` factory (placed after `_StorePaths`); `ask` and `eval` now build/teardown via it. `ingest` UNTOUCHED (distinct lifecycle: persistent `asyncio.new_event_loop()` for the sync Indexer shim, FRESH `SymbolGraph()`, no llm/retriever/agent).
- **`_QAStack`** (`@dataclass`): fields `embedder, llm, metadata, lexical, vector, agent` + `async aclose_providers()` (getattr-guarded `aclose` over embedder+llm) + `close_stores()` (metadata/lexical/vector). `metadata` kept as a field because eval's `EvalRunner` needs it (+ `llm` as judge).
- Commands keep their OWN `asyncio.run(_run())` shim and try/except/finally — only the construction + the two teardown blocks were lifted. Close order preserved exactly (providers in `_run` finally, 3 stores in outer finally). No flag/output/exit-code change (`ask`/`eval --help` verified identical).
- **Dataclass-annotation note (carry-forward)**: `EmbeddingProvider`/`LLMProvider` imported under `TYPE_CHECKING` — a PLAIN `@dataclass` under `from __future__ import annotations` does NOT resolve field annotations at runtime (unlike pydantic), so Protocol field types stay out of the runtime import graph. (Contrast Task 030: pydantic field types MUST be runtime imports.)
- Net: each of `ask`/`eval` dropped ~12 lines of construction + the duplicated teardown loop. Gate clean first-try (no format reflow — the `HybridRetriever(...)` one-liner is 119 cols): 48 source files, **318 tests** pass.
- The Task 024/025 "deferred shared composition root" is now resolved for the QA path. (`ingest` intentionally still bespoke.)

### Task 033 — `code-atlas eval` CLI subcommand (Phase 10 follow-up)
- New `eval` command in `cli.py`: `@app.command(name="eval")` + function **`run_eval`** (named to avoid shadowing the `eval` builtin; `name="eval"` keeps the CLI verb).
- **Signature**: `--repo-id` (REQUIRED, no default — must come first since it has no default), `--dataset` (default `Path("eval/datasets/seed.yaml")`), `--k` (default 10), `--out` (`Path | None`, default None → falls back to `settings.eval.reports_dir`).
- **Body order**: `Settings()` + `configure_logging` + `_StorePaths` + resolve `out_dir`; then **load `load_dataset(dataset)` + `load_cost_table(settings.eval.cost_rates_path)` inside a `CodeAtlasError` guard BEFORE opening any store** (clean Exit(1) on bad dataset/costs, no leaked store handles); then build the stack exactly like `ask` (embedder+llm+metadata/lexical/vector+graph+HybridRetriever+Toolbox(repo_id)+QAAgent); `EvalRunner(agent, metadata_store=metadata, judge_llm=llm, cost_table, provider=settings.chat.provider, model=settings.chat.model, k)`.
- **Async→sync shim = `ask` pattern** (NOT ingest's persistent loop): one `asyncio.run(_run())`, nested `_run() -> EvalRun` `aclose()`s embedder+llm in `finally`, outer `finally` closes the 3 stores. `write_report(run, out_dir) -> (json_path, md_path)`; rich aggregates summary (recall/mrr/ndcg/grounding/correctness/latency p50-p95/total cost) + report paths printed.
- New import: `from code_atlas.evaluation import EvalRun, EvalRunner, load_cost_table, load_dataset, write_report` (sorted after `errors`, before `indexing`).
- `--repo-id` binds the Toolbox/index to evaluate; per-case grounding still uses each `EvalCase.repo_id` inside the runner (v1 single-repo → pass the dataset's repo_id, e.g. `code-atlas`).
- CLI composition is DUPLICATED across `ask`/`ingest`/`eval` (established pattern; the deferred shared composition-root extraction from Task 024/025 still stands — now 3 call-sites, so extraction is more justified if a 4th appears).
- Tests: `test_eval_help_shows_flags` (offline `--help` asserts `--repo-id`/`--dataset`/`--k`); renamed `test_help_lists_three_commands` → `test_help_lists_commands` (+`assert "eval"`; name was stale with 4 commands). Full execution still needs live Ollama (out of unit scope) — offline `--help` only, per the Task 024 convention.
- Docs: `docs/usage.md` `## Evaluation` now documents `code-atlas eval` FIRST (flags table + example), keeps the `make eval` offline smoke and the programmatic-library snippet (reframed as "or drive it directly"). No longer claims the subcommand is absent. README needed no change (only links usage.md; no false claim).
- Post-write fix: `ruff format` reflowed the long `--dataset`/`--out` `Annotated` option lines (>120). Gate then clean: 48 source files, **318 tests** (+1). Verified `code-atlas eval --help` renders (`--repo-id [required]`, `--dataset` default seed.yaml, `--k` default 10, `--out`).

### Task 032 — Makefile + developer docs — **PHASE 9 + PLANNED BUILD COMPLETE**
- New `Makefile` (repo root, TAB-indented recipes, `.DEFAULT_GOAL := help`, all `.PHONY`; `help` greps `## ` doc comments + awk-colorizes). Targets: `install` (`uv sync --all-extras`), `fmt` (`ruff format`), `lint` (`ruff check`), `type` (`mypy src/code_atlas`), `test` (`pytest -q -m "not slow and not requires_ollama"`), `test-all` (`pytest -q`, all markers), `check` (full gate: format-check+lint+type+fast tests), `eval` (offline seed-dataset smoke), `run-api` (`uvicorn ... --reload 127.0.0.1:8000`), `docker-build`, `docker-up`, `clean` (caches/build artifacts only — KEEPS `data/` + `eval/reports/`).
- **`make eval` is a SMOKE, not a real eval run**: `python -c "load_dataset(eval/datasets/seed.yaml) → print count"` (prints `seed dataset OK: 10 cases`). There is **NO `code-atlas eval` CLI subcommand** — full grounded eval is the `code_atlas.evaluation` library (`EvalRunner`/`write_report`) and needs Ollama + an indexed repo (documented in `docs/usage.md`). **Candidate follow-up Task 033**: add a `code-atlas eval` CLI subcommand wiring `EvalRunner` + `write_report` + `EvalSettings.cost_rates_path`/`reports_dir`.
- **README fully rewritten** — deleted the stale "CLI not wired until task 024" stub. Now: Features, Requirements (Py3.11+/uv/Ollama), Install, Quickstart (init→ingest→ask), Run the API (curl /health + /ask), Docker, Configuration (`CODE_ATLAS_` prefix + `__` nesting, env wins), Documentation links, MIT license. README is also the package `readme` (pyproject) + used in Docker build.
- New `docs/usage.md` (CLI flags + each API endpoint w/ curl incl. SSE + eval-as-library Python snippet mirroring `cli.py` composition), `docs/development.md` (setup, 4-cmd gate + `make` equivalents table, markers, pre-commit, eval smoke, docker, project-layout tree, contributing → points to `ARCHITECTURE.md`), `docs/architecture.md` (user-facing narrative + ASCII pipeline diagram; NOT a copy of the internal `ARCHITECTURE.md` contract).
- **Acceptance VERIFIED**: `make help` lists targets (proves Makefile tabs valid — make rejects space indent); `make eval` → `seed dataset OK: 10 cases`; `make test` → **317 passed**; all README/docs relative links resolve (`docs/*.md`, `ARCHITECTURE.md`, `LICENSE`, `../ARCHITECTURE.md`, `usage.md` — all exist). No `.py` changes → ruff/mypy gate untouched.
- **PROJECT COMPLETE.** All 32 planned tasks `[done]` across Phases 1–9 (Foundation → Ingestion → Indexing → Providers → Retrieval → Agent → CLI+API → Evaluation → Infra+Docs). End-to-end: ingest a repo → hybrid-retrieve → grounded QA with citations, via CLI + HTTP API + Docker, with an evaluation harness. Only open follow-up: optional Task 033 (`code-atlas eval` subcommand).

### Task 031 — Docker + docker-compose (Ollama) — Phase 9 starts
- New infra files (NO `.py` changes; verified against a LIVE Docker daemon): `docker/Dockerfile`, `docker/docker-compose.yml`, `.dockerignore` (repo ROOT).
- **Dockerfile** — multi-stage uv builder → `python:3.12-slim` runtime, non-root `appuser` (uid 1000):
  - uv binary copied from `ghcr.io/astral-sh/uv:latest` (`COPY --from=... /uv /bin/uv`). `ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy UV_PYTHON_DOWNLOADS=0` (use base-image Python, don't fetch a managed one).
  - **Layer-cache pattern**: `COPY pyproject.toml uv.lock README.md ./` → `uv sync --frozen --no-dev --no-install-project` (deps only), THEN `COPY src ./src` → `uv sync --frozen --no-dev` (installs project). **README.md MUST be in context** (`pyproject.readme = "README.md"`) or the build fails — `.dockerignore` keeps it.
  - **GOTCHA (carry-forward)**: `uv sync` installs the project EDITABLE — a `.pth` records `/app/src`. Runtime stage MUST copy BOTH `/app/.venv` AND `/app/src` to the SAME absolute path or imports break. Both copied with `--chown=appuser:appuser`. PATH gets `/app/.venv/bin` prepended.
  - **DECISION: server via `CMD`, not `ENTRYPOINT`** — `CMD ["uvicorn","code_atlas.api.app:app","--host","0.0.0.0","--port","8000"]`. Acceptance runs `docker run <img> code-atlas --help`; `ENTRYPOINT ["uvicorn",...]` would APPEND those args to uvicorn and fail. `CMD` lets default run start the API while a trailing command overrides it. Both `uvicorn` + `code-atlas` resolve via `.venv/bin` on PATH. `EXPOSE 8000`.
- **docker-compose.yml** (in `docker/`): `ollama` (image `ollama/ollama:latest`, port 11434, named vol `ollama-models:/root/.ollama`) + `code-atlas` (`build: {context: .., dockerfile: docker/Dockerfile}` — **context `..` = repo root** since compose lives in `docker/`; port 8000; `depends_on: [ollama]`; `restart: unless-stopped`). **Ollama wiring via env `CODE_ATLAS_OLLAMA__BASE_URL=http://ollama:11434`** (prefix `CODE_ATLAS_`, nested delimiter `__`). No obsolete `version:` key (Compose v2 warns). Top-level `volumes: {ollama-models: {}}`.
- **.dockerignore** (repo root): excludes `.git`, `.venv`, mypy/ruff/pytest caches, `**/__pycache__`, `tests/`, `eval/reports/`, `data/`, `docs/`, `.orchestrator/`, `.github/`, `.pre-commit-config.yaml`, `.env*`, `docker/`. KEEPS `src/`, `pyproject.toml`, `uv.lock`, `README.md`, `config/`.
- **Acceptance VERIFIED on live daemon (Docker 28.5.1, Compose v2.40)**: `docker compose -f docker/docker-compose.yml config` valid ✓; `docker compose build code-atlas` succeeds (image `docker-code-atlas:latest`, **1.04 GB** — heavy from lancedb/pyarrow/numpy) ✓; `docker run --rm docker-code-atlas:latest code-atlas --help` exits 0, prints init/ingest/ask ✓. (Two structlog provider-registration lines hit stderr at import before help — known/harmless per Task 024.)
- **Phase 9 (Infra & Docs) started.** Next: Task 032 (Makefile + developer docs) — deps 031 ✓.

### Task 030 — Latency + cost tracking, eval runner, report writer — **Phase 8 COMPLETE**
- Three new modules + `config/costs.yaml`, all re-exported from `evaluation/__init__` (`__all__` RUF022-sorted, ASCII case-sensitive: ALL uppercase names before lowercase). New public names: `CostRate, CostTable, estimate_cost, load_cost_table, Agent, CaseResult, EvalAggregates, EvalRun, EvalRunner, render_markdown, write_report`.
- **`metrics_cost.py`**: `CostRate` (frozen pydantic `{prompt_per_1k, completion_per_1k: float=0.0}`), `CostTable = dict[str, dict[str, CostRate]]` (provider→model→rate). `load_cost_table(path) -> CostTable` mirrors `datasets.py` error contract (`costs: cannot read file` / `costs: invalid YAML` / `costs: invalid rate table` w/ `{path, error}`). `estimate_cost(usage, *, provider, model, table) -> float` = `prompt/1000*p_rate + completion/1000*c_rate`; **lookup `models.get(model) or models.get("default")`** → provider's `default` model fallback; missing → `log.warning("cost.rate_missing")` + `0.0`. **Gotcha**: the `or` fallback relies on a `CostRate` instance always being truthy (pydantic models are) — if a falsy rate is ever introduced, switch to explicit `in` checks.
- **`runner.py`**: `Agent` **structural Protocol** (`async ask(question)->Answer`) so the runner does NOT import `agent` (layering stays clean). `EvalRunner(*, agent, metadata_store, judge_llm, cost_table, provider, model, k=10)`; `async run(cases, *, run_id=None) -> EvalRun`. Per case (errors PROPAGATE, no per-case try/except): `answer = await agent.ask(q)`; `retrieved = [c.path for c in answer.citations]` (faithful proxy — QAAgent emits one citation per retrieved chunk); recall/mrr/ndcg @ `k`; `check_grounding(answer, store, repo_id=case.repo_id)` (SYNC, no thread → `:memory:` store OK); `await judge_answer(...)`; `estimate_cost(answer.token_usage, ...)`.
- **Records (all frozen pydantic)**: `CaseResult{case_id, recall_at_k, mrr, ndcg_at_k, grounding: GroundingReport, correctness: CorrectnessReport, latency_ms, cost_usd, token_usage: TokenUsage}`; `EvalAggregates{n_cases, k, mean_recall_at_k, mean_mrr, mean_ndcg_at_k, mean_grounding_rate, mean_correctness, latency_p50_ms, latency_p95_ms, total_cost_usd, mean_cost_usd}`; `EvalRun{run_id, k, cases, aggregates}`.
- **Aggregate helpers** (pure, module-level): `_mean` (empty→0.0), `_grounding_rate` (`grounded/total`, vacuous `1.0` when `total==0`), `_percentile(values: list[int], pct)` (empty→0.0, single→float, else **linear interpolation** between floor/ceil ranks — returns float on all paths). `run_id` default = `f"{datetime.now(UTC):%Y%m%dT%H%M%S}-{uuid4().hex[:8]}"` (filesystem-safe, NO colons).
- **PYDANTIC-FIELD IMPORT FOOTGUN (carry-forward)**: under `from __future__ import annotations`, pydantic resolves field types at class-build via `get_type_hints` → any type used as a **pydantic field annotation MUST be a RUNTIME import**. In `runner.py`: `GroundingReport`, `CorrectnessReport`, `TokenUsage` are runtime; `Answer`/`EvalCase`/`MetadataStore`/`LLMProvider`/`CostTable` stay under `TYPE_CHECKING` (signature-only, lazy). Protocol method return types are also lazy → `Answer` TYPE_CHECKING is fine for `Agent`.
- **`report.py`**: `render_markdown(run) -> str` emits EXACT headers `# Eval report: {run_id}` / `## Aggregates` / `## Per-case results` with Markdown tables (means `.4f`, costs `.6f`, grounded as `g/total`). `write_report(run, out_dir) -> (json_path, md_path)`: `mkdir(parents=True, exist_ok=True)`, writes `{run_id}.json` (`model_dump_json(indent=2)`) + `{run_id}.md`. **`eval/reports/` is gitignored** → tests write to `tmp_path`, never the real dir.
- **`config/costs.yaml`** (tracked, NOT gitignored): `ollama.default` → 0.0 (local); illustrative openai/anthropic rates. `EvalSettings.cost_rates_path`/`reports_dir` already existed (Task 006) — NOT wired into CLI/runner yet (no eval CLI subcommand in scope here).
- 5 tests in `tests/integration/evaluation/test_runner.py` (`asyncio_mode=auto` → plain `async def`): 2-case run (aggregates populated, `mean_recall==1.0`, p50==200.0 via interpolation, verifiable cost arithmetic), report JSON+MD written to `tmp_path` (sections + case_ids asserted), direct `estimate_cost` known-usage, missing-provider→0.0, `load_cost_table` round-trip vs shipped yaml (`Path(__file__).resolve().parents[3]/config/costs.yaml`; `ollama default`→0.0). `StubAgent` (canned Answer per question) + `StubJudge` (parses numbered traits from the prompt, marks all True).
- **Post-write fixes (apply step)**: (1) sub-agent placed `from code_atlas.domain.answer import TokenUsage` AFTER the `evaluation.*` imports in runner.py → I reordered (domain before evaluation) for isort. (2) `ruff format` wrapped 4 long `EvaluationError(...)` calls in metrics_cost.py. `ruff check --fix` confirmed `__all__` ASCII sort. Then gate clean: 48 source files, **317 tests pass**.
- **PHASE 8 (Evaluation) COMPLETE.** All metrics (retrieval/grounding/correctness/cost/latency) + runner + report writer shipped. Next: **Phase 9** — Task 031 (Docker + compose), then 032 (Makefile + docs). 031 deps 024✓ 025✓.

### Task 029 — Answer correctness (LLM-as-judge)
- New module `code_atlas.evaluation.metrics_correctness`. Re-exports `CorrectnessReport`, `judge_answer` from `evaluation/__init__` (`__all__` RUF022-sorted: `CorrectnessReport, EvalCase, GroundingReport, UngroundedCitation, check_grounding, judge_answer, load_dataset, mrr, ndcg_at_k, recall_at_k`).
- **`async judge_answer(answer: Answer, expected_traits: list[str], llm: LLMProvider) -> CorrectnessReport`** — ASYNC (first eval metric that calls an LLM; grounding/retrieval were sync/pure).
- **`CorrectnessReport`** (frozen pydantic, `ConfigDict(frozen=True)` — NO `extra="forbid"`, mirrors grounding reports): `score: float` (0..1), `per_trait: dict[str, bool]`, `rationale: str`.
- **Score is computed by US, not the LLM**: `score = sum(per_trait.values()) / len(expected_traits)`. The judge LLM only returns per-trait booleans + a rationale string. Keeps scoring deterministic.
- **Vacuous short-circuit**: empty `expected_traits` → `CorrectnessReport(score=1.0, per_trait={}, rationale="no traits to evaluate")` **without calling the LLM** (consistent with recall/grounding vacuous-pass convention).
- **Prompt**: module-level `_JUDGE_SYSTEM_PROMPT` (strict-evaluator, JSON-only, exact-trait-text keys) + `_build_messages(answer, traits)` (system + user embedding `answer.text` + numbered traits). `await llm.chat(messages)` with **NO tools**. Judges against `answer.text` only (not citations).
- **`_parse_judge(content, traits)`**: try `json.loads(content)`; on `JSONDecodeError` fall back to outermost-brace substring (`content.find("{")` .. `content.rfind("}")`) and retry — handles markdown-fenced/prose-wrapped JUDGE output (a real LLM path, not defensive bloat).
- **Malformed soft-fail** (parsed not a `dict`, OR `per_trait` missing/not a `dict`): `log.warning("correctness.judge_malformed", content=content[:200])` → `CorrectnessReport(score=0.0, per_trait={}, rationale="judge returned malformed output")`. Does NOT raise.
- **Trait alignment**: `per_trait = {t: _coerce_bool(raw.get(t, False)) for t in expected_traits}` — keys ALWAYS == `expected_traits` (missing trait → False; hallucinated extra keys dropped). `_coerce_bool`: real bool passthrough; str → `lower() in {"true","yes","1"}`; else `bool(v)` (LLMs emit stringified booleans).
- **Error policy**: `ProviderError` from `llm.chat` bubbles UNCAUGHT (matches hybrid/grounding bubble-as-is). Malformed judge output is the ONLY soft-fail. No `EvaluationError` raised here.
- **Imports**: `ChatMessage` + `get_logger` at runtime; `Answer` + `LLMProvider` under `TYPE_CHECKING` (annotation-only — `from __future__ import annotations` keeps `answer.text` access valid). Token/cost capture deferred to Task 030.
- 8 unit tests in `tests/unit/evaluation/test_metrics_correctness.py` (`asyncio_mode=auto` → plain `async def`, no decorator; local `FakeLLM(content)` with `.calls` recorder): all-true, partial (`approx(2/3)`), missing-trait→False, stringified-bool coercion, malformed non-JSON, malformed shape, markdown-fence extraction, empty-traits vacuous (asserts `fake.calls == []`). Gate clean first-try (no post-write fixes).
- **Phase 8 remaining: 030 only** (cost/runner/report — deps 027✓ 028✓ 029✓ now all met).

### Task 028 — Citation grounding (hallucination check)
- New module `code_atlas.evaluation.metrics_grounding`. Re-exports `check_grounding`, `GroundingReport`, `UngroundedCitation` from `evaluation/__init__` (`__all__` now multiline, RUF022-sorted).
- **`check_grounding(answer: Answer, metadata_store: MetadataStore, *, repo_id: str) -> GroundingReport`** — SYNC; calls `MetadataStore.find_by_path(repo_id, path)` directly (no threads → `:memory:` store fine in tests).
- **Report shapes** (both frozen pydantic):
  - `UngroundedCitation{citation: Citation, file_exists, line_range_valid, snippet_present: bool, reasons: list[str]}`.
  - `GroundingReport{total, grounded, ungrounded_citations: list[UngroundedCitation]}` + `@property is_fully_grounded` (`total == grounded`).
- **3 per-citation checks** (all must pass → counted grounded), each with a fixed failure-reason string:
  1. `file_exists` = `bool(find_by_path(...))` → fail `"file not indexed"`.
  2. `line_range_valid` = ∃ chunk fully covering the cited range (`chunk.start_line <= cite.start_line AND chunk.end_line >= cite.end_line`) → fail `"line range outside known chunks"`.
  3. `snippet_present` = snippet substring of a CONTAINING chunk's `content` → fail `"snippet not found in cited chunk"`.
- **Conventions**: empty snippet → `snippet_present=True` (vacuous — nothing fabricated). Empty `answer.citations` → `total=0, grounded=0, ungrounded=[]` → `is_fully_grounded` vacuously True. No `EvaluationError` paths (no invalid-arg case). No logger.
- **Import layering**: `Citation` imported at runtime (pydantic field type); `Answer`/`CodeChunk`/`MetadataStore` under `TYPE_CHECKING` so `evaluation` does NOT pull `sqlalchemy` at import time (`from __future__ import annotations` keeps the runtime `find_by_path` call valid). evaluation→indexing dependency is annotation-only.
- 7 unit tests in `tests/unit/evaluation/test_metrics_grounding.py` (real `:memory:` MetadataStore, fixture yields+closes): fully-grounded, fabricated-file, out-of-range-line, wrong-snippet, empty-snippet vacuous, empty-citations vacuous, mixed counts. Gate clean first-try.
- Phase 8 remaining: 029 (LLM-judge), 030 (cost/runner/report).

### Task 027 — Retrieval metrics (recall@k, MRR, nDCG)
- New module `code_atlas.evaluation.metrics_retrieval` — three PURE, stdlib-only functions (binary relevance: a file is relevant iff in the expected set). Re-exported from `evaluation/__init__` (`__all__` RUF022-sorted: `EvalCase, load_dataset, mrr, ndcg_at_k, recall_at_k`).
  - `recall_at_k(retrieved_files: list[str], expected_files: list[str], k: int) -> float`
  - `mrr(retrieved_files: list[str], expected_files: list[str]) -> float`
  - `ndcg_at_k(retrieved_files: list[str], expected_files: list[str], k: int) -> float`
- **Dedup semantics**: `retrieved_files` deduped via `_dedup = list(dict.fromkeys(items))` (preserves FIRST-occurrence rank — matters for mrr/ndcg). `expected_files` → `set`.
- **k-validation**: `recall_at_k` + `ndcg_at_k` require `k >= 1` else `EvaluationError("k must be >= 1", context={"k": k})`. `mrr` takes no k (full list).
- **Empty-expected conventions** (locked): `recall_at_k → 1.0` (vacuous — zero relevant, zero misses), `mrr → 0.0`, `ndcg_at_k → 0.0`.
- **nDCG**: DCG `= Σ 1/log2(i+1)` over relevant hits at 1-indexed rank i (rank 1 → `log2(2)=1`); IDCG ideal-ranked, capped at `min(k, len(expected))`. `mrr` is single-query reciprocal rank — the runner means across cases for true MRR.
- 23 tests in `tests/unit/evaluation/test_metrics_retrieval.py` (recall partial/full/zero/k-trunc/empty-exp(1.0)/empty-retr/dedup; mrr first/perfect/miss/empty/dedup-rank; ndcg perfect/partial(approx via math.log2)/miss/empty/k-trunc; `k<=0` raises for both `@k` metrics via parametrize). Gate clean first-try.
- Tasks 028 (grounding), 029 (LLM-judge), 030 (cost/runner/report) still pending in Phase 8.

### Task 026 — Eval dataset format + seed dataset — Phase 8 starts
- New pkg `code_atlas.evaluation` (re-exports `EvalCase`, `load_dataset`). Depends only on `errors` + `utils` (utils is a leaf — strict layering holds). Absolute imports in `__init__`.
- **`EvalCase`** (frozen pydantic, `extra="forbid"`): `case_id`, `repo_id`, `question` (all `min_length=1`, required); `expected_files`, `expected_symbols`, `expected_answer_traits` (`list[str]`, default `[]`).
- **`load_dataset(path: Path) -> list[EvalCase]`**: YAML top-level is a **mapping with a `cases:` list** (room for future dataset-level metadata). Returns cases in file order; `info` log `dataset.loaded` with count. `Path` imported under `TYPE_CHECKING` only (annotation-only; `from __future__ import annotations`).
- **Error contract** — all `EvaluationError` (`from exc` when wrapping a cause):
  - unreadable file (`OSError`) → `"dataset: cannot read file"` ctx `{path}`
  - bad YAML (`yaml.YAMLError`) → `"dataset: invalid YAML"` ctx `{path}`
  - top-level not a mapping / `cases` missing or non-list → `"dataset: 'cases' must be a list"` ctx `{path}`
  - per-case `ValidationError` (incl. non-mapping list items — pydantic rejects them) → `"dataset: invalid case"` ctx `{index, error}`
  - duplicate `case_id` → `"dataset: duplicate case_id"` ctx `{case_id}`
- **Seed**: `eval/datasets/seed.yaml` — **10 cases**, all `repo_id: code-atlas`, targeting THIS repo. File paths + symbols vetted against the current tree (hybrid.py/`HybridRetriever`+`_rrf_fuse`, providers/base.py/`EmbeddingProvider`, agent/tools.py/`Toolbox`+4 tools, agent/qa.py+domain/answer.py/`QAAgent`+`Answer`, symbol_graph.py, metadata_store.py, cli.py/`init`+`ingest`+`ask`, api/routes.py/`health`+`ask`+`ingest`+`ask_stream`, ingestion/parser.py/`chunk_file`). Load via cwd-independent `Path(__file__).resolve().parents[3] / "eval" / "datasets" / "seed.yaml"`.
- 6 unit tests in `tests/unit/evaluation/test_datasets.py` (new `tests/unit/evaluation/` dir, no `__init__.py`): seed parses (≥6 cases, all `EvalCase`, repo_id all `code-atlas`, unique ids), required-field validation, duplicate id (asserts `"case_id" in exc.value.context`), missing `cases` key, missing file, optional-list defaults.
- Quality gate clean first-try after apply (no post-write fixes). Changed `__init__` to absolute import (vs sub-agent's relative) per codebase convention; used sub-agent's cleaned test file (dropped a needless TYPE_CHECKING alias it flagged itself).
- **Phase 8 (Evaluation) seeded.** Tasks 027 (retrieval metrics), 028 (citation grounding), 029 (LLM-judge) consume `EvalCase`. 027/028 unblock next (029 needs 019 ✓).

### Task 025 — FastAPI app (/health, /ingest, /ask, SSE stream) — Phase 7 complete
- New `code_atlas.api` package: `app.py`, `routes.py`, `models.py`, `__init__.py`. Uvicorn entrypoint **`code_atlas.api.app:app`** (module-level `app = create_app()`).
- New runtime deps: `fastapi>=0.110,<1.0` (installed starlette 1.3.1), `uvicorn[standard]>=0.30,<1.0` (installed 0.49.0 + uvloop/httptools/watchfiles/websockets).
- **Endpoints**: `GET /health` → `HealthResponse{status,version}` (no state); `POST /ask` (`AskRequest`) → domain `Answer` as `response_model`; `POST /ingest` (`IngestRequest`, status 202) → `IngestResponse{job_id=uuid4().hex, status="accepted"}` via `BackgroundTasks`; `GET /ask/stream?repo_id&question` → `StreamingResponse` (`text/event-stream`).
- **FastAPI route params use `Annotated[T, Depends(...)]`** — NOT `x: T = Depends(...)`. Same ruff **B008** ("function call in argument defaults") as typer.Option; `Annotated` avoids it with no ruff-config change. **Carry-forward**: all `Depends`/`Query`/`typer.Option` go in `Annotated`.
- **DI / offline testing**: two dependency providers in `routes.py` are the ONLY request-time touch points for `app.state`/real I/O — `get_agent_factory(request)` (builds per-request `QAAgent`+`Toolbox` from `app.state`) and `get_ingest_runner(request)` (closure running a self-contained CLI-mirroring index job). Tests override BOTH via `app.dependency_overrides` and construct **`TestClient(app)` WITHOUT `with`** so the lifespan never opens real stores. Providers live in `routes.py` (not `app.py`) to avoid an app↔routes import cycle (app→routes only; routes read `request.app.state` at request time). **Carry-forward**: `TestClient(app)` (no context-manager) skips lifespan — the offline-test lever.
- **SSE replay**: `QAAgent` has NO streaming method, so `_event_stream` computes the full `Answer` then replays it — whitespace-split `data: <token>\n\n` events (`split() or [text]` guarantees ≥1) + terminal `event: done\ndata: <answer.model_dump_json()>\n\n`. True incremental streaming deferred (needs a QAAgent path that streams only the final post-tool turn).
- **Lifespan** (`app.py`): opens 4 stores + embedder + llm + `HybridRetriever`, loads `SymbolGraph` from disk if present else fresh, stashes all on `app.state`; shutdown `aclose()`s providers (getattr guard) + `.close()`s the 3 closeable stores. Same store-path derivation as CLI (`_StorePaths`).
- **Deferred**: shared CLI↔API composition root — lifecycles differ (CLI one-shot sync vs API long-lived async-native); extraction premature. **v1 limitation**: in-memory `app.state.graph` goes stale after a background ingest writes a fresh graph to disk until app restart (real-time index updates out of scope per ARCHITECTURE).
- `ingest` runner runs in a threadpool background-task thread (no event loop) → owns its own embedder + 4 stores + ONE persistent `asyncio.new_event_loop()` for the sync embed shim (same pattern as CLI; NOT `asyncio.run` per batch).
- 5 offline tests in `tests/unit/api/test_app.py` (new `tests/unit/api/` dir, no `__init__.py`): health (200 + version), ask (200 + canned Answer text + citations), ingest (202 + job_id + background runner spy called `[("/tmp/x","r")]` — TestClient runs background tasks after response), SSE (200 + `text/event-stream` + `data:` + `event: done`), ask validation (empty question → 422). Pytest fixture clears `app.dependency_overrides` in teardown (no cross-test leak).
- Post-write fixes: (1) converted `Depends(...)` defaults → `Annotated` (B008). (2) ruff format reflowed the `Citation(...)` ctor and a test override lambda. Gate clean (ruff/mypy 40 files/268 pytest).
- **Phase 7 (CLI + API) COMPLETE.** Both HTTP + CLI surfaces wrap `QAAgent.ask`. Phase 8 (Evaluation) unblocks — Task 026 (eval dataset) deps only on 007 (done).

### Task 024 — CLI (typer): init / ingest / ask — Phase 7 starts
- New runtime deps: `typer>=0.12,<1.0` (installed **0.26.7** — note: this typer does NOT depend on `click`; ships its own `rich`-based `CliRunner` with NO `isolated_filesystem()`), `rich>=13.0,<15.0` (installed 14.x). Pulls `annotated-doc`, `shellingham`, `markdown-it-py`, `mdurl`.
- New module `code_atlas.cli` — **the composition root**. `app = typer.Typer(name="code-atlas", no_args_is_help=True, add_completion=False)`. Console script `code-atlas = code_atlas.cli:app` (was stubbed since Task 001) now resolves. Two module-level `rich.Console`s (stdout + stderr).
- **Typer params use the `Annotated[T, typer.Option(...)]` style** — NOT `x: T = typer.Option(...)`. Ruff **B008** ("function call in argument defaults") fires on the bare-default form; `Annotated` is the modern idiom and avoids the lint with no suppression. Required options/args = Annotated with NO default; `--force` = `Annotated[bool, typer.Option("--force","-f")] = False`.
- Three commands:
  - **`init`** (`--force`/`-f`): writes `config/default.yaml` to cwd. Default config built from the PLAIN nested models (`AppSettings()`, `StorageSettings()`, … `.model_dump(mode="json")` → `Path`→str) — NOT `Settings()` (which would read env/.env and leak overrides). `yaml.safe_dump(..., sort_keys=False)`. Refuses + `typer.Exit(1)` if file exists without `--force`.
  - **`ingest --repo <path> --id <repo_id>`**: builds embedder + 4 stores + `Indexer`, `index_repo`, `graph.save()`, rich summary (seen/indexed/cached/edges).
  - **`ask "<question>" --repo-id <id>`** (question is positional Argument): builds embedder+llm+4 stores+HybridRetriever+Toolbox+QAAgent. SymbolGraph loaded from disk if `<root>/symbol_graph.json.gz` exists else fresh. Renders answer text + citations as `path:start-end (symbol)` + dim `latency_ms · total tokens` footer.
- **ASYNC→SYNC SHIM (architecture rule)**: `ingest` creates ONE persistent `asyncio.new_event_loop()` and a sync `_embed(texts)` = `loop.run_until_complete(embedder.embed(texts))` passed to the sync `Indexer`. Do NOT use `asyncio.run` per batch — a fresh loop per call breaks the embedder's reused httpx client ("loop is closed"/"attached to different loop"). `aclose()`+`loop.close()`+store closes in `finally`.
- **`ask` uses ONE `asyncio.run(_run())`** wrapping retrieval+chat in a single loop; providers `aclose()` inside the coroutine's `finally` (getattr guard — `aclose` is not on the Protocol); stores closed in the outer `finally`.
- **Providers imported from the `code_atlas.providers` PACKAGE** (not `.registry`) so import side-effects auto-register Ollama; `make_*` otherwise raise `ProviderError: not registered`.
- **Nested settings groups imported from `code_atlas.config.settings`** (the submodule), NOT `code_atlas.config` — the package `__init__` only re-exports `Settings` + `load_settings` (the nested `*Settings` groups are public in `settings.py.__all__` but not re-exported). **Carry-forward**: import nested groups from `.config.settings`.
- `_StorePaths(settings)` helper derives all four locations from `settings.storage` (metadata `sqlite:///<resolved sqlite_path>`, lexical `<root>/lexical.sqlite`, vector `lance_uri`, graph `<root>/symbol_graph.json.gz`). **Did NOT modify `settings.py` / `default.yaml`** (out of task scope).
- Error handling: `CodeAtlasError` in ingest/ask → red `error:` line + `typer.Exit(1)`; `typer.Exit` propagates untouched.
- Provider auto-register logs fire at IMPORT time (module-level side effect) BEFORE any `configure_logging` — so `code-atlas --help` emits two console-style structlog lines to stderr (default structlog config). Harmless.
- 7 unit tests in `tests/unit/test_cli.py`, fully OFFLINE (only `--help` + `init`; ingest/ask execution needs live Ollama). Uses `tmp_path` + `monkeypatch.chdir(tmp_path)` (NOT `runner.isolated_filesystem()` — absent on this typer's CliRunner). `result.exit_code` / `result.output` work.
- Post-write fixes: (1) sub-agent imported nested settings groups from `code_atlas.config` → would `ImportError`; switched to `code_atlas.config.settings`. (2) B008 on bare `typer.Option(...)` defaults → converted to `Annotated`. (3) `isolated_filesystem()` AttributeError on this typer's CliRunner → `tmp_path`+`monkeypatch.chdir`. (4) trimmed redundant `_store_paths` wrapper; tightened `_run() -> Answer`; cleaned RST backticks from `init` help docstring.
- **Phase 7 (CLI) half done.** Task 025 (FastAPI app) reuses the SAME composition wiring (consider extracting a shared builder if duplication bites; deferred for now).

### Task 023 — Prompts + QA agent — Phase 6 complete
- New module `code_atlas.agent.prompts`: `SYSTEM_PROMPT` (3 hard rules — cite `path:start-end`, never invent paths/symbols/lines, decline when context+tools insufficient), `DECLINE_MESSAGE` fallback string, `format_context(results)` (per-chunk header `[path:start-end] symbol=...` + content, blank-line separated; empty list → short notice), `format_user_prompt(question, results)` (question + `Context:` section).
- New module `code_atlas.agent.qa`: `QAAgent(*, retriever, llm, toolbox, max_tool_iters=4, retrieval_k=8)`. Kw-only (matches sibling ctors). `max_tool_iters < 1` or `retrieval_k < 1` → `AgentError`.
- **`async ask(question) -> Answer`** pipeline:
  1. Empty/whitespace question → `AgentError("qa: question is required")`.
  2. `time.perf_counter()` timer.
  3. `retriever.retrieve(RetrievalQuery(text=question, k=retrieval_k))` — **NO repo_id filter** (v1 = one indexed repo per agent instance; stores hold a single repo; `tools.py` out of scope so no public repo_id accessor to read).
  4. Seed messages `[system=SYSTEM_PROMPT, user=format_user_prompt(...)]`.
  5. **Bounded tool-use loop** `for iteration in range(max_tool_iters + 1)`: call `llm.chat(messages, tools=toolbox.specs)`; accumulate `usage.prompt`/`usage.completion`; append `{"step":"llm","iter",...}` to trace; break if no `tool_calls`; break (without executing) if `iteration == max_tool_iters` (cap); else append `assistant` turn + execute each tool, append `tool`-role message (`content=json.dumps(result)`, `tool_call_id`, `name`). Total LLM calls ≤ `max_tool_iters + 1`.
  6. `text = response.content.strip() or DECLINE_MESSAGE` (Answer.text is min_length=1 → never empty).
  7. Citations = `to_citation(r.chunk)` per retrieved result, **deduped by `(path, start_line, end_line)`** first-seen order — citations come from retrieved chunks, NOT parsed from answer text (guarantees grounding).
  8. `latency_ms`, `TokenUsage(prompt, completion)` (total auto-fills), trace returned on `Answer`.
- **`_run_tool(tc, trace)`**: traces `{"step":"tool","name","arguments"}`, then `toolbox.call(...)`; **swallows `AgentError` into `{"error": str(exc)}`** so tool failures feed back to the LLM rather than aborting the loop.
- Assistant tool-call turn appended as `ChatMessage(role="assistant", content=response.content)` (may be empty) before tool-result messages — `ChatMessage` has no `tool_calls` field and Ollama provider doesn't serialize assistant tool_calls; acceptable v1, tool-result messages carry the info.
- `agent/__init__.py` now exports `QAAgent` (`__all__ = ["QAAgent", "ToolResult", "Toolbox"]`).
- **Decisions (locked)**: no reranker in QAAgent (constructor locked; deferred); `assert response is not None` for mypy narrowing (loop runs ≥1×).
- 4 integration tests in `tests/integration/agent/test_qa.py` (real `HybridRetriever` + file-backed `MetadataStore` + real `SymbolGraph` + real `Toolbox`; fake embedder/vector/lexical/LLM): grounded answer + token total (10+5+12+8=35) + 2 LLM calls, tool executed & fed back as `role="tool"` message + trace entry, decline on blank content, empty-question raises. File-backed SQLite (not `:memory:`) per the `asyncio.to_thread` gotcha. No `__init__.py` under `tests/`.
- Post-write fix: ruff format collapsed one multi-line `messages.append(...)` onto a single line (≤120). Gate then clean (ruff/mypy/256 pytest).
- **Phase 6 (Agent) complete.** Retrieval+agent surface is end-to-end: `QAAgent.ask(question) -> Answer{text, citations, trace, latency_ms, token_usage}`. Phase 7 (CLI + API) unblocks next — Task 024 (CLI) deps (016, 023) now both done.

### Task 022 — Agent tools — Phase 6 starts
- New pkg `code_atlas.agent` with `Toolbox` + `ToolResult` type alias.
- **`MetadataStore.find_by_path(repo_id, path) -> list[CodeChunk]`** added — SELECT WHERE repo_id+path, ORDER BY start_line ASC. Backs `open_file`.
- **`SymbolGraph.find_by_name(name, kind=None) -> list[Symbol]`** added — iterates `_g.nodes(data=True)`, filters by `symbol_data["name"]` (+ optional kind). Sorted by `(path, line)`. Backs `find_symbol` and the `list_callers`/`list_callees` aggregation.
- `Toolbox(*, metadata_store, symbol_graph, repo_id)`. `repo_id` is bound at construction — **the LLM never sees or names it.** Empty `repo_id` → `AgentError`.
- Four LLM-callable tools, all SYNC, all return deterministic typed dicts:
  - `open_file(*, path, start_line, end_line) -> {"path", "start_line", "end_line", "chunks": [{chunk_id, start_line, end_line, kind, symbol, content}]}`. Returns chunks whose `[start_line, end_line]` OVERLAPS the requested range. Empty `path` or invalid range → `AgentError`. No matches → `chunks: []`.
  - `find_symbol(*, name, kind=None) -> {"name", "results": [{name, kind, path, line, parent}]}`. Unknown symbol → `results: []`, no error.
  - `list_callers(*, symbol_name) -> {"symbol", "results": [...]}`. Aggregates over ALL symbols matching `symbol_name` (any kind, any path), dedupes by `(path, name)`, sorts.
  - `list_callees(*, symbol_name) -> ...` — same shape, opposite direction.
- `Toolbox.specs` — list of 4 `ToolSpec` records (reuses `code_atlas.providers.base.ToolSpec`) with JSON-schema-style `parameters` dicts (`type: object`, properties, required, `additionalProperties: false`). Schemas do NOT include `repo_id` — bound at toolbox level.
- `Toolbox.callables` — `{name: bound method}` dict. `Toolbox.call(name, arguments) -> dict` dispatches by name; unknown name → `AgentError("toolbox: unknown tool", context={"name", "available": sorted(...)})`; arg-shape mismatch (`TypeError` from `**arguments`) → `AgentError("toolbox: invalid arguments", context={"tool", "error"})`.
- **Tool semantics**: tools NEVER raise on "not found" — they return empty results. They only raise `AgentError` on invalid arguments. Store errors (`IndexingError`) propagate untouched.
- **Symbol-graph node-key gotcha** (re-confirmed in this task): nodes are keyed by `(path, name)` — two symbols with the same name AT THE SAME PATH overwrite each other. Tests for `find_by_name` must use distinct paths per name. Affects future call-graph extractors too.
- 15 tests in `tests/unit/agent/test_tools.py` + 2 new tests in `tests/unit/indexing/test_metadata_store.py` + 2 in `tests/unit/indexing/test_symbol_graph.py`. Tests use real file-backed `MetadataStore` (per the SQLite + asyncio.to_thread gotcha — though tools are sync here, the convention sticks) and real `SymbolGraph`.
- Post-write fixes:
  1. **Ruff format**: 2 files needed reformat (tools.py, test_tools.py — minor line-wrap differences). Auto-fixed.
  2. **Ruff lint**: 1 import-order issue in `agent/__init__.py` (`ToolResult, Toolbox` → `Toolbox, ToolResult` per isort). Auto-fixed.
  3. **Test bug**: sub-agent's `test_find_by_name_returns_matches_sorted` added two `foo` symbols at the same path (`a/first.py`) with different lines — they collided on the `(path, name)` node key. Fixed by spreading across distinct paths.
- Phase 5 (Retrieval) complete; Phase 6 (Agent) seeded. Task 023 (prompts + qa orchestrator) will consume `Toolbox.specs` + `Toolbox.call`.

### Task 021 — Citation hydration + Reranker Protocol — Phase 5 complete
- New module `code_atlas.retrieval.citation`. Exports `to_citation(chunk, *, max_snippet_chars=800) -> Citation` and `DEFAULT_SNIPPET_MAX_CHARS = 800`.
- New module `code_atlas.retrieval.reranker`. Exports `Reranker` Protocol (async `rerank(query, results) -> list[RetrievalResult]`) and `PassthroughReranker` (no-op default impl, returns shallow copy).
- `to_citation`: snippet is a **raw prefix** of `chunk.content` (NO trailing ellipsis) so downstream tools can re-grep against the source file exactly. `max_snippet_chars < 0` → `ValueError`. `0` allowed (yields empty snippet). Pydantic `Citation` invariants (line-range, max_length=4096) propagate as `ValidationError`.
- `PassthroughReranker.rerank` returns `list(results)` (shallow copy, not the same reference) so callers can safely mutate without affecting the input. Uses `del query` to silence unused-arg lint.
- `Reranker` is a `Protocol` — structural subtyping. Future cross-encoder, LLM-judge, or BGE-based rerankers just need to satisfy the signature; no inheritance required.
- 10 unit tests across two files: 6 for citation (basic field projection, None symbol, truncation, full-content-under-limit, zero-chars empty, negative raises), 4 for reranker (preserves order, returns copy not same list, empty input, query ignored).
- Quality gate clean first-try (no post-write fixes).
- **Phase 5 (Retrieval) complete.** The retrieval surface is now `HybridRetriever.retrieve(query) -> list[RetrievalResult]` → optional `Reranker.rerank(query, results)` → `to_citation(result.chunk)` for `Answer.citations`. Phase 6 (Agent) unblocks next.

### Task 020 — Hybrid retrieval (RRF) — Phase 5 starts
- New pkg `code_atlas.retrieval`. Re-exports `HybridRetriever` + `RRF_K_DEFAULT` (=60).
- `HybridRetriever(*, vector_store, lexical_store, embedder, metadata_store, rrf_k=60, oversample=2)`. All store deps are SYNC; embedder is ASYNC. Sync calls wrapped in `asyncio.to_thread` from inside the async `retrieve`.
- `async retrieve(query: RetrievalQuery) -> list[RetrievalResult]`. Pipeline:
  1. Embed `[query.text]` (async).
  2. `asyncio.gather` the vector-search and lexical-search paths. Stores are called with `k = query.k * oversample` (default 2x) for a wider fusion pool.
  3. **RRF fusion** (k=60): score per chunk_id = `Σ 1 / (rrf_k + rank + 1)` across the two ranked lists. Rank is 0-indexed in code, +1 normalization for the 1-indexed RRF convention. Underlying scores are **discarded** — only rank position counts.
  4. Take top `query.k` (chunk_id, score) pairs, hydrate via `metadata_store.get_many`, emit `RetrievalResult(chunk=..., score=fused_score, source="fused")`.
- **Concurrency**: `_vector_path` chains `embed → vector.search` sequentially, but `asyncio.gather(_vector_path(), _lexical_path())` runs lexical concurrently with that chain. Tested at the surface (gather invocation, embedder call count) — not wall-clock.
- **Filter contract v1**: ONLY `repo_id` (str) is honored. Unknown keys (`language`, `kind`, etc.) silently ignored with a `hybrid.unknown_filters` debug log — forward-friendly for future expansion. Non-str `repo_id` → `RetrievalError(context={"field": "repo_id", "got_type": ...})`.
- Hydration drops chunk_ids missing from metadata store (with `hybrid.missing_metadata` warning log). Rank order preserved.
- **Error propagation policy**: embedder failures (`ProviderError`) and store failures (`IndexingError`) bubble AS-IS — callers can distinguish failure source. Only filter-validation issues raise `RetrievalError` directly. Empty fused list returns `[]` (no error).
- Ctor validation: `rrf_k < 1` or `oversample < 1` → `RetrievalError`.
- 12 unit tests in `tests/unit/retrieval/test_hybrid.py` using `FakeVectorStore`/`FakeLexicalStore`/`FakeEmbedder` + real `MetadataStore`. Coverage: deterministic RRF order (vec A>B>C, lex B>D>A → B>A>D>C), repo_id passthrough to both stores, no-filter → None to both, unknown filter keys ignored, bad repo_id type raises, top-k limit, dedup by chunk_id with summed score, hydration skip on missing, `source="fused"`, oversample multiplies k, embedder called once with `[query.text]`, both-empty returns `[]`.
- **Test gotcha discovered**: `MetadataStore("sqlite:///:memory:")` is NOT shareable across threads. `asyncio.to_thread` schedules `get_many` on a worker, and SQLite `:memory:` is per-connection, so the worker sees an empty DB ("no such table: chunks"). **Fix**: use `sqlite:///{tmp_path}/meta.sqlite` (file-backed) in tests that exercise async-wrapped store calls. Alternative would be `poolclass=StaticPool + check_same_thread=False`, but file-based is simpler. **Carry forward**: any future test that calls a SQLAlchemy store via `asyncio.to_thread` must use a file URL, NOT `:memory:`.
- Two post-write fixes:
  1. **RUF022**: ruff isort wants `["RRF_K_DEFAULT", "HybridRetriever"]` (R before H — appears to use a case-sensitive natural sort where all-caps tokens sort before mixed-case). Auto-fixed.
  2. **Test fixture URL**: switched `:memory:` → file-backed (see above).
- Did NOT ship `tests/unit/retrieval/__init__.py` despite sub-agent including it — project convention is no `__init__.py` in tests/ (pytest discovers via `pythonpath` + `rootdir`).

### Task 019 — Ollama LLM provider — Phase 4 complete
- New module `code_atlas.providers.ollama_llm` (re-exports `OllamaLLMProvider` from `code_atlas.providers`). Importing triggers `register_llm("ollama", _factory)` side-effect at module bottom.
- `OllamaLLMProvider(*, base_url, model, temperature=0.2, max_tokens=2048, timeout_s=60.0, client=None)`. Implements `LLMProvider` Protocol structurally.
- Endpoint: `POST {base_url}/api/chat`. Same `client` ownership pattern as Task 018 (`_owns_client` flag; injected clients survive `aclose()`).
- **Request payload (`_build_payload`)**:
  - `messages` → `[{"role", "content"}]`; `name`/`tool_call_id` included only when not None.
  - `tools` (if non-empty) → `[{"type": "function", "function": {"name", "description", "parameters"}}]`. Omitted entirely when no tools passed.
  - `options: {"temperature": ..., "num_predict": max_tokens}` always present.
  - `stream: true|false` set per method.
- **Non-streamed response → `ChatResponse`**:
  - `content` = `payload["message"]["content"]` (defensive `or ""` for None).
  - `tool_calls`: list comprehension over `message.tool_calls`, IDs synthesized as `f"call_{idx}"` (Ollama doesn't emit IDs). `arguments=dict(fn.get("arguments") or {})`.
  - `usage` = `TokenUsage(prompt=prompt_eval_count, completion=eval_count)`; total auto-fills.
  - `finish_reason` = `done_reason` (may be None on older Ollama).
- **Streaming via `httpx.AsyncClient.stream("POST", ...)`** as async context manager. Iterate `resp.aiter_lines()`, skip blank lines, JSON-decode each. Helper `_emit_line(chunk_data) -> Iterator[ChatChunk]` handles all four streaming cases: content-only intermediate, tool-call-only intermediate, content+tools combo, and final `done: true` line (folds last tool call into terminal chunk, emits preceding tool calls as their own chunks first).
- Async-generator `chat_stream` uses outer `try/except` wrapping the `async with stream(...)` block — captures both pre-iteration `raise_for_status()` failures AND mid-stream `RequestError` disconnects. Wraps to `ProviderError` consistently.
- Validation in `__init__`: empty `model`, `temperature` outside `[0, 2]`, `max_tokens < 1` → `ProviderError`. Empty `messages` in `chat`/`chat_stream` → `ProviderError`.
- Error wrapping mirrors Task 018: `HTTPStatusError` → `{status_code, url, body[:200]}`, `RequestError` → `{error_type, url}`, JSON decode → `{status_code}` for non-stream, `{line}` for stream, malformed payload (missing `message` key) → `{keys}`.
- 14 unit tests in `tests/unit/providers/test_ollama_llm.py`: plain chat content+usage+finish_reason+model passthrough, tool-call extraction (`call_0` id), payload tools-list shape (present when given, absent when not), options/temperature/max_tokens passthrough, multi-turn message serialization (system/user/tool roles incl. tool_call_id + name), HTTP error wrap, network error wrap, malformed response, streaming chunks (2 deltas + 1 done), streaming tool_call_delta (final chunk), streaming HTTP error, empty messages raises, invalid ctor args, auto-registration.
- Quality gate clean first try (no post-write fixes).
- **Phase 4 (Providers) complete.** Both Ollama providers (embeddings + LLM) wired; registry has `make_embedding(settings)` + `make_llm(settings)` resolving the default `"ollama"` provider. Phase 5 (Retrieval) unblocks next.

### Task 018 — Ollama embedding provider
- New runtime dep: `httpx>=0.27,<1.0` (installed 0.28.1 + anyio/h11/httpcore/certifi/idna).
- New module `code_atlas.providers.ollama_embeddings`. Re-exported as `OllamaEmbeddingProvider` from `code_atlas.providers`. **Importing the module triggers `register_embedding("ollama", _factory)` at module bottom** — side-effect registration. `providers/__init__.py` imports the class, which is enough to run the side-effect.
- `OllamaEmbeddingProvider(*, base_url, model, dimension, timeout_s=60.0, concurrency=4, client=None)`. Implements `EmbeddingProvider` Protocol structurally (`model`, `dimension`, `async embed`).
- Endpoint: `POST {base_url}/api/embeddings` with body `{"model": ..., "prompt": <single text>}`. Single-input API → batches via `asyncio.gather` with a **per-call `asyncio.Semaphore(concurrency)`** (per-call avoids event-loop binding bugs). Response: `{"embedding": [...]}`.
- Score / dim validation: returned vector len must equal `self.dimension`; mismatch → `ProviderError` with `{"expected", "got", "model"}`. Validates on every response (not just first).
- Client ownership tracked by `_owns_client: bool`. `client=None` → builds `httpx.AsyncClient(base_url=base_url, timeout=timeout_s)`, owns it (closed by `aclose()`). Injected client → not closed by `aclose()` (caller-owned, dependency-injection pattern for tests).
- Error wrapping: `HTTPStatusError` (4xx/5xx) → `ProviderError` with `{status_code, url, body[:200]}`. `RequestError` (connect/timeout/etc.) → `ProviderError` with `{error_type, url}`. JSON decode error → `ProviderError` with `{status_code}`. Non-dict payload or missing `embedding` key → `ProviderError` with `{keys}` or `{type}`. All wrap via `from exc`.
- `dimension < 1` or `concurrency < 1` in `__init__` → `ProviderError`. Empty `texts` → `[]`, no HTTP calls.
- `_factory(settings)` reads `settings.ollama.{base_url, timeout_s, concurrency}` + `settings.embeddings.{model, dimension}`. Note: cross-group composition (embedding model lives under `embeddings`, transport lives under `ollama`).
- Logging: `info` on `ollama_embeddings.batch` (once per `embed()`), `debug` on `.request` (per text — debug to avoid log floods), `warning` on `.failed`.
- 10 unit tests in `tests/unit/providers/test_ollama_embeddings.py` using `httpx.MockTransport(async_handler)`. Coverage: round-trip 2 inputs, empty batch → 0 calls, HTTP error wrap, network error wrap (`httpx.ConnectError`), malformed response, dim mismatch, concurrency cap (5 inputs / concurrency=2 → max-in-flight ≤ 2), auto-registration, invalid ctor args, injected-client not closed by `aclose()`.
- Test hygiene: NO `clear_registry` autouse in this file (would defeat the auto-registration test). The `test_auto_registration` test re-registers explicitly via the imported `_factory` reference, which is robust to test ordering.
- Quality gate clean first-try — no post-write fixes.

### Task 017 — Provider Protocols + registry — Phase 4 starts
- New pkg `code_atlas.providers` (depends on domain/errors/utils/config only — strict layering).
- `base.py` ships two Protocols and five frozen pydantic records:
  - `EmbeddingProvider` Protocol: `model: str`, `dimension: int`, `async embed(texts: list[str]) -> list[list[float]]`.
  - `LLMProvider` Protocol: `model: str`, `async chat(messages, *, tools=None) -> ChatResponse`, `chat_stream(messages, *, tools=None) -> AsyncIterator[ChatChunk]`.
  - `ChatRole = Literal["system", "user", "assistant", "tool"]`.
  - `ChatMessage{role, content, tool_call_id?, name?}` — `content` may be empty (for assistant turns that are tool-only).
  - `ToolSpec{name, description, parameters: dict}` — `parameters` is free-form JSON schema, NOT validated by us.
  - `ToolCall{id, name, arguments: dict}`.
  - `ChatResponse{content, tool_calls, usage: TokenUsage, model, finish_reason}`.
  - `ChatChunk{content_delta, tool_call_delta?, done, usage?, finish_reason?}` — final chunk has `done=True`; `usage` populated on final chunk only (consumer-checked, not model-enforced).
- **`chat_stream` Protocol signature is `def ... -> AsyncIterator[ChatChunk]` (NOT `async def`).** Concrete impls use `async def chat_stream(...) -> AsyncIterator[ChatChunk]` with `yield` (async-generator function). When called, this returns an `AsyncIterator` synchronously — no `await` on the call site, only on the `async for`. This is the standard Protocol+async-gen idiom; both signatures are compatible.
- `registry.py` — two module-level dicts `_EMBEDDINGS`, `_LLMS`. Public API: `register_embedding`, `register_llm`, `make_embedding`, `make_llm`, `clear_registry`, `registered_embeddings`, `registered_llms`. Aliases: `EmbeddingFactory = Callable[[Settings], EmbeddingProvider]`, `LLMFactory = Callable[[Settings], LLMProvider]`.
- Registry semantics:
  - Empty/whitespace name on register → `ProviderError`.
  - Re-register same name **silently overwrites** (so concrete providers can re-register on import without failing on test re-imports).
  - `make_embedding(settings)` reads `settings.embeddings.provider`; `make_llm(settings)` reads `settings.chat.provider`. Unknown name → `ProviderError("... not registered", context={"name": ..., "available": sorted(...)})`.
  - Factory exception → wrapped as `ProviderError(..., context={"name": name}) from exc` (original in `__cause__`).
  - Info log on register (`provider.embedding_registered`/`provider.llm_registered`), warn log on factory fail.
- **Registry ships empty.** Concrete providers will auto-register on import (Task 018: ollama embeddings, Task 019: ollama LLM).
- 10 unit tests in `tests/unit/providers/test_registry.py`. Uses `autouse` `_reset_registry` fixture that calls `clear_registry()` before AND after each test (state never leaks).
- Test fakes (`_FakeEmbedder`, `_FakeLLM`) are minimal Protocol-conformant classes inside the test module — duck-typed against the Protocols (no explicit `class ... (EmbeddingProvider):` inheritance needed since Protocols are structural).
- Phase 3 (Indexing) complete; Phase 4 (Providers) seeded.

### Task 016 — Indexer orchestrator — Phase 3 complete
- New module `code_atlas.indexing.indexer` (re-exports `Indexer`, `IndexResult`, `EmbedFunc`).
- New helper `code_atlas.indexing.edge_extractor.extract_python_edges(chunks) -> list[(Symbol, Symbol, EdgeKind)]`. Derives `defines` (module → top-level class/function) + `contained_in` (class → method, line-range nesting) from CodeChunk metadata alone — NO tree-sitter re-parse. Module Symbol synthesized as `Symbol(name=path.stem, kind="module", path=path, line=1)`.
- `EmbedFunc = Callable[[list[str]], list[list[float]]]` — **sync** boundary. Async embedders adapt via shim at caller boundary (Task 020 territory). Decision: indexer stays sync because all four stores are sync; wrapping every DB call in `asyncio.to_thread` would be over-engineering.
- `Indexer(*, metadata_store, lexical_store, vector_store, symbol_graph, embed, batch_size=64)`. Stores are caller-owned — `Indexer` NEVER closes them.
- `index_repo(root, repo_id, *, extra_ignores=None, max_chunk_lines=200, mtime_cache=None) -> IndexResult`.
- `IndexResult` (slotted dataclass): `chunks_seen, chunks_indexed, chunks_skipped_cached, embed_batches, embed_calls, edges_added, ingest_stats`.
- **Idempotency strategy (critical)**: per-batch, `metadata_store.get_many([c.chunk_id for c in batch])` → `{chunk_id: content_hash}`. Partition into `cached` (matching hash → skip embed + skip writes) vs `to_index` (new or hash-changed). Only `to_index` chunks pass to embedder + store upserts. Re-running `index_repo` on unchanged tree → `embed_calls == 0`, `embed_batches == 0`.
- **Batching**: stream from `ingest_repo`, accumulate `batch: list[CodeChunk]` to `batch_size`, flush. Final partial batch flushes at end. Embedder called once per non-empty `to_index` partition.
- **Symbol-graph build is per-file** (not per-batch), executed AFTER the chunk stream is exhausted. Maintains `per_file: dict[str, list[CodeChunk]]` for whole-run accumulation. **Memory note**: buffers all chunks in memory; acceptable v1, will need streaming on very large repos (deferred).
- Vector items get `metadata={"path": ..., "language": ..., "kind": ...}` (JSON-encoded by LanceVectorStore on persist).
- Error handling: embedder exception → `IndexingError("indexer: embed failed", context={"batch_size": N})`. Vector count mismatch → `IndexingError(..., {"expected": N, "got": M})`. Vector dim mismatch → `IndexingError(..., {"expected": dim, "got": got, "index": idx})`. Stores' own IndexingErrors propagate untouched.
- Info-level milestones: `indexer.index_repo.start`, `indexer.batch.flushed`, `indexer.symbol_graph.built`, `indexer.index_repo.completed`.
- `batch_size < 1` → `IndexingError` in `__init__`.
- 7 integration tests in `tests/integration/indexing/test_indexer.py`: 4-store fan-out, idempotency (re-run = 0 embed calls), re-embed on content change (only changed file's chunks), batching (small batch_size → ceil(n/2) batches), Python edges extracted (defines + contained_in for Greeter→greet), embed dim mismatch raises, embed count mismatch raises.
- Phase 3 (Indexing) is now complete. Phase 4 (Providers) unblocks next.

### Task 015 — Symbol graph (NetworkX MultiDiGraph)
- New runtime dep: `networkx>=3.2,<4.0` (installed: 3.6.1, ~2MB).
- New module `code_atlas.indexing.symbol_graph`. Re-exports `SymbolGraph` + `EdgeKind` (Literal) from `code_atlas.indexing`.
- Backed by `networkx.MultiDiGraph` (not plain DiGraph) so multiple edge kinds between same `(src, dst)` are independent. `key=kind` on `add_edge` makes same-kind re-adds idempotent.
- **Node key strategy**: tuple `(symbol.path, symbol.name)` as the networkx node id (hashable). Full `Symbol.model_dump()` stored as node attr `symbol_data`. Additional attr `node_id=[path, name]` (list copy) is the canonical key for save/load round-trip.
- `EdgeKind = Literal["calls", "imports", "defines", "contained_in"]`. Unknown kind on `add_edge` → `IndexingError` with `{"kind": ..., "valid_kinds": [...]}`. `get_args(EdgeKind)` drives the validation set.
- Public API: `add_symbol`, `has_symbol`, `add_edge`, `callers`, `callees`, `save`, `load` (classmethod), `__len__`, `edge_count`. Sync (matches sibling stores).
- `callers` / `callees` filter on `key == "calls"` only (NOT all edge kinds). Dedup + sort by `(path, name)` for deterministic order. Missing symbol → `[]`, not error.
- **Persistence**: `nx.node_link_data(g, edges="edges")` → `json.dumps(sort_keys=True)` → `gzip.compress(level=9)` → `path.write_bytes(...)`. `edges="edges"` (vs default "links") dodges nx 3.x FutureWarning. Pairs symmetric in `load`.
- **Round-trip gotcha**: tuple node ids serialize to JSON lists. `_coerce_key` accepts both list and tuple shapes; load rebuilds the graph fresh, keying nodes by the stored `node_id` attribute (NOT the raw `nx.node_link_graph` node id, which may be coerced inconsistently across nx versions).
- Errors: `save` / `load` wrap `Exception` → `IndexingError` with `{"path": ..., "error": str(exc)}`. `IndexingError` re-raised untouched.
- Mypy strict: `networkx` has no shipped stubs (mypy hint suggests `types-networkx`). Imported via `importlib.import_module("networkx")` + typed `Any`, matching tree-sitter-language-pack + lancedb precedent. Module-level `nx: Any` plus `self._g: Any`. `bool(...)` wrap on `has_node` return.
- No async, no context manager, no close — graph is in-memory; persistence is explicit.
- 16 unit tests cover: round-trip + has_symbol, idempotent add_symbol, add_edge auto-endpoints, all-4-kinds same pair, same-kind idempotent, unknown kind raises, callers/callees filter to "calls" only, missing symbol returns [], deterministic sort order, gzip magic bytes on save, edge_count + len totals, save to missing parent dir raises, load corrupted file raises, full save/load round-trip preserves Symbol payload (incl. line numbers), **acceptance test: 5 symbols + 6 edges across all 4 kinds, save → load, all queries identical**.

### Task 014 — Vector store (LanceDB + Protocol)
- New runtime dep: `lancedb>=0.13,<1.0`. Pulls `pyarrow`, `numpy`, `tqdm`, `urllib3`, etc. (~80MB on install). Heavy but locked-in architectural choice.
- New module `code_atlas.indexing.vector_store`. Re-exports `VectorStore` (Protocol), `VectorItem` (pydantic), `LanceVectorStore` from `code_atlas.indexing`.
- `VectorItem` (frozen pydantic, `extra="forbid"`): `chunk_id`, `repo_id`, `vector: list[float]` (min_length=1), `metadata: dict[str, Any]` (defaults to `{}`). Lives in `vector_store.py`, NOT in `domain/` (persistence record, not pure domain).
- `VectorStore` Protocol: `dimension: int` attr + `upsert(items) -> int`, `search(vector, k=10, filters=None) -> list[(chunk_id, score)]`, `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`. **Sync** API — LanceDB embedded is local file ops; callers wrap in `asyncio.to_thread`.
- `LanceVectorStore(uri, *, table_name="chunks_vec", dimension=768)`. `uri` is a directory path (NOT a SQLAlchemy URL). Creates/opens table on construction.
- PyArrow schema: `chunk_id: string, repo_id: string, vector: list_<float32, dimension>, metadata: string` (JSON-encoded via `json.dumps(..., sort_keys=True)`).
- Idempotency: `tbl.merge_insert("chunk_id").when_matched_update_all().when_not_matched_insert_all().execute(arrow_table)`. Re-upsert keeps row count at 1.
- Search: `tbl.search(vector).metric("cosine").limit(k)` + optional `.where("repo_id = '...'")`. Score = `1.0 - _distance` so higher = more relevant. For exact-match vectors, score ≈ 1.0.
- Filters dict: ONLY `repo_id: str` supported. Unknown keys → `IndexingError`. `repo_id` value containing single-quote → `IndexingError` (no SQL escape logic; reject the input instead). Empty filters dict treated as None.
- `delete_repo` counts BEFORE deleting (LanceDB's delete doesn't return rowcount reliably).
- `count(*, repo_id=...)` uses `tbl.count_rows(filter=...)` (available in lancedb >= 0.6).
- `dimension < 1`, `k < 1`, vector-length mismatch (upsert & search) → `IndexingError` with context.
- LanceDB/PyArrow exceptions wrapped via bare `except Exception` (LanceDB error hierarchy is messy across versions); IndexingErrors we raise ourselves are re-raised untouched.
- Mypy strict: `lancedb` and `pyarrow` imported via `importlib.import_module(...)` and stored as `Any` to dodge stub mismatches under `warn_unused_ignores`. Matches the tree-sitter-language-pack precedent from Task 010. `cast(list[str], ...)` for arrow result extraction.
- **lancedb 0.33 deprecation note**: `table_names()` emits a `DeprecationWarning` recommending `list_tables()`, but lancedb 0.33's `list_tables()` returns `(namespace, name)` tuples containing unhashable lists — not a string list. Don't chase the warning without checking the API; the swap is NOT a drop-in. Kept `table_names()`; warning is noise, not a bug.
- 16 unit tests cover round-trip + positive score, idempotent upsert, empty batch returns 0, repo_id filter isolation in search, delete_repo isolation, delete_repo no-match returns 0, count by repo, on-disk persistence via ctx-mgr, wrong-dim upsert + search, unknown filter key, k<1, single-quote rejected on delete/search/count, score positive for self-match, search k-limit, metadata JSON round-trip.

### Task 013 — Lexical store (SQLite FTS5)
- New module `code_atlas.indexing.lexical_store`; `LexicalStore` re-exported from `code_atlas.indexing`.
- Uses **stdlib `sqlite3`**, NOT SQLAlchemy. Diverges from `MetadataStore`: `url` is a raw filename or `":memory:"` (e.g. `LexicalStore("/tmp/lex.db")`), not a `sqlite:///...` URL. Two stores share no connection.
- Single FTS5 virtual table:
  ```
  CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content, symbol, repo_id UNINDEXED, chunk_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
  );
  ```
- Public API: `upsert(chunk)`, `upsert_many(chunks) -> int`, `search(query, k=10, repo_id=None) -> list[(chunk_id, score)]`, `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`, ctx-mgr, `connection` property.
- **Score convention**: returns `-bm25(chunks_fts)` so higher score = more relevant; rows ordered by raw `bm25 ASC` under the hood. Callers can compare scores naturally (`>`).
- Idempotency: no PK on FTS5 virtual tables, so `upsert` does `DELETE WHERE chunk_id=? + INSERT`. Re-upserting same chunk_id keeps row count at 1.
- `chunk.symbol` is `str | None`; FTS5 columns reject NULL → coerced to `""` on insert.
- All `sqlite3.Error` (incl. `OperationalError` for malformed MATCH) wrapped as `IndexingError` with context (`chunk_id`, `query`, or `repo_id`). Empty/malformed FTS query → `IndexingError`.
- `k < 1` → `IndexingError` (validated before SQL).
- 15 unit tests cover round-trip, idempotency, batch + empty, repo isolation, delete_repo, count-by-repo, on-disk persistence, BM25 multi-term-repetition ordering, positive score, malformed + empty query, k<1, None symbol coercion, no-match empty list.

### Task 012 — Metadata store (SQLite + SQLAlchemy Core) — Phase 3 starts
- New runtime dep: `sqlalchemy>=2.0,<3.0`.
- New pkg `code_atlas.indexing`. Re-exports `MetadataStore`.
- SQLAlchemy Core only (no ORM models). Module-level `MetaData` + single `chunks` table:
  - Columns: `chunk_id PK`, `repo_id` (indexed), `path`, `language`, `kind`, `symbol` (nullable), `start_line`, `end_line`, `content`, `content_hash` (indexed), `indexed_at` (ISO 8601 UTC).
- `MetadataStore(url="sqlite:///:memory:")` constructor creates engine + tables.
- Methods: `upsert(chunk)`, `upsert_many(chunks) -> int`, `get(chunk_id) -> CodeChunk | None`, `get_many(chunk_ids) -> list[CodeChunk]` (preserves input order, skips missing), `delete_repo(repo_id) -> int`, `count(*, repo_id=None) -> int`, `close()`, `__enter__`/`__exit__`, `engine` property.
- Idempotency via SQLite-dialect `insert(...).on_conflict_do_update(index_elements=["chunk_id"], set_={...})`. Re-upserting same chunk_id updates fields (incl. `indexed_at`), preserves row count.
- All SQLAlchemy errors wrapped as `IndexingError` with context (chunk_id or batch count).
- `RowMapping` ≠ `Mapping[str, Any]` for mypy strict — call sites convert via `dict(row)`.
- Use `datetime.UTC` alias (Python 3.11+, per ruff UP017).
- 12 unit tests cover round-trip, idempotent upsert, update semantics, batch + empty batch, get_many order + missing-skip, delete_repo isolation, count-by-repo, on-disk persistence via context manager, engine property.

### Task 011 — Ingestion pipeline — Phase 2 complete
- `code_atlas.ingestion.pipeline.ingest_repo(root, repo_id, *, extra_ignores=None, max_chunk_lines=200, mtime_cache=None, stats=None) -> Iterator[CodeChunk]`.
- Re-exported from `code_atlas.ingestion` along with `IngestStats`.
- Composes walker → language detection → chunker into a lazy chunk stream.
- **Eager validation**: bad root (non-dir) or empty repo_id raise `IngestionError` BEFORE iteration (uses an inner `_iter` generator + outer factory function).
- `IngestStats` (slotted dataclass): counters for `files_seen`, `files_skipped_no_language`, `files_skipped_unreadable`, `files_skipped_unchanged`, `files_chunked`, `chunks_emitted`. Caller can pass an instance; mutated in place.
- `mtime_cache: dict[str, tuple[float, int]]` — caller-managed dict mapping `rel_path` → `(mtime, size)`. Updated only after a file is fully processed (transient parse errors don't poison the cache). On match: skip + bump `files_skipped_unchanged`.
- Per-file flow: walker yields → stat → cache check → language detect → read text → skip empty/whitespace → chunk → yield. OSError on stat or read → warn + skip (counted as `files_skipped_unreadable`).
- `pipeline.completed` info log at end with all counters.
- Non-detectable files (`.txt`, `.md`, `.gitignore`) → `files_skipped_no_language`.
- 9 integration tests in `tests/integration/ingestion/test_pipeline.py` cover: chunk emission, repo_id propagation, relative-path output, all stats counters, mtime-cache skip + refresh on `os.utime` bump, eager errors for bad root + empty repo_id, lazy `next()` consumption.

### Task 010 — Tree-sitter AST chunker
- New runtime deps: `tree-sitter>=0.23,<1.0`, `tree-sitter-language-pack>=0.10,<1.0`.
- `code_atlas.ingestion.parser.chunk_file(*, path, repo_id, language, content, max_chunk_lines=200) -> list[CodeChunk]`. Keyword-only args.
- Re-exported from `code_atlas.ingestion`.
- AST chunking implemented for **python only** in v1. All other languages → fixed-window fallback. Note in `parser.no_ast_extractor` debug log.
- Python AST extraction:
  - One chunk per top-level `function_definition` (kind=function), `class_definition` (kind=class), `decorated_definition` (uses inner def's kind/symbol).
  - One chunk per first-level method inside a class body (kind=method).
  - Decorated classes also emit their nested methods.
  - Symbol extracted from first `identifier` child (UTF-8 sliced from source bytes).
- Empty / whitespace-only content → `[]`.
- Python file with no defs → single `kind="file"` chunk covering whole content.
- Python parser exception → warning log + whole-file chunk.
- Fixed-window fallback: 50-line windows, 5-line overlap. `kind="block"`, `symbol=None`. Last window may be shorter. 100 lines → 3 windows starting at [1, 46, 91].
- `content_hash` = sha256 hex (64 chars) of chunk content.
- `chunk_id` = first 32 chars of `sha256("{repo_id}\n{path}\n{start}-{end}\n{content_hash}")`. Deterministic across runs.
- Tree-sitter handles typed as `Any` (no stubs). `importlib.import_module("tree_sitter_language_pack")` avoids stub-mismatch issues with `mypy --strict + warn_unused_ignores`.
- DEFERRED (follow-up tasks): body-splitting for oversized defs (parameter accepted but unused, marked `_ = max_chunk_lines`); per-language AST extractors for JS/TS/Go/Java/Rust/C/C++; nested classes deeper than one level.
- 11 tests cover Python def extraction, class+methods, decorated, no-defs whole-file, fixed-window math (length + start offsets), empty/whitespace, hash stability, content slicing, isinstance check.

### Task 009 — Language detection
- `code_atlas.ingestion.language.detect_language(path, content=None) -> str | None`.
- Re-exported from `code_atlas.ingestion`.
- Resolution order: extension table (case-insensitive) → shebang line (from `content` if given, else `path.read_text(errors="replace")`) → `None`.
- Extension table maps to tree-sitter-language-pack names: `python` (`.py`, `.pyi`), `javascript` (`.js`, `.mjs`, `.cjs`, `.jsx`), `typescript` (`.ts`, `.mts`, `.cts`), `tsx` (`.tsx`), `go`, `java`, `rust`, `c` (`.c`, `.h`), `cpp` (`.cc`, `.cpp`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++`).
- Shebang interpreter map: `python`/`python2`/`python3` → python, `node` → javascript, `ts-node`/`deno`/`bun` → typescript, `go`/`java`/`rustc` → respective.
- Shebang handles `#!/usr/bin/env <interp>` and absolute paths. Strips trailing digits/dots so `python3.11` → `python` after lookup miss.
- CRLF tolerated. `OSError` on read → `None`.
- Extension wins over shebang (e.g., `script.py` with `#!/usr/bin/env node` → `python`).
- 35 tests (incl. 23 parametrized table cases) all green.

### Task 008 — Repo walker (gitignore-aware)
- New pkg `code_atlas.ingestion`. Re-exports `walk_repo`.
- Runtime dep added: `pathspec>=0.12,<1.0` (uses `GitIgnoreSpec.from_lines`).
- `walk_repo(root: Path, extra_ignores: list[str] | None = None) -> Iterator[Path]`.
  - Resolves root, raises `IngestionError` if not a directory.
  - Yields absolute resolved Path objects.
- Baseline ignore list (always applied): `.git/`, `node_modules/`, `.venv/`, `venv/`, `__pycache__/`, `dist/`, `build/`, `*.lock`, `*.pyc`, `*.pyo`, `*.so`, `*.dylib`, `*.dll`.
- Honors root + nested `.gitignore` (nested patterns scoped to their subtree), `.git/info/exclude` (blanks/comments stripped), and caller-supplied `extra_ignores`.
- Binary detection: null-byte scan in first 8 KB; unreadable files treated as binary (skipped) with debug log.
- `.gitignore` files themselves ARE yielded (matches git semantics — they're tracked).
- Validation is lazy because `walk_repo` is a generator; errors raise on first iteration.
- 10 tests cover: text-only filtering, root + nested `.gitignore`, baseline + extra ignores, `.git/info/exclude`, missing-root + file-root error paths, absolute-path output.

### Task 007 — Domain types (frozen pydantic v2 models)
- New pkg `code_atlas.domain`; 4 modules, zero internal deps. Pure types.
- All models `ConfigDict(frozen=True, extra="forbid")`. Paths are `str` (JSON-stable, cross-platform).
- `chunk.py`: Literal aliases `SymbolKind` (function/method/class/module/variable/constant/other) and `ChunkKind` (file/class/function/method/block). `Symbol{name,kind,path,line,parent}`. `CodeChunk{chunk_id,repo_id,path,language,kind,symbol,start_line,end_line,content,content_hash}`. Chunk `model_validator(after)` raises `ValueError` when `end_line < start_line`.
- `retrieval.py`: Literal `RetrievalSource` (vector/lexical/fused). `RetrievalQuery{text,k=10,filters={}}` with `1 <= k <= 200`. `RetrievalResult{chunk,score>=0,source}`.
- `answer.py`: `Citation{path,start_line,end_line,symbol?,snippet<=4096}` with end>=start invariant. `TokenUsage{prompt,completion,total}` auto-fills `total = prompt + completion` when `total == 0`, else rejects `total < prompt + completion`. `Answer{text,citations=[],trace=[],latency_ms=0,token_usage=TokenUsage()}`.
- Frozen-field write inside validator uses `object.__setattr__(self, ...)` — required pydantic pattern.
- `domain/__init__.py` re-exports 10 names. Absolute imports inside the package.
- 20 unit tests cover round-trip (dict + JSON), validation rejects (empty/negative/out-of-range/extras), frozen-assignment, line-range invariant, k bounds, token-usage reconciliation, citation invariant, Answer defaults, nested round-trip.
- Tests not mypy-checked (config files = src only), so test `# type: ignore` comments are cosmetic-only.

### Task 006 — Config (pydantic-settings + YAML)
- New runtime deps: `pydantic>=2.8`, `pydantic-settings>=2.4`, `pyyaml>=6.0`. Dev: `types-pyyaml`.
- `code_atlas.config` exports `Settings` + `load_settings`.
- Top-level `Settings(BaseSettings)` holds 7 nested `BaseModel` groups: `app`, `ingestion`, `storage`, `embeddings`, `chat`, `ollama`, `eval`.
- Env prefix `CODE_ATLAS_`, nested delimiter `__`, default yaml `config/default.yaml`, default env `.env`, `extra="ignore"`.
- Source precedence (highest first): init kwargs → process env → .env → yaml → secrets. First source with a value wins.
- `load_settings(yaml_path=None, env_file=None, **overrides)`: builds a one-off `_Scoped(Settings)` subclass when paths override; else plain `Settings(**overrides)`. Used by tests; production code calls `Settings()` directly.
- Missing yaml/env paths are silently empty (pydantic-settings default).
- `config/default.yaml` mirrors field defaults verbatim. `.env.example` shows one var per section with `__` nesting + env-wins comment.
- Field-level validation: temperature bounded `[0.0, 2.0]`, integers gt=0 / gte=0 where appropriate.
- Mypy strict required `cast("SettingsConfigDict", merged_dict)` for the `_Scoped` model_config (TypedDict can't accept `**dict[str, Any]`).
- 7 unit tests pass: defaults, yaml override, env-beats-yaml, dotenv layering, init-beats-all, bad-type validation, out-of-range validation. All tests use `monkeypatch.chdir(tmp_path)` + `delenv` for hermeticity.

### Task 005 — Logging setup (structlog)
- Runtime dep: `structlog>=24.1,<26.0` (first non-dev dep).
- `code_atlas.utils` package; re-exports `configure_logging`, `get_logger`.
- `configure_logging(level, *, json=True, stream=None)`:
  - level via `logging.getLevelNamesMapping()`.
  - Resets stdlib root handler so reconfigure is idempotent; attaches `StreamHandler(stream or sys.stderr)`.
  - Processor chain: `merge_contextvars` → `add_log_level` → `StackInfoRenderer()` → (`set_exc_info` only when console) → `TimeStamper(fmt="iso", utc=True)` → `JSONRenderer()` or `ConsoleRenderer(colors=False)`.
  - `wrapper_class=make_filtering_bound_logger(level)`. `logger_factory=PrintLoggerFactory(file=target)`. `cache_logger_on_first_use=True`.
- `get_logger(name=None)` → `structlog.stdlib.BoundLogger` (via `cast`).
- ConsoleRenderer colors=False so test output is stable. Re-enable via env when wiring dev.
- 6 tests pass: JSON parseable, console human, ISO timestamp, bound-logger shape, level filter drops, reconfigure swaps stream.

### Task 004 — Errors module (typed exception hierarchy)
- `src/code_atlas/errors.py`. Base `CodeAtlasError(Exception)` carries `message: str` and `context: dict[str, Any]` (per-instance, defaults to `{}`).
- `__str__`: bare message when context empty; `"{message} | context={ctx!r}"` otherwise.
- `__repr__`: `f"{type(self).__name__}(message=..., context=...)"`.
- 8 subclasses extend the base (one-line docstring each): `ConfigError`, `IngestionError`, `IndexingError`, `ProviderError`, `RetrievalError`, `AgentError`, `EvaluationError`.
- `RepositoryNotIndexed` extends `IndexingError` (not base) so callers catch either.
- `__all__` sorted alphabetically (RUF022).
- `tests/unit/test_errors.py`: 8 tests cover str/repr, inheritance, raise/except context survival, per-instance defaults, repr-form in str.
- `tests/` tree exists; no `__init__.py` per convention (pytest discovers via `pythonpath` + rootdir).
- Verified locally: ruff format/check, mypy strict, 8/8 pytest pass.

### Task 003 — CI pipeline (GitHub Actions)
- `.github/workflows/ci.yml`. Triggers: push + pull_request, all branches.
- Concurrency group cancels in-progress on same ref. Permissions: contents:read.
- Three jobs: `lint`, `type`, `test`. `test` needs `[lint, type]`.
- `lint` + `type` pinned to Python 3.11. `test` matrix on 3.11/3.12, fail-fast off.
- Setup chain per job: checkout@v4 → setup-uv@v3 (uv cache keyed on uv.lock) → `uv python install <ver>` → `uv sync --extra dev`.
- Test step tolerates ONLY pytest exit 5 (no-tests-collected) via shell wrapper.
- Coverage artifact `coverage-py<ver>` per matrix entry; `if-no-files-found: ignore`.

### Task 002 — Dev tooling (ruff, mypy, pytest, pre-commit, editorconfig)
- Dev extras pinned: ruff>=0.6, mypy>=1.11, pytest>=8, pytest-cov>=5, pytest-asyncio>=0.23, pre-commit>=3.7.
- Ruff: line-length 120, py311, select E/F/I/UP/B/SIM/RUF, format double quotes + lf.
- `[tool.ruff.lint.per-file-ignores]` empty placeholder (forward-friendly).
- Mypy strict over `src/code_atlas`. warn_unused_ignores + warn_redundant_casts on.
- Pytest: testpaths `tests`, pythonpath `src`, asyncio_mode auto, markers slow/network/requires_ollama, strict-markers + strict-config.
- Coverage: branch on, source `code_atlas`, excludes pragma + TYPE_CHECKING + NotImplementedError.
- Pre-commit hooks: ruff-format, ruff --fix, generic hygiene (trailing-ws, eof-fixer, check-yaml/toml/merge-conflict), local mypy via `uv run mypy`.
- Editorconfig: utf-8/lf root; py 4-space cap 120; yaml/json/md/toml 2-space; Makefile tabs.
- `uv sync --extra dev` resolves cleanly. Lockfile (`uv.lock`) committed.
- Verified locally: `ruff format --check`, `ruff check`, `mypy src/code_atlas` all pass.

### Task 001 — Project scaffolding (uv + src layout)
- Hatchling backend. src layout via `packages = ["src/code_atlas"]`.
- `pyproject.toml`: name `code-atlas`, version `0.1.0`, `requires-python >=3.11`, license MIT.
- Console script `code-atlas = code_atlas.cli:app` stubbed; `cli` module lands in task 024.
- `dev` extra empty placeholder; ruff/mypy/pytest deferred to task 002.
- `[tool.uv] package = true` set.
- `src/code_atlas/__init__.py` exports `__version__`.
- `src/code_atlas/py.typed` present (PEP 561 typed marker).
- README: title, tagline, install stub.
- `.gitignore` extended with project-specific section (orchestrator backup, data, *.lance, eval/reports, config/local.yaml, .uv-cache).

## Considered but deferred
<!-- Architectural suggestions sub-agents raised that we chose not to act on. -->

## Open issues
<!-- Bugs discovered but not yet scheduled. Move to TASKS.md when scheduled. -->
