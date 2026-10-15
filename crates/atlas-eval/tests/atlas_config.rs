use std::fs;
use std::path::Path;
use std::time::Duration;

use atlas_eval::agent::{AgentError, AtlasConfig};
use atlas_test_lsp::hermetic::{catl_binary as catl, hermetic_catl};

fn write(path: &Path, content: &str) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, content).unwrap();
}

fn config_dir(family: &str) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  write(&dir.path().join("config.toml"), "[defaults.detect]\nroute_families = [\"acme-http\"]\n");
  write(&dir.path().join("detectors/acme-http.toml"), family);
  dir
}

const TIMEOUT: Duration = Duration::from_secs(60);
const VALID_FAMILY: &str = "version = 1\n[client]\nfiles = [\"src/*.ts\"]\nmodules = [\"@acme/client\"]\n";

#[test]
fn should_land_the_files_where_catl_reads_them_and_list_the_enabled_family() {
  let source = config_dir(VALID_FAMILY);
  let home = tempfile::tempdir().unwrap();
  AtlasConfig::load(source.path()).unwrap().apply(home.path()).unwrap();

  let output = hermetic_catl(catl(), home.path()).args(["config", "show"]).output().unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "detect.route_families = [\"acme-http\"]");
  assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn should_show_no_family_when_nothing_was_applied() {
  let home = tempfile::tempdir().unwrap();
  let output = hermetic_catl(catl(), home.path()).args(["config", "show"]).output().unwrap();
  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  assert_eq!(
    (String::from_utf8_lossy(&output.stdout).as_ref(), String::from_utf8_lossy(&output.stderr).as_ref()),
    ("", "every key has its built-in value; pass --effective to list them\n")
  );
}

#[test]
fn should_accept_a_configuration_the_family_registry_loads() {
  let source = config_dir(VALID_FAMILY);
  let scratch = tempfile::tempdir().unwrap();
  AtlasConfig::load(source.path()).unwrap().validate(catl(), scratch.path(), TIMEOUT).unwrap();
}

#[test]
fn should_refuse_a_family_file_the_registry_cannot_load() {
  let source = config_dir("bogus = [\n");
  let scratch = tempfile::tempdir().unwrap();
  let error = AtlasConfig::load(source.path()).unwrap().validate(catl(), scratch.path(), TIMEOUT).unwrap_err();
  assert!(
    matches!(&error, AgentError::AtlasConfigRejected { reason, .. } if reason.contains("acme-http.toml")),
    "{error}"
  );
}

#[test]
fn should_refuse_an_enabled_family_without_a_definition() {
  let source = tempfile::tempdir().unwrap();
  write(&source.path().join("config.toml"), "[defaults.detect]\nroute_families = [\"acme-http\"]\n");
  let scratch = tempfile::tempdir().unwrap();
  let error = AtlasConfig::load(source.path()).unwrap().validate(catl(), scratch.path(), TIMEOUT).unwrap_err();
  assert!(
    matches!(&error, AgentError::AtlasConfigRejected { reason, .. } if reason.contains("does not exist")),
    "{error}"
  );
}

#[test]
fn should_refuse_a_configuration_that_stops_the_mcp_server_from_starting() {
  let source = tempfile::tempdir().unwrap();
  write(&source.path().join("config.toml"), "[mcp]\ntools = [\"atlas_bogus\"]\n");
  let scratch = tempfile::tempdir().unwrap();
  let error = AtlasConfig::load(source.path()).unwrap().validate(catl(), scratch.path(), TIMEOUT).unwrap_err();
  assert!(
    matches!(&error, AgentError::AtlasConfigRejected { reason, .. } if reason.contains("FAIL") && reason.contains("atlas_bogus")),
    "{error}"
  );
}
