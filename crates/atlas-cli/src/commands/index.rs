use std::io::{IsTerminal, stderr};

use atlas_engine::{IndexMode, RunOutcomeKind};

use crate::cli::IndexArgs;
use crate::context::Context;
use crate::error::CliResult;
use crate::output::{Outcome, emit_line, note};

const FAILED_FILES_SHOWN: usize = 20;

pub fn run(ctx: &Context, args: IndexArgs) -> CliResult<Outcome> {
  let project = ctx.resolve()?;
  let progress = stderr().is_terminal();
  if progress {
    note(format!("indexing {} ({} repos)...", project.name, project.repos.len()));
  }
  let engine = ctx.open_engine(&project)?;
  let mode = if args.full { IndexMode::Full } else { IndexMode::Incremental };
  let summary = engine.index(mode)?;
  if progress {
    for (phase, millis) in &summary.phase_timings_ms {
      note(format!("  {phase}: {millis}ms"));
    }
    for (name, count) in &summary.counts {
      note(format!("  {name}: {count}"));
    }
  }
  emit_line(format!(
    "indexed {}: run {}, {} files updated in {:.1}s",
    project.name,
    summary.run,
    summary.files_updated,
    summary.duration.as_secs_f64()
  ))?;
  let degraded = match &summary.outcome {
    RunOutcomeKind::Ok => {
      emit_line("outcome: ok")?;
      !summary.failed_files.is_empty()
    }
    RunOutcomeKind::Partial { reason } => {
      emit_line(format!("outcome: partial ({reason})"))?;
      true
    }
  };
  for file in summary.failed_files.iter().take(FAILED_FILES_SHOWN) {
    let location = match &file.path {
      Some(path) => format!("{}:{path}", file.repo),
      None => file.repo.to_string(),
    };
    emit_line(format!("  failed {location}: {}", file.reason))?;
  }
  if summary.failed_files.len() > FAILED_FILES_SHOWN {
    emit_line(format!("  {} more failed files", summary.failed_files.len() - FAILED_FILES_SHOWN))?;
  }
  Ok(Outcome::degraded_if(degraded))
}
