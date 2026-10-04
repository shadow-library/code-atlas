use std::collections::BTreeSet;

use atlas_core::{EdgeKind, EntityId, EntityKey, HttpMethod, ParseStatus, RelPath, RepoName};
use atlas_query::UsageState;

use super::context::{Ctx, Resolved, accepts};
use super::{AuthScore, SetScore};
use crate::quality::error::QualityResult;
use crate::quality::metrics::{Recall, RecallOutcome};
use crate::quality::selector::client_target_matches;
use crate::quality::truth::{AuthTruth, Certainties, EndpointAuthTruth, FileFlagTruth, Selector, UnmatchedCallTruth};

pub struct DomainScores {
  pub unmatched_calls: Recall,
  pub auth: AuthScore,
  pub endpoint_auth: Recall,
  pub file_flags: Recall,
}

pub fn evaluate(ctx: &Ctx<'_>) -> QualityResult<DomainScores> {
  let mut scores = DomainScores {
    unmatched_calls: Recall::default(),
    auth: AuthScore::default(),
    endpoint_auth: Recall::default(),
    file_flags: Recall::default(),
  };
  for truth in &ctx.truth.unmatched_call {
    let outcome = unmatched_call(ctx, truth);
    if !outcome.is_found() {
      ctx.finding(
        "unmatched_call",
        "api_client_call",
        format!("{} {} from {}", truth.method, truth.route, truth.from),
        outcome.describe(),
      );
    }
    scores.unmatched_calls.record(&outcome);
  }
  for truth in &ctx.truth.auth {
    auth(ctx, truth, &mut scores.auth)?;
  }
  for truth in &ctx.truth.endpoint_auth {
    let outcome = endpoint_auth(ctx, truth);
    if !outcome.is_found() {
      ctx.finding("endpoint_auth", "api_endpoint", format!("{} {}", truth.method, truth.route), outcome.describe());
    }
    scores.endpoint_auth.record(&outcome);
  }
  for truth in &ctx.truth.file_flag {
    let outcome = file_flag(ctx, truth);
    if !outcome.is_found() {
      ctx.finding("file_flag", &truth.flag, format!("{}:{}", truth.repo, truth.path), outcome.describe());
    }
    scores.file_flags.record(&outcome);
  }
  Ok(scores)
}

fn unmatched_call(ctx: &Ctx<'_>, truth: &UnmatchedCallTruth) -> RecallOutcome {
  let callers = match ctx.resolve_in_project("unmatched_call", &truth.from, None) {
    Resolved::Ids(ids) => ids,
    Resolved::Missing(detail) => return RecallOutcome::Missing { detail },
    Resolved::Unbound | Resolved::Foreign | Resolved::Absent => return RecallOutcome::Unbound,
  };
  let Some(method) = HttpMethod::parse_lenient(&truth.method) else {
    return RecallOutcome::Unbound;
  };
  for caller in callers {
    for request in ctx.snapshot.edges_from(caller).filter(|edge| edge.kind == EdgeKind::MakesRequest) {
      let Some(call) = ctx.snapshot.entity(request.to) else { continue };
      let EntityKey::ClientCall { method: call_method, target, .. } = &call.key else {
        continue;
      };
      if *call_method != method || !client_target_matches(target, &truth.route) {
        continue;
      }
      let linked: Vec<String> = ctx
        .snapshot
        .edges_from(caller)
        .filter(|edge| edge.kind == EdgeKind::CallsEndpoint && edge.attrs.get("client_call").and_then(serde_json::Value::as_str) == Some(call.key.to_string().as_str()))
        .map(|edge| ctx.snapshot.describe(edge.to))
        .collect();
      if !linked.is_empty() {
        return RecallOutcome::WrongDetail {
          detail: format!("linked to {}", linked.join(", ")),
        };
      }
      return match call.attrs.get("unmatched").and_then(serde_json::Value::as_bool) {
        Some(true) => RecallOutcome::Found,
        _ => RecallOutcome::WrongDetail {
          detail: "the client call is not flagged unmatched".to_owned(),
        },
      };
    }
  }
  RecallOutcome::Missing {
    detail: "no api_client_call entity with that method and route is recorded for the caller".to_owned(),
  }
}

fn set_score(expected: &BTreeSet<String>, predicted: &BTreeSet<String>) -> SetScore {
  SetScore {
    expected: expected.len() as u64,
    predicted: predicted.len() as u64,
    correct: expected.intersection(predicted).count() as u64,
  }
}

fn auth(ctx: &Ctx<'_>, truth: &AuthTruth, score: &mut AuthScore) -> QualityResult<()> {
  let answer = ctx.api()?;
  let matrix = answer.auth_matrix.iter().find(|matrix| matrix.repo == truth.api_repo);
  let schemes = matrix.map(|matrix| matrix.schemes.as_slice()).unwrap_or_default();
  let supported: BTreeSet<String> = schemes
    .iter()
    .filter(|usage| usage.supported.as_ref().is_some_and(|support| support.endpoints > 0))
    .map(|usage| usage.scheme.short.clone())
    .collect();
  let used: BTreeSet<String> = schemes
    .iter()
    .filter(|usage| usage.used == UsageState::Used)
    .flat_map(|usage| usage.used_by_repos.iter().map(|repo| format!("{}@{}", usage.scheme.short, repo)))
    .collect();
  let unused: BTreeSet<String> = schemes
    .iter()
    .filter(|usage| usage.used == UsageState::Unused)
    .map(|usage| usage.scheme.short.clone())
    .collect();
  let expected_supported: BTreeSet<String> = truth.supported.iter().cloned().collect();
  let expected_used: BTreeSet<String> = truth
    .used
    .iter()
    .flat_map(|(scheme, repos)| repos.iter().map(move |repo| format!("{scheme}@{repo}")))
    .collect();
  let expected_unused: BTreeSet<String> = truth.unused.iter().cloned().collect();
  for (section, expected, predicted) in [
    ("supported", &expected_supported, &supported),
    ("used", &expected_used, &used),
    ("unused", &expected_unused, &unused),
  ] {
    for missing in expected.difference(predicted) {
      ctx.finding("auth", section, format!("{} {missing}", truth.api_repo), "expected but not reported".to_owned());
    }
    for extra in predicted.difference(expected) {
      ctx.finding("auth", section, format!("{} {extra}", truth.api_repo), "reported but not in the ground truth".to_owned());
    }
  }
  score.supported.absorb(&set_score(&expected_supported, &supported));
  score.used.absorb(&set_score(&expected_used, &used));
  score.unused.absorb(&set_score(&expected_unused, &unused));
  Ok(())
}

fn endpoint_auth(ctx: &Ctx<'_>, truth: &EndpointAuthTruth) -> RecallOutcome {
  let selector = Selector {
    kind: Some("api_endpoint".to_owned()),
    method: Some(truth.method.clone()),
    route: Some(truth.route.clone()),
    ..Selector::default()
  };
  let endpoint = match ctx.resolve_in_project("endpoint_auth", &selector, None) {
    Resolved::Ids(ids) if ids.len() == 1 => ids[0],
    Resolved::Missing(detail) => return RecallOutcome::Missing { detail },
    _ => return RecallOutcome::Unbound,
  };
  let supports: Vec<_> = ctx.snapshot.edges_from(endpoint).filter(|edge| edge.kind == EdgeKind::SupportsAuth).collect();
  if supports.is_empty() {
    return RecallOutcome::Missing {
      detail: "no supports_auth edge from the endpoint".to_owned(),
    };
  }
  let actual: BTreeSet<String> = supports
    .iter()
    .filter_map(|edge| ctx.snapshot.entity(edge.to))
    .map(|scheme| scheme.key.display_name())
    .collect();
  let expected: BTreeSet<String> = truth.schemes.iter().cloned().collect();
  if actual != expected {
    return RecallOutcome::WrongDetail {
      detail: format!("schemes {actual:?}, expected {expected:?}"),
    };
  }
  if let Some(permission) = &truth.permission {
    let granted: BTreeSet<String> = permissions_of(ctx, endpoint);
    if !granted.contains(permission) {
      return RecallOutcome::WrongDetail {
        detail: format!("permissions {granted:?}, expected {permission}"),
      };
    }
  }
  if truth.schemes.len() > 1 {
    let combinator = ctx
      .snapshot
      .entity(endpoint)
      .and_then(|entity| entity.attrs.get("auth_combinator"))
      .and_then(serde_json::Value::as_str)
      .unwrap_or("none");
    if combinator != truth.mode {
      return RecallOutcome::WrongDetail {
        detail: format!("combinator `{combinator}`, expected `{}`", truth.mode),
      };
    }
  }
  let wanted = Certainties {
    certainty: truth.certainty.clone(),
    min_confidence: None,
  };
  match supports.iter().find(|edge| !accepts(&wanted, &edge.certainty)) {
    Some(edge) => RecallOutcome::WrongCertainty { actual: edge.certainty.tag() },
    None => RecallOutcome::Found,
  }
}

fn permissions_of(ctx: &Ctx<'_>, endpoint: EntityId) -> BTreeSet<String> {
  ctx
    .snapshot
    .edges_to(endpoint)
    .filter(|edge| edge.kind == EdgeKind::ExposesEndpoint)
    .flat_map(|exposed| ctx.snapshot.edges_from(exposed.from).filter(|edge| edge.kind == EdgeKind::RequiresPermission))
    .filter_map(|edge| ctx.snapshot.entity(edge.to))
    .map(|permission| permission.key.display_name())
    .collect()
}

fn file_flag(ctx: &Ctx<'_>, truth: &FileFlagTruth) -> RecallOutcome {
  let (Ok(repo), Ok(path)) = (RepoName::new(&truth.repo), RelPath::new(&truth.path)) else {
    return RecallOutcome::Unbound;
  };
  let Some(record) = ctx.snapshot.files.get(&(repo.clone(), path.clone())) else {
    return RecallOutcome::Missing {
      detail: "the file is not indexed".to_owned(),
    };
  };
  let present = match truth.flag.as_str() {
    "partial_syntax" => matches!(record.parse_status, ParseStatus::Partial { .. }),
    _ => ctx
      .snapshot
      .entity(EntityId::of(&EntityKey::File { repo, path }))
      .and_then(|entity| entity.attrs.get("generated"))
      .and_then(serde_json::Value::as_bool)
      .unwrap_or(false),
  };
  match present {
    true => RecallOutcome::Found,
    false => RecallOutcome::Missing {
      detail: format!("file is not flagged {}", truth.flag),
    },
  }
}
