# Headroom: when does an agent reach the files it must change?

Computed from recorded sessions only; no session was run for this report. For every session it gives the turn at which the first file the task must change is reached, the turn at which every such file has been reached, and where the session got its entry point from. Read the headline together with the two sections after it: they say what each number supports and how far these sessions can be generalised.

| batch | date recorded | sessions analysed | excluded |
|---|---|---|---|
| 2026-10-04-seed1 | 2026-10-04 | 44 | none |
| 2026-10-05-directed | 2026-10-04 | 44 | none |

Baseline sessions of all batches are pooled; atlas sessions are grouped by preamble variant.

## Headline

Baseline sessions (no Code Atlas): 44 over 11 tasks. 28 of them belong to tasks with more than one `must_change` file; counting each file once per session there are 176 files. Turn scale; cells are sessions or files out of the total.

| by turn | first file read | first file touched | lines of a file seen | every file read | every file touched | files read (pooled) | files touched (pooled) |
|---|---|---|---|---|---|---|---|
| 1 | 5/44 (11.4%) | 5/44 (11.4%) | 34/44 (77.3%) | 1/44 (2.3%) | 1/44 (2.3%) | 5/176 (2.8%) | 5/176 (2.8%) |
| 2 | 34/44 (77.3%) | 37/44 (84.1%) | 43/44 (97.7%) | 19/44 (43.2%) | 22/44 (50%) | 65/176 (36.9%) | 68/176 (38.6%) |
| 3 | 38/44 (86.4%) | 42/44 (95.5%) | 44/44 (100%) | 27/44 (61.4%) | 30/44 (68.2%) | 82/176 (46.6%) | 100/176 (56.8%) |
| 5 | 41/44 (93.2%) | 44/44 (100%) | 44/44 (100%) | 31/44 (70.5%) | 38/44 (86.4%) | 99/176 (56.3%) | 139/176 (79%) |
| ever | 41/44 (93.2%) | 44/44 (100%) | 44/44 (100%) | 31/44 (70.5%) | 44/44 (100%) | 111/176 (63.1%) | 176/176 (100%) |

- **First file.** First read at turn 2 (1–4, n=41 of 44); first touch at turn 2 (1–4, n=44); first lines seen at turn 1 (1–3, n=44); first edit at turn 4 (2–7, n=44). Before the first read lie 11.1% (0%–27.3%, n=41 of 44) of the turns and 4.5% (0.9%–41%, n=41 of 44) of the time.
- **Every file.** The last `must_change` file is first read at turn 2 (1–5, n=31 of 44) and first touched at turn 2.5 (1–16, n=44); before that first touch lie 20% (0%–83.3%, n=44) of the turns. 111 of 176 files were read at some point and 176 touched.
- **First naming.** A result named a `must_change` file before the agent addressed one at turn 1 (1–1, n=39 of 44); in 5 of 44 sessions the agent addressed one before any result had named one.
- **Entry point.** 34 sessions read a `must_change` file by turn 2. The first one was reached by a search that named it, using an identifier or word written in the prompt: 35 sessions, 29 of them reading one by turn 2; by a search that named it, using an identifier or word written in the injected instructions: 4 sessions, 0 of them reading one by turn 2; by opening it with no earlier naming, its path containing an identifier or word written in the prompt: 5 sessions, 5 of them reading one by turn 2.
- **Per task** (11 tasks; a task counts when every baseline session of it has the event and their median turn is within the limit). First file read: ≤ 1: 1, ≤ 2: 8, ≤ 3: 10, ≤ 5: 10. Every file read: ≤ 1: 0, ≤ 2: 5, ≤ 3: 7, ≤ 5: 7. Every file touched: ≤ 1: 0, ≤ 2: 6, ≤ 3: 7, ≤ 5: 9.

Note on scales: in 88 of 88 sessions the `num_turns` of the result event equals the tool-call count plus one, so the "turns" of the agent report are tool calls, not model responses. Baseline sessions here took 10 (4–18, n=44) turns and 11.5 (5–18, n=44) tool calls.

## What each measure supports

- **First-file rows** say when a session has an entry point: one file of the change set has been read or touched. They say nothing about the rest of the change set. For a task with one `must_change` file they are the whole answer; for the others they are the lower end.
- **Every-file rows** say when the whole change set has been located: the turn at which the last `must_change` file is first read, or first touched. **Pooled file rows** count files instead of sessions, so a task with many files weighs more.
- **Seen** is wider than read: it also counts lines of a file that came back in a search result. Where a session edits from such lines it never issues a read, and the read rows understate what it saw.
- **Where the entry point comes from** limits what an early first read can be credited to. When the first search uses a word the prompt wrote down, the early read shows that the task hands over its entry point, not that the agent located code quickly.
- None of these rows shows whether a context tool would shorten a session, and none shows that it could not. They show how much of a session lies before the entry point and before the complete change set in sessions that had no such tool.

## Limits

- **Few independent observations.** The 44 baseline sessions are 11 tasks with 4 runs each of the same prompt. Runs of one task are repetitions that mostly do the same thing, so the unit to read is the task, and there are 11 of them.
- **Same authors.** The tasks under `eval/tasks`, their prompts and their `must_change` lists were written by the developers of Code Atlas. A prompt written by someone who knows the fix tends to name the code involved.
- **Repositories.** 1 on fixture `acme` of this repository, 0 of them with project instructions injected into every session; 4 on https://github.com/honojs/hono.git, 0 of them with project instructions injected into every session; 6 on this repository, 6 of them with project instructions injected into every session. On this repository the injected instructions are an architecture guide that names crates, files and identifiers. A repository given by URL is public, so the model may know its layout from training.
- **So the result describes these tasks.** It does not carry over to tasks whose prompts do not name code, to repositories without such a guide, or to code the model has not seen. An early first read here is what such a prompt yields.
- **Detection.** Touches are recognised from a fixed set of forms; the last two sections say which, and what was missed in these sessions.

## Where the entry point comes from

Terms taken from the prompt and from the injected project instructions, and what one fixed-string grep for each returns at the base commit. A cell reads `terms that return a must_change file of terms checked`, then the most selective of them as `term (must_change files among the files returned of files returned)`.

| task | repository | `must_change` files | prompt | injected instructions |
|---|---|---|---|---|
| t00-smoke | fixture `acme` of this repository | 1 | 1 of 9: `User` (1 of 13 files) | none injected |
| t01-cwd-relative-targets | this repository | 1 | 10 of 39: `impact` (1 of 22 files), `explain` (1 of 24 files), `snippet` (1 of 36 files), `similar` (1 of 68 files), `query` (1 of 105 files) | 20 of 336: `stale` (1 of 42 files), `Unresolved` (1 of 48 files), `Engine` (1 of 56 files), `open` (1 of 71 files), `Project` (1 of 74 files) |
| t02-typed-degraded-cause | this repository | 6 | 17 of 32: `index update failed: {error}` (1 of 1 files), `some files or repositories could not be indexed` (1 of 1 files), `ProjectConfigChanged` (2 of 4 files), `project configuration changed; reopen the engine` (2 of 4 files), `FreshnessReport::Degraded` (5 of 5 files) | 110 of 336: `header.files_failed_analysis` (2 of 3 files), `engine:meta` (1 of 3 files), `header.scip` (1 of 3 files), `scip.lagging` (1 of 3 files), `FreshnessReport::Fresh` (2 of 4 files) |
| t03-nested-router-links | this repository | 1 | 7 of 35: `Router` (1 of 22 files), `routes` (1 of 41 files), `express` (1 of 111 files), `json` (1 of 196 files), `import` (1 of 228 files) | 44 of 336: `mounted_on` (1 of 3 files), `mounted_on_scope` (1 of 3 files), `cross-file` (1 of 4 files), `mount_prefix` (1 of 4 files), `route_scope` (1 of 5 files) |
| t04-detector-import-gates | this repository | 5 | 8 of 21: `atlas_core::ImportFact` (2 of 2 files), `ImportFact` (5 of 15 files), `require` (1 of 77 files), `atlas_core` (5 of 217 files), `import` (5 of 228 files) | 79 of 336: `[expr]` (1 of 3 files), `dynamic_url` (1 of 3 files), `http.dynamic_url` (1 of 3 files), `mounted_on` (1 of 3 files), `mounted_on_scope` (1 of 3 files) |
| t05-mcp-dependency-tools | this repository | 2 | 6 of 19: `atlas_callees` (2 of 2 files), `atlas_callers` (2 of 3 files), `depth` (2 of 62 files), `query` (2 of 105 files), `target` (2 of 148 files) | 40 of 336: `non-empty` (2 of 3 files), `ensure_fresh` (1 of 15 files), `ensure` (1 of 19 files), `router` (1 of 22 files), `structure` (1 of 27 files) |
| t06-repo-file-size-limit | this repository | 11 | 11 of 15: `[project.settings]` (1 of 1 files), `project.settings` (1 of 2 files), `projects.toml` (1 of 2 files), `max_file_bytes` (2 of 9 files), `TypeScript` (1 of 10 files) | 106 of 336: `Engine::ensure_fresh` (1 of 2 files), `[workspace.dependencies]` (1 of 3 files), `similarity_persist_idle` (1 of 3 files), `skip_serializing_if` (1 of 3 files), `workspace.dependencies` (1 of 3 files) |
| t07-static-precompressed-quality | https://github.com/honojs/hono.git | 1 | 8 of 30: `precompressed` (1 of 2 files), `Content-Encoding` (1 of 9 files), `Accept-Encoding` (1 of 12 files), `Vary` (1 of 12 files), `gzip` (1 of 14 files) | none injected |
| t08-cookie-samesite-default | https://github.com/honojs/hono.git | 8 | 21 of 39: `deleteCookie` (2 of 2 files), `generateCookie` (2 of 2 files), `generateSignedCookie` (2 of 2 files), `setSignedCookie` (2 of 3 files), `SameSite` (1 of 3 files) | none injected |
| t09-lambda-text-content-types | https://github.com/honojs/hono.git | 4 | 16 of 47: `bodyEncoding` (2 of 5 files), `bodyEncoding: 'base64'` (2 of 5 files), `@hono/lambda-edge` (1 of 5 files), `hono/lambda-edge` (1 of 5 files), `@hono/aws-lambda` (1 of 6 files) | none injected |
| t10-auth-challenge-quoting | https://github.com/honojs/hono.git | 4 | 13 of 29: `quoted-string` (2 of 2 files), `wwwAuthenticateHeader` (1 of 2 files), `bearerAuth` (1 of 3 files), `invalid_token` (3 of 5 files), `error_description` (2 of 5 files) | none injected |

Prompts with at least one term that returns a `must_change` file: 11 of 11. Tasks whose injected instructions have one: 6 of 6.

How each session reached its first `must_change` file. Cells are `sessions (of which read one by turn 2)`.

| the first file was reached by | baseline (44 sessions, 34 reading by turn 2) | atlas (standard) (22 sessions, 16 reading by turn 2) | atlas (directed) (22 sessions, 16 reading by turn 2) |
|---|---|---|---|
| a search that named it, using an identifier or word written in the prompt | 35 (29) | 16 (12) | 18 (15) |
| a search that named it, using an identifier or word written in the injected instructions | 4 (0) | 2 (0) | 2 (1) |
| opening it with no earlier naming, its path containing an identifier or word written in the prompt | 5 (5) | 4 (4) | 2 (0) |
| no `must_change` file named or addressed | 0 (0) | 0 (0) | 0 (0) |

## Definitions

- **Session.** One recorded run (`<task>.<condition>.r<i>`) that completed and is not contaminated, the same filter `agent report` applies.
- **Turn.** One model response: one distinct assistant `message.id` in `transcript.jsonl`. A response may issue several tool calls in parallel; all of them carry that turn's ordinal. Turn 1 is the first response after the prompt, and the final answer is a turn too. Results of calls issued in turn N reach the model before turn N+1.
- **Tool-call ordinal.** The 1-based position of a tool call in the transcript. The `num_turns` that `agent report` shows as "turns" is on this scale, not on the turn scale (see the note under the headline).
- **Event ordinals.** An event belongs to the call that issued it. "By turn N" means that call was issued in turn N or earlier, so the model has its result from turn N+1 on.
- **(a) Named.** The file's path appears in the result of a Grep, Glob, Bash or MCP call whose own input did not name the file. Results of Read, Edit and Write are not searched (they are file content or echo the path the agent gave). A path in a result counts when it resolves to the file from the tree root, from a working directory of the shell during that call, from a directory passed to `ls`, or from a `dir:` header line of the output. Per file, a naming is reported only if it comes before the agent first addressed that file; for a file set, only if it comes before the agent first addressed any file of the set.
- **Shown.** A result carries lines of the file: the path is followed by a line number (`path:12:` or `path-12-`, the form of grep output with content), or the search was addressed to the file and asked for content. For Code Atlas results every located item counts, whether or not its body was included.
- **Searched.** A search addressed to that one file: the Grep tool with `path` equal to the file, or `grep`, `egrep`, `fgrep`, `rg`, `ag`, `ack` or `git grep` in Bash with the file written as an operand. A search over a directory, or over a glob that the shell expands, is not addressed to one file; it can only name or show it.
- **(b) Read.** A Read call that did not fail, or `cat`, `head`, `tail` or `sed -n` in Bash with the file as an operand, written out or matched by a glob. These are the forms `agent report` counts as files read (`agent/bash.rs`); unlike that metric, the shell's working directory is followed across calls here.
- **Seen.** The earliest of shown and read: the first time any line of the file reached the model.
- **(c) Edited.** An Edit, Write, MultiEdit or NotebookEdit call that did not fail, or in Bash: `sed -i` (not counted when sed rejected the command, which BSD sed does with GNU syntax), `perl -i … -e`, output redirected or piped through `tee` into the file, or a Python script (`python3 - <<EOF` or `python3 -c`) that writes files and names the file in a string literal.
- **Touched.** The earliest of searched, read and edited: the first call the agent addressed to the file in a recognised form. A file with no touch has no recognised touch; the session may still have changed it in a form this analysis does not recognise, and the detection checks list those files.
- **Mentioned.** The call's input names the file (an operand that resolves to it, or its shortest path suffix that is unique in the base tree) in a form not listed above, for example as a test-runner argument, in the arguments of an MCP call, or in a call that failed. It is a diagnostic and never counts as a touch. Addressed is the earliest of mentioned and touched.
- **File sets.** `must_change` and `relevant_files` from `eval/tasks/<task>/task.toml`. A file absent from the base tree (the task creates it) has nothing to find: it is listed separately and excluded from every statistic. The base tree is the archived tree of the run with the stripped paths removed and the task's `base.patch` applied.
- **First file and every file.** For a file set, a first event is the earliest over its files. "Every file read" is defined only when each file of the set was read, and is the first read of the last of them; "every file touched" likewise, so it is the turn of the last file's first touch. Pooled file counts take each file of each session once.
- **Share of the session before an event.** By turns: (turn − 1) / turns; by tool calls: (call − 1) / tool calls; by time: the transcript offset of the call divided by the session duration.
- **Terms that reach a file.** From the prompt of a baseline session (an atlas prompt carries the preamble as well), and separately from the project instructions the harness injects with `--append-system-prompt`, these terms are taken: each code span, each identifier-shaped token (an underscore, an inner capital, `::`, `/` or a hyphen) and its parts, each word of four or more characters inside a code span or block, and each double-quoted phrase. A term reaches a file when the file contains it at the base commit as a fixed string, the result of one `grep -F`. Reported per task: how many terms were checked, how many return at least one `must_change` file, and for the most selective ones how many files they return.
- **Entry point of a session.** The call that first named a `must_change` file, or, when the agent addressed one before any result named it, that first address. Its text is the search pattern (Grep `pattern` and `glob`, the Glob pattern, the pattern of a `grep` in Bash or else the whole command, the arguments of an MCP call) or the path the agent opened. The text is compared with the prompt and with the injected instructions, case-insensitively, in this order: a whole word or identifier (letters, digits, underscore; at least four characters; language keywords and a few generic words such as `index` or `tests` excluded) shared with the prompt; with the instructions; a part of such a word (split at underscores and inner capitals) shared with the prompt; with the instructions; neither. The first that holds is the source. This is a lexical comparison, so it shows that the words were available, not what the model did with them.

## Overall, `must_change`

Cells are `median (min–max, n)` over sessions; `n` is given as `n of N` when some sessions have no such event. Shares are sessions, or files where the row says pooled.

| measure | baseline (44 sessions) | atlas (standard) (22 sessions) | atlas (directed) (22 sessions) |
|---|---|---|---|
| first file named in a result, turn | 1 (1–1, n=39 of 44) | 1 (1–1, n=18 of 22) | 1 (1–1, n=20 of 22) |
| first file named in a result, tool call | 1 (1–1, n=39 of 44) | 1 (1–1, n=18 of 22) | 1 (1–2, n=20 of 22) |
| first lines of a file seen (read, or shown in a result), turn | 1 (1–3, n=44) | 1 (1–3, n=22) | 1 (1–3, n=22) |
| first search addressed to a file, turn | 4 (2–9, n=24 of 44) | 3 (2–6, n=13 of 22) | 4 (2–8, n=10 of 22) |
| first search addressed to a file, tool call | 5 (3–14, n=24 of 44) | 4 (2–6, n=13 of 22) | 8 (3–20, n=10 of 22) |
| first file read, turn | 2 (1–4, n=41 of 44) | 2 (1–4, n=19 of 22) | 2 (1–4, n=21 of 22) |
| first file read, tool call | 2 (1–5, n=41 of 44) | 2 (1–4, n=19 of 22) | 3 (2–6, n=21 of 22) |
| first file touched (search, read or edit), turn | 2 (1–4, n=44) | 2 (1–3, n=22) | 2 (1–4, n=22) |
| first file touched (search, read or edit), tool call | 2 (1–5, n=44) | 2 (1–3, n=22) | 3 (2–6, n=22) |
| first file edited, turn | 4 (2–7, n=44) | 4 (2–9, n=21 of 22) | 4 (2–6, n=21 of 22) |
| first file edited, tool call | 5 (3–12, n=44) | 6 (2–9, n=21 of 22) | 8 (3–10, n=21 of 22) |
| every file read, turn of the last one's first read | 2 (1–5, n=31 of 44) | 2 (2–4, n=15 of 22) | 2 (2–4, n=14 of 22) |
| every file read, tool call | 3 (1–9, n=31 of 44) | 4 (2–7, n=15 of 22) | 4 (3–24, n=14 of 22) |
| every file touched, turn of the last one's first touch | 2.5 (1–16, n=44) | 2.5 (2–14, n=20 of 22) | 3 (2–12, n=21 of 22) |
| every file touched, tool call | 4 (1–16, n=44) | 4 (2–14, n=20 of 22) | 6 (3–24, n=21 of 22) |
| share of the session before the first read, turns | 11.1% (0%–27.3%, n=41 of 44) | 14.3% (0%–27.3%, n=19 of 22) | 12.5% (0%–37.5%, n=21 of 22) |
| share of the session before the first read, tool calls | 11.1% (0%–28.6%, n=41 of 44) | 12.5% (0%–30%, n=19 of 22) | 16.7% (6.1%–55.6%, n=21 of 22) |
| share of the session before the first read, time | 4.5% (0.9%–41%, n=41 of 44) | 5.2% (1.6%–31.2%, n=19 of 22) | 6.1% (1.3%–33.1%, n=21 of 22) |
| share of the session before the last file's first touch, turns | 20% (0%–83.3%, n=44) | 19.1% (9.1%–68.4%, n=20 of 22) | 25% (9.1%–73.3%, n=21 of 22) |
| files of the set that exist at base | 4 (1–11, n=44) | 4 (1–11, n=22) | 4 (1–11, n=22) |
| first file read by turn 1 | 5/44 (11.4%) | 2/22 (9.1%) | 2/22 (9.1%) |
| first file read by turn 2 | 34/44 (77.3%) | 16/22 (72.7%) | 16/22 (72.7%) |
| first file read by turn 3 | 38/44 (86.4%) | 17/22 (77.3%) | 20/22 (90.9%) |
| first file read by turn 5 | 41/44 (93.2%) | 19/22 (86.4%) | 21/22 (95.5%) |
| first file touched by turn 1 | 5/44 (11.4%) | 2/22 (9.1%) | 2/22 (9.1%) |
| first file touched by turn 2 | 37/44 (84.1%) | 19/22 (86.4%) | 17/22 (77.3%) |
| first file touched by turn 3 | 42/44 (95.5%) | 22/22 (100%) | 21/22 (95.5%) |
| first file touched by turn 5 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| lines of a file seen by turn 1 | 34/44 (77.3%) | 17/22 (77.3%) | 17/22 (77.3%) |
| lines of a file seen by turn 2 | 43/44 (97.7%) | 21/22 (95.5%) | 20/22 (90.9%) |
| lines of a file seen by turn 3 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| lines of a file seen by turn 5 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| every file read by turn 1 | 1/44 (2.3%) | 0/22 (0%) | 0/22 (0%) |
| every file read by turn 2 | 19/44 (43.2%) | 10/22 (45.5%) | 8/22 (36.4%) |
| every file read by turn 3 | 27/44 (61.4%) | 13/22 (59.1%) | 12/22 (54.5%) |
| every file read by turn 5 | 31/44 (70.5%) | 15/22 (68.2%) | 14/22 (63.6%) |
| every file touched by turn 1 | 1/44 (2.3%) | 0/22 (0%) | 0/22 (0%) |
| every file touched by turn 2 | 22/44 (50%) | 10/22 (45.5%) | 9/22 (40.9%) |
| every file touched by turn 3 | 30/44 (68.2%) | 15/22 (68.2%) | 13/22 (59.1%) |
| every file touched by turn 5 | 38/44 (86.4%) | 18/22 (81.8%) | 18/22 (81.8%) |
| files read, pooled by turn 1 | 5/176 (2.8%) | 2/88 (2.3%) | 2/88 (2.3%) |
| files read, pooled by turn 2 | 65/176 (36.9%) | 32/88 (36.4%) | 29/88 (33%) |
| files read, pooled by turn 3 | 82/176 (46.6%) | 41/88 (46.6%) | 38/88 (43.2%) |
| files read, pooled by turn 5 | 99/176 (56.3%) | 46/88 (52.3%) | 45/88 (51.1%) |
| files touched, pooled by turn 1 | 5/176 (2.8%) | 2/88 (2.3%) | 2/88 (2.3%) |
| files touched, pooled by turn 2 | 68/176 (38.6%) | 36/88 (40.9%) | 30/88 (34.1%) |
| files touched, pooled by turn 3 | 100/176 (56.8%) | 51/88 (58%) | 43/88 (48.9%) |
| files touched, pooled by turn 5 | 139/176 (79%) | 65/88 (73.9%) | 67/88 (76.1%) |
| files read at some point, pooled | 111/176 (63.1%) | 55/88 (62.5%) | 53/88 (60.2%) |
| files touched at some point, pooled | 176/176 (100%) | 78/88 (88.6%) | 86/88 (97.7%) |
| sessions with more than one file in the set | 28/44 (63.6%) | 14/22 (63.6%) | 14/22 (63.6%) |
| sessions with no recognised read of any file | 3/44 (6.8%) | 3/22 (13.6%) | 1/22 (4.5%) |
| sessions with no recognised touch of any file | 0/44 (0%) | 0/22 (0%) | 0/22 (0%) |
| sessions addressing a file before any result named one | 5/44 (11.4%) | 4/22 (18.2%) | 2/22 (9.1%) |
| sessions editing a file with no earlier read of it | 14/44 (31.8%) | 6/22 (27.3%) | 7/22 (31.8%) |
| sessions editing a file with no earlier read or search of it | 13/44 (29.5%) | 4/22 (18.2%) | 6/22 (27.3%) |
| sessions with at least one file without a recognised touch | 0/44 (0%) | 2/22 (9.1%) | 1/22 (4.5%) |
| total turns | 10 (4–18, n=44) | 9 (5–19, n=22) | 8.5 (6–15, n=22) |
| total tool calls | 11.5 (5–18, n=44) | 10 (5–19, n=22) | 13 (6–33, n=22) |

## Overall, `relevant_files`

| measure | baseline (44 sessions) | atlas (standard) (22 sessions) | atlas (directed) (22 sessions) |
|---|---|---|---|
| first file named in a result, turn | 1 (1–1, n=39 of 44) | 1 (1–1, n=20 of 22) | 1 (1–1, n=22) |
| first file named in a result, tool call | 1 (1–1, n=39 of 44) | 1 (1–1, n=20 of 22) | 1 (1–2, n=22) |
| first lines of a file seen (read, or shown in a result), turn | 1 (1–2, n=44) | 1 (1–2, n=22) | 1 (1–2, n=22) |
| first search addressed to a file, turn | 3 (2–9, n=35 of 44) | 3 (2–4, n=19 of 22) | 4 (2–8, n=13 of 22) |
| first search addressed to a file, tool call | 4 (2–11, n=35 of 44) | 4 (2–6, n=19 of 22) | 8 (3–20, n=13 of 22) |
| first file read, turn | 2 (1–5, n=44) | 2 (1–3, n=21 of 22) | 2 (1–4, n=22) |
| first file read, tool call | 2 (1–5, n=44) | 2 (1–3, n=21 of 22) | 3 (2–4, n=22) |
| first file touched (search, read or edit), turn | 2 (1–4, n=44) | 2 (1–3, n=22) | 2 (1–3, n=22) |
| first file touched (search, read or edit), tool call | 2 (1–5, n=44) | 2 (1–3, n=22) | 3 (2–4, n=22) |
| first file edited, turn | 4 (2–7, n=44) | 4 (2–9, n=21 of 22) | 4 (2–6, n=21 of 22) |
| first file edited, tool call | 5 (3–11, n=44) | 6 (2–9, n=21 of 22) | 8 (3–10, n=21 of 22) |
| every file read, turn of the last one's first read | 2 (2–6, n=7 of 44) | 5.5 (5–6, n=2 of 22) | – (n=0 of 22) |
| every file read, tool call | 3 (3–8, n=7 of 44) | 8.5 (8–9, n=2 of 22) | – (n=0 of 22) |
| every file touched, turn of the last one's first touch | 5 (2–16, n=15 of 44) | 5.5 (5–10, n=4 of 22) | 5 (5–5, n=2 of 22) |
| every file touched, tool call | 8 (3–16, n=15 of 44) | 8 (6–11, n=4 of 22) | 7.5 (5–10, n=2 of 22) |
| share of the session before the first read, turns | 10.6% (0%–37.5%, n=44) | 11.1% (0%–28.6%, n=21 of 22) | 12.5% (0%–37.5%, n=22) |
| share of the session before the first read, tool calls | 10.6% (0%–42.9%, n=44) | 11.1% (0%–33.3%, n=21 of 22) | 16.7% (6.1%–42.9%, n=22) |
| share of the session before the first read, time | 6.3% (0.9%–41.1%, n=44) | 4.6% (1%–31.2%, n=21 of 22) | 6.5% (1.3%–37.9%, n=22) |
| share of the session before the last file's first touch, turns | 50% (20%–83.3%, n=15 of 44) | 50% (40%–56.3%, n=4 of 22) | 45% (40%–50%, n=2 of 22) |
| files of the set that exist at base | 7 (2–11, n=44) | 7 (2–11, n=22) | 7 (2–11, n=22) |
| first file read by turn 1 | 5/44 (11.4%) | 3/22 (13.6%) | 2/22 (9.1%) |
| first file read by turn 2 | 38/44 (86.4%) | 20/22 (90.9%) | 18/22 (81.8%) |
| first file read by turn 3 | 39/44 (88.6%) | 21/22 (95.5%) | 21/22 (95.5%) |
| first file read by turn 5 | 44/44 (100%) | 21/22 (95.5%) | 22/22 (100%) |
| first file touched by turn 1 | 5/44 (11.4%) | 3/22 (13.6%) | 2/22 (9.1%) |
| first file touched by turn 2 | 39/44 (88.6%) | 21/22 (95.5%) | 19/22 (86.4%) |
| first file touched by turn 3 | 42/44 (95.5%) | 22/22 (100%) | 22/22 (100%) |
| first file touched by turn 5 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| lines of a file seen by turn 1 | 34/44 (77.3%) | 19/22 (86.4%) | 21/22 (95.5%) |
| lines of a file seen by turn 2 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| lines of a file seen by turn 3 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| lines of a file seen by turn 5 | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| every file read by turn 1 | 0/44 (0%) | 0/22 (0%) | 0/22 (0%) |
| every file read by turn 2 | 4/44 (9.1%) | 0/22 (0%) | 0/22 (0%) |
| every file read by turn 3 | 4/44 (9.1%) | 0/22 (0%) | 0/22 (0%) |
| every file read by turn 5 | 6/44 (13.6%) | 1/22 (4.5%) | 0/22 (0%) |
| every file touched by turn 1 | 0/44 (0%) | 0/22 (0%) | 0/22 (0%) |
| every file touched by turn 2 | 4/44 (9.1%) | 0/22 (0%) | 0/22 (0%) |
| every file touched by turn 3 | 4/44 (9.1%) | 0/22 (0%) | 0/22 (0%) |
| every file touched by turn 5 | 9/44 (20.5%) | 2/22 (9.1%) | 2/22 (9.1%) |
| files read, pooled by turn 1 | 5/300 (1.7%) | 3/150 (2%) | 2/150 (1.3%) |
| files read, pooled by turn 2 | 82/300 (27.3%) | 44/150 (29.3%) | 34/150 (22.7%) |
| files read, pooled by turn 3 | 109/300 (36.3%) | 58/150 (38.7%) | 50/150 (33.3%) |
| files read, pooled by turn 5 | 154/300 (51.3%) | 72/150 (48%) | 69/150 (46%) |
| files touched, pooled by turn 1 | 5/300 (1.7%) | 3/150 (2%) | 2/150 (1.3%) |
| files touched, pooled by turn 2 | 92/300 (30.7%) | 50/150 (33.3%) | 37/150 (24.7%) |
| files touched, pooled by turn 3 | 134/300 (44.7%) | 76/150 (50.7%) | 56/150 (37.3%) |
| files touched, pooled by turn 5 | 197/300 (65.7%) | 93/150 (62%) | 93/150 (62%) |
| files read at some point, pooled | 178/300 (59.3%) | 85/150 (56.7%) | 81/150 (54%) |
| files touched at some point, pooled | 233/300 (77.7%) | 112/150 (74.7%) | 105/150 (70%) |
| sessions with more than one file in the set | 44/44 (100%) | 22/22 (100%) | 22/22 (100%) |
| sessions with no recognised read of any file | 0/44 (0%) | 1/22 (4.5%) | 0/22 (0%) |
| sessions with no recognised touch of any file | 0/44 (0%) | 0/22 (0%) | 0/22 (0%) |
| sessions addressing a file before any result named one | 5/44 (11.4%) | 2/22 (9.1%) | 0/22 (0%) |
| sessions editing a file with no earlier read of it | 20/44 (45.5%) | 9/22 (40.9%) | 10/22 (45.5%) |
| sessions editing a file with no earlier read or search of it | 14/44 (31.8%) | 4/22 (18.2%) | 7/22 (31.8%) |
| sessions with at least one file without a recognised touch | 29/44 (65.9%) | 18/22 (81.8%) | 20/22 (90.9%) |
| total turns | 10 (4–18, n=44) | 9 (5–19, n=22) | 8.5 (6–15, n=22) |
| total tool calls | 11.5 (5–18, n=44) | 10 (5–19, n=22) | 13 (6–33, n=22) |

## Per task

Cells are `median (min–max, n)` over the sessions of the task in that group.

### Baseline, `must_change`: first file and every file

| task | files | sessions | first file read, turn | every file read, turn | every file touched, turn (the last file's first touch) | total turns | files read / touched at some point |
|---|---|---|---|---|---|---|---|
| t00-smoke | 1 | 4 | 2 (2–2, n=4) | 2 (2–2, n=4) | 2 (2–2, n=4) | 4.5 (4–5, n=4) | 4 / 4 of 4 |
| t01-cwd-relative-targets | 1 | 4 | 2 (2–2, n=4) | 2 (2–2, n=4) | 2 (2–2, n=4) | 11.5 (10–15, n=4) | 4 / 4 of 4 |
| t02-typed-degraded-cause | 6 | 4 | 2 (2–2, n=4) | 5 (4–5, n=3 of 4) | 4.5 (4–5, n=4) | 8 (7–11, n=4) | 23 / 24 of 24 |
| t03-nested-router-links | 1 | 4 | 3 (3–4, n=4) | 3 (3–4, n=4) | 2 (2–3, n=4) | 10.5 (9–11, n=4) | 4 / 4 of 4 |
| t04-detector-import-gates | 5 | 4 | 2 (2–2, n=4) | 2 (2–2, n=4) | 2 (2–2, n=4) | 8 (6–10, n=4) | 20 / 20 of 20 |
| t05-mcp-dependency-tools | 2 | 4 | 2 (2–2, n=4) | 3 (3–3, n=4) | 3 (3–3, n=4) | 9.5 (8–13, n=4) | 8 / 8 of 8 |
| t06-repo-file-size-limit | 11 | 4 | 2.5 (2–4, n=4) | – (n=0 of 4) | 5.5 (4–8, n=4) | 12 (10–14, n=4) | 5 / 44 of 44 |
| t07-static-precompressed-quality | 1 | 4 | 2 (1–2, n=4) | 2 (1–2, n=4) | 2 (1–2, n=4) | 8 (8–8, n=4) | 4 / 4 of 4 |
| t08-cookie-samesite-default | 8 | 4 | 1 (1–1, n=4) | – (n=0 of 4) | 12.5 (11–16, n=4) | 15.5 (15–18, n=4) | 21 / 32 of 32 |
| t09-lambda-text-content-types | 4 | 4 | 4 (4–4, n=1 of 4) | – (n=0 of 4) | 3.5 (3–5, n=4) | 10 (8–12, n=4) | 2 / 16 of 16 |
| t10-auth-challenge-quoting | 4 | 4 | 2 (2–2, n=4) | 2 (2–3, n=4) | 2 (2–3, n=4) | 10 (8–11, n=4) | 16 / 16 of 16 |

### First read of a `must_change` file, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t02-typed-degraded-cause | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t03-nested-router-links | 3 (3–4, n=4) | 4 (4–4, n=1 of 2) | 2.5 (2–3, n=2) |
| t04-detector-import-gates | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t05-mcp-dependency-tools | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t06-repo-file-size-limit | 2.5 (2–4, n=4) | 3.5 (3–4, n=2) | 2.5 (2–3, n=2) |
| t07-static-precompressed-quality | 2 (1–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t08-cookie-samesite-default | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t09-lambda-text-content-types | 4 (4–4, n=1 of 4) | – (n=0 of 2) | 4 (4–4, n=1 of 2) |
| t10-auth-challenge-quoting | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |

### Every `must_change` file read, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t02-typed-degraded-cause | 5 (4–5, n=3 of 4) | 3.5 (3–4, n=2) | 4 (4–4, n=2) |
| t03-nested-router-links | 3 (3–4, n=4) | 4 (4–4, n=1 of 2) | 2.5 (2–3, n=2) |
| t04-detector-import-gates | 2 (2–2, n=4) | 2 (2–2, n=2) | – (n=0 of 2) |
| t05-mcp-dependency-tools | 3 (3–3, n=4) | 3 (3–3, n=2) | 2.5 (2–3, n=2) |
| t06-repo-file-size-limit | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t07-static-precompressed-quality | 2 (1–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t08-cookie-samesite-default | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t09-lambda-text-content-types | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t10-auth-challenge-quoting | 2 (2–3, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |

### Every `must_change` file touched, turn (the last file's first touch)

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t02-typed-degraded-cause | 4.5 (4–5, n=4) | 3.5 (3–4, n=2) | 4 (4–4, n=2) |
| t03-nested-router-links | 2 (2–3, n=4) | 3 (3–3, n=2) | 2 (2–2, n=2) |
| t04-detector-import-gates | 2 (2–2, n=4) | 2 (2–2, n=2) | 4 (4–4, n=2) |
| t05-mcp-dependency-tools | 3 (3–3, n=4) | 3 (3–3, n=2) | 2.5 (2–3, n=2) |
| t06-repo-file-size-limit | 5.5 (4–8, n=4) | 5 (5–5, n=1 of 2) | 5.5 (5–6, n=2) |
| t07-static-precompressed-quality | 2 (1–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t08-cookie-samesite-default | 12.5 (11–16, n=4) | 12 (10–14, n=2) | 11.5 (11–12, n=2) |
| t09-lambda-text-content-types | 3.5 (3–5, n=4) | 4 (4–4, n=1 of 2) | 3 (3–3, n=1 of 2) |
| t10-auth-challenge-quoting | 2 (2–3, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |

### Share of the turns before the last `must_change` file's first touch

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 22.5% (20%–25%, n=4) | 17.1% (14.3%–20%, n=2) | 28.6% (28.6%–28.6%, n=2) |
| t01-cwd-relative-targets | 8.7% (6.7%–10%, n=4) | 10.1% (9.1%–11.1%, n=2) | 12.9% (9.1%–16.7%, n=2) |
| t02-typed-degraded-cause | 42.9% (36.4%–44.4%, n=4) | 27.5% (25%–30%, n=2) | 35.4% (33.3%–37.5%, n=2) |
| t03-nested-router-links | 10.6% (9.1%–18.2%, n=4) | 16.8% (15.4%–18.2%, n=2) | 9.5% (9.1%–10%, n=2) |
| t04-detector-import-gates | 13.3% (10%–16.7%, n=4) | 13.4% (12.5%–14.3%, n=2) | 43.8% (37.5%–50%, n=2) |
| t05-mcp-dependency-tools | 21.1% (15.4%–25%, n=4) | 34.3% (28.6%–40%, n=2) | 14.6% (12.5%–16.7%, n=2) |
| t06-repo-file-size-limit | 37.4% (30%–50%, n=4) | 36.4% (36.4%–36.4%, n=1 of 2) | 44.9% (44.4%–45.5%, n=2) |
| t07-static-precompressed-quality | 12.5% (0%–12.5%, n=4) | 17.1% (14.3%–20%, n=2) | 14.3% (14.3%–14.3%, n=2) |
| t08-cookie-samesite-default | 74% (66.7%–83.3%, n=4) | 62.3% (56.3%–68.4%, n=2) | 70% (66.7%–73.3%, n=2) |
| t09-lambda-text-content-types | 26.1% (22.2%–33.3%, n=4) | 33.3% (33.3%–33.3%, n=1 of 2) | 25% (25%–25%, n=1 of 2) |
| t10-auth-challenge-quoting | 10% (9.1%–25%, n=4) | 10.6% (10%–11.1%, n=2) | 10.6% (10%–11.1%, n=2) |

### First result naming a `must_change` file, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 1 (1–1, n=4) | – (n=0 of 2) | – (n=0 of 2) |
| t01-cwd-relative-targets | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t02-typed-degraded-cause | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t03-nested-router-links | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t04-detector-import-gates | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t05-mcp-dependency-tools | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t06-repo-file-size-limit | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t07-static-precompressed-quality | 1 (1–1, n=3 of 4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t08-cookie-samesite-default | – (n=0 of 4) | – (n=0 of 2) | 1 (1–1, n=2) |
| t09-lambda-text-content-types | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t10-auth-challenge-quoting | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |

### First lines of a `must_change` file seen, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 1 (1–1, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t02-typed-degraded-cause | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t03-nested-router-links | 2 (2–3, n=4) | 2 (1–3, n=2) | 1.5 (1–2, n=2) |
| t04-detector-import-gates | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t05-mcp-dependency-tools | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t06-repo-file-size-limit | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t07-static-precompressed-quality | 1.5 (1–2, n=4) | 1 (1–1, n=2) | 1.5 (1–2, n=2) |
| t08-cookie-samesite-default | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t09-lambda-text-content-types | 2 (2–2, n=4) | 2 (2–2, n=2) | 1.5 (1–2, n=2) |
| t10-auth-challenge-quoting | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |

### First touch of a `must_change` file, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t02-typed-degraded-cause | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t03-nested-router-links | 2 (2–3, n=4) | 3 (3–3, n=2) | 2 (2–2, n=2) |
| t04-detector-import-gates | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t05-mcp-dependency-tools | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t06-repo-file-size-limit | 2.5 (2–4, n=4) | 2 (2–2, n=2) | 2.5 (2–3, n=2) |
| t07-static-precompressed-quality | 2 (1–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t08-cookie-samesite-default | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t09-lambda-text-content-types | 3 (3–4, n=4) | 2.5 (2–3, n=2) | 3.5 (3–4, n=2) |
| t10-auth-challenge-quoting | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |

### First edit of a `must_change` file, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 3 (3–3, n=4) | 5 (4–6, n=2) | 4 (4–4, n=2) |
| t01-cwd-relative-targets | 3 (3–4, n=4) | 3.5 (3–4, n=2) | 3 (3–3, n=2) |
| t02-typed-degraded-cause | 3.5 (3–4, n=4) | 3.5 (3–4, n=2) | 3 (3–3, n=2) |
| t03-nested-router-links | 5 (4–6, n=4) | 7.5 (6–9, n=2) | 5 (4–6, n=2) |
| t04-detector-import-gates | 4 (4–4, n=4) | 4 (4–4, n=2) | 4 (4–4, n=2) |
| t05-mcp-dependency-tools | 6 (5–7, n=4) | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| t06-repo-file-size-limit | 5 (4–6, n=4) | 5 (4–6, n=2) | 5.5 (5–6, n=2) |
| t07-static-precompressed-quality | 4.5 (3–5, n=4) | 3 (3–3, n=2) | 4 (4–4, n=2) |
| t08-cookie-samesite-default | 2 (2–3, n=4) | 2.5 (2–3, n=2) | 2 (2–2, n=2) |
| t09-lambda-text-content-types | 3.5 (3–5, n=4) | 4 (4–4, n=1 of 2) | 3 (3–3, n=1 of 2) |
| t10-auth-challenge-quoting | 4 (4–4, n=4) | 4 (4–4, n=2) | 4 (4–4, n=2) |

### Share of the turns before the first read of a `must_change` file

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 22.5% (20%–25%, n=4) | 17.1% (14.3%–20%, n=2) | 28.6% (28.6%–28.6%, n=2) |
| t01-cwd-relative-targets | 8.7% (6.7%–10%, n=4) | 10.1% (9.1%–11.1%, n=2) | 12.9% (9.1%–16.7%, n=2) |
| t02-typed-degraded-cause | 12.7% (9.1%–14.3%, n=4) | 11.3% (10%–12.5%, n=2) | 11.8% (11.1%–12.5%, n=2) |
| t03-nested-router-links | 21.1% (18.2%–27.3%, n=4) | 23.1% (23.1%–23.1%, n=1 of 2) | 14.1% (10%–18.2%, n=2) |
| t04-detector-import-gates | 13.3% (10%–16.7%, n=4) | 13.4% (12.5%–14.3%, n=2) | 14.6% (12.5%–16.7%, n=2) |
| t05-mcp-dependency-tools | 10.6% (7.7%–12.5%, n=4) | 17.1% (14.3%–20%, n=2) | 10.4% (8.3%–12.5%, n=2) |
| t06-repo-file-size-limit | 12.7% (9.1%–21.4%, n=4) | 22.7% (18.2%–27.3%, n=2) | 14.6% (11.1%–18.2%, n=2) |
| t07-static-precompressed-quality | 12.5% (0%–12.5%, n=4) | 17.1% (14.3%–20%, n=2) | 14.3% (14.3%–14.3%, n=2) |
| t08-cookie-samesite-default | 0% (0%–0%, n=4) | 0% (0%–0%, n=2) | 0% (0%–0%, n=2) |
| t09-lambda-text-content-types | 25% (25%–25%, n=1 of 4) | – (n=0 of 2) | 37.5% (37.5%–37.5%, n=1 of 2) |
| t10-auth-challenge-quoting | 10% (9.1%–12.5%, n=4) | 10.6% (10%–11.1%, n=2) | 10.6% (10%–11.1%, n=2) |

### First read of a `relevant_files` member, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | 2 (2–2, n=2) | 3 (3–3, n=2) |
| t01-cwd-relative-targets | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t02-typed-degraded-cause | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t03-nested-router-links | 2 (2–3, n=4) | 2 (2–2, n=2) | 2.5 (2–3, n=2) |
| t04-detector-import-gates | 2 (2–2, n=4) | 1.5 (1–2, n=2) | 2 (2–2, n=2) |
| t05-mcp-dependency-tools | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t06-repo-file-size-limit | 2 (2–4, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t07-static-precompressed-quality | 2 (1–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |
| t08-cookie-samesite-default | 1 (1–1, n=4) | 1 (1–1, n=2) | 1 (1–1, n=2) |
| t09-lambda-text-content-types | 4 (4–5, n=4) | 3 (3–3, n=1 of 2) | 3 (2–4, n=2) |
| t10-auth-challenge-quoting | 2 (2–2, n=4) | 2 (2–2, n=2) | 2 (2–2, n=2) |

### Every `relevant_files` member read, turn

| task | baseline | atlas (standard) | atlas (directed) |
|---|---|---|---|
| t00-smoke | 2 (2–2, n=4) | – (n=0 of 2) | – (n=0 of 2) |
| t01-cwd-relative-targets | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t02-typed-degraded-cause | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t03-nested-router-links | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t04-detector-import-gates | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t05-mcp-dependency-tools | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t06-repo-file-size-limit | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t07-static-precompressed-quality | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t08-cookie-samesite-default | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t09-lambda-text-content-types | – (n=0 of 4) | – (n=0 of 2) | – (n=0 of 2) |
| t10-auth-challenge-quoting | 5 (5–6, n=3 of 4) | 5.5 (5–6, n=2) | – (n=0 of 2) |

### Total turns and tool calls

| task | baseline: turns | baseline: tool calls | atlas (standard): turns | atlas (standard): tool calls | atlas (directed): turns | atlas (directed): tool calls |
|---|---|---|---|---|---|---|
| t00-smoke | 4.5 (4–5, n=4) | 6.5 (5–7, n=4) | 6 (5–7, n=2) | 6.5 (5–8, n=2) | 7 (7–7, n=2) | 7.5 (7–8, n=2) |
| t01-cwd-relative-targets | 11.5 (10–15, n=4) | 10.5 (9–14, n=4) | 10 (9–11, n=2) | 9.5 (8–11, n=2) | 8.5 (6–11, n=2) | 9 (6–12, n=2) |
| t02-typed-degraded-cause | 8 (7–11, n=4) | 9 (8–14, n=4) | 9 (8–10, n=2) | 10 (9–11, n=2) | 8.5 (8–9, n=2) | 22.5 (12–33, n=2) |
| t03-nested-router-links | 10.5 (9–11, n=4) | 15 (13–16, n=4) | 12 (11–13, n=2) | 13 (13–13, n=2) | 10.5 (10–11, n=2) | 15 (14–16, n=2) |
| t04-detector-import-gates | 8 (6–10, n=4) | 10 (8–17, n=4) | 7.5 (7–8, n=2) | 8 (8–8, n=2) | 7 (6–8, n=2) | 16 (11–21, n=2) |
| t05-mcp-dependency-tools | 9.5 (8–13, n=4) | 16 (11–18, n=4) | 6 (5–7, n=2) | 10.5 (8–13, n=2) | 10 (8–12, n=2) | 18 (14–22, n=2) |
| t06-repo-file-size-limit | 12 (10–14, n=4) | 13.5 (10–15, n=4) | 11 (11–11, n=2) | 10 (10–10, n=2) | 10 (9–11, n=2) | 13 (13–13, n=2) |
| t07-static-precompressed-quality | 8 (8–8, n=4) | 12 (7–12, n=4) | 6 (5–7, n=2) | 8.5 (7–10, n=2) | 7 (7–7, n=2) | 12.5 (11–14, n=2) |
| t08-cookie-samesite-default | 15.5 (15–18, n=4) | 15.5 (15–17, n=4) | 17.5 (16–19, n=2) | 17.5 (16–19, n=2) | 15 (15–15, n=2) | 15 (15–15, n=2) |
| t09-lambda-text-content-types | 10 (8–12, n=4) | 9.5 (7–12, n=4) | 8 (7–9, n=2) | 7 (6–8, n=2) | 8 (8–8, n=2) | 8 (7–9, n=2) |
| t10-auth-challenge-quoting | 10 (8–11, n=4) | 10.5 (10–12, n=4) | 9.5 (9–10, n=2) | 11.5 (11–12, n=2) | 9.5 (9–10, n=2) | 13 (12–14, n=2) |

## Per session

Ordinals are `turn/tool call`; `–` means no such event. Columns refer to `must_change` files that exist at base. The entry column gives the source of the entry point and the words it shares with it.

| session | batch | group | turns | tool calls | files | named | seen | first read | every file read | first touch | every file touched | first edit | first read via | entry point |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| t00-smoke.r0 | 2026-10-04-seed1 | atlas (standard) | 5 | 5 | 1 | – | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 4/4 | Bash: cat | opened; an identifier or word written in the prompt: acme, entity, user |
| t00-smoke.r1 | 2026-10-04-seed1 | atlas (standard) | 7 | 8 | 1 | – | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 6/7 | Bash: cat | opened; an identifier or word written in the prompt: acme, entity, user |
| t00-smoke.r0 | 2026-10-04-seed1 | baseline | 5 | 7 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/4 | Read | search; an identifier or word written in the prompt: acme, findone, user |
| t00-smoke.r1 | 2026-10-04-seed1 | baseline | 4 | 5 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/4 | Read | search; an identifier or word written in the prompt: user, usersservice |
| t01-cwd-relative-targets.r0 | 2026-10-04-seed1 | atlas (standard) | 9 | 8 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 4/4 | Read | search; an identifier or word written in the prompt: snippet |
| t01-cwd-relative-targets.r1 | 2026-10-04-seed1 | atlas (standard) | 11 | 11 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/3 | Read | search; an identifier or word written in the prompt: snippet |
| t01-cwd-relative-targets.r0 | 2026-10-04-seed1 | baseline | 15 | 14 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 4/4 | Read | search; an identifier or word written in the prompt: snippet |
| t01-cwd-relative-targets.r1 | 2026-10-04-seed1 | baseline | 12 | 11 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/3 | Read | search; an identifier or word written in the prompt: snippet |
| t02-typed-degraded-cause.r0 | 2026-10-04-seed1 | atlas (standard) | 8 | 9 | 6 | 1/1 | 1/1 | 2/2 | 3/5 | 2/2 | 3/5 | 3/5 | Read | search; an identifier or word written in the prompt: changed, configuration, could, degraded, indexed, project |
| t02-typed-degraded-cause.r1 | 2026-10-04-seed1 | atlas (standard) | 10 | 11 | 6 | 1/1 | 1/1 | 2/2 | 4/6 | 2/2 | 4/6 | 4/6 | Read | search; an identifier or word written in the prompt: changed, configuration, could, degraded, indexed, project |
| t02-typed-degraded-cause.r0 | 2026-10-04-seed1 | baseline | 7 | 8 | 6 | 1/1 | 1/1 | 2/2 | – | 2/2 | 4/6 | 4/6 | Read | search; an identifier or word written in the prompt: changed, configuration, degraded, project |
| t02-typed-degraded-cause.r1 | 2026-10-04-seed1 | baseline | 9 | 10 | 6 | 1/1 | 1/1 | 2/2 | 5/7 | 2/2 | 5/7 | 3/5 | Read | search; an identifier or word written in the prompt: changed, configuration, could, degraded, indexed, project |
| t03-nested-router-links.r0 | 2026-10-04-seed1 | atlas (standard) | 11 | 13 | 1 | 1/1 | 1/1 | – | – | 3/3 | 3/3 | 6/8 | – | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on, router_relative |
| t03-nested-router-links.r1 | 2026-10-04-seed1 | atlas (standard) | 13 | 13 | 1 | 1/1 | 3/3 | 4/4 | 4/4 | 3/3 | 3/3 | 9/9 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on_scope |
| t03-nested-router-links.r0 | 2026-10-04-seed1 | baseline | 9 | 13 | 1 | 1/1 | 2/3 | 3/4 | 3/4 | 2/3 | 2/3 | 5/6 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on |
| t03-nested-router-links.r1 | 2026-10-04-seed1 | baseline | 10 | 14 | 1 | 1/1 | 2/3 | 3/4 | 3/4 | 2/3 | 2/3 | 5/6 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on |
| t04-detector-import-gates.r0 | 2026-10-04-seed1 | atlas (standard) | 7 | 8 | 5 | 1/1 | 1/1 | 2/3 | 2/4 | 2/3 | 2/4 | 4/6 | Read | search; an identifier or word written in the prompt: importfact |
| t04-detector-import-gates.r1 | 2026-10-04-seed1 | atlas (standard) | 8 | 8 | 5 | 1/1 | 1/1 | 2/2 | 2/3 | 2/2 | 2/3 | 4/5 | Read | search; an identifier or word written in the prompt: importfact |
| t04-detector-import-gates.r0 | 2026-10-04-seed1 | baseline | 6 | 8 | 5 | 1/1 | 1/1 | 2/3 | 2/4 | 2/3 | 2/4 | 4/7 | Read | search; an identifier or word written in the prompt: importfact |
| t04-detector-import-gates.r1 | 2026-10-04-seed1 | baseline | 6 | 17 | 5 | 1/1 | 1/1 | 2/2 | 2/6 | 2/2 | 2/6 | 4/10 | Read | search; an identifier or word written in the prompt: importfact |
| t05-mcp-dependency-tools.r0 | 2026-10-04-seed1 | atlas (standard) | 7 | 13 | 2 | 1/1 | 1/1 | 2/3 | 3/7 | 2/3 | 3/7 | 5/9 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers |
| t05-mcp-dependency-tools.r1 | 2026-10-04-seed1 | atlas (standard) | 5 | 8 | 2 | 1/1 | 1/1 | 2/3 | 3/6 | 2/3 | 3/6 | 4/8 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers |
| t05-mcp-dependency-tools.r0 | 2026-10-04-seed1 | baseline | 8 | 11 | 2 | 1/1 | 1/1 | 2/2 | 3/5 | 2/2 | 3/5 | 6/8 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers, dependents |
| t05-mcp-dependency-tools.r1 | 2026-10-04-seed1 | baseline | 13 | 18 | 2 | 1/1 | 1/1 | 2/3 | 3/7 | 2/3 | 3/7 | 6/10 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers |
| t06-repo-file-size-limit.r0 | 2026-10-04-seed1 | atlas (standard) | 11 | 10 | 11 | 1/1 | 1/1 | 3/3 | – | 2/2 | 5/5 | 4/4 | Bash: sed -n | search; an identifier or word written in the prompt: max_file_bytes |
| t06-repo-file-size-limit.r1 | 2026-10-04-seed1 | atlas (standard) | 11 | 10 | 11 | 1/1 | 1/1 | 4/4 | – | 2/2 | – | 6/6 | Bash: sed -n | search; an identifier or word written in the prompt: max_file_bytes |
| t06-repo-file-size-limit.r0 | 2026-10-04-seed1 | baseline | 11 | 13 | 11 | 1/1 | 1/1 | 2/3 | – | 2/3 | 5/8 | 4/7 | Read | search; an identifier or word written in the prompt: max_file_bytes |
| t06-repo-file-size-limit.r1 | 2026-10-04-seed1 | baseline | 13 | 14 | 11 | 1/1 | 1/1 | 3/5 | – | 3/5 | 6/8 | 6/8 | Bash: sed -n | search; an identifier or word written in the prompt: max_file_bytes |
| t07-static-precompressed-quality.r0 | 2026-10-04-seed1 | atlas (standard) | 5 | 7 | 1 | 1/1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 3/6 | Read | search; an identifier or word written in the prompt: precompressed |
| t07-static-precompressed-quality.r1 | 2026-10-04-seed1 | atlas (standard) | 7 | 10 | 1 | 1/1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 3/6 | Read | search; an identifier or word written in the prompt: precompressed |
| t07-static-precompressed-quality.r0 | 2026-10-04-seed1 | baseline | 8 | 12 | 1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 2/3 | 5/8 | Read | search; an identifier or word written in the prompt: precompressed |
| t07-static-precompressed-quality.r1 | 2026-10-04-seed1 | baseline | 8 | 12 | 1 | 1/1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 3/5 | Read | search; an identifier or word written in the prompt: precompressed |
| t08-cookie-samesite-default.r0 | 2026-10-04-seed1 | atlas (standard) | 16 | 16 | 8 | – | 1/1 | 1/1 | – | 1/1 | 10/11 | 3/3 | Read | opened; an identifier or word written in the prompt: cookie |
| t08-cookie-samesite-default.r1 | 2026-10-04-seed1 | atlas (standard) | 19 | 19 | 8 | – | 1/1 | 1/1 | – | 1/1 | 14/14 | 2/2 | Read | opened; an identifier or word written in the prompt: cookie |
| t08-cookie-samesite-default.r0 | 2026-10-04-seed1 | baseline | 15 | 15 | 8 | – | 1/1 | 1/1 | – | 1/1 | 11/12 | 2/3 | Read | opened; an identifier or word written in the prompt: cookie |
| t08-cookie-samesite-default.r1 | 2026-10-04-seed1 | baseline | 16 | 16 | 8 | – | 1/1 | 1/1 | – | 1/1 | 14/15 | 2/3 | Read | opened; an identifier or word written in the prompt: cookie |
| t09-lambda-text-content-types.r0 | 2026-10-04-seed1 | atlas (standard) | 9 | 8 | 4 | 1/1 | 2/2 | – | – | 3/3 | 4/4 | 4/4 | – | search; an identifier or word written in the prompt: node_modules |
| t09-lambda-text-content-types.r1 | 2026-10-04-seed1 | atlas (standard) | 7 | 6 | 4 | 1/1 | 2/2 | – | – | 2/2 | – | – | – | search; an identifier or word written in the prompt: node_modules |
| t09-lambda-text-content-types.r0 | 2026-10-04-seed1 | baseline | 11 | 10 | 4 | 1/1 | 2/2 | – | – | 4/4 | 4/4 | 4/4 | – | search; an identifier or word written in the prompt: node_modules |
| t09-lambda-text-content-types.r1 | 2026-10-04-seed1 | baseline | 8 | 7 | 4 | 1/1 | 2/2 | – | – | 3/3 | 3/3 | 3/3 | – | search; an identifier or word written in the prompt: node_modules |
| t10-auth-challenge-quoting.r0 | 2026-10-04-seed1 | atlas (standard) | 9 | 11 | 4 | 1/1 | 1/1 | 2/2 | 2/5 | 2/2 | 2/5 | 4/7 | Read | search; an identifier or word written in the prompt: authenticate |
| t10-auth-challenge-quoting.r1 | 2026-10-04-seed1 | atlas (standard) | 10 | 12 | 4 | 1/1 | 1/1 | 2/2 | 2/5 | 2/2 | 2/5 | 4/7 | Read | search; an identifier or word written in the prompt: authenticate |
| t10-auth-challenge-quoting.r0 | 2026-10-04-seed1 | baseline | 8 | 10 | 4 | 1/1 | 1/1 | 2/2 | 3/6 | 2/2 | 3/6 | 4/7 | Read | search; an identifier or word written in the prompt: authenticate, wwwauthenticateheader |
| t10-auth-challenge-quoting.r1 | 2026-10-04-seed1 | baseline | 11 | 11 | 4 | 1/1 | 1/1 | 2/2 | 2/3 | 2/2 | 2/3 | 4/5 | Read | search; an identifier or word written in the prompt: authenticate |
| t00-smoke.r0 | 2026-10-05-directed | atlas (directed) | 7 | 7 | 1 | – | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 4/4 | Read | opened; an identifier or word written in the prompt: acme, entity, user |
| t00-smoke.r1 | 2026-10-05-directed | atlas (directed) | 7 | 8 | 1 | – | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 4/5 | Read | opened; an identifier or word written in the prompt: acme, entity, user |
| t00-smoke.r0 | 2026-10-05-directed | baseline | 4 | 6 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/4 | Read | search; an identifier or word written in the prompt: user, usersservice |
| t00-smoke.r1 | 2026-10-05-directed | baseline | 5 | 7 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/4 | Read | search; an identifier or word written in the prompt: user, usersservice |
| t01-cwd-relative-targets.r0 | 2026-10-05-directed | atlas (directed) | 11 | 12 | 1 | 1/1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 3/5 | Read | search; an identifier or word written in the prompt: against, does, like, make, option, query, resolve, similar, snippet, targets |
| t01-cwd-relative-targets.r1 | 2026-10-05-directed | atlas (directed) | 6 | 6 | 1 | 1/1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 3/4 | Read | search; an identifier or word written in the prompt: against, atlas, does, like, option, query, resolve, snippet, targets |
| t01-cwd-relative-targets.r0 | 2026-10-05-directed | baseline | 10 | 9 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/3 | Read | search; an identifier or word written in the prompt: snippet |
| t01-cwd-relative-targets.r1 | 2026-10-05-directed | baseline | 11 | 10 | 1 | 1/1 | 1/1 | 2/2 | 2/2 | 2/2 | 2/2 | 3/3 | Read | search; an identifier or word written in the prompt: snippet |
| t02-typed-degraded-cause.r0 | 2026-10-05-directed | atlas (directed) | 8 | 12 | 6 | 1/1 | 1/1 | 2/3 | 4/9 | 2/3 | 4/9 | 3/8 | Read | search; an identifier or word written in the prompt: degraded, degradedcause, freshnessreport, reason, replace, string, typed |
| t02-typed-degraded-cause.r1 | 2026-10-05-directed | atlas (directed) | 9 | 33 | 6 | 1/1 | 1/1 | 2/3 | 4/24 | 2/3 | 4/24 | 3/7 | Read | search; an identifier or word written in the prompt: degraded, degradedcause, freshnessreport, reason, replace, string, typed |
| t02-typed-degraded-cause.r0 | 2026-10-05-directed | baseline | 11 | 14 | 6 | 1/1 | 1/1 | 2/2 | 5/9 | 2/2 | 5/9 | 4/7 | Read | search; an identifier or word written in the prompt: changed, configuration, could, degraded, indexed, project |
| t02-typed-degraded-cause.r1 | 2026-10-05-directed | baseline | 7 | 8 | 6 | 1/1 | 1/1 | 2/2 | 4/6 | 2/2 | 4/6 | 3/5 | Read | search; an identifier or word written in the prompt: changed, configuration, degraded, freshnessreport, project |
| t03-nested-router-links.r0 | 2026-10-05-directed | atlas (directed) | 10 | 16 | 1 | 1/2 | 1/2 | 2/4 | 2/4 | 2/4 | 2/4 | 4/7 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on_scope |
| t03-nested-router-links.r1 | 2026-10-05-directed | atlas (directed) | 11 | 14 | 1 | 1/2 | 2/3 | 3/4 | 3/4 | 2/3 | 2/3 | 6/9 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on_scope |
| t03-nested-router-links.r0 | 2026-10-05-directed | baseline | 11 | 16 | 1 | 1/1 | 3/4 | 4/5 | 4/5 | 3/4 | 3/4 | 6/8 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on |
| t03-nested-router-links.r1 | 2026-10-05-directed | baseline | 11 | 16 | 1 | 1/1 | 2/3 | 3/4 | 3/4 | 2/3 | 2/3 | 4/6 | Read | search; an identifier or word written in the injected instructions: mount_prefix, mounted_on |
| t04-detector-import-gates.r0 | 2026-10-05-directed | atlas (directed) | 6 | 21 | 5 | 1/1 | 1/1 | 2/3 | – | 2/3 | 4/18 | 4/9 | Read | search; an identifier or word written in the prompt: abstraction, atlas, detect, detectors, extract, importfact, matching, shared |
| t04-detector-import-gates.r1 | 2026-10-05-directed | atlas (directed) | 8 | 11 | 5 | 1/1 | 1/1 | 2/3 | – | 2/3 | 4/8 | 4/8 | Read | search; an identifier or word written in the prompt: abstraction, atlas, detect, detection, detectors, extract, importfact, matching, shared |
| t04-detector-import-gates.r0 | 2026-10-05-directed | baseline | 10 | 10 | 5 | 1/1 | 1/1 | 2/2 | 2/3 | 2/2 | 2/3 | 4/5 | Read | search; an identifier or word written in the prompt: importfact |
| t04-detector-import-gates.r1 | 2026-10-05-directed | baseline | 10 | 10 | 5 | 1/1 | 1/1 | 2/2 | 2/3 | 2/2 | 2/3 | 4/5 | Read | search; an identifier or word written in the prompt: importfact |
| t05-mcp-dependency-tools.r0 | 2026-10-05-directed | atlas (directed) | 12 | 22 | 2 | 1/1 | 1/1 | 2/3 | 3/7 | 2/3 | 3/7 | 5/10 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers, atlas_dependencies, atlas_dependents, dependencies, dependents, query, tools |
| t05-mcp-dependency-tools.r1 | 2026-10-05-directed | atlas (directed) | 8 | 14 | 2 | 1/1 | 1/1 | 2/3 | 2/5 | 2/3 | 2/5 | 5/10 | Read | search; an identifier or word written in the prompt: atlas, atlas_callees, atlas_callers, atlas_dependencies, atlas_dependents, dependencies, dependents, query, tools |
| t05-mcp-dependency-tools.r0 | 2026-10-05-directed | baseline | 10 | 15 | 2 | 1/1 | 1/1 | 2/2 | 3/6 | 2/2 | 3/6 | 7/12 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers, dependents |
| t05-mcp-dependency-tools.r1 | 2026-10-05-directed | baseline | 9 | 17 | 2 | 1/1 | 1/1 | 2/3 | 3/7 | 2/3 | 3/7 | 5/9 | Read | search; an identifier or word written in the prompt: atlas_callees, atlas_callers |
| t06-repo-file-size-limit.r0 | 2026-10-05-directed | atlas (directed) | 9 | 13 | 11 | 1/1 | 1/1 | 2/3 | – | 2/3 | 5/10 | 5/10 | Read | search; an identifier or word written in the prompt: freshness, full, honoured, incremental, max_file_bytes, projects, repo, scans, toml, updates |
| t06-repo-file-size-limit.r1 | 2026-10-05-directed | atlas (directed) | 11 | 13 | 11 | 1/1 | 1/1 | 3/5 | – | 3/5 | 6/9 | 6/9 | Read | search; an identifier or word written in the prompt: freshness, full, honoured, incremental, max_file_bytes, projects, repo, scans, toml, updates |
| t06-repo-file-size-limit.r0 | 2026-10-05-directed | baseline | 14 | 15 | 11 | 1/1 | 1/1 | 4/5 | – | 4/5 | 8/10 | 6/8 | Read | search; an identifier or word written in the prompt: max_file_bytes |
| t06-repo-file-size-limit.r1 | 2026-10-05-directed | baseline | 10 | 10 | 11 | 1/1 | 1/1 | 2/3 | – | 2/3 | 4/5 | 4/5 | Read | search; an identifier or word written in the prompt: max_file_bytes |
| t07-static-precompressed-quality.r0 | 2026-10-05-directed | atlas (directed) | 7 | 14 | 1 | 1/2 | 2/3 | 2/3 | 2/3 | 2/3 | 2/3 | 4/8 | Read | search; an identifier or word written in the prompt: node_modules, precompressed |
| t07-static-precompressed-quality.r1 | 2026-10-05-directed | atlas (directed) | 7 | 11 | 1 | 1/2 | 1/2 | 2/3 | 2/3 | 2/3 | 2/3 | 4/6 | Read | search; an identifier or word written in the prompt: precompressed |
| t07-static-precompressed-quality.r0 | 2026-10-05-directed | baseline | 8 | 7 | 1 | – | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 4/4 | Bash: cat | opened; an identifier or word written in the prompt: serve |
| t07-static-precompressed-quality.r1 | 2026-10-05-directed | baseline | 8 | 12 | 1 | 1/1 | 2/3 | 2/3 | 2/3 | 2/3 | 2/3 | 5/8 | Read | search; an identifier or word written in the prompt: precompressed |
| t08-cookie-samesite-default.r0 | 2026-10-05-directed | atlas (directed) | 15 | 15 | 8 | 1/1 | 1/1 | 1/2 | – | 1/2 | 11/12 | 2/3 | Read | search; an identifier or word written in the prompt: cookie, default, deletecookie, generatecookie, generatesignedcookie, hono, samesite, setcookie, setsignedcookie |
| t08-cookie-samesite-default.r1 | 2026-10-05-directed | atlas (directed) | 15 | 15 | 8 | 1/1 | 1/1 | 1/2 | – | 1/2 | 12/13 | 2/3 | Bash: cat | search; an identifier or word written in the prompt: cookie, default, deletecookie, generatecookie, generatesignedcookie, hono, samesite, setcookie, setsignedcookie |
| t08-cookie-samesite-default.r0 | 2026-10-05-directed | baseline | 18 | 17 | 8 | – | 1/1 | 1/1 | – | 1/1 | 16/16 | 3/3 | Read | opened; an identifier or word written in the prompt: cookie |
| t08-cookie-samesite-default.r1 | 2026-10-05-directed | baseline | 15 | 15 | 8 | – | 1/1 | 1/1 | – | 1/1 | 11/12 | 2/3 | Read | opened; an identifier or word written in the prompt: cookie |
| t09-lambda-text-content-types.r0 | 2026-10-05-directed | atlas (directed) | 8 | 7 | 4 | 1/1 | 2/2 | – | – | 3/3 | 3/3 | 3/3 | – | search; an identifier or word written in the prompt: node_modules |
| t09-lambda-text-content-types.r1 | 2026-10-05-directed | atlas (directed) | 8 | 9 | 4 | 1/1 | 1/1 | 4/6 | – | 4/6 | – | – | Bash: sed -n | search; an identifier or word written in the prompt: adapters, application, built, edge, form, javascript, lambda, packages, standalone, text, urlencoded |
| t09-lambda-text-content-types.r0 | 2026-10-05-directed | baseline | 12 | 12 | 4 | 1/1 | 2/2 | 4/4 | – | 3/3 | 5/5 | 5/5 | Bash: sed -n | search; an identifier or word written in the prompt: node_modules |
| t09-lambda-text-content-types.r1 | 2026-10-05-directed | baseline | 9 | 9 | 4 | 1/1 | 2/2 | – | – | 3/3 | 3/3 | 3/3 | – | search; an identifier or word written in the prompt: node_modules |
| t10-auth-challenge-quoting.r0 | 2026-10-05-directed | atlas (directed) | 10 | 14 | 4 | 1/2 | 1/2 | 2/3 | 2/6 | 2/3 | 2/6 | 4/9 | Read | search; an identifier or word written in the prompt: authenticate |
| t10-auth-challenge-quoting.r1 | 2026-10-05-directed | atlas (directed) | 9 | 12 | 4 | 1/2 | 1/2 | 2/3 | 2/6 | 2/3 | 2/6 | 4/8 | Read | search; an identifier or word written in the prompt: authenticate |
| t10-auth-challenge-quoting.r0 | 2026-10-05-directed | baseline | 10 | 10 | 4 | 1/1 | 1/1 | 2/2 | 2/3 | 2/2 | 2/3 | 4/5 | Read | search; an identifier or word written in the prompt: authenticate |
| t10-auth-challenge-quoting.r1 | 2026-10-05-directed | baseline | 10 | 12 | 4 | 1/1 | 1/1 | 2/2 | 2/5 | 2/2 | 2/5 | 4/7 | Read | search; an identifier or word written in the prompt: authenticate, wwwauthenticateheader |

## `must_change` files by session

### Edited with no earlier read

The file was edited and no read of it (as defined above) was issued in an earlier tool call. Lines of it may still have been shown in a search result.

- `t02-typed-degraded-cause.baseline.r0` (2026-10-04-seed1, baseline): `crates/atlas-cli/src/commands/watch.rs`
- `t03-nested-router-links.atlas.r0` (2026-10-04-seed1, atlas (standard)): `crates/atlas-detect/src/detectors/express.rs`
- `t06-repo-file-size-limit.atlas.r0` (2026-10-04-seed1, atlas (standard)): `crates/atlas-project/src/registry.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.atlas.r1` (2026-10-04-seed1, atlas (standard)): `crates/atlas-project/src/discovery.rs`
- `t06-repo-file-size-limit.baseline.r0` (2026-10-04-seed1, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r1` (2026-10-04-seed1, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t08-cookie-samesite-default.atlas.r0` (2026-10-04-seed1, atlas (standard)): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.atlas.r1` (2026-10-04-seed1, atlas (standard)): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r0` (2026-10-04-seed1, baseline): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r1` (2026-10-04-seed1, baseline): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t09-lambda-text-content-types.atlas.r0` (2026-10-04-seed1, atlas (standard)): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t09-lambda-text-content-types.baseline.r0` (2026-10-04-seed1, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t09-lambda-text-content-types.baseline.r1` (2026-10-04-seed1, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t02-typed-degraded-cause.baseline.r0` (2026-10-05-directed, baseline): `crates/atlas-cli/src/commands/watch.rs`
- `t04-detector-import-gates.atlas.r0` (2026-10-05-directed, atlas (directed)): `crates/atlas-detect/src/detectors/nest.rs`, `crates/atlas-detect/src/detectors/clients.rs`
- `t04-detector-import-gates.atlas.r1` (2026-10-05-directed, atlas (directed)): `crates/atlas-detect/src/detectors/nest.rs`, `crates/atlas-detect/src/detectors/clients.rs`
- `t06-repo-file-size-limit.atlas.r0` (2026-10-05-directed, atlas (directed)): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.atlas.r1` (2026-10-05-directed, atlas (directed)): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r0` (2026-10-05-directed, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r1` (2026-10-05-directed, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t08-cookie-samesite-default.atlas.r0` (2026-10-05-directed, atlas (directed)): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.atlas.r1` (2026-10-05-directed, atlas (directed)): `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r0` (2026-10-05-directed, baseline): `src/helper/cookie/index.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r1` (2026-10-05-directed, baseline): `src/helper/cookie/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t09-lambda-text-content-types.atlas.r0` (2026-10-05-directed, atlas (directed)): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t09-lambda-text-content-types.baseline.r0` (2026-10-05-directed, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`
- `t09-lambda-text-content-types.baseline.r1` (2026-10-05-directed, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`

### Edited with no earlier read or search

As above, and no search addressed to the file came earlier either.

- `t02-typed-degraded-cause.baseline.r0` (2026-10-04-seed1, baseline): `crates/atlas-cli/src/commands/watch.rs`
- `t06-repo-file-size-limit.atlas.r0` (2026-10-04-seed1, atlas (standard)): `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r1` (2026-10-04-seed1, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t08-cookie-samesite-default.atlas.r0` (2026-10-04-seed1, atlas (standard)): `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.atlas.r1` (2026-10-04-seed1, atlas (standard)): `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r0` (2026-10-04-seed1, baseline): `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r1` (2026-10-04-seed1, baseline): `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t09-lambda-text-content-types.atlas.r0` (2026-10-04-seed1, atlas (standard)): `src/adapter/lambda-edge/handler.ts`
- `t09-lambda-text-content-types.baseline.r0` (2026-10-04-seed1, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t09-lambda-text-content-types.baseline.r1` (2026-10-04-seed1, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t02-typed-degraded-cause.baseline.r0` (2026-10-05-directed, baseline): `crates/atlas-cli/src/commands/watch.rs`
- `t04-detector-import-gates.atlas.r0` (2026-10-05-directed, atlas (directed)): `crates/atlas-detect/src/detectors/nest.rs`, `crates/atlas-detect/src/detectors/clients.rs`
- `t04-detector-import-gates.atlas.r1` (2026-10-05-directed, atlas (directed)): `crates/atlas-detect/src/detectors/nest.rs`, `crates/atlas-detect/src/detectors/clients.rs`
- `t06-repo-file-size-limit.atlas.r0` (2026-10-05-directed, atlas (directed)): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.atlas.r1` (2026-10-05-directed, atlas (directed)): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r0` (2026-10-05-directed, baseline): `crates/atlas-project/src/discovery.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
- `t06-repo-file-size-limit.baseline.r1` (2026-10-05-directed, baseline): `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`
- `t08-cookie-samesite-default.atlas.r0` (2026-10-05-directed, atlas (directed)): `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r0` (2026-10-05-directed, baseline): `adapters/aws-lambda/src/handler.test.ts`
- `t08-cookie-samesite-default.baseline.r1` (2026-10-05-directed, baseline): `adapters/lambda-edge/src/handler.test.ts`
- `t09-lambda-text-content-types.atlas.r0` (2026-10-05-directed, atlas (directed)): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`
- `t09-lambda-text-content-types.baseline.r0` (2026-10-05-directed, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`
- `t09-lambda-text-content-types.baseline.r1` (2026-10-05-directed, baseline): `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`

### No recognised touch

No search, read or edit addressed to the file was recognised in the session. A file marked as changed was changed all the same, in a form this analysis does not recognise (see the detection checks).

- `t06-repo-file-size-limit.atlas.r1` (2026-10-04-seed1, atlas (standard)): `crates/atlas-project/src/manifest/tests.rs` (changed in the final diff), `crates/atlas-engine/src/watch.rs` (changed in the final diff), `crates/atlas-engine/tests/common/mod.rs` (changed in the final diff), `crates/atlas-resolve/tests/common/mod.rs` (changed in the final diff), `crates/atlas-resolve/tests/incremental.rs` (changed in the final diff), `crates/atlas-resolve/tests/workspace_quality.rs` (changed in the final diff), `crates/atlas-query/tests/common/mod.rs` (changed in the final diff), `crates/atlas-query/tests/perf.rs` (changed in the final diff), `crates/atlas-link/tests/common/mod.rs` (changed in the final diff)
- `t09-lambda-text-content-types.atlas.r1` (2026-10-04-seed1, atlas (standard)): `adapters/lambda-edge/src/handler.ts` (changed in the final diff)
- `t09-lambda-text-content-types.atlas.r1` (2026-10-05-directed, atlas (directed)): `src/adapter/lambda-edge/handler.ts` (changed in the final diff), `adapters/lambda-edge/src/handler.ts` (changed in the final diff)

### Created by the task

Absent from the base tree, so there was nothing to find; excluded from the statistics.

None.

## How Bash is mapped to files

Bash commands are interpreted lexically, never executed. Recognised: `;`, `&&`, `||`, `|`, newlines, subshells, quoting, output redirects, here-documents, `for x in a b; do …; done` over literal lists, variables assigned in the same command, brace lists, and globs expanded against the base tree plus the files the session created with Write, a redirect, `tee` or `touch`.

The working directory is followed the way Claude Code keeps it. A command's final directory carries over to the next call only if the command exits 0; the transcript records that per call (an error result, a `returnCodeInterpretation` such as "No matches found", or a command moved to the background, whose `backgroundCwdHint` states the directory). A `Shell cwd was reset to …` notice and the directory named in a failed Edit are honoured. A `cd` into a directory that the tree model does not know is taken to have failed when the output says so, or when the command hid its errors; the commands chained to it with `&&` are then skipped. Otherwise the directory is taken to exist (build output, installed packages).

Not recognised, so a touch made only this way is missed or shows up as a mention: scripts run from a file (`python3 fix.py a b`), `awk`, `less`, `nl`, `wc`, `diff`, `cp`, `mv`, `rm`, `patch`, `git checkout`/`restore`/`apply`/`stash`, formatters that rewrite files (`cargo fmt`, `prettier --write`), command substitution, parameter expansion with operators (`${f%:*}`), variables set in an earlier call, `xargs`, `find -exec`, `while read` loops, and Python that builds a path at run time or only reads a file. A Python script that writes is taken to edit every existing file that one of its string literals names, also as a whitespace-separated word, relative to the shell's directory or to a literal `os.chdir`. A failing command inside an `&&` chain stops the chain only when it is a rejected `sed -i` or a failed `cd`.

## Detection checks

- Sessions analysed: 88.
- Sessions with no recognised read of a `must_change` file: 7. For each, the turn/tool call at which a result first carried lines of one, and the first edit: `t03-nested-router-links.atlas.r0` (2026-10-04-seed1, atlas (standard)): lines first shown at 1/1, first edit at 6/8; `t09-lambda-text-content-types.atlas.r0` (2026-10-04-seed1, atlas (standard)): lines first shown at 2/2, first edit at 4/4; `t09-lambda-text-content-types.atlas.r1` (2026-10-04-seed1, atlas (standard)): lines first shown at 2/2, first edit at –; `t09-lambda-text-content-types.baseline.r0` (2026-10-04-seed1, baseline): lines first shown at 2/2, first edit at 4/4; `t09-lambda-text-content-types.baseline.r1` (2026-10-04-seed1, baseline): lines first shown at 2/2, first edit at 3/3; `t09-lambda-text-content-types.atlas.r0` (2026-10-05-directed, atlas (directed)): lines first shown at 2/2, first edit at 3/3; `t09-lambda-text-content-types.baseline.r1` (2026-10-05-directed, baseline): lines first shown at 2/2, first edit at 3/3.
- Sessions whose first agent-side reference to a `must_change` file is a mention, earlier than the first touch: 2: `t00-smoke.atlas.r0` (2026-10-05-directed, via Glob input), `t00-smoke.atlas.r1` (2026-10-05-directed, via Glob input). For these the first touch may be later than the truth.
- `cd` commands whose output reports that the directory does not exist (the directory is kept): 4. `cd` commands into a directory the tree model does not know, with errors hidden (assumed to have failed): 0 in 0 sessions. Working directories stated by the harness itself (a failed Edit's note, a backgrounded command's hint): 4, of which 0 contradict the tracked directory.
- Read commands with operands that could not be resolved (dynamic or unmatched glob): 19. `sed -i` commands rejected by sed and not counted as edits: 67 file operands. Failed Read/Edit/Write calls: 2. Shell working-directory resets: 1.
- Edit detection against each session's final diff, over `must_change` and `relevant_files`: 40 file(s) changed in the diff without a recognised edit event; 1 file(s) with an edit event that the diff does not show as changed (the edit was reverted, rewrote the file without changing it, or was misdetected).
  - `t04-detector-import-gates.atlas.r0` (2026-10-04-seed1): changed without an edit event: `crates/atlas-detect/src/detectors/auth.rs`
  - `t06-repo-file-size-limit.atlas.r1` (2026-10-04-seed1): changed without an edit event: `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-link/tests/common/mod.rs`
  - `t09-lambda-text-content-types.atlas.r1` (2026-10-04-seed1): changed without an edit event: `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`, `src/adapter/aws-lambda/handler.test.ts`, `src/adapter/lambda-edge/handler.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
  - `t09-lambda-text-content-types.baseline.r0` (2026-10-04-seed1): changed without an edit event: `src/adapter/aws-lambda/handler.test.ts`, `src/adapter/lambda-edge/handler.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
  - `t10-auth-challenge-quoting.baseline.r0` (2026-10-04-seed1): changed without an edit event: `src/middleware/jwt/jwt.ts`, `src/middleware/jwk/jwk.ts`
  - `t10-auth-challenge-quoting.baseline.r1` (2026-10-04-seed1): edit event but unchanged: `src/middleware/jwk/index.test.ts`
  - `t09-lambda-text-content-types.atlas.r1` (2026-10-05-directed): changed without an edit event: `src/adapter/aws-lambda/handler.ts`, `src/adapter/lambda-edge/handler.ts`, `adapters/aws-lambda/src/handler.ts`, `adapters/lambda-edge/src/handler.ts`, `src/adapter/aws-lambda/handler.test.ts`, `src/adapter/lambda-edge/handler.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
  - `t09-lambda-text-content-types.baseline.r0` (2026-10-05-directed): changed without an edit event: `src/adapter/aws-lambda/handler.test.ts`, `src/adapter/lambda-edge/handler.test.ts`, `adapters/aws-lambda/src/handler.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
  - `t10-auth-challenge-quoting.atlas.r1` (2026-10-05-directed): changed without an edit event: `src/middleware/jwt/jwt.ts`, `src/middleware/jwk/jwk.ts`
  - `t10-auth-challenge-quoting.baseline.r1` (2026-10-05-directed): changed without an edit event: `src/middleware/jwt/jwt.ts`, `src/middleware/jwk/jwk.ts`

