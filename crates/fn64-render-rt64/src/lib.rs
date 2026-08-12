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
use transaction::{NativeContextLease, NativeTaskMemoryRollback};

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

// The inherent impl lives in backend_impl.rs; as a child module it sees
// this file's private fields, so nothing here widened for the move.
mod backend_impl;


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
        wait_for_completion: bool,
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
            // No RDRAM rollback here, deliberately: on the only path that
            // would read `native_rdram_preimage` (the `Err` arm below), this
            // function calls `invalidate_native_state()`, which drops
            // `self.context` and tears down the whole native renderer
            // session before returning. A caller that gets `RenderError`
            // never continues against the RDRAM this transaction touched --
            // the session is gone. Restoring bytes that no live session will
            // ever read is pure cost: an 8 MiB copy_from_slice on the ONLY
            // call site this measured at 1.088ms/call RT64 FFI + 0.125ms/call
            // rollback (2026-08-10, 4,032-call sample, this route), i.e. paid
            // on every successful call to protect a failure path that
            // discards the very memory it would restore.
            if let Err(reason) = context.context_mut().process_rdp_commands_async(
                rdram,
                start,
                end,
                output_addr,
                wait_for_completion,
            ) {
                drop(context);
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64",
                    reason,
                });
            }
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

    fn raw_dpc_batch_capability(&self) -> fn64_render::RawDpcBatchCapability {
        fn64_render::RawDpcBatchCapability::Unsupported
    }

    fn process_raw_dpc_batch(
        &mut self,
        _rdram: &mut [u8],
        _batch: fn64_render::PreflightedRawDpcBatch,
        _output_addr: u32,
    ) -> Result<fn64_render::RawDpcBatchOutcome, RenderError> {
        Err(RenderError::Backend {
            backend: "rt64-raw-dpc-batch",
            reason: "RT64 raw-DPC batching requires a native separate-command-buffer seam; staged RDRAM replay is diagnostic-only and is not exposed by this backend".to_string(),
        })
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
            let context = self
                .context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?;
            // A raw-RDP submission earlier this field may have deferred its
            // GPU-completion wait (`process_rdp_commands_async`). Present is
            // the one place that unconditionally must see completed state --
            // flush before it reads anything.
            context
                .flush_pending_workload()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            context
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
mod tests;
