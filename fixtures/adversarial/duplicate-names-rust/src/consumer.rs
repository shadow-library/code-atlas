use crate::a::parse;
use crate::b;

pub fn split_fields(line: &str) -> usize {
  parse(line).len()
}

pub fn read_number(line: &str) -> Option<i64> {
  b::parse(line)
}

pub mod glob_explicit {
  use crate::a::*;
  use crate::b::parse;

  pub fn explicit_beats_glob(line: &str) -> Option<i64> {
    parse(line)
  }
}
