struct ItFake;

impl base::Analyzer for ItFake {
  fn analyze(&self) -> u32 {
    3
  }
}
