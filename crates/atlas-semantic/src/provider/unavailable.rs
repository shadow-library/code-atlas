use atlas_core::{Availability, LlmError, LlmProvider, LlmRequest, LlmResponse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableProvider {
  reason: String,
}

impl UnavailableProvider {
  pub fn new(reason: impl Into<String>) -> Self {
    Self { reason: reason.into() }
  }
}

impl LlmProvider for UnavailableProvider {
  fn id(&self) -> &str {
    "none"
  }

  fn model(&self) -> &str {
    "none"
  }

  fn availability(&self) -> Availability {
    Availability::Unavailable { reason: self.reason.clone() }
  }

  fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
    Err(LlmError::Unavailable(self.reason.clone()))
  }
}
