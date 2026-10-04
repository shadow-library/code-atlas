#[derive(Debug, Clone)]
pub struct Rng {
  state: u64,
}

impl Rng {
  pub fn new(seed: u64) -> Self {
    Self { state: seed }
  }

  pub fn derive(&self, salt: u64) -> Self {
    let mut mixed = Self::new(self.state ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    mixed.next_u64();
    mixed
  }

  pub fn next_u64(&mut self) -> u64 {
    self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = self.state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
  }

  pub fn below(&mut self, bound: usize) -> usize {
    if bound <= 1 {
      return 0;
    }
    (self.next_u64() % bound as u64) as usize
  }

  pub fn range(&mut self, low: usize, high_inclusive: usize) -> usize {
    low + self.below(high_inclusive - low + 1)
  }

  pub fn chance(&mut self, percent: usize) -> bool {
    self.below(100) < percent
  }

  pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
    &items[self.below(items.len())]
  }

  pub fn skewed(&mut self, bound: usize) -> usize {
    if bound <= 1 {
      return 0;
    }
    let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    ((unit * unit * unit * bound as f64) as usize).min(bound - 1)
  }

  pub fn constant(&mut self) -> u32 {
    self.range(100, 99_999) as u32
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_replay_the_same_sequence_for_the_same_seed() {
    let mut left = Rng::new(7);
    let mut right = Rng::new(7);
    let left: Vec<u64> = (0..5).map(|_| left.next_u64()).collect();
    let right: Vec<u64> = (0..5).map(|_| right.next_u64()).collect();
    assert_eq!(left, right);
  }

  #[test]
  fn should_keep_skewed_indices_in_bounds_and_biased_low() {
    let mut rng = Rng::new(1);
    let draws: Vec<usize> = (0..10_000).map(|_| rng.skewed(100)).collect();
    assert!(draws.iter().all(|index| *index < 100));
    let low = draws.iter().filter(|index| **index < 10).count();
    let high = draws.iter().filter(|index| **index >= 90).count();
    assert!(low > 4 * high, "low={low} high={high}");
  }

  #[test]
  fn should_derive_independent_streams_per_salt() {
    let base = Rng::new(42);
    assert_ne!(base.derive(1).next_u64(), base.derive(2).next_u64());
  }
}
