use crate::queries::{ImpactAnswer, ImpactGroup};
use crate::text::{RenderText, TextOut, caveats, entity_line, path_text};

fn group(out: &mut TextOut, title: &str, group: &ImpactGroup) {
  let uncertain = match group.uncertain {
    0 => String::new(),
    count => format!(", {count} uncertain"),
  };
  out.line(0, format!("{title} ({}{uncertain})", group.total));
  for item in &group.items {
    let flag = if item.uncertain { " UNCERTAIN" } else { "" };
    out.line(1, format!("{} score={:.3}{flag}", entity_line(&item.entity), item.score));
    if !item.why.is_empty() {
      out.line(2, format!("why: {}", path_text(&item.why)));
    }
  }
  if group.collapsed > 0 {
    out.line(1, format!("{} lower-ranked authoritative items collapsed (raise limit)", group.collapsed));
  }
}

impl RenderText for ImpactAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let targets: Vec<String> = self.targets.iter().map(entity_line).collect();
    out.line(0, format!("impact of {} depth={}", targets.join(", "), self.depth));
    group(out, "direct dependents", &self.direct);
    group(out, "transitive dependents", &self.transitive);
    group(out, "affected endpoints", &self.endpoints);
    group(out, "endpoint consumers", &self.consumers);
    out.line(0, format!("affected repos ({})", self.repos.len()));
    for repo in &self.repos {
      let apps: Vec<String> = repo.applications.iter().map(|app| app.short.clone()).collect();
      let apps = match apps.is_empty() {
        true => String::new(),
        false => format!(" apps: {}", apps.join(", ")),
      };
      let flag = if repo.only_uncertain { " (only via uncertain edges)" } else { "" };
      out.line(1, format!("{}: {} items{apps}{flag}", repo.repo, repo.items));
    }
    group(out, "relevant tests", &self.tests);
    group(out, "config and env", &self.config);
    out.line(0, format!("unresolved risk ({})", self.unresolved_risk.len()));
    if !self.unresolved_risk.is_empty() {
      let mut nested = TextOut::default();
      caveats(&mut nested, &self.unresolved_risk);
      for line in nested.finish().lines().skip(1) {
        out.line(0, line);
      }
    }
    caveats(out, &self.caveats);
  }
}
