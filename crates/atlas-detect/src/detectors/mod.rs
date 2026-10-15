mod auth;
mod axum;
mod clients;
mod config;
mod db;
mod declared;
mod events;
mod express;
mod headers;
mod jobs;
mod mocks;
mod nest;
mod routes;
mod testing;

pub use auth::AuthSchemeDetector;
pub use axum::AxumEndpointDetector;
pub use clients::HttpClientDetector;
pub use config::ConfigEnvDetector;
pub use db::DbDetector;
pub use declared::{DECLARED_ROUTES_ID, DECLARED_ROUTES_VERSION, DeclaredRoutesDetector};
pub use events::EventDetector;
pub use express::ExpressEndpointDetector;
pub use jobs::JobDetector;
pub use mocks::MockDetector;
pub use nest::NestEndpointDetector;
pub use testing::TestDetector;

use atlas_core::{LocalSymbolId, NodeRef, Span};

use crate::view::FileView;

macro_rules! detector {
  ($name:ident, $id:literal, $version:literal) => {
    impl atlas_core::Detector for $name {
      fn id(&self) -> &'static str {
        $id
      }

      fn producer(&self) -> atlas_core::Producer {
        atlas_core::Producer::new(concat!("atlas-detect.", $id), $version)
      }

      fn applies_to(&self, facts: &atlas_core::FileFacts) -> bool {
        self.prefilter(facts)
      }

      fn detect(&self, input: &atlas_core::DetectInput<'_>) -> atlas_core::Detection {
        crate::registry::DiagnosingDetector::detect_with_diagnostics(self, input).0
      }
    }

    impl crate::registry::DiagnosingDetector for $name {
      fn detect_with_diagnostics(&self, input: &atlas_core::DetectInput<'_>) -> (atlas_core::Detection, Vec<atlas_core::Diagnostic>) {
        let view = crate::view::FileView::new(input);
        let mut sink = crate::sink::Sink::default();
        self.run(&view, &mut sink);
        sink.finish()
      }
    }
  };
}

pub(crate) use detector;

pub(crate) fn handler_node(view: &FileView<'_>, path: &[String], scope: Option<LocalSymbolId>) -> NodeRef {
  if let [name] = path
    && let Some(symbol) = view.top_level_named(name).filter(|symbol| symbol.kind.is_code_symbol())
  {
    return NodeRef::Local(symbol.local);
  }
  NodeRef::Unresolved { path: path.to_vec(), from: scope }
}

pub(crate) fn strip_ws(text: &str) -> String {
  text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub(crate) fn same_statement(view: &FileView<'_>, earlier: Span, later: Span) -> bool {
  later.start_byte >= earlier.start_byte && !view.tokens_in(earlier.start_byte, later.start_byte).iter().any(|token| token.is_punct(";"))
}
