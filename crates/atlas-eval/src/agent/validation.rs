use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::process::{ProcessExit, run_captured, tail};

const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCounts {
  pub passed: u64,
  pub failed: u64,
  pub ignored: u64,
}

impl TestCounts {
  pub fn add(&self, other: &Self) -> Self {
    Self {
      passed: self.passed + other.passed,
      failed: self.failed + other.failed,
      ignored: self.ignored + other.ignored,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
  pub command: String,
  pub exit: ProcessExit,
  pub duration_ms: u64,
  pub counts: Option<TestCounts>,
  pub failing: Vec<String>,
  pub output_tail: String,
}

impl CommandResult {
  pub fn passed(&self) -> bool {
    self.exit.succeeded()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteResult {
  pub commands: Vec<CommandResult>,
}

impl SuiteResult {
  pub fn all_passed(&self) -> bool {
    self.commands.iter().all(CommandResult::passed)
  }

  pub fn counts(&self) -> Option<TestCounts> {
    self.commands.iter().filter_map(|command| command.counts).reduce(|total, counts| total.add(&counts))
  }

  pub fn passed_commands(&self) -> usize {
    self.commands.iter().filter(|command| command.passed()).count()
  }
}

pub fn run_suite(commands: &[String], tree: &Path, env: &[(String, String)], timeout: Duration) -> SuiteResult {
  let commands = commands.iter().map(|command| run_command(command, tree, env, timeout)).collect();
  SuiteResult { commands }
}

pub fn run_command(command: &str, tree: &Path, env: &[(String, String)], timeout: Duration) -> CommandResult {
  let mut shell = Command::new("sh");
  shell.arg("-c").arg(command).current_dir(tree);
  for (key, _) in std::env::vars().filter(|(key, _)| key.starts_with("CLAUDE")) {
    shell.env_remove(key);
  }
  shell.envs(env.iter().map(|(key, value)| (key, value)));
  let output = run_captured(shell, None, timeout);
  let combined = format!("{}\n{}", output.stdout, output.stderr);
  tracing::info!(command, exit = ?output.exit, duration_ms = output.duration.as_millis() as u64, "validation command finished");
  CommandResult {
    command: command.to_owned(),
    exit: output.exit,
    duration_ms: u64::try_from(output.duration.as_millis()).unwrap_or(u64::MAX),
    counts: parse_counts(&combined),
    failing: failing_tests(&combined),
    output_tail: tail(&combined, OUTPUT_TAIL_BYTES),
  }
}

pub fn parse_counts(output: &str) -> Option<TestCounts> {
  let total = output.lines().filter_map(|line| summary_counts(line.trim())).reduce(|total, counts| total.add(&counts));
  total.or_else(|| tap_counts(output)).or_else(|| bun_counts(output))
}

fn bun_counts(output: &str) -> Option<TestCounts> {
  let lines: Vec<&str> = output.lines().map(str::trim).collect();
  let closes_summary = |line: &str| line.starts_with("Ran ") && (line.contains(" tests across ") || line.contains(" test across "));
  let mut total: Option<TestCounts> = None;
  for end in (0..lines.len()).filter(|index| closes_summary(lines[*index])) {
    let block = lines[..end].iter().rev().map_while(|line| bun_summary_line(line));
    let counts = block.fold(TestCounts::default(), |counts, (count, label)| match label {
      "pass" => TestCounts {
        passed: counts.passed + count,
        ..counts
      },
      "fail" => TestCounts {
        failed: counts.failed + count,
        ..counts
      },
      "skip" | "todo" => TestCounts {
        ignored: counts.ignored + count,
        ..counts
      },
      _ => counts,
    });
    total = Some(total.map_or(counts, |sum| sum.add(&counts)));
  }
  total
}

fn bun_summary_line(line: &str) -> Option<(u64, &str)> {
  let (count, label) = line.split_once(' ')?;
  Some((count.parse().ok()?, label.trim()))
}

fn summary_counts(line: &str) -> Option<TestCounts> {
  if let Some(rest) = line.strip_prefix("test result: ") {
    return Some(TestCounts {
      passed: count_before(rest, "passed"),
      failed: count_before(rest, "failed"),
      ignored: count_before(rest, "ignored"),
    });
  }
  let rest = line.strip_prefix("Tests:").or_else(|| line.strip_prefix("Tests ").filter(|rest| rest.contains('(')))?;
  Some(TestCounts {
    passed: count_before(rest, "passed"),
    failed: count_before(rest, "failed"),
    ignored: count_before(rest, "skipped") + count_before(rest, "todo"),
  })
}

fn tap_counts(output: &str) -> Option<TestCounts> {
  let value = |prefix: &str| output.lines().find_map(|line| line.trim().strip_prefix(prefix)?.trim().parse::<u64>().ok());
  let passed = value("# pass ");
  let failed = value("# fail ");
  (passed.is_some() || failed.is_some()).then(|| TestCounts {
    passed: passed.unwrap_or(0),
    failed: failed.unwrap_or(0),
    ignored: value("# skipped ").unwrap_or(0) + value("# todo ").unwrap_or(0),
  })
}

fn count_before(text: &str, word: &str) -> u64 {
  let tokens: Vec<&str> = text
    .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '|' | '.' | '(' | ')'))
    .filter(|token| !token.is_empty())
    .collect();
  tokens.windows(2).filter(|pair| pair[1] == word).filter_map(|pair| pair[0].parse::<u64>().ok()).sum()
}

pub fn failing_tests(output: &str) -> Vec<String> {
  let mut failing: Vec<String> = output
    .lines()
    .filter_map(|line| {
      let trimmed = line.trim();
      if let Some(name) = trimmed.strip_prefix("test ").and_then(|rest| rest.strip_suffix(" ... FAILED")) {
        return Some(name.to_owned());
      }
      if let Some(name) = trimmed.strip_prefix("✕ ").or_else(|| trimmed.strip_prefix("× ")) {
        return Some(strip_duration(name).to_owned());
      }
      if let Some(name) = trimmed.strip_prefix("(fail) ") {
        return Some(strip_bun_duration(name).to_owned());
      }
      trimmed.strip_prefix("not ok ").map(|rest| rest.split_once(" - ").map_or(rest, |(_, name)| name).to_owned())
    })
    .collect();
  failing.sort();
  failing.dedup();
  failing
}

fn strip_duration(name: &str) -> &str {
  match name.rsplit_once(" (") {
    Some((head, tail)) if tail.ends_with("ms)") || tail.ends_with(" s)") => head,
    _ => name,
  }
}

fn strip_bun_duration(name: &str) -> &str {
  match name.rsplit_once(" [") {
    Some((head, tail)) if tail.ends_with("ms]") || tail.ends_with("s]") => head,
    _ => name,
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Regressions {
  pub new_failures: Vec<String>,
  pub preexisting_failures: Vec<String>,
  pub unattributed_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum RegressionVerdict {
  Clean,
  Introduced { count: usize },
  Unknown { reason: String },
}

impl Regressions {
  pub fn verdict(&self) -> RegressionVerdict {
    if !self.new_failures.is_empty() {
      return RegressionVerdict::Introduced { count: self.new_failures.len() };
    }
    if !self.unattributed_commands.is_empty() {
      return RegressionVerdict::Unknown {
        reason: format!("failing at base too and not itemised: {}", self.unattributed_commands.join("; ")),
      };
    }
    RegressionVerdict::Clean
  }

  pub fn count(&self) -> Option<usize> {
    match self.verdict() {
      RegressionVerdict::Clean => Some(0),
      RegressionVerdict::Introduced { count } => Some(count),
      RegressionVerdict::Unknown { .. } => None,
    }
  }
}

pub fn regressions(base: Option<&SuiteResult>, run: &SuiteResult) -> Regressions {
  let mut result = Regressions::default();
  for (index, command) in run.commands.iter().enumerate() {
    if command.passed() {
      continue;
    }
    let base_command = base
      .and_then(|base| base.commands.get(index))
      .filter(|base_command| base_command.command == command.command);
    let base_failing: BTreeSet<&String> = base_command.map(|base_command| base_command.failing.iter().collect()).unwrap_or_default();
    if command.failing.is_empty() {
      match base_command {
        Some(base_command) if !base_command.passed() => result.unattributed_commands.push(command.command.clone()),
        _ => result.new_failures.push(format!("{}: {:?}", command.command, command.exit)),
      }
      continue;
    }
    for failure in &command.failing {
      let bucket = if base_failing.contains(failure) {
        &mut result.preexisting_failures
      } else {
        &mut result.new_failures
      };
      bucket.push(failure.clone());
    }
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_sum_cargo_test_result_lines() {
    let output = "running 3 tests\ntest a::b ... ok\ntest a::c ... FAILED\ntest result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured\n\ntest result: ok. 4 passed; 0 failed; 0 ignored;\n";
    assert_eq!(parse_counts(output), Some(TestCounts { passed: 6, failed: 1, ignored: 1 }));
    assert_eq!(failing_tests(output), vec!["a::c"]);
  }

  #[test]
  fn should_parse_jest_and_vitest_summaries() {
    assert_eq!(
      parse_counts("Tests:       1 failed, 2 skipped, 3 passed, 6 total\n"),
      Some(TestCounts { passed: 3, failed: 1, ignored: 2 })
    );
    assert_eq!(
      parse_counts("      Tests  1 failed | 2 passed (3)\n"),
      Some(TestCounts { passed: 2, failed: 1, ignored: 0 })
    );
    assert_eq!(failing_tests("    ✕ should reject a missing user (5 ms)\n    ✓ ok\n"), vec!["should reject a missing user"]);
  }

  #[test]
  fn should_parse_node_test_tap_output() {
    let output = "not ok 2 - deleted_at is exposed\n# pass 1\n# fail 1\n";
    assert_eq!(parse_counts(output), Some(TestCounts { passed: 1, failed: 1, ignored: 0 }));
    assert_eq!(failing_tests(output), vec!["deleted_at is exposed"]);
  }

  #[test]
  fn should_parse_bun_test_summaries_and_failing_test_names() {
    let output = "tests/billing/invoice.spec.ts:\n(pass) settleInvoice > should keep a paid invoice paid [0.05ms]\n(fail) settleInvoice > should refuse a second [partial] payment [1.25ms]\n\n 1 pass\n 2 skip\n 1 fail\n 7 expect() calls\nRan 4 tests across 1 file. [90.00ms]\n";
    assert_eq!(parse_counts(output), Some(TestCounts { passed: 1, failed: 1, ignored: 2 }));
    assert_eq!(failing_tests(output), vec!["settleInvoice > should refuse a second [partial] payment"]);
    let two_suites = "\n 41 pass\n 0 fail\nRan 41 tests across 5 files. [70.00ms]\n\n 12 pass\n 1 todo\n 3 fail\nRan 16 tests across 2 files. [1.20s]\n";
    assert_eq!(
      parse_counts(two_suites),
      Some(TestCounts {
        passed: 53,
        failed: 3,
        ignored: 1
      })
    );
    assert_eq!(
      parse_counts("\n 1 pass\n 0 fail\nRan 1 test across 1 file. [8.00ms]\n"),
      Some(TestCounts { passed: 1, failed: 0, ignored: 0 })
    );
    assert_eq!(parse_counts("the queue has 3 pass\n 2 fail\n"), None);
  }

  #[test]
  fn should_count_only_the_bun_summary_block_when_a_test_prints_lines_that_look_like_it() {
    let output = "till.test.ts:\n(fail) till > should balance [0.10ms]\n 7 fail\n12 pass\n(pass) till > should print its own tally [0.20ms]\n(fail) till > should close the day [50.00ms]\n  ^ this test timed out after 50ms.\n\n 2 pass\n 1 skip\n 1 todo\n 4 fail\n 5 expect() calls\nRan 8 tests across 1 file. [60.00ms]\n";
    assert_eq!(parse_counts(output), Some(TestCounts { passed: 2, failed: 4, ignored: 2 }));
    let printed_last = "(pass) till > should print a tally [0.20ms]\n 9 fail\n\n 1 pass\n 0 fail\nRan 1 test across 1 file. [5.00ms]\n";
    assert_eq!(parse_counts(printed_last), Some(TestCounts { passed: 1, failed: 0, ignored: 0 }));
    assert_eq!(parse_counts("Ran 0 tests across 1 file. [2.00ms]\n"), Some(TestCounts::default()));
  }

  #[test]
  fn should_return_no_counts_for_unrecognised_output() {
    assert_eq!(parse_counts("all good\n"), None);
  }

  fn command(name: &str, code: i32, failing: &[&str]) -> CommandResult {
    CommandResult {
      command: name.to_owned(),
      exit: ProcessExit::Exited { code: Some(code) },
      duration_ms: 1,
      counts: None,
      failing: failing.iter().map(|failure| (*failure).to_owned()).collect(),
      output_tail: String::new(),
    }
  }

  #[test]
  fn should_separate_new_failures_from_failures_already_present_at_base() {
    let base = SuiteResult {
      commands: vec![command("cargo test", 101, &["a::flaky"])],
    };
    let run = SuiteResult {
      commands: vec![command("cargo test", 101, &["a::flaky", "b::broken"])],
    };
    let found = regressions(Some(&base), &run);
    assert_eq!(found.new_failures, vec!["b::broken"]);
    assert_eq!(found.preexisting_failures, vec!["a::flaky"]);
    assert_eq!(found.verdict(), RegressionVerdict::Introduced { count: 1 });
  }

  #[test]
  fn should_mark_unitemised_failures_unknown_when_the_base_also_failed() {
    let base = SuiteResult {
      commands: vec![command("make check", 2, &[])],
    };
    let run = SuiteResult {
      commands: vec![command("make check", 2, &[])],
    };
    assert_eq!(regressions(Some(&base), &run).count(), None);
    assert_eq!(regressions(None, &run).count(), Some(1));
  }

  #[test]
  fn should_report_clean_when_every_command_passes() {
    let run = SuiteResult {
      commands: vec![command("true", 0, &[])],
    };
    assert_eq!(regressions(None, &run).verdict(), RegressionVerdict::Clean);
  }

  #[test]
  fn should_run_commands_in_the_tree_and_capture_exit_codes() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("marker"), "x").unwrap();
    let suite = run_suite(&["test -f marker".to_owned(), "exit 4".to_owned()], tree.path(), &[], Duration::from_secs(10));
    assert_eq!(suite.commands[0].exit, ProcessExit::Exited { code: Some(0) });
    assert_eq!(suite.commands[1].exit, ProcessExit::Exited { code: Some(4) });
    assert_eq!(suite.passed_commands(), 1);
    assert!(!suite.all_passed());
  }
}
