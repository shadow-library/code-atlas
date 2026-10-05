use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{QualityError, QualityResult};
use super::evaluate::{RenameAggregate, RenameOutcome, RenameReport, RenameScenario, RenameScore};
use super::report::{pct, table};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameDocument {
  pub date: String,
  pub rename: RenameReport,
}

fn score_cells(score: &RenameScore) -> Vec<String> {
  vec![
    format!("{}/{}", score.edit.correct, score.edit.expected),
    pct(score.edit.recall()),
    pct(score.edit.precision()),
    format!("{}/{}", score.report.found, score.report.expected),
    pct(score.report.recall()),
    format!("{}/{}", score.untouched.violations, score.untouched.checked),
  ]
}

fn aggregate_row(aggregate: &RenameAggregate) -> Vec<String> {
  let mut row = vec![
    "**all scored**".to_owned(),
    String::new(),
    format!("{} scored, {} failed, {} skipped", aggregate.scored, aggregate.failed, aggregate.skipped),
  ];
  row.extend([
    format!("{}/{}", aggregate.edit.correct, aggregate.edit.expected),
    pct(aggregate.edit.recall()),
    pct(aggregate.edit.precision()),
    format!("{}/{}", aggregate.report.found, aggregate.report.expected),
    pct(aggregate.report.recall()),
    format!("{}/{}", aggregate.untouched.violations, aggregate.untouched.checked),
  ]);
  row
}

fn scenario_row(scenario: &RenameScenario) -> Vec<String> {
  let head = vec![scenario.id.clone(), scenario.fixture.clone()];
  match &scenario.outcome {
    RenameOutcome::Scored { score, .. } => [head, vec!["scored".to_owned()], score_cells(score)].concat(),
    RenameOutcome::Failed { score, .. } => [head, vec!["FAILED".to_owned()], score_cells(score)].concat(),
    RenameOutcome::Skipped { .. } => [head, vec!["skipped: no language server".to_owned()], vec!["n/a".to_owned(); 6]].concat(),
  }
}

fn detail(scenario: &RenameScenario, out: &mut String) {
  let (score, caveats, reason) = match &scenario.outcome {
    RenameOutcome::Scored { score, caveats } => (score, caveats.as_slice(), None),
    RenameOutcome::Failed { score, reason } => (score, &[][..], Some(format!("failed: {reason}"))),
    RenameOutcome::Skipped { reason } => {
      out.push_str(&format!("### {} ({} -> {})\n\n- skipped: {reason}\n\n", scenario.id, scenario.target, scenario.new_name));
      return;
    }
  };
  let lists = [
    ("missed edits", &score.missed_edits),
    ("unexpected edits", &score.unexpected_edits),
    ("report problems", &score.report_problems),
    ("untouched sites edited", &score.violations),
    ("reported but not in the truth", &score.extra_reports),
  ];
  let mut body = reason.map(|reason| format!("- {reason}\n")).unwrap_or_default();
  for (label, items) in lists {
    for item in items {
      body.push_str(&format!("- {label}: {item}\n"));
    }
  }
  for caveat in caveats {
    body.push_str(&format!("- caveat: {caveat}\n"));
  }
  if body.is_empty() {
    return;
  }
  out.push_str(&format!(
    "### {} ({} -> {}, cross_repo {})\n\n{body}\n",
    scenario.id, scenario.target, scenario.new_name, scenario.cross_repo
  ));
}

pub fn render_rename_section(report: &RenameReport) -> String {
  let mut out = String::from("## Rename (dry runs through the language servers)\n\n");
  let RenameReport::Run { servers, scenarios, aggregate } = report else {
    if let RenameReport::NotRequested { reason } = report {
      out.push_str(&format!("Not run: {reason}\n\n"));
    }
    return out;
  };
  out.push_str("These fixture scenarios (acme, trait-dispatch-rust, ts-overrides) were used while the rename operation (B7) was developed, so the numbers are a regression check, not a held-out evaluation; evidence for real-world renames comes from separate trials. Each scenario renames a symbol as a dry run (`atlas_edit` rename through `IdeService`, one-shot servers) on a fresh copy of the fixture and scores the planned edits against `[[rename_sites]]` truth read from the fixture files. Edit recall is expected edit sites planned, precision is planned edits that are expected, report recall is expected not-renamed sites listed with the expected reason, and violations are `untouched` lines the plan edits. A planned edit whose original text is not the old name never counts as a hit. A scenario without a language server is skipped, never scored as zero; a scenario that fails counts its expected sites as missed.\n\n");
  out.push_str("Servers used:\n\n");
  match servers.is_empty() {
    true => out.push_str("- none (no scenario reached a language server)\n"),
    false => {
      for server in servers {
        let typescript = server.typescript.as_deref().map(|version| format!(", TypeScript {version}")).unwrap_or_default();
        out.push_str(&format!("- {} {}{typescript}\n", server.server, server.version));
      }
    }
  }
  out.push('\n');
  let mut rows: Vec<Vec<String>> = scenarios.iter().map(scenario_row).collect();
  rows.push(aggregate_row(aggregate));
  out.push_str(&table(
    &[
      "scenario",
      "fixture",
      "state",
      "edits found/expected",
      "edit recall",
      "edit precision",
      "reports found/expected",
      "report recall",
      "violations/checked",
    ],
    &rows,
  ));
  for scenario in scenarios {
    detail(scenario, &mut out);
  }
  out
}

pub fn render_rename_markdown(document: &RenameDocument) -> String {
  format!("# Code Atlas rename evaluation\n\nDate: {}.\n\n{}", document.date, render_rename_section(&document.rename))
}

pub fn write_rename_report(document: &RenameDocument, out: &Path) -> QualityResult<(PathBuf, PathBuf)> {
  let base = match out.extension().and_then(|extension| extension.to_str()) {
    Some("md" | "json") => out.with_extension(""),
    _ => out.to_path_buf(),
  };
  if let Some(parent) = base.parent() {
    fs::create_dir_all(parent).map_err(|error| QualityError::io(parent, error))?;
  }
  let markdown = PathBuf::from(format!("{}.md", base.display()));
  let json = PathBuf::from(format!("{}.json", base.display()));
  fs::write(&markdown, render_rename_markdown(document)).map_err(|error| QualityError::io(&markdown, error))?;
  let encoded = serde_json::to_string_pretty(document).map_err(|error| QualityError::parse(&json, error))?;
  fs::write(&json, format!("{encoded}\n")).map_err(|error| QualityError::io(&json, error))?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::quality::evaluate::{RenameServer, aggregate_scenarios, score_rename};
  use crate::quality::truth::parse_truth;

  const TRUTH: &str = r#"
[project]
name = "demo"
repos = ["a"]

[[rename_sites]]
id = "demo"
target = { repo = "a", path = "src/x.ts", symbol = "X.run" }
new_name = "go"
edit = [{ repo = "a", path = "src/x.ts", line = 3 }, { repo = "a", path = "src/y.ts", line = 4 }]
untouched = [{ repo = "a", path = "src/x.ts", line = 5 }]
"#;

  fn scenario(id: &str, outcome: RenameOutcome) -> RenameScenario {
    RenameScenario {
      id: id.to_owned(),
      fixture: "demo".to_owned(),
      target: "X.run".to_owned(),
      new_name: "go".to_owned(),
      cross_repo: "report".to_owned(),
      outcome,
    }
  }

  #[test]
  fn should_render_servers_rows_details_and_a_skipped_row_without_numbers() {
    let truth = parse_truth(TRUTH, "demo.toml").unwrap().rename_sites.remove(0);
    let observed = crate::quality::evaluate::RenameObserved {
      edits: vec![
        crate::quality::evaluate::ObservedEdit {
          repo: "a".to_owned(),
          path: "src/x.ts".to_owned(),
          line: 3,
          column: 1,
          on_name: true,
        },
        crate::quality::evaluate::ObservedEdit {
          repo: "a".to_owned(),
          path: "src/x.ts".to_owned(),
          line: 5,
          column: 1,
          on_name: true,
        },
      ],
      reports: Vec::new(),
    };
    let scenarios = vec![
      scenario(
        "scored",
        RenameOutcome::Scored {
          score: Box::new(score_rename(&truth, &observed)),
          caveats: vec!["kept an alias".to_owned()],
        },
      ),
      scenario(
        "skipped",
        RenameOutcome::Skipped {
          reason: "no language server: rust-analyzer is unavailable".to_owned(),
        },
      ),
    ];
    let report = RenameReport::Run {
      servers: vec![RenameServer {
        server: "typescript-language-server".to_owned(),
        version: "5.1.3".to_owned(),
        typescript: Some("5.9.3".to_owned()),
      }],
      aggregate: aggregate_scenarios(&scenarios),
      scenarios,
    };

    let markdown = render_rename_markdown(&RenameDocument {
      date: "2026-10-06".to_owned(),
      rename: report,
    });

    assert!(markdown.contains("Date: 2026-10-06."), "{markdown}");
    assert!(markdown.contains("- typescript-language-server 5.1.3, TypeScript 5.9.3\n"), "{markdown}");
    assert!(markdown.contains("| scored | demo | scored | 1/2 | 50.0% | 50.0% | 0/0 | n/a | 1/1 |\n"), "{markdown}");
    assert!(
      markdown.contains("| skipped | demo | skipped: no language server | n/a | n/a | n/a | n/a | n/a | n/a |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("| **all scored** |  | 1 scored, 0 failed, 1 skipped | 1/2 | 50.0% | 50.0% | 0/0 | n/a | 1/1 |\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("- missed edits: a:src/y.ts:4\n- unexpected edits: a:src/x.ts:5:1\n- untouched sites edited: a:src/x.ts:5\n- caveat: kept an alias\n"),
      "{markdown}"
    );
    assert!(
      markdown.contains("### skipped (X.run -> go)\n\n- skipped: no language server: rust-analyzer is unavailable\n"),
      "{markdown}"
    );
  }

  #[test]
  fn should_say_why_nothing_ran_when_rename_was_not_requested() {
    let section = render_rename_section(&RenameReport::NotRequested { reason: "--no-rename".to_owned() });
    assert!(section.ends_with("Not run: --no-rename\n\n"), "{section}");
  }

  #[test]
  fn should_round_trip_the_document_through_json() {
    let document = RenameDocument {
      date: "2026-10-06".to_owned(),
      rename: RenameReport::Run {
        servers: Vec::new(),
        scenarios: vec![scenario("skipped", RenameOutcome::Skipped { reason: "none".to_owned() })],
        aggregate: RenameAggregate::default(),
      },
    };
    let encoded = serde_json::to_string(&document).unwrap();
    assert!(encoded.contains(r#""state":"skipped""#), "{encoded}");
    assert_eq!(serde_json::from_str::<RenameDocument>(&encoded).unwrap(), document);
  }
}
