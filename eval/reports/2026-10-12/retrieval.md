# Retrieval report

> Dev set, same authors: the tasks, their ground truth and Code Atlas were written by the same people, and the tasks were used while developing the product. Read these numbers as regression signals for the retrieval surface, not as an unbiased estimate of how it performs on unseen code.

Deterministic and LLM-free: every task's base tree is materialised like the agent harness does it (no setup commands), indexed in process, and queried once per query variant below. Ground truth is used only to score, and to pick the blast-radius seed symbols.

## Settings

| key | value |
|---|---|
| code atlas revision | 1c1232df7b9779c7b442740c48136972343cedf7 |
| context budget (CLI and MCP default) | 6000 |
| budgeted-yield budgets | 4000, 8000, 16000 |
| search limit per query | 10 |
| searches | the query, up to 8 code spans, up to 4 prose phrases |
| similar seeds | up to 5 top function/method search hits |
| impact group limit (visible) | 15 |

## Query variants

Every variant runs the same pipeline with its own query text in place of the prompt. A task with several queries for a variant is scored on each and its scores are averaged; a task with none is `n/a` for that variant and leaves its means.

| variant | query | tasks with a query | queries | queries with an empty MUST READ | queries with an empty pack |
|---|---|---|---|---|---|
| full | the whole task prompt | 11 of 11 | 11 | 0 | 0 |
| agent | the `task` argument of each distinct recorded `atlas_context` call, without its seeds | 11 of 11 | 20 | 2 | 2 |
| agent-seeded | each distinct recorded `atlas_context` call as sent: the `task` argument with its `seeds` | 11 of 11 | 20 | 2 | 2 |
| first-line | the first sentence of the prompt's first paragraph | 11 of 11 | 11 | 0 | 0 |

- Recorded calls: `eval/tasks/<task>/queries.toml`, from batch `2026-10-05-directed` (21 runs sent an `atlas_context` call, 1 run sent none).
- `agent-seeded` differs from `agent` only in `context`, which receives the seeds; `search` and `similar` take the same text. It equals `agent` on a task whose recorded calls carry no seeds.
- First sentence: the prompt's first paragraph (the text before the first blank line, lines joined by a space), cut before the first `.`, `?` or `!` that follows a letter or digit, lies outside a backtick code span and is followed by whitespace or the end of the paragraph. Without such a character the whole paragraph is the query.

## Metric definitions

- Recall of `must_change` and precision against `relevant_files` are per channel and unchanged from earlier reports. Precision is undefined (`n/a`) for a channel that returned no file, so an empty MUST READ section leaves the precision mean instead of lowering it.
- F0.5 = 1.25 x hits / (0.25 x gold + retrieved), which equals (1 + b^2) x P x R / (b^2 x P + R) with b = 0.5 and weights precision twice as heavily as recall. It is 0 when no gold item is retrieved, an empty channel included, and `n/a` only when the gold set is empty.
- File F0.5 takes one gold set for both precision and recall: the `relevant_files` present in the index. It is the only labelled set these reports already score for both, so the value follows from the `relevant_files` precision and recall columns. A `must_change` file that the task does not also list in `relevant_files` counts as a false positive here, as it does in the precision column. `must_change.f05.*` in the aggregate is the strict reading with `must_change` as the gold set.
- Line F0.5 (MUST READ and whole pack only). Gold: the old-side lines `reference.patch` changes in files of the base tree, that is every removed line plus, for an insertion that replaces nothing, the old lines directly before and after it. Files the patch adds have no base lines; they are left out and listed per task. Gold lines in base files the index does not hold stay in the gold set and are counted per task. Retrieved: the union of the indexed line spans of the entities the pack items name, whatever part of that span the pack prints; an item whose entity has no span retrieves no line and is counted. Precision, recall and F0.5 are over (file, line) pairs.
- Budgeted yield: recall of `must_change` when `context` is asked again with `budget` set to 4000, 8000, 16000 tokens, for MUST READ and for the whole pack. The budget is the query's own parameter and the product's own token estimate decides what fits; each task lists the estimate and the rendered bytes at every budget.

## Headline: `context` by query variant

Mean over the tasks that define the metric, with that task count in parentheses (11 of 11 tasks were scored). The first four metrics are at the default budget.

| variant | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|
| full | MUST READ | 0.555 (11) | 0.378 (11) | 0.365 (11) | 0.008 (10) | 0.555 (11) | 0.555 (11) | 0.555 (11) |
| full | whole pack | 0.776 (11) | 0.237 (11) | 0.26 (11) | 0.009 (10) | 0.776 (11) | 0.776 (11) | 0.776 (11) |
| agent | MUST READ | 0.287 (11) | 0.67 (10) | 0.45 (11) | 0.094 (10) | 0.287 (11) | 0.287 (11) | 0.287 (11) |
| agent | whole pack | 0.417 (11) | 0.405 (10) | 0.365 (11) | 0.014 (10) | 0.417 (11) | 0.417 (11) | 0.417 (11) |
| agent-seeded | MUST READ | 0.287 (11) | 0.67 (10) | 0.45 (11) | 0.094 (10) | 0.287 (11) | 0.287 (11) | 0.287 (11) |
| agent-seeded | whole pack | 0.417 (11) | 0.405 (10) | 0.365 (11) | 0.014 (10) | 0.417 (11) | 0.417 (11) | 0.417 (11) |
| first-line | MUST READ | 0.344 (11) | 0.421 (11) | 0.321 (11) | 0.07 (10) | 0.344 (11) | 0.344 (11) | 0.344 (11) |
| first-line | whole pack | 0.488 (11) | 0.261 (11) | 0.273 (11) | 0.012 (10) | 0.488 (11) | 0.488 (11) | 0.488 (11) |

What the budget changes, counted over queries: a pack estimate or a `must_change` recall changes when it is not the same at every yield budget.

| variant | queries | pack estimate changes | MUST READ recall changes | whole-pack recall changes |
|---|---|---|---|---|
| full | 11 | 5 | 0 | 0 |
| agent | 20 | 0 | 0 | 0 |
| agent-seeded | 20 | 0 | 0 | 0 |
| first-line | 11 | 0 | 0 | 0 |

## Short queries against the full prompt

Paired: each row uses only the tasks that define the metric for both `full` and the variant, so its `full` mean can differ from the headline. The last three columns count the tasks on which the variant is higher than, equal to and lower than `full`.

- `agent` is lower than `full` on MUST READ must_change.recall: 0.287 against 0.555 over 11 tasks (higher on 0, equal on 5, lower on 6).
- `agent` is higher than `full` on MUST READ relevant_files.f05: 0.45 against 0.365 over 11 tasks (higher on 8, equal on 0, lower on 3).
- `agent` is lower than `full` on whole pack must_change.recall: 0.417 against 0.776 over 11 tasks (higher on 1, equal on 2, lower on 8).
- `agent` is higher than `full` on whole pack relevant_files.f05: 0.365 against 0.26 over 11 tasks (higher on 8, equal on 0, lower on 3).
- `agent-seeded` is lower than `full` on MUST READ must_change.recall: 0.287 against 0.555 over 11 tasks (higher on 0, equal on 5, lower on 6).
- `agent-seeded` is higher than `full` on MUST READ relevant_files.f05: 0.45 against 0.365 over 11 tasks (higher on 8, equal on 0, lower on 3).
- `agent-seeded` is lower than `full` on whole pack must_change.recall: 0.417 against 0.776 over 11 tasks (higher on 1, equal on 2, lower on 8).
- `agent-seeded` is higher than `full` on whole pack relevant_files.f05: 0.365 against 0.26 over 11 tasks (higher on 8, equal on 0, lower on 3).
- `first-line` is lower than `full` on MUST READ must_change.recall: 0.344 against 0.555 over 11 tasks (higher on 1, equal on 6, lower on 4).
- `first-line` is lower than `full` on MUST READ relevant_files.f05: 0.321 against 0.365 over 11 tasks (higher on 4, equal on 0, lower on 7).
- `first-line` is lower than `full` on whole pack must_change.recall: 0.488 against 0.776 over 11 tasks (higher on 1, equal on 5, lower on 5).
- `first-line` is higher than `full` on whole pack relevant_files.f05: 0.273 against 0.26 over 11 tasks (higher on 5, equal on 0, lower on 6).

| variant | section | metric | tasks | full | variant | difference | higher | equal | lower |
|---|---|---|---|---|---|---|---|---|---|
| agent | MUST READ | must_change.recall | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent | MUST READ | relevant_files.precision | 10 | 0.366 | 0.67 | +0.304 | 8 | 0 | 2 |
| agent | MUST READ | relevant_files.f05 | 11 | 0.365 | 0.45 | +0.085 | 8 | 0 | 3 |
| agent | MUST READ | lines.f05 | 10 | 0.008 | 0.094 | +0.086 | 6 | 3 | 1 |
| agent | MUST READ | yield.4000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent | MUST READ | yield.8000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent | MUST READ | yield.16000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent | whole pack | must_change.recall | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent | whole pack | relevant_files.precision | 10 | 0.227 | 0.405 | +0.178 | 8 | 0 | 2 |
| agent | whole pack | relevant_files.f05 | 11 | 0.26 | 0.365 | +0.105 | 8 | 0 | 3 |
| agent | whole pack | lines.f05 | 10 | 0.009 | 0.014 | +0.006 | 6 | 3 | 1 |
| agent | whole pack | yield.4000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent | whole pack | yield.8000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent | whole pack | yield.16000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent-seeded | MUST READ | must_change.recall | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent-seeded | MUST READ | relevant_files.precision | 10 | 0.366 | 0.67 | +0.304 | 8 | 0 | 2 |
| agent-seeded | MUST READ | relevant_files.f05 | 11 | 0.365 | 0.45 | +0.085 | 8 | 0 | 3 |
| agent-seeded | MUST READ | lines.f05 | 10 | 0.008 | 0.094 | +0.086 | 6 | 3 | 1 |
| agent-seeded | MUST READ | yield.4000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent-seeded | MUST READ | yield.8000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent-seeded | MUST READ | yield.16000 | 11 | 0.555 | 0.287 | -0.268 | 0 | 5 | 6 |
| agent-seeded | whole pack | must_change.recall | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent-seeded | whole pack | relevant_files.precision | 10 | 0.227 | 0.405 | +0.178 | 8 | 0 | 2 |
| agent-seeded | whole pack | relevant_files.f05 | 11 | 0.26 | 0.365 | +0.105 | 8 | 0 | 3 |
| agent-seeded | whole pack | lines.f05 | 10 | 0.009 | 0.014 | +0.006 | 6 | 3 | 1 |
| agent-seeded | whole pack | yield.4000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent-seeded | whole pack | yield.8000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| agent-seeded | whole pack | yield.16000 | 11 | 0.776 | 0.417 | -0.359 | 1 | 2 | 8 |
| first-line | MUST READ | must_change.recall | 11 | 0.555 | 0.344 | -0.211 | 1 | 6 | 4 |
| first-line | MUST READ | relevant_files.precision | 11 | 0.378 | 0.421 | +0.043 | 5 | 0 | 6 |
| first-line | MUST READ | relevant_files.f05 | 11 | 0.365 | 0.321 | -0.044 | 4 | 0 | 7 |
| first-line | MUST READ | lines.f05 | 10 | 0.008 | 0.07 | +0.062 | 3 | 5 | 2 |
| first-line | MUST READ | yield.4000 | 11 | 0.555 | 0.344 | -0.211 | 1 | 6 | 4 |
| first-line | MUST READ | yield.8000 | 11 | 0.555 | 0.344 | -0.211 | 1 | 6 | 4 |
| first-line | MUST READ | yield.16000 | 11 | 0.555 | 0.344 | -0.211 | 1 | 6 | 4 |
| first-line | whole pack | must_change.recall | 11 | 0.776 | 0.488 | -0.288 | 1 | 5 | 5 |
| first-line | whole pack | relevant_files.precision | 11 | 0.237 | 0.261 | +0.024 | 4 | 1 | 6 |
| first-line | whole pack | relevant_files.f05 | 11 | 0.26 | 0.273 | +0.012 | 5 | 0 | 6 |
| first-line | whole pack | lines.f05 | 10 | 0.009 | 0.012 | +0.003 | 4 | 4 | 2 |
| first-line | whole pack | yield.4000 | 11 | 0.776 | 0.488 | -0.288 | 1 | 5 | 5 |
| first-line | whole pack | yield.8000 | 11 | 0.776 | 0.488 | -0.288 | 1 | 5 | 5 |
| first-line | whole pack | yield.16000 | 11 | 0.776 | 0.488 | -0.288 | 1 | 5 | 5 |

## Aggregate (mean over the 11 of 11 tasks that define each metric)

Metric names start with the query variant; `full.*` is what earlier reports listed without a prefix.

| metric | mean | tasks |
|---|---|---|
| full.must_change.recall.must_read | 0.555 | 11 |
| full.must_change.recall.pack | 0.776 | 11 |
| full.must_change.recall.search | 0.694 | 11 |
| full.must_change.recall.similar | 0.439 | 11 |
| full.must_change.recall.any | 0.87 | 11 |
| full.relevant_files.recall.must_read | 0.432 | 11 |
| full.relevant_files.recall.pack | 0.581 | 11 |
| full.relevant_files.recall.search | 0.726 | 11 |
| full.relevant_files.recall.similar | 0.365 | 11 |
| full.relevant_files.recall.any | 0.842 | 11 |
| full.must_consider.recall.must_read | 0.273 | 11 |
| full.must_consider.recall.pack | 0.386 | 11 |
| full.must_consider.recall.search | 0.75 | 11 |
| full.must_consider.recall.similar | 0.341 | 11 |
| full.must_consider.recall.any | 0.826 | 11 |
| full.relevant_files.precision.must_read | 0.378 | 11 |
| full.relevant_files.precision.pack | 0.237 | 11 |
| full.relevant_files.precision.search | 0.113 | 11 |
| full.relevant_files.precision.similar | 0.186 | 11 |
| full.relevant_files.precision.any | 0.097 | 11 |
| full.relevant_files.f05.must_read | 0.365 | 11 |
| full.relevant_files.f05.pack | 0.26 | 11 |
| full.relevant_files.f05.search | 0.136 | 11 |
| full.relevant_files.f05.similar | 0.19 | 11 |
| full.relevant_files.f05.any | 0.117 | 11 |
| full.must_change.f05.must_read | 0.223 | 11 |
| full.must_change.f05.pack | 0.182 | 11 |
| full.must_change.f05.search | 0.075 | 11 |
| full.must_change.f05.similar | 0.11 | 11 |
| full.must_change.f05.any | 0.071 | 11 |
| full.lines.precision.must_read | 0.006 | 10 |
| full.lines.recall.must_read | 0.135 | 10 |
| full.lines.f05.must_read | 0.008 | 10 |
| full.lines.precision.pack | 0.007 | 10 |
| full.lines.recall.pack | 0.221 | 10 |
| full.lines.f05.pack | 0.009 | 10 |
| full.yield.4000.must_read | 0.555 | 11 |
| full.yield.4000.pack | 0.776 | 11 |
| full.yield.8000.must_read | 0.555 | 11 |
| full.yield.8000.pack | 0.776 | 11 |
| full.yield.16000.must_read | 0.555 | 11 |
| full.yield.16000.pack | 0.776 | 11 |
| full.reuse_targets.recall.pack | 0.4 | 4 |
| full.reuse_targets.recall.search | 0.6 | 4 |
| full.reuse_targets.recall.similar | 0.05 | 4 |
| full.reuse_targets.recall.any | 0.7 | 4 |
| full.pack.bytes | 22961.727 | 11 |
| full.pack.estimated_tokens | 3441.091 | 11 |
| agent.must_change.recall.must_read | 0.287 | 11 |
| agent.must_change.recall.pack | 0.417 | 11 |
| agent.must_change.recall.search | 0.68 | 11 |
| agent.must_change.recall.similar | 0.402 | 11 |
| agent.must_change.recall.any | 0.767 | 11 |
| agent.relevant_files.recall.must_read | 0.301 | 11 |
| agent.relevant_files.recall.pack | 0.464 | 11 |
| agent.relevant_files.recall.search | 0.601 | 11 |
| agent.relevant_files.recall.similar | 0.35 | 11 |
| agent.relevant_files.recall.any | 0.778 | 11 |
| agent.must_consider.recall.must_read | 0.28 | 11 |
| agent.must_consider.recall.pack | 0.439 | 11 |
| agent.must_consider.recall.search | 0.572 | 11 |
| agent.must_consider.recall.similar | 0.295 | 11 |
| agent.must_consider.recall.any | 0.716 | 11 |
| agent.relevant_files.precision.must_read | 0.67 | 10 |
| agent.relevant_files.precision.pack | 0.405 | 10 |
| agent.relevant_files.precision.search | 0.272 | 11 |
| agent.relevant_files.precision.similar | 0.27 | 11 |
| agent.relevant_files.precision.any | 0.213 | 11 |
| agent.relevant_files.f05.must_read | 0.45 | 11 |
| agent.relevant_files.f05.pack | 0.365 | 11 |
| agent.relevant_files.f05.search | 0.299 | 11 |
| agent.relevant_files.f05.similar | 0.232 | 11 |
| agent.relevant_files.f05.any | 0.242 | 11 |
| agent.must_change.f05.must_read | 0.274 | 11 |
| agent.must_change.f05.pack | 0.214 | 11 |
| agent.must_change.f05.search | 0.16 | 11 |
| agent.must_change.f05.similar | 0.177 | 11 |
| agent.must_change.f05.any | 0.131 | 11 |
| agent.lines.precision.must_read | 0.099 | 9 |
| agent.lines.recall.must_read | 0.164 | 10 |
| agent.lines.f05.must_read | 0.094 | 10 |
| agent.lines.precision.pack | 0.013 | 9 |
| agent.lines.recall.pack | 0.238 | 10 |
| agent.lines.f05.pack | 0.014 | 10 |
| agent.yield.4000.must_read | 0.287 | 11 |
| agent.yield.4000.pack | 0.417 | 11 |
| agent.yield.8000.must_read | 0.287 | 11 |
| agent.yield.8000.pack | 0.417 | 11 |
| agent.yield.16000.must_read | 0.287 | 11 |
| agent.yield.16000.pack | 0.417 | 11 |
| agent.reuse_targets.recall.pack | 0.725 | 4 |
| agent.reuse_targets.recall.search | 0.325 | 4 |
| agent.reuse_targets.recall.similar | 0.175 | 4 |
| agent.reuse_targets.recall.any | 0.8 | 4 |
| agent.pack.bytes | 11435.136 | 11 |
| agent.pack.estimated_tokens | 1893.909 | 11 |
| agent-seeded.must_change.recall.must_read | 0.287 | 11 |
| agent-seeded.must_change.recall.pack | 0.417 | 11 |
| agent-seeded.must_change.recall.search | 0.68 | 11 |
| agent-seeded.must_change.recall.similar | 0.402 | 11 |
| agent-seeded.must_change.recall.any | 0.767 | 11 |
| agent-seeded.relevant_files.recall.must_read | 0.301 | 11 |
| agent-seeded.relevant_files.recall.pack | 0.464 | 11 |
| agent-seeded.relevant_files.recall.search | 0.601 | 11 |
| agent-seeded.relevant_files.recall.similar | 0.35 | 11 |
| agent-seeded.relevant_files.recall.any | 0.778 | 11 |
| agent-seeded.must_consider.recall.must_read | 0.28 | 11 |
| agent-seeded.must_consider.recall.pack | 0.439 | 11 |
| agent-seeded.must_consider.recall.search | 0.572 | 11 |
| agent-seeded.must_consider.recall.similar | 0.295 | 11 |
| agent-seeded.must_consider.recall.any | 0.716 | 11 |
| agent-seeded.relevant_files.precision.must_read | 0.67 | 10 |
| agent-seeded.relevant_files.precision.pack | 0.405 | 10 |
| agent-seeded.relevant_files.precision.search | 0.272 | 11 |
| agent-seeded.relevant_files.precision.similar | 0.27 | 11 |
| agent-seeded.relevant_files.precision.any | 0.213 | 11 |
| agent-seeded.relevant_files.f05.must_read | 0.45 | 11 |
| agent-seeded.relevant_files.f05.pack | 0.365 | 11 |
| agent-seeded.relevant_files.f05.search | 0.299 | 11 |
| agent-seeded.relevant_files.f05.similar | 0.232 | 11 |
| agent-seeded.relevant_files.f05.any | 0.242 | 11 |
| agent-seeded.must_change.f05.must_read | 0.274 | 11 |
| agent-seeded.must_change.f05.pack | 0.214 | 11 |
| agent-seeded.must_change.f05.search | 0.16 | 11 |
| agent-seeded.must_change.f05.similar | 0.177 | 11 |
| agent-seeded.must_change.f05.any | 0.131 | 11 |
| agent-seeded.lines.precision.must_read | 0.099 | 9 |
| agent-seeded.lines.recall.must_read | 0.164 | 10 |
| agent-seeded.lines.f05.must_read | 0.094 | 10 |
| agent-seeded.lines.precision.pack | 0.013 | 9 |
| agent-seeded.lines.recall.pack | 0.238 | 10 |
| agent-seeded.lines.f05.pack | 0.014 | 10 |
| agent-seeded.yield.4000.must_read | 0.287 | 11 |
| agent-seeded.yield.4000.pack | 0.417 | 11 |
| agent-seeded.yield.8000.must_read | 0.287 | 11 |
| agent-seeded.yield.8000.pack | 0.417 | 11 |
| agent-seeded.yield.16000.must_read | 0.287 | 11 |
| agent-seeded.yield.16000.pack | 0.417 | 11 |
| agent-seeded.reuse_targets.recall.pack | 0.725 | 4 |
| agent-seeded.reuse_targets.recall.search | 0.325 | 4 |
| agent-seeded.reuse_targets.recall.similar | 0.175 | 4 |
| agent-seeded.reuse_targets.recall.any | 0.8 | 4 |
| agent-seeded.pack.bytes | 11458.864 | 11 |
| agent-seeded.pack.estimated_tokens | 1893.909 | 11 |
| first-line.must_change.recall.must_read | 0.344 | 11 |
| first-line.must_change.recall.pack | 0.488 | 11 |
| first-line.must_change.recall.search | 0.728 | 11 |
| first-line.must_change.recall.similar | 0.458 | 11 |
| first-line.must_change.recall.any | 0.857 | 11 |
| first-line.relevant_files.recall.must_read | 0.252 | 11 |
| first-line.relevant_files.recall.pack | 0.409 | 11 |
| first-line.relevant_files.recall.search | 0.497 | 11 |
| first-line.relevant_files.recall.similar | 0.303 | 11 |
| first-line.relevant_files.recall.any | 0.671 | 11 |
| first-line.must_consider.recall.must_read | 0.091 | 11 |
| first-line.must_consider.recall.pack | 0.212 | 11 |
| first-line.must_consider.recall.search | 0.455 | 11 |
| first-line.must_consider.recall.similar | 0.205 | 11 |
| first-line.must_consider.recall.any | 0.576 | 11 |
| first-line.relevant_files.precision.must_read | 0.421 | 11 |
| first-line.relevant_files.precision.pack | 0.261 | 11 |
| first-line.relevant_files.precision.search | 0.154 | 11 |
| first-line.relevant_files.precision.similar | 0.174 | 11 |
| first-line.relevant_files.precision.any | 0.128 | 11 |
| first-line.relevant_files.f05.must_read | 0.321 | 11 |
| first-line.relevant_files.f05.pack | 0.273 | 11 |
| first-line.relevant_files.f05.search | 0.172 | 11 |
| first-line.relevant_files.f05.similar | 0.171 | 11 |
| first-line.relevant_files.f05.any | 0.15 | 11 |
| first-line.must_change.f05.must_read | 0.321 | 11 |
| first-line.must_change.f05.pack | 0.215 | 11 |
| first-line.must_change.f05.search | 0.108 | 11 |
| first-line.must_change.f05.similar | 0.124 | 11 |
| first-line.must_change.f05.any | 0.1 | 11 |
| first-line.lines.precision.must_read | 0.068 | 10 |
| first-line.lines.recall.must_read | 0.106 | 10 |
| first-line.lines.f05.must_read | 0.07 | 10 |
| first-line.lines.precision.pack | 0.01 | 10 |
| first-line.lines.recall.pack | 0.209 | 10 |
| first-line.lines.f05.pack | 0.012 | 10 |
| first-line.yield.4000.must_read | 0.344 | 11 |
| first-line.yield.4000.pack | 0.488 | 11 |
| first-line.yield.8000.must_read | 0.344 | 11 |
| first-line.yield.8000.pack | 0.488 | 11 |
| first-line.yield.16000.must_read | 0.344 | 11 |
| first-line.yield.16000.pack | 0.488 | 11 |
| first-line.reuse_targets.recall.pack | 0 | 4 |
| first-line.reuse_targets.recall.search | 0.25 | 4 |
| first-line.reuse_targets.recall.similar | 0.5 | 4 |
| first-line.reuse_targets.recall.any | 0.5 | 4 |
| first-line.pack.bytes | 12261.818 | 11 |
| first-line.pack.estimated_tokens | 1883.727 | 11 |
| blast_radius.recall.visible | 1 | 2 |
| blast_radius.recall.full | 1 | 2 |

## t00-smoke: Smoke: soft-delete timestamp on the users entity

Categories: smoke.

Index: 65 files in acme-api, acme-sdk, acme-web, acme-billing.
Line gold: none (the task has no readable reference.patch).

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 1 | 0.333 | 0.385 | n/a | 1 | 1 | 1 |
| full | 1 | whole pack | 1 | 0.111 | 0.135 | n/a | 1 | 1 | 1 |
| agent | 1 | MUST READ | 0 | 0.167 | 0.192 | n/a | 0 | 0 | 0 |
| agent | 1 | whole pack | 0 | 0.062 | 0.076 | n/a | 0 | 0 | 0 |
| agent-seeded | 1 | MUST READ | 0 | 0.167 | 0.192 | n/a | 0 | 0 | 0 |
| agent-seeded | 1 | whole pack | 0 | 0.062 | 0.076 | n/a | 0 | 0 | 0 |
| first-line | 1 | MUST READ | 1 | 0.5 | 0.556 | n/a | 1 | 1 | 1 |
| first-line | 1 | whole pack | 1 | 0.222 | 0.263 | n/a | 1 | 1 | 1 |

### `full`

Query: the task prompt (308 characters).
Context pack: 10948 bytes, ~1967 of 6000 tokens; items: must read 13, skim 13, patterns 0, reuse 0, impact 6, tests 5, uncertain 2, omitted 8.

Searches: the query, then `deletedAt`, `User`, `deleted_at`, `UsersService.findOne`, `database column named`, `soft-delete column decorator`, `change minimal`.
Similar seeds: `ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method` (members 10, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::remove#method` (members 1, shape-only 0), `ts:acme-web:acme-web:src/hooks/useUser::useUser#function` (members 3, shape-only 0), `rust:acme-billing:acme-billing:store::PgInvoiceStore::database_url#method` (members 1, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::update#method` (members 5, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| must_consider recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) |
| relevant_files precision | 0.333 | 0.111 | 0.143 | 0.1 | 0.083 |
| relevant_files F0.5 | 0.385 | 0.135 | 0.172 | 0.119 | 0.102 |
| must_change F0.5 | 0.2 | 0.068 | 0.088 | 0 | 0.052 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1967 | 10948 | 1 (1/1) | 1 (1/1) |
| 8000 | 1967 | 10948 | 1 (1/1) | 1 (1/1) |
| 16000 | 1967 | 10949 | 1 (1/1) | 1 (1/1) |

### `agent`

Query: `Add nullable deletedAt soft-delete column to User entity in acme-api and return it from UsersService.findOne`; sent by `2026-10-05-directed/t00-smoke.atlas.r0`, `2026-10-05-directed/t00-smoke.atlas.r1`.
Context pack: 11141 bytes, ~2086 of 6000 tokens; items: must read 13, skim 15, patterns 0, reuse 0, impact 6, tests 5, uncertain 2, omitted 6.

Searches: the query, then `nullable deletedAt soft-delete column`, `User entity`.
Similar seeds: `ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method` (members 10, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::remove#method` (members 1, shape-only 0), `ts:acme-web:acme-web:src/hooks/useUser::useUser#function` (members 3, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.5 (1/2) | 0.5 (1/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| must_consider recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) |
| relevant_files precision | 0.167 | 0.062 | 0.222 | 0.125 | 0.091 |
| relevant_files F0.5 | 0.192 | 0.076 | 0.263 | 0.147 | 0.111 |
| must_change F0.5 | 0 | 0 | 0.135 | 0 | 0.056 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2086 | 11141 | 0 (0/1) | 0 (0/1) |
| 8000 | 2086 | 11141 | 0 (0/1) | 0 (0/1) |
| 16000 | 2086 | 11142 | 0 (0/1) | 0 (0/1) |

### `agent-seeded`

Query: `Add nullable deletedAt soft-delete column to User entity in acme-api and return it from UsersService.findOne`; seeds `UsersService.findOne`, `User`; sent by `2026-10-05-directed/t00-smoke.atlas.r0`, `2026-10-05-directed/t00-smoke.atlas.r1`.
Context pack: 11177 bytes, ~2086 of 6000 tokens; items: must read 13, skim 15, patterns 0, reuse 0, impact 6, tests 5, uncertain 2, omitted 7.

Searches: the query, then `nullable deletedAt soft-delete column`, `User entity`.
Similar seeds: `ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method` (members 10, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::remove#method` (members 1, shape-only 0), `ts:acme-web:acme-web:src/hooks/useUser::useUser#function` (members 3, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.5 (1/2) | 0.5 (1/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| must_consider recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) | 1 (2/2) |
| relevant_files precision | 0.167 | 0.062 | 0.222 | 0.125 | 0.091 |
| relevant_files F0.5 | 0.192 | 0.076 | 0.263 | 0.147 | 0.111 |
| must_change F0.5 | 0 | 0 | 0.135 | 0 | 0.056 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2086 | 11177 | 0 (0/1) | 0 (0/1) |
| 8000 | 2086 | 11177 | 0 (0/1) | 0 (0/1) |
| 16000 | 2086 | 11178 | 0 (0/1) | 0 (0/1) |

### `first-line`

Query: `In acme-api, add a nullable 'deletedAt' timestamp to the 'User' entity, stored in a database column named 'deleted_at' (use TypeORM's soft-delete column decorator)`.
Context pack: 5765 bytes, ~1056 of 6000 tokens; items: must read 5, skim 6, patterns 0, reuse 0, impact 2, tests 1, uncertain 1, omitted 12.

Searches: the query, then `deletedAt`, `User`, `deleted_at`, `database column named`, `soft-delete column decorator`.
Similar seeds: `ts:acme-api:acme-api:src/common/database.config::databaseConfig#function` (members 1, shape-only 0), `ts:acme-api:acme-api:src/users/users.controller::UsersController::remove#method` (members 1, shape-only 0), `ts:acme-web:acme-web:src/hooks/useUser::useUser#function` (members 3, shape-only 0), `rust:acme-billing:acme-billing:store::PgInvoiceStore::database_url#method` (members 1, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| must_consider recall | 1 (2/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| relevant_files precision | 0.5 | 0.222 | 0.182 | 0.2 | 0.125 |
| relevant_files F0.5 | 0.556 | 0.263 | 0.217 | 0.227 | 0.152 |
| must_change F0.5 | 0.294 | 0.135 | 0.111 | 0 | 0.077 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1056 | 5765 | 1 (1/1) | 1 (1/1) |
| 8000 | 1056 | 5765 | 1 (1/1) | 1 (1/1) |
| 16000 | 1056 | 5766 | 1 (1/1) | 1 (1/1) |

## t01-cwd-relative-targets: CLI query targets accept paths relative to the working directory

Categories: find-and-extend, reuse.

Index: 491 files in tree.
Line gold: 15 old-side lines in 1 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 1 | 0.083 | 0.094 | 0 | 1 | 1 | 1 |
| full | 1 | whole pack | 1 | 0.033 | 0.04 | 0 | 1 | 1 | 1 |
| agent | 2 | MUST READ | 1 | 0.268 | 0.271 | 0.011 | 1 | 1 | 1 |
| agent | 2 | whole pack | 1 | 0.068 | 0.081 | 0.004 | 1 | 1 | 1 |
| agent-seeded | 2 | MUST READ | 1 | 0.268 | 0.271 | 0.011 | 1 | 1 | 1 |
| agent-seeded | 2 | whole pack | 1 | 0.068 | 0.081 | 0.004 | 1 | 1 | 1 |
| first-line | 1 | MUST READ | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| first-line | 1 | whole pack | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### `full`

Query: the task prompt (1169 characters).
Context pack: 24995 bytes, ~3502 of 6000 tokens; items: must read 22, skim 14, patterns 0, reuse 0, impact 6, tests 8, uncertain 10, omitted 63.

Searches: the query, then `code-atlas query similar --snippet`, `acme-billing/src`, `--snippet money.rs:1-15`, `acme-api/src/users`, `code-atlas query symbol users.service.ts:18`, `query callers ./users.service.ts:18`, `code-atlas query`, `symbol`, `path relative`, `understands paths relative`, `repository root`, `absolute path`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::run#function` (members 10, shape-only 0), `rust:tree:atlas-query:queries/similar::snippet_target#function` (members 7, shape-only 0), `rust:tree:atlas-resolve/test/acme:::billing#function` (members 10, shape-only 0), `ts:tree:globex-api:src/util/money::formatMoney#function` (members 6, shape-only 0), `ts:tree:acme-web:src/hooks/useMe::useMe#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.2 (1/5) | 0.2 (1/5) | 0.6 (3/5) | 0.4 (2/5) | 0.8 (4/5) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.5 (2/4) | 0.25 (1/4) | 0.75 (3/4) |
| relevant_files precision | 0.083 | 0.033 | 0.048 | 0.069 | 0.041 |
| relevant_files F0.5 | 0.094 | 0.04 | 0.058 | 0.083 | 0.05 |
| must_change F0.5 | 0.102 | 0.041 | 0.02 | 0.043 | 0.013 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/src/context.rs`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/context.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 22 | 0 | 241 | 0/15 | 0 | 0 | 0 |
| whole pack | 60 | 0 | 736 | 0/15 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3502 | 24995 | 1 (1/1) | 1 (1/1) |
| 8000 | 3502 | 24995 | 1 (1/1) | 1 (1/1) |
| 16000 | 3502 | 24996 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | no | yes | no |

### `agent`, query 1 of 2

Query: `Make CLI query file targets resolve against cwd like the similar --snippet option does`; sent by `2026-10-05-directed/t01-cwd-relative-targets.atlas.r0`.
Context pack: 20702 bytes, ~3839 of 6000 tokens; items: must read 8, skim 32, patterns 0, reuse 0, impact 0, tests 3, uncertain 2, omitted 20.

Searches: the query, then `targets resolve against cwd`, `similar snippet option`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::similar#function` (members 2, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/doctor::cwd_project#function` (members 6, shape-only 0), `rust:tree:atlas-eval:agent/atlas_cli::AtlasCli::similar_snippet#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.2 (1/5) | 0.2 (1/5) | 0.8 (4/5) | 0.4 (2/5) | 1 (5/5) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 1 (4/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | 0.25 | 0.045 | 0.25 | 0.154 | 0.119 |
| relevant_files F0.5 | 0.238 | 0.054 | 0.29 | 0.175 | 0.145 |
| must_change F0.5 | 0.294 | 0.056 | 0.077 | 0.094 | 0.03 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 107 | 1/15 | 0.009 | 0.067 | 0.011 |
| whole pack | 45 | 0 | 553 | 2/15 | 0.004 | 0.133 | 0.004 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3839 | 20702 | 1 (1/1) | 1 (1/1) |
| 8000 | 3839 | 20702 | 1 (1/1) | 1 (1/1) |
| 16000 | 3839 | 20703 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | yes | no | no |

### `agent`, query 2 of 2

Query: `CLI query subcommands: resolve file targets against cwd like the --snippet option does`; sent by `2026-10-05-directed/t01-cwd-relative-targets.atlas.r1`.
Context pack: 18940 bytes, ~3380 of 6000 tokens; items: must read 10, skim 30, patterns 0, reuse 0, impact 0, tests 8, uncertain 3, omitted 2.

Searches: the query, then `CLI query subcommands`, `file targets against cwd`, `snippet option`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::snippet_target#function` (members 10, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:context::Context::open_query#method` (members 10, shape-only 0), `rust:tree:atlas-project:manifest::PackageGraph::rust_targets_for_file#method` (members 10, shape-only 0), `rust:tree:atlas-engine:enrich::Engine::snippet#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.4 (2/5) | 0.4 (2/5) | 0.8 (4/5) | 0.4 (2/5) | 0.8 (4/5) |
| must_consider recall | 0.25 (1/4) | 0.5 (2/4) | 0.75 (3/4) | 0.5 (2/4) | 1 (4/4) |
| relevant_files precision | 0.286 | 0.091 | 0.2 | 0.074 | 0.078 |
| relevant_files F0.5 | 0.303 | 0.108 | 0.235 | 0.088 | 0.096 |
| must_change F0.5 | 0.172 | 0.056 | 0.062 | 0.046 | 0.024 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/tests/cli.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 10 | 0 | 118 | 1/15 | 0.008 | 0.067 | 0.01 |
| whole pack | 51 | 0 | 987 | 2/15 | 0.002 | 0.133 | 0.003 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3380 | 18940 | 1 (1/1) | 1 (1/1) |
| 8000 | 3380 | 18940 | 1 (1/1) | 1 (1/1) |
| 16000 | 3380 | 18941 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | yes | no | no |

### `agent-seeded`, query 1 of 2

Query: `Make CLI query file targets resolve against cwd like the similar --snippet option does`; sent by `2026-10-05-directed/t01-cwd-relative-targets.atlas.r0`.
Context pack: 20702 bytes, ~3839 of 6000 tokens; items: must read 8, skim 32, patterns 0, reuse 0, impact 0, tests 3, uncertain 2, omitted 20.

Searches: the query, then `targets resolve against cwd`, `similar snippet option`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::similar#function` (members 2, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/doctor::cwd_project#function` (members 6, shape-only 0), `rust:tree:atlas-eval:agent/atlas_cli::AtlasCli::similar_snippet#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.2 (1/5) | 0.2 (1/5) | 0.8 (4/5) | 0.4 (2/5) | 1 (5/5) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 1 (4/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | 0.25 | 0.045 | 0.25 | 0.154 | 0.119 |
| relevant_files F0.5 | 0.238 | 0.054 | 0.29 | 0.175 | 0.145 |
| must_change F0.5 | 0.294 | 0.056 | 0.077 | 0.094 | 0.03 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 107 | 1/15 | 0.009 | 0.067 | 0.011 |
| whole pack | 45 | 0 | 553 | 2/15 | 0.004 | 0.133 | 0.004 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3839 | 20702 | 1 (1/1) | 1 (1/1) |
| 8000 | 3839 | 20702 | 1 (1/1) | 1 (1/1) |
| 16000 | 3839 | 20703 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | yes | no | no |

### `agent-seeded`, query 2 of 2

Query: `CLI query subcommands: resolve file targets against cwd like the --snippet option does`; seeds `crates/atlas-cli`; sent by `2026-10-05-directed/t01-cwd-relative-targets.atlas.r1`.
Context pack: 19021 bytes, ~3380 of 6000 tokens; items: must read 10, skim 30, patterns 0, reuse 0, impact 0, tests 8, uncertain 3, omitted 3.

Searches: the query, then `CLI query subcommands`, `file targets against cwd`, `snippet option`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/query::snippet_target#function` (members 10, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:context::Context::open_query#method` (members 10, shape-only 0), `rust:tree:atlas-project:manifest::PackageGraph::rust_targets_for_file#method` (members 10, shape-only 0), `rust:tree:atlas-engine:enrich::Engine::snippet#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.4 (2/5) | 0.4 (2/5) | 0.8 (4/5) | 0.4 (2/5) | 0.8 (4/5) |
| must_consider recall | 0.25 (1/4) | 0.5 (2/4) | 0.75 (3/4) | 0.5 (2/4) | 1 (4/4) |
| relevant_files precision | 0.286 | 0.091 | 0.2 | 0.074 | 0.078 |
| relevant_files F0.5 | 0.303 | 0.108 | 0.235 | 0.088 | 0.096 |
| must_change F0.5 | 0.172 | 0.056 | 0.062 | 0.046 | 0.024 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/tests/cli.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 10 | 0 | 118 | 1/15 | 0.008 | 0.067 | 0.01 |
| whole pack | 51 | 0 | 987 | 2/15 | 0.002 | 0.133 | 0.003 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3380 | 19021 | 1 (1/1) | 1 (1/1) |
| 8000 | 3380 | 19021 | 1 (1/1) | 1 (1/1) |
| 16000 | 3380 | 19022 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | yes | no | no |

### `first-line`

Query: `'code-atlas query similar --snippet' lets me pass a path relative to where I run the command: from inside 'acme-billing/src' I can write '--snippet money.rs:1-15' and it works`.
Context pack: 13619 bytes, ~1953 of 6000 tokens; items: must read 11, skim 18, patterns 0, reuse 0, impact 0, tests 8, uncertain 1, omitted 5.

Searches: the query, then `code-atlas query similar --snippet`, `acme-billing/src`, `--snippet money.rs:1-15`, `path relative`.
Similar seeds: `rust:tree:atlas-eval:agent/atlas_cli::AtlasCli::similar_snippet#method` (members 9, shape-only 0), `rust:tree:atlas-query:queries/similar::snippet_target#function` (members 7, shape-only 0), `rust:tree:atlas-resolve/test/acme:::billing#function` (members 10, shape-only 0), `ts:tree:globex-api:src/util/money::formatMoney#function` (members 6, shape-only 0), `rust:tree:atlas-engine:scip::relative_to#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0 (0/5) | 0 (0/5) | 0.2 (1/5) | 0.6 (3/5) | 0.6 (3/5) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 0.25 (1/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files precision | 0 | 0 | 0.036 | 0.094 | 0.05 |
| relevant_files F0.5 | 0 | 0 | 0.043 | 0.113 | 0.061 |
| must_change F0.5 | 0 | 0 | 0.044 | 0.039 | 0.021 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/src/cli.rs`, `crates/atlas-cli/src/context.rs`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/cli.rs`, `crates/atlas-cli/src/context.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 11 | 0 | 73 | 0/15 | 0 | 0 | 0 |
| whole pack | 38 | 0 | 310 | 0/15 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1953 | 13619 | 0 (0/1) | 0 (0/1) |
| 8000 | 1953 | 13619 | 0 (0/1) | 0 (0/1) |
| 16000 | 1953 | 13620 | 0 (0/1) | 0 (0/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `query::repo_relative` | no | yes | yes |

## t02-typed-degraded-cause: Typed cause for degraded freshness reports

Categories: blast-radius.

Index: 491 files in tree.
Line gold: 31 old-side lines in 7 base files of `reference.patch`, 2 of them in files outside the index.
- gold files outside the index: `CLAUDE.md`

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.5 | 0.4 | 0.4 | 0.027 | 0.5 | 0.5 | 0.5 |
| full | 1 | whole pack | 1 | 0.389 | 0.427 | 0.028 | 1 | 1 | 1 |
| agent | 2 | MUST READ | 0.5 | 1 | 0.682 | 0.063 | 0.5 | 0.5 | 0.5 |
| agent | 2 | whole pack | 0.833 | 0.714 | 0.658 | 0.038 | 0.833 | 0.833 | 0.833 |
| agent-seeded | 2 | MUST READ | 0.5 | 1 | 0.682 | 0.063 | 0.5 | 0.5 | 0.5 |
| agent-seeded | 2 | whole pack | 0.833 | 0.714 | 0.658 | 0.038 | 0.833 | 0.833 | 0.833 |
| first-line | 1 | MUST READ | 0.667 | 0.8 | 0.667 | 0.042 | 0.667 | 0.667 | 0.667 |
| first-line | 1 | whole pack | 1 | 0.429 | 0.455 | 0.03 | 1 | 1 | 1 |

### `full`

Query: the task prompt (1760 characters).
Context pack: 28978 bytes, ~4077 of 6000 tokens; items: must read 24, skim 14, patterns 0, reuse 1, impact 4, tests 8, uncertain 2, omitted 32.

Searches: the query, then `code-atlas watch`, `FreshnessReport::Degraded`, `cause: DegradedCause`, `reason: String`, `DegradedCause`, `atlas_engine`, `ProjectConfigChanged`, `UpdateFailed { error: String }`, `process changed`, `project configuration`, `project configuration changed`, `free-text reason`.
Similar seeds: `rust:tree:atlas-engine:engine::Engine::barrier#method` (members 3, shape-only 0), `rust:tree:atlas-mcp:options::ServeOptions::with_watch#method` (members 3, shape-only 0), `rust:tree:atlas-query:freshness::Freshness::from_report#method` (members 3, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:output::Outcome::degraded_if#method` (members 2, shape-only 0), `rust:tree:atlas-resolve/test/workspace_quality:::reason_label#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (3/6) | 1 (6/6) | 0.833 (5/6) | 0.333 (2/6) | 1 (6/6) |
| relevant_files recall | 0.4 (4/10) | 0.7 (7/10) | 0.8 (8/10) | 0.3 (3/10) | 1 (10/10) |
| must_consider recall | 0.167 (1/6) | 0.167 (1/6) | 0.167 (1/6) | 0.333 (2/6) | 0.5 (3/6) |
| relevant_files precision | 0.4 | 0.389 | 0.143 | 0.167 | 0.141 |
| relevant_files F0.5 | 0.4 | 0.427 | 0.171 | 0.183 | 0.17 |
| must_change F0.5 | 0.326 | 0.385 | 0.109 | 0.128 | 0.103 |

- relevant_files unindexable, never in the index (not scored): `CLAUDE.md`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/enrich.rs`, `crates/atlas-cli/src/commands/scip.rs`, `crates/atlas-mcp/src/server.rs`
- must_consider unindexable, never in the index (not scored): `CLAUDE.md`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 24 | 0 | 267 | 6/31 | 0.022 | 0.194 | 0.027 |
| whole pack | 53 | 0 | 670 | 15/31 | 0.022 | 0.484 | 0.028 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3998 | 28661 | 0.5 (3/6) | 1 (6/6) |
| 8000 | 4077 | 28978 | 0.5 (3/6) | 1 (6/6) |
| 16000 | 4077 | 28979 | 0.5 (3/6) | 1 (6/6) |

### `agent`, query 1 of 2

Query: `Replace FreshnessReport::Degraded reason String with typed DegradedCause`; sent by `2026-10-05-directed/t02-typed-degraded-cause.atlas.r0`.
Context pack: 14304 bytes, ~1866 of 6000 tokens; items: must read 9, skim 6, patterns 0, reuse 0, impact 2, tests 8, uncertain 2, omitted 29.

Searches: the query, then `Replace FreshnessReport`, `Degraded reason String`, `typed DegradedCause`.
Similar seeds: `rust:tree:atlas-engine:engine::Engine::degraded#method` (members 10, shape-only 0), `rust:tree:atlas-query:freshness::Freshness::from_report#method` (members 3, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:output::Outcome::degraded_if#method` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (3/6) | 0.833 (5/6) | 0.667 (4/6) | 0.333 (2/6) | 1 (6/6) |
| relevant_files recall | 0.3 (3/10) | 0.5 (5/10) | 0.4 (4/10) | 0.3 (3/10) | 0.7 (7/10) |
| must_consider recall | 0 (0/6) | 0 (0/6) | 0 (0/6) | 0.167 (1/6) | 0.167 (1/6) |
| relevant_files precision | 1 | 0.714 | 0.267 | 0.429 | 0.292 |
| relevant_files F0.5 | 0.682 | 0.658 | 0.286 | 0.395 | 0.33 |
| must_change F0.5 | 0.833 | 0.735 | 0.303 | 0.294 | 0.294 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/tests/cli.rs`, `crates/atlas-engine/src/error.rs`, `crates/atlas-engine/src/lib.rs`
- relevant_files unindexable, never in the index (not scored): `CLAUDE.md`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/debug.rs`, `crates/atlas-cli/src/commands/enrich.rs`, `crates/atlas-cli/src/commands/scip.rs`, `crates/atlas-engine/src/error.rs`, `crates/atlas-mcp/src/server.rs`
- must_consider unindexable, never in the index (not scored): `CLAUDE.md`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 9 | 0 | 112 | 6/31 | 0.054 | 0.194 | 0.063 |
| whole pack | 27 | 0 | 484 | 15/31 | 0.031 | 0.484 | 0.038 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1866 | 14304 | 0.5 (3/6) | 0.833 (5/6) |
| 8000 | 1866 | 14304 | 0.5 (3/6) | 0.833 (5/6) |
| 16000 | 1866 | 14305 | 0.5 (3/6) | 0.833 (5/6) |

### `agent`, query 2 of 2

Query: `Replace FreshnessReport::Degraded reason String with typed DegradedCause enum`; sent by `2026-10-05-directed/t02-typed-degraded-cause.atlas.r1`.
Context pack: 14309 bytes, ~1866 of 6000 tokens; items: must read 9, skim 6, patterns 0, reuse 0, impact 2, tests 8, uncertain 2, omitted 29.

Searches: the query, then `Replace FreshnessReport`, `Degraded reason String`, `typed DegradedCause enum`.
Similar seeds: `rust:tree:atlas-engine:engine::Engine::degraded#method` (members 10, shape-only 0), `rust:tree:atlas-query:freshness::Freshness::from_report#method` (members 3, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:output::Outcome::degraded_if#method` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (3/6) | 0.833 (5/6) | 0.667 (4/6) | 0.333 (2/6) | 1 (6/6) |
| relevant_files recall | 0.3 (3/10) | 0.5 (5/10) | 0.5 (5/10) | 0.3 (3/10) | 0.7 (7/10) |
| must_consider recall | 0 (0/6) | 0 (0/6) | 0.167 (1/6) | 0.167 (1/6) | 0.167 (1/6) |
| relevant_files precision | 1 | 0.714 | 0.312 | 0.429 | 0.292 |
| relevant_files F0.5 | 0.682 | 0.658 | 0.338 | 0.395 | 0.33 |
| must_change F0.5 | 0.833 | 0.735 | 0.286 | 0.294 | 0.294 |

- relevant_files not retrieved anywhere: `crates/atlas-cli/tests/cli.rs`, `crates/atlas-engine/src/error.rs`, `crates/atlas-engine/src/lib.rs`
- relevant_files unindexable, never in the index (not scored): `CLAUDE.md`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/debug.rs`, `crates/atlas-cli/src/commands/enrich.rs`, `crates/atlas-cli/src/commands/scip.rs`, `crates/atlas-engine/src/error.rs`, `crates/atlas-mcp/src/server.rs`
- must_consider unindexable, never in the index (not scored): `CLAUDE.md`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 9 | 0 | 112 | 6/31 | 0.054 | 0.194 | 0.063 |
| whole pack | 27 | 0 | 484 | 15/31 | 0.031 | 0.484 | 0.038 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1866 | 14309 | 0.5 (3/6) | 0.833 (5/6) |
| 8000 | 1866 | 14309 | 0.5 (3/6) | 0.833 (5/6) |
| 16000 | 1866 | 14310 | 0.5 (3/6) | 0.833 (5/6) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `'code-atlas watch' decides whether another process changed the project configuration by looking for the phrase "project configuration changed" inside the free-text reason of a degraded freshness report`.
Context pack: 11938 bytes, ~1660 of 6000 tokens; items: must read 11, skim 15, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 0.

Searches: the query, then `code-atlas watch`, `process changed`, `project configuration`, `project configuration changed`, `free-text reason`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/watch::config_changed#function` (members 7, shape-only 0), `rust:tree:atlas-mcp:options::ServeOptions::with_watch#method` (members 3, shape-only 0), `rust:tree:atlas-engine:run::Pipeline::update_similarity#method` (members 9, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/project::apply_settings#function` (members 8, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/project::changed_fields#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.667 (4/6) | 1 (6/6) | 0.667 (4/6) | 0.333 (2/6) | 1 (6/6) |
| relevant_files recall | 0.4 (4/10) | 0.6 (6/10) | 0.7 (7/10) | 0.3 (3/10) | 1 (10/10) |
| must_consider recall | 0 (0/6) | 0 (0/6) | 0.333 (2/6) | 0 (0/6) | 0.333 (2/6) |
| relevant_files precision | 0.8 | 0.429 | 0.219 | 0.188 | 0.204 |
| relevant_files F0.5 | 0.667 | 0.455 | 0.254 | 0.203 | 0.243 |
| must_change F0.5 | 0.769 | 0.484 | 0.149 | 0.143 | 0.149 |

- relevant_files unindexable, never in the index (not scored): `CLAUDE.md`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/debug.rs`, `crates/atlas-cli/src/commands/enrich.rs`, `crates/atlas-cli/src/commands/scip.rs`, `crates/atlas-mcp/src/server.rs`
- must_consider unindexable, never in the index (not scored): `CLAUDE.md`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 11 | 0 | 82 | 3/31 | 0.037 | 0.097 | 0.042 |
| whole pack | 34 | 0 | 453 | 11/31 | 0.024 | 0.355 | 0.03 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1660 | 11938 | 0.667 (4/6) | 1 (6/6) |
| 8000 | 1660 | 11938 | 0.667 (4/6) | 1 (6/6) |
| 16000 | 1660 | 11939 | 0.667 (4/6) | 1 (6/6) |

Blast radius: `impact` on `rust:tree:atlas-engine:report::FreshnessReport::Degraded#variant` (symbols in `crates/atlas-engine/src/report.rs` that `reference.patch` changes) against the 5 other must_change files. Precision is against must_change only, so it is low by design; recall is the score.

| impact view | recall | hits | files retrieved | precision |
|---|---|---|---|---|
| visible (21 items collapsed) | 1 | 5/5 | 28 | 0.179 |
| full | 1 | 5/5 | 34 | 0.147 |

## t03-nested-router-links: Planted bug: routes of Express routers nested across files are never composed or linked

Categories: planted-bug, dependency-trace.

Index: 491 files in tree.
Line gold: 3 old-side lines in 2 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0 | 0.333 | 0.312 | 0 | 0 | 0 | 0 |
| full | 1 | whole pack | 1 | 0.097 | 0.117 | 0 | 1 | 1 | 1 |
| agent | 2 | MUST READ | 0 | 0.417 | 0.365 | 0 | 0 | 0 | 0 |
| agent | 2 | whole pack | 0 | 0.16 | 0.179 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | MUST READ | 0 | 0.417 | 0.365 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | whole pack | 0 | 0.16 | 0.179 | 0 | 0 | 0 | 0 |
| first-line | 1 | MUST READ | 0 | 0.167 | 0.179 | 0 | 0 | 0 | 0 |
| first-line | 1 | whole pack | 0 | 0.04 | 0.048 | 0 | 0 | 0 | 0 |

### `full`

Query: the task prompt (1336 characters).
Context pack: 16511 bytes, ~2156 of 6000 tokens; items: must read 3, skim 26, patterns 5, reuse 2, impact 13, tests 2, uncertain 9, omitted 35.

Searches: the query, then `GET /api/orders/{id}`, `GET /{id}`, `fetch(`, `calls_endpoint`, `app.use('/health', healthRouter)`, `health.ts`, `Bug report`, `Code Atlas stopped linking`, `web client`, `Express routers nested`.
Similar seeds: `ts:tree:globex-api:src/routes/orders.routes::registerOrderRoutes#function` (members 7, shape-only 0), `rust:tree:acme-billing:acme_client::AcmeApiClient::get_order#method` (members 8, shape-only 0), `ts:tree:@acme/sdk:src/client::AcmeClient::getOrder#method` (members 10, shape-only 0), `rust:tree:atlas-detect:detectors/clients::fetch_request#function` (members 10, shape-only 0), `rust:tree:atlas-link:http::calls_endpoint#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 1 (1/1) | 0 (0/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.25 (1/4) | 0.75 (3/4) | 0.5 (2/4) | 0.5 (2/4) | 0.75 (3/4) |
| must_consider recall | 0 (0/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files precision | 0.333 | 0.097 | 0.035 | 0.074 | 0.037 |
| relevant_files F0.5 | 0.312 | 0.117 | 0.043 | 0.089 | 0.045 |
| must_change F0.5 | 0 | 0.04 | 0 | 0 | 0.015 |

- relevant_files not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 3 | 0 | 37 | 0/3 | 0 | 0 | 0 |
| whole pack | 60 | 2 | 378 | 0/3 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2156 | 16511 | 0 (0/1) | 1 (1/1) |
| 8000 | 2156 | 16511 | 0 (0/1) | 1 (1/1) |
| 16000 | 2156 | 16512 | 0 (0/1) | 1 (1/1) |

### `agent`, query 1 of 2

Query: `Express router mounts across files: nested mounts (router.use('/x', otherRouter)) not composed into endpoint paths`; sent by `2026-10-05-directed/t03-nested-router-links.atlas.r0`.
Context pack: 19554 bytes, ~3274 of 6000 tokens; items: must read 6, skim 22, patterns 5, reuse 0, impact 0, tests 8, uncertain 0, omitted 17.

Searches: the query, then `router mounts across files`, `nested mounts`, `endpoint paths`.
Similar seeds: `rust:tree:atlas-detect:detectors/nest::NestEndpointDetector::emit_endpoint#method` (members 10, shape-only 0), `rust:tree:atlas-detect:detectors/express::emit_cross_file_mount#function` (members 10, shape-only 0), `rust:tree:atlas-detect:detectors/axum::AxumEndpointDetector::record_nest#method` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.25 (1/4) | 0.5 (2/4) | 0.5 (2/4) | 0.25 (1/4) | 0.75 (3/4) |
| must_consider recall | 0.5 (1/2) | 1 (2/2) | 0.5 (1/2) | 0 (0/2) | 1 (2/2) |
| relevant_files precision | 0.333 | 0.154 | 0.143 | 0.143 | 0.115 |
| relevant_files F0.5 | 0.312 | 0.179 | 0.167 | 0.156 | 0.139 |
| must_change F0.5 | 0 | 0 | 0.088 | 0.172 | 0.048 |

- relevant_files not retrieved anywhere: `crates/atlas-link/src/http.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 6 | 0 | 189 | 0/3 | 0 | 0 | 0 |
| whole pack | 41 | 0 | 571 | 0/3 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3274 | 19554 | 0 (0/1) | 0 (0/1) |
| 8000 | 3274 | 19554 | 0 (0/1) | 0 (0/1) |
| 16000 | 3274 | 19555 | 0 (0/1) | 0 (0/1) |

### `agent`, query 2 of 2

Query: `Fix Express nested router mounts across files not composing endpoint paths (mounts edges, mount_prefix, router_relative) and linking`; sent by `2026-10-05-directed/t03-nested-router-links.atlas.r1`.
Context pack: 12825 bytes, ~2870 of 6000 tokens; items: must read 7, skim 24, patterns 0, reuse 0, impact 0, tests 0, uncertain 0, omitted 0.

Searches: the query, then `router mounts across files`, `composing endpoint paths`, `mounts edges`.
Similar seeds: `rust:tree:atlas-link:compose::express_chains#function` (members 10, shape-only 0), `rust:tree:atlas-detect:detectors/express::emit_cross_file_mount#function` (members 10, shape-only 0), `rust:tree:atlas-link:compose::compose#function` (members 10, shape-only 0), `rust:tree:atlas-link:compose::mount_chain#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.25 (1/4) | 0.25 (1/4) | 0.5 (2/4) | 0.75 (3/4) | 0.75 (3/4) |
| must_consider recall | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files precision | 0.5 | 0.167 | 0.333 | 0.2 | 0.136 |
| relevant_files F0.5 | 0.417 | 0.179 | 0.357 | 0.234 | 0.163 |
| must_change F0.5 | 0 | 0 | 0.2 | 0.082 | 0.056 |

- relevant_files not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 7 | 0 | 133 | 0/3 | 0 | 0 | 0 |
| whole pack | 31 | 0 | 344 | 0/3 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2870 | 12825 | 0 (0/1) | 0 (0/1) |
| 8000 | 2870 | 12825 | 0 (0/1) | 0 (0/1) |
| 16000 | 2870 | 12826 | 0 (0/1) | 0 (0/1) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `Bug report: Code Atlas stopped linking our web client to endpoints that live on Express routers nested more than one level deep across files`.
Context pack: 21716 bytes, ~3433 of 6000 tokens; items: must read 8, skim 23, patterns 5, reuse 1, impact 2, tests 8, uncertain 4, omitted 26.

Searches: the query, then `Bug report`, `Code Atlas stopped linking`, `web client`, `Express routers nested`.
Similar seeds: `rust:tree:atlas-detect:detectors/nest::NestEndpointDetector::emit_endpoint#method` (members 10, shape-only 0), `rust:tree:atlas-query:freshness::Freshness::from_report#method` (members 3, shape-only 0), `rust:tree:atlas-link:auth::is_test_code#function` (members 10, shape-only 0), `rust:tree:atlas-resolve/test/acme:::web#function` (members 10, shape-only 0), `rust:tree:atlas-link/test/base_evidence:::express#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.25 (1/4) | 0.25 (1/4) | 0.75 (3/4) | 0.5 (2/4) | 0.75 (3/4) |
| must_consider recall | 0 (0/2) | 0 (0/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files precision | 0.167 | 0.04 | 0.094 | 0.065 | 0.042 |
| relevant_files F0.5 | 0.179 | 0.048 | 0.114 | 0.078 | 0.051 |
| must_change F0.5 | 0 | 0 | 0.039 | 0.04 | 0.017 |

- relevant_files not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/tests/endpoints.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 200 | 0/3 | 0 | 0 | 0 |
| whole pack | 51 | 0 | 832 | 0/3 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3433 | 21716 | 0 (0/1) | 0 (0/1) |
| 8000 | 3433 | 21716 | 0 (0/1) | 0 (0/1) |
| 16000 | 3433 | 21717 | 0 (0/1) | 0 (0/1) |

## t04-detector-import-gates: Refactor: one shared place for detector import checks

Categories: refactor.

Index: 491 files in tree.
Line gold: 66 old-side lines in 6 base files of `reference.patch`, 0 of them in files outside the index.
- added by the patch, no base lines (not scored): `crates/atlas-detect/src/imports.rs`

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.8 | 0.636 | 0.66 | 0.004 | 0.8 | 0.8 | 0.8 |
| full | 1 | whole pack | 0.8 | 0.389 | 0.432 | 0.024 | 0.8 | 0.8 | 0.8 |
| agent | 2 | MUST READ | 0.4 | 0.583 | 0.547 | 0.179 | 0.4 | 0.4 | 0.4 |
| agent | 2 | whole pack | 0.4 | 0.272 | 0.299 | 0.03 | 0.4 | 0.4 | 0.4 |
| agent-seeded | 2 | MUST READ | 0.4 | 0.583 | 0.547 | 0.179 | 0.4 | 0.4 | 0.4 |
| agent-seeded | 2 | whole pack | 0.4 | 0.272 | 0.299 | 0.03 | 0.4 | 0.4 | 0.4 |
| first-line | 1 | MUST READ | 0.4 | 0.667 | 0.476 | 0.108 | 0.4 | 0.4 | 0.4 |
| first-line | 1 | whole pack | 0.4 | 0.25 | 0.274 | 0.042 | 0.4 | 0.4 | 0.4 |

### `full`

Query: the task prompt (1034 characters).
Context pack: 32622 bytes, ~5106 of 6000 tokens; items: must read 22, skim 18, patterns 3, reuse 0, impact 6, tests 8, uncertain 2, omitted 38.

Searches: the query, then `atlas_core::ImportFact`, `import`, `require()`, `use`, `crates/atlas-detect/src/detectors/`, `ImportFact`, `extern crate`, `Tech debt`, `several framework detectors decide`, `path-qualified references`, `logic exists`.
Similar seeds: `rust:tree:atlas-query:queries/context::route_mentions#function` (members 10, shape-only 0), `rust:tree:atlas-resolve:output::Emitter::ts_import#method` (members 10, shape-only 0), `ts:tree:acme-api:src/auth/require-permission.decorator::RequirePermission#function` (members 8, shape-only 0), `rust:tree:atlas-resolve:surface::FileSurface::uses_in#method` (members 1, shape-only 1), `rust:tree:atlas-detect:registry::DetectorRegistry::run_all#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.8 (4/5) | 0.8 (4/5) | 1 (5/5) | 0.6 (3/5) | 1 (5/5) |
| relevant_files recall | 0.778 (7/9) | 0.778 (7/9) | 1 (9/9) | 0.333 (3/9) | 1 (9/9) |
| must_consider recall | 0.667 (2/3) | 0.667 (2/3) | 1 (3/3) | 0 (0/3) | 1 (3/3) |
| relevant_files precision | 0.636 | 0.389 | 0.184 | 0.111 | 0.132 |
| relevant_files F0.5 | 0.66 | 0.432 | 0.22 | 0.128 | 0.16 |
| must_change F0.5 | 0.408 | 0.26 | 0.124 | 0.133 | 0.09 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 22 | 0 | 338 | 1/66 | 0.003 | 0.015 | 0.004 |
| whole pack | 59 | 1 | 913 | 18/66 | 0.02 | 0.273 | 0.024 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3977 | 27271 | 0.8 (4/5) | 0.8 (4/5) |
| 8000 | 5106 | 32622 | 0.8 (4/5) | 0.8 (4/5) |
| 16000 | 5106 | 32623 | 0.8 (4/5) | 0.8 (4/5) |

### `agent`, query 1 of 2

Query: `Extract shared import-checking abstraction in atlas-detect detectors replacing ImportFact matching`; sent by `2026-10-05-directed/t04-detector-import-gates.atlas.r0`.
Context pack: 16098 bytes, ~2575 of 6000 tokens; items: must read 15, skim 19, patterns 4, reuse 0, impact 6, tests 8, uncertain 0, omitted 0.

Searches: the query, then `Extract shared import-checking abstraction`, `detectors replacing ImportFact matching`.
Similar seeds: `rust:tree:atlas-mcp:server::check_repo#function` (members 10, shape-only 0), `rust:tree:atlas-store/test/bench:common::replace#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.4 (2/5) | 0.4 (2/5) | 0.4 (2/5) | 0 (0/5) | 0.4 (2/5) |
| relevant_files recall | 0.444 (4/9) | 0.556 (5/9) | 0.222 (2/9) | 0 (0/9) | 0.556 (5/9) |
| must_consider recall | 0.333 (1/3) | 0.667 (2/3) | 0 (0/3) | 0 (0/3) | 0.667 (2/3) |
| relevant_files precision | 0.5 | 0.294 | 0.125 | 0 | 0.147 |
| relevant_files F0.5 | 0.488 | 0.325 | 0.137 | 0 | 0.172 |
| must_change F0.5 | 0.27 | 0.137 | 0.145 | 0 | 0.071 |

- must_change not retrieved anywhere: `crates/atlas-detect/src/detectors/axum.rs`, `crates/atlas-detect/src/detectors/express.rs`, `crates/atlas-detect/src/view.rs`
- relevant_files not retrieved anywhere: `crates/atlas-detect/src/detectors/auth.rs`, `crates/atlas-detect/src/detectors/axum.rs`, `crates/atlas-detect/src/detectors/express.rs`, `crates/atlas-detect/src/view.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/src/detectors/auth.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 15 | 0 | 106 | 19/66 | 0.179 | 0.288 | 0.194 |
| whole pack | 52 | 1 | 859 | 19/66 | 0.022 | 0.288 | 0.027 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2575 | 16098 | 0.4 (2/5) | 0.4 (2/5) |
| 8000 | 2575 | 16098 | 0.4 (2/5) | 0.4 (2/5) |
| 16000 | 2575 | 16099 | 0.4 (2/5) | 0.4 (2/5) |

### `agent`, query 2 of 2

Query: `Extract shared import-detection abstraction in atlas-detect replacing ImportFact matching in detectors`; sent by `2026-10-05-directed/t04-detector-import-gates.atlas.r1`.
Context pack: 17521 bytes, ~2905 of 6000 tokens; items: must read 15, skim 20, patterns 0, reuse 0, impact 6, tests 8, uncertain 5, omitted 2.

Searches: the query, then `Extract shared import-detection abstraction`, `atlas-detect replacing ImportFact matching`.
Similar seeds: `rust:tree:atlas-detect/test/auth:common::run#function` (members 8, shape-only 0), `rust:tree:atlas-detect/test/clients:::fixture_detection#function` (members 10, shape-only 0), `rust:tree:atlas-store/test/bench:common::replace#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.4 (2/5) | 0.4 (2/5) | 0.4 (2/5) | 0 (0/5) | 0.4 (2/5) |
| relevant_files recall | 0.444 (4/9) | 0.444 (4/9) | 0.222 (2/9) | 0 (0/9) | 0.444 (4/9) |
| must_consider recall | 0.333 (1/3) | 0.333 (1/3) | 0 (0/3) | 0 (0/3) | 0.333 (1/3) |
| relevant_files precision | 0.667 | 0.25 | 0.118 | 0 | 0.111 |
| relevant_files F0.5 | 0.606 | 0.274 | 0.13 | 0 | 0.131 |
| must_change F0.5 | 0.345 | 0.145 | 0.137 | 0 | 0.067 |

- must_change not retrieved anywhere: `crates/atlas-detect/src/detectors/axum.rs`, `crates/atlas-detect/src/detectors/express.rs`, `crates/atlas-detect/src/view.rs`
- relevant_files not retrieved anywhere: `crates/atlas-detect/src/detectors/auth.rs`, `crates/atlas-detect/src/detectors/axum.rs`, `crates/atlas-detect/src/detectors/express.rs`, `crates/atlas-detect/src/detectors/mod.rs`, `crates/atlas-detect/src/view.rs`
- must_consider not retrieved anywhere: `crates/atlas-detect/src/detectors/auth.rs`, `crates/atlas-detect/src/detectors/mod.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 15 | 0 | 129 | 19/66 | 0.147 | 0.288 | 0.163 |
| whole pack | 54 | 1 | 706 | 19/66 | 0.027 | 0.288 | 0.033 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2905 | 17521 | 0.4 (2/5) | 0.4 (2/5) |
| 8000 | 2905 | 17521 | 0.4 (2/5) | 0.4 (2/5) |
| 16000 | 2905 | 17522 | 0.4 (2/5) | 0.4 (2/5) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `Tech debt in atlas-detect: several framework detectors decide whether they apply to a file by pattern-matching 'atlas_core::ImportFact' variants by hand (TS 'import' vs 'require()' specifiers, Rust 'use' roots, plus a fallback on path-qualified references), and the same logic exists a second time for detectors that already work on a parsed file`.
Context pack: 24705 bytes, ~3983 of 6000 tokens; items: must read 15, skim 21, patterns 5, reuse 0, impact 0, tests 8, uncertain 2, omitted 21.

Searches: the query, then `atlas_core::ImportFact`, `import`, `require()`, `use`, `Tech debt`, `several framework detectors decide`, `path-qualified references`, `logic exists`.
Similar seeds: `rust:tree:atlas-detect:detectors/jobs::JobDetector::rust_jobs#method` (members 5, shape-only 0), `rust:tree:atlas-resolve:output::Emitter::ts_import#method` (members 10, shape-only 0), `ts:tree:acme-api:src/auth/require-permission.decorator::RequirePermission#function` (members 8, shape-only 0), `rust:tree:atlas-resolve:surface::FileSurface::uses_in#method` (members 1, shape-only 1), `rust:tree:atlas-lang-ts:testing::framework_of_import#function` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.4 (2/5) | 0.4 (2/5) | 1 (5/5) | 0.4 (2/5) | 1 (5/5) |
| relevant_files recall | 0.222 (2/9) | 0.444 (4/9) | 0.778 (7/9) | 0.222 (2/9) | 1 (9/9) |
| must_consider recall | 0 (0/3) | 0.333 (1/3) | 0.667 (2/3) | 0 (0/3) | 1 (3/3) |
| relevant_files precision | 0.667 | 0.25 | 0.163 | 0.095 | 0.145 |
| relevant_files F0.5 | 0.476 | 0.274 | 0.193 | 0.108 | 0.175 |
| must_change F0.5 | 0.588 | 0.145 | 0.141 | 0.112 | 0.099 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 15 | 0 | 227 | 21/66 | 0.093 | 0.318 | 0.108 |
| whole pack | 51 | 0 | 637 | 22/66 | 0.035 | 0.333 | 0.042 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3983 | 24705 | 0.4 (2/5) | 0.4 (2/5) |
| 8000 | 3983 | 24705 | 0.4 (2/5) | 0.4 (2/5) |
| 16000 | 3983 | 24706 | 0.4 (2/5) | 0.4 (2/5) |

## t05-mcp-dependency-tools: Expose the dependents/dependencies traversals as MCP tools

Categories: architecture-addition, test-discovery.

Index: 491 files in tree.
Line gold: 8 old-side lines in 3 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.5 | 0.8 | 0.741 | 0 | 0.5 | 0.5 | 0.5 |
| full | 1 | whole pack | 0.5 | 0.286 | 0.317 | 0 | 0.5 | 0.5 | 0.5 |
| agent | 2 | MUST READ | 0.5 | 1 | 0.789 | 0 | 0.5 | 0.5 | 0.5 |
| agent | 2 | whole pack | 1 | 0.462 | 0.508 | 0 | 1 | 1 | 1 |
| agent-seeded | 2 | MUST READ | 0.5 | 1 | 0.789 | 0 | 0.5 | 0.5 | 0.5 |
| agent-seeded | 2 | whole pack | 1 | 0.462 | 0.508 | 0 | 1 | 1 | 1 |
| first-line | 1 | MUST READ | 0.5 | 0.25 | 0.217 | 0 | 0.5 | 0.5 | 0.5 |
| first-line | 1 | whole pack | 0.5 | 0.111 | 0.116 | 0 | 0.5 | 0.5 | 0.5 |

### `full`

Query: the task prompt (1044 characters).
Context pack: 20566 bytes, ~3019 of 6000 tokens; items: must read 13, skim 22, patterns 0, reuse 5, impact 0, tests 8, uncertain 1, omitted 5.

Searches: the query, then `code-atlas query dependents <target>`, `code-atlas query dependencies <target>`, `atlas_callers`, `atlas_callees`, `atlas_dependents`, `atlas_dependencies`, `target`, `depth`, `Agents connected through`, `MCP server`, `follow imports`, `type usage`.
Similar seeds: `rust:tree:atlas-mcp:server::AtlasServer::callees#method` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependencies#method` (members 8, shape-only 0), `rust:tree:atlas-mcp:server::AtlasServer::callers#method` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::callees#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) |
| relevant_files recall | 0.571 (4/7) | 0.571 (4/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.5 (2/4) | 0.75 (3/4) | 0.5 (2/4) | 1 (4/4) |
| relevant_files precision | 0.8 | 0.286 | 0.096 | 0.571 | 0.097 |
| relevant_files F0.5 | 0.741 | 0.317 | 0.116 | 0.571 | 0.118 |
| must_change F0.5 | 0.227 | 0.086 | 0.024 | 0.167 | 0.02 |

- must_change not retrieved anywhere: `crates/atlas-mcp/tests/protocol.rs`
- relevant_files not retrieved anywhere: `crates/atlas-mcp/tests/protocol.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 13 | 0 | 125 | 0/8 | 0 | 0 | 0 |
| whole pack | 49 | 0 | 597 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3019 | 20566 | 0.5 (1/2) | 0.5 (1/2) |
| 8000 | 3019 | 20566 | 0.5 (1/2) | 0.5 (1/2) |
| 16000 | 3019 | 20567 | 0.5 (1/2) | 0.5 (1/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | yes | yes | no |
| `QueryEngine::dependencies` | no | yes | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

### `agent`, query 1 of 2

Query: `Add atlas_dependents and atlas_dependencies MCP tools mirroring atlas_callers/atlas_callees and CLI query dependents/dependencies`; sent by `2026-10-05-directed/t05-mcp-dependency-tools.atlas.r0`.
Context pack: 13469 bytes, ~2011 of 6000 tokens; items: must read 8, skim 10, patterns 0, reuse 6, impact 0, tests 1, uncertain 3, omitted 21.

Searches: the query, then `MCP tools mirroring atlas`, `CLI query dependents`.
Similar seeds: `rust:tree:atlas-query:queries/traverse::TraversalKind::as_str#method` (members 10, shape-only 2), `rust:tree:atlas-mcp/test/protocol:common::Harness::call#method` (members 7, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| relevant_files recall | 0.429 (3/7) | 0.857 (6/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.75 (3/4) | 0.75 (3/4) | 0.5 (2/4) | 0.75 (3/4) |
| relevant_files precision | 1 | 0.462 | 0.417 | 0.2 | 0.207 |
| relevant_files F0.5 | 0.789 | 0.508 | 0.455 | 0.23 | 0.244 |
| must_change F0.5 | 0.357 | 0.185 | 0.2 | 0.061 | 0.085 |

- relevant_files not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`
- must_consider not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 96 | 0/8 | 0 | 0 | 0 |
| whole pack | 28 | 0 | 343 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2011 | 13469 | 0.5 (1/2) | 1 (2/2) |
| 8000 | 2011 | 13469 | 0.5 (1/2) | 1 (2/2) |
| 16000 | 2011 | 13470 | 0.5 (1/2) | 1 (2/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | no | yes | no |
| `QueryEngine::dependencies` | no | no | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

### `agent`, query 2 of 2

Query: `Add MCP tools atlas_dependents and atlas_dependencies mirroring atlas_callers/atlas_callees and CLI query dependents/dependencies`; sent by `2026-10-05-directed/t05-mcp-dependency-tools.atlas.r1`.
Context pack: 13469 bytes, ~2011 of 6000 tokens; items: must read 8, skim 10, patterns 0, reuse 6, impact 0, tests 1, uncertain 3, omitted 21.

Searches: the query, then `MCP tools atlas`, `dependencies mirroring atlas`, `CLI query dependents`.
Similar seeds: `rust:tree:atlas-query:queries/traverse::TraversalKind::as_str#method` (members 10, shape-only 2), `rust:tree:atlas-mcp/test/protocol:common::Harness::call#method` (members 7, shape-only 0), `rust:tree:atlas-resolve:manifest::dependencies#function` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| relevant_files recall | 0.429 (3/7) | 0.857 (6/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.75 (3/4) | 0.75 (3/4) | 0.5 (2/4) | 0.75 (3/4) |
| relevant_files precision | 1 | 0.462 | 0.312 | 0.154 | 0.158 |
| relevant_files F0.5 | 0.789 | 0.508 | 0.352 | 0.18 | 0.189 |
| must_change F0.5 | 0.357 | 0.185 | 0.152 | 0.047 | 0.065 |

- relevant_files not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`
- must_consider not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 96 | 0/8 | 0 | 0 | 0 |
| whole pack | 28 | 0 | 343 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2011 | 13469 | 0.5 (1/2) | 1 (2/2) |
| 8000 | 2011 | 13469 | 0.5 (1/2) | 1 (2/2) |
| 16000 | 2011 | 13470 | 0.5 (1/2) | 1 (2/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | no | yes | no |
| `QueryEngine::dependencies` | no | yes | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

### `agent-seeded`, query 1 of 2

Query: `Add atlas_dependents and atlas_dependencies MCP tools mirroring atlas_callers/atlas_callees and CLI query dependents/dependencies`; seeds `atlas_callers`, `atlas_callees`; sent by `2026-10-05-directed/t05-mcp-dependency-tools.atlas.r0`.
Context pack: 13661 bytes, ~2011 of 6000 tokens; items: must read 8, skim 10, patterns 0, reuse 6, impact 0, tests 1, uncertain 3, omitted 23.

Searches: the query, then `MCP tools mirroring atlas`, `CLI query dependents`.
Similar seeds: `rust:tree:atlas-query:queries/traverse::TraversalKind::as_str#method` (members 10, shape-only 2), `rust:tree:atlas-mcp/test/protocol:common::Harness::call#method` (members 7, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| relevant_files recall | 0.429 (3/7) | 0.857 (6/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.75 (3/4) | 0.75 (3/4) | 0.5 (2/4) | 0.75 (3/4) |
| relevant_files precision | 1 | 0.462 | 0.417 | 0.2 | 0.207 |
| relevant_files F0.5 | 0.789 | 0.508 | 0.455 | 0.23 | 0.244 |
| must_change F0.5 | 0.357 | 0.185 | 0.2 | 0.061 | 0.085 |

- relevant_files not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`
- must_consider not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 96 | 0/8 | 0 | 0 | 0 |
| whole pack | 28 | 0 | 343 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2011 | 13661 | 0.5 (1/2) | 1 (2/2) |
| 8000 | 2011 | 13661 | 0.5 (1/2) | 1 (2/2) |
| 16000 | 2011 | 13662 | 0.5 (1/2) | 1 (2/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | no | yes | no |
| `QueryEngine::dependencies` | no | no | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

### `agent-seeded`, query 2 of 2

Query: `Add MCP tools atlas_dependents and atlas_dependencies mirroring atlas_callers/atlas_callees and CLI query dependents/dependencies`; seeds `atlas_callers`, `crates/atlas-mcp`; sent by `2026-10-05-directed/t05-mcp-dependency-tools.atlas.r1`.
Context pack: 13646 bytes, ~2011 of 6000 tokens; items: must read 8, skim 10, patterns 0, reuse 6, impact 0, tests 1, uncertain 3, omitted 23.

Searches: the query, then `MCP tools atlas`, `dependencies mirroring atlas`, `CLI query dependents`.
Similar seeds: `rust:tree:atlas-query:queries/traverse::TraversalKind::as_str#method` (members 10, shape-only 2), `rust:tree:atlas-mcp/test/protocol:common::Harness::call#method` (members 7, shape-only 0), `rust:tree:atlas-resolve:manifest::dependencies#function` (members 10, shape-only 0), `rust:tree:atlas-query:queries/traverse::QueryEngine::dependents#method` (members 8, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 1 (2/2) | 1 (2/2) | 0.5 (1/2) | 1 (2/2) |
| relevant_files recall | 0.429 (3/7) | 0.857 (6/7) | 0.714 (5/7) | 0.571 (4/7) | 0.857 (6/7) |
| must_consider recall | 0.5 (2/4) | 0.75 (3/4) | 0.75 (3/4) | 0.5 (2/4) | 0.75 (3/4) |
| relevant_files precision | 1 | 0.462 | 0.312 | 0.154 | 0.158 |
| relevant_files F0.5 | 0.789 | 0.508 | 0.352 | 0.18 | 0.189 |
| must_change F0.5 | 0.357 | 0.185 | 0.152 | 0.047 | 0.065 |

- relevant_files not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`
- must_consider not retrieved anywhere: `crates/atlas-mcp/src/descriptions.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 96 | 0/8 | 0 | 0 | 0 |
| whole pack | 28 | 0 | 343 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2011 | 13646 | 0.5 (1/2) | 1 (2/2) |
| 8000 | 2011 | 13646 | 0.5 (1/2) | 1 (2/2) |
| 16000 | 2011 | 13647 | 0.5 (1/2) | 1 (2/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | no | yes | no |
| `QueryEngine::dependencies` | no | yes | no |
| `render::targeted` | yes | no | yes |
| `arguments::depth` | yes | no | no |
| `arguments::TraverseArgs` | no | no | no |

### `first-line`

Query: `Agents connected through the MCP server cannot ask "what uses this?" or "what does this use?"`.
Context pack: 8660 bytes, ~1450 of 6000 tokens; items: must read 7, skim 12, patterns 0, reuse 0, impact 0, tests 3, uncertain 0, omitted 0.

Searches: the query, then `Agents connected through`, `MCP server`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/connect::plan_mcp_config#function` (members 7, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/connect::run#function` (members 10, shape-only 0), `rust:tree:atlas-mcp:runtime::serve#function` (members 3, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (1/2) | 0.5 (1/2) | 0.5 (1/2) | 0 (0/2) | 0.5 (1/2) |
| relevant_files recall | 0.143 (1/7) | 0.143 (1/7) | 0.143 (1/7) | 0 (0/7) | 0.143 (1/7) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 0 (0/4) | 0 (0/4) | 0 (0/4) |
| relevant_files precision | 0.25 | 0.111 | 0.083 | 0 | 0.04 |
| relevant_files F0.5 | 0.217 | 0.116 | 0.091 | 0 | 0.047 |
| must_change F0.5 | 0.278 | 0.132 | 0.1 | 0 | 0.049 |

- must_change not retrieved anywhere: `crates/atlas-mcp/tests/protocol.rs`
- relevant_files not retrieved anywhere: `crates/atlas-cli/src/commands/query.rs`, `crates/atlas-mcp/src/arguments.rs`, `crates/atlas-mcp/src/descriptions.rs`, `crates/atlas-mcp/src/render.rs`, `crates/atlas-mcp/tests/protocol.rs`, `crates/atlas-query/src/queries/traverse.rs`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/query.rs`, `crates/atlas-mcp/src/arguments.rs`, `crates/atlas-mcp/src/descriptions.rs`, `crates/atlas-query/src/queries/traverse.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 7 | 0 | 56 | 0/8 | 0 | 0 | 0 |
| whole pack | 22 | 0 | 235 | 0/8 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1450 | 8660 | 0.5 (1/2) | 0.5 (1/2) |
| 8000 | 1450 | 8660 | 0.5 (1/2) | 0.5 (1/2) |
| 16000 | 1450 | 8661 | 0.5 (1/2) | 0.5 (1/2) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `QueryEngine::dependents` | no | no | no |
| `QueryEngine::dependencies` | no | no | no |
| `render::targeted` | no | no | no |
| `arguments::depth` | no | no | no |
| `arguments::TraverseArgs` | no | no | no |

## t06-repo-file-size-limit: Per-repository max_file_bytes override

Categories: config-impact, blast-radius.

Index: 491 files in tree.
Line gold: 31 old-side lines in 11 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.182 | 0.333 | 0.349 | 0 | 0.182 | 0.182 | 0.182 |
| full | 1 | whole pack | 0.364 | 0.211 | 0.241 | 0.006 | 0.364 | 0.364 | 0.364 |
| agent | 2 | MUST READ | 0.136 | 0.6 | 0.556 | 0.012 | 0.136 | 0.136 | 0.136 |
| agent | 2 | whole pack | 0.227 | 0.304 | 0.329 | 0.01 | 0.227 | 0.227 | 0.227 |
| agent-seeded | 2 | MUST READ | 0.136 | 0.6 | 0.556 | 0.012 | 0.136 | 0.136 | 0.136 |
| agent-seeded | 2 | whole pack | 0.227 | 0.304 | 0.329 | 0.01 | 0.227 | 0.227 | 0.227 |
| first-line | 1 | MUST READ | 0.091 | 0.25 | 0.217 | 0 | 0.091 | 0.091 | 0.091 |
| first-line | 1 | whole pack | 0.091 | 0.167 | 0.161 | 0 | 0.091 | 0.091 | 0.091 |

### `full`

Query: the task prompt (1081 characters).
Context pack: 18943 bytes, ~2737 of 6000 tokens; items: must read 22, skim 16, patterns 0, reuse 1, impact 0, tests 8, uncertain 1, omitted 10.

Searches: the query, then `max_file_bytes`, `[project.settings]`, `projects.toml`, `[[project.repo]]`, `Per-repository file size limit`, `mixed projects`, `large hand-maintained TypeScript files`, `want indexed`.
Similar seeds: `rust:tree:atlas-cli/bin/atlas-cli:commands/project::apply_settings#function` (members 8, shape-only 0), `rust:tree:atlas-lang-rust:analyzer::RustAnalyzer::with_max_source_bytes#method` (members 1, shape-only 0), `rust:tree:atlas-project:registry::Registry::set_settings#method` (members 10, shape-only 0), `rust:tree:atlas-scip/test/rust_analyzer:::expected#function` (members 6, shape-only 0), `rust:tree:atlas-link:facts::Facts::in_project#method` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.182 (2/11) | 0.364 (4/11) | 0.545 (6/11) | 0.273 (3/11) | 0.818 (9/11) |
| relevant_files recall | 0.429 (3/7) | 0.571 (4/7) | 0.714 (5/7) | 0.286 (2/7) | 0.857 (6/7) |
| must_consider recall | 0.25 (1/4) | 0.25 (1/4) | 0.75 (3/4) | 0.25 (1/4) | 0.75 (3/4) |
| relevant_files precision | 0.333 | 0.211 | 0.122 | 0.118 | 0.098 |
| relevant_files F0.5 | 0.349 | 0.241 | 0.146 | 0.133 | 0.12 |
| must_change F0.5 | 0.213 | 0.23 | 0.171 | 0.19 | 0.176 |

- must_change not retrieved anywhere: `crates/atlas-query/tests/perf.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`
- relevant_files not retrieved anywhere: `crates/atlas-engine/src/engine.rs`
- must_consider not retrieved anywhere: `crates/atlas-engine/src/engine.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 22 | 0 | 146 | 0/31 | 0 | 0 | 0 |
| whole pack | 48 | 0 | 421 | 2/31 | 0.005 | 0.065 | 0.006 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2737 | 18943 | 0.182 (2/11) | 0.364 (4/11) |
| 8000 | 2737 | 18943 | 0.182 (2/11) | 0.364 (4/11) |
| 16000 | 2737 | 18944 | 0.182 (2/11) | 0.364 (4/11) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `discovery::discover` | no | no | no |

### `agent`, query 1 of 2

Query: `Add per-repo optional max_file_bytes override in projects.toml, honoured in discovery, full index, freshness scans, incremental updates`; sent by `2026-10-05-directed/t06-repo-file-size-limit.atlas.r0`.
Context pack: 12754 bytes, ~2254 of 6000 tokens; items: must read 11, skim 13, patterns 0, reuse 0, impact 0, tests 8, uncertain 2, omitted 0.

Searches: the query, then `per-repo optional max`, `bytes override`, `full index`, `freshness scans`.
Similar seeds: `rust:tree:atlas-engine:scan::scan_repo#function` (members 9, shape-only 0), `rust:tree:atlas-semantic:budget::BudgetState::try_reserve#method` (members 6, shape-only 0), `rust:tree:atlas-resolve:ts::Resolver::ts_override_candidates#method` (members 10, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/index::run#function` (members 10, shape-only 0), `rust:tree:atlas-engine:engine::Engine::header#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.182 (2/11) | 0.273 (3/11) | 0.273 (3/11) | 0.091 (1/11) | 0.455 (5/11) |
| relevant_files recall | 0.429 (3/7) | 0.429 (3/7) | 0.714 (5/7) | 0.571 (4/7) | 1 (7/7) |
| must_consider recall | 0.25 (1/4) | 0.25 (1/4) | 0.75 (3/4) | 0.75 (3/4) | 1 (4/4) |
| relevant_files precision | 0.6 | 0.3 | 0.156 | 0.16 | 0.125 |
| relevant_files F0.5 | 0.556 | 0.319 | 0.185 | 0.187 | 0.152 |
| must_change F0.5 | 0.323 | 0.294 | 0.108 | 0.045 | 0.106 |

- must_change not retrieved anywhere: `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 11 | 0 | 145 | 3/31 | 0.021 | 0.097 | 0.025 |
| whole pack | 34 | 0 | 506 | 8/31 | 0.016 | 0.258 | 0.019 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2254 | 12754 | 0.182 (2/11) | 0.273 (3/11) |
| 8000 | 2254 | 12754 | 0.182 (2/11) | 0.273 (3/11) |
| 16000 | 2254 | 12755 | 0.182 (2/11) | 0.273 (3/11) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `discovery::discover` | yes | yes | no |

### `agent`, query 2 of 2

Query: `Add optional per-repo max_file_bytes override in projects.toml, honoured in full index, freshness scans, incremental updates`; sent by `2026-10-05-directed/t06-repo-file-size-limit.atlas.r1`.
Context pack: 13035 bytes, ~2183 of 6000 tokens; items: must read 7, skim 11, patterns 0, reuse 0, impact 0, tests 6, uncertain 3, omitted 17.

Searches: the query, then `optional per-repo max`, `bytes override`, `full index`, `freshness scans`.
Similar seeds: `rust:tree:atlas-engine:scan::scan_repo#function` (members 9, shape-only 0), `rust:tree:atlas-semantic:budget::BudgetState::try_reserve#method` (members 6, shape-only 0), `rust:tree:atlas-resolve:ts::Resolver::ts_override_candidates#method` (members 10, shape-only 0), `rust:tree:atlas-cli/bin/atlas-cli:commands/index::run#function` (members 10, shape-only 0), `rust:tree:atlas-engine:engine::Engine::header#method` (members 9, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.091 (1/11) | 0.182 (2/11) | 0.182 (2/11) | 0.091 (1/11) | 0.364 (4/11) |
| relevant_files recall | 0.429 (3/7) | 0.571 (4/7) | 0.714 (5/7) | 0.571 (4/7) | 1 (7/7) |
| must_consider recall | 0.5 (2/4) | 0.5 (2/4) | 1 (4/4) | 0.75 (3/4) | 1 (4/4) |
| relevant_files precision | 0.6 | 0.308 | 0.156 | 0.16 | 0.125 |
| relevant_files F0.5 | 0.556 | 0.339 | 0.185 | 0.187 | 0.152 |
| must_change F0.5 | 0.161 | 0.159 | 0.072 | 0.045 | 0.085 |

- must_change not retrieved anywhere: `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-resolve/tests/common/mod.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 7 | 0 | 124 | 0/31 | 0 | 0 | 0 |
| whole pack | 27 | 0 | 885 | 1/31 | 0.001 | 0.032 | 0.001 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 2183 | 13035 | 0.091 (1/11) | 0.182 (2/11) |
| 8000 | 2183 | 13035 | 0.091 (1/11) | 0.182 (2/11) |
| 16000 | 2183 | 13036 | 0.091 (1/11) | 0.182 (2/11) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `discovery::discover` | yes | no | no |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `Per-repository file size limit`.
Context pack: 5764 bytes, ~781 of 6000 tokens; items: must read 9, skim 0, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 0.

Searches: the query, then `Per-repository file size limit`.
Similar seeds: `rust:tree:atlas-project:changes::FileStamp::matches_stat#method` (members 8, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.091 (1/11) | 0.091 (1/11) | 0.091 (1/11) | 0.182 (2/11) | 0.182 (2/11) |
| relevant_files recall | 0.143 (1/7) | 0.143 (1/7) | 0.143 (1/7) | 0.286 (2/7) | 0.286 (2/7) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 0 (0/4) | 0.25 (1/4) | 0.25 (1/4) |
| relevant_files precision | 0.25 | 0.167 | 0.167 | 0.333 | 0.143 |
| relevant_files F0.5 | 0.217 | 0.161 | 0.161 | 0.323 | 0.159 |
| must_change F0.5 | 0.185 | 0.143 | 0.143 | 0.286 | 0.149 |

- must_change not retrieved anywhere: `crates/atlas-engine/src/watch.rs`, `crates/atlas-engine/tests/common/mod.rs`, `crates/atlas-link/tests/common/mod.rs`, `crates/atlas-project/src/manifest/tests.rs`, `crates/atlas-project/src/registry.rs`, `crates/atlas-query/tests/common/mod.rs`, `crates/atlas-query/tests/perf.rs`, `crates/atlas-resolve/tests/incremental.rs`, `crates/atlas-resolve/tests/workspace_quality.rs`
- relevant_files not retrieved anywhere: `crates/atlas-cli/src/commands/project.rs`, `crates/atlas-engine/src/engine.rs`, `crates/atlas-engine/src/scan.rs`, `crates/atlas-engine/src/watch.rs`, `crates/atlas-project/src/registry.rs`
- must_consider not retrieved anywhere: `crates/atlas-cli/src/commands/project.rs`, `crates/atlas-engine/src/engine.rs`, `crates/atlas-engine/src/scan.rs`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 9 | 0 | 56 | 0/31 | 0 | 0 | 0 |
| whole pack | 17 | 0 | 357 | 0/31 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 781 | 5764 | 0.091 (1/11) | 0.091 (1/11) |
| 8000 | 781 | 5764 | 0.091 (1/11) | 0.091 (1/11) |
| 16000 | 781 | 5765 | 0.091 (1/11) | 0.091 (1/11) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `discovery::discover` | no | no | yes |

Blast radius: `impact` on `rust:tree:atlas-project:registry::RepoConfig#struct`, `rust:tree:atlas-project:registry::build_repo#function` (symbols in `crates/atlas-project/src/registry.rs` that `reference.patch` changes) against the 10 other must_change files. Precision is against must_change only, so it is low by design; recall is the score.

| impact view | recall | hits | files retrieved | precision |
|---|---|---|---|---|
| visible (135 items collapsed) | 1 | 10/10 | 31 | 0.323 |
| full | 1 | 10/10 | 47 | 0.213 |

## t07-static-precompressed-quality: serveStatic precompressed variants ignore Accept-Encoding quality values

Categories: reuse-trap.

Index: 486 files in tree.
Line gold: 38 old-side lines in 3 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 1 | 0.182 | 0.204 | 0 | 1 | 1 | 1 |
| full | 1 | whole pack | 1 | 0.167 | 0.189 | 0 | 1 | 1 | 1 |
| agent | 2 | MUST READ | 0 | 1 | 0.662 | 0 | 0 | 0 | 0 |
| agent | 2 | whole pack | 0 | 0.5 | 0.476 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | MUST READ | 0 | 1 | 0.662 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | whole pack | 0 | 0.5 | 0.476 | 0 | 0 | 0 | 0 |
| first-line | 1 | MUST READ | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| first-line | 1 | whole pack | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### `full`

Query: the task prompt (1477 characters).
Context pack: 30673 bytes, ~5014 of 6000 tokens; items: must read 31, skim 6, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 37.

Searches: the query, then `serveStatic({ precompressed: true })`, `Accept-Encoding: gzip;q=1.0, br;q=0.5`, `app.js.gz`, `app.js.br`, `app.js`, `Accept-Encoding: br;q=0.2, gzip;q=0.8`, `Accept-*`, `q=0`, `precompressed variant`, `client lists`, `bare token`, `Real browsers`.
Similar seeds: `ts:tree:hono:src/middleware/serve-static/index::serveStatic#function` (members 9, shape-only 0), `ts:tree:@hono/cloudflare-workers:src/serve-static::serveStatic#function` (members 10, shape-only 0), `ts:tree:hono:benchmarks/http-server/benchmark::getAppTemplate#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/encode.test::str2UInt8Array#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/accept::parseAccept#function` (members 7, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0.4 (2/5) | 0.4 (2/5) | 0.6 (3/5) | 0.4 (2/5) | 0.6 (3/5) |
| must_consider recall | 0.167 (1/6) | 0.167 (1/6) | 0.833 (5/6) | 0.667 (4/6) | 0.833 (5/6) |
| relevant_files precision | 0.182 | 0.167 | 0.058 | 0.1 | 0.045 |
| relevant_files F0.5 | 0.204 | 0.189 | 0.07 | 0.118 | 0.056 |
| must_change F0.5 | 0.111 | 0.102 | 0.024 | 0.062 | 0.019 |

- relevant_files not retrieved anywhere: `src/middleware/serve-static/index.test.ts`, `src/utils/compress.ts`
- must_consider not retrieved anywhere: `src/middleware/serve-static/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 31 | 0 | 1699 | 0/38 | 0 | 0 | 0 |
| whole pack | 45 | 0 | 1821 | 0/38 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3975 | 25380 | 1 (1/1) | 1 (1/1) |
| 8000 | 5014 | 30673 | 1 (1/1) | 1 (1/1) |
| 16000 | 5014 | 30674 | 1 (1/1) | 1 (1/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `src/utils/accept::parseAccept` | yes | yes | no |

### `agent`, query 1 of 2

Query: `Make serveStatic precompressed lookup negotiate Accept-Encoding with q values`; sent by `2026-10-05-directed/t07-static-precompressed-quality.atlas.r0`.
Context pack: 8276 bytes, ~1612 of 6000 tokens; items: must read 6, skim 0, patterns 0, reuse 0, impact 0, tests 2, uncertain 0, omitted 0.

Searches: the query, then `precompressed lookup negotiate Accept-Encoding`.
Similar seeds: `ts:tree:hono:src/utils/cookie::makeSignature#function` (members 8, shape-only 0), `ts:tree:hono:src/middleware/compress/index::selectEncoding#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 0 (0/1) | 0 (0/1) | 0 (0/1) |
| relevant_files recall | 0.4 (2/5) | 0.4 (2/5) | 0.4 (2/5) | 0 (0/5) | 0.4 (2/5) |
| must_consider recall | 0.333 (2/6) | 0.333 (2/6) | 0.333 (2/6) | 0 (0/6) | 0.333 (2/6) |
| relevant_files precision | 1 | 0.5 | 0.222 | 0 | 0.1 |
| relevant_files F0.5 | 0.769 | 0.476 | 0.244 | 0 | 0.118 |
| must_change F0.5 | 0 | 0 | 0 | 0 | 0 |

- must_change not retrieved anywhere: `src/middleware/serve-static/index.ts`
- relevant_files not retrieved anywhere: `src/middleware/serve-static/index.test.ts`, `src/middleware/serve-static/index.ts`, `src/utils/compress.ts`
- must_consider not retrieved anywhere: `src/adapter/bun/serve-static.ts`, `src/adapter/cloudflare-workers/serve-static.ts`, `src/adapter/deno/serve-static.ts`, `src/middleware/serve-static/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 6 | 0 | 181 | 0/38 | 0 | 0 | 0 |
| whole pack | 8 | 0 | 1003 | 0/38 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1612 | 8276 | 0 (0/1) | 0 (0/1) |
| 8000 | 1612 | 8276 | 0 (0/1) | 0 (0/1) |
| 16000 | 1612 | 8277 | 0 (0/1) | 0 (0/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `src/utils/accept::parseAccept` | no | no | no |

### `agent`, query 2 of 2

Query: `Make serveStatic precompressed negotiate Accept-Encoding with quality values`; sent by `2026-10-05-directed/t07-static-precompressed-quality.atlas.r1`.
Context pack: 7493 bytes, ~1045 of 6000 tokens; items: must read 3, skim 1, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 0.

Searches: the query, then `serveStatic precompressed negotiate Accept-Encoding`, `quality values`.
Similar seeds: `ts:tree:hono:src/utils/cookie::makeSignature#function` (members 8, shape-only 0), `ts:tree:hono:src/middleware/compress/index::selectEncoding#function` (members 4, shape-only 0), `ts:tree:hono:src/utils/accept::parseQuality#function` (members 10, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 0 (0/1) | 1 (1/1) |
| relevant_files recall | 0.2 (1/5) | 0.4 (2/5) | 0.6 (3/5) | 0.2 (1/5) | 0.6 (3/5) |
| must_consider recall | 0.167 (1/6) | 0.333 (2/6) | 0.333 (2/6) | 0.167 (1/6) | 0.333 (2/6) |
| relevant_files precision | 1 | 0.5 | 0.231 | 0.059 | 0.1 |
| relevant_files F0.5 | 0.556 | 0.476 | 0.263 | 0.068 | 0.12 |
| must_change F0.5 | 0 | 0 | 0.094 | 0 | 0.041 |

- relevant_files not retrieved anywhere: `src/middleware/serve-static/index.test.ts`, `src/utils/compress.ts`
- must_consider not retrieved anywhere: `src/adapter/bun/serve-static.ts`, `src/adapter/cloudflare-workers/serve-static.ts`, `src/adapter/deno/serve-static.ts`, `src/middleware/serve-static/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 3 | 0 | 108 | 0/38 | 0 | 0 | 0 |
| whole pack | 12 | 0 | 193 | 0/38 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1045 | 7493 | 0 (0/1) | 0 (0/1) |
| 8000 | 1045 | 7493 | 0 (0/1) | 0 (0/1) |
| 16000 | 1045 | 7494 | 0 (0/1) | 0 (0/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `src/utils/accept::parseAccept` | yes | yes | yes |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `'serveStatic({ precompressed: true })' only serves a precompressed variant when the client lists the coding as a bare token`.
Context pack: 9041 bytes, ~1412 of 6000 tokens; items: must read 8, skim 0, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 0.

Searches: the query, then `serveStatic({ precompressed: true })`, `precompressed variant`, `client lists`, `bare token`.
Similar seeds: `ts:tree:hono:src/client/utils::parseResponse#function` (members 2, shape-only 0), `ts:tree:@hono/cloudflare-workers:src/serve-static::serveStatic#function` (members 10, shape-only 0), `ts:tree:hono:src/middleware/cache/index::cache#function` (members 10, shape-only 0), `ts:tree:hono:src/jsx/dom/render::findChildNodeIndex#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/1) | 0 (0/1) | 1 (1/1) | 1 (1/1) | 1 (1/1) |
| relevant_files recall | 0 (0/5) | 0 (0/5) | 0.2 (1/5) | 0.2 (1/5) | 0.2 (1/5) |
| must_consider recall | 0 (0/6) | 0 (0/6) | 0.5 (3/6) | 0.5 (3/6) | 0.5 (3/6) |
| relevant_files precision | 0 | 0 | 0.04 | 0.053 | 0.027 |
| relevant_files F0.5 | 0 | 0 | 0.048 | 0.062 | 0.033 |
| must_change F0.5 | 0 | 0 | 0.05 | 0.065 | 0.034 |

- relevant_files not retrieved anywhere: `src/middleware/compress/index.ts`, `src/middleware/serve-static/index.test.ts`, `src/utils/accept.ts`, `src/utils/compress.ts`
- must_consider not retrieved anywhere: `src/middleware/compress/index.ts`, `src/middleware/serve-static/index.test.ts`, `src/utils/accept.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 8 | 0 | 530 | 0/38 | 0 | 0 | 0 |
| whole pack | 16 | 0 | 584 | 0/38 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1412 | 9041 | 0 (0/1) | 0 (0/1) |
| 8000 | 1412 | 9041 | 0 (0/1) | 0 (0/1) |
| 16000 | 1412 | 9042 | 0 (0/1) | 0 (0/1) |

| reuse target | pack | search | similar |
|---|---|---|---|
| `src/utils/accept::parseAccept` | no | no | no |

## t08-cookie-samesite-default: Default SameSite=Lax for cookies created by the cookie helper

Categories: test-discovery, cross-package.

Index: 486 files in tree.
Line gold: 64 old-side lines in 8 base files of `reference.patch`, 0 of them in files outside the index.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.125 | 0.286 | 0.27 | 0.027 | 0.125 | 0.125 | 0.125 |
| full | 1 | whole pack | 0.625 | 0.353 | 0.39 | 0.027 | 0.625 | 0.625 | 0.625 |
| agent | 2 | MUST READ | 0.125 | 1 | 0.385 | 0.547 | 0.125 | 0.125 | 0.125 |
| agent | 2 | whole pack | 0.625 | 0.708 | 0.699 | 0.058 | 0.625 | 0.625 | 0.625 |
| agent-seeded | 2 | MUST READ | 0.125 | 1 | 0.385 | 0.547 | 0.125 | 0.125 | 0.125 |
| agent-seeded | 2 | whole pack | 0.625 | 0.708 | 0.699 | 0.058 | 0.625 | 0.625 | 0.625 |
| first-line | 1 | MUST READ | 0.125 | 1 | 0.385 | 0.547 | 0.125 | 0.125 | 0.125 |
| first-line | 1 | whole pack | 0.625 | 0.462 | 0.492 | 0.04 | 0.625 | 0.625 | 0.625 |

### `full`

Query: the task prompt (1295 characters).
Context pack: 26825 bytes, ~4279 of 6000 tokens; items: must read 20, skim 18, patterns 0, reuse 0, impact 2, tests 8, uncertain 0, omitted 37.

Searches: the query, then `hono/cookie`, `setCookie`, `setSignedCookie`, `generateCookie`, `generateSignedCookie`, `deleteCookie`, `SameSite`, `Set-Cookie`, `Cookies created through`, `attribute unless`, `caller passes`, `Browsers disagree`.
Similar seeds: `ts:tree:hono:src/utils/cookie::serializeSigned#function` (members 10, shape-only 0), `ts:tree:hono:src/helper/cookie/index::getCookie#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setCookie#function` (members 5, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setSignedCookie#function` (members 8, shape-only 0), `ts:tree:hono:src/helper/cookie/index::generateCookie#function` (members 6, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.125 (1/8) | 0.625 (5/8) | 0.5 (4/8) | 0.125 (1/8) | 0.75 (6/8) |
| relevant_files recall | 0.222 (2/9) | 0.667 (6/9) | 0.556 (5/9) | 0.222 (2/9) | 0.778 (7/9) |
| must_consider recall | 0.25 (1/4) | 0.5 (2/4) | 1 (4/4) | 0.25 (1/4) | 1 (4/4) |
| relevant_files precision | 0.286 | 0.353 | 0.143 | 0.5 | 0.156 |
| relevant_files F0.5 | 0.27 | 0.39 | 0.168 | 0.4 | 0.185 |
| must_change F0.5 | 0.139 | 0.329 | 0.135 | 0.208 | 0.16 |

- must_change not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`
- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 20 | 0 | 1633 | 35/64 | 0.021 | 0.547 | 0.027 |
| whole pack | 48 | 0 | 2306 | 51/64 | 0.022 | 0.797 | 0.027 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3995 | 25688 | 0.125 (1/8) | 0.625 (5/8) |
| 8000 | 4279 | 26825 | 0.125 (1/8) | 0.625 (5/8) |
| 16000 | 4279 | 26826 | 0.125 (1/8) | 0.625 (5/8) |

### `agent`, query 1 of 2

Query: `Default SameSite=Lax in hono/cookie setCookie, setSignedCookie, generateCookie, generateSignedCookie, deleteCookie`; sent by `2026-10-05-directed/t08-cookie-samesite-default.atlas.r0`.
Context pack: 8536 bytes, ~1274 of 6000 tokens; items: must read 5, skim 10, patterns 0, reuse 0, impact 1, tests 8, uncertain 0, omitted 12.

Searches: the query, then `Default SameSite`, `cookie setCookie`.
Similar seeds: `ts:tree:hono:src/helper/cookie/index::generateSignedCookie#function` (members 9, shape-only 0), `ts:tree:hono:src/middleware/csrf/index::isSecFetchSite#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setCookie#function` (members 5, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.125 (1/8) | 0.625 (5/8) | 0.125 (1/8) | 0.125 (1/8) | 0.625 (5/8) |
| relevant_files recall | 0.111 (1/9) | 0.667 (6/9) | 0.222 (2/9) | 0.222 (2/9) | 0.667 (6/9) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.5 (2/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files precision | 1 | 0.667 | 0.25 | 0.333 | 0.375 |
| relevant_files F0.5 | 0.385 | 0.667 | 0.244 | 0.303 | 0.411 |
| must_change F0.5 | 0.417 | 0.568 | 0.125 | 0.156 | 0.347 |

- must_change not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- must_consider not retrieved anywhere: `adapters/aws-lambda/src/handler.ts`, `src/adapter/aws-lambda/handler.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 5 | 0 | 64 | 35/64 | 0.547 | 0.547 | 0.547 |
| whole pack | 24 | 0 | 1131 | 53/64 | 0.047 | 0.828 | 0.058 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1274 | 8536 | 0.125 (1/8) | 0.625 (5/8) |
| 8000 | 1274 | 8536 | 0.125 (1/8) | 0.625 (5/8) |
| 16000 | 1274 | 8537 | 0.125 (1/8) | 0.625 (5/8) |

### `agent`, query 2 of 2

Query: `Default SameSite=Lax in hono/cookie helpers (setCookie, setSignedCookie, generateCookie, generateSignedCookie, deleteCookie)`; sent by `2026-10-05-directed/t08-cookie-samesite-default.atlas.r1`.
Context pack: 8353 bytes, ~1227 of 6000 tokens; items: must read 5, skim 9, patterns 0, reuse 0, impact 1, tests 8, uncertain 0, omitted 12.

Searches: the query, then `Default SameSite`, `cookie helpers`.
Similar seeds: `ts:tree:hono:src/helper/cookie/index::generateSignedCookie#function` (members 9, shape-only 0), `ts:tree:hono:src/middleware/csrf/index::isSecFetchSite#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/cookie/index::getCookie#function` (members 1, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.125 (1/8) | 0.625 (5/8) | 0.25 (2/8) | 0.125 (1/8) | 0.625 (5/8) |
| relevant_files recall | 0.111 (1/9) | 0.667 (6/9) | 0.333 (3/9) | 0.222 (2/9) | 0.667 (6/9) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.5 (2/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files precision | 1 | 0.75 | 0.333 | 0.4 | 0.375 |
| relevant_files F0.5 | 0.385 | 0.732 | 0.333 | 0.345 | 0.411 |
| must_change F0.5 | 0.417 | 0.625 | 0.227 | 0.179 | 0.347 |

- must_change not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`, `adapters/lambda-edge/src/handler.test.ts`
- must_consider not retrieved anywhere: `adapters/aws-lambda/src/handler.ts`, `src/adapter/aws-lambda/handler.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 5 | 0 | 64 | 35/64 | 0.547 | 0.547 | 0.547 |
| whole pack | 23 | 0 | 1130 | 53/64 | 0.047 | 0.828 | 0.058 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1227 | 8353 | 0.125 (1/8) | 0.625 (5/8) |
| 8000 | 1227 | 8353 | 0.125 (1/8) | 0.625 (5/8) |
| 16000 | 1227 | 8354 | 0.125 (1/8) | 0.625 (5/8) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `Cookies created through 'hono/cookie' ('setCookie', 'setSignedCookie', 'generateCookie', 'generateSignedCookie', 'deleteCookie') omit the 'SameSite' attribute unless the caller passes 'sameSite'`.
Context pack: 11299 bytes, ~1825 of 6000 tokens; items: must read 5, skim 16, patterns 2, reuse 0, impact 1, tests 8, uncertain 0, omitted 13.

Searches: the query, then `hono/cookie`, `setCookie`, `setSignedCookie`, `generateCookie`, `generateSignedCookie`, `deleteCookie`, `SameSite`, `Cookies created through`, `attribute unless`, `caller passes`.
Similar seeds: `ts:tree:hono:src/helper/cookie/index::generateSignedCookie#function` (members 9, shape-only 0), `ts:tree:hono:src/helper/cookie/index::getCookie#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setCookie#function` (members 5, shape-only 0), `ts:tree:hono:src/helper/cookie/index::setSignedCookie#function` (members 8, shape-only 0), `ts:tree:hono:src/helper/cookie/index::generateCookie#function` (members 6, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.125 (1/8) | 0.625 (5/8) | 0.5 (4/8) | 0.125 (1/8) | 0.75 (6/8) |
| relevant_files recall | 0.111 (1/9) | 0.667 (6/9) | 0.556 (5/9) | 0.222 (2/9) | 0.778 (7/9) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 1 (4/4) | 0.25 (1/4) | 1 (4/4) |
| relevant_files precision | 1 | 0.462 | 0.167 | 0.4 | 0.184 |
| relevant_files F0.5 | 0.385 | 0.492 | 0.194 | 0.345 | 0.217 |
| must_change F0.5 | 0.417 | 0.417 | 0.156 | 0.179 | 0.188 |

- must_change not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`
- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `adapters/aws-lambda/test/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 5 | 0 | 64 | 35/64 | 0.547 | 0.547 | 0.547 |
| whole pack | 32 | 0 | 1656 | 53/64 | 0.032 | 0.828 | 0.04 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1825 | 11299 | 0.125 (1/8) | 0.625 (5/8) |
| 8000 | 1825 | 11299 | 0.125 (1/8) | 0.625 (5/8) |
| 16000 | 1825 | 11300 | 0.125 (1/8) | 0.625 (5/8) |

## t09-lambda-text-content-types: Lambda adapters base64-encode textual responses such as text/markdown and text/event-stream

Categories: cross-package, consistency.

Index: 486 files in tree.
Line gold: 4 old-side lines in 4 base files of `reference.patch`, 0 of them in files outside the index.
Runs that sent no `atlas_context` call: `2026-10-05-directed/t09-lambda-text-content-types.atlas.r0`.

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.75 | 0.273 | 0.288 | 0.001 | 0.75 | 0.75 | 0.75 |
| full | 1 | whole pack | 1 | 0.235 | 0.263 | 0.001 | 1 | 1 | 1 |
| agent | 1 | MUST READ | 0.5 | 0.667 | 0.5 | 0.125 | 0.5 | 0.5 | 0.5 |
| agent | 1 | whole pack | 0.5 | 0.8 | 0.714 | 0.004 | 0.5 | 0.5 | 0.5 |
| agent-seeded | 1 | MUST READ | 0.5 | 0.667 | 0.5 | 0.125 | 0.5 | 0.5 | 0.5 |
| agent-seeded | 1 | whole pack | 0.5 | 0.8 | 0.714 | 0.004 | 0.5 | 0.5 | 0.5 |
| first-line | 1 | MUST READ | 0.75 | 0.75 | 0.625 | 0 | 0.75 | 0.75 | 0.75 |
| first-line | 1 | whole pack | 1 | 0.857 | 0.833 | 0 | 1 | 1 | 1 |

### `full`

Query: the task prompt (1402 characters).
Context pack: 32902 bytes, ~5003 of 6000 tokens; items: must read 32, skim 4, patterns 0, reuse 10, impact 0, tests 8, uncertain 10, omitted 69.

Searches: the query, then `Content-Type: text/markdown`, `text/event-stream`, `text/calendar`, `application/javascript`, `bodyEncoding: 'base64'`, `text/*`, `application/x-www-form-urlencoded`, `charset`, `come back base64-encoded`, `AWS Lambda deployment`, `API Gateway REST`, `media types configured shows`.
Similar seeds: `ts:tree:@hono/aws-lambda:src/handler::EventProcessor::createResult#method` (members 10, shape-only 0), `ts:tree:hono:src/helper/ssg/ssg::parseResponseContent#function` (members 10, shape-only 0), `ts:tree:hono:src/helper/streaming/text::streamText#function` (members 5, shape-only 0), `ts:tree:hono:src/context::Context::text#method` (members 2, shape-only 0), `ts:tree:hono:src/jsx/dom/hooks/index::useFormStatus#function` (members 4, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.75 (3/4) | 1 (4/4) | 1 (4/4) | 0.75 (3/4) | 1 (4/4) |
| relevant_files recall | 0.375 (3/8) | 0.5 (4/8) | 1 (8/8) | 0.375 (3/8) | 1 (8/8) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 1 (4/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | 0.273 | 0.235 | 0.163 | 0.167 | 0.136 |
| relevant_files F0.5 | 0.288 | 0.263 | 0.196 | 0.188 | 0.164 |
| must_change F0.5 | 0.312 | 0.278 | 0.1 | 0.197 | 0.083 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 32 | 0 | 1740 | 2/4 | 0.001 | 0.5 | 0.001 |
| whole pack | 64 | 0 | 2362 | 2/4 | 0.001 | 0.5 | 0.001 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 3997 | 27715 | 0.75 (3/4) | 1 (4/4) |
| 8000 | 5007 | 33003 | 0.75 (3/4) | 1 (4/4) |
| 16000 | 5007 | 33004 | 0.75 (3/4) | 1 (4/4) |

### `agent`

Query: `Fix isContentTypeBinary in aws-lambda and lambda-edge adapters (built-in and standalone packages) to treat all text/*, application/javascript, x-www-form-urlencoded as text`; sent by `2026-10-05-directed/t09-lambda-text-content-types.atlas.r1`.
Context pack: 4266 bytes, ~451 of 6000 tokens; items: must read 3, skim 0, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 0.

Searches: the query, then `Fix isContentTypeBinary`, `lambda-edge adapters`, `standalone packages`.
Similar seeds: `ts:tree:hono:src/adapter/lambda-edge/handler::isContentTypeBinary#function` (members 5, shape-only 0), `ts:tree:@hono/lambda-edge:src/handler::isContentTypeBinary#function` (members 5, shape-only 0), `ts:tree:@hono/lambda-edge:src/handler::getCloudFrontRecord#function` (members 1, shape-only 0), `ts:tree:hono:src/compose::compose#function` (members 7, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.5 (2/4) | 0.5 (2/4) | 1 (4/4) | 1 (4/4) | 1 (4/4) |
| relevant_files recall | 0.25 (2/8) | 0.5 (4/8) | 0.75 (6/8) | 0.5 (4/8) | 1 (8/8) |
| must_consider recall | 0 (0/4) | 0.5 (2/4) | 0.5 (2/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | 0.667 | 0.8 | 0.353 | 0.4 | 0.32 |
| relevant_files F0.5 | 0.5 | 0.714 | 0.395 | 0.417 | 0.37 |
| must_change F0.5 | 0.625 | 0.417 | 0.278 | 0.455 | 0.192 |

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 3 | 0 | 19 | 2/4 | 0.105 | 0.5 | 0.125 |
| whole pack | 11 | 0 | 572 | 2/4 | 0.003 | 0.5 | 0.004 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 451 | 4266 | 0.5 (2/4) | 0.5 (2/4) |
| 8000 | 451 | 4266 | 0.5 (2/4) | 0.5 (2/4) |
| 16000 | 451 | 4267 | 0.5 (2/4) | 0.5 (2/4) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `Responses with 'Content-Type: text/markdown', 'text/event-stream', 'text/calendar' or 'application/javascript' come back base64-encoded from our AWS Lambda deployment (API Gateway REST without binary media types configured shows the base64 text to the client), and Lambda@Edge marks the same responses with 'bodyEncoding: 'base64''`.
Context pack: 11247 bytes, ~1490 of 6000 tokens; items: must read 6, skim 11, patterns 0, reuse 4, impact 0, tests 8, uncertain 0, omitted 11.

Searches: the query, then `Content-Type: text/markdown`, `text/event-stream`, `text/calendar`, `application/javascript`, `bodyEncoding: 'base64'`, `come back base64-encoded`, `AWS Lambda deployment`, `API Gateway REST`, `media types configured shows`.
Similar seeds: `ts:tree:@hono/aws-lambda:src/handler::EventProcessor::createResult#method` (members 10, shape-only 0), `ts:tree:hono:src/helper/ssg/ssg::parseResponseContent#function` (members 10, shape-only 0), `ts:tree:hono:src/helper/streaming/text::streamText#function` (members 5, shape-only 0), `ts:tree:hono:src/context::Context::text#method` (members 2, shape-only 0), `ts:tree:hono:src/utils/encode::encodeBase64Url#function` (members 6, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.75 (3/4) | 1 (4/4) | 1 (4/4) | 0.75 (3/4) | 1 (4/4) |
| relevant_files recall | 0.375 (3/8) | 0.75 (6/8) | 0.5 (4/8) | 0.375 (3/8) | 0.75 (6/8) |
| must_consider recall | 0 (0/4) | 0.5 (2/4) | 0 (0/4) | 0 (0/4) | 0.5 (2/4) |
| relevant_files precision | 0.75 | 0.857 | 0.105 | 0.158 | 0.128 |
| relevant_files F0.5 | 0.625 | 0.833 | 0.125 | 0.179 | 0.153 |
| must_change F0.5 | 0.75 | 0.625 | 0.128 | 0.188 | 0.104 |

- relevant_files not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `src/adapter/aws-lambda/handler.test.ts`
- must_consider not retrieved anywhere: `adapters/aws-lambda/src/handler.test.ts`, `src/adapter/aws-lambda/handler.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 6 | 0 | 157 | 0/4 | 0 | 0 | 0 |
| whole pack | 29 | 0 | 545 | 0/4 | 0 | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1490 | 11247 | 0.75 (3/4) | 1 (4/4) |
| 8000 | 1490 | 11247 | 0.75 (3/4) | 1 (4/4) |
| 16000 | 1490 | 11248 | 0.75 (3/4) | 1 (4/4) |

## t10-auth-challenge-quoting: WWW-Authenticate challenges do not escape backslashes in quoted-string parameters

Categories: cross-cutting, consistency.

Index: 486 files in tree.
Line gold: 21 old-side lines in 4 base files of `reference.patch`, 0 of them in files outside the index.
- added by the patch, no base lines (not scored): `src/utils/quoted-string.ts`

| variant | queries | section | must_change recall | relevant_files precision | file F0.5 | line F0.5 | yield@4000 | yield@8000 | yield@16000 |
|---|---|---|---|---|---|---|---|---|---|
| full | 1 | MUST READ | 0.25 | 0.5 | 0.312 | 0.018 | 0.25 | 0.25 | 0.25 |
| full | 1 | whole pack | 0.25 | 0.333 | 0.312 | 0.002 | 0.25 | 0.25 | 0.25 |
| agent | 2 | MUST READ | 0 | n/a | 0 | 0 | 0 | 0 | 0 |
| agent | 2 | whole pack | 0 | n/a | 0 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | MUST READ | 0 | n/a | 0 | 0 | 0 | 0 | 0 |
| agent-seeded | 2 | whole pack | 0 | n/a | 0 | 0 | 0 | 0 | 0 |
| first-line | 1 | MUST READ | 0.25 | 0.25 | 0.208 | 0.004 | 0.25 | 0.25 | 0.25 |
| first-line | 1 | whole pack | 0.75 | 0.333 | 0.357 | 0.007 | 0.75 | 0.75 | 0.75 |

### `full`

Query: the task prompt (1249 characters).
Context pack: 8616 bytes, ~992 of 6000 tokens; items: must read 2, skim 6, patterns 0, reuse 0, impact 0, tests 8, uncertain 0, omitted 3.

Searches: the query, then `bearerAuth({ token, realm: 'C:\\vault' })`, `WWW-Authenticate: Bearer realm="C:\vault"`, `C:vault`, `invalidToken: { wwwAuthenticateHeader: { error: 'invalid_token', error_description: 'bad "token"' } }`, `WWW-Authenticate`, `pnpm`, `npx pnpm@12.6.0 ...`, `./node_modules/.bin`, `clients receive`, `unterminated quoted-string`, `Double quotes`, `cut short`.
Similar seeds: `ts:tree:hono:src/middleware/bearer-auth/index::bearerAuth#function` (members 1, shape-only 0), `ts:tree:hono:src/utils/jwt/jwt::decodeHeader#function` (members 2, shape-only 0), `ts:tree:hono:src/utils/ipaddr::parseIPv6HexCode#function` (members 10, shape-only 0), `ts:tree:@hono/cloudflare-workers:src/serve-static-module::module#function` (members 8, shape-only 0), `ts:tree:hono:src/helper/websocket/index::createWSMessageEvent#function` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.25 (1/4) | 0.25 (1/4) | 0.25 (1/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files recall | 0.125 (1/8) | 0.25 (2/8) | 0.5 (4/8) | 0.125 (1/8) | 0.625 (5/8) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.75 (3/4) | 0 (0/4) | 0.75 (3/4) |
| relevant_files precision | 0.5 | 0.333 | 0.111 | 0.071 | 0.098 |
| relevant_files F0.5 | 0.312 | 0.312 | 0.132 | 0.078 | 0.118 |
| must_change F0.5 | 0.417 | 0.179 | 0.034 | 0.083 | 0.048 |

- must_change not retrieved anywhere: `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- relevant_files not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`, `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- must_consider not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 2 | 0 | 137 | 2/21 | 0.015 | 0.095 | 0.018 |
| whole pack | 16 | 0 | 1639 | 2/21 | 0.001 | 0.095 | 0.002 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 992 | 8616 | 0.25 (1/4) | 0.25 (1/4) |
| 8000 | 992 | 8616 | 0.25 (1/4) | 0.25 (1/4) |
| 16000 | 992 | 8617 | 0.25 (1/4) | 0.25 (1/4) |

### `agent`, query 1 of 2

Query: `Escape quoted-string auth-params in WWW-Authenticate headers in all built-in auth middleware (bearer, basic, digest)`; sent by `2026-10-05-directed/t10-auth-challenge-quoting.atlas.r0`.
Context pack: 560 bytes, ~200 of 6000 tokens; items: must read 0, skim 0, patterns 0, reuse 0, impact 0, tests 0, uncertain 0, omitted 0.

Searches: the query, then `Escape quoted-string auth-params`, `WWW-Authenticate headers`, `built-in auth middleware`.
Similar seeds: `ts:tree:hono:src/middleware/basic-auth/index::basicAuth#function` (members 1, shape-only 0), `ts:tree:hono:src/middleware/bearer-auth/index::bearerAuth#function` (members 1, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/4) | 0 (0/4) | 0.5 (2/4) | 0.5 (2/4) | 0.5 (2/4) |
| relevant_files recall | 0 (0/8) | 0 (0/8) | 0.75 (6/8) | 0.25 (2/8) | 0.75 (6/8) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 1 (4/4) | 0 (0/4) | 1 (4/4) |
| relevant_files precision | n/a | n/a | 0.462 | 1 | 0.462 |
| relevant_files F0.5 | 0 | 0 | 0.5 | 0.625 | 0.5 |
| must_change F0.5 | 0 | 0 | 0.179 | 0.833 | 0.179 |

- must_change not retrieved anywhere: `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- relevant_files not retrieved anywhere: `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 0 | 0 | 0 | 0/21 | n/a | 0 | 0 |
| whole pack | 0 | 0 | 0 | 0/21 | n/a | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 200 | 560 | 0 (0/4) | 0 (0/4) |
| 8000 | 200 | 560 | 0 (0/4) | 0 (0/4) |
| 16000 | 200 | 561 | 0 (0/4) | 0 (0/4) |

### `agent`, query 2 of 2

Query: `Escape quoted-string auth-params in WWW-Authenticate headers across built-in auth middlewares (bearer, basic, digest)`; sent by `2026-10-05-directed/t10-auth-challenge-quoting.atlas.r1`.
Context pack: 561 bytes, ~200 of 6000 tokens; items: must read 0, skim 0, patterns 0, reuse 0, impact 0, tests 0, uncertain 0, omitted 0.

Searches: the query, then `Escape quoted-string auth-params`, `across built-in auth middlewares`.
Similar seeds: `ts:tree:hono:src/middleware/bearer-auth/index::bearerAuth#function` (members 1, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0 (0/4) | 0 (0/4) | 0.5 (2/4) | 0.25 (1/4) | 0.5 (2/4) |
| relevant_files recall | 0 (0/8) | 0 (0/8) | 0.625 (5/8) | 0.125 (1/8) | 0.625 (5/8) |
| must_consider recall | 0 (0/4) | 0 (0/4) | 0.75 (3/4) | 0 (0/4) | 0.75 (3/4) |
| relevant_files precision | n/a | n/a | 0.556 | 1 | 0.556 |
| relevant_files F0.5 | 0 | 0 | 0.568 | 0.417 | 0.568 |
| must_change F0.5 | 0 | 0 | 0.25 | 0.625 | 0.25 |

- must_change not retrieved anywhere: `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- relevant_files not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`, `src/middleware/jwk/jwk.ts`, `src/middleware/jwt/jwt.ts`
- must_consider not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 0 | 0 | 0 | 0/21 | n/a | 0 | 0 |
| whole pack | 0 | 0 | 0 | 0/21 | n/a | 0 | 0 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 200 | 561 | 0 (0/4) | 0 (0/4) |
| 8000 | 200 | 561 | 0 (0/4) | 0 (0/4) |
| 16000 | 200 | 562 | 0 (0/4) | 0 (0/4) |

### `agent-seeded`

Same as `agent`: no recorded call of this task carries seeds.

### `first-line`

Query: `We configured 'bearerAuth({ token, realm: 'C:\\vault' })' and clients receive 'WWW-Authenticate: Bearer realm="C:\vault"', which they parse as 'C:vault'`.
Context pack: 11126 bytes, ~1678 of 6000 tokens; items: must read 5, skim 10, patterns 2, reuse 2, impact 0, tests 8, uncertain 0, omitted 4.

Searches: the query, then `bearerAuth({ token, realm: 'C:\\vault' })`, `WWW-Authenticate: Bearer realm="C:\vault"`, `C:vault`, `clients receive`.
Similar seeds: `ts:tree:hono:src/middleware/bearer-auth/index::bearerAuth#function` (members 1, shape-only 0), `ts:tree:hono:src/helper/websocket/index::createWSMessageEvent#function` (members 2, shape-only 0).

| files | must read | pack | search | similar | any |
|---|---|---|---|---|---|
| must_change recall | 0.25 (1/4) | 0.75 (3/4) | 0.25 (1/4) | 0.25 (1/4) | 1 (4/4) |
| relevant_files recall | 0.125 (1/8) | 0.5 (4/8) | 0.5 (4/8) | 0.125 (1/8) | 0.875 (7/8) |
| must_consider recall | 0 (0/4) | 0.25 (1/4) | 0.75 (3/4) | 0 (0/4) | 0.75 (3/4) |
| relevant_files precision | 0.25 | 0.333 | 0.444 | 0.333 | 0.318 |
| relevant_files F0.5 | 0.208 | 0.357 | 0.455 | 0.25 | 0.365 |
| must_change F0.5 | 0.25 | 0.288 | 0.125 | 0.312 | 0.217 |

- relevant_files not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`
- must_consider not retrieved anywhere: `src/middleware/basic-auth/index.test.ts`

| lines | items | items without a span | lines retrieved | gold lines hit | precision | recall | F0.5 |
|---|---|---|---|---|---|---|---|
| MUST READ | 5 | 0 | 618 | 2/21 | 0.003 | 0.095 | 0.004 |
| whole pack | 27 | 0 | 2007 | 12/21 | 0.006 | 0.571 | 0.007 |

| budget | estimated tokens | bytes | must_change in MUST READ | must_change in the whole pack |
|---|---|---|---|---|
| 4000 | 1678 | 11126 | 0.25 (1/4) | 0.75 (3/4) |
| 8000 | 1678 | 11126 | 0.25 (1/4) | 0.75 (3/4) |
| 16000 | 1678 | 11127 | 0.25 (1/4) | 0.75 (3/4) |

