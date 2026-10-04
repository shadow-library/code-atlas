use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

pub(crate) struct SingleFlight<T> {
  flights: Mutex<Flights<T>>,
  landed: Condvar,
}

struct Flights<T> {
  launched: u64,
  landed: Option<Landed<T>>,
}

struct Landed<T> {
  ticket: u64,
  value: T,
}

impl<T> Flights<T> {
  fn landed_ticket(&self) -> u64 {
    self.landed.as_ref().map_or(0, |landed| landed.ticket)
  }

  fn in_flight(&self) -> bool {
    self.launched > self.landed_ticket()
  }
}

impl<T> Default for SingleFlight<T> {
  fn default() -> Self {
    Self {
      flights: Mutex::new(Flights { launched: 0, landed: None }),
      landed: Condvar::new(),
    }
  }
}

impl<T: Clone> SingleFlight<T> {
  fn lock(&self) -> MutexGuard<'_, Flights<T>> {
    self.flights.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub fn launched(&self) -> u64 {
    self.lock().launched
  }

  // A flight already in the air may have scanned the tree before this caller's edits, so a caller only shares flights launched after it arrived.
  pub fn board(&self, fly: impl FnOnce() -> T) -> T {
    let mut flights = self.lock();
    let needed = flights.launched + 1;
    loop {
      if let Some(landed) = flights.landed.as_ref().filter(|landed| landed.ticket >= needed) {
        return landed.value.clone();
      }
      if !flights.in_flight() {
        break;
      }
      flights = self.landed.wait(flights).unwrap_or_else(PoisonError::into_inner);
    }
    flights.launched += 1;
    let mut pilot = Pilot {
      flight: Some(self),
      ticket: flights.launched,
    };
    drop(flights);
    let value = fly();
    pilot.land(value.clone());
    value
  }
}

struct Pilot<'a, T: Clone> {
  flight: Option<&'a SingleFlight<T>>,
  ticket: u64,
}

impl<T: Clone> Pilot<'_, T> {
  fn land(&mut self, value: T) {
    let Some(flight) = self.flight.take() else { return };
    flight.lock().landed = Some(Landed { ticket: self.ticket, value });
    flight.landed.notify_all();
  }
}

impl<T: Clone> Drop for Pilot<'_, T> {
  fn drop(&mut self) {
    let Some(flight) = self.flight.take() else { return };
    let mut flights = flight.lock();
    flights.launched = flights.landed_ticket();
    drop(flights);
    flight.landed.notify_all();
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicU64, Ordering};
  use std::sync::{Arc, Barrier, mpsc};
  use std::thread;
  use std::time::Duration;

  use super::*;

  #[test]
  fn should_fly_once_per_call_when_calls_do_not_overlap() {
    let flight = SingleFlight::default();
    let flown = AtomicU64::new(0);
    let values: Vec<u64> = (0..3).map(|_| flight.board(|| flown.fetch_add(1, Ordering::SeqCst) + 1)).collect();
    assert_eq!(values, [1, 2, 3]);
    assert_eq!(flight.launched(), 3);
  }

  #[test]
  fn should_make_callers_that_arrive_mid_flight_share_the_next_flight() {
    let flight = Arc::new(SingleFlight::default());
    let flown = Arc::new(AtomicU64::new(0));
    let (taken_off, take_off) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let first = {
      let (flight, flown) = (Arc::clone(&flight), Arc::clone(&flown));
      thread::spawn(move || {
        flight.board(|| {
          taken_off.send(()).unwrap();
          released.recv().unwrap();
          flown.fetch_add(1, Ordering::SeqCst) + 1
        })
      })
    };
    take_off.recv().unwrap();
    let boarded = Arc::new(Barrier::new(9));
    let late: Vec<_> = (0..8)
      .map(|_| {
        let (flight, flown, boarded) = (Arc::clone(&flight), Arc::clone(&flown), Arc::clone(&boarded));
        thread::spawn(move || {
          boarded.wait();
          flight.board(|| flown.fetch_add(1, Ordering::SeqCst) + 1)
        })
      })
      .collect();
    boarded.wait();
    thread::sleep(Duration::from_millis(100));
    release.send(()).unwrap();

    assert_eq!(first.join().unwrap(), 1);
    let late: Vec<u64> = late.into_iter().map(|caller| caller.join().unwrap()).collect();
    assert_eq!(late, [2; 8]);
    assert_eq!(flown.load(Ordering::SeqCst), 2);
    assert_eq!(flight.launched(), 2);
  }

  #[test]
  fn should_relaunch_for_waiting_callers_when_the_pilot_panics() {
    let flight = Arc::new(SingleFlight::default());
    let (taken_off, take_off) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let crashing = {
      let flight = Arc::clone(&flight);
      thread::spawn(move || {
        flight.board(|| -> u64 {
          taken_off.send(()).unwrap();
          released.recv().unwrap();
          panic!("injected pilot failure");
        })
      })
    };
    take_off.recv().unwrap();
    let waiting = {
      let flight = Arc::clone(&flight);
      thread::spawn(move || flight.board(|| 7))
    };
    thread::sleep(Duration::from_millis(50));
    release.send(()).unwrap();

    assert!(crashing.join().is_err());
    assert_eq!(waiting.join().unwrap(), 7);
    assert_eq!(flight.board(|| 8), 8);
    assert_eq!(flight.launched(), 2);
  }
}
