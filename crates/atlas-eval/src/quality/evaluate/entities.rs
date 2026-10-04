use atlas_core::{EdgeKind, EntityId};

use super::EntityScore;
use super::context::{Ctx, Resolved, accepts};
use crate::quality::metrics::RecallOutcome;
use crate::quality::selector::{Binding, BindingProblem};
use crate::quality::truth::EntityTruth;

const SECTION: &str = "entity";

pub fn evaluate(ctx: &Ctx<'_>) -> EntityScore {
  let mut score = EntityScore::default();
  for truth in &ctx.truth.entities {
    let (outcome, handler) = evaluate_one(ctx, truth);
    if let Some(handler) = handler {
      score.handlers.record(&handler);
      if !handler.is_found() {
        ctx.finding("entity.handler", &truth.kind, truth.selector().to_string(), handler.describe());
      }
    }
    if truth.best_effort {
      score.best_effort_expected += 1;
      score.best_effort_found += u64::from(outcome.is_found());
      continue;
    }
    if !outcome.is_found() {
      ctx.finding(SECTION, &truth.kind, truth.selector().to_string(), outcome.describe());
    }
    score.by_kind.entry(truth.kind.clone()).or_default().record(&outcome);
    score.total.record(&outcome);
  }
  score
}

fn evaluate_one(ctx: &Ctx<'_>, truth: &EntityTruth) -> (RecallOutcome, Option<RecallOutcome>) {
  let selector = truth.selector();
  let id = match ctx.binding(&selector, Some(&truth.kind)) {
    Binding::One(id) => id,
    Binding::Many(_) => return (RecallOutcome::Unbound, None),
    Binding::Foreign => {
      ctx.report_binding_error(
        SECTION,
        &selector,
        BindingProblem::NoMatch {
          hint: "the selector names a repo outside this project".to_owned(),
        },
      );
      return (RecallOutcome::Unbound, None);
    }
    Binding::Failed(BindingProblem::NoMatch { hint }) => {
      ctx.mark_missing(&selector);
      return (RecallOutcome::Missing { detail: hint }, None);
    }
    Binding::Failed(problem) => {
      ctx.report_binding_error(SECTION, &selector, problem);
      return (RecallOutcome::Unbound, None);
    }
  };
  let Some(entity) = ctx.snapshot.entity(id) else {
    return (RecallOutcome::Unbound, None);
  };
  let handler = truth.handler.as_ref().map(|handler| handler_outcome(ctx, handler, id));
  let wanted_kind = match truth.kind.as_str() {
    "db_model" => "db_table",
    other => other,
  };
  if entity.kind.as_str() != wanted_kind {
    return (
      RecallOutcome::WrongDetail {
        detail: format!("indexed as `{}`, expected `{}`", entity.kind, truth.kind),
      },
      handler,
    );
  }
  if !accepts(&truth.certainties(), &entity.certainty) {
    return (RecallOutcome::WrongCertainty { actual: entity.certainty.tag() }, handler);
  }
  if let Some(model) = truth.model_selector() {
    return (model_outcome(ctx, model, id), handler);
  }
  (RecallOutcome::Found, handler)
}

fn model_outcome(ctx: &Ctx<'_>, model: &crate::quality::truth::Selector, table: EntityId) -> RecallOutcome {
  match ctx.resolve_in_project(SECTION, model, None) {
    Resolved::Ids(ids) => {
      let mapped = ids.iter().any(|id| ctx.snapshot.edges_between(*id, table, &EdgeKind::MapsToTable).next().is_some());
      match mapped {
        true => RecallOutcome::Found,
        false => RecallOutcome::WrongDetail {
          detail: format!("model {model} has no maps_to_table edge to the table"),
        },
      }
    }
    Resolved::Missing(detail) => RecallOutcome::Missing { detail },
    Resolved::Foreign | Resolved::Unbound | Resolved::Absent => RecallOutcome::Unbound,
  }
}

fn handler_outcome(ctx: &Ctx<'_>, handler: &crate::quality::truth::Selector, endpoint: EntityId) -> RecallOutcome {
  match ctx.resolve_in_project("entity.handler", handler, None) {
    Resolved::Ids(ids) => {
      let exposed = ids.iter().any(|id| ctx.snapshot.edges_between(*id, endpoint, &EdgeKind::ExposesEndpoint).next().is_some());
      match exposed {
        true => RecallOutcome::Found,
        false => RecallOutcome::Missing {
          detail: format!("no exposes_endpoint edge from handler {handler}"),
        },
      }
    }
    Resolved::Missing(detail) => RecallOutcome::Missing { detail },
    Resolved::Foreign | Resolved::Unbound | Resolved::Absent => RecallOutcome::Unbound,
  }
}
