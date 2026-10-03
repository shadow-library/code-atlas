use atlas_core::{Availability, LlmError, LlmProvider, LlmRequest, LlmResponse, TokenUsage};
use serde_json::{Value, json};

use super::http::{RetryPolicy, post_json};

const PROVIDER_ID: &str = "openai-compatible";
const RETRY: RetryPolicy = RetryPolicy::new(&[429]);

pub struct OpenAiCompatibleProvider {
  base_url: String,
  model: String,
  api_key_env: String,
  api_key: Option<String>,
}

impl OpenAiCompatibleProvider {
  pub fn from_env(base_url: impl Into<String>, model: impl Into<String>, api_key_env: impl Into<String>) -> Self {
    let api_key_env = api_key_env.into();
    let api_key = std::env::var(&api_key_env).ok().filter(|key| !key.trim().is_empty());
    Self {
      base_url: base_url.into().trim_end_matches('/').to_owned(),
      model: model.into(),
      api_key_env,
      api_key,
    }
  }

  pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
    self.api_key = Some(api_key.into());
    self
  }
}

impl LlmProvider for OpenAiCompatibleProvider {
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
        reason: format!("{} is not set", self.api_key_env),
      },
    }
  }

  fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
    let api_key = self.api_key.as_ref().ok_or_else(|| LlmError::Unavailable(format!("{} is not set", self.api_key_env)))?;
    let body = json!({
      "model": self.model,
      "max_tokens": request.max_output_tokens,
      "messages": [
        { "role": "system", "content": request.system },
        { "role": "user", "content": request.prompt },
      ],
    });
    let headers = [("authorization", format!("Bearer {api_key}"))];
    let reply = post_json(&format!("{}/chat/completions", self.base_url), &headers, &body, request.timeout, RETRY)?;
    parse_reply(&reply, &self.model)
  }
}

fn parse_reply(reply: &Value, requested_model: &str) -> Result<LlmResponse, LlmError> {
  let text = reply.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default();
  if text.trim().is_empty() {
    return Err(LlmError::InvalidResponse("reply has no message content".to_owned()));
  }
  let tokens = |pointer: &str| reply.pointer(pointer).and_then(Value::as_u64).unwrap_or(0);
  let cached = tokens("/usage/prompt_tokens_details/cached_tokens");
  Ok(LlmResponse {
    text: text.to_owned(),
    model: reply.get("model").and_then(Value::as_str).unwrap_or(requested_model).to_owned(),
    usage: TokenUsage {
      input: tokens("/usage/prompt_tokens").saturating_sub(cached),
      output: tokens("/usage/completion_tokens"),
      cache_read: cached,
      cache_write: 0,
    },
  })
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::super::http::test_server::{Reply, serve};
  use super::*;

  fn request() -> LlmRequest {
    LlmRequest {
      system: "sys".into(),
      prompt: "hi".into(),
      max_output_tokens: 64,
      json_schema: None,
      timeout: Duration::from_secs(10),
    }
  }

  #[test]
  fn should_call_chat_completions_with_a_bearer_key_and_parse_the_reply() {
    let body =
      r#"{"model":"local-1","choices":[{"message":{"content":"{\"ok\":true}"}}],"usage":{"prompt_tokens":20,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":8}}}"#;
    let server = serve(vec![Reply {
      status: 200,
      headers: vec![],
      body,
    }]);
    let provider = OpenAiCompatibleProvider::from_env(format!("{}/v1/", server.base_url), "local", "ATLAS_TEST_UNSET_KEY").with_api_key("k");
    let response = provider.complete(&request()).unwrap();
    assert_eq!(response.text, r#"{"ok":true}"#);
    assert_eq!(response.model, "local-1");
    assert_eq!(
      response.usage,
      TokenUsage {
        input: 12,
        output: 4,
        cache_read: 8,
        cache_write: 0
      }
    );
    let raw = server.requests.lock().unwrap()[0].clone();
    assert!(raw.starts_with("POST /v1/chat/completions"));
    assert!(raw.to_ascii_lowercase().contains("authorization: bearer k"));
  }

  #[test]
  fn should_be_unavailable_when_the_configured_env_var_is_unset() {
    let provider = OpenAiCompatibleProvider::from_env("http://localhost:1", "m", "ATLAS_TEST_UNSET_KEY");
    assert_eq!(
      provider.availability(),
      Availability::Unavailable {
        reason: "ATLAS_TEST_UNSET_KEY is not set".to_owned()
      }
    );
  }
}
