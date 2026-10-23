use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use atlas_mcp::guidance::SERVER_NAME;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::claude::{self, HookEvent, RegistrationScope};
use super::{codex, guidance};
use crate::json_edit::{JsonEditError, remove_member, upsert_server};

const SERVERS_KEY: &str = "mcpServers";
const REMIND_HOOKS: Slot = Slot {
  outer: "hooks",
  inner: "PreToolUse",
  ours: claude::is_our_remind_group,
};
const SEED_HOOKS: Slot = Slot {
  outer: "hooks",
  inner: "UserPromptSubmit",
  ours: claude::is_our_seed_group,
};
const COUNTERPART_HOOKS: Slot = Slot {
  outer: "hooks",
  inner: "PostToolUse",
  ours: claude::is_our_counterpart_group,
};
const NOTES_HOOKS: Slot = Slot {
  outer: "hooks",
  inner: "PostToolUse",
  ours: claude::is_our_notes_group,
};
const PERMISSIONS: Slot = Slot {
  outer: "permissions",
  inner: "allow",
  ours: claude::is_our_permission,
};

struct Slot {
  outer: &'static str,
  inner: &'static str,
  ours: fn(&Value) -> bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Target {
  Registration {
    scope: RegistrationScope,
    config: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
  },
  McpJson {
    path: PathBuf,
  },
  Hooks {
    path: PathBuf,
    #[serde(default, skip_serializing_if = "HookEvent::is_pre_tool_use")]
    event: HookEvent,
  },
  Permissions {
    path: PathBuf,
  },
  Codex {
    path: PathBuf,
  },
  Guidance {
    path: PathBuf,
  },
  Exclude {
    path: PathBuf,
    line: String,
  },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Content {
  Server(Value),
  Entries(Vec<Value>),
  Block(String),
  Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Current {
  Absent,
  Present { hash: String, legacy: bool },
  Unreadable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Effect {
  Write {
    path: PathBuf,
    before: Option<String>,
    after: String,
  },
  Delete {
    path: PathBuf,
    before: String,
  },
  Register {
    scope: RegistrationScope,
    cwd: Option<PathBuf>,
    before: Option<Value>,
    entry: Value,
  },
  Unregister {
    scope: RegistrationScope,
    cwd: Option<PathBuf>,
    before: Option<Value>,
  },
}

impl Target {
  pub(super) fn label(&self) -> String {
    match self {
      Self::Registration { config, cwd: None, .. } => format!("{}#{SERVERS_KEY}.{SERVER_NAME}", config.display()),
      Self::Registration { config, cwd: Some(cwd), .. } => format!("{}#projects[{}].{SERVERS_KEY}.{SERVER_NAME}", config.display(), cwd.display()),
      Self::McpJson { path } => format!("{}#{SERVERS_KEY}.{SERVER_NAME}", path.display()),
      Self::Hooks { path, .. } | Self::Permissions { path } => format!("{}#{}.{}", path.display(), self.slot().outer, self.slot().inner),
      Self::Codex { path } => format!("{}#mcp_servers.{SERVER_NAME}", path.display()),
      Self::Guidance { path } => path.display().to_string(),
      Self::Exclude { path, line } => format!("{} ({line})", path.display()),
    }
  }

  pub(super) fn file(&self) -> &Path {
    match self {
      Self::Registration { config, .. } => config,
      Self::McpJson { path } | Self::Hooks { path, .. } | Self::Permissions { path } | Self::Codex { path } | Self::Guidance { path } | Self::Exclude { path, .. } => path,
    }
  }

  pub(super) fn adopts_identical(&self) -> bool {
    !matches!(self, Self::Exclude { .. })
  }

  pub(super) fn asks_before_touching_an_existing_file(&self) -> bool {
    !matches!(self, Self::Registration { .. } | Self::Exclude { .. })
  }

  pub(super) fn hash(&self, content: &Content) -> String {
    match (self, content) {
      (Self::Exclude { line, .. }, _) => digest(line),
      (_, Content::Server(entry)) => digest(&canonical(&server_view(entry))),
      (_, Content::Entries(entries)) => digest(&canonical(&Value::Array(entries.clone()))),
      (_, Content::Block(block)) => digest(block),
      (_, Content::Line) => digest(""),
    }
  }

  pub(super) fn current(&self) -> Current {
    let text = match read(self.file()) {
      Ok(Some(text)) => text,
      Ok(None) => return Current::Absent,
      Err(reason) => return Current::Unreadable(reason),
    };
    match self.fragment(&text) {
      Ok(Some((content, legacy))) => Current::Present {
        hash: self.hash(&content),
        legacy,
      },
      Ok(None) => Current::Absent,
      Err(reason) => Current::Unreadable(reason),
    }
  }

  fn fragment(&self, text: &str) -> Result<Option<(Content, bool)>, String> {
    match self {
      Self::Registration { cwd, .. } => {
        let document = parse_json(text)?;
        let servers = match cwd {
          None => document.get(SERVERS_KEY),
          Some(cwd) => document
            .get("projects")
            .and_then(|projects| projects.get(cwd.to_string_lossy().as_ref()))
            .and_then(|project| project.get(SERVERS_KEY)),
        };
        Ok(
          servers
            .and_then(|servers| servers.get(SERVER_NAME))
            .map(|entry| (Content::Server(entry.clone()), is_legacy(entry))),
        )
      }
      Self::McpJson { .. } => {
        let document = parse_json(text)?;
        Ok(
          document
            .get(SERVERS_KEY)
            .and_then(|servers| servers.get(SERVER_NAME))
            .map(|entry| (Content::Server(entry.clone()), is_legacy(entry))),
        )
      }
      Self::Hooks { .. } | Self::Permissions { .. } => {
        let (_, ours) = self.slot().split(&parse_json(text)?)?;
        Ok((!ours.is_empty()).then_some((Content::Entries(ours), false)))
      }
      Self::Codex { .. } => Ok(codex::entry(text)?.map(|entry| {
        let legacy = is_legacy(&entry);
        (Content::Server(entry), legacy)
      })),
      Self::Guidance { .. } => Ok(guidance::find_block(text)?.map(|block| (Content::Block(block.to_owned()), true))),
      Self::Exclude { line, .. } => Ok(text.lines().any(|existing| existing.trim_end() == line).then_some((Content::Line, false))),
    }
  }

  pub(super) fn write(&self, content: &Content) -> Result<Effect, String> {
    let before = read(self.file())?;
    let path = self.file().to_path_buf();
    let after = match (self, content) {
      (Self::Registration { scope, cwd, .. }, Content::Server(entry)) => {
        let before = before
          .as_deref()
          .and_then(|text| self.fragment(text).ok().flatten())
          .and_then(|(content, _)| match content {
            Content::Server(entry) => Some(entry),
            _ => None,
          });
        return Ok(Effect::Register {
          scope: *scope,
          cwd: cwd.clone(),
          before,
          entry: entry.clone(),
        });
      }
      (Self::McpJson { .. }, Content::Server(entry)) => upsert_json(before.as_deref(), &[SERVERS_KEY], SERVER_NAME, entry)?,
      (Self::Hooks { .. } | Self::Permissions { .. }, Content::Entries(entries)) => {
        let slot = self.slot();
        let others = match before.as_deref().filter(|text| !text.trim().is_empty()) {
          Some(text) => slot.split(&parse_json(text)?)?.0,
          None => Vec::new(),
        };
        let mut all = others;
        all.extend(entries.iter().cloned());
        upsert_json(before.as_deref(), &[slot.outer], slot.inner, &Value::Array(all))?
      }
      (Self::Codex { .. }, Content::Server(entry)) => codex::upsert(before.as_deref().unwrap_or(""), entry)?,
      (Self::Guidance { .. }, Content::Block(block)) => match before.as_deref() {
        Some(text) => guidance::upsert_block(text, block)?,
        None => format!("{block}\n"),
      },
      (Self::Exclude { line, .. }, Content::Line) => {
        let mut text = before.clone().unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
          text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
        text
      }
      _ => return Err("this kind of content does not belong in this file".to_owned()),
    };
    Ok(Effect::Write { path, before, after })
  }

  pub(super) fn remove(&self, created_file: bool) -> Result<Option<Effect>, String> {
    let Some(before) = read(self.file())? else { return Ok(None) };
    let path = self.file().to_path_buf();
    let after = match self {
      Self::Registration { scope, cwd, .. } => {
        let before = self.fragment(&before).ok().flatten().and_then(|(content, _)| match content {
          Content::Server(entry) => Some(entry),
          _ => None,
        });
        return Ok(Some(Effect::Unregister {
          scope: *scope,
          cwd: cwd.clone(),
          before,
        }));
      }
      Self::McpJson { .. } => json_edit(remove_member(&before, &[SERVERS_KEY], SERVER_NAME))?,
      Self::Hooks { .. } | Self::Permissions { .. } => {
        let slot = self.slot();
        let (others, _) = slot.split(&parse_json(&before)?)?;
        match others.is_empty() {
          true => json_edit(remove_member(&before, &[slot.outer], slot.inner))?.map(|text| drop_empty_object(&text, slot.outer)),
          false => json_edit(upsert_server(&before, &[slot.outer], slot.inner, &Value::Array(others)))?,
        }
      }
      Self::Codex { .. } => Some(codex::remove(&before)?),
      Self::Guidance { .. } => guidance::remove_block(&before)?,
      Self::Exclude { line, .. } => remove_line(&before, line),
    };
    let Some(after) = after else { return Ok(None) };
    Ok(Some(match created_file && self.hollow(&after) {
      true => Effect::Delete { path, before },
      false => Effect::Write {
        path,
        before: Some(before),
        after,
      },
    }))
  }

  fn slot(&self) -> &'static Slot {
    match self {
      Self::Permissions { .. } => &PERMISSIONS,
      Self::Hooks {
        event: HookEvent::UserPromptSubmit,
        ..
      } => &SEED_HOOKS,
      Self::Hooks {
        event: HookEvent::PostToolUse, ..
      } => &COUNTERPART_HOOKS,
      Self::Hooks {
        event: HookEvent::PostToolUseNotes,
        ..
      } => &NOTES_HOOKS,
      _ => &REMIND_HOOKS,
    }
  }

  fn hollow(&self, text: &str) -> bool {
    match self {
      Self::McpJson { .. } | Self::Hooks { .. } | Self::Permissions { .. } => serde_json::from_str::<Value>(text).is_ok_and(|value| hollow_json(&value)),
      Self::Codex { .. } => codex::hollow(text),
      Self::Guidance { .. } | Self::Exclude { .. } => text.trim().is_empty(),
      Self::Registration { .. } => false,
    }
  }
}

pub(super) fn digest(text: &str) -> String {
  format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}

pub(super) fn canonical(value: &Value) -> String {
  match value {
    Value::Object(map) => {
      let mut keys: Vec<&String> = map.keys().collect();
      keys.sort();
      let members: Vec<String> = keys.into_iter().map(|key| format!("{}:{}", Value::String(key.clone()), canonical(&map[key]))).collect();
      format!("{{{}}}", members.join(","))
    }
    Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
    other => other.to_string(),
  }
}

pub(super) fn server_view(entry: &Value) -> Value {
  let env = entry.get("env").filter(|env| env.as_object().is_some_and(|env| !env.is_empty())).cloned();
  json!({ "command": entry.get("command"), "args": entry.get("args"), "env": env })
}

pub(super) fn is_legacy(entry: &Value) -> bool {
  let absolute = entry.get("command").and_then(Value::as_str).is_some_and(|command| Path::new(command).is_absolute());
  let pinned = entry.get("args").and_then(Value::as_array).is_some_and(|args| args.iter().any(|arg| arg == "--project"));
  absolute || pinned
}

pub(super) fn read(path: &Path) -> Result<Option<String>, String> {
  match fs::read_to_string(path) {
    Ok(text) => Ok(Some(text)),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
    Err(error) => Err(format!("cannot read {}: {error}", path.display())),
  }
}

fn parse_json(text: &str) -> Result<Value, String> {
  if text.trim().is_empty() {
    return Ok(json!({}));
  }
  let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
  match value.is_object() {
    true => Ok(value),
    false => Err("the top level is not an object".to_owned()),
  }
}

fn upsert_json(before: Option<&str>, path: &[&str], key: &str, value: &Value) -> Result<String, String> {
  let Some(text) = before.filter(|text| !text.trim().is_empty()) else {
    let mut nested = json!({ key: value });
    for name in path.iter().rev() {
      nested = json!({ *name: nested });
    }
    return Ok(serde_json::to_string_pretty(&nested).map_err(|error| error.to_string())? + "\n");
  };
  parse_json(text)?;
  Ok(json_edit(upsert_server(text, path, key, value))?.unwrap_or_else(|| text.to_owned()))
}

fn json_edit(result: Result<Option<String>, JsonEditError>) -> Result<Option<String>, String> {
  result.map_err(|JsonEditError::Unsupported(reason)| reason.to_owned())
}

fn drop_empty_object(text: &str, key: &str) -> String {
  let empty = serde_json::from_str::<Value>(text).is_ok_and(|value| value.get(key).is_some_and(|inner| inner.as_object().is_some_and(serde_json::Map::is_empty)));
  match empty {
    true => remove_member(text, &[], key).ok().flatten().unwrap_or_else(|| text.to_owned()),
    false => text.to_owned(),
  }
}

fn hollow_json(value: &Value) -> bool {
  value.as_object().is_some_and(|map| map.values().all(hollow_json))
}

fn remove_line(text: &str, line: &str) -> Option<String> {
  let mut removed = false;
  let kept: Vec<&str> = text
    .split_inclusive('\n')
    .filter(|existing| {
      let matches = !removed && existing.trim_end() == line;
      removed |= matches;
      !matches
    })
    .collect();
  removed.then(|| kept.concat())
}

impl Slot {
  fn split(&self, document: &Value) -> Result<(Vec<Value>, Vec<Value>), String> {
    let Some(entries) = document.get(self.outer).and_then(|outer| outer.get(self.inner)) else {
      return Ok((Vec::new(), Vec::new()));
    };
    let entries = entries.as_array().ok_or_else(|| format!("{}.{} is not an array", self.outer, self.inner))?;
    Ok(entries.iter().cloned().partition(|entry| !(self.ours)(entry)))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_hash_servers_by_command_args_and_env_whatever_the_key_order_or_extra_fields() {
    let target = Target::McpJson {
      path: PathBuf::from("/r/.mcp.json"),
    };
    let ours = json!({"type": "stdio", "command": "catl", "args": ["mcp", "--watch"], "env": {}});
    let reordered = json!({"args": ["mcp", "--watch"], "command": "catl"});
    assert_eq!(target.hash(&Content::Server(ours)), target.hash(&Content::Server(reordered)));
    assert_ne!(
      target.hash(&Content::Server(json!({"command": "catl", "args": ["mcp"]}))),
      target.hash(&Content::Server(json!({"command": "catl", "args": ["mcp", "--watch"]})))
    );
  }

  #[test]
  fn should_call_absolute_commands_and_pinned_projects_legacy() {
    assert!(is_legacy(&json!({"command": "/usr/local/bin/catl", "args": ["mcp", "--watch"]})));
    assert!(is_legacy(&json!({"command": "catl", "args": ["mcp", "--project", "acme"]})));
    assert!(!is_legacy(&json!({"command": "catl", "args": ["mcp", "--watch"]})));
  }

  #[test]
  fn should_keep_other_hooks_and_delete_only_a_file_it_created_once_hollow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let target = Target::Hooks {
      path: path.clone(),
      event: HookEvent::PreToolUse,
    };
    let ours = claude::hook_groups(HookEvent::PreToolUse, "catl", None);
    let foreign = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "lint"}]});
    fs::write(
      &path,
      serde_json::to_string_pretty(&json!({"model": "x", "hooks": {"PreToolUse": [foreign.clone()]}})).unwrap(),
    )
    .unwrap();

    let Effect::Write { after, .. } = target.write(&Content::Entries(ours.clone())).unwrap() else {
      panic!("expected a write")
    };
    fs::write(&path, &after).unwrap();
    assert_eq!(
      target.current(),
      Current::Present {
        hash: target.hash(&Content::Entries(ours)),
        legacy: false
      }
    );
    let Some(Effect::Write { after, .. }) = target.remove(true).unwrap() else {
      panic!("expected a write")
    };
    let document: Value = serde_json::from_str(&after).unwrap();
    assert_eq!(document, json!({"model": "x", "hooks": {"PreToolUse": [foreign]}}));

    fs::write(&path, "{\n  \"hooks\": {\n    \"PreToolUse\": []\n  }\n}\n").unwrap();
    let created = target.write(&Content::Entries(claude::hook_groups(HookEvent::PreToolUse, "catl", None))).unwrap();
    let Effect::Write { after, .. } = created else { panic!("expected a write") };
    fs::write(&path, &after).unwrap();
    assert!(matches!(target.remove(true).unwrap(), Some(Effect::Delete { .. })));
  }

  #[test]
  fn should_own_the_seed_group_apart_from_the_remind_group_and_foreign_prompt_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let remind = Target::Hooks {
      path: path.clone(),
      event: HookEvent::PreToolUse,
    };
    let seed = Target::Hooks {
      path: path.clone(),
      event: HookEvent::UserPromptSubmit,
    };
    let foreign = json!({"hooks": [{"type": "command", "command": "log-prompt"}]});
    fs::write(&path, serde_json::to_string_pretty(&json!({"hooks": {"UserPromptSubmit": [foreign.clone()]}})).unwrap()).unwrap();
    for target in [&remind, &seed] {
      let event = if *target == seed { HookEvent::UserPromptSubmit } else { HookEvent::PreToolUse };
      let Effect::Write { after, .. } = target.write(&Content::Entries(claude::hook_groups(event, "catl", None))).unwrap() else {
        panic!("expected a write")
      };
      fs::write(&path, &after).unwrap();
    }
    let seed_groups = claude::hook_groups(HookEvent::UserPromptSubmit, "catl", None);
    assert_eq!(
      seed.current(),
      Current::Present {
        hash: seed.hash(&Content::Entries(seed_groups.clone())),
        legacy: false
      }
    );
    assert_eq!(seed.label(), format!("{}#hooks.UserPromptSubmit", path.display()));
    assert_eq!(remind.label(), format!("{}#hooks.PreToolUse", path.display()));

    let Some(Effect::Write { after, .. }) = seed.remove(false).unwrap() else {
      panic!("expected a write")
    };
    let document: Value = serde_json::from_str(&after).unwrap();
    assert_eq!(
      document,
      json!({"hooks": {"UserPromptSubmit": [foreign], "PreToolUse": claude::hook_groups(HookEvent::PreToolUse, "catl", None)}})
    );
  }

  #[test]
  fn should_read_a_hooks_record_without_an_event_as_the_remind_group_and_write_it_back_unchanged() {
    let path = PathBuf::from("/c/settings.json");
    let recorded = json!({"kind": "hooks", "path": "/c/settings.json"});
    let target: Target = serde_json::from_value(recorded.clone()).unwrap();
    assert_eq!(
      target,
      Target::Hooks {
        path: path.clone(),
        event: HookEvent::PreToolUse
      }
    );
    assert_eq!(serde_json::to_value(&target).unwrap(), recorded);
    let seed = Target::Hooks {
      path,
      event: HookEvent::UserPromptSubmit,
    };
    assert_eq!(
      serde_json::to_value(&seed).unwrap(),
      json!({"kind": "hooks", "path": "/c/settings.json", "event": "UserPromptSubmit"})
    );
  }

  #[test]
  fn should_remove_exactly_one_matching_exclude_line() {
    assert_eq!(remove_line("a\n/CLAUDE.local.md\nb\n", "/CLAUDE.local.md"), Some("a\nb\n".to_owned()));
    assert_eq!(remove_line("a\n", "/CLAUDE.local.md"), None);
  }
}
