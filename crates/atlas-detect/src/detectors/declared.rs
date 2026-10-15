use atlas_core::{DetectInput, Detection, Detector, Diagnostic, FileFacts, Producer};

use super::routes::{detect_family_routes, family_applies};
use crate::families::{DecoratorFamily, families_digest};
use crate::registry::DiagnosingDetector;
use crate::sink::Sink;
use crate::view::FileView;

pub const DECLARED_ROUTES_ID: &str = "http-endpoints-declared";
pub const DECLARED_ROUTES_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRoutesDetector {
  families: Vec<DecoratorFamily>,
  version: u32,
}

impl DeclaredRoutesDetector {
  pub fn new(mut families: Vec<DecoratorFamily>) -> Self {
    families.sort_by(|left, right| left.name.cmp(&right.name));
    families.dedup_by(|later, earlier| later.name == earlier.name);
    let digest = families_digest(&families);
    let mut seed = DECLARED_ROUTES_VERSION.to_le_bytes().to_vec();
    seed.extend_from_slice(digest.as_bytes());
    let hash = atlas_core::ContentHash::of(&seed);
    let [a, b, c, d, ..] = *hash.as_bytes();
    Self {
      families,
      version: u32::from_le_bytes([a, b, c, d]),
    }
  }

  pub fn families(&self) -> &[DecoratorFamily] {
    &self.families
  }
}

impl Detector for DeclaredRoutesDetector {
  fn id(&self) -> &'static str {
    DECLARED_ROUTES_ID
  }

  fn producer(&self) -> Producer {
    Producer::new(format!("atlas-detect.{DECLARED_ROUTES_ID}"), self.version)
  }

  fn applies_to(&self, facts: &FileFacts) -> bool {
    self.families.iter().any(|family| family_applies(family, facts))
  }

  fn detect(&self, input: &DetectInput<'_>) -> Detection {
    self.detect_with_diagnostics(input).0
  }
}

impl DiagnosingDetector for DeclaredRoutesDetector {
  fn detect_with_diagnostics(&self, input: &DetectInput<'_>) -> (Detection, Vec<Diagnostic>) {
    let view = FileView::new(input);
    let mut sink = Sink::default();
    for family in self.families.iter().filter(|family| family_applies(family, input.facts)) {
      detect_family_routes(family, &view, &mut sink);
    }
    sink.finish()
  }
}
