use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};

use atlas_core::{Certainty, CertaintyClass, EntityId};
use atlas_query::{ApiAnswer, ApiRequest};

use super::Finding;
use crate::quality::corpus::{Corpus, IndexedProject};
use crate::quality::error::QualityResult;
use crate::quality::metrics::JudgedEdge;
use crate::quality::selector::{Binding, BindingError, BindingProblem, bind};
use crate::quality::snapshot::Snapshot;
use crate::quality::truth::{Certainties, Selector, Truth};

const LARGE_LIMIT: usize = 1_000_000;

pub enum Resolved {
  Ids(Vec<EntityId>),
  Foreign,
  Absent,
  Missing(String),
  Unbound,
}

pub struct Ctx<'a> {
  pub fixture: &'a str,
  pub truth: &'a Truth,
  pub project: &'a IndexedProject,
  pub corpus: &'a Corpus,
  pub snapshot: &'a Snapshot,
  missing: RefCell<HashSet<Selector>>,
  cache: RefCell<HashMap<(Selector, Option<String>), Binding>>,
  errors: RefCell<Vec<BindingError>>,
  reported: RefCell<HashSet<(String, Selector)>>,
  findings: RefCell<Vec<Finding>>,
  api: OnceCell<ApiAnswer>,
}

impl<'a> Ctx<'a> {
  pub fn new(fixture: &'a str, truth: &'a Truth, project: &'a IndexedProject, corpus: &'a Corpus) -> Self {
    Self {
      fixture,
      truth,
      project,
      corpus,
      snapshot: &project.snapshot,
      missing: RefCell::default(),
      cache: RefCell::default(),
      errors: RefCell::default(),
      reported: RefCell::default(),
      findings: RefCell::default(),
      api: OnceCell::new(),
    }
  }

  pub fn limit() -> usize {
    LARGE_LIMIT
  }

  pub fn mark_missing(&self, selector: &Selector) {
    self.missing.borrow_mut().insert(selector.clone());
  }

  pub fn binding(&self, selector: &Selector, kind: Option<&str>) -> Binding {
    let key = (selector.clone(), kind.map(str::to_owned));
    if let Some(found) = self.cache.borrow().get(&key) {
      return found.clone();
    }
    let binding = bind(self.snapshot, selector, kind);
    self.cache.borrow_mut().insert(key, binding.clone());
    binding
  }

  pub fn report_binding_error(&self, section: &str, selector: &Selector, problem: BindingProblem) {
    if !self.reported.borrow_mut().insert((section.to_owned(), selector.clone())) {
      return;
    }
    self.errors.borrow_mut().push(BindingError {
      fixture: self.fixture.to_owned(),
      section: section.to_owned(),
      selector: selector.to_string(),
      problem,
    });
  }

  pub fn resolve(&self, section: &str, selector: &Selector, kind: Option<&str>) -> Resolved {
    self.resolve_with(section, selector, kind, false)
  }

  pub fn resolve_target(&self, section: &str, selector: &Selector) -> Resolved {
    let may_be_absent = selector.path.is_none() && selector.kind.is_some();
    self.resolve_with(section, selector, None, may_be_absent)
  }

  fn resolve_with(&self, section: &str, selector: &Selector, kind: Option<&str>, absence_is_valid: bool) -> Resolved {
    match self.binding(selector, kind) {
      Binding::One(id) => Resolved::Ids(vec![id]),
      Binding::Many(ids) => Resolved::Ids(ids),
      Binding::Foreign => Resolved::Foreign,
      Binding::Failed(BindingProblem::NoMatch { hint }) => {
        if self.missing.borrow().contains(selector) {
          return Resolved::Missing(format!("entity {selector} is missing from the index"));
        }
        if absence_is_valid {
          return Resolved::Absent;
        }
        self.report_binding_error(section, selector, BindingProblem::NoMatch { hint });
        Resolved::Unbound
      }
      Binding::Failed(problem) => {
        self.report_binding_error(section, selector, problem);
        Resolved::Unbound
      }
    }
  }

  pub fn resolve_in_project(&self, section: &str, selector: &Selector, kind: Option<&str>) -> Resolved {
    match self.resolve(section, selector, kind) {
      Resolved::Foreign => {
        self.report_binding_error(
          section,
          selector,
          BindingProblem::NoMatch {
            hint: "the selector names a repo outside this project".to_owned(),
          },
        );
        Resolved::Unbound
      }
      other => other,
    }
  }

  pub fn finding(&self, section: &str, kind: &str, label: String, outcome: String) {
    self.findings.borrow_mut().push(Finding {
      section: section.to_owned(),
      kind: kind.to_owned(),
      label,
      outcome,
    });
  }

  pub fn finish(self) -> (Vec<BindingError>, Vec<Finding>) {
    (self.errors.into_inner(), self.findings.into_inner())
  }

  pub fn api(&self) -> QualityResult<&ApiAnswer> {
    if let Some(answer) = self.api.get() {
      return Ok(answer);
    }
    let answer = self
      .project
      .query
      .api(&ApiRequest {
        limit: LARGE_LIMIT,
        ..ApiRequest::default()
      })?
      .answer;
    Ok(self.api.get_or_init(|| answer))
  }
}

pub fn tier(class: CertaintyClass) -> u8 {
  match class {
    CertaintyClass::CompilerVerified => 5,
    CertaintyClass::Syntactic | CertaintyClass::Declared => 4,
    CertaintyClass::Resolved => 3,
    CertaintyClass::Heuristic => 2,
    CertaintyClass::Inferred => 1,
  }
}

pub fn accepts(wanted: &Certainties, certainty: &Certainty) -> bool {
  let classes = wanted.classes();
  if classes.is_empty() {
    return true;
  }
  if !classes.contains(&certainty.class()) {
    return false;
  }
  match (certainty.confidence(), wanted.min_confidence) {
    (Some(actual), Some(minimum)) => actual.value() + 1e-6 >= minimum,
    _ => true,
  }
}

pub fn judge(wanted: &Certainties, certainty: &Certainty) -> JudgedEdge {
  if accepts(wanted, certainty) {
    return JudgedEdge::Correct;
  }
  let classes = wanted.classes();
  let strongest = classes.iter().map(|class| tier(*class)).max().unwrap_or(0);
  match tier(certainty.class()) > strongest {
    true => JudgedEdge::Overclaimed,
    false => JudgedEdge::Underclaimed,
  }
}
