use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const SYSTEM_PATH: &str = "/usr/bin:/bin";
const CLEARED_VARS: [&str; 5] = ["RUST_LOG", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "CLAUDE_CONFIG_DIR", "CODEX_HOME"];

pub fn fake_bin(atlas_home: &Path) -> PathBuf {
  atlas_home.join("fake-bin")
}

pub fn user_home(atlas_home: &Path) -> PathBuf {
  atlas_home.join("user-home")
}

pub fn claude_config_dir(atlas_home: &Path) -> PathBuf {
  atlas_home.join("claude")
}

pub fn codex_home(atlas_home: &Path) -> PathBuf {
  atlas_home.join("codex")
}

pub fn sandbox_path(atlas_home: &Path) -> String {
  format!("{}:{SYSTEM_PATH}", fake_bin(atlas_home).display())
}

pub fn hermetic_catl(binary: impl AsRef<OsStr>, atlas_home: &Path) -> Command {
  let real_home = std::env::var_os("HOME").map(PathBuf::from);
  let toolchain = |var: &str, default: &str| std::env::var_os(var).map(PathBuf::from).or_else(|| real_home.as_ref().map(|home| home.join(default)));
  let rustup_home = toolchain("RUSTUP_HOME", ".rustup");
  let cargo_home = toolchain("CARGO_HOME", ".cargo");
  let mut command = Command::new(binary);
  for (name, _) in std::env::vars_os() {
    if name.to_string_lossy().starts_with("CODE_ATLAS_") {
      command.env_remove(name);
    }
  }
  for name in CLEARED_VARS {
    command.env_remove(name);
  }
  for directory in [fake_bin(atlas_home), user_home(atlas_home), claude_config_dir(atlas_home), codex_home(atlas_home)] {
    std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("cannot create {}: {error}", directory.display()));
  }
  command
    .env("CODE_ATLAS_HOME", atlas_home)
    .env("HOME", user_home(atlas_home))
    .env("CLAUDE_CONFIG_DIR", claude_config_dir(atlas_home))
    .env("CODEX_HOME", codex_home(atlas_home))
    .env("PATH", sandbox_path(atlas_home));
  if let Some(rustup_home) = rustup_home {
    command.env("RUSTUP_HOME", rustup_home);
  }
  if let Some(cargo_home) = cargo_home {
    command.env("CARGO_HOME", cargo_home);
  }
  command
}

// The binary lives in atlas-cli, so Cargo gives other crates no CARGO_BIN_EXE_catl; build it into the caller's target dir and profile.
pub fn catl_binary() -> &'static Path {
  static BINARY: OnceLock<PathBuf> = OnceLock::new();
  BINARY.get_or_init(|| {
    if let Some(path) = std::env::var_os("CODE_ATLAS_BIN") {
      return PathBuf::from(path);
    }
    let test_exe = std::env::current_exe().unwrap_or_else(|error| panic!("cannot locate the running test binary: {error}"));
    let Some(profile_dir) = test_exe.parent().and_then(Path::parent).map(Path::to_path_buf) else {
      panic!("{} is not inside a Cargo target directory", test_exe.display());
    };
    let Some(target_dir) = profile_dir.parent() else {
      panic!("{} is not inside a Cargo target directory", profile_dir.display());
    };
    let profile = match profile_dir.file_name().and_then(OsStr::to_str) {
      Some("debug") | None => "dev".to_owned(),
      Some(other) => other.to_owned(),
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
      .args(["build", "--quiet", "-p", "atlas-cli", "--bin", "catl", "--profile", &profile, "--target-dir"])
      .arg(target_dir)
      .current_dir(env!("CARGO_MANIFEST_DIR"))
      .status()
      .unwrap_or_else(|error| panic!("cannot run cargo to build catl: {error}"));
    assert!(status.success(), "building catl failed: {status}");
    let binary = profile_dir.join(format!("catl{}", std::env::consts::EXE_SUFFIX));
    std::fs::canonicalize(&binary).unwrap_or_else(|error| panic!("{} was not built: {error}", binary.display()))
  })
}
