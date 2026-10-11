# Agent A/B evaluation (`atlas-eval agent`)

Runs headless Claude Code sessions on the same task in two conditions and scores them, deterministic checks first:

- **baseline**: no Code Atlas.
- **atlas**: the Code Atlas MCP server plus a preamble prepended to the prompt. `--atlas-preamble` selects the variant (`PreambleVariant` in `crates/atlas-eval/src/agent/plan.rs`):
  - `standard` (version 1, the default): `STANDARD_PREAMBLE`, three sentences naming the tools and saying to use them before reading files.
  - `directed` (version 2): `DIRECTED_PREAMBLE`, which says when to call `atlas_context`, `atlas_impact` and `atlas_search`/`atlas_similar`.

  The variant is recorded per run, and `agent report` refuses a batch that mixes variants.

## Layout

```
eval/tasks/<id>/task.toml        task spec (committed)
eval/tasks/<id>/hidden/...       hidden validation files, copied into the tree only after the session
eval/tasks/<id>/base.patch       optional planted change applied to the archived tree before the base commit
eval/tasks/<id>/reference.patch  reference solution (relative to the base, after base.patch); never read by the harness
eval/tasks/README.md             task catalogue; eval/tasks/VERIFICATION.md records the hidden-test verification
eval/runs/<batch>/               run artifacts (gitignored)
eval/reports/<date>/agent.{md,json}
```

## Task spec

```toml
id = "t03-add-endpoint"              # must equal the directory name
title = "…"
repo = "self"                        # or { git = "url", commit = "sha" } or { fixture = "acme" }
base_commit = "<sha>"                # required for self and fixture; for git it must equal `commit` if given
prompt = """…"""                     # identical for both conditions
category = ["reuse", "blast-radius"]
timeout_minutes = 30
max_turns = 120
atlas_repos = ["acme-api", "acme-sdk"]   # optional: tree subdirectories registered as Code Atlas repos (default: the tree root)
base_patch = "base.patch"            # optional, relative to the task dir: applied with `git apply` before the base commit
setup = ["bun install --frozen-lockfile"]   # optional: run with `sh -c` at the tree root before the base commit; each must exit 0

[validation]
hidden_tests = ["hidden/acme-api/test/x.spec.ts"]   # copied to acme-api/test/x.spec.ts (the `hidden/` prefix is stripped)
commands = ["…"]                                     # must pass
regression = ["…"]                                   # must still pass

[ground_truth]                       # all paths are relative to the tree root
relevant_files = []
must_change = []
must_consider = []
reuse_targets = ["crate::money::to_cents", "UsersService.findOne"]
forbidden_duplicates = []
```

Tree sources: `self` is `git archive <base_commit>` of this repository, `fixture` is `git archive <base_commit>:fixtures/<name>`, `git` is a cached bare clone archived at `commit`. Archives are used instead of `git worktree` so the agent cannot reach this repository's history or future commits through `.git`. Stripped from every tree: the root `eval/`, `scratch-pad/` and `hidden/` directories and the destination path of each declared hidden test. `self` trees (and `--repo-override` copies) also lose `docs/plan/`. They keep `fixtures/**/expected.toml`, because this repository's own test suite reads them and fixture ground truth is not task ground truth. Fixture trees (`{ fixture = … }`) lose every `expected.toml` / `expected.schema.md`. The tree is then committed as `eval-base` with a fixed identity and date.

Before that commit, two optional task steps run, in this order, for the base check and for every run alike. `base_patch` is applied with `git apply` (a ceiling directory keeps git from adopting an ancestor repository), so a planted bug is part of `eval-base` and invisible in the agent's history; a patch that does not apply fails the run at stage `materialize`. Then each `setup` command runs with `sh -c` at the tree root, with the harness's own environment and `--validation-timeout-secs` per command; it is meant for one networked dependency step such as `bun install --frozen-lockfile` or `cargo fetch --locked`, whose package cache makes later runs and the validation commands work offline. Files under `target/`, `node_modules/` and `.code-atlas/` are excluded from git, anything else setup writes becomes part of `eval-base`, so it never shows up in the agent's diff. Setup stops at the first failing command; later commands do not run. A failing setup command marks the run `setup_failed` at stage `setup` (a harness stage); on the base check it aborts the batch. The base patch's content is part of the task's `spec_hash`.

`reference.patch` and `base.patch` live in the task directory, which is never inside a tree: `self` trees lose the root `eval/` directory, and `git`/`fixture` trees are archived from elsewhere. `--repo-override <dir>` replaces the source of every task with a copy of that directory (without `.git`, `target`, `node_modules`); the report marks such batches as not reproducible from a commit.

Trees never live under the repository or the eval home: each run gets `<tree-root>/ae-<hash>/tree`, where `<tree-root>` is `--tree-root` (default: the system temp dir) and the hash is derived from the batch and run id, so neither the path nor its ancestors reveal the task, the condition or this repository. The harness refuses to run if any ancestor of the tree is a git repository or the tree sits under the repository or the eval home, so `git rev-parse` from the tree only finds the fresh repo. Every tree builds into `<scratch>/cargo-target` (the run's own scratch directory, beside `tree/`): `CARGO_TARGET_DIR` points there for setup commands, the agent session (both conditions alike) and the validation and regression commands. Rust build output therefore never lands in the tree. Runs never share a target directory either, which matters because archived files all carry the commit's mtime and cargo would otherwise reuse binaries built from another tree. The agent's accesses to that directory count as in-tree. The run's `mcp.json`, Claude config dir and Code Atlas homes (`atlas-home` for the session, `atlas-post-home` for duplication scoring) sit next to the tree in the same scratch directory, and each home passes the same isolation check, so nothing the MCP server prints or carries in its environment points into the repository. An atlas run's scratch directory therefore holds `atlas-home/` (and, after the session, `atlas-post-home/`) next to `tree/`, which a baseline run's does not. This is harmless: the atlas agent already knows its condition from the preamble, and the blinded judge never sees paths or the scratch directory. After scoring, the harness copies the small Code Atlas artifacts (every file in the homes except `*.db`/`*.sqlite` index data and their WAL/SHM files) and the `index` output (`atlas-index.log`) into `eval/runs/<batch>/<run>/`, then deletes the scratch directory unless `--keep-trees` is given. Bash still runs unsandboxed, so an agent could search the whole filesystem; the harness detects this after the fact instead. Every tool call is scanned (Read/Edit/Write/Glob/Grep path fields, and absolute, `~` and `..`-escaping paths in Bash commands, best effort) and paths outside the tree are recorded as `metrics.out_of_tree_accesses`, ignoring toolchain and system locations (`~/.cargo`, `~/.rustup`, `~/.nvm`, `~/.npm`, `~/.volta`, `~/.bun`, `/usr`, `/bin`, `/sbin`, `/opt/homebrew`, `/dev`, Xcode developer dirs). A run that touched the repository root, the eval home or another run's `ae-*` scratch directory is marked `contaminated`; the report lists contaminated runs at the top with the offending paths, counts them per condition, and excludes them from every statistic, including the paired aggregates.

## Commands

```
atlas-eval agent run --dry-run [--task t00-smoke] [--model …] [--seed N]   # planned order and a rough cost estimate
atlas-eval agent run --task t00-smoke --model claude-haiku-4-5-20251001 --judge-model claude-haiku-4-5-20251001 \
  --runs-per-condition 1 --atlas-bin target/release/catl --budget-usd 2
atlas-eval agent report --runs eval/runs/<batch> [--out eval/reports/<date>/agent] [--compare previous/agent.json]
atlas-eval agent probe --out <dir> [--model …]
atlas-eval retrieval [--task t00-smoke] [--out eval/reports/<date>/retrieval]   # deterministic, no sessions
```

`atlas-eval retrieval` reuses the task specs, tree materialisation and clone cache, indexes each base tree in process and scores what `context`, `search` and `similar` return for the prompt against the task's ground truth. It never runs a session. Method and limits: `docs/evaluation/methodology.md` (Retrieval eval).

Run flags: `--tree-root` (system temp dir), `--keep-trees`, `--runs-per-condition` (2), `--condition baseline|atlas` (both), `--seed` (1), `--parallel` (2), `--budget-usd` (stop starting runs once the reported spend exceeds it), `--judge claude|off`, `--no-probe`, `--claude-config user|isolated`, `--validation-timeout-secs`, `--atlas-timeout-secs`, `--eval-home` (`eval`), `--batch` (`<date>-seed<seed>`). A batch is resumable: runs that already have `score.json` are reused.

Per run (`eval/runs/<batch>/<task>.<condition>.r<i>/`): `run.json`, `transcript.jsonl` (+ `transcript.offsets.json`, ms since spawn per line), `diff.patch`, `metrics.json`, `score.json`, and for atlas runs `atlas-review.json` (the unblinded "did Code Atlas mislead the agent" review, never used in the blinded scores). `_base/<task>/base.json` holds the validation and regression results on the untouched base tree; regressions are failures that were not already failing there.

## Isolation

Every session uses the same flags except `--mcp-config` (empty vs Code Atlas), the `mcp__code-atlas` entry in `--allowedTools` and the preamble in the prompt:

```
claude --model <m> --strict-mcp-config --mcp-config <file> --setting-sources "" --disable-slash-commands --no-session-persistence
       --output-format stream-json --verbose --permission-mode bypassPermissions --allow-dangerously-skip-permissions
       --tools Read,Edit,Write,Glob,Grep,Bash --allowedTools Read,Edit,Write,Glob,Grep,Bash[,mcp__code-atlas]
       --add-dir <tree> --max-turns <n> [--append-system-prompt <project instructions>] -p <prompt>
```

**Both conditions receive the project instructions.** `--setting-sources ""` stops Claude Code from auto-loading the project's own `CLAUDE.md` (see the probe below), so the harness injects the tree's root `CLAUDE.md` and `AGENTS.md`, when present and non-empty, through `--append-system-prompt` under a "Project instructions" heading. The text is read from the base tree, so it is byte-identical in both conditions; `run.json` lists which files were injected.

cwd is the tree. The environment is cleared and rebuilt from an allowlist (`PATH HOME USER LOGNAME SHELL TMPDIR LANG LC_ALL LC_CTYPE TERM`, `ANTHROPIC_*`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK/VERTEX`) plus `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `DISABLE_AUTOUPDATER=1` and `ENABLE_TOOL_SEARCH=false` (MCP tool schemas are loaded up front instead of through a deferred tool-search step). Variables of a parent Claude Code session (`CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, …) never reach the child. Timeouts kill the whole process group.

`--claude-config isolated` also sets `CLAUDE_CONFIG_DIR` to a per-run directory. With claude.ai OAuth login that directory is not logged in (credentials are tied to the config dir), so it only works with `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`; the default is `user`.

Probe verification (2026-10-04, Claude Code 2.1.289, `claude-haiku-4-5-20251001`, the exact baseline flags; recorded transcript in `crates/atlas-eval/tests/fixtures/probe-transcript.jsonl`):

| question | result |
|---|---|
| tools | exactly Bash, Edit, Glob, Grep, Read, Write |
| MCP servers | none (the user's Serena, context7, Playwright, … are not loaded) |
| skills / slash commands | none |
| plugins | only built-ins: `cc-plugin-agents-md`, `cc-plugin-telemetry`, `cc-plugin-plugin-authoring` |
| user `~/.claude/CLAUDE.md` | not seen |
| project `CLAUDE.md` in the tree | not auto-loaded by the flags alone (the probe injects nothing); eval sessions get it, and `AGENTS.md`, through `--append-system-prompt` in both conditions, as described above |
| permission mode | bypassPermissions; the session created `hello.txt` without prompting |

The CLAUDE.md answers are the model's self-report: it was asked to quote a canary from the project CLAUDE.md and the first heading of the user CLAUDE.md, and answered `none` for both without reading either file. Every batch re-runs the probe (about $0.01) and records it in `batch.json` and the report, unless `--no-probe` is given.

## Code Atlas CLI surface

All `catl` invocations go through `AtlasCli` (`crates/atlas-eval/src/agent/atlas_cli.rs`), with `CODE_ATLAS_HOME` set to a per-run directory in the scratch dir:

- `catl project create <name> --repo <path>...` and `catl index --project <name>` (index time is recorded separately as `atlas_index_ms`);
- `mcp.json`: `{"mcpServers":{"code-atlas":{"command":<bin>,"args":["mcp","--project",<name>],"env":{"CODE_ATLAS_HOME":…}}}}`;
- duplication scoring indexes the post-session tree in a second home and runs `catl query similar --snippet <file>:<start>-<end> --json` (cwd = tree) for every function the diff added. Each cluster member's `structural` signal gives its Jaccard and its `lexical` signal its BM25 score; other JSON shapes are read leniently (a symbol from `key`/`symbol`/`qualified_name`/`target`/`name`, a Jaccard from `structural_score`/`jaccard`, a lexical score from `score`, or a `verdict`). A structural Jaccard ≥ 0.8 or an inferred-equivalent verdict against a reuse target or forbidden duplicate counts as duplication introduced. A match against a target with a lexical score but no qualifying structural Jaccard (≥ 0.8) is not duplication: it is stored in `duplication.lexical_candidates` and reported as `lexical_duplicate_candidates`, whose count is unknown when any query failed. `small_structural` signals (small bodies) do not count, matching the product, whose verdict treats them as shape only. If the binary or the query fails, duplication is `unavailable`/unknown, never `false`.
- run files written before the structural/lexical split stored every score as `{"kind": "structural", "score": …}`. `agent report` reads that as an unclassified score: above 1 it cannot be a Jaccard, so it becomes a lexical candidate; at most 1 it stays an unclassified finding, which makes `duplication_introduced` unknown rather than true. Those files never stored matches scoring below 0.8, nor structural matches below 0.8 with a lexical score, so re-scored candidate counts are a lower bound and are not comparable with candidate counts from new runs. Every finding in `2026-10-04-seed1` and `2026-10-05-directed` scored between 5.24 and 8.36, so both batches re-score exactly.

An atlas run whose session does not report the `code-atlas` MCP server as `connected` is recorded as `setup_failed` (stage `atlas-mcp`) and excluded from the statistics.

## Binary identity and version drift

Two different commits matter in a batch, and the environment records both. The eval repository commit (`code_atlas_commit`, reported as "eval repository commit") is the commit the tasks and the harness come from. The Code Atlas binary that serves the atlas condition (`--atlas-bin`) may have been built from a different commit. `catl --version` prints the commit the binary was built from (`code-atlas 0.1.0 (commit <sha>)`, or `unknown` without git at build time). The batch manifest stores it as `code_atlas_version`, together with the blake3 hash of the binary (`atlas_bin_blake3`), and the report's environment table lists both. The commit says which source was built; the hash catches uncommitted local changes and rebuilds. Binaries built before the commit was added to `--version` print only the version number, and for those the hash is the only identifier.

Self tasks freeze the tree at `base_commit`, but the atlas binary does not. A binary built after a self task's feature landed can hand the atlas agent the answer:
- **t05** asks for the `atlas_dependents` / `atlas_dependencies` MCP tools. A newer server would list and describe them in the atlas session's tool list.
- **t01** asks for cwd-relative CLI targets; a newer binary would show the expected behaviour.

Compare the recorded build commit with the tasks' `base_commit` before trusting self-task results, and prefer a binary built from `base_commit` (or one that predates the features) for those tasks.

## Metric caveats

- Totals come from the final `result` event; per-turn usage is what the stream reported for each message and its output tokens are partial (the stream emits a message before generation finishes).
- Files read = Read inputs plus the simple forms of `cat`, `head`, `tail`, `sed -n` in Bash (with a leading `cd`); globs, variables, subshells and loops are counted as `unparsed_bash_reads`.
- `relevant_files_missed` and `must_consider_coverage` both credit files that appeared in Code Atlas `MUST READ` sections (their bodies were in the agent's context), besides files read or changed. Information density counts bytes read from relevant files plus Code Atlas response bytes in items located in relevant files; it is a heuristic.
- Atlas accuracy extracts tagged relation lines (`[resolved]`, `[heuristic 0.72]`, `[inferred 0.81 stale]`) and counts `DEGRADED`/stale headers. Relations are reported as `unverified` until a SCIP reconciliation helper is available to the harness.
- Sessions that hit `--max-turns`, time out, end without a result event or report an error are still scored, but each condition's outcome counts (completed, max_turns, reported_error, timed_out, no_result, setup_failed) are reported per task, every metric cell whose sample is smaller than the run count prints its `n`, and a higher atlas rate of timeouts, max-turns hits or missing results is listed as a negative result. Runs without a result event use a cost estimated from per-turn usage, recorded as `cost_source = estimated_from_usage` and counted per task.
- Setup failures are excluded from the statistics and listed individually, split into Code Atlas stages (`atlas-project`, `atlas-index`, `atlas-mcp`) and harness stages (`isolation`, `materialize`, `setup`, `git-base`, `mcp-config`).
- Aggregates are means of per-task means over tasks scored in both conditions only (paired); the headers give the task count. `first_edit_ms`, `session_ms` and `wall_ms` exclude indexing, which is reported as `atlas_index_ms`.
- The dry-run cost is a rough guide from a fixed token profile and list prices, not a bound; `--budget-usd` uses the costs the CLI reports.
