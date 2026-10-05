mod render;
mod text;

pub use text::emit;

pub trait Analyzer {
  fn analyze(&self) -> u32;

  fn describe(&self) -> u32 {
    self.analyze()
  }
}

#[cfg(test)]
mod tests {
  use super::Analyzer;

  struct Fake;

  impl Analyzer for Fake {
    fn analyze(&self) -> u32 {
      0
    }
  }

  fn run(analyzer: &dyn Analyzer) -> u32 {
    analyzer.analyze()
  }
}
