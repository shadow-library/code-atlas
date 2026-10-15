use std::collections::BTreeMap;

use atlas_core::{ContentHash, DetectInput, DetectedEntity, Detection, Detector, Diagnostic, FileFacts, LanguageFamily, Producer, Severity};

use super::clients::{Request, emit_requests};
use super::typed_clients::{DefinitionRank, TypedShape, definition_rank, emit_client_definitions, emit_surface_definitions, shape_applies, typed_request};
use crate::families::{ClientShape, DecoratorFamily};
use crate::registry::DiagnosingDetector;
use crate::sink::Sink;
use crate::view::FileView;

pub const DECLARED_CLIENTS_ID: &str = "http-clients-declared";
pub const DECLARED_CLIENTS_VERSION: u32 = 3;
pub const CLIENT_DEFINITION_CONFLICT: &str = "client-definition-conflict";
const CONFLICT_MARKER: &str = " is claimed by families ";
const CONFLICT_SUFFIX: &str = "; none records it";

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
    let owners = shapes.iter().filter_map(|typed| Some((definition_rank(input.facts, typed.shape)?, typed)));
    emit_owned_definitions(&view, &mut sink, owners);
    sink.finish()
  }
}

pub fn conflicting_families(diagnostic: &Diagnostic) -> Vec<&str> {
  if diagnostic.code != CLIENT_DEFINITION_CONFLICT {
    return Vec::new();
  }
  let Some((_, rest)) = diagnostic.message.split_once(CONFLICT_MARKER) else {
    return Vec::new();
  };
  rest.strip_suffix(CONFLICT_SUFFIX).map_or_else(Vec::new, |families| families.split(", ").collect())
}

fn emit_owned_definitions<'s>(view: &FileView<'_>, sink: &mut Sink, owners: impl Iterator<Item = (DefinitionRank, &'s TypedShape<'s>)>) {
  let mut claims: Vec<(&str, Vec<(String, DetectedEntity)>)> = Vec::new();
  let mut claimants: BTreeMap<String, Vec<(DefinitionRank, &str)>> = BTreeMap::new();
  for (rank, typed) in owners {
    let mut scratch = Sink::default();
    emit_client_definitions(view, &mut scratch, typed);
    emit_surface_definitions(view, &mut scratch, typed);
    let entities: Vec<(String, DetectedEntity)> = scratch.finish().0.entities.into_iter().map(|entity| (entity.key.to_string(), entity)).collect();
    for (key, _) in &entities {
      claimants.entry(key.clone()).or_default().push((rank, typed.library));
    }
    claims.push((typed.library, entities));
  }
  let mut winners: BTreeMap<&str, &str> = BTreeMap::new();
  for (key, claimed) in &claimants {
    let top = claimed.iter().map(|(rank, _)| *rank).max();
    let leaders: Vec<&str> = claimed.iter().filter(|(rank, _)| Some(*rank) == top).map(|(_, library)| *library).collect();
    match leaders.as_slice() {
      [winner] => {
        winners.insert(key, winner);
      }
      _ => sink.diagnostic(
        Severity::Warning,
        CLIENT_DEFINITION_CONFLICT,
        format!("client definition {key}{CONFLICT_MARKER}{}{CONFLICT_SUFFIX}", leaders.join(", ")),
        None,
      ),
    }
  }
  for (library, entities) in claims {
    for (key, entity) in entities {
      if winners.get(key.as_str()) == Some(&library) {
        sink.adopt(entity);
      }
    }
  }
}
