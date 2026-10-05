mod diagnostic;
mod document;
mod edit;
mod jsonrpc;
mod lifecycle;
mod symbol;

use serde::{Deserialize, Deserializer};

pub use diagnostic::*;
pub use document::*;
pub use edit::*;
pub use jsonrpc::*;
pub use lifecycle::*;
pub use symbol::*;

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
  D: Deserializer<'de>,
  T: Default + Deserialize<'de>,
{
  Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}
