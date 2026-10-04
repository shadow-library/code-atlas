use std::collections::{BTreeSet, HashSet};

use atlas_core::{CertaintyClass, EdgeKind, EntityId, EntityKey};
use atlas_query::{ImpactAnswer, ImpactRequest, SearchRequest, SimilarRequest, SimilaritySignal, Targeted, TestsForRequest, UnusedRequest};

use super::context::{Ctx, Resolved};
use super::{BlastScore, IsolationScore, SimilarFinding, SimilarScore, UnusedScore};
use crate::quality::error::QualityResult;
use crate::quality::metrics::{Recall, RecallOutcome};
use crate::quality::selector::{is_foreign, matches};
use crate::quality::truth::{Certainties, Selector, SimilarTruth, TestOfTruth};

pub struct QueryScores {
  pub tests_of: Recall,
  pub unused: UnusedScore,
  pub similar: SimilarScore,
  pub blast_radius: BlastScore,
  pub isolation: IsolationScore,
}

pub fn evaluate(ctx: &Ctx<'_>) -> QualityResult<QueryScores> {
  Ok(QueryScores {
    tests_of: tests_of(ctx)?,
    unused: unused(ctx)?,
    similar: similar(ctx)?,
    blast_radius: blast_radius(ctx)?,
    isolation: isolation(ctx)?,
  })
}

fn single(ctx: &Ctx<'_>, section: &str, selector: &Selector) -> Result<EntityId, RecallOutcome> {
  match ctx.resolve_in_project(section, selector, None) {
    Resolved::Ids(ids) if ids.len() == 1 => Ok(ids[0]),
    Resolved::Missing(detail) => Err(RecallOutcome::Missing { detail }),
    _ => Err(RecallOutcome::Unbound),
  }
}

fn tests_of(ctx: &Ctx<'_>) -> QualityResult<Recall> {
  let mut recall = Recall::default();
  for truth in &ctx.truth.test_of {
    let outcome = test_of(ctx, truth)?;
    if !outcome.is_found() {
      ctx.finding("test_of", "tests_for", format!("{} tests {}", truth.test, truth.subject), outcome.describe());
    }
    recall.record(&outcome);
  }
  Ok(recall)
}

fn test_of(ctx: &Ctx<'_>, truth: &TestOfTruth) -> QualityResult<RecallOutcome> {
  let subject = match single(ctx, "test_of.subject", &truth.subject) {
    Ok(id) => id,
    Err(outcome) => return Ok(outcome),
  };
  let test = match single(ctx, "test_of.test", &truth.test) {
    Ok(id) => id,
    Err(outcome) => return Ok(outcome),
  };
  let request = TestsForRequest {
    limit: Ctx::limit(),
    ..TestsForRequest::new(ctx.snapshot.describe(subject))
  };
  let Targeted::Found(answer) = ctx.project.query.tests_for(&request)?.answer else {
    return Ok(RecallOutcome::Missing {
      detail: "tests-for could not resolve the subject".to_owned(),
    });
  };
  let wanted = Certainties {
    certainty: truth.certainty.clone(),
    min_confidence: None,
  };
  let wanted_classes = wanted.classes();
  let inside_suite = |candidate: EntityId| -> bool {
    let (Some(entity), Some(suite)) = (ctx.snapshot.entity(candidate), ctx.snapshot.entity(test)) else {
      return false;
    };
    match (&entity.key, &suite.key) {
      (EntityKey::Symbol(case), EntityKey::Symbol(group)) => entity.path == suite.path && case.item_path().starts_with(group.item_path()),
      _ => false,
    }
  };
  let matched: Vec<_> = answer
    .tests
    .items
    .iter()
    .filter(|item| item.test.entity_id().is_ok_and(|id| id == test || inside_suite(id)))
    .collect();
  if matched.is_empty() {
    return Ok(RecallOutcome::Missing {
      detail: format!("tests-for lists {} tests, none is the expected one", answer.tests.total),
    });
  }
  let accepted = matched.iter().any(|item| {
    let class = item.relation.certainty.class.parse::<CertaintyClass>().ok();
    wanted_classes.is_empty() || class.is_some_and(|class| wanted_classes.contains(&class))
  });
  Ok(match accepted {
    true => RecallOutcome::Found,
    false => RecallOutcome::WrongCertainty {
      actual: matched.iter().map(|item| item.relation.certainty.tag.clone()).collect::<Vec<_>>().join(", "),
    },
  })
}

fn unused(ctx: &Ctx<'_>) -> QualityResult<UnusedScore> {
  let mut score = UnusedScore::default();
  let answer = ctx
    .project
    .query
    .unused(&UnusedRequest {
      limit: Ctx::limit(),
      ..UnusedRequest::default()
    })?
    .answer;
  let ids = |candidates: &[atlas_query::UnusedCandidate]| -> HashSet<EntityId> { candidates.iter().filter_map(|candidate| candidate.entity.entity_id().ok()).collect() };
  let likely = ids(&answer.likely_unused.items);
  let possibly = ids(&answer.possibly_unused.items);
  for truth in &ctx.truth.unused {
    let Some(selector) = truth.selector() else { continue };
    let outcome = match single(ctx, "unused", &selector) {
      Err(outcome) => outcome,
      Ok(id) if likely.contains(&id) => {
        score.reported_likely += 1;
        RecallOutcome::Found
      }
      Ok(id) if possibly.contains(&id) => {
        score.reported_possibly += 1;
        RecallOutcome::Found
      }
      Ok(_) => RecallOutcome::Missing {
        detail: "not reported as unused".to_owned(),
      },
    };
    if !outcome.is_found() {
      ctx.finding("unused", "unused", selector.to_string(), outcome.describe());
    }
    score.expected_unused.record(&outcome);
  }
  for truth in &ctx.truth.not_unused {
    let Some(selector) = truth.selector() else { continue };
    let id = match single(ctx, "not_unused", &selector) {
      Ok(id) => id,
      Err(RecallOutcome::Missing { .. }) => {
        score.not_unused.vacuous += 1;
        continue;
      }
      Err(_) => {
        score.not_unused.unbound += 1;
        continue;
      }
    };
    score.not_unused.checked += 1;
    if likely.contains(&id) {
      score.not_unused.violations += 1;
      ctx.finding(
        "not_unused",
        "unused",
        selector.to_string(),
        format!("reported likely unused; ground truth: {}", truth.reason.as_deref().unwrap_or("reachable")),
      );
    } else if possibly.contains(&id) {
      score.not_unused_hedged += 1;
    }
  }
  for truth in &ctx.truth.ambiguous_unused {
    let Some(selector) = truth.selector() else { continue };
    score.ambiguous_total += 1;
    if let Ok(id) = single(ctx, "ambiguous_unused", &selector)
      && (likely.contains(&id) || possibly.contains(&id))
    {
      score.ambiguous_reported += 1;
    }
  }
  Ok(score)
}

fn similar(ctx: &Ctx<'_>) -> QualityResult<SimilarScore> {
  let mut score = SimilarScore::default();
  for truth in &ctx.truth.similar {
    score.pairs.push(similar_pair(ctx, truth)?);
  }
  fold_similar(ctx, &mut score);
  Ok(score)
}

fn similar_pair(ctx: &Ctx<'_>, truth: &SimilarTruth) -> QualityResult<SimilarFinding> {
  let mut finding = SimilarFinding {
    a: truth.a.to_string(),
    b: truth.b.to_string(),
    expected: truth.expected.clone(),
    acceptable: truth.acceptable.clone(),
    surfaced: false,
    structural_jaccard: None,
    system_verdict: None,
    inferred_verdict: None,
    deterministic_claim: false,
    scope: "project".to_owned(),
  };
  if is_foreign(ctx.snapshot, &truth.a) || is_foreign(ctx.snapshot, &truth.b) {
    finding.scope = "cross_project".to_owned();
    return Ok(finding);
  }
  let (Ok(a), Ok(b)) = (single(ctx, "similar.a", &truth.a), single(ctx, "similar.b", &truth.b)) else {
    finding.scope = "unbound".to_owned();
    return Ok(finding);
  };
  finding.deterministic_claim = ctx
    .snapshot
    .edges
    .iter()
    .any(|edge| edge.kind == EdgeKind::EquivalentTo && ((edge.from == a && edge.to == b) || (edge.from == b && edge.to == a)));
  let request = SimilarRequest {
    limit: 1_000,
    ..SimilarRequest::new(ctx.snapshot.describe(a))
  };
  let Targeted::Found(answer) = ctx.project.query.similar(&request)?.answer else {
    return Ok(finding);
  };
  let Some(member) = answer.members.iter().find(|member| member.entity.entity_id().is_ok_and(|id| id == b)) else {
    return Ok(finding);
  };
  finding.surfaced = true;
  finding.system_verdict = Some(member.verdict.clone());
  for signal in &member.signals {
    match signal {
      SimilaritySignal::Structural { jaccard } => finding.structural_jaccard = Some(*jaccard),
      SimilaritySignal::Inferred { verdict, .. } => finding.inferred_verdict = Some(verdict.clone()),
      _ => {}
    }
  }
  Ok(finding)
}

fn fold_similar(ctx: &Ctx<'_>, score: &mut SimilarScore) {
  let pairs = std::mem::take(&mut score.pairs);
  for (truth, pair) in ctx.truth.similar.iter().zip(&pairs) {
    if pair.scope != "project" {
      score.not_evaluable += 1;
      continue;
    }
    if !truth.deterministic_claim_allowed {
      score.claims_checked += 1;
      if pair.deterministic_claim {
        score.claims_violated += 1;
        ctx.finding(
          "similar",
          "deterministic_claim",
          format!("{} ~ {}", pair.a, pair.b),
          "an equivalent_to edge exists without an inferred verdict".to_owned(),
        );
      }
    }
    match &pair.inferred_verdict {
      None => score.verdicts_undecided += 1,
      Some(verdict) => {
        score.verdicts_inferred += 1;
        let mapped = if verdict == "equivalent" { "equivalent" } else { "similar_not_equivalent" };
        let accepted = mapped == truth.expected || truth.acceptable.iter().any(|candidate| candidate == mapped);
        score.verdicts_correct += u64::from(accepted);
        if !accepted {
          ctx.finding(
            "similar",
            "verdict",
            format!("{} ~ {}", pair.a, pair.b),
            format!("inferred {mapped}, expected {}", truth.expected),
          );
        }
      }
    }
  }
  score.pairs = pairs;
}

fn impacted(answer: &ImpactAnswer) -> HashSet<EntityId> {
  [&answer.direct, &answer.transitive, &answer.endpoints, &answer.consumers, &answer.tests, &answer.config]
    .into_iter()
    .flat_map(|group| group.items.iter().filter_map(|item| item.entity.entity_id().ok()))
    .collect()
}

fn blast_radius(ctx: &Ctx<'_>) -> QualityResult<BlastScore> {
  let mut score = BlastScore::default();
  for truth in &ctx.truth.blast_radius {
    let change = single(ctx, "blast_radius.change", &truth.change);
    let reached = match change {
      Ok(id) => {
        let request = ImpactRequest {
          limit: Ctx::limit(),
          ..ImpactRequest::new([ctx.snapshot.describe(id)])
        };
        match ctx.project.query.impact(&request)?.answer {
          Targeted::Found(answer) => Some(impacted(&answer)),
          _ => None,
        }
      }
      Err(_) => None,
    };
    let label = truth.change.to_string();
    for selector in &truth.must_include {
      let outcome = match (&reached, ctx.resolve_in_project("blast_radius.must_include", selector, None)) {
        (None, _) => RecallOutcome::Unbound,
        (Some(_), Resolved::Unbound | Resolved::Foreign | Resolved::Absent) => RecallOutcome::Unbound,
        (Some(_), Resolved::Missing(detail)) => RecallOutcome::Missing { detail },
        (Some(reached), Resolved::Ids(ids)) => match ids.iter().any(|id| reached.contains(id)) {
          true => RecallOutcome::Found,
          false => RecallOutcome::Missing {
            detail: "not in the impact answer".to_owned(),
          },
        },
      };
      if !outcome.is_found() {
        ctx.finding("blast_radius", "must_include", format!("change {label}: {selector}"), outcome.describe());
      }
      score.must_include.record(&outcome);
    }
    for selector in &truth.must_not_include {
      match (&reached, ctx.resolve_target("blast_radius.must_not_include", selector)) {
        (_, Resolved::Foreign | Resolved::Missing(_) | Resolved::Absent) => score.must_not_include.vacuous += 1,
        (None, _) | (_, Resolved::Unbound) => score.must_not_include.unbound += 1,
        (Some(reached), Resolved::Ids(ids)) => {
          score.must_not_include.checked += 1;
          if ids.iter().any(|id| reached.contains(id)) {
            score.must_not_include.violations += 1;
            ctx.finding(
              "blast_radius",
              "must_not_include",
              format!("change {label}: {selector}"),
              "present in the impact answer".to_owned(),
            );
          }
        }
      }
    }
    for selector in &truth.may_include {
      score.may_include_expected += 1;
      if let (Some(reached), Resolved::Ids(ids)) = (&reached, ctx.resolve("blast_radius.may_include", selector, None))
        && ids.iter().any(|id| reached.contains(id))
      {
        score.may_include_present += 1;
      }
    }
  }
  Ok(score)
}

fn search_name(selector: &Selector) -> Option<String> {
  if let Some(symbol) = &selector.symbol {
    return symbol.rsplit([':', '.']).next().map(str::to_owned);
  }
  if let Some(route) = &selector.route {
    return Some(route.clone());
  }
  selector.name.clone().or_else(|| selector.scheme.clone())
}

fn isolation(ctx: &Ctx<'_>) -> QualityResult<IsolationScore> {
  let mut score = IsolationScore::default();
  for truth in &ctx.truth.isolation {
    let Some(target) = ctx.corpus.project(&truth.project) else {
      score.not_evaluated += truth.must_not_return.len() as u64;
      continue;
    };
    let mut searched: BTreeSet<String> = BTreeSet::new();
    let mut leaked_names: BTreeSet<String> = BTreeSet::new();
    for selector in &truth.must_not_return {
      let Some(name) = search_name(selector) else { continue };
      if !searched.insert(name.clone()) {
        continue;
      }
      let answer = target.query.search(&SearchRequest::new(name.clone()))?.answer;
      let foreign = answer
        .exact_name
        .iter()
        .chain(&answer.strong)
        .chain(&answer.related)
        .map(|hit| hit.entity.repo())
        .any(|repo| !repo.is_empty() && !target.snapshot.repos.contains(repo));
      if foreign {
        leaked_names.insert(name);
      }
    }
    for selector in &truth.must_not_return {
      score.checked.checked += 1;
      let stored = target.snapshot.entities.values().any(|entity| {
        matches(entity, selector)
          || selector
            .repo
            .as_ref()
            .is_some_and(|repo| entity.repo.as_ref().is_some_and(|candidate| candidate.as_str() == repo) && !target.snapshot.repos.contains(repo))
      });
      let searched_leak = search_name(selector).is_some_and(|name| leaked_names.contains(&name));
      if stored || searched_leak {
        score.checked.violations += 1;
        ctx.finding(
          "isolation",
          "leak",
          format!("project {} must not return {selector}", truth.project),
          "returned by the project's store or search".to_owned(),
        );
      }
    }
  }
  Ok(score)
}
