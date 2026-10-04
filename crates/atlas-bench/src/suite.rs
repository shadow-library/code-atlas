use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use atlas_core::{ProjectName, RepoName};
use atlas_project::{Project, ProjectSettings, RepoConfig};
use clap::ValueEnum;

use crate::compare::{REGRESSION_THRESHOLD_PCT, compare};
use crate::corpus::{self, HONO, PinnedCorpus, RIPGREP};
use crate::error::{BenchError, BenchResult};
use crate::generate::{self, GenConfig, LangChoice, ProjectOutput};
use crate::markdown;
use crate::process::{Usage, run_measured};
use crate::profile;
use crate::report::{CorpusInfo, Environment, PerfReport, ProfileInfo, SCHEMA_VERSION, ScenarioRecord, today};
use crate::scenario::{Settings, WorkerMode, WorkerSpec};
use crate::source::{count_loc, list_sources};
use crate::stats::{median, range};

const SYNTH_SEED: u64 = 20_260_504;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Suite {
  Quick,
  Full,
}

impl Suite {
  pub fn name(self) -> &'static str {
    match self {
      Self::Quick => "quick",
      Self::Full => "full",
    }
  }
}

#[derive(Debug, Clone)]
enum Source {
  SelfRepo,
  Fixtures(&'static str),
  Pinned(PinnedCorpus),
  Synthetic(GenConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
  Session,
  Multi,
}

#[derive(Debug, Clone)]
struct Plan {
  name: String,
  source: Source,
  depth: Depth,
}

fn synthetic(name: &str, loc: u64, repos: usize, projects: usize, depth: Depth) -> Plan {
  Plan {
    name: name.to_owned(),
    source: Source::Synthetic(GenConfig {
      loc,
      repos,
      lang: LangChoice::Mixed,
      seed: SYNTH_SEED,
      projects,
    }),
    depth,
  }
}

fn plans(suite: Suite) -> Vec<Plan> {
  let mut plans = vec![
    Plan {
      name: "self".to_owned(),
      source: Source::SelfRepo,
      depth: Depth::Session,
    },
    Plan {
      name: "fixtures-acme".to_owned(),
      source: Source::Fixtures("acme"),
      depth: Depth::Session,
    },
    Plan {
      name: "fixtures-globex".to_owned(),
      source: Source::Fixtures("globex"),
      depth: Depth::Session,
    },
  ];
  match suite {
    Suite::Quick => {
      plans.push(synthetic("synth-100k", 100_000, 2, 1, Depth::Session));
      plans.push(synthetic("synth-multi-project", 30_000, 2, 3, Depth::Multi));
    }
    Suite::Full => {
      plans.push(synthetic("synth-10k", 10_000, 1, 1, Depth::Session));
      plans.push(synthetic("synth-100k", 100_000, 2, 1, Depth::Session));
      plans.push(synthetic("synth-100k-5-repos", 100_000, 5, 1, Depth::Session));
      plans.push(synthetic("synth-multi-project", 30_000, 2, 3, Depth::Multi));
      plans.push(Plan {
        name: "ripgrep".to_owned(),
        source: Source::Pinned(RIPGREP),
        depth: Depth::Session,
      });
      plans.push(Plan {
        name: "hono".to_owned(),
        source: Source::Pinned(HONO),
        depth: Depth::Session,
      });
      plans.push(synthetic("synth-1m", 1_000_000, 2, 1, Depth::Session));
    }
  }
  plans
}

struct Prepared {
  info: CorpusInfo,
  projects: Vec<PathBuf>,
  loc: u64,
  synthetic: bool,
}

fn repo_config(name: &str, path: &Path) -> BenchResult<RepoConfig> {
  Ok(RepoConfig {
    name: RepoName::new(name)?,
    path: path.to_path_buf(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  })
}

fn project(name: &str, repos: Vec<RepoConfig>) -> BenchResult<Project> {
  Ok(Project {
    name: ProjectName::new(name)?,
    settings: ProjectSettings::default(),
    repos,
  })
}

pub(crate) fn synthetic_projects(output: &[ProjectOutput]) -> BenchResult<Vec<Project>> {
  output
    .iter()
    .map(|generated| {
      project(
        &generated.name,
        generated.repos.iter().map(|repo| repo_config(&repo.name, &repo.path)).collect::<BenchResult<_>>()?,
      )
    })
    .collect()
}

fn workspace_root() -> BenchResult<PathBuf> {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
  root.canonicalize().map_err(|source| BenchError::io("resolve", &root, source))
}

struct Materialised {
  projects: Vec<Project>,
  kind: &'static str,
  commit: Option<String>,
  tree_hash: Option<String>,
}

fn materialise(plan: &Plan, dir: &Path, corpora_dir: &Path) -> BenchResult<Materialised> {
  match &plan.source {
    Source::SelfRepo => {
      let repo = dir.join("code-atlas");
      corpus::copy_tree(&workspace_root()?, &repo)?;
      Ok(Materialised {
        projects: vec![project("self", vec![repo_config("code-atlas", &repo)?])?],
        kind: "self",
        commit: None,
        tree_hash: None,
      })
    }
    Source::Fixtures(set) => {
      let from = workspace_root()?.join("fixtures").join(set);
      let mut repos = Vec::new();
      let mut names: Vec<String> = fs::read_dir(&from)
        .map_err(|source| BenchError::io("read", &from, source))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
      names.sort();
      for name in names {
        let target = dir.join(&name);
        corpus::copy_tree(&from.join(&name), &target)?;
        repos.push(repo_config(&name, &target)?);
      }
      Ok(Materialised {
        projects: vec![project(set, repos)?],
        kind: "fixtures",
        commit: None,
        tree_hash: None,
      })
    }
    Source::Pinned(pinned) => {
      let checkout = corpus::fetch(pinned, corpora_dir)?;
      let target = dir.join(pinned.name);
      corpus::copy_tree(&checkout, &target)?;
      Ok(Materialised {
        projects: vec![project(pinned.name, vec![repo_config(pinned.name, &target)?])?],
        kind: "external",
        commit: corpus::checked_out_commit(&checkout),
        tree_hash: None,
      })
    }
    Source::Synthetic(config) => {
      let generated = generate::generate(config, dir)?;
      Ok(Materialised {
        projects: synthetic_projects(&generated.projects)?,
        kind: "synthetic",
        commit: None,
        tree_hash: Some(generated.tree_hash),
      })
    }
  }
}

fn prepare(plan: &Plan, work: &Path, corpora_dir: &Path) -> BenchResult<Prepared> {
  let dir = work.join(&plan.name);
  fs::create_dir_all(&dir).map_err(|source| BenchError::io("create", &dir, source))?;
  let materialised = materialise(plan, &dir.join("tree"), corpora_dir)?;
  let (mut files, mut loc, mut repos) = (0, 0, 0);
  let mut paths = Vec::new();
  for (index, project) in materialised.projects.iter().enumerate() {
    let sources = list_sources(project)?;
    files += sources.len() as u64;
    loc += count_loc(&sources)?;
    repos += project.repos.len();
    let path = dir.join(format!("project-{index}.json"));
    let text = serde_json::to_string_pretty(project).map_err(|source| BenchError::json("project", source))?;
    fs::write(&path, text).map_err(|source| BenchError::io("write", &path, source))?;
    paths.push(path);
  }
  Ok(Prepared {
    info: CorpusInfo {
      name: plan.name.clone(),
      kind: materialised.kind.to_owned(),
      commit: materialised.commit,
      repos,
      files,
      loc,
      tree_hash: materialised.tree_hash,
      skipped: None,
    },
    projects: paths,
    loc,
    synthetic: materialised.kind == "synthetic",
  })
}

fn usage_metrics(record: ScenarioRecord, usage: &Usage) -> ScenarioRecord {
  record
    .metric("peak_rss_bytes", usage.peak_rss_bytes as f64)
    .metric("cpu_user_s", usage.user_s)
    .metric("cpu_sys_s", usage.sys_s)
    .metric("cpu_total_s", usage.cpu_s())
    .metric("cpu_utilisation", usage.cpu_s() / usage.wall_s.max(f64::EPSILON))
    .metric("process_wall_s", usage.wall_s)
}

fn worker_args(prepared: &Prepared, plan: &Plan, mode: WorkerMode, home: &Path, settings: &Settings, work: &Path) -> BenchResult<Vec<String>> {
  let spec = WorkerSpec {
    mode,
    corpus: plan.name.clone(),
    projects: prepared.projects.clone(),
    home: home.to_path_buf(),
    loc: prepared.loc,
    synthetic: prepared.synthetic,
    settings: settings.clone(),
  };
  let spec_path = work
    .join(&plan.name)
    .join(format!("worker-{}-{}.json", mode.name(), home.file_name().and_then(|name| name.to_str()).unwrap_or("home")));
  let text = serde_json::to_string(&spec).map_err(|source| BenchError::json("worker spec", source))?;
  fs::write(&spec_path, text).map_err(|source| BenchError::io("write", &spec_path, source))?;
  Ok(vec!["worker".to_owned(), "--spec".to_owned(), spec_path.display().to_string()])
}

fn current_exe() -> BenchResult<PathBuf> {
  std::env::current_exe().map_err(|source| BenchError::io("resolve", Path::new("current_exe"), source))
}

fn run_worker(prepared: &Prepared, plan: &Plan, mode: WorkerMode, home: &Path, settings: &Settings, work: &Path) -> BenchResult<(Vec<ScenarioRecord>, Usage)> {
  let args = worker_args(prepared, plan, mode, home, settings, work)?;
  let (stdout, usage) = run_measured(&current_exe()?, &args, mode.name())?;
  let records = serde_json::from_str(&stdout).map_err(|source| BenchError::json(format!("{} worker output", mode.name()), source))?;
  Ok((records, usage))
}

fn aggregate(runs: Vec<ScenarioRecord>) -> ScenarioRecord {
  let mut merged = runs[0].clone();
  let mut spread = serde_json::Map::new();
  for name in runs[0].metrics.keys() {
    let samples: Vec<f64> = runs.iter().filter_map(|run| run.metrics.get(name).copied()).collect();
    let (low, high) = range(&samples);
    spread.insert(name.clone(), serde_json::json!([low, high]));
    merged.metrics.insert(name.clone(), median(&samples));
  }
  merged.info.insert("repeats".to_owned(), serde_json::json!(runs.len()));
  merged.info.insert("spread".to_owned(), serde_json::Value::Object(spread));
  merged
}

fn run_corpus(plan: &Plan, prepared: &Prepared, work: &Path, settings: &Settings) -> Vec<ScenarioRecord> {
  let home = work.join(&plan.name).join("home");
  let first_mode = if plan.depth == Depth::Multi { WorkerMode::Multi } else { WorkerMode::Cold };
  let repeats = if first_mode == WorkerMode::Cold { settings.repeats } else { 1 };
  let mut runs = Vec::new();
  for _ in 0..repeats {
    let _ = fs::remove_dir_all(&home);
    match run_worker(prepared, plan, first_mode, &home, settings, work) {
      Ok((mut records, usage)) => {
        if let Some(first) = records.first_mut() {
          *first = usage_metrics(first.clone(), &usage);
          runs.push(first.clone());
        }
      }
      Err(error) => return vec![ScenarioRecord::failed(&plan.name, first_mode.name(), error.to_string())],
    }
  }
  let Some(_) = runs.first() else { return Vec::new() };
  let mut records = vec![aggregate(runs)];
  if plan.depth != Depth::Session {
    return records;
  }
  match run_worker(prepared, plan, WorkerMode::Session, &home, settings, work) {
    Ok((session, usage)) => {
      records.extend(session);
      records.push(usage_metrics(ScenarioRecord::ok(&plan.name, "session_process"), &usage));
    }
    Err(error) => records.push(ScenarioRecord::failed(&plan.name, "session", error.to_string())),
  }
  records
}

fn profile_target(suite: Suite) -> &'static str {
  match suite {
    Suite::Quick => "synth-100k",
    Suite::Full => "synth-1m",
  }
}

fn profile_cold(plan: &Plan, prepared: &Prepared, work: &Path, settings: &Settings, dir: &Path) -> BenchResult<ProfileInfo> {
  fs::create_dir_all(dir).map_err(|source| BenchError::io("create", dir, source))?;
  let home = work.join(&plan.name).join("home-profile");
  let args = worker_args(prepared, plan, WorkerMode::Cold, &home, settings, work)?;
  profile::sample_command(&current_exe()?, &args, &dir.join(format!("perf-profile-{}.txt", plan.name)))
}

fn skipped_corpus(plan: &Plan, reason: String) -> (CorpusInfo, Vec<ScenarioRecord>) {
  let kind = match plan.source {
    Source::Pinned(_) => "external",
    _ => "unknown",
  };
  (
    CorpusInfo {
      name: plan.name.clone(),
      kind: kind.to_owned(),
      commit: match &plan.source {
        Source::Pinned(pinned) => Some(pinned.commit.to_owned()),
        _ => None,
      },
      repos: 0,
      files: 0,
      loc: 0,
      tree_hash: None,
      skipped: Some(reason.clone()),
    },
    vec![ScenarioRecord::skipped(&plan.name, "cold_index", reason)],
  )
}

pub struct RunOptions {
  pub profile_dir: Option<PathBuf>,
  pub suite: Suite,
  pub only: Vec<String>,
  pub work_dir: Option<PathBuf>,
  pub corpora_dir: Option<PathBuf>,
  pub previous: Option<PathBuf>,
}

pub fn run(options: &RunOptions) -> BenchResult<PerfReport> {
  let started = Instant::now();
  let scratch = tempfile::tempdir().map_err(|source| BenchError::io("create", Path::new("tempdir"), source))?;
  let work = match &options.work_dir {
    Some(dir) => {
      fs::create_dir_all(dir).map_err(|source| BenchError::io("create", dir, source))?;
      dir.clone()
    }
    None => scratch.path().to_path_buf(),
  };
  let corpora_dir = corpus::corpora_dir(options.corpora_dir.as_deref())?;
  fs::create_dir_all(&corpora_dir).map_err(|source| BenchError::io("create", &corpora_dir, source))?;
  let settings = Settings::for_suite();
  let (mut corpora, mut records) = (Vec::new(), Vec::new());
  let mut profile = ProfileInfo {
    tool: None,
    path: None,
    note: "profiling was not requested; the per-phase timings from the run ledger are the profile".to_owned(),
  };
  for plan in plans(options.suite).into_iter().filter(|plan| options.only.is_empty() || options.only.contains(&plan.name)) {
    tracing::info!(corpus = plan.name, "preparing corpus");
    let prepared = match prepare(&plan, &work, &corpora_dir) {
      Ok(prepared) => prepared,
      Err(error) => {
        tracing::warn!(corpus = plan.name, error = %error, "skipping corpus");
        let (info, skipped) = skipped_corpus(&plan, error.to_string());
        corpora.push(info);
        records.extend(skipped);
        continue;
      }
    };
    tracing::info!(corpus = plan.name, files = prepared.info.files, loc = prepared.info.loc, "running scenarios");
    let scenarios = Instant::now();
    records.extend(run_corpus(&plan, &prepared, &work, &settings));
    tracing::info!(corpus = plan.name, seconds = scenarios.elapsed().as_secs_f64(), "corpus finished");
    if let Some(dir) = options.profile_dir.as_ref().filter(|_| plan.name == profile_target(options.suite)) {
      profile = profile_cold(&plan, &prepared, &work, &settings, dir).unwrap_or_else(|error| ProfileInfo {
        tool: None,
        path: None,
        note: format!("profiling failed: {error}"),
      });
    }
    corpora.push(prepared.info);
    if options.work_dir.is_none() || plan.name.starts_with("synth-1m") {
      let _ = fs::remove_dir_all(work.join(&plan.name));
    }
  }
  let mut report = PerfReport {
    schema_version: SCHEMA_VERSION,
    suite: options.suite.name().to_owned(),
    date: today(),
    environment: Environment::detect(),
    corpora,
    records,
    profile,
    total_seconds: started.elapsed().as_secs_f64(),
    comparison: None,
  };
  if let Some(path) = &options.previous {
    let text = fs::read_to_string(path).map_err(|source| BenchError::io("read", path, source))?;
    let previous: PerfReport = serde_json::from_str(&text).map_err(|source| BenchError::json(path.display().to_string(), source))?;
    report.comparison = Some(compare(&previous, &report, REGRESSION_THRESHOLD_PCT).map_err(BenchError::Invalid)?);
  }
  Ok(report)
}

pub fn report_dir(out: &Path) -> PathBuf {
  match out.extension() {
    Some(_) => out.parent().map(Path::to_path_buf).unwrap_or_default(),
    None => out.to_path_buf(),
  }
}

pub fn write_report(report: &PerfReport, out: &Path) -> BenchResult<(PathBuf, PathBuf)> {
  let (json, md) = match out.extension() {
    Some(_) => (out.with_extension("json"), out.with_extension("md")),
    None => (out.join("perf.json"), out.join("perf.md")),
  };
  if let Some(parent) = json.parent() {
    fs::create_dir_all(parent).map_err(|source| BenchError::io("create", parent, source))?;
  }
  let text = serde_json::to_string_pretty(report).map_err(|source| BenchError::json("report", source))?;
  fs::write(&json, text).map_err(|source| BenchError::io("write", &json, source))?;
  fs::write(&md, markdown::render(report)).map_err(|source| BenchError::io("write", &md, source))?;
  Ok((json, md))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_aggregate_repeats_into_medians_with_spread() {
    let runs = vec![
      ScenarioRecord::ok("c", "cold_index").metric("index_ms", 10.0),
      ScenarioRecord::ok("c", "cold_index").metric("index_ms", 30.0),
      ScenarioRecord::ok("c", "cold_index").metric("index_ms", 20.0),
    ];
    let merged = aggregate(runs);
    assert_eq!(merged.metrics["index_ms"], 20.0);
    assert_eq!(merged.info["spread"]["index_ms"], serde_json::json!([10.0, 30.0]));
    assert_eq!(merged.info["repeats"], serde_json::json!(3));
  }

  #[test]
  fn should_include_every_external_corpus_only_in_the_full_suite() {
    let names = |suite| plans(suite).into_iter().map(|plan| plan.name).collect::<Vec<_>>();
    assert!(!names(Suite::Quick).contains(&"hono".to_owned()));
    assert!(names(Suite::Full).contains(&"hono".to_owned()));
    assert!(names(Suite::Full).contains(&"ripgrep".to_owned()));
    assert!(names(Suite::Full).contains(&"synth-1m".to_owned()));
  }

  #[test]
  fn should_include_self_fixtures_and_100k_in_the_quick_suite() {
    let names: Vec<String> = plans(Suite::Quick).into_iter().map(|plan| plan.name).collect();
    assert_eq!(names, vec!["self", "fixtures-acme", "fixtures-globex", "synth-100k", "synth-multi-project"]);
  }

  #[test]
  fn should_write_json_and_markdown_next_to_each_other() {
    let scratch = tempfile::tempdir().unwrap();
    let report = PerfReport {
      schema_version: SCHEMA_VERSION,
      suite: "quick".to_owned(),
      date: "2026-10-04".to_owned(),
      environment: Environment::detect(),
      corpora: Vec::new(),
      records: Vec::new(),
      profile: ProfileInfo {
        tool: None,
        path: None,
        note: String::new(),
      },
      total_seconds: 0.0,
      comparison: None,
    };
    let (json, md) = write_report(&report, &scratch.path().join("2026-10-04")).unwrap();
    assert!(json.ends_with("2026-10-04/perf.json") && json.exists());
    assert!(md.ends_with("2026-10-04/perf.md") && md.exists());
    let (json, md) = write_report(&report, &scratch.path().join("x/perf.json")).unwrap();
    assert!(json.ends_with("x/perf.json") && md.ends_with("x/perf.md"));
  }
}
