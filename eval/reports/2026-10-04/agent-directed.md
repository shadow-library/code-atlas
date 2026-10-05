# Agent A/B report

## Environment

| key | value |
|---|---|
| date | 2026-10-04 |
| seed | 1 |
| claude CLI | 2.1.289 (Claude Code) |
| session model | claude-sonnet-5-5 |
| judge model | claude-sonnet-5-5 |
| claude config | user |
| eval repository commit | 3c54d92821abe63e4e25508b9fa7d3e6c4ae7d84 |
| code-atlas binary version (build commit) | code-atlas 0.1.0 (commit 6a2854f4df88e1e5705f8fae091824ec290e3cee) |
| code-atlas binary blake3 | ddbc1afe75ba566247744839faffbe9c145dfd64cc89b6734af90ad3f8e133b8 |
| preamble version | 2 (directed) |
| repo override | none |
| spent (reported) | $9.82 |

### Isolation probe

- tools: Bash, Edit, Glob, Grep, Read, Write
- MCP servers: none
- skills: none; slash commands: none
- plugins: cc-plugin-agents-md, cc-plugin-telemetry, cc-plugin-plugin-authoring
- user CLAUDE.md: NotSeen; project CLAUDE.md: NotSeen
- models reported: claude-sonnet-5-5

## Aggregates (mean of per-task means over 11 paired tasks)

Only tasks with scored, uncontaminated runs in both conditions are aggregated (0 contaminated runs excluded). Times (`first_edit_ms`, `session_ms`, `wall_ms`) exclude Code Atlas indexing, which is reported separately as `atlas_index_ms`.

### Correctness

| metric | baseline (11 tasks) | atlas (11 tasks) |
|---|---|---|
| hidden_tests_pass | 1 (1–1, tasks=11) | 1 (1–1, tasks=11) |
| hidden_tests_passed | 43.818 (3–336, tasks=11) | 43.773 (3–336, tasks=11) |
| regressions | 0 (0–0, tasks=11) | 0 (0–0, tasks=11) |
| judge_correctness | 4.545 (4–5, tasks=11) | 4.591 (4–5, tasks=11) |
| judge_completeness | 4.045 (3.5–4.5, tasks=11) | 4.045 (3–5, tasks=11) |

### Reuse and blast radius

| metric | baseline (11 tasks) | atlas (11 tasks) |
|---|---|---|
| must_change_coverage | 1 (1–1, tasks=11) | 1 (1–1, tasks=11) |
| must_consider_coverage | 0.475 (0–1, tasks=11) | 0.581 (0–1, tasks=11) |
| reuse_targets_referenced | 1 (1–1, tasks=4 of 11) | 0.875 (0.5–1, tasks=4 of 11) |
| duplication_introduced | 0 (0–0, tasks=5 of 11) | 0 (0–0, tasks=5 of 11) |
| lexical_duplicate_candidates | 0 (0–0, tasks=5 of 11) | 0.2 (0–1, tasks=5 of 11) |
| judge_architectural_consistency | 4.636 (4–5, tasks=11) | 4.591 (4–5, tasks=11) |
| judge_appropriate_reuse | 4.409 (3–5, tasks=11) | 4.773 (3.5–5, tasks=11) |
| judge_blast_radius_handling | 4.091 (4–5, tasks=11) | 3.864 (2.5–4.5, tasks=11) |

### Tokens and context

| metric | baseline (11 tasks) | atlas (11 tasks) |
|---|---|---|
| total_tokens | 220890.364 (36543.5–330611, tasks=11) | 275587.045 (126882–433830, tasks=11) |
| input_tokens | 20.273 (9–33, tasks=11) | 18.364 (14–30, tasks=11) |
| output_tokens | 4525.545 (1529–6614.5, tasks=11) | 4639.045 (1758.5–7037, tasks=11) |
| cache_read_tokens | 197714.045 (31466.5–300513, tasks=11) | 249573.455 (116757.5–401870, tasks=11) |
| cache_creation_tokens | 18630.5 (3539–31755.5, tasks=11) | 21356.182 (8352–28135, tasks=11) |
| cost_usd | 0.159 (0.036–0.233, tasks=11) | 0.182 (0.074–0.241, tasks=11) |
| num_turns | 12.682 (7.5–17, tasks=11) | 14.591 (8.5–23.5, tasks=11) |
| tool_calls | 11.682 (6.5–16, tasks=11) | 13.591 (7.5–22.5, tasks=11) |
| search_calls | 3.455 (1.5–7, tasks=11) | 2.955 (1.5–5, tasks=11) |
| files_read | 4.364 (2–8, tasks=11) | 3.727 (1.5–6.5, tasks=11) |
| irrelevant_files_read | 0.364 (0–1, tasks=11) | 0.273 (0–1, tasks=11) |
| relevant_files_missed | 1.636 (0–4, tasks=11) | 1.409 (0–4, tasks=11) |
| bytes_read | 14843.318 (1573–30614, tasks=11) | 11435.273 (456.5–18049, tasks=11) |
| lines_read | 385.455 (55–710.5, tasks=11) | 296.5 (22–452, tasks=11) |
| unparsed_bash_reads | 0.864 (0–2, tasks=11) | 0.636 (0–2, tasks=11) |
| atlas_calls | 0 (0–0, tasks=11) | 0.955 (0.5–1, tasks=11) |
| atlas_bytes | 0 (0–0, tasks=11) | 11198 (588.5–19705, tasks=11) |
| information_density | 0.516 (0.106–0.669, tasks=11) | 0.502 (0.168–0.745, tasks=11) |
| first_edit_ms | 16018.8 (6885–21694.5, tasks=5 of 11) | 14262.25 (7799–17538, tasks=6 of 11) |
| session_ms | 155120.545 (12264–326848, tasks=11) | 144542 (15702–336345.5, tasks=11) |
| wall_ms | 156674.364 (13430.5–330509.5, tasks=11) | 146341.727 (17338–337972.5, tasks=11) |
| atlas_index_ms | – | 672.227 (65.5–938, tasks=11) |

## Per task

### t00-smoke

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 3 (3–3, n=2) | 3 (3–3, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 4 (4–4, n=2) | 3 (3–3, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.5 (0.5–0.5, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 2.5 (2–3, n=2) |
| total_tokens | 36543.5 (31605–41482, n=2) | 126882 (126111–127653, n=2) |
| input_tokens | 9 (8–10, n=2) | 14 (14–14, n=2) |
| output_tokens | 1529 (1408–1650, n=2) | 1758.5 (1565–1952, n=2) |
| cache_read_tokens | 31466.5 (26961–35972, n=2) | 116757.5 (116413–117102, n=2) |
| cache_creation_tokens | 3539 (3228–3850, n=2) | 8352 (8119–8585, n=2) |
| cost_usd | 0.036 (0.032–0.039, n=2) | 0.074 (0.071–0.077, n=2) |
| num_turns | 7.5 (7–8, n=2) | 8.5 (8–9, n=2) |
| tool_calls | 6.5 (6–7, n=2) | 7.5 (7–8, n=2) |
| search_calls | 2.5 (2–3, n=2) | 2.5 (2–3, n=2) |
| files_read | 2 (2–2, n=2) | 1.5 (1–2, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 0 (0–0, n=2) | 0 (0–0, n=2) |
| bytes_read | 1573 (1573–1573, n=2) | 456.5 (334–579, n=2) |
| lines_read | 55 (55–55, n=2) | 22 (17–27, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 11205 (11205–11205, n=2) |
| information_density | 0.605 (0.5–0.71, n=2) | 0.168 (0.168–0.169, n=2) |
| first_edit_ms | 6885 (6510–7260, n=2) | 7799 (7154–8444, n=2) |
| session_ms | 12264 (11696–12832, n=2) | 15702 (14610–16794, n=2) |
| wall_ms | 13430.5 (12946–13915, n=2) | 17338 (16263–18413, n=2) |
| atlas_index_ms | – | 65.5 (61–70, n=2) |

### t01-cwd-relative-targets

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 4 (4–4, n=2) | 4 (4–4, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4.5 (4–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0 (0–0, n=2) | 0.125 (0–0.25, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 226802.5 (221578–232027, n=2) | 297342.5 (193142–401543, n=2) |
| input_tokens | 21 (20–22, n=2) | 17 (12–22, n=2) |
| output_tokens | 4324 (3931–4717, n=2) | 3987 (3449–4525, n=2) |
| cache_read_tokens | 206685.5 (200171–213200, n=2) | 271404.5 (169691–373118, n=2) |
| cache_creation_tokens | 15772 (14874–16670, n=2) | 21934 (19990–23878, n=2) |
| cost_usd | 0.148 (0.141–0.154, n=2) | 0.182 (0.148–0.215, n=2) |
| num_turns | 10.5 (10–11, n=2) | 10 (7–13, n=2) |
| tool_calls | 9.5 (9–10, n=2) | 9 (6–12, n=2) |
| search_calls | 3 (2–4, n=2) | 1.5 (1–2, n=2) |
| files_read | 2 (2–2, n=2) | 2 (2–2, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 2.5 (2–3, n=2) |
| bytes_read | 13909 (12165–15653, n=2) | 12360.5 (11895–12826, n=2) |
| lines_read | 355 (310–400, n=2) | 305.5 (299–312, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 19705 (18916–20494, n=2) |
| information_density | 0.601 (0.495–0.708, n=2) | 0.453 (0.408–0.498, n=2) |
| session_ms | 140890 (78089–203691, n=2) | 134366 (65889–202843, n=2) |
| wall_ms | 144529.5 (79033–210026, n=2) | 138390.5 (67358–209423, n=2) |
| atlas_index_ms | – | 855.5 (849–862, n=2) |

### t02-typed-degraded-cause

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 6 (6–6, n=2) | 6 (6–6, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.143 (0.143–0.143, n=2) | 0.143 (0.143–0.143, n=2) |
| judge_architectural_consistency | 4 (4–4, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 4 (4–4, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 218022.5 (161638–274407, n=2) | 312933.5 (271845–354022, n=2) |
| input_tokens | 18 (14–22, n=2) | 17 (16–18, n=2) |
| output_tokens | 5408.5 (4744–6073, n=2) | 7037 (5154–8920, n=2) |
| cache_read_tokens | 193400 (138268–248532, n=2) | 278882 (244126–313638, n=2) |
| cache_creation_tokens | 19196 (18612–19780, n=2) | 26997.5 (22549–31446, n=2) |
| cost_usd | 0.17 (0.15–0.19, n=2) | 0.234 (0.191–0.278, n=2) |
| num_turns | 12 (9–15, n=2) | 23.5 (13–34, n=2) |
| tool_calls | 11 (8–14, n=2) | 22.5 (12–33, n=2) |
| search_calls | 3 (2–4, n=2) | 2.5 (2–3, n=2) |
| files_read | 6 (5–7, n=2) | 5.5 (4–7, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 4 (4–4, n=2) | 4 (4–4, n=2) |
| bytes_read | 15179.5 (14771–15588, n=2) | 12612.5 (10557–14668, n=2) |
| lines_read | 353.5 (312–395, n=2) | 323 (275–371, n=2) |
| unparsed_bash_reads | 1 (1–1, n=2) | 0.5 (0–1, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 14276.5 (14274–14279, n=2) |
| information_density | 0.555 (0.522–0.588, n=2) | 0.659 (0.64–0.679, n=2) |
| first_edit_ms | 16447 (n=1 of 2) | 13123 (n=1 of 2) |
| session_ms | 264647.5 (202058–327237, n=2) | 336345.5 (330735–341956, n=2) |
| wall_ms | 265744 (203137–328351, n=2) | 337972.5 (332360–343585, n=2) |
| atlas_index_ms | – | 921 (809–1033, n=2) |

### t03-nested-router-links

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 5 (5–5, n=2) | 5 (5–5, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (n=1 of 2) | 5 (5–5, n=2) |
| judge_completeness | 4 (n=1 of 2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| judge_architectural_consistency | 5 (n=1 of 2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 5 (n=1 of 2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (n=1 of 2) | 4 (4–4, n=2) |
| total_tokens | 248547 (241949–255145, n=2) | 365564 (351342–379786, n=2) |
| input_tokens | 22 (22–22, n=2) | 21 (20–22, n=2) |
| output_tokens | 3812.5 (3375–4250, n=2) | 4062 (3971–4153, n=2) |
| cache_read_tokens | 225963 (218704–233222, n=2) | 335185.5 (323490–346881, n=2) |
| cache_creation_tokens | 18749.5 (18526–18973, n=2) | 26295.5 (23859–28732, n=2) |
| cost_usd | 0.158 (0.155–0.162, n=2) | 0.213 (0.2–0.226, n=2) |
| num_turns | 17 (17–17, n=2) | 16 (15–17, n=2) |
| tool_calls | 16 (16–16, n=2) | 15 (14–16, n=2) |
| search_calls | 7 (7–7, n=2) | 5 (4–6, n=2) |
| files_read | 3.5 (3–4, n=2) | 3 (3–3, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 1 (1–1, n=2) | 1 (1–1, n=2) |
| bytes_read | 17798.5 (17089–18508, n=2) | 18049 (14359–21739, n=2) |
| lines_read | 428.5 (406–451, n=2) | 418.5 (316–521, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 16217.5 (12853–19582, n=2) |
| information_density | 0.575 (0.504–0.647, n=2) | 0.628 (0.608–0.648, n=2) |
| first_edit_ms | 15571.5 (12010–19133, n=2) | 15206.5 (12616–17797, n=2) |
| session_ms | 325226.5 (314754–335699, n=2) | 315944.5 (308226–323663, n=2) |
| wall_ms | 326358 (315909–336807, n=2) | 317600 (309837–325363, n=2) |
| atlas_index_ms | – | 874 (804–944, n=2) |

### t04-detector-import-gates

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 7 (7–7, n=2) | 7 (7–7, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0 (0–0, n=2) | 0.333 (0.333–0.333, n=2) |
| judge_architectural_consistency | 4 (4–4, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 233872 (231665–236079, n=2) | 255327 (215876–294778, n=2) |
| input_tokens | 20 (20–20, n=2) | 14 (12–16, n=2) |
| output_tokens | 4879 (4768–4990, n=2) | 6184.5 (5901–6468, n=2) |
| cache_read_tokens | 212404.5 (210508–214301, n=2) | 220993.5 (179815–262172, n=2) |
| cache_creation_tokens | 16568.5 (16369–16768, n=2) | 28135 (26689–29581, n=2) |
| cost_usd | 0.158 (0.155–0.16, n=2) | 0.219 (0.218–0.219, n=2) |
| num_turns | 11 (11–11, n=2) | 17 (12–22, n=2) |
| tool_calls | 10 (10–10, n=2) | 16 (11–21, n=2) |
| search_calls | 2.5 (2–3, n=2) | 2 (1–3, n=2) |
| files_read | 5 (5–5, n=2) | 3.5 (3–4, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 2 (2–2, n=2) |
| bytes_read | 15046 (14439–15653, n=2) | 15329 (14917–15741, n=2) |
| lines_read | 409.5 (387–432, n=2) | 435 (430–440, n=2) |
| unparsed_bash_reads | 2 (1–3, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 16837.5 (16126–17549, n=2) |
| information_density | 0.669 (0.628–0.71, n=2) | 0.497 (0.488–0.506, n=2) |
| first_edit_ms | – | 16294 (16066–16522, n=2) |
| session_ms | 326848 (204513–449183, n=2) | 61829.5 (60232–63427, n=2) |
| wall_ms | 330509.5 (210764–450255, n=2) | 63372.5 (61757–64988, n=2) |
| atlas_index_ms | – | 938 (805–1071, n=2) |

### t05-mcp-dependency-tools

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 28 (28–28, n=2) | 27.5 (27–28, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.5 (0.5–0.5, n=2) | 0.75 (0.75–0.75, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 4.5 (4–5, n=2) | 4 (4–4, n=2) |
| judge_appropriate_reuse | 3 (3–3, n=2) | 3.5 (3–4, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 3.5 (3–4, n=2) |
| total_tokens | 307705.5 (287539–327872, n=2) | 348206 (251596–444816, n=2) |
| input_tokens | 19 (18–20, n=2) | 20 (16–24, n=2) |
| output_tokens | 5206.5 (4970–5443, n=2) | 4851 (3874–5828, n=2) |
| cache_read_tokens | 270724.5 (251364–290085, n=2) | 319161 (227676–410646, n=2) |
| cache_creation_tokens | 31755.5 (30714–32797, n=2) | 24174 (20030–28318, n=2) |
| cost_usd | 0.233 (0.228–0.239, n=2) | 0.209 (0.164–0.254, n=2) |
| num_turns | 17 (16–18, n=2) | 19 (15–23, n=2) |
| tool_calls | 16 (15–17, n=2) | 18 (14–22, n=2) |
| search_calls | 5 (4–6, n=2) | 3.5 (3–4, n=2) |
| files_read | 4.5 (4–5, n=2) | 4.5 (4–5, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 2 (2–2, n=2) |
| bytes_read | 30614 (29425–31803, n=2) | 13655 (9169–18141, n=2) |
| lines_read | 710.5 (683–738, n=2) | 289 (182–396, n=2) |
| unparsed_bash_reads | 0.5 (0–1, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 12893.5 (12886–12901, n=2) |
| information_density | 0.548 (0.493–0.602, n=2) | 0.615 (0.595–0.634, n=2) |
| first_edit_ms | 21694.5 (21165–22224, n=2) | 17538 (16180–18896, n=2) |
| session_ms | 84799 (75937–93661, n=2) | 176975.5 (78960–274991, n=2) |
| wall_ms | 85853 (76963–94743, n=2) | 178538 (80551–276525, n=2) |
| atlas_index_ms | – | 801 (787–815, n=2) |

### t06-repo-file-size-limit

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 3 (3–3, n=2) | 3 (3–3, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 3.5 (3–4, n=2) | 3.5 (3–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.625 (0.5–0.75, n=2) | 0.75 (0.75–0.75, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 0.5 (0–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 303691 (229220–378162, n=2) | 334602.5 (301759–367446, n=2) |
| input_tokens | 24 (20–28, n=2) | 20 (18–22, n=2) |
| output_tokens | 4552 (4458–4646, n=2) | 4355.5 (4020–4691, n=2) |
| cache_read_tokens | 278842 (207708–349976, n=2) | 307641 (275260–340022, n=2) |
| cache_creation_tokens | 20273 (17034–23512, n=2) | 22586 (22461–22711, n=2) |
| cost_usd | 0.182 (0.154–0.211, n=2) | 0.195 (0.185–0.206, n=2) |
| num_turns | 13.5 (11–16, n=2) | 14 (14–14, n=2) |
| tool_calls | 12.5 (10–15, n=2) | 13 (13–13, n=2) |
| search_calls | 5 (4–6, n=2) | 4 (4–4, n=2) |
| files_read | 3 (2–4, n=2) | 3.5 (3–4, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 1.5 (1–2, n=2) | 1 (1–1, n=2) |
| bytes_read | 14697.5 (14540–14855, n=2) | 9922.5 (9703–10142, n=2) |
| lines_read | 400.5 (387–414, n=2) | 280 (270–290, n=2) |
| unparsed_bash_reads | 0.5 (0–1, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 12922.5 (12782–13063, n=2) |
| information_density | 0.528 (0.429–0.627, n=2) | 0.541 (0.525–0.557, n=2) |
| session_ms | 290623 (277486–303760, n=2) | 323699 (306769–340629, n=2) |
| wall_ms | 291647 (278459–304835, n=2) | 325317.5 (308393–342242, n=2) |
| atlas_index_ms | – | 839 (809–869, n=2) |

### t07-static-precompressed-quality

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 8 (8–8, n=2) | 8 (8–8, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4.5 (4–5, n=2) | 4.5 (4–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4.5 (4–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.333 (0.333–0.333, n=2) | 0.417 (0.333–0.5, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 1 (1–1, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 119886.5 (112325–127448, n=2) | 159584 (158414–160754, n=2) |
| input_tokens | 16 (16–16, n=2) | 14 (14–14, n=2) |
| output_tokens | 3605.5 (2853–4358, n=2) | 4263.5 (3902–4625, n=2) |
| cache_read_tokens | 102306 (96922–107690, n=2) | 139093 (139078–139108, n=2) |
| cache_creation_tokens | 13959 (12534–15384, n=2) | 16213.5 (15420–17007, n=2) |
| cost_usd | 0.112 (0.098–0.127, n=2) | 0.135 (0.129–0.142, n=2) |
| num_turns | 10.5 (8–13, n=2) | 13.5 (12–15, n=2) |
| tool_calls | 9.5 (7–12, n=2) | 12.5 (11–14, n=2) |
| search_calls | 1.5 (0–3, n=2) | 2 (1–3, n=2) |
| files_read | 3.5 (3–4, n=2) | 2.5 (2–3, n=2) |
| irrelevant_files_read | 1 (0–2, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 2 (2–2, n=2) | 1.5 (1–2, n=2) |
| bytes_read | 18149.5 (17072–19227, n=2) | 11839 (10888–12790, n=2) |
| lines_read | 586.5 (582–591, n=2) | 354.5 (322–387, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 7912.5 (7521–8304, n=2) |
| information_density | 0.567 (0.28–0.853, n=2) | 0.745 (0.676–0.815, n=2) |
| first_edit_ms | 19496 (n=1 of 2) | 15613 (15211–16015, n=2) |
| session_ms | 34838.5 (29603–40074, n=2) | 39175.5 (38456–39895, n=2) |
| wall_ms | 36015 (30971–41059, n=2) | 40709.5 (40007–41412, n=2) |
| atlas_index_ms | – | 520 (514–526, n=2) |

### t08-cookie-samesite-default

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 336 (336–336, n=2) | 336 (336–336, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4.5 (4–5, n=2) | 3.5 (3–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.125 (0–0.25, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 4.5 (4–5, n=2) | 4 (4–4, n=2) |
| judge_appropriate_reuse | 4 (4–4, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| total_tokens | 330611 (299310–361912, n=2) | 433830 (405059–462601, n=2) |
| input_tokens | 33 (30–36, n=2) | 30 (30–30, n=2) |
| output_tokens | 6614.5 (6167–7062, n=2) | 5518.5 (5516–5521, n=2) |
| cache_read_tokens | 300513 (270807–330219, n=2) | 401870 (376559–427181, n=2) |
| cache_creation_tokens | 23450.5 (22306–24595, n=2) | 26411.5 (22954–29869, n=2) |
| cost_usd | 0.22 (0.205–0.235, n=2) | 0.241 (0.222–0.26, n=2) |
| num_turns | 17 (16–18, n=2) | 16 (16–16, n=2) |
| tool_calls | 16 (15–17, n=2) | 15 (15–15, n=2) |
| search_calls | 3 (3–3, n=2) | 4 (4–4, n=2) |
| files_read | 6.5 (6–7, n=2) | 5.5 (4–7, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 1 (0–2, n=2) |
| relevant_files_missed | 0.5 (0–1, n=2) | 1 (1–1, n=2) |
| bytes_read | 14761 (13029–16493, n=2) | 15920 (8208–23632, n=2) |
| lines_read | 402.5 (372–433, n=2) | 452 (257–647, n=2) |
| unparsed_bash_reads | 2 (2–2, n=2) | 2 (1–3, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 8472.5 (8381–8564, n=2) |
| information_density | 0.385 (0.364–0.407, n=2) | 0.458 (0.42–0.496, n=2) |
| session_ms | 118348.5 (98520–138177, n=2) | 99548.5 (93227–105870, n=2) |
| wall_ms | 119457 (99524–139390, n=2) | 101022 (94710–107334, n=2) |
| atlas_index_ms | – | 508.5 (478–539, n=2) |

### t09-lambda-text-content-types

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 76 (76–76, n=2) | 76 (76–76, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 196769.5 (171321–222218, n=2) | 176510 (164959–188061, n=2) |
| input_tokens | 21 (18–24, n=2) | 16 (16–16, n=2) |
| output_tokens | 3794.5 (3612–3977, n=2) | 2946 (2614–3278, n=2) |
| cache_read_tokens | 173514.5 (147415–199614, n=2) | 157833 (147886–167780, n=2) |
| cache_creation_tokens | 19439.5 (18968–19911, n=2) | 15715 (13779–17651, n=2) |
| cost_usd | 0.15 (0.149–0.152, n=2) | 0.124 (0.118–0.13, n=2) |
| num_turns | 11.5 (10–13, n=2) | 9 (8–10, n=2) |
| tool_calls | 10.5 (9–12, n=2) | 8 (7–9, n=2) |
| search_calls | 3.5 (2–5, n=2) | 3 (3–3, n=2) |
| files_read | 4 (3–5, n=2) | 3 (1–5, n=2) |
| irrelevant_files_read | 1 (1–1, n=2) | 1 (1–1, n=2) |
| relevant_files_missed | 0 (0–0, n=2) | 0 (0–0, n=2) |
| bytes_read | 4085.5 (4008–4163, n=2) | 3098 (511–5685, n=2) |
| lines_read | 100 (88–112, n=2) | 73.5 (16–131, n=2) |
| unparsed_bash_reads | 2 (1–3, n=2) | 2 (1–3, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 2147 (0–4294, n=2) |
| information_density | 0.106 (0.092–0.119, n=2) | 0.221 (0–0.443, n=2) |
| session_ms | 50979 (47235–54723, n=2) | 33681 (30680–36682, n=2) |
| wall_ms | 52006 (48254–55758, n=2) | 35185 (32178–38192, n=2) |
| atlas_index_ms | – | 542 (534–550, n=2) |

### t10-auth-challenge-quoting

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 6 (6–6, n=2) | 6 (6–6, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4.5 (4–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 1 (1–1, n=2) | 0.875 (0.75–1, n=2) |
| judge_architectural_consistency | 4 (4–4, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 207343 (182422–232264, n=2) | 220676 (213919–227433, n=2) |
| input_tokens | 20 (20–20, n=2) | 19 (18–20, n=2) |
| output_tokens | 6055 (5640–6470, n=2) | 6066 (5745–6387, n=2) |
| cache_read_tokens | 179035 (156860–201210, n=2) | 196487 (189042–203932, n=2) |
| cache_creation_tokens | 22233 (19902–24564, n=2) | 18104 (17094–19114, n=2) |
| cost_usd | 0.185 (0.167–0.203, n=2) | 0.172 (0.172–0.173, n=2) |
| num_turns | 12 (11–13, n=2) | 14 (13–15, n=2) |
| tool_calls | 11 (10–12, n=2) | 13 (12–14, n=2) |
| search_calls | 2 (2–2, n=2) | 2.5 (2–3, n=2) |
| files_read | 8 (8–8, n=2) | 6.5 (6–7, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| bytes_read | 17463 (12942–21984, n=2) | 12546 (12455–12637, n=2) |
| lines_read | 438.5 (359–518, n=2) | 308.5 (303–314, n=2) |
| unparsed_bash_reads | 1.5 (1–2, n=2) | 2 (2–2, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 588.5 (588–589, n=2) |
| information_density | 0.541 (0.343–0.738, n=2) | 0.541 (0.464–0.617, n=2) |
| session_ms | 56862 (46253–67471, n=2) | 52695 (52261–53129, n=2) |
| wall_ms | 57868.5 (47244–68493, n=2) | 54313.5 (53972–54655, n=2) |
| atlas_index_ms | – | 530 (522–538, n=2) |

## Negative results

- **t00-smoke** `judge_completeness`: baseline 4 vs atlas 3 — transcripts: t00-smoke.atlas.r0/transcript.jsonl, t00-smoke.atlas.r1/transcript.jsonl, t00-smoke.baseline.r0/transcript.jsonl, t00-smoke.baseline.r1/transcript.jsonl
- **t00-smoke** `total_tokens`: baseline 36543.5 vs atlas 126882 — transcripts: t00-smoke.atlas.r0/transcript.jsonl, t00-smoke.atlas.r1/transcript.jsonl, t00-smoke.baseline.r0/transcript.jsonl, t00-smoke.baseline.r1/transcript.jsonl
- **t01-cwd-relative-targets** `total_tokens`: baseline 226802.5 vs atlas 297342.5 — transcripts: t01-cwd-relative-targets.atlas.r0/transcript.jsonl, t01-cwd-relative-targets.atlas.r1/transcript.jsonl, t01-cwd-relative-targets.baseline.r0/transcript.jsonl, t01-cwd-relative-targets.baseline.r1/transcript.jsonl
- **t02-typed-degraded-cause** `total_tokens`: baseline 218022.5 vs atlas 312933.5 — transcripts: t02-typed-degraded-cause.atlas.r0/transcript.jsonl, t02-typed-degraded-cause.atlas.r1/transcript.jsonl, t02-typed-degraded-cause.baseline.r0/transcript.jsonl, t02-typed-degraded-cause.baseline.r1/transcript.jsonl
- **t03-nested-router-links** `total_tokens`: baseline 248547 vs atlas 365564 — transcripts: t03-nested-router-links.atlas.r0/transcript.jsonl, t03-nested-router-links.atlas.r1/transcript.jsonl, t03-nested-router-links.baseline.r0/transcript.jsonl, t03-nested-router-links.baseline.r1/transcript.jsonl
- **t04-detector-import-gates** `total_tokens`: baseline 233872 vs atlas 255327 — transcripts: t04-detector-import-gates.atlas.r0/transcript.jsonl, t04-detector-import-gates.atlas.r1/transcript.jsonl, t04-detector-import-gates.baseline.r0/transcript.jsonl, t04-detector-import-gates.baseline.r1/transcript.jsonl
- **t05-mcp-dependency-tools** `hidden_tests_passed`: baseline 28 vs atlas 27.5 — transcripts: t05-mcp-dependency-tools.atlas.r0/transcript.jsonl, t05-mcp-dependency-tools.atlas.r1/transcript.jsonl, t05-mcp-dependency-tools.baseline.r0/transcript.jsonl, t05-mcp-dependency-tools.baseline.r1/transcript.jsonl
- **t05-mcp-dependency-tools** `total_tokens`: baseline 307705.5 vs atlas 348206 — transcripts: t05-mcp-dependency-tools.atlas.r0/transcript.jsonl, t05-mcp-dependency-tools.atlas.r1/transcript.jsonl, t05-mcp-dependency-tools.baseline.r0/transcript.jsonl, t05-mcp-dependency-tools.baseline.r1/transcript.jsonl
- **t06-repo-file-size-limit** `total_tokens`: baseline 303691 vs atlas 334602.5 — transcripts: t06-repo-file-size-limit.atlas.r0/transcript.jsonl, t06-repo-file-size-limit.atlas.r1/transcript.jsonl, t06-repo-file-size-limit.baseline.r0/transcript.jsonl, t06-repo-file-size-limit.baseline.r1/transcript.jsonl
- **t07-static-precompressed-quality** `total_tokens`: baseline 119886.5 vs atlas 159584 — transcripts: t07-static-precompressed-quality.atlas.r0/transcript.jsonl, t07-static-precompressed-quality.atlas.r1/transcript.jsonl, t07-static-precompressed-quality.baseline.r0/transcript.jsonl, t07-static-precompressed-quality.baseline.r1/transcript.jsonl
- **t08-cookie-samesite-default** `judge_completeness`: baseline 4.5 vs atlas 3.5 — transcripts: t08-cookie-samesite-default.atlas.r0/transcript.jsonl, t08-cookie-samesite-default.atlas.r1/transcript.jsonl, t08-cookie-samesite-default.baseline.r0/transcript.jsonl, t08-cookie-samesite-default.baseline.r1/transcript.jsonl
- **t08-cookie-samesite-default** `total_tokens`: baseline 330611 vs atlas 433830 — transcripts: t08-cookie-samesite-default.atlas.r0/transcript.jsonl, t08-cookie-samesite-default.atlas.r1/transcript.jsonl, t08-cookie-samesite-default.baseline.r0/transcript.jsonl, t08-cookie-samesite-default.baseline.r1/transcript.jsonl
- **t10-auth-challenge-quoting** `total_tokens`: baseline 207343 vs atlas 220676 — transcripts: t10-auth-challenge-quoting.atlas.r0/transcript.jsonl, t10-auth-challenge-quoting.atlas.r1/transcript.jsonl, t10-auth-challenge-quoting.baseline.r0/transcript.jsonl, t10-auth-challenge-quoting.baseline.r1/transcript.jsonl

## Judge failures

- t03-nested-router-links.baseline.r1: Exited { code: Some(1) }: 

