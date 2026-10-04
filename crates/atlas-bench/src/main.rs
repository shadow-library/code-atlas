mod compare;
mod corpus;
mod error;
mod generate;
mod markdown;
mod process;
mod profile;
mod report;
mod rng;
mod scenario;
mod source;
mod stats;
mod suite;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::error::{BenchError, BenchResult};
use crate::generate::{GenConfig, LangChoice};
use crate::suite::{RunOptions, Suite};

#[derive(Debug, Parser)]
#[command(name = "atlas-bench", about = "Performance harness for Code Atlas")]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  Run {
    #[arg(long, value_enum)]
    suite: Suite,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    compare: Option<PathBuf>,
    #[arg(long)]
    work_dir: Option<PathBuf>,
    #[arg(long)]
    corpora_dir: Option<PathBuf>,
    #[arg(long = "only")]
    only: Vec<String>,
    #[arg(long = "scenario")]
    scenarios: Vec<String>,
    #[arg(long)]
    profile: bool,
    #[arg(long)]
    fail_on_regression: bool,
  },
  Fetch {
    #[arg(long)]
    corpora_dir: Option<PathBuf>,
    #[arg(long = "only")]
    only: Vec<String>,
  },
  Gen {
    #[arg(long)]
    loc: u64,
    #[arg(long, default_value_t = 1)]
    repos: usize,
    #[arg(long, value_enum, default_value = "mixed")]
    lang: LangChoice,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 1)]
    projects: usize,
    #[arg(long)]
    out: PathBuf,
  },
  #[command(hide = true)]
  Worker {
    #[arg(long)]
    spec: PathBuf,
  },
}

fn dispatch(command: Command) -> BenchResult<()> {
  match command {
    Command::Run {
      suite,
      out,
      compare,
      work_dir,
      corpora_dir,
      only,
      scenarios,
      profile,
      fail_on_regression,
    } => {
      let profile_dir = profile.then(|| suite::report_dir(&out));
      let report = suite::run(&RunOptions {
        profile_dir,
        suite,
        only,
        scenarios,
        work_dir,
        corpora_dir,
        previous: compare,
      })?;
      let (json, md) = suite::write_report(&report, &out)?;
      let regressions = report.comparison.as_ref().map(|comparison| comparison.regressions().count()).unwrap_or(0);
      tracing::info!(json = %json.display(), markdown = %md.display(), regressions, seconds = report.total_seconds, "report written");
      match fail_on_regression && regressions > 0 {
        true => Err(BenchError::Invalid(format!("{regressions} regression(s) above the threshold"))),
        false => Ok(()),
      }
    }
    Command::Fetch { corpora_dir, only } => {
      let base = corpus::corpora_dir(corpora_dir.as_deref())?;
      std::fs::create_dir_all(&base).map_err(|source| BenchError::io("create", &base, source))?;
      let mut failures = Vec::new();
      for pinned in corpus::PINNED.iter().filter(|pinned| only.is_empty() || only.iter().any(|name| name == pinned.name)) {
        match corpus::fetch(pinned, &base) {
          Ok(path) => tracing::info!(corpus = pinned.name, commit = pinned.commit, path = %path.display(), "corpus ready"),
          Err(error) => {
            tracing::error!(corpus = pinned.name, error = %error, "fetch failed; the run suite will record this corpus as skipped");
            failures.push(error.to_string());
          }
        }
      }
      match failures.is_empty() {
        true => Ok(()),
        false => Err(BenchError::Invalid(failures.join("; "))),
      }
    }
    Command::Gen {
      loc,
      repos,
      lang,
      seed,
      projects,
      out,
    } => {
      let summary = generate::generate(&GenConfig { loc, repos, lang, seed, projects }, &out)?;
      tracing::info!(files = summary.files, loc = summary.loc, tree_hash = summary.tree_hash, out = %out.display(), "generated corpus");
      Ok(())
    }
    Command::Worker { spec } => {
      let text = source::read(&spec)?;
      let spec: scenario::WorkerSpec = serde_json::from_str(&text).map_err(|error| BenchError::json("worker spec", error))?;
      let records = scenario::run_worker(&spec)?;
      let output = serde_json::to_string(&records).map_err(|error| BenchError::json("worker output", error))?;
      println!("{output}");
      Ok(())
    }
  }
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  if !matches!(cli.command, Command::Worker { .. }) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
  }
  match dispatch(cli.command) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      tracing::error!(error = %error, "atlas-bench failed");
      eprintln!("atlas-bench: {error}");
      ExitCode::FAILURE
    }
  }
}
