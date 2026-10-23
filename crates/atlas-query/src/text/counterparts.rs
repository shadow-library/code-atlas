use crate::queries::{Counterpart, CounterpartSymbol, CounterpartsAnswer};
use crate::text::{RenderText, TextOut, caveats};

pub(crate) const COUNTERPARTS: &str = "COUNTERPARTS";

pub(crate) fn counterpart_lines(out: &mut TextOut, indent: usize, counterpart: &Counterpart) {
  out.line(
    indent,
    format!("{}:{} {} score={:.2}", counterpart.repo, counterpart.path, counterpart.certainty.tag, counterpart.score),
  );
  out.line(indent + 1, format!("file: {}", counterpart.absolute_path));
  let rarest: Vec<String> = counterpart.shared.rarest.iter().map(|token| format!("'{token}'")).collect();
  out.line(indent + 1, format!("shared values ({}), rarest: {}", counterpart.shared.count, rarest.join(", ")));
  if !counterpart.symbols.is_empty() {
    let symbols: Vec<String> = counterpart.symbols.iter().map(CounterpartSymbol::located).collect();
    out.line(indent + 1, format!("most shared in {}", symbols.join(", ")));
  }
  if !counterpart.only_there.is_empty() {
    out.line(indent + 1, "only on that side, next to shared values:");
    for value in &counterpart.only_there {
      out.line(indent + 2, format!("{}  {}", value.line, value.text));
    }
  }
}

pub(crate) fn counterparts_section(out: &mut TextOut, counterparts: &[Counterpart]) {
  if counterparts.is_empty() {
    return;
  }
  out.line(
    0,
    format!(
      "{COUNTERPARTS} ({}): files in other repos or packages that share contract values with it",
      counterparts.len()
    ),
  );
  for counterpart in counterparts {
    counterpart_lines(out, 1, counterpart);
  }
}

impl RenderText for CounterpartsAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("counterparts of {}:{} ({})", self.file.repo, self.file.path, self.counterparts.len()));
    if self.counterparts.is_empty() {
      out.line(1, "none: no file in another repo or package shares enough rare string values and keys with it");
    }
    for counterpart in &self.counterparts {
      counterpart_lines(out, 1, counterpart);
    }
    caveats(out, &self.caveats);
  }
}
