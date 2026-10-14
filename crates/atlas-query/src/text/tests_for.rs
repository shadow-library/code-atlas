use crate::queries::{TestItem, TestsForAnswer};
use crate::text::{RenderText, TextOut, caveats, counted, entity_line, listing, omitted, relation_text};

fn test_lines(out: &mut TextOut, indent: usize, test: &TestItem) {
  out.line(
    indent,
    format!("{} [{}]  {}", entity_line(&test.test), test.reach.as_str(), relation_text(&test.relation, &test.path)),
  );
  if let Some(note) = &test.note {
    out.line(indent + 1, format!("note: {note}"));
  }
  if let Some(run) = &test.run {
    out.line(indent + 1, format!("run: {run}"));
  }
}

impl RenderText for TestsForAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("tests for {} ({})", entity_line(&self.target), self.tests.total));
    if self.tests.is_empty() {
      out.line(1, "no tests found; the target may be untested or reached only through unresolved references");
    }
    listing(out, 1, &self.tests, test_lines);
    if let Some(fallback) = &self.fallback {
      out.line(0, format!("possible tests from the method-name fallback ({})", fallback.tests.total));
      out.line(1, fallback.explanation.as_str());
      for test in &fallback.tests.items {
        test_lines(out, 1, test);
      }
      match fallback.capped_at {
        Some(cap) => out.line(1, format!("{} not shown: the fallback lists at most {cap} tests", fallback.tests.omitted())),
        None => omitted(out, 1, fallback.tests.omitted()),
      }
      if fallback.suppressed_by_mocks > 0 {
        out.line(
          1,
          format!(
            "{} left out because their scope replaces {} with a mock",
            counted(fallback.suppressed_by_mocks, "candidate"),
            fallback.replaced.join(", ")
          ),
        );
      }
    }
    caveats(out, &self.caveats);
  }
}
