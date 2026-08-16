#![allow(clippy::too_many_arguments)]

use super::*;
use crate::state::OtherMode;

fn one_cycle_mode(zsrc_prim: bool) -> OtherMode {
    let low = if zsrc_prim { 1u32 << 2 } else { 0 };
    OtherMode::from_wire(0u32 << 20, low)
}

fn copy_mode(zsrc_prim: bool) -> OtherMode {
    let low = if zsrc_prim { 1u32 << 2 } else { 0 };
    OtherMode::from_wire(2u32 << 20, low)
}

fn prim_depth(z15: u16, dz: u16) -> PrimDepth {
    let w1 = ((z15 as u32 & 0x7fff) << 16) | (dz as u32);
    PrimDepth::from_wire(w1)
}

fn params(is_rect: bool, other_mode: OtherMode, prim_depth: PrimDepth) -> RasterVsParams {
    RasterVsParams {
        is_rect,
        other_mode,
        prim_depth,
    }
}

fn identity_screen() -> ScreenTransform {
    ScreenTransform {
        scale: [1.0, 1.0],
        offset: [0.0, 0.0],
    }
}

fn res(width: f32, height: f32) -> Resolution {
    Resolution { width, height }
}

// --- Screen-space conversion (non-rect path) ---------------------------

#[test]
fn non_rect_maps_screen_center_to_ndc_origin() {
    let position = RasterVsPosition {
        x: 160.0,
        y: 120.0,
        z: 0.5,
        w: 1.0,
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(false, one_cycle_mode(false), prim_depth(0, 0)),
    );
    assert_eq!(out.x, 0.0);
    assert_eq!(out.y, 0.0);
    assert_eq!(out.z, 0.5);
    assert_eq!(out.w, 1.0);
}

#[test]
fn non_rect_maps_screen_corners_to_ndc_corners_with_y_flip() {
    let position = RasterVsPosition {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(false, one_cycle_mode(false), prim_depth(0, 0)),
    );
    // Top-left screen pixel (0,0) maps to NDC (-1, +1): RDP Y is top-down,
    // NDC Y is up, so the HLSL's negated Y divisor flips the sign.
    assert_eq!(out.x, -1.0);
    assert_eq!(out.y, 1.0);
}

#[test]
fn non_rect_scales_xyz_by_w() {
    let position = RasterVsPosition {
        x: 160.0,
        y: 120.0,
        z: 0.5,
        w: 2.0,
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(false, one_cycle_mode(false), prim_depth(0, 0)),
    );
    // (x - 160)/160 * w = 0 * 2 = 0 at screen center; z *= w = 1.0.
    assert_eq!(out.x, 0.0);
    assert_eq!(out.y, 0.0);
    assert_eq!(out.z, 1.0);
    assert_eq!(out.w, 2.0);
}

// --- Rect skip branch ----------------------------------------------------

#[test]
fn rect_skips_screen_to_ndc_conversion() {
    let position = RasterVsPosition {
        x: 160.0,
        y: 120.0,
        z: 0.5,
        w: 1.0,
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(true, one_cycle_mode(false), prim_depth(0, 0)),
    );
    // No conversion: x/y/z pass through the (identity) screen transform
    // unchanged, unlike the non-rect case which maps (160,120) to (0,0).
    assert_eq!(out.x, 160.0);
    assert_eq!(out.y, 120.0);
    assert_eq!(out.z, 0.5);
}

// --- Screen scale/offset (applies regardless of is_rect) -----------------

#[test]
fn screen_scale_and_offset_apply_after_rect_skip() {
    let position = RasterVsPosition {
        x: 10.0,
        y: 20.0,
        z: 0.0,
        w: 1.0,
    };
    let screen = ScreenTransform {
        scale: [2.0, 3.0],
        offset: [5.0, -5.0],
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        screen,
        params(true, one_cycle_mode(false), prim_depth(0, 0)),
    );
    assert_eq!(out.x, 10.0 * 2.0 + 5.0 * 1.0);
    assert_eq!(out.y, 20.0 * 3.0 + -5.0 * 1.0);
}

#[test]
fn screen_offset_is_scaled_by_w() {
    let position = RasterVsPosition {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 4.0,
    };
    let screen = ScreenTransform {
        scale: [1.0, 1.0],
        offset: [1.0, 1.0],
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        screen,
        params(true, one_cycle_mode(false), prim_depth(0, 0)),
    );
    assert_eq!(out.x, 4.0);
    assert_eq!(out.y, 4.0);
}

#[test]
fn screen_scale_and_offset_apply_after_non_rect_conversion() {
    let position = RasterVsPosition {
        x: 160.0,
        y: 120.0,
        z: 0.0,
        w: 1.0,
    };
    let screen = ScreenTransform {
        scale: [2.0, 2.0],
        offset: [1.0, 1.0],
    };
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        screen,
        params(false, one_cycle_mode(false), prim_depth(0, 0)),
    );
    // NDC at screen center is (0,0); scale by 2 keeps it 0, offset adds 1*w.
    assert_eq!(out.x, 1.0);
    assert_eq!(out.y, 1.0);
}

// --- Prim-depth Z override -----------------------------------------------

#[test]
fn zsource_prim_in_one_cycle_mode_overrides_z() {
    let position = RasterVsPosition {
        x: 0.0,
        y: 0.0,
        z: 0.75,
        w: 2.0,
    };
    let depth = prim_depth(32767, 0); // z_normalized() == 1.0
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(true, one_cycle_mode(true), depth),
    );
    assert_eq!(out.z, 1.0 * 2.0);
}

#[test]
fn zsource_pixel_in_one_cycle_mode_leaves_z_untouched() {
    let position = RasterVsPosition {
        x: 0.0,
        y: 0.0,
        z: 0.75,
        w: 1.0,
    };
    let depth = prim_depth(32767, 0);
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(true, one_cycle_mode(false), depth),
    );
    assert_eq!(out.z, 0.75);
}

#[test]
fn zsource_prim_in_copy_mode_leaves_z_untouched() {
    let position = RasterVsPosition {
        x: 0.0,
        y: 0.0,
        z: 0.75,
        w: 1.0,
    };
    let depth = prim_depth(32767, 0);
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(true, copy_mode(true), depth),
    );
    // G_CYC_COPY bypasses the Z override even when zSource selects G_ZS_PRIM.
    assert_eq!(out.z, 0.75);
}

#[test]
fn zsource_prim_override_composes_with_non_rect_z_scale_by_w() {
    // The override reassigns z *after* the non-rect `z *= w` step, using the
    // same final w -- it does not compose with the pre-override z, only
    // replaces it, matching RT64's unconditional `ndcPos.z = ... * ndcPos.w`.
    let position = RasterVsPosition {
        x: 160.0,
        y: 120.0,
        z: 999.0,
        w: 3.0,
    };
    let depth = prim_depth(16383, 0); // 16383 / 32767.0
    let out = raster_vs(
        position,
        res(320.0, 240.0),
        identity_screen(),
        params(false, one_cycle_mode(true), depth),
    );
    let expected_z = (16383.0_f32 / 32767.0) * 3.0;
    assert_eq!(out.z, expected_z);
}

#[test]
fn prim_depth_z_normalized_uses_15_bit_mask_and_32767_divisor() {
    assert_eq!(prim_depth(32767, 0).z_normalized(), 1.0);
    assert_eq!(prim_depth(0, 0).z_normalized(), 0.0);
}

// --- WGSL structural checks ------------------------------------------------

#[test]
fn wgsl_entry_point_name_matches_constant() {
    assert!(RASTER_VS_WGSL.contains(&format!("fn {RASTER_VS_ENTRY_POINT}(")));
}

#[test]
fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(RASTER_VS_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn wgsl_source_contains_the_exact_transform_and_override_branches() {
    assert!(RASTER_VS_WGSL.contains("if (input.is_rect == 0u)"));
    assert!(RASTER_VS_WGSL.contains("x -= input.resolution_x / 2.0;"));
    assert!(RASTER_VS_WGSL.contains("y /= input.resolution_y / -2.0;"));
    assert!(RASTER_VS_WGSL.contains("if (input.z_override != 0u)"));
    assert!(RASTER_VS_WGSL.contains("z = input.prim_depth_z_normalized * w;"));
}

#[test]
fn duplicate_binding_index_fails_naga_validation() {
    let duplicate_binding = RASTER_VS_WGSL.replacen("@binding(1)", "@binding(0)", 1);
    let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_err());
}

#[test]
fn malformed_wgsl_fails_to_parse() {
    let truncated = &RASTER_VS_WGSL[..RASTER_VS_WGSL.len() / 2];
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn wgsl_oracle_agrees_with_rust_across_a_representative_grid() {
    // Differential (structural/textual, not GPU-executed -- matching
    // blend.rs/alpha_compare.rs/depth_strict_less.rs's identically-scoped
    // precedent and this crate's lack of a compute-dispatch test harness):
    // independently re-evaluate the WGSL's exact textual formula in Rust and
    // confirm it agrees with the `raster_vs` CPU oracle across a
    // representative (position, resolution, screen, mode) grid.
    fn wgsl_formula(
        x_in: f32,
        y_in: f32,
        z_in: f32,
        w: f32,
        resolution: Resolution,
        screen: ScreenTransform,
        is_rect: bool,
        z_override: bool,
        prim_depth_z_normalized: f32,
    ) -> (f32, f32, f32, f32) {
        let mut x = x_in;
        let mut y = y_in;
        let mut z = z_in;
        if !is_rect {
            x -= resolution.width / 2.0;
            y -= resolution.height / 2.0;
            x /= resolution.width / 2.0;
            y /= resolution.height / -2.0;
            x *= w;
            y *= w;
            z *= w;
        }
        x = (x * screen.scale[0]) + screen.offset[0] * w;
        y = (y * screen.scale[1]) + screen.offset[1] * w;
        if z_override {
            z = prim_depth_z_normalized * w;
        }
        (x, y, z, w)
    }

    let resolution = res(320.0, 240.0);
    let positions = [(0.0, 0.0), (160.0, 120.0), (319.0, 239.0)];
    let ws = [1.0_f32, 0.5, 2.0];
    let screens = [
        identity_screen(),
        ScreenTransform {
            scale: [1.5, 0.75],
            offset: [3.0, -2.0],
        },
    ];
    for &(px, py) in &positions {
        for &w in &ws {
            for &screen in &screens {
                for &is_rect in &[false, true] {
                    for &z_override in &[false, true] {
                        let other_mode = if z_override {
                            one_cycle_mode(true)
                        } else {
                            one_cycle_mode(false)
                        };
                        let depth = prim_depth(16000, 0);
                        let position = RasterVsPosition {
                            x: px,
                            y: py,
                            z: 0.25,
                            w,
                        };
                        let expected = raster_vs(
                            position,
                            resolution,
                            screen,
                            params(is_rect, other_mode, depth),
                        );
                        let actual = wgsl_formula(
                            px,
                            py,
                            0.25,
                            w,
                            resolution,
                            screen,
                            is_rect,
                            z_override,
                            depth.z_normalized(),
                        );
                        assert_eq!(actual.0, expected.x);
                        assert_eq!(actual.1, expected.y);
                        assert_eq!(actual.2, expected.z);
                        assert_eq!(actual.3, expected.w);
                    }
                }
            }
        }
    }
}
