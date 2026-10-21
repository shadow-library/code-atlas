use std::collections::{BTreeMap, BTreeSet};

use super::bash::{ReadTargets, Word, read_operands};
use super::tree_index::{Cwd, TreeIndex, protect_char, unprotect, unprotect_char};

const LEADING_KEYWORDS: [&str; 13] = ["do", "then", "else", "elif", "if", "while", "until", "time", "!", "exec", "{", "}", "fi"];
const LOOP_OPENERS: [&str; 3] = ["for", "while", "until"];
const LOOP_CLOSER: &str = "done";
const SEARCH_PROGRAMS: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "ag", "ack"];
const SEARCH_VALUE_FLAGS: [char; 11] = ['A', 'B', 'C', 'm', 'e', 'f', 'g', 't', 'T', 'j', 'M'];
const SEARCH_PATTERN_FLAGS: [char; 2] = ['e', 'f'];
const SEARCH_LISTING_FLAGS: [char; 4] = ['l', 'L', 'c', 'q'];
const SEARCH_LISTING_LONG_FLAGS: [&str; 5] = ["--files-with-matches", "--files-without-match", "--count", "--quiet", "--files"];
const SEARCH_LONG_VALUE_FLAGS: [&str; 16] = [
  "--include",
  "--exclude",
  "--exclude-dir",
  "--glob",
  "--iglob",
  "--type",
  "--type-not",
  "--max-count",
  "--max-depth",
  "--after-context",
  "--before-context",
  "--context",
  "--regexp",
  "--file",
  "--color",
  "--colors",
];
const REFUSED_PROGRAMS: [&str; 16] = [
  "grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "fd", "ls", "tree", "cat", "head", "tail", "less", "more", "awk",
];
const REFUSED_GIT_SUBCOMMANDS: [&str; 2] = ["grep", "ls-files"];
const GIT_VALUED_OPTIONS: [&str; 6] = ["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"];
const ENV_VALUED_OPTIONS: [&str; 6] = ["-u", "-C", "-S", "--unset", "--chdir", "--split-string"];
const NICE_VALUED_OPTIONS: [&str; 2] = ["-n", "--adjustment"];
const TIMEOUT_VALUED_OPTIONS: [&str; 4] = ["-k", "-s", "--kill-after", "--signal"];
const STDBUF_VALUED_OPTIONS: [&str; 6] = ["-i", "-o", "-e", "--input", "--output", "--error"];
const XARGS_VALUED_OPTIONS: [&str; 16] = [
  "-I",
  "-L",
  "-n",
  "-P",
  "-s",
  "-d",
  "-E",
  "-a",
  "--max-lines",
  "--max-args",
  "--max-procs",
  "--max-chars",
  "--delimiter",
  "--arg-file",
  "--replace",
  "--process-slot-var",
];
const SUDO_VALUED_OPTIONS: [&str; 18] = [
  "-u",
  "-g",
  "-C",
  "-D",
  "-h",
  "-p",
  "-r",
  "-t",
  "-U",
  "-T",
  "--user",
  "--group",
  "--close-from",
  "--chdir",
  "--host",
  "--prompt",
  "--role",
  "--other-user",
];
const MAX_SUBSTITUTION_NESTING: usize = 32;
const PYTHON_WRITES: [&str; 3] = [".write(", ".write_text(", ".writelines("];
const PYTHON_CHDIR: &str = "os.chdir(";
const HEREDOC_MARK: char = '\u{E100}';

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShellAccessKind {
  Search,
  Read,
  Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShellForm {
  Cat,
  Head,
  Tail,
  SedPrint,
  Search,
  SearchListing,
  SedInPlace,
  PerlInPlace,
  PythonScript,
  Redirect,
  Tee,
}

impl ShellForm {
  pub fn label(self) -> &'static str {
    match self {
      Self::Cat => "cat",
      Self::Head => "head",
      Self::Tail => "tail",
      Self::SedPrint => "sed -n",
      Self::Search => "grep",
      Self::SearchListing => "grep -l",
      Self::SedInPlace => "sed -i",
      Self::PerlInPlace => "perl -i",
      Self::PythonScript => "python script",
      Self::Redirect => "redirect",
      Self::Tee => "tee",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessOutcome {
  Applied,
  Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShellAccess {
  pub kind: ShellAccessKind,
  pub form: ShellForm,
  pub path: String,
  pub outcome: AccessOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellTrace {
  pub accesses: Vec<ShellAccess>,
  pub operands: BTreeSet<String>,
  pub working_dirs: Vec<String>,
  pub listed_dirs: Vec<String>,
  pub search_patterns: Vec<String>,
  pub failed_cds: u32,
  pub unverified_cds: u32,
  pub unparsed_reads: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Link {
  #[default]
  Sequence,
  And,
  Or,
  Pipe,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Command {
  words: Vec<String>,
  outputs: Vec<String>,
  heredoc: Option<String>,
  link: Link,
  errors: ErrorStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ErrorStream {
  #[default]
  Shown,
  Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
  Command(Command),
  Open,
  Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Arg {
  text: String,
  dynamic: bool,
  expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
  Ran,
  Failed,
}

pub fn trace_bash(command: &str, output: &str, cwd: &mut Cwd, tree: &mut TreeIndex) -> ShellTrace {
  let mut interpreter = Interpreter {
    tree,
    output,
    rejected_sed_scripts: failed_sed_fragments(output),
    cwd: cwd.clone(),
    previous: cwd.clone(),
    scopes: Vec::new(),
    variables: BTreeMap::new(),
    trace: ShellTrace::default(),
  };
  interpreter.note_working_dir();
  interpreter.run(&parse(command));
  *cwd = interpreter.cwd;
  interpreter.trace
}

struct Interpreter<'a> {
  tree: &'a mut TreeIndex,
  output: &'a str,
  rejected_sed_scripts: Vec<String>,
  cwd: Cwd,
  previous: Cwd,
  scopes: Vec<Cwd>,
  variables: BTreeMap<String, String>,
  trace: ShellTrace,
}

impl Interpreter<'_> {
  fn run(&mut self, items: &[Item]) {
    let mut index = 0;
    let mut skipping = false;
    while index < items.len() {
      match &items[index] {
        Item::Open => self.scopes.push(self.cwd.clone()),
        Item::Close => {
          if let Some(cwd) = self.scopes.pop() {
            self.enter(cwd);
          }
        }
        Item::Command(command) => {
          if matches!(command.link, Link::Sequence | Link::Or) {
            skipping = false;
          }
          if for_loop(command).is_some() {
            let end = loop_end(items, index);
            if !skipping {
              self.run_for(command, &items[(index + 1).min(end)..end]);
            }
            index = end;
          } else if !skipping {
            skipping = self.execute(command) == Outcome::Failed;
          }
        }
      }
      index += 1;
    }
  }

  fn run_for(&mut self, command: &Command, body: &[Item]) {
    let Some((variable, values)) = for_loop(command) else {
      return;
    };
    let values: Vec<Arg> = values.iter().flat_map(|word| self.expand(word)).collect();
    if values.is_empty() {
      self.run(body);
      return;
    }
    for value in values {
      self.variables.insert(variable.to_owned(), value.text);
      self.run(body);
    }
  }

  fn execute(&mut self, command: &Command) -> Outcome {
    let words: Vec<&String> = command.words.iter().skip_while(|word| LEADING_KEYWORDS.contains(&word.as_str())).collect();
    let assignments = words.iter().take_while(|word| assignment(word).is_some()).count();
    for word in &words[..assignments] {
      if let Some((name, value)) = assignment(word) {
        let value = self.substitute(value);
        self.variables.insert(name.to_owned(), unprotect(&value));
      }
    }
    let args: Vec<Arg> = words[assignments..].iter().flat_map(|word| self.expand(word)).collect();
    self.record_outputs(command);
    let Some((program, args)) = args.split_first() else {
      return Outcome::Ran;
    };
    if program.dynamic {
      return Outcome::Ran;
    }
    self.record_operands(args);
    let name = program.text.rsplit('/').next().unwrap_or(&program.text);
    let fed_input = command.link == Link::Pipe || command.heredoc.is_some();
    match name {
      "cd" => return self.change_dir(args, command.errors),
      "ls" => self.list(args),
      "mkdir" => self.for_each_path(args, |tree, path| tree.add_dir(path)),
      "touch" => self.for_each_path(args, |tree, path| tree.add_file(path)),
      "tee" => self.tee(args),
      "sed" => match sed_in_place_operands(args) {
        Some(operands) => return self.edit_in_place(&operands),
        None => self.read(name, args, fed_input),
      },
      "perl" => self.record(ShellAccessKind::Edit, ShellForm::PerlInPlace, &perl_in_place_operands(args)),
      "git" if args.first().is_some_and(|arg| arg.text == "grep") => self.search(&args[1..]),
      _ if SEARCH_PROGRAMS.contains(&name) => self.search(args),
      _ if name.starts_with("python") => self.python(args, command.heredoc.as_deref()),
      _ => self.read(name, args, fed_input),
    }
    Outcome::Ran
  }

  fn search(&mut self, args: &[Arg]) {
    let search = search_args(args);
    let form = if search.listing { ShellForm::SearchListing } else { ShellForm::Search };
    self.record(ShellAccessKind::Search, form, &search.paths);
    self.trace.search_patterns.extend(search.patterns);
  }

  fn edit_in_place(&mut self, operands: &[&Arg]) -> Outcome {
    let rejected = operands
      .iter()
      .any(|operand| self.rejected_sed_scripts.iter().any(|script| operand.text.starts_with(script)));
    let outcome = if rejected { AccessOutcome::Rejected } else { AccessOutcome::Applied };
    for operand in operands.iter().filter(|operand| !operand.dynamic) {
      self.push(ShellAccessKind::Edit, ShellForm::SedInPlace, &operand.text, outcome);
    }
    if rejected { Outcome::Failed } else { Outcome::Ran }
  }

  fn change_dir(&mut self, args: &[Arg], errors: ErrorStream) -> Outcome {
    let Some(target) = args.iter().find(|arg| !arg.text.starts_with('-') || arg.text == "-") else {
      self.enter(Cwd::Outside);
      return Outcome::Ran;
    };
    if target.text == "-" {
      self.enter(self.previous.clone());
      return Outcome::Ran;
    }
    if target.dynamic {
      self.trace.unverified_cds += 1;
      self.enter(Cwd::Outside);
      return Outcome::Ran;
    }
    match self.tree.resolve(&self.cwd, &target.text) {
      Some(dir) if self.tree.is_dir(&dir) => {
        self.enter(Cwd::InTree(dir));
        Outcome::Ran
      }
      Some(_) if cd_failure_reported(self.output, &target.text) => {
        self.trace.failed_cds += 1;
        Outcome::Failed
      }
      Some(_) if errors == ErrorStream::Hidden => {
        self.trace.unverified_cds += 1;
        Outcome::Failed
      }
      Some(dir) => {
        self.tree.add_dir(&dir);
        self.enter(Cwd::InTree(dir));
        Outcome::Ran
      }
      None => {
        self.enter(Cwd::Outside);
        Outcome::Ran
      }
    }
  }

  fn enter(&mut self, cwd: Cwd) {
    self.previous = std::mem::replace(&mut self.cwd, cwd);
    self.note_working_dir();
  }

  fn note_working_dir(&mut self) {
    if let Some(dir) = self.cwd.in_tree()
      && !self.trace.working_dirs.iter().any(|known| known == dir)
    {
      self.trace.working_dirs.push(dir.to_owned());
    }
  }

  fn list(&mut self, args: &[Arg]) {
    for arg in operands(args) {
      if let Some(dir) = self.tree.resolve(&self.cwd, &arg.text).filter(|dir| self.tree.is_dir(dir))
        && !self.trace.listed_dirs.contains(&dir)
      {
        self.trace.listed_dirs.push(dir);
      }
    }
  }

  fn tee(&mut self, args: &[Arg]) {
    self.for_each_path(args, |tree, path| tree.add_file(path));
    self.record(ShellAccessKind::Edit, ShellForm::Tee, &operands(args));
  }

  fn for_each_path(&mut self, args: &[Arg], mut apply: impl FnMut(&mut TreeIndex, &str)) {
    for arg in operands(args) {
      if let Some(path) = self.tree.resolve(&self.cwd, &arg.text).filter(|path| !path.is_empty()) {
        apply(self.tree, &path);
      }
    }
  }

  fn read(&mut self, program: &str, args: &[Arg], piped_input: bool) {
    let form = match program {
      "cat" => ShellForm::Cat,
      "head" => ShellForm::Head,
      "tail" => ShellForm::Tail,
      "sed" => ShellForm::SedPrint,
      _ => return,
    };
    let words: Vec<Word> = args
      .iter()
      .map(|arg| Word {
        text: arg.text.clone(),
        dynamic: arg.dynamic,
      })
      .collect();
    match read_operands(program, &words, piped_input) {
      ReadTargets::NotARead => {}
      ReadTargets::Unparsed => self.trace.unparsed_reads += 1,
      ReadTargets::Files(files) => {
        for file in files {
          self.push(ShellAccessKind::Read, form, &file, AccessOutcome::Applied);
        }
      }
    }
  }

  fn python(&mut self, args: &[Arg], heredoc: Option<&str>) {
    let inline = args
      .iter()
      .position(|arg| arg.text == "-c")
      .and_then(|index| args.get(index + 1))
      .map(|arg| arg.text.as_str());
    let Some(script) = inline.or(heredoc) else {
      return;
    };
    let writes = PYTHON_WRITES.iter().any(|form| script.contains(form));
    let literals = python_literals(script);
    let moved = script.split(PYTHON_CHDIR).nth(1).and_then(|rest| python_literals(rest).into_iter().next());
    let base = match moved.and_then(|dir| self.tree.resolve(&self.cwd, &dir)) {
      Some(dir) if self.tree.is_dir(&dir) => Cwd::InTree(dir),
      _ => self.cwd.clone(),
    };
    let shell_cwd = std::mem::replace(&mut self.cwd, base);
    for word in literals.iter().flat_map(|literal| literal.split_whitespace()) {
      let Some(path) = self.tree.resolve(&self.cwd, word).filter(|path| self.tree.is_file(path)) else {
        continue;
      };
      self.trace.operands.insert(path);
      if writes {
        self.push(ShellAccessKind::Edit, ShellForm::PythonScript, word, AccessOutcome::Applied);
      }
    }
    self.cwd = shell_cwd;
  }

  fn record_outputs(&mut self, command: &Command) {
    for output in &command.outputs {
      for target in self.expand(output) {
        if target.dynamic {
          continue;
        }
        if let Some(path) = self.tree.resolve(&self.cwd, &target.text).filter(|path| !path.is_empty() && !self.tree.is_dir(path)) {
          self.tree.add_file(&path);
          self.push(ShellAccessKind::Edit, ShellForm::Redirect, &target.text, AccessOutcome::Applied);
        }
      }
    }
  }

  fn record_operands(&mut self, args: &[Arg]) {
    for arg in args.iter().filter(|arg| !arg.dynamic && !arg.expanded) {
      if let Some(path) = self.tree.resolve(&self.cwd, &arg.text).filter(|path| self.tree.is_file(path)) {
        self.trace.operands.insert(path);
      }
    }
  }

  fn record(&mut self, kind: ShellAccessKind, form: ShellForm, targets: &[&Arg]) {
    for target in targets.iter().filter(|target| !target.dynamic) {
      self.push(kind, form, &target.text, AccessOutcome::Applied);
    }
  }

  fn push(&mut self, kind: ShellAccessKind, form: ShellForm, written: &str, outcome: AccessOutcome) {
    let Some(path) = self.tree.resolve(&self.cwd, written).filter(|path| self.tree.is_file(path)) else {
      return;
    };
    let access = ShellAccess { kind, form, path, outcome };
    if !self.trace.accesses.contains(&access) {
      self.trace.accesses.push(access);
    }
  }

  fn substitute(&self, word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
      if chars[index] != '$' {
        out.push(chars[index]);
        index += 1;
        continue;
      }
      let braced = chars.get(index + 1).is_some_and(|c| unprotect_char(*c) == '{');
      let start = index + 1 + usize::from(braced);
      let length = chars[start.min(chars.len())..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_').count();
      let name: String = chars[start.min(chars.len())..start.min(chars.len()) + length].iter().collect();
      let closed = !braced || chars.get(start + length) == Some(&'}');
      match self.variables.get(&name).filter(|_| length > 0 && closed) {
        Some(value) => {
          out.extend(value.chars().map(protect_char));
          index = start + length + usize::from(braced);
        }
        None => {
          out.push('$');
          index += 1;
        }
      }
    }
    out
  }

  fn expand(&self, word: &str) -> Vec<Arg> {
    let whole_variable = word.strip_prefix('$').map(unprotect);
    let whole_variable = whole_variable.as_deref().map(|name| name.trim_start_matches('{').trim_end_matches('}'));
    if let Some(value) = whole_variable.and_then(|name| self.variables.get(name))
      && value.split_whitespace().count() > 1
    {
      return value.split_whitespace().map(literal_arg).collect();
    }
    expand_braces(&self.substitute(word)).into_iter().flat_map(|text| self.expand_globs(text)).collect()
  }

  fn expand_globs(&self, text: String) -> Vec<Arg> {
    if text.contains(['$', '`']) {
      return vec![Arg {
        text: unprotect(&text),
        dynamic: true,
        expanded: false,
      }];
    }
    if !text.contains(['*', '?', '[']) {
      return vec![literal_arg(&unprotect(&text))];
    }
    let matches = self.tree.glob(&self.cwd, &text);
    if matches.is_empty() {
      return vec![Arg {
        text: unprotect(&text),
        dynamic: true,
        expanded: false,
      }];
    }
    matches
      .iter()
      .map(|path| Arg {
        text: self.tree.absolute(path),
        dynamic: false,
        expanded: true,
      })
      .collect()
  }
}

fn literal_arg(text: &str) -> Arg {
  Arg {
    text: text.to_owned(),
    dynamic: false,
    expanded: false,
  }
}

fn operands(args: &[Arg]) -> Vec<&Arg> {
  args.iter().filter(|arg| !arg.dynamic && !arg.text.starts_with('-')).collect()
}

fn assignment(word: &str) -> Option<(&str, &str)> {
  let (name, value) = word.split_once('=')?;
  let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
  valid.then_some((name, value))
}

fn for_loop(command: &Command) -> Option<(&str, &[String])> {
  let start = command.words.iter().position(|word| !LEADING_KEYWORDS.contains(&word.as_str()))?;
  let words = &command.words[start..];
  if words.first().map(String::as_str) != Some("for") {
    return None;
  }
  let variable = words.get(1)?;
  let values = if words.get(2).map(String::as_str) == Some("in") {
    &words[3..]
  } else {
    &words[words.len()..]
  };
  Some((variable, values))
}

fn opens_loop(command: &Command) -> bool {
  command
    .words
    .iter()
    .take_while(|word| LEADING_KEYWORDS.contains(&word.as_str()) || LOOP_OPENERS.contains(&word.as_str()))
    .any(|word| LOOP_OPENERS.contains(&word.as_str()))
}

fn loop_end(items: &[Item], start: usize) -> usize {
  let mut depth = 0;
  for (index, item) in items.iter().enumerate().skip(start + 1) {
    let Item::Command(command) = item else {
      continue;
    };
    if opens_loop(command) {
      depth += 1;
    } else if command.words.first().map(String::as_str) == Some(LOOP_CLOSER) {
      if depth == 0 {
        return index;
      }
      depth -= 1;
    }
  }
  items.len()
}

fn expand_braces(text: &str) -> Vec<String> {
  let chars: Vec<char> = text.chars().collect();
  let Some(open) = chars.iter().position(|c| *c == '{') else {
    return vec![text.to_owned()];
  };
  let mut depth = 0;
  let mut commas = Vec::new();
  let mut close = None;
  for (index, c) in chars.iter().enumerate().skip(open) {
    match c {
      '{' => depth += 1,
      '}' => {
        depth -= 1;
        if depth == 0 {
          close = Some(index);
          break;
        }
      }
      ',' if depth == 1 => commas.push(index),
      _ => {}
    }
  }
  let prefix: String = chars[..open].iter().collect();
  let (Some(close), false) = (close, commas.is_empty()) else {
    let rest: String = chars[open + 1..].iter().collect();
    return expand_braces(&rest).into_iter().map(|tail| format!("{prefix}{{{tail}")).collect();
  };
  let suffix: String = chars[close + 1..].iter().collect();
  let mut bounds = vec![open];
  bounds.extend(commas);
  bounds.push(close);
  bounds
    .windows(2)
    .flat_map(|pair| {
      let alternative: String = chars[pair[0] + 1..pair[1]].iter().collect();
      expand_braces(&format!("{prefix}{alternative}{suffix}"))
    })
    .collect()
}

fn short_cluster(text: &str) -> Option<&str> {
  text.strip_prefix('-').filter(|rest| !rest.is_empty() && !rest.starts_with('-'))
}

struct SearchArgs<'a> {
  paths: Vec<&'a Arg>,
  patterns: Vec<String>,
  listing: bool,
}

fn search_args(args: &[Arg]) -> SearchArgs<'_> {
  let mut search = SearchArgs {
    paths: Vec::new(),
    patterns: Vec::new(),
    listing: false,
  };
  let mut pattern_given = false;
  let mut value_of: Option<char> = None;
  for arg in args {
    let text = arg.text.as_str();
    if let Some(flag) = value_of.take() {
      if flag == 'e' {
        search.patterns.push(text.to_owned());
      }
      continue;
    }
    if !arg.expanded && text.starts_with("--") {
      value_of = (!text.contains('=') && SEARCH_LONG_VALUE_FLAGS.contains(&text)).then_some(if text == "--regexp" { 'e' } else { ' ' });
      pattern_given |= text.starts_with("--regexp") || text.starts_with("--file");
      search.listing |= SEARCH_LISTING_LONG_FLAGS.contains(&text);
      continue;
    }
    if let Some(cluster) = short_cluster(text).filter(|_| !arg.expanded) {
      let valued = cluster.find(SEARCH_VALUE_FLAGS);
      search.listing |= cluster[..valued.unwrap_or(cluster.len())].contains(SEARCH_LISTING_FLAGS);
      if let Some(index) = valued {
        let flag = cluster[index..].chars().next().unwrap_or_default();
        let joined = &cluster[index + flag.len_utf8()..];
        pattern_given |= SEARCH_PATTERN_FLAGS.contains(&flag);
        match (joined.is_empty(), flag) {
          (true, _) => value_of = Some(flag),
          (false, 'e') => search.patterns.push(joined.to_owned()),
          (false, _) => {}
        }
      }
      continue;
    }
    if !pattern_given {
      pattern_given = true;
      search.patterns.push(text.to_owned());
      continue;
    }
    if !arg.expanded {
      search.paths.push(arg);
    }
  }
  search
}

fn sed_in_place_operands(args: &[Arg]) -> Option<Vec<&Arg>> {
  let mut in_place = false;
  let mut script_given = false;
  let mut paths = Vec::new();
  let mut index = 0;
  while index < args.len() {
    let arg = &args[index];
    let text = arg.text.as_str();
    index += 1;
    if !arg.expanded && text.starts_with("--") {
      in_place |= text.starts_with("--in-place");
      script_given |= text.starts_with("--expression") || text.starts_with("--file");
      continue;
    }
    if let Some(cluster) = short_cluster(text).filter(|_| !arg.expanded) {
      if cluster.contains('i') {
        in_place = true;
        if cluster.ends_with('i') && args.get(index).is_some_and(|next| next.text.is_empty()) {
          index += 1;
        }
      } else if cluster.ends_with(['e', 'f']) {
        script_given = true;
        index += 1;
      }
      continue;
    }
    if !script_given {
      script_given = true;
      continue;
    }
    paths.push(arg);
  }
  in_place.then_some(paths)
}

fn perl_in_place_operands(args: &[Arg]) -> Vec<&Arg> {
  let mut in_place = false;
  let mut script_given = false;
  let mut paths = Vec::new();
  let mut skip_next = false;
  for arg in args {
    if skip_next {
      skip_next = false;
      continue;
    }
    match short_cluster(&arg.text).filter(|_| !arg.expanded && paths.is_empty()) {
      Some(cluster) => {
        let flags = cluster.split('e').next().unwrap_or(cluster);
        in_place |= flags.contains('i');
        if cluster.ends_with('e') {
          script_given = true;
          skip_next = true;
        }
      }
      None => paths.push(arg),
    }
  }
  if in_place && script_given { paths } else { Vec::new() }
}

fn python_literals(script: &str) -> Vec<String> {
  let chars: Vec<char> = script.chars().collect();
  let mut literals = Vec::new();
  let mut index = 0;
  while index < chars.len() {
    let quote = chars[index];
    if quote == '#' {
      index += chars[index..].iter().position(|c| *c == '\n').unwrap_or(chars.len() - index);
      continue;
    }
    if quote != '\'' && quote != '"' {
      index += 1;
      continue;
    }
    if chars.get(index + 1) == Some(&quote) && chars.get(index + 2) == Some(&quote) {
      let body_end = triple_quote_end(&chars, index + 3, quote);
      literals.push(chars[index + 3..body_end].iter().collect());
      index = body_end + 3;
      continue;
    }
    let mut literal = String::new();
    let mut end = index + 1;
    while end < chars.len() && chars[end] != quote && chars[end] != '\n' {
      if chars[end] == '\\' && end + 1 < chars.len() {
        end += 1;
      }
      literal.push(chars[end]);
      end += 1;
    }
    if chars.get(end) == Some(&quote) && !literal.is_empty() {
      literals.push(literal);
    }
    index = end + 1;
  }
  literals
}

fn triple_quote_end(chars: &[char], from: usize, quote: char) -> usize {
  let mut index = from;
  while index < chars.len() {
    if chars[index] == '\\' {
      index += 2;
      continue;
    }
    if chars[index] == quote && chars.get(index + 1) == Some(&quote) && chars.get(index + 2) == Some(&quote) {
      return index;
    }
    index += 1;
  }
  chars.len()
}

fn split_heredocs(command: &str) -> (String, Vec<String>) {
  let mut text = String::new();
  let mut bodies = Vec::new();
  let mut lines = command.split('\n');
  while let Some(line) = lines.next() {
    let (rewritten, delimiters) = heredoc_markers(line, bodies.len());
    text.push_str(&rewritten);
    text.push('\n');
    for delimiter in delimiters {
      let mut body = String::new();
      for body_line in lines.by_ref() {
        if body_line.trim() == delimiter {
          break;
        }
        body.push_str(body_line);
        body.push('\n');
      }
      bodies.push(body);
    }
  }
  (text, bodies)
}

fn heredoc_markers(line: &str, first_index: usize) -> (String, Vec<String>) {
  let chars: Vec<char> = line.chars().collect();
  let mut rewritten = String::new();
  let mut delimiters = Vec::new();
  let mut quote: Option<char> = None;
  let mut index = 0;
  while index < chars.len() {
    let c = chars[index];
    match quote {
      Some(open) => {
        if c == open {
          quote = None;
        }
      }
      None if c == '\'' || c == '"' => quote = Some(c),
      None if c == '<' && chars.get(index + 1) == Some(&'<') && chars.get(index + 2) != Some(&'<') => {
        if let Some((delimiter, end)) = heredoc_delimiter(&chars, index + 2) {
          rewritten.push_str(&format!(" {HEREDOC_MARK}{} ", first_index + delimiters.len()));
          delimiters.push(delimiter);
          index = end;
          continue;
        }
      }
      None => {}
    }
    rewritten.push(c);
    index += 1;
  }
  (rewritten, delimiters)
}

fn heredoc_delimiter(chars: &[char], from: usize) -> Option<(String, usize)> {
  let mut index = from;
  if chars.get(index) == Some(&'-') {
    index += 1;
  }
  while chars.get(index).is_some_and(|c| *c == ' ' || *c == '\t') {
    index += 1;
  }
  let quote = chars.get(index).copied().filter(|c| *c == '\'' || *c == '"');
  index += usize::from(quote.is_some());
  let length = chars[index.min(chars.len())..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_').count();
  if length == 0 {
    return None;
  }
  let delimiter: String = chars[index..index + length].iter().collect();
  index += length;
  if quote.is_some() && chars.get(index).copied() == quote {
    index += 1;
  }
  Some((delimiter, index))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redirect {
  Output,
  Ignored,
}

#[derive(Default)]
struct Parser {
  items: Vec<Item>,
  current: Command,
  word: String,
  in_word: bool,
  pending: Option<Redirect>,
  heredocs: Vec<String>,
}

impl Parser {
  fn finish_word(&mut self) {
    if !self.in_word {
      return;
    }
    self.in_word = false;
    let text = std::mem::take(&mut self.word);
    match self.pending.take() {
      Some(Redirect::Output) => self.current.outputs.push(text),
      Some(Redirect::Ignored) => {}
      None => match text.strip_prefix(HEREDOC_MARK).and_then(|index| index.parse::<usize>().ok()) {
        Some(index) => self.current.heredoc = self.heredocs.get(index).cloned(),
        None => self.current.words.push(text),
      },
    }
  }

  fn finish_command(&mut self, next: Link) {
    self.finish_word();
    self.pending = None;
    let finished = std::mem::take(&mut self.current);
    let carried = finished.link;
    let empty = finished.words.is_empty() && finished.outputs.is_empty() && finished.heredoc.is_none();
    if !empty {
      self.items.push(Item::Command(finished));
    }
    self.current.link = if empty && next == Link::Sequence { carried } else { next };
  }

  fn push(&mut self, c: char) {
    self.in_word = true;
    self.word.push(c);
  }
}

fn parse(command: &str) -> Vec<Item> {
  let (text, heredocs) = split_heredocs(command);
  let mut parser = Parser { heredocs, ..Parser::default() };
  let mut chars = text.chars().peekable();
  while let Some(c) = chars.next() {
    match c {
      '\'' => {
        parser.in_word = true;
        for quoted in chars.by_ref() {
          if quoted == '\'' {
            break;
          }
          parser.word.push(protect_char(quoted));
        }
      }
      '"' => {
        parser.in_word = true;
        while let Some(quoted) = chars.next() {
          match quoted {
            '"' => break,
            '\\' => parser.word.extend(chars.next().map(protect_char)),
            '$' | '`' => parser.word.push(quoted),
            other => parser.word.push(protect_char(other)),
          }
        }
      }
      '\\' => match chars.next() {
        Some('\n') | None => {}
        Some(escaped) => parser.push(protect_char(escaped)),
      },
      '$' if chars.peek() == Some(&'(') => {
        parser.push('$');
        skip_balanced(&mut chars);
      }
      '`' => {
        parser.push('`');
        for skipped in chars.by_ref() {
          if skipped == '`' {
            break;
          }
        }
      }
      ' ' | '\t' => parser.finish_word(),
      '#' if !parser.in_word => while chars.next_if(|next| *next != '\n').is_some() {},
      '>' => {
        let descriptor = parser.in_word && !parser.word.is_empty() && parser.word.chars().all(|digit| digit.is_ascii_digit());
        let standard_output = !descriptor || parser.word == "1";
        if descriptor {
          parser.word.clear();
          parser.in_word = false;
        } else {
          parser.finish_word();
        }
        chars.next_if_eq(&'>');
        let duplicated = chars.next_if_eq(&'&').is_some() && chars.next_if(|next| next.is_ascii_digit() || *next == '-').is_some();
        if !duplicated {
          parser.pending = Some(if standard_output { Redirect::Output } else { Redirect::Ignored });
        }
        if !duplicated && !standard_output {
          parser.current.errors = ErrorStream::Hidden;
        }
      }
      '<' => {
        parser.finish_word();
        while chars.next_if_eq(&'<').is_some() {}
        if chars.peek() == Some(&'(') {
          parser.push('$');
          skip_balanced(&mut chars);
        } else {
          parser.pending = Some(Redirect::Ignored);
        }
      }
      ';' | '\n' => parser.finish_command(Link::Sequence),
      '&' if chars.next_if_eq(&'&').is_some() => parser.finish_command(Link::And),
      '&' if chars.next_if_eq(&'>').is_some() => {
        parser.finish_word();
        chars.next_if_eq(&'>');
        parser.pending = Some(Redirect::Output);
        parser.current.errors = ErrorStream::Hidden;
      }
      '&' => parser.finish_command(Link::Sequence),
      '|' if chars.next_if_eq(&'|').is_some() => parser.finish_command(Link::Or),
      '|' => {
        chars.next_if_eq(&'&');
        parser.finish_command(Link::Pipe);
      }
      '(' if !parser.in_word => {
        let link = parser.current.link;
        parser.finish_command(link);
        parser.items.push(Item::Open);
      }
      ')' => {
        parser.finish_command(Link::Sequence);
        parser.items.push(Item::Close);
      }
      other => parser.push(other),
    }
  }
  parser.finish_command(Link::Sequence);
  parser.items
}

pub fn refused_program(command: &str) -> Option<String> {
  refused_in(command, 0)
}

fn refused_in(command: &str, nesting: usize) -> Option<String> {
  if nesting > MAX_SUBSTITUTION_NESTING {
    return None;
  }
  let (text, _) = split_heredocs(command);
  let substitutions = Substitutions::of(&text)?;
  let head = parse(command).iter().find_map(|item| match item {
    Item::Command(command) if command.link != Link::Pipe && command.heredoc.is_none() => refused_stage(&command.words),
    _ => None,
  });
  head.or_else(|| substitutions.iter().find_map(|body| refused_in(body, nesting + 1)))
}

fn refused_stage(words: &[String]) -> Option<String> {
  let mut rest = words;
  loop {
    let (first, tail) = rest.split_first()?;
    rest = match first.as_str() {
      "env" => skip_options(tail, &ENV_VALUED_OPTIONS),
      "time" => skip_options(tail, &[]),
      "nice" => skip_options(tail, &NICE_VALUED_OPTIONS),
      "timeout" => skip_options(tail, &TIMEOUT_VALUED_OPTIONS).get(1..)?,
      "nohup" => skip_options(tail, &[]),
      "stdbuf" => skip_options(tail, &STDBUF_VALUED_OPTIONS),
      "xargs" => skip_options(tail, &XARGS_VALUED_OPTIONS),
      "sudo" => skip_options(tail, &SUDO_VALUED_OPTIONS),
      "command" if prints_location(tail) => return None,
      "command" => skip_options(tail, &[]),
      word if LEADING_KEYWORDS.contains(&word) || assignment(word).is_some() => tail,
      _ => break,
    };
  }
  let (program, args) = rest.split_first()?;
  if program.contains(['$', '`']) {
    return None;
  }
  let program = unprotect(program);
  let name = program.rsplit('/').next().unwrap_or(&program);
  let args: Vec<Arg> = args.iter().map(|arg| literal_arg(&unprotect(arg))).collect();
  match name {
    _ if REFUSED_PROGRAMS.contains(&name) => Some(name.to_owned()),
    "sed" if sed_in_place_operands(&args).is_none() => Some(name.to_owned()),
    "git" => git_subcommand(&args)
      .filter(|subcommand| REFUSED_GIT_SUBCOMMANDS.contains(subcommand))
      .map(|subcommand| format!("git {subcommand}")),
    _ => None,
  }
}

fn prints_location(args: &[String]) -> bool {
  args
    .iter()
    .take_while(|arg| arg.starts_with('-') && arg.as_str() != "--")
    .any(|arg| arg.contains(['v', 'V']))
}

fn skip_options<'a>(args: &'a [String], valued: &[&str]) -> &'a [String] {
  let mut index = 0;
  while let Some(arg) = args.get(index) {
    if arg == "--" {
      return &args[index + 1..];
    }
    if !arg.starts_with('-') || arg == "-" {
      break;
    }
    index += if valued.contains(&arg.as_str()) { 2 } else { 1 };
  }
  &args[index.min(args.len())..]
}

fn git_subcommand(args: &[Arg]) -> Option<&str> {
  let mut index = 0;
  while let Some(arg) = args.get(index) {
    if !arg.text.starts_with('-') {
      return Some(arg.text.as_str());
    }
    index += if GIT_VALUED_OPTIONS.contains(&arg.text.as_str()) { 2 } else { 1 };
  }
  None
}

struct Substitutions {
  chars: Vec<char>,
  index: usize,
  bodies: Vec<String>,
}

impl Substitutions {
  fn of(text: &str) -> Option<Vec<String>> {
    let mut scanner = Self {
      chars: text.chars().collect(),
      index: 0,
      bodies: Vec::new(),
    };
    scanner.scan(0)?;
    Some(scanner.bodies)
  }

  fn next(&mut self) -> Option<char> {
    let c = self.chars.get(self.index).copied()?;
    self.index += 1;
    Some(c)
  }

  fn opens_substitution(&mut self) -> bool {
    let opens = self.chars.get(self.index) == Some(&'(');
    self.index += usize::from(opens);
    opens
  }

  fn scan(&mut self, nesting: usize) -> Option<()> {
    let mut parens = 0usize;
    let mut previous: Option<char> = None;
    while let Some(c) = self.next() {
      match c {
        '\\' => {
          self.next();
        }
        '\'' => self.single_quoted()?,
        '"' => self.double_quoted(nesting)?,
        '`' => self.backticks()?,
        '$' | '<' | '>' if self.opens_substitution() => self.substitution(nesting)?,
        '#' if previous.is_none_or(|before| before.is_whitespace() || ";&|()".contains(before)) => {
          while self.chars.get(self.index).is_some_and(|next| *next != '\n') {
            self.index += 1;
          }
        }
        '(' => parens += 1,
        ')' if nesting > 0 && parens == 0 => return Some(()),
        ')' => parens = parens.saturating_sub(1),
        _ => {}
      }
      previous = Some(c);
    }
    (nesting == 0).then_some(())
  }

  fn substitution(&mut self, nesting: usize) -> Option<()> {
    if nesting >= MAX_SUBSTITUTION_NESTING {
      return None;
    }
    let start = self.index;
    self.scan(nesting + 1)?;
    self.bodies.push(self.chars[start..self.index - 1].iter().collect());
    Some(())
  }

  fn single_quoted(&mut self) -> Option<()> {
    while self.next()? != '\'' {}
    Some(())
  }

  fn double_quoted(&mut self, nesting: usize) -> Option<()> {
    loop {
      match self.next()? {
        '"' => return Some(()),
        '\\' => {
          self.next()?;
        }
        '`' => self.backticks()?,
        '$' if self.opens_substitution() => self.substitution(nesting)?,
        _ => {}
      }
    }
  }

  fn backticks(&mut self) -> Option<()> {
    let mut body = String::new();
    loop {
      match self.next()? {
        '`' => break,
        '\\' => body.push(self.next()?),
        other => body.push(other),
      }
    }
    self.bodies.push(body);
    Some(())
  }
}

fn skip_balanced(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
  let mut depth = 0;
  for c in chars.by_ref() {
    match c {
      '(' => depth += 1,
      ')' => {
        depth -= 1;
        if depth == 0 {
          return;
        }
      }
      _ => {}
    }
  }
}

// zsh prints `cd:1: no such file or directory: <dir>`, bash `cd: <dir>: No such file or directory`.
fn cd_failure_reported(output: &str, target: &str) -> bool {
  [format!("no such file or directory: {target}"), format!("cd: {target}: No such file or directory")]
    .iter()
    .any(|message| output.lines().any(|line| line.trim_end().ends_with(message.as_str())))
}

// BSD sed given GNU `-i` syntax takes the first file as its script and reports `sed: 1: "<start of that file name> ...": <reason>`.
fn failed_sed_fragments(output: &str) -> Vec<String> {
  output
    .lines()
    .filter_map(|line| {
      let rest = line.trim_start().strip_prefix("sed: ")?;
      let (position, quoted) = rest.split_once(": \"")?;
      if !position.chars().all(|c| c.is_ascii_digit()) {
        return None;
      }
      let fragment = quoted.split('"').next().unwrap_or(quoted).trim_end_matches(" ...");
      (!fragment.is_empty()).then(|| fragment.to_owned())
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  const ROOT: &str = "/var/folders/x/ae-1/tree";

  fn tree() -> TreeIndex {
    TreeIndex::new(
      [ROOT.to_owned()],
      [
        "CLAUDE.md",
        "crates/a/src/lib.rs",
        "crates/a/src/view.rs",
        "crates/a/src/detectors/axum.rs",
        "crates/a/src/detectors/express.rs",
        "crates/a/tests/endpoints.rs",
        "crates/b/src/lib.rs",
      ]
      .map(str::to_owned),
    )
  }

  fn trace(command: &str) -> ShellTrace {
    trace_from(command, &mut Cwd::root())
  }

  fn trace_from(command: &str, cwd: &mut Cwd) -> ShellTrace {
    trace_bash(command, "", cwd, &mut tree())
  }

  fn accesses(trace: &ShellTrace) -> Vec<(ShellAccessKind, &'static str, &str)> {
    trace.accesses.iter().map(|access| (access.kind, access.form.label(), access.path.as_str())).collect()
  }

  #[test]
  fn should_name_the_refused_program_at_the_head_of_each_pipeline() {
    let cases = [
      ("grep -rn x src", Some("grep")),
      ("true; ls -la", Some("ls")),
      ("make || tail -n 5 log", Some("tail")),
      ("make\nhead -n 3 f", Some("head")),
      ("cargo build & awk '{print}' f", Some("awk")),
      ("cargo test |& grep FAILED", None),
      ("bun test 2>&1 | tail -n 20", None),
      ("'g'rep x f", Some("grep")),
      ("\\grep x f", Some("grep")),
      ("\"rg\" x", Some("rg")),
      ("env -u HOME LC_ALL=C grep x", Some("grep")),
      ("time -p nice -n 5 find .", Some("find")),
      ("command -p cat f", Some("cat")),
      ("command -v rg", None),
      ("timeout 30 grep -r x .", Some("grep")),
      ("timeout -k 5 -s KILL 10s rg x", Some("rg")),
      ("timeout --preserve-status 60 bun test", None),
      ("timeout 30", None),
      ("nohup cat log &", Some("cat")),
      ("nohup bun run dev &", None),
      ("stdbuf -oL -e 0 tail -n 5 log", Some("tail")),
      ("stdbuf -o L bun test", None),
      ("xargs -n 1 -I {} grep x {} < files.txt", Some("grep")),
      ("xargs -0 -r rm < list", None),
      ("xargs < list", None),
      ("git diff --name-only | xargs cat", None),
      ("sudo -u root find / -name x", Some("find")),
      ("sudo -E FOO=1 nice -n 5 ls", Some("ls")),
      ("sudo -E bun test", None),
      ("exec fd x", Some("fd")),
      ("if grep -q x f; then echo y; fi", Some("grep")),
      ("for f in a b; do cat \"$f\"; done", Some("cat")),
      ("{ tree; }", Some("tree")),
      ("git -C sub --no-pager ls-files", Some("git ls-files")),
      ("git log --oneline | grep fix", None),
      ("git status", None),
      ("sed --in-place=.bak s/a/b/ f", None),
      ("sed -i.bak s/a/b/ f", None),
      ("sed 's/-i//' f", Some("sed")),
      ("$PAGER f", None),
      ("x=$(rg -l y) && echo $x", Some("rg")),
      ("echo \"$(cat f)\"", Some("cat")),
      ("echo `ls src`", Some("ls")),
      ("(cd sub && egrep x f)", Some("egrep")),
      ("diff <(cat a) b", Some("cat")),
      ("echo $(echo $(more f))", Some("more")),
      ("echo '$(grep x f)'", None),
      ("bun test # then grep the output", None),
      ("cat > notes.md <<'EOF'\nfiles=$(ls src)\nEOF", None),
      ("python3 - <<EOF\nprint(1)\nEOF\nless f", Some("less")),
    ];
    for (command, refused) in cases {
      assert_eq!(refused_program(command).as_deref(), refused, "{command}");
    }
  }

  #[test]
  fn should_allow_substitutions_nested_beyond_the_cap_or_left_open() {
    let deep = format!("echo {}grep x{}", "$(".repeat(40), ")".repeat(40));
    assert_eq!(refused_program(&deep), None);
    let shallow = format!("echo {}grep x{}", "$(".repeat(3), ")".repeat(3));
    assert_eq!(refused_program(&shallow).as_deref(), Some("grep"));
    assert_eq!(refused_program("echo \"$(grep x f\""), None);
    assert_eq!(refused_program("echo `grep x f"), None);
  }

  #[test]
  fn should_map_print_commands_to_reads_relative_to_the_tracked_directory() {
    let mut cwd = Cwd::root();
    let first = trace_from("cd crates/a && sed -n 60,95p src/view.rs; head -n 12 src/lib.rs | grep use", &mut cwd);
    assert_eq!(
      accesses(&first),
      vec![
        (ShellAccessKind::Read, "sed -n", "crates/a/src/view.rs"),
        (ShellAccessKind::Read, "head", "crates/a/src/lib.rs")
      ]
    );
    assert_eq!(cwd, Cwd::InTree("crates/a".to_owned()));
    let second = trace_from("cd src/detectors && cat express.rs; cd ../../../.. && tail -5 CLAUDE.md", &mut cwd);
    assert_eq!(
      accesses(&second),
      vec![
        (ShellAccessKind::Read, "cat", "crates/a/src/detectors/express.rs"),
        (ShellAccessKind::Read, "tail", "CLAUDE.md")
      ]
    );
    assert_eq!(second.working_dirs, vec!["crates/a", "crates/a/src/detectors", ""]);
    assert_eq!(cwd, Cwd::root());
  }

  #[test]
  fn should_map_grep_with_file_operands_to_searches_and_leave_directories_to_the_output() {
    let targeted = trace("grep -n \"fn should\" -B2 -A 12 crates/a/tests/endpoints.rs | head -40");
    assert_eq!(accesses(&targeted), vec![(ShellAccessKind::Search, "grep", "crates/a/tests/endpoints.rs")]);
    let recursive = trace("grep -rn --include=*.rs mounted_on crates | grep -v express.rs; rg -e mounted crates/a/src -g '*.rs'");
    assert_eq!(accesses(&recursive), Vec::new());
    let globbed = trace("grep -n mounted crates/a/src/detectors/*.rs; head -3 crates/a/src/detectors/*.rs");
    assert_eq!(
      accesses(&globbed),
      vec![
        (ShellAccessKind::Read, "head", "crates/a/src/detectors/axum.rs"),
        (ShellAccessKind::Read, "head", "crates/a/src/detectors/express.rs")
      ]
    );
    assert_eq!(globbed.operands, BTreeSet::new());
    assert_eq!(
      accesses(&trace("git grep -n mounted -- crates/a/src/view.rs")),
      vec![(ShellAccessKind::Search, "grep", "crates/a/src/view.rs")]
    );
    let listing = trace("grep -rlE 'mount|route' crates; grep -c -e mounted_on crates/a/src/view.rs; rg -A3 --regexp prefix crates/a/src/lib.rs");
    assert_eq!(
      accesses(&listing),
      vec![
        (ShellAccessKind::Search, "grep -l", "crates/a/src/view.rs"),
        (ShellAccessKind::Search, "grep", "crates/a/src/lib.rs")
      ]
    );
    assert_eq!(listing.search_patterns, vec!["mount|route", "mounted_on", "prefix"]);
  }

  #[test]
  fn should_map_in_place_sed_and_perl_to_edits() {
    assert_eq!(
      accesses(&trace("sed -i '' 's/a/b/' crates/a/src/view.rs crates/a/src/lib.rs && cargo fmt")),
      vec![
        (ShellAccessKind::Edit, "sed -i", "crates/a/src/view.rs"),
        (ShellAccessKind::Edit, "sed -i", "crates/a/src/lib.rs")
      ]
    );
    assert_eq!(
      accesses(&trace("sed -i.bak -e 's/a/b/' crates/a/src/view.rs")),
      vec![(ShellAccessKind::Edit, "sed -i", "crates/a/src/view.rs")]
    );
    assert_eq!(
      accesses(&trace("sed -Ei 's/a/b/' crates/a/src/view.rs")),
      vec![(ShellAccessKind::Edit, "sed -i", "crates/a/src/view.rs")]
    );
    assert_eq!(accesses(&trace("sed 's/a/b/' crates/a/src/view.rs")), Vec::new());
    assert_eq!(
      accesses(&trace("perl -pi -e 's/a/b/' crates/a/src/view.rs")),
      vec![(ShellAccessKind::Edit, "perl -i", "crates/a/src/view.rs")]
    );
    assert_eq!(accesses(&trace("perl -ne 'print if /a/' crates/a/src/view.rs")), Vec::new());
  }

  #[test]
  fn should_expand_for_loops_variables_and_braces() {
    let looped = trace("cd crates/a/src/detectors && for f in axum express; do sed -i '' 's/a/b/' $f.rs; done\ncd ../../../.. && cargo fmt");
    assert_eq!(
      accesses(&looped),
      vec![
        (ShellAccessKind::Edit, "sed -i", "crates/a/src/detectors/axum.rs"),
        (ShellAccessKind::Edit, "sed -i", "crates/a/src/detectors/express.rs")
      ]
    );
    let assigned = trace("F=\"crates/a/src/view.rs crates/a/src/lib.rs\"; f=crates/b/src/lib.rs; sed -n 1,5p $F; cat \"${f}\"");
    assert_eq!(
      accesses(&assigned),
      vec![
        (ShellAccessKind::Read, "sed -n", "crates/a/src/view.rs"),
        (ShellAccessKind::Read, "sed -n", "crates/a/src/lib.rs"),
        (ShellAccessKind::Read, "cat", "crates/b/src/lib.rs")
      ]
    );
    assert_eq!(
      accesses(&trace("cat crates/{a,b}/src/lib.rs")),
      vec![(ShellAccessKind::Read, "cat", "crates/a/src/lib.rs"), (ShellAccessKind::Read, "cat", "crates/b/src/lib.rs")]
    );
    let dynamic = trace("cat \"$UNKNOWN\" crates/a/src/'*.rs'");
    assert_eq!(accesses(&dynamic), Vec::new());
    assert_eq!(dynamic.unparsed_reads, 1);
  }

  #[test]
  fn should_map_python_scripts_that_write_to_edits_of_the_files_they_name() {
    let script = "cd crates/a/src && python3 - <<'E'\np='view.rs'\ns=open(p).read()\ns=s.replace(\"a\", '''b\n\"lib.rs\"''')\nopen(p,'w').write(s)\nE\ncd ../../.. && cargo fmt";
    let mut cwd = Cwd::root();
    let written = trace_from(script, &mut cwd);
    assert_eq!(accesses(&written), vec![(ShellAccessKind::Edit, "python script", "crates/a/src/view.rs")]);
    assert_eq!(cwd, Cwd::root());
    let printed = trace("python3 -c \"print(open('crates/a/src/view.rs').read()[:200])\"");
    assert_eq!(accesses(&printed), Vec::new());
    assert_eq!(printed.operands, BTreeSet::from(["crates/a/src/view.rs".to_owned()]));
  }

  #[test]
  fn should_map_output_redirects_and_tee_to_edits_and_register_new_files() {
    let mut tree = tree();
    let mut cwd = Cwd::root();
    let appended = trace_bash(
      "cat >> crates/a/tests/endpoints.rs <<'E'\n#[test]\nfn should_x() {}\nE\ncargo test 2>&1 | tail -5 > /tmp/out.log",
      "",
      &mut cwd,
      &mut tree,
    );
    assert_eq!(accesses(&appended), vec![(ShellAccessKind::Edit, "redirect", "crates/a/tests/endpoints.rs")]);
    let created = trace_bash(
      "mkdir -p crates/c/src && echo x | tee crates/c/src/new.rs >/dev/null && cd crates/c/src && cat new.rs",
      "",
      &mut cwd,
      &mut tree,
    );
    assert_eq!(
      accesses(&created),
      vec![(ShellAccessKind::Edit, "tee", "crates/c/src/new.rs"), (ShellAccessKind::Read, "cat", "crates/c/src/new.rs")]
    );
    assert_eq!(cwd, Cwd::InTree("crates/c/src".to_owned()));
  }

  #[test]
  fn should_keep_the_directory_when_cd_targets_an_unknown_directory_and_skip_the_guarded_commands() {
    let mut cwd = Cwd::InTree("crates/a".to_owned());
    let hedged = trace_from("cd crates/a 2>/dev/null; cat src/lib.rs", &mut cwd);
    assert_eq!(accesses(&hedged), vec![(ShellAccessKind::Read, "cat", "crates/a/src/lib.rs")]);
    assert_eq!(hedged.unverified_cds, 1);
    assert_eq!(cwd, Cwd::InTree("crates/a".to_owned()));
    let guarded = trace_bash(
      "cd src/x && cat lib.rs | head; cat src/view.rs",
      "(eval):cd:1: no such file or directory: src/x\nfn view() {}",
      &mut cwd,
      &mut tree(),
    );
    assert_eq!(accesses(&guarded), vec![(ShellAccessKind::Read, "cat", "crates/a/src/view.rs")]);
    assert_eq!((guarded.failed_cds, guarded.unverified_cds), (1, 0));
    assert_eq!(cwd, Cwd::InTree("crates/a".to_owned()));
    let outside = trace_from("cd /tmp && cat src/lib.rs; cat /private/var/folders/x/ae-1/tree/CLAUDE.md", &mut cwd);
    assert_eq!(accesses(&outside), vec![(ShellAccessKind::Read, "cat", "CLAUDE.md")]);
    assert_eq!(cwd, Cwd::Outside);
  }

  #[test]
  fn should_enter_a_directory_the_tree_does_not_list_when_cd_reported_no_failure() {
    let mut cwd = Cwd::root();
    let built = trace_from("cd crates/a/dist && cat ../src/lib.rs index.js", &mut cwd);
    assert_eq!(accesses(&built), vec![(ShellAccessKind::Read, "cat", "crates/a/src/lib.rs")]);
    assert_eq!((built.failed_cds, built.unverified_cds), (0, 0));
    assert_eq!(cwd, Cwd::InTree("crates/a/dist".to_owned()));
  }

  #[test]
  fn should_restore_the_directory_after_a_subshell() {
    let mut cwd = Cwd::root();
    let scoped = trace_from("(cd crates/a && cat src/lib.rs); cat crates/b/src/lib.rs", &mut cwd);
    assert_eq!(
      accesses(&scoped),
      vec![(ShellAccessKind::Read, "cat", "crates/a/src/lib.rs"), (ShellAccessKind::Read, "cat", "crates/b/src/lib.rs")]
    );
    assert_eq!(cwd, Cwd::root());
  }

  #[test]
  fn should_collect_listed_directories_and_named_operands() {
    let listed = trace("cd crates/a && ls src src/detectors && wc -l src/*.rs && ./run.sh src/view.rs");
    assert_eq!(listed.listed_dirs, vec!["crates/a/src", "crates/a/src/detectors"]);
    assert_eq!(listed.operands, BTreeSet::from(["crates/a/src/view.rs".to_owned()]));
    assert_eq!(accesses(&listed), Vec::new());
  }

  #[test]
  fn should_extract_the_script_fragment_of_a_bsd_sed_failure() {
    let output = "sed: 1: \"crates/atlas-cli/src/co ...\": command c expects \\ followed by text\nsed: 1: \"clients.rs\n\": invalid command code c\nerror: other";
    assert_eq!(failed_sed_fragments(output), vec!["crates/atlas-cli/src/co", "clients.rs"]);
    assert_eq!(failed_sed_fragments("sed: can't read x: No such file"), Vec::<String>::new());
  }

  #[test]
  fn should_reject_every_operand_of_a_sed_command_sed_refused_and_stop_its_chain() {
    let command = "cd crates/a && sed -i 's/a/b/' src/view.rs src/lib.rs && cd ../.. ; sed -i '' 's/a/b/' src/detectors/axum.rs";
    let mut cwd = Cwd::root();
    let rejected = trace_bash(command, "sed: 1: \"src/view.rs\": invalid command code c", &mut cwd, &mut tree());
    let outcomes: Vec<(&str, AccessOutcome)> = rejected.accesses.iter().map(|access| (access.path.as_str(), access.outcome)).collect();
    assert_eq!(
      outcomes,
      vec![
        ("crates/a/src/view.rs", AccessOutcome::Rejected),
        ("crates/a/src/lib.rs", AccessOutcome::Rejected),
        ("crates/a/src/detectors/axum.rs", AccessOutcome::Applied)
      ]
    );
    assert_eq!(cwd, Cwd::InTree("crates/a".to_owned()));
  }
}
