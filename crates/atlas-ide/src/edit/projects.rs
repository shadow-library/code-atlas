use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_lsp::canonical_path;
use atlas_project::PathGlobs;
use serde_json::Value;

use crate::error::IdeResult;
use crate::service::IdeService;
use crate::target::FileRef;

pub(crate) const MAX_PROJECTS: usize = 16;
const MAX_DIRECTORIES: usize = 4000;
const SKIPPED_DIRECTORIES: [&str; 8] = ["node_modules", "dist", "build", "coverage", "target", "out", "vendor", "bower_components"];
const SOURCE_EXTENSIONS: [&str; 6] = ["ts", "tsx", "mts", "cts", "js", "jsx"];

struct TsProject {
  config: PathBuf,
  root: PathBuf,
  files: Vec<PathBuf>,
  include: Option<PathGlobs>,
  exclude: Option<PathGlobs>,
  references: Vec<PathBuf>,
}

impl TsProject {
  fn contains(&self, file: &Path) -> bool {
    if self.files.iter().any(|listed| listed == file) {
      return true;
    }
    let (Some(include), Ok(relative)) = (&self.include, file.strip_prefix(&self.root)) else {
      return false;
    };
    let Some(relative) = relative.to_str() else { return false };
    include.matches_within(relative, false) && !self.exclude.as_ref().is_some_and(|exclude| exclude.matches_within(relative, false))
  }
}

pub(crate) struct ProjectFiles {
  pub open: Vec<PathBuf>,
  pub caveats: Vec<String>,
  pub unloaded: Vec<String>,
}

impl IdeService {
  pub(crate) fn project_representatives(&self, anchor: &FileRef, root: &Path, admits: impl Fn(&Path) -> bool) -> IdeResult<ProjectFiles> {
    let mut projects = Vec::new();
    let mut unparsed = Vec::new();
    for config in config_files(root) {
      match parse_project(&config, root) {
        Some(project) => projects.push(project),
        None => unparsed.push(config),
      }
    }
    let sources: Vec<PathBuf> = {
      let mut sources: Vec<PathBuf> = self
        .query
        .engine()
        .store()
        .read(|tx| tx.files_in_repo(&anchor.repo))?
        .into_iter()
        .map(|record| root.join(record.path.as_str()))
        .filter(|path| {
          path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension))
        })
        .collect();
      sources.sort();
      sources
    };
    let mut selected: BTreeSet<usize> = projects
      .iter()
      .enumerate()
      .filter(|(_, project)| project.contains(&anchor.absolute))
      .map(|(index, _)| index)
      .collect();
    let referencing: Vec<usize> = projects
      .iter()
      .enumerate()
      .filter(|(_, project)| {
        project
          .references
          .iter()
          .any(|reference| selected.iter().any(|index| projects[*index].config == *reference))
      })
      .map(|(index, _)| index)
      .collect();
    selected.extend(referencing);
    let selected: Vec<&TsProject> = selected.into_iter().map(|index| &projects[index]).collect();
    let mut open = Vec::new();
    let mut considered = 0;
    for project in &selected {
      let members: Vec<&PathBuf> = sources
        .iter()
        .filter(|source| **source != anchor.absolute && admits(source) && project.contains(source))
        .collect();
      if members.is_empty() {
        continue;
      }
      considered += 1;
      if considered > MAX_PROJECTS {
        continue;
      }
      let exclusive = members
        .iter()
        .find(|source| selected.iter().all(|other| std::ptr::eq(*other, *project) || !other.contains(source)));
      if let Some(representative) = exclusive.or(members.first()) {
        open.push((*representative).clone());
      }
    }
    let mut caveats: Vec<String> = (considered > MAX_PROJECTS)
      .then(|| format!("{considered} tsconfig projects contain {anchor}; only the first {MAX_PROJECTS} were loaded, so results may differ with what was opened before"))
      .into_iter()
      .collect();
    let mut unloaded = Vec::new();
    for config in unparsed {
      let shown = config.strip_prefix(root).unwrap_or(&config).display().to_string();
      let directory = config.parent().unwrap_or(root);
      let representative = sources.iter().find(|source| **source != anchor.absolute && source.starts_with(directory) && admits(source));
      match representative {
        Some(source) => {
          caveats.push(format!(
            "{shown} could not be read as a tsconfig, so code-atlas cannot tell what it includes; {} was opened so the language server loads it",
            source.strip_prefix(root).unwrap_or(source).display()
          ));
          open.push(source.clone());
        }
        None => {
          caveats.push(format!("{shown} could not be read as a tsconfig and has no file code-atlas could open to load it"));
          unloaded.push(shown);
        }
      }
    }
    open.sort();
    open.dedup();
    Ok(ProjectFiles { open, caveats, unloaded })
  }
}

fn config_files(root: &Path) -> Vec<PathBuf> {
  let mut found = Vec::new();
  let mut pending = vec![root.to_path_buf()];
  let mut visited = 0;
  while let Some(directory) = pending.pop() {
    visited += 1;
    if visited > MAX_DIRECTORIES {
      tracing::debug!(root = %root.display(), "stopped looking for tsconfig files after {MAX_DIRECTORIES} directories");
      break;
    }
    let Ok(entries) = fs::read_dir(&directory) else { continue };
    for entry in entries.flatten() {
      let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
      let Ok(file_type) = entry.file_type() else { continue };
      if file_type.is_dir() {
        if !name.starts_with('.') && !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
          pending.push(entry.path());
        }
      } else if file_type.is_file() && (name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json") {
        found.push(entry.path());
      }
    }
  }
  found.sort();
  found
}

fn read_config(path: &Path) -> Option<Value> {
  let text = fs::read_to_string(path).ok()?;
  serde_json::from_str(&strip_jsonc(&text)).ok()
}

fn strings(value: Option<&Value>) -> Option<Vec<String>> {
  value?.as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
}

fn normalized(path: &Path) -> PathBuf {
  let mut normal = PathBuf::new();
  for component in path.components() {
    match component {
      std::path::Component::ParentDir => {
        normal.pop();
      }
      std::path::Component::CurDir => {}
      other => normal.push(other.as_os_str()),
    }
  }
  normal
}

fn parse_project(config: &Path, root: &Path) -> Option<TsProject> {
  let config = canonical_path(config).ok()?;
  let directory = config.parent()?.to_path_buf();
  let json = read_config(&config)?;
  let base = json
    .get("extends")
    .and_then(Value::as_str)
    .filter(|extends| extends.starts_with('.'))
    .and_then(|extends| read_config(&directory.join(extends)));
  let field = |name: &str| strings(json.get(name)).or_else(|| strings(base.as_ref().and_then(|base| base.get(name))));
  let listed = field("files");
  let include = match (field("include"), &listed) {
    (Some(include), _) => include,
    (None, Some(_)) => Vec::new(),
    (None, None) => vec!["**/*".to_owned()],
  };
  let globs = |patterns: Vec<String>| {
    let patterns: Vec<String> = patterns
      .iter()
      .filter_map(|pattern| normalized(&directory.join(pattern)).strip_prefix(root).ok().and_then(Path::to_str).map(str::to_owned))
      .collect();
    (!patterns.is_empty()).then(|| PathGlobs::anchored(&patterns).ok()).flatten()
  };
  let references = json
    .get("references")
    .and_then(Value::as_array)
    .map(|references| {
      references
        .iter()
        .filter_map(|reference| reference.get("path").and_then(Value::as_str))
        .filter_map(|path| {
          let target = directory.join(path);
          let target = if target.extension().is_some_and(|extension| extension == "json") {
            target
          } else {
            target.join("tsconfig.json")
          };
          canonical_path(&target).ok()
        })
        .collect()
    })
    .unwrap_or_default();
  Some(TsProject {
    files: listed.unwrap_or_default().iter().filter_map(|file| canonical_path(&directory.join(file)).ok()).collect(),
    include: globs(include),
    exclude: field("exclude").and_then(globs),
    references,
    root: root.to_path_buf(),
    config,
  })
}

fn next_significant(rest: &str) -> Option<char> {
  let mut rest = rest;
  loop {
    rest = rest.trim_start();
    if let Some(comment) = rest.strip_prefix("//") {
      rest = comment.find('\n').map_or("", |end| &comment[end..]);
    } else if let Some(comment) = rest.strip_prefix("/*") {
      rest = comment.find("*/").map_or("", |end| &comment[end + 2..]);
    } else {
      return rest.chars().next();
    }
  }
}

fn strip_jsonc(text: &str) -> String {
  let mut output = String::with_capacity(text.len());
  let mut chars = text.chars().peekable();
  let mut in_string = false;
  while let Some(character) = chars.next() {
    if in_string {
      output.push(character);
      match character {
        '\\' => output.extend(chars.next()),
        '"' => in_string = false,
        _ => {}
      }
      continue;
    }
    match (character, chars.peek()) {
      ('"', _) => {
        in_string = true;
        output.push(character);
      }
      ('/', Some('/')) => {
        for skipped in chars.by_ref() {
          if skipped == '\n' {
            output.push('\n');
            break;
          }
        }
      }
      ('/', Some('*')) => {
        chars.next();
        let mut previous = ' ';
        for skipped in chars.by_ref() {
          if previous == '*' && skipped == '/' {
            break;
          }
          previous = skipped;
        }
      }
      (',', _) => {
        let rest: String = chars.clone().collect();
        if !matches!(next_significant(&rest), Some('}' | ']')) {
          output.push(',');
        }
      }
      _ => output.push(character),
    }
  }
  output
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_read_tsconfig_with_comments_and_trailing_commas() {
    let text = "{\n  // build\n  \"include\": [\"src/**/*.ts\", /* all */ \"a//b\",],\n}\n";
    let commented = "{\n  \"include\": [\"src\"], // note\n  /* more */\n}\n";

    let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
    let noted: Value = serde_json::from_str(&strip_jsonc(commented)).unwrap();

    assert_eq!(value["include"], serde_json::json!(["src/**/*.ts", "a//b"]));
    assert_eq!(noted["include"], serde_json::json!(["src"]));
  }

  #[test]
  fn should_tell_which_projects_contain_a_file() {
    let work = tempfile::tempdir().unwrap();
    let root = canonical_path(work.path()).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.ts"), "").unwrap();
    fs::write(root.join("src/a.test.ts"), "").unwrap();
    fs::write(root.join("tsconfig.json"), "{\"files\": [], \"references\": [{\"path\": \"./tsconfig.build.json\"}]}").unwrap();
    fs::write(root.join("tsconfig.build.json"), "{\"include\": [\"src/**/*.ts\"], \"exclude\": [\"src/**/*.test.ts\"]}").unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/tsconfig.json"), "{\"include\": [\"**/*.ts\", \"../src/**/*.ts\"]}").unwrap();

    let solution = parse_project(&root.join("tsconfig.json"), &root).unwrap();
    let build = parse_project(&root.join("tsconfig.build.json"), &root).unwrap();
    let tests = parse_project(&root.join("tests/tsconfig.json"), &root).unwrap();

    assert!(!solution.contains(&root.join("src/a.ts")));
    assert_eq!(solution.references, vec![root.join("tsconfig.build.json")]);
    assert!(build.contains(&root.join("src/a.ts")));
    assert!(!build.contains(&root.join("src/a.test.ts")));
    assert!(tests.contains(&root.join("src/a.ts")) && tests.contains(&root.join("tests/t1.ts")));
    assert!(!tests.contains(&root.join("other/b.ts")));
    assert_eq!(
      config_files(&root),
      vec![root.join("tests/tsconfig.json"), root.join("tsconfig.build.json"), root.join("tsconfig.json")]
    );
  }
}
