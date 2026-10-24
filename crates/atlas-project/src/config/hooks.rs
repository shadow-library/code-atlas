use super::keys::SEED_BUDGET_RANGE;
use super::user::UserConfig;

const DEFAULT_SEED_BUDGET: usize = 299;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSettings {
  pub seed_budget: usize,
}

impl Default for HookSettings {
  fn default() -> Self {
    Self { seed_budget: DEFAULT_SEED_BUDGET }
  }
}

impl UserConfig {
  pub fn hooks(&self) -> HookSettings {
    let mut settings = HookSettings::default();
    for entry in self.reserved_entries() {
      let Some(value) = entry.value.as_ref() else { continue };
      if entry.key == "hooks.seed_budget"
        && let Some(budget) = value
          .as_integer()
          .and_then(|budget| u64::try_from(budget).ok())
          .filter(|budget| SEED_BUDGET_RANGE.contains(budget))
      {
        settings.seed_budget = usize::try_from(budget).unwrap_or(DEFAULT_SEED_BUDGET);
      }
    }
    settings
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;
  use crate::home::AtlasHome;

  fn hooks(text: &str) -> Result<HookSettings, String> {
    let dir = tempfile::tempdir().unwrap();
    let home = AtlasHome::rooted_at(dir.path());
    fs::create_dir_all(home.config_dir()).unwrap();
    fs::write(home.config_path(), text).unwrap();
    UserConfig::load(&home).map(|config| config.hooks()).map_err(|error| error.to_string())
  }

  #[test]
  fn should_default_to_a_seed_of_299_tokens() {
    assert_eq!(hooks(""), Ok(HookSettings { seed_budget: 299 }));
  }

  #[test]
  fn should_read_the_seed_budget_and_reject_one_out_of_range() {
    assert_eq!(hooks("[hooks]\nseed_budget = 150\n"), Ok(HookSettings { seed_budget: 150 }));
    assert_eq!(hooks("[hooks]\nseed_budget = 1200\n"), Ok(HookSettings { seed_budget: 1_200 }));
    let error = hooks("[hooks]\nseed_budget = 50000\n").unwrap_err();
    assert!(error.ends_with("hooks.seed_budget: expected 150 to 2000 tokens"), "{error}");
  }
}
