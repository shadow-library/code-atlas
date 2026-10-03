use atlas_core::{Certainty, Confidence};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cert {
  Syntactic,
  Resolved,
  Heuristic(f32, &'static str),
}

impl Cert {
  pub fn heuristic(confidence: f32, rule: &'static str) -> Self {
    Self::Heuristic(confidence, rule)
  }

  fn tier(self) -> u8 {
    match self {
      Self::Syntactic => 2,
      Self::Resolved => 1,
      Self::Heuristic(..) => 0,
    }
  }

  pub fn weaker(self, other: Cert) -> Cert {
    match (self, other) {
      (Self::Heuristic(left, _), Self::Heuristic(right, _)) if right < left => other,
      (Self::Heuristic(..), Self::Heuristic(..)) => self,
      _ if other.tier() < self.tier() => other,
      _ => self,
    }
  }

  pub fn capped(self, confidence: f32, rule: &'static str) -> Cert {
    self.weaker(Self::Heuristic(confidence, rule))
  }

  pub fn to_certainty(self) -> Option<Certainty> {
    match self {
      Self::Syntactic => Some(Certainty::Syntactic),
      Self::Resolved => Some(Certainty::Resolved),
      Self::Heuristic(confidence, rule) => Confidence::new(confidence).ok().map(|confidence| Certainty::heuristic(confidence, rule)),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_keep_the_weaker_of_two_certainties() {
    assert_eq!(Cert::Syntactic.weaker(Cert::Resolved), Cert::Resolved);
    assert_eq!(Cert::Resolved.weaker(Cert::Syntactic), Cert::Resolved);
    assert_eq!(Cert::Resolved.weaker(Cert::heuristic(0.9, "a")), Cert::heuristic(0.9, "a"));
    assert_eq!(Cert::heuristic(0.9, "a").weaker(Cert::heuristic(0.5, "b")), Cert::heuristic(0.5, "b"));
    assert_eq!(Cert::heuristic(0.5, "a").weaker(Cert::heuristic(0.9, "b")), Cert::heuristic(0.5, "a"));
  }

  #[test]
  fn should_refuse_to_convert_invalid_confidences() {
    assert_eq!(Cert::heuristic(1.5, "a").to_certainty(), None);
    assert_eq!(Cert::Syntactic.to_certainty(), Some(Certainty::Syntactic));
  }
}
