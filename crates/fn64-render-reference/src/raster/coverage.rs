use crate::gbi::*;
use super::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct CoverageResult {
    pub(super) pixel: Coverage,
    pub(super) memory: Coverage,
    pub(super) destination: Coverage,
    pub(super) wraps: bool,
    pub(super) blend_enabled: bool,
}

/// Framebuffer color/coverage made available to the blender by `IM_RD`.
///
/// Programming Manual Chapter 15, "Mode Bit Descriptions," defines `IM_RD`
/// as enabling the color/coverage read-modify-write access. Keeping the old
/// sample optional prevents a disabled read from remaining accidentally
/// observable through either a framebuffer color mux or `G_BL_A_MEM`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ReadFramebufferMemory {
    pub(super) rgba: [u8; 4],
    pub(super) coverage: Coverage,
}

/// Publicly specified routing between coverage wrap and the four Z modes.
///
/// Programming Manual Chapter 15, "Blender Modes and Assumptions," requires
/// wrapping interpenetrating fragments to take a coverage-adjustment path.
/// The manual does not publish that adjustment's arithmetic, so keeping the
/// unsupported outcome in this type prevents it from silently collapsing to
/// the ordinary opaque correlation test.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum DepthCoverageDecision {
    Pass,
    Reject,
    UnsupportedInterpenetratingCoverageAdjustment,
}

pub(super) fn depth_coverage_decision(
    mode: crate::gbi::DepthMode,
    relations: crate::depth::DepthRelations,
    coverage_wraps: bool,
) -> DepthCoverageDecision {
    use crate::gbi::DepthMode;

    if mode == DepthMode::Interpenetrating && coverage_wraps {
        return DepthCoverageDecision::UnsupportedInterpenetratingCoverageAdjustment;
    }
    let passes = if mode == DepthMode::Opaque && coverage_wraps {
        relations.in_front
    } else {
        crate::depth::mode_passes(mode, relations)
    };
    if passes {
        DepthCoverageDecision::Pass
    } else {
        DepthCoverageDecision::Reject
    }
}

pub(super) fn coverage_result(pixel: Coverage, memory: Coverage, other_mode: OtherMode) -> CoverageResult {
    let image_read = other_mode.image_read_enabled();
    let sum = if image_read {
        pixel.count() + memory.count()
    } else {
        pixel.count()
    };
    let wraps = image_read && sum > Coverage::FULL.count();
    let blend_enabled = other_mode.force_blend() || (other_mode.antialias_enabled() && !wraps);
    let destination = match other_mode.coverage_destination() {
        CoverageDestination::Clamp => {
            if image_read && blend_enabled {
                Coverage::new(sum.min(Coverage::FULL.count()))
            } else {
                pixel
            }
        }
        CoverageDestination::Wrap => {
            if image_read {
                Coverage::new(if wraps {
                    sum - Coverage::FULL.count()
                } else {
                    sum
                })
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

pub(super) fn apply_coverage_alpha(
    other_mode: OtherMode,
    mut rgba: [u8; 4],
    coverage: Coverage,
) -> ([u8; 4], Coverage) {
    let coverage = if other_mode.coverage_times_alpha() {
        coverage.times_alpha(rgba[3])
    } else {
        coverage
    };
    if other_mode.alpha_coverage_select() {
        rgba[3] = coverage.alpha();
    }
    (rgba, coverage)
}

/// Selected sample offsets in eighth-pixel units. The public mask lies on a
/// 4×4 grid, so odd eighths express every sample center exactly.
pub(super) const COVERAGE_SAMPLES: [(i32, i32); 8] = [
    (1, 1),
    (5, 1),
    (3, 3),
    (7, 3),
    (1, 5),
    (5, 5),
    (3, 7),
    (7, 7),
];

/// Bounded host policy for choosing one on-primitive attribute point from a
/// partial coverage mask.
///
/// The public checkerboard positions and the requirement to correct partial
/// Z onto the primitive are known, but the silicon lookup is not. Keeping the
/// policy typed prevents a stable-order tie from masquerading as a discovered
/// centroid rule when hardware evidence eventually replaces it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PartialAttributeSamplePolicy {
    NearestToPixelCenterStableOrder,
}

pub(super) const PARTIAL_ATTRIBUTE_SAMPLE_POLICY: PartialAttributeSamplePolicy =
    PartialAttributeSamplePolicy::NearestToPixelCenterStableOrder;

impl PartialAttributeSamplePolicy {
    fn select(self, mask: CoverageMask) -> CoveredAttributeSample {
        assert!(
            mask.0 != 0 && mask.0 != u8::MAX,
            "partial attribute selector requires coverage from one through seven samples"
        );
        match self {
            Self::NearestToPixelCenterStableOrder => {
                // Increasing squared distance from the pixel center, with
                // equal-distance samples retaining COVERAGE_SAMPLES order.
                // This total order is the bounded policy, not a silicon fact.
                const PREFERENCE: [usize; 8] = [2, 5, 1, 3, 4, 6, 0, 7];
                let sample_index = PREFERENCE
                    .into_iter()
                    .find(|&index| mask.0 & (1u8 << index) != 0)
                    .expect("nonzero partial coverage lost every checkerboard sample");
                let (x_eighth, y_eighth) = COVERAGE_SAMPLES[sample_index];
                CoveredAttributeSample {
                    sample_index: u8::try_from(sample_index)
                        .expect("eight-sample attribute index exceeds u8"),
                    x_eighth,
                    y_eighth,
                }
            }
        }
    }
}

/// Identity-preserving form of the public eight-sample coverage result.
///
/// Framebuffer storage keeps only the population count, but raster coverage
/// and future attribute correction need to know *which* sample positions were
/// covered. Every triangle/line evaluator returns this type and derives the
/// count only at the fragment boundary, preventing an early identity collapse.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CoverageMask(pub(super) u8);

impl CoverageMask {
    pub(super) fn from_samples(mut covered: impl FnMut(i32, i32) -> bool) -> Self {
        let bits = COVERAGE_SAMPLES.iter().enumerate().fold(
            0u8,
            |bits, (index, &(offset_x, offset_y))| {
                if covered(offset_x, offset_y) {
                    bits | (1u8 << index)
                } else {
                    bits
                }
            },
        );
        Self(bits)
    }

    pub(super) fn coverage(self) -> Coverage {
        Coverage::new(self.0.count_ones() as u8)
    }

    /// Choose the point at which this fragment's attribute planes are
    /// evaluated.
    ///
    /// Programming Manual 15.4 requires partially covered Z samples to be
    /// corrected onto the primitive, but does not publish the RDP's covered-
    /// sample lookup. Full coverage therefore retains the ordinary pixel
    /// center. For partial coverage, the bounded reference chooses the
    /// covered checkerboard sample nearest that center, breaking equal-
    /// distance ties by the public sample-array order above. Keeping this as
    /// a typed policy prevents raw triangles, high-level triangles, and lines
    /// from silently choosing different correction points.
    pub(super) fn attribute_sample(self) -> AttributeSamplePoint {
        assert!(self.0 != 0, "zero coverage has no attribute sample");
        if self.0 == u8::MAX {
            return AttributeSamplePoint::PixelCenter;
        }

        AttributeSamplePoint::Covered(PARTIAL_ATTRIBUTE_SAMPLE_POLICY.select(self))
    }

    #[cfg(test)]
    pub(super) fn contains(self, sample_index: usize) -> bool {
        assert!(sample_index < COVERAGE_SAMPLES.len());
        self.0 & (1u8 << sample_index) != 0
    }
}

/// One coverage point proven to lie on a partially covered primitive.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct CoveredAttributeSample {
    pub(super) sample_index: u8,
    pub(super) x_eighth: i32,
    pub(super) y_eighth: i32,
}

/// Typed distinction between the uncorrected full-pixel center and a
/// coverage-derived on-primitive correction point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AttributeSamplePoint {
    PixelCenter,
    Covered(CoveredAttributeSample),
}

impl AttributeSamplePoint {
    pub(super) fn offsets_eighth(self) -> (i32, i32) {
        match self {
            Self::PixelCenter => (4, 4),
            Self::Covered(sample) => (sample.x_eighth, sample.y_eighth),
        }
    }
}

pub(super) const Q16_ONE: i64 = 1 << 16;

pub(super) fn fixed_mul_ratio(value: i32, numerator: i64, denominator: i64) -> i64 {
    i64::try_from((i128::from(value) * i128::from(numerator)).div_euclid(i128::from(denominator)))
        .expect("raw RDP fixed-point slope evaluation exceeds i64")
}

pub(super) fn ceil_ratio(numerator: i64, denominator: i64) -> i64 {
    -(-numerator).div_euclid(denominator)
}

pub(super) fn round_ratio(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (numerator + denominator / 2).div_euclid(denominator)
    } else {
        -((-numerator + denominator / 2).div_euclid(denominator))
    }
}

pub(super) fn raw_attribute_plane(
    base: i32,
    dx: i32,
    de: i32,
    edge_delta_y_eighth: i32,
    edge_delta_x_q16: i64,
) -> i64 {
    let x_term = i64::try_from(
        (i128::from(dx) * i128::from(edge_delta_x_q16)).div_euclid(i128::from(Q16_ONE)),
    )
    .expect("raw RDP attribute d/dx evaluation exceeds i64");
    i64::from(base)
        .checked_add(fixed_mul_ratio(de, i64::from(edge_delta_y_eighth), 8))
        .and_then(|value| value.checked_add(x_term))
        .expect("raw RDP attribute plane evaluation exceeds i64")
}
