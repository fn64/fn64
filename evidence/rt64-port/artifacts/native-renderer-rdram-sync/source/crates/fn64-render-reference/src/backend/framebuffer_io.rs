use crate::raster::Framebuffer;
use crate::{
    depth, gbi, raster,
};
use fn64_render::RenderError;

use super::hidden_bits::*;

/// Load an RGBA16 color image into the software surface before ordered work
/// continues on that target. Depth is deliberately not reset: the RDP depth
/// image is independent of color-image switches and persists across tasks.
pub(super) fn load_rgba5551_framebuffer(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 2).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let pixel = view.read_u16(address);
        let hidden = read_rdram_hidden_bits(hidden_bits, address.offset(), pixel);
        let stored_coverage = (((pixel & 1) as u8) << 2) | hidden;
        let expand = |value: u16| -> u8 {
            let value = value as u8;
            (value << 3) | (value >> 2)
        };
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[
            expand((pixel >> 11) & 0x1f),
            expand((pixel >> 6) & 0x1f),
            expand((pixel >> 1) & 0x1f),
            255,
        ]);
        fb.coverage[index] = raster::Coverage::from_stored(stored_coverage);
    }
}

/// Import the active public RDP color-image format into the software working
/// surface. Public Programming Manual section 15.5, "Color Image Format,"
/// defines RGBA32 memory alpha as five alpha bits plus the three coverage bits
/// in the byte's most-significant bits.
pub(super) fn load_color_image(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) {
    let layout = target
        .layout()
        .expect("validated color image changed format");
    match layout {
        gbi::ColorImageLayout::Index8 => load_intensity8_framebuffer(rdram, target, fb),
        gbi::ColorImageLayout::Rgba16 => load_rgba5551_framebuffer(rdram, target, fb, hidden_bits),
        gbi::ColorImageLayout::Rgba32 => load_rgba8888_framebuffer(rdram, target, fb),
    }
    fb.set_color_layout(layout);
}

/// Import the public 8-bit color-image layout. Programming Manual Figure
/// 15.5.4 labels each byte as one intensity component and states that hidden
/// coverage bits are ignored for this format.
pub(super) fn load_intensity8_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let address = start
            .checked_add(u32::try_from(index).expect("I8 color-image offset exceeds u32"))
            .expect("I8 color-image logical address overflow");
        let intensity = view.read_u8(address);
        let destination = index * 4;
        fb.pixels[destination..destination + 4]
            .copy_from_slice(&[intensity, intensity, intensity, 255]);
        fb.coverage[index] = raster::Coverage::FULL;
    }
}

pub(super) fn load_rgba8888_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 4).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let [red, green, blue, alpha_coverage] = view.read_u32(address).to_be_bytes();
        let alpha5 = alpha_coverage & 0x1f;
        let alpha = (alpha5 << 3) | (alpha5 >> 2);
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[red, green, blue, alpha]);
        fb.coverage[index] = raster::Coverage::from_stored(alpha_coverage >> 5);
    }
}

/// Convert `fb`'s RGBA8888 pixels to N64 RGBA5551 and write them into
/// `rdram` starting at logical byte offset `start`, row-major with a top-left
/// origin. [`fn64_runtime::RdramViewMut`] is the sole translation from those
/// logical halfwords to N64Recomp's native-word ABI storage. A pixel whose 2
/// bytes would run past `rdram` is skipped
/// (bounds-safe; the caller already validated `output_addr` is a real
/// framebuffer offset, but a wrong width/height must not panic).
///
/// Programming Manual Chapter 15.5 specifies that the memory interface adds
/// three low dither bits and then reduces RGB from eight to five bits. The
/// rasterizer applies the public ordered matrices before this common packing
/// path and rejects only the unproven noise sequence; disabled dither remains
/// exact `>> 3` truncation. RGBA16's visible LSB is the high bit of stored
/// coverage, not retained pixel alpha; the lower two bits are committed to
/// the physical hidden-bit sidecar.
pub(super) fn write_rgba5551_framebuffer(
    rdram: &mut [u8],
    start: usize,
    fb: &Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) {
    let px_count = (fb.width * fb.height) as usize;
    // The framebuffer format is a fixed 2 bytes/pixel; only write pixels the
    // fb actually has AND that fit within rdram.
    let to_5 = |c: u8| -> u16 { u16::from(c >> 3) };
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(4),
        "RGBA5551 framebuffer base must be word-aligned, got {:#x}",
        start.offset()
    );
    let available_pixels = (rdram.len().saturating_sub(start.offset() as usize) / 2).min(px_count);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let pixel = |i: usize| {
        let src = i * 4;
        let r = fb.pixels[src];
        let g = fb.pixels[src + 1];
        let b = fb.pixels[src + 2];
        let stored_coverage = fb.coverage[i].stored();
        let px: u16 = (to_5(r) << 11)
            | (to_5(g) << 6)
            | (to_5(b) << 1)
            | u16::from((stored_coverage >> 2) & 1);
        (px, stored_coverage & 3)
    };
    let paired_pixels = available_pixels & !1;
    for i in (0..paired_pixels).step_by(2) {
        let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
        let dst = start
            .checked_add(byte_offset)
            .expect("bounded framebuffer pair address overflow");
        let (first, first_hidden) = pixel(i);
        let (second, second_hidden) = pixel(i + 1);
        let native_word = if cfg!(target_endian = "little") {
            (u32::from(first) << 16) | u32::from(second)
        } else {
            (u32::from(second) << 16) | u32::from(first)
        };
        view.write_u32(dst, native_word);
        hidden_bits.insert_pair(
            dst.offset(),
            RdramHiddenSample {
                visible: first,
                bits: first_hidden,
            },
            RdramHiddenSample {
                visible: second,
                bits: second_hidden,
            },
        );
    }
    if available_pixels != paired_pixels {
        let i = paired_pixels;
        let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
        let dst = start
            .checked_add(byte_offset)
            .expect("bounded framebuffer tail address overflow");
        let (visible, bits) = pixel(i);
        view.write_u16(dst, visible);
        write_rdram_hidden_bits(hidden_bits, dst.offset(), visible, bits);
    }
}

pub(super) fn commit_color_image(
    rdram: &mut [u8],
    target: gbi::ColorImage,
    fb: &Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) {
    match target
        .layout()
        .expect("validated color image changed format")
    {
        gbi::ColorImageLayout::Index8 => {
            write_intensity8_framebuffer(rdram, target.address as usize, fb);
            refresh_rdp_visible_halfwords_preserving_hidden(
                rdram,
                hidden_bits,
                target.address,
                fb.pixels.len() / 4,
            );
        }
        gbi::ColorImageLayout::Rgba16 => {
            write_rgba5551_framebuffer(rdram, target.address as usize, fb, hidden_bits)
        }
        gbi::ColorImageLayout::Rgba32 => {
            write_rgba8888_framebuffer(rdram, target.address as usize, fb);
            refresh_rdp_visible_halfwords_preserving_hidden(
                rdram,
                hidden_bits,
                target.address,
                fb.pixels.len(),
            );
        }
    }
}

pub(super) fn trace_ir_color_image_write(
    trace: &mut Option<Vec<(usize, usize)>>,
    target: gbi::ColorImage,
    fb: &Framebuffer,
) {
    let Some(trace) = trace.as_mut() else {
        return;
    };
    let bytes = usize::from(target.width)
        .checked_mul(fb.height as usize)
        .and_then(|pixels| {
            pixels.checked_mul(
                target
                    .layout()
                    .expect("committed color target has a validated layout")
                    .bytes_per_pixel(),
            )
        })
        .expect("validated color-image write range overflowed host usize");
    let start = target.address as usize;
    trace.push((start, start + bytes));
}

/// Commit the color pipeline's intensity component to the public one-byte
/// color-image layout. The RDP exposes no palette for this target; callers
/// program equal RGB components when the intermediate image is meaningful,
/// so the common red/intensity lane is the byte written by the memory model.
pub(super) fn write_intensity8_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("I8 framebuffer RDRAM offset exceeds u32"),
    );
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let Some(destination) = start
            .checked_add(u32::try_from(index).expect("I8 framebuffer byte offset exceeds u32"))
        else {
            break;
        };
        if destination.offset() as usize >= view.len() {
            break;
        }
        view.write_u8(destination, fb.pixels[index * 4]);
    }
}

/// Commit RGBA32 as RGB8 plus the five-bit memory alpha and three-bit coverage
/// packing defined by public Programming Manual section 15.5. Unlike RGBA16,
/// this format does not use RDRAM hidden bits.
pub(super) fn write_rgba8888_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(8),
        "RGBA8888 framebuffer base must be 64-bit aligned, got {:#x}",
        start.offset()
    );
    // Chapter 15.5 stores only five bits of alpha beside three bits of
    // coverage. As with disabled RGB dither, the supported no-alpha-dither
    // path truncates rather than rounding to the nearest expanded value.
    let to_5 = |channel: u8| -> u8 { channel >> 3 };
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let byte_offset = u32::try_from(index.checked_mul(4).expect("framebuffer size overflow"))
            .expect("framebuffer byte offset exceeds u32");
        let Some(destination) = start.checked_add(byte_offset) else {
            break;
        };
        if destination.offset() as usize + 4 > view.len() {
            break;
        }
        let source = index * 4;
        let alpha_coverage = (fb.coverage[index].stored() << 5) | to_5(fb.pixels[source + 3]);
        view.write_u32(
            destination,
            u32::from_be_bytes([
                fb.pixels[source],
                fb.pixels[source + 1],
                fb.pixels[source + 2],
                alpha_coverage,
            ]),
        );
    }
}

pub(super) fn validate_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
) -> Result<(), RenderError> {
    if !target.address.is_multiple_of(2) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG base {:#010x} is not halfword-aligned",
                target.address
            ),
        });
    }
    let byte_len = (fb.width as usize)
        .checked_mul(fb.height as usize)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

/// Load CPU-visible compressed Z and the separately owned hidden DeltaZ bits
/// into the software compare buffer. Nintendo 64 Programming Manual Chapter
/// 16, "Z Image Format" defines this 14+4 split; ordinary RDRAM reads expose
/// only the 16-bit word, so the hidden pair is maintained by physical address.
pub(super) fn load_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..fb.depth.len() {
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        let visible = view.read_u16(address);
        let encoded = depth::EncodedDepth {
            visible,
            hidden: read_rdram_hidden_bits(hidden_bits, address.offset(), visible),
        };
        fb.depth[index] = depth::unpack(encoded).0 as f32;
        fb.encoded_depth[index] = Some(encoded);
    }
    Ok(())
}

/// Commit passing Z_UPD/fill samples to both halves of RDP depth memory.
/// Samples without an encoding are left loud at their producer rather than
/// fabricated here; every current persistent producer supplies one.
pub(super) fn commit_rdp_depth_image(
    rdram: &mut [u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for (index, encoded) in fb.encoded_depth.iter().copied().enumerate() {
        let Some(encoded) = encoded else {
            continue;
        };
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        view.write_u16(address, encoded.visible);
        write_rdram_hidden_bits(
            hidden_bits,
            address.offset(),
            encoded.visible,
            encoded.hidden,
        );
    }
    Ok(())
}

pub(super) fn trace_ir_depth_image_write(
    trace: &mut Option<Vec<(usize, usize)>>,
    target: gbi::DepthImage,
    fb: &Framebuffer,
) {
    let Some(trace) = trace.as_mut() else {
        return;
    };
    for (index, encoded) in fb.encoded_depth.iter().enumerate() {
        if encoded.is_some() {
            let start = target.address as usize + index * 2;
            trace.push((start, start + 2));
        }
    }
}
