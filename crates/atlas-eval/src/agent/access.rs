use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::bash_guard::denied_by_bash_guard;
use super::transcript::{ToolCall, Transcript};

const PATH_FIELDS: [&str; 4] = ["file_path", "path", "pattern", "notebook_path"];
const HOME_TOOLCHAIN_DIRS: [&str; 7] = [".cargo", ".rustup", ".nvm", ".npm", ".volta", ".bun", ".node-gyp"];
const SYSTEM_PREFIXES: [&str; 8] = ["/usr", "/bin", "/sbin", "/opt/homebrew", "/dev", "/Library/Developer", "/Applications/Xcode.app", "/System"];
const SCRATCH_PREFIX: &str = "ae-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contamination {
  RepositoryRoot,
  EvalHome,
  OtherRunScratch,
  TaskSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutOfTreeAccess {
  pub tool: String,
  pub path: String,
  pub contamination: Option<Contamination>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessPolicy {
  pub trees: Vec<PathBuf>,
  pub home: Option<PathBuf>,
  pub repo_root: PathBuf,
  pub eval_home: PathBuf,
  pub scratch_root: PathBuf,
  pub own_scratch: PathBuf,
  pub source_roots: Vec<PathBuf>,
}

impl AccessPolicy {
  fn within_tree(&self, path: &Path) -> bool {
    self.trees.iter().any(|tree| path.starts_with(normalize(tree)))
  }

  fn allowed_system_path(&self, path: &Path) -> bool {
    let home_toolchain = self
      .home
      .as_ref()
      .is_some_and(|home| HOME_TOOLCHAIN_DIRS.iter().any(|dir| path.starts_with(normalize(&home.join(dir)))));
    home_toolchain || SYSTEM_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
  }

  fn contamination(&self, path: &Path) -> Option<Contamination> {
    if path.starts_with(normalize(&self.eval_home)) {
      return Some(Contamination::EvalHome);
    }
    if path.starts_with(normalize(&self.repo_root)) {
      return Some(Contamination::RepositoryRoot);
    }
    if self.source_roots.iter().any(|root| path.starts_with(normalize(root))) {
      return Some(Contamination::TaskSource);
    }
    let scratch_root = normalize(&self.scratch_root);
    let own = normalize(&self.own_scratch);
    let other_scratch = path
      .strip_prefix(&scratch_root)
      .ok()
      .and_then(|rest| rest.components().next())
      .is_some_and(|first| first.as_os_str().to_string_lossy().starts_with(SCRATCH_PREFIX) && !path.starts_with(&own));
    other_scratch.then_some(Contamination::OtherRunScratch)
  }
}

pub fn out_of_tree_accesses(transcript: &Transcript, policy: &AccessPolicy) -> Vec<OutOfTreeAccess> {
  let mut accesses: Vec<OutOfTreeAccess> = Vec::new();
  for call in transcript.tool_calls.iter().filter(|call| !denied_by_bash_guard(call)) {
    for raw in candidate_paths(call) {
      let Some(path) = resolve(&raw, policy) else {
        continue;
      };
      if policy.within_tree(&path) || policy.allowed_system_path(&path) {
        continue;
      }
      let access = OutOfTreeAccess {
        tool: call.name.clone(),
        path: path.display().to_string(),
        contamination: policy.contamination(&path),
      };
      if !accesses.contains(&access) {
        accesses.push(access);
      }
    }
  }
  accesses
}

pub fn contaminated(accesses: &[OutOfTreeAccess]) -> bool {
  accesses.iter().any(|access| access.contamination.is_some())
}

fn candidate_paths(call: &ToolCall) -> Vec<String> {
  if call.name == "Bash" {
    return call.input_str("command").map(bash_paths).unwrap_or_default();
  }
  PATH_FIELDS
    .iter()
    .filter_map(|field| call.input_str(field))
    .filter(|value| looks_like_path(value))
    .map(str::to_owned)
    .collect()
}

fn bash_paths(command: &str) -> Vec<String> {
  command
    .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '<' | '>' | '`'))
    .map(|token| token.trim_matches(|c| matches!(c, '\'' | '"' | ',')))
    .flat_map(|token| std::iter::once(token).chain(token.split_once('=').map(|(_, value)| value)))
    .filter(|token| looks_like_path(token))
    .map(str::to_owned)
    .collect()
}

fn looks_like_path(value: &str) -> bool {
  value.starts_with('/') || value.starts_with("~/") || value == "~" || value == ".." || value.starts_with("../") || value.contains("/../") || value.ends_with("/..")
}

fn resolve(raw: &str, policy: &AccessPolicy) -> Option<PathBuf> {
  let expanded = match raw.strip_prefix('~') {
    Some(rest) => policy.home.as_ref()?.join(rest.trim_start_matches('/')),
    None => PathBuf::from(raw),
  };
  let absolute = if expanded.is_absolute() { expanded } else { policy.trees.first()?.join(expanded) };
  Some(normalize(&absolute))
}

fn normalize(path: &Path) -> PathBuf {
  let mut result = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        result.pop();
      }
      other => result.push(other.as_os_str()),
    }
  }
  match result.strip_prefix("/private") {
    Ok(rest) if rest.starts_with("var") || rest.starts_with("tmp") => Path::new("/").join(rest),
    _ => result,
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::super::bash_guard::BASH_GUARD_REASON;
  use super::super::transcript::ToolResult;
  use super::*;

  fn policy() -> AccessPolicy {
    AccessPolicy {
      trees: vec![PathBuf::from("/var/tmp/T/ae-111/tree"), PathBuf::from("/private/var/tmp/T/ae-111/tree")],
      home: Some(PathBuf::from("/Users/dev")),
      repo_root: PathBuf::from("/Users/dev/src/code-atlas"),
      eval_home: PathBuf::from("/Users/dev/src/code-atlas/eval"),
      scratch_root: PathBuf::from("/var/tmp/T"),
      own_scratch: PathBuf::from("/var/tmp/T/ae-111"),
      source_roots: vec![PathBuf::from("/Users/dev/src/storefront"), PathBuf::from("/private/var/cache/clones/storefront.git")],
    }
  }

  #[test]
  fn should_never_flag_a_bash_command_the_guard_refused() {
    let mut refused = call("Bash", json!({"command": "cat /Users/dev/src/code-atlas/eval/tasks/t01/hidden/x.rs"}));
    refused.result = Some(ToolResult {
      text: BASH_GUARD_REASON.to_owned(),
      is_error: true,
      ..ToolResult::default()
    });
    let ran = call("Bash", json!({"command": "cat /Users/dev/src/code-atlas/eval/tasks/t01/hidden/x.rs"}));
    let transcript = |calls: Vec<ToolCall>| Transcript {
      tool_calls: calls,
      ..Transcript::default()
    };
    assert_eq!(out_of_tree_accesses(&transcript(vec![refused]), &policy()), Vec::new());
    assert_eq!(
      out_of_tree_accesses(&transcript(vec![ran]), &policy()),
      vec![OutOfTreeAccess {
        tool: "Bash".to_owned(),
        path: "/Users/dev/src/code-atlas/eval/tasks/t01/hidden/x.rs".to_owned(),
        contamination: Some(Contamination::EvalHome),
      }]
    );
  }

  fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
      id: name.to_owned(),
      name: name.to_owned(),
      input,
      elapsed_ms: None,
      result: None,
      turn: 1,
    }
  }

  #[test]
  fn should_flag_out_of_tree_paths_and_classify_contamination() {
    let transcript = Transcript {
      tool_calls: vec![
        call("Read", json!({"file_path": "/private/var/tmp/T/ae-111/tree/src/lib.rs"})),
        call("Read", json!({"file_path": "/Users/dev/src/code-atlas/eval/tasks/t01/hidden/x.rs"})),
        call("Grep", json!({"pattern": "fn main", "path": "/Users/dev/src/code-atlas"})),
        call("Glob", json!({"pattern": "/var/tmp/T/ae-222/tree/**/*.rs"})),
        call("Bash", json!({"command": "cd .. && ls ../../ && cat /etc/hosts"})),
        call(
          "Bash",
          json!({"command": "cargo build --manifest-path=/Users/dev/other/Cargo.toml && ls ~/.cargo/registry /usr/bin 2>/dev/null"}),
        ),
        call("Bash", json!({"command": "cat src/../Cargo.toml"})),
        call("Edit", json!({"file_path": "/Users/dev/.ssh/config"})),
      ],
      ..Transcript::default()
    };
    let accesses = out_of_tree_accesses(&transcript, &policy());
    let summary: Vec<(&str, &str, Option<Contamination>)> = accesses.iter().map(|access| (access.tool.as_str(), access.path.as_str(), access.contamination)).collect();
    assert_eq!(
      summary,
      vec![
        ("Read", "/Users/dev/src/code-atlas/eval/tasks/t01/hidden/x.rs", Some(Contamination::EvalHome)),
        ("Grep", "/Users/dev/src/code-atlas", Some(Contamination::RepositoryRoot)),
        ("Glob", "/var/tmp/T/ae-222/tree/**/*.rs", Some(Contamination::OtherRunScratch)),
        ("Bash", "/var/tmp/T/ae-111", None),
        ("Bash", "/var/tmp/T", None),
        ("Bash", "/etc/hosts", None),
        ("Bash", "/Users/dev/other/Cargo.toml", None),
        ("Edit", "/Users/dev/.ssh/config", None),
      ]
    );
    assert!(contaminated(&accesses));
  }

  #[test]
  fn should_mark_a_run_contaminated_when_it_reaches_the_repository_or_clone_its_task_was_archived_from() {
    let transcript = Transcript {
      tool_calls: vec![
        call("Read", json!({"file_path": "/Users/dev/src/storefront/api/src/orders.ts"})),
        call(
          "Bash",
          json!({"command": "git -C /var/cache/clones/storefront.git log -3 && ls /Users/dev/src/storefront-notes"}),
        ),
        call("Grep", json!({"pattern": "settle", "path": "../../../../../Users/dev/src/storefront"})),
      ],
      ..Transcript::default()
    };
    let accesses = out_of_tree_accesses(&transcript, &policy());
    let summary: Vec<(&str, &str, Option<Contamination>)> = accesses.iter().map(|access| (access.tool.as_str(), access.path.as_str(), access.contamination)).collect();
    assert_eq!(
      summary,
      vec![
        ("Read", "/Users/dev/src/storefront/api/src/orders.ts", Some(Contamination::TaskSource)),
        ("Bash", "/var/cache/clones/storefront.git", Some(Contamination::TaskSource)),
        ("Bash", "/Users/dev/src/storefront-notes", None),
        ("Grep", "/Users/dev/src/storefront", Some(Contamination::TaskSource)),
      ]
    );
    assert!(contaminated(&accesses));
    let elsewhere = AccessPolicy {
      source_roots: Vec::new(),
      ..policy()
    };
    assert!(!contaminated(&out_of_tree_accesses(&transcript, &elsewhere)));
  }

  #[test]
  fn should_accept_in_tree_and_toolchain_paths() {
    let transcript = Transcript {
      tool_calls: vec![
        call("Read", json!({"file_path": "/var/tmp/T/ae-111/tree/a.rs"})),
        call(
          "Bash",
          json!({"command": "ls ~/.rustup/toolchains && /opt/homebrew/bin/node -v && grep -r x src/ > /dev/null"}),
        ),
        call("Glob", json!({"pattern": "**/*.rs"})),
      ],
      ..Transcript::default()
    };
    let accesses = out_of_tree_accesses(&transcript, &policy());
    assert_eq!(accesses, Vec::<OutOfTreeAccess>::new());
    assert!(!contaminated(&accesses));
  }
}
