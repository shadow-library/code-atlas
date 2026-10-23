use crate::queries::{AlsoChangeAnswer, AlsoChangeFile};
use crate::text::{RenderText, TextOut, caveats};

fn also_change_line(file: &AlsoChangeFile) -> String {
  let tag = match file.certainty.authoritative {
    true => String::new(),
    false => format!(" {}", file.certainty.tag),
  };
  format!("{}:{} score={:.2} — {}{tag}", file.repo, file.path, file.score, file.reason)
}

impl RenderText for AlsoChangeAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let changed: Vec<String> = self.changed.iter().map(|file| format!("{}:{}", file.repo, file.path)).collect();
    out.line(0, format!("also change with {} ({})", changed.join(", "), self.files.len()));
    if self.files.is_empty() {
      out.line(1, "none: no other file is linked to these in the graph");
    }
    for file in &self.files {
      out.line(1, also_change_line(file));
    }
    caveats(out, &self.caveats);
  }
}
