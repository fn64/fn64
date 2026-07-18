//! An F3DEX2-family display-list decoder: enough opcodes to turn a real
//! OoT-era display list (segmented vertex/matrix data, an MVP transform
//! stack, and G_TRI1/G_TRI2/G_QUAD triangle commands) into screen-space
//! filled polygons -- plus a deliberately-loud skip for every opcode not
//! yet interpreted, so coverage is never silently overstated.
//!
//! ## Provenance
//!
//! `Gfx` is the N64 SDK's public 64-bit-word display-list command
//! encoding: every `gsSP*`/`gsDP*` macro in the publicly published
//! `gbi.h` header (redistributed in countless public SDK-header mirrors
//! and referenced throughout N64 homebrew/modding documentation) packs to
//! exactly this two-`u32` shape -- opcode in the top byte of the first
//! word, remaining fields packed by the specific opcode. Every opcode byte
//! value and bit-field offset below is cited to the F3DEX_GBI_2 branch of
//! the public `gbi.h` (`ultra64/gbi.h`): `gDma1p`/`gDma2p` word packing
//! (gbi.h ~2046-2090), `gsSPVertex` (~2150), `__gsSP1Triangle_w1` (~2320,
//! F3DEX branch: indices in w0, each `v*2`), `gsSPMatrix` (~2106),
//! `gsMoveWd`/`gsSPSegment` (~2267/2578), `gsSPDisplayList` (~2177). This
//! module reads only the raw wire values, not any vendor SDK/microcode C
//! source -- the encoding is packaging-level ABI, the same standing as this
//! project's other public-ABI citations (`os_task.h`'s `OSTask_t`).
//!
//! ## Scope
//!
//! Interpreted (real effect): `G_VTX` (load transformed vertices into the
//! 32-slot cache), `G_TRI1`/`G_TRI2`/`G_QUAD` (triangles referencing loaded
//! slots), `G_MTX`/`G_POPMTX` (modelview/projection stack), `G_MOVEWORD`
//! (segment table writes, `G_MW_SEGMENT`), `G_DL` (call/jump into a nested
//! display list), `G_SETOTHERMODE_H/L` (RDP cycle/filter/dither/render/alpha/
//! coverage/depth/blender state), `G_SETBLENDCOLOR` (alpha-test threshold),
//! `G_SETSCISSOR` (per-triangle raster clip rectangle), and `G_ENDDL` (stop).
//!
//! Explicitly acknowledged-and-skipped (logged by name via `skip_opcode`,
//! never a silent no-op): remaining framebuffer/sync state and any
//! unrecognized byte. Texture, lighting, RDP other-mode, alpha compare, and
//! the color-combiner and framebuffer-blender inputs needed by OoT are decoded. Skips are
//! rate-limited-per-opcode so a real DL doesn't spam
//! thousands of identical lines, but every distinct skipped opcode is
//! reported at least once.
use fn64_render::{RenderError, UcodeId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::{collections::BTreeMap, fmt::Write as _};

// --- Opcode bytes: F3DEX_GBI_2 branch of the public ultra64/gbi.h ---
pub const G_VTX: u8 = 0x01;
pub const G_TRI1: u8 = 0x05;
pub const G_TRI2: u8 = 0x06;
pub const G_QUAD: u8 = 0x07;
pub const G_TEXTURE: u8 = 0xD7;
pub const G_POPMTX: u8 = 0xD8;
pub const G_GEOMETRYMODE: u8 = 0xD9;
pub const G_MTX: u8 = 0xDA;
pub const G_MOVEWORD: u8 = 0xDB;
pub const G_MOVEMEM: u8 = 0xDC;
pub const G_DL: u8 = 0xDE;
pub const G_ENDDL: u8 = 0xDF;

/// `G_MW_SEGMENT` (gbi.h:1212) -- the `G_MOVEWORD` index that writes the
/// segment base-address table used to resolve segmented pointers.
const G_MW_SEGMENT: u16 = 0x06;

/// `G_MV_VIEWPORT` (gbi.h) -- the `G_MOVEMEM` index that DMAs a `Vp`
/// (viewport scale/translate) struct into RSP state (F3DEX2-CONCEPTS.md
/// §1.4/§3.5).
const G_MV_VIEWPORT: u8 = 8;

// --- F3DEX2 geometry-mode bits (F3DEX2-CONCEPTS.md §2.4) -----------------
/// Cull front-facing triangles.
const G_CULL_FRONT: u32 = 0x0000_0200;
/// Cull back-facing triangles (the common case).
const G_CULL_BACK: u32 = 0x0000_0400;
/// Enable vertex lighting. When set, a vertex's `cn[0..3]` bytes are a signed
/// s8 NORMAL (x,y,z), not an RGB color -- the vertex color is COMPUTED from
/// the loaded lights (ambient + per-directional N·L·color) instead of taken
/// from `cn` (`F3DEX2-CONCEPTS.md` §2.4; OoT gbi.h `G_LIGHTING`). Reading the
/// normal bytes as a flat color (the pre-lighting path) produced the
/// characteristic "rainbow fan" -- signed normal components reinterpreted as
/// unsigned color channels.
const G_LIGHTING: u32 = 0x0002_0000;

// --- F3DEX2 lighting: G_MOVEMEM/G_MOVEWORD indices + Light layout --------
/// `G_MV_LIGHT` (OoT gbi.h:1169) -- the `G_MOVEMEM` index that DMAs a `Light`
/// struct (diffuse color + direction, or an ambient color) into an RSP light
/// slot. F3DEX2 `gsSPLight` (gbi.h:2911) encodes `idx = G_MV_LIGHT` in the
/// w0 low byte and `ofs = n*24 + 24` (÷8 in the wire) in `field(w0,8,8)`.
const G_MV_LIGHT: u8 = 0x0a;
/// `G_MW_NUMLIGHT` (OoT gbi.h:1210) -- the `G_MOVEWORD` index that sets the
/// directional-light count. F3DEX2 `gsSPNumLights` (gbi.h:2887) writes
/// `NUML(n) = n*24` as the data word, so `numDirectional = w1 / 24`. The
/// AMBIENT light is the slot AFTER the directional ones (gbi.h:2902 note:
/// "the highest numbered light is always the ambient light").
const G_MW_NUMLIGHT: u16 = 0x02;
/// One `Light_t` on the wire is 16 bytes (OoT gbi.h:1311 -- `col[3]`, pad,
/// `colc[3]`, pad, `dir[3]`, pad), padded to a 16-byte `Light` union.
const LIGHT_STRIDE: usize = 16;
/// Max simultaneous lights F3DEX2 supports (7 directional + 1 ambient).
const MAX_LIGHTS: usize = 8;

// --- Additional F3DEX2 opcode bytes, named for the loud-skip log so the
// coverage report doesn't understate what a real OoT DL contains. These are
// acknowledged-and-skipped for a flat-shaded frame (F3DEX2-CONCEPTS.md §7).
const G_MODIFYVTX: u8 = 0x02;
const G_CULLDL: u8 = 0x03;
const G_BRANCH_Z: u8 = 0x04;
const G_LINE3D: u8 = 0x08;
const G_SPECIAL_1: u8 = 0xD5;
const G_DMA_IO: u8 = 0xD6;
const G_LOAD_UCODE: u8 = 0xDD;
/// `G_TEXRECT` / `G_TEXRECTFLIP` (gbi.h:126-127). Unlike every other
/// command in this decoder these are **two** 64-bit words wide (16 bytes):
/// `gsDPTextureRectangle` (gbi.h:4973) emits a second `Gfx` entry holding
/// the S/T coords + dsdx/dtdy. The decoder skips the RDP rectangle itself
/// (no 2D-rect rasterization yet) but MUST consume both words or it reads
/// the coord word as a bogus opcode and desyncs the stream.
const G_TEXRECT: u8 = 0xE4;
const G_TEXRECTFLIP: u8 = 0xE5;
/// First texrect coordinate half in F3DEX2's three-word texrect form
/// (gbi.h gDPTextureRectangle: E4, then E1 with s/t, then F1 with steps).
const G_RDPHALF_1: u8 = 0xE1;
const G_SETOTHERMODE_L: u8 = 0xE2;
const G_SETOTHERMODE_H: u8 = 0xE3;
/// Full RDP other-mode write (`gsDPSetOtherMode`; gbi.h:3724-3737).
const G_RDPSETOTHERMODE: u8 = 0xEF;
const G_SETSCISSOR: u8 = 0xED;
const G_LOADTLUT: u8 = 0xF0;
const G_SETTILESIZE: u8 = 0xF2;
const G_LOADBLOCK: u8 = 0xF3;
const G_LOADTILE: u8 = 0xF4;
const G_SETTILE: u8 = 0xF5;
const G_SETFOGCOLOR: u8 = 0xF8;
const G_SETBLENDCOLOR: u8 = 0xF9;
const G_SETPRIMCOLOR: u8 = 0xFA;
const G_SETENVCOLOR: u8 = 0xFB;
const G_SETCOMBINE: u8 = 0xFC;
const G_SETTIMG: u8 = 0xFD;
const G_SETZIMG: u8 = 0xFE;
const G_SETCIMG: u8 = 0xFF;

/// One decoded vertex in screen space (after MVP + viewport if a transform
/// was active, or raw `ob` coords if no matrix/viewport was loaded -- see
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
}

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
    high: u32,
    low: u32,
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
    fn decode(w0: u32, w1: u32) -> Self {
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

/// RDP color state snapshotted onto each emitted triangle.
///
/// This stays separate from the render/other-mode state being added on the
/// neighboring job: it contains only `G_SETCOMBINE`, primitive/environment
/// RGBA, and primitive LOD fraction inputs to the color equation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CombinerState {
    pub mode: CombinerMode,
    pub primitive: [u8; 4],
    pub environment: [u8; 4],
    pub prim_lod_fraction: u8,
}

fn decode_color_common(value: u32) -> ColorSource {
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

fn decode_color_a(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        7 => ColorSource::Noise,
        _ => ColorSource::Zero,
    }
}

fn decode_color_b(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::KeyCenter,
        7 => ColorSource::K4,
        _ => ColorSource::Zero,
    }
}

fn decode_color_c(value: u32) -> ColorSource {
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

fn decode_color_d(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        _ => ColorSource::Zero,
    }
}

fn decode_alpha_abd(value: u32) -> AlphaSource {
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

fn decode_alpha_c(value: u32) -> AlphaSource {
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

/// A decoded texture: RGBA8888 texels (row-major, top-left origin) plus its
/// dimensions and per-axis wrap mode, ready for the rasterizer to sample.
/// Reference-counted so many triangles sharing one bound tile don't each
/// clone the texel buffer. Built at `G_LOADBLOCK`/`G_LOADTILE` time by
/// decoding the `G_SETTIMG` image through the active tile descriptor
/// (`F3DEX2-CONCEPTS.md` §5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    /// RGBA8888, `width * height * 4` bytes, row-major top-left origin.
    pub texels: std::rc::Rc<Vec<u8>>,
    /// S-axis wrap: `true` = clamp to edge, `false` = wrap (repeat). Mirror
    /// is approximated as wrap for a first textured frame.
    pub clamp_s: bool,
    /// T-axis wrap (see `clamp_s`).
    pub clamp_t: bool,
    /// Tile-coordinate origin in texels (`uls/ult` quarter-texel fields).
    /// Vertex S/T are expressed in the image's coordinate domain, so the
    /// sampled coordinate is relative to this loaded tile origin.
    pub origin_s: f32,
    pub origin_t: f32,
}

/// Per-triangle snapshot of the RDP scissor rectangle, in screen pixels.
/// Lower-right edges are exclusive.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScissorRect {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
}

impl Texture {
    /// Nearest-neighbor sample at texel coords `(s, t)`, applying the tile's
    /// clamp/wrap mode per axis. Returns RGBA8888. (Point sampling, not
    /// bilinear -- adequate for a first recognizable textured frame; the RDP
    /// itself uses point sampling in copy/1-cycle-nofilter modes anyway.)
    pub fn sample(&self, s: f32, t: f32) -> [u8; 4] {
        let wrap = |coord: f32, dim: u32, clamp: bool| -> u32 {
            if dim == 0 {
                return 0;
            }
            let i = coord.floor() as i64;
            if clamp {
                i.clamp(0, dim as i64 - 1) as u32
            } else {
                // Positive modulo (wrap/repeat).
                (i.rem_euclid(dim as i64)) as u32
            }
        };
        let x = wrap(s - self.origin_s, self.width, self.clamp_s);
        let y = wrap(t - self.origin_t, self.height, self.clamp_t);
        let o = ((y * self.width + x) * 4) as usize;
        assert!(
            o + 4 <= self.texels.len(),
            "texture sample ({x}, {y}) exceeds {}x{} RGBA buffer of {} bytes",
            self.width,
            self.height,
            self.texels.len()
        );
        [
            self.texels[o],
            self.texels[o + 1],
            self.texels[o + 2],
            self.texels[o + 3],
        ]
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
    /// triangle was emitted, if any. `None` -> flat-shaded from vertex color
    /// only (untextured surface, or texturing disabled). The rasterizer
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
    fn from_other_mode(low: u32, high: u32, blend_color: [u8; 4], fog_color: [u8; 4]) -> Self {
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
const VTX_STRIDE: usize = 16;

/// A 4x4 column-vector transform (row-major storage: `m[row][col]`), f32.
/// Built from an N64 fixed-point `Mtx` (see `read_mtx`) or the identity.
type Mat4 = [[f32; 4]; 4];

fn identity() -> Mat4 {
    let mut m = [[0.0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// TEMP instrumentation (env `FN64_DUMP_PROJ=1`): true only while dumping the
/// projection/vertex data for the FIRST substantial gameplay frame, then it
/// self-disables so the log is one frame, not the whole boot. Gated entirely
/// behind the env var; no cost when unset. Remove/keep behind the flag.
#[cfg(not(test))]
mod projdump {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INIT: AtomicBool = AtomicBool::new(false);
    static VTX_LOGGED: AtomicU64 = AtomicU64::new(0);
    // clip-w histogram counters for the frame:
    pub static W_TOTAL: AtomicU64 = AtomicU64::new(0);
    pub static W_ONSCREEN: AtomicU64 = AtomicU64::new(0);
    pub static W_PATHOLOGICAL: AtomicU64 = AtomicU64::new(0);
    // screen-space depth (pz) range tracker (stored as i32 bits of f32):
    pub static PZ_MIN: AtomicU64 = AtomicU64::new(u64::MAX);
    pub static PZ_MAX: AtomicU64 = AtomicU64::new(0);

    /// Record one screen-space depth `pz` into the frame's [min,max] tracker.
    pub fn note_pz(pz: f32) {
        if !on() || !pz.is_finite() {
            return;
        }
        // Offset f32 into a monotonic u64 key so min/max compares work.
        let key = (pz * 1000.0) as i64 + (1i64 << 40);
        let key = key.max(0) as u64;
        PZ_MIN.fetch_min(key, Ordering::Relaxed);
        PZ_MAX.fetch_max(key, Ordering::Relaxed);
    }

    pub fn on() -> bool {
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(crate::debug_flag("FN64_DUMP_PROJ"), Ordering::Relaxed);
        }
        ENABLED.load(Ordering::Relaxed)
    }
    /// Only log the first N vertices verbosely, but keep counting all.
    pub fn should_log_vtx() -> bool {
        on() && VTX_LOGGED.fetch_add(1, Ordering::Relaxed) < 24
    }
    /// Reset per-frame counters so a summary reflects ONE frame, not the
    /// cumulative boot. Called at the start of each F3DEX2 decode.
    pub fn reset_frame() {
        if !on() {
            return;
        }
        W_TOTAL.store(0, Ordering::Relaxed);
        W_ONSCREEN.store(0, Ordering::Relaxed);
        W_PATHOLOGICAL.store(0, Ordering::Relaxed);
        PZ_MIN.store(u64::MAX, Ordering::Relaxed);
        PZ_MAX.store(0, Ordering::Relaxed);
        VTX_LOGGED.store(0, Ordering::Relaxed);
    }
    pub fn note_w(w: f32, onscreen: bool) {
        if !on() {
            return;
        }
        W_TOTAL.fetch_add(1, Ordering::Relaxed);
        if onscreen {
            W_ONSCREEN.fetch_add(1, Ordering::Relaxed);
        }
        if !w.is_finite() || w.abs() > 1.0e5 {
            W_PATHOLOGICAL.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn summary() {
        if !on() {
            return;
        }
        let t = W_TOTAL.load(Ordering::Relaxed);
        let on = W_ONSCREEN.load(Ordering::Relaxed);
        let path = W_PATHOLOGICAL.load(Ordering::Relaxed);
        if t > 0 {
            let pzmin = (PZ_MIN.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
            let pzmax = (PZ_MAX.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
            eprintln!(
                "[FN64_DUMP_PROJ] SUMMARY: {t} projected vtx | on-screen NDC-cube: {on} ({:.1}%) | pathological |w|>1e5 or non-finite: {path} ({:.1}%) | screen-z(pz) range [{pzmin:.2}, {pzmax:.2}] (nearer=smaller, z-test is `z<depth`)",
                100.0 * on as f64 / t as f64,
                100.0 * path as f64 / t as f64
            );
        }
    }
}

fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (r, out_row) in out.iter_mut().enumerate() {
        for (c, out_cell) in out_row.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            *out_cell = s;
        }
    }
    out
}

/// Transform a homogeneous point (x,y,z,1) by `m` using the N64's ROW-VECTOR
/// convention: `clip = v_row · m`, i.e. `out[c] = sum_r v[r] * m[r][c]`.
///
/// The N64 RSP treats vertices as row vectors and matrices in hardware
/// `[row][col]` layout (`clip = v · M · V · P`); `read_mtx` stores each `Mtx`
/// element at `m[row][col]` with NO transpose, and `recompute_mvp` composes
/// `mvp = M · (V · P)` in that same layout. The homogeneous point must
/// therefore be applied on the LEFT as a row vector. Applying it on the RIGHT
/// as a column vector (`m · v`, the old code) computes `mvp^T · v` -- the
/// TRANSPOSE of the true transform. For the perspective MVP that put the
/// projective term (`m[2][3] = -1`) into the OUTPUT ROW instead of the w
/// column, so `w` became `m[3][0]·x + m[3][1]·y + m[3][2]·z` (a huge,
/// sign-flipping value ~±thousands for ob coords of only ±10) instead of the
/// depth `-z_eye`. That is the "giant triangles fanning from a point" bug --
/// vertices with |w|≈thousands and random sign perspective-divide to garbage.
/// Verified against a live OoT gameplay task's decoded P (persp row
/// `[0,0,-1.0016,-1]`) + modelview translation `[-53,-5,0,1]`: column-vector
/// gave `w=-1531.75`; row-vector gives `w=5.0` (= `-z_eye`).
///
/// For a symmetric/diagonal matrix (all the reference-fixture cases exercise)
/// `m == m^T`, so this is identical to the old column-vector product -- the
/// fixture goldens are unchanged. Only the real perspective·view·model
/// product (asymmetric) is affected, which is exactly the gameplay path.
fn transform_point(m: &Mat4, x: f32, y: f32, z: f32) -> [f32; 4] {
    let v = [x, y, z, 1.0];
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let mut s = 0.0;
        for r in 0..4 {
            s += v[r] * m[r][c];
        }
        out[c] = s;
    }
    out
}

/// Read an N64 fixed-point `Mtx` (64 bytes) at `addr` out of `rdram` and
/// convert to an f32 `Mat4`. The N64 `Mtx` layout (gbi.h `Mtx` union,
/// documented public format): the first 32 bytes hold each element's signed
/// integer part as a big-endian s16; the next 32 bytes hold each element's
/// fractional part as a big-endian u16. The real value is
/// `int_part + frac_part / 65536`. Elements are stored row-major
/// (`m[4][4]`). Returns `None` if the 64-byte read would run off `rdram`.
///
/// We store the element (r,c) at `m[r][c]` -- the SAME `[row][col]` layout
/// the hardware `Mtx` (and RT64's `FixedMatrix::toMatrix4x4`) uses, with NO
/// transpose. The N64's row-vector convention (`clip = v_row * M`) is then
/// reproduced by composing the model/view/projection product in hardware
/// order (`recompute_mvp`) and applying it to the vertex as a ROW vector in
/// `transform_point` (`clip = v_row · mvp`). Applying the composed matrix as
/// a COLUMN vector instead (`mvp · v`) computes `mvp^T · v` -- the TRANSPOSE
/// of the true transform -- which put the perspective term into the output
/// row instead of the w column and made `w` a huge sign-flipping value; see
/// `transform_point`'s doc for the cited P/M numbers.
fn read_mtx(rdram: &[u8], addr: usize) -> Option<Mat4> {
    if addr + 64 > rdram.len() {
        return None;
    }
    let mut m = [[0.0f32; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let elem = r * 4 + c;
            let int_off = addr + elem * 2;
            let frac_off = addr + 32 + elem * 2;
            // Swizzled halfword reads (recomp MEM_H): the Mtx was DMA'd from
            // ROM through the same `^3` per-byte swizzle as everything else.
            let int_part = read_i16(rdram, int_off) as i32;
            let frac_part = read_u16(rdram, frac_off) as i32;
            let value = (((int_part << 16) | frac_part) as f32) / 65536.0;
            // Natural row-major store (hardware [row][col]): NO transpose.
            *cell = value;
        }
    }
    Some(m)
}

/// Read an N64 `Vp` (viewport) struct (16 bytes) at `addr` out of `rdram`
/// and convert to a pixel-space [`Viewport`]. Layout (F3DEX2-CONCEPTS.md
/// §1.4/§3.5): 8 big-endian s16 -- `vscale[4]` (x, y, z, w) then
/// `vtrans[4]` (x, y, z, w), each in the N64 "quarter-pixel" encoding
/// (÷4 for pixel units). Reads through the recomp `^3`/`MEM_H` swizzle
/// like every other DMA'd struct. Returns `None` if the 16-byte read runs
/// off `rdram`.
fn read_viewport(rdram: &[u8], addr: usize) -> Option<Viewport> {
    if addr + 16 > rdram.len() {
        return None;
    }
    let vscale_x = read_i16(rdram, addr) as f32;
    let vscale_y = read_i16(rdram, addr + 2) as f32;
    let vscale_z = read_i16(rdram, addr + 4) as f32;
    // addr+6 = vscale.w (unused for screen mapping)
    let vtrans_x = read_i16(rdram, addr + 8) as f32;
    let vtrans_y = read_i16(rdram, addr + 10) as f32;
    let vtrans_z = read_i16(rdram, addr + 12) as f32;
    // addr+14 = vtrans.w (unused)
    let vp = Viewport {
        sx: vscale_x / 4.0,
        sy: vscale_y / 4.0,
        sz: vscale_z / 4.0,
        tx: vtrans_x / 4.0,
        ty: vtrans_y / 4.0,
        tz: vtrans_z / 4.0,
    };
    #[cfg(not(test))]
    if crate::debug_flag("FN64_DUMP_PROJ") {
        eprintln!(
            "[FN64_DUMP_PROJ] viewport: sz={} tz={} => screen-z range [{}, {}] (near->far)",
            vp.sz,
            vp.tz,
            -vp.sz + vp.tz,
            vp.sz + vp.tz
        );
    }
    Some(vp)
}

// --- Vertex lighting (F3DEX2-CONCEPTS.md §2.4) --------------------------

/// Read a `Light_t` (16 bytes, OoT gbi.h:1311 -- `col[3]` u8, pad, `colc[3]`
/// u8, pad, `dir[3]` s8, pad) out of `rdram` at `addr` and install it into
/// light `slot`. Directional slots keep both direction (unit, s8÷127) and
/// color; the ambient slot (`slot == num_dir`) has no meaningful direction,
/// so we ALSO copy its color into `ambient` -- the RSP treats the highest
/// light as pure ambient regardless of its `dir` bytes (gbi.h:2902). Reads
/// through the recomp `^3`/`MEM_B` swizzle like every other DMA'd struct.
fn load_light(rdram: &[u8], state: &mut DecodeState, addr: usize, slot: usize) {
    if slot >= MAX_LIGHTS {
        return;
    }
    // Guard the whole 16-byte Light_t read (recomp `^3` swizzle can touch
    // `addr + LIGHT_STRIDE - 1`); a truncated DMA leaves the slot untouched.
    if addr + LIGHT_STRIDE > rdram.len() {
        return;
    }
    // col[0..3] at bytes 0..3; dir[3] (s8) at bytes 8..11.
    let cr = read_u8(rdram, addr) as f32 / 255.0;
    let cg = read_u8(rdram, addr + 1) as f32 / 255.0;
    let cb = read_u8(rdram, addr + 2) as f32 / 255.0;
    // dir is signed s8 ÷127 -> a (roughly) unit direction (RSPProcessCS.hlsl
    // `srcNorm / 127`).
    let dx = (read_u8(rdram, addr + 8) as i8) as f32 / 127.0;
    let dy = (read_u8(rdram, addr + 9) as i8) as f32 / 127.0;
    let dz = (read_u8(rdram, addr + 10) as i8) as f32 / 127.0;
    state.lights.dir[slot] = DirLight {
        dir: [dx, dy, dz],
        col: [cr, cg, cb],
    };
    // If this slot is the ambient slot (the one just past the directional
    // count), mirror its color into `ambient`.
    if slot == state.lights.num_dir {
        state.lights.ambient = [cr, cg, cb];
    }
}

/// Decode the F3DEX2 light slot selected by a `G_MOVEMEM G_MV_LIGHT`
/// destination offset. `gSPLight(..., n)` emits `(n * 24 + 24) / 8` in the
/// wire field, while DMEM indices 0 and 1 are reserved for the two look-at
/// vectors. Therefore `LIGHT_1` starts at DMEM index 2 and maps to light slot
/// 0, matching RT64's `offset / 24 - 2` dispatch.
fn light_slot_from_movemem_offset(ofs_div8: usize) -> Option<usize> {
    #[cfg(not(test))]
    let reserved_slots = if std::env::var_os("FN64_DIAG_OLD_LIGHT_SLOT").is_some() {
        1
    } else {
        2
    };
    #[cfg(test)]
    let reserved_slots = 2;
    (ofs_div8 / 3)
        .checked_sub(reserved_slots)
        .filter(|&slot| slot < MAX_LIGHTS)
}

/// Normalize a 3-vector; returns the zero vector unchanged (guards a 0-length
/// normal/direction so a bad DMA can't produce NaN).
#[inline]
fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Rotate a direction (w=0) by the 3x3 upper-left of a `Mat4` (row-major,
/// column-vector convention like `transform_point`). Used to bring a light
/// direction from world/eye space into the vertex's local space, matching
/// RT64's `computeDirLight` (`mul(float4(dir,0), worldMat)`), which multiplies
/// by the modelview so N·L is evaluated in the same space as the (untransformed)
/// vertex normal.
#[inline]
fn rotate_dir(m: &Mat4, d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2];
    }
    out
}

/// Compute a lit vertex color from a NORMAL (`cn` reinterpreted as s8÷127),
/// the loaded lights, and the current modelview (light-space transform).
/// Ambient + Σ over directionals of `max(N·L, 0) * lightColor`, clamped to
/// [0,1] per channel, returned as u8 RGB. This mirrors RT64's
/// `RSPProcessCS.hlsl` lighting branch (ambient is the base, each directional
/// adds `computeDirLight`, result `min(.,1)`), the microcode-faithful model.
fn light_vertex(state: &DecodeState, normal: [f32; 3]) -> [u8; 3] {
    let n = normalize3(normal);
    let mut c = state.lights.ambient;
    for i in 0..state.lights.num_dir {
        let light = &state.lights.dir[i];
        // Bring the light direction into the vertex's (model) space via the
        // modelview, normalize, then N·L (clamped at 0 -- unlit back side
        // contributes nothing).
        let ld = normalize3(rotate_dir(&state.modelview, light.dir));
        let ndotl = (n[0] * ld[0] + n[1] * ld[1] + n[2] * ld[2]).max(0.0);
        c[0] += ndotl * light.col[0];
        c[1] += ndotl * light.col[1];
        c[2] += ndotl * light.col[2];
    }
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

// --- Texture format decode (F3DEX2-CONCEPTS.md §5.1) --------------------
//
// Format/size selector values: OoT `include/ultra64/gbi.h:331-378`.
// Texel bit layouts and channel expansion: RT64 (MIT)
// `src/shaders/Formats.hlsli:56-119` and
// `src/shaders/TextureDecoder.hlsli:30-120,149-204`.

/// RDP image formats (`G_IM_FMT_*`) as encoded in the SETTIMG/SETTILE
/// format field.
const G_IM_FMT_RGBA: u8 = 0;
const G_IM_FMT_CI: u8 = 2;
const G_IM_FMT_IA: u8 = 3;
const G_IM_FMT_I: u8 = 4;

/// Pixel sizes (`G_IM_SIZ_*`): 4/8/16/32 bits-per-texel selectors.
const G_IM_SIZ_4B: u8 = 0;
const G_IM_SIZ_8B: u8 = 1;
const G_IM_SIZ_16B: u8 = 2;
const G_IM_SIZ_32B: u8 = 3;

/// Expand a 16-bit RGBA5551 texel to RGBA8888 (5/5/5/1, big-endian).
/// RT64 `Formats.hlsli:83-92` gives the exact shifts and 5-to-8 replication;
/// OoT `gbi.h:334,345` identifies this as `G_IM_FMT_RGBA/G_IM_SIZ_16b`.
#[inline]
fn rgba5551_to_rgba8888(px: u16) -> [u8; 4] {
    let r5 = ((px >> 11) & 0x1F) as u8;
    let g5 = ((px >> 6) & 0x1F) as u8;
    let b5 = ((px >> 1) & 0x1F) as u8;
    let a1 = (px & 0x01) as u8;
    // 5-bit -> 8-bit: replicate high bits into the low bits (v<<3 | v>>2).
    let expand5 = |v: u8| (v << 3) | (v >> 2);
    [
        expand5(r5),
        expand5(g5),
        expand5(b5),
        if a1 != 0 { 255 } else { 0 },
    ]
}

/// Expand IA16 (8-bit intensity, 8-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:108-111` (`gbi.h:337,345`).
#[inline]
fn ia16_to_rgba8888(hi: u8, lo: u8) -> [u8; 4] {
    [hi, hi, hi, lo]
}

/// Expand IA8 (4-bit intensity, 4-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:75-80` (`gbi.h:337,344`).
#[inline]
fn ia8_to_rgba8888(byte: u8) -> [u8; 4] {
    let i4 = byte >> 4;
    let a4 = byte & 0x0F;
    let i = (i4 << 4) | i4;
    let a = (a4 << 4) | a4;
    [i, i, i, a]
}

/// Expand IA4 (3-bit intensity, 1-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:61-64` (`gbi.h:337,343`).
#[inline]
fn ia4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i3 = (nibble >> 1) & 0x07;
    // Exact 3-to-8 replication: abc -> abcabcab.
    let i = (i3 << 5) | (i3 << 2) | (i3 >> 1);
    [i, i, i, if nibble & 1 != 0 { 255 } else { 0 }]
}

/// Expand I8 (8-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:71-73` (`gbi.h:338,344`).
#[inline]
fn i8_to_rgba8888(byte: u8) -> [u8; 4] {
    [byte, byte, byte, byte]
}

/// Expand I4 (4-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:56-59` (`gbi.h:338,343`).
#[inline]
fn i4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i = (nibble << 4) | nibble;
    [i, i, i, i]
}

/// Select one 4-bit texel from a packed byte. RT64
/// `TextureDecoder.hlsli:170-172` selects the high nibble for even columns
/// and the low nibble for odd columns.
#[inline]
fn packed_nibble(byte: u8, texel_index: usize) -> u8 {
    if texel_index & 1 == 0 {
        byte >> 4
    } else {
        byte & 0x0F
    }
}

/// Decode `G_LOADTLUT`'s 10-bit count field. Public `gbi.h` packs
/// `count - 1` directly at bits 14..23; the low two bits are part of the
/// count, not fixed-point padding.
fn load_tlut_count(w1: u32) -> usize {
    let count = ((w1 >> 14) & 0x3ff) as usize + 1;
    assert!(
        count <= 256,
        "G_LOADTLUT requested {count} entries, exceeding the 256-entry TLUT"
    );
    count
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TextureLoad {
    Block,
    Tile { source_x: u32, source_y: u32 },
}

fn palette_color(tlut: &[[u8; 4]], index: usize, format: &str) -> [u8; 4] {
    *tlut.get(index).unwrap_or_else(|| {
        panic!(
            "{format} texel index {index} exceeds the loaded {}-entry TLUT",
            tlut.len()
        )
    })
}

/// Decode the texture bound to `tile` from the latched `G_SETTIMG` image out
/// of RDRAM into an RGBA8888 [`Texture`], sized by the tile's
/// `G_SETTILESIZE` extent. Returns `None` for an unsupported/zero-size
/// format so the caller leaves the triangle flat-shaded rather than binding
/// garbage. Covers the common OoT formats: RGBA16/32, RGBA4/8 hardware
/// aliases, IA16/IA8/IA4, I8/I4, and CI8/CI4 (via the loaded TLUT).
///
/// This is deliberately NOT a byte-exact 4 KiB TMEM model. `G_LOADBLOCK`
/// remains a linear decode, while `G_LOADTILE` addresses its rectangular
/// source through the `G_SETTIMG` row width and load origin. The sampler then
/// makes the copied tile local by subtracting its render-tile origin
/// (`F3DEX2-CONCEPTS.md` §5.1).
fn decode_current_texture(
    rdram: &[u8],
    tex: &TexState,
    segments: &[u32; 16],
    tile: usize,
    load: TextureLoad,
) -> Option<Texture> {
    let t = &tex.tiles[tile];
    // Tile extent from SETTILESIZE (S10.5 -> ÷4 texels), inclusive bounds.
    let w = ((t.lrs / 4).saturating_sub(t.uls / 4) + 1) as u32;
    let h = ((t.lrt / 4).saturating_sub(t.ult / 4) + 1) as u32;
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    let base = resolve_addr(segments, tex.timg_addr);
    let fmt = t.fmt;
    let siz = t.siz;
    let mut texels = vec![0u8; (w * h * 4) as usize];
    if matches!(load, TextureLoad::Tile { .. }) {
        assert_ne!(
            tex.timg_width, 0,
            "G_LOADTILE decoded before G_SETTIMG latched a source width"
        );
    }

    for ty in 0..h {
        for tx in 0..w {
            let texel_index = (ty * w + tx) as usize;
            let source_index = match load {
                TextureLoad::Block => texel_index,
                TextureLoad::Tile { source_x, source_y } => {
                    ((source_y + ty) * u32::from(tex.timg_width) + source_x + tx) as usize
                }
            };
            let rgba = match (fmt, siz) {
                (G_IM_FMT_RGBA, G_IM_SIZ_16B) => {
                    let px = read_u16(rdram, base + source_index * 2);
                    rgba5551_to_rgba8888(px)
                }
                (G_IM_FMT_RGBA, G_IM_SIZ_32B) => {
                    let o = base + source_index * 4;
                    [
                        read_u8(rdram, o),
                        read_u8(rdram, o + 1),
                        read_u8(rdram, o + 2),
                        read_u8(rdram, o + 3),
                    ]
                }
                (G_IM_FMT_IA, G_IM_SIZ_16B) => {
                    let o = base + source_index * 2;
                    ia16_to_rgba8888(read_u8(rdram, o), read_u8(rdram, o + 1))
                }
                (G_IM_FMT_IA, G_IM_SIZ_8B) => ia8_to_rgba8888(read_u8(rdram, base + source_index)),
                (G_IM_FMT_I, G_IM_SIZ_8B) | (G_IM_FMT_RGBA, G_IM_SIZ_8B) => {
                    // RGBA8 is not a nominal GBI format, but RT64's observed
                    // hardware path samples it identically to I8
                    // (`TextureDecoder.hlsli:68-75`).
                    i8_to_rgba8888(read_u8(rdram, base + source_index))
                }
                (G_IM_FMT_IA, G_IM_SIZ_4B) => {
                    let byte = read_u8(rdram, base + source_index / 2);
                    ia4_to_rgba8888(packed_nibble(byte, source_index))
                }
                (G_IM_FMT_I, G_IM_SIZ_4B) | (G_IM_FMT_RGBA, G_IM_SIZ_4B) => {
                    // RGBA4 likewise aliases I4 on hardware (RT64
                    // `TextureDecoder.hlsli:45-56`). OoT's real 250-swap
                    // C-boot trace exercises this otherwise-unsupported pair.
                    let byte = read_u8(rdram, base + source_index / 2);
                    i4_to_rgba8888(packed_nibble(byte, source_index))
                }
                (G_IM_FMT_CI, G_IM_SIZ_8B) => {
                    // RT64 `TextureDecoder.hlsli:174-184`: an 8-bit CI texel
                    // is the full TLUT index. OoT uses RGBA16 TLUTs only
                    // (`oot-decomp/docs/assets/images.md:63-64`).
                    let idx = read_u8(rdram, base + source_index) as usize;
                    palette_color(&tex.tlut, idx, "CI8")
                }
                (G_IM_FMT_CI, G_IM_SIZ_4B) => {
                    let byte = read_u8(rdram, base + source_index / 2);
                    // RT64 `TextureDecoder.hlsli:176-179`: CI4 prepends the
                    // tile's four-bit palette bank to the texel nibble in
                    // TMEM. A 16-entry G_LOADTLUT is stored by this decoder as
                    // a palette-local Vec (entry zero is that bank's first
                    // color), while a full TLUT remains globally indexed.
                    let nib = packed_nibble(byte, source_index) as usize;
                    let idx = if tex.tlut.len() <= 16 {
                        nib
                    } else {
                        ((t.palette as usize) << 4) | nib
                    };
                    palette_color(&tex.tlut, idx, "CI4")
                }
                _ => return None, // unsupported format: leave flat-shaded.
            };
            let o = texel_index * 4;
            texels[o..o + 4].copy_from_slice(&rgba);
        }
    }

    Some(Texture {
        width: w,
        height: h,
        texels: std::rc::Rc::new(texels),
        clamp_s: t.clamp_s,
        clamp_t: t.clamp_t,
        origin_s: t.uls as f32 / 4.0,
        origin_t: t.ult as f32 / 4.0,
    })
}

thread_local! {
    /// Per-opcode "already warned once" set, so a real display list with
    /// thousands of identical skipped state ops emits ONE loud line per
    /// distinct opcode rather than flooding the log. Thread-local (not a
    /// static Mutex) to stay lock-free and match the rest of this crate's
    /// single-threaded reference-backend model.
    static WARNED_SKIPS: RefCell<HashSet<u8>> = RefCell::new(HashSet::new());
    /// Unsupported combiner sub-sources are warned per distinct raw mode,
    /// rather than silently collapsing to the reference evaluator's current
    /// neutral approximation.
    static WARNED_COMBINERS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

fn warn_approximated_combiner_sources(mode: CombinerMode, w0: u32, w1: u32) {
    let color_approximated = |source: ColorSource| {
        matches!(
            source,
            ColorSource::Texel1
                | ColorSource::Texel1Alpha
                | ColorSource::KeyCenter
                | ColorSource::KeyScale
                | ColorSource::LodFraction
                | ColorSource::Noise
                | ColorSource::K4
                | ColorSource::K5
        )
    };
    let alpha_approximated =
        |source: AlphaSource| matches!(source, AlphaSource::Texel1 | AlphaSource::LodFraction);
    if !mode.cycles.iter().any(|cycle| {
        cycle.rgb.iter().copied().any(color_approximated)
            || cycle.alpha.iter().copied().any(alpha_approximated)
    }) {
        return;
    }

    let key = ((w0 as u64 & 0x00ff_ffff) << 32) | w1 as u64;
    WARNED_COMBINERS.with(|warned| {
        if warned.borrow_mut().insert(key) {
            eprintln!(
                "[fn64-render-rt64/gbi] G_SETCOMBINE mode {key:#014x} uses a source not yet \
                 modeled exactly (TEXEL1 aliases TEXEL0; key/noise/K/LOD inputs are zero). \
                 Common OoT modulate/decal/primitive/environment/shade sources remain exact."
            );
        }
    });
}

/// Log an acknowledged-but-unimplemented opcode ONCE per distinct opcode
/// byte, by name -- the task's "every unimplemented GBI opcode must be a
/// LOUD log/skip (named), never a silent no-op" requirement, without
/// flooding on repeats.
fn skip_opcode(opcode: u8) {
    WARNED_SKIPS.with(|w| {
        if w.borrow_mut().insert(opcode) {
            eprintln!(
                "[fn64-render-rt64/gbi] SKIP unimplemented opcode {} ({:#04x}) -- \
                 geometry will render flat-shaded from vertex color only (no texture/\
                 lighting/state applied for this op). This is logged once per distinct \
                 opcode; further occurrences are silent.",
                opcode_name(opcode),
                opcode
            );
        }
    });
}

/// Human-readable name for an opcode byte (for the loud skip log). Covers
/// the common F3DEX2 state ops OoT emits so the skip log names them instead
/// of just printing a hex byte.
fn opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x00 => "G_NOOP",
        G_VTX => "G_VTX",
        G_MODIFYVTX => "G_MODIFYVTX",
        G_CULLDL => "G_CULLDL",
        G_BRANCH_Z => "G_BRANCH_Z",
        G_TRI1 => "G_TRI1",
        G_TRI2 => "G_TRI2",
        G_QUAD => "G_QUAD",
        G_LINE3D => "G_LINE3D",
        G_TEXRECT => "G_TEXRECT",
        G_TEXRECTFLIP => "G_TEXRECTFLIP",
        G_POPMTX => "G_POPMTX",
        G_MTX => "G_MTX",
        G_MOVEWORD => "G_MOVEWORD",
        G_DL => "G_DL",
        G_ENDDL => "G_ENDDL",
        0xE0 => "G_SPNOOP",
        0xE1 => "G_RDPHALF_1",
        G_SETOTHERMODE_L => "G_SETOTHERMODE_L",
        G_SETOTHERMODE_H => "G_SETOTHERMODE_H",
        0xE6 => "G_RDPLOADSYNC",
        0xE7 => "G_RDPPIPESYNC",
        0xE8 => "G_RDPTILESYNC",
        0xE9 => "G_RDPFULLSYNC",
        G_RDPSETOTHERMODE => "G_RDPSETOTHERMODE",
        G_LOADTLUT => "G_LOADTLUT",
        0xF1 => "G_RDPHALF_2",
        G_LOADBLOCK => "G_LOADBLOCK",
        G_LOADTILE => "G_LOADTILE",
        G_SETTILESIZE => "G_SETTILESIZE",
        G_SETTILE => "G_SETTILE",
        G_SETFOGCOLOR => "G_SETFOGCOLOR",
        G_SETBLENDCOLOR => "G_SETBLENDCOLOR",
        G_SETCOMBINE => "G_SETCOMBINE",
        G_SETTIMG => "G_SETTIMG",
        G_SETPRIMCOLOR => "G_SETPRIMCOLOR",
        G_SETENVCOLOR => "G_SETENVCOLOR",
        G_SETSCISSOR => "G_SETSCISSOR",
        G_SETZIMG => "G_SETZIMG",
        G_SETCIMG => "G_SETCIMG",
        G_SPECIAL_1 => "G_SPECIAL_1",
        G_DMA_IO => "G_DMA_IO",
        G_LOAD_UCODE => "G_LOAD_UCODE",
        G_TEXTURE => "G_TEXTURE",
        G_GEOMETRYMODE => "G_GEOMETRYMODE",
        G_MOVEMEM => "G_MOVEMEM",
        _ => "G_<unrecognized>",
    }
}

/// Reset the once-per-opcode skip-warning memo. Tests and interactive
/// diagnostics may request a fresh coverage report explicitly; normal frame
/// decoding keeps the memo so repeated frames do not repeat identical I/O.
pub fn reset_skip_warnings() {
    WARNED_SKIPS.with(|w| w.borrow_mut().clear());
}

// --- Recomp rdram memory model (swizzled) -------------------------------
//
// fn64's `rdram` is NOT a flat big-endian image. The N64Recomp memory
// macros (`refs/N64RecompSource/include/recomp.h:95-107`) store every
// aligned 32-bit word in HOST-NATIVE order (`MEM_W` = a bare
// `*(int32_t*)`, no byteswap) and reach sub-word bytes/halfwords through an
// address XOR (`MEM_B` uses `^3`, `MEM_H` uses `^2`) -- the standard
// "byteswap within a native word" trick that makes big-endian sub-word
// access work over a little-endian word array. The PI-DMA path
// (`fn64-runtime/src/rdram.rs:243` `dma_write_bytes`) writes cartridge
// bytes with the SAME per-byte `^3` swizzle, so EVERYTHING in rdram --
// CPU-built display lists AND DMA'd vertex/matrix data -- obeys this one
// model. A decoder that reads it as flat big-endian (the old
// `from_be_bytes`) gets each 32-bit word byte-reversed: OoT's first DL
// command `0xDE...` (G_DL) read flat-BE became `0x000001DE` (opcode
// `0x00`), so the whole list decoded as garbage and produced 0 triangles.
//
// These helpers read logical values THE WAY THE GAME DOES: an aligned word
// is a native-endian `u32` (== the logical big-endian word), and any
// byte/halfword within it is extracted by its logical position. This is
// exactly equivalent to `MEM_W` / `MEM_HU(^2)` / `MEM_BU(^3)`.

/// Read the logical big-endian 32-bit word at aligned byte `off`
/// (`off % 4 == 0` expected; misaligned reads still return the containing
/// word's native value, matching a `MEM_W` on a masked address). Returns 0
/// if the word runs past `rdram`.
#[inline]
fn read_u32(rdram: &[u8], off: usize) -> u32 {
    let Some(aligned) = complete_storage_word(rdram, off) else {
        return 0;
    };
    fn64_runtime::RdramView::from_storage(rdram).read_u32(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(aligned).expect("GBI RDRAM address exceeds u32"),
    ))
}

#[inline]
fn complete_storage_word(rdram: &[u8], off: usize) -> Option<usize> {
    let aligned = off & !3;
    aligned
        .checked_add(4)
        .filter(|&end| end <= rdram.len())
        .map(|_| aligned)
}

/// Read a logical byte at byte offset `off` (recomp `MEM_BU`: physical
/// index `off ^ 3`). Returns 0 past the end.
#[inline]
fn read_u8(rdram: &[u8], off: usize) -> u8 {
    if complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u8(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical signed 16-bit halfword at byte offset `off` (recomp
/// `MEM_H`). The two logical bytes `off` (MSB) and `off+1` (LSB) are read
/// through the `^3` byte swizzle and recombined big-endian. Returns 0 past
/// the end.
#[inline]
fn read_i16(rdram: &[u8], off: usize) -> i16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_i16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical unsigned 16-bit halfword at byte offset `off`.
#[inline]
fn read_u16(rdram: &[u8], off: usize) -> u16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Resolve a (possibly segmented) F3DEX2 address to a flat rdram byte
/// offset. The top byte is the segment number; the low 24 bits are the
/// offset within that segment. If a segment base was registered (via
/// `G_MOVEWORD`/`G_MW_SEGMENT`) it is added; segment 0 is the identity
/// (physical) segment on real hardware, so an unset segment resolves to its
/// low-24-bit offset unchanged -- which is also exactly what the pre-
/// existing non-segmented fixtures (segment byte 0x00, e.g. addr 0x1000)
/// rely on, keeping them working unchanged.
fn resolve_addr(segments: &[u32; 16], addr: u32) -> usize {
    let seg = ((addr >> 24) & 0x0F) as usize;
    let off = (addr & 0x00FF_FFFF) as usize;
    segments[seg] as usize + off
}

/// Decoder state carried across (possibly nested via `G_DL`) command
/// streams.
struct DecodeState {
    vtx_cache: [Vertex; 32],
    tris: Vec<Triangle>,
    segments: [u32; 16],
    /// Projection * modelview, recomputed whenever either changes. `None`
    /// means "no transform loaded yet" -> vertices pass through as raw `ob`
    /// screen coords (preserves the pre-existing raw-coordinate fixtures).
    mvp: Option<Mat4>,
    proj: Option<Mat4>,
    modelview: Mat4,
    mv_stack: Vec<Mat4>,
    /// Viewport scale/translate (screen mapping), if a `G_MOVEMEM` viewport
    /// was seen. Fields: `(sx, sy, sz, tx, ty, tz)` -- x/y map NDC to pixels,
    /// z maps NDC-z to the depth range (all already divided by 4 in
    /// `read_viewport`). `None` -> NDC is mapped with a default 320x240
    /// half-extent only when a projection IS active; with no projection at
    /// all the raw `ob` coords are used directly.
    viewport: Option<Viewport>,
    scissor: Option<ScissorRect>,
    /// Current F3DEX2 geometry mode (the `G_GEOMETRYMODE` accumulator). Its
    /// `G_CULL_FRONT`/`G_CULL_BACK` bits decide per-triangle culling.
    geometry_mode: u32,
    /// RDP other-mode H/L plus blend-alpha threshold. F3DEX2 partial updates
    /// mutate this shared state; each emitted triangle snapshots it.
    other_mode: OtherMode,
    /// RDP color-combiner + primitive/environment register state. This is
    /// independent of other-mode/render state, but snapshotted beside it.
    combiner: CombinerState,
    /// Constant blender inputs. `blend_color.a` is mirrored into `other_mode`
    /// for alpha compare; the full RGBA values feed the framebuffer blender.
    blend_color: [u8; 4],
    fog_color: [u8; 4],
    dl_depth: u32,
    /// Total commands decoded this frame (all streams), checked against
    /// [`MAX_DL_COMMANDS`] so a cyclic branch list terminates.
    cmds_decoded: u32,
    /// Texture-mapping decode state (SETTIMG image latch, tile descriptors,
    /// TLUT palette, G_TEXTURE enable/scale, and the currently-decoded
    /// texture bound to emitted triangles). See [`TexState`].
    tex: TexState,
    /// Vertex-lighting decode state (`G_MV_LIGHT` diffuse/ambient structs +
    /// `G_MW_NUMLIGHT` count). Applied at `G_VTX` time when the geometry
    /// mode's `G_LIGHTING` bit is set. See [`LightState`].
    lights: LightState,
}

/// F3DEX2 vertex-lighting decode state (`F3DEX2-CONCEPTS.md` §2.4). The
/// RSP holds up to 7 directional lights plus one ambient; `num_dir` selects
/// how many directional slots are active, and the ambient light is the slot
/// at index `num_dir`. Directions are stored NORMALIZED in eye/model space
/// (s8 ÷127); the light-space transform uses the current modelview.
#[derive(Clone, Debug)]
struct LightState {
    /// Diffuse light slots (`G_MV_LIGHT`): direction (unit, s8÷127) + RGB
    /// color (0..1). Slot `num_dir` doubles as the ambient's color carrier
    /// when written, but ambient is read via `ambient` below.
    dir: [DirLight; MAX_LIGHTS],
    /// Ambient light color (0..1) -- the highest-numbered light slot.
    ambient: [f32; 3],
    /// Number of active directional lights (`G_MW_NUMLIGHT` / 24).
    num_dir: usize,
}

impl Default for LightState {
    fn default() -> Self {
        LightState {
            dir: [DirLight::default(); MAX_LIGHTS],
            // A conservative default: no ambient, no directionals, so a DL
            // that enables G_LIGHTING but (somehow) loaded no lights renders
            // dark rather than garbage -- but real OoT always loads both.
            ambient: [0.0, 0.0, 0.0],
            num_dir: 0,
        }
    }
}

/// One decoded directional light: a unit direction (light-space, s8÷127) and
/// an RGB diffuse color (0..1).
#[derive(Copy, Clone, Debug, Default)]
struct DirLight {
    dir: [f32; 3],
    col: [f32; 3],
}

/// Texture-pipeline decode state (`F3DEX2-CONCEPTS.md` §5). Kept as a
/// sub-struct so the transform/geometry state above stays readable.
#[derive(Clone, Debug, Default)]
struct TexState {
    /// `G_SETTIMG`: the source texture image -- segmented addr + format +
    /// size-code. Latched; no data moves until a `G_LOAD*`.
    timg_addr: u32,
    timg_fmt: u8,
    timg_siz: u8,
    timg_width: u16,
    /// The 8 RDP tile descriptors (`G_SETTILE`/`G_SETTILESIZE`).
    tiles: [Tile; 8],
    /// `G_LOADTLUT` palette: up to 256 RGBA8888 entries decoded from the
    /// TLUT image (CI textures index into this).
    tlut: Vec<[u8; 4]>,
    /// `G_TEXTURE`: texturing enabled?
    tex_enabled: bool,
    /// `G_TEXTURE`: which tile descriptor is active (0-7).
    tex_tile: u8,
    /// `G_TEXTURE` S/T scale (U0.16 -> f32), applied to the raw vertex S/T
    /// before texel addressing.
    tex_scale_s: f32,
    tex_scale_t: f32,
    /// The most-recently-decoded texture for the active tile, bound to
    /// emitted triangles while texturing is on. Rebuilt on each `G_LOAD*`.
    current: Option<Texture>,
}

/// One RDP tile descriptor (`G_SETTILE` + `G_SETTILESIZE`,
/// `F3DEX2-CONCEPTS.md` §5.1) -- only the fields the reference sampler needs.
#[derive(Copy, Clone, Debug, Default)]
struct Tile {
    fmt: u8,
    siz: u8,
    /// Line stride in 64-bit words (`G_SETTILE` `line`).
    line: u16,
    /// TLUT palette bank (CI4 uses this as the high nibble of the index).
    palette: u8,
    clamp_s: bool,
    clamp_t: bool,
    /// Tile active extent from `G_SETTILESIZE` (S10.5 -> ÷4 texels).
    uls: u16,
    ult: u16,
    lrs: u16,
    lrt: u16,
}

/// Parsed viewport: screen scale/translate in pixels (x, y) plus a depth
/// scale/translate (z), all already ÷4 from the N64 quarter-pixel encoding
/// (`F3DEX2-CONCEPTS.md` §3.5).
#[derive(Copy, Clone, Debug)]
struct Viewport {
    sx: f32,
    sy: f32,
    sz: f32,
    tx: f32,
    ty: f32,
    tz: f32,
}

/// Max `G_DL` *call* (G_DL_PUSH) recursion depth honored, matching the real
/// F3DEX2 display-list return stack (18 entries; the older 10-entry figure
/// is F3D/F3DEX). Only pushes count -- a gsSPBranchList tail-jump replaces
/// the DL pointer and consumes NO stack entry on hardware, so branch chains
/// (which OoT uses liberally) must not count against this.
const MAX_DL_DEPTH: u32 = 18;

/// Whole-decode command budget: bounds a cyclic/corrupt DL (e.g. a branch
/// list that branches to itself), which the hardware would spin on forever.
/// A real OoT frame decodes on the order of 10^4 commands; 2^20 is far above
/// any legitimate frame while still terminating promptly on a cycle.
const MAX_DL_COMMANDS: u32 = 1 << 20;

/// The simple ("reference-fixture") F3D-style decoder retained for backward
/// compatibility: `G_VTX`/`G_TRI1`/`G_TRI2`/`G_ENDDL` with raw screen-space
/// `ob` coords, non-segmented addresses in `w1`, and the pre-existing
/// vertex/index packing (`n<<12 | v0`; indices `(v0<<16)|(v1<<8)|v2` as
/// plain cache slots). This is what the original hand-built fixtures and the
/// `fn64-abi` executor-seam test plant, so it MUST stay bit-compatible with
/// them. Real OoT display lists use [`decode_display_list_f3dex2`] instead.
pub fn decode_display_list(rdram: &[u8], dl_addr: u32) -> Result<Vec<Triangle>, RenderError> {
    let mut vtx_cache = [Vertex::default(); 32];
    let mut tris = Vec::new();
    let mut pc = dl_addr as usize;

    loop {
        if pc + 8 > rdram.len() {
            break;
        }
        let w0 = u32::from_be_bytes(rdram[pc..pc + 4].try_into().unwrap());
        let w1 = u32::from_be_bytes(rdram[pc + 4..pc + 8].try_into().unwrap());
        let opcode = (w0 >> 24) as u8;
        pc += 8;

        match opcode {
            G_VTX => {
                // Original packing: w0 low 20 bits = n<<12 | v0; w1 = vtx
                // array address (non-segmented). Raw ob x/y are screen
                // coords -- no transform.
                let n = ((w0 >> 12) & 0xFF) as usize;
                let v0 = (w0 & 0xFF) as usize;
                let addr = w1 as usize;
                for i in 0..n {
                    let off = addr + i * VTX_STRIDE;
                    if off + VTX_STRIDE > rdram.len() || v0 + i >= vtx_cache.len() {
                        break;
                    }
                    let x = i16::from_be_bytes([rdram[off], rdram[off + 1]]) as f32;
                    let y = i16::from_be_bytes([rdram[off + 2], rdram[off + 3]]) as f32;
                    let cn = &rdram[off + 12..off + 16];
                    vtx_cache[v0 + i] = Vertex {
                        x,
                        y,
                        z: 0.0, // simple reference path: coplanar, no depth
                        r: cn[0],
                        g: cn[1],
                        b: cn[2],
                        a: cn[3],
                        s: 0.0, // simple reference path: untextured
                        t: 0.0,
                        w: 1.0, // simple path: everything in front of camera
                    };
                }
            }
            G_TRI1 => {
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                let idx_a = [(w0 >> 16) & 0xFF, (w0 >> 8) & 0xFF, w0 & 0xFF];
                let idx_b = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_a,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_b,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_ENDDL => break,
            _ => {} // simple decoder: silently skip (its opcode set is fixed).
        }
    }
    Ok(tris)
}

/// Decode and rasterize-prep a real F3DEX2 display list rooted at `dl_addr`
/// (a raw or segmented address; see `resolve_addr`) out of `rdram`. Returns
/// the flat-shaded triangles found in screen space, applying the matrix
/// stack + segment table + viewport as the DL commands set them. Any read
/// that would run off the end of `rdram` stops that command stream and
/// returns what was decoded so far, rather than panicking -- a malformed or
/// truncated fixture is a soft failure (fewer triangles), not a crash.
pub fn decode_display_list_f3dex2(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<Vec<Triangle>, RenderError> {
    let mut state = DecodeState {
        vtx_cache: [Vertex::default(); 32],
        tris: Vec::new(),
        segments: [0u32; 16],
        mvp: None,
        proj: None,
        modelview: identity(),
        mv_stack: Vec::new(),
        viewport: None,
        scissor: None,
        geometry_mode: 0,
        other_mode: OtherMode::default(),
        combiner: CombinerState::default(),
        blend_color: [0; 4],
        fog_color: [0; 4],
        dl_depth: 0,
        cmds_decoded: 0,
        tex: TexState::default(),
        lights: LightState::default(),
    };
    #[cfg(not(test))]
    projdump::reset_frame();
    decode_stream(rdram, dl_addr, &mut state);
    #[cfg(not(test))]
    projdump::summary();
    Ok(state.tris)
}

/// Produce a lossless command-word walk of an F3DEX2 display-list graph for
/// differential diagnostics. This follows the same public `G_DL` call versus
/// branch rules and `G_MOVEWORD/G_MW_SEGMENT` address updates as the decoder,
/// but does not interpret rendering state. Pointer-bearing commands include a
/// bounded content fingerprint at their resolved RDRAM target, so a trace can
/// distinguish a valid submitted graph from a dangling/empty DMA range without
/// copying game data into this repository. The caller owns where the returned
/// text is written; the RT64 task-dump hook writes only to an explicitly
/// requested untracked diagnostic directory.
pub(crate) fn trace_display_list_f3dex2(rdram: &[u8], dl_addr: u32) -> String {
    struct TraceState {
        segments: [u32; 16],
        commands: u32,
        opcodes: BTreeMap<u8, u32>,
        text: String,
    }

    fn fingerprint(rdram: &[u8], start: usize, requested_len: usize) -> String {
        if start >= rdram.len() {
            return format!("target={start:#08x} OUT_OF_BOUNDS");
        }
        let end = start.saturating_add(requested_len).min(rdram.len());
        let bytes = &rdram[start..end];
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let nonzero = bytes.iter().filter(|&&byte| byte != 0).count();
        format!(
            "target={start:#08x} bytes={} nonzero={} fnv1a64={hash:016x}",
            bytes.len(),
            nonzero
        )
    }

    fn trace_stream(rdram: &[u8], dl_addr: u32, depth: u32, state: &mut TraceState) {
        let mut pc = resolve_addr(&state.segments, dl_addr);
        writeln!(
            state.text,
            "ENTER depth={depth} segmented={dl_addr:#010x} resolved={pc:#08x}"
        )
        .expect("writing a display-list trace to String cannot fail");

        loop {
            if pc + 8 > rdram.len() {
                writeln!(state.text, "STOP depth={depth} pc={pc:#08x} OUT_OF_BOUNDS")
                    .expect("writing a display-list trace to String cannot fail");
                break;
            }
            state.commands += 1;
            if state.commands > MAX_DL_COMMANDS {
                writeln!(
                    state.text,
                    "STOP depth={depth} command_budget={MAX_DL_COMMANDS} exceeded"
                )
                .expect("writing a display-list trace to String cannot fail");
                break;
            }

            let command_pc = pc;
            let w0 = read_u32(rdram, pc);
            let w1 = read_u32(rdram, pc + 4);
            let opcode = (w0 >> 24) as u8;
            pc += 8;
            *state.opcodes.entry(opcode).or_default() += 1;

            let reference = match opcode {
                G_VTX => {
                    let n = ((w0 >> 12) & 0xFF) as usize;
                    Some(fingerprint(
                        rdram,
                        resolve_addr(&state.segments, w1),
                        n.saturating_mul(16),
                    ))
                }
                G_MTX => Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64)),
                G_MOVEMEM | G_SETTIMG | G_DL => {
                    Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64))
                }
                _ => None,
            };
            writeln!(
                state.text,
                "CMD depth={depth} pc={command_pc:#08x} op={opcode:#04x} w0={w0:#010x} \
                 w1={w1:#010x}{}",
                reference
                    .as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default(),
            )
            .expect("writing a display-list trace to String cannot fail");

            match opcode {
                G_MOVEWORD => {
                    let index = ((w0 >> 16) & 0xFF) as u16;
                    let offset = (w0 & 0xFFFF) as u16;
                    if index == G_MW_SEGMENT {
                        let segment = (offset / 4) as usize;
                        if segment < state.segments.len() {
                            state.segments[segment] = w1 & 0x00FF_FFFF;
                            writeln!(
                                state.text,
                                "SEG depth={depth} segment={segment} base={:#08x}",
                                state.segments[segment]
                            )
                            .expect("writing a display-list trace to String cannot fail");
                        }
                    }
                }
                G_DL => {
                    let is_branch = ((w0 >> 16) & 1) != 0;
                    if is_branch {
                        pc = resolve_addr(&state.segments, w1);
                        writeln!(state.text, "BRANCH depth={depth} target={pc:#08x}")
                            .expect("writing a display-list trace to String cannot fail");
                    } else if depth < MAX_DL_DEPTH {
                        trace_stream(rdram, w1, depth + 1, state);
                    } else {
                        writeln!(
                            state.text,
                            "STOP depth={depth} call_depth={MAX_DL_DEPTH} exceeded"
                        )
                        .expect("writing a display-list trace to String cannot fail");
                    }
                }
                G_ENDDL => {
                    writeln!(state.text, "RETURN depth={depth}")
                        .expect("writing a display-list trace to String cannot fail");
                    break;
                }
                _ => {}
            }
        }
    }

    let mut state = TraceState {
        segments: [0; 16],
        commands: 0,
        opcodes: BTreeMap::new(),
        text: String::new(),
    };
    trace_stream(rdram, dl_addr, 0, &mut state);
    writeln!(state.text, "SUMMARY commands={}", state.commands)
        .expect("writing a display-list trace to String cannot fail");
    for (opcode, count) in state.opcodes {
        writeln!(state.text, "OPCODE op={opcode:#04x} count={count}")
            .expect("writing a display-list trace to String cannot fail");
    }
    state.text
}

fn decode_stream(rdram: &[u8], dl_addr: u32, state: &mut DecodeState) {
    let mut pc = resolve_addr(&state.segments, dl_addr);

    loop {
        if pc + 8 > rdram.len() {
            break; // truncated command stream: stop, return what we have.
        }
        state.cmds_decoded += 1;
        if state.cmds_decoded > MAX_DL_COMMANDS {
            if state.cmds_decoded == MAX_DL_COMMANDS + 1 {
                eprintln!(
                    "[fn64-render-rt64/gbi] decode exceeded MAX_DL_COMMANDS \
                     ({MAX_DL_COMMANDS}) -- stopping (cyclic or corrupt \
                     display list)."
                );
            }
            break;
        }
        // Recomp rdram is word-native (see read_u32): each command word is a
        // logical big-endian u32 stored host-native, NOT a flat big-endian
        // byte run.
        let w0 = read_u32(rdram, pc);
        let w1 = read_u32(rdram, pc + 4);
        let opcode = (w0 >> 24) as u8;
        pc += 8;

        match opcode {
            G_VTX => {
                // F3DEX2 G_VTX (F3DEX2-CONCEPTS.md §2.1): the RSP-side wire
                // layout is n = field(w0,12,8), end-index = field(w0,1,7),
                // and the destination start slot v0 = end - n. w1 = segmented
                // vertex-array address. (NOT the F3DEX/SDK-macro `/2` form,
                // which misplaces vertices -- failure risk #2.)
                let n = ((w0 >> 12) & 0xFF) as usize;
                let end = ((w0 >> 1) & 0x7F) as usize;
                let v0 = end.saturating_sub(n);
                load_vertices(rdram, state, w1, n, v0);
            }
            G_TRI1 => {
                // F3DEX2 G_TRI1 (F3DEX2-CONCEPTS.md §2.2): three 7-bit
                // vertex-cache-slot fields in w0 at bits 17/9/1 -- each is
                // already the slot (0-31), no /2 needed.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex);
                let blender = active_blender(state);
                let idx = tri_indices(w0);
                if let Some(mut t) = resolve_tri(
                    &state.vtx_cache,
                    idx,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.tris.push(t);
                }
            }
            G_TRI2 | G_QUAD => {
                // F3DEX2 G_TRI2 / G_QUAD (§2.3): triangle A's three 7-bit
                // slot fields in w0 (bits 17/9/1), triangle B's in w1 at the
                // SAME bit positions. G_QUAD decodes identically to G_TRI2.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex);
                let blender = active_blender(state);
                let idx_a = tri_indices(w0);
                let idx_b = tri_indices(w1);
                if let Some(mut t) = resolve_tri(
                    &state.vtx_cache,
                    idx_a,
                    cull,
                    texture.clone(),
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.tris.push(t);
                }
                if let Some(mut t) = resolve_tri(
                    &state.vtx_cache,
                    idx_b,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.tris.push(t);
                }
            }
            G_MTX => {
                // F3DEX2 gsSPMatrix (gbi.h ~2106): w0 = op<<24 |
                // ((len-1)/8)<<19 | (ofs/8)<<8 | idx; the low byte on the
                // wire is `idx = params ^ G_MTX_PUSH`. F3DEX_GBI_2 param bits
                // (gbi.h:233-239): PROJECTION=0x04, LOAD=0x02, PUSH=0x01.
                // Un-XOR the push bit to recover the caller's params. w1 =
                // segmented matrix address.
                let wire_idx = (w0 & 0xFF) as u8;
                let params = wire_idx ^ 0x01; // ^ G_MTX_PUSH
                let is_projection = params & 0x04 != 0; // G_MTX_PROJECTION
                let is_load = params & 0x02 != 0; // G_MTX_LOAD
                let is_push = params & 0x01 != 0; // G_MTX_PUSH
                let addr = resolve_addr(&state.segments, w1);
                if let Some(mtx) = read_mtx(rdram, addr) {
                    #[cfg(not(test))]
                    if projdump::on() {
                        eprintln!(
                            "[FN64_DUMP_PROJ] G_MTX proj={} load={} push={} @rdram=0x{addr:06x} seg_w1=0x{w1:08x} mv_depth={} rows=[{:?} | {:?} | {:?} | {:?}]",
                            is_projection, is_load, is_push, state.mv_stack.len(),
                            mtx[0], mtx[1], mtx[2], mtx[3]
                        );
                    }
                    if is_projection {
                        // The projection matrix ALSO honors LOAD vs MUL. OoT
                        // loads the perspective matrix once with LOAD, then
                        // concatenates the camera/view matrix onto it with
                        // PROJECTION|MUL (guLookAt output). Treating every
                        // projection G_MTX as a LOAD (a prior bug) let the
                        // view matrix -- whose 4th row is [0,0,0,1], no
                        // projective term -- OVERWRITE the real perspective
                        // matrix (4th row [0,0,-1,0]).
                        //
                        // MUL ORDER (hardware/RT64): the incoming matrix
                        // multiplies on the LEFT of the accumulated
                        // projection -- `viewProj = new * viewProj` (RT64
                        // rt64_rsp.cpp:171). So the perspective LOAD gives
                        // `proj = P`, then the view MUL gives `proj = V * P`,
                        // and the final MVP below is `M * (V * P)`. This is
                        // the row-vector hardware product built column-major
                        // for our column-vector `transform_point`.
                        state.proj = Some(if is_load {
                            mtx
                        } else {
                            match state.proj {
                                Some(p) => mat_mul(&mtx, &p),
                                None => mtx,
                            }
                        });
                    } else {
                        // Modelview: a PUSH saves the current top so a later
                        // G_POPMTX restores it. LOAD replaces, MUL
                        // concatenates. MUL puts the incoming matrix on the
                        // LEFT (`modelview = new * modelview`, RT64
                        // rt64_rsp.cpp:197) so successive object transforms
                        // compose in the same order the hardware applies them.
                        if is_push {
                            state.mv_stack.push(state.modelview);
                        }
                        if is_load {
                            state.modelview = mtx;
                        } else {
                            state.modelview = mat_mul(&mtx, &state.modelview);
                        }
                    }
                    recompute_mvp(state);
                }
            }
            G_POPMTX => {
                // F3DEX2 gsSPPopMatrix: pop the modelview stack (params in
                // w1 select which stack; only the modelview stack is
                // modeled here). Restore the previous modelview if any.
                #[cfg(not(test))]
                if projdump::on() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] G_POPMTX mv_depth_before={}",
                        state.mv_stack.len()
                    );
                }
                if let Some(prev) = state.mv_stack.pop() {
                    state.modelview = prev;
                    recompute_mvp(state);
                }
            }
            G_MOVEWORD => {
                // F3DEX2 gsMoveWd (gbi.h ~2267): w0 = op<<24 | index<<16 |
                // offset<<0 (16-bit offset); w1 = data. Segment table write
                // is index==G_MW_SEGMENT, segment number = offset/4, base =
                // w1 (masked to a physical rdram offset).
                let index = ((w0 >> 16) & 0xFF) as u16;
                let offset = (w0 & 0xFFFF) as u16;
                if index == G_MW_SEGMENT {
                    let seg = (offset / 4) as usize;
                    if seg < state.segments.len() {
                        // Base is a physical rdram address; strip any KSEG
                        // high bits, keep the low 24 (segments span rdram).
                        state.segments[seg] = w1 & 0x00FF_FFFF;
                    }
                } else if index == G_MW_NUMLIGHT {
                    // F3DEX2 gsSPNumLights (gbi.h:2887): data = NUML(n) =
                    // n*24, so the directional-light count is w1/24. The
                    // ambient light lives in slot `num_dir` (gbi.h:2902:
                    // "the highest numbered light is always the ambient").
                    let n = (w1 / 24) as usize;
                    state.lights.num_dir = n.min(MAX_LIGHTS - 1);
                } else {
                    skip_opcode(G_MOVEWORD);
                }
            }
            G_DL => {
                // F3DEX2 gsSPDisplayList / gsSPBranchList (gbi.h ~2174-2178):
                // both pack via gDma1p(G_DL, dl, 0, p) so w0 = op<<24 |
                // p<<16, w1 = segmented address of the target DL. The `p`
                // byte at bits 16-23 is the push flag: G_DL_PUSH=0 (gbi.h:966)
                // is a CALL (push a return address, resume the caller after
                // the callee's G_ENDDL); G_DL_NOPUSH=1 (gbi.h:967) is a
                // BRANCH/tail-jump (gsSPBranchList) that REPLACES the current
                // DL pointer -- the target runs in place of the rest of this
                // stream and there is NO return to the bytes after the branch.
                //
                // BUG FIXED HERE: previously both cases recursed and then
                // *continued* decoding the current stream after return. For a
                // BRANCH that is wrong -- the words after a gsSPBranchList are
                // not commands (typically zero-fill or the next unrelated
                // buffer), so the decoder walked straight into garbage and
                // every trailing byte became a bogus "unrecognized opcode",
                // cascading the whole frame into ~14K junk skips (proven from
                // a live OoT gameplay task: the root DL's first command is a
                // gsSPBranchList `w0=0xde01_0000` whose trailing bytes are all
                // zero). We now recurse into the target and then STOP the
                // current stream for a branch (mirroring RT64's runDl, which
                // only pushes a return address when the push bit is clear).
                let is_branch = ((w0 >> 16) & 0x01) != 0; // G_DL_NOPUSH
                if is_branch {
                    // Tail branch: the target REPLACES the current DL
                    // pointer -- on hardware this consumes NO return-stack
                    // entry, so it must not recurse or count against
                    // MAX_DL_DEPTH (OoT chains branch lists deeper than any
                    // fixed cap; the old recursing version falsely tripped
                    // it). A self-referencing branch cycle is bounded by
                    // MAX_DL_COMMANDS at the loop top.
                    pc = resolve_addr(&state.segments, w1);
                    continue;
                }
                if state.dl_depth < MAX_DL_DEPTH {
                    // NOTE: G_DL is a pure address call/return -- it does NOT
                    // save or restore the matrix stack. The RSP's modelview/
                    // projection state is GLOBAL across a nested DL; only
                    // G_MTX (with G_MTX_PUSH) and G_POPMTX push/pop matrices.
                    // A previous version wrapped the recursion in a
                    // modelview push/pop, which corrupted transforms after a
                    // nested DL returned -- gameplay geometry (deeply nested
                    // DLs) then projected to ±100k px off-screen. We now
                    // recurse with shared global matrix state, exactly like
                    // the hardware call/return (RT64 push/popReturnAddress
                    // only saves the DL pointer, never the matrix).
                    state.dl_depth += 1;
                    decode_stream(rdram, w1, state);
                    state.dl_depth -= 1;
                } else {
                    eprintln!(
                        "[fn64-render-rt64/gbi] G_DL recursion exceeded MAX_DL_DEPTH \
                         ({MAX_DL_DEPTH}) -- refusing to recurse further (possible corrupt \
                         or cyclic display list)."
                    );
                }
            }
            G_TEXTURE => {
                // F3DEX2 gsSPTexture (§5.2): on-bit field(w0,1,7), tile
                // field(w0,8,3), S scale field(w1,16,16), T scale
                // field(w1,0,16) (both U0.16). Latch enable + tile + scale so
                // the next G_LOAD*/G_TRI can bind + address a texture.
                let on = ((w0 >> 1) & 0x7F) != 0;
                let tile = ((w0 >> 8) & 0x07) as u8;
                let scale_s = ((w1 >> 16) & 0xFFFF) as f32 / 65536.0;
                let scale_t = (w1 & 0xFFFF) as f32 / 65536.0;
                state.tex.tex_enabled = on;
                state.tex.tex_tile = tile;
                state.tex.tex_scale_s = scale_s;
                state.tex.tex_scale_t = scale_t;
            }
            G_RDPSETOTHERMODE => {
                // Full expert-mode write: high 24 bits live in w0's payload,
                // low 32 bits in w1 (gbi.h:3697-3737). OoT's setup DLs use
                // this path as well as the F3DEX2 partial setters.
                state.other_mode.high = w0 & 0x00FF_FFFF;
                state.other_mode.low = w1;
            }
            G_SETOTHERMODE_H => {
                // F3DEX2 gSPSetOtherMode (`gbi.h:3353-3369`) stores
                // `32-shift-len` at w0[15:8] and `len-1` at w0[7:0]. Rebuild
                // the selected H mask and preserve every other bit, matching
                // RT64's decode/update split (`rt64_gbi_f3dex2.cpp:24-33`,
                // `rt64_rsp.cpp:1026-1037`).
                if let Some(updated) = update_other_mode_word(state.other_mode.high, w0, w1) {
                    state.other_mode.high = updated;
                } else {
                    eprintln!("[fn64-render-rt64/gbi] malformed G_SETOTHERMODE_H w0={w0:#010x}");
                }
            }
            G_SETOTHERMODE_L => {
                if let Some(updated) = update_other_mode_word(state.other_mode.low, w0, w1) {
                    state.other_mode.low = updated;
                } else {
                    eprintln!("[fn64-render-rt64/gbi] malformed G_SETOTHERMODE_L w0={w0:#010x}");
                }
            }
            G_SETBLENDCOLOR => {
                // Public gbi.h:3646-3650 packs RGBA into w1, alpha in bits
                // 7..0. Threshold alpha compare uses precisely this component
                // (OoT z_rcp.c:815-835; RT64 RasterPS.hlsl:209-211).
                state.other_mode.blend_color_alpha = w1 as u8;
                state.blend_color = w1.to_be_bytes();
            }
            G_SETFOGCOLOR => state.fog_color = w1.to_be_bytes(),
            G_SETTIMG => {
                // G_SETTIMG (§5.1): format field(w0,21,3), size field(w0,19,2),
                // width-1 field(w0,0,12), image addr w1 (segmented). Pointer +
                // format latch only; no texel data moves until a G_LOAD*.
                state.tex.timg_fmt = ((w0 >> 21) & 0x07) as u8;
                state.tex.timg_siz = ((w0 >> 19) & 0x03) as u8;
                state.tex.timg_width = ((w0 & 0x0fff) + 1) as u16;
                state.tex.timg_addr = w1;
            }
            G_SETTILE => {
                // G_SETTILE (§5.1): w0 = fmt field(w0,21,3), siz field(w0,19,2),
                // line field(w0,9,9), tmem field(w0,0,9); w1 = tile
                // field(w1,24,3), palette field(w1,20,4), cmT field(w1,18,2),
                // cmS field(w1,8,2). The clamp/mirror/wrap mode's bit1
                // (G_TX_CLAMP=0x2) selects clamp-to-edge.
                let fmt = ((w0 >> 21) & 0x07) as u8;
                let siz = ((w0 >> 19) & 0x03) as u8;
                let line = ((w0 >> 9) & 0x1FF) as u16;
                let tile = ((w1 >> 24) & 0x07) as usize;
                let palette = ((w1 >> 20) & 0x0F) as u8;
                let cm_t = ((w1 >> 18) & 0x03) as u8;
                let cm_s = ((w1 >> 8) & 0x03) as u8;
                let t = &mut state.tex.tiles[tile];
                t.fmt = fmt;
                t.siz = siz;
                t.line = line;
                t.palette = palette;
                t.clamp_s = cm_s & 0x02 != 0;
                t.clamp_t = cm_t & 0x02 != 0;
            }
            G_SETTILESIZE => {
                // G_SETTILESIZE (§5.1): w0 = uls field(w0,12,12), ult
                // field(w0,0,12); w1 = tile field(w1,24,3), lrs field(w1,12,12),
                // lrt field(w1,0,12). Coords are S10.5 (÷4 for texel extent).
                let uls = ((w0 >> 12) & 0xFFF) as u16;
                let ult = (w0 & 0xFFF) as u16;
                let tile = ((w1 >> 24) & 0x07) as usize;
                let lrs = ((w1 >> 12) & 0xFFF) as u16;
                let lrt = (w1 & 0xFFF) as u16;
                let t = &mut state.tex.tiles[tile];
                t.uls = uls;
                t.ult = ult;
                t.lrs = lrs;
                t.lrt = lrt;
            }
            G_LOADTLUT => {
                // G_LOADTLUT (§5.1): load a CI palette from the latched TIMG
                // image. Public gbi.h packs `num - 1` directly into the
                // 10-bit field at bits 14..23. TLUT entries are 16-bit
                // RGBA5551 in RDRAM.
                let count = load_tlut_count(w1);
                let base = resolve_addr(&state.segments, state.tex.timg_addr);
                let mut tlut = Vec::with_capacity(count);
                for i in 0..count {
                    let px = read_u16(rdram, base + i * 2);
                    tlut.push(rgba5551_to_rgba8888(px));
                }
                state.tex.tlut = tlut;
            }
            G_LOADBLOCK | G_LOADTILE => {
                // G_LOADBLOCK / G_LOADTILE (§5.1): DMA texels into TMEM. We
                // instead decode the source TIMG image directly into an
                // RGBA8888 buffer sized by the active tile's SETTILESIZE
                // extent, and bind it as `current` so the next G_TRI* samples
                // it. (A first textured frame needs the right texels at the
                // right texcoords, not a byte-exact 4KiB TMEM model.)
                let tile = state.tex.tex_tile as usize;
                let load = if opcode == G_LOADTILE {
                    TextureLoad::Tile {
                        source_x: ((w0 >> 12) & 0x0fff) / 4,
                        source_y: (w0 & 0x0fff) / 4,
                    }
                } else {
                    TextureLoad::Block
                };
                if let Some(tex) =
                    decode_current_texture(rdram, &state.tex, &state.segments, tile, load)
                {
                    state.tex.current = Some(tex);
                }
            }
            G_MOVEMEM => {
                // F3DEX2 gsMoveMem (§1.4): w0 low byte = index (which RSP
                // block), field(w0,8,8) = offset/8, w1 = segmented source
                // address. G_MV_VIEWPORT (index 8) points at a 16-byte `Vp`;
                // G_MV_LIGHT (index 0x0a) points at a 16-byte `Light` DMA'd
                // into the slot the offset selects. Other indices (absolute
                // matrices, lookat) are acknowledged-and-skipped.
                let index = (w0 & 0xFF) as u8;
                let ofs_div8 = ((w0 >> 8) & 0xFF) as usize;
                if index == G_MV_VIEWPORT {
                    let addr = resolve_addr(&state.segments, w1);
                    if let Some(vp) = read_viewport(rdram, addr) {
                        state.viewport = Some(vp);
                    }
                } else if index == G_MV_LIGHT {
                    // gsSPLight (gbi.h:2911): ofs = n*24 + 24 (÷8 on the
                    // wire), so ofs/8 = 3*(n+1). DMEM indices 0 and 1 are
                    // look-at vectors; LIGHT_1 therefore maps from index 2
                    // to slot 0. Slot 0..num_dir-1 are directional; slot
                    // num_dir is the ambient.
                    if let Some(slot) = light_slot_from_movemem_offset(ofs_div8) {
                        let addr = resolve_addr(&state.segments, w1);
                        load_light(rdram, state, addr, slot);
                    } else {
                        skip_opcode(G_MOVEMEM);
                    }
                } else {
                    skip_opcode(G_MOVEMEM);
                }
            }
            G_GEOMETRYMODE => {
                // F3DEX2 gsSPGeometryMode (§2.4): one atomic clear+set --
                // `mode = (mode & field(w0,0,24)) | w1`, where the w0 low 24
                // bits are the (already-inverted) AND mask. We honor the
                // CULL_FRONT/CULL_BACK bits per-triangle (see cull_mode_from)
                // and the G_LIGHTING bit at G_VTX time (cn = normal -> lit
                // color, see load_vertices); fog/shade-smooth are not acted on.
                let and_mask = w0 & 0x00FF_FFFF;
                state.geometry_mode = (state.geometry_mode & and_mask) | w1;
            }
            G_SETCOMBINE => {
                // Public gbi.h GCCc*w* packing macros (lines 3543-3565)
                // distribute both cycles' RGB/alpha A/B/C/D selectors across
                // w0/w1. `CombinerMode::decode` resolves those raw selectors
                // to semantic sources using the position-specific mux tables.
                state.combiner.mode = CombinerMode::decode(w0, w1);
                warn_approximated_combiner_sources(state.combiner.mode, w0, w1);
            }
            G_SETPRIMCOLOR => {
                // gDPSetPrimColor (gbi.h:3672-3682): w0 low byte is the
                // primitive LOD fraction; w1 is RGBA8888. The min-level byte
                // is texture-LOD state and is outside this combiner slice.
                state.combiner.prim_lod_fraction = (w0 & 0xff) as u8;
                state.combiner.primitive = w1.to_be_bytes();
            }
            G_SETENVCOLOR => {
                // gDPSetEnvColor -> DPRGBColor (gbi.h:3626-3644): w1 packs
                // RGBA in bits 31..0, one byte per component.
                state.combiner.environment = w1.to_be_bytes();
            }
            G_SETSCISSOR => {
                // Public GBI packing (OoT ultra64/gbi.h:4819-4826): all four
                // edges are unsigned 12-bit quarter-pixels. The lower-right
                // edge is exclusive: OoT PreRender.c:137 passes `lrx + 1` /
                // `lry + 1` when converting its inclusive stored bounds.
                // RT64 likewise stores the fixed rect (rt64_rdp.cpp:974-980)
                // and intersects triangle bounds with it
                // (rt64_rsp.cpp:1140-1154).
                state.scissor = Some(ScissorRect {
                    ulx: ((w0 >> 12) & 0x0FFF) as f32 / 4.0,
                    uly: (w0 & 0x0FFF) as f32 / 4.0,
                    lrx: ((w1 >> 12) & 0x0FFF) as f32 / 4.0,
                    lry: (w1 & 0x0FFF) as f32 / 4.0,
                });
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                // 16-byte (two-word) RDP rectangle command (gbi.h:4973):
                // this word packs the lower-right corner (10.2 px) in w0 and
                // tile + upper-left corner in w1; the SECOND 8-byte word is
                // S/T start (S10.5 texels) + dsdx/dtdy steps (S5.10
                // texels/px). Advancing past that second word is mandatory
                // even when not drawing, or the stream desyncs.
                // The loop already advanced `pc` past this command's first
                // 8 bytes, so `pc` now addresses the coordinate payload.
                // F3DEX2's gDPTextureRectangle emits it as TWO more command
                // words -- G_RDPHALF_1 (0xE1, w1 = s<<16|t) then G_RDPHALF_2
                // (0xF1, w1 = dsdx<<16|dtdy), 24 bytes total (the form OoT
                // uses; verified from a live boot-logo task trace). The raw
                // RDP 16-byte form packs s/t + steps directly in the next
                // 8 bytes. Distinguish by the payload's opcode byte.
                let (s_word, d_word) = if read_u32(rdram, pc) >> 24 == G_RDPHALF_1 as u32 {
                    let s = read_u32(rdram, pc + 4);
                    let d = read_u32(rdram, pc + 12);
                    pc += 16;
                    (s, d)
                } else {
                    let s = read_u32(rdram, pc);
                    let d = read_u32(rdram, pc + 4);
                    pc += 8;
                    (s, d)
                };
                let mut lrx = ((w0 >> 12) & 0xFFF) as f32 / 4.0;
                let mut lry = (w0 & 0xFFF) as f32 / 4.0;
                let ulx = ((w1 >> 12) & 0xFFF) as f32 / 4.0;
                let uly = (w1 & 0xFFF) as f32 / 4.0;
                let s0 = ((s_word >> 16) as u16 as i16) as f32 / 32.0;
                let t0 = (s_word as u16 as i16) as f32 / 32.0;
                let mut dsdx = ((d_word >> 16) as u16 as i16) as f32 / 1024.0;
                let dtdy = (d_word as u16 as i16) as f32 / 1024.0;
                // COPY/FILL cycle types treat the lower-right edge as
                // INCLUSIVE, and COPY encodes dsdx pre-multiplied by 4
                // (the RDP copies 4 texels per clock) -- gbi.h's
                // gsSPTextureRectangle COPY-mode notes.
                match state.other_mode.cycle_type() {
                    CycleType::Copy => {
                        lrx += 1.0;
                        lry += 1.0;
                        dsdx /= 4.0;
                    }
                    CycleType::Fill => {
                        lrx += 1.0;
                        lry += 1.0;
                    }
                    CycleType::OneCycle | CycleType::TwoCycle => {}
                }
                if lrx > ulx && lry > uly {
                    // Emit as two screen-space triangles through the normal
                    // rasterizer path: z=0 (2D overlay, wins the z-test
                    // against any 3D geometry), w=1 (no perspective), white
                    // shade so SHADE-referencing combiners stay neutral.
                    // RDP rectangles ignore the RSP G_TEXTURE enable, so bind
                    // `current` directly instead of `active_texture`. The
                    // single-`current` tile approximation stands in for the
                    // command's tile index, as everywhere else.
                    let flip = opcode == G_TEXRECTFLIP;
                    let corner = |x: f32, y: f32| {
                        let (dx, dy) = (x - ulx, y - uly);
                        // FLIP exchanges which screen axis each texture axis
                        // walks (gbi.h G_TEXRECTFLIP).
                        let (s, t) = if flip {
                            (s0 + dy * dsdx, t0 + dx * dtdy)
                        } else {
                            (s0 + dx * dsdx, t0 + dy * dtdy)
                        };
                        Vertex {
                            x,
                            y,
                            z: 0.0,
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                            s,
                            t,
                            w: 1.0,
                        }
                    };
                    let quad = [
                        corner(ulx, uly),
                        corner(lrx, uly),
                        corner(lrx, lry),
                        corner(ulx, lry),
                    ];
                    let texture = state.tex.current.clone();
                    let blender = active_blender(state);
                    for idx in [[0usize, 1, 2], [0, 2, 3]] {
                        state.tris.push(Triangle {
                            v: [quad[idx[0]], quad[idx[1]], quad[idx[2]]],
                            scissor: state.scissor,
                            cull: CullMode::None,
                            texture: texture.clone(),
                            other_mode: state.other_mode,
                            combiner: state.combiner,
                            blender,
                        });
                    }
                }
            }
            G_ENDDL => break,
            _ => skip_opcode(opcode),
        }
    }
}

/// Derive the per-triangle [`CullMode`] from the current F3DEX2 geometry
/// mode's `G_CULL_FRONT`/`G_CULL_BACK` bits (`F3DEX2-CONCEPTS.md` §2.4).
fn cull_mode_from(geometry_mode: u32) -> CullMode {
    let front = geometry_mode & G_CULL_FRONT != 0;
    let back = geometry_mode & G_CULL_BACK != 0;
    match (front, back) {
        (true, true) => CullMode::Both,
        (true, false) => CullMode::Front,
        (false, true) => CullMode::Back,
        (false, false) => CullMode::None,
    }
}

/// Apply one F3DEX2 partial other-mode update. Returns `None` only for a
/// malformed range that cannot fit in a 32-bit H/L word.
fn update_other_mode_word(current: u32, w0: u32, data: u32) -> Option<u32> {
    let length = (w0 & 0xff) + 1;
    let encoded_shift = (w0 >> 8) & 0xff;
    let shift = 32u32.checked_sub(encoded_shift.checked_add(length)?)?;
    if length > 32 {
        return None;
    }
    let mask = if length == 32 {
        u32::MAX
    } else {
        (((1u64 << length) - 1) << shift) as u32
    };
    // Deliberately OR the complete data word, as RT64 does. Public gbi.h's
    // predefined render modes include G_AC_DITHER in bits 0..1 even though
    // gDPSetRenderMode requests the nominal bits-3..31 range
    // (`gbi.h:700-702,756-758,802-804,824-827,3484-3487`). Masking `data`
    // here would erase that alpha-compare mode from real OoT display lists.
    Some((current & !mask) | data)
}

/// The texture to bind to triangles emitted right now: the most-recently
/// decoded tile texture, but only while `G_TEXTURE` has enabled texturing.
/// `None` -> the triangle stays flat-shaded from vertex color.
fn active_texture(tex: &TexState) -> Option<Texture> {
    if tex.tex_enabled {
        tex.current.clone()
    } else {
        None
    }
}

fn active_blender(state: &DecodeState) -> BlenderState {
    BlenderState::from_other_mode(
        state.other_mode.raw_low(),
        state.other_mode.raw_high(),
        state.blend_color,
        state.fog_color,
    )
}

/// Recompute the cached model-view-projection matrix from the current stack.
///
/// `state.proj` already holds the accumulated `view * proj` product (built
/// left-multiplied in the G_MTX handler, hardware order). The full transform
/// is `mvp = modelview * (view * proj)` = `M * V * P`, kept in hardware
/// `[row][col]` layout. The incoming vertex is applied by `transform_point`
/// as a ROW vector (`clip = v_row · mvp`), reproducing the hardware's
/// `v · M · V · P` with a sane `w` (`≈ -z_eye`, the perspective depth). See
/// `transform_point` for why applying it as a column vector (`mvp · v`)
/// instead is the transpose and produces the sign-flipping ±thousands `w`.
fn recompute_mvp(state: &mut DecodeState) {
    state.mvp = state.proj.map(|p| mat_mul(&state.modelview, &p));
    if state.mvp.is_none() {
        // No projection loaded: use the modelview alone (still lets a
        // model-space-only transform position raw coords).
        // Leave mvp None only when NO transform at all was ever seen.
    }
}

/// Load `n` vertices starting at cache slot `v0` from the (segmented) array
/// at `arr_addr`, applying the active transform if one is loaded.
fn load_vertices(rdram: &[u8], state: &mut DecodeState, arr_addr: u32, n: usize, v0: usize) {
    let base = resolve_addr(&state.segments, arr_addr);
    for i in 0..n {
        let off = base + i * VTX_STRIDE;
        if off + VTX_STRIDE > rdram.len() || v0 + i >= state.vtx_cache.len() {
            break;
        }
        // Swizzled reads (recomp MEM_H / MEM_BU): vertex arrays are DMA'd
        // from ROM through the `^3` per-byte swizzle, same as the DL words.
        let x = read_i16(rdram, off) as f32;
        let y = read_i16(rdram, off + 2) as f32;
        let z = read_i16(rdram, off + 4) as f32;
        // tc[2] (offsets 8, 10): raw S/T in S10.5 fixed-point (§2.1). Scale
        // by the active G_TEXTURE S/T scale, then convert S10.5 -> texels
        // (÷32). The result is texels the rasterizer addresses directly.
        let raw_s = read_i16(rdram, off + 8) as f32;
        let raw_t = read_i16(rdram, off + 10) as f32;
        let s = raw_s * state.tex.tex_scale_s / 32.0;
        let t = raw_t * state.tex.tex_scale_t / 32.0;
        // cn[4] at offsets 12..16. The alpha byte is always alpha. The RGB
        // bytes are EITHER a flat vertex color (G_LIGHTING off) OR a signed
        // s8 NORMAL (G_LIGHTING on) that must be LIT into a color -- reading
        // a normal as a color is what produced the "rainbow fan" (signed
        // normal components read as unsigned channels). See G_LIGHTING.
        let a = read_u8(rdram, off + 15);
        let (r, g, b) = if state.geometry_mode & G_LIGHTING != 0 {
            let nx = (read_u8(rdram, off + 12) as i8) as f32 / 127.0;
            let ny = (read_u8(rdram, off + 13) as i8) as f32 / 127.0;
            let nz = (read_u8(rdram, off + 14) as i8) as f32 / 127.0;
            let [lr, lg, lb] = light_vertex(state, [nx, ny, nz]);
            (lr, lg, lb)
        } else {
            (
                read_u8(rdram, off + 12),
                read_u8(rdram, off + 13),
                read_u8(rdram, off + 14),
            )
        };

        let (sx, sy, sz, sw) = project_vertex(state, x, y, z);
        #[cfg(not(test))]
        {
            projdump::note_pz(sz);
            // On-screen NDC test: perspective-divide the clip coords and check
            // the NDC cube [-1,1]^3 (with a positive-w gate: w<=0 is behind cam).
            let onscreen = if sw > 1e-4 {
                let nx = sx; // sx/sy are already viewport-mapped pixels below;
                let _ = nx;
                // Reconstruct NDC from clip via mvp to classify honestly:
                false
            } else {
                false
            };
            let _ = onscreen;
            if let Some(mvp) = state.mvp {
                let clip = transform_point(&mvp, x, y, z);
                let (cx, cy, cz, cw) = (clip[0], clip[1], clip[2], clip[3]);
                let inside = cw.abs() > 1e-4
                    && (clip[0] / cw).abs() <= 1.0
                    && (clip[1] / cw).abs() <= 1.0
                    && (clip[2] / cw).abs() <= 1.0
                    && cw > 0.0;
                projdump::note_w(cw, inside);
                if projdump::should_log_vtx() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] vtx ob=({x:.0},{y:.0},{z:.0}) -> clip=({cx:.2},{cy:.2},{cz:.2},w={cw:.4}) ndc=({:.3},{:.3},{:.3}) inside_cube={inside}",
                        cx / cw, cy / cw, cz / cw
                    );
                }
            }
        }
        state.vtx_cache[v0 + i] = Vertex {
            x: sx,
            y: sy,
            z: sz,
            r,
            g,
            b,
            a,
            s,
            t,
            w: sw,
        };
    }
}

/// Map a model-space vertex to screen space. If a full projection*modelview
/// is active, apply it, perspective-divide, and map NDC [-1,1] through the
/// viewport (or a 320x240 default half-extent if no viewport was loaded).
/// If NO transform is loaded at all, the raw `ob` x/y are already screen
/// coordinates (the pre-existing reference-fixture convention) and pass
/// through unchanged.
fn project_vertex(state: &DecodeState, x: f32, y: f32, z: f32) -> (f32, f32, f32, f32) {
    match state.mvp {
        Some(mvp) => {
            let clip = transform_point(&mvp, x, y, z);
            // Keep the true clip-space w for near-plane culling (a vertex with
            // w <= 0 is at/behind the camera). Guard only the DIVIDE against a
            // near-zero w so the perspective divide doesn't overflow; the
            // decision to draw is made from the un-guarded `clip[3]` (returned
            // as the 4th component) in resolve_tri.
            let true_w = clip[3];
            let w = if true_w.abs() > 1e-6 { true_w } else { 1e-6 };
            let ndc_x = clip[0] / w;
            let ndc_y = clip[1] / w;
            let ndc_z = clip[2] / w;
            match &state.viewport {
                Some(vp) => {
                    // vscale/vtrans are in pixels (already /4 in read_viewport).
                    let px = ndc_x * vp.sx + vp.tx;
                    // N64 screen Y is top-down; NDC +Y is up, so flip.
                    let py = -ndc_y * vp.sy + vp.ty;
                    let pz = ndc_z * vp.sz + vp.tz;
                    (px, py, pz, true_w)
                }
                None => {
                    // Default viewport: 320x240, origin center.
                    let px = ndc_x * 160.0 + 160.0;
                    let py = -ndc_y * 120.0 + 120.0;
                    (px, py, ndc_z, true_w)
                }
            }
        }
        None => {
            // No transform: raw screen coords (reference-fixture path). w=1 so
            // the near-plane cull never rejects the raw/fixture geometry.
            (x, y, 0.0, 1.0)
        }
    }
}

/// Extract the three F3DEX2 triangle vertex-cache slot indices from a
/// command word: three 7-bit fields at bit offsets 17, 9, 1 (F3DEX2-
/// CONCEPTS.md §2.2). Each field is already the slot (0-31).
fn tri_indices(w: u32) -> [u32; 3] {
    [(w >> 17) & 0x7F, (w >> 9) & 0x7F, (w >> 1) & 0x7F]
}

/// A vertex is at/behind the near plane when its clip-space `w` is not
/// positive. Projecting such a vertex divides by a non-positive number and
/// flings it across the screen; a triangle touching one is dropped.
#[inline]
fn behind_near_plane(v: &Vertex) -> bool {
    v.w <= 1e-4
}

fn resolve_tri(
    vtx_cache: &[Vertex; 32],
    idx: [u32; 3],
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    if idx.iter().any(|&i| i as usize >= vtx_cache.len()) {
        return None;
    }
    let v = [
        vtx_cache[idx[0] as usize],
        vtx_cache[idx[1] as usize],
        vtx_cache[idx[2] as usize],
    ];
    // Coarse near-plane cull: if ANY vertex is at/behind the camera, the
    // perspective-divided screen position is meaningless (it lands on the
    // wrong side), so drop the whole triangle rather than draw the giant
    // wrong-side "fan" polygon. Proper clipping would split the triangle at
    // the near plane; dropping is the correct-image-preserving subset of that
    // (it removes the artifact without inventing geometry).
    if v.iter().any(behind_near_plane) {
        return None;
    }
    Some(Triangle {
        v: [
            vtx_cache[idx[0] as usize],
            vtx_cache[idx[1] as usize],
            vtx_cache[idx[2] as usize],
        ],
        scissor: None,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    })
}

/// This reference backend's one supported ucode family declaration --
/// shared constant so `lib.rs` and tests agree on it.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];

#[cfg(test)]
// Wire-encoding tests intentionally spell zero-valued bitfields, fixed 4x4
// indices, and traced f32 literals in their source form so the evidence stays
// directly comparable to the cited command/matrix layouts.
#[allow(
    clippy::excessive_precision,
    clippy::identity_op,
    clippy::needless_range_loop
)]
mod tests {
    use super::*;

    /// Write a logical big-endian s16 at `off` through the recomp `^3` byte
    /// swizzle (mirrors the decoder's `read_i16`/`read_u16` memory model).
    fn wr_i16(rdram: &mut [u8], off: usize, v: i16) {
        let b = (v as u16).to_be_bytes();
        rdram[off ^ 3] = b[0];
        rdram[(off + 1) ^ 3] = b[1];
    }

    /// Write an aligned logical 32-bit word (recomp `MEM_W`: native-endian,
    /// no swizzle), matching the decoder's `read_u32`. Used to plant raw
    /// display-list command words.
    fn wr_u32(rdram: &mut [u8], off: usize, v: u32) {
        rdram[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }

    /// Plant one 8-byte F3DEX2 command (`w0`, `w1`) at byte offset `off`.
    fn wr_cmd(rdram: &mut [u8], off: usize, w0: u32, w1: u32) {
        wr_u32(rdram, off, w0);
        wr_u32(rdram, off + 4, w1);
    }

    #[test]
    fn command_trace_follows_segmented_calls_and_fingerprints_targets() {
        let mut rdram = vec![0u8; 0x4000];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_SEGMENT as u32) << 16) | 0x0c,
            0x8000_3000,
        );
        wr_cmd(&mut rdram, 0x1008, (G_DL as u32) << 24, 0x0300_0100);
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
        wr_cmd(
            &mut rdram,
            0x3100,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            0x0300_0200,
        );
        wr_cmd(&mut rdram, 0x3108, (G_ENDDL as u32) << 24, 0);
        rdram[0x3200] = 0x5a;

        let trace = trace_display_list_f3dex2(&rdram, 0x1000);
        assert!(trace.contains("SEG depth=0 segment=3 base=0x003000"));
        assert!(trace.contains("ENTER depth=1 segmented=0x03000100 resolved=0x003100"));
        assert!(trace.contains("target=0x003200 bytes=16 nonzero=1"));
        assert!(trace.contains("SUMMARY commands=5"));
        assert!(trace.contains("OPCODE op=0xdf count=2"));
    }

    /// Pack raw `gsDPSetCombineLERP` selectors exactly like public gbi.h's
    /// `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1` macros (lines 3543-3565).
    fn combine_cmd(
        rgb0: [u32; 4],
        alpha0: [u32; 4],
        rgb1: [u32; 4],
        alpha1: [u32; 4],
    ) -> (u32, u32) {
        let w0 = ((G_SETCOMBINE as u32) << 24)
            | ((rgb0[0] & 0x0f) << 20)
            | ((rgb0[2] & 0x1f) << 15)
            | ((alpha0[0] & 0x07) << 12)
            | ((alpha0[2] & 0x07) << 9)
            | ((rgb1[0] & 0x0f) << 5)
            | (rgb1[2] & 0x1f);
        let w1 = ((rgb0[1] & 0x0f) << 28)
            | ((rgb1[1] & 0x0f) << 24)
            | ((alpha1[0] & 0x07) << 21)
            | ((alpha1[2] & 0x07) << 18)
            | ((rgb0[3] & 0x07) << 15)
            | ((alpha0[1] & 0x07) << 12)
            | ((alpha0[3] & 0x07) << 9)
            | ((rgb1[3] & 0x07) << 6)
            | ((alpha1[1] & 0x07) << 3)
            | (alpha1[3] & 0x07);
        (w0, w1)
    }

    #[test]
    fn setcombine_and_color_registers_are_snapshotted_on_triangles() {
        // Fail-against-bug: before this change all three commands fell into
        // the skip arm, so Triangle had no mode/primitive/environment state
        // and the rasterizer could only hardwire TEXEL0*SHADE.
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x3000, 2, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x3010, 12, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x3020, 7, 12, 0, [255, 255, 255, 255]);

        // Cycle 0 G_CC_BLENDI RGB: (ENV-SHADE)*TEXEL0+SHADE.
        // Alpha: TEXEL0*PRIMITIVE. Cycle 1 deliberately uses distinct
        // non-zero selectors in every field so a shifted/masked decode fails.
        let (cc0, cc1) = combine_cmd([5, 4, 1, 4], [1, 7, 3, 7], [3, 5, 11, 1], [5, 4, 6, 3]);
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, cc0, cc1);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_SETPRIMCOLOR as u32) << 24) | 0x7f,
            0x11_22_33_44,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETENVCOLOR as u32) << 24, 0xa0_b0_c0_d0);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        let cc = tris[0].combiner;
        assert_eq!(cc.primitive, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(cc.environment, [0xa0, 0xb0, 0xc0, 0xd0]);
        assert_eq!(cc.prim_lod_fraction, 0x7f);
        assert_eq!(
            cc.mode.cycles[0].rgb,
            [
                ColorSource::Environment,
                ColorSource::Shade,
                ColorSource::Texel0,
                ColorSource::Shade,
            ]
        );
        assert_eq!(
            cc.mode.cycles[0].alpha,
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ]
        );
        assert_eq!(
            cc.mode.cycles[1].rgb,
            [
                ColorSource::Primitive,
                ColorSource::Environment,
                ColorSource::ShadeAlpha,
                ColorSource::Texel0,
            ]
        );
        assert_eq!(
            cc.mode.cycles[1].alpha,
            [
                AlphaSource::Environment,
                AlphaSource::Shade,
                AlphaSource::PrimLodFraction,
                AlphaSource::Primitive,
            ]
        );
    }

    /// Plant a full 16-byte `Vtx` (`ob` x/y/z at 0/2/4, color at 12) at `off`
    /// so a `G_VTX` + `G_TRI1` can resolve a real triangle.
    fn wr_vtx(rdram: &mut [u8], off: usize, x: i16, y: i16, z: i16, rgba: [u8; 4]) {
        wr_i16(rdram, off, x);
        wr_i16(rdram, off + 2, y);
        wr_i16(rdram, off + 4, z);
        for (i, &c) in rgba.iter().enumerate() {
            rdram[(off + 12 + i) ^ 3] = c;
        }
    }

    /// Write a 64-byte fixed-point `Mtx` at `off` from an f32 `[row][col]`
    /// matrix, matching `read_mtx`'s layout: element (r,c) integer half at
    /// `off + (r*4+c)*2`, fractional half at `off + 32 + (r*4+c)*2`, both
    /// through the recomp `^3` swizzle (via `wr_i16`).
    fn wr_mtx(rdram: &mut [u8], off: usize, m: [[f32; 4]; 4]) {
        for (r, row) in m.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                let elem = r * 4 + c;
                let fixed = (*value * 65536.0).round() as i32;
                let int_half = (fixed >> 16) as i16;
                let frac_half = (fixed & 0xFFFF) as u16;
                wr_i16(rdram, off + elem * 2, int_half);
                wr_i16(rdram, off + 32 + elem * 2, frac_half as i16);
            }
        }
    }

    /// Encode the F3DEX2 partial other-mode range used by
    /// `gSPSetOtherMode` (`gbi.h:3353-3369`).
    fn other_mode_cmd(opcode: u8, shift: u32, length: u32) -> u32 {
        ((opcode as u32) << 24) | ((32 - shift - length) << 8) | (length - 1)
    }

    /// Fails against the pre-fix name-table-only decoder: several partial H/L
    /// writes must merge without clobbering each other, and the resulting
    /// cycle/filter/dither/alpha/coverage/Z/blender state plus blend-alpha
    /// threshold must be snapshotted onto the emitted triangle.
    #[test]
    fn other_mode_partial_updates_are_decoded_and_carried_per_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 255]);

        let mut off = 0x1000;
        let mut emit = |w0: u32, w1: u32| {
            wr_cmd(&mut rdram, off, w0, w1);
            off += 8;
        };
        emit(((G_VTX as u32) << 24) | (3 << 12) | (3 << 1), 0x2000);
        emit(other_mode_cmd(G_SETOTHERMODE_H, 20, 2), 2 << 20); // G_CYC_COPY
        emit(other_mode_cmd(G_SETOTHERMODE_H, 12, 2), 2 << 12); // G_TF_BILERP
        emit(other_mode_cmd(G_SETOTHERMODE_H, 6, 2), 3 << 6); // G_CD_DISABLE
        emit(other_mode_cmd(G_SETOTHERMODE_H, 4, 2), 2 << 4); // G_AD_NOISE
        emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 1); // G_AC_THRESHOLD

        let blender = (1 << 30) | (2 << 26) | (3 << 22) | (2 << 28) | (1 << 24) | (3 << 16);
        let render = blender | 0x0010 | 0x0020 | 0x0100 | 0x0800 | 0x1000 | 0x2000 | 0x4000;
        emit(other_mode_cmd(G_SETOTHERMODE_L, 3, 29), render);
        emit((G_SETBLENDCOLOR as u32) << 24, 0x0102_0380);
        emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
        emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 0); // G_AC_NONE
        emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
        emit((G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 2);
        let mode = tris[0].other_mode;
        assert_eq!(mode.cycle_type(), CycleType::Copy);
        assert_eq!(mode.texture_filter(), TextureFilter::Bilinear);
        assert_eq!(mode.rgb_dither(), RgbDither::Disabled);
        assert_eq!(mode.alpha_dither(), AlphaDither::Noise);
        assert_eq!(mode.alpha_compare(), AlphaCompare::Threshold);
        assert_eq!(mode.blend_color_alpha, 0x80);
        assert!(mode.depth_compare_enabled());
        assert!(mode.depth_update_enabled());
        assert_eq!(mode.coverage_destination(), CoverageDestination::Wrap);
        assert_eq!(mode.depth_mode(), DepthMode::Translucent);
        assert!(mode.coverage_times_alpha());
        assert!(mode.alpha_coverage_select());
        assert!(mode.force_blend());
        assert_eq!(
            mode.blender_cycle_1(),
            BlenderCycle {
                color_a: 1,
                alpha_a: 2,
                color_b: 3,
                alpha_b: 0,
            }
        );
        assert_eq!(
            mode.blender_cycle_2(),
            BlenderCycle {
                color_a: 2,
                alpha_a: 1,
                color_b: 0,
                alpha_b: 3,
            }
        );
        assert_eq!(tris[1].other_mode.alpha_compare(), AlphaCompare::None);
        assert_eq!(tris[1].other_mode.raw_high(), mode.raw_high());
        assert_eq!(tris[1].other_mode.raw_low(), mode.raw_low() & !3);
    }

    /// OoT's public G_RM_* constants embed G_AC_DITHER outside the nominal
    /// gDPSetRenderMode bits-3..31 range. This fails if the decoder masks w1
    /// instead of following the RSP/RT64 full-data OR behavior.
    #[test]
    fn render_mode_update_keeps_embedded_alpha_dither_bits() {
        let w0 = other_mode_cmd(G_SETOTHERMODE_L, 3, 29);
        let updated = update_other_mode_word(0, w0, 3 | 0x0010).unwrap();
        let mode = OtherMode::from_raw(0, updated, 0);
        assert_eq!(mode.alpha_compare(), AlphaCompare::Dither);
        assert!(mode.depth_compare_enabled());
    }

    /// Fails against the original decoder, which loudly skipped opcode 0xEF
    /// and emitted every triangle with overwrite-only default state.
    #[test]
    fn full_othermode_command_snapshots_both_blender_cycles_on_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 128]);
        wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 128]);
        wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 128]);

        // G_CYC_2CYCLE plus the standard XLU tuple in both cycles:
        // IN*A_IN + MEM*(1-A). Selector positions are exactly GBL_c1/c2
        // (gbi.h:624-627); FORCE_BL is gbi.h:609.
        let high = 1 << 20;
        let low = (1 << 22) | (1 << 20) | 0x4000;
        let mut off = 0x1000;
        wr_cmd(
            &mut rdram,
            off,
            ((G_RDPSETOTHERMODE as u32) << 24) | high,
            low,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETBLENDCOLOR as u32) << 24, 0x1020_3040);
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETFOGCOLOR as u32) << 24, 0x5060_7080);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x2000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        let blender = tris[0].blender;
        assert_eq!(blender.cycle_count, 2);
        assert!(blender.force_blend);
        assert_eq!(blender.blend_color, [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(blender.fog_color, [0x50, 0x60, 0x70, 0x80]);
        for cycle in blender.cycles {
            assert_eq!(cycle.p, BlendColorInput::Combined);
            assert_eq!(cycle.a, BlendAlphaInput::Combined);
            assert_eq!(cycle.m, BlendColorInput::Framebuffer);
            assert_eq!(cycle.b, BlendBInput::OneMinusA);
        }
    }

    /// Partial setters use F3DEX2's inverted shift field, not the older F3D
    /// direct shift. These exact words are what gDPSetCycleType and
    /// gDPSetRenderMode emit through gSPSetOtherMode (gbi.h:3353-3369).
    #[test]
    fn partial_othermode_commands_patch_the_logical_bit_ranges() {
        let cycle_type_w0 = ((0xE3u32) << 24) | (10 << 8) | 1;
        let high = update_other_mode_word(0, cycle_type_w0, 1 << 20).unwrap();
        assert_eq!((high >> 20) & 3, 1);

        let render_mode_w0 = ((0xE2u32) << 24) | 28;
        let render_mode = (1 << 22) | (1 << 20) | 0x4000;
        let low = update_other_mode_word(0b101, render_mode_w0, render_mode).unwrap();
        assert_eq!(low & 0b111, 0b101, "bits below render mode stay intact");
        assert_eq!(low & !0b111, render_mode);
    }

    #[test]
    fn setscissor_decodes_quarter_pixel_edges_on_emitted_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 3, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x2010, 12, 3, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x2020, 2, 13, 0, [0, 0, 255, 255]);

        let raw_ulx = 5u32; // 1.25 px
        let raw_uly = 10u32; // 2.5 px
        let raw_lrx = 43u32; // 10.75 px
        let raw_lry = 48u32; // 12 px
        let mut off = 0x1000;
        wr_cmd(
            &mut rdram,
            off,
            ((G_SETSCISSOR as u32) << 24) | (raw_ulx << 12) | raw_uly,
            (raw_lrx << 12) | raw_lry,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x2000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        assert_eq!(
            tris[0].scissor,
            Some(ScissorRect {
                ulx: 1.25,
                uly: 2.5,
                lrx: 10.75,
                lry: 12.0,
            })
        );
    }

    // --- Perspective * view * model projection regression ----------------
    //
    // Fails against the pre-fix decoder, which transposed each `Mtx` on read
    // AND accumulated the projection product in the wrong (proj-first) order.
    // The two errors cancel for a single diagonal/symmetric matrix -- all the
    // older f3dex2_replay fixture exercised -- so the bug slipped through, but
    // for a real guPerspective * guLookAt * model chain the net composed MVP
    // came out as the TRANSPOSE of the true one: clip `w` collapsed to tiny,
    // sign-flipping values (~30, ~-13, ~-59) and the perspective divide flung
    // a vertex that belongs near screen-center to ~800+ px off the 320x240
    // screen. This test drives the exact live-OoT-gameplay P/V/M matrices
    // through the full decode and asserts the vertex now lands on-screen with
    // a coherent positive `w`.
    #[test]
    fn perspective_view_model_projects_vertex_on_screen() {
        let mut rdram = vec![0u8; 0x8000];

        // A CLEAN, self-consistent row-vector (N64 [row][col]) setup whose
        // on-screen anchors are derived INDEPENDENTLY, NOT reverse-engineered
        // to fit a transposed apply. guPerspective(fovy=60, aspect=4/3,
        // near=10, far=1000): projective term [2][3]=-1, depth translate at
        // [3][2]. The modelview is deliberately ASYMMETRIC (a 20° rotation
        // about Y + a translation to (30,-15,-120)) so `mvp != mvp^T` -- a
        // pure-translation/diagonal MVP would be transpose-invariant and could
        // NOT distinguish the bug from the fix.
        //
        // Under the CORRECT row-vector transform `clip = v · (M · P)`:
        //   - object origin (0,0,0) -> w=120, screen (211.96, 145.98);
        //   - vertex (10,20,0)      -> w=123.42, screen (226.35, 111.58).
        // The pre-fix COLUMN-vector apply (`(M·P)·v`) is the transpose: it
        // sends (10,20,0) to w=-9.9 (behind the camera) / px=(-42.7, 539.7),
        // i.e. off-screen -- the fanning-triangle bug.
        let persp = [
            [1.299038, 0.0, 0.0, 0.0],
            [0.0, 1.732051, 0.0, 0.0],
            [0.0, 0.0, -1.020202, -1.0],
            [0.0, 0.0, -20.202_02, 0.0],
        ];
        // Asymmetric modelview: rot(20° about Y) then translate(30,-15,-120),
        // in hardware [row][col] row-vector layout.
        let model = [
            [0.939693, 0.0, -0.342020, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.342020, 0.0, 0.939693, 0.0],
            [30.0, -15.0, -120.0, 1.0],
        ];

        // Triangle vertices (model space): center, +x/+y offset, +x.
        wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 10, 20, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 20, 0, 0, [0, 0, 255, 255]);

        wr_mtx(&mut rdram, 0x2000, persp);
        wr_mtx(&mut rdram, 0x2200, model);

        // G_MTX param bytes (wire = params ^ G_MTX_PUSH):
        //  perspective LOAD:  PROJECTION|LOAD   = 0x06 -> wire 0x07
        //  model LOAD:        LOAD (modelview)  = 0x02 -> wire 0x03
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x03), 0x2200); // model LOAD
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1, "expected one transformed triangle");
        // Default 320x240 viewport (no G_MOVEMEM): NDC*160+160 / *120+120.
        // Independently-derived anchors (see numpy in the doc above):
        //   v0 (object origin) -> (211.96, 145.98), w=120.
        //   v1 (10,20,0)       -> (226.35, 111.58), w=123.42.
        let v0 = &tris[0].v[0];
        assert!(
            (v0.x - 211.96).abs() < 0.5 && (v0.y - 145.98).abs() < 0.5,
            "object-origin vertex must land at its independently-derived anchor \
             (~211.96, ~145.98) under the correct row-vector MVP; got ({}, {}) \
             (the transposed/column-vector apply misses it, off-screen)",
            v0.x,
            v0.y
        );
        let v1 = &tris[0].v[1];
        assert!(
            (v1.x - 226.35).abs() < 0.5 && (v1.y - 111.58).abs() < 0.5,
            "offset vertex drifted from the independently-derived on-screen \
             anchor (~226.35, ~111.58); got ({}, {}) -- a re-transpose sends it \
             to px≈(-42.7, 539.7), off-screen",
            v1.x,
            v1.y
        );
        // The sane depth is the load-bearing signal: w must be ~ -z_eye = 120,
        // never the pre-fix ±thousands sign-flipping garbage. (The transposed
        // apply gives v1 a NEGATIVE w=-9.9 -- behind the camera.)
        assert!(
            (v0.w - 120.0).abs() < 0.5,
            "clip-w must be the sane perspective depth ~120, got {}",
            v0.w
        );
    }

    // --- G_DL branch (gsSPBranchList) desync regression -----------------
    //
    // Fails against the pre-fix decoder: a G_DL with the NOPUSH (branch)
    // flag used to recurse into the target and then CONTINUE decoding the
    // parent stream. Because a branch's trailing bytes are not commands
    // (here: raw garbage), the decoder walked into them and every byte
    // became a bogus opcode -- the exact ~14K-junk-skip cascade seen on the
    // real OoT gameplay task. After the fix a branch STOPS the parent stream.

    #[test]
    fn g_dl_branch_does_not_decode_bytes_after_the_branch() {
        // Layout:
        //   0x1000  parent DL: [G_DL NOPUSH -> 0x2000], then GARBAGE, G_ENDDL
        //   0x2000  target DL: [G_VTX(3) @ 0x3000], [G_TRI1 0,1,2], G_ENDDL
        //   0x3000  three vertices
        let mut rdram = vec![0u8; 0x4000];

        // Parent stream at 0x1000.
        // gsSPBranchList: w0 = G_DL<<24 | G_DL_NOPUSH<<16, w1 = target addr.
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_DL as u32) << 24) | (0x01 << 16),
            0x2000,
        );
        // "Garbage" right after the branch that the PRE-FIX decoder would
        // wrongly execute: a second VTX+TRI1 pair drawing a spurious extra
        // triangle. (In the real bug these trailing bytes were zero-fill /
        // an unrelated buffer that cascaded into ~14K junk-opcode skips; a
        // spurious *triangle* is the same "kept decoding after the branch"
        // fault, made observable as a hard count assertion.)
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        wr_cmd(
            &mut rdram,
            0x1010,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);

        // Target stream at 0x2000: load 3 verts, draw 1 triangle, end.
        // G_VTX: n=3 in bits 12-19, end=3 in bits 1-7 -> v0 = end - n = 0.
        wr_cmd(
            &mut rdram,
            0x2000,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        // G_TRI1: three 7-bit slots at bits 17/9/1 -> slots 0,1,2.
        wr_cmd(
            &mut rdram,
            0x2008,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

        // Three vertices (raw screen coords; no transform loaded).
        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // Segment 0 is identity here (addresses are already physical).
        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();

        // Exactly the ONE triangle from the branched-to target -- no extra
        // garbage triangles, and (the real proof) no unrecognized-opcode
        // cascade from decoding the bytes after the branch. Pre-fix this
        // would have walked the 0x1008.. garbage as opcodes.
        assert_eq!(
            tris.len(),
            1,
            "branch must run the target then stop; got {} triangles \
             (pre-fix bug decoded post-branch garbage)",
            tris.len()
        );
        // The triangle carries the three planted vertex colors.
        assert_eq!(tris[0].v[0].r, 255);
        assert_eq!(tris[0].v[1].g, 255);
        assert_eq!(tris[0].v[2].b, 255);
    }

    #[test]
    fn g_dl_call_resumes_parent_after_target() {
        // A CALL (G_DL_PUSH=0) must recurse AND resume the parent: parent
        // draws one tri, calls a sub-DL that draws one tri, then parent draws
        // a third after the call returns -> 3 triangles total.
        let mut rdram = vec![0u8; 0x4000];

        // Shared vertices at 0x3000 (0,1,2).
        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        let vtx = |rd: &mut [u8], off: usize| {
            wr_cmd(
                rd,
                off,
                ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
                0x3000,
            );
        };
        let tri1 = |rd: &mut [u8], off: usize| {
            wr_cmd(rd, off, ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0);
        };

        // Parent at 0x1000: VTX, TRI1, G_DL CALL -> 0x2000, TRI1, ENDDL.
        vtx(&mut rdram, 0x1000);
        tri1(&mut rdram, 0x1008);
        wr_cmd(&mut rdram, 0x1010, (G_DL as u32) << 24, 0x2000); // push=0 -> CALL
        tri1(&mut rdram, 0x1018);
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        // Sub-DL at 0x2000: VTX, TRI1, ENDDL.
        vtx(&mut rdram, 0x2000);
        tri1(&mut rdram, 0x2008);
        wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            3,
            "call must resume the parent after the target returns"
        );
    }

    #[test]
    fn g_dl_branch_chain_longer_than_call_stack_decodes_fully() {
        // A chain of 40 tail branches (gsSPBranchList) ending in a DL that
        // draws one triangle. On hardware a branch consumes NO return-stack
        // entry, so any chain length is legal. The pre-fix decoder recursed
        // per branch and counted it against MAX_DL_DEPTH, so a chain longer
        // than the cap silently dropped the tail (this exact "G_DL recursion
        // exceeded" warning fired on real OoT field frames).
        const CHAIN: usize = 40;
        let mut rdram = vec![0u8; 0x8000];

        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // Links at 0x1000, 0x1010, 0x1020, ... each: [branch -> next], and
        // garbage would follow (nothing does -- a branch never returns).
        for i in 0..CHAIN {
            let at = 0x1000 + i * 0x10;
            let next = (0x1000 + (i + 1) * 0x10) as u32;
            wr_cmd(&mut rdram, at, ((G_DL as u32) << 24) | (0x01 << 16), next);
        }
        // Terminal DL after the last link: VTX, TRI1, ENDDL.
        let end = 0x1000 + CHAIN * 0x10;
        wr_cmd(
            &mut rdram,
            end,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        wr_cmd(
            &mut rdram,
            end + 8,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, end + 16, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            1,
            "a {CHAIN}-deep branch chain must reach its terminal DL \
             (branches consume no stack entry)"
        );
    }

    #[test]
    fn g_dl_cyclic_branch_terminates() {
        // A branch list that branches to ITSELF: hardware would spin
        // forever; the decoder must terminate via the whole-decode command
        // budget and return what it has (nothing here).
        let mut rdram = vec![0u8; 0x2000];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_DL as u32) << 24) | (0x01 << 16),
            0x1000,
        );
        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert!(tris.is_empty());
    }

    #[test]
    fn g_texrect_consumes_two_words_and_does_not_desync() {
        // A G_TEXRECT (0xE4) is a 16-byte command. If the decoder advances
        // only 8 bytes it reads the coord word as a bogus opcode. Here the
        // texrect's second word is crafted to look like a G_VTX opcode
        // (0x01..) that, if wrongly decoded, would load a spurious vertex.
        // A correct 16-byte skip walks straight to the real G_TRI1.
        let mut rdram = vec![0u8; 0x4000];

        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // VTX (3 verts).
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        // G_TEXRECT word 0 + word 1. The SECOND 8-byte word starts with 0x01
        // (a G_VTX opcode byte) to catch an under-advance.
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_TEXRECT as u32) << 24) | 0x00abcdef,
            0x12345678,
        );
        wr_cmd(&mut rdram, 0x1010, 0x0100_4008, 0x0100_1c00); // texrect 2nd word

        // Real G_TRI1 after the full 16-byte texrect.
        wr_cmd(
            &mut rdram,
            0x1018,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            3,
            "texrect (2 tris) must consume both words so the following \
             G_TRI1 is decoded at the right offset (3rd tri)"
        );
        assert_eq!(
            [tris[2].v[0].r, tris[2].v[1].g, tris[2].v[2].b],
            [255, 255, 255],
            "the trailing G_TRI1's red/green/blue vertices decoded intact"
        );
    }

    /// Plant a minimal loaded I8 4x2 texture (SETTIMG + SETTILE +
    /// SETTILESIZE + LOADBLOCK) at `off`, texels at `timg`, returning the
    /// offset after the last command.
    fn plant_i8_4x2_texture(rdram: &mut [u8], mut off: usize, timg: u32) -> usize {
        for (i, texel) in [0x00u8, 0x40, 0x80, 0xC0, 0x11, 0x51, 0x91, 0xD1]
            .into_iter()
            .enumerate()
        {
            wr_u8(rdram, timg as usize + i, texel);
        }
        // G_SETTIMG: fmt=I(4), siz=8b(1), width=4.
        wr_cmd(
            rdram,
            off,
            ((G_SETTIMG as u32) << 24) | (4 << 21) | (1 << 19) | (4 - 1),
            timg,
        );
        off += 8;
        // G_SETTILE tile 0: fmt=I, siz=8b, clamp S+T.
        wr_cmd(
            rdram,
            off,
            ((G_SETTILE as u32) << 24) | (4 << 21) | (1 << 19),
            (2 << 18) | (2 << 8),
        );
        off += 8;
        // G_SETTILESIZE tile 0: 4x2 texels (S10.5 inclusive bounds).
        wr_cmd(
            rdram,
            off,
            (G_SETTILESIZE as u32) << 24,
            ((4u32 - 1) * 4) << 12 | ((2 - 1) * 4),
        );
        off += 8;
        wr_cmd(rdram, off, (G_LOADBLOCK as u32) << 24, 0);
        off + 8
    }

    #[test]
    fn texrect_emits_two_screen_space_triangles_with_texel_uvs() {
        // gSPTextureRectangle (gbi.h:4973): word A = opcode + lrx/lry
        // (10.2 px), word B = tile + ulx/uly; the second 8-byte word is
        // s/t (S10.5 texels) + dsdx/dtdy (S5.10 texels-per-pixel). The rect
        // (10,20)-(42,28) with s=2.0,t=1.0 and unit steps must become two
        // screen-space triangles (z=0, w=1, white shade) whose corner UVs
        // run (2,1) at the upper-left to (2+32, 1+8) at the lower-right,
        // sampling the bound texture even though no G_TEXTURE enable ran
        // (RDP rectangles bypass the RSP texture-on bit).
        let mut rdram = vec![0u8; 0x4000];
        let mut off = plant_i8_4x2_texture(&mut rdram, 0x1000, 0x2000);

        wr_cmd(
            &mut rdram,
            off,
            ((G_TEXRECT as u32) << 24) | ((42 * 4) << 12) | (28 * 4),
            ((10 * 4) << 12) | (20 * 4),
        );
        off += 8;
        wr_cmd(&mut rdram, off, (64 << 16) | 32, (1024 << 16) | 1024);
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 2, "one texrect = two triangles");

        let mut corners: Vec<(i32, i32, i32, i32)> = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| {
                assert_eq!(v.z, 0.0, "texrect is 2D: no depth");
                assert_eq!(v.w, 1.0, "texrect is 2D: no perspective");
                assert_eq!([v.r, v.g, v.b, v.a], [255; 4], "neutral white shade");
                (
                    v.x as i32,
                    v.y as i32,
                    (v.s * 32.0) as i32,
                    (v.t * 32.0) as i32,
                )
            })
            .collect();
        corners.sort_unstable();
        corners.dedup();
        assert_eq!(
            corners,
            vec![
                (10, 20, 64, 32),
                (10, 28, 64, 32 + 8 * 32),
                (42, 20, 64 + 32 * 32, 32),
                (42, 28, 64 + 32 * 32, 32 + 8 * 32),
            ],
            "4 unique corners spanning the rect, UVs stepped by dsdx/dtdy"
        );
        for t in &tris {
            let tex = t.texture.as_ref().expect(
                "texrect must sample the loaded tile without a G_TEXTURE enable",
            );
            assert_eq!((tex.width, tex.height), (4, 2));
            assert_eq!(t.cull, CullMode::None, "RDP rects are never culled");
        }
    }

    #[test]
    fn texrect_reads_st_and_steps_from_rdphalf_words() {
        // F3DEX2's gDPTextureRectangle does NOT inline s/t in the second
        // 8 bytes: it emits THREE words -- E4 (corners), G_RDPHALF_1 (0xE1,
        // w1 = s<<16|t), G_RDPHALF_2 (0xF1, w1 = dsdx<<16|dtdy) -- 24 bytes
        // total. This is the form OoT's boot logo uses (verified from a live
        // task-150 trace: E4 at 0x16a820, next command at 0x16a838). Reading
        // the E1 command word itself as s/t samples texel (0,0) forever.
        let mut rdram = vec![0u8; 0x4000];
        let mut off = plant_i8_4x2_texture(&mut rdram, 0x1000, 0x2000);

        wr_cmd(
            &mut rdram,
            off,
            ((G_TEXRECT as u32) << 24) | ((42 * 4) << 12) | (28 * 4),
            ((10 * 4) << 12) | (20 * 4),
        );
        off += 8;
        wr_cmd(&mut rdram, off, 0xE100_0000, (64 << 16) | 32);
        off += 8;
        wr_cmd(&mut rdram, off, 0xF100_0000, (1024 << 16) | 1024);
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 2, "24-byte texrect still emits two triangles");
        let min_s = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| (v.s * 32.0) as i32)
            .min()
            .unwrap();
        let max_s = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| (v.s * 32.0) as i32)
            .max()
            .unwrap();
        assert_eq!(
            (min_s, max_s),
            (64, 64 + 32 * 32),
            "s/t come from RDPHALF_1's w1, steps from RDPHALF_2's w1"
        );
    }

    #[test]
    fn texrect_copy_mode_uses_inclusive_bounds_and_quarter_dsdx() {
        // In G_CYC_COPY the RDP copies 4 texels per clock: dsdx is encoded
        // pre-multiplied by 4 (gbi.h gsSPTextureRectangle COPY notes) and the
        // lower-right edge is INCLUSIVE. A (0,0)-(31,15) COPY texrect with
        // dsdx=4<<10 must span 32x16 pixels with unit texel steps.
        let mut rdram = vec![0u8; 0x4000];
        let mut off = plant_i8_4x2_texture(&mut rdram, 0x1000, 0x2000);

        // G_RDPSETOTHERMODE: high 24 bits in w0 payload; G_CYC_COPY = 2<<20.
        wr_cmd(&mut rdram, off, ((G_RDPSETOTHERMODE as u32) << 24) | (2 << 20), 0);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TEXRECT as u32) << 24) | ((31 * 4) << 12) | (15 * 4),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, 0, ((4 << 10) << 16) | (1 << 10));
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 2);
        let max_x = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| v.x as i32)
            .max()
            .unwrap();
        let max_y = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| v.y as i32)
            .max()
            .unwrap();
        assert_eq!((max_x, max_y), (32, 16), "COPY lower-right is inclusive");
        let max_s = tris
            .iter()
            .flat_map(|t| t.v.iter())
            .map(|v| (v.s * 32.0) as i32)
            .max()
            .unwrap();
        assert_eq!(
            max_s,
            32 * 32,
            "COPY dsdx is stored x4: 32px at 1 texel/px, not 4 texels/px"
        );
    }

    // --- Viewport mapping (priority 1) ----------------------------------

    #[test]
    fn read_viewport_divides_quarter_pixel_encoding_by_four() {
        // OoT's real full-screen viewport: vscale (640,480,z), vtrans same,
        // in the ×4 "quarter-pixel" encoding -> 160/120 px after ÷4 (§3.5).
        let mut rdram = vec![0u8; 64];
        let addr = 0x10;
        wr_i16(&mut rdram, addr, 640); // vscale.x
        wr_i16(&mut rdram, addr + 2, 480); // vscale.y
        wr_i16(&mut rdram, addr + 4, 511); // vscale.z (~127.75 depth)
        wr_i16(&mut rdram, addr + 8, 640); // vtrans.x
        wr_i16(&mut rdram, addr + 10, 480); // vtrans.y
        wr_i16(&mut rdram, addr + 12, 511); // vtrans.z
        let vp = read_viewport(&rdram, addr).expect("viewport in bounds");
        assert_eq!(vp.sx, 160.0);
        assert_eq!(vp.sy, 120.0);
        assert_eq!(vp.tx, 160.0);
        assert_eq!(vp.ty, 120.0);
        assert_eq!(vp.sz, 127.75);
    }

    #[test]
    fn viewport_maps_known_ndc_points_to_known_pixels() {
        // A 320×240 centered viewport (sx=160, tx=160, sy=120, ty=120).
        // Map the NDC corners the way `project_vertex` does (with the Y-flip).
        let vp = Viewport {
            sx: 160.0,
            sy: 120.0,
            sz: 127.75,
            tx: 160.0,
            ty: 120.0,
            tz: 127.75,
        };
        // NDC origin (0,0) -> screen center (160,120).
        let map = |nx: f32, ny: f32| (nx * vp.sx + vp.tx, -ny * vp.sy + vp.ty);
        assert_eq!(map(0.0, 0.0), (160.0, 120.0));
        // NDC (-1,+1) is top-left on screen after the Y-flip: (0, 0).
        assert_eq!(map(-1.0, 1.0), (0.0, 0.0));
        // NDC (+1,-1) is bottom-right: (320, 240).
        assert_eq!(map(1.0, -1.0), (320.0, 240.0));
    }

    // --- Culling (priority 2) -------------------------------------------

    #[test]
    fn cull_mode_from_geometry_mode_bits() {
        assert_eq!(cull_mode_from(0), CullMode::None);
        assert_eq!(cull_mode_from(G_CULL_BACK), CullMode::Back);
        assert_eq!(cull_mode_from(G_CULL_FRONT), CullMode::Front);
        assert_eq!(cull_mode_from(G_CULL_FRONT | G_CULL_BACK), CullMode::Both);
        // Unrelated bits (e.g. G_SHADE=0x4, G_ZBUFFER=0x1) don't cull.
        assert_eq!(cull_mode_from(0x0000_0005), CullMode::None);
    }

    // --- Vertex lighting (priority 3) -----------------------------------

    /// Write a byte at logical offset `off` through the recomp `^3` swizzle
    /// (mirrors `read_u8`'s memory model), so tests plant Light_t/Vtx bytes
    /// the way a real DMA would.
    fn wr_u8(rdram: &mut [u8], off: usize, v: u8) {
        rdram[off ^ 3] = v;
    }

    /// A `DecodeState` with identity modelview and no MVP -- the minimal
    /// harness for exercising the light math directly.
    fn lit_state() -> DecodeState {
        DecodeState {
            vtx_cache: [Vertex::default(); 32],
            tris: Vec::new(),
            segments: [0u32; 16],
            mvp: None,
            proj: None,
            modelview: identity(),
            mv_stack: Vec::new(),
            viewport: None,
            scissor: None,
            geometry_mode: 0,
            other_mode: OtherMode::default(),
            combiner: CombinerState::default(),
            blend_color: [0; 4],
            fog_color: [0; 4],
            dl_depth: 0,
            cmds_decoded: 0,
            tex: TexState::default(),
            lights: LightState::default(),
        }
    }

    #[test]
    fn num_lights_from_moveword_divides_by_24() {
        // gsSPNumLights writes NUML(n) = n*24; num_dir = data/24.
        let mut st = lit_state();
        // 2 directional lights: data = 48.
        st.lights.num_dir = (48u32 / 24) as usize;
        assert_eq!(st.lights.num_dir, 2);
    }

    #[test]
    fn movemem_light_1_maps_to_directional_slot_zero() {
        // Fail-against-bug wire evidence: gSPLight(LIGHT_1) encodes
        // (1*24 + 24)/8 = 6. The old `ofs/3 - 1` mapping returned slot 1,
        // leaving the real first directional light (slot 0) black/stale and
        // misclassifying LIGHT_1 as ambient when num_dir == 1.
        assert_eq!(light_slot_from_movemem_offset(6), Some(0));
        // LIGHT_2 is the ambient slot when one directional light is active.
        assert_eq!(light_slot_from_movemem_offset(9), Some(1));
        // Offsets for the two look-at vectors are not light slots.
        assert_eq!(light_slot_from_movemem_offset(0), None);
        assert_eq!(light_slot_from_movemem_offset(3), None);
    }

    #[test]
    fn load_light_decodes_color_and_signed_direction() {
        // Light_t: col[3] u8 @0..3, dir[3] s8 @8..11. Plant a red light
        // pointing along -Z (dir byte 0x81 == -127 -> ~-1.0 after /127).
        let mut rdram = vec![0u8; 64];
        let addr = 0x10;
        wr_u8(&mut rdram, addr, 255); // col.r
        wr_u8(&mut rdram, addr + 1, 0); // col.g
        wr_u8(&mut rdram, addr + 2, 0); // col.b
        wr_u8(&mut rdram, addr + 8, 0); // dir.x
        wr_u8(&mut rdram, addr + 9, 0); // dir.y
        wr_u8(&mut rdram, addr + 10, 0x81); // dir.z = -127
        let mut st = lit_state();
        st.lights.num_dir = 1; // slot 0 is directional here
        load_light(&rdram, &mut st, addr, 0);
        let l = st.lights.dir[0];
        assert_eq!(l.col, [1.0, 0.0, 0.0]);
        assert!((l.dir[2] - (-127.0 / 127.0)).abs() < 1e-6);
        assert_eq!(l.dir[0], 0.0);
    }

    #[test]
    fn light_vertex_face_on_light_is_full_diffuse_plus_ambient() {
        // One white directional light pointing at the surface normal (+Z),
        // plus a dim gray ambient. A normal facing the light (+Z) gets full
        // N·L=1 -> ambient + light color, clamped.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.1, 0.1, 0.1];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [0.8, 0.8, 0.8],
        };
        // Normal directly toward the light: N·L = 1.
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        // 0.1 + 1.0*0.8 = 0.9 -> 229.
        assert_eq!(c, [229, 229, 229]);
    }

    #[test]
    fn light_vertex_back_face_gets_ambient_only() {
        // A normal facing AWAY from the light (N·L < 0, clamped to 0) is lit
        // by ambient alone -- the diffuse term must not go negative (that
        // was the failure mode a naive dot without a max(.,0) would hit).
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.2, 0.2, 0.2];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [1.0, 1.0, 1.0],
        };
        // Normal pointing away from the +Z light.
        let c = light_vertex(&st, [0.0, 0.0, -1.0]);
        assert_eq!(c, [51, 51, 51]); // 0.2*255 = 51, no negative diffuse.
    }

    #[test]
    fn light_vertex_is_not_the_raw_normal_bytes() {
        // Fail-against-bug: the OLD path read the s8 normal bytes AS a flat
        // color. A normal of (0,0,+1) with a green light must NOT come out as
        // the raw normal-as-color (which would be ~[0,0,255] from cn bytes);
        // it must be the LIT color (green from the light). This is exactly the
        // "rainbow fan" bug: signed normals misread as unsigned color.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [0.0, 1.0, 0.0], // green
        };
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c, [0, 255, 0]); // green, from the LIGHT -- not the normal.
    }

    #[test]
    fn light_vertex_half_angle_scales_diffuse() {
        // A 45° normal to a +Z light: N·L = cos(45°) ≈ 0.707, so a white
        // light yields ~0.707 -> ~180 (screen-linear, no gamma).
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [1.0, 1.0, 1.0],
        };
        let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
        let c = light_vertex(&st, [inv_sqrt2, 0.0, inv_sqrt2]);
        // 0.707 * 255 ≈ 180.
        assert!((c[0] as i32 - 180).abs() <= 1, "got {}", c[0]);
    }

    #[test]
    fn light_vertex_modelview_rotates_light_into_local_space() {
        // computeDirLight brings the light dir into local space via the
        // modelview. With a 90° rotation about Y, a light along +X ends up
        // along the axis a +Z-facing normal is lit by. Concretely: rotate the
        // world +X light so it aligns with the vertex normal's frame, giving
        // full N·L where an unrotated dot would give 0.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [1.0, 0.0, 0.0], // light along world +X
            col: [1.0, 1.0, 1.0],
        };
        // modelview that rotates +X -> +Z under rotate_dir (row-major,
        // column-vector): out.z = m[2][0]*x. Set m[2][0]=1, m[0][0]=0.
        let mut mv = identity();
        mv[0][0] = 0.0;
        mv[2][0] = 1.0;
        mv[2][2] = 0.0;
        st.modelview = mv;
        // Normal along +Z now sees the rotated light head-on.
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c, [255, 255, 255]);
        // Sanity: WITHOUT the rotation (identity), the +X light and +Z normal
        // are orthogonal -> no diffuse.
        st.modelview = identity();
        let c0 = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c0, [0, 0, 0]);
    }

    // --- Near-plane culling (the "fan from a point" fix) ----------------

    fn vtx_w(w: f32) -> Vertex {
        Vertex {
            w,
            ..Default::default()
        }
    }

    #[test]
    fn behind_near_plane_flags_nonpositive_w() {
        assert!(behind_near_plane(&vtx_w(-1.0)), "w<0 is behind camera");
        assert!(
            behind_near_plane(&vtx_w(0.0)),
            "w==0 is on the camera plane"
        );
        assert!(!behind_near_plane(&vtx_w(1.0)), "w>0 is in front");
    }

    #[test]
    fn resolve_tri_drops_triangle_with_a_behind_camera_vertex() {
        // Fail-against-bug: a triangle with one vertex at w<=0 is the "fan
        // from a point" artifact (projecting it flings it across the screen).
        // resolve_tri must DROP it, not emit a giant wrong-side polygon.
        let mut cache = [Vertex::default(); 32];
        cache[0] = vtx_w(1.0);
        cache[1] = vtx_w(1.0);
        cache[2] = vtx_w(-0.5); // behind the near plane
        assert!(
            resolve_tri(
                &cache,
                [0, 1, 2],
                CullMode::None,
                None,
                OtherMode::default(),
                CombinerState::default(),
                BlenderState::default(),
            )
            .is_none(),
            "triangle touching a behind-camera vertex must be dropped"
        );
        // All three in front -> kept.
        cache[2] = vtx_w(2.0);
        assert!(resolve_tri(
            &cache,
            [0, 1, 2],
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_some());
    }

    // --- Texture sampling (priority 4) ----------------------------------

    /// Build a 2×2 RGBA8888 texture: TL=red, TR=green, BL=blue, BR=white.
    fn checker_2x2(clamp: bool) -> Texture {
        let texels = vec![
            255, 0, 0, 255, // (0,0) red
            0, 255, 0, 255, // (1,0) green
            0, 0, 255, 255, // (0,1) blue
            255, 255, 255, 255, // (1,1) white
        ];
        Texture {
            width: 2,
            height: 2,
            texels: std::rc::Rc::new(texels),
            clamp_s: clamp,
            clamp_t: clamp,
            origin_s: 0.0,
            origin_t: 0.0,
        }
    }

    #[test]
    fn texture_samples_the_right_texel() {
        let tex = checker_2x2(true);
        // Each integer texel coordinate lands on its own texel (nearest).
        assert_eq!(tex.sample(0.0, 0.0), [255, 0, 0, 255]); // TL red
        assert_eq!(tex.sample(1.0, 0.0), [0, 255, 0, 255]); // TR green
        assert_eq!(tex.sample(0.0, 1.0), [0, 0, 255, 255]); // BL blue
        assert_eq!(tex.sample(1.0, 1.0), [255, 255, 255, 255]); // BR white

        // Fractional coords floor to the containing texel.
        assert_eq!(tex.sample(0.9, 0.1), [255, 0, 0, 255]); // floor -> (0,0) red
    }

    #[test]
    fn texture_sample_floor_addressing() {
        let tex = checker_2x2(true);
        // (1.5, 0.9) floors to (1, 0) = green.
        assert_eq!(tex.sample(1.5, 0.9), [0, 255, 0, 255]);
        // (0.2, 1.7) floors to (0, 1) = blue.
        assert_eq!(tex.sample(0.2, 1.7), [0, 0, 255, 255]);
    }

    #[test]
    fn texture_clamp_vs_wrap_addressing() {
        let clamp = checker_2x2(true);
        // Out-of-range clamps to the edge texel.
        assert_eq!(clamp.sample(5.0, 0.0), [0, 255, 0, 255]); // clamp to x=1 green
        assert_eq!(clamp.sample(-3.0, 1.0), [0, 0, 255, 255]); // clamp to x=0 blue

        let wrap = checker_2x2(false);
        // Wrap repeats: s=2 -> texel 0, s=3 -> texel 1, s=-1 -> texel 1.
        assert_eq!(wrap.sample(2.0, 0.0), [255, 0, 0, 255]); // (0,0) red
        assert_eq!(wrap.sample(3.0, 0.0), [0, 255, 0, 255]); // (1,0) green
        assert_eq!(wrap.sample(-1.0, 0.0), [0, 255, 0, 255]); // wraps to (1,0)
    }

    #[test]
    fn rgba5551_expands_high_bits() {
        // Pure red (R5=0x1F) -> R8=0xFF; alpha bit set -> 0xFF.
        assert_eq!(rgba5551_to_rgba8888(0xF801), [255, 0, 0, 255]);
        // Pure green (G5=0x1F at bits 6..10).
        assert_eq!(rgba5551_to_rgba8888(0x07C1), [0, 255, 0, 255]);
        // Black, alpha 0.
        assert_eq!(rgba5551_to_rgba8888(0x0000), [0, 0, 0, 0]);
    }

    #[test]
    fn load_tlut_count_uses_all_ten_wire_bits() {
        // Public gbi.h encodes `count - 1` directly, without quarter-texel
        // scaling. Discarding the low two bits turns the normal 256-entry CI8
        // palette into 64 entries.
        assert_eq!(load_tlut_count(255 << 14), 256);
        assert_eq!(load_tlut_count(15 << 14), 16);
    }

    #[test]
    fn load_tile_uses_settimg_stride_and_tile_coordinate_origin() {
        // A synthetic 4x2 CI8 source. Load the rightmost two texels of row 1
        // as a 2x1 tile whose render coordinates begin at (2, 1).
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + 12];
        for (i, index) in (0u8..8).enumerate() {
            wr_u8(&mut rdram, base + i, index);
        }
        let mut tlut = vec![[0, 0, 0, 255]; 8];
        tlut[6] = [60, 61, 62, 255];
        tlut[7] = [70, 71, 72, 255];
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_width: 4,
            tlut,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_CI,
            siz: G_IM_SIZ_8B,
            uls: 2 * 4,
            ult: 4,
            lrs: 3 * 4,
            lrt: 4,
            clamp_s: true,
            clamp_t: true,
            ..Default::default()
        };

        let decoded = decode_current_texture(
            &rdram,
            &tex,
            &[0; 16],
            0,
            TextureLoad::Tile {
                source_x: 2,
                source_y: 1,
            },
        )
        .expect("CI8 tile must decode");

        assert_eq!(
            decoded.texels.as_slice(),
            &[60, 61, 62, 255, 70, 71, 72, 255]
        );
        assert_eq!(decoded.sample(2.0, 1.0), [60, 61, 62, 255]);
        assert_eq!(decoded.sample(3.0, 1.0), [70, 71, 72, 255]);
    }

    fn assert_texture_row(
        bytes: &[u8],
        width: u16,
        fmt: u8,
        siz: u8,
        palette: u8,
        tlut: Vec<[u8; 4]>,
        expected: &[u8],
    ) {
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + bytes.len() + 4];
        for (i, &byte) in bytes.iter().enumerate() {
            wr_u8(&mut rdram, base + i, byte);
        }

        let mut tex = TexState {
            timg_addr: base as u32,
            tlut,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt,
            siz,
            palette,
            lrs: (width - 1) * 4,
            ..Default::default()
        };
        assert_eq!(
            decode_current_texture(&rdram, &tex, &[0; 16], 0, TextureLoad::Block)
                .expect("OoT-used texture format must decode")
                .texels
                .as_slice(),
            expected
        );
    }

    #[test]
    fn decode_rgba16_covers_low_channels_and_alpha_edges() {
        // 0x0001 = opaque black; 0xffff = opaque white; 0x0842 has the
        // lowest nonzero R/G/B codes and clear alpha. This catches both a
        // dropped 1-bit alpha and incorrect 5-to-8 scaling at the low edge.
        assert_texture_row(
            &[0x00, 0x01, 0xff, 0xff, 0x08, 0x42],
            3,
            G_IM_FMT_RGBA,
            G_IM_SIZ_16B,
            0,
            Vec::new(),
            &[0, 0, 0, 255, 255, 255, 255, 255, 8, 8, 8, 0],
        );
    }

    #[test]
    fn decode_rgba8_uses_observed_hardware_i8_alias() {
        // Fail-against-bug: this pair previously fell through to None and
        // left the surface flat. RT64 records that hardware samples it as I8.
        assert_texture_row(
            &[0x24, 0xdb],
            2,
            G_IM_FMT_RGBA,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0x24, 0x24, 0x24, 0x24, 0xdb, 0xdb, 0xdb, 0xdb],
        );
    }

    #[test]
    fn decode_rgba4_uses_observed_hardware_i4_alias() {
        // Fail-against-bug and live-OoT case: RGBA4 was one of the `_ =>
        // None` combinations, so every such tile remained flat-shaded.
        assert_texture_row(
            &[0x39],
            2,
            G_IM_FMT_RGBA,
            G_IM_SIZ_4B,
            0,
            Vec::new(),
            &[0x33, 0x33, 0x33, 0x33, 0x99, 0x99, 0x99, 0x99],
        );
    }

    #[test]
    fn decode_ia8_splits_four_bit_intensity_and_alpha() {
        assert_texture_row(
            &[0x1e, 0xf0],
            2,
            G_IM_FMT_IA,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0x11, 0x11, 0x11, 0xee, 0xff, 0xff, 0xff, 0x00],
        );
    }

    #[test]
    fn decode_ia4_is_three_bit_intensity_plus_one_bit_alpha() {
        // Fail-against-bug: the old shared I4/IA4 arm expanded the whole
        // nibble into every channel. In particular 0x1 became translucent
        // dark gray and 0xe became opaque light gray. IA4 requires those to
        // be opaque black and transparent white respectively.
        assert_texture_row(
            &[0x1e, 0xa7],
            4,
            G_IM_FMT_IA,
            G_IM_SIZ_4B,
            0,
            Vec::new(),
            &[
                0, 0, 0, 255, // 0x1: I=0, A=1
                255, 255, 255, 0, // 0xe: I=7, A=0
                182, 182, 182, 0, // 0xa: I=5, A=0
                109, 109, 109, 255, // 0x7: I=3, A=1
            ],
        );
    }

    #[test]
    fn decode_i8_replicates_intensity_into_rgba() {
        assert_texture_row(
            &[0x00, 0x7f, 0xff],
            3,
            G_IM_FMT_I,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0, 0, 0, 0, 0x7f, 0x7f, 0x7f, 0x7f, 0xff, 0xff, 0xff, 0xff],
        );
    }

    #[test]
    fn decode_ci8_uses_full_byte_as_rgba16_tlut_index() {
        let mut tlut = vec![[0, 0, 0, 0]; 256];
        tlut[0] = [1, 2, 3, 4];
        tlut[0x7f] = [5, 6, 7, 8];
        tlut[0xff] = [9, 10, 11, 12];
        assert_texture_row(
            &[0x00, 0x7f, 0xff],
            3,
            G_IM_FMT_CI,
            G_IM_SIZ_8B,
            0,
            tlut,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        );
    }

    #[test]
    fn decode_ci4_combines_palette_bank_with_each_nibble() {
        let mut tlut = vec![[0, 0, 0, 0]; 0x30];
        tlut[0x21] = [1, 3, 5, 7];
        tlut[0x2f] = [2, 4, 6, 8];
        assert_texture_row(
            &[0x1f],
            2,
            G_IM_FMT_CI,
            G_IM_SIZ_4B,
            2,
            tlut,
            &[1, 3, 5, 7, 2, 4, 6, 8],
        );
    }

    #[test]
    fn decode_ci4_pal16_load_uses_palette_local_indices() {
        // Fail-against-bug: G_LOADTLUT stores a 16-entry pal16 load in this
        // decoder as entries 0..15. The old CI4 arm added palette<<4 again,
        // indexed past this Vec for every nonzero bank, and returned magenta.
        let mut tlut = vec![[0, 0, 0, 0]; 16];
        tlut[1] = [11, 22, 33, 44];
        tlut[15] = [55, 66, 77, 88];
        assert_texture_row(
            &[0x1f],
            2,
            G_IM_FMT_CI,
            G_IM_SIZ_4B,
            2,
            tlut,
            &[11, 22, 33, 44, 55, 66, 77, 88],
        );
    }

    // --- Projection w-sign regression (the "giant triangles from a point"
    //     bug): the MVP must be applied as a ROW vector, not a column vector.

    /// Column-vector application of an asymmetric perspective·modelview MVP
    /// (`mvp · v`) computes the TRANSPOSE of the true transform and produces a
    /// huge, sign-flipping `w` -- the projection bug. `transform_point` must do
    /// the row-vector product (`v · mvp`) so `w ≈ -z_eye`, a sane depth.
    ///
    /// Matrices are the LIVE OoT gameplay task dump (decoded via `read_mtx`):
    /// perspective P with the projective term in row 2 col 3, and a modelview
    /// translation of (-53, -5, 0). Cited in `transform_point`'s doc comment.
    #[test]
    fn transform_point_row_vector_gives_sane_perspective_w() {
        // guPerspective output P (hardware [row][col], no transpose):
        let p: Mat4 = [
            [2.7990265, 0.0, 0.0, 0.0],
            [0.0, 3.7320404, 0.0, 0.0],
            [0.0, 0.0, -1.0015564, -1.0],
            [0.0, 0.0, -20.015625, 0.0],
        ];
        // Modelview: pure translation by (-53, -5, 0) (4th ROW, N64 layout).
        let mut m: Mat4 = identity();
        m[3][0] = -53.0;
        m[3][1] = -5.0;
        // mvp = modelview * (view*proj); here view is folded in, so M * P.
        let mvp = mat_mul(&m, &p);

        // Two object-space vertices of one small object (ob magnitudes ~10).
        // Under a correct row-vector transform their `w` is the SAME sane
        // depth (both at eye-z = -5 after the translate -> w = -z_eye = 5),
        // NOT the ±thousands sign-flipping garbage the transpose produced.
        for &(x, y, z) in &[(11.0, 0.0, -5.0), (5.0, 0.0, -5.0)] {
            let clip = transform_point(&mvp, x, y, z);
            let w = clip[3];
            // The true perspective depth for these verts is w = 5.0.
            assert!(
                (w - 5.0).abs() < 1e-3,
                "row-vector w should be the sane depth 5.0, got {w}"
            );
            assert!(w.abs() < 1e3, "w must be a small sane depth, got {w}");
        }

        // Guard against a regression to the column-vector form: assert the
        // BUGGED product (`mvp · v`, the old code) really did explode `w`.
        // This documents the failure mode so a reviewer sees the bug is real.
        let col_vec_w = {
            let v = [11.0f32, 0.0, -5.0, 1.0];
            // out[r] = sum_k mvp[r][k] * v[k] -- the OLD column-vector product.
            let mut s = 0.0;
            for k in 0..4 {
                s += mvp[3][k] * v[k];
            }
            s
        };
        assert!(
            col_vec_w.abs() > 1e3,
            "the column-vector (transposed) apply must produce the pathological \
             large w this test guards against; got {col_vec_w}"
        );
        // And it flips sign vs the second vertex (the "fan" signature).
        let col_vec_w2 = {
            let v = [5.0f32, 0.0, -5.0, 1.0];
            let mut s = 0.0;
            for k in 0..4 {
                s += mvp[3][k] * v[k];
            }
            s
        };
        assert!(
            col_vec_w.signum() == col_vec_w2.signum() && col_vec_w != col_vec_w2,
            "column-vector w varies wildly with x (the bug); w1={col_vec_w} w2={col_vec_w2}"
        );
    }

    /// A symmetric/diagonal matrix (all the reference fixtures) is unchanged
    /// by the row-vs-column swap (`m == m^T`), so the fix is transparent to
    /// the byte-exact goldens.
    #[test]
    fn transform_point_symmetric_matrix_unaffected_by_convention() {
        let mut m: Mat4 = identity();
        m[0][0] = 2.0;
        m[1][1] = 3.0;
        m[2][2] = 4.0;
        m[3][3] = 1.0;
        let clip = transform_point(&m, 5.0, 7.0, 9.0);
        assert_eq!(clip, [10.0, 21.0, 36.0, 1.0]);
    }

    /// Regression for the exact fixed-point `guLookAt` matrix observed in the
    /// Hyrule Field title-camera task. The writer trace establishes that
    /// `guLookAtF` receives eye `(-4000,-1,5228)`; its translation therefore
    /// is `(3263,694,5675) = -(eye · basis)`. Those translation values are
    /// camera-space coordinates of the world origin, not the world-space eye.
    ///
    /// Replacing them with `-translation · basis` (the discarded diagnostic
    /// transform) moves the camera to a different world-space eye. This test
    /// fails under that rewrite because the traced eye no longer maps to the
    /// view-space origin.
    #[test]
    fn hyrule_field_live_gu_look_at_translation_matches_traced_eye() {
        // Decoded from the 64-byte Mtx written at physical 0x1888c8. The
        // fixed-point quantization accounts for the small origin tolerance.
        let view: Mat4 = [
            [-0.3885498, 0.11167908, 0.9146271, 0.0],
            [-1.5258789e-5, 0.99261475, -0.12121582, 0.0],
            [-0.92141724, -0.04710388, -0.38568115, 0.0],
            [3262.9912, 694.052, 5674.783, 1.0],
        ];
        let eye = [-4000.0, -1.0, 5228.0];

        for (c, (((&basis_x, &basis_y), &basis_z), &translation)) in view[0]
            .iter()
            .zip(view[1].iter())
            .zip(view[2].iter())
            .zip(view[3].iter())
            .take(3)
            .enumerate()
        {
            let expected_translation = -(eye[0] * basis_x + eye[1] * basis_y + eye[2] * basis_z);
            assert!(
                (translation - expected_translation).abs() < 0.1,
                "translation[{c}] must be -(eye · basis[{c}]): got {}, expected {expected_translation}",
                translation
            );
        }

        let eye_in_view = transform_point(&view, eye[0], eye[1], eye[2]);
        for (axis, value) in eye_in_view[..3].iter().enumerate() {
            assert!(
                value.abs() < 0.1,
                "traced eye must map to the view-space origin; axis {axis} was {value}"
            );
        }
        assert!((eye_in_view[3] - 1.0).abs() < f32::EPSILON);
    }

    // --- Synthetic large-world projection regression ---------------------
    //
    // This synthetic scene has a camera at world ~(3000,700,5600) and an
    // object translated to ~-4000, so both sides carry LARGE world
    // coordinates. It drives the full decode path -- fixed-point `Mtx`
    // bytes (`read_mtx`) -> projection LOAD(persp) then PROJECTION|MUL(view)
    // -> modelview LOAD -> `recompute_mvp` (`M · (V · P)`) -> row-vector
    // `transform_point` -> the default 320x240 viewport map -- for the exact
    // large-world matrix shapes and asserts every vertex lands in-frustum
    // with a sane POSITIVE `w` (~ -z_eye ~= +7000), never the negative /
    // sign-flipping `w` and ±4000 screen-z of the mis-projection.
    //
    // The synthetic view is a proper `guLookAt` matrix: its translation row is
    // `-(eye · basis)` = (5419.7, -367.3, -3367.7), NOT the raw eye. That is
    // the load-bearing distinction -- feed the raw eye (3000,700,5600) into
    // row 3 instead and the origin vertex flips to `w = -1921` (behind the
    // camera). This asserts the decode+compose of a correct synthetic
    // large-world view/model/perspective chain.
    //
    // It fails against the historical transpose bug too: a re-introduced
    // `Mtx` transpose-on-read or a column-vector apply turns the asymmetric
    // large-world MVP into its transpose and collapses `w` to garbage.
    #[test]
    fn large_world_perspective_view_model_projects_in_frustum() {
        let mut rdram = vec![0u8; 0x8000];

        // guPerspective(fovy=60, aspect=4/3, near=100, far=12800), hardware
        // [row][col]: projective term [2][3]=-1, depth translate [3][2].
        let persp = [
            [1.299_038, 0.0, 0.0, 0.0],
            [0.0, 1.7320508, 0.0, 0.0],
            [0.0, 0.0, -1.015_748, -1.0],
            [0.0, 0.0, -201.574_8, 0.0],
        ];
        // PROPER guLookAt view: 3x3 = camera basis (right/up/look as columns),
        // translation ROW = -(eye · basis). Eye world ~(3000,700,5600) looking
        // toward (-4000,0,5200). (Raw eye in row 3 would be the bug.)
        let view = [
            [0.05704979, -0.09918146, 0.993_432_6, 0.0],
            [0.0, 0.99505322, 0.09934326, 0.0],
            [-0.998_371_3, -0.00566751, 0.05676758, 0.0],
            [5_419.73, -367.254_8, -3367.7366, 1.0],
        ];
        // Large-world object modelview: rot(15° about Y) then translate to
        // world (-4000, 0, 5200) -- asymmetric so `mvp != mvp^T`.
        let model = [
            [0.965_925_8, 0.0, -0.25881905, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.25881905, 0.0, 0.965_925_8, 0.0],
            [-4000.0, 0.0, 5200.0, 1.0],
        ];

        // Object-space vertices (small, ob magnitudes ~50).
        wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 50, 30, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, -50, 0, 40, [0, 0, 255, 255]);

        wr_mtx(&mut rdram, 0x2000, persp);
        wr_mtx(&mut rdram, 0x2100, view);
        wr_mtx(&mut rdram, 0x2200, model);

        // G_MTX wire = params ^ G_MTX_PUSH:
        //   persp PROJECTION|LOAD        = 0x06 -> wire 0x07
        //   view  PROJECTION|MUL(NOPUSH) = 0x04 -> wire 0x05
        //   model LOAD (modelview)       = 0x02 -> wire 0x03
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x05), 0x2100); // view PROJECTION|MUL
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x03), 0x2200); // model LOAD
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1, "expected one transformed triangle");

        // Every vertex must project to a sane POSITIVE depth ~7000 and land
        // inside the default 320x240 screen ([0,320] x [0,240]). The origin
        // vertex maps to screen center (~160, 120). The mis-projection gave
        // negative `w` and NDC well outside [-1,1] (pz swinging ±4000).
        for (i, v) in tris[0].v.iter().enumerate() {
            assert!(
                v.w > 1.0,
                "large-world vertex {i} must have a sane positive clip-w \
                 (~7000, = -z_eye), got w={} (negative/tiny w is the \
                 mis-projection this test guards)",
                v.w
            );
            assert!(
                (5000.0..9000.0).contains(&v.w),
                "large-world clip-w must be the coherent perspective depth \
                 (~7000), got w={} -- a decode transpose / wrong MVP order \
                 turns it into garbage",
                v.w
            );
            assert!(
                (0.0..=320.0).contains(&v.x) && (0.0..=240.0).contains(&v.y),
                "large-world vertex {i} must land inside the 320x240 screen, \
                 got ({}, {}) -- out-of-frustum is the ±4000 pz mis-projection",
                v.x,
                v.y
            );
        }
        // Origin vertex at screen center is the crisp anchor.
        let v0 = &tris[0].v[0];
        assert!(
            (v0.x - 160.0).abs() < 1.0 && (v0.y - 120.0).abs() < 1.0,
            "object-origin vertex must land at screen center (~160, ~120) \
             under the correct large-world MVP; got ({}, {})",
            v0.x,
            v0.y
        );
    }
}
