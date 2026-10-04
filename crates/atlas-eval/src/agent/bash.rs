use serde::{Deserialize, Serialize};

const SEARCH_PROGRAMS: [&str; 4] = ["grep", "rg", "find", "ls"];
const READ_PROGRAMS: [&str; 4] = ["cat", "head", "tail", "sed"];
const SHELL_KEYWORDS: [&str; 6] = ["do", "then", "else", "time", "!", "exec"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashAnalysis {
  pub search: bool,
  pub read_files: Vec<String>,
  pub unparsed_reads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
  text: String,
  dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Segment {
  words: Vec<Word>,
  piped_input: bool,
}

pub fn analyze_bash(command: &str) -> BashAnalysis {
  let Some(segments) = split_segments(command) else {
    return BashAnalysis {
      search: starts_with_search(command),
      read_files: Vec::new(),
      unparsed_reads: u32::from(mentions_read_program(command)),
    };
  };
  let mut analysis = BashAnalysis {
    search: segments
      .iter()
      .find(|segment| program(segment) != Some("cd"))
      .and_then(program)
      .is_some_and(|name| SEARCH_PROGRAMS.contains(&name)),
    ..BashAnalysis::default()
  };
  let mut cwd: Option<String> = None;
  for segment in &segments {
    match program(segment) {
      Some("cd") => cwd = segment.words.get(1).filter(|word| !word.dynamic).map(|word| join(cwd.as_deref(), &word.text)),
      Some(name) => match read_targets(name, segment) {
        ReadTargets::NotARead => {}
        ReadTargets::Unparsed => analysis.unparsed_reads += 1,
        ReadTargets::Files(files) => analysis.read_files.extend(files.iter().map(|file| join(cwd.as_deref(), file))),
      },
      None => {}
    }
  }
  analysis
}

enum ReadTargets {
  NotARead,
  Unparsed,
  Files(Vec<String>),
}

fn read_targets(program: &str, segment: &Segment) -> ReadTargets {
  let start = segment
    .words
    .iter()
    .position(|word| !SHELL_KEYWORDS.contains(&word.text.as_str()))
    .map_or(segment.words.len(), |index| index + 1);
  let args = &segment.words[start..];
  let operands = match program {
    "cat" => Some(plain_operands(args, &[])),
    "head" | "tail" => Some(plain_operands(args, &["-n", "-c", "--lines", "--bytes"])),
    "sed" => sed_operands(args),
    _ => return ReadTargets::NotARead,
  };
  let Some(operands) = operands else {
    return ReadTargets::NotARead;
  };
  if operands.is_empty() {
    return if segment.piped_input { ReadTargets::NotARead } else { ReadTargets::Unparsed };
  }
  if operands.iter().any(|word| word.dynamic || word.text == "-") {
    return ReadTargets::Unparsed;
  }
  ReadTargets::Files(operands.into_iter().map(|word| word.text.clone()).collect())
}

fn plain_operands<'a>(args: &'a [Word], valued_flags: &[&str]) -> Vec<&'a Word> {
  let mut operands = Vec::new();
  let mut skip_next = false;
  for word in args {
    if skip_next {
      skip_next = false;
      continue;
    }
    let text = word.text.as_str();
    if valued_flags.contains(&text) {
      skip_next = true;
      continue;
    }
    if text.starts_with('-') && text != "-" || text.starts_with('+') {
      continue;
    }
    operands.push(word);
  }
  operands
}

fn sed_operands(args: &[Word]) -> Option<Vec<&Word>> {
  let short = |flag: &str| flag.starts_with('-') && !flag.starts_with("--");
  let flags: Vec<&str> = args.iter().map(|word| word.text.as_str()).filter(|text| text.starts_with('-')).collect();
  let quiet = flags.iter().any(|flag| matches!(*flag, "--quiet" | "--silent") || short(flag) && flag.contains('n'));
  let in_place = flags.iter().any(|flag| flag.starts_with("--in-place") || short(flag) && flag.contains('i'));
  if !quiet || in_place {
    return None;
  }
  let mut operands = Vec::new();
  let mut script_given = false;
  let mut skip_next = false;
  for word in args {
    if skip_next {
      skip_next = false;
      continue;
    }
    let text = word.text.as_str();
    if text == "-e" || text == "-f" {
      script_given = true;
      skip_next = true;
      continue;
    }
    if text.starts_with('-') {
      continue;
    }
    if !script_given {
      script_given = true;
      continue;
    }
    operands.push(word);
  }
  Some(operands)
}

fn program(segment: &Segment) -> Option<&str> {
  let word = segment.words.iter().find(|word| !SHELL_KEYWORDS.contains(&word.text.as_str()))?;
  if word.dynamic {
    return None;
  }
  Some(word.text.rsplit('/').next().unwrap_or(&word.text))
}

fn join(cwd: Option<&str>, path: &str) -> String {
  match cwd {
    Some(dir) if !path.starts_with('/') && !path.starts_with('~') => format!("{}/{path}", dir.trim_end_matches('/')),
    _ => path.to_owned(),
  }
}

fn starts_with_search(command: &str) -> bool {
  command.split_whitespace().next().is_some_and(|first| SEARCH_PROGRAMS.contains(&first))
}

fn mentions_read_program(command: &str) -> bool {
  command
    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
    .any(|word| READ_PROGRAMS.contains(&word))
}

fn split_segments(command: &str) -> Option<Vec<Segment>> {
  let mut segments = Vec::new();
  let mut current = Segment::default();
  let mut word = String::new();
  let mut word_dynamic = false;
  let mut in_word = false;
  let mut skip_redirect_target = false;
  let mut chars = command.chars().peekable();
  let finish_word = |current: &mut Segment, word: &mut String, dynamic: &mut bool, in_word: &mut bool, skip: &mut bool| {
    if !*in_word {
      return;
    }
    if *skip {
      *skip = false;
    } else {
      current.words.push(Word {
        text: std::mem::take(word),
        dynamic: *dynamic,
      });
    }
    word.clear();
    *dynamic = false;
    *in_word = false;
  };
  while let Some(c) = chars.next() {
    match c {
      '\'' => {
        in_word = true;
        loop {
          match chars.next()? {
            '\'' => break,
            other => word.push(other),
          }
        }
      }
      '"' => {
        in_word = true;
        loop {
          match chars.next()? {
            '"' => break,
            '\\' => word.push(chars.next()?),
            other => {
              word_dynamic |= other == '$' || other == '`';
              word.push(other);
            }
          }
        }
      }
      '\\' => {
        in_word = true;
        match chars.next()? {
          '\n' => {}
          other => word.push(other),
        }
      }
      ' ' | '\t' => finish_word(&mut current, &mut word, &mut word_dynamic, &mut in_word, &mut skip_redirect_target),
      '>' | '<' => {
        let digits_only = in_word && word.chars().all(|digit| digit.is_ascii_digit());
        if digits_only {
          word.clear();
          in_word = false;
        } else {
          finish_word(&mut current, &mut word, &mut word_dynamic, &mut in_word, &mut skip_redirect_target);
        }
        while chars.peek().is_some_and(|next| *next == '>' || *next == '&') {
          chars.next();
        }
        if chars.peek().is_some_and(char::is_ascii_digit) {
          chars.next();
        } else {
          skip_redirect_target = true;
        }
      }
      ';' | '\n' | '&' | '|' => {
        finish_word(&mut current, &mut word, &mut word_dynamic, &mut in_word, &mut skip_redirect_target);
        let piped = c == '|' && chars.peek() != Some(&'|');
        if matches!(chars.peek(), Some('&') | Some('|')) && (c == '&' || c == '|') {
          chars.next();
        }
        if !current.words.is_empty() {
          segments.push(std::mem::take(&mut current));
        }
        current.piped_input = piped;
      }
      '(' | ')' | '{' | '}' if !in_word => return None,
      other => {
        in_word = true;
        word_dynamic |= matches!(other, '$' | '`' | '*' | '?' | '[' | '~');
        word.push(other);
      }
    }
  }
  finish_word(&mut current, &mut word, &mut word_dynamic, &mut in_word, &mut skip_redirect_target);
  if !current.words.is_empty() {
    segments.push(current);
  }
  Some(segments)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn files(command: &str) -> Vec<String> {
    analyze_bash(command).read_files
  }

  #[test]
  fn should_extract_cat_targets() {
    assert_eq!(files("cat src/a.rs src/b.rs"), vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(files("cat -n 'src/with space.rs'"), vec!["src/with space.rs"]);
  }

  #[test]
  fn should_skip_head_and_tail_option_values() {
    assert_eq!(files("head -n 40 src/lib.rs"), vec!["src/lib.rs"]);
    assert_eq!(files("tail -20 log.txt"), vec!["log.txt"]);
    assert_eq!(files("tail -n +5 a.ts"), vec!["a.ts"]);
  }

  #[test]
  fn should_extract_sed_print_targets_but_not_edits() {
    assert_eq!(files("sed -n '10,40p' crates/x/src/lib.rs"), vec!["crates/x/src/lib.rs"]);
    assert_eq!(files("sed -n -e '1,5p' a.rs b.rs"), vec!["a.rs", "b.rs"]);
    assert_eq!(analyze_bash("sed -i 's/a/b/' a.rs"), BashAnalysis::default());
    assert_eq!(analyze_bash("sed 's/a/b/' a.rs"), BashAnalysis::default());
  }

  #[test]
  fn should_resolve_paths_against_a_leading_cd() {
    assert_eq!(files("cd crates/atlas-core && cat src/lib.rs"), vec!["crates/atlas-core/src/lib.rs"]);
    assert_eq!(files("cd sub && cat /abs/file.rs"), vec!["/abs/file.rs"]);
  }

  #[test]
  fn should_ignore_reads_from_a_pipe_and_redirect_targets() {
    assert_eq!(files("git show HEAD:a.rs | head -n 20"), Vec::<String>::new());
    assert_eq!(files("cat a.rs > out.txt 2>&1"), vec!["a.rs"]);
    assert_eq!(files("cat a.rs 2>/dev/null"), vec!["a.rs"]);
  }

  #[test]
  fn should_count_dynamic_targets_as_unparsed() {
    let analysis = analyze_bash("cat src/*.rs && head -n 5 $FILE");
    assert_eq!(analysis.read_files, Vec::<String>::new());
    assert_eq!(analysis.unparsed_reads, 2);
    assert_eq!(analyze_bash("for f in a b; do cat \"$f\"; done").unparsed_reads, 1);
    assert_eq!(analyze_bash("(cat a.rs)").unparsed_reads, 1);
  }

  #[test]
  fn should_classify_search_commands_by_their_first_program() {
    assert!(analyze_bash("grep -rn foo src").search);
    assert!(analyze_bash("rg foo").search);
    assert!(analyze_bash("cd src && find . -name '*.rs'").search);
    assert!(analyze_bash("ls -la").search);
    assert!(!analyze_bash("cargo test | grep FAILED").search);
  }

  #[test]
  fn should_split_on_every_shell_operator() {
    assert_eq!(files("cat a.rs; cat b.rs || cat c.rs & cat d.rs"), vec!["a.rs", "b.rs", "c.rs", "d.rs"]);
    assert_eq!(files("cat a.rs\ncat b.rs"), vec!["a.rs", "b.rs"]);
  }

  #[test]
  fn should_treat_unbalanced_quotes_as_unparsed() {
    let analysis = analyze_bash("cat 'a.rs");
    assert_eq!(analysis.read_files, Vec::<String>::new());
    assert_eq!(analysis.unparsed_reads, 1);
  }
}
