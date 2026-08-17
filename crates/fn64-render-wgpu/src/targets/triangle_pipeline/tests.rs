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
