use crate::queries::TraversalAnswer;
use crate::text::{RenderText, TextOut, caveats, entity_line, omitted, relation_text};

impl RenderText for TraversalAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let http = if self.via_http { " via_http" } else { "" };
    out.line(0, format!("{} of {} depth={}{http}", self.kind.as_str(), entity_line(&self.target), self.depth));
    if let Some(symbols) = self.aggregated_symbols {
      out.line(1, format!("file target: aggregated over {symbols} symbols"));
    }
    if self.total == 0 {
      let hops = if self.via_http { " and HTTP hops" } else { "" };
      out.line(1, format!("none found over {}{hops}", self.edge_kinds.join(", ")));
    }
    for group in &self.groups {
      out.line(0, format!("{} ({})", group.repo, group.total));
      for item in &group.items {
        out.line(1, format!("{}  {}", entity_line(&item.entity), relation_text(&item.relation, &item.path)));
      }
    }
    omitted(out, 0, self.total - self.shown);
    caveats(out, &self.caveats);
  }
}
