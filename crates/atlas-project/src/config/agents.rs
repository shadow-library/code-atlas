use toml::Value;

use super::keys::as_words;
use super::user::UserConfig;

const DEFAULT_BINARY: &str = "catl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceMode {
  None,
  Auto,
  Local,
  Tracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeScope {
  User,
  Local,
  Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClaudeHook {
  AutoApprove,
  Remind,
  Seed,
  Counterpart,
  Notes,
}

impl ClaudeHook {
  pub fn name(self) -> &'static str {
    match self {
      Self::AutoApprove => "auto-approve",
      Self::Remind => "remind",
      Self::Seed => "seed",
      Self::Counterpart => "counterpart",
      Self::Notes => "notes",
    }
  }

  fn parse(name: &str) -> Option<Self> {
    match name {
      "auto-approve" => Some(Self::AutoApprove),
      "remind" => Some(Self::Remind),
      "seed" => Some(Self::Seed),
      "counterpart" => Some(Self::Counterpart),
      "notes" => Some(Self::Notes),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HooksSource {
  BuiltIn,
  ConfigFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeAgent {
  pub enabled: bool,
  pub scope: ClaudeScope,
  pub hooks: Vec<ClaudeHook>,
  pub hooks_source: HooksSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAgent {
  pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsSettings {
  pub binary: String,
  pub guidance: GuidanceMode,
  pub claude: ClaudeAgent,
  pub codex: CodexAgent,
}

impl Default for AgentsSettings {
  fn default() -> Self {
    Self {
      binary: DEFAULT_BINARY.to_owned(),
      guidance: GuidanceMode::Auto,
      claude: ClaudeAgent {
        enabled: true,
        scope: ClaudeScope::User,
        hooks: vec![ClaudeHook::AutoApprove, ClaudeHook::Seed],
        hooks_source: HooksSource::BuiltIn,
      },
      codex: CodexAgent { enabled: true },
    }
  }
}

impl UserConfig {
  pub fn agents(&self) -> AgentsSettings {
    let mut settings = AgentsSettings::default();
    for entry in self.reserved_entries() {
      let Some(value) = entry.value.as_ref() else { continue };
      settings.apply(&entry.key, value);
    }
    settings
  }
}

impl AgentsSettings {
  fn apply(&mut self, key: &str, value: &Value) {
    let text = value.as_str();
    match (key, text) {
      ("agents.binary", Some(binary)) => self.binary = binary.to_owned(),
      ("agents.guidance", Some("none")) => self.guidance = GuidanceMode::None,
      ("agents.guidance", Some("auto")) => self.guidance = GuidanceMode::Auto,
      ("agents.guidance", Some("local")) => self.guidance = GuidanceMode::Local,
      ("agents.guidance", Some("tracked")) => self.guidance = GuidanceMode::Tracked,
      ("agents.claude.scope", Some("user")) => self.claude.scope = ClaudeScope::User,
      ("agents.claude.scope", Some("local")) => self.claude.scope = ClaudeScope::Local,
      ("agents.claude.scope", Some("project")) => self.claude.scope = ClaudeScope::Project,
      ("agents.claude.enabled", None) => self.claude.enabled = value.as_bool().unwrap_or(self.claude.enabled),
      ("agents.codex.enabled", None) => self.codex.enabled = value.as_bool().unwrap_or(self.codex.enabled),
      ("agents.claude.hooks", None) => {
        let mut hooks: Vec<ClaudeHook> = as_words(value).unwrap_or_default().iter().filter_map(|name| ClaudeHook::parse(name)).collect();
        hooks.sort();
        hooks.dedup();
        self.claude.hooks = hooks;
        self.claude.hooks_source = HooksSource::ConfigFile;
      }
      _ => {}
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;
  use crate::home::AtlasHome;

  fn agents(text: &str) -> AgentsSettings {
    let dir = tempfile::tempdir().unwrap();
    let home = AtlasHome::rooted_at(dir.path());
    fs::create_dir_all(home.config_dir()).unwrap();
    fs::write(home.config_path(), text).unwrap();
    UserConfig::load(&home).unwrap().agents()
  }

  #[test]
  fn should_default_to_a_user_scope_claude_with_auto_approve_seeding_and_codex_enabled() {
    assert_eq!(agents(""), AgentsSettings::default());
    assert_eq!(AgentsSettings::default().claude.hooks, [ClaudeHook::AutoApprove, ClaudeHook::Seed]);
  }

  #[test]
  fn should_read_every_agents_key_from_config_toml() {
    let settings = agents(
      "[agents]\nbinary = \"/opt/bin/catl\"\nguidance = \"local\"\n[agents.claude]\nenabled = true\nscope = \"local\"\nhooks = [\"notes\", \"counterpart\", \"seed\", \"remind\", \"auto-approve\"]\n[agents.codex]\nenabled = false\n",
    );
    assert_eq!(
      settings,
      AgentsSettings {
        binary: "/opt/bin/catl".to_owned(),
        guidance: GuidanceMode::Local,
        claude: ClaudeAgent {
          enabled: true,
          scope: ClaudeScope::Local,
          hooks: vec![ClaudeHook::AutoApprove, ClaudeHook::Remind, ClaudeHook::Seed, ClaudeHook::Counterpart, ClaudeHook::Notes],
          hooks_source: HooksSource::ConfigFile,
        },
        codex: CodexAgent { enabled: false },
      }
    );
  }

  #[test]
  fn should_allow_turning_every_hook_off() {
    assert_eq!(agents("[agents.claude]\nhooks = []\n").claude.hooks, []);
  }
}
