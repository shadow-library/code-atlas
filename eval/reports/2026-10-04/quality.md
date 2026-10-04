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
| adversarial/trait-dispatch-rust | 15/15 | 8/8 | 100.0% | 0 | 0 | 0 | 0/10 (+0 vacuous, 0 unbound) | 0/0 | 0 | 0 |
| **all fixtures** | 184/184 | 98/98 | 100.0% | 0 | 0 | 0 | 0/63 (+12 vacuous, 0 unbound) | 14/15 | 0 | 0 |

## Edge kinds (all fixtures)

| edge kind | expected | found | wrong certainty | wrong detail | missing | unbound | recall (bound) | recall (all) | optional present/expected | forbidden violations/checked | exhaustive unexpected |
|---|---|---|---|---|---|---|---|---|---|---|---|
| calls | 32 | 32 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 10/10 | 0/31 (+1 vacuous, 0 unbound) | not declared |
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
| adversarial/trait-dispatch-rust | calls | 6/6 | 0 | 0 | 0 | 100.0% | 0/10 (+0 vacuous, 0 unbound) |
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
| heuristic | 46 | 46 | 0 | 0 | 0 | 100.0% | 100.0% |
| resolved | 43 | 43 | 0 | 0 | 0 | 100.0% | 100.0% |
| syntactic | 19 | 19 | 0 | 0 | 0 | 100.0% | 100.0% |

Heuristic edges by confidence:

| confidence | judged | correct | false | true relation % |
|---|---|---|---|---|
| [0.0, 0.4) | 10 | 10 | 0 | 100.0% |
| [0.4, 0.6) | 1 | 1 | 0 | 100.0% |
| [0.6, 0.8) | 13 | 13 | 0 | 100.0% |
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

## Resolver versus the compiler oracle (SCIP)

Headline (calls + references + instantiates, all targets):

- resolved precision: 97.6% (confirmable edges, 42537 of 43286) / 95.9% all-in; 41529 agreed, 3 conflicting, 1005 same-package unconfirmed, 749 unconfirmable (141 cross-package, 608 file-level).
- heuristic precision: 92.6% (confirmable edges, 6301 of 7037) / 82.9% all-in; 5834 agreed, 9 conflicting, 458 same-package unconfirmed, 736 unconfirmable (736 cross-package, 0 file-level).
- recall of SCIP's edges: 53.3% by resolved edges, 60.8% by all of our edges.

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
| workspace | code-atlas:/ rust-analyzer indexed 442 files, 56639 edges ingested, 0 files without usable facts (443 files); code-atlas:crates/atlas-resolve/tests/pin-corpus/app/ scip-typescript indexed 1 files, 1 edges ingested, 0 files without usable facts (1 files) | 443 |  |

| edge kind / our class | agreed | ours-only | ours-only cross-package | ours-only file-level | scip-only | conflicting | precision (confirmable-only) | precision (all-in) | recall vs SCIP |
|---|---|---|---|---|---|---|---|---|---|
| calls / all | 20356 (60 respanned) | 766 | 718 | 5 | 2244 | 7 | 96.3% | 93.2% | 90.1% |
| calls / resolved | 17143 (60 respanned) | 320 | 0 | 5 | 0 | 0 | 98.2% | 98.1% | 75.8% |
| calls / heuristic | 3213 (0 respanned) | 446 | 718 | 0 | 0 | 7 | 87.6% | 73.3% | 14.2% |
| implements / all | 46 (0 respanned) | 52 | 64 | 0 | 0 | 0 | 46.9% | 28.4% | 100.0% |
| implements / resolved | 46 (0 respanned) | 52 | 64 | 0 | 0 | 0 | 46.9% | 28.4% | 100.0% |
| implements / heuristic | 0 (0 respanned) | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 0.0% |
| instantiates / all | 3458 (2 respanned) | 216 | 1 | 0 | 21 | 2 | 94.1% | 94.0% | 99.3% |
| instantiates / resolved | 3453 (2 respanned) | 216 | 1 | 0 | 0 | 2 | 94.1% | 94.0% | 99.1% |
| instantiates / heuristic | 5 (0 respanned) | 0 | 0 | 0 | 0 | 0 | 100.0% | 100.0% | 0.1% |
| references / all | 23549 (10 respanned) | 481 | 158 | 603 | 28238 | 3 | 98.0% | 95.0% | 45.5% |
| references / resolved | 20933 (9 respanned) | 469 | 140 | 603 | 0 | 1 | 97.8% | 94.5% | 40.4% |
| references / heuristic | 2616 (1 respanned) | 12 | 18 | 0 | 0 | 2 | 99.5% | 98.8% | 5.1% |
| **calls+references+instantiates / all** | 47363 (72 respanned) | 1463 | 877 | 608 | 30503 | 12 | 97.0% | 94.1% | 60.8% |
| **calls+references+instantiates / resolved** | 41529 (71 respanned) | 1005 | 141 | 608 | 0 | 3 | 97.6% | 95.9% | 53.3% |
| **calls+references+instantiates / heuristic** | 5834 (1 respanned) | 458 | 736 | 0 | 0 | 9 | 92.6% | 82.9% | 7.5% |

SCIP indexes used (blake3 of each `.scip`; the cache key encodes the inputs, so identical inputs reproduce the digest):

| target | repo | root | indexer | blake3 |
|---|---|---|---|---|
| acme | acme-api | / | scip-typescript | 17b16d40084dbfc3cdc9607d94aa6a346df4098d803cc2689ba05a00455fabd6 |
| acme | acme-sdk | / | scip-typescript | ca15255bac307401539c6397cd6d4c0c695902cd1af67b3eb8e457474c6e1814 |
| acme | acme-web | / | scip-typescript | 80c62b9b57866c8ded4ab1d4561a5288f54b1d8bf65c23224fbdd82dffc904b3 |
| acme | acme-billing | / | rust-analyzer | 0a32989b736fc474752bcca6fb1ec25b89181a64476e449391d88b2ac4cb229e |
| globex | globex-api | / | scip-typescript | 8bbf8d70dc105ea4262774439a01887baa942843946ac0d9192dd9f57cea43ff |
| globex | globex-worker | / | rust-analyzer | fbf85aa465ddc3777ea11a76fa38e8d483a77f75f7228c3b3358c6efb225cd7c |
| adversarial/computed-routes-ts | computed-routes-ts | / | scip-typescript | f797f003cf77fcee89a4810c351c702f51c66e5ed74d4580f93ca2c1208583e1 |
| adversarial/duplicate-names-rust | duplicate-names-rust | / | rust-analyzer | 3b8644354b955981808d413bc477285f6ed57e6e452ea6c9b294da8f1c279b7c |
| adversarial/dynamic-ts | dynamic-ts | / | scip-typescript | 3165f9cc0c56414a0e3edec72961bf2b3b99d85febbcb74f27d6fbb7424e0d07 |
| adversarial/generated-code | generated-code | rust/ | rust-analyzer | 97d2cd41ce76b84ab56cef906c2a06f68f0aa4893b2068992f2ec4e22590cc9c |
| adversarial/generated-code | generated-code | / | scip-typescript | fe99033261bbb19e90425ca08a34eed28744024d04fa2747b38df4dc771ebcf3 |
| adversarial/macro-heavy-rust | macro-heavy-rust | / | rust-analyzer | f2adf5e2b2fb211d0b0a88d77ef8baf56940b5bfd3982164ea0feeda16cf539f |
| adversarial/reexport-chain-ts | reexport-chain-ts | / | scip-typescript | 1ce54e70442ef67c3fa4ac87d3fd6ed2b952054252db58b65e3fb4810a842dd3 |
| adversarial/shadowing-ts | shadowing-ts | / | scip-typescript | 8c8f8997c37d71659a6b335c9806bb72d140314ad0904a3c859ab4444ab75ba8 |
| adversarial/similar-not-equivalent | similar-not-equivalent | / | scip-typescript | 8f605aa258bd933ca9a33ecf37506596e2e87bf28251b428baa6212529e40a58 |
| adversarial/trait-dispatch-rust | trait-dispatch-rust | / | rust-analyzer | 8aa9c77a26f8beb9183fb7a7324e478b5ac8a1ab93ac0403c471f9418bc4240d |
| workspace | code-atlas | / | rust-analyzer | af5681e4b9ff87faa49bf221b23afb8dd4e0240388cba93e9d346b102f21242e |
| workspace | code-atlas | crates/atlas-resolve/tests/pin-corpus/app/ | scip-typescript | 0d4a71ddfc3c45034252af711adc1fd77c4bbec790719023e4bcbcc4bdd13b1b |

Heuristic edges by confidence against the oracle (unconfirmed = same-package ours-only):

| confidence | agreed | conflicting | unconfirmed | agreed / (agreed + conflicting) |
|---|---|---|---|---|
| [0.0, 0.4) | 668 | 5 | 431 | 99.3% |
| [0.4, 0.6) | 5155 | 0 | 25 | 100.0% |
| [0.6, 0.8) | 11 | 2 | 2 | 84.6% |
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
| workspace | code-atlas:crates/atlas-scip/src/typescript.rs:141 | references | rust:code-atlas:atlas-scip:typescript::ScipTypescript::<ScipIndexer>::index_with_cancel#method | rust:code-atlas:atlas-scip:typescript::classify_failure#function [resolved] | rust:code-atlas:atlas-scip:indexer::CachedInvocation::classify_failure#field |

Unconfirmed resolved edges (ours-only, same package): 1005 in total, 20 sampled evenly for manual inspection.

| target | location | edge kind | enclosing symbol | our target(s) | SCIP target(s) |
|---|---|---|---|---|---|
| acme | acme-billing:src/acme_client.rs:21 | instantiates | rust:acme-billing:acme-billing:acme_client::AcmeApiClient::new#method | rust:acme-billing:acme-billing:acme_client::AcmeApiClient#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-cli/tests/e2e_support/mod.rs:262 | instantiates | rust:code-atlas:atlas-cli/test/e2e_adversarial:e2e_support::Found::of#method | rust:code-atlas:atlas-cli/test/e2e_adversarial:e2e_support::Found#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-engine/src/enrich.rs:335 | references | rust:code-atlas:atlas-engine:enrich::Engine::enrich#method | rust:code-atlas:atlas-engine:enrich/sources::Sources#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-engine/tests/auto_scip.rs:341 | calls | rust:code-atlas:atlas-engine/test/auto_scip:::should_skip_a_failing_package_until_its_inputs_change#test | rust:code-atlas:atlas-engine/test/auto_scip:::wait_until#function [resolved] |  |
| workspace | code-atlas:crates/atlas-engine/tests/regressions.rs:171 | calls | rust:code-atlas:atlas-engine/test/regressions:::should_re_resolve_a_file_whose_detector_key_target_another_file_emits#test | rust:code-atlas:atlas-engine/test/regressions:::named#function [resolved] |  |
| workspace | code-atlas:crates/atlas-eval/src/agent/bash.rs:293 | references | rust:code-atlas:atlas-eval:agent/bash/tests::should_extract_sed_print_targets_but_not_edits#test | rust:code-atlas:atlas-eval:agent/bash::BashAnalysis#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-eval/src/quality/evaluate/unresolved.rs:18 | references | rust:code-atlas:atlas-eval:quality/evaluate/unresolved::evaluate#function | rust:code-atlas:atlas-eval:quality/evaluate::UnresolvedScore#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-lang-rust/src/macros.rs:193 | instantiates | rust:code-atlas:atlas-lang-rust:macros::ParsedMacro::parse#method | rust:code-atlas:atlas-lang-rust:macros::ParsedMacro#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-project/src/manifest/types.rs:21 | instantiates | rust:code-atlas:atlas-project:manifest/types::RepoDir::of_file#method | rust:code-atlas:atlas-project:manifest/types::RepoDir#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-query/src/queries/textual.rs:266 | instantiates | rust:code-atlas:atlas-query:queries/textual::Matcher::new#method | rust:code-atlas:atlas-query:queries/textual::Matcher#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-query/tests/search_ranking.rs:19 | references | rust:code-atlas:atlas-query/test/search_ranking:::should_not_treat_a_single_token_name_hit_as_an_exact_match_for_a_multi_word_query#test | rust:code-atlas:atlas-query/test/search_ranking:::SOURCE#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/src/rust.rs:1441 | references | rust:code-atlas:atlas-resolve:rust::Resolver::rust_use_targets#method | rust:code-atlas:atlas-resolve:engine::Guard#struct [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/adversarial.rs:239 | references | rust:code-atlas:atlas-resolve/test/adversarial:dynamic::should_resolve_the_static_namespace_call#test | rust:code-atlas:atlas-resolve/test/adversarial:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/review_regressions.rs:251 | references | rust:code-atlas:atlas-resolve/test/review_regressions:::should_separate_static_and_instance_ts_members#test | rust:code-atlas:atlas-resolve/test/review_regressions:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/review_regressions.rs:735 | references | rust:code-atlas:atlas-resolve/test/review_regressions:::should_weaken_a_glob_of_a_fn_local_module_that_shares_a_module_level_name#test | rust:code-atlas:atlas-resolve/test/review_regressions:::AUTHORITATIVE#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/rust_rules.rs:438 | references | rust:code-atlas:atlas-resolve/test/rust_rules:::should_resolve_field_accesses_on_typed_receivers_to_field_symbols#test | rust:code-atlas:atlas-resolve/test/rust_rules:::RESOLVED#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-resolve/tests/ts_rules.rs:315 | calls | rust:code-atlas:atlas-resolve/test/ts_rules:::should_keep_the_class_edge_when_a_static_comes_from_an_external_base#test | rust:code-atlas:atlas-resolve/test/ts_rules:::app#function [resolved] |  |
| workspace | code-atlas:crates/atlas-scip/tests/scip_typescript.rs:62 | references | rust:code-atlas:atlas-scip/test/scip_typescript:::should_resolve_the_alias_chain_reference_to_the_original_definition#test | rust:code-atlas:atlas-scip/test/scip_typescript:::REPO#constant [resolved] |  |
| workspace | code-atlas:crates/atlas-store/src/store.rs:411 | references | rust:code-atlas:atlas-store:store::run_write#function | rust:code-atlas:atlas-store:error::StoreError#enum [resolved] |  |
| workspace | code-atlas:crates/atlas-store/tests/encoding.rs:697 | calls | rust:code-atlas:atlas-store/test/encoding:::should_reject_a_core_serde_row_stored_under_another_id#test | rust:code-atlas:atlas-store/test/encoding:::owner_b#function [resolved] |  |

SCIP-only edges (missed by the resolver): 30503 in total, 20 sampled evenly for manual inspection.

| target | location | edge kind | enclosing symbol | our target(s) | SCIP target(s) |
|---|---|---|---|---|---|
| acme | acme-api:src/auth/api-key.guard.ts:11 | references | ts:acme-api:acme-api:src/auth/api-key.guard::ApiKeyGuard::canActivate#method |  | ts:acme-api:acme-api:src/auth/api-key.guard::ApiKeyGuard::configService#field |
| workspace | code-atlas:crates/atlas-cli/src/cli.rs:338 | references | rust:code-atlas:atlas-cli/bin/atlas-cli:cli::ImpactArgs::filter#field |  | rust:code-atlas:atlas-cli/bin/atlas-cli:cli::TargetFilterArgs#struct |
| workspace | code-atlas:crates/atlas-core/src/facts.rs:590 | references | rust:code-atlas:atlas-core:facts::ImplFact::scope#field |  | rust:code-atlas:atlas-core:facts::LocalSymbolId#struct |
| workspace | code-atlas:crates/atlas-detect/src/detectors/express.rs:149 | references | rust:code-atlas:atlas-detect:detectors/express::ExpressEndpointDetector::emit_registration#method |  | rust:code-atlas:atlas-core:facts::ReferenceFact::from#field |
| workspace | code-atlas:crates/atlas-engine/src/enrich.rs:450 | references | rust:code-atlas:atlas-engine:enrich::Engine::settle_link_facts#method |  | rust:code-atlas:atlas-store:inferred::FreshnessPolicy#enum |
| workspace | code-atlas:crates/atlas-engine/src/run.rs:69 | references | rust:code-atlas:atlas-engine:run::Ledger::record_delta#method |  | rust:code-atlas:atlas-store:facts::FactDelta#struct |
| workspace | code-atlas:crates/atlas-engine/tests/enrich.rs:59 | references | rust:code-atlas:atlas-engine/test/enrich:::budget#function |  | rust:code-atlas:atlas-engine:enrich::EnrichOptions::summaries#field |
| workspace | code-atlas:crates/atlas-eval/src/agent/report.rs:165 | references | rust:code-atlas:atlas-eval:agent/report::METRICS#constant |  | rust:code-atlas:atlas-eval:agent/transcript::TokenUsage::input#field |
| workspace | code-atlas:crates/atlas-eval/src/quality/evaluate/context.rs:31 | references | rust:code-atlas:atlas-eval:quality/evaluate/context::Ctx::missing#field |  | rust:code-atlas:atlas-eval:quality/truth::Selector#struct |
| workspace | code-atlas:crates/atlas-eval/src/quality/report.rs:218 | references | rust:code-atlas:atlas-eval:quality/report::oracle_metrics#function |  | rust:code-atlas:atlas-eval:quality/oracle::ClassSplit::all#field |
| workspace | code-atlas:crates/atlas-lang-rust/tests/robustness.rs:98 | references | rust:code-atlas:atlas-lang-rust/test/robustness:::should_never_panic_and_always_satisfy_invariants_on_mutated_sources#test |  | rust:code-atlas:atlas-core:traits::AnalysisInput::source#field |
| workspace | code-atlas:crates/atlas-link/src/auth.rs:518 | references | rust:code-atlas:atlas-link:auth::option_usages#function |  | rust:code-atlas:atlas-core:graph::Edge::certainty#field |
| workspace | code-atlas:crates/atlas-project/src/manifest/json.rs:231 | calls | rust:code-atlas:atlas-project:manifest/json/tests::should_strip_line_and_block_comments#test |  | rust:code-atlas:atlas-project:manifest/json::Json::get#method |
| workspace | code-atlas:crates/atlas-query/src/queries/context.rs:1104 | references | rust:code-atlas:atlas-query:queries/context::route_seeds#function |  | rust:code-atlas:atlas-query:queries/context::Node::named#field |
| workspace | code-atlas:crates/atlas-query/src/queries/status.rs:144 | references | rust:code-atlas:atlas-query:queries/status::QueryEngine::status#method |  | rust:code-atlas:atlas-query:queries/status::StatusAnswer::dangling_detector_edges_dropped#field |
| workspace | code-atlas:crates/atlas-query/tests/inferred_links.rs:214 | references | rust:code-atlas:atlas-query/test/inferred_links:::should_drop_an_inferred_link_at_query_time_once_a_deterministic_link_exists#test |  | rust:code-atlas:atlas-query:queries/api::Related::entity#field |
| workspace | code-atlas:crates/atlas-resolve/src/surface.rs:365 | references | rust:code-atlas:atlas-resolve:surface::is_scope_item#function |  | rust:code-atlas:atlas-resolve:surface::SymSurface#struct |
| workspace | code-atlas:crates/atlas-scip/src/indexer.rs:515 | references | rust:code-atlas:atlas-scip:indexer/tests::should_never_prune_the_entry_it_just_published#test |  | rust:code-atlas:atlas-scip:indexer::ScipRequest::cache_dir#field |
| workspace | code-atlas:crates/atlas-semantic/src/task/similarity.rs:139 | references | rust:code-atlas:atlas-semantic:task/similarity::SimilarityPair::render#method |  | rust:code-atlas:atlas-semantic:task::RenderedPrompt::template_version#field |
| workspace | code-atlas:crates/atlas-store/src/facts.rs:797 | references | rust:code-atlas:atlas-store:facts::ReadTx::owned_facts#method |  | rust:code-atlas:atlas-store:facts::OwnedFacts::edges#field |

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
- pass incremental index equals a full index (digest): digest 9b4287a1fe34c9bdf7a8f406c988ef2e43be9187d72c40778a68ee6e40b27ddb
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
- pass incremental index equals a full index (digest): digest f0c9c3a527a6521aa81b118ec7f578349a12f5efada3192b167307f8d9643385
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
- pass incremental index equals a full index (digest): digest 06def942be7ba2f93dbe6732f712541b6c038224a4c1c3f0b3764460ef94b68b
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
- pass incremental index equals a full index (digest): digest 8371472e5a3386db6235348de8250f3f2a5322c3f1717154f57f79089953a722
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
- pass incremental index equals a full index (digest): digest db58e56e56a21d9792ae5b89ca8d692eadfd9dc67aaf70b9e5303d308cc72af8
- pass no stale deterministic fact survives: every deterministic edge cites current file content and every located entity's file exists
- pass inferred facts on changed evidence are stale: 0 fact(s) on changed evidence still current; freshness of all facts: [Stale { reason: "acme-api:src/users/users.controller.ts changed" }, Current]; changed subject Some(EntityId(e485a4bb18a317ae))
- pass inferred facts on unchanged evidence stay current: control fact on acme-billing money.rs

### swap_guard

- pass incremental update ran: 1 files updated
- pass the endpoint requires the new guard: requires_guard ApiKeyGuard
- pass the endpoint no longer requires the old guard: requires_guard JwtAuthGuard
- pass supported schemes follow the guard: api_key yes, bearer no
- pass incremental index equals a full index (digest): digest fa8d0b2de8c0f7984e45418760b32acfbd784eda4cf9b86567c9108655f1d575
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

- skipped `t01-cwd-relative-targets`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t02-typed-degraded-cause`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t03-nested-router-links`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t04-detector-import-gates`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t05-mcp-dependency-tools`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t06-repo-file-size-limit`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t07-static-precompressed-quality`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t08-cookie-samesite-default`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t09-lambda-text-content-types`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation
- skipped `t10-auth-challenge-quoting`: only `repo = { fixture = "..." }` tasks can be indexed by the quality evaluation

## Comparison with the previous report

**17 regressed, 30 improved, 349 unchanged, 0 added, 0 removed**

Regressed:

- REGRESSED oracle/calls/all/precision_all_in: 0.9488 -> 0.9315 (-0.0173, higher is better)
- REGRESSED oracle/calls/all/precision_confirmable: 0.9749 -> 0.9634 (-0.0115, higher is better)
- REGRESSED oracle/calls/all/recall: 0.9256 -> 0.9005 (-0.0251, higher is better)
- REGRESSED oracle/calls/heuristic/precision_all_in: 0.7637 -> 0.7329 (-0.0308, higher is better)
- REGRESSED oracle/calls/heuristic/precision_confirmable: 0.8921 -> 0.8764 (-0.0157, higher is better)
- REGRESSED oracle/calls/resolved/precision_all_in: 0.9904 -> 0.9814 (-0.0090, higher is better)
- REGRESSED oracle/calls/resolved/precision_confirmable: 0.9908 -> 0.9817 (-0.0091, higher is better)
- REGRESSED oracle/calls/resolved/recall: 0.7889 -> 0.7584 (-0.0305, higher is better)
- REGRESSED oracle/implements/all/precision_all_in: 0.3034 -> 0.2840 (-0.0195, higher is better)
- REGRESSED oracle/implements/all/precision_confirmable: 0.4783 -> 0.4694 (-0.0089, higher is better)
- REGRESSED oracle/implements/resolved/precision_all_in: 0.3034 -> 0.2840 (-0.0195, higher is better)
- REGRESSED oracle/implements/resolved/precision_confirmable: 0.4783 -> 0.4694 (-0.0089, higher is better)
- REGRESSED oracle/instantiates/all/precision_all_in: 0.9429 -> 0.9404 (-0.0025, higher is better)
- REGRESSED oracle/instantiates/all/precision_confirmable: 0.9429 -> 0.9407 (-0.0022, higher is better)
- REGRESSED oracle/instantiates/heuristic/recall: 0.0017 -> 0.0014 (-0.0003, higher is better)
- REGRESSED oracle/instantiates/resolved/precision_all_in: 0.9428 -> 0.9404 (-0.0025, higher is better)
- REGRESSED oracle/instantiates/resolved/precision_confirmable: 0.9428 -> 0.9406 (-0.0022, higher is better)

Improved:

- improved calibration/heuristic/correct: 0.9583 -> 1.0000 (+0.0417, higher is better)
- improved fixture/adversarial/trait-dispatch-rust/edge/calls/recall: 0.6667 -> 1.0000 (+0.3333, higher is better)
- improved fixture/adversarial/trait-dispatch-rust/edge/calls/recall_all: 0.6667 -> 1.0000 (+0.3333, higher is better)
- improved fixture/adversarial/trait-dispatch-rust/edges/recall: 0.7500 -> 1.0000 (+0.2500, higher is better)
- improved fixture/adversarial/trait-dispatch-rust/edges/recall_all: 0.7500 -> 1.0000 (+0.2500, higher is better)
- improved oracle/calls/heuristic/recall: 0.1367 -> 0.1421 (+0.0055, higher is better)
- improved oracle/instantiates/all/recall: 0.9922 -> 0.9928 (+0.0006, higher is better)
- improved oracle/instantiates/resolved/recall: 0.9905 -> 0.9914 (+0.0009, higher is better)
- improved oracle/references/all/precision_all_in: 0.9155 -> 0.9498 (+0.0343, higher is better)
- improved oracle/references/all/precision_confirmable: 0.9653 -> 0.9799 (+0.0146, higher is better)
- improved oracle/references/all/recall: 0.2613 -> 0.4547 (+0.1934, higher is better)
- improved oracle/references/heuristic/precision_all_in: 0.6944 -> 0.9879 (+0.2935, higher is better)
- improved oracle/references/heuristic/precision_confirmable: 0.8621 -> 0.9947 (+0.1326, higher is better)
- improved oracle/references/heuristic/recall: 0.0006 -> 0.0505 (+0.0499, higher is better)
- improved oracle/references/resolved/precision_all_in: 0.9161 -> 0.9452 (+0.0291, higher is better)
- improved oracle/references/resolved/precision_confirmable: 0.9655 -> 0.9780 (+0.0125, higher is better)
- improved oracle/references/resolved/recall: 0.2607 -> 0.4042 (+0.1435, higher is better)
- improved oracle/site_kinds/all/precision_all_in: 0.9356 -> 0.9412 (+0.0055, higher is better)
- improved oracle/site_kinds/all/precision_confirmable: 0.9680 -> 0.9698 (+0.0018, higher is better)
- improved oracle/site_kinds/all/recall: 0.4789 -> 0.6082 (+0.1293, higher is better)
- improved oracle/site_kinds/heuristic/precision_all_in: 0.7632 -> 0.8290 (+0.0658, higher is better)
- improved oracle/site_kinds/heuristic/precision_confirmable: 0.8920 -> 0.9259 (+0.0339, higher is better)
- improved oracle/site_kinds/heuristic/recall: 0.0380 -> 0.0749 (+0.0369, higher is better)
- improved oracle/site_kinds/resolved/precision_all_in: 0.9542 -> 0.9594 (+0.0052, higher is better)
- improved oracle/site_kinds/resolved/precision_confirmable: 0.9752 -> 0.9763 (+0.0011, higher is better)
- improved oracle/site_kinds/resolved/recall: 0.4409 -> 0.5333 (+0.0924, higher is better)
- improved total/edge/calls/recall: 0.9375 -> 1.0000 (+0.0625, higher is better)
- improved total/edge/calls/recall_all: 0.9375 -> 1.0000 (+0.0625, higher is better)
- improved total/edges/recall: 0.9796 -> 1.0000 (+0.0204, higher is better)
- improved total/edges/recall_all: 0.9796 -> 1.0000 (+0.0204, higher is better)

