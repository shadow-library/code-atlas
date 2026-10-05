use atlas_core::EntityKind;

use crate::queries::{ClusterMember, DraftMatches, DraftOrigin, DraftSimilarAnswer, SearchScope, SimilarAnswer, SimilaritySignal};
use crate::text::{RenderText, TextOut, caveats, entity_line};

fn signals(member: &ClusterMember) -> String {
  member
    .signals
    .iter()
    .map(|signal| match signal {
      SimilaritySignal::Structural { jaccard } => format!("structural {jaccard:.2}"),
      SimilaritySignal::SmallStructural { jaccard } => format!("structural {jaccard:.2} (small body, corroborated by name)"),
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
      if let Some(importability) = &member.importability {
        out.line(2, format!("importability: {importability}"));
      }
      out.line(2, usage(member));
    }
    if !self.shape_only.is_empty() {
      out.line(0, format!("shape-only, low weight ({})", self.shape_only.len()));
      for hit in &self.shape_only {
        out.line(1, format!("{} [structural {:.2}] {}", entity_line(&hit.entity), hit.jaccard, hit.note));
      }
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

fn lines_label(lines: u32) -> String {
  match lines {
    1 => "1 line".to_owned(),
    count => format!("{count} lines"),
  }
}

fn scope_line(scope: &SearchScope) -> String {
  format!(
    "searched {} symbols in {} (index of {} symbols; structural comparison needs bodies of at least {} tokens); structural threshold {:.2}",
    scope.languages.join(", "),
    scope.repos.join(", "),
    scope.symbols,
    scope.min_tokens,
    scope.threshold
  )
}

fn draft_head(draft: &DraftMatches, language: &str) -> String {
  match draft.draft.origin {
    DraftOrigin::Declared => {
      let word = if draft.draft.kind == EntityKind::Method { "method" } else { "fn" };
      format!("similar to draft {word} {} ({language}, {})", draft.draft.name, lines_label(draft.draft.lines))
    }
    DraftOrigin::WrappedFragment => format!("similar to draft fragment ({language}, {})", lines_label(draft.draft.lines)),
  }
}

fn draft_member(out: &mut TextOut, member: &ClusterMember) {
  out.line(1, format!("{} [{}]", entity_line(&member.entity), member.language));
  out.line(2, format!("evidence: {}", signals(member)));
  out.line(2, format!("verdict: {}", member.verdict));
  if let Some(importability) = &member.importability {
    out.line(2, format!("importability: {importability}"));
  }
  out.line(2, usage(member));
}

impl RenderText for DraftSimilarAnswer {
  fn render_text(&self, out: &mut TextOut) {
    if self.drafts.is_empty() {
      out.line(0, format!("similar to draft ({}, {}): nothing to compare", self.language, lines_label(self.lines)));
    }
    for draft in &self.drafts {
      out.line(0, draft_head(draft, &self.language));
      out.line(0, format!("candidates ({})", draft.members.len()));
      draft.members.iter().for_each(|member| draft_member(out, member));
      if !draft.related.is_empty() {
        out.line(0, format!("related by name/signature (low precision) ({}):", draft.related.len()));
        draft.related.iter().for_each(|member| draft_member(out, member));
      }
      if !draft.shape_only.is_empty() {
        out.line(0, format!("shape-only, low weight ({})", draft.shape_only.len()));
        for hit in &draft.shape_only {
          out.line(1, format!("{} [structural {:.2}] {}", entity_line(&hit.entity), hit.jaccard, hit.note));
        }
      }
      if draft.members.is_empty() && draft.related.is_empty() && draft.shape_only.is_empty() {
        let searched = match draft.draft.origin {
          DraftOrigin::Declared => "no structural, lexical or signature neighbours found",
          DraftOrigin::WrappedFragment => "no structural neighbours found",
        };
        out.line(1, searched);
        out.line(1, scope_line(&draft.searched));
      }
      caveats(out, &draft.caveats);
    }
    out.line(0, "similarity is evidence, not a decision: matches are similar text or shape, behaviour is unverified");
    caveats(out, &self.caveats);
  }
}
