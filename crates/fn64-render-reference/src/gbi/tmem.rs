// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use super::types::Texture;
use super::state::*;

// --- Texture format decode (F3DEX2-CONCEPTS.md §5.1) --------------------
//
// Format/size selector values: OoT `include/ultra64/gbi.h:331-378`.
// Texel bit layouts and channel expansion: RT64 (MIT)
// `src/shaders/Formats.hlsli:56-119` and
// `src/shaders/TextureDecoder.hlsli:30-120,149-204`.

/// RDP image formats (`G_IM_FMT_*`) as encoded in the SETTIMG/SETTILE
/// format field.
pub(super) const G_IM_FMT_RGBA: u8 = 0;
pub(super) const G_IM_FMT_YUV: u8 = 1;
pub(super) const G_IM_FMT_CI: u8 = 2;
pub(super) const G_IM_FMT_IA: u8 = 3;
pub(super) const G_IM_FMT_I: u8 = 4;

/// Pixel sizes (`G_IM_SIZ_*`): 4/8/16/32 bits-per-texel selectors.
pub(super) const G_IM_SIZ_4B: u8 = 0;
pub(super) const G_IM_SIZ_8B: u8 = 1;
pub(super) const G_IM_SIZ_16B: u8 = 2;
pub(super) const G_IM_SIZ_32B: u8 = 3;

/// Expand a 16-bit RGBA5551 texel to RGBA8888 (5/5/5/1, big-endian).
/// RT64 `Formats.hlsli:83-92` gives the exact shifts and 5-to-8 replication;
/// OoT `gbi.h:334,345` identifies this as `G_IM_FMT_RGBA/G_IM_SIZ_16b`.
#[inline]
pub(super) fn rgba5551_to_rgba8888(px: u16) -> [u8; 4] {
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
pub(super) fn ia16_to_rgba8888(hi: u8, lo: u8) -> [u8; 4] {
    [hi, hi, hi, lo]
}

/// Expand IA8 (4-bit intensity, 4-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:75-80` (`gbi.h:337,344`).
#[inline]
pub(super) fn ia8_to_rgba8888(byte: u8) -> [u8; 4] {
    let i4 = byte >> 4;
    let a4 = byte & 0x0F;
    let i = (i4 << 4) | i4;
    let a = (a4 << 4) | a4;
    [i, i, i, a]
}

/// Expand IA4 (3-bit intensity, 1-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:61-64` (`gbi.h:337,343`).
#[inline]
pub(super) fn ia4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i3 = (nibble >> 1) & 0x07;
    // Exact 3-to-8 replication: abc -> abcabcab.
    let i = (i3 << 5) | (i3 << 2) | (i3 >> 1);
    [i, i, i, if nibble & 1 != 0 { 255 } else { 0 }]
}

/// Expand I8 (8-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:71-73` (`gbi.h:338,344`).
#[inline]
pub(super) fn i8_to_rgba8888(byte: u8) -> [u8; 4] {
    [byte, byte, byte, byte]
}

/// Expand I4 (4-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:56-59` (`gbi.h:338,343`).
#[inline]
pub(super) fn i4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i = (nibble << 4) | nibble;
    [i, i, i, i]
}

/// Select one 4-bit texel from a packed byte. RT64
/// `TextureDecoder.hlsli:170-172` selects the high nibble for even columns
/// and the low nibble for odd columns.
#[inline]
pub(super) fn packed_nibble(byte: u8, texel_index: usize) -> u8 {
    if texel_index & 1 == 0 {
        byte >> 4
    } else {
        byte & 0x0F
    }
}

/// Decode `G_LOADTLUT`'s 10-bit count field. Public `gbi.h` packs
/// `count - 1` directly at bits 14..23; the low two bits are part of the
/// count, not fixed-point padding.
pub(super) fn load_tlut_count(w1: u32) -> usize {
    let count = ((w1 >> 14) & 0x3ff) as usize + 1;
    assert!(
        count <= 256,
        "G_LOADTLUT requested {count} entries, exceeding the 256-entry TLUT"
    );
    count
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TextureLoad {
    Block,
    Tile { source_x: u32, source_y: u32 },
}

pub(super) fn source_texel(rdram: &[u8], base: usize, index: usize, size: u8) -> u32 {
    match size {
        G_IM_SIZ_4B => {
            let byte = read_u8(rdram, base + index / 2);
            u32::from(packed_nibble(byte, index))
        }
        G_IM_SIZ_8B => u32::from(read_u8(rdram, base + index)),
        G_IM_SIZ_16B => u32::from(read_u16(rdram, base + index * 2)),
        G_IM_SIZ_32B => {
            let offset = base + index * 4;
            u32::from_be_bytes([
                read_u8(rdram, offset),
                read_u8(rdram, offset + 1),
                read_u8(rdram, offset + 2),
                read_u8(rdram, offset + 3),
            ])
        }
        _ => unreachable!("RDP image size is a two-bit field"),
    }
}

pub(super) fn assert_texture_source_range(
    rdram: &[u8],
    base: usize,
    last_index: usize,
    size: u8,
    command: &str,
) {
    let last_byte = match size {
        G_IM_SIZ_4B => last_index / 2,
        G_IM_SIZ_8B => last_index,
        G_IM_SIZ_16B => last_index * 2 + 1,
        G_IM_SIZ_32B => last_index * 4 + 3,
        _ => unreachable!("RDP image size is a two-bit field"),
    };
    assert!(
        base.checked_add(last_byte)
            .is_some_and(|end| end < rdram.len()),
        "{command} source texel {last_index} exceeds RDRAM length {:#x}",
        rdram.len()
    );
}

pub(super) fn load_tile_into_tmem(
    rdram: &[u8],
    tex: &mut TexState,
    segments: &[u32; 16],
    tile_index: usize,
    w0: u32,
    w1: u32,
) {
    let raw_source_x = ((w0 >> 12) & 0x0fff) as usize;
    let raw_source_y = (w0 & 0x0fff) as usize;
    let raw_high_x = ((w1 >> 12) & 0x0fff) as usize;
    let raw_high_y = (w1 & 0x0fff) as usize;
    // SGI RDP Command Summary Table 7 says equal L/H fractions are the usual
    // subpixel-offset form, not a validity requirement. The integer parts
    // select the inclusive DRAM texel span while all raw quarters remain in
    // the tile descriptor for later sampling/clamping.
    let source_x = raw_source_x / 4;
    let source_y = raw_source_y / 4;
    let high_x = raw_high_x / 4;
    let high_y = raw_high_y / 4;
    assert!(
        high_x >= source_x && high_y >= source_y,
        "G_LOADTILE has inverted source bounds ({source_x}, {source_y})..=({high_x}, {high_y})"
    );
    assert_ne!(
        tex.timg_width, 0,
        "G_LOADTILE decoded before G_SETTIMG latched a source width"
    );
    let width = high_x - source_x + 1;
    let height = high_y - source_y + 1;
    let base = resolve_addr(segments, tex.timg_addr);
    let last_index = (high_y * usize::from(tex.timg_width)) + high_x;
    assert_texture_source_range(rdram, base, last_index, tex.timg_siz, "G_LOADTILE");
    let tile = tex.tiles[tile_index];
    if tex.timg_fmt == G_IM_FMT_YUV {
        assert_eq!(
            (tile.fmt, tile.siz),
            (G_IM_FMT_YUV, G_IM_SIZ_16B),
            "YUV G_LOADTILE requires a YUV16 load descriptor"
        );
        assert!(
            source_x.is_multiple_of(2) && width.is_multiple_of(2),
            "YUV G_LOADTILE requires an even S origin and width"
        );
        for y in 0..height {
            let source_row = source_y + y;
            for pair in 0..width / 2 {
                let index = source_row * usize::from(tex.timg_width) + source_x + pair * 2;
                let offset = base + index * 2;
                std::rc::Rc::make_mut(&mut tex.tmem).write_yuv_pair(
                    tile,
                    pair,
                    y,
                    source_row & 1 != 0,
                    [
                        read_u8(rdram, offset),
                        read_u8(rdram, offset + 1),
                        read_u8(rdram, offset + 2),
                        read_u8(rdram, offset + 3),
                    ],
                );
            }
        }
    } else {
        for y in 0..height {
            let source_row = source_y + y;
            for x in 0..width {
                let index = source_row * usize::from(tex.timg_width) + source_x + x;
                let value = source_texel(rdram, base, index, tex.timg_siz);
                // The same Table 7 usage notes allow image and load-tile
                // sizes to differ. DRAM addressing and transferred bit width
                // belong to G_SETTIMG; the tile still owns TMEM base/line.
                std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
                    tile,
                    x,
                    y,
                    source_row & 1 != 0,
                    tex.timg_siz,
                    value,
                );
            }
        }
    }

    let tile = &mut tex.tiles[tile_index];
    tile.uls = ((w0 >> 12) & 0x0fff) as u16;
    tile.ult = (w0 & 0x0fff) as u16;
    tile.lrs = ((w1 >> 12) & 0x0fff) as u16;
    tile.lrt = (w1 & 0x0fff) as u16;
}

pub(super) fn load_block_into_tmem(
    rdram: &[u8],
    tex: &mut TexState,
    segments: &[u32; 16],
    tile_index: usize,
    w0: u32,
    w1: u32,
) {
    let source_s = ((w0 >> 12) & 0x0fff) as usize;
    let source_t = (w0 & 0x0fff) as usize;
    let high_s = ((w1 >> 12) & 0x0fff) as usize;
    let dxt = (w1 & 0x0fff) as usize;
    assert!(
        high_s >= source_s,
        "G_LOADBLOCK has inverted source span {source_s}..={high_s}"
    );
    assert_ne!(
        tex.timg_width, 0,
        "G_LOADBLOCK decoded before G_SETTIMG latched a source width"
    );

    let count = high_s - source_s + 1;
    let start = source_t * usize::from(tex.timg_width) + source_s;
    let base = resolve_addr(segments, tex.timg_addr);
    assert_texture_source_range(rdram, base, start + count - 1, tex.timg_siz, "G_LOADBLOCK");
    let tile = tex.tiles[tile_index];
    if tex.timg_fmt == G_IM_FMT_YUV {
        assert_eq!(
            (tile.fmt, tile.siz),
            (G_IM_FMT_YUV, G_IM_SIZ_16B),
            "YUV G_LOADBLOCK requires a YUV16 load descriptor"
        );
        assert!(
            start.is_multiple_of(2) && count.is_multiple_of(2),
            "YUV G_LOADBLOCK requires an even source origin and texel count"
        );
        for offset in (0..count).step_by(2) {
            let word = offset / 8;
            let t_advance = (word * dxt) >> 11;
            let destination_word =
                usize::from(tile.tmem) + word + t_advance * usize::from(tile.line);
            let destination = Tile {
                tmem: (destination_word & 0x01ff) as u16,
                line: 0,
                ..tile
            };
            let source_offset = base + (start + offset) * 2;
            std::rc::Rc::make_mut(&mut tex.tmem).write_yuv_pair(
                destination,
                (offset % 8) / 2,
                0,
                (source_t + t_advance) & 1 != 0,
                [
                    read_u8(rdram, source_offset),
                    read_u8(rdram, source_offset + 1),
                    read_u8(rdram, source_offset + 2),
                    read_u8(rdram, source_offset + 3),
                ],
            );
        }
    } else {
        // Table 8 defines DXT stepping per transferred 64-bit word, so the
        // number of command texels per word comes from the source image size,
        // not a deliberately mismatched load descriptor.
        let texels_per_word = 16usize >> tex.timg_siz;
        for offset in 0..count {
            let word = offset / texels_per_word;
            let t_advance = (word * dxt) >> 11;
            let destination_word =
                usize::from(tile.tmem) + word + t_advance * usize::from(tile.line);
            let destination = Tile {
                tmem: (destination_word & 0x01ff) as u16,
                line: 0,
                ult: ((source_t + t_advance) as u16).wrapping_mul(4),
                ..tile
            };
            let value = source_texel(rdram, base, start + offset, tex.timg_siz);
            std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
                destination,
                offset % texels_per_word,
                0,
                (source_t + t_advance) & 1 != 0,
                tex.timg_siz,
                value,
            );
        }
    }

    let tile = &mut tex.tiles[tile_index];
    tile.uls = (source_s as u16) & 0x0fff;
    tile.ult = (source_t as u16) & 0x0fff;
    tile.lrs = (high_s as u16) & 0x0fff;
    tile.lrt = (dxt as u16) & 0x0fff;
}

#[cfg(test)]
pub(super) fn palette_color(tlut: &[[u8; 4]], index: usize, format: &str) -> [u8; 4] {
    *tlut.get(index).unwrap_or_else(|| {
        panic!(
            "{format} texel index {index} exceeds the loaded {}-entry TLUT",
            tlut.len()
        )
    })
}

/// Test-only direct decoder retained as a format-conversion oracle. Production
/// display-list execution loads and samples physical [`Tmem`] instead.
/// Decode the texture bound to `tile` from the latched `G_SETTIMG` image out
/// of RDRAM into an RGBA8888 [`Texture`], sized by the tile's
/// `G_SETTILESIZE` extent. Unsupported dimensions or formats trap by their
/// decoded fields; a texture request must never degrade into flat shading.
/// Covers the common OoT formats: RGBA16/32, RGBA4/8 hardware
/// aliases, IA16/IA8/IA4, I8/I4, and CI8/CI4 (via the loaded TLUT).
///
/// This helper deliberately bypasses TMEM so individual conversion tests can
/// isolate one source format without constructing a command stream.
#[cfg(test)]
pub(super) fn decode_current_texture(
    rdram: &[u8],
    tex: &TexState,
    segments: &[u32; 16],
    tile: usize,
    load: TextureLoad,
) -> Texture {
    let t = &tex.tiles[tile];
    // Tile extent from SETTILESIZE (S10.5 -> ÷4 texels), inclusive bounds.
    let (uls, ult, lrs, lrt) = (t.uls / 4, t.ult / 4, t.lrs / 4, t.lrt / 4);
    assert!(
        lrs >= uls && lrt >= ult,
        "texture tile {tile} has reversed extent ({uls}, {ult})..({lrs}, {lrt})"
    );
    let w = u32::from(lrs - uls + 1);
    let h = u32::from(lrt - ult + 1);
    assert!(
        w != 0 && h != 0 && w <= 1024 && h <= 1024,
        "texture tile {tile} has unsupported extent {w}x{h}"
    );
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
                (G_IM_FMT_YUV, G_IM_SIZ_16B) => {
                    // SGI RDP Command Summary, Set Tile/Load Tile notes:
                    // YUV images are byte-interleaved Y0,U,Y1,V and each
                    // adjacent Y pair shares its U/V chroma samples.
                    let pair = source_index / 2;
                    let pair_base = base + pair * 4;
                    let y_offset = if source_index & 1 == 0 { 0 } else { 2 };
                    [
                        read_u8(rdram, pair_base + y_offset),
                        read_u8(rdram, pair_base + 1),
                        read_u8(rdram, pair_base + 3),
                        255,
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
                _ => panic!("texture tile {tile} uses unsupported format {fmt} size {siz}"),
            };
            let o = texel_index * 4;
            texels[o..o + 4].copy_from_slice(&rgba);
        }
    }

    Texture {
        format: t.fmt,
        size: t.siz,
        width: w,
        height: h,
        texels: std::rc::Rc::new(texels),
        clamp_s: t.clamp_s,
        clamp_t: t.clamp_t,
        mirror_s: t.mirror_s,
        mirror_t: t.mirror_t,
        mask_s: t.mask_s,
        mask_t: t.mask_t,
        shift_s: t.shift_s,
        shift_t: t.shift_t,
        origin_s: t.uls as f32 / 4.0,
        origin_t: t.ult as f32 / 4.0,
        tmem: None,
        lod: None,
    }
}
