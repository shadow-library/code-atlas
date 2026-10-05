use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

pub const PARSE_BUDGET_EXCEEDED: &str = "parse budget exceeded";

const BASE_MS: u64 = 2_000;
const MS_PER_STEP: u64 = 1_000;
const STEP_BYTES: u64 = 100 * 1024;
const CAP: Duration = Duration::from_secs(30);
const DEFAULT_STALL: Duration = Duration::from_secs(2);
const MAX_STALL_FACTOR: u32 = 4;
const TICKS_PER_CLOCK_READ: u32 = 64;
const WALL_BACKSTOP_FACTOR: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BudgetLimit {
  ScaledBySize,
  Fixed(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseBudget {
  limit: BudgetLimit,
  stall: Duration,
  factor: u32,
}

impl Default for ParseBudget {
  fn default() -> Self {
    Self {
      limit: BudgetLimit::ScaledBySize,
      stall: DEFAULT_STALL,
      factor: 1,
    }
  }
}

impl ParseBudget {
  pub fn fixed(limit: Duration) -> Self {
    Self {
      limit: BudgetLimit::Fixed(limit),
      ..Self::default()
    }
  }

  pub fn with_stall_limit(self, stall: Duration) -> Self {
    Self { stall, ..self }
  }

  pub fn escalated(self, factor: u32) -> Self {
    Self {
      factor: self.factor.saturating_mul(factor.max(1)),
      ..self
    }
  }

  pub fn factor(self) -> u32 {
    self.factor
  }

  pub fn base_limit_for(self, source_bytes: usize) -> Duration {
    match self.limit {
      BudgetLimit::Fixed(limit) => limit,
      BudgetLimit::ScaledBySize => {
        let scaled = u64::try_from(source_bytes).unwrap_or(u64::MAX).saturating_mul(MS_PER_STEP) / STEP_BYTES;
        Duration::from_millis(BASE_MS.saturating_add(scaled)).min(CAP)
      }
    }
  }

  pub fn limit_for(self, source_bytes: usize) -> Duration {
    let base = self.base_limit_for(source_bytes);
    base.saturating_mul(self.factor).min(base.max(CAP))
  }

  pub fn stall_limit_for(self, source_bytes: usize) -> Duration {
    self.stall.saturating_mul(self.factor.min(MAX_STALL_FACTOR)).min(self.limit_for(source_bytes))
  }

  pub fn failure_reason(self, source_bytes: usize) -> String {
    format!("{PARSE_BUDGET_EXCEEDED} ({} ms, {source_bytes} bytes)", self.base_limit_for(source_bytes).as_millis())
  }

  pub fn start(self, source_bytes: usize) -> Deadline {
    Deadline {
      started: Reading::now(),
      limit: self.limit_for(source_bytes),
      stall: self.stall_limit_for(source_bytes),
      overrun: Cell::new(None),
      ticks: Cell::new(0),
      _same_thread: PhantomData,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Overrun {
  Limit,
  Stall,
}

#[derive(Debug, Clone, Copy)]
struct Reading {
  wall: Instant,
  cpu: Option<Duration>,
}

impl Reading {
  fn now() -> Self {
    Self {
      wall: Instant::now(),
      cpu: thread_cpu_time(),
    }
  }

  fn charged_since(&self, earlier: &Reading) -> Duration {
    match (self.cpu, earlier.cpu) {
      (Some(now), Some(then)) => now.saturating_sub(then),
      _ => self.wall.saturating_duration_since(earlier.wall),
    }
  }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd", target_vendor = "apple"))]
fn thread_cpu_time() -> Option<Duration> {
  let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
  Some(Duration::new(u64::try_from(time.tv_sec).ok()?, u32::try_from(time.tv_nsec).ok()?))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd", target_vendor = "apple")))]
fn thread_cpu_time() -> Option<Duration> {
  None
}

// Budgets are charged in this thread's CPU time, so a Deadline must stay on the thread that started it.
pub struct Deadline {
  started: Reading,
  limit: Duration,
  stall: Duration,
  overrun: Cell<Option<Overrun>>,
  ticks: Cell<u32>,
  _same_thread: PhantomData<*const ()>,
}

impl Deadline {
  pub fn check(&self) -> ControlFlow<()> {
    self.check_at(&Reading::now())
  }

  pub fn tick(&self) -> ControlFlow<()> {
    let ticks = self.ticks.get().wrapping_add(1);
    self.ticks.set(ticks);
    if !ticks.is_multiple_of(TICKS_PER_CLOCK_READ) {
      return ControlFlow::Continue(());
    }
    self.check()
  }

  pub fn parse_progress(&self) -> ParseProgress<'_> {
    ParseProgress {
      deadline: self,
      furthest: 0,
      advanced_at: Reading::now(),
    }
  }

  pub fn overrun(&self) -> Option<Overrun> {
    self.overrun.get()
  }

  pub fn exceeded(&self) -> bool {
    self.overrun.get().is_some()
  }

  fn check_at(&self, now: &Reading) -> ControlFlow<()> {
    let charged = now.charged_since(&self.started);
    let wall = now.wall.saturating_duration_since(self.started.wall);
    if self.exceeded() || charged >= self.limit || wall >= self.limit.saturating_mul(WALL_BACKSTOP_FACTOR) {
      return self.exceed(Overrun::Limit);
    }
    ControlFlow::Continue(())
  }

  fn exceed(&self, overrun: Overrun) -> ControlFlow<()> {
    if self.overrun.get().is_none() {
      self.overrun.set(Some(overrun));
    }
    ControlFlow::Break(())
  }
}

pub struct ParseProgress<'d> {
  deadline: &'d Deadline,
  furthest: usize,
  advanced_at: Reading,
}

impl ParseProgress<'_> {
  pub fn observe(&mut self, byte_offset: usize) -> ControlFlow<()> {
    let now = Reading::now();
    if byte_offset > self.furthest {
      self.furthest = byte_offset;
      self.advanced_at = now;
    } else if now.charged_since(&self.advanced_at) >= self.deadline.stall {
      return self.deadline.exceed(Overrun::Stall);
    }
    self.deadline.check_at(&now)
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::{Overrun, ParseBudget};

  #[test]
  fn should_scale_the_default_budget_with_source_size_up_to_the_cap() {
    let limit = |bytes: usize| ParseBudget::default().limit_for(bytes).as_millis();
    assert_eq!(limit(0), 2_000);
    assert_eq!(limit(352), 2_003);
    assert_eq!(limit(100 * 1024), 3_000);
    assert_eq!(limit(2 * 1024 * 1024), 22_480);
    assert_eq!(limit(usize::MAX), 30_000);
    assert_eq!(ParseBudget::fixed(Duration::from_millis(7)).limit_for(usize::MAX), Duration::from_millis(7));
  }

  #[test]
  fn should_escalate_the_limit_to_the_cap_and_the_stall_limit_four_fold_at_most() {
    let budget = ParseBudget::default();
    assert_eq!(budget.escalated(2).limit_for(352), Duration::from_millis(4_006));
    assert_eq!(budget.escalated(16).limit_for(352), Duration::from_secs(30));
    assert_eq!(budget.escalated(2).stall_limit_for(352), Duration::from_secs(4));
    assert_eq!(budget.escalated(16).stall_limit_for(352), Duration::from_secs(8));
    assert_eq!(budget.escalated(16).failure_reason(352), "parse budget exceeded (2003 ms, 352 bytes)");
    assert_eq!(ParseBudget::fixed(Duration::from_secs(40)).escalated(2).limit_for(0), Duration::from_secs(40));
  }

  #[test]
  fn should_stop_a_parse_whose_furthest_offset_stops_advancing() {
    let deadline = ParseBudget::default().with_stall_limit(Duration::ZERO).start(10);
    let mut progress = deadline.parse_progress();
    assert!(progress.observe(5).is_continue());
    assert!(progress.observe(3).is_break());
    assert_eq!(deadline.overrun(), Some(Overrun::Stall));
    assert!(deadline.check().is_break());
    assert_eq!(deadline.overrun(), Some(Overrun::Stall));
  }

  #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd", target_vendor = "apple"))]
  #[test]
  fn should_charge_cpu_time_so_a_descheduled_thread_keeps_its_budget_until_the_wall_backstop() {
    let deadline = ParseBudget::fixed(Duration::from_millis(30)).start(10);
    std::thread::sleep(Duration::from_millis(60));
    assert!(deadline.check().is_continue());
    std::thread::sleep(Duration::from_millis(250));
    assert!(deadline.check().is_break());
    assert_eq!(deadline.overrun(), Some(Overrun::Limit));
  }

  #[test]
  fn should_stop_once_the_limit_passes() {
    let deadline = ParseBudget::fixed(Duration::ZERO).start(10);
    assert!(deadline.parse_progress().observe(1).is_break());
    assert_eq!(deadline.overrun(), Some(Overrun::Limit));
  }
}
