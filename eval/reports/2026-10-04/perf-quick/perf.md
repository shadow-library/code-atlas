# Performance report (quick)

- Date: 2026-10-04
- Suite: `quick`
- Platform: macos / aarch64, 14 logical CPUs, rayon threads: 14
- Build profile: release
- Resource usage from: `/usr/bin/time -l` on a child process per scenario group
- Total harness time: 501 s

## Corpora

| corpus | kind | commit | repos | source files | LOC | skipped |
|---|---|---|---|---|---|---|
| self | self | - | 1 | 563 | 115739 |  |
| fixtures-acme | fixtures | - | 4 | 58 | 1091 |  |
| fixtures-globex | fixtures | - | 2 | 12 | 169 |  |
| synth-100k | synthetic | - | 2 | 584 | 100811 |  |
| synth-multi-project | synthetic | - | 6 | 555 | 91264 |  |

## Cold index

Index time is the median of 3 runs (min-max in brackets) of the run ledger duration of `IndexMode::Full` in a fresh process on an empty database; the OS page cache is warm. Peak RSS and CPU are the median for the whole cold child process. CPU utilisation is CPU seconds divided by wall seconds.

| corpus | index | files/s | LOC/s | peak RSS | CPU | CPU/wall | DB | similarity blob | persist sim. | parse failed/partial |
|---|---|---|---|---|---|---|---|---|---|---|
| self | 1233 ms (1185 ms-1261 ms) | 457 | 93855 | 545.1 MB | 3.6 s | 2.7x | 79.0 MB | 8.1 MB | 41.4 ms | 0/2 |
| fixtures-acme | 18.4 ms (18.0 ms-18.5 ms) | 3158 | 59407 | 34.1 MB | 0.1 s | 4.3x | 1.6 MB | 0.1 MB | 0.2 ms | 0/0 |
| fixtures-globex | 4.4 ms (4.3 ms-4.5 ms) | 2724 | 38366 | 22.7 MB | 0.0 s | 2.0x | 0.6 MB | 0.0 MB | 0.1 ms | 0/0 |
| synth-100k | 465 ms (456 ms-469 ms) | 1255 | 216637 | 295.1 MB | 2.0 s | 3.7x | 39.3 MB | 4.4 MB | 19.1 ms | 0/0 |

## Cold index phases

Share is of the summed phase time; the phases are timed sequentially by the run ledger.

### self

| phase | time | share |
|---|---|---|
| persist | 465 ms | 38.8% |
| similarity | 154 ms | 12.9% |
| analyze | 121 ms | 10.1% |
| dependents | 100 ms | 8.3% |
| reindex | 89.0 ms | 7.4% |
| checkpoint | 83.0 ms | 6.9% |

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

### synth-100k

| phase | time | share |
|---|---|---|
| persist | 174 ms | 39.5% |
| similarity | 69.0 ms | 15.6% |
| analyze | 55.0 ms | 12.5% |
| reindex | 35.0 ms | 7.9% |
| checkpoint | 29.0 ms | 6.6% |
| encode | 21.0 ms | 4.8% |

## Warm startup and no-change barrier

`Engine::open` includes rebuilding the symbol table from persisted surfaces. The barrier is `ensure_fresh` with nothing changed, 50 iterations, no watcher (every call is a full stat scan).

| corpus | Engine::open | first barrier | barrier p50 | barrier p95 | barrier max |
|---|---|---|---|---|---|
| self | 282 ms | 8.3 ms | 7.5 ms | 8.0 ms | 8.3 ms |
| fixtures-acme | 8.3 ms | 0.5 ms | 0.4 ms | 0.6 ms | 0.6 ms |
| fixtures-globex | 4.5 ms | 0.4 ms | 0.3 ms | 0.3 ms | 0.3 ms |
| synth-100k | 120 ms | 11.4 ms | 1.8 ms | 10.3 ms | 10.4 ms |

## Incremental updates

Latency is one `ensure_fresh` that detects and applies the change. Entities and edges are the run ledger's added, removed and changed counts. The edges column counts edges rewritten because their evidence content hash changed; it is not semantic churn. Each scenario starts from a settled index, and the note flags any run whose changed-file count differs from the files touched.

| corpus | scenario | latency | files touched | entities changed | edges rewritten (evidence content hash) | renames detected | note |
|---|---|---|---|---|---|---|---|
| self | incremental_1_files | 20.8 ms | 1 | 0 | 0 | 0 |  |
| self | incremental_10_files | 44.3 ms | 10 | 0 | 0 | 0 |  |
| self | incremental_100_files | 215 ms | 100 | 15 | 56 | 0 |  |
| self | incremental_1000_files | - | - | - | - | - | skipped: only 172 editable files in this corpus |
| self | rename_symbol | 889 ms | 318 | 5753 | 34146 | 3 |  |
| self | file_move | 150 ms | 1 | 16 | 234 | 7 |  |
| self | branch_switch | 146 ms | 51 | 105 | 64 | 0 |  |
| self | delete_directory | 313 ms | 45 | 2232 | 8294 | 0 |  |
| fixtures-acme | incremental_1_files | 3.9 ms | 1 | 0 | 0 | 0 |  |
| fixtures-acme | incremental_10_files | 5.1 ms | 10 | 1 | 0 | 0 |  |
| fixtures-acme | incremental_100_files | - | - | - | - | - | skipped: only 11 editable files in this corpus |
| fixtures-acme | incremental_1000_files | - | - | - | - | - | skipped: only 11 editable files in this corpus |
| fixtures-acme | rename_symbol | 4.6 ms | 4 | 13 | 31 | 4 |  |
| fixtures-acme | file_move | 6.2 ms | 1 | 14 | 23 | 6 |  |
| fixtures-acme | branch_switch | 4.3 ms | 3 | 6 | 3 | 0 |  |
| fixtures-acme | delete_directory | 6.4 ms | 5 | 43 | 108 | 0 |  |
| fixtures-globex | incremental_1_files | 1.9 ms | 1 | 1 | 0 | 0 |  |
| fixtures-globex | incremental_10_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | incremental_100_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | incremental_1000_files | - | - | - | - | - | skipped: only 5 editable files in this corpus |
| fixtures-globex | rename_symbol | 2.3 ms | 3 | 23 | 31 | 1 |  |
| fixtures-globex | file_move | 2.4 ms | 1 | 11 | 24 | 0 | FILE COUNT MISMATCH: run changed 1 |
| fixtures-globex | branch_switch | 1.3 ms | 1 | 3 | 1 | 0 |  |
| fixtures-globex | delete_directory | - | - | - | - | - | skipped: no directory holds between 3 source files and 10% of the corpus |
| synth-100k | incremental_1_files | 15.3 ms | 1 | 0 | 0 | 0 |  |
| synth-100k | incremental_10_files | 22.4 ms | 10 | 0 | 0 | 0 |  |
| synth-100k | incremental_100_files | 87.2 ms | 100 | 0 | 0 | 0 |  |
| synth-100k | incremental_1000_files | - | - | - | - | - | skipped: only 448 editable files in this corpus |
| synth-100k | rename_symbol | 258 ms | 215 | 3433 | 18237 | 2 |  |
| synth-100k | file_move | 118 ms | 1 | 38 | 159 | 18 |  |
| synth-100k | branch_switch | 134 ms | 134 | 268 | 134 | 0 |  |
| synth-100k | delete_directory | 134 ms | 13 | 179 | 1003 | 0 |  |

## Query latency (p50 / p95, ms)

`impact` uses depth 3, `context` a 6000 token budget. 30 deterministic targets (hubs plus a stride sample). With fewer than 20 samples there is no p95: the cell shows the max instead (`api` and `unused` take 3 samples, are abandoned after 60 s, and a kind stops sampling once it has spent 30 s). `unused` is left out of the concurrency mix.

### Including the freshness barrier

Each call runs the barrier first; the second column is the no-change barrier p50.

| corpus | barrier p50 | symbol | callers | impact | search | similar | context | api | unused |
|---|---|---|---|---|---|---|---|---|---|
| self | 7.5 | 7.8 / 9.3 | 8.9 / 11.6 | 9.4 / 13.0 | 8.5 / 8.8 | 9.2 / 11.8 | 14.7 / 27.6 | 11.2 / max 12.3 | 315.2 / max 316.5 |
| fixtures-acme | 0.4 | 0.5 / 0.7 | 0.5 / 0.7 | 0.6 / 0.9 | 0.6 / 0.8 | 0.7 / 1.0 | 1.2 / 2.3 | 1.0 / max 1.9 | 2.0 / max 2.8 |
| fixtures-globex | 0.3 | 0.3 / max 0.4 | 0.3 / max 0.3 | 0.4 / max 0.5 | 0.3 / max 0.5 | 0.4 / max 0.5 | 0.7 / max 0.9 | 0.5 / max 0.6 | 0.9 / max 1.0 |
| synth-100k | 1.8 | 1.5 / 9.1 | 1.6 / 7.8 | 1.8 / 36.6 | 2.2 / 2.9 | 3.6 / 10.5 | 12.8 / 20.2 | 5.6 / max 6.0 | 83.1 / max 86.7 |

### Query alone (barrier skipped)

The same calls through `QueryEngine::without_barrier`, which answers from the current generation without scanning.

| corpus | barrier p50 | symbol | callers | impact | search | similar | context | api | unused |
|---|---|---|---|---|---|---|---|---|---|
| self | 7.5 | 0.1 / 0.4 | 0.2 / 3.4 | 1.0 / 5.0 | 0.3 / 0.5 | 1.1 / 3.5 | 6.8 / 20.5 | 2.2 / max 2.5 | 305.8 / max 306.8 |
| fixtures-acme | 0.4 | 0.0 / 0.1 | 0.0 / 0.1 | 0.1 / 0.3 | 0.1 / 0.3 | 0.2 / 0.4 | 0.6 / 1.6 | 0.3 / max 1.2 | 1.5 / max 2.3 |
| fixtures-globex | 0.3 | 0.0 / max 0.1 | 0.0 / max 0.1 | 0.1 / max 0.2 | 0.1 / max 0.2 | 0.1 / max 0.2 | 0.4 / max 0.6 | 0.3 / max 0.3 | 0.6 / max 0.6 |
| synth-100k | 1.8 | 0.1 / 6.9 | 0.0 / 6.3 | 0.3 / 34.1 | 0.7 / 1.6 | 1.7 / 9.1 | 10.9 / 17.8 | 3.3 / max 4.3 | 80.8 / max 81.4 |

## Concurrency

Mixed query workload against one engine in 5 s windows, repeated 3 times; throughput in queries per second and p95 latency per query, median of the 3 windows. The edit-stream variant runs last, after the incremental scenarios.

### Read-only

| corpus | N=1 | N=4 | N=8 | N=16 |
|---|---|---|---|---|
| self | 132 qps / 12.7 ms | 322 qps / 17.7 ms | 609 qps / 18.3 ms | 1148 qps / 18.8 ms |
| fixtures-acme | 1451 qps / 1.6 ms | 3307 qps / 2.2 ms | 6163 qps / 2.4 ms | 11336 qps / 2.5 ms |
| fixtures-globex | 2968 qps / 0.7 ms | 6827 qps / 0.9 ms | 10718 qps / 1.1 ms | 19300 qps / 1.2 ms |
| synth-100k | 210 qps / 19.0 ms | 656 qps / 21.3 ms | 1126 qps / 23.0 ms | 1850 qps / 25.7 ms |

### With a concurrent edit stream

| corpus | N=1 | N=4 | N=8 | N=16 |
|---|---|---|---|---|
| self | 117 qps / 19.4 ms | 226 qps / 33.9 ms | 446 qps / 34.5 ms | 791 qps / 40.0 ms |
| fixtures-acme | 1255 qps / 1.7 ms | 2854 qps / 2.4 ms | 5289 qps / 2.5 ms | 9746 qps / 2.7 ms |
| fixtures-globex | 2266 qps / 0.7 ms | 4645 qps / 1.1 ms | 8742 qps / 1.2 ms | 15936 qps / 1.3 ms |
| synth-100k | 162 qps / 22.1 ms | 403 qps / 38.8 ms | 652 qps / 60.8 ms | 798 qps / 68.7 ms |

## Multi-project

| corpus | projects | total index time | distinct databases | cross-talk violations |
|---|---|---|---|---|
| synth-multi-project | 3 | 546 ms | 3 | 0 |

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
| synth-100k | incremental_1000_files | skipped | only 448 editable files in this corpus |

## Profiling

- Tool: none
- profiling was not requested; the per-phase timings from the run ledger are the profile

## Limits of these numbers

- Synthetic corpora are a relative scale test, not a realistic codebase: bodies are uniform arithmetic-shaped code, names come from a fixed word list, 4% of functions are planted near-duplicates and files average about 175 LOC. The synth-1m row in particular is not representative of a real 1M LOC codebase. ripgrep and hono are the realistic data points.
- "Cold" means a fresh process and an empty database, but a warm OS page cache.
- The session worker's peak RSS mixes warm-open, query and edit workloads.
- Cold index and each concurrency level are medians of 3 runs; min-max is stored in each record's `spread` info.

