//! Audio/video sync arithmetic: where an audio cue sits on the emulated
//! cycle timeline, and how far apart two wall-clock instants are in signed
//! milliseconds.
//!
//! Both computations appeared twice each, inline and verbatim, in `main.rs`'s
//! `about_to_wait` -- once for the audio landmark report and once for the
//! audio/video pair report. Duplicated float arithmetic is exactly what
//! drifts, and neither copy was reachable by a test. Both are pure functions
//! of their inputs (no clock, no environment), so they move here.
//!
//! ## The sign convention is the load-bearing part
//!
//! A phase number without a stated sign convention cannot be acted on: it
//! does not say whether to delay audio or delay video. The convention these
//! functions implement, and the one the operator lines quote, is:
//!
//! > **positive means the second instant follows the first.**
//!
//! So `signed_delta_ms(from: audio, to: video)` is positive when video is
//! late relative to audio. The tests below pin both signs and the zero case,
//! because `Instant` subtraction panics on underflow -- the reason the
//! original code branched on the ordering rather than subtracting directly.

/// The N64's AI advances one sample every `(dacrate + 1)` VI-clock ticks, and
/// the cycle timeline is in CPU clocks; converting a guest frame offset to
/// cycles therefore scales by `CPU_CLOCK_HZ * (dacrate + 1) / vi_clock_hz`.
///
/// Returns `None` when the cue cannot be placed on the emulated timeline at
/// all: no recorded DMA start, no starting DACRATE, or -- crucially -- a DMA
/// that was **retimed after it started**. A retimed DMA's `start_dacrate` no
/// longer describes the rate that actually played it, so projecting from it
/// would produce a confidently wrong cycle rather than an absent one.
pub fn landmark_cycle(
    dma_started_at: Option<u64>,
    start_dacrate: Option<u32>,
    retimed_after_start: bool,
    guest_frame_offset: u64,
    cpu_clock_hz: u64,
    vi_clock_hz: u32,
) -> Option<f64> {
    match (dma_started_at, start_dacrate, retimed_after_start) {
        (Some(start), Some(dacrate), false) => Some(
            start as f64
                + guest_frame_offset as f64 * cpu_clock_hz as f64 * f64::from(dacrate + 1)
                    / f64::from(vi_clock_hz),
        ),
        _ => None,
    }
}

/// Signed milliseconds from `from` to `to`: **positive when `to` follows
/// `from`**.
///
/// Written as a branch on the ordering rather than a subtraction because
/// `Instant::duration_since` saturates (and the older `-` operator panicked)
/// on a negative interval; a phase measurement that silently clamps to zero
/// would report perfect sync for the one case worth reporting.
pub fn signed_delta_ms(from: std::time::Instant, to: std::time::Instant) -> f64 {
    if to >= from {
        to.duration_since(from).as_secs_f64() * 1_000.0
    } else {
        -from.duration_since(to).as_secs_f64() * 1_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// NTSC VI clock and the VR4300 CPU clock, so the worked example below is
    /// the real ratio rather than round numbers that could hide a swapped
    /// multiply and divide.
    const VI_CLOCK_HZ: u32 = 48_681_812;
    const CPU_CLOCK_HZ: u64 = 93_750_000;

    #[test]
    fn a_zero_frame_offset_is_exactly_the_dma_start_cycle() {
        assert_eq!(
            landmark_cycle(Some(1_234_567), Some(0), false, 0, CPU_CLOCK_HZ, VI_CLOCK_HZ),
            Some(1_234_567.0)
        );
    }

    /// The conversion, spelled out independently of the implementation: one
    /// guest frame at DACRATE d advances `CPU_CLOCK_HZ * (d + 1) / VI_CLOCK_HZ`
    /// cycles. A swapped multiply/divide or a missing `+ 1` fails here.
    #[test]
    fn one_guest_frame_advances_by_the_dacrate_scaled_cycle_count() {
        let dacrate = 1_562_u32;
        let got = landmark_cycle(Some(0), Some(dacrate), false, 1, CPU_CLOCK_HZ, VI_CLOCK_HZ)
            .expect("a started, un-retimed DMA places on the timeline");
        let expected =
            CPU_CLOCK_HZ as f64 * f64::from(dacrate + 1) / f64::from(VI_CLOCK_HZ);
        assert!(
            (got - expected).abs() < 1e-6,
            "got {got}, expected {expected}"
        );
        // Sanity on magnitude: ~1500 VI ticks per sample at ~48.68 MHz is
        // about 32 kHz, so one "frame" here is a single sample period --
        // roughly 3000 CPU cycles, not 3 and not 3 million.
        assert!((2_000.0..4_000.0).contains(&got), "implausible scale: {got}");
    }

    /// Offsets scale linearly, which is what makes the projection usable for
    /// a cue thousands of samples into a DMA.
    #[test]
    fn the_offset_term_is_linear_in_the_frame_count() {
        let at = |n| {
            landmark_cycle(Some(100), Some(1_562), false, n, CPU_CLOCK_HZ, VI_CLOCK_HZ).unwrap()
        };
        let one = at(1) - at(0);
        let thousand = at(1_000) - at(0);
        assert!(
            (thousand - one * 1_000.0).abs() < 1e-3,
            "1000 frames ({thousand}) is not 1000x one frame ({one})"
        );
    }

    /// A retimed DMA cannot be placed: its `start_dacrate` no longer
    /// describes the rate that played it. Absent beats confidently wrong.
    #[test]
    fn a_retimed_dma_has_no_cycle_even_with_both_inputs_present() {
        assert_eq!(
            landmark_cycle(Some(1), Some(1_562), true, 10, CPU_CLOCK_HZ, VI_CLOCK_HZ),
            None
        );
    }

    #[test]
    fn a_missing_start_or_dacrate_has_no_cycle() {
        assert_eq!(
            landmark_cycle(None, Some(1_562), false, 10, CPU_CLOCK_HZ, VI_CLOCK_HZ),
            None
        );
        assert_eq!(
            landmark_cycle(Some(1), None, false, 10, CPU_CLOCK_HZ, VI_CLOCK_HZ),
            None
        );
    }

    /// The sign convention, stated as the operator lines state it: positive
    /// means the second instant follows the first.
    #[test]
    fn a_later_instant_gives_a_positive_delta() {
        let from = Instant::now();
        let to = from + Duration::from_millis(20);
        let got = signed_delta_ms(from, to);
        assert!((got - 20.0).abs() < 1e-6, "{got}");
    }

    /// The case a saturating subtraction would silently report as 0.0 --
    /// perfect sync for the one measurement worth reporting.
    #[test]
    fn an_earlier_instant_gives_a_negative_delta_rather_than_clamping() {
        let from = Instant::now();
        let to = from - Duration::from_millis(20);
        let got = signed_delta_ms(from, to);
        assert!((got + 20.0).abs() < 1e-6, "{got}");
        assert!(got < 0.0, "a preceding instant must not read as zero: {got}");
    }

    #[test]
    fn identical_instants_are_exactly_zero() {
        let t = Instant::now();
        assert_eq!(signed_delta_ms(t, t), 0.0);
    }

    /// Antisymmetry: swapping the arguments flips the sign and nothing else.
    #[test]
    fn swapping_the_arguments_negates_the_delta() {
        let from = Instant::now();
        let to = from + Duration::from_micros(12_345);
        assert!((signed_delta_ms(from, to) + signed_delta_ms(to, from)).abs() < 1e-9);
    }
}
