//! Transactional control-flow inspection for admitted geometry tasks.
//!
//! This is deliberately not a renderer. It executes only the published GBI
//! state that can change which command is reached: display-list links,
//! segmented pointers, debug DMA, microcode replacement, and the transform /
//! vertex fields consumed by `G_CULLDL`, `G_BRANCH_Z`, and F3DZEX2
//! `G_BRANCH_W`. RDP drawing state is irrelevant here; the only RDP command
//! retained is FullSync completion.
//!
//! Provenance: command encodings and retained-state rules are from the public
//! `gbi.h`, Fast3D/F3DEX/F3DEX2/L3DEX manuals, SGI RSP Programmer's Guide
//! chapter 4, SGI RDP Command Summary, and pinned MIT RT64
//! `rt64_gbi_f3dzex2.cpp`/`rt64_rsp.cpp` for BranchW software parity. The
//! optional forced-branch policy is fn64's typed rendering policy for pinned
//! MIT RT64's public enhancement.

use crate::{
    DpFullSyncStatus, GeometryUcodeCatalog, GeometryUcodeProfile, GeometryWireFamily,
    MicrocodeDataImageIdentity, OsTask, RenderError, TaskAdmissionGeneration, TaskAdmissionPlan,
    TaskAdmissionSource, UcodeDigest,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut, RspMemAddr, RspMemory, RspMemoryBank};
use sha2::{Digest, Sha256};

const INSPECTOR: &str = "geometry-task-inspection";
const SP_UCODE_SIZE: usize = fn64_runtime::RSP_MEMORY_BANK_SIZE;
const MAX_COMMANDS: u32 = 1 << 20;
const MODERN_DL_DEPTH: usize = 18;
const LEGACY_DL_DEPTH: usize = 10;
const VTX_STRIDE: usize = 16;

const G_NOOP: u8 = 0x00;
const G_VTX: u8 = 0x01;
const G_MODIFYVTX: u8 = 0x02;
const G_CULLDL: u8 = 0x03;
const G_BRANCH_Z: u8 = 0x04;
const G_TRI1: u8 = 0x05;
const G_TRI2: u8 = 0x06;
const G_QUAD: u8 = 0x07;
const G_LINE3D: u8 = 0x08;
const G_SPECIAL_3: u8 = 0xd3;
const G_SPECIAL_2: u8 = 0xd4;
const G_SPECIAL_1: u8 = 0xd5;
const G_DMA_IO: u8 = 0xd6;
const G_TEXTURE: u8 = 0xd7;
const G_POPMTX: u8 = 0xd8;
const G_GEOMETRYMODE: u8 = 0xd9;
const G_MTX: u8 = 0xda;
const G_MOVEWORD: u8 = 0xdb;
const G_MOVEMEM: u8 = 0xdc;
const G_LOAD_UCODE: u8 = 0xdd;
const G_DL: u8 = 0xde;
const G_ENDDL: u8 = 0xdf;
const G_SPNOOP: u8 = 0xe0;
const G_RDPHALF_1: u8 = 0xe1;
const G_TEXRECT: u8 = 0xe4;
const G_TEXRECTFLIP: u8 = 0xe5;
const G_RDPFULLSYNC: u8 = 0xe9;
const G_RDPHALF_2: u8 = 0xf1;

const LEGACY_G_SPNOOP: u8 = 0x00;
const LEGACY_G_MTX: u8 = 0x01;
const LEGACY_G_MOVEMEM: u8 = 0x03;
const LEGACY_G_VTX: u8 = 0x04;
const LEGACY_G_DL: u8 = 0x06;
const LEGACY_G_LOAD_UCODE: u8 = 0xaf;
const LEGACY_G_BRANCH_Z: u8 = 0xb0;
const LEGACY_G_TRI2: u8 = 0xb1;
const LEGACY_G_MODIFYVTX: u8 = 0xb2;
const LEGACY_G_RDPHALF_2: u8 = 0xb3;
const LEGACY_G_RDPHALF_1: u8 = 0xb4;
const LEGACY_G_LINE3D: u8 = 0xb5;
const LEGACY_G_CLEAR_GEOMETRYMODE: u8 = 0xb6;
const LEGACY_G_SET_GEOMETRYMODE: u8 = 0xb7;
const LEGACY_G_ENDDL: u8 = 0xb8;
const LEGACY_G_SETOTHERMODE_L: u8 = 0xb9;
const LEGACY_G_SETOTHERMODE_H: u8 = 0xba;
const LEGACY_G_TEXTURE: u8 = 0xbb;
const LEGACY_G_MOVEWORD: u8 = 0xbc;
const LEGACY_G_POPMTX: u8 = 0xbd;
const LEGACY_G_CULLDL: u8 = 0xbe;
const LEGACY_G_TRI1: u8 = 0xbf;
const LEGACY_G_NOOP: u8 = 0xc0;
const LEGACY_G_MW_POINTS: u32 = 0x0c;

const G_MV_VIEWPORT: u8 = 0x08;
const G_MV_LIGHT: u8 = 0x0a;
const G_MV_MATRIX: u8 = 0x0e;
const G_MW_MATRIX: u16 = 0x00;
const G_MW_NUMLIGHT: u16 = 0x02;
const G_MW_CLIP: u16 = 0x04;
const G_MW_SEGMENT: u16 = 0x06;
const G_MW_FOG: u16 = 0x08;
const G_MW_LIGHTCOL: u16 = 0x0a;
const G_MW_FORCEMTX: u16 = 0x0c;
const G_MW_PERSPNORM: u16 = 0x0e;
const G_MWO_POINT_RGBA: u8 = 0x10;
const G_MWO_POINT_ST: u8 = 0x14;
const G_MWO_POINT_XYSCREEN: u8 = 0x18;
const G_MWO_POINT_ZSCREEN: u8 = 0x1c;

/// Host policy that changes public display-list control flow.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GeometryTaskInspectionPolicy {
    /// Force every supported geometry branch to take its staged target.
    pub force_branch: bool,
}

/// Optional raw backing-store window requested by a native adapter.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TaskAdmissionRawWindowSize {
    pub text: usize,
    pub data: usize,
}

/// Raw host-storage bytes at one exact microcode activation boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskAdmissionRawWindow {
    pub text: Vec<u8>,
    pub data: Vec<u8>,
}

/// Immutable result of one successful transactional task inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryTaskInspection {
    pub admission_plan: TaskAdmissionPlan,
    /// Empty when no raw window was requested; otherwise one per generation,
    /// including task entry at index zero.
    pub raw_windows: Box<[TaskAdmissionRawWindow]>,
    pub dp_full_sync: DpFullSyncStatus,
    pub full_sync_count: u64,
}

type Mat4 = [[f32; 4]; 4];

#[derive(Copy, Clone, Debug, Default)]
struct ControlVertex {
    clip_code: u8,
    z_screen: u32,
    clip_w: Option<f32>,
}

#[derive(Copy, Clone, Debug)]
struct Viewport {
    sz: f32,
    tz: f32,
}

struct WalkState {
    profile: GeometryUcodeProfile,
    segments: [u32; 16],
    vertices: [ControlVertex; 128],
    projection: Option<Mat4>,
    modelview: Mat4,
    mvp: Option<Mat4>,
    pending_forced_mvp: Option<Mat4>,
    modelview_stack: Vec<Mat4>,
    viewport: Option<Viewport>,
    persp_normalize: Option<u16>,
    rdp_half_1: Option<u32>,
    return_stack: Vec<usize>,
    commands: u32,
    full_sync_count: u64,
    self_loads: Vec<TaskAdmissionGeneration>,
    raw_windows: Vec<TaskAdmissionRawWindow>,
}

impl WalkState {
    fn new(profile: GeometryUcodeProfile) -> Self {
        Self {
            profile,
            segments: [0; 16],
            vertices: [ControlVertex::default(); 128],
            projection: None,
            modelview: identity(),
            mvp: None,
            pending_forced_mvp: None,
            modelview_stack: Vec::new(),
            viewport: None,
            persp_normalize: None,
            rdp_half_1: None,
            return_stack: Vec::new(),
            commands: 0,
            full_sync_count: 0,
            self_loads: Vec::new(),
            raw_windows: Vec::new(),
        }
    }

    fn reset_after_ucode_load(
        &mut self,
        loading_profile: GeometryUcodeProfile,
    ) -> Result<(), RenderError> {
        self.vertices = [ControlVertex::default(); 128];
        self.rdp_half_1 = None;
        if loading_profile.wire_family().is_legacy_loadable() {
            if !self.return_stack.is_empty() {
                return Err(reject_unsupported(
                    "F3DEX/L3DEX G_LOAD_UCODE inside a called display list resets link state and cannot return",
                ));
            }
            self.segments = [0; 16];
            self.projection = None;
            self.modelview = identity();
            self.mvp = None;
            self.pending_forced_mvp = None;
            self.modelview_stack.clear();
            self.viewport = None;
            self.persp_normalize = None;
        } else {
            self.mvp = None;
            self.pending_forced_mvp = None;
        }
        Ok(())
    }

    fn family(&self) -> GeometryWireFamily {
        self.profile.wire_family()
    }

    fn recompute_mvp(&mut self) {
        self.pending_forced_mvp = None;
        self.mvp = Some(match self.projection {
            Some(projection) => mat_mul(&self.modelview, &projection),
            None => self.modelview,
        });
    }
}

/// Inspect one admitted public polygon-family task without mutating its input
/// memories.
pub fn inspect_geometry_task(
    rdram: &[u8],
    rsp_memory: &RspMemory,
    task: &OsTask,
    catalog: &GeometryUcodeCatalog,
    policy: GeometryTaskInspectionPolicy,
    raw_window_size: Option<TaskAdmissionRawWindowSize>,
) -> Result<GeometryTaskInspection, RenderError> {
    if let Some(size) = raw_window_size {
        if size.text == 0 || size.data == 0 {
            return Err(reject("raw recognition window sizes must be nonzero"));
        }
    }

    let live_text = rsp_memory.bank(RspMemoryBank::Imem);
    let profile = catalog.require_profile_text(live_text)?;
    let family = profile.wire_family();
    if !is_supported_polygon_family(family) {
        return Err(reject_unsupported(format!(
            "{} task inspection is outside the supported public polygon-family frontier",
            family.name()
        )));
    }
    let entry = task_entry(rdram, live_text, task, profile)?;
    let mut raw_windows = Vec::new();
    if let Some(size) = raw_window_size {
        raw_windows.push(capture_raw_window(
            rdram,
            entry.text_address,
            entry.data_address,
            size,
        )?);
    }

    let mut scratch_rdram = rdram.to_vec();
    let mut scratch_rsp = rsp_memory.clone();
    let mut state = WalkState::new(profile);
    state.raw_windows = raw_windows;
    walk(
        &mut scratch_rdram,
        &mut scratch_rsp,
        task.data_ptr,
        catalog,
        policy,
        raw_window_size,
        &mut state,
    )?;
    let full_sync_count = state.full_sync_count;
    let admission_plan = TaskAdmissionPlan::new(entry, state.self_loads);
    if raw_window_size.is_some() && state.raw_windows.len() != admission_plan.len() {
        return Err(reject(format!(
            "raw recognition windows ({}) differ from admission generations ({})",
            state.raw_windows.len(),
            admission_plan.len()
        )));
    }
    Ok(GeometryTaskInspection {
        admission_plan,
        raw_windows: state.raw_windows.into_boxed_slice(),
        dp_full_sync: if full_sync_count == 0 {
            DpFullSyncStatus::NotReached
        } else {
            DpFullSyncStatus::Reached
        },
        full_sync_count,
    })
}

fn task_entry(
    rdram: &[u8],
    live_text: &[u8; SP_UCODE_SIZE],
    task: &OsTask,
    profile: GeometryUcodeProfile,
) -> Result<TaskAdmissionGeneration, RenderError> {
    let text_address = task.ucode & 0x00ff_ffff;
    let data_address = task.ucode_data & 0x00ff_ffff;
    let data_bytes = usize::try_from(task.ucode_data_size)
        .map_err(|_| reject("task microcode data size does not fit usize"))?;
    validate_dma_image(text_address, SP_UCODE_SIZE, rdram.len(), "task text")?;
    validate_dma_image(data_address, data_bytes, rdram.len(), "task data")?;
    if data_bytes == 0 || data_bytes > SP_UCODE_SIZE || !data_bytes.is_multiple_of(8) {
        return Err(reject(format!(
            "task microcode data size {data_bytes} must be a nonzero 64-bit multiple no larger than 4 KiB"
        )));
    }
    let text = logical_bytes(rdram, text_address, SP_UCODE_SIZE)?;
    let text_sha256 = UcodeDigest::from_text(&text);
    let live_sha256 = UcodeDigest::from_text(live_text);
    if text_sha256 != live_sha256 {
        return Err(RenderError::RequiresLle {
            ucode_sha256: live_sha256.as_bytes(),
        });
    }
    let data = logical_bytes(rdram, data_address, data_bytes)?;
    Ok(TaskAdmissionGeneration {
        source: TaskAdmissionSource::TaskEntry,
        text_address,
        data_address,
        text_sha256,
        data: MicrocodeDataImageIdentity {
            bytes: task.ucode_data_size,
            sha256: Sha256::digest(data).into(),
        },
        ucode: profile.admission_ucode(),
    })
}

fn walk(
    rdram: &mut [u8],
    rsp_memory: &mut RspMemory,
    start: u32,
    catalog: &GeometryUcodeCatalog,
    policy: GeometryTaskInspectionPolicy,
    raw_window_size: Option<TaskAdmissionRawWindowSize>,
    state: &mut WalkState,
) -> Result<(), RenderError> {
    let mut pc = resolve_addr(&state.segments, start)?;
    loop {
        state.commands = state
            .commands
            .checked_add(1)
            .ok_or_else(|| reject("display-list command count overflow"))?;
        if state.commands > MAX_COMMANDS {
            return Err(reject(format!(
                "display list exceeded the {MAX_COMMANDS}-command budget; cyclic or corrupt command graph"
            )));
        }
        checked_range(pc, 8, rdram.len(), "display-list command")?;
        let command_pc = pc;
        let wire_w0 = read_u32(rdram, pc)?;
        let wire_w1 = read_u32(rdram, pc + 4)?;
        pc += 8;
        if consume_line_triangle_noop(state.family(), wire_w0, wire_w1)? {
            continue;
        }
        let (w0, w1) = normalize_command(state.family(), wire_w0, wire_w1, command_pc)?;
        let opcode = (w0 >> 24) as u8;

        match opcode {
            G_RDPHALF_1 => state.rdp_half_1 = Some(w1),
            G_CULLDL => {
                let start = usize::try_from(w0 & 0xffff).expect("u16 fits usize") / 2;
                let end = usize::try_from(w1 & 0xffff).expect("u16 fits usize") / 2;
                let capacity = state.family().cache_capacity();
                if !(start < end && end < capacity) {
                    return Err(reject(format!(
                        "{} G_CULLDL range {start}..={end} is outside its {capacity}-entry cache",
                        state.family().name()
                    )));
                }
                let common = state.vertices[start..=end]
                    .iter()
                    .fold(u8::MAX, |bits, vertex| bits & vertex.clip_code);
                if common != 0 {
                    if let Some(return_pc) = state.return_stack.pop() {
                        pc = return_pc;
                    } else {
                        return Ok(());
                    }
                }
            }
            G_BRANCH_Z if state.family() == GeometryWireFamily::F3dzex2 => {
                let slot = ((w0 >> 1) & 0x7f) as usize;
                if slot >= state.family().cache_capacity() {
                    return Err(reject(format!(
                        "{} G_BRANCH_W selects out-of-range cache slot {slot}",
                        state.family().name()
                    )));
                }
                let clip_w = state.vertices[slot].clip_w.ok_or_else(|| {
                    reject(format!(
                        "{} G_BRANCH_W selects unloaded cache slot {slot}",
                        state.family().name()
                    ))
                })?;
                if !clip_w.is_finite() {
                    return Err(reject(format!(
                        "{} G_BRANCH_W cache slot {slot} has non-finite transformed W",
                        state.family().name()
                    )));
                }
                if policy.force_branch || clip_w < w1 as f32 {
                    let target = state.rdp_half_1.ok_or_else(|| {
                        reject("G_BRANCH_W reached without a preceding G_RDPHALF_1 target")
                    })?;
                    pc = resolve_branch_w_target(&state.segments, target, rdram.len())?;
                }
            }
            G_BRANCH_Z => {
                let encoded_vertex = ((w0 >> 12) & 0x0fff) as usize;
                let encoded_z = (w0 & 0x0fff) as usize;
                if !encoded_vertex.is_multiple_of(5) || !encoded_z.is_multiple_of(2) {
                    return Err(reject(format!(
                        "G_BRANCH_Z malformed cache offsets v*5={encoded_vertex:#x}, v*2={encoded_z:#x}"
                    )));
                }
                let slot = encoded_vertex / 5;
                if slot != encoded_z / 2 || slot >= state.family().cache_capacity() {
                    return Err(reject(format!(
                        "{} G_BRANCH_Z selects inconsistent/out-of-range cache slot {slot}",
                        state.family().name()
                    )));
                }
                if policy.force_branch || state.vertices[slot].z_screen <= w1 {
                    let target = state.rdp_half_1.ok_or_else(|| {
                        reject("G_BRANCH_Z reached without a preceding G_RDPHALF_1 target")
                    })?;
                    pc = resolve_addr(&state.segments, target)?;
                }
            }
            G_VTX => load_vertices(rdram, w0, w1, state)?,
            G_MODIFYVTX => modify_vertex(w0, w1, state)?,
            G_MTX => apply_matrix(rdram, w0, w1, state)?,
            G_POPMTX => pop_matrix(w0, w1, state)?,
            G_DMA_IO => execute_dma_io(rdram, rsp_memory, &state.segments, w0, w1)?,
            G_LOAD_UCODE => {
                let data_address = state.rdp_half_1.ok_or_else(|| {
                    reject("G_LOAD_UCODE reached without a preceding G_RDPHALF_1 data address")
                })?;
                let loading_profile = state.profile;
                let loaded = execute_load_ucode(rdram, rsp_memory, w0, w1, data_address)?;
                state.reset_after_ucode_load(loading_profile)?;
                let Some(next_profile) = catalog.profile(loaded.text_sha256) else {
                    return Err(RenderError::RequiresLle {
                        ucode_sha256: loaded.text_sha256.as_bytes(),
                    });
                };
                let next_family = next_profile.wire_family();
                if !is_supported_polygon_family(next_family) {
                    return Err(reject_unsupported(format!(
                        "G_LOAD_UCODE selected {}, outside the supported public polygon-family frontier",
                        next_family.name()
                    )));
                }
                let generation = TaskAdmissionGeneration {
                    source: TaskAdmissionSource::SelfLoad,
                    text_address: loaded.text_address,
                    data_address: loaded.data_address,
                    text_sha256: loaded.text_sha256,
                    data: loaded.data,
                    ucode: next_profile.admission_ucode(),
                };
                if let Some(size) = raw_window_size {
                    state.raw_windows.push(capture_raw_window(
                        rdram,
                        generation.text_address,
                        generation.data_address,
                        size,
                    )?);
                }
                state.self_loads.push(generation);
                state.profile = next_profile;
            }
            G_MOVEWORD => apply_moveword(rdram, w0, w1, state)?,
            G_MOVEMEM => apply_movemem(rdram, w0, w1, state)?,
            G_DL => {
                let branch = (w0 >> 16) & 1 != 0;
                let target = resolve_addr(&state.segments, w1)?;
                if branch {
                    pc = target;
                } else {
                    let limit = if is_modern(state.family()) {
                        MODERN_DL_DEPTH
                    } else {
                        LEGACY_DL_DEPTH
                    };
                    if state.return_stack.len() >= limit {
                        return Err(reject(format!(
                            "{} G_DL call exceeds its {limit}-entry display-list stack",
                            state.family().name()
                        )));
                    }
                    state.return_stack.push(pc);
                    pc = target;
                }
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                let next = skip_texture_rectangle_continuation(rdram, pc, state.family(), opcode)?;
                let continuation_commands = u32::try_from((next - pc) / 8)
                    .expect("texture-rectangle continuation count fits u32");
                state.commands = state
                    .commands
                    .checked_add(continuation_commands)
                    .ok_or_else(|| reject("display-list command count overflow"))?;
                if state.commands > MAX_COMMANDS {
                    return Err(reject(format!(
                        "display list exceeded the {MAX_COMMANDS}-command budget in a texture-rectangle continuation"
                    )));
                }
                pc = next;
            }
            G_RDPFULLSYNC => {
                state.full_sync_count = state
                    .full_sync_count
                    .checked_add(1)
                    .ok_or_else(|| reject("FullSync count overflow"))?;
            }
            G_ENDDL => {
                if let Some(return_pc) = state.return_stack.pop() {
                    pc = return_pc;
                } else {
                    return Ok(());
                }
            }
            G_SPECIAL_1 | G_SPECIAL_2 | G_SPECIAL_3 => {
                return Err(reject_unsupported(format!(
                    "reserved {} command {opcode:#04x} at RDRAM {command_pc:#010x}",
                    state.family().name()
                )));
            }
            opcode if is_non_control_command(opcode) => {}
            _ => {
                return Err(reject_unsupported(format!(
                    "unsupported {} command {opcode:#04x} at RDRAM {command_pc:#010x}: w0={wire_w0:#010x} w1={wire_w1:#010x}",
                    state.family().name()
                )));
            }
        }
    }
}

fn normalize_command(
    family: GeometryWireFamily,
    w0: u32,
    w1: u32,
    pc: usize,
) -> Result<(u32, u32), RenderError> {
    if is_modern(family) {
        return Ok((w0, w1));
    }
    let opcode = (w0 >> 24) as u8;
    let normalized = match opcode {
        LEGACY_G_SPNOOP => (u32::from(G_SPNOOP) << 24, w1),
        LEGACY_G_MTX => {
            let params = ((w0 >> 16) & 0xff) as u8;
            if w0 & 0xffff != 64 || params & !7 != 0 {
                return Err(reject(format!(
                    "malformed {} G_MTX at {pc:#010x}",
                    family.name()
                )));
            }
            let modern = ((params & 1) << 2) | (params & 2) | ((params & 4) >> 2);
            (
                (u32::from(G_MTX) << 24) | (7 << 19) | u32::from(modern ^ 1),
                w1,
            )
        }
        LEGACY_G_MOVEMEM => {
            let index = ((w0 >> 16) & 0xff) as u8;
            if w0 & 0xffff != 16 {
                return Err(reject(format!(
                    "malformed {} G_MOVEMEM length at {pc:#010x}",
                    family.name()
                )));
            }
            let (index, offset) = match index {
                0x80 => (G_MV_VIEWPORT, 0),
                0x82 => (G_MV_LIGHT, 3),
                0x84 => (G_MV_LIGHT, 0),
                0x86..=0x94 if index.is_multiple_of(2) => {
                    let light = usize::from((index - 0x86) / 2) + 1;
                    (G_MV_LIGHT, (light + 1) as u8 * 3)
                }
                _ => {
                    return Err(reject_unsupported(format!(
                        "unsupported {} G_MOVEMEM index {index:#04x} at {pc:#010x}",
                        family.name()
                    )))
                }
            };
            (
                (u32::from(G_MOVEMEM) << 24)
                    | (1 << 19)
                    | (u32::from(offset) << 8)
                    | u32::from(index),
                w1,
            )
        }
        LEGACY_G_VTX => normalize_legacy_vertex(family, w0, w1, pc)?,
        LEGACY_G_DL => {
            let parameter = (w0 >> 16) & 0xff;
            if !matches!(parameter, 0 | 1) || w0 & 0xffff != 0 {
                return Err(reject(format!(
                    "malformed {} G_DL at {pc:#010x}",
                    family.name()
                )));
            }
            ((u32::from(G_DL) << 24) | (parameter << 16), w1)
        }
        LEGACY_G_LOAD_UCODE if family.is_legacy_loadable() => {
            ((u32::from(G_LOAD_UCODE) << 24) | (w0 & 0x00ff_ffff), w1)
        }
        LEGACY_G_BRANCH_Z if family.uses_legacy_polygon_wire() => {
            ((u32::from(G_BRANCH_Z) << 24) | (w0 & 0x00ff_ffff), w1)
        }
        LEGACY_G_MODIFYVTX if family.uses_legacy_polygon_wire() => {
            ((u32::from(G_MODIFYVTX) << 24) | (w0 & 0x00ff_ffff), w1)
        }
        LEGACY_G_RDPHALF_1 => (u32::from(G_RDPHALF_1) << 24, w1),
        LEGACY_G_RDPHALF_2 => (u32::from(G_RDPHALF_2) << 24, w1),
        LEGACY_G_CULLDL => normalize_legacy_cull(family, w0, w1, pc)?,
        LEGACY_G_ENDDL => (u32::from(G_ENDDL) << 24, w1),
        LEGACY_G_POPMTX => (u32::from(G_POPMTX) << 24, 64),
        LEGACY_G_MOVEWORD => normalize_legacy_moveword(family, w0, w1, pc)?,
        LEGACY_G_LINE3D if family == GeometryWireFamily::Fast3d => (u32::from(G_LINE3D) << 24, w1),
        LEGACY_G_CLEAR_GEOMETRYMODE | LEGACY_G_SET_GEOMETRYMODE => {
            (u32::from(G_GEOMETRYMODE) << 24, w1)
        }
        LEGACY_G_SETOTHERMODE_L => (0xe200_0000 | (w0 & 0xffff), w1),
        LEGACY_G_SETOTHERMODE_H => (0xe300_0000 | (w0 & 0xffff), w1),
        LEGACY_G_TEXTURE => (u32::from(G_TEXTURE) << 24, w1),
        LEGACY_G_TRI1 | LEGACY_G_TRI2 => (u32::from(G_TRI1) << 24, w1),
        LEGACY_G_NOOP => (u32::from(G_NOOP) << 24, w1),
        0xe4..=0xff => (w0, w1),
        _ => {
            return Err(reject_unsupported(format!(
                "unsupported {} command byte {opcode:#04x} at RDRAM {pc:#010x}",
                family.name()
            )))
        }
    };
    Ok(normalized)
}

fn normalize_legacy_vertex(
    family: GeometryWireFamily,
    w0: u32,
    w1: u32,
    pc: usize,
) -> Result<(u32, u32), RenderError> {
    let parameter = ((w0 >> 16) & 0xff) as usize;
    let length = w0 & 0xffff;
    let (count, start) = if family == GeometryWireFamily::Fast3d {
        let count = (parameter >> 4) + 1;
        let start = parameter & 0x0f;
        if length != (count * VTX_STRIDE) as u32 || start + count > 16 {
            return Err(reject(format!("malformed Fast3D G_VTX at {pc:#010x}")));
        }
        (count, start)
    } else {
        if !parameter.is_multiple_of(2) {
            return Err(reject(format!(
                "malformed {} G_VTX destination at {pc:#010x}",
                family.name()
            )));
        }
        let start = parameter / 2;
        let count = ((length >> 10) & 0x3f) as usize;
        if count == 0
            || length & 0x03ff != (count * VTX_STRIDE - 1) as u32
            || start + count > family.cache_capacity()
        {
            return Err(reject(format!(
                "malformed {} G_VTX at {pc:#010x}",
                family.name()
            )));
        }
        (count, start)
    };
    Ok((
        (u32::from(G_VTX) << 24) | ((count as u32) << 12) | (((start + count) as u32) << 1),
        w1,
    ))
}

fn normalize_legacy_cull(
    family: GeometryWireFamily,
    w0: u32,
    w1: u32,
    pc: usize,
) -> Result<(u32, u32), RenderError> {
    let (start, end) = if family == GeometryWireFamily::Fast3d {
        let start_bytes = (w0 & 0x00ff_ffff) as usize;
        let end_bytes = w1 as usize;
        if !start_bytes.is_multiple_of(40) || !end_bytes.is_multiple_of(40) {
            return Err(reject(format!("malformed Fast3D G_CULLDL at {pc:#010x}")));
        }
        let start = start_bytes / 40;
        let end_exclusive = end_bytes / 40;
        if start >= end_exclusive || end_exclusive > 16 {
            return Err(reject(format!(
                "Fast3D G_CULLDL range {start}..{end_exclusive} exceeds its 16-entry cache at {pc:#010x}"
            )));
        }
        (start, end_exclusive - 1)
    } else {
        if w0 & 0x00ff_0000 != 0 || w1 & 0xffff_0000 != 0 {
            return Err(reject(format!(
                "{} G_CULLDL reserved payload is nonzero at {pc:#010x}",
                family.name()
            )));
        }
        let encoded = [(w0 & 0xffff) as usize, (w1 & 0xffff) as usize];
        if encoded.iter().any(|value| !value.is_multiple_of(2)) {
            return Err(reject(format!(
                "malformed {} G_CULLDL at {pc:#010x}",
                family.name()
            )));
        }
        (encoded[0] / 2, encoded[1] / 2)
    };
    Ok((
        (u32::from(G_CULLDL) << 24) | (start as u32 * 2),
        end as u32 * 2,
    ))
}

fn normalize_legacy_moveword(
    family: GeometryWireFamily,
    w0: u32,
    w1: u32,
    pc: usize,
) -> Result<(u32, u32), RenderError> {
    let offset = (w0 >> 8) & 0xffff;
    let index = w0 & 0xff;
    if index == LEGACY_G_MW_POINTS {
        let slot = offset / 40;
        let field = offset % 40;
        if slot >= family.cache_capacity() as u32 || !matches!(field, 0x10 | 0x14 | 0x18 | 0x1c) {
            return Err(reject(format!(
                "malformed {} G_MW_POINTS at {pc:#010x}",
                family.name()
            )));
        }
        return Ok((
            (u32::from(G_MODIFYVTX) << 24) | (field << 16) | (slot * 2),
            w1,
        ));
    }
    Ok(((u32::from(G_MOVEWORD) << 24) | (index << 16) | offset, w1))
}

fn consume_line_triangle_noop(
    family: GeometryWireFamily,
    w0: u32,
    w1: u32,
) -> Result<bool, RenderError> {
    let opcode = (w0 >> 24) as u8;
    let packed = match family {
        GeometryWireFamily::L3dex2 if opcode == G_TRI1 => w0 & 0x00ff_ffff,
        GeometryWireFamily::L3dex if opcode == LEGACY_G_TRI1 => w1 & 0x00ff_ffff,
        _ => return Ok(false),
    };
    let encoded = [(packed >> 16) as u8, (packed >> 8) as u8, packed as u8];
    if encoded
        .iter()
        .any(|value| !value.is_multiple_of(2) || *value > 62)
    {
        return Err(reject(format!(
            "{} line-triangle NOOP has malformed vertex packing {encoded:?}",
            family.name()
        )));
    }
    Ok(true)
}

fn is_modern(family: GeometryWireFamily) -> bool {
    matches!(
        family,
        GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
            | GeometryWireFamily::F3dzex2
            | GeometryWireFamily::L3dex2
    )
}

fn is_supported_polygon_family(family: GeometryWireFamily) -> bool {
    matches!(
        family,
        GeometryWireFamily::Fast3d
            | GeometryWireFamily::F3dex
            | GeometryWireFamily::F3dlx
            | GeometryWireFamily::F3dlxRej
            | GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
    )
}

fn is_non_control_command(opcode: u8) -> bool {
    matches!(
        opcode,
        G_NOOP
            | G_TRI1
            | G_TRI2
            | G_QUAD
            | G_LINE3D
            | G_TEXTURE
            | G_GEOMETRYMODE
            | G_SPNOOP
            | 0xe2
            | 0xe3
            | 0xe6..=0xff
    )
}

fn apply_moveword(
    _rdram: &[u8],
    w0: u32,
    w1: u32,
    state: &mut WalkState,
) -> Result<(), RenderError> {
    let index = ((w0 >> 16) & 0xff) as u16;
    let offset = (w0 & 0xffff) as u16;
    match index {
        G_MW_SEGMENT => {
            if !offset.is_multiple_of(4) || usize::from(offset / 4) >= 16 {
                return Err(reject(format!(
                    "G_MOVEWORD segment offset {offset:#06x} is invalid"
                )));
            }
            state.segments[usize::from(offset / 4)] = w1 & 0x00ff_ffff;
        }
        G_MW_FORCEMTX => {
            if offset != 0 || w1 != 0x0001_0000 {
                return Err(reject("G_MOVEWORD G_MW_FORCEMTX marker is malformed"));
            }
            state.mvp = Some(state.pending_forced_mvp.take().ok_or_else(|| {
                reject("G_MOVEWORD G_MW_FORCEMTX lacks a preceding G_MOVEMEM matrix")
            })?);
        }
        G_MW_PERSPNORM => {
            if offset != 0 || w1 & 0xffff_0000 != 0 {
                return Err(reject("G_MOVEWORD G_MW_PERSPNORM is malformed"));
            }
            state.persp_normalize = Some(w1 as u16);
        }
        G_MW_NUMLIGHT | G_MW_CLIP | G_MW_FOG | G_MW_LIGHTCOL => {}
        // Public gbi.h's G_MW_MATRIX (index 0x00) writes raw fixed-point bytes
        // directly into the RSP's retained model/proj/MVP matrix triple at an
        // implementation-defined byte offset (pinned MIT RT64
        // `rt64_rsp.cpp::insertMatrix`). This inspector's `Mat4` holds resolved
        // f32 values, not the RSP's split integer/fraction fixed-point layout,
        // so a byte patch cannot be represented losslessly here. Named
        // separately from the generic unsupported-index frontier because it is
        // a real, documented index this inspector deliberately cannot track.
        G_MW_MATRIX => {
            return Err(reject_unsupported(format!(
                "G_MOVEWORD G_MW_MATRIX raw fixed-point matrix patch at offset {offset:#06x} is outside this inspector's f32 matrix model"
            )))
        }
        _ => {
            return Err(reject_unsupported(format!(
                "unsupported {} G_MOVEWORD index {index:#04x} offset {offset:#06x}",
                state.family().name()
            )))
        }
    }
    Ok(())
}

fn apply_movemem(rdram: &[u8], w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    let index = (w0 & 0xff) as u8;
    let offset = ((w0 >> 8) & 0xff) as usize;
    let length = ((w0 >> 19) & 0x1f) as usize;
    match index {
        G_MV_VIEWPORT => {
            if offset != 0 || length != 1 {
                return Err(reject("G_MOVEMEM viewport fields are malformed"));
            }
            let address = resolve_addr(&state.segments, w1)?;
            state.viewport = Some(read_viewport(rdram, address)?);
        }
        G_MV_MATRIX => {
            if offset != 0 || length != 7 {
                return Err(reject("G_MOVEMEM force-matrix fields are malformed"));
            }
            let address = resolve_addr(&state.segments, w1)?;
            state.pending_forced_mvp = Some(read_matrix(rdram, address)?);
        }
        G_MV_LIGHT if length == 1 => {}
        G_MV_LIGHT => {
            return Err(reject(format!(
                "G_MOVEMEM light length {length} is malformed"
            )))
        }
        _ => {
            return Err(reject_unsupported(format!(
                "unsupported {} G_MOVEMEM index {index:#04x} offset/8 {offset:#04x}",
                state.family().name()
            )))
        }
    }
    Ok(())
}

fn load_vertices(rdram: &[u8], w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    let count = ((w0 >> 12) & 0xff) as usize;
    let end = ((w0 >> 1) & 0x7f) as usize;
    if count == 0
        || count > state.family().max_vertex_load_count()
        || end < count
        || end > state.family().cache_capacity()
    {
        return Err(reject(format!(
            "{} G_VTX count/end {count}/{end} is outside its cache",
            state.family().name()
        )));
    }
    let start = end - count;
    let source = resolve_addr(&state.segments, w1)?;
    checked_range(source, count * VTX_STRIDE, rdram.len(), "G_VTX source")?;
    for index in 0..count {
        let address = source + index * VTX_STRIDE;
        let x = f32::from(read_i16(rdram, address)?);
        let y = f32::from(read_i16(rdram, address + 2)?);
        let z = f32::from(read_i16(rdram, address + 4)?);
        state.vertices[start + index] = project_vertex(state, x, y, z)?;
    }
    Ok(())
}

fn modify_vertex(w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    let field = ((w0 >> 16) & 0xff) as u8;
    let encoded = (w0 & 0xffff) as usize;
    if !encoded.is_multiple_of(2) || encoded / 2 >= state.family().cache_capacity() {
        return Err(reject("G_MODIFYVTX cache index is malformed"));
    }
    match field {
        G_MWO_POINT_ZSCREEN => state.vertices[encoded / 2].z_screen = w1,
        G_MWO_POINT_RGBA | G_MWO_POINT_ST | G_MWO_POINT_XYSCREEN => {}
        _ => {
            return Err(reject_unsupported(format!(
                "G_MODIFYVTX field {field:#04x} is unsupported"
            )))
        }
    }
    Ok(())
}

fn apply_matrix(rdram: &[u8], w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    let wire = (w0 & 0xff) as u8;
    if ((w0 >> 8) & 0xff) != 0 || ((w0 >> 19) & 0x1f) != 7 || wire & !7 != 0 {
        return Err(reject("G_MTX wire fields are malformed"));
    }
    let params = wire ^ 1;
    let projection = params & 4 != 0;
    let load = params & 2 != 0;
    let push = params & 1 != 0;
    let address = resolve_addr(&state.segments, w1)?;
    let matrix = read_matrix(rdram, address)?;
    if projection {
        state.projection = Some(if load {
            matrix
        } else {
            state
                .projection
                .map_or(matrix, |current| mat_mul(&matrix, &current))
        });
    } else {
        if push {
            state.modelview_stack.push(state.modelview);
        }
        state.modelview = if load {
            matrix
        } else {
            mat_mul(&matrix, &state.modelview)
        };
    }
    state.recompute_mvp();
    Ok(())
}

fn pop_matrix(w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    if w0 & 0x00ff_ffff != 0 || w1 == 0 || !w1.is_multiple_of(64) {
        return Err(reject("G_POPMTX fields are malformed"));
    }
    let count = (w1 / 64) as usize;
    if count > state.modelview_stack.len() {
        return Err(reject(format!(
            "G_POPMTX requests {count} entries from depth {}",
            state.modelview_stack.len()
        )));
    }
    for _ in 0..count {
        state.modelview = state
            .modelview_stack
            .pop()
            .expect("validated matrix stack depth");
    }
    state.recompute_mvp();
    Ok(())
}

fn project_vertex(state: &WalkState, x: f32, y: f32, z: f32) -> Result<ControlVertex, RenderError> {
    if state.persp_normalize == Some(0) {
        return Ok(ControlVertex {
            clip_w: Some(0.0),
            ..ControlVertex::default()
        });
    }
    let Some(mvp) = state.mvp else {
        // The decoder's raw-fixture path has no explicit matrix commands,
        // which is the identity transform and therefore homogeneous W=1.
        // Loaded state must remain distinct from an untouched cache slot.
        return Ok(ControlVertex {
            clip_w: Some(1.0),
            ..ControlVertex::default()
        });
    };
    let clip = transform_point(&mvp, x, y, z);
    let clip_code = homogeneous_clip_code(clip);
    let divisor = if clip[3].abs() > 1e-6 { clip[3] } else { 1e-6 };
    let ndc_z = clip[2] / divisor;
    let viewport = state.viewport.ok_or_else(|| {
        reject("G_VTX with an active matrix requires a preceding G_MOVEMEM viewport")
    })?;
    let screen_z = ndc_z * viewport.sz + viewport.tz;
    Ok(ControlVertex {
        clip_code,
        z_screen: screen_depth_to_fixed(screen_z),
        clip_w: Some(clip[3]),
    })
}

fn execute_dma_io(
    rdram: &mut [u8],
    rsp_memory: &mut RspMemory,
    segments: &[u32; 16],
    w0: u32,
    w1: u32,
) -> Result<(), RenderError> {
    if w0 & 0x1000 != 0 {
        return Err(reject("G_DMA_IO reserved bit 12 is nonzero"));
    }
    let write_to_dram = w0 & 0x0080_0000 != 0;
    let rsp_address = ((w0 >> 13) & 0x03ff) * 8;
    let bytes = ((w0 & 0x0fff) + 1) as usize;
    if !bytes.is_multiple_of(8) {
        return Err(reject(format!(
            "G_DMA_IO size {bytes} is not a 64-bit multiple"
        )));
    }
    let dram_address = resolve_addr(segments, w1)?;
    if !dram_address.is_multiple_of(8) {
        return Err(reject(format!(
            "G_DMA_IO RDRAM address {dram_address:#x} is unaligned"
        )));
    }
    checked_range(dram_address, bytes, rdram.len(), "G_DMA_IO RDRAM range")?;
    let rsp_address = RspMemAddr::from_register(rsp_address);
    if write_to_dram {
        let data = rsp_memory
            .read_bytes(rsp_address, bytes)
            .map_err(|error| reject(format!("G_DMA_IO RSP read: {error}")))?;
        RdramViewMut::from_storage(rdram)
            .write_logical_bytes(RdramAddr::from_offset(dram_address as u32), &data);
    } else {
        let data = logical_bytes(rdram, dram_address as u32, bytes)?;
        rsp_memory
            .write_bytes(rsp_address, &data)
            .map_err(|error| reject(format!("G_DMA_IO RSP write: {error}")))?;
    }
    Ok(())
}

struct LoadedUcode {
    text_address: u32,
    data_address: u32,
    text_sha256: UcodeDigest,
    data: MicrocodeDataImageIdentity,
}

fn execute_load_ucode(
    rdram: &[u8],
    rsp_memory: &mut RspMemory,
    w0: u32,
    text_address: u32,
    data_address: u32,
) -> Result<LoadedUcode, RenderError> {
    if w0 & 0x00ff_0000 != 0 {
        return Err(reject("G_LOAD_UCODE reserved bits 16..23 are nonzero"));
    }
    let data_bytes = ((w0 & 0xffff) + 1) as usize;
    if data_bytes > SP_UCODE_SIZE || !data_bytes.is_multiple_of(8) {
        return Err(reject(format!(
            "G_LOAD_UCODE data size {data_bytes} is invalid"
        )));
    }
    validate_dma_image(
        text_address,
        SP_UCODE_SIZE,
        rdram.len(),
        "G_LOAD_UCODE text",
    )?;
    validate_dma_image(data_address, data_bytes, rdram.len(), "G_LOAD_UCODE data")?;
    let data = logical_bytes(rdram, data_address, data_bytes)?;
    let text = logical_bytes(rdram, text_address, SP_UCODE_SIZE)?;
    rsp_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Dmem, 0), &data)
        .map_err(|error| reject(format!("G_LOAD_UCODE DMEM write: {error}")))?;
    rsp_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &text)
        .map_err(|error| reject(format!("G_LOAD_UCODE IMEM write: {error}")))?;
    Ok(LoadedUcode {
        text_address,
        data_address,
        text_sha256: UcodeDigest::from_text(&text),
        data: MicrocodeDataImageIdentity {
            bytes: data_bytes as u32,
            sha256: Sha256::digest(data).into(),
        },
    })
}

fn skip_texture_rectangle_continuation(
    rdram: &[u8],
    pc: usize,
    family: GeometryWireFamily,
    opcode: u8,
) -> Result<usize, RenderError> {
    checked_range(pc, 8, rdram.len(), "texture-rectangle continuation")?;
    let first = read_u32(rdram, pc)?;
    let modern = is_modern(family);
    let expected_half_1 = if modern {
        G_RDPHALF_1
    } else {
        LEGACY_G_RDPHALF_1
    };
    let expected_half_2 = if modern {
        G_RDPHALF_2
    } else {
        LEGACY_G_RDPHALF_2
    };
    let actual_half_1 = (first >> 24) as u8;
    if matches!(actual_half_1, G_RDPHALF_1 | LEGACY_G_RDPHALF_1) && actual_half_1 != expected_half_1
    {
        return Err(reject(format!(
            "{} {opcode:#04x} continuation uses wrong-family G_RDPHALF_1 {actual_half_1:#04x}",
            family.name()
        )));
    }
    if actual_half_1 != expected_half_1 {
        return Ok(pc + 8);
    }
    checked_range(
        pc,
        16,
        rdram.len(),
        "wrapped texture-rectangle continuation",
    )?;
    let second = read_u32(rdram, pc + 8)?;
    if second != u32::from(expected_half_2) << 24 {
        return Err(reject(format!(
            "{} {opcode:#04x} continuation lacks family G_RDPHALF_2",
            family.name()
        )));
    }
    Ok(pc + 16)
}

fn capture_raw_window(
    rdram: &[u8],
    text_address: u32,
    data_address: u32,
    size: TaskAdmissionRawWindowSize,
) -> Result<TaskAdmissionRawWindow, RenderError> {
    let text_start = text_address as usize;
    let data_start = data_address as usize;
    checked_range(
        text_start,
        size.text,
        rdram.len(),
        "raw text recognition window",
    )?;
    checked_range(
        data_start,
        size.data,
        rdram.len(),
        "raw data recognition window",
    )?;
    Ok(TaskAdmissionRawWindow {
        text: rdram[text_start..text_start + size.text].to_vec(),
        data: rdram[data_start..data_start + size.data].to_vec(),
    })
}

fn validate_dma_image(
    address: u32,
    bytes: usize,
    rdram_len: usize,
    name: &str,
) -> Result<(), RenderError> {
    if !address.is_multiple_of(8) {
        return Err(reject(format!(
            "{name} address {address:#010x} is not 64-bit aligned"
        )));
    }
    checked_range(address as usize, bytes, rdram_len, name).map(|_| ())
}

fn logical_bytes(rdram: &[u8], address: u32, bytes: usize) -> Result<Vec<u8>, RenderError> {
    checked_range(address as usize, bytes, rdram.len(), "logical RDRAM read")?;
    let mut result = vec![0; bytes];
    RdramView::from_storage(rdram).copy_logical_bytes(RdramAddr::from_offset(address), &mut result);
    Ok(result)
}

fn resolve_addr(segments: &[u32; 16], address: u32) -> Result<usize, RenderError> {
    let segment = ((address >> 24) & 0x0f) as usize;
    let offset = address & 0x00ff_ffff;
    let resolved = segments[segment]
        .checked_add(offset)
        .ok_or_else(|| reject("segmented address overflow"))?;
    Ok(resolved as usize)
}

fn resolve_branch_w_target(
    segments: &[u32; 16],
    address: u32,
    rdram_len: usize,
) -> Result<usize, RenderError> {
    let resolved = resolve_addr(segments, address)? & 0x00ff_fff8;
    checked_range(resolved, 8, rdram_len, "G_BRANCH_W target")?;
    Ok(resolved)
}

fn checked_range(
    start: usize,
    bytes: usize,
    len: usize,
    name: &str,
) -> Result<std::ops::Range<usize>, RenderError> {
    let end = start
        .checked_add(bytes)
        .ok_or_else(|| reject(format!("{name} range overflows")))?;
    if end > len {
        return Err(reject(format!(
            "{name} {start:#010x}..{end:#010x} exceeds RDRAM length {len:#x}"
        )));
    }
    Ok(start..end)
}

fn read_u32(rdram: &[u8], address: usize) -> Result<u32, RenderError> {
    checked_range(address, 4, rdram.len(), "u32 read")?;
    Ok(RdramView::from_storage(rdram).read_u32(RdramAddr::from_offset(address as u32)))
}

fn read_u16(rdram: &[u8], address: usize) -> Result<u16, RenderError> {
    checked_range(address, 2, rdram.len(), "u16 read")?;
    Ok(RdramView::from_storage(rdram).read_u16(RdramAddr::from_offset(address as u32)))
}

fn read_i16(rdram: &[u8], address: usize) -> Result<i16, RenderError> {
    checked_range(address, 2, rdram.len(), "i16 read")?;
    Ok(RdramView::from_storage(rdram).read_i16(RdramAddr::from_offset(address as u32)))
}

fn read_matrix(rdram: &[u8], address: usize) -> Result<Mat4, RenderError> {
    checked_range(address, 64, rdram.len(), "matrix")?;
    let mut matrix = [[0.0; 4]; 4];
    for (row_index, row) in matrix.iter_mut().enumerate() {
        for (column_index, cell) in row.iter_mut().enumerate() {
            let element = row_index * 4 + column_index;
            let integer = i32::from(read_i16(rdram, address + element * 2)?);
            let fraction = i32::from(read_u16(rdram, address + 32 + element * 2)?);
            *cell = (((integer << 16) | fraction) as f32) / 65536.0;
        }
    }
    Ok(matrix)
}

fn read_viewport(rdram: &[u8], address: usize) -> Result<Viewport, RenderError> {
    checked_range(address, 16, rdram.len(), "viewport")?;
    Ok(Viewport {
        sz: f32::from(read_i16(rdram, address + 4)?) / 4.0,
        tz: f32::from(read_i16(rdram, address + 12)?) / 4.0,
    })
}

fn identity() -> Mat4 {
    let mut result = [[0.0; 4]; 4];
    for (index, row) in result.iter_mut().enumerate() {
        row[index] = 1.0;
    }
    result
}

fn mat_mul(left: &Mat4, right: &Mat4) -> Mat4 {
    let mut result = [[0.0; 4]; 4];
    for (row_index, row) in result.iter_mut().enumerate() {
        for (column_index, cell) in row.iter_mut().enumerate() {
            *cell = (0..4)
                .map(|index| left[row_index][index] * right[index][column_index])
                .sum();
        }
    }
    result
}

fn transform_point(matrix: &Mat4, x: f32, y: f32, z: f32) -> [f32; 4] {
    let vertex = [x, y, z, 1.0];
    let mut result = [0.0; 4];
    for (column, value) in result.iter_mut().enumerate() {
        *value = (0..4).map(|row| vertex[row] * matrix[row][column]).sum();
    }
    result
}

fn homogeneous_clip_code([x, y, z, w]: [f32; 4]) -> u8 {
    u8::from(x < -w)
        | (u8::from(x > w) << 1)
        | (u8::from(y < -w) << 2)
        | (u8::from(y > w) << 3)
        | (u8::from(z < -w) << 4)
        | (u8::from(z > w) << 5)
}

fn screen_depth_to_fixed(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= u32::MAX as f32 / 65536.0 {
        u32::MAX
    } else {
        (value * 65536.0) as u32
    }
}

fn reject(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: INSPECTOR,
        reason: reason.into(),
    }
}

fn reject_unsupported(reason: impl Into<String>) -> RenderError {
    let reason = reason.into();
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Render,
        "render.geometry-task-inspection.rejected",
        format!("inspector={INSPECTOR}; reason={reason}"),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    RenderError::Backend {
        backend: INSPECTOR,
        reason,
    }
}

#[cfg(test)]
mod tests;
