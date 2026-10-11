# MCP tool surface cost

- model: `claude-haiku-4-5-20251001`
- Claude Code: 2.1.296 (Claude Code)
- catl: catl 0.1.0 (commit 1c1232df7b9779c7b442740c48136972343cedf7)
- MCP server: rmcp 3.5.0

## Bytes (deterministic, no session)

Measured by starting `catl mcp` on the acme fixture and sending `initialize` and `tools/list`. Sizes are UTF-8 bytes of the compact JSON.

| item | bytes |
|---|---:|
| `instructions` string | 1584 |
| whole `tools/list` result | 13937 |
| tool count | 15 |
| instructions + tools/list, characters / 4 (rough estimate, not a tokenizer) | 3880 |

| tool | name + description | input schema | whole tool object |
|---|---:|---:|---:|
| `atlas_edit` | 965 | 1661 | 2753 |
| `atlas_symbol` | 553 | 1460 | 2092 |
| `atlas_context` | 520 | 484 | 1085 |
| `atlas_similar` | 391 | 456 | 926 |
| `atlas_search` | 234 | 544 | 857 |
| `atlas_impact` | 416 | 353 | 848 |
| `atlas_callers` | 308 | 402 | 789 |
| `atlas_callees` | 302 | 402 | 783 |
| `atlas_unused` | 263 | 306 | 648 |
| `atlas_status` | 313 | 231 | 623 |
| `atlas_api` | 252 | 270 | 601 |
| `atlas_tests_for` | 244 | 255 | 578 |
| `atlas_explain` | 175 | 255 | 509 |
| `atlas_patterns` | 164 | 242 | 485 |
| `atlas_overview` | 144 | 111 | 334 |

## Tokens (measured, tiny sessions)

Prompt `Reply with the single word ok.`, `--max-turns 1`, the eval session flags (`--strict-mcp-config`, `--setting-sources ""`, the six base tools, plus `ToolSearch` in the allowed mode), 3 repeats per configuration; 12 sessions started, 0 of them retries.

Compared fields are those of the first (only) API request, read from its assistant event: `input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`. Their sum (`prompt total`) is the size of the prompt whatever the cache state, so it is the number compared; the individual fields only show how that prompt was split between cache writes and cache reads, which depends on what earlier sessions left in the cache.

| configuration | run | input | cache creation | cache read | prompt total |
|---|---:|---:|---:|---:|---:|
| no MCP, upfront (`ENABLE_TOOL_SEARCH=false`) | 1 | 10 | 1964 | 8428 | 10402 |
| no MCP, upfront (`ENABLE_TOOL_SEARCH=false`) | 2 | 10 | 0 | 10392 | 10402 |
| no MCP, upfront (`ENABLE_TOOL_SEARCH=false`) | 3 | 10 | 0 | 10392 | 10402 |
| Code Atlas, upfront (`ENABLE_TOOL_SEARCH=false`) | 1 | 10 | 14832 | 0 | 14842 |
| Code Atlas, upfront (`ENABLE_TOOL_SEARCH=false`) | 2 | 10 | 0 | 14832 | 14842 |
| Code Atlas, upfront (`ENABLE_TOOL_SEARCH=false`) | 3 | 10 | 0 | 14832 | 14842 |
| no MCP, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 1 | 10 | 0 | 10392 | 10402 |
| no MCP, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 2 | 10 | 0 | 10392 | 10402 |
| no MCP, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 3 | 10 | 0 | 10392 | 10402 |
| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 1 | 10 | 0 | 14832 | 14842 |
| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 2 | 10 | 0 | 14832 | 14842 |
| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 3 | 10 | 0 | 14832 | 14842 |
| Code Atlas, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 1 | 10 | 11476 | 0 | 11486 |
| Code Atlas, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 2 | 10 | 1805 | 9671 | 11486 |
| Code Atlas, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 3 | 10 | 0 | 11476 | 11486 |
| no MCP, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 1 | 10 | 1126 | 9266 | 10402 |
| no MCP, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 2 | 10 | 0 | 10392 | 10402 |
| no MCP, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 3 | 10 | 0 | 10392 | 10402 |

| configuration | prompt total min / median / max | cache creation median | cache read median |
|---|---|---:|---:|
| no MCP, upfront (`ENABLE_TOOL_SEARCH=false`) | 10402 / 10402 / 10402 | 0 | 10392 |
| Code Atlas, upfront (`ENABLE_TOOL_SEARCH=false`) | 14842 / 14842 / 14842 | 0 | 14832 |
| no MCP, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 10402 / 10402 / 10402 | 0 | 10392 |
| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 14842 / 14842 / 14842 | 0 | 14832 |
| Code Atlas, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 11486 / 11486 / 11486 | 1805 | 9671 |
| no MCP, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 10402 / 10402 / 10402 | 0 | 10392 |

### Tokens Code Atlas adds per request

Difference of medians, with the extreme range (lowest Code Atlas run minus highest baseline run, highest minus lowest).

| loading | prompt total | input | cache creation | cache read |
|---|---|---|---|---|
| upfront (`ENABLE_TOOL_SEARCH=false`) | +4440 (+4440 to +4440) | +0 (+0 to +0) | +0 (-1964 to +14832) | +4440 (-10392 to +6404) |
| tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | +4440 (+4440 to +4440) | +0 (+0 to +0) | +0 (+0 to +0) | +4440 (+4440 to +4440) |
| tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | +1084 (+1084 to +1084) | +0 (+0 to +0) | +1805 (-1126 to +11476) | -721 (-10392 to +2210) |

- upfront (`ENABLE_TOOL_SEARCH=false`) adds 4440 tokens per request.
- tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) adds 4440 tokens per request.
- tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) adds 1084 tokens per request.
- The byte accounting predicts about 3880 tokens (characters / 4) if every schema and the instructions were sent; the upfront delta is 1.14 times that.
- With the `ToolSearch` tool not allowed nothing was deferred (median delta 4440 against 4440 upfront): the init event listed 15 atlas tools and the `ToolSearch` tool was absent.
- The tool-search-allowed mode was measured in a second run: 6 sessions started, 0 of them retries.

### What the init event exposed

| configuration | tools | atlas tools | `ToolSearch` tool | MCP servers | identical in every run |
|---|---:|---:|---|---|---|
| no MCP, upfront (`ENABLE_TOOL_SEARCH=false`) | 6 | 0 | absent | none | yes |
| Code Atlas, upfront (`ENABLE_TOOL_SEARCH=false`) | 21 | 15 | absent | code-atlas (connected) | yes |
| no MCP, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 6 | 0 | absent | none | yes |
| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 21 | 15 | absent | code-atlas (connected) | yes |
| Code Atlas, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 22 | 15 | present | code-atlas (connected) | yes |
| no MCP, tool search on, `ToolSearch` allowed (`ENABLE_TOOL_SEARCH=true` + `--tools`/`--allowedTools` with `ToolSearch`) | 7 | 0 | present | none | yes |

### Limits

- One model and one Claude Code version; the byte numbers do not depend on either.
- The prompt total is the size of the first request, not its cost: cache reads and cache writes are billed differently.
- The sessions run in an empty directory with no project instructions, so the baseline prompt (system prompt, six built-in tools) is a floor, not a typical repository session.
- characters / 4 is a rough estimate, not a tokenizer count.
