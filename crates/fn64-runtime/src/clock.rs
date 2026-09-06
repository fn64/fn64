//! Typed boundaries between the N64 CPU master clock and libultra `OSTime`.
//!
//! The public libultra `osGetTime` manual defines one `OSTime` tick as one
//! CP0 Count tick. The VR4300 Count register advances once per two CPU master
//! cycles. Device deadlines remain in [`crate::Cycles`]; an `OSTime` value can
//! enter that domain only through the explicit duration conversion below.

use crate::Cycles;
use std::fmt;

/// A monotonic position on the emulated R4300 master-cycle timeline.
///
/// This is deliberately distinct from [`Cycles`], which is a duration. The
/// type exposes only instant-plus-duration and instant-minus-instant so a
/// deadline cannot be accidentally added to another deadline or interpreted
/// as host wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmulatedInstant(u64);

impl EmulatedInstant {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn checked_add(self, duration: Cycles) -> Option<Self> {
        match self.0.checked_add(duration.get()) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn checked_duration_since(self, earlier: Self) -> Option<Cycles> {
        match self.0.checked_sub(earlier.0) {
            Some(value) => Some(Cycles::new(value)),
            None => None,
        }
    }
}

impl fmt::Display for EmulatedInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A libultra `OSTime` value, in CP0 Count-rate ticks (46.875 MHz on N64).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OsTime(u64);

impl OsTime {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Derive the unadjusted OS clock from monotonic CPU master cycles.
    pub const fn from_master_cycles(cycles: Cycles) -> Self {
        Self(cycles.get() / 2)
    }

    /// Convert an `OSTime` duration to the master-cycle deadline domain.
    pub const fn checked_duration_as_master_cycles(self) -> Option<Cycles> {
        match self.0.checked_mul(2) {
            Some(value) => Some(Cycles::new(value)),
            None => None,
        }
    }

    pub const fn wrapping_add(self, ticks: u64) -> Self {
        Self(self.0.wrapping_add(ticks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_time_is_the_half_rate_master_clock() {
        assert_eq!(OsTime::from_master_cycles(Cycles::new(0)).get(), 0);
        assert_eq!(OsTime::from_master_cycles(Cycles::new(1)).get(), 0);
        assert_eq!(OsTime::from_master_cycles(Cycles::new(2)).get(), 1);
        assert_eq!(
            OsTime::new(0x1234)
                .checked_duration_as_master_cycles()
                .unwrap(),
            Cycles::new(0x2468)
        );
        assert!(OsTime::new(u64::MAX)
            .checked_duration_as_master_cycles()
            .is_none());
    }

    #[test]
    fn emulated_instants_accept_durations_and_reject_regression_or_overflow() {
        let start = EmulatedInstant::new(10);
        let end = start.checked_add(Cycles::new(7)).unwrap();
        assert_eq!(end.get(), 17);
        assert_eq!(end.checked_duration_since(start), Some(Cycles::new(7)));
        assert_eq!(start.checked_duration_since(end), None);
        assert_eq!(
            EmulatedInstant::new(u64::MAX).checked_add(Cycles::new(1)),
            None
        );
    }
}
