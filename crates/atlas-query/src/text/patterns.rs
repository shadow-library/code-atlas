use crate::queries::PatternsAnswer;
use crate::text::{RenderText, TextOut, counted, entity_line, snippet};

impl RenderText for PatternsAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(
      0,
      format!(
        "patterns for {:?}: {} instances in {} shapes",
        self.query,
        self.instances,
        self.shapes.len() + self.omitted_shapes
      ),
    );
    if self.instances == 0 {
      out.line(1, "no facts of this kind are indexed");
    }
    for (rank, shape) in self.shapes.iter().enumerate() {
      out.line(0, format!("{}. {} ({})", rank + 1, shape.shape, shape.count));
      for exemplar in &shape.exemplars {
        out.line(1, entity_line(&exemplar.entity));
        if let Some(signature) = &exemplar.signature {
          out.line(2, format!("sig {signature}"));
        }
        if let Some(excerpt) = &exemplar.excerpt {
          snippet(out, 2, excerpt);
        }
      }
      if shape.count > shape.exemplars.len() {
        out.line(1, counted(shape.count - shape.exemplars.len(), "more instance"));
      }
    }
    if self.omitted_shapes > 0 {
      out.line(0, format!("{} more shapes omitted (raise limit)", self.omitted_shapes));
    }
  }
}
