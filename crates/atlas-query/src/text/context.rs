use crate::queries::{ContextAnswer, ContextItem, LineRange, RangeBasis, SectionSummary};
use crate::text::{RenderText, TextOut, caveats, entity_head, snippet};

fn range(lines: &LineRange) -> String {
  let span = match lines.start == lines.end {
    true => lines.start.to_string(),
    false => format!("{}-{}", lines.start, lines.end),
  };
  match lines.basis {
    RangeBasis::Symbol => span,
    RangeBasis::Site => format!("{span} (site)"),
  }
}

fn place(item: &ContextItem) -> String {
  let entity = &item.entity;
  let located = match (&entity.location, &item.lines) {
    (Some(location), Some(lines)) => format!("{} {} {}:{}:{}", entity.short, entity.kind, location.repo, location.path, range(lines)),
    _ => entity_head(entity),
  };
  match entity.tag() {
    Some(tag) => format!("{located} {tag}"),
    None => located,
  }
}

fn header(item: &ContextItem) -> String {
  let flag = if item.uncertain { " UNCERTAIN" } else { "" };
  let edit = if item.read_directly { " (named in the task: read the file directly)" } else { "" };
  format!("{}{flag}{edit}  why: {}", place(item), item.why)
}

fn section(out: &mut TextOut, title: &str, items: &[ContextItem], summaries: &[SectionSummary], render: impl Fn(&mut TextOut, &ContextItem)) {
  if items.is_empty() {
    return;
  }
  out.line(0, format!("{title} ({})", items.len()));
  for item in items {
    render(out, item);
  }
  for summary in summaries.iter().filter(|summary| summary.section == title) {
    let repos: Vec<String> = summary.by_repo.iter().map(|(repo, count)| format!("{repo} {count}")).collect();
    out.line(
      1,
      format!("{} more omitted ({} uncertain; {}): {}", summary.omitted, summary.uncertain, repos.join(", "), summary.hint),
    );
  }
}

fn mention(out: &mut TextOut, item: &ContextItem) {
  out.line(1, header(item));
}

fn skim(out: &mut TextOut, item: &ContextItem) {
  out.line(1, header(item));
  if let Some(signature) = &item.signature {
    out.line(2, format!("sig {signature}"));
  }
  if let Some(doc) = &item.doc {
    out.line(2, format!("doc {doc}"));
  }
  if let Some(excerpt) = &item.excerpt {
    snippet(out, 2, excerpt);
  }
  for member in &item.skeleton {
    out.line(3, member);
  }
}

impl RenderText for ContextAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("context for {:?} (~{} of {} tokens)", self.task, self.estimated_tokens, self.budget));
    let summaries = &self.summaries;
    section(out, "MUST READ", &self.must_read, summaries, |out, item| match &item.body {
      Some(body) => {
        out.line(1, header(item));
        snippet(out, 2, body);
        for member in &item.skeleton {
          out.line(3, member);
        }
      }
      None => skim(out, item),
    });
    section(out, "SKIM", &self.skim, summaries, skim);
    section(out, "PATTERN", &self.patterns, summaries, mention);
    section(out, "REUSE CANDIDATES", &self.reuse, summaries, mention);
    section(out, "IMPACT", &self.impact, summaries, mention);
    section(out, "TESTS", &self.tests, summaries, |out, item| {
      out.line(1, header(item));
      if let Some(run) = &item.run {
        out.line(2, format!("run: {run}"));
      }
    });
    section(out, "UNCERTAIN", &self.uncertain, summaries, mention);
    if !self.omitted.is_empty() {
      out.line(0, format!("OMITTED ({})", self.omitted.total));
      for item in &self.omitted.items {
        out.line(1, format!("{} [{}]", item.label, item.reason.as_str()));
      }
      if self.omitted.omitted() > 0 {
        out.line(1, format!("{} more omitted items (scores with explain=true)", self.omitted.omitted()));
      }
    }
    if let Some(explain) = &self.explain {
      out.line(0, "EXPLAIN seeds");
      for seed in &explain.seeds {
        out.line(1, seed);
      }
      out.line(0, "EXPLAIN expansions");
      for expansion in &explain.expansions {
        out.line(1, expansion);
      }
      out.line(0, "EXPLAIN scores");
      for scored in &explain.scores {
        out.line(1, format!("{:.6} {}", scored.score, scored.key));
      }
    }
    caveats(out, &self.caveats);
  }
}
