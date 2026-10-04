use crate::queries::TestsForAnswer;
use crate::text::{RenderText, TextOut, caveats, entity_line, listing, relation_text};

impl RenderText for TestsForAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("tests for {} ({})", entity_line(&self.target), self.tests.total));
    if self.tests.is_empty() {
      out.line(1, "no tests found; the target may be untested or reached only through unresolved references");
    }
    listing(out, 1, &self.tests, |out, indent, test| {
      out.line(
        indent,
        format!("{} [{}]  {}", entity_line(&test.test), test.reach.as_str(), relation_text(&test.relation, &test.path)),
      );
      if let Some(run) = &test.run {
        out.line(indent + 1, format!("run: {run}"));
      }
    });
    caveats(out, &self.caveats);
  }
}
