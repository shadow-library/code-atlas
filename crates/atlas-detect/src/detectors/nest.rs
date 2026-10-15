use atlas_core::FileFacts;

use super::detector;
use super::routes::{detect_family_routes, family_applies};
use crate::families::NEST;
use crate::sink::Sink;
use crate::view::FileView;

pub struct NestEndpointDetector;

detector!(NestEndpointDetector, "http-endpoints-nest", 1);

impl NestEndpointDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    family_applies(&NEST, facts)
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    detect_family_routes(&NEST, view, sink);
  }
}
