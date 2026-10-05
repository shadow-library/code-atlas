macro_rules! line {
  ($out:expr) => {
    $out.push('\n')
  };
}
pub(crate) use line;

pub fn emit(out: &mut String) -> u32 {
  line!(out);
  crate::render::render(out)
}
