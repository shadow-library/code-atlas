mod codec;
mod config;
mod doc;
mod error;
mod hashing;
mod index;
mod lexical;
mod minhash;
mod structural;
mod synonyms;
mod text;

pub use config::SimilarityConfig;
pub use doc::SimilarityDoc;
pub use error::SimilarityError;
pub use index::{MemoryEstimate, SearchHit, SearchOptions, SimilarityIndex};
pub use lexical::{ExpansionMatch, Field, FieldMatch};
pub use minhash::{LANES, SHINGLE_SIZE, SMALL_BODY_TOKENS, SMALL_SHINGLE_SIZE, collision_probability};
pub use structural::{ClusterReport, SMALL_PAIR_MIN_JACCARD, StructuralCluster, StructuralMatch, StructuralPair, StructuralStatus};
pub use text::{analyze as analyze_terms, name_words, names_related};

#[cfg(test)]
mod tests;
