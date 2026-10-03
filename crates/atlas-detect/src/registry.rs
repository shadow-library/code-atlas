use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use atlas_core::{DetectInput, DetectedEdge, Detection, Detector, Diagnostic, NodeRef, Producer};
use serde::{Deserialize, Serialize};

use crate::detectors::{
  AuthSchemeDetector, AxumEndpointDetector, ConfigEnvDetector, DbDetector, EventDetector, ExpressEndpointDetector, HttpClientDetector, JobDetector, NestEndpointDetector,
  TestDetector,
};

pub trait DiagnosingDetector: Detector {
  fn detect_with_diagnostics(&self, input: &DetectInput<'_>) -> (Detection, Vec<Diagnostic>);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorDiagnostic {
  pub detector: String,
  pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorRun {
  pub detector: &'static str,
  pub producer: Producer,
  pub detection: Detection,
  pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorStats {
  pub detector: String,
  pub files_seen: u64,
  pub files_applied: u64,
  pub entities: u64,
  pub edges: u64,
  pub diagnostics: u64,
  pub elapsed: Duration,
}

#[derive(Debug, Default)]
struct Counters {
  files_seen: AtomicU64,
  files_applied: AtomicU64,
  entities: AtomicU64,
  edges: AtomicU64,
  diagnostics: AtomicU64,
  nanos: AtomicU64,
}

struct Registered {
  detector: Box<dyn DiagnosingDetector>,
  counters: Counters,
}

pub struct DetectorRegistry {
  detectors: Vec<Registered>,
}

impl Default for DetectorRegistry {
  fn default() -> Self {
    Self::new(vec![
      Box::new(NestEndpointDetector),
      Box::new(ExpressEndpointDetector),
      Box::new(AxumEndpointDetector),
      Box::new(HttpClientDetector),
      Box::new(AuthSchemeDetector),
      Box::new(ConfigEnvDetector),
      Box::new(TestDetector),
      Box::new(EventDetector),
      Box::new(JobDetector),
      Box::new(DbDetector),
    ])
  }
}

impl DetectorRegistry {
  pub fn new(detectors: Vec<Box<dyn DiagnosingDetector>>) -> Self {
    Self {
      detectors: detectors
        .into_iter()
        .map(|detector| Registered {
          detector,
          counters: Counters::default(),
        })
        .collect(),
    }
  }

  pub fn ids(&self) -> Vec<&'static str> {
    self.detectors.iter().map(|registered| registered.detector.id()).collect()
  }

  pub fn producers(&self) -> Vec<Producer> {
    self.detectors.iter().map(|registered| registered.detector.producer()).collect()
  }

  pub fn run_each(&self, input: &DetectInput<'_>) -> Vec<DetectorRun> {
    self
      .detectors
      .iter()
      .filter_map(|registered| {
        let counters = &registered.counters;
        counters.files_seen.fetch_add(1, Ordering::Relaxed);
        let detector = registered.detector.as_ref();
        if !detector.applies_to(input.facts) {
          return None;
        }
        counters.files_applied.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let (mut detection, diagnostics) = detector.detect_with_diagnostics(input);
        counters.nanos.fetch_add(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
        canonicalize(&mut detection);
        counters.entities.fetch_add(detection.entities.len() as u64, Ordering::Relaxed);
        counters.edges.fetch_add(detection.edges.len() as u64, Ordering::Relaxed);
        counters.diagnostics.fetch_add(diagnostics.len() as u64, Ordering::Relaxed);
        tracing::trace!(detector = detector.id(), file = %input.facts.file.path, entities = detection.entities.len(), edges = detection.edges.len(), "detector finished");
        Some(DetectorRun {
          detector: detector.id(),
          producer: detector.producer(),
          detection,
          diagnostics,
        })
      })
      .collect()
  }

  pub fn run_all(&self, input: &DetectInput<'_>) -> (Detection, Vec<DetectorDiagnostic>) {
    let mut detection = Detection::default();
    let mut diagnostics = Vec::new();
    for run in self.run_each(input) {
      detection.merge(run.detection);
      diagnostics.extend(run.diagnostics.into_iter().map(|diagnostic| DetectorDiagnostic {
        detector: run.detector.to_owned(),
        diagnostic,
      }));
    }
    canonicalize(&mut detection);
    (detection, diagnostics)
  }

  pub fn stats(&self) -> Vec<DetectorStats> {
    self
      .detectors
      .iter()
      .map(|registered| {
        let counters = &registered.counters;
        DetectorStats {
          detector: registered.detector.id().to_owned(),
          files_seen: counters.files_seen.load(Ordering::Relaxed),
          files_applied: counters.files_applied.load(Ordering::Relaxed),
          entities: counters.entities.load(Ordering::Relaxed),
          edges: counters.edges.load(Ordering::Relaxed),
          diagnostics: counters.diagnostics.load(Ordering::Relaxed),
          elapsed: Duration::from_nanos(counters.nanos.load(Ordering::Relaxed)),
        }
      })
      .collect()
  }
}

fn canonicalize(detection: &mut Detection) {
  detection
    .entities
    .sort_by(|left, right| left.key.cmp(&right.key).then_with(|| right.certainty.cmp_strength(&left.certainty)));
  detection.entities.dedup_by(|later, earlier| later.key == earlier.key);
  detection.edges.sort_by_cached_key(edge_sort_key);
  detection.edges.dedup();
}

fn edge_sort_key(edge: &DetectedEdge) -> (String, String, String, u32, u32, String) {
  (
    edge.kind.as_str().to_owned(),
    node_sort_key(&edge.from),
    node_sort_key(&edge.to),
    edge.span.start_byte,
    edge.span.end_byte,
    serde_json::to_string(&edge.attrs).unwrap_or_default(),
  )
}

fn node_sort_key(node: &NodeRef) -> String {
  match node {
    NodeRef::Local(local) => format!("0:{:010}", local.0),
    NodeRef::Key(key) => format!("1:{key}"),
    NodeRef::Unresolved { path, from } => format!("2:{}:{}", path.join("::"), from.map_or(-1, |local| i64::from(local.0))),
  }
}
