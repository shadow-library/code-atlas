//! Tree-sitter TypeScript and JavaScript analyzer. It never panics, hangs or overflows a 2 MiB stack: a file that outlasts its `ParseBudget` yields failed facts.

mod analyzer;
mod args;
mod extract;
mod fingerprint;
mod grammar;
mod modules;
mod scope;
mod signature;
mod syntax;
mod tagged_templates;
mod testing;
mod type_arguments;

pub use analyzer::{ANALYZER_NAME, ANALYZER_VERSION, MAX_SOURCE_BYTES, TsAnalyzer};
pub use atlas_core::ParseBudget;
