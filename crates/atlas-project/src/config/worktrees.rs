use std::time::Duration;

use super::user::UserConfig;

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const DEFAULT_GC_AFTER_DAYS: u64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeMode {
  Overlay,
  Main,
  Off,
}

impl WorktreeMode {
  pub fn name(self) -> &'static str {
    match self {
      Self::Overlay => "overlay",
      Self::Main => "main",
      Self::Off => "off",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeSettings {
  pub mode: WorktreeMode,
  pub gc_after_days: u64,
}

impl WorktreeSettings {
  pub fn unused_limit(&self) -> Duration {
    Duration::from_secs(self.gc_after_days.saturating_mul(SECONDS_PER_DAY))
  }
}

impl Default for WorktreeSettings {
  fn default() -> Self {
    Self {
      mode: WorktreeMode::Overlay,
      gc_after_days: DEFAULT_GC_AFTER_DAYS,
    }
  }
}

impl UserConfig {
  pub fn worktrees(&self) -> WorktreeSettings {
    let mut settings = WorktreeSettings::default();
    for entry in self.reserved_entries() {
      let Some(value) = entry.value.as_ref() else { continue };
      match (entry.key.as_str(), value.as_str()) {
        ("worktrees.mode", Some("overlay")) => settings.mode = WorktreeMode::Overlay,
        ("worktrees.mode", Some("main")) => settings.mode = WorktreeMode::Main,
        ("worktrees.mode", Some("off")) => settings.mode = WorktreeMode::Off,
        ("worktrees.gc_after_days", None) => settings.gc_after_days = value.as_integer().and_then(|days| u64::try_from(days).ok()).unwrap_or(settings.gc_after_days),
        _ => {}
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

  fn worktrees(text: &str) -> WorktreeSettings {
    let dir = tempfile::tempdir().unwrap();
    let home = AtlasHome::rooted_at(dir.path());
    fs::create_dir_all(home.config_dir()).unwrap();
    fs::write(home.config_path(), text).unwrap();
    UserConfig::load(&home).unwrap().worktrees()
  }

  #[test]
  fn should_default_to_overlays_collected_after_two_weeks() {
    assert_eq!(
      worktrees(""),
      WorktreeSettings {
        mode: WorktreeMode::Overlay,
        gc_after_days: 14
      }
    );
    assert_eq!(WorktreeSettings::default().unused_limit(), Duration::from_secs(14 * 86_400));
  }

  #[test]
  fn should_read_the_worktrees_table_of_config_toml() {
    assert_eq!(
      worktrees("[worktrees]\nmode = \"main\"\ngc_after_days = 3\n"),
      WorktreeSettings {
        mode: WorktreeMode::Main,
        gc_after_days: 3
      }
    );
    assert_eq!(worktrees("[worktrees]\nmode = \"off\"\n").mode, WorktreeMode::Off);
  }
}
