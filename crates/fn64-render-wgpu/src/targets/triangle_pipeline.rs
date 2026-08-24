//! Real `wgpu` vertex+fragment triangle pipeline (port card
//! `/private/tmp/fn64-rt64-gpu-pipeline-implementation-card.md`): the
//! smallest slice where the **host GPU** rasterizes a triangle, matching how
//! RT64's own D3D12/Vulkan/Metal backend works, rather than a CPU software
//! rasterizer (explicitly rejected, see the card's header).
//!
//! Modeled on `targets/raster.rs`'s `NativeRasterRenderer` shape (§2a):
//! adapter/device/queue request sequence, `BoundedErrorSink`
//! device-poisoning pattern, exact-submission-wait + callback-channel
//! completion protocol -- all reused verbatim in structure, not shared code
//! (this module owns its own device, a third device-owning path alongside
//! `device/mod.rs` and `targets/raster.rs`, matching that file's own
//! precedent of not sharing a device, port card §5).
//!
//! Restriction set (port card §3): one fixed triangle, hardcoded
//! vertex positions/colors/UVs in the test fixture (no `RawTriangle`
//! decode in this module's own fixture -- real admitted triangles decoded
//! from wire words are accepted separately via `submit_admitted_triangle`,
//! added by `2b3ed203`); opaque only (`blend: None`, no fixed-function blend state --
//! this is the actual "blend is a no-op" mechanism for this slice, not an
//! `OtherMode` bit combination. Independently verified against RT64's own
//! PSO construction (`rt64_raster_shader.cpp:311`, pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`): `renderTargetBlend[0]`
//! defaults to `RenderBlendDesc::Copy()` (fixed-function blend disabled),
//! only overridden to `blendEnabled = true` when the draw's `alphaBlend`
//! flag is set (line 334) -- this fixture never sets it, so `blend: None`
//! matches RT64's own real PSO exactly, not by analogy. Separately, this
//! fixture's `fixed_fixture_other_mode()` wire `(0, 0)` also happens to
//! decode to a `blend_color`/`blend.rs` selector combination
//! (`BlendColorInput::Combined`, cycle count 1) that reduces to `src`
//! unchanged if `blend.rs`'s runtime path were invoked -- but this slice
//! does not invoke it at all, so that coincidence is not the reason blend
//! is a no-op here); textureless (`renderFlagUsesTexture0/1` both false,
//! `tex_val0`/`tex_val1` zeroed in the fragment wrapper); no alpha compare,
//! no coverage-write variation, no decal, no backface culling, no MSAA, no
//! upscaling; smooth-shaded color only.
//!
//! Depth: a real `DepthStencilState` per draw, fixed-function GPU depth-test
//! hardware state, not fragment-shader arithmetic (port card §2b; RT64's own
//! `RasterPS` contains no ordinary Z-compare/write). `Z_CMP`/`Z_UPD`
//! pipeline-variant depth gating (production depth-slice task card,
//! `.claude-handoffs/production-depth-audit-review.md`): four
//! `wgpu::RenderPipeline`s are precreated once in `request()`, identical in
//! every respect except `depth_stencil.depth_compare`/
//! `depth_stencil.depth_write_enabled`, and one is selected per draw from
//! `OtherMode::depth_compare_enabled()`/`depth_update_enabled()` -- never
//! compiled synchronously on the draw path. This mirrors RT64's own
//! production mechanism, not an fn64 invention: RT64 precreates exactly
//! eight raster PSOs indexed by the `zCmp`/`zUpd`/`cvgAdd` axis (2x2x2) and
//! selects one per draw call (`docs/RT64-PUBLIC-FEATURE-INVENTORY.md:53`,
//! "ubershader-no-pipeline-stutter", pinned
//! `f0728a2520d5aa735886240de3fee75cc805f6d6`,
//! `rt64_raster_shader.cpp:460`); this slice takes the `zCmp`x`zUpd` subset
//! of that same axis (`cvgAdd`/coverage is out of scope, so 4 variants not
//! 8), matching the source's own upfront-precreation shape rather than
//! compiling on the draw path. The per-variant `depth_compare` selector is
//! `Less` when `Z_CMP` is set and `Always` when clear, independently
//! confirmed against RT64's own PSO construction (not by
//! `depth_strict_less.rs`'s name -- that module's own doc states it cites
//! only the public N64 Programming Manual/libultra, not RT64 -- coincidental
//! naming, not authority): `rt64_raster_shader.cpp:317` sets `depthFunction =
//! c.zCmp ? RenderComparisonFunction::LESS : RenderComparisonFunction::ALWAYS`,
//! pinned `5473732a822a4423b5696e7cb18fecc425a59875`, and
//! `RenderComparisonFunction::LESS` maps to each native backend's
//! non-inclusive less-than compare op. `Z_UPD` gates `depth_write_enabled`
//! the same shape of boolean toggle as `zCmp`'s ternary above; RT64's
//! `zUpd` bit is the sibling axis of the same cited `rt64_raster_shader.cpp`
//! eight-PSO table (`rt64_raster_shader.cpp:460`), not separately quoted
//! verbatim here because it is not a ternary expression the way `zCmp` is --
//! the eight-PSO table citation is `zUpd`'s own PSO-selection authority.
//! `(Z_CMP, Z_UPD) = (set, set)` reduces to `Less`/`true`, the prior
//! unconditional state, so existing draws are bit-identical through the new
//! selection path. Validated post-hoc against `depth_strict_less.rs`'s
//! oracle on the read-back depth buffer as a differential check on wgpu's
//! own depth-test hardware result, not as fragment-shader logic.
//!
//! Nonclaims (port card §7, extended by the production depth-slice task
//! card's nonclaims): no RT64 parity claim, no performance claim, no
//! `decode_stream` wiring -- draws arrive either from this module's own
//! fixture or from `production.rs` via `submit_admitted_triangle`, never
//! from the raw-DPC decode path itself.
//!
//! **Five stages this block previously declared absent are wired below and
//! are no longer nonclaims** (`docs/RT64-COVERAGE-AUDIT.md`; each verified
//! against the code in this file, not against the audit's summary): the
//! `SetCombine` decode (`fragment_combine_params_bytes`, `:231`), texture
//! sampling (the TMEM bytes/validity/tile-binding bindings and
//! `tmem_sample.wgsl`'s `TMEM_SAMPLE_STATUS_*` readback channel, `:165`),
//! alpha compare (`fragment_alpha_compare_params_bytes`, `:254`), coverage
//! (`fragment_coverage_params_bytes`, `:345`), and blend
//! (`fragment_blend_params_bytes`, `:541`, including the
//! framebuffer-color-reading composite path). The restriction set quoted
//! at the top of this header describes the original one-triangle fixture
//! slice and is retained as that slice's history, not as the module's
//! current surface. Stating them as absent would clear an auditor on five
//! stages that do run -- the inverse of the defect that cost 99.38% of
//! pixels, where a true nonclaim went unchecked against the ROM.
//!
//! What is genuinely still absent in those five areas: no **memory**
//! coverage read (node 2) and no sub-pixel `CoverageMask` geometry
//! (node 3) -- `fragment_coverage_params_bytes` panics by name on `Save`
//! and on the `image_read_enabled` combinations where the unsupplied
//! `memory` value could reach an output, and admits the rest only under
//! the proven `!alpha_coverage_select && force_blend` predicate; and no
//! `AlphaCompare::Dither` mode, likewise a named panic.
//!
//! Still absent and still nonclaimed: no decal, no backface culling
//! (`cull_mode: None`, `:1109`), no MSAA
//! (`wgpu::MultisampleState::default()`), no upscaling, and no
//! rasterization-algorithm claim of any
//! kind -- coverage determination routes entirely through wgpu's own
//! `TriangleList` primitive state and the host GPU's rasterizer. The
//! `Z_CMP`/`Z_UPD` pipeline-variant slice additionally makes no `DepthMode`
//! four-way dispatch claim (`mode_passes`/`depth_mode_decision`,
//! `Opaque`/`Interpenetrating`/`Translucent`/`Decal` all get plain hardware
//! `Less`/write-toggle only -- `other_mode.depth_mode()` is not read
//! anywhere in this module), no memory-Z/delta-Z/coverage-wrap/encoded-Z
//! claim, no primitive-depth-source (`SetPrimDepth`) claim, and no
//! cross-submission or memory-resident Z-buffer claim -- the transient
//! per-submission attachment and its `LoadOp::Load` cross-draw chaining
//! within one submission stay exactly as before this slice.

use core::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::device::{HeadlessBackend, NoAdapter};
use crate::shader_manifest::{
    triangle_pipeline_fragment_wgsl, COMPUTE_RASTER_RGBA16_ROUND_TRIP_ENTRY_POINT,
    COMPUTE_RASTER_RGBA16_ROUND_TRIP_WGSL, COMPUTE_TRIANGLE_COVERAGE_ENTRY_POINT,
    COMPUTE_TRIANGLE_HOT_COLOR_ENTRY_POINT, TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT,
    TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT, TRIANGLE_PIPELINE_VERTEX_WGSL,
};
use crate::state::{AlphaCompare, Color4, CoverageDestination, OtherMode, PrimColor};
use crate::tmem::{
    TileBindingParams, TmemGpuProjection, TILE_BINDING_PARAMS_BYTES, TMEM_BYTE_WORDS,
    TMEM_VALIDITY_WORDS,
};
use crate::{neutral_vertex_to_raster_vertex, CombineParams};
use fn64_render::NeutralTriangleVertex;

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);

fn compute_chain_timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_COMPUTE_CHAIN_TIMING") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_COMPUTE_CHAIN_TIMING must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => panic!("FN64_COMPUTE_CHAIN_TIMING is not valid Unicode: {error}"),
    })
}

fn compute_gpu_timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_COMPUTE_GPU_TIMING") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_COMPUTE_GPU_TIMING must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => panic!("FN64_COMPUTE_GPU_TIMING is not valid Unicode: {error}"),
    })
}

fn compute_state_table_fusion_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_COMPUTE_STATE_TABLE_FUSION") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => {
            panic!("FN64_COMPUTE_STATE_TABLE_FUSION must be exactly 0 or 1, got {value:?}")
        }
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => panic!("FN64_COMPUTE_STATE_TABLE_FUSION is not valid Unicode: {error}"),
    })
}

fn compute_chain_timing_lap(mark: &mut Option<Instant>) -> Duration {
    let Some(previous) = *mark else {
        return Duration::ZERO;
    };
    let now = Instant::now();
    *mark = Some(now);
    now.duration_since(previous)
}

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STATUS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const RASTER_PARAMS_BYTES: u64 = 32;
const COMBINE_PARAMS_BYTES: u64 = 16;
const ALPHA_COMPARE_PARAMS_BYTES: u64 = 16;
const COVERAGE_PARAMS_BYTES: u64 = 32;
const MATERIAL_PARAMS_BYTES: u64 = 48;
const TMEM_BYTES_BUFFER_SIZE: u64 = TMEM_BYTE_WORDS as u64 * 4;
const TMEM_VALIDITY_BUFFER_SIZE: u64 = TMEM_VALIDITY_WORDS as u64 * 4;
/// Binding 9's shared dummy buffer for every fixture in a submission whose
/// `reads_framebuffer_color` is false -- wgpu requires every layout entry to
/// have a bound resource even when the shader's `has_framebuffer_color`
/// uniform flag skips reading it. One word is sufficient: never indexed.
const FRAMEBUFFER_COLOR_DUMMY_BUFFER_SIZE: u64 = 4;

/// Card §6's "no tile binding was snapshotted" status
/// (`tmem_sample.wgsl`'s `TMEM_SAMPLE_STATUS_NO_TILE_BINDING`) --
/// the observable-shader-failure-status channel's own status codes,
/// mirrored here so `production.rs`'s readback check does not need to
/// reach into the WGSL source to know what a non-zero status means.
pub const TMEM_SAMPLE_STATUS_OK: u32 = 0;
pub const TMEM_SAMPLE_STATUS_NO_TILE_BINDING: u32 = 1;
pub const TMEM_SAMPLE_STATUS_INVALID_BYTE: u32 = 2;
pub const TMEM_SAMPLE_STATUS_REVERSED_EXTENT: u32 = 3;
pub const TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT: u32 = 4;
/// RETIRED, and reserved so no future status reuses the code.
///
/// This was the enabled-TLUT low-half REFUSAL, mirroring the CPU reader's
/// `PhysicalTexelReadError::EnabledCiSourceOutsideLowHalf`. The low-half
/// rule itself is real and is still enforced -- RT64's
/// `src/shaders/TextureDecoder.hlsli:162-163` (pinned port source
/// `5473732a`) confines an enabled-TLUT index source to `RDP_TMEM_MASK16`
/// (`0x7FF`) exactly as it confines RGBA32. But RT64 confines it by
/// MASKING inside `implLoadTMEM` (`:17-25`), never by refusing, so both
/// lanes now wrap: `tmem/read.rs`'s `AddressScope::LowHalf` and
/// `tmem_sample.wgsl`'s `tmem_indexed_byte_address`. Wrapping preserves
/// what the refusal protected -- an index read can still never reach the
/// palette's own half -- without refusing a frame the RDP would draw.
///
/// No shader path emits this value any more. It is kept, unused, so the
/// numbering of codes 0..4 is undisturbed and a stale readback carrying a
/// 5 is still nameable.
pub const TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF: u32 = 5;

/// One `RasterVS`-shaped vertex: RDP screen-pixel position, UV (unused by
/// this slice's textureless fragment shader, but present in the layout to
/// keep it stable for a future textured slice -- port card §3 step 1), and
/// interpolated color.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct RasterVertex {
    pub position: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl RasterVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x4,
        1 => Float32x2,
        2 => Float32x4,
    ];

    const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: core::mem::size_of::<RasterVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }

    fn to_bytes(self) -> [u8; 40] {
        let mut bytes = [0u8; 40];
        bytes[0..16].copy_from_slice(bytemuck_f32x4(self.position).as_slice());
        bytes[16..24].copy_from_slice(bytemuck_f32x2(self.uv).as_slice());
        bytes[24..40].copy_from_slice(bytemuck_f32x4(self.color).as_slice());
        bytes
    }
}

fn bytemuck_f32x4(values: [f32; 4]) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn bytemuck_f32x2(values: [f32; 2]) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Serializes the fragment shader's `FragmentCombineParams` uniform
/// (`shaders/triangle_pipeline_fragment.wgsl`): `low`/`high` at bytes 0..8
/// (the raw `SetCombine` wire split, `CombineParams::low`/`high`),
/// `texture_referenced` at bytes 8..12 (SHADE-only-triangle repair --
/// `CombineParams::references_texels_in_first_cycle`'s own doc), and bytes
/// 12..16 left zero (`reserved_zero`, matching `tmem_sample.wgsl`'s
/// `TileBindingParams::reserved_zero` convention of an explicit named pad
/// rather than an implicit one).
fn fragment_combine_params_bytes(
    combine_params: CombineParams,
) -> [u8; COMBINE_PARAMS_BYTES as usize] {
    let mut bytes = [0u8; COMBINE_PARAMS_BYTES as usize];
    bytes[0..4].copy_from_slice(&combine_params.low().to_le_bytes());
    bytes[4..8].copy_from_slice(&combine_params.high().to_le_bytes());
    let texture_referenced: u32 = combine_params.references_texels_in_first_cycle().into();
    bytes[8..12].copy_from_slice(&texture_referenced.to_le_bytes());
    bytes
}

/// Serializes the fragment shader's `FragmentAlphaCompareParams` uniform
/// (`shaders/triangle_pipeline_fragment.wgsl`, alpha-compare production card
/// §3b): `mode` at bytes 0..4 (`OtherMode::alpha_compare()`'s wire encoding,
/// guaranteed 0=`None` or 1=`Threshold` by the retrieval-time rejection of
/// `Reserved`/`Dither` -- see `raw_dpc::triangle_draw_data`'s and this
/// crate's `PlanCollector`'s own retrieval-time panics), `threshold_alpha`
/// at bytes 4..8 (`blend_color.rgba8()[3]`, i.e. `G_SETBLENDCOLOR.a`,
/// zero-extended to `u32`; `0` when `blend_color` is `None`, which only
/// happens for `mode == 0` -- the WGSL callable's `None` branch never reads
/// `threshold_alpha`, so this zero is never observed as a real threshold),
/// bytes 8..16 left zero (`_reserved_0`/`_reserved_1`, matching this file's
/// existing pad-to-16-byte-multiple convention).
fn fragment_alpha_compare_params_bytes(
    mode: AlphaCompare,
    blend_color: Color4,
) -> [u8; ALPHA_COMPARE_PARAMS_BYTES as usize] {
    let mode_wire: u32 = match mode {
        AlphaCompare::None => 0,
        AlphaCompare::Threshold => 1,
        AlphaCompare::Dither => unreachable!(
            "submit_admitted_triangle received an alpha-compare mode ({mode:?}) that must have \
             been rejected at retrieval time before reaching the pipeline"
        ),
    };
    let threshold_alpha = u32::from(blend_color.rgba8()[3]);
    let mut bytes = [0u8; ALPHA_COMPARE_PARAMS_BYTES as usize];
    bytes[0..4].copy_from_slice(&mode_wire.to_le_bytes());
    bytes[4..8].copy_from_slice(&threshold_alpha.to_le_bytes());
    bytes
}

/// Serializes the fragment shader's `FragmentCoverageParams` uniform
/// (`shaders/triangle_pipeline_fragment.wgsl`, production coverage node 1):
/// all six already-decoded `OtherMode` coverage fields, one `u32` each (bool
/// fields 0/1), matching this file's existing pad-to-16-byte-multiple
/// convention (here padded to 32 bytes: six real fields plus two reserved
/// words).
///
/// This slice is the `Full`/no-image-read subset only (production coverage
/// node 1 card §3, "retained mechanics"). `Save`
/// (`CoverageDestination::Save`) always retrieval-time-panics: this pipeline
/// has no framebuffer-read mechanism to supply a real `memory: Coverage`
/// value (node 2, a separate unresolved architectural decision), so `Save`'s
/// pass-through-memory semantics cannot be honored at all -- a silent
/// substitute would be exactly the "silent no-op that hides corruption"
/// AGENTS.md forbids. `Clamp`/`Wrap` retrieval-time-panic only when
/// `image_read_enabled` is set, mirroring `Save`'s reasoning exactly: both
/// modes' accumulation math (`coverage_result`, `coverage.rs:149-166`) reads
/// `memory` whenever `image_read_enabled` is true, and this pipeline cannot
/// supply that value honestly either. With `image_read_enabled` clear,
/// `Clamp`/`Wrap` degenerate to `pixel` (== `Coverage::FULL`, this slice's
/// only supplied `pixel` value) and pass through this function -- the exact
/// boundary `coverage_fragment_fn.wgsl`'s own header already documents.
/// Loud panic, not a fallback: mirrors the `AlphaCompareMode::Reserved`/
/// `Dither` precedent's "reject before rasterization" shape exactly, applied
/// here at the pipeline's own submission boundary since no upstream
/// retrieval-time collector exists for coverage yet (this card's scope is
/// this crate's pipeline/shader files only, not `raw_dpc`/`production.rs`).
///
/// ## The `memory`-independent `Clamp`/`Wrap` admission
///
/// `Clamp`/`Wrap` with `image_read_enabled` are admitted **only** when the
/// unknown `memory` value provably cannot reach any shader output. That is
/// not a substituted coverage value: `memory_count` stays the `0u` literal
/// `fs_main` already passes, and the admission is conditioned on the
/// accumulation's result being unobservable rather than on it being correct.
///
/// `coverage_fragment_fn`'s result reaches `FragmentOutput` by exactly two
/// routes, both read off `shaders/triangle_pipeline_fragment.wgsl`:
///
/// 1. `output.color.a = coverage.adjusted_alpha`, guarded by
///    `alpha_coverage_select != 0u` (`:283-285`). `adjusted_alpha` is the
///    only consumer of `destination`/`adjusted_coverage`, and both
///    `destination` and `adjusted_coverage` are `memory`-dependent under
///    `Clamp`/`Wrap` + image read. With `alpha_coverage_select` clear this
///    route is dead, so `destination` is computed and discarded.
/// 2. `coverage.blend_enabled`, passed to `blend_fragment_cycle_fn` /
///    `blend_fragment_memory_composite_fn` (`:326`, `:344`).
///    `blend_enabled == force_blend || (antialias_enabled && !wraps)`
///    (`coverage.rs:149`), and `wraps` is the `memory`-dependent term. With
///    `force_blend` set the disjunction short-circuits to `true` for every
///    `memory` in `0..=8`, so this route is `memory`-independent too.
///
/// `wraps` itself is otherwise unexported: the shader writes it to no output
/// and this pipeline has no `clear_on_coverage` discard (the CPU reference's
/// `set_blended` consumer, `raster/draw.rs:598`, has no counterpart here).
///
/// So the admitted predicate is `!alpha_coverage_select && force_blend`, and
/// under it the draw's every observable output is a function of the supplied
/// `pixel` alone. Anything outside it -- `alpha_coverage_select` set, or
/// `force_blend` clear (where `antialias_enabled && !wraps` makes
/// `blend_enabled` genuinely read `memory`) -- still panics, and `Save`
/// still panics unconditionally since `destination = memory` has no
/// `memory`-independent case at all.
///
/// **This is a narrowing of the refusal, not an implementation of the read.**
/// No framebuffer coverage is read, and none is invented. A draw whose
/// coverage arithmetic would actually matter is refused exactly as before.
/// Measured, not assumed: all 60 of WM2000's frame-0 texrects latch low word
/// `0x005041c8` -- `cvg_dst=Wrap`, `IM_RD`, `AA_EN`, `CLR_ON_CVG`,
/// `FORCE_BL`, with `CVG_X_ALPHA` and `ALPHA_CVG_SEL` both clear -- which
/// satisfies this predicate (`docs/RT64-WM2000-REPLAY.md` §2's capture,
/// decoded per `state.rs`'s own bit accessors).
fn fragment_coverage_params_bytes(
    coverage_destination: CoverageDestination,
    image_read_enabled: bool,
    force_blend: bool,
    antialias_enabled: bool,
    coverage_times_alpha: bool,
    alpha_coverage_select: bool,
) -> [u8; COVERAGE_PARAMS_BYTES as usize] {
    let coverage_destination_wire: u32 = match coverage_destination {
        CoverageDestination::Clamp | CoverageDestination::Wrap
            if image_read_enabled && (alpha_coverage_select || !force_blend) =>
        {
            panic!(
                "submit_admitted_triangle received coverage_destination={coverage_destination:?} \
                 with image_read_enabled=true and alpha_coverage_select={alpha_coverage_select} \
                 force_blend={force_blend}: this pipeline has no framebuffer-read mechanism to \
                 supply a real memory coverage value (node 2, out of scope), and this mode \
                 combination lets that value reach a shader output -- must be rejected before \
                 GPU submission, not silently substituted"
            )
        }
        CoverageDestination::Save => panic!(
            "submit_admitted_triangle received coverage_destination=Save: this pipeline has no \
             framebuffer-read mechanism to supply a real memory coverage value (node 2, out of \
             scope) -- must be rejected before GPU submission, not silently substituted"
        ),
        CoverageDestination::Clamp => 0,
        CoverageDestination::Wrap => 1,
        CoverageDestination::Full => 2,
    };
    let mut bytes = [0u8; COVERAGE_PARAMS_BYTES as usize];
    bytes[0..4].copy_from_slice(&coverage_destination_wire.to_le_bytes());
    bytes[4..8].copy_from_slice(&u32::from(image_read_enabled).to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::from(force_blend).to_le_bytes());
    bytes[12..16].copy_from_slice(&u32::from(antialias_enabled).to_le_bytes());
    bytes[16..20].copy_from_slice(&u32::from(coverage_times_alpha).to_le_bytes());
    bytes[20..24].copy_from_slice(&u32::from(alpha_coverage_select).to_le_bytes());
    bytes
}

/// Serializes the fragment shader's `FragmentMaterialParams` uniform
/// (`shaders/triangle_pipeline_fragment.wgsl`, production literal combiner
/// Slice B): `env_color` at bytes 0..16 and `prim_color` at bytes 16..32
/// (both `Color4::normalized()`, RGBA8/255.0 -- RT64's own `setEnvColor`/
/// `setPrimColor` normalization, `RasterPS.hlsl:169-183`), `prim_lod_frac`
/// at bytes 32..36 (`PrimColor::lod().lod_frac_normalized()`, lodFrac/256.0,
/// matching `primLOD.x` in the same RT64 assembly), bytes 36..48 left zero
/// (`_reserved_0`/`_reserved_1`/`_reserved_2`). `None` (no `SetEnvColor`/
/// `SetPrimColor` before this triangle) serializes as all-zero, matching
/// `CombinerInputs`'s pre-Slice-B hardcoded default exactly.
fn fragment_material_params_bytes(
    env_color: Color4,
    prim_color: PrimColor,
) -> [u8; MATERIAL_PARAMS_BYTES as usize] {
    let env = env_color.normalized();
    let (prim_rgba, prim_lod_frac) = (
        prim_color.color().normalized(),
        prim_color.lod().lod_frac_normalized(),
    );
    let mut bytes = [0u8; MATERIAL_PARAMS_BYTES as usize];
    bytes[0..16].copy_from_slice(&bytemuck_f32x4(env));
    bytes[16..32].copy_from_slice(&bytemuck_f32x4(prim_rgba));
    bytes[32..36].copy_from_slice(&prim_lod_frac.to_le_bytes());
    // bytes[36..48] left zero: _reserved_0/_reserved_1/_reserved_2.
    bytes
}

const BLEND_PARAMS_BYTES: u64 = 80;

/// One resolved selector's wire number, matching
/// `shaders/blend_fragment_fn.wgsl`'s header encoding exactly
/// (`0=Combined/OneMinusA, 1=Framebuffer/FramebufferAlpha, 2=Blend/One,
/// 3=Fog/Zero` for color, and the alpha table `0=Combined, 1=Fog, 2=Shade,
/// 3=Zero`). Shared by `p`/`a`/`m`/`b` since `BlendColorInput`/
/// `BlendAlphaInput`/`BlendBInput::from_wire` all decode a raw 2-bit field
/// the same way; this helper just reads back the same wire value
/// `from_wire` was built from, via each enum's own discriminant order
/// (`Clone, Copy` closed enums, no hidden reserved encoding to worry about
/// per `crate::blend`'s own module doc).
const fn blend_color_input_wire(input: crate::blend::BlendColorInput) -> u32 {
    match input {
        crate::blend::BlendColorInput::Combined => 0,
        crate::blend::BlendColorInput::Framebuffer => 1,
        crate::blend::BlendColorInput::Blend => 2,
        crate::blend::BlendColorInput::Fog => 3,
    }
}

const fn blend_alpha_input_wire(input: crate::blend::BlendAlphaInput) -> u32 {
    match input {
        crate::blend::BlendAlphaInput::Combined => 0,
        crate::blend::BlendAlphaInput::Fog => 1,
        crate::blend::BlendAlphaInput::Shade => 2,
        crate::blend::BlendAlphaInput::Zero => 3,
    }
}

const fn blend_b_input_wire(input: crate::blend::BlendBInput) -> u32 {
    match input {
        crate::blend::BlendBInput::OneMinusA => 0,
        crate::blend::BlendBInput::FramebufferAlpha => 1,
        crate::blend::BlendBInput::One => 2,
        crate::blend::BlendBInput::Zero => 3,
    }
}

/// Typed host-side resolved fragment-blend parameter object (production
/// blend wiring slice 1, card §4): carries only the admitted subset's cycle
/// count/selectors/register bytes -- no raw parallel `OtherMode` decode, no
/// framebuffer-dependent field. Built once per admitted triangle by
/// `production.rs` from `crate::blend::BlendModeState`/`ResolvedBlendCycle`
/// (the same landed selector types `crate::blend`'s own CPU characterization
/// uses, reused here rather than re-decoded), after that caller has already
/// rejected any active cycle whose selectors need a framebuffer sample (see
/// `ResolvedBlendCycle::requires_framebuffer_sample`) -- constructing this
/// type at all is proof the admitted-subset gate already passed for this
/// triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedFragmentBlendParams {
    /// `crate::blend::BlendModeState::cycle_count()`: `0` (Copy/Fill
    /// bypass), `1` (OneCycle), or `2` (TwoCycle).
    pub cycle_count: u8,
    /// Cycle 0's four selectors, meaningful only when `cycle_count >= 1`.
    pub cycle0: crate::blend::ResolvedBlendCycle,
    /// Cycle 1's four selectors, meaningful only when `cycle_count == 2`.
    pub cycle1: crate::blend::ResolvedBlendCycle,
    /// `G_SETBLENDCOLOR`, read whenever an active cycle's `P`/`M` selects
    /// [`crate::blend::BlendColorInput::Blend`]. Not an `Option`: the RDP
    /// register always holds a value, zero until the guest writes one (see
    /// `crate::state::RdpState`'s constant-color field doc).
    pub blend_color: Color4,
    /// `G_SETFOGCOLOR`, read whenever an active cycle's `P`/`M` selects
    /// [`crate::blend::BlendColorInput::Fog`] or `A` selects
    /// [`crate::blend::BlendAlphaInput::Fog`]. Same power-on-register
    /// reasoning as `blend_color`.
    pub fog_color: Color4,
    /// Framebuffer-blend Slice B: `true` when this fixture's active cycle(s)
    /// select [`crate::blend::BlendColorInput::Framebuffer`] on `P` or `M`
    /// -- computed once in `production.rs`'s `draw_admitted_triangles` from
    /// the same selectors `cycle0`/`cycle1` above already carry, not
    /// re-derived here. Never `true` together with an active cycle whose `B`
    /// selects `FramebufferAlpha` reaching this struct at all -- that
    /// combination is rejected by `production.rs` before construction (see
    /// `ResolvedBlendCycle::requires_framebuffer_alpha`).
    pub reads_framebuffer_color: bool,
}

impl ResolvedFragmentBlendParams {
    /// The Copy/Fill bypass value (`cycle_count == 0`): `blend_fragment_cycle_fn`
    /// returns `src` unchanged for this value, matching the pipeline's prior
    /// `blend: None` no-op exactly (module doc, byte-identical-output
    /// regression bar). This crate's own fixtures reuse this constant as
    /// their no-blend default, the same "regression-guard default every
    /// existing test reuses unmodified" convention already established for
    /// `coverage_destination: Full`/`image_read_enabled: false`.
    pub const NO_OP: Self = Self {
        cycle_count: 0,
        cycle0: crate::blend::ResolvedBlendCycle {
            p: crate::blend::BlendColorInput::Combined,
            a: crate::blend::BlendAlphaInput::Combined,
            m: crate::blend::BlendColorInput::Combined,
            b: crate::blend::BlendBInput::Zero,
        },
        cycle1: crate::blend::ResolvedBlendCycle {
            p: crate::blend::BlendColorInput::Combined,
            a: crate::blend::BlendAlphaInput::Combined,
            m: crate::blend::BlendColorInput::Combined,
            b: crate::blend::BlendBInput::Zero,
        },
        blend_color: Color4::from_wire(0),
        fog_color: Color4::from_wire(0),
        reads_framebuffer_color: false,
    };
}

/// Serializes the fragment shader's `FragmentBlendParams` uniform
/// (`shaders/triangle_pipeline_fragment.wgsl`): `cycle_count` at bytes 0..4,
/// cycle 0's `p/a/m/b` at bytes 4..20, cycle 1's `p/a/m/b` at bytes 20..36
/// (each wire-numbered exactly as `shaders/blend_fragment_fn.wgsl`'s header
/// documents; `cycle1`'s bytes are zero and unread by the shader when
/// `cycle_count < 2`, matching `blend_fragment_cycle_fn`'s own
/// `cycle_count == 2u` gate), `has_framebuffer_color`
/// (`params.reads_framebuffer_color` as `0`/`1`, the former `_reserved_0`)
/// at bytes 36..40, `row_stride_words` (the caller-supplied value, the
/// former `_reserved_1`) at bytes 40..44, bytes 44..48 left zero
/// (`_reserved_2`, unused by this card), `blend_color` at bytes 48..64 and
/// `fog_color` at bytes 64..80 (both `Color4::normalized()`, matching
/// `fragment_material_params_bytes`'s own normalization convention). `None`
/// (no `SetBlendColor`/`SetFogColor` before this triangle) serializes as
/// all-zero, exactly like `fragment_material_params_bytes`'s own `None`
/// handling -- this is safe here because the caller (`production.rs`) has
/// already rejected, before this function ever runs, any admitted triangle
/// whose active cycle selectors actually need the missing register (see
/// `ResolvedFragmentBlendParams`'s own doc).
///
/// `row_stride_words` must be `padded_bytes_per_row / 4` (the caller's own
/// already-computed value at snapshot-creation time in `submit_triangles`),
/// never re-derived from `params`/`fixture.extent.width` here -- the row-
/// stride correctness fix (this card) depends on there being exactly one
/// source of "width" for this value, not two that can silently disagree.
fn fragment_blend_params_bytes(
    params: ResolvedFragmentBlendParams,
    row_stride_words: u32,
) -> [u8; BLEND_PARAMS_BYTES as usize] {
    let mut bytes = [0u8; BLEND_PARAMS_BYTES as usize];
    bytes[0..4].copy_from_slice(&u32::from(params.cycle_count).to_le_bytes());
    bytes[4..8].copy_from_slice(&blend_color_input_wire(params.cycle0.p).to_le_bytes());
    bytes[8..12].copy_from_slice(&blend_alpha_input_wire(params.cycle0.a).to_le_bytes());
    bytes[12..16].copy_from_slice(&blend_color_input_wire(params.cycle0.m).to_le_bytes());
    bytes[16..20].copy_from_slice(&blend_b_input_wire(params.cycle0.b).to_le_bytes());
    bytes[20..24].copy_from_slice(&blend_color_input_wire(params.cycle1.p).to_le_bytes());
    bytes[24..28].copy_from_slice(&blend_alpha_input_wire(params.cycle1.a).to_le_bytes());
    bytes[28..32].copy_from_slice(&blend_color_input_wire(params.cycle1.m).to_le_bytes());
    bytes[32..36].copy_from_slice(&blend_b_input_wire(params.cycle1.b).to_le_bytes());
    bytes[36..40].copy_from_slice(&u32::from(params.reads_framebuffer_color).to_le_bytes());
    bytes[40..44].copy_from_slice(&row_stride_words.to_le_bytes());
    // bytes[44..48] left zero: _reserved_2.
    let blend_color = params.blend_color.normalized();
    let fog_color = params.fog_color.normalized();
    bytes[48..64].copy_from_slice(&bytemuck_f32x4(blend_color));
    bytes[64..80].copy_from_slice(&bytemuck_f32x4(fog_color));
    bytes
}

/// The snapshot resource for one framebuffer-color-dependent draw's
/// destination read (framebuffer-blend Slice B): a `STORAGE`-usage buffer
/// holding a `copy_texture_to_buffer` capture of the color attachment,
/// matching this crate's existing TMEM storage-buffer convention (bindings
/// 2/3) rather than a second bindable texture.
struct FramebufferColorSnapshot {
    buffer: wgpu::Buffer,
}

/// `RasterParams.resolution`/`screenScale`/`screenOffset`, matching the
/// vertex shader's `RasterParams` uniform (`shaders/triangle_pipeline_vertex.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleRasterParams {
    pub resolution: [f32; 2],
    pub screen_scale: [f32; 2],
    pub screen_offset: [f32; 2],
}

impl TriangleRasterParams {
    fn to_bytes(self) -> [u8; RASTER_PARAMS_BYTES as usize] {
        let mut bytes = [0u8; RASTER_PARAMS_BYTES as usize];
        bytes[0..8].copy_from_slice(&bytemuck_f32x2(self.resolution));
        bytes[8..16].copy_from_slice(&bytemuck_f32x2(self.screen_scale));
        bytes[16..24].copy_from_slice(&bytemuck_f32x2(self.screen_offset));
        // bytes[24..32] left zero: WGSL struct pads to a 16-byte multiple
        // (two trailing f32 reserved fields), matching `RasterParams`'s
        // `reserved_0`/`reserved_1`.
        bytes
    }
}

/// Serializes the vertex shader's `RasterParams` uniform with `is_rect`
/// folded into `reserved_0` (bytes 24..28) -- the seam between
/// `TriangleFixture.is_rect` and the bytes the GPU actually receives; see
/// `shaders/triangle_pipeline_vertex.wgsl`'s `is_rect` gate.
fn raster_params_bytes(
    params: TriangleRasterParams,
    is_rect: bool,
) -> [u8; RASTER_PARAMS_BYTES as usize] {
    let mut bytes = params.to_bytes();
    bytes[24..28].copy_from_slice(&u32::from(is_rect).to_le_bytes());
    // bytes[28..32] left zero: WGSL struct pads to a 16-byte multiple (one
    // trailing f32 reserved field), matching `RasterParams`'s `reserved_1`.
    bytes
}

/// Small fixed render target extent (port card §3: "propose 8x8 or 16x16").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleTargetExtent {
    pub width: u32,
    pub height: u32,
}

/// One fixed-fixture triangle submission: three vertices, the raster
/// screen-transform uniform, the fragment stage's caller-supplied literal
/// `CombineParams` (no `SetCombine` decode in this slice -- port card §3
/// step 3), and this triangle's own committed-TMEM texture-sample inputs
/// (published committed-TMEM textured-draw card §2/§3): the byte-projected
/// physical TMEM snapshot this draw samples against, and the bound tile's
/// `TileBindingParams` (or [`TileBindingParams::unbound`] when the triangle
/// carries no real tile snapshot -- e.g. the flat-colored fixed fixtures
/// this file's own tests still construct for the non-textured cases).
/// `alpha_compare_mode`/`blend_color` (alpha-compare production card §3b)
/// feed the real post-combiner `fs_main` discard gate -- `alpha_compare_mode`
/// must already be `None` or `Threshold`; `Reserved`/`Dither` are rejected
/// before a triangle ever reaches this struct (retrieval-time panic, see
/// `raw_dpc::triangle_draw_data`/`production.rs`'s `PlanCollector`).
/// `depth_compare_enabled`/`depth_update_enabled` (production depth-slice
/// task card, `Z_CMP`/`Z_UPD` pipeline-variant depth gating) are
/// `OtherMode::depth_compare_enabled()`/`depth_update_enabled()` verbatim --
/// this draw's two bits selecting one of the four precreated
/// `TrianglePipelineRenderer::pipelines` variants (see
/// [`depth_pipeline_index`]), not new arithmetic. `(true, true)` reduces to
/// the pipeline's prior sole `Less`/write-always state.
/// `coverage_destination`/`image_read_enabled`/`force_blend`/
/// `antialias_enabled`/`coverage_times_alpha`/`alpha_coverage_select`
/// (production coverage node 1) are `OtherMode`'s six coverage-related bits
/// verbatim -- `Save`, and `Clamp`/`Wrap` with `image_read_enabled` set,
/// retrieval-time-panic before this fixture ever reaches the GPU (see
/// [`fragment_coverage_params_bytes`]); this pipeline's `Full`/no-image-read
/// subset otherwise passes through unmodified.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleFixture {
    pub vertices: [RasterVertex; 3],
    pub raster_params: TriangleRasterParams,
    pub combine_params: CombineParams,
    pub extent: TriangleTargetExtent,
    pub tmem: TmemGpuProjection,
    pub tile_binding: TileBindingParams,
    pub alpha_compare_mode: AlphaCompare,
    pub blend_color: Color4,
    pub env_color: Color4,
    pub prim_color: PrimColor,
    /// Production blend wiring slice 1: the admitted-subset resolved
    /// blend-cycle parameters this triangle's real `OtherMode` decoded to.
    /// Always present (never `Option`) -- a `cycle_count == 0` value (built
    /// from `BlendModeState::cycle_count()`'s own Copy/Fill short-circuit)
    /// is a legitimate, common admitted value, not an absence.
    pub blend_params: ResolvedFragmentBlendParams,
    pub depth_compare_enabled: bool,
    pub depth_update_enabled: bool,
    pub coverage_destination: CoverageDestination,
    pub image_read_enabled: bool,
    pub force_blend: bool,
    pub antialias_enabled: bool,
    pub coverage_times_alpha: bool,
    pub alpha_coverage_select: bool,
    /// `true` for a `TextureRectangle`/`TextureRectangleFlip`-sourced
    /// triangle; gates the vertex shader's RDP-screen-to-NDC transform off
    /// (see `shaders/triangle_pipeline_vertex.wgsl`'s `is_rect` gate) since
    /// a rectangle's six vertices are already fixed NDC corners.
    pub is_rect: bool,
}

/// Converts one admitted raw-DPC triangle draw's own arguments (vertices,
/// `OtherMode`, combine/tile/tmem state, per-draw viewport-derived raster
/// params) into a [`TriangleFixture`], applying the same field-by-field
/// mapping [`TrianglePipelineRenderer::submit_admitted_triangle`] used to
/// build its single fixture before delegating to [`Self::submit_triangle`]
/// -- the sole conversion this crate uses both for that one-triangle path
/// and for `production.rs`'s `draw_admitted_triangles`, which must collect
/// one [`TriangleFixture`] per draw into a `Vec` and submit them all through
/// one [`TrianglePipelineRenderer::submit_triangles`] call so a multi-
/// triangle primitive (e.g. a `TextureRectangle`'s two triangles) lands in
/// one shared render pass instead of each triangle re-clearing the target.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admitted_triangle_fixture(
    vertices: [NeutralTriangleVertex; 3],
    other_mode: OtherMode,
    combine_params: CombineParams,
    raster_params: TriangleRasterParams,
    extent: TriangleTargetExtent,
    tmem: TmemGpuProjection,
    tile_binding: TileBindingParams,
    blend_color: Color4,
    env_color: Color4,
    prim_color: PrimColor,
    blend_params: ResolvedFragmentBlendParams,
    is_rect: bool,
) -> TriangleFixture {
    let alpha_compare_mode = match other_mode.alpha_compare() {
        mode @ (AlphaCompare::None | AlphaCompare::Threshold) => mode,
        unsupported @ AlphaCompare::Dither => unreachable!(
            "admitted_triangle_fixture received alpha-compare mode {unsupported:?}, which must \
             have been rejected at retrieval time before reaching the pipeline"
        ),
    };
    TriangleFixture {
        vertices: vertices.map(neutral_vertex_to_raster_vertex),
        raster_params,
        combine_params,
        extent,
        tmem,
        tile_binding,
        alpha_compare_mode,
        blend_color,
        env_color,
        prim_color,
        blend_params,
        depth_compare_enabled: other_mode.depth_compare_enabled(),
        depth_update_enabled: other_mode.depth_update_enabled(),
        coverage_destination: other_mode.coverage_destination(),
        image_read_enabled: other_mode.image_read_enabled(),
        force_blend: other_mode.force_blend(),
        antialias_enabled: other_mode.antialias_enabled(),
        coverage_times_alpha: other_mode.coverage_times_alpha(),
        alpha_coverage_select: other_mode.alpha_coverage_select(),
        is_rect,
    }
}

/// Maps a draw's `(Z_CMP, Z_UPD)` enable bits
/// (`OtherMode::depth_compare_enabled()`/`depth_update_enabled()`) to the
/// index of its precreated `TrianglePipelineRenderer::pipelines` variant.
/// Exact four-row truth table (production depth-slice task card §3):
///
/// | `Z_CMP` | `Z_UPD` | index | `depth_compare` | `depth_write_enabled` |
/// |---|---|---|---|---|
/// | set | set | 0 | `Less` | `true` |
/// | set | clear | 1 | `Less` | `false` |
/// | clear | set | 2 | `Always` | `true` |
/// | clear | clear | 3 | `Always` | `false` |
///
/// Index 0 (`(true, true)`) is the pipeline's prior sole state, so existing
/// callers that always set both bits select the same `Less`/write-always
/// behavior as before this slice, bit-identical.
pub(crate) const fn depth_pipeline_index(
    depth_compare_enabled: bool,
    depth_update_enabled: bool,
) -> usize {
    match (depth_compare_enabled, depth_update_enabled) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    }
}

/// The four `(depth_compare, depth_write_enabled)` pairs `request()` builds
/// its precreated pipelines from, indexed identically to
/// [`depth_pipeline_index`] -- one source of truth for both pipeline
/// construction and draw-time selection so the two cannot drift apart.
const DEPTH_STENCIL_VARIANTS: [(wgpu::CompareFunction, bool); 4] = [
    (wgpu::CompareFunction::Less, true),
    (wgpu::CompareFunction::Less, false),
    (wgpu::CompareFunction::Always, true),
    (wgpu::CompareFunction::Always, false),
];

pub enum TrianglePipelineDeviceOutcome {
    Ready(Box<TrianglePipelineRenderer>),
    NoAdapter(NoAdapter),
}

pub struct UninitializedTrianglePipeline {
    backend: HeadlessBackend,
}

impl UninitializedTrianglePipeline {
    pub const fn new(backend: HeadlessBackend) -> Self {
        Self { backend }
    }

    pub async fn request(self) -> Result<TrianglePipelineDeviceOutcome, TrianglePipelineError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: self.backend.wgpu_backends(),
            flags: wgpu::InstanceFlags::VALIDATION,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                return Ok(TrianglePipelineDeviceOutcome::NoAdapter(NoAdapter::new(
                    self.backend,
                )));
            }
            Err(error) => return Err(TrianglePipelineError::RequestAdapter(error.to_string())),
        };
        crate::device::adapter_selection::assert_expected_adapter(&adapter);
        let required_features = if compute_gpu_timing_enabled() {
            let feature = wgpu::Features::TIMESTAMP_QUERY;
            if !adapter.features().contains(feature) {
                return Err(TrianglePipelineError::TimestampQueryUnsupported {
                    adapter: adapter.get_info().name,
                });
            }
            feature
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-triangle-pipeline"),
                required_features,
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| TrianglePipelineError::RequestDevice(error.to_string()))?;

        let errors = Arc::new(BoundedErrorSink::default());
        let uncaptured = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error| {
            uncaptured.record(error.to_string());
        }));

        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-triangle-pipeline-vertex"),
            source: wgpu::ShaderSource::Wgsl(TRIANGLE_PIPELINE_VERTEX_WGSL.into()),
        });
        let fragment_source = triangle_pipeline_fragment_wgsl();
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-triangle-pipeline-fragment"),
            source: wgpu::ShaderSource::Wgsl(fragment_source.into()),
        });
        let compute_raster_round_trip_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fn64-compute-raster-rgba16-round-trip"),
                source: wgpu::ShaderSource::Wgsl(COMPUTE_RASTER_RGBA16_ROUND_TRIP_WGSL.into()),
            });
        let compute_raster_round_trip_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fn64-compute-raster-rgba16-round-trip"),
                layout: None,
                module: &compute_raster_round_trip_shader,
                entry_point: Some(COMPUTE_RASTER_RGBA16_ROUND_TRIP_ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let compute_triangle_coverage_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fn64-compute-triangle-coverage"),
                source: wgpu::ShaderSource::Wgsl(
                    crate::shader_manifest::compute_triangle_color_wgsl().into(),
                ),
            });
        let compute_triangle_coverage_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fn64-compute-triangle-coverage"),
                layout: None,
                module: &compute_triangle_coverage_shader,
                entry_point: Some(COMPUTE_TRIANGLE_COVERAGE_ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let compute_triangle_color_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fn64-compute-triangle-hot-color"),
                source: wgpu::ShaderSource::Wgsl(
                    crate::shader_manifest::compute_triangle_color_wgsl().into(),
                ),
            });
        let compute_triangle_color_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fn64-compute-triangle-hot-color"),
                layout: None,
                module: &compute_triangle_color_shader,
                entry_point: Some(COMPUTE_TRIANGLE_HOT_COLOR_ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        let raster_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-raster-params"),
            size: RASTER_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let combine_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-combine-params"),
            size: COMBINE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tmem_bytes_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-tmem-bytes"),
            size: TMEM_BYTES_BUFFER_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tmem_validity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-tmem-validity"),
            size: TMEM_VALIDITY_BUFFER_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tile_binding_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-tile-binding"),
            size: TILE_BINDING_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let alpha_compare_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-alpha-compare-params"),
            size: ALPHA_COMPARE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let coverage_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-coverage-params"),
            size: COVERAGE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let material_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-material-params"),
            size: MATERIAL_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blend_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-blend-params"),
            size: BLEND_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let framebuffer_color_dummy_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-framebuffer-color-dummy"),
            size: FRAMEBUFFER_COLOR_DUMMY_BUFFER_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fn64-triangle-pipeline-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Published committed-TMEM textured-draw card §2/§3
                // (Option B, mandatory): the committed TMEM byte image and
                // its parallel validity bitmap, read-only storage buffers
                // (this crate's own existing `direct_texel_decode.wgsl`/
                // `three_nearest_filter.wgsl` convention -- not
                // `texture_2d`/`sampler`, which Option A would have used
                // and which this card explicitly rejects), plus the bound
                // tile's `TileBindingParams` uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Alpha-compare production card §3b: `FragmentAlphaCompareParams`,
                // the real post-combiner discard gate's mode/threshold uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Production coverage node 1: `FragmentCoverageParams`, the
                // real per-fragment `cvg_dst`/coverage-alpha uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Production literal combiner Slice B: `FragmentMaterialParams`,
                // the real per-triangle PRIMITIVE/ENVIRONMENT/PRIM_LOD_FRAC
                // combiner-input uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Production blend wiring slice 1: `FragmentBlendParams`,
                // the admitted-subset resolved blend-cycle uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Framebuffer-blend Slice B: the per-draw destination-color
                // snapshot, read-only storage (this crate's TMEM
                // storage-buffer convention, bindings 2/3). A fixture whose
                // `reads_framebuffer_color` is false still binds SOME buffer
                // here (wgpu requires every layout entry to have a bound
                // resource) -- a single always-allocated dummy buffer shared
                // across every non-reading fixture in a submission.
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // Prewarm-only bind group: exercised once here so a bind-group-layout
        // mismatch surfaces at `request()` time (the `errors.count()` check
        // below), not on the first real `submit_triangles` call. Each real
        // draw builds its own bind group over its own per-draw uniform
        // buffers (see `submit_triangles`) -- this prewarm instance and its
        // buffers are not retained on the renderer.
        let prewarm_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-triangle-pipeline-prewarm-bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: raster_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: combine_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: tmem_bytes_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tmem_validity_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tile_binding_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: alpha_compare_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: coverage_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: material_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: blend_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: framebuffer_color_dummy_buffer.as_entire_binding(),
                },
            ],
        });
        let _ = &prewarm_bind_group;
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fn64-triangle-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Four precreated `Z_CMP`x`Z_UPD` pipeline variants (production
        // depth-slice task card §3/RT64's own eight-PSO ubershader
        // precedent, module doc above): identical vertex/fragment modules,
        // identical `primitive`/`multisample`/`targets` state across all
        // four, varying only `depth_stencil.depth_compare`/
        // `depth_write_enabled` per [`DEPTH_STENCIL_VARIANTS`] -- matching
        // RT64's own upfront-precreation shape, nothing compiled on the draw
        // path. Index order matches [`depth_pipeline_index`] exactly.
        let pipelines = DEPTH_STENCIL_VARIANTS.map(|(depth_compare, depth_write_enabled)| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("fn64-triangle-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vertex_shader,
                    entry_point: Some(TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(RasterVertex::layout())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write_enabled),
                    depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &fragment_shader,
                    entry_point: Some(TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    // Second attachment (card audit repair: "observable shader
                    // failure status"): `fs_main`'s own `FragmentOutput::
                    // tmem_sample_status`, one `TMEM_SAMPLE_STATUS_*` code per
                    // fragment -- read back and checked by
                    // `production.rs`'s draw-completion path, never silently
                    // discarded.
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: COLOR_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: STATUS_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: None,
            })
        });

        let _ = device.poll(wgpu::PollType::Poll);
        if errors.count() != 0 {
            return Err(TrianglePipelineError::PipelinePrewarm(
                errors
                    .first()
                    .unwrap_or_else(|| "uncaptured validation error".into()),
            ));
        }

        Ok(TrianglePipelineDeviceOutcome::Ready(Box::new(
            TrianglePipelineRenderer {
                _instance: instance,
                adapter_info: adapter.get_info(),
                device,
                queue,
                pipelines,
                bind_group_layout,
                compute_raster_round_trip_pipeline,
                compute_triangle_coverage_pipeline,
                compute_triangle_color_pipeline,
                framebuffer_color_dummy_buffer,
                fixture_buffers: Vec::new(),
                compute_hot_color_buffers: Vec::new(),
                compute_hot_color_chain_status_readback: None,
                compute_hot_color_resource_generations: 0,
                errors,
            },
        )))
    }
}

#[derive(Default)]
struct BoundedErrorSink {
    count: AtomicUsize,
    first: Mutex<Option<String>>,
}

impl BoundedErrorSink {
    fn record(&self, error: String) {
        let _ = self
            .count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                count.checked_add(1)
            });
        let mut first = self.first.lock().expect("wgpu error sink poisoned");
        if first.is_none() {
            *first = Some(error);
        }
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn first(&self) -> Option<String> {
        self.first.lock().expect("wgpu error sink poisoned").clone()
    }
}

/// Pure precondition check for [`TrianglePipelineRenderer::submit_triangle`]:
/// rejects a zero-width or zero-height target before any device work
/// (buffer/texture creation) happens. Factored out so this rejection is
/// independently testable without a device.
const fn validate_triangle_extent(
    extent: TriangleTargetExtent,
) -> Result<(), TrianglePipelineError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(TrianglePipelineError::ZeroExtent {
            width: extent.width,
            height: extent.height,
        });
    }
    Ok(())
}

/// Framebuffer-blend Slice B: partitions `fixtures.len()` draws into maximal
/// contiguous runs, one render pass per run, such that every
/// framebuffer-color-reading fixture (where `reads_framebuffer_color[i]` is
/// `true`) is a singleton run of its own -- a framebuffer-color-reading
/// draw must snapshot the color attachment *after* every earlier-ordered
/// draw has landed and *before* its own draw runs, which is impossible
/// within an already-open pass. A run of only non-reading fixtures needs no
/// split (unaffected fixtures still cost one pass total, not one pass
/// each). Returns each run as a `(start, end)` index pair into `fixtures`
/// (`end` exclusive), in submission order. Pure and independently testable
/// without a device.
fn split_fixture_runs(reads_framebuffer_color: &[bool]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = 0;
    while start < reads_framebuffer_color.len() {
        if reads_framebuffer_color[start] {
            runs.push((start, start + 1));
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < reads_framebuffer_color.len() && !reads_framebuffer_color[end] {
            end += 1;
        }
        runs.push((start, end));
        start = end;
    }
    runs
}

pub struct TrianglePipelineRenderer {
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Four precreated `Z_CMP`x`Z_UPD` pipeline variants, indexed by
    /// [`depth_pipeline_index`]; see module doc and [`DEPTH_STENCIL_VARIANTS`].
    pipelines: [wgpu::RenderPipeline; 4],
    bind_group_layout: wgpu::BindGroupLayout,
    /// Precreated transport-only compute pipeline for qualifying a dynamic
    /// packed-RGBA16 target before raster arithmetic is admitted.
    compute_raster_round_trip_pipeline: wgpu::ComputePipeline,
    /// Exact integer coverage prototype. It writes one 0..=8 count per
    /// target pixel and triangle; no color or production state reaches it.
    compute_triangle_coverage_pipeline: wgpu::ComputePipeline,
    /// Complete color-producing prototype for the leading WM2000 census
    /// state. It remains outside production dispatch until byte differential
    /// and the >=3 ms live kill gate pass.
    compute_triangle_color_pipeline: wgpu::ComputePipeline,
    /// Binding 9's shared dummy resource for every fixture in a submission
    /// whose `reads_framebuffer_color` is false -- one always-allocated
    /// buffer reused across every non-reading fixture and every submission,
    /// never a per-fixture or per-submission allocation.
    framebuffer_color_dummy_buffer: wgpu::Buffer,
    /// Per-fixture buffers, reused across submissions instead of recreated.
    ///
    /// `submit_triangles` used to `create_buffer` ten times per fixture, on
    /// every submission. Measured on WM2000 (rs + wgpu, bounded census,
    /// warmup 300 / 1200 pumps): **1,361,480 buffer creations in ~35 s**, or
    /// ~2,813 per slow pump. A `sample` profile of the same run put
    /// `AGXBuffer initWithDevice:`, `IOGPUResourceCreate`,
    /// `IOConnectCallMethod` and `mach_msg2_trap` at the top -- each wgpu
    /// `create_buffer` is a Metal resource creation with a kernel round
    /// trip. At ~20 us apiece that is ~56 ms of the 64.8 ms slow-pump mean.
    ///
    /// Every one of the ten has a FIXED descriptor (nine compile-time size
    /// constants; the vertex buffer is always `3 * 40` bytes for one
    /// triangle), so a slot can be reused verbatim: same size, same usage.
    /// The pool grows to the submission high-water mark and never shrinks,
    /// so steady state performs zero allocations.
    ///
    /// This is the same principle `framebuffer_color_dummy_buffer` above
    /// already applies, extended from one shared buffer to the per-fixture
    /// set.
    fixture_buffers: Vec<FixtureBuffers>,
    /// High-water resource set for the exact compute-color path. Recreated
    /// only when a later batch exceeds a variable capacity; identical
    /// steady-state draws rewrite and reuse every GPU allocation.
    compute_hot_color_buffers: Vec<ComputeHotColorBuffers>,
    /// One high-water readback for every status range in an ordered chain.
    /// Distinct offsets retain per-batch failure attribution while one map
    /// replaces a map/poll callback cycle per semantic batch.
    compute_hot_color_chain_status_readback: Option<ChainStatusReadback>,
    compute_hot_color_resource_generations: u64,
    errors: Arc<BoundedErrorSink>,
}

struct ComputeHotColorBuffers {
    triangle_capacity: u64,
    target_capacity: u64,
    status_capacity: u64,
    tmem_state_capacity: u64,
    work_item_capacity: u64,
    work_triangle_index_capacity: u64,
    triangle: wgpu::Buffer,
    params: wgpu::Buffer,
    target: wgpu::Buffer,
    status: wgpu::Buffer,
    work_items: wgpu::Buffer,
    work_triangle_indices: wgpu::Buffer,
    tmem_bytes: wgpu::Buffer,
    tmem_validity: wgpu::Buffer,
    tile: wgpu::Buffer,
    target_readback: wgpu::Buffer,
    status_readback: wgpu::Buffer,
    tmem_group: wgpu::BindGroup,
    compute_group: wgpu::BindGroup,
}

struct ChainStatusReadback {
    capacity: u64,
    buffer: wgpu::Buffer,
}

impl ComputeHotColorBuffers {
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::ComputePipeline,
        triangle_capacity: u64,
        target_capacity: u64,
        status_capacity: u64,
        tmem_state_capacity: u64,
        work_item_capacity: u64,
        work_triangle_index_capacity: u64,
    ) -> Self {
        let create = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let triangle = create(
            "fn64-compute-hot-color-triangles",
            triangle_capacity,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let params = create(
            "fn64-compute-hot-color-params",
            32,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let target = create(
            "fn64-compute-hot-color-target",
            target_capacity,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let status = create(
            "fn64-compute-hot-color-status",
            status_capacity,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let work_items = create(
            "fn64-compute-hot-color-work-items",
            work_item_capacity,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let work_triangle_indices = create(
            "fn64-compute-hot-color-work-triangle-indices",
            work_triangle_index_capacity,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let tmem_bytes = create(
            "fn64-compute-hot-color-tmem",
            tmem_state_capacity * TMEM_BYTE_WORDS as u64 * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let tmem_validity = create(
            "fn64-compute-hot-color-tmem-validity",
            tmem_state_capacity * TMEM_VALIDITY_WORDS as u64 * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let tile = create(
            "fn64-compute-hot-color-tile",
            tmem_state_capacity * TILE_BINDING_PARAMS_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let target_readback = create(
            "fn64-compute-hot-color-target-readback",
            target_capacity,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let status_readback = create(
            "fn64-compute-hot-color-status-readback",
            status_capacity,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let tmem_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-compute-hot-color-tmem-group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: tmem_bytes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tmem_validity.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tile.as_entire_binding(),
                },
            ],
        });
        let compute_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-compute-hot-color-state-group"),
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: triangle.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: target.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: status.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: work_items.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: work_triangle_indices.as_entire_binding(),
                },
            ],
        });
        Self {
            triangle_capacity,
            target_capacity,
            status_capacity,
            tmem_state_capacity,
            work_item_capacity,
            work_triangle_index_capacity,
            triangle,
            params,
            target,
            status,
            work_items,
            work_triangle_indices,
            tmem_bytes,
            tmem_validity,
            tile,
            target_readback,
            status_readback,
            tmem_group,
            compute_group,
        }
    }

    const fn fits(
        &self,
        triangle_bytes: u64,
        target_bytes: u64,
        status_bytes: u64,
        tmem_state_count: u64,
        work_item_bytes: u64,
        work_triangle_index_bytes: u64,
    ) -> bool {
        self.triangle_capacity >= triangle_bytes
            && self.target_capacity >= target_bytes
            && self.status_capacity >= status_bytes
            && self.tmem_state_capacity >= tmem_state_count
            && self.work_item_capacity >= work_item_bytes
            && self.work_triangle_index_capacity >= work_triangle_index_bytes
    }
}

/// One fixture's ten reusable GPU buffers. Created once per pool slot and
/// rewritten per submission with `queue.write_buffer`, which is a staging
/// copy rather than a resource creation.
struct FixtureBuffers {
    vertex: wgpu::Buffer,
    raster_params: wgpu::Buffer,
    combine_params: wgpu::Buffer,
    tmem_bytes: wgpu::Buffer,
    tmem_validity: wgpu::Buffer,
    tile_binding: wgpu::Buffer,
    alpha_compare_params: wgpu::Buffer,
    coverage_params: wgpu::Buffer,
    material_params: wgpu::Buffer,
    blend_params: wgpu::Buffer,
}

/// Bytes in one triangle's vertex buffer: three vertices of 40 bytes.
const VERTEX_BUFFER_BYTES: u64 = 3 * 40;

impl FixtureBuffers {
    fn new(device: &wgpu::Device) -> Self {
        let uniform = |label: &'static str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let storage = |label: &'static str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            vertex: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-vertices"),
                size: VERTEX_BUFFER_BYTES,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            raster_params: uniform("fn64-triangle-pipeline-raster-params", RASTER_PARAMS_BYTES),
            combine_params: uniform(
                "fn64-triangle-pipeline-combine-params",
                COMBINE_PARAMS_BYTES,
            ),
            tmem_bytes: storage("fn64-triangle-pipeline-tmem-bytes", TMEM_BYTES_BUFFER_SIZE),
            tmem_validity: storage(
                "fn64-triangle-pipeline-tmem-validity",
                TMEM_VALIDITY_BUFFER_SIZE,
            ),
            tile_binding: uniform(
                "fn64-triangle-pipeline-tile-binding",
                TILE_BINDING_PARAMS_BYTES,
            ),
            alpha_compare_params: uniform(
                "fn64-triangle-pipeline-alpha-compare-params",
                ALPHA_COMPARE_PARAMS_BYTES,
            ),
            coverage_params: uniform(
                "fn64-triangle-pipeline-coverage-params",
                COVERAGE_PARAMS_BYTES,
            ),
            material_params: uniform(
                "fn64-triangle-pipeline-material-params",
                MATERIAL_PARAMS_BYTES,
            ),
            blend_params: uniform("fn64-triangle-pipeline-blend-params", BLEND_PARAMS_BYTES),
        }
    }
}

/// The edge fields consumed by the integer coverage prototype, in the same
/// order as `compute_triangle_coverage.wgsl`'s `TriangleEdges`. Constructed
/// only from the real raw-triangle decoder; callers cannot supply a partial
/// or float-converted edge set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComputeCoverageTriangle {
    left_major: bool,
    yl: i32,
    ym: i32,
    yh: i32,
    xl: i32,
    xh: i32,
    xm: i32,
    dxldy: i32,
    dxhdy: i32,
    dxmdy: i32,
    planes: [ComputeAttributePlane; 7],
    env_rgba8: u32,
    prim_rgba8: u32,
}

const COMPUTE_COVERAGE_TRIANGLE_BYTES: u64 = 164;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ComputeAttributePlane {
    base: i32,
    dx: i32,
    de: i32,
    dy: i32,
}

impl From<crate::raw_dpc::triangle_span::AttributePlane> for ComputeAttributePlane {
    fn from(plane: crate::raw_dpc::triangle_span::AttributePlane) -> Self {
        Self {
            base: plane.base,
            dx: plane.dx,
            de: plane.de,
            dy: plane.dy,
        }
    }
}

/// Exact coverage count and first-covered-subsample attribute origin emitted
/// by the integer compute prototype. A zero-coverage pixel has no attribute
/// origin; nonzero coverage always carries one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComputeRasterSample {
    pub coverage: u32,
    pub attribute_sample: Option<(i32, i64)>,
    pub plane_values: Option<[i64; 7]>,
}

pub(crate) struct ComputeHotColorBatch<'a> {
    pub(crate) extent: TriangleTargetExtent,
    pub(crate) resident_bytes: &'a [u8],
    pub(crate) triangles: &'a [ComputeCoverageTriangle],
    pub(crate) tmem: &'a TmemGpuProjection,
    pub(crate) tile: TileBindingParams,
}

/// One state-compatible dispatch in an ordered compute-color chain. The
/// chain owns the initial target once and preserves dispatch order while
/// selecting each dispatch's immutable TMEM/tile state on the device.
pub(crate) struct ComputeHotColorDispatch<'a> {
    pub(crate) triangles: &'a [ComputeCoverageTriangle],
    pub(crate) tmem: &'a TmemGpuProjection,
    pub(crate) tile: TileBindingParams,
    /// Exact contiguous target-row band containing every declared write in
    /// this state-compatible batch.
    pub(crate) first_row: u32,
    pub(crate) row_count: u32,
    /// Even-aligned target-column band containing every declared write.
    /// Pair alignment preserves one invocation's exclusive ownership of both
    /// RGBA16 pixels in a packed storage word.
    pub(crate) first_column: u32,
    pub(crate) column_count: u32,
}

struct PreparedComputeHotColorBatch {
    expected_bytes: usize,
    triangle_bytes: u64,
    target_bytes: u64,
    status_bytes: u64,
    workgroups: u32,
    packed_triangles: Vec<u8>,
    params: Vec<u8>,
    padded_target: Vec<u8>,
    work_items: Vec<u8>,
    work_triangle_indices: Vec<u8>,
}

struct PreparedComputeHotColorChainBatch {
    first_dispatch: usize,
    dispatch_count: usize,
    checkpoint: bool,
    triangle_bytes: u64,
    target_bytes: u64,
    status_bytes: u64,
    tmem_state_count: u64,
    word_count: u32,
    workgroups: u32,
    draw_count: usize,
    packed_triangles: Vec<u8>,
    params: Vec<u8>,
    tmem_bytes: Vec<u8>,
    tmem_validity: Vec<u8>,
    tiles: Vec<u8>,
    work_items: Vec<u8>,
    work_triangle_indices: Vec<u8>,
    work_item_words: Vec<u32>,
}

fn validate_compute_color_checkpoints(
    dispatches: usize,
    checkpoint_dispatch_limits: &[usize],
) -> Result<(), TrianglePipelineError> {
    let mut previous = 0usize;
    for (checkpoint, &dispatch_limit) in checkpoint_dispatch_limits.iter().enumerate() {
        if dispatch_limit <= previous || dispatch_limit > dispatches {
            return Err(TrianglePipelineError::ComputeColorCheckpointOrder {
                checkpoint,
                previous,
                dispatch_limit,
                dispatches,
            });
        }
        previous = dispatch_limit;
    }
    if previous != dispatches {
        return Err(TrianglePipelineError::ComputeColorCheckpointMissingFinal {
            final_checkpoint: previous,
            dispatches,
        });
    }
    Ok(())
}

impl ComputeCoverageTriangle {
    pub fn from_raw(triangle: crate::RawTriangle) -> Self {
        let mut result = Self {
            // `triangle_span::left_major` proves the decoder's historically
            // named `right_major` accessor is the left-major wire bit.
            left_major: triangle.right_major(),
            yl: triangle.yl() as i32,
            ym: triangle.ym() as i32,
            yh: triangle.yh() as i32,
            xl: triangle.xl(),
            xh: triangle.xh(),
            xm: triangle.xm(),
            dxldy: triangle.dxldy(),
            dxhdy: triangle.dxhdy(),
            dxmdy: triangle.dxmdy(),
            planes: [ComputeAttributePlane {
                base: 0,
                dx: 0,
                de: 0,
                dy: 0,
            }; 7],
            env_rgba8: 0,
            prim_rgba8: 0,
        };
        if let Some(shade) = triangle.shade() {
            for (destination, source) in result.planes[..4]
                .iter_mut()
                .zip(crate::raw_dpc::triangle_span::shade_planes(shade))
            {
                *destination = source.into();
            }
        }
        if let Some(texture) = triangle.texture() {
            for (destination, source) in result.planes[4..]
                .iter_mut()
                .zip(crate::raw_dpc::triangle_span::texture_planes(texture))
            {
                *destination = source.into();
            }
        }
        result
    }

    pub const fn with_material(mut self, environment: Color4, primitive: PrimColor) -> Self {
        self.env_rgba8 = environment.value();
        self.prim_rgba8 = primitive.color().value();
        self
    }

    fn storage_bytes(self) -> [u8; COMPUTE_COVERAGE_TRIANGLE_BYTES as usize] {
        self.storage_bytes_with_tmem_state(0)
    }

    fn storage_bytes_with_tmem_state(
        self,
        tmem_state_index: u32,
    ) -> [u8; COMPUTE_COVERAGE_TRIANGLE_BYTES as usize] {
        let edge_words = [
            u32::from(self.left_major),
            self.yl as u32,
            self.ym as u32,
            self.yh as u32,
            self.xl as u32,
            self.xh as u32,
            self.xm as u32,
            self.dxldy as u32,
            self.dxhdy as u32,
            self.dxmdy as u32,
        ];
        let mut bytes = [0u8; COMPUTE_COVERAGE_TRIANGLE_BYTES as usize];
        for (index, word) in edge_words.into_iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        for (plane_index, plane) in self.planes.into_iter().enumerate() {
            for (field_index, field) in [plane.base, plane.dx, plane.de, plane.dy]
                .into_iter()
                .enumerate()
            {
                let offset = 40 + plane_index * 16 + field_index * 4;
                bytes[offset..offset + 4].copy_from_slice(&field.to_le_bytes());
            }
        }
        bytes[152..156].copy_from_slice(&self.env_rgba8.to_le_bytes());
        bytes[156..160].copy_from_slice(&self.prim_rgba8.to_le_bytes());
        bytes[160..164].copy_from_slice(&tmem_state_index.to_le_bytes());
        bytes
    }
}

impl TrianglePipelineRenderer {
    pub const fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    #[cfg(all(test, feature = "host-gpu-tests"))]
    pub(crate) const fn compute_hot_color_resource_generations(&self) -> u64 {
        self.compute_hot_color_resource_generations
    }

    /// Runs the transport-only compute shader over an arbitrary packed
    /// RGBA16 target and returns exactly the guest-visible bytes. The GPU
    /// buffers are word-addressed, so a final two-byte pixel is zero-padded
    /// only for device transport and truncated after readback.
    pub fn round_trip_compute_raster_rgba16(
        &mut self,
        extent: TriangleTargetExtent,
        resident_bytes: &[u8],
    ) -> Result<Vec<u8>, TrianglePipelineError> {
        validate_triangle_extent(extent)?;
        let expected_bytes_u64 = u64::from(extent.width)
            .checked_mul(u64::from(extent.height))
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or(TrianglePipelineError::Rgba16TargetSizeOverflow {
                width: extent.width,
                height: extent.height,
            })?;
        let expected_bytes = usize::try_from(expected_bytes_u64).map_err(|_| {
            TrianglePipelineError::Rgba16TargetSizeOverflow {
                width: extent.width,
                height: extent.height,
            }
        })?;
        if resident_bytes.len() != expected_bytes {
            return Err(TrianglePipelineError::Rgba16TargetByteLength {
                expected: expected_bytes,
                actual: resident_bytes.len(),
            });
        }

        let transport_bytes = expected_bytes_u64
            .checked_add(3)
            .map(|bytes| bytes / 4 * 4)
            .ok_or(TrianglePipelineError::Rgba16TargetSizeOverflow {
                width: extent.width,
                height: extent.height,
            })?;
        let word_count = transport_bytes / 4;
        let workgroups = word_count.div_ceil(64);
        let limits = self.device.limits();
        if workgroups > u64::from(limits.max_compute_workgroups_per_dimension)
            || transport_bytes > limits.max_buffer_size
            || transport_bytes > u64::from(limits.max_storage_buffer_binding_size)
        {
            return Err(TrianglePipelineError::Rgba16TargetTooLarge {
                width: extent.width,
                height: extent.height,
            });
        }
        let mut padded = vec![0_u8; transport_bytes as usize];
        padded[..resident_bytes.len()].copy_from_slice(resident_bytes);

        let source = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-raster-rgba16-round-trip-source"),
            size: transport_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let target = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-raster-rgba16-round-trip-target"),
            size: transport_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-raster-rgba16-round-trip-readback"),
            size: transport_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&source, 0, &padded);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-compute-raster-rgba16-round-trip-bind-group"),
            layout: &self
                .compute_raster_round_trip_pipeline
                .get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: source.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: target.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-compute-raster-rgba16-round-trip"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-compute-raster-rgba16-round-trip"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_raster_round_trip_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&target, 0, &readback, 0, transport_bytes);
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let submission = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| TrianglePipelineError::CompletionCallbackNotObserved)?;
        let mut output = map_and_read(&self.device, &readback)?;
        output.truncate(expected_bytes);
        Ok(output)
    }

    /// Evaluate the repository's eight-subsample raw-triangle coverage rule
    /// on the GPU. The output is triangle-major, then row-major pixels, with
    /// one `u32` count per pixel. This is a differential substrate only: it
    /// neither reads nor writes a color target and is not on production
    /// dispatch.
    pub fn compute_triangle_coverage(
        &mut self,
        extent: TriangleTargetExtent,
        triangles: &[ComputeCoverageTriangle],
    ) -> Result<Vec<u32>, TrianglePipelineError> {
        Ok(self
            .compute_triangle_samples(extent, triangles)?
            .into_iter()
            .map(|sample| sample.coverage)
            .collect())
    }

    /// Evaluate coverage and the first covered subsample used as the exact
    /// attribute-plane origin. Output ordering matches
    /// [`Self::compute_triangle_coverage`].
    pub fn compute_triangle_samples(
        &mut self,
        extent: TriangleTargetExtent,
        triangles: &[ComputeCoverageTriangle],
    ) -> Result<Vec<ComputeRasterSample>, TrianglePipelineError> {
        validate_triangle_extent(extent)?;
        if triangles.is_empty() {
            return Err(TrianglePipelineError::EmptyCoverageBatch);
        }
        let pixels = extent
            .width
            .checked_mul(extent.height)
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        let triangle_count = u32::try_from(triangles.len())
            .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
        let output_words = pixels
            .checked_mul(triangle_count)
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        let output_bytes = u64::from(output_words) * 72;
        let triangle_bytes = u64::from(triangle_count) * COMPUTE_COVERAGE_TRIANGLE_BYTES;
        let workgroups = output_words.div_ceil(64);
        let limits = self.device.limits();
        if workgroups > limits.max_compute_workgroups_per_dimension
            || output_bytes > limits.max_buffer_size
            || output_bytes > u64::from(limits.max_storage_buffer_binding_size)
            || triangle_bytes > limits.max_buffer_size
            || triangle_bytes > u64::from(limits.max_storage_buffer_binding_size)
        {
            return Err(TrianglePipelineError::CoverageTooLarge);
        }

        let mut packed_triangles = Vec::with_capacity(triangle_bytes as usize);
        for triangle in triangles {
            packed_triangles.extend_from_slice(&triangle.storage_bytes());
        }
        let word_count = pixels.div_ceil(2);
        let mut params = Vec::with_capacity(32);
        for word in [
            extent.width,
            extent.height,
            pixels,
            triangle_count,
            0,
            word_count,
            0,
            0,
        ] {
            params.extend_from_slice(&word.to_le_bytes());
        }
        let triangle_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-triangle-coverage-edges"),
            size: triangle_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-triangle-coverage-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-triangle-coverage-output"),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-compute-triangle-coverage-readback"),
            size: output_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&triangle_buffer, 0, &packed_triangles);
        self.queue.write_buffer(&params_buffer, 0, &params);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-compute-triangle-coverage-bind-group"),
            layout: &self
                .compute_triangle_coverage_pipeline
                .get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: triangle_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-compute-triangle-coverage"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-compute-triangle-coverage"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_triangle_coverage_pipeline);
            pass.set_bind_group(1, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback, 0, output_bytes);
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let submission = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| TrianglePipelineError::CompletionCallbackNotObserved)?;
        let bytes = map_and_read(&self.device, &readback)?;
        Ok(bytes
            .chunks_exact(72)
            .map(|sample| {
                let word = |index: usize| {
                    u32::from_le_bytes(
                        sample[index * 4..index * 4 + 4]
                            .try_into()
                            .expect("four compute sample bytes"),
                    )
                };
                let coverage = word(0);
                let delta_y_eighth = word(1) as i32;
                let delta_x_q16 = i64::from(word(2)) | (i64::from(word(3) as i32) << 32);
                let plane_values = core::array::from_fn(|plane| {
                    let low = word(4 + plane * 2);
                    let high = word(5 + plane * 2) as i32;
                    i64::from(low) | (i64::from(high) << 32)
                });
                ComputeRasterSample {
                    coverage,
                    attribute_sample: (coverage != 0).then_some((delta_y_eighth, delta_x_q16)),
                    plane_values: (coverage != 0).then_some(plane_values),
                }
            })
            .collect())
    }

    /// Execute the leading WM2000 census state over a packed RGBA16 target.
    /// All triangles share one immutable committed-TMEM projection and tile
    /// binding; callers must split at a TMEM identity change.
    pub fn compute_triangle_hot_color(
        &mut self,
        extent: TriangleTargetExtent,
        resident_bytes: &[u8],
        triangles: &[ComputeCoverageTriangle],
        tmem: &TmemGpuProjection,
        tile: TileBindingParams,
    ) -> Result<Vec<u8>, TrianglePipelineError> {
        let mut outputs = self.compute_triangle_hot_color_batches(&[ComputeHotColorBatch {
            extent,
            resident_bytes,
            triangles,
            tmem,
            tile,
        }])?;
        Ok(outputs
            .pop()
            .expect("one submitted compute-color batch returns one target"))
    }

    pub(crate) fn compute_triangle_hot_color_batches(
        &mut self,
        batches: &[ComputeHotColorBatch<'_>],
    ) -> Result<Vec<Vec<u8>>, TrianglePipelineError> {
        if batches.is_empty() {
            return Err(TrianglePipelineError::EmptyCoverageBatch);
        }
        let limits = self.device.limits();
        let mut prepared = Vec::with_capacity(batches.len());
        for batch in batches {
            validate_triangle_extent(batch.extent)?;
            if batch.triangles.is_empty() {
                return Err(TrianglePipelineError::EmptyCoverageBatch);
            }
            let pixels = batch
                .extent
                .width
                .checked_mul(batch.extent.height)
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            let expected_bytes = usize::try_from(pixels)
                .ok()
                .and_then(|count| count.checked_mul(2))
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            if batch.resident_bytes.len() != expected_bytes {
                return Err(TrianglePipelineError::ComputeColorTargetLength {
                    expected: expected_bytes,
                    actual: batch.resident_bytes.len(),
                });
            }
            let triangle_count = u32::try_from(batch.triangles.len())
                .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
            let triangle_bytes = u64::from(triangle_count) * COMPUTE_COVERAGE_TRIANGLE_BYTES;
            let word_count = pixels.div_ceil(2);
            let target_bytes = u64::from(word_count) * 4;
            let status_bytes = u64::from(pixels) * 4;
            let work_item_bytes = u64::from(word_count) * 12;
            let work_triangle_index_bytes = u64::from(triangle_count) * 4;
            let workgroups = word_count.div_ceil(64);
            if workgroups > limits.max_compute_workgroups_per_dimension
                || triangle_bytes > limits.max_buffer_size
                || triangle_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || target_bytes > limits.max_buffer_size
                || target_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || status_bytes > limits.max_buffer_size
                || status_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || work_item_bytes > limits.max_buffer_size
                || work_item_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || work_triangle_index_bytes > limits.max_buffer_size
                || work_triangle_index_bytes > u64::from(limits.max_storage_buffer_binding_size)
            {
                return Err(TrianglePipelineError::CoverageTooLarge);
            }
            let mut packed_triangles = Vec::with_capacity(triangle_bytes as usize);
            for triangle in batch.triangles {
                packed_triangles.extend_from_slice(&triangle.storage_bytes());
            }
            let mut work_items = Vec::with_capacity(work_item_bytes as usize);
            for target_word in 0..word_count {
                for word in [target_word, 0, triangle_count] {
                    work_items.extend_from_slice(&word.to_le_bytes());
                }
            }
            let mut work_triangle_indices = Vec::with_capacity(work_triangle_index_bytes as usize);
            for triangle_index in 0..triangle_count {
                work_triangle_indices.extend_from_slice(&triangle_index.to_le_bytes());
            }
            let mut params = Vec::with_capacity(32);
            for word in [
                batch.extent.width,
                batch.extent.height,
                pixels,
                triangle_count,
                0,
                word_count,
                0,
                0,
            ] {
                params.extend_from_slice(&word.to_le_bytes());
            }
            let mut padded_target = batch.resident_bytes.to_vec();
            padded_target.resize(target_bytes as usize, 0);
            prepared.push(PreparedComputeHotColorBatch {
                expected_bytes,
                triangle_bytes,
                target_bytes,
                status_bytes,
                workgroups,
                packed_triangles,
                params,
                padded_target,
                work_items,
                work_triangle_indices,
            });
        }
        for (index, batch) in prepared.iter().enumerate() {
            let replacement = self
                .compute_hot_color_buffers
                .get(index)
                .is_some_and(|buffers| {
                    !buffers.fits(
                        batch.triangle_bytes,
                        batch.target_bytes,
                        batch.status_bytes,
                        1,
                        batch.work_items.len() as u64,
                        batch.work_triangle_indices.len() as u64,
                    )
                });
            if index == self.compute_hot_color_buffers.len() || replacement {
                let buffers = ComputeHotColorBuffers::new(
                    &self.device,
                    &self.compute_triangle_color_pipeline,
                    batch.triangle_bytes,
                    batch.target_bytes,
                    batch.status_bytes,
                    1,
                    batch.work_items.len() as u64,
                    batch.work_triangle_indices.len() as u64,
                );
                if index == self.compute_hot_color_buffers.len() {
                    self.compute_hot_color_buffers.push(buffers);
                } else {
                    self.compute_hot_color_buffers[index] = buffers;
                }
                self.compute_hot_color_resource_generations = self
                    .compute_hot_color_resource_generations
                    .checked_add(1)
                    .expect("compute-color resource generation overflow");
            }
        }
        for (index, (batch, input)) in prepared.iter().zip(batches).enumerate() {
            let buffers = &self.compute_hot_color_buffers[index];
            self.queue
                .write_buffer(&buffers.triangle, 0, &batch.packed_triangles);
            self.queue.write_buffer(&buffers.params, 0, &batch.params);
            self.queue
                .write_buffer(&buffers.work_items, 0, &batch.work_items);
            self.queue.write_buffer(
                &buffers.work_triangle_indices,
                0,
                &batch.work_triangle_indices,
            );
            self.queue
                .write_buffer(&buffers.target, 0, &batch.padded_target);
            self.queue
                .write_buffer(&buffers.tmem_bytes, 0, &input.tmem.byte_words_bytes());
            self.queue.write_buffer(
                &buffers.tmem_validity,
                0,
                &input.tmem.validity_words_bytes(),
            );
            self.queue
                .write_buffer(&buffers.tile, 0, &input.tile.to_bytes());
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-compute-hot-color"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-compute-hot-color"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_triangle_color_pipeline);
            for (index, batch) in prepared.iter().enumerate() {
                let buffers = &self.compute_hot_color_buffers[index];
                pass.set_bind_group(0, &buffers.tmem_group, &[]);
                pass.set_bind_group(1, &buffers.compute_group, &[]);
                pass.dispatch_workgroups(batch.workgroups, 1, 1);
            }
        }
        for (index, batch) in prepared.iter().enumerate() {
            let buffers = &self.compute_hot_color_buffers[index];
            encoder.copy_buffer_to_buffer(
                &buffers.target,
                0,
                &buffers.target_readback,
                0,
                batch.target_bytes,
            );
            encoder.copy_buffer_to_buffer(
                &buffers.status,
                0,
                &buffers.status_readback,
                0,
                batch.status_bytes,
            );
        }
        let submission = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        let mut outputs = Vec::with_capacity(prepared.len());
        for (index, batch) in prepared.iter().enumerate() {
            let buffers = &self.compute_hot_color_buffers[index];
            let statuses =
                map_and_read_prefix(&self.device, &buffers.status_readback, batch.status_bytes)?;
            if let Some((pixel, status)) = statuses
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().expect("four status bytes")))
                .enumerate()
                .find(|(_, status)| (*status & 0xff) != TMEM_SAMPLE_STATUS_OK)
            {
                return Err(TrianglePipelineError::ComputeColorBatchTmemStatus {
                    batch: index,
                    pixel,
                    status: status & 0xff,
                });
            }
            let mut bytes =
                map_and_read_prefix(&self.device, &buffers.target_readback, batch.target_bytes)?;
            bytes.truncate(batch.expected_bytes);
            outputs.push(bytes);
        }
        Ok(outputs)
    }

    /// Executes an ordered sequence of state-compatible dispatches against
    /// one packed RGBA16 target. A sparse word worklist preserves painter
    /// order while one compute pass selects each triangle's immutable
    /// TMEM/tile state. The chain performs one host target upload, one
    /// submission/wait, and one target readback regardless of the number of
    /// typed state boundaries. Each pixel retains its first TMEM refusal and
    /// the originating state index, so a later success cannot hide it.
    pub(crate) fn compute_triangle_hot_color_chain(
        &mut self,
        extent: TriangleTargetExtent,
        resident_bytes: &[u8],
        dispatches: &[ComputeHotColorDispatch<'_>],
    ) -> Result<Vec<u8>, TrianglePipelineError> {
        let mut outputs = self.compute_triangle_hot_color_chain_checkpoints(
            extent,
            resident_bytes,
            dispatches,
            &[dispatches.len()],
        )?;
        Ok(outputs
            .pop()
            .expect("a non-empty compute chain has its final checkpoint"))
    }

    /// Executes one ordered chain while retaining the exact target image at
    /// each requested dispatch boundary. All passes and checkpoint copies
    /// occupy one command buffer and therefore one submit/wait. A caller can
    /// use those images to commit each original packet independently without
    /// turning packet-local guest-write journals into one synthetic journal.
    pub(crate) fn compute_triangle_hot_color_chain_checkpoints(
        &mut self,
        extent: TriangleTargetExtent,
        resident_bytes: &[u8],
        dispatches: &[ComputeHotColorDispatch<'_>],
        checkpoint_dispatch_limits: &[usize],
    ) -> Result<Vec<Vec<u8>>, TrianglePipelineError> {
        let timing_total = compute_chain_timing_enabled().then(Instant::now);
        let mut timing_mark = timing_total;
        if dispatches.is_empty() {
            return Err(TrianglePipelineError::EmptyCoverageBatch);
        }
        validate_compute_color_checkpoints(dispatches.len(), checkpoint_dispatch_limits)?;
        validate_triangle_extent(extent)?;
        let limits = self.device.limits();
        let pixels = extent
            .width
            .checked_mul(extent.height)
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        let expected_bytes = usize::try_from(pixels)
            .ok()
            .and_then(|count| count.checked_mul(2))
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        if resident_bytes.len() != expected_bytes {
            return Err(TrianglePipelineError::ComputeColorTargetLength {
                expected: expected_bytes,
                actual: resident_bytes.len(),
            });
        }
        let target_words = pixels.div_ceil(2);
        let target_bytes = u64::from(target_words) * 4;
        if target_bytes > limits.max_buffer_size
            || target_bytes > u64::from(limits.max_storage_buffer_binding_size)
        {
            return Err(TrianglePipelineError::CoverageTooLarge);
        }

        if compute_gpu_timing_enabled() {
            let mut state_runs = Vec::new();
            let mut run_start = 0usize;
            while run_start < dispatches.len() {
                let mut run_end = run_start + 1;
                while run_end < dispatches.len()
                    && dispatches[run_end].tmem == dispatches[run_start].tmem
                    && dispatches[run_end].tile == dispatches[run_start].tile
                {
                    run_end += 1;
                }
                state_runs.push((
                    run_end - run_start,
                    dispatches[run_start..run_end]
                        .iter()
                        .map(|dispatch| dispatch.triangles.len())
                        .sum::<usize>(),
                ));
                run_start = run_end;
            }
            eprintln!(
                "[compute-state-runs] dispatches={} runs={} run(dispatches,draws)={state_runs:?}",
                dispatches.len(),
                state_runs.len(),
            );
        }

        let fusion_enabled = compute_state_table_fusion_enabled();
        let dispatch_ranges: Vec<(usize, usize)> = if fusion_enabled {
            let mut start = 0usize;
            checkpoint_dispatch_limits
                .iter()
                .map(|&end| {
                    let range = (start, end);
                    start = end;
                    range
                })
                .collect()
        } else {
            (0..dispatches.len())
                .map(|index| (index, index + 1))
                .collect()
        };
        let mut prepared = Vec::with_capacity(dispatch_ranges.len());
        for (dispatch_start, dispatch_end) in dispatch_ranges {
            let batch_dispatches = &dispatches[dispatch_start..dispatch_end];
            let mut draw_count = 0usize;
            for dispatch in batch_dispatches {
                if dispatch.triangles.is_empty() {
                    return Err(TrianglePipelineError::EmptyCoverageBatch);
                }
                let dispatch_row_limit = dispatch.first_row.checked_add(dispatch.row_count).ok_or(
                    TrianglePipelineError::ComputeColorDispatchRows {
                        first_row: dispatch.first_row,
                        row_count: dispatch.row_count,
                        height: extent.height,
                    },
                )?;
                if dispatch.row_count == 0 || dispatch_row_limit > extent.height {
                    return Err(TrianglePipelineError::ComputeColorDispatchRows {
                        first_row: dispatch.first_row,
                        row_count: dispatch.row_count,
                        height: extent.height,
                    });
                }
                let dispatch_column_limit = dispatch
                    .first_column
                    .checked_add(dispatch.column_count)
                    .ok_or(TrianglePipelineError::ComputeColorDispatchColumns {
                        first_column: dispatch.first_column,
                        column_count: dispatch.column_count,
                        width: extent.width,
                    })?;
                if dispatch.column_count == 0 || dispatch_column_limit > extent.width {
                    return Err(TrianglePipelineError::ComputeColorDispatchColumns {
                        first_column: dispatch.first_column,
                        column_count: dispatch.column_count,
                        width: extent.width,
                    });
                }
                draw_count = draw_count
                    .checked_add(dispatch.triangles.len())
                    .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            }
            let triangle_count = u32::try_from(draw_count)
                .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
            let triangle_bytes = u64::from(triangle_count)
                .checked_mul(COMPUTE_COVERAGE_TRIANGLE_BYTES)
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            let tmem_state_count = u64::try_from(batch_dispatches.len())
                .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
            if tmem_state_count > u64::from(u32::MAX >> 8) {
                return Err(TrianglePipelineError::CoverageSizeOverflow);
            }
            let tmem_bytes_size = tmem_state_count
                .checked_mul(TMEM_BYTE_WORDS as u64 * 4)
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            let tmem_validity_size = tmem_state_count
                .checked_mul(TMEM_VALIDITY_WORDS as u64 * 4)
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            let tile_bytes_size = tmem_state_count
                .checked_mul(TILE_BINDING_PARAMS_BYTES)
                .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
            let mut packed_triangles = Vec::with_capacity(triangle_bytes as usize);
            let mut tmem_bytes = Vec::with_capacity(tmem_bytes_size as usize);
            let mut tmem_validity = Vec::with_capacity(tmem_validity_size as usize);
            let mut tiles = Vec::with_capacity(tile_bytes_size as usize);
            let mut dispatch_triangle_ranges = Vec::with_capacity(batch_dispatches.len());
            let mut first_triangle_index = 0u32;
            for (state_index, dispatch) in batch_dispatches.iter().enumerate() {
                let state_index = u32::try_from(state_index)
                    .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
                for triangle in dispatch.triangles {
                    packed_triangles
                        .extend_from_slice(&triangle.storage_bytes_with_tmem_state(state_index));
                }
                let dispatch_triangle_count = u32::try_from(dispatch.triangles.len())
                    .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
                dispatch_triangle_ranges.push((
                    dispatch,
                    first_triangle_index,
                    dispatch_triangle_count,
                ));
                first_triangle_index = first_triangle_index
                    .checked_add(dispatch_triangle_count)
                    .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
                tmem_bytes.extend_from_slice(&dispatch.tmem.byte_words_bytes());
                tmem_validity.extend_from_slice(&dispatch.tmem.validity_words_bytes());
                tiles.extend_from_slice(&dispatch.tile.to_bytes());
            }
            let dense_worklist = batch_dispatches.iter().all(|dispatch| {
                dispatch.first_row == 0
                    && dispatch.row_count == extent.height
                    && dispatch.first_column == 0
                    && dispatch.column_count == extent.width
            });
            let (work_items, work_triangle_indices, work_item_words) = if dense_worklist {
                (
                    vec![0; 12],
                    vec![0; 4],
                    (0..target_words).collect::<Vec<_>>(),
                )
            } else {
                let mut work_items = Vec::new();
                let mut work_triangle_indices = Vec::new();
                let mut work_item_words = Vec::new();
                let aligned_rectangles = extent.width.is_multiple_of(2)
                    && dispatch_triangle_ranges.iter().all(|(dispatch, _, _)| {
                        dispatch.first_column.is_multiple_of(2)
                            && dispatch.column_count.is_multiple_of(2)
                    });
                let mut append_target_word =
                    |target_word: u32| -> Result<(), TrianglePipelineError> {
                        let first_work_triangle = u32::try_from(work_triangle_indices.len())
                            .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
                        for &(dispatch, first_triangle, triangle_count) in &dispatch_triangle_ranges
                        {
                            let covered = if aligned_rectangles {
                                let words_per_row = extent.width / 2;
                                let row = target_word / words_per_row;
                                let column_word = target_word % words_per_row;
                                row >= dispatch.first_row
                                    && row < dispatch.first_row + dispatch.row_count
                                    && column_word >= dispatch.first_column / 2
                                    && column_word
                                        < (dispatch.first_column + dispatch.column_count) / 2
                            } else {
                                // A packed word can cross an odd-width row boundary.
                                // Match the prior complete-contiguous-row fallback.
                                let first_pixel = dispatch.first_row * extent.width;
                                let pixel_limit =
                                    (dispatch.first_row + dispatch.row_count) * extent.width;
                                target_word >= first_pixel / 2
                                    && target_word < pixel_limit.div_ceil(2)
                            };
                            if covered {
                                work_triangle_indices
                                    .extend(first_triangle..first_triangle + triangle_count);
                            }
                        }
                        let triangle_limit = u32::try_from(work_triangle_indices.len())
                            .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
                        let work_triangle_count = triangle_limit - first_work_triangle;
                        if work_triangle_count == 0 {
                            return Ok(());
                        }
                        for word in [target_word, first_work_triangle, work_triangle_count] {
                            work_items.extend_from_slice(&word.to_le_bytes());
                        }
                        work_item_words.push(target_word);
                        Ok(())
                    };
                if aligned_rectangles {
                    let words_per_row = extent.width / 2;
                    let first_row = batch_dispatches
                        .iter()
                        .map(|dispatch| dispatch.first_row)
                        .min()
                        .expect("a prepared batch has at least one dispatch");
                    let row_limit = batch_dispatches
                        .iter()
                        .map(|dispatch| dispatch.first_row + dispatch.row_count)
                        .max()
                        .expect("a prepared batch has at least one dispatch");
                    for row in first_row..row_limit {
                        let mut first_column_word = words_per_row;
                        let mut column_word_limit = 0;
                        for dispatch in batch_dispatches.iter().filter(|dispatch| {
                            row >= dispatch.first_row
                                && row < dispatch.first_row + dispatch.row_count
                        }) {
                            first_column_word = first_column_word.min(dispatch.first_column / 2);
                            column_word_limit = column_word_limit
                                .max((dispatch.first_column + dispatch.column_count) / 2);
                        }
                        for column_word in first_column_word..column_word_limit {
                            append_target_word(row * words_per_row + column_word)?;
                        }
                    }
                } else {
                    for target_word in 0..target_words {
                        append_target_word(target_word)?;
                    }
                }
                (work_items, work_triangle_indices, work_item_words)
            };
            let word_count = u32::try_from(work_item_words.len())
                .map_err(|_| TrianglePipelineError::CoverageSizeOverflow)?;
            let status_bytes = u64::from(word_count) * 8;
            let work_item_bytes = work_items.len() as u64;
            let work_triangle_index_bytes = work_triangle_indices.len() as u64 * 4;
            let workgroups = word_count.div_ceil(64);
            if triangle_bytes > limits.max_buffer_size
                || triangle_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || status_bytes > limits.max_buffer_size
                || status_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || tmem_bytes_size > limits.max_buffer_size
                || tmem_bytes_size > u64::from(limits.max_storage_buffer_binding_size)
                || tmem_validity_size > limits.max_buffer_size
                || tmem_validity_size > u64::from(limits.max_storage_buffer_binding_size)
                || tile_bytes_size > limits.max_buffer_size
                || tile_bytes_size > u64::from(limits.max_storage_buffer_binding_size)
                || work_item_bytes > limits.max_buffer_size
                || work_item_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || work_triangle_index_bytes > limits.max_buffer_size
                || work_triangle_index_bytes > u64::from(limits.max_storage_buffer_binding_size)
                || workgroups > limits.max_compute_workgroups_per_dimension
            {
                return Err(TrianglePipelineError::CoverageTooLarge);
            }
            let mut work_triangle_index_bytes =
                Vec::with_capacity(work_triangle_index_bytes as usize);
            for triangle_index in &work_triangle_indices {
                work_triangle_index_bytes.extend_from_slice(&triangle_index.to_le_bytes());
            }
            let mut params = Vec::with_capacity(32);
            for word in [
                extent.width,
                extent.height,
                pixels,
                triangle_count,
                0,
                word_count,
                if dense_worklist { u32::MAX } else { 0 },
                0,
            ] {
                params.extend_from_slice(&word.to_le_bytes());
            }
            prepared.push(PreparedComputeHotColorChainBatch {
                first_dispatch: dispatch_start,
                dispatch_count: batch_dispatches.len(),
                checkpoint: checkpoint_dispatch_limits
                    .binary_search(&dispatch_end)
                    .is_ok(),
                triangle_bytes,
                target_bytes,
                status_bytes,
                tmem_state_count,
                word_count,
                workgroups,
                draw_count,
                packed_triangles,
                params,
                tmem_bytes,
                tmem_validity,
                tiles,
                work_items,
                work_triangle_indices: work_triangle_index_bytes,
                work_item_words,
            });
        }
        let timing_prepare = compute_chain_timing_lap(&mut timing_mark);

        for (index, batch) in prepared.iter().enumerate() {
            let replacement = self
                .compute_hot_color_buffers
                .get(index)
                .is_some_and(|buffers| {
                    !buffers.fits(
                        batch.triangle_bytes,
                        batch.target_bytes,
                        batch.status_bytes,
                        batch.tmem_state_count,
                        batch.work_items.len() as u64,
                        batch.work_triangle_indices.len() as u64,
                    )
                });
            if index == self.compute_hot_color_buffers.len() || replacement {
                let buffers = ComputeHotColorBuffers::new(
                    &self.device,
                    &self.compute_triangle_color_pipeline,
                    batch.triangle_bytes,
                    batch.target_bytes,
                    batch.status_bytes,
                    batch.tmem_state_count,
                    batch.work_items.len() as u64,
                    batch.work_triangle_indices.len() as u64,
                );
                if index == self.compute_hot_color_buffers.len() {
                    self.compute_hot_color_buffers.push(buffers);
                } else {
                    self.compute_hot_color_buffers[index] = buffers;
                }
                self.compute_hot_color_resource_generations = self
                    .compute_hot_color_resource_generations
                    .checked_add(1)
                    .expect("compute-color resource generation overflow");
            }
        }

        let chain_status_bytes = prepared
            .iter()
            .try_fold(0u64, |total, batch| total.checked_add(batch.status_bytes));
        let chain_status_bytes =
            chain_status_bytes.ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        if self
            .compute_hot_color_chain_status_readback
            .as_ref()
            .is_none_or(|readback| readback.capacity < chain_status_bytes)
        {
            self.compute_hot_color_chain_status_readback = Some(ChainStatusReadback {
                capacity: chain_status_bytes,
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fn64-compute-hot-color-chain-status-readback"),
                    size: chain_status_bytes,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            });
        }
        let timing_resources = compute_chain_timing_lap(&mut timing_mark);

        let mut padded_target = resident_bytes.to_vec();
        padded_target.resize(target_bytes as usize, 0);
        self.queue
            .write_buffer(&self.compute_hot_color_buffers[0].target, 0, &padded_target);
        for (index, batch) in prepared.iter().enumerate() {
            let buffers = &self.compute_hot_color_buffers[index];
            self.queue
                .write_buffer(&buffers.triangle, 0, &batch.packed_triangles);
            self.queue.write_buffer(&buffers.params, 0, &batch.params);
            self.queue
                .write_buffer(&buffers.work_items, 0, &batch.work_items);
            self.queue.write_buffer(
                &buffers.work_triangle_indices,
                0,
                &batch.work_triangle_indices,
            );
            self.queue
                .write_buffer(&buffers.tmem_bytes, 0, &batch.tmem_bytes);
            self.queue
                .write_buffer(&buffers.tmem_validity, 0, &batch.tmem_validity);
            self.queue.write_buffer(&buffers.tile, 0, &batch.tiles);
        }
        let timing_uploads = compute_chain_timing_lap(&mut timing_mark);

        // Every pass in the chain mutates the same packed target. Pass
        // boundaries order storage writes, while each invocation retains
        // exclusive ownership of its packed word. Binding one target avoids
        // a full-target device copy at every semantic state boundary.
        let shared_target = &self.compute_hot_color_buffers[0].target;
        let layout = self
            .compute_triangle_color_pipeline
            .get_bind_group_layout(1);
        let mut chain_groups = Vec::with_capacity(prepared.len());
        for buffers in &self.compute_hot_color_buffers[..prepared.len()] {
            chain_groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fn64-compute-hot-color-chain-state-group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffers.triangle.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffers.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: shared_target.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: buffers.status.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: buffers.work_items.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: buffers.work_triangle_indices.as_entire_binding(),
                    },
                ],
            }));
        }
        let timing_bind_groups = compute_chain_timing_lap(&mut timing_mark);

        let gpu_query_count = u32::try_from(prepared.len())
            .ok()
            .and_then(|count| count.checked_mul(2))
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        let gpu_allocated_query_count = gpu_query_count
            .checked_add(2)
            .ok_or(TrianglePipelineError::CoverageSizeOverflow)?;
        let gpu_query_bytes = u64::from(gpu_allocated_query_count) * u64::from(wgpu::QUERY_SIZE);
        let gpu_queries = compute_gpu_timing_enabled().then(|| {
            let set = self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("fn64-compute-hot-color-chain-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: gpu_allocated_query_count,
            });
            let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-compute-hot-color-chain-timestamp-resolve"),
                size: gpu_query_bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-compute-hot-color-chain-timestamp-readback"),
                size: gpu_query_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            (set, resolve, readback)
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-compute-hot-color-chain"),
            });
        for (index, batch) in prepared.iter().enumerate() {
            {
                let timestamp_writes =
                    gpu_queries
                        .as_ref()
                        .map(|(set, _, _)| wgpu::ComputePassTimestampWrites {
                            query_set: set,
                            beginning_of_pass_write_index: Some(index as u32 * 2 + 1),
                            end_of_pass_write_index: Some(index as u32 * 2 + 2),
                        });
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("fn64-compute-hot-color-chain-dispatch"),
                    timestamp_writes,
                });
                let buffers = &self.compute_hot_color_buffers[index];
                pass.set_pipeline(&self.compute_triangle_color_pipeline);
                pass.set_bind_group(0, &buffers.tmem_group, &[]);
                pass.set_bind_group(1, &chain_groups[index], &[]);
                pass.dispatch_workgroups(batch.workgroups, 1, 1);
            }
            if batch.checkpoint {
                let checkpoint_readback = &self.compute_hot_color_buffers[index].target_readback;
                encoder.copy_buffer_to_buffer(
                    shared_target,
                    0,
                    checkpoint_readback,
                    0,
                    target_bytes,
                );
            }
        }
        let chain_status_readback = &self
            .compute_hot_color_chain_status_readback
            .as_ref()
            .expect("non-empty chain has a status readback")
            .buffer;
        let mut status_offset = 0u64;
        for (index, batch) in prepared.iter().enumerate() {
            let buffers = &self.compute_hot_color_buffers[index];
            encoder.copy_buffer_to_buffer(
                &buffers.status,
                0,
                chain_status_readback,
                status_offset,
                batch.status_bytes,
            );
            status_offset += batch.status_bytes;
        }
        if let Some((set, resolve, readback)) = &gpu_queries {
            encoder.resolve_query_set(set, 0..gpu_allocated_query_count, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, gpu_query_bytes);
        }
        let command_buffer = encoder.finish();
        let timing_encode = compute_chain_timing_lap(&mut timing_mark);
        let submission = self.queue.submit([command_buffer]);
        let timing_submit = compute_chain_timing_lap(&mut timing_mark);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        let timing_wait = compute_chain_timing_lap(&mut timing_mark);
        if let Some((_, _, readback)) = &gpu_queries {
            let bytes = map_and_read_prefix(&self.device, readback, gpu_query_bytes)?;
            let resolved_timestamps = bytes
                .chunks_exact(wgpu::QUERY_SIZE as usize)
                .map(|word| u64::from_le_bytes(word.try_into().expect("eight timestamp bytes")))
                .collect::<Vec<_>>();
            let timestamps = &resolved_timestamps[1..gpu_query_count as usize + 1];
            let period_ns = f64::from(self.queue.get_timestamp_period());
            let dispatch_ms = timestamps
                .chunks_exact(2)
                .map(|pair| {
                    (pair[0] != 0 && pair[1] >= pair[0])
                        .then(|| (pair[1] - pair[0]) as f64 * period_ns / 1_000_000.0)
                })
                .collect::<Vec<_>>();
            let final_timestamp = *timestamps
                .last()
                .expect("non-empty timestamp query has a final value");
            let span_ms = (timestamps[0] != 0 && final_timestamp >= timestamps[0])
                .then(|| (final_timestamp - timestamps[0]) as f64 * period_ns / 1_000_000.0);
            let rows = prepared
                .iter()
                .zip(&dispatch_ms)
                .map(|(batch, elapsed_ms)| {
                    (
                        batch.draw_count,
                        u64::from(batch.word_count) * 2,
                        *elapsed_ms,
                    )
                })
                .collect::<Vec<_>>();
            let valid_sum_ms = dispatch_ms.iter().flatten().sum::<f64>();
            let invalid_dispatches = dispatch_ms
                .iter()
                .filter(|elapsed| elapsed.is_none())
                .count();
            eprintln!(
                "[compute-gpu-timing] semantic_dispatches={} passes={} period_ns={period_ns:.6} \
                 span_ms={span_ms:?} \
                 valid_sum_ms={valid_sum_ms:.3} invalid_dispatches={invalid_dispatches} \
                 dispatches(draws,pixels,ms)={rows:?}",
                dispatches.len(),
                prepared.len(),
            );
        }
        let timing_gpu_map = compute_chain_timing_lap(&mut timing_mark);
        let mut readbacks = Vec::with_capacity(checkpoint_dispatch_limits.len() + 1);
        readbacks.push((chain_status_readback, chain_status_bytes));
        for (index, batch) in prepared.iter().enumerate() {
            if batch.checkpoint {
                readbacks.push((
                    &self.compute_hot_color_buffers[index].target_readback,
                    target_bytes,
                ));
            }
        }
        let mut mapped = map_and_read_prefixes(&self.device, &readbacks)?;
        let statuses = mapped.remove(0);
        let mut status_offset = 0usize;
        for batch in &prepared {
            let status_end = status_offset + batch.status_bytes as usize;
            if let Some((pixel, status)) = statuses[status_offset..status_end]
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().expect("four status bytes")))
                .enumerate()
                .find(|(_, status)| (*status & 0xff) != TMEM_SAMPLE_STATUS_OK)
            {
                let state_index = status >> 8;
                if state_index as usize >= batch.dispatch_count {
                    return Err(TrianglePipelineError::ComputeColorStatusState {
                        first_dispatch: batch.first_dispatch,
                        state_index,
                        state_count: batch.dispatch_count,
                    });
                }
                return Err(TrianglePipelineError::ComputeColorBatchTmemStatus {
                    batch: batch.first_dispatch + state_index as usize,
                    pixel: batch.work_item_words[pixel / 2] as usize * 2 + pixel % 2,
                    status: status & 0xff,
                });
            }
            status_offset = status_end;
        }
        let timing_status_map = compute_chain_timing_lap(&mut timing_mark);
        let mut outputs = mapped;
        for output in &mut outputs {
            output.truncate(expected_bytes);
        }
        let timing_target_map = compute_chain_timing_lap(&mut timing_mark);
        if let Some(started) = timing_total {
            eprintln!(
                "[compute-chain-timing] dispatches={} draws={} pixels={} \
                 prepare_ms={:.3} resources_ms={:.3} uploads_ms={:.3} bind_groups_ms={:.3} \
                 encode_ms={:.3} submit_ms={:.3} wait_ms={:.3} gpu_map_ms={:.3} status_map_ms={:.3} \
                 target_map_ms={:.3} total_ms={:.3}",
                dispatches.len(),
                dispatches
                    .iter()
                    .map(|dispatch| dispatch.triangles.len())
                    .sum::<usize>(),
                prepared
                    .iter()
                    .map(|batch| u64::from(batch.word_count) * 2)
                    .sum::<u64>(),
                timing_prepare.as_secs_f64() * 1_000.0,
                timing_resources.as_secs_f64() * 1_000.0,
                timing_uploads.as_secs_f64() * 1_000.0,
                timing_bind_groups.as_secs_f64() * 1_000.0,
                timing_encode.as_secs_f64() * 1_000.0,
                timing_submit.as_secs_f64() * 1_000.0,
                timing_wait.as_secs_f64() * 1_000.0,
                timing_gpu_map.as_secs_f64() * 1_000.0,
                timing_status_map.as_secs_f64() * 1_000.0,
                timing_target_map.as_secs_f64() * 1_000.0,
                started.elapsed().as_secs_f64() * 1_000.0,
            );
        }
        Ok(outputs)
    }

    /// Submits one fixed-fixture triangle draw (`draw(0..3, 0..1)`) into a
    /// fresh per-submission color+depth texture pair (matching M3.3c's
    /// per-submission buffer pattern, not a persistent swapchain -- port
    /// card §3 step 6), then reads back both the color and depth targets.
    /// Thin single-draw wrapper over [`Self::submit_triangles`]; see that
    /// method for the multi-draw-same-target shape a real depth-reject
    /// differential needs.
    pub fn submit_triangle(
        &mut self,
        fixture: TriangleFixture,
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        self.submit_triangles(&[fixture])
    }

    /// Submits one triangle sourced from a real admitted raw-DPC plan
    /// (`raw_dpc::triangle_draw_data`'s `RetrievedTriangleDraw`, snapshotted
    /// at that triangle's own stream position) rather than a hand-built
    /// fixture: `vertices` are the plan's own decoded `NeutralTriangleVertex`
    /// triple, adapted field-by-field via [`neutral_vertex_to_raster_vertex`]
    /// (no arithmetic), and `combine_params` is the plan's own real
    /// `SetCombine` value, not a caller-supplied literal.
    ///
    /// `other_mode` feeds the vertex shader's still-unconsumed Z-override
    /// slice (this slice's vertex shader,
    /// `shaders/triangle_pipeline_vertex.wgsl`, still hardcodes
    /// `is_rect = false`/no Z-override -- module doc, `fixed_fixture_other_mode`
    /// -- a future slice that wires the Z-override branch into the vertex
    /// shader would read it here) and now also the real fragment-stage
    /// alpha-compare gate: `other_mode.alpha_compare()` selects
    /// `FragmentAlphaCompareParams::mode`, guaranteed `None` or `Threshold`
    /// by the retrieval-time rejection of `Reserved`/`Dither` upstream
    /// (`raw_dpc::triangle_draw_data`/`production.rs`'s `PlanCollector`) --
    /// this call site asserts-unreachable on the other two for
    /// defense-in-depth, matching this crate's "loud trap even for
    /// should-be-unreachable state" convention, rather than re-deriving its
    /// own `Reserved`/`Dither` match arms. `blend_color` is the plan's own
    /// real `G_SETBLENDCOLOR` snapshot (`Some` only for `Threshold`, `None`
    /// for `None` mode -- see `RetrievedTriangleDraw::blend_color`'s doc).
    /// `raster_params`/`extent` are caller-supplied because they describe
    /// the render target/viewport, not RDP command state this card's
    /// admission mechanism carries.
    ///
    /// `resolution`/`screen_scale`/`screen_offset`/depth conversion happen
    /// inside the vertex shader itself, not here -- `vertices`' `position`
    /// stays raw RDP screen-pixel `x`/`y`/`z`/`w`, matching
    /// `triangle_pipeline_vertex.wgsl`'s own module doc.
    ///
    /// `other_mode.depth_compare_enabled()`/`depth_update_enabled()`
    /// (production depth-slice task card, `Z_CMP`/`Z_UPD` pipeline-variant
    /// depth gating) select this draw's precreated pipeline variant verbatim
    /// -- see [`depth_pipeline_index`]. `other_mode.depth_mode()` is not
    /// read here or anywhere in this module: every `DepthMode` value gets
    /// exactly the plain hardware `Less`/write-toggle behavior this slice
    /// implements, no mode-specific dispatch (nonclaims, module doc).
    ///
    /// `other_mode`'s six coverage bits (production coverage node 1) feed
    /// the real fragment-stage `cvg_dst`/coverage-alpha uniform verbatim --
    /// `Save`, and `Clamp`/`Wrap` with `image_read_enabled` set, panic here
    /// (via [`fragment_coverage_params_bytes`] in `submit_triangles`) rather
    /// than reach the GPU with a memory-coverage value this pipeline cannot
    /// honestly supply.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_admitted_triangle(
        &mut self,
        vertices: [NeutralTriangleVertex; 3],
        other_mode: OtherMode,
        combine_params: CombineParams,
        raster_params: TriangleRasterParams,
        extent: TriangleTargetExtent,
        tmem: TmemGpuProjection,
        tile_binding: TileBindingParams,
        blend_color: Color4,
        env_color: Color4,
        prim_color: PrimColor,
        blend_params: ResolvedFragmentBlendParams,
        is_rect: bool,
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        let fixture = admitted_triangle_fixture(
            vertices,
            other_mode,
            combine_params,
            raster_params,
            extent,
            tmem,
            tile_binding,
            blend_color,
            env_color,
            prim_color,
            blend_params,
            is_rect,
        );
        self.submit_triangle(fixture)
    }

    /// Submits one or more fixed-fixture triangle draws, in order, into ONE
    /// shared per-submission color+depth texture pair -- the color
    /// attachment clears once (`LoadOp::Clear`) before the first draw, then
    /// every subsequent draw in the same render pass uses `LoadOp::Load` on
    /// both attachments, so later draws' real GPU depth test competes
    /// against earlier draws' committed depth/color, not a fresh buffer.
    /// This is the shape port card §6's depth differential needs ("a second
    /// triangle drawn behind/in-front of the first") -- `submit_triangle`'s
    /// single-fixture call is a degenerate one-draw case of this method, not
    /// a separately-implemented path.
    ///
    /// # Panics
    /// If `fixtures` is empty (a caller bug, not a runtime/device condition
    /// -- there is no meaningful "submit nothing" draw).
    pub fn submit_triangles(
        &mut self,
        fixtures: &[TriangleFixture],
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        assert!(
            !fixtures.is_empty(),
            "submit_triangles requires at least one fixture"
        );
        let error_count = self.errors.count();
        if error_count != 0 {
            return Err(TrianglePipelineError::DevicePoisoned {
                count: error_count,
                first: self.errors.first(),
            });
        }

        let extent = fixtures[0].extent;
        for fixture in fixtures {
            validate_triangle_extent(fixture.extent)?;
            if fixture.extent != extent {
                return Err(TrianglePipelineError::MixedExtent {
                    first: extent,
                    other: fixture.extent,
                });
            }
        }
        // wgpu's `copy_texture_to_buffer` requires each row to start at a
        // `COPY_BYTES_PER_ROW_ALIGNMENT`-aligned offset; this slice's 8x8/
        // 16x16 fixed targets (port card §3) have an unaligned natural row
        // stride (e.g. 8 pixels * 4 bytes = 32), so the readback buffers use
        // a padded stride and `complete()` strips the padding back out. The
        // framebuffer-color snapshot buffer (Slice B) reuses this exact
        // padded stride, in words, so the WGSL side and the host side never
        // have two independent notions of "row width" (row-stride
        // correctness fix).
        let unpadded_bytes_per_row = extent.width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row_stride_words = padded_bytes_per_row / 4;
        let color_bytes = u64::from(padded_bytes_per_row) * u64::from(extent.height);
        let depth_bytes = u64::from(padded_bytes_per_row) * u64::from(extent.height);
        let snapshot_bytes = color_bytes;

        // Each draw gets its own vertex buffer and its own raster/combine/
        // tmem/tile-binding uniform+storage buffers + bind group:
        // mid-render-pass `queue.write_buffer` calls are not safe against a
        // buffer already bound by an in-flight pass, so per-draw uniforms
        // must be distinct resources written before the pass opens, not
        // one shared buffer rewritten between draws. Binding 9's resource is
        // NOT built here -- it depends on which pass-splitting run this
        // fixture falls into (see below), so it is added to each bind group
        // when that run is processed.
        struct DrawResources {
            vertex_buffer: wgpu::Buffer,
            // This draw's precreated pipeline-variant index (production
            // depth-slice task card §3), from this fixture's own
            // `depth_compare_enabled`/`depth_update_enabled` -- selected per
            // draw at `pass.set_pipeline`, not once for the whole pass.
            depth_pipeline_index: usize,
            reads_framebuffer_color: bool,
            raster_params_buffer: wgpu::Buffer,
            combine_params_buffer: wgpu::Buffer,
            tmem_bytes_buffer: wgpu::Buffer,
            tmem_validity_buffer: wgpu::Buffer,
            tile_binding_buffer: wgpu::Buffer,
            alpha_compare_params_buffer: wgpu::Buffer,
            coverage_params_buffer: wgpu::Buffer,
            material_params_buffer: wgpu::Buffer,
            blend_params_buffer: wgpu::Buffer,
        }
        // Grow the pool to this submission's high-water mark. Steady state
        // reuses every slot, so this allocates only when a frame needs more
        // fixtures than any frame before it.
        while self.fixture_buffers.len() < fixtures.len() {
            self.fixture_buffers.push(FixtureBuffers::new(&self.device));
        }

        let mut draws = Vec::with_capacity(fixtures.len());
        for (fixture_index, fixture) in fixtures.iter().enumerate() {
            let slot = &self.fixture_buffers[fixture_index];
            let mut vertex_bytes = Vec::with_capacity(3 * 40);
            for vertex in fixture.vertices {
                vertex_bytes.extend_from_slice(&vertex.to_bytes());
            }
            debug_assert_eq!(
                vertex_bytes.len() as u64,
                VERTEX_BUFFER_BYTES,
                "the pooled vertex buffer is sized for exactly one triangle"
            );
            let vertex_buffer = &slot.vertex;
            self.queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);

            let raster_params_buffer = &slot.raster_params;
            self.queue.write_buffer(
                &raster_params_buffer,
                0,
                &raster_params_bytes(fixture.raster_params, fixture.is_rect),
            );
            let combine_params_buffer = &slot.combine_params;
            let combine_bytes = fragment_combine_params_bytes(fixture.combine_params);
            self.queue
                .write_buffer(&combine_params_buffer, 0, &combine_bytes);

            let tmem_bytes_buffer = &slot.tmem_bytes;
            self.queue
                .write_buffer(&tmem_bytes_buffer, 0, &fixture.tmem.byte_words_bytes());
            let tmem_validity_buffer = &slot.tmem_validity;
            self.queue.write_buffer(
                &tmem_validity_buffer,
                0,
                &fixture.tmem.validity_words_bytes(),
            );
            let tile_binding_buffer = &slot.tile_binding;
            self.queue
                .write_buffer(&tile_binding_buffer, 0, &fixture.tile_binding.to_bytes());

            let alpha_compare_params_buffer = &slot.alpha_compare_params;
            let alpha_compare_bytes = fragment_alpha_compare_params_bytes(
                fixture.alpha_compare_mode,
                fixture.blend_color,
            );
            self.queue
                .write_buffer(&alpha_compare_params_buffer, 0, &alpha_compare_bytes);

            let coverage_params_buffer = &slot.coverage_params;
            let coverage_bytes = fragment_coverage_params_bytes(
                fixture.coverage_destination,
                fixture.image_read_enabled,
                fixture.force_blend,
                fixture.antialias_enabled,
                fixture.coverage_times_alpha,
                fixture.alpha_coverage_select,
            );
            self.queue
                .write_buffer(&coverage_params_buffer, 0, &coverage_bytes);

            let material_params_buffer = &slot.material_params;
            let material_bytes =
                fragment_material_params_bytes(fixture.env_color, fixture.prim_color);
            self.queue
                .write_buffer(&material_params_buffer, 0, &material_bytes);

            let blend_params_buffer = &slot.blend_params;
            let blend_params_bytes =
                fragment_blend_params_bytes(fixture.blend_params, row_stride_words);
            self.queue
                .write_buffer(&blend_params_buffer, 0, &blend_params_bytes);

            draws.push(DrawResources {
                vertex_buffer: vertex_buffer.clone(),
                depth_pipeline_index: depth_pipeline_index(
                    fixture.depth_compare_enabled,
                    fixture.depth_update_enabled,
                ),
                reads_framebuffer_color: fixture.blend_params.reads_framebuffer_color,
                raster_params_buffer: raster_params_buffer.clone(),
                combine_params_buffer: combine_params_buffer.clone(),
                tmem_bytes_buffer: tmem_bytes_buffer.clone(),
                tmem_validity_buffer: tmem_validity_buffer.clone(),
                tile_binding_buffer: tile_binding_buffer.clone(),
                alpha_compare_params_buffer: alpha_compare_params_buffer.clone(),
                coverage_params_buffer: coverage_params_buffer.clone(),
                material_params_buffer: material_params_buffer.clone(),
                blend_params_buffer: blend_params_buffer.clone(),
            });
        }

        let color_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64-triangle-pipeline-color"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64-triangle-pipeline-depth"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let status_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64-triangle-pipeline-tmem-sample-status"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STATUS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let status_view = status_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let color_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-color-readback"),
            size: color_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let depth_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-depth-readback"),
            size: depth_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // `R32Uint` is 4 bytes/texel, same as color/depth -- reuses the same
        // padded-row-stride math.
        let status_bytes = u64::from(padded_bytes_per_row) * u64::from(extent.height);
        let status_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-tmem-sample-status-readback"),
            size: status_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-triangle-pipeline-submit"),
            });

        // Framebuffer-blend Slice B: partition the draws into maximal
        // contiguous runs, one render pass per run, with every
        // framebuffer-color-reading fixture a singleton run of its own (see
        // `split_fixture_runs`'s own doc) -- a reading fixture must snapshot
        // the color attachment *after* every earlier-ordered draw has landed
        // and *before* its own draw runs, impossible within an
        // already-open pass. Every fixture still shares the one clear (only
        // the first run clears; every later run uses `LoadOp::Load` on all
        // three attachments) and the one final readback below, exactly as
        // the prior single-pass code did -- only the number of passes that
        // produced the color texture's content changes, not how it is read
        // back.
        let reads_framebuffer_color: Vec<bool> = draws
            .iter()
            .map(|draw| draw.reads_framebuffer_color)
            .collect();
        let runs = split_fixture_runs(&reads_framebuffer_color);
        // Kept alive until `encoder.finish()`: each reading run's snapshot
        // buffer is bound by that run's bind group(s) via `as_entire_binding`.
        let mut snapshots: Vec<FramebufferColorSnapshot> = Vec::new();
        // Kept alive until `encoder.finish()`: every bind group built below.
        let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
        for (run_index, &(start, end)) in runs.iter().enumerate() {
            let run_reads_framebuffer_color = reads_framebuffer_color[start];
            let framebuffer_color_binding: wgpu::BindingResource<'_> =
                if run_reads_framebuffer_color {
                    // This run's one fixture reads framebuffer color: snapshot
                    // the live color attachment (already
                    // `RENDER_ATTACHMENT | COPY_SRC`) at `padded_bytes_per_row`
                    // stride before opening this run's pass, so the shader
                    // reads every earlier-ordered draw's committed output.
                    let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("fn64-triangle-pipeline-framebuffer-color-snapshot"),
                        size: snapshot_bytes,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: &color_texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(padded_bytes_per_row),
                                rows_per_image: Some(extent.height),
                            },
                        },
                        wgpu::Extent3d {
                            width: extent.width,
                            height: extent.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    snapshots.push(FramebufferColorSnapshot { buffer });
                    snapshots
                        .last()
                        .expect("just pushed")
                        .buffer
                        .as_entire_binding()
                } else {
                    self.framebuffer_color_dummy_buffer.as_entire_binding()
                };

            for resources in &draws[start..end] {
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("fn64-triangle-pipeline-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: resources.raster_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: resources.combine_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: resources.tmem_bytes_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: resources.tmem_validity_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: resources.tile_binding_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: resources.alpha_compare_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: resources.coverage_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: resources.material_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: resources.blend_params_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: framebuffer_color_binding.clone(),
                        },
                    ],
                });
                bind_groups.push(bind_group);
            }

            let is_first_run = run_index == 0;
            let load_op_color = if is_first_run {
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
            } else {
                wgpu::LoadOp::Load
            };
            let load_op_depth = if is_first_run {
                wgpu::LoadOp::Clear(1.0)
            } else {
                wgpu::LoadOp::Load
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fn64-triangle-pipeline-pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: load_op_color,
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    // The status attachment's `LoadOp::Load` on every
                    // non-first run is load-bearing: `complete()`'s
                    // `status_readback` copies the status texture to the CPU
                    // exactly once, after every run's pass has ended, so a
                    // `LoadOp::Clear` here on any non-first run would
                    // silently discard every prior run's
                    // `tmem_sample_status` writes.
                    Some(wgpu::RenderPassColorAttachment {
                        view: &status_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: load_op_color,
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: load_op_depth,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for (resources, bind_group) in draws[start..end].iter().zip(&bind_groups[start..end]) {
                pass.set_pipeline(&self.pipelines[resources.depth_pipeline_index]);
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_vertex_buffer(0, resources.vertex_buffer.slice(..));
                pass.draw(0..3, 0..1);
            }
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &color_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &depth_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &depth_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &status_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &status_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let submission = self.queue.submit([encoder.finish()]);

        Ok(InFlightTriangleDraw {
            renderer: self,
            extent,
            padded_bytes_per_row,
            color_readback,
            depth_readback,
            status_readback,
            submission,
            callback_receiver,
        })
    }
}

pub struct InFlightTriangleDraw<'renderer> {
    renderer: &'renderer mut TrianglePipelineRenderer,
    extent: TriangleTargetExtent,
    padded_bytes_per_row: u32,
    color_readback: wgpu::Buffer,
    depth_readback: wgpu::Buffer,
    status_readback: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    callback_receiver: mpsc::Receiver<()>,
}

/// Readback: RGBA8 color bytes (`Rgba8Unorm`), per-pixel depth as `f32`
/// (`Depth32Float`), and per-pixel `tmem_sample.wgsl`
/// `TMEM_SAMPLE_STATUS_*` codes (`R32Uint`) -- row-major, `extent.width *
/// extent.height` pixels each. `tmem_sample_status` is the observable
/// shader-failure-status channel (card audit repair): every fragment this
/// draw covered wrote its own status here, so a caller can distinguish
/// `TMEM_SAMPLE_STATUS_OK` everywhere from any fragment reporting a named
/// sampling failure, without guessing from the color alone.
pub struct TriangleDrawOutput {
    pub extent: TriangleTargetExtent,
    pub color_rgba8: Vec<u8>,
    pub depth_f32: Vec<f32>,
    pub tmem_sample_status: Vec<u32>,
}

impl InFlightTriangleDraw<'_> {
    pub fn complete(self) -> Result<TriangleDrawOutput, TrianglePipelineError> {
        self.renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        self.callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| TrianglePipelineError::CompletionCallbackNotObserved)?;

        let color_padded = map_and_read(&self.renderer.device, &self.color_readback)?;
        let depth_padded = map_and_read(&self.renderer.device, &self.depth_readback)?;
        let status_padded = map_and_read(&self.renderer.device, &self.status_readback)?;

        let error_count = self.renderer.errors.count();
        if error_count != 0 {
            return Err(TrianglePipelineError::DevicePoisoned {
                count: error_count,
                first: self.renderer.errors.first(),
            });
        }

        let unpadded_bytes_per_row = self.extent.width as usize * 4;
        let color_rgba8 = strip_row_padding(
            &color_padded,
            self.padded_bytes_per_row as usize,
            unpadded_bytes_per_row,
            self.extent.height as usize,
        );
        let depth_bytes = strip_row_padding(
            &depth_padded,
            self.padded_bytes_per_row as usize,
            unpadded_bytes_per_row,
            self.extent.height as usize,
        );
        let status_bytes = strip_row_padding(
            &status_padded,
            self.padded_bytes_per_row as usize,
            unpadded_bytes_per_row,
            self.extent.height as usize,
        );

        let mut depth_f32 = Vec::with_capacity(depth_bytes.len() / 4);
        for chunk in depth_bytes.chunks_exact(4) {
            depth_f32.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        let mut tmem_sample_status = Vec::with_capacity(status_bytes.len() / 4);
        for chunk in status_bytes.chunks_exact(4) {
            tmem_sample_status.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }

        Ok(TriangleDrawOutput {
            extent: self.extent,
            color_rgba8,
            depth_f32,
            tmem_sample_status,
        })
    }
}

fn strip_row_padding(
    padded: &[u8],
    padded_bytes_per_row: usize,
    unpadded_bytes_per_row: usize,
    height: usize,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(unpadded_bytes_per_row * height);
    for row in 0..height {
        let start = row * padded_bytes_per_row;
        output.extend_from_slice(&padded[start..start + unpadded_bytes_per_row]);
    }
    output
}

fn map_and_read(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
) -> Result<Vec<u8>, TrianglePipelineError> {
    map_and_read_prefix(device, buffer, buffer.size())
}

fn map_and_read_prefix(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    bytes: u64,
) -> Result<Vec<u8>, TrianglePipelineError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let slice = buffer.slice(..bytes);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.try_send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(POLL_TIMEOUT),
        })
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    receiver
        .recv_timeout(CALLBACK_TIMEOUT)
        .map_err(|_| TrianglePipelineError::Readback("map callback timeout".into()))?
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let mapped = slice
        .get_mapped_range()
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let output = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(output)
}

fn map_and_read_prefixes(
    device: &wgpu::Device,
    buffers: &[(&wgpu::Buffer, u64)],
) -> Result<Vec<Vec<u8>>, TrianglePipelineError> {
    let mut receivers = Vec::with_capacity(buffers.len());
    for &(buffer, bytes) in buffers {
        let (sender, receiver) = mpsc::sync_channel(1);
        buffer
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.try_send(result);
            });
        receivers.push(receiver);
    }
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(POLL_TIMEOUT),
        })
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let mut outputs = Vec::with_capacity(buffers.len());
    for ((buffer, bytes), receiver) in buffers.iter().zip(receivers) {
        receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| TrianglePipelineError::Readback("map callback timeout".into()))?
            .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
        let slice = buffer.slice(..*bytes);
        let mapped = slice
            .get_mapped_range()
            .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
        outputs.push(mapped.to_vec());
        drop(mapped);
        buffer.unmap();
    }
    Ok(outputs)
}

/// `OtherMode` this slice's fixed fixture always uses: wire `(0, 0)` decodes
/// to `CycleType::OneCycle` (see `state.rs`'s wire-decode table) with
/// `primitive_depth_source()` and `force_blend()` both false -- no Z-override
/// branch, no blend-force -- matching the restriction set (§3) without
/// invoking `blend.rs`'s runtime OtherMode dispatch for this slice (see
/// module doc's blend-no-op note).
pub const fn fixed_fixture_other_mode() -> OtherMode {
    OtherMode::from_wire(0, 0)
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrianglePipelineError {
    RequestAdapter(String),
    RequestDevice(String),
    PipelinePrewarm(String),
    DevicePoisoned {
        count: usize,
        first: Option<String>,
    },
    ZeroExtent {
        width: u32,
        height: u32,
    },
    MixedExtent {
        first: TriangleTargetExtent,
        other: TriangleTargetExtent,
    },
    Rgba16TargetSizeOverflow {
        width: u32,
        height: u32,
    },
    Rgba16TargetTooLarge {
        width: u32,
        height: u32,
    },
    Rgba16TargetByteLength {
        expected: usize,
        actual: usize,
    },
    EmptyCoverageBatch,
    CoverageSizeOverflow,
    CoverageTooLarge,
    ComputeColorTargetLength {
        expected: usize,
        actual: usize,
    },
    TimestampQueryUnsupported {
        adapter: String,
    },
    ComputeColorDispatchRows {
        first_row: u32,
        row_count: u32,
        height: u32,
    },
    ComputeColorDispatchColumns {
        first_column: u32,
        column_count: u32,
        width: u32,
    },
    ComputeColorBatchTmemStatus {
        batch: usize,
        pixel: usize,
        status: u32,
    },
    ComputeColorStatusState {
        first_dispatch: usize,
        state_index: u32,
        state_count: usize,
    },
    ComputeColorCheckpointOrder {
        checkpoint: usize,
        previous: usize,
        dispatch_limit: usize,
        dispatches: usize,
    },
    ComputeColorCheckpointMissingFinal {
        final_checkpoint: usize,
        dispatches: usize,
    },
    ExactSubmissionWait(String),
    CompletionCallbackNotObserved,
    Readback(String),
}

impl fmt::Display for TrianglePipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestAdapter(reason) => {
                write!(formatter, "wgpu adapter request failed: {reason}")
            }
            Self::RequestDevice(reason) => {
                write!(formatter, "wgpu device request failed: {reason}")
            }
            Self::PipelinePrewarm(reason) => {
                write!(formatter, "triangle-pipeline prewarm failed: {reason}")
            }
            Self::DevicePoisoned { count, first } => write!(
                formatter,
                "triangle-pipeline device recorded {count} uncaptured errors; first={first:?}"
            ),
            Self::ZeroExtent { width, height } => {
                write!(formatter, "triangle-pipeline target extent is empty: {width}x{height}")
            }
            Self::MixedExtent { first, other } => write!(
                formatter,
                "triangle-pipeline batch has mixed target extents: {}x{} vs {}x{}",
                first.width, first.height, other.width, other.height
            ),
            Self::Rgba16TargetSizeOverflow { width, height } => write!(
                formatter,
                "packed RGBA16 target size overflows host addressing: {width}x{height}"
            ),
            Self::Rgba16TargetTooLarge { width, height } => write!(
                formatter,
                "packed RGBA16 target exceeds one-dimensional compute dispatch limits: {width}x{height}"
            ),
            Self::Rgba16TargetByteLength { expected, actual } => write!(
                formatter,
                "packed RGBA16 target byte length mismatch: expected {expected}, got {actual}"
            ),
            Self::EmptyCoverageBatch => {
                formatter.write_str("compute triangle coverage batch is empty")
            }
            Self::CoverageSizeOverflow => {
                formatter.write_str("compute triangle coverage output size overflows")
            }
            Self::CoverageTooLarge => {
                formatter.write_str("compute triangle coverage batch exceeds device limits")
            }
            Self::ComputeColorTargetLength { expected, actual } => write!(
                formatter,
                "compute color target byte length mismatch: expected {expected}, got {actual}"
            ),
            Self::TimestampQueryUnsupported { adapter } => write!(
                formatter,
                "compute GPU timing requested but adapter {adapter:?} lacks timestamp queries"
            ),
            Self::ComputeColorDispatchRows {
                first_row,
                row_count,
                height,
            } => write!(
                formatter,
                "compute color dispatch row band {first_row}+{row_count} exceeds target height {height}"
            ),
            Self::ComputeColorDispatchColumns {
                first_column,
                column_count,
                width,
            } => write!(
                formatter,
                "compute color dispatch column band {first_column}+{column_count} exceeds target width {width}"
            ),
            Self::ComputeColorBatchTmemStatus {
                batch,
                pixel,
                status,
            } => write!(
                formatter,
                "compute color batch {batch} TMEM sample failed at pixel {pixel} with status {status}"
            ),
            Self::ComputeColorStatusState {
                first_dispatch,
                state_index,
                state_count,
            } => write!(
                formatter,
                "compute color status named state {state_index} outside {state_count} states at \
                 dispatch {first_dispatch}"
            ),
            Self::ComputeColorCheckpointOrder {
                checkpoint,
                previous,
                dispatch_limit,
                dispatches,
            } => write!(
                formatter,
                "compute color checkpoint #{checkpoint} ends at dispatch {dispatch_limit}, which \
                 is not strictly after {previous} within the {dispatches}-dispatch chain"
            ),
            Self::ComputeColorCheckpointMissingFinal {
                final_checkpoint,
                dispatches,
            } => write!(
                formatter,
                "compute color checkpoints end at dispatch {final_checkpoint}, not the chain's \
                 final dispatch {dispatches}"
            ),
            Self::ExactSubmissionWait(reason) => {
                write!(formatter, "exact triangle-pipeline submission wait failed: {reason}")
            }
            Self::CompletionCallbackNotObserved => formatter.write_str(
                "triangle-pipeline completion callback was not observable after exact submission wait",
            ),
            Self::Readback(reason) => write!(formatter, "triangle-pipeline readback failed: {reason}"),
        }
    }
}

impl std::error::Error for TrianglePipelineError {}

#[cfg(test)]
mod tests;
