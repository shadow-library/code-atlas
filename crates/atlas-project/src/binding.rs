use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use atlas_core::{ProjectName, RepoName};

use crate::registry::{Project, RepoConfig};
use crate::worktree::{CheckoutKind, GitCheckout, GitLocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutRole {
  MainCheckout,
  OtherCheckout,
}

impl CheckoutRole {
  fn described(self) -> &'static str {
    match self {
      Self::MainCheckout => "main checkout",
      Self::OtherCheckout => "indexed checkout",
    }
  }

  fn indexed(self) -> &'static str {
    match self {
      Self::MainCheckout => "main checkout",
      Self::OtherCheckout => "checkout",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeOf {
  pub project: ProjectName,
  pub repo: RepoName,
  pub worktree: PathBuf,
  pub indexed_checkout: PathBuf,
  pub indexed_role: CheckoutRole,
}

impl WorktreeOf {
  pub fn caveat(&self) -> String {
    format!(
      "worktree caveat: answers describe the {} at {}, not the git worktree at {} (repo {} of project {}); edits are refused here",
      self.indexed_role.described(),
      self.indexed_checkout.display(),
      self.worktree.display(),
      self.repo,
      self.project
    )
  }

  pub fn edit_refusal(&self) -> String {
    format!(
      "refusing to edit from the git worktree at {}: project {} indexes the {} at {} (repo {}), so the edit would write there, not into this worktree; nothing was written. Edit the worktree's files directly, or run the edit from {}",
      self.worktree.display(),
      self.project,
      self.indexed_role.indexed(),
      self.indexed_checkout.display(),
      self.repo,
      self.indexed_checkout.display()
    )
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedWorktree {
  pub project: ProjectName,
  pub repo: RepoName,
  pub root: PathBuf,
  pub registered: PathBuf,
}

impl UnresolvedWorktree {
  pub fn caveat(&self) -> String {
    format!(
      "worktree caveat: {} has a .git entry that does not resolve to a git checkout (a broken worktree?); answers describe the checkout registered at {} (repo {} of project {}), not {}; edits are refused here",
      self.root.display(),
      self.registered.display(),
      self.repo,
      self.project,
      self.root.display()
    )
  }

  pub fn edit_refusal(&self) -> String {
    format!(
      "refusing to edit from {}: its .git entry does not resolve to a git checkout, so the worktree could not be resolved and the edit could write into the checkout registered at {} (repo {} of project {}) instead; nothing was written. Repair it with `git worktree repair`, or run the edit from {}",
      self.root.display(),
      self.registered.display(),
      self.repo,
      self.project,
      self.registered.display()
    )
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayOf {
  pub project: ProjectName,
  pub overlay: ProjectName,
  pub label: String,
  pub worktree: PathBuf,
  pub repos: Vec<RepoName>,
}

impl OverlayOf {
  pub fn describe(&self) -> String {
    let repos: Vec<&str> = self.repos.iter().map(RepoName::as_str).collect();
    let noun = if repos.len() == 1 { "repo" } else { "repos" };
    format!(
      "worktree overlay {} of project {}: {noun} {} indexed from the git worktree at {}",
      self.label,
      self.project,
      repos.join(", "),
      self.worktree.display()
    )
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
  Direct,
  Worktree(WorktreeOf),
  UnresolvedWorktree(UnresolvedWorktree),
  Overlay(OverlayOf),
}

impl Binding {
  pub fn caveat(&self) -> Option<String> {
    match self {
      Self::Direct | Self::Overlay(_) => None,
      Self::Worktree(worktree) => Some(worktree.caveat()),
      Self::UnresolvedWorktree(unresolved) => Some(unresolved.caveat()),
    }
  }

  pub fn edit_refusal(&self) -> Option<String> {
    match self {
      Self::Direct | Self::Overlay(_) => None,
      Self::Worktree(worktree) => Some(worktree.edit_refusal()),
      Self::UnresolvedWorktree(unresolved) => Some(unresolved.edit_refusal()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
  pub project: Project,
  pub binding: Binding,
  pub warnings: Vec<String>,
}

enum Relation {
  Unrelated,
  Direct,
  OtherCheckout(GitCheckout),
  Enclosed,
  Unresolved,
}

pub(crate) struct Locator {
  pub(crate) cwd: PathBuf,
  location: GitLocation,
}

impl Locator {
  pub(crate) fn new(canonical_cwd: PathBuf) -> Self {
    let location = GitCheckout::locate(&canonical_cwd);
    Self { cwd: canonical_cwd, location }
  }

  pub(crate) fn bind(&self, project: &Project) -> Option<Binding> {
    let relations: Vec<(&RepoConfig, Relation)> = project.repos.iter().map(|repo| (repo, self.relation(repo))).collect();
    if relations.iter().any(|(_, relation)| matches!(relation, Relation::Direct)) {
      return Some(Binding::Direct);
    }
    self
      .other_checkout(project, &relations)
      .or_else(|| self.enclosed(project, &relations))
      .or_else(|| self.unresolved(project, &relations))
  }

  fn relation(&self, repo: &RepoConfig) -> Relation {
    let contained = match self.cwd.starts_with(&repo.path) {
      true => Relation::Direct,
      false => Relation::Unrelated,
    };
    match &self.location {
      GitLocation::Outside => contained,
      GitLocation::Unresolved { root } if strictly_below(root, &repo.path) => Relation::Unresolved,
      GitLocation::Unresolved { .. } => contained,
      GitLocation::Checkout(checkout) => {
        if let Some(other) = GitCheckout::find(&repo.path).filter(|other| checkout.is_other_checkout_of(other)) {
          return Relation::OtherCheckout(other);
        }
        match checkout.kind == CheckoutKind::LinkedWorktree && strictly_below(&checkout.root, &repo.path) {
          true => Relation::Enclosed,
          false => contained,
        }
      }
    }
  }

  fn other_checkout(&self, project: &Project, relations: &[(&RepoConfig, Relation)]) -> Option<Binding> {
    let GitLocation::Checkout(checkout) = &self.location else { return None };
    let (repo, other) = relations
      .iter()
      .filter_map(|(repo, relation)| match relation {
        Relation::OtherCheckout(other) => Some((*repo, other)),
        _ => None,
      })
      .min_by_key(|(repo, other)| Reverse(self.mapped_depth(checkout, repo, other)))?;
    let indexed_role = match other.kind {
      CheckoutKind::Primary => CheckoutRole::MainCheckout,
      CheckoutKind::LinkedWorktree => CheckoutRole::OtherCheckout,
    };
    Some(Binding::Worktree(WorktreeOf {
      project: project.name.clone(),
      repo: repo.name.clone(),
      worktree: checkout.root.clone(),
      indexed_checkout: other.root.clone(),
      indexed_role,
    }))
  }

  fn enclosed(&self, project: &Project, relations: &[(&RepoConfig, Relation)]) -> Option<Binding> {
    let GitLocation::Checkout(checkout) = &self.location else { return None };
    let repo = innermost(relations, |relation| matches!(relation, Relation::Enclosed))?;
    let (indexed_checkout, indexed_role) = match checkout.main_root().filter(|main| main.starts_with(&repo.path)) {
      Some(main) => (main.to_path_buf(), CheckoutRole::MainCheckout),
      None => (repo.path.clone(), CheckoutRole::OtherCheckout),
    };
    Some(Binding::Worktree(WorktreeOf {
      project: project.name.clone(),
      repo: repo.name.clone(),
      worktree: checkout.root.clone(),
      indexed_checkout,
      indexed_role,
    }))
  }

  fn unresolved(&self, project: &Project, relations: &[(&RepoConfig, Relation)]) -> Option<Binding> {
    let GitLocation::Unresolved { root } = &self.location else { return None };
    let repo = innermost(relations, |relation| matches!(relation, Relation::Unresolved))?;
    Some(Binding::UnresolvedWorktree(UnresolvedWorktree {
      project: project.name.clone(),
      repo: repo.name.clone(),
      root: root.clone(),
      registered: repo.path.clone(),
    }))
  }

  fn mapped_depth(&self, checkout: &GitCheckout, repo: &RepoConfig, other: &GitCheckout) -> Option<usize> {
    let mapped = checkout.root.join(repo.path.strip_prefix(&other.root).ok()?);
    self.cwd.starts_with(&mapped).then(|| mapped.components().count())
  }
}

fn strictly_below(path: &Path, ancestor: &Path) -> bool {
  path != ancestor && path.starts_with(ancestor)
}

fn innermost<'a>(relations: &[(&'a RepoConfig, Relation)], wanted: impl Fn(&Relation) -> bool) -> Option<&'a RepoConfig> {
  relations
    .iter()
    .filter(|(_, relation)| wanted(relation))
    .map(|(repo, _)| *repo)
    .min_by_key(|repo| Reverse(repo.path.components().count()))
}
