use std::collections::BTreeMap;

use atlas_core::TokenUsage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
  pub input_per_mtok_usd: f64,
  pub output_per_mtok_usd: f64,
  pub cache_read_per_mtok_usd: f64,
  pub cache_write_per_mtok_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceTable {
  by_model_prefix: BTreeMap<String, ModelPrice>,
}

impl PriceTable {
  pub fn empty() -> Self {
    Self { by_model_prefix: BTreeMap::new() }
  }

  pub fn with_price(mut self, model_prefix: impl Into<String>, price: ModelPrice) -> Self {
    self.by_model_prefix.insert(model_prefix.into(), price);
    self
  }

  pub fn estimate_usd(&self, model: &str, usage: &TokenUsage) -> Option<f64> {
    let (_, price) = self
      .by_model_prefix
      .iter()
      .filter(|(prefix, _)| model.starts_with(prefix.as_str()))
      .max_by_key(|(prefix, _)| prefix.len())?;
    let million = 1_000_000.0;
    Some(
      (usage.input as f64 * price.input_per_mtok_usd
        + usage.output as f64 * price.output_per_mtok_usd
        + usage.cache_read as f64 * price.cache_read_per_mtok_usd
        + usage.cache_write as f64 * price.cache_write_per_mtok_usd)
        / million,
    )
  }
}

// Unverified estimates for budgeting only; override with `with_price`. Unknown models have no cost estimate.
impl Default for PriceTable {
  fn default() -> Self {
    Self::empty()
      .with_price(
        "claude-sonnet",
        ModelPrice {
          input_per_mtok_usd: 3.0,
          output_per_mtok_usd: 15.0,
          cache_read_per_mtok_usd: 0.3,
          cache_write_per_mtok_usd: 3.75,
        },
      )
      .with_price(
        "claude-haiku-4-5",
        ModelPrice {
          input_per_mtok_usd: 1.0,
          output_per_mtok_usd: 5.0,
          cache_read_per_mtok_usd: 0.1,
          cache_write_per_mtok_usd: 1.25,
        },
      )
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
      input,
      output,
      cache_read: 0,
      cache_write: 0,
    }
  }

  #[test]
  fn should_price_known_models_per_million_tokens() {
    let table = PriceTable::default();
    assert_eq!(table.estimate_usd("claude-sonnet-4-5", &usage(1_000_000, 100_000)), Some(4.5));
  }

  #[test]
  fn should_prefer_the_longest_matching_prefix() {
    let cheap = ModelPrice {
      input_per_mtok_usd: 1.0,
      output_per_mtok_usd: 1.0,
      cache_read_per_mtok_usd: 0.0,
      cache_write_per_mtok_usd: 0.0,
    };
    let table = PriceTable::default().with_price("claude-sonnet-lite", cheap);
    assert_eq!(table.estimate_usd("claude-sonnet-lite-1", &usage(1_000_000, 0)), Some(1.0));
  }

  #[test]
  fn should_not_guess_a_price_for_unknown_models() {
    assert_eq!(PriceTable::default().estimate_usd("gpt-4o", &usage(10, 10)), None);
  }
}
