include!(concat!(env!("OUT_DIR"), "/constants.rs"));

pub fn capacity() -> usize {
  MAX_ITEMS * 2
}

pub fn label() -> &'static str {
  default_label()
}

pub fn handwritten() -> usize {
  capacity() + 1
}
