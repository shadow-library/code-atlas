//! Tree-sitter Rust analyzer. It never panics, hangs or overflows a 2 MiB stack: a file that outlasts its `ParseBudget` yields failed facts.

mod analyzer;
mod extract;
mod fingerprint;
mod literals;
mod macros;
mod refs;
mod source;
mod summary;
mod syntax;
mod tokens;

pub use analyzer::*;
pub use atlas_core::ParseBudget;
