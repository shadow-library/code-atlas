use atlas_core::{Attrs, DetectedEdge, DetectedEntity, Detection, Diagnostic, EdgeKind, EntityKey, NodeRef, Severity, Span};
use serde_json::Value;

use crate::cert::Cert;

#[derive(Debug, Default)]
pub(crate) struct Sink {
  detection: Detection,
  diagnostics: Vec<Diagnostic>,
}

impl Sink {
  pub fn entity(&mut self, key: EntityKey, span: Span, cert: Cert, attrs: Attrs) {
    let Some(certainty) = cert.to_certainty() else {
      tracing::warn!(key = %key, "dropping entity with an invalid confidence");
      return;
    };
    if let Some(existing) = self.detection.entities.iter_mut().find(|entity| entity.key == key) {
      if certainty.cmp_strength(&existing.certainty).is_gt() {
        existing.certainty = certainty;
        existing.span = span;
      }
      for (name, value) in attrs {
        existing.attrs.entry(name).or_insert(value);
      }
      return;
    }
    self.detection.entities.push(DetectedEntity {
      name: key.display_name(),
      key,
      certainty,
      span,
      attrs,
    });
  }

  pub fn edge(&mut self, kind: EdgeKind, from: NodeRef, to: NodeRef, span: Span, cert: Cert, attrs: Attrs) {
    let Some(certainty) = cert.to_certainty() else {
      tracing::warn!(kind = %kind, "dropping edge with an invalid confidence");
      return;
    };
    let edge = DetectedEdge {
      kind,
      from,
      to,
      certainty,
      span,
      note: None,
      attrs,
    };
    if !self.detection.edges.contains(&edge) {
      self.detection.edges.push(edge);
    }
  }

  pub fn diagnostic(&mut self, severity: Severity, code: &str, message: String, span: Option<Span>) {
    self.diagnostics.push(Diagnostic {
      severity,
      code: code.to_owned(),
      message,
      span,
    });
  }

  pub fn finish(self) -> (Detection, Vec<Diagnostic>) {
    (self.detection, self.diagnostics)
  }
}

pub(crate) fn attrs<const N: usize>(entries: [(&str, Value); N]) -> Attrs {
  entries
    .into_iter()
    .filter(|(_, value)| !value.is_null())
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}
