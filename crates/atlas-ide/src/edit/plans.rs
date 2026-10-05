use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atlas_core::ContentHash;
use atlas_edit::EditPlan;
use serde::{Deserialize, Serialize};

use super::extras::VerifySite;
use super::{ChangedFile, OperationView};
use crate::error::{IdeError, IdeResult};
use crate::service::{IdeService, guard};

pub const PLAN_CAPACITY: usize = 32;
pub const PLAN_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanGuard {
  Files,
  Project { run: Option<i64> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdePlan {
  pub id: String,
  pub project: String,
  pub operation: OperationView,
  pub guard: PlanGuard,
  pub edits: EditPlan,
  pub files: Vec<ChangedFile>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub resources: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub not_renamed: Vec<VerifySite>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub applied_from_graph: Vec<VerifySite>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub caveats: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub unverified: Vec<VerifySite>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub unloaded_projects: Vec<String>,
  #[serde(default)]
  pub allow_unverified: bool,
}

pub(crate) struct PlanDraft {
  pub project: String,
  pub operation: OperationView,
  pub guard: PlanGuard,
  pub edits: EditPlan,
  pub files: Vec<ChangedFile>,
  pub resources: Vec<String>,
  pub not_renamed: Vec<VerifySite>,
  pub applied_from_graph: Vec<VerifySite>,
  pub caveats: Vec<String>,
  pub unverified: Vec<VerifySite>,
  pub unloaded_projects: Vec<String>,
  pub allow_unverified: bool,
}

impl PlanDraft {
  pub(crate) fn seal(self) -> IdePlan {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let fingerprint = serde_json::to_string(&self.edits).unwrap_or_default();
    let hash = ContentHash::of(format!("{}\n{}\n{nanos}\n{fingerprint}", self.project, self.operation).as_bytes()).to_string();
    IdePlan {
      id: format!("p-{}", &hash[..12]),
      project: self.project,
      operation: self.operation,
      guard: self.guard,
      edits: self.edits,
      files: self.files,
      resources: self.resources,
      not_renamed: self.not_renamed,
      applied_from_graph: self.applied_from_graph,
      caveats: self.caveats,
      unverified: self.unverified,
      unloaded_projects: self.unloaded_projects,
      allow_unverified: self.allow_unverified,
    }
  }
}

struct Cached {
  plan: IdePlan,
  made: Instant,
}

pub(crate) struct PlanCache {
  ttl: Duration,
  plans: VecDeque<Cached>,
}

enum Lookup {
  Found(Box<IdePlan>),
  Expired(Duration),
  Unknown,
}

impl PlanCache {
  pub(crate) fn new(ttl: Duration) -> Self {
    Self { ttl, plans: VecDeque::new() }
  }

  pub(crate) fn ttl(&self) -> Duration {
    self.ttl
  }

  pub(crate) fn insert(&mut self, plan: IdePlan) {
    self.plans.retain(|cached| cached.made.elapsed() < self.ttl * 4);
    while self.plans.len() >= PLAN_CAPACITY {
      self.plans.pop_front();
    }
    self.plans.push_back(Cached { plan, made: Instant::now() });
  }

  fn peek(&self, id: &str) -> Option<&IdePlan> {
    self
      .plans
      .iter()
      .find(|cached| cached.plan.id == id && cached.made.elapsed() < self.ttl)
      .map(|cached| &cached.plan)
  }

  fn take(&mut self, id: &str) -> Lookup {
    let Some(index) = self.plans.iter().position(|cached| cached.plan.id == id) else {
      return Lookup::Unknown;
    };
    let Some(cached) = self.plans.remove(index) else { return Lookup::Unknown };
    let age = cached.made.elapsed();
    match age < self.ttl {
      true => Lookup::Found(Box::new(cached.plan)),
      false => Lookup::Expired(age),
    }
  }
}

impl IdeService {
  pub fn planned(&self, id: &str) -> Option<IdePlan> {
    guard(&self.plans).peek(id).cloned()
  }

  pub(crate) fn plan_ttl(&self) -> Duration {
    guard(&self.plans).ttl()
  }

  pub(crate) fn take_plan(&self, id: &str) -> IdeResult<IdePlan> {
    let ttl = self.plan_ttl();
    match guard(&self.plans).take(id) {
      Lookup::Found(plan) => Ok(*plan),
      Lookup::Expired(age) => Err(IdeError::PlanExpired {
        id: id.to_owned(),
        age_secs: age.as_secs(),
        ttl_secs: ttl.as_secs(),
      }),
      Lookup::Unknown => Err(IdeError::UnknownPlan {
        id: id.to_owned(),
        capacity: PLAN_CAPACITY,
      }),
    }
  }
}
