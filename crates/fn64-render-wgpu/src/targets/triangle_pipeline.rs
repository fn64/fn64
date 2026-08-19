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
//! `AlphaCompare::Reserved`/`Dither` mode, likewise a named panic.
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
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::device::{HeadlessBackend, NoAdapter};
use crate::shader_manifest::{
    triangle_pipeline_fragment_wgsl, TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT,
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
        AlphaCompare::Reserved | AlphaCompare::Dither => unreachable!(
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
        unsupported @ (AlphaCompare::Reserved | AlphaCompare::Dither) => unreachable!(
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
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-triangle-pipeline"),
                required_features: wgpu::Features::empty(),
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
                framebuffer_color_dummy_buffer,
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
    /// Binding 9's shared dummy resource for every fixture in a submission
    /// whose `reads_framebuffer_color` is false -- one always-allocated
    /// buffer reused across every non-reading fixture and every submission,
    /// never a per-fixture or per-submission allocation.
    framebuffer_color_dummy_buffer: wgpu::Buffer,
    errors: Arc<BoundedErrorSink>,
}

impl TrianglePipelineRenderer {
    pub const fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
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
        let mut draws = Vec::with_capacity(fixtures.len());
        for fixture in fixtures {
            let mut vertex_bytes = Vec::with_capacity(3 * 40);
            for vertex in fixture.vertices {
                vertex_bytes.extend_from_slice(&vertex.to_bytes());
            }
            let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);

            let raster_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-raster-params"),
                size: RASTER_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(
                &raster_params_buffer,
                0,
                &raster_params_bytes(fixture.raster_params, fixture.is_rect),
            );
            let combine_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-combine-params"),
                size: COMBINE_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let combine_bytes = fragment_combine_params_bytes(fixture.combine_params);
            self.queue
                .write_buffer(&combine_params_buffer, 0, &combine_bytes);

            let tmem_bytes_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-tmem-bytes"),
                size: TMEM_BYTES_BUFFER_SIZE,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&tmem_bytes_buffer, 0, &fixture.tmem.byte_words_bytes());
            let tmem_validity_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-tmem-validity"),
                size: TMEM_VALIDITY_BUFFER_SIZE,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(
                &tmem_validity_buffer,
                0,
                &fixture.tmem.validity_words_bytes(),
            );
            let tile_binding_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-tile-binding"),
                size: TILE_BINDING_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&tile_binding_buffer, 0, &fixture.tile_binding.to_bytes());

            let alpha_compare_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-alpha-compare-params"),
                size: ALPHA_COMPARE_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let alpha_compare_bytes = fragment_alpha_compare_params_bytes(
                fixture.alpha_compare_mode,
                fixture.blend_color,
            );
            self.queue
                .write_buffer(&alpha_compare_params_buffer, 0, &alpha_compare_bytes);

            let coverage_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-coverage-params"),
                size: COVERAGE_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
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

            let material_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-material-params"),
                size: MATERIAL_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let material_bytes =
                fragment_material_params_bytes(fixture.env_color, fixture.prim_color);
            self.queue
                .write_buffer(&material_params_buffer, 0, &material_bytes);

            let blend_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-blend-params"),
                size: BLEND_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let blend_params_bytes =
                fragment_blend_params_bytes(fixture.blend_params, row_stride_words);
            self.queue
                .write_buffer(&blend_params_buffer, 0, &blend_params_bytes);

            draws.push(DrawResources {
                vertex_buffer,
                depth_pipeline_index: depth_pipeline_index(
                    fixture.depth_compare_enabled,
                    fixture.depth_update_enabled,
                ),
                reads_framebuffer_color: fixture.blend_params.reads_framebuffer_color,
                raster_params_buffer,
                combine_params_buffer,
                tmem_bytes_buffer,
                tmem_validity_buffer,
                tile_binding_buffer,
                alpha_compare_params_buffer,
                coverage_params_buffer,
                material_params_buffer,
                blend_params_buffer,
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
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
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
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let output = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(output)
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
