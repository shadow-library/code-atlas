use std::path::{Path, PathBuf};

use atlas_mcp::guidance::{BLOCK_BEGIN, BLOCK_END};
use atlas_project::{GitCheckout, GitLocation};

use super::target::read;

pub(super) const LOCAL_FILE: &str = "CLAUDE.local.md";
const TRACKED_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];
const NEW_TRACKED_FILE: &str = "AGENTS.md";
const MALFORMED: &str = "unterminated or duplicated Code Atlas guidance block; fix or delete the code-atlas:begin / code-atlas:end markers";

pub(super) enum ExcludeLocation {
  Excluded { path: PathBuf, line: String },
  NotInGit,
  Unresolved(PathBuf),
}

fn block_range(text: &str) -> Result<Option<(usize, usize)>, String> {
  let begins: Vec<usize> = text.match_indices(BLOCK_BEGIN).map(|(index, _)| index).collect();
  let ends: Vec<usize> = text.match_indices(BLOCK_END).map(|(index, _)| index).collect();
  match (begins.as_slice(), ends.as_slice()) {
    ([], []) => Ok(None),
    ([begin], [end]) if begin < end => Ok(Some((*begin, end + BLOCK_END.len()))),
    _ => Err(MALFORMED.to_owned()),
  }
}

pub(super) fn find_block(text: &str) -> Result<Option<&str>, String> {
  Ok(block_range(text)?.map(|(begin, end)| &text[begin..end]))
}

pub(super) fn upsert_block(text: &str, block: &str) -> Result<String, String> {
  if let Some((begin, end)) = block_range(text)? {
    return Ok(format!("{}{block}{}", &text[..begin], &text[end..]));
  }
  let mut after = text.to_owned();
  if !after.is_empty() && !after.ends_with('\n') {
    after.push('\n');
  }
  if !after.is_empty() && !after.ends_with("\n\n") {
    after.push('\n');
  }
  after.push_str(block);
  after.push('\n');
  Ok(after)
}

pub(super) fn remove_block(text: &str) -> Result<Option<String>, String> {
  let Some((begin, end)) = block_range(text)? else { return Ok(None) };
  let rest = &text[end..];
  let rest = rest.strip_prefix('\n').unwrap_or(rest);
  let before = &text[..begin];
  let before = match rest.is_empty() {
    true => before.strip_suffix('\n').filter(|trimmed| trimmed.ends_with('\n')).unwrap_or(before),
    false => before,
  };
  Ok(Some(format!("{before}{rest}")))
}

pub(super) fn tracked_path(root: &Path) -> Result<PathBuf, String> {
  let mut existing = Vec::new();
  for name in TRACKED_FILES {
    let path = root.join(name);
    if let Some(text) = read(&path)? {
      existing.push((path, text));
    }
  }
  let chosen = existing.iter().find(|(_, text)| text.contains(BLOCK_BEGIN)).or(existing.first());
  Ok(chosen.map_or_else(|| root.join(NEW_TRACKED_FILE), |(path, _)| path.clone()))
}

pub(super) fn is_local_file(path: &Path) -> bool {
  path.file_name().is_some_and(|name| name == LOCAL_FILE)
}

pub(super) fn hides_agents_md(root: &Path) -> bool {
  let present = |relative: &str| root.join(relative).is_file();
  present("AGENTS.md") && !present("CLAUDE.md") && !present(".claude/CLAUDE.md") && !present(LOCAL_FILE)
}

pub(super) fn exclude_for(file: &Path) -> ExcludeLocation {
  let Some(existing) = file.ancestors().skip(1).find(|ancestor| ancestor.is_dir()) else {
    return ExcludeLocation::NotInGit;
  };
  let Ok(pending) = file.strip_prefix(existing) else { return ExcludeLocation::NotInGit };
  let checkout = match GitCheckout::locate(existing) {
    GitLocation::Checkout(checkout) => checkout,
    GitLocation::Outside => return ExcludeLocation::NotInGit,
    GitLocation::Unresolved { root } => return ExcludeLocation::Unresolved(root),
  };
  let Ok(existing) = std::fs::canonicalize(existing) else {
    return ExcludeLocation::NotInGit;
  };
  let Ok(relative) = existing.strip_prefix(&checkout.root) else {
    return ExcludeLocation::NotInGit;
  };
  let segments: Vec<String> = relative
    .join(pending)
    .components()
    .map(|component| component.as_os_str().to_string_lossy().into_owned())
    .collect();
  ExcludeLocation::Excluded {
    path: checkout.common_dir.join("info").join("exclude"),
    line: format!("/{}", segments.join("/")),
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;

  fn block(project: &str) -> String {
    atlas_mcp::guidance::repo_block(project)
  }

  #[test]
  fn should_append_the_block_after_existing_content_and_take_it_out_again() {
    let after = upsert_block("# Mine\nkeep me", &block("acme")).unwrap();
    assert_eq!(after, format!("# Mine\nkeep me\n\n{}\n", block("acme")));
    assert_eq!(remove_block(&after).unwrap(), Some("# Mine\nkeep me\n".to_owned()));
  }

  #[test]
  fn should_replace_only_the_marked_block() {
    let text = format!("before\n{}\nafter\n", block("old"));
    assert_eq!(upsert_block(&text, &block("new")).unwrap(), format!("before\n{}\nafter\n", block("new")));
    assert_eq!(remove_block(&text).unwrap(), Some("before\nafter\n".to_owned()));
  }

  #[test]
  fn should_refuse_an_unterminated_block() {
    assert_eq!(upsert_block(&format!("{BLOCK_BEGIN}\nhand written\n"), &block("p")), Err(MALFORMED.to_owned()));
  }

  #[test]
  fn should_exclude_through_the_common_git_dir_for_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = fs::canonicalize(dir.path()).unwrap().join("main");
    let admin = main.join(".git/worktrees/feature");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    let worktree = main.join(".worktrees/feature");
    fs::create_dir_all(worktree.join("sub")).unwrap();
    fs::write(worktree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();

    let ExcludeLocation::Excluded { path, line } = exclude_for(&worktree.join("sub/.claude/settings.local.json")) else {
      panic!("expected an exclude")
    };
    assert_eq!(path, main.join(".git/info/exclude"));
    assert_eq!(line, "/sub/.claude/settings.local.json");
    assert!(matches!(exclude_for(&dir.path().join("loose/CLAUDE.local.md")), ExcludeLocation::NotInGit));
  }

  #[test]
  fn should_warn_that_a_local_file_hides_agents_md_only_without_a_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("AGENTS.md"), "rules").unwrap();
    assert!(hides_agents_md(dir.path()));
    fs::write(dir.path().join("CLAUDE.md"), "rules").unwrap();
    assert!(!hides_agents_md(dir.path()));
  }
}
