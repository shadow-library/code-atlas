use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const SCRIPT: &str = include_str!("../../../scripts/check-denylist.sh");

fn git(dir: &Path, args: &[&str]) {
  let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
  assert!(status.success());
}

fn repo(files: &[(&str, &str)]) -> TempDir {
  let dir = TempDir::new().unwrap();
  git(dir.path(), &["init", "-q"]);
  fs::create_dir_all(dir.path().join("scripts")).unwrap();
  fs::write(dir.path().join("scripts/check-denylist.sh"), SCRIPT).unwrap();
  for (path, content) in files {
    let target = dir.path().join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
  }
  git(dir.path(), &["add", "-f", "."]);
  dir
}

fn run(dir: &TempDir, denylist: Option<&str>) -> Output {
  let mut command = Command::new("bash");
  command.arg("scripts/check-denylist.sh").current_dir(dir.path()).env_remove("ATLAS_DENYLIST");
  let list_dir = TempDir::new().unwrap();
  if let Some(content) = denylist {
    let list = list_dir.path().join("denylist.txt");
    fs::write(&list, content).unwrap();
    command.env("ATLAS_DENYLIST", list);
  }
  command.output().unwrap()
}

#[test]
fn should_pass_silently_when_the_denylist_file_is_absent() {
  let dir = repo(&[("src/a.txt", "needle\n")]);
  let output = run(&dir, None);
  assert_eq!(output.status.code(), Some(0));
  assert!(output.stdout.is_empty() && output.stderr.is_empty());
}

#[test]
fn should_pass_when_the_list_holds_only_comments_and_blank_lines() {
  let dir = repo(&[("src/a.txt", "needle\n")]);
  let output = run(&dir, Some("# needle\n\n   \n"));
  assert_eq!(output.status.code(), Some(0));
  assert!(output.stderr.is_empty());
}

#[test]
fn should_report_only_path_and_line_of_a_case_insensitive_match() {
  let dir = repo(&[("src/a.txt", "ok\nhas a NeEdLe inside\n"), ("src/b.txt", "clean\n")]);
  let output = run(&dir, Some("# comment\n\nneedle\n"));
  assert_eq!(output.status.code(), Some(1));
  assert_eq!(String::from_utf8_lossy(&output.stderr), "tracked files contain denylisted terms at:\nsrc/a.txt:2\n");
  assert!(output.stdout.is_empty());
}

#[test]
fn should_match_terms_as_fixed_strings() {
  let dir = repo(&[("src/a.txt", "abc\na.c\n")]);
  let output = run(&dir, Some("a.c\n"));
  assert_eq!(String::from_utf8_lossy(&output.stderr), "tracked files contain denylisted terms at:\nsrc/a.txt:2\n");
}

#[test]
fn should_skip_the_private_and_scratch_pad_directories() {
  let dir = repo(&[("eval/private/denylist.txt", "needle\n"), ("scratch-pad/notes.md", "needle\n"), ("src/a.txt", "clean\n")]);
  let output = run(&dir, Some("needle\n"));
  assert_eq!(output.status.code(), Some(0));
}

#[test]
fn should_read_the_default_list_inside_the_repository_when_no_override_is_set() {
  let dir = repo(&[("eval/private/denylist.txt", "needle\n"), ("src/a.txt", "needle\n")]);
  let output = run(&dir, None);
  assert_eq!(output.status.code(), Some(1));
  assert_eq!(String::from_utf8_lossy(&output.stderr), "tracked files contain denylisted terms at:\nsrc/a.txt:1\n");
}

#[test]
fn should_find_the_default_list_of_the_main_checkout_from_a_linked_worktree() {
  let dir = repo(&[("eval/private/denylist.txt", "needle\n"), ("src/a.txt", "needle\n")]);
  git(dir.path(), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
  let linked = TempDir::new().unwrap();
  let worktree = linked.path().join("wt");
  git(dir.path(), &["worktree", "add", "-q", worktree.to_str().unwrap()]);
  fs::remove_dir_all(worktree.join("eval/private")).ok();
  let output = Command::new("bash")
    .arg("scripts/check-denylist.sh")
    .current_dir(&worktree)
    .env_remove("ATLAS_DENYLIST")
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(1));
  assert_eq!(String::from_utf8_lossy(&output.stderr), "tracked files contain denylisted terms at:\nsrc/a.txt:1\n");
}
