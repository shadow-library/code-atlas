use crate::queries::{RelationCount, SymbolAnswer};
use crate::text::docs::pointer_sections;
use crate::text::{RenderText, TextOut, entity_head, entity_line, snippet};

fn count(label: &str, count: RelationCount) -> String {
  match count.uncertain {
    0 => format!("{label} {}", count.total),
    uncertain => format!("{label} {} ({uncertain} uncertain)", count.total),
  }
}

impl RenderText for SymbolAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("{} {}", entity_head(&self.entity), self.certainty.tag));
    out.line(1, format!("key {}", self.entity.key));
    if let Some(signature) = &self.signature {
      out.line(1, format!("sig {signature}"));
    }
    if let Some(doc) = &self.doc {
      out.line(1, format!("doc {doc}"));
    }
    if let Some(visibility) = &self.visibility {
      out.line(1, format!("visibility {visibility}"));
    }
    if let Some(parent) = &self.parent {
      out.line(1, format!("parent {}", entity_line(parent)));
    }
    let counts = &self.counts;
    out.line(
      1,
      [
        count("callers", counts.callers),
        count("callees", counts.callees),
        count("references", counts.references),
        count("tests", counts.tests),
        count("implementors", counts.implementors),
        format!("children {}", counts.children),
      ]
      .join(" | "),
    );
    if let Some(body) = &self.body {
      out.line(1, "body:");
      snippet(out, 2, body);
    }
    pointer_sections(out, &self.doc_pointers);
  }
}
