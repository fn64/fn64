//! Raw RDP TextureRectangle/TextureRectangleFlip command decode (opcodes
//! `0x24`/`0x25`) and RT64's deterministic rectangle-to-six-vertex
//! position/texcoord conversion.
//!
//! Field layout comes from the permitted MIT RT64 source pinned by
//! `docs/RT64-PORT-AUTHORITY.md` at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, `src/gbi/rt64_gbi_rdp.cpp`'s
//! `texrectLLE`/`texrectFlipLLE` (the raw/LLE wire-decode variants -- the
//! ones this crate's raw-DPC stream actually carries, not the HLE
//! `texrect`/`texrectFlip` that read from a live RSP `DisplayList**`
//! cursor) and `DisplayList::p0`/`p1` (`src/gbi/rt64_gbi.cpp`:
//! `return ((w0 >> pos) & ((0x01 << bits) - 1));` /
//! `return ((w1 >> pos) & ((0x01 << bits) - 1));`). The conversion comes
//! from `src/hle/rt64_rdp.cpp`'s `RDP::drawTexRect` (the two-argument-list
//! overload's copy-mode `dsdx`/`lrx`/`lry` mutation) and `RDP::drawRect`
//! (fill/copy UL rounding, `FixedRect` construction, `width`/`height`,
//! `lrs`/`lrt`, `vFractionOffset`, and the six
//! `triPosFloats`/`triColorFloats`/`triTcFloats` vertex writes), plus
//! `src/common/rt64_common.cpp`'s `FixedRect::isEmpty`/`left`/`top`/`right`/
//! `bottom`/`width`/`height` (the bounded default-alignment quarter-pixel
//! rounding this module ports).
//!
//! Nonclaims (explicitly out of this slice): no production raw-DPC
//! admission or execution -- this module is not wired into
//! `decode_stream`/`push_decoded_raw_dpc`; no scissor-rectangle
//! intersection, no `movedFromOrigin`/`ExtendedAlignment` origin-stack
//! offset (RT64's `drawRect` applies both before constructing its
//! `FixedRect`; this module constructs the `FixedRect` directly from
//! `ulx`/`uly`/`lrx`/`lry` with no origin/scissor adjustment -- the exact
//! bounded default RT64's own `FixedRect` type performs, not an invented
//! alignment correction); no texture sampling or TMEM read; no rasterizer;
//! no combiner/blend/depth/coverage/render-target/VI; no native GPU,
//! parity, or performance claim. `RDP::updateCallTexcoords`'s tracked-tile
//! texcoord bookkeeping and the scissor-intersected `intU1`/`intV1`/`intU2`/
//! `intV2` branch are workload/tile-tracking side effects this pure
//! conversion has no state to attach to and does not reproduce.

use core::fmt;

use crate::state::CycleType;

/// One decoded raw RDP TextureRectangle/TextureRectangleFlip command's exact
/// wire payload -- fixed-point, no float conversion.
///
/// Wire shape (16 bytes / two 64-bit words), from RT64's `texrectLLE`/
/// `texrectFlipLLE`:
///
/// - Word 0 `w0` (`p0`): bits 12:12 = `lrx`, bits 0:12 = `lry` (12-bit
///   unsigned fixed coordinates, matching this crate's existing
///   `FillRectangle` decode of the same wire shape in `raw_dpc::mod`).
/// - Word 0 `w1` (`p1`): bits 24:3 = `tile`, bits 12:12 = `ulx`, bits 0:12 =
///   `uly`.
/// - Word 1 `w0` (`p0`): bits 16:16 = `uls`, bits 0:16 = `ult` (signed
///   16-bit).
/// - Word 1 `w1` (`p1`): bits 16:16 = `dsdx`, bits 0:16 = `dtdy` (signed
///   16-bit).
///
/// `flip` is not a wire field: RT64 dispatches `texrectLLE` (opcode `0x24`)
/// with `flip = false` and `texrectFlipLLE` (opcode `0x25`) with
/// `flip = true`. `RawTextureRectangle::decode` reproduces that exactly:
/// `flip` is derived solely from which of the two opcodes was decoded, never
/// read from a wire bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawTextureRectangle {
    ulx: u16,
    uly: u16,
    lrx: u16,
    lry: u16,
    tile: u8,
    uls: i16,
    ult: i16,
    dsdx: i16,
    dtdy: i16,
    flip: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawTextureRectangleError {
    /// `opcode` is neither `0x24` (TextureRectangle) nor `0x25`
    /// (TextureRectangleFlip).
    OpcodeOutOfRange { opcode: u8 },
    /// The command slice is not exactly the 16-byte public width both
    /// opcodes share.
    UnexpectedLength { expected: u32, actual: u32 },
}

impl fmt::Display for RawTextureRectangleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpcodeOutOfRange { opcode } => write!(
                formatter,
                "opcode {opcode:#04x} is neither TextureRectangle (0x24) nor TextureRectangleFlip (0x25)"
            ),
            Self::UnexpectedLength { expected, actual } => write!(
                formatter,
                "texture rectangle command slice is {actual} bytes, expected exactly {expected}"
            ),
        }
    }
}

impl std::error::Error for RawTextureRectangleError {}

const TEXRECT: u8 = 0x24;
const TEXRECT_FLIP: u8 = 0x25;

/// Exact public wire width of a TextureRectangle/TextureRectangleFlip
/// command: two 64-bit words.
pub const TEXTURE_RECTANGLE_COMMAND_BYTES: u32 = 16;

impl RawTextureRectangle {
    /// Decodes one TextureRectangle/TextureRectangleFlip command from its
    /// exact opcode (masked to the low six bits, matching `decode_stream`'s
    /// own `wire_opcode & 0x3f`) and 16-byte payload slice.
    ///
    /// `opcode` must be `0x24` or `0x25`; any other value is rejected before
    /// the length or any word is read. `command` must then be exactly
    /// [`TEXTURE_RECTANGLE_COMMAND_BYTES`] bytes.
    pub fn decode(opcode: u8, command: &[u8]) -> Result<Self, RawTextureRectangleError> {
        let flip = match opcode {
            TEXRECT => false,
            TEXRECT_FLIP => true,
            other => return Err(RawTextureRectangleError::OpcodeOutOfRange { opcode: other }),
        };
        let actual = u32::try_from(command.len()).unwrap_or(u32::MAX);
        if actual != TEXTURE_RECTANGLE_COMMAND_BYTES {
            return Err(RawTextureRectangleError::UnexpectedLength {
                expected: TEXTURE_RECTANGLE_COMMAND_BYTES,
                actual,
            });
        }

        let word0_w0 = u32::from_be_bytes(command[0..4].try_into().expect("checked length"));
        let word0_w1 = u32::from_be_bytes(command[4..8].try_into().expect("checked length"));
        let word1_w0 = u32::from_be_bytes(command[8..12].try_into().expect("checked length"));
        let word1_w1 = u32::from_be_bytes(command[12..16].try_into().expect("checked length"));

        let lrx = ((word0_w0 >> 12) & 0x0fff) as u16;
        let lry = (word0_w0 & 0x0fff) as u16;
        let tile = ((word0_w1 >> 24) & 0x7) as u8;
        let ulx = ((word0_w1 >> 12) & 0x0fff) as u16;
        let uly = (word0_w1 & 0x0fff) as u16;

        let uls = ((word1_w0 >> 16) & 0xffff) as i16;
        let ult = (word1_w0 & 0xffff) as i16;
        let dsdx = ((word1_w1 >> 16) & 0xffff) as i16;
        let dtdy = (word1_w1 & 0xffff) as i16;

        Ok(Self {
            ulx,
            uly,
            lrx,
            lry,
            tile,
            uls,
            ult,
            dsdx,
            dtdy,
            flip,
        })
    }

    pub const fn ulx(self) -> u16 {
        self.ulx
    }

    pub const fn uly(self) -> u16 {
        self.uly
    }

    pub const fn lrx(self) -> u16 {
        self.lrx
    }

    pub const fn lry(self) -> u16 {
        self.lry
    }

    pub const fn tile(self) -> u8 {
        self.tile
    }

    pub const fn uls(self) -> i16 {
        self.uls
    }

    pub const fn ult(self) -> i16 {
        self.ult
    }

    pub const fn dsdx(self) -> i16 {
        self.dsdx
    }

    pub const fn dtdy(self) -> i16 {
        self.dtdy
    }

    /// `true` for opcode `0x25` (TextureRectangleFlip), `false` for `0x24`.
    pub const fn flip(self) -> bool {
        self.flip
    }
}

/// One vertex's clip-space-ready position (RT64's static
/// `rectPosFloats` -- always one of the four corners `(-1,1)`/`(1,1)`/
/// `(-1,-1)`/`(1,-1)` with `z = 0.0`, `w = 1.0`) and shaded color (RT64's
/// static `rectColorFloats` -- always exactly zero), plus this vertex's
/// texture coordinate from `triTcFloats`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureRectangleVertex {
    position: [f32; 4],
    color: [f32; 4],
    texcoord: [f32; 2],
}

impl TextureRectangleVertex {
    pub const fn position(self) -> [f32; 4] {
        self.position
    }

    pub const fn color(self) -> [f32; 4] {
        self.color
    }

    pub const fn texcoord(self) -> [f32; 2] {
        self.texcoord
    }
}

/// The six vertices RT64's `RDP::drawRect` writes for one rectangle (two
/// triangles, RT64's exact `triPosFloats`/`triColorFloats`/`triTcFloats`
/// push order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureRectangleVertices {
    vertices: [TextureRectangleVertex; 6],
}

impl TextureRectangleVertices {
    /// Vertex `index` (`0..=5`) in RT64's exact write order.
    ///
    /// # Panics
    ///
    /// Panics if `index >= 6`.
    pub const fn vertex(&self, index: usize) -> TextureRectangleVertex {
        self.vertices[index]
    }
}

/// This pure conversion's minimal caller-supplied cycle-mode fact: RT64's
/// `RDP::drawTexRect`/`RDP::drawRect` read `otherMode.cycleType()` twice --
/// once each for `usesCopyMode` (drawTexRect's `dsdx`/`lrx`/`lry` mutation)
/// and `usesFillMode || usesCopyMode` (drawRect's UL rounding). Both reads
/// are of the same single `CycleType` value, so this module accepts the
/// caller's already-decoded `CycleType` directly rather than two redundant
/// booleans -- `is_copy_mode`/`is_fill_or_copy_mode` below derive both facts
/// from it exactly as RT64's own two call sites do.
fn is_copy_mode(cycle_type: CycleType) -> bool {
    cycle_type == CycleType::Copy
}

fn is_fill_or_copy_mode(cycle_type: CycleType) -> bool {
    matches!(cycle_type, CycleType::Fill | CycleType::Copy)
}

/// Converts one decoded raw TextureRectangle/TextureRectangleFlip command to
/// its six vertices, exactly as RT64's `RDP::drawTexRect` followed by
/// `RDP::drawRect` does for one rectangle -- with no scissor intersection,
/// no `ExtendedAlignment` origin/offset, and no workload/tile-tracking side
/// effects (see the module doc's nonclaims).
///
/// `cycle_type` is the caller-supplied already-decoded RDP cycle type
/// (RT64: `state->rdp->otherMode.cycleType()`); this function performs no
/// OtherMode decode of its own.
///
/// Returns `None` when RT64's own `drawRect` would return early --
/// `FixedRect::isEmpty()` after fill/copy UL rounding (RT64: `if
/// (drawRect.isEmpty()) { return; }`). A caller that needs to distinguish
/// "empty, no draw" from "would-be vertices" gets that distinction for
/// free: `None` is the exact reproduction of RT64's early return, not a
/// silently-substituted degenerate rectangle.
///
/// IEEE f32 results -- including infinities and NaNs a literal division or
/// shift-then-convert RT64's fixed-point math can produce -- are preserved
/// exactly as RT64's own arithmetic produces them. No defensive fallback is
/// applied.
pub fn texture_rectangle_vertices(
    rectangle: RawTextureRectangle,
    cycle_type: CycleType,
) -> Option<TextureRectangleVertices> {
    let flip = rectangle.flip();

    // RT64 `RDP::drawTexRect`: `int32_t`-widened rectangle corners (the RSP
    // decode already narrowed these to unsigned 12-bit fields; RT64's own
    // `texrectLLE` extraction leaves them as plain non-negative `int32_t`,
    // so widening this module's `u16` wire fields to `i32` here is bit-exact
    // with RT64's own type, not a narrowing or a sign-changing conversion).
    let mut ulx = i32::from(rectangle.ulx());
    let mut uly = i32::from(rectangle.uly());
    let mut lrx = i32::from(rectangle.lrx());
    let mut lry = i32::from(rectangle.lry());
    let mut dsdx = i32::from(rectangle.dsdx());
    let dtdy = i32::from(rectangle.dtdy());
    let uls = i32::from(rectangle.uls());
    let ult = i32::from(rectangle.ult());

    // RT64 `RDP::drawTexRect`: "Divide dsdx by 4 and add an extra pixel to
    // the edges if it uses copy mode." `dsdx >>= 2` operates on RT64's own
    // `int16_t dsdx` parameter: the shift is performed at `int` precision
    // (integral promotion) and the result narrows back into `int16_t` on
    // reassignment, which is a lossless no-op for every value this 16-bit
    // wire field can carry (`dsdx >> 2` stays within `int16_t`'s range for
    // all `dsdx` in `i16::MIN..=i16::MAX`). This module instead performs the
    // shift on the already-widened `i32` local throughout -- both are
    // signed arithmetic right shifts, and the widening changes nothing
    // observable at this value range. `lrx |= 3` / `lry |= 3` are plain
    // bitwise-or.
    if is_copy_mode(cycle_type) {
        dsdx >>= 2;
        lrx |= 3;
        lry |= 3;
    }

    // RT64 `RDP::drawRect`: "Round down the left and top coordinates in
    // fill mode or copy mode." `ulx &= ~3; uly &= ~3;` on `int32_t` --
    // these coordinates are always non-negative 12-bit wire values before
    // this point, so the top bits `~3` clears are already zero and this is
    // an ordinary round-down, not a two's-complement sign trap.
    if is_fill_or_copy_mode(cycle_type) {
        ulx &= !3;
        uly &= !3;
    }

    // RT64 `RDP::drawRect`: `const FixedRect drawRect(movedFromOrigin(ulx,
    // extAlignment.leftOrigin), uly, movedFromOrigin(lrx,
    // extAlignment.rightOrigin), lry);` -- this module applies no
    // `movedFromOrigin`/`ExtendedAlignment` offset (nonclaim), so the
    // `FixedRect` is built directly from the (possibly copy-mode-mutated)
    // `ulx`/`uly`/`lrx`/`lry`.
    //
    // `FixedRect::isEmpty()` (`src/common/rt64_common.cpp`):
    // `isNull() || (lrx == ulx) || (lry == uly)`; `isNull()`: `(ulx > lrx)
    // || (uly > lry)`. Ported directly, not renormalized: a reversed
    // rectangle is `isNull()` (hence `isEmpty()`) and returns `None` here,
    // exactly as RT64's `drawRect` returns early -- it is not silently
    // swapped into a valid orientation.
    let is_null = ulx > lrx || uly > lry;
    let is_empty = is_null || lrx == ulx || lry == uly;
    if is_empty {
        return None;
    }

    // `FixedRect::left(bool ceil)`/`top`/`right`/`bottom`:
    // `(coordinate + (ceil ? 3 : 0)) >> 2`, a signed arithmetic right shift
    // on `int32_t`. `RDP::drawRect` calls `drawRect.width(true, true)` /
    // `.height(true, true)` -- both `ceil` arguments `true` -- so both ends
    // of both axes use the `+3` rounding path here; `width`/`height` are
    // `right(rightCeil) - left(leftCeil)` / `bottom(bottomCeil) -
    // top(topCeil)`.
    let left = (ulx + 3) >> 2;
    let top = (uly + 3) >> 2;
    let right = (lrx + 3) >> 2;
    let bottom = (lry + 3) >> 2;
    let rect_width = right - left;
    let rect_height = bottom - top;

    // RT64: `const int32_t uvWidth = (flip ? rectHeight : rectWidth) << 2;`
    // and the matching `uvHeight` line -- UV width/height swap under flip,
    // then both are converted back to the 2.2 fixed-point coordinate space
    // rectWidth/rectHeight were extracted from (`<< 2`, undoing `left`/
    // `top`/`right`/`bottom`'s `>> 2`).
    let uv_width = (if flip { rect_height } else { rect_width }) << 2;
    let uv_height = (if flip { rect_width } else { rect_height }) << 2;

    // RT64: `const int32_t lrs = ((uls << 7) + dsdx * uvWidth) >> 7;` and
    // the matching `lrt` line. `<<7`, the `dsdx * uvWidth` multiply, `+`,
    // then `>>7` in that exact left-to-right operation order on `int32_t` --
    // Rust's `i32` arithmetic matches C++'s `int32_t` bit-for-bit (both
    // two's-complement, both wrapping on overflow in release builds; this
    // module does not add overflow guards RT64's own unchecked arithmetic
    // does not have).
    let lrs = ((uls << 7) + dsdx * uv_width) >> 7;
    let lrt = ((ult << 7) + dtdy * uv_height) >> 7;

    // RT64: `const float vFractionOffset = (uly & 0x3) ? (dtdy >> 5) /
    // 32.0f : 0.0f;` -- note this reads the *original* (pre-round-down)
    // `uly`, not the FixedRect's rounded `uly` local RT64 shadows it with;
    // RT64's own `uly` parameter is mutated in place by the `&= ~3` above,
    // so by this point RT64's `uly` *is* already rounded -- meaning
    // `vFractionOffset` is always computed against the rounded value, and
    // in fill/copy mode `uly & 0x3` is therefore always zero, making
    // `vFractionOffset` always `0.0` in those two cycle types. This module
    // reproduces that exactly by reading the same post-round-down `uly`
    // local used above, not the pre-round-down wire value.
    let v_fraction_offset = if uly & 0x3 != 0 {
        f32::from((dtdy >> 5) as i16) / 32.0
    } else {
        0.0
    };

    let u1 = uls as f32 / 32.0;
    let v1 = ult as f32 / 32.0 + v_fraction_offset;
    let u2 = lrs as f32 / 32.0;
    let v2 = lrt as f32 / 32.0 + v_fraction_offset;

    const RECT_POS_FLOATS: [[f32; 4]; 6] = [
        [-1.0, 1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0, 1.0],
        [-1.0, -1.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0, 1.0],
        [-1.0, -1.0, 0.0, 1.0],
    ];
    const ZERO_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

    // RT64's exact `triTcFloats.emplace_back(...)` push order: (u1, v1),
    // (flip ? u1 : u2, flip ? v2 : v1), (flip ? u2 : u1, flip ? v1 : v2),
    // (u2, v2), (flip ? u1 : u2, flip ? v2 : v1), (flip ? u2 : u1,
    // flip ? v1 : v2) -- six pushes, one `[u, v]` pair each, matching the
    // six position/color entries one-for-one.
    let texcoords: [[f32; 2]; 6] = [
        [u1, v1],
        [if flip { u1 } else { u2 }, if flip { v2 } else { v1 }],
        [if flip { u2 } else { u1 }, if flip { v1 } else { v2 }],
        [u2, v2],
        [if flip { u1 } else { u2 }, if flip { v2 } else { v1 }],
        [if flip { u2 } else { u1 }, if flip { v1 } else { v2 }],
    ];

    let mut vertices = [TextureRectangleVertex {
        position: [0.0; 4],
        color: ZERO_COLOR,
        texcoord: [0.0; 2],
    }; 6];
    for index in 0..6 {
        vertices[index] = TextureRectangleVertex {
            position: RECT_POS_FLOATS[index],
            color: ZERO_COLOR,
            texcoord: texcoords[index],
        };
    }

    Some(TextureRectangleVertices { vertices })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Independent oracle: re-derives every formula directly from the raw
    // 16-byte wire payload and the RT64 source text, without calling any
    // production helper (`RawTextureRectangle::decode`,
    // `texture_rectangle_vertices`, `is_copy_mode`/`is_fill_or_copy_mode`).
    // Uses plain i64/f64 arithmetic internally to avoid sharing an
    // intermediate-overflow mechanism with the implementation, narrowing to
    // i32/f32 only at the same points RT64's own `int32_t`/`float` locals
    // would narrow.
    struct Oracle {
        ulx: i64,
        uly: i64,
        lrx: i64,
        lry: i64,
        uls: i64,
        ult: i64,
        dsdx: i64,
        dtdy: i64,
        flip: bool,
    }

    impl Oracle {
        fn wrap_i32(value: i64) -> i32 {
            value as i32
        }

        fn drawn_vertices(&self, is_copy: bool, is_fill_or_copy: bool) -> Option<[[f32; 2]; 6]> {
            let mut ulx = self.ulx;
            let uly = self.uly;
            let mut lrx = self.lrx;
            let mut lry = self.lry;
            let mut dsdx = self.dsdx;

            if is_copy {
                dsdx = Self::wrap_i32(dsdx >> 2) as i64;
                lrx |= 3;
                lry |= 3;
            }
            if is_fill_or_copy {
                ulx &= !3;
            }
            let uly = if is_fill_or_copy { uly & !3 } else { uly };

            let is_null = ulx > lrx || uly > lry;
            let is_empty = is_null || lrx == ulx || lry == uly;
            if is_empty {
                return None;
            }

            let left = Self::wrap_i32(ulx + 3) >> 2;
            let top = Self::wrap_i32(uly + 3) >> 2;
            let right = Self::wrap_i32(lrx + 3) >> 2;
            let bottom = Self::wrap_i32(lry + 3) >> 2;
            let width = right - left;
            let height = bottom - top;

            let uv_width = (if self.flip { height } else { width }) << 2;
            let uv_height = (if self.flip { width } else { height }) << 2;

            let uls32 = Self::wrap_i32(self.uls);
            let ult32 = Self::wrap_i32(self.ult);
            let dsdx32 = Self::wrap_i32(dsdx);
            let dtdy32 = Self::wrap_i32(self.dtdy);

            let lrs =
                Self::wrap_i32((((uls32 << 7) as i64) + (dsdx32 as i64) * (uv_width as i64)) >> 7);
            let lrt =
                Self::wrap_i32((((ult32 << 7) as i64) + (dtdy32 as i64) * (uv_height as i64)) >> 7);

            let v_fraction_offset = if uly & 0x3 != 0 {
                f64::from((dtdy32 >> 5) as i16) / 32.0
            } else {
                0.0
            };

            let u1 = f64::from(uls32) / 32.0;
            let v1 = f64::from(ult32) / 32.0 + v_fraction_offset;
            let u2 = f64::from(lrs) / 32.0;
            let v2 = f64::from(lrt) / 32.0 + v_fraction_offset;
            let (u1, v1, u2, v2) = (u1 as f32, v1 as f32, u2 as f32, v2 as f32);

            Some([
                [u1, v1],
                [
                    if self.flip { u1 } else { u2 },
                    if self.flip { v2 } else { v1 },
                ],
                [
                    if self.flip { u2 } else { u1 },
                    if self.flip { v1 } else { v2 },
                ],
                [u2, v2],
                [
                    if self.flip { u1 } else { u2 },
                    if self.flip { v2 } else { v1 },
                ],
                [
                    if self.flip { u2 } else { u1 },
                    if self.flip { v1 } else { v2 },
                ],
            ])
        }
    }

    fn word_bytes(w0: u32, w1: u32) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&w0.to_be_bytes());
        bytes[4..8].copy_from_slice(&w1.to_be_bytes());
        bytes
    }

    #[allow(clippy::too_many_arguments)]
    fn command_bytes(
        ulx: u16,
        uly: u16,
        lrx: u16,
        lry: u16,
        tile: u8,
        uls: i16,
        ult: i16,
        dsdx: i16,
        dtdy: i16,
    ) -> Vec<u8> {
        let word0_w0 = (u32::from(lrx) & 0x0fff) << 12 | (u32::from(lry) & 0x0fff);
        let word0_w1 = (u32::from(tile) & 0x7) << 24
            | (u32::from(ulx) & 0x0fff) << 12
            | (u32::from(uly) & 0x0fff);
        let word1_w0 = (uls as u16 as u32) << 16 | (ult as u16 as u32);
        let word1_w1 = (dsdx as u16 as u32) << 16 | (dtdy as u16 as u32);
        let mut bytes = Vec::with_capacity(16);
        bytes.extend(word_bytes(word0_w0, word0_w1));
        bytes.extend(word_bytes(word1_w0, word1_w1));
        bytes
    }

    #[allow(clippy::too_many_arguments)]
    fn oracle(
        ulx: u16,
        uly: u16,
        lrx: u16,
        lry: u16,
        uls: i16,
        ult: i16,
        dsdx: i16,
        dtdy: i16,
        flip: bool,
    ) -> Oracle {
        Oracle {
            ulx: i64::from(ulx),
            uly: i64::from(uly),
            lrx: i64::from(lrx),
            lry: i64::from(lry),
            uls: i64::from(uls),
            ult: i64::from(ult),
            dsdx: i64::from(dsdx),
            dtdy: i64::from(dtdy),
            flip,
        }
    }

    fn assert_close(actual: f32, expected: f32, context: &str) {
        assert!(
            (actual - expected).abs() <= expected.abs() * 1e-5 + 1e-6,
            "{context}: expected {expected}, got {actual}"
        );
    }

    fn assert_vertices_match(computed: TextureRectangleVertices, expected_tc: [[f32; 2]; 6]) {
        for (index, expected_tc) in expected_tc.into_iter().enumerate() {
            let vertex = computed.vertex(index);
            for (actual, expected) in vertex.texcoord().into_iter().zip(expected_tc) {
                assert_close(actual, expected, "texcoord");
            }
        }
    }

    // --- decode: wire shape, both opcodes, all-zero/max fields ---

    #[test]
    fn decode_extracts_every_field_for_both_opcodes() {
        let bytes = command_bytes(0x123, 0x456, 0x789, 0xabc, 5, -100, 200, -300, 400);
        let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        assert_eq!(texrect.ulx(), 0x123);
        assert_eq!(texrect.uly(), 0x456);
        assert_eq!(texrect.lrx(), 0x789);
        assert_eq!(texrect.lry(), 0xabc);
        assert_eq!(texrect.tile(), 5);
        assert_eq!(texrect.uls(), -100);
        assert_eq!(texrect.ult(), 200);
        assert_eq!(texrect.dsdx(), -300);
        assert_eq!(texrect.dtdy(), 400);
        assert!(!texrect.flip());

        let flipped = RawTextureRectangle::decode(0x25, &bytes).unwrap();
        assert_eq!(flipped.ulx(), texrect.ulx());
        assert!(flipped.flip());
    }

    #[test]
    fn decode_all_zero_fields() {
        let bytes = command_bytes(0, 0, 0, 0, 0, 0, 0, 0, 0);
        let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        assert_eq!(texrect.ulx(), 0);
        assert_eq!(texrect.uly(), 0);
        assert_eq!(texrect.lrx(), 0);
        assert_eq!(texrect.lry(), 0);
        assert_eq!(texrect.tile(), 0);
        assert_eq!(texrect.uls(), 0);
        assert_eq!(texrect.ult(), 0);
        assert_eq!(texrect.dsdx(), 0);
        assert_eq!(texrect.dtdy(), 0);
    }

    #[test]
    fn decode_max_coordinate_and_tile_fields() {
        let bytes = command_bytes(0x0fff, 0x0fff, 0x0fff, 0x0fff, 7, 0, 0, 0, 0);
        let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        assert_eq!(texrect.ulx(), 0x0fff);
        assert_eq!(texrect.uly(), 0x0fff);
        assert_eq!(texrect.lrx(), 0x0fff);
        assert_eq!(texrect.lry(), 0x0fff);
        assert_eq!(texrect.tile(), 7);
    }

    #[test]
    fn decode_signed_field_boundaries() {
        for (uls, ult, dsdx, dtdy) in [
            (i16::MIN, i16::MAX, i16::MIN, i16::MAX),
            (i16::MAX, i16::MIN, i16::MAX, i16::MIN),
            (-1, -1, -1, -1),
        ] {
            let bytes = command_bytes(0, 0, 0x0fff, 0x0fff, 0, uls, ult, dsdx, dtdy);
            let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
            assert_eq!(texrect.uls(), uls);
            assert_eq!(texrect.ult(), ult);
            assert_eq!(texrect.dsdx(), dsdx);
            assert_eq!(texrect.dtdy(), dtdy);
        }
    }

    #[test]
    fn tile_bits_do_not_leak_into_ulx_or_uly() {
        for bit in 0..3 {
            let bytes = command_bytes(0, 0, 0x0fff, 0x0fff, 1u8 << bit, 0, 0, 0, 0);
            let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
            assert_eq!(texrect.tile(), 1u8 << bit, "tile bit {bit}");
            assert_eq!(texrect.ulx(), 0);
            assert_eq!(texrect.uly(), 0);
        }
    }

    // --- decode: wrong opcode / one-byte-short / high prefix ---

    #[test]
    fn decode_rejects_wrong_opcode_before_length_check() {
        for opcode in [0x00u8, 0x23, 0x26, 0x36, 0xff] {
            let error = RawTextureRectangle::decode(opcode, &[]).unwrap_err();
            assert_eq!(
                error,
                RawTextureRectangleError::OpcodeOutOfRange { opcode },
                "opcode {opcode:#04x} must be rejected as out of range"
            );
        }
    }

    #[test]
    fn decode_rejects_one_byte_short() {
        for opcode in [0x24u8, 0x25] {
            let full = command_bytes(1, 2, 3, 4, 5, 6, 7, 8, 9);
            let short = &full[..full.len() - 1];
            let error = RawTextureRectangle::decode(opcode, short).unwrap_err();
            assert_eq!(
                error,
                RawTextureRectangleError::UnexpectedLength {
                    expected: 16,
                    actual: 15,
                }
            );
        }
    }

    #[test]
    fn decode_rejects_oversized_slice() {
        let mut bytes = command_bytes(1, 2, 3, 4, 5, 6, 7, 8, 9);
        bytes.push(0);
        let error = RawTextureRectangle::decode(0x24, &bytes).unwrap_err();
        assert_eq!(
            error,
            RawTextureRectangleError::UnexpectedLength {
                expected: 16,
                actual: 17,
            }
        );
    }

    /// Every high wire-prefix combination (bits 6:7 of the opcode byte,
    /// e.g. `decode_stream`'s `wire_opcode & 0xc0`) must decode identically
    /// once masked to the low six bits -- this module receives the
    /// already-masked opcode (matching `RawTriangle::decode`'s contract),
    /// so the prefix must never leak into the semantic opcode or any
    /// decoded field.
    #[test]
    fn all_high_prefix_variants_decode_identically_once_masked() {
        let bytes = command_bytes(0x100, 0x200, 0x300, 0x400, 3, 10, 20, 30, 40);
        let baseline = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
            let masked_opcode = (prefix | TEXRECT) & 0x3f;
            assert_eq!(masked_opcode, TEXRECT);
            let decoded = RawTextureRectangle::decode(masked_opcode, &bytes).unwrap();
            assert_eq!(decoded, baseline, "prefix {prefix:#04x}");
        }
    }

    // --- conversion: cycle mode combinations, flip/nonflip ---

    #[test]
    fn one_cycle_mode_applies_neither_copy_nor_fill_rounding() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0005, 0x0006, 0x0020, 0x0020, 0, 0, 0, 0x0020, 0x0020),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        let expected = oracle(0x0005, 0x0006, 0x0020, 0x0020, 0, 0, 0x0020, 0x0020, false)
            .drawn_vertices(false, false)
            .unwrap();
        assert_vertices_match(computed, expected);
    }

    #[test]
    fn two_cycle_mode_applies_neither_copy_nor_fill_rounding() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0005, 0x0006, 0x0020, 0x0020, 0, 0, 0, 0x0020, 0x0020),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::TwoCycle).unwrap();
        let expected = oracle(0x0005, 0x0006, 0x0020, 0x0020, 0, 0, 0x0020, 0x0020, false)
            .drawn_vertices(false, false)
            .unwrap();
        assert_vertices_match(computed, expected);
    }

    #[test]
    fn fill_mode_rounds_ul_but_not_copy_mutation() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0007, 0x0009, 0x0020, 0x0020, 0, 0, 0, 0x0020, 0x0020),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::Fill).unwrap();
        let expected = oracle(0x0007, 0x0009, 0x0020, 0x0020, 0, 0, 0x0020, 0x0020, false)
            .drawn_vertices(false, true)
            .unwrap();
        assert_vertices_match(computed, expected);
    }

    #[test]
    fn copy_mode_applies_dsdx_shift_and_lrx_lry_or_and_ul_rounding() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0007, 0x0009, 0x0020, 0x0020, 0, 0, 0, 0x0100, 0x0020),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::Copy).unwrap();
        let expected = oracle(0x0007, 0x0009, 0x0020, 0x0020, 0, 0, 0x0100, 0x0020, false)
            .drawn_vertices(true, true)
            .unwrap();
        assert_vertices_match(computed, expected);

        // Direct sensitivity check: copy mode's dsdx >>= 2 must have fired
        // (0x0100 >> 2 = 0x0040), not been skipped -- comparing against the
        // unmutated (`is_copy=false`) oracle path must disagree.
        let unmutated = oracle(0x0007, 0x0009, 0x0020, 0x0020, 0, 0, 0x0100, 0x0020, false)
            .drawn_vertices(false, true)
            .unwrap();
        assert_ne!(computed.vertex(3).texcoord(), unmutated[3]);
    }

    #[test]
    fn flip_swaps_uv_width_and_height_and_texcoord_order() {
        let bytes = command_bytes(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0, 0x0040, 0x0080);
        let nonflip = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        let flipped = RawTextureRectangle::decode(0x25, &bytes).unwrap();
        assert!(!nonflip.flip());
        assert!(flipped.flip());

        let computed_nonflip = texture_rectangle_vertices(nonflip, CycleType::OneCycle).unwrap();
        let computed_flip = texture_rectangle_vertices(flipped, CycleType::OneCycle).unwrap();
        assert_ne!(computed_nonflip, computed_flip);

        let expected_nonflip = oracle(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0x0040, 0x0080, false)
            .drawn_vertices(false, false)
            .unwrap();
        let expected_flip = oracle(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0x0040, 0x0080, true)
            .drawn_vertices(false, false)
            .unwrap();
        assert_vertices_match(computed_nonflip, expected_nonflip);
        assert_vertices_match(computed_flip, expected_flip);
    }

    // --- fractional UL Y and vFractionOffset ---

    #[test]
    fn fractional_ul_y_in_one_cycle_mode_yields_nonzero_fraction_offset() {
        // uly = 5 (binary ...101, low two bits nonzero) in a non-fill/copy
        // mode leaves uly unrounded, so `uly & 0x3 != 0` and
        // vFractionOffset must be nonzero.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0005, 0x0020, 0x0020, 0, 0, 0, 0, 0x0100),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        let expected_offset = f32::from(0x0100i16 >> 5) / 32.0;
        assert_ne!(
            expected_offset, 0.0,
            "fixture sanity: offset must be nonzero"
        );
        assert_close(computed.vertex(0).texcoord()[1], expected_offset, "v1");
    }

    #[test]
    fn fill_mode_always_rounds_uly_to_zero_fraction_offset() {
        // In fill/copy mode uly is rounded down (&= ~3) before
        // vFractionOffset reads it, so vFractionOffset is always exactly
        // 0.0 regardless of the original wire uly's low two bits.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0005, 0x0020, 0x0020, 0, 0, 0, 0, 0x0100),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::Fill).unwrap();
        assert_eq!(computed.vertex(0).texcoord()[1], 0.0);
    }

    #[test]
    fn exact_fraction_offset_matches_dtdy_shift_five_over_32() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0001, 0x0020, 0x0020, 0, 0, 0, 0, 0x0400),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        // dtdy=0x0400=1024; 1024>>5 = 32; 32/32.0 = 1.0 exactly.
        assert_close(
            computed.vertex(0).texcoord()[1],
            1.0,
            "exact fraction offset",
        );
    }

    // --- negative shifts and right-shift semantics ---

    #[test]
    fn negative_dsdx_arithmetic_right_shift_in_copy_mode() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0, -0x0100i16, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::Copy).unwrap();
        let expected = oracle(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, -0x0100, 0, false)
            .drawn_vertices(true, true)
            .unwrap();
        assert_vertices_match(computed, expected);
        // Sanity: -0x0100 >> 2 must be the arithmetic-shift result -0x40,
        // not an unsigned/logical-shift artifact.
        assert_eq!(-0x0100i32 >> 2, -0x40);
    }

    #[test]
    fn negative_dtdy_shift_five_in_fraction_offset() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0001, 0x0020, 0x0020, 0, 0, 0, 0, -0x0400i16),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        // dtdy=-0x0400=-1024; -1024>>5 = -32 (arithmetic shift); -32/32.0 =
        // -1.0 exactly.
        assert_close(
            computed.vertex(0).texcoord()[1],
            -1.0,
            "negative fraction offset",
        );
    }

    #[test]
    fn negative_uls_and_ult_pass_through_the_32_divisor_signed() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, -320, -640, 0, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        assert_close(computed.vertex(0).texcoord()[0], -10.0, "u1 = -320/32");
        assert_close(computed.vertex(0).texcoord()[1], -20.0, "v1 = -640/32");
    }

    // --- reversed / empty rectangles: no silent normalization ---

    #[test]
    fn reversed_rectangle_returns_none_not_a_normalized_rectangle() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0020, 0x0020, 0x0010, 0x0010, 0, 0, 0, 0, 0),
        )
        .unwrap();
        assert_eq!(texture_rectangle_vertices(raw, CycleType::OneCycle), None);
    }

    #[test]
    fn zero_width_rectangle_is_empty_returns_none() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0010, 0x0000, 0x0010, 0x0020, 0, 0, 0, 0, 0),
        )
        .unwrap();
        assert_eq!(texture_rectangle_vertices(raw, CycleType::OneCycle), None);
    }

    #[test]
    fn zero_height_rectangle_is_empty_returns_none() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0010, 0x0020, 0x0010, 0, 0, 0, 0, 0),
        )
        .unwrap();
        assert_eq!(texture_rectangle_vertices(raw, CycleType::OneCycle), None);
    }

    #[test]
    fn all_zero_rectangle_is_empty_returns_none() {
        let raw =
            RawTextureRectangle::decode(0x24, &command_bytes(0, 0, 0, 0, 0, 0, 0, 0, 0)).unwrap();
        assert_eq!(texture_rectangle_vertices(raw, CycleType::OneCycle), None);
    }

    #[test]
    fn max_coordinate_rectangle_that_is_nonempty_still_converts() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0fff, 0x0fff, 0, 0, 0, 0, 0),
        )
        .unwrap();
        assert!(texture_rectangle_vertices(raw, CycleType::OneCycle).is_some());
    }

    /// Worst-case bounded-field stress: maximum 12-bit rectangle extent
    /// (`lrx`/`lry = 0x0fff`, the widest `uv_width`/`uv_height` this wire
    /// shape can produce) combined with every `i16::MIN`/`i16::MAX` corner
    /// of `uls`/`ult`/`dsdx`/`dtdy`, under every `CycleType` (so both the
    /// copy-mode `dsdx`-widened-then-shifted path and the plain path are
    /// exercised). `lrs`/`lrt`'s `(value << 7) + factor * uv_width` must not
    /// debug-mode overflow-panic for any input this wire shape can encode --
    /// RT64's own `int32_t` arithmetic would silently wrap at this
    /// magnitude, never trap, so a Rust panic here would be a real
    /// behavioral divergence this test exists to catch.
    #[test]
    fn worst_case_bounded_fields_never_overflow_panic_in_conversion() {
        for (uls, ult, dsdx, dtdy) in [
            (i16::MIN, i16::MAX, i16::MIN, i16::MAX),
            (i16::MAX, i16::MIN, i16::MAX, i16::MIN),
            (i16::MIN, i16::MIN, i16::MIN, i16::MIN),
            (i16::MAX, i16::MAX, i16::MAX, i16::MAX),
        ] {
            let bytes = command_bytes(0x0000, 0x0000, 0x0fff, 0x0fff, 0, uls, ult, dsdx, dtdy);
            for opcode in [0x24u8, 0x25] {
                let raw = RawTextureRectangle::decode(opcode, &bytes).unwrap();
                for cycle_type in [
                    CycleType::OneCycle,
                    CycleType::TwoCycle,
                    CycleType::Copy,
                    CycleType::Fill,
                ] {
                    // Must not panic (overflow or otherwise); the specific
                    // Some/None outcome is not asserted here, only that
                    // evaluation completes.
                    let _ = texture_rectangle_vertices(raw, cycle_type);
                }
            }
        }
    }

    // --- copy mode can turn a would-be-empty rectangle nonempty via |=3 ---

    #[test]
    fn copy_mode_or_three_can_make_a_reversed_rectangle_nonempty() {
        // ulx=0, lrx=0: reversed/empty under any mode with the |=3 mutation
        // absent. But lry gets |=3'd from 0 to 3 in copy mode, and uly is
        // 0, so if ulx/lrx are left nonempty this demonstrates the mutation
        // actually widens the rect before the emptiness check runs.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0008, 0x0000, 0, 0, 0, 0, 0),
        )
        .unwrap();
        // Without copy mode: lry=0 == uly=0 -> empty.
        assert_eq!(
            texture_rectangle_vertices(raw, CycleType::OneCycle),
            None,
            "fixture sanity: empty without copy-mode mutation"
        );
        // With copy mode: lry |= 3 -> lry=3 != uly=0 -> nonempty.
        assert!(
            texture_rectangle_vertices(raw, CycleType::Copy).is_some(),
            "copy mode's lry |= 3 must turn this nonempty"
        );
    }

    // --- six-vertex position/color/order ---

    #[test]
    fn six_vertex_positions_and_colors_match_rt64_exact_static_arrays() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0, 0, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        let expected_positions: [[f32; 4]; 6] = [
            [-1.0, 1.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [-1.0, -1.0, 0.0, 1.0],
            [1.0, -1.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [-1.0, -1.0, 0.0, 1.0],
        ];
        for (index, expected_position) in expected_positions.into_iter().enumerate() {
            assert_eq!(
                computed.vertex(index).position(),
                expected_position,
                "vertex {index} position"
            );
            assert_eq!(
                computed.vertex(index).color(),
                [0.0, 0.0, 0.0, 0.0],
                "vertex {index} color"
            );
        }
    }

    #[test]
    #[should_panic]
    fn vertex_index_out_of_range_panics() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0, 0, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        let _ = computed.vertex(6);
    }

    // --- operation-order hostiles: <<7, multiply, >>7, /32 ---

    #[test]
    fn lrs_lrt_operation_order_shift_then_multiply_add_then_shift() {
        // uls=1, dsdx=1, uvWidth chosen so a wrong operation order (e.g.
        // multiplying before shifting uls, or shifting the sum by a
        // different amount) produces a detectably different lrs.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0084, 0x0004, 0, 1, 0, 1, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        let expected = oracle(0x0000, 0x0000, 0x0084, 0x0004, 1, 0, 1, 0, false)
            .drawn_vertices(false, false)
            .unwrap();
        assert_vertices_match(computed, expected);
        // width = (0x84+3)>>2 - (0+3)>>2 = 33 - 0 = 33; uvWidth = 33<<2 =
        // 132. lrs = ((1<<7) + 1*132) >> 7 = (128+132)>>7 = 260>>7 = 2.
        // u2 = 2/32.0 = 0.0625.
        assert_close(
            computed.vertex(3).texcoord()[0],
            0.0625,
            "lrs operation order",
        );
    }

    #[test]
    fn wrong_divisor_would_be_caught_by_exact_oracle_comparison() {
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 320, 0, 0, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        // u1 = 320 / 32.0 = 10.0 exactly -- not /16.0 (=20.0) or /64.0 (=5.0).
        assert_close(computed.vertex(0).texcoord()[0], 10.0, "32.0 divisor");
        assert_ne!(computed.vertex(0).texcoord()[0], 20.0);
        assert_ne!(computed.vertex(0).texcoord()[0], 5.0);
    }

    // --- source-shape hostiles: swapped halves, unsigned signed fields ---

    #[test]
    fn swapping_word0_w0_and_w1_changes_the_decode() {
        // word0.w0 carries lrx/lry; word0.w1 carries tile/ulx/uly. Swapping
        // them must change ulx/uly, not silently decode the same rectangle.
        let bytes = command_bytes(0x0111, 0x0222, 0x0333, 0x0444, 0, 0, 0, 0, 0);
        let mut swapped = bytes.clone();
        swapped[0..4].copy_from_slice(&bytes[4..8]);
        swapped[4..8].copy_from_slice(&bytes[0..4]);
        let original = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        let swapped = RawTextureRectangle::decode(0x24, &swapped).unwrap();
        assert_ne!(original.ulx(), swapped.ulx());
        assert_ne!(original.lrx(), swapped.lrx());
    }

    #[test]
    fn swapping_word1_w0_and_w1_changes_uls_ult_vs_dsdx_dtdy() {
        let bytes = command_bytes(0, 0, 0x0fff, 0x0fff, 0, 111, 222, 333, 444);
        let mut swapped = bytes.clone();
        swapped[8..12].copy_from_slice(&bytes[12..16]);
        swapped[12..16].copy_from_slice(&bytes[8..12]);
        let original = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        let swapped = RawTextureRectangle::decode(0x24, &swapped).unwrap();
        assert_ne!(original.uls(), swapped.uls());
        assert_ne!(original.dsdx(), swapped.dsdx());
        assert_eq!(original.uls(), swapped.dsdx());
        assert_eq!(original.dsdx(), swapped.uls());
    }

    #[test]
    fn uls_ult_dsdx_dtdy_are_signed_not_unsigned_fields() {
        // 0xFFFF as unsigned would be 65535; as signed i16 it is -1. A
        // decoder that mistakenly treated these as unsigned would produce a
        // very different (and out-of-range-for-i16) value.
        let bytes = command_bytes(0, 0, 0x0fff, 0x0fff, 0, -1, -1, -1, -1);
        let texrect = RawTextureRectangle::decode(0x24, &bytes).unwrap();
        assert_eq!(texrect.uls(), -1);
        assert_eq!(texrect.ult(), -1);
        assert_eq!(texrect.dsdx(), -1);
        assert_eq!(texrect.dtdy(), -1);
    }

    #[test]
    fn missing_copy_mutation_would_be_caught() {
        // Fixture where dsdx is odd, so >>2 truncates differently than not
        // shifting at all -- a decoder that "forgot" the copy-mode dsdx
        // mutation would disagree with the oracle's copy-mode-applied path
        // but agree with its unmutated path.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0, 0x0007, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::Copy).unwrap();
        let with_mutation = oracle(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0x0007, 0, false)
            .drawn_vertices(true, true)
            .unwrap();
        let without_mutation = oracle(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0x0007, 0, false)
            .drawn_vertices(false, true)
            .unwrap();
        assert_ne!(with_mutation[3], without_mutation[3], "fixture sanity");
        assert_vertices_match(computed, with_mutation);
    }

    #[test]
    fn missing_flip_swap_would_be_caught() {
        let bytes = command_bytes(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0, 0x0040, 0x0080);
        let flipped = RawTextureRectangle::decode(0x25, &bytes).unwrap();
        let computed = texture_rectangle_vertices(flipped, CycleType::OneCycle).unwrap();
        let with_flip = oracle(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0x0040, 0x0080, true)
            .drawn_vertices(false, false)
            .unwrap();
        let without_flip = oracle(0x0004, 0x0004, 0x0024, 0x0044, 0, 0, 0x0040, 0x0080, false)
            .drawn_vertices(false, false)
            .unwrap();
        assert_ne!(with_flip, without_flip, "fixture sanity");
        assert_vertices_match(computed, with_flip);
    }

    #[test]
    fn wrong_vertex_order_would_be_caught_by_position_table() {
        // A decoder emitting the six positions in a different order (e.g.
        // fan order instead of two-triangle-strip-like order) would fail
        // this exact index-by-index comparison.
        let raw = RawTextureRectangle::decode(
            0x24,
            &command_bytes(0x0000, 0x0000, 0x0020, 0x0020, 0, 0, 0, 0, 0),
        )
        .unwrap();
        let computed = texture_rectangle_vertices(raw, CycleType::OneCycle).unwrap();
        assert_eq!(computed.vertex(0).position(), [-1.0, 1.0, 0.0, 1.0]);
        assert_eq!(computed.vertex(1).position(), [1.0, 1.0, 0.0, 1.0]);
        assert_eq!(computed.vertex(2).position(), [-1.0, -1.0, 0.0, 1.0]);
        assert_eq!(computed.vertex(3).position(), [1.0, -1.0, 0.0, 1.0]);
        assert_eq!(computed.vertex(4).position(), [1.0, 1.0, 0.0, 1.0]);
        assert_eq!(computed.vertex(5).position(), [-1.0, -1.0, 0.0, 1.0]);
    }
}
