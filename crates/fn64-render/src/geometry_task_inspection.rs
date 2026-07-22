//! Transactional control-flow inspection for admitted geometry tasks.
//!
//! This is deliberately not a renderer. It executes only the published GBI
//! state that can change which command is reached: display-list links,
//! segmented pointers, debug DMA, microcode replacement, and the transform /
//! vertex fields consumed by `G_CULLDL` and `G_BRANCH_Z`. RDP drawing state is
//! irrelevant here; the only RDP command retained is FullSync completion.
//!
//! Provenance: command encodings and retained-state rules are from the public
//! `gbi.h`, Fast3D/F3DEX/F3DEX2/L3DEX manuals, SGI RSP Programmer's Guide
//! chapter 4, and SGI RDP Command Summary. The optional forced-branch policy
//! is fn64's typed rendering policy for pinned MIT RT64's public enhancement.

use crate::{
    DpFullSyncStatus, GeometryUcodeCatalog, GeometryWireFamily, MicrocodeDataImageIdentity, OsTask,
    RenderError, TaskAdmissionGeneration, TaskAdmissionPlan, TaskAdmissionSource,
    TaskAdmissionUcode, UcodeDigest,
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
    /// Force every supported depth branch to take its staged target.
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
}

#[derive(Copy, Clone, Debug)]
struct Viewport {
    sz: f32,
    tz: f32,
}

struct WalkState {
    family: GeometryWireFamily,
    segments: [u32; 16],
    vertices: [ControlVertex; 64],
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
    fn new(family: GeometryWireFamily) -> Self {
        Self {
            family,
            segments: [0; 16],
            vertices: [ControlVertex::default(); 64],
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
        loading_family: GeometryWireFamily,
    ) -> Result<(), RenderError> {
        self.vertices = [ControlVertex::default(); 64];
        self.rdp_half_1 = None;
        if loading_family.is_legacy_loadable() {
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
    let family = catalog.require_text(live_text)?;
    if !is_supported_polygon_family(family) {
        return Err(reject_unsupported(format!(
            "{} task inspection is outside the supported public polygon-family frontier",
            family.name()
        )));
    }
    let entry = task_entry(rdram, live_text, task, family)?;
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
    let mut state = WalkState::new(family);
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
    family: GeometryWireFamily,
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
        ucode: TaskAdmissionUcode::from_family(family.ucode_id()),
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
        if consume_line_triangle_noop(state.family, wire_w0, wire_w1)? {
            continue;
        }
        let (w0, w1) = normalize_command(state.family, wire_w0, wire_w1, command_pc)?;
        let opcode = (w0 >> 24) as u8;

        match opcode {
            G_RDPHALF_1 => state.rdp_half_1 = Some(w1),
            G_CULLDL => {
                let start = usize::try_from(w0 & 0xffff).expect("u16 fits usize") / 2;
                let end = usize::try_from(w1 & 0xffff).expect("u16 fits usize") / 2;
                let capacity = state.family.cache_capacity();
                if !(start < end && end < capacity) {
                    return Err(reject(format!(
                        "{} G_CULLDL range {start}..={end} is outside its {capacity}-entry cache",
                        state.family.name()
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
            G_BRANCH_Z => {
                let encoded_vertex = ((w0 >> 12) & 0x0fff) as usize;
                let encoded_z = (w0 & 0x0fff) as usize;
                if !encoded_vertex.is_multiple_of(5) || !encoded_z.is_multiple_of(2) {
                    return Err(reject(format!(
                        "G_BRANCH_Z malformed cache offsets v*5={encoded_vertex:#x}, v*2={encoded_z:#x}"
                    )));
                }
                let slot = encoded_vertex / 5;
                if slot != encoded_z / 2 || slot >= state.family.cache_capacity() {
                    return Err(reject(format!(
                        "{} G_BRANCH_Z selects inconsistent/out-of-range cache slot {slot}",
                        state.family.name()
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
                let loading_family = state.family;
                let loaded = execute_load_ucode(rdram, rsp_memory, w0, w1, data_address)?;
                state.reset_after_ucode_load(loading_family)?;
                let Some(next_family) = catalog.family(loaded.text_sha256) else {
                    return Err(RenderError::RequiresLle {
                        ucode_sha256: loaded.text_sha256.as_bytes(),
                    });
                };
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
                    ucode: TaskAdmissionUcode::from_family(next_family.ucode_id()),
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
                state.family = next_family;
            }
            G_MOVEWORD => apply_moveword(rdram, w0, w1, state)?,
            G_MOVEMEM => apply_movemem(rdram, w0, w1, state)?,
            G_DL => {
                let branch = (w0 >> 16) & 1 != 0;
                let target = resolve_addr(&state.segments, w1)?;
                if branch {
                    pc = target;
                } else {
                    let limit = if is_modern(state.family) {
                        MODERN_DL_DEPTH
                    } else {
                        LEGACY_DL_DEPTH
                    };
                    if state.return_stack.len() >= limit {
                        return Err(reject(format!(
                            "{} G_DL call exceeds its {limit}-entry display-list stack",
                            state.family.name()
                        )));
                    }
                    state.return_stack.push(pc);
                    pc = target;
                }
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                let next = skip_texture_rectangle_continuation(rdram, pc, state.family, opcode)?;
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
                    state.family.name()
                )));
            }
            opcode if is_non_control_command(opcode) => {}
            _ => {
                return Err(reject_unsupported(format!(
                    "unsupported {} command {opcode:#04x} at RDRAM {command_pc:#010x}: w0={wire_w0:#010x} w1={wire_w1:#010x}",
                    state.family.name()
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
    if family.has_unpublished_wire() {
        return Err(reject_unsupported("F3DZEX2 command wire is not admitted"));
    }
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
        _ => {
            return Err(reject_unsupported(format!(
                "unsupported {} G_MOVEWORD index {index:#04x} offset {offset:#06x}",
                state.family.name()
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
                state.family.name()
            )))
        }
    }
    Ok(())
}

fn load_vertices(rdram: &[u8], w0: u32, w1: u32, state: &mut WalkState) -> Result<(), RenderError> {
    let count = ((w0 >> 12) & 0xff) as usize;
    let end = ((w0 >> 1) & 0x7f) as usize;
    if count == 0
        || count > state.family.max_vertex_load_count()
        || end < count
        || end > state.family.cache_capacity()
    {
        return Err(reject(format!(
            "{} G_VTX count/end {count}/{end} is outside its cache",
            state.family.name()
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
    if !encoded.is_multiple_of(2) || encoded / 2 >= state.family.cache_capacity() {
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
        return Ok(ControlVertex::default());
    }
    let Some(mvp) = state.mvp else {
        return Ok(ControlVertex::default());
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
mod tests {
    use super::*;

    const DL: u32 = 0x1000;
    const TEXT: u32 = 0x2000;
    const DATA: u32 = 0x4000;

    fn write_command(rdram: &mut [u8], address: u32, w0: u32, w1: u32) {
        let mut view = RdramViewMut::from_storage(rdram);
        view.write_u32(RdramAddr::from_offset(address), w0);
        view.write_u32(RdramAddr::from_offset(address + 4), w1);
    }

    fn write_logical(rdram: &mut [u8], address: u32, bytes: &[u8]) {
        RdramViewMut::from_storage(rdram)
            .write_logical_bytes(RdramAddr::from_offset(address), bytes);
    }

    fn fixture_for_family(
        family: GeometryWireFamily,
    ) -> (Vec<u8>, RspMemory, OsTask, GeometryUcodeCatalog) {
        let mut rdram = vec![0; 0x9000];
        let text: Vec<u8> = (0..SP_UCODE_SIZE)
            .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
            .collect();
        let data = [0x31, 0x42, 0x53, 0x64, 0x75, 0x86, 0x97, 0xa8];
        write_logical(&mut rdram, TEXT, &text);
        write_logical(&mut rdram, DATA, &data);
        let mut rsp = RspMemory::new();
        rsp.write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &text)
            .unwrap();
        let mut catalog = GeometryUcodeCatalog::default();
        catalog.admit_text_for(family, &text);
        let task = OsTask {
            ucode: TEXT,
            ucode_data: DATA,
            ucode_data_size: data.len() as u32,
            data_ptr: DL,
            ..OsTask::default()
        };
        (rdram, rsp, task, catalog)
    }

    fn fixture() -> (Vec<u8>, RspMemory, OsTask, GeometryUcodeCatalog) {
        fixture_for_family(GeometryWireFamily::F3dex2)
    }

    fn load_word(bytes: usize) -> u32 {
        (u32::from(G_LOAD_UCODE) << 24) | (bytes as u32 - 1)
    }

    fn dma_word(write_to_dram: bool, rsp_address: u16, bytes: usize) -> u32 {
        (u32::from(G_DMA_IO) << 24)
            | (u32::from(write_to_dram) << 23)
            | ((u32::from(rsp_address) / 8) << 13)
            | (bytes as u32 - 1)
    }

    #[test]
    fn every_supported_public_polygon_family_has_an_explicit_wire_test() {
        let families = [
            GeometryWireFamily::Fast3d,
            GeometryWireFamily::F3dex,
            GeometryWireFamily::F3dlx,
            GeometryWireFamily::F3dlxRej,
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
        ];
        for family in families {
            let (mut rdram, rsp, task, catalog) = fixture_for_family(family);
            let end = if is_modern(family) {
                G_ENDDL
            } else {
                LEGACY_G_ENDDL
            };
            write_command(&mut rdram, DL, u32::from(end) << 24, 0);
            let result = inspect_geometry_task(
                &rdram,
                &rsp,
                &task,
                &catalog,
                GeometryTaskInspectionPolicy::default(),
                None,
            )
            .unwrap_or_else(|error| panic!("{} inspection failed: {error}", family.name()));
            assert_eq!(result.admission_plan.entry().family(), family.ucode_id());
        }
    }

    #[test]
    fn line_families_are_named_frontiers() {
        for family in [GeometryWireFamily::L3dex, GeometryWireFamily::L3dex2] {
            let (rdram, rsp, task, catalog) = fixture_for_family(family);
            let error = inspect_geometry_task(
                &rdram,
                &rsp,
                &task,
                &catalog,
                GeometryTaskInspectionPolicy::default(),
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains(family.name()));
            assert!(error.to_string().contains("polygon-family frontier"));
        }
    }

    #[test]
    fn nested_call_tail_and_full_sync_count_follow_executed_path() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_DL) << 24, 0x1100);
        write_command(&mut rdram, DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        write_command(
            &mut rdram,
            0x1100,
            (u32::from(G_DL) << 24) | (1 << 16),
            0x1200,
        );
        write_command(&mut rdram, 0x1108, u32::from(G_SPECIAL_1) << 24, 0);
        write_command(&mut rdram, 0x1200, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, 0x1208, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.full_sync_count, 2);
        assert_eq!(result.dp_full_sync, DpFullSyncStatus::Reached);
    }

    #[test]
    fn forced_depth_branch_changes_both_completion_and_generation_path() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        const TARGET: u32 = 0x1800;
        let other: Vec<u8> = (0..SP_UCODE_SIZE)
            .map(|index| (index as u8).wrapping_mul(19).wrapping_add(3))
            .collect();
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(&mut rdram, OTHER_DATA, &[1, 3, 5, 7, 9, 11, 13, 15]);
        catalog.admit_text(&other);
        write_command(&mut rdram, DL, u32::from(G_RDPHALF_1) << 24, TARGET);
        // Slot zero defaults to screen Z zero; threshold u32::MAX makes the
        // ordinary condition true. Invert the fixture by setting Z to max.
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_MODIFYVTX) << 24) | (u32::from(G_MWO_POINT_ZSCREEN) << 16),
            u32::MAX,
        );
        write_command(&mut rdram, DL + 16, u32::from(G_BRANCH_Z) << 24, 0);
        write_command(&mut rdram, DL + 24, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 32, u32::from(G_ENDDL) << 24, 0);
        write_command(&mut rdram, TARGET, u32::from(G_RDPHALF_1) << 24, OTHER_DATA);
        write_command(&mut rdram, TARGET + 8, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, TARGET + 16, u32::from(G_ENDDL) << 24, 0);

        let normal = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        let forced = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy { force_branch: true },
            None,
        )
        .unwrap();
        assert_eq!(normal.full_sync_count, 1);
        assert_eq!(normal.admission_plan.len(), 1);
        assert_eq!(forced.full_sync_count, 0);
        assert_eq!(forced.admission_plan.len(), 2);
    }

    #[test]
    fn cull_uses_transformed_clip_codes_to_skip_a_self_load() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const MATRIX: u32 = 0x7000;
        const VIEWPORT: u32 = 0x7100;
        const VERTICES: u32 = 0x7200;
        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        let other = vec![0x5d; SP_UCODE_SIZE];
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(&mut rdram, OTHER_DATA, &[2, 4, 6, 8, 10, 12, 14, 16]);
        catalog.admit_text(&other);
        // Identity matrix in split 16.16 Mtx layout.
        for index in 0..4 {
            RdramViewMut::from_storage(&mut rdram)
                .write_u16(RdramAddr::from_offset(MATRIX + (index * 10) as u32), 1);
        }
        // Positive Z viewport scale/translate, only needed to satisfy the
        // transformed-vertex contract.
        RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(VIEWPORT + 4), 4);
        RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(VIEWPORT + 12), 4);
        for index in 0..2 {
            let base = VERTICES + index * VTX_STRIDE as u32;
            RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(base), 2);
        }
        write_command(
            &mut rdram,
            DL,
            (u32::from(G_MOVEMEM) << 24) | (1 << 19) | u32::from(G_MV_VIEWPORT),
            VIEWPORT,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_MTX) << 24) | (7 << 19) | 3,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 16,
            (u32::from(G_VTX) << 24) | (2 << 12) | (2 << 1),
            VERTICES,
        );
        write_command(&mut rdram, DL + 24, u32::from(G_CULLDL) << 24, 2);
        write_command(
            &mut rdram,
            DL + 32,
            u32::from(G_RDPHALF_1) << 24,
            OTHER_DATA,
        );
        write_command(&mut rdram, DL + 40, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, DL + 48, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.admission_plan.len(), 1);
    }

    #[test]
    fn dma_self_modification_preserves_non_palindromic_a_b_a_raw_windows() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const PREFIX_A: u32 = 0x7400;
        const PREFIX_B: u32 = 0x7410;
        let a = logical_bytes(&rdram, TEXT, SP_UCODE_SIZE).unwrap();
        let mut b = a.clone();
        b[..8].copy_from_slice(&[0x02, 0x17, 0x2c, 0x41, 0x56, 0x6b, 0x80, 0x95]);
        write_logical(&mut rdram, PREFIX_A, &a[..8]);
        write_logical(&mut rdram, PREFIX_B, &b[..8]);
        catalog.admit_text(&b);
        let mut pc = DL;
        let emit = |rdram: &mut [u8], pc: &mut u32, w0, w1| {
            write_command(rdram, *pc, w0, w1);
            *pc += 8;
        };
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, dma_word(false, 0, 8), PREFIX_B);
        emit(&mut rdram, &mut pc, dma_word(true, 0, 8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, dma_word(false, 0, 8), PREFIX_A);
        emit(&mut rdram, &mut pc, dma_word(true, 0, 8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            Some(TaskAdmissionRawWindowSize { text: 16, data: 8 }),
        )
        .unwrap();
        assert_eq!(result.admission_plan.len(), 4);
        assert_eq!(result.raw_windows.len(), 4);
        assert_eq!(result.raw_windows[1], result.raw_windows[3]);
        assert_ne!(result.raw_windows[1], result.raw_windows[2]);
        assert_eq!(
            result.admission_plan.self_loads()[0].text_sha256,
            UcodeDigest::from_text(&a)
        );
        assert_eq!(
            result.admission_plan.self_loads()[1].text_sha256,
            UcodeDigest::from_text(&b)
        );
        assert_eq!(
            result.admission_plan.self_loads()[2].text_sha256,
            UcodeDigest::from_text(&a)
        );
        assert_eq!(
            &rdram[TEXT as usize..TEXT as usize + 8],
            &result.raw_windows[0].text[..8]
        );
    }

    #[test]
    fn texture_rectangle_payload_cannot_fabricate_full_sync() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_TEXRECT) << 24, 0);
        write_command(&mut rdram, DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.full_sync_count, 0);
    }

    #[test]
    fn rejection_records_one_stable_detailed_journal_event() {
        let path = std::env::temp_dir().join(format!(
            "fn64-geometry-inspection-unsupported-{}.journal",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        fn64_runtime::arm_unsupported_events(Some(&path)).unwrap();

        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_SPECIAL_1) << 24, 0x1234_5678);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        let reason = error.to_string();
        assert!(reason.contains("reserved F3DEX2 command 0xd5"));
        assert!(reason.contains("RDRAM 0x00001000"));

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.subsystem, fn64_runtime::UnsupportedSubsystem::Render);
        assert_eq!(event.operation, "render.geometry-task-inspection.rejected");
        assert_eq!(
            event.disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(event.guest_cycle, None);
        assert_eq!(
            event.context,
            "inspector=geometry-task-inspection; reason=reserved F3DEX2 command 0xd5 at RDRAM 0x00001000"
        );

        let journal = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = journal.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "fn64.unsupported-journal.v2\tarmed");
        let fields: Vec<_> = lines[1].split('\t').collect();
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[0], "fn64.unsupported-journal.v2");
        assert_eq!(fields[1], "event");
        assert_eq!(fields[3], "unknown");
        assert_eq!(fields[4], "render");
        assert_eq!(fields[5], "returned_error");
        assert_eq!(
            fields[6],
            "72656e6465722e67656f6d657472792d7461736b2d696e7370656374696f6e2e72656a6563746564"
        );
        assert_eq!(
            fields[7],
            "696e73706563746f723d67656f6d657472792d7461736b2d696e7370656374696f6e3b20726561736f6e3d72657365727665642046334445583220636f6d6d616e64203078643520617420524452414d2030783030303031303030"
        );

        fn64_runtime::arm_unsupported_events(None).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_rejection_does_not_poison_unsupported_evidence() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, (u32::from(G_BRANCH_Z) << 24) | 1, 0);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("G_BRANCH_Z malformed cache offsets"));
        assert!(fn64_runtime::copy_unsupported_events().is_empty());
    }

    #[test]
    fn needs_lle_is_not_misreported_as_an_inspection_rejection() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let (rdram, mut rsp, task, catalog) = fixture();
        rsp.write_bytes(
            RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
            &[0xa5; SP_UCODE_SIZE],
        )
        .unwrap();

        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, RenderError::RequiresLle { .. }));
        assert!(fn64_runtime::copy_unsupported_events().is_empty());

        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        let (mut rdram, rsp, task, catalog) = fixture();
        let other = vec![0x5d; SP_UCODE_SIZE];
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(
            &mut rdram,
            OTHER_DATA,
            &[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        );
        write_command(&mut rdram, DL, u32::from(G_RDPHALF_1) << 24, OTHER_DATA);
        write_command(&mut rdram, DL + 8, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        let RenderError::RequiresLle { ucode_sha256 } = error else {
            panic!("unadmitted self-loaded ucode did not request LLE")
        };
        assert_eq!(ucode_sha256, UcodeDigest::from_text(&other).as_bytes());
        assert!(fn64_runtime::copy_unsupported_events().is_empty());
    }

    #[test]
    fn failure_is_transactional_and_named() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_SPECIAL_1) << 24, 0);
        let before_rdram = rdram.clone();
        let before_rsp = rsp.clone();
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(INSPECTOR));
        assert_eq!(rdram, before_rdram);
        assert_eq!(rsp, before_rsp);
    }
}
