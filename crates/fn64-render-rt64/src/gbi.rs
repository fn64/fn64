//! A minimal F3DEX2-family display-list decoder: just enough opcodes to
//! turn a vertex buffer + a handful of triangles into filled polygons.
//!
//! ## Provenance
//!
//! `Gfx` is the N64 SDK's public 64-bit-word display-list command
//! encoding: every `gsSP*`/`gsDP*` macro in the publicly published
//! `gbi.h` header (redistributed in countless public SDK-header mirrors
//! and referenced throughout N64 homebrew/modding documentation) packs to
//! exactly this two-`u32` shape -- opcode in the top byte of the first
//! word, remaining fields packed by the specific opcode. This module reads
//! only the raw wire values (opcode byte + field bit-offsets), not any
//! vendor SDK/microcode C source -- the encoding is packaging-level ABI
//! (how a `u64` is laid out on the wire), the same standing as this
//! project's other public-ABI citations (`os_task.h`'s `OSTask_t`, per
//! `fn64_runtime::rsp`'s doc comment).
//!
//! ## Scope (deliberately small)
//!
//! Only four opcodes are interpreted: `G_VTX` (load vertices into a fixed
//! 32-slot buffer, the real hardware's vertex cache size), `G_TRI1`/`G_TRI2`
//! (one/two flat-shaded triangles referencing already-loaded vertex slots),
//! and `G_ENDDL` (stop). No matrix stack, no texturing, no lighting, no
//! clipping against the view frustum -- this is a reference backend whose
//! job is proving the render SEAM end-to-end (a real display list ->
//! nonzero pixels), not a faithful RDP/RSP reimplementation. Any other
//! opcode byte is simply skipped (its second word ignored), matching how a
//! real display list interleaves state-setting ops this backend doesn't
//! yet need to honor for a flat-color triangle to land on screen.
use fn64_render::{RenderError, UcodeId};

pub const G_VTX: u8 = 0x01;
pub const G_TRI1: u8 = 0x05;
pub const G_TRI2: u8 = 0x06;
pub const G_ENDDL: u8 = 0xDF;

/// One decoded vertex: screen-space position (already projected -- this
/// reference backend does no transform/projection, per module doc) plus a
/// flat RGBA color, matching the position+color fields of the SDK's public
/// `Vtx` union (the `n`-normal/lighting variant is not modeled -- out of
/// scope, see module doc).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A decoded, screen-space-ready triangle (three already-resolved
/// vertices) -- the display-list decoder's actual output, consumed by the
/// rasterizer in `raster.rs`.
#[derive(Copy, Clone, Debug, Default)]
pub struct Triangle {
    pub v: [Vertex; 3],
}

/// The N64 SDK's public per-vertex wire format (`Vtx_t`, the position-color
/// variant): 16 bytes -- `ob[3]` (s16 x/y/z), `flag` (u16, unused here),
/// `tc[2]` (s16 st, unused here), `cn[4]` (u8 r/g/b/a). This reference
/// backend reads `ob[0]`/`ob[1]` directly as screen-space x/y (no
/// projection matrix applied -- see module doc's scope note) and `cn` as a
/// flat vertex color.
const VTX_STRIDE: usize = 16;

/// Decode and rasterize-prep a display list rooted at `dl_addr` (an
/// rdram-relative byte offset, already KSEG0-translated by the caller) out
/// of `rdram`. Returns the flat-shaded triangles found. Any read that would
/// run off the end of `rdram` stops decoding and returns what was
/// successfully decoded so far, rather than panicking -- a malformed or
/// truncated fixture is a soft failure (fewer triangles), not a crash.
pub fn decode_display_list(rdram: &[u8], dl_addr: u32) -> Result<Vec<Triangle>, RenderError> {
    let mut vtx_cache = [Vertex::default(); 32];
    let mut tris = Vec::new();
    let mut pc = dl_addr as usize;

    loop {
        if pc + 8 > rdram.len() {
            break; // truncated command stream: stop, return what we have.
        }
        let w0 = u32::from_be_bytes(rdram[pc..pc + 4].try_into().unwrap());
        let w1 = u32::from_be_bytes(rdram[pc + 4..pc + 8].try_into().unwrap());
        let opcode = (w0 >> 24) as u8;
        pc += 8;

        match opcode {
            G_VTX => {
                // gsSPVertex(v, n, v0): w0 low 20 bits = n<<12 | v0 (SDK's
                // documented packing), w1 = rdram-relative vertex array
                // address (already segment-relative in this reference
                // backend -- no segment table is modeled, see module doc).
                let n = ((w0 >> 12) & 0xFF) as usize;
                let v0 = (w0 & 0xFF) as usize;
                let addr = w1 as usize;
                for i in 0..n {
                    let off = addr + i * VTX_STRIDE;
                    if off + VTX_STRIDE > rdram.len() || v0 + i >= vtx_cache.len() {
                        break;
                    }
                    let x = i16::from_be_bytes(rdram[off..off + 2].try_into().unwrap()) as f32;
                    let y = i16::from_be_bytes(rdram[off + 2..off + 4].try_into().unwrap()) as f32;
                    let cn = &rdram[off + 12..off + 16];
                    vtx_cache[v0 + i] = Vertex {
                        x,
                        y,
                        r: cn[0],
                        g: cn[1],
                        b: cn[2],
                        a: cn[3],
                    };
                }
            }
            G_TRI1 => {
                // gsSP1Triangle(v0,v1,v2,_): w1 byte-packed vertex-cache
                // indices, SDK's documented `(v0<<16)|(v1<<8)|v2`
                // (post-flag-shift convention; this reference backend reads
                // the indices pre-divided-by-2 form some SDK macro variants
                // use is NOT modeled -- indices are read as plain byte
                // slots, matching the raw wire value written by this
                // crate's own fixture builder in `tests/`).
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(&vtx_cache, idx) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                // gsSP2Triangles: two triangles packed across w0/w1's
                // remaining bytes (SDK's documented layout). Reference
                // backend reads both triangles' six indices from the two
                // words' low three bytes each.
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
            _ => {} // unmodeled opcode: skip, per module doc's scope note.
        }
    }

    Ok(tris)
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
    })
}

/// This reference backend's one supported ucode family declaration --
/// shared constant so `lib.rs` and tests agree on it.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];
