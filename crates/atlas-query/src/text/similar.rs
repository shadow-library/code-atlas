use crate::queries::{ClusterMember, SimilarAnswer, SimilaritySignal};
use crate::text::{RenderText, TextOut, caveats, entity_line};

fn signals(member: &ClusterMember) -> String {
  member
    .signals
    .iter()
    .map(|signal| match signal {
      SimilaritySignal::Structural { jaccard } => format!("structural {jaccard:.2}"),
      SimilaritySignal::Lexical { score, terms } => format!("lexical {score:.2} ({})", terms.join(" ")),
      SimilaritySignal::Inferred { certainty, .. } => format!("inferred {}", certainty.tag),
      SimilaritySignal::SignatureShape { shape } => format!("same signature {shape}"),
    })
    .collect::<Vec<_>>()
    .join(", ")
}

fn usage(member: &ClusterMember) -> String {
  let repos = match member.consumers.repos.is_empty() {
    true => "-".to_owned(),
    false => member.consumers.repos.join(", "),
  };
  let uncertain = match member.consumers.uncertain {
    0 => String::new(),
    count => format!(" ({count} uncertain)"),
  };
  format!("consumers {}{uncertain} in {repos}; tests {}", member.consumers.callers, member.tests)
}

impl RenderText for SimilarAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("similar to {} [{}]", entity_line(&self.target.entity), self.target.language));
    out.line(1, usage(&self.target));
    out.line(0, format!("candidates ({})", self.members.len()));
    if self.members.is_empty() {
      out.line(1, "no structural, lexical or inferred neighbours found");
    }
    for member in &self.members {
      out.line(1, format!("{} [{}]", entity_line(&member.entity), member.language));
      out.line(2, format!("evidence: {}", signals(member)));
      out.line(2, format!("verdict: {}", member.verdict));
      out.line(2, usage(member));
    }
    if !self.shared_home.is_empty() {
      out.line(0, "likely shared home");
      for home in &self.shared_home {
        out.line(1, format!("{}: {} ({})", home.language, home.package.as_deref().unwrap_or("-"), home.reason));
      }
    }
    if !self.coupling_risks.is_empty() {
      out.line(0, "coupling risk");
      for risk in &self.coupling_risks {
        out.line(1, risk);
      }
    }
    caveats(out, &self.caveats);
  }
}
