use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use super::error::{AgentError, AgentResult};

const SECTION_SEPARATOR: char = '#';
const FENCE_MARKERS: [char; 2] = ['`', '~'];
const MIN_FENCE_LENGTH: usize = 3;
const MAX_FENCE_INDENT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRef<'a> {
  pub file: &'a str,
  pub heading: &'a str,
}

impl<'a> SectionRef<'a> {
  pub fn parse(entry: &'a str) -> Option<Self> {
    let (file, heading) = entry.split_once(SECTION_SEPARATOR)?;
    let (file, heading) = (file.trim(), heading.trim());
    (!file.is_empty() && !heading.is_empty()).then_some(Self { file, heading })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinalLink {
  Removable,
  Refused,
}

pub(super) fn tree_path(tree: &Path, relative: &str, final_link: FinalLink) -> AgentResult<PathBuf> {
  let components: Vec<Component<'_>> = Path::new(relative).components().collect();
  let mut current = tree.to_owned();
  for (index, component) in components.iter().enumerate() {
    current.push(component);
    let is_link = fs::symlink_metadata(&current).is_ok_and(|kind| kind.file_type().is_symlink());
    let is_final = index + 1 == components.len();
    if is_link && !(is_final && final_link == FinalLink::Removable) {
      let link: PathBuf = components[..=index].iter().collect();
      return Err(AgentError::TreePathThroughSymlink {
        path: relative.to_owned(),
        link: link.display().to_string(),
      });
    }
  }
  Ok(current)
}

pub fn remove_paths(tree: &Path, paths: &[String]) -> AgentResult<()> {
  for path in paths {
    let target = tree_path(tree, path, FinalLink::Removable)?;
    let kind = fs::symlink_metadata(&target).map_err(|error| match error.kind() {
      ErrorKind::NotFound => AgentError::InvalidArgument(format!("strip path `{path}` does not exist in the tree")),
      _ => AgentError::io(&target, error),
    })?;
    let removed = if kind.is_dir() { fs::remove_dir_all(&target) } else { fs::remove_file(&target) };
    removed.map_err(|error| AgentError::io(&target, error))?;
  }
  Ok(())
}

pub fn drop_sections(tree: &Path, entries: &[String]) -> AgentResult<()> {
  for entry in entries {
    let section = SectionRef::parse(entry).ok_or_else(|| AgentError::InvalidArgument(format!("`{entry}` is not `<file>#<heading>`")))?;
    let path = tree_path(tree, section.file, FinalLink::Refused)?;
    let text = fs::read_to_string(&path).map_err(|error| AgentError::io(&path, error))?;
    let kept = without_section(&text, section.heading).map_err(|found| {
      AgentError::InvalidArgument(format!(
        "{} has {found} headings named `{}`; a section to drop must match exactly one",
        section.file, section.heading
      ))
    })?;
    fs::write(&path, kept).map_err(|error| AgentError::io(&path, error))?;
  }
  Ok(())
}

fn heading_of(line: &str) -> Option<(usize, &str)> {
  let level = line.chars().take_while(|c| *c == '#').count();
  let rest = &line[level..];
  ((1..=6).contains(&level) && rest.starts_with(' ')).then(|| (level, rest.trim()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fence {
  marker: char,
  length: usize,
}

fn fence_run(line: &str) -> Option<(Fence, &str)> {
  let body = line.trim_start_matches(' ');
  let marker = body.chars().next().filter(|first| FENCE_MARKERS.contains(first))?;
  let length = body.chars().take_while(|c| *c == marker).count();
  (line.len() - body.len() <= MAX_FENCE_INDENT && length >= MIN_FENCE_LENGTH).then(|| (Fence { marker, length }, &body[length..]))
}

fn next_fence_state(open: Option<Fence>, line: &str) -> Option<Fence> {
  match (open, fence_run(line)) {
    (None, Some((fence, info))) if fence.marker != '`' || !info.contains('`') => Some(fence),
    (Some(opened), Some((fence, rest))) if fence.marker == opened.marker && fence.length >= opened.length && rest.trim().is_empty() => None,
    (state, _) => state,
  }
}

pub fn without_section(text: &str, heading: &str) -> Result<String, usize> {
  let mut open: Option<Fence> = None;
  let headings: Vec<(usize, usize, &str)> = text
    .split_inclusive('\n')
    .enumerate()
    .filter_map(|(index, line)| {
      let line = line.trim_end();
      let inside = open.is_some();
      open = next_fence_state(open, line);
      let (level, title) = heading_of(line).filter(|_| !inside)?;
      Some((index, level, title))
    })
    .collect();
  let matching: Vec<&(usize, usize, &str)> = headings.iter().filter(|(_, _, title)| *title == heading).collect();
  let [(start, level, _)] = matching.as_slice() else {
    return Err(matching.len());
  };
  let end = headings.iter().find(|(index, other, _)| index > start && other <= level).map(|(index, _, _)| *index);
  let kept = text
    .split_inclusive('\n')
    .enumerate()
    .filter(|(index, _)| index < start || end.is_some_and(|end| *index >= end))
    .map(|(_, line)| line);
  Ok(kept.collect())
}

#[cfg(test)]
mod tests {
  use super::*;

  const GUIDE: &str = "# Guide\n\nIntro.\n\n## Tools\n\nUse the indexer.\n\n```sh\n# not a heading\nindexer run\n```\n\n### Flags\n\n- `--fast`\n\n## Style\n\nTwo spaces.\n";

  #[test]
  fn should_split_a_section_reference_at_the_first_separator() {
    assert_eq!(
      SectionRef::parse("docs/GUIDE.md#Tools"),
      Some(SectionRef {
        file: "docs/GUIDE.md",
        heading: "Tools"
      })
    );
    assert_eq!(SectionRef::parse("GUIDE.md# C# notes ").map(|section| section.heading), Some("C# notes"));
    for malformed in ["GUIDE.md", "GUIDE.md#", "#Tools", " # "] {
      assert_eq!(SectionRef::parse(malformed), None, "{malformed}");
    }
  }

  #[test]
  fn should_drop_a_section_with_its_subsections_up_to_the_next_heading_of_its_level() {
    assert_eq!(without_section(GUIDE, "Tools").unwrap(), "# Guide\n\nIntro.\n\n## Style\n\nTwo spaces.\n");
    assert_eq!(
      without_section(GUIDE, "Flags").unwrap(),
      "# Guide\n\nIntro.\n\n## Tools\n\nUse the indexer.\n\n```sh\n# not a heading\nindexer run\n```\n\n## Style\n\nTwo spaces.\n"
    );
    assert_eq!(without_section(GUIDE, "Style").unwrap(), GUIDE.replace("## Style\n\nTwo spaces.\n", ""));
    assert_eq!(without_section(GUIDE, "Guide").unwrap(), "");
  }

  #[test]
  fn should_refuse_a_heading_that_is_missing_repeated_or_only_inside_a_code_fence() {
    assert_eq!(without_section(GUIDE, "Missing"), Err(0));
    assert_eq!(without_section(GUIDE, "not a heading"), Err(0));
    assert_eq!(without_section("## Notes\n\na\n\n## Notes\n\nb\n", "Notes"), Err(2));
    assert_eq!(without_section("#NoSpace\n", "NoSpace"), Err(0));
  }

  #[test]
  fn should_remove_listed_files_and_directories_and_fail_on_a_path_the_tree_does_not_have() {
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("suite/cases")).unwrap();
    fs::write(tree.path().join("suite/cases/a.spec.ts"), "x").unwrap();
    fs::write(tree.path().join("NOTES.md"), "x").unwrap();
    fs::write(tree.path().join("keep.ts"), "x").unwrap();

    remove_paths(tree.path(), &["suite".to_owned(), "NOTES.md".to_owned()]).unwrap();

    assert!(!tree.path().join("suite").exists() && !tree.path().join("NOTES.md").exists());
    assert!(tree.path().join("keep.ts").is_file());
    let error = remove_paths(tree.path(), &["suite".to_owned()]).unwrap_err();
    assert_eq!(error.to_string(), "invalid argument: strip path `suite` does not exist in the tree");
  }

  #[test]
  fn should_rewrite_the_file_without_the_section_and_fail_without_touching_it_when_the_heading_is_absent() {
    let tree = tempfile::tempdir().unwrap();
    fs::write(tree.path().join("GUIDE.md"), GUIDE).unwrap();

    drop_sections(tree.path(), &["GUIDE.md#Tools".to_owned()]).unwrap();

    assert_eq!(fs::read_to_string(tree.path().join("GUIDE.md")).unwrap(), "# Guide\n\nIntro.\n\n## Style\n\nTwo spaces.\n");
    let error = drop_sections(tree.path(), &["GUIDE.md#Tools".to_owned()]).unwrap_err();
    assert_eq!(
      error.to_string(),
      "invalid argument: GUIDE.md has 0 headings named `Tools`; a section to drop must match exactly one"
    );
    assert!(matches!(drop_sections(tree.path(), &["ABSENT.md#Tools".to_owned()]), Err(AgentError::Io { .. })));
  }

  #[test]
  fn should_keep_a_code_fence_open_until_a_fence_of_the_same_character_and_at_least_its_length_closes_it() {
    let tilde_inside = "## Tools\n\n```sh\n~~~\n# still code\n## Tools\n```\n\n## Style\n\nTwo spaces.\n";
    assert_eq!(without_section(tilde_inside, "Tools").unwrap(), "## Style\n\nTwo spaces.\n");
    let longer_fence = "## Tools\n\n````md\n```sh\n## Tools\n```\n# still code\n````\n\n## Style\n\nTwo spaces.\n";
    assert_eq!(without_section(longer_fence, "Tools").unwrap(), "## Style\n\nTwo spaces.\n");
    assert_eq!(without_section(longer_fence, "still code"), Err(0));
    let closed_by_longer = "## Tools\n\n~~~\ncode\n~~~~~\n\n## Style\n";
    assert_eq!(without_section(closed_by_longer, "Style").unwrap(), "## Tools\n\n~~~\ncode\n~~~~~\n\n");
    let inline_span = "```not a fence``` text\n\n## Tools\n\nx\n";
    assert_eq!(without_section(inline_span, "Tools").unwrap(), "```not a fence``` text\n\n");
    let info_after_close = "```\ncode\n``` not a close\n## Tools\n```\n\n## Style\n";
    assert_eq!(without_section(info_after_close, "Tools"), Err(0));
  }

  #[test]
  fn should_match_only_plain_headings_at_the_start_of_a_line() {
    for unmatched in ["## Tools ##\n\nx\n", "Tools\n-----\n\nx\n", "  ## Tools\n\nx\n"] {
      assert_eq!(without_section(unmatched, "Tools"), Err(0), "{unmatched}");
    }
  }

  #[cfg(unix)]
  #[test]
  fn should_refuse_to_strip_through_a_symlinked_directory_and_leave_what_it_points_at_untouched() {
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(outside.path().join("victim")).unwrap();
    fs::write(outside.path().join("victim/keep.txt"), "kept").unwrap();
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("docs")).unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.path().join("docs/deep")).unwrap();

    let refused = remove_paths(tree.path(), &["link/victim".to_owned()]).unwrap_err();
    assert_eq!(
      refused.to_string(),
      "`link/victim` reaches outside the tree through the symbolic link `link`; a task may only edit files of its own tree"
    );
    let nested = remove_paths(tree.path(), &["docs/deep/victim/keep.txt".to_owned()]).unwrap_err();
    assert!(matches!(nested, AgentError::TreePathThroughSymlink { ref link, .. } if link == "docs/deep"), "{nested}");
    assert_eq!(fs::read_to_string(outside.path().join("victim/keep.txt")).unwrap(), "kept");

    remove_paths(tree.path(), &["link".to_owned()]).unwrap();
    assert!(fs::symlink_metadata(tree.path().join("link")).is_err());
    assert_eq!(fs::read_to_string(outside.path().join("victim/keep.txt")).unwrap(), "kept");
  }

  #[cfg(unix)]
  #[test]
  fn should_refuse_to_drop_a_section_from_a_symlinked_file_and_leave_the_file_it_points_at_untouched() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("GUIDE.md"), GUIDE).unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path().join("GUIDE.md"), tree.path().join("GUIDE.md")).unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.path().join("docs")).unwrap();

    for entry in ["GUIDE.md#Tools", "docs/GUIDE.md#Tools"] {
      let refused = drop_sections(tree.path(), &[entry.to_owned()]).unwrap_err();
      assert!(matches!(refused, AgentError::TreePathThroughSymlink { .. }), "{entry}: {refused}");
    }
    assert_eq!(fs::read_to_string(outside.path().join("GUIDE.md")).unwrap(), GUIDE);
  }

  #[cfg(unix)]
  #[test]
  fn should_report_a_metadata_error_other_than_a_missing_path_as_itself() {
    use std::os::unix::fs::PermissionsExt;

    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("sealed/inner")).unwrap();
    fs::set_permissions(tree.path().join("sealed"), fs::Permissions::from_mode(0o000)).unwrap();

    let unreadable = fs::read_dir(tree.path().join("sealed")).is_err();
    let denied = remove_paths(tree.path(), &["sealed/inner".to_owned()]);
    fs::set_permissions(tree.path().join("sealed"), fs::Permissions::from_mode(0o755)).unwrap();

    if unreadable {
      assert!(matches!(denied, Err(AgentError::Io { .. })), "{denied:?}");
    }
    let missing = remove_paths(tree.path(), &["sealed/absent".to_owned()]).unwrap_err();
    assert_eq!(missing.to_string(), "invalid argument: strip path `sealed/absent` does not exist in the tree");
  }
}
