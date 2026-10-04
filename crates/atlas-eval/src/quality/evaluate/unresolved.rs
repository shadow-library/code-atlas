use atlas_core::{CertaintyClass, Edge, EdgeKind, Entity, EntityId, EntityKey, RefKind, Span, UnresolvedReason, UnresolvedRef};

use super::UnresolvedScore;
use super::context::{Ctx, Resolved};
use crate::quality::metrics::RecallOutcome;
use crate::quality::truth::UnresolvedTruth;

const SECTION: &str = "unresolved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReasonMatch {
  Wrong,
  Approximate,
  Exact,
}

pub fn evaluate(ctx: &Ctx<'_>) -> UnresolvedScore {
  let mut score = UnresolvedScore::default();
  for truth in &ctx.truth.unresolved {
    let label = format!("{} {} ({})", truth.ref_kind, truth.name.as_deref().unwrap_or("*"), truth.from);
    let ids = match ctx.resolve_in_project(SECTION, &truth.from, None) {
      Resolved::Ids(ids) => ids,
      Resolved::Missing(detail) => {
        let outcome = RecallOutcome::Missing { detail };
        ctx.finding(SECTION, &truth.ref_kind, label, outcome.describe());
        score.presence.record(&outcome);
        continue;
      }
      Resolved::Unbound | Resolved::Foreign | Resolved::Absent => {
        score.presence.record(&RecallOutcome::Unbound);
        continue;
      }
    };
    let Some(&subject) = ids.first() else {
      score.presence.record(&RecallOutcome::Unbound);
      continue;
    };
    let refs: Vec<&UnresolvedRef> = ctx
      .snapshot
      .unresolved_from(subject)
      .filter(|reference| kind_matches(&truth.ref_kind, reference) && name_matches(truth, reference))
      .collect();
    let best = refs.iter().map(|reference| reason_match(&truth.reason, &reference.reason)).max();
    let best = best.max(diagnostic_match(ctx, truth, subject));
    let outcome = match best {
      Some(ReasonMatch::Exact) => RecallOutcome::Found,
      Some(ReasonMatch::Approximate) => {
        score.approximate_reason += 1;
        RecallOutcome::Found
      }
      Some(ReasonMatch::Wrong) => RecallOutcome::WrongDetail {
        detail: format!("recorded as {}, expected {}", describe_reasons(&refs), truth.reason),
      },
      None => RecallOutcome::Missing {
        detail: "no unresolved reference or diagnostic recorded at the site".to_owned(),
      },
    };
    if !outcome.is_found() {
      ctx.finding(SECTION, &truth.ref_kind, label.clone(), outcome.describe());
    }
    score.presence.record(&outcome);
    check_fabrication(ctx, truth, subject, &refs, &label, &mut score);
  }
  score
}

fn describe_reasons(refs: &[&UnresolvedRef]) -> String {
  let mut reasons: Vec<String> = refs.iter().map(|reference| format!("{:?}", reference.reason)).collect();
  reasons.sort();
  reasons.dedup();
  reasons.join(" / ")
}

fn kind_matches(ref_kind: &str, reference: &UnresolvedRef) -> bool {
  let head = reference.path.first().map(String::as_str);
  match ref_kind {
    "import" => head == Some("import"),
    "require" => head == Some("require"),
    "include" => head == Some("include"),
    "call" => {
      matches!(reference.kind, RefKind::Call | RefKind::MethodCall | RefKind::MacroCall | RefKind::Instantiation) && !matches!(head, Some("import" | "require" | "include"))
    }
    "reference" => matches!(
      reference.kind,
      RefKind::ValueRef | RefKind::TypeRef | RefKind::Decorator | RefKind::Inherit | RefKind::JsxElement
    ),
    _ => false,
  }
}

fn name_matches(truth: &UnresolvedTruth, reference: &UnresolvedRef) -> bool {
  truth.name.as_ref().is_none_or(|name| reference.path.iter().any(|segment| segment.contains(name.as_str())))
}

fn reason_match(expected: &str, actual: &UnresolvedReason) -> ReasonMatch {
  match (expected, actual) {
    ("dynamic", UnresolvedReason::Dynamic) | ("ambiguous", UnresolvedReason::Ambiguous { .. }) | ("external", UnresolvedReason::External { .. }) => ReasonMatch::Exact,
    ("macro_generated", UnresolvedReason::Unsupported { detail }) if detail == "macro_generated" => ReasonMatch::Exact,
    ("generated_code", UnresolvedReason::Unsupported { detail }) if detail == "macro_generated" => ReasonMatch::Approximate,
    _ => ReasonMatch::Wrong,
  }
}

fn diagnostic_match(ctx: &Ctx<'_>, truth: &UnresolvedTruth, subject: EntityId) -> Option<ReasonMatch> {
  if truth.reason != "dynamic" || !matches!(truth.ref_kind.as_str(), "route" | "call") {
    return None;
  }
  let entity = ctx.snapshot.entity(subject)?;
  let file = file_of(ctx, entity)?;
  let wanted = match truth.ref_kind.as_str() {
    "route" => "route-dynamic",
    _ => "http.dynamic_url",
  };
  let span = match &entity.key {
    EntityKey::File { .. } => None,
    _ => entity.span,
  };
  let diagnostics = file.attrs.get("detector_diagnostics")?.as_array()?;
  let hit = diagnostics.iter().any(|item| {
    let diagnostic = &item["diagnostic"];
    let code_ok = diagnostic["code"].as_str() == Some(wanted);
    let line = diagnostic["span"]["start_line"].as_u64().unwrap_or(0) as u32;
    code_ok && span.is_none_or(|span| span.start_line <= line && line <= span.end_line)
  });
  hit.then_some(ReasonMatch::Exact)
}

fn file_of<'s>(ctx: &'s Ctx<'_>, entity: &Entity) -> Option<&'s Entity> {
  let repo = entity.repo.clone()?;
  let path = match &entity.key {
    EntityKey::File { path, .. } => path.clone(),
    _ => entity.path.clone()?,
  };
  ctx.snapshot.entities.get(&atlas_core::EntityId::of(&EntityKey::File { repo, path }))
}

fn edge_kinds(ref_kind: &str) -> &'static [EdgeKind] {
  match ref_kind {
    "call" => &[EdgeKind::Calls, EdgeKind::Instantiates, EdgeKind::References],
    "reference" => &[EdgeKind::References, EdgeKind::Calls],
    "import" | "require" | "include" => &[EdgeKind::Imports],
    "route" => &[EdgeKind::ExposesEndpoint],
    _ => &[],
  }
}

fn overlaps(edge: &Edge, reference: &UnresolvedRef) -> bool {
  edge
    .evidence
    .iter()
    .any(|evidence| evidence.path == reference.file && span_overlaps(&evidence.span, &reference.span))
}

fn span_overlaps(left: &Span, right: &Span) -> bool {
  left.start_byte < right.end_byte && right.start_byte < left.end_byte
}

fn check_fabrication(ctx: &Ctx<'_>, truth: &UnresolvedTruth, subject: EntityId, refs: &[&UnresolvedRef], label: &str, score: &mut UnresolvedScore) {
  let kinds = edge_kinds(&truth.ref_kind);
  let candidate_edges: Vec<&Edge> = ctx
    .snapshot
    .edges_from(subject)
    .filter(|edge| kinds.contains(&edge.kind))
    .filter(|edge| match refs.is_empty() {
      false => refs.iter().any(|reference| overlaps(edge, reference)),
      true => truth
        .name
        .as_ref()
        .is_some_and(|name| ctx.snapshot.entity(edge.to).is_some_and(|target| target.key.display_name() == *name)),
    })
    .collect();
  let checkable = !refs.is_empty() || truth.name.is_some();
  if !checkable {
    score.not_checkable += 1;
    return;
  }
  let candidates: Vec<EntityId> = truth.candidates.iter().flat_map(|selector| ctx.binding(selector, None).ids()).collect();
  let tolerate_heuristic = truth.max_certainty.as_deref() == Some("heuristic");
  let lifted: Vec<CertaintyClass> = truth.unless_certainty.iter().filter_map(|class| class.parse().ok()).collect();
  let violations: Vec<&Edge> = candidate_edges
    .into_iter()
    .filter(|edge| {
      let class = edge.certainty.class();
      if lifted.contains(&class) {
        return false;
      }
      match class {
        CertaintyClass::Resolved | CertaintyClass::Syntactic | CertaintyClass::CompilerVerified => true,
        CertaintyClass::Heuristic => !(tolerate_heuristic && (candidates.is_empty() || candidates.contains(&edge.to))),
        CertaintyClass::Inferred | CertaintyClass::Declared => false,
      }
    })
    .collect();
  score.fabrication.checked += 1;
  if violations.is_empty() {
    return;
  }
  score.fabrication.violations += 1;
  let described: Vec<String> = violations
    .iter()
    .map(|edge| format!("{} -> {} {}", edge.kind, ctx.snapshot.describe(edge.to), edge.certainty.tag()))
    .collect();
  ctx.finding(
    "unresolved.fabricated",
    &truth.ref_kind,
    label.to_owned(),
    format!("edge despite unresolved reference: {}", described.join("; ")),
  );
}

#[cfg(test)]
mod tests {
  use atlas_core::UnresolvedReason;

  use super::*;

  #[test]
  fn should_map_resolver_reasons_onto_ground_truth_reasons() {
    let macro_generated = UnresolvedReason::Unsupported { detail: "macro_generated".into() };
    assert_eq!(reason_match("macro_generated", &macro_generated), ReasonMatch::Exact);
    assert_eq!(reason_match("generated_code", &macro_generated), ReasonMatch::Approximate);
    assert_eq!(reason_match("dynamic", &UnresolvedReason::Dynamic), ReasonMatch::Exact);
    assert_eq!(reason_match("dynamic", &UnresolvedReason::NotFound), ReasonMatch::Wrong);
    assert_eq!(reason_match("external", &UnresolvedReason::External { package: "std".into() }), ReasonMatch::Exact);
    assert!(ReasonMatch::Exact > ReasonMatch::Approximate && ReasonMatch::Approximate > ReasonMatch::Wrong);
  }
}
