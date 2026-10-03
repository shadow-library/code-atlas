use std::sync::LazyLock;

use crate::hashing::{fnv_offset, fnv1a_extend, splitmix64};

pub const SHINGLE_SIZE: usize = 5;
pub const LANES: usize = 128;

const LANE_SEED_BASE: u64 = 0x5eed_c0de_a71a_5001;

pub(crate) type Signature = Box<[u32; LANES]>;

static LANE_SEEDS: LazyLock<[u64; LANES]> = LazyLock::new(|| {
  let mut seeds = [0u64; LANES];
  for (lane, seed) in seeds.iter_mut().enumerate() {
    *seed = splitmix64(LANE_SEED_BASE.wrapping_add((lane as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
  }
  seeds
});

pub(crate) fn shingles(tokens: &[u32]) -> Vec<u64> {
  let mut hashes: Vec<u64> = tokens
    .windows(SHINGLE_SIZE)
    .map(|window| {
      let hash = window.iter().fold(fnv_offset(), |hash, token| fnv1a_extend(hash, &token.to_le_bytes()));
      splitmix64(hash)
    })
    .collect();
  hashes.sort_unstable();
  hashes.dedup();
  hashes
}

pub(crate) fn signature(shingles: &[u64]) -> Signature {
  let mut lanes = Box::new([u32::MAX; LANES]);
  let seeds = &*LANE_SEEDS;
  for &shingle in shingles {
    for (lane, &seed) in lanes.iter_mut().zip(seeds.iter()) {
      let value = (splitmix64(shingle ^ seed) >> 32) as u32;
      if value < *lane {
        *lane = value;
      }
    }
  }
  lanes
}

pub(crate) fn exact_jaccard(left: &[u64], right: &[u64]) -> f64 {
  let (mut i, mut j, mut shared) = (0, 0, 0usize);
  while i < left.len() && j < right.len() {
    match left[i].cmp(&right[j]) {
      std::cmp::Ordering::Less => i += 1,
      std::cmp::Ordering::Greater => j += 1,
      std::cmp::Ordering::Equal => {
        shared += 1;
        i += 1;
        j += 1;
      }
    }
  }
  let union = left.len() + right.len() - shared;
  if union == 0 {
    return 0.0;
  }
  shared as f64 / union as f64
}

pub(crate) fn estimated_jaccard(left: &[u32; LANES], right: &[u32; LANES]) -> f64 {
  let matching = left.iter().zip(right.iter()).filter(|(a, b)| a == b).count();
  matching as f64 / LANES as f64
}

pub(crate) fn band_key(band: usize, rows: &[u32]) -> u64 {
  let seed = fnv1a_extend(fnv_offset(), &(band as u64).to_le_bytes());
  splitmix64(rows.iter().fold(seed, |hash, lane| fnv1a_extend(hash, &lane.to_le_bytes())))
}

pub fn collision_probability(jaccard: f64, bands: usize, rows: usize) -> f64 {
  1.0 - (1.0 - jaccard.powi(rows as i32)).powi(bands as i32)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tokens(len: u32, offset: u32) -> Vec<u32> {
    (0..len).map(|i| (i.wrapping_mul(2_654_435_761) ^ offset) % 1_000 + 16).collect()
  }

  #[test]
  fn should_produce_identical_signatures_across_calls() {
    let shingles = shingles(&tokens(120, 7));
    assert_eq!(signature(&shingles), signature(&shingles));
  }

  #[test]
  fn should_pin_the_signature_of_a_fixed_input() {
    let sig = signature(&shingles(&[16, 17, 18, 19, 20, 21, 22, 23]));
    assert_eq!(sig[0], 446_570_567);
    assert_eq!(sig[LANES - 1], 410_961_464);
  }

  #[test]
  fn should_return_no_shingles_below_the_shingle_size() {
    assert!(shingles(&[1, 2, 3, 4]).is_empty());
    assert_eq!(shingles(&[1, 2, 3, 4, 5]).len(), 1);
  }

  #[test]
  fn should_deduplicate_repeated_shingles() {
    assert_eq!(shingles(&[9; 12]).len(), 1);
  }

  #[test]
  fn should_compute_exact_jaccard() {
    assert_eq!(exact_jaccard(&[1, 2, 3, 4], &[3, 4, 5, 6]), 2.0 / 6.0);
    assert_eq!(exact_jaccard(&[1, 2], &[1, 2]), 1.0);
    assert_eq!(exact_jaccard(&[], &[]), 0.0);
  }

  #[test]
  fn should_estimate_jaccard_close_to_exact() {
    let a = tokens(400, 3);
    let mut b = a.clone();
    for i in (0..b.len()).step_by(20) {
      b[i] = 5_000 + i as u32;
    }
    let (sa, sb) = (shingles(&a), shingles(&b));
    let exact = exact_jaccard(&sa, &sb);
    let estimate = estimated_jaccard(&signature(&sa), &signature(&sb));
    assert!((exact - estimate).abs() < 0.12, "exact {exact} estimate {estimate}");
  }

  #[test]
  fn should_follow_the_lsh_collision_curve_for_32_bands_of_4_rows() {
    let p = |s: f64| collision_probability(s, 32, 4);
    assert!((p(0.2) - 0.0499).abs() < 1e-3, "{}", p(0.2));
    assert!((p(0.4) - 0.5639).abs() < 1e-3, "{}", p(0.4));
    assert!((p(0.5) - 0.8732).abs() < 1e-3, "{}", p(0.5));
    assert!((p(0.6) - 0.9882).abs() < 1e-3, "{}", p(0.6));
    assert!((p(0.8) - 1.0).abs() < 1e-6, "{}", p(0.8));
    assert!(p(0.6) > 0.98);
  }

  #[test]
  fn should_show_that_16_bands_of_8_rows_would_miss_the_target_threshold() {
    assert!(collision_probability(0.6, 16, 8) < 0.25);
  }
}
