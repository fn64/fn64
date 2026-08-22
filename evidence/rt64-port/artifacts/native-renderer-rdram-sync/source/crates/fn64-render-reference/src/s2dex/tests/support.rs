// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::gbi::{CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, Vertex};
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

use crate::s2dex::*;
use crate::s2dex::object_mode::*;
use crate::s2dex::common::*;
use crate::s2dex::background::*;
use crate::s2dex::object_draw::*;
use crate::s2dex::object_ops::*;
use crate::gbi::{ConvertState, OtherMode, TextureRectangle};


pub(super) fn write_command(rdram: &mut [u8], offset: usize, w0: u32, w1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
}


pub(super) fn write_block_texture(rdram: &mut [u8], address: u32, image: u32, flag: u32) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u32(base, G_OBJLT_TXTRBLOCK);
    view.write_u32(base.checked_add(4).unwrap(), image);
    view.write_u16(base.checked_add(8).unwrap(), 0);
    view.write_u16(base.checked_add(10).unwrap(), 1); // two 64-bit words
    view.write_u16(base.checked_add(12).unwrap(), 1 << 11); // one word/row
    view.write_u16(base.checked_add(14).unwrap(), 0);
    view.write_u32(base.checked_add(16).unwrap(), flag);
    view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
}


pub(super) fn write_tile_texture(rdram: &mut [u8], address: u32, image: u32, flag: u32) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u32(base, G_OBJLT_TXTRTILE);
    view.write_u32(base.checked_add(4).unwrap(), image);
    view.write_u16(base.checked_add(8).unwrap(), 0);
    view.write_u16(base.checked_add(10).unwrap(), 3); // one word/row
    view.write_u16(base.checked_add(12).unwrap(), 7); // two rows
    view.write_u16(base.checked_add(14).unwrap(), 0);
    view.write_u32(base.checked_add(16).unwrap(), flag);
    view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
}


pub(super) fn write_tlut_texture(rdram: &mut [u8], address: u32, image: u32) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u32(base, G_OBJLT_TLUT);
    view.write_u32(base.checked_add(4).unwrap(), image);
    view.write_u16(base.checked_add(8).unwrap(), 256);
    view.write_u16(base.checked_add(10).unwrap(), 15);
    view.write_u16(base.checked_add(12).unwrap(), 0);
    view.write_u16(base.checked_add(14).unwrap(), 4);
    view.write_u32(base.checked_add(16).unwrap(), image);
    view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
}


pub(super) fn write_object_matrix(
    rdram: &mut [u8],
    address: u32,
    x: i16,
    y: i16,
    base_scale_x: u16,
    base_scale_y: u16,
) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u32(base, 1 << 16);
    view.write_u32(base.checked_add(4).unwrap(), 0);
    view.write_u32(base.checked_add(8).unwrap(), 0);
    view.write_u32(base.checked_add(12).unwrap(), 1 << 16);
    view.write_u16(base.checked_add(16).unwrap(), x as u16);
    view.write_u16(base.checked_add(18).unwrap(), y as u16);
    view.write_u16(base.checked_add(20).unwrap(), base_scale_x);
    view.write_u16(base.checked_add(22).unwrap(), base_scale_y);
}


pub(super) fn write_object_rotation_matrix(
    rdram: &mut [u8],
    address: u32,
    rotation: [i32; 4],
    x: i16,
    y: i16,
) {
    write_object_matrix(rdram, address, x, y, 1 << 10, 1 << 10);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    for (index, value) in rotation.into_iter().enumerate() {
        view.write_u32(base.checked_add((index * 4) as u32).unwrap(), value as u32);
    }
}


pub(super) fn write_object_sub_matrix(
    rdram: &mut [u8],
    address: u32,
    x: i16,
    y: i16,
    base_scale_x: u16,
    base_scale_y: u16,
) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u16(base, x as u16);
    view.write_u16(base.checked_add(2).unwrap(), y as u16);
    view.write_u16(base.checked_add(4).unwrap(), base_scale_x);
    view.write_u16(base.checked_add(6).unwrap(), base_scale_y);
}


#[allow(clippy::too_many_arguments)]
pub(super) fn write_background_common(
    rdram: &mut [u8],
    address: u32,
    image: u32,
    image_width: u16,
    image_height: u16,
    frame_width: u16,
    frame_height: u16,
    image_load: u16,
    image_size: u8,
) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u16(base, 0);
    view.write_u16(base.checked_add(2).unwrap(), image_width * 4);
    view.write_u16(base.checked_add(4).unwrap(), 0);
    view.write_u16(base.checked_add(6).unwrap(), frame_width * 4);
    view.write_u16(base.checked_add(8).unwrap(), 0);
    view.write_u16(base.checked_add(10).unwrap(), image_height * 4);
    view.write_u16(base.checked_add(12).unwrap(), 0);
    view.write_u16(base.checked_add(14).unwrap(), frame_height * 4);
    view.write_u32(base.checked_add(16).unwrap(), image);
    view.write_u16(base.checked_add(20).unwrap(), image_load);
    view.write_u8(base.checked_add(22).unwrap(), 0);
    view.write_u8(base.checked_add(23).unwrap(), image_size);
    view.write_u16(base.checked_add(24).unwrap(), 0);
    view.write_u16(base.checked_add(26).unwrap(), 0);
}


pub(super) fn write_copy_background_init(
    rdram: &mut [u8],
    address: u32,
    image_width: u16,
    frame_width: u16,
    image_load: u16,
    image_size: u8,
) {
    let shift = 4 - u32::from(image_size);
    let image_words = u32::from(image_width) >> shift;
    let frame_words = u32::from(frame_width) >> shift;
    let tmem_w = if image_load == G_BGLT_LOADBLOCK {
        image_words
    } else {
        frame_words + 1
    };
    let tmem_h = (512 / tmem_w) * 4;
    let tmem_size_w = if image_load == G_BGLT_LOADBLOCK {
        tmem_w * 2
    } else {
        image_words * 2
    };
    let tmem_size = tmem_size_w * tmem_h;
    let tmem_load_sh = if image_load == G_BGLT_LOADBLOCK {
        tmem_size / 2 - 1
    } else {
        tmem_w * 16 - 1
    };
    let tmem_load_th = if image_load == G_BGLT_LOADBLOCK {
        2047 / tmem_w + 1
    } else {
        tmem_h - 1
    };
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    for (offset, value) in [
        (28, tmem_w),
        (30, tmem_h),
        (32, tmem_load_sh),
        (34, tmem_load_th),
        (36, tmem_size_w),
        (38, tmem_size),
    ] {
        view.write_u16(base.checked_add(offset).unwrap(), value as u16);
    }
}


pub(super) fn write_background_window(
    rdram: &mut [u8],
    address: u32,
    image_x: u16,
    image_y: u16,
    flipped: bool,
) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u16(base, image_x << 5);
    view.write_u16(base.checked_add(8).unwrap(), image_y << 5);
    view.write_u16(
        base.checked_add(26).unwrap(),
        if flipped { G_BG_FLAG_FLIPS } else { 0 },
    );
}


pub(super) fn write_scale_background_tail(
    rdram: &mut [u8],
    address: u32,
    scale_w: u16,
    scale_h: u16,
    image_y_origin: i32,
) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    view.write_u16(base.checked_add(28).unwrap(), scale_w);
    view.write_u16(base.checked_add(30).unwrap(), scale_h);
    view.write_u32(base.checked_add(32).unwrap(), image_y_origin as u32);
    view.write_u32(base.checked_add(36).unwrap(), 0);
}


pub(super) fn write_sprite(rdram: &mut [u8], address: u32, width: u16, height: u16, format: u8, size: u8) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address);
    for (offset, value) in [
        (0, 0),
        (2, 1 << 10),
        (4, width << 5),
        (6, 0),
        (8, 0),
        (10, 1 << 10),
        (12, height << 5),
        (14, 0),
        (16, 1),
        (18, 0),
    ] {
        view.write_u16(base.checked_add(offset).unwrap(), value);
    }
    view.write_u8(base.checked_add(20).unwrap(), format);
    view.write_u8(base.checked_add(21).unwrap(), size);
    view.write_u8(base.checked_add(22).unwrap(), 0);
    view.write_u8(base.checked_add(23).unwrap(), 0);
}


pub(super) fn rectangle_texture(operation: &RenderOp) -> (&crate::gbi::Texture, OtherMode) {
    let RenderOp::TextureRectangle(rectangle) = operation else {
        panic!("expected texture rectangle, got {operation:?}");
    };
    (
        rectangle
            .texture
            .as_ref()
            .expect("object rectangle must bind loaded TMEM"),
        rectangle.other_mode,
    )
}
