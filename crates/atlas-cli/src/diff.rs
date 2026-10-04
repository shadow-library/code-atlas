use std::path::Path;

const CONTEXT_LINES: usize = 2;
const MAX_TABLE_CELLS: usize = 4_000_000;

pub fn unified(path: &Path, before: Option<&str>, after: &str) -> String {
  let old: Vec<&str> = before.map(|text| text.lines().collect()).unwrap_or_default();
  let new: Vec<&str> = after.lines().collect();
  let prefix = old.iter().zip(&new).take_while(|(left, right)| left == right).count();
  let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(left, right)| left == right).count();
  let old_middle = &old[prefix..old.len() - suffix];
  let new_middle = &new[prefix..new.len() - suffix];
  let mut out = String::new();
  out.push_str(&match before {
    Some(_) => format!("--- {}\n", path.display()),
    None => "--- /dev/null\n".to_owned(),
  });
  out.push_str(&format!("+++ {}\n", path.display()));
  for line in &old[prefix.saturating_sub(CONTEXT_LINES)..prefix] {
    out.push_str(&format!("  {line}\n"));
  }
  match old_middle.len().saturating_mul(new_middle.len()) <= MAX_TABLE_CELLS {
    true => push_changes(&mut out, old_middle, new_middle),
    false => out.push_str(&format!("  ({} lines replaced by {} lines)\n", old_middle.len(), new_middle.len())),
  }
  let suffix_start = old.len() - suffix;
  for line in &old[suffix_start..(suffix_start + CONTEXT_LINES).min(old.len())] {
    out.push_str(&format!("  {line}\n"));
  }
  out
}

fn push_changes(out: &mut String, old: &[&str], new: &[&str]) {
  let width = new.len() + 1;
  let mut common = vec![0u32; (old.len() + 1) * width];
  for i in (0..old.len()).rev() {
    for j in (0..new.len()).rev() {
      common[i * width + j] = match old[i] == new[j] {
        true => common[(i + 1) * width + j + 1] + 1,
        false => common[(i + 1) * width + j].max(common[i * width + j + 1]),
      };
    }
  }
  let (mut i, mut j) = (0, 0);
  while i < old.len() && j < new.len() {
    if old[i] == new[j] {
      out.push_str(&format!("  {}\n", old[i]));
      i += 1;
      j += 1;
    } else if common[(i + 1) * width + j] >= common[i * width + j + 1] {
      out.push_str(&format!("- {}\n", old[i]));
      i += 1;
    } else {
      out.push_str(&format!("+ {}\n", new[j]));
      j += 1;
    }
  }
  for line in &old[i..] {
    out.push_str(&format!("- {line}\n"));
  }
  for line in &new[j..] {
    out.push_str(&format!("+ {line}\n"));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_show_only_the_changed_lines_with_context() {
    let before = "a\nb\nc\nd\ne\nf\n";
    let after = "a\nb\nc\nX\ne\nf\n";
    let text = unified(Path::new("f.txt"), Some(before), after);
    assert_eq!(text, "--- f.txt\n+++ f.txt\n  b\n  c\n- d\n+ X\n  e\n  f\n");
  }

  #[test]
  fn should_mark_a_new_file_as_all_additions() {
    let text = unified(Path::new("n.md"), None, "x\ny\n");
    assert_eq!(text, "--- /dev/null\n+++ n.md\n+ x\n+ y\n");
  }
}
