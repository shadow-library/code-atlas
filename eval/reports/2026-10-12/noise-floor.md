# Noise floor: how much do two runs of the same condition differ?

Computed from recorded runs only; no session was run for this report. An A/B effect smaller than the difference between two runs of the same condition is not evidence, so this report measures that difference for every metric of the agent report and places the reported A/B effects next to it.

This is the noise floor of the task set that exists: 11 tasks written by the authors of Code Atlas, on which the main outcome sits at its ceiling. It is not the A/A run on a harder task set that the roadmap asks for before any arm is compared; that needs a task set which does not exist yet. A harder, more varied task set will have a different and probably larger variance, so none of the bounds below transfer to it. In the repetition pairs of these batches the hidden tests passed in 88 of 88 runs.

| batch | date recorded | preamble | tasks | scored runs | excluded |
|---|---|---|---|---|---|
| 2026-10-04-seed1 | 2026-10-04 | 1 | 11 | 44 | none |
| 2026-10-05-directed | 2026-10-04 | 2 (directed) | 11 | 44 | none |

## Is the baseline the same in every batch?

### 2026-10-04-seed1 and 2026-10-05-directed

**Identical.** Every field that defines a baseline session is the same in both batches for all 11 tasks, so the two baselines are compared task by task.

| compared | 2026-10-04-seed1 | 2026-10-05-directed | |
|---|---|---|---|
| session model | claude-sonnet-5-5 | claude-sonnet-5-5 | same |
| claude CLI | 2.1.289 (Claude Code) | 2.1.289 (Claude Code) | same |
| claude config | user | user | same |
| judge model | claude-sonnet-5-5 | claude-sonnet-5-5 | same |
| repo override in use | false | false | same |
| probe recorded | true | true | same |
| probe: Claude Code version | 2.1.289 | 2.1.289 | same |
| probe: models reported | ["claude-sonnet-5-5"] | ["claude-sonnet-5-5"] | same |
| probe: permission mode | bypassPermissions | bypassPermissions | same |
| probe: tools | ["Bash","Edit","Glob","Grep","Read","Write"] | ["Bash","Edit","Glob","Grep","Read","Write"] | same |
| probe: MCP servers | [] | [] | same |
| probe: skills | [] | [] | same |
| probe: slash commands | [] | [] | same |
| probe: plugins | ["cc-plugin-agents-md","cc-plugin-telemetry","cc-plugin-plugin-authoring"] | ["cc-plugin-agents-md","cc-plugin-telemetry","cc-plugin-plugin-authoring"] | same |
| probe: agents | ["claude","Explore","general-purpose","Plan","statusline-setup"] | ["claude","Explore","general-purpose","Plan","statusline-setup"] | same |
| probe: project CLAUDE.md seen | {"verdict":"not_seen"} | {"verdict":"not_seen"} | same |
| probe: user CLAUDE.md seen | {"verdict":"not_seen"} | {"verdict":"not_seen"} | same |
| date recorded | 2026-10-04 | 2026-10-04 | same |
| seed | 1 | 1 | same |
| eval repository commit | 0dc8a96fffd02b1a82f0da9e5469ed0128f90a74 | 3c54d92821abe63e4e25508b9fa7d3e6c4ae7d84 | **differs** |

Per task, over every scored baseline run of both batches, these recorded fields were compared: task spec hash (prompt, ground truth, validation, base patch); tree source (repository, commit, subdirectory); base commit of the tree; injected project instructions; model requested; models reported; Claude Code version; command line with the scratch directory masked (flags, system prompt, prompt); environment variable names; preamble.

Differences that do not define the baseline session but do describe the harness that ran and scored it: eval repository commit. `git diff --name-only` between the two commits lists these files under `crates/atlas-eval`: `src/agent/error.rs`, `src/agent/plan.rs`, `src/agent/record.rs`, `src/agent/report.rs`, `src/agent/runner.rs`, `src/main.rs`, `tests/agent_pipeline.rs`. Metrics the harness derives (reads, judged scores) are comparable across the batches only as far as those files leave the scoring unchanged.

## Definitions

- **Runs.** Completed, uncontaminated runs, the filter `agent report` applies. Metrics are the ones `agent report` aggregates, read through the same extraction code, plus `cache_weighted_tokens` = input + 1.25 × cache-creation + 0.1 × cache-read tokens (output tokens are not part of it).
- **A/A difference.** A difference between two measurements of the same task under the same condition. Two kinds are computed. *Repetitions*: the second run minus the first run of a task within one batch and condition (one run on each side). *Across batches*: the baseline per-task mean of one batch minus that of the other (two runs on each side), only for tasks whose baseline is identical in both batches. Run indices and batch order carry no meaning, so only the size of an A/A difference matters, not its sign.
- **Per comparison.** `pairs`: tasks with a value on both sides. `median |diff|`: the median absolute per-task difference. `median relative`: the median of |b − a| / ((|a| + |b|) / 2) over the tasks where that is defined. `range`: the smallest and largest signed per-task difference. `paired difference`: mean over the paired tasks of side b minus mean over the same tasks of side a, the statistic `agent report` uses for A/B (difference of the means of per-task means), with its size relative to side a.
- **A/B effect.** Atlas minus baseline, as the mean over the tasks that have the metric in both conditions of (atlas per-task mean − baseline per-task mean); `relative` divides it by the baseline mean of those same tasks. The `baseline` and `atlas` columns are the aggregate means `agent report` prints. When both conditions cover the same tasks, the effect equals their difference; where they do not, the `tasks` column gives the three counts.
- **Replicate standard error (SE).** For each task, the variance of each condition's per-task mean is estimated from its repetitions (sample variance / runs; with two runs that is (r1 − r0)² / 4). SE = √(mean over tasks of (baseline variance + atlas variance) / tasks). It uses only differences between repetitions of one condition. `this batch` takes the variances from the batch whose effect is judged, `pooled` from all batches given.
- **One scale.** A repetition difference compares one run with one run, an A/B effect compares the mean of the runs of each cell. With two runs per cell the first has √2 times the standard deviation of the second, so every repetition difference is multiplied by √(mean over the cells of 1 / runs) before it is compared with an effect. The across-batch baseline difference compares cell means and needs no scaling.
- **Noise bound.** The largest of 2 × SE of the batch, 2 × pooled SE, and the largest paired A/A difference on the A/B scale observed for the metric in any comparison (repetitions of either condition in any batch, and baseline across batches). The third term is a guard: where repetitions tie often, as judged 1 to 5 scores do, the SE can come out too small, and a difference that large between two runs of one condition was nevertheless observed. An effect is called outside only if it exceeds every one of the three.
- **Effect / SE.** The effect divided by the SE of the batch, and by the pooled SE, printed beside every class so that a label can be checked against the usual 2.
- **Leave one task out.** The whole classification of a metric is repeated with each task removed in turn (its runs leave the effect, both SEs and every A/A difference). The last column counts the tasks whose removal moves the effect across the bound, from outside to not outside or the reverse; the JSON names them.
- **Classes.** `no difference`: the effect is exactly 0. `inside`: |effect| ≤ noise bound. `outside`: |effect| > noise bound. `undetermined`: no repetition exists to estimate noise from.

## A/B effects against the noise

### Batch 2026-10-04-seed1 (preamble 1)

35 metrics with a value in both conditions: 6 outside the noise, 23 inside, 6 with no difference, 0 undetermined.

- `reuse_targets_referenced`: -0.25 (-25%), – SE of the batch and -2.83 pooled SE, noise bound 0.1768; atlas worse; **falls inside the bound when any one of 1 of its 4 tasks is left out**.
- `total_tokens`: +38650 (+18.7%), +2.22 SE of the batch and +2.21 pooled SE, noise bound 34937; atlas worse; **falls inside the bound when any one of 6 of its 11 tasks is left out**.
- `cache_read_tokens`: +37650 (+20.4%), +2.32 SE of the batch and +2.28 pooled SE, noise bound 33069; atlas worse; **falls inside the bound when any one of 6 of its 11 tasks is left out**.
- `files_read`: -0.7273 (-17%), -2.10 SE of the batch and -2.12 pooled SE, noise bound 0.6923; atlas better; **falls inside the bound when any one of 7 of its 11 tasks is left out**.
- `atlas_calls`: +0.09091 (–), – SE of the batch and +2.83 pooled SE, noise bound 0.06428; no preferred direction; **falls inside the bound when any one of 1 of its 11 tasks is left out**.
- `atlas_bytes`: +1018 (–), +2488.33 SE of the batch and +3.81 pooled SE, noise bound 533.8; no preferred direction; **falls inside the bound when any one of 1 of its 11 tasks is left out**.

Inside the bound although the effect exceeds 2 × SE of this batch:

- `num_turns`: -1.455 (-11.5%), -2.11 SE of the batch and -1.43 pooled SE; kept inside by 2 × pooled SE (2.035); atlas better.
- `tool_calls`: -1.455 (-12.5%), -2.11 SE of the batch and -1.43 pooled SE; kept inside by 2 × pooled SE (2.035); atlas better.

`cache_read_tokens` is 89.1% of `total_tokens` in the baseline of this batch, so the two rows are one signal, not two.

The committed A/B report of this batch has 34 of these metrics; 0 differ from the recomputation.

| metric | baseline | atlas | effect | relative | tasks | SE, this batch | SE, pooled | largest A/A on the A/B scale | noise bound | bound / baseline | effect / SE | effect / pooled SE | class | tasks whose removal moves it across the bound |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| hidden_tests_pass | 1 | 1 | 0 | 0% | 11 | 0 | 0 | 0 | 0 | 0% | – | – | no difference | 0 of 11 |
| hidden_tests_passed | 43.77 | 43.77 | 0 | 0% | 11 | 0.06428 | 0.05567 | 0.06428 | 0.1286 | 0.3% | +0.00 | +0.00 | no difference | 0 of 11 |
| regressions | 0 | 0 | 0 | – | 11 | 0 | 0 | 0 | 0 | – | – | – | no difference | 0 of 11 |
| judge_correctness | 4.591 | 4.591 | 0 | 0% | 11 | 0 | 0.06333 | 0.1446 | 0.1446 | 3.1% | – | +0.00 | no difference | 0 of 11 |
| judge_completeness | 4.136 | 4.091 | -0.04545 | -1.1% | 11 | 0.05698 | 0.1097 | 0.1708 | 0.2194 | 5.3% | -0.80 | -0.41 | inside | 0 of 11 |
| must_change_coverage | 1 | 1 | 0 | 0% | 11 | 0 | 0 | 0 | 0 | 0% | – | – | no difference | 0 of 11 |
| must_consider_coverage | 0.4524 | 0.4789 | +0.02652 | +5.9% | 11 | 0.0518 | 0.04035 | 0.05893 | 0.1036 | 22.9% | +0.51 | +0.66 | inside | **1 of 11** |
| reuse_targets_referenced | 1 | 0.75 | -0.25 | -25% | 4 | 0 | 0.08839 | 0.1768 | 0.1768 | 17.7% | – | -2.83 | **outside** | **1 of 4** |
| duplication_introduced | 0 | 0 | 0 | – | 5 | 0 | 0 | 0 | 0 | – | – | – | no difference | 0 of 5 |
| lexical_duplicate_candidates | 0 | 0.4 | +0.4 | – | 5 | 0.2 | 0.1414 | 0.2828 | 0.4 | – | +2.00 | +2.83 | inside | 0 of 5 |
| judge_architectural_consistency | 4.5 | 4.636 | +0.1364 | +3% | 11 | 0.114 | 0.1213 | 0.1708 | 0.2425 | 5.4% | +1.20 | +1.12 | inside | 0 of 11 |
| judge_appropriate_reuse | 4.409 | 4.682 | +0.2727 | +6.2% | 11 | 0.1396 | 0.1156 | 0.2562 | 0.2791 | 6.3% | +1.95 | +2.36 | inside | **4 of 11** |
| judge_blast_radius_handling | 4 | 3.773 | -0.2273 | -5.7% | 11 | 0.114 | 0.09674 | 0.2562 | 0.2562 | 6.4% | -1.99 | -2.35 | inside | **1 of 11** |
| total_tokens | 207066 | 245716 | +38650 | +18.7% | 11 | 17388 | 17469 | 21638 | 34937 | 16.9% | +2.22 | +2.21 | **outside** | **6 of 11** |
| input_tokens | 19.36 | 18.64 | -0.7273 | -3.8% | 11 | 0.9181 | 0.9249 | 1.671 | 1.85 | 9.6% | -0.79 | -0.79 | inside | 0 of 11 |
| output_tokens | 4323 | 4094 | -229.4 | -5.3% | 11 | 210.9 | 228.2 | 340.2 | 456.5 | 10.6% | -1.09 | -1.01 | inside | 0 of 11 |
| cache_read_tokens | 184424 | 222074 | +37650 | +20.4% | 11 | 16233 | 16535 | 20401 | 33069 | 17.9% | +2.32 | +2.28 | **outside** | **6 of 11** |
| cache_creation_tokens | 18300 | 19530 | +1230 | +6.7% | 11 | 1617 | 1293 | 3585 | 3585 | 19.6% | +0.76 | +0.95 | inside | 0 of 11 |
| cost_usd | 0.1534 | 0.1635 | +0.01015 | +6.6% | 11 | 0.01033 | 0.009251 | 0.01928 | 0.02067 | 13.5% | +0.98 | +1.10 | inside | 0 of 11 |
| num_turns | 12.64 | 11.18 | -1.455 | -11.5% | 11 | 0.6894 | 1.017 | 0.9 | 2.035 | 16.1% | -2.11 | -1.43 | inside | 0 of 11 |
| tool_calls | 11.64 | 10.18 | -1.455 | -12.5% | 11 | 0.6894 | 1.017 | 0.9 | 2.035 | 17.5% | -2.11 | -1.43 | inside | 0 of 11 |
| search_calls | 3.318 | 2.864 | -0.4545 | -13.7% | 11 | 0.2802 | 0.3066 | 0.3214 | 0.6132 | 18.5% | -1.62 | -1.48 | inside | 0 of 11 |
| files_read | 4.273 | 3.545 | -0.7273 | -17% | 11 | 0.3462 | 0.3432 | 0.5143 | 0.6923 | 16.2% | -2.10 | -2.12 | **outside** | **7 of 11** |
| irrelevant_files_read | 0.6364 | 0.3636 | -0.2727 | -42.9% | 11 | 0.4454 | 0.3371 | 0.2727 | 0.8907 | 140% | -0.61 | -0.81 | inside | 0 of 11 |
| relevant_files_missed | 1.773 | 1.818 | +0.04545 | +2.6% | 11 | 0.1981 | 0.1575 | 0.2571 | 0.3963 | 22.4% | +0.23 | +0.29 | inside | 0 of 11 |
| bytes_read | 14675 | 13739 | -935.9 | -6.4% | 11 | 1042 | 1050 | 1932 | 2100 | 14.3% | -0.90 | -0.89 | inside | 0 of 11 |
| lines_read | 377.3 | 349.7 | -27.59 | -7.3% | 11 | 24.48 | 25.1 | 42.81 | 50.21 | 13.3% | -1.13 | -1.10 | inside | 0 of 11 |
| unparsed_bash_reads | 0.7273 | 0.9545 | +0.2273 | +31.3% | 11 | 0.2448 | 0.2273 | 0.45 | 0.4896 | 67.3% | +0.93 | +1.00 | inside | 0 of 11 |
| atlas_calls | 0 | 0.09091 | +0.09091 | – | 11 | 0 | 0.03214 | 0.06428 | 0.06428 | – | – | +2.83 | **outside** | **1 of 11** |
| atlas_bytes | 0 | 1018 | +1018 | – | 11 | 0.4091 | 266.9 | 211.1 | 533.8 | – | +2488.33 | +3.81 | **outside** | **1 of 11** |
| information_density | 0.5119 | 0.4987 | -0.01324 | -2.6% | 11 | 0.03733 | 0.04055 | 0.07732 | 0.08109 | 15.8% | -0.35 | -0.33 | inside | 0 of 11 |
| first_edit_ms | 15878 | 29632 | +3159 | +20.6% | 4 (baseline 5, atlas 5) | 3055 | 2199 | 7070 | 7070 | 46.2% | +1.03 | +1.44 | inside | 0 of 4 |
| session_ms | 136166 | 142513 | +6347 | +4.7% | 11 | 26048 | 22320 | 30325 | 52095 | 38.3% | +0.24 | +0.28 | inside | 0 of 11 |
| wall_ms | 137240 | 144389 | +7149 | +5.2% | 11 | 26116 | 22367 | 30373 | 52233 | 38.1% | +0.27 | +0.32 | inside | 0 of 11 |
| cache_weighted_tokens | 41337 | 46639 | +5302 | +12.8% | 11 | 3249 | 2910 | 6153 | 6499 | 15.7% | +1.63 | +1.82 | inside | **1 of 11** |

### Batch 2026-10-05-directed (preamble 2 (directed))

35 metrics with a value in both conditions: 11 outside the noise, 19 inside, 5 with no difference, 0 undetermined.

- `must_consider_coverage`: +0.1061 (+22.3%), +4.43 SE of the batch and +2.63 pooled SE, noise bound 0.08071; atlas better; **falls inside the bound when any one of 2 of its 11 tasks is left out**.
- `judge_appropriate_reuse`: +0.3636 (+8.2%), +3.81 SE of the batch and +3.14 pooled SE, noise bound 0.2562; atlas better; stays outside whichever task is left out.
- `total_tokens`: +54697 (+24.8%), +3.12 SE of the batch and +3.13 pooled SE, noise bound 35098; atlas worse; stays outside whichever task is left out.
- `input_tokens`: -1.909 (-9.4%), -2.05 SE of the batch and -2.06 pooled SE, noise bound 1.863; atlas better; **falls inside the bound when any one of 8 of its 11 tasks is left out**.
- `cache_read_tokens`: +51859 (+26.2%), +3.08 SE of the batch and +3.14 pooled SE, noise bound 33662; atlas worse; stays outside whichever task is left out.
- `cost_usd`: +0.02241 (+14.1%), +2.79 SE of the batch and +2.42 pooled SE, noise bound 0.01928; atlas worse; **falls inside the bound when any one of 2 of its 11 tasks is left out**.
- `bytes_read`: -3408 (-23%), -3.22 SE of the batch and -3.25 pooled SE, noise bound 2117; atlas better; **falls inside the bound when any one of 1 of its 11 tasks is left out**.
- `lines_read`: -88.95 (-23.1%), -3.46 SE of the batch and -3.54 pooled SE, noise bound 51.42; atlas better; stays outside whichever task is left out.
- `atlas_calls`: +0.9545 (–), +21.00 SE of the batch and +29.70 pooled SE, noise bound 0.09091; no preferred direction; stays outside whichever task is left out.
- `atlas_bytes`: +11198 (–), +29.67 SE of the batch and +41.95 pooled SE, noise bound 754.9; no preferred direction; stays outside whichever task is left out.
- `cache_weighted_tokens`: +8591 (+19.9%), +3.40 SE of the batch and +2.95 pooled SE, noise bound 6153; atlas worse; stays outside whichever task is left out.

Inside the bound although the effect exceeds 2 × SE of this batch:

- `lexical_duplicate_candidates`: +0.2 (–), – SE of the batch and +1.41 pooled SE; kept inside by 2 × pooled SE (0.2828); no preferred direction.
- `judge_blast_radius_handling`: -0.2273 (-5.6%), -2.75 SE of the batch and -2.35 pooled SE; kept inside by the largest A/A difference (0.2562); atlas worse.
- `cache_creation_tokens`: +2726 (+14.6%), +3.19 SE of the batch and +2.11 pooled SE; kept inside by the largest A/A difference (3585); atlas worse.
- `relevant_files_missed`: -0.2273 (-13.9%), -2.24 SE of the batch and -1.44 pooled SE; kept inside by 2 × pooled SE (0.3149); atlas better.

`cache_read_tokens` is 89.5% of `total_tokens` in the baseline of this batch, so the two rows are one signal, not two.

The committed A/B report of this batch has 34 of these metrics; 0 differ from the recomputation.

| metric | baseline | atlas | effect | relative | tasks | SE, this batch | SE, pooled | largest A/A on the A/B scale | noise bound | bound / baseline | effect / SE | effect / pooled SE | class | tasks whose removal moves it across the bound |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| hidden_tests_pass | 1 | 1 | 0 | 0% | 11 | 0 | 0 | 0 | 0 | 0% | – | – | no difference | 0 of 11 |
| hidden_tests_passed | 43.82 | 43.77 | -0.04545 | -0.1% | 11 | 0.04545 | 0.05567 | 0.06428 | 0.1113 | 0.3% | -1.00 | -0.82 | inside | 0 of 11 |
| regressions | 0 | 0 | 0 | – | 11 | 0 | 0 | 0 | 0 | – | – | – | no difference | 0 of 11 |
| judge_correctness | 4.545 | 4.591 | +0.04545 | +1% | 11 | 0.08257 | 0.06333 | 0.1446 | 0.1651 | 3.6% | +0.55 | +0.72 | inside | 0 of 11 |
| judge_completeness | 4.045 | 4.045 | 0 | 0% | 11 | 0.1348 | 0.1097 | 0.1708 | 0.2697 | 6.7% | +0.00 | +0.00 | no difference | 0 of 11 |
| must_change_coverage | 1 | 1 | 0 | 0% | 11 | 0 | 0 | 0 | 0 | 0% | – | – | no difference | 0 of 11 |
| must_consider_coverage | 0.4751 | 0.5812 | +0.1061 | +22.3% | 11 | 0.02396 | 0.04035 | 0.05893 | 0.08071 | 17% | +4.43 | +2.63 | **outside** | **2 of 11** |
| reuse_targets_referenced | 1 | 0.875 | -0.125 | -12.5% | 4 | 0.125 | 0.08839 | 0.1768 | 0.25 | 25% | -1.00 | -1.41 | inside | 0 of 4 |
| duplication_introduced | 0 | 0 | 0 | – | 5 | 0 | 0 | 0 | 0 | – | – | – | no difference | 0 of 5 |
| lexical_duplicate_candidates | 0 | 0.2 | +0.2 | – | 5 | 0 | 0.1414 | 0.2828 | 0.2828 | – | – | +1.41 | inside | 0 of 5 |
| judge_architectural_consistency | 4.636 | 4.591 | -0.04545 | -1% | 11 | 0.1261 | 0.1213 | 0.1708 | 0.2523 | 5.4% | -0.36 | -0.37 | inside | 0 of 11 |
| judge_appropriate_reuse | 4.409 | 4.773 | +0.3636 | +8.2% | 11 | 0.09535 | 0.1156 | 0.2562 | 0.2562 | 5.8% | +3.81 | +3.14 | **outside** | 0 of 11 |
| judge_blast_radius_handling | 4.091 | 3.864 | -0.2273 | -5.6% | 11 | 0.08257 | 0.09674 | 0.2562 | 0.2562 | 6.3% | -2.75 | -2.35 | inside | **3 of 11** |
| total_tokens | 220890 | 275587 | +54697 | +24.8% | 11 | 17549 | 17469 | 21638 | 35098 | 15.9% | +3.12 | +3.13 | **outside** | 0 of 11 |
| input_tokens | 20.27 | 18.36 | -1.909 | -9.4% | 11 | 0.9315 | 0.9249 | 1.671 | 1.863 | 9.2% | -2.05 | -2.06 | **outside** | **8 of 11** |
| output_tokens | 4526 | 4639 | +113.5 | +2.5% | 11 | 244.3 | 228.2 | 340.2 | 488.7 | 10.8% | +0.46 | +0.50 | inside | 0 of 11 |
| cache_read_tokens | 197714 | 249573 | +51859 | +26.2% | 11 | 16831 | 16535 | 20401 | 33662 | 17% | +3.08 | +3.14 | **outside** | 0 of 11 |
| cache_creation_tokens | 18630 | 21356 | +2726 | +14.6% | 11 | 853.3 | 1293 | 3585 | 3585 | 19.2% | +3.19 | +2.11 | inside | 0 of 11 |
| cost_usd | 0.1594 | 0.1818 | +0.02241 | +14.1% | 11 | 0.008025 | 0.009251 | 0.01928 | 0.01928 | 12.1% | +2.79 | +2.42 | **outside** | **2 of 11** |
| num_turns | 12.68 | 14.59 | +1.909 | +15.1% | 11 | 1.263 | 1.017 | 0.9 | 2.526 | 19.9% | +1.51 | +1.88 | inside | 0 of 11 |
| tool_calls | 11.68 | 13.59 | +1.909 | +16.3% | 11 | 1.263 | 1.017 | 0.9 | 2.526 | 21.6% | +1.51 | +1.88 | inside | 0 of 11 |
| search_calls | 3.455 | 2.955 | -0.5 | -14.5% | 11 | 0.3309 | 0.3066 | 0.3214 | 0.6618 | 19.2% | -1.51 | -1.63 | inside | 0 of 11 |
| files_read | 4.364 | 3.727 | -0.6364 | -14.6% | 11 | 0.3402 | 0.3432 | 0.5143 | 0.6863 | 15.7% | -1.87 | -1.85 | inside | **1 of 11** |
| irrelevant_files_read | 0.3636 | 0.2727 | -0.09091 | -25% | 11 | 0.1701 | 0.3371 | 0.2727 | 0.6742 | 185.4% | -0.53 | -0.27 | inside | 0 of 11 |
| relevant_files_missed | 1.636 | 1.409 | -0.2273 | -13.9% | 11 | 0.1016 | 0.1575 | 0.2571 | 0.3149 | 19.2% | -2.24 | -1.44 | inside | **1 of 11** |
| bytes_read | 14843 | 11435 | -3408 | -23% | 11 | 1059 | 1050 | 1932 | 2117 | 14.3% | -3.22 | -3.25 | **outside** | **1 of 11** |
| lines_read | 385.5 | 296.5 | -88.95 | -23.1% | 11 | 25.71 | 25.1 | 42.81 | 51.42 | 13.3% | -3.46 | -3.54 | **outside** | 0 of 11 |
| unparsed_bash_reads | 0.8636 | 0.6364 | -0.2273 | -26.3% | 11 | 0.2083 | 0.2273 | 0.45 | 0.4545 | 52.6% | -1.09 | -1.00 | inside | 0 of 11 |
| atlas_calls | 0 | 0.9545 | +0.9545 | – | 11 | 0.04545 | 0.03214 | 0.06428 | 0.09091 | – | +21.00 | +29.70 | **outside** | 0 of 11 |
| atlas_bytes | 0 | 11198 | +11198 | – | 11 | 377.5 | 266.9 | 211.1 | 754.9 | – | +29.67 | +41.95 | **outside** | 0 of 11 |
| information_density | 0.5164 | 0.5025 | -0.01386 | -2.7% | 11 | 0.04353 | 0.04055 | 0.07732 | 0.08705 | 16.9% | -0.32 | -0.34 | inside | 0 of 11 |
| first_edit_ms | 16019 | 14262 | -2163 | -13.5% | 5 (baseline 5, atlas 6) | 1213 | 1967 | 7070 | 7070 | 44.1% | -1.78 | -1.10 | inside | **1 of 5** |
| session_ms | 155121 | 144542 | -10579 | -6.8% | 11 | 17829 | 22320 | 30325 | 44640 | 28.8% | -0.59 | -0.47 | inside | 0 of 11 |
| wall_ms | 156674 | 146342 | -10333 | -6.6% | 11 | 17847 | 22367 | 30373 | 44734 | 28.6% | -0.58 | -0.46 | inside | 0 of 11 |
| cache_weighted_tokens | 43080 | 51671 | +8591 | +19.9% | 11 | 2525 | 2910 | 6153 | 6153 | 14.3% | +3.40 | +2.95 | **outside** | 0 of 11 |

## What the data supports

- **Two repetitions per cell.** Each task and condition has two runs, so each per-task variance rests on a single difference. The SE pools those differences over the tasks and both conditions; it is a rough scale, not a confidence interval, and no significance is claimed for any effect.
- **The bound is not a test.** About one pure-noise effect in twenty exceeds 2 × SE, and every batch classifies a few dozen metrics, many of them correlated (tokens, cost, turns and tool calls move together). The SE of one metric can differ by a factor of two between batches, which shows how uncertain the noise scale itself is. An `outside` label is a lead to replicate, not a result, and one that a single task can remove is weaker still.
- **Borderline labels.** A class is a threshold on a noisy scale. The lists above name every `outside` effect that one task can turn, and every `inside` effect that exceeds 2 × SE of its batch together with the term that keeps it inside.
- **Inside does not mean no effect.** It means the design cannot tell the effect from run-to-run variation. The column `bound / baseline` says how large an effect would have to be to leave the noise with this task set and two runs per cell; the SE shrinks with the square root of the number of tasks and of the repetitions.
- **Across batches is more than repetition.** Batches run at different times and may come from different harness commits (see the table above). The across-batch baseline difference therefore includes drift between batches (machine load, service latency) that a within-batch A/B comparison does not contain. It is listed as an A/A difference because the baseline sessions were configured identically.
- **Metrics that only one condition can move** (`atlas_calls`, `atlas_bytes`) differ by construction, so their class says nothing about noise, and `atlas_index_ms` has no baseline value at all.
- **Judged scores** come from one judge call per run on a 1 to 5 scale; runs whose judge call failed have no score and drop out of the pairs.

## A/A differences per metric

| metric | comparison | pairs | median \|diff\| | median relative | range | paired difference | relative | size on the A/B scale |
|---|---|---|---|---|---|---|---|---|
| hidden_tests_pass | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_pass | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_pass | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_pass | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_pass | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_passed | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to +1 | +0.09091 | +0.2% | 0.06428 |
| hidden_tests_passed | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | -1 to 0 | -0.09091 | -0.2% | 0.06428 |
| hidden_tests_passed | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| hidden_tests_passed | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to 0 | -0.09091 | -0.2% | 0.06428 |
| hidden_tests_passed | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | 0 to +0.5 | +0.04545 | +0.1% | 0.04545 |
| regressions | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| regressions | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| regressions | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| regressions | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| regressions | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| judge_correctness | baseline, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.1111 | +2.4% | 0.08541 |
| judge_correctness | atlas, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.1111 | +2.5% | 0.08541 |
| judge_correctness | baseline, second run − first run, batch 2026-10-05-directed | 10 | 0 | 0% | -1 to 0 | -0.2 | -4.3% | 0.1446 |
| judge_correctness | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to +1 | +0.09091 | +2% | 0.06573 |
| judge_correctness | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | -0.5 to 0 | -0.04545 | -1% | 0.04545 |
| judge_completeness | baseline, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.1111 | +2.7% | 0.08541 |
| judge_completeness | atlas, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.2222 | +5.7% | 0.1708 |
| judge_completeness | baseline, second run − first run, batch 2026-10-05-directed | 10 | 0 | 0% | -1 to +1 | +0.1 | +2.5% | 0.0723 |
| judge_completeness | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to +1 | -0.09091 | -2.2% | 0.06573 |
| judge_completeness | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | -0.5 to +0.5 | -0.09091 | -2.2% | 0.09091 |
| must_change_coverage | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| must_change_coverage | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| must_change_coverage | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| must_change_coverage | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| must_change_coverage | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| must_consider_coverage | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | -0.25 to 0 | -0.04545 | -9.6% | 0.03214 |
| must_consider_coverage | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | -0.3333 to +1 | +0.08333 | +19.1% | 0.05893 |
| must_consider_coverage | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to +0.25 | +0.04545 | +10% | 0.03214 |
| must_consider_coverage | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -0.25 to +0.25 | -0.01515 | -2.6% | 0.01071 |
| must_consider_coverage | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | -0.125 to +0.125 | +0.02273 | +5% | 0.02273 |
| reuse_targets_referenced | baseline, second run − first run, batch 2026-10-04-seed1 | 4 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| reuse_targets_referenced | atlas, second run − first run, batch 2026-10-04-seed1 | 4 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| reuse_targets_referenced | baseline, second run − first run, batch 2026-10-05-directed | 4 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| reuse_targets_referenced | atlas, second run − first run, batch 2026-10-05-directed | 4 | 0 | 0% | 0 to +1 | +0.25 | +33.3% | 0.1768 |
| reuse_targets_referenced | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 4 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| duplication_introduced | baseline, second run − first run, batch 2026-10-04-seed1 | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| duplication_introduced | atlas, second run − first run, batch 2026-10-04-seed1 | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| duplication_introduced | baseline, second run − first run, batch 2026-10-05-directed | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| duplication_introduced | atlas, second run − first run, batch 2026-10-05-directed | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| duplication_introduced | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| lexical_duplicate_candidates | baseline, second run − first run, batch 2026-10-04-seed1 | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| lexical_duplicate_candidates | atlas, second run − first run, batch 2026-10-04-seed1 | 5 | 0 | 100% | -2 to 0 | -0.4 | -66.7% | 0.2828 |
| lexical_duplicate_candidates | baseline, second run − first run, batch 2026-10-05-directed | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| lexical_duplicate_candidates | atlas, second run − first run, batch 2026-10-05-directed | 5 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| lexical_duplicate_candidates | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 5 | 0 | – | 0 to 0 | 0 | – | 0 |
| judge_architectural_consistency | baseline, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | -1 to +1 | +0.1111 | +2.6% | 0.08541 |
| judge_architectural_consistency | atlas, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.2222 | +4.9% | 0.1708 |
| judge_architectural_consistency | baseline, second run − first run, batch 2026-10-05-directed | 10 | 0 | 0% | -1 to 0 | -0.2 | -4.3% | 0.1446 |
| judge_architectural_consistency | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to +1 | -0.09091 | -2% | 0.06573 |
| judge_architectural_consistency | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0.5 | 10.5% | -1 to +1 | +0.1364 | +3% | 0.1364 |
| judge_appropriate_reuse | baseline, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | -2 to 0 | -0.3333 | -7.3% | 0.2562 |
| judge_appropriate_reuse | atlas, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.1111 | +2.4% | 0.08541 |
| judge_appropriate_reuse | baseline, second run − first run, batch 2026-10-05-directed | 10 | 0 | 0% | -1 to +1 | +0.1 | +2.3% | 0.0723 |
| judge_appropriate_reuse | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to 0 | -0.09091 | -1.9% | 0.06573 |
| judge_appropriate_reuse | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | -0.5 to +0.5 | 0 | 0% | 0 |
| judge_blast_radius_handling | baseline, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | -1 to 0 | -0.2222 | -5.4% | 0.1708 |
| judge_blast_radius_handling | atlas, second run − first run, batch 2026-10-04-seed1 | 9 | 0 | 0% | 0 to +1 | +0.3333 | +9.4% | 0.2562 |
| judge_blast_radius_handling | baseline, second run − first run, batch 2026-10-05-directed | 10 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| judge_blast_radius_handling | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to +1 | -0.09091 | -2.3% | 0.06573 |
| judge_blast_radius_handling | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | 0 to +0.5 | +0.09091 | +2.3% | 0.09091 |
| total_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 45115 | 24.8% | -63119 to +272019 | +30600 | +16% | 21638 |
| total_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 55449 | 25.6% | -87949 to +139901 | +28915 | +12.5% | 20446 |
| total_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 40333 | 18.9% | -148942 to +49842 | -29225 | -12.4% | 20665 |
| total_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 57542 | 13.3% | -208401 to +82177 | -16652 | -5.9% | 11775 |
| total_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 28709 | 8.9% | -78572 to +104386 | +13824 | +6.7% | 13824 |
| input_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 4 | 22.2% | -6 to +10 | +1.273 | +6.8% | 0.9 |
| input_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 4 | 20% | -4 to +6 | +2 | +11.3% | 1.414 |
| input_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 2 | 10.5% | -8 to +2 | -2.364 | -11% | 1.671 |
| input_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 2 | 10.5% | -10 to +4 | -0.7273 | -3.9% | 0.5143 |
| input_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 2 | 10% | -6 to +8 | +0.9091 | +4.7% | 0.9091 |
| output_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 552 | 22% | -1196 to +2392 | +481.2 | +11.8% | 340.2 |
| output_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 546 | 14% | -1466 to +1280 | +226 | +5.7% | 159.8 |
| output_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 786 | 13.7% | -1329 to +1505 | -80 | -1.8% | 56.57 |
| output_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 664 | 17% | -1954 to +3766 | +31.73 | +0.7% | 22.43 |
| output_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 566.5 | 10.7% | -796 to +937.5 | +202.3 | +4.7% | 202.3 |
| cache_read_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 43818 | 27.4% | -64833 to +255967 | +28851 | +17% | 20401 |
| cache_read_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 46811 | 24.7% | -83498 to +131569 | +23617 | +11.2% | 16700 |
| cache_read_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 38721 | 19.8% | -142268 to +44350 | -28635 | -13.5% | 20248 |
| cache_read_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 50622 | 12.6% | -203427 to +82357 | -16060 | -6.2% | 11356 |
| cache_read_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 30248 | 12% | -76706 to +105620 | +13290 | +7.2% | 13290 |
| cache_creation_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 1420 | 12.4% | -9506 to +13650 | +1267 | +7.2% | 895.7 |
| cache_creation_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 2981 | 19.2% | -5231 to +19920 | +5070 | +29.8% | 3585 |
| cache_creation_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 1796 | 9.8% | -6478 to +4662 | -507.5 | -2.7% | 358.9 |
| cache_creation_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 3872 | 17.7% | -8288 to +8897 | -622.9 | -2.9% | 440.5 |
| cache_creation_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 1968 | 10.7% | -4112 to +4768 | +330.5 | +1.8% | 330.5 |
| cost_usd | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0.02086 | 18% | -0.04894 to +0.1297 | +0.01565 | +10.8% | 0.01107 |
| cost_usd | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0.03982 | 27.6% | -0.04329 to +0.09663 | +0.02727 | +18.2% | 0.01928 |
| cost_usd | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0.01243 | 13.6% | -0.05626 to +0.03582 | -0.008562 | -5.2% | 0.006054 |
| cost_usd | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0.02067 | 10.6% | -0.0893 to +0.08715 | -0.005388 | -2.9% | 0.00381 |
| cost_usd | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0.01502 | 9.7% | -0.02757 to +0.02751 | +0.006005 | +3.9% | 0.006005 |
| num_turns | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 2 | 20% | -3 to +9 | +1.273 | +10.6% | 0.9 |
| num_turns | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 2 | 18.2% | -5 to +3 | +0.7273 | +6.7% | 0.5143 |
| num_turns | baseline, second run − first run, batch 2026-10-05-directed | 11 | 2 | 13.3% | -6 to +5 | -0.4545 | -3.5% | 0.3214 |
| num_turns | atlas, second run − first run, batch 2026-10-05-directed | 11 | 2 | 22.2% | -10 to +21 | -0.6364 | -4.3% | 0.45 |
| num_turns | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 2 | 15.9% | -3 to +2.5 | +0.04545 | +0.4% | 0.04545 |
| tool_calls | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 2 | 22.2% | -3 to +9 | +1.273 | +11.6% | 0.9 |
| tool_calls | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 2 | 20% | -5 to +3 | +0.7273 | +7.4% | 0.5143 |
| tool_calls | baseline, second run − first run, batch 2026-10-05-directed | 11 | 2 | 15.4% | -6 to +5 | -0.4545 | -3.8% | 0.3214 |
| tool_calls | atlas, second run − first run, batch 2026-10-05-directed | 11 | 2 | 24% | -10 to +21 | -0.6364 | -4.6% | 0.45 |
| tool_calls | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 2 | 16.9% | -3 to +2.5 | +0.04545 | +0.4% | 0.04545 |
| search_calls | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 1 | 50% | -3 to +2 | -0.09091 | -2.7% | 0.06428 |
| search_calls | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 1 | 28.6% | -1 to +2 | +0.09091 | +3.2% | 0.06428 |
| search_calls | baseline, second run − first run, batch 2026-10-05-directed | 11 | 2 | 40% | -3 to +3 | -0.3636 | -10% | 0.2571 |
| search_calls | atlas, second run − first run, batch 2026-10-05-directed | 11 | 1 | 40% | -2 to +1 | -0.4545 | -14.3% | 0.3214 |
| search_calls | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 1 | 22.2% | -2.5 to +2.5 | +0.1364 | +4.1% | 0.1364 |
| files_read | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 1 | 13.3% | -4 to +4 | 0 | 0% | 0 |
| files_read | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 1 | 18.2% | -1 to +4 | +0.7273 | +22.9% | 0.5143 |
| files_read | baseline, second run − first run, batch 2026-10-05-directed | 11 | 1 | 22.2% | -2 to +2 | -0.3636 | -8% | 0.2571 |
| files_read | atlas, second run − first run, batch 2026-10-05-directed | 11 | 1 | 28.6% | -1 to +4 | +0.5455 | +15.8% | 0.3857 |
| files_read | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0.5 | 15.4% | -2 to +1.5 | +0.09091 | +2.1% | 0.09091 |
| irrelevant_files_read | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 200% | -8 to +3 | -0.3636 | -44.4% | 0.2571 |
| irrelevant_files_read | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 200% | -4 to +1 | -0.3636 | -66.7% | 0.2571 |
| irrelevant_files_read | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 200% | -2 to +1 | -0.3636 | -66.7% | 0.2571 |
| irrelevant_files_read | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 200% | -1 to +2 | +0.1818 | +100% | 0.1286 |
| irrelevant_files_read | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0.5 | 200% | -4 to +1 | -0.2727 | -42.9% | 0.2727 |
| relevant_files_missed | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to +1 | +0.09091 | +5.3% | 0.06428 |
| relevant_files_missed | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | -4 to +1 | -0.3636 | -18.2% | 0.2571 |
| relevant_files_missed | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to 0 | -0.1818 | -10.5% | 0.1286 |
| relevant_files_missed | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | -1 to +1 | +0.09091 | +6.7% | 0.06428 |
| relevant_files_missed | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 0% | -0.5 to 0 | -0.1364 | -7.7% | 0.1364 |
| bytes_read | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 1839 | 22% | -4510 to +10709 | +1954 | +14.3% | 1382 |
| bytes_read | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 3105 | 23% | -3472 to +8058 | +2733 | +22.1% | 1932 |
| bytes_read | baseline, second run − first run, batch 2026-10-05-directed | 11 | 1419 | 8% | -9042 to +2378 | -1140 | -7.4% | 806.3 |
| bytes_read | atlas, second run − first run, batch 2026-10-05-directed | 11 | 1902 | 32.6% | -8972 to +15424 | +517.8 | +4.6% | 366.2 |
| bytes_read | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 1271 | 7.5% | -13782 to +6286 | +168.5 | +1.1% | 168.5 |
| lines_read | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 44 | 18.8% | -159 to +269 | +36.45 | +10.2% | 25.78 |
| lines_read | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 70 | 22.1% | -80 to +169 | +60.55 | +19% | 42.81 |
| lines_read | baseline, second run − first run, batch 2026-10-05-directed | 11 | 45 | 11% | -159 to +55 | -34.55 | -8.6% | 24.43 |
| lines_read | atlas, second run − first run, batch 2026-10-05-directed | 11 | 65 | 29.7% | -214 to +390 | +9.182 | +3.1% | 6.493 |
| lines_read | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 37 | 11% | -311.5 to +111 | +8.136 | +2.2% | 8.136 |
| unparsed_bash_reads | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 100% | -2 to +2 | +0.1818 | +28.6% | 0.1286 |
| unparsed_bash_reads | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 1 | 100% | -1 to +3 | +0.6364 | +100% | 0.45 |
| unparsed_bash_reads | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | 100% | -2 to +1 | -0.4545 | -41.7% | 0.3214 |
| unparsed_bash_reads | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 100% | -2 to +2 | -0.1818 | -25% | 0.1286 |
| unparsed_bash_reads | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | 120% | -1.5 to +2 | +0.1364 | +18.7% | 0.1364 |
| atlas_calls | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| atlas_calls | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0% | 0 to 0 | 0 | 0% | 0 |
| atlas_calls | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| atlas_calls | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0 | 0% | 0 to +1 | +0.09091 | +10% | 0.06428 |
| atlas_calls | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| atlas_bytes | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| atlas_bytes | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0 | 0.1% | 0 to +9 | +0.8182 | +0.1% | 0.5785 |
| atlas_bytes | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| atlas_bytes | atlas, second run − first run, batch 2026-10-05-directed | 11 | 281 | 2.2% | -6729 to +4294 | -298.5 | -2.6% | 211.1 |
| atlas_bytes | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0 | – | 0 to 0 | 0 | – | 0 |
| information_density | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 0.1026 | 16.5% | -0.2461 to +0.3795 | +0.01486 | +2.9% | 0.01051 |
| information_density | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 0.1286 | 22.3% | -0.1286 to +0.3667 | +0.1094 | +24.6% | 0.07732 |
| information_density | baseline, second run − first run, batch 2026-10-05-directed | 11 | 0.1427 | 25.8% | -0.3949 to +0.5728 | +0.006537 | +1.3% | 0.004622 |
| information_density | atlas, second run − first run, batch 2026-10-05-directed | 11 | 0.0398 | 6.4% | -0.1528 to +0.4428 | +0.01339 | +2.7% | 0.009468 |
| information_density | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 0.07461 | 15.1% | -0.2626 to +0.1985 | +0.004441 | +0.9% | 0.004441 |
| first_edit_ms | baseline, second run − first run, batch 2026-10-04-seed1 | 5 | 2436 | 13.5% | -6661 to +4179 | -648 | -4% | 522.4 |
| first_edit_ms | atlas, second run − first run, batch 2026-10-04-seed1 | 2 | 8770 | 33.9% | +833 to +16706 | +8770 | +58.1% | 7070 |
| first_edit_ms | baseline, second run − first run, batch 2026-10-05-directed | 3 | 1059 | 10.9% | -7123 to -750 | -2977 | -18.4% | 2375 |
| first_edit_ms | atlas, second run − first run, batch 2026-10-05-directed | 5 | 1290 | 15.5% | -2716 to +5181 | +1003 | +7.2% | 800.1 |
| first_edit_ms | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 4 | 1782 | 16.2% | -2868 to +4950 | +592 | +3.9% | 592 |
| session_ms | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 25005 | 25.5% | -185085 to +356546 | +23401 | +18.8% | 16547 |
| session_ms | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 16598 | 31.2% | -152053 to +277739 | +19892 | +15% | 14066 |
| session_ms | baseline, second run − first run, batch 2026-10-05-directed | 11 | 21218 | 30.1% | -244670 to +26274 | -42886 | -24.3% | 30325 |
| session_ms | atlas, second run − first run, batch 2026-10-05-directed | 11 | 11221 | 10.5% | -196031 to +15437 | -31415 | -19.6% | 22214 |
| session_ms | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 20386 | 26.1% | -73726 to +265056 | +18954 | +13.9% | 18954 |
| wall_ms | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 24878 | 25.1% | -185118 to +356689 | +23410 | +18.6% | 16553 |
| wall_ms | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 16586 | 29.9% | -151591 to +278218 | +20423 | +15.2% | 14441 |
| wall_ms | baseline, second run − first run, batch 2026-10-05-directed | 11 | 21249 | 28% | -239491 to +26376 | -42954 | -24.1% | 30373 |
| wall_ms | atlas, second run − first run, batch 2026-10-05-directed | 11 | 11225 | 10.4% | -195974 to +15526 | -31887 | -19.6% | 22547 |
| wall_ms | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 20472 | 23.8% | -71163 to +267708 | +19434 | +14.2% | 19434 |
| atlas_index_ms | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 45 | 9.1% | -94 to +159 | +10.64 | +1.6% | 7.521 |
| atlas_index_ms | atlas, second run − first run, batch 2026-10-05-directed | 11 | 28 | 7.2% | -28 to +266 | +61.36 | +9.6% | 43.39 |
| cache_weighted_tokens | baseline, second run − first run, batch 2026-10-04-seed1 | 11 | 5234 | 18.1% | -11361 to +42669 | +4470 | +11.4% | 3161 |
| cache_weighted_tokens | atlas, second run − first run, batch 2026-10-04-seed1 | 11 | 12080 | 27.7% | -13257 to +29585 | +8701 | +20.6% | 6153 |
| cache_weighted_tokens | baseline, second run − first run, batch 2026-10-05-directed | 11 | 4639 | 14.8% | -22332 to +10262 | -3500 | -7.8% | 2475 |
| cache_weighted_tokens | atlas, second run − first run, batch 2026-10-05-directed | 11 | 6829 | 12.7% | -28665 to +18074 | -2385 | -4.5% | 1687 |
| cache_weighted_tokens | baseline, batch 2026-10-05-directed − batch 2026-10-04-seed1 (per-task means) | 11 | 3920 | 8.2% | -9007 to +9258 | +1743 | +4.2% | 1743 |

