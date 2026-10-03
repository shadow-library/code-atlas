use std::hash::{BuildHasherDefault, Hasher};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) fn fnv1a_extend(mut hash: u64, bytes: &[u8]) -> u64 {
  for &byte in bytes {
    hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
  }
  hash
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
  fnv1a_extend(FNV_OFFSET, bytes)
}

pub(crate) const fn fnv_offset() -> u64 {
  FNV_OFFSET
}

pub(crate) fn splitmix64(value: u64) -> u64 {
  let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
  z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
  z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
  z ^ (z >> 31)
}

#[derive(Default)]
pub(crate) struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
  fn finish(&self) -> u64 {
    self.0
  }

  fn write(&mut self, bytes: &[u8]) {
    self.0 = fnv1a_extend(self.0, bytes);
  }

  fn write_u64(&mut self, value: u64) {
    self.0 = value;
  }
}

pub(crate) type IdentityBuild = BuildHasherDefault<IdentityHasher>;
