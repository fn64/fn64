use fn64_render::{
    MicrocodeDataImageIdentity, TaskAdmissionGeneration,
    TaskAdmissionSource,
};
use sha2::{Digest, Sha256};
use super::*;
use super::wire::*;
use super::types::*;
use super::matrix::*;
use super::tmem::*;
use super::state::*;
use super::entries::*;
use super::geometry::*;

pub(super) fn decode_stream(
    rdram: &mut [u8],
    dl_addr: u32,
    state: &mut DecodeState,
    rsp_memory: Option<&mut fn64_runtime::RspMemory>,
    ucode_catalog: Option<&F3dex2UcodeCatalog>,
    family: &mut GeometryWireFamily,
) {
    decode_stream_impl(
        rdram,
        dl_addr,
        state,
        false,
        rsp_memory,
        ucode_catalog,
        family,
    );
}

/// Apply public F3DEX2 `gSPDmaRead`/`gSPDmaWrite` wire semantics. The header
/// names these as debug transfers; the SGI RSP Programmer's Guide, chapter 4
/// tables 4-1/4-6, defines READ as DRAM -> I/DMEM and WRITE as
/// I/DMEM -> DRAM. Its DMA section requires 64-bit-aligned addresses and a
/// 64-bit-multiple length, with a maximum 4 KiB transfer. Those malformed
/// cases trap here instead of being rounded into a different transfer.
pub(super) fn execute_dma_io(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    segments: &[u32; 16],
    w0: u32,
    w1: u32,
) {
    assert_eq!(
        w0 & 0x0000_1000,
        0,
        "G_DMA_IO reserved wire bit 12 must be zero"
    );
    let write_to_dram = w0 & 0x0080_0000 != 0;
    let rsp_address = ((w0 >> 13) & 0x03ff) * 8;
    let size = usize::try_from((w0 & 0x0fff) + 1).expect("G_DMA_IO size fits usize");
    assert!(
        size.is_multiple_of(8),
        "G_DMA_IO transfer size {size} is not a 64-bit multiple"
    );
    let dram_address = resolve_addr(segments, w1);
    assert!(
        dram_address.is_multiple_of(8),
        "G_DMA_IO DRAM address {dram_address:#010x} is not 64-bit aligned"
    );
    let dram_end = dram_address
        .checked_add(size)
        .expect("G_DMA_IO DRAM range overflow");
    assert!(
        dram_end <= rdram.len(),
        "G_DMA_IO DRAM range {dram_address:#010x}..{dram_end:#010x} exceeds RDRAM length {:#x}",
        rdram.len()
    );

    let rsp_address = fn64_runtime::RspMemAddr::from_register(rsp_address);
    if write_to_dram {
        let bytes = rsp_memory
            .read_bytes(rsp_address, size)
            .unwrap_or_else(|error| panic!("G_DMA_IO gSPDmaWrite cannot read RSP memory: {error}"));
        fn64_runtime::RdramViewMut::from_storage(rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(dram_address as u32),
            &bytes,
        );
    } else {
        let mut bytes = vec![0; size];
        fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(dram_address as u32),
            &mut bytes,
        );
        rsp_memory
            .write_bytes(rsp_address, &bytes)
            .unwrap_or_else(|error| panic!("G_DMA_IO gSPDmaRead cannot write RSP memory: {error}"));
    }
}

/// Apply the public F3DEX2 `gSPLoadUcodeEx` compound command to the live RSP
/// memories. `G_RDPHALF_1` supplies the physical data-section address;
/// `G_LOAD_UCODE` supplies `(data_size - 1)` and the physical text address.
/// Public `OSTask` guidance fixes the text section at `SP_UCODE_SIZE` (4 KiB),
/// while the RSP Programmer's Guide states that a microcode `.dat` section is
/// loaded at the beginning of DMEM. Both sources are physical, not segmented,
/// addresses, and all SP DMA operands obey the hardware's 64-bit granularity.
pub(super) struct LoadedUcodeIdentity {
    pub(super) text_address: u32,
    pub(super) data_address: u32,
    pub(super) text_sha256: UcodeDigest,
    pub(super) data: MicrocodeDataImageIdentity,
}

pub(super) fn capture_raw_recognition_window(
    rdram: &[u8],
    address: u32,
    bytes: usize,
    section: &str,
) -> Result<Vec<u8>, String> {
    let start = usize::try_from(address).expect("24-bit microcode address fits usize");
    let end = start.checked_add(bytes).ok_or_else(|| {
        format!("microcode {section} recognition window overflows at {address:#010x} + {bytes:#x}")
    })?;
    let window = rdram.get(start..end).ok_or_else(|| {
        format!(
            "microcode {section} recognition window {start:#010x}..{end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        )
    })?;
    Ok(window.to_vec())
}

pub(super) fn execute_load_ucode(
    rdram: &[u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    w0: u32,
    text_address: u32,
    data_address: u32,
) -> LoadedUcodeIdentity {
    assert_eq!(
        w0 & 0x00ff_0000,
        0,
        "G_LOAD_UCODE reserved wire bits 16..23 must be zero"
    );
    let data_size =
        usize::try_from((w0 & 0xffff) + 1).expect("G_LOAD_UCODE data-section size fits usize");
    assert!(
        data_size <= fn64_runtime::RSP_MEMORY_BANK_SIZE,
        "G_LOAD_UCODE data-section size {data_size} exceeds the 4 KiB DMEM bank"
    );
    assert!(
        data_size.is_multiple_of(8),
        "G_LOAD_UCODE data-section size {data_size} is not a 64-bit multiple"
    );
    assert!(
        text_address.is_multiple_of(8),
        "G_LOAD_UCODE text address {text_address:#010x} is not 64-bit aligned"
    );
    assert!(
        data_address.is_multiple_of(8),
        "G_LOAD_UCODE data address {data_address:#010x} is not 64-bit aligned"
    );

    let checked_source = |name: &str, address: u32, size: usize| {
        let start = usize::try_from(address).expect("physical RDRAM address fits usize");
        let end = start
            .checked_add(size)
            .unwrap_or_else(|| panic!("G_LOAD_UCODE {name} RDRAM range overflow"));
        assert!(
            end <= rdram.len(),
            "G_LOAD_UCODE {name} RDRAM range {start:#010x}..{end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        );
        start
    };
    let data_start = checked_source("data", data_address, data_size);
    let text_start = checked_source("text", text_address, SP_UCODE_SIZE);

    let mut data = vec![0; data_size];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(data_start as u32),
        &mut data,
    );
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
            &data,
        )
        .unwrap_or_else(|error| panic!("G_LOAD_UCODE cannot load DMEM data section: {error}"));

    let mut text = vec![0; SP_UCODE_SIZE];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(text_start as u32),
        &mut text,
    );
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &text,
        )
        .unwrap_or_else(|error| panic!("G_LOAD_UCODE cannot load IMEM text section: {error}"));
    LoadedUcodeIdentity {
        text_address,
        data_address,
        text_sha256: UcodeDigest::from_text(&text),
        data: MicrocodeDataImageIdentity {
            bytes: u32::try_from(data_size).expect("G_LOAD_UCODE data size fits u32"),
            sha256: Sha256::digest(data).into(),
        },
    }
}

pub(super) fn decode_stream_impl(
    rdram: &mut [u8],
    dl_addr: u32,
    state: &mut DecodeState,
    raw_rdp: bool,
    mut rsp_memory: Option<&mut fn64_runtime::RspMemory>,
    ucode_catalog: Option<&F3dex2UcodeCatalog>,
    family: &mut GeometryWireFamily,
) {
    assert_eq!(
        state.profile.wire_family(),
        *family,
        "active geometry profile and command wire family diverged"
    );
    let mut pc = resolve_addr(&state.segments, dl_addr);

    loop {
        let command_end = pc.checked_add(8).unwrap_or_else(|| {
            panic!(
                "{} display-list PC {pc:#010x} overflows the host address space",
                family.name()
            )
        });
        assert!(
            command_end <= rdram.len(),
            "{} display list is truncated at RDRAM {pc:#010x}: need 8 command bytes, rdram_bytes={}",
            family.name(),
            rdram.len()
        );
        state.cmds_decoded += 1;
        assert!(
            state.cmds_decoded <= MAX_DL_COMMANDS,
            "{} display list exceeded the {MAX_DL_COMMANDS}-command budget at RDRAM {pc:#010x}; cyclic or corrupt command graph",
            family.name()
        );
        // Recomp rdram is word-native (see read_u32): each command word is a
        // logical big-endian u32 stored host-native, NOT a flat big-endian
        // byte run.
        let command_pc = pc;
        let wire_w0 = read_u32(rdram, pc);
        let wire_w1 = read_u32(rdram, pc + 4);
        let (w0, w1) = if raw_rdp {
            (wire_w0, wire_w1)
        } else {
            if consume_line_triangle_noop(*family, wire_w0, wire_w1) {
                pc += 8;
                continue;
            }
            normalize_geometry_command(*family, wire_w0, wire_w1, command_pc)
        };
        let wire_opcode = (w0 >> 24) as u8;
        let opcode = if raw_rdp {
            canonical_raw_rdp_opcode(wire_opcode)
        } else {
            wire_opcode
        };
        pc += 8;

        if !raw_rdp && family.is_line() && matches!(opcode, G_TRI2 | G_QUAD) {
            crate::render_unsupported_panic(
                "render.gbi.geometry.command",
                format!(
                    "unsupported {family:?} polygon command byte {:#04x} at RDRAM {command_pc:#010x}: line microcode admits only public G_LINE3D; w0={wire_w0:#010x} w1={wire_w1:#010x}",
                    wire_w0 >> 24
                ),
            );
        }

        match opcode {
            G_NOOP => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_NOOP reserved first-word payload must be zero at RDRAM {:#010x}",
                    pc - 8
                );
                // Public gDPNoOpTag deliberately carries an arbitrary tag in
                // w1. The untagged macro is the same command with tag zero.
            }
            // RDP command ids 0x01..=0x07 are the rest of the low No
            // Operation block that 0x00 opens: one command word, no state
            // change, one stalled pipeline cycle. The guard is load-bearing --
            // in the NON-raw GBI lane these same bytes are G_VTX (0x01),
            // G_MODIFYVTX, G_CULLDL, G_BRANCH_Z, G_TRI1, G_TRI2 and G_QUAD
            // (0x07), so an unguarded arm would silently swallow geometry.
            // Unlike G_NOOP no public macro fixes the payload, so nothing is
            // asserted about the low 24 bits or w1; the RDP ignores them.
            0x01..=0x07 if raw_rdp => {}
            G_SPNOOP => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_SPNOOP reserved first-word payload must be zero"
                );
                assert_eq!(w1, 0, "G_SPNOOP reserved second word must be zero");
            }
            // The raw-lane wire spelling was normalized above. The low three
            // bits select shade (4), texture (2), and Z (1).
            opcode @ 0x08..=0x0f if raw_rdp => {
                let command_pc = pc - 8;
                let coefficients = decode_rdp_edge_coefficients(rdram, command_pc)
                    .expect("validated raw RDP edge triangle became truncated during decode");
                let shade_coefficients = (opcode & 4 != 0).then(|| {
                    decode_rdp_shade_coefficients(rdram, command_pc + 32)
                        .expect("validated raw RDP shade triangle became truncated during decode")
                });
                let shade_bytes = if opcode & 4 != 0 { 64 } else { 0 };
                let texture_coefficients = (opcode & 2 != 0).then(|| {
                    decode_rdp_texture_coefficients(rdram, command_pc + 32 + shade_bytes)
                        .expect("validated raw RDP texture triangle became truncated during decode")
                });
                let z_coefficients = (opcode & 1 != 0).then(|| {
                    let texture_bytes = if opcode & 2 != 0 { 64 } else { 0 };
                    decode_rdp_z_coefficients(rdram, command_pc + 32 + shade_bytes + texture_bytes)
                        .expect("validated raw RDP Z triangle became truncated during decode")
                });
                let texture = (opcode & 2 != 0)
                    .then(|| {
                        bind_texture_set(
                            &state.tex,
                            coefficients.tile,
                            coefficients.level,
                            state.other_mode.texture_lut(),
                        )
                    })
                    .flatten();
                assert!(
                    opcode & 2 == 0 || texture.is_some(),
                    "raw RDP textured triangle references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    coefficients.tile
                );
                state.ops.push(RenderOp::RawTriangle(RawRdpTriangle {
                    edge: coefficients,
                    shade: shade_coefficients,
                    texture_coefficients,
                    z: z_coefficients,
                    texture,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                    scissor: state.scissor,
                }));
                pc += raw_rdp_command_width(opcode).expect("raw triangle width") as usize - 8;
            }
            G_RDPHALF_1 => {
                // Public gbi.h composes BranchLessZ as G_RDPHALF_1(target)
                // followed by G_BRANCH_Z(vertex offsets, depth threshold).
                state.rdp_half_1 = Some(w1);
            }
            G_CULLDL => {
                // F3DEX2 gSPCullDisplayList packs inclusive cache indices as
                // v*2 in the low 16 bits of each word. The microcode ANDs
                // their retained clipping codes; any common side terminates
                // this display list exactly like G_ENDDL.
                let encoded_start = (w0 & 0xffff) as usize;
                let encoded_end = (w1 & 0xffff) as usize;
                assert!(
                    encoded_start.is_multiple_of(2) && encoded_end.is_multiple_of(2),
                    "G_CULLDL cache indices must use F3DEX2 v*2 encoding: {encoded_start:#06x}..={encoded_end:#06x}"
                );
                let start = encoded_start / 2;
                let end = encoded_end / 2;
                let cache_capacity = family.cache_capacity();
                assert!(
                    start < end && end < cache_capacity,
                    "{} G_CULLDL inclusive cache range {start}..={end} must satisfy 0 <= start < end < {cache_capacity}",
                    family.name()
                );
                let common_code = state.vtx_cache[start..=end]
                    .iter()
                    .fold(u8::MAX, |common, vertex| common & vertex.clip_code);
                if common_code != 0 {
                    break;
                }
            }
            G_BRANCH_Z => {
                if *family == GeometryWireFamily::F3dzex2 {
                    // Pinned MIT RT64 maps F3DZEX2 opcode 0x04 to BranchW:
                    // bits 1..7 select one of 128 cache slots and the
                    // retained transformed homogeneous W is compared
                    // strictly against the unsigned command threshold.
                    let vertex_slot = ((w0 >> 1) & 0x7f) as usize;
                    let cache_capacity = family.cache_capacity();
                    assert!(
                        vertex_slot < cache_capacity,
                        "F3DZEX2 G_BRANCH_W cache slot {vertex_slot} is outside slots 0..={}",
                        cache_capacity - 1
                    );
                    assert!(
                        state.vtx_loaded[vertex_slot],
                        "F3DZEX2 G_BRANCH_W cache slot {vertex_slot} has not been loaded"
                    );
                    let vertex_w = state.vtx_cache[vertex_slot].w;
                    assert!(
                        vertex_w.is_finite(),
                        "F3DZEX2 G_BRANCH_W cache slot {vertex_slot} has non-finite transformed W {vertex_w}"
                    );
                    if state.force_branch || vertex_w < w1 as f32 {
                        let target = state.rdp_half_1.unwrap_or_else(|| {
                            panic!(
                                "F3DZEX2 G_BRANCH_W reached without a preceding G_RDPHALF_1 target"
                            )
                        });
                        let target_pc = resolve_addr(&state.segments, target) & 0x00ff_fff8;
                        let target_end = target_pc.checked_add(8).unwrap_or_else(|| {
                            panic!(
                                "F3DZEX2 G_BRANCH_W target {target_pc:#010x} overflows the host address space"
                            )
                        });
                        assert!(
                            target_end <= rdram.len(),
                            "F3DZEX2 G_BRANCH_W target {target_pc:#010x} has no complete 8-byte command in rdram_bytes={}",
                            rdram.len()
                        );
                        pc = target_pc;
                        continue;
                    }
                    continue;
                }
                // gSPBranchLessZraw redundantly packs the same cache slot as
                // v*5 (vertex record offset) and v*2 (screen-Z offset).
                let encoded_vertex = ((w0 >> 12) & 0x0fff) as usize;
                let encoded_z = (w0 & 0x0fff) as usize;
                assert!(
                    encoded_vertex.is_multiple_of(5) && encoded_z.is_multiple_of(2),
                    "G_BRANCH_Z malformed vertex offsets v*5={encoded_vertex:#05x} v*2={encoded_z:#05x}"
                );
                let vertex_slot = encoded_vertex / 5;
                let z_slot = encoded_z / 2;
                assert_eq!(
                    vertex_slot, z_slot,
                    "G_BRANCH_Z vertex offsets select different cache slots {vertex_slot} and {z_slot}"
                );
                let cache_capacity = family.cache_capacity();
                assert!(
                    vertex_slot < cache_capacity,
                    "{} G_BRANCH_Z cache slot {vertex_slot} is outside slots 0..={}",
                    family.name(),
                    cache_capacity - 1
                );
                let vertex = &state.vtx_cache[vertex_slot];
                // Keep transactional task admission on the exact path the
                // active native RT64 enhancement will execute. Otherwise a
                // forced branch could select different self-load and
                // FullSync commands after preflight has committed its plan.
                if state.force_branch || vertex.z_screen <= w1 {
                    let target = state.rdp_half_1.unwrap_or_else(|| {
                        panic!("G_BRANCH_Z reached without a preceding G_RDPHALF_1 target")
                    });
                    pc = resolve_addr(&state.segments, target);
                    continue;
                }
            }
            G_VTX => {
                // F3DEX2 G_VTX (F3DEX2-CONCEPTS.md §2.1): the RSP-side wire
                // layout is n = field(w0,12,8), end-index = field(w0,1,7),
                // and the destination start slot v0 = end - n. w1 = segmented
                // vertex-array address. (NOT the F3DEX/SDK-macro `/2` form,
                // which misplaces vertices -- failure risk #2.)
                let n = ((w0 >> 12) & 0xFF) as usize;
                let end = ((w0 >> 1) & 0x7F) as usize;
                let cache_capacity = family.cache_capacity();
                let max_load = family.max_vertex_load_count();
                assert!(
                    (1..=max_load).contains(&n),
                    "{} G_VTX count {n} must be in 1..={max_load} at RDRAM {:#010x}",
                    family.name(),
                    pc - 8
                );
                assert!(
                    end <= cache_capacity && end >= n,
                    "{} G_VTX encoded end slot {end} and count {n} do not select a cache range within slots 0..={} at RDRAM {:#010x}",
                    family.name(),
                    cache_capacity - 1,
                    pc - 8
                );
                let v0 = end - n;
                load_vertices(rdram, state, w1, n, v0, *family);
            }
            G_MODIFYVTX => {
                // Public F3DEX2 gSPModifyVertex packs `where` in w0[23:16],
                // cache slot * 2 in w0[15:0], and the replacement in w1.
                // Values are already post-transform cache values: RGBA bytes,
                // signed S10.5 ST, signed S13.2 screen XY, or unsigned 16.16
                // screen Z. In particular ST is not multiplied by G_TEXTURE
                // here; the manual requires callers to provide that scaled
                // value themselves.
                let where_field = ((w0 >> 16) & 0xFF) as u8;
                let encoded_slot = (w0 & 0xFFFF) as usize;
                assert!(
                    encoded_slot.is_multiple_of(2),
                    "G_MODIFYVTX cache index encoding {encoded_slot:#06x} is not divisible by two"
                );
                let slot = encoded_slot / 2;
                let cache_capacity = family.cache_capacity();
                assert!(
                    slot < cache_capacity,
                    "{} G_MODIFYVTX cache slot {slot} is outside slots 0..={}",
                    family.name(),
                    cache_capacity - 1
                );
                let vertex = &mut state.vtx_cache[slot];
                match where_field {
                    G_MWO_POINT_RGBA => {
                        [vertex.r, vertex.g, vertex.b, vertex.a] = w1.to_be_bytes();
                    }
                    G_MWO_POINT_ST => {
                        let [s_hi, s_lo, t_hi, t_lo] = w1.to_be_bytes();
                        vertex.s = i16::from_be_bytes([s_hi, s_lo]) as f32 / 32.0;
                        vertex.t = i16::from_be_bytes([t_hi, t_lo]) as f32 / 32.0;
                    }
                    G_MWO_POINT_XYSCREEN => {
                        let [x_hi, x_lo, y_hi, y_lo] = w1.to_be_bytes();
                        vertex.x = i16::from_be_bytes([x_hi, x_lo]) as f32 / 4.0;
                        vertex.y = i16::from_be_bytes([y_hi, y_lo]) as f32 / 4.0;
                        vertex.clip_position = None;
                    }
                    G_MWO_POINT_ZSCREEN => {
                        vertex.z = w1 as f32 / 65536.0;
                        vertex.z_screen = w1;
                        vertex.clip_position = None;
                    }
                    _ => crate::render_unsupported_panic(
                        "render.gbi.geometry.modify-vtx",
                        format!(
                            "G_MODIFYVTX cache slot {slot} uses unsupported where field {where_field:#04x}"
                        ),
                    ),
                }
            }
            G_LINE3D => {
                // Public F3DEX2 gbi.h packs v0*2, v1*2, and the half-pixel
                // width increment into w0[23:16], w0[15:8], and w0[7:0].
                // The flat-shade flag is already expressed by swapping the
                // two encoded endpoints; w1 is reserved and emitted as zero.
                assert_eq!(w1, 0, "G_LINE3D reserved second word must be zero");
                let encoded = [((w0 >> 16) & 0xff) as usize, ((w0 >> 8) & 0xff) as usize];
                assert!(
                    encoded.iter().all(|value| value.is_multiple_of(2)),
                    "G_LINE3D cache indices must use F3DEX2 v*2 encoding: {} and {}",
                    encoded[0],
                    encoded[1]
                );
                let slots = [encoded[0] / 2, encoded[1] / 2];
                let width_parameter = (w0 & 0xff) as u8;
                if let Some(line) = resolve_line(
                    &state.vtx_cache,
                    slots,
                    width_parameter,
                    LineDecodeSnapshot {
                        smooth_shading: state.geometry_mode & G_SHADING_SMOOTH != 0,
                        texture: active_texture(&state.tex, state.other_mode),
                        other_mode: state.other_mode,
                        combiner: state.combiner,
                        blender: active_blender(state),
                        scissor: state.scissor,
                        viewport: state.viewport,
                        clip_ratio: state.clip_ratio,
                    },
                ) {
                    state.ops.push(RenderOp::Line(line));
                }
            }
            G_TRI1 => {
                // F3DEX2 G_TRI1 (F3DEX2-CONCEPTS.md §2.2): three 7-bit
                // vertex-cache-slot fields in w0 at bits 17/9/1 -- each is
                // already the slot (0-31), no /2 needed.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex, state.other_mode);
                let blender = active_blender(state);
                let idx = tri_indices(w0);
                if let Some(mut t) = resolve_tri_for_profile(
                    &state.vtx_cache,
                    idx,
                    state.profile,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
            }
            G_TRI2 | G_QUAD => {
                // F3DEX2 G_TRI2 / G_QUAD (§2.3): triangle A's three 7-bit
                // slot fields in w0 (bits 17/9/1), triangle B's in w1 at the
                // SAME bit positions. G_QUAD decodes identically to G_TRI2.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex, state.other_mode);
                let blender = active_blender(state);
                let idx_a = tri_indices(w0);
                let idx_b = tri_indices(w1);
                if let Some(mut t) = resolve_tri_for_profile(
                    &state.vtx_cache,
                    idx_a,
                    state.profile,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture.clone(),
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
                if let Some(mut t) = resolve_tri_for_profile(
                    &state.vtx_cache,
                    idx_b,
                    state.profile,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
            }
            G_MTX => {
                // F3DEX2 gsSPMatrix (gbi.h ~2106): w0 = op<<24 |
                // ((len-1)/8)<<19 | (ofs/8)<<8 | idx; the low byte on the
                // wire is `idx = params ^ G_MTX_PUSH`. F3DEX_GBI_2 param bits
                // (gbi.h:233-239): PROJECTION=0x04, LOAD=0x02, PUSH=0x01.
                // Un-XOR the push bit to recover the caller's params. w1 =
                // segmented matrix address.
                let wire_idx = (w0 & 0xFF) as u8;
                let destination_offset_div8 = ((w0 >> 8) & 0xFF) as u8;
                let length_div8_minus_one = ((w0 >> 19) & 0x1F) as u8;
                assert_eq!(
                    destination_offset_div8, 0,
                    "G_MTX destination offset/8 must be zero"
                );
                assert_eq!(length_div8_minus_one, 7, "G_MTX must carry one 64-byte Mtx");
                assert_eq!(
                    wire_idx & !0x07,
                    0,
                    "G_MTX wire parameter {wire_idx:#04x} contains non-public flag bits"
                );
                let params = wire_idx ^ 0x01; // ^ G_MTX_PUSH
                let is_projection = params & 0x04 != 0; // G_MTX_PROJECTION
                let is_load = params & 0x02 != 0; // G_MTX_LOAD
                let is_push = params & 0x01 != 0; // G_MTX_PUSH
                let addr = resolve_addr(&state.segments, w1);
                let mtx = read_mtx(rdram, addr).unwrap_or_else(|| {
                    panic!(
                        "G_MTX reads past RDRAM: source={addr:#x}, bytes=64, rdram_bytes={}",
                        rdram.len()
                    )
                });
                {
                    #[cfg(not(test))]
                    if projdump::on() {
                        eprintln!(
                            "[FN64_DUMP_PROJ] G_MTX proj={} load={} push={} @rdram=0x{addr:06x} seg_w1=0x{w1:08x} mv_depth={} rows=[{:?} | {:?} | {:?} | {:?}]",
                            is_projection,
                            is_load,
                            is_push,
                            state.mv_stack.len(),
                            mtx[0],
                            mtx[1],
                            mtx[2],
                            mtx[3]
                        );
                    }
                    if is_projection {
                        // The projection matrix ALSO honors LOAD vs MUL. OoT
                        // loads the perspective matrix once with LOAD, then
                        // concatenates the camera/view matrix onto it with
                        // PROJECTION|MUL (guLookAt output). Treating every
                        // projection G_MTX as a LOAD (a prior bug) let the
                        // view matrix -- whose 4th row is [0,0,0,1], no
                        // projective term -- OVERWRITE the real perspective
                        // matrix (4th row [0,0,-1,0]).
                        //
                        // MUL ORDER (hardware/RT64): the incoming matrix
                        // multiplies on the LEFT of the accumulated
                        // projection -- `viewProj = new * viewProj` (RT64
                        // rt64_rsp.cpp:171). So the perspective LOAD gives
                        // `proj = P`, then the view MUL gives `proj = V * P`,
                        // and the final MVP below is `M * (V * P)`. This is
                        // the row-vector hardware product built column-major
                        // for our column-vector `transform_point`.
                        state.proj = Some(if is_load {
                            mtx
                        } else {
                            match state.proj {
                                Some(p) => mat_mul(&mtx, &p),
                                None => mtx,
                            }
                        });
                    } else {
                        // Modelview: a PUSH saves the current top so a later
                        // G_POPMTX restores it. LOAD replaces, MUL
                        // concatenates. MUL puts the incoming matrix on the
                        // LEFT (`modelview = new * modelview`, RT64
                        // rt64_rsp.cpp:197) so successive object transforms
                        // compose in the same order the hardware applies them.
                        if is_push {
                            state.mv_stack.push(state.modelview);
                        }
                        if is_load {
                            state.modelview = mtx;
                        } else {
                            state.modelview = mat_mul(&mtx, &state.modelview);
                        }
                    }
                    recompute_mvp(state);
                }
            }
            G_POPMTX => {
                // F3DEX2 gsSPPopMatrixN encodes the requested count as a
                // byte address `num * 64` in w1. Only the modelview stack is
                // public for this command.
                #[cfg(not(test))]
                if projdump::on() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] G_POPMTX mv_depth_before={}",
                        state.mv_stack.len()
                    );
                }
                assert!(
                    w1.is_multiple_of(64) && w1 != 0,
                    "G_POPMTX count address {w1:#010x} must be a nonzero multiple of 64"
                );
                let count = (w1 / 64) as usize;
                assert!(
                    count <= state.mv_stack.len(),
                    "G_POPMTX requests {count} entries from modelview depth {}",
                    state.mv_stack.len()
                );
                for _ in 0..count {
                    state.modelview = state
                        .mv_stack
                        .pop()
                        .expect("validated G_POPMTX depth changed during pop");
                }
                recompute_mvp(state);
            }
            G_DMA_IO => {
                let rsp_memory = rsp_memory.as_deref_mut().unwrap_or_else(|| {
                    panic!("G_DMA_IO requires execute_display_list_f3dex2_ops with live RSP memory")
                });
                execute_dma_io(rdram, rsp_memory, &state.segments, w0, w1);
            }
            G_LOAD_UCODE => {
                let loading_family = *family;
                let rsp_memory = rsp_memory.as_deref_mut().unwrap_or_else(|| {
                    panic!(
                        "G_LOAD_UCODE requires execute_display_list_f3dex2_ops with live RSP memory"
                    )
                });
                let data_address = state.rdp_half_1.unwrap_or_else(|| {
                    panic!(
                        "G_LOAD_UCODE reached without the compound command's preceding G_RDPHALF_1 data address"
                    )
                });
                let loaded = execute_load_ucode(rdram, rsp_memory, w0, w1, data_address);
                if loading_family.is_legacy_loadable() {
                    reset_legacy_rsp_state_from_ucode_load(state);
                } else {
                    reset_rsp_state_from_ucode_load(state);
                }
                if let Some(catalog) = ucode_catalog {
                    if let Some(next_profile) = catalog.profile(loaded.text_sha256) {
                        let next_family = next_profile.wire_family();
                        let generation = TaskAdmissionGeneration {
                            source: TaskAdmissionSource::SelfLoad,
                            text_address: loaded.text_address,
                            data_address: loaded.data_address,
                            text_sha256: loaded.text_sha256,
                            data: loaded.data,
                            ucode: next_profile.admission_ucode(),
                        };
                        if let Some((text_bytes, data_bytes)) = state.admission_raw_window_bytes {
                            let raw_window = capture_raw_recognition_window(
                                rdram,
                                generation.text_address,
                                text_bytes,
                                "text",
                            )
                            .and_then(|text| {
                                capture_raw_recognition_window(
                                    rdram,
                                    generation.data_address,
                                    data_bytes,
                                    "data",
                                )
                                .map(|data| TaskAdmissionRawWindow { text, data })
                            });
                            match raw_window {
                                Ok(raw_window) => state.admission_raw_windows.push(raw_window),
                                Err(reason) => {
                                    state.admission_raw_window_error = Some(reason);
                                    break;
                                }
                            }
                        }
                        state.admission_generations.push(generation);
                        *family = next_family;
                        initialize_geometry_profile_state(state, next_profile);
                    } else {
                        state.unsupported_ucode_reload = Some(loaded.text_sha256);
                        break;
                    }
                }
            }
            G_MOVEWORD => {
                // F3DEX2 gsMoveWd (gbi.h ~2267): w0 = op<<24 | index<<16 |
                // offset<<0 (16-bit offset); w1 = data. Segment table write
                // is index==G_MW_SEGMENT, segment number = offset/4, base =
                // w1 (masked to a physical rdram offset).
                let index = ((w0 >> 16) & 0xFF) as u16;
                let offset = (w0 & 0xFFFF) as u16;
                if index == G_MW_SEGMENT {
                    assert!(
                        offset.is_multiple_of(4),
                        "G_MOVEWORD G_MW_SEGMENT offset {offset:#06x} is not word aligned"
                    );
                    let seg = (offset / 4) as usize;
                    assert!(
                        seg < state.segments.len(),
                        "G_MOVEWORD G_MW_SEGMENT index {seg} exceeds segments 0..=15"
                    );
                    // Base is a physical rdram address; strip any KSEG high
                    // bits, keep the low 24 (segments span rdram).
                    state.segments[seg] = w1 & 0x00FF_FFFF;
                } else if index == G_MW_NUMLIGHT {
                    // F3DEX2 gsSPNumLights (gbi.h:2887): data = NUML(n) =
                    // n*24, so the directional-light count is w1/24. The
                    // ambient light lives in slot `num_dir` (gbi.h:2902:
                    // "the highest numbered light is always the ambient").
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_NUMLIGHT offset must be zero");
                    assert!(
                        w1.is_multiple_of(24),
                        "G_MOVEWORD G_MW_NUMLIGHT value {w1} is not a 24-byte light stride"
                    );
                    let n = (w1 / 24) as usize;
                    assert!(
                        n < MAX_LIGHTS,
                        "G_MOVEWORD G_MW_NUMLIGHT directional count {n} exceeds seven"
                    );
                    state.lights.num_dir = n;
                } else if index == G_MW_CLIP {
                    if family.is_reject() {
                        let ratio = w1 as u16;
                        assert!(
                            !matches!(ratio, 1 | 0xffff),
                            "F3DLX.Rej reject-box ratio must be public FRUSTRATIO_2..6"
                        );
                    }
                    state.clip_ratio.write(offset, w1);
                } else if index == G_MW_LIGHTCOL {
                    // Public gSPLightColor emits the same RGBA word to the
                    // primary and copied color destinations. Alpha is ignored;
                    // neither write changes the retained light direction.
                    let slot = light_slot_from_moveword_offset(offset).unwrap_or_else(|| {
                        panic!(
                            "G_MOVEWORD G_MW_LIGHTCOL offset {offset:#06x} is not a public F3DEX2 light-color destination"
                        )
                    });
                    set_light_color(state, slot, w1);
                } else if index == G_MW_FOG {
                    assert_eq!(
                        offset, 0,
                        "G_MOVEWORD G_MW_FOG offset must be G_MWO_FOG (zero)"
                    );
                    state.fog = FogFactor {
                        multiplier: (w1 >> 16) as u16 as i16,
                        offset: w1 as u16 as i16,
                    };
                } else if index == G_MW_FORCEMTX {
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_FORCEMTX offset must be zero");
                    assert_eq!(
                        w1, 0x0001_0000,
                        "G_MOVEWORD G_MW_FORCEMTX marker must be 0x00010000"
                    );
                    state.mvp = Some(state.pending_forced_mvp.take().expect(
                        "G_MOVEWORD G_MW_FORCEMTX requires a preceding G_MOVEMEM G_MV_MATRIX",
                    ));
                } else if index == G_MW_PERSPNORM {
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_PERSPNORM offset must be zero");
                    assert_eq!(
                        w1 & 0xffff_0000,
                        0,
                        "G_MOVEWORD G_MW_PERSPNORM scale must be a public u16 value"
                    );
                    state.persp_normalize = PerspectiveNormalize(Some(w1 as u16));
                } else {
                    crate::render_unsupported_panic(
                        "render.gbi.geometry.moveword",
                        format!(
                            "G_MOVEWORD unsupported index {index:#04x} offset {offset:#06x}: w0={w0:#010x} w1={w1:#010x}"
                        ),
                    );
                }
            }
            G_DL => {
                // F3DEX2 gsSPDisplayList / gsSPBranchList (gbi.h ~2174-2178):
                // both pack via gDma1p(G_DL, dl, 0, p) so w0 = op<<24 |
                // p<<16, w1 = segmented address of the target DL. The `p`
                // byte at bits 16-23 is the push flag: G_DL_PUSH=0 (gbi.h:966)
                // is a CALL (push a return address, resume the caller after
                // the callee's G_ENDDL); G_DL_NOPUSH=1 (gbi.h:967) is a
                // BRANCH/tail-jump (gsSPBranchList) that REPLACES the current
                // DL pointer -- the target runs in place of the rest of this
                // stream and there is NO return to the bytes after the branch.
                //
                // BUG FIXED HERE: previously both cases recursed and then
                // *continued* decoding the current stream after return. For a
                // BRANCH that is wrong -- the words after a gsSPBranchList are
                // not commands (typically zero-fill or the next unrelated
                // buffer), so the decoder walked straight into garbage and
                // every trailing byte became a bogus "unrecognized opcode",
                // cascading the whole frame into ~14K junk skips (proven from
                // a live OoT gameplay task: the root DL's first command is a
                // gsSPBranchList `w0=0xde01_0000` whose trailing bytes are all
                // zero). We now recurse into the target and then STOP the
                // current stream for a branch (mirroring RT64's runDl, which
                // only pushes a return address when the push bit is clear).
                let is_branch = ((w0 >> 16) & 0x01) != 0; // G_DL_NOPUSH
                if is_branch {
                    // Tail branch: the target REPLACES the current DL
                    // pointer -- on hardware this consumes NO return-stack
                    // entry, so it must not recurse or count against
                    // MAX_DL_DEPTH (OoT chains branch lists deeper than any
                    // fixed cap; the old recursing version falsely tripped
                    // it). A self-referencing branch cycle is bounded by
                    // MAX_DL_COMMANDS at the loop top.
                    pc = resolve_addr(&state.segments, w1);
                    continue;
                }
                if state.dl_depth < MAX_DL_DEPTH {
                    // NOTE: G_DL is a pure address call/return -- it does NOT
                    // save or restore the matrix stack. The RSP's modelview/
                    // projection state is GLOBAL across a nested DL; only
                    // G_MTX (with G_MTX_PUSH) and G_POPMTX push/pop matrices.
                    // A previous version wrapped the recursion in a
                    // modelview push/pop, which corrupted transforms after a
                    // nested DL returned -- gameplay geometry (deeply nested
                    // DLs) then projected to ±100k px off-screen. We now
                    // recurse with shared global matrix state, exactly like
                    // the hardware call/return (RT64 push/popReturnAddress
                    // only saves the DL pointer, never the matrix).
                    state.dl_depth += 1;
                    decode_stream(
                        rdram,
                        w1,
                        state,
                        rsp_memory.as_deref_mut(),
                        ucode_catalog,
                        family,
                    );
                    state.dl_depth -= 1;
                    if state.unsupported_ucode_reload.is_some() {
                        break;
                    }
                } else {
                    panic!(
                        "G_DL call at RDRAM {:#010x} exceeds the {MAX_DL_DEPTH}-entry display-list stack",
                        pc - 8
                    );
                }
            }
            G_TEXTURE => {
                // F3DEX2 gsSPTexture (§5.2): on-bit field(w0,1,7), tile
                // field(w0,8,3), max-level field(w0,11,3), S scale field(w1,16,16), T scale
                // field(w1,0,16) (both U0.16). Latch enable + tile + scale so
                // the next G_LOAD*/G_TRI can bind + address a texture.
                let on = ((w0 >> 1) & 0x7F) != 0;
                let tile = ((w0 >> 8) & 0x07) as u8;
                let scale_s = ((w1 >> 16) & 0xFFFF) as f32 / 65536.0;
                let scale_t = (w1 & 0xFFFF) as f32 / 65536.0;
                state.tex.tex_enabled = on;
                state.tex.tex_tile = tile;
                state.tex.tex_max_level = ((w0 >> 11) & 0x07) as u8;
                state.tex.tex_scale_s = scale_s;
                state.tex.tex_scale_t = scale_t;
            }
            G_RDPSETOTHERMODE => {
                // Full expert-mode write: high 24 bits live in w0's payload,
                // low 32 bits in w1 (gbi.h:3697-3737). OoT's setup DLs use
                // this path as well as the F3DEX2 partial setters.
                state.other_mode.high = w0 & 0x00FF_FFFF;
                state.other_mode.low = w1;
            }
            G_SETOTHERMODE_H => {
                // F3DEX2 gSPSetOtherMode (`gbi.h:3353-3369`) stores
                // `32-shift-len` at w0[15:8] and `len-1` at w0[7:0]. Rebuild
                // the selected H mask and preserve every other bit, matching
                // RT64's decode/update split (`rt64_gbi_f3dex2.cpp:24-33`,
                // `rt64_rsp.cpp:1026-1037`).
                state.other_mode.high = update_other_mode_word(state.other_mode.high, w0, w1)
                    .unwrap_or_else(|| {
                        panic!(
                            "malformed G_SETOTHERMODE_H range at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                            pc - 8
                        )
                    });
            }
            G_SETOTHERMODE_L => {
                state.other_mode.low = update_other_mode_word(state.other_mode.low, w0, w1)
                    .unwrap_or_else(|| {
                        panic!(
                            "malformed G_SETOTHERMODE_L range at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                            pc - 8
                        )
                    });
            }
            G_SETBLENDCOLOR => {
                // Public gbi.h:3646-3650 packs RGBA into w1, alpha in bits
                // 7..0. Threshold alpha compare uses precisely this component
                // (OoT z_rcp.c:815-835; RT64 RasterPS.hlsl:209-211).
                state.other_mode.blend_color_alpha = w1 as u8;
                state.blend_color = w1.to_be_bytes();
            }
            G_SETFOGCOLOR => state.fog_color = w1.to_be_bytes(),
            G_SETKEYGB => state.combiner.key.set_gb(w0, w1),
            G_SETKEYR => state.combiner.key.set_r(w1),
            G_SETCONVERT => {
                state.combiner.convert = ConvertState::decode(w0, w1);
            }
            G_SETCIMG => {
                // Public gDPSetColorImage packing: format[23:21], size[20:19],
                // width-1[11:0], and image address in w1. The F3DEX2 command
                // processor resolves segmented addresses before the RDP sees
                // them; this decoder performs the same mapping explicitly.
                state.ops.push(RenderOp::SetColorImage(ColorImage {
                    format: ((w0 >> 21) & 0x07) as u8,
                    size: ((w0 >> 19) & 0x03) as u8,
                    width: ((w0 & 0x0fff) + 1) as u16,
                    address: u32::try_from(resolve_addr(&state.segments, w1))
                        .expect("resolved color-image address exceeds u32"),
                }));
            }
            G_SETZIMG => {
                state.ops.push(RenderOp::SetDepthImage(DepthImage {
                    address: u32::try_from(resolve_addr(&state.segments, w1))
                        .expect("resolved depth-image address exceeds u32"),
                }));
            }
            G_SETPRIMDEPTH => {
                // Public gDPSetPrimDepth uses the generic set-color packing:
                // Z in the high halfword, DeltaZ in the low halfword.
                state.ops.push(RenderOp::SetPrimitiveDepth(PrimitiveDepth {
                    z: (w1 >> 16) as u16,
                    delta_z: w1 as u16,
                }));
            }
            G_SETFILLCOLOR => {
                // gDPSetFillColor writes the raw 32-bit fill register. On a
                // 16-bit color image its high/low RGBA5551 halfwords alternate
                // across logical pixels.
                state.fill_color = w1;
            }
            G_FILLRECT => {
                // Public gDPFillRectangle packing uses lower-right in w0 and
                // upper-left in w1, all as unsigned quarter-pixel fields.
                // Fill-cycle lower-right coverage is inclusive; raster.rs
                // applies that rule together with the exclusive scissor.
                state.ops.push(RenderOp::FillRectangle(FillRectangle {
                    ulx: ((w1 >> 12) & 0x0fff) as f32 / 4.0,
                    uly: (w1 & 0x0fff) as f32 / 4.0,
                    lrx: ((w0 >> 12) & 0x0fff) as f32 / 4.0,
                    lry: (w0 & 0x0fff) as f32 / 4.0,
                    fill_color: state.fill_color,
                    cycle_type: state.other_mode.cycle_type(),
                    scissor: state.scissor,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                }));
            }
            G_RDPLOADSYNC | G_RDPPIPESYNC | G_RDPTILESYNC => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "{} reserved first-word payload must be zero",
                    opcode_name(opcode)
                );
                // SGI RDP Command Summary Tables 1/32/33 assign no field to
                // this word. F3DEX2 macros generate zero and remain checked,
                // while the raw RDP stream admitted by DPC can retain an
                // unrelated word there; the hardware command has no input to
                // consume it.
                if !raw_rdp {
                    assert_eq!(
                        w1,
                        0,
                        "{} reserved second word must be zero",
                        opcode_name(opcode)
                    );
                }
            }
            G_RDPFULLSYNC => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_RDPFULLSYNC reserved first-word payload must be zero"
                );
                if !raw_rdp {
                    assert_eq!(w1, 0, "G_RDPFULLSYNC reserved second word must be zero");
                }
                state.ops.push(RenderOp::FullSync);
            }
            G_SETTIMG => {
                // G_SETTIMG (§5.1): format field(w0,21,3), size field(w0,19,2),
                // width-1 field(w0,0,12), image addr w1 (segmented). Pointer +
                // format latch only; no texel data moves until a G_LOAD*.
                state.tex.timg_fmt = ((w0 >> 21) & 0x07) as u8;
                state.tex.timg_siz = ((w0 >> 19) & 0x03) as u8;
                state.tex.timg_width = ((w0 & 0x0fff) + 1) as u16;
                state.tex.timg_addr = w1;
            }
            G_SETTILE => {
                // G_SETTILE (§5.1): w0 = fmt field(w0,21,3), siz field(w0,19,2),
                // line field(w0,9,9), tmem field(w0,0,9); w1 = tile
                // field(w1,24,3), palette field(w1,20,4), cmT field(w1,18,2),
                // maskT field(w1,14,4), shiftT field(w1,10,4), cmS
                // field(w1,8,2), maskS field(w1,4,4), shiftS field(w1,0,4).
                // In each cm field bit0 enables mirror and bit1 enables clamp.
                let tile = ((w1 >> 24) & 0x07) as usize;
                apply_set_tile(&mut state.tex.tiles[tile], w0, w1);
            }
            G_SETTILESIZE => {
                // G_SETTILESIZE (§5.1): w0 = uls field(w0,12,12), ult
                // field(w0,0,12); w1 = tile field(w1,24,3), lrs field(w1,12,12),
                // lrt field(w1,0,12). Coords are S10.5 (÷4 for texel extent).
                let uls = ((w0 >> 12) & 0xFFF) as u16;
                let ult = (w0 & 0xFFF) as u16;
                let tile = ((w1 >> 24) & 0x07) as usize;
                let lrs = ((w1 >> 12) & 0xFFF) as u16;
                let lrt = (w1 & 0xFFF) as u16;
                let t = &mut state.tex.tiles[tile];
                t.uls = uls;
                t.ult = ult;
                t.lrs = lrs;
                t.lrt = lrt;
            }
            G_LOADTLUT => {
                // G_LOADTLUT (§5.1): load a CI palette from the latched TIMG
                // image. Public gbi.h packs `num - 1` directly into the
                // 10-bit field at bits 14..23. TLUT entries are 16-bit
                // RGBA5551 in RDRAM.
                let count = load_tlut_count(w1);
                let base = resolve_addr(&state.segments, state.tex.timg_addr);
                assert_texture_source_range(rdram, base, count - 1, G_IM_SIZ_16B, "G_LOADTLUT");
                let tile_index = ((w1 >> 24) & 0x07) as usize;
                let tmem_base = state.tex.tiles[tile_index].tmem;
                let mut tlut = Vec::with_capacity(count);
                for i in 0..count {
                    let px = read_u16(rdram, base + i * 2);
                    tlut.push(rgba5551_to_rgba8888(px));
                    std::rc::Rc::make_mut(&mut state.tex.tmem).write_tlut(tmem_base, i, px);
                }
                state.tex.tlut = tlut;
                let tile = &mut state.tex.tiles[tile_index];
                tile.uls = ((w0 >> 12) & 0x0fff) as u16;
                tile.ult = (w0 & 0x0fff) as u16;
                tile.lrs = ((w1 >> 12) & 0x0fff) as u16;
                tile.lrt = (w1 & 0x0fff) as u16;
            }
            G_LOADBLOCK | G_LOADTILE => {
                // G_LOADBLOCK / G_LOADTILE (§5.1): DMA source texels into the
                // physical 4 KiB TMEM image using this LOAD tile's base,
                // stride, size, and odd-row bank exchange. A later render
                // tile can reinterpret the same bytes with a different
                // format, extent, or tile number.
                let tile = ((w1 >> 24) & 0x07) as usize;
                if opcode == G_LOADTILE {
                    load_tile_into_tmem(rdram, &mut state.tex, &state.segments, tile, w0, w1);
                } else {
                    load_block_into_tmem(rdram, &mut state.tex, &state.segments, tile, w0, w1);
                }
            }
            G_MOVEMEM => {
                // F3DEX2 gsMoveMem (§1.4): w0 low byte = index (which RSP
                // block), field(w0,8,8) = offset/8, w1 = segmented source
                // address. G_MV_VIEWPORT (index 8) points at a 16-byte `Vp`;
                // G_MV_LIGHT (index 0x0a) addresses the two public LookAt
                // directions followed by the 16-byte directional/ambient
                // `Light` records. G_MV_MATRIX stages the public force-matrix
                // compound operation. Point indices remain unimplemented.
                let index = (w0 & 0xFF) as u8;
                let ofs_div8 = ((w0 >> 8) & 0xFF) as usize;
                let length_div8_minus_one = ((w0 >> 19) & 0x1f) as usize;
                if index == G_MV_VIEWPORT {
                    assert_eq!(
                        ofs_div8, 0,
                        "G_MOVEMEM G_MV_VIEWPORT destination offset must be zero"
                    );
                    assert_eq!(
                        length_div8_minus_one, 1,
                        "G_MOVEMEM G_MV_VIEWPORT must carry one 16-byte Vp"
                    );
                    let addr = resolve_addr(&state.segments, w1);
                    state.viewport = Some(read_viewport(rdram, addr).unwrap_or_else(|| {
                        panic!(
                            "G_MOVEMEM G_MV_VIEWPORT reads past RDRAM: source={addr:#x}, bytes=16, rdram_bytes={}",
                            rdram.len()
                        )
                    }));
                } else if index == G_MV_LIGHT {
                    assert_eq!(
                        length_div8_minus_one, 1,
                        "G_MOVEMEM G_MV_LIGHT must carry one 16-byte Light or LookAt record"
                    );
                    // Public F3DEX2 gbi.h assigns offsets 0*24 and 1*24 to
                    // LookAt X/Y. gsSPLight starts at 2*24; LIGHT_1 therefore
                    // maps to slot 0 after the two reserved entries.
                    let addr = resolve_addr(&state.segments, w1);
                    if ofs_div8 == 0 {
                        load_look_at(rdram, state, addr, LookAtAxis::X);
                    } else if ofs_div8 == 3 {
                        load_look_at(rdram, state, addr, LookAtAxis::Y);
                    } else if let Some(slot) = light_slot_from_movemem_offset(ofs_div8) {
                        load_light(rdram, state, addr, slot);
                    } else {
                        panic!(
                            "G_MOVEMEM G_MV_LIGHT offset/8 {ofs_div8:#04x} is not a public LookAt or light destination"
                        );
                    }
                } else if index == G_MV_MATRIX {
                    assert_eq!(
                        ofs_div8, 0,
                        "G_MOVEMEM G_MV_MATRIX destination offset must be zero"
                    );
                    assert_eq!(
                        length_div8_minus_one, 7,
                        "G_MOVEMEM G_MV_MATRIX must carry one 64-byte Mtx"
                    );
                    let addr = resolve_addr(&state.segments, w1);
                    state.pending_forced_mvp = Some(read_mtx(rdram, addr).unwrap_or_else(|| {
                        panic!("G_MOVEMEM G_MV_MATRIX reads past RDRAM: source={addr:#x}, bytes=64")
                    }));
                } else {
                    crate::render_unsupported_panic(
                        "render.gbi.geometry.movemem",
                        format!(
                            "G_MOVEMEM unsupported index {index:#04x} offset/8 {ofs_div8:#04x}: w0={w0:#010x} w1={w1:#010x}"
                        ),
                    );
                }
            }
            G_GEOMETRYMODE => {
                // F3DEX2 gsSPGeometryMode (§2.4): one atomic clear+set --
                // `mode = (mode & field(w0,0,24)) | w1`, where the w0 low 24
                // bits are the (already-inverted) AND mask. We honor the
                // CULL_FRONT/CULL_BACK bits per-triangle (see cull_mode_from)
                // and the G_LIGHTING bit at G_VTX time (cn = normal -> lit
                // color, see load_vertices). G_FOG replaces vertex alpha from
                // projected depth; shade-smooth remains incomplete.
                let and_mask = w0 & 0x00FF_FFFF;
                state.geometry_mode = (state.geometry_mode & and_mask) | w1;
            }
            G_SETCOMBINE => {
                // Public gbi.h GCCc*w* packing macros (lines 3543-3565)
                // distribute both cycles' RGB/alpha A/B/C/D selectors across
                // w0/w1. `CombinerMode::decode` resolves those raw selectors
                // to semantic sources using the position-specific mux tables.
                state.combiner.mode = CombinerMode::decode(w0, w1);
            }
            G_SETPRIMCOLOR => {
                // gDPSetPrimColor (gbi.h:3672-3682): w0 low byte is the
                // primitive LOD fraction, the preceding byte is the minimum
                // LOD clamp, and w1 is RGBA8888.
                state.combiner.min_lod_level = ((w0 >> 8) & 0xff) as u8;
                state.combiner.prim_lod_fraction = (w0 & 0xff) as u8;
                state.combiner.primitive = w1.to_be_bytes();
            }
            G_SETENVCOLOR => {
                // gDPSetEnvColor -> DPRGBColor (gbi.h:3626-3644): w1 packs
                // RGBA in bits 31..0, one byte per component.
                state.combiner.environment = w1.to_be_bytes();
            }
            G_SETSCISSOR => {
                // SGI RDP Command Summary Table 27: all four edges are
                // unsigned 12-bit quarter-pixels; w1 bits 25/24 enable field
                // scissoring and select the odd field (zero keeps even).
                // The lower-right edge is exclusive: OoT PreRender.c:137
                // passes `lrx + 1` / `lry + 1` when converting its inclusive stored bounds.
                // RT64 likewise stores the fixed rect (rt64_rdp.cpp:974-980)
                // and intersects triangle bounds with it
                // (rt64_rsp.cpp:1140-1154).
                state.scissor = Some(ScissorRect {
                    ulx: ((w0 >> 12) & 0x0FFF) as f32 / 4.0,
                    uly: (w0 & 0x0FFF) as f32 / 4.0,
                    lrx: ((w1 >> 12) & 0x0FFF) as f32 / 4.0,
                    lry: (w1 & 0x0FFF) as f32 / 4.0,
                    field: w1 & (1 << 25) != 0,
                    keep_odd: w1 & (1 << 24) != 0,
                });
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                let (coords, gradients, continuation_bytes) =
                    decode_texture_rectangle_continuation(rdram, pc, *family, raw_rdp, opcode);
                pc += continuation_bytes;
                if continuation_bytes == 16 {
                    state.cmds_decoded += 2;
                    assert!(
                        state.cmds_decoded <= MAX_DL_COMMANDS,
                        "{} display list exceeded the {MAX_DL_COMMANDS}-command budget in the {} continuation at RDRAM {command_pc:#010x}",
                        family.name(),
                        opcode_name(opcode)
                    );
                }
                let tile = ((w1 >> 24) & 0x07) as u8;
                let storage = state.tex.tmem.clone();
                state.ops.push(RenderOp::TextureRectangle(TextureRectangle {
                    ulx: ((w1 >> 12) & 0x0fff) as f32 / 4.0,
                    uly: (w1 & 0x0fff) as f32 / 4.0,
                    lrx: ((w0 >> 12) & 0x0fff) as f32 / 4.0,
                    lry: (w0 & 0x0fff) as f32 / 4.0,
                    tile,
                    s: ((coords >> 16) as u16 as i16) as f32 / 32.0,
                    t: (coords as u16 as i16) as f32 / 32.0,
                    dsdx: (gradients >> 16) as u16 as i16,
                    dtdy: gradients as u16 as i16,
                    flip: opcode == G_TEXRECTFLIP,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                    scissor: state.scissor,
                    texture: bind_texture_set(&state.tex, tile, 0, state.other_mode.texture_lut()),
                    texture1: texture_for_tile(
                        &state.tex,
                        tile.wrapping_add(1) & 7,
                        state.other_mode.texture_lut(),
                        &storage,
                    ),
                }));
            }
            G_SPECIAL_1 | G_SPECIAL_2 | G_SPECIAL_3 => panic!(
                "reserved {} command {} at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                family.name(),
                opcode_name(opcode),
                pc - 8
            ),
            G_ENDDL => break,
            _ => crate::render_unsupported_panic(
                "render.gbi.geometry.command",
                format!(
                    "unsupported {} command {} ({opcode:#04x}) at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                    family.name(),
                    opcode_name(opcode),
                    pc - 8
                ),
            ),
        }
    }
}
