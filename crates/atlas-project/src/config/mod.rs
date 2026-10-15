mod agents;
mod env;
mod families;
mod hooks;
mod keys;
mod layers;
mod project;
mod user;
mod worktrees;

pub use agents::{AgentsSettings, ClaudeAgent, ClaudeHook, ClaudeScope, CodexAgent, GuidanceMode, HooksSource};
pub use env::ConfigEnv;
pub use hooks::HookSettings;
pub use keys::{KeyClass, McpSettings, ToolsSetting};
pub use layers::{ConfigEntry, McpFlags, Origin, ResolvedMcp, resolve_mcp};
pub use user::UserConfig;
pub use worktrees::{WorktreeMode, WorktreeSettings};

pub(crate) use keys::{Keyed, PROJECT_KEYS, as_words, flatten};
pub(crate) use project::{ProjectView, default_entries, inherited_settings, resolve_project};
