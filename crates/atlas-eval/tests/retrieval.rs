use std::path::{Path, PathBuf};

use atlas_eval::agent::AtlasConfig;
use atlas_eval::retrieval::{
  GoldFile, LineGoldSummary, NO_RECORDED_QUERY, Phrase, PhraseSource, Provenance, QueryScore, QueryVariant, RetrievalArgs, RetrievalError, RetrievalOptions, RetrievalReport,
  SeededConfig, TaskOutcome, TaskRetrieval, VariantOutcome, VariantQuery, execute_retrieval, render_markdown, run_retrieval,
};
use atlas_test_lsp::hermetic::catl_binary;

fn workspace_root() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn run_task(work: &tempfile::TempDir, tasks_dir: PathBuf, id: &str) -> RetrievalReport {
  run_configured(work, tasks_dir, id, None)
}

fn run_configured(work: &tempfile::TempDir, tasks_dir: PathBuf, id: &str, atlas_config: Option<SeededConfig>) -> RetrievalReport {
  let options = RetrievalOptions {
    tasks_dir,
    repo_root: workspace_root(),
    only: vec![id.to_owned()],
    cache_dir: work.path().join("cache"),
    work_dir: work.path().join("work"),
    atlas_config,
  };
  run_retrieval(&options).unwrap()
}

fn run_t00(work: &tempfile::TempDir) -> RetrievalReport {
  run_task(work, workspace_root().join("eval/tasks"), "t00-smoke")
}

fn only_query(retrieval: &TaskRetrieval, variant: QueryVariant) -> &QueryScore {
  let queries = retrieval.queries(variant);
  assert_eq!(queries.len(), 1, "{variant:?}");
  &queries[0]
}

fn directed_run(run: &str) -> Provenance {
  Provenance {
    batch: "2026-10-05-directed".to_owned(),
    run: run.to_owned(),
  }
}

const SMOKE_BASE_COMMIT: &str = "e5209b4c5c99df0e0e5cdab2bd885b0821b91927";

fn write_fixture_task(tasks: &Path, id: &str, prompt: &str, reference_patch: Option<&str>) {
  let dir = tasks.join(id);
  std::fs::create_dir_all(&dir).unwrap();
  let spec = format!(
    "id = \"{id}\"\ntitle = \"Synthetic\"\nrepo = {{ fixture = \"acme\" }}\nbase_commit = \"{SMOKE_BASE_COMMIT}\"\nprompt = \"{prompt}\"\ntimeout_minutes = 1\nmax_turns = 1\n\
atlas_repos = [\"acme-api\", \"acme-sdk\", \"acme-web\", \"acme-billing\"]\n\n[ground_truth]\nrelevant_files = [\"acme-api/src/users/users.service.ts\"]\n\
must_change = [\"acme-api/src/users/users.service.ts\"]\n"
  );
  std::fs::write(dir.join("task.toml"), spec).unwrap();
  if let Some(patch) = reference_patch {
    std::fs::write(dir.join("reference.patch"), patch).unwrap();
  }
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
  let task = scored(&report);
  assert_eq!(task.index.repos, vec!["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  assert_eq!(task.index.degraded, None);
  let retrieval = only_query(task, QueryVariant::Full);
  assert_eq!(
    retrieval.searches.search_phrases,
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
  assert_eq!(task.blast_radius, None);
  assert_eq!(retrieval.pack.budget, report.settings.context_budget);
  assert!(retrieval.pack.bytes > 0 && retrieval.pack.estimated_tokens <= retrieval.pack.budget);
  assert_eq!(retrieval.yields.iter().map(|at| at.budget).collect::<Vec<_>>(), vec![4_000, 8_000, 16_000]);
  assert_eq!(retrieval.yields[0].must_read.recall, Some(1.0));
  assert!(retrieval.yields.iter().all(|at| at.estimated_tokens <= at.budget && at.bytes > 0));
  assert_eq!(
    task.line_gold,
    LineGoldSummary::Unavailable {
      reason: "the task has no readable reference.patch".to_owned()
    }
  );
  assert_eq!(retrieval.lines, None);
  let again = run_t00(&tempfile::tempdir().unwrap());
  assert_eq!(serde_json::to_string(&again).unwrap(), serde_json::to_string(&report).unwrap());
  assert_eq!(render_markdown(&again), render_markdown(&report));
  let reread: RetrievalReport = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
  assert_eq!(scored(&reread).variants.iter().map(|result| result.variant).collect::<Vec<_>>(), QueryVariant::ALL);
  assert_eq!(only_query(scored(&reread), QueryVariant::Full).query, retrieval.query);
}

#[test]
fn should_replay_the_recorded_agent_call_with_and_without_its_seeds() {
  let work = tempfile::tempdir().unwrap();
  let report = run_t00(&work);
  let task = scored(&report);
  let text = "Add nullable deletedAt soft-delete column to User entity in acme-api and return it from UsersService.findOne";
  let sent_by = vec![directed_run("t00-smoke.atlas.r0"), directed_run("t00-smoke.atlas.r1")];
  assert_eq!(
    only_query(task, QueryVariant::Agent).query,
    VariantQuery {
      text: text.to_owned(),
      seeds: Vec::new(),
      sent_by: sent_by.clone(),
    }
  );
  let seeded = only_query(task, QueryVariant::AgentSeeded);
  assert_eq!(
    seeded.query,
    VariantQuery {
      text: text.to_owned(),
      seeds: vec!["UsersService.findOne".to_owned(), "User".to_owned()],
      sent_by,
    }
  );
  assert_eq!(seeded.searches, only_query(task, QueryVariant::Agent).searches);
  assert_eq!(
    seeded.searches.search_phrases,
    vec![
      phrase("nullable deletedAt soft-delete column", PhraseSource::Prose),
      phrase("User entity", PhraseSource::Prose),
    ]
  );
  assert_eq!(
    only_query(task, QueryVariant::FirstLine).query.text,
    "In acme-api, add a nullable `deletedAt` timestamp to the `User` entity, stored in a database column named `deleted_at` (use TypeORM's soft-delete column decorator)"
  );
  assert_eq!(task.no_call, Vec::new());
  let agent_metrics: Vec<usize> = report
    .aggregate
    .iter()
    .filter(|mean| mean.metric.starts_with("agent.must_change."))
    .map(|mean| mean.tasks)
    .collect();
  assert_eq!(agent_metrics, vec![1; 10]);
}

#[test]
fn should_report_agent_variants_as_not_available_for_a_task_without_recorded_queries() {
  let work = tempfile::tempdir().unwrap();
  let tasks = work.path().join("tasks");
  write_fixture_task(&tasks, "t96-unrecorded", "Make UsersService.findOne reject blank ids. Keep the change minimal.", None);

  let report = run_task(&work, tasks, "t96-unrecorded");

  let task = scored(&report);
  let outcome = |variant: QueryVariant| &task.variants.iter().find(|result| result.variant == variant).unwrap().outcome;
  let unavailable = VariantOutcome::Unavailable {
    reason: NO_RECORDED_QUERY.to_owned(),
  };
  assert_eq!(outcome(QueryVariant::Agent), &unavailable);
  assert_eq!(outcome(QueryVariant::AgentSeeded), &unavailable);
  assert_eq!(only_query(task, QueryVariant::FirstLine).query.text, "Make UsersService.findOne reject blank ids");
  assert_eq!(only_query(task, QueryVariant::Full).must_change.channels.must_read.recall, Some(1.0));
  let mean = |metric: &str| report.aggregate.iter().find(|mean| mean.metric == metric).map(|mean| (mean.tasks, mean.mean)).unwrap();
  assert_eq!(mean("agent.must_change.recall.must_read"), (0, None));
  assert_eq!(mean("agent-seeded.relevant_files.f05.pack"), (0, None));
  assert_eq!(mean("full.must_change.recall.must_read"), (1, Some(1.0)));
  let markdown = render_markdown(&report);
  assert!(
    markdown.contains("| agent | MUST READ | n/a (0) | n/a (0) | n/a (0) | n/a (0) | n/a (0) | n/a (0) | n/a (0) |\n"),
    "{markdown}"
  );
  assert!(
    markdown.contains("### `agent`\n\nn/a: no recorded `atlas_context` call: queries.toml is missing or lists none.\n"),
    "{markdown}"
  );
}

const LINES_PATCH: &str = "diff --git a/acme-api/src/users/users.service.ts b/acme-api/src/users/users.service.ts
--- a/acme-api/src/users/users.service.ts
+++ b/acme-api/src/users/users.service.ts
@@ -17,6 +17,7 @@ export class UsersService {
 
   async findOne(id: string): Promise<User> {
-    const user = await this.users.findOneBy({ id });
+    if (!id.trim()) throw new NotFoundException('blank id');
+    const user = await this.users.findOneBy({ id: id.trim() });
     if (!user) throw new NotFoundException(`User ${id} not found`);
     return user;
   }
diff --git a/acme-api/tsconfig.json b/acme-api/tsconfig.json
--- a/acme-api/tsconfig.json
+++ b/acme-api/tsconfig.json
@@ -4,4 +4,5 @@
     \"target\": \"ES2022\",
     \"strict\": true,
+    \"noImplicitOverride\": true,
     \"experimentalDecorators\": true,
     \"emitDecoratorMetadata\": true,
diff --git a/expected.toml b/expected.toml
--- a/expected.toml
+++ b/expected.toml
@@ -1,2 +1,1 @@
-gone
 kept
diff --git a/acme-api/src/users/blank.ts b/acme-api/src/users/blank.ts
new file mode 100644
--- /dev/null
+++ b/acme-api/src/users/blank.ts
@@ -0,0 +1 @@
+export const BLANK = '';
";

#[test]
fn should_score_pack_line_spans_against_the_old_side_lines_of_the_reference_patch() {
  let work = tempfile::tempdir().unwrap();
  let tasks = work.path().join("tasks");
  write_fixture_task(&tasks, "t97-lines", "Make `UsersService.findOne` reject blank ids.", Some(LINES_PATCH));

  let report = run_task(&work, tasks, "t97-lines");

  let task = scored(&report);
  assert_eq!(
    task.line_gold,
    LineGoldSummary::Available {
      lines: 3,
      lines_outside_index: 0,
      files: vec![
        GoldFile {
          path: "acme-api/src/users/users.service.ts".to_owned(),
          lines: 1,
          indexed: true
        },
        GoldFile {
          path: "acme-api/tsconfig.json".to_owned(),
          lines: 2,
          indexed: true
        },
      ],
      added_files: vec!["acme-api/src/users/blank.ts".to_owned()],
      absent_files: vec!["expected.toml".to_owned()],
    }
  );
  let lines = only_query(task, QueryVariant::Full).lines.as_ref().unwrap();
  assert_eq!(
    (lines.must_read.lines.relevant, lines.must_read.lines.hits, lines.must_read.lines.recall),
    (3, 1, Some(1.0 / 3.0))
  );
  assert_eq!((lines.pack.lines.relevant, lines.pack.lines.hits), (3, 1));
  assert!(lines.must_read.lines.retrieved >= 5 && lines.must_read.lines.retrieved <= lines.pack.lines.retrieved);
  let retrieved = lines.must_read.lines.retrieved as f64;
  assert_eq!(lines.must_read.f05, Some(1.25 / (0.75 + retrieved)));
  assert_eq!(lines.must_read.lines.precision, Some(1.0 / retrieved));
  assert!(lines.pack.items >= lines.must_read.items && lines.must_read.items > lines.must_read.items_without_span);
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
    atlas_config: None,
    atlas_bin: None,
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

#[test]
fn should_put_the_router_of_the_invented_framework_in_must_read_from_the_first_line_with_its_route_family() {
  let work = tempfile::tempdir().unwrap();
  let tasks = workspace_root().join("eval/tasks");
  let config = SeededConfig {
    config: AtlasConfig::load(&tasks.join("t11-compressed-reply-body/atlas-config")).unwrap(),
    atlas_bin: catl_binary().to_owned(),
  };

  let report = run_configured(&work, tasks, "t11-compressed-reply-body", Some(config));

  let seeded = report.settings.atlas_config.as_ref().unwrap();
  assert_eq!(seeded.files, ["config.toml", "detectors/corvid-http.toml"]);
  let task = scored(&report);
  assert_eq!(task.index.degraded, None);
  let first_line = only_query(task, QueryVariant::FirstLine);
  assert_eq!(first_line.must_change.expected, ["corvid/packages/http/src/module/http-router.ts"]);
  assert_eq!(first_line.must_change.channels.must_read.recall, Some(1.0));
}

#[test]
fn should_put_the_invoker_builder_in_must_read_from_the_first_line_through_the_handlers_parameter_decorator() {
  let work = tempfile::tempdir().unwrap();
  let tasks = workspace_root().join("eval/tasks");
  let config = SeededConfig {
    config: AtlasConfig::load(&tasks.join("t12-deferred-reply-handle/atlas-config")).unwrap(),
    atlas_bin: catl_binary().to_owned(),
  };

  let report = run_configured(&work, tasks, "t12-deferred-reply-handle", Some(config));

  let seeded = report.settings.atlas_config.as_ref().unwrap();
  assert_eq!(seeded.files, ["config.toml", "detectors/corvid-web.toml"]);
  let task = scored(&report);
  assert_eq!(task.index.degraded, None);
  let first_line = only_query(task, QueryVariant::FirstLine);
  assert_eq!(first_line.must_change.expected, ["corvid/packages/web/src/web-router.ts"]);
  assert_eq!(first_line.must_change.channels.must_read.recall, Some(1.0));
}
