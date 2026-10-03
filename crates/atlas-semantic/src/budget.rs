#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticBudget {
  pub max_calls_per_run: u32,
  pub max_input_tokens_per_run: u64,
  pub max_concurrency: usize,
}

impl Default for SemanticBudget {
  fn default() -> Self {
    Self {
      max_calls_per_run: 50,
      max_input_tokens_per_run: 200_000,
      max_concurrency: 4,
    }
  }
}

#[derive(Debug)]
pub(crate) struct BudgetState {
  limits: SemanticBudget,
  calls: u32,
  input_tokens: u64,
  spent_tokens: u64,
}

impl BudgetState {
  pub(crate) fn new(limits: SemanticBudget) -> Self {
    Self {
      limits,
      calls: 0,
      input_tokens: 0,
      spent_tokens: 0,
    }
  }

  pub(crate) fn try_reserve(&mut self, estimated_input_tokens: u64) -> bool {
    if self.calls >= self.limits.max_calls_per_run || self.input_tokens.saturating_add(estimated_input_tokens) > self.limits.max_input_tokens_per_run {
      return false;
    }
    self.calls += 1;
    self.input_tokens += estimated_input_tokens;
    true
  }

  pub(crate) fn settle(&mut self, estimated_input_tokens: u64, actual_input_tokens: u64) {
    self.input_tokens = self.input_tokens.saturating_sub(estimated_input_tokens).saturating_add(actual_input_tokens);
    self.spent_tokens = self.spent_tokens.saturating_add(actual_input_tokens);
  }

  pub(crate) fn has_headroom(&self) -> bool {
    self.spent_tokens < self.limits.max_input_tokens_per_run
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn state(calls: u32, tokens: u64) -> BudgetState {
    BudgetState::new(SemanticBudget {
      max_calls_per_run: calls,
      max_input_tokens_per_run: tokens,
      max_concurrency: 1,
    })
  }

  #[test]
  fn should_default_to_the_conservative_limits() {
    assert_eq!(
      SemanticBudget::default(),
      SemanticBudget {
        max_calls_per_run: 50,
        max_input_tokens_per_run: 200_000,
        max_concurrency: 4
      }
    );
  }

  #[test]
  fn should_refuse_reservations_beyond_the_call_limit() {
    let mut budget = state(2, 1_000);
    assert!([budget.try_reserve(1), budget.try_reserve(1), budget.try_reserve(1)] == [true, true, false]);
  }

  #[test]
  fn should_refuse_reservations_beyond_the_token_limit_and_honour_settlement() {
    let mut budget = state(10, 100);
    assert!(budget.try_reserve(60));
    assert!(!budget.try_reserve(60));
    budget.settle(60, 20);
    assert!(budget.try_reserve(60));
  }
}
