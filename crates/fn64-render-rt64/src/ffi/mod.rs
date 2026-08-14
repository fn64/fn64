use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::NonNull;

#[cfg(test)]
use fn64_render::ViScanoutRegisters;
use fn64_render::{
    ActiveRenderGraphicsApi, AspectTarget, DownsampleMultiplier, DpFullSyncStatus, OsTask,
    RefreshRateTarget, RenderAntialiasing, RenderAspectRatio, RenderDisplayBuffering,
    RenderEmulatorSettings, RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi,
    RenderHardwareResolve, RenderInternalColorFormat, RenderPresentationMode, RenderRefreshRate,
    RenderReplacementAutoPath, RenderReplacementOperation, RenderReplacementPackIdentity,
    RenderReplacementShift, RenderResolution, RenderRuntimeSettings, RenderUpscale2d,
    ResolutionMultiplier, TaskAdmissionUcode, ViPixelType, ViPresentation, ViScanoutState,
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

const UCODE_PLAN_SCHEMA: u32 = 2;
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
    expected_detail: u32,
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

fn raw_ucode_identity(ucode: TaskAdmissionUcode) -> (u32, u32) {
    match ucode {
        TaskAdmissionUcode::Fast3d => (1, 0),
        TaskAdmissionUcode::F3dex => (2, 0),
        TaskAdmissionUcode::F3dlx => (3, 0),
        TaskAdmissionUcode::F3dlxRej => (4, 0),
        TaskAdmissionUcode::F3dex2 => (5, 0),
        TaskAdmissionUcode::F3dex2NoN => (6, 0),
        TaskAdmissionUcode::F3dex2Rej => (7, 0),
        TaskAdmissionUcode::F3dlx2Rej => (8, 0),
        TaskAdmissionUcode::F3dzex2(variant) => (9, variant.canonical_tag()),
        TaskAdmissionUcode::S2dex => (10, 0),
        TaskAdmissionUcode::S2dex2 => (11, 0),
        TaskAdmissionUcode::L3dex => (12, 0),
        TaskAdmissionUcode::L3dex2 => (13, 0),
        TaskAdmissionUcode::Other(value) => (0, value),
    }
}

fn validate_f3dzex2_profile(
    expected: TaskAdmissionUcode,
    observed: Option<fn64_render::F3dzex2Variant>,
) -> Result<(), String> {
    match (expected, observed) {
        (TaskAdmissionUcode::F3dzex2(expected), Some(observed)) if expected == observed => Ok(()),
        (TaskAdmissionUcode::F3dzex2(_), Some(_)) => {
            Err("typed F3DZEX2 variant disagrees with the raw recognition pair".into())
        }
        (TaskAdmissionUcode::F3dzex2(_), None) => {
            Err("typed F3DZEX2 admission lacks a recognized raw text/data pair".into())
        }
        (_, Some(_)) => {
            Err("raw F3DZEX2 text/data pair contradicts the planned microcode family".into())
        }
        (_, None) => Ok(()),
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
            let (expected_family, expected_detail) = raw_ucode_identity(generation.ucode);
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
            validate_f3dzex2_profile(generation.ucode, fn64_render::identify_f3dzex2(window))?;
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
                expected_detail,
                text_sha256: generation.text_sha256.as_bytes(),
                data_sha256: generation.data.sha256,
                reserved: [0; 4],
            });
        }
        let raw_pool_len = u64::try_from(raw_pool.len())
            .map_err(|_| "microcode raw byte pool length exceeds u64".to_owned())?;
        let mut hash = Sha256::new();
        hash.update(b"fn64-rt64-ucode-plan-v2");
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
                generation.expected_detail,
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
    pub(crate) workload_id_before: u64,
    pub(crate) workload_id_after: u64,
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
        workload_id_before: raw.workload_id_before,
        workload_id_after: raw.workload_id_after,
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
    aa_mode_specified: u32,
    vi_filter_flags: u32,
    noise_seed_low: u32,
    noise_seed_high: u32,
    registers: [u32; 24],
    registers_after_submission: [u32; 24],
}

const _: [(); 69 * std::mem::size_of::<u32>()] = [(); std::mem::size_of::<RawAdapterCapture>()];

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
            ));
        }
    };
    let graphics_api = match metadata.graphics_api {
        1 => ActiveRenderGraphicsApi::D3d12,
        2 => ActiveRenderGraphicsApi::Vulkan,
        3 => ActiveRenderGraphicsApi::Metal,
        value => {
            return Err(format!(
                "RT64 returned unknown present graphics API {value}"
            ));
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

// Split by concern: raw config wire structs, the Context impl, and the
// test module live in child modules.
//
// The re-exports below are load-bearing, and the original split omitted them:
// a child sees this module's OWN items, but not names this module merely
// `use`d privately, and two children cannot see each other at all. So
// `config_wire`'s `unsafe extern "C"` block was invisible to `context.rs`
// (37 unresolved `fn64_rt64_*` symbols) and `context.rs`'s `error_string` was
// invisible to `config_wire.rs` (6 more), while `crate::ffi::Context` did not
// resolve from lib.rs because `mod context;` alone publishes no name here.
//
// `pub(crate)` rather than a private `use`, precisely so the children and
// lib.rs resolve these through `ffi`. This does not widen the crate's public
// API: the crate root exports no `ffi`, so `pub(crate)` is the whole reach.
//
// None of this is reachable without the non-default `rt64` feature, which is
// why `cargo build`/CI never typechecked it and the split landed green.
mod config_wire;
pub(crate) use config_wire::*;
mod context;
pub(crate) use context::*;
#[cfg(test)]
mod tests;
