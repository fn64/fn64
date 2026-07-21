use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr::NonNull;

use fn64_render::{
    ActiveRenderGraphicsApi, AspectTarget, DownsampleMultiplier, DpFullSyncStatus, OsTask,
    RefreshRateTarget, RenderAntialiasing, RenderAspectRatio, RenderDisplayBuffering,
    RenderEmulatorSettings, RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi,
    RenderHardwareResolve, RenderInternalColorFormat, RenderPresentationMode, RenderRefreshRate,
    RenderReplacementAutoPath, RenderReplacementOperation, RenderReplacementPackIdentity,
    RenderReplacementShift, RenderResolution, RenderRuntimeSettings, RenderUpscale2d,
    ResolutionMultiplier, UcodeId, ViPixelType, ViPresentation, ViScanoutState,
};
use fn64_runtime::{RspMemAddr, RspMemory, RspMemoryBank};
use sha2::{Digest, Sha256};

const ERROR_CAPACITY: usize = 1024;

#[repr(C)]
struct RawContext {
    _private: [u8; 0],
}

#[derive(Copy, Clone, Default)]
#[repr(C)]
struct RawTask {
    task_type: u32,
    flags: u32,
    ucode_boot: u32,
    ucode_boot_size: u32,
    ucode: u32,
    ucode_size: u32,
    ucode_data: u32,
    ucode_data_size: u32,
    dram_stack: u32,
    dram_stack_size: u32,
    output_buff: u32,
    output_buff_size: u32,
    data_ptr: u32,
    data_size: u32,
}

const _: [(); 14 * std::mem::size_of::<u32>()] = [(); std::mem::size_of::<RawTask>()];

impl RawTask {
    fn words(self) -> [u32; 14] {
        [
            self.task_type,
            self.flags,
            self.ucode_boot,
            self.ucode_boot_size,
            self.ucode,
            self.ucode_size,
            self.ucode_data,
            self.ucode_data_size,
            self.dram_stack,
            self.dram_stack_size,
            self.output_buff,
            self.output_buff_size,
            self.data_ptr,
            self.data_size,
        ]
    }
}

impl From<&OsTask> for RawTask {
    fn from(task: &OsTask) -> Self {
        Self {
            task_type: task.task_type,
            flags: task.flags,
            ucode_boot: task.ucode_boot,
            ucode_boot_size: task.ucode_boot_size,
            ucode: task.ucode,
            ucode_size: task.ucode_size,
            ucode_data: task.ucode_data,
            ucode_data_size: task.ucode_data_size,
            dram_stack: task.dram_stack,
            dram_stack_size: task.dram_stack_size,
            output_buff: task.output_buff,
            output_buff_size: task.output_buff_size,
            data_ptr: task.data_ptr,
            data_size: task.data_size,
        }
    }
}

const TASK_RESULT_SCHEMA: u32 = 2;

const UCODE_PLAN_SCHEMA: u32 = 1;
const UCODE_SOURCE_TASK_ENTRY: u32 = 1;
const UCODE_SOURCE_SELF_LOAD: u32 = 2;
const UCODE_DISPOSITION_COMPLETE: u32 = 1;
const UCODE_DISPOSITION_NEEDS_LLE: u32 = 2;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawUcodeGeneration {
    source: u32,
    text_address: u32,
    data_address: u32,
    expected_family: u32,
    data_bytes: u32,
    raw_text_offset: u32,
    raw_text_len: u32,
    raw_data_offset: u32,
    raw_data_len: u32,
    reserved0: u32,
    text_sha256: [u8; 32],
    data_sha256: [u8; 32],
    reserved: [u32; 4],
}

const _: [(); 120] = [(); std::mem::size_of::<RawUcodeGeneration>()];

#[derive(Copy, Clone)]
#[repr(C)]
struct RawUcodePlan {
    schema: u32,
    generation_count: u32,
    generations: *const RawUcodeGeneration,
    raw_pool: *const u8,
    raw_pool_len: u64,
    plan_sha256: [u8; 32],
    reserved: [u32; 4],
}

const _: [(); 80] = [(); std::mem::size_of::<RawUcodePlan>()];

struct PreparedUcodePlan {
    generations: Vec<RawUcodeGeneration>,
    raw_pool: Vec<u8>,
    plan_sha256: [u8; 32],
}

fn raw_ucode_family(family: UcodeId) -> (u32, u32) {
    match family {
        UcodeId::Fast3d => (1, 0),
        UcodeId::F3dex => (2, 0),
        UcodeId::F3dlx => (3, 0),
        UcodeId::F3dlxRej => (4, 0),
        UcodeId::F3dex2 => (5, 0),
        UcodeId::F3dex2NoN => (6, 0),
        UcodeId::F3dex2Rej => (7, 0),
        UcodeId::F3dlx2Rej => (8, 0),
        UcodeId::F3dzex2 => (9, 0),
        UcodeId::S2dex => (10, 0),
        UcodeId::S2dex2 => (11, 0),
        UcodeId::L3dex => (12, 0),
        UcodeId::L3dex2 => (13, 0),
        UcodeId::Other(value) => (0, value),
    }
}

impl PreparedUcodePlan {
    fn new(admission: &crate::Rt64TaskAdmission) -> Result<Self, String> {
        if admission.plan.len() != admission.raw_windows.len() {
            return Err(format!(
                "logical microcode generations ({}) and raw recognition windows ({}) differ",
                admission.plan.len(),
                admission.raw_windows.len()
            ));
        }
        let mut raw_pool = Vec::new();
        let mut generations = Vec::with_capacity(admission.plan.len());
        for (generation, window) in admission
            .plan
            .generations()
            .iter()
            .zip(&admission.raw_windows)
        {
            let (expected_family, reserved0) = raw_ucode_family(generation.family);
            if window.text.len() != crate::RT64_GBI_TEXT_RECOGNITION_BYTES
                || window.data.len() != crate::RT64_GBI_DATA_RECOGNITION_BYTES
            {
                return Err(format!(
                    "microcode recognition window has text/data lengths {:#x}/{:#x}, required {:#x}/{:#x}",
                    window.text.len(),
                    window.data.len(),
                    crate::RT64_GBI_TEXT_RECOGNITION_BYTES,
                    crate::RT64_GBI_DATA_RECOGNITION_BYTES
                ));
            }
            let raw_text_offset = u32::try_from(raw_pool.len())
                .map_err(|_| "microcode raw byte pool exceeds u32 offsets".to_owned())?;
            raw_pool.extend_from_slice(&window.text);
            let raw_data_offset = u32::try_from(raw_pool.len())
                .map_err(|_| "microcode raw byte pool exceeds u32 offsets".to_owned())?;
            raw_pool.extend_from_slice(&window.data);
            generations.push(RawUcodeGeneration {
                source: match generation.source {
                    fn64_render::TaskAdmissionSource::TaskEntry => UCODE_SOURCE_TASK_ENTRY,
                    fn64_render::TaskAdmissionSource::SelfLoad => UCODE_SOURCE_SELF_LOAD,
                },
                text_address: generation.text_address,
                data_address: generation.data_address,
                expected_family,
                data_bytes: generation.data.bytes,
                raw_text_offset,
                raw_text_len: u32::try_from(window.text.len())
                    .expect("pinned RT64 text recognition window fits u32"),
                raw_data_offset,
                raw_data_len: u32::try_from(window.data.len())
                    .expect("pinned RT64 data recognition window fits u32"),
                reserved0,
                text_sha256: generation.text_sha256.as_bytes(),
                data_sha256: generation.data.sha256,
                reserved: [0; 4],
            });
        }
        let raw_pool_len = u64::try_from(raw_pool.len())
            .map_err(|_| "microcode raw byte pool length exceeds u64".to_owned())?;
        let mut hash = Sha256::new();
        hash.update(b"fn64-rt64-ucode-plan-v1");
        hash.update(UCODE_PLAN_SCHEMA.to_le_bytes());
        hash.update(
            u32::try_from(generations.len())
                .map_err(|_| "microcode generation count exceeds u32".to_owned())?
                .to_le_bytes(),
        );
        for generation in &generations {
            for value in [
                generation.source,
                generation.text_address,
                generation.data_address,
                generation.expected_family,
                generation.data_bytes,
                generation.raw_text_offset,
                generation.raw_text_len,
                generation.raw_data_offset,
                generation.raw_data_len,
                generation.reserved0,
            ] {
                hash.update(value.to_le_bytes());
            }
            hash.update(generation.text_sha256);
            hash.update(generation.data_sha256);
            for value in generation.reserved {
                hash.update(value.to_le_bytes());
            }
        }
        hash.update(raw_pool_len.to_le_bytes());
        for value in [0u32; 4] {
            hash.update(value.to_le_bytes());
        }
        hash.update(&raw_pool);
        Ok(Self {
            generations,
            raw_pool,
            plan_sha256: hash.finalize().into(),
        })
    }

    fn raw(&self) -> RawUcodePlan {
        RawUcodePlan {
            schema: UCODE_PLAN_SCHEMA,
            generation_count: u32::try_from(self.generations.len())
                .expect("validated microcode generation count fits u32"),
            generations: self.generations.as_ptr(),
            raw_pool: self.raw_pool.as_ptr(),
            raw_pool_len: u64::try_from(self.raw_pool.len())
                .expect("validated microcode raw pool length fits u64"),
            plan_sha256: self.plan_sha256,
            reserved: [0; 4],
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawTaskResult {
    schema: u32,
    entry_gbi_available: u32,
    workload_id_before: u64,
    workload_id_after: u64,
    initial_ucode_text_address: u32,
    initial_ucode_data_address: u32,
    final_ucode_text_address: u32,
    final_ucode_data_address: u32,
    disposition: u32,
    planned_generation_count: u32,
    observed_generation_count: u32,
    rejected_generation: u32,
    plan_sha256: [u8; 32],
}

const _: [(); 88] = [(); std::mem::size_of::<RawTaskResult>()];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeTaskResult {
    pub(crate) dp_full_sync: DpFullSyncStatus,
    pub(crate) full_sync_count: u64,
    pub(crate) initial_ucode_addresses: (u32, u32),
    pub(crate) final_ucode_addresses: (u32, u32),
    pub(crate) planned_generation_count: u32,
    pub(crate) observed_generation_count: u32,
    pub(crate) plan_sha256: [u8; 32],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NativeTaskOutcome {
    Complete(NativeTaskResult),
    NeedsLle {
        rejected_generation: u32,
        plan_sha256: [u8; 32],
    },
}

fn task_result_from_raw(
    raw: RawTaskResult,
    expected_generation_count: u32,
    expected_plan_sha256: [u8; 32],
) -> Result<NativeTaskOutcome, String> {
    if raw.schema != TASK_RESULT_SCHEMA {
        return Err(format!(
            "RT64 task result schema {} does not match required schema {TASK_RESULT_SCHEMA}",
            raw.schema
        ));
    }
    if raw.planned_generation_count != expected_generation_count {
        return Err(format!(
            "RT64 task result planned {} microcode generations, expected {expected_generation_count}",
            raw.planned_generation_count
        ));
    }
    if raw.plan_sha256 != expected_plan_sha256 {
        return Err("RT64 task result returned a different microcode plan identity".into());
    }
    if raw.observed_generation_count > raw.planned_generation_count {
        return Err(format!(
            "RT64 observed {} microcode generations from a {}-generation plan",
            raw.observed_generation_count, raw.planned_generation_count
        ));
    }
    if raw.disposition == UCODE_DISPOSITION_NEEDS_LLE {
        if raw.entry_gbi_available != 0
            || raw.observed_generation_count != 0
            || raw.rejected_generation >= raw.planned_generation_count
            || raw.workload_id_before != 0
            || raw.workload_id_after != 0
            || raw.initial_ucode_text_address != 0
            || raw.initial_ucode_data_address != 0
            || raw.final_ucode_text_address != 0
            || raw.final_ucode_data_address != 0
        {
            return Err(
                "RT64 needs-LLE task result contains committed native-task evidence".into(),
            );
        }
        return Ok(NativeTaskOutcome::NeedsLle {
            rejected_generation: raw.rejected_generation,
            plan_sha256: raw.plan_sha256,
        });
    }
    if raw.disposition != UCODE_DISPOSITION_COMPLETE
        || raw.entry_gbi_available != 1
        || raw.observed_generation_count != raw.planned_generation_count
        || raw.rejected_generation != u32::MAX
    {
        return Err(format!(
            "RT64 complete task returned disposition/entry/planned/observed/rejected={}/{}/{}/{}/{}",
            raw.disposition,
            raw.entry_gbi_available,
            raw.planned_generation_count,
            raw.observed_generation_count,
            raw.rejected_generation
        ));
    }
    let full_sync_count = raw
        .workload_id_after
        .checked_sub(raw.workload_id_before)
        .ok_or_else(|| {
            format!(
                "RT64 task workload ID moved backwards from {} to {}",
                raw.workload_id_before, raw.workload_id_after
            )
        })?;
    Ok(NativeTaskOutcome::Complete(NativeTaskResult {
        dp_full_sync: if full_sync_count == 0 {
            DpFullSyncStatus::NotReached
        } else {
            DpFullSyncStatus::Reached
        },
        full_sync_count,
        initial_ucode_addresses: (
            raw.initial_ucode_text_address,
            raw.initial_ucode_data_address,
        ),
        final_ucode_addresses: (raw.final_ucode_text_address, raw.final_ucode_data_address),
        planned_generation_count: raw.planned_generation_count,
        observed_generation_count: raw.observed_generation_count,
        plan_sha256: raw.plan_sha256,
    }))
}

#[derive(Copy, Clone, Default)]
#[repr(C)]
struct RawAdapterCapture {
    task: RawTask,
    output_addr: u32,
    width: u32,
    height: u32,
    registers: [u32; 24],
    registers_after_submission: [u32; 24],
}

const _: [(); 65 * std::mem::size_of::<u32>()] = [(); std::mem::size_of::<RawAdapterCapture>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawPresentCapture {
    width: u32,
    height: u32,
    row_bytes: u32,
    format: u32,
    graphics_api: u32,
    reserved: u32,
    byte_len: u64,
    present_id: u64,
    workload_id: u64,
}

const _: [(); 48] = [(); std::mem::size_of::<RawPresentCapture>()];

fn validate_present_capture_metadata(
    metadata: RawPresentCapture,
) -> Result<
    (
        usize,
        crate::Rt64PresentPixelFormat,
        ActiveRenderGraphicsApi,
    ),
    String,
> {
    if metadata.present_id == 0
        || metadata.workload_id == 0
        || metadata.width == 0
        || metadata.height == 0
    {
        return Err("RT64 present capture returned incomplete provenance or dimensions".into());
    }
    let row_bytes = metadata
        .width
        .checked_mul(4)
        .ok_or_else(|| "RT64 present capture row size overflowed".to_string())?;
    let byte_len = u64::from(row_bytes) * u64::from(metadata.height);
    if metadata.row_bytes != row_bytes || metadata.byte_len != byte_len {
        return Err("RT64 present capture byte geometry is inconsistent".into());
    }
    let host_len = usize::try_from(byte_len)
        .map_err(|_| "RT64 present capture exceeds host address space".to_string())?;
    if metadata.reserved != 0 {
        return Err("RT64 present capture returned nonzero reserved metadata".into());
    }
    let format = match metadata.format {
        1 => crate::Rt64PresentPixelFormat::Bgra8Unorm,
        2 => crate::Rt64PresentPixelFormat::Rgba8Unorm,
        value => {
            return Err(format!(
                "RT64 returned unknown present pixel format {value}"
            ))
        }
    };
    let graphics_api = match metadata.graphics_api {
        1 => ActiveRenderGraphicsApi::D3d12,
        2 => ActiveRenderGraphicsApi::Vulkan,
        3 => ActiveRenderGraphicsApi::Metal,
        value => {
            return Err(format!(
                "RT64 returned unknown present graphics API {value}"
            ))
        }
    };
    Ok((host_len, format, graphics_api))
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawPresentSelection {
    present_id: u64,
    source_texture_identity: u64,
    target_address: u32,
    target_width: u32,
    target_height: u32,
    target_size: u32,
}

const _: [(); 32] = [(); std::mem::size_of::<RawPresentSelection>()];

const DEFERRED_MAX_FRAMEBUFFER_PAIRS: usize = 4;
const DEFERRED_MAX_DRAW_CALLS: usize = 16;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawDeferredWorkloadSnapshot {
    workload_id: u64,
    present_id: u64,
    submission_frame: u64,
    content_digest: u64,
    identity_digest: u64,
    framebuffer_pair_count: u32,
    projection_count: u32,
    game_call_count: u32,
    triangle_count: u32,
    vertex_count: u32,
    face_index_count: u32,
    rdp_param_count: u32,
    load_operation_count: u32,
    selected_framebuffer_index: i32,
    selected_draw_call_index: i32,
    selected_framebuffer_address: u32,
    paused: u32,
    pair_color_addresses: [u32; DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pair_game_call_counts: [u32; DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pair_projection_counts: [u32; DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    call_uids: [u32; DEFERRED_MAX_DRAW_CALLS],
    call_fill_colors: [u32; DEFERRED_MAX_DRAW_CALLS],
    call_triangle_counts: [u32; DEFERRED_MAX_DRAW_CALLS],
}

const _: [(); 328] = [(); std::mem::size_of::<RawDeferredWorkloadSnapshot>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawDeferredWorkloadEvidence {
    pre_submission: RawDeferredWorkloadSnapshot,
    current: RawDeferredWorkloadSnapshot,
}

const _: [(); 656] = [(); std::mem::size_of::<RawDeferredWorkloadEvidence>()];

const FRAMEBUFFER_COPY_PATH_GPU: u32 = 1;
const FRAMEBUFFER_COPY_PATH_CPU: u32 = 2;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawFramebufferCopyPathEvidence {
    workload_id: u64,
    source_framebuffer_identity: u64,
    source_framebuffer_address: u32,
    path: u32,
    gpu_create_tile_copy_operation_count: u32,
    gpu_tile_dispatch_count: u32,
    cpu_rdram_tmem_upload_count: u32,
    raw_tmem_tile_count: u32,
    sync_framebuffer_pair_count: u32,
    reserved: u32,
}

const _: [(); 48] = [(); std::mem::size_of::<RawFramebufferCopyPathEvidence>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawS2dexFastPathEvidence {
    workload_id: u64,
    source_framebuffer_identity: u64,
    load_operation_digest: u64,
    source_address: u32,
    source_width: u32,
    source_height: u32,
    source_size: u32,
    gpu_create_tile_copy_operation_count: u32,
    gpu_tile_dispatch_count: u32,
    cpu_rdram_tmem_upload_count: u32,
    raw_tmem_tile_count: u32,
    sync_framebuffer_pair_count: u32,
    framebuffer_pair_count: u32,
    valid_tile_count: u32,
    load_operation_count: u32,
    distinct_source_address_count: u32,
    minimum_source_address: u32,
    maximum_source_address: u32,
    base_source_load_count: u32,
    offset_source_load_count: u32,
    source_is_managed_framebuffer: u32,
    reserved: u32,
}

const _: [(); 104] = [(); std::mem::size_of::<RawS2dexFastPathEvidence>()];

const EXTENDED_COMMAND_COUNT: usize = 0x34;
const EXTENDED_MAX_RECTS: usize = 16;
const EXTENDED_MAX_GROUPS: usize = 16;
const EXTENDED_MAX_VERTEX_Z_MARKERS: usize = 16;
const EXTENDED_MAX_GENERATED_PRESENTS: usize = 8;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawExtendedRectEvidence {
    draw_call_uid: u32,
    left_origin: u16,
    right_origin: u16,
    left_offset: i32,
    top_offset: i32,
    right_offset: i32,
    bottom_offset: i32,
    upper_left_x: i32,
    upper_left_y: i32,
    lower_right_x: i32,
    lower_right_y: i32,
    aspect_mode: u32,
}

const _: [(); 44] = [(); std::mem::size_of::<RawExtendedRectEvidence>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawTransformGroupEvidence {
    group_id: u32,
    projection: u8,
    push: u8,
    decompose: u8,
    editable: u8,
    position_selector: u8,
    rotation_selector: u8,
    scale_selector: u8,
    skew_selector: u8,
    perspective_selector: u8,
    vertex_selector: u8,
    texcoord_selector: u8,
    tile_selector: u8,
    look_at_selector: u8,
    ordering: u8,
    aspect_mode: u8,
    reserved: u8,
}

const _: [(); 20] = [(); std::mem::size_of::<RawTransformGroupEvidence>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawVertexZEvidence {
    marker_kind: u32,
    command_vertex_index: u32,
    resolved_source_index: u32,
    affected_face_index_start: u32,
    affected_face_index_count: u32,
}

const _: [(); 20] = [(); std::mem::size_of::<RawVertexZEvidence>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawGeneratedPresentEvidence {
    previous_workload_id: u64,
    current_workload_id: u64,
    present_id: u64,
    presentation_ordinal: u32,
    interpolation_numerator: u32,
    interpolation_denominator: u32,
    original_refresh_rate: u32,
    target_refresh_rate: u32,
}

const _: [(); 48] = [(); std::mem::size_of::<RawGeneratedPresentEvidence>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawExtendedPresentCapture {
    capture_generation: u64,
    workload_id: u64,
    present_id: u64,
    capture_ordinal: u32,
    capture_count: u32,
    generated_ordinal: u32,
    interpolation_numerator: u32,
    interpolation_denominator: u32,
    width: u32,
    height: u32,
    row_bytes: u32,
    format: u32,
    byte_len: u64,
}

const _: [(); 72] = [(); std::mem::size_of::<RawExtendedPresentCapture>()];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
struct RawExtendedGbiEvidence {
    workload_id: u64,
    present_id: u64,
    enabled_opcode: u8,
    reserved0: [u8; 3],
    hook_enable_count: u32,
    command_counts: [u32; EXTENDED_COMMAND_COUNT],
    has_refresh_rate: u32,
    refresh_rate: u16,
    reserved1: u16,
    rect_count: u32,
    group_count: u32,
    vertex_z_count: u32,
    generated_present_count: u32,
    rects: [RawExtendedRectEvidence; EXTENDED_MAX_RECTS],
    groups: [RawTransformGroupEvidence; EXTENDED_MAX_GROUPS],
    vertex_z: [RawVertexZEvidence; EXTENDED_MAX_VERTEX_Z_MARKERS],
    generated_presents: [RawGeneratedPresentEvidence; EXTENDED_MAX_GENERATED_PRESENTS],
}

impl Default for RawExtendedGbiEvidence {
    fn default() -> Self {
        // SAFETY: every field is an integer or an array of repr(C) integer
        // fields, and zero is the documented empty wire value for each.
        unsafe { std::mem::zeroed() }
    }
}

const _: [(); 1984] = [(); std::mem::size_of::<RawExtendedGbiEvidence>()];

#[cfg(feature = "synthetic-f3dex2-evidence")]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawRegionRateEvidence {
    workload_id: u64,
    configured_nominal_refresh_rate: u32,
    registered_nominal_refresh_rate: u32,
    workload_original_refresh_rate: u32,
    extended_refresh_override_absent: u32,
}

#[cfg(feature = "synthetic-f3dex2-evidence")]
const _: [(); 24] = [(); std::mem::size_of::<RawRegionRateEvidence>()];

#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawHfrEvidence {
    previous_workload_id: u64,
    current_workload_id: u64,
    present_id: u64,
    interpolation_framebuffer_identity: u64,
    interpolation_framebuffer_address: u32,
    original_refresh_rate: u32,
    target_refresh_rate: u32,
    presentation_count: u32,
    available_interpolated_target_count: u32,
    presented_counter_value: u32,
    skipped: u32,
    reserved: u32,
    generated_presents: [RawGeneratedPresentEvidence; EXTENDED_MAX_GENERATED_PRESENTS],
}

#[cfg(feature = "hfr-evidence")]
const _: [(); 448] = [(); std::mem::size_of::<RawHfrEvidence>()];

#[cfg(feature = "hfr-evidence")]
const HFR_MAX_PACING_SAMPLES: usize = 64;

#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawHfrPacingSample {
    call_start_monotonic_ns: u64,
    call_return_monotonic_ns: u64,
    present_id: u64,
    burst_ordinal: u32,
    burst_count: u32,
    original_refresh_rate: u32,
    target_refresh_rate: u32,
    swapchain_valid: u32,
    reserved: u32,
}

#[cfg(feature = "hfr-evidence")]
const _: [(); 48] = [(); std::mem::size_of::<RawHfrPacingSample>()];

#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
struct RawHfrPacingEvidence {
    sample_count: u32,
    reserved: u32,
    samples: [RawHfrPacingSample; HFR_MAX_PACING_SAMPLES],
}

#[cfg(feature = "hfr-evidence")]
impl Default for RawHfrPacingEvidence {
    fn default() -> Self {
        // SAFETY: this wire image contains only integer fields and zero is the
        // documented empty value for the count, reserved field, and samples.
        unsafe { std::mem::zeroed() }
    }
}

#[cfg(feature = "hfr-evidence")]
const _: [(); 3080] = [(); std::mem::size_of::<RawHfrPacingEvidence>()];

const UBERSHADER_MAX_RASTER_CALLS: usize = 16;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawUbershaderEvidence {
    workload_id: u64,
    present_id: u64,
    descriptor_digest: u64,
    pipeline_digest: u64,
    graphics_pipeline_construction_events: u64,
    background_construction_events: u64,
    caller_construction_events: u32,
    workload_construction_events: u32,
    present_construction_events: u32,
    precreated_pipeline_count: u32,
    raster_call_count: u32,
    matched_ubershader_call_count: u32,
    specialized_shader_count: u32,
    ubershaders_only: u32,
    shader_hashes: [u64; UBERSHADER_MAX_RASTER_CALLS],
    pipeline_state_indices: [u32; UBERSHADER_MAX_RASTER_CALLS],
    pipeline_identities: [u64; UBERSHADER_MAX_RASTER_CALLS],
}

const _: [(); 400] = [(); std::mem::size_of::<RawUbershaderEvidence>()];

fn deferred_snapshot(raw: RawDeferredWorkloadSnapshot) -> crate::Rt64DeferredWorkloadSnapshot {
    crate::Rt64DeferredWorkloadSnapshot {
        workload_id: raw.workload_id,
        present_id: raw.present_id,
        submission_frame: raw.submission_frame,
        content_digest: raw.content_digest,
        identity_digest: raw.identity_digest,
        framebuffer_pair_count: raw.framebuffer_pair_count,
        projection_count: raw.projection_count,
        game_call_count: raw.game_call_count,
        triangle_count: raw.triangle_count,
        vertex_count: raw.vertex_count,
        face_index_count: raw.face_index_count,
        rdp_param_count: raw.rdp_param_count,
        load_operation_count: raw.load_operation_count,
        selected_framebuffer_index: raw.selected_framebuffer_index,
        selected_draw_call_index: raw.selected_draw_call_index,
        selected_framebuffer_address: raw.selected_framebuffer_address,
        paused: raw.paused != 0,
        pair_color_addresses: raw.pair_color_addresses,
        pair_game_call_counts: raw.pair_game_call_counts,
        pair_projection_counts: raw.pair_projection_counts,
        call_uids: raw.call_uids,
        call_fill_colors: raw.call_fill_colors,
        call_triangle_counts: raw.call_triangle_counts,
    }
}

fn extended_aspect_mode(value: u32) -> Result<crate::Rt64ExtendedAspectMode, String> {
    match value {
        0 => Ok(crate::Rt64ExtendedAspectMode::Auto),
        1 => Ok(crate::Rt64ExtendedAspectMode::Stretch),
        2 => Ok(crate::Rt64ExtendedAspectMode::Adjust),
        _ => Err(format!(
            "RT64 returned invalid Extended aspect mode {value}"
        )),
    }
}

fn transform_selector(value: u8) -> Result<crate::Rt64TransformComponentSelector, String> {
    match value {
        0 => Ok(crate::Rt64TransformComponentSelector::Skip),
        1 => Ok(crate::Rt64TransformComponentSelector::Interpolate),
        2 => Ok(crate::Rt64TransformComponentSelector::Auto),
        _ => Err(format!(
            "RT64 returned invalid transform-component selector {value}"
        )),
    }
}

fn evidence_count(value: u32, capacity: usize, name: &str) -> Result<usize, String> {
    let count = usize::try_from(value)
        .map_err(|_| format!("RT64 {name} evidence count exceeds host address space"))?;
    if count > capacity {
        Err(format!(
            "RT64 {name} evidence count {count} exceeds capacity {capacity}"
        ))
    } else {
        Ok(count)
    }
}

fn extended_present_capture_from_raw(
    raw: RawExtendedPresentCapture,
    bytes: Vec<u8>,
) -> Result<crate::Rt64ExtendedPresentedPixels, String> {
    if raw.capture_count == 0 || raw.capture_count as usize > EXTENDED_MAX_GENERATED_PRESENTS {
        return Err("RT64 Extended present-capture count is outside its bounded capacity".into());
    }
    if raw.capture_ordinal >= raw.capture_count
        || raw.capture_generation == 0
        || raw.workload_id == 0
        || raw.present_id == 0
        || raw.width == 0
        || raw.height == 0
    {
        return Err("RT64 Extended present-capture metadata is incomplete".into());
    }
    let expected_row_bytes = raw
        .width
        .checked_mul(4)
        .ok_or_else(|| "RT64 Extended present-capture row size overflowed".to_string())?;
    let expected_byte_len = u64::from(expected_row_bytes) * u64::from(raw.height);
    if raw.row_bytes != expected_row_bytes
        || raw.byte_len != expected_byte_len
        || bytes.len() as u64 != expected_byte_len
    {
        return Err("RT64 Extended present-capture byte geometry is inconsistent".into());
    }
    let generated_ordinal = if raw.generated_ordinal == u32::MAX {
        if raw.capture_count != 1
            || raw.capture_ordinal != 0
            || raw.interpolation_numerator != 1
            || raw.interpolation_denominator != 1
        {
            return Err("RT64 ordinary endpoint capture has invalid fraction metadata".into());
        }
        None
    } else {
        if raw.generated_ordinal != raw.capture_ordinal
            || raw.interpolation_numerator != raw.generated_ordinal + 1
            || raw.interpolation_denominator != raw.capture_count
        {
            return Err("RT64 generated capture has invalid ordinal or fraction metadata".into());
        }
        Some(raw.generated_ordinal)
    };
    let format = match raw.format {
        1 => crate::Rt64PresentPixelFormat::Bgra8Unorm,
        2 => crate::Rt64PresentPixelFormat::Rgba8Unorm,
        value => {
            return Err(format!(
                "RT64 returned unknown Extended present pixel format {value}"
            ));
        }
    };
    Ok(crate::Rt64ExtendedPresentedPixels {
        capture_generation: raw.capture_generation,
        workload_id: raw.workload_id,
        present_id: raw.present_id,
        capture_ordinal: raw.capture_ordinal,
        generated_ordinal,
        interpolation_numerator: raw.interpolation_numerator,
        interpolation_denominator: raw.interpolation_denominator,
        width: raw.width,
        height: raw.height,
        row_bytes: raw.row_bytes,
        format,
        bytes,
    })
}

#[cfg(feature = "hfr-evidence")]
fn hfr_present_capture_from_raw(
    raw: RawExtendedPresentCapture,
    bytes: Vec<u8>,
) -> Result<crate::Rt64HfrPresentedPixels, String> {
    let capture = extended_present_capture_from_raw(raw, bytes)?;
    Ok(crate::Rt64HfrPresentedPixels {
        capture_generation: capture.capture_generation,
        workload_id: capture.workload_id,
        present_id: capture.present_id,
        capture_ordinal: capture.capture_ordinal,
        burst_ordinal: capture.generated_ordinal,
        derived_weight_numerator: capture.interpolation_numerator,
        derived_weight_denominator: capture.interpolation_denominator,
        width: capture.width,
        height: capture.height,
        row_bytes: capture.row_bytes,
        format: capture.format,
        bytes: capture.bytes,
    })
}

#[cfg(feature = "hfr-evidence")]
fn hfr_evidence_from_raw(raw: RawHfrEvidence) -> Result<crate::Rt64HfrEvidence, String> {
    if raw.reserved != 0 || raw.skipped != 0 {
        return Err("RT64 HFR evidence is reserved or skipped".into());
    }
    let count = evidence_count(
        raw.presentation_count,
        raw.generated_presents.len(),
        "HFR presentation",
    )?;
    let original_control = raw.target_refresh_rate == 0
        && raw.presentation_count == 1
        && raw.available_interpolated_target_count == 0
        && raw.presented_counter_value == 1;
    let exact_double_rate = raw
        .original_refresh_rate
        .checked_mul(2)
        .is_some_and(|rate| rate == raw.target_refresh_rate)
        && raw.presentation_count == 2
        && raw.available_interpolated_target_count == 1
        && raw.presented_counter_value == 1;
    if count == 0
        || raw.previous_workload_id == 0
        || raw.current_workload_id == 0
        || raw.previous_workload_id == raw.current_workload_id
        || raw.present_id == 0
        || raw.interpolation_framebuffer_identity == 0
        || raw.interpolation_framebuffer_address == 0
        || raw.original_refresh_rate == 0
        || (!original_control && !exact_double_rate)
    {
        return Err("RT64 HFR evidence identity or counters are incomplete".into());
    }
    let presentations = if raw.target_refresh_rate == 0 {
        if count != 1
            || raw.generated_presents
                != [RawGeneratedPresentEvidence::default(); EXTENDED_MAX_GENERATED_PRESENTS]
        {
            return Err("RT64 Original refresh control unexpectedly generated frames".into());
        }
        Vec::new()
    } else {
        if !exact_double_rate {
            return Err(
                "RT64 HFR target rate does not exactly explain the presentation count".into(),
            );
        }
        raw.generated_presents[..count]
            .iter()
            .enumerate()
            .map(|(index, generated)| {
                if generated.previous_workload_id != raw.previous_workload_id
                    || generated.current_workload_id != raw.current_workload_id
                    || generated.present_id != raw.present_id
                    || generated.presentation_ordinal != index as u32
                    || generated.interpolation_numerator != index as u32 + 1
                    || generated.interpolation_denominator != raw.presentation_count
                    || generated.original_refresh_rate != raw.original_refresh_rate
                    || generated.target_refresh_rate != raw.target_refresh_rate
                {
                    return Err(format!(
                        "RT64 HFR generated-presentation provenance differs at index {index}"
                    ));
                }
                Ok(crate::Rt64HfrPresentationEvidence {
                    previous_workload_id: generated.previous_workload_id,
                    current_workload_id: generated.current_workload_id,
                    present_id: generated.present_id,
                    presentation_ordinal: generated.presentation_ordinal,
                    kind: if index == 0 {
                        crate::Rt64HfrPresentationKind::SpatialIntermediate
                    } else {
                        crate::Rt64HfrPresentationKind::CurrentEndpoint
                    },
                    derived_weight_numerator: generated.interpolation_numerator,
                    derived_weight_denominator: generated.interpolation_denominator,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    Ok(crate::Rt64HfrEvidence {
        previous_workload_id: raw.previous_workload_id,
        current_workload_id: raw.current_workload_id,
        present_id: raw.present_id,
        interpolation_framebuffer_identity: raw.interpolation_framebuffer_identity,
        interpolation_framebuffer_address: raw.interpolation_framebuffer_address,
        original_refresh_rate: raw.original_refresh_rate,
        target_refresh_rate: raw.target_refresh_rate,
        presentation_count: raw.presentation_count,
        available_interpolated_target_count: raw.available_interpolated_target_count,
        presented_counter_value: raw.presented_counter_value,
        presentations,
    })
}

#[cfg(feature = "hfr-evidence")]
fn hfr_pacing_from_raw(raw: RawHfrPacingEvidence) -> Result<crate::Rt64HfrPacingEvidence, String> {
    if raw.reserved != 0 {
        return Err("RT64 HFR pacing evidence has nonzero reserved state".into());
    }
    let count = evidence_count(raw.sample_count, raw.samples.len(), "HFR pacing sample")?;
    if count == 0
        || raw.samples[count..]
            .iter()
            .any(|sample| *sample != RawHfrPacingSample::default())
    {
        return Err("RT64 HFR pacing evidence is empty or has a nonempty tail".into());
    }

    let mut previous: Option<RawHfrPacingSample> = None;
    let mut samples = Vec::with_capacity(count);
    for (index, sample) in raw.samples[..count].iter().copied().enumerate() {
        let expected_count = sample
            .target_refresh_rate
            .checked_div(sample.original_refresh_rate.max(1));
        if sample.reserved != 0
            || sample.call_start_monotonic_ns == 0
            || sample.call_return_monotonic_ns <= sample.call_start_monotonic_ns
            || sample.present_id == 0
            || sample.burst_count < 2
            || sample.burst_ordinal >= sample.burst_count
            || sample.original_refresh_rate == 0
            || sample.target_refresh_rate <= sample.original_refresh_rate
            || sample.target_refresh_rate % sample.original_refresh_rate != 0
            || expected_count != Some(sample.burst_count)
            || sample.swapchain_valid != 1
        {
            return Err(format!(
                "RT64 HFR pacing sample {index} is incomplete or inconsistent: {sample:?}"
            ));
        }
        if let Some(prior) = previous {
            if sample.call_start_monotonic_ns <= prior.call_start_monotonic_ns
                || sample.call_return_monotonic_ns <= prior.call_return_monotonic_ns
                || (prior.burst_ordinal + 1 < prior.burst_count
                    && (sample.present_id != prior.present_id
                        || sample.burst_ordinal != prior.burst_ordinal + 1))
                || (prior.burst_ordinal + 1 == prior.burst_count
                    && (sample.present_id <= prior.present_id || sample.burst_ordinal != 0))
            {
                return Err(format!("RT64 HFR pacing order changed at sample {index}"));
            }
        } else if sample.burst_ordinal != 0 {
            return Err("RT64 HFR pacing history starts inside a burst".into());
        }
        previous = Some(sample);
        samples.push(crate::Rt64HfrPacingSample {
            call_start_monotonic_ns: sample.call_start_monotonic_ns,
            call_return_monotonic_ns: sample.call_return_monotonic_ns,
            present_id: sample.present_id,
            burst_ordinal: sample.burst_ordinal,
            burst_count: sample.burst_count,
            original_refresh_rate: sample.original_refresh_rate,
            target_refresh_rate: sample.target_refresh_rate,
        });
    }
    if previous.is_some_and(|sample| sample.burst_ordinal + 1 != sample.burst_count) {
        return Err("RT64 HFR pacing history ends inside a burst".into());
    }
    Ok(crate::Rt64HfrPacingEvidence { samples })
}

fn extended_evidence_from_raw(
    raw: RawExtendedGbiEvidence,
) -> Result<crate::Rt64ExtendedGbiEvidence, String> {
    if raw.reserved0 != [0; 3] || raw.reserved1 != 0 {
        return Err("RT64 returned nonzero reserved Extended-evidence fields".into());
    }
    let enabled_opcode = if raw.hook_enable_count == 0 {
        if raw.enabled_opcode != 0 {
            return Err("RT64 returned an enabled opcode without an enable hook".into());
        }
        None
    } else if raw.enabled_opcode == 0 {
        return Err("RT64 returned the forbidden zero Extended opcode".into());
    } else {
        Some(raw.enabled_opcode)
    };
    let refresh_rate = match raw.has_refresh_rate {
        0 => {
            if raw.refresh_rate != 0 {
                return Err("RT64 returned a refresh rate without a refresh command".into());
            }
            None
        }
        1 if raw.refresh_rate != 0 => Some(raw.refresh_rate),
        1 => return Err("RT64 returned a zero Extended refresh rate".into()),
        value => {
            return Err(format!(
                "RT64 returned invalid has-refresh-rate boolean {value}"
            ));
        }
    };
    let rect_count = evidence_count(raw.rect_count, raw.rects.len(), "rectangle")?;
    let group_count = evidence_count(raw.group_count, raw.groups.len(), "transform-group")?;
    let vertex_count = evidence_count(raw.vertex_z_count, raw.vertex_z.len(), "vertex-Z")?;
    let generated_count = evidence_count(
        raw.generated_present_count,
        raw.generated_presents.len(),
        "generated-presentation",
    )?;

    let rects = raw.rects[..rect_count]
        .iter()
        .map(|rect| {
            Ok(crate::Rt64ExtendedRectEvidence {
                draw_call_uid: rect.draw_call_uid,
                left_origin: rect.left_origin,
                right_origin: rect.right_origin,
                left_offset: rect.left_offset,
                top_offset: rect.top_offset,
                right_offset: rect.right_offset,
                bottom_offset: rect.bottom_offset,
                upper_left_x: rect.upper_left_x,
                upper_left_y: rect.upper_left_y,
                lower_right_x: rect.lower_right_x,
                lower_right_y: rect.lower_right_y,
                aspect_mode: extended_aspect_mode(rect.aspect_mode)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let groups = raw.groups[..group_count]
        .iter()
        .map(|group| {
            if group.reserved != 0 {
                return Err("RT64 returned a nonzero transform-group reserved byte".into());
            }
            Ok(crate::Rt64TransformGroupEvidence {
                group_id: group.group_id,
                class: match group.projection {
                    0 => crate::Rt64TransformClass::Model,
                    1 => crate::Rt64TransformClass::Projection,
                    value => {
                        return Err(format!(
                            "RT64 returned invalid transform projection boolean {value}"
                        ));
                    }
                },
                push: match group.push {
                    0 => false,
                    1 => true,
                    value => {
                        return Err(format!(
                            "RT64 returned invalid transform push boolean {value}"
                        ));
                    }
                },
                decompose: match group.decompose {
                    0 => false,
                    1 => true,
                    value => {
                        return Err(format!(
                            "RT64 returned invalid transform decompose boolean {value}"
                        ));
                    }
                },
                editable: match group.editable {
                    0 => false,
                    1 => true,
                    value => {
                        return Err(format!(
                            "RT64 returned invalid transform editable boolean {value}"
                        ));
                    }
                },
                position: transform_selector(group.position_selector)?,
                rotation: transform_selector(group.rotation_selector)?,
                scale: transform_selector(group.scale_selector)?,
                skew: transform_selector(group.skew_selector)?,
                perspective: transform_selector(group.perspective_selector)?,
                vertex: transform_selector(group.vertex_selector)?,
                texcoord: transform_selector(group.texcoord_selector)?,
                tile: transform_selector(group.tile_selector)?,
                look_at: transform_selector(group.look_at_selector)?,
                ordering: match group.ordering {
                    0 => crate::Rt64TransformOrdering::Linear,
                    1 => crate::Rt64TransformOrdering::Auto,
                    value => {
                        return Err(format!("RT64 returned invalid transform ordering {value}"));
                    }
                },
                aspect_mode: extended_aspect_mode(u32::from(group.aspect_mode))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let vertex_z = raw.vertex_z[..vertex_count]
        .iter()
        .map(|marker| {
            let marker_kind = match marker.marker_kind {
                1 => crate::Rt64VertexZMarkerKind::Begin,
                2 => crate::Rt64VertexZMarkerKind::End,
                value => return Err(format!("RT64 returned invalid vertex-Z marker {value}")),
            };
            let command_vertex_index = if marker.command_vertex_index == u32::MAX {
                if marker_kind != crate::Rt64VertexZMarkerKind::End {
                    return Err("RT64 omitted a begin vertex-Z command index".into());
                }
                None
            } else {
                if marker_kind != crate::Rt64VertexZMarkerKind::Begin {
                    return Err("RT64 returned a command index for a vertex-Z end marker".into());
                }
                Some(marker.command_vertex_index)
            };
            Ok(crate::Rt64VertexZEvidence {
                marker_kind,
                command_vertex_index,
                resolved_source_index: marker.resolved_source_index,
                affected_face_index_start: marker.affected_face_index_start,
                affected_face_index_count: marker.affected_face_index_count,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let generated_presents = raw.generated_presents[..generated_count]
        .iter()
        .enumerate()
        .map(|(index, generated)| {
            if generated.previous_workload_id == 0
                || generated.current_workload_id != raw.workload_id
                || generated.present_id != raw.present_id
                || generated.presentation_ordinal != index as u32
                || generated.interpolation_denominator == 0
                || generated.interpolation_numerator != index as u32 + 1
                || generated.interpolation_numerator > generated.interpolation_denominator
                || generated.original_refresh_rate == 0
                || generated.target_refresh_rate <= generated.original_refresh_rate
            {
                return Err(format!(
                    "RT64 returned inconsistent generated-presentation evidence at index {index}"
                ));
            }
            Ok(crate::Rt64GeneratedPresentEvidence {
                previous_workload_id: generated.previous_workload_id,
                current_workload_id: generated.current_workload_id,
                present_id: generated.present_id,
                presentation_ordinal: generated.presentation_ordinal,
                interpolation_numerator: generated.interpolation_numerator,
                interpolation_denominator: generated.interpolation_denominator,
                original_refresh_rate: generated.original_refresh_rate,
                target_refresh_rate: generated.target_refresh_rate,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(crate::Rt64ExtendedGbiEvidence {
        workload_id: raw.workload_id,
        present_id: raw.present_id,
        enabled_opcode,
        hook_enable_count: raw.hook_enable_count,
        command_counts: raw.command_counts,
        refresh_rate,
        rects,
        groups,
        vertex_z,
        generated_presents,
    })
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawReplacementDatabaseConfig {
    auto_path: u32,
    default_operation: u32,
    default_shift: u32,
    configuration_version: u32,
    hash_version: u32,
}

const _: [(); 20] = [(); std::mem::size_of::<RawReplacementDatabaseConfig>()];

#[repr(C)]
struct RawReplacementPack {
    path_utf8: *const c_char,
    expected_database: RawReplacementDatabaseConfig,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawTextureReplacementState {
    texture_hash: u64,
    stream_load_count: u64,
    texture_count: u32,
    texture_known: u32,
    replacement_resolved: u32,
    replacement_installed: u32,
    replacement_mip_levels: u32,
    replacements_enabled: u32,
    stream_queued: u32,
    stream_active: u32,
    stream_results_pending: u32,
    uploads_pending: u32,
    resolved_paths_pending: u32,
    observed_resolved_not_installed: u32,
    stream_workers_paused: u32,
    stream_worker_count: u32,
}

const _: [(); 72] = [(); std::mem::size_of::<RawTextureReplacementState>()];

impl From<&RenderReplacementPackIdentity> for RawReplacementDatabaseConfig {
    fn from(identity: &RenderReplacementPackIdentity) -> Self {
        Self {
            auto_path: match identity.auto_path {
                RenderReplacementAutoPath::Rt64 => 0,
                RenderReplacementAutoPath::Rice => 1,
            },
            default_operation: match identity.default_operation {
                RenderReplacementOperation::Preload => 0,
                RenderReplacementOperation::Stream => 1,
                RenderReplacementOperation::Stall => 2,
            },
            default_shift: match identity.default_shift {
                RenderReplacementShift::None => 0,
                RenderReplacementShift::Half => 1,
            },
            configuration_version: identity.configuration_version,
            hash_version: identity.hash_version,
        }
    }
}

fn replacement_identity_from_raw(
    raw: RawReplacementDatabaseConfig,
) -> Result<RenderReplacementPackIdentity, String> {
    Ok(RenderReplacementPackIdentity {
        content_sha256: [0; 32],
        database_sha256: [0; 32],
        auto_path: match raw.auto_path {
            0 => RenderReplacementAutoPath::Rt64,
            1 => RenderReplacementAutoPath::Rice,
            value => {
                return Err(format!(
                    "RT64 returned invalid replacement auto-path tag {value}"
                ));
            }
        },
        default_operation: match raw.default_operation {
            0 => RenderReplacementOperation::Preload,
            1 => RenderReplacementOperation::Stream,
            2 => RenderReplacementOperation::Stall,
            value => {
                return Err(format!(
                    "RT64 returned invalid replacement operation tag {value}"
                ));
            }
        },
        default_shift: match raw.default_shift {
            0 => RenderReplacementShift::None,
            1 => RenderReplacementShift::Half,
            value => {
                return Err(format!(
                    "RT64 returned invalid replacement shift tag {value}"
                ));
            }
        },
        configuration_version: raw.configuration_version,
        hash_version: raw.hash_version,
    })
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
struct RawUserConfig {
    graphics_api: u32,
    resolution: u32,
    display_buffering: u32,
    antialiasing: u32,
    resolution_multiplier: f64,
    downsample_multiplier: u32,
    filtering: u32,
    aspect_ratio: u32,
    aspect_target: f64,
    extended_aspect_ratio: u32,
    extended_aspect_target: f64,
    upscale_2d: u32,
    three_point_filtering: u32,
    refresh_rate: u32,
    refresh_rate_target: u32,
    internal_color_format: u32,
    hardware_resolve: u32,
    idle_work_active: u32,
    developer_mode: u32,
}

const _: [(); 96] = [(); std::mem::size_of::<RawUserConfig>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawEnhancementConfig {
    framebuffer_reinterpret_fix_uls: u32,
    presentation_mode: u32,
    remove_black_borders: u32,
    rect_fix_lower_right: u32,
    f3dex_force_branch: u32,
    s2dex_fix_bilerp_mismatch: u32,
    s2dex_framebuffer_fast_path: u32,
    texture_lod_scale: u32,
}

const _: [(); 32] = [(); std::mem::size_of::<RawEnhancementConfig>()];

impl From<&RenderEnhancementSettings> for RawEnhancementConfig {
    fn from(settings: &RenderEnhancementSettings) -> Self {
        Self {
            framebuffer_reinterpret_fix_uls: u32::from(settings.framebuffer_reinterpret_fix_uls),
            presentation_mode: match settings.presentation_mode {
                RenderPresentationMode::Console => 0,
                RenderPresentationMode::SkipBuffering => 1,
                RenderPresentationMode::PresentEarly => 2,
            },
            remove_black_borders: u32::from(settings.remove_black_borders),
            rect_fix_lower_right: u32::from(settings.rect_fix_lower_right),
            f3dex_force_branch: u32::from(settings.f3dex_force_branch),
            s2dex_fix_bilerp_mismatch: u32::from(settings.s2dex_fix_bilerp_mismatch),
            s2dex_framebuffer_fast_path: u32::from(settings.s2dex_framebuffer_fast_path),
            texture_lod_scale: u32::from(settings.texture_lod_scale),
        }
    }
}

impl TryFrom<RawEnhancementConfig> for RenderEnhancementSettings {
    type Error = String;

    fn try_from(raw: RawEnhancementConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            framebuffer_reinterpret_fix_uls: decode_raw_bool(
                raw.framebuffer_reinterpret_fix_uls,
                "framebuffer_reinterpret_fix_uls",
            )?,
            presentation_mode: match raw.presentation_mode {
                0 => RenderPresentationMode::Console,
                1 => RenderPresentationMode::SkipBuffering,
                2 => RenderPresentationMode::PresentEarly,
                value => {
                    return Err(format!(
                        "C++ returned invalid presentation_mode tag {value}"
                    ));
                }
            },
            remove_black_borders: decode_raw_bool(
                raw.remove_black_borders,
                "remove_black_borders",
            )?,
            rect_fix_lower_right: decode_raw_bool(
                raw.rect_fix_lower_right,
                "rect_fix_lower_right",
            )?,
            f3dex_force_branch: decode_raw_bool(raw.f3dex_force_branch, "f3dex_force_branch")?,
            s2dex_fix_bilerp_mismatch: decode_raw_bool(
                raw.s2dex_fix_bilerp_mismatch,
                "s2dex_fix_bilerp_mismatch",
            )?,
            s2dex_framebuffer_fast_path: decode_raw_bool(
                raw.s2dex_framebuffer_fast_path,
                "s2dex_framebuffer_fast_path",
            )?,
            texture_lod_scale: decode_raw_bool(raw.texture_lod_scale, "texture_lod_scale")?,
        })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct RawEmulatorConfig {
    post_blend_noise: u32,
    post_blend_noise_negative: u32,
    framebuffer_render_to_ram: u32,
    framebuffer_copy_with_gpu: u32,
}

const _: [(); 16] = [(); std::mem::size_of::<RawEmulatorConfig>()];

impl From<&RenderEmulatorSettings> for RawEmulatorConfig {
    fn from(settings: &RenderEmulatorSettings) -> Self {
        Self {
            post_blend_noise: u32::from(settings.post_blend_noise),
            post_blend_noise_negative: u32::from(settings.post_blend_noise_negative),
            framebuffer_render_to_ram: u32::from(settings.framebuffer_render_to_ram),
            framebuffer_copy_with_gpu: u32::from(settings.framebuffer_copy_with_gpu),
        }
    }
}

impl TryFrom<RawEmulatorConfig> for RenderEmulatorSettings {
    type Error = String;

    fn try_from(raw: RawEmulatorConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            post_blend_noise: decode_raw_bool(raw.post_blend_noise, "post_blend_noise")?,
            post_blend_noise_negative: decode_raw_bool(
                raw.post_blend_noise_negative,
                "post_blend_noise_negative",
            )?,
            framebuffer_render_to_ram: decode_raw_bool(
                raw.framebuffer_render_to_ram,
                "framebuffer_render_to_ram",
            )?,
            framebuffer_copy_with_gpu: decode_raw_bool(
                raw.framebuffer_copy_with_gpu,
                "framebuffer_copy_with_gpu",
            )?,
        })
    }
}

fn decode_raw_bool(value: u32, field: &str) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!("C++ returned invalid {field} boolean {value}")),
    }
}

impl From<&RenderRuntimeSettings> for RawUserConfig {
    fn from(settings: &RenderRuntimeSettings) -> Self {
        Self {
            graphics_api: match settings.graphics_api {
                RenderGraphicsApi::D3d12 => 0,
                RenderGraphicsApi::Vulkan => 1,
                RenderGraphicsApi::Metal => 2,
                RenderGraphicsApi::Automatic => 3,
            },
            resolution: match settings.resolution {
                RenderResolution::Original => 0,
                RenderResolution::WindowIntegerScale => 1,
                RenderResolution::Manual => 2,
            },
            display_buffering: match settings.display_buffering {
                RenderDisplayBuffering::Double => 0,
                RenderDisplayBuffering::Triple => 1,
            },
            antialiasing: match settings.antialiasing {
                RenderAntialiasing::None => 0,
                RenderAntialiasing::Msaa2x => 1,
                RenderAntialiasing::Msaa4x => 2,
                RenderAntialiasing::Msaa8x => 3,
            },
            resolution_multiplier: settings.resolution_multiplier.get(),
            downsample_multiplier: u32::from(settings.downsample_multiplier.get()),
            filtering: match settings.filtering {
                RenderFiltering::Nearest => 0,
                RenderFiltering::Linear => 1,
                RenderFiltering::AntiAliasedPixelScaling => 2,
            },
            aspect_ratio: aspect_tag(settings.aspect_ratio),
            aspect_target: settings.aspect_target.get(),
            extended_aspect_ratio: aspect_tag(settings.extended_aspect_ratio),
            extended_aspect_target: settings.extended_aspect_target.get(),
            upscale_2d: match settings.upscale_2d {
                RenderUpscale2d::Original => 0,
                RenderUpscale2d::ScaledOnly => 1,
                RenderUpscale2d::All => 2,
            },
            three_point_filtering: u32::from(settings.three_point_filtering),
            refresh_rate: match settings.refresh_rate {
                RenderRefreshRate::Original => 0,
                RenderRefreshRate::Display => 1,
                RenderRefreshRate::Manual => 2,
            },
            refresh_rate_target: u32::from(settings.refresh_rate_target.get()),
            internal_color_format: match settings.internal_color_format {
                RenderInternalColorFormat::Standard => 0,
                RenderInternalColorFormat::High => 1,
                RenderInternalColorFormat::Automatic => 2,
            },
            hardware_resolve: match settings.hardware_resolve {
                RenderHardwareResolve::Disabled => 0,
                RenderHardwareResolve::Enabled => 1,
                RenderHardwareResolve::Automatic => 2,
            },
            idle_work_active: u32::from(settings.idle_work_active),
            developer_mode: u32::from(settings.developer_mode),
        }
    }
}

fn aspect_tag(value: RenderAspectRatio) -> u32 {
    match value {
        RenderAspectRatio::Original => 0,
        RenderAspectRatio::Expand => 1,
        RenderAspectRatio::Manual => 2,
    }
}

fn decode_aspect(value: u32, field: &str) -> Result<RenderAspectRatio, String> {
    match value {
        0 => Ok(RenderAspectRatio::Original),
        1 => Ok(RenderAspectRatio::Expand),
        2 => Ok(RenderAspectRatio::Manual),
        _ => Err(format!("C++ returned invalid {field} tag {value}")),
    }
}

impl TryFrom<RawUserConfig> for RenderRuntimeSettings {
    type Error = String;

    fn try_from(raw: RawUserConfig) -> Result<Self, Self::Error> {
        let boolean = |value, field| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("C++ returned invalid {field} boolean {value}")),
        };
        Ok(Self {
            graphics_api: match raw.graphics_api {
                0 => RenderGraphicsApi::D3d12,
                1 => RenderGraphicsApi::Vulkan,
                2 => RenderGraphicsApi::Metal,
                3 => RenderGraphicsApi::Automatic,
                value => return Err(format!("C++ returned invalid graphics_api tag {value}")),
            },
            resolution: match raw.resolution {
                0 => RenderResolution::Original,
                1 => RenderResolution::WindowIntegerScale,
                2 => RenderResolution::Manual,
                value => return Err(format!("C++ returned invalid resolution tag {value}")),
            },
            display_buffering: match raw.display_buffering {
                0 => RenderDisplayBuffering::Double,
                1 => RenderDisplayBuffering::Triple,
                value => {
                    return Err(format!(
                        "C++ returned invalid display_buffering tag {value}"
                    ));
                }
            },
            antialiasing: match raw.antialiasing {
                0 => RenderAntialiasing::None,
                1 => RenderAntialiasing::Msaa2x,
                2 => RenderAntialiasing::Msaa4x,
                3 => RenderAntialiasing::Msaa8x,
                value => return Err(format!("C++ returned invalid antialiasing tag {value}")),
            },
            resolution_multiplier: ResolutionMultiplier::new(raw.resolution_multiplier)
                .map_err(|error| error.to_string())?,
            downsample_multiplier: DownsampleMultiplier::new(raw.downsample_multiplier)
                .map_err(|error| error.to_string())?,
            filtering: match raw.filtering {
                0 => RenderFiltering::Nearest,
                1 => RenderFiltering::Linear,
                2 => RenderFiltering::AntiAliasedPixelScaling,
                value => return Err(format!("C++ returned invalid filtering tag {value}")),
            },
            aspect_ratio: decode_aspect(raw.aspect_ratio, "aspect_ratio")?,
            aspect_target: AspectTarget::new(raw.aspect_target)
                .map_err(|error| error.to_string())?,
            extended_aspect_ratio: decode_aspect(
                raw.extended_aspect_ratio,
                "extended_aspect_ratio",
            )?,
            extended_aspect_target: AspectTarget::new(raw.extended_aspect_target)
                .map_err(|error| error.to_string())?,
            upscale_2d: match raw.upscale_2d {
                0 => RenderUpscale2d::Original,
                1 => RenderUpscale2d::ScaledOnly,
                2 => RenderUpscale2d::All,
                value => return Err(format!("C++ returned invalid upscale_2d tag {value}")),
            },
            three_point_filtering: boolean(raw.three_point_filtering, "three_point_filtering")?,
            refresh_rate: match raw.refresh_rate {
                0 => RenderRefreshRate::Original,
                1 => RenderRefreshRate::Display,
                2 => RenderRefreshRate::Manual,
                value => return Err(format!("C++ returned invalid refresh_rate tag {value}")),
            },
            refresh_rate_target: RefreshRateTarget::new(raw.refresh_rate_target)
                .map_err(|error| error.to_string())?,
            internal_color_format: match raw.internal_color_format {
                0 => RenderInternalColorFormat::Standard,
                1 => RenderInternalColorFormat::High,
                2 => RenderInternalColorFormat::Automatic,
                value => {
                    return Err(format!(
                        "C++ returned invalid internal_color_format tag {value}"
                    ));
                }
            },
            hardware_resolve: match raw.hardware_resolve {
                0 => RenderHardwareResolve::Disabled,
                1 => RenderHardwareResolve::Enabled,
                2 => RenderHardwareResolve::Automatic,
                value => return Err(format!("C++ returned invalid hardware_resolve tag {value}")),
            },
            idle_work_active: boolean(raw.idle_work_active, "idle_work_active")?,
            developer_mode: boolean(raw.developer_mode, "developer_mode")?,
        })
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
struct RawVi {
    registers: [u32; 14],
    registers_present: u8,
    blanked: u8,
    fade_enabled: u8,
    repeat_line: u8,
    fade_factor: u16,
    reserved: u16,
    noise_seed: u64,
}

const _: [(); 72] = [(); std::mem::size_of::<RawVi>()];

fn raw_vi(vi: ViPresentation) -> Result<RawVi, String> {
    let filters = vi.scanout.filters();
    let pixel_type = match filters.pixel_type {
        ViPixelType::Unspecified | ViPixelType::Rgba16 => 2u32,
        ViPixelType::Rgba32 => 3u32,
        ViPixelType::Blank => 0u32,
        ViPixelType::Reserved => return Err("VI STATUS selects reserved pixel type 1".into()),
    };
    let (registers_present, mut registers) = match vi.scanout {
        ViScanoutState::BackendOnly(_) => (0, [0; 14]),
        ViScanoutState::Registers(registers) => (1, registers.words()),
    };
    if registers_present == 0 {
        registers[0] = pixel_type
            | filters.antialias_mode.status_bits().unwrap_or(0)
            | (u32::from(filters.gamma_dither) << 2)
            | (u32::from(filters.gamma) << 3)
            | (u32::from(filters.divot) << 4)
            | (u32::from(filters.dither_filter) << 16);
    }
    Ok(RawVi {
        registers,
        registers_present,
        blanked: u8::from(vi.blanked),
        fade_enabled: u8::from(vi.fade.is_some()),
        repeat_line: u8::from(vi.repeat_line),
        fade_factor: vi.fade.unwrap_or(0),
        reserved: 0,
        noise_seed: vi.noise_seed,
    })
}

unsafe extern "C" {
    fn fn64_rt64_roundtrip_user_config(
        input: *const RawUserConfig,
        output: *mut RawUserConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_roundtrip_enhancement_config(
        input: *const RawEnhancementConfig,
        output: *mut RawEnhancementConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_roundtrip_emulator_config(
        input: *const RawEmulatorConfig,
        output: *mut RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_inspect_replacement_pack(
        path_utf8: *const c_char,
        config: *mut RawReplacementDatabaseConfig,
        database_bytes: *mut u8,
        database_capacity: usize,
        database_size: *mut usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_capture_adapter_inputs(
        task: *const RawTask,
        output_addr: u32,
        width: u32,
        height: u32,
        vi: *const RawVi,
        capture: *mut RawAdapterCapture,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(test)]
    fn fn64_rt64_probe_logical_rate(
        nominal_refresh_rate: u32,
        factor: u32,
        logical_rate: *mut u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_create(
        width: u32,
        height: u32,
        nominal_refresh_rate: u32,
        user_config: *const RawUserConfig,
        enhancement_config: *const RawEnhancementConfig,
        emulator_config: *const RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut RawContext;
    fn fn64_rt64_apply_user_config(
        context: *mut RawContext,
        user_config: *const RawUserConfig,
        framebuffers_discarded: *mut u8,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_apply_enhancement_config(
        context: *mut RawContext,
        enhancement_config: *const RawEnhancementConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_apply_emulator_config(
        context: *mut RawContext,
        emulator_config: *const RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_load_replacement_packs(
        context: *mut RawContext,
        packs: *const RawReplacementPack,
        pack_count: usize,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_reload_replacement_packs(
        context: *mut RawContext,
        packs: *const RawReplacementPack,
        pack_count: usize,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_replacement_enabled(
        context: *mut RawContext,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_wait_texture_replacement_state(
        context: *mut RawContext,
        texture_hash: u64,
        require_replacement: u32,
        state: *mut RawTextureReplacementState,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_stream_workers_paused(
        context: *mut RawContext,
        paused: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_wait_stream_fallback_state(
        context: *mut RawContext,
        texture_hash: u64,
        state: *mut RawTextureReplacementState,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_process_task(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        dmem: *mut u8,
        dmem_len: usize,
        imem: *mut u8,
        imem_len: usize,
        task: *const RawTask,
        output_addr: u32,
        ucode_plan: *const RawUcodePlan,
        result: *mut RawTaskResult,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_process_rdp_commands(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        start: u32,
        end: u32,
        output_addr: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_present(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        vi: *const RawVi,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_present_capture(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_present_capture(
        context: *mut RawContext,
        capture: *mut RawPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_present_selection(
        context: *mut RawContext,
        selection: *mut RawPresentSelection,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_deferred_workload_capture(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_deferred_workload_evidence(
        context: *mut RawContext,
        evidence: *mut RawDeferredWorkloadEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_framebuffer_copy_path_evidence(
        context: *mut RawContext,
        evidence: *mut RawFramebufferCopyPathEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_s2dex_fast_path_evidence(
        context: *mut RawContext,
        evidence: *mut RawS2dexFastPathEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_extended_gbi_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_extended_gbi_evidence(
        context: *mut RawContext,
        evidence: *mut RawExtendedGbiEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_extended_present_capture(
        context: *mut RawContext,
        capture_index: u32,
        capture: *mut RawExtendedPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_enable_hfr_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "synthetic-f3dex2-evidence")]
    fn fn64_rt64_process_synthetic_hfr_f3dex2(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
        region_rate_evidence: *mut RawRegionRateEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "synthetic-s2dex-evidence")]
    fn fn64_rt64_process_synthetic_s2dex2(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        display_list: u32,
        output_addr: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_evidence(
        context: *mut RawContext,
        evidence: *mut RawHfrEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_present_capture(
        context: *mut RawContext,
        capture_index: u32,
        capture: *mut RawExtendedPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_enable_hfr_pacing_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_pacing_evidence(
        context: *mut RawContext,
        evidence: *mut RawHfrPacingEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_debugger_inspection_for_evidence(
        context: *mut RawContext,
        paused: u32,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_ubershader_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_ubershader_evidence(
        context: *mut RawContext,
        evidence: *mut RawUbershaderEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_resize(
        context: *mut RawContext,
        width: u32,
        height: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_destroy(context: *mut RawContext);
}

pub(crate) fn capture_adapter_inputs(
    task: &OsTask,
    output_addr: u32,
    width: u32,
    height: u32,
    vi: ViPresentation,
) -> Result<crate::Rt64AdapterCapture, String> {
    let raw_task = RawTask::from(task);
    let vi = raw_vi(vi)?;
    let mut capture = RawAdapterCapture::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: both repr(C) values are live for the synchronous call and the
    // output/error pointers are writable for their advertised full sizes.
    // This entry performs scalar marshalling only and creates no RT64 device.
    let ok = unsafe {
        fn64_rt64_capture_adapter_inputs(
            &raw_task,
            output_addr,
            width,
            height,
            &vi,
            &mut capture,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 adapter capture failed without a diagnostic",
        ));
    }
    Ok(crate::Rt64AdapterCapture {
        task_words: capture.task.words(),
        output_addr: capture.output_addr,
        width: capture.width,
        height: capture.height,
        registers: capture.registers,
        registers_after_submission: capture.registers_after_submission,
    })
}

pub(crate) fn roundtrip_user_config(
    settings: &RenderRuntimeSettings,
) -> Result<RenderRuntimeSettings, String> {
    let input = RawUserConfig::from(settings);
    let mut output = RawUserConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: both repr(C) settings values and the writable error buffer are
    // live for the synchronous scalar-only call. This entry creates no RT64
    // device and retains no pointer.
    let ok = unsafe {
        fn64_rt64_roundtrip_user_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 user-config roundtrip failed without a diagnostic",
        ));
    }
    RenderRuntimeSettings::try_from(output)
}

pub(crate) fn roundtrip_enhancement_config(
    settings: &RenderEnhancementSettings,
) -> Result<RenderEnhancementSettings, String> {
    let input = RawEnhancementConfig::from(settings);
    let mut output = RawEnhancementConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: scalar repr(C) input/output and the error buffer remain live
    // for this synchronous device-free validation call.
    let ok = unsafe {
        fn64_rt64_roundtrip_enhancement_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 enhancement-config roundtrip failed without a diagnostic",
        ));
    }
    RenderEnhancementSettings::try_from(output)
}

pub(crate) fn roundtrip_emulator_config(
    settings: &RenderEmulatorSettings,
) -> Result<RenderEmulatorSettings, String> {
    let input = RawEmulatorConfig::from(settings);
    let mut output = RawEmulatorConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: scalar repr(C) input/output and the error buffer remain live
    // for this synchronous device-free validation call.
    let ok = unsafe {
        fn64_rt64_roundtrip_emulator_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 emulator-config roundtrip failed without a diagnostic",
        ));
    }
    RenderEmulatorSettings::try_from(output)
}

pub(crate) fn inspect_replacement_pack(
    path: &CString,
) -> Result<(RenderReplacementPackIdentity, Vec<u8>), String> {
    let mut config = RawReplacementDatabaseConfig::default();
    let mut database_size = 0usize;
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: path and scalar outputs remain live. A null database pointer is
    // paired with zero capacity for the documented size query.
    let ok = unsafe {
        fn64_rt64_inspect_replacement_pack(
            path.as_ptr(),
            &mut config,
            std::ptr::null_mut(),
            0,
            &mut database_size,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 replacement-pack inspection failed without a diagnostic",
        ));
    }
    let mut database = vec![0u8; database_size];
    let mut second_config = RawReplacementDatabaseConfig::default();
    let mut second_size = 0usize;
    error.fill(0);
    // SAFETY: the exact capacity returned by the first pass is writable and
    // all other pointers remain live for the synchronous device-free call.
    let ok = unsafe {
        fn64_rt64_inspect_replacement_pack(
            path.as_ptr(),
            &mut second_config,
            database.as_mut_ptr(),
            database.len(),
            &mut second_size,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 replacement-pack second inspection failed without a diagnostic",
        ));
    }
    if second_size != database.len() || second_config != config {
        return Err("replacement database changed between inspection passes".into());
    }
    Ok((replacement_identity_from_raw(config)?, database))
}

fn error_string(buffer: &[c_char; ERROR_CAPACITY], fallback: &str) -> String {
    // SAFETY: every C ABI operation receives the full buffer capacity and
    // the shim always writes a trailing NUL when it reports an error. The
    // zero-initialized Rust buffer also guarantees a NUL if no text arrived.
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}

pub(crate) struct Context(NonNull<RawContext>);

impl Context {
    pub(crate) fn create(
        width: u32,
        height: u32,
        nominal_refresh_rate: u32,
        user_settings: &RenderRuntimeSettings,
        enhancement_settings: &RenderEnhancementSettings,
        emulator_settings: &RenderEmulatorSettings,
    ) -> Result<Self, String> {
        let raw_user = RawUserConfig::from(user_settings);
        let raw_enhancement = RawEnhancementConfig::from(enhancement_settings);
        let raw_emulator = RawEmulatorConfig::from(emulator_settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: `error` is writable for the advertised capacity; the C++
        // shim returns either a uniquely-owned opaque context or null.
        let raw = unsafe {
            fn64_rt64_create(
                width,
                height,
                nominal_refresh_rate,
                &raw_user,
                &raw_enhancement,
                &raw_emulator,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| error_string(&error, "RT64 create failed without a diagnostic"))
    }

    pub(crate) fn apply_user_config(
        &mut self,
        settings: &RenderRuntimeSettings,
    ) -> Result<bool, String> {
        let raw_settings = RawUserConfig::from(settings);
        let mut framebuffers_discarded = 0;
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // and result pointers remain live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_user_config(
                self.0.as_ptr(),
                &raw_settings,
                &mut framebuffers_discarded,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 settings apply failed without a diagnostic",
            ))
        } else {
            match framebuffers_discarded {
                0 => Ok(false),
                1 => Ok(true),
                value => Err(format!(
                    "RT64 returned invalid framebuffer-discard boolean {value}"
                )),
            }
        }
    }

    pub(crate) fn apply_enhancement_config(
        &mut self,
        settings: &RenderEnhancementSettings,
    ) -> Result<(), String> {
        let raw_settings = RawEnhancementConfig::from(settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // pointer remains live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_enhancement_config(
                self.0.as_ptr(),
                &raw_settings,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 enhancement apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn apply_emulator_config(
        &mut self,
        settings: &RenderEmulatorSettings,
    ) -> Result<(), String> {
        let raw_settings = RawEmulatorConfig::from(settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // pointer remains live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_emulator_config(
                self.0.as_ptr(),
                &raw_settings,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 emulator apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    fn apply_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
        reload: bool,
    ) -> Result<(), String> {
        let raw: Vec<_> = packs
            .iter()
            .map(|(path, identity)| RawReplacementPack {
                path_utf8: path.as_ptr(),
                expected_database: RawReplacementDatabaseConfig::from(identity),
            })
            .collect();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is uniquely borrowed; every CString and raw
        // entry remains live for this synchronous call and no pointer is kept.
        let function = if reload {
            fn64_rt64_reload_replacement_packs
        } else {
            fn64_rt64_load_replacement_packs
        };
        let ok = unsafe {
            function(
                self.0.as_ptr(),
                raw.as_ptr(),
                raw.len(),
                u32::from(enabled),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 replacement-pack apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn load_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
    ) -> Result<(), String> {
        self.apply_replacement_packs(packs, enabled, false)
    }

    pub(crate) fn reload_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
    ) -> Result<(), String> {
        self.apply_replacement_packs(packs, enabled, true)
    }

    pub(crate) fn set_replacement_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed for this scalar
        // synchronous call.
        let ok = unsafe {
            fn64_rt64_set_replacement_enabled(
                self.0.as_ptr(),
                u32::from(enabled),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 replacement enable failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn wait_texture_replacement_state(
        &mut self,
        texture_hash: Option<u64>,
        require_replacement: bool,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let mut raw = RawTextureReplacementState::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; the state and
        // diagnostic buffers remain writable for this synchronous wait.
        let ok = unsafe {
            fn64_rt64_wait_texture_replacement_state(
                self.0.as_ptr(),
                texture_hash.unwrap_or(0),
                u32::from(require_replacement),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 texture-replacement evidence failed without a diagnostic",
            ));
        }
        Self::texture_replacement_evidence_from_raw(raw)
    }

    fn texture_replacement_evidence_from_raw(
        raw: RawTextureReplacementState,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let boolean = |name: &str, value: u32| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("RT64 returned invalid {name} boolean {value}")),
        };
        Ok(crate::Rt64TextureReplacementEvidence {
            texture_hash: raw.texture_hash,
            stream_load_count: raw.stream_load_count,
            texture_count: raw.texture_count,
            texture_known: boolean("texture-known", raw.texture_known)?,
            replacement_resolved: boolean("replacement-resolved", raw.replacement_resolved)?,
            replacement_installed: boolean("replacement-installed", raw.replacement_installed)?,
            replacement_mip_levels: raw.replacement_mip_levels,
            replacements_enabled: boolean("replacements-enabled", raw.replacements_enabled)?,
            stream_queued: raw.stream_queued,
            stream_active: raw.stream_active,
            stream_results_pending: raw.stream_results_pending,
            uploads_pending: raw.uploads_pending,
            resolved_paths_pending: raw.resolved_paths_pending,
            observed_resolved_not_installed: boolean(
                "observed-resolved-not-installed",
                raw.observed_resolved_not_installed,
            )?,
            stream_workers_paused: boolean("stream-workers-paused", raw.stream_workers_paused)?,
            stream_worker_count: raw.stream_worker_count,
        })
    }

    pub(crate) fn set_stream_workers_paused(&mut self, paused: bool) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The strict C++
        // control accepts only a quiescent worker set and retains no pointer.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                self.0.as_ptr(),
                u32::from(paused),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 stream-worker evidence control failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn wait_stream_fallback_state(
        &mut self,
        texture_hash: u64,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let mut raw = RawTextureReplacementState::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; output and
        // diagnostic buffers remain writable for the synchronous state wait.
        let ok = unsafe {
            fn64_rt64_wait_stream_fallback_state(
                self.0.as_ptr(),
                texture_hash,
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 stream-fallback evidence failed without a diagnostic",
            ));
        }
        Self::texture_replacement_evidence_from_raw(raw)
    }

    pub(crate) fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut RspMemory,
        task: &OsTask,
        output_addr: u32,
        admission: &crate::Rt64TaskAdmission,
    ) -> Result<NativeTaskOutcome, String> {
        let raw_task = RawTask::from(task);
        let prepared_plan = PreparedUcodePlan::new(admission)?;
        let raw_plan = prepared_plan.raw();
        let mut dmem = *rsp_memory.bank(RspMemoryBank::Dmem);
        let mut imem = *rsp_memory.bank(RspMemoryBank::Imem);
        let mut result = RawTaskResult::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed; both
        // slice pointer/length and the repr(C) task remain valid for the
        // synchronous call. The shim waits for RT64's render-to-RAM worker
        // before returning, so no foreign thread retains the Rust borrow.
        let ok = unsafe {
            fn64_rt64_process_task(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                dmem.as_mut_ptr(),
                dmem.len(),
                imem.as_mut_ptr(),
                imem.len(),
                &raw_task,
                output_addr,
                &raw_plan,
                &mut result,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            let expected_generation_count = u32::try_from(prepared_plan.generations.len())
                .expect("validated microcode generation count fits u32");
            let outcome =
                task_result_from_raw(result, expected_generation_count, prepared_plan.plan_sha256)?;
            if matches!(outcome, NativeTaskOutcome::Complete(_)) {
                if rsp_memory.bank(RspMemoryBank::Dmem) != &dmem {
                    rsp_memory
                        .write_bytes(RspMemAddr::from_register(0), &dmem)
                        .expect("RT64 returned a complete 4 KiB DMEM bank");
                }
                if rsp_memory.bank(RspMemoryBank::Imem) != &imem {
                    rsp_memory
                        .write_bytes(RspMemAddr::from_register(0x1000), &imem)
                        .expect("RT64 returned a complete 4 KiB IMEM bank");
                }
            }
            Ok(outcome)
        } else {
            Err(error_string(
                &error,
                "RT64 task processing failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed, and RT64 waits
        // for the submitted render-to-RAM workload before this call returns.
        let ok = unsafe {
            fn64_rt64_process_rdp_commands(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                start,
                end,
                output_addr,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 raw RDP processing failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn present(
        &mut self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        vi: ViPresentation,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        let vi = raw_vi(vi)?;
        // SAFETY: the opaque context is alive and uniquely borrowed. The
        // call-scoped physical capability proves the exact 8 MiB allocation
        // remains live. This entry only reads VI source bytes; the shim waits
        // every present worker and restores its placeholder aliases before
        // returning.
        let ok = unsafe {
            fn64_rt64_present(
                self.0.as_ptr(),
                memory.as_mut_ptr(),
                memory.len(),
                &vi,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 present failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn enable_present_capture(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed. Hook
        // registration is synchronous and retains no Rust-owned pointer.
        let ok = unsafe {
            fn64_rt64_enable_present_capture(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 present capture could not be enabled without a diagnostic",
            ))
        }
    }

    /// Concrete graphics API observed from the most recent completed capture.
    /// The C++ hook publishes this under the same mutex and generation as the
    /// pixel geometry, so requested settings cannot manufacture the value.
    pub(crate) fn presented_graphics_api(&self) -> Result<ActiveRenderGraphicsApi, String> {
        let mut metadata = RawPresentCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive. The C++ metadata-only query locks the
        // capture owner and writes only the caller-owned output/error buffers.
        let queried = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if queried == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture query failed without a diagnostic",
            ));
        }
        let (_, _, graphics_api) = validate_present_capture_metadata(metadata)?;
        Ok(graphics_api)
    }

    pub(crate) fn presented_pixels(&mut self) -> Result<crate::Rt64PresentedPixels, String> {
        let mut metadata = RawPresentCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; null with zero
        // capacity is the C API's metadata-only query form.
        let queried = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if queried == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture query failed without a diagnostic",
            ));
        }
        let (byte_len, format, graphics_api) = validate_present_capture_metadata(metadata)?;
        let mut bytes = vec![0; byte_len];
        let queried_metadata = metadata;
        error.fill(0);
        // SAFETY: `bytes` is writable for exactly the capacity advertised by
        // the preceding metadata query. No later present can race this call
        // through the unique Rust borrow of the context.
        let read = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                bytes.as_mut_ptr(),
                bytes.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if read == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture read failed without a diagnostic",
            ));
        }
        if metadata != queried_metadata {
            return Err("RT64 present capture metadata changed during synchronous readback".into());
        }
        Ok(crate::Rt64PresentedPixels {
            width: metadata.width,
            height: metadata.height,
            row_bytes: metadata.row_bytes,
            format,
            graphics_api,
            present_id: metadata.present_id,
            workload_id: metadata.workload_id,
            bytes,
        })
    }

    pub(crate) fn present_selection(&mut self) -> Result<crate::Rt64PresentSelection, String> {
        let mut selection = RawPresentSelection::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed. The C++
        // query waits both RT64 queue workers idle before reading descriptor
        // and render-target state into this fixed-size output value.
        let ok = unsafe {
            fn64_rt64_read_present_selection(
                self.0.as_ptr(),
                &mut selection,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 present-selection query failed without a diagnostic",
            ));
        }
        Ok(crate::Rt64PresentSelection {
            present_id: selection.present_id,
            source_texture_identity: selection.source_texture_identity,
            target_address: selection.target_address,
            target_width: selection.target_width,
            target_height: selection.target_height,
            target_size: selection.target_size,
        })
    }

    pub(crate) fn enable_deferred_workload_capture(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. Arming only
        // changes shim-owned evidence state after both RT64 workers are idle.
        let ok = unsafe {
            fn64_rt64_enable_deferred_workload_capture(
                self.0.as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 deferred-workload capture could not be armed without a diagnostic",
            ))
        }
    }

    pub(crate) fn deferred_workload_evidence(
        &mut self,
    ) -> Result<crate::Rt64DeferredWorkloadEvidence, String> {
        let mut raw = RawDeferredWorkloadEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; C++ waits both
        // worker queues idle before copying fixed-size scalar snapshots.
        let ok = unsafe {
            fn64_rt64_read_deferred_workload_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 deferred-workload evidence query failed without a diagnostic",
            ));
        }
        Ok(crate::Rt64DeferredWorkloadEvidence {
            pre_submission: deferred_snapshot(raw.pre_submission),
            current: deferred_snapshot(raw.current),
        })
    }

    pub(crate) fn framebuffer_copy_path_evidence(
        &mut self,
    ) -> Result<crate::Rt64FramebufferCopyPathEvidence, String> {
        let mut raw = RawFramebufferCopyPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; C++ waits both
        // worker queues idle before reading the completed workload's bounded
        // path counters into this fixed-size value.
        let ok = unsafe {
            fn64_rt64_read_framebuffer_copy_path_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 framebuffer-copy path evidence query failed without a diagnostic",
            ));
        }
        if raw.reserved != 0 {
            return Err("RT64 returned nonzero reserved framebuffer-copy evidence".into());
        }
        let path = match raw.path {
            FRAMEBUFFER_COPY_PATH_GPU => crate::Rt64FramebufferCopyPath::GpuTileCopy,
            FRAMEBUFFER_COPY_PATH_CPU => crate::Rt64FramebufferCopyPath::CpuRdramTmemUpload,
            other => {
                return Err(format!(
                    "RT64 returned unknown framebuffer-copy path tag {other}"
                ));
            }
        };
        Ok(crate::Rt64FramebufferCopyPathEvidence {
            workload_id: raw.workload_id,
            source_framebuffer_identity: raw.source_framebuffer_identity,
            source_framebuffer_address: raw.source_framebuffer_address,
            path,
            gpu_create_tile_copy_operation_count: raw.gpu_create_tile_copy_operation_count,
            gpu_tile_dispatch_count: raw.gpu_tile_dispatch_count,
            cpu_rdram_tmem_upload_count: raw.cpu_rdram_tmem_upload_count,
            raw_tmem_tile_count: raw.raw_tmem_tile_count,
            sync_framebuffer_pair_count: raw.sync_framebuffer_pair_count,
        })
    }

    pub(crate) fn s2dex_fast_path_evidence(
        &mut self,
    ) -> Result<crate::Rt64S2dexFastPathEvidence, String> {
        let mut raw = RawS2dexFastPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: both renderer queues are joined by C++ before it copies the
        // completed workload's scalar/vector counts into this fixed wire image.
        let ok = unsafe {
            fn64_rt64_read_s2dex_fast_path_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 S2DEX fast-path evidence query failed without a diagnostic",
            ));
        }
        if raw.reserved != 0 || raw.source_is_managed_framebuffer > 1 {
            return Err("RT64 returned invalid S2DEX fast-path evidence wire fields".into());
        }
        Ok(crate::Rt64S2dexFastPathEvidence {
            workload_id: raw.workload_id,
            source_framebuffer_identity: raw.source_framebuffer_identity,
            load_operation_digest: raw.load_operation_digest,
            source_address: raw.source_address,
            source_width: raw.source_width,
            source_height: raw.source_height,
            source_size: raw.source_size,
            gpu_create_tile_copy_operation_count: raw.gpu_create_tile_copy_operation_count,
            gpu_tile_dispatch_count: raw.gpu_tile_dispatch_count,
            cpu_rdram_tmem_upload_count: raw.cpu_rdram_tmem_upload_count,
            raw_tmem_tile_count: raw.raw_tmem_tile_count,
            sync_framebuffer_pair_count: raw.sync_framebuffer_pair_count,
            framebuffer_pair_count: raw.framebuffer_pair_count,
            valid_tile_count: raw.valid_tile_count,
            load_operation_count: raw.load_operation_count,
            distinct_source_address_count: raw.distinct_source_address_count,
            minimum_source_address: raw.minimum_source_address,
            maximum_source_address: raw.maximum_source_address,
            base_source_load_count: raw.base_source_load_count,
            offset_source_load_count: raw.offset_source_load_count,
            source_is_managed_framebuffer: raw.source_is_managed_framebuffer != 0,
        })
    }

    pub(crate) fn enable_extended_gbi_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // queues idle and arms only shim-owned pass-through observation state.
        let ok = unsafe {
            fn64_rt64_enable_extended_gbi_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 Extended-GBI evidence could not be armed without a diagnostic",
            ))
        }
    }

    pub(crate) fn extended_gbi_evidence(
        &mut self,
    ) -> Result<crate::Rt64ExtendedGbiEvidence, String> {
        let mut raw = RawExtendedGbiEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // RT64 queues idle before copying one fixed-size bounded wire image.
        let ok = unsafe {
            fn64_rt64_read_extended_gbi_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 Extended-GBI evidence query failed without a diagnostic",
            ));
        }
        extended_evidence_from_raw(raw)
    }

    pub(crate) fn extended_presented_pixels(
        &mut self,
    ) -> Result<Vec<crate::Rt64ExtendedPresentedPixels>, String> {
        let mut captures: Vec<crate::Rt64ExtendedPresentedPixels> = Vec::new();
        let mut expected_count = None;
        for index in 0..EXTENDED_MAX_GENERATED_PRESENTS {
            let mut metadata = RawExtendedPresentCapture::default();
            let mut error = [0; ERROR_CAPACITY];
            // SAFETY: metadata-only query with a null byte pointer and zero
            // capacity. Extended evidence finalization already joined the
            // present worker before exposing this retained slot.
            let queried = unsafe {
                fn64_rt64_read_extended_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    std::ptr::null_mut(),
                    0,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if queried == 0 {
                return Err(error_string(
                    &error,
                    "RT64 Extended present-capture query failed without a diagnostic",
                ));
            }
            let count = usize::try_from(metadata.capture_count)
                .map_err(|_| "RT64 Extended capture count exceeds host space".to_string())?;
            if count == 0 || count > EXTENDED_MAX_GENERATED_PRESENTS {
                return Err("RT64 Extended capture count exceeds bounded capacity".into());
            }
            if let Some(expected) = expected_count {
                if count != expected {
                    return Err("RT64 Extended capture count changed during readback".into());
                }
            } else {
                expected_count = Some(count);
            }
            let byte_len = usize::try_from(metadata.byte_len)
                .map_err(|_| "RT64 Extended capture exceeds host address space".to_string())?;
            let mut bytes = vec![0; byte_len];
            let queried_metadata = metadata;
            error.fill(0);
            // SAFETY: the byte allocation exactly matches the metadata-only
            // query and the unique context borrow excludes a concurrent Rust
            // producer while C++ retains the slot under its capture mutex.
            let read = unsafe {
                fn64_rt64_read_extended_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if read == 0 {
                return Err(error_string(
                    &error,
                    "RT64 Extended present-capture read failed without a diagnostic",
                ));
            }
            if metadata != queried_metadata {
                return Err("RT64 Extended capture metadata changed during readback".into());
            }
            if metadata.capture_ordinal != index as u32 {
                return Err("RT64 Extended capture ordinal changed during ordered readback".into());
            }
            let capture = extended_present_capture_from_raw(metadata, bytes)?;
            if let Some(first) = captures.first() {
                if capture.workload_id != first.workload_id
                    || capture.present_id != first.present_id
                    || capture.capture_generation <= captures.last().unwrap().capture_generation
                {
                    return Err(
                        "RT64 Extended capture history identity or generation order changed".into(),
                    );
                }
            }
            captures.push(capture);
            if captures.len() == count {
                return Ok(captures);
            }
        }
        Err("RT64 Extended capture count exceeds bounded capacity".into())
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn enable_hfr_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ joins both
        // workers before arming only shim-owned bounded capture state.
        let ok = unsafe {
            fn64_rt64_enable_hfr_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 HFR evidence could not be armed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "synthetic-f3dex2-evidence")]
    pub(crate) fn process_synthetic_hfr_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the mutable allocation is valid for the passed length and
        // uniquely borrowed for the synchronous evidence-only HLE call.
        let ok = unsafe {
            fn64_rt64_process_synthetic_hfr_f3dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                original_refresh_rate,
                std::ptr::null_mut(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "synthetic RT64 HFR F3DEX2 processing failed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "region-rate-evidence")]
    pub(crate) fn process_synthetic_region_rate_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<crate::Rt64RegionRateEvidence, String> {
        let mut raw = RawRegionRateEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the mutable allocation and evidence output are valid for
        // the synchronous evidence-only HLE call and uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_process_synthetic_hfr_f3dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                0,
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "synthetic RT64 region-rate F3DEX2 processing failed without a diagnostic",
            ));
        }
        if raw.workload_id == 0
            || raw.extended_refresh_override_absent != 1
            || raw.configured_nominal_refresh_rate != raw.registered_nominal_refresh_rate
        {
            return Err("RT64 region-rate evidence returned inconsistent authority".into());
        }
        Ok(crate::Rt64RegionRateEvidence {
            workload_id: raw.workload_id,
            configured_nominal_refresh_rate: raw.configured_nominal_refresh_rate,
            registered_nominal_refresh_rate: raw.registered_nominal_refresh_rate,
            workload_original_refresh_rate: raw.workload_original_refresh_rate,
        })
    }

    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub(crate) fn process_synthetic_s2dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the allocation is valid for its passed length and uniquely
        // borrowed throughout the synchronous evidence-only HLE call.
        let ok = unsafe {
            fn64_rt64_process_synthetic_s2dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "synthetic RT64 S2DEX2 processing failed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "extended-gbi-evidence")]
    pub(crate) fn process_synthetic_extended_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        self.process_synthetic_hfr_f3dex2(rdram, display_list, output_addr, 60)
            .map_err(|reason| reason.replace("synthetic RT64 HFR", "synthetic RT64 Extended-GBI"))
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_evidence(&mut self) -> Result<crate::Rt64HfrEvidence, String> {
        let mut raw = RawHfrEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: C++ joins both workers and copies one fixed-size scalar wire
        // image while the live context is uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_read_hfr_evidence(self.0.as_ptr(), &mut raw, error.as_mut_ptr(), error.len())
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 HFR evidence query failed without a diagnostic",
            ));
        }
        hfr_evidence_from_raw(raw)
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_presented_pixels(
        &mut self,
    ) -> Result<Vec<crate::Rt64HfrPresentedPixels>, String> {
        let mut captures: Vec<crate::Rt64HfrPresentedPixels> = Vec::new();
        let mut expected_count = None;
        for index in 0..EXTENDED_MAX_GENERATED_PRESENTS {
            let mut metadata = RawExtendedPresentCapture::default();
            let mut error = [0; ERROR_CAPACITY];
            // SAFETY: null bytes with zero capacity is the metadata-only form;
            // the HFR evidence query finalized and joined this history.
            let queried = unsafe {
                fn64_rt64_read_hfr_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    std::ptr::null_mut(),
                    0,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if queried == 0 {
                return Err(error_string(
                    &error,
                    "RT64 HFR present-capture query failed without a diagnostic",
                ));
            }
            let count = usize::try_from(metadata.capture_count)
                .map_err(|_| "RT64 HFR capture count exceeds host space".to_string())?;
            if count == 0 || count > EXTENDED_MAX_GENERATED_PRESENTS {
                return Err("RT64 HFR capture count exceeds bounded capacity".into());
            }
            if let Some(expected) = expected_count {
                if count != expected {
                    return Err("RT64 HFR capture count changed during readback".into());
                }
            } else {
                expected_count = Some(count);
            }
            let byte_len = usize::try_from(metadata.byte_len)
                .map_err(|_| "RT64 HFR capture exceeds host address space".to_string())?;
            let mut bytes = vec![0; byte_len];
            let queried_metadata = metadata;
            error.fill(0);
            // SAFETY: the allocation exactly matches the preceding metadata
            // query, and the unique borrow excludes a new Rust producer.
            let read = unsafe {
                fn64_rt64_read_hfr_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if read == 0 {
                return Err(error_string(
                    &error,
                    "RT64 HFR present-capture read failed without a diagnostic",
                ));
            }
            if metadata != queried_metadata || metadata.capture_ordinal != index as u32 {
                return Err("RT64 HFR capture metadata changed during ordered readback".into());
            }
            let capture = hfr_present_capture_from_raw(metadata, bytes)?;
            if let Some(first) = captures.first() {
                if capture.workload_id != first.workload_id
                    || capture.present_id != first.present_id
                    || capture.capture_generation <= captures.last().unwrap().capture_generation
                {
                    return Err("RT64 HFR capture identity or generation order changed".into());
                }
            }
            captures.push(capture);
            if captures.len() == count {
                return Ok(captures);
            }
        }
        Err("RT64 HFR capture count exceeds bounded capacity".into())
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn enable_hfr_pacing_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the live context is uniquely borrowed. C++ joins both queue
        // workers before resetting and arming mutex-protected bounded state.
        let ok = unsafe {
            fn64_rt64_enable_hfr_pacing_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 HFR pacing evidence could not be armed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_pacing_evidence(&mut self) -> Result<crate::Rt64HfrPacingEvidence, String> {
        let mut raw = RawHfrPacingEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: C++ joins both workers before copying the fixed-size scalar
        // wire image while the live context is uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_read_hfr_pacing_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 HFR pacing evidence query failed without a diagnostic",
            ));
        }
        hfr_pacing_from_raw(raw)
    }

    pub(crate) fn set_debugger_inspection_for_evidence(
        &mut self,
        paused: bool,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: bool,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the live context is uniquely borrowed. C++ first waits both
        // RT64 queue threads idle, validates every selected index, then updates
        // the backend-independent DebuggerInspector state by scalar value.
        let ok = unsafe {
            fn64_rt64_set_debugger_inspection_for_evidence(
                self.0.as_ptr(),
                u32::from(paused),
                framebuffer_index,
                draw_call_index,
                u32::from(framebuffer_depth),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 debugger evidence control failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn enable_ubershader_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // queue workers idle, joins ubershader construction, then installs a
        // process-global Metal hook with exclusive ownership validation.
        let ok = unsafe {
            fn64_rt64_enable_ubershader_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 ubershader evidence could not be enabled without a diagnostic",
            ))
        }
    }

    pub(crate) fn ubershader_evidence(&mut self) -> Result<crate::Rt64UbershaderEvidence, String> {
        let mut raw = RawUbershaderEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits the
        // workload and present workers idle before copying atomic counters and
        // bounded renderer-owned scalar evidence into this fixed-size value.
        let ok = unsafe {
            fn64_rt64_read_ubershader_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 ubershader evidence query failed without a diagnostic",
            ));
        }
        Ok(crate::Rt64UbershaderEvidence {
            workload_id: raw.workload_id,
            present_id: raw.present_id,
            descriptor_digest: raw.descriptor_digest,
            pipeline_digest: raw.pipeline_digest,
            graphics_pipeline_construction_events: raw.graphics_pipeline_construction_events,
            background_construction_events: raw.background_construction_events,
            caller_construction_events: raw.caller_construction_events,
            workload_construction_events: raw.workload_construction_events,
            present_construction_events: raw.present_construction_events,
            precreated_pipeline_count: raw.precreated_pipeline_count,
            raster_call_count: raw.raster_call_count,
            matched_ubershader_call_count: raw.matched_ubershader_call_count,
            specialized_shader_count: raw.specialized_shader_count,
            ubershaders_only: raw.ubershaders_only != 0,
            shader_hashes: raw.shader_hashes,
            pipeline_state_indices: raw.pipeline_state_indices,
            pipeline_identities: raw.pipeline_identities,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_resize(
                self.0.as_ptr(),
                width,
                height,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_ne!(
            ok,
            0,
            "{}",
            error_string(&error, "RT64 resize failed without a diagnostic")
        );
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: Context is the unique owner of the pointer returned by
        // fn64_rt64_create and calls destroy exactly once.
        unsafe { fn64_rt64_destroy(self.0.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_task_result_derives_full_sync_count_and_ucode_transition() {
        let plan_sha256 = [0x42; 32];
        let outcome = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 7,
                workload_id_after: 10,
                initial_ucode_text_address: 0x1000,
                initial_ucode_data_address: 0x2000,
                final_ucode_text_address: 0x3000,
                final_ucode_data_address: 0x4000,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 3,
                observed_generation_count: 3,
                rejected_generation: u32::MAX,
                plan_sha256,
            },
            3,
            plan_sha256,
        )
        .unwrap();
        let NativeTaskOutcome::Complete(result) = outcome else {
            panic!("complete native result decoded as NeedsLle");
        };
        assert_eq!(result.dp_full_sync, DpFullSyncStatus::Reached);
        assert_eq!(result.full_sync_count, 3);
        assert_eq!(result.initial_ucode_addresses, (0x1000, 0x2000));
        assert_eq!(result.final_ucode_addresses, (0x3000, 0x4000));
        assert_eq!(result.planned_generation_count, 3);
        assert_eq!(result.observed_generation_count, 3);
        assert_eq!(result.plan_sha256, plan_sha256);

        let no_sync = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 11,
                workload_id_after: 11,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            1,
            plan_sha256,
        )
        .unwrap();
        let NativeTaskOutcome::Complete(no_sync) = no_sync else {
            panic!("complete no-sync result decoded as NeedsLle");
        };
        assert_eq!(no_sync.dp_full_sync, DpFullSyncStatus::NotReached);
        assert_eq!(no_sync.full_sync_count, 0);
    }

    #[test]
    fn native_task_result_preserves_precommit_needs_lle_generation() {
        let plan_sha256 = [0x31; 32];
        let outcome = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 3,
                observed_generation_count: 0,
                rejected_generation: 1,
                plan_sha256,
                ..RawTaskResult::default()
            },
            3,
            plan_sha256,
        )
        .unwrap();
        assert_eq!(
            outcome,
            NativeTaskOutcome::NeedsLle {
                rejected_generation: 1,
                plan_sha256
            }
        );
    }

    #[test]
    fn native_task_result_rejects_untyped_or_inconsistent_success() {
        let plan_sha256 = [0x42; 32];
        for raw in [
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA + 1,
                entry_gbi_available: 1,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 0,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 2,
                workload_id_after: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 2,
                observed_generation_count: 2,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256: [0x24; 32],
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 2,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 1,
                rejected_generation: 0,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 1,
                rejected_generation: 1,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: 99,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
        ] {
            assert!(task_result_from_raw(raw, 1, plan_sha256).is_err());
        }
    }

    #[test]
    fn native_ucode_plan_binds_ordered_logical_and_raw_recognition_images() {
        let generation = |source, text_address, digest| fn64_render::TaskAdmissionGeneration {
            source,
            text_address,
            data_address: 0x4000,
            text_sha256: fn64_render::UcodeDigest::from_sha256([digest; 32]),
            data: fn64_render::MicrocodeDataImageIdentity {
                bytes: 8,
                sha256: [digest.wrapping_add(1); 32],
            },
            family: UcodeId::F3dex2,
        };
        let entry = generation(fn64_render::TaskAdmissionSource::TaskEntry, 0x1000, 0x11);
        let self_load = generation(fn64_render::TaskAdmissionSource::SelfLoad, 0x2000, 0x22);
        let raw_window = |byte| fn64_render::TaskAdmissionRawWindow {
            text: vec![byte; crate::RT64_GBI_TEXT_RECOGNITION_BYTES],
            data: vec![byte.wrapping_add(1); crate::RT64_GBI_DATA_RECOGNITION_BYTES],
        };
        let admission = crate::Rt64TaskAdmission {
            plan: fn64_render::TaskAdmissionPlan::new(entry, [self_load]),
            raw_windows: vec![raw_window(0x31), raw_window(0x42)].into_boxed_slice(),
        };
        let prepared = PreparedUcodePlan::new(&admission).unwrap();
        assert_eq!(prepared.generations.len(), 2);
        assert_eq!(prepared.generations[0].source, UCODE_SOURCE_TASK_ENTRY);
        assert_eq!(prepared.generations[1].source, UCODE_SOURCE_SELF_LOAD);
        assert_eq!(prepared.generations[0].raw_text_offset, 0);
        assert_eq!(
            prepared.generations[0].raw_data_offset as usize,
            crate::RT64_GBI_TEXT_RECOGNITION_BYTES
        );
        assert_eq!(prepared.raw().plan_sha256, prepared.plan_sha256);

        let mut opaque_entry = entry;
        opaque_entry.family = UcodeId::Other(0x5645_4e44);
        let opaque = PreparedUcodePlan::new(&crate::Rt64TaskAdmission {
            plan: fn64_render::TaskAdmissionPlan::new(opaque_entry, []),
            raw_windows: vec![raw_window(0x53)].into_boxed_slice(),
        })
        .unwrap();
        assert_eq!(opaque.generations[0].expected_family, 0);
        assert_eq!(opaque.generations[0].reserved0, 0x5645_4e44);

        let mut changed = admission;
        changed.raw_windows[1].text[0] ^= 0xff;
        assert_ne!(
            prepared.plan_sha256,
            PreparedUcodePlan::new(&changed).unwrap().plan_sha256
        );
    }

    #[test]
    fn vi_status_wire_preserves_every_typed_antialias_mode() {
        for (mode, bits) in [
            (fn64_render::ViAaMode::AaResampleAlways, 0),
            (fn64_render::ViAaMode::AaResampleWhenNeeded, 1 << 8),
            (fn64_render::ViAaMode::ResampleOnly, 2 << 8),
            (fn64_render::ViAaMode::Replicate, 3 << 8),
        ] {
            let vi = ViPresentation {
                scanout: ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba16,
                    antialias_mode: mode,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert_eq!(raw_vi(vi).unwrap().registers[0] & (3 << 8), bits);
        }

        let unspecified = ViPresentation {
            scanout: ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(raw_vi(unspecified).unwrap().registers[0] & (3 << 8), 0);
    }

    #[test]
    fn vi_wire_preserves_the_complete_noise_seed() {
        for noise_seed in [0, 0x0123_4567_89ab_cdef, u64::MAX] {
            let vi = ViPresentation {
                noise_seed,
                ..Default::default()
            };
            assert_eq!(raw_vi(vi).unwrap().noise_seed, noise_seed);
        }
    }

    #[test]
    fn cpp_vi_ingress_rejects_an_odd_half_line_extent() {
        let task = RawTask::default();
        let mut vi = RawVi {
            registers: [0; 14],
            registers_present: 1,
            blanked: 0,
            fade_enabled: 0,
            repeat_line: 0,
            fade_factor: 0,
            reserved: 0,
            noise_seed: 0,
        };
        vi.registers[0] = 3;
        vi.registers[2] = 320;
        vi.registers[9] = 0x006c_02ec;
        vi.registers[10] = 0x0025_0200;
        let mut capture = RawAdapterCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: every pointer references a live fixed-size C-layout value;
        // the adapter capture retains none of them.
        let ok = unsafe {
            fn64_rt64_capture_adapter_inputs(
                &task,
                0,
                320,
                240,
                &vi,
                &mut capture,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        assert!(error_string(&error, "missing malformed-VI diagnostic")
            .contains("invalid width or active window"));
    }

    fn present_capture_wire(format: u32) -> RawPresentCapture {
        RawPresentCapture {
            width: 3,
            height: 2,
            row_bytes: 12,
            format,
            graphics_api: 2,
            reserved: 0,
            byte_len: 24,
            present_id: 11,
            workload_id: 7,
        }
    }

    #[test]
    fn portable_present_capture_abi_accepts_exact_geometry_format_and_observed_api() {
        for (format_tag, format) in [
            (1, crate::Rt64PresentPixelFormat::Bgra8Unorm),
            (2, crate::Rt64PresentPixelFormat::Rgba8Unorm),
        ] {
            for (api_tag, graphics_api) in [
                (1, ActiveRenderGraphicsApi::D3d12),
                (2, ActiveRenderGraphicsApi::Vulkan),
                (3, ActiveRenderGraphicsApi::Metal),
            ] {
                let mut capture = present_capture_wire(format_tag);
                capture.graphics_api = api_tag;
                assert_eq!(
                    validate_present_capture_metadata(capture).unwrap(),
                    (24, format, graphics_api)
                );
            }
        }
    }

    #[test]
    fn portable_present_capture_abi_rejects_bad_pitch_format_and_provenance() {
        for invalid in [
            RawPresentCapture {
                row_bytes: 16,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                format: 3,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                workload_id: 0,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                graphics_api: 0,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                graphics_api: 4,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                reserved: 1,
                ..present_capture_wire(1)
            },
        ] {
            assert!(validate_present_capture_metadata(invalid).is_err());
        }
    }

    #[test]
    fn portable_present_capture_keeps_backend_copy_and_fence_seams() {
        let shim = include_str!("../ffi/fn64_rt64_shim.cpp");
        for required in [
            "minimumLinearTextureAlignmentForPixelFormat",
            "vkCmdCopyImageToBuffer(",
            "D3D12_TEXTURE_DATA_PITCH_ALIGNMENT",
            "d3d_list->d3d->CopyTextureRegion(",
            "present_capture_graphics_api = capture_graphics_api;",
            "capture->graphics_api = context->present_capture_graphics_api;",
            "waitForPresentId(submitted_present);",
            "completed.workloadId",
        ] {
            assert!(
                shim.contains(required),
                "portable present-capture seam lost {required}"
            );
        }
    }

    #[test]
    fn raster_shader_start_stop_overlay_is_identity_bound_and_shape_checked() {
        let cmake = include_str!("../ffi/CMakeLists.txt");
        assert!(cmake.contains("FN64_RT64_COMPILATION_THREAD_START_ORIGINAL"));
        assert!(cmake.contains("FN64_RT64_COMPILATION_THREAD_LOOP_ORIGINAL"));
        assert!(cmake.contains("threadRunning = true;\\n        thread ="));
        assert!(cmake.contains("leaves the destructor as its only post-launch writer"));
        assert!(cmake.contains("9b3cf39bb15fc0c7d52085566197042f4960cc410b241e38457bb817f2501e5b"));
        assert!(cmake.contains("fn64_rt64_nominal_full_rate(this)"));
        let expected_overlay = if cfg!(feature = "hfr-evidence") {
            "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-divot:v1+vi-retrace-cadence:v1+hfr-post-present-call:v1"
        } else {
            "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-divot:v1+vi-retrace-cadence:v1"
        };
        assert_eq!(env!("FN64_RT64_SOURCE_OVERLAY_ID"), expected_overlay);
    }

    fn cpp_logical_rate(nominal_refresh_rate: u32, factor: u32) -> Result<u32, String> {
        let mut logical_rate = u32::MAX;
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the scalar output and error buffer remain live for this
        // synchronous, device-free exact-source probe.
        let ok = unsafe {
            fn64_rt64_probe_logical_rate(
                nominal_refresh_rate,
                factor,
                &mut logical_rate,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing logical-rate diagnostic"))
        } else {
            Ok(logical_rate)
        }
    }

    #[test]
    fn cpp_vi_history_uses_context_region_rate_for_stable_factors() {
        assert_eq!(cpp_logical_rate(60, 1).unwrap(), 60);
        assert_eq!(cpp_logical_rate(60, 2).unwrap(), 30);
        assert_eq!(cpp_logical_rate(50, 1).unwrap(), 50);
        assert_eq!(cpp_logical_rate(50, 2).unwrap(), 25);
    }

    #[test]
    fn cpp_vi_history_rejects_missing_or_invalid_region_authority() {
        assert!(cpp_logical_rate(59, 1).unwrap_err().contains("50 or 60 Hz"));
        assert!(cpp_logical_rate(60, 0).unwrap_err().contains("non-zero"));
    }

    #[test]
    fn cpp_vi_history_keeps_concurrent_region_registrations_isolated() {
        let ntsc = std::thread::spawn(|| {
            for _ in 0..128 {
                assert_eq!(cpp_logical_rate(60, 2).unwrap(), 30);
            }
        });
        let pal = std::thread::spawn(|| {
            for _ in 0..128 {
                assert_eq!(cpp_logical_rate(50, 2).unwrap(), 25);
            }
        });
        ntsc.join().unwrap();
        pal.join().unwrap();
    }

    fn raw_roundtrip(input: RawUserConfig) -> Result<RawUserConfig, String> {
        let mut output = RawUserConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_user_config(&input, &mut output, error.as_mut_ptr(), error.len())
        };
        if ok == 0 {
            Err(error_string(&error, "missing settings diagnostic"))
        } else {
            Ok(output)
        }
    }

    #[test]
    fn cpp_settings_validator_accepts_every_public_enum_tag() {
        type RawEnumField = fn(&mut RawUserConfig) -> &mut u32;
        let base = RawUserConfig::from(&RenderRuntimeSettings::default());
        let fields: &[(RawEnumField, u32)] = &[
            (|raw| &mut raw.graphics_api, 4),
            (|raw| &mut raw.resolution, 3),
            (|raw| &mut raw.display_buffering, 2),
            (|raw| &mut raw.antialiasing, 4),
            (|raw| &mut raw.filtering, 3),
            (|raw| &mut raw.aspect_ratio, 3),
            (|raw| &mut raw.extended_aspect_ratio, 3),
            (|raw| &mut raw.upscale_2d, 3),
            (|raw| &mut raw.refresh_rate, 3),
            (|raw| &mut raw.internal_color_format, 3),
            (|raw| &mut raw.hardware_resolve, 3),
        ];
        for (field, count) in fields {
            for tag in 0..*count {
                let mut raw = base;
                *field(&mut raw) = tag;
                assert_eq!(raw_roundtrip(raw).unwrap(), raw);
            }
        }
    }

    #[test]
    fn cpp_settings_validator_rejects_instead_of_clamping_or_coercing() {
        let base = RawUserConfig::from(&RenderRuntimeSettings::default());
        let invalid = [
            RawUserConfig {
                graphics_api: 4,
                ..base
            },
            RawUserConfig {
                three_point_filtering: 2,
                ..base
            },
            RawUserConfig {
                resolution_multiplier: f64::NAN,
                ..base
            },
            RawUserConfig {
                downsample_multiplier: 0,
                ..base
            },
            RawUserConfig {
                aspect_target: 100.1,
                ..base
            },
            RawUserConfig {
                refresh_rate_target: 1001,
                ..base
            },
        ];
        for raw in invalid {
            let error = raw_roundtrip(raw).unwrap_err();
            assert!(
                error.contains("user-config"),
                "unexpected diagnostic: {error}"
            );
        }
    }

    fn raw_enhancement_roundtrip(
        input: RawEnhancementConfig,
    ) -> Result<RawEnhancementConfig, String> {
        let mut output = RawEnhancementConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_enhancement_config(
                &input,
                &mut output,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing enhancement diagnostic"))
        } else {
            Ok(output)
        }
    }

    fn raw_emulator_roundtrip(input: RawEmulatorConfig) -> Result<RawEmulatorConfig, String> {
        let mut output = RawEmulatorConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_emulator_config(
                &input,
                &mut output,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing emulator diagnostic"))
        } else {
            Ok(output)
        }
    }

    #[test]
    fn cpp_enhancement_and_emulator_validators_reject_unknown_tags_and_booleans() {
        let enhancement = RawEnhancementConfig::from(&RenderEnhancementSettings::default());
        for invalid in [
            RawEnhancementConfig {
                presentation_mode: 3,
                ..enhancement
            },
            RawEnhancementConfig {
                framebuffer_reinterpret_fix_uls: 2,
                ..enhancement
            },
            RawEnhancementConfig {
                s2dex_framebuffer_fast_path: u32::MAX,
                ..enhancement
            },
        ] {
            let error = raw_enhancement_roundtrip(invalid).unwrap_err();
            assert!(
                error.contains("enhancement-config"),
                "unexpected diagnostic: {error}"
            );
        }

        let emulator = RawEmulatorConfig::from(&RenderEmulatorSettings::default());
        for invalid in [
            RawEmulatorConfig {
                post_blend_noise: 2,
                ..emulator
            },
            RawEmulatorConfig {
                framebuffer_render_to_ram: u32::MAX,
                ..emulator
            },
        ] {
            let error = raw_emulator_roundtrip(invalid).unwrap_err();
            assert!(
                error.contains("emulator-config"),
                "unexpected diagnostic: {error}"
            );
        }
    }

    #[test]
    fn stream_worker_evidence_control_rejects_invalid_input_and_missing_setup() {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the validation boundary rejects the invalid scalar before
        // dereferencing the deliberately null context.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                std::ptr::null_mut(),
                2,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        let diagnostic = error_string(&error, "missing stream-worker diagnostic");
        assert!(
            diagnostic.contains("stream_workers_paused") && diagnostic.contains("boolean"),
            "unexpected diagnostic: {diagnostic}"
        );

        error.fill(0);
        // SAFETY: a valid scalar with a null context is the public missing-
        // setup error path and retains no pointer.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                std::ptr::null_mut(),
                1,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        let diagnostic = error_string(&error, "missing stream-worker setup diagnostic");
        assert!(
            diagnostic.contains("requires a completed setup"),
            "unexpected diagnostic: {diagnostic}"
        );
    }

    fn complete_extended_wire() -> RawExtendedGbiEvidence {
        let mut raw = RawExtendedGbiEvidence {
            workload_id: 7,
            present_id: 9,
            enabled_opcode: 0x64,
            hook_enable_count: 1,
            has_refresh_rate: 1,
            refresh_rate: 60,
            rect_count: 1,
            group_count: 1,
            vertex_z_count: 2,
            generated_present_count: 2,
            ..Default::default()
        };
        raw.command_counts[0x06] = 1;
        raw.command_counts[0x09] = 1;
        raw.command_counts[0x0A] = 1;
        raw.command_counts[0x0B] = 1;
        raw.command_counts[0x0C] = 1;
        raw.rects[0] = RawExtendedRectEvidence {
            draw_call_uid: 11,
            left_origin: 0x200,
            right_origin: 0x400,
            left_offset: -4,
            top_offset: 8,
            right_offset: 12,
            bottom_offset: -16,
            upper_left_x: 4,
            upper_left_y: 8,
            lower_right_x: 40,
            lower_right_y: 44,
            aspect_mode: 2,
        };
        raw.groups[0] = RawTransformGroupEvidence {
            group_id: 42,
            projection: 0,
            push: 1,
            decompose: 1,
            editable: 1,
            position_selector: 1,
            rotation_selector: 2,
            scale_selector: 0,
            skew_selector: 0,
            perspective_selector: 1,
            vertex_selector: 1,
            texcoord_selector: 0,
            tile_selector: 2,
            look_at_selector: 1,
            ordering: 1,
            aspect_mode: 2,
            reserved: 0,
        };
        raw.vertex_z[0] = RawVertexZEvidence {
            marker_kind: 1,
            command_vertex_index: 3,
            resolved_source_index: 17,
            affected_face_index_start: 12,
            affected_face_index_count: 6,
        };
        raw.vertex_z[1] = RawVertexZEvidence {
            marker_kind: 2,
            command_vertex_index: u32::MAX,
            resolved_source_index: 17,
            affected_face_index_start: 12,
            affected_face_index_count: 6,
        };
        for (index, generated) in raw.generated_presents.iter_mut().take(2).enumerate() {
            *generated = RawGeneratedPresentEvidence {
                previous_workload_id: 6,
                current_workload_id: 7,
                present_id: 9,
                presentation_ordinal: index as u32,
                interpolation_numerator: index as u32 + 1,
                interpolation_denominator: 2,
                original_refresh_rate: 60,
                target_refresh_rate: 120,
            };
        }
        raw
    }

    #[test]
    fn extended_evidence_wire_decodes_every_semantic_field() {
        let evidence = extended_evidence_from_raw(complete_extended_wire()).unwrap();
        assert_eq!(evidence.workload_id, 7);
        assert_eq!(evidence.present_id, 9);
        assert_eq!(evidence.enabled_opcode, Some(0x64));
        assert_eq!(evidence.refresh_rate, Some(60));
        assert_eq!(evidence.rects[0].left_offset, -4);
        assert_eq!(
            evidence.rects[0].aspect_mode,
            crate::Rt64ExtendedAspectMode::Adjust
        );
        assert_eq!(evidence.groups[0].class, crate::Rt64TransformClass::Model);
        assert_eq!(
            evidence.groups[0].rotation,
            crate::Rt64TransformComponentSelector::Auto
        );
        assert_eq!(evidence.vertex_z[0].command_vertex_index, Some(3));
        assert_eq!(evidence.vertex_z[1].command_vertex_index, None);
        assert_eq!(
            (
                evidence.generated_presents[0].interpolation_numerator,
                evidence.generated_presents[0].interpolation_denominator
            ),
            (1, 2)
        );
    }

    #[test]
    fn extended_evidence_wire_rejects_overflow_and_ambiguous_tags() {
        let mut excess = complete_extended_wire();
        excess.group_count = EXTENDED_MAX_GROUPS as u32 + 1;
        assert!(extended_evidence_from_raw(excess)
            .unwrap_err()
            .contains("exceeds capacity"));

        let mut bad_selector = complete_extended_wire();
        bad_selector.groups[0].position_selector = 3;
        assert!(extended_evidence_from_raw(bad_selector)
            .unwrap_err()
            .contains("selector"));

        let mut bad_fraction = complete_extended_wire();
        bad_fraction.generated_presents[0].interpolation_denominator = 0;
        assert!(extended_evidence_from_raw(bad_fraction)
            .unwrap_err()
            .contains("inconsistent generated-presentation"));
    }

    #[cfg(feature = "hfr-evidence")]
    fn exact_double_hfr_wire() -> RawHfrEvidence {
        let mut raw = RawHfrEvidence {
            previous_workload_id: 6,
            current_workload_id: 7,
            present_id: 9,
            interpolation_framebuffer_identity: 11,
            interpolation_framebuffer_address: 0x20_0000,
            original_refresh_rate: 60,
            target_refresh_rate: 120,
            presentation_count: 2,
            available_interpolated_target_count: 1,
            presented_counter_value: 1,
            ..Default::default()
        };
        for (index, generated) in raw.generated_presents.iter_mut().take(2).enumerate() {
            *generated = RawGeneratedPresentEvidence {
                previous_workload_id: 6,
                current_workload_id: 7,
                present_id: 9,
                presentation_ordinal: index as u32,
                interpolation_numerator: index as u32 + 1,
                interpolation_denominator: 2,
                original_refresh_rate: 60,
                target_refresh_rate: 120,
            };
        }
        raw
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_wire_decodes_original_control_and_exact_double_rate() {
        let hfr = hfr_evidence_from_raw(exact_double_hfr_wire()).unwrap();
        assert_eq!(hfr.presentation_count, 2);
        assert_eq!(hfr.presented_counter_value, 1);
        assert_eq!(
            hfr.presentations
                .iter()
                .map(|present| (
                    present.kind,
                    present.derived_weight_numerator,
                    present.derived_weight_denominator,
                ))
                .collect::<Vec<_>>(),
            vec![
                (crate::Rt64HfrPresentationKind::SpatialIntermediate, 1, 2),
                (crate::Rt64HfrPresentationKind::CurrentEndpoint, 2, 2),
            ]
        );

        let control = RawHfrEvidence {
            target_refresh_rate: 0,
            presentation_count: 1,
            available_interpolated_target_count: 0,
            presented_counter_value: 1,
            generated_presents: Default::default(),
            ..exact_double_hfr_wire()
        };
        assert!(hfr_evidence_from_raw(control)
            .unwrap()
            .presentations
            .is_empty());
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_wire_rejects_counter_identity_and_fraction_drift() {
        let mut wrong_counter = exact_double_hfr_wire();
        wrong_counter.presented_counter_value = 2;
        assert!(hfr_evidence_from_raw(wrong_counter).is_err());

        let mut duplicate_id = exact_double_hfr_wire();
        duplicate_id.previous_workload_id = duplicate_id.current_workload_id;
        assert!(hfr_evidence_from_raw(duplicate_id).is_err());

        let mut wrong_fraction = exact_double_hfr_wire();
        wrong_fraction.generated_presents[0].interpolation_numerator = 2;
        assert!(hfr_evidence_from_raw(wrong_fraction).is_err());
    }

    #[cfg(feature = "hfr-evidence")]
    fn exact_hfr_pacing_wire() -> RawHfrPacingEvidence {
        let mut raw = RawHfrPacingEvidence {
            sample_count: 4,
            ..Default::default()
        };
        for burst in 0..2 {
            for ordinal in 0..2 {
                let index = burst * 2 + ordinal;
                let start = 1_000_000 + index as u64 * 8_333_333;
                raw.samples[index] = RawHfrPacingSample {
                    call_start_monotonic_ns: start,
                    call_return_monotonic_ns: start + 20_000,
                    present_id: 10 + burst as u64,
                    burst_ordinal: ordinal as u32,
                    burst_count: 2,
                    original_refresh_rate: 60,
                    target_refresh_rate: 120,
                    swapchain_valid: 1,
                    reserved: 0,
                };
            }
        }
        raw
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_pacing_wire_decodes_exact_ordered_multi_burst_calls() {
        let pacing = hfr_pacing_from_raw(exact_hfr_pacing_wire()).unwrap();
        assert_eq!(pacing.samples.len(), 4);
        assert_eq!(pacing.samples[0].present_id, 10);
        assert_eq!(pacing.samples[1].burst_ordinal, 1);
        assert_eq!(pacing.samples[2].present_id, 11);
        assert_eq!(
            pacing.samples[3].call_return_monotonic_ns - pacing.samples[3].call_start_monotonic_ns,
            20_000
        );
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_pacing_wire_rejects_tail_pair_order_time_and_success_drift() {
        let mut nonempty_tail = exact_hfr_pacing_wire();
        nonempty_tail.samples[4] = nonempty_tail.samples[3];
        assert!(hfr_pacing_from_raw(nonempty_tail).is_err());

        let mut incomplete = exact_hfr_pacing_wire();
        incomplete.sample_count = 3;
        assert!(hfr_pacing_from_raw(incomplete).is_err());

        let mut order_drift = exact_hfr_pacing_wire();
        order_drift.samples[1].present_id = 12;
        assert!(hfr_pacing_from_raw(order_drift).is_err());

        let mut zero_duration = exact_hfr_pacing_wire();
        zero_duration.samples[0].call_return_monotonic_ns =
            zero_duration.samples[0].call_start_monotonic_ns;
        assert!(hfr_pacing_from_raw(zero_duration).is_err());

        let mut invalid_success = exact_hfr_pacing_wire();
        invalid_success.samples[0].swapchain_valid = 2;
        assert!(hfr_pacing_from_raw(invalid_success).is_err());
    }

    #[cfg(feature = "synthetic-f3dex2-evidence")]
    #[test]
    fn synthetic_f3dex2_transport_bounds_the_full_rgba16_target() {
        let shim = include_str!("../ffi/fn64_rt64_shim.cpp");
        for required in [
            "static_cast<uint64_t>(context->width) * context->height * 2U",
            "static_cast<uint64_t>(rdram_len) - target_byte_len",
            "(output_addr & 0xFF000000U) != 0U",
        ] {
            assert!(
                shim.contains(required),
                "synthetic target guard lost {required}"
            );
        }
    }

    fn generated_capture_wire(ordinal: u32) -> RawExtendedPresentCapture {
        RawExtendedPresentCapture {
            capture_generation: 20 + u64::from(ordinal),
            workload_id: 7,
            present_id: 9,
            capture_ordinal: ordinal,
            capture_count: 2,
            generated_ordinal: ordinal,
            interpolation_numerator: ordinal + 1,
            interpolation_denominator: 2,
            width: 2,
            height: 1,
            row_bytes: 8,
            format: 1,
            byte_len: 8,
        }
    }

    #[test]
    fn extended_present_capture_wire_decodes_exact_pixels_and_fraction() {
        let pixels = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let first =
            extended_present_capture_from_raw(generated_capture_wire(0), pixels.clone()).unwrap();
        let second = extended_present_capture_from_raw(generated_capture_wire(1), pixels).unwrap();
        assert_eq!(first.generated_ordinal, Some(0));
        assert_eq!(second.generated_ordinal, Some(1));
        assert_eq!(first.bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            (
                second.interpolation_numerator,
                second.interpolation_denominator
            ),
            (2, 2)
        );
    }

    #[test]
    fn extended_present_capture_wire_rejects_bad_count_geometry_and_provenance() {
        let bytes = vec![0; 8];
        for invalid in [
            RawExtendedPresentCapture {
                capture_count: 9,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                workload_id: 0,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                generated_ordinal: 1,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                row_bytes: 4,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                format: 99,
                ..generated_capture_wire(0)
            },
        ] {
            assert!(extended_present_capture_from_raw(invalid, bytes.clone()).is_err());
        }
    }

    #[test]
    fn extended_evidence_controls_fail_loudly_without_a_live_context() {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the public C boundary validates the deliberately null
        // context before dereferencing it and retains no pointer.
        let armed = unsafe {
            fn64_rt64_enable_extended_gbi_evidence(
                std::ptr::null_mut(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(armed, 0);
        assert!(error_string(&error, "missing arm diagnostic").contains("not initialized"));

        error.fill(0);
        let mut evidence = RawExtendedGbiEvidence::default();
        // SAFETY: the public C boundary again validates the null context
        // before touching the live output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_extended_gbi_evidence(
                std::ptr::null_mut(),
                &mut evidence,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing read diagnostic").contains("not initialized"));

        error.fill(0);
        let mut capture = RawExtendedPresentCapture::default();
        // SAFETY: the public C boundary rejects the null context before
        // touching either output pointer.
        let read = unsafe {
            fn64_rt64_read_extended_present_capture(
                std::ptr::null_mut(),
                0,
                &mut capture,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing capture diagnostic").contains("not initialized"));
    }

    #[test]
    fn framebuffer_copy_path_evidence_fails_loudly_without_a_live_context() {
        let mut evidence = RawFramebufferCopyPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the public C boundary rejects the deliberately null context
        // before touching the fixed-size output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_framebuffer_copy_path_evidence(
                std::ptr::null_mut(),
                &mut evidence,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing copy-path diagnostic").contains("not initialized"));
        assert_eq!(evidence, RawFramebufferCopyPathEvidence::default());

        let mut s2dex = RawS2dexFastPathEvidence::default();
        error.fill(0);
        // SAFETY: the same null-context precondition rejects the call before
        // touching the fixed-size S2DEX output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_s2dex_fast_path_evidence(
                std::ptr::null_mut(),
                &mut s2dex,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing S2DEX diagnostic").contains("not initialized"));
        assert_eq!(s2dex, RawS2dexFastPathEvidence::default());
    }
}
