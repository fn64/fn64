//! Two pure decisions the present path makes every pump: whether a frame can
//! be cached at all, and what surface geometry the VI registers imply.
//!
//! Both were inline inside `Shell::probe_pump_present_dependency` and
//! `Shell::present`, tangled with `&mut self` cache bookkeeping and live
//! `fn64_abi` register reads. What moves here takes plain values and returns
//! plain values; the caller still does the cache recording and the register
//! reading.

use crate::framebuffer::UncacheablePresentReason;

/// The shell-state facts that decide cacheability before any VI register is
/// consulted. Kept separate from the address so the caller can skip the
/// register read entirely when one of these already settles the answer --
/// this runs once per pump, on the frame path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheabilityFacts {
    /// The settings overlay is up, so the presented image is not the game's
    /// alone.
    pub overlay_active: bool,
    /// The frame tripwire is armed and must hash every presented frame.
    pub frame_trip_armed: bool,
    /// Every presented frame is being written out as a PNG.
    pub frame_dump_armed: bool,
    /// A presenter exists to present into.
    pub presenter_available: bool,
}

/// The address half, read from the VI only when [`shell_state_reason`]
/// returned `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramebufferFacts {
    /// VI_ORIGIN (or the swap pointer) as an RDRAM offset, if either has been
    /// programmed at all.
    pub framebuffer_offset: Option<usize>,
    /// Length of the RDRAM backing store, to bound the offset.
    pub rdram_len: usize,
}

/// The first four rungs: reasons that depend only on shell state.
///
/// Returning `None` here is what licenses the caller to read the VI origin
/// and call [`framebuffer_reason`].
pub fn shell_state_reason(facts: CacheabilityFacts) -> Option<UncacheablePresentReason> {
    if facts.overlay_active {
        return Some(UncacheablePresentReason::Overlay);
    }
    if facts.frame_trip_armed {
        return Some(UncacheablePresentReason::FrameTrip);
    }
    if facts.frame_dump_armed {
        return Some(UncacheablePresentReason::FrameDump);
    }
    if !facts.presenter_available {
        return Some(UncacheablePresentReason::UnavailableFramebuffer);
    }
    None
}

/// The last three rungs: reasons that depend on the VI origin.
pub fn framebuffer_reason(facts: FramebufferFacts) -> Option<UncacheablePresentReason> {
    let Some(offset) = facts.framebuffer_offset else {
        return Some(UncacheablePresentReason::MissingFramebuffer);
    };
    if offset >= facts.rdram_len {
        return Some(UncacheablePresentReason::OutsideRdram);
    }
    if !offset.is_multiple_of(4) {
        return Some(UncacheablePresentReason::UnalignedFramebuffer);
    }
    None
}

/// The whole ladder, for tests and for any caller that already has both
/// halves. Production takes the two-step form so the register read stays
/// behind the shell-state checks.
///
/// Decide why this frame cannot be cached, or `None` if it can.
///
/// **The order is the contract.** Each reason answers a different question,
/// and a frame can trip several at once; the ladder reports the first one
/// that applies, from "the image is not purely the game's" down to "the
/// address is unusable":
///
/// 1. `Overlay` -- the composed image includes the settings panel.
/// 2. `FrameTrip` -- the tripwire must observe every frame, so a cache hit
///    that skipped a present would silently shorten the gate's sample.
/// 3. `FrameDump` -- same reasoning for the PNG-per-frame diagnostic.
/// 4. `UnavailableFramebuffer` -- no presenter to present into.
/// 5. `MissingFramebuffer` -- the VI has no programmed origin yet (boot).
/// 6. `OutsideRdram` -- the origin points past the backing store.
/// 7. `UnalignedFramebuffer` -- the RGBA5551 decode's `^ 2` halfword swap
///    assumes a word-aligned base, which every real VI framebuffer has.
///
/// The first three are deliberately ahead of the address checks: an armed
/// tripwire or dump must be reported as the reason even when the address
/// would also have been rejected, so a diagnostic run's cache line does not
/// read as an addressing fault.
pub fn uncacheable_reason(
    shell: CacheabilityFacts,
    framebuffer: FramebufferFacts,
) -> Option<UncacheablePresentReason> {
    shell_state_reason(shell).or_else(|| framebuffer_reason(framebuffer))
}

/// Largest surface dimension wgpu is asked for. The real cap is device
/// dependent and larger; this is a defensive clamp so a nonsense VI register
/// cannot request an allocation that fails.
pub const MAX_SURFACE_DIMENSION: usize = 4096;

/// Presented width from the framebuffer line stride and the operator's
/// overscan setting.
///
/// Overscan is a display **policy**, not a geometry-derived width: those
/// columns are genuinely scanned by the VI, they just hold stale RDRAM a real
/// TV would hide. `overscan = 0` presents the raw full scanout.
///
/// Never crops below one column, whatever the overscan setting -- a zero-width
/// surface is not presentable, and a stride of 1 with an overscan of 4 must
/// still yield 1.
pub fn presented_width(src_stride: usize, overscan: usize) -> usize {
    let overscan = overscan.min(src_stride.saturating_sub(1));
    (src_stride - overscan).clamp(1, MAX_SURFACE_DIMENSION)
}

/// Presented height from the VI's active output line count, clamped into a
/// presentable range.
///
/// Rows past the guest's own active rectangle were never rendered into, so
/// presenting a fixed 240 shows stale RDRAM along the bottom -- WM2000
/// programs 237.
pub fn presented_height(vi_output_height: usize) -> usize {
    vi_output_height.clamp(1, MAX_SURFACE_DIMENSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with nothing wrong with it is cacheable. Without this the
    /// ladder could return a reason for everything and every test below would
    /// still pass.
    fn clean_shell() -> CacheabilityFacts {
        CacheabilityFacts {
            overlay_active: false,
            frame_trip_armed: false,
            frame_dump_armed: false,
            presenter_available: true,
        }
    }

    fn clean_fb() -> FramebufferFacts {
        FramebufferFacts {
            framebuffer_offset: Some(0x100),
            rdram_len: 0x80_0000,
        }
    }

    #[test]
    fn an_ordinary_frame_is_cacheable() {
        assert_eq!(uncacheable_reason(clean_shell(), clean_fb()), None);
        assert_eq!(shell_state_reason(clean_shell()), None);
        assert_eq!(framebuffer_reason(clean_fb()), None);
    }

    #[test]
    fn each_shell_state_condition_alone_yields_its_own_reason() {
        let cases = [
            (
                CacheabilityFacts { overlay_active: true, ..clean_shell() },
                UncacheablePresentReason::Overlay,
            ),
            (
                CacheabilityFacts { frame_trip_armed: true, ..clean_shell() },
                UncacheablePresentReason::FrameTrip,
            ),
            (
                CacheabilityFacts { frame_dump_armed: true, ..clean_shell() },
                UncacheablePresentReason::FrameDump,
            ),
            (
                CacheabilityFacts { presenter_available: false, ..clean_shell() },
                UncacheablePresentReason::UnavailableFramebuffer,
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(shell_state_reason(facts), Some(expected), "{facts:?}");
        }
    }

    #[test]
    fn each_framebuffer_condition_alone_yields_its_own_reason() {
        let cases = [
            (
                FramebufferFacts { framebuffer_offset: None, ..clean_fb() },
                UncacheablePresentReason::MissingFramebuffer,
            ),
            (
                FramebufferFacts { framebuffer_offset: Some(0x80_0000), ..clean_fb() },
                UncacheablePresentReason::OutsideRdram,
            ),
            (
                FramebufferFacts { framebuffer_offset: Some(0x102), ..clean_fb() },
                UncacheablePresentReason::UnalignedFramebuffer,
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(framebuffer_reason(facts), Some(expected), "{facts:?}");
        }
    }

    /// The laziness the production call site depends on: whenever a
    /// shell-state reason applies, the answer is settled WITHOUT the
    /// framebuffer half, so the caller may skip the VI register read.
    #[test]
    fn a_shell_state_reason_settles_the_answer_without_the_address() {
        for shell in [
            CacheabilityFacts { overlay_active: true, ..clean_shell() },
            CacheabilityFacts { frame_trip_armed: true, ..clean_shell() },
            CacheabilityFacts { frame_dump_armed: true, ..clean_shell() },
            CacheabilityFacts { presenter_available: false, ..clean_shell() },
        ] {
            let reason = shell_state_reason(shell).expect("this state is uncacheable");
            // Same verdict whatever the address turns out to be.
            for fb in [
                clean_fb(),
                FramebufferFacts { framebuffer_offset: None, ..clean_fb() },
                FramebufferFacts { framebuffer_offset: Some(0x9000_0001), ..clean_fb() },
            ] {
                assert_eq!(uncacheable_reason(shell, fb), Some(reason));
            }
        }
    }

    /// The precedence, asserted where it actually matters: a diagnostic that
    /// is armed must be named as the reason even when the address is ALSO
    /// unusable, so a tripwire run's cache line does not read as an
    /// addressing fault.
    #[test]
    fn an_armed_diagnostic_outranks_a_broken_address() {
        let broken = FramebufferFacts {
            framebuffer_offset: Some(0x9000_0001),
            ..clean_fb()
        };
        assert_eq!(
            uncacheable_reason(
                CacheabilityFacts { frame_trip_armed: true, ..clean_shell() },
                broken
            ),
            Some(UncacheablePresentReason::FrameTrip)
        );
        assert_eq!(
            uncacheable_reason(
                CacheabilityFacts { frame_dump_armed: true, ..clean_shell() },
                broken
            ),
            Some(UncacheablePresentReason::FrameDump)
        );
        // ... and the overlay outranks both of those in turn.
        assert_eq!(
            uncacheable_reason(
                CacheabilityFacts {
                    overlay_active: true,
                    frame_trip_armed: true,
                    frame_dump_armed: true,
                    ..clean_shell()
                },
                broken
            ),
            Some(UncacheablePresentReason::Overlay)
        );
    }

    /// An out-of-range origin is reported as out of range, not as unaligned,
    /// even when it is both -- the offset bound is checked first.
    #[test]
    fn an_out_of_range_origin_outranks_its_own_misalignment() {
        assert_eq!(
            framebuffer_reason(FramebufferFacts {
                framebuffer_offset: Some(0x80_0001),
                ..clean_fb()
            }),
            Some(UncacheablePresentReason::OutsideRdram)
        );
    }

    /// The last word of RDRAM is inside it; one past the end is not.
    #[test]
    fn the_rdram_bound_is_exclusive() {
        let at = |offset| {
            framebuffer_reason(FramebufferFacts {
                framebuffer_offset: Some(offset),
                rdram_len: 0x1000,
            })
        };
        assert_eq!(at(0x0ffc), None);
        assert_eq!(at(0x1000), Some(UncacheablePresentReason::OutsideRdram));
    }

    #[test]
    fn zero_overscan_presents_the_full_stride() {
        assert_eq!(presented_width(320, 0), 320);
        assert_eq!(presented_width(640, 0), 640);
    }

    #[test]
    fn overscan_crops_that_many_columns() {
        assert_eq!(presented_width(320, 1), 319);
        assert_eq!(presented_width(320, 8), 312);
    }

    /// The clamp that keeps a surface presentable: an overscan at or beyond
    /// the stride still leaves one column, never zero and never a wrap.
    #[test]
    fn overscan_never_crops_below_one_column() {
        assert_eq!(presented_width(320, 320), 1);
        assert_eq!(presented_width(320, 100_000), 1);
        assert_eq!(presented_width(1, 4), 1);
    }

    /// A zero stride cannot underflow into a huge width.
    #[test]
    fn a_zero_stride_yields_one_column_not_an_underflow() {
        assert_eq!(presented_width(0, 0), 1);
        assert_eq!(presented_width(0, 1), 1);
    }

    #[test]
    fn an_absurd_stride_is_capped_at_the_surface_limit() {
        assert_eq!(presented_width(100_000, 0), MAX_SURFACE_DIMENSION);
    }

    /// Equivalence with the inline expression these helpers replaced at four
    /// present-path sites, swept over the edges that matter: a zero and a
    /// one-column stride (saturating subtraction), the clamp boundary, and
    /// `usize::MAX` on both arguments. This is what says the extraction did
    /// not change a pixel.
    #[test]
    fn presented_width_matches_the_inline_expression_it_replaced() {
        fn inline(src_stride: usize, overscan: usize) -> usize {
            let overscan = overscan.min(src_stride.saturating_sub(1));
            (src_stride - overscan).clamp(1, 4096)
        }
        let strides = [
            0usize, 1, 2, 3, 319, 320, 321, 639, 640, 641, 4095, 4096, 4097, 100_000,
            usize::MAX,
        ];
        let overscans = [0usize, 1, 2, 8, 319, 320, 4096, 100_000, usize::MAX];
        for &stride in &strides {
            for &overscan in &overscans {
                assert_eq!(
                    presented_width(stride, overscan),
                    inline(stride, overscan),
                    "stride={stride} overscan={overscan}"
                );
            }
        }
    }

    #[test]
    fn presented_height_matches_the_inline_expression_it_replaced() {
        for height in [0usize, 1, 236, 237, 239, 240, 4095, 4096, 4097, 100_000, usize::MAX] {
            assert_eq!(presented_height(height), height.clamp(1, 4096), "h={height}");
        }
    }

    #[test]
    fn the_active_output_height_is_clamped_into_a_presentable_range() {
        // WM2000's real 237, unchanged.
        assert_eq!(presented_height(237), 237);
        assert_eq!(presented_height(0), 1);
        assert_eq!(presented_height(100_000), MAX_SURFACE_DIMENSION);
    }
}
