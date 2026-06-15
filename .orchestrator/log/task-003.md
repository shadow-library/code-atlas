# Task 003 — CI pipeline (GitHub Actions)

**Status:** done
**Sub-agent:** general-purpose (agentId a064bbf1ae784fde2)

## Files changed
- .github/workflows/ci.yml (new)

## Sub-agent response (verbatim)

### Summary
Create `.github/workflows/ci.yml` with three jobs (lint, type, test). Test depends on lint+type, runs matrix on Python 3.11/3.12, tolerates pytest exit code 5, and uploads per-version coverage artifacts.

### State update
- ci.yml made. 3 jobs: lint, type, test. test needs lint+type.
- Matrix py 3.11+3.12.
- Pytest exit 5 tolerated only (pre-task-004 path).
- Coverage artifact per py ver.
- uv cache on, keyed on uv.lock.

### Next task
Task 004: add first pytest scaffolding under `tests/unit/` (smoke test importing `code_atlas`) so coverage artifact actually populates and the exit-code-5 branch becomes dead code.

## Apply notes

- Sub-agent's diff had a malformed `@@` hunk header (no line ranges) — common for new-file diffs from LLMs. Wrote the file directly via Write; content matches diff bytes exactly.
- Acceptance verified: YAML parses, `jobs` = [lint, type, test], `test.needs` = [lint, type], matrix = ["3.11", "3.12"], all actions use major-version tags, exit-code-5 only tolerated.
- Sub-agent's Next task suggestion (smoke test) conflicts with the planned task 004 (Errors module). Tests for the errors module will arrive in task 004 itself per its brief. Ignored.
