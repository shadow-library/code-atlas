# Extending Code Atlas

Rules that apply to every extension (from `CLAUDE.md`): never present heuristic or inferred knowledge as certain; every fact carries exactly one `Certainty`; `atlas-core` contracts are append-only (add items or `#[non_exhaustive]` variants, never change signatures or key formats); no panics on any input; typed errors; tests named `should_*` with exact expected values. Run `./scripts/check.sh` before submitting.

Extension points differ in how pluggable they are:

| Extension | Pluggable without editing the engine? |
|---|---|
| Detector | built in by registration in `DetectorRegistry::default` (one line); `EngineOptions::extra_detectors` exists but is `#[doc(hidden)]` |
| Project pass | yes, `EngineOptions::passes` |
| LLM provider | by adding a `ProviderConfig` variant; the trait itself is public |
| Language analyzer | no: analyzers are wired into the engine's `Toolchain`, the resolver and the key scheme |

## Add a language analyzer

Contract (`atlas-core/src/traits.rs`):

```rust
pub trait LanguageAnalyzer: Send + Sync {
  fn language_family(&self) -> LanguageFamily;
  fn supports(&self, language: Language) -> bool;
  fn producer(&self) -> Producer;
  fn analyze(&self, input: &AnalysisInput<'_>) -> Result<FileFacts, AnalysisError>;
}
```

Steps:

1. Add the language and family to `Language` / `LanguageFamily` (`atlas-core/src/lang.rs`, both `#[non_exhaustive]`) with extensions in `from_extension`; extend `SymbolKey` rendering only additively (the family string is part of every key, and `EntityId` values are pinned by tests).
2. Create `atlas-lang-<name>` depending on `atlas-core` only. Produce `FileFacts`: `symbols` (use `Entity::from_symbol` conventions and `symbol_keys`), `imports`, `exports`, `references` (with `RefKind`, `ReceiverHint`, `ArgSummary` for literal arguments), `impls`, `module_decls`, body fingerprints (`BodyFingerprintInput`, at most `MAX_BODY_TOKENS`), `diagnostics`, `parse_status` (`Ok`, `Partial` with at most `MAX_REPORTED_PARSE_ERRORS`, `Failed`). Never panic; return `Partial` for syntax errors and extract what you can. `FileFacts::check_invariants` is run by the engine.
3. Emit references with the right receiver semantics (documented in `CLAUDE.md`): `TypeHint` only for declared types, never inferred; `Expr` for anything else; computed members as `[expr]` segments.
4. Teach `atlas-resolve` the language's module and name rules (`ProjectSymbolTable`, a new resolver module beside `rust.rs` and `ts.rs`). Cap every uncertain result at `heuristic` and never emit `compiler_verified`. Honour the incremental contract: `upsert_file` returns a `SurfaceDelta`, and `resolve_file` records every looked-up name.
5. Register the analyzer in `atlas-engine/src/analyze.rs` (`Toolchain::analyzer`), add manifest and package-graph support in `atlas-project/src/manifest` if the ecosystem has packages, and add a `ScipIndexer` in `atlas-scip` if a SCIP indexer exists.
6. Fixtures and ground truth: add a case under `fixtures/` with an `expected.toml` (schema in `fixtures/expected.schema.md`), including adversarial cases for dynamic dispatch, generated code and syntax errors. The quality evaluation picks them up.
7. Tests: unit tests per construct, a never-panics fuzz-style test over malformed input, and the incremental-equals-full regression tests.

## Add a detector

Contract:

```rust
pub trait Detector: Send + Sync {
  fn id(&self) -> &'static str;
  fn producer(&self) -> Producer;
  fn applies_to(&self, facts: &FileFacts) -> bool;
  fn detect(&self, input: &DetectInput<'_>) -> Detection;
}
```

Detectors run per file on `FileFacts` plus source text and an optional `PackageContext` (dependencies). In `atlas-detect`, implement through the `detector!` macro in `detectors/mod.rs` (`detector!(MyDetector, "my-id", 1)`), which provides the trait impls and the `DiagnosingDetector` variant returning diagnostics. Write a `prefilter(&FileFacts) -> bool` (cheap, used by `applies_to`) and `run(&FileView, &mut Sink)`; use `FileView` helpers (`imports_named`, `imports_module`, `rust_path_is`, receiver and type lookups) so a name only counts when it is imported from the expected package.

Output rules:

- Return `DetectedEntity` (key, certainty, span, attrs) and `DetectedEdge`. Endpoints, clients, auth schemes, env vars, permissions and jobs are repo-scoped; events, queues and tables are project-scoped.
- Node references: `Local` for symbols of this file, `Key` for entities you assert (the engine drops the edge if the target does not exist after the run and counts it), `Unresolved { path, from }` for names the engine should resolve in this file (the stored edge keeps the weaker certainty).
- Pick the certainty honestly: `Syntactic` for literal syntax, `Cert::heuristic(confidence, "rule-name")` otherwise; name the rule. Mark immature detectors with `attrs.experimental = true`.
- Do not copy detector attrs into passes; link-derived attrs are asserted by `atlas-link` with their own certainty.
- Emit diagnostics for constructs you recognise but cannot evaluate (dynamic routes and URLs) instead of silently skipping.
- Parallel edges between the same nodes need a distinct `EdgeId` discriminator only when they mean different things.

Register it in `DetectorRegistry::default` (`atlas-detect/src/registry.rs`). Detector output is part of the persisted file artifact, so bump the `producer` version when behaviour changes. Tests live in `crates/atlas-detect/tests` using `render_source` / `render_repo` helpers; add false-positive cases (`tests/false_positives.rs`) and fixture coverage (`tests/coverage.rs` compares against `expected.toml`).

If a detector produces facts that need cross-repo meaning (a new link type), extend `atlas-link` (`link(tx, project)`), which reads only file-owned facts and earlier passes.

## Add a project pass

Contract (`atlas-engine/src/passes.rs`):

```rust
pub trait ProjectPass: Send + Sync {
  fn name(&self) -> &'static str;
  fn run(&self, tx: &ReadTx<'_>, context: &PassContext<'_>) -> Result<PassOutput, PassError>;
}
```

`PassOutput { entities, edges }` replaces the owner `Project { pass: name }` wholesale on every index run, after all file and manifest owners are committed (`structure` and `link` first, then registered passes in order). Pass it through `EngineOptions::passes`; every engine that opens a database must register the same passes, or facts will flap between processes.

Constraints: the pass must be a deterministic function of file-owned facts, `PassContext` (project, package graphs, manifest hashes) and earlier passes; it must not read its own previous output or the `lineage` and `scip:*` owners (so incremental equals full). Names `repos`, `structure`, `link`, `lineage` and the prefix `scip:` are reserved and `Engine::open` rejects duplicates. An error or panic removes the pass's facts and makes the run `partial`, surfaced as degraded freshness. Annotating shared keys works through the store's attribute union; assert only the attributes you derive and cap their certainty at the weakest input.

## Add an LLM provider

Contract (`atlas-core/src/traits.rs`):

```rust
pub trait LlmProvider: Send + Sync {
  fn id(&self) -> &str;
  fn model(&self) -> &str;
  fn availability(&self) -> Availability;
  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
}
```

Implement it in `atlas-semantic/src/provider/` next to `claude_cli.rs`, `anthropic.rs` and `openai.rs`. Requirements:

- `availability()` reports `Unavailable { reason }` instead of failing at call time; enrichment then skips tasks and records a `partial` run.
- Map every SDK, HTTP or process error to `LlmError` (`Unavailable`, `Timeout`, `RateLimited`, `InvalidResponse`, `Provider`); never leak raw errors. Subprocess providers must kill the whole process group on timeout.
- Return `TokenUsage` (input, output, cache read/write) and the model actually used, since cost and cache attribution use it.
- `LlmRequest` carries an optional JSON schema; responses are validated and one repair attempt is made, so return text that parses.
- Register it in `ProviderConfig` (`provider/config.rs`): a variant, parsing in `from_settings` (the `--llm-provider` string), and `build_for(TaskKind)`. Per-task default models (`default_model`) and prices (`PriceTable`) are in `atlas-semantic`; unknown models get no cost estimate.
- Use `FakeProvider` for tests; no test may call a real provider. Never send source to a provider unless enrichment is enabled for the project.

Anything an LLM returns is stored as `Certainty::Inferred { confidence, provider, model, rationale }` with evidence spans and goes stale when the evidence file changes; do not bypass this.

## Add a query or MCP tool

Queries live in `atlas-query/src/queries/<name>.rs` with a text renderer in `text/<name>.rs`; they must go through the freshness barrier, surface `Freshness::Degraded`, tag every non-authoritative relation, render per-attribute provenance, and list caveats for empty results. Expose it in `atlas-cli` (`cli.rs`, `commands/query.rs`) and in `atlas-mcp` (`arguments.rs` with validation, `descriptions.rs`, `server.rs`); the MCP protocol test pins the exact tool list and must be updated.
