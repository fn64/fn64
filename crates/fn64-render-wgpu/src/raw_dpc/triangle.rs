//! Raw RDP triangle command decode (opcodes `0x08..=0x0f`).
//!
//! Field layout and word ordering come from the permitted MIT RT64 source
//! (`src/hle/rt64_rdp.h`'s `RDPTriangle` enum and `triangleBaseWords`/
//! `triangleShadeWords`/`triangleTexWords`/`triangleDepthWords` constants;
//! `src/gbi/rt64_gbi_rdp.cpp`'s `getTrianglePointers`/`decodeTriangles`), and
//! the public SGI *RDP Command Summary* triangle command sections (Table 11's
//! `0x08..=0x0f` "Non-Shaded"/"Non-Shaded, Z-Buffered"/"Shaded"/"Shaded,
//! Z-Buffered"/"Textured"/"Textured, Z-Buffered"/"Shaded, Textured"/"Shaded,
//! Textured, Z-Buffered" triangle rows and their per-row edge/shade/texture/
//! depth coefficient layouts). `raw_rdp_command_width` (`fn64-render`'s
//! `rdp_completion.rs`) already proves the exact eight word counts this
//! module decodes against; this module does not duplicate that stride table,
//! only the payload fields within an already-width-checked command slice.
//!
//! This module decodes the exact fixed-point wire payload only: tile, level,
//! flip, YL/YM/YH, XL/XM/XH, the three edge slopes, and every present raw
//! shade/texture/depth coefficient word. It performs no float conversion, no
//! edge-walk/rasterization, and no RDP state-machine transition -- the base
//! Edge command carries no `RdpState` field this decoder's neighbors already
//! model, so `decode_stream` gains a new `RawTriangle` arm with no delta.

use core::fmt;

use crate::tmem::TileIndex;

/// One coefficient block's presence, decoded from the low three opcode bits
/// (RT64 `RDPTriangle::Depth`/`Textured`/`Shaded`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleFlags {
    depth: bool,
    textured: bool,
    shaded: bool,
}

impl TriangleFlags {
    const fn from_opcode(opcode: u8) -> Self {
        Self {
            depth: opcode & 0x1 != 0,
            textured: opcode & 0x2 != 0,
            shaded: opcode & 0x4 != 0,
        }
    }

    pub const fn depth(self) -> bool {
        self.depth
    }

    pub const fn textured(self) -> bool {
        self.textured
    }

    pub const fn shaded(self) -> bool {
        self.shaded
    }
}

/// Exact 64-bit word count of one triangle command's payload for the given
/// flags, in RT64's base(4) + shade(8) + texture(8) + depth(2) word order.
/// Multiply by 8 for the byte width.
pub const fn triangle_word_count(flags: TriangleFlags) -> u32 {
    let mut words = 4;
    if flags.shaded {
        words += 8;
    }
    if flags.textured {
        words += 8;
    }
    if flags.depth {
        words += 2;
    }
    words
}

/// One raw 64-bit command word split into its two 32-bit halves, matching
/// RT64's `DisplayList { w0, w1 }` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawWord {
    w0: u32,
    w1: u32,
}

impl RawWord {
    pub const fn w0(self) -> u32 {
        self.w0
    }

    pub const fn w1(self) -> u32 {
        self.w1
    }
}

/// The eight raw shade or texture coefficient words, in wire order.
pub type CoefficientWords = [RawWord; 8];

/// The two raw depth coefficient words, in wire order.
pub type DepthWords = [RawWord; 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriangleDecodeError {
    /// `opcode` is outside the eight triangle opcodes `0x08..=0x0f`.
    OpcodeOutOfRange { opcode: u8 },
    /// The command slice is not exactly `triangle_word_count(flags)` bytes.
    UnexpectedLength { expected: u32, actual: u32 },
}

impl fmt::Display for TriangleDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpcodeOutOfRange { opcode } => write!(
                formatter,
                "opcode {opcode:#04x} is outside the eight triangle opcodes 0x08..=0x0f"
            ),
            Self::UnexpectedLength { expected, actual } => write!(
                formatter,
                "triangle command slice is {actual} bytes, expected exactly {expected}"
            ),
        }
    }
}

impl std::error::Error for TriangleDecodeError {}

/// One decoded raw RDP triangle command's complete fixed-point payload.
///
/// Every field is the raw wire value: no float conversion, no edge walk, no
/// rasterization. `RawTriangle::decode` proves the command slice is exactly
/// `triangle_word_count(flags())` bytes before reading any optional block, so
/// a truncated slice is rejected before any coefficient word is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawTriangle {
    flags: TriangleFlags,
    tile: TileIndex,
    level: u8,
    right_major: bool,
    yl: i16,
    ym: i16,
    yh: i16,
    xl: i32,
    dxldy: i32,
    xh: i32,
    dxhdy: i32,
    xm: i32,
    dxmdy: i32,
    shade: Option<CoefficientWords>,
    texture: Option<CoefficientWords>,
    depth: Option<DepthWords>,
}

impl RawTriangle {
    /// Decodes one triangle command from its exact opcode and payload slice.
    ///
    /// `opcode` must be one of the eight triangle opcodes `0x08..=0x0f`; any
    /// other value is rejected before the length or any word is read.
    /// `command` must then be exactly the bytes `raw_rdp_command_width(opcode)`
    /// admitted for this opcode -- the caller (`decode_stream`) has already
    /// proven that boundary against the stream length before this is called,
    /// so no partial optional block can be read: length is checked once,
    /// before any word is interpreted.
    pub fn decode(opcode: u8, command: &[u8]) -> Result<Self, TriangleDecodeError> {
        if !(0x08..=0x0f).contains(&opcode) {
            return Err(TriangleDecodeError::OpcodeOutOfRange { opcode });
        }
        let flags = TriangleFlags::from_opcode(opcode);
        let expected = triangle_word_count(flags) * 8;
        let actual = u32::try_from(command.len()).unwrap_or(u32::MAX);
        if actual != expected {
            return Err(TriangleDecodeError::UnexpectedLength { expected, actual });
        }

        let words: Vec<RawWord> = command
            .chunks_exact(8)
            .map(|chunk| RawWord {
                w0: u32::from_be_bytes(chunk[0..4].try_into().expect("chunk is 8 bytes")),
                w1: u32::from_be_bytes(chunk[4..8].try_into().expect("chunk is 8 bytes")),
            })
            .collect();

        // Base edge block (RT64 `triangleBaseWords` = 4): word 0 carries
        // tile/level/flip and YL/YM/YH; words 1..=3 carry XL/dxldy, XH/dxhdy,
        // XM/dxmdy in that RT64 order.
        let w0 = words[0];
        let tile_raw = ((w0.w0 >> 16) & 0x7) as u8;
        let tile = TileIndex::try_new(tile_raw).expect("triangle tile field is masked to 3 bits");
        let level = ((w0.w0 >> 19) & 0x7) as u8;
        let right_major = (w0.w0 >> 23) & 0x1 != 0;
        let yl = (w0.w0 & 0xffff) as i16;
        let ym = (w0.w1 >> 16) as i16;
        let yh = (w0.w1 & 0xffff) as i16;

        let edge1 = words[1];
        let xl = edge1.w0 as i32;
        let dxldy = edge1.w1 as i32;
        let edge2 = words[2];
        let xh = edge2.w0 as i32;
        let dxhdy = edge2.w1 as i32;
        let edge3 = words[3];
        let xm = edge3.w0 as i32;
        let dxmdy = edge3.w1 as i32;

        let mut cursor = 4usize;
        let shade = if flags.shaded {
            let block = read_coefficient_block(&words, cursor);
            cursor += 8;
            Some(block)
        } else {
            None
        };
        let texture = if flags.textured {
            let block = read_coefficient_block(&words, cursor);
            cursor += 8;
            Some(block)
        } else {
            None
        };
        let depth = if flags.depth {
            let block = [words[cursor], words[cursor + 1]];
            cursor += 2;
            Some(block)
        } else {
            None
        };
        debug_assert_eq!(cursor, words.len());

        Ok(Self {
            flags,
            tile,
            level,
            right_major,
            yl,
            ym,
            yh,
            xl,
            dxldy,
            xh,
            dxhdy,
            xm,
            dxmdy,
            shade,
            texture,
            depth,
        })
    }

    pub const fn flags(self) -> TriangleFlags {
        self.flags
    }

    pub const fn tile(self) -> TileIndex {
        self.tile
    }

    pub const fn level(self) -> u8 {
        self.level
    }

    /// The right-major (flip) bit: wire bit 23 of the command's first word.
    pub const fn right_major(self) -> bool {
        self.right_major
    }

    /// Signed 14-bit Y-low, sign-extended over the full 16-bit wire field
    /// (the public field's two unused high bits are hardware-written sign
    /// copies, so a 16-bit signed reinterpretation is bit-exact with a
    /// 14-bit sign extension).
    pub const fn yl(self) -> i16 {
        self.yl
    }

    pub const fn ym(self) -> i16 {
        self.ym
    }

    pub const fn yh(self) -> i16 {
        self.yh
    }

    /// Q16.16 signed fixed-point X at YL, on the "left"/low major edge.
    pub const fn xl(self) -> i32 {
        self.xl
    }

    /// Q16.16 signed fixed-point slope dXL/dY.
    pub const fn dxldy(self) -> i32 {
        self.dxldy
    }

    /// Q16.16 signed fixed-point X at YH, on the major edge.
    pub const fn xh(self) -> i32 {
        self.xh
    }

    /// Q16.16 signed fixed-point slope dXH/dY, the major edge's slope.
    pub const fn dxhdy(self) -> i32 {
        self.dxhdy
    }

    /// Q16.16 signed fixed-point X at YM, on the "middle" edge.
    pub const fn xm(self) -> i32 {
        self.xm
    }

    /// Q16.16 signed fixed-point slope dXM/dY.
    pub const fn dxmdy(self) -> i32 {
        self.dxmdy
    }

    /// The eight raw shade coefficient words, present only when
    /// `flags().shaded()`.
    pub const fn shade(&self) -> Option<&CoefficientWords> {
        self.shade.as_ref()
    }

    /// The eight raw texture coefficient words, present only when
    /// `flags().textured()`.
    pub const fn texture(&self) -> Option<&CoefficientWords> {
        self.texture.as_ref()
    }

    /// The two raw depth coefficient words, present only when
    /// `flags().depth()`.
    pub const fn depth(&self) -> Option<&DepthWords> {
        self.depth.as_ref()
    }
}

fn read_coefficient_block(words: &[RawWord], start: usize) -> CoefficientWords {
    let mut block = [RawWord { w0: 0, w1: 0 }; 8];
    block.copy_from_slice(&words[start..start + 8]);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u8 = 0x08;

    fn triangle_word(w0: u32, w1: u32) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&w0.to_be_bytes());
        bytes[4..8].copy_from_slice(&w1.to_be_bytes());
        bytes
    }

    fn base_word0(tile: u32, level: u32, right_major: bool, yl: u16, ym: u16, yh: u16) -> [u8; 8] {
        let w0 =
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(right_major) << 23 | u32::from(yl);
        let w1 = u32::from(ym) << 16 | u32::from(yh);
        triangle_word(w0, w1)
    }

    fn edge_word(x: i32, dxdy: i32) -> [u8; 8] {
        triangle_word(x as u32, dxdy as u32)
    }

    fn concat(blocks: &[[u8; 8]]) -> Vec<u8> {
        blocks.iter().flatten().copied().collect()
    }

    fn filler_block(seed: u32) -> Vec<[u8; 8]> {
        (0..8)
            .map(|index| triangle_word(seed.wrapping_add(index), seed.wrapping_add(index * 2)))
            .collect()
    }

    // --- word counts and flag decomposition, all 8 opcodes ---

    #[test]
    fn all_eight_opcodes_decompose_flags_and_word_counts_exactly() {
        let cases = [
            (0x08, false, false, false, 4u32),
            (0x09, false, false, true, 6),
            (0x0a, false, true, false, 12),
            (0x0b, false, true, true, 14),
            (0x0c, true, false, false, 12),
            (0x0d, true, false, true, 14),
            (0x0e, true, true, false, 20),
            (0x0f, true, true, true, 22),
        ];
        for (opcode, shaded, textured, depth, words) in cases {
            let flags = TriangleFlags::from_opcode(opcode);
            assert_eq!(flags.shaded(), shaded, "opcode {opcode:#04x} shaded");
            assert_eq!(flags.textured(), textured, "opcode {opcode:#04x} textured");
            assert_eq!(flags.depth(), depth, "opcode {opcode:#04x} depth");
            assert_eq!(
                triangle_word_count(flags),
                words,
                "opcode {opcode:#04x} word count"
            );
        }
    }

    /// Cross-checks every opcode's word count against
    /// `fn64_render::raw_rdp_command_width`, so the two independently
    /// authored tables cannot silently diverge.
    #[test]
    fn word_counts_match_raw_rdp_command_width_table() {
        for opcode in 0x08u8..=0x0f {
            let flags = TriangleFlags::from_opcode(opcode);
            let bytes = triangle_word_count(flags) * 8;
            assert_eq!(
                fn64_render::raw_rdp_command_width(opcode),
                Some(bytes),
                "opcode {opcode:#04x} disagrees with raw_rdp_command_width"
            );
        }
    }

    // --- base-edge decode: tile/level/flip/Y/X/slopes ---

    #[test]
    fn base_edge_command_decodes_every_field() {
        let words = concat(&[
            base_word0(5, 3, true, 0x1234, 0x5678, 0x9abc),
            edge_word(0x0011_2233, -0x0044_5566i32),
            edge_word(0x00aa_bbcc, -0x00dd_eeffi32),
            edge_word(0x0099_8877, -0x0066_5544i32),
        ]);
        let triangle = RawTriangle::decode(BASE, &words).unwrap();
        assert_eq!(triangle.tile().get(), 5);
        assert_eq!(triangle.level(), 3);
        assert!(triangle.right_major());
        assert_eq!(triangle.yl(), 0x1234);
        assert_eq!(triangle.ym(), 0x5678u16 as i16);
        assert_eq!(triangle.yh(), 0x9abcu16 as i16);
        assert_eq!(triangle.xl(), 0x0011_2233);
        assert_eq!(triangle.dxldy(), -0x0044_5566);
        assert_eq!(triangle.xh(), 0x00aa_bbccu32 as i32);
        assert_eq!(triangle.dxhdy(), -0x00dd_eeff);
        assert_eq!(triangle.xm(), 0x0099_8877u32 as i32);
        assert_eq!(triangle.dxmdy(), -0x0066_5544);
        assert!(triangle.shade().is_none());
        assert!(triangle.texture().is_none());
        assert!(triangle.depth().is_none());
    }

    #[test]
    fn right_major_flip_bit_and_tile_level_are_independent() {
        let words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        assert!(!RawTriangle::decode(BASE, &words).unwrap().right_major());

        let words = concat(&[
            base_word0(7, 7, true, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        let triangle = RawTriangle::decode(BASE, &words).unwrap();
        assert!(triangle.right_major());
        assert_eq!(triangle.tile().get(), 7);
        assert_eq!(triangle.level(), 7);
    }

    /// YL/YM/YH signed boundaries: most-negative, most-positive, and the
    /// exact sign-flip edge of the 16-bit two's-complement field.
    #[test]
    fn y_fields_cover_signed_boundaries() {
        let words = concat(&[
            base_word0(0, 0, false, 0x8000, 0x7fff, 0xffff),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        let triangle = RawTriangle::decode(BASE, &words).unwrap();
        assert_eq!(triangle.yl(), i16::MIN);
        assert_eq!(triangle.ym(), i16::MAX);
        assert_eq!(triangle.yh(), -1);
    }

    /// X/slope Q16.16 fields cover their signed i32 boundaries.
    #[test]
    fn x_and_slope_fields_cover_signed_boundaries() {
        let words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(i32::MIN, i32::MAX),
            edge_word(i32::MAX, i32::MIN),
            edge_word(-1, 1),
        ]);
        let triangle = RawTriangle::decode(BASE, &words).unwrap();
        assert_eq!(triangle.xl(), i32::MIN);
        assert_eq!(triangle.dxldy(), i32::MAX);
        assert_eq!(triangle.xh(), i32::MAX);
        assert_eq!(triangle.dxhdy(), i32::MIN);
        assert_eq!(triangle.xm(), -1);
        assert_eq!(triangle.dxmdy(), 1);
    }

    // --- optional block presence and content, every combination ---

    #[test]
    fn shaded_triangle_carries_exactly_eight_shade_words() {
        let shade_block = filler_block(0x1000_0000);
        let mut words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        words.extend(shade_block.iter().flatten().copied());
        let triangle = RawTriangle::decode(0x0c, &words).unwrap();
        let decoded_shade = triangle.shade().expect("0x0c is shaded");
        for (index, block) in shade_block.iter().enumerate() {
            let expected_w0 = u32::from_be_bytes(block[0..4].try_into().unwrap());
            let expected_w1 = u32::from_be_bytes(block[4..8].try_into().unwrap());
            assert_eq!(
                decoded_shade[index].w0, expected_w0,
                "shade word {index} w0"
            );
            assert_eq!(
                decoded_shade[index].w1, expected_w1,
                "shade word {index} w1"
            );
        }
        assert!(triangle.texture().is_none());
        assert!(triangle.depth().is_none());
    }

    #[test]
    fn textured_triangle_carries_exactly_eight_texture_words() {
        let texture_block = filler_block(0x2000_0000);
        let mut words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        words.extend(texture_block.iter().flatten().copied());
        let triangle = RawTriangle::decode(0x0a, &words).unwrap();
        assert!(triangle.shade().is_none());
        let decoded_texture = triangle.texture().expect("0x0a is textured");
        for (index, block) in texture_block.iter().enumerate() {
            let expected_w0 = u32::from_be_bytes(block[0..4].try_into().unwrap());
            assert_eq!(
                decoded_texture[index].w0, expected_w0,
                "texture word {index} w0"
            );
        }
        assert!(triangle.depth().is_none());
    }

    #[test]
    fn depth_triangle_carries_exactly_two_depth_words() {
        let depth_block = [
            triangle_word(0x3000_0001, 0x3000_0002),
            triangle_word(0x3000_0003, 0x3000_0004),
        ];
        let mut words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        words.extend(depth_block.iter().flatten().copied());
        let triangle = RawTriangle::decode(0x09, &words).unwrap();
        assert!(triangle.shade().is_none());
        assert!(triangle.texture().is_none());
        let decoded_depth = triangle.depth().expect("0x09 has depth");
        assert_eq!(decoded_depth[0].w0, 0x3000_0001);
        assert_eq!(decoded_depth[0].w1, 0x3000_0002);
        assert_eq!(decoded_depth[1].w0, 0x3000_0003);
        assert_eq!(decoded_depth[1].w1, 0x3000_0004);
    }

    /// Block order for the fully-populated opcode (0x0f) is base, shade,
    /// texture, depth -- RT64's exact `curData +=` advance order.
    #[test]
    fn fully_populated_triangle_orders_shade_then_texture_then_depth() {
        let shade_block = filler_block(0x1000_0000);
        let texture_block = filler_block(0x2000_0000);
        let depth_block = [
            triangle_word(0x3000_0001, 0x3000_0002),
            triangle_word(0x3000_0003, 0x3000_0004),
        ];
        let mut words = concat(&[
            base_word0(2, 1, false, 10, 20, 30),
            edge_word(1, 2),
            edge_word(3, 4),
            edge_word(5, 6),
        ]);
        words.extend(shade_block.iter().flatten().copied());
        words.extend(texture_block.iter().flatten().copied());
        words.extend(depth_block.iter().flatten().copied());
        assert_eq!(words.len(), 176);

        let triangle = RawTriangle::decode(0x0f, &words).unwrap();
        assert_eq!(
            triangle.shade().unwrap()[0].w0,
            u32::from_be_bytes(shade_block[0][0..4].try_into().unwrap())
        );
        assert_eq!(
            triangle.texture().unwrap()[0].w0,
            u32::from_be_bytes(texture_block[0][0..4].try_into().unwrap())
        );
        assert_eq!(triangle.depth().unwrap()[0].w0, 0x3000_0001);
    }

    // --- every opcode independently constructed end to end ---

    #[test]
    fn every_opcode_round_trips_its_own_word_count() {
        for opcode in 0x08u8..=0x0f {
            let flags = TriangleFlags::from_opcode(opcode);
            let mut words = concat(&[
                base_word0(u32::from(opcode & 0x7), 0, false, 0, 0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
            ]);
            if flags.shaded() {
                words.extend(filler_block(0x1000_0000).iter().flatten().copied());
            }
            if flags.textured() {
                words.extend(filler_block(0x2000_0000).iter().flatten().copied());
            }
            if flags.depth() {
                words.extend(
                    [
                        triangle_word(0x3000_0001, 0x3000_0002),
                        triangle_word(0x3000_0003, 0x3000_0004),
                    ]
                    .iter()
                    .flatten()
                    .copied(),
                );
            }
            let triangle = RawTriangle::decode(opcode, &words)
                .unwrap_or_else(|error| panic!("opcode {opcode:#04x}: {error}"));
            assert_eq!(triangle.shade().is_some(), flags.shaded());
            assert_eq!(triangle.texture().is_some(), flags.textured());
            assert_eq!(triangle.depth().is_some(), flags.depth());
        }
    }

    // --- truncation boundaries: one byte short, for every opcode ---

    #[test]
    fn truncation_one_byte_short_is_rejected_for_every_opcode() {
        for opcode in 0x08u8..=0x0f {
            let flags = TriangleFlags::from_opcode(opcode);
            let full_len = (triangle_word_count(flags) * 8) as usize;
            let mut words = vec![0u8; full_len];
            // Fill with a structurally valid base word so failure is purely
            // about length, not an incidental decode panic.
            words[0..8].copy_from_slice(&base_word0(0, 0, false, 0, 0, 0));
            let short = &words[..full_len - 1];
            let error = RawTriangle::decode(opcode, short).unwrap_err();
            assert_eq!(
                error,
                TriangleDecodeError::UnexpectedLength {
                    expected: full_len as u32,
                    actual: full_len as u32 - 1,
                },
                "opcode {opcode:#04x} must reject a one-byte-short slice"
            );
        }
    }

    #[test]
    fn truncation_at_every_block_boundary_is_rejected() {
        // 0x0f (fully populated, 176 bytes) truncated to each smaller
        // opcode's exact length must be rejected as 0x0f, proving a
        // truncated fully-populated command cannot be silently reinterpreted
        // as a smaller valid one.
        let shade_block = filler_block(0x1000_0000);
        let texture_block = filler_block(0x2000_0000);
        let depth_block = [
            triangle_word(0x3000_0001, 0x3000_0002),
            triangle_word(0x3000_0003, 0x3000_0004),
        ];
        let mut full = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        full.extend(shade_block.iter().flatten().copied());
        full.extend(texture_block.iter().flatten().copied());
        full.extend(depth_block.iter().flatten().copied());

        for boundary in [32usize, 48, 96, 112, 160] {
            let error = RawTriangle::decode(0x0f, &full[..boundary]).unwrap_err();
            assert_eq!(
                error,
                TriangleDecodeError::UnexpectedLength {
                    expected: 176,
                    actual: boundary as u32,
                },
                "0x0f truncated to {boundary} bytes must be rejected"
            );
        }
    }

    #[test]
    fn oversized_slice_is_also_rejected() {
        let mut words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        words.push(0);
        let error = RawTriangle::decode(BASE, &words).unwrap_err();
        assert_eq!(
            error,
            TriangleDecodeError::UnexpectedLength {
                expected: 32,
                actual: 33,
            }
        );
    }

    // --- mutation/swap/endian hostiles: parser and fixture must not share one constant source ---

    /// Swapping the byte order of the base word's low half must change the
    /// decoded Y fields -- proves the decoder actually reads big-endian, not
    /// an endian-neutral accident (e.g. a symmetric test value).
    #[test]
    fn endian_swap_of_y_half_changes_decoded_values() {
        let be = triangle_word(0, 0x1234_5678);
        let le = {
            let mut bytes = be;
            bytes[4..8].reverse();
            bytes
        };
        let words_be = concat(&[be, edge_word(0, 0), edge_word(0, 0), edge_word(0, 0)]);
        let words_le = concat(&[le, edge_word(0, 0), edge_word(0, 0), edge_word(0, 0)]);
        let be_triangle = RawTriangle::decode(BASE, &words_be).unwrap();
        let le_triangle = RawTriangle::decode(BASE, &words_le).unwrap();
        assert_ne!(be_triangle.ym(), le_triangle.ym());
        assert_ne!(be_triangle.yh(), le_triangle.yh());
    }

    /// Swapping two adjacent edge words (XL/dXLdY vs XH/dXHdY) must produce a
    /// different decode -- catches a mixed-up word index in the reader.
    #[test]
    fn swapping_adjacent_edge_words_changes_decode() {
        let ordered = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0x1111_1111, 0x2222_2222),
            edge_word(0x3333_3333, 0x4444_4444),
            edge_word(0x5555_5555, 0x6666_6666),
        ]);
        let swapped = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0x3333_3333, 0x4444_4444),
            edge_word(0x1111_1111, 0x2222_2222),
            edge_word(0x5555_5555, 0x6666_6666),
        ]);
        let ordered_triangle = RawTriangle::decode(BASE, &ordered).unwrap();
        let swapped_triangle = RawTriangle::decode(BASE, &swapped).unwrap();
        assert_ne!(ordered_triangle.xl(), swapped_triangle.xl());
        assert_ne!(ordered_triangle.xh(), swapped_triangle.xh());
    }

    /// A single mutated bit in the tile field must not silently alias
    /// another tile index -- every one of the 3 tile bits is independently
    /// significant.
    #[test]
    fn each_tile_bit_is_independently_significant() {
        for bit in 0..3 {
            let tile = 1u32 << bit;
            let words = concat(&[
                base_word0(tile, 0, false, 0, 0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
            ]);
            let triangle = RawTriangle::decode(BASE, &words).unwrap();
            assert_eq!(triangle.tile().get(), tile as u8, "tile bit {bit}");
        }
    }

    /// A single mutated bit in the level field must not silently alias
    /// another level -- every one of the 3 level bits is independently
    /// significant, and must not leak into the adjacent flip bit.
    #[test]
    fn each_level_bit_is_independently_significant_and_does_not_leak_into_flip() {
        for bit in 0..3 {
            let level = 1u32 << bit;
            let words = concat(&[
                base_word0(0, level, false, 0, 0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
                edge_word(0, 0),
            ]);
            let triangle = RawTriangle::decode(BASE, &words).unwrap();
            assert_eq!(triangle.level(), level as u8, "level bit {bit}");
            assert!(!triangle.right_major(), "level bit {bit} must not set flip");
        }
    }

    /// The flip bit must not leak into level or tile.
    #[test]
    fn flip_bit_does_not_leak_into_level_or_tile() {
        let words = concat(&[
            base_word0(0, 0, true, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        let triangle = RawTriangle::decode(BASE, &words).unwrap();
        assert_eq!(triangle.tile().get(), 0);
        assert_eq!(triangle.level(), 0);
    }

    /// `decode` must reject any opcode outside the eight triangle opcodes
    /// `0x08..=0x0f`, named at the exact boundary values adjacent to the
    /// valid range (0x07 just below, 0x10 just above), before it reads a
    /// length or any word -- an out-of-range opcode with bits 0:2 aliasing a
    /// valid flag combination (e.g. 0x38, which shares 0x08's low three
    /// bits) must not be silently accepted as if it were that opcode.
    #[test]
    fn opcode_outside_the_triangle_range_is_rejected_before_length_or_word_decode() {
        for opcode in [0x00u8, 0x07, 0x10, 0x38, 0xff] {
            let error = RawTriangle::decode(opcode, &[]).unwrap_err();
            assert_eq!(
                error,
                TriangleDecodeError::OpcodeOutOfRange { opcode },
                "opcode {opcode:#04x} must be rejected as out of range"
            );
        }
    }

    /// The two in-range boundary opcodes (0x08 and 0x0f) must still decode
    /// successfully -- the range check is inclusive on both ends.
    #[test]
    fn opcode_range_boundaries_are_inclusive() {
        let base_words = concat(&[
            base_word0(0, 0, false, 0, 0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
            edge_word(0, 0),
        ]);
        assert!(RawTriangle::decode(0x08, &base_words).is_ok());

        let mut full_words = base_words;
        full_words.extend(filler_block(0x1000_0000).iter().flatten().copied());
        full_words.extend(filler_block(0x2000_0000).iter().flatten().copied());
        full_words.extend(
            [
                triangle_word(0x3000_0001, 0x3000_0002),
                triangle_word(0x3000_0003, 0x3000_0004),
            ]
            .iter()
            .flatten()
            .copied(),
        );
        assert!(RawTriangle::decode(0x0f, &full_words).is_ok());
    }
}
