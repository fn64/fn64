//! Minimal, content-admitted S2DEX decoder.
//!
//! This slice implements public legacy S2DEX and S2DEX2 backgrounds, object
//! rectangles, matrix, rotating sprite, and object texture-load wire forms.
//! Exact admitted microcode identity selects the colliding GBI envelope. Loads
//! use the existing raw-RDP TMEM path; draws lower through existing rectangle/
//! triangle paths.

use crate::gbi::{CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, Vertex};
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

#[cfg(test)]
pub const SUPPORTED: &[UcodeId] = &[UcodeId::S2dex, UcodeId::S2dex2];
pub use fn64_render::S2dexWireFamily;
#[cfg(test)]
pub type UcodeCatalog = fn64_render::S2dexUcodeCatalog;

pub(super) const G_OBJ_RECTANGLE: u8 = 0x01;
pub(super) const G_OBJ_SPRITE: u8 = 0x02;
pub(super) const G_SELECT_DL: u8 = 0x04;
pub(super) const G_OBJ_LOADTXTR: u8 = 0x05;
pub(super) const G_OBJ_LDTX_SPRITE: u8 = 0x06;
pub(super) const G_OBJ_LDTX_RECT: u8 = 0x07;
pub(super) const G_OBJ_LDTX_RECT_R: u8 = 0x08;
pub(super) const G_BG_1CYC: u8 = 0x09;
pub(super) const G_BG_COPY: u8 = 0x0a;
pub(super) const G_OBJ_RENDERMODE: u8 = 0x0b;
pub(super) const G_OBJ_RECTANGLE_R: u8 = 0xda;
pub(super) const G_MOVEWORD: u8 = 0xdb;
pub(super) const G_OBJ_MOVEMEM: u8 = 0xdc;
pub(super) const G_RDPHALF_0: u8 = 0xe4;
pub(super) const G_ENDDL: u8 = 0xdf;

pub(super) const S2DEX_G_BG_1CYC: u8 = 0x01;
pub(super) const S2DEX_G_BG_COPY: u8 = 0x02;
pub(super) const S2DEX_G_OBJ_RECTANGLE: u8 = 0x03;
pub(super) const S2DEX_G_OBJ_SPRITE: u8 = 0x04;
pub(super) const S2DEX_G_OBJ_MOVEMEM: u8 = 0x05;
pub(super) const S2DEX_G_SELECT_DL: u8 = 0xb0;
pub(super) const S2DEX_G_OBJ_RENDERMODE: u8 = 0xb1;
pub(super) const S2DEX_G_OBJ_RECTANGLE_R: u8 = 0xb2;
pub(super) const S2DEX_G_ENDDL: u8 = 0xb8;
pub(super) const S2DEX_G_MOVEWORD: u8 = 0xbc;
pub(super) const S2DEX_G_OBJ_LOADTXTR: u8 = 0xc1;
pub(super) const S2DEX_G_OBJ_LDTX_SPRITE: u8 = 0xc2;
pub(super) const S2DEX_G_OBJ_LDTX_RECT: u8 = 0xc3;
pub(super) const S2DEX_G_OBJ_LDTX_RECT_R: u8 = 0xc4;

pub(super) const MAX_COMMANDS: usize = 1 << 20;
pub(super) const MAX_DL_DEPTH: usize = 18;
pub(super) const PHYSICAL_RDRAM_BYTES: usize = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
pub(super) const OBJ_SPRITE_BYTES: usize = 24;
pub(super) const OBJ_TEXTURE_BYTES: usize = 24;
pub(super) const OBJ_TX_SPRITE_BYTES: usize = OBJ_TEXTURE_BYTES + OBJ_SPRITE_BYTES;
pub(super) const OBJ_BG_BYTES: usize = 40;
pub(super) const OBJECT_TEXTURE_SCRATCH_BYTES: usize = 4096 + 40;
pub(super) const BACKGROUND_SCRATCH_BYTES: usize = 8192 + 48;

pub(super) const G_BGLT_LOADBLOCK: u16 = 0x0033;
pub(super) const G_BGLT_LOADTILE: u16 = 0xfff4;
pub(super) const G_BG_FLAG_FLIPS: u16 = 1;

pub(super) const G_OBJLT_TXTRBLOCK: u32 = 0x0000_1033;
pub(super) const G_OBJLT_TXTRTILE: u32 = 0x00fc_1034;
pub(super) const G_OBJLT_TLUT: u32 = 0x0000_0030;

pub(super) const G_MW_SEGMENT: u8 = 0x06;
pub(super) const G_MW_GENSTAT: u8 = 0x08;

pub(super) const G_OBJ_FLAG_FLIPS: u8 = 1 << 0;
pub(super) const G_OBJ_FLAG_FLIPT: u8 = 1 << 4;
pub(super) const G_OBJRM_NOTXCLAMP: u32 = 0x01;
pub(super) const G_OBJRM_XLU: u32 = 0x02;
pub(super) const G_OBJRM_ANTIALIAS: u32 = 0x04;
pub(super) const G_OBJRM_BILERP: u32 = 0x08;
pub(super) const G_OBJRM_SHRINKSIZE_1: u32 = 0x10;
pub(super) const G_OBJRM_SHRINKSIZE_2: u32 = 0x20;
pub(super) const G_OBJRM_WIDEN: u32 = 0x40;
pub(super) const G_OBJRM_ALL: u32 = G_OBJRM_NOTXCLAMP
    | G_OBJRM_XLU
    | G_OBJRM_ANTIALIAS
    | G_OBJRM_BILERP
    | G_OBJRM_SHRINKSIZE_1
    | G_OBJRM_SHRINKSIZE_2
    | G_OBJRM_WIDEN;

pub(super) const RDP_SETTIMG: u8 = 0xfd;
pub(super) const RDP_SETTILE: u8 = 0xf5;
pub(super) const RDP_LOADSYNC: u8 = 0xe6;
pub(super) const RDP_LOADBLOCK: u8 = 0xf3;
pub(super) const RDP_LOADTILE: u8 = 0xf4;
pub(super) const RDP_LOADTLUT: u8 = 0xf0;

/// Public `uObjSprite_t` wire fields from `gs2dex.h` / Programming Manual
/// Chapter 25, "S2DEX Microcode", section 4.2.1.

mod object_mode;
mod common;
mod background;
mod object_draw;
mod object_ops;

use object_mode::*;
use common::*;
use background::*;
use object_ops::*;

// Preserve the crate-visible entry point path `s2dex::decode_ops_for_family`.
pub(crate) use object_draw::decode_ops_for_family;
// pub(crate) object types reached from gbi.rs via `crate::s2dex::*`.
pub(crate) use object_mode::{
    ObjectFilterCorrection, ObjectRenderMode, ObjectSprite, ObjectTextureClamp,
};

#[cfg(test)]
pub(super) fn decode_ops(
    rdram: &[u8],
    start: u32,
    rdp: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    decode_ops_for_family(rdram, start, rdp, S2dexWireFamily::S2dex2)
}

pub(super) fn require_image_range(
    rdram: &[u8],
    image: u32,
    texture: ObjectTexture,
    command: &str,
) -> Result<(u32, usize), RenderError> {
    let class = image >> 24;
    if !matches!(class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} texture image {image:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let image = image & 0x00ff_ffff;
    if !image.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} texture image {image:#010x} is not 8-byte aligned"
        )));
    }
    let bytes = match texture {
        ObjectTexture::Block {
            tmem, tsize, tline, ..
        } => {
            let words = u32::from(tsize) + 1;
            if tmem > 511 || u32::from(tmem) + words > 512 || !(1..=0x0fff).contains(&tline) {
                return Err(reject(format!(
                    "{command} uObjTxtrBlock has invalid tmem={tmem} tsize={tsize} tline={tline}"
                )));
            }
            words * 8
        }
        ObjectTexture::Tile {
            tmem,
            twidth,
            theight,
            ..
        } => {
            if tmem > 511
                || twidth & 3 != 3
                || theight & 3 != 3
                || twidth > 0x03ff
                || theight > 0x0fff
            {
                return Err(reject(format!(
                    "{command} uObjTxtrTile has invalid tmem={tmem} twidth={twidth} theight={theight}"
                )));
            }
            let words_per_row = (u32::from(twidth) + 1) / 4;
            let rows = (u32::from(theight) + 1) / 4;
            let words = words_per_row * rows;
            if u32::from(tmem) + words > 512 {
                return Err(reject(format!(
                    "{command} uObjTxtrTile range tmem={tmem} words={words} exceeds TMEM"
                )));
            }
            words * 8
        }
        ObjectTexture::Tlut { phead, pnum, .. } => {
            let entries = u32::from(pnum) + 1;
            if !(256..=511).contains(&phead) || pnum > 255 || u32::from(phead) + entries > 512 {
                return Err(reject(format!(
                    "{command} uObjTxtrTLUT has invalid phead={phead} pnum={pnum}"
                )));
            }
            entries * 2
        }
    };
    let end = image
        .checked_add(bytes)
        .ok_or_else(|| reject(format!("{command} texture image range overflow")))?;
    if end as usize > PHYSICAL_RDRAM_BYTES {
        return Err(reject(format!(
            "{command} texture image range [{image:#010x}, {end:#010x}) exceeds physical 8 MiB RDRAM"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} texture image range [{image:#010x}, {end:#010x}) exceeds RDRAM length {}",
            rdram.len()
        )));
    }
    // RdramView's halfword access uses the generated-C native-word layout:
    // a logical TLUT entry at offset zero occupies storage bytes 2..4.
    // Preserve the complete containing word when the logical image has a
    // two-byte tail; the other admitted load shapes already end on words.
    let storage_end = (end + 3) & !3;
    if storage_end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} texture image native-word storage ends at {storage_end:#010x}, beyond RDRAM length {}",
            rdram.len()
        )));
    }
    Ok((
        image,
        usize::try_from(bytes).expect("physical S2DEX image size fits usize"),
    ))
}

pub(super) fn read_u32(rdram: &[u8], address: usize) -> u32 {
    fn64_runtime::RdramView::from_storage(rdram).read_u32(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(address).expect("S2DEX RDRAM address exceeds u32"),
    ))
}

pub(super) fn reject(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: "reference-s2dex",
        reason: reason.into(),
    }
}

pub(super) fn unsupported(operation: &'static str, reason: impl Into<String>) -> RenderError {
    crate::render_unsupported_error("reference-s2dex", operation, reason)
}

#[cfg(test)]
mod tests;
