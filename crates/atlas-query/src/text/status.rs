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
    out.line(0, format!("scip {}/{} files {:?}", self.scip_files, self.scip_total, self.scip));
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
