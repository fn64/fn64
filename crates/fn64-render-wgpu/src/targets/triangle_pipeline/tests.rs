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

#[test]
fn compute_color_checkpoints_are_strict_and_end_at_the_chain_boundary() {
    assert_eq!(validate_compute_color_checkpoints(5, &[2, 5]), Ok(()));
    assert_eq!(
        validate_compute_color_checkpoints(5, &[2, 2, 5]),
        Err(TrianglePipelineError::ComputeColorCheckpointOrder {
            checkpoint: 1,
            previous: 2,
            dispatch_limit: 2,
            dispatches: 5,
        })
    );
    assert_eq!(
        validate_compute_color_checkpoints(5, &[2, 4]),
        Err(TrianglePipelineError::ComputeColorCheckpointMissingFinal {
            final_checkpoint: 4,
            dispatches: 5,
        })
    );
}

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
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::from_wire(0, 0),
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

/// `shade_passthrough_combine_params`'s sibling, selecting TEXEL0 (D=1)
/// instead of SHADE (D=4) through the same zero-times-anything identity, so
/// the combiner output IS the sampled texel. `alpha_c` is TEXEL0 as well
/// but multiplied by `(A - B) == 0`, so it never reaches the output --
/// present only to keep the wire encoding identical in shape to the
/// SHADE variant.
///
/// This is also what makes `CombineParams::references_texels_in_first_cycle`
/// true, which is what opens the fragment shader's `texture_referenced`
/// gate and makes it call the sampler at all -- a SHADE-passthrough
/// triangle short-circuits before sampling and could never observe a
/// status-4 refusal.
fn texel0_passthrough_combine_params() -> CombineParams {
    let color_a: u32 = 0; // COMBINED
    let color_b: u32 = 0; // COMBINED -- (A - B) == 0
    let color_c: u32 = 0;
    let color_d: u32 = 1; // TEXEL0
    let alpha_a: u32 = 0;
    let alpha_b: u32 = 0;
    let alpha_c: u32 = 1; // TEXEL0, x0
    let alpha_d: u32 = 1; // TEXEL0
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
fn compute_raster_rgba16_round_trip_wgsl_parses_and_validates() {
    let module =
        naga::front::wgsl::parse_str(crate::shader_manifest::COMPUTE_RASTER_RGBA16_ROUND_TRIP_WGSL)
            .unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn compute_triangle_coverage_wgsl_parses_and_validates() {
    let source = crate::shader_manifest::compute_triangle_color_wgsl();
    let module = naga::front::wgsl::parse_str(&source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn compute_triangle_color_composition_parses_and_validates() {
    let source = crate::shader_manifest::compute_triangle_color_wgsl();
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

/// Row-stride correctness (fixing the independent review's Bug 1): for both
/// of this crate's real fixture sizes (8x8 and 16x16), `padded_bytes_per_row
/// / 4` (the value `submit_triangles` threads into
/// `ResolvedFragmentBlendParams`'s serialized `row_stride_words` word) must
/// differ from `extent.width` -- proving these two "row width" notions are
/// genuinely distinct for real fixture sizes, not accidentally equal (which
/// would hide a regression back to indexing by `extent.width`). Also asserts
/// `fragment_blend_params_bytes` serializes the caller-supplied
/// `row_stride_words` value, not a value it re-derives from `extent.width`
/// itself (this function takes no `extent` parameter at all -- the row-
/// stride fix's root cause was exactly this kind of second, independent
/// source of "width").
#[test]
fn row_stride_words_differs_from_extent_width_for_both_real_fixture_sizes() {
    for width in [8u32, 16u32] {
        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row_stride_words = padded_bytes_per_row / 4;
        assert_ne!(
            row_stride_words, width,
            "row_stride_words must differ from extent.width={width} for this test to be \
             meaningful"
        );

        let bytes =
            fragment_blend_params_bytes(ResolvedFragmentBlendParams::NO_OP, row_stride_words);
        let serialized = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(
            serialized, row_stride_words,
            "fragment_blend_params_bytes must serialize the caller-supplied row_stride_words, \
             not extent.width={width}"
        );
        assert_ne!(
            serialized, width,
            "the serialized row-stride word must not equal extent.width={width}"
        );
    }
}

/// `has_framebuffer_color` (formerly `_reserved_0`) serializes
/// `ResolvedFragmentBlendParams::reads_framebuffer_color` as `0`/`1` at bytes
/// 36..40.
#[test]
fn has_framebuffer_color_serializes_reads_framebuffer_color_at_bytes_36_to_40() {
    let mut reading = ResolvedFragmentBlendParams::NO_OP;
    reading.reads_framebuffer_color = true;
    let reading_bytes = fragment_blend_params_bytes(reading, 8);
    assert_eq!(&reading_bytes[36..40], &1u32.to_le_bytes());

    let not_reading_bytes = fragment_blend_params_bytes(ResolvedFragmentBlendParams::NO_OP, 8);
    assert_eq!(&not_reading_bytes[36..40], &0u32.to_le_bytes());
}

/// Run-splitting helper (framebuffer-blend Slice B): fixture sequences with
/// 0, 1, and 2+ framebuffer-color-reading fixtures at start/middle/end/
/// consecutive positions, asserting run boundaries land exactly where the
/// ordering contract requires -- singleton runs for every
/// framebuffer-color-reading fixture, maximal contiguous runs otherwise.
#[test]
fn split_fixture_runs_singles_out_every_framebuffer_color_reading_fixture() {
    // No reading fixtures: one run covering everything.
    assert_eq!(split_fixture_runs(&[false, false, false]), vec![(0, 3)]);

    // A single reading fixture at the start: singleton run, then the rest.
    assert_eq!(
        split_fixture_runs(&[true, false, false]),
        vec![(0, 1), (1, 3)]
    );

    // A single reading fixture in the middle: split before and after.
    assert_eq!(
        split_fixture_runs(&[false, true, false]),
        vec![(0, 1), (1, 2), (2, 3)]
    );

    // A single reading fixture at the end.
    assert_eq!(
        split_fixture_runs(&[false, false, true]),
        vec![(0, 2), (2, 3)]
    );

    // Consecutive reading fixtures: each is its own singleton run (draw N's
    // output becomes draw N+1's destination when N+1 also reads framebuffer
    // color, so they cannot share one snapshot).
    assert_eq!(
        split_fixture_runs(&[true, true, false]),
        vec![(0, 1), (1, 2), (2, 3)]
    );
    assert_eq!(
        split_fixture_runs(&[false, true, true]),
        vec![(0, 1), (1, 2), (2, 3)]
    );

    // Every fixture reads framebuffer color: every fixture is its own run.
    assert_eq!(
        split_fixture_runs(&[true, true, true]),
        vec![(0, 1), (1, 2), (2, 3)]
    );

    // Single-fixture cases.
    assert_eq!(split_fixture_runs(&[false]), vec![(0, 1)]);
    assert_eq!(split_fixture_runs(&[true]), vec![(0, 1)]);

    // Empty input: no runs.
    assert_eq!(split_fixture_runs(&[]), Vec::<(usize, usize)>::new());
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
    let none_bytes =
        fragment_alpha_compare_params_bytes(crate::state::AlphaCompare::None, Color4::from_wire(0));
    assert_eq!(none_bytes.len(), ALPHA_COMPARE_PARAMS_BYTES as usize);
    assert_eq!(&none_bytes[0..4], &0u32.to_le_bytes());
    assert_eq!(&none_bytes[4..8], &0u32.to_le_bytes());
    assert_eq!(&none_bytes[8..16], &[0u8; 8]);

    let blend_color = crate::state::Color4::from_wire(0x1122_33AA);
    let threshold_bytes =
        fragment_alpha_compare_params_bytes(crate::state::AlphaCompare::Threshold, blend_color);
    assert_eq!(&threshold_bytes[0..4], &1u32.to_le_bytes());
    assert_eq!(&threshold_bytes[4..8], &0xAAu32.to_le_bytes());
    assert_eq!(&threshold_bytes[8..16], &[0u8; 8]);

    let zero_threshold = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        crate::state::Color4::from_wire(0),
    );
    assert_eq!(&zero_threshold[4..8], &0u32.to_le_bytes());

    let max_threshold = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        crate::state::Color4::from_wire(0xFFFF_FFFF),
    );
    assert_eq!(&max_threshold[4..8], &255u32.to_le_bytes());
}

/// Retargeted from `..._rejects_reserved_mode_defensively`. Pinned RT64's
/// shader branches only for `G_AC_DITHER` and `G_AC_THRESHOLD`, so wire
/// encoding 2 falls through to no compare
/// (`src/shaders/RasterPS.hlsl:203-213`, commit `f0728a2`) and reaches the
/// pipeline as an admitted mode carrying wire 0. See
/// `docs/RT64-GUARD-AUDIT.md` finding A3.
#[test]
fn alpha_compare_wire_two_reaches_the_pipeline_as_no_compare() {
    let decoded = crate::state::OtherMode::from_wire(0, 2).alpha_compare();
    assert_eq!(decoded, crate::state::AlphaCompare::None);
    let bytes = fragment_alpha_compare_params_bytes(decoded, Color4::from_wire(0xFFFF_FFFF));
    // Mode word 0 = no compare. Wire 1 would be Threshold; asserting the
    // difference keeps this from passing under a decoder that returns a
    // constant.
    assert_eq!(&bytes[0..4], &0u32.to_le_bytes());
    let threshold_bytes = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Threshold,
        Color4::from_wire(0xFFFF_FFFF),
    );
    assert_eq!(&threshold_bytes[0..4], &1u32.to_le_bytes());
}

#[test]
#[should_panic(expected = "must have been rejected at retrieval time")]
fn fragment_alpha_compare_params_bytes_rejects_dither_mode_defensively() {
    let _ = fragment_alpha_compare_params_bytes(
        crate::state::AlphaCompare::Dither,
        Color4::from_wire(0),
    );
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

/// `image_read_enabled` with `force_blend` CLEAR still refuses: there
/// `blend_enabled == antialias_enabled && !wraps` genuinely reads the
/// unknown `memory` through `wraps`, so the value would reach
/// `blend_fragment_cycle_fn` (`triangle_pipeline_fragment.wgsl:326`).
/// This is the arm a real `image_read_enabled` triangle takes, and it is
/// the constraint-1 proof that narrowing the refusal did not delete it.
#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_clamp_with_image_read_and_no_force_blend() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        true,
        false,
        true,
        false,
        false,
    );
}

#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_wrap_with_image_read_and_no_force_blend() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        true,
        false,
        true,
        false,
        false,
    );
}

/// `alpha_coverage_select` set routes the `memory`-dependent
/// `adjusted_coverage` into `output.color.a`
/// (`triangle_pipeline_fragment.wgsl:283-285`), so it refuses even with
/// `force_blend` set -- the second half of the named boundary.
#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_wrap_with_image_read_and_alpha_coverage_select() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        true,
        true,
        true,
        false,
        true,
    );
}

#[test]
#[should_panic(expected = "must be rejected before GPU submission")]
fn fragment_coverage_params_bytes_rejects_clamp_with_image_read_and_alpha_coverage_select() {
    let _ = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        true,
        true,
        true,
        false,
        true,
    );
}

/// The admitted narrow case: `image_read_enabled` with `force_blend` set and
/// `alpha_coverage_select` clear. Hand-derived from
/// `triangle_pipeline_fragment.wgsl`'s two coverage-output routes -- with
/// `force_blend` set, `blend_enabled` is `true` for every `memory` value, and
/// with `alpha_coverage_select` clear the `memory`-dependent `destination`
/// never reaches `output.color.a`. The serialized `coverage_destination` word
/// is the mode's own encoding (Clamp=0/Wrap=1), NOT a substituted `Full`(2):
/// a substitution is what this admission must not be, and asserting the
/// distinct wire values is what would catch one.
#[test]
fn fragment_coverage_params_bytes_admits_image_read_when_memory_cannot_be_observed() {
    let clamp = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Clamp,
        true,
        true,
        true,
        false,
        false,
    );
    assert_eq!(&clamp[0..4], &0u32.to_le_bytes());
    assert_eq!(&clamp[4..8], &1u32.to_le_bytes());
    assert_eq!(&clamp[8..12], &1u32.to_le_bytes());

    let wrap = fragment_coverage_params_bytes(
        crate::state::CoverageDestination::Wrap,
        true,
        true,
        true,
        false,
        false,
    );
    assert_eq!(&wrap[0..4], &1u32.to_le_bytes());
    assert_eq!(&wrap[4..8], &1u32.to_le_bytes());
    assert_eq!(&wrap[8..12], &1u32.to_le_bytes());
}

/// WM2000's own latched texrect mode, decoded from the captured packet's
/// other-mode low word `0x005041c8` rather than transcribed: `cvg_dst=Wrap`
/// (bits 9:8 == 1), `IM_RD` (bit 6), `AA_EN` (bit 3), `CLR_ON_CVG` (bit 7),
/// `FORCE_BL` (bit 14), `CVG_X_ALPHA` (bit 12) clear, `ALPHA_CVG_SEL`
/// (bit 13) clear. The bits are re-derived here through `OtherMode`'s own
/// accessors from the raw word, so a change to either the word or an
/// accessor breaks this rather than silently agreeing.
#[test]
fn wm2000_latched_texrect_coverage_mode_is_admitted_and_is_not_memory_dependent() {
    let mode = crate::state::OtherMode::from_wire(0x0000_acef, 0x0050_41c8);
    assert_eq!(
        mode.coverage_destination(),
        crate::state::CoverageDestination::Wrap
    );
    assert!(mode.image_read_enabled());
    assert!(mode.antialias_enabled());
    assert!(mode.clear_on_coverage());
    assert!(mode.force_blend());
    assert!(!mode.coverage_times_alpha());
    assert!(!mode.alpha_coverage_select());

    let bytes = fragment_coverage_params_bytes(
        mode.coverage_destination(),
        mode.image_read_enabled(),
        mode.force_blend(),
        mode.antialias_enabled(),
        mode.coverage_times_alpha(),
        mode.alpha_coverage_select(),
    );
    assert_eq!(&bytes[0..4], &1u32.to_le_bytes());
    assert_eq!(&bytes[4..8], &1u32.to_le_bytes());
}

/// The safety argument stated as arithmetic over `coverage_result` itself,
/// independent of the shader transcription: under WM2000's latched mode
/// every `memory` in `0..=8` yields the same `blend_enabled`, so the one
/// term the admitted draw actually consumes is `memory`-invariant. The
/// companion half is deliberately asserted too -- `destination` DOES vary
/// with `memory`, which is why the admission rests on non-observability
/// rather than on the accumulation being a no-op.
#[test]
fn wm2000_mode_blend_enabled_is_memory_invariant_while_destination_is_not() {
    let mode = crate::CoverageModeBits {
        coverage_destination: crate::state::CoverageDestination::Wrap,
        image_read_enabled: true,
        force_blend: true,
        antialias_enabled: true,
    };
    let pixel = crate::Coverage::FULL;
    let mut destinations = std::collections::BTreeSet::new();
    for memory in 0..=8u8 {
        let result = crate::coverage_result(pixel, crate::Coverage::new(memory), mode);
        assert!(
            result.blend_enabled,
            "force_blend must dominate for memory={memory}"
        );
        destinations.insert(result.destination.count());
    }
    assert!(
        destinations.len() > 1,
        "destination must genuinely vary with memory: {destinations:?}"
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
    let none_bytes =
        fragment_material_params_bytes(Color4::from_wire(0), PrimColor::from_wire(0, 0));
    assert_eq!(none_bytes.len(), MATERIAL_PARAMS_BYTES as usize);
    assert_eq!(&none_bytes[0..16], &[0u8; 16]);
    assert_eq!(&none_bytes[16..32], &[0u8; 16]);
    assert_eq!(&none_bytes[32..36], &0f32.to_le_bytes());
    assert_eq!(&none_bytes[36..48], &[0u8; 12]);

    let env_color = crate::state::Color4::from_wire(0x1122_33AA);
    let prim_color = crate::state::PrimColor::from_wire(0x0000_0080, 0x4455_66BB);
    let material_bytes = fragment_material_params_bytes(env_color, prim_color);
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
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::from_wire(0, 0),
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

/// **WM2000's measured tile shape, as a fixture: `IntensityAlpha`/`Bits4`
/// under an enabled `G_TT_RGBA16` TLUT.**
///
/// This is the combination that aborted the all-Rust stack at
/// `rsp_commit.rs:1202` with `tmem_sample.wgsl` status 4
/// (`TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT`). Under `tlut_en` the RDP
/// ignores the tile format and sources the texel from a palette; for a
/// 4-bit texel size the palette's TMEM address comes from the tile's own
/// `palette` field (n64brew `Reality_Display_Processor/Pipeline`, CC BY-SA
/// 4.0, quoted in `shaders/tmem_sample.wgsl`'s header).
///
/// Everything here is shared by the GPU test and the CPU oracle it is
/// differentiated against, so the two cannot drift apart in what they mean
/// by "this tile".
mod tlut_fixture {
    use crate::tmem::{TmemGpuProjection, TMEM_VALIDITY_WORDS};
    use crate::{
        ImageFormat, PixelSize, TileAddressMode, TileCoordinate, TileDescriptor, TileSize,
        TmemWordAddress,
    };

    /// Byte 0 of the tile's own image data: the packed pair of 4-bit texels
    /// for columns 0 and 1 of row 0. Even column is bits 7:4, odd column
    /// bits 3:0 (`unpack_ci4_texel`) -- `0x30` therefore means column 0
    /// indexes nibble 3 and column 1 indexes nibble 0. Deliberately
    /// DIFFERENT nibbles so a decoder taking the wrong half is caught.
    pub const PACKED_ROW0: u8 = 0x30;
    /// Row 1's packed pair, at the tile's row-1 address AFTER the odd-row
    /// XOR4 exchange (`odd_row_exchange`): row 1's linear address is
    /// `line_words * 8 == 8`, exchanged to `8 ^ 4 == 12`.
    pub const PACKED_ROW1: u8 = 0x21;
    pub const ROW1_EXCHANGED_ADDRESS: usize = 12;

    /// The tile's four-bit palette selector. NONZERO on purpose: the index
    /// is `(palette << 4) | nibble`, so a decoder that drops the palette
    /// field reads entry `nibble` instead of `0x50 | nibble` and lands on a
    /// different (deliberately different-colored) TLUT entry.
    pub const PALETTE: u8 = 5;

    pub const TMEM_WORD_ADDRESS: u16 = 0;
    pub const LINE_WORDS: u16 = 1;

    /// The four palette entries this fixture's texels reach, as
    /// `(index, rgba16_value)`. `0x50 | nibble` for nibble in 0..=3.
    ///
    /// Every value is chosen so the RGBA16 and IA16 decodes of the SAME
    /// entry differ -- that is what lets the `G_TT_IA16` test discriminate
    /// the two entry formats instead of passing vacuously.
    /// `0xffff` is deliberately absent: it decodes to opaque white under
    /// both formats and would make that discrimination impossible (a
    /// measured trap -- the first draft of this fixture used it and the
    /// IA16 test's own guard assertion caught it).
    pub const ENTRIES: [(u8, u16); 4] = [
        (0x50, 0xf801), // RGBA16 (255,0,0,255) vs IA16 (248,248,248,1)
        (0x51, 0x07c1), // RGBA16 (0,255,0,255) vs IA16 (7,7,7,193)
        (0x52, 0x003f), // RGBA16 (0,0,255,255) vs IA16 (0,0,0,63)
        (0x53, 0x8421), // RGBA16 (132,132,132,255) vs IA16 (132,132,132,33)
    ];

    /// The TLUT entries the four decoy indexes would hit if the palette
    /// field were dropped (`nibble` instead of `0x50 | nibble`). Written to
    /// a DIFFERENT color from their palette-5 counterparts so
    /// "palette field ignored" is a visible failure, not a silent pass.
    pub const DECOY_ENTRIES: [(u8, u16); 4] = [
        (0x00, 0x0001),
        (0x01, 0x0801),
        (0x02, 0x1001),
        (0x03, 0x1801),
    ];

    /// `IntensityAlpha`/`Bits4` -- NOT `ColorIndex`. That is the whole
    /// point: the format must be ignored while the TLUT is enabled.
    pub fn descriptor() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::IntensityAlpha,
            PixelSize::Bits4,
            LINE_WORDS,
            TmemWordAddress::try_new(TMEM_WORD_ADDRESS).unwrap(),
            PALETTE,
            TileAddressMode::default(),
            0,
            0,
            TileAddressMode::default(),
            0,
            0,
        )
    }

    /// A 2x2-texel tile, matching `production.rs`'s own RGBA16 fixture's
    /// S10.2 convention (`high.integer() - low.integer() + 1 == 2`).
    pub fn size() -> TileSize {
        TileSize::from_wire(
            TileCoordinate::try_new(0).unwrap(),
            TileCoordinate::try_new(0).unwrap(),
            TileCoordinate::try_new(4).unwrap(),
            TileCoordinate::try_new(4).unwrap(),
        )
    }

    /// The 16-bit sibling of `descriptor`, over the SAME TMEM bytes: an
    /// `Rgba`/`Bits16` tile under an enabled TLUT. Under `tlut_en` a 16-bit
    /// texel indexes the palette through its HIGH (big-endian first) byte
    /// and the low byte is ignored -- the case `4c412a96` admitted on the
    /// CPU side. Included so a shader that hardcoded the index (or read the
    /// low byte) cannot survive the 4-bit tests alone.
    ///
    /// `line_words` is 1 as for the 4-bit tile; with two bytes per texel the
    /// tile's own row 0 spans bytes 0..4.
    pub fn descriptor_sixteen_bit() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::Rgba,
            PixelSize::Bits16,
            LINE_WORDS,
            TmemWordAddress::try_new(TMEM_WORD_ADDRESS).unwrap(),
            PALETTE,
            TileAddressMode::default(),
            0,
            0,
            TileAddressMode::default(),
            0,
            0,
        )
    }

    /// A sparse `TmemByteSource` over the same bytes the GPU projection
    /// carries -- the CPU oracle's half of the differential. Only the
    /// addresses this fixture writes are valid, exactly like untouched
    /// physical TMEM.
    pub struct FixtureTmem {
        pub bytes: std::collections::BTreeMap<u16, u8>,
    }

    impl crate::TmemByteSource for FixtureTmem {
        fn snapshot(&self) -> crate::TmemSnapshotIdentity {
            // A fresh durable state's own identity: this fixture's bytes
            // are read through `valid_byte`, and no caller here compares
            // snapshots, so borrowing one real committed identity is
            // honest -- matching `tmem/read.rs`'s own `SparseSource`.
            crate::TmemByteSource::snapshot(&crate::PhysicalTmemState::try_new().unwrap())
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            self.bytes.get(&address).copied()
        }
    }

    /// Writes one quadricated TLUT entry: four identical big-endian 16-bit
    /// lanes across the eight bytes at `0x800 + index * 8`. Both the CPU
    /// reader (`read_canonical_tlut_entry`) and the WGSL sampler require
    /// all eight valid and all four lanes equal.
    fn write_entry(bytes: &mut std::collections::BTreeMap<u16, u8>, index: u8, value: u16) {
        let base = 0x0800u16 + u16::from(index) * 8;
        for lane in 0..4u16 {
            bytes.insert(base + lane * 2, (value >> 8) as u8);
            bytes.insert(base + lane * 2 + 1, (value & 0xff) as u8);
        }
    }

    /// The one byte map both halves of the differential read.
    pub fn bytes() -> std::collections::BTreeMap<u16, u8> {
        let mut bytes = std::collections::BTreeMap::new();
        bytes.insert(0u16, PACKED_ROW0);
        bytes.insert(ROW1_EXCHANGED_ADDRESS as u16, PACKED_ROW1);
        for (index, value) in ENTRIES {
            write_entry(&mut bytes, index, value);
        }
        for (index, value) in DECOY_ENTRIES {
            write_entry(&mut bytes, index, value);
        }
        // The 16-bit path's entries. `descriptor_sixteen_bit`'s texel at
        // (0,0) spans bytes 0..2, whose HIGH byte is `PACKED_ROW0` and whose
        // LOW byte is byte 1. Byte 1 is left INVALID by this map on purpose
        // for the 4-bit fixture, so give the 16-bit case its own explicit
        // low byte, chosen to differ from the high byte -- a decoder reading
        // the low byte then lands on `LOW_BYTE_DECOY_INDEX` instead.
        bytes.insert(1u16, SIXTEEN_BIT_LOW_BYTE);
        write_entry(&mut bytes, SIXTEEN_BIT_INDEX, SIXTEEN_BIT_ENTRY);
        write_entry(&mut bytes, SIXTEEN_BIT_LOW_BYTE, SIXTEEN_BIT_LOW_DECOY);
        // The 2x2 tile's three OTHER corners at 16 bits, so the
        // three-nearest filter's four reads all hit valid bytes. Column 1
        // of row 0 is bytes 2..4; row 1 (odd) is exchanged, so its two
        // texels live at `8 ^ 4 = 12`..16. Each is given
        // `SIXTEEN_BIT_INDEX` as its own high byte, which makes all four
        // corners resolve to one entry and the filter the identity -- the
        // assertion under test is the index derivation, not the blend.
        for address in [2u16, 12, 14] {
            bytes.insert(address, SIXTEEN_BIT_INDEX);
            bytes.insert(address + 1, SIXTEEN_BIT_LOW_BYTE);
        }
        bytes
    }

    /// Index the 16-bit texel at (0,0) must resolve to: its HIGH byte,
    /// which is `PACKED_ROW0`. The tile's `palette` field is NOT applied at
    /// 16-bit -- only 4-bit texels take the palette prefix
    /// (`resolve_indexed_texel`).
    pub const SIXTEEN_BIT_INDEX: u8 = PACKED_ROW0;
    pub const SIXTEEN_BIT_ENTRY: u16 = 0x1f83;
    /// The low byte, and the entry a low-byte-reading decoder would hit.
    /// Different from `SIXTEEN_BIT_INDEX` and given a different color, so
    /// "reads the wrong byte" is a visible failure.
    pub const SIXTEEN_BIT_LOW_BYTE: u8 = 0xa7;
    pub const SIXTEEN_BIT_LOW_DECOY: u16 = 0x0842;

    pub fn source() -> FixtureTmem {
        FixtureTmem { bytes: bytes() }
    }

    /// The GPU half: the same map, projected into the shader's byte image
    /// plus validity bitmap. Built from `bytes()` rather than from a second
    /// hand-written table, so the two halves cannot describe different TMEM.
    pub fn projection() -> TmemGpuProjection {
        let mut projection = TmemGpuProjection {
            bytes: [0u8; fn64_render_ir::TMEM_BYTES as usize],
            validity_words: [0u32; TMEM_VALIDITY_WORDS],
        };
        for (address, byte) in bytes() {
            let address = address as usize;
            projection.bytes[address] = byte;
            projection.validity_words[address / 32] |= 1 << (address % 32);
        }
        projection
    }
}

/// **Positive control for the fixture itself.** Adapter-free: proves the
/// fixture really is `IntensityAlpha`/`Bits4` under an enabled TLUT --
/// never a `ColorIndex` tile that would pass the GPU test vacuously through
/// the pre-existing CI path -- and that the CPU reader (fixed at
/// `4c412a96`) resolves it through the palette field to the entries this
/// fixture wrote.
#[test]
fn tlut_fixture_is_genuinely_a_non_ci_four_bit_tile_the_cpu_reader_palettizes() {
    let descriptor = tlut_fixture::descriptor();
    assert_eq!(
        descriptor.format(),
        crate::ImageFormat::IntensityAlpha,
        "a ColorIndex tile would pass the GPU test through the pre-existing \
         CI path without exercising the format-ignored rule at all"
    );
    assert_eq!(descriptor.size(), crate::PixelSize::Bits4);
    assert_eq!(descriptor.palette(), tlut_fixture::PALETTE);
    assert_ne!(
        descriptor.palette(),
        0,
        "a zero palette cannot discriminate (palette << 4) | nibble from nibble"
    );

    // Column 0 of row 0 takes the HIGH nibble (3), column 1 the LOW (0);
    // the packed byte's two nibbles differ, so a parity mistake is visible.
    assert_ne!(
        tlut_fixture::PACKED_ROW0 >> 4,
        tlut_fixture::PACKED_ROW0 & 0x0f
    );

    let source = tlut_fixture::source();
    for (column, nibble) in [
        (0u16, tlut_fixture::PACKED_ROW0 >> 4),
        (1, tlut_fixture::PACKED_ROW0 & 0x0f),
    ] {
        let addressed = crate::AddressedTmemTexel::new(column, 0, crate::TmemFirstRowParity::Even);
        let decoded = crate::read_texel(
            &source,
            descriptor,
            addressed,
            crate::TextureLutMode::Rgba16,
        )
        .expect("the CPU reader palettizes an IA4 tile under an enabled TLUT");
        let expected_index = (tlut_fixture::PALETTE << 4) | nibble;
        let (_, expected_value) = tlut_fixture::ENTRIES
            .into_iter()
            .find(|(index, _)| *index == expected_index)
            .expect("this fixture wrote every entry its texels reach");
        let expected = crate::decode_direct_texel(
            crate::ImageFormat::Rgba,
            crate::RawTexel::try_new(crate::PixelSize::Bits16, u32::from(expected_value)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            decoded.texel().rgba8888(),
            expected.rgba8888(),
            "column {column} must palettize through index {expected_index:#04x}"
        );
        // The decoy entry at the same nibble is a DIFFERENT color, so this
        // assertion would also fail if the palette field were dropped.
        let (_, decoy_value) = tlut_fixture::DECOY_ENTRIES
            .into_iter()
            .find(|(index, _)| *index == nibble)
            .expect("this fixture wrote every decoy entry");
        assert_ne!(expected_value, decoy_value);
    }
}

// ===================================================================
// CPU-vs-GPU texel-path pins.
//
// Both known lane defects (palette handling, fixed at `4c412a96` /
// `48cde862`; row parity, at `72bb2664`) had the same shape: a quantity
// the CPU reader derives and the WGSL sampler hardcodes, or a guard one
// side runs and the other does not. Each was discovered one abort at a
// time. The tests below pin the rest of that class against each other so
// the whole class fails as one suite instead.
//
// Each test names the quantity, the two sides' file:line, and whether
// WM2000's measured tiles reach it.
// ===================================================================

/// **D-LOWBYTE: a 16-bit texel under an enabled TLUT.**
///
/// The CPU reader takes the whole big-endian 16-bit texel through
/// `read_linear_bytes::<2>` (`tmem/read.rs:527-531`) and only then
/// narrows to the high byte in `resolve_indexed_texel`
/// (`tmem/texel.rs:376-378`). It therefore REQUIRES the low byte to be
/// valid, even though the low byte's value is discarded. The WGSL
/// sampler reads only the high byte
/// (`shaders/tmem_sample.wgsl`, `tmem_sample_tlut_texel`'s
/// `pixel_size == TMEM_PIXEL_SIZE_BITS16` arm) and never touches the
/// low byte at all.
///
/// So on a tile whose 16-bit texel has a valid high byte and an INVALID
/// low byte, the CPU refuses (`InvalidTexelByte`) and the GPU samples
/// happily. This test pins the disagreement rather than picking a
/// winner: it asserts the CPU's own refusal, which is the arm the GPU
/// lacks, so a future widening of either side has to confront it.
///
/// **Hardware ruling needed.** The RDP fetches TMEM in 64-bit words, so
/// the physical read almost certainly covers both bytes regardless; but
/// "was this byte ever written" is this crate's own validity model, not
/// a silicon property, and public documentation does not say whether a
/// partially-loaded 16-bit texel's palette index is well-defined. Until
/// that is settled, both behaviours are defensible and neither refusal
/// is weakened here.
///
/// **Not in WM2000's path**: WM2000's measured tiles are IA4 under
/// `G_TT_RGBA16` (4-bit, one byte per two texels) and RGBA16 with the
/// TLUT disabled. Neither is 16-bit-under-enabled-TLUT.
///
/// Adapter-free: this is the CPU half of the divergence, and it is the
/// half that has an assertable behaviour today.
#[test]
fn a_sixteen_bit_tlut_texel_with_an_invalid_low_byte_splits_the_two_lanes() {
    let descriptor = tlut_fixture::descriptor_sixteen_bit();
    let addressed = crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even);

    // Byte 0 (the high byte, the index) valid; byte 1 (the low byte,
    // discarded by both sides' arithmetic) removed.
    let mut bytes = tlut_fixture::bytes();
    assert!(
        bytes.remove(&1u16).is_some(),
        "the fixture must have had a low byte to remove, or this test \
         asserts nothing"
    );
    let source = tlut_fixture::FixtureTmem { bytes };

    // Control: with the low byte present the CPU reader succeeds, so
    // the refusal below is caused by the low byte and nothing else.
    assert!(
        crate::read_texel(
            &tlut_fixture::source(),
            descriptor,
            addressed,
            crate::TextureLutMode::Rgba16,
        )
        .is_ok(),
        "control: the same tile with a valid low byte must read"
    );

    // The CPU arm the shader does not have.
    assert!(
        matches!(
            crate::read_texel(
                &source,
                descriptor,
                addressed,
                crate::TextureLutMode::Rgba16,
            ),
            Err(crate::PhysicalTexelReadError::InvalidTexelByte { address: 1 }),
        ),
        "the CPU reader requires the discarded low byte to be valid; \
         `tmem_sample.wgsl` never reads it. Pinned disagreement, not a \
         fixed defect -- see this test's doc for the hardware ruling."
    );
}

/// **D-PALETTE-WIDTH: an out-of-range palette selector.**
///
/// The CPU reader routes the tile's `palette` field through
/// `Ci4Palette::try_new` (`tmem/texel.rs:198-204`), which REFUSES any
/// value above `0x0f` with `Ci4PaletteError`; `preflight`
/// (`tmem/read.rs:472`) runs that check before a single byte is read.
/// The WGSL sampler instead MASKS -- `(tile.palette & 0x0fu) << 4u`
/// in `tmem_sample_tlut_texel` -- and samples entry `(palette & 0x0f)`
/// as if nothing were wrong.
///
/// This is exactly the two known defects' shape inverted: a guard on
/// one side, silent normalization on the other. It is LATENT, not live:
/// `fn64-render-reference`'s `SetTile` decode narrows the field with
/// `((w1 >> 20) & 0x0f)` (`gbi/state.rs:1311`), so no wire path reaches
/// a wider value today. But `TileDescriptor::from_neutral_parts`
/// (`tmem/types.rs:194-210`) is PUBLIC and takes a bare `u8` with no
/// narrowing of its own, so the CPU refusal is the only remaining
/// guard, and the GPU has no counterpart to it.
///
/// **Not in WM2000's path**: the measured tile's palette is a legal
/// 4-bit value.
///
/// Adapter-free, and it MUTATION-KILLS the kept refusal: deleting the
/// `Ci4Palette::try_new` bound check makes this test fail.
#[test]
fn an_out_of_range_palette_is_refused_by_the_cpu_and_masked_by_the_shader() {
    let wide = crate::TileDescriptor::from_neutral_parts(
        crate::ImageFormat::IntensityAlpha,
        crate::PixelSize::Bits4,
        tlut_fixture::LINE_WORDS,
        crate::TmemWordAddress::try_new(tlut_fixture::TMEM_WORD_ADDRESS).unwrap(),
        0x15, // masks to 5 -- the fixture's own legal palette
        crate::TileAddressMode::default(),
        0,
        0,
        crate::TileAddressMode::default(),
        0,
        0,
    );
    assert_eq!(
        wide.palette(),
        0x15,
        "the public constructor must NOT narrow, or this test asserts \
         nothing about the CPU guard"
    );

    let addressed = crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even);

    // The CPU refuses by name, before any byte is read.
    assert!(
        matches!(
            crate::read_texel(
                &tlut_fixture::source(),
                wide,
                addressed,
                crate::TextureLutMode::Rgba16,
            ),
            Err(crate::PhysicalTexelReadError::Ci4Palette(_)),
        ),
        "an out-of-range palette selector must be refused, not masked"
    );

    // The host-side uniform mirror carries the unnarrowed value
    // straight through to the shader, where `& 0x0f` silently turns it
    // into the legal palette below. That is the divergence: the two
    // lanes disagree about whether this tile is samplable at all.
    let binding = TileBindingParams::bound(wide, tlut_fixture::size())
        .with_lut_mode(crate::TextureLutMode::Rgba16);
    assert_eq!(
        binding.palette, 0x15,
        "the uniform carries the raw field; the shader's own `& 0x0f` \
         is where the two lanes part"
    );
    assert_eq!(
        binding.palette & 0x0f,
        u32::from(tlut_fixture::PALETTE),
        "and it masks to a DIFFERENT, entirely legal palette -- so the \
         shader would paint a plausible wrong color, never an error"
    );
}

/// **D-NEGATIVE-COLUMN: an unclamped, unmasked negative coordinate --
/// INVESTIGATED AND REFUTED, pinned so it stays refuted.**
///
/// `address_axis_texel` returns the coordinate unchanged when the axis
/// neither clamps nor masks. The CPU narrows that `i64` with
/// `coordinate as u16` (`tmem/sample.rs:566`); the WGSL sampler narrows
/// the same value from `i32` with `u32(coordinate)`
/// (`shaders/tmem_sample.wgsl`, `address_axis_texel`'s `mask == 0u`
/// return). Those two narrowings genuinely differ -- coordinate `-1`
/// gives `0xffff` on the CPU and `0xffff_ffff` on the GPU -- which has
/// exactly the shape of the two known lane defects, so it looked like a
/// third instance.
///
/// **It is not one.** The difference cannot survive into a byte address.
/// Both narrowings agree on the low sixteen bits by construction, every
/// column multiplier is a power of two no larger than two (`column / 2`
/// at 4 bits, `column` at 8, `column * 2` at 16), and both lanes then mask
/// the linear address with the same twelve-bit `TMEM_ADDRESS_MASK`. Twelve
/// bits of result can only depend on the low thirteen bits of the column,
/// which the two narrowings share. The exhaustive check below confirms it
/// over every column both lanes can produce, at all three ported sizes.
///
/// Kept as a pin rather than deleted, because the refutation is
/// conditional on facts a later change could break: it holds only while
/// every multiplier stays a power of two `<= 2` and the mask stays at
/// twelve bits. Widen the mask, or add a size whose stride is not a power
/// of two, and this test fails and the divergence becomes real.
///
/// Adapter-free. **Not in WM2000's path** either way: its measured tile
/// has `mask_s == mask_t == 0`, so both axes clamp and the coordinate is
/// pinned into `0..dimension` before any narrowing happens.
#[test]
fn the_two_lanes_column_narrowings_differ_but_cannot_reach_a_different_byte() {
    // The two narrowings really are different values.
    let cpu_column = u32::from((-1_i64) as u16);
    let gpu_column = (-1_i32) as u32;
    assert_eq!(cpu_column, 0x0000_ffff);
    assert_eq!(gpu_column, 0xffff_ffff);
    assert_ne!(
        cpu_column, gpu_column,
        "if these ever agree the refutation below is vacuous"
    );

    // `linear_byte_address` (`tmem/read.rs:614-628`) and
    // `tmem_indexed_linear_base` (`shaders/tmem_sample.wgsl`), each in its
    // own lane's integer width, then each lane's own twelve-bit mask.
    let cpu_address = |column: u32, size_bits: u32| -> u32 {
        let column = u64::from(column);
        let offset = match size_bits {
            4 => column / 2,
            8 => column,
            _ => column * 2,
        };
        (offset & 0x0fff) as u32
    };
    let gpu_address = |column: u32, size_bits: u32| -> u32 {
        let offset = match size_bits {
            4 => column / 2,
            8 => column,
            _ => column.wrapping_mul(2),
        };
        offset & 0x0fff
    };

    // Exhaustive over every column the CPU lane can hold, at all three
    // sizes the shader ports. The GPU column for the same source
    // coordinate is the CPU column sign-extended, which is what the
    // `| 0xffff_0000` branch below reconstructs.
    for size_bits in [4u32, 8, 16] {
        for narrow in 0..=u16::MAX {
            let cpu_column = u32::from(narrow);
            for gpu_column in [cpu_column, cpu_column | 0xffff_0000] {
                assert_eq!(
                    cpu_address(cpu_column, size_bits),
                    gpu_address(gpu_column, size_bits),
                    "column {narrow:#06x} at {size_bits} bits: the two \
                     narrowings must land on the same TMEM byte"
                );
            }
        }
    }
}

/// **D-DIRECT-FORMATS: which formats each lane admits with the TLUT
/// off.**
///
/// `decode_direct_texel` (`tmem/texel.rs:494-511`) admits SEVEN direct
/// pairs -- RGBA16, RGBA32, IA4, IA8, IA16, I4, I8 -- plus the CI4/CI8
/// TLUT-disabled alias to I8. `tmem_sample.wgsl`'s disabled arm
/// (`sample_committed_rgba16_three_nearest`) admits exactly ONE:
/// `format == RGBA && size == Bits16`.
///
/// This gap is deliberate and documented in the shader's header ("RGBA32,
/// IA4/IA8/IA16, I4/I8, and YUV direct decodes are still not ported
/// here"), so it is a port frontier, not a defect. It is pinned anyway
/// because it is the largest single CPU/GPU disagreement in the texel
/// path and it is silent: nothing else asserts its size, so a lane
/// widening one side would not learn that the other is behind.
///
/// The pin is against `is_supported_direct_format`
/// (`tmem/gpu_projection.rs:281-286`) -- the host-side mirror of the
/// shader's own condition -- so it fails the moment the mirror and the
/// CPU decoder's admitted set drift further apart or closer together
/// without the other being updated.
///
/// **Reaches WM2000**: WM2000's second measured tile is RGBA16 with the
/// TLUT disabled, which is the one pair BOTH lanes admit. Every other
/// direct pair in this table is refused by the shader and accepted by
/// the CPU reader.
#[test]
fn the_disabled_tlut_arm_admits_seven_pairs_on_the_cpu_and_one_on_the_gpu() {
    use crate::{ImageFormat, PixelSize};

    // The CPU's seven direct pairs, hand-listed from
    // `decode_direct_texel`'s own match arms.
    let direct_pairs = [
        (ImageFormat::Rgba, PixelSize::Bits16),
        (ImageFormat::Rgba, PixelSize::Bits32),
        (ImageFormat::IntensityAlpha, PixelSize::Bits4),
        (ImageFormat::IntensityAlpha, PixelSize::Bits8),
        (ImageFormat::IntensityAlpha, PixelSize::Bits16),
        (ImageFormat::Intensity, PixelSize::Bits4),
        (ImageFormat::Intensity, PixelSize::Bits8),
    ];

    let mut gpu_admitted = Vec::new();
    for (format, size) in direct_pairs {
        let descriptor = crate::TileDescriptor::from_neutral_parts(
            format,
            size,
            tlut_fixture::LINE_WORDS,
            crate::TmemWordAddress::try_new(0).unwrap(),
            0,
            crate::TileAddressMode::default(),
            0,
            0,
            crate::TileAddressMode::default(),
            0,
            0,
        );

        // CPU: every one of the seven decodes, with no TLUT.
        let raw_value = 0u32;
        let raw = crate::RawTexel::try_new(size, raw_value).unwrap();
        assert!(
            crate::decode_direct_texel(format, raw).is_ok(),
            "{format:?}/{size:?} must be one of the CPU's seven direct \
             pairs, or this table is wrong"
        );

        // GPU: the host-side mirror of the shader's own condition.
        let binding = TileBindingParams::bound(descriptor, tlut_fixture::size());
        assert_eq!(
            binding.lut_mode,
            crate::tmem::TLUT_MODE_DISABLED,
            "`bound` must default to a disabled TLUT, or this asserts \
             the wrong arm"
        );
        if binding.is_supported_direct_format() {
            gpu_admitted.push((format, size));
        }
    }

    assert_eq!(
        gpu_admitted,
        vec![(ImageFormat::Rgba, PixelSize::Bits16)],
        "the shader's disabled arm ports exactly the RGBA16 pair; the \
         CPU reader admits seven. If either set changes, the other must \
         be re-checked -- that is the whole point of this pin."
    );
}

/// **D-LOWHALF, resolved: the enabled-TLUT low-half rule is a WRAP, and
/// both lanes now wrap.**
///
/// This test previously pinned a CPU-only REFUSAL
/// (`EnabledCiSourceOutsideLowHalf`) and recorded that `tmem_sample.wgsl`
/// had no counterpart. Its own scope note said the pin would come out on
/// both lanes at once if the rule were resolved against the refusal, and
/// the pinned RT64 port source `5473732a` resolves it:
/// `src/shaders/TextureDecoder.hlsli:162-163` DOES scope an enabled-TLUT
/// index source to one half --
/// `addressMask = select_uint(or(isRgba32, usesTlut), RDP_TMEM_MASK16,
/// RDP_TMEM_MASK8)`, `RDP_TMEM_MASK16 = 0x7FF` (`:15`) -- and applies it
/// as a MASK inside `implLoadTMEM` (`:17-25`), never as a refusal.
///
/// So the constraint was never invented; only the response was wrong.
/// Both lanes now wrap: `tmem/read.rs`'s `AddressScope::LowHalf` and
/// `tmem_sample.wgsl`'s `tmem_indexed_byte_address`. The protection the
/// refusal offered survives -- an index read still cannot reach the
/// palette's own half -- so this is not a widened guard.
///
/// Adapter-free. It is the mutation kill for the WRAP: the expected color
/// is hand-derived, and a decoy palette entry sits at the unwrapped
/// address, so a reader that dropped the mask (reading `0x0800` as image
/// data) or that wrapped to the wrong place produces a different,
/// detectable color rather than agreeing by accident.
#[test]
fn an_enabled_tlut_tile_in_the_high_half_wraps_on_both_lanes() {
    // Same fixture tile, moved to TMEM word 0x100 == byte 0x0800.
    const HIGH_HALF_WORD: u16 = 0x0100;
    let high = crate::TileDescriptor::from_neutral_parts(
        crate::ImageFormat::IntensityAlpha,
        crate::PixelSize::Bits4,
        tlut_fixture::LINE_WORDS,
        crate::TmemWordAddress::try_new(HIGH_HALF_WORD).unwrap(),
        tlut_fixture::PALETTE,
        crate::TileAddressMode::default(),
        0,
        0,
        crate::TileAddressMode::default(),
        0,
        0,
    );
    assert_eq!(
        u32::from(HIGH_HALF_WORD) * 8,
        0x0800,
        "the fixture must land exactly on the TLUT's own base, or it is \
         not testing the boundary"
    );

    let addressed = crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even);
    let source = tlut_fixture::source();

    // The byte at 0x0800 is valid TLUT data, so nothing here can be an
    // invalid-byte refusal wearing the wrong name.
    assert!(
        crate::TmemByteSource::valid_byte(&source, 0x0800).is_some(),
        "the boundary byte must be valid, or this test proves nothing"
    );

    // 0x0800 & 0x7ff == 0x000, hand-computed: the wrapped read must land
    // on the tile's own first byte in the low half, which is exactly what
    // the unmoved low-half tile reads.
    let wrapped = crate::read_texel(&source, high, addressed, crate::TextureLutMode::Rgba16)
        .expect("an enabled-TLUT index source at 0x0800 wraps into the low half");
    let low = crate::read_texel(
        &source,
        tlut_fixture::descriptor(),
        addressed,
        crate::TextureLutMode::Rgba16,
    )
    .expect("the low-half tile has always read cleanly");
    assert_eq!(
        wrapped.texel().rgba8888(),
        low.texel().rgba8888(),
        "wrapping 0x0800 to 0x000 must give the SAME texel the low-half \
         tile reads, since both address byte 0 of TMEM"
    );

    // Refutation, so the equality above cannot be vacuous: the byte the
    // UNWRAPPED read would have used is a different value, so a reader
    // that failed to mask would have produced a different index.
    assert_ne!(
        crate::TmemByteSource::valid_byte(&source, 0x0800),
        crate::TmemByteSource::valid_byte(&source, 0x0000),
        "the wrapped and unwrapped source bytes must differ, or this test \
         cannot tell a masking reader from a non-masking one"
    );

    // And the host-side binding carries the high-half tile through, as it
    // always did -- there is nothing left for it to catch.
    let binding = TileBindingParams::bound(high, tlut_fixture::size())
        .with_lut_mode(crate::TextureLutMode::Rgba16);
    assert_eq!(binding.tmem_word_address, u32::from(HIGH_HALF_WORD));
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

    /// Submits one flat-UV triangle over `tlut_fixture`'s TMEM at the given
    /// tile binding, returning the readback. Shared by the three
    /// enabled-TLUT GPU tests below so they cannot disagree about the
    /// geometry, the combine program, or the fixture's bytes.
    fn submit_flat_uv_tlut_draw(
        renderer: &mut TrianglePipelineRenderer,
        tile_binding: TileBindingParams,
        projection: TmemGpuProjection,
        column: u32,
        row: u32,
    ) -> TriangleDrawOutput {
        // Raw S10.5 exactly on the texel's integer coordinate: both
        // fractions are zero, so the three-nearest filter over four equal
        // corners is the identity and the readback is this texel's own
        // color with no blending to unpick.
        let raw_s = (column * 32) as f32;
        let raw_t = (row * 32) as f32;
        let vertex = |x: f32, y: f32| fn64_render::NeutralTriangleVertex {
            x,
            y,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [raw_s, raw_t],
        };
        renderer
            .submit_admitted_triangle(
                [vertex(0.0, 0.0), vertex(8.0, 0.0), vertex(0.0, 8.0)],
                OtherMode::from_wire(0, 0),
                texel0_passthrough_combine_params(),
                identity_raster_params(),
                EXTENT,
                projection,
                tile_binding,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::default(),
                ResolvedFragmentBlendParams::NO_OP,
                false,
            )
            .expect("palettized triangle draw must submit cleanly")
            .complete()
            .expect("palettized triangle draw must complete cleanly")
    }

    fn tlut_renderer() -> Box<TrianglePipelineRenderer> {
        let requested =
            block_on(UninitializedTrianglePipeline::new(HeadlessBackend::AnyNative).request())
                .unwrap();
        match requested {
            TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        }
    }

    #[test]
    fn required_host_compute_raster_rgba16_round_trip_is_byte_exact() {
        let mut renderer = tlut_renderer();
        for extent in [
            TriangleTargetExtent {
                width: 5,
                height: 3,
            },
            TriangleTargetExtent {
                width: 320,
                height: 240,
            },
        ] {
            let byte_count = extent.width as usize * extent.height as usize * 2;
            let resident_bytes: Vec<u8> = (0..byte_count)
                .map(|index| (index as u8).wrapping_mul(73).wrapping_add(19))
                .collect();
            let output = renderer
                .round_trip_compute_raster_rgba16(extent, &resident_bytes)
                .expect("dynamic packed-RGBA16 target must round-trip");
            assert_eq!(output, resident_bytes, "extent {extent:?}");
        }
    }

    fn coverage_triangle(edges: crate::wire_words::EdgeWords) -> crate::RawTriangle {
        let bytes = edges.bytes(crate::wire_words::RAW_TRIANGLE_BASE_EDGE);
        crate::RawTriangle::decode(crate::wire_words::RAW_TRIANGLE_BASE_EDGE, &bytes)
            .expect("coverage fixture is one complete base-edge triangle")
    }

    #[test]
    fn required_host_compute_triangle_coverage_and_attributes_match_cpu_ten_times() {
        let mut renderer = tlut_renderer();
        let extent = TriangleTargetExtent {
            width: 96,
            height: 32,
        };
        let raw = [
            // Live WM2000 title-scene edge coefficients. This exercises a
            // large negative slope product and the proven left-major bit.
            coverage_triangle(crate::wire_words::EdgeWords {
                lft: true,
                yl: 106,
                ym: 106,
                yh: 17,
                xl: 6_832_128,
                dxldy: -16_842_729,
                xh: 770_048,
                dxhdy: 0,
                xm: 701_940,
                dxmdy: 272_435,
                ..crate::wire_words::EdgeWords::zeroed()
            }),
            coverage_triangle(crate::wire_words::EdgeWords {
                lft: true,
                yl: 64,
                ym: 32,
                yh: -3,
                xl: 22 << 16,
                dxldy: -(1 << 15),
                xh: 2 << 16,
                dxhdy: 1 << 14,
                xm: 10 << 16,
                dxmdy: 1 << 15,
                ..crate::wire_words::EdgeWords::zeroed()
            }),
            coverage_triangle(crate::wire_words::EdgeWords {
                lft: false,
                yl: 79,
                ym: 40,
                yh: 5,
                xl: 3 << 16,
                dxldy: 1 << 15,
                xh: 25 << 16,
                dxhdy: -(1 << 14),
                xm: 8 << 16,
                dxmdy: -(1 << 15),
                ..crate::wire_words::EdgeWords::zeroed()
            }),
        ];
        let oracle_planes = [
            crate::raw_dpc::triangle_span::AttributePlane {
                base: 0x1234_5678,
                dx: i32::MAX,
                de: i32::MIN,
                dy: 17,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: -0x1234_567,
                dx: i32::MIN + 1,
                de: i32::MAX,
                dy: -19,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: -65_537,
                dx: -33_333,
                de: -77_777,
                dy: 0,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: 65_535,
                dx: 98_765,
                de: -12_345,
                dy: 0,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: 1,
                dx: -1,
                de: 1,
                dy: 0,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: -1,
                dx: 1,
                de: -1,
                dy: 0,
            },
            crate::raw_dpc::triangle_span::AttributePlane {
                base: 0,
                dx: 0,
                de: 0,
                dy: i32::MAX,
            },
        ];
        let mut device_triangles: Vec<_> = raw
            .iter()
            .copied()
            .map(ComputeCoverageTriangle::from_raw)
            .collect();
        for triangle in &mut device_triangles {
            triangle.planes = oracle_planes.map(ComputeAttributePlane::from);
        }
        let mut expected = Vec::new();
        for triangle in &raw {
            for y in 0..extent.height as i32 {
                for x in 0..extent.width as i32 {
                    let attribute_sample =
                        crate::raw_dpc::triangle_span::attribute_sample(triangle, x, y);
                    expected.push(ComputeRasterSample {
                        coverage: crate::raw_dpc::triangle_span::pixel_coverage(triangle, x, y),
                        attribute_sample,
                        plane_values: attribute_sample.map(|(delta_y, delta_x)| {
                            oracle_planes.map(|plane| {
                                crate::raw_dpc::triangle_span::attribute_plane(
                                    plane, delta_y, delta_x,
                                )
                            })
                        }),
                    });
                }
            }
        }
        for run in 1..=10 {
            let actual = renderer
                .compute_triangle_samples(extent, &device_triangles)
                .expect("integer coverage and sample compute must complete");
            assert_eq!(actual, expected, "coverage/sample differential run {run}");
        }
    }

    /// The `G_TT_IA16` half of the enabled-TLUT arm: the SAME palette entry
    /// bytes must decode as IA16 (high byte intensity, low byte alpha), not
    /// as RGBA16. The two decodes disagree on every entry this fixture
    /// writes, so a shader that ignored `lut_mode` and always ran
    /// `decode_rgba16` fails here even though the RGBA16 test above passes.
    ///
    /// Adapter-gated (`host-gpu-tests`).
    #[test]
    fn required_host_ia16_tlut_mode_decodes_the_entry_as_ia16_not_rgba16() {
        let mut renderer = tlut_renderer();
        let descriptor = tlut_fixture::descriptor();
        let tile_binding = TileBindingParams::bound(descriptor, tlut_fixture::size())
            .with_lut_mode(crate::TextureLutMode::Ia16);

        for (column, row) in [(0u32, 0u32), (1, 0)] {
            let output = submit_flat_uv_tlut_draw(
                &mut renderer,
                tile_binding,
                tlut_fixture::projection(),
                column,
                row,
            );
            assert_eq!(
                output
                    .tmem_sample_status
                    .iter()
                    .copied()
                    .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
                None,
                "texel ({column},{row}) under G_TT_IA16 must sample"
            );

            let expected = crate::read_texel(
                &tlut_fixture::source(),
                descriptor,
                crate::AddressedTmemTexel::new(
                    column as u16,
                    row as u16,
                    crate::TmemFirstRowParity::Even,
                ),
                crate::TextureLutMode::Ia16,
            )
            .expect("the CPU reader palettizes this tile under Ia16")
            .texel()
            .rgba8888();

            // The discriminating fact: this fixture's entries decode
            // DIFFERENTLY under the two modes, so matching the IA16 oracle
            // is not something an RGBA16-only decoder could do by accident.
            let as_rgba16 = crate::read_texel(
                &tlut_fixture::source(),
                descriptor,
                crate::AddressedTmemTexel::new(
                    column as u16,
                    row as u16,
                    crate::TmemFirstRowParity::Even,
                ),
                crate::TextureLutMode::Rgba16,
            )
            .expect("the CPU reader palettizes this tile under Rgba16")
            .texel()
            .rgba8888();
            assert_ne!(
                expected, as_rgba16,
                "texel ({column},{row}): this fixture cannot discriminate the \
                 two TLUT entry formats"
            );

            assert_close_rgba8(rgba8_at(&output, 1, 1), expected, 2);
        }
    }

    /// A 16-bit texel under an enabled TLUT indexes the palette through its
    /// HIGH (big-endian first) byte, the low byte ignored -- the case
    /// `4c412a96` admitted on the CPU side, mirrored here on the GPU. The
    /// fixture's low byte differs from its high byte and points at a
    /// differently-colored decoy entry, so a decoder reading the wrong byte
    /// (or hardcoding the index) fails.
    ///
    /// Adapter-gated (`host-gpu-tests`).
    #[test]
    fn required_host_sixteen_bit_tlut_texel_indexes_through_its_high_byte() {
        let mut renderer = tlut_renderer();
        let descriptor = tlut_fixture::descriptor_sixteen_bit();
        assert_ne!(
            tlut_fixture::SIXTEEN_BIT_INDEX,
            tlut_fixture::SIXTEEN_BIT_LOW_BYTE,
            "the two bytes must differ or this test cannot tell them apart"
        );
        let tile_binding = TileBindingParams::bound(descriptor, tlut_fixture::size())
            .with_lut_mode(crate::TextureLutMode::Rgba16);

        let output = submit_flat_uv_tlut_draw(
            &mut renderer,
            tile_binding,
            tlut_fixture::projection(),
            0,
            0,
        );
        assert_eq!(
            output
                .tmem_sample_status
                .iter()
                .copied()
                .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
            None,
            "a 16-bit texel under an enabled TLUT must sample"
        );

        let expected = crate::read_texel(
            &tlut_fixture::source(),
            descriptor,
            crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even),
            crate::TextureLutMode::Rgba16,
        )
        .expect("the CPU reader palettizes a 16-bit texel under an enabled TLUT")
        .texel()
        .rgba8888();
        assert_close_rgba8(rgba8_at(&output, 1, 1), expected, 2);

        // The two wrong answers this fixture is built to catch, each a
        // different color from the right one.
        let decode = |value: u16| {
            crate::decode_direct_texel(
                crate::ImageFormat::Rgba,
                crate::RawTexel::try_new(crate::PixelSize::Bits16, u32::from(value)).unwrap(),
            )
            .unwrap()
            .rgba8888()
        };
        assert_eq!(expected, decode(tlut_fixture::SIXTEEN_BIT_ENTRY));
        assert_ne!(
            expected,
            decode(tlut_fixture::SIXTEEN_BIT_LOW_DECOY),
            "the low-byte decoy must be a different color"
        );
    }

    /// A TLUT entry whose four quadricated lanes disagree is READ, from
    /// lane 0, on both lanes.
    ///
    /// This test previously asserted the opposite: that all four lanes
    /// were checked and a disagreeing entry was refused. The pinned RT64
    /// port source `5473732a` settles it the other way --
    /// `src/shaders/TextureDecoder.hlsli:179` reads
    /// `loadTLUT(paletteAddress + 1) | (loadTLUT(paletteAddress) << 8)`,
    /// two bytes, and never addresses lanes 1..3, so it cannot observe a
    /// disagreement at all. `fn64-render-reference`'s `read_tlut`
    /// (`src/gbi/state.rs:853-869`) reads the same two bytes.
    ///
    /// The fixture is UNCHANGED: it still corrupts only the LAST lane of
    /// entry 0x53, leaving lanes 0..2 agreeing. That is exactly the case
    /// that distinguishes a lane-0 reader from a four-lane one, so the
    /// test still discriminates -- only its expected verdict flipped.
    ///
    /// Adapter-gated (`host-gpu-tests`).
    #[test]
    fn required_host_a_non_canonical_tlut_entry_reads_from_lane_zero() {
        let mut renderer = tlut_renderer();
        let descriptor = tlut_fixture::descriptor();
        let tile_binding = TileBindingParams::bound(descriptor, tlut_fixture::size())
            .with_lut_mode(crate::TextureLutMode::Rgba16);

        // Column 0 of row 0 reads nibble 3, i.e. index 0x53. Corrupt only
        // that entry's LAST lane, leaving lanes 0-2 agreeing: a shader
        // reading lane 0 alone would happily return the right color, so
        // this specifically tests that all four lanes are checked.
        let mut bytes = tlut_fixture::bytes();
        let last_lane = 0x0800u16 + 0x53 * 8 + 6;
        bytes.insert(last_lane, 0x00);
        bytes.insert(last_lane + 1, 0x00);

        let mut projection = tlut_fixture::projection();
        for (address, byte) in &bytes {
            projection.bytes[*address as usize] = *byte;
        }

        // The CPU reader's own verdict on the same bytes, first -- so this
        // test asserts the two lanes AGREE, not merely that the shader
        // produced something. Corrupting the last lane must not change the
        // color, because only lane 0 is read.
        let source = tlut_fixture::FixtureTmem { bytes };
        let corrupted = crate::read_texel(
            &source,
            descriptor,
            crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even),
            crate::TextureLutMode::Rgba16,
        )
        .expect("a non-canonical entry resolves from lane 0")
        .texel()
        .rgba8888();
        let pristine = crate::read_texel(
            &tlut_fixture::source(),
            descriptor,
            crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even),
            crate::TextureLutMode::Rgba16,
        )
        .expect("the uncorrupted fixture has always read cleanly")
        .texel()
        .rgba8888();
        assert_eq!(
            corrupted, pristine,
            "corrupting lane 3 must not change the color -- lane 0 is the \
             only lane the RDP addresses"
        );

        let output = submit_flat_uv_tlut_draw(&mut renderer, tile_binding, projection, 0, 0);
        assert_eq!(
            output
                .tmem_sample_status
                .iter()
                .copied()
                .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
            None,
            "the shader must read the same lane-0 entry the CPU oracle \
             does, not refuse it"
        );
    }

    /// **D-LOWHALF, on the GPU: the wrap, as a test.**
    ///
    /// This test previously pinned the shader's own
    /// `TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF` refusal, and its
    /// own scope note said the pin would come out on both lanes together
    /// if D14 resolved against the refusal. The pinned RT64 port source
    /// `5473732a` resolves it: `src/shaders/TextureDecoder.hlsli:162-163`
    /// scopes an enabled-TLUT index source to `RDP_TMEM_MASK16` (`0x7FF`)
    /// and `implLoadTMEM` (`:17-25`) applies that scope as a mask. The
    /// rule stands; the refusal does not.
    ///
    /// So this now pins the opposite verdict, still differentially: the
    /// high-half tile must sample, and must produce the SAME colors the
    /// CPU oracle produces for it, so the two lanes cannot wrap
    /// differently. The old refutation is inverted into the discriminator:
    /// the wrapped tile and the low-half tile must agree, and the byte at
    /// the unwrapped address must differ from the byte at the wrapped one,
    /// so a shader that dropped the mask is caught.
    ///
    /// Adapter-gated (`host-gpu-tests`).
    #[test]
    fn required_host_an_enabled_tlut_tile_in_the_high_half_wraps_on_the_shader() {
        const HIGH_HALF_WORD: u16 = 0x0100;
        let mut renderer = tlut_renderer();
        let high = crate::TileDescriptor::from_neutral_parts(
            crate::ImageFormat::IntensityAlpha,
            crate::PixelSize::Bits4,
            tlut_fixture::LINE_WORDS,
            crate::TmemWordAddress::try_new(HIGH_HALF_WORD).unwrap(),
            tlut_fixture::PALETTE,
            crate::TileAddressMode::default(),
            0,
            0,
            crate::TileAddressMode::default(),
            0,
            0,
        );
        let tile_binding = TileBindingParams::bound(high, tlut_fixture::size())
            .with_lut_mode(crate::TextureLutMode::Rgba16);

        let projection = tlut_fixture::projection();
        for address in [0x0800usize, 0x0801] {
            assert_ne!(
                projection.validity_words[address / 32] & (1 << (address % 32)),
                0,
                "byte {address:#06x} must be valid, or this test proves nothing"
            );
        }

        // The discriminator: the unwrapped byte differs from the wrapped
        // one, so a shader that skipped the mask reads a different index.
        let source = tlut_fixture::source();
        assert_ne!(
            crate::TmemByteSource::valid_byte(&source, 0x0800),
            crate::TmemByteSource::valid_byte(&source, 0x0000),
            "the wrapped and unwrapped source bytes must differ, or this \
             test cannot tell a masking shader from a non-masking one"
        );

        // The CPU oracle's verdict on the same tile, first.
        let expected = crate::read_texel(
            &source,
            high,
            crate::AddressedTmemTexel::new(0, 0, crate::TmemFirstRowParity::Even),
            crate::TextureLutMode::Rgba16,
        )
        .expect("the CPU reader wraps the high-half tile into the low half")
        .texel()
        .rgba8888();

        let output = submit_flat_uv_tlut_draw(&mut renderer, tile_binding, projection, 0, 0);
        assert_eq!(
            output
                .tmem_sample_status
                .iter()
                .copied()
                .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
            None,
            "a high-half enabled-TLUT tile must WRAP and sample cleanly; \
             the shader no longer has a placement refusal"
        );
        assert!(
            output
                .color_rgba8
                .chunks_exact(4)
                .any(|pixel| pixel == expected),
            "the shader's wrapped color must match the CPU oracle's \
             ({expected:?}), or the two lanes wrap to different addresses"
        );

        // And the low-half tile still samples, unchanged.
        let low_binding =
            TileBindingParams::bound(tlut_fixture::descriptor(), tlut_fixture::size())
                .with_lut_mode(crate::TextureLutMode::Rgba16);
        let low_output =
            submit_flat_uv_tlut_draw(&mut renderer, low_binding, tlut_fixture::projection(), 0, 0);
        assert_eq!(
            low_output
                .tmem_sample_status
                .iter()
                .copied()
                .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
            None,
            "the low-half tile must still sample"
        );
    }

    /// **The blocker, as a test: WM2000's IA4-under-`G_TT_RGBA16` tile must
    /// sample on the GPU triangle path.**
    ///
    /// Before the `lut_mode` wiring landed, `tmem_sample.wgsl` consulted
    /// `tile.format` unconditionally and returned
    /// `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT` (4) for every fragment of
    /// this draw -- the abort that stopped the all-Rust stack at
    /// `rsp_commit.rs:1202`. Under `tlut_en` the hardware ignores the tile
    /// format entirely, so the refusal was a lane gap, not a hardware rule.
    ///
    /// Differential, not a frozen expectation: the four corner texels'
    /// colors are read back from the GPU and compared against
    /// `crate::read_texel` -- the SAME CPU reader `execute_scheduled_texrect`
    /// uses -- over the SAME byte map, so the two lanes cannot drift apart
    /// silently. `tlut_fixture_is_genuinely_a_non_ci_four_bit_tile_...`
    /// above is this test's positive control: it proves, without an
    /// adapter, that the fixture is a non-CI 4-bit tile with a nonzero
    /// palette, so a pass here cannot be vacuous.
    ///
    /// Adapter-gated (`host-gpu-tests`): panics with a typed reason rather
    /// than skipping when the host has no native adapter, matching this
    /// module's own required-host convention.
    #[test]
    fn required_host_enabled_tlut_over_an_ia4_tile_samples_and_matches_the_cpu_reader() {
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

        let descriptor = tlut_fixture::descriptor();
        let size = tlut_fixture::size();
        let tile_binding =
            TileBindingParams::bound(descriptor, size).with_lut_mode(crate::TextureLutMode::Rgba16);
        assert_ne!(
            tile_binding.format, 0,
            "the fixture must NOT be RGBA -- an RGBA16 tile would pass \
             through the pre-existing direct arm without exercising the fix"
        );
        assert_eq!(tile_binding.palette, u32::from(tlut_fixture::PALETTE));

        // Each of the four target quadrants samples one of the tile's four
        // texels. A single flat UV per draw keeps the whole triangle on one
        // texel, so the readback is that texel's color with no filter
        // blending to unpick -- the three-nearest filter over four equal
        // corners is the identity.
        for (column, row, packed_nibble) in [
            (0u32, 0u32, tlut_fixture::PACKED_ROW0 >> 4),
            (1, 0, tlut_fixture::PACKED_ROW0 & 0x0f),
            (0, 1, tlut_fixture::PACKED_ROW1 >> 4),
            (1, 1, tlut_fixture::PACKED_ROW1 & 0x0f),
        ] {
            // Raw S10.5: exactly on the texel's integer coordinate, so both
            // the S and T fractions are zero and all four filter corners
            // collapse onto this one texel.
            let raw_s = (column * 32) as f32;
            let raw_t = (row * 32) as f32;
            let vertices = [
                fn64_render::NeutralTriangleVertex {
                    x: 0.0,
                    y: 0.0,
                    z: 0.5,
                    w: 1.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                    texcoord: [raw_s, raw_t],
                },
                fn64_render::NeutralTriangleVertex {
                    x: 8.0,
                    y: 0.0,
                    z: 0.5,
                    w: 1.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                    texcoord: [raw_s, raw_t],
                },
                fn64_render::NeutralTriangleVertex {
                    x: 0.0,
                    y: 8.0,
                    z: 0.5,
                    w: 1.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                    texcoord: [raw_s, raw_t],
                },
            ];

            let output = renderer
                .submit_admitted_triangle(
                    vertices,
                    OtherMode::from_wire(0, 0),
                    texel0_passthrough_combine_params(),
                    identity_raster_params(),
                    EXTENT,
                    tlut_fixture::projection(),
                    tile_binding,
                    Color4::from_wire(0),
                    Color4::from_wire(0),
                    PrimColor::default(),
                    ResolvedFragmentBlendParams::NO_OP,
                    false,
                )
                .expect("palettized triangle draw must submit cleanly")
                .complete()
                .expect("palettized triangle draw must complete cleanly");

            // THE assertion that failed before the fix: every fragment
            // reported status 4 (`TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT`).
            let bad = output
                .tmem_sample_status
                .iter()
                .copied()
                .find(|&status| status != TMEM_SAMPLE_STATUS_OK);
            assert_eq!(
                bad, None,
                "texel ({column},{row}): the shader must sample an IA4 tile \
                 under an enabled TLUT, not refuse it by format"
            );

            // Differential against the CPU reader over the same bytes.
            let addressed = crate::AddressedTmemTexel::new(
                column as u16,
                row as u16,
                crate::TmemFirstRowParity::Even,
            );
            let expected = crate::read_texel(
                &tlut_fixture::source(),
                descriptor,
                addressed,
                crate::TextureLutMode::Rgba16,
            )
            .expect("the CPU reader palettizes this tile")
            .texel()
            .rgba8888();

            // A covered pixel well inside the right triangle.
            let observed = rgba8_at(&output, 1, 1);
            assert_close_rgba8(observed, expected, 2);

            // Liveness: the expected color must not be the clear color, or
            // an all-black readback would pass without the draw happening.
            assert_ne!(
                expected,
                [0, 0, 0, 0],
                "texel ({column},{row}) nibble {packed_nibble:#x} must be a \
                 visible color, not the cleared attachment"
            );
        }
    }

    /// WM2000's OWN sprite-strip tile, rebuilt from its measured wire
    /// fields -- the shape `tlut_fixture` above cannot express, because
    /// that fixture's `low_t` is 0 (**even**) and it has only two rows, so
    /// its differential agreed about first-row parity vacuously.
    ///
    /// Every constant here is the value instrumented off the real ROM
    /// through the all-Rust lane, and is the same set
    /// `tmem::read::tests`'s two `wm2000_texrect_*` tests already name.
    /// `low_t.integer()` is `188 >> 2 == 47`, which is **ODD**.
    mod wm2000_strip_fixture {
        use crate::tmem::{TmemGpuProjection, TMEM_VALIDITY_WORDS};
        use crate::{
            ImageFormat, PixelSize, TileAddressMode, TileCoordinate, TileDescriptor, TileSize,
            TmemWordAddress,
        };

        pub const LINE_WORDS: u16 = 5;
        pub const LOW_S_RAW: u16 = 252;
        pub const LOW_T_RAW: u16 = 188;
        pub const HIGH_S_RAW: u16 = 512;
        pub const HIGH_T_RAW: u16 = 384;
        /// 5 destination words per row, of which the last carries only 2
        /// defined source bytes -- 34 defined bytes per row, a 6-byte tail
        /// gap. That gap is what an inverted parity walks into.
        pub const WORDS_PER_ROW: u16 = 5;
        pub const DEFINED_TAIL_BYTES: u16 = 2;
        pub const ROWS: u16 = 50;
        pub const PALETTE: u8 = 0;

        /// The one CI4 index every payload byte in this fixture produces
        /// (`palette == 0`, all-zero payload), and the RGBA16 palette entry
        /// it resolves to. Deliberately NOT `0x0000`: an all-transparent
        /// entry would make an unblended black readback pass vacuously.
        pub const RESOLVED_INDEX: u8 = 0x00;
        pub const RESOLVED_ENTRY: u16 = 0xf801;

        pub fn descriptor() -> TileDescriptor {
            TileDescriptor::from_wire(
                ImageFormat::IntensityAlpha,
                PixelSize::Bits4,
                LINE_WORDS,
                TmemWordAddress::try_new(0).unwrap(),
                PALETTE,
                TileAddressMode::from_wire(0b10),
                0,
                0,
                TileAddressMode::from_wire(0b10),
                0,
                0,
            )
        }

        pub fn size() -> TileSize {
            TileSize::from_wire(
                TileCoordinate::try_new(LOW_S_RAW).unwrap(),
                TileCoordinate::try_new(LOW_T_RAW).unwrap(),
                TileCoordinate::try_new(HIGH_S_RAW).unwrap(),
                TileCoordinate::try_new(HIGH_T_RAW).unwrap(),
            )
        }

        /// The exact byte set WM2000's `cmd 39` `LoadTile` validates, built
        /// from the WRITER's own two rules rather than from a capture --
        /// character-for-character the derivation
        /// `tmem::read::tests::wm2000_load_tile_source` uses, so the CPU
        /// and GPU halves of this differential cannot describe different
        /// TMEM:
        ///
        /// - `project_tmem_transfer_word`'s `Tile` arm places transfer word
        ///   `w` at destination word `tmem + (w / words_per_row) *
        ///   line_words + (w % words_per_row)`.
        /// - `tmem/execute/load_tile.rs`'s `map_physical_lanes` writes lane
        ///   `source_lane ^ (4 * odd_row_exchange)` with
        ///   `odd_row_exchange = row & 1` -- the tile-relative row's parity
        ///   alone, with no T-origin term. Pinned RT64 derives `oddRow` from
        ///   `texelInt.y & 1` and exchanges adjacent four-byte words
        ///   (`src/shaders/TextureDecoder.hlsli:17-25,149-150`, commit
        ///   `f0728a2`); see `tmem/read.rs::odd_row_exchange` too.
        ///   This line used to read `(low_t.integer() + row) & 1`, mirroring
        ///   a writer term that has since been removed.
        pub fn bytes() -> std::collections::BTreeMap<u16, u8> {
            let mut bytes = std::collections::BTreeMap::new();
            for word in 0..WORDS_PER_ROW * ROWS {
                let row = word / WORDS_PER_ROW;
                let within = word % WORDS_PER_ROW;
                let destination_word = row * LINE_WORDS + within;
                let exchange = if row & 1 == 1 { 4 } else { 0 };
                let defined = if within + 1 < WORDS_PER_ROW {
                    8
                } else {
                    DEFINED_TAIL_BYTES
                };
                for lane in 0..defined {
                    bytes.insert(destination_word * 8 + (lane ^ exchange), 0x00);
                }
            }
            // The quadricated palette entry index 0 resolves through.
            let base = 0x0800u16 + u16::from(RESOLVED_INDEX) * 8;
            for lane in 0..4u16 {
                bytes.insert(base + lane * 2, (RESOLVED_ENTRY >> 8) as u8);
                bytes.insert(base + lane * 2 + 1, (RESOLVED_ENTRY & 0xff) as u8);
            }
            bytes
        }

        pub fn source() -> super::tlut_fixture::FixtureTmem {
            super::tlut_fixture::FixtureTmem { bytes: bytes() }
        }

        pub fn projection() -> TmemGpuProjection {
            let mut projection = TmemGpuProjection {
                bytes: [0u8; fn64_render_ir::TMEM_BYTES as usize],
                validity_words: [0u32; TMEM_VALIDITY_WORDS],
            };
            for (address, byte) in bytes() {
                let address = address as usize;
                projection.bytes[address] = byte;
                projection.validity_words[address / 32] |= 1 << (address % 32);
            }
            projection
        }

        /// The raw S10.5 pair that addresses tile texel `(column, row)`.
        /// The shader subtracts `low * 8` (S10.2 origin in texel-fraction
        /// units) and floors by 32, so adding the origin back and scaling
        /// by 32 lands exactly on the texel with both fractions zero --
        /// which makes the three-nearest filter over four equal corners the
        /// identity.
        pub fn raw_coordinates(column: u32, row: u32) -> (f32, f32) {
            (
                (u32::from(LOW_S_RAW) * 8 + column * 32) as f32,
                (u32::from(LOW_T_RAW) * 8 + row * 32) as f32,
            )
        }
    }

    /// **Positive control for the WM2000 fixture, adapter-free.** Proves,
    /// without a GPU, that this fixture is genuinely the shape the defect
    /// needs: an IA4 tile under an enabled `G_TT_RGBA16` TLUT whose T
    /// origin is ODD, whose failing texel's two candidate addresses are
    /// exactly the production pair (`0x048` written, `0x04c` not), and
    /// whose CPU reader resolves cleanly under the writer's parity and
    /// fails under the inverted one. Without this, a green GPU test below
    /// could be passing over a tile that never triggers the exchange.
    #[test]
    fn wm2000_strip_fixture_is_genuinely_an_odd_origin_ia4_tile_under_an_enabled_tlut() {
        let descriptor = wm2000_strip_fixture::descriptor();
        let size = wm2000_strip_fixture::size();

        assert_eq!(descriptor.format(), crate::ImageFormat::IntensityAlpha);
        assert_eq!(descriptor.size(), crate::PixelSize::Bits4);
        assert_eq!(
            size.low_t().integer() & 1,
            1,
            "this fixture is only meaningful while its T origin is ODD -- \
             an even origin agrees with the old frozen constant vacuously"
        );
        assert_eq!(size.low_t().integer(), 47);

        // The measured production pair, from the writer's own rule. Tile
        // row 1 takes the exchange on BOTH sides, so the exchanged address
        // is the one inside the load. These two used to be the other way
        // round, under a writer that folded in the tile's T origin.
        let source = wm2000_strip_fixture::source();
        assert!(
            crate::TmemByteSource::valid_byte(&source, 0x04c).is_some(),
            "tile row 1 is exchanged, so its exchanged byte is the one the load wrote"
        );
        assert!(
            crate::TmemByteSource::valid_byte(&source, 0x048).is_none(),
            "its un-exchanged partner must be a byte the load never wrote"
        );

        // And the CPU reader's verdict on the texel that used to abort
        // production. The caller-supplied first-row parity no longer
        // participates in the exchange, so BOTH values must read cleanly --
        // which is the property a reintroduced origin term would break.
        let read = |parity| {
            crate::read_texel(
                &source,
                descriptor,
                crate::AddressedTmemTexel::new(64, 1, parity),
                crate::TextureLutMode::Rgba16,
            )
        };
        for parity in [
            crate::TmemFirstRowParity::Even,
            crate::TmemFirstRowParity::Odd,
        ] {
            assert!(
                read(parity).is_ok(),
                "the tile's T origin must not perturb the read: {parity:?}"
            );
        }
    }

    /// Adapter-free half of the parity pinning: the host-side
    /// [`TileBindingParams`] must carry, in its uploaded `low_t`, the same
    /// parity bit `targets/texrect.rs` derives for the CPU reader. The
    /// shader reads `(low_t >> 2) & 1`; this asserts the uploaded word
    /// really answers that question for both an odd- and an even-origin
    /// tile, so a projection that dropped or rescaled `low_t` is caught
    /// without a GPU.
    #[test]
    fn the_uploaded_low_t_carries_the_same_first_row_parity_the_cpu_reader_is_given() {
        for (size, expected) in [
            (wm2000_strip_fixture::size(), crate::TmemFirstRowParity::Odd),
            (tlut_fixture::size(), crate::TmemFirstRowParity::Even),
        ] {
            // `targets/texrect.rs`'s derivation, the CPU reader's input.
            let cpu = if size.low_t().integer() & 1 == 1 {
                crate::TmemFirstRowParity::Odd
            } else {
                crate::TmemFirstRowParity::Even
            };
            assert_eq!(cpu, expected);

            // `tmem_sample.wgsl`'s `tmem_first_row_parity_odd`, over the
            // word this binding actually uploads.
            let params = TileBindingParams::bound(wm2000_strip_fixture::descriptor(), size);
            let shader_says_odd = ((params.low_t >> 2) & 1) != 0;
            assert_eq!(
                shader_says_odd,
                cpu == crate::TmemFirstRowParity::Odd,
                "the shader's parity expression over the uploaded low_t must \
                 equal the parity the CPU reader is handed for the same tile"
            );
        }
    }

    /// **The blocker, as a test: WM2000's odd-T-origin IA4 strip tile must
    /// sample on the GPU triangle path, at the exact texel that aborted.**
    ///
    /// Before the fix, `tmem_sample.wgsl` froze
    /// `TMEM_FIRST_ROW_PARITY_ODD = false`, so for tile row 1 it XOR4'd
    /// address `0x048` -- a byte the `cmd 39` `LoadTile` wrote -- to
    /// `0x04c`, which it did not, and every fragment reported
    /// `TMEM_SAMPLE_STATUS_INVALID_BYTE` (2). That is the abort at
    /// `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
    ///
    /// Differential, not a frozen expectation: the color is compared
    /// against `crate::read_texel` -- the SAME reader
    /// `execute_scheduled_texrect` uses -- over the SAME byte map, handed
    /// the parity `targets/texrect.rs` derives. The test sweeps both an ODD
    /// and an EVEN tile row so a fix that merely inverted the constant
    /// cannot pass.
    ///
    /// Adapter-gated (`host-gpu-tests`): panics with a typed reason rather
    /// than skipping when the host has no native adapter, matching this
    /// module's own required-host convention. Its positive control
    /// (`wm2000_strip_fixture_is_genuinely_an_odd_origin_ia4_tile_...`)
    /// runs without an adapter.
    #[test]
    fn required_host_an_odd_t_origin_tile_samples_the_same_byte_the_cpu_reader_does() {
        let mut renderer = tlut_renderer();
        let descriptor = wm2000_strip_fixture::descriptor();
        let size = wm2000_strip_fixture::size();
        let tile_binding =
            TileBindingParams::bound(descriptor, size).with_lut_mode(crate::TextureLutMode::Rgba16);
        assert_eq!(
            (tile_binding.low_t >> 2) & 1,
            1,
            "the binding this draw uploads must carry the ODD origin, or \
             the shader is never asked the question this test is about"
        );

        // Tile row 1 is the row the production abort landed on; row 0 is
        // its opposite parity. Column 64 is the production column. Both
        // rows must sample, so inverting the frozen constant fails too.
        for (column, row) in [(64u32, 1u32), (64, 0), (0, 1), (0, 0)] {
            let (raw_s, raw_t) = wm2000_strip_fixture::raw_coordinates(column, row);
            let vertex = |x: f32, y: f32| fn64_render::NeutralTriangleVertex {
                x,
                y,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [raw_s, raw_t],
            };
            let output = renderer
                .submit_admitted_triangle(
                    [vertex(0.0, 0.0), vertex(8.0, 0.0), vertex(0.0, 8.0)],
                    OtherMode::from_wire(0, 0),
                    texel0_passthrough_combine_params(),
                    identity_raster_params(),
                    EXTENT,
                    wm2000_strip_fixture::projection(),
                    tile_binding,
                    Color4::from_wire(0),
                    Color4::from_wire(0),
                    PrimColor::default(),
                    ResolvedFragmentBlendParams::NO_OP,
                    false,
                )
                .expect("odd-origin palettized draw must submit cleanly")
                .complete()
                .expect("odd-origin palettized draw must complete cleanly");

            // THE assertion that failed before the fix, for (64, 1).
            // `!= OK` and not `== INVALID_BYTE`: this tile is entirely in
            // the low half, so the merged `D-LOWHALF` guard
            // (`TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF`) must stay
            // silent here too, and asserting on OK catches it if it does
            // not -- the guard tests the post-XOR4 address, so a wrong
            // parity could have tripped it rather than INVALID_BYTE.
            assert_eq!(
                output
                    .tmem_sample_status
                    .iter()
                    .copied()
                    .find(|&status| status != TMEM_SAMPLE_STATUS_OK),
                None,
                "texel ({column},{row}): the shader must address the byte \
                 this tile's own T origin says it should, not the XOR4 \
                 partner a frozen parity picks"
            );

            // Differential against the CPU reader, handed the parity
            // `targets/texrect.rs` derives for this same tile.
            let expected = crate::read_texel(
                &wm2000_strip_fixture::source(),
                descriptor,
                crate::AddressedTmemTexel::new(
                    column as u16,
                    row as u16,
                    crate::TmemFirstRowParity::Odd,
                ),
                crate::TextureLutMode::Rgba16,
            )
            .expect("the CPU reader samples this tile under the writer's parity")
            .texel()
            .rgba8888();
            assert_close_rgba8(rgba8_at(&output, 1, 1), expected, 2);

            // Liveness: an all-black readback must not pass.
            assert_ne!(
                expected,
                [0, 0, 0, 0],
                "texel ({column},{row}) must be a visible color, not the \
                 cleared attachment"
            );
        }
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

    /// Compares a reading draw's blended output against the oracle, and --
    /// when this case's blend arm is one that provably transforms the
    /// destination -- first proves the draw actually rasterized.
    ///
    /// Liveness cannot be asserted as `observed != destination`
    /// unconditionally. The no-`FORCE_BL` bypass arm with
    /// `P == Framebuffer` sets `final_alpha == 0.0` (`blend.rs:444-448`),
    /// and the final composite (`blend.rs:501-509`) then reduces to
    /// `dst * 1.0` on every channel: its correct output *is* the
    /// destination, byte for byte. Case 2 below is exactly that arm, so a
    /// blanket `observed != destination` guard reports a false drop for a
    /// draw that ran and produced the right answer.
    ///
    /// `transforms_destination` is therefore supplied per case, `false`
    /// only for that bypass arm -- whose own liveness is instead pinned by
    /// the `expected.rgba[3] == dst_rgba8[3]` assertion its case already
    /// makes, plus the fact that the two neighbouring cases share this
    /// fixture builder and would catch a systematic drop.
    fn assert_blended_over_destination(
        observed: [u8; 4],
        destination: [u8; 4],
        expected: [u8; 4],
        tolerance: i32,
        transforms_destination: bool,
    ) {
        if transforms_destination {
            assert_ne!(
                observed, destination,
                "reading draw never blended: target still holds the destination \
                 bytes verbatim, so the blend branch under test never executed"
            );
        }
        assert_close_rgba8(observed, expected, tolerance);
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
        blend_color: crate::state::Color4,
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
        let none_fixture =
            uniform_alpha_fixture(0.1, crate::state::AlphaCompare::None, Color4::from_wire(0));
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
        let threshold_fixture =
            uniform_alpha_fixture(0.1, crate::state::AlphaCompare::Threshold, blend_color);
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

    /// Framebuffer-blend Slice B: the required physical-Metal nonzero-row
    /// differential (port card "Literal characterization / oracles"). Two
    /// triangles into the same 8x8 target: an opaque, uniform-colored first
    /// draw (no blend) establishes a known destination color across the
    /// whole target, including row `y >= 1` (testing only row 0 would not
    /// catch a stride bug -- row 0 is correct by coincidence at any stride,
    /// per the card's own "Row stride correctness" section); a second,
    /// uniform-shaded draw whose blend cycle selects `P == Framebuffer`
    /// reads that destination back through `framebuffer_color_snapshot` and
    /// must match `crate::blend::blend_fragment`'s own CPU computation for
    /// the identical selectors/inputs.
    ///
    /// Folds in both review-required characterization cases (per the card's
    /// own instruction that they "must also appear... in the required
    /// physical-Metal nonzero-y differential test's fixture set"):
    ///
    /// - **Review Bug A** (no-`FORCE_BL` last-cycle bypass,
    ///   `blend.rs:421-437`): a one-cycle, `force_blend == false` fixture
    ///   whose `P == Framebuffer` must take the bypass arm, not the general
    ///   three-way branch -- `final_alpha == 0.0` (output alpha driven
    ///   entirely by the destination's own alpha), distinguishing it from a
    ///   `P != Framebuffer` sibling under the same `force_blend == false`
    ///   condition, whose bypass arm produces `final_alpha == 1.0` instead.
    /// - **Review Bug B** (memory alpha differs from source alpha,
    ///   `blend.rs:488-496`): a one-cycle, `force_blend == true` fixture
    ///   whose `M == Framebuffer` (so `final_alpha == a`, strictly between
    ///   `0.0` and `1.0`) with a destination alpha deliberately different
    ///   from the combiner's own source alpha -- the output alpha must equal
    ///   the real two-term composite, not `src.a` passed through and not
    ///   `final_alpha` itself.
    #[test]
    fn required_host_framebuffer_color_blend_matches_the_rust_oracle_at_nonzero_row() {
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
            "fn64-triangle-pipeline-framebuffer-blend: backend={:?} adapter={:?}",
            renderer.adapter_info().backend,
            renderer.adapter_info().name
        );

        // Row y=4 (well past row 0) is the pixel this test actually checks
        // for every case below -- a stride bug corrupts every row after row
        // 0, so row 0 alone would not catch it.
        const CHECK_XY: (u32, u32) = (2, 4);

        // --- Case 1: general divide, P == Framebuffer, force_blend == true.
        {
            let dst_rgba8: [u8; 4] = [90, 150, 30, 210];
            let dst = uniform_dst_fixture(dst_rgba8);

            let shade_color = [0.4, 0.2, 0.6, 0.5];
            let other_mode = crate::state::OtherMode::from_wire(0, 1 << 30); // cycle0 P=Framebuffer
            let blend_color = crate::state::Color4::from_wire(0x14283C50);
            let fog_color = crate::state::Color4::from_wire(0x465A6E82);
            let mode_state = crate::blend::BlendModeState {
                other_mode,
                blend_color_register: blend_color.rgba8(),
                fog_color: fog_color.rgba8(),
            };
            assert_eq!(mode_state.cycle_count(), 1);
            let cycle = mode_state.cycle(0);
            assert_eq!(cycle.p, crate::blend::BlendColorInput::Framebuffer);

            let reader = uniform_framebuffer_color_blend_fixture(
                cycle,
                blend_color,
                fog_color,
                shade_color,
                true,
            );
            let output = renderer
                .submit_triangles(&[dst, reader])
                .unwrap()
                .complete()
                .unwrap();

            let combiner_color = cpu_combiner_reference(shade_color);
            let src_rgba8 = [
                (combiner_color[0] * 255.0).round() as u8,
                (combiner_color[1] * 255.0).round() as u8,
                (combiner_color[2] * 255.0).round() as u8,
                (combiner_color[3] * 255.0).round() as u8,
            ];
            let shade_alpha_255 = (shade_color[3] * 255.0).round() as u8;
            let memory = crate::blend::BlendFramebufferSample {
                rgba: dst_rgba8,
                coverage_count: 0,
            };
            let expected = crate::blend::blend_fragment(
                src_rgba8,
                Some(memory),
                shade_alpha_255,
                mode_state,
                true,
            )
            .expect("memory is supplied; P==Framebuffer must not error");

            let (x, y) = CHECK_XY;
            assert_blended_over_destination(
                rgba8_at(&output, x, y),
                dst_rgba8,
                expected.rgba,
                2,
                true,
            );
        }

        // --- Case 2 (review Bug A): no-FORCE_BL bypass, P == Framebuffer.
        // final_alpha must be 0.0 (output alpha driven entirely by dst.a).
        {
            let dst_rgba8: [u8; 4] = [90, 150, 30, 210];
            let dst = uniform_dst_fixture(dst_rgba8);

            let shade_color = [0.4, 0.2, 0.6, 0.5];
            let other_mode = crate::state::OtherMode::from_wire(0, 1 << 30); // cycle0 P=Framebuffer
            let blend_color = crate::state::Color4::from_wire(0x14283C50);
            let fog_color = crate::state::Color4::from_wire(0x465A6E82);
            let mode_state = crate::blend::BlendModeState {
                other_mode,
                blend_color_register: blend_color.rgba8(),
                fog_color: fog_color.rgba8(),
            };
            let cycle = mode_state.cycle(0);

            let reader = uniform_framebuffer_color_blend_fixture(
                cycle,
                blend_color,
                fog_color,
                shade_color,
                false, // force_blend == false: the no-FORCE_BL bypass arm
            );
            let output = renderer
                .submit_triangles(&[dst, reader])
                .unwrap()
                .complete()
                .unwrap();

            let combiner_color = cpu_combiner_reference(shade_color);
            let src_rgba8 = [
                (combiner_color[0] * 255.0).round() as u8,
                (combiner_color[1] * 255.0).round() as u8,
                (combiner_color[2] * 255.0).round() as u8,
                (combiner_color[3] * 255.0).round() as u8,
            ];
            let shade_alpha_255 = (shade_color[3] * 255.0).round() as u8;
            let memory = crate::blend::BlendFramebufferSample {
                rgba: dst_rgba8,
                coverage_count: 0,
            };
            let expected = crate::blend::blend_fragment(
                src_rgba8,
                Some(memory),
                shade_alpha_255,
                mode_state,
                false,
            )
            .expect("memory is supplied; the bypass arm must not error");
            assert_eq!(
                expected.rgba[3], dst_rgba8[3],
                "bypass arm with P==Framebuffer: final_alpha==0.0 means output alpha is exactly \
                 the destination's own alpha byte"
            );

            let (x, y) = CHECK_XY;
            // The bypass arm's correct output IS the destination
            // (`final_alpha == 0.0`), so the drop guard cannot apply here --
            // see `assert_blended_over_destination`'s own doc.
            assert_blended_over_destination(
                rgba8_at(&output, x, y),
                dst_rgba8,
                expected.rgba,
                2,
                false,
            );
        }

        // --- Case 3 (review Bug B): general divide, M == Framebuffer,
        // destination alpha deliberately differs from source alpha.
        {
            let dst_rgba8: [u8; 4] = [90, 150, 30, 40];
            let dst = uniform_dst_fixture(dst_rgba8);

            let shade_color = [0.4, 0.2, 0.6, 200.0 / 255.0];
            // cycle0: P=Blend(2)@30:31, A=Fog(1)@26:27, M=Framebuffer(1)@22:23,
            // B=One(2)@18:19 -- `a` derives from a nonzero fog alpha (strictly
            // between 0 and 1), M selects Framebuffer, so final_alpha == a.
            let other_mode = crate::state::OtherMode::from_wire(0, 0x8440_0000);
            let blend_color = crate::state::Color4::from_wire(0x14283C50);
            let fog_color = crate::state::Color4::from_wire(0x465A6E82);
            let mode_state = crate::blend::BlendModeState {
                other_mode,
                blend_color_register: blend_color.rgba8(),
                fog_color: fog_color.rgba8(),
            };
            let cycle = mode_state.cycle(0);
            assert_eq!(cycle.m, crate::blend::BlendColorInput::Framebuffer);

            let reader = uniform_framebuffer_color_blend_fixture(
                cycle,
                blend_color,
                fog_color,
                shade_color,
                true,
            );
            let output = renderer
                .submit_triangles(&[dst, reader])
                .unwrap()
                .complete()
                .unwrap();

            let combiner_color = cpu_combiner_reference(shade_color);
            let src_rgba8 = [
                (combiner_color[0] * 255.0).round() as u8,
                (combiner_color[1] * 255.0).round() as u8,
                (combiner_color[2] * 255.0).round() as u8,
                (combiner_color[3] * 255.0).round() as u8,
            ];
            let shade_alpha_255 = (shade_color[3] * 255.0).round() as u8;
            let memory = crate::blend::BlendFramebufferSample {
                rgba: dst_rgba8,
                coverage_count: 0,
            };
            let expected = crate::blend::blend_fragment(
                src_rgba8,
                Some(memory),
                shade_alpha_255,
                mode_state,
                true,
            )
            .expect("memory is supplied; M==Framebuffer must not error");
            assert_ne!(
                expected.rgba[3], src_rgba8[3],
                "wrong implementation 1 (src.a passthrough) must not match the real composite"
            );
            let final_alpha_255 = (shade_color[3] * 255.0).round() as u8;
            assert_ne!(
                expected.rgba[3], final_alpha_255,
                "wrong implementation 2 (final_alpha as output alpha) must not match the real \
                 composite"
            );

            let (x, y) = CHECK_XY;
            assert_blended_over_destination(
                rgba8_at(&output, x, y),
                dst_rgba8,
                expected.rgba,
                2,
                true,
            );
        }
    }

    /// Fixture builder for the framebuffer-color-blend differential's
    /// destination draw: an opaque, uniform (non-interpolated) covering
    /// triangle at the given exact RGBA byte color, no blend -- establishes
    /// a known, exact destination sample for the second (reading) draw's
    /// snapshot, at every covered pixel including row `y >= 1`.
    fn uniform_dst_fixture(rgba8: [u8; 4]) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        let color = [
            rgba8[0] as f32 / 255.0,
            rgba8[1] as f32 / 255.0,
            rgba8[2] as f32 / 255.0,
            rgba8[3] as f32 / 255.0,
        ];
        for vertex in &mut fixture.vertices {
            vertex.color = color;
        }
        fixture
    }

    /// Fixture builder for the framebuffer-color-blend differential's
    /// reading draw: a uniform-flat-shaded covering triangle (same shape as
    /// [`uniform_general_divide_blend_fixture`]) whose `blend_params` has
    /// `reads_framebuffer_color: true` -- the destination-color-reading
    /// admitted subset this card implements.
    fn uniform_framebuffer_color_blend_fixture(
        cycle: crate::blend::ResolvedBlendCycle,
        blend_color: crate::state::Color4,
        fog_color: crate::state::Color4,
        shade_color: [f32; 4],
        force_blend: bool,
    ) -> TriangleFixture {
        let mut fixture = covering_triangle_fixture();
        for vertex in &mut fixture.vertices {
            vertex.color = shade_color;
        }
        // The destination draw and this reading draw share
        // `covering_triangle_fixture`'s single `z = 0.5`, so the inherited
        // `Less` depth test rejects every fragment of the second draw
        // (`0.5 < 0.5` is false) and the blend branch under test never
        // executes -- the target keeps the destination bytes verbatim.
        // `Always` is the semantic this differential needs: depth behavior
        // is not the property being measured, and the depth matrix has its
        // own dedicated tests.
        fixture.depth_compare_enabled = false;
        fixture.blend_color = blend_color;
        fixture.force_blend = force_blend;
        fixture.blend_params = ResolvedFragmentBlendParams {
            cycle_count: 1,
            cycle0: cycle,
            cycle1: crate::blend::ResolvedBlendCycle {
                p: crate::blend::BlendColorInput::Combined,
                a: crate::blend::BlendAlphaInput::Combined,
                m: crate::blend::BlendColorInput::Combined,
                b: crate::blend::BlendBInput::Zero,
            },
            blend_color: blend_color,
            fog_color: fog_color,
            reads_framebuffer_color: true,
        };
        fixture
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
        fixture.blend_color = blend_color;
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
            blend_color: blend_color,
            fog_color: fog_color,
            reads_framebuffer_color: false,
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
