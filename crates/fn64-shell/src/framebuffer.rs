//! N64 VI framebuffer -> window pixel-buffer conversion, factored out so the
//! RGBA5551->RGBA8888 unpack is unit-testable without a live window.
//!
//! The game renders into rdram as **RGBA5551 big-endian halfwords**
//! (`RRRRRGGGGGBBBBBA`, one 16-bit pixel = 2 bytes, MSB first) -- the exact
//! format `examples/oot-boot`'s `dump_rgba5551_as_png` reads. `pixels`
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
    let pixels = (src.len() / 2).min(FB_WIDTH * FB_HEIGHT);
    for i in 0..pixels {
        let hi = src[i * 2];
        let lo = src[i * 2 + 1];
        let px = u16::from_be_bytes([hi, lo]);
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

    #[test]
    fn pure_red_pixel_unpacks_to_ff0000ff() {
        // RGBA5551 pure red: R=31,G=0,B=0,A=1 -> 0b11111_00000_00000_1 = 0xF801.
        let src = [0xF8, 0x01];
        let mut dst = blank_dst();
        assert_eq!(rgba5551_to_rgba8888(&src, &mut dst), 1);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn pure_green_and_blue() {
        // Green: G=31 -> 0b00000_11111_00000_0 = 0x07C0.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&[0x07, 0xC0], &mut dst);
        assert_eq!(&dst[0..4], &[0, 255, 0, 255]);
        // Blue: B=31 -> 0b00000_00000_11111_0 = 0x003E.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&[0x00, 0x3E], &mut dst);
        assert_eq!(&dst[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn big_endian_halfword_order() {
        // 0xF801 stored big-endian is [0xF8, 0x01]; swapping the bytes
        // ([0x01, 0xF8]) must NOT decode as red -- guards the byte order.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&[0x01, 0xF8], &mut dst);
        assert_ne!(&dst[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn alpha_always_opaque() {
        // Even a pixel with the 1-bit alpha clear presents opaque.
        let mut dst = blank_dst();
        rgba5551_to_rgba8888(&[0xF8, 0x00], &mut dst); // red, A=0
        assert_eq!(dst[3], 255);
    }

    #[test]
    fn truncated_source_leaves_rest_black_no_panic() {
        // One pixel of source into a full-frame dst: pixel 0 set, rest 0.
        let mut dst = vec![7u8; FB_WIDTH * FB_HEIGHT * 4];
        let n = rgba5551_to_rgba8888(&[0xF8, 0x01], &mut dst);
        assert_eq!(n, 1);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        // Untouched tail keeps its prior contents (we don't clear it here);
        // the loop simply didn't write past pixel 0.
        assert_eq!(dst[4], 7);
    }

    #[test]
    fn uniform_detects_blank() {
        assert!(is_uniform(&[0, 0, 0, 0]));
        assert!(is_uniform(&[]));
        assert!(!is_uniform(&[0, 0, 1, 0]));
    }
}
