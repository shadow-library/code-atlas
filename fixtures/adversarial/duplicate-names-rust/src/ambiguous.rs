use crate::a::*;
use crate::b::*;

pub fn ambiguous_call(line: &str) {
  let _ = parse(line);
}
