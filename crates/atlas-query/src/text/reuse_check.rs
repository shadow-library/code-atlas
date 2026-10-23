use crate::queries::{IndexState, ReuseAnswer, ReuseCandidate};
use crate::text::{RenderText, TextOut, caveats};

pub(crate) fn candidate_line(candidate: &ReuseCandidate) -> String {
  let location = candidate.entity.location.as_ref().map_or_else(String::new, |location| {
    format!(" {}:{}:{}-{}", location.repo, location.path, candidate.start_line, candidate.end_line)
  });
  format!("{} {}{location} — {}", candidate.certainty.tag, candidate.entity.short, candidate.reason.text())
}

impl RenderText for ReuseAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let candidates = self.candidates();
    out.line(
      0,
      format!(
        "reuse check of {}:{} ({} checked, {})",
        self.file.repo,
        self.file.path,
        counted(self.units.len(), "function"),
        counted(candidates, "candidate")
      ),
    );
    if self.index_state == IndexState::Changed {
      out.line(1, "note: the file on disk differs from the index, so its own symbols are matched by the indexed version");
    }
    for unit in &self.units {
      out.line(
        1,
        format!("{} ({}) {}-{}, {} tokens", unit.name, unit.status.label(), unit.start_line, unit.end_line, unit.tokens),
      );
      for candidate in &unit.candidates {
        out.line(2, candidate_line(candidate));
        if let Some(signature) = &candidate.signature {
          out.line(3, signature);
        }
      }
    }
    if candidates == 0 {
      out.line(1, "none: no existing function looks like the checked code");
    }
    caveats(out, &self.caveats);
  }
}

fn counted(count: usize, noun: &str) -> String {
  match count {
    1 => format!("1 {noun}"),
    count => format!("{count} {noun}s"),
  }
}
