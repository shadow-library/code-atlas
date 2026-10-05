mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use atlas_edit::{EditError, Refusal, RefusalReason};
use common::{Workspace, target};

fn reason(workspace: &Workspace, path: &Path) -> RefusalReason {
  workspace.scope().resolve(path).unwrap_err().reason
}

#[test]
fn should_resolve_an_existing_file_to_its_repo_and_relative_path() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "src/a.ts", "x");
  let resolved = workspace.scope().resolve(&workspace.path("api", "src/a.ts")).unwrap();
  assert_eq!(resolved.file, target("api", "src/a.ts"));
  assert_eq!(resolved.absolute, workspace.path("api", "src/a.ts"));
}

#[test]
fn should_refuse_paths_outside_registered_repos() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "src/a.ts", "x");
  let outside = workspace.outside("secret.ts");
  fs::write(&outside, "x").unwrap();
  assert_eq!(reason(&workspace, &outside), RefusalReason::OutsideRepos);
  let escape = workspace.root("api").join("src/../../outside/secret.ts");
  assert_eq!(reason(&workspace, &escape), RefusalReason::OutsideRepos);
  assert_eq!(reason(&workspace, Path::new("src/a.ts")), RefusalReason::NotAbsolute);
  assert_eq!(reason(&workspace, &workspace.root("api").join("src/..")), RefusalReason::InvalidPath);
  assert_eq!(reason(&workspace, &workspace.path("api", "missing/dir/a.ts")), RefusalReason::MissingParent);
  assert_eq!(workspace.scope().locate(&target("web", "a.ts")).unwrap_err().reason, RefusalReason::UnknownRepo);
}

#[test]
fn should_refuse_symlinks_and_gitignored_files() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "src/real.ts", "x");
  let outside = workspace.outside("dir/secret.ts");
  fs::write(&outside, "x").unwrap();
  symlink(workspace.path("api", "src/real.ts"), workspace.path("api", "src/link.ts")).unwrap();
  symlink(outside.parent().unwrap(), workspace.path("api", "linked")).unwrap();
  assert_eq!(reason(&workspace, &workspace.path("api", "src/link.ts")), RefusalReason::Symlink);
  assert_eq!(reason(&workspace, &workspace.path("api", "linked/secret.ts")), RefusalReason::OutsideRepos);

  workspace.write("api", ".gitignore", "generated/\n*.log.ts\n!keep.log.ts\n");
  workspace.write("api", "generated/a.ts", "x");
  workspace.write("api", "src/debug.log.ts", "x");
  workspace.write("api", "src/keep.log.ts", "x");
  workspace.write("api", "src/nested/.gitignore", "local.ts\n");
  workspace.write("api", "src/nested/local.ts", "x");
  workspace.write("api", "src/nested/.ignore", "scratch.ts\n");
  workspace.write("api", "src/nested/scratch.ts", "x");
  workspace.write("api", "src/nested/kept.ts", "x");
  for ignored in ["generated/a.ts", "src/debug.log.ts", "src/nested/local.ts", "src/nested/scratch.ts"] {
    assert_eq!(reason(&workspace, &workspace.path("api", ignored)), RefusalReason::Gitignored, "{ignored}");
  }
  for kept in ["src/keep.log.ts", "src/nested/kept.ts", "src/real.ts"] {
    assert_eq!(workspace.scope().resolve(&workspace.path("api", kept)).unwrap().file, target("api", kept));
  }
}

#[test]
fn should_honour_the_git_info_exclude_file() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", ".git/info/exclude", "private.ts\n");
  workspace.write("api", "private.ts", "x");
  assert_eq!(reason(&workspace, &workspace.path("api", "private.ts")), RefusalReason::Gitignored);
}

#[test]
fn should_refuse_git_metadata_and_default_excluded_directories() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", ".git/config", "x");
  workspace.write("api", "node_modules/pkg/index.ts", "x");
  workspace.write("api", "pkg/dist/a.ts", "x");
  workspace.write("api", "build.rs", "x");
  workspace.write("api", "builder/a.ts", "x");
  assert_eq!(reason(&workspace, &workspace.path("api", ".git/config")), RefusalReason::VcsMetadata(".git".to_owned()));
  assert_eq!(
    reason(&workspace, &workspace.path("api", "node_modules/pkg/index.ts")),
    RefusalReason::DefaultExcluded("node_modules".to_owned())
  );
  assert_eq!(
    reason(&workspace, &workspace.path("api", "pkg/dist/a.ts")),
    RefusalReason::DefaultExcluded("dist".to_owned())
  );
  assert_eq!(workspace.scope().resolve(&workspace.path("api", "build.rs")).unwrap().file, target("api", "build.rs"));
  assert_eq!(
    workspace.scope().resolve(&workspace.path("api", "builder/a.ts")).unwrap().file,
    target("api", "builder/a.ts")
  );
}

#[test]
fn should_admit_default_excluded_directories_that_the_repo_includes() {
  let mut workspace = Workspace::new(&["api"]);
  workspace.project.repos[0].include = vec!["vendor/lib/**".to_owned()];
  workspace.write("api", "vendor/lib/a.ts", "x");
  workspace.write("api", "vendor/other/b.ts", "x");
  assert_eq!(
    workspace.scope().resolve(&workspace.path("api", "vendor/lib/a.ts")).unwrap().file,
    target("api", "vendor/lib/a.ts")
  );
  assert_eq!(
    reason(&workspace, &workspace.path("api", "vendor/other/b.ts")),
    RefusalReason::DefaultExcluded("vendor".to_owned())
  );
}

#[test]
fn should_refuse_directories_and_accept_missing_files_in_existing_directories() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "src/a.ts", "x");
  assert_eq!(reason(&workspace, &workspace.path("api", "src")), RefusalReason::NotAFile);
  assert_eq!(reason(&workspace, &workspace.root("api")), RefusalReason::NotAFile);
  assert_eq!(workspace.scope().resolve(&workspace.path("api", "src/new.ts")).unwrap().file, target("api", "src/new.ts"));
}

#[test]
fn should_resolve_nested_repos_to_the_innermost_root() {
  let mut workspace = Workspace::new(&["outer"]);
  let inner = workspace.root("outer").join("packages/inner");
  fs::create_dir_all(&inner).unwrap();
  workspace.project.repos.push(atlas_project::RepoConfig {
    name: atlas_core::RepoName::new("inner").unwrap(),
    path: inner.clone(),
    include: Vec::new(),
    exclude: Vec::new(),
    languages: None,
  });
  fs::write(inner.join("a.ts"), "x").unwrap();
  assert_eq!(workspace.scope().resolve(&inner.join("a.ts")).unwrap().file, target("inner", "a.ts"));
}

#[test]
fn should_refuse_the_whole_set_when_one_path_is_refused() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "a.ts", "x");
  workspace.write("api", ".gitignore", "b.ts\n");
  workspace.write("api", "b.ts", "x");
  let paths: Vec<PathBuf> = vec![workspace.path("api", "a.ts"), workspace.path("api", "b.ts")];
  let error = workspace.scope().resolve_all(paths.iter().map(PathBuf::as_path)).unwrap_err();
  let EditError::OutOfScope { refused } = error else {
    panic!("expected OutOfScope, got {error:?}")
  };
  assert_eq!(
    refused,
    [Refusal {
      path: workspace.path("api", "b.ts"),
      reason: RefusalReason::Gitignored,
    }]
  );
}

#[test]
fn should_refuse_case_aliases_of_version_control_metadata() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", ".git/config", "[core]\n");
  assert_eq!(reason(&workspace, &workspace.path("api", ".GIT/config")), RefusalReason::VcsMetadata(".git".to_owned()));
  for vcs in [".hg/hgrc", ".svn/entries", ".jj/repo", ".Hg/new.txt"] {
    workspace.write("api", vcs, "x");
    let refused = reason(&workspace, &workspace.path("api", vcs));
    assert!(matches!(refused, RefusalReason::VcsMetadata(_)), "{vcs}: {refused:?}");
  }
}

#[test]
fn should_refuse_a_worktree_git_file_under_any_case() {
  let workspace = Workspace::new(&["web"]);
  workspace.write("web", ".git", "gitdir: /real/.git/worktrees/web\n");
  assert_eq!(reason(&workspace, &workspace.path("web", ".git")), RefusalReason::VcsMetadata(".git".to_owned()));
  let alias = reason(&workspace, &workspace.path("web", ".GIT"));
  assert!(matches!(&alias, RefusalReason::VcsMetadata(name) if name.eq_ignore_ascii_case(".git")), "{alias:?}");
}

#[test]
fn should_resolve_case_aliases_to_the_on_disk_name_before_checking_ignores() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", ".gitignore", "secret.env\n");
  workspace.write("api", "secret.env", "TOKEN=a\n");
  workspace.write("api", "Real.ts", "x");
  let case_insensitive = workspace.path("api", "real.ts").exists();
  if case_insensitive {
    assert_eq!(reason(&workspace, &workspace.path("api", "Secret.env")), RefusalReason::Gitignored);
    assert_eq!(workspace.scope().resolve(&workspace.path("api", "real.ts")).unwrap().file, target("api", "Real.ts"));
  } else {
    assert_eq!(workspace.scope().resolve(&workspace.path("api", "Secret.env")).unwrap().file, target("api", "Secret.env"));
  }
  assert_eq!(reason(&workspace, &workspace.path("api", "secret.env")), RefusalReason::Gitignored);
}

#[test]
fn should_refuse_backslashes_in_file_names() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", "a\\b.ts", "back");
  workspace.write("api", "a/b.ts", "front");
  assert_eq!(reason(&workspace, &workspace.path("api", "a\\b.ts")), RefusalReason::BackslashInName);
  assert_eq!(workspace.scope().resolve(&workspace.path("api", "a/b.ts")).unwrap().file, target("api", "a/b.ts"));
}

#[test]
fn should_not_re_include_files_below_an_ignored_directory() {
  let workspace = Workspace::new(&["api"]);
  workspace.write("api", ".gitignore", "generated/\n!generated/keep.ts\nout/*\n!out/keep.ts\n");
  workspace.write("api", "generated/keep.ts", "x");
  workspace.write("api", "out/keep.ts", "x");
  workspace.write("api", "out/drop.ts", "x");
  assert_eq!(reason(&workspace, &workspace.path("api", "generated/keep.ts")), RefusalReason::Gitignored);
  assert_eq!(workspace.scope().resolve(&workspace.path("api", "out/keep.ts")).unwrap().file, target("api", "out/keep.ts"));
  assert_eq!(reason(&workspace, &workspace.path("api", "out/drop.ts")), RefusalReason::Gitignored);
}
