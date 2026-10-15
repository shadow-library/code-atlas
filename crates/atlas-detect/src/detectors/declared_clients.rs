use atlas_core::{ContentHash, DetectInput, Detection, Detector, Diagnostic, FileFacts, LanguageFamily, Producer};

use super::clients::{Request, emit_requests};
use super::typed_clients::{TypedShape, shape_applies, typed_request};
use crate::families::{ClientShape, DecoratorFamily};
use crate::registry::DiagnosingDetector;
use crate::sink::Sink;
use crate::view::FileView;

pub const DECLARED_CLIENTS_ID: &str = "http-clients-declared";
pub const DECLARED_CLIENTS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredClientsDetector {
  clients: Vec<(String, ClientShape)>,
  version: u32,
}

impl DeclaredClientsDetector {
  pub fn new(families: &[DecoratorFamily]) -> Self {
    let mut clients: Vec<(String, ClientShape)> = families
      .iter()
      .filter_map(|family| family.client().map(|shape| (family.name().to_owned(), shape.clone())))
      .collect();
    clients.sort_by(|left, right| left.0.cmp(&right.0));
    clients.dedup_by(|later, earlier| later.0 == earlier.0);
    let mut seed = DECLARED_CLIENTS_VERSION.to_le_bytes().to_vec();
    seed.extend_from_slice(serde_json::to_string(&clients).unwrap_or_default().as_bytes());
    let [a, b, c, d, ..] = *ContentHash::of(&seed).as_bytes();
    Self {
      clients,
      version: u32::from_le_bytes([a, b, c, d]),
    }
  }

  pub fn is_empty(&self) -> bool {
    self.clients.is_empty()
  }
}

impl Detector for DeclaredClientsDetector {
  fn id(&self) -> &'static str {
    DECLARED_CLIENTS_ID
  }

  fn producer(&self) -> Producer {
    Producer::new(format!("atlas-detect.{DECLARED_CLIENTS_ID}"), self.version)
  }

  fn applies_to(&self, facts: &FileFacts) -> bool {
    facts.file.language.family() == LanguageFamily::Ts && self.clients.iter().any(|(_, shape)| shape_applies(facts, shape))
  }

  fn detect(&self, input: &DetectInput<'_>) -> Detection {
    self.detect_with_diagnostics(input).0
  }
}

impl DiagnosingDetector for DeclaredClientsDetector {
  fn detect_with_diagnostics(&self, input: &DetectInput<'_>) -> (Detection, Vec<Diagnostic>) {
    let view = FileView::new(input);
    let mut sink = Sink::default();
    let shapes: Vec<TypedShape<'_>> = self
      .clients
      .iter()
      .filter(|(_, shape)| shape_applies(input.facts, shape))
      .map(|(library, shape)| TypedShape { library, shape })
      .collect();
    let requests: Vec<Request<'_>> = input
      .facts
      .references
      .iter()
      .filter_map(|reference| shapes.iter().find_map(|typed| typed_request(&view, reference, typed)))
      .collect();
    emit_requests(&view, &mut sink, &requests);
    sink.finish()
  }
}
