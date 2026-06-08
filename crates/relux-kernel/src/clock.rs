//! A deterministic logical clock.
//!
//! The kernel is intentionally local-only and reproducible: it must never read
//! the wall clock, so every timestamp in the demo loop comes from this monotonic
//! counter. Each [`Clock::tick`] advances one logical second and renders an
//! ISO-8601-shaped string anchored at a fixed base so two runs produce byte-for-byte
//! identical output.

/// A monotonic logical clock that renders deterministic ISO-8601-shaped timestamps.
#[derive(Debug, Default, Clone)]
pub struct Clock {
    secs: u64,
}

impl Clock {
    /// A clock starting at the fixed base instant.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a clock that has already advanced `secs` logical seconds.
    ///
    /// Used when restoring kernel state from a snapshot so timestamps keep
    /// advancing monotonically across process restarts instead of resetting to
    /// the base instant.
    pub fn from_secs(secs: u64) -> Self {
        Self { secs }
    }

    /// The number of logical seconds this clock has advanced so far.
    ///
    /// This is the value that must be persisted to resume the clock exactly.
    pub fn secs(&self) -> u64 {
        self.secs
    }

    /// Advance one logical second and return the new timestamp string.
    ///
    /// The base instant is `2026-06-08T00:00:00Z`; ticks roll seconds into
    /// minutes and minutes into hours so the loop can emit many events without
    /// collisions while staying fully deterministic.
    pub fn tick(&mut self) -> String {
        let t = self.secs;
        self.secs += 1;
        let secs = t % 60;
        let mins = (t / 60) % 60;
        let hours = (t / 3600) % 24;
        format!("2026-06-08T{hours:02}:{mins:02}:{secs:02}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_monotonic_and_deterministic() {
        let mut a = Clock::new();
        let mut b = Clock::new();
        let first_a = a.tick();
        let first_b = b.tick();
        assert_eq!(first_a, "2026-06-08T00:00:00Z");
        assert_eq!(
            first_a, first_b,
            "two fresh clocks must agree tick-for-tick"
        );
        assert_eq!(a.tick(), "2026-06-08T00:00:01Z");
    }

    #[test]
    fn ticks_roll_into_minutes() {
        let mut c = Clock::new();
        for _ in 0..60 {
            c.tick();
        }
        assert_eq!(c.tick(), "2026-06-08T00:01:00Z");
    }
}
