use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use atlas_core::{Availability, LlmError, LlmProvider, LlmRequest, LlmResponse, TokenUsage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeStep {
  pub result: Result<String, LlmError>,
  pub usage: TokenUsage,
  pub delay: Duration,
  pub model: Option<String>,
}

impl FakeStep {
  pub fn respond(text: impl Into<String>) -> Self {
    Self {
      result: Ok(text.into()),
      usage: TokenUsage {
        input: 100,
        output: 50,
        cache_read: 0,
        cache_write: 0,
      },
      delay: Duration::ZERO,
      model: None,
    }
  }

  pub fn fail(error: LlmError) -> Self {
    Self {
      result: Err(error),
      usage: TokenUsage::default(),
      delay: Duration::ZERO,
      model: None,
    }
  }

  pub fn with_usage(mut self, usage: TokenUsage) -> Self {
    self.usage = usage;
    self
  }

  pub fn with_model(mut self, model: impl Into<String>) -> Self {
    self.model = Some(model.into());
    self
  }

  pub fn with_delay(mut self, delay: Duration) -> Self {
    self.delay = delay;
    self
  }
}

#[derive(Debug)]
struct Script {
  queue: VecDeque<FakeStep>,
  rules: Vec<(String, FakeStep)>,
  fallback: Option<FakeStep>,
}

#[derive(Debug)]
pub struct FakeProvider {
  id: String,
  model: String,
  availability: Mutex<Availability>,
  script: Mutex<Script>,
  requests: Mutex<Vec<LlmRequest>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for FakeProvider {
  fn default() -> Self {
    Self::new()
  }
}

impl FakeProvider {
  pub fn new() -> Self {
    Self {
      id: "fake".to_owned(),
      model: "fake-model".to_owned(),
      availability: Mutex::new(Availability::Available),
      script: Mutex::new(Script {
        queue: VecDeque::new(),
        rules: Vec::new(),
        fallback: None,
      }),
      requests: Mutex::new(Vec::new()),
    }
  }

  pub fn with_identity(mut self, id: impl Into<String>, model: impl Into<String>) -> Self {
    self.id = id.into();
    self.model = model.into();
    self
  }

  pub fn unavailable(self, reason: impl Into<String>) -> Self {
    *lock(&self.availability) = Availability::Unavailable { reason: reason.into() };
    self
  }

  pub fn then(self, step: FakeStep) -> Self {
    lock(&self.script).queue.push_back(step);
    self
  }

  pub fn when_prompt_contains(self, needle: impl Into<String>, step: FakeStep) -> Self {
    lock(&self.script).rules.push((needle.into(), step));
    self
  }

  pub fn otherwise(self, step: FakeStep) -> Self {
    lock(&self.script).fallback = Some(step);
    self
  }

  pub fn requests(&self) -> Vec<LlmRequest> {
    lock(&self.requests).clone()
  }

  pub fn call_count(&self) -> usize {
    lock(&self.requests).len()
  }

  fn next_step(&self, request: &LlmRequest) -> Option<FakeStep> {
    let mut script = lock(&self.script);
    if let Some((_, step)) = script.rules.iter().find(|(needle, _)| request.prompt.contains(needle.as_str())) {
      return Some(step.clone());
    }
    script.queue.pop_front().or_else(|| script.fallback.clone())
  }
}

impl LlmProvider for FakeProvider {
  fn id(&self) -> &str {
    &self.id
  }

  fn model(&self) -> &str {
    &self.model
  }

  fn availability(&self) -> Availability {
    lock(&self.availability).clone()
  }

  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
    lock(&self.requests).push(request.clone());
    let step = self
      .next_step(request)
      .ok_or_else(|| LlmError::Provider("fake provider has no scripted response left".to_owned()))?;
    if step.delay > request.timeout {
      thread::sleep(request.timeout);
      return Err(LlmError::Timeout(request.timeout));
    }
    thread::sleep(step.delay);
    let model = step.model.unwrap_or_else(|| self.model.clone());
    step.result.map(|text| LlmResponse { text, usage: step.usage, model })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn request(prompt: &str, timeout: Duration) -> LlmRequest {
    LlmRequest {
      system: "s".into(),
      prompt: prompt.into(),
      max_output_tokens: 8,
      json_schema: None,
      timeout,
    }
  }

  #[test]
  fn should_replay_the_script_in_order_and_record_requests() {
    let fake = FakeProvider::new()
      .then(FakeStep::respond("one"))
      .then(FakeStep::fail(LlmError::Provider("boom".into())))
      .otherwise(FakeStep::respond("rest"));
    let timeout = Duration::from_secs(1);
    assert_eq!(fake.complete(&request("a", timeout)).unwrap().text, "one");
    assert_eq!(fake.complete(&request("b", timeout)), Err(LlmError::Provider("boom".into())));
    assert_eq!(fake.complete(&request("c", timeout)).unwrap().text, "rest");
    assert_eq!(fake.requests().iter().map(|r| r.prompt.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
  }

  #[test]
  fn should_match_rules_by_prompt_before_the_queue() {
    let fake = FakeProvider::new()
      .when_prompt_contains("needle", FakeStep::respond("ruled"))
      .then(FakeStep::respond("queued"));
    let timeout = Duration::from_secs(1);
    assert_eq!(fake.complete(&request("has needle", timeout)).unwrap().text, "ruled");
    assert_eq!(fake.complete(&request("plain", timeout)).unwrap().text, "queued");
  }

  #[test]
  fn should_time_out_when_the_scripted_delay_exceeds_the_request_timeout() {
    let fake = FakeProvider::new().otherwise(FakeStep::respond("late").with_delay(Duration::from_millis(200)));
    let timeout = Duration::from_millis(20);
    assert_eq!(fake.complete(&request("x", timeout)), Err(LlmError::Timeout(timeout)));
  }

  #[test]
  fn should_fail_loudly_when_the_script_is_exhausted() {
    assert!(matches!(FakeProvider::new().complete(&request("x", Duration::from_secs(1))), Err(LlmError::Provider(_))));
  }
}
