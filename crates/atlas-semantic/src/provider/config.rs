use std::sync::Arc;

use atlas_core::LlmProvider;
use atlas_project::LlmSettings;

use super::anthropic::AnthropicHttpProvider;
use super::claude_cli::ClaudeCliProvider;
use super::openai::OpenAiCompatibleProvider;
use super::unavailable::UnavailableProvider;
use crate::error::{SemanticError, SemanticResult};
use crate::task::TaskKind;

pub const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

pub const SIMILARITY_DEFAULT_MODEL: &str = "claude-sonnet-5-5";
pub const LIGHTWEIGHT_DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";

pub fn default_model(kind: TaskKind) -> &'static str {
  match kind {
    TaskKind::ClassifySimilarityPair => SIMILARITY_DEFAULT_MODEL,
    TaskKind::SummarizeCapability | TaskKind::LinkUnresolvedClientCall => LIGHTWEIGHT_DEFAULT_MODEL,
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfig {
  None { reason: String },
  ClaudeCli { model: Option<String> },
  AnthropicHttp { model: Option<String> },
  OpenAiCompatible { base_url: String, model: String, api_key_env: String },
}

impl ProviderConfig {
  pub fn from_settings(settings: &LlmSettings) -> SemanticResult<Self> {
    if !settings.enabled {
      return Ok(Self::None {
        reason: "LLM enrichment is disabled in the project settings".to_owned(),
      });
    }
    let model = settings.model.clone().filter(|model| !model.trim().is_empty());
    match settings.provider.as_deref().map(str::trim) {
      None | Some("claude-cli" | "claude") => Ok(Self::ClaudeCli { model }),
      Some("none") => Ok(Self::None {
        reason: "LLM provider is set to none".to_owned(),
      }),
      Some("anthropic") => Ok(Self::AnthropicHttp { model }),
      Some("openai-compatible" | "openai") => {
        let base_url = std::env::var(OPENAI_BASE_URL_ENV).ok().filter(|url| !url.trim().is_empty());
        let base_url = base_url.ok_or_else(|| SemanticError::Config(format!("provider \"openai-compatible\" requires {OPENAI_BASE_URL_ENV}")))?;
        let model = model.ok_or_else(|| SemanticError::Config("provider \"openai-compatible\" requires llm.model".to_owned()))?;
        Ok(Self::OpenAiCompatible {
          base_url,
          model,
          api_key_env: OPENAI_API_KEY_ENV.to_owned(),
        })
      }
      Some(other) => Err(SemanticError::Config(format!("unknown LLM provider {other:?}"))),
    }
  }

  pub fn build_for(&self, kind: TaskKind) -> Box<dyn LlmProvider> {
    let concrete = |configured: &Option<String>| configured.clone().unwrap_or_else(|| default_model(kind).to_owned());
    match self {
      Self::None { reason } => Box::new(UnavailableProvider::new(reason.clone())),
      Self::ClaudeCli { model } => Box::new(ClaudeCliProvider::new(concrete(model))),
      Self::AnthropicHttp { model } => Box::new(AnthropicHttpProvider::from_env(concrete(model))),
      Self::OpenAiCompatible { base_url, model, api_key_env } => Box::new(OpenAiCompatibleProvider::from_env(base_url.clone(), model.clone(), api_key_env.clone())),
    }
  }
}

pub struct TaskProviders {
  similarity: Arc<dyn LlmProvider>,
  summary: Arc<dyn LlmProvider>,
  link: Arc<dyn LlmProvider>,
}

impl TaskProviders {
  pub fn from_config(config: &ProviderConfig) -> Self {
    Self {
      similarity: Arc::from(config.build_for(TaskKind::ClassifySimilarityPair)),
      summary: Arc::from(config.build_for(TaskKind::SummarizeCapability)),
      link: Arc::from(config.build_for(TaskKind::LinkUnresolvedClientCall)),
    }
  }

  pub fn shared(provider: Arc<dyn LlmProvider>) -> Self {
    Self {
      similarity: Arc::clone(&provider),
      summary: Arc::clone(&provider),
      link: provider,
    }
  }

  pub fn for_kind(&self, kind: TaskKind) -> &dyn LlmProvider {
    match kind {
      TaskKind::ClassifySimilarityPair => self.similarity.as_ref(),
      TaskKind::SummarizeCapability => self.summary.as_ref(),
      TaskKind::LinkUnresolvedClientCall => self.link.as_ref(),
    }
  }
}

pub fn providers_from_settings(settings: &LlmSettings) -> TaskProviders {
  match ProviderConfig::from_settings(settings) {
    Ok(config) => TaskProviders::from_config(&config),
    Err(error) => {
      tracing::warn!(error = %error, "LLM provider configuration is invalid; semantic enrichment is disabled");
      TaskProviders::from_config(&ProviderConfig::None { reason: error.to_string() })
    }
  }
}

#[cfg(test)]
mod tests {
  use atlas_core::Availability;

  use super::*;

  fn settings(provider: Option<&str>, model: Option<&str>, enabled: bool) -> LlmSettings {
    LlmSettings {
      provider: provider.map(str::to_owned),
      model: model.map(str::to_owned),
      enabled,
    }
  }

  #[test]
  fn should_map_default_settings_to_unavailable_providers() {
    let providers = providers_from_settings(&LlmSettings::default());
    let provider = providers.for_kind(TaskKind::SummarizeCapability);
    assert_eq!(provider.id(), "none");
    assert_eq!(
      provider.availability(),
      Availability::Unavailable {
        reason: "LLM enrichment is disabled in the project settings".to_owned()
      }
    );
  }

  #[test]
  fn should_resolve_concrete_per_task_default_models() {
    let providers = providers_from_settings(&settings(None, None, true));
    let models: Vec<&str> = [TaskKind::ClassifySimilarityPair, TaskKind::SummarizeCapability, TaskKind::LinkUnresolvedClientCall]
      .into_iter()
      .map(|kind| providers.for_kind(kind).model())
      .collect();
    assert_eq!(models, ["claude-sonnet-5-5", "claude-haiku-4-5-20251001", "claude-haiku-4-5-20251001"]);
    assert_eq!(providers.for_kind(TaskKind::SummarizeCapability).id(), "claude-cli");
  }

  #[test]
  fn should_let_a_configured_model_override_every_task() {
    let providers = providers_from_settings(&settings(Some("anthropic"), Some("claude-opus-x"), true));
    for kind in [TaskKind::ClassifySimilarityPair, TaskKind::SummarizeCapability, TaskKind::LinkUnresolvedClientCall] {
      assert_eq!((providers.for_kind(kind).id(), providers.for_kind(kind).model()), ("anthropic", "claude-opus-x"));
    }
  }

  #[test]
  fn should_default_anthropic_models_per_task_when_unconfigured() {
    let providers = providers_from_settings(&settings(Some("anthropic"), None, true));
    assert_eq!(providers.for_kind(TaskKind::ClassifySimilarityPair).model(), "claude-sonnet-5-5");
  }

  #[test]
  fn should_treat_an_explicit_none_as_unavailable() {
    assert!(matches!(
      ProviderConfig::from_settings(&settings(Some("none"), None, true)).unwrap(),
      ProviderConfig::None { .. }
    ));
  }

  #[test]
  fn should_use_the_configured_model_for_openai_compatible_providers() {
    let config = ProviderConfig::OpenAiCompatible {
      base_url: "http://x".into(),
      model: "m".into(),
      api_key_env: "K".into(),
    };
    assert_eq!(config.build_for(TaskKind::SummarizeCapability).model(), "m");
  }

  #[test]
  fn should_degrade_unknown_providers_to_unavailable_instead_of_guessing() {
    assert!(matches!(
      ProviderConfig::from_settings(&settings(Some("gemini"), None, true)),
      Err(SemanticError::Config(_))
    ));
    assert_eq!(
      providers_from_settings(&settings(Some("gemini"), None, true))
        .for_kind(TaskKind::LinkUnresolvedClientCall)
        .id(),
      "none"
    );
  }
}
