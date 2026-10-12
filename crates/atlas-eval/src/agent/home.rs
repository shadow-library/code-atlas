use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::error::{AgentError, AgentResult};
use super::process::{ProcessExit, run_captured};
use super::workspace::git_command;

const TASKS_DIR: &str = "tasks";
const RUNS_DIR: &str = "runs";
const REPORTS_DIR: &str = "reports";
const CLONE_CACHE_DIR: &str = ".cache";
const GIT_ENTRY: &str = ".git";
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalHome {
  root: PathBuf,
}

impl EvalHome {
  pub fn new(root: impl Into<PathBuf>) -> Self {
    Self { root: root.into() }
  }

  pub fn of_tasks(tasks_dir: &Path) -> AgentResult<Self> {
    let tasks_dir = absolute(tasks_dir);
    let root = tasks_dir.parent().ok_or_else(|| {
      AgentError::InvalidArgument(format!(
        "the task directory {} has no parent to keep its runs and reports in; pass --eval-home",
        tasks_dir.display()
      ))
    })?;
    Ok(Self::new(root))
  }

  pub fn of_batch(batch_dir: &Path) -> AgentResult<Self> {
    let batch_dir = without_parent_hops(&absolute(batch_dir));
    let runs = batch_dir.parent().filter(|runs| runs.file_name().is_some_and(|name| name == RUNS_DIR));
    let root = runs.and_then(Path::parent).ok_or_else(|| {
      AgentError::InvalidArgument(format!(
        "{} is not laid out as <eval-home>/{RUNS_DIR}/<batch>, so its eval home is unknown; pass --out",
        batch_dir.display()
      ))
    })?;
    Ok(Self::new(root))
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  pub fn tasks(&self) -> PathBuf {
    self.root.join(TASKS_DIR)
  }

  pub fn batch(&self, name: &str) -> PathBuf {
    self.root.join(RUNS_DIR).join(name)
  }

  pub fn clone_cache(&self) -> PathBuf {
    self.root.join(RUNS_DIR).join(CLONE_CACHE_DIR)
  }

  pub fn report(&self, date: &str, name: &str) -> PathBuf {
    self.root.join(REPORTS_DIR).join(date).join(name)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitTracking {
  Trackable,
  Ignored,
  OutsideRepository,
  Unknown,
}

pub fn git_tracking(path: &Path) -> GitTracking {
  let path = absolute(path);
  let Some(existing) = path.ancestors().find(|ancestor| ancestor.is_dir()) else {
    return GitTracking::OutsideRepository;
  };
  let Ok(anchor) = existing.canonicalize() else {
    return GitTracking::OutsideRepository;
  };
  if !anchor.ancestors().any(|ancestor| ancestor.join(GIT_ENTRY).exists()) {
    return GitTracking::OutsideRepository;
  }
  let target = path.strip_prefix(existing).map_or_else(|_| anchor.clone(), |rest| anchor.join(rest));
  let mut command = git_command(&anchor);
  command.args(["check-ignore", "--quiet", "--"]).arg(&target);
  match run_captured(command, None, GIT_TIMEOUT).exit {
    ProcessExit::Exited { code: Some(0) } => GitTracking::Ignored,
    ProcessExit::Exited { code: Some(1) } => GitTracking::Trackable,
    _ => GitTracking::Unknown,
  }
}

pub fn keep_private_untracked(sources: &[&Path], outputs: &[&Path]) -> AgentResult<()> {
  let tracked: Vec<(&Path, GitTracking)> = sources.iter().map(|source| (*source, git_tracking(source))).collect();
  let find = |wanted: GitTracking| tracked.iter().find(|(_, tracking)| *tracking == wanted).map(|(source, _)| *source);
  let private = match (find(GitTracking::Ignored), find(GitTracking::Unknown)) {
    (Some(ignored), _) => format!("{} is ignored by git, so it is a private task set", ignored.display()),
    (None, Some(unknown)) => format!(
      "{} is inside a git repository and git could not say whether it is ignored, so it is treated as a private task set",
      unknown.display()
    ),
    (None, None) => return Ok(()),
  };
  for output in outputs {
    let refusal = match git_tracking(output) {
      GitTracking::Trackable => "which git would track",
      GitTracking::Unknown => "where git could not say whether it would be tracked",
      GitTracking::Ignored | GitTracking::OutsideRepository => continue,
    };
    return Err(AgentError::InvalidArgument(format!(
      "{private}; refusing to write what derives from it to {}, {refusal}",
      output.display()
    )));
  }
  Ok(())
}

pub fn batch_sources(batch_dirs: &[PathBuf]) -> Vec<PathBuf> {
  let sources = batch_dirs.iter().flat_map(|batch| {
    let as_written = EvalHome::of_batch(batch).map_or_else(|_| absolute(batch), |home| home.root);
    let on_disk = batch.canonicalize().ok().and_then(|real| EvalHome::of_batch(&real).ok()).map(|home| home.root);
    std::iter::once(as_written).chain(on_disk)
  });
  sources.collect()
}

pub fn batch_report_base(batch_dirs: &[PathBuf], also_read: &[&Path], out: Option<&Path>, date: &str, name: &str) -> AgentResult<PathBuf> {
  let out = match (out, batch_dirs.first()) {
    (Some(out), _) => out.to_owned(),
    (None, Some(first)) => EvalHome::of_batch(first)?.report(date, name),
    (None, None) => return Err(AgentError::InvalidArgument("no batch directory given".to_owned())),
  };
  let base = without_report_extension(out);
  let sources = batch_sources(batch_dirs);
  let sources: Vec<&Path> = sources.iter().map(PathBuf::as_path).chain(also_read.iter().copied()).collect();
  keep_private_untracked(&sources, &[&base])?;
  Ok(base)
}

pub fn task_report_base(tasks_dir: &Path, out: Option<&Path>, also_written: &[&Path], date: &str, name: &str) -> AgentResult<PathBuf> {
  let out = match out {
    Some(out) => out.to_owned(),
    None => EvalHome::of_tasks(tasks_dir)?.report(date, name),
  };
  let base = without_report_extension(out);
  let outputs: Vec<&Path> = std::iter::once(base.as_path()).chain(also_written.iter().copied()).collect();
  keep_private_untracked(&[tasks_dir], &outputs)?;
  Ok(base)
}

fn without_report_extension(out: PathBuf) -> PathBuf {
  match out.extension().and_then(|extension| extension.to_str()) {
    Some("md" | "json") => out.with_extension(""),
    _ => out,
  }
}

fn absolute(path: &Path) -> PathBuf {
  std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

fn without_parent_hops(path: &Path) -> PathBuf {
  let mut resolved = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        resolved.pop();
      }
      other => resolved.push(other),
    }
  }
  resolved
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;

  fn repository() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join(".gitignore"), "eval/private/\neval/runs/\n").unwrap();
    for dir in ["eval/tasks/t00", "eval/reports", "eval/private/acme/tasks/p00"] {
      fs::create_dir_all(repo.path().join(dir)).unwrap();
    }
    let mut init = git_command(repo.path());
    init.args(["init", "--quiet"]);
    assert!(run_captured(init, None, GIT_TIMEOUT).exit.succeeded());
    repo
  }

  #[test]
  fn should_derive_the_eval_home_from_a_task_directory_and_place_runs_cache_and_reports_in_it() {
    let home = EvalHome::of_tasks(Path::new("/work/eval/private/acme/tasks")).unwrap();
    assert_eq!(home.root(), Path::new("/work/eval/private/acme"));
    assert_eq!(home.tasks(), Path::new("/work/eval/private/acme/tasks"));
    assert_eq!(home.batch("2026-10-12-seed1"), Path::new("/work/eval/private/acme/runs/2026-10-12-seed1"));
    assert_eq!(home.clone_cache(), Path::new("/work/eval/private/acme/runs/.cache"));
    assert_eq!(home.report("2026-10-12", "agent"), Path::new("/work/eval/private/acme/reports/2026-10-12/agent"));
    assert_eq!(EvalHome::of_tasks(Path::new("/work/eval/tasks")).unwrap().root(), Path::new("/work/eval"));
  }

  #[test]
  fn should_derive_the_eval_home_from_a_batch_directory_and_reject_other_layouts() {
    let home = EvalHome::of_batch(Path::new("/work/eval/private/acme/runs/b1/")).unwrap();
    assert_eq!(home.root(), Path::new("/work/eval/private/acme"));
    assert_eq!(
      EvalHome::of_batch(Path::new("/work/eval/runs/b1")).unwrap().report("d", "headroom"),
      Path::new("/work/eval/reports/d/headroom")
    );
    for stray in ["/work/batches/b1", "/runs", "/"] {
      assert!(matches!(EvalHome::of_batch(Path::new(stray)), Err(AgentError::InvalidArgument(_))), "{stray}");
    }
    assert!(matches!(EvalHome::of_tasks(Path::new("/")), Err(AgentError::InvalidArgument(_))));
  }

  #[test]
  fn should_resolve_a_relative_batch_directory_against_the_working_directory() {
    let home = EvalHome::of_batch(Path::new("eval/runs/b1")).unwrap();
    assert_eq!(home.root(), std::env::current_dir().unwrap().join("eval"));
  }

  #[test]
  fn should_accept_a_batch_directory_written_with_parent_hops() {
    assert_eq!(EvalHome::of_batch(Path::new("/work/eval/runs/b0/../b1")).unwrap().root(), Path::new("/work/eval"));
    assert_eq!(
      EvalHome::of_batch(Path::new("/work/eval/runs/b0/./../b1/")).unwrap().batch("b1"),
      Path::new("/work/eval/runs/b1")
    );
    let sibling = EvalHome::of_batch(Path::new("runs/b0/../b1")).unwrap();
    assert_eq!(sibling.root(), std::env::current_dir().unwrap());
  }

  #[test]
  fn should_tell_ignored_paths_from_trackable_ones_and_from_paths_outside_a_repository() {
    let repo = repository();
    let outside = tempfile::tempdir().unwrap();
    let at = |path: &str| git_tracking(&repo.path().join(path));
    assert_eq!(at("eval/tasks"), GitTracking::Trackable);
    assert_eq!(at("eval/reports/2026-10-12/agent"), GitTracking::Trackable);
    assert_eq!(at("eval"), GitTracking::Trackable);
    assert_eq!(at("eval/private/absent/../../reports/x"), GitTracking::Trackable);
    assert_eq!(at("eval/private/acme/tasks"), GitTracking::Ignored);
    assert_eq!(at("eval/private/acme/reports/2026-10-12/agent"), GitTracking::Ignored);
    assert_eq!(at("eval/runs/b1"), GitTracking::Ignored);
    assert_eq!(git_tracking(&outside.path().join("reports/agent")), GitTracking::OutsideRepository);
  }

  fn broken_repository() -> tempfile::TempDir {
    let broken = tempfile::tempdir().unwrap();
    fs::write(broken.path().join(GIT_ENTRY), "gitdir: /nonexistent/elsewhere\n").unwrap();
    fs::create_dir_all(broken.path().join("sets/acme/tasks")).unwrap();
    broken
  }

  #[test]
  fn should_report_unknown_when_a_path_is_inside_a_repository_that_git_cannot_read() {
    let broken = broken_repository();
    assert_eq!(git_tracking(&broken.path().join("sets/acme/tasks")), GitTracking::Unknown);
    assert_eq!(git_tracking(&broken.path().join("reports/new/agent")), GitTracking::Unknown);
  }

  #[test]
  fn should_refuse_to_write_what_derives_from_a_private_task_set_where_git_would_track_it() {
    let repo = repository();
    let outside = tempfile::tempdir().unwrap();
    let private_tasks = repo.path().join("eval/private/acme/tasks");
    let private_home = repo.path().join("eval/private/acme");
    let public_tasks = repo.path().join("eval/tasks");
    let tracked_report = repo.path().join("eval/reports/2026-10-12/agent");
    let private_report = repo.path().join("eval/private/acme/reports/2026-10-12/agent");
    let public_home = repo.path().join("eval");

    let refused = keep_private_untracked(&[&private_tasks], &[&tracked_report]).unwrap_err();
    assert_eq!(
      refused.to_string(),
      format!(
        "invalid argument: {} is ignored by git, so it is a private task set; refusing to write what derives from it to {}, which git would track",
        private_tasks.display(),
        tracked_report.display()
      )
    );
    assert!(keep_private_untracked(&[&private_home], &[&public_home]).is_err());
    assert!(keep_private_untracked(&[&public_tasks, &private_home], &[&private_report, &tracked_report]).is_err());
    keep_private_untracked(&[&private_tasks], &[&private_report]).unwrap();
    keep_private_untracked(&[&private_tasks], &[&outside.path().join("agent")]).unwrap();
    keep_private_untracked(&[&public_tasks], &[&tracked_report]).unwrap();
    keep_private_untracked(&[&outside.path().join("tasks")], &[&tracked_report]).unwrap();
  }

  #[test]
  fn should_fail_closed_when_git_cannot_say_whether_a_source_or_an_output_inside_a_repository_is_ignored() {
    let repo = repository();
    let broken = broken_repository();
    let outside = tempfile::tempdir().unwrap();
    let unreadable_tasks = broken.path().join("sets/acme/tasks");
    let tracked_report = repo.path().join("eval/reports/2026-10-12/agent");
    let private_tasks = repo.path().join("eval/private/acme/tasks");

    let refused = keep_private_untracked(&[&unreadable_tasks], &[&tracked_report]).unwrap_err();
    assert_eq!(
      refused.to_string(),
      format!(
        "invalid argument: {} is inside a git repository and git could not say whether it is ignored, so it is treated as a private task set; refusing to write what derives from it to {}, which git would track",
        unreadable_tasks.display(),
        tracked_report.display()
      )
    );
    let unreadable_report = broken.path().join("reports/agent");
    let refused = keep_private_untracked(&[&private_tasks], &[&unreadable_report]).unwrap_err();
    assert!(refused.to_string().ends_with("where git could not say whether it would be tracked"), "{refused}");
    keep_private_untracked(&[&unreadable_tasks], &[&outside.path().join("agent")]).unwrap();
    keep_private_untracked(&[&unreadable_tasks], &[&repo.path().join("eval/private/acme/reports/x")]).unwrap();
    keep_private_untracked(&[&repo.path().join("eval/tasks")], &[&unreadable_report]).unwrap();
  }

  #[test]
  fn should_default_a_batch_report_into_the_home_of_its_first_batch_and_strip_a_report_extension() {
    let batches = [PathBuf::from("/work/eval/private/acme/runs/b1"), PathBuf::from("/work/eval/private/acme/runs/b2")];
    assert_eq!(
      batch_report_base(&batches, &[], None, "2026-10-12", "headroom").unwrap(),
      PathBuf::from("/work/eval/private/acme/reports/2026-10-12/headroom")
    );
    assert_eq!(
      batch_report_base(&batches, &[], Some(Path::new("/out/agent.md")), "d", "agent").unwrap(),
      PathBuf::from("/out/agent")
    );
    assert_eq!(
      batch_report_base(&batches, &[], Some(Path::new("/out/noise-floor")), "d", "noise-floor").unwrap(),
      PathBuf::from("/out/noise-floor")
    );
    assert!(matches!(
      batch_report_base(&[PathBuf::from("/work/loose")], &[], None, "d", "agent"),
      Err(AgentError::InvalidArgument(_))
    ));
    assert!(matches!(batch_report_base(&[], &[], None, "d", "agent"), Err(AgentError::InvalidArgument(_))));
  }

  #[test]
  fn should_keep_reports_of_a_private_batch_or_task_set_out_of_the_tracked_reports() {
    let repo = repository();
    let private_batch = [repo.path().join("eval/private/acme/runs/b1")];
    let public_batch = [repo.path().join("eval/runs/b1")];
    let tracked = repo.path().join("eval/reports/2026-10-12/agent");
    let canonical = repo.path().canonicalize().unwrap();

    assert_eq!(
      batch_report_base(&private_batch, &[], None, "2026-10-12", "agent").unwrap(),
      repo.path().join("eval/private/acme/reports/2026-10-12/agent")
    );
    assert!(batch_report_base(&private_batch, &[], Some(&tracked), "2026-10-12", "agent").is_err());
    assert!(batch_report_base(&[public_batch[0].clone(), private_batch[0].clone()], &[], None, "2026-10-12", "noise-floor").is_err());
    assert_eq!(batch_report_base(&public_batch, &[], None, "2026-10-12", "agent").unwrap(), tracked);

    let private_tasks = repo.path().join("eval/private/acme/tasks");
    assert_eq!(
      task_report_base(&private_tasks, None, &[], "2026-10-12", "retrieval").unwrap(),
      repo.path().join("eval/private/acme/reports/2026-10-12/retrieval")
    );
    assert!(task_report_base(&private_tasks, Some(&tracked), &[], "2026-10-12", "retrieval").is_err());
    assert!(task_report_base(&private_tasks, Some(&canonical.join("eval/reports/x.md")), &[], "2026-10-12", "retrieval").is_err());
    assert_eq!(
      task_report_base(&repo.path().join("eval/tasks"), None, &[], "2026-10-12", "retrieval").unwrap(),
      repo.path().join("eval/reports/2026-10-12/retrieval")
    );
  }

  #[test]
  fn should_refuse_a_tracked_report_for_a_private_batch_that_is_not_laid_out_under_runs() {
    let repo = repository();
    let stray = [repo.path().join("eval/private/acme/custom/b9")];
    let tracked = repo.path().join("eval/reports/t2");

    let refused = batch_report_base(&stray, &[], Some(&tracked), "2026-10-12", "agent").unwrap_err();
    assert_eq!(
      refused.to_string(),
      format!(
        "invalid argument: {} is ignored by git, so it is a private task set; refusing to write what derives from it to {}, which git would track",
        stray[0].display(),
        tracked.display()
      )
    );
    assert_eq!(
      batch_report_base(&stray, &[], Some(&repo.path().join("eval/private/acme/reports/t2.md")), "2026-10-12", "agent").unwrap(),
      repo.path().join("eval/private/acme/reports/t2")
    );
    assert_eq!(batch_sources(&stray), vec![stray[0].clone()]);
  }

  #[test]
  fn should_refuse_a_tracked_report_when_a_public_batch_is_read_with_a_private_task_directory() {
    let repo = repository();
    let public_batch = [repo.path().join("eval/runs/b1")];
    let private_tasks = repo.path().join("eval/private/acme/tasks");

    assert!(batch_report_base(&public_batch, &[&private_tasks], None, "2026-10-12", "headroom").is_err());
    assert!(batch_report_base(&public_batch, &[&private_tasks], Some(&repo.path().join("eval/reports/h")), "2026-10-12", "headroom").is_err());
    assert_eq!(
      batch_report_base(
        &public_batch,
        &[&private_tasks],
        Some(&repo.path().join("eval/private/acme/reports/h")),
        "2026-10-12",
        "headroom"
      )
      .unwrap(),
      repo.path().join("eval/private/acme/reports/h")
    );
    assert_eq!(
      batch_report_base(&public_batch, &[&repo.path().join("eval/tasks")], None, "2026-10-12", "headroom").unwrap(),
      repo.path().join("eval/reports/2026-10-12/headroom")
    );
  }

  #[test]
  fn should_refuse_a_tracked_report_when_a_public_batch_is_compared_with_a_private_report() {
    let repo = repository();
    let public_batch = [repo.path().join("eval/runs/b1")];
    let private_report = repo.path().join("eval/private/acme/reports/2026-10-12/agent.json");
    let public_report = repo.path().join("eval/reports/2026-10-04/agent.json");

    assert!(batch_report_base(&public_batch, &[&private_report], None, "2026-10-12", "noise-floor").is_err());
    assert_eq!(
      batch_report_base(&public_batch, &[&public_report], None, "2026-10-12", "noise-floor").unwrap(),
      repo.path().join("eval/reports/2026-10-12/noise-floor")
    );
  }

  #[cfg(unix)]
  #[test]
  fn should_judge_an_output_where_the_file_system_puts_it_when_its_path_hops_back_through_a_symlink() {
    let repo = repository();
    fs::create_dir_all(repo.path().join("eval/reports/sub")).unwrap();
    std::os::unix::fs::symlink(repo.path().join("eval/reports/sub"), repo.path().join("eval/private/acme/lnk")).unwrap();
    let private_tasks = repo.path().join("eval/private/acme/tasks");
    let through_link = repo.path().join("eval/private/acme/lnk/../z");

    assert_eq!(git_tracking(&through_link), GitTracking::Trackable);
    assert_eq!(git_tracking(&repo.path().join("eval/private/acme/lnk/z")), GitTracking::Trackable);
    let refused = task_report_base(&private_tasks, Some(&through_link), &[], "2026-10-12", "leak-audit").unwrap_err();
    assert!(refused.to_string().ends_with("which git would track"), "{refused}");
    assert!(batch_report_base(&[repo.path().join("eval/private/acme/runs/b1")], &[], Some(&through_link), "2026-10-12", "agent").is_err());
    assert!(!repo.path().join("eval/reports/z.md").exists());
  }

  #[cfg(unix)]
  #[test]
  fn should_treat_a_batch_as_private_when_its_path_reads_public_but_resolves_into_a_private_home() {
    let repo = repository();
    fs::create_dir_all(repo.path().join("eval/private/acme/runs/b1")).unwrap();
    fs::create_dir_all(repo.path().join("eval/private/acme/sub")).unwrap();
    std::os::unix::fs::symlink(repo.path().join("eval/private/acme/sub"), repo.path().join("eval/publnk")).unwrap();
    let disguised = [repo.path().join("eval/publnk/../runs/b1")];
    let tracked = repo.path().join("eval/reports/2026-10-12/agent");

    assert_eq!(EvalHome::of_batch(&disguised[0]).unwrap().root(), repo.path().join("eval"));
    assert_eq!(
      batch_sources(&disguised),
      vec![repo.path().join("eval"), repo.path().canonicalize().unwrap().join("eval/private/acme")]
    );
    assert!(batch_report_base(&disguised, &[], Some(&tracked), "2026-10-12", "agent").is_err());
    assert!(batch_report_base(&disguised, &[], None, "2026-10-12", "agent").is_err());
  }

  #[test]
  fn should_refuse_a_tracked_clone_cache_or_work_directory_for_a_private_task_set() {
    let repo = repository();
    let outside = tempfile::tempdir().unwrap();
    let private_tasks = repo.path().join("eval/private/acme/tasks");
    let tracked_cache = repo.path().join("eval/cache-here");
    let private_cache = repo.path().join("eval/private/acme/runs/.cache");

    let refused = task_report_base(&private_tasks, None, &[&tracked_cache], "2026-10-12", "leak-audit").unwrap_err();
    assert!(refused.to_string().contains(&format!("to {}, which git would track", tracked_cache.display())), "{refused}");
    assert!(task_report_base(&private_tasks, None, &[&private_cache, &tracked_cache], "2026-10-12", "retrieval").is_err());
    assert_eq!(
      task_report_base(&private_tasks, None, &[&private_cache, outside.path()], "2026-10-12", "retrieval").unwrap(),
      repo.path().join("eval/private/acme/reports/2026-10-12/retrieval")
    );
    assert_eq!(
      task_report_base(&repo.path().join("eval/tasks"), None, &[&tracked_cache], "2026-10-12", "retrieval").unwrap(),
      repo.path().join("eval/reports/2026-10-12/retrieval")
    );
  }

  #[test]
  fn should_use_the_home_of_a_well_formed_batch_and_the_directory_itself_of_any_other_as_guard_sources() {
    let batches = [
      PathBuf::from("/work/eval/private/acme/runs/b1"),
      PathBuf::from("/work/loose"),
      PathBuf::from("/work/eval/runs/b2"),
    ];
    assert_eq!(
      batch_sources(&batches),
      vec![PathBuf::from("/work/eval/private/acme"), PathBuf::from("/work/loose"), PathBuf::from("/work/eval")]
    );
  }
}
