//! Deterministic software rasterization into an RGBA8888 working framebuffer.
//! F3DEX2 geometry uses the existing barycentric path. Raw RDP triangles retain
//! their major/minor edge and attribute coefficient planes and walk commanded
//! spans directly with the public eight-sample checkerboard coverage mask.
//! Fixed-width edge/attribute accumulator truncation remains an explicit
//! fidelity gap.

use crate::depth::EncodedDepth;
use crate::gbi::{
    AlphaCompare, AlphaDither, AlphaSource, BlendAlphaInput, BlendBInput, BlendColorInput,
    BlenderState, ColorImage, ColorImageLayout, ColorSource, CombinerCycle, CombinerState,
    CoverageDestination, CullMode, CycleType, FillRectangle, Line, OtherMode, PrimitiveDepth,
    RawRdpTriangle, RgbDither, ScissorRect, TextureDerivatives, TextureRectangle,
    TextureSampleRequest, Triangle, Vertex,
};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct DepthControl {
    compare: bool,
    update: bool,
    mode: crate::gbi::DepthMode,
}

impl DepthControl {
    const DISABLED: Self = Self {
        compare: false,
        update: false,
        mode: crate::gbi::DepthMode::Opaque,
    };

    fn from_other_mode(other_mode: OtherMode) -> Self {
        Self {
            compare: other_mode.depth_compare_enabled(),
            update: other_mode.depth_update_enabled(),
            mode: other_mode.depth_mode(),
        }
    }

    fn for_line(other_mode: OtherMode) -> Self {
        Self {
            compare: other_mode.depth_compare_enabled(),
            // Public line modes may read Z but never update it.
            update: false,
            mode: other_mode.depth_mode(),
        }
    }
}

#[derive(Copy, Clone)]
struct FragmentInputs {
    z: f32,
    delta_z: u16,
    encoded_depth: Option<EncodedDepth>,
    coverage: Coverage,
    shade: [u8; 4],
    texel0: [u8; 4],
    texel1: [u8; 4],
    lod_fraction: f32,
}

#[derive(Copy, Clone)]
struct FragmentPipeline {
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
    depth: DepthControl,
}

#[derive(Copy, Clone)]
struct DepthFragment {
    z: f32,
    delta_z: u16,
    encoded_depth: Option<EncodedDepth>,
    coverage: Coverage,
    rgba: [u8; 4],
    shade_alpha: u8,
    noise: NoiseSample,
}

#[derive(Copy, Clone)]
struct ColorFragment {
    rgba: [u8; 4],
    coverage: Coverage,
    shade_alpha: u8,
    noise: NoiseSample,
}

/// One eight-bit pseudo-random value consumed by a covered RDP fragment.
///
/// The public Programming Manual defines one random threshold per pixel and
/// routes that value to combiner NOISE, RGB/alpha noise dither, and
/// `G_AC_DITHER`. It does not publish the hardware generator or its seed, so
/// the deterministic reference policy below is deliberately not described as
/// the silicon sequence. Keeping the value typed and singular does preserve
/// the observable same-fragment routing invariant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct NoiseSample(u8);

impl NoiseSample {
    #[cfg(test)]
    const ZERO: Self = Self(0);

    fn byte(self) -> u8 {
        self.0
    }

    fn dither(self) -> u8 {
        self.0 & 7
    }

    fn unit(self) -> f32 {
        f32::from(self.0) / 255.0
    }
}

/// Reproducible host policy for the RDP's publicly random, but unpublished,
/// per-pixel noise stream.
///
/// SplitMix64 supplies a long-period, uniform stream without pretending to be
/// the RDP's unknown polynomial. A fixed default keeps framebuffer digests
/// stable; callers can set a different explicit seed when exercising temporal
/// noise. The stream advances exactly once for each covered one/two-cycle
/// fragment entering the combiner, including fragments later rejected by
/// alpha or depth.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct NoiseState {
    seed: u64,
    fragment_index: u64,
}

impl Default for NoiseState {
    fn default() -> Self {
        Self {
            seed: Framebuffer::DEFAULT_NOISE_SEED,
            fragment_index: 0,
        }
    }
}

impl NoiseState {
    fn reseed(&mut self, seed: u64) {
        self.seed = seed;
        self.fragment_index = 0;
    }

    fn next_sample(&mut self) -> NoiseSample {
        let mut value = self
            .seed
            .wrapping_add(self.fragment_index.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        self.fragment_index = self.fragment_index.wrapping_add(1);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        NoiseSample((value ^ (value >> 31)) as u8)
    }
}

/// RDP coverage is the intersection of a 4x4 subpixel grid with the public
/// checkerboard dither mask, yielding exactly 0..=8 selected samples.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Coverage(u8);

impl Coverage {
    pub(crate) const FULL: Self = Self(8);

    pub(crate) fn new(count: u8) -> Self {
        assert!(
            count <= 8,
            "RDP coverage count {count} exceeds eight samples"
        );
        Self(count)
    }

    pub(crate) fn count(self) -> u8 {
        self.0
    }

    pub(crate) fn from_stored(stored: u8) -> Self {
        Self::new((stored & 7) + 1)
    }

    pub(crate) fn stored(self) -> u8 {
        debug_assert!(self.0 > 0, "zero coverage is never stored in RDRAM");
        self.0 - 1
    }

    fn alpha(self) -> u8 {
        // Coverage is a normalized 0..1 blender input. Preserve the exact
        // endpoints while mapping its eight discrete nonzero steps to u8.
        // The public manual says the blender alpha mux has five-bit
        // resolution but does not publish this selector's internal encoding;
        // normalized-u8 remains an explicit hardware-vector frontier.
        ((u16::from(self.0) * 255 + 4) / 8) as u8
    }

    fn times_alpha(self, alpha: u8) -> Self {
        // The public coverage/alpha combiner defines this operation as a
        // normalized product. Round to the nearest representable one-eighth;
        // exact gate-level tie behavior remains a differential-test frontier.
        Self::new(((u16::from(self.0) * u16::from(alpha) + 127) / 255) as u8)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct CoverageResult {
    pixel: Coverage,
    memory: Coverage,
    destination: Coverage,
    wraps: bool,
    blend_enabled: bool,
}

/// Publicly specified routing between coverage wrap and the four Z modes.
///
/// Programming Manual Chapter 15, "Blender Modes and Assumptions," requires
/// wrapping interpenetrating fragments to take a coverage-adjustment path.
/// The manual does not publish that adjustment's arithmetic, so keeping the
/// unsupported outcome in this type prevents it from silently collapsing to
/// the ordinary opaque correlation test.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DepthCoverageDecision {
    Pass,
    Reject,
    UnsupportedInterpenetratingCoverageAdjustment,
}

fn depth_coverage_decision(
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

fn coverage_result(pixel: Coverage, memory: Coverage, other_mode: OtherMode) -> CoverageResult {
    let sum = pixel.count() + memory.count();
    let wraps = sum > Coverage::FULL.count();
    let blend_enabled = other_mode.force_blend() || (other_mode.antialias_enabled() && !wraps);
    let destination = match other_mode.coverage_destination() {
        CoverageDestination::Clamp => {
            if blend_enabled {
                Coverage::new(sum.min(Coverage::FULL.count()))
            } else {
                pixel
            }
        }
        CoverageDestination::Wrap => Coverage::new(if wraps {
            sum - Coverage::FULL.count()
        } else {
            sum
        }),
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

fn apply_coverage_alpha(
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
const COVERAGE_SAMPLES: [(i32, i32); 8] = [
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
enum PartialAttributeSamplePolicy {
    NearestToPixelCenterStableOrder,
}

const PARTIAL_ATTRIBUTE_SAMPLE_POLICY: PartialAttributeSamplePolicy =
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
struct CoverageMask(u8);

impl CoverageMask {
    fn from_samples(mut covered: impl FnMut(i32, i32) -> bool) -> Self {
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

    fn coverage(self) -> Coverage {
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
    /// a typed policy prevents raw and high-level triangles from silently
    /// choosing different correction points.
    fn attribute_sample(self) -> AttributeSamplePoint {
        assert!(self.0 != 0, "zero coverage has no attribute sample");
        if self.0 == u8::MAX {
            return AttributeSamplePoint::PixelCenter;
        }

        AttributeSamplePoint::Covered(PARTIAL_ATTRIBUTE_SAMPLE_POLICY.select(self))
    }

    #[cfg(test)]
    fn contains(self, sample_index: usize) -> bool {
        assert!(sample_index < COVERAGE_SAMPLES.len());
        self.0 & (1u8 << sample_index) != 0
    }
}

/// One coverage point proven to lie on a partially covered primitive.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct CoveredAttributeSample {
    sample_index: u8,
    x_eighth: i32,
    y_eighth: i32,
}

/// Typed distinction between the uncorrected full-pixel center and a
/// coverage-derived on-primitive correction point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AttributeSamplePoint {
    PixelCenter,
    Covered(CoveredAttributeSample),
}

impl AttributeSamplePoint {
    fn offsets_eighth(self) -> (i32, i32) {
        match self {
            Self::PixelCenter => (4, 4),
            Self::Covered(sample) => (sample.x_eighth, sample.y_eighth),
        }
    }
}

const Q16_ONE: i64 = 1 << 16;

fn fixed_mul_ratio(value: i32, numerator: i64, denominator: i64) -> i64 {
    i64::try_from((i128::from(value) * i128::from(numerator)).div_euclid(i128::from(denominator)))
        .expect("raw RDP fixed-point slope evaluation exceeds i64")
}

fn ceil_ratio(numerator: i64, denominator: i64) -> i64 {
    -(-numerator).div_euclid(denominator)
}

fn round_ratio(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (numerator + denominator / 2).div_euclid(denominator)
    } else {
        -((-numerator + denominator / 2).div_euclid(denominator))
    }
}

fn raw_attribute_plane(
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

/// Evaluate both programmed RDP color-combiner cycles.
///
/// Each cycle computes `(A - B) * C + D` independently for RGB and alpha.
/// The source meanings follow RT64's MIT `shared/rt64_color_combiner.h`
/// `fromColorInput`/`fromAlphaInput` (lines 468-540), and the equation/cycle
/// ordering follows `runCycle` (lines 567-608). The decoded presets duplicate
/// inactive one-cycle terms, while OoT's PASS2/`*2` presets consume COMBINED
/// and therefore require the sequential two-cycle result.
#[derive(Copy, Clone)]
struct CombinerPixel {
    lod_fraction: f32,
    shade: [u8; 4],
    texel0: [u8; 4],
    texel1: [u8; 4],
    noise: NoiseSample,
}

#[cfg(test)]
impl CombinerPixel {
    fn new(
        lod_fraction: f32,
        shade: [u8; 4],
        texel0: [u8; 4],
        texel1: [u8; 4],
        noise: NoiseSample,
    ) -> Self {
        Self {
            lod_fraction,
            shade,
            texel0,
            texel1,
            noise,
        }
    }
}

fn evaluate_combiner(
    state: CombinerState,
    cycle_type: CycleType,
    key_enabled: bool,
    pixel: CombinerPixel,
) -> [u8; 4] {
    let to_unit = |rgba: [u8; 4]| rgba.map(|v| v as f32 / 255.0);
    let mut inputs = CombinerInputs {
        combined: [0.0; 4],
        texel0: to_unit(pixel.texel0),
        texel1: to_unit(pixel.texel1),
        primitive: to_unit(state.primitive),
        shade: to_unit(pixel.shade),
        environment: to_unit(state.environment),
        lod_fraction: pixel.lod_fraction,
        prim_lod_fraction: state.prim_lod_fraction as f32 / 255.0,
        k4: state.convert.k4(),
        k5: state.convert.k5(),
        key_center: state.key.center_unit(),
        key_scale: state.key.scale_unit(),
        noise: pixel.noise.unit(),
    };

    let cycle_count = match cycle_type {
        CycleType::OneCycle => 1,
        CycleType::TwoCycle => 2,
        CycleType::Copy | CycleType::Fill => {
            unreachable!("copy/fill cycle reached color combiner")
        }
    };
    for cycle in state.mode.cycles.into_iter().take(cycle_count) {
        inputs.combined = evaluate_cycle(cycle, &inputs);
    }
    if key_enabled {
        inputs.combined[3] = state.key.alpha_from_key_prime([
            inputs.combined[0],
            inputs.combined[1],
            inputs.combined[2],
        ]);
    }

    inputs
        .combined
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[derive(Copy, Clone)]
struct CombinerInputs {
    combined: [f32; 4],
    texel0: [f32; 4],
    texel1: [f32; 4],
    primitive: [f32; 4],
    shade: [f32; 4],
    environment: [f32; 4],
    lod_fraction: f32,
    prim_lod_fraction: f32,
    k4: f32,
    k5: f32,
    key_center: [f32; 3],
    key_scale: [f32; 3],
    noise: f32,
}

fn evaluate_cycle(cycle: CombinerCycle, inputs: &CombinerInputs) -> [f32; 4] {
    let a = color_input(cycle.rgb[0], inputs);
    let b = color_input(cycle.rgb[1], inputs);
    let c = color_input(cycle.rgb[2], inputs);
    let d = color_input(cycle.rgb[3], inputs);
    let mut out = [0.0; 4];
    for channel in 0..3 {
        out[channel] = (a[channel] - b[channel]) * c[channel] + d[channel];
    }

    let aa = alpha_input(cycle.alpha[0], inputs);
    let ab = alpha_input(cycle.alpha[1], inputs);
    let ac = alpha_input(cycle.alpha[2], inputs);
    let ad = alpha_input(cycle.alpha[3], inputs);
    out[3] = (aa - ab) * ac + ad;
    out
}

fn color_input(source: ColorSource, inputs: &CombinerInputs) -> [f32; 3] {
    let rgb = |rgba: [f32; 4]| [rgba[0], rgba[1], rgba[2]];
    let splat = |v| [v; 3];
    match source {
        ColorSource::Combined => rgb(inputs.combined),
        ColorSource::Texel0 => rgb(inputs.texel0),
        ColorSource::Texel1 => rgb(inputs.texel1),
        ColorSource::Primitive => rgb(inputs.primitive),
        ColorSource::Shade => rgb(inputs.shade),
        ColorSource::Environment => rgb(inputs.environment),
        ColorSource::CombinedAlpha => splat(inputs.combined[3]),
        ColorSource::Texel0Alpha => splat(inputs.texel0[3]),
        ColorSource::Texel1Alpha => splat(inputs.texel1[3]),
        ColorSource::PrimitiveAlpha => splat(inputs.primitive[3]),
        ColorSource::ShadeAlpha => splat(inputs.shade[3]),
        ColorSource::EnvironmentAlpha => splat(inputs.environment[3]),
        ColorSource::LodFraction => splat(inputs.lod_fraction),
        ColorSource::PrimLodFraction => splat(inputs.prim_lod_fraction),
        ColorSource::One => [1.0; 3],
        ColorSource::Zero => [0.0; 3],
        ColorSource::K4 => splat(inputs.k4),
        ColorSource::K5 => splat(inputs.k5),
        ColorSource::KeyCenter => inputs.key_center,
        ColorSource::KeyScale => inputs.key_scale,
        ColorSource::Noise => splat(inputs.noise),
    }
}

fn alpha_input(source: AlphaSource, inputs: &CombinerInputs) -> f32 {
    match source {
        AlphaSource::Combined => inputs.combined[3],
        AlphaSource::Texel0 => inputs.texel0[3],
        AlphaSource::Texel1 => inputs.texel1[3],
        AlphaSource::Primitive => inputs.primitive[3],
        AlphaSource::Shade => inputs.shade[3],
        AlphaSource::Environment => inputs.environment[3],
        AlphaSource::LodFraction => inputs.lod_fraction,
        AlphaSource::PrimLodFraction => inputs.prim_lod_fraction,
        AlphaSource::One => 1.0,
        AlphaSource::Zero => 0.0,
    }
}

/// TEMP instrumentation (env `FN64_DUMP_PROJ=1`): count z-test passes vs
/// rejections so a real overlapping-geometry frame can PROVE the z-buffer is
/// doing occlusion work (rejecting farther fragments) rather than being a
/// no-op. Gated entirely behind the env var; call `zstat::summary()` after a
/// frame to print + reset. Remove/keep behind the flag.
#[cfg(not(test))]
pub mod zstat {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INIT: AtomicBool = AtomicBool::new(false);
    static PASS: AtomicU64 = AtomicU64::new(0);
    static REJECT: AtomicU64 = AtomicU64::new(0);
    fn on() -> bool {
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(crate::debug_flag("FN64_DUMP_PROJ"), Ordering::Relaxed);
        }
        ENABLED.load(Ordering::Relaxed)
    }
    pub fn note_pass() {
        if on() {
            PASS.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn note_reject() {
        if on() {
            REJECT.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Print the frame's pass/reject counts and reset for the next frame.
    pub fn summary() {
        if !on() {
            return;
        }
        let p = PASS.swap(0, Ordering::Relaxed);
        let r = REJECT.swap(0, Ordering::Relaxed);
        if p + r > 0 {
            eprintln!(
                "[FN64_DUMP_PROJ] z-test: {p} passes (fragment written) | {r} rejects \
                 (farther fragment occluded) -- rejects>0 proves the z-buffer is \
                 doing real occlusion, not a no-op"
            );
        }
    }
}

#[derive(Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8888, row-major, top-left origin.
    pub pixels: Vec<u8>,
    /// Actual RDP coverage count (1..=8) for every resident color sample.
    /// Zero-coverage fragments never enter memory; RGBA16 stores `count - 1`
    /// across its visible LSB and two physical RDRAM hidden bits.
    pub(crate) coverage: Vec<Coverage>,
    /// Per-pixel RDP 18-bit 15.3 working depth (nearer = smaller), represented
    /// as `f32` only so the existing interpolators share one compare path.
    /// Parallel to `pixels` and initialized to `f32::INFINITY` until a depth
    /// image is selected or a fragment updates it. See `F3DEX2-CONCEPTS.md`
    /// §4.3.
    pub depth: Vec<f32>,
    /// Encoded Z-memory samples parallel to [`Self::depth`]. `None` means the
    /// software-only sample cannot be committed to an RDP depth image.
    pub(crate) encoded_depth: Vec<Option<EncodedDepth>>,
    primitive_depth: Option<PrimitiveDepth>,
    /// Active RDP color-image layout. Ordered RGB dither changes only the
    /// RGBA16 memory-interface reduction from eight to five color bits.
    color_layout: ColorImageLayout,
    noise: NoiseState,
}

fn raw_span_edges_at_y_eighth(
    edge: crate::gbi::RdpEdgeCoefficients,
    sample_y_eighth: i32,
) -> (i64, i64) {
    // RDP Command Summary Table 12 / page 15: XH and XM are evaluated at
    // the scanline preceding YH; XL is evaluated at the next subpixel at or
    // below YM. Y is S11.2, while this function's odd-eighth sample centers
    // retain the public checkerboard mask without a float conversion.
    let high_origin_eighth = i32::from(edge.yh & !3) * 2;
    let middle_eighth = i32::from(edge.ym) * 2;
    let major_x = i64::from(edge.xh)
        + fixed_mul_ratio(
            edge.dxhdy,
            i64::from(sample_y_eighth - high_origin_eighth),
            8,
        );
    let minor_x = if sample_y_eighth < middle_eighth {
        i64::from(edge.xm)
            + fixed_mul_ratio(
                edge.dxmdy,
                i64::from(sample_y_eighth - high_origin_eighth),
                8,
            )
    } else {
        i64::from(edge.xl)
            + fixed_mul_ratio(edge.dxldy, i64::from(sample_y_eighth - middle_eighth), 8)
    };
    if edge.right_major {
        (minor_x, major_x)
    } else {
        (major_x, minor_x)
    }
}

fn raw_pixel_coverage(
    edge: crate::gbi::RdpEdgeCoefficients,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> CoverageMask {
    if !scissor.line_enabled(y) {
        return CoverageMask::default();
    }
    let yh_eighth = i32::from(edge.yh) * 2;
    let yl_eighth = i32::from(edge.yl) * 2;
    let scissor_ulx_eighth = (scissor.ulx * 8.0).round() as i32;
    let scissor_uly_eighth = (scissor.uly * 8.0).round() as i32;
    let scissor_lrx_eighth = (scissor.lrx * 8.0).round() as i32;
    let scissor_lry_eighth = (scissor.lry * 8.0).round() as i32;
    CoverageMask::from_samples(|offset_x, offset_y| {
        let sample_x_eighth = x * 8 + offset_x;
        let sample_y_eighth = y * 8 + offset_y;
        if sample_x_eighth < scissor_ulx_eighth
            || sample_x_eighth >= scissor_lrx_eighth
            || sample_y_eighth < scissor_uly_eighth
            || sample_y_eighth >= scissor_lry_eighth
            || sample_y_eighth < yh_eighth
            || sample_y_eighth >= yl_eighth
        {
            return false;
        }
        let (left_x, right_x) = raw_span_edges_at_y_eighth(edge, sample_y_eighth);
        let sample_x = i64::from(sample_x_eighth) * Q16_ONE / 8;
        sample_x >= left_x && sample_x < right_x
    })
}

fn triangle_pixel_coverage(
    vertices: [Vertex; 3],
    area: f32,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> CoverageMask {
    if !scissor.line_enabled(y) {
        return CoverageMask::default();
    }
    let [a, b, c] = if area > 0.0 {
        vertices
    } else {
        [vertices[0], vertices[2], vertices[1]]
    };
    let top_left = |start: Vertex, end: Vertex| {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        // Screen Y grows downward. An upward edge is a left edge; a
        // horizontal edge directed right is a top edge. Exactly one of two
        // oppositely directed shared edges therefore owns an on-edge sample,
        // matching the raw RDP span walk's left-inclusive/right-exclusive
        // rule instead of double-counting it.
        dy < 0.0 || (dy == 0.0 && dx > 0.0)
    };
    let covered_by_edge = |start: Vertex, end: Vertex, sample: Vertex| {
        let value = edge(start, end, sample);
        value > 0.0 || (value == 0.0 && top_left(start, end))
    };
    CoverageMask::from_samples(|offset_x, offset_y| {
        let sample = Vertex {
            x: x as f32 + offset_x as f32 / 8.0,
            y: y as f32 + offset_y as f32 / 8.0,
            ..Default::default()
        };
        if sample.x < scissor.ulx
            || sample.x >= scissor.lrx
            || sample.y < scissor.uly
            || sample.y >= scissor.lry
        {
            return false;
        }
        covered_by_edge(b, c, sample)
            && covered_by_edge(c, a, sample)
            && covered_by_edge(a, b, sample)
    })
}

fn line_parameter_and_distance_squared(a: Vertex, b: Vertex, x: f32, y: f32) -> (f32, f32) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f32::EPSILON {
        return (0.0, (x - a.x).powi(2) + (y - a.y).powi(2));
    }
    let parameter = ((x - a.x) * dx + (y - a.y) * dy) / length_squared;
    let closest_x = a.x + parameter * dx;
    let closest_y = a.y + parameter * dy;
    (parameter, (x - closest_x).powi(2) + (y - closest_y).powi(2))
}

fn line_pixel_coverage(line: &Line, scissor: ScissorRect, x: i32, y: i32) -> CoverageMask {
    if !scissor.line_enabled(y) {
        return CoverageMask::default();
    }
    let [a, b] = line.v;
    let radius_squared = (line.width * 0.5).powi(2);
    let point_line = (b.x - a.x).abs() <= f32::EPSILON && (b.y - a.y).abs() <= f32::EPSILON;
    CoverageMask::from_samples(|offset_x, offset_y| {
        let sample_x = x as f32 + offset_x as f32 / 8.0;
        let sample_y = y as f32 + offset_y as f32 / 8.0;
        if sample_x < scissor.ulx
            || sample_x >= scissor.lrx
            || sample_y < scissor.uly
            || sample_y >= scissor.lry
        {
            return false;
        }
        let (parameter, distance_squared) =
            line_parameter_and_distance_squared(a, b, sample_x, sample_y);
        (point_line || (0.0..=1.0).contains(&parameter)) && distance_squared <= radius_squared
    })
}

/// Test-only evidence channel for sample-identity-sensitive integration
/// vectors outside this module. The production framebuffer intentionally
/// stores only coverage population, so these assertions must inspect the mask
/// before that boundary.
#[cfg(test)]
pub(crate) fn test_triangle_attribute_sample(
    vertices: [Vertex; 3],
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> (u8, Option<(u8, i32, i32)>) {
    let area = edge(vertices[0], vertices[1], vertices[2]);
    let mask = triangle_pixel_coverage(vertices, area, scissor, x, y);
    let sample = if mask.0 == 0 {
        None
    } else {
        match mask.attribute_sample() {
            AttributeSamplePoint::PixelCenter => None,
            AttributeSamplePoint::Covered(sample) => {
                Some((sample.sample_index, sample.x_eighth, sample.y_eighth))
            }
        }
    };
    (mask.0, sample)
}

#[cfg(test)]
pub(crate) fn test_raw_attribute_sample(
    edge: crate::gbi::RdpEdgeCoefficients,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> (u8, Option<(u8, i32, i32)>) {
    let mask = raw_pixel_coverage(edge, scissor, x, y);
    let sample = if mask.0 == 0 {
        None
    } else {
        match mask.attribute_sample() {
            AttributeSamplePoint::PixelCenter => None,
            AttributeSamplePoint::Covered(sample) => {
                Some((sample.sample_index, sample.x_eighth, sample.y_eighth))
            }
        }
    };
    (mask.0, sample)
}

impl Framebuffer {
    pub const DEFAULT_NOISE_SEED: u64 = 0x4e36_3452_4450_4e53;

    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
            coverage: vec![Coverage::FULL; (width * height) as usize],
            depth: vec![f32::INFINITY; (width * height) as usize],
            encoded_depth: vec![None; (width * height) as usize],
            primitive_depth: None,
            color_layout: ColorImageLayout::Rgba16,
            noise: NoiseState::default(),
        }
    }

    /// Select the deterministic reference stream used for all RDP noise
    /// inputs. This is a reproducibility policy, not the unpublished hardware
    /// seed or generator.
    pub fn set_noise_seed(&mut self, seed: u64) {
        self.noise.reseed(seed);
    }

    pub(crate) fn resized(&self, width: u32, height: u32) -> Self {
        let mut resized = Self::new(width, height);
        resized.noise = self.noise;
        resized
    }

    #[cfg(test)]
    pub(crate) fn noise_position(&self) -> (u64, u64) {
        (self.noise.seed, self.noise.fragment_index)
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, a]);
        }
        self.coverage.fill(Coverage::FULL);
        for d in self.depth.iter_mut() {
            *d = f32::INFINITY;
        }
        self.encoded_depth.fill(None);
    }

    pub(crate) fn set_primitive_depth(&mut self, primitive_depth: Option<PrimitiveDepth>) {
        self.primitive_depth = primitive_depth;
    }

    pub(crate) fn set_color_layout(&mut self, color_layout: ColorImageLayout) {
        self.color_layout = color_layout;
    }

    pub(crate) fn color_layout(&self) -> ColorImageLayout {
        self.color_layout
    }

    pub(crate) fn coverage_count(&self, pixel: usize) -> u8 {
        self.coverage[pixel].count()
    }

    /// True if any pixel differs from a uniform `(r,g,b,a)` fill -- the
    /// honest "did this frame actually render geometry, not just a clear"
    /// check the task requires (`first_frame`'s whole point).
    pub fn has_non_uniform_content(&self, r: u8, g: u8, b: u8, a: u8) -> bool {
        self.pixels.chunks_exact(4).any(|px| px != [r, g, b, a])
    }

    /// Execute an RDP rectangle against the active public 8-bit, RGBA16, or
    /// RGBA32 color-image format. Fill cycle bypasses the pixel pipeline and
    /// includes the lower/right edge; one/two-cycle mode uses the combiner,
    /// blender, and exclusive lower/right edge.
    pub fn draw_fill_rectangle(&mut self, rect: &FillRectangle, target: ColorImage) {
        let layout = target
            .layout()
            .expect("fill target must be I8/CI8, RGBA16, or RGBA32");
        self.color_layout = layout;
        if matches!(rect.cycle_type, CycleType::OneCycle | CycleType::TwoCycle) {
            self.draw_combined_fill_rectangle(rect);
            return;
        }
        assert_eq!(
            rect.cycle_type,
            CycleType::Fill,
            "G_FILLRECT in copy cycle is not implemented"
        );
        let decode_16 = |pixel: u16| {
            let expand = |value: u16| -> u8 {
                let value = value as u8;
                (value << 3) | (value >> 2)
            };
            [
                expand((pixel >> 11) & 0x1f),
                expand((pixel >> 6) & 0x1f),
                expand((pixel >> 1) & 0x1f),
                if pixel & 1 != 0 { 255 } else { 0 },
            ]
        };
        let (colors, coverages, period) = match layout {
            ColorImageLayout::Index8 => {
                let bytes = rect.fill_color.to_be_bytes();
                (
                    bytes.map(|intensity| [intensity, intensity, intensity, 255]),
                    [Coverage::FULL; 4],
                    4,
                )
            }
            ColorImageLayout::Rgba16 => (
                [
                    decode_16((rect.fill_color >> 16) as u16),
                    decode_16(rect.fill_color as u16),
                    decode_16((rect.fill_color >> 16) as u16),
                    decode_16(rect.fill_color as u16),
                ],
                [
                    if (rect.fill_color >> 16) as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if rect.fill_color as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if (rect.fill_color >> 16) as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if rect.fill_color as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                ],
                2,
            ),
            ColorImageLayout::Rgba32 => {
                let [red, green, blue, alpha_coverage] = rect.fill_color.to_be_bytes();
                let coverage = Coverage::from_stored(alpha_coverage >> 5);
                let alpha = (alpha_coverage & 0x1f) << 3 | (alpha_coverage & 0x1f) >> 2;
                let color = [red, green, blue, alpha];
                ([color; 4], [coverage; 4], 1)
            }
        };
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let clip_min_x = (scissor.ulx - 0.5).ceil() as i32;
        let clip_max_x = (scissor.lrx - 0.5).ceil() as i32;
        let clip_min_y = (scissor.uly - 0.5).ceil() as i32;
        let clip_max_y = (scissor.lry - 0.5).ceil() as i32;
        let min_x = (rect.ulx.ceil() as i32).max(clip_min_x).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(clip_max_x - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.ceil() as i32).max(clip_min_y).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(clip_max_y - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let index = (y as u32 * self.width + x as u32) as usize * 4;
                let fill_index = (x as usize) % period;
                self.pixels[index..index + 4].copy_from_slice(&colors[fill_index]);
                self.coverage[index / 4] = coverages[fill_index];
            }
        }
    }

    fn draw_combined_fill_rectangle(&mut self, rect: &FillRectangle) {
        require_supported_alpha_compare(rect.other_mode, "combined G_FILLRECT");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let min_x = pixel_min(rect.ulx).max(pixel_min(scissor.ulx)).max(0);
        let max_x = (pixel_min(rect.lrx) - 1)
            .min(pixel_min(scissor.lrx) - 1)
            .min(self.width as i32 - 1);
        let min_y = pixel_min(rect.uly).max(pixel_min(scissor.uly)).max(0);
        let max_y = (pixel_min(rect.lry) - 1)
            .min(pixel_min(scissor.lry) - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        let depth = DepthControl::from_other_mode(rect.other_mode);

        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let noise = self.noise.next_sample();
                let rgba = evaluate_combiner(
                    rect.combiner,
                    rect.cycle_type,
                    rect.other_mode.combine_key(),
                    CombinerPixel {
                        lod_fraction: 0.0,
                        shade: [0; 4],
                        texel0: [0; 4],
                        texel1: [0; 4],
                        noise,
                    },
                );
                let (rgba, coverage) = apply_coverage_alpha(rect.other_mode, rgba, Coverage::FULL);
                if coverage.count() == 0
                    || !alpha_compare_value(
                        rect.other_mode.alpha_compare(),
                        rgba[3],
                        rect.other_mode.blend_color_alpha,
                        noise,
                    )
                {
                    continue;
                }
                if depth.compare || depth.update {
                    let primitive = self.primitive_depth.expect(
                        "depth-enabled fill rectangle selected primitive Z without G_SETPRIMDEPTH",
                    );
                    let encoded =
                        crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
                    self.set_depth_controlled_blended(
                        x,
                        y,
                        DepthFragment {
                            z: (u32::from(primitive.z & 0x7fff) << 3) as f32,
                            delta_z: primitive.delta_z,
                            encoded_depth: Some(encoded),
                            coverage,
                            rgba,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        depth,
                        rect.other_mode,
                    );
                } else {
                    self.set_blended(
                        x,
                        y,
                        ColorFragment {
                            rgba,
                            coverage,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        rect.other_mode,
                    );
                }
            }
        }
    }

    /// Clear software depth samples under a fill directed at the depth image.
    /// The coverage calculation intentionally mirrors `draw_fill_rectangle`.
    pub fn clear_depth_rectangle(&mut self, rect: &FillRectangle) {
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let clip_min_x = (scissor.ulx - 0.5).ceil() as i32;
        let clip_max_x = (scissor.lrx - 0.5).ceil() as i32;
        let clip_min_y = (scissor.uly - 0.5).ceil() as i32;
        let clip_max_y = (scissor.lry - 0.5).ceil() as i32;
        let min_x = (rect.ulx.ceil() as i32).max(clip_min_x).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(clip_max_x - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.ceil() as i32).max(clip_min_y).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(clip_max_y - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let encoded = [
            EncodedDepth::from_fill_halfword((rect.fill_color >> 16) as u16),
            EncodedDepth::from_fill_halfword(rect.fill_color as u16),
        ];
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let index = (y as u32 * self.width + x as u32) as usize;
                let sample = encoded[(x as usize) & 1];
                self.depth[index] = crate::depth::unpack(sample).0 as f32;
                self.encoded_depth[index] = Some(sample);
            }
        }
    }

    /// Execute the public GBI copy-cycle texture-rectangle path. Copy mode
    /// includes the lower/right bounds and emits four horizontal texels per
    /// clock, so raw `dsdx = 4<<10` advances one texel per output pixel.
    pub fn draw_copy_texture_rectangle(&mut self, rect: &TextureRectangle) {
        assert_eq!(rect.other_mode.cycle_type(), CycleType::Copy);
        require_supported_alpha_compare(rect.other_mode, "copy-cycle G_TEXRECT");
        let texture = rect
            .texture
            .as_ref()
            .expect("copy texture rectangle reached rasterizer without its tile texture");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        // Copy mode ignores the two screen-coordinate fraction bits. Its
        // lower/right pixel is included; scissor lower/right remains an
        // exclusive boundary after the caller has checked the documented
        // four-pixel copy-mode restriction.
        let min_x = (rect.ulx.floor() as i32).max(scissor.ulx as i32).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(scissor.lrx as i32 - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.floor() as i32).max(scissor.uly as i32).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(scissor.lry as i32 - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let origin_x = rect.ulx.floor();
        let origin_y = rect.uly.floor();
        let ds_per_pixel = rect.dsdx as f32 / 4096.0;
        let dt_per_pixel = rect.dtdy as f32 / 1024.0;
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let dx = x as f32 - origin_x;
                let dy = y as f32 - origin_y;
                // Public gSPTextureRectangleFlip swaps the screen axes that
                // advance S and T. Copy mode still applies its documented
                // four-texel dsdx encoding, so normalize each field exactly
                // as the non-flipped path before swapping the axes.
                let (s, t) = if rect.flip {
                    (rect.s + dy * ds_per_pixel, rect.t + dx * dt_per_pixel)
                } else {
                    (rect.s + dx * ds_per_pixel, rect.t + dy * dt_per_pixel)
                };
                let sample = texture.sample_copy(s, t);
                let texel = sample.rgba;
                let noise = self.noise.next_sample();
                if !copy_alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    texture,
                    texel[3],
                    rect.other_mode.blend_color_alpha,
                    noise,
                ) {
                    continue;
                }
                let index = (y as u32 * self.width + x as u32) as usize * 4;
                if self.color_layout == ColorImageLayout::Index8 {
                    // Programming Manual 13.11 and 15.5 define copy as a
                    // direct 8-bit memory transfer after source-format alpha
                    // comparison. In particular, IA8 must retain both packed
                    // nibbles rather than store its expanded intensity lane.
                    let byte = sample.direct_8bit.unwrap_or_else(|| {
                        panic!(
                            "copy-cycle 8-bit target reached rasterizer without a direct source byte (format={} size={})",
                            texture.format, texture.size
                        )
                    });
                    self.pixels[index..index + 4].copy_from_slice(&[byte, byte, byte, texel[3]]);
                } else {
                    self.pixels[index..index + 4].copy_from_slice(&texel);
                }
                self.coverage[index / 4] = Coverage::FULL;
            }
        }
    }

    /// Execute a one/two-cycle texture rectangle through the shared texture
    /// filter, color combiner, alpha compare, and framebuffer blender. The
    /// public command excludes its lower/right edge in these cycle modes;
    /// `G_TEXRECTFLIP` swaps the screen axes driven by S and T.
    pub fn draw_texture_rectangle(&mut self, rect: &TextureRectangle) {
        let cycle_type = rect.other_mode.cycle_type();
        assert!(matches!(
            cycle_type,
            CycleType::OneCycle | CycleType::TwoCycle
        ));
        require_supported_alpha_compare(rect.other_mode, "combined G_TEXRECT");
        let texture0 = rect
            .texture
            .as_ref()
            .expect("texture rectangle reached rasterizer without TEXEL0 tile");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let min_x = pixel_min(rect.ulx).max(pixel_min(scissor.ulx)).max(0);
        let max_x = (pixel_min(rect.lrx) - 1)
            .min(pixel_min(scissor.lrx) - 1)
            .min(self.width as i32 - 1);
        let min_y = pixel_min(rect.uly).max(pixel_min(scissor.uly)).max(0);
        let max_y = (pixel_min(rect.lry) - 1)
            .min(pixel_min(scissor.lry) - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        let origin_x = rect.ulx.floor();
        let origin_y = rect.uly.floor();
        let ds = rect.dsdx as f32 / 1024.0;
        let dt = rect.dtdy as f32 / 1024.0;
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let dx = x as f32 - origin_x;
                let dy = y as f32 - origin_y;
                let (s, t) = if rect.flip {
                    (rect.s + dy * ds, rect.t + dx * dt)
                } else {
                    (rect.s + dx * ds, rect.t + dy * dt)
                };
                let derivatives = if rect.flip {
                    TextureDerivatives {
                        dtdx: dt,
                        dsdy: ds,
                        ..TextureDerivatives::default()
                    }
                } else {
                    TextureDerivatives {
                        dsdx: ds,
                        dtdy: dt,
                        ..TextureDerivatives::default()
                    }
                };
                let (texel0, texel1, lod_fraction) = texture0.sample_rdp_pair(
                    rect.texture1.as_ref(),
                    TextureSampleRequest {
                        s,
                        t,
                        derivatives,
                        other_mode: rect.other_mode,
                        convert: rect.combiner.convert,
                        min_level: rect.combiner.min_lod_level,
                        require_texel1: rect.combiner.mode.uses_texel1(cycle_type),
                    },
                );
                // Rectangle commands carry no shade attributes. Validation
                // rejects programs selecting SHADE, so zero is an inert and
                // observable placeholder rather than an invented constant.
                let shade = [0; 4];
                let noise = self.noise.next_sample();
                let rgba = evaluate_combiner(
                    rect.combiner,
                    cycle_type,
                    rect.other_mode.combine_key(),
                    CombinerPixel {
                        lod_fraction,
                        shade,
                        texel0,
                        texel1,
                        noise,
                    },
                );
                let (rgba, coverage) = apply_coverage_alpha(rect.other_mode, rgba, Coverage::FULL);
                if coverage.count() == 0 {
                    continue;
                }
                if !alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    rgba[3],
                    rect.other_mode.blend_color_alpha,
                    noise,
                ) {
                    continue;
                }
                let depth = DepthControl::from_other_mode(rect.other_mode);
                if depth.compare || depth.update {
                    let primitive = self.primitive_depth.expect(
                        "depth-enabled texture rectangle selected primitive Z without G_SETPRIMDEPTH",
                    );
                    let encoded =
                        crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
                    self.set_depth_controlled_blended(
                        x,
                        y,
                        DepthFragment {
                            z: (u32::from(primitive.z & 0x7fff) << 3) as f32,
                            delta_z: primitive.delta_z,
                            encoded_depth: Some(encoded),
                            coverage,
                            rgba,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        depth,
                        rect.other_mode,
                    );
                } else {
                    self.set_blended(
                        x,
                        y,
                        ColorFragment {
                            rgba,
                            coverage,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        rect.other_mode,
                    );
                }
            }
        }
    }

    fn set_blended(
        &mut self,
        x: i32,
        y: i32,
        fragment: ColorFragment,
        blender: BlenderState,
        other_mode: OtherMode,
    ) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        let result = coverage_result(fragment.coverage, self.coverage[pix], other_mode);
        self.coverage[pix] = result.destination;
        if other_mode.clear_on_coverage() && !result.wraps {
            return false;
        }
        let idx = pix * 4;
        let dst = self.pixels[idx..idx + 4].try_into().unwrap();
        let mut rgba = fragment.rgba;
        rgba[3] = apply_alpha_dither(
            rgba[3],
            other_mode.alpha_dither(),
            other_mode.rgb_dither(),
            x,
            y,
            fragment.noise,
        );
        let out = blend_fragment(
            rgba,
            dst,
            fragment.shade_alpha,
            blender,
            result.blend_enabled,
            result.memory,
        );
        let out = apply_rgb_dither(out, other_mode.rgb_dither(), x, y, fragment.noise);
        self.pixels[idx..idx + 4].copy_from_slice(&out);
        true
    }

    /// Depth-tested pixel write: pass iff `z` is strictly nearer (less than)
    /// the stored depth. On pass, write the color AND the new depth. This is
    /// the standard "less-than passes, nearer wins" z-compare
    /// (`F3DEX2-CONCEPTS.md` §4.3). Returns whether the write happened (used
    /// only by tests to assert occlusion behavior).
    #[cfg(test)]
    fn set_depth_tested(&mut self, x: i32, y: i32, z: f32, rgba: [u8; 4]) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        if z < self.depth[pix] {
            self.depth[pix] = z;
            self.pixels[pix * 4..pix * 4 + 4].copy_from_slice(&rgba);
            #[cfg(not(test))]
            zstat::note_pass();
            true
        } else {
            // A farther (or equal) fragment landed on an already-written
            // pixel and was correctly discarded -- the actual occlusion work
            // the z-buffer does. Counted (env-gated) to PROVE, on a real
            // overlapping frame, that depth is doing meaningful rejection and
            // not a no-op. See `FN64_DUMP_PROJ` in gbi.rs.
            #[cfg(not(test))]
            zstat::note_reject();
            false
        }
    }

    fn set_depth_controlled_blended(
        &mut self,
        x: i32,
        y: i32,
        fragment: DepthFragment,
        blender: BlenderState,
        depth: DepthControl,
        other_mode: OtherMode,
    ) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        let coverage = coverage_result(fragment.coverage, self.coverage[pix], other_mode);
        let passes_depth = if !depth.compare {
            true
        } else {
            let (memory_z, memory_encoded_delta_z) = self.encoded_depth[pix].map_or_else(
                || (self.depth[pix].clamp(0.0, 0x3ffff as f32).round() as u32, 0),
                crate::depth::unpack,
            );
            let relations = crate::depth::relations(
                fragment.z.clamp(0.0, 0x3ffff as f32).round() as u32,
                fragment.delta_z,
                memory_z,
                memory_encoded_delta_z,
            );
            match depth_coverage_decision(depth.mode, relations, coverage.wraps) {
                DepthCoverageDecision::Pass => true,
                DepthCoverageDecision::Reject => false,
                DepthCoverageDecision::UnsupportedInterpenetratingCoverageAdjustment => {
                    crate::render_unsupported_panic(
                        "render.reference.raster.interpenetration-coverage-adjustment",
                        format!(
                            "ZMODE_INTER coverage wrap requires unsupported interpenetration \
                             coverage adjustment: pixel_coverage={} memory_coverage={} \
                             depth_relations={relations:?}",
                            coverage.pixel.count(),
                            coverage.memory.count(),
                        ),
                    )
                }
            }
        };
        if passes_depth {
            self.coverage[pix] = coverage.destination;
            if other_mode.clear_on_coverage() && !coverage.wraps {
                return false;
            }
            let idx = pix * 4;
            let dst = self.pixels[idx..idx + 4].try_into().unwrap();
            let mut rgba = fragment.rgba;
            rgba[3] = apply_alpha_dither(
                rgba[3],
                other_mode.alpha_dither(),
                other_mode.rgb_dither(),
                x,
                y,
                fragment.noise,
            );
            let out = blend_fragment(
                rgba,
                dst,
                fragment.shade_alpha,
                blender,
                coverage.blend_enabled,
                coverage.memory,
            );
            let out = apply_rgb_dither(out, other_mode.rgb_dither(), x, y, fragment.noise);
            // The fragment pipeline is combiner -> alpha compare -> depth
            // test -> blend -> write. Keep both writes after compositing so
            // a rejected fragment cannot mutate either target.
            if depth.update {
                self.depth[pix] = fragment
                    .encoded_depth
                    .map_or(fragment.z, |encoded| crate::depth::unpack(encoded).0 as f32);
                self.encoded_depth[pix] = fragment.encoded_depth;
            }
            self.pixels[idx..idx + 4].copy_from_slice(&out);
            #[cfg(not(test))]
            zstat::note_pass();
            true
        } else {
            #[cfg(not(test))]
            zstat::note_reject();
            false
        }
    }

    /// Rasterize one flat/interpolated-color triangle with no culling and no
    /// depth test -- the original textbook edge-function (Pineda 1988-style)
    /// scan, kept for the depth-free reference/fixture path and tests that
    /// assert pure 2D fill. `draw_triangle_culled` layers culling + z-test on
    /// top for the real F3DEX2 scene path.
    pub fn draw_triangle(&mut self, tri: &Triangle) {
        self.draw_triangle_impl(tri, CullMode::None, DepthControl::DISABLED);
    }

    /// Rasterize with F3DEX2 back/front-face culling (by screen-space signed
    /// area / winding, `F3DEX2-CONCEPTS.md` §2.4/§4.2) and z-buffering
    /// (§4.3). This is the path the real OoT scene uses so far geometry is
    /// occluded correctly and inside-out back faces don't overpaint front
    /// faces.
    pub fn draw_triangle_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, DepthControl::from_other_mode(tri.other_mode));
    }

    /// Same culling as [`draw_triangle_culled`] but with NO depth test
    /// (submission/painter's order). Used only by the `FN64_NO_DEPTH` A/B
    /// instrumentation to prove that correct occlusion comes from the
    /// z-buffer, not draw order.
    pub fn draw_triangle_no_depth_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, DepthControl::DISABLED);
    }

    /// Rasterize an F3DEX2/L3DEX line with the public width, shade, texture,
    /// scissor, blender, and read-only depth contract.
    pub fn draw_line(&mut self, line: &Line) {
        self.draw_line_impl(line, DepthControl::for_line(line.other_mode));
    }

    pub fn draw_line_no_depth(&mut self, line: &Line) {
        self.draw_line_impl(line, DepthControl::DISABLED);
    }

    /// Rasterize a raw RDP triangle directly from its edge and attribute
    /// planes. SGI *RDP Command Summary* Tables 12-15 define the major edge,
    /// upper/lower minor edges, and the `d/de` plus `d/dx` coefficient groups.
    /// The public 4x4 checkerboard mask supplies eight coverage samples per
    /// pixel, retained as a typed identity mask until the fragment boundary.
    /// Full-coverage attributes retain pixel-center evaluation. Partial masks
    /// use the shared typed on-primitive sample policy; the unpublished
    /// silicon lookup and fixed-width accumulator truncation remain separate
    /// fidelity work.
    pub fn draw_raw_rdp_triangle(&mut self, triangle: &RawRdpTriangle) {
        self.draw_raw_rdp_triangle_impl(
            triangle,
            DepthControl::from_other_mode(triangle.other_mode),
        );
    }

    pub fn draw_raw_rdp_triangle_no_depth(&mut self, triangle: &RawRdpTriangle) {
        self.draw_raw_rdp_triangle_impl(triangle, DepthControl::DISABLED);
    }

    fn draw_raw_rdp_triangle_impl(&mut self, triangle: &RawRdpTriangle, depth: DepthControl) {
        require_supported_alpha_compare(triangle.other_mode, "raw RDP triangle");
        let edge = triangle.edge;
        let yh_eighth = i32::from(edge.yh) * 2;
        let yl_eighth = i32::from(edge.yl) * 2;
        let high_origin_eighth = i32::from(edge.yh & !3) * 2;
        let scissor = triangle
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let scissor_ulx_eighth = (scissor.ulx * 8.0).round() as i32;
        let scissor_uly_eighth = (scissor.uly * 8.0).round() as i32;
        let scissor_lrx_eighth = (scissor.lrx * 8.0).round() as i32;
        let scissor_lry_eighth = (scissor.lry * 8.0).round() as i32;
        let min_y = (ceil_ratio(i64::from(yh_eighth - 7), 8) as i32)
            .max(ceil_ratio(i64::from(scissor_uly_eighth - 7), 8) as i32)
            .clamp(0, self.height as i32);
        let max_y = (ceil_ratio(i64::from(yl_eighth - 1), 8) as i32)
            .min(ceil_ratio(i64::from(scissor_lry_eighth - 1), 8) as i32)
            .clamp(0, self.height as i32);
        for y in min_y..max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            let mut min_left = i64::MAX;
            let mut max_right = i64::MIN;
            for offset_y in [1, 3, 5, 7] {
                let row_y_eighth = y * 8 + offset_y;
                if row_y_eighth < yh_eighth
                    || row_y_eighth >= yl_eighth
                    || row_y_eighth < scissor_uly_eighth
                    || row_y_eighth >= scissor_lry_eighth
                {
                    continue;
                }
                let (left_x, right_x) = raw_span_edges_at_y_eighth(edge, row_y_eighth);
                if right_x > left_x {
                    min_left = min_left.min(left_x);
                    max_right = max_right.max(right_x);
                }
            }
            if min_left == i64::MAX || max_right == i64::MIN {
                continue;
            }
            let min_x = (ceil_ratio(min_left - 7 * Q16_ONE / 8, Q16_ONE) as i32)
                .max(ceil_ratio(i64::from(scissor_ulx_eighth - 7), 8) as i32)
                .clamp(0, self.width as i32);
            let max_x = (ceil_ratio(max_right - Q16_ONE / 8, Q16_ONE) as i32)
                .min(ceil_ratio(i64::from(scissor_lrx_eighth - 1), 8) as i32)
                .clamp(0, self.width as i32);

            for x in min_x..max_x {
                let coverage_mask = raw_pixel_coverage(edge, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let attribute_sample = coverage_mask.attribute_sample();
                let (sample_x_eighth, sample_y_eighth) = attribute_sample.offsets_eighth();
                let sample_y_eighth = y * 8 + sample_y_eighth;
                let edge_delta_y_eighth = sample_y_eighth - high_origin_eighth;
                let major_x = i64::from(edge.xh)
                    + fixed_mul_ratio(edge.dxhdy, i64::from(edge_delta_y_eighth), 8);
                let sample_x = i64::from(x) * Q16_ONE + i64::from(sample_x_eighth) * Q16_ONE / 8;
                let edge_delta_x = sample_x - major_x;
                let plane = |base: i32, dx: i32, de: i32| {
                    raw_attribute_plane(base, dx, de, edge_delta_y_eighth, edge_delta_x)
                };
                let shade = triangle.shade.map_or(triangle.combiner.primitive, |shade| {
                    std::array::from_fn(|component| {
                        let value = plane(
                            shade.color[component],
                            shade.dcdx[component],
                            shade.dcde[component],
                        )
                        .div_euclid(Q16_ONE)
                        .clamp(0, 255);
                        value as u8
                    })
                });
                let (texel0, texel1, lod_fraction) = if let Some(coefficients) =
                    triangle.texture_coefficients
                {
                    let stw = std::array::from_fn::<_, 3, _>(|component| {
                        plane(
                            coefficients.stw[component],
                            coefficients.dstdx[component],
                            coefficients.dstde[component],
                        )
                    });
                    assert!(
                        stw[2] > 0,
                        "raw RDP textured triangle tile {} produced non-positive W reciprocal {} at ({x}, {y})",
                        edge.tile,
                        stw[2]
                    );
                    let corrected = |values: [i64; 3]| {
                        assert!(
                            values[2] > 0,
                            "raw RDP LOD derivative produced non-positive W reciprocal {} at ({x}, {y})",
                            values[2]
                        );
                        (
                            values[0] as f32 / values[2] as f32,
                            values[1] as f32 / values[2] as f32,
                        )
                    };
                    let (s, t) = corrected(stw);
                    let next_x = std::array::from_fn(|component| {
                        stw[component] + i64::from(coefficients.dstdx[component])
                    });
                    let next_y = std::array::from_fn(|component| {
                        stw[component] + i64::from(coefficients.dstdy[component])
                    });
                    let (sx, tx) = corrected(next_x);
                    let (sy, ty) = corrected(next_y);
                    triangle
                        .texture
                        .as_ref()
                        .expect("validated raw RDP texture disappeared before rasterization")
                        .sample_rdp_pair(
                            None,
                            TextureSampleRequest {
                                s,
                                t,
                                derivatives: TextureDerivatives {
                                    dsdx: sx - s,
                                    dtdx: tx - t,
                                    dsdy: sy - s,
                                    dtdy: ty - t,
                                },
                                other_mode: triangle.other_mode,
                                convert: triangle.combiner.convert,
                                min_level: triangle.combiner.min_lod_level,
                                require_texel1: triangle
                                    .combiner
                                    .mode
                                    .uses_texel1(triangle.other_mode.cycle_type()),
                            },
                        )
                } else {
                    ([255; 4], [255; 4], 0.0)
                };
                let (z, delta_z, encoded_depth) = triangle.z.map_or((0.0, 0, None), |z| {
                    // Nintendo 64 Programming Manual, "Z Stepper": command
                    // Z is 16.16 while the blender compares unsigned 15.3;
                    // near is zero and far is G_MAXZ. Clamp to that documented
                    // 18-bit working range after the eightfold conversion.
                    let working_z = round_ratio(
                        i128::from(plane(z.z, z.dzdx, z.dzde)) * 8,
                        i128::from(Q16_ONE),
                    )
                    .clamp(0, 0x3ffff) as u32;
                    // Nintendo 64 Programming Manual, Chapter 16, Equation 4:
                    // DeltaZpix = |dZ/dx| + |dZ/dy|. Command derivatives are
                    // 16.16 and convert to the same 15.3 working domain as Z.
                    let delta_z = round_ratio(
                        (i128::from(z.dzdx).abs() + i128::from(z.dzdy).abs()) * 8,
                        i128::from(Q16_ONE),
                    )
                    .clamp(0, i128::from(u16::MAX)) as u16;
                    let encoded = crate::depth::pack(working_z, delta_z);
                    (working_z as f32, delta_z, Some(encoded))
                });
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth,
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: triangle.other_mode,
                        combiner: triangle.combiner,
                        blender: triangle.blender,
                        depth,
                    },
                );
            }
        }
    }

    fn write_combined_fragment(
        &mut self,
        x: i32,
        y: i32,
        fragment: FragmentInputs,
        pipeline: FragmentPipeline,
    ) -> bool {
        let mut fragment = fragment;
        if fragment.coverage.count() == 0 {
            return false;
        }
        if pipeline.other_mode.primitive_depth_source()
            && (pipeline.depth.compare || pipeline.depth.update)
        {
            let primitive = self
                .primitive_depth
                .expect("depth-enabled primitive selected primitive Z without G_SETPRIMDEPTH");
            let encoded =
                crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
            fragment.z = (u32::from(primitive.z & 0x7fff) << 3) as f32;
            fragment.delta_z = primitive.delta_z;
            fragment.encoded_depth = Some(encoded);
        }
        let noise = self.noise.next_sample();
        let rgba = evaluate_combiner(
            pipeline.combiner,
            pipeline.other_mode.cycle_type(),
            pipeline.other_mode.combine_key(),
            CombinerPixel {
                lod_fraction: fragment.lod_fraction,
                shade: fragment.shade,
                texel0: fragment.texel0,
                texel1: fragment.texel1,
                noise,
            },
        );
        let (rgba, coverage) = apply_coverage_alpha(pipeline.other_mode, rgba, fragment.coverage);
        if coverage.count() == 0 {
            return false;
        }
        if !alpha_compare_value(
            pipeline.other_mode.alpha_compare(),
            rgba[3],
            pipeline.other_mode.blend_color_alpha,
            noise,
        ) {
            return false;
        }
        if pipeline.depth.compare || pipeline.depth.update {
            self.set_depth_controlled_blended(
                x,
                y,
                DepthFragment {
                    z: fragment.z,
                    delta_z: fragment.delta_z,
                    encoded_depth: fragment.encoded_depth,
                    coverage,
                    rgba,
                    shade_alpha: fragment.shade[3],
                    noise,
                },
                pipeline.blender,
                pipeline.depth,
                pipeline.other_mode,
            )
        } else {
            self.set_blended(
                x,
                y,
                ColorFragment {
                    rgba,
                    coverage,
                    shade_alpha: fragment.shade[3],
                    noise,
                },
                pipeline.blender,
                pipeline.other_mode,
            )
        }
    }

    fn draw_triangle_impl(&mut self, tri: &Triangle, cull: CullMode, depth: DepthControl) {
        require_supported_alpha_compare(tri.other_mode, "F3DEX2 triangle");
        let [a, b, c] = tri.v;
        if tri.texture.is_some() {
            assert!(
                [a.w, b.w, c.w].iter().all(|&w| w > 1e-4),
                "textured triangle reached perspective interpolation with non-positive clip w; \
                 F3DEX2 decode must near-plane-cull it before rasterization"
            );
        }
        #[cfg(not(test))]
        let ignore_scissor = std::env::var_os("FN64_DIAG_IGNORE_SCISSOR").is_some();
        #[cfg(test)]
        let ignore_scissor = false;
        let scissor = (!ignore_scissor)
            .then_some(tri.scissor)
            .flatten()
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        // Candidate bounds are deliberately one pixel wider than the vertex
        // and scissor extrema. Coverage samples range from 1/8 through 7/8,
        // so a pixel center outside either bound can still contain selected
        // samples. The mask below performs the exact rejection.
        let clip_min_x = scissor.ulx.floor() as i32 - 1;
        let clip_max_x = scissor.lrx.ceil() as i32 + 1;
        let clip_min_y = scissor.uly.floor() as i32 - 1;
        let clip_max_y = scissor.lry.ceil() as i32 + 1;
        let min_x = (a.x.min(b.x).min(c.x).floor() as i32 - 1)
            .max(clip_min_x)
            .clamp(0, self.width as i32);
        let max_x = (a.x.max(b.x).max(c.x).ceil() as i32 + 1)
            .min(clip_max_x)
            .clamp(0, self.width as i32);
        let min_y = (a.y.min(b.y).min(c.y).floor() as i32 - 1)
            .max(clip_min_y)
            .clamp(0, self.height as i32);
        let max_y = (a.y.max(b.y).max(c.y).ceil() as i32 + 1)
            .min(clip_max_y)
            .clamp(0, self.height as i32);

        let area = edge(a, b, c);
        if area == 0.0 {
            return; // degenerate triangle: zero screen-space area.
        }

        // Back/front-face cull by the sign of the screen-space signed area.
        // N64 screen Y is top-down (see project_vertex's Y-flip), which makes
        // a front-facing (CCW-in-model) triangle come out with a NEGATIVE
        // signed area under this `edge` convention; that is the "front" sign
        // here, so `G_CULL_BACK` drops POSITIVE-area triangles. If culling
        // ever removes the wrong half, this sign is the knob (§2.4).
        let culled = match cull {
            CullMode::None => false,
            CullMode::Back => area > 0.0,
            CullMode::Front => area < 0.0,
            CullMode::Both => true,
        };
        if culled {
            return;
        }

        for y in min_y..max_y {
            for x in min_x..max_x {
                let coverage_mask = triangle_pixel_coverage([a, b, c], area, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let attribute_sample = coverage_mask.attribute_sample();
                let (sample_x_eighth, sample_y_eighth) = attribute_sample.offsets_eighth();
                let p = Vertex {
                    x: x as f32 + sample_x_eighth as f32 / 8.0,
                    y: y as f32 + sample_y_eighth as f32 / 8.0,
                    ..Default::default()
                };
                let w0 = edge(b, c, p) / area;
                let w1 = edge(c, a, p) / area;
                let w2 = edge(a, b, p) / area;
                // Interpolated (screen-linear) shade color.
                let shade = [
                    (w0 * a.r as f32 + w1 * b.r as f32 + w2 * c.r as f32) as u8,
                    (w0 * a.g as f32 + w1 * b.g as f32 + w2 * c.g as f32) as u8,
                    (w0 * a.b as f32 + w1 * b.b as f32 + w2 * c.b as f32) as u8,
                    (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8,
                ];
                // Interpolate S/w, T/w, and 1/w, then divide before sampling.
                // Derivatives retain the selected within-pixel offset so the
                // correction translates the plane without changing its
                // adjacent-pixel gradient.
                let (texel0, texel1, lod_fraction) = if let Some(tex) = &tri.texture {
                    #[cfg(not(test))]
                    let affine_texture = std::env::var_os("FN64_DIAG_AFFINE_TEXTURE").is_some();
                    #[cfg(test)]
                    let affine_texture = false;
                    let coordinates_at = |px: f32, py: f32| {
                        let point = Vertex {
                            x: px,
                            y: py,
                            ..Default::default()
                        };
                        let q0 = edge(b, c, point) / area;
                        let q1 = edge(c, a, point) / area;
                        let q2 = edge(a, b, point) / area;
                        if affine_texture {
                            (
                                q0 * a.s + q1 * b.s + q2 * c.s,
                                q0 * a.t + q1 * b.t + q2 * c.t,
                            )
                        } else {
                            let rw0 = q0 / a.w;
                            let rw1 = q1 / b.w;
                            let rw2 = q2 / c.w;
                            let reciprocal_w = rw0 + rw1 + rw2;
                            assert!(
                                reciprocal_w > 0.0,
                                "F3DEX2 texture interpolation produced non-positive reciprocal W"
                            );
                            (
                                (rw0 * a.s + rw1 * b.s + rw2 * c.s) / reciprocal_w,
                                (rw0 * a.t + rw1 * b.t + rw2 * c.t) / reciprocal_w,
                            )
                        }
                    };
                    let (s, t) = coordinates_at(p.x, p.y);
                    let (sx, tx) = coordinates_at(p.x + 1.0, p.y);
                    let (sy, ty) = coordinates_at(p.x, p.y + 1.0);
                    tex.sample_rdp_pair(
                        None,
                        TextureSampleRequest {
                            s,
                            t,
                            derivatives: TextureDerivatives {
                                dsdx: sx - s,
                                dtdx: tx - t,
                                dsdy: sy - s,
                                dtdy: ty - t,
                            },
                            other_mode: tri.other_mode,
                            convert: tri.combiner.convert,
                            min_level: tri.combiner.min_lod_level,
                            require_texel1: tri
                                .combiner
                                .mode
                                .uses_texel1(tri.other_mode.cycle_type()),
                        },
                    )
                } else {
                    ([255; 4], [255; 4], 0.0)
                };
                // Screen-linear depth interpolation remains the F3DEX2
                // approximation. Raw RDP work uses its coefficient plane.
                // F3DEX vertices carry viewport-mapped screen Z. Convert
                // to the RDP blender's unsigned 15.3 working domain so
                // HLE and raw-command samples compare in identical units.
                let z = ((w0 * a.z + w1 * b.z + w2 * c.z) * 8.0).clamp(0.0, 0x3ffff as f32);
                let denominator = edge(a, b, c);
                let dzdx = ((b.z - a.z) * (c.y - a.y) - (c.z - a.z) * (b.y - a.y)) / denominator;
                let dzdy = ((b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z)) / denominator;
                let delta_z = ((dzdx.abs() + dzdy.abs()) * 8.0)
                    .round()
                    .clamp(0.0, u16::MAX as f32) as u16;
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth: Some(crate::depth::pack(z.round() as u32, delta_z)),
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: tri.other_mode,
                        combiner: tri.combiner,
                        blender: tri.blender,
                        depth,
                    },
                );
            }
        }
    }

    fn draw_line_impl(&mut self, line: &Line, depth: DepthControl) {
        require_supported_alpha_compare(line.other_mode, "F3DEX2/L3DEX line");
        let [a, b] = line.v;
        if line.texture.is_some() {
            assert!(
                a.w > 1e-4 && b.w > 1e-4,
                "textured G_LINE3D reached perspective interpolation with non-positive clip w"
            );
        }
        let scissor = line
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let radius = line.width * 0.5;
        let min_x = ((a.x.min(b.x) - radius).floor() as i32 - 1)
            .max(scissor.ulx.floor() as i32 - 1)
            .clamp(0, self.width as i32);
        let max_x = ((a.x.max(b.x) + radius).ceil() as i32 + 1)
            .min(scissor.lrx.ceil() as i32 + 1)
            .clamp(0, self.width as i32);
        let min_y = ((a.y.min(b.y) - radius).floor() as i32 - 1)
            .max(scissor.uly.floor() as i32 - 1)
            .clamp(0, self.height as i32);
        let max_y = ((a.y.max(b.y) + radius).ceil() as i32 + 1)
            .min(scissor.lry.ceil() as i32 + 1)
            .clamp(0, self.height as i32);
        let segment_length = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let delta_z = if segment_length > f32::EPSILON {
            (((b.z - a.z).abs() / segment_length) * 8.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16
        } else {
            0
        };
        let lerp_channel = |start: u8, end: u8, parameter: f32| {
            (f32::from(start) + (f32::from(end) - f32::from(start)) * parameter).clamp(0.0, 255.0)
                as u8
        };
        let parameter_at = |x: f32, y: f32| {
            line_parameter_and_distance_squared(a, b, x, y)
                .0
                .clamp(0.0, 1.0)
        };
        let texture_coordinates_at = |x: f32, y: f32| {
            let parameter = parameter_at(x, y);
            let start_weight = 1.0 - parameter;
            let end_weight = parameter;
            let reciprocal_w = start_weight / a.w + end_weight / b.w;
            assert!(
                reciprocal_w > 0.0,
                "G_LINE3D texture interpolation produced non-positive reciprocal W"
            );
            (
                (start_weight * a.s / a.w + end_weight * b.s / b.w) / reciprocal_w,
                (start_weight * a.t / a.w + end_weight * b.t / b.w) / reciprocal_w,
            )
        };

        for y in min_y..max_y {
            for x in min_x..max_x {
                let coverage_mask = line_pixel_coverage(line, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let center_x = x as f32 + 0.5;
                let center_y = y as f32 + 0.5;
                let parameter = parameter_at(center_x, center_y);
                let shade = if line.smooth_shading {
                    [
                        lerp_channel(a.r, b.r, parameter),
                        lerp_channel(a.g, b.g, parameter),
                        lerp_channel(a.b, b.b, parameter),
                        lerp_channel(a.a, b.a, parameter),
                    ]
                } else {
                    [a.r, a.g, a.b, a.a]
                };
                let (texel0, texel1, lod_fraction) = if let Some(texture) = &line.texture {
                    let (s, t) = texture_coordinates_at(center_x, center_y);
                    let (sx, tx) = texture_coordinates_at(center_x + 1.0, center_y);
                    let (sy, ty) = texture_coordinates_at(center_x, center_y + 1.0);
                    texture.sample_rdp_pair(
                        None,
                        TextureSampleRequest {
                            s,
                            t,
                            derivatives: TextureDerivatives {
                                dsdx: sx - s,
                                dtdx: tx - t,
                                dsdy: sy - s,
                                dtdy: ty - t,
                            },
                            other_mode: line.other_mode,
                            convert: line.combiner.convert,
                            min_level: line.combiner.min_lod_level,
                            require_texel1: line
                                .combiner
                                .mode
                                .uses_texel1(line.other_mode.cycle_type()),
                        },
                    )
                } else {
                    ([255; 4], [255; 4], 0.0)
                };
                let z = ((a.z + (b.z - a.z) * parameter) * 8.0).clamp(0.0, 0x3ffff as f32);
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth: Some(crate::depth::pack(z.round() as u32, delta_z)),
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: line.other_mode,
                        combiner: line.combiner,
                        blender: line.blender,
                        depth,
                    },
                );
            }
        }
    }
}

fn require_supported_alpha_compare(other_mode: OtherMode, primitive: &str) {
    match other_mode.alpha_compare() {
        AlphaCompare::None | AlphaCompare::Threshold | AlphaCompare::Dither => {}
        AlphaCompare::Reserved => {
            panic!("{primitive} selected reserved G_AC alpha-compare mode 2")
        }
    }
}

/// Screen-registered three-bit thresholds for the two ordered RGB modes.
/// Each 4x4 tile contains every threshold 0..=7 twice. The standard Bayer
/// tile maximizes spatial separation; the magic-square tile gives every row
/// and column the same threshold sum for use with the VI dither filter.
fn ordered_rgb_dither_threshold(mode: RgbDither, x: i32, y: i32) -> u8 {
    const MAGIC_SQUARE: [[u8; 4]; 4] = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
    const BAYER: [[u8; 4]; 4] = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
    let row = y.rem_euclid(4) as usize;
    let column = x.rem_euclid(4) as usize;
    match mode {
        RgbDither::MagicSquare => MAGIC_SQUARE[row][column],
        RgbDither::Bayer => BAYER[row][column],
        RgbDither::Noise | RgbDither::Disabled => {
            unreachable!("ordered dither threshold requested for {mode:?}")
        }
    }
}

/// Apply the memory-interface dither decision while retaining an RGBA8888
/// working surface. Programming Manual Chapter 15.5 places this RGB
/// perturbation before the selected color-image format is written and says it
/// remains active for RGBA32 even though that layout does not discard the low
/// bits. Keeping the destination layout out of this function makes that
/// pre-write ordering structural: I8, RGBA16, and RGBA32 all consume the same
/// dithered working color. For RGBA16, a component reaches the next five-bit
/// bucket exactly when its low three bits exceed the selected threshold; the
/// eventual writer performs the common `>> 3` packing.
fn apply_rgb_dither(
    mut rgba: [u8; 4],
    mode: RgbDither,
    x: i32,
    y: i32,
    noise: NoiseSample,
) -> [u8; 4] {
    let threshold = match mode {
        RgbDither::MagicSquare | RgbDither::Bayer => ordered_rgb_dither_threshold(mode, x, y),
        RgbDither::Disabled => return rgba,
        RgbDither::Noise => noise.dither(),
    };
    for component in &mut rgba[..3] {
        if (*component & 7) > threshold {
            *component = (*component & !7).saturating_add(8);
        }
    }
    rgba
}

/// Reduce post-combiner pixel alpha to the blender's five-bit input. Public
/// `gDPSetAlphaDither` defines PATTERN as the selected RGB matrix, with Bayer
/// substituted when RGB dither is disabled and magic square substituted when
/// RGB noise is selected. NOTPATTERN reverses the three-bit threshold.
fn apply_alpha_dither(
    alpha: u8,
    alpha_mode: AlphaDither,
    rgb_mode: RgbDither,
    x: i32,
    y: i32,
    noise: NoiseSample,
) -> u8 {
    let threshold = match alpha_mode {
        AlphaDither::Disabled => return alpha,
        AlphaDither::Noise => noise.dither(),
        AlphaDither::Pattern | AlphaDither::InversePattern => {
            let pattern = match rgb_mode {
                RgbDither::MagicSquare | RgbDither::Bayer => rgb_mode,
                RgbDither::Noise => RgbDither::MagicSquare,
                RgbDither::Disabled => RgbDither::Bayer,
            };
            let threshold = ordered_rgb_dither_threshold(pattern, x, y);
            if alpha_mode == AlphaDither::InversePattern {
                7 - threshold
            } else {
                threshold
            }
        }
    };
    let rounded = u16::from(alpha >> 3) + u16::from((alpha & 7) > threshold);
    let five = rounded.min(31) as u8;
    (five << 3) | (five >> 2)
}

fn alpha_compare_value(
    mode: AlphaCompare,
    alpha: u8,
    threshold_alpha: u8,
    noise: NoiseSample,
) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        // Programming Manual 15.5.4 describes this as alpha greater than a
        // random value in [0,1). Cross-multiply alpha/255 and noise/256 so
        // transparent always rejects and opaque always passes. The byte is the
        // shared per-fragment source, not an ordered screen-space matrix.
        AlphaCompare::Dither => u32::from(alpha) * 256 > u32::from(noise.byte()) * 255,
        AlphaCompare::Reserved => {
            unreachable!("reserved alpha compare is rejected before rasterization")
        }
    }
}

/// Copy-cycle alpha comparison is format-dependent. Programming Manual
/// section 15.5.4 states that an RGBA16 texel does not enter the eight-bit
/// comparator: its single alpha bit is the write enable. The supported direct
/// 8-bit source retains the ordinary blend-alpha threshold.
fn copy_alpha_compare_value(
    mode: AlphaCompare,
    texture: &crate::gbi::Texture,
    alpha: u8,
    threshold_alpha: u8,
    noise: NoiseSample,
) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold | AlphaCompare::Dither
            if texture.format == ColorImage::RGBA_FORMAT && texture.size == ColorImage::BITS_16 =>
        {
            alpha != 0
        }
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        AlphaCompare::Dither => u32::from(alpha) * 256 > u32::from(noise.byte()) * 255,
        AlphaCompare::Reserved => {
            unreachable!("reserved alpha compare is rejected before copy rasterization")
        }
    }
}

/// Evaluate the RDP blender selectors for one covered fragment. The public
/// GBI defines each cycle as `P*A + M*B` (`GBL_c1`/`GBL_c2`, gbi.h:612-627).
/// In a second cycle, `G_BL_CLR_IN` names the first cycle's blender result;
/// the framebuffer selector always names the pre-fragment destination.
/// RT64 models the same selector ordering and sequential cycle handoff in
/// `shared/rt64_blender.h:68-81,366-504`.
fn blend_fragment(
    src: [u8; 4],
    dst: [u8; 4],
    shade_alpha: u8,
    state: BlenderState,
    blend_enabled: bool,
    memory_coverage: Coverage,
) -> [u8; 4] {
    if state.cycle_count == 0 {
        return src;
    }

    let src_rgb = [src[0] as f32, src[1] as f32, src[2] as f32];
    let mut blender_rgb = src_rgb;
    let mut final_alpha = 1.0;

    for cycle_index in 0..state.cycle_count.min(2) as usize {
        let cycle = state.cycles[cycle_index];
        let is_last = cycle_index + 1 == state.cycle_count as usize;

        // Without FORCE_BL the last blender cycle is bypassed and selects P;
        // in two-cycle mode cycle 1 still runs (the standard fog-then-pass
        // arrangement). RT64's cycle count/bypass has the same structure at
        // shared/rt64_blender.h:45-65,370-383.
        if is_last && !blend_enabled {
            blender_rgb = blend_color(cycle.p, src_rgb, dst, state, blender_rgb, cycle_index);
            final_alpha = if cycle.p == BlendColorInput::Framebuffer {
                0.0
            } else {
                1.0
            };
            continue;
        }

        let a = blend_a(cycle.a, src[3], shade_alpha, state.fog_color[3]);
        let p = blend_color(cycle.p, src_rgb, dst, state, blender_rgb, cycle_index);
        let m = blend_color(cycle.m, src_rgb, dst, state, blender_rgb, cycle_index);

        // RT64 emits framebuffer terms through dual-source alpha blending
        // (`rt64_blender.h:414-424`; `rt64_raster_shader.cpp:332-339`). This
        // software target performs that final composite here instead: the
        // non-framebuffer input becomes the source color and A becomes its
        // source-alpha factor.
        if cycle.p == BlendColorInput::Framebuffer {
            blender_rgb = m;
            final_alpha = 1.0 - a;
        } else if cycle.m == BlendColorInput::Framebuffer {
            blender_rgb = p;
            final_alpha = a;
        } else {
            let b = blend_b(cycle.b, a, memory_coverage);
            if a == 0.0 {
                blender_rgb = m;
            } else if b == 0.0 {
                blender_rgb = p;
            } else {
                let divisor = a + b;
                for channel in 0..3 {
                    blender_rgb[channel] =
                        ((p[channel] * a + m[channel] * b) / divisor).clamp(0.0, 255.0);
                }
            }
            final_alpha = 1.0;
        }
    }

    let mut out_rgb = [0u8; 3];
    for channel in 0..3 {
        out_rgb[channel] = (blender_rgb[channel] * final_alpha
            + dst[channel] as f32 * (1.0 - final_alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let alpha = (255.0 * final_alpha + dst[3] as f32 * (1.0 - final_alpha))
        .round()
        .clamp(0.0, 255.0) as u8;
    [out_rgb[0], out_rgb[1], out_rgb[2], alpha]
}

fn blend_color(
    input: BlendColorInput,
    src_rgb: [f32; 3],
    dst: [u8; 4],
    state: BlenderState,
    blender_rgb: [f32; 3],
    cycle_index: usize,
) -> [f32; 3] {
    match input {
        BlendColorInput::Combined if cycle_index == 0 => src_rgb,
        BlendColorInput::Combined => blender_rgb,
        BlendColorInput::Framebuffer => [dst[0] as f32, dst[1] as f32, dst[2] as f32],
        BlendColorInput::Blend => [
            state.blend_color[0] as f32,
            state.blend_color[1] as f32,
            state.blend_color[2] as f32,
        ],
        BlendColorInput::Fog => [
            state.fog_color[0] as f32,
            state.fog_color[1] as f32,
            state.fog_color[2] as f32,
        ],
    }
}

fn blend_a(input: BlendAlphaInput, combined: u8, shade: u8, fog: u8) -> f32 {
    let value = match input {
        BlendAlphaInput::Combined => combined,
        BlendAlphaInput::Fog => fog,
        BlendAlphaInput::Shade => shade,
        BlendAlphaInput::Zero => 0,
    };
    value as f32 / 255.0
}

fn blend_b(input: BlendBInput, a: f32, memory_coverage: Coverage) -> f32 {
    match input {
        BlendBInput::OneMinusA => 1.0 - a,
        BlendBInput::FramebufferAlpha => memory_coverage.count() as f32 / 8.0,
        BlendBInput::One => 1.0,
        BlendBInput::Zero => 0.0,
    }
}

fn edge(a: Vertex, b: Vertex, c: Vertex) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gbi::{BlendCycle, ScissorRect};

    fn cycle(rgb: [ColorSource; 4], alpha: [AlphaSource; 4]) -> CombinerCycle {
        CombinerCycle { rgb, alpha }
    }

    fn repeated_state(
        cycle: CombinerCycle,
        primitive: [u8; 4],
        environment: [u8; 4],
    ) -> CombinerState {
        CombinerState {
            mode: crate::gbi::CombinerMode { cycles: [cycle; 2] },
            primitive,
            environment,
            min_lod_level: 0,
            prim_lod_fraction: 0,
            convert: crate::gbi::ConvertState::default(),
            key: crate::gbi::KeyState::default(),
        }
    }

    #[test]
    fn noise_combiner_source_uses_the_fragment_noise_byte() {
        let state = repeated_state(
            cycle(
                [
                    ColorSource::Noise,
                    ColorSource::Zero,
                    ColorSource::One,
                    ColorSource::Zero,
                ],
                [AlphaSource::Zero; 4],
            ),
            [0; 4],
            [0; 4],
        );
        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, [0; 4], [0; 4], [0; 4], NoiseSample(0x80)),
            ),
            [128, 128, 128, 0],
        );
    }

    fn v(x: f32, y: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
        Vertex {
            x,
            y,
            w: 1.0,
            r,
            g,
            b,
            a,
            ..Default::default()
        }
    }

    fn standard_alpha_blender(cycle_count: u8) -> BlenderState {
        let cycle = BlendCycle {
            p: BlendColorInput::Combined,
            a: BlendAlphaInput::Combined,
            m: BlendColorInput::Framebuffer,
            b: BlendBInput::OneMinusA,
        };
        BlenderState {
            cycle_count,
            force_blend: true,
            cycles: [cycle; 2],
            ..Default::default()
        }
    }

    fn shade_only_combiner() -> CombinerState {
        repeated_state(
            cycle(
                [
                    ColorSource::Zero,
                    ColorSource::Zero,
                    ColorSource::Zero,
                    ColorSource::Shade,
                ],
                [
                    AlphaSource::Zero,
                    AlphaSource::Zero,
                    AlphaSource::Zero,
                    AlphaSource::Shade,
                ],
            ),
            [0; 4],
            [0; 4],
        )
    }

    fn test_line(width: f32, smooth_shading: bool) -> Line {
        Line {
            v: [v(2.0, 4.0, 255, 0, 0, 255), v(6.0, 4.0, 0, 0, 255, 255)],
            width,
            smooth_shading,
            scissor: None,
            texture: None,
            other_mode: OtherMode::default(),
            combiner: shade_only_combiner(),
            blender: BlenderState::default(),
        }
    }

    #[test]
    fn line_raster_uses_public_minimum_width_and_butt_endpoints() {
        let mut framebuffer = Framebuffer::new(10, 8);
        framebuffer.draw_line_no_depth(&test_line(1.5, false));

        let pixel = |x: usize, y: usize| &framebuffer.pixels[(y * 10 + x) * 4..][..4];
        assert_eq!(pixel(3, 3), &[255, 0, 0, 255]);
        assert_eq!(pixel(3, 4), &[255, 0, 0, 255]);
        assert_eq!(pixel(3, 2), &[0, 0, 0, 0]);
        assert_eq!(pixel(1, 4), &[0, 0, 0, 0]);
        assert_eq!(pixel(6, 4), &[0, 0, 0, 0]);
    }

    #[test]
    fn line_width_and_smooth_shading_change_coverage_and_color() {
        let mut narrow = Framebuffer::new(10, 8);
        narrow.draw_line_no_depth(&test_line(1.5, false));
        let mut wide = Framebuffer::new(10, 8);
        wide.draw_line_no_depth(&test_line(3.0, true));

        let painted = |framebuffer: &Framebuffer| {
            framebuffer
                .pixels
                .chunks_exact(4)
                .filter(|pixel| pixel.iter().any(|component| *component != 0))
                .count()
        };
        assert!(painted(&wide) > painted(&narrow));
        let center = (4 * 10 + 4) * 4;
        assert_eq!(&narrow.pixels[center..center + 4], &[255, 0, 0, 255]);
        assert_eq!(&wide.pixels[center..center + 4], &[95, 0, 159, 255]);
    }

    #[test]
    fn line_depth_is_read_only_even_when_update_bit_is_programmed() {
        let mut framebuffer = Framebuffer::new(10, 8);
        let mut line = test_line(1.5, false);
        line.other_mode = OtherMode::from_raw(0xf0, 0x10 | 0x20, 0);
        framebuffer.draw_line(&line);
        assert!(framebuffer.depth.iter().all(|depth| depth.is_infinite()));
        assert!(framebuffer.pixels.iter().any(|component| *component != 0));
    }

    fn solid_texture(rgba: [u8; 4]) -> crate::gbi::Texture {
        crate::gbi::Texture {
            format: 0,
            size: 2,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(rgba.to_vec()),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        }
    }

    #[test]
    fn textured_line_uses_perspective_attribute_path_and_scissor() {
        let mut line = test_line(1.5, true);
        line.v[0].s = 0.0;
        line.v[1].s = 1.0;
        line.v[0].w = 1.0;
        line.v[1].w = 2.0;
        line.texture = Some(solid_texture([0, 255, 0, 255]));
        line.combiner = repeated_state(
            texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0),
            [0; 4],
            [0; 4],
        );
        line.scissor = Some(ScissorRect {
            ulx: 4.0,
            uly: 0.0,
            lrx: 5.0,
            lry: 8.0,
            field: false,
            keep_odd: false,
        });

        let mut framebuffer = Framebuffer::new(10, 8);
        framebuffer.draw_line_no_depth(&line);
        let inside = (4 * 10 + 4) * 4;
        let outside = (4 * 10 + 3) * 4;
        assert_eq!(&framebuffer.pixels[inside..inside + 4], &[0, 255, 0, 255]);
        assert_eq!(&framebuffer.pixels[outside..outside + 4], &[0, 0, 0, 0]);
    }

    fn texel_passthrough_cycle(source: ColorSource, alpha: AlphaSource) -> CombinerCycle {
        cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                source,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                alpha,
            ],
        )
    }

    fn texture_rectangle(
        texture: crate::gbi::Texture,
        other_mode: crate::gbi::OtherMode,
        combiner: CombinerState,
    ) -> TextureRectangle {
        TextureRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: 1 << 10,
            dtdy: 1 << 10,
            flip: false,
            other_mode,
            combiner,
            blender: BlenderState {
                cycle_count: match other_mode.cycle_type() {
                    CycleType::OneCycle => 1,
                    CycleType::TwoCycle => 2,
                    _ => 0,
                },
                ..BlenderState::default()
            },
            scissor: None,
            texture: Some(texture),
            texture1: None,
        }
    }

    #[test]
    fn clear_fills_every_pixel() {
        let mut fb = Framebuffer::new(4, 4);
        fb.clear(10, 20, 30, 255);
        assert!(!fb.has_non_uniform_content(10, 20, 30, 255));
        assert_eq!(&fb.pixels[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn field_scissor_rejects_opposite_parity_in_every_raster_path() {
        let odd_field = Some(ScissorRect {
            ulx: 0.0,
            uly: 0.0,
            lrx: 4.0,
            lry: 4.0,
            field: true,
            keep_odd: true,
        });
        let assert_rows = |framebuffer: &Framebuffer, painted: [u8; 4]| {
            for y in 0..4usize {
                for x in 0..4usize {
                    let offset = (y * 4 + x) * 4;
                    let expected = if y % 2 == 1 { painted } else { [0, 0, 0, 255] };
                    assert_eq!(
                        &framebuffer.pixels[offset..offset + 4],
                        &expected,
                        "field scissor mismatch at ({x}, {y})"
                    );
                }
            }
        };

        let fill = FillRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 3.0,
            lry: 3.0,
            fill_color: 0xffff_ffff,
            cycle_type: CycleType::Fill,
            scissor: odd_field,
            other_mode: OtherMode::default(),
            combiner: CombinerState::default(),
            blender: BlenderState::default(),
        };
        let mut fill_fb = Framebuffer::new(4, 4);
        fill_fb.clear(0, 0, 0, 255);
        fill_fb.draw_fill_rectangle(
            &fill,
            ColorImage {
                format: ColorImage::RGBA_FORMAT,
                size: ColorImage::BITS_16,
                width: 4,
                address: 0,
            },
        );
        assert_rows(&fill_fb, [255; 4]);

        let mut depth_fb = Framebuffer::new(4, 4);
        depth_fb.clear_depth_rectangle(&fill);
        for y in 0..4usize {
            for x in 0..4usize {
                let depth = depth_fb.depth[y * 4 + x];
                assert_eq!(
                    depth.is_finite(),
                    y % 2 == 1,
                    "depth field scissor mismatch at ({x}, {y})"
                );
            }
        }

        let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
        let mut combined = texture_rectangle(
            solid_texture([255, 0, 0, 255]),
            OtherMode::default(),
            repeated_state(passthrough, [0; 4], [0; 4]),
        );
        combined.lrx = 4.0;
        combined.lry = 4.0;
        combined.scissor = odd_field;
        let mut combined_fb = Framebuffer::new(4, 4);
        combined_fb.clear(0, 0, 0, 255);
        combined_fb.draw_texture_rectangle(&combined);
        assert_rows(&combined_fb, [255, 0, 0, 255]);

        let mut copy_texture = solid_texture([255, 0, 0, 255]);
        copy_texture.width = 4;
        copy_texture.height = 4;
        copy_texture.texels = std::rc::Rc::new([255, 0, 0, 255].repeat(16));
        let mut copy = texture_rectangle(
            copy_texture,
            OtherMode::from_raw(2 << 20, 0, 0),
            CombinerState::default(),
        );
        copy.lrx = 3.0;
        copy.lry = 3.0;
        copy.dsdx = 4 << 10;
        copy.scissor = odd_field;
        let mut copy_fb = Framebuffer::new(4, 4);
        copy_fb.clear(0, 0, 0, 255);
        copy_fb.draw_copy_texture_rectangle(&copy);
        assert_rows(&copy_fb, [255, 0, 0, 255]);

        let high = Triangle {
            v: [
                v(-10.0, -10.0, 255, 0, 0, 255),
                v(20.0, -10.0, 255, 0, 0, 255),
                v(-10.0, 20.0, 255, 0, 0, 255),
            ],
            scissor: odd_field,
            ..Triangle::default()
        };
        let mut high_fb = Framebuffer::new(4, 4);
        high_fb.clear(0, 0, 0, 255);
        high_fb.draw_triangle(&high);
        assert_rows(&high_fb, [255, 0, 0, 255]);

        let raw = RawRdpTriangle {
            edge: crate::gbi::RdpEdgeCoefficients {
                right_major: false,
                level: 0,
                tile: 0,
                yl: 16,
                ym: 8,
                yh: 0,
                xl: 4 << 16,
                dxldy: 0,
                xh: 0,
                dxhdy: 0,
                xm: 4 << 16,
                dxmdy: 0,
            },
            shade: None,
            texture_coefficients: None,
            z: None,
            texture: None,
            other_mode: OtherMode::default(),
            combiner: CombinerState {
                primitive: [255, 0, 0, 255],
                ..CombinerState::default()
            },
            blender: BlenderState::default(),
            scissor: odd_field,
        };
        let mut raw_fb = Framebuffer::new(4, 4);
        raw_fb.clear(0, 0, 0, 255);
        raw_fb.draw_raw_rdp_triangle(&raw);
        assert_rows(&raw_fb, [255, 0, 0, 255]);
    }

    #[test]
    fn primitive_depth_drives_texture_rectangle_compare_and_update() {
        let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
        let other_mode =
            crate::gbi::OtherMode::from_raw(crate::gbi::OtherMode::default().raw_high(), 0x34, 0);
        let rectangle = texture_rectangle(
            solid_texture([255, 0, 0, 255]),
            other_mode,
            repeated_state(passthrough, [0; 4], [0; 4]),
        );
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.depth[0] = 0x3ffff as f32;
        framebuffer.encoded_depth[0] = Some(crate::depth::EncodedDepth {
            visible: 0xfffc,
            hidden: 0,
        });
        framebuffer.set_primitive_depth(Some(PrimitiveDepth { z: 8, delta_z: 32 }));

        framebuffer.draw_texture_rectangle(&rectangle);

        let expected = crate::depth::pack(8 << 3, 32);
        assert_eq!(&framebuffer.pixels, &[255, 0, 0, 255]);
        assert_eq!(framebuffer.encoded_depth[0], Some(expected));
        assert_eq!(
            framebuffer.depth[0],
            crate::depth::unpack(expected).0 as f32
        );
    }

    #[test]
    fn flipped_texture_rectangle_swaps_s_and_t_screen_axes() {
        let texture = crate::gbi::Texture {
            format: 0,
            size: 2,
            width: 2,
            height: 2,
            texels: std::rc::Rc::new(vec![
                255, 0, 0, 255, // top-left
                0, 255, 0, 255, // top-right
                0, 0, 255, 255, // bottom-left
                255, 255, 255, 255, // bottom-right
            ]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
        let mut rectangle = texture_rectangle(
            texture,
            crate::gbi::OtherMode::default(),
            repeated_state(passthrough, [0; 4], [0; 4]),
        );
        rectangle.lrx = 2.0;
        rectangle.lry = 2.0;
        rectangle.flip = true;

        let mut framebuffer = Framebuffer::new(2, 2);
        framebuffer.draw_texture_rectangle(&rectangle);
        assert_eq!(
            framebuffer.pixels,
            vec![
                255, 0, 0, 255, // source (0,0)
                0, 0, 255, 255, // source (0,1)
                0, 255, 0, 255, // source (1,0)
                255, 255, 255, 255, // source (1,1)
            ]
        );
    }

    #[test]
    fn flipped_copy_texture_rectangle_swaps_axes_with_copy_gradient_scaling() {
        let texture = crate::gbi::Texture {
            format: 0,
            size: 2,
            width: 2,
            height: 2,
            texels: std::rc::Rc::new(vec![
                255, 0, 0, 255, // top-left
                0, 255, 0, 255, // top-right
                0, 0, 255, 255, // bottom-left
                255, 255, 255, 255, // bottom-right
            ]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut rectangle = texture_rectangle(
            texture,
            crate::gbi::OtherMode::from_raw(2 << 20, 0, 0),
            CombinerState::default(),
        );
        rectangle.lrx = 1.0;
        rectangle.lry = 1.0;
        rectangle.dsdx = 4 << 10;
        rectangle.dtdy = 1 << 10;
        rectangle.flip = true;

        let mut framebuffer = Framebuffer::new(2, 2);
        framebuffer.draw_copy_texture_rectangle(&rectangle);
        assert_eq!(
            framebuffer.pixels,
            vec![
                255, 0, 0, 255, // source (0,0)
                0, 0, 255, 255, // source (0,1)
                0, 255, 0, 255, // source (1,0)
                255, 255, 255, 255, // source (1,1)
            ]
        );
    }

    #[test]
    fn copy_clamp_bits_are_ignored_before_mask_wrap() {
        let texture = crate::gbi::Texture {
            format: 0,
            size: 2,
            width: 4,
            height: 1,
            texels: std::rc::Rc::new(
                (0..4u8)
                    .flat_map(|value| [value, value, value, 255])
                    .collect(),
            ),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 2,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut rectangle = texture_rectangle(
            texture,
            OtherMode::from_raw(2 << 20, 0, 0),
            CombinerState::default(),
        );
        rectangle.lrx = 7.0;
        rectangle.lry = 0.0;
        rectangle.dsdx = 4 << 10;

        let mut framebuffer = Framebuffer::new(8, 1);
        framebuffer.draw_copy_texture_rectangle(&rectangle);
        assert_eq!(
            framebuffer
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 0, 1, 2, 3]
        );
    }

    #[test]
    fn rgba16_copy_alpha_compare_uses_alpha_bit_even_when_blend_threshold_is_zero() {
        let mut texture = solid_texture([0; 4]);
        texture.width = 2;
        texture.texels = std::rc::Rc::new(vec![
            255, 0, 0, 0, // RGBA16 alpha bit clear: write disabled
            0, 255, 0, 255, // RGBA16 alpha bit set: write enabled
        ]);
        for alpha_compare in [AlphaCompare::Threshold, AlphaCompare::Dither] {
            let low = match alpha_compare {
                AlphaCompare::Threshold => 1,
                AlphaCompare::Dither => 3,
                _ => unreachable!(),
            };
            let mut rectangle = texture_rectangle(
                texture.clone(),
                OtherMode::from_raw(2 << 20, low, 0),
                CombinerState::default(),
            );
            rectangle.lrx = 1.0;
            rectangle.lry = 0.0;
            rectangle.dsdx = 4 << 10;

            let mut framebuffer = Framebuffer::new(2, 1);
            framebuffer.clear(9, 8, 7, 255);
            framebuffer.draw_copy_texture_rectangle(&rectangle);

            assert_eq!(&framebuffer.pixels[0..4], &[9, 8, 7, 255]);
            assert_eq!(&framebuffer.pixels[4..8], &[0, 255, 0, 255]);
        }
    }

    #[test]
    fn rgba16_copy_without_alpha_compare_writes_alpha_zero_texel() {
        let mut rectangle = texture_rectangle(
            solid_texture([255, 0, 0, 0]),
            OtherMode::from_raw(2 << 20, 0, 0),
            CombinerState::default(),
        );
        rectangle.lrx = 0.0;
        rectangle.lry = 0.0;
        rectangle.dsdx = 4 << 10;

        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.clear(9, 8, 7, 255);
        framebuffer.draw_copy_texture_rectangle(&rectangle);

        assert_eq!(framebuffer.pixels, [255, 0, 0, 0]);
    }

    #[test]
    fn two_cycle_texture_rectangle_combines_distinct_texel_tiles() {
        let first = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
        let second = cycle(
            [
                ColorSource::Texel1,
                ColorSource::Combined,
                ColorSource::EnvironmentAlpha,
                ColorSource::Combined,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Combined,
            ],
        );
        let high = (crate::gbi::OtherMode::default().raw_high() & !(3 << 20)) | (1 << 20);
        let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
        let mut rectangle = texture_rectangle(
            solid_texture([100, 100, 100, 255]),
            other_mode,
            CombinerState {
                mode: crate::gbi::CombinerMode {
                    cycles: [first, second],
                },
                primitive: [0; 4],
                environment: [0, 0, 0, 128],
                min_lod_level: 0,
                prim_lod_fraction: 0,
                convert: crate::gbi::ConvertState::default(),
                key: crate::gbi::KeyState::default(),
            },
        );
        rectangle.texture1 = Some(solid_texture([200, 200, 200, 255]));

        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.draw_texture_rectangle(&rectangle);
        assert_eq!(framebuffer.pixels, vec![150, 150, 150, 255]);
    }

    #[test]
    fn texture_rectangle_lod_selects_adjacent_mips_and_feeds_fraction() {
        let tile0 = solid_texture([255, 255, 255, 255]);
        let tile1 = solid_texture([0, 0, 0, 255]);
        let tile2 = solid_texture([200, 200, 200, 255]);
        let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
        tiles[0] = Some(tile0.clone());
        tiles[1] = Some(tile1.clone());
        tiles[2] = Some(tile2);
        let base = tile0.with_lod_snapshot(tiles, 0, 2);

        let trilerp = cycle(
            [
                ColorSource::Texel1,
                ColorSource::Texel0,
                ColorSource::LodFraction,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Texel1,
                AlphaSource::Texel0,
                AlphaSource::LodFraction,
                AlphaSource::Texel0,
            ],
        );
        let pass = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Combined,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Combined,
            ],
        );
        let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
            | (1 << 20)
            | (1 << 16);
        let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
        let mut rectangle = texture_rectangle(
            base,
            other_mode,
            CombinerState {
                mode: crate::gbi::CombinerMode {
                    cycles: [trilerp, pass],
                },
                ..CombinerState::default()
            },
        );
        rectangle.texture1 = Some(tile1);
        rectangle.dsdx = (2.5 * 1024.0) as i16;
        rectangle.dtdy = 0;

        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.draw_texture_rectangle(&rectangle);
        assert_eq!(framebuffer.pixels, vec![50, 50, 50, 255]);
    }

    #[test]
    fn high_level_triangle_uses_the_shared_lod_tile_and_fraction_path() {
        let tile0 = solid_texture([255, 255, 255, 255]);
        let tile1 = solid_texture([0, 0, 0, 255]);
        let tile2 = solid_texture([200, 200, 200, 255]);
        let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
        tiles[0] = Some(tile0.clone());
        tiles[1] = Some(tile1);
        tiles[2] = Some(tile2);
        let texture = tile0.with_lod_snapshot(tiles, 0, 2);
        let trilerp = cycle(
            [
                ColorSource::Texel1,
                ColorSource::Texel0,
                ColorSource::LodFraction,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Texel1,
                AlphaSource::Texel0,
                AlphaSource::LodFraction,
                AlphaSource::Texel0,
            ],
        );
        let pass = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Combined,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Combined,
            ],
        );
        let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
            | (1 << 20)
            | (1 << 16);
        let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
        let textured = |x: f32, y: f32, s: f32, t: f32| Vertex {
            x,
            y,
            s,
            t,
            w: 1.0,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            ..Vertex::default()
        };
        let triangle = Triangle {
            v: [
                textured(0.0, 0.0, 0.0, 0.0),
                textured(2.0, 0.0, 5.0, 0.0),
                textured(0.0, 2.0, 0.0, 5.0),
            ],
            texture: Some(texture),
            other_mode,
            combiner: CombinerState {
                mode: crate::gbi::CombinerMode {
                    cycles: [trilerp, pass],
                },
                ..CombinerState::default()
            },
            blender: BlenderState {
                cycle_count: 2,
                ..BlenderState::default()
            },
            ..Triangle::default()
        };

        let mut framebuffer = Framebuffer::new(2, 2);
        framebuffer.draw_triangle(&triangle);
        assert_eq!(&framebuffer.pixels[..4], &[50, 50, 50, 255]);
    }

    #[test]
    fn raw_triangle_uses_the_shared_lod_tile_and_fraction_path() {
        let tile0 = solid_texture([255, 255, 255, 255]);
        let tile1 = solid_texture([0, 0, 0, 255]);
        let tile2 = solid_texture([200, 200, 200, 255]);
        let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
        tiles[0] = Some(tile0.clone());
        tiles[1] = Some(tile1);
        tiles[2] = Some(tile2);
        let texture = tile0.with_lod_snapshot(tiles, 0, 2);
        let trilerp = cycle(
            [
                ColorSource::Texel1,
                ColorSource::Texel0,
                ColorSource::LodFraction,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Texel1,
                AlphaSource::Texel0,
                AlphaSource::LodFraction,
                AlphaSource::Texel0,
            ],
        );
        let pass = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Combined,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Combined,
            ],
        );
        let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
            | (1 << 20)
            | (1 << 16);
        let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
        let triangle = RawRdpTriangle {
            edge: crate::gbi::RdpEdgeCoefficients {
                right_major: true,
                level: 2,
                tile: 0,
                yl: 4,
                ym: 2,
                yh: 0,
                xl: 0,
                dxldy: 0,
                xh: 1 << 16,
                dxhdy: 0,
                xm: 0,
                dxmdy: 0,
            },
            shade: None,
            texture_coefficients: Some(crate::gbi::RdpTextureCoefficients {
                stw: [0, 0, 1 << 16],
                dstdx: [(2.5 * 65536.0) as i32, 0, 0],
                dstde: [0; 3],
                dstdy: [0, (2.5 * 65536.0) as i32, 0],
            }),
            z: None,
            texture: Some(texture),
            other_mode,
            combiner: CombinerState {
                mode: crate::gbi::CombinerMode {
                    cycles: [trilerp, pass],
                },
                ..CombinerState::default()
            },
            blender: BlenderState {
                cycle_count: 2,
                ..BlenderState::default()
            },
            scissor: None,
        };

        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.draw_raw_rdp_triangle(&triangle);
        assert_eq!(framebuffer.pixels, vec![50, 50, 50, 255]);
    }

    #[test]
    fn combiner_presets_select_decal_primitive_environment_and_shade_sources() {
        // Fail-against-bug: the old rasterizer always returned TEXEL0*SHADE,
        // so every assertion except MODULATE below produced the same wrong
        // color regardless of the decoded primitive/environment registers.
        let shade = [50, 100, 150, 220];
        let texel = [128, 64, 255, 180];

        let shade_only = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Shade,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Shade,
            ],
        );
        assert_eq!(
            evaluate_combiner(
                repeated_state(shade_only, [0; 4], [0; 4]),
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
            ),
            shade
        );

        let decal = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Texel0,
            ],
        );
        assert_eq!(
            evaluate_combiner(
                repeated_state(decal, [0; 4], [0; 4]),
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
            ),
            texel
        );

        let primitive_tint = cycle(
            [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Primitive,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ],
        );
        let primitive = [128, 255, 64, 128];
        assert_eq!(
            evaluate_combiner(
                repeated_state(primitive_tint, primitive, [0; 4]),
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
            ),
            [64, 64, 64, 90]
        );

        // G_CC_BLENDI: (ENVIRONMENT - SHADE) * TEXEL0 + SHADE.
        let env_blend = cycle(
            [
                ColorSource::Environment,
                ColorSource::Shade,
                ColorSource::Texel0,
                ColorSource::Shade,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Shade,
            ],
        );
        assert_eq!(
            evaluate_combiner(
                repeated_state(env_blend, [0; 4], [250, 200, 100, 255]),
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
            ),
            [150, 125, 100, 220]
        );
    }

    #[test]
    fn combiner_second_cycle_consumes_first_cycle_combined_result() {
        // Cycle 0: TEXEL0*SHADE. Cycle 1: COMBINED*PRIMITIVE. This fails if
        // only the second programmed tuple is evaluated or COMBINED is not
        // carried between cycles.
        let first = cycle(
            [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Shade,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Shade,
                AlphaSource::Zero,
            ],
        );
        let second = cycle(
            [
                ColorSource::Combined,
                ColorSource::Zero,
                ColorSource::Primitive,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Combined,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ],
        );
        let state = CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [first, second],
            },
            primitive: [128; 4],
            environment: [0; 4],
            min_lod_level: 0,
            prim_lod_fraction: 0,
            convert: crate::gbi::ConvertState::default(),
            key: crate::gbi::KeyState::default(),
        };
        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::TwoCycle,
                false,
                CombinerPixel::new(0.0, [128; 4], [200; 4], [200; 4], NoiseSample::ZERO),
            ),
            [50; 4]
        );
    }

    #[test]
    fn conversion_k4_and_k5_feed_the_color_combiner() {
        // Public Set Convert stage two: (R' - K4) * K5 + R'. K4 is the
        // 8-bit offset and K5 is its 8-bit fractional scale.
        let conversion = cycle(
            [
                ColorSource::Texel0,
                ColorSource::K4,
                ColorSource::K5,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Texel0,
            ],
        );
        let state = CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [conversion; 2],
            },
            ..CombinerState::default()
        };
        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::OneCycle,
                false,
                CombinerPixel::new(0.0, [0; 4], [100, 150, 200, 255], [0; 4], NoiseSample::ZERO,),
            ),
            [98, 156, 214, 255]
        );
    }

    #[test]
    fn chroma_key_center_scale_and_width_drive_alpha_fixup() {
        let key_cycle = cycle(
            [
                ColorSource::Texel0,
                ColorSource::KeyCenter,
                ColorSource::KeyScale,
                ColorSource::Zero,
            ],
            [AlphaSource::Zero; 4],
        );
        let state = CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [key_cycle; 2],
            },
            key: crate::gbi::KeyState {
                center: [100; 3],
                scale: [255; 3],
                width: [0x100; 3],
            },
            ..CombinerState::default()
        };

        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::OneCycle,
                true,
                CombinerPixel::new(0.0, [0; 4], [100, 100, 100, 255], [0; 4], NoiseSample::ZERO,),
            ),
            [0, 0, 0, 255]
        );
        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::OneCycle,
                true,
                CombinerPixel::new(0.0, [0; 4], [200, 100, 100, 255], [0; 4], NoiseSample::ZERO,),
            ),
            [100, 0, 0, 155]
        );
    }

    #[test]
    fn triangle_paints_interior_pixels_and_leaves_exterior_clear() {
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        let tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 0, 0, 255),
                v(12.0, 2.0, 255, 0, 0, 255),
                v(7.0, 12.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        assert!(fb.has_non_uniform_content(0, 0, 0, 255));
        // Centroid should be red.
        let cx = 7u32;
        let cy = 6u32;
        let idx = (cy * fb.width + cx) as usize * 4;
        assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
        // A far corner should remain untouched (still clear color).
        let idx0 = 0usize;
        assert_eq!(&fb.pixels[idx0..idx0 + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn high_level_triangle_uses_the_public_eight_sample_coverage_mask() {
        let mut fb = Framebuffer::new(1, 1);
        fb.clear(0, 0, 0, 255);
        let tri = Triangle {
            // The right edge cuts pixel zero at x=1/2. Exactly the four
            // checkerboard samples at x=1/8 or 3/8 are inside.
            v: [
                v(-10.0, -10.0, 255, 0, 0, 255),
                v(0.5, -10.0, 255, 0, 0, 255),
                v(0.5, 10.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };

        let area = edge(tri.v[0], tri.v[1], tri.v[2]);
        let mask = triangle_pixel_coverage(tri.v, area, ScissorRect::framebuffer(1, 1), 0, 0);
        assert_eq!(mask, CoverageMask(0x55));
        assert_eq!(mask.coverage(), Coverage::new(4));

        fb.draw_triangle(&tri);

        assert_eq!(fb.coverage[0], Coverage::new(4));
        assert_eq!(&fb.pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn high_level_partial_coverage_retains_the_exact_covered_sample_identity() {
        let tri = [
            Vertex {
                x: 0.0,
                y: 0.0,
                ..Vertex::default()
            },
            Vertex {
                x: 0.4,
                y: 0.0,
                ..Vertex::default()
            },
            Vertex {
                x: 0.0,
                y: 0.4,
                ..Vertex::default()
            },
        ];
        let mask = triangle_pixel_coverage(
            tri,
            edge(tri[0], tri[1], tri[2]),
            ScissorRect::framebuffer(1, 1),
            0,
            0,
        );

        assert_eq!(mask, CoverageMask(0x01));
        assert_eq!(mask.coverage(), Coverage::new(1));
        assert!(mask.contains(0));
        assert!((1..COVERAGE_SAMPLES.len()).all(|index| !mask.contains(index)));

        let covered = Vertex {
            x: COVERAGE_SAMPLES[0].0 as f32 / 8.0,
            y: COVERAGE_SAMPLES[0].1 as f32 / 8.0,
            ..Vertex::default()
        };
        let center = Vertex {
            x: 0.5,
            y: 0.5,
            ..Vertex::default()
        };
        assert!(edge(tri[1], tri[2], covered) > 0.0);
        assert!(edge(tri[1], tri[2], center) < 0.0);
    }

    #[test]
    fn setscissor_bounds_triangle_writes_to_exclusive_rect() {
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        let tri = Triangle {
            v: [
                v(0.0, 0.0, 255, 0, 0, 255),
                v(16.0, 0.0, 255, 0, 0, 255),
                v(0.0, 16.0, 255, 0, 0, 255),
            ],
            scissor: Some(ScissorRect {
                ulx: 4.25,
                uly: 3.75,
                lrx: 9.25,
                lry: 8.75,
                field: false,
                keep_odd: false,
            }),
            ..Default::default()
        };

        fb.draw_triangle(&tri);

        let inside = (5usize * 16 + 5) * 4;
        assert_eq!(&fb.pixels[inside..inside + 4], &[255, 0, 0, 255]);
        for y in 0..16usize {
            for x in 0..16usize {
                // The eight-sample mask reaches x=9 and y=3 even though
                // those pixel centers lie outside the quarter-pixel scissor.
                if !(4..10).contains(&x) || !(3..9).contains(&y) {
                    let i = (y * 16 + x) * 4;
                    assert_eq!(
                        &fb.pixels[i..i + 4],
                        &[0, 0, 0, 255],
                        "triangle wrote outside exclusive scissor at ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn textured_triangle_modulates_texel_by_shade() {
        use crate::gbi::Texture;
        // 1×1 white texture: modulate leaves the shade color unchanged.
        let white = Texture {
            format: 0,
            size: 2,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255, 255, 255, 255]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        // Green-shaded triangle, all S/T = 0 (samples the one white texel).
        let mut tri = Triangle {
            v: [
                v(2.0, 2.0, 0, 200, 0, 255),
                v(12.0, 2.0, 0, 200, 0, 255),
                v(7.0, 12.0, 0, 200, 0, 255),
            ],
            ..Default::default()
        };
        tri.texture = Some(white);
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        // white(255) * shade(200) / 255 == 200: texture didn't tint the shade.
        assert_eq!(&fb.pixels[idx..idx + 4], &[0, 200, 0, 255]);
    }

    #[test]
    fn textured_triangle_paints_texel_color() {
        use crate::gbi::Texture;
        // 1×1 red texture under a white shade -> red pixel (modulate).
        let red = Texture {
            format: 0,
            size: 2,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255, 0, 0, 255]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        let mut tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 255, 255, 255),
                v(12.0, 2.0, 255, 255, 255, 255),
                v(7.0, 12.0, 255, 255, 255, 255),
            ],
            ..Default::default()
        };
        tri.texture = Some(red);
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn perspective_correct_st_uses_reciprocal_clip_w() {
        use crate::gbi::Texture;

        let mut texels = vec![0u8; 4 * 4 * 4];
        texels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let green = 20usize; // texel (1,1) in a 4-wide RGBA8888 texture
        texels[green..green + 4].copy_from_slice(&[0, 255, 0, 255]);
        let texture = Texture {
            format: 0,
            size: 2,
            width: 4,
            height: 4,
            texels: std::rc::Rc::new(texels),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let textured = |x: f32, y: f32, s: f32, t: f32, w: f32| Vertex {
            x,
            y,
            s,
            t,
            w,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            ..Default::default()
        };
        let tri = Triangle {
            v: [
                textured(0.0, 0.0, 0.0, 0.0, 1.0),
                textured(8.0, 0.0, 3.0, 3.0, 4.0),
                textured(0.0, 8.0, 0.0, 0.0, 1.0),
            ],
            texture: Some(texture),
            ..Default::default()
        };

        let mut fb = Framebuffer::new(8, 8);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle(&tri);

        let sample = 4usize * 4;
        assert_eq!(
            &fb.pixels[sample..sample + 4],
            &[255, 0, 0, 255],
            "at pixel center (4.5,0.5), reciprocal-w interpolation gives S/T≈0.73 \
             (red texel 0,0); screen-linear S/T≈1.69 incorrectly samples green 1,1"
        );
    }

    /// Fails against the pre-alpha-compare rasterizer: the transparent black
    /// half of a cutout texture used to overwrite the clear color as an opaque
    /// black box. With G_AC_THRESHOLD + blend alpha 128, it is discarded while
    /// the opaque half still draws.
    #[test]
    fn threshold_alpha_compare_cuts_out_transparent_texels() {
        use crate::gbi::Texture;

        let cutout = Texture {
            format: 0,
            size: 2,
            width: 2,
            height: 1,
            texels: std::rc::Rc::new(vec![0, 0, 0, 0, 255, 255, 255, 255]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut tri = Triangle {
            v: [
                v(0.0, 0.0, 255, 255, 255, 255),
                v(8.0, 0.0, 255, 255, 255, 255),
                v(0.0, 8.0, 255, 255, 255, 255),
            ],
            texture: Some(cutout),
            other_mode: crate::gbi::OtherMode::from_raw((6 << 9) | 0xf0, 1 | 0x10 | 0x20, 128),
            ..Default::default()
        };
        tri.v[0].s = 0.0;
        tri.v[1].s = 2.0;
        tri.v[2].s = 0.0;

        let mut fb = Framebuffer::new(8, 8);
        fb.clear(9, 8, 7, 255);
        fb.draw_triangle(&tri);

        let transparent = (fb.width + 1) as usize * 4;
        let opaque = (fb.width + 5) as usize * 4;
        assert_eq!(&fb.pixels[transparent..transparent + 4], &[9, 8, 7, 255]);
        assert_eq!(&fb.pixels[opaque..opaque + 4], &[255, 255, 255, 255]);
    }

    /// Alpha rejection must precede both color and z writes. Otherwise a
    /// transparent near cutout poisons depth and wrongly occludes opaque
    /// geometry behind it even though its color was discarded.
    #[test]
    fn rejected_alpha_does_not_update_depth() {
        let near_cutout = Triangle {
            v: [
                vz(2.0, 2.0, 1.0, 0, 0, 0, 0),
                vz(12.0, 2.0, 1.0, 0, 0, 0, 0),
                vz(7.0, 12.0, 1.0, 0, 0, 0, 0),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0xf0, 1 | 0x10 | 0x20, 128),
            ..Default::default()
        };
        let far_opaque = Triangle {
            v: [
                vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
            ..Default::default()
        };

        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle_culled(&near_cutout, CullMode::None);
        fb.draw_triangle_culled(&far_opaque, CullMode::None);
        let overlap = (6u32 * fb.width + 7) as usize * 4;
        assert_eq!(&fb.pixels[overlap..overlap + 4], &[255, 0, 0, 255]);
        assert_eq!(fb.depth[overlap / 4], 64.0);
    }

    #[test]
    fn dither_alpha_compare_produces_reproducible_stipple_without_ordered_bayer() {
        let tri = Triangle {
            v: [
                v(0.0, 0.0, 255, 255, 255, 128),
                v(16.0, 0.0, 255, 255, 255, 128),
                v(0.0, 16.0, 255, 255, 255, 128),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0, 3, 0),
            ..Default::default()
        };
        let render = || {
            let mut fb = Framebuffer::new(16, 16);
            fb.set_noise_seed(0x1234);
            fb.clear(0, 0, 0, 255);
            fb.draw_triangle(&tri);
            fb.pixels
        };
        let first = render();
        let second = render();
        assert_eq!(first, second);
        let written = first
            .chunks_exact(4)
            .filter(|px| px[..3] == [255, 255, 255])
            .count();
        assert!(
            written > 0 && written < 120,
            "half-alpha dither must stipple the covered triangle"
        );

        let mut advancing = Framebuffer::new(16, 16);
        advancing.set_noise_seed(0x1234);
        advancing.clear(0, 0, 0, 255);
        advancing.draw_triangle(&tri);
        let first_frame = advancing.pixels.clone();
        advancing.clear(0, 0, 0, 255);
        advancing.draw_triangle(&tri);
        assert_ne!(
            advancing.pixels, first_frame,
            "the noise stream must advance rather than freeze on screen coordinates"
        );
    }

    #[test]
    fn ordered_rgb_dither_tables_are_screen_registered() {
        let magic = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
        let bayer = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    ordered_rgb_dither_threshold(RgbDither::MagicSquare, x, y),
                    magic[y as usize & 3][x as usize & 3]
                );
                assert_eq!(
                    ordered_rgb_dither_threshold(RgbDither::Bayer, x, y),
                    bayer[y as usize & 3][x as usize & 3]
                );
            }
        }
        assert_eq!(ordered_rgb_dither_threshold(RgbDither::Bayer, -1, -1), 2);
    }

    #[test]
    fn ordered_rgb_dither_applies_before_color_image_format_write() {
        assert_eq!(
            apply_rgb_dither(
                [7, 6, 1, 93],
                RgbDither::MagicSquare,
                0,
                0,
                NoiseSample::ZERO,
            ),
            [8, 8, 8, 93]
        );
        assert_eq!(
            apply_rgb_dither(
                [7, 6, 1, 93],
                RgbDither::MagicSquare,
                3,
                0,
                NoiseSample::ZERO,
            ),
            [7, 6, 1, 93]
        );
        assert_eq!(
            apply_rgb_dither(
                [255, 254, 253, 93],
                RgbDither::Bayer,
                0,
                0,
                NoiseSample::ZERO,
            ),
            [255, 255, 255, 93]
        );
    }

    #[test]
    fn rgb_dither_selector_sweep_matches_every_public_threshold() {
        for mode in [
            RgbDither::MagicSquare,
            RgbDither::Bayer,
            RgbDither::Noise,
            RgbDither::Disabled,
        ] {
            for y in -4..8 {
                for x in -4..8 {
                    for noise_threshold in 0..=7 {
                        let noise = NoiseSample(noise_threshold);
                        let threshold = match mode {
                            RgbDither::MagicSquare | RgbDither::Bayer => {
                                ordered_rgb_dither_threshold(mode, x, y)
                            }
                            RgbDither::Noise => noise_threshold,
                            RgbDither::Disabled => 7,
                        };
                        for component in 0..=u8::MAX {
                            let expected =
                                if mode == RgbDither::Disabled || component & 7 <= threshold {
                                    component
                                } else {
                                    (component & !7).saturating_add(8)
                                };
                            let actual = apply_rgb_dither([component; 4], mode, x, y, noise);
                            assert_eq!(actual[..3], [expected; 3]);
                            assert_eq!(actual[3], component, "RGB dither must preserve alpha");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn alpha_pattern_uses_rgb_pattern_fallback_and_inverse_before_blending() {
        assert_eq!(
            apply_alpha_dither(
                93,
                AlphaDither::Pattern,
                RgbDither::MagicSquare,
                0,
                0,
                NoiseSample::ZERO,
            ),
            99
        );
        assert_eq!(
            apply_alpha_dither(
                93,
                AlphaDither::InversePattern,
                RgbDither::MagicSquare,
                0,
                0,
                NoiseSample::ZERO,
            ),
            90
        );
        assert_eq!(
            apply_alpha_dither(
                93,
                AlphaDither::Pattern,
                RgbDither::Disabled,
                0,
                0,
                NoiseSample::ZERO,
            ),
            99,
            "disabled RGB dither must route the standard Bayer alpha pattern"
        );
        assert_eq!(
            apply_alpha_dither(
                93,
                AlphaDither::Pattern,
                RgbDither::Noise,
                3,
                0,
                NoiseSample::ZERO,
            ),
            90,
            "RGB noise must route the magic-square alpha fallback"
        );
    }

    #[test]
    fn noise_selectors_share_one_fragment_sample_at_their_documented_widths() {
        let noise = NoiseSample(5);
        assert_eq!(
            apply_rgb_dither([6, 5, 4, 255], RgbDither::Noise, 99, 99, noise),
            [8, 5, 4, 255]
        );
        assert_eq!(
            apply_alpha_dither(6, AlphaDither::Noise, RgbDither::Disabled, 99, 99, noise),
            8
        );
        assert!(alpha_compare_value(AlphaCompare::Dither, 6, 0, noise));
        assert!(!alpha_compare_value(AlphaCompare::Dither, 4, 0, noise));
        assert!(!alpha_compare_value(
            AlphaCompare::Dither,
            0,
            0,
            NoiseSample(0)
        ));
        assert!(alpha_compare_value(
            AlphaCompare::Dither,
            255,
            0,
            NoiseSample(255)
        ));
    }

    #[test]
    fn deterministic_noise_policy_is_seeded_reproducible_and_temporally_advancing() {
        let mut first = NoiseState::default();
        let mut second = NoiseState::default();
        let a: Vec<_> = (0..64).map(|_| first.next_sample()).collect();
        let b: Vec<_> = (0..64).map(|_| second.next_sample()).collect();
        assert_eq!(a, b);
        assert!(a.windows(2).any(|pair| pair[0] != pair[1]));
        assert_ne!(first.next_sample(), a[0]);

        second.reseed(7);
        assert_ne!(second.next_sample(), a[0]);
    }

    /// Fails against the overwrite bug: a half-alpha red fragment used to
    /// replace the blue framebuffer with `[255,0,0,128]`. The standard OoT
    /// XLU tuple must evaluate IN*A_IN + MEM*(1-A), retaining both colors.
    #[test]
    fn translucent_triangle_composites_over_existing_framebuffer() {
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 255, 255);
        let tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 0, 0, 128),
                v(12.0, 2.0, 255, 0, 0, 128),
                v(7.0, 12.0, 255, 0, 0, 128),
            ],
            other_mode: OtherMode::from_raw(0xf0, 0x4000, 0),
            blender: standard_alpha_blender(1),
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        // Barycentric interpolation truncates the nominal 128 alpha to 127 at
        // this sample, so the exact source-over result is 127 red / 128 blue.
        assert_eq!(&fb.pixels[idx..idx + 4], &[127, 0, 128, 255]);
    }

    /// Cycle 2 consumes cycle 1's blender result, not the original combined
    /// color. Reusing the original red source in cycle 2 would produce
    /// `[128,0,127]` rather than retaining cycle 1's green contribution.
    #[test]
    fn two_cycle_blender_feeds_cycle_one_result_into_cycle_two() {
        let state = BlenderState {
            cycle_count: 2,
            force_blend: true,
            cycles: [
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Combined,
                    m: BlendColorInput::Blend,
                    b: BlendBInput::OneMinusA,
                },
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Combined,
                    m: BlendColorInput::Fog,
                    b: BlendBInput::OneMinusA,
                },
            ],
            blend_color: [0, 255, 0, 255],
            fog_color: [0, 0, 255, 255],
        };
        assert_eq!(
            blend_fragment(
                [255, 0, 0, 128],
                [0, 0, 0, 255],
                128,
                state,
                true,
                Coverage::FULL,
            ),
            [64, 64, 127, 255]
        );
    }

    /// The common two-cycle fog arrangement blends fog by SHADE alpha in c1,
    /// then uses a non-forced c2 P-input pass. This covers selector sources
    /// beyond the standard framebuffer-alpha tuple.
    #[test]
    fn fog_cycle_then_pass_uses_shade_alpha_and_prior_cycle_color() {
        let fog_then_pass = BlenderState {
            cycle_count: 2,
            force_blend: false,
            cycles: [
                BlendCycle {
                    p: BlendColorInput::Fog,
                    a: BlendAlphaInput::Shade,
                    m: BlendColorInput::Combined,
                    b: BlendBInput::OneMinusA,
                },
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Zero,
                    m: BlendColorInput::Combined,
                    b: BlendBInput::One,
                },
            ],
            fog_color: [255, 0, 0, 255],
            ..Default::default()
        };
        assert_eq!(
            blend_fragment(
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                64,
                fog_then_pass,
                false,
                Coverage::FULL,
            ),
            [64, 0, 191, 255]
        );
    }

    #[test]
    fn degenerate_triangle_paints_nothing() {
        let mut fb = Framebuffer::new(8, 8);
        fb.clear(1, 2, 3, 4);
        let tri = Triangle {
            v: [
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
            ],
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        assert!(!fb.has_non_uniform_content(1, 2, 3, 4));
    }

    // --- Depth / z-buffer occlusion regression ---------------------------
    //
    // These prove the z-buffer resolves overlapping geometry by DEPTH, not by
    // submission (painter's) order, and in the correct DIRECTION (nearer =
    // smaller `z` wins the `z < depth` compare, matching the OoT viewport z
    // mapping `pz = ndc_z*sz + tz` with sz>0, verified live: sz=tz=127.75,
    // ndc_z↑ with distance -> pz↑ with distance -> nearer has smaller pz).

    /// A vertex with an explicit screen-space depth `z`.
    fn vz(x: f32, y: f32, z: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
        Vertex {
            x,
            y,
            z,
            r,
            g,
            b,
            a,
            ..Default::default()
        }
    }

    /// Two fully-overlapping triangles at different depths: a NEAR blue one
    /// (z=1) and a FAR red one (z=9), covering the same pixels. The nearer
    /// (blue) color must survive at the overlap REGARDLESS of the order they
    /// are submitted -- proving z-test, not painter's order.
    #[test]
    fn nearer_triangle_wins_over_farther_regardless_of_submission_order() {
        // Same screen footprint for both; only z (and color) differ.
        let near = Triangle {
            v: [
                vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
            ..Default::default()
        };
        let far = Triangle {
            v: [
                vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
            ..Default::default()
        };
        let overlap = (6u32 * 16 + 7u32) as usize * 4; // interior pixel (7,6)

        // Order A: far first, then near. Near must overwrite far.
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle_culled(&far, CullMode::None);
        fb.draw_triangle_culled(&near, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[0, 0, 255, 255],
            "near (blue) must win at overlap when drawn AFTER far"
        );

        // Order B: near first, then far. Near must STILL win (far is z-rejected).
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle_culled(&near, CullMode::None);
        fb.draw_triangle_culled(&far, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[0, 0, 255, 255],
            "near (blue) must STILL win when drawn BEFORE far -- this is what \
             separates a real z-test from painter's order"
        );
    }

    /// The whole point of a z-buffer over painter's order: WITHOUT the depth
    /// test, submission order decides the overlap (last drawn wins), so the
    /// far triangle drawn last would incorrectly show through. This documents
    /// the difference the z-test makes and would catch a regression that
    /// silently dropped the z-test on the culled path.
    #[test]
    fn without_depth_test_painter_order_lets_farther_show_through() {
        let far = Triangle {
            v: [
                vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };
        let near = Triangle {
            v: [
                vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
            ],
            ..Default::default()
        };
        let overlap = (6u32 * 16 + 7u32) as usize * 4;
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        // No-depth path: near first, far last -> far shows through (WRONG for a
        // real scene; this is exactly the artifact the z-buffer removes).
        fb.draw_triangle_no_depth_culled(&near, CullMode::None);
        fb.draw_triangle_no_depth_culled(&far, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[255, 0, 0, 255],
            "without depth test, last-drawn (far/red) wins -- the painter's-order \
             artifact the z-buffer exists to prevent"
        );
    }

    /// Directly proves the z-test DIRECTION. `set_depth_tested` returns
    /// whether it wrote. A nearer z (smaller) must pass over an existing
    /// farther z; a farther z (larger) must be rejected. If the compare were
    /// inverted (`z > depth`), the first assert would fail -- so this test
    /// fails against a sign-flipped z-test bug.
    #[test]
    fn set_depth_tested_passes_nearer_rejects_farther() {
        let mut fb = Framebuffer::new(2, 2);
        fb.clear(0, 0, 0, 255);
        // Write a mid-depth fragment.
        assert!(fb.set_depth_tested(0, 0, 5.0, [1, 1, 1, 1]));
        // A NEARER (smaller z) fragment must PASS and overwrite.
        assert!(
            fb.set_depth_tested(0, 0, 2.0, [2, 2, 2, 2]),
            "nearer z (2 < 5) must pass -- if this fails the z-test is inverted"
        );
        assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
        // A FARTHER (larger z) fragment must be REJECTED (color unchanged).
        assert!(
            !fb.set_depth_tested(0, 0, 8.0, [3, 3, 3, 3]),
            "farther z (8 > 2) must be rejected"
        );
        assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
    }

    #[test]
    fn programmed_depth_compare_and_update_bits_are_independent() {
        let draw = |fb: &mut Framebuffer, z: f32, rgba: [u8; 4], depth: DepthControl| {
            fb.coverage[0] = Coverage::new(1);
            fb.set_depth_controlled_blended(
                0,
                0,
                DepthFragment {
                    z,
                    delta_z: 0,
                    encoded_depth: None,
                    coverage: Coverage::new(1),
                    rgba,
                    shade_alpha: 255,
                    noise: NoiseSample::ZERO,
                },
                BlenderState::default(),
                depth,
                OtherMode::default(),
            )
        };

        let mut disabled = Framebuffer::new(1, 1);
        disabled.depth[0] = 5.0;
        assert!(draw(
            &mut disabled,
            8.0,
            [8, 0, 0, 255],
            DepthControl::DISABLED
        ));
        assert_eq!(disabled.depth[0], 5.0);

        let mut update_only = Framebuffer::new(1, 1);
        update_only.depth[0] = 5.0;
        assert!(draw(
            &mut update_only,
            8.0,
            [8, 0, 0, 255],
            DepthControl {
                compare: false,
                update: true,
                ..DepthControl::DISABLED
            }
        ));
        assert_eq!(update_only.depth[0], 8.0);

        let mut compare_only = Framebuffer::new(1, 1);
        compare_only.depth[0] = 5.0;
        assert!(draw(
            &mut compare_only,
            2.0,
            [0, 2, 0, 255],
            DepthControl {
                compare: true,
                update: false,
                ..DepthControl::DISABLED
            }
        ));
        assert_eq!(compare_only.depth[0], 5.0);
        assert!(!draw(
            &mut compare_only,
            8.0,
            [8, 0, 0, 255],
            DepthControl {
                compare: true,
                update: false,
                ..DepthControl::DISABLED
            }
        ));
        assert_eq!(&compare_only.pixels[..4], &[0, 2, 0, 255]);
    }

    #[test]
    fn programmed_z_modes_distinguish_front_correlated_and_behind_fragments() {
        let draw = |mode: crate::gbi::DepthMode, z: u32| {
            let mut framebuffer = Framebuffer::new(1, 1);
            let memory = crate::depth::pack(128, 8);
            framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
            framebuffer.encoded_depth[0] = Some(memory);
            framebuffer.coverage[0] = Coverage::new(1);
            let wrote = framebuffer.set_depth_controlled_blended(
                0,
                0,
                DepthFragment {
                    z: z as f32,
                    delta_z: 4,
                    encoded_depth: Some(crate::depth::pack(z, 4)),
                    coverage: Coverage::new(1),
                    rgba: [255, 0, 0, 255],
                    shade_alpha: 255,
                    noise: NoiseSample::ZERO,
                },
                BlenderState::default(),
                DepthControl {
                    compare: true,
                    update: false,
                    mode,
                },
                OtherMode::default(),
            );
            assert_eq!(framebuffer.depth[0], 128.0, "compare-only changed Z");
            wrote
        };

        use crate::gbi::DepthMode;
        let clearly_front = 119;
        let correlated_far_side = 136;
        let clearly_behind = 137;

        assert!(draw(DepthMode::Opaque, clearly_front));
        assert!(draw(DepthMode::Opaque, correlated_far_side));
        assert!(!draw(DepthMode::Opaque, clearly_behind));

        assert!(draw(DepthMode::Interpenetrating, clearly_front));
        assert!(draw(DepthMode::Interpenetrating, correlated_far_side));
        assert!(!draw(DepthMode::Interpenetrating, clearly_behind));

        assert!(draw(DepthMode::Translucent, clearly_front));
        assert!(!draw(DepthMode::Translucent, correlated_far_side));

        assert!(!draw(DepthMode::Decal, clearly_front));
        assert!(draw(DepthMode::Decal, correlated_far_side));
        assert!(!draw(DepthMode::Decal, clearly_behind));
    }

    #[test]
    fn depth_mode_and_wrap_routing_exhaustively_preserves_supported_relations() {
        use crate::gbi::DepthMode;

        for mode in [
            DepthMode::Opaque,
            DepthMode::Interpenetrating,
            DepthMode::Translucent,
            DepthMode::Decal,
        ] {
            for coverage_wraps in [false, true] {
                for relation_bits in 0u8..16 {
                    let relations = crate::depth::DepthRelations {
                        memory_is_max: relation_bits & 1 != 0,
                        farther: relation_bits & 2 != 0,
                        nearer: relation_bits & 4 != 0,
                        in_front: relation_bits & 8 != 0,
                    };
                    let actual = depth_coverage_decision(mode, relations, coverage_wraps);
                    let expected = if mode == DepthMode::Interpenetrating && coverage_wraps {
                        DepthCoverageDecision::UnsupportedInterpenetratingCoverageAdjustment
                    } else {
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
                    };
                    assert_eq!(
                        actual, expected,
                        "depth routing differs for {mode:?}, wraps={coverage_wraps}, \
                         relations={relations:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn interpenetrating_coverage_wrap_traps_before_silently_using_opaque_routing() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut framebuffer = Framebuffer::new(1, 1);
        let memory = crate::depth::pack(128, 8);
        framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
        framebuffer.encoded_depth[0] = Some(memory);
        framebuffer.coverage[0] = Coverage::FULL;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            framebuffer.set_depth_controlled_blended(
                0,
                0,
                DepthFragment {
                    z: 119.0,
                    delta_z: 4,
                    encoded_depth: Some(crate::depth::pack(119, 4)),
                    coverage: Coverage::new(1),
                    rgba: [255, 0, 0, 255],
                    shade_alpha: 255,
                    noise: NoiseSample::ZERO,
                },
                BlenderState::default(),
                DepthControl {
                    compare: true,
                    update: false,
                    mode: crate::gbi::DepthMode::Interpenetrating,
                },
                OtherMode::from_raw(0, 0x0110, 0),
            );
        }))
        .expect_err("wrapping ZMODE_INTER must trap before rendering");
        let panic = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("unsupported raster panic payload must be text");
        assert!(panic.contains(
            "ZMODE_INTER coverage wrap requires unsupported interpenetration coverage adjustment"
        ));
        assert!(panic.contains("pixel_coverage=1 memory_coverage=8"));

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Render
        );
        assert_eq!(
            events[0].operation,
            "render.reference.raster.interpenetration-coverage-adjustment"
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0]
            .context
            .contains("pixel_coverage=1 memory_coverage=8"));
    }

    #[test]
    fn raw_coverage_uses_the_public_eight_sample_checkerboard_mask() {
        let vertical_strip = |left: f32, right: f32| crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 4,
            ym: 2,
            yh: 0,
            xl: (left * 65536.0) as i32,
            dxldy: 0,
            xh: (right * 65536.0) as i32,
            dxhdy: 0,
            xm: (left * 65536.0) as i32,
            dxmdy: 0,
        };
        let scissor = ScissorRect {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            field: false,
            keep_odd: false,
        };
        assert_eq!(
            raw_pixel_coverage(vertical_strip(0.0, 1.0), scissor, 0, 0),
            CoverageMask(0xff)
        );
        let left = raw_pixel_coverage(vertical_strip(0.0, 0.5), scissor, 0, 0);
        let right = raw_pixel_coverage(vertical_strip(0.5, 1.0), scissor, 0, 0);
        assert_eq!(left, CoverageMask(0x55));
        assert_eq!(right, CoverageMask(0xaa));
        assert_eq!(left.0 | right.0, u8::MAX);
        assert_eq!(left.0 & right.0, 0);

        let top_half = crate::gbi::RdpEdgeCoefficients {
            yl: 2,
            ym: 1,
            ..vertical_strip(0.0, 1.0)
        };
        let bottom_half = crate::gbi::RdpEdgeCoefficients {
            yh: 2,
            ym: 3,
            ..vertical_strip(0.0, 1.0)
        };
        let top = raw_pixel_coverage(top_half, scissor, 0, 0);
        let bottom = raw_pixel_coverage(bottom_half, scissor, 0, 0);
        assert_eq!(top, CoverageMask(0x0f));
        assert_eq!(bottom, CoverageMask(0xf0));
        assert_eq!(top.0 | bottom.0, u8::MAX);
        assert_eq!(top.0 & bottom.0, 0);
    }

    #[test]
    fn raw_coverage_axis_aligned_boundaries_exhaustively_preserve_sample_identity() {
        let full_scissor = ScissorRect::framebuffer(1, 1);
        let edge = |left_eighth: i32, right_eighth: i32, top_quarter: i16, bottom_quarter: i16| {
            crate::gbi::RdpEdgeCoefficients {
                right_major: true,
                level: 0,
                tile: 0,
                yl: bottom_quarter,
                ym: top_quarter,
                yh: top_quarter,
                xl: left_eighth * (Q16_ONE as i32 / 8),
                dxldy: 0,
                xh: right_eighth * (Q16_ONE as i32 / 8),
                dxhdy: 0,
                xm: left_eighth * (Q16_ONE as i32 / 8),
                dxmdy: 0,
            }
        };

        for top_quarter in 0..=4 {
            for bottom_quarter in top_quarter..=4 {
                for left_eighth in 0..=8 {
                    for right_eighth in left_eighth..=8 {
                        let actual = raw_pixel_coverage(
                            edge(left_eighth, right_eighth, top_quarter, bottom_quarter),
                            full_scissor,
                            0,
                            0,
                        );
                        let expected = CoverageMask::from_samples(|sample_x, sample_y| {
                            sample_x >= left_eighth
                                && sample_x < right_eighth
                                && sample_y >= i32::from(top_quarter) * 2
                                && sample_y < i32::from(bottom_quarter) * 2
                        });
                        assert_eq!(
                            actual, expected,
                            "raw coverage identity differs for x [{left_eighth}/8, {right_eighth}/8), y [{top_quarter}/4, {bottom_quarter}/4)"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn raw_coverage_q16_lsb_sweep_preserves_every_checkerboard_boundary() {
        let edge = |left_q16: i32, right_q16: i32| crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 4,
            ym: 0,
            yh: 0,
            xl: left_q16,
            dxldy: 0,
            xh: right_q16,
            dxhdy: 0,
            xm: left_q16,
            dxmdy: 0,
        };
        let scissor = ScissorRect::framebuffer(1, 1);

        for (sample_index, &(x_eighth, _)) in COVERAGE_SAMPLES.iter().enumerate() {
            let sample_q16 = x_eighth * (Q16_ONE as i32 / 8);
            for delta_lsb in -1..=1 {
                let left = raw_pixel_coverage(
                    edge(sample_q16 + delta_lsb, 2 * Q16_ONE as i32),
                    scissor,
                    0,
                    0,
                );
                assert_eq!(
                    left.contains(sample_index),
                    delta_lsb <= 0,
                    "left-inclusive edge differs for sample {sample_index} at {delta_lsb:+} Q16 LSB"
                );

                let right = raw_pixel_coverage(
                    edge(-(Q16_ONE as i32), sample_q16 + delta_lsb),
                    scissor,
                    0,
                    0,
                );
                assert_eq!(
                    right.contains(sample_index),
                    delta_lsb > 0,
                    "right-exclusive edge differs for sample {sample_index} at {delta_lsb:+} Q16 LSB"
                );
            }
        }
    }

    #[test]
    fn raw_coverage_scissor_boundaries_exhaustively_preserve_sample_identity() {
        let full_pixel = crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 4,
            ym: 0,
            yh: 0,
            xl: 0,
            dxldy: 0,
            xh: Q16_ONE as i32,
            dxhdy: 0,
            xm: 0,
            dxmdy: 0,
        };
        for top_quarter in 0..=4 {
            for bottom_quarter in top_quarter..=4 {
                for left_quarter in 0..=4 {
                    for right_quarter in left_quarter..=4 {
                        let scissor = ScissorRect {
                            ulx: left_quarter as f32 / 4.0,
                            uly: top_quarter as f32 / 4.0,
                            lrx: right_quarter as f32 / 4.0,
                            lry: bottom_quarter as f32 / 4.0,
                            field: false,
                            keep_odd: false,
                        };
                        let actual = raw_pixel_coverage(full_pixel, scissor, 0, 0);
                        let expected = CoverageMask::from_samples(|sample_x, sample_y| {
                            sample_x >= left_quarter * 2
                                && sample_x < right_quarter * 2
                                && sample_y >= top_quarter * 2
                                && sample_y < bottom_quarter * 2
                        });
                        assert_eq!(
                            actual, expected,
                            "raw scissor identity differs for x [{left_quarter}/4, {right_quarter}/4), y [{top_quarter}/4, {bottom_quarter}/4)"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn high_level_shared_edge_assigns_each_checkerboard_sample_once() {
        let vertex = |x, y| Vertex {
            x,
            y,
            ..Vertex::default()
        };
        let upper_right = [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(1.0, 1.0)];
        let lower_left = [vertex(0.0, 0.0), vertex(1.0, 1.0), vertex(0.0, 1.0)];
        let scissor = ScissorRect::framebuffer(1, 1);
        let coverage = |vertices: [Vertex; 3]| {
            triangle_pixel_coverage(
                vertices,
                edge(vertices[0], vertices[1], vertices[2]),
                scissor,
                0,
                0,
            )
        };

        let first = coverage(upper_right);
        let second = coverage(lower_left);
        assert_eq!(first, CoverageMask(0xaf));
        assert_eq!(second, CoverageMask(0x50));
        assert_eq!(first.0 | second.0, u8::MAX);
        assert_eq!(first.0 & second.0, 0);
        assert_eq!(
            first.coverage().count() + second.coverage().count(),
            Coverage::FULL.count()
        );
        assert_eq!(first.coverage(), Coverage::new(6));
        assert_eq!(second.coverage(), Coverage::new(2));

        let reversed = |[a, b, c]: [Vertex; 3]| [a, c, b];
        assert_eq!(coverage(reversed(upper_right)), first);
        assert_eq!(coverage(reversed(lower_left)), second);
    }

    #[test]
    fn covered_attribute_sample_exhausts_every_nonzero_mask() {
        for bits in 1u16..=u16::from(u8::MAX) {
            let mask = CoverageMask(bits as u8);
            let actual = mask.attribute_sample();
            if bits == u16::from(u8::MAX) {
                assert_eq!(actual, AttributeSamplePoint::PixelCenter);
                continue;
            }

            let AttributeSamplePoint::Covered(actual) = actual else {
                panic!("partial coverage {bits:#04x} selected pixel center");
            };
            assert!(mask.contains(usize::from(actual.sample_index)));
            assert_eq!(
                COVERAGE_SAMPLES[usize::from(actual.sample_index)],
                (actual.x_eighth, actual.y_eighth)
            );
            let expected_index = COVERAGE_SAMPLES
                .iter()
                .enumerate()
                .filter(|(index, _)| mask.contains(*index))
                .min_by_key(|(_, &(x, y))| {
                    let dx = x - 4;
                    let dy = y - 4;
                    dx * dx + dy * dy
                })
                .map(|(index, _)| index)
                .unwrap();
            assert_eq!(usize::from(actual.sample_index), expected_index);
        }
    }

    #[test]
    fn partial_attribute_sample_policy_exhausts_every_equal_distance_tie() {
        let equal_distance_groups: [&[usize]; 3] = [&[2, 5], &[1, 3, 4, 6], &[0, 7]];
        for group in equal_distance_groups {
            let first = COVERAGE_SAMPLES[group[0]];
            let first_distance = (first.0 - 4).pow(2) + (first.1 - 4).pow(2);
            assert!(group.iter().all(|&index| {
                let sample = COVERAGE_SAMPLES[index];
                (sample.0 - 4).pow(2) + (sample.1 - 4).pow(2) == first_distance
            }));

            for subset in 1usize..(1usize << group.len()) {
                let bits = group
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| subset & (1usize << position) != 0)
                    .fold(0u8, |bits, (_, &sample_index)| bits | (1u8 << sample_index));
                let expected = group
                    .iter()
                    .enumerate()
                    .find(|(position, _)| subset & (1usize << position) != 0)
                    .map(|(_, &sample_index)| sample_index)
                    .expect("nonempty tie subset lost every sample");
                let AttributeSamplePoint::Covered(actual) = CoverageMask(bits).attribute_sample()
                else {
                    panic!("partial tie mask {bits:#04x} selected pixel center");
                };
                assert_eq!(usize::from(actual.sample_index), expected);
            }
        }
    }

    #[test]
    fn full_coverage_attributes_preserve_pixel_center() {
        let full = CoverageMask::from_samples(|_, _| true);
        assert_eq!(full, CoverageMask(u8::MAX));
        assert_eq!(full.attribute_sample(), AttributeSamplePoint::PixelCenter);
        assert_eq!(full.attribute_sample().offsets_eighth(), (4, 4));
    }

    #[test]
    #[should_panic(expected = "zero coverage has no attribute sample")]
    fn zero_coverage_has_no_attribute_sample() {
        CoverageMask(0).attribute_sample();
    }

    #[test]
    fn raw_and_high_level_partial_attributes_use_the_shared_covered_sample() {
        let raw_edge = crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 4,
            ym: 0,
            yh: 0,
            xl: 0,
            dxldy: 0,
            xh: Q16_ONE as i32 / 2,
            dxhdy: 0,
            xm: 0,
            dxmdy: 0,
        };
        let raw_mask = raw_pixel_coverage(raw_edge, ScissorRect::framebuffer(1, 1), 0, 0);
        assert_eq!(raw_mask, CoverageMask(0x55));
        assert_eq!(
            raw_mask.attribute_sample(),
            AttributeSamplePoint::Covered(CoveredAttributeSample {
                sample_index: 2,
                x_eighth: 3,
                y_eighth: 3,
            })
        );
        let raw = RawRdpTriangle {
            edge: raw_edge,
            shade: Some(crate::gbi::RdpShadeCoefficients {
                // The plane is red=100+8*x. At the selected x=3/8 the
                // result is 103; the old pixel-center path produced 104.
                color: [104 << 16, 0, 0, 255 << 16],
                dcdx: [8 << 16, 0, 0, 0],
                dcde: [0; 4],
                dcdy: [0; 4],
            }),
            texture_coefficients: None,
            z: Some(crate::gbi::RdpZCoefficients {
                z: 104 << 16,
                dzdx: 8 << 16,
                dzde: 0,
                dzdy: 0,
            }),
            texture: None,
            other_mode: OtherMode::from_raw(OtherMode::default().raw_high(), 0x20, 0),
            combiner: shade_only_combiner(),
            blender: BlenderState::default(),
            scissor: None,
        };
        let mut raw_framebuffer = Framebuffer::new(1, 1);
        raw_framebuffer.draw_raw_rdp_triangle(&raw);
        assert_eq!(&raw_framebuffer.pixels[..4], &[103, 0, 0, 255]);
        let selected_depth = crate::depth::pack(103 * 8, 8 * 8);
        assert_eq!(raw_framebuffer.encoded_depth[0], Some(selected_depth));

        let mut high_vertices = [
            v(-10.0, -10.0, 20, 0, 0, 255),
            v(0.5, -10.0, 104, 0, 0, 255),
            v(0.5, 10.0, 104, 0, 0, 255),
        ];
        high_vertices[0].z = 20.0;
        high_vertices[1].z = 104.0;
        high_vertices[2].z = 104.0;
        let high = Triangle {
            v: high_vertices,
            other_mode: OtherMode::from_raw(OtherMode::default().raw_high(), 0x20, 0),
            combiner: shade_only_combiner(),
            ..Triangle::default()
        };
        let high_mask = triangle_pixel_coverage(
            high.v,
            edge(high.v[0], high.v[1], high.v[2]),
            ScissorRect::framebuffer(1, 1),
            0,
            0,
        );
        assert_eq!(high_mask, raw_mask);
        let mut high_framebuffer = Framebuffer::new(1, 1);
        high_framebuffer.draw_triangle_culled(&high, CullMode::None);
        assert_eq!(&high_framebuffer.pixels[..4], &[103, 0, 0, 255]);
        assert_eq!(high_framebuffer.encoded_depth[0], Some(selected_depth));
    }

    #[test]
    fn raw_and_high_level_partial_texture_coordinates_use_the_shared_covered_sample() {
        let raw_edge = crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 4,
            ym: 0,
            yh: 0,
            xl: 0,
            dxldy: 0,
            xh: Q16_ONE as i32 / 2,
            dxhdy: 0,
            xm: 0,
            dxmdy: 0,
        };
        let texture = crate::gbi::Texture {
            format: 0,
            size: 2,
            width: 5,
            height: 1,
            texels: std::rc::Rc::new(
                [10u8, 20, 30, 40, 50]
                    .into_iter()
                    .flat_map(|red| [red, 0, 0, 255])
                    .collect(),
            ),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let texel_combiner = repeated_state(
            texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0),
            [0; 4],
            [0; 4],
        );
        let raw = RawRdpTriangle {
            edge: raw_edge,
            shade: None,
            texture_coefficients: Some(crate::gbi::RdpTextureCoefficients {
                // S=8*x: selected x=3/8 samples texel 3, while the old
                // pixel-center path sampled texel 4.
                stw: [4 << 16, 0, 1 << 16],
                dstdx: [8 << 16, 0, 0],
                dstde: [0; 3],
                dstdy: [0; 3],
            }),
            z: None,
            texture: Some(texture.clone()),
            other_mode: OtherMode::default(),
            combiner: texel_combiner,
            blender: BlenderState::default(),
            scissor: None,
        };
        let mut raw_framebuffer = Framebuffer::new(1, 1);
        raw_framebuffer.draw_raw_rdp_triangle(&raw);
        assert_eq!(&raw_framebuffer.pixels[..4], &[40, 0, 0, 255]);

        let mut high_vertices = [
            v(-10.0, -10.0, 255, 255, 255, 255),
            v(0.5, -10.0, 255, 255, 255, 255),
            v(0.5, 10.0, 255, 255, 255, 255),
        ];
        high_vertices[0].s = -80.0;
        high_vertices[1].s = 4.0;
        high_vertices[2].s = 4.0;
        let high = Triangle {
            v: high_vertices,
            texture: Some(texture),
            combiner: texel_combiner,
            ..Triangle::default()
        };
        let mut high_framebuffer = Framebuffer::new(1, 1);
        high_framebuffer.draw_triangle(&high);
        assert_eq!(&high_framebuffer.pixels[..4], &[40, 0, 0, 255]);
    }

    #[test]
    fn shared_edge_attribute_samples_stay_on_their_own_triangle() {
        let vertex = |x: f32, y: f32| v(x, y, (100.0 + 64.0 * x + 32.0 * y) as u8, 0, 0, 255);
        let triangles = [
            [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(1.0, 1.0)],
            [vertex(0.0, 0.0), vertex(1.0, 1.0), vertex(0.0, 1.0)],
        ];
        let expected = [
            (CoverageMask(0xaf), [136, 0, 0, 255]),
            (CoverageMask(0x50), [128, 0, 0, 255]),
        ];

        for (vertices, (expected_mask, expected_pixel)) in triangles.into_iter().zip(expected) {
            let area = edge(vertices[0], vertices[1], vertices[2]);
            let mask =
                triangle_pixel_coverage(vertices, area, ScissorRect::framebuffer(1, 1), 0, 0);
            assert_eq!(mask, expected_mask);
            let AttributeSamplePoint::Covered(sample) = mask.attribute_sample() else {
                panic!("shared-edge partial mask selected pixel center");
            };
            assert!(mask.contains(usize::from(sample.sample_index)));
            let point = Vertex {
                x: sample.x_eighth as f32 / 8.0,
                y: sample.y_eighth as f32 / 8.0,
                ..Vertex::default()
            };
            let signs = [
                edge(vertices[1], vertices[2], point),
                edge(vertices[2], vertices[0], point),
                edge(vertices[0], vertices[1], point),
            ];
            assert!(signs.iter().all(|value| *value * area >= 0.0));

            let mut framebuffer = Framebuffer::new(1, 1);
            framebuffer.draw_triangle(&Triangle {
                v: vertices,
                combiner: shade_only_combiner(),
                ..Triangle::default()
            });
            assert_eq!(&framebuffer.pixels[..4], &expected_pixel);
        }
    }

    #[test]
    fn raw_edges_use_the_public_preceding_scanline_reference_point() {
        let edge = crate::gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 8,
            ym: 4,
            yh: 1,
            xl: 0,
            dxldy: 0,
            xh: 1 << 16,
            dxhdy: 1 << 16,
            xm: 0,
            dxmdy: 0,
        };
        let (_, major) = raw_span_edges_at_y_eighth(edge, 1);
        assert_eq!(
            major,
            (1 << 16) + (1 << 13),
            "XH at YH=.25 is referenced to scanline zero, so y=.125 adds one eighth of the slope"
        );
    }

    #[test]
    fn raw_attribute_plane_keeps_fractional_terms_in_fixed_point() {
        let value = raw_attribute_plane(
            (10 << 16) + (1 << 14),
            -(1 << 14),
            1 << 15,
            3,
            Q16_ONE + Q16_ONE / 2,
        );
        assert_eq!(value, (10 << 16) + (1 << 12));
    }

    #[test]
    fn coverage_destinations_follow_public_clamp_wrap_full_and_save_rules() {
        let mode = |low| OtherMode::from_raw(0, low, 0);
        let pixel = Coverage::new(3);
        let memory = Coverage::new(5);

        let clamp_blend = coverage_result(pixel, memory, mode(0x0008));
        assert!(clamp_blend.blend_enabled);
        assert!(!clamp_blend.wraps);
        assert_eq!(clamp_blend.destination, Coverage::FULL);

        let clamp_new = coverage_result(pixel, Coverage::new(6), mode(0x0008));
        assert!(!clamp_new.blend_enabled);
        assert!(clamp_new.wraps);
        assert_eq!(clamp_new.destination, pixel);

        let force_clamp = coverage_result(pixel, Coverage::new(6), mode(0x4000));
        assert!(force_clamp.blend_enabled);
        assert_eq!(force_clamp.destination, Coverage::FULL);

        let wrap_at_unity = coverage_result(pixel, memory, mode(0x0100));
        assert!(!wrap_at_unity.wraps);
        assert_eq!(wrap_at_unity.destination, Coverage::FULL);
        let wrap_over_unity = coverage_result(pixel, Coverage::new(6), mode(0x0100));
        assert!(wrap_over_unity.wraps);
        assert_eq!(wrap_over_unity.destination, Coverage::new(1));

        assert_eq!(
            coverage_result(pixel, memory, mode(0x0200)).destination,
            Coverage::FULL
        );
        assert_eq!(
            coverage_result(pixel, memory, mode(0x0300)).destination,
            memory
        );
    }

    #[test]
    fn coverage_alpha_combiner_can_reduce_a_fragment_to_zero_samples() {
        let multiply_and_select = OtherMode::from_raw(0, 0x3000, 0);
        let (rgba, coverage) =
            apply_coverage_alpha(multiply_and_select, [1, 2, 3, 128], Coverage::FULL);
        assert_eq!(coverage, Coverage::new(4));
        assert_eq!(rgba[3], 128);

        let (rgba, coverage) =
            apply_coverage_alpha(multiply_and_select, [1, 2, 3, 0], Coverage::FULL);
        assert_eq!(coverage.count(), 0);
        assert_eq!(rgba[3], 0);
    }

    #[test]
    fn alpha_coverage_hardware_probe_fixture_distinguishes_rounding_hypotheses() {
        // Programming Manual 15.5.4 and 15.7 prove the normalized product and
        // selector topology, but do not publish the multiplier width,
        // normalization denominator, quantizer, or tie rule. These synthetic
        // inputs are an inventory for a raw-DPC hardware capture: the output
        // coverage channel distinguishes the four common integer hypotheses
        // without treating any one of them as silicon evidence.
        let probes = [
            // coverage, alpha, nearest/255, nearest/256, truncate/255,
            // truncate/256
            (8u8, 15u8, 0u8, 0u8, 0u8, 0u8),
            (8, 16, 1, 1, 0, 0),
            (3, 212, 2, 2, 2, 2),
            (3, 213, 3, 2, 2, 2),
            (8, 254, 8, 8, 7, 7),
            (8, 255, 8, 8, 8, 7),
        ];

        for (coverage, alpha, nearest_255, nearest_256, truncate_255, truncate_256) in probes {
            let product = u16::from(coverage) * u16::from(alpha);
            assert_eq!(((product + 127) / 255) as u8, nearest_255);
            assert_eq!(((product + 128) / 256) as u8, nearest_256);
            assert_eq!((product / 255) as u8, truncate_255);
            assert_eq!((product / 256) as u8, truncate_256);
        }

        assert_eq!(Coverage::FULL.times_alpha(15), Coverage::new(0));
        assert_eq!(Coverage::FULL.times_alpha(16), Coverage::new(1));
        assert_eq!(Coverage::new(3).times_alpha(212), Coverage::new(2));
        assert_eq!(Coverage::new(3).times_alpha(213), Coverage::new(3));
        assert_eq!(Coverage::FULL.times_alpha(254), Coverage::FULL);
        assert_eq!(Coverage::FULL.times_alpha(255), Coverage::FULL);
    }

    #[test]
    fn alpha_coverage_threshold_sweep_fixture_records_the_current_reference_codes() {
        // With ALPHA_CVG_SEL enabled and CVG_X_ALPHA disabled, a synthetic
        // G_AC_THRESHOLD sweep can recover the coverage-to-alpha code without
        // involving blender arithmetic: the largest passing threshold is the
        // selected alpha. These are the current normalized-u8 reference codes,
        // not a claim about the unpublished five-bit silicon path.
        let selected_alpha = [32u8, 64, 96, 128, 159, 191, 223, 255];
        for (index, expected) in selected_alpha.into_iter().enumerate() {
            let coverage = Coverage::new(index as u8 + 1);
            let selected = coverage.alpha();
            let passes_threshold = |threshold| selected >= threshold;
            assert_eq!(selected, expected);
            assert!(passes_threshold(expected));
            if expected < u8::MAX {
                assert!(!passes_threshold(expected + 1));
            }
        }
    }

    #[test]
    fn alpha_coverage_current_policy_is_bounded_monotonic_and_endpoint_exact() {
        for coverage in 0..=Coverage::FULL.count() {
            let coverage = Coverage::new(coverage);
            assert_eq!(coverage.times_alpha(0), Coverage::new(0));
            assert_eq!(coverage.times_alpha(u8::MAX), coverage);

            let mut previous = Coverage::new(0);
            for alpha in 0..=u8::MAX {
                let current = coverage.times_alpha(alpha);
                assert!(current.count() <= coverage.count());
                assert!(current.count() >= previous.count());
                previous = current;
            }
        }
    }

    #[test]
    fn alpha_coverage_selector_precedes_both_one_and_two_cycle_blenders() {
        let one_cycle = OtherMode::from_raw(0, 0x3000, 0);
        let two_cycle = OtherMode::from_raw(1 << 20, 0x3000, 0);
        assert_eq!(one_cycle.cycle_type(), CycleType::OneCycle);
        assert_eq!(two_cycle.cycle_type(), CycleType::TwoCycle);

        let input = [1, 2, 3, 213];
        let coverage = Coverage::new(3);
        let one = apply_coverage_alpha(one_cycle, input, coverage);
        let two = apply_coverage_alpha(two_cycle, input, coverage);
        assert_eq!(one, two);
        assert_eq!(one, ([1, 2, 3, 96], Coverage::new(3)));
    }

    #[test]
    fn clear_on_coverage_updates_coverage_even_when_it_inhibits_color() {
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.clear(0, 0, 255, 255);
        framebuffer.coverage[0] = Coverage::new(3);
        let mode = OtherMode::from_raw(0, 0x4180, 0);

        assert!(!framebuffer.set_blended(
            0,
            0,
            ColorFragment {
                rgba: [255, 0, 0, 255],
                coverage: Coverage::new(4),
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            mode,
        ));
        assert_eq!(framebuffer.coverage[0], Coverage::new(7));
        assert_eq!(&framebuffer.pixels[..4], &[0, 0, 255, 255]);

        assert!(framebuffer.set_blended(
            0,
            0,
            ColorFragment {
                rgba: [255, 0, 0, 255],
                coverage: Coverage::new(2),
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            mode,
        ));
        assert_eq!(framebuffer.coverage[0], Coverage::new(1));
        assert_eq!(&framebuffer.pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn opaque_coverage_wrap_replaces_delta_range_with_strict_front_test() {
        let draw = |memory_coverage: Coverage| {
            let mut framebuffer = Framebuffer::new(1, 1);
            let memory = crate::depth::pack(128, 8);
            framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
            framebuffer.encoded_depth[0] = Some(memory);
            framebuffer.coverage[0] = memory_coverage;
            framebuffer.set_depth_controlled_blended(
                0,
                0,
                DepthFragment {
                    z: 136.0,
                    delta_z: 4,
                    encoded_depth: Some(crate::depth::pack(136, 4)),
                    coverage: Coverage::new(1),
                    rgba: [255, 0, 0, 255],
                    shade_alpha: 255,
                    noise: NoiseSample::ZERO,
                },
                BlenderState::default(),
                DepthControl {
                    compare: true,
                    update: false,
                    mode: crate::gbi::DepthMode::Opaque,
                },
                OtherMode::from_raw(0, 0x0110, 0),
            )
        };

        assert!(draw(Coverage::new(1)), "correlated non-wrap must pass");
        assert!(
            !draw(Coverage::FULL),
            "wrapped coverage must require the pixel to be strictly in front"
        );
    }

    #[test]
    fn raw_left_major_edge_selects_commanded_span_sides() {
        let major_slope = -(5.0f32 / 6.0 * 65536.0).round() as i32;
        let lower_slope = -(5.0f32 / 3.0 * 65536.0).round() as i32;
        let triangle = RawRdpTriangle {
            edge: crate::gbi::RdpEdgeCoefficients {
                right_major: false,
                level: 0,
                tile: 0,
                yl: 7 * 4,
                ym: 4 * 4,
                yh: 4,
                xl: 6 << 16,
                dxldy: lower_slope,
                xh: 6 << 16,
                dxhdy: major_slope,
                xm: 6 << 16,
                dxmdy: 0,
            },
            shade: None,
            texture_coefficients: None,
            z: None,
            texture: None,
            other_mode: OtherMode::default(),
            combiner: CombinerState {
                primitive: [255; 4],
                ..CombinerState::default()
            },
            blender: BlenderState::default(),
            scissor: None,
        };
        let edge = triangle.edge;
        let mut framebuffer = Framebuffer::new(8, 8);

        framebuffer.draw_raw_rdp_triangle(&triangle);

        let pixel = |x: usize, y: usize| {
            let offset = (y * 8 + x) * 4;
            &framebuffer.pixels[offset..offset + 4]
        };
        assert_eq!(pixel(3, 4), &[255, 255, 255, 255]);
        assert!(
            raw_pixel_coverage(
                edge,
                ScissorRect {
                    ulx: 0.0,
                    uly: 0.0,
                    lrx: 8.0,
                    lry: 8.0,
                    field: false,
                    keep_odd: false,
                },
                2,
                4,
            )
            .coverage()
            .count()
                > 0
        );
        assert_eq!(pixel(2, 4), &[255, 255, 255, 255]);
        assert_eq!(pixel(1, 4), &[0, 0, 0, 0]);
    }
}
