//! Inert RT64 literal-port characterization portfolio.
//!
//! Every module here is a literal port of an RT64 source unit, carried for
//! the characterization tests attached to it. None of it is wired to a
//! rendered frame: this crate depends on `fn64-render-wgpu`, and
//! `fn64-render-wgpu` does not depend on this crate, so no port here can
//! reach a production draw path even by accident. That one-way arrow is
//! the structural form of the invariant `scripts/lint-rt64-ports-inert.py`
//! used to enforce with a grep over `pub mod` in the backend's `lib.rs`.
//!
//! The portfolio lived in `fn64-render-wgpu/src` until Task 4.6 of
//! `docs/plans/CLEANUP-2026-09.md`; it moved out because it doubled that
//! crate's `cargo test` compile surface while contributing nothing to a
//! default build. Wiring a port ON PURPOSE means moving the module back
//! into `fn64-render-wgpu` and calling it explicitly, the way
//! `rt64_gbi_rdp_decode::decode_set_scissor` (which stayed behind, being
//! the one production-wired port) is called from `raw_dpc`.
#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod color_converter;
pub mod color_hlsli;
pub mod depth_encode;
pub mod fbcommon;
pub mod rt64_blender_emulation;
pub mod rt64_common;
pub mod rt64_extended_gbi;
pub mod rt64_extra_params;
pub mod rt64_fb_reinterpret;
pub mod rt64_float4_quantize;
pub mod rt64_frame_compatibility;
pub mod rt64_framebuffer_geometry;
pub mod rt64_framebuffer_shaders;
pub mod rt64_framebuffer_storage;
pub mod rt64_framebuffer_tile;
pub mod rt64_fullscreen_vs;
pub mod rt64_gaussian_filter;
pub mod rt64_gbi_extended_decode;
pub mod rt64_gbi_f3d;
pub mod rt64_gbi_f3d_variants;
pub mod rt64_gbi_f3dex;
pub mod rt64_gbi_f3dex2;
pub mod rt64_gbi_opcodes;
pub mod rt64_gbi_s2dex2;
pub mod rt64_hle_geometry;
pub mod rt64_hlsl_interop;
pub mod rt64_interpolation_helpers;
pub mod rt64_light_estimation;
pub mod rt64_lights_math;
pub mod rt64_luminance_histogram;
pub mod rt64_math;
pub mod rt64_math_decompose;
pub mod rt64_math_matrix;
pub mod rt64_postprocess;
pub mod rt64_present_shaders;
pub mod rt64_preset_draw_call_match;
pub mod rt64_preset_light;
pub mod rt64_preset_material;
pub mod rt64_preset_scene;
pub mod rt64_profiling_timer;
pub mod rt64_rdp_state;
pub mod rt64_render_flags;
pub mod rt64_render_pipeline_types;
pub mod rt64_render_target_geometry;
pub mod rt64_replacement_resolve;
pub mod rt64_resample;
pub mod rt64_rigid_body;
pub mod rt64_rsp_matrix_stack;
pub mod rt64_rsp_patch;
pub mod rt64_rsp_process;
pub mod rt64_rsp_segment;
pub mod rt64_rsp_smooth_normal;
pub mod rt64_rsp_world_modify;
pub mod rt64_shader_description;
pub mod rt64_shared_params;
pub mod rt64_texture_map_lru;
pub mod rt64_texture_sampler;
pub mod rt64_tmem_hasher;
pub mod rt64_tmem_regions;
pub mod rt64_upload_geometry;
pub mod rt64_user_configuration;
pub mod rt64_vi_timing;
pub mod rt64_workload_geometry;
pub mod texture_lod;

/// The portfolio's own census. It is the moved counterpart of
/// `fn64-render-wgpu`'s `characterization_gate_tests`: that test asserted
/// every `rt64_*` module but one sat behind
/// `#[cfg(any(test, feature = "rt64-port-characterization"))]`, which was
/// how inertness was expressed while the modules lived in the backend.
/// Here inertness is structural (nothing depends on this crate), so what
/// is left to pin is the inventory itself -- a module silently added or
/// dropped should be a deliberate, reviewed act.
#[cfg(test)]
mod portfolio_census {
    #[test]
    fn the_characterization_portfolio_declares_every_moved_module() {
        let source = include_str!("lib.rs");
        let declared: Vec<&str> = source
            .lines()
            .filter_map(|line| line.strip_prefix("pub mod "))
            .filter_map(|line| line.strip_suffix(';'))
            .collect();
        assert_eq!(declared.len(), 64);
        let ports = declared.iter().filter(|n| n.starts_with("rt64_")).count();
        assert_eq!(
            ports, 59,
            "the RT64 literal ports, less `rt64_gbi_rdp_decode`, which stayed \
             in fn64-render-wgpu because `raw_dpc` calls it"
        );
        let mut sorted = declared.clone();
        sorted.sort_unstable();
        assert_eq!(declared, sorted, "module declarations stay sorted");
    }
}
