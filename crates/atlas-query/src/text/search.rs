use crate::queries::{SearchAnswer, SearchHitView};
use crate::text::{RenderText, TextOut, caveats, entity_line};

fn hits(out: &mut TextOut, title: &str, hits: &[SearchHitView]) {
  if hits.is_empty() {
    return;
  }
  out.line(0, format!("{title} ({})", hits.len()));
  for hit in hits {
    out.line(1, format!("{} score={:.3}", entity_line(&hit.entity), hit.score));
    if let Some(signature) = &hit.signature {
      out.line(2, format!("sig {signature}"));
    }
    if let Some(doc) = &hit.doc {
      out.line(2, format!("doc {doc}"));
    }
    let mut why = hit.matched_fields.join("; ");
    if !hit.expansion_terms.is_empty() {
      why.push_str(&format!("; expanded {}", hit.expansion_terms.join(", ")));
    }
    out.line(2, format!("matched {why}"));
  }
}

impl RenderText for SearchAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("search {:?}: {} hits", self.query, self.total_hits));
    hits(out, "exact name", &self.exact_name);
    hits(out, "strong", &self.strong);
    hits(out, "related", &self.related);
    if !self.structurally_similar.is_empty() {
      out.line(0, format!("structurally similar ({})", self.structurally_similar.len()));
      for hit in &self.structurally_similar {
        let shape = if hit.small_body { "similar small body to" } else { "same shape as" };
        out.line(
          1,
          format!(
            "{} {shape} {} [structural {:.2}], behaviour unverified",
            entity_line(&hit.entity),
            hit.similar_to,
            hit.jaccard
          ),
        );
      }
    }
    if !self.inferred_capabilities.is_empty() {
      out.line(0, format!("inferred capability summaries ({})", self.inferred_capabilities.len()));
      for hit in &self.inferred_capabilities {
        out.line(1, format!("{} {} likely: {}", entity_line(&hit.entity), hit.certainty.tag, hit.summary));
      }
    }
    caveats(out, &self.caveats);
  }
}
