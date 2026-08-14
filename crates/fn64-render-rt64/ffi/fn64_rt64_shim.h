#ifndef FN64_RT64_SHIM_H
#define FN64_RT64_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Fn64Rt64Context Fn64Rt64Context;

typedef struct Fn64Rt64Task {
    uint32_t task_type;
    uint32_t flags;
    uint32_t ucode_boot;
    uint32_t ucode_boot_size;
    uint32_t ucode;
    uint32_t ucode_size;
    uint32_t ucode_data;
    uint32_t ucode_data_size;
    uint32_t dram_stack;
    uint32_t dram_stack_size;
    uint32_t output_buff;
    uint32_t output_buff_size;
    uint32_t data_ptr;
    uint32_t data_size;
} Fn64Rt64Task;

enum {
    FN64_RT64_UCODE_PLAN_SCHEMA = 2,
    FN64_RT64_TASK_RESULT_SCHEMA = 2,
    FN64_RT64_UCODE_SOURCE_TASK_ENTRY = 1,
    FN64_RT64_UCODE_SOURCE_SELF_LOAD = 2,
    FN64_RT64_TASK_DISPOSITION_COMPLETE = 1,
    FN64_RT64_TASK_DISPOSITION_NEEDS_LLE = 2,
    FN64_RT64_UCODE_NO_REJECTED_GENERATION = UINT32_MAX,
    FN64_RT64_UCODE_TEXT_RECOGNITION_BYTES = 0x18D0,
    FN64_RT64_UCODE_DATA_RECOGNITION_BYTES = 0x0FC0
};

/* One immutable, ordered microcode generation. The raw windows are offsets
 * into Fn64Rt64UcodePlan::raw_pool and must have the exact recognition lengths
 * above. The logical digests retain fn64's full admitted text/data identities
 * independently of RT64's shorter recognition windows. expected_family zero
 * denotes UcodeId::Other and carries its opaque value in expected_detail.
 * Named families other than F3DZEX2 require expected_detail zero. F3DZEX2
 * requires detail 1 for NoN fifo 2.06H, 2 for 2.08I, or 3 for 2.08J. The
 * reserved array must always be zero. */
typedef struct Fn64Rt64UcodeGeneration {
    uint32_t source;
    uint32_t text_address;
    uint32_t data_address;
    uint32_t expected_family;
    uint32_t data_bytes;
    uint32_t raw_text_offset;
    uint32_t raw_text_len;
    uint32_t raw_data_offset;
    uint32_t raw_data_len;
    uint32_t expected_detail;
    uint8_t logical_text_sha256[32];
    uint8_t logical_data_sha256[32];
    uint32_t reserved[4];
} Fn64Rt64UcodeGeneration;

/* plan_sha256 is SHA-256 over the schema-2 canonical encoding documented by
 * the shim implementation: domain, little-endian scalar generation fields,
 * logical digests, zero reserved fields, little-endian raw length, then the
 * complete raw pool. Pointer values are deliberately excluded. */
typedef struct Fn64Rt64UcodePlan {
    uint32_t schema;
    uint32_t count;
    const Fn64Rt64UcodeGeneration *entries;
    const uint8_t *raw_pool;
    uint64_t raw_len;
    uint8_t plan_sha256[32];
    uint32_t reserved[4];
} Fn64Rt64UcodePlan;

/* Typed observations from one synchronous native HLE task. A workload-ID
 * advance is emitted only by pinned RT64's FullSync path. Every successful HLE
 * completion exhausts the ordered plan; a preflight recognition mismatch is a
 * successful call with NEEDS_LLE and no live interpreter mutation. */
typedef struct Fn64Rt64TaskResult {
    uint32_t schema;
    uint32_t entry_gbi_available;
    uint64_t workload_id_before;
    uint64_t workload_id_after;
    uint32_t initial_ucode_text_address;
    uint32_t initial_ucode_data_address;
    uint32_t final_ucode_text_address;
    uint32_t final_ucode_data_address;
    uint32_t disposition;
    uint32_t planned_count;
    uint32_t observed_count;
    uint32_t rejected_generation;
    uint8_t plan_sha256[32];
} Fn64Rt64TaskResult;

#if defined(__cplusplus)
static_assert(sizeof(Fn64Rt64UcodeGeneration) == 120U,
              "Fn64Rt64UcodeGeneration ABI size changed");
#if UINTPTR_MAX == UINT64_MAX
static_assert(sizeof(Fn64Rt64UcodePlan) == 80U,
              "Fn64Rt64UcodePlan ABI size changed");
#endif
static_assert(sizeof(Fn64Rt64TaskResult) == 88U,
              "Fn64Rt64TaskResult ABI size changed");
#elif defined(__STDC_VERSION__) && (__STDC_VERSION__ >= 201112L)
_Static_assert(sizeof(Fn64Rt64UcodeGeneration) == 120U,
               "Fn64Rt64UcodeGeneration ABI size changed");
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(Fn64Rt64UcodePlan) == 80U,
               "Fn64Rt64UcodePlan ABI size changed");
#endif
_Static_assert(sizeof(Fn64Rt64TaskResult) == 88U,
               "Fn64Rt64TaskResult ABI size changed");
#endif

typedef struct Fn64Rt64ViState {
    uint32_t registers[14];
    uint8_t registers_present;
    uint8_t blanked;
    uint8_t fade_enabled;
    uint8_t repeat_line;
    uint16_t fade_factor;
    uint8_t aa_mode_specified;
    uint8_t reserved;
    uint64_t noise_seed;
} Fn64Rt64ViState;

typedef struct Fn64Rt64AdapterCapture {
    Fn64Rt64Task task;
    uint32_t output_addr;
    uint32_t width;
    uint32_t height;
    uint32_t aa_mode_specified;
    uint32_t vi_filter_flags;
    uint32_t noise_seed_low;
    uint32_t noise_seed_high;
    uint32_t registers[24];
    uint32_t registers_after_submission[24];
} Fn64Rt64AdapterCapture;

enum {
    FN64_RT64_PRESENT_FORMAT_BGRA8_UNORM = 1,
    FN64_RT64_PRESENT_FORMAT_RGBA8_UNORM = 2
};

enum {
    FN64_RT64_PRESENT_GRAPHICS_API_D3D12 = 1,
    FN64_RT64_PRESENT_GRAPHICS_API_VULKAN = 2,
    FN64_RT64_PRESENT_GRAPHICS_API_METAL = 3
};

typedef struct Fn64Rt64PresentCapture {
    uint32_t width;
    uint32_t height;
    uint32_t row_bytes;
    uint32_t format;
    uint32_t graphics_api;
    uint32_t reserved;
    uint64_t byte_len;
    uint64_t present_id;
    uint64_t workload_id;
} Fn64Rt64PresentCapture;

/* Read-only identity of the render target sampled by the most recently
 * completed VI draw. `source_texture_identity` is process-local evidence. */
typedef struct Fn64Rt64PresentSelection {
    uint64_t present_id;
    uint64_t source_texture_identity;
    uint32_t target_address;
    uint32_t target_width;
    uint32_t target_height;
    uint32_t target_size;
} Fn64Rt64PresentSelection;

enum {
    FN64_RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS = 4,
    FN64_RT64_DEFERRED_MAX_DRAW_CALLS = 16
};

/* Bounded, ordered image of one RT64 deferred Workload. `content_digest`
 * excludes only queue IDs and debugger selection so paused replay can prove
 * content preservation; `identity_digest` additionally binds workload_id. */
typedef struct Fn64Rt64DeferredWorkloadSnapshot {
    uint64_t workload_id;
    uint64_t present_id;
    uint64_t submission_frame;
    uint64_t content_digest;
    uint64_t identity_digest;
    uint32_t framebuffer_pair_count;
    uint32_t projection_count;
    uint32_t game_call_count;
    uint32_t triangle_count;
    uint32_t vertex_count;
    uint32_t face_index_count;
    uint32_t rdp_param_count;
    uint32_t load_operation_count;
    int32_t selected_framebuffer_index;
    int32_t selected_draw_call_index;
    uint32_t selected_framebuffer_address;
    uint32_t paused;
    uint32_t pair_color_addresses[FN64_RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS];
    uint32_t pair_game_call_counts[FN64_RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS];
    uint32_t pair_projection_counts[FN64_RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS];
    uint32_t call_uids[FN64_RT64_DEFERRED_MAX_DRAW_CALLS];
    uint32_t call_fill_colors[FN64_RT64_DEFERRED_MAX_DRAW_CALLS];
    uint32_t call_triangle_counts[FN64_RT64_DEFERRED_MAX_DRAW_CALLS];
} Fn64Rt64DeferredWorkloadSnapshot;

typedef struct Fn64Rt64DeferredWorkloadEvidence {
    Fn64Rt64DeferredWorkloadSnapshot pre_submission;
    Fn64Rt64DeferredWorkloadSnapshot current;
} Fn64Rt64DeferredWorkloadEvidence;

enum {
    FN64_RT64_FRAMEBUFFER_COPY_PATH_GPU = 1,
    FN64_RT64_FRAMEBUFFER_COPY_PATH_CPU = 2
};

/* Exclusive mechanism evidence from one completed region-copy workload. */
typedef struct Fn64Rt64FramebufferCopyPathEvidence {
    uint64_t workload_id;
    uint64_t source_framebuffer_identity;
    uint32_t source_framebuffer_address;
    uint32_t path;
    uint32_t gpu_create_tile_copy_operation_count;
    uint32_t gpu_tile_dispatch_count;
    uint32_t cpu_rdram_tmem_upload_count;
    uint32_t raw_tmem_tile_count;
    uint32_t sync_framebuffer_pair_count;
    uint32_t reserved;
} Fn64Rt64FramebufferCopyPathEvidence;

/* Read-only completed-workload vectors for S2DEX enhancement evidence. Unlike
 * the exclusive region-copy query above, this retains exact multiplicities and
 * a digest of every ordered load descriptor. */
typedef struct Fn64Rt64S2dexFastPathEvidence {
    uint64_t workload_id;
    uint64_t source_framebuffer_identity;
    uint64_t load_operation_digest;
    uint32_t source_address;
    uint32_t source_width;
    uint32_t source_height;
    uint32_t source_size;
    uint32_t gpu_create_tile_copy_operation_count;
    uint32_t gpu_tile_dispatch_count;
    uint32_t cpu_rdram_tmem_upload_count;
    uint32_t raw_tmem_tile_count;
    uint32_t sync_framebuffer_pair_count;
    uint32_t framebuffer_pair_count;
    uint32_t valid_tile_count;
    uint32_t load_operation_count;
    uint32_t distinct_source_address_count;
    uint32_t minimum_source_address;
    uint32_t maximum_source_address;
    uint32_t base_source_load_count;
    uint32_t offset_source_load_count;
    uint32_t source_is_managed_framebuffer;
    uint32_t reserved;
} Fn64Rt64S2dexFastPathEvidence;

enum {
    FN64_RT64_EXTENDED_COMMAND_COUNT = 0x34,
    FN64_RT64_EXTENDED_MAX_RECTS = 16,
    FN64_RT64_EXTENDED_MAX_GROUPS = 16,
    FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS = 16,
    FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS = 8
};

typedef struct Fn64Rt64ExtendedRectEvidence {
    uint32_t draw_call_uid;
    uint16_t left_origin;
    uint16_t right_origin;
    int32_t left_offset;
    int32_t top_offset;
    int32_t right_offset;
    int32_t bottom_offset;
    int32_t upper_left_x;
    int32_t upper_left_y;
    int32_t lower_right_x;
    int32_t lower_right_y;
    uint32_t aspect_mode;
} Fn64Rt64ExtendedRectEvidence;

typedef struct Fn64Rt64TransformGroupEvidence {
    uint32_t group_id;
    uint8_t projection;
    uint8_t push;
    uint8_t decompose;
    uint8_t editable;
    uint8_t position_selector;
    uint8_t rotation_selector;
    uint8_t scale_selector;
    uint8_t skew_selector;
    uint8_t perspective_selector;
    uint8_t vertex_selector;
    uint8_t texcoord_selector;
    uint8_t tile_selector;
    uint8_t look_at_selector;
    uint8_t ordering;
    uint8_t aspect_mode;
    uint8_t reserved;
} Fn64Rt64TransformGroupEvidence;

enum {
    FN64_RT64_VERTEX_Z_BEGIN = 1,
    FN64_RT64_VERTEX_Z_END = 2,
    FN64_RT64_VERTEX_Z_NO_COMMAND_INDEX = UINT32_MAX
};

typedef struct Fn64Rt64VertexZEvidence {
    uint32_t marker_kind;
    uint32_t command_vertex_index;
    uint32_t resolved_source_index;
    uint32_t affected_face_index_start;
    uint32_t affected_face_index_count;
} Fn64Rt64VertexZEvidence;

typedef struct Fn64Rt64GeneratedPresentEvidence {
    uint64_t previous_workload_id;
    uint64_t current_workload_id;
    uint64_t present_id;
    uint32_t presentation_ordinal;
    uint32_t interpolation_numerator;
    uint32_t interpolation_denominator;
    uint32_t original_refresh_rate;
    uint32_t target_refresh_rate;
} Fn64Rt64GeneratedPresentEvidence;

enum {
    FN64_RT64_EXTENDED_NO_GENERATED_ORDINAL = UINT32_MAX
};

/* Metadata for one ordered post-VI image captured while Extended-GBI
 * evidence was armed. Bytes are retrieved separately by index so this ABI
 * remains bounded independently of the host output resolution. */
typedef struct Fn64Rt64ExtendedPresentCapture {
    uint64_t capture_generation;
    uint64_t workload_id;
    uint64_t present_id;
    uint32_t capture_ordinal;
    uint32_t capture_count;
    uint32_t generated_ordinal;
    uint32_t interpolation_numerator;
    uint32_t interpolation_denominator;
    uint32_t width;
    uint32_t height;
    uint32_t row_bytes;
    uint32_t format;
    uint64_t byte_len;
} Fn64Rt64ExtendedPresentCapture;

/* Bounded semantic image of one explicitly armed recognized-HLE task. The
 * pass-through dispatch probe is removed before task processing returns. */
typedef struct Fn64Rt64ExtendedGbiEvidence {
    uint64_t workload_id;
    uint64_t present_id;
    uint8_t enabled_opcode;
    uint8_t reserved0[3];
    uint32_t hook_enable_count;
    uint32_t command_counts[FN64_RT64_EXTENDED_COMMAND_COUNT];
    uint32_t has_refresh_rate;
    uint16_t refresh_rate;
    uint16_t reserved1;
    uint32_t rect_count;
    uint32_t group_count;
    uint32_t vertex_z_count;
    uint32_t generated_present_count;
    Fn64Rt64ExtendedRectEvidence rects[FN64_RT64_EXTENDED_MAX_RECTS];
    Fn64Rt64TransformGroupEvidence groups[FN64_RT64_EXTENDED_MAX_GROUPS];
    Fn64Rt64VertexZEvidence vertex_z[FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS];
    Fn64Rt64GeneratedPresentEvidence generated_presents[FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS];
} Fn64Rt64ExtendedGbiEvidence;

/* Region-rate evidence from one synthetic-only F3DEX2 workload. The fixture
 * omits Extended GBI's explicit refresh-rate command, so FullSync must derive
 * viOriginalRate from the exact VIHistory registered by context creation. */
typedef struct Fn64Rt64RegionRateEvidence {
    uint64_t workload_id;
    uint32_t configured_nominal_refresh_rate;
    uint32_t registered_nominal_refresh_rate;
    uint32_t workload_original_refresh_rate;
    uint32_t extended_refresh_override_absent;
} Fn64Rt64RegionRateEvidence;

#if defined(FN64_RT64_HFR_EVIDENCE)
/* Causal state for one explicitly armed high-frame-rate presentation burst.
 * The synthetic F3DEX2 admission function below is evidence-only; production
 * tasks continue to require exact recognized microcode through process_task. */
typedef struct Fn64Rt64HfrEvidence {
    uint64_t previous_workload_id;
    uint64_t current_workload_id;
    uint64_t present_id;
    uint64_t interpolation_framebuffer_identity;
    uint32_t interpolation_framebuffer_address;
    uint32_t original_refresh_rate;
    uint32_t target_refresh_rate;
    uint32_t presentation_count;
    uint32_t available_interpolated_target_count;
    uint32_t presented_counter_value;
    uint32_t skipped;
    uint32_t reserved;
    Fn64Rt64GeneratedPresentEvidence generated_presents[FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS];
} Fn64Rt64HfrEvidence;

enum {
    FN64_RT64_HFR_MAX_PACING_SAMPLES = 64
};

/* One paired monotonic observation bracketing the actual swapchain-present
 * call. The start is after RT64's precise sleep and optional present wait. */
typedef struct Fn64Rt64HfrPacingSample {
    uint64_t call_start_monotonic_ns;
    uint64_t call_return_monotonic_ns;
    uint64_t present_id;
    uint32_t burst_ordinal;
    uint32_t burst_count;
    uint32_t original_refresh_rate;
    uint32_t target_refresh_rate;
    uint32_t swapchain_valid;
    uint32_t reserved;
} Fn64Rt64HfrPacingSample;

typedef struct Fn64Rt64HfrPacingEvidence {
    uint32_t sample_count;
    uint32_t reserved;
    Fn64Rt64HfrPacingSample samples[FN64_RT64_HFR_MAX_PACING_SAMPLES];
} Fn64Rt64HfrPacingEvidence;
#endif

enum {
    FN64_RT64_UBERSHADER_MAX_RASTER_CALLS = 16
};

/* Exact Metal pipeline-construction events plus the ordered raster pipelines
 * selected by the most recently completed workload. Pipeline identities are
 * process-local evidence. */
typedef struct Fn64Rt64UbershaderEvidence {
    uint64_t workload_id;
    uint64_t present_id;
    uint64_t descriptor_digest;
    uint64_t pipeline_digest;
    uint64_t graphics_pipeline_construction_events;
    uint64_t background_construction_events;
    uint32_t caller_construction_events;
    uint32_t workload_construction_events;
    uint32_t present_construction_events;
    uint32_t precreated_pipeline_count;
    uint32_t raster_call_count;
    uint32_t matched_ubershader_call_count;
    uint32_t specialized_shader_count;
    uint32_t ubershaders_only;
    uint64_t shader_hashes[FN64_RT64_UBERSHADER_MAX_RASTER_CALLS];
    uint32_t pipeline_state_indices[FN64_RT64_UBERSHADER_MAX_RASTER_CALLS];
    uint64_t pipeline_identities[FN64_RT64_UBERSHADER_MAX_RASTER_CALLS];
} Fn64Rt64UbershaderEvidence;

/* Complete scalar wire image of pinned RT64 UserConfiguration. Enum values
 * follow the declaration order in rt64_user_configuration.h. */
typedef struct Fn64Rt64UserConfig {
    uint32_t graphics_api;
    uint32_t resolution;
    uint32_t display_buffering;
    uint32_t antialiasing;
    double resolution_multiplier;
    uint32_t downsample_multiplier;
    uint32_t filtering;
    uint32_t aspect_ratio;
    double aspect_target;
    uint32_t extended_aspect_ratio;
    double extended_aspect_target;
    uint32_t upscale_2d;
    uint32_t three_point_filtering;
    uint32_t refresh_rate;
    uint32_t refresh_rate_target;
    uint32_t internal_color_format;
    uint32_t hardware_resolve;
    uint32_t idle_work_active;
    uint32_t developer_mode;
} Fn64Rt64UserConfig;

typedef struct Fn64Rt64EnhancementConfig {
    uint32_t framebuffer_reinterpret_fix_uls;
    uint32_t presentation_mode;
    uint32_t remove_black_borders;
    uint32_t rect_fix_lower_right;
    uint32_t f3dex_force_branch;
    uint32_t s2dex_fix_bilerp_mismatch;
    uint32_t s2dex_framebuffer_fast_path;
    uint32_t texture_lod_scale;
} Fn64Rt64EnhancementConfig;

typedef struct Fn64Rt64EmulatorConfig {
    uint32_t post_blend_noise;
    uint32_t post_blend_noise_negative;
    uint32_t framebuffer_render_to_ram;
    uint32_t framebuffer_copy_with_gpu;
} Fn64Rt64EmulatorConfig;

/* Parsed behavior of one pinned-RT64 replacement database. */
typedef struct Fn64Rt64ReplacementDatabaseConfig {
    uint32_t auto_path;
    uint32_t default_operation;
    uint32_t default_shift;
    uint32_t configuration_version;
    uint32_t hash_version;
} Fn64Rt64ReplacementDatabaseConfig;

typedef struct Fn64Rt64ReplacementPack {
    const char *path_utf8;
    Fn64Rt64ReplacementDatabaseConfig expected_database;
} Fn64Rt64ReplacementPack;

/* Live, mutex-consistent texture-cache evidence for one TMEM hash. */
typedef struct Fn64Rt64TextureReplacementState {
    uint64_t texture_hash;
    uint64_t stream_load_count;
    uint32_t texture_count;
    uint32_t texture_known;
    uint32_t replacement_resolved;
    uint32_t replacement_installed;
    uint32_t replacement_mip_levels;
    uint32_t replacements_enabled;
    uint32_t stream_queued;
    uint32_t stream_active;
    uint32_t stream_results_pending;
    uint32_t uploads_pending;
    uint32_t resolved_paths_pending;
    uint32_t observed_resolved_not_installed;
    uint32_t stream_workers_paused;
    uint32_t stream_worker_count;
} Fn64Rt64TextureReplacementState;

#ifdef __cplusplus
static_assert(sizeof(Fn64Rt64Task) == 14 * sizeof(uint32_t));
static_assert(sizeof(Fn64Rt64ViState) == 72);
static_assert(sizeof(Fn64Rt64AdapterCapture) == 69 * sizeof(uint32_t));
static_assert(sizeof(Fn64Rt64PresentCapture) == 48);
static_assert(sizeof(Fn64Rt64PresentSelection) == 32);
static_assert(sizeof(Fn64Rt64DeferredWorkloadSnapshot) == 328);
static_assert(sizeof(Fn64Rt64DeferredWorkloadEvidence) == 656);
static_assert(sizeof(Fn64Rt64FramebufferCopyPathEvidence) == 48);
static_assert(sizeof(Fn64Rt64S2dexFastPathEvidence) == 104);
static_assert(sizeof(Fn64Rt64ExtendedRectEvidence) == 44);
static_assert(sizeof(Fn64Rt64TransformGroupEvidence) == 20);
static_assert(sizeof(Fn64Rt64VertexZEvidence) == 20);
static_assert(sizeof(Fn64Rt64GeneratedPresentEvidence) == 48);
static_assert(sizeof(Fn64Rt64ExtendedPresentCapture) == 72);
static_assert(sizeof(Fn64Rt64ExtendedGbiEvidence) == 1984);
static_assert(sizeof(Fn64Rt64UbershaderEvidence) == 400);
static_assert(sizeof(Fn64Rt64UserConfig) == 96);
static_assert(sizeof(Fn64Rt64EnhancementConfig) == 32);
static_assert(sizeof(Fn64Rt64EmulatorConfig) == 16);
static_assert(sizeof(Fn64Rt64ReplacementDatabaseConfig) == 20);
static_assert(sizeof(Fn64Rt64TextureReplacementState) == 72);
#endif

int fn64_rt64_capture_adapter_inputs(
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    uint32_t width,
    uint32_t height,
    const Fn64Rt64ViState *vi,
    Fn64Rt64AdapterCapture *capture,
    char *error,
    size_t error_capacity);

/* Device-free exact-source probe for RT64's stable-factor workload-rate
 * inference. The shim supplies the context's IPL-selected 50/60 Hz base. */
int fn64_rt64_probe_logical_rate(
    uint32_t nominal_refresh_rate,
    uint32_t factor,
    uint32_t *logical_rate,
    char *error,
    size_t error_capacity);

int fn64_rt64_roundtrip_user_config(
    const Fn64Rt64UserConfig *input,
    Fn64Rt64UserConfig *output,
    char *error,
    size_t error_capacity);

int fn64_rt64_roundtrip_enhancement_config(
    const Fn64Rt64EnhancementConfig *input,
    Fn64Rt64EnhancementConfig *output,
    char *error,
    size_t error_capacity);

int fn64_rt64_roundtrip_emulator_config(
    const Fn64Rt64EmulatorConfig *input,
    Fn64Rt64EmulatorConfig *output,
    char *error,
    size_t error_capacity);

/* Device-free strict inspection. `database_bytes` may be null only when
 * `database_capacity` is zero; `database_size` always receives the required
 * size on success. */
int fn64_rt64_inspect_replacement_pack(
    const char *path_utf8,
    Fn64Rt64ReplacementDatabaseConfig *config,
    uint8_t *database_bytes,
    size_t database_capacity,
    size_t *database_size,
    char *error,
    size_t error_capacity);

Fn64Rt64Context *fn64_rt64_create(
    uint32_t width,
    uint32_t height,
    uint32_t nominal_refresh_rate,
    const Fn64Rt64UserConfig *user_config,
    const Fn64Rt64EnhancementConfig *enhancement_config,
    const Fn64Rt64EmulatorConfig *emulator_config,
    char *error,
    size_t error_capacity);

int fn64_rt64_apply_user_config(
    Fn64Rt64Context *context,
    const Fn64Rt64UserConfig *user_config,
    uint8_t *framebuffers_discarded,
    char *error,
    size_t error_capacity);

int fn64_rt64_apply_enhancement_config(
    Fn64Rt64Context *context,
    const Fn64Rt64EnhancementConfig *enhancement_config,
    char *error,
    size_t error_capacity);

int fn64_rt64_apply_emulator_config(
    Fn64Rt64Context *context,
    const Fn64Rt64EmulatorConfig *emulator_config,
    char *error,
    size_t error_capacity);

int fn64_rt64_load_replacement_packs(
    Fn64Rt64Context *context,
    const Fn64Rt64ReplacementPack *packs,
    size_t pack_count,
    uint32_t enabled,
    char *error,
    size_t error_capacity);

int fn64_rt64_reload_replacement_packs(
    Fn64Rt64Context *context,
    const Fn64Rt64ReplacementPack *packs,
    size_t pack_count,
    uint32_t enabled,
    char *error,
    size_t error_capacity);

int fn64_rt64_set_replacement_enabled(
    Fn64Rt64Context *context,
    uint32_t enabled,
    char *error,
    size_t error_capacity);

/* A zero `texture_hash` selects the sole live TMEM texture. The wait ends on
 * RT64's cache/queue state, not an elapsed-time deadline. */
int fn64_rt64_wait_texture_replacement_state(
    Fn64Rt64Context *context,
    uint64_t texture_hash,
    uint32_t require_replacement,
    Fn64Rt64TextureReplacementState *state,
    char *error,
    size_t error_capacity);

/* Validation-only scheduling control. Pause is accepted only while RT64's
 * stream queue and workers are quiescent; resume recreates the exact worker
 * count owned by the pinned TextureCache. */
int fn64_rt64_set_stream_workers_paused(
    Fn64Rt64Context *context,
    uint32_t paused,
    char *error,
    size_t error_capacity);

/* Wait until a Stream replacement is resolved and queued but not installed,
 * with the validation worker hold active. */
int fn64_rt64_wait_stream_fallback_state(
    Fn64Rt64Context *context,
    uint64_t texture_hash,
    Fn64Rt64TextureReplacementState *state,
    char *error,
    size_t error_capacity);

int fn64_rt64_process_task(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint8_t *dmem,
    size_t dmem_len,
    uint8_t *imem,
    size_t imem_len,
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    const Fn64Rt64UcodePlan *ucode_plan,
    Fn64Rt64TaskResult *result,
    char *error,
    size_t error_capacity);

int fn64_rt64_process_rdp_commands(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t start,
    uint32_t end,
    uint32_t output_addr,
    int wait_for_completion,
    char *error,
    size_t error_capacity);

int fn64_rt64_flush_pending_workload(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_present(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    const Fn64Rt64ViState *vi,
    char *error,
    size_t error_capacity);

int fn64_rt64_enable_present_capture(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

/* Registers a per-frame draw callback that fires before present-capture's own
 * readback (if enabled) and before the frame is finalized/presented, drawing
 * into the same already-open command list RT64's present thread hands to
 * this shim -- present-capture's readback runs last so it captures whatever
 * this callback draws composited into the frame (see fn64_rt64_shim.cpp's
 * draw_hook_dispatch for the full ordering rationale: present-capture is
 * also the readback wm2000-shell's own present path uses for every frame
 * the player sees, so a UI overlay has to land before it, not after).
 * `command_list`/`framebuffer` are opaque `plume::RenderCommandList*`/
 * `plume::RenderFramebuffer*`, matching how this header keeps `plume` types
 * internal to the C++ side everywhere else -- a caller linking against the
 * same `plume` headers (as fn64-rmlui does) casts back to the real types.
 * The callback runs on RT64's present thread, not the caller's thread; it
 * must not block or call back into this shim's registration functions.
 * Calling this twice for the same `context` replaces the previously
 * registered callback/user_data rather than erroring, mirroring
 * `fn64_rt64_enable_present_capture`'s idempotent-enable behavior. */
int fn64_rt64_register_overlay_draw(
    Fn64Rt64Context *context,
    void (*callback)(void *command_list, void *framebuffer, void *user_data),
    void *user_data,
    char *error,
    size_t error_capacity);

/* Removes a callback registered via fn64_rt64_register_overlay_draw. A no-op
 * success if `context` has no registered callback. Also called automatically
 * from the context's own destruction, so an explicit call is only needed to
 * stop drawing an overlay before the context itself goes away. */
int fn64_rt64_unregister_overlay_draw(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

/* Returns the context's live `plume::RenderDevice*`, opaque as `void*` for
 * the same reason fn64_rt64_register_overlay_draw's command_list/framebuffer
 * parameters are -- a caller linking against the same `plume` headers (as
 * fn64-rmlui does, to construct its own RmlUi render-interface bridge
 * against this device) casts back to the real type. Only valid once the
 * context has completed setup (the same precondition
 * fn64_rt64_register_overlay_draw already enforces); returns NULL and sets
 * `error` otherwise. The returned pointer is owned by `context` and must
 * not be used past `context`'s destruction. */
void *fn64_rt64_get_render_device(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_present_capture(
    Fn64Rt64Context *context,
    Fn64Rt64PresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_present_selection(
    Fn64Rt64Context *context,
    Fn64Rt64PresentSelection *selection,
    char *error,
    size_t error_capacity);

/* Arm one raw-DPC submission for a worker-excluded pre-submit snapshot. */
int fn64_rt64_enable_deferred_workload_capture(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_deferred_workload_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64DeferredWorkloadEvidence *evidence,
    char *error,
    size_t error_capacity);

/* Read one exclusive GPU tile-copy or CPU synchronization fallback path from
 * the completed workload captured by deferred-workload evidence. */
int fn64_rt64_read_framebuffer_copy_path_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64FramebufferCopyPathEvidence *evidence,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_s2dex_fast_path_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64S2dexFastPathEvidence *evidence,
    char *error,
    size_t error_capacity);

/* Arm typed Extended-GBI evidence for exactly the next recognized-HLE task. */
int fn64_rt64_enable_extended_gbi_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_extended_gbi_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64ExtendedGbiEvidence *evidence,
    char *error,
    size_t error_capacity);

/* Read one integrity-bound generated/endpoint post-VI image. The metadata-
 * only form passes null bytes and zero capacity. Extended semantic evidence
 * must be read first so workload/present/fraction provenance is finalized. */
int fn64_rt64_read_extended_present_capture(
    Fn64Rt64Context *context,
    uint32_t capture_index,
    Fn64Rt64ExtendedPresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity);

#if defined(FN64_RT64_HFR_EVIDENCE)
/* Arm exactly one HFR burst and retain every post-VI image. This is separate
 * from Extended-GBI evidence so public runtime policy can be certified with
 * fully synthetic, non-ROM inputs. */
int fn64_rt64_enable_hfr_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

#endif

#if defined(FN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE)
/* Evidence-only HLE admission for a hand-authored F3DEX2 display list. It
 * exercises RT64's real state/workload/render/present path but deliberately
 * does not add a synthetic hash to production microcode recognition. */
int fn64_rt64_process_synthetic_hfr_f3dex2(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t display_list,
    uint32_t output_addr,
    uint16_t original_refresh_rate,
    Fn64Rt64RegionRateEvidence *region_rate_evidence,
    char *error,
    size_t error_capacity);
#endif

#if defined(FN64_RT64_SYNTHETIC_S2DEX_EVIDENCE)
/* Evidence-only HLE admission for a hand-authored public S2DEX/S2DEX2 display list.
 * Production task processing continues to require exact recognized microcode. */
int fn64_rt64_process_synthetic_s2dex2(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t display_list,
    uint32_t output_addr,
    uint32_t legacy_wire,
    char *error,
    size_t error_capacity);
#endif

#if defined(FN64_RT64_HFR_EVIDENCE)
int fn64_rt64_read_hfr_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64HfrEvidence *evidence,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_hfr_present_capture(
    Fn64Rt64Context *context,
    uint32_t capture_index,
    Fn64Rt64ExtendedPresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity);

/* Record actual present-call boundaries after RT64's pacing sleep. Reading
 * joins both queue workers and finalizes the bounded observation window. */
int fn64_rt64_enable_hfr_pacing_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_hfr_pacing_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64HfrPacingEvidence *evidence,
    char *error,
    size_t error_capacity);
#endif

/* Headless evidence control for the backend-independent state ordinarily
 * owned by DebuggerInspector's GUI. */
int fn64_rt64_set_debugger_inspection_for_evidence(
    Fn64Rt64Context *context,
    uint32_t paused,
    int32_t framebuffer_index,
    int32_t draw_call_index,
    uint32_t framebuffer_depth,
    char *error,
    size_t error_capacity);

/* Wait for all eight pinned raster ubershader pipelines, then arm exact Metal
 * construction-event and selected-pipeline evidence for subsequent work. */
int fn64_rt64_enable_ubershader_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

int fn64_rt64_read_ubershader_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64UbershaderEvidence *evidence,
    char *error,
    size_t error_capacity);

int fn64_rt64_resize(
    Fn64Rt64Context *context,
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity);
void fn64_rt64_destroy(Fn64Rt64Context *context);

#ifdef __cplusplus
}
#endif

#endif
