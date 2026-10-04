# Code Atlas knowledge-quality report

Date: 2026-10-04. Schema version 1.

## Environment

- rust-analyzer SCIP: available (rust-analyzer scip)
- scip-typescript: available (npx @sourcegraph/scip-typescript)

## Limits

- **Fixture sample bias.** The fixtures are a few small, hand-written projects plus one real workspace (this repository). Recall and precision on them say little about how the system behaves on arbitrary code, and the ground truth was written alongside the system.
- **Oracle scope.** SCIP is compared only for `calls`, `references`, `instantiates`, `implements` and `extends`, only in files an indexer covered, and only for Rust and TypeScript. References outside any symbol and cross-package targets in per-package indexes cannot be confirmed. SCIP itself disagrees with us on granularity (constructor versus class, receiver versus method).
- **Heuristic calibration rests on a small sample.** The fixture-judged heuristic edges number a few dozen; the oracle buckets are dominated by one confidence tier.
- **No exhaustive kinds.** No fixture declares an exhaustive edge set, so unlisted-edge precision is not measured.
- **No LLM verdicts.** Inferred similarity verdicts do not exist in these runs, so verdict accuracy is not measured.

## How to read this report

- Each expectation ends in one of: **found** (present with an accepted certainty class and confidence), **wrong certainty** (present, but its class or confidence is outside the expectation), **wrong detail** (present, but a secondary property such as kind, occurrences, reason or handler differs), **missing**, or **unbound** (a selector could not be bound to exactly one entity: a ground-truth binding error, kept out of recall).
- *Recall (bound)* is found / (found + wrong + missing) and ignores unbound expectations. *Recall (all)* is found / expected and counts unbound expectations as failures, the pessimistic bound. A selector that matches nothing is only a binding error when no `[[entity]]` expectation explains the absence; if the entity expectation itself is missing, dependent edges count as missing.
- Selectors naming repos of another project are *foreign*: they cannot exist in this project's index, so prohibitions on them hold vacuously and are counted separately.
- No fixture declares an exhaustive edge set (`exhaustive_edge_kinds` is absent from every `expected.toml`; `unused_exhaustive` is false everywhere), so no unlisted-edge precision is claimed. Precision is reported as forbidden-edge satisfaction, fabrication checks on unresolved references, `not_unused` violations, `must_not_include` violations, isolation leaks, and as the oracle precision against SCIP.
- Calibration judges only edges that appear in an expectation (expected edges, tolerated candidates, forbidden violations), so it is biased towards the hard cases the fixtures were written to probe.
- Selector conventions the evaluator applies (none edits a fixture): Rust symbols are the item path relative to the file's own module, so inline modules appear (`tests::name`) and trait impl methods render as `<Type as Trait>::method`; a TypeScript `default` selects the default-exported symbol of the file; `suite` matches the label of a `describe` block even when it is nested; `db_model` binds to the `db_table` entity named by `name` and requires a `maps_to_table` edge from `sym`; an unresolved `generated_code` reference is accepted when the resolver records `macro_generated` (counted as an approximate reason), and a dynamic route or URL is accepted when the detector emitted a `route-dynamic` or `http.dynamic_url` diagnostic inside the selected symbol.
- Nothing in this report has been tuned: every number measures the system as committed.

## Headline per fixture

| fixture | entities found | edges found | edge recall (bound) | edges wrong cert/detail | edges missing | edges unbound | forbidden violations/checked | unresolved found | fabricated | binding errors |
|---|---|---|---|---|---|---|---|---|---|---|
| acme | 89/89 | 56/56 | 100.0% | 0 | 0 | 0 | 0/22 (+6 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| globex | 20/20 | 10/10 | 100.0% | 0 | 0 | 0 | 0/0 (+3 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| adversarial/broken-syntax | 12/12 | 6/6 | 100.0% | 0 | 0 | 0 | 0/2 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| adversarial/computed-routes-ts | 4/4 | 4/4 | 100.0% | 0 | 0 | 0 | 0/4 (+3 vacuous, 0 unbound) | 2/2 | 0 | 0 |
| adversarial/duplicate-names-rust | 6/6 | 3/3 | 100.0% | 0 | 0 | 0 | 0/5 (+0 vacuous, 0 unbound) | 1/1 | 0 | 0 |
| adversarial/dynamic-ts | 9/9 | 1/1 | 100.0% | 0 | 0 | 0 | 0/8 (+0 vacuous, 0 unbound) | 5/5 | 0 | 0 |
| adversarial/generated-code | 7/7 | 2/2 | 100.0% | 0 | 0 | 0 | 0/2 (+0 vacuous, 0 unbound) | 2/3 | 0 | 0 |
| adversarial/macro-heavy-rust | 6/6 | 2/2 | 100.0% | 0 | 0 | 0 | 0/2 (+0 vacuous, 0 unbound) | 4/4 | 0 | 0 |
| adversarial/reexport-chain-ts | 3/3 | 4/4 | 100.0% | 0 | 0 | 0 | 0/1 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| adversarial/shadowing-ts | 5/5 | 2/2 | 100.0% | 0 | 0 | 0 | 0/3 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| adversarial/similar-not-equivalent | 8/8 | 0/0 | n/a | 0 | 0 | 0 | 0/4 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| adversarial/trait-dispatch-rust | 15/15 | 6/8 | 75.0% | 2 | 0 | 0 | 0/10 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| **all fixtures** | 184/184 | 96/98 | 98.0% | 2 | 0 | 0 | 0/63 (+12 vacuous, 0 unbound) | 14/15 | 0 | 0 |

## Edge kinds (all fixtures)

| edge kind | expected | found | wrong certainty | wrong detail | missing | unbound | recall (bound) | recall (all) | optional present/expected | forbidden violations/checked | exhaustive unexpected |
|---|---|---|---|---|---|---|---|---|---|---|---|
| calls | 32 | 30 | 2 | 0 | 0 | 0 | 93.8% | 93.8% | 10/10 | 0/31 (+1 vacuous, 0 unbound) | not declared |
| calls_endpoint | 10 | 10 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/21 (+6 vacuous, 0 unbound) | not declared |
| depends_on_package | 1 | 1 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| equivalent_to | 0 | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 0/0 | 0/7 (+1 vacuous, 0 unbound) | not declared |
| exposes_endpoint | 7 | 7 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+2 vacuous, 0 unbound) | not declared |
| implements | 4 | 4 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| implements_auth | 3 | 3 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| imports | 0 | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 0/0 | 0/1 (+0 vacuous, 0 unbound) | not declared |
| instantiates | 1 | 1 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| publishes | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| re_exports | 5 | 5 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| reads_config | 1 | 1 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| reads_env | 9 | 9 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/1 (+0 vacuous, 0 unbound) | not declared |
| references | 3 | 3 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/2 (+0 vacuous, 0 unbound) | not declared |
| requires_permission | 4 | 4 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| subscribes | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+2 vacuous, 0 unbound) | not declared |
| supports_auth | 9 | 9 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |
| uses_auth | 5 | 5 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0/0 | 0/0 (+0 vacuous, 0 unbound) | not declared |

### Edge kinds per fixture

| fixture | edge kind | found/expected | wrong cert/detail | missing | unbound | recall (bound) | forbidden violations/checked |
|---|---|---|---|---|---|---|---|
| acme | calls | 7/7 | 0 | 0 | 0 | 100.0% | 0/1 (+1 vacuous, 0 unbound) |
| acme | calls_endpoint | 6/6 | 0 | 0 | 0 | 100.0% | 0/17 (+3 vacuous, 0 unbound) |
| acme | depends_on_package | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | equivalent_to | 0/0 | 0 | 0 | 0 | n/a | 0/3 (+1 vacuous, 0 unbound) |
| acme | exposes_endpoint | 5/5 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | implements | 2/2 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | implements_auth | 3/3 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | instantiates | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | publishes | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | re_exports | 2/2 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | reads_config | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | reads_env | 6/6 | 0 | 0 | 0 | 100.0% | 0/1 (+0 vacuous, 0 unbound) |
| acme | references | 2/2 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | requires_permission | 4/4 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | subscribes | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+1 vacuous, 0 unbound) |
| acme | supports_auth | 9/9 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| acme | uses_auth | 5/5 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| globex | calls | 3/3 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| globex | calls_endpoint | 0/0 | 0 | 0 | 0 | n/a | 0/0 (+2 vacuous, 0 unbound) |
| globex | exposes_endpoint | 2/2 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| globex | publishes | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| globex | reads_env | 3/3 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| globex | subscribes | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+1 vacuous, 0 unbound) |
| adversarial/broken-syntax | calls | 6/6 | 0 | 0 | 0 | 100.0% | 0/2 (+0 vacuous, 0 unbound) |
| adversarial/computed-routes-ts | calls_endpoint | 4/4 | 0 | 0 | 0 | 100.0% | 0/4 (+1 vacuous, 0 unbound) |
| adversarial/computed-routes-ts | exposes_endpoint | 0/0 | 0 | 0 | 0 | n/a | 0/0 (+2 vacuous, 0 unbound) |
| adversarial/duplicate-names-rust | calls | 3/3 | 0 | 0 | 0 | 100.0% | 0/5 (+0 vacuous, 0 unbound) |
| adversarial/dynamic-ts | calls | 1/1 | 0 | 0 | 0 | 100.0% | 0/7 (+0 vacuous, 0 unbound) |
| adversarial/dynamic-ts | imports | 0/0 | 0 | 0 | 0 | n/a | 0/1 (+0 vacuous, 0 unbound) |
| adversarial/generated-code | calls | 2/2 | 0 | 0 | 0 | 100.0% | 0/1 (+0 vacuous, 0 unbound) |
| adversarial/generated-code | references | 0/0 | 0 | 0 | 0 | n/a | 0/1 (+0 vacuous, 0 unbound) |
| adversarial/macro-heavy-rust | calls | 1/1 | 0 | 0 | 0 | 100.0% | 0/2 (+0 vacuous, 0 unbound) |
| adversarial/macro-heavy-rust | references | 1/1 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/reexport-chain-ts | calls | 1/1 | 0 | 0 | 0 | 100.0% | 0/1 (+0 vacuous, 0 unbound) |
| adversarial/reexport-chain-ts | re_exports | 3/3 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/shadowing-ts | calls | 2/2 | 0 | 0 | 0 | 100.0% | 0/2 (+0 vacuous, 0 unbound) |
| adversarial/shadowing-ts | references | 0/0 | 0 | 0 | 0 | n/a | 0/1 (+0 vacuous, 0 unbound) |
| adversarial/similar-not-equivalent | equivalent_to | 0/0 | 0 | 0 | 0 | n/a | 0/4 (+0 vacuous, 0 unbound) |
| adversarial/trait-dispatch-rust | calls | 4/6 | 2 | 0 | 0 | 66.7% | 0/10 (+0 vacuous, 0 unbound) |
| adversarial/trait-dispatch-rust | implements | 2/2 | 0 | 0 | 0 | 100.0% | 0/0 (+0 vacuous, 0 unbound) |

## Entities by kind (all fixtures)

| entity kind | expected | found | wrong certainty | wrong detail | missing | unbound | recall (bound) | recall (all) |
|---|---|---|---|---|---|---|---|---|
| api_endpoint | 17 | 17 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| auth_scheme | 3 | 3 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| class | 18 | 18 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| config_key | 1 | 1 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| constant | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| db_model | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| env_var | 8 | 8 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| event | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| function | 73 | 73 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| interface | 1 | 1 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| macro | 2 | 2 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| method | 32 | 32 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| package | 4 | 4 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| permission | 3 | 3 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| struct | 9 | 9 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| test | 4 | 4 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| trait | 3 | 3 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| **all required** | 184 | 184 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |
| endpoint handlers | 17 | 17 | 0 | 0 | 0 | 0 | 100.0% | 100.0% |

Best-effort entities (not required): 2/2 found.

## Other expectation tables

| fixture | unmatched calls | endpoint auth | tests-for | file flags | blast must_include | blast must_not_include violations | isolation leaks |
|---|---|---|---|---|---|---|---|
| acme | 1/1 | 8/8 | 8/9 | 0/0 | 22/22 | 0/10 (+5 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| globex | 0/0 | 0/0 | 0/0 | 0/0 | 3/3 | 0/0 (+5 vacuous, 0 unbound) | 0/18 (+0 vacuous, 0 unbound) |
| adversarial/broken-syntax | 0/0 | 0/0 | 0/0 | 2/2 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/computed-routes-ts | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/duplicate-names-rust | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/dynamic-ts | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/generated-code | 1/1 | 0/0 | 0/0 | 1/1 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/macro-heavy-rust | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/reexport-chain-ts | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/shadowing-ts | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/similar-not-equivalent | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |
| adversarial/trait-dispatch-rust | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 (+0 vacuous, 0 unbound) | 0/0 (+0 vacuous, 0 unbound) |

### Auth matrix, unused code and similarity

| fixture | supported schemes (precision / recall) | used (precision / recall) | unused (precision / recall) | [[unused]] found | [[not_unused]] violations/checked | unused precision over judged | similarity |
|---|---|---|---|---|---|---|---|
| acme | 100.0% / 100.0% | 100.0% / 100.0% | 100.0% / 100.0% | 9/9 reported (5 likely, 4 possibly) | 0/19 (+0 vacuous, 0 unbound); 0 hedged | 100.0% | 3 claims checked, 0 violated; 0 inferred verdicts (0 correct), 3 undecided, 2 not evaluable (cross-project) |
| globex | 100.0% / 100.0% | n/a / n/a | n/a / 0.0% | 0/0 reported (0 likely, 0 possibly) | 0/0 (+0 vacuous, 0 unbound); 0 hedged | n/a | 0 claims checked, 0 violated; 0 inferred verdicts (0 correct), 0 undecided, 0 not evaluable (cross-project) |
| adversarial/dynamic-ts | n/a / n/a | n/a / n/a | n/a / n/a | 0/0 reported (0 likely, 0 possibly) | 0/2 (+0 vacuous, 0 unbound); 2 hedged | n/a | 0 claims checked, 0 violated; 0 inferred verdicts (0 correct), 0 undecided, 0 not evaluable (cross-project) |
| adversarial/similar-not-equivalent | n/a / n/a | n/a / n/a | n/a / n/a | 0/0 reported (0 likely, 0 possibly) | 0/0 (+0 vacuous, 0 unbound); 0 hedged | n/a | 4 claims checked, 0 violated; 0 inferred verdicts (0 correct), 4 undecided, 0 not evaluable (cross-project) |
| adversarial/trait-dispatch-rust | n/a / n/a | n/a / n/a | n/a / n/a | 0/0 reported (0 likely, 0 possibly) | 0/1 (+0 vacuous, 0 unbound); 0 hedged | n/a | 0 claims checked, 0 violated; 0 inferred verdicts (0 correct), 0 undecided, 0 not evaluable (cross-project) |

Similarity pairs:

| fixture | a | b | expected | scope | surfaced | structural jaccard | inferred verdict | system verdict | deterministic equivalent_to |
|---|---|---|---|---|---|---|---|---|---|
| acme | { repo=acme-web, path=src/utils/currency.ts, symbol=parseMoney } | { repo=acme-sdk, path=src/money.ts, symbol=toCents } | equivalent | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |
| acme | { repo=acme-billing, path=src/money.rs, symbol=to_cents } | { repo=acme-sdk, path=src/money.ts, symbol=toCents } | equivalent | project | true | none | none (no LLM verdict) | related by name only, behaviour unverified | false |
| acme | { repo=acme-api, path=src/billing/rounding.ts, symbol=roundHalfEven } | { repo=acme-api, path=src/billing/rounding.ts, symbol=roundHalfUp } | similar_not_equivalent | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |
| acme | { repo=acme-api, path=src/common/money.ts, symbol=formatMoney } | { repo=globex-api, path=src/util/money.ts, symbol=formatMoney } | unrelated | cross_project | false | none | none (no LLM verdict) |  | false |
| acme | { repo=acme-billing, path=src/money.rs, symbol=to_cents } | { repo=globex-worker, path=src/money.rs, symbol=to_cents } | similar_not_equivalent | cross_project | false | none | none (no LLM verdict) |  | false |
| adversarial/similar-not-equivalent | { repo=similar-not-equivalent, path=src/rounding.ts, symbol=roundHalfUp } | { repo=similar-not-equivalent, path=src/rounding.ts, symbol=roundHalfEven } | similar_not_equivalent | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |
| adversarial/similar-not-equivalent | { repo=similar-not-equivalent, path=src/user-input.ts, symbol=normalize } | { repo=similar-not-equivalent, path=src/path-input.ts, symbol=normalize } | unrelated | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |
| adversarial/similar-not-equivalent | { repo=similar-not-equivalent, path=src/sums.ts, symbol=sumLoop } | { repo=similar-not-equivalent, path=src/sums.ts, symbol=sumReduce } | equivalent | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |
| adversarial/similar-not-equivalent | { repo=similar-not-equivalent, path=src/sums.ts, symbol=isEvenMod } | { repo=similar-not-equivalent, path=src/sums.ts, symbol=isEvenBit } | similar_not_equivalent | project | true | none | none (no LLM verdict) | same signature shape and related name, behaviour unverified | false |

## Certainty calibration

Per class, over judged edges: *correct* = the edge is expected and its class and confidence are accepted; *overclaimed* = the relation is expected but the class is stronger than allowed; *underclaimed* = weaker than allowed; *false* = a forbidden edge exists. *True relation* = (judged - false) / judged.

| class | judged | correct | overclaimed | underclaimed | false | correct % | true relation % |
|---|---|---|---|---|---|---|---|
| heuristic | 48 | 46 | 0 | 2 | 0 | 95.8% | 100.0% |
| resolved | 41 | 41 | 0 | 0 | 0 | 100.0% | 100.0% |
| syntactic | 19 | 19 | 0 | 0 | 0 | 100.0% | 100.0% |

Heuristic edges by confidence:

| confidence | judged | correct | false | true relation % |
|---|---|---|---|---|
| [0.0, 0.4) | 10 | 10 | 0 | 100.0% |
| [0.4, 0.6) | 1 | 1 | 0 | 100.0% |
| [0.6, 0.8) | 15 | 13 | 0 | 100.0% |
| [0.8, 1.0] | 22 | 22 | 0 | 100.0% |

True-relation rate is non-decreasing with confidence across the populated buckets.

## Ground-truth binding errors

A selector that matched zero or several entities and is not explained by a missing `[[entity]]` expectation. These are excluded from recall and precision. They may indicate a wrong fixture or a naming-convention gap in the evaluator; none was edited to fit the system.

None.

## Findings (every non-passing expectation)

### acme

- `test_of` [tests_for] { repo=acme-billing, path=tests/invoices.rs, symbol=unknown_invoice_returns_404 } tests { repo=acme-billing, path=src/lib.rs, symbol=router }: wrong certainty: [heuristic 0.60]

### globex

- `auth` [unused] globex-api api_key: expected but not reported

### adversarial/generated-code

- `unresolved` [include] include * ({ repo=generated-code, path=rust/src/lib.rs }): wrong detail: recorded as External { package: "std" }, expected generated_code

### adversarial/trait-dispatch-rust

- `edge` [calls] calls: { repo=trait-dispatch-rust, path=src/lib.rs, symbol=read_generic } -> { repo=trait-dispatch-rust, path=src/lib.rs, symbol=Store::get }: wrong certainty: [heuristic 0.70]
- `edge` [calls] calls: { repo=trait-dispatch-rust, path=src/lib.rs, symbol=write_boxed } -> { repo=trait-dispatch-rust, path=src/lib.rs, symbol=Store::put }: wrong certainty: [heuristic 0.70]

## Resolver versus the compiler oracle (SCIP)

Headline (calls + references + instantiates, all targets):

- resolved precision: 97.5% (confirmable edges, 27613 of 28220) / 95.4% all-in; 26928 agreed, 3 conflicting, 682 same-package unconfirmed, 607 unconfirmable (98 cross-package, 509 file-level).
- heuristic precision: 89.2% (confirmable edges, 2602 of 3041) / 76.3% all-in; 2321 agreed, 9 conflicting, 272 same-package unconfirmed, 439 unconfirmable (439 cross-package, 0 file-level).
- recall of SCIP's edges: 44.1% by resolved edges, 47.9% by all of our edges.

Our file-owned `calls`, `references`, `instantiates`, `implements` and `extends` edges in the files SCIP indexed are reconciled with the SCIP edges by `atlas_scip::reconcile`. Cargo workspaces are indexed once at the workspace root so that cross-crate edges can be confirmed; other Cargo and npm packages are indexed per package. Because `reconcile` keys sites by exact span, a second pass pairs leftover ours-only and scip-only edges with the same enclosing symbol and file: the same target counts as *agreed* (shown as *respanned*) when the spans overlap, or when they start on one line and both spans contain the callee name; a different target with overlapping spans counts as *conflicting*. *agreed*: same site and target. *ours-only*: we have an edge SCIP does not (a heuristic candidate beside an agreed target, or a site SCIP does not know). *ours-only cross-package*: our edge targets another package, which a per-package index cannot confirm. *ours-only file-level*: the enclosing entity is a file, and the ingestor drops SCIP references outside any symbol (`references_unmapped`), so the oracle cannot confirm them. These two are *unconfirmable*. *scip-only*: SCIP target we lack. *conflicting*: both sides have targets at one site and none is shared.

Precision (confirmable-only) = agreed / (agreed + conflicting + same-package ours-only): it counts every unconfirmed confirmable edge as wrong but ignores unconfirmable ones. Precision (all-in) = agreed / (all of our edges in covered files, unconfirmable included): it counts every edge the oracle did not confirm as wrong. Recall vs SCIP = agreed / (agreed + scip-only + SCIP targets in conflicts), always over all of SCIP's edges, so the per-class rows show how much of SCIP's graph each of our classes reproduces. `implements` is shown separately: method-level `implements` edges (impl method to trait method) are not modelled by SCIP and always appear ours-only. SCIP points `new X()` at the constructor and `acme.createOrder()` occurrences at several symbols, so some conflicts are granularity differences rather than wrong targets; the sample below is for manual inspection.

| target | packages | files covered by SCIP | note |
|---|---|---|---|
| acme | acme-api:/ scip-typescript indexed 32 files, 118 edges ingested, 0 files without usable facts (32 files); acme-sdk:/ scip-typescript indexed 6 files, 35 edges ingested, 0 files without usable facts (6 files); acme-web:/ scip-typescript indexed 11 files, 15 edges ingested, 0 files without usable facts (11 files); acme-billing:/ rust-analyzer indexed 9 files, 116 edges ingested, 0 files without usable facts (9 files) | 58 |  |
| globex | globex-api:/ scip-typescript indexed 8 files, 21 edges ingested, 0 files without usable facts (8 files); globex-worker:/ rust-analyzer indexed 3 files, 9 edges ingested, 0 files without usable facts (3 files) | 11 |  |
| adversarial/computed-routes-ts | computed-routes-ts:/ scip-typescript indexed 4 files, 0 edges ingested, 0 files without usable facts (4 files) | 4 |  |
| adversarial/duplicate-names-rust | duplicate-names-rust:/ rust-analyzer indexed 4 files, 4 edges ingested, 0 files without usable facts (5 files) | 4 |  |
| adversarial/dynamic-ts | dynamic-ts:/ scip-typescript indexed 3 files, 1 edges ingested, 0 files without usable facts (3 files) | 3 |  |
| adversarial/generated-code | generated-code:rust/ rust-analyzer indexed 2 files, 1 edges ingested, 0 files without usable facts (2 files); generated-code:/ scip-typescript indexed 2 files, 3 edges ingested, 0 files without usable facts (2 files) | 4 |  |
| adversarial/macro-heavy-rust | macro-heavy-rust:/ rust-analyzer indexed 1 files, 1 edges ingested, 0 files without usable facts (1 files) | 1 |  |
| adversarial/reexport-chain-ts | reexport-chain-ts:/ scip-typescript indexed 6 files, 1 edges ingested, 0 files without usable facts (6 files) | 6 |  |
| adversarial/shadowing-ts | shadowing-ts:/ scip-typescript indexed 2 files, 2 edges ingested, 0 files without usable facts (2 files) | 2 |  |
| adversarial/similar-not-equivalent | similar-not-equivalent:/ scip-typescript indexed 4 files, 0 edges ingested, 0 files without usable facts (4 files) | 4 |  |
| adversarial/trait-dispatch-rust | trait-dispatch-rust:/ rust-analyzer indexed 1 files, 18 edges ingested, 0 files without usable facts (1 files) | 1 |  |
| workspace | code-atlas:/ rust-analyzer indexed 350 files, 44193 edges ingested, 0 files without usable facts (350 files) | 350 |  |

| edge kind / our class | agreed | ours-only | ours-only cross-package | ours-only file-level | scip-only | conflicting | precision (confirmable-only) | precision (all-in) | recall vs SCIP |
|---|---|---|---|---|---|---|---|---|---|
| calls / all | 15516 (58 respanned) | 393 | 432 | 5 | 1243 | 7 | 97.5% | 94.9% | 92.6% |
| calls / resolved | 13225 (58 respanned) | 123 | 0 | 5 | 0 | 0 | 99.1% | 99.0% | 78.9% |
| calls / heuristic | 2291 (0 respanned) | 270 | 432 | 0 | 0 | 7 | 89.2% | 76.4% | 13.7% |
| implements / all | 44 (0 respanned) | 48 | 53 | 0 | 0 | 0 | 47.8% | 30.3% | 100.0% |
| implements / resolved | 44 (0 respanned) | 48 | 53 | 0 | 0 | 0 | 47.8% | 30.3% | 100.0% |
| implements / heuristic | 0 (0 respanned) | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 0.0% |
| instantiates / all | 2924 (2 respanned) | 175 | 0 | 0 | 19 | 2 | 94.3% | 94.3% | 99.2% |
| instantiates / resolved | 2919 (2 respanned) | 175 | 0 | 0 | 0 | 2 | 94.3% | 94.3% | 99.0% |
| instantiates / heuristic | 5 (0 respanned) | 0 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0.2% |
| references / all | 10809 (9 respanned) | 386 | 105 | 504 | 30559 | 3 | 96.5% | 91.5% | 26.1% |
| references / resolved | 10784 (9 respanned) | 384 | 98 | 504 | 0 | 1 | 96.6% | 91.6% | 26.1% |
| references / heuristic | 25 (0 respanned) | 2 | 7 | 0 | 0 | 2 | 86.2% | 69.4% | 0.1% |
| **calls+references+instantiates / all** | 29249 (69 respanned) | 954 | 537 | 509 | 31821 | 12 | 96.8% | 93.6% | 47.9% |
| **calls+references+instantiates / resolved** | 26928 (69 respanned) | 682 | 98 | 509 | 0 | 3 | 97.5% | 95.4% | 44.1% |
| **calls+references+instantiates / heuristic** | 2321 (0 respanned) | 272 | 439 | 0 | 0 | 9 | 89.2% | 76.3% | 3.8% |

SCIP indexes used (blake3 of each `.scip`; the cache key encodes the inputs, so identical inputs reproduce the digest):

| target | repo | root | indexer | blake3 |
|---|---|---|---|---|
| acme | acme-api | / | scip-typescript | 2d58df426d07f16c660460de93417e5e4dff0d1607aa1214070099a24119acf7 |
| acme | acme-sdk | / | scip-typescript | 70a333159b2e8f08c395bb0371a5a3b2656074d58b77a83d26c9e2e2cff9b4ce |
| acme | acme-web | / | scip-typescript | f1b820dbba806b0183398e5766bc60c9715dfc765e4d3ee18a8adc3894a44932 |
| acme | acme-billing | / | rust-analyzer | ed73e02025df137ba6d7116bc2d3ac3a926218a6807bcd81c3b146c64d636263 |
| globex | globex-api | / | scip-typescript | 68e5a9da62f726645ba7a229af621f027e036f5fa9d9837f2084d5620bfe4798 |
| globex | globex-worker | / | rust-analyzer | f80a0f73f1a16f378573eaecbf1efc7b8eaa3a141c4f32600f7546c0fba89040 |
| adversarial/computed-routes-ts | computed-routes-ts | / | scip-typescript | 1856b7dee85529d142887032bfa87d8108bd66cc15cb2b0c5fff824329e9f8a7 |
| adversarial/duplicate-names-rust | duplicate-names-rust | / | rust-analyzer | 513b346ce77b4edb19e690c561324d1c786f84271f348f6c2c37af73980a0cdd |
| adversarial/dynamic-ts | dynamic-ts | / | scip-typescript | f6e36f2e2f17dcedbb620d9a50832859599aa111d0f5f12be03dd5f6a34fb860 |
| adversarial/generated-code | generated-code | rust/ | rust-analyzer | 29c35b517510e83c34bb1b1bc96cc453e3638d87bfe28f872f987f79593dd6fd |
| adversarial/generated-code | generated-code | / | scip-typescript | aa069c2f3ff932c757dd9abe8cb498cf12c78dc701e8eb5a20296ffa46fb9690 |
| adversarial/macro-heavy-rust | macro-heavy-rust | / | rust-analyzer | 5a3dbbe27b7f92b9246ec1c6ca58d3a3df593b2dbbf902f11e1e6a66d917ab07 |
| adversarial/reexport-chain-ts | reexport-chain-ts | / | scip-typescript | 04eac349a2ad474db0154d39d10d417e79c2c524a928bf2faf4246121c078623 |
| adversarial/shadowing-ts | shadowing-ts | / | scip-typescript | f28df5ee40dc2bc239f6c71bd58b9dc17e249b472a400ccfefdcf5ae1a902428 |
| adversarial/similar-not-equivalent | similar-not-equivalent | / | scip-typescript | f4d8629f1a84123e01f20c7c6f0832158814b1fd450afe6d4c97ea4bcada6a7c |
| adversarial/trait-dispatch-rust | trait-dispatch-rust | / | rust-analyzer | e1d0b7543749298991b6c598a3dff4fc6afb7e68611e822561f412fa5216c6aa |
| workspace | code-atlas | / | rust-analyzer | e6c8685c60a97dd953aed8f6b057bfe9bad0bbbe5ae93dc054ea43e391727c0e |

Heuristic edges by confidence against the oracle (unconfirmed = same-package ours-only):

| confidence | agreed | conflicting | unconfirmed | agreed / (agreed + conflicting) |
|---|---|---|---|---|
| [0.0, 0.4) | 425 | 5 | 265 | 98.8% |
| [0.4, 0.6) | 1884 | 0 | 5 | 100.0% |
| [0.6, 0.8) | 12 | 2 | 2 | 85.7% |
| [0.8, 1.0] | 0 | 2 | 0 | 0.0% |

Conflicts: 11 in total; 11 sampled below for manual inspection.

| target | location | our edge kind | enclosing symbol | our target(s) | SCIP target(s) |
|---|---|---|---|---|---|
| acme | acme-api:src/orders/orders.controller.spec.ts:6 | instantiates | ts:acme-api:acme-api:src/orders/orders.controller.spec::OrdersController#test_suite | ts:acme-api:acme-api:src/orders/orders.controller::OrdersController#class [resolved] | ts:acme-api:acme-api:src/orders/orders.controller::OrdersController::constructor#method |
| acme | acme-api:src/users/users.service.spec.ts:8 | instantiates | ts:acme-api:acme-api:src/users/users.service.spec::UsersService#test_suite | ts:acme-api:acme-api:src/users/users.service::UsersService#class [resolved] | ts:acme-api:acme-api:src/users/users.service::UsersService::constructor#method |
| acme | acme-sdk:src/client.test.ts:11 | references | ts:acme-sdk:@acme/sdk:src/client.test::AcmeClient::should send the bearer token in the Authorization header#test | ts:acme-sdk:@acme/sdk:src/client::AcmeClient#class [heuristic 0.60] | ts:acme-sdk:@acme/sdk:src/client::AcmeClient::constructor#method |
| acme | acme-sdk:src/client.test.ts:19 | references | ts:acme-sdk:@acme/sdk:src/client.test::AcmeClient::should send the api key in the x-api-key header#test | ts:acme-sdk:@acme/sdk:src/client::AcmeClient#class [heuristic 0.60] | ts:acme-sdk:@acme/sdk:src/client::AcmeClient::constructor#method |
| acme | acme-web:src/hooks/useCreateOrder.ts:12 | calls | ts:acme-web:acme-web:src/hooks/useCreateOrder::useCreateOrder#function | ts:acme-sdk:@acme/sdk:src/client::AcmeClient::createOrder#method [heuristic 0.85] | ts:acme-web:acme-web:src/api/client::acme#constant |
| acme | acme-web:src/hooks/useUser.ts:11 | calls | ts:acme-web:acme-web:src/hooks/useUser::useUser#function | ts:acme-sdk:@acme/sdk:src/client::AcmeClient::getUser#method [heuristic 0.85] | ts:acme-web:acme-web:src/api/client::acme#constant |
| workspace | code-atlas:crates/atlas-core/src/facts.rs:272 | calls | rust:code-atlas:atlas-core:facts::ImplInfo::item_prefix#method | rust:code-atlas:atlas-core:keys::SymbolKey::name#method [heuristic 0.25]; rust:code-atlas:atlas-core:facts::ReferenceFact::name#method [heuristic 0.25] | rust:code-atlas:atlas-core:facts::TypePath::name#method |
| workspace | code-atlas:crates/atlas-engine/tests/incremental_equivalence.rs:27 | calls | rust:code-atlas:atlas-engine/test/incremental_equivalence:::assert_equivalent#function | rust:code-atlas:atlas-project:home::AtlasHome::db_path#method [heuristic 0.35] | rust:code-atlas:atlas-engine:engine::Engine::db_path#method |
| workspace | code-atlas:crates/atlas-engine/tests/regressions.rs:186 | calls | rust:code-atlas:atlas-engine/test/regressions:::should_re_resolve_a_file_whose_detector_key_target_another_file_emits#test | rust:code-atlas:atlas-semantic:engine::SemanticEngine::store#method [heuristic 0.35] | rust:code-atlas:atlas-engine:engine::Engine::store#method |
| workspace | code-atlas:crates/atlas-engine/tests/regressions.rs:33 | calls | rust:code-atlas:atlas-engine/test/regressions:::mismatch_with#function | rust:code-atlas:atlas-project:home::AtlasHome::db_path#method [heuristic 0.35] | rust:code-atlas:atlas-engine:engine::Engine::db_path#method |
| workspace | code-atlas:crates/atlas-scip/src/typescript.rs:136 | references | rust:code-atlas:atlas-scip:typescript::ScipTypescript::<ScipIndexer>::index#method | rust:code-atlas:atlas-scip:typescript::classify_failure#function [resolved] | rust:code-atlas:atlas-scip:indexer::CachedInvocation::classify_failure#field |

Unconfirmed resolved edges (ours-only, same package): 682 in total, 20 sampled evenly for manual inspection.

| target | location | edge kind | enclosing symbol | our target(s) | SCIP target(s) |
|---|---|---|---|---|---|
| acme | acme-billing:src/acme_client.rs:21 | instantiates | rust:acme-billing:acme-billing:acme_client::AcmeApiClient::new#method | rust:acme-billing:acme-billing:acme_client::AcmeApiClient#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-core/src/provenance.rs:243 | instantiates | rust:code-atlas:atlas-core:provenance::Producer::new#method | rust:code-atlas:atlas-core:provenance::Producer#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-engine/tests/engine.rs:362 | calls | rust:code-atlas:atlas-engine/test/engine:::should_refresh_after_the_watch_debounce_and_rescan_after_a_watcher_error#test | rust:code-atlas:atlas-engine/test/engine:::named#function [resolved] |  |
| workspace | code-atlas:crates/atlas-engine/tests/scip.rs:63 | calls | rust:code-atlas:atlas-engine/test/scip:::index_bytes#function | rust:code-atlas:atlas-engine/test/scip:::occurrence#function [resolved] |  |
| workspace | code-atlas:crates/atlas-eval/src/agent/transcript.rs:161 | references | rust:code-atlas:atlas-eval:agent/transcript::Transcript::turn_usage_total#method | rust:code-atlas:atlas-eval:agent/transcript::TokenUsage#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-eval/src/quality/metrics.rs:345 | references | rust:code-atlas:atlas-eval:quality/metrics/tests::should_sum_recall_tallies#test | rust:code-atlas:atlas-eval:quality/metrics::Recall#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-lang-rust/src/extract.rs:100 | instantiates | rust:code-atlas:atlas-lang-rust:extract::Extractor::new#method | rust:code-atlas:atlas-lang-rust:extract::Extractor#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-mcp/src/session.rs:32 | instantiates | rust:code-atlas:atlas-mcp:session::Session::open#method | rust:code-atlas:atlas-mcp:session::Session#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-query/src/model.rs:118 | instantiates | rust:code-atlas:atlas-query:model::CertaintyTag::inferred#method | rust:code-atlas:atlas-query:model::CertaintyTag#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-query/src/text/impact.rs:44 | references | rust:code-atlas:atlas-query:text/impact::ImpactAnswer::<RenderText>::render_text#method | rust:code-atlas:atlas-query:text::TextOut#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/acme.rs:111 | references | rust:code-atlas:atlas-resolve/test/acme:::should_resolve_decorators_and_guard_references#test | rust:code-atlas:atlas-resolve/test/acme:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/adversarial.rs:69 | references | rust:code-atlas:atlas-resolve/test/adversarial:trait_dispatch::should_derive_implements_edges_from_impl_blocks#test | rust:code-atlas:atlas-resolve/test/adversarial:::STRUCTURAL#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/review_regressions.rs:239 | calls | rust:code-atlas:atlas-resolve/test/review_regressions:::should_keep_ts_value_and_type_namespaces_apart_in_either_declaration_order#test | rust:code-atlas:atlas-resolve/test/review_regressions:::app#function [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/review_regressions.rs:481 | references | rust:code-atlas:atlas-resolve/test/review_regressions:::should_resolve_mutual_glob_and_star_webs_quickly#test | rust:code-atlas:atlas-resolve/test/review_regressions:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/rust_rules.rs:126 | references | rust:code-atlas:atlas-resolve/test/rust_rules:::should_resolve_self_paths_fields_and_deref_wrappers#test | rust:code-atlas:atlas-resolve/test/rust_rules:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/ts_rules.rs:165 | calls | rust:code-atlas:atlas-resolve/test/ts_rules:::should_resolve_members_through_this_fields_extends_super_and_constructors#test | rust:code-atlas:atlas-resolve/test/ts_rules:::app#function [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/ts_rules.rs:351 | calls | rust:code-atlas:atlas-resolve/test/ts_rules:::should_reference_class_fields_read_through_this_including_inherited_ones#test | rust:code-atlas:atlas-resolve/test/ts_rules:::app#function [resolved] |  |
| workspace | code-atlas:crates/atlas-scip/tests/rust_analyzer.rs:64 | references | rust:code-atlas:atlas-scip/test/rust_analyzer:::analyze_fixture#function | rust:code-atlas:atlas-scip/test/rust_analyzer:::REPO#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-similarity/src/structural.rs:134 | instantiates | rust:code-atlas:atlas-similarity:structural::StructuralStore::new#method | rust:code-atlas:atlas-similarity:structural::StructuralStore#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-store/tests/inferred.rs:58 | references | rust:code-atlas:atlas-store/test/inferred:::should_store_inferred_facts_with_their_subjects_and_evidence#test | rust:code-atlas:atlas-store:inferred::InferredQuery#struct [resolved] |  |

SCIP-only edges (missed by the resolver): 31821 in total, 20 sampled evenly for manual inspection.

| target | location | edge kind | enclosing symbol | our target(s) | SCIP target(s) |
|---|---|---|---|---|---|
| acme | acme-api:src/auth/api-key.guard.ts:11 | references | ts:acme-api:acme-api:src/auth/api-key.guard::ApiKeyGuard::canActivate#method |  | ts:acme-api:acme-api:src/auth/api-key.guard::ApiKeyGuard::configService#field |
| workspace | code-atlas:crates/atlas-detect/src/detectors/axum.rs:202 | references | rust:code-atlas:atlas-detect:detectors/axum::AxumEndpointDetector::emit_attribute_routes#method |  | rust:code-atlas:atlas-core:facts::SymbolFact::kind#field |
| workspace | code-atlas:crates/atlas-detect/src/view.rs:168 | references | rust:code-atlas:atlas-detect:view::FileView::rust_use#method |  | rust:code-atlas:atlas-core:facts::UseBinding#enum |
| workspace | code-atlas:crates/atlas-engine/src/run.rs:1224 | references | rust:code-atlas:atlas-engine:run::apply_write#function |  | rust:code-atlas:atlas-engine:run::FileUpdate::record#field |
| workspace | code-atlas:crates/atlas-engine/tests/engine.rs:535 | references | rust:code-atlas:atlas-engine/test/engine:::should_report_status_and_run_ledger#test |  | rust:code-atlas:atlas-engine:report::IndexStatus::files_by_language#field |
| workspace | code-atlas:crates/atlas-eval/src/agent/report.rs:152 | references | rust:code-atlas:atlas-eval:agent/report::METRICS#constant |  | rust:code-atlas:atlas-eval:agent/report::Better#enum |
| workspace | code-atlas:crates/atlas-eval/src/agent/workspace.rs:418 | references | rust:code-atlas:atlas-eval:agent/workspace/tests::should_copy_an_override_directory_without_git_or_build_output#test |  | rust:code-atlas:atlas-eval:agent/workspace::TreeSource#enum |
| workspace | code-atlas:crates/atlas-eval/src/quality/metrics.rs:411 | references | rust:code-atlas:atlas-eval:quality/metrics/tests::should_separate_true_relation_from_correct_claim#test |  | rust:code-atlas:atlas-eval:quality/metrics::ClassCalibration::correct#field |
| workspace | code-atlas:crates/atlas-eval/src/quality/selector.rs:105 | calls | rust:code-atlas:atlas-eval:quality/selector::matches#function |  | rust:code-atlas:atlas-core:ids::RelPath::as_str#method |
| workspace | code-atlas:crates/atlas-lang-rust/tests/references.rs:85 | references | rust:code-atlas:atlas-lang-rust/test/references:::should_emit_a_method_call_for_every_link_of_a_call_chain#test |  | rust:code-atlas:atlas-core:facts::RefKind#enum |
| workspace | code-atlas:crates/atlas-lang-ts/tests/symbols.rs:131 | references | rust:code-atlas:atlas-lang-ts/test/symbols:::should_extract_functions_generators_and_function_valued_variables#test |  | rust:code-atlas:atlas-core:facts::SymbolFact::visibility#field |
| workspace | code-atlas:crates/atlas-mcp/src/server.rs:80 | calls | rust:code-atlas:atlas-mcp:server::AtlasServer::run#method |  | rust:code-atlas:atlas-engine:engine::Engine::ensure_fresh#method |
| workspace | code-atlas:crates/atlas-project/src/registry.rs:665 | references | rust:code-atlas:atlas-project:registry/tests::should_reject_duplicates_in_a_hand_edited_file#test |  | rust:code-atlas:atlas-project:error::ProjectError::RegistryParse#variant |
| workspace | code-atlas:crates/atlas-query/src/queries/explain.rs:200 | references | rust:code-atlas:atlas-query:queries/explain::explain_edge#function |  | rust:code-atlas:atlas-core:graph::Edge::certainty#field |
| workspace | code-atlas:crates/atlas-query/src/text/api.rs:103 | references | rust:code-atlas:atlas-query:text/api::ApiAnswer::<RenderText>::render_text#method |  | rust:code-atlas:atlas-query:queries/api::SchemeUsage::supported#field |
| workspace | code-atlas:crates/atlas-resolve/src/output.rs:304 | references | rust:code-atlas:atlas-resolve:output::Emitter::evidence#method |  | rust:code-atlas:atlas-resolve:engine::Resolver::surface#field |
| workspace | code-atlas:crates/atlas-resolve/src/table.rs:674 | references | rust:code-atlas:atlas-resolve:table::ProjectSymbolTable::unindex_modules#method |  | rust:code-atlas:atlas-resolve:table::ProjectSymbolTable::modules#field |
| workspace | code-atlas:crates/atlas-scip/src/ingest.rs:388 | references | rust:code-atlas:atlas-scip:ingest::Ingestor::report_unmapped_definitions#method |  | rust:code-atlas:atlas-scip:ingest::DocumentView::file#field |
| workspace | code-atlas:crates/atlas-semantic/src/provider/unavailable.rs:24 | references | rust:code-atlas:atlas-semantic:provider/unavailable::UnavailableProvider::<LlmProvider>::availability#method |  | rust:code-atlas:atlas-core:traits::Availability#enum |
| workspace | code-atlas:crates/atlas-similarity/src/tests.rs:570 | references | rust:code-atlas:atlas-similarity:tests::should_sample_oversized_buckets_and_report_it#test |  | rust:code-atlas:atlas-similarity:structural::StructuralCluster::min_pair_jaccard#field |

## Refactor robustness

Each scenario edits a temporary copy of the acme fixtures, runs the incremental update and checks the expected graph deltas, incremental equals full (fact-set digest, lineage imported), no stale deterministic fact (every edge cites current file content), and that seeded inferred facts on changed evidence are no longer current.

| scenario | result | description | checks passed | error |
|---|---|---|---|---|
| rename_symbol_and_callers | PASS | rename acme-api formatMoney to formatCents in its definition and both caller files | 10/10 |  |
| move_file_and_fix_imports | PASS | move acme-api src/common/money.ts to src/shared/money.ts and update both importers | 12/12 |  |
| extract_function | PASS | extract the fraction parsing of acme-sdk toCents into a new function fractionCents | 10/10 |  |
| delete_function_and_its_calls | PASS | delete acme-api roundHalfUp and the call and import in ReportsService | 10/10 |  |
| change_route_literal | PASS | change the UsersController findOne route from ':id' to 'by-id/:id' | 11/11 |  |
| swap_guard | PASS | replace JwtAuthGuard by ApiKeyGuard on UsersController | 8/8 |  |

Scenarios passed: 6/6.

### rename_symbol_and_callers

- pass incremental update ran: 3 files updated
- pass old symbol key is gone: formatMoney
- pass new symbol key is present: formatCents
- pass callers are re-pointed at the new symbol: OrdersService.findOne and ReportsService.exportCsv call formatCents
- pass no edge still targets the old symbol: edges other than renamed_from lineage that target the old entity id
- pass renamed_from lineage links new to old with confidence >= 0.85: confidence Some(1.0)
- pass incremental index equals a full index (digest): digest 0e1afe473897a377c243f7dbe35211ce177ac2dc86eb56c8e7c47f3e43ce2832
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Current, NeedsRevalidation { reason: "subject renamed from ts:acme-api:acme-api:src/common/money::formatMoney#function" }]; changed subject Some(EntityId(8206fc974d98b462))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### move_file_and_fix_imports

- pass incremental update ran: 4 files updated
- pass old file entity is gone: src/common/money.ts
- pass new file entity is present: src/shared/money.ts
- pass symbol exists under its new key: formatMoney at src/shared/money.ts
- pass caller edge is re-pointed: OrdersService.findOne -> formatMoney
- pass import edge is re-pointed: orders.service.ts imports shared/money.ts
- pass no edge still targets the old symbol: edges other than renamed_from lineage that target the old entity id
- pass renamed_from lineage links the moved symbol to its old key: confidence Some(1.0)
- pass incremental index equals a full index (digest): digest 529bec31c28883151bad329945c0b97b8ac09c2c7494f545e531f56f1f82bf8f
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Current, NeedsRevalidation { reason: "subject renamed from ts:acme-api:acme-api:src/common/money::formatMoney#function" }]; changed subject Some(EntityId(8206fc974d98b462))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### extract_function

- pass incremental update ran: 1 files updated
- pass new function entity is present: fractionCents
- pass original symbol key is unchanged: toCents
- pass original calls the extracted function: toCents -> fractionCents
- pass callers of the original keep their edge: OrderForm -> toCents
- pass no rename is inferred for a pure extraction: 0 renamed_from edges
- pass incremental index equals a full index (digest): digest 1f4e9064d51efd6f375e0b30a01543a0f25355ec4b61b6ff9c6af4c36adea626
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Stale { reason: "acme-sdk:src/money.ts changed" }, Current]; changed subject Some(EntityId(d209f398d5da9e95))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### delete_function_and_its_calls

- pass incremental update ran: 2 files updated
- pass deleted symbol key is gone: roundHalfUp
- pass no edge still targets the deleted symbol: edges other than renamed_from lineage that target the old entity id
- pass the unrelated sibling survives: roundHalfEven
- pass the caller no longer references the sibling by mistake: ReportsService.exportCsv -/-> roundHalfEven
- pass a deletion is not recorded as a rename: 0 renamed_from edges
- pass incremental index equals a full index (digest): digest 842691288e8f10a15ada30cb727d8f314bd74c69b4bd1f95819ee7c3e257da94
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Stale { reason: "acme-api:src/billing/rounding.ts changed" }, Current]; changed subject Some(EntityId(e7da81986fea6bfb))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### change_route_literal

- pass incremental update ran: 1 files updated
- pass old endpoint key is gone: GET /users/:id
- pass new endpoint key is present: GET /users/by-id/:id
- pass the handler edge is re-pointed: UsersController.findOne exposes the new endpoint
- pass no edge still targets the old endpoint: edges to the old endpoint id
- pass guard-derived auth follows the new endpoint: supports_auth bearer on the new endpoint
- pass the SDK is no longer linked to the removed route: AcmeClient.getUser -/-> GET /users/:id
- pass incremental index equals a full index (digest): digest 7d28d5a71cb99d4a65bc8f4c81d4394926bafbbde028250034debf50268ce83a
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Stale { reason: "acme-api:src/users/users.controller.ts changed" }, Current]; changed subject Some(EntityId(e485a4bb18a317ae))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### swap_guard

- pass incremental update ran: 1 files updated
- pass the endpoint requires the new guard: requires_guard ApiKeyGuard
- pass the endpoint no longer requires the old guard: requires_guard JwtAuthGuard
- pass supported schemes follow the guard: api_key yes, bearer no
- pass incremental index equals a full index (digest): digest 13de31319a688f0f4c7168283f368dba951e1ae5c92e8b8f5cc339ba71758d9f
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Stale { reason: "acme-api:src/users/users.controller.ts changed" }, Current]; changed subject Some(EntityId(e485a4bb18a317ae))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

## Context selection quality

| selection | tasks | precision | recall |
|---|---|---|---|
| MUST READ files | 1 | 20.0% | 50.0% |
| SKIM files | 1 | 10.0% | 50.0% |
| MUST READ + SKIM files | 1 | 10.0% | 50.0% |
| MUST READ symbols | 0 | n/a | n/a |
| SKIM symbols | 0 | n/a | n/a |
| MUST READ + SKIM symbols | 0 | n/a | n/a |

| task | files (precision / recall, combined) | symbols (precision / recall, combined) |
|---|---|---|
| t00-smoke | 10.0% / 50.0% | n/a / n/a |


