use std::fs;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use atlas_core::{ContentHash, ProjectName, RelPath, RepoName};
use atlas_project::{Project, ProjectSettings, RepoConfig};
use tempfile::TempDir;

use super::*;
use crate::plan::TextEdit;

struct Repo {
  _dir: TempDir,
  root: PathBuf,
  project: Project,
}

impl Repo {
  fn new() -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap().join("api");
    fs::create_dir_all(&root).unwrap();
    let project = Project {
      name: ProjectName::new("demo").unwrap(),
      settings: ProjectSettings::default(),
      repos: vec![RepoConfig {
        name: RepoName::new("api").unwrap(),
        path: root.clone(),
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      }],
    };
    Self { _dir: dir, root, project }
  }

  fn write(&self, path: &str, content: &str) -> ContentHash {
    fs::write(self.root.join(path), content).unwrap();
    ContentHash::of(content.as_bytes())
  }

  fn read(&self, path: &str) -> Option<String> {
    fs::read_to_string(self.root.join(path)).ok()
  }

  fn scope(&self) -> EditScope {
    EditScope::new(&self.project).unwrap()
  }

  fn options(&self) -> ApplyOptions {
    ApplyOptions::new(self.root.parent().unwrap().join("db.edit.lock"))
  }
}

fn target(path: &str) -> FileTarget {
  FileTarget::new(RepoName::new("api").unwrap(), RelPath::new(path).unwrap())
}

fn upper(path: &str, expected: ContentHash, len: usize, text: &str) -> FileEdits {
  FileEdits::new(target(path), expected, vec![TextEdit::replace(0..len, text)])
}

#[derive(Default)]
struct Injected {
  fail_commit: Option<usize>,
  fail_restore: Option<usize>,
  on_commit: Option<Box<dyn Fn(usize)>>,
}

impl Faults for Injected {
  fn before_commit(&self, step: usize) -> io::Result<()> {
    if let Some(hook) = &self.on_commit {
      hook(step);
    }
    if self.fail_commit == Some(step) { Err(io::Error::other("injected")) } else { Ok(()) }
  }

  fn before_restore(&self, step: usize) -> io::Result<()> {
    if self.fail_restore == Some(step) { Err(io::Error::other("injected")) } else { Ok(()) }
  }
}

fn three_files_and_resources(repo: &Repo) -> EditPlan {
  let a = repo.write("a.ts", "a");
  let b = repo.write("b.ts", "b");
  let c = repo.write("c.ts", "c");
  let e = repo.write("e.ts", "e");
  EditPlan {
    files: vec![upper("a.ts", a, 1, "A"), upper("b.ts", b, 1, "B"), upper("c.ts", c, 1, "C")],
    resources: vec![
      ResourceOp::Create {
        file: target("d.ts"),
        content: "d".to_owned(),
      },
      ResourceOp::Rename {
        from: target("e.ts"),
        to: target("f.ts"),
        expected: e,
      },
    ],
  }
}

#[test]
fn should_roll_back_every_file_when_a_commit_fails_midway() {
  let repo = Repo::new();
  let plan = three_files_and_resources(&repo);
  let faults = Injected {
    fail_commit: Some(4),
    ..Injected::default()
  };
  let error = apply_with(&plan, &repo.scope(), &repo.options(), &faults).unwrap_err();
  let EditError::RolledBack { cause } = error else {
    panic!("expected RolledBack, got {error:?}")
  };
  assert_eq!(cause.to_string(), format!("commit {}: injected", repo.root.join("e.ts").display()));
  assert_eq!(
    ["a.ts", "b.ts", "c.ts", "d.ts", "e.ts", "f.ts"].map(|path| repo.read(path)),
    [Some("a".to_owned()), Some("b".to_owned()), Some("c".to_owned()), None, Some("e".to_owned()), None]
  );
}

#[test]
fn should_reverse_a_committed_rename_during_rollback() {
  let repo = Repo::new();
  let mut plan = three_files_and_resources(&repo);
  plan.resources.push(ResourceOp::Create {
    file: target("g.ts"),
    content: "g".to_owned(),
  });
  let faults = Injected {
    fail_commit: Some(5),
    ..Injected::default()
  };
  assert!(matches!(apply_with(&plan, &repo.scope(), &repo.options(), &faults), Err(EditError::RolledBack { .. })));
  assert_eq!(
    ["a.ts", "d.ts", "e.ts", "f.ts", "g.ts"].map(|path| repo.read(path)),
    [Some("a".to_owned()), None, Some("e".to_owned()), None, None]
  );
}

#[test]
fn should_report_partial_application_when_rollback_fails() {
  let repo = Repo::new();
  let plan = three_files_and_resources(&repo);
  let faults = Injected {
    fail_commit: Some(2),
    fail_restore: Some(0),
    ..Injected::default()
  };
  let error = apply_with(&plan, &repo.scope(), &repo.options(), &faults).unwrap_err();
  let EditError::PartiallyApplied { applied, failed, .. } = error else {
    panic!("expected PartiallyApplied, got {error:?}")
  };
  assert_eq!(applied, ["api:b.ts"]);
  assert_eq!(failed, ["api:b.ts: roll back api:b.ts: injected"]);
  assert_eq!(
    ["a.ts", "b.ts", "c.ts"].map(|path| repo.read(path)),
    [Some("a".to_owned()), Some("B".to_owned()), Some("c".to_owned())]
  );
}

#[test]
fn should_roll_back_when_a_file_changes_between_prepare_and_commit() {
  let repo = Repo::new();
  let plan = three_files_and_resources(&repo);
  let path = repo.root.join("b.ts");
  let faults = Injected {
    on_commit: Some(Box::new(move |step| {
      if step == 1 {
        fs::write(&path, "external").unwrap();
      }
    })),
    ..Injected::default()
  };
  let error = apply_with(&plan, &repo.scope(), &repo.options(), &faults).unwrap_err();
  let EditError::RolledBack { cause } = error else {
    panic!("expected RolledBack, got {error:?}")
  };
  assert!(matches!(*cause, EditError::Stale { ref paths } if paths == &["api:b.ts"]), "{cause:?}");
  assert_eq!(
    ["a.ts", "b.ts", "c.ts", "d.ts"].map(|path| repo.read(path)),
    [Some("a".to_owned()), Some("external".to_owned()), Some("c".to_owned()), None]
  );
}

#[test]
fn should_serialize_concurrent_applies_with_the_edit_lock() {
  let repo = Repo::new();
  let original = repo.write("a.ts", "a");
  let scope = repo.scope();
  let options = repo.options();
  let first_plan = EditPlan {
    files: vec![upper("a.ts", original, 1, "first")],
    resources: Vec::new(),
  };
  let second_plan = EditPlan {
    files: vec![upper("a.ts", original, 1, "second")],
    resources: Vec::new(),
  };
  let (inside, entered) = mpsc::channel();
  let (first, second) = thread::scope(|threads| {
    let first = threads.spawn(|| {
      let faults = Injected {
        on_commit: Some(Box::new(move |_| {
          inside.send(()).unwrap();
          thread::sleep(Duration::from_millis(150));
        })),
        ..Injected::default()
      };
      apply_with(&first_plan, &scope, &options, &faults)
    });
    entered.recv().unwrap();
    let second = threads.spawn(|| apply(&second_plan, &scope, &options));
    (first.join().unwrap(), second.join().unwrap())
  });
  assert_eq!(first.unwrap().changes.len(), 1);
  assert!(matches!(second, Err(EditError::Stale { ref paths }) if paths == &["api:a.ts"]), "{second:?}");
  assert_eq!(repo.read("a.ts"), Some("first".to_owned()));
}

#[test]
fn should_report_a_left_destination_when_both_unlinks_fail() {
  let failing = |_: &Path| -> io::Result<()> { Err(io::Error::other("busy")) };
  let failure = unlink_source(Path::new("/a"), Path::new("/b"), &failing).unwrap_err();
  assert_eq!((failure.error.to_string(), failure.destination_left), ("busy".to_owned(), true));
  let source_only = |path: &Path| -> io::Result<()> { if path == Path::new("/a") { Err(io::Error::other("busy")) } else { Ok(()) } };
  assert!(!unlink_source(Path::new("/a"), Path::new("/b"), &source_only).unwrap_err().destination_left);
  assert!(unlink_source(Path::new("/a"), Path::new("/b"), &|_| Ok(())).is_ok());
}

#[test]
fn should_roll_back_a_partially_moved_rename_through_the_journal() {
  let repo = Repo::new();
  repo.write("e.ts", "e");
  let from = repo.scope().locate(&target("e.ts")).unwrap();
  let to = repo.scope().resolve(&repo.root.join("f.ts")).unwrap();
  fs::hard_link(&from.absolute, &to.absolute).unwrap();
  let journal = vec![Undo::RemoveCreated { target: to }];
  let cause = EditError::Stale {
    paths: vec!["api:e.ts".to_owned()],
  };
  let partial = rollback(
    journal,
    cause,
    &Injected {
      fail_restore: Some(0),
      ..Injected::default()
    },
    ".atlas-edit.test.",
  );
  assert!(matches!(&partial, EditError::PartiallyApplied { applied, .. } if applied == &["api:f.ts"]), "{partial:?}");
  let journal = vec![Undo::RemoveCreated {
    target: repo.scope().resolve(&repo.root.join("f.ts")).unwrap(),
  }];
  let restored = rollback(journal, EditError::Stale { paths: Vec::new() }, &Injected::default(), ".atlas-edit.test.");
  assert!(matches!(restored, EditError::RolledBack { .. }));
  assert_eq!((repo.read("e.ts"), repo.read("f.ts")), (Some("e".to_owned()), None));
}

#[test]
fn should_only_sweep_temporary_files_of_its_own_lock() {
  let repo = Repo::new();
  let options = repo.options();
  let own = format!("{}abc", temp_prefix(&options.lock_path));
  let other = format!("{}abc", temp_prefix(&repo.root.join("other.db.edit.lock")));
  assert_ne!(own, other);
  fs::write(repo.root.join(&own), "stale").unwrap();
  fs::write(repo.root.join(&other), "foreign").unwrap();
  let expected = repo.write("a.ts", "a");
  let plan = EditPlan {
    files: vec![upper("a.ts", expected, 1, "A")],
    resources: Vec::new(),
  };
  let report = apply(&plan, &repo.scope(), &options).unwrap();
  assert_eq!(report.swept, [repo.root.join(&own)]);
  assert_eq!(repo.read(&other), Some("foreign".to_owned()));
}
