use std::path::{Path, PathBuf};

use clap::Args;

use super::error::{RetrievalError, RetrievalResult};
use super::report::{TaskOutcome, write_report};
use super::run::{RetrievalOptions, run_retrieval};
use crate::agent::{EvalHome, task_report_base, today_utc};

#[derive(Debug, Clone, Args)]
pub struct RetrievalArgs {
  #[arg(
    long,
    help = "Repository root holding eval/tasks and the task base commits (default: the git top level of the working directory)"
  )]
  pub repo_root: Option<PathBuf>,
  #[arg(long, help = "Task directory (default: <repo-root>/eval/tasks)")]
  pub tasks: Option<PathBuf>,
  #[arg(long = "task", help = "Only evaluate this task id (repeatable)")]
  pub only: Vec<String>,
  #[arg(
    long,
    help = "Output base path; .md and .json are appended (default: reports/<date>/retrieval beside the task directory, so eval/reports/<date>/retrieval for eval/tasks)"
  )]
  pub out: Option<PathBuf>,
  #[arg(
    long,
    help = "Clone cache for external task repositories (default: runs/.cache beside the task directory, shared with `agent run`)"
  )]
  pub cache_dir: Option<PathBuf>,
  #[arg(long, help = "Directory for the per-task scratch trees and indexes (default: the system temp dir)")]
  pub work_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalOutcome {
  pub markdown: PathBuf,
  pub json: PathBuf,
}

pub fn execute_retrieval(args: &RetrievalArgs, repo_root: &Path) -> RetrievalResult<RetrievalOutcome> {
  let tasks_dir = args.tasks.clone().unwrap_or_else(|| repo_root.join("eval").join("tasks"));
  let cache_dir = match &args.cache_dir {
    Some(path) => path.clone(),
    None => EvalHome::of_tasks(&tasks_dir)?.clone_cache(),
  };
  let work_dir = args.work_dir.clone().unwrap_or_else(std::env::temp_dir);
  let base = task_report_base(&tasks_dir, args.out.as_deref(), &[&cache_dir, &work_dir], &today_utc(), "retrieval")?;
  let options = RetrievalOptions {
    repo_root: repo_root.to_owned(),
    tasks_dir,
    only: args.only.clone(),
    cache_dir,
    work_dir,
  };
  let report = run_retrieval(&options)?;
  let (markdown, json) = write_report(&report, &base)?;
  let failed = report.tasks.iter().filter(|task| matches!(task.outcome, TaskOutcome::Failed { .. })).count();
  if failed > 0 {
    return Err(RetrievalError::TasksFailed {
      failed,
      total: report.tasks.len(),
      report: markdown,
    });
  }
  Ok(RetrievalOutcome { markdown, json })
}
