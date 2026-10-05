use base::Analyzer;

pub fn run(analyzer: &dyn Analyzer) -> u32 {
  analyzer.analyze()
}

pub fn by_path(analyzer: Box<dyn Analyzer>) -> u32 {
  Analyzer::analyze(analyzer.as_ref())
}

#[cfg(test)]
mod tests {
  use base::Analyzer;

  fn unit(analyzer: &dyn Analyzer) -> u32 {
    analyzer.analyze()
  }
}
