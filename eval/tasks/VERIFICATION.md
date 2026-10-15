# Hidden-test verification

Every hidden test was run in scratch copies, never in a worktree:

1. **Fail at base.** Build the harness tree (archive at the pinned commit, `base.patch` if any, strip plan, `git init` plus the `eval-base` tag), copy the hidden tests in, and run `[validation].commands`. They must fail.
2. **Pass with the reference.** Build a fresh tree the same way, apply `reference.patch` (on top of `base.patch` where one exists), copy the hidden tests in, and run the validation and regression commands. They must pass.
3. **Regression at base.** Run `[validation].regression` on the base tree, with the hidden tests in place, exactly as the harness's base check does. It must pass, except for failures the task plants on purpose.

**Self trees** reproduce the harness materialization for `repo = "self"`:
- `git archive 327f4dc`, `git apply` of `base.patch`;
- removal of `eval/`, `scratch-pad/`, `hidden/`, `docs/plan/` and the hidden destinations;
- `git init` and the `eval-base` tag.

They keep `fixtures/**/expected.toml`, which this repository's own tests read. Like the harness, every tree gets its own `CARGO_TARGET_DIR` outside the tree (`<tree>.target`).

**Why the target directory is per tree:** a target directory shared between archived trees gives false results. `git archive` stamps every file with the commit's mtime, so cargo treats another tree's sources as up to date and reuses binaries built from a different tree.

**Every `cargo test` command uses `--no-fail-fast`.** Without it cargo stops at the first failing test target, which undercounts regressions and favours agents that break more; the t03 base check, for example, counted 37 atlas-link tests instead of 45. **Regression commands never include a hidden test target.** They list test targets explicitly where a package has a hidden test, so the base check measures only existing tests.

Times are wall-clock on a 14-core Apple Silicon machine with two trees building in parallel; the first command of each tree is a cold build (empty target dir, warm `~/.cargo` registry). Last re-run on 2026-10-04, after the harness stopped stripping fixture `expected.toml` from self trees, moved `CARGO_TARGET_DIR` out of the tree, and added `--no-fail-fast` to every command.

## Self tasks (this repository @ 327f4dc99bfcab783d882d8fa39f5b7eb2bbb22f)

### t01-cwd-relative-targets (find and extend, reuse)
| tree | command | result |
|---|---|---|
| base | `cargo test --no-fail-fast -p atlas-cli --test cwd_targets` | **exit 101**, 1 passed / 3 failed (79 s cold). The failing tests are `should_resolve_file_targets_relative_to_the_working_directory_in_every_target_query`, `should_resolve_paths_relative_to_a_directory_above_the_repositories_and_absolute_paths` and `should_resolve_a_whole_file_target_relative_to_the_working_directory`; the passing one is the no-regression guard. |
| base | regression `cargo test --no-fail-fast -p atlas-cli --bins --test cli` | exit 0, 40 passed (13 s) |
| reference | validation | exit 0, 4 passed (47 s cold) |
| reference | regression | exit 0, 40 passed (14 s) |

Reference (`crates/atlas-cli/src/commands/query.rs` only): fmt and `cargo clippy -p atlas-cli --all-targets -- -D warnings` clean.

### t02-typed-degraded-cause (cross-crate blast radius)
| tree | command | result |
|---|---|---|
| base | `cargo test --no-fail-fast -p atlas-engine --test degraded_cause` | **exit 101, compile failure**: E0432 unresolved import `atlas_engine::DegradedCause`, and E0026 `Degraded` has no field `cause` (26 s cold) |
| base | `cargo test --no-fail-fast -p atlas-query --test degraded_freshness` | **exit 101, compile failure**: E0432, E0559 (4 s) |
| base | `cargo test --no-fail-fast -p atlas-cli --test cli should_reload_the_project_when_another_process_changes_it_while_watching` | exit 0, 1 passed. Behaviour guard: watch must still reload. |
| base | regression `cargo test --no-fail-fast -p atlas-engine --lib --test engine --test regressions` | exit 0, 42 passed / 1 ignored (21 s) |
| base | regression `cargo test --no-fail-fast -p atlas-query --lib --test acme --test freshness --test snapshots --test unused` | exit 0, 35 passed (11 s). This now includes the fixture-truth targets. |
| base | regression `cargo test --no-fail-fast -p atlas-cli --bins --test cli` | exit 0, 40 passed (5 s) |
| reference | validation | exit 0: 4, 1 and 1 passed |
| reference | regression | exit 0: 42 / 1 ignored, 35, 40 passed |

Reference (engine `report.rs`, `engine.rs`, `tests/engine.rs`, `tests/regressions.rs`; query `freshness.rs`; cli `commands/watch.rs`; `CLAUDE.md`): fmt and clippy clean.

### t03-nested-router-links (planted bug, dependency trace)
The planted bug lives in `base.patch`. In `crates/atlas-detect/src/detectors/express.rs`, `emit_cross_file_mount` records `("mounted_on", json!(child))` where it should record `json!(parent)`, a mix-up between two `&str` parameters one token apart.
- **Symptom:** it appears a crate away, in `crates/atlas-link/src/compose.rs::express_chains`. A `router_relative` mount looks up its parent router by that name. The child is only an import in that file, so the lookup fails and the chain is dropped without any error.
- **Effect:** routes of routers nested two or more files deep get no composed endpoint, and client calls to them get no `calls_endpoint` link.

| tree | command | result |
|---|---|---|
| base (+ base.patch) | `cargo test --no-fail-fast -p atlas-link --test nested_router_links` | **exit 101**, 1 passed / 2 failed (23 s cold). Failing: `should_compose_routes_of_routers_mounted_two_files_deep` and `should_link_client_calls_to_routes_of_nested_routers`; the control `should_keep_linking_routers_mounted_directly_on_the_app` passes. |
| base (+ base.patch) | `cargo test --no-fail-fast -p atlas-detect --test cross_file_mounts` | **exit 101**, 0 passed / 2 failed. `mounted_on` is `ordersRouter` instead of `apiRouter`, and `apiRouter` instead of `app`. This pins the fix to the detector: a downstream workaround in atlas-link still fails here. |
| base (+ base.patch) | regression `cargo test --no-fail-fast -p atlas-detect --lib --test auth --test clients --test config_env --test coverage --test endpoints --test experimental --test false_positives --test registry --test test_coverage` | exit 0, 84 passed (12 s) |
| base (+ base.patch) | regression `cargo test --no-fail-fast -p atlas-link --lib --test fixtures --test incremental --test synthetic --test base_evidence` | **exit 101**, 44 passed / 1 failed: `fixtures::should_compose_mounted_express_routes_and_match_clients_against_them`. This is expected: the planted bug breaks that fixture-truth test now that self trees keep `expected.toml`. The harness counts it as already failing at base, not as a regression. |
| reference (base.patch + reference.patch) | validation | exit 0: 3 passed (19 s cold); 2 passed |
| reference | regression | exit 0: detect 85 passed (one new reference test in `tests/endpoints.rs`), link 45 passed |
| unpatched 327f4dc (sanity) | both hidden tests | 3 passed; 2 passed |

Now that self trees keep `expected.toml`, one existing atlas-link test (the `computed-routes-ts` fixture) fails on the patched base. It points at the symptom in atlas-link, but the agent must still trace it to the detector in atlas-detect, which is the root cause the prompt asks to fix.

### t04-detector-import-gates (refactor)
| tree | command | result |
|---|---|---|
| base | `cargo test --no-fail-fast -p atlas-detect --test import_gates` | exit 0, 7 passed (12 s cold). It pins today's behaviour on purpose. |
| base | `! grep -rn "ImportFact" crates/atlas-detect/src/detectors` | **exit 1**, with matches in `nest.rs`, `axum.rs`, `express.rs` and `clients.rs`. The task fails at base through this check. |
| base | regression (atlas-detect lib and every existing test target, including `coverage`) | exit 0, 84 passed |
| reference | validation | exit 0: 7 passed; grep check exit 0 |
| reference | regression | exit 0, 84 passed |

**What the hidden test pins:** the gates of express, nest, http-clients and axum on 12 TS and 10 Rust sources, plus the parsed-file callers of the `FileView` helpers. That includes the deliberate quirks: nest and clients ignore `require()`; axum and clients ignore `extern crate`.

**Mutation check:** widening the nest gate to accept `require()` makes `should_gate_the_nest_detector_on_esm_imports_from_the_nestjs_scope_only` fail.

### t05-mcp-dependency-tools (architecture-consistent addition, test discovery)
| tree | command | result |
|---|---|---|
| base | `cargo test --no-fail-fast -p atlas-mcp --test dependency_tools` | **exit 101**, 0 passed / 4 failed (35 s cold). The tools are not advertised, and calls to them fail. |
| base | `cargo test --no-fail-fast -p atlas-mcp --test protocol` | exit 0, 23 passed |
| base | regression `cargo test --no-fail-fast -p atlas-mcp --lib`; `cargo test --no-fail-fast -p atlas-cli` | exit 0: 3 passed; 40 passed (20 s) |
| reference | validation | exit 0: 4 passed (33 s cold); 23 passed |
| reference | regression | exit 0: 3 passed; 40 passed |
| reference without its `protocol.rs` update | `cargo test --no-fail-fast -p atlas-mcp --test protocol` | **exit 101**, 22 passed / 1 failed (`should_list_every_tool_with_a_schema_and_an_agent_facing_description`). A solution must find and update the existing tool-list test. |

Reference (`crates/atlas-mcp/src/{server,descriptions}.rs`, `crates/atlas-mcp/tests/protocol.rs`): fmt and `cargo clippy -p atlas-mcp --all-targets -- -D warnings` clean.

### t06-repo-file-size-limit (config impact, blast radius)
| tree | command | result |
|---|---|---|
| base | `cargo test --no-fail-fast -p atlas-engine --test repo_file_limit` | **exit 101**, 0 passed / 3 failed (29 s cold). Each test fails with `RegistryParse ... unknown field max_file_bytes`. |
| base | regression `cargo test --no-fail-fast -p atlas-project --lib` | exit 0, 111 passed (5 s) |
| base | regression `cargo test --no-fail-fast -p atlas-engine --lib --test engine --test incremental_equivalence --test regressions --test scip` | exit 0, 46 passed / 1 ignored (44 s) |
| base | regression `cargo test --no-fail-fast -p atlas-resolve -p atlas-query -p atlas-link` | exit 0, 175 passed / 2 ignored (8 s, 43 s cold). Includes the fixture-truth targets, and these crates hold most of the `RepoConfig` literals. |
| base | regression `cargo test --workspace --no-run` | exit 0 (32 s cold) |
| reference | validation | exit 0, 3 passed (33 s cold) |
| reference | regression | exit 0: 112 (one new discovery unit test); 46 / 1 ignored; 175 / 2 ignored; workspace builds |

**What the hidden test covers:**
- full index: a repo limit above the project default admits a file, a limit below it skips one, and a repo without a limit keeps the default;
- incremental: an edited file and a new file still follow the repo limit;
- registry: saving does not add the key to repos that don't set it.

Reference: fmt and `cargo clippy -p atlas-project -p atlas-engine --all-targets -- -D warnings` clean.

## External tasks: honojs/hono @ f23b146afcec63606144cde50b5fbd360dd60238

**Repository.** hono is a TypeScript pnpm workspace on v4.13.12 (main at pinning time). It holds the core `hono` package and the `adapters/*` packages (`@hono/aws-lambda`, `@hono/lambda-edge`, ...), which are near-copies of the deprecated `src/adapter/*`. Tests run with vitest on node. The repository has no root `eval/`, `scratch-pad/` or `hidden/` directory, so the strip plan removes nothing that belongs to it.

**Procedure for each task,** in scratch copies:
1. `git archive f23b146 | tar -x` into a fresh directory, then run the task's `setup`.
2. **Base:** copy the hidden tests to their destinations, asserting that no destination existed before. Run `[validation].commands`; at least one must fail. Run `[validation].regression`; it must pass.
3. **Reference:** build a fresh tree, run `setup`, `git apply reference.patch`, copy the hidden tests in, and run the commands and the regression. All must pass.

**Setup step:** `npx --yes pnpm@12.6.0 install --frozen-lockfile --prefer-offline`.

| install | time |
|---|---|
| cold first install (network) | 39.3 s |
| warm install in a fresh tree | 0.9–1.7 s |
| fully offline in a fresh tree (`npx --offline pnpm@12.6.0 install --frozen-lockfile --offline`) | 0.8 s; the tests ran afterwards |

After one networked install the pnpm store and the npx cache hold everything, and setup plus validation run offline. The corepack `pnpm` shim on PATH (`~/.local/bin/pnpm`) cannot run pnpm 12.6.0 on this machine, so the prompts carry an environment note pointing to `npx pnpm@12.6.0` and `./node_modules/.bin`.

**Adapter packages.** The adapter packages import `hono` from `dist/`, so the t08 and t09 commands run `./node_modules/.bin/vp pack` (3–7 s) first. `dist/` is gitignored, so the build never appears in the diff.

**vitest filters.** `--exclude` is ignored under `--project main`, so the t07 regression lists files explicitly. `--exclude` does work inside the adapter packages, which have no projects.

In the tables below, `$V` stands for `./node_modules/.bin/vitest run --project main --coverage.enabled=false`.

### t07-static-precompressed-quality (reuse trap)
| tree | command | result |
|---|---|---|
| base | `$V src/middleware/serve-static/precompressed-negotiation.test.ts` | **exit 1**, 6 failed / 2 passed |
| base | regression `$V src/middleware/serve-static/index.test.ts src/middleware/serve-static/path.test.ts src/middleware/compress src/utils src/adapter src/helper/accepts src/middleware/language` | exit 0, 730 passed / 3 skipped (1.6 s) |
| base | regression `./node_modules/.bin/tsc -p tsconfig.spec.json` | exit 0 (4.4 s) |
| reference | hidden test | exit 0, 8 passed |
| reference | regression vitest / tsc | exit 0, 730 passed / 3 skipped (1.2 s); tsc exit 0 (4.2 s) |

### t08-cookie-samesite-default (test discovery, cross-package)
| tree | command | result |
|---|---|---|
| base | `$V src/helper/cookie/same-site-default.test.ts` | **exit 1**, 4 failed / 2 passed |
| base | `$V src/helper/cookie/index.test.ts src/context.test.ts src/adapter/aws-lambda src/adapter/lambda-edge` (existing tests) | exit 0, 185 passed |
| base | `./node_modules/.bin/vp pack > /dev/null && cd adapters/aws-lambda && ../../node_modules/.bin/vitest run && cd ../lambda-edge && ../../node_modules/.bin/vitest run` | exit 0, 88 + 57 passed (3.8 s) |
| base | regression `$V src/utils/cookie.test.ts src/middleware src/adapter src/hono.test.ts src/context.test.ts src/helper/cookie/index.test.ts` | exit 0, 1247 passed / 1 skipped (5.3 s) |
| base | regression tsc | exit 0 (4.2 s) |
| reference | the three validation commands | exit 0: 6 passed; 185 passed; 88 + 57 passed (5.6 s) |
| reference | regression vitest / tsc | exit 0, 1247 passed / 1 skipped (5.2 s); tsc exit 0 (6.7 s) |
| source change only (no test updates) | hidden / existing core tests / adapter packages | 6 passed / **exit 1**, 21 of 185 failed / **exit 1**, 6 of 88 failed. A solution that does not find and update the existing tests fails validation. |

### t09-lambda-text-content-types (cross-package consistency)
| tree | command | result |
|---|---|---|
| base | `$V src/adapter/aws-lambda/text-content-types.test.ts src/adapter/lambda-edge/text-content-types.test.ts` | **exit 1**, 18 failed / 20 passed |
| base | `./node_modules/.bin/vp pack > /dev/null && cd adapters/aws-lambda && ../../node_modules/.bin/vitest run src/text-content-types.test.ts && cd ../lambda-edge && ../../node_modules/.bin/vitest run src/text-content-types.test.ts` | **exit 1**, 9 failed / 10 passed |
| base | regression `$V src/adapter/aws-lambda/handler.test.ts src/adapter/aws-lambda/conninfo.test.ts src/adapter/lambda-edge/handler.test.ts src/adapter/lambda-edge/conninfo.test.ts` | exit 0, 93 passed (1.6 s) |
| base | regression: `vp pack`, then `vitest run --exclude src/text-content-types.test.ts` in `adapters/aws-lambda` and `adapters/lambda-edge` | exit 0, 88 + 57 passed (4.1 s) |
| base | regression tsc | exit 0 (4.7 s) |
| reference | both validation commands | exit 0: 38 passed; 19 + 19 passed |
| reference | regression | exit 0: 93 passed; 88 + 57 passed (2.2 s); tsc exit 0 (4.3 s) |
| core-only fix (the `src/` half of the patch) | core hidden / package hidden | 38 passed / **exit 1**, 9 of 19 failed. The standalone packages must be fixed too. |

### t10-auth-challenge-quoting (cross-cutting consistency)
| tree | command | result |
|---|---|---|
| base | `$V src/middleware/www-authenticate-quoting.test.ts` | **exit 1**, 5 failed / 1 passed |
| base | regression `$V src/middleware/basic-auth src/middleware/bearer-auth src/middleware/jwt src/middleware/jwk src/utils src/helper/cookie` | exit 0, 624 passed / 2 skipped (3.8 s) |
| base | regression tsc | exit 0 |
| reference | hidden test | exit 0, 6 passed |
| reference | regression | exit 0, 624 passed / 2 skipped (1.6 s); tsc exit 0 (6.7 s) |

## Invented task: acme fixture @ e5209b4c5c99df0e0e5cdab2bd885b0821b91927

`t11-compressed-reply-body` archives `fixtures/acme` at the smoke task's commit, applies `base.patch` (which adds the whole `corvid/` workspace, the planted bug included), strips the four acme repos and runs the hidden test with Bun 1.3.14; it has no setup and no regression command. Checked on 2026-10-15 with `atlas-eval agent run --base-only --tasks <copy> --task <id>` (no session), once on the task and once on a copy whose `base.patch` already holds the reference fix.

### t11-compressed-reply-body (planted bug, serving path)
| tree | command | result |
|---|---|---|
| base | `cd corvid && bun test packages/http/test/compressed-reply.test.ts` | **exit 1**, 4 failed / 5 passed (0.1 s). The failing tests are the raw and string bodies under `gzip` and `br`; JSON bodies, the two uncompressed replies and the empty reply pass. |
| reference | hidden test | exit 0, 9 passed |
| `toReply` fix instead (text replies become buffers) | hidden test | **exit 1**, 2 failed / 7 passed: the uncompressed raw and string replies no longer match what the handler produced. Checked with Bun directly on a scratch copy, not through the harness. |

`t12-deferred-reply-handle` is built the same way (its `base.patch` adds a three-package `corvid/` workspace) and was checked on 2026-10-15 with the same two `--base-only` runs.

### t12-deferred-reply-handle (planted bug, serving path)
| tree | command | result |
|---|---|---|
| base | `cd corvid && bun test packages/web/test/reply-handle.test.ts` | **exit 1**, 2 failed / 4 passed (0.2 s). The failing tests are the two async handlers that send after an await; the sync sender, the returned value, the parameter position and the empty 204 pass. |
| reference | hidden test | exit 0, 6 passed |

## Dry-run of the full suite

`atlas-eval agent run --dry-run --runs-per-condition 2 --model claude-sonnet-5-5` (2026-10-04, seed 1, parallel 2):
- loads all 11 tasks (`t00-smoke` and `t01`–`t10`) and plans **44 runs**: 11 tasks × 2 conditions × 2 runs;
- estimates about **$56.72** in total: roughly $1.28 per baseline run and $1.35 per atlas run (t00-smoke about $0.98 / $1.05), assuming up to 40 turns per run at 1h-cache prices. The tool itself calls this a rough guide, not a bound;
- notes that no code-atlas binary was found, so real atlas runs need `--atlas-bin`.
