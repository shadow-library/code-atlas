mod analyzer;
mod args;
mod extract;
mod fingerprint;
mod grammar;
mod modules;
mod scope;
mod signature;
mod syntax;
mod testing;

pub use analyzer::{ANALYZER_NAME, ANALYZER_VERSION, MAX_SOURCE_BYTES, TsAnalyzer};
