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
    /// Per-pixel 3-bit RGB dither value drawn at fragment time (see
    /// [`Framebuffer::fragment_dither`]), applied to the blender output
    /// just before the pixel write. `None` = RGB dithering disabled.
    rgb_dither: Option<u8>,
}

#[derive(Copy, Clone)]
struct ColorFragment {
    rgba: [u8; 4],
    coverage: Coverage,
    shade_alpha: u8,
    /// See [`DepthFragment::rgb_dither`].
    rgb_dither: Option<u8>,
}

/// One fragment's dither draw: the RDP computes a single per-pixel dither
/// value for the RGB channels and one for alpha, and the public gbi.h
/// couples them -- `G_AD_PATTERN` reuses the RGB stage's per-pixel value and
/// `G_AD_NOTPATTERN` its 3-bit complement. Drawing both in one place keeps
/// that coupling explicit instead of spreading generator state across the
/// pre-blend (alpha) and post-blend (RGB) pipeline stages.
#[derive(Copy, Clone)]
struct FragmentDither {
    rgb: Option<u8>,
    alpha: Option<u8>,
}

impl FragmentDither {
    /// Alpha dither applies between coverage/alpha selection and alpha
    /// compare (Programming Manual pipeline order).
    fn apply_alpha(self, mut rgba: [u8; 4]) -> [u8; 4] {
        if let Some(noise) = self.alpha {
            rgba[3] = rgba[3].saturating_add(noise);
        }
        rgba
    }
}

/// RGB dither applies to the blender output, immediately before the pixel
/// write (blend -> dither -> memory-interface truncation). Hardware applies
/// the single per-pixel value to all three channels; alpha is untouched.
fn apply_rgb_dither_value(mut rgba: [u8; 4], noise: Option<u8>) -> [u8; 4] {
    if let Some(noise) = noise {
        for channel in &mut rgba[..3] {
            *channel = channel.saturating_add(noise);
        }
    }
    rgba
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
fn evaluate_combiner(
    state: CombinerState,
    cycle_type: CycleType,
    key_enabled: bool,
    lod_fraction: f32,
    shade: [u8; 4],
    texel0: [u8; 4],
    texel1: [u8; 4],
) -> [u8; 4] {
    let to_unit = |rgba: [u8; 4]| rgba.map(|v| v as f32 / 255.0);
    let mut inputs = CombinerInputs {
        combined: [0.0; 4],
        texel0: to_unit(texel0),
        texel1: to_unit(texel1),
        primitive: to_unit(state.primitive),
        shade: to_unit(shade),
        environment: to_unit(state.environment),
        lod_fraction,
        prim_lod_fraction: state.prim_lod_fraction as f32 / 255.0,
        k4: state.convert.k4(),
        k5: state.convert.k5(),
        key_center: state.key.center_unit(),
        key_scale: state.key.scale_unit(),
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
        ColorSource::Noise => panic!(
            "RDP color combiner selects NOISE; exact hardware noise state is not implemented"
        ),
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
    /// xorshift32 state shared by `AlphaDither::Noise` and
    /// `RgbDither::Noise` (see [`Self::apply_alpha_dither`] and
    /// [`Self::apply_rgb_dither`]). Deterministically seeded so a replayed
    /// task stream produces identical framebuffer bytes run over run.
    alpha_dither_noise_state: u32,
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

fn raw_pixel_coverage(
    edge: crate::gbi::RdpEdgeCoefficients,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> Coverage {
    if !scissor.line_enabled(y) {
        return Coverage::new(0);
    }
    let yh_eighth = i32::from(edge.yh) * 2;
    let yl_eighth = i32::from(edge.yl) * 2;
    let scissor_ulx_eighth = (scissor.ulx * 8.0).round() as i32;
    let scissor_uly_eighth = (scissor.uly * 8.0).round() as i32;
    let scissor_lrx_eighth = (scissor.lrx * 8.0).round() as i32;
    let scissor_lry_eighth = (scissor.lry * 8.0).round() as i32;
    let count = COVERAGE_SAMPLES
        .iter()
        .filter(|&&(offset_x, offset_y)| {
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
        .count() as u8;
    Coverage::new(count)
}

fn triangle_pixel_coverage(
    vertices: [Vertex; 3],
    area: f32,
    scissor: ScissorRect,
    x: i32,
    y: i32,
) -> Coverage {
    if !scissor.line_enabled(y) {
        return Coverage::new(0);
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
    let count = COVERAGE_SAMPLES
        .iter()
        .filter(|&&(offset_x, offset_y)| {
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
        .count() as u8;
    Coverage::new(count)
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

fn line_pixel_coverage(line: &Line, scissor: ScissorRect, x: i32, y: i32) -> Coverage {
    if !scissor.line_enabled(y) {
        return Coverage::new(0);
    }
    let [a, b] = line.v;
    let radius_squared = (line.width * 0.5).powi(2);
    let point_line = (b.x - a.x).abs() <= f32::EPSILON && (b.y - a.y).abs() <= f32::EPSILON;
    let count = COVERAGE_SAMPLES
        .iter()
        .filter(|&&(offset_x, offset_y)| {
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
        .count() as u8;
    Coverage::new(count)
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
            coverage: vec![Coverage::FULL; (width * height) as usize],
            depth: vec![f32::INFINITY; (width * height) as usize],
            encoded_depth: vec![None; (width * height) as usize],
            primitive_depth: None,
            alpha_dither_noise_state: 0x2545_F491,
        }
    }

    /// Draw one fragment's dither values. Only the noise-derived selectors
    /// are implemented: the manual specifies pseudo-random noise of 3-bit
    /// magnitude perturbing the color/alpha LSBs before memory-interface
    /// truncation but does not publish the hardware generator, so this uses
    /// a documented deterministic xorshift32 source -- REAL noise with the
    /// specified magnitude, NOT claimed to be sequence-exact against
    /// silicon. `G_AD_PATTERN`/`G_AD_NOTPATTERN` reuse the RGB stage's
    /// per-pixel value (respectively its 3-bit complement), so they are
    /// exactly representable whenever the RGB selector is `Noise`; with an
    /// ordered RGB matrix they would need the unpublished table and remain
    /// trapped in `require_supported_dither`, as do the ordered
    /// `MagicSquare`/`Bayer` RGB selectors themselves.
    fn fragment_dither(&mut self, other_mode: OtherMode) -> FragmentDither {
        let rgb = match other_mode.rgb_dither() {
            RgbDither::Disabled => None,
            RgbDither::Noise => Some(self.dither_noise_3bit()),
            RgbDither::MagicSquare | RgbDither::Bayer => {
                unreachable!("unsupported RGB dither is rejected before rasterization")
            }
        };
        let alpha = match other_mode.alpha_dither() {
            AlphaDither::Disabled => None,
            AlphaDither::Noise => Some(self.dither_noise_3bit()),
            AlphaDither::Pattern | AlphaDither::InversePattern => {
                let base = rgb.unwrap_or_else(|| {
                    unreachable!(
                        "pattern alpha dither without RGB noise is rejected before rasterization"
                    )
                });
                match other_mode.alpha_dither() {
                    AlphaDither::Pattern => Some(base),
                    _ => Some(!base & 7),
                }
            }
        };
        FragmentDither { rgb, alpha }
    }

    /// One 3-bit draw from the deterministic xorshift32 noise source shared
    /// by the alpha and RGB noise dither stages.
    fn dither_noise_3bit(&mut self) -> u8 {
        let mut state = self.alpha_dither_noise_state;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        self.alpha_dither_noise_state = state;
        (state & 7) as u8
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

    /// True if any pixel differs from a uniform `(r,g,b,a)` fill -- the
    /// honest "did this frame actually render geometry, not just a clear"
    /// check the task requires (`first_frame`'s whole point).
    pub fn has_non_uniform_content(&self, r: u8, g: u8, b: u8, a: u8) -> bool {
        self.pixels.chunks_exact(4).any(|px| px != [r, g, b, a])
    }

    /// Execute an RDP fill-cycle rectangle against the active public 8-bit,
    /// RGBA16, or RGBA32 color-image format.
    /// Public GBI fill rectangles cover the lower-right pixel inclusively;
    /// the independently programmed scissor retains its exclusive
    /// lower-right edge.
    pub fn draw_fill_rectangle(&mut self, rect: &FillRectangle, target: ColorImage) {
        assert_eq!(
            rect.cycle_type,
            CycleType::Fill,
            "G_FILLRECT outside fill cycle requires combiner/copy semantics"
        );

        let layout = target
            .layout()
            .expect("fill target must be I8/CI8, RGBA16, or RGBA32");
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
            EncodedDepth {
                visible: (rect.fill_color >> 16) as u16,
                hidden: 0,
            },
            EncodedDepth {
                visible: rect.fill_color as u16,
                hidden: 0,
            },
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
                let texel = texture.sample(s, t);
                if !copy_alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    texture,
                    texel[3],
                    rect.other_mode.blend_color_alpha,
                ) {
                    continue;
                }
                let index = (y as u32 * self.width + x as u32) as usize * 4;
                self.pixels[index..index + 4].copy_from_slice(&texel);
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
        require_supported_dither(rect.other_mode, "combined G_TEXRECT");
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
                let rgba = evaluate_combiner(
                    rect.combiner,
                    cycle_type,
                    rect.other_mode.combine_key(),
                    lod_fraction,
                    shade,
                    texel0,
                    texel1,
                );
                let (rgba, coverage) = apply_coverage_alpha(rect.other_mode, rgba, Coverage::FULL);
                let dither = self.fragment_dither(rect.other_mode);
                let rgba = dither.apply_alpha(rgba);
                if coverage.count() == 0 {
                    continue;
                }
                if !alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    rgba[3],
                    rect.other_mode.blend_color_alpha,
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
                            rgb_dither: dither.rgb,
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
                            rgb_dither: dither.rgb,
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
        let out = blend_fragment(
            fragment.rgba,
            dst,
            fragment.shade_alpha,
            blender,
            result.blend_enabled,
            result.memory,
        );
        let out = apply_rgb_dither_value(out, fragment.rgb_dither);
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
            if depth.mode == crate::gbi::DepthMode::Opaque && coverage.wraps {
                relations.in_front
            } else {
                crate::depth::mode_passes(depth.mode, relations)
            }
        };
        if passes_depth {
            self.coverage[pix] = coverage.destination;
            if other_mode.clear_on_coverage() && !coverage.wraps {
                return false;
            }
            let idx = pix * 4;
            let dst = self.pixels[idx..idx + 4].try_into().unwrap();
            let out = blend_fragment(
                fragment.rgba,
                dst,
                fragment.shade_alpha,
                blender,
                coverage.blend_enabled,
                coverage.memory,
            );
            let out = apply_rgb_dither_value(out, fragment.rgb_dither);
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
    /// pixel. Attribute evaluation remains at pixel center; fixed-width
    /// accumulator truncation and subpixel attribute correction are separate
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
        require_supported_dither(triangle.other_mode, "raw RDP triangle");
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
            let sample_y_eighth = y * 8 + 4;
            let edge_delta_y_eighth = sample_y_eighth - high_origin_eighth;
            let major_x =
                i64::from(edge.xh) + fixed_mul_ratio(edge.dxhdy, i64::from(edge_delta_y_eighth), 8);
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
                let coverage = raw_pixel_coverage(edge, scissor, x, y);
                if coverage.count() == 0 {
                    continue;
                }
                let sample_x = i64::from(x) * Q16_ONE + Q16_ONE / 2;
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
                    // Non-positive W tolerance (2026-07-21 WM2000 demo-scene
                    // rung): a perspective triangle crossing the near plane
                    // legitimately presents w <= 0 at edge pixels of the
                    // interpolated plane. Real RDP hardware's tcdiv derives
                    // 1/w from the operand's top bits with NO sign trap --
                    // the pixel samples garbage texels but the chip never
                    // faults. Mirror that defined-garbage tolerance: divide
                    // by the magnitude (min one ULP so it stays finite).
                    // This replaced a loud assert the moment real content
                    // (WM2000 gfx task ~#27, pixel-level near-plane
                    // crossing) hit it; the assert was right to exist until
                    // then, and the hardware-faithful behavior is "keep
                    // rasterizing", not "abort the machine".
                    //
                    // Scale (2026-07-21 WM2000 title-scene rung): hardware
                    // tcdiv is not a bare S/W ratio -- it produces an S10.5
                    // texel coordinate. The pipeline feeds tcdiv the high
                    // bits of the s15.16 attribute planes and multiplies by
                    // a 2^15-normalized reciprocal of W, so the output is
                    // (S/W) * 2^15 in S10.5 units = (S/W) * 2^10 texels
                    // (angrylion `tcdiv` persp path; RT64 divides s.w by
                    // w and scales identically). Without the 2^10 the whole
                    // title-screen quad collapsed onto texel (0,0) -- every
                    // pixel sampled the image's corner and the presented
                    // frame was a uniform field. With G_TP_NONE the divide
                    // is skipped entirely and the plane's integer part IS
                    // the S10.5 coordinate (angrylion `tcdiv_nopersp`).
                    let persp = triangle.other_mode.texture_perspective();
                    let corrected = move |values: [i64; 3]| {
                        if persp {
                            let denom = values[2].unsigned_abs().max(1) as f32;
                            (
                                values[0] as f32 / denom * 1024.0,
                                values[1] as f32 / denom * 1024.0,
                            )
                        } else {
                            // s15.16 plane -> S10.5 texels: 2^16 * 2^5.
                            const PLANE_TO_TEXEL: f32 = (1u32 << 21) as f32;
                            (
                                values[0] as f32 / PLANE_TO_TEXEL,
                                values[1] as f32 / PLANE_TO_TEXEL,
                            )
                        }
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
        let rgba = evaluate_combiner(
            pipeline.combiner,
            pipeline.other_mode.cycle_type(),
            pipeline.other_mode.combine_key(),
            fragment.lod_fraction,
            fragment.shade,
            fragment.texel0,
            fragment.texel1,
        );
        let (rgba, coverage) = apply_coverage_alpha(pipeline.other_mode, rgba, fragment.coverage);
        let dither = self.fragment_dither(pipeline.other_mode);
        let rgba = dither.apply_alpha(rgba);
        if coverage.count() == 0 {
            return false;
        }
        if !alpha_compare_value(
            pipeline.other_mode.alpha_compare(),
            rgba[3],
            pipeline.other_mode.blend_color_alpha,
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
                    rgb_dither: dither.rgb,
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
                    rgb_dither: dither.rgb,
                },
                pipeline.blender,
                pipeline.other_mode,
            )
        }
    }

    fn draw_triangle_impl(&mut self, tri: &Triangle, cull: CullMode, depth: DepthControl) {
        require_supported_alpha_compare(tri.other_mode, "F3DEX2 triangle");
        require_supported_dither(tri.other_mode, "F3DEX2 triangle");
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
                let coverage = triangle_pixel_coverage([a, b, c], area, scissor, x, y);
                if coverage.count() == 0 {
                    continue;
                }
                let p = Vertex {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
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
                // Interpolate S/w, T/w, and 1/w, then divide before
                // sampling. Adjacent pixel-center evaluations feed the same
                // derivative LOD selector used by raw triangles/rectangles.
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
        require_supported_dither(line.other_mode, "F3DEX2/L3DEX line");
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
                let coverage = line_pixel_coverage(line, scissor, x, y);
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
        AlphaCompare::None | AlphaCompare::Threshold => {}
        AlphaCompare::Reserved => {
            panic!("{primitive} selected reserved G_AC alpha-compare mode 2")
        }
        AlphaCompare::Dither => panic!(
            "{primitive} selected G_AC_DITHER, whose hardware pseudo-random alpha threshold is not implemented"
        ),
    }
}

/// The public Programming Manual defines the selector routing and says that
/// RGB dithering adds three low bits before memory-interface truncation, but
/// it does not publish either ordered 4x4 table or the long-period noise
/// generator. Copy and fill cycles bypass this path; callers invoke this gate
/// only for one/two-cycle primitives. Treating an active selector as disabled
/// would silently change both RGBA16 memory bytes and RGBA32 RGB values.
fn require_supported_dither(other_mode: OtherMode, primitive: &str) {
    match other_mode.rgb_dither() {
        // `Noise` is implemented (Framebuffer::apply_rgb_dither) under the
        // same precedent as alpha noise below: the manual documents the
        // effect (pseudo-random 3-bit perturbation of the blended RGB
        // before truncation) without publishing the generator. The ordered
        // matrices still trap: substituting a guessed table would silently
        // change deterministic memory bytes.
        RgbDither::Disabled | RgbDither::Noise => {}
        selector => panic!(
            "{primitive} selected RGB dither {selector:?}, whose hardware matrix/noise sequence is not implemented"
        ),
    }
    match (other_mode.rgb_dither(), other_mode.alpha_dither()) {
        // `Noise` is implemented (Framebuffer::fragment_dither): the
        // hardware generator is unpublished, but the manual-specified
        // effect (pseudo-random 3-bit perturbation of alpha before
        // truncation) is real and documented there.
        (_, AlphaDither::Disabled | AlphaDither::Noise) => {}
        // The public gbi.h couples `G_AD_PATTERN`/`G_AD_NOTPATTERN` to the
        // RGB dither stage's per-pixel value (respectively its complement),
        // so they are exactly representable whenever that value exists and
        // is itself implemented -- i.e. under RGB noise.
        (RgbDither::Noise, AlphaDither::Pattern | AlphaDither::InversePattern) => {}
        // With an ordered RGB matrix (or RGB dithering disabled) a pattern
        // alpha selector would need the unpublished 4x4 table; substituting
        // a guessed one would silently change deterministic memory bytes.
        (rgb, alpha) => panic!(
            "{primitive} selected alpha dither {alpha:?} with RGB dither {rgb:?}; the ordered hardware dither matrix is not implemented"
        ),
    }
}

fn alpha_compare_value(mode: AlphaCompare, alpha: u8, threshold_alpha: u8) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        AlphaCompare::Dither | AlphaCompare::Reserved => {
            unreachable!("unsupported alpha compare is rejected before rasterization")
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
) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold
            if texture.format == ColorImage::RGBA_FORMAT && texture.size == ColorImage::BITS_16 =>
        {
            alpha != 0
        }
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        AlphaCompare::Dither | AlphaCompare::Reserved => {
            unreachable!("unsupported alpha compare is rejected before copy rasterization")
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
    #[should_panic(expected = "RDP color combiner selects NOISE")]
    fn noise_combiner_source_traps_instead_of_substituting_black() {
        let state = repeated_state(
            cycle(
                [
                    ColorSource::Noise,
                    ColorSource::Zero,
                    ColorSource::Zero,
                    ColorSource::Zero,
                ],
                [AlphaSource::Zero; 4],
            ),
            [0; 4],
            [0; 4],
        );
        let _ = evaluate_combiner(
            state,
            CycleType::OneCycle,
            false,
            0.0,
            [0; 4],
            [0; 4],
            [0; 4],
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

        let mut copy = texture_rectangle(
            solid_texture([255, 0, 0, 255]),
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
                left_major: true,
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
    fn rgba16_copy_threshold_uses_alpha_bit_even_when_blend_threshold_is_zero() {
        let mut texture = solid_texture([0; 4]);
        texture.width = 2;
        texture.texels = std::rc::Rc::new(vec![
            255, 0, 0, 0, // RGBA16 alpha bit clear: write disabled
            0, 255, 0, 255, // RGBA16 alpha bit set: write enabled
        ]);
        let mut rectangle = texture_rectangle(
            texture,
            OtherMode::from_raw(2 << 20, 1, 0),
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
                left_major: false,
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
                // W = 1024: tcdiv unity under the hardware persp scale
                // (S/W * 2^10 texels), so dstdx/dstdy below read directly
                // in texels per pixel.
                stw: [0, 0, 1024 << 16],
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

        // G_TP_NONE variant of the same primitive: hardware `tcdiv_nopersp`
        // skips the divide entirely and the plane's INTEGER part is the
        // S10.5 texel coordinate, so the same 2.5-texel-per-pixel gradient
        // is spelled `2.5 * 2^21` in plane units and W is irrelevant.
        let mut nopersp = triangle;
        nopersp.other_mode =
            crate::gbi::OtherMode::from_raw(other_mode.raw_high() & !(1 << 19), 0, 0);
        nopersp.texture_coefficients = Some(crate::gbi::RdpTextureCoefficients {
            stw: [0, 0, 0],
            dstdx: [(2.5 * (1u32 << 21) as f64) as i32, 0, 0],
            dstde: [0; 3],
            dstdy: [0, (2.5 * (1u32 << 21) as f64) as i32, 0],
        });
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.draw_raw_rdp_triangle(&nopersp);
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
                0.0,
                shade,
                texel,
                texel,
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
                0.0,
                shade,
                texel,
                texel,
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
                0.0,
                shade,
                texel,
                texel,
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
                0.0,
                shade,
                texel,
                texel,
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
                0.0,
                [128; 4],
                [200; 4],
                [200; 4],
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
                0.0,
                [0; 4],
                [100, 150, 200, 255],
                [0; 4]
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
                0.0,
                [0; 4],
                [100, 100, 100, 255],
                [0; 4],
            ),
            [0, 0, 0, 255]
        );
        assert_eq!(
            evaluate_combiner(
                state,
                CycleType::OneCycle,
                true,
                0.0,
                [0; 4],
                [200, 100, 100, 255],
                [0; 4],
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

        fb.draw_triangle(&tri);

        assert_eq!(fb.coverage[0], Coverage::new(4));
        assert_eq!(&fb.pixels[..4], &[255, 0, 0, 255]);
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

    /// The public Programming Manual defines G_AC_DITHER as a comparison
    /// against hardware-generated pseudo-random noise. An ordered Bayer mask
    /// is observably different (screen-locked and frame-invariant), so it
    /// cannot stand in for the missing generator.
    #[test]
    #[should_panic(expected = "hardware pseudo-random alpha threshold is not implemented")]
    fn dither_alpha_compare_traps_instead_of_using_ordered_bayer() {
        let tri = Triangle {
            v: [
                v(0.0, 0.0, 255, 255, 255, 128),
                v(16.0, 0.0, 255, 255, 255, 128),
                v(0.0, 16.0, 255, 255, 255, 128),
            ],
            other_mode: crate::gbi::OtherMode::from_raw(0, 3, 0),
            ..Default::default()
        };
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle(&tri);
    }

    #[test]
    #[should_panic(expected = "RGB dither MagicSquare")]
    fn active_rgb_dither_traps_instead_of_silently_truncating() {
        let tri = Triangle {
            v: [
                v(0.0, 0.0, 7, 7, 7, 255),
                v(8.0, 0.0, 7, 7, 7, 255),
                v(0.0, 8.0, 7, 7, 7, 255),
            ],
            // RGB magic-square, alpha dither disabled.
            other_mode: OtherMode::from_raw(3 << 4, 0, 0),
            ..Default::default()
        };
        Framebuffer::new(8, 8).draw_triangle(&tri);
    }

    #[test]
    fn rgb_noise_dither_is_deterministic_and_bounded_to_three_bits() {
        let solid = |high: u32| {
            let tri = Triangle {
                v: [
                    v(0.0, 0.0, 100, 120, 140, 255),
                    v(8.0, 0.0, 100, 120, 140, 255),
                    v(0.0, 8.0, 100, 120, 140, 255),
                ],
                other_mode: OtherMode::from_raw(high, 0, 0),
                ..Default::default()
            };
            let mut fb = Framebuffer::new(8, 8);
            fb.draw_triangle(&tri);
            fb
        };
        // RGB noise selected (selector 2 at bits 6-7), alpha dither disabled.
        let noisy = solid((2 << 6) | (3 << 4));
        let replay = solid((2 << 6) | (3 << 4));
        let clean = solid((3 << 6) | (3 << 4));
        assert_eq!(
            noisy.pixels, replay.pixels,
            "seeded xorshift noise must replay to identical framebuffer bytes"
        );
        let mut perturbed = 0u32;
        for (noisy_px, clean_px) in noisy
            .pixels
            .chunks_exact(4)
            .zip(clean.pixels.chunks_exact(4))
        {
            if clean_px == [0, 0, 0, 0] {
                // Pixel outside the triangle: noise must not invent coverage.
                assert_eq!(noisy_px, [0, 0, 0, 0]);
                continue;
            }
            // One per-pixel 3-bit value applied to all three channels;
            // alpha untouched by RGB dither.
            let delta = noisy_px[0].wrapping_sub(clean_px[0]);
            assert!(delta < 8, "noise magnitude must stay within 3 bits");
            assert_eq!(noisy_px[1].wrapping_sub(clean_px[1]), delta);
            assert_eq!(noisy_px[2].wrapping_sub(clean_px[2]), delta);
            assert_eq!(noisy_px[3], clean_px[3]);
            perturbed += u32::from(delta != 0);
        }
        assert!(
            perturbed > 0,
            "an all-zero draw would mean the noise stage silently disabled itself"
        );
    }

    #[test]
    fn pattern_alpha_dither_reuses_the_rgb_noise_value() {
        let mut fb = Framebuffer::new(1, 1);
        // RGB noise (selector 2 at bits 6-7) + alpha pattern (0 at bits 4-5):
        // one per-pixel draw serves both stages.
        let pattern = fb.fragment_dither(OtherMode::from_raw(2 << 6, 0, 0));
        assert!(pattern.rgb.unwrap() < 8);
        assert_eq!(pattern.alpha, pattern.rgb);
        // Inverse pattern (1 at bits 4-5) is the 3-bit complement of the
        // same per-pixel value.
        let inverse = fb.fragment_dither(OtherMode::from_raw((2 << 6) | (1 << 4), 0, 0));
        assert_eq!(inverse.alpha, inverse.rgb.map(|noise| !noise & 7));
        // Independent noise selectors draw separately: alpha noise (2 at
        // bits 4-5) must not be forced equal to the RGB value forever.
        let mut ever_distinct = false;
        for _ in 0..32 {
            let both = fb.fragment_dither(OtherMode::from_raw((2 << 6) | (2 << 4), 0, 0));
            ever_distinct |= both.alpha != both.rgb;
        }
        assert!(ever_distinct, "alpha noise must be its own draw, not a copy");
    }

    #[test]
    #[should_panic(expected = "alpha dither Pattern")]
    fn active_alpha_dither_traps_instead_of_using_full_precision_alpha() {
        let tri = Triangle {
            v: [
                v(0.0, 0.0, 255, 255, 255, 7),
                v(8.0, 0.0, 255, 255, 255, 7),
                v(0.0, 8.0, 255, 255, 255, 7),
            ],
            // RGB dither disabled, alpha pattern selected.
            other_mode: OtherMode::from_raw(3 << 6, 0, 0),
            ..Default::default()
        };
        Framebuffer::new(8, 8).draw_triangle(&tri);
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
                    rgb_dither: None,
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
                    rgb_dither: None,
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
    fn raw_coverage_uses_the_public_eight_sample_checkerboard_mask() {
        let vertical_strip = |left: f32, right: f32| crate::gbi::RdpEdgeCoefficients {
            left_major: false,
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
            Coverage::FULL
        );
        let left = raw_pixel_coverage(vertical_strip(0.0, 0.5), scissor, 0, 0);
        let right = raw_pixel_coverage(vertical_strip(0.5, 1.0), scissor, 0, 0);
        assert_eq!(left, Coverage::new(4));
        assert_eq!(right, Coverage::new(4));
        assert_eq!(left.count() + right.count(), Coverage::FULL.count());

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
        assert_eq!(top, Coverage::new(4));
        assert_eq!(bottom, Coverage::new(4));
        assert_eq!(top.count() + bottom.count(), Coverage::FULL.count());
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
        assert_eq!(first.count() + second.count(), Coverage::FULL.count());
        assert_eq!(first, Coverage::new(6));
        assert_eq!(second, Coverage::new(2));

        let reversed = |[a, b, c]: [Vertex; 3]| [a, c, b];
        assert_eq!(coverage(reversed(upper_right)), first);
        assert_eq!(coverage(reversed(lower_left)), second);
    }

    #[test]
    fn raw_edges_use_the_public_preceding_scanline_reference_point() {
        let edge = crate::gbi::RdpEdgeCoefficients {
            left_major: false,
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
    fn clear_on_coverage_updates_coverage_even_when_it_inhibits_color() {
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.clear(0, 0, 255, 255);
        framebuffer.coverage[0] = Coverage::new(3);
        // 0xf0 high bits: RGB and alpha dither both disabled -- this test
        // exercises coverage semantics, not the dither stage, and it calls
        // the internal write path directly (below the primitive-level
        // require_supported_dither gate).
        let mode = OtherMode::from_raw(0xf0, 0x4180, 0);

        assert!(!framebuffer.set_blended(
            0,
            0,
            ColorFragment {
                rgba: [255, 0, 0, 255],
                coverage: Coverage::new(4),
                shade_alpha: 255,
                rgb_dither: None,
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
                rgb_dither: None,
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
                    rgb_dither: None,
                },
                BlenderState::default(),
                DepthControl {
                    compare: true,
                    update: false,
                    mode: crate::gbi::DepthMode::Opaque,
                },
                // Dither disabled (0xf0): direct internal-path call below
                // the primitive-level dither gate.
                OtherMode::from_raw(0xf0, 0x0110, 0),
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
                left_major: true,
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
            .count()
                > 0
        );
        assert_eq!(pixel(2, 4), &[255, 255, 255, 255]);
        assert_eq!(pixel(1, 4), &[0, 0, 0, 0]);
    }

    #[test]
    fn real_stream_left_major_rect_split_triangle_rasterizes_interior() {
        // Byte-exact edge coefficients from WM2000's live title-scene XBUS
        // stream (task #783, first tri): `lft`=1 with the constant XH edge
        // on the LEFT (11.75) and XM marching right at +4.157/line, lower
        // half degenerate (ym == yl) -- the canonical rect-split shape every
        // real F3DEX2 quad decomposes into. Under the inverted lft reading
        // every span computed right < left and the triangle rasterized ZERO
        // pixels (the whole title logo vanished); this pins the corrected
        // convention to live-stream evidence.
        let triangle = RawRdpTriangle {
            edge: crate::gbi::RdpEdgeCoefficients {
                left_major: true,
                level: 0,
                tile: 0,
                yl: 106,
                ym: 106,
                yh: 17,
                xl: 6832128,
                dxldy: -16842729,
                xh: 770048,
                dxhdy: 0,
                xm: 701940,
                dxmdy: 272435,
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
        let mut framebuffer = Framebuffer::new(64, 32);
        framebuffer.draw_raw_rdp_triangle(&triangle);
        let pixel = |x: usize, y: usize| {
            let offset = (y * 64 + x) * 4;
            &framebuffer.pixels[offset..offset + 4]
        };
        // Interior at y=15: span is [11.75, 10.71 + 4.157 * 11.5 ~= 58.5).
        assert_eq!(pixel(30, 15), &[255, 255, 255, 255]);
        // Left of the major edge and right of the minor edge stay untouched.
        assert_eq!(pixel(5, 15), &[0, 0, 0, 0]);
        assert_eq!(pixel(60, 10), &[0, 0, 0, 0]);
    }
}
