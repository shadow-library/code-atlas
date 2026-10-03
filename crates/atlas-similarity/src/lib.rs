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
pub use minhash::{LANES, SHINGLE_SIZE, collision_probability};
pub use structural::{ClusterReport, StructuralCluster, StructuralMatch, StructuralStatus};

#[cfg(test)]
mod tests;
