use crate::queries::{UnusedAnswer, UnusedCandidate};
use crate::text::{RenderText, TextOut, caveats, entity_line, listing};

fn candidate(out: &mut TextOut, indent: usize, candidate: &UnusedCandidate) {
  out.line(indent, entity_line(&candidate.entity));
  out.line(indent + 1, candidate.evidence.join("; "));
}

impl RenderText for UnusedAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("likely unused ({})", self.likely_unused.total));
    listing(out, 1, &self.likely_unused, candidate);
    out.line(0, format!("possibly unused ({})", self.possibly_unused.total));
    listing(out, 1, &self.possibly_unused, candidate);
    if !self.excluded_entrypoints.is_empty() {
      let excluded: Vec<String> = self.excluded_entrypoints.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
      out.line(0, format!("excluded as entrypoints without inbound edges: {}", excluded.join(", ")));
    }
    caveats(out, &self.caveats);
  }
}
