use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::claude::ATLAS_SERVER_NAME;
use super::error::{AgentError, AgentResult};
use super::process::{CapturedOutput, ProcessExit, run_captured, tail};

pub const ATLAS_HOME_VAR: &str = "CODE_ATLAS_HOME";
const SANDBOX_PATH: &str = "/usr/bin:/bin";
const SANDBOX_CLEARED_VARS: [&str; 3] = ["RUST_LOG", "XDG_CONFIG_HOME", "XDG_DATA_HOME"];
const ERROR_TAIL_BYTES: usize = 2000;
const SYMBOL_FIELDS: [&str; 5] = ["key", "symbol", "qualified_name", "target", "name"];
const STRUCTURAL_FIELDS: [&str; 2] = ["structural_score", "jaccard"];
const LEXICAL_FIELDS: [&str; 1] = ["score"];
const VERDICT_FIELDS: [&str; 2] = ["verdict", "inferred_verdict"];
const LOCATION_FIELDS: [&str; 3] = ["location", "path", "file"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasCli {
  binary: PathBuf,
  home: PathBuf,
  timeout: Duration,
  sandbox: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarMatch {
  pub symbol: String,
  pub structural: Option<f64>,
  pub lexical: Option<f64>,
  pub verdict: Option<String>,
  pub location: Option<String>,
}

impl SimilarMatch {
  pub fn inferred_equivalent(&self) -> bool {
    self.verdict.as_deref().is_some_and(|verdict| {
      let verdict = verdict.to_ascii_lowercase();
      verdict.contains("equivalent") && !verdict.contains("not") && !verdict.contains("different")
    })
  }
}

impl AtlasCli {
  pub fn new(binary: impl Into<PathBuf>, home: impl Into<PathBuf>, timeout: Duration) -> Self {
    Self {
      binary: binary.into(),
      home: home.into(),
      timeout,
      sandbox: None,
    }
  }

  pub fn sandboxed(mut self, user_dir: impl Into<PathBuf>) -> Self {
    self.sandbox = Some(user_dir.into());
    self
  }

  pub fn home(&self) -> &Path {
    &self.home
  }

  pub fn binary(&self) -> &Path {
    &self.binary
  }

  pub fn project_create(&self, project: &str, repos: &[PathBuf], cwd: &Path) -> AgentResult<()> {
    let mut args = vec!["project".to_owned(), "create".to_owned(), project.to_owned()];
    for repo in repos {
      args.push("--repo".to_owned());
      args.push(repo.display().to_string());
    }
    self.run(&args, cwd).map(|_| ())
  }

  pub fn add_repo(&self, project: &str, root: &Path, name: &str, globs: &RepoGlobs, cwd: &Path) -> AgentResult<()> {
    self.run(&add_repo_args(project, root, name, globs), cwd).map(|_| ())
  }

  pub fn set_repo_excludes(&self, project: &str, repo: &str, exclude: &[String], cwd: &Path) -> AgentResult<()> {
    self.run(&set_repo_exclude_args(project, repo, exclude), cwd).map(|_| ())
  }

  pub fn index(&self, project: &str, cwd: &Path) -> AgentResult<CapturedOutput> {
    self.run(&["index".to_owned(), "--project".to_owned(), project.to_owned()], cwd)
  }

  pub fn agents_hooks(&self, cwd: &Path) -> AgentResult<CapturedOutput> {
    let args =
      ["agents", "hooks", "--json", "--binary"]
        .map(str::to_owned)
        .into_iter()
        .chain([self.binary.display().to_string(), "--home".to_owned(), self.home.display().to_string()]);
    self.run(&args.collect::<Vec<_>>(), cwd)
  }

  pub fn config_show(&self, project: &str, cwd: &Path) -> AgentResult<CapturedOutput> {
    self.run(&["config".to_owned(), "show".to_owned(), "--project".to_owned(), project.to_owned()], cwd)
  }

  pub fn doctor(&self, project: &str, cwd: &Path) -> AgentResult<CapturedOutput> {
    let args = ["doctor".to_owned(), "--project".to_owned(), project.to_owned()];
    let output = self.execute(&args, cwd);
    let detail = match &output.exit {
      ProcessExit::Exited { .. } => return Ok(output),
      ProcessExit::TimedOut => format!("timed out after {}s", self.timeout.as_secs()),
      ProcessExit::SpawnFailed { reason } => format!("cannot start: {reason}"),
    };
    Err(AgentError::Command {
      command: self.render(&args),
      detail,
    })
  }

  pub fn similar_snippet(&self, path: &str, start_line: u32, end_line: u32, cwd: &Path) -> AgentResult<Vec<SimilarMatch>> {
    let snippet = format!("{path}:{start_line}-{end_line}");
    let args = ["query", "similar", "--snippet", &snippet, "--json"].map(str::to_owned);
    let output = self.run(&args, cwd)?;
    let document: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| AgentError::Command {
      command: self.render(&args),
      detail: format!("stdout is not JSON: {error}"),
    })?;
    Ok(similar_matches(&document))
  }

  pub fn version(&self) -> Option<String> {
    let mut command = isolated_command(&self.binary);
    command.arg("--version").env(ATLAS_HOME_VAR, &self.home);
    let output = run_captured(command, None, Duration::from_secs(10));
    output.exit.succeeded().then(|| output.stdout.trim().to_owned())
  }

  pub fn mcp_config(&self, project: &str, tool_profile: Option<&str>) -> Value {
    let mut args = vec!["mcp", "--project", project];
    if let Some(profile) = tool_profile {
      args.extend(["--tools", profile]);
    }
    json!({
      "mcpServers": {
        ATLAS_SERVER_NAME: {
          "type": "stdio",
          "command": self.binary.display().to_string(),
          "args": args,
          "env": { ATLAS_HOME_VAR: self.home.display().to_string() }
        }
      }
    })
  }

  fn run(&self, args: &[String], cwd: &Path) -> AgentResult<CapturedOutput> {
    let output = self.execute(args, cwd);
    let detail = match &output.exit {
      ProcessExit::Exited { code: Some(0) } => return Ok(output),
      ProcessExit::Exited { code } => format!("exited with {code:?}: {}", tail(output.stderr.trim(), ERROR_TAIL_BYTES)),
      ProcessExit::TimedOut => format!("timed out after {}s", self.timeout.as_secs()),
      ProcessExit::SpawnFailed { reason } => format!("cannot start: {reason}"),
    };
    Err(AgentError::Command {
      command: self.render(args),
      detail,
    })
  }

  fn execute(&self, args: &[String], cwd: &Path) -> CapturedOutput {
    let mut command = isolated_command(&self.binary);
    command.args(args).current_dir(cwd).env(ATLAS_HOME_VAR, &self.home);
    if let Some(user_dir) = &self.sandbox {
      for name in SANDBOX_CLEARED_VARS {
        command.env_remove(name);
      }
      command
        .env("HOME", user_dir)
        .env("CLAUDE_CONFIG_DIR", user_dir.join("claude"))
        .env("CODEX_HOME", user_dir.join("codex"))
        .env("PATH", SANDBOX_PATH);
    }
    run_captured(command, None, self.timeout)
  }

  fn render(&self, args: &[String]) -> String {
    format!("{} {}", self.binary.display(), args.join(" "))
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoGlobs {
  pub include: Vec<String>,
  pub exclude: Vec<String>,
}

pub fn add_repo_args(project: &str, root: &Path, name: &str, globs: &RepoGlobs) -> Vec<String> {
  let mut args = vec![
    "project".to_owned(),
    "add-repo".to_owned(),
    project.to_owned(),
    root.display().to_string(),
    "--name".to_owned(),
    name.to_owned(),
  ];
  args.extend(glob_args("--include", &globs.include));
  args.extend(glob_args("--exclude", &globs.exclude));
  args
}

pub fn set_repo_exclude_args(project: &str, repo: &str, exclude: &[String]) -> Vec<String> {
  let mut args = ["project", "set-repo", project, repo].map(str::to_owned).to_vec();
  args.extend(glob_args("--exclude", exclude));
  args
}

fn glob_args<'a>(flag: &'a str, globs: &'a [String]) -> impl Iterator<Item = String> + 'a {
  globs.iter().flat_map(move |glob| [flag.to_owned(), glob.clone()])
}

pub fn isolated_command(binary: &Path) -> Command {
  let mut command = Command::new(binary);
  for (name, _) in std::env::vars_os() {
    if name.to_string_lossy().starts_with("CODE_ATLAS_") {
      command.env_remove(name);
    }
  }
  command
}

pub fn similar_matches(document: &Value) -> Vec<SimilarMatch> {
  if let Some(members) = document.pointer("/answer/members").and_then(Value::as_array) {
    return members.iter().filter_map(cluster_member).collect();
  }
  let mut matches = Vec::new();
  collect_matches(document, &mut matches);
  matches
}

fn cluster_member(member: &Value) -> Option<SimilarMatch> {
  let entity = member.get("entity")?;
  let symbol = entity.get("key")?.as_str()?.to_owned();
  let signals: Vec<&Value> = member.get("signals").and_then(Value::as_array).map(|signals| signals.iter().collect()).unwrap_or_default();
  let find = |kind: &str| signals.iter().find(|signal| signal.get("signal").and_then(Value::as_str) == Some(kind));
  let structural = find("structural").and_then(|signal| signal.get("jaccard")).and_then(Value::as_f64);
  let lexical = find("lexical").and_then(|signal| signal.get("score")).and_then(Value::as_f64);
  let verdict = match find("inferred") {
    Some(signal) if signal.get("verdict").and_then(Value::as_str) == Some("equivalent") => Some("inferred equivalent".to_owned()),
    Some(_) => Some("inferred similar, not equivalent".to_owned()),
    None => member.get("verdict").and_then(Value::as_str).map(str::to_owned),
  };
  let location = entity.get("location").and_then(|location| {
    let (repo, path) = (location.get("repo")?.as_str()?, location.get("path")?.as_str()?);
    Some(match location.get("line").and_then(Value::as_u64) {
      Some(line) => format!("{repo}:{path}:{line}"),
      None => format!("{repo}:{path}"),
    })
  });
  Some(SimilarMatch {
    symbol,
    structural,
    lexical,
    verdict,
    location,
  })
}

fn collect_matches(value: &Value, matches: &mut Vec<SimilarMatch>) {
  match value {
    Value::Array(items) => items.iter().for_each(|item| collect_matches(item, matches)),
    Value::Object(object) => {
      let symbol = first_string(object, &SYMBOL_FIELDS);
      let structural = first_number(object, &STRUCTURAL_FIELDS);
      let lexical = first_number(object, &LEXICAL_FIELDS);
      let verdict = first_string(object, &VERDICT_FIELDS);
      if let Some(symbol) = symbol.filter(|_| structural.is_some() || lexical.is_some() || verdict.is_some()) {
        matches.push(SimilarMatch {
          symbol,
          structural,
          lexical,
          verdict,
          location: first_string(object, &LOCATION_FIELDS),
        });
      }
      object.values().for_each(|nested| collect_matches(nested, matches));
    }
    _ => {}
  }
}

fn first_string(object: &serde_json::Map<String, Value>, fields: &[&str]) -> Option<String> {
  fields.iter().find_map(|field| object.get(*field).and_then(Value::as_str)).map(str::to_owned)
}

fn first_number(object: &serde_json::Map<String, Value>, fields: &[&str]) -> Option<f64> {
  fields.iter().find_map(|field| object.get(*field).and_then(Value::as_f64))
}

pub fn locate_atlas_bin(explicit: Option<&Path>, repo_root: &Path) -> Option<PathBuf> {
  if let Some(path) = explicit {
    return Some(path.to_owned());
  }
  let built = ["release", "debug"]
    .iter()
    .map(|profile| repo_root.join("target").join(profile).join("catl"))
    .find(|path| path.is_file());
  built.or_else(|| {
    std::env::var_os("PATH")
      .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join("catl")).collect::<Vec<_>>())
      .unwrap_or_default()
      .into_iter()
      .find(|path| path.is_file())
  })
}

#[cfg(test)]
mod tests {
  use super::super::plan::AtlasSurface;
  use super::*;

  #[test]
  fn should_render_the_mcp_config_with_an_isolated_home() {
    let cli = AtlasCli::new("/bin/catl", "/runs/r1/atlas", Duration::from_secs(1));
    assert_eq!(
      cli.mcp_config("eval-r1", None),
      json!({"mcpServers": {"code-atlas": {"type": "stdio", "command": "/bin/catl", "args": ["mcp", "--project", "eval-r1"], "env": {"CODE_ATLAS_HOME": "/runs/r1/atlas"}}}})
    );
  }

  #[test]
  fn should_serve_only_the_core_profile_to_a_forced_session() {
    let cli = AtlasCli::new("/bin/catl", "/runs/r1/atlas", Duration::from_secs(1));
    assert_eq!(
      cli.mcp_config("eval-r1", AtlasSurface::Forced.mcp_tool_profile()).to_string(),
      r#"{"mcpServers":{"code-atlas":{"args":["mcp","--project","eval-r1","--tools","core"],"command":"/bin/catl","env":{"CODE_ATLAS_HOME":"/runs/r1/atlas"},"type":"stdio"}}}"#
    );
    assert_eq!(AtlasSurface::Standard.mcp_tool_profile(), None);
  }

  #[test]
  fn should_collect_scored_or_judged_symbols_at_any_depth() {
    let document = json!({
      "target": "snippet",
      "clusters": [{
        "members": [
          {"key": "rust:billing:billing:money::to_cents#function", "structural_score": 0.91, "location": "billing:src/money.rs:3"},
          {"symbol": "ts:web:web:src/utils/money::toCents#function", "verdict": "inferred equivalent (0.9)"},
          {"key": "unscored"}
        ]
      }]
    });
    let matches = similar_matches(&document);
    assert_eq!(matches.len(), 2);
    assert_eq!((matches[0].structural, matches[0].lexical), (Some(0.91), None));
    assert_eq!(matches[0].location.as_deref(), Some("billing:src/money.rs:3"));
    assert!(matches[1].inferred_equivalent());
    assert!(!matches[0].inferred_equivalent());
  }

  #[test]
  fn should_parse_the_similar_answer_printed_by_the_cli() {
    let document: Value = serde_json::from_str(include_str!("testdata/similar_snippet.json")).unwrap();
    let matches = similar_matches(&document);
    assert_eq!(
      matches,
      vec![SimilarMatch {
        symbol: "ts:acme-sdk:@acme/sdk:src/money::toCents#function".to_owned(),
        structural: None,
        lexical: Some(4.588),
        verdict: Some("related by name only, behaviour unverified".to_owned()),
        location: Some("acme-sdk:src/money.ts:1".to_owned()),
      }]
    );
    assert!(!matches[0].inferred_equivalent());
  }

  #[test]
  fn should_read_an_inferred_verdict_from_the_signals_not_the_rationale() {
    let document = json!({"answer": {"members": [
      {"entity": {"key": "k1"}, "signals": [{"signal": "structural", "jaccard": 0.97}, {"signal": "inferred", "verdict": "equivalent", "rationale": "does not differ"}], "verdict": "inferred equivalent (0.90): does not differ"},
      {"entity": {"key": "k2"}, "signals": [{"signal": "inferred", "verdict": "different", "rationale": "x"}], "verdict": "inferred similar, not equivalent (0.80): rounding"}
    ]}});
    let matches = similar_matches(&document);
    assert_eq!((matches[0].structural, matches[0].lexical), (Some(0.97), None));
    assert!(matches[0].inferred_equivalent());
    assert!(!matches[1].inferred_equivalent());
  }

  #[test]
  fn should_not_read_negative_verdicts_as_equivalence() {
    let not_equivalent = SimilarMatch {
      symbol: "x".to_owned(),
      structural: None,
      lexical: None,
      verdict: Some("similar_but_different".to_owned()),
      location: None,
    };
    assert!(!not_equivalent.inferred_equivalent());
  }

  #[test]
  fn should_surface_a_missing_binary_as_a_command_error() {
    let cli = AtlasCli::new("/definitely/missing/catl", "/tmp/atlas-home", Duration::from_secs(1));
    let error = cli.index("p", Path::new("/")).unwrap_err();
    assert!(matches!(error, AgentError::Command { .. }), "got {error:?}");
  }
}
