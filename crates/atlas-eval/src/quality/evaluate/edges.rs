use std::collections::{BTreeMap, BTreeSet, HashSet};

use atlas_core::{Edge, EdgeKind, EntityId};

use super::EdgeKindScore;
use super::context::{Ctx, Resolved, accepts, judge};
use crate::quality::metrics::{Calibration, Prohibition, Recall, RecallOutcome};
use crate::quality::truth::{EdgeTruth, ForbiddenTruth};

pub struct EdgeScores {
  pub by_kind: BTreeMap<String, EdgeKindScore>,
  pub total: Recall,
  pub forbidden: Prohibition,
  pub calibration: Calibration,
}

type Pair = (EdgeKind, EntityId, EntityId);

pub fn evaluate(ctx: &Ctx<'_>) -> EdgeScores {
  let mut scores = EdgeScores {
    by_kind: BTreeMap::new(),
    total: Recall::default(),
    forbidden: Prohibition::default(),
    calibration: Calibration::new(),
  };
  let mut expected: HashSet<Pair> = HashSet::new();
  let mut selected: BTreeSet<EntityId> = BTreeSet::new();
  for truth in &ctx.truth.edges {
    let kind = edge_kind(&truth.kind);
    let found = lookup(ctx, truth, &kind);
    for (from, to) in &found.pairs {
      expected.insert((kind.clone(), *from, *to));
      selected.extend([*from, *to]);
    }
    let entry = scores.by_kind.entry(truth.kind.clone()).or_default();
    if let Some((edge, wanted)) = &found.chosen {
      scores
        .calibration
        .record(edge.certainty.class().as_str(), edge.certainty.confidence().map(f32::from), judge(wanted, &edge.certainty));
    }
    if !truth.required {
      record_optional(ctx, truth, &found, entry);
      continue;
    }
    if !found.outcome.is_found() {
      ctx.finding("edge", &truth.kind, label(truth), found.outcome.describe());
    }
    entry.recall.record(&found.outcome);
    scores.total.record(&found.outcome);
  }
  for truth in &ctx.truth.forbidden {
    let kind = edge_kind(&truth.kind);
    let entry = scores.by_kind.entry(truth.kind.clone()).or_default();
    let before = entry.forbidden.clone();
    check_forbidden(ctx, truth, &kind, &mut entry.forbidden, &mut scores.calibration);
    scores.forbidden.absorb(&Prohibition {
      checked: entry.forbidden.checked - before.checked,
      violations: entry.forbidden.violations - before.violations,
      unbound: entry.forbidden.unbound - before.unbound,
      vacuous: entry.forbidden.vacuous - before.vacuous,
    });
  }
  for kind_name in &ctx.truth.project.exhaustive_edge_kinds {
    let kind = edge_kind(kind_name);
    let unexpected = ctx
      .snapshot
      .edges
      .iter()
      .filter(|edge| edge.kind == kind && selected.contains(&edge.from) && selected.contains(&edge.to))
      .filter(|edge| !expected.contains(&(kind.clone(), edge.from, edge.to)))
      .count() as u64;
    scores.by_kind.entry(kind_name.clone()).or_default().exhaustive_unexpected = Some(unexpected);
  }
  scores
}

fn edge_kind(name: &str) -> EdgeKind {
  name.parse().unwrap_or_else(|_| EdgeKind::Custom("unknown_edge_kind".to_owned()))
}

fn label(truth: &EdgeTruth) -> String {
  format!("{}: {} -> {}", truth.kind, truth.from, truth.to)
}

struct Lookup<'s> {
  outcome: RecallOutcome,
  chosen: Option<(&'s Edge, crate::quality::truth::Certainties)>,
  pairs: Vec<(EntityId, EntityId)>,
  present: bool,
}

fn lookup<'s>(ctx: &'s Ctx<'_>, truth: &EdgeTruth, kind: &EdgeKind) -> Lookup<'s> {
  let from = ctx.resolve_in_project("edge.from", &truth.from, None);
  let to = ctx.resolve_in_project("edge.to", &truth.to, None);
  let absent = |outcome| Lookup {
    outcome,
    chosen: None,
    pairs: Vec::new(),
    present: false,
  };
  let (from, to) = match (from, to) {
    (Resolved::Ids(from), Resolved::Ids(to)) => (from, to),
    (Resolved::Unbound | Resolved::Foreign | Resolved::Absent, _) | (_, Resolved::Unbound | Resolved::Foreign | Resolved::Absent) => return absent(RecallOutcome::Unbound),
    (Resolved::Missing(detail), _) | (_, Resolved::Missing(detail)) => return absent(RecallOutcome::Missing { detail }),
  };
  let wanted = truth.certainties();
  let candidates: Vec<&Edge> = from
    .iter()
    .flat_map(|id| ctx.snapshot.edges_from(*id))
    .filter(|edge| &edge.kind == kind && to.contains(&edge.to))
    .collect();
  let pairs: Vec<(EntityId, EntityId)> = candidates.iter().map(|edge| (edge.from, edge.to)).collect();
  let pairs = match pairs.is_empty() {
    true => from.iter().flat_map(|f| to.iter().map(move |t| (*f, *t))).take(1_000).collect(),
    false => pairs,
  };
  if candidates.is_empty() {
    let others: BTreeSet<String> = from
      .iter()
      .flat_map(|f| to.iter().flat_map(move |t| ctx.snapshot.edges_from(*f).filter(move |edge| edge.to == *t)))
      .map(|edge| format!("{} {}", edge.kind, edge.certainty.tag()))
      .collect();
    let detail = match others.is_empty() {
      true => "no edge between the entities".to_owned(),
      false => format!("only other edges exist between them: {}", others.into_iter().collect::<Vec<_>>().join(", ")),
    };
    return Lookup {
      outcome: RecallOutcome::Missing { detail },
      chosen: None,
      pairs,
      present: false,
    };
  }
  let accepted = candidates.iter().copied().find(|edge| accepts(&wanted, &edge.certainty));
  let strongest = || candidates.iter().copied().max_by(|left, right| left.certainty.cmp_strength(&right.certainty));
  let (edge, outcome) = match accepted {
    Some(edge) => (edge, occurrence_check(truth, edge)),
    None => {
      let edge = strongest().unwrap_or(candidates[0]);
      (edge, RecallOutcome::WrongCertainty { actual: edge.certainty.tag() })
    }
  };
  Lookup {
    outcome,
    chosen: Some((edge, wanted)),
    pairs,
    present: true,
  }
}

fn occurrence_check(truth: &EdgeTruth, edge: &Edge) -> RecallOutcome {
  let Some(expected) = truth.occurrences else { return RecallOutcome::Found };
  let distinct: BTreeSet<(String, u32, u32)> = edge
    .evidence
    .iter()
    .map(|evidence| (evidence.path.to_string(), evidence.span.start_byte, evidence.span.end_byte))
    .collect();
  match distinct.len() == expected {
    true => RecallOutcome::Found,
    false => RecallOutcome::WrongDetail {
      detail: format!("{} distinct evidence spans, expected {expected}", distinct.len()),
    },
  }
}

fn record_optional(ctx: &Ctx<'_>, truth: &EdgeTruth, found: &Lookup<'_>, entry: &mut EdgeKindScore) {
  entry.optional_expected += 1;
  if !found.present {
    return;
  }
  match found.outcome {
    RecallOutcome::Found => entry.optional_present += 1,
    _ => {
      entry.optional_wrong_certainty += 1;
      ctx.finding("edge.optional", &truth.kind, label(truth), found.outcome.describe());
    }
  }
}

fn check_forbidden(ctx: &Ctx<'_>, truth: &ForbiddenTruth, kind: &EdgeKind, tally: &mut Prohibition, calibration: &mut Calibration) {
  let from = ctx.resolve("forbidden.from", &truth.from, None);
  let to = ctx.resolve_target("forbidden.to", &truth.to);
  let (from, to) = match (from, to) {
    (Resolved::Ids(from), Resolved::Ids(to)) => (from, to),
    (Resolved::Foreign | Resolved::Missing(_) | Resolved::Absent, _) | (_, Resolved::Foreign | Resolved::Missing(_) | Resolved::Absent) => {
      tally.vacuous += 1;
      return;
    }
    (Resolved::Unbound, _) | (_, Resolved::Unbound) => {
      tally.unbound += 1;
      return;
    }
  };
  let restricted = crate::quality::truth::Certainties {
    certainty: truth.certainty.clone(),
    min_confidence: None,
  };
  let violations: Vec<&Edge> = from
    .iter()
    .flat_map(|id| ctx.snapshot.edges_from(*id))
    .filter(|edge| &edge.kind == kind && to.contains(&edge.to) && accepts(&restricted, &edge.certainty))
    .collect();
  tally.checked += 1;
  if violations.is_empty() {
    return;
  }
  tally.violations += 1;
  for edge in &violations {
    calibration.record(
      edge.certainty.class().as_str(),
      edge.certainty.confidence().map(f32::from),
      crate::quality::metrics::JudgedEdge::False,
    );
  }
  let described: Vec<String> = violations.iter().map(|edge| describe(ctx, edge)).collect();
  ctx.finding(
    "forbidden_edge",
    &truth.kind,
    format!("{}: {} -> {}", truth.kind, truth.from, truth.to),
    format!("violated by {}", described.join("; ")),
  );
}

fn describe(ctx: &Ctx<'_>, edge: &Edge) -> String {
  format!(
    "{} {} -> {} {}",
    edge.kind,
    ctx.snapshot.describe(edge.from),
    ctx.snapshot.describe(edge.to),
    edge.certainty.tag()
  )
}
