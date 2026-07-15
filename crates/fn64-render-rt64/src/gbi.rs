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
const G_SETSCISSOR: u8 = 0xED;
const G_SETTILESIZE: u8 = 0xF2;
const G_LOADTILE: u8 = 0xF4;
const G_SETPRIMCOLOR: u8 = 0xFA;
const G_SETENVCOLOR: u8 = 0xFB;
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

/// A decoded, screen-space-ready triangle (three already-resolved
/// vertices) -- the display-list decoder's actual output, consumed by the
/// rasterizer in `raster.rs`.
#[derive(Copy, Clone, Debug, Default)]
pub struct Triangle {
    pub v: [Vertex; 3],
    /// The culling mode in effect (from `G_GEOMETRYMODE`) when this triangle
    /// was emitted. Carried per-triangle because geometry mode is decode-time
    /// RSP state that can change between `G_TRI*` commands; the rasterizer
    /// reads it to cull by winding. `None` for the simple reference path.
    pub cull: CullMode,
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
        G_MODIFYVTX => "G_MODIFYVTX",
        G_CULLDL => "G_CULLDL",
        G_BRANCH_Z => "G_BRANCH_Z",
        G_LINE3D => "G_LINE3D",
        0xE0 => "G_SPNOOP",
        0xE1 => "G_RDPHALF_1",
        0xE2 => "G_SETOTHERMODE_L",
        0xE3 => "G_SETOTHERMODE_H",
        0xE6 => "G_RDPLOADSYNC",
        0xE7 => "G_RDPPIPESYNC",
        0xE8 => "G_RDPTILESYNC",
        0xE9 => "G_RDPFULLSYNC",
        0xF0 => "G_LOADTLUT",
        0xF1 => "G_RDPHALF_2",
        0xF3 => "G_LOADBLOCK",
        G_LOADTILE => "G_LOADTILE",
        G_SETTILESIZE => "G_SETTILESIZE",
        0xF5 => "G_SETTILE",
        0xFC => "G_SETCOMBINE",
        0xFD => "G_SETTIMG",
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
                    };
                }
            }
            G_TRI1 => {
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(&vtx_cache, idx) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                let idx_a = [(w0 >> 16) & 0xFF, (w0 >> 8) & 0xFF, w0 & 0xFF];
                let idx_b = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(&vtx_cache, idx_a) {
                    tris.push(t);
                }
                if let Some(t) = resolve_tri(&vtx_cache, idx_b) {
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
                let idx = tri_indices(w0);
                if let Some(t) = resolve_tri(&state.vtx_cache, idx) {
                    state.tris.push(t);
                }
            }
            G_TRI2 | G_QUAD => {
                // F3DEX2 G_TRI2 / G_QUAD (§2.3): triangle A's three 7-bit
                // slot fields in w0 (bits 17/9/1), triangle B's in w1 at the
                // SAME bit positions. G_QUAD decodes identically to G_TRI2.
                let idx_a = tri_indices(w0);
                let idx_b = tri_indices(w1);
                if let Some(t) = resolve_tri(&state.vtx_cache, idx_a) {
                    state.tris.push(t);
                }
                if let Some(t) = resolve_tri(&state.vtx_cache, idx_b) {
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
                // F3DEX2 gsSPDisplayList (gbi.h ~2177): w0 = op<<24 |
                // push<<16; w1 = segmented address of the nested DL. push==0
                // (G_DL_PUSH) recurses (call); low bit set (G_DL_NOPUSH)
                // jumps (tail). We recurse for both, bounded by MAX_DL_DEPTH,
                // and the jump case simply continues after return.
                if state.dl_depth < MAX_DL_DEPTH {
                    let saved_mv = state.modelview;
                    state.mv_stack.push(saved_mv);
                    state.dl_depth += 1;
                    decode_stream(rdram, w1, state);
                    state.dl_depth -= 1;
                    if let Some(m) = state.mv_stack.pop() {
                        state.modelview = m;
                    }
                    recompute_mvp(state);
                } else {
                    eprintln!(
                        "[fn64-render-rt64/gbi] G_DL recursion exceeded MAX_DL_DEPTH \
                         ({MAX_DL_DEPTH}) -- refusing to recurse further (possible corrupt \
                         or cyclic display list)."
                    );
                }
            }
            G_ENDDL => break,
            _ => skip_opcode(opcode),
        }
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

fn resolve_tri(vtx_cache: &[Vertex; 32], idx: [u32; 3]) -> Option<Triangle> {
    if idx.iter().any(|&i| i as usize >= vtx_cache.len()) {
        return None;
    }
    Some(Triangle {
        v: [
            vtx_cache[idx[0] as usize],
            vtx_cache[idx[1] as usize],
            vtx_cache[idx[2] as usize],
        ],
        // Culling not yet driven from G_GEOMETRYMODE (that opcode is still a
        // loud-skip); default to no culling so geometry isn't wrongly dropped.
        cull: CullMode::None,
    })
}

/// This reference backend's one supported ucode family declaration --
/// shared constant so `lib.rs` and tests agree on it.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];
