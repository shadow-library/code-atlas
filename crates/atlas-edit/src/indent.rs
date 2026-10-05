#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstLine {
  Bare,
  Indented,
}

pub(crate) fn line_indent(content: &str, offset: usize) -> &str {
  let line_start = content[..offset].rfind('\n').map_or(0, |newline| newline + 1);
  let line = &content[line_start..offset];
  let line = line.strip_prefix(crate::text::UTF8_BOM).unwrap_or(line);
  let width = line.len() - line.trim_start_matches([' ', '\t']).len();
  &line[..width]
}

pub fn reindent(text: &str, base: &str, first_line: FirstLine) -> String {
  let mut lines = text.split('\n');
  let first = lines.next().unwrap_or_default();
  let first = first.strip_prefix(base).unwrap_or(first);
  let rest: Vec<&str> = lines.collect();
  let keep = rest.iter().filter(|line| !line.trim().is_empty()).all(|line| line.starts_with(base));
  let mut output = String::with_capacity(text.len() + base.len() * (rest.len() + 1));
  if first_line == FirstLine::Indented && !first.trim().is_empty() {
    output.push_str(base);
  }
  output.push_str(first);
  for line in rest {
    output.push('\n');
    if !keep && !line.trim().is_empty() {
      output.push_str(base);
    }
    output.push_str(line);
  }
  output
}

pub(crate) fn clean_text(text: &str) -> String {
  let unix = text.replace("\r\n", "\n");
  let lines: Vec<&str> = unix.split('\n').collect();
  let first = lines.iter().position(|line| !line.trim().is_empty()).unwrap_or(lines.len());
  let last = lines.iter().rposition(|line| !line.trim().is_empty()).map_or(first, |index| index + 1);
  lines[first..last.max(first)].join("\n")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_reindent_a_flush_left_body_to_the_symbol_indent() {
    let text = "fn b(&self) -> u32 {\n  let x = 1;\n\n  x\n}";
    assert_eq!(reindent(text, "    ", FirstLine::Bare), "fn b(&self) -> u32 {\n      let x = 1;\n\n      x\n    }");
    assert_eq!(reindent(text, "    ", FirstLine::Indented), "    fn b(&self) -> u32 {\n      let x = 1;\n\n      x\n    }");
  }

  #[test]
  fn should_keep_following_lines_that_already_carry_the_base_indent() {
    let text = "    fn b(&self) {\n        work();\n    }";
    assert_eq!(reindent(text, "    ", FirstLine::Bare), "fn b(&self) {\n        work();\n    }");
  }

  #[test]
  fn should_indent_nested_methods_with_tabs() {
    let text = "find() {\n\treturn 1;\n}";
    assert_eq!(reindent(text, "\t\t", FirstLine::Indented), "\t\tfind() {\n\t\t\treturn 1;\n\t\t}");
  }

  #[test]
  fn should_leave_top_level_text_unchanged() {
    assert_eq!(reindent("fn a() {\n  x\n}", "", FirstLine::Indented), "fn a() {\n  x\n}");
  }

  #[test]
  fn should_read_the_indent_of_the_line_containing_an_offset() {
    let content = "impl A {\n    fn b() {}\n}\n";
    assert_eq!(line_indent(content, content.find("fn b").unwrap()), "    ");
    assert_eq!(line_indent("\u{feff}fn a() {}", 3), "");
  }

  #[test]
  fn should_trim_surrounding_blank_lines_and_normalize_line_endings() {
    assert_eq!(clean_text("\r\n\n  fn a() {  \r\n  x\r\n}\n \n"), "  fn a() {  \n  x\n}");
    assert_eq!(clean_text("\n \n"), "");
  }
}
