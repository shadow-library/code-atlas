use std::fs;
use std::path::Path;
use std::time::Duration;

use atlas_eval::agent::{AgentError, AtlasConfig, atlas_config_mismatch, load_tasks};
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

fn task_declaring_config(family: &str) -> (tempfile::TempDir, atlas_eval::agent::LoadedTask) {
  let tasks = tempfile::tempdir().unwrap();
  let task = tasks.path().join("t90-configured");
  write(
    &task.join("task.toml"),
    "id = \"t90-configured\"\ntitle = \"t\"\nrepo = { fixture = \"acme\" }\nbase_commit = \"e5209b4c5c99df0e0e5cdab2bd885b0821b91927\"\nprompt = \"p\"\ntimeout_minutes = 1\nmax_turns = 1\natlas_config = \"atlas-config\"\n",
  );
  write(&task.join("atlas-config/config.toml"), "[defaults.detect]\nroute_families = [\"acme-http\"]\n");
  write(&task.join("atlas-config/detectors/acme-http.toml"), family);
  let loaded = load_tasks(tasks.path(), &[]).unwrap().remove(0);
  (tasks, loaded)
}

#[test]
fn should_warn_when_a_run_seeds_no_configuration_or_another_one_than_the_task_declares() {
  let (tasks, task) = task_declaring_config(VALID_FAMILY);
  let declared = tasks.path().join("t90-configured/atlas-config");
  let other = config_dir("version = 1\n[client]\nfiles = [\"lib/*.ts\"]\nmodules = [\"@acme/other\"]\n");

  let missing = atlas_config_mismatch(&task, None).unwrap();
  let same = atlas_config_mismatch(&task, Some(&AtlasConfig::load(&declared).unwrap())).unwrap();
  let differing = atlas_config_mismatch(&task, Some(&AtlasConfig::load(other.path()).unwrap())).unwrap();

  assert_eq!(
    missing,
    Some(format!(
      "task t90-configured declares the Code Atlas configuration {}, but this run seeds none; pass --atlas-config {}",
      declared.display(),
      declared.display()
    ))
  );
  assert_eq!(same, None);
  assert!(differing.unwrap().starts_with("task t90-configured declares the Code Atlas configuration "));
}

#[test]
fn should_refuse_a_task_configuration_outside_its_directory() {
  let tasks = tempfile::tempdir().unwrap();
  write(
    &tasks.path().join("t91-escape/task.toml"),
    "id = \"t91-escape\"\ntitle = \"t\"\nrepo = { fixture = \"acme\" }\nbase_commit = \"e5209b4c5c99df0e0e5cdab2bd885b0821b91927\"\nprompt = \"p\"\ntimeout_minutes = 1\nmax_turns = 1\natlas_config = \"../config\"\n",
  );

  let error = load_tasks(tasks.path(), &[]).unwrap_err();

  assert!(
    error
      .to_string()
      .contains("atlas_config `../config` must be a relative path inside the task directory, outside hidden/"),
    "{error}"
  );
}
