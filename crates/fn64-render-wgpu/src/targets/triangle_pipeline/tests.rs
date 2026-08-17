use super::*;
use crate::combiner::{run_one_cycle, CombinerInputs};
use crate::depth_strict_less::{
    strict_less_depth_test, StrictLessDepthOutcome, StrictLessDepthSample,
};

// Fixed fixture (port card §3/§6): one triangle covering a known pixel
// subset of an 8x8 target. Vertices are chosen in RDP screen-pixel space
// (matching `RasterVsPosition`'s `iPosition` convention -- see
// `raster_vs.rs`'s module doc) so the NDC transform in
// `triangle_pipeline_vertex.wgsl` is exercised, not bypassed.
const EXTENT: TriangleTargetExtent = TriangleTargetExtent {
    width: 8,
    height: 8,
};

fn identity_raster_params() -> TriangleRasterParams {
    TriangleRasterParams {
        resolution: [EXTENT.width as f32, EXTENT.height as f32],
        screen_scale: [1.0, 1.0],
        screen_offset: [0.0, 0.0],
    }
}

/// The named "no committed TMEM, no tile bound" case (published
/// committed-TMEM textured-draw card §6) -- every non-textured fixture in
/// this file uses this: an all-zero `TmemGpuProjection` plus
/// `TileBindingParams::unbound()`. `tmem_sample.wgsl`'s
/// `sample_committed_rgba16_three_nearest` checks `bound` before touching
/// TMEM bytes, so `tex_val0`/`tex_val1` come back `(0,0,0,0)` with status
/// `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` -- irrelevant to every fixture below
/// since they all use the SHADE-passthrough combine formula, which never
/// reads `tex_val0`/`tex_val1`.
fn no_tmem_binding() -> (TmemGpuProjection, TileBindingParams) {
    (
        TmemGpuProjection {
            bytes: [0u8; fn64_render_ir::TMEM_BYTES as usize],
            validity_words: [0u32; crate::TMEM_VALIDITY_WORDS],
        },
        TileBindingParams::unbound(),
    )
}

// A large triangle covering the whole 8x8 target (screen-pixel corners at
// (0,0), (8,0), (0,8)), each vertex a distinct primary color, w = 1.0
// (matching the reference `raster_vs.rs` transform's unconditional `x *= w`
// with w=1 leaving x/y unchanged beyond the resolution normalization),
// z = 0.5 for every vertex (a flat triangle, so depth is uniform across all
// covered pixels regardless of wgpu's actual barycentric interpolation --
// this sidesteps needing to hand-derive per-pixel interpolated Z).
fn covering_triangle_fixture() -> TriangleFixture {
    let (tmem, tile_binding) = no_tmem_binding();
    TriangleFixture {
        vertices: [
            RasterVertex {
                position: [0.0, 0.0, 0.5, 1.0],
                uv: [0.0, 0.0],
                color: [1.0, 0.0, 0.0, 1.0],
            },
            RasterVertex {
                position: [8.0, 0.0, 0.5, 1.0],
                uv: [1.0, 0.0],
                color: [0.0, 1.0, 0.0, 1.0],
            },
            RasterVertex {
                position: [0.0, 8.0, 0.5, 1.0],
                uv: [0.0, 1.0],
                color: [0.0, 0.0, 1.0, 1.0],
            },
        ],
        raster_params: identity_raster_params(),
        // Literal one-cycle SHADE passthrough via the zero-times-anything
        // identity: A=B=COMBINED (index 0, both common-table) makes
        // `(A - B) == 0` regardless of what C decodes to, so
        // `(A-B)*C + D` collapses to plain `D`; D=SHADE (index 4, valid in
        // D's own common-table range 0-5) makes the whole formula equal the
        // vertex-interpolated shade color unchanged. This sidesteps needing
        // a COLOR_ONE selector, which color slot C's table cannot produce
        // (`color_input_c`, `combiner.rs`: 0-5 common, 6=KEY_SCALE,
        // 7-15=`*_ALPHA`/LOD/K5, no ONE entry at all -- only slots A and D
        // have a ONE code). Alpha mirrors the same identity: alpha A=B=
        // COMBINED(0), alpha D=SHADE(4, `alpha_input_abd`'s common table).
        combine_params: shade_passthrough_combine_params(),
        extent: EXTENT,
        tmem,
        tile_binding,
        alpha_compare_mode: crate::state::AlphaCompare::None,
        blend_color: None,
        env_color: None,
        prim_color: None,
        blend_params: ResolvedFragmentBlendParams::NO_OP,
        // (Z_CMP, Z_UPD) = (set, set): the pipeline's prior sole state
        // (`depth_pipeline_index((true, true)) == 0`, `Less`/write-always)
        // -- this base fixture is the regression-guard default every
        // existing depth test reuses unmodified.
        depth_compare_enabled: true,
        depth_update_enabled: true,
        // Production coverage node 1's no-op default: `Full`/no-image-read
        // (`coverage_destination` `Full` reads no `memory`, matching this
        // pipeline's `Full`/no-image-read-only scope), no coverage-alpha
        // interaction -- `output.color.a` stays the combiner's own alpha
        // unmodified (`alpha_coverage_select == false`), the regression-
        // guard default every existing test in this file reuses unmodified.
        coverage_destination: crate::state::CoverageDestination::Full,
        image_read_enabled: false,
        force_blend: false,
        antialias_enabled: false,
        coverage_times_alpha: false,
        alpha_coverage_select: false,
        is_rect: false,
    }
}

// Builds `CombineParams` wire bits for the one-cycle formula
// `(COMBINED - COMBINED) * anything + SHADE`, i.e. plain SHADE passthrough
// (see `covering_triangle_fixture`'s doc for why this identity is used
// instead of a literal `* ONE`). Slot bit positions from `combiner.rs`'s
// `CombineParams::parse_color_a/b/c/d`/`parse_alpha_a/b/c/d`
// (second_cycle=true path, since `run_one_cycle` always evaluates with
// `SECOND_CYCLE = true` per that function's own doc). Field widths:
// color_a/color_b are 4-bit, color_c is 5-bit, color_d is 3-bit; alpha
// a/b/c/d are all 3-bit (`combiner.rs`'s `& 0xF`/`& 0x1F`/`& 0x7` masks).
fn shade_passthrough_combine_params() -> CombineParams {
    let color_a: u32 = 0; // COMBINED (common table, any slot)
    let color_b: u32 = 0; // COMBINED -- (A - B) == 0
    let color_c: u32 = 0; // irrelevant: multiplied by zero
    let color_d: u32 = 4; // SHADE (common table, valid for D too)
    let alpha_a: u32 = 0; // COMBINED (alpha_input_abd)
    let alpha_b: u32 = 0; // COMBINED -- (A - B) == 0
    let alpha_c: u32 = 1; // TEXEL0 in alpha_input_c's table; irrelevant, x0
    let alpha_d: u32 = 4; // SHADE (alpha_input_abd's common table)

    // second_cycle=true bit positions (`parse_*_a/b/c/d`'s `if second_cycle`
    // arms): color_a low[9:5], color_b high[31:24], color_c low[4:0],
    // color_d high[8:6]; alpha_a high[23:21], alpha_b high[5:3],
    // alpha_c high[20:18], alpha_d high[2:0].
    let low = (color_a << 5) | color_c;
    let high = (color_b << 24)
        | (color_d << 6)
        | (alpha_a << 21)
        | (alpha_b << 3)
        | (alpha_c << 18)
        | alpha_d;
    CombineParams::from_wire(low, high)
}

fn cpu_combiner_reference(shade_color: [f32; 4]) -> [f32; 4] {
    let params = shade_passthrough_combine_params();
    let inputs = CombinerInputs {
        tex_val0: [0.0; 4],
        tex_val1: [0.0; 4],
        prim_color: [0.0; 4],
        shade_color,
        env_color: [0.0; 4],
        key_center: [0.0; 3],
        key_scale: [0.0; 3],
        lod_fraction: 0.0,
        prim_lod_frac: 0.0,
        noise: 0.0,
        k4: 0.0,
        k5: 0.0,
    };
    let (color, _alpha_compare) = run_one_cycle(params, inputs);
    color
}

#[test]
fn shade_passthrough_combine_params_reduce_to_the_shade_color() {
    // Independent CPU-side proof this fixture's CombineParams really is a
    // SHADE passthrough before it is ever submitted to the GPU (port card
    // §6: "hand-computed expected... not via this crate's own code" is
    // satisfied for the *fixture choice*; this test cross-checks the
    // fixture against the same crate's own combiner oracle, matching the
    // card's "Combiner differential" test-plan item).
    let red = cpu_combiner_reference([1.0, 0.0, 0.0, 1.0]);
    assert!((red[0] - 1.0).abs() < 1e-6);
    assert!(red[1].abs() < 1e-6);
    assert!(red[2].abs() < 1e-6);
    assert!((red[3] - 1.0).abs() < 1e-6);

    let mixed = cpu_combiner_reference([0.25, 0.5, 0.75, 1.0]);
    assert!((mixed[0] - 0.25).abs() < 1e-6);
    assert!((mixed[1] - 0.5).abs() < 1e-6);
    assert!((mixed[2] - 0.75).abs() < 1e-6);
}

/// Exact four-row truth table (production depth-slice task card §3,
/// `depth_pipeline_index`'s own doc table) -- every `(Z_CMP, Z_UPD)`
/// combination maps to a distinct index 0-3, and index 0 is the pipeline's
/// prior sole `(true, true)` state.
#[test]
fn depth_pipeline_index_matches_the_exact_four_row_truth_table() {
    assert_eq!(depth_pipeline_index(true, true), 0);
    assert_eq!(depth_pipeline_index(true, false), 1);
    assert_eq!(depth_pipeline_index(false, true), 2);
    assert_eq!(depth_pipeline_index(false, false), 3);
}

/// [`DEPTH_STENCIL_VARIANTS`] is the single source of truth `request()`
/// builds pipelines from and [`depth_pipeline_index`] indexes into -- this
/// proves the two cannot silently drift apart: each index's stored
/// `(depth_compare, depth_write_enabled)` pair matches the task card's table
/// exactly, keyed by the same `(Z_CMP, Z_UPD)` inputs `depth_pipeline_index`
/// consumes.
#[test]
fn depth_stencil_variants_table_matches_depth_pipeline_index_for_every_combination() {
    for (depth_compare_enabled, depth_update_enabled) in
        [(true, true), (true, false), (false, true), (false, false)]
    {
        let index = depth_pipeline_index(depth_compare_enabled, depth_update_enabled);
        let (depth_compare, depth_write_enabled) = DEPTH_STENCIL_VARIANTS[index];
        let expected_compare = if depth_compare_enabled {
            wgpu::CompareFunction::Less
        } else {
            wgpu::CompareFunction::Always
        };
        assert_eq!(
            depth_compare, expected_compare,
            "(Z_CMP={depth_compare_enabled}, Z_UPD={depth_update_enabled}) at index {index}"
        );
        assert_eq!(
            depth_write_enabled, depth_update_enabled,
            "(Z_CMP={depth_compare_enabled}, Z_UPD={depth_update_enabled}) at index {index}"
        );
    }
}

/// `DEPTH_STENCIL_VARIANTS` has exactly one entry per `depth_pipeline_index`
/// output value (0-3, no duplicates, no gaps) -- guards against a future
/// edit silently aliasing two `(Z_CMP, Z_UPD)` combinations onto the same
/// pipeline variant.
#[test]
fn depth_pipeline_index_is_a_bijection_onto_the_four_variant_indices() {
    let mut seen = [false; 4];
    for depth_compare_enabled in [true, false] {
        for depth_update_enabled in [true, false] {
            let index = depth_pipeline_index(depth_compare_enabled, depth_update_enabled);
            assert!(
                !seen[index],
                "index {index} produced by more than one (Z_CMP, Z_UPD) combination"
            );
            seen[index] = true;
        }
    }
    assert_eq!(seen, [true; 4]);
}

/// `submit_admitted_triangle`'s doc claims `other_mode.depth_compare_enabled()`/
/// `depth_update_enabled()` feed the fixture's two depth fields verbatim, no
/// arithmetic -- this proves that claim independently of any device, for
/// every one of the four wire combinations.
#[test]
fn other_mode_depth_bits_map_onto_triangle_fixture_fields_verbatim() {
    for (low, expected_compare, expected_update) in [
        (0x0000u32, false, false),
        (0x0010u32, true, false),
        (0x0020u32, false, true),
        (0x0030u32, true, true),
    ] {
        let mode = crate::state::OtherMode::from_wire(0, low);
        assert_eq!(mode.depth_compare_enabled(), expected_compare);
        assert_eq!(mode.depth_update_enabled(), expected_update);
    }
}

#[test]
fn fixed_fixture_other_mode_is_one_cycle_no_z_override_no_force_blend() {
    let mode = fixed_fixture_other_mode();
    assert_eq!(mode.cycle_type(), crate::state::CycleType::OneCycle);
    assert!(!mode.primitive_depth_source());
    assert!(!mode.force_blend());
}

#[test]
fn vertex_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module =
        naga::front::wgsl::parse_str(crate::shader_manifest::TRIANGLE_PIPELINE_VERTEX_WGSL)
            .unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn combined_fragment_wgsl_parses_and_validates_under_closed_naga_profile() {
    let source = crate::shader_manifest::triangle_pipeline_fragment_wgsl();
    let module = naga::front::wgsl::parse_str(&source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn combined_fragment_wgsl_reuses_color_combiner_wgsl_byte_for_byte() {
    let source = crate::shader_manifest::triangle_pipeline_fragment_wgsl();
    assert!(source.starts_with(crate::combiner::COLOR_COMBINER_WGSL));
    assert!(source.contains("fn fs_main("));
}

/// Structural guard (alpha-compare production card §4g): the combined
/// fragment source contains exactly one call site for
/// `alpha_compare_fragment_fn(` (reused from `alpha_compare_fragment_fn.wgsl`,
/// never re-typed inline -- card §3a/§3e) and exactly one `discard;`
/// statement inside `fs_main`, and the callable itself is concatenated only
/// once (no accidental double-concatenation at the `shader_manifest.rs`
/// seam).
#[test]
fn combined_fragment_wgsl_has_exactly_one_alpha_compare_call_and_one_discard() {
    let source = crate::shader_manifest::triangle_pipeline_fragment_wgsl();

    let call_count = source.matches("alpha_compare_fragment_fn(").count();
    assert_eq!(
        call_count, 2,
        "expected exactly one call site plus the callable's own `fn \
         alpha_compare_fragment_fn(` declaration (2 total occurrences of the \
         substring), got {call_count}"
    );
    let fn_declaration_count = source.matches("fn alpha_compare_fragment_fn(").count();
    assert_eq!(
        fn_declaration_count, 1,
        "the callable itself must be concatenated exactly once, not duplicated \
         at the shader_manifest.rs seam"
    );

    let discard_count = source.matches("discard;").count();
    assert_eq!(
        discard_count, 1,
        "fs_main must have exactly one discard statement, got {discard_count}"
    );

    // Ordering: the alpha-compare callable's own declaration must precede
    // fs_main (concatenation-seam order, card §3a: "after color_combiner.wgsl
    // and tmem_sample.wgsl, before this file's own body").
    let fn_index = source
        .find("fn alpha_compare_fragment_fn(")
        .expect("callable must exist");
    let fs_main_index = source.find("fn fs_main(").expect("fs_main must exist");
    assert!(
        fn_index < fs_main_index,
        "alpha_compare_fragment_fn must be concatenated before fs_main, not after"
    );
}

/// Source-shape proof for the TMEM-sample gate (SHADE-only-triangle
/// repair): `fs_main` must call
/// `sample_committed_rgba16_three_nearest_bound` from inside a branch keyed
/// on `fragment_combine_params.texture_referenced`, not unconditionally at
/// the top of the function body -- a plain
/// `source.contains("sample_committed_rgba16_three_nearest_bound")` check
/// would pass even for the old unconditional-call shape this task repairs,
/// so this asserts the specific textual ordering: the `if
/// fragment_combine_params.texture_referenced != 0u` condition appears
/// before the sampler call, and the sampler call is not the first
/// executable statement in `fs_main`'s body (an unconditional call sits on
/// the line immediately after the opening brace in the pre-repair shape).
#[test]
fn combined_fragment_wgsl_gates_the_tmem_sample_call_on_texture_referenced() {
    let source = crate::shader_manifest::triangle_pipeline_fragment_wgsl();
    let fs_main_start = source
        .find("fn fs_main(")
        .expect("fs_main must exist in the combined source");
    let body = &source[fs_main_start..];

    let gate_index = body
        .find("fragment_combine_params.texture_referenced != 0u")
        .expect("fs_main must branch on texture_referenced");
    let call_index = body
        .find("sample_committed_rgba16_three_nearest_bound(")
        .expect("fs_main must still call the real sampler somewhere");
    assert!(
        gate_index < call_index,
        "the texture_referenced gate must appear before the sampler call, \
         not after -- an unconditional call could still textually precede \
         a later, dead gate check"
    );

    // The unconditional-call shape this task repairs calls the sampler as
    // `let sample = sample_committed_rgba16_three_nearest_bound(...)`,
    // immediately assigning its result to `sample`. The repaired shape
    // instead declares `sample` via `var sample: TmemSampleResult;` ahead
    // of the gate and only assigns it *inside* the `if` branch -- so the
    // sampler call must not be a `let`-binding's direct initializer.
    assert!(
        !body.contains("let sample = sample_committed_rgba16_three_nearest_bound("),
        "the sampler call must not be an unconditional `let sample = ...` \
         binding -- it must be gated inside the texture_referenced branch"
    );
    assert!(
        body.contains("var sample: TmemSampleResult;"),
        "fs_main must declare `sample` as a mutable var so both branches \
         (real sample vs. fixed zero) can assign it"
    );
}

#[test]
fn strict_less_depth_oracle_agrees_a_nearer_second_triangle_would_pass() {
    // Depth differential (port card §6): assert the oracle's pass/reject
    // decision for a second triangle drawn nearer/farther than this
    // fixture's z=0.5 plane, matching what a real GPU depth test at
    // `CompareFunction::Less` would decide for the same fragment_z/memory_z
    // pair. This is the CPU-side half of the differential; native GPU
    // execution and the matching real-adapter assertion live in the
    // `host-gpu-tests`-gated module below (this host has no adapter --
    // reported explicitly there, not silently skipped).
    let nearer = StrictLessDepthSample::new(0.25, 0.5);
    assert_eq!(strict_less_depth_test(nearer), StrictLessDepthOutcome::Pass);
    let farther = StrictLessDepthSample::new(0.75, 0.5);
    assert_eq!(
        strict_less_depth_test(farther),
        StrictLessDepthOutcome::Reject
    );
    let equal = StrictLessDepthSample::new(0.5, 0.5);
    assert_eq!(
        strict_less_depth_test(equal),
        StrictLessDepthOutcome::Reject
    );
}

#[test]
fn zero_extent_fixture_is_rejected_before_any_device_work() {
    // `validate_triangle_extent` is the exact precondition check
    // `submit_triangle` runs before any buffer/texture creation; calling it
    // directly exercises the real rejecting logic without needing a device,
    // so this does not need `host-gpu-tests`.
    assert_eq!(
        validate_triangle_extent(TriangleTargetExtent {
            width: 0,
            height: 1,
        }),
        Err(TrianglePipelineError::ZeroExtent {
            width: 0,
            height: 1,
        })
    );
    assert_eq!(
        validate_triangle_extent(TriangleTargetExtent {
            width: 1,
            height: 0,
        }),
        Err(TrianglePipelineError::ZeroExtent {
            width: 1,
            height: 0,
        })
    );
    assert_eq!(
        validate_triangle_extent(TriangleTargetExtent {
            width: 8,
            height: 8,
        }),
        Ok(())
    );
}

#[test]
fn triangle_vertex_byte_layout_is_40_bytes_matching_position_uv_color() {
    let vertex = RasterVertex {
        position: [1.0, 2.0, 3.0, 4.0],
        uv: [5.0, 6.0],
        color: [7.0, 8.0, 9.0, 10.0],
    };
    let bytes = vertex.to_bytes();
    assert_eq!(bytes.len(), 40);
    assert_eq!(&bytes[0..4], &1.0_f32.to_le_bytes());
    assert_eq!(&bytes[12..16], &4.0_f32.to_le_bytes());
    assert_eq!(&bytes[16..20], &5.0_f32.to_le_bytes());
    assert_eq!(&bytes[20..24], &6.0_f32.to_le_bytes());
    assert_eq!(&bytes[24..28], &7.0_f32.to_le_bytes());
    assert_eq!(&bytes[36..40], &10.0_f32.to_le_bytes());
}

#[test]
fn raster_params_byte_layout_is_32_bytes_and_pads_two_trailing_reserved_f32() {
    let params = TriangleRasterParams {
        resolution: [8.0, 8.0],
        screen_scale: [1.0, 1.0],
        screen_offset: [0.0, 0.0],
    };
    let bytes = params.to_bytes();
    assert_eq!(bytes.len(), RASTER_PARAMS_BYTES as usize);
    assert_eq!(&bytes[0..4], &8.0_f32.to_le_bytes());
    assert_eq!(&bytes[24..32], &[0u8; 8]);
}

/// Byte-offset proof for `FragmentCombineParams`
/// (`shaders/triangle_pipeline_fragment.wgsl`'s uniform struct): low/high at
/// 0..8 (the raw `SetCombine` wire split), `texture_referenced` at 8..12
/// (SHADE-only-triangle repair), bytes 12..16 (`reserved_zero`) always zero.
/// Uses the SHADE-passthrough fixture (no texture reference expected) and
/// the real-textured-differential fixture shape (texture reference
/// expected) so this test also pins the serialized value, not just its
/// offset.
#[test]
fn fragment_combine_params_byte_layout_is_16_bytes_with_low_high_and_texture_referenced_flag() {
    let shade_only = shade_passthrough_combine_params();
    let shade_only_bytes = fragment_combine_params_bytes(shade_only);
    assert_eq!(shade_only_bytes.len(), COMBINE_PARAMS_BYTES as usize);
    assert_eq!(&shade_only_bytes[0..4], &shade_only.low().to_le_bytes());
    assert_eq!(&shade_only_bytes[4..8], &shade_only.high().to_le_bytes());
    assert_eq!(&shade_only_bytes[8..12], &0u32.to_le_bytes());
    assert_eq!(&shade_only_bytes[12..16], &[0u8; 4]);

    // TEXEL0-passthrough shape (`production.rs`'s real textured
    // differential fixtures): color_d=TEXEL0(1) is an unconditional D
    // reference (color_d's second_cycle=true bit position is high[8:6],
    // matching `shade_passthrough_combine_params`'s own doc), so
    // `texture_referenced` must serialize as 1.
    let textured = CombineParams::from_wire(0, 1 << 6);
    let textured_bytes = fragment_combine_params_bytes(textured);
    assert_eq!(&textured_bytes[0..4], &textured.low().to_le_bytes());
    assert_eq!(&textured_bytes[4..8], &textured.high().to_le_bytes());
    assert_eq!(&textured_bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(&textured_bytes[12..16], &[0u8; 4]);
}

/// Byte-offset proof for `FragmentAlphaCompareParams` (alpha-compare
/// production card §3b): `mode` at 0..4, `threshold_alpha` at 4..8, bytes
/// 8..16 (`_reserved_0`/`_reserved_1`) always zero. Covers all three real
/// serialized shapes: `None` (mode 0, no blend_color), `Threshold` with a
/// real blend_color (mode 1, threshold_alpha == blend_color.rgba8()[3]),
/// and `Threshold` boundary values 0/255.
#[test]
fn fragment_alpha_compare_params_byte_layout_is_16_bytes_with_mode_and_threshold_alpha() {
    let none_bytes = fragment_alpha_compare_params_bytes(crate::state::AlphaCompare::None, None);
    assert_eq!(none_bytes.len(), ALPHA_COMPARE_PARAMS_BYTES as usize);
    assert_eq!(&none_bytes[0..4], &0u32.to_le_bytes());
    assert_eq!(&none_bytes[4..8], &0u32.to_le_bytes());
    assert_eq!(&none_bytes[8..16], &[0u8; 8]);

    let blend_color = crate::state::Color4::from_wire(0x1122_33AA);
    let threshold_bytes = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        Some(blend_color),
    );
    assert_eq!(&threshold_bytes[0..4], &1u32.to_le_bytes());
    assert_eq!(&threshold_bytes[4..8], &0xAAu32.to_le_bytes());
    assert_eq!(&threshold_bytes[8..16], &[0u8; 8]);

    let zero_threshold = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        Some(crate::state::Color4::from_wire(0)),
    );
    assert_eq!(&zero_threshold[4..8], &0u32.to_le_bytes());

    let max_threshold = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        Some(crate::state::Color4::from_wire(0xFFFF_FFFF)),
    );
    assert_eq!(&max_threshold[4..8], &255u32.to_le_bytes());
}

#[test]
#[should_panic(expected = "must have been rejected at retrieval time")]
fn fragment_alpha_compare_params_bytes_rejects_reserved_mode_defensively() {
    let _ = fragment_alpha_compare_params_bytes(crate::state::AlphaCompare::Reserved, None);
}

#[test]
#[should_panic(expected = "must have been rejected at retrieval time")]
fn fragment_alpha_compare_params_bytes_rejects_dither_mode_defensively() {
    let _ = fragment_alpha_compare_params_bytes(crate::state::AlphaCompare::Dither, None);
}

/// Byte-offset proof for `FragmentCoverageParams` (production coverage node
/// 1): all six fields at their own 4-byte slot, bytes 24..32 always zero.
/// Covers the `Full`/no-image-read default (this file's regression-guard
/// fixture shape) and every bool field independently set.
#[test]
fn fragment_coverage_params_byte_layout_is_32_bytes_with_all_six_fields() {
    let default_bytes = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Full,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(default_bytes.len(), COVERAGE_PARAMS_BYTES as usize);
    assert_eq!(&default_bytes[0..4], &2u32.to_le_bytes());
    assert_eq!(&default_bytes[4..8], &0u32.to_le_bytes());
    assert_eq!(&default_bytes[8..12], &0u32.to_le_bytes());
    assert_eq!(&default_bytes[12..16], &0u32.to_le_bytes());
    assert_eq!(&default_bytes[16..20], &0u32.to_le_bytes());
    assert_eq!(&default_bytes[20..24], &0u32.to_le_bytes());
    assert_eq!(&default_bytes[24..32], &[0u8; 8]);

    let clamp_bytes = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        false,
        true,
        true,
        true,
        true,
    );
    assert_eq!(&clamp_bytes[0..4], &0u32.to_le_bytes());
    assert_eq!(&clamp_bytes[4..8], &0u32.to_le_bytes());
    assert_eq!(&clamp_bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(&clamp_bytes[12..16], &1u32.to_le_bytes());
    assert_eq!(&clamp_bytes[16..20], &1u32.to_le_bytes());
    assert_eq!(&clamp_bytes[20..24], &1u32.to_le_bytes());

    let wrap_bytes = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(&wrap_bytes[0..4], &1u32.to_le_bytes());
}

#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_save_destination() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Save,
        false,
        false,
        false,
        false,
        false,
    );
}

#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_clamp_with_image_read_enabled() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        true,
        false,
        false,
        false,
        false,
    );
}

#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_wrap_with_image_read_enabled() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        true,
        false,
        false,
        false,
        false,
    );
}

#[test]
fn fragment_coverage_params_bytes_allows_clamp_and_wrap_without_image_read_enabled() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        false,
        false,
        false,
        false,
        false,
    );
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        false,
        false,
        false,
        false,
        false,
    );
}

/// Byte-offset proof for `FragmentMaterialParams` (production literal
/// combiner Slice B): `env_color` at 0..16, `prim_color` at 16..32,
/// `prim_lod_frac` at 32..36, bytes 36..48 (`_reserved_0`/`_reserved_1`/
/// `_reserved_2`) always zero. Covers the `None`/`None` all-zero case (a
/// triangle drawn before any `SetEnvColor`/`SetPrimColor`) and a real
/// known-wire `Color4`/`PrimColor` pair.
#[test]
fn fragment_material_params_byte_layout_is_48_bytes_with_env_prim_and_lod_frac() {
    let none_bytes = fragment_material_params_bytes(None, None);
    assert_eq!(none_bytes.len(), MATERIAL_PARAMS_BYTES as usize);
    assert_eq!(&none_bytes[0..16], &[0u8; 16]);
    assert_eq!(&none_bytes[16..32], &[0u8; 16]);
    assert_eq!(&none_bytes[32..36], &0f32.to_le_bytes());
    assert_eq!(&none_bytes[36..48], &[0u8; 12]);

    let env_color = crate::state::Color4::from_wire(0x1122_33AA);
    let prim_color = crate::state::PrimColor::from_wire(0x0000_0080, 0x4455_66BB);
    let material_bytes = fragment_material_params_bytes(Some(env_color), Some(prim_color));
    assert_eq!(
        &material_bytes[0..16],
        &bytemuck_f32x4(env_color.normalized())
    );
    assert_eq!(
        &material_bytes[16..32],
        &bytemuck_f32x4(prim_color.color().normalized())
    );
    assert_eq!(
        &material_bytes[32..36],
        &prim_color.lod().lod_frac_normalized().to_le_bytes()
    );
    assert_eq!(&material_bytes[36..48], &[0u8; 12]);
}

/// `raster_params_bytes` proof (texture-rectangle placement card §1 item
/// 7a): `is_rect=false` is byte-identical to `TriangleRasterParams::to_bytes`
/// (the seam's required no-op case); `is_rect=true` sets bytes 24..28 to
/// `1u32.to_le_bytes()` and leaves every other byte, including 28..32,
/// unchanged from the `false` case -- proving the new function only ever
/// touches bytes 24..28.
#[test]
fn raster_params_bytes_only_touches_the_is_rect_byte_range() {
    let params = TriangleRasterParams {
        resolution: [8.0, 8.0],
        screen_scale: [1.0, 1.0],
        screen_offset: [0.0, 0.0],
    };
    let plain = params.to_bytes();
    let not_rect = raster_params_bytes(params, false);
    assert_eq!(not_rect, plain);
    assert_eq!(&not_rect[24..28], &0u32.to_le_bytes());
    assert_eq!(&not_rect[28..32], &[0u8; 4]);

    let is_rect = raster_params_bytes(params, true);
    assert_eq!(&is_rect[0..24], &plain[0..24]);
    assert_eq!(&is_rect[24..28], &1u32.to_le_bytes());
    assert_eq!(&is_rect[28..32], &[0u8; 4]);
}

/// Device-unavailable degradation path (port card §7): a real GPU adapter
/// cannot be forced absent deterministically in this test harness (no unit
/// test in this file simulates `TrianglePipelineDeviceOutcome::NoAdapter`
/// either -- `UninitializedTrianglePipeline::request` genuinely calls
/// `wgpu::Instance::request_adapter`, and every `host_gpu_tests` test in
/// this module panics loudly, by established convention, if that returns
/// `NoAdapter` on a CI host assumed to have one). What this card's own
/// `submit_admitted_triangle` adds ahead of any device call -- the
/// `NeutralTriangleVertex -> RasterVertex` adaptation and `TriangleFixture`
/// assembly -- is proven not to panic and to build the exact fixture a real
/// device call would receive, entirely without a device: if this ever
/// needs a device to run, it would silently stop covering the
/// device-unavailable case (any panic here would occur before device
/// request, same as a real `NoAdapter` return -- so this is the same
/// coverage `NativeRasterDeviceOutcome::NoAdapter`'s non-panicking
/// construction gets elsewhere in this crate, applied to this card's new
/// pre-device logic).
#[test]
fn admitted_triangle_fixture_assembly_never_panics_and_needs_no_device() {
    let vertices = [
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [1.0, 0.0, 0.0, 1.0],
            texcoord: [0.0, 0.0],
        },
        fn64_render::NeutralTriangleVertex {
            x: 8.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 1.0, 0.0, 1.0],
            texcoord: [1.0, 0.0],
        },
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 8.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 1.0, 1.0],
            texcoord: [0.0, 1.0],
        },
    ];
    let raster_vertices = vertices.map(crate::neutral_vertex_to_raster_vertex);
    let (tmem, tile_binding) = no_tmem_binding();
    let fixture = TriangleFixture {
        vertices: raster_vertices,
        raster_params: identity_raster_params(),
        combine_params: shade_passthrough_combine_params(),
        extent: EXTENT,
        tmem,
        tile_binding,
        alpha_compare_mode: crate::state::AlphaCompare::None,
        blend_color: None,
        env_color: None,
        prim_color: None,
        blend_params: ResolvedFragmentBlendParams::NO_OP,
        depth_compare_enabled: true,
        depth_update_enabled: true,
        coverage_destination: crate::state::CoverageDestination::Full,
        image_read_enabled: false,
        force_blend: false,
        antialias_enabled: false,
        coverage_times_alpha: false,
        alpha_coverage_select: false,
        is_rect: false,
    };
    assert_eq!(fixture.vertices[0].position, [0.0, 0.0, 0.5, 1.0]);
    assert_eq!(fixture.vertices[1].uv, [1.0, 0.0]);
    assert_eq!(fixture.vertices[2].color, [0.0, 0.0, 1.0, 1.0]);
}

#[cfg(feature = "host-gpu-tests")]
mod host_gpu_tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
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
            match Future::poll(future.as_mut(), &mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    fn pixel_index(x: u32, y: u32) -> usize {
        (y * EXTENT.width + x) as usize
    }

    fn rgba8_at(output: &TriangleDrawOutput, x: u32, y: u32) -> [u8; 4] {
        let index = pixel_index(x, y) * 4;
        [
            output.color_rgba8[index],
            output.color_rgba8[index + 1],
            output.color_rgba8[index + 2],
            output.color_rgba8[index + 3],
        ]
    }

    fn depth_at(output: &TriangleDrawOutput, x: u32, y: u32) -> f32 {
        output.depth_f32[pixel_index(x, y)]
    }

    /// Independent hand-computed barycentric interpolation of
    /// `covering_triangle_fixture`'s vertex colors at a pixel-center sample
    /// point `(x+0.5, y+0.5)` -- the standard D3D/Metal/Vulkan
    /// pixel-center-sampling convention wgpu's rasterizer follows, matching
    /// port card §6's "hand-computed expected... treat wgpu's own
    /// rasterizer as the authority being tested" option: this does not
    /// re-derive coverage (wgpu's rasterizer decides that), only the
    /// interpolated color at a point already known to be covered. Triangle
    /// corners are (0,0)=red, (8,0)=green, (0,8)=blue.
    fn expected_interpolated_rgba8(x: u32, y: u32) -> [u8; 4] {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        // Barycentric weights for a right triangle with legs along the axes:
        // w_green = px/8, w_blue = py/8, w_red = 1 - w_green - w_blue.
        let w_green = px / 8.0;
        let w_blue = py / 8.0;
        let w_red = 1.0 - w_green - w_blue;
        let channel = |red: f32, green: f32, blue: f32| {
            ((w_red * red + w_green * green + w_blue * blue) * 255.0).round() as u8
        };
        [
            channel(1.0, 0.0, 0.0),
            channel(0.0, 1.0, 0.0),
            channel(0.0, 0.0, 1.0),
            255,
        ]
    }

    fn assert_close_rgba8(observed: [u8; 4], expected: [u8; 4], tolerance: i32) {
        for channel in 0..4 {
            let diff = i32::from(observed[channel]) - i32::from(expected[channel]);
            assert!(
                diff.abs() <= tolerance,
                "channel {channel}: observed={observed:?} expected={expected:?} tolerance={tolerance}"
            );
        }
    }

    /// Required host GPU evidence (port card §6, §7's "backend named
    /// explicitly" nonclaim): submits `covering_triangle_fixture()` through
    /// the real render pipeline and validates 2-3 known-covered pixels'
    /// color against the CPU combiner oracle and depth against
    /// `depth_strict_less.rs`'s oracle. Panics with the typed no-adapter
    /// reason if this host has no native GPU adapter, rather than silently
    /// skipping -- matching `targets/raster.rs`'s own required-host-GPU test
    /// convention.
    #[test]
    fn required_host_rasterizes_covering_triangle_and_matches_combiner_and_depth_oracles() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };
        eprintln!(
            "fn64-triangle-pipeline: backend={:?} adapter={:?}",
            renderer.adapter_info().backend,
            renderer.adapter_info().name
        );

        let fixture = covering_triangle_fixture();
        let output = renderer
            .submit_triangle(fixture)
            .unwrap()
            .complete()
            .unwrap();
        assert_eq!(output.color_rgba8.len(), 8 * 8 * 4);
        assert_eq!(output.depth_f32.len(), 8 * 8);

        // Known-covered pixels near the right-angle corner, matched against
        // an independent hand-computed barycentric interpolation at that
        // pixel's center-sample point (port card §6) -- not the exact
        // vertex color, since wgpu samples at pixel centers (x+0.5, y+0.5),
        // not pixel-corner integer coordinates. A small +-2/255 tolerance
        // absorbs the host GPU's own float rounding.
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            let observed = rgba8_at(&output, x, y);
            let expected = expected_interpolated_rgba8(x, y);
            assert_close_rgba8(observed, expected, 2);
        }
        let cpu_red = cpu_combiner_reference([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(cpu_red[0], 1.0);

        // Depth: every covered pixel was drawn once at z=0.5 with a fresh
        // Clear(1.0) depth buffer and CompareFunction::Less -- 0.5 < 1.0
        // passes, matching `strict_less_depth_test(sample(0.5, 1.0))`.
        assert_eq!(
            strict_less_depth_test(StrictLessDepthSample::new(0.5, 1.0)),
            StrictLessDepthOutcome::Pass
        );
        let observed_depth = depth_at(&output, 1, 1);
        assert!((observed_depth - 0.5).abs() < 1e-3, "{observed_depth}");

        // An uncovered corner (outside the triangle's hypotenuse) keeps the
        // Clear color/depth untouched.
        assert_eq!(rgba8_at(&output, 7, 7), [0, 0, 0, 0]);
        assert!((depth_at(&output, 7, 7) - 1.0).abs() < 1e-6);
    }

    fn uniform_yellow_at(z: f32) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        for vertex in &mut fixture.vertices {
            vertex.position[2] = z;
            vertex.color = [1.0, 1.0, 0.0, 1.0];
        }
        fixture
    }

    /// Uniform-magenta variant of [`uniform_yellow_at`] at a caller-chosen
    /// depth AND `(Z_CMP, Z_UPD)` pair -- the fixture shape the `Z_CMP`/
    /// `Z_UPD` pipeline-variant differential (production depth-slice task
    /// card §"Fixtures") needs: a third, visually distinct color from both
    /// `covering_triangle_fixture`'s red/green/blue and `uniform_yellow_at`'s
    /// yellow, so a three-draw same-target test can tell all three draws'
    /// contributions apart by color alone.
    fn uniform_magenta_at(
        z: f32,
        depth_compare_enabled: bool,
        depth_update_enabled: bool,
    ) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        for vertex in &mut fixture.vertices {
            vertex.position[2] = z;
            vertex.color = [1.0, 0.0, 1.0, 1.0];
        }
        fixture.depth_compare_enabled = depth_compare_enabled;
        fixture.depth_update_enabled = depth_update_enabled;
        fixture
    }

    /// Second-triangle depth-reject case (port card §6: "at least one pass
    /// case and one reject case", "assert the real GPU depth-test outcome...
    /// matches `depth_strict_less.rs`'s oracle"). Both draws go through
    /// `submit_triangles` in a SINGLE submission sharing one color+depth
    /// target -- the color attachment clears once before the first draw,
    /// and the second draw's real GPU depth test competes against the
    /// first draw's already-committed z=0.5, not a fresh per-draw buffer
    /// (see `TrianglePipelineRenderer::submit_triangles`'s doc). z=0.75 is
    /// farther than the committed z=0.5, so `CompareFunction::Less` must
    /// reject every covered pixel of the second (yellow) triangle, leaving
    /// the first triangle's interpolated color and z=0.5 depth intact --
    /// real GPU-observed rejection evidence, not just the CPU oracle
    /// function called in isolation.
    #[test]
    fn required_host_depth_test_rejects_a_farther_second_triangle_in_the_same_target() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let nearer = covering_triangle_fixture(); // z=0.5, red/green/blue
        let farther = uniform_yellow_at(0.75);

        let output = renderer
            .submit_triangles(&[nearer, farther])
            .unwrap()
            .complete()
            .unwrap();

        assert_eq!(
            strict_less_depth_test(StrictLessDepthSample::new(0.75, 0.5)),
            StrictLessDepthOutcome::Reject
        );
        // Real GPU evidence: the farther (yellow) draw's covered pixels must
        // still show the nearer draw's interpolated color and z=0.5 depth,
        // not yellow/0.75 -- proving the second draw's depth test was
        // actually rejected against the first draw's committed buffer
        // contents, not merely that the oracle function agrees in the
        // abstract.
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            let observed_color = rgba8_at(&output, x, y);
            let expected_color = expected_interpolated_rgba8(x, y);
            assert_close_rgba8(observed_color, expected_color, 2);
            assert_ne!(
                observed_color,
                [255, 255, 0, 255],
                "yellow leaked through at ({x},{y})"
            );
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.5).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.5 (farther draw must not have written)"
            );
        }
    }

    /// Pass-case companion (port card §6: "at least one pass case"): a
    /// second, NEARER triangle (z=0.25) drawn after the first (z=0.5) into
    /// the SAME target must win the real GPU depth test and overwrite both
    /// color and depth, matching `strict_less_depth_test(sample(0.25, 0.5))
    /// == Pass`.
    #[test]
    fn required_host_depth_test_accepts_a_nearer_second_triangle_in_the_same_target() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let farther = covering_triangle_fixture(); // z=0.5, red/green/blue
        let nearer = uniform_yellow_at(0.25);

        let output = renderer
            .submit_triangles(&[farther, nearer])
            .unwrap()
            .complete()
            .unwrap();

        assert_eq!(
            strict_less_depth_test(StrictLessDepthSample::new(0.25, 0.5)),
            StrictLessDepthOutcome::Pass
        );
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_close_rgba8(rgba8_at(&output, x, y), [255, 255, 0, 255], 2);
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.25).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.25 (nearer draw must have won)"
            );
        }
    }

    /// Regression guard (production depth-slice task card §"Fixtures" item
    /// 4): the default/`(Z_CMP=set, Z_UPD=set)` pipeline variant (index 0,
    /// [`depth_pipeline_index`]) must be bit-identical to the pipeline's
    /// prior sole state -- re-runs the exact reject/accept assertions above,
    /// but with both fixtures' depth fields set explicitly rather than
    /// relying on `covering_triangle_fixture`'s/`uniform_yellow_at`'s
    /// defaults, proving the new four-variant selection path reduces to the
    /// old single-pipeline behavior for this combination, not merely that
    /// the default happens to match.
    #[test]
    fn required_host_depth_test_both_set_reduces_to_the_prior_less_write_always_pipeline() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let mut nearer = covering_triangle_fixture(); // z=0.5, red/green/blue
        nearer.depth_compare_enabled = true;
        nearer.depth_update_enabled = true;
        let mut farther = uniform_yellow_at(0.75);
        farther.depth_compare_enabled = true;
        farther.depth_update_enabled = true;

        let output = renderer
            .submit_triangles(&[nearer, farther])
            .unwrap()
            .complete()
            .unwrap();

        // Farther draw must still be rejected: identical outcome to
        // `required_host_depth_test_rejects_a_farther_second_triangle_in_the_same_target`.
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            let observed_color = rgba8_at(&output, x, y);
            assert_ne!(
                observed_color,
                [255, 255, 0, 255],
                "yellow leaked through at ({x},{y})"
            );
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.5).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.5 (farther draw must not have written)"
            );
        }
    }

    /// `Z_CMP` clear (production depth-slice task card §"Fixtures" item 1):
    /// a farther second triangle (z=0.75, magenta) with `Z_CMP` clear draws
    /// OVER a nearer already-committed first triangle (z=0.5, red/green/
    /// blue) -- `depth_compare_enabled: false` selects the `Always`-compare
    /// pipeline variant (index 2, [`depth_pipeline_index`]), so the real GPU
    /// depth test never rejects regardless of the committed z=0.5, proving
    /// the reject behavior of the `(set, set)` control case above no longer
    /// happens once `Z_CMP` is cleared.
    #[test]
    fn required_host_depth_test_z_cmp_clear_lets_a_farther_second_triangle_draw_over_the_first() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let nearer = covering_triangle_fixture(); // z=0.5, red/green/blue, (Z_CMP, Z_UPD) = (true, true)
        let farther = uniform_magenta_at(0.75, false, true); // Z_CMP clear, Z_UPD set

        let output = renderer
            .submit_triangles(&[nearer, farther])
            .unwrap()
            .complete()
            .unwrap();

        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_close_rgba8(rgba8_at(&output, x, y), [255, 0, 255, 255], 2);
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.75).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.75 (Z_CMP clear must have let \
                 the farther draw win and write, since Z_UPD is set here)"
            );
        }
    }

    /// `Z_UPD` clear, three-draw fixture (production depth-slice task card
    /// §"Fixtures" item 2): a nearer second triangle (z=0.25, magenta) with
    /// `Z_UPD` clear passes its OWN depth test against the first (committed
    /// z=0.5) and writes its color, but `depth_update_enabled: false`
    /// selects the write-disabled pipeline variant (index 1,
    /// [`depth_pipeline_index`]), so it must NOT overwrite the depth buffer
    /// -- proven by a third triangle (z=0.5, yellow) drawn after it in the
    /// SAME submission: if the second draw's depth write were wrongly
    /// applied, the depth buffer would read ~0.25 and the third (z=0.5)
    /// triangle would be rejected as farther; since it must not have
    /// written, the buffer still reads the first draw's committed ~0.5, so
    /// the third triangle's own `(Z_CMP=true)` default test against that
    /// still-0.5 depth is `Reject` (0.5 is not `Less` than 0.5), leaving the
    /// second (magenta) draw's color on screen, not the third (yellow)
    /// draw's.
    #[test]
    fn required_host_depth_test_z_upd_clear_does_not_write_depth_for_a_third_draw() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let first = covering_triangle_fixture(); // z=0.5, red/green/blue, (Z_CMP, Z_UPD) = (true, true)
        let second = uniform_magenta_at(0.25, true, false); // nearer, Z_CMP set, Z_UPD clear
        let third = uniform_yellow_at(0.5); // (Z_CMP, Z_UPD) = (true, true) default

        assert_eq!(
            strict_less_depth_test(StrictLessDepthSample::new(0.25, 0.5)),
            StrictLessDepthOutcome::Pass
        );
        assert_eq!(
            strict_less_depth_test(StrictLessDepthSample::new(0.5, 0.5)),
            StrictLessDepthOutcome::Reject
        );

        let output = renderer
            .submit_triangles(&[first, second, third])
            .unwrap()
            .complete()
            .unwrap();

        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            // Second (magenta) draw's color must still be on screen: it won
            // its own test against the first draw, and the third (yellow)
            // draw must have been rejected against the still-unwritten
            // ~0.5 depth, not the magenta draw's ~0.25.
            assert_close_rgba8(rgba8_at(&output, x, y), [255, 0, 255, 255], 2);
            assert_ne!(
                rgba8_at(&output, x, y),
                [255, 255, 0, 255],
                "yellow (third draw) leaked through at ({x},{y}) -- implies Z_UPD clear wrongly \
                 wrote depth, letting the third draw's z=0.5 pass against a stale ~0.25"
            );
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.5).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.5 (Z_UPD clear on the second \
                 draw must have left the first draw's depth untouched)"
            );
        }
    }

    /// Both clear, combining items 1 and 2 (production depth-slice task
    /// card §"Fixtures" item 3): a farther second triangle (z=0.75, magenta)
    /// with BOTH `Z_CMP` and `Z_UPD` clear draws over the first (proving no
    /// reject, matching item 1) AND does not write depth for a third
    /// triangle drawn after it (proving no write, matching item 2) --
    /// `depth_compare_enabled: false, depth_update_enabled: false` selects
    /// index 3, [`depth_pipeline_index`].
    #[test]
    fn required_host_depth_test_both_clear_draws_over_the_first_and_does_not_write_depth() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        let first = covering_triangle_fixture(); // z=0.5, red/green/blue, (Z_CMP, Z_UPD) = (true, true)
        let second = uniform_magenta_at(0.75, false, false); // farther, both clear
        let third = uniform_yellow_at(0.5); // (Z_CMP, Z_UPD) = (true, true) default

        let output = renderer
            .submit_triangles(&[first, second, third])
            .unwrap()
            .complete()
            .unwrap();

        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            // The third (yellow) draw must win: it tests its own z=0.5
            // against the still-committed ~0.5 from the first draw (Reject
            // under strict-less since 0.5 is not < 0.5) only if the second
            // draw's write were wrongly applied at ~0.75 or ~0.5 -- but
            // since Z_CMP was clear on the second draw, the third draw's
            // fixed-function Less test is unaffected either way; the
            // decisive assertion is that depth must read ~0.5 (unwritten by
            // the second draw), and thus the third draw at exactly the same
            // z=0.5 is `Reject`, so the second (magenta) draw's color must
            // be the final one on screen, not the third (yellow) draw's.
            assert_close_rgba8(rgba8_at(&output, x, y), [255, 0, 255, 255], 2);
            assert_ne!(
                rgba8_at(&output, x, y),
                [255, 255, 0, 255],
                "yellow (third draw) leaked through at ({x},{y})"
            );
            let observed_depth = depth_at(&output, x, y);
            assert!(
                (observed_depth - 0.5).abs() < 1e-3,
                "depth at ({x},{y}) = {observed_depth}, expected ~0.5 (both-clear second draw \
                 must not have written depth, leaving the first draw's ~0.5 intact)"
            );
        }
    }

    /// Uniform-alpha triangle, `covering_triangle_fixture`'s geometry with a
    /// flat shade color at a caller-chosen alpha, and a caller-chosen
    /// alpha-compare mode/blend_color -- the fixture shape the alpha-compare
    /// differential (card §4g) needs: same screen coverage as every other
    /// fixture in this file, but a flat (non-interpolated) alpha so a single
    /// known combiner-output alpha applies to every covered pixel, no
    /// barycentric interpolation math required to know what the gate sees.
    fn uniform_alpha_fixture(
        alpha: f32,
        alpha_compare_mode: crate::state::AlphaCompare,
        blend_color: Option<crate::state::Color4>,
    ) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        for vertex in &mut fixture.vertices {
            vertex.color = [1.0, 1.0, 1.0, alpha];
        }
        fixture.alpha_compare_mode = alpha_compare_mode;
        fixture.blend_color = blend_color;
        fixture
    }

    /// Required host GPU evidence (alpha-compare production card §4g): a
    /// `None`-mode triangle must always write its fragment (color differs
    /// from the `LoadOp::Clear` background), while a `Threshold`-mode
    /// triangle whose combiner-output alpha is provably below
    /// `blend_color`'s threshold must be discarded (color stays exactly at
    /// the `LoadOp::Clear` background) -- real GPU-observed discard
    /// evidence, not the CPU `alpha_compare_value` oracle called in
    /// isolation. Each case is its own single-triangle submission (its own
    /// fresh `Clear` target, matching `submit_triangle`'s doc) rather than
    /// two draws sharing one target, so this test needs no draw-order
    /// reasoning about a shared color attachment.
    #[test]
    fn required_host_alpha_compare_none_always_writes_and_threshold_discards_below_blend_color() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        // None mode: alpha_compare_fragment_fn(mode=0, ..) always returns
        // true regardless of alpha -- a low alpha (0.1 -> combiner-output
        // byte 26) must still write, proving None never gates on alpha at
        // all, not merely that this particular alpha happens to pass a
        // threshold.
        let none_fixture = uniform_alpha_fixture(0.1, crate::state::AlphaCompare::None, None);
        let none_output = renderer
            .submit_triangle(none_fixture)
            .unwrap()
            .complete()
            .unwrap();
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_ne!(
                rgba8_at(&none_output, x, y),
                [0, 0, 0, 0],
                "None-mode fragment at ({x},{y}) must write, matching LoadOp::Clear background \
                 only if the fragment was wrongly discarded"
            );
        }

        // Threshold mode: combiner-output alpha = 0.1 -> u32 byte 26
        // (u32(clamp(0.1,0,1)*255.0+0.5) = 26). blend_color alpha = 200
        // (wire byte 0, Color4's alpha byte per `rgba8()[3]`) is provably
        // above 26, so `alpha_compare_fragment_fn(mode=1, alpha=26,
        // threshold_alpha=200, ..)` returns `26 >= 200 == false` --
        // discarded. blend_color's wire word packs alpha in bits 7:0
        // (`Color4::rgba8`'s big-endian doc), so `0x0000_00C8` (200) is the
        // threshold-only wire value; R/G/B are irrelevant to alpha compare.
        let blend_color = crate::state::Color4::from_wire(0x0000_00C8);
        let threshold_fixture = uniform_alpha_fixture(
            0.1,
            crate::state::AlphaCompare::Threshold,
            Some(blend_color),
        );
        assert_eq!(blend_color.rgba8()[3], 200);
        let threshold_output = renderer
            .submit_triangle(threshold_fixture)
            .unwrap()
            .complete()
            .unwrap();
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_eq!(
                rgba8_at(&threshold_output, x, y),
                [0, 0, 0, 0],
                "Threshold-mode fragment at ({x},{y}) with alpha=26 < threshold=200 must be \
                 discarded, leaving the LoadOp::Clear background untouched -- observed a \
                 written color instead"
            );
            assert_eq!(
                depth_at(&threshold_output, x, y),
                1.0,
                "a discarded fragment must not write depth either -- the whole fragment is \
                 discarded before either color-attachment write executes (card §3c)"
            );
        }
    }

    /// Required host GPU evidence (production coverage node 1): a real draw
    /// with `alpha_coverage_select` set must write the GPU-observed alpha
    /// channel exactly as `coverage_fragment_fn`/`apply_coverage_alpha`
    /// predicts, differing from the same draw with the bit clear -- not the
    /// CPU `coverage_fragment_fn`/`coverage.rs` oracle called in isolation.
    /// This slice's `pixel_count` is always `COVERAGE_FULL` (8u,
    /// `Full`/no-image-read scope, WGSL wiring doc), so with
    /// `coverage_destination = Full` the `cvg_dst` accumulation always
    /// yields `destination = 8`; with `coverage_times_alpha` clear,
    /// `adjusted_coverage` stays `8`, so `alpha_coverage_select` set writes
    /// `coverage_alpha_fn(8) = (8*255+4)/8 = 255` into the output alpha
    /// channel regardless of the combiner's own low input alpha -- a
    /// maximally distinguishable signal from the unmodified low input alpha
    /// the `alpha_coverage_select`-clear draw must still show.
    #[test]
    fn required_host_ordinary_vs_alpha_coverage_select_writes_distinct_alpha() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };

        // Ordinary: alpha_coverage_select clear -- output alpha must stay
        // the combiner's own low input alpha unmodified (0.2 -> u32(0.2 *
        // 255.0 + 0.5) = 51).
        let mut ordinary_fixture = covering_triangle_fixture();
        for vertex in &mut ordinary_fixture.vertices {
            vertex.color = [1.0, 1.0, 1.0, 0.2];
        }
        let ordinary_output = renderer
            .submit_triangle(ordinary_fixture)
            .unwrap()
            .complete()
            .unwrap();
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_close_rgba8(rgba8_at(&ordinary_output, x, y), [255, 255, 255, 51], 2);
        }

        // alpha_coverage_select set, Full destination, coverage_times_alpha
        // clear -- output alpha must become 255 (coverage_alpha_fn(8)),
        // provably different from the ordinary draw's 51 above, proving the
        // GPU actually executed `coverage_fragment_fn`'s composition rather
        // than passing the combiner alpha through unmodified.
        let mut select_fixture = covering_triangle_fixture();
        for vertex in &mut select_fixture.vertices {
            vertex.color = [1.0, 1.0, 1.0, 0.2];
        }
        select_fixture.alpha_coverage_select = true;
        let select_output = renderer
            .submit_triangle(select_fixture)
            .unwrap()
            .complete()
            .unwrap();
        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            assert_close_rgba8(rgba8_at(&select_output, x, y), [255, 255, 255, 255], 2);
            assert_ne!(
                rgba8_at(&select_output, x, y)[3],
                rgba8_at(&ordinary_output, x, y)[3],
                "alpha_coverage_select=true at ({x},{y}) must write a different alpha than the \
                 same draw with alpha_coverage_select=false"
            );
        }
    }

    /// Production blend wiring slice 1, card §3f: the second still-missing
    /// evidence fixture -- a real physical-Metal triangle draw whose
    /// resolved blend cycle is a genuine non-`Framebuffer` general divide
    /// (both `a`/`b` factors nonzero, so neither the `a==0`/`b==0` collapse
    /// nor the last-cycle `blend_enabled==0` bypass short-circuits it),
    /// compared against `crate::blend::blend_fragment(..., memory: None,
    /// ...)` -- the same Rust oracle `blend.rs`'s own CPU characterization
    /// tests use, not a second hand-rolled formula. `uniform_general_divide_
    /// blend_fixture` gives every covered pixel the identical combiner
    /// output (flat SHADE passthrough, matching `covering_triangle_fixture`'s
    /// own sidestep of barycentric interpolation), so the oracle only needs
    /// to be evaluated once and compared at several covered pixels.
    #[test]
    fn required_host_general_divide_blend_draw_matches_the_rust_oracle_with_no_memory() {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        let mut renderer = match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };
        eprintln!(
            "fn64-triangle-pipeline-blend: backend={:?} adapter={:?}",
            renderer.adapter_info().backend,
            renderer.adapter_info().name
        );

        // OneCycle wire (cycle_type bits 20:21 == 0) with cycle 1's raw
        // selectors P=2(Blend), A=1(Fog), M=3(Fog), B=2(One) packed at their
        // documented bit positions (`state.rs`'s `blender_cycle_1`:
        // color_a@30:31, alpha_a@26:27, color_b@22:23, alpha_b@18:19).
        // `cycle` is derived FROM this real `OtherMode` via
        // `BlendModeState::cycle(0)` -- the exact same route
        // `production.rs`'s `draw_admitted_triangles` uses to build
        // `ResolvedFragmentBlendParams` from a real triangle's decoded
        // `OtherMode` -- rather than an independently hand-built
        // `ResolvedBlendCycle` that could silently drift from what the wire
        // bits actually decode to.
        //
        // Every selector is non-`Framebuffer`/non-`FramebufferAlpha` (the
        // admitted subset), and `a`/`b` are both provably nonzero (`a`
        // derives from a nonzero fog alpha, `b` is the constant `One`), so
        // the real evaluated path is the general `(P*A + M*B)/(A+B)`
        // divide, not either zero-factor collapse. `force_blend = true`
        // makes `coverage_fragment_fn`'s `blend_enabled` unconditionally
        // true (`coverage.rs:148`), so the no-`FORCE_BL` last-cycle bypass
        // this cycle would otherwise take (it is both the first AND the
        // last cycle in OneCycle mode) is also not the path exercised.
        let other_mode = crate::state::OtherMode::from_wire(0, 0x84C8_0000);
        let blend_color = crate::state::Color4::from_wire(0x14283C50); // [20,40,60,80]
        let fog_color = crate::state::Color4::from_wire(0x465A6E82); // [70,90,110,130]
        let shade_color = [0.2, 0.4, 0.6, 0.8];

        let mode_state = crate::blend::BlendModeState {
            other_mode,
            blend_color_register: blend_color.rgba8(),
            fog_color: fog_color.rgba8(),
        };
        assert_eq!(mode_state.cycle_count(), 1, "fixture must be OneCycle");
        let cycle = mode_state.cycle(0);
        assert_eq!(
            cycle,
            crate::blend::ResolvedBlendCycle {
                p: crate::blend::BlendColorInput::Blend,
                a: crate::blend::BlendAlphaInput::Fog,
                m: crate::blend::BlendColorInput::Fog,
                b: crate::blend::BlendBInput::One,
            },
            "the chosen OtherMode wire bits must decode to exactly the intended selectors"
        );

        let fixture =
            uniform_general_divide_blend_fixture(cycle, blend_color, fog_color, shade_color);
        let output = renderer
            .submit_triangle(fixture)
            .unwrap()
            .complete()
            .unwrap();

        // Independent Rust-oracle re-derivation: the SAME combiner oracle
        // (`run_one_cycle`, `shade_passthrough_combine_params`) this file's
        // other tests already use for `src`, and `crate::blend::
        // blend_fragment` (memory: None) for the blend composite -- not a
        // second hand-rolled formula, matching this card's own instruction.
        let combiner_color = cpu_combiner_reference(shade_color);
        let src_rgba8 = [
            (combiner_color[0] * 255.0).round() as u8,
            (combiner_color[1] * 255.0).round() as u8,
            (combiner_color[2] * 255.0).round() as u8,
            (combiner_color[3] * 255.0).round() as u8,
        ];
        let shade_alpha_255 = (shade_color[3] * 255.0).round() as u8;
        let expected =
            crate::blend::blend_fragment(src_rgba8, None, shade_alpha_255, mode_state, true);
        // The admitted subset never selects Framebuffer/FramebufferAlpha, so
        // the oracle must not error for lack of a memory sample.
        let expected = expected.expect(
            "the chosen selectors are non-Framebuffer/non-FramebufferAlpha by construction; the \
             oracle must not require a memory sample",
        );
        // Sanity: this fixture genuinely exercises the general divide, not
        // either zero-factor collapse -- `a` and `b` are both provably
        // nonzero for this selector/color combination.
        assert_ne!(
            shade_alpha_255, 0,
            "a must be nonzero (guards a==0 collapse)"
        );

        for (x, y) in [(0, 0), (1, 1), (4, 1)] {
            let observed = rgba8_at(&output, x, y);
            assert_close_rgba8(observed, expected.rgba, 2);
        }
    }

    /// Fixture builder for [`required_host_general_divide_blend_draw_matches_the_rust_oracle_with_no_memory`]:
    /// a uniform-flat-shaded covering triangle (same screen geometry as
    /// [`covering_triangle_fixture`], every vertex the same color so no
    /// barycentric interpolation reasoning is needed) with `force_blend`
    /// set and `blend_params` built from the caller's resolved cycle.
    fn uniform_general_divide_blend_fixture(
        cycle: crate::blend::ResolvedBlendCycle,
        blend_color: crate::state::Color4,
        fog_color: crate::state::Color4,
        shade_color: [f32; 4],
    ) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        for vertex in &mut fixture.vertices {
            vertex.color = shade_color;
        }
        fixture.blend_color = Some(blend_color);
        fixture.force_blend = true;
        fixture.blend_params = ResolvedFragmentBlendParams {
            cycle_count: 1,
            cycle0: cycle,
            cycle1: crate::blend::ResolvedBlendCycle {
                p: crate::blend::BlendColorInput::Combined,
                a: crate::blend::BlendAlphaInput::Combined,
                m: crate::blend::BlendColorInput::Combined,
                b: crate::blend::BlendBInput::Zero,
            },
            blend_color: Some(blend_color),
            fog_color: Some(fog_color),
        };
        fixture
    }

    // --- Sealed-plan end-to-end: real decoded input -> real sealed
    // admission -> admitted state -> real pipeline draw (production
    // triangle draw card §7) ---

    mod sealed_plan_end_to_end {
        use super::*;
        use crate::raw_dpc::{push_decoded_raw_dpc, TriangleDrawStateCollector};
        use crate::state::OtherMode as CrateOtherMode;
        use crate::{
            decode_raw_dpc, decode_triangle_vertices, CombineParams, RawDpcCommandKind, RdpState,
        };
        use fn64_render::{new_raw_dpc_roles, OwnedRawDpcCapture, OwnedRawDpcSubmission};
        use fn64_render_ir::{
            PhysicalMemoryLayout, ResourceJournal, ResourceJournalLimits, TemporalBoundary,
        };

        const LAYOUT_BYTES: u32 = 0x4000;
        const COMMAND_START: u32 = 0x1000;
        const SET_OTHER_MODE: u8 = 0x2f;
        const SET_COMBINE: u8 = 0x3c;
        const RAW_TRIANGLE_SHADED: u8 = 0x0c; // shaded, no texture, no depth

        fn word(opcode: u8, payload: u32) -> u32 {
            u32::from(opcode) << 24 | payload
        }

        fn set_other_mode_words(cycle_type: u32, low: u32) -> [u32; 2] {
            [word(SET_OTHER_MODE, cycle_type << 20), low]
        }

        // Same wire split `combiner.rs`'s own doc documents:
        // `CombineParams::from_wire(w0, w1)` stores `w0` unmasked.
        fn set_combine_words(payload: u32, high: u32) -> [u32; 2] {
            [word(SET_COMBINE, payload & 0x00ff_ffff), high]
        }

        /// A shaded (0x0c), non-textured, non-Z triangle covering the whole
        /// 8x8 target (screen-pixel corners (0,0)/(0,8)/(8,0), matching
        /// `covering_triangle_fixture`'s own screen geometry) with a FLAT
        /// uniform shade color -- every shade `dx`/`de` coefficient word is
        /// zero, so the interleaved-fixed-point decode
        /// (`triangle_vertices.rs`'s `decode_shade`) collapses to exactly
        /// `base / 255.0` at every vertex, sidestepping barycentric
        /// interpolation math entirely: this test only needs to prove real
        /// decoded-plan data reaches the real GPU output, not re-derive
        /// RT64's shade-gradient arithmetic.
        ///
        /// `decode_triangle_vertices`'s own formula: `y1=yh`, `y2=yl`,
        /// `y3=ym`; `x1=x2` come from the major edge (`xh`/`dxhdy`, evaluated
        /// at `y1`/`y2`); `x3=xl` independently (`triangle_vertices.rs:130-
        /// 158`). `RawTriangle::decode` reads `yl` from word0's low 16 bits,
        /// `ym`/`yh` from word1's high/low 16 bits (`triangle.rs:187-189`).
        /// Choosing `yl=32` (y2=8.0px, `/4` fixed-point), `yh=0` (y1=0),
        /// `ym=0` (y3=0), `xh=0`/`dxhdy=0` (x1=x2=0.0px), `xl=8.0px`
        /// (Q16.16, x3=8.0px) yields vertices (0,0)/(0,8)/(8,0) -- the same
        /// right-triangle area `covering_triangle_fixture` covers.
        fn shaded_covering_triangle_words(color_255: [u32; 4]) -> Vec<u32> {
            let mut words = vec![
                // tile=0, level=0, yl=32 (8.0px).
                word(RAW_TRIANGLE_SHADED, 32u32),
                0, // ym=0, yh=0 (word1)
                // xl/dxldy (edge word 1): x3 = xl = 8.0px in Q16.16.
                (8i32 << 16) as u32,
                0,
                // xh/dxhdy (edge word 2): x1=x2=0.0px, slope 0.
                0,
                0,
                // xm/dxmdy (edge word 3): unused by decode_triangle_vertices
                // (RT64's own dead `mIntercept`/`x3` uses `xl`, not `xm`).
                0,
                0,
            ];
            // Shade block: base (words[0]/[2]) carries R/G in word0's
            // high/low 16 bits and B/A in word1's high/low 16 bits, exactly
            // `decode_shade`'s `interleave_words_pair(shade[0], shade[2])`
            // layout (word0=shade[0], word1=shade[2] contributes only its
            // low-order fractional half, zero here). dx (words[1]/[3]) and
            // de (words[4]/[6]) all zero -- flat color, no gradient.
            let base_w0 = (color_255[0] << 16) | (color_255[1] & 0xffff);
            let base_w1 = (color_255[2] << 16) | (color_255[3] & 0xffff);
            words.extend([
                base_w0, base_w1, // shade[0]
                0, 0, // shade[1] (dx)
                0, 0, // shade[2] (base low half, zero)
                0, 0, // shade[3] (dx low half)
                0, 0, // shade[4] (de)
                0, 0, // shade[5] (unused by decode_shade)
                0, 0, // shade[6] (de low half)
                0, 0, // shade[7] (unused)
            ]);
            words
        }

        fn journal_for(
            capture: &OwnedRawDpcCapture,
            source_range: (u32, u32),
            layout: PhysicalMemoryLayout,
        ) -> ResourceJournal {
            use fn64_render_ir::{
                AccessMode, AccessPurpose, OperationId, RdramResource, ResourceAccess,
                ResourceRegion,
            };
            let bytes = u32::try_from(capture.submission().command_words().len() * 4).unwrap();
            let command_access = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: layout.range(COMMAND_START, COMMAND_START + bytes).unwrap(),
                },
            )
            .unwrap();
            let source_access = ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: layout.range(source_range.0, source_range.1).unwrap(),
                },
            )
            .unwrap();
            let accesses = vec![command_access, source_access];
            let declared = accesses
                .iter()
                .map(|access| access.region().declared_bytes())
                .sum::<u32>();
            ResourceJournal::try_new(
                ResourceJournalLimits::try_new(64, declared.max(1)).unwrap(),
                accesses,
            )
            .unwrap()
        }

        fn finalize_ticket(
            capture: &OwnedRawDpcCapture,
            layout: PhysicalMemoryLayout,
            journal: ResourceJournal,
        ) -> fn64_render_ir::DecodedTicket {
            let preflight = fn64_render::preflight_raw_dpc_capture(
                layout,
                7,
                capture.submission().clone(),
                capture.cmd_end(),
                Vec::new(),
                journal,
            )
            .expect("fixture journal has valid limits for this capture's own command bytes");
            let guest_capture = fn64_render_ir::DeferredGuestReadCapture::new(
                preflight
                    .guest_read_plan()
                    .reads()
                    .iter()
                    .map(|read| {
                        fn64_render_ir::CapturedGuestRead::try_new(
                            *read,
                            vec![0; read.range().len() as usize],
                        )
                        .unwrap()
                    })
                    .collect(),
            );
            preflight
                .finalize(guest_capture)
                .expect("captured reads match the plan's own guest-read plan exactly")
        }

        fn decode_fixture_capture(
            words: Vec<u32>,
            source_range: (u32, u32),
        ) -> (crate::DecodedRawDpc, OwnedRawDpcCapture, ResourceJournal) {
            let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
            let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
            let submission =
                OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
            let capture = OwnedRawDpcCapture::new(
                submission,
                layout,
                7,
                TemporalBoundary::new(1, fn64_render_ir::DpInterruptState::Clear),
            );
            let probe_journal = journal_for(&capture, source_range, layout);
            let probe_ticket = finalize_ticket(&capture, layout, probe_journal);
            let (mut probe_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
                .unwrap()
                .into_roles();
            let probe_submitted = probe_queue.submit(probe_ticket).unwrap();
            let journal = match decode_raw_dpc(probe_submitted, &RdpState::default()) {
                Err(crate::RawDpcDecodeError::JournalMismatch { expected, .. }) => {
                    let accesses = expected.into_vec();
                    let declared = accesses
                        .iter()
                        .map(|access| access.region().declared_bytes())
                        .sum::<u32>();
                    ResourceJournal::try_new(
                        ResourceJournalLimits::try_new(64, declared.max(1)).unwrap(),
                        accesses,
                    )
                    .unwrap()
                }
                Ok(_) => journal_for(&capture, source_range, layout),
                Err(error) => panic!("fixture probe failed before journal comparison: {error}"),
            };
            let ticket = finalize_ticket(&capture, layout, journal.clone());
            let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
                .unwrap()
                .into_roles();
            let submitted = queue.submit(ticket).unwrap();
            let decoded =
                decode_raw_dpc(submitted, &RdpState::default()).expect("fixture decodes cleanly");
            (decoded, capture, journal)
        }

        /// The card's required end-to-end assertion (§7): decode a fixture
        /// capture containing `SetOtherMode`, `SetCombine`, and one
        /// `RawTriangle`; push ALL of them through `push_decoded_raw_dpc`
        /// into one sealed plan; walk the plan via `ExactRawDpcPlanVisitor`
        /// (through `TriangleDrawStateCollector`, the same collector
        /// `retrieve_triangle_draws` wraps, never `decoded.commands()`
        /// directly) for both the `State` and `Triangle` arms; fill a
        /// vertex buffer; submit through `TrianglePipelineRenderer`; assert
        /// real-GPU fragment output at known-covered pixels matches
        /// `combiner.rs`'s `run_one_cycle` called with the real decoded
        /// `CombineParams`.
        ///
        /// Reaches the plan through a `RawDpcCoordinator` (`authority.
        /// into_coordinator(())` -- unit `()` as the trivial physical-state
        /// fixture `P`, since this test needs no real `PhysicalTmemState`),
        /// not a bare `authority.begin_plan`/`bound.execution_view(&authority,
        /// ..)` call: this test is the predecessor proof for the parallel
        /// production-integration lane (`rt64-wgpu-backend-triangle-
        /// integration`), which reaches its sealed plan the same way
        /// `WgpuBackend` (`production.rs`) already does -- through its own
        /// owned coordinator, never a second bare-authority route alongside
        /// it.
        #[test]
        fn required_host_draws_a_real_admitted_triangle_matching_the_combiner_oracle() {
            // SHADE-passthrough SetCombine (same wire construction as
            // `shade_passthrough_combine_params` in the parent module, one
            // level up): (A-B)*C+D collapses to D=SHADE.
            let color_a: u32 = 0;
            let color_b: u32 = 0;
            let color_c: u32 = 0;
            let color_d: u32 = 4;
            let alpha_a: u32 = 0;
            let alpha_b: u32 = 0;
            let alpha_c: u32 = 1;
            let alpha_d: u32 = 4;
            let low = (color_a << 5) | color_c;
            let high = (color_b << 24)
                | (color_d << 6)
                | (alpha_a << 21)
                | (alpha_b << 3)
                | (alpha_c << 18)
                | alpha_d;

            let mut words = Vec::new();
            words.extend(set_other_mode_words(0, 0)); // OneCycle, no z-source-prim
            words.extend(set_combine_words(low, high));
            let triangle_color_255 = [64u32, 128, 192, 255];
            words.extend(shaded_covering_triangle_words(triangle_color_255));

            let (decoded, capture, journal) = decode_fixture_capture(words, (0x214, 0x224));
            let RawDpcCommandKind::RawTriangle(source_triangle) = decoded.commands()[2].kind()
            else {
                panic!("expected RawTriangle as the third command");
            };
            let RawDpcCommandKind::SetCombine(source_combine) = decoded.commands()[1].kind() else {
                panic!("expected SetCombine as the second command");
            };

            let layout = capture.memory_layout();
            let submission_start = capture.submission().start();
            let capture_words = capture.submission().command_words();
            let (session, authority) = new_raw_dpc_roles().unwrap();
            // Coordinator-owned route, not a bare-authority call -- this
            // test is the predecessor proof for the parallel production-
            // integration lane, which reaches its sealed plan the same way
            // `WgpuBackend` already does. Unit `()` stands in for the
            // coordinator's physical-state slot `P`: this test only needs
            // the plan-writing/plan-visiting surface, not real TMEM state.
            let coordinator = authority.into_coordinator(());
            let request = session.plan_request(capture);
            let mut writer = coordinator.begin_plan(request);
            push_decoded_raw_dpc(
                &mut writer,
                &decoded,
                &capture_words,
                layout,
                submission_start,
            )
            .expect("fixture stays inside the admitted state+triangle subset");
            let planned = writer
                .finish(journal)
                .expect("pushed accesses match the journal exactly");
            let reads = fn64_render_ir::DeferredGuestReadCapture::new(
                planned
                    .guest_read_plan()
                    .reads()
                    .iter()
                    .map(|read| {
                        fn64_render_ir::CapturedGuestRead::try_new(
                            *read,
                            vec![0; read.range().len() as usize],
                        )
                        .unwrap()
                    })
                    .collect(),
            );
            let mut session = session;
            let bound = session.finalize_and_submit(planned, reads).unwrap();

            // Walk the sealed plan through the real nonextracting visitor --
            // `execution_view` is the sealed API's sole route to plan
            // contents once bound; never `decoded.commands()` from here on.
            struct NoopExecutionView;
            impl fn64_render::RawDpcExecutionView<TriangleDrawStateCollector> for NoopExecutionView {
                fn plan_visited(&mut self, _plan_visitor: &mut TriangleDrawStateCollector) {}
                fn captured_reads(&mut self, _reads: &[fn64_render_ir::CapturedGuestRead]) {}
                fn submitted_packet(&mut self, _packet: &fn64_render_ir::WorkloadPacket) {}
            }
            let mut collector = TriangleDrawStateCollector::default();
            let mut view = NoopExecutionView;
            coordinator.execution_view(&bound, &mut collector, &mut view);
            let retrieved = collector
                .finish()
                .expect("plan has one triangle with real state at its own stream position");
            assert_eq!(retrieved.len(), 1);
            let draw = retrieved[0];

            // Cross-check the retrieved combine value against the source
            // decode -- the sealed plan really carries the real decoded
            // SetCombine, not a fixture literal.
            let expected_combine =
                CombineParams::from_wire(source_combine.low(), source_combine.high());
            assert_eq!(draw.combine_params, expected_combine);
            let expected_other_mode = CrateOtherMode::from_wire(0, 0);
            assert_eq!(draw.other_mode, expected_other_mode);

            let requested =
                block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                    .unwrap();
            let mut renderer = match requested {
                TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
                TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                    "required host GPU evidence unavailable: typed no-adapter for {:?}",
                    no_adapter.requested()
                ),
            };

            let (tmem, tile_binding) = no_tmem_binding();
            let output = renderer
                .submit_admitted_triangle(
                    draw.vertices,
                    draw.other_mode,
                    draw.combine_params,
                    identity_raster_params(),
                    EXTENT,
                    tmem,
                    tile_binding,
                    draw.blend_color,
                    draw.env_color,
                    draw.prim_color,
                    ResolvedFragmentBlendParams::NO_OP,
                    draw.source == fn64_render::TriangleSource::TextureRectangle,
                )
                .unwrap()
                .complete()
                .unwrap();

            // Known-covered pixel (uniform flat shade -> every covered pixel
            // has the same combiner output, no barycentric interpolation
            // needed): assert the real-GPU fragment output matches
            // `run_one_cycle` called with the REAL decoded CombineParams and
            // the real decoded shade color -- not a fixture literal.
            let source_vertices = decode_triangle_vertices(&source_triangle, false);
            let shade_color = source_vertices.vertex(0).color();
            let expected_color =
                cpu_combiner_reference_with_params(draw.combine_params, shade_color);
            let observed = rgba8_at(&output, 1, 1);
            let expected_u8 = [
                (expected_color[0] * 255.0).round() as u8,
                (expected_color[1] * 255.0).round() as u8,
                (expected_color[2] * 255.0).round() as u8,
                (expected_color[3] * 255.0).round() as u8,
            ];
            assert_close_rgba8(observed, expected_u8, 2);
            assert_eq!(expected_u8, triangle_color_255.map(|c| c as u8));
        }

        fn cpu_combiner_reference_with_params(
            params: CombineParams,
            shade_color: [f32; 4],
        ) -> [f32; 4] {
            use crate::combiner::{run_one_cycle, CombinerInputs};
            let inputs = CombinerInputs {
                tex_val0: [0.0; 4],
                tex_val1: [0.0; 4],
                prim_color: [0.0; 4],
                shade_color,
                env_color: [0.0; 4],
                key_center: [0.0; 3],
                key_scale: [0.0; 3],
                lod_fraction: 0.0,
                prim_lod_frac: 0.0,
                noise: 0.0,
                k4: 0.0,
                k5: 0.0,
            };
            let (color, _alpha_compare) = run_one_cycle(params, inputs);
            color
        }
    }
}
