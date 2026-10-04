use crate::model::AttrView;
use crate::queries::{AssertionView, ExplainAnswer};
use crate::text::{RenderText, TextOut, entity_line, snippet};

fn assertions(out: &mut TextOut, assertions: &[AssertionView]) {
  out.line(0, format!("assertions ({})", assertions.len()));
  for assertion in assertions {
    out.line(1, format!("{} {} ({})", assertion.owner, assertion.certainty.tag, assertion.derived_in));
    for evidence in &assertion.evidence {
      let note = evidence.note.as_deref().map(|note| format!(" note: {note}")).unwrap_or_default();
      out.line(2, format!("evidence {} by {}{note}", evidence.location, evidence.producer));
      if let Some(excerpt) = &evidence.excerpt {
        snippet(out, 3, excerpt);
      }
    }
  }
}

fn attr_lines(out: &mut TextOut, attrs: &[AttrView]) {
  for attr in attrs {
    out.line(1, format!("attr {}: {} {}", attr.name, attr.value_text(), attr.tag()));
  }
}

impl RenderText for ExplainAnswer {
  fn render_text(&self, out: &mut TextOut) {
    match self {
      Self::Entity {
        entity,
        certainty,
        attrs,
        assertions: owned,
        lineage,
        inferred,
      } => {
        out.line(0, format!("{} {}", entity_line(entity), certainty.tag));
        out.line(1, format!("key {}", entity.key));
        out.line(1, format!("id {}", entity.id));
        attr_lines(out, attrs);
        assertions(out, owned);
        if !lineage.is_empty() {
          out.line(0, "lineage");
          for step in lineage {
            let run = step.run.map(|run| format!(" in run {run}")).unwrap_or_default();
            out.line(1, format!("renamed from {} {}{run}", step.old_key, step.certainty));
          }
        }
        if !inferred.is_empty() {
          out.line(0, format!("inferred facts ({})", inferred.len()));
          for fact in inferred {
            out.line(1, format!("{} {} {}", fact.relation, fact.subjects.join(", "), fact.certainty.tag));
            let call = fact.llm_call.map(|call| format!(" call {call}")).unwrap_or_default();
            let run = fact.created_run.map(|run| format!(" in run {run}")).unwrap_or_default();
            out.line(2, format!("by {}/{}{call}{run}", fact.provider, fact.model));
            out.line(2, format!("rationale: {}", fact.rationale));
          }
        }
      }
      Self::Edge {
        id,
        kind,
        from,
        to,
        certainty,
        attrs,
        assertions: owned,
      } => {
        let name = |entity: &Option<crate::model::EntityRef>| entity.as_ref().map_or_else(|| "<missing>".to_owned(), entity_line);
        out.line(0, format!("edge {id} {kind} {}", certainty.tag));
        out.line(1, format!("from {}", name(from)));
        out.line(1, format!("to {}", name(to)));
        attr_lines(out, attrs);
        assertions(out, owned);
      }
    }
  }
}
