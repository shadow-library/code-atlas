use base::Analyzer;

pub struct RustAnalyzer;

pub fn here() -> u32 {
  line!()
}

impl Analyzer for RustAnalyzer {
  fn analyze(&self) -> u32 {
    1
  }
}

#[cfg(test)]
mod tests {
  struct PluginFake;

  impl base::Analyzer for PluginFake {
    fn analyze(&self) -> u32 {
      2
    }
  }
}
