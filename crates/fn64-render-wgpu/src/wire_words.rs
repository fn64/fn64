//! The single shared builder for RDP command wire words.
//!
//! Nine hand-rolled encoders for the same RDP command set existed across this
//! crate's test modules, three of them predating the raw-triangle lane. Each
//! re-derived the same bit layouts from the same command summary, and each was
//! an independent place to get a shift wrong. This module is the one place
//! those layouts are written down.
//!
//! **Every layout here is derived from the WIRE, not from the decoder.** The
//! builders emit `u32` command words exactly as the RDP command stream carries
//! them, so a stream built here goes through the real decoder. A builder that
//! shortcut past the decoder -- constructing `RawTriangle` or `RawWord`
//! directly -- would stop testing the thing that breaks.
//!
//! Test-only: this is `#[cfg(test)]` in the crate root, shared by
//! `production`, `raw_dpc`, `raw_dpc::production_adapter`,
//! `raw_dpc::triangle_span` and `targets::raw_triangle`.

/// One RDP command word 0: the opcode in bits 31..24, payload below.
///
/// The `prefix` is the high bit-pair some callers set to prove the decoder
/// masks the opcode rather than comparing the whole byte; it is OR-ed into the
/// opcode byte exactly as `raw_dpc`'s own `word` did.
pub(crate) const fn word_with_prefix(prefix: u8, opcode: u8, payload: u32) -> u32 {
    ((prefix | opcode) as u32) << 24 | payload
}

/// One RDP command word 0 with no prefix bits set.
pub(crate) const fn word(opcode: u8, payload: u32) -> u32 {
    word_with_prefix(0, opcode, payload)
}

// --- opcodes ---------------------------------------------------------------

/// The non-shaded, non-textured, non-depth triangle opcode.
pub(crate) const RAW_TRIANGLE_BASE_EDGE: u8 = 0x08;
/// The shaded (non-textured, non-depth) triangle opcode.
pub(crate) const RAW_TRIANGLE_SHADE: u8 = 0x0c;
pub(crate) const SET_OTHER_MODE: u8 = 0x2f;
pub(crate) const SET_COMBINE: u8 = 0x3c;
pub(crate) const SET_PRIM_COLOR: u8 = 0x3a;

// --- triangle edge words ---------------------------------------------------

/// Q16.16 for a whole number of pixels.
pub(crate) const fn px(pixels: i32) -> i32 {
    pixels << 16
}

/// S11.2 for a whole number of scanlines.
pub(crate) const fn line(scanlines: i16) -> i16 {
    scanlines << 2
}

/// One triangle command's four base-edge words (eight `u32` halves), as the
/// stream carries them.
///
/// Layout taken from the wire: word 0 high half holds `lft` at bit 23,
/// `level` at 21..19, `tile` at 18..16; word 0 low half is YL (S11.2). Word 1
/// high half is YM, low half is YH. Then XL/dXLdy, XH/dXHdy, XM/dXMdy as full
/// 32-bit Q16.16 pairs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeWords {
    pub(crate) lft: bool,
    pub(crate) tile: u32,
    pub(crate) level: u32,
    pub(crate) yl: i16,
    pub(crate) ym: i16,
    pub(crate) yh: i16,
    pub(crate) xl: i32,
    pub(crate) dxldy: i32,
    pub(crate) xh: i32,
    pub(crate) dxhdy: i32,
    pub(crate) xm: i32,
    pub(crate) dxmdy: i32,
}

impl EdgeWords {
    /// An all-zero edge set; every field is then named explicitly by the
    /// caller, so no builder here silently supplies geometry.
    pub(crate) const fn zeroed() -> Self {
        Self {
            lft: false,
            tile: 0,
            level: 0,
            yl: 0,
            ym: 0,
            yh: 0,
            xl: 0,
            dxldy: 0,
            xh: 0,
            dxhdy: 0,
            xm: 0,
            dxmdy: 0,
        }
    }

    /// Word 0 alone, for callers that assemble the remaining seven words
    /// themselves.
    pub(crate) const fn word0(&self, prefix: u8, opcode: u8) -> u32 {
        word_with_prefix(
            prefix,
            opcode,
            (self.lft as u32) << 23
                | (self.level & 0x7) << 19
                | (self.tile & 0x7) << 16
                | (self.yl as u16 as u32),
        )
    }

    /// All eight `u32` halves of the four base-edge words.
    pub(crate) const fn words(&self, prefix: u8, opcode: u8) -> [u32; 8] {
        [
            self.word0(prefix, opcode),
            ((self.ym as u16 as u32) << 16) | (self.yh as u16 as u32),
            self.xl as u32,
            self.dxldy as u32,
            self.xh as u32,
            self.dxhdy as u32,
            self.xm as u32,
            self.dxmdy as u32,
        ]
    }

    /// The same four words as big-endian bytes, the form
    /// `RawTriangle::decode` consumes.
    pub(crate) fn bytes(&self, opcode: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32);
        for half in self.words(0, opcode) {
            bytes.extend_from_slice(&half.to_be_bytes());
        }
        bytes
    }
}

// --- coefficient block -----------------------------------------------------

/// One 8-word (64-byte) coefficient block from four groups of four Q16.16
/// components.
///
/// The block is NOT sixteen consecutive Q16.16 values. As sixteen `u32`
/// halves (half `n` is byte `4n`), each component's HIGH 16 bits sit at its
/// integer offset and its LOW 16 bits sixteen bytes later. Components 0 and 2
/// occupy their `u32`'s high half, 1 and 3 the low half.
///
/// Byte offsets, from the RDP command summary and matched against
/// `fn64-render-reference`'s `decode_rdp_shade_coefficients`:
///   value (0, 16)  d/dx (8, 24)  d/de (32, 48)  d/dy (40, 56)
pub(crate) fn coefficient_halves(
    value: [i32; 4],
    dx: [i32; 4],
    de: [i32; 4],
    dy: [i32; 4],
) -> [u32; 16] {
    let mut halves = [0u32; 16];
    let mut put = |integer_byte: usize, fraction_byte: usize, components: [i32; 4]| {
        for (index, component) in components.iter().enumerate() {
            let half_index = integer_byte / 4 + index / 2;
            let fraction_index = fraction_byte / 4 + index / 2;
            let shift = if index % 2 == 0 { 16 } else { 0 };
            halves[half_index] |= (((*component >> 16) as u32) & 0xffff) << shift;
            halves[fraction_index] |= ((*component as u32) & 0xffff) << shift;
        }
    };
    put(0, 16, value);
    put(8, 24, dx);
    put(32, 48, de);
    put(40, 56, dy);
    halves
}

// --- state commands --------------------------------------------------------

/// `SetOtherMode`'s two words: cycle type at bits 21..20 of the payload.
pub(crate) const fn set_other_mode(cycle_type: u32, low: u32) -> [u32; 2] {
    [word(SET_OTHER_MODE, cycle_type << 20), low]
}

/// `SetCombine`'s two words, from the two packed bitfield slices.
pub(crate) const fn set_combine(low: u32, high: u32) -> [u32; 2] {
    [word(SET_COMBINE, low), high]
}

/// `SetPrimColor`'s two words: LOD min at payload bits 15..8, LOD fraction at
/// 7..0, and the RGBA colour in word 1.
pub(crate) const fn set_prim_color(lod_frac: u32, lod_min: u32, color: u32) -> [u32; 2] {
    [word(SET_PRIM_COLOR, lod_min << 8 | lod_frac), color]
}

/// The two packed `SetCombine` bitfield slices for a combiner program named by
/// its eight slot indices.
///
/// Colour A/B/C/D and alpha A/B/C/D pack as:
///   low  = (cA << 5) | cC
///   high = (cB << 24) | (cD << 6) | (aA << 21) | (aB << 3) | (aC << 18) | aD
pub(crate) const fn combine_wire(color: [u32; 4], alpha: [u32; 4]) -> (u32, u32) {
    let [ca, cb, cc, cd] = color;
    let [aa, ab, ac, ad] = alpha;
    let low = (ca << 5) | cc;
    let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
    (low, high)
}

/// Colour/alpha slot indices selecting `Zero` for A/B/C. Colour A/B use slot
/// 8 and colour C slot 16 (each that input's own out-of-table `Zero`); alpha
/// A/B/C use slot 7.
pub(crate) const ZERO_ABC_COLOR: [u32; 3] = [8, 8, 16];
pub(crate) const ZERO_ABC_ALPHA: [u32; 3] = [7, 7, 7];

/// `(Zero - Zero) * Zero + D` for the D slot named, in both colour and alpha.
pub(crate) const fn passthrough_combine(d_slot: u32) -> (u32, u32) {
    combine_wire(
        [
            ZERO_ABC_COLOR[0],
            ZERO_ABC_COLOR[1],
            ZERO_ABC_COLOR[2],
            d_slot,
        ],
        [
            ZERO_ABC_ALPHA[0],
            ZERO_ABC_ALPHA[1],
            ZERO_ABC_ALPHA[2],
            d_slot,
        ],
    )
}

/// The `Primitive` slot in `colorInputD` / `alphaInputABD`.
pub(crate) const D_SLOT_PRIMITIVE: u32 = 3;
/// The `Texel0` slot in `colorInputD` / `alphaInputABD`.
///
/// Index 1 of `color_input_common` and of the alpha D table alike -- the two
/// tables agree at this index, which is why one constant serves both halves
/// of `passthrough_combine`.
pub(crate) const D_SLOT_TEXEL0: u32 = 1;
/// The `Shade` slot in `colorInputD` / `alphaInputABD`.
pub(crate) const D_SLOT_SHADE: u32 = 4;
