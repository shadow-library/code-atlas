# Performance report (full)

- Date: 2026-10-04
- Suite: `full`
- Platform: macos / aarch64, 14 logical CPUs, rayon threads: 14
- Build profile: release
- Resource usage from: `/usr/bin/time -l` on a child process per scenario group
- Total harness time: 1178 s

## Corpora

| corpus | kind | commit | repos | source files | LOC | skipped |
|---|---|---|---|---|---|---|
| self | self | - | 1 | 564 | 115763 |  |
| fixtures-acme | fixtures | - | 4 | 59 | 1120 |  |
| fixtures-globex | fixtures | - | 2 | 12 | 169 |  |
| synth-10k | synthetic | - | 1 | 68 | 10627 |  |
| synth-100k | synthetic | - | 2 | 583 | 100495 |  |
| synth-100k-5-repos | synthetic | - | 5 | 585 | 101170 |  |
| synth-multi-project | synthetic | - | 6 | 555 | 91264 |  |
| ripgrep | external | 3fce3b5bb023 | 1 | 110 | 56386 |  |
| hono | external | f23b146afcec | 1 | 450 | 91940 |  |
| synth-1m | synthetic | - | 2 | 5511 | 1001293 |  |

## Cold index

Index time is the median of 3 runs (min-max in brackets) of the run ledger duration of `IndexMode::Full` in a fresh process on an empty database; the OS page cache is warm. Peak RSS and CPU are the median for the whole cold child process. CPU utilisation is CPU seconds divided by wall seconds.

| corpus | index | files/s | LOC/s | peak RSS | CPU | CPU/wall | DB | similarity blob | persist sim. | parse failed/partial |
|---|---|---|---|---|---|---|---|---|---|---|
| self | 1074 ms (1002 ms-1113 ms) | 525 | 107743 | 539.2 MB | 3.7 s | 3.1x | 79.0 MB | 8.1 MB | 38.1 ms | 0/2 |
| fixtures-acme | 18.6 ms (18.4 ms-19.8 ms) | 3173 | 60235 | 34.0 MB | 0.1 s | 4.3x | 1.7 MB | 0.1 MB | 0.3 ms | 0/0 |
| fixtures-globex | 4.6 ms (4.3 ms-4.9 ms) | 2615 | 36832 | 22.9 MB | 0.0 s | 2.0x | 0.6 MB | 0.0 MB | 0.1 ms | 0/0 |
| synth-10k | 54.3 ms (53.7 ms-55.6 ms) | 1253 | 195751 | 59.5 MB | 0.3 s | 4.1x | 8.1 MB | 0.5 MB | 6.3 ms | 0/0 |
| synth-100k | 523 ms (486 ms-549 ms) | 1115 | 192127 | 299.7 MB | 2.0 s | 3.3x | 39.3 MB | 4.4 MB | 24.5 ms | 0/0 |
| synth-100k-5-repos | 482 ms (482 ms-495 ms) | 1214 | 209880 | 311.3 MB | 2.0 s | 3.6x | 41.1 MB | 4.5 MB | 17.9 ms | 0/0 |
| ripgrep | 322 ms (311 ms-324 ms) | 341 | 175026 | 207.9 MB | 1.0 s | 2.9x | 29.2 MB | 2.2 MB | 5.8 ms | 0/0 |
| hono | 421 ms (416 ms-427 ms) | 1069 | 218427 | 275.9 MB | 1.7 s | 3.4x | 35.1 MB | 5.1 MB | 20.5 ms | 0/8 |
| synth-1m | 5600 ms (5465 ms-5976 ms) | 984 | 178795 | 2258.9 MB | 19.3 s | 3.2x | 388.3 MB | 42.8 MB | 145 ms | 0/0 |

## Cold index phases

Share is of the summed phase time; the phases are timed sequentially by the run ledger.

### self

| phase | time | share |
|---|---|---|
| persist | 406 ms | 38.8% |
| similarity | 134 ms | 12.8% |
| analyze | 103 ms | 9.8% |
| checkpoint | 94.0 ms | 9.0% |
| dependents | 79.0 ms | 7.6% |
| reindex | 77.0 ms | 7.4% |

### fixtures-acme

| phase | time | share |
|---|---|---|
| persist | 5.0 ms | 38.5% |
| analyze | 3.0 ms | 23.1% |
| passes | 2.0 ms | 15.4% |
| encode | 1.0 ms | 7.7% |
| resolve | 1.0 ms | 7.7% |
| similarity | 1.0 ms | 7.7% |

### fixtures-globex

| phase | time | share |
|---|---|---|
| persist | 1.0 ms | 100.0% |
| analyze | 0.0 ms | 0.0% |
| dependents | 0.0 ms | 0.0% |
| encode | 0.0 ms | 0.0% |
| existence | 0.0 ms | 0.0% |
| finish | 0.0 ms | 0.0% |

### synth-10k

| phase | time | share |
|---|---|---|
| persist | 21.0 ms | 42.9% |
| analyze | 9.0 ms | 18.4% |
| similarity | 8.0 ms | 16.3% |
| encode | 5.0 ms | 10.2% |
| passes | 2.0 ms | 4.1% |
| resolve | 2.0 ms | 4.1% |

### synth-100k

| phase | time | share |
|---|---|---|
| persist | 192 ms | 38.9% |
| similarity | 69.0 ms | 14.0% |
| analyze | 59.0 ms | 12.0% |
| checkpoint | 54.0 ms | 11.0% |
| reindex | 41.0 ms | 8.3% |
| encode | 20.0 ms | 4.1% |

### synth-100k-5-repos

| phase | time | share |
|---|---|---|
| persist | 179 ms | 38.7% |
| similarity | 73.0 ms | 15.8% |
| analyze | 59.0 ms | 12.7% |
| reindex | 35.0 ms | 7.6% |
| checkpoint | 34.0 ms | 7.3% |
| encode | 21.0 ms | 4.5% |

### ripgrep

| phase | time | share |
|---|---|---|
| persist | 151 ms | 48.4% |
| analyze | 48.0 ms | 15.4% |
| similarity | 41.0 ms | 13.1% |
| resolve | 21.0 ms | 6.7% |
| encode | 20.0 ms | 6.4% |
| dependents | 19.0 ms | 6.1% |

### hono

| phase | time | share |
|---|---|---|
| persist | 125 ms | 31.0% |
| similarity | 84.0 ms | 20.8% |
| analyze | 66.0 ms | 16.4% |
| checkpoint | 26.0 ms | 6.5% |
| reindex | 25.0 ms | 6.2% |
| encode | 20.0 ms | 5.0% |

### synth-1m

| phase | time | share |
|---|---|---|
| persist | 3057 ms | 55.6% |
| similarity | 679 ms | 12.3% |
| analyze | 515 ms | 9.4% |
| reindex | 407 ms | 7.4% |
| symbols | 189 ms | 3.4% |
| resolve | 171 ms | 3.1% |

## Warm startup and no-change barrier

`Engine::open` includes rebuilding the symbol table from persisted surfaces. The barrier is `ensure_fresh` with nothing changed, 50 iterations, no watcher (every call is a full stat scan).

| corpus | Engine::open | first barrier | barrier p50 | barrier p95 | barrier max |
|---|---|---|---|---|---|
| self | 242 ms | 7.7 ms | 6.0 ms | 6.1 ms | 6.2 ms |
| fixtures-acme | 8.3 ms | 0.7 ms | 0.4 ms | 0.6 ms | 0.6 ms |
| fixtures-globex | 4.6 ms | 0.5 ms | 0.3 ms | 0.3 ms | 0.4 ms |
| synth-10k | 18.5 ms | 1.9 ms | 1.9 ms | 2.0 ms | 2.1 ms |
| synth-100k | 122 ms | 10.6 ms | 1.5 ms | 2.6 ms | 6.7 ms |
| synth-100k-5-repos | 124 ms | 10.3 ms | 1.4 ms | 10.4 ms | 11.1 ms |
| ripgrep | 77.8 ms | 9.0 ms | 4.0 ms | 4.2 ms | 4.3 ms |
| hono | 126 ms | 13.6 ms | 4.4 ms | 12.9 ms | 13.3 ms |
| synth-1m | 1135 ms | 16.6 ms | 12.4 ms | 14.5 ms | 14.7 ms |

## Incremental updates

Latency is one `ensure_fresh` that detects and applies the change. Entities and edges are the run ledger's added, removed and changed counts. The edges column counts edges rewritten because their evidence content hash changed; it is not semantic churn. Each scenario starts from a settled index, and the note flags any run whose changed-file count differs from the files touched.

| corpus | scenario | latency | files touched | entities changed | edges rewritten (evidence content hash) | renames detected | note |
|---|---|---|---|---|---|---|---|
| self | incremental_1_files | 23.5 ms | 1 | 0 | 0 | 0 |  |
| self | incremental_10_files | 49.7 ms | 10 | 0 | 0 | 0 |  |
| self | incremental_100_files | 221 ms | 100 | 15 | 56 | 0 |  |
| self | incremental_1000_files | - | - | - | - | - | skipped: only 172 editable files in this corpus |
| self | rename_symbol | 904 ms | 318 | 5753 | 34146 | 3 |  |
| self | file_move | 234 ms | 1 | 58 | 227 | 28 |  |
| self | branch_switch | 148 ms | 51 | 105 | 64 | 0 |  |
| self | delete_directory | 323 ms | 45 | 2232 | 8295 | 0 |  |
| fixtures-acme | incremental_1_files | 3.9 ms | 1 | 0 | 0 | 0 |  |
| fixtures-acme | incremental_10_files | 5.3 ms | 10 | 1 | 0 | 0 |  |
| fixtures-acme | incremental_100_files | - | - | - | - | - | skipped: only 11 editable files in this corpus |
| fixtures-acme | incremental_1000_files | - | - | - | - | - | skipped: only 11 editable files in this corpus |
| fixtures-acme | rename_symbol | 4.6 ms | 4 | 13 | 31 | 4 |  |
| fixtures-acme | file_move | 7.3 ms | 1 | 35 | 84 | 7 |  |
| fixtures-acme | branch_switch | 3.9 ms | 3 | 6 | 3 | 0 |  |
| fixtures-acme | delete_directory | 6.6 ms | 5 | 43 | 114 | 0 |  |
| fixtures-globex | incremental_1_files | 1.9 ms | 1 | 1 | 0 | 0 |  |
| fixtures-globex | incremental_10_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | incremental_100_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | incremental_1000_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | rename_symbol | 2.5 ms | 3 | 18 | 41 | 1 |  |
| fixtures-globex | file_move | 2.9 ms | 1 | 21 | 36 | 0 |  |
| fixtures-globex | branch_switch | 1.3 ms | 1 | 3 | 1 | 0 |  |
| fixtures-globex | delete_directory | - | - | - | - | - | skipped: no directory holds between 3 source files and 10% of the corpus |
| synth-10k | incremental_1_files | 5.2 ms | 1 | 0 | 0 | 0 |  |
| synth-10k | incremental_10_files | 11.3 ms | 10 | 0 | 0 | 0 |  |
| synth-10k | incremental_100_files | - | - | - | - | - | skipped: only 49 editable files in this corpus |
| synth-10k | incremental_1000_files | - | - | - | - | - | skipped: only 49 editable files in this corpus |
| synth-10k | rename_symbol | 27.8 ms | 27 | 406 | 1774 | 2 |  |
| synth-10k | file_move | 19.4 ms | 1 | 30 | 146 | 14 |  |
| synth-10k | branch_switch | 17.5 ms | 13 | 26 | 13 | 0 |  |
| synth-10k | delete_directory | - | - | - | - | - | skipped: no directory holds between 3 source files and 10% of the corpus |
| synth-100k | incremental_1_files | 14.7 ms | 1 | 0 | 0 | 0 |  |
| synth-100k | incremental_10_files | 19.6 ms | 10 | 0 | 0 | 0 |  |
| synth-100k | incremental_100_files | 82.9 ms | 100 | 0 | 0 | 0 |  |
| synth-100k | incremental_1000_files | - | - | - | - | - | skipped: only 447 editable files in this corpus |
| synth-100k | rename_symbol | 242 ms | 215 | 3433 | 18237 | 2 |  |
| synth-100k | file_move | 112 ms | 1 | 40 | 188 | 19 |  |
| synth-100k | branch_switch | 126 ms | 134 | 268 | 134 | 0 |  |
| synth-100k | delete_directory | 163 ms | 12 | 154 | 6221 | 0 |  |
| synth-100k-5-repos | incremental_1_files | 17.8 ms | 1 | 0 | 0 | 0 |  |
| synth-100k-5-repos | incremental_10_files | 23.7 ms | 10 | 0 | 0 | 0 |  |
| synth-100k-5-repos | incremental_100_files | 88.0 ms | 100 | 0 | 4 | 0 |  |
| synth-100k-5-repos | incremental_1000_files | - | - | - | - | - | skipped: only 443 editable files in this corpus |
| synth-100k-5-repos | rename_symbol | 129 ms | 102 | 1594 | 7696 | 2 |  |
| synth-100k-5-repos | file_move | 63.5 ms | 1 | 20 | 59 | 3 |  |
| synth-100k-5-repos | branch_switch | 135 ms | 132 | 264 | 135 | 0 |  |
| synth-100k-5-repos | delete_directory | 158 ms | 12 | 157 | 3457 | 0 |  |
| ripgrep | incremental_1_files | 5.4 ms | 1 | 1 | 0 | 0 |  |
| ripgrep | incremental_10_files | 42.6 ms | 10 | 3 | 0 | 0 |  |
| ripgrep | incremental_100_files | - | - | - | - | - | skipped: only 36 editable files in this corpus |
| ripgrep | incremental_1000_files | - | - | - | - | - | skipped: only 36 editable files in this corpus |
| ripgrep | rename_symbol | 261 ms | 44 | 2831 | 12291 | 3 |  |
| ripgrep | file_move | 80.4 ms | 1 | 34 | 89 | 16 |  |
| ripgrep | branch_switch | 41.6 ms | 10 | 23 | 10 | 0 |  |
| ripgrep | delete_directory | 110 ms | 11 | 504 | 2602 | 0 |  |
| hono | incremental_1_files | 8.6 ms | 1 | 0 | 0 | 0 |  |
| hono | incremental_10_files | 16.1 ms | 10 | 4 | 6 | 1 |  |
| hono | incremental_100_files | 146 ms | 100 | 107 | 184 | 6 |  |
| hono | incremental_1000_files | - | - | - | - | - | skipped: only 176 editable files in this corpus |
| hono | rename_symbol | 273 ms | 128 | 1971 | 6590 | 79 |  |
| hono | file_move | 93.5 ms | 1 | 34 | 109 | 16 |  |
| hono | branch_switch | 115 ms | 52 | 127 | 83 | 4 |  |
| hono | delete_directory | 155 ms | 45 | 767 | 2898 | 0 |  |
| synth-1m | incremental_1_files | 117 ms | 1 | 0 | 0 | 0 |  |
| synth-1m | incremental_10_files | 125 ms | 10 | 0 | 0 | 0 |  |
| synth-1m | incremental_100_files | 261 ms | 100 | 0 | 0 | 0 |  |
| synth-1m | incremental_1000_files | 1436 ms | 1000 | 0 | 0 | 0 |  |
| synth-1m | rename_symbol | 2954 ms | 1630 | 26963 | 158277 | 2 |  |
| synth-1m | file_move | 417 ms | 1 | 12 | 38 | 1 |  |
| synth-1m | branch_switch | 1979 ms | 1275 | 2550 | 1275 | 0 |  |
| synth-1m | delete_directory | 1902 ms | 12 | 154 | 32165 | 0 |  |

## Query latency (p50 / p95, ms)

`impact` uses depth 3, `context` a 6000 token budget. 30 deterministic targets (hubs plus a stride sample). With fewer than 20 samples there is no p95: the cell shows the max instead (`api` and `unused` take 3 samples, are abandoned after 60 s, and a kind stops sampling once it has spent 30 s). `unused` is left out of the concurrency mix.

### Including the freshness barrier

Each call runs the barrier first; the second column is the no-change barrier p50.

| corpus | barrier p50 | symbol | callers | impact | search | similar | context | api | unused |
|---|---|---|---|---|---|---|---|---|---|
| self | 6.0 | 6.1 / 6.4 | 6.1 / 9.2 | 7.0 / 11.0 | 6.2 / 6.4 | 7.1 / 9.5 | 11.3 / 21.9 | 8.4 / max 9.2 | 260.9 / max 266.1 |
| fixtures-acme | 0.4 | 0.6 / 0.7 | 0.5 / 0.7 | 0.6 / 0.9 | 0.7 / 0.9 | 0.7 / 1.2 | 1.2 / 2.2 | 0.9 / max 1.9 | 2.1 / max 3.1 |
| fixtures-globex | 0.3 | 0.3 / max 0.3 | 0.3 / max 0.4 | 0.4 / max 0.4 | 0.3 / max 0.5 | 0.4 / max 0.5 | 0.7 / max 0.9 | 0.6 / max 0.6 | 0.9 / max 1.0 |
| synth-10k | 1.9 | 2.0 / 3.3 | 2.0 / 3.2 | 2.3 / 6.3 | 2.2 / 2.4 | 3.0 / 4.0 | 5.0 / 6.6 | 3.0 / max 3.1 | 16.5 / max 16.6 |
| synth-100k | 1.5 | 1.5 / 8.8 | 1.5 / 7.9 | 1.7 / 36.4 | 2.3 / 3.1 | 3.4 / 10.6 | 12.9 / 19.0 | 5.8 / max 6.2 | 82.0 / max 86.4 |
| synth-100k-5-repos | 1.4 | 1.4 / 5.5 | 1.4 / 4.8 | 1.5 / 29.9 | 1.9 / 2.5 | 4.0 / 11.2 | 7.3 / 13.1 | 5.3 / max 5.3 | 56.9 / max 60.0 |
| ripgrep | 4.0 | 4.2 / 4.7 | 4.2 / 4.7 | 4.9 / 7.8 | 4.2 / 4.6 | 2.3 / 3.3 | 7.3 / 15.9 | 2.0 / max 2.5 | 71.4 / max 72.6 |
| hono | 4.4 | 4.1 / 4.5 | 4.1 / 5.4 | 4.5 / 6.6 | 4.2 / 4.4 | 4.9 / 5.8 | 5.8 / 34.0 | 4.5 / max 5.4 | 91.9 / max 93.0 |
| synth-1m | 12.4 | 15.8 / 65.2 | 14.5 / 46.4 | 16.1 / 90.8 | 19.3 / 23.2 | 21.9 / 74.8 | 74.5 / 135.7 | 53.4 / max 63.5 | 1084.7 / max 1379.6 |

### Query alone (barrier skipped)

The same calls through `QueryEngine::without_barrier`, which answers from the current generation without scanning.

| corpus | barrier p50 | symbol | callers | impact | search | similar | context | api | unused |
|---|---|---|---|---|---|---|---|---|---|
| self | 6.0 | 0.1 / 0.4 | 0.2 / 2.6 | 0.9 / 4.0 | 0.3 / 0.4 | 0.8 / 2.9 | 5.0 / 15.0 | 1.9 / max 2.0 | 254.1 / max 255.6 |
| fixtures-acme | 0.4 | 0.0 / 0.1 | 0.0 / 0.1 | 0.1 / 0.3 | 0.1 / 0.3 | 0.3 / 0.5 | 0.6 / 1.6 | 0.3 / max 1.3 | 1.6 / max 2.3 |
| fixtures-globex | 0.3 | 0.0 / max 0.1 | 0.0 / max 0.1 | 0.1 / max 0.2 | 0.1 / max 0.2 | 0.1 / max 0.2 | 0.4 / max 0.6 | 0.2 / max 0.3 | 0.6 / max 0.7 |
| synth-10k | 1.9 | 0.1 / 1.4 | 0.0 / 1.2 | 0.3 / 4.1 | 0.3 / 0.5 | 1.0 / 2.0 | 3.0 / 4.5 | 1.0 / max 1.0 | 13.9 / max 14.4 |
| synth-100k | 1.5 | 0.1 / 6.9 | 0.0 / 6.3 | 0.2 / 34.1 | 0.8 / 1.5 | 1.8 / 8.8 | 11.1 / 17.2 | 3.2 / max 4.4 | 80.1 / max 81.1 |
| synth-100k-5-repos | 1.4 | 0.1 / 3.8 | 0.1 / 3.4 | 0.3 / 27.3 | 0.7 / 1.0 | 2.4 / 9.8 | 6.0 / 11.7 | 3.7 / max 3.7 | 55.0 / max 59.1 |
| ripgrep | 4.0 | 0.1 / 0.5 | 0.1 / 0.5 | 0.7 / 3.4 | 0.3 / 0.4 | 0.6 / 1.5 | 5.3 / 13.9 | 0.0 / max 0.0 | 70.5 / max 71.6 |
| hono | 4.4 | 0.1 / 0.3 | 0.1 / 0.9 | 0.4 / 2.4 | 0.2 / 0.3 | 0.8 / 1.6 | 1.6 / 28.7 | 0.2 / max 0.2 | 87.4 / max 87.7 |
| synth-1m | 12.4 | 0.2 / 45.2 | 0.1 / 31.8 | 1.3 / 68.2 | 5.1 / 7.7 | 8.4 / 60.1 | 59.3 / 118.6 | 37.2 / max 49.1 | 1055.2 / max 1360.4 |

## Concurrency

Mixed query workload against one engine in 5 s windows, repeated 3 times; throughput in queries per second and p95 latency per query, median of the 3 windows. The edit-stream variant runs last, after the incremental scenarios.

### Read-only

| corpus | N=1 | N=4 | N=8 | N=16 |
|---|---|---|---|---|
| self | 133 qps / 12.3 ms | 314 qps / 17.9 ms | 594 qps / 18.6 ms | 1117 qps / 19.2 ms |
| fixtures-acme | 1433 qps / 1.7 ms | 3230 qps / 2.2 ms | 6022 qps / 2.3 ms | 11061 qps / 2.5 ms |
| fixtures-globex | 2824 qps / 0.7 ms | 6740 qps / 0.9 ms | 10607 qps / 1.1 ms | 19185 qps / 1.2 ms |
| synth-10k | 855 qps / 3.8 ms | 2682 qps / 4.4 ms | 4711 qps / 4.8 ms | 8256 qps / 5.2 ms |
| synth-100k | 210 qps / 17.9 ms | 659 qps / 20.8 ms | 1161 qps / 21.8 ms | 1958 qps / 24.3 ms |
| synth-100k-5-repos | 264 qps / 12.9 ms | 807 qps / 14.7 ms | 1471 qps / 15.9 ms | 2399 qps / 17.8 ms |
| ripgrep | 357 qps / 8.4 ms | 941 qps / 9.9 ms | 1749 qps / 10.3 ms | 3227 qps / 10.8 ms |
| hono | 207 qps / 6.9 ms | 473 qps / 10.2 ms | 891 qps / 10.9 ms | 1695 qps / 11.0 ms |
| synth-1m | 33 qps / 93.1 ms | 89 qps / 111.0 ms | 150 qps / 132.0 ms | 225 qps / 187.9 ms |

### With a concurrent edit stream

| corpus | N=1 | N=4 | N=8 | N=16 |
|---|---|---|---|---|
| self | 115 qps / 19.5 ms | 260 qps / 28.7 ms | 450 qps / 34.1 ms | 775 qps / 40.9 ms |
| fixtures-acme | 1295 qps / 1.5 ms | 2872 qps / 2.2 ms | 5330 qps / 2.4 ms | 9957 qps / 2.4 ms |
| fixtures-globex | 2202 qps / 0.8 ms | 4586 qps / 1.2 ms | 8651 qps / 1.2 ms | 15901 qps / 1.3 ms |
| synth-10k | 591 qps / 4.9 ms | 1669 qps / 5.8 ms | 3079 qps / 6.1 ms | 5656 qps / 6.5 ms |
| synth-100k | 228 qps / 15.6 ms | 583 qps / 19.0 ms | 950 qps / 23.0 ms | 1277 qps / 61.8 ms |
| synth-100k-5-repos | 220 qps / 18.0 ms | 562 qps / 21.9 ms | 964 qps / 27.4 ms | 1118 qps / 66.6 ms |
| ripgrep | 271 qps / 10.6 ms | 671 qps / 15.0 ms | 1209 qps / 19.0 ms | 2099 qps / 23.8 ms |
| hono | 185 qps / 8.9 ms | 387 qps / 16.4 ms | 689 qps / 18.5 ms | 1195 qps / 42.4 ms |
| synth-1m | 8 qps / 158.9 ms | 10 qps / 973.9 ms | 17 qps / 690.9 ms | 26 qps / 703.8 ms |

## Multi-project

| corpus | projects | total index time | distinct databases | cross-talk violations |
|---|---|---|---|---|
| synth-multi-project | 3 | 527 ms | 3 | 0 |

## Skipped and failed scenarios

| corpus | scenario | status | reason |
|---|---|---|---|
| self | incremental_1000_files | skipped | only 172 editable files in this corpus |
| fixtures-acme | incremental_100_files | skipped | only 11 editable files in this corpus |
| fixtures-acme | incremental_1000_files | skipped | only 11 editable files in this corpus |
| fixtures-globex | incremental_10_files | skipped | only 5 editable files in this corpus |
| fixtures-globex | incremental_100_files | skipped | only 5 editable files in this corpus |
| fixtures-globex | incremental_1000_files | skipped | only 5 editable files in this corpus |
| fixtures-globex | delete_directory | skipped | no directory holds between 3 source files and 10% of the corpus |
| synth-10k | incremental_100_files | skipped | only 49 editable files in this corpus |
| synth-10k | incremental_1000_files | skipped | only 49 editable files in this corpus |
| synth-10k | delete_directory | skipped | no directory holds between 3 source files and 10% of the corpus |
| synth-100k | incremental_1000_files | skipped | only 447 editable files in this corpus |
| synth-100k-5-repos | incremental_1000_files | skipped | only 443 editable files in this corpus |
| ripgrep | incremental_100_files | skipped | only 36 editable files in this corpus |
| ripgrep | incremental_1000_files | skipped | only 36 editable files in this corpus |
| hono | incremental_1000_files | skipped | only 176 editable files in this corpus |

## Profiling

- Tool: none
- profiling was not requested; the per-phase timings from the run ledger are the profile

## Limits of these numbers

- Synthetic corpora are a relative scale test, not a realistic codebase: bodies are uniform arithmetic-shaped code, names come from a fixed word list, 4% of functions are planted near-duplicates and files average about 175 LOC. The synth-1m row in particular is not representative of a real 1M LOC codebase. ripgrep and hono are the realistic data points.
- "Cold" means a fresh process and an empty database, but a warm OS page cache.
- The session worker's peak RSS mixes warm-open, query and edit workloads.
- Cold index and each concurrency level are medians of 3 runs; min-max is stored in each record's `spread` info.

