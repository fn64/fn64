//! N64 VI framebuffer -> window pixel-buffer conversion, factored out so the
//! RGBA5551->RGBA8888 unpack is unit-testable without a live window.
//!
//! The game's pixels are logical **RGBA5551 halfwords** (`RRRRRGGGGGBBBBBA`),
//! but fn64's rdram is native-endian-WORD storage: pixel `i`'s halfword
//! lives at byte offset `(2*i) ^ 2` within a word-aligned framebuffer and is
//! read native-endian -- the exact rule `examples/oot-boot`'s (fn64#1-fixed)
//! `dump_rgba5551_as_png` and the runtime's `MEM_H` accessors use. Decoding
//! flat big-endian instead scrambles the halfword pair inside every 32-bit
//! word: colors shift fields (green tint) and neighboring pixels interleave
//! (pixel noise). `pixels`
//! (wgpu's `Rgba8UnormSrgb` texture) wants a tightly-packed RGBA8888 buffer,
//! one byte each R,G,B,A in that order. This converts one into the other.

/// N64 low-res NTSC framebuffer dimensions. Matches oot-boot's
/// `capture_framebuffer` assumption; this shell does not yet decode the
/// ROM's real `OSViMode` mode table (same limitation `fn64_runtime::vi`
/// documents), so a fixed 320x240 is the honest default.
pub const FB_WIDTH: usize = 320;
pub const FB_HEIGHT: usize = 240;
/// RGBA5551 is 2 bytes per pixel.
pub const FB_BYTES: usize = FB_WIDTH * FB_HEIGHT * 2;

/// Expand a 5-bit channel (0..=31) to 8-bit (0..=255) with rounding, the same
/// `(v*255+15)/31` expansion oot-boot uses -- so a byte-for-byte identical
/// image to the PNG dumps.
#[inline]
fn expand5(v: u16) -> u8 {
    ((v * 255 + 15) / 31) as u8
}

/// Convert one N64 RGBA5551 framebuffer region into `dst` as RGBA8888.
///
/// `dst` must be exactly `FB_WIDTH * FB_HEIGHT * 4` bytes (the size of
/// `pixels`' frame buffer for a 320x240 surface). `src` is the raw rdram
/// slice at the VI framebuffer offset; if it's shorter than [`FB_BYTES`]
/// (e.g. a truncated capture near an rdram bound), the missing pixels are
/// left black rather than reading out of bounds. Returns the number of
/// source pixels actually converted.
pub fn rgba5551_to_rgba8888(src: &[u8], dst: &mut [u8]) -> usize {
    debug_assert_eq!(dst.len(), FB_WIDTH * FB_HEIGHT * 4);
    // Whole words only: the `^ 2` byte-lane rule pairs halfwords within a
    // 4-byte word, so a trailing 2-byte remnant has no in-bounds partner.
    let pixels = ((src.len() / 4) * 2).min(FB_WIDTH * FB_HEIGHT);
    for i in 0..pixels {
        // Pixel `i` lives at `(2*i) ^ 2` in native-word rdram storage (the
        // caller passes a word-aligned framebuffer slice), read native-endian.
        let at = (i * 2) ^ 2;
        let px = u16::from_ne_bytes([src[at], src[at + 1]]);
        let r5 = (px >> 11) & 0x1F;
        let g5 = (px >> 6) & 0x1F;
        let b5 = (px >> 1) & 0x1F;
        let a1 = px & 0x1;
        let o = i * 4;
        dst[o] = expand5(r5);
        dst[o + 1] = expand5(g5);
        dst[o + 2] = expand5(b5);
        // N64's 1-bit alpha is coverage, not transparency for a presented
        // frame -- force opaque so the window never shows through.
        dst[o + 3] = 255;
        let _ = a1;
    }
    pixels
}

/// True if every byte in `region` is identical -- a blank/uniform frame the
/// game hasn't rendered into yet. Mirrors oot-boot's `uniform` check so the
/// shell can report "blank" honestly instead of implying content.
pub fn is_uniform(region: &[u8]) -> bool {
    match region.first() {
        Some(&first) => region.iter().all(|&b| b == first),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_dst() -> Vec<u8> {
        vec![0u8; FB_WIDTH * FB_HEIGHT * 4]
    }

    /// Build a word-aligned framebuffer holding `px` values at pixels 0..n,
    /// in fn64's native-word storage: pixel i at byte `(2*i) ^ 2`, native-endian.
    fn fb_with(pixels_in: &[u16]) -> Vec<u8> {
        let words = pixels_in.len().div_ceil(2);
        let mut buf = vec![0u8; words * 4];
        for (i, px) in pixels_in.iter().enumerate() {
            let at = (i * 2) ^ 2;
            buf[at..at + 2].copy_from_slice(&px.to_ne_bytes());
        }
        buf
    }

    #[test]
    fn pure_red_pixel_unpacks_to_ff0000ff() {
        // RGBA5551 pure red: R=31,G=0,B=0,A=1 -> 0b11111_00000_00000_1 = 0xF801.
        let src = fb_with(&[0xF801]);
        let mut dst = blank_dst();
        assert!(rgba5551_to_rgba8888(&src, &mut dst) >= 1);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn pure_green_and_blue() {
        // Green: G=31 -> 0x07C0. Blue: B=31 -> 0x003E.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&fb_with(&[0x07C0]), &mut dst);
        assert_eq!(&dst[0..4], &[0, 255, 0, 255]);
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&fb_with(&[0x003E]), &mut dst);
        assert_eq!(&dst[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn word_swizzle_pairs_pixels_correctly() {
        // Regression for the green-tinted/noisy N64 logo: pixels 0 and 1
        // (red then blue) share one word with their halfwords SWAPPED in
        // storage. A flat big-endian walk decodes them in the wrong order
        // and with scrambled fields; the `^ 2` native read must yield
        // red at pixel 0 and blue at pixel 1.
        let src = fb_with(&[0xF801, 0x003E]);
        let mut dst = blank_dst();
        assert!(rgba5551_to_rgba8888(&src, &mut dst) >= 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "pixel 0 must decode red");
        assert_eq!(&dst[4..8], &[0, 0, 255, 255], "pixel 1 must decode blue");
    }

    #[test]
    fn alpha_always_opaque() {
        // Even a pixel with the 1-bit alpha clear presents opaque.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&fb_with(&[0xF800]), &mut dst); // red, A=0
        assert_eq!(dst[3], 255);
    }

    #[test]
    fn truncated_source_leaves_rest_black_no_panic() {
        // One word of source into a full-frame dst: pixels 0-1 set, rest
        // untouched; a sub-word remnant is skipped, never read OOB.
        let mut dst = vec![7u8; FB_WIDTH * FB_HEIGHT * 4];
        let n = rgba5551_to_rgba8888(&fb_with(&[0xF801, 0xF801]), &mut dst);
        assert_eq!(n, 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        assert_eq!(dst[8], 7);
        // 2-byte remnant (half a word): zero pixels, no panic.
        assert_eq!(rgba5551_to_rgba8888(&[0xF8, 0x01], &mut dst), 0);
    }

    #[test]
    fn uniform_detects_blank() {
        assert!(is_uniform(&[0, 0, 0, 0]));
        assert!(is_uniform(&[]));
        assert!(!is_uniform(&[0, 0, 1, 0]));
    }
}
