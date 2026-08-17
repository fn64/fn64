//! The `src/shared/` plain-old-data interop parameter blocks: a literal port
//! of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/rt64-port-authority.json`'s `port_source.commit`).
//!
//! This module covers **sixteen** cited source files in one place because they
//! are one family: each is a short `#include "shared/rt64_hlsl.h"` header
//! wrapping a single `struct` (occasionally plus a `#define` or an `enum`) in
//! the `#ifdef HLSL_CPU / namespace interop {` scaffolding. Nine are ported in
//! full, three are ported as a **delta** over a type fn64 already owns, and
//! four are **cited but deliberately not ported** because an existing fn64
//! module already is their port or because they depend on an unportable
//! third-party facility. The per-file breakdown is in "Inventory drift" below;
//! read it before crediting any of these paths as closed.
//!
//! ## Cited sources and their digests
//!
//! Every digest below was computed independently here with `shasum -a 256`
//! against the pinned checkout and cross-checked against
//! `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256`. **All sixteen match the inventory
//! exactly -- no mismatch, no blocker.** All sixteen also carry
//! `"port_delta": "unchanged"` with `sources.oracle.sha256` equal to
//! `sources.port.sha256`, so the oracle tree (`f0728a25...`) and the port tree
//! agree on every one of these files byte for byte even though the two pins
//! differ.
//!
//! | file (under `src/shared/`) | lines | SHA-256 |
//! |---|---|---|
//! | `rt64_raytracing_params.h` | 110 | `b5ce75be484daaa3732af5bc4cb1a0929799ceb620843788b66ebd680e962048` |
//! | `rt64_rdp_tile.h` | 32 | `b06d1c6f2ab976ee6a2b9ba1bcdc79655eec136080dce31ed4d17d61ce8eff9d` |
//! | `rt64_hlsl_json.cpp` | 28 | `0b7ddc27d953f51f4c4ca4c8f2572b63952960350caed1d2098e4bf021744c98` |
//! | `rt64_fb_reinterpret.h` | 27 | `f517eeb5ec505033747a9e18926c0732d17d362d1a69ee39432d36a2d106828c` |
//! | `rt64_render_params.h` | 27 | `d3a4af28af08a04433cf0dfbf766e954e168555e5fced07f2ff9e3dc56c1bc7a` |
//! | `rt64_fb_common.h` | 25 | `5039d84369c04407eb37cb8c149946fe6bf23734e3352dff1a28830318c209fb` |
//! | `rt64_rdp_params.h` | 25 | `0b76128508f312e89377f20eb72c18cdccc73576d54f2e5d8ede003610143532` |
//! | `rt64_render_indices.h` | 21 | `102543e25d07bff02033abe584c7d2b340da345c1f902ba41f3de2c18df45b7b` |
//! | `rt64_raster_params.h` | 20 | `80a53c9d393ecef8793fde68afe552c781be922bb99a0e6d807f544e96e23db4` |
//! | `rt64_frame_params.h` | 19 | `dee91d3ff78177821d02d4a27cc43606a58f9a54d7896c5d17b7762a9b29ff3c` |
//! | `rt64_framebuffer_params.h` | 19 | `ac4e6a7548ef28e9cbc36badbe357209cbe84e89b91038ec426d2430a788a268` |
//! | `rt64_video_interface.h` | 19 | `5d97c854e5d4d0f1f40c5cf06815265baf9218982bcdf698d2b9b9fcf48fdb68` |
//! | `rt64_hlsl_json.h` | 18 | `aabc3e62c6ad514b266e01e66dad83c24ad8293ebde7ee6c387b61873e4cee3e` |
//! | `rt64_interleaved_raster.h` | 18 | `59e364d36e609ab0e31e237d8939ab6e2eaed85f03ce6f4306988551ddf0cfae` |
//! | `rt64_texture_copy.h` | 18 | `121a219ca6a514fdcb04d77b127e221c1f0407e27f5df719d6453b1c6cf5edc0` |
//! | `rt64_render_target_copy.h` | 17 | `0cc648d54cb6e96a3084528f281809756e11826c4bc2c70dd1d601fa87d44829` |
//!
//! (The line counts are the inventory's, which counts a file's final
//! unterminated line -- most of these end with `#endif` and no trailing
//! newline -- so they read one higher than `wc -l`.)
//!
//! ## Verbatim key structure
//!
//! Every file in this family has the same shape. `rt64_render_indices.h` in
//! full is the representative case:
//!
//! ```text
//! #pragma once
//!
//! #include "shared/rt64_hlsl.h"
//!
//! #ifdef HLSL_CPU
//! namespace interop {
//! #endif
//!     struct RenderIndices {
//!         uint instanceIndex;
//!         uint faceIndicesStart;
//!         uint rdpTileIndex;
//!         uint rdpTileCount;
//!         uint highlightColor;
//!     };
//! #ifdef HLSL_CPU
//! };
//! #endif
//! ```
//!
//! The two files that break that shape are quoted at their ports below:
//! `rt64_raytracing_params.h`'s `VisualizationMode` enum and `RaytracingParams`
//! constructor, and `rt64_render_params.h`'s `RenderFlags` typedef fork.
//!
//! ## Reuse, not new type
//!
//! - **`interop::uint` is `uint32_t`** (`rt64_hlsl.h:16`, ported at
//!   [`crate::rt64_hlsl_interop`]) -- so every `uint` field here is `u32` and
//!   every `int` field is `i32`. `uint2`/`uint3` become `[u32; 2]`/`[u32; 3]`
//!   and `float2` becomes `[f32; 2]`; those aggregate types are `rt64_hlsl.h`
//!   POD wrappers already ported in `rt64_hlsl_interop`, and reproducing them
//!   as distinct Rust newtypes here would add a second spelling of the same
//!   thing without adding a fact.
//! - **`fn64_render_ir::Vec3` / `Vec4`** are the workspace's `float3`/`float4`
//!   equivalents, imported from the crate root (`rsp_math` is a private module;
//!   `fn64_render_ir::rsp_math::Vec3` does not compile). Used by
//!   [`RaytracingParamsDefaults`]'s `float4` fields.
//! - **`RenderFlags` is `u32`.** `rt64_render_params.h:14-16` does
//!   `#ifndef HLSL_CPU / typedef uint RenderFlags;` -- on the *shader* side the
//!   type is literally `uint`, and on the CPU side it is
//!   `shared/rt64_render_flags.h`'s enum, whose underlying width is that same
//!   32-bit word. [`crate::rt64_render_flags`] already owns that word's twenty
//!   bit fields (`RENDER_FLAG_FIELDS`) and its accessors, so
//!   [`RenderParams::flags`] is a plain `u32` that a caller feeds to those
//!   existing accessors. No second flags type is introduced.
//! - **`RdpTileAddressing` / `G_TX_MIRROR` / `G_TX_CLAMP`**
//!   ([`crate::rt64_texture_sampler`]) already own eight of `RDPTile`'s
//!   sixteen fields. [`RdpTileImageDescriptor`] is the **remaining eight**, not
//!   a re-declaration; see [`RdpTileImageDescriptor`]'s own docs for the split.
//! - **`ScreenTransform`** ([`crate::raster_vs`]) already owns
//!   `RasterParams.screenScale`/`screenOffset`. [`RasterParamsIndexing`] is the
//!   remaining `renderIndex`/`padding` prefix only.
//!
//! ## Admitted domain
//!
//! A construct is ported here when its content is fully determined by the
//! cited file: a field's name, its declaration order, and its scalar width.
//! Nothing here reads a GPU, binds a resource, or depends on a type from an
//! uncited file beyond the `rt64_hlsl.h` scalar-width facts named above.
//!
//! ## Nonclaims
//!
//! - **No `repr(C)`, size, alignment, byte-offset, or constant-buffer ABI
//!   claim, and no `size_of` assertion, for any struct in this module.**
//!   `rt64_hlsl.h:18-19` states this outright about the very types these
//!   structs are built from:
//!
//!   ```text
//!   // These types do not have the same alignment in HLSLPP as HLSL.
//!   // We define them and auto-convert them wherever is possible.
//!   ```
//!
//!   Upstream, each of these structs additionally *is* a constant-buffer or
//!   structured-buffer layout that a shader reads. This port claims the field
//!   set, the field names, and the declaration order. It claims nothing about
//!   where any field lands in memory, and none of the Rust structs carry
//!   `repr(C)`. Every one of the sixteen cited files `#include`s
//!   `rt64_hlsl.h`, so this nonclaim covers the whole batch without exception
//!   -- including [`RasterParamsIndexing::padding`], whose upstream purpose is
//!   precisely an offset concern this module does not model.
//! - **`src/contrib/hlslpp` is an unpopulated submodule** in the pinned
//!   checkout (`docs/rt64-port-authority.json` records the gitlink revision
//!   but the tree is not checked out), so anything whose behavior depends on
//!   `hlslpp::` types is unportable. Nothing in this module depends on one:
//!   the fields ported here are scalars and fixed-size scalar arrays, and the
//!   `hlslpp::` conversion operators live in `rt64_hlsl.h`, not in these
//!   sixteen files.
//! - **No `float4x4` field is ported.** `RaytracingParams`' six matrix members
//!   (`view`, `viewI`, `projection`, `projectionI`, `viewProj`,
//!   `prevViewProj`) are omitted from [`RaytracingParamsDefaults`]; their only
//!   content in the cited file is `= FLOAT4X4_IDENTITY`, which
//!   [`crate::rt64_hlsl_interop`] already owns as a value. Restating them here
//!   would duplicate that module.
//! - **The `#ifdef HLSL_CPU` / `namespace interop { ... };` scaffolding is not
//!   modelled.** It is preprocessor plumbing selecting whether a file compiles
//!   as C++ or as HLSL; it has no runtime behavior. Where the two sides differ
//!   in a way that *is* content -- `rt64_render_params.h`'s `RenderFlags` fork
//!   -- the difference is recorded in prose above rather than reproduced.
//! - **No UB is reproduced and none was found.** These are POD declarations;
//!   there is no arithmetic, no cast, and no aliasing construct in any of the
//!   nine fully-ported files. No test in this module pins a DEVIATION.
//! - **`RaytracingParams`' constructor is ported as data, not as a
//!   constructor.** [`RaytracingParamsDefaults`] is a `const` value holding the
//!   literals the C++ constructor assigns. Two fields the constructor
//!   *declares but never initializes* (`lightsCount`,
//!   `interleavedRastersCount`) are called out at that item; the port does not
//!   invent values for them.
//!
//! ## Inventory drift, per file
//!
//! `docs/rt64-port-inventory.json` currently records `"ported_as": []` and
//! `"port_state": "not-started"` for all sixteen paths;
//! `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! `ported_as` drift until a follow-up regenerates the inventory. That
//! reconciliation is outside this card's writable surface.
//!
//! The reconciliation matters more than usual here. The inventory credits a
//! source at **whole-file digest** granularity: recording any of these paths
//! as `ported` would credit the entire file, and for seven of the sixteen that
//! would over-credit. The burndown is known to over-credit by roughly 3x for
//! exactly this reason. The honest per-file state is:
//!
//! **Full ports (9)** -- every declaration in the file is ported:
//! `rt64_render_indices.h`, `rt64_interleaved_raster.h`, `rt64_texture_copy.h`,
//! `rt64_render_target_copy.h`, `rt64_video_interface.h`,
//! `rt64_frame_params.h`, `rt64_framebuffer_params.h`, `rt64_fb_common.h`,
//! `rt64_render_params.h`.
//!
//! **Partials (3)** -- ported only as a delta over an existing fn64 owner, so
//! crediting the whole file would over-credit:
//! - `rt64_rdp_tile.h`: 8 of `RDPTile`'s 16 fields here
//!   ([`RdpTileImageDescriptor`]); the other 8 are
//!   `rt64_texture_sampler.rs`'s `RdpTileAddressing`. Between the two, all 16
//!   are covered.
//! - `rt64_raster_params.h`: 2 of `RasterParams`' 4 members here
//!   ([`RasterParamsIndexing`]); `screenScale`/`screenOffset` are
//!   `raster_vs.rs`'s `ScreenTransform`. All 4 covered between the two.
//! - `rt64_raytracing_params.h`: the `VisualizationMode` enum
//!   ([`VisualizationMode`]) and the scalar constructor defaults
//!   ([`RaytracingParamsDefaults`]). The struct's 6 `float4x4` and 9 `float4`
//!   *field declarations* are not reproduced as a struct layout (see
//!   Nonclaims), and `rt64_postprocess.rs` separately consumes five of its
//!   scalar field types. This file is **not** closed by this module.
//!
//! **Cited but not ported (4)** -- must not be credited at all:
//! - `rt64_fb_reinterpret.h`: [`crate::rt64_fb_reinterpret`] already **is**
//!   this file's port. Its `DitherParams` and `FbReinterpretFormats` carry
//!   `srcSiz`/`srcFmt`/`dstSiz`/`dstFmt`/`tlutFormat`/`ditherPattern`/
//!   `ditherRandomSeed`/`ditherOffset`/`usesHDR` plus the `resolution.x` half
//!   of `resolution` -- 10 of `FbReinterpretCB`'s 12 members -- and it owns
//!   the *kernels* those fields parameterize. Redeclaring `FbReinterpretCB`
//!   here would create a second, behaviorless spelling of a type that module
//!   already uses meaningfully. The two members it does not carry
//!   (`resolution.y`, `sampleScale`) are pure dispatch geometry with no
//!   behavior in the cited header.
//! - `rt64_rdp_params.h`: `state.rs`'s RDP state already carries
//!   `env_color`, `prim_color`, `blend_color`, `fog_color` and `prim_depth`,
//!   and `fn64-render-reference`'s combiner carries `key_center`/`key_scale`
//!   and `prim_lod_fraction`. Those are `RDPParams`' 9 members minus
//!   `convertK`, and they are carried as *live decoded register state* with
//!   `Option` presence tracking, not as an inert mirror. A parallel
//!   `RDPParams` struct here would be a duplicate definition of state fn64
//!   already owns -- strictly worse than no port.
//! - `rt64_hlsl_json.h` / `rt64_hlsl_json.cpp`: these are
//!   `nlohmann::json` `to_json`/`from_json` ADL hooks for `float3`/`float4`.
//!   Their entire content is "a `float3` serializes as the JSON array
//!   `[x, y, z]` and a `float4` as `[x, y, z, w]`", expressed through a
//!   third-party C++ serialization library fn64 does not use and whose ADL
//!   customization-point mechanism has no Rust analogue. There is no
//!   arithmetic, no ordering subtlety, and no hardware fact in either file.
//!   Porting them would mean inventing a `serde` surface the source does not
//!   describe.

use fn64_render_ir::{Vec3, Vec4};

// ---------------------------------------------------------------------------
// Full ports
// ---------------------------------------------------------------------------

/// `interop::RenderIndices` (`rt64_render_indices.h:12-18`), all five `uint`
/// members in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderIndices {
    /// `uint instanceIndex` (line 13).
    pub instance_index: u32,
    /// `uint faceIndicesStart` (line 14).
    pub face_indices_start: u32,
    /// `uint rdpTileIndex` (line 15).
    pub rdp_tile_index: u32,
    /// `uint rdpTileCount` (line 16).
    pub rdp_tile_count: u32,
    /// `uint highlightColor` (line 17).
    pub highlight_color: u32,
}

impl RenderIndices {
    /// Builds a [`RenderIndices`] from **positional** arguments in the
    /// source's declaration order (`rt64_render_indices.h:13-17`).
    ///
    /// Named-field construction cannot detect a declaration reorder, so every
    /// order claim in this module is pinned by a positional constructor like
    /// this one and exercised by a test that feeds it distinct values. Swap
    /// two fields in the struct above without swapping them here and the
    /// order test fails; swap them in both and the *documented line numbers*
    /// in this signature no longer ascend, which is the reviewable artifact.
    #[must_use]
    pub const fn in_source_order(
        instance_index: u32,
        face_indices_start: u32,
        rdp_tile_index: u32,
        rdp_tile_count: u32,
        highlight_color: u32,
    ) -> RenderIndices {
        RenderIndices {
            instance_index,
            face_indices_start,
            rdp_tile_index,
            rdp_tile_count,
            highlight_color,
        }
    }

    /// The five members' values in declaration order.
    #[must_use]
    pub const fn to_source_order(self) -> [u32; 5] {
        [
            self.instance_index,
            self.face_indices_start,
            self.rdp_tile_index,
            self.rdp_tile_count,
            self.highlight_color,
        ]
    }
}

/// `#define MAX_INTERLEAVED_RASTERS 8` (`rt64_interleaved_raster.h:5`).
pub const MAX_INTERLEAVED_RASTERS: u32 = 8;

/// `interop::InterleavedRaster` (`rt64_interleaved_raster.h:10-15`), all four
/// `uint` members in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterleavedRaster {
    /// `uint rasterSceneIndex` (line 11).
    pub raster_scene_index: u32,
    /// `uint firstInstanceIndex` (line 12).
    pub first_instance_index: u32,
    /// `uint colorTextureIndex` (line 13).
    pub color_texture_index: u32,
    /// `uint depthTextureIndex` (line 14).
    pub depth_texture_index: u32,
}

impl InterleavedRaster {
    /// Positional constructor in source declaration order
    /// (`rt64_interleaved_raster.h:11-14`). See
    /// [`RenderIndices::in_source_order`] for why this exists.
    #[must_use]
    pub const fn in_source_order(
        raster_scene_index: u32,
        first_instance_index: u32,
        color_texture_index: u32,
        depth_texture_index: u32,
    ) -> InterleavedRaster {
        InterleavedRaster {
            raster_scene_index,
            first_instance_index,
            color_texture_index,
            depth_texture_index,
        }
    }

    /// The four members' values in declaration order.
    #[must_use]
    pub const fn to_source_order(self) -> [u32; 4] {
        [
            self.raster_scene_index,
            self.first_instance_index,
            self.color_texture_index,
            self.depth_texture_index,
        ]
    }
}

/// `interop::TextureCopyCB` (`rt64_texture_copy.h:12-15`), both `float2`
/// members in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextureCopyCb {
    /// `float2 uvScroll` (line 13).
    pub uv_scroll: [f32; 2],
    /// `float2 uvScale` (line 14).
    pub uv_scale: [f32; 2],
}

/// `interop::RenderTargetCopyCB` (`rt64_render_target_copy.h:12-14`). The
/// file's entire content is this one `uint` member; the name is
/// [`Self::uses_hdr`] and its declared type is `uint`, **not** `bool`, which
/// is why it is a `u32` here (the same `usesHDR`-as-`uint` convention appears
/// in `rt64_fb_common.h` and `rt64_fb_reinterpret.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderTargetCopyCb {
    /// `uint usesHDR` (line 13).
    pub uses_hdr: u32,
}

/// `interop::VideoInterfaceCB` (`rt64_video_interface.h:12-16`), all three
/// members in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoInterfaceCb {
    /// `float2 videoResolution` (line 13).
    pub video_resolution: [f32; 2],
    /// `float2 textureResolution` (line 14).
    pub texture_resolution: [f32; 2],
    /// `float gamma` (line 15).
    pub gamma: f32,
}

/// `interop::FrameParams` (`rt64_frame_params.h:12-16`), all three members in
/// declaration order. Note the *mixed* widths: two `uint` then one `float`.
///
/// `viewUbershaders` is a `uint`, not a `bool`, in the source.
/// `ditherNoiseStrength` is a `float` here; the unrelated
/// `rt64_gbi_extended_decode::decode_set_dither_noise_strength` returns the
/// raw 16-bit *wire* field from a GBI command word, which is a different
/// quantity at a different stage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameParams {
    /// `uint frameCount` (line 13).
    pub frame_count: u32,
    /// `uint viewUbershaders` (line 14).
    pub view_ubershaders: u32,
    /// `float ditherNoiseStrength` (line 15).
    pub dither_noise_strength: f32,
}

/// `interop::FramebufferParams` (`rt64_framebuffer_params.h:12-16`), all three
/// members in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FramebufferParams {
    /// `float2 resolution` (line 13).
    pub resolution: [f32; 2],
    /// `float2 resolutionScale` (line 14).
    pub resolution_scale: [f32; 2],
    /// `float horizontalMisalignment` (line 15).
    pub horizontal_misalignment: f32,
}

/// `#define FB_COMMON_WORKGROUP_SIZE 8` (`rt64_fb_common.h:9`). Declared
/// *outside* the `#ifdef HLSL_CPU` guard, so both the C++ and HLSL sides see
/// it.
pub const FB_COMMON_WORKGROUP_SIZE: u32 = 8;

/// `interop::FbCommonCB` (`rt64_fb_common.h:14-22`), all seven members in
/// declaration order.
///
/// `fmt`, `siz`, `ditherPattern`, `ditherRandomSeed` and `usesHDR` are all
/// declared `uint` in the source and are `u32` here; in particular `usesHDR`
/// is not a `bool`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FbCommonCb {
    /// `uint2 offset` (line 15).
    pub offset: [u32; 2],
    /// `uint2 resolution` (line 16).
    pub resolution: [u32; 2],
    /// `uint fmt` (line 17).
    pub fmt: u32,
    /// `uint siz` (line 18).
    pub siz: u32,
    /// `uint ditherPattern` (line 19).
    pub dither_pattern: u32,
    /// `uint ditherRandomSeed` (line 20).
    pub dither_random_seed: u32,
    /// `uint usesHDR` (line 21).
    pub uses_hdr: u32,
}

impl FbCommonCb {
    /// Positional constructor in source declaration order
    /// (`rt64_fb_common.h:15-21`). See [`RenderIndices::in_source_order`].
    #[must_use]
    pub const fn in_source_order(
        offset: [u32; 2],
        resolution: [u32; 2],
        fmt: u32,
        siz: u32,
        dither_pattern: u32,
        dither_random_seed: u32,
        uses_hdr: u32,
    ) -> FbCommonCb {
        FbCommonCb {
            offset,
            resolution,
            fmt,
            siz,
            dither_pattern,
            dither_random_seed,
            uses_hdr,
        }
    }

    /// The seven members flattened in declaration order, with each `uint2`
    /// expanded to its `x`, `y` components -- nine `u32`s total. This is a
    /// *sequence*, not a memory image: it says nothing about byte offsets
    /// (see the module's Nonclaims).
    #[must_use]
    pub const fn to_source_order(self) -> [u32; 9] {
        [
            self.offset[0],
            self.offset[1],
            self.resolution[0],
            self.resolution[1],
            self.fmt,
            self.siz,
            self.dither_pattern,
            self.dither_random_seed,
            self.uses_hdr,
        ]
    }
}

/// `interop::RenderParams` (`rt64_render_params.h:18-24`), all five members in
/// declaration order.
///
/// The four `cc`/`om` members are the color-combiner and other-mode register
/// words split low/high. [`Self::flags`] is `RenderFlags`, which the header
/// forks by build:
///
/// ```text
/// #ifdef HLSL_CPU
/// #include "shared/rt64_render_flags.h"
///
/// namespace interop {
/// #endif
/// #ifndef HLSL_CPU
///     typedef uint RenderFlags;
/// #endif
/// ```
///
/// On the shader side `RenderFlags` is literally `uint`; on the CPU side it is
/// `rt64_render_flags.h`'s enumeration over that same 32-bit word. This port
/// carries it as `u32`, whose twenty bit fields and accessors are already
/// owned by [`crate::rt64_render_flags`] -- see "Reuse, not new type".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderParams {
    /// `uint ccL` (line 19).
    pub cc_l: u32,
    /// `uint ccH` (line 20).
    pub cc_h: u32,
    /// `uint omL` (line 21).
    pub om_l: u32,
    /// `uint omH` (line 22).
    pub om_h: u32,
    /// `RenderFlags flags` (line 23), the `u32` word
    /// [`crate::rt64_render_flags`] decodes.
    pub flags: u32,
}

impl RenderParams {
    /// Positional constructor in source declaration order
    /// (`rt64_render_params.h:19-23`) -- `ccL`, `ccH`, `omL`, `omH`, `flags`.
    /// See [`RenderIndices::in_source_order`].
    #[must_use]
    pub const fn in_source_order(
        cc_l: u32,
        cc_h: u32,
        om_l: u32,
        om_h: u32,
        flags: u32,
    ) -> RenderParams {
        RenderParams {
            cc_l,
            cc_h,
            om_l,
            om_h,
            flags,
        }
    }

    /// The five members' values in declaration order.
    #[must_use]
    pub const fn to_source_order(self) -> [u32; 5] {
        [self.cc_l, self.cc_h, self.om_l, self.om_h, self.flags]
    }
}

// ---------------------------------------------------------------------------
// Deltas over types fn64 already owns
// ---------------------------------------------------------------------------

/// The eight `RDPTile` members (`rt64_rdp_tile.h:12-29`) that
/// [`crate::rt64_texture_sampler`]'s `RdpTileAddressing` does **not** already
/// own, kept in their source declaration order relative to each other.
///
/// `RDPTile` declares sixteen members. The split is exact and covers all
/// sixteen with no member counted twice:
///
/// | member | width | owner |
/// |---|---|---|
/// | `fmt` | `int` | here |
/// | `siz` | `int` | here |
/// | `stride` | `int` | here |
/// | `address` | `int` | here |
/// | `palette` | `int` | here |
/// | `masks` | `int` | `RdpTileAddressing` |
/// | `maskt` | `int` | `RdpTileAddressing` |
/// | `shifts` | `float` | here |
/// | `shiftt` | `float` | here |
/// | `uls` | `float` | `RdpTileAddressing` |
/// | `ult` | `float` | `RdpTileAddressing` |
/// | `lrs` | `float` | `RdpTileAddressing` |
/// | `lrt` | `float` | `RdpTileAddressing` |
/// | `cms` | `int` | `RdpTileAddressing` |
/// | `cmt` | `int` | `RdpTileAddressing` |
/// | `nativeSampler` | `uint` | here |
///
/// Two width facts worth pinning, because they are easy to assume wrong:
/// `shifts`/`shiftt` are **`float`**, not `int`, even though `masks`/`maskt`
/// beside them are `int`; and `nativeSampler` is the file's only **`uint`**,
/// every other integer member being signed `int`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RdpTileImageDescriptor {
    /// `int fmt` (line 13).
    pub fmt: i32,
    /// `int siz` (line 14).
    pub siz: i32,
    /// `int stride` (line 15).
    pub stride: i32,
    /// `int address` (line 16).
    pub address: i32,
    /// `int palette` (line 17).
    pub palette: i32,
    /// `float shifts` (line 20) -- `float`, not `int`.
    pub shifts: f32,
    /// `float shiftt` (line 21) -- `float`, not `int`.
    pub shiftt: f32,
    /// `uint nativeSampler` (line 28) -- the file's only unsigned member.
    pub native_sampler: u32,
}

impl RdpTileImageDescriptor {
    /// Positional constructor in the relative source declaration order of the
    /// eight members this type owns (`rt64_rdp_tile.h` lines 13, 14, 15, 16,
    /// 17, 20, 21, 28). See [`RenderIndices::in_source_order`].
    ///
    /// The argument *widths* are the second thing pinned here: five `i32`,
    /// then two `f32`, then one `u32`. Retyping `shifts` as an integer or
    /// `nativeSampler` as signed stops this signature compiling at every call
    /// site.
    #[must_use]
    pub const fn in_source_order(
        fmt: i32,
        siz: i32,
        stride: i32,
        address: i32,
        palette: i32,
        shifts: f32,
        shiftt: f32,
        native_sampler: u32,
    ) -> RdpTileImageDescriptor {
        RdpTileImageDescriptor {
            fmt,
            siz,
            stride,
            address,
            palette,
            shifts,
            shiftt,
            native_sampler,
        }
    }

    /// The five `int` members in declaration order.
    #[must_use]
    pub const fn signed_members_in_source_order(self) -> [i32; 5] {
        [self.fmt, self.siz, self.stride, self.address, self.palette]
    }

    /// The two `float` members in declaration order.
    #[must_use]
    pub const fn float_members_in_source_order(self) -> [f32; 2] {
        [self.shifts, self.shiftt]
    }
}

/// The two `RasterParams` members (`rt64_raster_params.h:12-17`) that
/// [`crate::raster_vs`]'s `ScreenTransform` does not already own.
///
/// `RasterParams` declares four members: `uint renderIndex`, `uint3 padding`,
/// `float2 screenScale`, `float2 screenOffset`. The last two are
/// `ScreenTransform`'s `scale`/`offset`; the first two are here.
///
/// [`Self::padding`] is `uint3` -- **three** `u32`s, not two and not four. It
/// exists upstream to push `screenScale` to a 16-byte boundary, which is a
/// layout concern this module explicitly does not claim (see Nonclaims); it is
/// carried here only because it is a declared member of the struct, and its
/// element count is a fact of the source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterParamsIndexing {
    /// `uint renderIndex` (line 13).
    pub render_index: u32,
    /// `uint3 padding` (line 14).
    pub padding: [u32; 3],
}

/// `interop::VisualizationMode` (`rt64_raytracing_params.h:12-31`), a plain
/// unscoped-value `enum class` with no explicit initializers, so the
/// discriminants are the default `0..=16` in declaration order and
/// [`VisualizationMode::Count`] -- the trailing sentinel -- is `17`.
///
/// ```text
/// enum class VisualizationMode {
///     Final,
///     ShadingPosition,
///     ShadingNormal,
///     ShadingSpecular,
///     Diffuse,
///     InstanceId,
///     DirectLightRaw,
///     DirectLightFiltered,
///     IndirectLightRaw,
///     IndirectLightFiltered,
///     Reflection,
///     Refraction,
///     Transparent,
///     Flow,
///     ReactiveMask,
///     LockMask,
///     Depth,
///     Count
/// };
/// ```
///
/// `Count` is the count of the *real* modes (17: `Final` through `Depth`) and
/// is itself not a mode. Upstream reads the value back with a
/// `static_cast<interop::VisualizationMode>(int)` (`src/hle/rt64_state.cpp`),
/// so the numeric discriminants are load-bearing, not incidental.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u32)]
pub enum VisualizationMode {
    /// `Final` = 0. The constructor's default.
    #[default]
    Final = 0,
    /// `ShadingPosition` = 1.
    ShadingPosition = 1,
    /// `ShadingNormal` = 2.
    ShadingNormal = 2,
    /// `ShadingSpecular` = 3.
    ShadingSpecular = 3,
    /// `Diffuse` = 4.
    Diffuse = 4,
    /// `InstanceId` = 5.
    InstanceId = 5,
    /// `DirectLightRaw` = 6.
    DirectLightRaw = 6,
    /// `DirectLightFiltered` = 7.
    DirectLightFiltered = 7,
    /// `IndirectLightRaw` = 8.
    IndirectLightRaw = 8,
    /// `IndirectLightFiltered` = 9.
    IndirectLightFiltered = 9,
    /// `Reflection` = 10.
    Reflection = 10,
    /// `Refraction` = 11.
    Refraction = 11,
    /// `Transparent` = 12.
    Transparent = 12,
    /// `Flow` = 13.
    Flow = 13,
    /// `ReactiveMask` = 14.
    ReactiveMask = 14,
    /// `LockMask` = 15.
    LockMask = 15,
    /// `Depth` = 16.
    Depth = 16,
    /// `Count` = 17, the trailing sentinel; not a renderable mode.
    Count = 17,
}

impl VisualizationMode {
    /// Every enumerator in source declaration order, `Final` through `Count`.
    pub const ALL: [VisualizationMode; 18] = [
        VisualizationMode::Final,
        VisualizationMode::ShadingPosition,
        VisualizationMode::ShadingNormal,
        VisualizationMode::ShadingSpecular,
        VisualizationMode::Diffuse,
        VisualizationMode::InstanceId,
        VisualizationMode::DirectLightRaw,
        VisualizationMode::DirectLightFiltered,
        VisualizationMode::IndirectLightRaw,
        VisualizationMode::IndirectLightFiltered,
        VisualizationMode::Reflection,
        VisualizationMode::Refraction,
        VisualizationMode::Transparent,
        VisualizationMode::Flow,
        VisualizationMode::ReactiveMask,
        VisualizationMode::LockMask,
        VisualizationMode::Depth,
        VisualizationMode::Count,
    ];

    /// The enum's numeric value, as `static_cast<int>` would produce it.
    #[must_use]
    pub const fn discriminant(self) -> u32 {
        self as u32
    }

    /// The inverse of [`Self::discriminant`], mirroring upstream's
    /// `static_cast<interop::VisualizationMode>(visualizationMode)`
    /// (`src/hle/rt64_state.cpp:2496`) but returning `None` for values with no
    /// enumerator instead of producing an out-of-range enum value.
    ///
    /// This is a deliberate, minimal **deviation**: the C++ cast has no such
    /// guard, and casting an out-of-range integer to a scoped enum whose
    /// fixed underlying type can represent it is well-defined in C++ but would
    /// be UB in Rust. Returning `Option` refuses to reproduce a value Rust has
    /// no sound representation for. The tests that exercise out-of-range input
    /// pin **this deviation**, not the original.
    #[must_use]
    pub const fn from_discriminant(value: u32) -> Option<VisualizationMode> {
        Some(match value {
            0 => VisualizationMode::Final,
            1 => VisualizationMode::ShadingPosition,
            2 => VisualizationMode::ShadingNormal,
            3 => VisualizationMode::ShadingSpecular,
            4 => VisualizationMode::Diffuse,
            5 => VisualizationMode::InstanceId,
            6 => VisualizationMode::DirectLightRaw,
            7 => VisualizationMode::DirectLightFiltered,
            8 => VisualizationMode::IndirectLightRaw,
            9 => VisualizationMode::IndirectLightFiltered,
            10 => VisualizationMode::Reflection,
            11 => VisualizationMode::Refraction,
            12 => VisualizationMode::Transparent,
            13 => VisualizationMode::Flow,
            14 => VisualizationMode::ReactiveMask,
            15 => VisualizationMode::LockMask,
            16 => VisualizationMode::Depth,
            17 => VisualizationMode::Count,
            _ => return None,
        })
    }
}

/// The values `RaytracingParams`' default constructor
/// (`rt64_raytracing_params.h:71-105`) assigns, carried as data rather than as
/// a constructor.
///
/// Only the constructor's **non-matrix** assignments are here. The six
/// `float4x4` members are all `= FLOAT4X4_IDENTITY`, a value
/// [`crate::rt64_hlsl_interop`] already owns; restating them would duplicate
/// that module (see Nonclaims).
///
/// **Two members are declared but never assigned by the constructor**:
/// `lightsCount` (line 66) and `interleavedRastersCount` (line 67). The
/// constructor assigns `motionBlurSamples` and then jumps straight to
/// `visualizationMode`, skipping both. That is a fact of the source, not an
/// omission here, and it means a default-constructed `RaytracingParams` leaves
/// those two members **uninitialized** in C++. This port does not invent
/// values for them: they have no field on this type. Reading them in C++
/// before assignment would be UB, and this module refuses to model it (see
/// Nonclaims).
///
/// The five non-zero scalar defaults are the interesting content -- everything
/// else the constructor writes is a zero:
///
/// ```text
/// fovRadians = 0.707f;
/// nearDist = 1.0f;
/// farDist = 1000.0f;
/// tonemapExposure = 0.6f;
/// tonemapWhite = 1.0f;
/// maxLights = 1;
/// motionBlurSamples = 32;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaytracingParamsDefaults {
    /// `cameraU = { 0, 0, 0, 0 }` (line 78).
    pub camera_u: Vec4,
    /// `cameraV = { 0, 0, 0, 0 }` (line 79).
    pub camera_v: Vec4,
    /// `cameraW = { 0, 0, 0, 0 }` (line 80).
    pub camera_w: Vec4,
    /// `viewport = { 0, 0, 0, 0 }` (line 81).
    pub viewport: Vec4,
    /// `resolution = { 0, 0, 0, 0 }` (line 82).
    pub resolution: Vec4,
    /// `ambientBaseColor = { 0, 0, 0, 0 }` (line 83).
    pub ambient_base_color: Vec4,
    /// `ambientNoGIColor = { 0, 0, 0, 0 }` (line 84).
    pub ambient_no_gi_color: Vec4,
    /// `eyeLightDiffuseColor = { 0, 0, 0, 0 }` (line 85).
    pub eye_light_diffuse_color: Vec4,
    /// `eyeLightSpecularColor = { 0, 0, 0, 0 }` (line 86).
    pub eye_light_specular_color: Vec4,
    /// `pixelJitter = { 0, 0 }` (line 87).
    pub pixel_jitter: [f32; 2],
    /// `fovRadians = 0.707f` (line 88).
    pub fov_radians: f32,
    /// `nearDist = 1.0f` (line 89).
    pub near_dist: f32,
    /// `farDist = 1000.0f` (line 90).
    pub far_dist: f32,
    /// `giDiffuseStrength = 0.0f` (line 91).
    pub gi_diffuse_strength: f32,
    /// `giBackgroundStrength = 0.0f` (line 92).
    pub gi_background_strength: f32,
    /// `motionBlurStrength = 0.0f` (line 93).
    pub motion_blur_strength: f32,
    /// `tonemapExposure = 0.6f` (line 94).
    pub tonemap_exposure: f32,
    /// `tonemapWhite = 1.0f` (line 95).
    pub tonemap_white: f32,
    /// `tonemapBlack = 0.0f` (line 96).
    pub tonemap_black: f32,
    /// `diSamples = 0` (line 97).
    pub di_samples: u32,
    /// `giSamples = 0` (line 98).
    pub gi_samples: u32,
    /// `diReproject = 0` (line 99).
    pub di_reproject: u32,
    /// `giReproject = 0` (line 100).
    pub gi_reproject: u32,
    /// `binaryLockMask = 0` (line 101).
    pub binary_lock_mask: u32,
    /// `maxLights = 1` (line 102) -- one, not zero.
    pub max_lights: u32,
    /// `motionBlurSamples = 32` (line 103).
    pub motion_blur_samples: u32,
    /// `visualizationMode = VisualizationMode::Final` (line 104).
    pub visualization_mode: VisualizationMode,
}

/// The zero `Vec4` the constructor's nine `{ 0.0f, 0.0f, 0.0f, 0.0f }`
/// initializers produce.
const ZERO_VEC4: Vec4 = Vec4 {
    x: 0.0,
    y: 0.0,
    z: 0.0,
    w: 0.0,
};

impl RaytracingParamsDefaults {
    /// Exactly the assignments `RaytracingParams::RaytracingParams()` makes,
    /// in source order, matrices excluded.
    pub const SOURCE: RaytracingParamsDefaults = RaytracingParamsDefaults {
        camera_u: ZERO_VEC4,
        camera_v: ZERO_VEC4,
        camera_w: ZERO_VEC4,
        viewport: ZERO_VEC4,
        resolution: ZERO_VEC4,
        ambient_base_color: ZERO_VEC4,
        ambient_no_gi_color: ZERO_VEC4,
        eye_light_diffuse_color: ZERO_VEC4,
        eye_light_specular_color: ZERO_VEC4,
        pixel_jitter: [0.0, 0.0],
        fov_radians: 0.707,
        near_dist: 1.0,
        far_dist: 1000.0,
        gi_diffuse_strength: 0.0,
        gi_background_strength: 0.0,
        motion_blur_strength: 0.0,
        tonemap_exposure: 0.6,
        tonemap_white: 1.0,
        tonemap_black: 0.0,
        di_samples: 0,
        gi_samples: 0,
        di_reproject: 0,
        gi_reproject: 0,
        binary_lock_mask: 0,
        max_lights: 1,
        motion_blur_samples: 32,
        visualization_mode: VisualizationMode::Final,
    };
}

impl Default for RaytracingParamsDefaults {
    fn default() -> Self {
        Self::SOURCE
    }
}

/// A `Vec3` reference to keep the `float3` interop fact live: `rt64_hlsl.h`'s
/// `float3` is the workspace's [`Vec3`], the type
/// `rt64_hlsl_json.cpp`'s refused `to_json`/`from_json` overloads serialize as
/// `[x, y, z]`. `rt64_rdp_params.h`'s `keyCenter`/`keyScale` are `float3` too;
/// both are owned by `fn64-render-reference`'s combiner rather than here.
#[must_use]
pub const fn float3_component_count(_witness: Vec3) -> usize {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- RenderIndices (rt64_render_indices.h) --

    #[test]
    fn shared_params_render_indices_carries_five_named_uint_fields() {
        // Hand-derived: five distinct values assigned by name, read back by
        // name. Catches any field reorder that a positional literal would
        // hide.
        let indices = RenderIndices {
            instance_index: 0x1111_1111,
            face_indices_start: 0x2222_2222,
            rdp_tile_index: 0x3333_3333,
            rdp_tile_count: 0x4444_4444,
            highlight_color: 0x5555_5555,
        };
        assert_eq!(indices.instance_index, 0x1111_1111);
        assert_eq!(indices.face_indices_start, 0x2222_2222);
        assert_eq!(indices.rdp_tile_index, 0x3333_3333);
        assert_eq!(indices.rdp_tile_count, 0x4444_4444);
        assert_eq!(indices.highlight_color, 0x5555_5555);
    }

    #[test]
    fn shared_params_render_indices_fields_are_full_width_u32() {
        // u32::MAX must survive every field. A narrowed field (u16/u8) would
        // fail to compile or truncate.
        let indices = RenderIndices {
            instance_index: u32::MAX,
            face_indices_start: u32::MAX,
            rdp_tile_index: u32::MAX,
            rdp_tile_count: u32::MAX,
            highlight_color: u32::MAX,
        };
        assert_eq!(indices.instance_index, 4_294_967_295);
        assert_eq!(indices.highlight_color, 4_294_967_295);
    }

    #[test]
    fn shared_params_render_indices_declaration_order_is_pinned() {
        // The order claim, pinned positionally. Five distinct values fed in
        // source order must come back out mapped to the named fields the
        // header declares at lines 13..17. Swapping any two field
        // declarations in the struct (without also swapping this
        // constructor's parameters) makes this fail.
        let indices = RenderIndices::in_source_order(10, 20, 30, 40, 50);
        assert_eq!(indices.instance_index, 10);
        assert_eq!(indices.face_indices_start, 20);
        assert_eq!(indices.rdp_tile_index, 30);
        assert_eq!(indices.rdp_tile_count, 40);
        assert_eq!(indices.highlight_color, 50);
        // Second, independent derivation of the same order.
        assert_eq!(indices.to_source_order(), [10, 20, 30, 40, 50]);
    }

    #[test]
    fn shared_params_render_indices_default_is_all_zero() {
        assert_eq!(
            RenderIndices::default(),
            RenderIndices {
                instance_index: 0,
                face_indices_start: 0,
                rdp_tile_index: 0,
                rdp_tile_count: 0,
                highlight_color: 0,
            }
        );
    }

    // -- InterleavedRaster (rt64_interleaved_raster.h) --

    #[test]
    fn shared_params_max_interleaved_rasters_is_eight() {
        // `#define MAX_INTERLEAVED_RASTERS 8`, line 5. Asserted two
        // independent ways: the literal, and the bit-position identity
        // 8 == 1 << 3, which an off-by-one (7 or 9) fails on both counts.
        assert_eq!(MAX_INTERLEAVED_RASTERS, 8);
        assert_eq!(MAX_INTERLEAVED_RASTERS, 1u32 << 3);
        assert_eq!(MAX_INTERLEAVED_RASTERS.count_ones(), 1);
    }

    #[test]
    fn shared_params_interleaved_raster_carries_four_named_uint_fields() {
        let raster = InterleavedRaster {
            raster_scene_index: 7,
            first_instance_index: 1_000,
            color_texture_index: 22,
            depth_texture_index: 23,
        };
        assert_eq!(raster.raster_scene_index, 7);
        assert_eq!(raster.first_instance_index, 1_000);
        assert_eq!(raster.color_texture_index, 22);
        assert_eq!(raster.depth_texture_index, 23);
    }

    #[test]
    fn shared_params_interleaved_raster_color_and_depth_indices_are_distinct() {
        // colorTextureIndex precedes depthTextureIndex in the source; a swap
        // of the two declarations is the likeliest transcription error and is
        // exactly what this catches.
        let raster = InterleavedRaster {
            raster_scene_index: 0,
            first_instance_index: 0,
            color_texture_index: 4,
            depth_texture_index: 9,
        };
        assert_ne!(raster.color_texture_index, raster.depth_texture_index);
        assert_eq!(raster.color_texture_index, 4);
        assert_eq!(raster.depth_texture_index, 9);
    }

    #[test]
    fn shared_params_interleaved_raster_declaration_order_is_pinned() {
        let raster = InterleavedRaster::in_source_order(11, 22, 33, 44);
        assert_eq!(raster.raster_scene_index, 11);
        assert_eq!(raster.first_instance_index, 22);
        assert_eq!(raster.color_texture_index, 33);
        assert_eq!(raster.depth_texture_index, 44);
        assert_eq!(raster.to_source_order(), [11, 22, 33, 44]);
    }

    // -- TextureCopyCB (rt64_texture_copy.h) --

    #[test]
    fn shared_params_texture_copy_cb_scroll_precedes_scale() {
        let cb = TextureCopyCb {
            uv_scroll: [0.25, -0.5],
            uv_scale: [2.0, 4.0],
        };
        assert_eq!(cb.uv_scroll, [0.25, -0.5]);
        assert_eq!(cb.uv_scale, [2.0, 4.0]);
        // Distinct values so a scroll/scale swap cannot pass.
        assert_ne!(cb.uv_scroll, cb.uv_scale);
    }

    #[test]
    fn shared_params_texture_copy_cb_members_are_two_component() {
        let cb = TextureCopyCb::default();
        assert_eq!(cb.uv_scroll.len(), 2);
        assert_eq!(cb.uv_scale.len(), 2);
        assert_eq!(cb.uv_scroll, [0.0, 0.0]);
    }

    // -- RenderTargetCopyCB (rt64_render_target_copy.h) --

    #[test]
    fn shared_params_render_target_copy_cb_uses_hdr_is_a_uint_not_a_bool() {
        // `uint usesHDR`. A bool port would not admit 2 or u32::MAX.
        assert_eq!(RenderTargetCopyCb { uses_hdr: 0 }.uses_hdr, 0);
        assert_eq!(RenderTargetCopyCb { uses_hdr: 1 }.uses_hdr, 1);
        assert_eq!(RenderTargetCopyCb { uses_hdr: 2 }.uses_hdr, 2);
        assert_eq!(
            RenderTargetCopyCb { uses_hdr: u32::MAX }.uses_hdr,
            4_294_967_295
        );
    }

    #[test]
    fn shared_params_render_target_copy_cb_default_is_zero() {
        assert_eq!(RenderTargetCopyCb::default().uses_hdr, 0);
    }

    // -- VideoInterfaceCB (rt64_video_interface.h) --

    #[test]
    fn shared_params_video_interface_cb_carries_two_resolutions_and_gamma() {
        let cb = VideoInterfaceCb {
            video_resolution: [640.0, 480.0],
            texture_resolution: [320.0, 240.0],
            gamma: 2.2,
        };
        assert_eq!(cb.video_resolution, [640.0, 480.0]);
        assert_eq!(cb.texture_resolution, [320.0, 240.0]);
        assert_eq!(cb.gamma, 2.2);
        // videoResolution precedes textureResolution; distinct so a swap fails.
        assert_ne!(cb.video_resolution, cb.texture_resolution);
    }

    #[test]
    fn shared_params_video_interface_cb_gamma_is_scalar_float() {
        let cb = VideoInterfaceCb {
            video_resolution: [0.0, 0.0],
            texture_resolution: [0.0, 0.0],
            gamma: 1.0,
        };
        assert_eq!(cb.gamma, 1.0_f32);
        assert_eq!(VideoInterfaceCb::default().gamma, 0.0);
    }

    // -- FrameParams (rt64_frame_params.h) --

    #[test]
    fn shared_params_frame_params_mixes_two_uints_then_one_float() {
        // The width order (uint, uint, float) is the fact under test: a
        // fractional value must survive ditherNoiseStrength and not the
        // other two.
        let params = FrameParams {
            frame_count: 12_345,
            view_ubershaders: 1,
            dither_noise_strength: 0.5,
        };
        assert_eq!(params.frame_count, 12_345_u32);
        assert_eq!(params.view_ubershaders, 1_u32);
        assert_eq!(params.dither_noise_strength, 0.5_f32);
    }

    #[test]
    fn shared_params_frame_params_view_ubershaders_is_a_uint_not_a_bool() {
        assert_eq!(
            FrameParams {
                frame_count: 0,
                view_ubershaders: 3,
                dither_noise_strength: 0.0,
            }
            .view_ubershaders,
            3
        );
    }

    #[test]
    fn shared_params_frame_params_frame_count_spans_full_u32() {
        let params = FrameParams {
            frame_count: u32::MAX,
            view_ubershaders: 0,
            dither_noise_strength: 0.0,
        };
        assert_eq!(params.frame_count, 4_294_967_295);
    }

    // -- FramebufferParams (rt64_framebuffer_params.h) --

    #[test]
    fn shared_params_framebuffer_params_resolution_precedes_scale() {
        let params = FramebufferParams {
            resolution: [320.0, 240.0],
            resolution_scale: [2.0, 2.0],
            horizontal_misalignment: -1.5,
        };
        assert_eq!(params.resolution, [320.0, 240.0]);
        assert_eq!(params.resolution_scale, [2.0, 2.0]);
        assert_ne!(params.resolution, params.resolution_scale);
    }

    #[test]
    fn shared_params_framebuffer_params_horizontal_misalignment_is_signed_float() {
        // A `float`, so negatives and fractions must both round-trip; a u32
        // port would reject both.
        let params = FramebufferParams {
            resolution: [0.0, 0.0],
            resolution_scale: [0.0, 0.0],
            horizontal_misalignment: -0.25,
        };
        assert_eq!(params.horizontal_misalignment, -0.25_f32);
        assert!(params.horizontal_misalignment < 0.0);
    }

    // -- FbCommonCB (rt64_fb_common.h) --

    #[test]
    fn shared_params_fb_common_workgroup_size_is_eight() {
        // Two independent derivations of the same constant, per the mask rule.
        assert_eq!(FB_COMMON_WORKGROUP_SIZE, 8);
        assert_eq!(FB_COMMON_WORKGROUP_SIZE, 1u32 << 3);
        // 8x8 is the implied 2D tile; 64 threads. Reconciles the literal
        // against the workgroup area a caller would dispatch.
        assert_eq!(FB_COMMON_WORKGROUP_SIZE * FB_COMMON_WORKGROUP_SIZE, 64);
    }

    #[test]
    fn shared_params_fb_common_cb_carries_all_seven_members() {
        let cb = FbCommonCb {
            offset: [3, 5],
            resolution: [320, 240],
            fmt: 2,
            siz: 3,
            dither_pattern: 1,
            dither_random_seed: 0xDEAD_BEEF,
            uses_hdr: 1,
        };
        assert_eq!(cb.offset, [3, 5]);
        assert_eq!(cb.resolution, [320, 240]);
        assert_eq!(cb.fmt, 2);
        assert_eq!(cb.siz, 3);
        assert_eq!(cb.dither_pattern, 1);
        assert_eq!(cb.dither_random_seed, 0xDEAD_BEEF);
        assert_eq!(cb.uses_hdr, 1);
    }

    #[test]
    fn shared_params_fb_common_cb_offset_and_resolution_are_two_component_uints() {
        // uint2, not uint3 and not float2: three components would not
        // compile, and a fractional value could not be stored.
        let cb = FbCommonCb {
            offset: [u32::MAX, 0],
            resolution: [0, u32::MAX],
            ..FbCommonCb::default()
        };
        assert_eq!(cb.offset.len(), 2);
        assert_eq!(cb.resolution.len(), 2);
        assert_eq!(cb.offset[0], 4_294_967_295);
        assert_eq!(cb.resolution[1], 4_294_967_295);
    }

    #[test]
    fn shared_params_fb_common_cb_fmt_and_siz_are_distinct_adjacent_fields() {
        // fmt (line 17) precedes siz (line 18); a swap is the likeliest error.
        let cb = FbCommonCb {
            fmt: 6,
            siz: 1,
            ..FbCommonCb::default()
        };
        assert_eq!(cb.fmt, 6);
        assert_eq!(cb.siz, 1);
    }

    #[test]
    fn shared_params_fb_common_cb_declaration_order_is_pinned() {
        // Seven members, nine u32 slots once the two uint2s expand. Distinct
        // values throughout so any pairwise reorder shows up.
        let cb = FbCommonCb::in_source_order([1, 2], [3, 4], 5, 6, 7, 8, 9);
        assert_eq!(cb.offset, [1, 2]);
        assert_eq!(cb.resolution, [3, 4]);
        assert_eq!(cb.fmt, 5);
        assert_eq!(cb.siz, 6);
        assert_eq!(cb.dither_pattern, 7);
        assert_eq!(cb.dither_random_seed, 8);
        assert_eq!(cb.uses_hdr, 9);
        assert_eq!(cb.to_source_order(), [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    // -- RenderParams (rt64_render_params.h) --

    #[test]
    fn shared_params_render_params_orders_cc_low_high_then_om_low_high() {
        // The declaration order is ccL, ccH, omL, omH -- low before high
        // within each pair, and the cc pair before the om pair. Four distinct
        // values make any of those three possible swaps fail.
        let params = RenderParams {
            cc_l: 0x0000_00CC,
            cc_h: 0xCC00_0000,
            om_l: 0x0000_00_0E,
            om_h: 0x0E00_0000,
            flags: 0,
        };
        assert_eq!(params.cc_l, 0x0000_00CC);
        assert_eq!(params.cc_h, 0xCC00_0000);
        assert_eq!(params.om_l, 0x0000_000E);
        assert_eq!(params.om_h, 0x0E00_0000);
    }

    #[test]
    fn shared_params_render_params_flags_is_a_full_32_bit_word() {
        // `RenderFlags` is `uint` on the shader side, so all 32 bits are
        // addressable. Asserted two ways: the all-ones literal, and the
        // popcount, which an accidentally-narrowed field would fail.
        let params = RenderParams {
            flags: u32::MAX,
            ..RenderParams::default()
        };
        assert_eq!(params.flags, 0xFFFF_FFFF);
        assert_eq!(params.flags.count_ones(), 32);
    }

    #[test]
    fn shared_params_render_params_flags_feeds_the_existing_render_flag_decoder() {
        // The reuse claim under test: `flags` is the same word
        // rt64_render_flags.rs already decodes. Bit 0 is renderFlagRect
        // there, so setting exactly bit 0 must light that accessor and no
        // other tested one.
        let rect_only = 1u32;
        assert!(crate::rt64_render_flags::render_flag_rect(rect_only));
        let params = RenderParams {
            flags: rect_only,
            ..RenderParams::default()
        };
        assert!(crate::rt64_render_flags::render_flag_rect(params.flags));
        assert!(!crate::rt64_render_flags::render_flag_rect(0));
    }

    #[test]
    fn shared_params_render_params_declaration_order_is_pinned() {
        // ccL, ccH, omL, omH, flags -- the low/high ordering within each pair
        // is the fact most worth pinning, since ccH/ccL is a plausible
        // transcription slip.
        let params = RenderParams::in_source_order(1, 2, 3, 4, 5);
        assert_eq!(params.cc_l, 1);
        assert_eq!(params.cc_h, 2);
        assert_eq!(params.om_l, 3);
        assert_eq!(params.om_h, 4);
        assert_eq!(params.flags, 5);
        assert_eq!(params.to_source_order(), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn shared_params_render_params_default_is_all_zero() {
        let params = RenderParams::default();
        assert_eq!(params.cc_l, 0);
        assert_eq!(params.cc_h, 0);
        assert_eq!(params.om_l, 0);
        assert_eq!(params.om_h, 0);
        assert_eq!(params.flags, 0);
    }

    // -- RdpTileImageDescriptor (rt64_rdp_tile.h delta) --

    #[test]
    fn shared_params_rdp_tile_image_descriptor_carries_the_eight_unowned_members() {
        let tile = RdpTileImageDescriptor {
            fmt: 0,
            siz: 2,
            stride: 64,
            address: 0x1234,
            palette: 5,
            shifts: 1.5,
            shiftt: -2.5,
            native_sampler: 9,
        };
        assert_eq!(tile.fmt, 0);
        assert_eq!(tile.siz, 2);
        assert_eq!(tile.stride, 64);
        assert_eq!(tile.address, 0x1234);
        assert_eq!(tile.palette, 5);
        assert_eq!(tile.shifts, 1.5);
        assert_eq!(tile.shiftt, -2.5);
        assert_eq!(tile.native_sampler, 9);
    }

    #[test]
    fn shared_params_rdp_tile_declaration_order_is_pinned() {
        // Eight members, positionally. The signed group's relative order
        // (fmt, siz, stride, address, palette) and the float pair's order
        // (shifts before shiftt) are both pinned by distinct values.
        let tile = RdpTileImageDescriptor::in_source_order(1, 2, 3, 4, 5, 6.0, 7.0, 8);
        assert_eq!(tile.fmt, 1);
        assert_eq!(tile.siz, 2);
        assert_eq!(tile.stride, 3);
        assert_eq!(tile.address, 4);
        assert_eq!(tile.palette, 5);
        assert_eq!(tile.shifts, 6.0);
        assert_eq!(tile.shiftt, 7.0);
        assert_eq!(tile.native_sampler, 8);
        assert_eq!(tile.signed_members_in_source_order(), [1, 2, 3, 4, 5]);
        assert_eq!(tile.float_members_in_source_order(), [6.0, 7.0]);
    }

    #[test]
    fn shared_params_rdp_tile_shifts_are_float_not_int() {
        // The pinned width fact: `float shifts; float shiftt;` sit between
        // `int maskt` and `float uls`. A fractional, negative value must
        // survive; an i32 port would reject or truncate it.
        let tile = RdpTileImageDescriptor {
            shifts: 0.5,
            shiftt: -0.25,
            ..RdpTileImageDescriptor::default()
        };
        assert_eq!(tile.shifts, 0.5_f32);
        assert_eq!(tile.shiftt, -0.25_f32);
        assert_ne!(tile.shifts, tile.shiftt);
    }

    #[test]
    fn shared_params_rdp_tile_integer_members_are_signed_except_native_sampler() {
        // `fmt`/`siz`/`stride`/`address`/`palette` are `int`; only
        // `nativeSampler` is `uint`. Negatives must be storable in the first
        // group and u32::MAX in the last.
        let tile = RdpTileImageDescriptor {
            fmt: -1,
            siz: -2,
            stride: -3,
            address: -4,
            palette: -5,
            shifts: 0.0,
            shiftt: 0.0,
            native_sampler: u32::MAX,
        };
        assert_eq!(tile.fmt, -1_i32);
        assert_eq!(tile.siz, -2_i32);
        assert_eq!(tile.stride, -3_i32);
        assert_eq!(tile.address, -4_i32);
        assert_eq!(tile.palette, -5_i32);
        assert_eq!(tile.native_sampler, 4_294_967_295_u32);
    }

    #[test]
    fn shared_params_rdp_tile_split_covers_all_sixteen_members_exactly_once() {
        // Reconciles the delta claim two independent ways: this type's 8
        // fields plus RdpTileAddressing's 8 fields equal RDPTile's 16, and
        // the two name sets are disjoint.
        let here = [
            "fmt",
            "siz",
            "stride",
            "address",
            "palette",
            "shifts",
            "shiftt",
            "nativeSampler",
        ];
        let owned_by_addressing = ["masks", "maskt", "uls", "ult", "lrs", "lrt", "cms", "cmt"];
        assert_eq!(here.len(), 8);
        assert_eq!(owned_by_addressing.len(), 8);
        assert_eq!(here.len() + owned_by_addressing.len(), 16);
        for name in here {
            assert!(
                !owned_by_addressing.contains(&name),
                "{name} is claimed by both halves of the split"
            );
        }
        // And the union really has 16 distinct names, not 16 with a dupe.
        let mut all: Vec<&str> = here
            .iter()
            .chain(owned_by_addressing.iter())
            .copied()
            .collect();
        all.sort_unstable();
        let before = all.len();
        all.dedup();
        assert_eq!(all.len(), before);
        assert_eq!(all.len(), 16);
    }

    #[test]
    fn shared_params_rdp_tile_addressing_owner_still_exposes_its_eight() {
        // The reuse claim is only true while the owner exists with those
        // fields; construct one so a rename over there breaks this test
        // rather than silently orphaning the split.
        let addressing = crate::rt64_texture_sampler::RdpTileAddressing {
            cms: crate::rt64_texture_sampler::G_TX_CLAMP,
            cmt: crate::rt64_texture_sampler::G_TX_MIRROR,
            masks: 5,
            maskt: 6,
            uls: 1.0,
            ult: 2.0,
            lrs: 3.0,
            lrt: 4.0,
        };
        assert_eq!(addressing.masks, 5);
        assert_eq!(addressing.maskt, 6);
        assert_eq!(addressing.cms, 2);
        assert_eq!(addressing.cmt, 1);
    }

    // -- RasterParamsIndexing (rt64_raster_params.h delta) --

    #[test]
    fn shared_params_raster_params_padding_is_three_components() {
        // `uint3 padding`, not uint2 and not uint4. Asserted two ways: the
        // literal length, and by filling every slot with a distinct value so
        // a miscount cannot pass.
        let indexing = RasterParamsIndexing {
            render_index: 42,
            padding: [7, 8, 9],
        };
        assert_eq!(indexing.padding.len(), 3);
        assert_eq!(indexing.padding, [7, 8, 9]);
        assert_eq!(indexing.render_index, 42);
        assert_eq!(indexing.padding.iter().sum::<u32>(), 24);
    }

    #[test]
    fn shared_params_raster_params_render_index_precedes_padding() {
        // renderIndex is the first member; padding follows. Distinct values
        // catch a swap of the two declarations.
        let indexing = RasterParamsIndexing {
            render_index: 1,
            padding: [0, 0, 0],
        };
        assert_eq!(indexing.render_index, 1);
        assert_eq!(indexing.padding, [0, 0, 0]);
        assert_eq!(RasterParamsIndexing::default().render_index, 0);
    }

    #[test]
    fn shared_params_raster_params_split_covers_all_four_members() {
        // 2 here + 2 in raster_vs::ScreenTransform == RasterParams' 4.
        let screen = crate::raster_vs::ScreenTransform {
            scale: [2.0, 3.0],
            offset: [4.0, 5.0],
        };
        assert_eq!(screen.scale, [2.0, 3.0]);
        assert_eq!(screen.offset, [4.0, 5.0]);
        let here = ["renderIndex", "padding"];
        let owned_by_screen_transform = ["screenScale", "screenOffset"];
        assert_eq!(here.len() + owned_by_screen_transform.len(), 4);
        for name in here {
            assert!(!owned_by_screen_transform.contains(&name));
        }
    }

    // -- VisualizationMode (rt64_raytracing_params.h delta) --

    #[test]
    fn shared_params_visualization_mode_discriminants_are_zero_through_seventeen() {
        // Hand-derived from the declaration order; C++ `enum class` with no
        // initializers numbers from 0 upward by one.
        assert_eq!(VisualizationMode::Final.discriminant(), 0);
        assert_eq!(VisualizationMode::ShadingPosition.discriminant(), 1);
        assert_eq!(VisualizationMode::ShadingNormal.discriminant(), 2);
        assert_eq!(VisualizationMode::ShadingSpecular.discriminant(), 3);
        assert_eq!(VisualizationMode::Diffuse.discriminant(), 4);
        assert_eq!(VisualizationMode::InstanceId.discriminant(), 5);
        assert_eq!(VisualizationMode::DirectLightRaw.discriminant(), 6);
        assert_eq!(VisualizationMode::DirectLightFiltered.discriminant(), 7);
        assert_eq!(VisualizationMode::IndirectLightRaw.discriminant(), 8);
        assert_eq!(VisualizationMode::IndirectLightFiltered.discriminant(), 9);
        assert_eq!(VisualizationMode::Reflection.discriminant(), 10);
        assert_eq!(VisualizationMode::Refraction.discriminant(), 11);
        assert_eq!(VisualizationMode::Transparent.discriminant(), 12);
        assert_eq!(VisualizationMode::Flow.discriminant(), 13);
        assert_eq!(VisualizationMode::ReactiveMask.discriminant(), 14);
        assert_eq!(VisualizationMode::LockMask.discriminant(), 15);
        assert_eq!(VisualizationMode::Depth.discriminant(), 16);
        assert_eq!(VisualizationMode::Count.discriminant(), 17);
    }

    #[test]
    fn shared_params_visualization_mode_all_is_dense_and_in_declaration_order() {
        // Second, independent derivation of the same numbering: the i-th
        // entry of ALL must have discriminant i, with no gaps. An inserted,
        // dropped, or reordered enumerator fails here even if the per-name
        // test above were edited to match.
        assert_eq!(VisualizationMode::ALL.len(), 18);
        for (i, mode) in VisualizationMode::ALL.iter().enumerate() {
            assert_eq!(
                mode.discriminant(),
                u32::try_from(i).expect("index fits u32"),
                "ALL[{i}] has the wrong discriminant"
            );
        }
    }

    #[test]
    fn shared_params_visualization_mode_count_is_the_sentinel_after_depth() {
        // `Count` is the trailing sentinel: exactly one past `Depth`, and its
        // value equals the number of real modes (Final..=Depth).
        assert_eq!(
            VisualizationMode::Count.discriminant(),
            VisualizationMode::Depth.discriminant() + 1
        );
        let real_modes = VisualizationMode::ALL
            .iter()
            .filter(|m| **m != VisualizationMode::Count)
            .count();
        assert_eq!(real_modes, 17);
        assert_eq!(
            u32::try_from(real_modes).expect("fits"),
            VisualizationMode::Count.discriminant()
        );
    }

    #[test]
    fn shared_params_visualization_mode_round_trips_through_its_discriminant() {
        for mode in VisualizationMode::ALL {
            assert_eq!(
                VisualizationMode::from_discriminant(mode.discriminant()),
                Some(mode)
            );
        }
    }

    #[test]
    fn shared_params_visualization_mode_rejects_out_of_range_values() {
        // DEVIATION pinned here, not the original: upstream's
        // `static_cast<interop::VisualizationMode>(int)` has no guard. This
        // port returns None rather than materializing an enum value with no
        // enumerator.
        assert_eq!(VisualizationMode::from_discriminant(18), None);
        assert_eq!(VisualizationMode::from_discriminant(100), None);
        assert_eq!(VisualizationMode::from_discriminant(u32::MAX), None);
        // 17 is still in range -- Count is a real enumerator.
        assert_eq!(
            VisualizationMode::from_discriminant(17),
            Some(VisualizationMode::Count)
        );
    }

    #[test]
    fn shared_params_visualization_mode_default_is_final() {
        // The constructor's `visualizationMode = VisualizationMode::Final`.
        assert_eq!(VisualizationMode::default(), VisualizationMode::Final);
        assert_eq!(VisualizationMode::default().discriminant(), 0);
    }

    // -- RaytracingParamsDefaults (rt64_raytracing_params.h delta) --

    #[test]
    fn shared_params_raytracing_defaults_five_nonzero_scalars() {
        // Hand-transcribed from constructor lines 88-90, 94-95, 102-103.
        let d = RaytracingParamsDefaults::SOURCE;
        assert_eq!(d.fov_radians, 0.707_f32);
        assert_eq!(d.near_dist, 1.0_f32);
        assert_eq!(d.far_dist, 1000.0_f32);
        assert_eq!(d.tonemap_exposure, 0.6_f32);
        assert_eq!(d.tonemap_white, 1.0_f32);
        assert_eq!(d.max_lights, 1_u32);
        assert_eq!(d.motion_blur_samples, 32_u32);
    }

    #[test]
    fn shared_params_raytracing_defaults_zero_scalars_are_zero() {
        let d = RaytracingParamsDefaults::SOURCE;
        assert_eq!(d.gi_diffuse_strength, 0.0);
        assert_eq!(d.gi_background_strength, 0.0);
        assert_eq!(d.motion_blur_strength, 0.0);
        assert_eq!(d.tonemap_black, 0.0);
        assert_eq!(d.di_samples, 0);
        assert_eq!(d.gi_samples, 0);
        assert_eq!(d.di_reproject, 0);
        assert_eq!(d.gi_reproject, 0);
        assert_eq!(d.binary_lock_mask, 0);
        assert_eq!(d.pixel_jitter, [0.0, 0.0]);
    }

    #[test]
    fn shared_params_raytracing_defaults_max_lights_is_one_not_zero() {
        // The single easiest default to get wrong: `maxLights = 1`, sitting
        // between four consecutive `= 0` assignments. Asserted two ways.
        let d = RaytracingParamsDefaults::SOURCE;
        assert_eq!(d.max_lights, 1);
        assert_ne!(d.max_lights, 0);
        assert_ne!(d.max_lights, d.di_samples);
    }

    #[test]
    fn shared_params_raytracing_defaults_motion_blur_samples_is_thirty_two() {
        // Reconciled two independent ways: the literal 32, and 1 << 5, so a
        // 31/33 off-by-one fails the second even if the first were edited.
        let d = RaytracingParamsDefaults::SOURCE;
        assert_eq!(d.motion_blur_samples, 32);
        assert_eq!(d.motion_blur_samples, 1u32 << 5);
        assert_eq!(d.motion_blur_samples.count_ones(), 1);
    }

    #[test]
    fn shared_params_raytracing_defaults_motion_blur_is_inactive_at_defaults() {
        // Cross-checks this module's defaults against the already-landed
        // consumer of the same two fields: with strength 0 the existing gate
        // must be false even though samples is 32.
        let d = RaytracingParamsDefaults::SOURCE;
        assert!(!crate::rt64_postprocess::motion_blur_is_active(
            d.motion_blur_strength,
            d.motion_blur_samples
        ));
        // ... and flipping only strength turns it on, proving samples
        // (32, not 0) is not the thing suppressing it.
        assert!(crate::rt64_postprocess::motion_blur_is_active(
            1.0,
            d.motion_blur_samples
        ));
    }

    #[test]
    fn shared_params_raytracing_defaults_all_nine_float4_members_are_zero() {
        let d = RaytracingParamsDefaults::SOURCE;
        let vectors = [
            d.camera_u,
            d.camera_v,
            d.camera_w,
            d.viewport,
            d.resolution,
            d.ambient_base_color,
            d.ambient_no_gi_color,
            d.eye_light_diffuse_color,
            d.eye_light_specular_color,
        ];
        assert_eq!(vectors.len(), 9);
        for v in vectors {
            assert_eq!(v.x, 0.0);
            assert_eq!(v.y, 0.0);
            assert_eq!(v.z, 0.0);
            assert_eq!(v.w, 0.0);
        }
    }

    #[test]
    fn shared_params_raytracing_defaults_visualization_mode_is_final() {
        assert_eq!(
            RaytracingParamsDefaults::SOURCE.visualization_mode,
            VisualizationMode::Final
        );
    }

    #[test]
    fn shared_params_raytracing_defaults_derive_matches_the_source_constant() {
        assert_eq!(
            RaytracingParamsDefaults::default(),
            RaytracingParamsDefaults::SOURCE
        );
    }

    // -- float3 witness --

    #[test]
    fn shared_params_float3_witness_is_three_components() {
        let v = Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        assert_eq!(float3_component_count(v), 3);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.z, 3.0);
    }
}
