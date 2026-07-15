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
//! display list), `G_ENDDL` (stop).
//!
//! Explicitly acknowledged-and-skipped (logged by name via `skip_opcode`,
//! never a silent no-op): texture/combiner/other-mode/sync/geometry-mode
//! ops and any unrecognized byte. A skipped state op means the geometry is
//! flat-shaded from vertex color only (textures/lighting are NOT applied) --
//! this is a seam/first-image proof, not RDP fidelity (see `lib.rs` module
//! doc). Skips are rate-limited-per-opcode so a real DL doesn't spam
//! thousands of identical lines, but every distinct skipped opcode is
//! reported at least once.
use fn64_render::{RenderError, UcodeId};
use std::cell::RefCell;
use std::collections::HashSet;

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
const G_SETSCISSOR: u8 = 0xED;
const G_LOADTLUT: u8 = 0xF0;
const G_SETTILESIZE: u8 = 0xF2;
const G_LOADBLOCK: u8 = 0xF3;
const G_LOADTILE: u8 = 0xF4;
const G_SETTILE: u8 = 0xF5;
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
        let x = wrap(s, self.width, self.clamp_s);
        let y = wrap(t, self.height, self.clamp_t);
        let o = ((y * self.width + x) * 4) as usize;
        if o + 4 <= self.texels.len() {
            [
                self.texels[o],
                self.texels[o + 1],
                self.texels[o + 2],
                self.texels[o + 3],
            ]
        } else {
            [255, 0, 255, 255] // out-of-range guard: magenta (never expected).
        }
    }
}

/// A decoded, screen-space-ready triangle (three already-resolved
/// vertices) -- the display-list decoder's actual output, consumed by the
/// rasterizer in `raster.rs`.
#[derive(Clone, Debug, Default)]
pub struct Triangle {
    pub v: [Vertex; 3],
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
    for i in 0..4 {
        m[i][i] = 1.0;
    }
    m
}

fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            out[r][c] = s;
        }
    }
    out
}

/// Transform a homogeneous point (x,y,z,1) by `m` (m row-major, point as a
/// column vector: `out_row = sum_k m[row][k] * v[k]`).
fn transform_point(m: &Mat4, x: f32, y: f32, z: f32) -> [f32; 4] {
    let v = [x, y, z, 1.0];
    let mut out = [0.0f32; 4];
    for r in 0..4 {
        let mut s = 0.0;
        for k in 0..4 {
            s += m[r][k] * v[k];
        }
        out[r] = s;
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
/// The N64 stores matrices column-major relative to how it multiplies
/// `vertex * matrix` (row-vector convention). We read it into `m[row][col]`
/// and treat the vertex as a COLUMN vector; because the on-chip convention
/// is row-vector `v*M`, the equivalent column-vector product is `M^T * v`.
/// We therefore transpose on read so `transform_point` (column-vector) gives
/// the same result the hardware's `v*M` would.
fn read_mtx(rdram: &[u8], addr: usize) -> Option<Mat4> {
    if addr + 64 > rdram.len() {
        return None;
    }
    let mut m = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            let elem = r * 4 + c;
            let int_off = addr + elem * 2;
            let frac_off = addr + 32 + elem * 2;
            // Swizzled halfword reads (recomp MEM_H): the Mtx was DMA'd from
            // ROM through the same `^3` per-byte swizzle as everything else.
            let int_part = read_i16(rdram, int_off) as i32;
            let frac_part = read_u16(rdram, frac_off) as i32;
            let value = (((int_part << 16) | frac_part) as f32) / 65536.0;
            // Transpose on read (row-vector hardware convention -> our
            // column-vector `transform_point`): store element (r,c) at
            // m[c][r].
            m[c][r] = value;
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
    Some(Viewport {
        sx: vscale_x / 4.0,
        sy: vscale_y / 4.0,
        sz: vscale_z / 4.0,
        tx: vtrans_x / 4.0,
        ty: vtrans_y / 4.0,
        tz: vtrans_z / 4.0,
    })
}

// --- Texture format decode (F3DEX2-CONCEPTS.md §5.1) --------------------

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

/// Expand a 16-bit RGBA5551 texel to RGBA8888 (5/5/5/1, big-endian --
/// `pixel16 = R5<<11 | G5<<6 | B5<<1 | A1`, `F3DEX2-CONCEPTS.md` §4.4).
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

/// Expand an IA16 texel (8-bit intensity, 8-bit alpha) to RGBA8888.
#[inline]
fn ia16_to_rgba8888(hi: u8, lo: u8) -> [u8; 4] {
    [hi, hi, hi, lo]
}

/// Expand an IA8 texel (4-bit intensity, 4-bit alpha) to RGBA8888.
#[inline]
fn ia8_to_rgba8888(byte: u8) -> [u8; 4] {
    let i4 = byte >> 4;
    let a4 = byte & 0x0F;
    let i = (i4 << 4) | i4;
    let a = (a4 << 4) | a4;
    [i, i, i, a]
}

/// Expand an I8 texel (8-bit intensity; alpha = intensity) to RGBA8888.
#[inline]
fn i8_to_rgba8888(byte: u8) -> [u8; 4] {
    [byte, byte, byte, byte]
}

/// Decode the texture bound to `tile` from the latched `G_SETTIMG` image out
/// of RDRAM into an RGBA8888 [`Texture`], sized by the tile's
/// `G_SETTILESIZE` extent. Returns `None` for an unsupported/zero-size
/// format so the caller leaves the triangle flat-shaded rather than binding
/// garbage. Covers the common OoT formats: RGBA16/32, IA16/IA8, I8/I4,
/// CI8/CI4 (via the loaded TLUT).
///
/// This is deliberately NOT a byte-exact 4 KiB TMEM model. A first
/// recognizable textured frame needs the right texels addressed by the right
/// texcoords, not cycle-accurate TMEM tiling -- so we read the source image
/// linearly at `line`-implied width and let the sampler address it by the
/// interpolated S/T (`F3DEX2-CONCEPTS.md` §5.1, "to sample" bullet).
fn decode_current_texture(
    rdram: &[u8],
    tex: &TexState,
    segments: &[u32; 16],
    tile: usize,
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

    for ty in 0..h {
        for tx in 0..w {
            let texel_index = (ty * w + tx) as usize;
            let rgba = match (fmt, siz) {
                (G_IM_FMT_RGBA, G_IM_SIZ_16B) => {
                    let px = read_u16(rdram, base + texel_index * 2);
                    rgba5551_to_rgba8888(px)
                }
                (G_IM_FMT_RGBA, G_IM_SIZ_32B) => {
                    let o = base + texel_index * 4;
                    [
                        read_u8(rdram, o),
                        read_u8(rdram, o + 1),
                        read_u8(rdram, o + 2),
                        read_u8(rdram, o + 3),
                    ]
                }
                (G_IM_FMT_IA, G_IM_SIZ_16B) => {
                    let o = base + texel_index * 2;
                    ia16_to_rgba8888(read_u8(rdram, o), read_u8(rdram, o + 1))
                }
                (G_IM_FMT_IA, G_IM_SIZ_8B) => ia8_to_rgba8888(read_u8(rdram, base + texel_index)),
                (G_IM_FMT_I, G_IM_SIZ_8B) => i8_to_rgba8888(read_u8(rdram, base + texel_index)),
                (G_IM_FMT_I, G_IM_SIZ_4B) | (G_IM_FMT_IA, G_IM_SIZ_4B) => {
                    // 4-bit intensity: two texels per byte, high nibble first.
                    let byte = read_u8(rdram, base + texel_index / 2);
                    let nib = if texel_index & 1 == 0 {
                        byte >> 4
                    } else {
                        byte & 0x0F
                    };
                    let v = (nib << 4) | nib;
                    [v, v, v, v]
                }
                (G_IM_FMT_CI, G_IM_SIZ_8B) => {
                    let idx = read_u8(rdram, base + texel_index) as usize;
                    tex.tlut.get(idx).copied().unwrap_or([255, 0, 255, 255])
                }
                (G_IM_FMT_CI, G_IM_SIZ_4B) => {
                    let byte = read_u8(rdram, base + texel_index / 2);
                    let nib = if texel_index & 1 == 0 {
                        byte >> 4
                    } else {
                        byte & 0x0F
                    } as usize;
                    let idx = ((t.palette as usize) << 4) | nib;
                    tex.tlut.get(idx).copied().unwrap_or([255, 0, 255, 255])
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
    })
}

thread_local! {
    /// Per-opcode "already warned once" set, so a real display list with
    /// thousands of identical skipped state ops emits ONE loud line per
    /// distinct opcode rather than flooding the log. Thread-local (not a
    /// static Mutex) to stay lock-free and match the rest of this crate's
    /// single-threaded reference-backend model.
    static WARNED_SKIPS: RefCell<HashSet<u8>> = RefCell::new(HashSet::new());
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
        0xE2 => "G_SETOTHERMODE_L",
        0xE3 => "G_SETOTHERMODE_H",
        0xE6 => "G_RDPLOADSYNC",
        0xE7 => "G_RDPPIPESYNC",
        0xE8 => "G_RDPTILESYNC",
        0xE9 => "G_RDPFULLSYNC",
        G_LOADTLUT => "G_LOADTLUT",
        0xF1 => "G_RDPHALF_2",
        G_LOADBLOCK => "G_LOADBLOCK",
        G_LOADTILE => "G_LOADTILE",
        G_SETTILESIZE => "G_SETTILESIZE",
        G_SETTILE => "G_SETTILE",
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

/// Reset the once-per-opcode skip-warning memo. Called at the start of each
/// `decode_display_list` so a fresh frame re-reports its coverage (a
/// long-running harness otherwise only ever sees the first frame's skips).
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
    if off + 4 > rdram.len() {
        return 0;
    }
    // Native-endian read == the logical big-endian word the game stored
    // (recomp.h MEM_W is a bare `*(int32_t*)`, no swap).
    u32::from_ne_bytes([rdram[off], rdram[off + 1], rdram[off + 2], rdram[off + 3]])
}

/// Read a logical byte at byte offset `off` (recomp `MEM_BU`: physical
/// index `off ^ 3`). Returns 0 past the end.
#[inline]
fn read_u8(rdram: &[u8], off: usize) -> u8 {
    let p = off ^ 3;
    if p >= rdram.len() {
        return 0;
    }
    rdram[p]
}

/// Read a logical signed 16-bit halfword at byte offset `off` (recomp
/// `MEM_H`). The two logical bytes `off` (MSB) and `off+1` (LSB) are read
/// through the `^3` byte swizzle and recombined big-endian. Returns 0 past
/// the end.
#[inline]
fn read_i16(rdram: &[u8], off: usize) -> i16 {
    let hi = read_u8(rdram, off) as u16;
    let lo = read_u8(rdram, off + 1) as u16;
    ((hi << 8) | lo) as i16
}

/// Read a logical unsigned 16-bit halfword at byte offset `off`.
#[inline]
fn read_u16(rdram: &[u8], off: usize) -> u16 {
    let hi = read_u8(rdram, off) as u16;
    let lo = read_u8(rdram, off + 1) as u16;
    (hi << 8) | lo
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
    /// Current F3DEX2 geometry mode (the `G_GEOMETRYMODE` accumulator). Its
    /// `G_CULL_FRONT`/`G_CULL_BACK` bits decide per-triangle culling.
    geometry_mode: u32,
    dl_depth: u32,
    /// Texture-mapping decode state (SETTIMG image latch, tile descriptors,
    /// TLUT palette, G_TEXTURE enable/scale, and the currently-decoded
    /// texture bound to emitted triangles). See [`TexState`].
    tex: TexState,
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

/// Max `G_DL` recursion depth honored, matching the real F3DEX2 display-
/// list stack size (10 entries, public SDK `GBI` documentation) -- a
/// runaway/corrupt DL that would recurse forever is bounded here rather
/// than blowing the host stack.
const MAX_DL_DEPTH: u32 = 10;

/// The simple ("reference-fixture") F3D-style decoder retained for backward
/// compatibility: `G_VTX`/`G_TRI1`/`G_TRI2`/`G_ENDDL` with raw screen-space
/// `ob` coords, non-segmented addresses in `w1`, and the pre-existing
/// vertex/index packing (`n<<12 | v0`; indices `(v0<<16)|(v1<<8)|v2` as
/// plain cache slots). This is what the original hand-built fixtures and the
/// `fn64-abi` executor-seam test plant, so it MUST stay bit-compatible with
/// them. Real OoT display lists use [`decode_display_list_f3dex2`] instead.
pub fn decode_display_list(rdram: &[u8], dl_addr: u32) -> Result<Vec<Triangle>, RenderError> {
    reset_skip_warnings();
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
                    };
                }
            }
            G_TRI1 => {
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(&vtx_cache, idx, CullMode::None, None) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                let idx_a = [(w0 >> 16) & 0xFF, (w0 >> 8) & 0xFF, w0 & 0xFF];
                let idx_b = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(&vtx_cache, idx_a, CullMode::None, None) {
                    tris.push(t);
                }
                if let Some(t) = resolve_tri(&vtx_cache, idx_b, CullMode::None, None) {
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
    reset_skip_warnings();
    let mut state = DecodeState {
        vtx_cache: [Vertex::default(); 32],
        tris: Vec::new(),
        segments: [0u32; 16],
        mvp: None,
        proj: None,
        modelview: identity(),
        mv_stack: Vec::new(),
        viewport: None,
        geometry_mode: 0,
        dl_depth: 0,
        tex: TexState::default(),
    };
    decode_stream(rdram, dl_addr, &mut state);
    Ok(state.tris)
}

fn decode_stream(rdram: &[u8], dl_addr: u32, state: &mut DecodeState) {
    let mut pc = resolve_addr(&state.segments, dl_addr);

    loop {
        if pc + 8 > rdram.len() {
            break; // truncated command stream: stop, return what we have.
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
                let idx = tri_indices(w0);
                if let Some(t) = resolve_tri(&state.vtx_cache, idx, cull, texture) {
                    state.tris.push(t);
                }
            }
            G_TRI2 | G_QUAD => {
                // F3DEX2 G_TRI2 / G_QUAD (§2.3): triangle A's three 7-bit
                // slot fields in w0 (bits 17/9/1), triangle B's in w1 at the
                // SAME bit positions. G_QUAD decodes identically to G_TRI2.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex);
                let idx_a = tri_indices(w0);
                let idx_b = tri_indices(w1);
                if let Some(t) = resolve_tri(&state.vtx_cache, idx_a, cull, texture.clone()) {
                    state.tris.push(t);
                }
                if let Some(t) = resolve_tri(&state.vtx_cache, idx_b, cull, texture) {
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
                    if is_projection {
                        // The projection matrix ALSO honors LOAD vs MUL. OoT
                        // loads the perspective matrix once with LOAD, then
                        // concatenates the camera/view matrix onto it with
                        // PROJECTION|MUL (guLookAt output). Treating every
                        // projection G_MTX as a LOAD (the old bug) let the
                        // view matrix -- whose 4th row is [0,0,0,1], no
                        // projective term -- OVERWRITE the real perspective
                        // matrix (4th row [0,0,-1,0]). The result was w==1
                        // for every vertex (no perspective divide), so
                        // eye-space coords like x=-42 were used directly as
                        // NDC and every triangle projected thousands of
                        // pixels off-screen -> uniform/blank frames. Verified
                        // from a live task's three G_MTX loads (perspective
                        // LOAD, view MUL, model LOAD).
                        state.proj = Some(if is_load {
                            mtx
                        } else {
                            match state.proj {
                                Some(p) => mat_mul(&p, &mtx),
                                None => mtx,
                            }
                        });
                    } else {
                        // Modelview: a PUSH saves the current top so a later
                        // G_POPMTX restores it. LOAD replaces, MUL
                        // concatenates onto the current modelview.
                        if is_push {
                            state.mv_stack.push(state.modelview);
                        }
                        if is_load {
                            state.modelview = mtx;
                        } else {
                            state.modelview = mat_mul(&state.modelview, &mtx);
                        }
                    }
                    recompute_mvp(state);
                }
            }
            G_POPMTX => {
                // F3DEX2 gsSPPopMatrix: pop the modelview stack (params in
                // w1 select which stack; only the modelview stack is
                // modeled here). Restore the previous modelview if any.
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
                    if is_branch {
                        // Tail branch: the target replaced this stream; nothing
                        // valid follows the branch command. Stop here.
                        break;
                    }
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
            G_SETTIMG => {
                // G_SETTIMG (§5.1): format field(w0,21,3), size field(w0,19,2),
                // width-1 field(w0,0,12), image addr w1 (segmented). Pointer +
                // format latch only; no texel data moves until a G_LOAD*.
                state.tex.timg_fmt = ((w0 >> 21) & 0x07) as u8;
                state.tex.timg_siz = ((w0 >> 19) & 0x03) as u8;
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
                // image. w1 count field(w1,14,10) = (num-1)<<2 in the SDK
                // macro. TLUT entries are 16-bit RGBA5551 in RDRAM.
                let count = (((w1 >> 14) & 0x3FF) >> 2) as usize + 1;
                let n = count.min(256);
                let base = resolve_addr(&state.segments, state.tex.timg_addr);
                let mut tlut = Vec::with_capacity(n);
                for i in 0..n {
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
                if let Some(tex) =
                    decode_current_texture(rdram, &state.tex, &state.segments, tile)
                {
                    state.tex.current = Some(tex);
                }
            }
            G_MOVEMEM => {
                // F3DEX2 gsMoveMem (§1.4): w0 low byte = index (which RSP
                // block), w1 = segmented source address. G_MV_VIEWPORT
                // (index 8) points at a 16-byte `Vp` we parse into the screen
                // mapping; other indices (lights, absolute matrices) are
                // phase-2 and acknowledged-and-skipped.
                let index = (w0 & 0xFF) as u8;
                if index == G_MV_VIEWPORT {
                    let addr = resolve_addr(&state.segments, w1);
                    if let Some(vp) = read_viewport(rdram, addr) {
                        state.viewport = Some(vp);
                    }
                } else {
                    skip_opcode(G_MOVEMEM);
                }
            }
            G_GEOMETRYMODE => {
                // F3DEX2 gsSPGeometryMode (§2.4): one atomic clear+set --
                // `mode = (mode & field(w0,0,24)) | w1`, where the w0 low 24
                // bits are the (already-inverted) AND mask. We honor the
                // CULL_FRONT/CULL_BACK bits per-triangle (see cull_mode_from);
                // the other bits (shade/lighting/fog) are decode-time state a
                // flat/modulated frame doesn't act on yet.
                let and_mask = w0 & 0x00FF_FFFF;
                state.geometry_mode = (state.geometry_mode & and_mask) | w1;
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                // 16-byte (two-word) RDP rectangle command (gbi.h:4973). We
                // don't rasterize 2D texrects yet, but we MUST advance past
                // the second word (S/T + dsdx/dtdy) so the following command
                // is read at the right offset -- otherwise the coord word
                // decodes as a garbage opcode and desyncs the stream.
                skip_opcode(opcode);
                pc += 8;
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

/// Recompute the cached projection*modelview matrix from the current stack.
fn recompute_mvp(state: &mut DecodeState) {
    state.mvp = state.proj.map(|p| mat_mul(&p, &state.modelview));
    if state.mvp.is_none() {
        // No projection loaded: use the modelview alone (still lets a
        // model-space-only transform position raw coords).
        // Leave mvp None only when NO transform at all was ever seen.
    }
}

/// Load `n` vertices starting at cache slot `v0` from the (segmented) array
/// at `arr_addr`, applying the active transform if one is loaded.
fn load_vertices(
    rdram: &[u8],
    state: &mut DecodeState,
    arr_addr: u32,
    n: usize,
    v0: usize,
) {
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
        let r = read_u8(rdram, off + 12);
        let g = read_u8(rdram, off + 13);
        let b = read_u8(rdram, off + 14);
        let a = read_u8(rdram, off + 15);

        let (sx, sy, sz) = project_vertex(state, x, y, z);
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
        };
    }
}

/// Map a model-space vertex to screen space. If a full projection*modelview
/// is active, apply it, perspective-divide, and map NDC [-1,1] through the
/// viewport (or a 320x240 default half-extent if no viewport was loaded).
/// If NO transform is loaded at all, the raw `ob` x/y are already screen
/// coordinates (the pre-existing reference-fixture convention) and pass
/// through unchanged.
fn project_vertex(state: &DecodeState, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    match state.mvp {
        Some(mvp) => {
            let clip = transform_point(&mvp, x, y, z);
            // Perspective divide: clip -> NDC. Guard w~0 (a vertex on the
            // camera plane) so a divide-by-zero doesn't fling it to infinity.
            let w = if clip[3].abs() > 1e-6 { clip[3] } else { 1.0 };
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
                    (px, py, pz)
                }
                None => {
                    // Default viewport: 320x240, origin center.
                    let px = ndc_x * 160.0 + 160.0;
                    let py = -ndc_y * 120.0 + 120.0;
                    (px, py, ndc_z)
                }
            }
        }
        None => {
            // No transform: raw screen coords (reference-fixture path).
            (x, y, 0.0)
        }
    }
}

/// Extract the three F3DEX2 triangle vertex-cache slot indices from a
/// command word: three 7-bit fields at bit offsets 17, 9, 1 (F3DEX2-
/// CONCEPTS.md §2.2). Each field is already the slot (0-31).
fn tri_indices(w: u32) -> [u32; 3] {
    [(w >> 17) & 0x7F, (w >> 9) & 0x7F, (w >> 1) & 0x7F]
}

fn resolve_tri(
    vtx_cache: &[Vertex; 32],
    idx: [u32; 3],
    cull: CullMode,
    texture: Option<Texture>,
) -> Option<Triangle> {
    if idx.iter().any(|&i| i as usize >= vtx_cache.len()) {
        return None;
    }
    Some(Triangle {
        v: [
            vtx_cache[idx[0] as usize],
            vtx_cache[idx[1] as usize],
            vtx_cache[idx[2] as usize],
        ],
        cull,
        texture,
    })
}

/// This reference backend's one supported ucode family declaration --
/// shared constant so `lib.rs` and tests agree on it.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];

#[cfg(test)]
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
            ((G_TRI1 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
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
            ((G_TRI1 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
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
            wr_cmd(
                rd,
                off,
                ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
                0,
            );
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
            1,
            "texrect must consume both words so the following G_TRI1 is \
             decoded at the right offset"
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

    // --- Texture sampling (priority 3) ----------------------------------

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
}
