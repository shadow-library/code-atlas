use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub mod hermetic;

pub const FAKE_LSP_BIN: &str = "atlas-fake-lsp";

// Cargo builds no binaries of dependencies and sets CARGO_BIN_EXE_* only for the defining package's tests, so build on first use into the caller's target dir.
pub fn fake_lsp_path() -> &'static Path {
  static PATH: OnceLock<PathBuf> = OnceLock::new();
  PATH.get_or_init(build_fake_lsp)
}

fn build_fake_lsp() -> PathBuf {
  let test_exe = std::env::current_exe().unwrap_or_else(|error| panic!("cannot locate the running test binary: {error}"));
  let Some(profile_dir) = test_exe.parent().and_then(Path::parent) else {
    panic!("{} is not inside a Cargo target directory", test_exe.display());
  };
  let Some(target_dir) = profile_dir.parent() else {
    panic!("{} is not inside a Cargo target directory", profile_dir.display());
  };
  let profile = match profile_dir.file_name().and_then(OsStr::to_str) {
    Some("debug") | None => "dev",
    Some(other) => other,
  };
  let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
  let status = Command::new(cargo)
    .args([
      "build",
      "--quiet",
      "-p",
      env!("CARGO_PKG_NAME"),
      "--bin",
      FAKE_LSP_BIN,
      "--profile",
      profile,
      "--target-dir",
    ])
    .arg(target_dir)
    .current_dir(env!("CARGO_MANIFEST_DIR"))
    .status()
    .unwrap_or_else(|error| panic!("cannot run cargo to build {FAKE_LSP_BIN}: {error}"));
  assert!(status.success(), "building {FAKE_LSP_BIN} failed: {status}");
  let binary = profile_dir.join(format!("{FAKE_LSP_BIN}{}", std::env::consts::EXE_SUFFIX));
  std::fs::canonicalize(&binary).unwrap_or_else(|error| panic!("{} was not built: {error}", binary.display()))
}
