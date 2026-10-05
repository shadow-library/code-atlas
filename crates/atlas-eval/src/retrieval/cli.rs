use std::path::{Path, PathBuf};

use clap::Args;

use super::error::{RetrievalError, RetrievalResult};
use super::report::{TaskOutcome, write_report};
use super::run::{RetrievalOptions, run_retrieval};
use crate::agent::today_utc;

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
  #[arg(long, help = "Output base path; .md and .json are appended (default: eval/reports/<date>/retrieval)")]
  pub out: Option<PathBuf>,
  #[arg(long, help = "Clone cache for external task repositories (default: <repo-root>/eval/runs/.cache, shared with `agent run`)")]
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
  let options = RetrievalOptions {
    repo_root: repo_root.to_owned(),
    tasks_dir: args.tasks.clone().unwrap_or_else(|| repo_root.join("eval").join("tasks")),
    only: args.only.clone(),
    cache_dir: args.cache_dir.clone().unwrap_or_else(|| repo_root.join("eval").join("runs").join(".cache")),
    work_dir: args.work_dir.clone().unwrap_or_else(std::env::temp_dir),
  };
  let report = run_retrieval(&options)?;
  let out = args
    .out
    .clone()
    .unwrap_or_else(|| PathBuf::from("eval").join("reports").join(today_utc()).join("retrieval"));
  let base = match out.extension().and_then(|extension| extension.to_str()) {
    Some("md" | "json") => out.with_extension(""),
    _ => out,
  };
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
