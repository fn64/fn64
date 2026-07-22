//! Thin `RenderBackend` adapter over the pinned MIT RT64 renderer.
//!
//! This is the workspace's only C++/RT64 interop crate. Backend-neutral task
//! admission lives in `fn64-render`; the deterministic software oracle lives
//! in `fn64-render-reference`. Building without the opt-in `rt64` feature
//! keeps the Rust API available but returns named not-ready errors instead of
//! pretending a native renderer exists.

// Unsafe is quarantined to `ffi`: this crate is the workspace's audited C++
// interop boundary. Keep unsafe operations explicit even inside unsafe fns.
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(test)]
#[path = "../adapter_source_identity.rs"]
mod adapter_source_identity;

pub mod extended_gbi;
#[cfg(feature = "f3dzex2-characterization-evidence")]
mod f3dzex2_characterization;
#[cfg(feature = "rt64")]
mod ffi;
mod ingress;
#[cfg(any(feature = "rt64", test))]
mod transaction;

#[cfg(feature = "rt64")]
use std::ffi::CString;
use std::path::{Path, PathBuf};

#[cfg(any(feature = "rt64", test))]
use fn64_render::RenderGraphicsApi;
use fn64_render::{
    ActiveRenderGraphicsApi, F3dex2UcodeCatalog, FrameStatus, MicrocodeDataImageIdentity,
    MicrocodePairCatalog, NonRdpWrite16, NonRdpWrite16Disposition, OsTask, PresentRequest,
    RenderBackend, RenderConfig, RenderEmulatorSettings, RenderEnhancementSettings, RenderError,
    RenderPolicyApply, RenderReplacementPackIdentity, RenderReplacementSettings,
    RenderRuntimePolicy, RenderRuntimeSettings, RenderSettingsApply, UcodeId, ViPresentation,
};
#[cfg(feature = "rt64")]
use fn64_render::{PresentMemory, TaskAdmissionPlan, TaskAdmissionRawWindow};
use sha2::Digest;
#[cfg(feature = "rt64")]
use transaction::{NativeContextLease, NativeRdramRollback, NativeTaskMemoryRollback};

#[cfg(feature = "rt64")]
const RT64_GBI_TEXT_RECOGNITION_BYTES: usize = 0x18d0;
#[cfg(feature = "rt64")]
const RT64_GBI_DATA_RECOGNITION_BYTES: usize = 0x0fc0;

#[cfg(feature = "f3dzex2-characterization-evidence")]
pub use f3dzex2_characterization::{
    Rt64F3dzex2CharacterizationEvidence, Rt64F3dzex2UcodeAddresses,
};

/// C++-observed scalar state at the RT64 adapter boundary.
///
/// The capture is produced without creating a graphics device, but it crosses
/// the same C ABI and uses the same VI-register builder as live presentation.
/// Register indices match the private `RT64::Application::Core` block named in
/// `ffi/fn64_rt64_shim.cpp`; consumers should compare the complete array so a
/// newly populated register changes the evidence digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64AdapterCapture {
    pub task_words: [u32; 14],
    pub output_addr: u32,
    pub width: u32,
    pub height: u32,
    /// Whether STATUS bits 8..=9 came from an authoritative guest register
    /// image rather than the compatibility-only default encoding.
    pub aa_mode_specified: bool,
    /// Exact bounded native VI overlay flags derived by the C++ adapter.
    pub vi_filter_flags: u32,
    /// Guest-cycle-derived seed consumed by native gamma dithering.
    pub noise_seed: u64,
    pub registers: [u32; 24],
    /// Same context after the no-argument refresh used by the next HLE/raw
    /// task or resize. Equality proves that submission did not restore
    /// compatibility geometry over the retained live VI image.
    pub registers_after_submission: [u32; 24],
}

/// Byte layout returned by RT64's post-VI swapchain render-target capture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64PresentPixelFormat {
    /// Four unorm bytes in blue, green, red, alpha order.
    Bgra8Unorm,
    /// Four unorm bytes in red, green, blue, alpha order.
    Rgba8Unorm,
}

/// How the exact RT64 source identity was established at build time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64SourceProvenance {
    GitClean,
    GitDirty,
    Declared,
}

/// Reproducible identity for the concrete RT64 adapter linked into this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64BackendIdentity {
    pub adapter: &'static str,
    /// Canonical SHA-256 of fn64's Rust/C++ adapter sources, shared neutral
    /// render seam, target, and enabled feature set for this build.
    pub adapter_source_sha256: &'static str,
    pub source_id: &'static str,
    pub source_provenance: Rt64SourceProvenance,
    /// Stable revision of fn64's exact-source build overlay.
    pub source_overlay_id: &'static str,
    pub post_vi_api: &'static str,
}

/// Mutex-consistent live evidence from RT64's texture cache and worker queues.
/// This is intended for behavioral closure fixtures, not game policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64TextureReplacementEvidence {
    pub texture_hash: u64,
    pub stream_load_count: u64,
    pub texture_count: u32,
    pub texture_known: bool,
    pub replacement_resolved: bool,
    pub replacement_installed: bool,
    pub replacement_mip_levels: u32,
    pub replacements_enabled: bool,
    pub stream_queued: u32,
    pub stream_active: u32,
    pub stream_results_pending: u32,
    pub uploads_pending: u32,
    pub resolved_paths_pending: u32,
    pub observed_resolved_not_installed: bool,
    pub stream_workers_paused: bool,
    pub stream_worker_count: u32,
}

impl Rt64BackendIdentity {
    /// Stable identity placed inside fixed-cycle live-render evidence.
    pub fn canonical_id(&self) -> String {
        let provenance = match self.source_provenance {
            Rt64SourceProvenance::GitClean => "git-clean",
            Rt64SourceProvenance::GitDirty => "git-dirty",
            Rt64SourceProvenance::Declared => "declared",
        };
        format!(
            "adapter={};adapter_sha256={};source={};provenance={provenance};overlay={};post_vi_api={}",
            self.adapter,
            self.adapter_source_sha256,
            self.source_id,
            self.source_overlay_id,
            self.post_vi_api
        )
    }

    /// Only a clean Git tree binds its source contents without trusting an
    /// externally declared identifier or omitting local modifications.
    pub const fn is_source_authoritative(&self) -> bool {
        matches!(self.source_provenance, Rt64SourceProvenance::GitClean)
    }
}

/// One completed RT64 post-VI swapchain render target.
///
/// `bytes` is tightly packed even when the graphics API's internal readback
/// buffer requires padded rows. These bytes precede the window compositor and
/// display color management; they are not a measurement of analog VI output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64PresentedPixels {
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    /// Backend type observed from the concrete framebuffer and command-list
    /// pair that produced this capture, not inferred from requested settings.
    pub graphics_api: ActiveRenderGraphicsApi,
    pub present_id: u64,
    /// Workload selected by the completed present carrying these pixels.
    pub workload_id: u64,
    pub bytes: Vec<u8>,
}

/// Exact managed render target sampled by the most recently completed RT64
/// VI draw. The texture identity is process-local and intended for behavioral
/// evidence, while the address and dimensions name guest-visible state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64PresentSelection {
    pub present_id: u64,
    pub source_texture_identity: u64,
    pub target_address: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub target_size: u32,
}

pub const RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS: usize = 4;
pub const RT64_DEFERRED_MAX_DRAW_CALLS: usize = 16;

/// Ordered scalar evidence for one pinned-RT64 deferred Workload.
///
/// The content digest excludes queue IDs and debugger selection so it remains
/// stable when the same recorded workload is replayed. The identity digest
/// additionally binds `workload_id` and `present_id`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64DeferredWorkloadSnapshot {
    pub workload_id: u64,
    pub present_id: u64,
    pub submission_frame: u64,
    pub content_digest: u64,
    pub identity_digest: u64,
    pub framebuffer_pair_count: u32,
    pub projection_count: u32,
    pub game_call_count: u32,
    pub triangle_count: u32,
    pub vertex_count: u32,
    pub face_index_count: u32,
    pub rdp_param_count: u32,
    pub load_operation_count: u32,
    pub selected_framebuffer_index: i32,
    pub selected_draw_call_index: i32,
    pub selected_framebuffer_address: u32,
    pub paused: bool,
    pub pair_color_addresses: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub pair_game_call_counts: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub pair_projection_counts: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub call_uids: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
    pub call_fill_colors: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
    pub call_triangle_counts: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
}

/// Pre-submission and current images of the same deferred workload queue slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64DeferredWorkloadEvidence {
    pub pre_submission: Rt64DeferredWorkloadSnapshot,
    pub current: Rt64DeferredWorkloadSnapshot,
}

/// Exclusive completed-workload route for a framebuffer-backed texture load.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64FramebufferCopyPath {
    /// RT64 created and sampled a GPU framebuffer tile copy.
    GpuTileCopy,
    /// RT64 replayed the RDRAM load into TMEM and used the ordinary texture upload.
    CpuRdramTmemUpload,
}

/// Read-only mechanism evidence from one completed region-copy workload.
///
/// The evidence query rejects zero, mixed, or multiple copy routes rather than
/// reducing an ambiguous workload to one policy label.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64FramebufferCopyPathEvidence {
    pub workload_id: u64,
    /// Process-local identity of the prior managed source framebuffer.
    pub source_framebuffer_identity: u64,
    pub source_framebuffer_address: u32,
    pub path: Rt64FramebufferCopyPath,
    pub gpu_create_tile_copy_operation_count: u32,
    pub gpu_tile_dispatch_count: u32,
    pub cpu_rdram_tmem_upload_count: u32,
    pub raw_tmem_tile_count: u32,
    pub sync_framebuffer_pair_count: u32,
}

/// Read-only load geometry and vector counts from one completed S2DEX texture workload.
///
/// These are downstream workload artifacts, not counters in either enhancement
/// branch. The digest binds every ordered texture, tile, and load-operation
/// descriptor; exact multiplicities distinguish the ordinary texture-upload
/// route from the single managed-framebuffer tile-copy route.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64S2dexFastPathEvidence {
    pub workload_id: u64,
    pub source_framebuffer_identity: u64,
    pub load_operation_digest: u64,
    pub source_address: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub source_size: u32,
    pub gpu_create_tile_copy_operation_count: u32,
    pub gpu_tile_dispatch_count: u32,
    pub cpu_rdram_tmem_upload_count: u32,
    pub raw_tmem_tile_count: u32,
    pub sync_framebuffer_pair_count: u32,
    pub framebuffer_pair_count: u32,
    pub valid_tile_count: u32,
    pub load_operation_count: u32,
    pub distinct_source_address_count: u32,
    pub minimum_source_address: u32,
    pub maximum_source_address: u32,
    pub base_source_load_count: u32,
    pub offset_source_load_count: u32,
    pub source_is_managed_framebuffer: bool,
}

pub const RT64_EXTENDED_COMMAND_COUNT: usize = 0x34;
pub const RT64_EXTENDED_MAX_RECTS: usize = 16;
pub const RT64_EXTENDED_MAX_GROUPS: usize = 16;
pub const RT64_EXTENDED_MAX_VERTEX_Z_MARKERS: usize = 16;
pub const RT64_EXTENDED_MAX_GENERATED_PRESENTS: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformClass {
    Model,
    Projection,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformComponentSelector {
    Skip,
    Interpolate,
    Auto,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformOrdering {
    Linear,
    Auto,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64ExtendedAspectMode {
    Auto,
    Stretch,
    Adjust,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedRectEvidence {
    pub draw_call_uid: u32,
    pub left_origin: u16,
    pub right_origin: u16,
    pub left_offset: i32,
    pub top_offset: i32,
    pub right_offset: i32,
    pub bottom_offset: i32,
    pub upper_left_x: i32,
    pub upper_left_y: i32,
    pub lower_right_x: i32,
    pub lower_right_y: i32,
    pub aspect_mode: Rt64ExtendedAspectMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64TransformGroupEvidence {
    pub group_id: u32,
    pub class: Rt64TransformClass,
    pub push: bool,
    pub decompose: bool,
    pub editable: bool,
    pub position: Rt64TransformComponentSelector,
    pub rotation: Rt64TransformComponentSelector,
    pub scale: Rt64TransformComponentSelector,
    pub skew: Rt64TransformComponentSelector,
    pub perspective: Rt64TransformComponentSelector,
    pub vertex: Rt64TransformComponentSelector,
    pub texcoord: Rt64TransformComponentSelector,
    pub tile: Rt64TransformComponentSelector,
    pub look_at: Rt64TransformComponentSelector,
    pub ordering: Rt64TransformOrdering,
    pub aspect_mode: Rt64ExtendedAspectMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64VertexZMarkerKind {
    Begin,
    End,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64VertexZEvidence {
    pub marker_kind: Rt64VertexZMarkerKind,
    pub command_vertex_index: Option<u32>,
    pub resolved_source_index: u32,
    pub affected_face_index_start: u32,
    pub affected_face_index_count: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64GeneratedPresentEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub presentation_ordinal: u32,
    pub interpolation_numerator: u32,
    pub interpolation_denominator: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
}

/// One ordered post-VI image retained from an explicitly armed Extended-GBI
/// evidence interval. `generated_ordinal` is absent for a single ordinary
/// endpoint and present for every generated/interpolated image in a burst.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedPresentedPixels {
    pub capture_generation: u64,
    pub workload_id: u64,
    pub present_id: u64,
    pub capture_ordinal: u32,
    pub generated_ordinal: Option<u32>,
    pub interpolation_numerator: u32,
    pub interpolation_denominator: u32,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    pub bytes: Vec<u8>,
}

/// One completed workload whose source refresh rate was inferred from the
/// context's registered TV standard.
///
/// The evidence-only synthetic transport substitutes only F3DEX2 identity.
/// It emits no Extended GBI refresh-rate command, so pinned RT64's normal
/// FullSync fallback must derive `workload_original_refresh_rate` from the
/// exact `VIHistory` owned by this production context.
#[cfg(feature = "region-rate-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64RegionRateEvidence {
    pub workload_id: u64,
    pub configured_nominal_refresh_rate: u32,
    pub registered_nominal_refresh_rate: u32,
    pub workload_original_refresh_rate: u32,
}

/// One ordered post-VI image from a synthetic HFR evidence burst.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPresentedPixels {
    pub capture_generation: u64,
    pub workload_id: u64,
    pub present_id: u64,
    pub capture_ordinal: u32,
    pub burst_ordinal: Option<u32>,
    pub derived_weight_numerator: u32,
    pub derived_weight_denominator: u32,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    pub bytes: Vec<u8>,
}

#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64HfrPresentationKind {
    SpatialIntermediate,
    CurrentEndpoint,
}

/// Ordered 120/60 presentation identity. The weight is derived from pinned
/// RT64's exact-double-rate integral algorithm; it is not a sampled shader value.
#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPresentationEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub presentation_ordinal: u32,
    pub kind: Rt64HfrPresentationKind,
    pub derived_weight_numerator: u32,
    pub derived_weight_denominator: u32,
}

/// Causal state from one runtime-selected RT64 presentation burst.
///
/// The evidence-only synthetic admission substitutes only microcode hash
/// recognition. Workload matching, interpolation, rendering, presentation,
/// and the user refresh-rate policy are the pinned RT64 mechanisms.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub interpolation_framebuffer_identity: u64,
    pub interpolation_framebuffer_address: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
    pub presentation_count: u32,
    pub available_interpolated_target_count: u32,
    /// The pinned present queue's internal `presented` counter. Both the
    /// Original control and exact-double-rate burst report one here.
    pub presented_counter_value: u32,
    pub presentations: Vec<Rt64HfrPresentationEvidence>,
}

/// One actual swapchain-present call bracketed by a monotonic host clock.
///
/// The start timestamp is taken after RT64's precise sleep and optional
/// present wait; the return timestamp is taken immediately after `present`
/// returns. These are API-call observations, not physical display scanout
/// timestamps.
#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPacingSample {
    pub call_start_monotonic_ns: u64,
    pub call_return_monotonic_ns: u64,
    pub present_id: u64,
    pub burst_ordinal: u32,
    pub burst_count: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
}

/// Bounded ordered actual-present call history from pinned RT64.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPacingEvidence {
    pub samples: Vec<Rt64HfrPacingSample>,
}

/// Typed, bounded evidence from one explicitly armed recognized-HLE task.
///
/// This is an observation surface only. It does not admit a microcode image,
/// enable Extended GBI, or imply that any public feature claim is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedGbiEvidence {
    pub workload_id: u64,
    pub present_id: u64,
    pub enabled_opcode: Option<u8>,
    pub hook_enable_count: u32,
    pub command_counts: [u32; RT64_EXTENDED_COMMAND_COUNT],
    pub refresh_rate: Option<u16>,
    pub rects: Vec<Rt64ExtendedRectEvidence>,
    pub groups: Vec<Rt64TransformGroupEvidence>,
    pub vertex_z: Vec<Rt64VertexZEvidence>,
    pub generated_presents: Vec<Rt64GeneratedPresentEvidence>,
}

pub const RT64_UBERSHADER_MAX_RASTER_CALLS: usize = 16;

/// Exact Metal construction events and ordered raster pipeline selections for
/// one pinned-RT64 evidence interval.
///
/// Pipeline identities are process-local. Background construction is reported
/// separately from caller, workload-worker, and present-worker events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64UbershaderEvidence {
    pub workload_id: u64,
    pub present_id: u64,
    pub descriptor_digest: u64,
    pub pipeline_digest: u64,
    pub graphics_pipeline_construction_events: u64,
    pub background_construction_events: u64,
    pub caller_construction_events: u32,
    pub workload_construction_events: u32,
    pub present_construction_events: u32,
    pub precreated_pipeline_count: u32,
    pub raster_call_count: u32,
    pub matched_ubershader_call_count: u32,
    pub specialized_shader_count: u32,
    pub ubershaders_only: bool,
    pub shader_hashes: [u64; RT64_UBERSHADER_MAX_RASTER_CALLS],
    pub pipeline_state_indices: [u32; RT64_UBERSHADER_MAX_RASTER_CALLS],
    pub pipeline_identities: [u64; RT64_UBERSHADER_MAX_RASTER_CALLS],
}

impl Rt64AdapterCapture {
    /// Stable SHA-256 evidence over a versioned, big-endian encoding.
    pub fn sha256(&self) -> [u8; 32] {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"fn64-rt64-adapter-capture-v3\0");
        for word in self.task_words {
            hasher.update(word.to_be_bytes());
        }
        for word in [self.output_addr, self.width, self.height] {
            hasher.update(word.to_be_bytes());
        }
        hasher.update(u32::from(self.aa_mode_specified).to_be_bytes());
        hasher.update(self.vi_filter_flags.to_be_bytes());
        hasher.update(self.noise_seed.to_be_bytes());
        for word in self.registers {
            hasher.update(word.to_be_bytes());
        }
        for word in self.registers_after_submission {
            hasher.update(word.to_be_bytes());
        }
        hasher.finalize().into()
    }
}

/// Round typed fn64 task and VI state through the production Rust/C/C++ ABI.
///
/// This capture does not initialize SDL, a graphics API, or a GPU. Enabling
/// the `rt64` feature is still required because the C++ shim and pinned MIT
/// RT64 archive are one intentionally quarantined link unit.
pub fn capture_rt64_adapter_inputs(
    task: &OsTask,
    output_addr: u32,
    cfg: RenderConfig,
    vi: ViPresentation,
) -> Result<Rt64AdapterCapture, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::capture_adapter_inputs(task, output_addr, cfg.width, cfg.height, vi).map_err(
            |reason| RenderError::Backend {
                backend: "rt64-adapter-capture",
                reason,
            },
        )
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = (task, output_addr, cfg, vi);
        Err(RenderError::Backend {
            backend: "rt64-adapter-capture",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Validate and round-trip the complete typed settings image through the
/// production Rust/C/C++ ABI without creating a graphics device.
pub fn roundtrip_rt64_runtime_settings(
    settings: &RenderRuntimeSettings,
) -> Result<RenderRuntimeSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_user_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-settings-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-settings-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Device-free validation of the complete pinned enhancement configuration
/// across the production Rust/C/C++ boundary.
pub fn roundtrip_rt64_enhancement_settings(
    settings: &RenderEnhancementSettings,
) -> Result<RenderEnhancementSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_enhancement_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-enhancement-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-enhancement-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Device-free validation of the complete pinned emulator configuration
/// across the production Rust/C/C++ boundary.
pub fn roundtrip_rt64_emulator_settings(
    settings: &RenderEmulatorSettings,
) -> Result<RenderEmulatorSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_emulator_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-emulator-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-emulator-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// RT64's MIT C++ render/HLE core behind one crate-local C ABI boundary.
/// The feature-gated implementation passes fn64's stable RDRAM allocation,
/// the task's ucode/display-list addresses, and a private register block to
/// `RT64::Application::Core`. RT64's render-to-RAM path writes the native
/// RGBA5551 framebuffer back into the same slice the existing fn64 VI path
/// presents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ReplacementPackInput {
    path: PathBuf,
}

impl Rt64ReplacementPackInput {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedReplacementPack {
    input: Rt64ReplacementPackInput,
    canonical_path: PathBuf,
    identity: RenderReplacementPackIdentity,
}

#[cfg(feature = "rt64")]
fn hash_replacement_content(path: &Path) -> Result<[u8; 32], String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("replacement-pack metadata failed for {path:?}: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "replacement-pack root may not be a symlink: {path:?}"
        ));
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64.rt64-replacement-content.v1\0");
    if metadata.is_file() {
        if path.extension().and_then(|value| value.to_str()) != Some("rtz") {
            return Err(format!(
                "replacement-pack file must have lowercase .rtz extension: {path:?}"
            ));
        }
        hasher.update([1]);
        let bytes = std::fs::read(path)
            .map_err(|error| format!("replacement-pack read failed for {path:?}: {error}"))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    } else if metadata.is_dir() {
        hasher.update([2]);
        let mut pending = vec![(PathBuf::new(), path.to_path_buf())];
        let mut files = Vec::new();
        while let Some((relative_dir, absolute_dir)) = pending.pop() {
            let mut entries = std::fs::read_dir(&absolute_dir)
                .map_err(|error| {
                    format!("replacement-pack directory read failed for {absolute_dir:?}: {error}")
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    format!("replacement-pack entry read failed for {absolute_dir:?}: {error}")
                })?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_str().ok_or_else(|| {
                    format!("replacement-pack contains a non-UTF-8 path under {absolute_dir:?}")
                })?;
                let relative = relative_dir.join(name);
                let entry_path = entry.path();
                let entry_metadata = std::fs::symlink_metadata(&entry_path).map_err(|error| {
                    format!("replacement-pack metadata failed for {entry_path:?}: {error}")
                })?;
                if entry_metadata.file_type().is_symlink() {
                    return Err(format!(
                        "replacement-pack contains a symbolic link: {entry_path:?}"
                    ));
                }
                if entry_metadata.is_dir() {
                    pending.push((relative, entry_path));
                } else if entry_metadata.is_file() {
                    let relative = relative
                        .components()
                        .map(|component| {
                            component
                                .as_os_str()
                                .to_str()
                                .expect("all path segments were checked as UTF-8")
                        })
                        .collect::<Vec<_>>()
                        .join("/");
                    files.push((relative, entry_path));
                } else {
                    return Err(format!(
                        "replacement-pack contains a non-file entry: {entry_path:?}"
                    ));
                }
            }
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        for (relative, absolute) in files {
            let relative_bytes = relative.as_bytes();
            let bytes = std::fs::read(&absolute).map_err(|error| {
                format!("replacement-pack read failed for {absolute:?}: {error}")
            })?;
            hasher.update(
                u32::try_from(relative_bytes.len())
                    .map_err(|_| format!("replacement-pack path is too long: {relative}"))?
                    .to_be_bytes(),
            );
            hasher.update(relative_bytes);
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
    } else {
        return Err(format!(
            "replacement-pack path is neither one directory nor one .rtz file: {path:?}"
        ));
    }
    Ok(hasher.finalize().into())
}

#[cfg(feature = "rt64")]
fn resolve_replacement_packs(
    inputs: &[Rt64ReplacementPackInput],
) -> Result<Vec<ResolvedReplacementPack>, String> {
    let mut resolved = Vec::with_capacity(inputs.len());
    let mut seen = std::collections::HashSet::new();
    for input in inputs {
        let root_metadata = std::fs::symlink_metadata(&input.path).map_err(|error| {
            format!(
                "replacement-pack metadata failed for {:?}: {error}",
                input.path
            )
        })?;
        if root_metadata.file_type().is_symlink() {
            return Err(format!(
                "replacement-pack root may not be a symlink: {:?}",
                input.path
            ));
        }
        let canonical_path = std::fs::canonicalize(&input.path).map_err(|error| {
            format!(
                "replacement-pack path resolution failed for {:?}: {error}",
                input.path
            )
        })?;
        if !seen.insert(canonical_path.clone()) {
            return Err(format!(
                "replacement-pack input is duplicated: {canonical_path:?}"
            ));
        }
        let path_utf8 = canonical_path.to_str().ok_or_else(|| {
            format!("replacement-pack root is not valid UTF-8: {canonical_path:?}")
        })?;
        let path_c = CString::new(path_utf8)
            .map_err(|_| format!("replacement-pack root contains NUL: {canonical_path:?}"))?;
        let content_sha256 = hash_replacement_content(&canonical_path)?;
        let (mut identity, database_bytes) = ffi::inspect_replacement_pack(&path_c)?;
        identity.content_sha256 = content_sha256;
        identity.database_sha256 = sha2::Sha256::digest(database_bytes).into();
        // Catch writes that raced the database inspection itself.
        if hash_replacement_content(&canonical_path)? != content_sha256 {
            return Err(format!(
                "replacement-pack changed during inspection: {canonical_path:?}"
            ));
        }
        resolved.push(ResolvedReplacementPack {
            input: input.clone(),
            canonical_path,
            identity,
        });
    }
    Ok(resolved)
}

#[cfg(feature = "rt64")]
fn replacement_ffi_inputs(
    packs: &[ResolvedReplacementPack],
) -> Result<Vec<(CString, RenderReplacementPackIdentity)>, String> {
    packs
        .iter()
        .map(|pack| {
            let path = pack.canonical_path.to_str().ok_or_else(|| {
                format!(
                    "replacement-pack root stopped being UTF-8: {:?}",
                    pack.canonical_path
                )
            })?;
            Ok((
                CString::new(path).expect("validated path has no NUL"),
                pack.identity.clone(),
            ))
        })
        .collect()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Rt64PresentAuthority {
    LiveRegisters,
    BackendOnlyCompatibility,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct CompletedRt64Present {
    guest_cycle: u64,
    authority: Rt64PresentAuthority,
}

impl CompletedRt64Present {
    #[cfg(any(feature = "rt64", test))]
    fn release_guest_cycle(self) -> Result<u64, RenderError> {
        if self.authority != Rt64PresentAuthority::LiveRegisters {
            return Err(RenderError::NotReady(
                "RT64 release capture requires a completed live-register VI present",
            ));
        }
        Ok(self.guest_cycle)
    }
}

#[cfg(feature = "rt64")]
#[derive(Debug)]
struct Rt64TaskAdmission {
    plan: TaskAdmissionPlan,
    raw_windows: Box<[TaskAdmissionRawWindow]>,
}

#[cfg(any(feature = "rt64", test))]
fn validate_native_full_sync_count(
    inspected_count: u64,
    native_count: u64,
) -> Result<fn64_render::DpFullSyncStatus, String> {
    if inspected_count != native_count {
        return Err(format!(
            "native RT64 executed {native_count} FullSync commands but transactional inspection executed {inspected_count}"
        ));
    }
    Ok(if native_count == 0 {
        fn64_render::DpFullSyncStatus::NotReached
    } else {
        fn64_render::DpFullSyncStatus::Reached
    })
}

pub struct Rt64Backend {
    /// TV standard accepted by the last successful `create`. This is
    /// independent of surface resizing and is published only with the live
    /// RT64 policy/device evidence created from the same configuration.
    active_tv_type: Option<fn64_runtime::TvType>,
    /// Dimensions owned by the current native context. Native task/raw-DPC
    /// ingress uses this exact image to bound a nonzero RGBA16 output target.
    active_surface_size: Option<ingress::ActiveSurfaceSize>,
    /// RT64's GBI selection is still HLE. Apply the same exact task-entry
    /// admission as the Rust reference backend before crossing the C ABI.
    f3dex2_ucodes: F3dex2UcodeCatalog,
    /// Exact complete pairs admitted for RT64 runtime-recognition evidence.
    microcode_pairs: MicrocodePairCatalog,
    /// FullSync result of the last successfully committed submission.
    last_dp_full_sync: fn64_render::DpFullSyncStatus,
    #[cfg(feature = "rt64")]
    context: Option<ffi::Context>,
    /// Reused full-device preimage for failure-atomic native task/raw-RDP
    /// calls. It is scratch only and never becomes a second RDRAM authority.
    #[cfg(feature = "rt64")]
    native_rdram_preimage: Vec<u8>,
    #[cfg(not(feature = "rt64"))]
    created: bool,
    /// Authority and guest cycle of the last successfully completed VI
    /// present. A compatibility image remains observable to behavior tests but
    /// cannot be relabeled as live-register release evidence.
    last_present: Option<CompletedRt64Present>,
    /// Requested settings for the next create. This may differ from active
    /// settings only when an apply returned `RestartRequired`.
    configured_settings: RenderRuntimeSettings,
    /// Settings actually installed into the live RT64 application. Release
    /// evidence hashes this image, never pending recreate settings.
    active_settings: Option<RenderRuntimeSettings>,
    configured_enhancement_settings: RenderEnhancementSettings,
    active_enhancement_settings: Option<RenderEnhancementSettings>,
    configured_emulator_settings: RenderEmulatorSettings,
    active_emulator_settings: Option<RenderEmulatorSettings>,
    configured_replacement_packs: Vec<ResolvedReplacementPack>,
    configured_replacement_enabled: bool,
    active_replacement_settings: Option<RenderReplacementSettings>,
}

impl Rt64Backend {
    pub fn new() -> Self {
        Rt64Backend {
            active_tv_type: None,
            active_surface_size: None,
            f3dex2_ucodes: F3dex2UcodeCatalog::default(),
            microcode_pairs: MicrocodePairCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            #[cfg(feature = "rt64")]
            context: None,
            #[cfg(feature = "rt64")]
            native_rdram_preimage: Vec::new(),
            #[cfg(not(feature = "rt64"))]
            created: false,
            last_present: None,
            configured_settings: RenderRuntimeSettings::default(),
            active_settings: None,
            configured_enhancement_settings: RenderEnhancementSettings::default(),
            active_enhancement_settings: None,
            configured_emulator_settings: RenderEmulatorSettings::default(),
            active_emulator_settings: None,
            configured_replacement_packs: Vec::new(),
            configured_replacement_enabled: RenderReplacementSettings::default().enabled,
            active_replacement_settings: None,
        }
    }

    #[cfg(any(feature = "rt64", test))]
    fn clear_active_native_identity(&mut self) {
        self.active_tv_type = None;
        self.active_surface_size = None;
        self.last_present = None;
        self.active_settings = None;
        self.active_enhancement_settings = None;
        self.active_emulator_settings = None;
        self.active_replacement_settings = None;
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
    }

    #[cfg(feature = "rt64")]
    fn invalidate_native_state(&mut self) {
        self.context = None;
        self.clear_active_native_identity();
    }

    /// Present one complete live register image against a standalone
    /// embedder's exact physical allocation. Integrated execution reaches the
    /// same required trait seam through its raw, higher-ranked capability.
    pub fn present_live(&mut self, rdram: &[u8], vi: ViPresentation) -> Result<(), RenderError> {
        <Self as RenderBackend>::present(
            self,
            PresentRequest::live(vi, fn64_runtime::PhysicalRdramRead::from_storage(rdram)),
        )
    }

    /// Present with explicit synthesized backend geometry. This compatibility
    /// path can drive standalone behavior tests but cannot produce release
    /// evidence.
    pub fn present_physical_compatibility(
        &mut self,
        rdram: &[u8],
        vi: ViPresentation,
    ) -> Result<(), RenderError> {
        <Self as RenderBackend>::present(
            self,
            PresentRequest::physical_compatibility(
                vi,
                fn64_runtime::PhysicalRdramRead::from_storage(rdram),
            ),
        )
    }

    /// Platform-wide RT64 source/capture identity used by non-release behavior
    /// examples. On Windows this intentionally retains its historical
    /// D3D12-or-Vulkan label; fixed-cycle evidence must use
    /// [`Self::release_identity_for_api`] instead.
    ///
    /// The build script derives Git state from the selected source tree or
    /// records an explicit `FN64_RT64_SOURCE_ID` as declared provenance.
    #[cfg(feature = "rt64")]
    pub fn release_identity() -> Rt64BackendIdentity {
        Self::release_identity_with_post_vi_api(if cfg!(target_os = "macos") {
            "metal-bgra8-unorm"
        } else if cfg!(target_os = "windows") {
            "d3d12-or-vulkan-bgra8-rgba8-unorm"
        } else {
            "vulkan-bgra8-rgba8-unorm"
        })
    }

    /// Identity of the RT64 source and the concrete graphics API that owns
    /// the release image. Unlike [`Self::release_identity`], this cannot
    /// carry the legacy ambiguous Windows API label.
    #[cfg(feature = "rt64")]
    pub fn release_identity_for_api(api: ActiveRenderGraphicsApi) -> Rt64BackendIdentity {
        Self::release_identity_with_post_vi_api(post_vi_api_for_graphics_api(api))
    }

    #[cfg(feature = "rt64")]
    fn release_identity_with_post_vi_api(post_vi_api: &'static str) -> Rt64BackendIdentity {
        let source_provenance = match env!("FN64_RT64_SOURCE_PROVENANCE") {
            "git-clean" => Rt64SourceProvenance::GitClean,
            "git-dirty" => Rt64SourceProvenance::GitDirty,
            "declared" => Rt64SourceProvenance::Declared,
            value => panic!("unknown RT64 source provenance {value}"),
        };
        Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256: env!("FN64_RT64_ADAPTER_SOURCE_SHA256"),
            source_id: env!("FN64_RT64_SOURCE_ID"),
            source_provenance,
            source_overlay_id: env!("FN64_RT64_SOURCE_OVERLAY_ID"),
            post_vi_api,
        }
    }

    /// Enable exact post-VI swapchain render-target capture.
    ///
    /// The pinned RT64 generic render hook does not expose its framebuffer's
    /// attachment. This opt-in path validates the concrete Plume Metal,
    /// Vulkan, or D3D12 attachment and retains a fenced readback buffer.
    pub fn enable_present_capture(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_present_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for the sole/selected TMEM texture, optionally including its
    /// installed replacement. Completion is defined by RT64's live cache map;
    /// the C++ seam does not use a duration, sleep, or timing threshold.
    pub fn wait_texture_replacement_evidence(
        &mut self,
        texture_hash: Option<u64>,
        require_replacement: bool,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_texture_replacement_state(texture_hash, require_replacement)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-replacement-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (texture_hash, require_replacement);
            Err(RenderError::Backend {
                backend: "rt64-texture-replacement-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Pause or restore RT64's texture Stream workers for a deterministic
    /// behavior fixture. Pause succeeds only when the upload and stream queues
    /// are quiescent; resume recreates the exact pinned-cache worker count.
    /// This is an evidence scheduling gate, not renderer policy.
    pub fn set_texture_stream_workers_paused_for_evidence(
        &mut self,
        paused: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_stream_workers_paused(paused)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-evidence-control",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = paused;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-evidence-control",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Wait for a real RT64 Stream path to be resolved and queued while the
    /// evidence worker hold keeps its replacement absent from the texture map.
    pub fn wait_texture_stream_fallback_evidence(
        &mut self,
        texture_hash: u64,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_stream_fallback_state(texture_hash)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-fallback-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = texture_hash;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-fallback-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Read the most recent completed post-VI swapchain render target.
    pub fn presented_pixels(&mut self) -> Result<Rt64PresentedPixels, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exact source texture and framebuffer identity bound by RT64's
    /// most recently completed VI draw.
    pub fn present_selection(&mut self) -> Result<Rt64PresentSelection, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present_selection()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-selection",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-selection",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm the next raw-DPC workload for a worker-excluded pre-submission
    /// snapshot. This evidence control is bounded to one completed workload.
    pub fn enable_deferred_workload_capture_for_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_deferred_workload_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the captured pre-submission workload and its current paused-replay
    /// image after both RT64 queue workers become idle.
    pub fn deferred_workload_evidence(
        &mut self,
    ) -> Result<Rt64DeferredWorkloadEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .deferred_workload_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exclusive GPU-tile-copy or CPU synchronization fallback route
    /// taken by the captured completed workload.
    pub fn framebuffer_copy_path_evidence(
        &mut self,
    ) -> Result<Rt64FramebufferCopyPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .framebuffer_copy_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-framebuffer-copy-path-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-framebuffer-copy-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read downstream texture-route vectors for the captured S2DEX workload.
    pub fn s2dex_fast_path_evidence(&mut self) -> Result<Rt64S2dexFastPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .s2dex_fast_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-s2dex-fast-path-evidence",
                    reason,
                })
        }
        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-s2dex-fast-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm pass-through typed evidence for exactly the next recognized-HLE
    /// task. This does not admit microcode or enable Extended GBI itself.
    pub fn enable_extended_gbi_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the semantic Extended-GBI, aspect, vertex-Z, and generated-frame
    /// evidence bound to the explicitly armed completed workload.
    pub fn extended_gbi_evidence(&mut self) -> Result<Rt64ExtendedGbiEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read every ordered post-VI image retained while the current Extended
    /// evidence interval was armed. Semantic evidence must be read first so
    /// the workload/present/fraction provenance has reached queue idle.
    pub fn extended_presented_pixels(
        &mut self,
    ) -> Result<Vec<Rt64ExtendedPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm exactly one bounded HFR presentation history.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored F3DEX2 display list for HFR evidence.
    ///
    /// Production [`RenderBackend::process_task`] recognition is deliberately
    /// unchanged; this method substitutes only the test fixture's microcode
    /// hash admission and then runs RT64's normal HLE/workload/render path.
    #[cfg(feature = "synthetic-f3dex2-evidence")]
    pub fn process_synthetic_hfr_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_hfr_f3dex2(
                    rdram,
                    display_list,
                    output_addr,
                    original_refresh_rate,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-hfr-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr, original_refresh_rate);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-hfr-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a hand-authored F3DEX2 display list without an Extended GBI
    /// refresh override and return the completed workload's inferred rate.
    ///
    /// This non-default evidence seam does not alter production microcode
    /// admission. Callers must drive ordinary VI events between submissions
    /// so RT64 can accumulate the stable-factor history used by FullSync.
    #[cfg(feature = "region-rate-evidence")]
    pub fn process_synthetic_region_rate_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<Rt64RegionRateEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_region_rate_f3dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-region-rate-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-region-rate-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored public S2DEX2 display list.
    ///
    /// This non-default evidence seam substitutes only the fixture's GBI
    /// dialect. Normal [`RenderBackend::process_task`] recognition continues
    /// to require an exact supported microcode identity.
    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub fn process_synthetic_s2dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_s2dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-s2dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-s2dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a hand-authored, non-ROM F3DEX2 display list through RT64's
    /// normal interpreter/workload/render path for Extended-GBI evidence.
    ///
    /// This non-default test seam substitutes the fixture's GBI dialect only.
    /// Production [`RenderBackend::process_task`] still requires RT64 to
    /// recognize the submitted microcode text/data pair by hash.
    #[cfg(feature = "extended-gbi-evidence")]
    pub fn process_synthetic_extended_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_extended_f3dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-extended-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-extended-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Finalize and read the causal HFR workload/presentation state.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_evidence(&mut self) -> Result<Rt64HfrEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the ordered post-VI images after [`Self::hfr_evidence`] finalizes
    /// the associated workload and interpolation fractions.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_presented_pixels(&mut self) -> Result<Vec<Rt64HfrPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Start a bounded observation window at RT64's actual present-call seam.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_pacing_evidence(&mut self) -> Result<(), RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .enable_hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Join both RT64 queues and finalize actual present-call timing evidence.
    ///
    /// This observes post-sleep call cadence, not physical display scanout.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_pacing_evidence(&mut self) -> Result<Rt64HfrPacingEvidence, RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Set the backend-independent debugger pause and render boundary used by
    /// pinned RT64's paused replay path.
    ///
    /// This is a deterministic host evidence seam, not a claim that RT64's
    /// ImGui Inspector frontend supports Metal.
    pub fn set_debugger_inspection_for_evidence(
        &mut self,
        paused: bool,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_debugger_inspection_for_evidence(
                    paused,
                    framebuffer_index,
                    draw_call_index,
                    framebuffer_depth,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-debugger-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (
                paused,
                framebuffer_index,
                draw_call_index,
                framebuffer_depth,
            );
            Err(RenderError::Backend {
                backend: "rt64-debugger-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for all eight pinned raster ubershader pipelines, force the
    /// backend's ubershader-only selection path, and begin exact Metal PSO
    /// construction-event evidence.
    pub fn enable_ubershader_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read construction counters and the exact precreated ubershader pipeline
    /// selected for every raster call in the most recently completed workload.
    pub fn ubershader_evidence(&mut self) -> Result<Rt64UbershaderEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Admit one exact task-entry F3DEX2 text image for RT64 HLE. Unknown
    /// images return `NeedsLle` without crossing the C ABI.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB task-entry image, retaining only its
    /// SHA-256 identity. This mirrors the reference backend's fixture setup.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
        self
    }

    /// Admit one exact complete microcode text/data identity for runtime
    /// recognition evidence. This is separate from HLE text admission.
    pub fn with_microcode_pair_sha256(
        mut self,
        family: UcodeId,
        text_sha256: [u8; 32],
        data_bytes: u32,
        data_sha256: [u8; 32],
    ) -> Self {
        self.microcode_pairs.admit(
            family,
            text_sha256,
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: data_sha256,
            },
        );
        self
    }

    /// Byte-backed fixture convenience for [`Self::with_microcode_pair_sha256`].
    pub fn with_microcode_pair(mut self, family: UcodeId, text: &[u8], data: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "microcode pair admission requires one complete 4 KiB IMEM image"
        );
        let data_bytes = u32::try_from(data.len())
            .expect("microcode pair data length exceeds the OSTask u32 size field");
        self.microcode_pairs.admit(
            family,
            sha2::Sha256::digest(text).into(),
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: sha2::Sha256::digest(data).into(),
            },
        );
        self
    }

    /// Compatibility helper for exact F3DEX2 runtime-recognition identity.
    pub fn with_f3dex2_ucode_pair_sha256(
        self,
        text_sha256: [u8; 32],
        data_bytes: u32,
        data_sha256: [u8; 32],
    ) -> Self {
        self.with_microcode_pair_sha256(UcodeId::F3dex2, text_sha256, data_bytes, data_sha256)
    }

    /// Compatibility helper for exact F3DEX2 runtime-recognition bytes.
    pub fn with_f3dex2_ucode_pair(self, text: &[u8], data: &[u8]) -> Self {
        self.with_microcode_pair(UcodeId::F3dex2, text, data)
    }

    /// Stage a complete settings image for the next backend creation.
    pub fn with_runtime_settings(mut self, settings: RenderRuntimeSettings) -> Self {
        self.configured_settings = settings;
        self
    }

    pub fn configured_settings(&self) -> &RenderRuntimeSettings {
        &self.configured_settings
    }

    pub fn active_settings(&self) -> Option<&RenderRuntimeSettings> {
        self.active_settings.as_ref()
    }

    pub fn with_enhancement_settings(mut self, settings: RenderEnhancementSettings) -> Self {
        self.configured_enhancement_settings = settings;
        self
    }

    pub fn with_emulator_settings(mut self, settings: RenderEmulatorSettings) -> Self {
        self.configured_emulator_settings = settings;
        self
    }

    pub fn with_runtime_policy(mut self, policy: RenderRuntimePolicy) -> Self {
        assert!(
            policy.replacement.packs.is_empty(),
            "with_runtime_policy cannot reconstruct replacement-pack host paths from byte identities; call load_replacement_packs before create"
        );
        self.configured_settings = policy.user;
        self.configured_enhancement_settings = policy.enhancement;
        self.configured_emulator_settings = policy.emulator;
        self.configured_replacement_packs.clear();
        self.configured_replacement_enabled = policy.replacement.enabled;
        self
    }

    pub fn configured_enhancement_settings(&self) -> &RenderEnhancementSettings {
        &self.configured_enhancement_settings
    }

    pub fn active_enhancement_settings(&self) -> Option<&RenderEnhancementSettings> {
        self.active_enhancement_settings.as_ref()
    }

    pub fn configured_emulator_settings(&self) -> &RenderEmulatorSettings {
        &self.configured_emulator_settings
    }

    pub fn active_emulator_settings(&self) -> Option<&RenderEmulatorSettings> {
        self.active_emulator_settings.as_ref()
    }

    pub fn configured_replacement_settings(&self) -> RenderReplacementSettings {
        RenderReplacementSettings {
            enabled: self.configured_replacement_enabled,
            packs: self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.identity.clone())
                .collect(),
        }
    }

    pub fn active_replacement_settings(&self) -> Option<&RenderReplacementSettings> {
        self.active_replacement_settings.as_ref()
    }

    pub fn configured_runtime_policy(&self) -> RenderRuntimePolicy {
        RenderRuntimePolicy {
            user: self.configured_settings.clone(),
            enhancement: self.configured_enhancement_settings.clone(),
            emulator: self.configured_emulator_settings.clone(),
            replacement: self.configured_replacement_settings(),
        }
    }

    pub fn active_runtime_policy(&self) -> Option<RenderRuntimePolicy> {
        Some(RenderRuntimePolicy {
            user: self.active_settings.as_ref()?.clone(),
            enhancement: self.active_enhancement_settings.as_ref()?.clone(),
            emulator: self.active_emulator_settings.as_ref()?.clone(),
            replacement: self.active_replacement_settings.as_ref()?.clone(),
        })
    }

    /// Inspect and stage ordered replacement packs, or transactionally load
    /// them into an existing RT64 context. Only a stable pre/load/post byte
    /// identity becomes active release policy.
    pub fn load_replacement_packs(
        &mut self,
        inputs: &[Rt64ReplacementPackInput],
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let resolved =
                resolve_replacement_packs(inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-inspect",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            self.configured_replacement_enabled = enabled;
            let configured_policy_sha = self.configured_runtime_policy().sha256();
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: configured_policy_sha,
                });
            };
            let ffi_inputs =
                replacement_ffi_inputs(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                })?;
            if let Err(reason) = context.load_replacement_packs(&ffi_inputs, enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                });
            }
            let after = resolve_replacement_packs(inputs).map_err(|reason| {
                self.active_replacement_settings = None;
                RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                }
            })?;
            if after != resolved {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason: "replacement-pack bytes changed while RT64 activated them".into(),
                });
            }
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: after.into_iter().map(|pack| pack.identity).collect(),
            });
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement load has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (inputs, enabled);
            Err(RenderError::Backend {
                backend: "rt64-replacement-inspect",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Re-inspect and reload the currently configured ordered pack paths.
    pub fn reload_replacement_packs(&mut self) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let enabled = self.configured_replacement_enabled;
            let resolved =
                resolve_replacement_packs(&inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            let ffi_inputs =
                replacement_ffi_inputs(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            if let Err(reason) = context.reload_replacement_packs(&ffi_inputs, enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                });
            }
            let after = resolve_replacement_packs(&inputs).map_err(|reason| {
                self.active_replacement_settings = None;
                RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                }
            })?;
            if after != resolved {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason: "replacement-pack bytes changed while RT64 reloaded them".into(),
                });
            }
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: after.into_iter().map(|pack| pack.identity).collect(),
            });
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement reload has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-replacement-reload",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    pub fn set_replacements_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_replacement_enabled = enabled;
        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.set_replacement_enabled(enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-enable",
                    reason,
                });
            }
            let active = self
                .active_replacement_settings
                .as_mut()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement enable has no active pack identity",
                ))?;
            active.enabled = enabled;
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement enable has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }
}

impl Default for Rt64Backend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "rt64", test))]
const fn graphics_api_matches_request(
    requested: RenderGraphicsApi,
    observed: ActiveRenderGraphicsApi,
) -> bool {
    matches!(requested, RenderGraphicsApi::Automatic)
        || matches!(
            (requested, observed),
            (RenderGraphicsApi::D3d12, ActiveRenderGraphicsApi::D3d12)
                | (RenderGraphicsApi::Vulkan, ActiveRenderGraphicsApi::Vulkan)
                | (RenderGraphicsApi::Metal, ActiveRenderGraphicsApi::Metal)
        )
}

#[cfg(any(feature = "rt64", test))]
const fn post_vi_api_for_graphics_api(api: ActiveRenderGraphicsApi) -> &'static str {
    match api {
        ActiveRenderGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
        ActiveRenderGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
        ActiveRenderGraphicsApi::Metal => "metal-bgra8-unorm",
    }
}

impl RenderBackend for Rt64Backend {
    fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
        #[cfg(feature = "rt64")]
        {
            let Some(tv_type) = self.active_tv_type else {
                return fn64_render::RenderBackendEvidence::Unidentified;
            };
            let Some(policy) = self.active_runtime_policy() else {
                return fn64_render::RenderBackendEvidence::Unidentified;
            };
            let Some(context) = self.context.as_ref() else {
                return fn64_render::RenderBackendEvidence::Unidentified;
            };
            let Ok(graphics_api) = context.presented_graphics_api() else {
                return fn64_render::RenderBackendEvidence::Unidentified;
            };
            if !graphics_api_matches_request(policy.user.graphics_api, graphics_api) {
                return fn64_render::RenderBackendEvidence::Unidentified;
            }
            let identity = Self::release_identity_for_api(graphics_api);
            fn64_render::RenderBackendEvidence::Rt64 {
                tv_type,
                backend_identity: identity.canonical_id(),
                source_authoritative: identity.is_source_authoritative(),
                graphics_api,
                settings_sha256: policy.sha256(),
                replacement_packs_active: policy.replacement.enabled
                    && !policy.replacement.packs.is_empty(),
            }
        }

        #[cfg(not(feature = "rt64"))]
        {
            fn64_render::RenderBackendEvidence::Unidentified
        }
    }

    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.active_tv_type = None;
        self.active_surface_size = None;
        self.last_present = None;
        self.active_settings = None;
        self.active_enhancement_settings = None;
        self.active_emulator_settings = None;
        self.active_replacement_settings = None;
        #[cfg(feature = "rt64")]
        {
            self.context = None;
            let replacement_inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let replacements =
                resolve_replacement_packs(&replacement_inputs).map_err(|reason| {
                    RenderError::Backend {
                        backend: "rt64-replacement-create",
                        reason,
                    }
                })?;
            self.configured_replacement_packs = replacements.clone();
            let mut context = ffi::Context::create(
                cfg.width,
                cfg.height,
                cfg.tv_type.nominal_field_hz(),
                &self.configured_settings,
                &self.configured_enhancement_settings,
                &self.configured_emulator_settings,
            )
            .map_err(|reason| RenderError::Backend {
                backend: "rt64",
                reason,
            })?;
            let ffi_inputs =
                replacement_ffi_inputs(&replacements).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason,
                })?;
            context
                .load_replacement_packs(&ffi_inputs, self.configured_replacement_enabled)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason,
                })?;
            let replacements_after =
                resolve_replacement_packs(&replacement_inputs).map_err(|reason| {
                    RenderError::Backend {
                        backend: "rt64-replacement-create",
                        reason,
                    }
                })?;
            if replacements_after != replacements {
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason: "replacement-pack bytes changed while RT64 created the backend".into(),
                });
            }
            self.context = Some(context);
            self.active_settings = Some(self.configured_settings.clone());
            self.active_enhancement_settings = Some(self.configured_enhancement_settings.clone());
            self.active_emulator_settings = Some(self.configured_emulator_settings.clone());
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled: self.configured_replacement_enabled,
                packs: replacements_after
                    .into_iter()
                    .map(|pack| pack.identity)
                    .collect(),
            });
            self.active_tv_type = Some(cfg.tv_type);
            self.active_surface_size = Some(ingress::ActiveSurfaceSize {
                width: cfg.width,
                height: cfg.height,
            });
            Ok(())
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = cfg;
            self.created = false;
            Err(RenderError::Backend {
                backend: "rt64",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    fn observe_non_rdp_write16(&mut self, _write: NonRdpWrite16) -> NonRdpWrite16Disposition {
        // Native RT64 does not expose its hidden-bit ownership through this
        // Rust adapter. Explicitly report that boundary; acknowledging this
        // event is not evidence of native RT64 hidden-bit parity.
        NonRdpWrite16Disposition::NoRustHiddenSidecar
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        #[cfg(feature = "rt64")]
        {
            ingress::validate_task_ingress(
                rdram.len(),
                task,
                output_addr,
                self.active_surface_size,
            )?;
            let force_branch = self
                .active_enhancement_settings
                .as_ref()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .f3dex_force_branch;
            // FullSync and ordered microcode activation are public control-flow
            // effects, not reference-renderer output. The shared walker owns
            // its speculative memories, so late LLE fallback cannot leak a
            // partial DMA or self-load into the live task state.
            let inspection = match fn64_render::inspect_geometry_task(
                rdram,
                rsp_memory,
                task,
                &self.f3dex2_ucodes,
                fn64_render::GeometryTaskInspectionPolicy { force_branch },
                Some(fn64_render::TaskAdmissionRawWindowSize {
                    text: RT64_GBI_TEXT_RECOGNITION_BYTES,
                    data: RT64_GBI_DATA_RECOGNITION_BYTES,
                }),
            ) {
                Ok(inspection) => inspection,
                Err(RenderError::RequiresLle { ucode_sha256 }) => {
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
                Err(error) => return Err(error),
            };
            let inspected_full_sync_count = inspection.full_sync_count;
            let admission_plan = Rt64TaskAdmission {
                plan: inspection.admission_plan,
                raw_windows: inspection.raw_windows,
            };
            let mut context = NativeContextLease::take(&mut self.context)
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
            let mut transaction =
                NativeTaskMemoryRollback::new(rdram, rsp_memory, &mut self.native_rdram_preimage);
            let native_call = {
                let (native_rdram, native_rsp) = transaction.memories_mut();
                context.context_mut().process_task(
                    native_rdram,
                    native_rsp,
                    task,
                    output_addr,
                    &admission_plan,
                )
            };
            let native_outcome = match native_call {
                Ok(outcome) => outcome,
                Err(reason) => {
                    // Destroy the potentially mutated native context before
                    // the rollback guard republishes the guest-memory
                    // preimage. No later call can observe either half of the
                    // rejected task.
                    drop(context);
                    drop(transaction);
                    self.invalidate_native_state();
                    return Err(RenderError::Backend {
                        backend: "rt64",
                        reason,
                    });
                }
            };
            let native_result = match native_outcome {
                ffi::NativeTaskOutcome::Complete(result) => result,
                ffi::NativeTaskOutcome::NeedsLle {
                    rejected_generation,
                    plan_sha256: _,
                } => {
                    let generation = admission_plan
                        .plan
                        .generations()
                        .get(rejected_generation as usize)
                        .expect("schema-checked native rejection index is in the admission plan");
                    let ucode_sha256 = generation.text_sha256.as_bytes();
                    if !transaction.unchanged() {
                        drop(context);
                        drop(transaction);
                        self.invalidate_native_state();
                        return Err(RenderError::Backend {
                            backend: "rt64-task-result",
                            reason: format!(
                                "native RT64 mutated guest memory during precommit NeedsLle for generation {rejected_generation}"
                            ),
                        });
                    }
                    transaction.commit();
                    context.restore();
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
            };
            let full_sync = match validate_native_full_sync_count(
                inspected_full_sync_count,
                native_result.full_sync_count,
            ) {
                Ok(full_sync) => full_sync,
                Err(reason) => {
                    drop(context);
                    drop(transaction);
                    self.invalidate_native_state();
                    return Err(RenderError::Backend {
                        backend: "rt64-task-result",
                        reason: format!(
                            "{reason}; planned microcode generations {}, ucode addresses {:#010x}/{:#010x} -> {:#010x}/{:#010x}",
                            admission_plan.plan.len(),
                            native_result.initial_ucode_addresses.0,
                            native_result.initial_ucode_addresses.1,
                            native_result.final_ucode_addresses.0,
                            native_result.final_ucode_addresses.1,
                        ),
                    });
                }
            };
            transaction.commit();
            context.restore();
            self.last_dp_full_sync = full_sync;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, rsp_memory, task, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        #[cfg(feature = "rt64")]
        {
            ingress::validate_output_target(rdram.len(), output_addr, self.active_surface_size)?;
            let start_usize = usize::try_from(start).expect("u32 RDP start fits usize");
            let end_usize = usize::try_from(end).expect("u32 RDP end fits usize");
            if start >= end
                || !start.is_multiple_of(8)
                || !end.is_multiple_of(8)
                || end_usize > rdram.len()
            {
                return Err(RenderError::InvalidTaskBounds {
                    offset: start,
                    len: end.saturating_sub(start),
                    rdram_len: rdram.len(),
                });
            }
            debug_assert!(start_usize < end_usize);
            let full_sync = fn64_render::inspect_raw_rdp_full_sync(rdram, start, end)?;
            let mut context = NativeContextLease::take(&mut self.context)
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
            let mut transaction = NativeRdramRollback::new(rdram, &mut self.native_rdram_preimage);
            if let Err(reason) = context.context_mut().process_rdp_commands(
                transaction.memory_mut(),
                start,
                end,
                output_addr,
            ) {
                drop(context);
                drop(transaction);
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64",
                    reason,
                });
            }
            transaction.commit();
            context.restore();
            self.last_dp_full_sync = full_sync;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, start, end, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.last_dp_full_sync
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        // RT64's public task entry is presently one synchronous native call;
        // the adapter cannot manufacture a resumable native stack.
        fn64_render::RenderTaskChunking::Atomic
    }

    fn present(&mut self, request: PresentRequest<'_>) -> Result<(), RenderError> {
        let (vi, memory) = request.into_parts();
        let authority = if vi.scanout.registers().is_some() {
            Rt64PresentAuthority::LiveRegisters
        } else {
            Rt64PresentAuthority::BackendOnlyCompatibility
        };
        #[cfg(feature = "rt64")]
        {
            let PresentMemory::Physical(memory) = memory else {
                return Err(RenderError::Backend {
                    backend: "rt64",
                    reason: "RT64 presentation requires current physical RDRAM authority"
                        .to_string(),
                });
            };
            // Validate only the rows selected by public coordinate arithmetic.
            // RT64 receives the complete 8 MiB device and owns its internal
            // filter/bus fetch contract; the reference renderer's bounded
            // bottom halo is not evidence about that native implementation.
            if let Some(footprint) = fn64_render::programmed_vi_source_footprint(vi)? {
                footprint.validate_rdram_len(memory.len())?;
            }
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present(&memory, vi)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            self.last_present = Some(CompletedRt64Present {
                guest_cycle: vi.noise_seed,
                authority,
            });
            Ok(())
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (vi, memory, authority);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn release_capture(&mut self) -> Result<fn64_render::RenderReleaseCapture, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let completed = self.last_present.ok_or(RenderError::NotReady(
                "RT64 release capture requested before a completed VI present",
            ))?;
            let guest_cycle = completed.release_guest_cycle()?;
            let replacement_inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let replacement_enabled = self
                .active_replacement_settings
                .as_ref()
                .ok_or(RenderError::NotReady(
                    "RT64 release capture has no active replacement identity",
                ))?
                .enabled;
            let current_replacements = match resolve_replacement_packs(&replacement_inputs) {
                Ok(packs) => RenderReplacementSettings {
                    enabled: replacement_enabled,
                    packs: packs.into_iter().map(|pack| pack.identity).collect(),
                },
                Err(reason) => {
                    self.active_replacement_settings = None;
                    return Err(RenderError::Backend {
                        backend: "rt64-release-capture",
                        reason: format!(
                            "active replacement packs could not be revalidated: {reason}"
                        ),
                    });
                }
            };
            if self.active_replacement_settings.as_ref() != Some(&current_replacements) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-release-capture",
                    reason: "active replacement-pack bytes changed after activation; reload or recreate before capture".into(),
                });
            }
            let policy = self.active_runtime_policy().ok_or(RenderError::NotReady(
                "RT64 release capture has no complete active runtime policy",
            ))?;
            let settings_sha256 = policy.sha256();
            let mut pixels = self.presented_pixels()?;
            let graphics_api = pixels.graphics_api;
            if !graphics_api_matches_request(policy.user.graphics_api, graphics_api) {
                return Err(RenderError::Backend {
                    backend: "rt64-release-capture",
                    reason: format!(
                        "observed {graphics_api:?} capture backend disagrees with active {:?} request",
                        policy.user.graphics_api
                    ),
                });
            }
            let identity = Self::release_identity_for_api(graphics_api);
            let workload_id = std::num::NonZeroU64::new(pixels.workload_id).ok_or_else(|| {
                RenderError::Backend {
                    backend: "rt64-release-capture",
                    reason: "completed post-VI pixels have a zero RT64 workload ID".into(),
                }
            })?;
            let format = match pixels.format {
                Rt64PresentPixelFormat::Bgra8Unorm => {
                    fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm
                }
                Rt64PresentPixelFormat::Rgba8Unorm => {
                    for pixel in pixels.bytes.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm
                }
            };
            Ok(fn64_render::RenderReleaseCapture {
                guest_cycle,
                backend_identity: identity.canonical_id(),
                source_authoritative: identity.is_source_authoritative(),
                settings_sha256,
                width: pixels.width,
                height: pixels.height,
                row_bytes: pixels.row_bytes,
                format,
                workload_id,
                present_id: pixels.present_id,
                bytes: pixels.bytes,
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-release-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    fn apply_runtime_settings(
        &mut self,
        settings: &RenderRuntimeSettings,
    ) -> Result<RenderSettingsApply, RenderError> {
        self.configured_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderSettingsApply::StagedForCreate {
                    settings_sha256: settings.sha256(),
                });
            };
            let active = self.active_settings.as_ref().ok_or(RenderError::NotReady(
                "RT64 context exists without active runtime settings",
            ))?;
            let restart_fields = settings.restart_changes_from(active);
            if !restart_fields.is_empty() {
                return Ok(RenderSettingsApply::RestartRequired {
                    fields: restart_fields,
                    active_settings_sha256: active.sha256(),
                    requested_settings_sha256: settings.sha256(),
                });
            }
            let framebuffers_discarded = match context.apply_user_config(settings) {
                Ok(discarded) => discarded,
                Err(reason) => {
                    // An exception after RT64 begins its resource-update path
                    // cannot be rolled back transactionally. Forgetting the
                    // active identity forces recreation before any release
                    // capture can claim which configuration produced it.
                    self.active_settings = None;
                    return Err(RenderError::Backend {
                        backend: "rt64-settings",
                        reason,
                    });
                }
            };
            self.active_settings = Some(settings.clone());
            Ok(RenderSettingsApply::LiveApplied {
                settings_sha256: settings.sha256(),
                framebuffers_discarded,
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderSettingsApply::StagedForCreate {
                settings_sha256: settings.sha256(),
            })
        }
    }

    fn apply_enhancement_settings(
        &mut self,
        settings: &RenderEnhancementSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_enhancement_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.apply_enhancement_config(settings) {
                self.active_enhancement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-enhancement-settings",
                    reason,
                });
            }
            self.active_enhancement_settings = Some(settings.clone());
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 enhancement apply has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }

    fn apply_emulator_settings(
        &mut self,
        settings: &RenderEmulatorSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_emulator_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.apply_emulator_config(settings) {
                self.active_emulator_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-emulator-settings",
                    reason,
                });
            }
            self.active_emulator_settings = Some(settings.clone());
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 emulator apply has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        #[cfg(feature = "rt64")]
        if let Some(context) = &mut self.context {
            context.resize(w, h);
            self.active_surface_size = Some(ingress::ActiveSurfaceSize {
                width: w,
                height: h,
            });
        }

        #[cfg(not(feature = "rt64"))]
        let _ = (w, h);
    }

    fn identify_microcode(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        self.f3dex2_ucodes.identify_text(imem)
    }

    fn identify_microcode_pair(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        data: MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        self.microcode_pairs.identify(imem, data)
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        #[cfg(feature = "rt64")]
        {
            self.f3dex2_ucodes.supported_ucodes()
        }

        #[cfg(not(feature = "rt64"))]
        {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rt64_process_task_has_no_reference_decoder_paths() {
        let source = include_str!("lib.rs");
        let rt64_impl = source
            .find("impl RenderBackend for Rt64Backend")
            .expect("Rt64Backend RenderBackend implementation exists");
        let process_start = rt64_impl
            + source[rt64_impl..]
                .find("    fn process_task(")
                .expect("Rt64Backend process_task exists");
        let process_end = process_start
            + source[process_start..]
                .find("    fn process_rdp_commands(")
                .expect("process_rdp_commands follows process_task");
        let process_task = &source[process_start..process_end];

        assert_eq!(
            process_task
                .matches("fn64_render::inspect_geometry_task(")
                .count(),
            1,
            "RT64 production task submission must have one shared admission walk"
        );
        assert!(process_task.contains("f3dex_force_branch"));
        assert!(process_task.contains("GeometryTaskInspectionPolicy { force_branch }"));
        assert!(process_task.contains("NativeContextLease::take(&mut self.context)"));
        assert!(process_task.contains("NativeTaskMemoryRollback::new("));
        assert!(process_task.contains("&mut self.native_rdram_preimage"));
        assert!(process_task.contains("transaction.commit()"));
        assert!(
            !process_task.contains("self.context.as_mut()"),
            "native task execution must take context ownership before FFI"
        );
        for forbidden in [
            "gbi::inspect_",
            "gbi::execute_",
            "gbi::decode_",
            "gbi::trace_",
            "gbi::RenderOp",
        ] {
            assert!(
                !process_task.contains(forbidden),
                "RT64 production task submission still references {forbidden}"
            );
        }
    }

    #[test]
    fn rt64_raw_rdp_submission_owns_context_and_rdram_rollback() {
        let source = include_str!("lib.rs");
        let rt64_impl = source
            .find("impl RenderBackend for Rt64Backend")
            .expect("Rt64Backend RenderBackend implementation exists");
        let process_start = rt64_impl
            + source[rt64_impl..]
                .find("    fn process_rdp_commands(")
                .expect("Rt64Backend process_rdp_commands exists");
        let process_end = process_start
            + source[process_start..]
                .find("    fn last_dp_full_sync(")
                .expect("last_dp_full_sync follows process_rdp_commands");
        let process_rdp = &source[process_start..process_end];

        assert!(process_rdp.contains("NativeContextLease::take(&mut self.context)"));
        assert!(process_rdp.contains("NativeRdramRollback::new("));
        assert!(process_rdp.contains("&mut self.native_rdram_preimage"));
        assert!(process_rdp.contains("transaction.commit()"));
        assert!(
            !process_rdp.contains("self.context.as_mut()"),
            "raw RDP execution must take context ownership before FFI"
        );
    }

    #[test]
    fn native_full_sync_count_comparison_is_exact_not_boolean() {
        for (count, expected) in [
            (0, fn64_render::DpFullSyncStatus::NotReached),
            (1, fn64_render::DpFullSyncStatus::Reached),
            (3, fn64_render::DpFullSyncStatus::Reached),
        ] {
            assert_eq!(validate_native_full_sync_count(count, count), Ok(expected));
        }
        for (inspected, native) in [(1, 2), (2, 1)] {
            let error = validate_native_full_sync_count(inspected, native).unwrap_err();
            assert!(error.contains(&format!("executed {native} FullSync")));
            assert!(error.contains(&format!("executed {inspected}")));
        }
    }

    #[test]
    fn native_invalidation_clears_active_identity_but_keeps_recreate_configuration() {
        let mut backend = Rt64Backend::new();
        let configured_policy_sha256 = backend.configured_runtime_policy().sha256();
        backend.active_tv_type = Some(fn64_runtime::TvType::Pal);
        backend.last_present = Some(CompletedRt64Present {
            guest_cycle: 91,
            authority: Rt64PresentAuthority::LiveRegisters,
        });
        backend.active_settings = Some(RenderRuntimeSettings::default());
        backend.active_enhancement_settings = Some(RenderEnhancementSettings::default());
        backend.active_emulator_settings = Some(RenderEmulatorSettings::default());
        backend.active_replacement_settings = Some(RenderReplacementSettings::default());
        backend.last_dp_full_sync = fn64_render::DpFullSyncStatus::Reached;

        backend.clear_active_native_identity();

        assert_eq!(backend.active_tv_type, None);
        assert_eq!(backend.last_present, None);
        assert_eq!(backend.active_settings, None);
        assert_eq!(backend.active_enhancement_settings, None);
        assert_eq!(backend.active_emulator_settings, None);
        assert_eq!(backend.active_replacement_settings, None);
        assert_eq!(
            backend.last_dp_full_sync,
            fn64_render::DpFullSyncStatus::Unidentified
        );
        assert_eq!(
            backend.configured_runtime_policy().sha256(),
            configured_policy_sha256
        );
    }

    #[test]
    fn rt64_release_authority_rejects_backend_only_compatibility() {
        let compatibility = CompletedRt64Present {
            guest_cycle: 17,
            authority: Rt64PresentAuthority::BackendOnlyCompatibility,
        };
        assert!(matches!(
            compatibility.release_guest_cycle(),
            Err(RenderError::NotReady(
                "RT64 release capture requires a completed live-register VI present"
            ))
        ));
        let live = CompletedRt64Present {
            guest_cycle: 19,
            authority: Rt64PresentAuthority::LiveRegisters,
        };
        assert_eq!(live.release_guest_cycle().unwrap(), 19);
    }

    #[cfg(feature = "rt64")]
    struct SyntheticPack(PathBuf);

    #[cfg(feature = "rt64")]
    impl SyntheticPack {
        fn new(name: &str, auto_path: &str, operation: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "fn64-rt64-pack-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                name
            ));
            std::fs::create_dir(&path).expect("create synthetic replacement pack");
            let database = format!(
                "{{\"configuration\":{{\"configurationVersion\":3,\"autoPath\":\"{auto_path}\",\"defaultOperation\":\"{operation}\",\"defaultShift\":\"half\",\"hashVersion\":5}},\"textures\":[],\"operationFilters\":[],\"shiftFilters\":[],\"extraFiles\":[]}}"
            );
            std::fs::write(path.join("rt64.json"), database)
                .expect("write synthetic replacement database");
            Self(path)
        }

        fn input(&self) -> Rt64ReplacementPackInput {
            Rt64ReplacementPackInput::new(&self.0)
        }
    }

    #[cfg(feature = "rt64")]
    impl Drop for SyntheticPack {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove synthetic replacement pack");
        }
    }

    #[test]
    fn explicit_graphics_api_request_must_match_the_observed_capture_backend() {
        for (requested, observed) in [
            (RenderGraphicsApi::D3d12, ActiveRenderGraphicsApi::D3d12),
            (RenderGraphicsApi::Vulkan, ActiveRenderGraphicsApi::Vulkan),
            (RenderGraphicsApi::Metal, ActiveRenderGraphicsApi::Metal),
        ] {
            assert!(graphics_api_matches_request(requested, observed));
            for other in [
                ActiveRenderGraphicsApi::D3d12,
                ActiveRenderGraphicsApi::Vulkan,
                ActiveRenderGraphicsApi::Metal,
            ] {
                assert_eq!(
                    graphics_api_matches_request(requested, other),
                    other == observed
                );
            }
        }
    }

    #[test]
    fn release_post_vi_api_identity_is_concrete_and_api_specific() {
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::D3d12),
            "d3d12-bgra8-rgba8-unorm"
        );
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::Vulkan),
            "vulkan-bgra8-rgba8-unorm"
        );
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::Metal),
            "metal-bgra8-unorm"
        );
        for api in [
            ActiveRenderGraphicsApi::D3d12,
            ActiveRenderGraphicsApi::Vulkan,
            ActiveRenderGraphicsApi::Metal,
        ] {
            assert!(!post_vi_api_for_graphics_api(api).contains("-or-"));
        }
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn release_backend_identity_binds_the_concrete_api() {
        for (api, expected) in [
            (ActiveRenderGraphicsApi::D3d12, "d3d12-bgra8-rgba8-unorm"),
            (ActiveRenderGraphicsApi::Vulkan, "vulkan-bgra8-rgba8-unorm"),
            (ActiveRenderGraphicsApi::Metal, "metal-bgra8-unorm"),
        ] {
            let identity = Rt64Backend::release_identity_for_api(api);
            assert_eq!(identity.post_vi_api, expected);
            assert!(identity.canonical_id().contains(expected));
            assert!(!identity.canonical_id().contains("d3d12-or-vulkan"));
        }
    }

    #[test]
    fn automatic_graphics_api_evidence_accepts_only_the_observed_capture_backend() {
        for observed in [
            ActiveRenderGraphicsApi::D3d12,
            ActiveRenderGraphicsApi::Vulkan,
            ActiveRenderGraphicsApi::Metal,
        ] {
            assert!(graphics_api_matches_request(
                RenderGraphicsApi::Automatic,
                observed
            ));
        }
    }

    #[test]
    fn rt64_release_environment_requires_a_completed_observed_capture_backend() {
        let backend = Rt64Backend::new();
        assert_eq!(
            backend.release_environment(),
            fn64_render::RenderBackendEvidence::Unidentified
        );
        assert_eq!(backend.release_environment().tv_type(), None);
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_is_ordered_typed_and_staged_without_active_evidence() {
        let first = SyntheticPack::new("first", "rt64", "preload");
        let second = SyntheticPack::new("second", "rice", "stall");
        std::fs::write(first.0.join("extra.bin"), b"first-content")
            .expect("write synthetic pack content");

        let mut backend = Rt64Backend::new();
        let inputs = [first.input(), second.input()];
        let applied = backend.load_replacement_packs(&inputs, false).unwrap();
        let replacement = backend.configured_replacement_settings();
        assert!(!replacement.enabled);
        assert_eq!(replacement.packs.len(), 2);
        assert_eq!(
            replacement.packs[0].auto_path,
            fn64_render::RenderReplacementAutoPath::Rt64
        );
        assert_eq!(
            replacement.packs[0].default_operation,
            fn64_render::RenderReplacementOperation::Preload
        );
        assert_eq!(
            replacement.packs[1].auto_path,
            fn64_render::RenderReplacementAutoPath::Rice
        );
        assert_eq!(
            replacement.packs[1].default_operation,
            fn64_render::RenderReplacementOperation::Stall
        );
        assert_ne!(
            replacement.packs[0].content_sha256,
            replacement.packs[1].content_sha256
        );
        assert_ne!(
            replacement.packs[0].database_sha256,
            replacement.packs[1].database_sha256
        );
        assert_eq!(backend.active_replacement_settings(), None);
        assert_eq!(
            applied,
            RenderPolicyApply::StagedForCreate {
                policy_sha256: backend.configured_runtime_policy().sha256()
            }
        );

        let reversed = resolve_replacement_packs(&[second.input(), first.input()]).unwrap();
        let reversed_policy = RenderReplacementSettings {
            enabled: false,
            packs: reversed.into_iter().map(|pack| pack.identity).collect(),
        };
        assert_ne!(replacement.sha256(), reversed_policy.sha256());
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_rejects_ambiguous_or_silently_ignored_inputs() {
        let pack = SyntheticPack::new("duplicate", "rt64", "stream");
        let duplicate = resolve_replacement_packs(&[pack.input(), pack.input()]).unwrap_err();
        assert!(duplicate.contains("duplicated"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"hashVersion\":999}}",
        )
        .expect("write unsupported synthetic database");
        let unsupported = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(unsupported.contains("newer than pinned RT64"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"autoPath\":\"guess\"}}",
        )
        .expect("write ambiguous synthetic database");
        let ambiguous = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(ambiguous.contains("unknown autoPath"));
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_backend_without_feature_is_a_named_error_not_a_silent_success() {
        let mut backend = Rt64Backend::new();
        assert_eq!(
            backend.task_chunking(),
            fn64_render::RenderTaskChunking::Atomic
        );
        let err = backend.create(&RenderConfig::ntsc(320, 240)).unwrap_err();
        match err {
            RenderError::Backend { backend, .. } => assert_eq!(backend, "rt64"),
            other => panic!("expected Backend stub error, got {other:?}"),
        }
        assert!(!backend.created);
        assert_eq!(backend.release_environment().tv_type(), None);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    fn rt64_backend_identifies_only_exact_admitted_imem_images() {
        let admitted = [0x81; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let unadmitted = [0x82; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = Rt64Backend::new().with_f3dex2_ucode_text(&admitted);

        assert_eq!(backend.identify_microcode(&admitted), Some(UcodeId::F3dex2));
        assert_eq!(backend.identify_microcode(&unadmitted), None);
    }

    #[test]
    fn rt64_pair_recognition_requires_exact_text_data_length_and_digest() {
        let text = [0x81; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let other_text = [0x82; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x31, 0x41, 0x59, 0x26, 0x53];
        let identity = MicrocodeDataImageIdentity {
            bytes: data.len() as u32,
            sha256: sha2::Sha256::digest(data).into(),
        };
        let text_only = Rt64Backend::new().with_f3dex2_ucode_text(&text);
        assert_eq!(text_only.identify_microcode_pair(&text, identity), None);

        let backend = text_only.with_f3dex2_ucode_pair(&text, &data);
        assert_eq!(
            backend.identify_microcode_pair(&text, identity),
            Some(UcodeId::F3dex2)
        );
        assert_eq!(backend.identify_microcode_pair(&other_text, identity), None);
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    bytes: identity.bytes + 1,
                    ..identity
                }
            ),
            None
        );
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    sha256: [0xff; 32],
                    ..identity
                }
            ),
            None
        );
    }

    #[test]
    fn rt64_f3dzex2_pair_recognition_does_not_admit_hle() {
        let text = [0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let other_text = [0x7b; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x5a, 0x45, 0x58, 0x32];
        let identity = MicrocodeDataImageIdentity {
            bytes: data.len() as u32,
            sha256: sha2::Sha256::digest(data).into(),
        };
        let backend = Rt64Backend::new().with_microcode_pair(UcodeId::F3dzex2, &text, &data);

        assert_eq!(
            backend.identify_microcode_pair(&text, identity),
            Some(UcodeId::F3dzex2)
        );
        assert_eq!(backend.identify_microcode_pair(&other_text, identity), None);
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    sha256: [0xff; 32],
                    ..identity
                }
            ),
            None
        );
        assert_eq!(backend.identify_microcode(&text), None);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn rt64_shared_task_entry_plan_binds_native_rdram_to_admitted_live_imem() {
        const TEXT: u32 = 0x1000;
        const DATA: u32 = 0x3000;
        const DL: u32 = 0x4800;
        let text = [0x73; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x29; 8];
        let mut rdram = vec![0u8; 0x5000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT), &text);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA), &data);
            view.write_u32(fn64_runtime::RdramAddr::from_offset(DL), 0xdf00_0000);
            view.write_u32(fn64_runtime::RdramAddr::from_offset(DL + 4), 0);
        }
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &text,
            )
            .unwrap();
        let task = OsTask {
            ucode: TEXT,
            ucode_data: DATA,
            ucode_data_size: data.len() as u32,
            data_ptr: DL,
            ..OsTask::default()
        };
        let mut catalog = F3dex2UcodeCatalog::default();
        catalog.admit_text(&text);
        let inspection = fn64_render::inspect_geometry_task(
            &rdram,
            &rsp_memory,
            &task,
            &catalog,
            fn64_render::GeometryTaskInspectionPolicy::default(),
            Some(fn64_render::TaskAdmissionRawWindowSize {
                text: RT64_GBI_TEXT_RECOGNITION_BYTES,
                data: RT64_GBI_DATA_RECOGNITION_BYTES,
            }),
        )
        .unwrap();
        let plan = Rt64TaskAdmission {
            plan: inspection.admission_plan,
            raw_windows: inspection.raw_windows,
        };
        assert_eq!(plan.plan.len(), 1);
        assert_eq!(
            plan.plan.entry().source,
            fn64_render::TaskAdmissionSource::TaskEntry
        );
        assert_eq!(
            plan.plan.entry().text_sha256,
            fn64_render::UcodeDigest::from_text(&text)
        );
        let data_sha256: [u8; 32] = sha2::Sha256::digest(data).into();
        assert_eq!(plan.plan.entry().data.sha256, data_sha256);
        assert_eq!(plan.raw_windows.len(), 1);
        assert_eq!(
            plan.raw_windows[0].text,
            rdram[TEXT as usize..TEXT as usize + RT64_GBI_TEXT_RECOGNITION_BYTES]
        );

        rdram[TEXT as usize ^ 3] ^= 0xff;
        let mismatch = fn64_render::inspect_geometry_task(
            &rdram,
            &rsp_memory,
            &task,
            &catalog,
            fn64_render::GeometryTaskInspectionPolicy::default(),
            Some(fn64_render::TaskAdmissionRawWindowSize {
                text: RT64_GBI_TEXT_RECOGNITION_BYTES,
                data: RT64_GBI_DATA_RECOGNITION_BYTES,
            }),
        )
        .unwrap_err();
        assert!(matches!(
            mismatch,
            RenderError::RequiresLle { ucode_sha256 }
                if ucode_sha256 == fn64_render::UcodeDigest::from_text(&text).as_bytes()
        ));
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_settings_stage_before_create_without_claiming_an_active_image() {
        let mut backend = Rt64Backend::new();
        let settings = RenderRuntimeSettings::upstream_default();
        assert_eq!(
            backend.apply_runtime_settings(&settings).unwrap(),
            RenderSettingsApply::StagedForCreate {
                settings_sha256: settings.sha256()
            }
        );
        assert_eq!(backend.configured_settings(), &settings);
        assert_eq!(backend.active_settings(), None);

        let enhancement = RenderEnhancementSettings::upstream_default();
        let expected_policy = RenderRuntimePolicy {
            user: settings,
            enhancement: enhancement.clone(),
            emulator: RenderEmulatorSettings::default(),
            replacement: fn64_render::RenderReplacementSettings::default(),
        };
        assert_eq!(
            backend.apply_enhancement_settings(&enhancement).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        let emulator = RenderEmulatorSettings {
            post_blend_noise: false,
            ..RenderEmulatorSettings::default()
        };
        let expected_policy = RenderRuntimePolicy {
            emulator: emulator.clone(),
            ..expected_policy
        };
        assert_eq!(
            backend.apply_emulator_settings(&emulator).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        assert_eq!(backend.configured_runtime_policy(), expected_policy);
        assert_eq!(backend.active_runtime_policy(), None);
    }

    #[test]
    fn backend_identity_binds_fn64_adapter_source_sha256() {
        let baseline = Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256:
                "1111111111111111111111111111111111111111111111111111111111111111",
            source_id: "git:2222222222222222222222222222222222222222",
            source_provenance: Rt64SourceProvenance::GitClean,
            source_overlay_id: "fn64:test-overlay:v1",
            post_vi_api: "metal-bgra8-unorm",
        };
        let changed = Rt64BackendIdentity {
            adapter_source_sha256:
                "3333333333333333333333333333333333333333333333333333333333333333",
            ..baseline.clone()
        };
        assert_ne!(baseline.canonical_id(), changed.canonical_id());
        assert!(baseline
            .canonical_id()
            .contains("adapter_sha256=1111111111111111"));
    }
}
