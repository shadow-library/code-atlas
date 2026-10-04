use std::path::Path;
use std::process::Command;

const UNKNOWN: &str = "unknown";

fn git(args: &[&str]) -> Option<String> {
  let output = Command::new("git").args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()?;
  output
    .status
    .success()
    .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    .filter(|text| !text.is_empty())
}

fn watch(git_path: &str) {
  if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", git_path])
    && Path::new(&path).exists()
  {
    println!("cargo::rerun-if-changed={path}");
  }
}

fn main() {
  let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| UNKNOWN.to_owned());
  println!("cargo::rustc-env=CODE_ATLAS_BUILD_COMMIT={commit}");
  watch("HEAD");
  watch("packed-refs");
  if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
    watch(&reference);
  }
}
