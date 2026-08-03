// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use sha2::Digest;
use std::fmt::Write as _;
use super::*;
use super::state::*;

/// One decoded vertex in screen space (after MVP + viewport if a transform
/// was active, or raw `ob` coords if no matrix was loaded -- see
/// `decode_display_list`) plus a flat RGBA color, matching the
/// position+color fields of the SDK's public `Vtx` union.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    /// Screen-space depth (mapped NDC-z through the viewport, nearer =
    /// smaller). Used by the z-buffer in `raster.rs`; 0.0 for the raw
    /// no-transform reference-fixture path (where all geometry is coplanar).
    pub z: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    /// Texture S/T coordinates in texels: the raw `Vtx` `tc[2]` S10.5
    /// fixed-point value multiplied by the `G_TEXTURE` S/T scale, then
    /// converted from the S10.5 encoding to texels (÷32). Only meaningful
    /// when the emitting triangle carries a `texture`; the rasterizer
    /// interpolates these per-pixel to address the decoded texel buffer
    /// (`F3DEX2-CONCEPTS.md` §5). 0.0 on the untextured/reference path.
    pub s: f32,
    pub t: f32,
    /// The homogeneous clip-space `w` this vertex was divided by (before the
    /// perspective divide). `w <= 0` means the vertex is AT or BEHIND the
    /// camera's near plane -- projecting it divides by a non-positive number
    /// and flings it to the opposite side of the screen, which is the "fan/
    /// bowtie from a central point" artifact. A triangle with any such vertex
    /// is dropped (coarse near-plane cull, see `behind_near_plane`) rather
    /// than drawn as a giant wrong-side polygon. `1.0` on the raw/reference
    /// path (no projection, everything in front).
    pub w: f32,
    /// Raw unsigned 16.16 screen-depth value retained for `G_BRANCH_Z`.
    /// Keeping this beside the display `z` prevents the conditional command
    /// from reconstructing a fixed-point comparison through host float.
    pub z_screen: u32,
    /// Six homogeneous viewing-volume side bits maintained when the vertex is
    /// transformed. `G_CULLDL` ANDs these codes across its inclusive range;
    /// a shared nonzero side means the complete bounding volume is outside.
    pub clip_code: u8,
    /// Homogeneous clip position retained for line clipping. Post-transform
    /// XY/Z modification invalidates this value because the public command
    /// supplies only final screen coordinates, not a reconstructable clip W.
    pub clip_position: Option<[f32; 4]>,
}

pub(super) const CLIP_NEG_X: u8 = 1 << 0;
pub(super) const CLIP_POS_X: u8 = 1 << 1;
pub(super) const CLIP_NEG_Y: u8 = 1 << 2;
pub(super) const CLIP_POS_Y: u8 = 1 << 3;
pub(super) const CLIP_NEG_Z: u8 = 1 << 4;
pub(super) const CLIP_POS_Z: u8 = 1 << 5;

/// Screen-space back/front-face culling selector, derived from the F3DEX2
/// `G_GEOMETRYMODE` `G_CULL_FRONT`/`G_CULL_BACK` bits
/// (`F3DEX2-CONCEPTS.md` §2.4). The rasterizer (`raster.rs`) applies it by
/// the sign of a triangle's screen-space signed area.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CullMode {
    /// No culling (both faces drawn).
    #[default]
    None,
    /// Cull back faces (`G_CULL_BACK`) -- the common OoT case.
    Back,
    /// Cull front faces (`G_CULL_FRONT`).
    Front,
    /// Cull both (`G_CULL_BOTH`) -- draws nothing.
    Both,
}

/// RDP cycle type from other-mode high bits 20..21 (`G_MDSFT_CYCLETYPE`).
/// Public `gbi.h` defines the four values at lines 527-531; RT64 exposes the
/// same masked field in `shared/rt64_other_mode.h:26-28`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CycleType {
    #[default]
    OneCycle,
    TwoCycle,
    Copy,
    Fill,
}

/// RDP texture filter from other-mode high bits 12..13
/// (`G_MDSFT_TEXTFILT`; public `gbi.h:514,551-554`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TextureFilter {
    #[default]
    Point,
    Reserved,
    Bilinear,
    Average,
}

/// RGB dither selector from other-mode high bits 6..7
/// (`G_MDSFT_RGBDITHER`; public `gbi.h:510,565-571`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RgbDither {
    #[default]
    MagicSquare,
    Bayer,
    Noise,
    Disabled,
}

/// Alpha dither selector from other-mode high bits 4..5
/// (`G_MDSFT_ALPHADITHER`; public `gbi.h:509,578-582`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AlphaDither {
    #[default]
    Pattern,
    InversePattern,
    Noise,
    Disabled,
}

/// Alpha-compare mode from other-mode low bits 0..1. The public constants
/// are `G_AC_NONE=0`, `G_AC_THRESHOLD=1`, and `G_AC_DITHER=3`
/// (`gbi.h:500,584-587`); value 2 is reserved.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AlphaCompare {
    #[default]
    None,
    Threshold,
    Reserved,
    Dither,
}

/// Coverage destination from render-mode bits 8..9
/// (`CVG_DST_*`, public `gbi.h:599-602`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CoverageDestination {
    #[default]
    Clamp,
    Wrap,
    Full,
    Save,
}

/// Z-mode from render-mode bits 10..11 (`ZMODE_*`, public
/// `gbi.h:603-606`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DepthMode {
    #[default]
    Opaque,
    Interpenetrating,
    Translucent,
    Decal,
}

/// The four two-bit blender selectors for one RDP cycle. Their positions are
/// the public `GBL_c1`/`GBL_c2` packing contract (`gbi.h:624-627`). Keeping
/// selectors as wire values avoids coupling this task to the separate color-
/// combiner implementation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlenderCycle {
    pub color_a: u8,
    pub alpha_a: u8,
    pub color_b: u8,
    pub alpha_b: u8,
}

/// The two RDP other-mode words plus the one color-register component alpha
/// comparison needs. F3DEX2 updates arbitrary bit ranges of H/L, so retaining
/// the raw words is the smallest merge-friendly representation; typed accessors
/// expose every render field this rasterizer or a future backend consumes.
///
/// Sources: public OoT `include/ultra64/gbi.h:497-627` (field shifts, values,
/// coverage/Z/blender packing), `gbi.h:3353-3369` (F3DEX2 partial-update wire
/// encoding), RT64 `shared/rt64_other_mode.h:14-101` (H/L field structure),
/// and RT64 `hle/rt64_rsp.cpp:1026-1037` (masked partial updates).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OtherMode {
    pub(super) high: u32,
    pub(super) low: u32,
    /// `G_SETBLENDCOLOR.a`, used by `G_AC_THRESHOLD` (RT64
    /// `shaders/RasterPS.hlsl:209-211`). Kept here, rather than adding prim/env
    /// color state, so the independently landing combiner remains isolated.
    pub blend_color_alpha: u8,
}

impl Default for OtherMode {
    fn default() -> Self {
        Self {
            // RT64's F3DEX2 reset state (`hle/rt64_rsp.cpp:88-89`). Low=0
            // means alpha compare off until the display list enables it.
            high: 0x0008_0cff,
            low: 0,
            blend_color_alpha: 0,
        }
    }
}

/// One semantic RGB input to the RDP color-combiner equation
/// `(A - B) * C + D`.
///
/// The raw numeric selector is position-dependent: selector `6`, for
/// example, means ONE in input A/D but KEY_CENTER/KEY_SCALE in B/C. The
/// decoder therefore resolves the wire value to this semantic enum at
/// `G_SETCOMBINE` time. Source values and position-specific meanings are
/// from OoT's public `ultra64/gbi.h:383-404` and RT64's MIT
/// `shared/rt64_color_combiner.h:59-151`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSource {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    KeyCenter,
    KeyScale,
    CombinedAlpha,
    Texel0Alpha,
    Texel1Alpha,
    PrimitiveAlpha,
    ShadeAlpha,
    EnvironmentAlpha,
    LodFraction,
    PrimLodFraction,
    Noise,
    K4,
    K5,
    One,
    Zero,
}

/// One semantic alpha input to the RDP color-combiner equation.
/// Selector values come from public `gbi.h:406-416`; the distinct C-input
/// mapping (where zero selects LOD fraction) is corroborated by RT64's MIT
/// `shared/rt64_color_combiner.h:153-193`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AlphaSource {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    LodFraction,
    PrimLodFraction,
    One,
    Zero,
}

/// The eight selectors for one RDP combiner cycle: four RGB and four alpha.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CombinerCycle {
    pub rgb: [ColorSource; 4],
    pub alpha: [AlphaSource; 4],
}

/// Both cycles programmed by one `G_SETCOMBINE` command.
///
/// Bit locations are the public `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1`
/// packing macros (`ultra64/gbi.h:3543-3565`) and match RT64's MIT parse
/// helpers (`shared/rt64_color_combiner.h:195-240`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CombinerMode {
    pub cycles: [CombinerCycle; 2],
}

impl CombinerMode {
    pub(super) fn decode(w0: u32, w1: u32) -> Self {
        CombinerMode {
            cycles: [
                CombinerCycle {
                    rgb: [
                        decode_color_a((w0 >> 20) & 0x0f),
                        decode_color_b((w1 >> 28) & 0x0f),
                        decode_color_c((w0 >> 15) & 0x1f),
                        decode_color_d((w1 >> 15) & 0x07),
                    ],
                    alpha: [
                        decode_alpha_abd((w0 >> 12) & 0x07),
                        decode_alpha_abd((w1 >> 12) & 0x07),
                        decode_alpha_c((w0 >> 9) & 0x07),
                        decode_alpha_abd((w1 >> 9) & 0x07),
                    ],
                },
                CombinerCycle {
                    rgb: [
                        decode_color_a((w0 >> 5) & 0x0f),
                        decode_color_b((w1 >> 24) & 0x0f),
                        decode_color_c(w0 & 0x1f),
                        decode_color_d((w1 >> 6) & 0x07),
                    ],
                    alpha: [
                        decode_alpha_abd((w1 >> 21) & 0x07),
                        decode_alpha_abd((w1 >> 3) & 0x07),
                        decode_alpha_c((w1 >> 18) & 0x07),
                        decode_alpha_abd(w1 & 0x07),
                    ],
                },
            ],
        }
    }

    pub(crate) fn uses_texel1(self, cycle_type: CycleType) -> bool {
        let cycle_count = match cycle_type {
            CycleType::OneCycle => 1,
            CycleType::TwoCycle => 2,
            CycleType::Copy | CycleType::Fill => 0,
        };
        self.cycles.iter().take(cycle_count).any(|cycle| {
            cycle
                .rgb
                .iter()
                .any(|source| matches!(source, ColorSource::Texel1 | ColorSource::Texel1Alpha))
                || cycle
                    .alpha
                    .iter()
                    .any(|source| matches!(source, AlphaSource::Texel1))
        })
    }
}

impl Default for CombinerMode {
    fn default() -> Self {
        // Neutral legacy/default path: TEXEL0 * SHADE for RGB and alpha.
        // A missing texture is supplied as white by the software evaluator,
        // preserving the original untextured shade-only fixture behavior.
        let modulate = CombinerCycle {
            rgb: [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Shade,
                ColorSource::Zero,
            ],
            alpha: [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Shade,
                AlphaSource::Zero,
            ],
        };
        CombinerMode {
            cycles: [modulate; 2],
        }
    }
}

impl OtherMode {
    pub fn raw_high(self) -> u32 {
        self.high
    }

    pub fn raw_low(self) -> u32 {
        self.low
    }

    pub fn cycle_type(self) -> CycleType {
        match (self.high >> 20) & 3 {
            0 => CycleType::OneCycle,
            1 => CycleType::TwoCycle,
            2 => CycleType::Copy,
            _ => CycleType::Fill,
        }
    }

    pub fn texture_filter(self) -> TextureFilter {
        match (self.high >> 12) & 3 {
            0 => TextureFilter::Point,
            1 => TextureFilter::Reserved,
            2 => TextureFilter::Bilinear,
            _ => TextureFilter::Average,
        }
    }

    pub fn rgb_dither(self) -> RgbDither {
        match (self.high >> 6) & 3 {
            0 => RgbDither::MagicSquare,
            1 => RgbDither::Bayer,
            2 => RgbDither::Noise,
            _ => RgbDither::Disabled,
        }
    }

    pub fn alpha_dither(self) -> AlphaDither {
        match (self.high >> 4) & 3 {
            0 => AlphaDither::Pattern,
            1 => AlphaDither::InversePattern,
            2 => AlphaDither::Noise,
            _ => AlphaDither::Disabled,
        }
    }

    pub fn combine_key(self) -> bool {
        self.high & (1 << 8) != 0
    }

    pub fn texture_convert(self) -> u8 {
        ((self.high >> 9) & 7) as u8
    }

    pub fn texture_lut(self) -> u8 {
        ((self.high >> 14) & 3) as u8
    }

    pub fn texture_lod(self) -> bool {
        self.high & (1 << 16) != 0
    }

    pub fn texture_detail(self) -> u8 {
        ((self.high >> 17) & 3) as u8
    }

    pub fn texture_perspective(self) -> bool {
        self.high & (1 << 19) != 0
    }

    pub fn one_primitive_pipeline(self) -> bool {
        self.high & (1 << 23) != 0
    }

    pub fn alpha_compare(self) -> AlphaCompare {
        match self.low & 3 {
            0 => AlphaCompare::None,
            1 => AlphaCompare::Threshold,
            2 => AlphaCompare::Reserved,
            _ => AlphaCompare::Dither,
        }
    }

    pub fn primitive_depth_source(self) -> bool {
        self.low & (1 << 2) != 0
    }

    pub fn antialias_enabled(self) -> bool {
        self.low & 0x0008 != 0
    }

    pub fn depth_compare_enabled(self) -> bool {
        self.low & 0x0010 != 0
    }

    pub fn depth_update_enabled(self) -> bool {
        self.low & 0x0020 != 0
    }

    pub fn image_read_enabled(self) -> bool {
        self.low & 0x0040 != 0
    }

    /// Classify state that is unsafe while the RDP arithmetic pipeline is
    /// bypassed by Fill cycle.
    ///
    /// Provenance: Nintendo 64 Functions Reference, `gDPFillRectangle`,
    /// "Note" (Fill mode must not use a Z-buffer render mode), and
    /// `gDPSetCycleType`, "Notes" (Fill mode requires
    /// `G_RM_NOOP`/`G_RM_NOOP2`; a Z read can hang the RDP).
    pub(crate) fn validate_fill_cycle_bypass(self) -> Result<(), FillCycleBypassHazards> {
        let hazards = FillCycleBypassHazards {
            depth_compare: self.depth_compare_enabled(),
            depth_update: self.depth_update_enabled(),
            image_read: self.image_read_enabled(),
        };
        if hazards.is_empty() {
            Ok(())
        } else {
            Err(hazards)
        }
    }

    pub fn clear_on_coverage(self) -> bool {
        self.low & 0x0080 != 0
    }

    pub fn coverage_destination(self) -> CoverageDestination {
        match (self.low >> 8) & 3 {
            0 => CoverageDestination::Clamp,
            1 => CoverageDestination::Wrap,
            2 => CoverageDestination::Full,
            _ => CoverageDestination::Save,
        }
    }

    pub fn depth_mode(self) -> DepthMode {
        match (self.low >> 10) & 3 {
            0 => DepthMode::Opaque,
            1 => DepthMode::Interpenetrating,
            2 => DepthMode::Translucent,
            _ => DepthMode::Decal,
        }
    }

    pub fn coverage_times_alpha(self) -> bool {
        self.low & 0x1000 != 0
    }

    pub fn alpha_coverage_select(self) -> bool {
        self.low & 0x2000 != 0
    }

    pub fn force_blend(self) -> bool {
        self.low & 0x4000 != 0
    }

    pub fn blender_cycle_1(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 30) & 3) as u8,
            alpha_a: ((self.low >> 26) & 3) as u8,
            color_b: ((self.low >> 22) & 3) as u8,
            alpha_b: ((self.low >> 18) & 3) as u8,
        }
    }

    pub fn blender_cycle_2(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 28) & 3) as u8,
            alpha_a: ((self.low >> 24) & 3) as u8,
            color_b: ((self.low >> 20) & 3) as u8,
            alpha_b: ((self.low >> 16) & 3) as u8,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_raw(high: u32, low: u32, blend_color_alpha: u8) -> Self {
        Self {
            high,
            low,
            blend_color_alpha,
        }
    }
}

/// Unsafe memory/depth consumers retained in Other Modes while Fill cycle
/// bypasses the ordinary RDP pixel pipeline.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FillCycleBypassHazards {
    pub depth_compare: bool,
    pub depth_update: bool,
    pub image_read: bool,
}

impl FillCycleBypassHazards {
    pub(super) const fn is_empty(self) -> bool {
        !self.depth_compare && !self.depth_update && !self.image_read
    }
}

impl std::fmt::Display for FillCycleBypassHazards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut separator = "";
        for (enabled, name) in [
            (self.depth_compare, "Z_CMP"),
            (self.depth_update, "Z_UPD"),
            (self.image_read, "IM_RD"),
        ] {
            if enabled {
                f.write_str(separator)?;
                f.write_str(name)?;
                separator = "+";
            }
        }
        Ok(())
    }
}

/// RDP color state snapshotted onto each emitted triangle.
///
/// This stays separate from the render/other-mode state being added on the
/// neighboring job: it contains `G_SETCOMBINE`, primitive/environment RGBA,
/// primitive LOD fraction, conversion constants, and chroma-key registers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CombinerState {
    pub mode: CombinerMode,
    pub primitive: [u8; 4],
    pub environment: [u8; 4],
    pub min_lod_level: u8,
    pub prim_lod_fraction: u8,
    pub convert: ConvertState,
    pub key: KeyState,
}

/// Persistent chroma-key center, scale, and 4.8-width registers.
///
/// Public SGI *RDP Command Summary* Tables 29-30 define the split `SETKEYR`
/// and `SETKEYGB` wire layouts and the alpha-fixup equation. Keeping width in
/// its twelve-bit wire form preserves the documented `> 1.0` channel-disable
/// rule without round-tripping through floating point during decode.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyState {
    pub center: [u8; 3],
    pub scale: [u8; 3],
    pub width: [u16; 3],
}

impl KeyState {
    pub(super) fn set_r(&mut self, w1: u32) {
        self.width[0] = ((w1 >> 16) & 0x0fff) as u16;
        self.center[0] = (w1 >> 8) as u8;
        self.scale[0] = w1 as u8;
    }

    pub(super) fn set_gb(&mut self, w0: u32, w1: u32) {
        self.width[1] = ((w0 >> 12) & 0x0fff) as u16;
        self.width[2] = (w0 & 0x0fff) as u16;
        self.center[1] = (w1 >> 24) as u8;
        self.scale[1] = (w1 >> 16) as u8;
        self.center[2] = (w1 >> 8) as u8;
        self.scale[2] = w1 as u8;
    }

    pub(crate) fn center_unit(self) -> [f32; 3] {
        self.center.map(|value| f32::from(value) / 255.0)
    }

    pub(crate) fn scale_unit(self) -> [f32; 3] {
        self.scale.map(|value| f32::from(value) / 255.0)
    }

    pub(crate) fn alpha_from_key_prime(self, key_prime: [f32; 3]) -> f32 {
        let mut alpha = 1.0f32;
        for (channel, value) in key_prime.into_iter().enumerate() {
            let component = if self.width[channel] > 0x100 {
                // The public programming manual specifies width > 1.0 as
                // disabling keying for that channel.
                1.0
            } else {
                (f32::from(self.width[channel]) / 256.0 - value.abs()).clamp(0.0, 1.0)
            };
            alpha = alpha.min(component);
        }
        alpha
    }
}

/// Persistent `G_SETCONVERT` K0..K5 registers. SGI's public *RDP Command
/// Summary*, Table 28, defines six signed nine-bit fields and the two-stage
/// YUV conversion equations. Keeping the wire integers avoids losing their
/// distinct fixed-point interpretations: K0..K3 are S1.7 texture-filter
/// multipliers, K4 is an 8-bit combiner offset, and K5 is the combiner scale.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ConvertState {
    pub coefficients: [i16; 6],
}

impl Default for ConvertState {
    fn default() -> Self {
        // Public gbi.h G_CV_K0..G_CV_K5 defaults for YUV-to-RGB.
        Self {
            coefficients: [175, -43, -89, 222, 114, 42],
        }
    }
}

impl ConvertState {
    pub(super) fn decode(w0: u32, w1: u32) -> Self {
        let signed_9 = |value: u32| ((value << 23) as i32 >> 23) as i16;
        Self {
            coefficients: [
                signed_9((w0 >> 13) & 0x1ff),
                signed_9((w0 >> 4) & 0x1ff),
                signed_9(((w0 & 0x0f) << 5) | ((w1 >> 27) & 0x1f)),
                signed_9((w1 >> 18) & 0x1ff),
                signed_9((w1 >> 9) & 0x1ff),
                signed_9(w1 & 0x1ff),
            ],
        }
    }

    pub(crate) fn convert_texel(self, texel: [u8; 4]) -> [u8; 4] {
        let [k0, k1, k2, k3, _, _] = self.coefficients;
        let y = i32::from(texel[0]);
        let u = i32::from(texel[1]) - 128;
        let v = i32::from(texel[2]) - 128;
        let multiply =
            |coefficient: i16, component: i32| (i32::from(coefficient) * component).div_euclid(128);
        let clamp = |value: i32| value.clamp(0, 255) as u8;
        [
            clamp(y + multiply(k0, v)),
            clamp(y + multiply(k1, u) + multiply(k2, v)),
            clamp(y + multiply(k3, u)),
            texel[3],
        ]
    }

    pub(crate) fn k4(self) -> f32 {
        f32::from(self.coefficients[4]) / 255.0
    }

    pub(crate) fn k5(self) -> f32 {
        f32::from(self.coefficients[5]) / 256.0
    }
}

pub(super) fn decode_color_common(value: u32) -> ColorSource {
    match value {
        0 => ColorSource::Combined,
        1 => ColorSource::Texel0,
        2 => ColorSource::Texel1,
        3 => ColorSource::Primitive,
        4 => ColorSource::Shade,
        5 => ColorSource::Environment,
        _ => ColorSource::Zero,
    }
}

pub(super) fn decode_color_a(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        7 => ColorSource::Noise,
        _ => ColorSource::Zero,
    }
}

pub(super) fn decode_color_b(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::KeyCenter,
        7 => ColorSource::K4,
        _ => ColorSource::Zero,
    }
}

pub(super) fn decode_color_c(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::KeyScale,
        7 => ColorSource::CombinedAlpha,
        8 => ColorSource::Texel0Alpha,
        9 => ColorSource::Texel1Alpha,
        10 => ColorSource::PrimitiveAlpha,
        11 => ColorSource::ShadeAlpha,
        12 => ColorSource::EnvironmentAlpha,
        13 => ColorSource::LodFraction,
        14 => ColorSource::PrimLodFraction,
        15 => ColorSource::K5,
        _ => ColorSource::Zero,
    }
}

pub(super) fn decode_color_d(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        _ => ColorSource::Zero,
    }
}

pub(super) fn decode_alpha_abd(value: u32) -> AlphaSource {
    match value {
        0 => AlphaSource::Combined,
        1 => AlphaSource::Texel0,
        2 => AlphaSource::Texel1,
        3 => AlphaSource::Primitive,
        4 => AlphaSource::Shade,
        5 => AlphaSource::Environment,
        6 => AlphaSource::One,
        _ => AlphaSource::Zero,
    }
}

pub(super) fn decode_alpha_c(value: u32) -> AlphaSource {
    match value {
        0 => AlphaSource::LodFraction,
        1 => AlphaSource::Texel0,
        2 => AlphaSource::Texel1,
        3 => AlphaSource::Primitive,
        4 => AlphaSource::Shade,
        5 => AlphaSource::Environment,
        6 => AlphaSource::PrimLodFraction,
        _ => AlphaSource::Zero,
    }
}

/// An immutable texture view plus its complete public per-axis tile-coordinate
/// mode, ready for the rasterizer to sample. Display-list textures retain a
/// physical TMEM snapshot and render-tile descriptor; hand-built fixtures use
/// the RGBA8888 row-major buffer. Both backings are reference-counted so many
/// primitives can share one command-ordered image (`F3DEX2-CONCEPTS.md` §5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    /// Public GBI image format/size wire values retained for copy-mode
    /// legality checks and future format-preserving framebuffer copies.
    pub format: u8,
    pub size: u8,
    pub width: u32,
    pub height: u32,
    /// RGBA8888, `width * height * 4` bytes, row-major top-left origin.
    pub texels: std::rc::Rc<Vec<u8>>,
    /// S-axis clamp-enable bit. A zero mask still implies clamp regardless of
    /// this bit, per Programming Manual Chapter 13, "Clamp S,T".
    pub clamp_s: bool,
    /// T-axis clamp-enable bit.
    pub clamp_t: bool,
    /// Per-axis mirror-enable bits.
    pub mirror_s: bool,
    pub mirror_t: bool,
    /// Number of low coordinate bits passed by wrapping (0..=15). Zero means
    /// no mask and therefore implicit clamp.
    pub mask_s: u8,
    pub mask_t: u8,
    /// Public four-bit post-perspective coordinate shift encodings.
    pub shift_s: u8,
    pub shift_t: u8,
    /// Tile-coordinate origin in texels (`uls/ult` quarter-texel fields).
    /// Vertex S/T are expressed in the image's coordinate domain, so the
    /// sampled coordinate is relative to this loaded tile origin.
    pub origin_s: f32,
    pub origin_t: f32,
    /// Immutable physical TMEM snapshot plus the render-tile descriptor.
    /// Display-list textures use this backing so sampling observes tile base,
    /// line stride, odd-row bank swapping, format reinterpretation, and data
    /// loaded through a different tile descriptor. Hand-built reference
    /// fixtures retain the decoded `texels` backing above.
    pub(crate) tmem: Option<std::rc::Rc<TmemTexture>>,
    /// Immutable tile set captured when a textured primitive is emitted.
    /// Loaded textures inside the snapshot never carry another snapshot, so
    /// the `Rc` indirection is finite and many primitives can share it.
    pub(crate) lod: Option<std::rc::Rc<TextureLodSnapshot>>,
}

/// One copy-cycle texture sample.
///
/// Copy mode writes supported 8-bit texels as their original memory byte, but
/// alpha compare still consumes the source format's decoded alpha. Keeping
/// both values prevents IA8's packed intensity/alpha byte from being rebuilt
/// from an RGBA intermediate at the framebuffer boundary.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct CopyTextureSample {
    pub rgba: [u8; 4],
    pub direct_8bit: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextureLodSnapshot {
    pub(super) tiles: [Option<Texture>; 8],
    pub(super) primitive_tile: u8,
    pub(super) max_level: u8,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct TextureDerivatives {
    pub dsdx: f32,
    pub dtdx: f32,
    pub dsdy: f32,
    pub dtdy: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct TextureSampleRequest {
    pub s: f32,
    pub t: f32,
    pub derivatives: TextureDerivatives,
    pub other_mode: OtherMode,
    pub convert: ConvertState,
    pub min_level: u8,
    pub require_texel1: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct TextureLodSelection {
    pub tile0: u8,
    pub tile1: u8,
    pub fraction: f32,
}

/// Post-perspective texture coordinate in the signed S10.5 fixed-point space.
///
/// Programming Manual 13.7 states that the texture unit has five fractional
/// coordinate bits, and 13.11 gives -1024..+1023.99 as the nominal input
/// window. On silicon the coordinate unit is a FIXED-WIDTH signed register:
/// interpolated coordinates that leave that window (legitimately common for
/// wrapped/tiled surfaces addressed with G_TX_WRAP/G_TX_MIRROR -- floors,
/// walls, sky domes, and the deep attract/menu scenes WM2000 draws) overflow
/// MODULARLY into the register, and the downstream per-tile clamp/mirror/wrap
/// addressing (`address_texel`) resolves them to an in-bounds texel. It does
/// not trap. Match that here: quantize to the containing 1/32-texel cell and
/// wrap the integer into the S10.5 register width, rather than panicking on a
/// coordinate hardware handles routinely. A non-finite coordinate is still a
/// genuine unsupported condition (no valid register value) and traps. This
/// quantize-and-wrap is a bounded host policy until silicon
/// reciprocal/quantization traces exist.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct TextureCoordinateS10_5(pub(super) i16);

impl TextureCoordinateS10_5 {
    const FRACTION_BITS: u32 = 5;
    const SCALE: i64 = 1 << Self::FRACTION_BITS;

    pub(super) fn from_texels_bounded(coordinate: f32) -> Self {
        if !coordinate.is_finite() {
            crate::render_unsupported_panic(
                "render.gbi.texture-coordinate-range",
                "non-finite coordinate reached RDP texture sampler",
            );
        }
        let scaled = f64::from(coordinate) * Self::SCALE as f64;
        let quantized = scaled.floor();
        // Reduce modulo 2^16 into the signed S10.5 register width. `quantized`
        // is a finite, floored f64; take it through i64 (always exact for a
        // floored product of an f32 texel count and 32) then wrap to i16 --
        // the hardware coordinate register's fixed-width overflow behavior.
        // In-range coordinates are unaffected (identical to the previous
        // exact conversion), so existing render-parity fixtures are unchanged.
        let wrapped = (quantized as i64 as u64 & 0xFFFF) as u16 as i16;
        Self(wrapped)
    }

    pub(super) fn shifted(self, encoded: u8) -> TextureCoordinateAccumulator5 {
        let raw = i64::from(self.0);
        match encoded {
            0 => TextureCoordinateAccumulator5(raw),
            1..=10 => TextureCoordinateAccumulator5(raw >> encoded),
            11..=15 => TextureCoordinateAccumulator5(
                raw.checked_mul(1_i64 << (16 - encoded))
                    .expect("RDP texture coordinate left shift overflowed fixed-point host range"),
            ),
            _ => unreachable!("G_SETTILE shift is a four-bit field"),
        }
    }
}

/// Wider five-fractional-bit host accumulator created only after a public tile
/// shift. Left shifts 11..=15 can expand a valid signed S10.5 input through an
/// S15.5-equivalent magnitude before the S10.2 tile origin is subtracted.
/// This width is a safe host mechanism, not a claim about a silicon register.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct TextureCoordinateAccumulator5(pub(super) i64);

impl TextureCoordinateAccumulator5 {
    pub(super) fn relative_to(self, origin: TextureCoordinateS10_5) -> Self {
        Self(
            self.0
                .checked_sub(i64::from(origin.0))
                .expect("RDP texture origin subtraction overflowed fixed-point host range"),
        )
    }

    pub(super) fn texel(self) -> i64 {
        self.0.div_euclid(TextureCoordinateS10_5::SCALE)
    }

    pub(super) fn fraction(self) -> i64 {
        self.0.rem_euclid(TextureCoordinateS10_5::SCALE)
    }
}

pub(super) fn filter_three_nearest_s10_5(samples: [[u8; 4]; 4], sf: i64, tf: i64) -> [u8; 4] {
    debug_assert!((0..TextureCoordinateS10_5::SCALE).contains(&sf));
    debug_assert!((0..TextureCoordinateS10_5::SCALE).contains(&tf));
    std::array::from_fn(|channel| {
        let [c00, c10, c01, c11] = samples.map(|sample| i64::from(sample[channel]));
        let value = if sf + tf <= TextureCoordinateS10_5::SCALE {
            c00 * TextureCoordinateS10_5::SCALE + sf * (c10 - c00) + tf * (c01 - c00)
        } else {
            c11 * TextureCoordinateS10_5::SCALE
                + (TextureCoordinateS10_5::SCALE - sf) * (c01 - c11)
                + (TextureCoordinateS10_5::SCALE - tf) * (c10 - c11)
        };
        // Preserve the reference lane's round-to-nearest output policy;
        // public documentation does not establish the silicon filter
        // accumulator width or tie rule.
        ((value + TextureCoordinateS10_5::SCALE / 2) / TextureCoordinateS10_5::SCALE).clamp(0, 255)
            as u8
    })
}

/// Which public texture-coordinate path owns clamp selection. Ordinary
/// point/filter sampling consumes the programmed per-axis clamp bit (with the
/// zero-mask implicit clamp rule). Programming Manual Chapter 13.11 states
/// that copy mode disables clamping, so that path cannot observe those bits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TextureAddressMode {
    Programmed,
    Copy,
}

pub(super) fn texture_axis_address(
    coordinate: i64,
    dimension: u32,
    clamp: bool,
    mirror: bool,
    mask: u8,
    mode: TextureAddressMode,
) -> u32 {
    if dimension == 0 {
        return 0;
    }
    assert!(mask <= 15, "G_SETTILE mask exceeds its four-bit field");

    // Outside copy mode, mask zero forces clamping. With a nonzero mask, the
    // explicit clamp bit clamps before mirror/mask; this reproduces the
    // manual's SH=11/mask=2 example, where every input above 11 resolves to
    // mirrored texel 3. Copy mode bypasses both clamp sources and proceeds to
    // its documented wrap/mirror addressing.
    let clamps = mode == TextureAddressMode::Programmed && (mask == 0 || clamp);
    let coordinate = if clamps {
        coordinate.clamp(0, i64::from(dimension) - 1)
    } else {
        coordinate
    };
    if mask == 0 {
        return coordinate as u32;
    }

    let low_mask = (1_i64 << mask) - 1;
    if mirror && coordinate & (1_i64 << mask) != 0 {
        ((!coordinate) & low_mask) as u32
    } else {
        (coordinate & low_mask) as u32
    }
}

/// Per-primitive snapshot of the RDP scissor rectangle, in screen pixels.
/// Lower-right edges are exclusive. `field`/`keep_odd` retain the public
/// Set Scissor command's interlace controls: when enabled, an entire opposite-
/// parity scanline is rejected before coverage or image writes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScissorRect {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub field: bool,
    pub keep_odd: bool,
}

impl ScissorRect {
    pub(crate) fn framebuffer(width: u32, height: u32) -> Self {
        Self {
            ulx: 0.0,
            uly: 0.0,
            lrx: width as f32,
            lry: height as f32,
            field: false,
            keep_odd: false,
        }
    }

    pub(crate) fn line_enabled(self, y: i32) -> bool {
        !self.field || y.rem_euclid(2) == i32::from(self.keep_odd)
    }
}

impl Texture {
    pub(crate) fn with_lod_snapshot(
        mut self,
        tiles: [Option<Texture>; 8],
        primitive_tile: u8,
        max_level: u8,
    ) -> Self {
        debug_assert!(tiles.iter().flatten().all(|texture| texture.lod.is_none()));
        self.lod = Some(std::rc::Rc::new(TextureLodSnapshot {
            tiles,
            primitive_tile,
            max_level,
        }));
        self
    }

    /// Point-sample at texel coordinates `(s, t)`, applying the tile's shift,
    /// clamp, mirror, and mask state per axis. Integer coordinates address
    /// texel centers in the public GBI coordinate domain; fractional parts
    /// select the same texel until the next integer boundary.
    pub fn sample(&self, s: f32, t: f32) -> [u8; 4] {
        self.sample_filtered(s, t, TextureFilter::Point)
    }

    /// Point-sample through the copy-cycle coordinate path. Chapter 13.11,
    /// "Restrictions," specifies that copy implicitly disables clamping while
    /// retaining wrap/mirror for supported texel sizes.
    pub(crate) fn sample_copy(&self, s: f32, t: f32) -> CopyTextureSample {
        let (s, t) = self.relative_coordinates(s, t);
        let (x, y) = self.address_texel(s.texel(), t.texel(), TextureAddressMode::Copy);
        let rgba = self.sample_addressed(x, y);
        let direct_8bit = if self.size != ColorImage::BITS_8 {
            None
        } else {
            match self.format {
                ColorImage::I_FORMAT | ColorImage::CI_FORMAT => Some(
                    self.tmem
                        .as_ref()
                        .map_or(rgba[0], |tmem| tmem.raw_texel(x as usize, y as usize) as u8),
                ),
                ColorImage::IA_FORMAT => Some(self.tmem.as_ref().map_or_else(
                    || {
                        debug_assert_eq!(rgba[0], rgba[1]);
                        debug_assert_eq!(rgba[0], rgba[2]);
                        debug_assert_eq!(rgba[0] >> 4, rgba[0] & 0xf);
                        debug_assert_eq!(rgba[3] >> 4, rgba[3] & 0xf);
                        (rgba[0] & 0xf0) | (rgba[3] >> 4)
                    },
                    |tmem| tmem.raw_texel(x as usize, y as usize) as u8,
                )),
                _ => None,
            }
        };
        CopyTextureSample { rgba, direct_8bit }
    }

    /// Sample through the RDP texture-filter mode.
    ///
    /// Nintendo's Programming Manual, "TF: Texture Filter" and "Sampling
    /// Overview", defines point selection, a four-texel box average, and the
    /// hardware's bilerp optimization: triangular interpolation of the three
    /// nearest samples selected by the sample's position in the 2x2 cell.
    /// Keeping this on `Texture` makes triangles and rectangles consume the
    /// same clean-room filter rather than growing backend-specific samplers.
    pub fn sample_filtered(&self, s: f32, t: f32, filter: TextureFilter) -> [u8; 4] {
        self.sample_filtered_with_address_mode(s, t, filter, TextureAddressMode::Programmed)
    }

    pub(super) fn sample_filtered_with_address_mode(
        &self,
        s: f32,
        t: f32,
        filter: TextureFilter,
        address_mode: TextureAddressMode,
    ) -> [u8; 4] {
        let texel = |x: i64, y: i64| -> [u8; 4] {
            let (x, y) = self.address_texel(x, y, address_mode);
            self.sample_addressed(x, y)
        };

        let (s, t) = self.relative_coordinates(s, t);
        let s0 = s.texel();
        let t0 = t.texel();
        if filter == TextureFilter::Point {
            return texel(s0, t0);
        }
        assert_ne!(
            filter,
            TextureFilter::Reserved,
            "reserved RDP texture-filter mode reached sampler"
        );

        let samples = [
            texel(s0, t0),
            texel(s0 + 1, t0),
            texel(s0, t0 + 1),
            texel(s0 + 1, t0 + 1),
        ];
        if filter == TextureFilter::Average {
            return std::array::from_fn(|channel| {
                let sum = samples
                    .iter()
                    .map(|sample| u16::from(sample[channel]))
                    .sum::<u16>();
                ((sum + 2) / 4) as u8
            });
        }

        let sf = s.fraction();
        let tf = t.fraction();
        filter_three_nearest_s10_5(samples, sf, tf)
    }

    pub(super) fn relative_coordinates(
        &self,
        s: f32,
        t: f32,
    ) -> (TextureCoordinateAccumulator5, TextureCoordinateAccumulator5) {
        let s = TextureCoordinateS10_5::from_texels_bounded(s)
            .shifted(self.shift_s)
            .relative_to(TextureCoordinateS10_5::from_texels_bounded(self.origin_s));
        let t = TextureCoordinateS10_5::from_texels_bounded(t)
            .shifted(self.shift_t)
            .relative_to(TextureCoordinateS10_5::from_texels_bounded(self.origin_t));
        (s, t)
    }

    pub(super) fn address_texel(&self, x: i64, y: i64, address_mode: TextureAddressMode) -> (u32, u32) {
        (
            texture_axis_address(
                x,
                self.width,
                self.clamp_s,
                self.mirror_s,
                self.mask_s,
                address_mode,
            ),
            texture_axis_address(
                y,
                self.height,
                self.clamp_t,
                self.mirror_t,
                self.mask_t,
                address_mode,
            ),
        )
    }

    pub(super) fn sample_addressed(&self, x: u32, y: u32) -> [u8; 4] {
        if let Some(tmem) = &self.tmem {
            return tmem.sample(x as usize, y as usize);
        }
        assert!(
            x < self.width && y < self.height,
            "G_SETTILE masks ({}, {}) address texel ({x}, {y}) outside decoded {}x{} fixture",
            self.mask_s,
            self.mask_t,
            self.width,
            self.height,
        );
        let offset = ((y * self.width + x) * 4) as usize;
        assert!(
            offset + 4 <= self.texels.len(),
            "texture sample ({x}, {y}) exceeds {}x{} RGBA buffer of {} bytes",
            self.width,
            self.height,
            self.texels.len()
        );
        [
            self.texels[offset],
            self.texels[offset + 1],
            self.texels[offset + 2],
            self.texels[offset + 3],
        ]
    }

    /// Run the public texture-filter conversion selection. `G_TC_CONV`
    /// performs point conversion, `G_TC_FILTCONV` filters then converts, and
    /// `G_TC_FILT` returns the filtered texel unchanged.
    pub(crate) fn sample_rdp(
        &self,
        s: f32,
        t: f32,
        other_mode: OtherMode,
        convert: ConvertState,
    ) -> [u8; 4] {
        match other_mode.texture_convert() {
            0 => convert.convert_texel(self.sample_filtered(s, t, TextureFilter::Point)),
            5 => convert.convert_texel(self.sample_filtered(s, t, other_mode.texture_filter())),
            6 => self.sample_filtered(s, t, other_mode.texture_filter()),
            mode => panic!("reserved RDP texture-convert mode {mode} reached sampler"),
        }
    }

    pub(super) fn lod_selection(
        snapshot: &TextureLodSnapshot,
        derivatives: TextureDerivatives,
        other_mode: OtherMode,
        min_level: u8,
    ) -> TextureLodSelection {
        if !other_mode.texture_lod() {
            return TextureLodSelection {
                tile0: snapshot.primitive_tile,
                tile1: snapshot.primitive_tile.wrapping_add(1) & 7,
                fraction: 0.0,
            };
        }

        let detail = other_mode.texture_detail();
        assert_ne!(
            detail, 3,
            "reserved RDP texture-detail mode reached sampler"
        );
        let lod = derivatives
            .dsdx
            .abs()
            .max(derivatives.dtdx.abs())
            .max(derivatives.dsdy.abs())
            .max(derivatives.dtdy.abs());
        assert!(
            lod.is_finite(),
            "non-finite texture derivative reached RDP LOD"
        );
        let minimum = f32::from(min_level) / 255.0;
        let clamped = lod.max(minimum);
        let magnifying = clamped <= 1.0;
        let unclamped_tile = if clamped < 2.0 {
            0
        } else {
            clamped.floor().log2().floor() as u8
        };
        let level = unclamped_tile.min(snapshot.max_level.min(7));
        let base_fraction = if magnifying {
            clamped
        } else {
            (clamped / (1_u32 << level) as f32 - 1.0).clamp(0.0, 1.0)
        };

        // Programming Manual Chapter 13.7 Tables 3-4. Detail shifts both
        // cycle tiles above the primitive detail tile outside magnification;
        // sharpen keeps the ordinary adjacent pair but extrapolates with a
        // negative fraction while magnifying. Clamp mode reuses the finest
        // tile for both cycles during magnification.
        let (offset0, offset1, fraction) = match (detail, magnifying) {
            (2, true) => (0, 1, base_fraction.max(minimum)),
            (2, false) => (
                level.saturating_add(1),
                level.saturating_add(2),
                base_fraction,
            ),
            (1, true) => (0, 1, clamped - 1.0),
            (1, false) => (level, level.saturating_add(1), base_fraction),
            (0, true) => (0, 0, base_fraction),
            (0, false) => (
                level,
                level.saturating_add(1).min(snapshot.max_level),
                base_fraction,
            ),
            _ => unreachable!("texture detail field is two bits"),
        };
        TextureLodSelection {
            tile0: snapshot.primitive_tile.wrapping_add(offset0) & 7,
            tile1: snapshot.primitive_tile.wrapping_add(offset1) & 7,
            fraction,
        }
    }

    pub(crate) fn sample_rdp_pair(
        &self,
        fallback_texel1: Option<&Texture>,
        request: TextureSampleRequest,
    ) -> ([u8; 4], [u8; 4], f32) {
        let TextureSampleRequest {
            s,
            t,
            derivatives,
            other_mode,
            convert,
            min_level,
            require_texel1,
        } = request;
        let Some(snapshot) = self.lod.as_deref() else {
            assert!(
                !other_mode.texture_lod() && other_mode.texture_detail() == 0,
                "texture LOD/detail reached sampler without an immutable tile snapshot"
            );
            let texel0 = self.sample_rdp(s, t, other_mode, convert);
            let texel1 = if require_texel1 {
                fallback_texel1
                    .expect("RDP combiner selected TEXEL1 without a decoded tile+1 image")
                    .sample_rdp(s, t, other_mode, convert)
            } else {
                texel0
            };
            return (texel0, texel1, 0.0);
        };
        let selection = Self::lod_selection(snapshot, derivatives, other_mode, min_level);
        let tile0 = snapshot.tiles[usize::from(selection.tile0)]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "RDP LOD selected tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    selection.tile0
                )
            });
        let texel0 = tile0.sample_rdp(s, t, other_mode, convert);
        // With LOD disabled and no TEXEL1 combiner input, the second tile has
        // no observable consumer. Avoiding that complete filter/address/TMEM
        // read is especially material for the reference software rasterizer;
        // LOD mode retains both selected-tile validations below.
        if !other_mode.texture_lod() && !require_texel1 {
            return (texel0, texel0, selection.fraction);
        }
        let tile1 = snapshot.tiles[usize::from(selection.tile1)]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "RDP LOD selected tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    selection.tile1
                )
            });
        (
            texel0,
            tile1.sample_rdp(s, t, other_mode, convert),
            selection.fraction,
        )
    }
}

/// A decoded, screen-space-ready triangle (three already-resolved
/// vertices) -- the display-list decoder's actual output, consumed by the
/// rasterizer in `raster.rs`.
#[derive(Clone, Debug, Default)]
pub struct Triangle {
    pub v: [Vertex; 3],
    /// RDP scissor active when this triangle was emitted. `None` preserves
    /// framebuffer-only clipping for the legacy fixture decoder.
    pub scissor: Option<ScissorRect>,
    /// The culling mode in effect (from `G_GEOMETRYMODE`) when this triangle
    /// was emitted. Carried per-triangle because geometry mode is decode-time
    /// RSP state that can change between `G_TRI*` commands; the rasterizer
    /// reads it to cull by winding. `None` for the simple reference path.
    pub cull: CullMode,
    /// The texture bound (via `G_TEXTURE` enable + a loaded tile) when this
    /// triangle was emitted, if any. `None` means texturing was disabled (or
    /// this is a fixture-only primitive); an enabled tile without live TMEM
    /// traps during decode rather than arriving here as white. The rasterizer
    /// modulates the sampled texel by the interpolated shade color
    /// (`F3DEX2-CONCEPTS.md` §5.2, the MODULATE combiner).
    pub texture: Option<Texture>,
    /// RDP other-mode and alpha-threshold state in effect when this triangle
    /// was emitted. Like culling/texture, this is snapshotted per triangle
    /// because later display-list commands may mutate global decode state.
    pub other_mode: OtherMode,
    /// Color-combiner mode and its primitive/environment inputs in effect
    /// when this triangle was emitted. Kept per-triangle for the same reason
    /// as texture/cull state: later display-list commands may change it.
    pub combiner: CombinerState,
    /// RDP framebuffer blender state in effect when this triangle was emitted.
    /// This is derived from the same other-mode snapshot: cycle type,
    /// `FORCE_BL`, the two `GBL_c1`/`GBL_c2` selector tuples, and the constant
    /// colors those tuples can address.
    pub blender: BlenderState,
}

/// One F3DEX2/L3DEX line after vertex-cache resolution.
///
/// Public `gSPLineW3D` defines `width = 1.5 + wd / 2` pixels. The command's
/// flat-shading flag is represented by endpoint order on the wire, so `v[0]`
/// is also the selected flat color when smooth shading is disabled.
#[derive(Clone, Debug)]
pub struct Line {
    pub v: [Vertex; 2],
    pub width: f32,
    pub smooth_shading: bool,
    pub scissor: Option<ScissorRect>,
    pub texture: Option<Texture>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
}

pub(super) struct LineDecodeSnapshot {
    pub(super) smooth_shading: bool,
    pub(super) texture: Option<Texture>,
    pub(super) other_mode: OtherMode,
    pub(super) combiner: CombinerState,
    pub(super) blender: BlenderState,
    pub(super) scissor: Option<ScissorRect>,
    pub(super) viewport: Option<Viewport>,
    pub(super) clip_ratio: ClipRatio,
}

/// The four 64-bit edge-coefficient words shared by every raw RDP triangle.
/// Y values are signed S11.2 and X/slopes are signed 16.16 on the wire.
///
/// Provenance: SGI *RDP Command Summary*, Tables 11-12 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpEdgeCoefficients {
    pub left_major: bool,
    pub level: u8,
    pub tile: u8,
    pub yl: i16,
    pub ym: i16,
    pub yh: i16,
    pub xl: i32,
    pub dxldy: i32,
    pub xh: i32,
    pub dxhdy: i32,
    pub xm: i32,
    pub dxmdy: i32,
}

/// The two 64-bit Z coefficient words appended by raw opcodes with bit 0 set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpZCoefficients {
    pub z: i32,
    pub dzdx: i32,
    pub dzde: i32,
    pub dzdy: i32,
}

/// The eight 64-bit shade coefficient words appended by raw opcodes with
/// bit 2 set. Each component is retained as signed 16.16 so negative color
/// gradients survive ingestion.
///
/// Provenance: SGI *RDP Command Summary*, Table 13 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpShadeCoefficients {
    pub color: [i32; 4],
    pub dcdx: [i32; 4],
    pub dcde: [i32; 4],
    pub dcdy: [i32; 4],
}

/// The eight 64-bit texture coefficient words appended by raw opcodes with
/// bit 1 set. S, T, normalized inverse-W, and their gradients remain signed
/// 16.16 values until vertex reconstruction.
///
/// Provenance: SGI *RDP Command Summary*, Table 14 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpTextureCoefficients {
    pub stw: [i32; 3],
    pub dstdx: [i32; 3],
    pub dstde: [i32; 3],
    pub dstdy: [i32; 3],
}

/// One raw RDP triangle retaining the hardware's edge and attribute planes.
/// Keeping this distinct from [`Triangle`] prevents the command decoder from
/// throwing away major-edge direction and `d/de` stepping before rasterization.
#[derive(Clone, Debug)]
pub struct RawRdpTriangle {
    pub edge: RdpEdgeCoefficients,
    pub shade: Option<RdpShadeCoefficients>,
    pub texture_coefficients: Option<RdpTextureCoefficients>,
    pub z: Option<RdpZCoefficients>,
    pub texture: Option<Texture>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
    pub scissor: Option<ScissorRect>,
}

/// One RDP color-image descriptor from `G_SETCIMG`.
///
/// Format and size retain their public GBI wire values. The reference path
/// supports all three public RDP memory-interface color-image sizes: 8-bit
/// intensity, RGBA16, and RGBA32. Retaining the raw format field lets invalid
/// 16/32-bit combinations fail by name while the size-defined 8-bit layout is
/// represented without inventing a palette.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColorImage {
    pub format: u8,
    pub size: u8,
    pub width: u16,
    pub address: u32,
}

/// Legal public RDP color-image memory layouts.
///
/// The memory interface exposes one size-defined 8-bit layout plus RGBA16 and
/// RGBA32. Classifying the raw `G_SETCIMG` fields once prevents individual
/// import, raster, and writeback paths from accepting different format sets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorImageLayout {
    Index8,
    Rgba16,
    Rgba32,
}

impl ColorImageLayout {
    pub const ALL: [Self; 3] = [Self::Index8, Self::Rgba16, Self::Rgba32];

    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Index8 => 1,
            Self::Rgba16 => 2,
            Self::Rgba32 => 4,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Index8 => "I8/CI8",
            Self::Rgba16 => "RGBA16",
            Self::Rgba32 => "RGBA32",
        }
    }
}

/// One fully classified color-image target switch.
///
/// The RDP permits every transition among its three public memory layouts.
/// Constructing this value from the raw `G_SETCIMG` descriptors makes the
/// admission check a single, typed boundary shared by commit and import;
/// unsupported format/size pairs trap before either side mutates RDRAM.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColorImageLayoutTransition {
    pub from: ColorImageLayout,
    pub to: ColorImageLayout,
}

/// RDP depth-image register. The public command carries only a DRAM address;
/// dimensions follow the active color image/scissor state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DepthImage {
    pub address: u32,
}

/// Uniform RDP primitive Z/DeltaZ registers written by `G_SETPRIMDEPTH`.
/// Public libultra packs Z in `w1[31:16]` and DeltaZ in `w1[15:0]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PrimitiveDepth {
    pub z: u16,
    pub delta_z: u16,
}

impl ColorImage {
    pub const RGBA_FORMAT: u8 = 0;
    pub const CI_FORMAT: u8 = 2;
    pub const IA_FORMAT: u8 = 3;
    pub const I_FORMAT: u8 = 4;
    pub const BITS_8: u8 = 1;
    pub const BITS_16: u8 = 2;
    pub const BITS_32: u8 = 3;

    pub const fn layout(self) -> Option<ColorImageLayout> {
        if self.size == Self::BITS_8 {
            Some(ColorImageLayout::Index8)
        } else if self.format == Self::RGBA_FORMAT && self.size == Self::BITS_16 {
            Some(ColorImageLayout::Rgba16)
        } else if self.format == Self::RGBA_FORMAT && self.size == Self::BITS_32 {
            Some(ColorImageLayout::Rgba32)
        } else {
            None
        }
    }

    pub const fn is_rgba16(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Rgba16))
    }

    pub const fn is_rgba32(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Rgba32))
    }

    pub const fn is_intensity8(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Index8))
    }

    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self.layout() {
            Some(layout) => Some(layout.bytes_per_pixel()),
            None => None,
        }
    }

    pub fn transition_to(self, next: Self) -> ColorImageLayoutTransition {
        let from = self.layout().unwrap_or_else(|| {
            crate::render_unsupported_panic(
                "render.gbi.color-image-layout",
                format!(
                    "unsupported source color-image layout: format={} size={}",
                    self.format, self.size
                ),
            )
        });
        let to = next.layout().unwrap_or_else(|| {
            crate::render_unsupported_panic(
                "render.gbi.color-image-layout",
                format!(
                    "unsupported destination color-image layout: format={} size={}",
                    next.format, next.size
                ),
            )
        });
        ColorImageLayoutTransition { from, to }
    }
}

/// One rectangle primitive with all RDP state required at its command position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FillRectangle {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub fill_color: u32,
    pub cycle_type: CycleType,
    pub scissor: Option<ScissorRect>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
}

/// One complete 16-byte texture-rectangle command and its decode-time RDP
/// state. Screen coordinates are pixels, texture origins are texels, and the
/// gradients remain raw signed S5.10 values so cycle-specific execution can
/// apply the documented stepping rule without losing precision.
#[derive(Clone, Debug)]
pub struct TextureRectangle {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub tile: u8,
    pub s: f32,
    pub t: f32,
    pub dsdx: i16,
    pub dtdy: i16,
    pub flip: bool,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
    pub scissor: Option<ScissorRect>,
    pub texture: Option<Texture>,
    /// With LOD disabled, TEXEL1 comes from the tile immediately after the
    /// command's TEXEL0 tile (N64 Graphics Tutorial, "Multi-tile Texture
    /// Rectangles"). Retaining it makes two-cycle rectangle programs real
    /// rather than aliasing TEXEL1 to TEXEL0.
    pub texture1: Option<Texture>,
}

/// Ordered RSP/RDP work produced by the F3DEX2 decoder.
///
/// A triangle-only return type loses framebuffer changes, fills, and sync
/// boundaries. This stream is the shared representation that later texrect,
/// copy-cycle, raw-RDP, and framebuffer-format work extends without another
/// decoder/backend seam change.
#[derive(Clone, Debug)]
pub enum RenderOp {
    Triangle(Triangle),
    Line(Line),
    RawTriangle(RawRdpTriangle),
    SetColorImage(ColorImage),
    SetDepthImage(DepthImage),
    SetPrimitiveDepth(PrimitiveDepth),
    FillRectangle(FillRectangle),
    TextureRectangle(TextureRectangle),
    FullSync,
}

/// One color input selected for the RDP blender's `P` or `M` term.
/// Values are the public `G_BL_CLR_*` encodings (gbi.h:612-615).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendColorInput {
    #[default]
    Combined,
    Framebuffer,
    Blend,
    Fog,
}

/// The multiplier selected for the blender's `A` term (`G_BL_A_*`,
/// gbi.h:618-622).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendAlphaInput {
    #[default]
    Combined,
    Fog,
    Shade,
    Zero,
}

/// The multiplier selected for the blender's `B` term (`G_BL_1MA`,
/// `G_BL_A_MEM`, `G_BL_1`, `G_BL_0`; gbi.h:616-622).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendBInput {
    #[default]
    OneMinusA,
    FramebufferAlpha,
    One,
    Zero,
}

/// One `GBL_c1`/`GBL_c2` tuple, evaluated as `P*A + M*B`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlendCycle {
    pub p: BlendColorInput,
    pub a: BlendAlphaInput,
    pub m: BlendColorInput,
    pub b: BlendBInput,
}

/// Minimal, per-triangle RDP blender snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlenderState {
    /// 0 for copy/fill (blender bypass), 1 for `G_CYC_1CYCLE`, 2 for
    /// `G_CYC_2CYCLE` (gbi.h:527-531).
    pub cycle_count: u8,
    pub force_blend: bool,
    pub cycles: [BlendCycle; 2],
    pub blend_color: [u8; 4],
    pub fog_color: [u8; 4],
}

impl BlenderState {
    pub(super) fn from_other_mode(low: u32, high: u32, blend_color: [u8; 4], fog_color: [u8; 4]) -> Self {
        let color = |bits: u32| match bits & 3 {
            0 => BlendColorInput::Combined,
            1 => BlendColorInput::Framebuffer,
            2 => BlendColorInput::Blend,
            _ => BlendColorInput::Fog,
        };
        let alpha = |bits: u32| match bits & 3 {
            0 => BlendAlphaInput::Combined,
            1 => BlendAlphaInput::Fog,
            2 => BlendAlphaInput::Shade,
            _ => BlendAlphaInput::Zero,
        };
        let b = |bits: u32| match bits & 3 {
            0 => BlendBInput::OneMinusA,
            1 => BlendBInput::FramebufferAlpha,
            2 => BlendBInput::One,
            _ => BlendBInput::Zero,
        };
        let cycle_type = (high >> 20) & 3;
        BlenderState {
            cycle_count: match cycle_type {
                0 => 1,
                1 => 2,
                _ => 0,
            },
            force_blend: low & 0x0000_4000 != 0,
            // gbi.h:624-627: c1 fields at 30/26/22/18, c2 at
            // 28/24/20/16. Keeping that order visible makes merges with the
            // fuller othermode decoder mechanical.
            cycles: [
                BlendCycle {
                    p: color(low >> 30),
                    a: alpha(low >> 26),
                    m: color(low >> 22),
                    b: b(low >> 18),
                },
                BlendCycle {
                    p: color(low >> 28),
                    a: alpha(low >> 24),
                    m: color(low >> 20),
                    b: b(low >> 16),
                },
            ],
            blend_color,
            fog_color,
        }
    }
}

/// The N64 SDK's public per-vertex wire format (`Vtx_t`): 16 bytes --
/// `ob[3]` (s16 x/y/z), `flag` (u16, unused here), `tc[2]` (s16 st, unused
/// here), `cn[4]` (u8 r/g/b/a). x/y/z are read as model-space coords and
/// transformed through the active matrix stack; `cn` is a flat vertex color.
pub(super) const VTX_STRIDE: usize = 16;
