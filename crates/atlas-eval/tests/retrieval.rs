use std::path::PathBuf;

use atlas_eval::retrieval::{Phrase, PhraseSource, RetrievalArgs, RetrievalError, RetrievalOptions, RetrievalReport, TaskOutcome, TaskRetrieval, execute_retrieval, run_retrieval};

fn workspace_root() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn run_t00(work: &tempfile::TempDir) -> RetrievalReport {
  let root = workspace_root();
  let options = RetrievalOptions {
    tasks_dir: root.join("eval/tasks"),
    repo_root: root,
    only: vec!["t00-smoke".to_owned()],
    cache_dir: work.path().join("cache"),
    work_dir: work.path().join("work"),
  };
  run_retrieval(&options).unwrap()
}

fn scored(report: &RetrievalReport) -> &TaskRetrieval {
  match &report.tasks[0].outcome {
    TaskOutcome::Scored(retrieval) => retrieval,
    TaskOutcome::Failed { stage, reason } => panic!("t00 failed at {stage}: {reason}"),
  }
}

fn phrase(text: &str, source: PhraseSource) -> Phrase {
  Phrase { text: text.to_owned(), source }
}

#[test]
fn should_score_the_smoke_task_from_its_prompt_and_reproduce_the_report() {
  let work = tempfile::tempdir().unwrap();
  let report = run_t00(&work);
  assert_eq!(report.tasks.len(), 1);
  let retrieval = scored(&report);
  assert_eq!(retrieval.index.repos, vec!["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  assert_eq!(retrieval.index.degraded, None);
  assert_eq!(
    retrieval.queries.search_phrases,
    vec![
      phrase("deletedAt", PhraseSource::CodeSpan),
      phrase("User", PhraseSource::CodeSpan),
      phrase("deleted_at", PhraseSource::CodeSpan),
      phrase("UsersService.findOne", PhraseSource::CodeSpan),
      phrase("database column named", PhraseSource::Prose),
      phrase("soft-delete column decorator", PhraseSource::Prose),
      phrase("change minimal", PhraseSource::Prose),
    ]
  );
  let must_change = &retrieval.must_change;
  assert_eq!(must_change.expected, vec!["acme-api/src/users/user.entity.ts"]);
  assert_eq!(must_change.absent_at_base, Vec::<String>::new());
  assert_eq!(must_change.channels.search.hits, 1);
  assert_eq!(must_change.channels.any.recall, Some(1.0));
  assert_eq!(must_change.missed, Vec::<String>::new());
  assert_eq!(retrieval.must_consider.channels.must_read.recall, Some(1.0));
  assert_eq!(retrieval.relevant_files.channels.any.recall, Some(1.0));
  assert_eq!(retrieval.reuse, None);
  assert_eq!(retrieval.blast_radius, None);
  assert_eq!(retrieval.pack.budget, report.settings.context_budget);
  assert!(retrieval.pack.bytes > 0 && retrieval.pack.estimated_tokens <= retrieval.pack.budget);
  let again = run_t00(&tempfile::tempdir().unwrap());
  assert_eq!(serde_json::to_string(&again).unwrap(), serde_json::to_string(&report).unwrap());
}

#[test]
fn should_write_the_report_and_fail_when_a_task_cannot_be_scored() {
  let work = tempfile::tempdir().unwrap();
  let task_dir = work.path().join("tasks/t98-unreachable");
  std::fs::create_dir_all(&task_dir).unwrap();
  std::fs::write(
    task_dir.join("task.toml"),
    "id = \"t98-unreachable\"\ntitle = \"Unreachable\"\nrepo = { fixture = \"acme\" }\nbase_commit = \"deadbeef\"\nprompt = \"p\"\ntimeout_minutes = 1\nmax_turns = 1\n",
  )
  .unwrap();
  let repo_root = work.path().join("not-a-repo");
  std::fs::create_dir_all(&repo_root).unwrap();
  let args = RetrievalArgs {
    repo_root: None,
    tasks: Some(work.path().join("tasks")),
    only: Vec::new(),
    out: Some(work.path().join("out/retrieval")),
    cache_dir: Some(work.path().join("cache")),
    work_dir: Some(work.path().join("work")),
  };
  let error = execute_retrieval(&args, &repo_root).unwrap_err();
  let RetrievalError::TasksFailed { failed, total, report } = error else {
    panic!("{error:?}")
  };
  assert_eq!((failed, total), (1, 1));
  assert_eq!(report, work.path().join("out/retrieval.md"));
  let markdown = std::fs::read_to_string(&report).unwrap();
  assert!(
    markdown.contains("## t98-unreachable: Unreachable\n\nCategories: .\n\nNot scored: `materialize` failed:"),
    "{markdown}"
  );
  assert_eq!(std::fs::read_dir(work.path().join("work")).unwrap().count(), 0);
}
