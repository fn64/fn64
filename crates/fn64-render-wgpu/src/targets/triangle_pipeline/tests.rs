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

// A large triangle covering the whole 8x8 target (screen-pixel corners at
// (0,0), (8,0), (0,8)), each vertex a distinct primary color, w = 1.0
// (matching the reference `raster_vs.rs` transform's unconditional `x *= w`
// with w=1 leaving x/y unchanged beyond the resolution normalization),
// z = 0.5 for every vertex (a flat triangle, so depth is uniform across all
// covered pixels regardless of wgpu's actual barycentric interpolation --
// this sidesteps needing to hand-derive per-pixel interpolated Z).
fn covering_triangle_fixture() -> TriangleFixture {
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
}
