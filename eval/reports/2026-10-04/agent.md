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
| eval repository commit | 0dc8a96fffd02b1a82f0da9e5469ed0128f90a74 |
| code-atlas binary version (build commit) | code-atlas 0.1.0 (commit 6a2854f4df88e1e5705f8fae091824ec290e3cee) |
| code-atlas binary blake3 | ddbc1afe75ba566247744839faffbe9c145dfd64cc89b6734af90ad3f8e133b8 |
| preamble version | 1 |
| repo override | none |
| spent (reported) | $8.53 |

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
| hidden_tests_passed | 43.773 (3–336, tasks=11) | 43.773 (3–336.5, tasks=11) |
| regressions | 0 (0–0, tasks=11) | 0 (0–0, tasks=11) |
| judge_correctness | 4.591 (4–5, tasks=11) | 4.591 (4–5, tasks=11) |
| judge_completeness | 4.136 (4–5, tasks=11) | 4.091 (3–5, tasks=11) |

### Reuse and blast radius

| metric | baseline (11 tasks) | atlas (11 tasks) |
|---|---|---|
| must_change_coverage | 1 (1–1, tasks=11) | 1 (1–1, tasks=11) |
| must_consider_coverage | 0.452 (0–1, tasks=11) | 0.479 (0–1, tasks=11) |
| reuse_targets_referenced | 1 (1–1, tasks=4 of 11) | 0.75 (0–1, tasks=4 of 11) |
| duplication_introduced | 0 (0–0, tasks=5 of 11) | 0 (0–0, tasks=5 of 11) |
| lexical_duplicate_candidates | 0 (0–0, tasks=5 of 11) | 0.4 (0–2, tasks=5 of 11) |
| judge_architectural_consistency | 4.5 (3.5–5, tasks=11) | 4.636 (4–5, tasks=11) |
| judge_appropriate_reuse | 4.409 (3–5, tasks=11) | 4.682 (4–5, tasks=11) |
| judge_blast_radius_handling | 4 (3.5–5, tasks=11) | 3.773 (2–4.5, tasks=11) |

### Tokens and context

| metric | baseline (11 tasks) | atlas (11 tasks) |
|---|---|---|
| total_tokens | 207066.227 (37790–336414.5, tasks=11) | 245716.455 (107634.5–443073.5, tasks=11) |
| input_tokens | 19.364 (9–31, tasks=11) | 18.636 (12–35, tasks=11) |
| output_tokens | 4323.227 (1420.5–6048, tasks=11) | 4093.818 (1599.5–6617, tasks=11) |
| cache_read_tokens | 184423.636 (32757.5–305136.5, tasks=11) | 222073.909 (97777–413894.5, tasks=11) |
| cache_creation_tokens | 18300 (3603–26988, tasks=11) | 19530.091 (8246–26559, tasks=11) |
| cost_usd | 0.153 (0.035–0.212, tasks=11) | 0.164 (0.069–0.233, tasks=11) |
| num_turns | 12.636 (7–16.5, tasks=11) | 11.182 (7.5–18.5, tasks=11) |
| tool_calls | 11.636 (6–15.5, tasks=11) | 10.182 (6.5–17.5, tasks=11) |
| search_calls | 3.318 (1.5–5.5, tasks=11) | 2.864 (0–4, tasks=11) |
| files_read | 4.273 (2–7.5, tasks=11) | 3.545 (1–6, tasks=11) |
| irrelevant_files_read | 0.636 (0–4, tasks=11) | 0.364 (0–2, tasks=11) |
| relevant_files_missed | 1.773 (0–4, tasks=11) | 1.818 (0–4, tasks=11) |
| bytes_read | 14674.818 (1695.5–31581, tasks=11) | 13738.909 (657–28088, tasks=11) |
| lines_read | 377.318 (60–740, tasks=11) | 349.727 (36.5–646.5, tasks=11) |
| unparsed_bash_reads | 0.727 (0–2, tasks=11) | 0.955 (0–3.5, tasks=11) |
| atlas_calls | 0 (0–0, tasks=11) | 0.091 (0–1, tasks=11) |
| atlas_bytes | 0 (0–0, tasks=11) | 1017.955 (0–11197.5, tasks=11) |
| information_density | 0.512 (0.091–0.829, tasks=11) | 0.499 (0.183–0.835, tasks=11) |
| first_edit_ms | 15877.6 (9752.5–20997.5, tasks=5 of 11) | 29632.5 (11271.5–74249, tasks=5 of 11) |
| session_ms | 136166.045 (15948.5–342912.5, tasks=11) | 142513.227 (18049.5–331792.5, tasks=11) |
| wall_ms | 137240 (17054.5–344041, tasks=11) | 144388.864 (19801–333729.5, tasks=11) |
| atlas_index_ms | – | 661.227 (64.5–911, tasks=11) |

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
| judge_architectural_consistency | 4.5 (4–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 3.5 (3–4, n=2) | 2 (2–2, n=2) |
| total_tokens | 37790 (31063–44517, n=2) | 107634.5 (87933–127336, n=2) |
| input_tokens | 9 (8–10, n=2) | 12 (10–14, n=2) |
| output_tokens | 1420.5 (1191–1650, n=2) | 1599.5 (1338–1861, n=2) |
| cache_read_tokens | 32757.5 (26956–38559, n=2) | 97777 (78684–116870, n=2) |
| cache_creation_tokens | 3603 (2908–4298, n=2) | 8246 (7901–8591, n=2) |
| cost_usd | 0.035 (0.029–0.041, n=2) | 0.069 (0.061–0.076, n=2) |
| num_turns | 7 (6–8, n=2) | 7.5 (6–9, n=2) |
| tool_calls | 6 (5–7, n=2) | 6.5 (5–8, n=2) |
| search_calls | 1.5 (1–2, n=2) | 0 (0–0, n=2) |
| files_read | 2.5 (2–3, n=2) | 1.5 (1–2, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 0 (0–0, n=2) | 0 (0–0, n=2) |
| bytes_read | 1695.5 (1573–1818, n=2) | 657 (517–797, n=2) |
| lines_read | 60 (55–65, n=2) | 36.5 (29–44, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 11197.5 (11193–11202, n=2) |
| information_density | 0.545 (0.355–0.735, n=2) | 0.19 (0.173–0.207, n=2) |
| first_edit_ms | 9752.5 (9167–10338, n=2) | 11271.5 (10855–11688, n=2) |
| session_ms | 15948.5 (13030–18867, n=2) | 18049.5 (15027–21072, n=2) |
| wall_ms | 17054.5 (14188–19921, n=2) | 19801 (16763–22839, n=2) |
| atlas_index_ms | – | 64.5 (57–72, n=2) |

### t01-cwd-relative-targets

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 4 (4–4, n=2) | 4 (4–4, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (n=1 of 2) | 4 (4–4, n=2) |
| judge_completeness | 4 (n=1 of 2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.125 (0–0.25, n=2) | 0 (0–0, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 5 (n=1 of 2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 4 (n=1 of 2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (n=1 of 2) | 3.5 (3–4, n=2) |
| total_tokens | 305374.5 (273815–336934, n=2) | 268078 (236518–299638, n=2) |
| input_tokens | 27 (24–30, n=2) | 20 (18–22, n=2) |
| output_tokens | 5120 (4970–5270, n=2) | 4166.5 (3677–4656, n=2) |
| cache_read_tokens | 283391.5 (250975–315808, n=2) | 249111.5 (218332–279891, n=2) |
| cache_creation_tokens | 16836 (16126–17546, n=2) | 14780 (14491–15069, n=2) |
| cost_usd | 0.175 (0.173–0.177, n=2) | 0.151 (0.138–0.163, n=2) |
| num_turns | 13.5 (12–15, n=2) | 10.5 (9–12, n=2) |
| tool_calls | 12.5 (11–14, n=2) | 9.5 (8–11, n=2) |
| search_calls | 5.5 (4–7, n=2) | 3 (3–3, n=2) |
| files_read | 2 (2–2, n=2) | 1.5 (1–2, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 3 (3–3, n=2) |
| bytes_read | 13017.5 (12098–13937, n=2) | 11742.5 (10190–13295, n=2) |
| lines_read | 318 (310–326, n=2) | 294 (259–329, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.593 (0.563–0.623, n=2) | 0.649 (0.555–0.743, n=2) |
| first_edit_ms | – | 74249 (n=1 of 2) |
| session_ms | 214616 (198264–230968, n=2) | 148325 (74568–222082, n=2) |
| wall_ms | 215692.5 (199362–232023, n=2) | 152386 (76088–228684, n=2) |
| atlas_index_ms | – | 822.5 (800–845, n=2) |

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
| judge_architectural_consistency | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 4.5 (4–5, n=2) | 4.5 (4–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 181558.5 (159001–204116, n=2) | 263289.5 (229543–297036, n=2) |
| input_tokens | 16 (14–18, n=2) | 18 (16–20, n=2) |
| output_tokens | 4592.5 (4016–5169, n=2) | 5406 (5027–5785, n=2) |
| cache_read_tokens | 159722 (137813–181631, n=2) | 231306.5 (207901–254712, n=2) |
| cache_creation_tokens | 17228 (17158–17298, n=2) | 26559 (16599–36519, n=2) |
| cost_usd | 0.147 (0.136–0.157, n=2) | 0.207 (0.158–0.255, n=2) |
| num_turns | 10 (9–11, n=2) | 11 (10–12, n=2) |
| tool_calls | 9 (8–10, n=2) | 10 (9–11, n=2) |
| search_calls | 2.5 (2–3, n=2) | 2.5 (2–3, n=2) |
| files_read | 6 (6–6, n=2) | 3.5 (3–4, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 4 (4–4, n=2) | 4 (4–4, n=2) |
| bytes_read | 14181.5 (12622–15741, n=2) | 11169.5 (10215–12124, n=2) |
| lines_read | 333.5 (319–348, n=2) | 256.5 (226–287, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 1 (1–1, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.579 (0.537–0.621, n=2) | 0.464 (0.454–0.474, n=2) |
| session_ms | 252424.5 (214409–290440, n=2) | 284659.5 (261732–307587, n=2) |
| wall_ms | 253557.5 (215539–291576, n=2) | 286357.5 (263422–309293, n=2) |
| atlas_index_ms | – | 860.5 (781–940, n=2) |

### t03-nested-router-links

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 5 (5–5, n=2) | 5 (5–5, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 240302.5 (202683–277922, n=2) | 398031.5 (370307–425756, n=2) |
| input_tokens | 19 (18–20, n=2) | 24 (22–26, n=2) |
| output_tokens | 3637 (3399–3875, n=2) | 4153 (4077–4229, n=2) |
| cache_read_tokens | 213784.5 (180091–247478, n=2) | 368156 (341347–394965, n=2) |
| cache_creation_tokens | 22862 (19175–26549, n=2) | 25698.5 (24709–26688, n=2) |
| cost_usd | 0.171 (0.147–0.194, n=2) | 0.218 (0.209–0.227, n=2) |
| num_turns | 14.5 (14–15, n=2) | 14 (14–14, n=2) |
| tool_calls | 13.5 (13–14, n=2) | 13 (13–13, n=2) |
| search_calls | 4.5 (4–5, n=2) | 4 (4–4, n=2) |
| files_read | 3.5 (3–4, n=2) | 3 (3–3, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 1 (1–1, n=2) | 1 (1–1, n=2) |
| bytes_read | 31581 (26322–36840, n=2) | 27313 (24608–30018, n=2) |
| lines_read | 740 (625–855, n=2) | 646.5 (595–698, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 1.5 (0–3, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.749 (0.656–0.842, n=2) | 0.568 (0.447–0.689, n=2) |
| first_edit_ms | 15983.5 (13894–18073, n=2) | 27673 (19320–36026, n=2) |
| session_ms | 238740 (60467–417013, n=2) | 331792.5 (255766–407819, n=2) |
| wall_ms | 239816.5 (61472–418161, n=2) | 333729.5 (257934–409525, n=2) |
| atlas_index_ms | – | 867 (820–914, n=2) |

### t04-detector-import-gates

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 7 (7–7, n=2) | 7 (7–7, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (n=1 of 2) |
| judge_completeness | 4 (4–4, n=2) | 5 (n=1 of 2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0 (0–0, n=2) | 0.167 (0–0.333, n=2) |
| judge_architectural_consistency | 4 (4–4, n=2) | 4 (n=1 of 2) |
| judge_appropriate_reuse | 4 (4–4, n=2) | 4 (n=1 of 2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (n=1 of 2) |
| total_tokens | 129485.5 (125651–133320, n=2) | 214142.5 (202785–225500, n=2) |
| input_tokens | 12 (12–12, n=2) | 15 (14–16, n=2) |
| output_tokens | 5070.5 (4215–5926, n=2) | 4550.5 (4318–4783, n=2) |
| cache_read_tokens | 106784.5 (105822–107747, n=2) | 184417 (180453–188381, n=2) |
| cache_creation_tokens | 17618.5 (15602–19635, n=2) | 25160 (17535–32785, n=2) |
| cost_usd | 0.143 (0.126–0.159, n=2) | 0.183 (0.154–0.212, n=2) |
| num_turns | 13.5 (9–18, n=2) | 9 (9–9, n=2) |
| tool_calls | 12.5 (8–17, n=2) | 8 (8–8, n=2) |
| search_calls | 2 (2–2, n=2) | 2.5 (2–3, n=2) |
| files_read | 7 (5–9, n=2) | 5.5 (5–6, n=2) |
| irrelevant_files_read | 4 (0–8, n=2) | 2 (0–4, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 2.5 (2–3, n=2) |
| bytes_read | 14778 (13861–15695, n=2) | 15103 (13367–16839, n=2) |
| lines_read | 377.5 (342–413, n=2) | 389 (349–429, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.488 (0.395–0.58, n=2) | 0.526 (0.408–0.643, n=2) |
| first_edit_ms | 18109 (16891–19327, n=2) | – |
| session_ms | 61791.5 (58592–64991, n=2) | 195496.5 (56627–334366, n=2) |
| wall_ms | 62801.5 (59628–65975, n=2) | 197232 (58123–336341, n=2) |
| atlas_index_ms | – | 911 (840–982, n=2) |

### t05-mcp-dependency-tools

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 27.5 (27–28, n=2) | 27 (27–27, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.5 (0.5–0.5, n=2) | 0.5 (0.5–0.5, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 3.5 (3–4, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 3 (3–3, n=2) | 4 (4–4, n=2) |
| judge_blast_radius_handling | 3.5 (3–4, n=2) | 3.5 (3–4, n=2) |
| total_tokens | 336414.5 (200405–472424, n=2) | 188815.5 (144841–232790, n=2) |
| input_tokens | 21 (16–26, n=2) | 12 (10–14, n=2) |
| output_tokens | 4269 (3073–5465, n=2) | 3088 (2355–3821, n=2) |
| cache_read_tokens | 305136.5 (177153–433120, n=2) | 163888 (122139–205637, n=2) |
| cache_creation_tokens | 26988 (20163–33813, n=2) | 21827.5 (20337–23318, n=2) |
| cost_usd | 0.212 (0.147–0.277, n=2) | 0.151 (0.129–0.173, n=2) |
| num_turns | 15.5 (12–19, n=2) | 11.5 (9–14, n=2) |
| tool_calls | 14.5 (11–18, n=2) | 10.5 (8–13, n=2) |
| search_calls | 4.5 (4–5, n=2) | 3.5 (3–4, n=2) |
| files_read | 4 (4–4, n=2) | 4 (4–4, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 3 (3–3, n=2) | 3 (3–3, n=2) |
| bytes_read | 28952.5 (23598–34307, n=2) | 28088 (24805–31371, n=2) |
| lines_read | 649.5 (515–784, n=2) | 646.5 (575–718, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.622 (0.571–0.673, n=2) | 0.737 (0.634–0.841, n=2) |
| first_edit_ms | 20997.5 (19986–22009, n=2) | 14722 (n=1 of 2) |
| session_ms | 152890 (60922–244858, n=2) | 54042.5 (51429–56656, n=2) |
| wall_ms | 154041 (62112–245970, n=2) | 55573.5 (52943–58204, n=2) |
| atlas_index_ms | – | 797 (775–819, n=2) |

### t06-repo-file-size-limit

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 3 (3–3, n=2) | 3 (3–3, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_completeness | 4 (4–4, n=2) | 3.5 (3–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.5 (0.5–0.5, n=2) | 0.625 (0.5–0.75, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 0 (0–0, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 279165.5 (237249–321082, n=2) | 287708 (282770–292646, n=2) |
| input_tokens | 24 (22–26, n=2) | 22 (22–22, n=2) |
| output_tokens | 4965 (4304–5626, n=2) | 3558 (3422–3694, n=2) |
| cache_read_tokens | 251610 (210964–292256, n=2) | 270340.5 (266860–273821, n=2) |
| cache_creation_tokens | 22566.5 (21959–23174, n=2) | 13787.5 (12466–15109, n=2) |
| cost_usd | 0.19 (0.173–0.207, n=2) | 0.145 (0.138–0.152, n=2) |
| num_turns | 14.5 (14–15, n=2) | 11 (11–11, n=2) |
| tool_calls | 13.5 (13–14, n=2) | 10 (10–10, n=2) |
| search_calls | 4 (3–5, n=2) | 4 (3–5, n=2) |
| files_read | 4 (2–6, n=2) | 4.5 (4–5, n=2) |
| irrelevant_files_read | 1.5 (0–3, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 2 (2–2, n=2) | 1.5 (1–2, n=2) |
| bytes_read | 12234 (8239–16229, n=2) | 10045 (9717–10373, n=2) |
| lines_read | 351 (250–452, n=2) | 259 (240–278, n=2) |
| unparsed_bash_reads | 2 (2–2, n=2) | 0 (0–0, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.33 (0.206–0.453, n=2) | 0.577 (0.513–0.641, n=2) |
| session_ms | 342912.5 (250370–435455, n=2) | 300850.5 (286944–314757, n=2) |
| wall_ms | 344041 (251482–436600, n=2) | 302489 (288545–316433, n=2) |
| atlas_index_ms | – | 831 (813–849, n=2) |

### t07-static-precompressed-quality

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 8 (8–8, n=2) | 8 (8–8, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 4.5 (4–5, n=2) | 5 (n=1 of 2) |
| judge_completeness | 4.5 (4–5, n=2) | 4 (n=1 of 2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.333 (0.333–0.333, n=2) | 0.333 (0.333–0.333, n=2) |
| reuse_targets_referenced | 1 (1–1, n=2) | 1 (1–1, n=2) |
| duplication_introduced | 0 (0–0, n=2) | 0 (0–0, n=2) |
| lexical_duplicate_candidates | 0 (0–0, n=2) | 2 (1–3, n=2) |
| judge_architectural_consistency | 4 (4–4, n=2) | 5 (n=1 of 2) |
| judge_appropriate_reuse | 5 (5–5, n=2) | 5 (n=1 of 2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (n=1 of 2) |
| total_tokens | 126094.5 (125237–126952, n=2) | 130881.5 (105221–156542, n=2) |
| input_tokens | 16 (16–16, n=2) | 12 (10–14, n=2) |
| output_tokens | 4070 (3794–4346, n=2) | 3745 (3484–4006, n=2) |
| cache_read_tokens | 106809 (106533–107085, n=2) | 106154.5 (87134–125175, n=2) |
| cache_creation_tokens | 15199.5 (14894–15505, n=2) | 20970 (14593–27347, n=2) |
| cost_usd | 0.123 (0.119–0.127, n=2) | 0.143 (0.111–0.175, n=2) |
| num_turns | 13 (13–13, n=2) | 9.5 (8–11, n=2) |
| tool_calls | 12 (12–12, n=2) | 8.5 (7–10, n=2) |
| search_calls | 3 (3–3, n=2) | 2 (2–2, n=2) |
| files_read | 3 (3–3, n=2) | 3 (3–3, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 2 (2–2, n=2) | 2 (2–2, n=2) |
| bytes_read | 18839 (18216–19462, n=2) | 19088 (18021–20155, n=2) |
| lines_read | 579 (557–601, n=2) | 583.5 (551–616, n=2) |
| unparsed_bash_reads | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.829 (0.804–0.855, n=2) | 0.835 (0.821–0.849, n=2) |
| first_edit_ms | 14545.5 (11215–17876, n=2) | 20247 (n=1 of 2) |
| session_ms | 35707.5 (35340–36075, n=2) | 45556 (37257–53855, n=2) |
| wall_ms | 36757 (36348–37166, n=2) | 47071 (38778–55364, n=2) |
| atlas_index_ms | – | 496.5 (485–508, n=2) |

### t08-cookie-samesite-default

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 336 (336–336, n=2) | 336.5 (336–337, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 4 (4–4, n=2) | 4 (4–4, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_architectural_consistency | 3.5 (3–4, n=2) | 4 (4–4, n=2) |
| judge_appropriate_reuse | 4 (3–5, n=2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 5 (5–5, n=2) | 4.5 (4–5, n=2) |
| total_tokens | 311236 (306773–315699, n=2) | 443073.5 (373123–513024, n=2) |
| input_tokens | 31 (30–32, n=2) | 35 (32–38, n=2) |
| output_tokens | 6048 (5807–6289, n=2) | 5610.5 (5220–6001, n=2) |
| cache_read_tokens | 281767 (278997–284537, n=2) | 413894.5 (348110–479679, n=2) |
| cache_creation_tokens | 23390 (21937–24843, n=2) | 23533.5 (19761–27306, n=2) |
| cost_usd | 0.21 (0.202–0.219, n=2) | 0.233 (0.201–0.265, n=2) |
| num_turns | 16.5 (16–17, n=2) | 18.5 (17–20, n=2) |
| tool_calls | 15.5 (15–16, n=2) | 17.5 (16–19, n=2) |
| search_calls | 4 (3–5, n=2) | 4 (4–4, n=2) |
| files_read | 5 (5–5, n=2) | 5.5 (5–6, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 1 (1–1, n=2) | 1 (1–1, n=2) |
| bytes_read | 12152.5 (12093–12212, n=2) | 12900 (8871–16929, n=2) |
| lines_read | 345 (330–360, n=2) | 344.5 (260–429, n=2) |
| unparsed_bash_reads | 2 (1–3, n=2) | 1.5 (1–2, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.376 (0.349–0.403, n=2) | 0.346 (0.331–0.361, n=2) |
| session_ms | 97962.5 (85460–110465, n=2) | 95581.5 (93243–97920, n=2) |
| wall_ms | 98985 (86546–111424, n=2) | 97267 (94988–99546, n=2) |
| atlas_index_ms | – | 562 (515–609, n=2) |

### t09-lambda-text-content-types

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 76 (76–76, n=2) | 76 (76–76, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_completeness | 5 (5–5, n=2) | 5 (5–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| judge_architectural_consistency | 5 (5–5, n=2) | 5 (5–5, n=2) |
| judge_appropriate_reuse | 4 (4–4, n=2) | 4 (4–4, n=2) |
| judge_blast_radius_handling | 4 (4–4, n=2) | 4 (4–4, n=2) |
| total_tokens | 162556.5 (133583–191530, n=2) | 173920 (137456–210384, n=2) |
| input_tokens | 19 (16–22, n=2) | 16 (14–18, n=2) |
| output_tokens | 2922 (2684–3160, n=2) | 2538 (2265–2811, n=2) |
| cache_read_tokens | 143267 (114888–171646, n=2) | 156277.5 (122704–189851, n=2) |
| cache_creation_tokens | 16348.5 (15995–16702, n=2) | 15088.5 (12473–17704, n=2) |
| cost_usd | 0.123 (0.114–0.133, n=2) | 0.117 (0.097–0.137, n=2) |
| num_turns | 9.5 (8–11, n=2) | 8 (7–9, n=2) |
| tool_calls | 8.5 (7–10, n=2) | 7 (6–8, n=2) |
| search_calls | 3 (2–4, n=2) | 3.5 (3–4, n=2) |
| files_read | 2.5 (2–3, n=2) | 1 (1–1, n=2) |
| irrelevant_files_read | 0.5 (0–1, n=2) | 0.5 (0–1, n=2) |
| relevant_files_missed | 0 (0–0, n=2) | 0 (0–0, n=2) |
| bytes_read | 2814.5 (2178–3451, n=2) | 4093.5 (1469–6718, n=2) |
| lines_read | 69.5 (56–83, n=2) | 77.5 (33–122, n=2) |
| unparsed_bash_reads | 2 (1–3, n=2) | 2 (1–3, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.091 (0.085–0.096, n=2) | 0.183 (0–0.367, n=2) |
| session_ms | 38553.5 (34707–42400, n=2) | 37966 (32042–43890, n=2) |
| wall_ms | 39554 (35724–43384, n=2) | 39478 (33567–45389, n=2) |
| atlas_index_ms | – | 516.5 (493–540, n=2) |

### t10-auth-challenge-quoting

- baseline: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0
- atlas: 2 runs: completed 2, max_turns 0, reported_error 0, timed_out 0, no_result 0, setup_failed 0; contaminated 0; estimated costs 0

| metric | baseline | atlas |
|---|---|---|
| hidden_tests_pass | 1 (1–1, n=2) | 1 (1–1, n=2) |
| hidden_tests_passed | 6 (6–6, n=2) | 6 (6–6, n=2) |
| regressions | 0 (0–0, n=2) | 0 (0–0, n=2) |
| judge_correctness | 5 (n=1 of 2) | 4.5 (4–5, n=2) |
| judge_completeness | 4 (n=1 of 2) | 4.5 (4–5, n=2) |
| must_change_coverage | 1 (1–1, n=2) | 1 (1–1, n=2) |
| must_consider_coverage | 0.875 (0.75–1, n=2) | 0.5 (0–1, n=2) |
| judge_architectural_consistency | 5 (n=1 of 2) | 4 (4–4, n=2) |
| judge_appropriate_reuse | 5 (n=1 of 2) | 5 (5–5, n=2) |
| judge_blast_radius_handling | 4 (n=1 of 2) | 4 (4–4, n=2) |
| total_tokens | 167750.5 (164980–170521, n=2) | 227306.5 (212475–242138, n=2) |
| input_tokens | 19 (16–22, n=2) | 19 (18–20, n=2) |
| output_tokens | 5441 (4843–6039, n=2) | 6617 (5977–7257, n=2) |
| cache_read_tokens | 143630.5 (141053–146208, n=2) | 201490 (188610–214370, n=2) |
| cache_creation_tokens | 18660 (13907–23413, n=2) | 19180.5 (17870–20491, n=2) |
| cost_usd | 0.158 (0.133–0.182, n=2) | 0.183 (0.169–0.197, n=2) |
| num_turns | 11.5 (11–12, n=2) | 12.5 (12–13, n=2) |
| tool_calls | 10.5 (10–11, n=2) | 11.5 (11–12, n=2) |
| search_calls | 2 (1–3, n=2) | 2.5 (2–3, n=2) |
| files_read | 7.5 (7–8, n=2) | 6 (4–8, n=2) |
| irrelevant_files_read | 0 (0–0, n=2) | 0 (0–0, n=2) |
| relevant_files_missed | 0.5 (0–1, n=2) | 2 (0–4, n=2) |
| bytes_read | 11177 (8922–13432, n=2) | 10928.5 (10565–11292, n=2) |
| lines_read | 327.5 (248–407, n=2) | 313.5 (302–325, n=2) |
| unparsed_bash_reads | 2 (1–3, n=2) | 3.5 (3–4, n=2) |
| atlas_calls | 0 (0–0, n=2) | 0 (0–0, n=2) |
| atlas_bytes | 0 (0–0, n=2) | 0 (0–0, n=2) |
| information_density | 0.43 (0.359–0.501, n=2) | 0.411 (0.366–0.456, n=2) |
| session_ms | 46280 (41783–50777, n=2) | 55326 (50808–59844, n=2) |
| wall_ms | 47339.5 (42785–51894, n=2) | 56893 (52366–61420, n=2) |
| atlas_index_ms | – | 545 (532–558, n=2) |

## Negative results

- **t00-smoke** `judge_completeness`: baseline 4 vs atlas 3 — transcripts: t00-smoke.atlas.r0/transcript.jsonl, t00-smoke.atlas.r1/transcript.jsonl, t00-smoke.baseline.r0/transcript.jsonl, t00-smoke.baseline.r1/transcript.jsonl
- **t00-smoke** `total_tokens`: baseline 37790 vs atlas 107634.5 — transcripts: t00-smoke.atlas.r0/transcript.jsonl, t00-smoke.atlas.r1/transcript.jsonl, t00-smoke.baseline.r0/transcript.jsonl, t00-smoke.baseline.r1/transcript.jsonl
- **t02-typed-degraded-cause** `total_tokens`: baseline 181558.5 vs atlas 263289.5 — transcripts: t02-typed-degraded-cause.atlas.r0/transcript.jsonl, t02-typed-degraded-cause.atlas.r1/transcript.jsonl, t02-typed-degraded-cause.baseline.r0/transcript.jsonl, t02-typed-degraded-cause.baseline.r1/transcript.jsonl
- **t03-nested-router-links** `total_tokens`: baseline 240302.5 vs atlas 398031.5 — transcripts: t03-nested-router-links.atlas.r0/transcript.jsonl, t03-nested-router-links.atlas.r1/transcript.jsonl, t03-nested-router-links.baseline.r0/transcript.jsonl, t03-nested-router-links.baseline.r1/transcript.jsonl
- **t04-detector-import-gates** `total_tokens`: baseline 129485.5 vs atlas 214142.5 — transcripts: t04-detector-import-gates.atlas.r0/transcript.jsonl, t04-detector-import-gates.atlas.r1/transcript.jsonl, t04-detector-import-gates.baseline.r0/transcript.jsonl, t04-detector-import-gates.baseline.r1/transcript.jsonl
- **t05-mcp-dependency-tools** `hidden_tests_passed`: baseline 27.5 vs atlas 27 — transcripts: t05-mcp-dependency-tools.atlas.r0/transcript.jsonl, t05-mcp-dependency-tools.atlas.r1/transcript.jsonl, t05-mcp-dependency-tools.baseline.r0/transcript.jsonl, t05-mcp-dependency-tools.baseline.r1/transcript.jsonl
- **t06-repo-file-size-limit** `judge_completeness`: baseline 4 vs atlas 3.5 — transcripts: t06-repo-file-size-limit.atlas.r0/transcript.jsonl, t06-repo-file-size-limit.atlas.r1/transcript.jsonl, t06-repo-file-size-limit.baseline.r0/transcript.jsonl, t06-repo-file-size-limit.baseline.r1/transcript.jsonl
- **t06-repo-file-size-limit** `total_tokens`: baseline 279165.5 vs atlas 287708 — transcripts: t06-repo-file-size-limit.atlas.r0/transcript.jsonl, t06-repo-file-size-limit.atlas.r1/transcript.jsonl, t06-repo-file-size-limit.baseline.r0/transcript.jsonl, t06-repo-file-size-limit.baseline.r1/transcript.jsonl
- **t07-static-precompressed-quality** `judge_completeness`: baseline 4.5 vs atlas 4 — transcripts: t07-static-precompressed-quality.atlas.r0/transcript.jsonl, t07-static-precompressed-quality.atlas.r1/transcript.jsonl, t07-static-precompressed-quality.baseline.r0/transcript.jsonl, t07-static-precompressed-quality.baseline.r1/transcript.jsonl
- **t07-static-precompressed-quality** `total_tokens`: baseline 126094.5 vs atlas 130881.5 — transcripts: t07-static-precompressed-quality.atlas.r0/transcript.jsonl, t07-static-precompressed-quality.atlas.r1/transcript.jsonl, t07-static-precompressed-quality.baseline.r0/transcript.jsonl, t07-static-precompressed-quality.baseline.r1/transcript.jsonl
- **t08-cookie-samesite-default** `total_tokens`: baseline 311236 vs atlas 443073.5 — transcripts: t08-cookie-samesite-default.atlas.r0/transcript.jsonl, t08-cookie-samesite-default.atlas.r1/transcript.jsonl, t08-cookie-samesite-default.baseline.r0/transcript.jsonl, t08-cookie-samesite-default.baseline.r1/transcript.jsonl
- **t09-lambda-text-content-types** `total_tokens`: baseline 162556.5 vs atlas 173920 — transcripts: t09-lambda-text-content-types.atlas.r0/transcript.jsonl, t09-lambda-text-content-types.atlas.r1/transcript.jsonl, t09-lambda-text-content-types.baseline.r0/transcript.jsonl, t09-lambda-text-content-types.baseline.r1/transcript.jsonl
- **t10-auth-challenge-quoting** `judge_correctness`: baseline 5 vs atlas 4.5 — transcripts: t10-auth-challenge-quoting.atlas.r0/transcript.jsonl, t10-auth-challenge-quoting.atlas.r1/transcript.jsonl, t10-auth-challenge-quoting.baseline.r0/transcript.jsonl, t10-auth-challenge-quoting.baseline.r1/transcript.jsonl
- **t10-auth-challenge-quoting** `total_tokens`: baseline 167750.5 vs atlas 227306.5 — transcripts: t10-auth-challenge-quoting.atlas.r0/transcript.jsonl, t10-auth-challenge-quoting.atlas.r1/transcript.jsonl, t10-auth-challenge-quoting.baseline.r0/transcript.jsonl, t10-auth-challenge-quoting.baseline.r1/transcript.jsonl

## Judge failures

- t01-cwd-relative-targets.baseline.r0: Exited { code: Some(1) }: 
- t04-detector-import-gates.atlas.r0: Exited { code: Some(1) }: 
- t07-static-precompressed-quality.atlas.r0: Exited { code: Some(1) }: 
- t10-auth-challenge-quoting.baseline.r1: Exited { code: Some(1) }: 

