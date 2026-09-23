use glade_clock_api::{ClockPort, conformance};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// The clock a test composition shares: every clone reads one atomic. A
/// fixture, not a time source; it proves nothing about the OS clock.
#[derive(Clone)]
struct FakeClock {
    instant: Arc<AtomicI64>,
    /// Deliberately wrong: reads the instant in seconds.
    seconds: bool,
}

impl FakeClock {
    fn at(instant: i64, seconds: bool) -> FakeClock {
        FakeClock {
            instant: Arc::new(AtomicI64::new(instant)),
            seconds,
        }
    }
    fn set(&self, instant: i64) {
        self.instant.store(instant, Ordering::SeqCst);
    }
}

impl ClockPort for FakeClock {
    fn now_ms(&self) -> i64 {
        let instant = self.instant.load(Ordering::SeqCst);
        if self.seconds {
            return instant / 1_000;
        }
        instant
    }
}

/// Deliberately wrong: each clone keeps its own copy of the instant.
struct CopyingClock(AtomicI64);

impl Clone for CopyingClock {
    fn clone(&self) -> CopyingClock {
        CopyingClock(AtomicI64::new(self.0.load(Ordering::SeqCst)))
    }
}

impl ClockPort for CopyingClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn cl_001_every_handle_reads_one_instant() {
    let clock = FakeClock::at(0, false);
    let first: Arc<dyn ClockPort> = Arc::new(clock.clone());
    let second: Arc<dyn ClockPort> = Arc::new(clock.clone());
    conformance::one_instant(&*first, &*second, &|instant| clock.set(instant));
}

#[test]
fn cl_002_reads_epoch_milliseconds() {
    let clock = FakeClock::at(1_700_000_000_000, false);
    conformance::epoch_millis(&clock, &|| clock.instant.load(Ordering::SeqCst));
}

#[test]
#[should_panic(expected = "CL-001")]
fn rejects_a_per_handle_copy() {
    let clock = CopyingClock(AtomicI64::new(0));
    let second = clock.clone();
    conformance::one_instant(&clock, &second, &|instant| {
        clock.0.store(instant, Ordering::SeqCst)
    });
}

#[test]
#[should_panic(expected = "CL-002")]
fn rejects_a_clock_in_seconds() {
    let clock = FakeClock::at(1_700_000_000_000, true);
    conformance::epoch_millis(&clock, &|| clock.instant.load(Ordering::SeqCst));
}
