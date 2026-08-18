// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use fn64_render::{
    GeometryUcodeProfile, RenderError, TaskAdmissionGeneration,
};
use sha2::Digest;
use std::{collections::BTreeMap, fmt::Write as _};
use super::*;
use super::wire::*;
use super::types::*;
use super::state::*;
use super::stream::*;
use super::geometry::*;

/// The simple ("reference-fixture") F3D-style decoder retained for backward
/// compatibility: `G_VTX`/`G_TRI1`/`G_TRI2`/`G_ENDDL` with raw screen-space
/// `ob` coords, non-segmented addresses in `w1`, and the pre-existing
/// vertex/index packing (`n<<12 | v0`; indices `(v0<<16)|(v1<<8)|v2` as
/// plain cache slots). This is what the original hand-built fixtures and the
/// `fn64-abi` executor-seam test plant, so it MUST stay bit-compatible with
/// them. Real OoT display lists use [`decode_display_list_f3dex2`] instead.
pub fn decode_display_list(rdram: &[u8], dl_addr: u32) -> Result<Vec<Triangle>, RenderError> {
    let mut vtx_cache = [Vertex::default(); 32];
    let mut tris = Vec::new();
    let mut pc = dl_addr as usize;

    loop {
        if pc + 8 > rdram.len() {
            return Err(RenderError::Backend {
                backend: "reference-simple",
                reason: format!(
                    "display list reached truncated command at {pc:#010x} without G_ENDDL"
                ),
            });
        }
        let command_pc = pc;
        let w0 = u32::from_be_bytes(rdram[pc..pc + 4].try_into().unwrap());
        let w1 = u32::from_be_bytes(rdram[pc + 4..pc + 8].try_into().unwrap());
        let opcode = (w0 >> 24) as u8;
        pc += 8;

        match opcode {
            G_VTX => {
                // Original packing: w0 low 20 bits = n<<12 | v0; w1 = vtx
                // array address (non-segmented). Raw ob x/y are screen
                // coords -- no transform.
                let n = ((w0 >> 12) & 0xFF) as usize;
                let v0 = (w0 & 0xFF) as usize;
                let addr = w1 as usize;
                let cache_end = v0.checked_add(n).ok_or_else(|| RenderError::Backend {
                    backend: "reference-simple",
                    reason: format!("G_VTX cache range overflows at {command_pc:#010x}"),
                })?;
                let byte_len = n
                    .checked_mul(VTX_STRIDE)
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!("G_VTX byte count overflows at {command_pc:#010x}"),
                    })?;
                let addr_end = addr
                    .checked_add(byte_len)
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!("G_VTX RDRAM range overflows at {command_pc:#010x}"),
                    })?;
                if n == 0 || cache_end > vtx_cache.len() || addr_end > rdram.len() {
                    return Err(RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!(
                            "G_VTX at {command_pc:#010x} selects n={n}, cache={v0}..{cache_end}, RDRAM={addr:#010x}..{addr_end:#010x}"
                        ),
                    });
                }
                for i in 0..n {
                    let off = addr + i * VTX_STRIDE;
                    let x = i16::from_be_bytes([rdram[off], rdram[off + 1]]) as f32;
                    let y = i16::from_be_bytes([rdram[off + 2], rdram[off + 3]]) as f32;
                    let cn = &rdram[off + 12..off + 16];
                    vtx_cache[v0 + i] = Vertex {
                        x,
                        y,
                        z: 0.0, // simple reference path: coplanar, no depth
                        r: cn[0],
                        g: cn[1],
                        b: cn[2],
                        a: cn[3],
                        s: 0.0, // simple reference path: untextured
                        t: 0.0,
                        w: 1.0, // simple path: everything in front of camera
                        z_screen: 0,
                        clip_code: 0,
                        clip_position: None,
                    };
                }
            }
            G_TRI1 => {
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                let idx_a = [(w0 >> 16) & 0xFF, (w0 >> 8) & 0xFF, w0 & 0xFF];
                let idx_b = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_a,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_b,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_ENDDL => break,
            G_NOOP if w0 == 0 && w1 == 0 => {}
            _ => {
                return Err(crate::render_unsupported_error(
                    "reference-simple",
                    "render.gbi.simple.command",
                    format!(
                        "unsupported opcode {opcode:#04x} at {command_pc:#010x} (w0={w0:#010x}, w1={w1:#010x})"
                    ),
                ));
            }
        }
    }
    Ok(tris)
}

pub(super) fn decode_display_list_f3dex2_state(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<DecodeState, RenderError> {
    let mut state = fresh_decode_state();
    let mut scratch = rdram.to_vec();
    let mut family = GeometryWireFamily::F3dex2;
    #[cfg(not(test))]
    projdump::reset_frame();
    decode_stream(&mut scratch, dl_addr, &mut state, None, None, &mut family);
    #[cfg(not(test))]
    projdump::summary();
    Ok(state)
}

pub(super) fn execute_display_list_f3dex2_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
) -> Result<DecodeState, RenderError> {
    execute_display_list_f3dex2_state_with_catalog(rdram, rsp_memory, dl_addr, None)
}

pub(super) fn execute_display_list_f3dex2_state_with_catalog(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: Option<&F3dex2UcodeCatalog>,
) -> Result<DecodeState, RenderError> {
    execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        catalog,
        None,
        GeometryUcodeProfile::from_public_family(GeometryWireFamily::F3dex2),
        DecodeAdmissionPolicy::default(),
    )
}

pub(super) fn execute_display_list_f3dex2_state_with_catalog_and_rdp(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: Option<&F3dex2UcodeCatalog>,
    rdp_state: Option<&mut RdpDecodeState>,
    initial_profile: GeometryUcodeProfile,
    admission_policy: DecodeAdmissionPolicy,
) -> Result<DecodeState, RenderError> {
    let mut state = rdp_state
        .as_deref()
        .map(RdpDecodeState::begin_task)
        .unwrap_or_else(fresh_decode_state);
    state.admission_raw_window_bytes = admission_policy.raw_window_bytes;
    state.force_branch = admission_policy.force_branch;
    super::census::note_decode_entry();
    #[cfg(not(test))]
    projdump::reset_frame();
    let mut family = initial_profile.wire_family();
    initialize_geometry_profile_state(&mut state, initial_profile);
    decode_stream(
        rdram,
        dl_addr,
        &mut state,
        Some(rsp_memory),
        catalog,
        &mut family,
    );
    #[cfg(not(test))]
    projdump::summary();
    if let Some(digest) = state.unsupported_ucode_reload {
        return Err(RenderError::RequiresLle {
            ucode_sha256: digest.as_bytes(),
        });
    }
    if let Some(reason) = state.admission_raw_window_error {
        return Err(RenderError::Backend {
            backend: "microcode-admission-window",
            reason,
        });
    }
    if let Some(rdp_state) = rdp_state {
        rdp_state.commit_task(&state);
    }
    Ok(state)
}

/// Decode a bounded raw RDP stream. Unlike an RSP display list, opcodes
/// 0x08..0x0f are variable-width edge/coefficient commands.
pub fn decode_raw_rdp_ops(rdram: &[u8], start: u32) -> Result<Vec<RenderOp>, RenderError> {
    let mut state = fresh_decode_state();
    let mut scratch = rdram.to_vec();
    let mut family = GeometryWireFamily::F3dex2;
    decode_stream_impl(
        &mut scratch,
        start,
        &mut state,
        true,
        None,
        None,
        &mut family,
    );
    Ok(state.ops)
}

pub(crate) fn decode_raw_rdp_ops_with_state(
    rdram: &mut [u8],
    start: u32,
    rdp_state: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut state = rdp_state.begin_task();
    super::census::note_decode_entry();
    let mut family = GeometryWireFamily::F3dex2;
    // No `rdram.to_vec()` scratch copy here (unlike the sibling decoders):
    // raw-RDP tasks always pass `rsp_memory: None` below, and the only
    // commands `decode_stream_impl` writes RDRAM through -- G_DMA_IO,
    // G_LOAD_UCODE -- panic without live RSP memory. So `rdram` is read-only
    // in practice on this path, and the caller already owns a genuine
    // `&mut [u8]` (see `prepare_reference_task`'s `DecodeMode::RawRdp` arm),
    // so decoding can borrow it directly instead of cloning all 8 MiB of
    // guest RDRAM on every task. Measured live with `sample` on WM2000
    // (100% raw-RDP/XBUS submission, ~18,838 tasks/route): this clone alone
    // was ~4.5% of a 10s in-process capture during heavy rasterization.
    decode_stream_impl(rdram, start, &mut state, true, None, None, &mut family);
    rdp_state.commit_task(&state);
    Ok(state.ops)
}

/// Decode a real F3DEX2 display-list graph to ordered RSP/RDP operations.
/// State mutations, framebuffer targets, fills, syncs, and triangles retain
/// command order across nested display lists.
pub fn decode_display_list_f3dex2_ops(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(decode_display_list_f3dex2_state(rdram, dl_addr)?.ops)
}

/// Execute an F3DEX2 display-list graph against the console's live RDRAM and
/// persistent RSP memories. Unlike the read-only inspection helper above,
/// this path applies `G_DMA_IO` in command order, so a debug DMA write can
/// change bytes consumed by a later command in the same task.
pub fn execute_display_list_f3dex2_ops(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(execute_display_list_f3dex2_state(rdram, rsp_memory, dl_addr)?.ops)
}

/// Execute with an exact catalog for compatible self-loaded text images.
/// The backend separately verifies the initial live IMEM image against this
/// same catalog before entering this decoder; any changed, unlisted
/// generation stops at `G_LOAD_UCODE` before HLE can interpret its following
/// commands under the wrong microcode family.
pub fn execute_display_list_f3dex2_ops_admitted(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &F3dex2UcodeCatalog,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(
        execute_display_list_f3dex2_state_with_catalog(rdram, rsp_memory, dl_addr, Some(catalog))?
            .ops,
    )
}

#[cfg(test)]
pub(crate) fn execute_display_list_f3dex2_ops_admitted_with_rdp_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &F3dex2UcodeCatalog,
    rdp_state: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        Some(catalog),
        Some(rdp_state),
        GeometryUcodeProfile::from_public_family(GeometryWireFamily::F3dex2),
        DecodeAdmissionPolicy::default(),
    )?
    .ops)
}

pub(crate) fn execute_display_list_geometry_ops_admitted_with_rdp_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &GeometryUcodeCatalog,
    rdp_state: &mut RdpDecodeState,
    family: GeometryWireFamily,
) -> Result<Vec<RenderOp>, RenderError> {
    let inspection = inspect_display_list_geometry_admission_with_rdp_state(
        rdram, rsp_memory, dl_addr, catalog, rdp_state, family,
    )?;
    let _ = inspection.self_loads;
    Ok(inspection.operations)
}

pub(crate) struct GeometryTaskInspection {
    pub(crate) operations: Vec<RenderOp>,
    pub(crate) self_loads: Vec<TaskAdmissionGeneration>,
    #[cfg(test)]
    pub(crate) self_load_raw_windows: Vec<TaskAdmissionRawWindow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskAdmissionRawWindow {
    pub(crate) text: Vec<u8>,
    pub(crate) data: Vec<u8>,
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskAdmissionRawWindowSize {
    pub(crate) text: usize,
    pub(crate) data: usize,
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct GeometryTaskAdmissionOptions {
    pub(crate) raw_window_size: TaskAdmissionRawWindowSize,
    pub(crate) force_branch: bool,
}

pub(crate) fn inspect_display_list_geometry_admission_with_rdp_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &GeometryUcodeCatalog,
    rdp_state: &mut RdpDecodeState,
    family: GeometryWireFamily,
) -> Result<GeometryTaskInspection, RenderError> {
    let state = execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        Some(catalog),
        Some(rdp_state),
        GeometryUcodeProfile::from_public_family(family),
        DecodeAdmissionPolicy::default(),
    )?;
    Ok(GeometryTaskInspection {
        operations: state.ops,
        self_loads: state.admission_generations,
        #[cfg(test)]
        self_load_raw_windows: state.admission_raw_windows,
    })
}

#[cfg(test)]
pub(crate) fn inspect_display_list_geometry_admission_with_raw_windows(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &GeometryUcodeCatalog,
    rdp_state: &mut RdpDecodeState,
    family: GeometryWireFamily,
    options: GeometryTaskAdmissionOptions,
) -> Result<GeometryTaskInspection, RenderError> {
    let raw_window_size = options.raw_window_size;
    assert!(raw_window_size.text > 0 && raw_window_size.data > 0);
    let state = execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        Some(catalog),
        Some(rdp_state),
        GeometryUcodeProfile::from_public_family(family),
        DecodeAdmissionPolicy {
            raw_window_bytes: Some((raw_window_size.text, raw_window_size.data)),
            force_branch: options.force_branch,
        },
    )?;
    assert_eq!(
        state.admission_generations.len(),
        state.admission_raw_windows.len(),
        "each admitted self-load must retain exactly one requested raw recognition window"
    );
    Ok(GeometryTaskInspection {
        operations: state.ops,
        self_loads: state.admission_generations,
        self_load_raw_windows: state.admission_raw_windows,
    })
}

/// Validate a bounded DRAM-backed raw RDP command range before the reference
/// backend executes it. The HLE decoder currently implements the listed
/// rectangle/image/texture/state commands and all eight edge/coefficient
/// triangle layouts. Unmodeled RDP state opcodes remain loud instead of
/// disappearing through `skip_opcode`.
pub fn validate_raw_rdp_command_range(
    rdram: &[u8],
    start: u32,
    end: u32,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) {
        return Err(reject(format!(
            "raw RDP range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "raw RDP range end {end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        )));
    }
    let mut pc = start;
    while pc < end {
        let wire_opcode = (read_u32(rdram, pc as usize) >> 24) as u8;
        let opcode = canonical_raw_rdp_opcode(wire_opcode);
        let supported = matches!(
            opcode,
            // 0x00..=0x07 is the low No Operation block (G_NOOP is its 0x00
            // member); the executor treats 0x01..=0x07 as no-ops on the raw
            // lane only, since the same bytes are G_VTX..G_QUAD on the
            // geometry lane. 0x08..=0x0f are the eight triangle layouts.
            0x00..=0x0f
                    | G_TEXRECT
                    | G_TEXRECTFLIP
                    | G_RDPLOADSYNC
                    | G_RDPPIPESYNC
                    | G_RDPTILESYNC
                    | G_RDPFULLSYNC
                    | G_SETOTHERMODE_L
                    | G_SETOTHERMODE_H
                    | G_RDPSETOTHERMODE
                    | G_SETSCISSOR
                    | G_SETKEYGB
                    | G_SETKEYR
                    | G_SETCONVERT
                    | G_SETPRIMDEPTH
                    | G_LOADTLUT
                    | G_SETTILESIZE
                    | G_LOADBLOCK
                    | G_LOADTILE
                    | G_SETTILE
                    | G_FILLRECT
                    | G_SETFILLCOLOR
                    | G_SETFOGCOLOR
                    | G_SETBLENDCOLOR
                    | G_SETPRIMCOLOR
                    | G_SETENVCOLOR
                    | G_SETCOMBINE
                    | G_SETTIMG
                    | G_SETZIMG
                    | G_SETCIMG
        );
        if !supported {
            return Err(crate::render_unsupported_error(
                "reference",
                "render.gbi.raw-rdp.command",
                format!(
                    "raw RDP opcode {} ({opcode:#04x}, wire byte {wire_opcode:#04x}) \
                     at {pc:#010x} is unsupported",
                    raw_rdp_opcode_name(opcode)
                ),
            ));
        }
        let width = raw_rdp_command_width(opcode).unwrap_or(8);
        pc = pc.checked_add(width).ok_or_else(|| {
            reject(format!(
                "raw RDP command at {pc:#010x} overflows address space"
            ))
        })?;
        if pc > end {
            return Err(reject(format!(
                "raw RDP {} at {:#010x} is truncated by range end {end:#010x}",
                raw_rdp_opcode_name(opcode),
                pc - width
            )));
        }
    }
    Ok(())
}

pub use fn64_render::inspect_raw_rdp_full_sync as raw_rdp_full_sync_status;

pub(super) fn raw_rdp_opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x08 => "RDP_TRI_FILL",
        0x09 => "RDP_TRI_FILL_ZBUFF",
        0x0a => "RDP_TRI_TXTR",
        0x0b => "RDP_TRI_TXTR_ZBUFF",
        0x0c => "RDP_TRI_SHADE",
        0x0d => "RDP_TRI_SHADE_ZBUFF",
        0x0e => "RDP_TRI_SHADE_TXTR",
        0x0f => "RDP_TRI_SHADE_TXTR_ZBUFF",
        _ => opcode_name(opcode),
    }
}

/// Byte width of one raw RDP command. Triangle sizes concatenate the edge,
/// shade, texture, and Z groups from SGI *RDP Command Summary* Table 11.
pub(super) fn raw_rdp_command_width(opcode: u8) -> Option<u32> {
    fn64_render::raw_rdp_command_width(opcode)
}

/// Canonical spelling of a raw-lane RDP wire byte.
///
/// The RDP command field is bits 61:56 -- the low six bits of the top wire
/// byte -- and bits 63:62 are documented don't-care. A command may therefore
/// legitimately arrive under any of four spellings, and WCW/nWo Revenge does
/// emit `Set Color Image` as `0x7f` where the GBI macros emit `0xff`.
///
/// The decoder's match arms are written in the public GBI names, so the two
/// halves of the command space normalize in opposite directions: triangles
/// (`0x08..=0x0f`) are matched bare, while every other command is matched
/// under its `0xc0`-based GBI constant. Mapping to those two canonical forms
/// here keeps a single spelling reaching the match without rewriting every
/// arm, and keeps the geometry lane -- where these same bytes are G_VTX and
/// friends -- entirely untouched.
pub(super) fn canonical_raw_rdp_opcode(wire_opcode: u8) -> u8 {
    let command = wire_opcode & 0x3f;
    // 0x00..=0x07 (the No Operation block, whose 0x00 is G_NOOP) and
    // 0x08..=0x0f (the triangles) are matched bare; everything else is
    // matched under its 0xc0-based GBI constant.
    if matches!(command, 0x00..=0x0f) {
        command
    } else {
        0xc0 | command
    }
}

pub(super) fn decode_rdp_edge_coefficients(rdram: &[u8], pc: usize) -> Option<RdpEdgeCoefficients> {
    if pc.checked_add(32)? > rdram.len() {
        return None;
    }
    let w0 = read_u32(rdram, pc);
    let w1 = read_u32(rdram, pc + 4);
    Some(RdpEdgeCoefficients {
        left_major: w0 & (1 << 23) != 0,
        level: ((w0 >> 19) & 0x07) as u8,
        tile: ((w0 >> 16) & 0x07) as u8,
        yl: sign_extend_u32(w0 & 0x3fff, 14) as i16,
        ym: sign_extend_u32((w1 >> 16) & 0x3fff, 14) as i16,
        yh: sign_extend_u32(w1 & 0x3fff, 14) as i16,
        xl: read_u32(rdram, pc + 8) as i32,
        dxldy: read_u32(rdram, pc + 12) as i32,
        xh: read_u32(rdram, pc + 16) as i32,
        dxhdy: read_u32(rdram, pc + 20) as i32,
        xm: read_u32(rdram, pc + 24) as i32,
        dxmdy: read_u32(rdram, pc + 28) as i32,
    })
}

pub(super) fn decode_rdp_z_coefficients(rdram: &[u8], pc: usize) -> Option<RdpZCoefficients> {
    if pc.checked_add(16)? > rdram.len() {
        return None;
    }
    Some(RdpZCoefficients {
        z: read_u32(rdram, pc) as i32,
        dzdx: read_u32(rdram, pc + 4) as i32,
        dzde: read_u32(rdram, pc + 8) as i32,
        dzdy: read_u32(rdram, pc + 12) as i32,
    })
}

pub(super) fn decode_rdp_shade_coefficients(rdram: &[u8], pc: usize) -> Option<RdpShadeCoefficients> {
    if pc.checked_add(64)? > rdram.len() {
        return None;
    }
    let components = |integer_offset: usize, fraction_offset: usize| {
        let integer_rg = read_u32(rdram, pc + integer_offset);
        let integer_ba = read_u32(rdram, pc + integer_offset + 4);
        let fraction_rg = read_u32(rdram, pc + fraction_offset);
        let fraction_ba = read_u32(rdram, pc + fraction_offset + 4);
        [
            fixed_16_16(integer_rg >> 16, fraction_rg >> 16),
            fixed_16_16(integer_rg, fraction_rg),
            fixed_16_16(integer_ba >> 16, fraction_ba >> 16),
            fixed_16_16(integer_ba, fraction_ba),
        ]
    };
    Some(RdpShadeCoefficients {
        color: components(0, 16),
        dcdx: components(8, 24),
        dcde: components(32, 48),
        dcdy: components(40, 56),
    })
}

pub(super) fn decode_rdp_texture_coefficients(rdram: &[u8], pc: usize) -> Option<RdpTextureCoefficients> {
    if pc.checked_add(64)? > rdram.len() {
        return None;
    }
    let components = |integer_offset: usize, fraction_offset: usize| {
        let integer_st = read_u32(rdram, pc + integer_offset);
        let integer_w = read_u32(rdram, pc + integer_offset + 4);
        let fraction_st = read_u32(rdram, pc + fraction_offset);
        let fraction_w = read_u32(rdram, pc + fraction_offset + 4);
        [
            fixed_16_16(integer_st >> 16, fraction_st >> 16),
            fixed_16_16(integer_st, fraction_st),
            fixed_16_16(integer_w >> 16, fraction_w >> 16),
        ]
    };
    Some(RdpTextureCoefficients {
        stw: components(0, 16),
        dstdx: components(8, 24),
        dstde: components(32, 48),
        dstdy: components(40, 56),
    })
}

pub(super) fn fixed_16_16(integer: u32, fraction: u32) -> i32 {
    (i32::from(integer as u16 as i16) << 16) | i32::from(fraction as u16)
}

pub(super) fn sign_extend_u32(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

/// Compatibility view of F3DEX2 decode for callers that inspect geometry.
/// The reference backend consumes [`decode_display_list_f3dex2_ops`] so it
/// does not discard non-triangle RDP work.
pub fn decode_display_list_f3dex2(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<Vec<Triangle>, RenderError> {
    Ok(decode_display_list_f3dex2_ops(rdram, dl_addr)?
        .into_iter()
        .filter_map(|op| match op {
            RenderOp::Triangle(triangle) => Some(triangle),
            _ => None,
        })
        .collect())
}

/// Produce a lossless command-word walk of an F3DEX2 display-list graph for
/// differential diagnostics. This follows the same public `G_DL` call versus
/// branch rules and `G_MOVEWORD/G_MW_SEGMENT` address updates as the decoder,
/// but does not interpret rendering state. Pointer-bearing commands include a
/// bounded content fingerprint at their resolved RDRAM target, so a trace can
/// distinguish a valid submitted graph from a dangling/empty DMA range without
/// copying game data into this repository. The caller owns where the returned
/// text is written; the RT64 task-dump hook writes only to an explicitly
/// requested untracked diagnostic directory.
pub(crate) fn trace_display_list_f3dex2(rdram: &[u8], dl_addr: u32) -> String {
    struct TraceState {
        segments: [u32; 16],
        commands: u32,
        opcodes: BTreeMap<u8, u32>,
        text: String,
    }

    pub(super) fn fingerprint(rdram: &[u8], start: usize, requested_len: usize) -> String {
        if start >= rdram.len() {
            return format!("target={start:#08x} OUT_OF_BOUNDS");
        }
        let end = start.saturating_add(requested_len).min(rdram.len());
        let bytes = &rdram[start..end];
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let nonzero = bytes.iter().filter(|&&byte| byte != 0).count();
        format!(
            "target={start:#08x} bytes={} nonzero={} fnv1a64={hash:016x}",
            bytes.len(),
            nonzero
        )
    }

    pub(super) fn trace_stream(rdram: &[u8], dl_addr: u32, depth: u32, state: &mut TraceState) {
        let mut pc = resolve_addr(&state.segments, dl_addr);
        writeln!(
            state.text,
            "ENTER depth={depth} segmented={dl_addr:#010x} resolved={pc:#08x}"
        )
        .expect("writing a display-list trace to String cannot fail");

        loop {
            if pc + 8 > rdram.len() {
                writeln!(state.text, "STOP depth={depth} pc={pc:#08x} OUT_OF_BOUNDS")
                    .expect("writing a display-list trace to String cannot fail");
                break;
            }
            state.commands += 1;
            if state.commands > MAX_DL_COMMANDS {
                writeln!(
                    state.text,
                    "STOP depth={depth} command_budget={MAX_DL_COMMANDS} exceeded"
                )
                .expect("writing a display-list trace to String cannot fail");
                break;
            }

            let command_pc = pc;
            let w0 = read_u32(rdram, pc);
            let w1 = read_u32(rdram, pc + 4);
            let opcode = (w0 >> 24) as u8;
            pc += 8;
            *state.opcodes.entry(opcode).or_default() += 1;

            let reference = match opcode {
                G_VTX => {
                    let n = ((w0 >> 12) & 0xFF) as usize;
                    Some(fingerprint(
                        rdram,
                        resolve_addr(&state.segments, w1),
                        n.saturating_mul(16),
                    ))
                }
                G_MTX => Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64)),
                G_MOVEMEM | G_SETTIMG | G_DL => {
                    Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64))
                }
                _ => None,
            };
            writeln!(
                state.text,
                "CMD depth={depth} pc={command_pc:#08x} op={opcode:#04x} w0={w0:#010x} \
                 w1={w1:#010x}{}",
                reference
                    .as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default(),
            )
            .expect("writing a display-list trace to String cannot fail");

            match opcode {
                G_MOVEWORD => {
                    let index = ((w0 >> 16) & 0xFF) as u16;
                    let offset = (w0 & 0xFFFF) as u16;
                    if index == G_MW_SEGMENT {
                        let segment = (offset / 4) as usize;
                        if segment < state.segments.len() {
                            state.segments[segment] = w1 & 0x00FF_FFFF;
                            writeln!(
                                state.text,
                                "SEG depth={depth} segment={segment} base={:#08x}",
                                state.segments[segment]
                            )
                            .expect("writing a display-list trace to String cannot fail");
                        }
                    }
                }
                G_DL => {
                    let is_branch = ((w0 >> 16) & 1) != 0;
                    if is_branch {
                        pc = resolve_addr(&state.segments, w1);
                        writeln!(state.text, "BRANCH depth={depth} target={pc:#08x}")
                            .expect("writing a display-list trace to String cannot fail");
                    } else if depth < MAX_DL_DEPTH {
                        trace_stream(rdram, w1, depth + 1, state);
                    } else {
                        writeln!(
                            state.text,
                            "STOP depth={depth} call_depth={MAX_DL_DEPTH} exceeded"
                        )
                        .expect("writing a display-list trace to String cannot fail");
                    }
                }
                G_ENDDL => {
                    writeln!(state.text, "RETURN depth={depth}")
                        .expect("writing a display-list trace to String cannot fail");
                    break;
                }
                _ => {}
            }
        }
    }

    let mut state = TraceState {
        segments: [0; 16],
        commands: 0,
        opcodes: BTreeMap::new(),
        text: String::new(),
    };
    trace_stream(rdram, dl_addr, 0, &mut state);
    writeln!(state.text, "SUMMARY commands={}", state.commands)
        .expect("writing a display-list trace to String cannot fail");
    for (opcode, count) in state.opcodes {
        writeln!(state.text, "OPCODE op={opcode:#04x} count={count}")
            .expect("writing a display-list trace to String cannot fail");
    }
    state.text
}
