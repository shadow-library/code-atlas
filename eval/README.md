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
eval/tasks/<id>/reference.patch  reference solution (relative to the base, after base.patch); never read by the agent harness, read by `retrieval` only to score
eval/tasks/<id>/queries.toml     optional: `atlas_context` calls recorded from agent transcripts, replayed by `retrieval`
eval/tasks/README.md             task catalogue; eval/tasks/VERIFICATION.md records the hidden-test verification
eval/runs/<batch>/               run artifacts (gitignored)
eval/reports/<date>/agent.{md,json}
eval/private/<name>/             a private task set with its own tasks/, runs/ and reports/ (gitignored, see "Private task sets")
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
prompt_leak = "guided"               # optional: handed | guided | open, the class `agent leak-audit` computes; a differing value fails the audit
strip = ["acme-e2e", "docs/postmortem.md"]            # optional: files or directories removed from the tree in both conditions
drop_instruction_sections = ["AGENTS.md#Code search"] # optional: `<file>#<heading>`, a Markdown section removed from that file in both conditions
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

Before that commit, four optional task steps run, in this order, for the base check and for every run alike: `base_patch`, `strip`, `drop_instruction_sections`, `setup`. The first three also shape the tree that `retrieval`, `agent leak-audit` and `agent headroom` read; `setup` does not run there.

`strip` removes each listed file or directory, for example a suite or a note that states the diagnosis a task asks for. `drop_instruction_sections` removes one Markdown section per entry (`<file>#<heading>`: the heading line, any level, through the line before the next heading of the same or a higher level; headings inside code fences are ignored, a fence closing only on a run of its own character at least as long as the one that opened it). Only a plain `#` heading at the start of a line is matched: one with closing hashes, an underlined (setext) heading or an indented one is not, and the drop then fails. It exists for project instructions that direct the agent to a tool the sessions do not have: the root `CLAUDE.md` and `AGENTS.md` are read from the tree after these steps, so the injected text and the file on disk agree and both conditions get the same edit. Both steps run after `base_patch`, so a patch may touch what is stripped afterwards. A listed path that does not exist, or a heading that does not match exactly one section, fails the run at stage `materialize`: a task never runs silently unedited. So does a path that passes through a symbolic link, because the link could lead out of the tree; `strip` may name a link itself, which is removed as a link, and a section is never dropped from a linked file. Neither shows up in the agent's diff, both are part of the `spec_hash`, and `batch.json` lists the dropped sections per task under `instruction_sections_dropped`. Removing the line from `task.toml` undoes either.

`base_patch` is applied with `git apply` (a ceiling directory keeps git from adopting an ancestor repository), so a planted bug is part of `eval-base` and invisible in the agent's history; a patch that does not apply fails the run at stage `materialize`. Then each `setup` command runs with `sh -c` at the tree root, with the harness's own environment and `--validation-timeout-secs` per command; it is meant for one networked dependency step such as `bun install --frozen-lockfile` or `cargo fetch --locked`, whose package cache makes later runs and the validation commands work offline. Files under `target/`, `node_modules/` and `.code-atlas/` are excluded from git, anything else setup writes becomes part of `eval-base`, so it never shows up in the agent's diff. Setup stops at the first failing command; later commands do not run. A failing setup command marks the run `setup_failed` at stage `setup` (a harness stage); on the base check it aborts the batch. The base patch's content is part of the task's `spec_hash`.

`reference.patch` and `base.patch` live in the task directory, which is never inside a tree: `self` trees lose the root `eval/` directory, and `git`/`fixture` trees are archived from elsewhere. `--repo-override <dir>` replaces the source of every task with a copy of that directory (without `.git`, `target`, `node_modules`); the report marks such batches as not reproducible from a commit.

Trees never live under the repository or the eval home: each run gets `<tree-root>/ae-<hash>/tree`, where `<tree-root>` is `--tree-root` (default: the system temp dir) and the hash is derived from the batch and run id, so neither the path nor its ancestors reveal the task, the condition or this repository. The harness refuses to run if any ancestor of the tree is a git repository or the tree sits under the repository or the eval home, so `git rev-parse` from the tree only finds the fresh repo. Every tree builds into `<scratch>/cargo-target` (the run's own scratch directory, beside `tree/`): `CARGO_TARGET_DIR` points there for setup commands, the agent session (both conditions alike) and the validation and regression commands. Rust build output therefore never lands in the tree. Runs never share a target directory either, which matters because archived files all carry the commit's mtime and cargo would otherwise reuse binaries built from another tree. The agent's accesses to that directory count as in-tree. The run's `mcp.json`, Claude config dir and Code Atlas homes (`atlas-home` for the session, `atlas-post-home` for duplication scoring) sit next to the tree in the same scratch directory, and each home passes the same isolation check, so nothing the MCP server prints or carries in its environment points into the repository. An atlas run's scratch directory therefore holds `atlas-home/` (and, after the session, `atlas-post-home/`) next to `tree/`, which a baseline run's does not. This is harmless: the atlas agent already knows its condition from the preamble, and the blinded judge never sees paths or the scratch directory. After scoring, the harness copies the small Code Atlas artifacts (every file in the homes except `*.db`/`*.sqlite` index data and their WAL/SHM files) and the `index` output (`atlas-index.log`) into `eval/runs/<batch>/<run>/` (config files seeded by `--atlas-config` are excluded from the preserved homes), then deletes the scratch directory unless `--keep-trees` is given. Bash still runs unsandboxed, so an agent could search the whole filesystem; the harness detects this after the fact instead. Every tool call is scanned (Read/Edit/Write/Glob/Grep path fields, and absolute, `~` and `..`-escaping paths in Bash commands, best effort) and paths outside the tree are recorded as `metrics.out_of_tree_accesses`, ignoring toolchain and system locations (`~/.cargo`, `~/.rustup`, `~/.nvm`, `~/.npm`, `~/.volta`, `~/.bun`, `/usr`, `/bin`, `/sbin`, `/opt/homebrew`, `/dev`, Xcode developer dirs). A run that touched the repository root, the eval home, another run's `ae-*` scratch directory, or the repository its own tree was archived from (the local directory a task's `repo.git` names, absolute or relative to the clone cache, and the clone or `--repo-override` directory the archive was taken from) is marked `contaminated`; the report lists contaminated runs at the top with the offending paths, counts them per condition, and excludes them from every statistic, including the paired aggregates.

## Commands

```
atlas-eval agent run --dry-run [--task t00-smoke] [--model …] [--seed N]   # planned order and a rough cost estimate
atlas-eval agent run --task t00-smoke --model claude-haiku-4-5-20251001 --judge-model claude-haiku-4-5-20251001 \
  --runs-per-condition 1 --atlas-bin target/release/catl --budget-usd 2
atlas-eval agent run --base-only [--task t00-smoke]                        # base check of every task (setup, hidden tests, regression); no session
atlas-eval agent leak-audit [--tasks <dir>] [--task t00-smoke] [--out …]   # which prompt terms return a file the task must change; no session
atlas-eval agent report --runs eval/runs/<batch> [--out eval/reports/<date>/agent] [--compare previous/agent.json]
atlas-eval agent probe --out <dir> [--model …]
atlas-eval agent headroom --runs eval/runs/<batch>... [--out eval/reports/<date>/headroom]         # recorded sessions only
atlas-eval agent noise --runs eval/runs/<batch>... [--ab-report eval/reports/<date>/agent.json...] \
  [--out eval/reports/<date>/noise-floor]                                                         # recorded runs only
atlas-eval retrieval [--task t00-smoke] [--out eval/reports/<date>/retrieval]   # deterministic, no sessions
```

`atlas-eval retrieval` reuses the task specs, tree materialisation and clone cache, indexes each base tree in process and scores what `context`, `search` and `similar` return against the task's ground truth. It never runs a session. Every task is queried four ways: with the whole prompt (`full`), with the `task` text agents really sent to `atlas_context` (`agent`), with those calls as sent, seeds included (`agent-seeded`), and with the first sentence of the prompt (`first-line`). Method, metrics and limits: `docs/evaluation/methodology.md` (Retrieval eval).

`queries.toml` holds the recorded calls of one task, one `[[query]]` per distinct (task, seeds) pair, so the report reproduces without the gitignored transcripts:

```toml
no_call = [{ batch = "<batch>", run = "<task>.atlas.r0" }]   # optional: atlas runs of the batch that never called atlas_context

[[query]]
task = "…"                                                   # the `task` argument, verbatim
seeds = ["…"]                                                # optional: the `seeds` argument, when one was sent
sent_by = [{ batch = "<batch>", run = "<task>.atlas.r1" }]   # every run that sent exactly this call
```

A task without the file, or with no `[[query]]`, is `n/a` for the two agent variants and leaves their means; it is never scored 0. A malformed file fails the task at stage `queries`.

Run flags: `--tree-root` (system temp dir), `--keep-trees`, `--runs-per-condition` (2), `--condition baseline|atlas` (both), `--seed` (1), `--parallel` (2), `--budget-usd` (stop starting runs once the reported spend exceeds it), `--judge claude|off`, `--no-probe`, `--base-only`, `--claude-config user|isolated`, `--validation-timeout-secs`, `--atlas-timeout-secs`, `--eval-home` (the parent of the task directory, so `eval` for `eval/tasks`), `--batch` (`<date>-seed<seed>`). A batch is resumable: runs that already have `score.json` are reused.

**Where output goes.** An eval home is a directory with `tasks/`, `runs/` and `reports/`, and every command derives it from its input. `agent run` writes its batch and the clone cache (`runs/.cache`) into the home of the task directory. `retrieval` and `agent leak-audit` write `reports/<date>/<name>` beside the task directory. `agent report`, `agent headroom` and `agent noise` write `reports/<date>/<name>` into the home of the first `--runs` batch, and `agent headroom` reads the task specs from that home unless `--tasks` or `--repo-root` says otherwise. A batch that is not laid out as `<home>/runs/<batch>` has no home, so those three need `--out`. For `eval/tasks` and `eval/runs/<batch>` this is `eval/reports/<date>/<name>`, as before.

`agent run --base-only` plans no run: it materialises every task, runs `setup`, copies the hidden tests in and records the validation and regression results in `_base/<task>/base.json`. It starts no session and no probe. Running it on a copy of a task whose `base_patch` is the task's `reference.patch` checks the reference solution with the same code path.

`agent leak-audit` materialises each task's base tree (no `setup`) and searches every code span, identifier-shaped token and quoted phrase of the prompt in it as a fixed string, the way `agent headroom` derives entry points. A term reaches a task in one of two ways, reported as separate reasons: a file it returns is a `must_change` file, or a file it returns names one (it contains the path of a `must_change` file, or its file name when no other file of the tree carries that name; this is how a comment in a test or a note hands over the diagnosis). The report lists each reaching term with the number of files it returns, the same for the injected project instructions, and a class per task: `handed` (a prompt term returns at most 3 files and reaches the task through one of them), `guided` (a term reaches it among more files) or `open` (none does). A task that declares another `prompt_leak` is listed and makes the command exit non-zero after the report is written. What the audit cannot see: a label or message cited without quotes (it is not a term), and output a command named in the prompt would print.

Per run (`eval/runs/<batch>/<task>.<condition>.r<i>/`): `run.json`, `transcript.jsonl` (+ `transcript.offsets.json`, ms since spawn per line), `diff.patch`, `metrics.json`, `score.json`, and for atlas runs `atlas-review.json` (the unblinded "did Code Atlas mislead the agent" review, never used in the blinded scores). `_base/<task>/base.json` holds the validation and regression results on the untouched base tree; regressions are failures that were not already failing there.

## Private task sets

A task set built from a repository that must not be published keeps everything in an eval home of its own, `eval/private/<name>/`, which `.gitignore` excludes:

```
eval/private/<name>/tasks/<id>/      task specs, hidden tests, reference patches
eval/private/<name>/runs/<batch>/    run artifacts, and the clone cache in runs/.cache
eval/private/<name>/reports/<date>/  every report derived from the set
```

Point a command at the task directory or at a batch of that home and its output stays there, by the rule above:

```
atlas-eval agent run --tasks eval/private/<name>/tasks --base-only
atlas-eval agent leak-audit --tasks eval/private/<name>/tasks
atlas-eval retrieval --tasks eval/private/<name>/tasks
atlas-eval agent run --tasks eval/private/<name>/tasks --atlas-bin target/release/catl …
atlas-eval agent report --runs eval/private/<name>/runs/<batch>
```

A guard backs the defaults. It asks git about every source and every output of a command.

- **Sources.** The task directory (`--tasks`, also on `agent headroom`), the eval home of each `--runs` batch (as the path is written, and as it resolves on disk when the two differ), the batch directory itself when it is not laid out as `<home>/runs/<batch>`, and every `--ab-report` of `agent noise`.
- **Outputs.** The report base (`--out` or the default), the eval home of `agent run` (`--eval-home` or the default), `--cache-dir` of `retrieval` and `agent leak-audit`, and `--work-dir` of `retrieval`. An output is judged where the file system puts it: the deepest existing directory of the path is resolved through links and `..` first.
- **Refused.** When any source is ignored by git, every output that git would track: `--out eval/reports/…`, `--eval-home eval`, a clone cache or work directory in a tracked place, a default that resolves to one of these.
- **Refused when git cannot answer.** A source inside a repository (an ancestor holds `.git`) for which `git check-ignore` fails counts as private, and an output inside a repository for which it fails counts as tracked.
- **Accepted.** Outputs that git ignores and outputs outside any repository.
- **Not recognised.** A task set or batch outside any repository is never private to the guard, so a report on it can be written anywhere. A batch whose task directory was private but whose own directory and home are not ignored (for example a base check run with `--eval-home` outside the repository) is in this class. A public batch kept in an ignored directory outside the `<home>/runs/<batch>` layout is treated as private.

Nothing derived from such a repository belongs in a tracked file: no code, prompt, path, patch or identifier, in tasks, reports, docs, tests or commit messages. A task's `repo` may name a local path (`{ git = "/path/to/repo", commit = "<sha>" }`); the harness clones it into the home's cache and archives from the clone, and never writes to the source.

### Building a task from a merged commit

- **Base and reference.** `commit` is the parent of the real commit. `reference.patch` is the real change minus the hidden tests: a test file the commit only added cases to is left out, a test file whose existing cases had to change stays in.
- **Diagnoses in the tree.** Search the base tree for the error text, the route and the labels the prompt cites. A skipped test, a note or a report that names the cause or the file hands the task over in one search; remove it with `strip` in every task of the set, not only where it was noticed, and check that install, build and the regression commands still pass without it.
- **Hidden tests.** Prefer the cases the real commit added. Put them in new files, so the agent's tree keeps every file of the base (a hidden test's destination is stripped from the tree). Test through the outermost surface that exists at the base (the route, the application, the public function a user-visible behaviour passes through), so a correct solution built differently from the reference passes. Assert the property the prompt states, not the reference's wording, order or encoding; where a choice must be pinned, state it in the prompt as a requirement and count it as part of what the task hands over. Check the other direction too: write down a wrong or partial solution and make sure a hidden test fails for it. Hidden tests must type-check at the base when a type-check is part of `regression` (cast new values rather than leave an error that would mask a regression).
- **Regression.** The existing tests of every package touched, with the hidden files excluded, plus a type-check or build of those packages and of the packages that consume them. A refactor also checks for leftovers: the old name, the old file, or whatever structural fault the task removes.
- **Verification.** `agent run --base-only` on the set: validation must fail at the base for the expected reason, not a setup error, and regression must pass. Then the same on a copy of each task with `base_patch = "reference.patch"`: everything must pass. Record both.
- **Prompt.** Written the way the product owner would ask: the behaviour wanted or the symptom seen. No file paths, no directory names, no internal identifiers unless a user would know them (a route, a UI label, an error text). Put every user-visible label and message the prompt cites in double quotes, so the leak audit sees it as a term. Do not name a command whose output lists the files to change. A bug report quotes the failure output captured at the base, and the task says how the capture was made. Write it from the intent of the commit message, not from the diff, then run the leak audit and record the class as `prompt_leak`. Keep a realistic prompt that turns out `handed`; never distort a prompt to hide a name.
- **Ground truth.** `must_change` is the set of files any correct solution has to change, without tests and generated files; record which of them a hidden test or a regression command enforces. Everything else the reference touched goes to `relevant_files` with its reason, next to what a careful engineer must read (callers, the type or schema it depends on, the sibling that sets the pattern, a generated file that has to be regenerated). A task with one required file is recorded as such. A refactor lists every use site.
- **Dependencies.** `setup` installs from a local package cache without the network where the package manager allows it, and the verification notes say what a cold machine needs.

## Analyses of recorded runs

`agent headroom` and `agent noise` read batches that already exist under `eval/runs/` and start no session. Both write a `.md` and a `.json` and take several batch directories. Their reports carry the full definitions; the summary below is what to know before reading one.

**`agent headroom`** reports when a session reaches the files its task must change, and where it got its entry point from. For every completed, uncontaminated session it finds, per `must_change` and `relevant_files` entry, the first call at which the file is *named* in a result of a Grep, Glob, Bash or MCP call that did not itself name it, *shown* (a result carries lines of it), *searched* (a search addressed to that one file), *read* (Read, or `cat`/`head`/`tail`/`sed -n` in Bash) and *edited* (Edit/Write, `sed -i`, `perl -i`, a redirect, `tee`, or a Python script that writes and names the file).

- Two views are given side by side: the **first file** of the set (when the session has an entry point) and **every file** of the set (when the last file is first read or first touched), plus counts pooled over files. The first-file view alone says nothing about locating the rest of a multi-file change.
- **Where the entry point comes from.** Per task, the identifiers, code spans and quoted phrases of the prompt, and separately of the injected project instructions, are grepped as fixed strings in the base tree; the report says which return a `must_change` file and how many files they return. Per session, the search pattern (or opened path) that first reached a `must_change` file is compared word by word with the prompt and the instructions.
- Ordinals are given on two scales. A *turn* is one model response (one assistant `message.id`), which may issue several tool calls. The *tool-call ordinal* is the position of the call in the transcript. The `num_turns` that `agent report` prints as "turns" is on the tool-call scale: in the recorded batches it equals tool calls + 1.
- Files absent from the base tree (the task creates them) are listed separately and excluded. The base tree is materialised the way a run does it (archive of the repository and commit recorded in `run.json`, stripped paths removed, `base.patch` applied), so the command needs that repository; for `git` tasks that is the clone cache under `eval/runs/.cache`.
- Bash is interpreted lexically. The shell's working directory is followed across calls the way Claude Code keeps it: a command's final directory persists only if the command exits 0, which the transcript records per call (`is_error`, `returnCodeInterpretation`, a backgrounded command's `backgroundCwdHint`). The stored `files_read` metric resolves paths against a `cd` inside the same command only, so it can differ.
- Baseline sessions of all batches are pooled; atlas sessions are grouped by preamble variant. The report states its own limits (few tasks, repeated runs, same-author prompts that name code) and ends with detection checks: mentions in unrecognised forms, `cd` failures, and recognised edits compared with each session's final diff.

**`agent noise`** is an A/A analysis: how much each metric of `agent report` (plus `cache_weighted_tokens` = input + 1.25 × cache-creation + 0.1 × cache-read tokens) differs between runs of the same condition.

- It first checks whether the baseline is the same session in every pair of batches (model, CLI version, config, probe, and per run the spec hash, tree source, base commit, masked command line and environment names). Tasks that differ are left out of the across-batch comparison; a differing session field cancels it.
- A/A differences are the second run minus the first run of a task within one batch and condition, and the baseline per-task mean of one batch minus the other. Each comparison reports pairs, median absolute and relative difference, range, and the paired statistic of the A/B report (difference of the means of per-task means).
- Each A/B effect (atlas minus baseline over the tasks that have the metric in both conditions) is set against a noise bound: the largest of 2 × the replicate standard error of that batch, 2 × the pooled one, and the largest paired A/A difference seen for the metric, the last brought to the scale of an A/B effect (a single-run difference is divided by √2 with two runs per cell). The effect is `inside` or `outside` that bound, or `no difference`. Beside every class the report prints the effect in standard errors and how many tasks, left out one at a time, move it across the bound. `--ab-report` names committed `agent.json` files; their means are checked against the recomputation from the run records. `--repo-root` lets it list what changed in the harness between the commits of two batches.
- With two runs per cell this is a rough scale, not a significance test, and it describes the task set that was run, not a harder one.

## Isolation

Every session uses the same flags except `--mcp-config` (empty vs Code Atlas), the `mcp__code-atlas` entry in `--allowedTools` and the preamble in the prompt:

```
claude --model <m> --strict-mcp-config --mcp-config <file> --setting-sources "" --disable-slash-commands --no-session-persistence
       --output-format stream-json --verbose --permission-mode bypassPermissions --allow-dangerously-skip-permissions
       --tools Read,Edit,Write,Glob,Grep,Bash --allowedTools Read,Edit,Write,Glob,Grep,Bash[,mcp__code-atlas]
       --add-dir <tree> --max-turns <n> [--append-system-prompt <project instructions>] -p <prompt>
```

**Both conditions receive the project instructions.** `--setting-sources ""` stops Claude Code from auto-loading the project's own `CLAUDE.md` (see the probe below), so the harness injects the tree's root `CLAUDE.md` and `AGENTS.md`, when present and non-empty, through `--append-system-prompt` under a "Project instructions" heading. The text is read from the base tree, so it is byte-identical in both conditions; `run.json` lists which files were injected.

cwd is the tree. The environment is cleared and rebuilt from an allowlist (`PATH HOME USER LOGNAME SHELL TMPDIR LANG LC_ALL LC_CTYPE TERM`, `ANTHROPIC_*`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK/VERTEX`) plus `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `DISABLE_AUTOUPDATER=1` and `ENABLE_TOOL_SEARCH=false` (MCP tool schemas are loaded up front instead of through a deferred tool-search step). Variables of a parent Claude Code session (`CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, …) never reach the child. Timeouts kill the whole process group. Because `--tools` lists no `ToolSearch` and `ENABLE_TOOL_SEARCH=false`, every eval session loads MCP schemas up front: past A/B batches measured the upfront regime only (see "Tool surface cost").

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

## Tool surface cost

`atlas-eval agent surface --atlas-bin <catl> [--model <m>] [--out <dir>] [--no-sessions]` measures what the MCP tool surface adds to every request (default model `claude-haiku-4-5-20251001`, `--out` defaults to `eval/reports/surface`; the committed run is `eval/reports/2026-10-12/surface.{md,json}`):

- bytes, no session: it copies `fixtures/acme` to a temp dir, indexes it under a temp `CODE_ATLAS_HOME`, starts `catl mcp`, sends `initialize` and `tools/list`, and records the bytes of the `instructions` string, the whole `tools/list` result and, per tool, name + description, input schema and the whole tool object, largest first, plus characters / 4 as a labelled estimate;
- tokens, measured: single-turn sessions (`Reply with the single word ok.`, `--max-turns 1`, the session flags above, an empty tree, no project instructions) in three loading modes, each with and without Code Atlas, three repeats each: "upfront" (`ENABLE_TOOL_SEARCH=false`), "tool search on but `ToolSearch` tool not allowed (nothing deferred)" (`ENABLE_TOOL_SEARCH=true` with the eval `--tools`, which has no `ToolSearch`), and "tool search on with `ToolSearch` allowed" (`ENABLE_TOOL_SEARCH=true`, `ToolSearch` added to `--tools` and `--allowedTools` for that mode only). The first two modes (12 sessions plus 4 retries at most) run in one invocation; `--extend-from <surface.json>` adds the third (6 sessions plus 2 retries) to a recorded report, resumes a partial extension without repeating recorded sessions, and first checks that its first Code Atlas session has `ToolSearch` in the init event and a smaller prompt than the not-deferred one, otherwise it stops after that one session. It compares `input + cache_creation + cache_read` of the first request (the assistant event, not the aggregated result), because prompt caching moves tokens between the cache fields from one session to the next while their sum stays the prompt size. The first failed session of a run (not logged in, binary missing) stops it and marks the token section `not measured` with the reason;
- the init event of each session is summarised (tool count, atlas tool count, whether a `ToolSearch` tool is present, MCP server status), so what each mode really exposed is reported rather than assumed (the init event lists tool names even when their schemas are deferred, so it cannot show whether schemas were sent; the prompt total can);
- `--render-from <surface.json>` rewrites `surface.md` from a recorded `surface.json` without a session.

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
- Test counts and failing test names are parsed from `cargo test`, jest and vitest summaries, node TAP output and `bun test` (the `N pass` / `N fail` / `N skip` / `N todo` lines of the summary block that ends in `Ran N tests across M files`, and `(fail) name [time]`; lines a test prints itself are not counted). Other runners are compared by exit code only.
- Files read = Read inputs plus the simple forms of `cat`, `head`, `tail`, `sed -n` in Bash (with a leading `cd`); globs, variables, subshells and loops are counted as `unparsed_bash_reads`.
- `relevant_files_missed` and `must_consider_coverage` both credit files that appeared in Code Atlas `MUST READ` sections (their bodies were in the agent's context), besides files read or changed. Information density counts bytes read from relevant files plus Code Atlas response bytes in items located in relevant files; it is a heuristic.
- Atlas accuracy extracts tagged relation lines (`[resolved]`, `[heuristic 0.72]`, `[inferred 0.81 stale]`) and counts `DEGRADED`/stale headers. Relations are reported as `unverified` until a SCIP reconciliation helper is available to the harness.
- Sessions that hit `--max-turns`, time out, end without a result event or report an error are still scored, but each condition's outcome counts (completed, max_turns, reported_error, timed_out, no_result, setup_failed) are reported per task, every metric cell whose sample is smaller than the run count prints its `n`, and a higher atlas rate of timeouts, max-turns hits or missing results is listed as a negative result. Runs without a result event use a cost estimated from per-turn usage, recorded as `cost_source = estimated_from_usage` and counted per task.
- Setup failures are excluded from the statistics and listed individually, split into Code Atlas stages (`atlas-project`, `atlas-index`, `atlas-mcp`) and harness stages (`isolation`, `materialize`, `setup`, `git-base`, `mcp-config`).
- Aggregates are means of per-task means over tasks scored in both conditions only (paired); the headers give the task count. `first_edit_ms`, `session_ms` and `wall_ms` exclude indexing, which is reported as `atlas_index_ms`.
- The dry-run cost is a rough guide from a fixed token profile and list prices, not a bound; `--budget-usd` uses the costs the CLI reports.
