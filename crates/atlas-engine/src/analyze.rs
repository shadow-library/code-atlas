use std::any::Any;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;

use atlas_core::{
  AnalysisInput, ContentHash, DetectInput, Detection, Detector, Diagnostic, FileFacts, FileRef, Language, LanguageAnalyzer, LanguageFamily, PackageContext, ParseBudget,
  ParseStatus, Producer,
};
use atlas_detect::{DeclaredClientsDetector, DeclaredRoutesDetector, DetectorRegistry, DiagnosingDetector};
use atlas_lang_rust::RustAnalyzer;
use atlas_lang_ts::TsAnalyzer;
use atlas_project::Project;

use crate::artifact::{DetectorOutput, FileArtifact, FileKey};
use crate::options::FaultInjection;

const GENERATED_HEADER_LINES: usize = 5;
const UNSUPPORTED_ANALYZER: &str = "unsupported";

pub(crate) struct Toolchain {
  rust: RustAnalyzer,
  ts: TsAnalyzer,
  budget: ParseBudget,
  detectors: DetectorRegistry,
  declared: DetectorRegistry,
  extra: DetectorRegistry,
  detector_producers: Vec<Producer>,
}

pub(crate) struct AnalysisJob {
  pub key: FileKey,
  pub absolute: PathBuf,
  pub expected_hash: ContentHash,
  pub package: Option<PackageContext>,
}

pub(crate) enum AnalysisOutcome {
  Analyzed(Box<FileArtifact>),
  Vanished,
}

impl Toolchain {
  pub fn new(project: &Project, extra: &[Arc<dyn DiagnosingDetector>], budget: ParseBudget) -> Self {
    let detectors = DetectorRegistry::default();
    let families = &project.settings.detect.families;
    let mut declared: Vec<Box<dyn DiagnosingDetector>> = Vec::new();
    if families.iter().any(|family| family.has_routes()) {
      declared.push(Box::new(DeclaredRoutesDetector::new(
        families.iter().filter(|family| family.has_routes()).cloned().collect(),
      )));
    }
    let clients = DeclaredClientsDetector::new(families);
    if !clients.is_empty() {
      declared.push(Box::new(clients));
    }
    let declared = DetectorRegistry::new(declared);
    let extra = DetectorRegistry::new(
      extra
        .iter()
        .map(|detector| Box::new(SharedDetector(Arc::clone(detector))) as Box<dyn DiagnosingDetector>)
        .collect(),
    );
    let detector_producers = detectors.producers().into_iter().chain(declared.producers()).chain(extra.producers()).collect();
    Self {
      rust: RustAnalyzer::new().with_parse_budget(budget),
      ts: TsAnalyzer::new().with_parse_budget(budget),
      budget,
      detectors,
      declared,
      extra,
      detector_producers,
    }
  }

  pub(crate) fn analyzer(&self, language: Language) -> Option<&dyn LanguageAnalyzer> {
    match language.family() {
      LanguageFamily::Rust => Some(&self.rust),
      LanguageFamily::Ts => Some(&self.ts),
      _ => None,
    }
  }

  pub fn producer(&self, language: Language) -> Producer {
    self
      .analyzer(language)
      .map_or_else(|| Producer::new(UNSUPPORTED_ANALYZER, 0), |analyzer| analyzer.producer())
  }

  pub fn is_current(&self, artifact: &FileArtifact, package: Option<&PackageContext>) -> bool {
    artifact.facts.analyzer == self.producer(artifact.facts.file.language) && artifact.detectors == self.detector_producers && artifact.package.as_ref() == package
  }

  pub fn budget(&self) -> ParseBudget {
    self.budget
  }

  pub fn analyze_with_budget(&self, job: &AnalysisJob, faults: &FaultInjection, budget: ParseBudget) -> AnalysisOutcome {
    let (repo, path) = &job.key;
    let bytes = match fs::read(&job.absolute) {
      Ok(bytes) => bytes,
      Err(error) if error.kind() == io::ErrorKind::NotFound => return AnalysisOutcome::Vanished,
      Err(error) => return self.failed_artifact(job, job.expected_hash, 0, format!("unreadable: {error}")),
    };
    let hash = ContentHash::of(&bytes);
    let size = bytes.len() as u64;
    let Ok(source) = String::from_utf8(bytes) else {
      return self.failed_artifact(job, hash, size, "source is not valid UTF-8".to_owned());
    };
    let file = match FileRef::detect(repo.clone(), path.clone(), hash) {
      Ok(file) => file,
      Err(error) => return self.failed_artifact(job, hash, size, error.to_string()),
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
      if faults.should_panic(repo, path) {
        panic!("injected analyzer fault");
      }
      self.analyze_source(&file, &source, job.package.as_ref(), budget)
    }));
    let (facts, detections) = match outcome {
      Ok(Ok(analyzed)) => analyzed,
      Ok(Err(reason)) => (FileFacts::failed(file.clone(), self.producer(file.language), reason), Vec::new()),
      Err(payload) => {
        let reason = format!("analyzer panicked: {}", panic_message(payload.as_ref()));
        tracing::error!(repo = %repo, path = %path, reason = %reason, "analysis failed");
        (FileFacts::failed(file.clone(), self.producer(file.language), reason), Vec::new())
      }
    };
    AnalysisOutcome::Analyzed(Box::new(FileArtifact {
      facts,
      detections,
      detectors: self.detector_producers.clone(),
      package: job.package.clone(),
      generated: is_generated(path.file_name(), &source),
      size,
    }))
  }

  fn analyze_source(&self, file: &FileRef, source: &str, package: Option<&PackageContext>, budget: ParseBudget) -> Result<(FileFacts, Vec<DetectorOutput>), String> {
    let input = AnalysisInput { file, source };
    let analyzed = match file.language.family() {
      LanguageFamily::Rust => self.rust.clone().with_parse_budget(budget).analyze(&input),
      LanguageFamily::Ts => self.ts.with_parse_budget(budget).analyze(&input),
      _ => return Err(format!("no analyzer supports {}", file.language)),
    };
    let facts = analyzed.map_err(|error| error.to_string())?;
    facts.check_invariants(source.len()).map_err(|error| error.to_string())?;
    if matches!(facts.parse_status, ParseStatus::Failed { .. }) {
      return Ok((facts, Vec::new()));
    }
    let detect = DetectInput { facts: &facts, source, package };
    let detections = self
      .detectors
      .run_each(&detect)
      .into_iter()
      .chain(self.declared.run_each(&detect))
      .chain(self.extra.run_each(&detect))
      .map(|run| DetectorOutput {
        detector: run.detector.to_owned(),
        producer: run.producer,
        detection: run.detection,
        diagnostics: run.diagnostics,
      })
      .collect();
    Ok((facts, detections))
  }

  fn failed_artifact(&self, job: &AnalysisJob, hash: ContentHash, size: u64, reason: String) -> AnalysisOutcome {
    let (repo, path) = &job.key;
    tracing::warn!(repo = %repo, path = %path, reason = %reason, "file could not be analyzed");
    let Ok(file) = FileRef::detect(repo.clone(), path.clone(), hash) else {
      return AnalysisOutcome::Vanished;
    };
    let producer = self.producer(file.language);
    AnalysisOutcome::Analyzed(Box::new(FileArtifact {
      facts: FileFacts::failed(file, producer, reason),
      detections: Vec::new(),
      detectors: self.detector_producers.clone(),
      package: job.package.clone(),
      generated: false,
      size,
    }))
  }
}

struct SharedDetector(Arc<dyn DiagnosingDetector>);

impl Detector for SharedDetector {
  fn id(&self) -> &'static str {
    self.0.id()
  }

  fn producer(&self) -> Producer {
    self.0.producer()
  }

  fn applies_to(&self, facts: &FileFacts) -> bool {
    self.0.applies_to(facts)
  }

  fn detect(&self, input: &DetectInput<'_>) -> Detection {
    self.0.detect(input)
  }
}

impl DiagnosingDetector for SharedDetector {
  fn detect_with_diagnostics(&self, input: &DetectInput<'_>) -> (Detection, Vec<Diagnostic>) {
    self.0.detect_with_diagnostics(input)
  }
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
  if let Some(message) = payload.downcast_ref::<&str>() {
    return (*message).to_owned();
  }
  if let Some(message) = payload.downcast_ref::<String>() {
    return message.clone();
  }
  "non-string panic payload".to_owned()
}

pub(crate) fn is_generated(file_name: &str, source: &str) -> bool {
  let inner_segments: Vec<&str> = file_name.split('.').collect();
  let named_generated = inner_segments.len() > 2 && inner_segments[1..inner_segments.len() - 1].iter().any(|segment| matches!(*segment, "generated" | "g"));
  if named_generated {
    return true;
  }
  source
    .lines()
    .take(GENERATED_HEADER_LINES)
    .any(|line| line.contains("@generated") || (line.contains("Code generated") && line.contains("DO NOT EDIT")))
}

#[cfg(test)]
mod tests {
  use super::is_generated;

  #[test]
  fn should_flag_generated_files_by_name_and_header() {
    assert!(is_generated("api.generated.ts", ""));
    assert!(is_generated("parser.g.rs", ""));
    assert!(is_generated("schema.ts", "// header\n// @generated by protoc\n"));
    assert!(is_generated("x.rs", "// Code generated by tool. DO NOT EDIT.\n"));
    assert!(!is_generated("g.ts", ""));
    assert!(!is_generated("generated.ts", ""));
    assert!(!is_generated("x.ts", "1\n2\n3\n4\n5\n// @generated\n"));
  }
}
