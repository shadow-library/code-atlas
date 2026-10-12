use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::claude::project_instructions;
use super::error::{AgentError, AgentResult};
use super::headroom::read_tree;
use super::provenance::candidate_terms;
use super::record::write_json;
use super::spec::{LoadedTask, PromptLeak};
use super::workspace::{StripPlan, materialize_task, resolve_source};

pub const HANDED_MAX_FILES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeakTerm {
  pub term: String,
  pub files: usize,
  pub must_change: usize,
  pub naming: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakReason {
  ReturnsFile,
  ReturnsPointer,
}

impl LeakReason {
  pub fn label(self) -> &'static str {
    match self {
      Self::ReturnsFile => "a term returns a file to change",
      Self::ReturnsPointer => "a term returns a file that names one",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReach {
  pub terms: usize,
  pub reaching: Vec<LeakTerm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeak {
  pub task_id: String,
  pub must_change: usize,
  pub must_change_absent: Vec<String>,
  pub prompt: TextReach,
  pub instructions: Option<TextReach>,
  pub class: PromptLeak,
  pub reasons: Vec<LeakReason>,
  pub declared: Option<PromptLeak>,
}

impl TaskLeak {
  pub fn disagrees(&self) -> bool {
    self.declared.is_some_and(|declared| declared != self.class)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeakReport {
  pub handed_max_files: usize,
  pub tasks: Vec<TaskLeak>,
}

pub fn classify_leak(reaching: &[LeakTerm]) -> (PromptLeak, Vec<LeakReason>) {
  let Some(fewest) = reaching.iter().map(|term| term.files).min() else {
    return (PromptLeak::Open, Vec::new());
  };
  let class = if fewest <= HANDED_MAX_FILES { PromptLeak::Handed } else { PromptLeak::Guided };
  let deciding = reaching.iter().filter(|term| class == PromptLeak::Guided || term.files <= HANDED_MAX_FILES);
  let reasons: BTreeSet<LeakReason> = deciding
    .flat_map(|term| {
      [
        (term.must_change > 0).then_some(LeakReason::ReturnsFile),
        (term.naming > 0).then_some(LeakReason::ReturnsPointer),
      ]
    })
    .flatten()
    .collect();
  (class, reasons.into_iter().collect())
}

fn file_name(path: &str) -> &str {
  path.rsplit('/').next().unwrap_or(path)
}

fn pointers(contents: &BTreeMap<String, String>, must_change: &[String]) -> Vec<String> {
  let mut pointers: Vec<String> = must_change.to_vec();
  for path in must_change {
    let name = file_name(path);
    if name != path && contents.keys().filter(|other| file_name(other) == name).count() <= 1 {
      pointers.push(name.to_owned());
    }
  }
  pointers
}

pub fn reach(text: &str, contents: &BTreeMap<String, String>, must_change: &[String]) -> TextReach {
  let pointers = pointers(contents, must_change);
  let naming: BTreeSet<&String> = contents
    .iter()
    .filter(|(path, content)| !must_change.contains(path) && pointers.iter().any(|pointer| content.contains(pointer.as_str())))
    .map(|(path, _)| path)
    .collect();
  let terms = candidate_terms(text);
  let mut reaching: Vec<LeakTerm> = terms
    .iter()
    .map(|term| {
      let files: Vec<&String> = contents.iter().filter(|(_, content)| content.contains(term.as_str())).map(|(path, _)| path).collect();
      LeakTerm {
        term: term.clone(),
        files: files.len(),
        must_change: files.iter().filter(|path| must_change.contains(path)).count(),
        naming: files.iter().filter(|path| naming.contains(*path)).count(),
      }
    })
    .filter(|term| term.must_change + term.naming > 0)
    .collect();
  reaching.sort_by(|a, b| (a.files, std::cmp::Reverse(a.must_change), &a.term).cmp(&(b.files, std::cmp::Reverse(b.must_change), &b.term)));
  TextReach { terms: terms.len(), reaching }
}

pub fn audit_task(task: &LoadedTask, contents: &BTreeMap<String, String>, instructions: Option<&str>) -> TaskLeak {
  let truth = &task.spec.ground_truth;
  let prompt = reach(&task.spec.prompt, contents, &truth.must_change);
  let (class, reasons) = classify_leak(&prompt.reaching);
  TaskLeak {
    task_id: task.spec.id.clone(),
    must_change: truth.must_change.len(),
    must_change_absent: truth.must_change.iter().filter(|path| !contents.contains_key(*path)).cloned().collect(),
    class,
    reasons,
    prompt,
    instructions: instructions.map(|text| reach(text, contents, &truth.must_change)),
    declared: task.spec.prompt_leak,
  }
}

pub fn audit_leaks(tasks: &[LoadedTask], repo_root: &Path, cache_dir: &Path) -> AgentResult<LeakReport> {
  let mut audited = Vec::new();
  for task in tasks {
    let scratch = tempfile::tempdir().map_err(|error| AgentError::io(std::env::temp_dir(), error))?;
    let tree = scratch.path().join("tree");
    let source = resolve_source(task, repo_root, None, cache_dir)?;
    materialize_task(task, &source, &tree, &StripPlan::for_task(task, false))?;
    let mut contents = BTreeMap::new();
    read_tree(&tree, "", &mut contents)?;
    let instructions = project_instructions(&tree).map(|(text, _)| text);
    audited.push(audit_task(task, &contents, instructions.as_deref()));
  }
  Ok(LeakReport {
    handed_max_files: HANDED_MAX_FILES,
    tasks: audited,
  })
}

fn reach_cell(reach: &TextReach) -> String {
  if reach.reaching.is_empty() {
    return format!("0 of {}", reach.terms);
  }
  let listed: Vec<String> = reach
    .reaching
    .iter()
    .map(|term| {
      let naming = if term.naming == 0 { String::new() } else { format!(", {} naming one", term.naming) };
      format!("`{}` ({} of {} files{naming})", term.term.replace('|', "\\|"), term.must_change, term.files)
    })
    .collect();
  format!("{} of {}: {}", reach.reaching.len(), reach.terms, listed.join(", "))
}

pub fn render_leak_markdown(report: &LeakReport) -> String {
  let mut out = String::from("# Prompt leak audit\n\n");
  out.push_str(
    "Every code span, identifier-shaped token and quoted phrase of a prompt is searched as a fixed string in the task's base tree, the way `agent headroom` derives its entry points. \
     A term reaches a task in one of two ways: a file it returns is one of `must_change`, or a file it returns names one \
     (it contains the path of a `must_change` file, or its file name when no other file of the tree has that name).\n\n",
  );
  out.push_str(&format!(
    "- `handed`: a prompt term returns at most {} files and reaches the task through one of them.\n- `guided`: a term reaches the task among more files.\n- `open`: no prompt term reaches it.\n\n",
    report.handed_max_files
  ));
  out.push_str("A label or message the prompt cites without quotes is not a term and is not audited; the prompt rule is to quote them.\n\n");
  let count = |class: PromptLeak| report.tasks.iter().filter(|task| task.class == class).count();
  out.push_str(&format!(
    "Tasks: {}. handed {}, guided {}, open {}.\n\n",
    report.tasks.len(),
    count(PromptLeak::Handed),
    count(PromptLeak::Guided),
    count(PromptLeak::Open)
  ));
  let disagreeing: Vec<&str> = report.tasks.iter().filter(|task| task.disagrees()).map(|task| task.task_id.as_str()).collect();
  if !disagreeing.is_empty() {
    out.push_str(&format!("**Declared class differs from the audit:** {}.\n\n", disagreeing.join(", ")));
  }
  out.push_str(
    "A cell reads `reaching terms of terms checked`, then each reaching term as `term (must_change files among the files returned of files returned, files among them that name one)`, most selective first.\n\n",
  );
  out.push_str("| task | class | reason | declared | `must_change` files | prompt | injected instructions |\n|---|---|---|---|---|---|---|\n");
  for task in &report.tasks {
    let absent = if task.must_change_absent.is_empty() {
      String::new()
    } else {
      format!(" ({} created by the task)", task.must_change_absent.len())
    };
    let reasons: Vec<&str> = task.reasons.iter().map(|reason| reason.label()).collect();
    out.push_str(&format!(
      "| {} | {} | {} | {} | {}{} | {} | {} |\n",
      task.task_id,
      task.class.label(),
      if reasons.is_empty() { "-".to_owned() } else { reasons.join("; ") },
      task.declared.map_or("-", PromptLeak::label),
      task.must_change,
      absent,
      reach_cell(&task.prompt),
      task.instructions.as_ref().map_or_else(|| "none injected".to_owned(), reach_cell)
    ));
  }
  out
}

pub fn write_leak_report(report: &LeakReport, out_base: &Path) -> AgentResult<(PathBuf, PathBuf)> {
  if let Some(parent) = out_base.parent() {
    fs::create_dir_all(parent).map_err(|error| AgentError::io(parent, error))?;
  }
  let markdown = out_base.with_extension("md");
  let json = out_base.with_extension("json");
  fs::write(&markdown, render_leak_markdown(report)).map_err(|error| AgentError::io(&markdown, error))?;
  write_json(&json, report)?;
  Ok((markdown, json))
}

#[cfg(test)]
mod tests {
  use super::super::spec::{TASK_FILE, load_task};
  use super::*;

  fn task(prompt: &str, must_change: &[&str], declared: Option<&str>) -> (tempfile::TempDir, LoadedTask) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("p00-sample");
    fs::create_dir_all(&dir).unwrap();
    let listed: Vec<String> = must_change.iter().map(|path| format!("\"{path}\"")).collect();
    let declared = declared.map(|class| format!("prompt_leak = \"{class}\"\n")).unwrap_or_default();
    let spec = format!(
      "id = \"p00-sample\"\ntitle = \"Sample\"\nrepo = \"self\"\nbase_commit = \"abcdef1\"\nprompt = '''{prompt}'''\ntimeout_minutes = 5\nmax_turns = 30\n{declared}[ground_truth]\nmust_change = [{}]\n",
      listed.join(", ")
    );
    fs::write(dir.join(TASK_FILE), spec).unwrap();
    let loaded = load_task(&dir).unwrap();
    (root, loaded)
  }

  fn tree() -> BTreeMap<String, String> {
    [
      (
        "server/src/billing/invoice.service.ts",
        "export class InvoiceService { settle() { throw new Error('invoice already settled'); } }",
      ),
      ("server/src/billing/invoice.controller.ts", "import { InvoiceService } from './invoice.service';"),
      ("server/src/billing/index.ts", "export * from './invoice.service';"),
      (
        "server/src/orders/order.service.ts",
        "import { InvoiceService } from '../billing/invoice.service'; // settle",
      ),
      ("server/src/orders/index.ts", "export * from './order.service';"),
      ("web/src/pages/invoices.tsx", "const label = 'Settle invoice'; InvoiceService;"),
      ("web/src/pages/orders.tsx", "const label = 'Orders';"),
      (
        "suite/checkout.spec.ts",
        "// skipped: paying twice answers 'double charge refused'; the cause is in invoice.service.ts:1 (settle)",
      ),
    ]
    .into_iter()
    .map(|(path, content)| (path.to_owned(), content.to_owned()))
    .collect()
  }

  fn term(term: &str, files: usize, must_change: usize, naming: usize) -> LeakTerm {
    LeakTerm {
      term: term.to_owned(),
      files,
      must_change,
      naming,
    }
  }

  #[test]
  fn should_class_a_prompt_by_its_most_selective_reaching_term_and_say_how_that_term_reaches() {
    assert_eq!(classify_leak(&[]), (PromptLeak::Open, Vec::new()));
    assert_eq!(classify_leak(&[term("InvoiceService", 4, 1, 0)]), (PromptLeak::Guided, vec![LeakReason::ReturnsFile]));
    assert_eq!(
      classify_leak(&[term("InvoiceService", 4, 1, 0), term("invoice already settled", 3, 1, 0)]),
      (PromptLeak::Handed, vec![LeakReason::ReturnsFile])
    );
    assert_eq!(
      classify_leak(&[term("double charge refused", 1, 0, 1), term("InvoiceService", 4, 1, 0)]),
      (PromptLeak::Handed, vec![LeakReason::ReturnsPointer])
    );
    assert_eq!(
      classify_leak(&[term("settle", 5, 1, 1), term("charge", 9, 0, 1)]),
      (PromptLeak::Guided, vec![LeakReason::ReturnsFile, LeakReason::ReturnsPointer])
    );
  }

  #[test]
  fn should_call_a_prompt_handed_when_a_quoted_error_text_returns_the_file_to_change() {
    let (_root, task) = task(
      "Paying twice fails with \"invoice already settled\" but still charges the card.",
      &["server/src/billing/invoice.service.ts"],
      None,
    );
    let audit = audit_task(&task, &tree(), None);
    assert_eq!(audit.prompt.reaching, vec![term("invoice already settled", 1, 1, 0)]);
    assert_eq!((audit.class, audit.declared, audit.disagrees()), (PromptLeak::Handed, None, false));
    assert_eq!(audit.reasons, vec![LeakReason::ReturnsFile]);
    assert_eq!(audit.instructions, None);
  }

  #[test]
  fn should_call_a_prompt_handed_when_its_error_text_returns_one_file_that_names_the_file_to_change() {
    let (_root, task) = task("Paying twice answers \"double charge refused\".", &["server/src/billing/invoice.service.ts"], Some("open"));
    let audit = audit_task(&task, &tree(), None);
    assert_eq!(audit.prompt.reaching, vec![term("double charge refused", 1, 0, 1)]);
    assert_eq!((audit.class, audit.disagrees()), (PromptLeak::Handed, true));
    assert_eq!(audit.reasons, vec![LeakReason::ReturnsPointer]);
  }

  #[test]
  fn should_count_a_bare_file_name_as_a_pointer_only_when_no_other_file_shares_it() {
    let contents = tree();
    let shared_name = reach("Orders page shows \"Orders\".", &contents, &["server/src/orders/index.ts".to_owned()]);
    assert_eq!(shared_name.reaching, Vec::new());
    let by_path: BTreeMap<String, String> = [
      ("server/src/orders/index.ts", "export {};"),
      ("server/src/billing/index.ts", "export {};"),
      ("docs/notes.md", "The \"Orders\" total is computed in server/src/orders/index.ts."),
    ]
    .into_iter()
    .map(|(path, content)| (path.to_owned(), content.to_owned()))
    .collect();
    let named = reach("Orders page shows \"Orders\".", &by_path, &["server/src/orders/index.ts".to_owned()]);
    assert_eq!(named.reaching, vec![term("Orders", 1, 0, 1)]);
  }

  #[test]
  fn should_call_a_prompt_guided_when_its_only_reaching_term_returns_more_than_three_files() {
    let (_root, task) = task(
      "`InvoiceService` should refuse a second payment.",
      &["server/src/billing/invoice.service.ts"],
      Some("guided"),
    );
    let audit = audit_task(&task, &tree(), Some("Use `orders.tsx` conventions."));
    assert_eq!(audit.prompt.reaching, vec![term("InvoiceService", 4, 1, 0)]);
    assert_eq!((audit.class, audit.disagrees()), (PromptLeak::Guided, false));
    assert_eq!(audit.instructions.unwrap().reaching, Vec::new());
  }

  #[test]
  fn should_call_a_prompt_open_when_no_term_reaches_a_file_to_change_and_flag_a_wrong_declaration() {
    let (_root, task) = task(
      "Customers are charged twice when they pay an order again.",
      &["server/src/billing/invoice.service.ts", "server/src/billing/refund.service.ts"],
      Some("guided"),
    );
    let audit = audit_task(&task, &tree(), None);
    assert_eq!((audit.class, audit.declared, audit.disagrees()), (PromptLeak::Open, Some(PromptLeak::Guided), true));
    assert_eq!(audit.reasons, Vec::new());
    assert_eq!(audit.must_change_absent, vec!["server/src/billing/refund.service.ts"]);
    assert_eq!(audit.prompt.reaching, Vec::new());
  }

  #[test]
  fn should_render_the_counts_the_reasons_the_reaching_terms_and_the_disagreements() {
    let (_root, handed) = task(
      "Paying twice fails with \"invoice already settled\" or with \"double charge refused\".",
      &["server/src/billing/invoice.service.ts"],
      Some("open"),
    );
    let report = LeakReport {
      handed_max_files: HANDED_MAX_FILES,
      tasks: vec![audit_task(&handed, &tree(), Some("Read `orders.tsx` first."))],
    };
    let markdown = render_leak_markdown(&report);
    assert!(markdown.contains("Tasks: 1. handed 1, guided 0, open 0.\n"), "{markdown}");
    assert!(markdown.contains("**Declared class differs from the audit:** p00-sample.\n"), "{markdown}");
    assert!(
      markdown.contains(
        "| p00-sample | handed | a term returns a file to change; a term returns a file that names one | open | 1 | 2 of 2: `invoice already settled` (1 of 1 files), `double charge refused` (0 of 1 files, 1 naming one) | 0 of 2 |\n"
      ),
      "{markdown}"
    );
  }
}
