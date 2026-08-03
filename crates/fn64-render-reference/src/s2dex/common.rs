// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

#[cfg(test)]
use fn64_render::UcodeId;

use super::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingSelectDl {
    pub(super) sid: u8,
    pub(super) flag: u32,
    pub(super) target_low: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectTextureCommon {
    pub(super) image: u32,
    pub(super) sid: u16,
    pub(super) flag: u32,
    pub(super) mask: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ObjectMatrix {
    pub(super) a: i32,
    pub(super) b: i32,
    pub(super) c: i32,
    pub(super) d: i32,
    pub(super) x: i16,
    pub(super) y: i16,
    pub(super) base_scale_x: u16,
    pub(super) base_scale_y: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BackgroundCommon {
    pub(super) image_x: u16,
    pub(super) image_w: u16,
    pub(super) frame_x: i16,
    pub(super) frame_w: u16,
    pub(super) image_y: u16,
    pub(super) image_h: u16,
    pub(super) frame_y: i16,
    pub(super) frame_h: u16,
    pub(super) image: u32,
    pub(super) image_load: u16,
    pub(super) image_format: u8,
    pub(super) image_size: u8,
    pub(super) image_palette: u16,
    pub(super) image_flip: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Background {
    Copy {
        common: BackgroundCommon,
        tmem_w: u16,
        tmem_h: u16,
        tmem_load_sh: u16,
        tmem_load_th: u16,
        tmem_size_w: u16,
        tmem_size: u16,
    },
    Scale {
        common: BackgroundCommon,
        scale_w: u16,
        scale_h: u16,
        image_y_origin: i32,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum S2dexCommand {
    ObjRectangle,
    ObjSprite,
    SelectDl,
    ObjLoadTxtr,
    ObjLdTxSprite,
    ObjLdTxRect,
    ObjLdTxRectR,
    Bg1Cyc,
    BgCopy,
    ObjRenderMode,
    ObjRectangleR,
    MoveWord,
    ObjMoveMem,
    RdpHalf0,
    EndDl,
}

impl S2dexCommand {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::ObjRectangle => "G_OBJ_RECTANGLE",
            Self::ObjSprite => "G_OBJ_SPRITE",
            Self::SelectDl => "G_SELECT_DL",
            Self::ObjLoadTxtr => "G_OBJ_LOADTXTR",
            Self::ObjLdTxSprite => "G_OBJ_LDTX_SPRITE",
            Self::ObjLdTxRect => "G_OBJ_LDTX_RECT",
            Self::ObjLdTxRectR => "G_OBJ_LDTX_RECT_R",
            Self::Bg1Cyc => "G_BG_1CYC",
            Self::BgCopy => "G_BG_COPY",
            Self::ObjRenderMode => "G_OBJ_RENDERMODE",
            Self::ObjRectangleR => "G_OBJ_RECTANGLE_R",
            Self::MoveWord => "G_MOVEWORD",
            Self::ObjMoveMem => "G_OBJ_MOVEMEM",
            Self::RdpHalf0 => "G_RDPHALF_0",
            Self::EndDl => "G_ENDDL",
        }
    }
}

pub(super) fn decode_command(family: S2dexWireFamily, opcode: u8) -> Option<S2dexCommand> {
    use S2dexCommand as Command;
    match family {
        S2dexWireFamily::S2dex2 => match opcode {
            G_OBJ_RECTANGLE => Some(Command::ObjRectangle),
            G_OBJ_SPRITE => Some(Command::ObjSprite),
            G_SELECT_DL => Some(Command::SelectDl),
            G_OBJ_LOADTXTR => Some(Command::ObjLoadTxtr),
            G_OBJ_LDTX_SPRITE => Some(Command::ObjLdTxSprite),
            G_OBJ_LDTX_RECT => Some(Command::ObjLdTxRect),
            G_OBJ_LDTX_RECT_R => Some(Command::ObjLdTxRectR),
            G_BG_1CYC => Some(Command::Bg1Cyc),
            G_BG_COPY => Some(Command::BgCopy),
            G_OBJ_RENDERMODE => Some(Command::ObjRenderMode),
            G_OBJ_RECTANGLE_R => Some(Command::ObjRectangleR),
            G_MOVEWORD => Some(Command::MoveWord),
            G_OBJ_MOVEMEM => Some(Command::ObjMoveMem),
            G_RDPHALF_0 => Some(Command::RdpHalf0),
            G_ENDDL => Some(Command::EndDl),
            _ => None,
        },
        S2dexWireFamily::S2dex => match opcode {
            S2DEX_G_BG_1CYC => Some(Command::Bg1Cyc),
            S2DEX_G_BG_COPY => Some(Command::BgCopy),
            S2DEX_G_OBJ_RECTANGLE => Some(Command::ObjRectangle),
            S2DEX_G_OBJ_SPRITE => Some(Command::ObjSprite),
            S2DEX_G_OBJ_MOVEMEM => Some(Command::ObjMoveMem),
            S2DEX_G_SELECT_DL => Some(Command::SelectDl),
            S2DEX_G_OBJ_RENDERMODE => Some(Command::ObjRenderMode),
            S2DEX_G_OBJ_RECTANGLE_R => Some(Command::ObjRectangleR),
            S2DEX_G_ENDDL => Some(Command::EndDl),
            S2DEX_G_MOVEWORD => Some(Command::MoveWord),
            S2DEX_G_OBJ_LOADTXTR => Some(Command::ObjLoadTxtr),
            S2DEX_G_OBJ_LDTX_SPRITE => Some(Command::ObjLdTxSprite),
            S2DEX_G_OBJ_LDTX_RECT => Some(Command::ObjLdTxRect),
            S2DEX_G_OBJ_LDTX_RECT_R => Some(Command::ObjLdTxRectR),
            G_RDPHALF_0 => Some(Command::RdpHalf0),
            _ => None,
        },
    }
}

pub(super) fn move_word_fields(family: S2dexWireFamily, word: u32) -> (u8, u16) {
    match family {
        S2dexWireFamily::S2dex => ((word & 0xff) as u8, ((word >> 8) & 0xffff) as u16),
        S2dexWireFamily::S2dex2 => (((word >> 16) & 0xff) as u8, (word & 0xffff) as u16),
    }
}
