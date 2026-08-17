//! Pure coverage semantics: the smallest self-contained slice of
//! `docs/RT64-BLENDER-DEPTH-PORT-CARD.md` §2 ("Coverage").
//!
//! Characterization-first, selective literal port of
//! `crates/fn64-render-reference/src/raster/{mod,coverage}.rs`'s `Coverage`
//! newtype, `CoverageMask`, and `coverage_result`/`apply_coverage_alpha`
//! functions, re-expressed as a standalone Rust/WGSL seam for
//! `fn64-render-wgpu`. Every function below accepts its mode bits and prior
//! coverage as plain typed values (`bool`s, `u8`s, `crate::state::
//! CoverageDestination`) rather than importing `fn64-render-reference` or
//! reading `state::OtherMode` itself -- `fn64-render-wgpu` has no dependency
//! on `fn64-render-reference` (see `depth_strict_less.rs`'s module doc for
//! the same constraint). `state::OtherMode` now decodes the full coverage
//! bitfield contract (`coverage_destination`/`image_read_enabled`/
//! `force_blend`/`antialias_enabled`/`coverage_times_alpha`/
//! `alpha_coverage_select`, landed alongside this module), so this module
//! reuses its `CoverageDestination` type verbatim instead of duplicating it
//! -- callers extract the plain values from their own `OtherMode` and pass
//! them in; this module still does not read `OtherMode` directly, keeping
//! the pure-value-in/value-out seam.
//!
//! RT64 citation (inherited, not independently re-verified -- see the port
//! card's source-availability caveat): `shared/rt64_other_mode.h` for the
//! `CVG_DST_*`/`IM_RD`/`AA_EN`/`FORCE_BL` bit semantics this module's callers
//! are expected to decode from, at pin
//! `5473732a822a4423b5696e7cb18fecc425a59875`. This module does not read or
//! cite RT64 source text directly; every arithmetic fact below is inherited
//! from `fn64-render-reference`'s own citations, ported as literal Rust, not
//! rederived.
//!
//! Explicitly out of scope, matching the port card's "Nonclaims" and its
//! smallest-slice ordering (coverage is step 5, after depth/alpha-compare):
//! the framebuffer-read mechanism that supplies `memory: Coverage` (§ card
//! "framebuffer-read problem", step 8), any draw-path wiring, RHI, bind
//! groups, or global/mutable state. Every function here is `const` where
//! possible or otherwise a pure value-in/value-out transform.

use crate::state::CoverageDestination;

/// RDP coverage: the population count (0..=8) of the public eight-sample
/// checkerboard mask covered by one fragment. Mirrors
/// `fn64-render-reference::raster::Coverage` (`raster/mod.rs:156-197`)
/// exactly, including its invariant (`count <= 8`, enforced by panic, not
/// clamp) and its two encodings: `stored`/`from_stored` for the RDRAM 3-bit
/// `count - 1` representation, and `alpha`/`times_alpha` for the blender's
/// normalized-`u8` coverage-as-alpha inputs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Coverage(u8);

impl Coverage {
    pub const FULL: Self = Self(8);
    pub const ZERO: Self = Self(0);

    /// Panics if `count` exceeds eight samples -- matches the reference's
    /// `assert!` exactly; this is a loud invariant, not a clamp (AGENTS.md
    /// "loud traps, no silent shrugs").
    pub const fn new(count: u8) -> Self {
        assert!(count <= 8, "RDP coverage count exceeds eight samples");
        Self(count)
    }

    pub const fn count(self) -> u8 {
        self.0
    }

    /// Decodes the RDRAM-resident 3-bit `count - 1` representation. Only the
    /// low three bits of `stored` are consulted, matching
    /// `Coverage::from_stored` (`raster/mod.rs:173-175`).
    pub const fn from_stored(stored: u8) -> Self {
        Self::new((stored & 7) + 1)
    }

    /// Encodes to the RDRAM-resident 3-bit `count - 1` representation.
    /// Debug-asserts `count() > 0` -- zero coverage is never stored, matching
    /// `Coverage::stored` (`raster/mod.rs:177-180`).
    pub const fn stored(self) -> u8 {
        debug_assert!(self.0 > 0, "zero coverage is never stored in RDRAM");
        self.0 - 1
    }

    /// Coverage as a normalized `[0,255]` blender alpha input:
    /// `(count*255 + 4) / 8`, round-to-nearest of the `count/8` fraction.
    /// Matches `Coverage::alpha` (`raster/mod.rs:182-189`) exactly, including
    /// its explicit "unverified frontier" status -- this is a documented fn64
    /// policy for the RDP's unpublished internal encoding, not a hardware
    /// fact (see module doc and the port card's Nonclaims).
    pub const fn alpha(self) -> u8 {
        (((self.0 as u16) * 255 + 4) / 8) as u8
    }

    /// Coverage multiplied by a post-combiner alpha byte, rounded to the
    /// nearest representable one-eighth: `(count*alpha + 127) / 255`. Matches
    /// `Coverage::times_alpha` (`raster/mod.rs:191-196`) exactly, including
    /// its "unverified frontier" status for exact gate-level tie behavior.
    pub const fn times_alpha(self, alpha: u8) -> Self {
        Self::new((((self.0 as u16) * (alpha as u16) + 127) / 255) as u8)
    }
}

/// The four independently-decoded `OtherMode` bits `coverage_result` reads,
/// passed as plain booleans by the caller (see module doc: this module does
/// not decode `state::OtherMode` itself).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoverageModeBits {
    pub image_read_enabled: bool,
    pub force_blend: bool,
    pub antialias_enabled: bool,
    pub coverage_destination: CoverageDestination,
}

/// The full `cvg_dst` accumulation outcome. Matches
/// `fn64-render-reference`'s `CoverageResult` (`raster/coverage.rs:5-11`),
/// public here (unlike the reference's `pub(super)`) since this module is
/// this crate's own top-level coverage seam.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoverageResult {
    pub pixel: Coverage,
    pub memory: Coverage,
    pub destination: Coverage,
    pub wraps: bool,
    pub blend_enabled: bool,
}

/// `cvg_dst` accumulation: literal port of
/// `fn64-render-reference::raster::coverage::coverage_result`
/// (`raster/coverage.rs:61-99`).
///
/// - `sum = pixel.count() + memory.count()` if `image_read_enabled`, else
///   just `pixel.count()`.
/// - `wraps = image_read_enabled && sum > 8`.
/// - `blend_enabled = force_blend || (antialias_enabled && !wraps)`.
/// - `destination` per the four `CoverageDestination` modes: **Clamp** ->
///   `min(sum,8)` if `image_read_enabled && blend_enabled` else `pixel`;
///   **Wrap** -> `sum-8` if `wraps` else `sum` (only if `image_read_enabled`,
///   else `pixel`); **Full** -> always `Coverage::FULL`; **Save** -> always
///   `memory` (pass-through, no accumulation).
pub const fn coverage_result(
    pixel: Coverage,
    memory: Coverage,
    mode: CoverageModeBits,
) -> CoverageResult {
    let sum = if mode.image_read_enabled {
        pixel.0 + memory.0
    } else {
        pixel.0
    };
    let wraps = mode.image_read_enabled && sum > Coverage::FULL.0;
    let blend_enabled = mode.force_blend || (mode.antialias_enabled && !wraps);
    let destination = match mode.coverage_destination {
        CoverageDestination::Clamp => {
            if mode.image_read_enabled && blend_enabled {
                Coverage::new(if sum > 8 { 8 } else { sum })
            } else {
                pixel
            }
        }
        CoverageDestination::Wrap => {
            if mode.image_read_enabled {
                Coverage::new(if wraps { sum - Coverage::FULL.0 } else { sum })
            } else {
                pixel
            }
        }
        CoverageDestination::Full => Coverage::FULL,
        CoverageDestination::Save => memory,
    };
    CoverageResult {
        pixel,
        memory,
        destination,
        wraps,
        blend_enabled,
    }
}

/// Coverage-to-alpha interaction: literal port of
/// `fn64-render-reference::raster::coverage::apply_coverage_alpha`
/// (`raster/coverage.rs:101-115`).
///
/// `coverage_times_alpha` (`OtherMode` bit `low[12]`) multiplies `coverage`
/// by the fragment's current alpha channel (`rgba[3]`) first, matching
/// `Coverage::times_alpha`'s rounding. `alpha_coverage_select` (bit
/// `low[13]`) then independently overwrites `rgba[3]` with the (possibly
/// multiplied) coverage's `alpha()` encoding. The two bits are independent:
/// either, both, or neither may be set.
pub const fn apply_coverage_alpha(
    coverage_times_alpha: bool,
    alpha_coverage_select: bool,
    mut rgba: [u8; 4],
    coverage: Coverage,
) -> ([u8; 4], Coverage) {
    let coverage = if coverage_times_alpha {
        coverage.times_alpha(rgba[3])
    } else {
        coverage
    };
    if alpha_coverage_select {
        rgba[3] = coverage.alpha();
    }
    (rgba, coverage)
}

/// The eight public subpixel sample positions, in eighth-pixel units, that
/// define the RDP's coverage checkerboard mask. Matches
/// `fn64-render-reference::raster::coverage::COVERAGE_SAMPLES`
/// (`raster/coverage.rs:119-128`) exactly, index-for-index -- `CoverageMask`
/// bit `i` corresponds to `COVERAGE_SAMPLES[i]`.
pub const COVERAGE_SAMPLES: [(i32, i32); 8] = [
    (1, 1),
    (5, 1),
    (3, 3),
    (7, 3),
    (1, 5),
    (5, 5),
    (3, 7),
    (7, 7),
];

/// One eight-sample coverage mask: bit `i` set means `COVERAGE_SAMPLES[i]`
/// was covered. Matches `fn64-render-reference::raster::CoverageMask`
/// (`raster/coverage.rs:180`) exactly; public here since this module owns no
/// rasterizer to keep it crate-private against.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageMask(pub u8);

impl CoverageMask {
    pub const EMPTY: Self = Self(0);
    pub const FULL: Self = Self(0xff);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn contains(self, sample_index: usize) -> bool {
        assert!(sample_index < COVERAGE_SAMPLES.len());
        self.0 & (1u8 << sample_index) != 0
    }

    /// Population count as a [`Coverage`]. Matches `CoverageMask::coverage`
    /// (`raster/coverage.rs:198-200`).
    pub const fn coverage(self) -> Coverage {
        Coverage::new(self.0.count_ones() as u8)
    }
}

/// One coverage point proven to lie on a partially covered primitive.
/// Matches `fn64-render-reference::raster::coverage::CoveredAttributeSample`
/// (`raster/coverage.rs:231-235`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoveredAttributeSample {
    pub sample_index: u8,
    pub x_eighth: i32,
    pub y_eighth: i32,
}

/// Typed distinction between the uncorrected full-pixel center and a
/// coverage-derived on-primitive correction point. Matches
/// `fn64-render-reference::raster::coverage::AttributeSamplePoint`
/// (`raster/coverage.rs:239-243`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AttributeSamplePoint {
    PixelCenter,
    Covered(CoveredAttributeSample),
}

impl AttributeSamplePoint {
    pub const fn offsets_eighth(self) -> (i32, i32) {
        match self {
            Self::PixelCenter => (4, 4),
            Self::Covered(sample) => (sample.x_eighth, sample.y_eighth),
        }
    }
}

/// Bounded host policy for choosing one on-primitive attribute point from a
/// partial coverage mask -- nearest to pixel center, `COVERAGE_SAMPLES`
/// order breaking ties. Matches
/// `fn64-render-reference::raster::coverage::PartialAttributeSamplePolicy::NearestToPixelCenterStableOrder`'s
/// precomputed preference order (`raster/coverage.rs:156`) exactly. This is
/// an explicit fn64 policy choice, not a silicon fact (port card §2,
/// partition 5) -- a future WGSL port may treat this as an open frontier
/// rather than reproducing it.
const PARTIAL_ATTRIBUTE_PREFERENCE: [usize; 8] = [2, 5, 1, 3, 4, 6, 0, 7];

/// Selects the attribute-sample point for a coverage mask. Panics on empty
/// coverage (matches the reference's `assert!(self.0 != 0, ...)`,
/// `raster/coverage.rs:214`); full coverage (`0xff`) returns
/// [`AttributeSamplePoint::PixelCenter`] without consulting the preference
/// order.
pub const fn attribute_sample(mask: CoverageMask) -> AttributeSamplePoint {
    assert!(mask.0 != 0, "zero coverage has no attribute sample");
    if mask.0 == u8::MAX {
        return AttributeSamplePoint::PixelCenter;
    }
    let mut i = 0;
    while i < PARTIAL_ATTRIBUTE_PREFERENCE.len() {
        let sample_index = PARTIAL_ATTRIBUTE_PREFERENCE[i];
        if mask.0 & (1u8 << sample_index) != 0 {
            let (x_eighth, y_eighth) = COVERAGE_SAMPLES[sample_index];
            return AttributeSamplePoint::Covered(CoveredAttributeSample {
                sample_index: sample_index as u8,
                x_eighth,
                y_eighth,
            });
        }
        i += 1;
    }
    panic!("nonzero partial coverage lost every checkerboard sample");
}

pub const COVERAGE_WGSL: &str = include_str!("coverage.wgsl");
pub const COVERAGE_ENTRY_POINT: &str = "evaluate_coverage";

/// Fragment-callable twin of [`COVERAGE_WGSL`]'s existing `evaluate`
/// compute-shader logic: an ordinary WGSL function (`coverage_fragment_fn`,
/// no `@compute`, no `@group`/`@binding`, no entry point) taking scalar
/// arguments and returning a plain struct, concatenatable into a future
/// `@fragment` entry point the same way `color_combiner.wgsl` already is per
/// `shaders/triangle_pipeline_fragment.wgsl`'s header. Not wired into any
/// draw path, bind group layout, or pipeline used elsewhere in this crate --
/// see this module's doc comment and the sibling `coverage.wgsl`'s own
/// header for the shared scope boundary. The existing `COVERAGE_WGSL`
/// `@compute` entry point is untouched by this addition.
pub const COVERAGE_FRAGMENT_FN_WGSL: &str = include_str!("coverage_fragment_fn.wgsl");

#[cfg(test)]
mod tests;
