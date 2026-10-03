use std::time::Duration;

use atlas_core::{Availability, LlmError, LlmProvider, LlmRequest, LlmResponse, TokenUsage};
use serde_json::{Value, json};

use super::http::{RetryPolicy, post_json};

const PROVIDER_ID: &str = "anthropic";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const RETRY: RetryPolicy = RetryPolicy::new(&[429, 529]);

pub struct AnthropicHttpProvider {
  model: String,
  api_key: Option<String>,
  base_url: String,
  retry: RetryPolicy,
}

impl AnthropicHttpProvider {
  pub fn from_env(model: impl Into<String>) -> Self {
    let api_key = std::env::var(ANTHROPIC_API_KEY_ENV).ok().filter(|key| !key.trim().is_empty());
    Self {
      model: model.into(),
      api_key,
      base_url: DEFAULT_BASE_URL.to_owned(),
      retry: RETRY,
    }
  }

  pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
    self.api_key = Some(api_key.into());
    self
  }

  pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
    self.base_url = base_url.into().trim_end_matches('/').to_owned();
    self
  }

  pub fn with_max_retry_delay(mut self, max_delay: Duration) -> Self {
    self.retry.max_delay = max_delay;
    self
  }
}

impl LlmProvider for AnthropicHttpProvider {
  fn id(&self) -> &str {
    PROVIDER_ID
  }

  fn model(&self) -> &str {
    &self.model
  }

  fn availability(&self) -> Availability {
    match self.api_key {
      Some(_) => Availability::Available,
      None => Availability::Unavailable {
        reason: format!("{ANTHROPIC_API_KEY_ENV} is not set"),
      },
    }
  }

  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
    let api_key = self.api_key.as_ref().ok_or_else(|| LlmError::Unavailable(format!("{ANTHROPIC_API_KEY_ENV} is not set")))?;
    let body = json!({
      "model": self.model,
      "max_tokens": request.max_output_tokens,
      "system": request.system,
      "messages": [{ "role": "user", "content": request.prompt }],
    });
    let headers = [("x-api-key", api_key.clone()), ("anthropic-version", API_VERSION.to_owned())];
    let reply = post_json(&format!("{}/v1/messages", self.base_url), &headers, &body, request.timeout, self.retry)?;
    parse_reply(&reply, &self.model)
  }
}

fn parse_reply(reply: &Value, requested_model: &str) -> Result<LlmResponse, LlmError> {
  let blocks = reply
    .get("content")
    .and_then(Value::as_array)
    .ok_or_else(|| LlmError::InvalidResponse("reply has no content blocks".to_owned()))?;
  let text: String = blocks
    .iter()
    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
    .filter_map(|block| block.get("text").and_then(Value::as_str))
    .collect();
  if text.trim().is_empty() {
    return Err(LlmError::InvalidResponse("reply has no text content".to_owned()));
  }
  let tokens = |field: &str| reply.pointer(&format!("/usage/{field}")).and_then(Value::as_u64).unwrap_or(0);
  Ok(LlmResponse {
    text,
    model: reply.get("model").and_then(Value::as_str).unwrap_or(requested_model).to_owned(),
    usage: TokenUsage {
      input: tokens("input_tokens"),
      output: tokens("output_tokens"),
      cache_read: tokens("cache_read_input_tokens"),
      cache_write: tokens("cache_creation_input_tokens"),
    },
  })
}

#[cfg(test)]
mod tests {
  use super::super::http::test_server::{Reply, serve};
  use super::*;

  const OK_BODY: &str =
    r#"{"model":"claude-x-1","content":[{"type":"text","text":"{\"a\":"},{"type":"text","text":"1}"}],"usage":{"input_tokens":5,"output_tokens":2,"cache_read_input_tokens":1}}"#;

  fn request() -> LlmRequest {
    LlmRequest {
      system: "be brief".into(),
      prompt: "hi".into(),
      max_output_tokens: 64,
      json_schema: None,
      timeout: Duration::from_secs(10),
    }
  }

  fn provider(base_url: &str) -> AnthropicHttpProvider {
    AnthropicHttpProvider::from_env("claude-x").with_api_key("secret").with_base_url(base_url)
  }

  #[test]
  fn should_post_a_messages_request_and_parse_text_and_usage() {
    let server = serve(vec![Reply {
      status: 200,
      headers: vec![],
      body: OK_BODY,
    }]);
    let response = provider(&server.base_url).complete(&request()).unwrap();
    assert_eq!(response.text, r#"{"a":1}"#);
    assert_eq!(response.model, "claude-x-1");
    assert_eq!(
      response.usage,
      TokenUsage {
        input: 5,
        output: 2,
        cache_read: 1,
        cache_write: 0
      }
    );
    let raw = server.requests.lock().unwrap()[0].clone();
    assert!(raw.starts_with("POST /v1/messages"));
    assert!(raw.to_ascii_lowercase().contains("x-api-key: secret"));
    assert!(raw.contains(r#""system": "be brief""#));
  }

  #[test]
  fn should_retry_throttled_requests_honouring_retry_after() {
    let throttled = Reply {
      status: 429,
      headers: vec![("retry-after", "0")],
      body: "{}",
    };
    let overloaded = Reply {
      status: 529,
      headers: vec![("retry-after", "0")],
      body: "{}",
    };
    let server = serve(vec![
      throttled,
      overloaded,
      Reply {
        status: 200,
        headers: vec![],
        body: OK_BODY,
      },
    ]);
    assert_eq!(provider(&server.base_url).complete(&request()).unwrap().text, r#"{"a":1}"#);
    assert_eq!(server.requests.lock().unwrap().len(), 3);
  }

  #[test]
  fn should_give_up_as_rate_limited_after_two_retries() {
    let replies = (0..3)
      .map(|_| Reply {
        status: 429,
        headers: vec![("retry-after", "0")],
        body: "{}",
      })
      .collect();
    let server = serve(replies);
    let error = provider(&server.base_url).complete(&request()).unwrap_err();
    assert_eq!(
      error,
      LlmError::RateLimited {
        retry_after: Some(Duration::ZERO)
      }
    );
    assert_eq!(server.requests.lock().unwrap().len(), 3);
  }

  #[test]
  fn should_not_retry_other_http_errors() {
    let server = serve(vec![Reply {
      status: 401,
      headers: vec![],
      body: r#"{"error":"bad key"}"#,
    }]);
    let error = provider(&server.base_url).complete(&request()).unwrap_err();
    assert_eq!(error, LlmError::Provider(r#"HTTP 401: {"error":"bad key"}"#.to_owned()));
  }

  #[test]
  fn should_be_unavailable_without_an_api_key() {
    let provider = AnthropicHttpProvider {
      model: "m".into(),
      api_key: None,
      base_url: String::new(),
      retry: RETRY,
    };
    assert_eq!(
      provider.availability(),
      Availability::Unavailable {
        reason: "ANTHROPIC_API_KEY is not set".to_owned()
      }
    );
    assert!(matches!(provider.complete(&request()), Err(LlmError::Unavailable(_))));
  }
}
