# Task 022 — Agent tools (file + symbol lookups)

**Status:** done
**Sub-agent:** general-purpose (agentId a2642576ab8c12362)

## Files changed
- src/code_atlas/indexing/metadata_store.py (modified — added find_by_path)
- src/code_atlas/indexing/symbol_graph.py (modified — added find_by_name)
- src/code_atlas/agent/__init__.py (new)
- src/code_atlas/agent/tools.py (new)
- tests/unit/indexing/test_metadata_store.py (modified — +2 tests)
- tests/unit/indexing/test_symbol_graph.py (modified — +2 tests, one with fix)
- tests/unit/agent/test_tools.py (new)

## Sub-agent response (verbatim)

### Summary
Add `MetadataStore.find_by_path` + `SymbolGraph.find_by_name` query helpers. New `code_atlas.agent` package with `Toolbox` bundling four LLM-callable tools (`open_file`, `find_symbol`, `list_callers`, `list_callees`) bound to a `repo_id`, paired with `ToolSpec` JSON schemas for the LLM.

### State update
New `code_atlas.agent` package seeded with `Toolbox` (4 LLM-callable sync tools + 4 `ToolSpec` JSON schemas, bound to `repo_id`). `MetadataStore.find_by_path` and `SymbolGraph.find_by_name` added to back the tools. All tools return deterministic typed dicts; unknown symbol returns `{"results": []}` and never raises; only `AgentError` on invalid args / unknown dispatch.

### Next task
Task 023: `agent/prompts.py` + `agent/qa.py` (async orchestrator driving the LLM tool-use loop via `LLMProvider.chat`, wrapping `Toolbox.call` in `asyncio.to_thread`).

## Apply notes

- Diff was clean (no HTML entities). Applied via Edit (metadata_store.py, symbol_graph.py, test_metadata_store.py, test_symbol_graph.py) + Write (agent/__init__.py, agent/tools.py, test_tools.py).
- Three post-write fixes:
  1. **Ruff format**: 2 files needed reformat (tools.py, test_tools.py — minor line-wrap differences in long descriptions). Auto-fixed via `ruff format`.
  2. **Ruff I001**: `agent/__init__.py` import order — isort wanted `Toolbox, ToolResult` (uppercase T before mixed-case TR). Auto-fixed via `ruff check --fix`.
  3. **Real test bug**: sub-agent's `test_find_by_name_returns_matches_sorted` added TWO `foo` symbols at the same path (`a/first.py`) with different lines — but `SymbolGraph` node key is `(path, name)` so the second overwrote the first, returning 2 matches instead of 3. **Fixed** by spreading the three `foo` symbols across distinct paths (`a/first.py`, `m/mid.py`, `z/last.py`). Added an inline comment in the test explaining the constraint. **Carry-forward**: future symbol-graph features (call-extractor, overload handling) must respect that node identity is per-file.
- Scope expanded beyond the original TASKS.md "files" line (`agent/__init__.py, agent/tools.py, test_tools.py`) to include `metadata_store.py` and `symbol_graph.py`. **Justified**: each got ONE small query method (`find_by_path`, `find_by_name`) that the tools need. Alternative was raw SQL via `metadata_store.engine` and `graph._g.nodes(...)` private-attr access in the tools module — worse for layering and testability. Five files modified total is reasonable for a "primary + 2 helper" task.
- Acceptance verified locally:
  - `uv run ruff format --check src tests` → 53 files already formatted (after auto-fix)
  - `uv run ruff check src tests` → All checks passed
  - `uv run mypy src/code_atlas` → Success (33 source files, strict)
  - `uv run pytest tests/ -q` → 252 passed (230 prior + 22 new — 15 tools + 2 metadata + 2 symbol_graph; minor over the sub-agent's "~19 new" estimate due to test count)
- Sub-agent's "Next task" matches the planned Task 023 (Prompts + QA agent).
