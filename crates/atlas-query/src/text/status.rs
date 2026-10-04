use crate::freshness::ScipState;
use crate::queries::{ChangesAnswer, ReadAnswer, StatusAnswer};
use crate::text::{RenderText, TextOut, entity_line, snippet};

fn counts(map: &std::collections::BTreeMap<String, u64>) -> String {
  map.iter().map(|(name, count)| format!("{name} {count}")).collect::<Vec<_>>().join(", ")
}

impl RenderText for StatusAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("project {} repos {}", self.project, self.repos.join(", ")));
    out.line(0, format!("files {}", counts(&self.files_by_language)));
    out.line(0, format!("parse {}", counts(&self.files_by_parse_status)));
    for file in self.failed_files.iter().chain(&self.repos_missing) {
      out.line(1, format!("failed {}:{} {}", file.repo, file.path.as_deref().unwrap_or("-"), file.reason));
    }
    out.line(0, format!("unresolved {}", counts(&self.unresolved_by_reason)));
    out.line(0, "edges by certainty");
    for (kind, classes) in &self.edge_certainty {
      out.line(1, format!("{kind}: {}", counts(classes)));
    }
    let coverage = format!("{}/{} files covered", self.scip_files, self.scip_total);
    match &self.scip {
      Some(ScipState::NeverRun) => out.line(0, format!("scip: never run ({coverage})")),
      Some(ScipState::Current) => out.line(0, format!("scip: current ({coverage})")),
      Some(ScipState::Behind { files }) => out.line(0, format!("scip: behind on {files} files ({coverage})")),
      None => out.line(0, format!("scip: unknown ({coverage})")),
    }
    if self.skipped_total > 0 {
      let listed = self
        .skipped_files
        .iter()
        .map(|file| format!("{}/{} ({})", file.repo, file.path, file.reason))
        .collect::<Vec<_>>()
        .join(", ");
      let omitted = self.skipped_total.saturating_sub(self.skipped_files.len() as u64);
      let more = if omitted > 0 { format!(", and {omitted} more") } else { String::new() };
      let noun = if self.skipped_total == 1 { "file" } else { "files" };
      out.line(0, format!("skipped {} {noun}: {listed}{more}", self.skipped_total));
    }
    out.line(
      0,
      format!(
        "inferred current {} stale {} needs revalidation {}",
        self.inferred_current, self.inferred_stale, self.inferred_needs_revalidation
      ),
    );
    out.line(
      0,
      format!(
        "llm calls {} tokens in {} out {} cost ~${:.4}",
        self.llm_calls, self.llm_input_tokens, self.llm_output_tokens, self.llm_cost_estimate_usd
      ),
    );
    out.line(0, format!("dangling detector edges dropped {}", self.dangling_detector_edges_dropped));
    out.line(0, "recent runs");
    for run in &self.runs {
      let detail = run.detail.as_deref().map(|detail| format!(" ({detail})")).unwrap_or_default();
      out.line(1, format!("run {} {} {}{detail}", run.id, run.trigger, run.status));
    }
  }
}

impl RenderText for ReadAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, entity_line(&self.entity));
    snippet(out, 0, &self.source);
  }
}

impl RenderText for ChangesAnswer {
  fn render_text(&self, out: &mut TextOut) {
    if self.runs.is_empty() {
      out.line(0, "no runs in range");
    }
    for change in &self.runs {
      let detail = change.run.detail.as_deref().map(|detail| format!(" ({detail})")).unwrap_or_default();
      out.line(0, format!("run {} {} {}{detail}", change.run.id, change.run.trigger, change.run.status));
      for (label, entries) in [("analyzed", &change.analyzed), ("deleted", &change.deleted), ("moved", &change.moved)] {
        if !entries.is_empty() {
          out.line(1, format!("{label}: {}", entries.join(", ")));
        }
      }
      if change.omitted_entries > 0 {
        out.line(1, format!("{} more file entries omitted", change.omitted_entries));
      }
      for owner in &change.owners.items {
        out.line(
          1,
          format!("{} entities {} edges {} unresolved {}", owner.owner, owner.entities, owner.edges, owner.unresolved),
        );
      }
      if change.owners.omitted() > 0 {
        out.line(1, format!("{} more changed owners", change.owners.omitted()));
      }
      for rename in &change.renames {
        let new = rename.new.as_ref().map_or_else(|| "<gone>".to_owned(), entity_line);
        out.line(1, format!("renamed {} -> {new} {}", rename.old_key, rename.certainty));
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;

  use super::*;
  use crate::queries::SkippedFileView;

  fn answer(scip: Option<ScipState>, skipped_total: u64, skipped_files: Vec<SkippedFileView>) -> StatusAnswer {
    StatusAnswer {
      project: "acme".to_owned(),
      repos: vec!["acme-api".to_owned()],
      runs: Vec::new(),
      files_by_language: BTreeMap::new(),
      files_by_parse_status: BTreeMap::new(),
      failed_files: Vec::new(),
      repos_missing: Vec::new(),
      unresolved_by_reason: BTreeMap::new(),
      edge_certainty: BTreeMap::new(),
      scip_files: 0,
      scip_total: 58,
      scip,
      inferred_current: 0,
      inferred_stale: 0,
      inferred_needs_revalidation: 0,
      llm_calls: 0,
      llm_input_tokens: 0,
      llm_output_tokens: 0,
      llm_cost_estimate_usd: 0.0,
      dangling_detector_edges_dropped: 0,
      skipped_total,
      skipped_files,
    }
  }

  fn render(answer: &StatusAnswer) -> String {
    let mut out = TextOut::default();
    answer.render_text(&mut out);
    out.finish()
  }

  #[test]
  fn should_render_scip_state_as_human_text_without_debug_formatting() {
    let never = render(&answer(Some(ScipState::NeverRun), 0, Vec::new()));
    assert!(never.contains("scip: never run (0/58 files covered)"), "{never}");
    let behind = render(&answer(Some(ScipState::Behind { files: 3 }), 0, Vec::new()));
    assert!(behind.contains("scip: behind on 3 files (0/58 files covered)"), "{behind}");
    let current = render(&answer(Some(ScipState::Current), 0, Vec::new()));
    assert!(current.contains("scip: current (0/58 files covered)"), "{current}");
    for text in [never, behind, current] {
      assert!(!text.contains("Some(") && !text.contains("NeverRun") && !text.contains("Behind {"), "{text}");
    }
  }

  #[test]
  fn should_render_skipped_files_with_their_reasons_and_the_omitted_count() {
    let big = SkippedFileView {
      repo: "acme-api".to_owned(),
      path: "big.ts".to_owned(),
      reason: "6.1 MB > max_file_bytes 1 MB".to_owned(),
    };
    let one = render(&answer(Some(ScipState::Current), 1, vec![big.clone()]));
    assert!(one.contains("skipped 1 file: acme-api/big.ts (6.1 MB > max_file_bytes 1 MB)\n"), "{one}");
    let many = render(&answer(Some(ScipState::Current), 4, vec![big]));
    assert!(many.contains("skipped 4 files: acme-api/big.ts (6.1 MB > max_file_bytes 1 MB), and 3 more\n"), "{many}");
    let none = render(&answer(Some(ScipState::Current), 0, Vec::new()));
    assert!(!none.contains("skipped"), "{none}");
  }
}
