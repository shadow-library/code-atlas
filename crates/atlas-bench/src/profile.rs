use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{BenchError, BenchResult};
use crate::report::ProfileInfo;

const SAMPLE_BINARY: &str = "/usr/bin/sample";
const SAMPLE_SECONDS: &str = "900";
const TOP_OF_STACK_HEADER: &str = "Sort by top of stack";
const TOP_OF_STACK_LINES: usize = 40;
const TREE_LINES: usize = 120;
const MIN_SHARE_PERCENT: u64 = 3;
const FRAME_NAME_WIDTH: usize = 140;
const HEADER_LINES: usize = 16;

fn frame(line: &str) -> Option<(usize, u64, &str)> {
  let body = line.trim_start_matches([' ', '+', '!', ':', '|']);
  let (count, name) = body.split_once(' ')?;
  Some((line.len() - body.len(), count.parse().ok()?, name.trim()))
}

fn trimmed(name: &str) -> &str {
  let name = name.split("  (in ").next().unwrap_or(name);
  &name[..name.len().min(FRAME_NAME_WIDTH)]
}

pub fn main_thread_tree(raw: &str) -> String {
  let block: Vec<&str> = raw
    .lines()
    .skip_while(|line| !line.starts_with("Call graph"))
    .skip(1)
    .skip_while(|line| line.trim().is_empty())
    .enumerate()
    .take_while(|(index, line)| *index == 0 || frame(line).is_none_or(|(indent, _, name)| indent > 4 || !name.starts_with("Thread_")))
    .map(|(_, line)| line)
    .collect();
  let Some(total) = block.first().and_then(|line| frame(line)).map(|(_, count, _)| count) else {
    return String::new();
  };
  let floor = total * MIN_SHARE_PERCENT / 100;
  block
    .iter()
    .filter_map(|line| frame(line))
    .filter(|(_, count, _)| *count >= floor)
    .take(TREE_LINES)
    .map(|(indent, count, name)| format!("{}{count:>7} {:>5.1}%  {}", " ".repeat(indent / 2), count as f64 / total as f64 * 100.0, trimmed(name)))
    .collect::<Vec<_>>()
    .join("\n")
}

pub fn top_of_stack(raw: &str) -> String {
  raw
    .lines()
    .skip_while(|line| !line.starts_with(TOP_OF_STACK_HEADER))
    .take_while(|line| !line.starts_with("Binary Images"))
    .take(TOP_OF_STACK_LINES)
    .map(trimmed_summary)
    .collect::<Vec<_>>()
    .join("\n")
}

fn trimmed_summary(line: &str) -> String {
  match line.rsplit_once("  (in ") {
    Some((name, rest)) => format!("{}  (in {rest}", &name[..name.len().min(FRAME_NAME_WIDTH)]),
    None => line.to_owned(),
  }
}

pub fn summarise(raw: &str) -> String {
  let header: Vec<&str> = raw.lines().take_while(|line| !line.starts_with("Call graph")).take(HEADER_LINES).collect();
  format!(
    "{}\n\nMain thread call tree, frames holding at least {MIN_SHARE_PERCENT}% of its samples (count, share, symbol):\n{}\n\n{}\n",
    header.join("\n"),
    main_thread_tree(raw),
    top_of_stack(raw)
  )
}

pub fn sample_command(exe: &Path, args: &[String], out: &Path) -> BenchResult<ProfileInfo> {
  if !Path::new(SAMPLE_BINARY).exists() {
    return Ok(ProfileInfo {
      tool: None,
      path: None,
      note: "no sampling profiler is installed (samply, cargo flamegraph and /usr/bin/sample are all missing); the per-phase timings from the run ledger are the profile"
        .to_owned(),
    });
  }
  let mut child = Command::new(exe)
    .args(args)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|source| BenchError::io("spawn", exe, source))?;
  let raw = out.with_extension("raw.txt");
  let sampler = Command::new(SAMPLE_BINARY)
    .arg(child.id().to_string())
    .arg(SAMPLE_SECONDS)
    .arg("-mayDie")
    .arg("-file")
    .arg(&raw)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  child.wait().map_err(|source| BenchError::io("wait", exe, source))?;
  if !sampler.map(|status| status.success()).unwrap_or(false) || !raw.exists() {
    return Ok(ProfileInfo {
      tool: Some("sample".to_owned()),
      path: None,
      note: "`sample` could not attach to the worker; the per-phase timings from the run ledger are the profile".to_owned(),
    });
  }
  let text = fs::read_to_string(&raw).map_err(|source| BenchError::io("read", &raw, source))?;
  fs::write(out, summarise(&text)).map_err(|source| BenchError::io("write", out, source))?;
  let _ = fs::remove_file(&raw);
  Ok(ProfileInfo {
    tool: Some("macOS sample (stack sampling at 1 ms)".to_owned()),
    path: Some(out.display().to_string()),
    note: "top-of-stack summary of the cold index worker; the per-phase timings come from the run ledger".to_owned(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  const RAW: &str = "Analysis of sampling atlas-bench (pid 1) every 1 millisecond\nPath: /x\n----\n\nCall graph:\n    100 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 100 start  (in dyld) + 1  [0x1]\n    +   60 persist  (in atlas-bench) + 4  [0x2]\n    +   : 50 fsync  (in libsystem_kernel.dylib) + 8  [0x3]\n    +   : 2 tiny  (in atlas-bench) + 8  [0x3]\n    +   40 analyze  (in atlas-bench) + 4  [0x4]\n    100 Thread_2\n    + 100 other_thread_frame  (in atlas-bench) + 1  [0x5]\n\nTotal number in stack (recursive counted multiple, when >=5):\n  7 noise\n\nSort by top of stack, same collapsed (when >= 5):\n  fsync  (in libsystem_kernel.dylib)        50\n  malloc  (in libsystem_malloc.dylib)        10\n\nBinary Images:\n  0x1 lib\n";

  #[test]
  fn should_list_main_thread_frames_above_the_share_floor_with_indentation() {
    let tree = main_thread_tree(RAW);
    let lines: Vec<&str> = tree.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "      100 100.0%  Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)");
    assert!(lines[2].contains("60  60.0%  persist"));
    assert!(lines[3].starts_with("   ") && lines[3].contains("50  50.0%  fsync"));
    assert!(!tree.contains("tiny") && !tree.contains("other_thread_frame"));
  }

  #[test]
  fn should_include_header_tree_and_top_of_stack_in_the_summary() {
    let text = summarise(RAW);
    assert!(text.contains("Analysis of sampling atlas-bench"));
    assert!(text.contains("persist"));
    assert!(text.contains("malloc  (in libsystem_malloc.dylib)        10"));
    assert!(!text.contains("noise") && !text.contains("Binary Images"));
  }

  #[test]
  fn should_return_an_empty_tree_without_a_call_graph() {
    assert_eq!(main_thread_tree("nothing"), "");
  }
}
