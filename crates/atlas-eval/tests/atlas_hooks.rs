use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use atlas_eval::agent::AtlasHooks;
use atlas_test_lsp::hermetic::{catl_binary as catl, hermetic_catl};

const BINARY: &str = "/opt/eval tools/catl";

fn rendered(home: &Path) -> AtlasHooks {
  let output = hermetic_catl(catl(), home)
    .args(["agents", "hooks", "--json", "--binary", BINARY, "--home"])
    .arg(home)
    .output()
    .unwrap();
  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  AtlasHooks::parse(&String::from_utf8_lossy(&output.stdout), BINARY, &home.display().to_string()).unwrap()
}

fn with_hooks(names: &str) -> tempfile::TempDir {
  let home = tempfile::tempdir().unwrap();
  fs::create_dir_all(home.path().join("config")).unwrap();
  fs::write(home.path().join("config/config.toml"), format!("[agents.claude]\nhooks = {names}\n")).unwrap();
  home
}

#[test]
fn should_read_the_seed_hook_catl_installs_by_default() {
  let home = tempfile::tempdir().unwrap();
  let hooks = rendered(home.path());
  assert_eq!(hooks.record().groups, BTreeMap::from([("UserPromptSubmit".to_owned(), 1)]));
  let command = hooks.settings()["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"].as_str().unwrap().to_owned();
  assert_eq!(command, format!("CODE_ATLAS_HOME={} '{BINARY}' hook seed || true", home.path().display()));
  assert_eq!(hooks.settings().as_object().unwrap().keys().collect::<Vec<_>>(), ["hooks"]);
}

#[test]
fn should_digest_the_same_hooks_alike_in_different_homes_and_a_changed_list_differently() {
  let first = with_hooks("[\"remind\", \"seed\"]");
  let second = with_hooks("[\"remind\", \"seed\"]");
  let seed_only = with_hooks("[\"seed\"]");
  let (first, second, seed_only) = (rendered(first.path()), rendered(second.path()), rendered(seed_only.path()));
  assert_eq!(first.record().groups, BTreeMap::from([("PreToolUse".to_owned(), 1), ("UserPromptSubmit".to_owned(), 1)]));
  assert_eq!(first.record(), second.record());
  assert_ne!(first.settings(), second.settings());
  assert_ne!(first.record().blake3, seed_only.record().blake3);
}

#[test]
fn should_refuse_a_home_whose_configuration_installs_no_hook() {
  let home = with_hooks("[\"auto-approve\"]");
  let output = hermetic_catl(catl(), home.path())
    .args(["agents", "hooks", "--json", "--binary", BINARY, "--home"])
    .arg(home.path())
    .output()
    .unwrap();
  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let error = AtlasHooks::parse(&String::from_utf8_lossy(&output.stdout), BINARY, &home.path().display().to_string()).unwrap_err();
  assert_eq!(error, "no hook group, so [agents.claude] hooks of the session home installs none");
}
