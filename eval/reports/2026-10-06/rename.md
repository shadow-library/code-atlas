# Code Atlas rename evaluation

Date: 2026-10-06.

## Rename (dry runs through the language servers)

These fixture scenarios (acme, trait-dispatch-rust, ts-overrides) were used while the rename operation (B7) was developed, so the numbers are a regression check, not a held-out evaluation; evidence for real-world renames comes from separate trials. Each scenario renames a symbol as a dry run (`atlas_edit` rename through `IdeService`, one-shot servers) on a fresh copy of the fixture and scores the planned edits against `[[rename_sites]]` truth read from the fixture files. Edit recall is expected edit sites planned, precision is planned edits that are expected, report recall is expected not-renamed sites listed with the expected reason, and violations are `untouched` lines the plan edits. A planned edit whose original text is not the old name never counts as a hit. A scenario without a language server is skipped, never scored as zero; a scenario that fails counts its expected sites as missed.

Servers used:

- typescript-language-server 5.1.3, TypeScript 5.9.3
- rust-analyzer 1.97.1 (8bab26f4 2026-07-14)

| scenario | fixture | state | edits found/expected | edit recall | edit precision | reports found/expected | report recall | violations/checked |
|---|---|---|---|---|---|---|---|---|
| format-money | acme | scored | 6/6 | 100.0% | 100.0% | 0/0 | n/a | 0/0 |
| users-service-find-one | acme | scored | 5/5 | 100.0% | 100.0% | 0/0 | n/a | 0/4 |
| orders-service-find-one | acme | scored | 2/2 | 100.0% | 100.0% | 3/3 | 100.0% | 0/4 |
| to-cents | acme | scored | 7/7 | 100.0% | 100.0% | 2/2 | 100.0% | 0/1 |
| to-cents-apply | acme | scored | 8/8 | 100.0% | 100.0% | 0/0 | n/a | 0/1 |
| store-get | adversarial/trait-dispatch-rust | scored | 7/7 | 100.0% | 100.0% | 0/0 | n/a | 0/1 |
| payment-provider-charge | adversarial/ts-overrides | scored | 6/6 | 100.0% | 100.0% | 1/1 | 100.0% | 0/2 |
| **all scored** |  | 7 scored, 0 failed, 0 skipped | 41/41 | 100.0% | 100.0% | 6/6 | 100.0% | 0/13 |

### to-cents (toCents -> toMinorUnits, cross_repo report)

- caveat: the language server kept the old name toCents at acme-sdk:src/index.ts:3:10 ("toMinorUnits as toCents"); code that reaches it through those places keeps working under the old name

