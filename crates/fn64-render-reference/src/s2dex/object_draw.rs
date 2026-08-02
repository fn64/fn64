use crate::gbi::{CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, Vertex};
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

use super::*;
use super::object_mode::*;
use super::common::*;
use super::background::*;
use super::object_ops::*;

pub(crate) fn decode_ops_for_family(
    rdram: &[u8],
    start: u32,
    rdp: &mut RdpDecodeState,
    family: S2dexWireFamily,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut pc = (start & 0x00ff_ffff) as usize;
    let mut operations = Vec::new();
    let mut speculative_rdp = rdp.clone();
    let mut object_status = [0u32; 4];
    let mut object_texture_scratch = ObjectTextureScratch::new();
    let mut object_matrix = None;
    let mut rotation_matrix_loaded = false;
    let mut object_render_mode = ObjectRenderMode::default();
    let mut pending_select = None;
    let mut return_stack = Vec::new();
    let mut segments = [0u32; 16];
    for command_index in 0..MAX_COMMANDS {
        let end = pc
            .checked_add(8)
            .ok_or_else(|| reject("display-list PC overflow"))?;
        if end > rdram.len() {
            return Err(reject(format!(
                "display list is truncated at RDRAM {pc:#010x}: need 8 command bytes, rdram_bytes={}",
                rdram.len()
            )));
        }
        let w0 = read_u32(rdram, pc);
        let w1 = read_u32(rdram, pc + 4);
        let opcode = (w0 >> 24) as u8;
        let decoded = decode_command(family, opcode);
        let command_pc = pc;
        pc = end;

        if pending_select.is_some() && decoded != Some(S2dexCommand::SelectDl) {
            return Err(reject(format!(
                "G_RDPHALF_0 at the preceding command must be followed immediately by G_SELECT_DL, got {} at RDRAM {command_pc:#010x}",
                decoded.map_or("UNKNOWN", S2dexCommand::name)
            )));
        }

        match decoded {
            Some(S2dexCommand::ObjRectangle) => {
                if w0 & 0x00ff_ffff != 0 {
                    return Err(reject(format!(
                        "G_OBJ_RECTANGLE at {command_pc:#010x} has nonzero reserved/length payload {:#08x}; public gsSPObjRectangle uses gDma0p length zero",
                        w0 & 0x00ff_ffff
                    )));
                }
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_RECTANGLE", "uObjSprite")?,
                    "G_OBJ_RECTANGLE",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_RECTANGLE",
                )?);
            }
            Some(S2dexCommand::ObjSprite) => {
                require_dma_length(w0, 0, "G_OBJ_SPRITE", command_pc)?;
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_SPRITE", "uObjSprite")?,
                    "G_OBJ_SPRITE",
                )?;
                let matrix = require_rotation_matrix(
                    object_matrix,
                    rotation_matrix_loaded,
                    "G_OBJ_SPRITE",
                    command_pc,
                    false,
                )?;
                operations.extend(object_sprite_ops(
                    &mut speculative_rdp,
                    sprite,
                    matrix,
                    object_render_mode,
                    "G_OBJ_SPRITE",
                )?);
            }
            Some(S2dexCommand::ObjRectangleR) => {
                require_dma_length(w0, 0, "G_OBJ_RECTANGLE_R", command_pc)?;
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_RECTANGLE_R", "uObjSprite")?,
                    "G_OBJ_RECTANGLE_R",
                )?;
                let matrix = object_matrix.ok_or_else(|| {
                    reject(format!(
                        "G_OBJ_RECTANGLE_R at RDRAM {command_pc:#010x} requires a preceding G_OBJ_MOVEMEM matrix command"
                    ))
                })?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    matrix_relative_sprite(sprite, matrix)?,
                    object_render_mode,
                    "G_OBJ_RECTANGLE_R",
                )?);
            }
            Some(S2dexCommand::ObjMoveMem) => {
                let matrix_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_MOVEMEM", "matrix")?;
                let (matrix, loads_rotation) = read_object_matrix_command(
                    rdram,
                    w0,
                    matrix_address,
                    object_matrix,
                    command_pc,
                )?;
                object_matrix = Some(matrix);
                rotation_matrix_loaded |= loads_rotation;
            }
            Some(S2dexCommand::ObjLoadTxtr) => {
                require_dma_length(w0, 23, "G_OBJ_LOADTXTR", command_pc)?;
                let texture_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LOADTXTR", "uObjTxtr")?;
                let texture =
                    read_object_texture(rdram, texture_address, &segments, "G_OBJ_LOADTXTR")?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LOADTXTR",
                )?;
            }
            Some(S2dexCommand::ObjLdTxRect) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_RECT", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_RECT", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_RECT",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_RECT")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_RECT uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_RECT")?;

                // Section 4.6.2 defines this command as LoadTxtr then
                // Rectangle. Both changes remain task-local until G_ENDDL,
                // so a rejected draw cannot commit its preceding load.
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_RECT",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_LDTX_RECT",
                )?);
            }
            Some(S2dexCommand::ObjLdTxRectR) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_RECT_R", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_RECT_R", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_RECT_R",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_RECT_R")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_RECT_R uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_RECT_R")?;
                let matrix = object_matrix.ok_or_else(|| {
                    reject(format!(
                        "G_OBJ_LDTX_RECT_R at RDRAM {command_pc:#010x} requires a preceding G_OBJ_MOVEMEM matrix command; texture load was not applied"
                    ))
                })?;
                let sprite = matrix_relative_sprite(sprite, matrix)?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_RECT_R",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_LDTX_RECT_R",
                )?);
            }
            Some(S2dexCommand::ObjLdTxSprite) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_SPRITE", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_SPRITE", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_SPRITE",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_SPRITE")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_SPRITE uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_SPRITE")?;
                let matrix = require_rotation_matrix(
                    object_matrix,
                    rotation_matrix_loaded,
                    "G_OBJ_LDTX_SPRITE",
                    command_pc,
                    true,
                )?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_SPRITE",
                )?;
                operations.extend(object_sprite_ops(
                    &mut speculative_rdp,
                    sprite,
                    matrix,
                    object_render_mode,
                    "G_OBJ_LDTX_SPRITE",
                )?);
            }
            Some(command @ (S2dexCommand::BgCopy | S2dexCommand::Bg1Cyc)) => {
                let name = command.name();
                require_dma_length(w0, 0, name, command_pc)?;
                let background_address = resolve_s2dex_pointer(&segments, w1, name, "uObjBg")?;
                let background = read_background(rdram, background_address, &segments, command)?;
                operations.extend(background_ops(
                    rdram,
                    background,
                    &mut speculative_rdp,
                    name,
                )?);
            }
            Some(S2dexCommand::ObjRenderMode) => {
                if w0 & 0x00ff_ffff != 0 {
                    return Err(reject(format!(
                        "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} has nonzero reserved payload {:#08x}",
                        w0 & 0x00ff_ffff
                    )));
                }
                object_render_mode = read_object_render_mode(w1, command_pc)?;
            }
            Some(S2dexCommand::MoveWord) => {
                let (index, offset) = move_word_fields(family, w0);
                match index {
                    G_MW_SEGMENT => {
                        if !offset.is_multiple_of(4) || offset / 4 >= 16 {
                            return Err(reject(format!(
                                "G_MOVEWORD G_MW_SEGMENT at RDRAM {command_pc:#010x} has offset {offset:#06x}; public segment offsets are aligned 0..=60"
                            )));
                        }
                        segments[usize::from(offset / 4)] = w1 & 0x00ff_ffff;
                    }
                    G_MW_GENSTAT => {
                        if !matches!(offset, 0 | 4 | 8 | 12) {
                            return Err(reject(format!(
                                "G_MOVEWORD G_MW_GENSTAT at RDRAM {command_pc:#010x} has status ID {offset}, outside 0,4,8,12"
                            )));
                        }
                        object_status[usize::from(offset / 4)] = w1;
                    }
                    _ => {
                        return Err(unsupported(
                            "render.s2dex.moveword-index",
                            format!(
                                "unsupported S2DEX G_MOVEWORD index {index:#04x} at RDRAM {command_pc:#010x}: offset={offset:#06x} data={w1:#010x}"
                            ),
                        ));
                    }
                }
            }
            Some(S2dexCommand::RdpHalf0) => {
                let sid = ((w0 >> 16) & 0xff) as u8;
                if !matches!(sid, 0 | 4 | 8 | 12) {
                    return Err(reject(format!(
                        "G_RDPHALF_0 at RDRAM {command_pc:#010x} stages G_SELECT_DL status ID {sid}, outside 0,4,8,12"
                    )));
                }
                pending_select = Some(PendingSelectDl {
                    sid,
                    flag: w1,
                    target_low: w0 as u16,
                });
            }
            Some(S2dexCommand::SelectDl) => {
                let staged = pending_select.take().ok_or_else(|| {
                    reject(format!(
                        "G_SELECT_DL at RDRAM {command_pc:#010x} is missing its preceding G_RDPHALF_0"
                    ))
                })?;
                let push = ((w0 >> 16) & 0xff) as u8;
                if !matches!(push, 0 | 1) {
                    return Err(reject(format!(
                        "G_SELECT_DL at RDRAM {command_pc:#010x} has push selector {push}, expected G_DL_PUSH=0 or G_DL_NOPUSH=1"
                    )));
                }
                let slot = usize::from(staged.sid / 4);
                if object_status[slot] & w1 != staged.flag {
                    object_status[slot] = (object_status[slot] & !w1) | (staged.flag & w1);
                    let target = (u32::from(w0 as u16) << 16) | u32::from(staged.target_low);
                    let target = resolve_s2dex_pointer(&segments, target, "G_SELECT_DL", "target")?;
                    if !target.is_multiple_of(8) {
                        return Err(reject(format!(
                            "G_SELECT_DL target {target:#010x} is not 8-byte aligned"
                        )));
                    }
                    let target = target as usize;
                    if target >= PHYSICAL_RDRAM_BYTES || target + 8 > rdram.len() {
                        return Err(reject(format!(
                            "G_SELECT_DL target {target:#010x} lies outside physical/backed RDRAM"
                        )));
                    }
                    if push == 0 {
                        if return_stack.len() == MAX_DL_DEPTH {
                            return Err(reject(format!(
                                "G_SELECT_DL call depth exceeds the public {MAX_DL_DEPTH}-entry F3DEX_GBI_2 stack"
                            )));
                        }
                        return_stack.push(pc);
                    }
                    pc = target;
                }
            }
            Some(S2dexCommand::EndDl) => {
                if w0 & 0x00ff_ffff != 0 || w1 != 0 {
                    return Err(reject(format!(
                        "G_ENDDL at {command_pc:#010x} has nonzero reserved payload: w0={w0:#010x} w1={w1:#010x}"
                    )));
                }
                if let Some(return_pc) = return_stack.pop() {
                    pc = return_pc;
                } else {
                    *rdp = speculative_rdp;
                    return Ok(operations);
                }
            }
            None => {
                return Err(unsupported(
                    "render.s2dex.command",
                    format!(
                        "unsupported {family:?} command byte {opcode:#04x} at RDRAM {command_pc:#010x}: w0={w0:#010x} w1={w1:#010x}"
                    ),
                ));
            }
        }

        if command_index + 1 == MAX_COMMANDS {
            return Err(reject(format!(
                "display list exceeded the {MAX_COMMANDS}-command budget; missing G_ENDDL or cyclic graph"
            )));
        }
    }
    unreachable!("bounded S2DEX command loop exits through a result")
}
