use std::sync::{Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
  Open,
  Writing,
  Closed,
}

pub(crate) struct WriteGate {
  phase: Mutex<Phase>,
}

pub(crate) struct Writing<'a> {
  gate: &'a WriteGate,
}

impl WriteGate {
  pub(crate) fn new() -> Self {
    Self { phase: Mutex::new(Phase::Open) }
  }

  fn phase(&self) -> MutexGuard<'_, Phase> {
    self.phase.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub(crate) fn enter(&self) -> Option<Writing<'_>> {
    let mut phase = self.phase();
    if *phase != Phase::Open {
      return None;
    }
    *phase = Phase::Writing;
    Some(Writing { gate: self })
  }

  pub(crate) fn close(&self) -> bool {
    let mut phase = self.phase();
    if *phase == Phase::Writing {
      return false;
    }
    *phase = Phase::Closed;
    true
  }
}

impl Drop for Writing<'_> {
  fn drop(&mut self) {
    *self.gate.phase() = Phase::Open;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_refuse_to_close_while_a_write_runs_and_refuse_writes_once_closed() {
    let gate = WriteGate::new();
    let writing = gate.enter().unwrap();
    assert!(!gate.close());
    drop(writing);
    assert!(gate.close());
    assert!(gate.enter().is_none());
  }
}
