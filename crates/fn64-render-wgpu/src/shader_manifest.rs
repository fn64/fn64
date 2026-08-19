//! Closed admission for repository-owned runtime shader components.
//!
//! M2.5.3a retains candidate mechanics for the seven direct texel conversion
//! formulas already owned by [`crate::decode_direct_texel`]. The component is
//! not qualified or natively verified, and it promotes no denominator row.

use core::fmt;

#[cfg(test)]
use crate::{DirectTexelDecodeError, PixelSize};
use crate::{ImageFormat, RawTexel};

pub const DIRECT_TEXEL_DECODE_WGSL: &str = include_str!("shaders/direct_texel_decode.wgsl");
pub const DIRECT_TEXEL_DECODE_ENTRY_POINT: &str = "decode_direct_texels";
pub const DIRECT_TEXEL_DECODE_FIXTURE_SCHEMA: &str =
    "fn64.render-wgpu.direct-texel-decode-fixture.v1";
pub const DIRECT_TEXEL_DECODE_CASES: u32 = 131_710;
pub const DIRECT_TEXEL_DECODE_INPUT_BYTES: u64 = DIRECT_TEXEL_DECODE_CASES as u64 * 16;
pub const DIRECT_TEXEL_DECODE_OUTPUT_BYTES: u64 = DIRECT_TEXEL_DECODE_CASES as u64 * 8;
pub const DIRECT_TEXEL_DECODE_WORKGROUPS: u32 = DIRECT_TEXEL_DECODE_CASES.div_ceil(64);
pub const DIRECT_TEXEL_DECODE_RT64_SOURCE_COMMIT: &str = "5473732a822a4423b5696e7cb18fecc425a59875";
pub const DIRECT_TEXEL_DECODE_FORMATS_SHA256: &str =
    "9b5765371d19de1e410dbe919433922db975994e2a6077bf9e499a8a94f33b7b";
pub const DIRECT_TEXEL_DECODE_TEXTURE_DECODER_SHA256: &str =
    "63b2c1ce683e7e7880c9508d3232d90e90236157ac86ae91947c62ae1d359f07";
pub const DIRECT_TEXEL_DECODE_DENOMINATOR_SHA256: &str =
    "cae8956fff3258bf5c21bb5cea7ffb550ab726118840a16db69764d3507d3ebe";
pub const DIRECT_TEXEL_DECODE_DENOMINATOR_PATH: &str = "docs/rt64-shader-source-denominator.json";
pub const DIRECT_TEXEL_DECODE_DEPENDENCY_SOURCES: [&str; 2] = [
    "src/shaders/Formats.hlsli",
    "src/shaders/TextureDecoder.hlsli",
];

// Filled only after the final owned source and deterministic fixture bytes are
// generated. Tests independently recompute both identities.
pub const DIRECT_TEXEL_DECODE_SOURCE_SHA256: [u8; 32] = [
    0x2f, 0x59, 0x38, 0x0f, 0x62, 0xdb, 0x77, 0xf1, 0xc1, 0x1b, 0x81, 0xe1, 0x49, 0x89, 0x49, 0x47,
    0xd0, 0x1a, 0xd8, 0xc8, 0x12, 0xee, 0x11, 0xe3, 0x77, 0x11, 0x25, 0x31, 0x7f, 0xff, 0x38, 0x80,
];
pub const DIRECT_TEXEL_DECODE_FIXTURE_SHA256: [u8; 32] = [
    0xf2, 0x4a, 0xca, 0x79, 0x5a, 0xe8, 0x95, 0x4a, 0xe3, 0x62, 0x28, 0x0c, 0xd9, 0x1c, 0x75, 0x01,
    0x76, 0x23, 0xbf, 0x7d, 0xb1, 0x60, 0x16, 0x88, 0xe9, 0xbc, 0x27, 0x75, 0xdc, 0xfb, 0x7d, 0x37,
];
pub const DIRECT_TEXEL_DECODE_INPUT_SHA256: [u8; 32] = [
    0x61, 0x99, 0xdd, 0x74, 0x58, 0x7f, 0x3a, 0x8c, 0xa8, 0x6e, 0x24, 0xfb, 0xab, 0x59, 0x49, 0xbb,
    0x0d, 0xcb, 0x04, 0xdb, 0x8c, 0x4c, 0x47, 0xf3, 0xe9, 0xa6, 0x55, 0x04, 0xed, 0xd9, 0xc2, 0x74,
];
pub const DIRECT_TEXEL_DECODE_EXPECTED_SHA256: [u8; 32] = [
    0x89, 0xbc, 0xe8, 0x8b, 0x39, 0x7f, 0xa2, 0xa5, 0xe0, 0x8e, 0xb1, 0x54, 0x94, 0x98, 0xeb, 0xa4,
    0x94, 0x65, 0x36, 0x89, 0x68, 0xff, 0x84, 0x4e, 0xd0, 0xe4, 0x2e, 0x40, 0x7c, 0x17, 0x11, 0x4f,
];

pub const THREE_NEAREST_FILTER_WGSL: &str = include_str!("shaders/three_nearest_filter.wgsl");
pub const THREE_NEAREST_FILTER_ENTRY_POINT: &str = "filter_three_nearest";
pub const THREE_NEAREST_FILTER_FIXTURE_SCHEMA: &str =
    "fn64.render-wgpu.three-nearest-filter-fixture.v1";
pub const THREE_NEAREST_FILTER_CASES: u32 = 262_144;
pub const THREE_NEAREST_FILTER_INPUT_BYTES: u64 = THREE_NEAREST_FILTER_CASES as u64 * 32;
pub const THREE_NEAREST_FILTER_OUTPUT_BYTES: u64 = THREE_NEAREST_FILTER_CASES as u64 * 8;
pub const THREE_NEAREST_FILTER_WORKGROUPS: u32 = THREE_NEAREST_FILTER_CASES.div_ceil(64);
// No RT64 upstream source exists for this formula: the reference lane cites
// only the public Nintendo Programming Manual, "TF: Texture Filter" and
// "Sampling Overview" (fn64-render-reference/src/gbi/types.rs:1109-1112).
// This component's provenance field carries that citation string rather than
// an RT64 commit hash; the manifest struct's field names stay RT64-shaped
// (see M2.5.3a) pending the provenance-kind design question this card left
// open (§3/§9). No RT64 dependency source or candidate consumer exists for
// this component, so those lists are empty, not fabricated.
pub const THREE_NEAREST_FILTER_RT64_SOURCE_COMMIT: &str =
    "Nintendo 64 Programming Manual: \"TF: Texture Filter\", \"Sampling Overview\"";
pub const THREE_NEAREST_FILTER_FORMATS_SHA256: &str = "";
pub const THREE_NEAREST_FILTER_TEXTURE_DECODER_SHA256: &str = "";
pub const THREE_NEAREST_FILTER_DENOMINATOR_SHA256: &str = "";
pub const THREE_NEAREST_FILTER_DENOMINATOR_PATH: &str = "";
pub const THREE_NEAREST_FILTER_DEPENDENCY_SOURCES: [&str; 0] = [];

// Filled only after the final owned source and deterministic fixture bytes
// are generated. Tests independently recompute both.
pub const THREE_NEAREST_FILTER_SOURCE_SHA256: [u8; 32] = [
    0xf6, 0xec, 0x47, 0xb5, 0xb3, 0xa5, 0x3e, 0xbc, 0xbb, 0x65, 0x3c, 0x28, 0x66, 0x8d, 0xe6, 0xa3,
    0x55, 0xee, 0xb6, 0x8f, 0x65, 0xe7, 0xfa, 0x60, 0x82, 0x0f, 0xad, 0xa1, 0x0f, 0x10, 0xe6, 0x5e,
];
pub const THREE_NEAREST_FILTER_FIXTURE_SHA256: [u8; 32] = [
    0x71, 0xc2, 0xc9, 0xa1, 0x8a, 0xfb, 0xe5, 0x93, 0x55, 0xeb, 0xc9, 0x8a, 0x63, 0x28, 0xfc, 0x1e,
    0xb4, 0x88, 0x10, 0xd5, 0xe1, 0x06, 0x3c, 0x88, 0xca, 0x8b, 0xeb, 0x48, 0x17, 0x4d, 0x93, 0x38,
];
pub const THREE_NEAREST_FILTER_INPUT_SHA256: [u8; 32] = [
    0x78, 0xc8, 0x4a, 0xa9, 0x77, 0xdb, 0x90, 0x23, 0x19, 0x96, 0xa6, 0xf6, 0x27, 0xf7, 0x73, 0x57,
    0xf2, 0x4b, 0xa5, 0x8a, 0xce, 0x6e, 0x7b, 0x30, 0x88, 0xb4, 0xaf, 0xcc, 0x8f, 0x3b, 0x5c, 0xc5,
];
pub const THREE_NEAREST_FILTER_EXPECTED_SHA256: [u8; 32] = [
    0x26, 0x9f, 0x60, 0x4e, 0x46, 0x52, 0xe2, 0xd2, 0x07, 0x84, 0x85, 0x27, 0x61, 0xc7, 0x24, 0x1b,
    0xb6, 0x5f, 0x1a, 0xfc, 0x38, 0x13, 0x15, 0x6f, 0xb0, 0xaf, 0x81, 0xa8, 0x64, 0x35, 0x77, 0x00,
];

pub const THREE_NEAREST_FILTER_CANDIDATE_CONSUMERS: [&str; 0] = [];

pub const DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS: [&str; 21] = [
    "src-shaders-rasterpsdynamic",
    "src-shaders-rasterpsdynamicms",
    "src-shaders-rasterpsspecconstant",
    "src-shaders-rasterpsspecconstantms",
    "src-shaders-rasterpsspecconstantflat",
    "src-shaders-rasterpsspecconstantflatms",
    "src-shaders-fbreadanychangescs",
    "src-shaders-fbreinterpretcs",
    "src-shaders-fbreadanyfullcs",
    "src-shaders-fbwritecolorcs",
    "src-shaders-fbwritedepthcs",
    "src-shaders-fbwritedepthcsms",
    "src-shaders-rtcopycolortodepthps",
    "src-shaders-rtcopycolortodepth2xps",
    "src-shaders-rtcopycolortodepth4xps",
    "src-shaders-rtcopycolortodepth8xps",
    "src-shaders-rtcopydepthtocolorps",
    "src-shaders-rtcopydepthtocolor2xps",
    "src-shaders-rtcopydepthtocolor4xps",
    "src-shaders-rtcopydepthtocolor8xps",
    "src-shaders-texturedecodecs",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeShaderComponentId {
    DirectTexelDecodeV1,
    ThreeNearestFilterV1,
    TrianglePipelineFragmentV1,
    TrianglePipelineVertexV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeShaderStage {
    Compute,
    Vertex,
    Fragment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeShaderPromotion {
    NotQualified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeShaderNativeState {
    NativeUnverified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeShaderComponentManifest {
    component: RuntimeShaderComponentId,
    stage: RuntimeShaderStage,
    entry_point: &'static str,
    source_bytes: u32,
    source_sha256: [u8; 32],
    fixture_schema: &'static str,
    fixture_sha256: [u8; 32],
    promotion: RuntimeShaderPromotion,
    native_state: RuntimeShaderNativeState,
    rt64_source_commit: &'static str,
    formats_sha256: &'static str,
    texture_decoder_sha256: &'static str,
    denominator_path: &'static str,
    denominator_sha256: &'static str,
    dependency_sources: &'static [&'static str],
    candidate_consumers: &'static [&'static str],
}

impl RuntimeShaderComponentManifest {
    pub const fn component(self) -> RuntimeShaderComponentId {
        self.component
    }

    pub const fn stage(self) -> RuntimeShaderStage {
        self.stage
    }

    pub const fn entry_point(self) -> &'static str {
        self.entry_point
    }

    pub const fn source_bytes(self) -> u32 {
        self.source_bytes
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn fixture_schema(self) -> &'static str {
        self.fixture_schema
    }

    pub const fn fixture_sha256(self) -> [u8; 32] {
        self.fixture_sha256
    }

    pub const fn promotion(self) -> RuntimeShaderPromotion {
        self.promotion
    }

    pub const fn native_state(self) -> RuntimeShaderNativeState {
        self.native_state
    }

    pub const fn rt64_source_commit(self) -> &'static str {
        self.rt64_source_commit
    }

    pub const fn formats_sha256(self) -> &'static str {
        self.formats_sha256
    }

    pub const fn texture_decoder_sha256(self) -> &'static str {
        self.texture_decoder_sha256
    }

    pub const fn denominator_path(self) -> &'static str {
        self.denominator_path
    }

    pub const fn denominator_sha256(self) -> &'static str {
        self.denominator_sha256
    }

    pub const fn dependency_sources(self) -> &'static [&'static str] {
        self.dependency_sources
    }

    pub const fn candidate_consumers(self) -> &'static [&'static str] {
        self.candidate_consumers
    }
}

pub const DIRECT_TEXEL_DECODE_MANIFEST: RuntimeShaderComponentManifest =
    RuntimeShaderComponentManifest {
        component: RuntimeShaderComponentId::DirectTexelDecodeV1,
        stage: RuntimeShaderStage::Compute,
        entry_point: DIRECT_TEXEL_DECODE_ENTRY_POINT,
        source_bytes: DIRECT_TEXEL_DECODE_WGSL.len() as u32,
        source_sha256: DIRECT_TEXEL_DECODE_SOURCE_SHA256,
        fixture_schema: DIRECT_TEXEL_DECODE_FIXTURE_SCHEMA,
        fixture_sha256: DIRECT_TEXEL_DECODE_FIXTURE_SHA256,
        promotion: RuntimeShaderPromotion::NotQualified,
        native_state: RuntimeShaderNativeState::NativeUnverified,
        rt64_source_commit: DIRECT_TEXEL_DECODE_RT64_SOURCE_COMMIT,
        formats_sha256: DIRECT_TEXEL_DECODE_FORMATS_SHA256,
        texture_decoder_sha256: DIRECT_TEXEL_DECODE_TEXTURE_DECODER_SHA256,
        denominator_path: DIRECT_TEXEL_DECODE_DENOMINATOR_PATH,
        denominator_sha256: DIRECT_TEXEL_DECODE_DENOMINATOR_SHA256,
        dependency_sources: &DIRECT_TEXEL_DECODE_DEPENDENCY_SOURCES,
        candidate_consumers: &DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS,
    };

pub const THREE_NEAREST_FILTER_MANIFEST: RuntimeShaderComponentManifest =
    RuntimeShaderComponentManifest {
        component: RuntimeShaderComponentId::ThreeNearestFilterV1,
        stage: RuntimeShaderStage::Compute,
        entry_point: THREE_NEAREST_FILTER_ENTRY_POINT,
        source_bytes: THREE_NEAREST_FILTER_WGSL.len() as u32,
        source_sha256: THREE_NEAREST_FILTER_SOURCE_SHA256,
        fixture_schema: THREE_NEAREST_FILTER_FIXTURE_SCHEMA,
        fixture_sha256: THREE_NEAREST_FILTER_FIXTURE_SHA256,
        promotion: RuntimeShaderPromotion::NotQualified,
        native_state: RuntimeShaderNativeState::NativeUnverified,
        rt64_source_commit: THREE_NEAREST_FILTER_RT64_SOURCE_COMMIT,
        formats_sha256: THREE_NEAREST_FILTER_FORMATS_SHA256,
        texture_decoder_sha256: THREE_NEAREST_FILTER_TEXTURE_DECODER_SHA256,
        denominator_path: THREE_NEAREST_FILTER_DENOMINATOR_PATH,
        denominator_sha256: THREE_NEAREST_FILTER_DENOMINATOR_SHA256,
        dependency_sources: &THREE_NEAREST_FILTER_DEPENDENCY_SOURCES,
        candidate_consumers: &THREE_NEAREST_FILTER_CANDIDATE_CONSUMERS,
    };

// Triangle-pipeline fragment shader (port card §2d/§3 step 3): a real
// @fragment entry point wrapping crate::combiner's existing WGSL
// transcription (`shaders/color_combiner.wgsl`'s `run_one_cycle`), admitted
// per this file's established shape -- same full field set as
// `DIRECT_TEXEL_DECODE_MANIFEST`/`THREE_NEAREST_FILTER_MANIFEST`, including
// the SHA-256 fixture-identity fields. Unlike those two compute kernels this
// component has no *bulk* input/output buffer (it is a per-fragment render-
// pipeline shader, exercised via `targets/triangle_pipeline.rs`'s vertex/
// pixel fixtures and differential spot-checks per port card §6, not a
// dispatched compute kernel over thousands of cases) -- so its "fixture" is
// the combined WGSL source text itself (`TRIANGLE_PIPELINE_FRAGMENT_CASES`
// = 1, `*_INPUT_BYTES`/`*_OUTPUT_BYTES` = the combined source's own byte
// length), following `THREE_NEAREST_FILTER_MANIFEST`'s own precedent of a
// non-RT64-CPU-source, non-uniform-fixture-shape component still carrying
// every manifest field with a real, hash-checked value rather than an
// explanatory omission.
pub const TRIANGLE_PIPELINE_VERTEX_WGSL: &str =
    include_str!("shaders/triangle_pipeline_vertex.wgsl");
pub const TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT: &str = "vs_main";
pub const TRIANGLE_PIPELINE_VERTEX_FIXTURE_SCHEMA: &str =
    "fn64.render-wgpu.triangle-pipeline-vertex-fixture.v1";
pub const TRIANGLE_PIPELINE_VERTEX_CASES: u32 = 1;
pub const TRIANGLE_PIPELINE_VERTEX_RT64_SOURCE_COMMIT: &str =
    "5473732a822a4423b5696e7cb18fecc425a59875";
pub const TRIANGLE_PIPELINE_VERTEX_DEPENDENCY_SOURCES: [&str; 1] = ["src/shaders/RasterVS.hlsl"];
pub const TRIANGLE_PIPELINE_VERTEX_CANDIDATE_CONSUMERS: [&str; 1] = ["src-shaders-rastervs"];

// Filled only after the final owned source's deterministic digest is
// generated (this file's established freeze convention, matching
// `DIRECT_TEXEL_DECODE_SOURCE_SHA256`/`THREE_NEAREST_FILTER_SOURCE_SHA256`):
// unlike the fragment shader's concatenated source (which still freezes to
// a real literal, see `TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256`, but needs
// `recompute_triangle_pipeline_fragment_digests()` to verify it since the
// combined text isn't a single `include_str!`), this file is a single
// `include_str!` literal, so its digest is `const`-frozen directly, the
// simpler of the two shapes. Tests independently recompute it.
pub const TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256: [u8; 32] = [
    0x21, 0x31, 0xe1, 0xe3, 0x63, 0x33, 0x9a, 0x3f, 0x43, 0xc4, 0xe3, 0x9f, 0x47, 0x9f, 0xbc, 0x55,
    0xd9, 0x2b, 0xad, 0x3d, 0x88, 0xbc, 0xd6, 0x90, 0x62, 0xdc, 0x8c, 0x19, 0x62, 0x53, 0x33, 0x77,
];
pub const TRIANGLE_PIPELINE_VERTEX_FIXTURE_SHA256: [u8; 32] = [
    0xdb, 0x70, 0xad, 0xe0, 0x10, 0xb3, 0x4b, 0x03, 0x53, 0xf1, 0x8f, 0xef, 0x0e, 0x88, 0x35, 0x7f,
    0xb5, 0xca, 0xe4, 0x1a, 0x70, 0x51, 0xf7, 0x8a, 0x2d, 0x5a, 0xd8, 0xce, 0x6d, 0xbf, 0xf3, 0x35,
];

pub const TRIANGLE_PIPELINE_VERTEX_MANIFEST: RuntimeShaderComponentManifest =
    RuntimeShaderComponentManifest {
        component: RuntimeShaderComponentId::TrianglePipelineVertexV1,
        stage: RuntimeShaderStage::Vertex,
        entry_point: TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT,
        source_bytes: TRIANGLE_PIPELINE_VERTEX_WGSL.len() as u32,
        source_sha256: TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256,
        fixture_schema: TRIANGLE_PIPELINE_VERTEX_FIXTURE_SCHEMA,
        fixture_sha256: TRIANGLE_PIPELINE_VERTEX_FIXTURE_SHA256,
        promotion: RuntimeShaderPromotion::NotQualified,
        native_state: RuntimeShaderNativeState::NativeUnverified,
        rt64_source_commit: TRIANGLE_PIPELINE_VERTEX_RT64_SOURCE_COMMIT,
        formats_sha256: "",
        texture_decoder_sha256: "",
        denominator_path: "",
        denominator_sha256: "",
        dependency_sources: &TRIANGLE_PIPELINE_VERTEX_DEPENDENCY_SOURCES,
        candidate_consumers: &TRIANGLE_PIPELINE_VERTEX_CANDIDATE_CONSUMERS,
    };

pub const TRIANGLE_PIPELINE_FRAGMENT_WRAPPER_WGSL: &str =
    include_str!("shaders/triangle_pipeline_fragment.wgsl");
pub const TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT: &str = "fs_main";
pub const TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SCHEMA: &str =
    "fn64.render-wgpu.triangle-pipeline-fragment-fixture.v1";
pub const TRIANGLE_PIPELINE_FRAGMENT_CASES: u32 = 1;
pub const TRIANGLE_PIPELINE_FRAGMENT_RT64_SOURCE_COMMIT: &str =
    "5473732a822a4423b5696e7cb18fecc425a59875";
pub const TRIANGLE_PIPELINE_FRAGMENT_DEPENDENCY_SOURCES: [&str; 2] =
    ["src/shaders/RasterVS.hlsl", "src/shaders/RasterPS.hlsl"];
// This slice's own restricted, textureless/opaque/no-blend RasterPS variant
// (port card §3 restriction set) most closely corresponds to the base
// non-MSAA dynamic-render-params RasterPS variant already named as a
// candidate consumer of direct-texel-decode (§2d).
pub const TRIANGLE_PIPELINE_FRAGMENT_CANDIDATE_CONSUMERS: [&str; 1] =
    ["src-shaders-rasterpsdynamic"];

/// Published committed-TMEM textured-draw card, Option B: the fragment-
/// callable WGSL port of `tmem/read.rs`+`tmem/sample.rs`'s committed
/// physical-TMEM addressing/filter chain. Library-only (no `@group`/
/// `@binding` entry point of its own besides its module-scope storage/
/// uniform bindings), concatenated into the combined fragment source ahead
/// of this file's own `@fragment` wrapper, matching `color_combiner.wgsl`'s
/// established pattern.
pub const TMEM_SAMPLE_WGSL: &str = include_str!("shaders/tmem_sample.wgsl");

/// The fragment shader module actually submitted to `wgpu`: `color_combiner.wgsl`'s
/// existing library functions (reused byte-for-byte, unmodified) concatenated
/// with `tmem_sample.wgsl`'s TMEM addressing/filter port,
/// `alpha_compare_fragment_fn.wgsl`'s real per-fragment alpha-compare gate
/// (alpha-compare production card §3a -- reused verbatim, not re-typed
/// inline, matching this seam's own no-duplication convention),
/// `coverage_fragment_fn.wgsl`'s existing, already-GPU-differentially-
/// validated `Full`/`Save` `cvg_dst` callable (production coverage node 1 --
/// reused verbatim, same no-duplication convention),
/// `blend_fragment_fn.wgsl`'s admitted-subset blend-cycle callable
/// (production blend wiring slice 1 -- same no-duplication convention), and
/// this component's thin `@fragment` wrapper. WGSL has no cross-module
/// include mechanism reachable from separate `wgpu::ShaderSource::Wgsl`
/// strings in this wgpu version, so the six source files are combined into
/// one module at this seam, not at shader-module-creation time in
/// `targets/triangle_pipeline.rs` -- keeping that file free of string
/// concatenation logic and this manifest the single place the combined
/// source text is assembled.
pub fn triangle_pipeline_fragment_wgsl() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        crate::combiner::COLOR_COMBINER_WGSL,
        TMEM_SAMPLE_WGSL,
        crate::alpha_compare::ALPHA_COMPARE_FRAGMENT_FN_WGSL,
        crate::coverage::COVERAGE_FRAGMENT_FN_WGSL,
        crate::blend::BLEND_FRAGMENT_FN_WGSL,
        TRIANGLE_PIPELINE_FRAGMENT_WRAPPER_WGSL
    )
}

pub const TRIANGLE_PIPELINE_FRAGMENT_MANIFEST_ENTRY_POINT: &str =
    TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT;

// Filled only after the final combined source's deterministic digest is
// generated (this file's established freeze convention). The combined
// source is the concatenation of two separately-owned `include_str!`
// constants (`COLOR_COMBINER_WGSL` and `TRIANGLE_PIPELINE_FRAGMENT_WRAPPER_WGSL`)
// via `format!`, not a single literal -- `String` hashing is not
// `const`-evaluable in this crate's Rust edition, so unlike
// `TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256` these bytes were frozen by
// running `triangle_pipeline_fragment_wgsl()` once (the ignored freeze test
// below) and pasting its printed digest here, exactly like every other
// frozen SHA-256 constant in this file. Tests independently recompute both
// and assert they still match this frozen literal -- the public manifest
// constant itself carries the real value, not a placeholder.
pub const TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256: [u8; 32] = [
    0x95, 0xb0, 0xc5, 0x35, 0xb9, 0x42, 0xe5, 0x2b, 0x1a, 0xfc, 0x6b, 0x98, 0xfa, 0xba, 0x62, 0x12,
    0x6a, 0xd4, 0x20, 0xaa, 0xdb, 0x7f, 0x46, 0xf3, 0x70, 0x22, 0xa8, 0xfb, 0xce, 0xce, 0x42, 0x54,
];
pub const TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SHA256: [u8; 32] = [
    0x7d, 0xbe, 0x39, 0x1f, 0x29, 0xf7, 0x0c, 0x78, 0xcd, 0xd9, 0x5b, 0x79, 0xb7, 0xeb, 0xbe, 0xd2,
    0x60, 0x4b, 0x16, 0xc7, 0xd9, 0xce, 0xb4, 0x0d, 0xae, 0x38, 0x71, 0x84, 0x62, 0x07, 0xdc, 0xec,
];
pub const TRIANGLE_PIPELINE_FRAGMENT_SOURCE_BYTES: u32 = 101_893;

pub const TRIANGLE_PIPELINE_FRAGMENT_MANIFEST: RuntimeShaderComponentManifest =
    RuntimeShaderComponentManifest {
        component: RuntimeShaderComponentId::TrianglePipelineFragmentV1,
        stage: RuntimeShaderStage::Fragment,
        entry_point: TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT,
        source_bytes: TRIANGLE_PIPELINE_FRAGMENT_SOURCE_BYTES,
        source_sha256: TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256,
        fixture_schema: TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SCHEMA,
        fixture_sha256: TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SHA256,
        promotion: RuntimeShaderPromotion::NotQualified,
        native_state: RuntimeShaderNativeState::NativeUnverified,
        rt64_source_commit: TRIANGLE_PIPELINE_FRAGMENT_RT64_SOURCE_COMMIT,
        formats_sha256: "",
        texture_decoder_sha256: "",
        denominator_path: "",
        denominator_sha256: "",
        dependency_sources: &TRIANGLE_PIPELINE_FRAGMENT_DEPENDENCY_SOURCES,
        candidate_consumers: &TRIANGLE_PIPELINE_FRAGMENT_CANDIDATE_CONSUMERS,
    };

/// Recomputes [`TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256`]/
/// [`TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SHA256`] from the live combined
/// source text, for this module's own tests to assert against the frozen
/// literals (proving the freeze is correct, not substituting for it --
/// the public [`TRIANGLE_PIPELINE_FRAGMENT_MANIFEST`] carries the frozen
/// values directly, this function is not consulted by any non-test code
/// path). `sha2` is this crate's existing dev-only dependency, so this
/// helper stays `#[cfg(test)]` rather than promoting it to a real
/// dependency for a manifest-identity convenience.
#[cfg(test)]
pub fn recompute_triangle_pipeline_fragment_digests() -> (u32, [u8; 32], [u8; 32]) {
    use sha2::{Digest, Sha256};
    let source = triangle_pipeline_fragment_wgsl();
    let source_sha256: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    let mut fixture_hasher = Sha256::new();
    fixture_hasher.update(TRIANGLE_PIPELINE_FRAGMENT_FIXTURE_SCHEMA.as_bytes());
    fixture_hasher.update([0]);
    fixture_hasher.update((source.len() as u64).to_be_bytes());
    fixture_hasher.update(source.as_bytes());
    let fixture_sha256: [u8; 32] = fixture_hasher.finalize().into();
    (source.len() as u32, source_sha256, fixture_sha256)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectTexelDecodeDeviceProfile;

impl DirectTexelDecodeDeviceProfile {
    pub const fn required_features(self) -> wgpu::Features {
        wgpu::Features::empty()
    }

    pub fn required_limits(self) -> wgpu::Limits {
        wgpu::Limits {
            max_buffer_size: DIRECT_TEXEL_DECODE_INPUT_BYTES,
            max_storage_buffer_binding_size: DIRECT_TEXEL_DECODE_INPUT_BYTES,
            max_storage_buffers_per_shader_stage: 2,
            max_compute_workgroup_size_x: 64,
            max_compute_invocations_per_workgroup: 64,
            max_compute_workgroups_per_dimension: DIRECT_TEXEL_DECODE_WORKGROUPS,
            ..wgpu::Limits::default()
        }
    }

    pub fn validate_adapter(
        self,
        adapter_features: wgpu::Features,
        adapter_limits: &wgpu::Limits,
    ) -> Result<ValidatedDirectTexelDecodeProfile, DirectTexelDecodeProfileError> {
        let required_features = self.required_features();
        if !adapter_features.contains(required_features) {
            return Err(DirectTexelDecodeProfileError::MissingFeatures);
        }
        let required = self.required_limits();
        let mut failure = None;
        required.check_limits_with_fail_fn(adapter_limits, false, |name, requested, allowed| {
            if failure.is_none() {
                failure = Some(DirectTexelDecodeProfileError::LimitTooSmall {
                    name,
                    actual: allowed,
                    minimum: requested,
                });
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(ValidatedDirectTexelDecodeProfile {
            requested_features: required_features,
            requested_limits: required,
        })
    }

    pub fn validate_request_contract(
        self,
        requested_features: wgpu::Features,
        requested_limits: &wgpu::Limits,
    ) -> Result<(), DirectTexelDecodeProfileError> {
        if requested_features != self.required_features() {
            return Err(DirectTexelDecodeProfileError::UnexpectedRequestedFeatures);
        }
        if requested_limits != &self.required_limits() {
            return Err(DirectTexelDecodeProfileError::RequestedLimitsMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedDirectTexelDecodeProfile {
    requested_features: wgpu::Features,
    requested_limits: wgpu::Limits,
}

impl ValidatedDirectTexelDecodeProfile {
    pub const fn requested_features(&self) -> wgpu::Features {
        self.requested_features
    }

    pub const fn requested_limits(&self) -> &wgpu::Limits {
        &self.requested_limits
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectTexelDecodeProfileError {
    MissingFeatures,
    UnexpectedRequestedFeatures,
    RequestedLimitsMismatch,
    LimitTooSmall {
        name: &'static str,
        actual: u64,
        minimum: u64,
    },
}

impl fmt::Display for DirectTexelDecodeProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFeatures => formatter.write_str("direct-texel profile features missing"),
            Self::UnexpectedRequestedFeatures => {
                formatter.write_str("direct-texel request contains unreviewed features")
            }
            Self::RequestedLimitsMismatch => {
                formatter.write_str("direct-texel request limits differ from the closed profile")
            }
            Self::LimitTooSmall {
                name,
                actual,
                minimum,
            } => write!(
                formatter,
                "direct-texel profile limit {name} is {actual}, requires at least {minimum}"
            ),
        }
    }
}

impl std::error::Error for DirectTexelDecodeProfileError {}

#[derive(Debug)]
pub enum DirectTexelDecodeNativeError {
    NativeAdapterUnavailable(String),
    Profile(DirectTexelDecodeProfileError),
    FrozenIdentityMismatch { field: &'static str },
    RequestDevice(String),
    PipelineValidation(String),
    ExactSubmissionWait(String),
    CompletionCallbackNotObserved,
    MapWait(String),
    MapCallbackNotObserved,
    Map(String),
    MappedRange(String),
    SemanticMismatch { first_byte: Option<usize> },
    UncapturedErrors { count: usize },
}

impl fmt::Display for DirectTexelDecodeNativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeAdapterUnavailable(reason) => {
                write!(
                    formatter,
                    "direct-texel native adapter unavailable: {reason}"
                )
            }
            Self::Profile(error) => error.fmt(formatter),
            Self::FrozenIdentityMismatch { field } => {
                write!(formatter, "direct-texel frozen {field} identity mismatch")
            }
            Self::RequestDevice(reason) => {
                write!(formatter, "direct-texel device request failed: {reason}")
            }
            Self::PipelineValidation(reason) => {
                write!(
                    formatter,
                    "direct-texel pipeline validation failed: {reason}"
                )
            }
            Self::ExactSubmissionWait(reason) => {
                write!(
                    formatter,
                    "direct-texel exact submission wait failed: {reason}"
                )
            }
            Self::CompletionCallbackNotObserved => {
                formatter.write_str("direct-texel completion callback was not observed")
            }
            Self::MapWait(reason) => write!(formatter, "direct-texel map wait failed: {reason}"),
            Self::MapCallbackNotObserved => {
                formatter.write_str("direct-texel map callback was not observed")
            }
            Self::Map(reason) => write!(formatter, "direct-texel map failed: {reason}"),
            Self::MappedRange(reason) => {
                write!(formatter, "direct-texel mapped range failed: {reason}")
            }
            Self::SemanticMismatch { first_byte } => {
                write!(
                    formatter,
                    "direct-texel semantic mismatch at byte {first_byte:?}"
                )
            }
            Self::UncapturedErrors { count } => {
                write!(
                    formatter,
                    "direct-texel device recorded {count} uncaptured errors"
                )
            }
        }
    }
}

impl std::error::Error for DirectTexelDecodeNativeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectTexelShaderStatus {
    Direct,
    IndexedSeparate,
    YuvDeferred,
    UnsupportedPair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectTexelShaderInput {
    format: ImageFormat,
    raw: RawTexel,
}

impl DirectTexelShaderInput {
    pub const fn new(format: ImageFormat, raw: RawTexel) -> Self {
        Self { format, raw }
    }

    pub const fn format(self) -> ImageFormat {
        self.format
    }

    pub const fn raw(self) -> RawTexel {
        self.raw
    }

    #[cfg(test)]
    fn encode(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&format_code(self.format).to_le_bytes());
        bytes.extend_from_slice(&size_code(self.raw.size()).to_le_bytes());
        bytes.extend_from_slice(&self.raw.value().to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
    }
}

#[cfg(test)]
const fn format_code(format: ImageFormat) -> u32 {
    match format {
        ImageFormat::Rgba => 0,
        ImageFormat::Yuv => 1,
        ImageFormat::ColorIndex => 2,
        ImageFormat::IntensityAlpha => 3,
        ImageFormat::Intensity => 4,
    }
}

#[cfg(test)]
const fn size_code(size: PixelSize) -> u32 {
    match size {
        PixelSize::Bits4 => 0,
        PixelSize::Bits8 => 1,
        PixelSize::Bits16 => 2,
        PixelSize::Bits32 => 3,
    }
}

#[cfg(test)]
fn oracle_output(input: DirectTexelShaderInput) -> (DirectTexelShaderStatus, u32) {
    match crate::decode_direct_texel(input.format, input.raw) {
        Ok(decoded) => (
            DirectTexelShaderStatus::Direct,
            u32::from_be_bytes(decoded.rgba8888()),
        ),
        Err(DirectTexelDecodeError::IndexedDecodeIsSeparate { .. }) => {
            (DirectTexelShaderStatus::IndexedSeparate, 0)
        }
        Err(DirectTexelDecodeError::YuvConversionDeferred { .. }) => {
            (DirectTexelShaderStatus::YuvDeferred, 0)
        }
        Err(DirectTexelDecodeError::UnsupportedPair { .. }) => {
            (DirectTexelShaderStatus::UnsupportedPair, 0)
        }
    }
}

#[cfg(test)]
const fn status_code(status: DirectTexelShaderStatus) -> u32 {
    match status {
        DirectTexelShaderStatus::Direct => 0,
        DirectTexelShaderStatus::IndexedSeparate => 1,
        DirectTexelShaderStatus::YuvDeferred => 2,
        DirectTexelShaderStatus::UnsupportedPair => 3,
    }
}

#[derive(Clone, Debug)]
pub struct DirectTexelDecodeNativeReceipt {
    component: RuntimeShaderComponentId,
    adapter: wgpu::AdapterInfo,
    entry_point: &'static str,
    requested_features: wgpu::Features,
    requested_limits: wgpu::Limits,
    source_sha256: [u8; 32],
    fixture_sha256: [u8; 32],
    input_sha256: [u8; 32],
    case_count: u32,
    expected_sha256: [u8; 32],
    observed_sha256: [u8; 32],
    output_bytes: u64,
    pipeline_creation_succeeded: bool,
    exact_submission_complete: bool,
    callback_observed: bool,
    validation_error_count: usize,
}

impl DirectTexelDecodeNativeReceipt {
    pub const fn component(&self) -> RuntimeShaderComponentId {
        self.component
    }

    pub const fn adapter(&self) -> &wgpu::AdapterInfo {
        &self.adapter
    }

    pub const fn entry_point(&self) -> &'static str {
        self.entry_point
    }

    pub const fn requested_features(&self) -> wgpu::Features {
        self.requested_features
    }

    pub const fn requested_limits(&self) -> &wgpu::Limits {
        &self.requested_limits
    }

    pub const fn source_sha256(&self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn fixture_sha256(&self) -> [u8; 32] {
        self.fixture_sha256
    }

    pub const fn input_sha256(&self) -> [u8; 32] {
        self.input_sha256
    }

    pub const fn case_count(&self) -> u32 {
        self.case_count
    }

    pub const fn expected_sha256(&self) -> [u8; 32] {
        self.expected_sha256
    }

    pub const fn observed_sha256(&self) -> [u8; 32] {
        self.observed_sha256
    }

    pub const fn output_bytes(&self) -> u64 {
        self.output_bytes
    }

    pub const fn pipeline_creation_succeeded(&self) -> bool {
        self.pipeline_creation_succeeded
    }

    pub const fn exact_submission_complete(&self) -> bool {
        self.exact_submission_complete
    }

    pub const fn callback_observed(&self) -> bool {
        self.callback_observed
    }

    pub const fn validation_error_count(&self) -> usize {
        self.validation_error_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use sha2::{Digest, Sha256};

    const ALL_FORMATS: [ImageFormat; 5] = [
        ImageFormat::Rgba,
        ImageFormat::Yuv,
        ImageFormat::ColorIndex,
        ImageFormat::IntensityAlpha,
        ImageFormat::Intensity,
    ];
    const ALL_SIZES: [PixelSize; 4] = [
        PixelSize::Bits4,
        PixelSize::Bits8,
        PixelSize::Bits16,
        PixelSize::Bits32,
    ];

    #[derive(Clone)]
    struct Fixture {
        inputs: Vec<DirectTexelShaderInput>,
        input_bytes: Vec<u8>,
        expected_bytes: Vec<u8>,
        identity: [u8; 32],
    }

    fn fixture() -> Fixture {
        let mut inputs = Vec::with_capacity(DIRECT_TEXEL_DECODE_CASES as usize);
        for format in ALL_FORMATS {
            for size in ALL_SIZES {
                inputs.push(DirectTexelShaderInput::new(
                    format,
                    RawTexel::try_new(size, 0).unwrap(),
                ));
            }
        }
        for value in 0..=0x0f {
            inputs.push(input(ImageFormat::IntensityAlpha, PixelSize::Bits4, value));
        }
        for value in 0..=0x0f {
            inputs.push(input(ImageFormat::Intensity, PixelSize::Bits4, value));
        }
        for value in 0..=0xff {
            inputs.push(input(ImageFormat::IntensityAlpha, PixelSize::Bits8, value));
        }
        for value in 0..=0xff {
            inputs.push(input(ImageFormat::Intensity, PixelSize::Bits8, value));
        }
        for value in 0..=0xffff {
            inputs.push(input(ImageFormat::Rgba, PixelSize::Bits16, value));
        }
        for value in 0..=0xffff {
            inputs.push(input(ImageFormat::IntensityAlpha, PixelSize::Bits16, value));
        }
        let mut rgba32 = vec![0, u32::MAX];
        rgba32.extend((0..32).map(|bit| 1_u32 << bit));
        rgba32.extend((0..32).map(|bit| !(1_u32 << bit)));
        rgba32.extend([
            0x0123_4567,
            0x89ab_cdef,
            0xff00_0000,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff,
            0x1122_3344,
            0x4433_2211,
        ]);
        assert_eq!(rgba32.len(), 74);
        for value in rgba32 {
            inputs.push(input(ImageFormat::Rgba, PixelSize::Bits32, value));
        }
        assert_eq!(inputs.len(), DIRECT_TEXEL_DECODE_CASES as usize);

        let mut input_bytes = Vec::with_capacity(inputs.len() * 16);
        let mut expected_bytes = Vec::with_capacity(inputs.len() * 8);
        for &value in &inputs {
            value.encode(&mut input_bytes);
            let (status, rgba) = oracle_output(value);
            expected_bytes.extend_from_slice(&status_code(status).to_le_bytes());
            expected_bytes.extend_from_slice(&rgba.to_le_bytes());
        }
        assert_eq!(input_bytes.len() as u64, DIRECT_TEXEL_DECODE_INPUT_BYTES);
        assert_eq!(
            expected_bytes.len() as u64,
            DIRECT_TEXEL_DECODE_OUTPUT_BYTES
        );
        let mut hasher = Sha256::new();
        hasher.update(DIRECT_TEXEL_DECODE_FIXTURE_SCHEMA.as_bytes());
        hasher.update([0]);
        hasher.update((input_bytes.len() as u64).to_be_bytes());
        hasher.update(&input_bytes);
        hasher.update((expected_bytes.len() as u64).to_be_bytes());
        hasher.update(&expected_bytes);
        let identity = hasher.finalize().into();
        Fixture {
            inputs,
            input_bytes,
            expected_bytes,
            identity,
        }
    }

    fn input(format: ImageFormat, size: PixelSize, value: u32) -> DirectTexelShaderInput {
        DirectTexelShaderInput::new(format, RawTexel::try_new(size, value).unwrap())
    }

    fn digest(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn verify_pre_submission(
        source: &str,
        entry_point: &str,
        fixture: &Fixture,
    ) -> Result<(), DirectTexelDecodeNativeError> {
        for (field, actual, expected) in [
            (
                "source",
                digest(source.as_bytes()),
                DIRECT_TEXEL_DECODE_SOURCE_SHA256,
            ),
            (
                "fixture",
                fixture.identity,
                DIRECT_TEXEL_DECODE_FIXTURE_SHA256,
            ),
            (
                "input",
                digest(&fixture.input_bytes),
                DIRECT_TEXEL_DECODE_INPUT_SHA256,
            ),
            (
                "expected output",
                digest(&fixture.expected_bytes),
                DIRECT_TEXEL_DECODE_EXPECTED_SHA256,
            ),
        ] {
            if actual != expected {
                return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch { field });
            }
        }
        if entry_point != DIRECT_TEXEL_DECODE_ENTRY_POINT {
            return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch {
                field: "entry point",
            });
        }
        if fixture.inputs.len() != DIRECT_TEXEL_DECODE_CASES as usize
            || fixture.input_bytes.len() as u64 != DIRECT_TEXEL_DECODE_INPUT_BYTES
            || fixture.expected_bytes.len() as u64 != DIRECT_TEXEL_DECODE_OUTPUT_BYTES
        {
            return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch {
                field: "fixture shape",
            });
        }
        Ok(())
    }

    fn verify_observed(
        expected: &[u8],
        observed: &[u8],
    ) -> Result<(), DirectTexelDecodeNativeError> {
        if observed == expected {
            return Ok(());
        }
        Err(DirectTexelDecodeNativeError::SemanticMismatch {
            first_byte: observed
                .iter()
                .zip(expected)
                .position(|(observed, expected)| observed != expected),
        })
    }

    #[test]
    fn shader_manifest_retains_zero_row_promotion() {
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.promotion(),
            RuntimeShaderPromotion::NotQualified
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.native_state(),
            RuntimeShaderNativeState::NativeUnverified
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.rt64_source_commit(),
            DIRECT_TEXEL_DECODE_RT64_SOURCE_COMMIT
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.formats_sha256(),
            DIRECT_TEXEL_DECODE_FORMATS_SHA256
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.texture_decoder_sha256(),
            DIRECT_TEXEL_DECODE_TEXTURE_DECODER_SHA256
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.denominator_path(),
            DIRECT_TEXEL_DECODE_DENOMINATOR_PATH
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.denominator_sha256(),
            DIRECT_TEXEL_DECODE_DENOMINATOR_SHA256
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.dependency_sources(),
            &DIRECT_TEXEL_DECODE_DEPENDENCY_SOURCES
        );
        assert_eq!(
            DIRECT_TEXEL_DECODE_MANIFEST.candidate_consumers(),
            &DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS
        );
        assert_eq!(DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS.len(), 21);
        assert_eq!(DIRECT_TEXEL_DECODE_WORKGROUPS, 2_058);
    }

    #[test]
    fn triangle_pipeline_fragment_manifest_retains_zero_row_promotion_and_matches_its_own_source() {
        let manifest = TRIANGLE_PIPELINE_FRAGMENT_MANIFEST;
        assert_eq!(
            manifest.component(),
            RuntimeShaderComponentId::TrianglePipelineFragmentV1
        );
        assert_eq!(manifest.stage(), RuntimeShaderStage::Fragment);
        assert_eq!(
            manifest.entry_point(),
            TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT
        );
        assert_eq!(manifest.promotion(), RuntimeShaderPromotion::NotQualified);
        assert_eq!(
            manifest.native_state(),
            RuntimeShaderNativeState::NativeUnverified
        );
        assert_eq!(
            manifest.rt64_source_commit(),
            TRIANGLE_PIPELINE_FRAGMENT_RT64_SOURCE_COMMIT
        );
        assert_eq!(
            manifest.dependency_sources(),
            &TRIANGLE_PIPELINE_FRAGMENT_DEPENDENCY_SOURCES
        );
        assert_eq!(
            manifest.candidate_consumers(),
            &TRIANGLE_PIPELINE_FRAGMENT_CANDIDATE_CONSUMERS
        );
        assert_eq!(TRIANGLE_PIPELINE_FRAGMENT_CANDIDATE_CONSUMERS.len(), 1);
        assert_eq!(TRIANGLE_PIPELINE_FRAGMENT_CASES, 1);

        // The frozen public constant must match the live combined source,
        // not a placeholder -- this is the check that would have caught the
        // zero-value regression.
        let (source_bytes, source_sha256, fixture_sha256) =
            recompute_triangle_pipeline_fragment_digests();
        assert_eq!(manifest.source_bytes(), source_bytes);
        assert_eq!(manifest.source_sha256(), source_sha256);
        assert_eq!(manifest.fixture_sha256(), fixture_sha256);
        assert_ne!(manifest.source_sha256(), [0u8; 32]);
        assert_ne!(manifest.fixture_sha256(), [0u8; 32]);
    }

    #[test]
    fn triangle_pipeline_fragment_manifest_fixture_hash_is_hostile_to_a_source_mutation() {
        let mutated_source = format!("{}\n// mutated", triangle_pipeline_fragment_wgsl());
        let mutated_hash: [u8; 32] = {
            use sha2::{Digest as _, Sha256};
            Sha256::digest(mutated_source.as_bytes()).into()
        };
        assert_ne!(
            TRIANGLE_PIPELINE_FRAGMENT_MANIFEST.source_sha256(),
            mutated_hash
        );
    }

    // #[ignore]: prints the exact digests to paste into the
    // TRIANGLE_PIPELINE_FRAGMENT_*_SHA256/_SOURCE_BYTES constants above. Not
    // part of the default 10x loop -- one-time freeze step, matching this
    // file's other freeze-print precedents.
    #[test]
    #[ignore]
    fn triangle_pipeline_fragment_fixture_freeze_prints_digests() {
        let (source_bytes, source_sha256, fixture_sha256) =
            recompute_triangle_pipeline_fragment_digests();
        println!("source_bytes: {source_bytes}");
        println!("source: {source_sha256:02x?}");
        println!("fixture: {fixture_sha256:02x?}");
    }

    fn triangle_pipeline_vertex_fixture_identity() -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(TRIANGLE_PIPELINE_VERTEX_FIXTURE_SCHEMA.as_bytes());
        hasher.update([0]);
        hasher.update((TRIANGLE_PIPELINE_VERTEX_WGSL.len() as u64).to_be_bytes());
        hasher.update(TRIANGLE_PIPELINE_VERTEX_WGSL.as_bytes());
        hasher.finalize().into()
    }

    #[test]
    fn triangle_pipeline_vertex_manifest_retains_zero_row_promotion_and_matches_its_own_source() {
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.component(),
            RuntimeShaderComponentId::TrianglePipelineVertexV1
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.stage(),
            RuntimeShaderStage::Vertex
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.entry_point(),
            TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.promotion(),
            RuntimeShaderPromotion::NotQualified
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.native_state(),
            RuntimeShaderNativeState::NativeUnverified
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.rt64_source_commit(),
            TRIANGLE_PIPELINE_VERTEX_RT64_SOURCE_COMMIT
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.dependency_sources(),
            &TRIANGLE_PIPELINE_VERTEX_DEPENDENCY_SOURCES
        );
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.candidate_consumers(),
            &TRIANGLE_PIPELINE_VERTEX_CANDIDATE_CONSUMERS
        );
        assert_eq!(TRIANGLE_PIPELINE_VERTEX_CANDIDATE_CONSUMERS.len(), 1);
        assert_eq!(TRIANGLE_PIPELINE_VERTEX_CASES, 1);
        assert_eq!(
            TRIANGLE_PIPELINE_VERTEX_MANIFEST.source_bytes() as usize,
            TRIANGLE_PIPELINE_VERTEX_WGSL.len()
        );
        assert_eq!(
            digest(TRIANGLE_PIPELINE_VERTEX_WGSL.as_bytes()),
            TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256
        );
        assert_eq!(
            triangle_pipeline_vertex_fixture_identity(),
            TRIANGLE_PIPELINE_VERTEX_FIXTURE_SHA256
        );
    }

    #[test]
    fn triangle_pipeline_vertex_manifest_fixture_hash_is_hostile_to_a_source_mutation() {
        let mutated = format!("{TRIANGLE_PIPELINE_VERTEX_WGSL}\n// mutated");
        let mut hasher = Sha256::new();
        hasher.update(TRIANGLE_PIPELINE_VERTEX_FIXTURE_SCHEMA.as_bytes());
        hasher.update([0]);
        hasher.update((mutated.len() as u64).to_be_bytes());
        hasher.update(mutated.as_bytes());
        let mutated_hash: [u8; 32] = hasher.finalize().into();
        assert_ne!(TRIANGLE_PIPELINE_VERTEX_FIXTURE_SHA256, mutated_hash);
        assert_ne!(
            digest(mutated.as_bytes()),
            TRIANGLE_PIPELINE_VERTEX_SOURCE_SHA256
        );
    }

    // #[ignore]: prints the exact digests to paste into the
    // TRIANGLE_PIPELINE_VERTEX_*_SHA256 constants above. Not part of the
    // default 10x loop -- one-time freeze step, matching
    // `three_nearest_filter_fixture_freeze_prints_digests`'s precedent.
    #[test]
    #[ignore]
    fn triangle_pipeline_vertex_fixture_freeze_prints_digests() {
        println!(
            "source: {:02x?}",
            digest(TRIANGLE_PIPELINE_VERTEX_WGSL.as_bytes())
        );
        println!(
            "fixture: {:02x?}",
            triangle_pipeline_vertex_fixture_identity()
        );
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(DIRECT_TEXEL_DECODE_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();

        let duplicate_binding = DIRECT_TEXEL_DECODE_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    #[test]
    fn deterministic_fixture_is_exact_and_oracle_derived() {
        let fixture = fixture();
        assert_eq!(fixture.inputs.len(), DIRECT_TEXEL_DECODE_CASES as usize);
        assert_eq!(
            fixture.input_bytes.len() as u64,
            DIRECT_TEXEL_DECODE_INPUT_BYTES
        );
        assert_eq!(
            fixture.expected_bytes.len() as u64,
            DIRECT_TEXEL_DECODE_OUTPUT_BYTES
        );
        assert_eq!(
            digest(DIRECT_TEXEL_DECODE_WGSL.as_bytes()),
            DIRECT_TEXEL_DECODE_SOURCE_SHA256
        );
        assert_eq!(fixture.identity, DIRECT_TEXEL_DECODE_FIXTURE_SHA256);
        assert_eq!(
            digest(&fixture.input_bytes),
            DIRECT_TEXEL_DECODE_INPUT_SHA256
        );
        assert_eq!(
            digest(&fixture.expected_bytes),
            DIRECT_TEXEL_DECODE_EXPECTED_SHA256
        );
        verify_pre_submission(
            DIRECT_TEXEL_DECODE_WGSL,
            DIRECT_TEXEL_DECODE_ENTRY_POINT,
            &fixture,
        )
        .unwrap();
    }

    fn hex(bytes: [u8; 32]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
    }

    /// `deterministic_fixture_is_exact_and_oracle_derived` already recomputes
    /// all four DirectTexelDecodeV1 identities, but it compares against
    /// `[u8; 32]` literals. `docs/RT64-RUNTIME-SHADER-CORPUS.md` republishes
    /// the same identities as hex text, and nothing tied the two
    /// representations together -- so an identity could be re-frozen in code
    /// while the doc kept asserting the stale digest as evidence. This test
    /// owns the doc's hex rows directly: it re-derives each digest from
    /// committed inputs, renders it as hex, and requires the published table
    /// to contain exactly that string.
    #[test]
    fn published_corpus_doc_cites_the_recomputed_direct_texel_identities() {
        let doc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/RT64-RUNTIME-SHADER-CORPUS.md");
        let doc = std::fs::read_to_string(&doc_path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", doc_path.display()));

        // The published hex appears here as source text on purpose. It makes
        // the recomputation checkable against a literal a reader can compare
        // to the doc by eye, and it is what `scripts/lint-docs.py` greps for
        // when it asks whether a documented hash is owned by a test.
        let fixture = fixture();
        for (row, actual, published) in [
            (
                "source SHA-256",
                digest(DIRECT_TEXEL_DECODE_WGSL.as_bytes()),
                "2f59380f62db77f1c11b81e149894947d01ad8c812ee11e3771125317fff3880",
            ),
            (
                "fixture SHA-256",
                fixture.identity,
                "f24aca795ae8954ae362280cd91c75017623bf7db1601688e9bc2775dcfb7d37",
            ),
            (
                "input SHA-256",
                digest(&fixture.input_bytes),
                "6199dd74587f3a8ca86e24fbab5949bb0dcb04db8c4c47f3e9a65504edd9c274",
            ),
            (
                "expected-output SHA-256",
                digest(&fixture.expected_bytes),
                "89bce88b397fa2a5e08eb1549498eba49465368968ff844ed0e42e407c17114f",
            ),
        ] {
            assert_eq!(
                hex(actual),
                published,
                "DirectTexelDecodeV1 {row} recomputed from committed inputs no \
                 longer equals the frozen identity",
            );
            let expected = format!("| {row} | `{published}` |");
            assert!(
                doc.lines().any(|line| line.trim() == expected),
                "docs/RT64-RUNTIME-SHADER-CORPUS.md must publish the recomputed \
                 DirectTexelDecodeV1 {row} row as `{expected}`",
            );
        }
    }

    #[test]
    fn typed_profile_rejects_each_insufficient_limit() {
        let profile = DirectTexelDecodeDeviceProfile;
        let required = profile.required_limits();
        assert_eq!(profile.required_features(), wgpu::Features::empty());
        let validated = profile
            .validate_adapter(wgpu::Features::empty(), &required)
            .unwrap();
        assert_eq!(validated.requested_features(), wgpu::Features::empty());
        assert_eq!(validated.requested_limits(), &required);
        profile
            .validate_request_contract(wgpu::Features::empty(), &required)
            .unwrap();

        let mut short = required.clone();
        short.max_buffer_size = DIRECT_TEXEL_DECODE_INPUT_BYTES - 1;
        assert!(matches!(
            profile.validate_adapter(wgpu::Features::empty(), &short),
            Err(DirectTexelDecodeProfileError::LimitTooSmall {
                name: "max_buffer_size",
                ..
            })
        ));
        short = required.clone();
        short.max_storage_buffers_per_shader_stage = 1;
        assert!(matches!(
            profile.validate_adapter(wgpu::Features::empty(), &short),
            Err(DirectTexelDecodeProfileError::LimitTooSmall {
                name: "max_storage_buffers_per_shader_stage",
                ..
            })
        ));
        short = required.clone();
        short.max_storage_buffer_binding_size = DIRECT_TEXEL_DECODE_INPUT_BYTES - 1;
        assert!(profile
            .validate_adapter(wgpu::Features::empty(), &short)
            .is_err());
        short = required.clone();
        short.max_compute_workgroup_size_x = 63;
        assert!(profile
            .validate_adapter(wgpu::Features::empty(), &short)
            .is_err());
        short = required.clone();
        short.max_compute_invocations_per_workgroup = 63;
        assert!(profile
            .validate_adapter(wgpu::Features::empty(), &short)
            .is_err());
        short = required.clone();
        short.max_compute_workgroups_per_dimension = DIRECT_TEXEL_DECODE_WORKGROUPS - 1;
        assert!(profile
            .validate_adapter(wgpu::Features::empty(), &short)
            .is_err());
        let mut changed = profile.required_limits();
        changed.max_buffer_size += 1;
        assert_eq!(
            profile.validate_request_contract(wgpu::Features::empty(), &changed),
            Err(DirectTexelDecodeProfileError::RequestedLimitsMismatch)
        );
        changed = profile.required_limits();
        changed.max_buffer_size -= 1;
        assert_eq!(
            profile.validate_request_contract(wgpu::Features::empty(), &changed),
            Err(DirectTexelDecodeProfileError::RequestedLimitsMismatch)
        );
        assert_eq!(
            profile.validate_request_contract(wgpu::Features::IMMEDIATES, &required),
            Err(DirectTexelDecodeProfileError::UnexpectedRequestedFeatures)
        );
    }

    #[test]
    fn hostile_mutation_matrix_fails_closed() {
        let fixture = fixture();
        let source_mutations = [
            ("source byte", format!("{DIRECT_TEXEL_DECODE_WGSL} ")),
            (
                "binding order",
                DIRECT_TEXEL_DECODE_WGSL
                    .replace("@binding(0)", "@binding(9)")
                    .replace("@binding(1)", "@binding(0)")
                    .replace("@binding(9)", "@binding(1)"),
            ),
            (
                "binding type/access",
                DIRECT_TEXEL_DECODE_WGSL.replace(
                    "var<storage, read> inputs",
                    "var<storage, read_write> inputs",
                ),
            ),
            (
                "output access",
                DIRECT_TEXEL_DECODE_WGSL.replace(
                    "var<storage, read_write> outputs",
                    "var<storage, read> outputs",
                ),
            ),
            (
                "RGBA channel swap",
                DIRECT_TEXEL_DECODE_WGSL.replace(
                    "(red << 24u) | (green << 16u) | (blue << 8u) | alpha",
                    "(blue << 24u) | (green << 16u) | (red << 8u) | alpha",
                ),
            ),
            (
                "RGBA16 shift-only expansion",
                DIRECT_TEXEL_DECODE_WGSL.replace("(value << 3u) | (value >> 2u)", "value << 3u"),
            ),
            (
                "IA alpha/intensity nibble",
                DIRECT_TEXEL_DECODE_WGSL.replace(
                    "let alpha_nibble = input.value & 0xfu;",
                    "let alpha_nibble = (input.value >> 4u) & 0xfu;",
                ),
            ),
        ];
        for (name, source) in source_mutations {
            assert!(
                matches!(
                    verify_pre_submission(&source, DIRECT_TEXEL_DECODE_ENTRY_POINT, &fixture),
                    Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch { field: "source" })
                ),
                "source mutation admitted: {name}"
            );
        }
        assert!(matches!(
            verify_pre_submission(DIRECT_TEXEL_DECODE_WGSL, "mutated_entry", &fixture),
            Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch {
                field: "entry point"
            })
        ));

        for (name, byte_offset, value) in [
            ("format", 0, 5_u32),
            ("size", 4, 4_u32),
            ("raw width", 8, 16_u32),
            ("reserved", 12, 1_u32),
        ] {
            let mut mutated = fixture.clone();
            mutated.input_bytes[byte_offset..byte_offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                matches!(
                    verify_pre_submission(
                        DIRECT_TEXEL_DECODE_WGSL,
                        DIRECT_TEXEL_DECODE_ENTRY_POINT,
                        &mutated,
                    ),
                    Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch { field: "input" })
                ),
                "input mutation admitted: {name}"
            );
        }
        assert!(RawTexel::try_new(PixelSize::Bits4, 16).is_err());

        let expected = &fixture.expected_bytes;
        assert_ne!(&expected[..8], &expected[16..24]);
        let mut reordered = expected.clone();
        let (unsupported, direct_and_rest) = reordered.split_at_mut(16);
        unsupported[..8].swap_with_slice(&mut direct_and_rest[..8]);
        let mut omitted = expected.clone();
        omitted.drain(..8);
        let mut duplicated = expected.clone();
        duplicated.extend_from_slice(&expected[..8]);
        let mut truncated = expected.clone();
        truncated.pop();
        let mut appended = expected.clone();
        appended.push(0);
        for (name, observed) in [
            ("reordered", reordered),
            ("omitted", omitted),
            ("duplicated", duplicated),
            ("truncated", truncated),
            ("appended", appended),
        ] {
            assert!(
                verify_observed(expected, &observed).is_err(),
                "output mutation admitted: {name}"
            );
        }
    }

    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn direct_texel_decode_native() {
        let receipt = block_on(run_native()).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            receipt.component(),
            RuntimeShaderComponentId::DirectTexelDecodeV1
        );
        assert_eq!(receipt.case_count(), DIRECT_TEXEL_DECODE_CASES);
        assert_eq!(receipt.output_bytes(), DIRECT_TEXEL_DECODE_OUTPUT_BYTES);
        assert_eq!(receipt.entry_point(), DIRECT_TEXEL_DECODE_ENTRY_POINT);
        assert_eq!(receipt.input_sha256(), DIRECT_TEXEL_DECODE_INPUT_SHA256);
        assert!(receipt.pipeline_creation_succeeded());
        assert_eq!(receipt.expected_sha256(), receipt.observed_sha256());
        assert!(receipt.exact_submission_complete());
        assert!(receipt.callback_observed());
        assert_eq!(receipt.validation_error_count(), 0);
        assert_eq!(receipt.source_sha256(), DIRECT_TEXEL_DECODE_SOURCE_SHA256);
        assert_eq!(receipt.fixture_sha256(), DIRECT_TEXEL_DECODE_FIXTURE_SHA256);
        assert_eq!(receipt.requested_features(), wgpu::Features::empty());
        assert_eq!(
            receipt.requested_limits(),
            &DirectTexelDecodeDeviceProfile.required_limits()
        );
        assert!(!receipt.adapter().name.is_empty());
    }

    #[cfg(feature = "host-gpu-tests")]
    async fn run_native() -> Result<DirectTexelDecodeNativeReceipt, DirectTexelDecodeNativeError> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let fixture = fixture();
        verify_pre_submission(
            DIRECT_TEXEL_DECODE_WGSL,
            DIRECT_TEXEL_DECODE_ENTRY_POINT,
            &fixture,
        )?;
        let profile = DirectTexelDecodeDeviceProfile;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::device::adapter_selection::backends_for_request(
                wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ),
            flags: wgpu::InstanceFlags::VALIDATION,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| {
                DirectTexelDecodeNativeError::NativeAdapterUnavailable(error.to_string())
            })?;
        crate::device::adapter_selection::assert_expected_adapter(&adapter);
        let validated = profile
            .validate_adapter(adapter.features(), &adapter.limits())
            .map_err(DirectTexelDecodeNativeError::Profile)?;
        let requested_limits = validated.requested_limits().clone();
        let requested_features = validated.requested_features();
        profile
            .validate_request_contract(requested_features, &requested_limits)
            .map_err(DirectTexelDecodeNativeError::Profile)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-m2.5.3a-direct-texel"),
                required_features: requested_features,
                required_limits: requested_limits.clone(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| DirectTexelDecodeNativeError::RequestDevice(error.to_string()))?;
        let uncaptured = Arc::new(AtomicUsize::new(0));
        let error_count = Arc::clone(&uncaptured);
        device.on_uncaptured_error(Arc::new(move |_| {
            error_count.fetch_add(1, Ordering::Relaxed);
        }));

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel"),
            source: wgpu::ShaderSource::Wgsl(DIRECT_TEXEL_DECODE_WGSL.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel"),
            layout: None,
            module: &module,
            entry_point: Some(DIRECT_TEXEL_DECODE_ENTRY_POINT),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = scope.pop().await {
            return Err(DirectTexelDecodeNativeError::PipelineValidation(
                error.to_string(),
            ));
        }

        let input_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel-input"),
            size: DIRECT_TEXEL_DECODE_INPUT_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel-output"),
            size: DIRECT_TEXEL_DECODE_OUTPUT_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel-readback"),
            size: DIRECT_TEXEL_DECODE_OUTPUT_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        verify_pre_submission(
            DIRECT_TEXEL_DECODE_WGSL,
            DIRECT_TEXEL_DECODE_ENTRY_POINT,
            &fixture,
        )?;
        profile
            .validate_request_contract(requested_features, &requested_limits)
            .map_err(DirectTexelDecodeNativeError::Profile)?;
        queue.write_buffer(&input_buffer, 0, &fixture.input_bytes);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel-bind-group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-m2.5.3a-direct-texel"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-m2.5.3a-direct-texel"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(DIRECT_TEXEL_DECODE_WORKGROUPS, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &output_buffer,
            0,
            &readback,
            0,
            DIRECT_TEXEL_DECODE_OUTPUT_BYTES,
        );
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let submission = queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|error| {
                DirectTexelDecodeNativeError::ExactSubmissionWait(error.to_string())
            })?;
        callback_receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| DirectTexelDecodeNativeError::CompletionCallbackNotObserved)?;

        let (map_sender, map_receiver) = mpsc::sync_channel(1);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = map_sender.try_send(result);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|error| DirectTexelDecodeNativeError::MapWait(error.to_string()))?;
        map_receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| DirectTexelDecodeNativeError::MapCallbackNotObserved)?
            .map_err(|error| DirectTexelDecodeNativeError::Map(error.to_string()))?;
        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .map_err(|error| DirectTexelDecodeNativeError::MappedRange(error.to_string()))?;
        let observed = mapped.to_vec();
        drop(mapped);
        readback.unmap();
        verify_observed(&fixture.expected_bytes, &observed)?;
        let validation_error_count = uncaptured.load(Ordering::Relaxed);
        if validation_error_count != 0 {
            return Err(DirectTexelDecodeNativeError::UncapturedErrors {
                count: validation_error_count,
            });
        }
        Ok(DirectTexelDecodeNativeReceipt {
            component: RuntimeShaderComponentId::DirectTexelDecodeV1,
            adapter: adapter.get_info(),
            entry_point: DIRECT_TEXEL_DECODE_ENTRY_POINT,
            requested_features,
            requested_limits,
            source_sha256: digest(DIRECT_TEXEL_DECODE_WGSL.as_bytes()),
            fixture_sha256: fixture.identity,
            input_sha256: digest(&fixture.input_bytes),
            case_count: DIRECT_TEXEL_DECODE_CASES,
            expected_sha256: digest(&fixture.expected_bytes),
            observed_sha256: digest(&observed),
            output_bytes: observed.len() as u64,
            pipeline_creation_succeeded: true,
            exact_submission_complete: true,
            callback_observed: true,
            validation_error_count,
        })
    }

    #[cfg(feature = "host-gpu-tests")]
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct ThreadWake(std::thread::Thread);

        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }

        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    // M2.5.3b: three-nearest triangular filter, ported verbatim from
    // fn64-render-reference/src/gbi/types.rs:954-972. filter_three_nearest_s10_5
    // is pub(super) to fn64-render-reference::gbi, not pub -- widening its
    // visibility is out of this card's scope, so this oracle duplicates the
    // reference sweep's literal seed/formula logic (group4.rs:467-501)
    // rather than cross-crate-calling it, matching this crate's existing
    // self-contained-fixture convention.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ThreeNearestFilterShaderInput {
        c00: u32,
        c10: u32,
        c01: u32,
        c11: u32,
        sf: u32,
        tf: u32,
    }

    impl ThreeNearestFilterShaderInput {
        fn encode(self, bytes: &mut Vec<u8>) {
            bytes.extend_from_slice(&self.c00.to_le_bytes());
            bytes.extend_from_slice(&self.c10.to_le_bytes());
            bytes.extend_from_slice(&self.c01.to_le_bytes());
            bytes.extend_from_slice(&self.c11.to_le_bytes());
            bytes.extend_from_slice(&self.sf.to_le_bytes());
            bytes.extend_from_slice(&self.tf.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
        }
    }

    const THREE_NEAREST_STATUS_OK: u32 = 0;
    const THREE_NEAREST_STATUS_INVALID_FRACTION: u32 = 1;

    fn monochrome_rgba8888(value: u8) -> u32 {
        u32::from_be_bytes([value; 4])
    }

    // Oracle: same round-to-nearest/clamp policy as
    // fn64-render-reference::filter_three_nearest_s10_5, applied per channel;
    // every fixture corner here is monochrome ([value; 4]) so a single
    // channel oracle covers all four channels identically, matching
    // group4.rs's own sweep shape.
    fn oracle_three_nearest(samples: [u8; 4], sf: i64, tf: i64) -> Option<u32> {
        if !(0..32).contains(&sf) || !(0..32).contains(&tf) {
            return None;
        }
        let [c00, c10, c01, c11] = samples.map(i64::from);
        let value = if sf + tf <= 32 {
            c00 * 32 + sf * (c10 - c00) + tf * (c01 - c00)
        } else {
            c11 * 32 + (32 - sf) * (c01 - c11) + (32 - tf) * (c10 - c11)
        };
        let channel = ((value + 16) / 32).clamp(0, 255) as u8;
        Some(monochrome_rgba8888(channel))
    }

    #[derive(Clone)]
    struct ThreeNearestFixture {
        inputs: Vec<ThreeNearestFilterShaderInput>,
        input_bytes: Vec<u8>,
        expected_bytes: Vec<u8>,
        identity: [u8; 32],
    }

    fn three_nearest_fixture() -> ThreeNearestFixture {
        let mut inputs = Vec::with_capacity(THREE_NEAREST_FILTER_CASES as usize);
        for seed in 0..=255u16 {
            let values = [
                seed as u8,
                seed.wrapping_mul(73).wrapping_add(19) as u8,
                seed.wrapping_mul(151).wrapping_add(41) as u8,
                seed.wrapping_mul(211).wrapping_add(97) as u8,
            ];
            for sf in 0..32u32 {
                for tf in 0..32u32 {
                    inputs.push(ThreeNearestFilterShaderInput {
                        c00: monochrome_rgba8888(values[0]),
                        c10: monochrome_rgba8888(values[1]),
                        c01: monochrome_rgba8888(values[2]),
                        c11: monochrome_rgba8888(values[3]),
                        sf,
                        tf,
                    });
                }
            }
        }
        assert_eq!(inputs.len(), THREE_NEAREST_FILTER_CASES as usize);

        let mut input_bytes = Vec::with_capacity(inputs.len() * 32);
        let mut expected_bytes = Vec::with_capacity(inputs.len() * 8);
        for &value in &inputs {
            value.encode(&mut input_bytes);
            let samples = [
                (value.c00 & 0xff) as u8,
                (value.c10 & 0xff) as u8,
                (value.c01 & 0xff) as u8,
                (value.c11 & 0xff) as u8,
            ];
            match oracle_three_nearest(samples, i64::from(value.sf), i64::from(value.tf)) {
                Some(rgba) => {
                    expected_bytes.extend_from_slice(&THREE_NEAREST_STATUS_OK.to_le_bytes());
                    expected_bytes.extend_from_slice(&rgba.to_le_bytes());
                }
                None => {
                    expected_bytes
                        .extend_from_slice(&THREE_NEAREST_STATUS_INVALID_FRACTION.to_le_bytes());
                    expected_bytes.extend_from_slice(&0_u32.to_le_bytes());
                }
            }
        }
        assert_eq!(input_bytes.len() as u64, THREE_NEAREST_FILTER_INPUT_BYTES);
        assert_eq!(
            expected_bytes.len() as u64,
            THREE_NEAREST_FILTER_OUTPUT_BYTES
        );
        let mut hasher = Sha256::new();
        hasher.update(THREE_NEAREST_FILTER_FIXTURE_SCHEMA.as_bytes());
        hasher.update([0]);
        hasher.update((input_bytes.len() as u64).to_be_bytes());
        hasher.update(&input_bytes);
        hasher.update((expected_bytes.len() as u64).to_be_bytes());
        hasher.update(&expected_bytes);
        let identity = hasher.finalize().into();
        ThreeNearestFixture {
            inputs,
            input_bytes,
            expected_bytes,
            identity,
        }
    }

    #[test]
    fn three_nearest_hand_worked_cases_match_reference_arithmetic() {
        // Both cases reused verbatim from the card's worked example (§4),
        // using the reference formula's own corner-name order.
        assert_eq!(
            oracle_three_nearest([100, 150, 50, 200], 8, 8),
            Some(monochrome_rgba8888(100))
        );
        assert_eq!(
            oracle_three_nearest([100, 150, 50, 200], 24, 24),
            Some(monochrome_rgba8888(150))
        );
    }

    #[test]
    fn three_nearest_value_is_never_negative_for_valid_inputs() {
        // §4's own conclusion, checked exhaustively over the byte/fraction
        // domain rather than argued by hand: value = c00*(32-sf-tf) +
        // sf*c10 + tf*c01 (lower branch) is a sum of non-negative terms, and
        // the symmetric substitution holds for the upper branch, so
        // toward-zero vs toward-negative-infinity rounding never diverges.
        for c00 in [0u8, 1, 127, 255] {
            for c10 in [0u8, 1, 127, 255] {
                for c01 in [0u8, 1, 127, 255] {
                    for c11 in [0u8, 1, 127, 255] {
                        for sf in 0..32i64 {
                            for tf in 0..32i64 {
                                let [c00, c10, c01, c11] = [c00, c10, c01, c11].map(i64::from);
                                let value = if sf + tf <= 32 {
                                    c00 * 32 + sf * (c10 - c00) + tf * (c01 - c00)
                                } else {
                                    c11 * 32 + (32 - sf) * (c01 - c11) + (32 - tf) * (c10 - c11)
                                };
                                assert!(value >= 0, "negative value: {value}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn three_nearest_manifest_retains_zero_row_promotion() {
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.promotion(),
            RuntimeShaderPromotion::NotQualified
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.native_state(),
            RuntimeShaderNativeState::NativeUnverified
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.rt64_source_commit(),
            THREE_NEAREST_FILTER_RT64_SOURCE_COMMIT
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.formats_sha256(),
            THREE_NEAREST_FILTER_FORMATS_SHA256
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.texture_decoder_sha256(),
            THREE_NEAREST_FILTER_TEXTURE_DECODER_SHA256
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.denominator_path(),
            THREE_NEAREST_FILTER_DENOMINATOR_PATH
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.denominator_sha256(),
            THREE_NEAREST_FILTER_DENOMINATOR_SHA256
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.dependency_sources(),
            &THREE_NEAREST_FILTER_DEPENDENCY_SOURCES
        );
        assert_eq!(
            THREE_NEAREST_FILTER_MANIFEST.candidate_consumers(),
            &THREE_NEAREST_FILTER_CANDIDATE_CONSUMERS
        );
        assert_eq!(THREE_NEAREST_FILTER_CANDIDATE_CONSUMERS.len(), 0);
        assert_eq!(THREE_NEAREST_FILTER_WORKGROUPS, 4_096);
    }

    #[test]
    fn three_nearest_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(THREE_NEAREST_FILTER_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();

        let duplicate_binding = THREE_NEAREST_FILTER_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    fn three_nearest_verify_pre_submission(
        source: &str,
        entry_point: &str,
        fixture: &ThreeNearestFixture,
    ) -> Result<(), DirectTexelDecodeNativeError> {
        for (field, actual, expected) in [
            (
                "source",
                digest(source.as_bytes()),
                THREE_NEAREST_FILTER_SOURCE_SHA256,
            ),
            (
                "fixture",
                fixture.identity,
                THREE_NEAREST_FILTER_FIXTURE_SHA256,
            ),
            (
                "input",
                digest(&fixture.input_bytes),
                THREE_NEAREST_FILTER_INPUT_SHA256,
            ),
            (
                "expected output",
                digest(&fixture.expected_bytes),
                THREE_NEAREST_FILTER_EXPECTED_SHA256,
            ),
        ] {
            if actual != expected {
                return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch { field });
            }
        }
        if entry_point != THREE_NEAREST_FILTER_ENTRY_POINT {
            return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch {
                field: "entry point",
            });
        }
        if fixture.inputs.len() != THREE_NEAREST_FILTER_CASES as usize
            || fixture.input_bytes.len() as u64 != THREE_NEAREST_FILTER_INPUT_BYTES
            || fixture.expected_bytes.len() as u64 != THREE_NEAREST_FILTER_OUTPUT_BYTES
        {
            return Err(DirectTexelDecodeNativeError::FrozenIdentityMismatch {
                field: "fixture shape",
            });
        }
        Ok(())
    }

    fn three_nearest_verify_observed(
        expected: &[u8],
        observed: &[u8],
    ) -> Result<(), DirectTexelDecodeNativeError> {
        if observed == expected {
            return Ok(());
        }
        Err(DirectTexelDecodeNativeError::SemanticMismatch {
            first_byte: observed
                .iter()
                .zip(expected)
                .position(|(observed, expected)| observed != expected),
        })
    }

    #[test]
    fn three_nearest_deterministic_fixture_is_exact_and_oracle_derived() {
        let fixture = three_nearest_fixture();
        assert_eq!(fixture.inputs.len(), THREE_NEAREST_FILTER_CASES as usize);
        assert_eq!(
            fixture.input_bytes.len() as u64,
            THREE_NEAREST_FILTER_INPUT_BYTES
        );
        assert_eq!(
            fixture.expected_bytes.len() as u64,
            THREE_NEAREST_FILTER_OUTPUT_BYTES
        );
        assert_eq!(
            digest(THREE_NEAREST_FILTER_WGSL.as_bytes()),
            THREE_NEAREST_FILTER_SOURCE_SHA256
        );
        assert_eq!(fixture.identity, THREE_NEAREST_FILTER_FIXTURE_SHA256);
        assert_eq!(
            digest(&fixture.input_bytes),
            THREE_NEAREST_FILTER_INPUT_SHA256
        );
        assert_eq!(
            digest(&fixture.expected_bytes),
            THREE_NEAREST_FILTER_EXPECTED_SHA256
        );
        three_nearest_verify_pre_submission(
            THREE_NEAREST_FILTER_WGSL,
            THREE_NEAREST_FILTER_ENTRY_POINT,
            &fixture,
        )
        .unwrap();
        assert!(
            three_nearest_verify_observed(&fixture.expected_bytes, &fixture.expected_bytes).is_ok()
        );
        let mut mutated = fixture.expected_bytes.clone();
        mutated[0] ^= 1;
        assert!(three_nearest_verify_observed(&fixture.expected_bytes, &mutated).is_err());
    }

    // #[ignore]: prints the exact digests to paste into the
    // THREE_NEAREST_FILTER_*_SHA256 constants above. Not part of the default
    // 10x loop -- this is a one-time freeze step, matching M2.5.3a's own
    // history (frozen by the PR that introduced the component, not before).
    #[test]
    #[ignore]
    fn three_nearest_filter_fixture_freeze_prints_digests() {
        let fixture = three_nearest_fixture();
        println!(
            "source: {:02x?}",
            digest(THREE_NEAREST_FILTER_WGSL.as_bytes())
        );
        println!("fixture: {:02x?}", fixture.identity);
        println!("input: {:02x?}", digest(&fixture.input_bytes));
        println!("expected: {:02x?}", digest(&fixture.expected_bytes));
    }
}


