use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::context::{ContextReport, MeanOverlap};
use super::error::{QualityError, QualityResult};
use super::evaluate::{AuthScore, BlastScore, EdgeKindScore, EntityScore, FixtureReport, IsolationScore, SetScore, SimilarScore, UnresolvedScore, UnusedScore};
use super::metrics::{Better, Calibration, Comparison, Metric, Prohibition, Recall, bucket_label, compare_metrics};
use super::oracle::{IndexerState, OracleReport, Tally};
use super::refactor::RefactorReport;
use super::selector::BindingError;

pub const SCHEMA_VERSION: u32 = 1;
const FINDINGS_SHOWN: usize = 40;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
  pub date: String,
  pub rust_analyzer: IndexerState,
  pub scip_typescript: IndexerState,
  pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
  pub entities: EntityScore,
  pub edge_kinds: BTreeMap<String, EdgeKindScore>,
  pub edges: Recall,
  pub forbidden: Prohibition,
  pub unresolved: UnresolvedScore,
  pub unmatched_calls: Recall,
  pub auth: AuthScore,
  pub endpoint_auth: Recall,
  pub tests_of: Recall,
  pub unused: UnusedScore,
  pub similar_claims_checked: u64,
  pub similar_claims_violated: u64,
  pub blast_radius: BlastScore,
  pub isolation: IsolationScore,
  pub file_flags: Recall,
  pub binding_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityReport {
  pub schema_version: u32,
  pub environment: Environment,
  pub fixtures: Vec<FixtureReport>,
  pub totals: Totals,
  pub calibration: Calibration,
  pub oracle: OracleReport,
  pub refactors: RefactorReport,
  pub context: ContextReport,
  pub metrics: BTreeMap<String, Metric>,
}

fn add_prohibition(into: &mut Prohibition, other: &Prohibition) {
  into.absorb(other);
}

fn absorb_unused(into: &mut UnusedScore, other: &UnusedScore) {
  into.expected_unused.absorb(&other.expected_unused);
  into.reported_likely += other.reported_likely;
  into.reported_possibly += other.reported_possibly;
  into.not_unused.absorb(&other.not_unused);
  into.not_unused_hedged += other.not_unused_hedged;
  into.ambiguous_reported += other.ambiguous_reported;
  into.ambiguous_total += other.ambiguous_total;
}

pub fn totals_of(fixtures: &[FixtureReport]) -> Totals {
  let mut totals = Totals::default();
  for fixture in fixtures {
    for (kind, score) in &fixture.entities.by_kind {
      totals.entities.by_kind.entry(kind.clone()).or_default().absorb(score);
    }
    totals.entities.total.absorb(&fixture.entities.total);
    totals.entities.handlers.absorb(&fixture.entities.handlers);
    totals.entities.best_effort_expected += fixture.entities.best_effort_expected;
    totals.entities.best_effort_found += fixture.entities.best_effort_found;
    for (kind, score) in &fixture.edge_kinds {
      let entry = totals.edge_kinds.entry(kind.clone()).or_default();
      entry.recall.absorb(&score.recall);
      entry.optional_expected += score.optional_expected;
      entry.optional_present += score.optional_present;
      entry.optional_wrong_certainty += score.optional_wrong_certainty;
      entry.forbidden.absorb(&score.forbidden);
      if let Some(unexpected) = score.exhaustive_unexpected {
        *entry.exhaustive_unexpected.get_or_insert(0) += unexpected;
      }
    }
    totals.edges.absorb(&fixture.edges);
    add_prohibition(&mut totals.forbidden, &fixture.forbidden);
    totals.unresolved.presence.absorb(&fixture.unresolved.presence);
    totals.unresolved.approximate_reason += fixture.unresolved.approximate_reason;
    totals.unresolved.fabrication.absorb(&fixture.unresolved.fabrication);
    totals.unresolved.not_checkable += fixture.unresolved.not_checkable;
    totals.unmatched_calls.absorb(&fixture.unmatched_calls);
    totals.auth.supported.absorb(&fixture.auth.supported);
    totals.auth.used.absorb(&fixture.auth.used);
    totals.auth.unused.absorb(&fixture.auth.unused);
    totals.endpoint_auth.absorb(&fixture.endpoint_auth);
    totals.tests_of.absorb(&fixture.tests_of);
    absorb_unused(&mut totals.unused, &fixture.unused);
    totals.similar_claims_checked += fixture.similar.claims_checked;
    totals.similar_claims_violated += fixture.similar.claims_violated;
    totals.blast_radius.must_include.absorb(&fixture.blast_radius.must_include);
    totals.blast_radius.must_not_include.absorb(&fixture.blast_radius.must_not_include);
    totals.blast_radius.may_include_expected += fixture.blast_radius.may_include_expected;
    totals.blast_radius.may_include_present += fixture.blast_radius.may_include_present;
    totals.isolation.checked.absorb(&fixture.isolation.checked);
    totals.isolation.not_evaluated += fixture.isolation.not_evaluated;
    totals.file_flags.absorb(&fixture.file_flags);
    totals.binding_errors += fixture.binding_errors.len();
  }
  totals
}

pub fn calibration_of(fixtures: &[FixtureReport]) -> Calibration {
  let mut calibration = Calibration::new();
  for fixture in fixtures {
    calibration.absorb(&fixture.calibration);
  }
  calibration
}

pub fn binding_errors_of(fixtures: &[FixtureReport]) -> Vec<&BindingError> {
  fixtures.iter().flat_map(|fixture| fixture.binding_errors.iter()).collect()
}

fn put(metrics: &mut BTreeMap<String, Metric>, name: String, metric: Option<Metric>) {
  if let Some(metric) = metric {
    metrics.insert(name, metric);
  }
}

fn higher(numerator: u64, denominator: u64) -> Option<Metric> {
  Metric::ratio(numerator, denominator, Better::Higher)
}

fn set_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, score: &SetScore) {
  put(metrics, format!("{prefix}/precision"), higher(score.correct, score.predicted));
  put(metrics, format!("{prefix}/recall"), higher(score.correct, score.expected));
}

fn recall_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, recall: &Recall) {
  put(metrics, format!("{prefix}/recall"), higher(recall.found, recall.bound()));
  put(metrics, format!("{prefix}/recall_all"), higher(recall.found, recall.expected));
}

fn prohibition_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, prohibition: &Prohibition) {
  put(
    metrics,
    format!("{prefix}/satisfaction"),
    higher(prohibition.checked - prohibition.violations, prohibition.checked),
  );
  if prohibition.checked > 0 {
    metrics.insert(format!("{prefix}/violations"), Metric::count(prohibition.violations, Better::Lower));
  }
}

fn scope_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, scope: &Totals) {
  recall_metrics(metrics, &format!("{prefix}/entities"), &scope.entities.total);
  recall_metrics(metrics, &format!("{prefix}/edges"), &scope.edges);
  for (kind, score) in &scope.edge_kinds {
    recall_metrics(metrics, &format!("{prefix}/edge/{kind}"), &score.recall);
    prohibition_metrics(metrics, &format!("{prefix}/edge/{kind}/forbidden"), &score.forbidden);
  }
  prohibition_metrics(metrics, &format!("{prefix}/forbidden"), &scope.forbidden);
  recall_metrics(metrics, &format!("{prefix}/unresolved"), &scope.unresolved.presence);
  prohibition_metrics(metrics, &format!("{prefix}/unresolved/fabrication"), &scope.unresolved.fabrication);
  recall_metrics(metrics, &format!("{prefix}/unmatched_calls"), &scope.unmatched_calls);
  recall_metrics(metrics, &format!("{prefix}/endpoint_auth"), &scope.endpoint_auth);
  recall_metrics(metrics, &format!("{prefix}/tests_of"), &scope.tests_of);
  recall_metrics(metrics, &format!("{prefix}/file_flags"), &scope.file_flags);
  set_metrics(metrics, &format!("{prefix}/auth/supported"), &scope.auth.supported);
  set_metrics(metrics, &format!("{prefix}/auth/used"), &scope.auth.used);
  set_metrics(metrics, &format!("{prefix}/auth/unused"), &scope.auth.unused);
  recall_metrics(metrics, &format!("{prefix}/unused/expected_unused"), &scope.unused.expected_unused);
  put(
    metrics,
    format!("{prefix}/unused/precision"),
    scope
      .unused
      .precision()
      .and_then(|_| higher(scope.unused.reported_likely, scope.unused.reported_likely + scope.unused.not_unused.violations)),
  );
  prohibition_metrics(metrics, &format!("{prefix}/unused/not_unused"), &scope.unused.not_unused);
  recall_metrics(metrics, &format!("{prefix}/blast/must_include"), &scope.blast_radius.must_include);
  prohibition_metrics(metrics, &format!("{prefix}/blast/must_not_include"), &scope.blast_radius.must_not_include);
  prohibition_metrics(metrics, &format!("{prefix}/isolation"), &scope.isolation.checked);
  if scope.similar_claims_checked > 0 {
    metrics.insert(format!("{prefix}/similar/claim_violations"), Metric::count(scope.similar_claims_violated, Better::Lower));
  }
  metrics.insert(format!("{prefix}/binding_errors"), Metric::count(scope.binding_errors as u64, Better::Lower));
}

fn fixture_totals(fixture: &FixtureReport) -> Totals {
  totals_of(std::slice::from_ref(fixture))
}

fn tally_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, tally: &Tally, all: &Tally) {
  put(metrics, format!("{prefix}/precision_confirmable"), higher(tally.agreed, tally.confirmable()));
  put(metrics, format!("{prefix}/precision_all_in"), higher(tally.agreed, tally.total_edges()));
  put(metrics, format!("{prefix}/recall"), higher(tally.agreed, all.agreed + all.scip_only + all.scip_conflicting));
  if tally.agreed + tally.conflicting + tally.ours_only + tally.scip_only > 0 {
    metrics.insert(format!("{prefix}/conflicting"), Metric::count(tally.conflicting, Better::Lower));
  }
}

fn oracle_metrics(metrics: &mut BTreeMap<String, Metric>, oracle: &OracleReport) {
  if oracle.targets.is_empty() {
    return;
  }
  for (kind, split) in &oracle.per_kind {
    tally_metrics(metrics, &format!("oracle/{kind}/all"), &split.all, &split.all);
    tally_metrics(metrics, &format!("oracle/{kind}/resolved"), &split.resolved, &split.all);
    tally_metrics(metrics, &format!("oracle/{kind}/heuristic"), &split.heuristic, &split.all);
  }
  tally_metrics(metrics, "oracle/site_kinds/all", &oracle.site_kinds.all, &oracle.site_kinds.all);
  tally_metrics(metrics, "oracle/site_kinds/resolved", &oracle.site_kinds.resolved, &oracle.site_kinds.all);
  tally_metrics(metrics, "oracle/site_kinds/heuristic", &oracle.site_kinds.heuristic, &oracle.site_kinds.all);
}

fn mean_metrics(metrics: &mut BTreeMap<String, Metric>, prefix: &str, mean: &MeanOverlap) {
  if let Some(value) = mean.precision {
    metrics.insert(
      format!("{prefix}/precision"),
      Metric {
        value,
        numerator: 0,
        denominator: mean.tasks as u64,
        better: Better::Higher,
      },
    );
  }
  if let Some(value) = mean.recall {
    metrics.insert(
      format!("{prefix}/recall"),
      Metric {
        value,
        numerator: 0,
        denominator: mean.tasks as u64,
        better: Better::Higher,
      },
    );
  }
}

pub fn metrics_of(report: &QualityReport) -> BTreeMap<String, Metric> {
  let mut metrics = BTreeMap::new();
  scope_metrics(&mut metrics, "total", &report.totals);
  for fixture in &report.fixtures {
    scope_metrics(&mut metrics, &format!("fixture/{}", fixture.name), &fixture_totals(fixture));
  }
  for (class, cell) in &report.calibration.by_class {
    put(&mut metrics, format!("calibration/{class}/correct"), higher(cell.correct, cell.judged));
    put(
      &mut metrics,
      format!("calibration/{class}/true_relation"),
      higher(cell.judged - cell.false_edges, cell.judged),
    );
  }
  oracle_metrics(&mut metrics, &report.oracle);
  for scenario in &report.refactors.scenarios {
    metrics.insert(format!("refactor/{}", scenario.name), Metric::count(u64::from(scenario.passed), Better::Higher));
  }
  if let ContextReport::Scored { aggregate, .. } = &report.context {
    mean_metrics(&mut metrics, "context/must_read/files", &aggregate.must_read_files);
    mean_metrics(&mut metrics, "context/skim/files", &aggregate.skim_files);
    mean_metrics(&mut metrics, "context/combined/files", &aggregate.combined_files);
    mean_metrics(&mut metrics, "context/must_read/symbols", &aggregate.must_read_symbols);
    mean_metrics(&mut metrics, "context/skim/symbols", &aggregate.skim_symbols);
    mean_metrics(&mut metrics, "context/combined/symbols", &aggregate.combined_symbols);
  }
  metrics
}

pub fn assemble(environment: Environment, fixtures: Vec<FixtureReport>, oracle: OracleReport, refactors: RefactorReport, context: ContextReport) -> QualityReport {
  let totals = totals_of(&fixtures);
  let calibration = calibration_of(&fixtures);
  let mut report = QualityReport {
    schema_version: SCHEMA_VERSION,
    environment,
    fixtures,
    totals,
    calibration,
    oracle,
    refactors,
    context,
    metrics: BTreeMap::new(),
  };
  report.metrics = metrics_of(&report);
  report
}

pub fn compare_reports(previous: &QualityReport, current: &QualityReport) -> Comparison {
  compare_metrics(&previous.metrics, &current.metrics)
}

pub fn read_report(path: &Path) -> QualityResult<QualityReport> {
  let text = fs::read_to_string(path).map_err(|error| QualityError::io(path, error))?;
  serde_json::from_str(&text).map_err(|error| QualityError::parse(path, error))
}

pub fn write_report(report: &QualityReport, out: &Path, comparison: Option<&Comparison>) -> QualityResult<(PathBuf, PathBuf)> {
  let base = match out.extension().and_then(|extension| extension.to_str()) {
    Some("md" | "json") => out.with_extension(""),
    _ => out.to_path_buf(),
  };
  if let Some(parent) = base.parent() {
    fs::create_dir_all(parent).map_err(|error| QualityError::io(parent, error))?;
  }
  let markdown = PathBuf::from(format!("{}.md", base.display()));
  let json = PathBuf::from(format!("{}.json", base.display()));
  fs::write(&markdown, render_markdown(report, comparison)).map_err(|error| QualityError::io(&markdown, error))?;
  let encoded = serde_json::to_string_pretty(report).map_err(|error| QualityError::parse(&json, error))?;
  fs::write(&json, format!("{encoded}\n")).map_err(|error| QualityError::io(&json, error))?;
  Ok((markdown, json))
}

fn pct(value: Option<f64>) -> String {
  value.map_or_else(|| "n/a".to_owned(), |value| format!("{:.1}%", value * 100.0))
}

fn frac(numerator: u64, denominator: u64) -> String {
  format!("{numerator}/{denominator}")
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
  let mut out = format!("| {} |\n|{}|\n", headers.join(" | "), headers.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
  for row in rows {
    out.push_str(&format!(
      "| {} |\n",
      row.iter().map(|cell| cell.replace('|', "\\|").replace('\n', " ")).collect::<Vec<_>>().join(" | ")
    ));
  }
  out.push('\n');
  out
}

fn recall_row(label: &str, recall: &Recall) -> Vec<String> {
  vec![
    label.to_owned(),
    recall.expected.to_string(),
    recall.found.to_string(),
    recall.wrong_certainty.to_string(),
    recall.wrong_detail.to_string(),
    recall.missing.to_string(),
    recall.unbound.to_string(),
    pct(recall.recall_over_bound()),
    pct(recall.recall_over_all()),
  ]
}

const RECALL_HEADERS: [&str; 9] = [
  "",
  "expected",
  "found",
  "wrong certainty",
  "wrong detail",
  "missing",
  "unbound",
  "recall (bound)",
  "recall (all)",
];

fn headers_with(first: &str) -> Vec<&str> {
  let mut headers = RECALL_HEADERS.to_vec();
  headers[0] = first;
  headers
}

fn prohibition_cell(prohibition: &Prohibition) -> String {
  format!(
    "{}/{} (+{} vacuous, {} unbound)",
    prohibition.violations, prohibition.checked, prohibition.vacuous, prohibition.unbound
  )
}

pub fn render_markdown(report: &QualityReport, comparison: Option<&Comparison>) -> String {
  let mut out = String::new();
  out.push_str("# Code Atlas knowledge-quality report\n\n");
  out.push_str(&format!("Date: {}. Schema version {}.\n\n", report.environment.date, report.schema_version));
  out.push_str(&environment_section(report));
  out.push_str(&limits_section());
  out.push_str(&reading_guide(report));
  out.push_str(&headline_section(report));
  out.push_str(&edge_kind_section(report));
  out.push_str(&entity_section(report));
  out.push_str(&domain_section(report));
  out.push_str(&calibration_section(report));
  out.push_str(&binding_section(report));
  out.push_str(&findings_section(report));
  out.push_str(&oracle_section(&report.oracle));
  out.push_str(&refactor_section(&report.refactors));
  out.push_str(&context_section(&report.context));
  if let Some(comparison) = comparison {
    out.push_str("## Comparison with the previous report\n\n");
    out.push_str(&render_comparison(comparison));
  }
  out
}

fn limits_section() -> String {
  String::from(
    "## Limits\n\n\
     - **Fixture sample bias.** The fixtures are a few small, hand-written projects plus one real workspace (this repository). Recall and precision on them say little about how the system behaves on arbitrary code, and the ground truth was written alongside the system.\n\
     - **Oracle scope.** SCIP is compared only for `calls`, `references`, `instantiates`, `implements` and `extends`, only in files an indexer covered, and only for Rust and TypeScript. References outside any symbol and cross-package targets in per-package indexes cannot be confirmed. SCIP itself disagrees with us on granularity (constructor versus class, receiver versus method).\n\
     - **Heuristic calibration rests on a small sample.** The fixture-judged heuristic edges number a few dozen; the oracle buckets are dominated by one confidence tier.\n\
     - **No exhaustive kinds.** No fixture declares an exhaustive edge set, so unlisted-edge precision is not measured.\n\
     - **No LLM verdicts.** Inferred similarity verdicts do not exist in these runs, so verdict accuracy is not measured.\n\n",
  )
}

fn environment_section(report: &QualityReport) -> String {
  let describe = |state: &IndexerState| match state {
    IndexerState::Available { detail } => format!("available ({detail})"),
    IndexerState::Unavailable { reason } => format!("unavailable: {reason}"),
    IndexerState::Disabled { reason } => format!("not run: {reason}"),
  };
  let mut out = String::from("## Environment\n\n");
  out.push_str(&format!("- rust-analyzer SCIP: {}\n", describe(&report.environment.rust_analyzer)));
  out.push_str(&format!("- scip-typescript: {}\n", describe(&report.environment.scip_typescript)));
  for note in &report.environment.notes {
    out.push_str(&format!("- {note}\n"));
  }
  out.push('\n');
  out
}

fn reading_guide(report: &QualityReport) -> String {
  let declared: Vec<String> = report
    .fixtures
    .iter()
    .filter(|fixture| !fixture.exhaustive_kinds.is_empty())
    .map(|fixture| format!("{}: {}", fixture.name, fixture.exhaustive_kinds.join(", ")))
    .collect();
  let exhaustive = match declared.is_empty() {
    true => "No fixture declares an exhaustive edge set (`exhaustive_edge_kinds` is absent from every `expected.toml`; `unused_exhaustive` is false everywhere), so no unlisted-edge precision is claimed. Precision is reported as forbidden-edge satisfaction, fabrication checks on unresolved references, `not_unused` violations, `must_not_include` violations, isolation leaks, and as the oracle precision against SCIP.".to_owned(),
    false => format!("Exhaustive edge kinds declared by the ground truth: {}. For those kinds every edge between selected entities that is not listed counts as unexpected.", declared.join("; ")),
  };
  format!(
    "## How to read this report\n\n\
     - Each expectation ends in one of: **found** (present with an accepted certainty class and confidence), **wrong certainty** (present, but its class or confidence is outside the expectation), **wrong detail** (present, but a secondary property such as kind, occurrences, reason or handler differs), **missing**, or **unbound** (a selector could not be bound to exactly one entity: a ground-truth binding error, kept out of recall).\n\
     - *Recall (bound)* is found / (found + wrong + missing) and ignores unbound expectations. *Recall (all)* is found / expected and counts unbound expectations as failures, the pessimistic bound. A selector that matches nothing is only a binding error when no `[[entity]]` expectation explains the absence; if the entity expectation itself is missing, dependent edges count as missing.\n\
     - Selectors naming repos of another project are *foreign*: they cannot exist in this project's index, so prohibitions on them hold vacuously and are counted separately.\n\
     - {exhaustive}\n\
     - Calibration judges only edges that appear in an expectation (expected edges, tolerated candidates, forbidden violations), so it is biased towards the hard cases the fixtures were written to probe.\n\
     - Selector conventions the evaluator applies (none edits a fixture): Rust symbols are the item path relative to the file's own module, so inline modules appear (`tests::name`) and trait impl methods render as `<Type as Trait>::method`; a TypeScript `default` selects the default-exported symbol of the file; `suite` matches the label of a `describe` block even when it is nested; `db_model` binds to the `db_table` entity named by `name` and requires a `maps_to_table` edge from `sym`; an unresolved `generated_code` reference is accepted when the resolver records `macro_generated` (counted as an approximate reason), and a dynamic route or URL is accepted when the detector emitted a `route-dynamic` or `http.dynamic_url` diagnostic inside the selected symbol.\n\
     - Nothing in this report has been tuned: every number measures the system as committed.\n\n"
  )
}

fn headline_section(report: &QualityReport) -> String {
  let mut rows = Vec::new();
  for fixture in &report.fixtures {
    rows.push(vec![
      fixture.name.clone(),
      frac(fixture.entities.total.found, fixture.entities.total.expected),
      frac(fixture.edges.found, fixture.edges.expected),
      pct(fixture.edges.recall_over_bound()),
      (fixture.edges.wrong_certainty + fixture.edges.wrong_detail).to_string(),
      fixture.edges.missing.to_string(),
      fixture.edges.unbound.to_string(),
      prohibition_cell(&fixture.forbidden),
      frac(fixture.unresolved.presence.found, fixture.unresolved.presence.expected),
      fixture.unresolved.fabrication.violations.to_string(),
      fixture.binding_errors.len().to_string(),
    ]);
  }
  let totals = &report.totals;
  rows.push(vec![
    "**all fixtures**".to_owned(),
    frac(totals.entities.total.found, totals.entities.total.expected),
    frac(totals.edges.found, totals.edges.expected),
    pct(totals.edges.recall_over_bound()),
    (totals.edges.wrong_certainty + totals.edges.wrong_detail).to_string(),
    totals.edges.missing.to_string(),
    totals.edges.unbound.to_string(),
    prohibition_cell(&totals.forbidden),
    frac(totals.unresolved.presence.found, totals.unresolved.presence.expected),
    totals.unresolved.fabrication.violations.to_string(),
    totals.binding_errors.to_string(),
  ]);
  let mut out = String::from("## Headline per fixture\n\n");
  out.push_str(&table(
    &[
      "fixture",
      "entities found",
      "edges found",
      "edge recall (bound)",
      "edges wrong cert/detail",
      "edges missing",
      "edges unbound",
      "forbidden violations/checked",
      "unresolved found",
      "fabricated",
      "binding errors",
    ],
    &rows,
  ));
  out
}

fn edge_kind_section(report: &QualityReport) -> String {
  let mut rows = Vec::new();
  for (kind, score) in &report.totals.edge_kinds {
    let mut row = recall_row(kind, &score.recall);
    row.push(format!("{}/{}", score.optional_present, score.optional_expected));
    row.push(prohibition_cell(&score.forbidden));
    row.push(score.exhaustive_unexpected.map_or_else(|| "not declared".to_owned(), |count| count.to_string()));
    rows.push(row);
  }
  let mut headers = headers_with("edge kind");
  headers.extend(["optional present/expected", "forbidden violations/checked", "exhaustive unexpected"]);
  let mut out = String::from("## Edge kinds (all fixtures)\n\n");
  out.push_str(&table(&headers, &rows));
  out.push_str("### Edge kinds per fixture\n\n");
  let mut per_fixture = Vec::new();
  for fixture in &report.fixtures {
    for (kind, score) in &fixture.edge_kinds {
      let recall = &score.recall;
      per_fixture.push(vec![
        fixture.name.clone(),
        kind.clone(),
        frac(recall.found, recall.expected),
        (recall.wrong_certainty + recall.wrong_detail).to_string(),
        recall.missing.to_string(),
        recall.unbound.to_string(),
        pct(recall.recall_over_bound()),
        prohibition_cell(&score.forbidden),
      ]);
    }
  }
  out.push_str(&table(
    &[
      "fixture",
      "edge kind",
      "found/expected",
      "wrong cert/detail",
      "missing",
      "unbound",
      "recall (bound)",
      "forbidden violations/checked",
    ],
    &per_fixture,
  ));
  out
}

fn entity_section(report: &QualityReport) -> String {
  let mut rows: Vec<Vec<String>> = report.totals.entities.by_kind.iter().map(|(kind, recall)| recall_row(kind, recall)).collect();
  rows.push(recall_row("**all required**", &report.totals.entities.total));
  rows.push(recall_row("endpoint handlers", &report.totals.entities.handlers));
  let mut out = String::from("## Entities by kind (all fixtures)\n\n");
  out.push_str(&table(&headers_with("entity kind"), &rows));
  out.push_str(&format!(
    "Best-effort entities (not required): {}/{} found.\n\n",
    report.totals.entities.best_effort_found, report.totals.entities.best_effort_expected
  ));
  out
}

fn similar_cell(similar: &SimilarScore) -> String {
  format!(
    "{} claims checked, {} violated; {} inferred verdicts ({} correct), {} undecided, {} not evaluable (cross-project)",
    similar.claims_checked, similar.claims_violated, similar.verdicts_inferred, similar.verdicts_correct, similar.verdicts_undecided, similar.not_evaluable
  )
}

fn domain_section(report: &QualityReport) -> String {
  let mut out = String::from("## Other expectation tables\n\n");
  let mut rows = Vec::new();
  for fixture in &report.fixtures {
    rows.push(vec![
      fixture.name.clone(),
      frac(fixture.unmatched_calls.found, fixture.unmatched_calls.expected),
      frac(fixture.endpoint_auth.found, fixture.endpoint_auth.expected),
      frac(fixture.tests_of.found, fixture.tests_of.expected),
      frac(fixture.file_flags.found, fixture.file_flags.expected),
      frac(fixture.blast_radius.must_include.found, fixture.blast_radius.must_include.expected),
      prohibition_cell(&fixture.blast_radius.must_not_include),
      prohibition_cell(&fixture.isolation.checked),
    ]);
  }
  out.push_str(&table(
    &[
      "fixture",
      "unmatched calls",
      "endpoint auth",
      "tests-for",
      "file flags",
      "blast must_include",
      "blast must_not_include violations",
      "isolation leaks",
    ],
    &rows,
  ));
  out.push_str("### Auth matrix, unused code and similarity\n\n");
  let mut detail = Vec::new();
  for fixture in &report.fixtures {
    let auth = &fixture.auth;
    let unused = &fixture.unused;
    let nothing =
      auth.supported.expected + auth.used.expected + auth.unused.expected + unused.expected_unused.expected + unused.not_unused.checked + fixture.similar.pairs.len() as u64;
    if nothing == 0 {
      continue;
    }
    detail.push(vec![
      fixture.name.clone(),
      format!("{} / {}", pct(auth.supported.precision()), pct(auth.supported.recall())),
      format!("{} / {}", pct(auth.used.precision()), pct(auth.used.recall())),
      format!("{} / {}", pct(auth.unused.precision()), pct(auth.unused.recall())),
      format!(
        "{} reported ({} likely, {} possibly)",
        frac(unused.expected_unused.found, unused.expected_unused.expected),
        unused.reported_likely,
        unused.reported_possibly
      ),
      format!("{}; {} hedged", prohibition_cell(&unused.not_unused), unused.not_unused_hedged),
      pct(unused.precision()),
      similar_cell(&fixture.similar),
    ]);
  }
  out.push_str(&table(
    &[
      "fixture",
      "supported schemes (precision / recall)",
      "used (precision / recall)",
      "unused (precision / recall)",
      "[[unused]] found",
      "[[not_unused]] violations/checked",
      "unused precision over judged",
      "similarity",
    ],
    &detail,
  ));
  out.push_str("Similarity pairs:\n\n");
  let mut pairs = Vec::new();
  for fixture in &report.fixtures {
    for pair in &fixture.similar.pairs {
      pairs.push(vec![
        fixture.name.clone(),
        pair.a.clone(),
        pair.b.clone(),
        pair.expected.clone(),
        pair.scope.clone(),
        pair.surfaced.to_string(),
        pair.structural_jaccard.map_or_else(|| "none".to_owned(), |value| format!("{value:.2}")),
        pair.inferred_verdict.clone().unwrap_or_else(|| "none (no LLM verdict)".to_owned()),
        pair.system_verdict.clone().unwrap_or_default(),
        pair.deterministic_claim.to_string(),
      ]);
    }
  }
  out.push_str(&table(
    &[
      "fixture",
      "a",
      "b",
      "expected",
      "scope",
      "surfaced",
      "structural jaccard",
      "inferred verdict",
      "system verdict",
      "deterministic equivalent_to",
    ],
    &pairs,
  ));
  out
}

fn calibration_section(report: &QualityReport) -> String {
  let mut rows = Vec::new();
  for (class, cell) in &report.calibration.by_class {
    rows.push(vec![
      class.clone(),
      cell.judged.to_string(),
      cell.correct.to_string(),
      cell.overclaimed.to_string(),
      cell.underclaimed.to_string(),
      cell.false_edges.to_string(),
      pct(cell.fraction_correct()),
      pct(cell.fraction_true_relation()),
    ]);
  }
  let mut out = String::from("## Certainty calibration\n\n");
  out.push_str("Per class, over judged edges: *correct* = the edge is expected and its class and confidence are accepted; *overclaimed* = the relation is expected but the class is stronger than allowed; *underclaimed* = weaker than allowed; *false* = a forbidden edge exists. *True relation* = (judged - false) / judged.\n\n");
  out.push_str(&table(
    &["class", "judged", "correct", "overclaimed", "underclaimed", "false", "correct %", "true relation %"],
    &rows,
  ));
  let buckets: Vec<Vec<String>> = report
    .calibration
    .heuristic_buckets
    .iter()
    .enumerate()
    .map(|(index, cell)| {
      vec![
        bucket_label(index),
        cell.judged.to_string(),
        cell.correct.to_string(),
        cell.false_edges.to_string(),
        pct(cell.fraction_true_relation()),
      ]
    })
    .collect();
  out.push_str("Heuristic edges by confidence:\n\n");
  out.push_str(&table(&["confidence", "judged", "correct", "false", "true relation %"], &buckets));
  out.push_str(&match report.calibration.heuristic_monotone() {
    Some(true) => "True-relation rate is non-decreasing with confidence across the populated buckets.\n\n".to_owned(),
    Some(false) => "True-relation rate is NOT monotone in confidence across the populated buckets: confidence does not track correctness on the judged edges.\n\n".to_owned(),
    None => "Fewer than two populated confidence buckets: monotonicity cannot be assessed.\n\n".to_owned(),
  });
  out
}

fn binding_section(report: &QualityReport) -> String {
  let mut out = String::from(
    "## Ground-truth binding errors\n\nA selector that matched zero or several entities and is not explained by a missing `[[entity]]` expectation. These are excluded from recall and precision. They may indicate a wrong fixture or a naming-convention gap in the evaluator; none was edited to fit the system.\n\n",
  );
  let errors = binding_errors_of(&report.fixtures);
  if errors.is_empty() {
    out.push_str("None.\n\n");
    return out;
  }
  let rows: Vec<Vec<String>> = errors
    .iter()
    .map(|error| {
      let problem = match &error.problem {
        super::selector::BindingProblem::NoMatch { hint } => format!("matches nothing: {hint}"),
        super::selector::BindingProblem::Ambiguous { candidates } => format!("ambiguous: {}", candidates.join(", ")),
      };
      vec![error.fixture.clone(), error.section.clone(), error.selector.clone(), problem]
    })
    .collect();
  out.push_str(&table(&["fixture", "section", "selector", "problem"], &rows));
  out
}

fn findings_section(report: &QualityReport) -> String {
  let mut out = String::from("## Findings (every non-passing expectation)\n\n");
  for fixture in &report.fixtures {
    if fixture.findings.is_empty() {
      continue;
    }
    out.push_str(&format!("### {}\n\n", fixture.name));
    for finding in fixture.findings.iter().take(FINDINGS_SHOWN) {
      out.push_str(&format!("- `{}` [{}] {}: {}\n", finding.section, finding.kind, finding.label, finding.outcome));
    }
    if fixture.findings.len() > FINDINGS_SHOWN {
      out.push_str(&format!("- ... {} more in quality.json\n", fixture.findings.len() - FINDINGS_SHOWN));
    }
    out.push('\n');
  }
  out
}

fn tally_row(label: &str, tally: &Tally, all: &Tally) -> Vec<String> {
  vec![
    label.to_owned(),
    format!("{} ({} respanned)", tally.agreed, tally.agreed_respanned),
    tally.ours_only.to_string(),
    tally.ours_only_cross_package.to_string(),
    tally.ours_only_file_level.to_string(),
    tally.scip_only.to_string(),
    tally.conflicting.to_string(),
    pct(tally.precision_confirmable()),
    pct(tally.precision_all_in()),
    pct(tally.recall_within(all)),
  ]
}

fn headline_sentence(label: &str, tally: &Tally) -> String {
  format!(
    "- {label} precision: {} (confirmable edges, {} of {}) / {} all-in; {} agreed, {} conflicting, {} same-package unconfirmed, {} unconfirmable ({} cross-package, {} file-level).\n",
    pct(tally.precision_confirmable()),
    tally.confirmable(),
    tally.total_edges(),
    pct(tally.precision_all_in()),
    tally.agreed,
    tally.conflicting,
    tally.ours_only,
    tally.unconfirmable(),
    tally.ours_only_cross_package,
    tally.ours_only_file_level,
  )
}

fn oracle_section(oracle: &OracleReport) -> String {
  let mut out = String::from("## Resolver versus the compiler oracle (SCIP)\n\n");
  if oracle.targets.is_empty() {
    out.push_str("The oracle did not run (see Environment). No resolver-vs-compiler claims are made.\n\n");
    return out;
  }
  out.push_str("Headline (calls + references + instantiates, all targets):\n\n");
  out.push_str(&headline_sentence("resolved", &oracle.site_kinds.resolved));
  out.push_str(&headline_sentence("heuristic", &oracle.site_kinds.heuristic));
  out.push_str(&format!(
    "- recall of SCIP's edges: {} by resolved edges, {} by all of our edges.\n\n",
    pct(oracle.site_kinds.resolved.recall_within(&oracle.site_kinds.all)),
    pct(oracle.site_kinds.all.recall_within(&oracle.site_kinds.all))
  ));
  out.push_str(
    "Our file-owned `calls`, `references`, `instantiates`, `implements` and `extends` edges in the files SCIP indexed are reconciled with the SCIP edges by `atlas_scip::reconcile`. \
     Cargo workspaces are indexed once at the workspace root so that cross-crate edges can be confirmed; other Cargo and npm packages are indexed per package. \
     Because `reconcile` keys sites by exact span, a second pass pairs leftover ours-only and scip-only edges with the same enclosing symbol and file: the same target counts as *agreed* (shown as *respanned*) when the spans overlap, or when they start on one line and both spans contain the callee name; a different target with overlapping spans counts as *conflicting*. \
     *agreed*: same site and target. *ours-only*: we have an edge SCIP does not (a heuristic candidate beside an agreed target, or a site SCIP does not know). \
     *ours-only cross-package*: our edge targets another package, which a per-package index cannot confirm. *ours-only file-level*: the enclosing entity is a file, and the ingestor drops SCIP references outside any symbol (`references_unmapped`), so the oracle cannot confirm them. These two are *unconfirmable*. \
     *scip-only*: SCIP target we lack. *conflicting*: both sides have targets at one site and none is shared.\n\n",
  );
  out.push_str(
    "Precision (confirmable-only) = agreed / (agreed + conflicting + same-package ours-only): it counts every unconfirmed confirmable edge as wrong but ignores unconfirmable ones. Precision (all-in) = agreed / (all of our edges in covered files, unconfirmable included): it counts every edge the oracle did not confirm as wrong. Recall vs SCIP = agreed / (agreed + scip-only + SCIP targets in conflicts), always over all of SCIP's edges, so the per-class rows show how much of SCIP's graph each of our classes reproduces. `implements` is shown separately: method-level `implements` edges (impl method to trait method) are not modelled by SCIP and always appear ours-only. SCIP points `new X()` at the constructor and `acme.createOrder()` occurrences at several symbols, so some conflicts are granularity differences rather than wrong targets; the sample below is for manual inspection.\n\n",
  );
  let rows: Vec<Vec<String>> = oracle
    .targets
    .iter()
    .map(|target| {
      vec![
        target.name.clone(),
        target
          .packages
          .iter()
          .map(|package| format!("{}:{}/ {} {} ({} files)", package.repo, package.root, package.indexer, package.outcome, package.files))
          .collect::<Vec<_>>()
          .join("; "),
        target.files_covered.to_string(),
        target.skipped.clone().unwrap_or_default(),
      ]
    })
    .collect();
  out.push_str(&table(&["target", "packages", "files covered by SCIP", "note"], &rows));
  let mut rows = Vec::new();
  for (kind, split) in &oracle.per_kind {
    rows.push(tally_row(&format!("{kind} / all"), &split.all, &split.all));
    rows.push(tally_row(&format!("{kind} / resolved"), &split.resolved, &split.all));
    rows.push(tally_row(&format!("{kind} / heuristic"), &split.heuristic, &split.all));
  }
  rows.push(tally_row("**calls+references+instantiates / all**", &oracle.site_kinds.all, &oracle.site_kinds.all));
  rows.push(tally_row(
    "**calls+references+instantiates / resolved**",
    &oracle.site_kinds.resolved,
    &oracle.site_kinds.all,
  ));
  rows.push(tally_row(
    "**calls+references+instantiates / heuristic**",
    &oracle.site_kinds.heuristic,
    &oracle.site_kinds.all,
  ));
  out.push_str(&table(
    &[
      "edge kind / our class",
      "agreed",
      "ours-only",
      "ours-only cross-package",
      "ours-only file-level",
      "scip-only",
      "conflicting",
      "precision (confirmable-only)",
      "precision (all-in)",
      "recall vs SCIP",
    ],
    &rows,
  ));
  out.push_str("SCIP indexes used (blake3 of each `.scip`; the cache key encodes the inputs, so identical inputs reproduce the digest):\n\n");
  let digests: Vec<Vec<String>> = oracle
    .indexes
    .iter()
    .map(|index| {
      vec![
        index.target.clone(),
        index.repo.clone(),
        format!("{}/", index.root),
        index.indexer.clone(),
        index.blake3.clone(),
      ]
    })
    .collect();
  out.push_str(&table(&["target", "repo", "root", "indexer", "blake3"], &digests));
  out.push_str("Heuristic edges by confidence against the oracle (unconfirmed = same-package ours-only):\n\n");
  let buckets: Vec<Vec<String>> = oracle
    .heuristic_buckets
    .iter()
    .map(|bucket| {
      vec![
        bucket.label.clone(),
        bucket.agreed.to_string(),
        bucket.conflicting.to_string(),
        bucket.unconfirmed.to_string(),
        pct(bucket.agreement()),
      ]
    })
    .collect();
  out.push_str(&table(&["confidence", "agreed", "conflicting", "unconfirmed", "agreed / (agreed + conflicting)"], &buckets));
  out.push_str(&format!(
    "Conflicts: {} in total; {} sampled below for manual inspection.\n\n",
    oracle.conflicts_total,
    oracle.conflict_sample.len()
  ));
  let samples: Vec<Vec<String>> = oracle
    .conflict_sample
    .iter()
    .map(|sample| {
      vec![
        sample.target.clone(),
        sample.location.clone(),
        sample.edge_kind.clone(),
        sample.enclosing.clone(),
        sample.ours.join("; "),
        sample.scip.join("; "),
      ]
    })
    .collect();
  out.push_str(&table(
    &["target", "location", "our edge kind", "enclosing symbol", "our target(s)", "SCIP target(s)"],
    &samples,
  ));
  out.push_str(&sample_table(
    "Unconfirmed resolved edges (ours-only, same package)",
    oracle.unconfirmed_total,
    &oracle.unconfirmed_sample,
  ));
  out.push_str(&sample_table("SCIP-only edges (missed by the resolver)", oracle.scip_only_total, &oracle.scip_only_sample));
  out
}

fn sample_table(title: &str, total: usize, samples: &[super::oracle::ConflictSample]) -> String {
  let rows: Vec<Vec<String>> = samples
    .iter()
    .map(|sample| {
      vec![
        sample.target.clone(),
        sample.location.clone(),
        sample.edge_kind.clone(),
        sample.enclosing.clone(),
        sample.ours.join("; "),
        sample.scip.join("; "),
      ]
    })
    .collect();
  format!(
    "{title}: {total} in total, {} sampled evenly for manual inspection.\n\n{}",
    samples.len(),
    table(&["target", "location", "edge kind", "enclosing symbol", "our target(s)", "SCIP target(s)"], &rows)
  )
}

fn refactor_section(refactors: &RefactorReport) -> String {
  let mut out = String::from(
    "## Refactor robustness\n\nEach scenario edits a temporary copy of the acme fixtures, runs the incremental update and checks the expected graph deltas, incremental equals full (fact-set digest, lineage imported), no stale deterministic fact (every edge cites current file content), and that seeded inferred facts on changed evidence are no longer current.\n\n",
  );
  let rows: Vec<Vec<String>> = refactors
    .scenarios
    .iter()
    .map(|scenario| {
      vec![
        scenario.name.clone(),
        if scenario.passed { "PASS".to_owned() } else { "FAIL".to_owned() },
        scenario.description.clone(),
        frac(scenario.checks.iter().filter(|check| check.passed).count() as u64, scenario.checks.len() as u64),
        scenario.error.clone().unwrap_or_default(),
      ]
    })
    .collect();
  out.push_str(&table(&["scenario", "result", "description", "checks passed", "error"], &rows));
  out.push_str(&format!("Scenarios passed: {}/{}.\n\n", refactors.passed(), refactors.scenarios.len()));
  for scenario in &refactors.scenarios {
    out.push_str(&format!("### {}\n\n", scenario.name));
    for check in &scenario.checks {
      out.push_str(&format!("- {} {}: {}\n", if check.passed { "pass" } else { "FAIL" }, check.name, check.detail));
    }
    out.push('\n');
  }
  out
}

fn context_section(context: &ContextReport) -> String {
  let mut out = String::from("## Context selection quality\n\n");
  match context {
    ContextReport::NotRequested { reason } => out.push_str(&format!("Not run: {reason}\n\n")),
    ContextReport::Scored { tasks, skipped, aggregate } => {
      let mean = |label: &str, value: &MeanOverlap| vec![label.to_owned(), value.tasks.to_string(), pct(value.precision), pct(value.recall)];
      out.push_str(&table(
        &["selection", "tasks", "precision", "recall"],
        &[
          mean("MUST READ files", &aggregate.must_read_files),
          mean("SKIM files", &aggregate.skim_files),
          mean("MUST READ + SKIM files", &aggregate.combined_files),
          mean("MUST READ symbols", &aggregate.must_read_symbols),
          mean("SKIM symbols", &aggregate.skim_symbols),
          mean("MUST READ + SKIM symbols", &aggregate.combined_symbols),
        ],
      ));
      let rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|task| {
          vec![
            task.id.clone(),
            format!("{} / {}", pct(task.combined.files.precision), pct(task.combined.files.recall)),
            format!("{} / {}", pct(task.combined.symbols.precision), pct(task.combined.symbols.recall)),
          ]
        })
        .collect();
      out.push_str(&table(&["task", "files (precision / recall, combined)", "symbols (precision / recall, combined)"], &rows));
      for skip in skipped {
        out.push_str(&format!("- skipped `{}`: {}\n", skip.id, skip.reason));
      }
      out.push('\n');
    }
  }
  out
}

pub fn render_comparison(comparison: &Comparison) -> String {
  let mut out = String::new();
  let line = |marker: &str, delta: &super::metrics::Delta| {
    format!(
      "{marker} {}: {:.4} -> {:.4} ({:+.4}, {} is better)\n",
      delta.metric,
      delta.before,
      delta.after,
      delta.change,
      match delta.better {
        Better::Higher => "higher",
        Better::Lower => "lower",
      }
    )
  };
  out.push_str(&format!(
    "**{} regressed, {} improved, {} unchanged, {} added, {} removed**\n\n",
    comparison.regressed.len(),
    comparison.improved.len(),
    comparison.unchanged,
    comparison.added.len(),
    comparison.removed.len()
  ));
  if !comparison.regressed.is_empty() {
    out.push_str("Regressed:\n\n");
    for delta in &comparison.regressed {
      out.push_str(&format!("- {}", line("REGRESSED", delta)));
    }
    out.push('\n');
  }
  if !comparison.improved.is_empty() {
    out.push_str("Improved:\n\n");
    for delta in &comparison.improved {
      out.push_str(&format!("- {}", line("improved", delta)));
    }
    out.push('\n');
  }
  if !comparison.added.is_empty() {
    out.push_str(&format!("Added metrics: {}\n\n", comparison.added.join(", ")));
  }
  if !comparison.removed.is_empty() {
    out.push_str(&format!("Removed metrics: {}\n\n", comparison.removed.join(", ")));
  }
  out
}
