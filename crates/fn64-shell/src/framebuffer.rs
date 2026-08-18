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

use fn64_runtime::{RdramAddr, RdramView};

/// N64 low-res NTSC framebuffer dimensions, used only as the pre-boot
/// default: the surface starts at this size and is resized to the guest's
/// own programmed geometry once VI_WIDTH and VI_V_START are latched.
///
/// **`FB_HEIGHT` is not the scanned-out line count.** The VI's active output
/// rectangle comes from V_START, and a game is free to program fewer lines
/// than 240 -- WM2000 programs 237. Rows past that rectangle were never
/// rendered into, so presenting a fixed 240 shows stale RDRAM along the
/// bottom edge. `fn64_abi::vi_output_height` is the authority; this constant
/// is only the value to use before the guest has programmed one.
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
/// destination pixels actually written.
///
/// `src_stride` is the framebuffer's real line width in pixels (VI_WIDTH,
/// from `fn64_abi::vi_width`). The source advances by `src_stride` per row
/// while `dst` stays `dst_width`-wide, so a game whose framebuffer line width
/// differs from the presented 320 no longer shears/offsets each scanline.
///
/// `dst_height` is the guest's own active output line count
/// (`fn64_abi::vi_output_height`), NOT a fixed 240. Reading past it walks
/// into rows the game never rendered, which is visible as a band of stale or
/// uninitialized pixels along the bottom of the window.
pub fn rgba5551_to_rgba8888(
    rdram: RdramView<'_>,
    start: RdramAddr,
    src_stride: usize,
    dst_width: usize,
    dst_height: usize,
    dst: &mut [u8],
) -> usize {
    debug_assert_eq!(dst.len(), dst_width * dst_height * 4);
    assert!(
        start.offset().is_multiple_of(4),
        "RGBA5551 framebuffer base must be word-aligned, got {:#x}",
        start.offset()
    );
    let src_stride = src_stride.max(1);
    // The `^ 2` halfword swizzle in read_u16 touches the whole containing
    // word, so a trailing sub-word remnant is not a readable pixel. Count
    // pixels at word granularity, matching the previous `(available/4)*2`.
    let available_pixels = (rdram.len().saturating_sub(start.offset() as usize) / 4) * 2;
    // Present the full framebuffer line when the surface is sized to it
    // (`dst_width == src_stride`); if the surface is narrower, show the left
    // `dst_width` columns of each row, still at the correct per-row offset.
    let copy_width = dst_width.min(src_stride);
    let mut written = 0;
    for row in 0..dst_height {
        let row_first = row * src_stride;
        for col in 0..copy_width {
            let i = row_first + col;
            if i >= available_pixels {
                return written; // ran off the rdram-backed region
            }
            let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
            let addr = start
                .checked_add(byte_offset)
                .expect("framebuffer logical address overflow");
            let px = rdram.read_u16(addr);
            let r5 = (px >> 11) & 0x1F;
            let g5 = (px >> 6) & 0x1F;
            let b5 = (px >> 1) & 0x1F;
            let o = (row * dst_width + col) * 4;
            dst[o] = expand5(r5);
            dst[o + 1] = expand5(g5);
            dst[o + 2] = expand5(b5);
            // N64's 1-bit alpha is coverage, not transparency for a presented
            // frame -- force opaque so the window never shows through.
            dst[o + 3] = 255;
            written += 1;
        }
    }
    written
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

/// Stable diagnostic fingerprint of the decoded RGBA frame. Unlike the old
/// `NON-BLANK` label this makes two capture paths mechanically comparable;
/// it is evidence of equality, not a claim that either image is faithful.
pub fn rgba_hash(rgba: &[u8]) -> u64 {
    rgba.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    })
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
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut buf);
        for (i, px) in pixels_in.iter().enumerate() {
            view.write_u16(RdramAddr::from_offset((i * 2) as u32), *px);
        }
        buf
    }

    fn decode(src: &[u8], dst: &mut [u8]) -> usize {
        // Default helper: stride == dst_width == FB_WIDTH (the 320 case).
        rgba5551_to_rgba8888(
            RdramView::from_storage(src),
            RdramAddr::from_offset(0),
            FB_WIDTH,
            FB_WIDTH,
            FB_HEIGHT,
            dst,
        )
    }

    #[test]
    fn pure_red_pixel_unpacks_to_ff0000ff() {
        // RGBA5551 pure red: R=31,G=0,B=0,A=1 -> 0b11111_00000_00000_1 = 0xF801.
        let src = fb_with(&[0xF801]);
        let mut dst = blank_dst();
        assert!(decode(&src, &mut dst) >= 1);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn pure_green_and_blue() {
        // Green: G=31 -> 0x07C0. Blue: B=31 -> 0x003E.
        let mut dst = blank_dst();
        decode(&fb_with(&[0x07C0]), &mut dst);
        assert_eq!(&dst[0..4], &[0, 255, 0, 255]);
        let mut dst = blank_dst();
        decode(&fb_with(&[0x003E]), &mut dst);
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
        assert!(decode(&src, &mut dst) >= 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "pixel 0 must decode red");
        assert_eq!(&dst[4..8], &[0, 0, 255, 255], "pixel 1 must decode blue");
    }

    #[test]
    fn alpha_always_opaque() {
        // Even a pixel with the 1-bit alpha clear presents opaque.
        let mut dst = blank_dst();
        decode(&fb_with(&[0xF800]), &mut dst); // red, A=0
        assert_eq!(dst[3], 255);
    }

    #[test]
    fn full_width_surface_presents_the_whole_line_at_its_real_stride() {
        // WM2000's case: stride == dst_width == 480. Row 1 begins at source
        // pixel `stride`, and the surface is wide enough to show the whole
        // line (no crop). Confirm both rows land fully and at the right offset.
        let width = 480usize;
        let mut src_px = vec![0u16; width * 2];
        src_px[0] = 0xF801; // row 0, col 0 = red
        src_px[width - 1] = 0x07C0; // row 0, last col = green (only visible if full width shown)
        src_px[width] = 0x003E; // row 1, col 0 = blue (at the real stride)
        let src = fb_with(&src_px);
        let mut dst = vec![0u8; width * FB_HEIGHT * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            width, // src_stride
            width, // dst_width (surface sized to the real width)
            FB_HEIGHT,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "row 0 col 0 red");
        let last = (width - 1) * 4;
        assert_eq!(
            &dst[last..last + 4],
            &[0, 255, 0, 255],
            "row 0 last col green -- full width presented, not cropped to 320"
        );
        let row1 = width * 4;
        assert_eq!(
            &dst[row1..row1 + 4],
            &[0, 0, 255, 255],
            "row 1 col 0 blue -- read from offset `stride`, no shear"
        );
    }

    #[test]
    fn narrow_surface_crops_but_keeps_row_alignment() {
        // If the surface stays 320 while the source is wider, each row still
        // reads from its real stride (no shear), just cropped to the left 320.
        let stride = 480usize;
        let mut src_px = vec![0u16; stride * 2];
        src_px[0] = 0xF801; // row 0 red
        src_px[stride] = 0x003E; // row 1 blue at real stride
        src_px[320] = 0x07C0; // beyond the 320 crop -- must not appear
        let src = fb_with(&src_px);
        let mut dst = vec![0u8; FB_WIDTH * FB_HEIGHT * 4];
        rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            stride,
            FB_WIDTH, // narrow surface
            FB_HEIGHT,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        let row1 = FB_WIDTH * 4;
        assert_eq!(&dst[row1..row1 + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn truncated_source_leaves_rest_black_no_panic() {
        // One word of source into a full-frame dst: pixels 0-1 set, rest
        // untouched; a sub-word remnant is skipped, never read OOB.
        let mut dst = vec![7u8; FB_WIDTH * FB_HEIGHT * 4];
        let n = decode(&fb_with(&[0xF801, 0xF801]), &mut dst);
        assert_eq!(n, 2);
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
        assert_eq!(dst[8], 7);
        // 2-byte remnant (half a word): zero pixels, no panic.
        assert_eq!(decode(&[0xF8, 0x01], &mut dst), 0);
    }

    /// The bug this pins: the presenter used to read a fixed `FB_HEIGHT`
    /// (240) rows regardless of what the guest programmed, so a game whose
    /// VI active window is shorter had rows of never-rendered RDRAM blitted
    /// into the window as an edge band.
    ///
    /// WM2000's measured V_START is `0x002501ff` -- half-lines 37..511, i.e.
    /// **237** output lines (the same decode `fn64_render::ViActiveWindow`
    /// asserts). The fixture below fills 237 rows with a known value and the
    /// three rows past them with a *different* known value, so reading one
    /// row too many is a visible wrong answer rather than a silent one.
    #[test]
    fn conversion_stops_at_the_programmed_output_height() {
        const STRIDE: usize = 480;
        const OUT_H: usize = 237; // WM2000's V_START decode
        const PAST: usize = 3; // rows the guest never rendered

        // In-image pixels are white; the rows past the active window hold a
        // distinct "stale RDRAM" value that must never be presented.
        let in_image = 0xFFFFu16;
        let stale = 0x0843u16;
        let mut src_px = vec![in_image; STRIDE * (OUT_H + PAST)];
        for px in src_px.iter_mut().skip(STRIDE * OUT_H) {
            *px = stale;
        }
        let src = fb_with(&src_px);

        let mut dst = vec![0u8; STRIDE * OUT_H * 4];
        let written = rgba5551_to_rgba8888(
            RdramView::from_storage(&src),
            RdramAddr::from_offset(0),
            STRIDE,
            STRIDE,
            OUT_H,
            &mut dst,
        );
        assert_eq!(
            written,
            STRIDE * OUT_H,
            "every pixel of the programmed rectangle must be written"
        );

        // Positive control: the fixture really does exercise the last row.
        // Without this a test that read 236 rows would also pass.
        let last_row = (OUT_H - 1) * STRIDE * 4;
        assert_eq!(
            &dst[last_row..last_row + 4],
            &[255, 255, 255, 255],
            "row {} (the last programmed line) must be present and in-image",
            OUT_H - 1
        );

        // The stale value expands to a distinct RGBA, so its absence is a
        // real assertion rather than a coincidence of two identical colors.
        let stale_rgba = [
            expand5((stale >> 11) & 0x1F),
            expand5((stale >> 6) & 0x1F),
            expand5((stale >> 1) & 0x1F),
            255,
        ];
        assert_ne!(
            stale_rgba,
            [255, 255, 255, 255],
            "fixture is only meaningful if the stale rows differ from the image"
        );
        assert!(
            !dst.chunks_exact(4).any(|px| px == stale_rgba),
            "no pixel past the programmed {OUT_H}-line output rectangle may be presented"
        );
    }

    /// The other half of the same defect: the presenter must read from
    /// VI_ORIGIN, and a base one row low shifts the whole image and drags a
    /// never-rendered row in at the bottom. This pins the arithmetic that
    /// makes the two bases differ by exactly one line.
    #[test]
    fn a_base_one_row_low_shifts_every_presented_row() {
        const STRIDE: usize = 480;
        const OUT_H: usize = 8;

        // Row r is filled with the value r+1, so a one-row shift is visible
        // as an off-by-one in the decoded red channel rather than as noise.
        let mut src_px = vec![0u16; STRIDE * (OUT_H + 1)];
        for row in 0..=OUT_H {
            let shade = (row as u16 + 1) & 0x1F;
            for col in 0..STRIDE {
                src_px[row * STRIDE + col] = (shade << 11) | 1;
            }
        }
        let src = fb_with(&src_px);

        let read_at = |start_pixels: usize| {
            let mut dst = vec![0u8; STRIDE * OUT_H * 4];
            rgba5551_to_rgba8888(
                RdramView::from_storage(&src),
                RdramAddr::from_offset((start_pixels * 2) as u32),
                STRIDE,
                STRIDE,
                OUT_H,
                &mut dst,
            );
            dst
        };

        let correct = read_at(STRIDE); // VI_ORIGIN: one row into the buffer
        let one_row_low = read_at(0); // the buffer base the RDP renders to

        assert_eq!(
            correct[0],
            expand5(2),
            "reading from VI_ORIGIN must start at the second stored row"
        );
        assert_eq!(
            one_row_low[0],
            expand5(1),
            "reading from the render base starts one row earlier"
        );
        assert_ne!(
            correct, one_row_low,
            "a one-row base error must change the presented image"
        );
    }

    #[test]
    fn uniform_detects_blank() {
        assert!(is_uniform(&[0, 0, 0, 0]));
        assert!(is_uniform(&[]));
        assert!(!is_uniform(&[0, 0, 1, 0]));
    }

    #[test]
    fn rgba_hash_is_stable_and_content_sensitive() {
        assert_eq!(rgba_hash(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(rgba_hash(&[0, 1, 2, 3]), 0x4475_327f_98e0_5411);
        assert_ne!(rgba_hash(&[0, 1, 2, 3]), rgba_hash(&[0, 1, 3, 2]));
    }
}
