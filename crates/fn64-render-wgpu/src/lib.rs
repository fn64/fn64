//! Pure-Rust wgpu renderer ownership for fn64.
//!
//! M4.1 adds typed, transaction-local texture-image, tile, tile-size, sync,
//! block, tile, and TLUT load decoding with exact M4.0-owned source-read
//! identities. M4.2.0 binds LoadBlock and LoadTile to ordered 64-bit transfer
//! words plus a separate canonical sorted/disjoint physical-TMEM destination
//! union. Undefined word padding remains distinct from defined source bytes;
//! direct four-bit loads, YUV destination planning, and LoadTLUT destination
//! execution stay loud gaps.
//! M4.2a adds renderer-owned 4 KiB physical TMEM with per-byte validity and
//! last-touch generation. One move-only packet transaction chains every exact
//! admitted LoadBlock/LoadTile, snapshots each load's canonical device-local
//! effect before later overlaps, and publishes one final generation only after
//! exact GPU-effect and guest-commit lifecycle evidence. Physical lane bytes
//! can be asserted only inside this crate, carry an explicit eight-lane
//! defined mask, and are move-only/exact-load-bound; M4.2b and M4.2c own the
//! captured-source mapping for LoadTile and LoadBlock respectively.
//! M4.2b executes one exact checked LoadTile and M4.2c executes one exact
//! checked LoadBlock from the submitted packet's owned guest-read set into
//! that M4.2a transaction. Preparation binds every
//! global source access index, operation, range, and byte count; execution is
//! complete 64-bit words, never spills a source tail, exchanges linear
//! odd-row halves, and maps RGBA32 texel channels into the low/high 2 KiB
//! banks. Each executor returns only transaction-local state and ordered physical
//! fragment descriptors. Durable publication and backend reports remain
//! later owners.
//! M4.3.3a adds `RawTexel`, a format-neutral raw-value carrier reusable by
//! later CI/TLUT and YUV layers, and `decode_direct_texel`, a pure,
//! allocation-free RGBA8888 decoder for the seven direct texel pairs (RGBA16,
//! RGBA32, IA4, IA8, IA16, I4, I8); it does not read TMEM, resolve a CI
//! palette, convert YUV, or touch a GPU.
//! M4.3.3b adds pure CI4/CI8 index normalization and typed TLUT-mode
//! resolution. Disabled CI aliases the normalized index to I8; enabled
//! RGBA16/IA16 modes return a lookup value consumed with one caller-supplied
//! big-endian 16-bit entry. It performs no physical TMEM read, validity,
//! generation, sampling, cache, shader, or production-dispatch work.
//! M4.3.3c adds `read_committed_texel`, a CPU-only reader over durable
//! `PhysicalTmemState`. It preflights through the pure decoders, takes
//! explicit first-row parity, requires every direct/index/TLUT footprint byte
//! to be valid, accepts valid bytes touched by earlier generations, and binds
//! the result to one captured state identity/generation. Enabled CI accepts
//! only canonical low-half sources and a conservative all-eight-valid/equal
//! quadricated TLUT subset; partial/unequal sample-lane behavior remains a
//! hardware-measurement frontier. It adds no coordinate normalization,
//! sampling, filtering, LOD, cache, WGSL/GPU, or production path.
//! M4.3.3d composes that reader with an integer-only CPU point-addressing
//! layer. It accepts already-quantized signed S10.5 coordinates, applies the
//! public tile shift and S10.2 origin, then clamp-before-mirror/mask addressing.
//! First-row parity remains explicit caller input. Float/perspective
//! quantization, copy addressing, filter selection and lanes, unequal TLUT
//! banks, cache, GPU, raster integration, parity, and performance remain out
//! of scope.
//! M4.3.3e extends the same integer path to the containing texture cell. It
//! exposes exact post-shift/post-origin five-bit fractions, addresses all four
//! semantic corners independently, and can gather four committed decoded
//! texels with corner-located failures and unchanged snapshot identity. It
//! does not select three-nearest corners or their exact validity footprint,
//! settle diagonal/tie behavior, trigger or round an average, interpolate
//! colors, fix filter accumulator width or reciprocal quantization, decode
//! filter state, implement copy mode, convert float/perspective coordinates,
//! infer first-row parity, relax unequal TLUT lanes, or add LOD, YUV, cache,
//! WGSL, or GPU work. It adds no production-DPC integration; primitive,
//! rectangle, or triangle decode; combiner, coverage, depth, blend, target,
//! or VI behavior; derivatives, detail/sharpen, or two-cycle selection;
//! full-ROM qualification; RT64 pixel parity; visual/silicon parity; or
//! performance claim.
//! M2.5.3a adds one repository-owned WGSL component for M4.3.3a's exact
//! seven direct texel conversions. Its closed manifest, baseline device
//! profile, Naga validation, and CPU oracle establish candidate mechanics.
//! The component remains `NotQualified` and `NativeUnverified`; no complete
//! RT64 shader-denominator row is promoted.
//! The RT64 color combiner port (characterization-first, source: MIT RT64
//! pinned commit `5473732a822a4423b5696e7cb18fecc425a59875`,
//! `docs/RT64-PORT-AUTHORITY.md`'s Rust-port source pin) adds typed
//! `ColorInput`/`AlphaInput`/`CombineParams` selector decode, exact and
//! complete for every wire-legal index (matching RT64 bit-for-bit); full
//! one-cycle `(A-B)*C+D` arithmetic evaluating every selector either enum
//! can hold — including KEY_CENTER/KEY_SCALE/K4/K5, NOISE,
//! LOD_FRACTION/PRIM_LOD_FRAC, and the `*_ALPHA`/COMBINED_ALPHA cross-reads
//! — as caller-supplied typed [`CombinerInputs`] fields, not a PRNG or
//! derivative computed here; and full two-cycle mode
//! ([`run_combiner`]/[`run_two_cycle`]) with cycle-0-then-cycle-1 ordering,
//! `COMBINED`/`COMBINED_ALPHA` cross-cycle reads, the `TEXEL0`/`TEXEL1`
//! cycle-1 swap, the `twoCycle`-conditioned pre-arithmetic
//! `wrapInputC`/`wrapInputABD` cross-cycle-carry wrap (range chosen
//! independently per channel by that channel's own slot-C selector), and
//! `alphaCompareValue`'s cycle-0 capture timing. An independently-derived
//! Rust oracle ([`run_one_cycle`], [`run_combiner`]) with a matching owned
//! WGSL transcription (`shaders/color_combiner.wgsl`, Naga-validated). It
//! adds no copy mode, shader-keying, `RdpState`/`SetCombine` wiring,
//! draw-path integration, real NOISE/LOD generation, or native/GPU-verified
//! behavior.
//! Hardware fields and transfer rules come from the public SGI *Nintendo 64
//! RDP Command Summary*, Tables 1, 3, and 6–10, public Programming Manual
//! section 13.9, and the public libultra `gbi.h` `gDPLoadTLUTCmd` macro. RT64
//! is not hardware authority for this slice. M4.2a does not execute guest
//! reads, assemble backend reports, issue lifecycle receipts, migrate
//! production dispatch, or establish parity or performance.
//! M4.3.2 adds a LoadTLUT executor (`tmem/execute/load_tlut.rs`) alongside
//! M4.2b/M4.2c's LoadTile/LoadBlock executors: it consumes one checked
//! LoadTLUT transfer and the submitted packet's owned guest reads, then
//! quadricates each entry's 2 captured source bytes into all four 16-bit
//! lanes of its high-bank destination word through the same M4.2a
//! move-only staged-transaction API. Like its siblings it returns only
//! transaction-local state; it does not publish durable bytes or issue a
//! lifecycle receipt.
//!
//! T3 Phase A adds `PendingTmemTransaction::into_physical_successor`, the
//! inactive-slot `next_physical` shape `fn64_render::RawDpcCoordinator::
//! complete_execution` needs. T3 Phase B (`production` module) adds the
//! concrete `WgpuBackend`: it owns that coordinator, plans through T1's real
//! decoder/push loop (`raw_dpc::production_adapter`), executes a sealed
//! `BoundSubmittedRawDpc` using only its authority-scoped `execution_view`
//! (never the private decoder's own `SubmittedTicket`/`BoundTmemTransfer`),
//! and publishes as exactly `self.coordinator.prepare_publication(
//! publication).commit()`. Scope is TMEM-only, no-FullSync, no-guest-write,
//! headless; `process_task`/`present` are honest named rejections, not
//! general gfx-task/presentation support. See `docs/DESIGN.md`'s "T3 Phase
//! A/B" section for the full account.
//!
//! `raw_dpc::triangle` decodes all eight raw RDP triangle opcodes
//! (`0x08..=0x0f`) into `RawTriangle`: tile/level/flip, signed YL/YM/YH,
//! signed Q16.16 XL/XH/XM and their three edge slopes, and every present raw
//! shade/texture/depth coefficient word (opaque `RawWord` pairs, no float
//! conversion). `RawDpcCommandKind::RawTriangle` wires it into
//! `decode_raw_dpc`; the T1 production adapter rejects it exactly like every
//! other command kind outside its TMEM/state subset -- loudly, never
//! silently. It adds no edge walk, rasterization, RDP state-machine
//! transition, clipping/scissor, texture sampling, combiner/blender/depth/
//! target write, GPU pipeline, production dispatch, or parity/performance
//! claim. See the crate README's "Raw RDP triangle command decode" section.
//!
//! Downstream callers cannot relabel logical source bytes as physical TMEM
//! lanes; the bounded assertion constructor is crate-private:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{StagedTmemTransaction, TmemTransferWord};
//! # fn staged() -> StagedTmemTransaction { unimplemented!() }
//! # fn word() -> TmemTransferWord { unimplemented!() }
//! let staged = staged();
//! let physical = staged.physical_word_payload(word(), [None; 8]);
//! # drop(physical);
//! ```
//!
//! A physical-lane payload cannot be reused for a second load or word:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{DefinedPhysicalTmemWordBytes, StagedTmemTransaction};
//! # fn staged() -> StagedTmemTransaction { unimplemented!() }
//! # fn physical() -> DefinedPhysicalTmemWordBytes { unimplemented!() }
//! let mut staged = staged();
//! let physical = physical();
//! let first = staged.stage_word(physical);
//! let second = staged.stage_word(physical);
//! # drop((first, second));
//! ```
//!
//! A prepared LoadTile is one-use operation authority:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{PreparedLoadTile, StagedTmemTransaction};
//! # fn prepared() -> PreparedLoadTile { unimplemented!() }
//! # fn staged() -> StagedTmemTransaction { unimplemented!() }
//! let prepared = prepared();
//! let first = prepared.execute(staged());
//! let second = prepared.execute(staged());
//! # drop((first, second));
//! ```
//!
//! Packet transactions and their pending publication owner are move-only:
//!
//! ```compile_fail
//! use fn64_render_wgpu::PendingTmemTransaction;
//! # fn pending() -> PendingTmemTransaction { unimplemented!() }
//! let pending = pending();
//! let duplicate = pending.clone();
//! # drop(duplicate);
//! ```
//!
//! M3.3c executes M3.3a's exact native 4x2 RGBA16 fill through a prewarmed
//! wgpu pipeline, M3.3b's typed target generations, and M3.3d's separately
//! typed bounded VI mechanism. The target and RDP state publish only after the
//! guest owner commits the exact backing-storage bytes. This remains one
//! synthetic mechanism: it is not a general raster/VI implementation, live VI
//! adapter, surface path, RT64 parity result, or performance claim.
//!
//! Submission and completion are type states. An in-flight operation has no
//! early receipt conversion:
//!
//! ```compile_fail
//! use fn64_render_wgpu::InFlightFill;
//! # fn in_flight() -> InFlightFill<'static> { unimplemented!() }
//! let in_flight = in_flight();
//! let completion = in_flight.into_completion();
//! # drop(completion);
//! ```
//!
//! Completed effects are move-only and cannot be published twice:
//!
//! ```compile_fail
//! use fn64_render_wgpu::WgpuBackendCompletion;
//! # fn completion() -> WgpuBackendCompletion { unimplemented!() }
//! let completion = completion();
//! let first = completion.into_parts();
//! let second = completion.into_parts();
//! # drop((first, second));
//! ```
//!
//! Transaction-local RDP state is likewise consumed by an explicit successor
//! decode and cannot be reused for another packet:
//!
//! ```compile_fail
//! use fn64_render_ir::SubmittedTicket;
//! use fn64_render_wgpu::{decode_raw_dpc_after, DecodedRawDpc};
//! # fn decoded() -> DecodedRawDpc { unimplemented!() }
//! # fn submitted() -> SubmittedTicket { unimplemented!() }
//! let staged = decoded().into_staged_state();
//! let next = decode_raw_dpc_after(submitted(), staged);
//! let stale = decode_raw_dpc_after(submitted(), staged);
//! # drop((next, stale));
//! ```
//!
//! A submitted ticket itself is also consumed by decode, so one submission
//! cannot mint two independent staged-state results:
//!
//! ```compile_fail
//! use fn64_render_ir::SubmittedTicket;
//! use fn64_render_wgpu::{decode_raw_dpc, RdpState};
//! # fn submitted() -> SubmittedTicket { unimplemented!() }
//! let submitted = submitted();
//! let first = decode_raw_dpc(submitted, &RdpState::default());
//! let duplicate = decode_raw_dpc(submitted, &RdpState::default());
//! # drop((first, duplicate));
//! ```
//!
//! Preparation does not grant public GPU-completion authority. Backend modules
//! inside this crate must own that transition:
//!
//! ```compile_fail
//! use fn64_render_wgpu::PreparedNativeFill;
//! # fn prepared() -> PreparedNativeFill<'static> { unimplemented!() }
//! let in_flight = prepared().begin();
//! # drop(in_flight);
//! ```
//!
//! The exclusive durable-state borrow remains live until the candidate is
//! dropped or guest commit succeeds, so no competing early publication fits:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{prepare_native_fill, DecodedRawDpc, NativeDurableState};
//! # fn decoded() -> DecodedRawDpc { unimplemented!() }
//! let mut durable = NativeDurableState::default();
//! let prepared = prepare_native_fill(decoded(), &mut durable).unwrap();
//! let early = durable.generation();
//! # drop((prepared, early));
//! ```
//!
//! Logical/device pixels and N64Recomp ABI backing-storage bytes are distinct
//! types, so the guest commit seam cannot accept unswizzled RGBA16 by accident:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{DeviceRgba16Bytes, N64RecompRdramStorageBytes};
//! # fn device_pixels() -> DeviceRgba16Bytes { unimplemented!() }
//! # fn abi_storage() -> N64RecompRdramStorageBytes { unimplemented!() }
//! # fn assemble_native_output(_: DeviceRgba16Bytes, _: N64RecompRdramStorageBytes) {}
//! assemble_native_output(abi_storage(), device_pixels());
//! ```
//!
//! Native completion is move-only, so one GPU observation cannot publish two
//! target generations:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{InFlightNativeRasterFill, NativeRasterError};
//! # fn in_flight() -> InFlightNativeRasterFill<'static, 'static> { unimplemented!() }
//! let in_flight = in_flight();
//! let first = in_flight.complete();
//! let second = in_flight.complete();
//! # let _: (Result<_, NativeRasterError>, Result<_, NativeRasterError>) = (first, second);
//! ```
//!
//! Blender (port card §1, characterization-first port of
//! `/private/tmp/rt64-blender-depth-port-card.md`) adds full one-cycle/
//! two-cycle selector and cycle semantics: [`blend::BlendColorInput`]/
//! [`blend::BlendAlphaInput`]/[`blend::BlendBInput`] selector resolution over
//! the already-landed `state::BlenderCycle` wire decode, sequential cycle
//! handoff, the no-`FORCE_BL` last-cycle bypass, the zero-factor divisor
//! collapse, and a loud [`blend::BlendImageReadError`] rather than a silent
//! fallback when a fragment reaches a framebuffer-dependent selector without
//! a supplied memory sample. [`blend::DualSourceBlendOutput`]/
//! [`blend::manual_blend_composite`] reuse the exact native-dual-source and
//! manual-fallback contract the accepted M2.2 Metal-execution evidence
//! proved executable (`probes/m2-wgpu-metal-headless`), rather than
//! inventing a third blend model. It adds no combiner, coverage, alpha
//! compare, depth, framebuffer resource binding/readback, raster primitive
//! execution, target storage, presentation, native adapter qualification,
//! full-ROM/pixel parity, or performance claim.
//!
//! `texture_gen` is a characterization-first literal port of RT64's
//! `normalizeSafe`/`computeTextureGen` (`src/shaders/TextureGen.hlsli:9-34`,
//! pinned commit `5473732a822a4423b5696e7cb18fecc425a59875`,
//! `docs/RT64-PORT-AUTHORITY.md`): safe vector normalization, the
//! row-vector-form `mul(lookAt-axis, worldMatrix)` transform (preserved
//! exactly as the pinned source calls it, not reconciled with the opposite
//! matrix-first convention used elsewhere in the same RT64 source tree), the
//! unconditional pre-branch `[-1,1]` clamp, both the linear
//! (`acos(-x) * 325.94932`) and non-linear (`(x+1) * 512`) modes, and the
//! final `(inputUV / 65536) * texgenUV` scale. An independently-derived Rust
//! oracle (`compute_texture_gen`) is matched by an owned, Naga-validated
//! WGSL transcription (`shaders/texture_gen.wgsl`); neither is compiled into
//! any pipeline or wired to a draw path. It adds no RSP lookat-matrix
//! derivation, world-matrix upload/storage-buffer plumbing, vertex-shader
//! integration, combiner/texture-sample consumption of the returned UV,
//! draw-path or production-DPC wiring, or RT64 visual/pixel/silicon parity
//! or performance claim.
#![forbid(unsafe_code)]

mod alpha_compare;
mod blend;
mod color_converter;
mod color_hlsli;
mod combiner;
mod coverage;
mod depth_encode;
mod depth_mode;
mod depth_strict_less;
mod device;
mod endian_swap;
mod fbcommon;
mod formats_dither;
mod lifecycle;
mod math_hlsli;
mod native_contract;
mod production;
mod random;
mod raster_vs;
mod raw_dpc;
mod rgb_dither;
mod rt64_blender_analysis;
mod rt64_blender_emulation;
mod rt64_common;
mod rt64_extended_gbi;
mod rt64_fb_reinterpret;
mod rt64_float4_quantize;
mod rt64_frame_compatibility;
mod rt64_framebuffer_geometry;
mod rt64_framebuffer_storage;
mod rt64_fullscreen_vs;
mod rt64_gaussian_filter;
mod rt64_gbi_extended_decode;
mod rt64_gbi_f3d;
mod rt64_gbi_f3d_variants;
mod rt64_gbi_f3dex;
mod rt64_gbi_f3dex2;
mod rt64_gbi_rdp_decode;
mod rt64_gbi_s2dex2;
mod rt64_light_estimation;
mod rt64_luminance_histogram;
mod rt64_math;
mod rt64_math_decompose;
mod rt64_math_matrix;
mod rt64_postprocess;
mod rt64_preset_draw_call_match;
mod rt64_replacement_resolve;
mod rt64_resample;
mod rt64_rigid_body;
mod rt64_rsp_matrix_stack;
mod rt64_rsp_patch;
mod rt64_rsp_segment;
mod rt64_rsp_smooth_normal;
mod rt64_rsp_world_modify;
mod rt64_texture_map_lru;
mod rt64_texture_sampler;
mod rt64_tmem_hasher;
mod rt64_tmem_regions;
mod rt64_user_configuration;
mod shader_manifest;
mod state;
mod targets;
mod texture_gen;
mod texture_lod;
mod tmem;
mod vi;

pub use alpha_compare::{
    alpha_compare_value, apply_alpha_dither, copy_alpha_compare_value,
    require_supported_alpha_compare, AlphaCompareNoise, CopyCycleSourceFormat,
    ALPHA_COMPARE_ENTRY_POINT, ALPHA_COMPARE_FRAGMENT_FN_WGSL, ALPHA_COMPARE_WGSL,
};
pub use blend::{
    blend_a, blend_b, blend_color, blend_fragment, dual_source_blend_output,
    manual_blend_composite, BlendAlphaInput, BlendBInput, BlendColorInput, BlendFramebufferSample,
    BlendImageReadError, BlendModeState, BlendedFragment, DualSourceBlendOutput,
    ResolvedBlendCycle, BLEND_ENTRY_POINT, BLEND_WGSL,
};
pub use combiner::{
    combiner_inputs_from_fragment_registers, run_combiner, run_one_cycle, run_two_cycle,
    AlphaInput, AlphaInputSlot, ColorInput, ColorInputSlot, CombineParams, CombinerCycleMode,
    CombinerInputs, COLOR_COMBINER_WGSL,
};
pub use coverage::{
    apply_coverage_alpha, attribute_sample, coverage_result, AttributeSamplePoint, Coverage,
    CoverageMask, CoverageModeBits, CoverageResult, CoveredAttributeSample, COVERAGE_ENTRY_POINT,
    COVERAGE_FRAGMENT_FN_WGSL, COVERAGE_SAMPLES, COVERAGE_WGSL,
};
pub use depth_mode::{
    depth_mode_decision, mode_passes, relations, DepthModeDecision, DepthRelations,
    DEPTH_MODE_ENTRY_POINT, DEPTH_MODE_WGSL,
};
pub use depth_strict_less::{
    strict_less_depth_test, strict_less_depth_write, StrictLessDepthOutcome, StrictLessDepthSample,
    StrictLessDepthWrite, STRICT_LESS_DEPTH_ENTRY_POINT, STRICT_LESS_DEPTH_WGSL,
};
pub use device::{
    HeadlessBackend, HeadlessDeviceOutcome, InFlightFill, NoAdapter, PrewarmedRenderer,
    UninitializedRenderer,
};
pub use endian_swap::{
    endian_swap_uint, endian_swap_uint16, endian_swap_uint32, ENDIAN_SWAP_ENTRY_POINT,
    ENDIAN_SWAP_WGSL,
};
pub use formats_dither::{
    alpha_dither_value, float4_to_rgba32, float_to_uint8, Rgba32Packed, FORMATS_DITHER_ENTRY_POINT,
    FORMATS_DITHER_WGSL,
};
pub use lifecycle::{
    NativeCompletionIdentity, StagedWgpuEffect, WgpuBackendCompletion, WgpuRenderError,
    FILL_FIXTURE_BYTES, FILL_FIXTURE_HEIGHT, FILL_FIXTURE_TEST_COLOR, FILL_FIXTURE_TEST_OUTPUT,
    FILL_FIXTURE_WIDTH,
};
pub use math_hlsli::{get_perpendicular_vector, modulo};
pub use native_contract::{
    prepare_native_fill, CommittedNativeFrame, DeviceRgba16Bytes, InFlightNativeFill,
    N64RecompRdramStorageBytes, NativeContractError, NativeDurableState, NativeFrameBinding,
    NativeGuestCommitError, NativeTargetIdentity, PendingNativeCommit, PreparedNativeFill,
    NATIVE_FILL_COMMAND_END, NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_WORDS,
    NATIVE_FILL_DEVICE_RGBA16, NATIVE_FILL_FIXTURE_SCHEMA, NATIVE_FILL_HEIGHT,
    NATIVE_FILL_JOURNAL_SHA256, NATIVE_FILL_N64RECOMP_STORAGE_RGBA16,
    NATIVE_FILL_N64RECOMP_STORAGE_RGBA16_SHA256, NATIVE_FILL_NATIVE_RGBA8,
    NATIVE_FILL_NATIVE_RGBA8_SHA256, NATIVE_FILL_POST_VI_BGRA8, NATIVE_FILL_POST_VI_BGRA8_SHA256,
    NATIVE_FILL_RDRAM_BYTES, NATIVE_FILL_STREAM_SHA256, NATIVE_FILL_TARGET_END,
    NATIVE_FILL_TARGET_START, NATIVE_FILL_TRANSACTION_SEQUENCE, NATIVE_FILL_WIDTH,
    NATIVE_FILL_WORKLOAD_SHA256,
};
pub use production::{WgpuBackend, WgpuBackendConstructionError, WgpuRawDpcExecutionError};
pub use random::{RandomState, RANDOM_ENTRY_POINT, RANDOM_WGSL};
pub use raster_vs::{
    raster_vs, RasterVsParams, RasterVsPosition, Resolution, ScreenTransform,
    RASTER_VS_ENTRY_POINT, RASTER_VS_WGSL,
};
pub use raw_dpc::{
    decode_raw_dpc, decode_raw_dpc_after, decode_triangle_vertices,
    neutral_vertex_to_raster_vertex, push_decoded_raw_dpc, retrieve_triangle_draws,
    texture_rectangle_vertices, triangle_word_count, BoundTmemTransfer, CoefficientWords,
    DecodedRawDpc, DecodedRawDpcCommand, DegenerateTextureRectangle, DepthWords, FillRectangle,
    MissingTriangleDrawState, PushDecodedRawDpcError, RawDpcCommandKind, RawDpcCommandLocation,
    RawDpcDecodeError, RawDpcResourcePlan, RawTextureRectangle, RawTextureRectangleError,
    RawTriangle, RawWord, RetrievedTriangleDraw, TextureRectangleBeforeAnyOtherMode,
    TextureRectangleVertex, TextureRectangleVertices, TmemLoadSourcePlanError,
    TriangleBeforeAnyOtherMode, TriangleDecodeError, TriangleDrawStateCollector, TriangleFlags,
    TriangleVertex, TriangleVertices, UnadmittedRawDpcCommand, TEXTURE_RECTANGLE_COMMAND_BYTES,
};
pub use rgb_dither::{
    dither_pattern_index, dither_pattern_value, quantize_post_float_rgba16_non_hdr,
    CoverageModulo8, CoverageModulo8Error, DitherNoiseByte, DitherThreshold, DitherThresholdError,
    Rgba16Packed, Rgba16QuantizeInput, RGB_DITHER_ENTRY_POINT, RGB_DITHER_WGSL,
};
pub use shader_manifest::triangle_pipeline_fragment_wgsl;
pub use shader_manifest::{
    DirectTexelDecodeDeviceProfile, DirectTexelDecodeNativeError, DirectTexelDecodeNativeReceipt,
    DirectTexelDecodeProfileError, DirectTexelShaderInput, DirectTexelShaderStatus,
    RuntimeShaderComponentId, RuntimeShaderComponentManifest, RuntimeShaderNativeState,
    RuntimeShaderPromotion, RuntimeShaderStage, ValidatedDirectTexelDecodeProfile,
    DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS, DIRECT_TEXEL_DECODE_CASES,
    DIRECT_TEXEL_DECODE_DENOMINATOR_PATH, DIRECT_TEXEL_DECODE_DENOMINATOR_SHA256,
    DIRECT_TEXEL_DECODE_DEPENDENCY_SOURCES, DIRECT_TEXEL_DECODE_ENTRY_POINT,
    DIRECT_TEXEL_DECODE_EXPECTED_SHA256, DIRECT_TEXEL_DECODE_FIXTURE_SCHEMA,
    DIRECT_TEXEL_DECODE_FIXTURE_SHA256, DIRECT_TEXEL_DECODE_FORMATS_SHA256,
    DIRECT_TEXEL_DECODE_INPUT_BYTES, DIRECT_TEXEL_DECODE_INPUT_SHA256,
    DIRECT_TEXEL_DECODE_MANIFEST, DIRECT_TEXEL_DECODE_OUTPUT_BYTES,
    DIRECT_TEXEL_DECODE_RT64_SOURCE_COMMIT, DIRECT_TEXEL_DECODE_SOURCE_SHA256,
    DIRECT_TEXEL_DECODE_TEXTURE_DECODER_SHA256, DIRECT_TEXEL_DECODE_WGSL,
    DIRECT_TEXEL_DECODE_WORKGROUPS, THREE_NEAREST_FILTER_CANDIDATE_CONSUMERS,
    THREE_NEAREST_FILTER_CASES, THREE_NEAREST_FILTER_DENOMINATOR_PATH,
    THREE_NEAREST_FILTER_DENOMINATOR_SHA256, THREE_NEAREST_FILTER_DEPENDENCY_SOURCES,
    THREE_NEAREST_FILTER_ENTRY_POINT, THREE_NEAREST_FILTER_EXPECTED_SHA256,
    THREE_NEAREST_FILTER_FIXTURE_SCHEMA, THREE_NEAREST_FILTER_FIXTURE_SHA256,
    THREE_NEAREST_FILTER_FORMATS_SHA256, THREE_NEAREST_FILTER_INPUT_BYTES,
    THREE_NEAREST_FILTER_INPUT_SHA256, THREE_NEAREST_FILTER_MANIFEST,
    THREE_NEAREST_FILTER_OUTPUT_BYTES, THREE_NEAREST_FILTER_RT64_SOURCE_COMMIT,
    THREE_NEAREST_FILTER_SOURCE_SHA256, THREE_NEAREST_FILTER_TEXTURE_DECODER_SHA256,
    THREE_NEAREST_FILTER_WGSL, THREE_NEAREST_FILTER_WORKGROUPS,
    TRIANGLE_PIPELINE_FRAGMENT_CANDIDATE_CONSUMERS, TRIANGLE_PIPELINE_FRAGMENT_CASES,
    TRIANGLE_PIPELINE_FRAGMENT_DEPENDENCY_SOURCES, TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT,
    TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SCHEMA, TRIANGLE_PIPELINE_FRAGMENT_MANIFEST,
    TRIANGLE_PIPELINE_FRAGMENT_MANIFEST_ENTRY_POINT, TRIANGLE_PIPELINE_FRAGMENT_RT64_SOURCE_COMMIT,
    TRIANGLE_PIPELINE_FRAGMENT_WRAPPER_WGSL, TRIANGLE_PIPELINE_VERTEX_CANDIDATE_CONSUMERS,
    TRIANGLE_PIPELINE_VERTEX_CASES, TRIANGLE_PIPELINE_VERTEX_DEPENDENCY_SOURCES,
    TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT, TRIANGLE_PIPELINE_VERTEX_FIXTURE_SCHEMA,
    TRIANGLE_PIPELINE_VERTEX_FIXTURE_SHA256, TRIANGLE_PIPELINE_VERTEX_MANIFEST,
    TRIANGLE_PIPELINE_VERTEX_RT64_SOURCE_COMMIT, TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256,
    TRIANGLE_PIPELINE_VERTEX_WGSL,
};
pub use state::{
    AlphaCompare, AlphaDither, BlenderCycle, Color4, ColorImage, CoverageDestination, CycleType,
    DepthMode, FillColor, ImageFormat, OtherMode, PixelSize, PrimColor, PrimDepth, PrimLod,
    RdpState, RdpStateDelta, RgbDither, StagedRdpState, TextureFilter, TextureLutMode,
    TextureLutModeError,
};
pub use targets::{
    decode_fill_cycle_pixel, execute_fill_rectangle, fixed_fixture_other_mode, pack_device_pixels,
    resolve_fill_pixel_rectangle, unpack_device_pixels, CandidateColorTarget, ColorTargetExtent,
    ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, CommittedNativeRasterFrame,
    CompletedColorTargetWrite, DeviceColorBytes, ExactRowPlan, FillCoordinateError,
    FillCycleBypassHazards, FillExecutionError, FillPixelRectangle, InFlightNativeRasterFill,
    InFlightTriangleDraw, InitializedCandidateColorTarget, InitializedRegionProof,
    NativeRasterDeviceOutcome, NativeRasterError, NativeRasterRenderer, PendingNativeRasterCommit,
    RasterVertex, ResidentColorTarget, Rgba8, TargetError, TargetGeneration, TargetRectangle,
    TargetRowRange, TargetRows, TriangleDrawOutput, TriangleFixture, TrianglePipelineDeviceOutcome,
    TrianglePipelineError, TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
    UninitializedNativeRaster, UninitializedTrianglePipeline, TMEM_SAMPLE_STATUS_INVALID_BYTE,
    TMEM_SAMPLE_STATUS_NO_TILE_BINDING, TMEM_SAMPLE_STATUS_OK, TMEM_SAMPLE_STATUS_REVERSED_EXTENT,
    TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT,
};
pub use texture_gen::{
    compute_texture_gen, normalize_safe, RspLookAt, WorldMatrix, TEXTURE_GEN_ENTRY_POINT,
    TEXTURE_GEN_WGSL,
};
pub use tmem::{
    address_point_texel, address_texture_cell, decode_direct_texel, decode_tlut_entry,
    execute_ordered_tmem_loads, filter_three_nearest_committed_cell, gather_committed_texture_cell,
    prepare_load_block, prepare_load_tile, prepare_load_tlut, project_committed_tmem,
    read_committed_texel, resolve_indexed_texel, sample_committed_point, unpack_ci4_texel,
    AddressedTextureCell, AddressedTmemTexel, Ci4Palette, Ci4PaletteError, Ci4UnpackError,
    CommittedTextureCell, CommittedTmemTransaction, DecodedPhysicalTexel, DecodedTexel,
    DefinedPhysicalTmemWordBytes, DirectTexelDecodeError, ExecutedLoadBlock, ExecutedLoadTile,
    ExecutedLoadTlut, GpuBoundTmemTransaction, IndexedTexelResolveError, LoadBlockExecutionError,
    LoadTileExecutionError, LoadTlutExecutionError, PendingTmemTransaction, PhysicalTexelReadError,
    PhysicalTmemBinding, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemPublicationAuthority, PhysicalTmemSnapshotIdentity, PhysicalTmemState,
    PhysicalTmemStateIdentity, PhysicalTmemTransactionIdentity, PointAddressError,
    PointSampleCoordinates, PointSampleError, PointSampleRequest, PreparedLoadBlock,
    PreparedLoadTile, PreparedLoadTlut, RawTexel, RawTexelError, ResolvedIndexedTexel,
    StagedTmemTransaction, TexelColumnParity, TextureAxis, TextureCellCorner, TextureCellFractions,
    TextureCellSampleError, TextureCoordinateS10_5, TextureImage, TileAddressMode,
    TileBindingParams, TileCoordinate, TileDescriptor, TileIndex, TileSize, TileState,
    TlutEntryCount, TlutEntryDecodeError, TlutLookup, TmemDxt, TmemFirstRowParity,
    TmemGpuProjection, TmemLoad, TmemLoadContract, TmemLoadDestinationPlan, TmemLoadEpoch,
    TmemLoadKind, TmemLoadSourceIdentity, TmemLoadSourcePlan, TmemPacketExecutionError, TmemState,
    TmemTransferLayout, TmemTransferPhysicalWord, TmemTransferPlan, TmemTransferWord,
    TmemWordAddress, TILE_BINDING_PARAMS_BYTES, TILE_BINDING_PARAMS_FIELDS, TMEM_BYTE_WORDS,
    TMEM_VALIDITY_WORDS,
};
