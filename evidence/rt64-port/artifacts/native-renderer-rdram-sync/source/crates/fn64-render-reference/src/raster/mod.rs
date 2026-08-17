//! Deterministic software rasterization into an RGBA8888 working framebuffer.
//! F3DEX2 geometry uses the existing barycentric path. Raw RDP triangles retain
//! their major/minor edge and attribute coefficient planes and walk commanded
//! spans directly with the public eight-sample checkerboard coverage mask.
//! Fixed-width edge/attribute accumulator truncation remains an explicit
//! fidelity gap.

use crate::depth::EncodedDepth;
use crate::gbi::{
    BlenderState, ColorImageLayout, CombinerState, Line, OtherMode, PrimitiveDepth, ScissorRect, Vertex,
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


mod coverage;
mod combiner;
mod blend;
mod draw;

use coverage::*;
use blend::*;

/// TEMP instrumentation (env `FN64_DUMP_PROJ=1`): count z-test passes vs
/// rejections so a real overlapping-geometry frame can PROVE the z-buffer is
/// doing occlusion work (rejecting farther fragments) rather than being a
/// no-op. Gated entirely behind the env var; call `zstat::summary()` after a
/// frame to print + reset. Remove/keep behind the flag.
pub mod zstat {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INIT: AtomicBool = AtomicBool::new(false);
    static PASS: AtomicU64 = AtomicU64::new(0);
    static REJECT: AtomicU64 = AtomicU64::new(0);
    fn on() -> bool {
        if crate::speculative_observations_suppressed() {
            return false;
        }
        #[cfg(test)]
        {
            return ENABLED.load(Ordering::Relaxed);
        }
        #[cfg(not(test))]
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(crate::debug_flag("FN64_DUMP_PROJ"), Ordering::Relaxed);
        }
        #[cfg(not(test))]
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

    #[cfg(test)]
    pub(crate) fn test_enable_and_reset() {
        ENABLED.store(true, Ordering::Relaxed);
        INIT.store(true, Ordering::Relaxed);
        PASS.store(0, Ordering::Relaxed);
        REJECT.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn test_counts() -> (u64, u64) {
        (
            PASS.load(Ordering::Relaxed),
            REJECT.load(Ordering::Relaxed),
        )
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

pub(super) fn raw_span_edges_at_y_eighth(
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
    // `lft` (command bit 55): 1 = LEFT-major -- the H (major) edge walks the
    // triangle's LEFT side and spans run major -> minor; 0 mirrors it.
    // Verified against WM2000's live title-scene stream (task #783): its
    // rect-split tris carry lft=1 with a constant XH on the left and XM/XL
    // marching right. The previous reading was inverted, which made every
    // real triangle's span come back right < left = empty -- raw RDP
    // geometry decoded but never rasterized a single pixel.
    if edge.left_major {
        (major_x, minor_x)
    } else {
        (minor_x, major_x)
    }
}

pub(super) fn raw_pixel_coverage(
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

/// As [`raw_pixel_coverage`], but for a caller that has already evaluated
/// `raw_span_edges_at_y_eighth` for this row's four sample offsets (in
/// [`COVERAGE_SAMPLES`] Y order: 1, 3, 5, 7 eighths).
///
/// `draw_raw_rdp_triangle_impl`'s outer scanline loop computes exactly these
/// four edges already, to find the row's covered X range before walking its
/// pixels. `raw_pixel_coverage` used unmodified would recompute the same
/// four `fixed_mul_ratio` evaluations from scratch for every pixel in the
/// row via `from_samples`' per-sample closure -- for an 8-sample mask that
/// shares each Y offset across two X samples, that is every edge evaluated
/// twice per pixel that was already known. A per-call cache inside
/// `raw_pixel_coverage` was tried and measured as a net loss (live `sample`,
/// WM2000): a 4-entry lookup checked on every one of 8 samples costs more
/// than the two cheap i64 multiplies it replaces. Accepting the row's
/// answers as a parameter instead removes the recompute AND the cache
/// entirely, at the caller that actually has them for free.
pub(super) fn raw_pixel_coverage_with_row_edges(
    scissor: ScissorRect,
    x: i32,
    y: i32,
    yh_eighth: i32,
    yl_eighth: i32,
    row_edges: [(i64, i64); 4],
) -> CoverageMask {
    if !scissor.line_enabled(y) {
        return CoverageMask::default();
    }
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
        // COVERAGE_SAMPLES' Y offsets are [1,1,3,3,5,5,7,7]; row_edges is
        // indexed in that same order, one entry per distinct offset.
        let row_index = match offset_y {
            1 => 0,
            3 => 1,
            5 => 2,
            7 => 3,
            other => unreachable!("coverage sample Y offset {other} outside the public checkerboard"),
        };
        let (left_x, right_x) = row_edges[row_index];
        let sample_x = i64::from(sample_x_eighth) * Q16_ONE / 8;
        sample_x >= left_x && sample_x < right_x
    })
}

pub(super) fn triangle_pixel_coverage(
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

pub(super) fn line_parameter_and_distance_squared(a: Vertex, b: Vertex, x: f32, y: f32) -> (f32, f32) {
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

pub(super) fn line_pixel_coverage(line: &Line, scissor: ScissorRect, x: i32, y: i32) -> CoverageMask {
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
pub(super) fn test_attribute_sample(mask: CoverageMask) -> (u8, Option<(u8, i32, i32)>) {
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
pub(crate) fn test_triangle_attribute_sample(
    vertices: [Vertex; 3],
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> (u8, Option<(u8, i32, i32)>) {
    let area = edge(vertices[0], vertices[1], vertices[2]);
    test_attribute_sample(triangle_pixel_coverage(vertices, area, scissor, x, y))
}

#[cfg(test)]
pub(crate) fn test_raw_attribute_sample(
    edge: crate::gbi::RdpEdgeCoefficients,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> (u8, Option<(u8, i32, i32)>) {
    test_attribute_sample(raw_pixel_coverage(edge, scissor, x, y))
}

#[cfg(test)]
pub(crate) fn test_line_attribute_sample(
    line: &Line,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> (u8, Option<(u8, i32, i32)>) {
    test_attribute_sample(line_pixel_coverage(line, scissor, x, y))
}


#[cfg(test)]
mod tests;
