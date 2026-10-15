use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use atlas_mcp::guidance::SERVER_NAME;
use serde_json::{Value, json};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table};

use super::claude::HOME_VAR;

const CODEX_HOME_VAR: &str = "CODEX_HOME";
const CONFIG_FILE: &str = "config.toml";
const SERVERS: &str = "mcp_servers";

pub(super) fn codex_home() -> Option<PathBuf> {
  let non_empty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
  non_empty(CODEX_HOME_VAR).or_else(|| non_empty("HOME").map(|home| home.join(".codex")))
}

pub(super) fn config_path(home: &Path) -> PathBuf {
  home.join(CONFIG_FILE)
}

pub(super) fn server_entry(command: &str, home: Option<&OsStr>) -> Value {
  let mut entry = json!({ "command": command, "args": ["mcp", "--watch"] });
  if let Some(home) = home {
    entry["env"] = json!({ HOME_VAR: home.to_string_lossy() });
  }
  entry
}

pub(super) fn entry(text: &str) -> Result<Option<Value>, String> {
  let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
  let Some(servers) = table.get(SERVERS) else { return Ok(None) };
  let servers = servers.as_table().ok_or_else(|| format!("{SERVERS} is not a table"))?;
  servers
    .get(SERVER_NAME)
    .map(|entry| serde_json::to_value(entry).map_err(|error| error.to_string()))
    .transpose()
}

pub(super) fn upsert(text: &str, entry: &Value) -> Result<String, String> {
  let mut document: DocumentMut = text.parse().map_err(|error: toml_edit::TomlError| error.to_string())?;
  if !document.contains_key(SERVERS) {
    let mut servers = Table::new();
    servers.set_implicit(true);
    document.insert(SERVERS, Item::Table(servers));
  }
  let servers = document[SERVERS]
    .as_table_mut()
    .ok_or_else(|| format!("{SERVERS} is not a table; edit {SERVER_NAME} by hand"))?;
  if !servers.contains_key(SERVER_NAME) {
    servers.insert(SERVER_NAME, Item::Table(Table::new()));
  }
  let server = servers[SERVER_NAME]
    .as_table_like_mut()
    .ok_or_else(|| format!("{SERVERS}.{SERVER_NAME} is not a table; edit it by hand"))?;
  let command = entry.get("command").and_then(Value::as_str).unwrap_or_default();
  server.insert("command", toml_edit::value(command));
  let args: Array = entry.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
  server.insert("args", toml_edit::value(args));
  match entry.get("env").and_then(Value::as_object) {
    Some(env) => {
      let mut table = InlineTable::new();
      for (name, value) in env {
        table.insert(name, value.as_str().unwrap_or_default().into());
      }
      server.insert("env", toml_edit::value(table));
    }
    None => {
      server.remove("env");
    }
  }
  Ok(document.to_string())
}

pub(super) fn remove(text: &str) -> Result<String, String> {
  let mut document: DocumentMut = text.parse().map_err(|error: toml_edit::TomlError| error.to_string())?;
  let Some(servers) = document.get_mut(SERVERS).and_then(Item::as_table_like_mut) else {
    return Ok(text.to_owned());
  };
  servers.remove(SERVER_NAME);
  if servers.is_empty() {
    document.remove(SERVERS);
  }
  Ok(document.to_string())
}

pub(super) fn hollow(text: &str) -> bool {
  toml::from_str::<toml::Table>(text).is_ok_and(|table| table.is_empty())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_add_the_server_table_and_keep_every_other_line_and_comment() {
    let before = "# my codex config\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n";
    let after = upsert(before, &server_entry("catl", Some(OsStr::new("/iso")))).unwrap();
    assert_eq!(
      after,
      "# my codex config\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n\n[mcp_servers.code-atlas]\ncommand = \"catl\"\nargs = [\"mcp\", \"--watch\"]\nenv = { CODE_ATLAS_HOME = \"/iso\" }\n"
    );
    assert_eq!(entry(&after).unwrap(), Some(server_entry("catl", Some(OsStr::new("/iso")))));
    assert_eq!(
      remove(&after).unwrap(),
      "# my codex config\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n"
    );
  }

  #[test]
  fn should_update_in_place_keeping_keys_the_user_added_and_drop_env_when_unset() {
    let before = "[mcp_servers.code-atlas]\ncommand = \"/old/catl\"\nargs = [\"mcp\", \"--project\", \"acme\"]\nstartup_timeout_sec = 30\nenv = { CODE_ATLAS_HOME = \"/x\" }\n";
    let after = upsert(before, &server_entry("catl", None)).unwrap();
    assert_eq!(
      after,
      "[mcp_servers.code-atlas]\ncommand = \"catl\"\nargs = [\"mcp\", \"--watch\"]\nstartup_timeout_sec = 30\n"
    );
  }

  #[test]
  fn should_create_a_file_with_only_the_server_and_find_it_hollow_once_removed() {
    let created = upsert("", &server_entry("catl", None)).unwrap();
    assert_eq!(created, "[mcp_servers.code-atlas]\ncommand = \"catl\"\nargs = [\"mcp\", \"--watch\"]\n");
    assert!(hollow(&remove(&created).unwrap()));
  }

  #[test]
  fn should_report_a_broken_file_instead_of_rewriting_it() {
    assert!(entry("[mcp_servers\n").is_err());
    assert!(upsert("[mcp_servers\n", &server_entry("catl", None)).is_err());
  }
}
