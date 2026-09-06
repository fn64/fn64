//! Planning-side unit tests: `plan_raw_dpc`, the `PlanCollector`
//! state snapshots, scissor/tile/colour latching and the durable RDP
//! state carried across submissions.

use super::*;

/// End-to-end: plan -> execute -> commit zero guest writes -> seal ->
/// publish, driven entirely through `WgpuBackend`'s `RenderBackend`
/// methods (`plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc`) plus the
/// ABI-owned `RawDpcAbiSession` calls a real caller would make around
/// them. Proves the whole production seam actually completes and flips
/// the coordinator's active physical slot.
#[test]
fn plan_execute_publish_completes_and_flips_active_physical_slot() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();

    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);

    assert_ne!(
        backend.physical_tmem().identity(),
        initial_identity,
        "publish must flip the coordinator's active slot to the executed candidate"
    );
    assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
}

/// End-to-end, real Metal execution: a mixed plan (TMEM load +
/// triangle) must plan/execute/publish and flip the coordinator's
/// active physical slot exactly like a TMEM-only plan does (mirrors
/// `plan_execute_publish_completes_and_flips_active_physical_slot`'s
/// own full sequence) -- proving the real successor route
/// (`complete_execution`), not the preserving-physical route, was
/// actually used for a mixed plan. If the preserving-physical route
/// had been used instead, `complete_execution_preserving_physical`'s
/// own internal `BackendEffectReport::try_new(packet, Vec::new())`
/// call would have failed outright (the load's own journal entry
/// declares a real write access, `Vec::new()` declares zero, and
/// `validate_effects` rejects any count mismatch) -- so this test's
/// mere success, not just the slot flip, is itself evidence the
/// correct route was taken.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn mixed_load_and_triangle_plan_uses_the_real_successor_route_not_preserving() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    let initial_identity = backend.physical_tmem().identity();

    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, mixed_load_and_triangle_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();

    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("a mixed load+triangle plan must execute successfully");
    assert!(
        backend.last_triangle_draw().is_some(),
        "the mixed plan's triangle must still be drawn during execute_raw_dpc"
    );
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);
    assert_ne!(
        backend.physical_tmem().identity(),
        initial_identity,
        "a mixed plan's TMEM load must still flip the active physical slot on publish, via \
         the real complete_execution successor route"
    );
}

/// End-to-end, real Metal execution: a triangle-only plan (zero TMEM
/// loads) completes via `complete_execution_preserving_physical` and
/// its publish must leave the active physical slot's identity
/// UNCHANGED (the opposite assertion direction from every TMEM-load
/// test in this module) -- there is no successor state to flip to,
/// by design (§1c).
#[cfg(feature = "host-gpu-tests")]
#[test]
fn triangle_only_plan_completes_via_preserving_physical_and_never_flips_the_slot() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    let initial_identity = backend.physical_tmem().identity();

    let planned = plan_with_no_reads(&mut backend, &session, triangle_only_words());
    let guest_capture = guest_read_capture(&planned, &[]);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();

    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("a triangle-only plan must execute successfully via preserving_physical");
    assert!(
        backend.last_triangle_draw().is_some(),
        "the triangle-only plan's triangle must still be drawn"
    );
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);
    assert_eq!(
        backend.physical_tmem().identity(),
        initial_identity,
        "a triangle-only plan has no TMEM successor to flip to -- the active physical slot's \
         identity must remain exactly what it was before, proving complete_execution (the \
         route that WOULD flip it) was never used"
    );
}

/// The real end-to-end test (§2): a real decoded capture containing
/// `SetOtherMode`/`SetCombine`/one `RawTriangle`, pushed through the
/// actual production entry points
/// (`WgpuBackend::create`/`plan_raw_dpc`/`execute_raw_dpc`), asserted
/// against real GPU-observed pixel output -- matching the rigor
/// `targets::triangle_pipeline::tests`'s own
/// `required_host_draws_a_real_admitted_triangle_matching_the_combiner_oracle`
/// already established for its own standalone (non-`WgpuBackend`)
/// proof, but through the actual `RenderBackend` seam this card
/// closes, not a bare coordinator.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn wgpu_backend_draws_a_real_admitted_triangle_matching_the_combiner_oracle() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }

    // PRIMITIVE-passthrough SetCombine: (A-B)*C+D collapses to D, and D
    // now decodes to PRIMITIVE (index 3, `color_input_d`) instead of
    // SHADE -- this genuinely exercises Slice B's new
    // `fragment_material_params` uniform rather than continuing to
    // collapse to a SHADE-only formula where env/prim would silently
    // not matter (production-combiner-slice-b-card §6 step 2).
    let color_a: u32 = 0;
    let color_b: u32 = 0;
    let color_c: u32 = 0;
    let color_d: u32 = 3;
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

    // Real SetEnvColor/SetPrimColor wire commands (card §6 step 1),
    // pushed before the triangle so Slice A's command-time capture
    // (`RetrievedTriangleDraw.env_color`/`.prim_color`) resolves them.
    let env_color_wire: u32 = 0x1122_33AA;
    let prim_lod_frac_wire: u32 = 0x40;
    let prim_lod_min_wire: u32 = 0x05;
    let prim_color_wire: u32 = 0x4455_66BB;

    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(low, high));
    words.extend(set_env_color(env_color_wire));
    words.extend(set_prim_color(
        prim_lod_frac_wire,
        prim_lod_min_wire,
        prim_color_wire,
    ));
    let triangle_color_255 = [64u32, 128, 192, 255];
    words.extend(shaded_covering_triangle_words(triangle_color_255));

    // `set_combine(payload, high)` masks `payload` to the low 24 bits
    // and bakes the `SET_COMBINE` opcode byte into the top 8 bits of
    // the wire word -- `CombineParams::from_wire(w0, w1)` stores `w0`
    // unmasked (`combiner.rs`'s own module doc), so the expected value
    // is derived from the exact same wire word this fixture pushed,
    // not read back from the sealed plan (which exposes no such
    // accessor -- this mirrors how the standalone parallel-lane test
    // cross-checks against its own raw decoded ticket, not the plan).
    let combine_params = CombineParams::from_wire(word(SET_COMBINE, low & 0x00ff_ffff), high);
    let planned = plan_with_no_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &[]);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();

    backend
        .execute_raw_dpc(bound)
        .expect("the fixture stays inside the admitted state+triangle subset");

    let output = backend.last_triangle_draw().expect(
        "a successful triangle-bearing execute_raw_dpc must populate last_triangle_draw",
    );

    // Known-covered pixel, flat shade -> every covered pixel has the
    // same combiner output, no barycentric interpolation needed.
    let shade_color = [
        triangle_color_255[0] as f32 / 255.0,
        triangle_color_255[1] as f32 / 255.0,
        triangle_color_255[2] as f32 / 255.0,
        triangle_color_255[3] as f32 / 255.0,
    ];
    let base_inputs = crate::combiner::CombinerInputs {
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
    // Real env_color/prim_color values (via
    // combiner_inputs_from_fragment_registers, the exact Rust-side
    // machinery Slice B's uniform mirrors) instead of hardcoded zero --
    // proves the expected value matches what the production path now
    // actually computes.
    let inputs = crate::combiner::combiner_inputs_from_fragment_registers(
        base_inputs,
        crate::state::Color4::from_wire(env_color_wire),
        crate::state::PrimColor::from_wire(
            prim_lod_min_wire << 8 | prim_lod_frac_wire,
            prim_color_wire,
        ),
    );
    let (expected_color, _alpha_compare) =
        crate::combiner::run_one_cycle(combine_params, inputs);
    let expected_u8 = expected_color.map(|component| (component * 255.0).round() as u8);

    let pixel_index = (output.extent.width + 1) as usize * 4;
    let observed = [
        output.color_rgba8[pixel_index],
        output.color_rgba8[pixel_index + 1],
        output.color_rgba8[pixel_index + 2],
        output.color_rgba8[pixel_index + 3],
    ];
    for channel in 0..4 {
        assert!(
            observed[channel].abs_diff(expected_u8[channel]) <= 2,
            "pixel (1,1) channel {channel}: observed {observed:?} vs expected {expected_u8:?} \
             (real decoded CombineParams via the real WgpuBackend production seam)"
        );
    }
    // D now decodes to PRIMITIVE (not SHADE), so the expected color's
    // RGB channels come from prim_color_wire's real bytes; alpha_d is
    // still SHADE(4), so alpha comes from the triangle's own shade
    // alpha (triangle_color_255[3] == 255, normalized to 1.0).
    let prim_color_rgba8 = prim_color_wire.to_be_bytes();
    assert_eq!(
        expected_u8,
        [
            prim_color_rgba8[0],
            prim_color_rgba8[1],
            prim_color_rgba8[2],
            triangle_color_255[3] as u8,
        ]
    );
}

/// The real end-to-end test (texture-rectangle placement card §3): a
/// real decoded capture containing `SetOtherMode`/`SetCombine`/one
/// `TextureRectangle` (opcode `0x24`), sampling this module's already-
/// committed fixture texture, pushed through the actual production
/// entry points (`WgpuBackend::create`/`plan_raw_dpc`/
/// `execute_raw_dpc`), asserted against real GPU-observed pixel output
/// at the pixel range `[left, right) x [top, bottom)` this rectangle's
/// own `ulx`/`uly`/`lrx`/`lry` place it at -- genuinely wire-position-
/// faithful, not a fixed-corner artifact (the gap this card's own §0
/// closes).
#[cfg(feature = "host-gpu-tests")]
#[test]
fn wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    load_and_publish_fixture_texture(&mut backend, &mut session);

    // Re-declare the tile binding: tile-binding state does not persist
    // across separate `execute_raw_dpc` calls (`PlanCollector` is fresh
    // per plan) -- only `project_committed_tmem`'s underlying physical
    // TMEM bytes persist, via publish.
    let mut words = Vec::new();
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(set_other_mode(0, 0));
    // TEXEL0-passthrough SetCombine, same idiom as
    // `required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`.
    let color_a: u32 = 0;
    let color_b: u32 = 0;
    let color_c: u32 = 0;
    let color_d: u32 = 1; // TEXEL0
    let alpha_a: u32 = 0;
    let alpha_b: u32 = 0;
    let alpha_c: u32 = 1;
    let alpha_d: u32 = 1; // TEXEL0
    let low = (color_a << 5) | color_c;
    let high = (color_b << 24)
        | (color_d << 6)
        | (alpha_a << 21)
        | (alpha_b << 3)
        | (alpha_c << 18)
        | alpha_d;
    words.extend(set_combine(low, high));
    words.extend(texrect_words(TEXRECT, 0));

    let planned = plan_with_no_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &[]);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    backend
        .execute_raw_dpc(bound)
        .expect("fixture stays inside the admitted state+rect subset");

    let output = backend
        .last_triangle_draw()
        .expect("a successful rect-bearing execute_raw_dpc must populate last_triangle_draw");

    // `texrect_words`' own fixture: [left, right) x [top, bottom) ==
    // [2, 6) x [2, 6) in this 8x8 target. Every covered pixel samples
    // TEXEL0 -- a real (non-uniform) texture read, so this only proves
    // real position, not that every pixel has the same color; the
    // uncovered corner proves the rectangle did NOT cover the whole
    // target (a fixed-NDC-corner bug would cover all 64 pixels).
    let width = output.extent.width;
    let covered_pixel_index = (2 * width + 2) as usize * 4;
    let covered = [
        output.color_rgba8[covered_pixel_index],
        output.color_rgba8[covered_pixel_index + 1],
        output.color_rgba8[covered_pixel_index + 2],
        output.color_rgba8[covered_pixel_index + 3],
    ];
    assert_ne!(
        covered,
        [0, 0, 0, 0],
        "pixel (2,2) is inside [2,6)x[2,6) and must be covered by the real rectangle position"
    );
    let outside_pixel_index = 0usize;
    let outside = [
        output.color_rgba8[outside_pixel_index],
        output.color_rgba8[outside_pixel_index + 1],
        output.color_rgba8[outside_pixel_index + 2],
        output.color_rgba8[outside_pixel_index + 3],
    ];
    assert_eq!(
        outside,
        [0, 0, 0, 0],
        "pixel (0,0) is outside [2,6)x[2,6) and must stay the Clear color -- a fixed-NDC-\
         corner bug would cover the whole 8x8 target and fail this assertion"
    );
}

/// Flip sibling of
/// `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`
/// (texture-rectangle placement card §3 item 3): `TextureRectangleFlip`
/// (opcode `0x25`) places the SAME rectangle at the SAME pixel range --
/// flip only transposes UV pairing (`texture_rectangle.rs`'s own
/// module doc), never position -- proving flip ordering survives all
/// the way to real pixel coverage, not just the CPU-side vertex/texcoord
/// unit tests `raw_dpc::texture_rectangle`/`production_adapter` already
/// cover.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn wgpu_backend_draws_a_real_texture_rectangle_flip_at_the_same_wire_position() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    load_and_publish_fixture_texture(&mut backend, &mut session);

    // Re-declare the tile binding: see the non-flip sibling's own
    // comment for why this is required per-plan, not durable.
    let mut words = Vec::new();
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(set_other_mode(0, 0));
    let color_a: u32 = 0;
    let color_b: u32 = 0;
    let color_c: u32 = 0;
    let color_d: u32 = 1; // TEXEL0
    let alpha_a: u32 = 0;
    let alpha_b: u32 = 0;
    let alpha_c: u32 = 1;
    let alpha_d: u32 = 1; // TEXEL0
    let low = (color_a << 5) | color_c;
    let high = (color_b << 24)
        | (color_d << 6)
        | (alpha_a << 21)
        | (alpha_b << 3)
        | (alpha_c << 18)
        | alpha_d;
    words.extend(set_combine(low, high));
    words.extend(texrect_words(TEXRECT_FLIP, 0));

    let planned = plan_with_no_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &[]);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    backend
        .execute_raw_dpc(bound)
        .expect("fixture stays inside the admitted state+rect subset");

    let output = backend
        .last_triangle_draw()
        .expect("a successful rect-bearing execute_raw_dpc must populate last_triangle_draw");

    // Same [2,6)x[2,6) placement as the non-flip sibling -- flip must
    // not move the rectangle.
    let width = output.extent.width;
    let covered_pixel_index = (2 * width + 2) as usize * 4;
    let covered = [
        output.color_rgba8[covered_pixel_index],
        output.color_rgba8[covered_pixel_index + 1],
        output.color_rgba8[covered_pixel_index + 2],
        output.color_rgba8[covered_pixel_index + 3],
    ];
    assert_ne!(
        covered,
        [0, 0, 0, 0],
        "flip must not change the rectangle's covered pixel range"
    );
    let outside_pixel_index = 0usize;
    let outside = [
        output.color_rgba8[outside_pixel_index],
        output.color_rgba8[outside_pixel_index + 1],
        output.color_rgba8[outside_pixel_index + 2],
        output.color_rgba8[outside_pixel_index + 3],
    ];
    assert_eq!(
        outside,
        [0, 0, 0, 0],
        "flip must not change the rectangle's covered pixel range"
    );
}

/// CPU-only half of the differential (card §4/§7 requirement "(a) the
/// CPU oracle chain ... invoked directly in a `#[cfg(test)]` unit test
/// with no GPU involved"): runs this fixture's real production
/// TMEM-only load through `WgpuBackend::execute_raw_dpc` (no GPU
/// adapter required -- `execute_raw_dpc` only reaches the triangle
/// pipeline when a plan admits a `Triangle` command, which a TMEM-only
/// plan never does), then asserts hand-derivable properties of
/// `cpu_oracle_sample`'s output directly against
/// `address_texture_cell`/`gather_committed_texture_cell`/
/// `filter_three_nearest_committed_cell`. Exact texel-center point
/// `(16,16)` has no filtering ambiguity (three-nearest weighting at an
/// exact-center coordinate reduces to selecting that corner at full
/// weight) and must equal pure red -- this is the load-bearing
/// assertion the GPU-side differential below reuses as its own known-
/// good anchor.
#[test]
fn required_cpu_tmem_oracle_matches_hand_derived_texel_colors() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(load_sync());
    let source_bytes = fixture_load_block_source_bytes();
    words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

    let (planned, _unused_deterministic_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture's TMEM-only load stays inside the admitted subset");
    // `physical_tmem()` reflects the coordinator's ACTIVE physical
    // slot, which only flips at publish (see
    // `plan_execute_publish_completes_and_flips_active_physical_slot`'s
    // own doc/assertion) -- `execute_raw_dpc` alone stages the
    // candidate but does not publish it.
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);

    // Addressing convention (this slice's own verification against
    // `relative_axis_coordinate`/`address_axis_texel`/
    // `filter_three_nearest`, corrected from an initial wrong
    // assumption that raw=16 was a texel's own unambiguous center):
    // `base_texel = raw.div_euclid(32)`, `fraction = raw.rem_euclid(32)`,
    // and `filter_three_nearest`'s weight is 100% on corner `(s0,t0)`
    // only when `fraction == 0` on both axes (`sf=tf=0` collapses
    // `value = c00*32` exactly). So a texel's own unambiguous center is
    // at raw = `base_texel * 32`, NOT `base_texel*32 + 16` -- `+16`
    // instead lands exactly halfway toward the NEXT texel, which is
    // this fixture's genuinely-blended "tile center" case below.
    //
    // Exact address of texel (0,0): red, no filtering ambiguity
    // (sf=tf=0, base_texel=(0,0)).
    assert_eq!(
        cpu_oracle_sample(backend.physical_tmem(), 0, 0),
        [255, 0, 0, 255]
    );
    // Exact address of texel (1,0): green (base_texel=(1,0)).
    assert_eq!(
        cpu_oracle_sample(backend.physical_tmem(), 32, 0),
        [0, 255, 0, 255]
    );
    // Exact address of texel (0,1): blue (base_texel=(0,1)).
    assert_eq!(
        cpu_oracle_sample(backend.physical_tmem(), 0, 32),
        [0, 0, 255, 255]
    );
    // Exact address of texel (1,1): white (base_texel=(1,1), clamped
    // to the tile's own [0,1] dimension on each axis).
    assert_eq!(
        cpu_oracle_sample(backend.physical_tmem(), 32, 32),
        [255, 255, 255, 255]
    );
    // Genuine four-corner blend (card's own "tile's geometric center"
    // intent): raw=(16,16), `base_texel=(0,0)`, `sf=tf=16` (halfway
    // toward `s1`/`t1`) -- `filter_three_nearest`'s `sf+tf<=32` branch
    // gives `value = c00*32 + 16*(c10-c00) + 16*(c01-c00)` per channel,
    // hand-substituted here directly from red `(255,0,0)`, green
    // `(0,255,0)`, blue `(0,0,255)` (the `c11`/white corner does not
    // enter this branch at all, since `sf+tf=32<=32`):
    // R: 255*32 + 16*(0-255) + 16*(0-255) = 8160 - 4080 - 4080 = 0
    // G: 0*32 + 16*(255-0) + 16*(0-0) = 4080 -> round((4080+16)/32) = 128
    // B: 0*32 + 16*(0-0) + 16*(255-0) = 4080 -> 128
    // A: 255*32 + 16*(255-255) + 16*(255-255) = 8160 -> 255
    let tile_center = cpu_oracle_sample(backend.physical_tmem(), 16, 16);
    assert_eq!(tile_center, [0, 128, 128, 255]);

    // Negative-coordinate floor-vs-truncation repair (independent
    // adversarial-review finding): `triangle_pipeline_fragment.wgsl`
    // used to compute `i32(uv.x)` directly on the interpolated `f32` raw
    // S10.5 coordinate, which truncates toward zero instead of flooring
    // toward negative infinity -- disagreeing with this CPU oracle's
    // (and `tmem_sample.wgsl`'s own `relative_axis_coordinate` port's)
    // `div_euclid`/`rem_euclid` floor convention for any negative
    // coordinate. This fixture's own clamp-addressed tile
    // (`fixture_tile_descriptor`, `mask_s = mask_t = 0`) cannot expose
    // that bug: `address_axis_texel` clamps unconditionally when
    // `mask == 0`, so `base_texel = -1` (the correct floor of raw `-1`)
    // and `base_texel = 0` (`i32`-truncated raw `0`, the wrong result if
    // truncation were used instead of floor) both clamp `s0`/`s1` to the
    // SAME column pair regardless -- the two address paths are
    // mathematically indistinguishable in the final blended color at
    // that boundary. `fixture_wrap_tile_descriptor` instead uses
    // `mask_s = mask_t = 1` (non-clamp, non-mirror wrap addressing over
    // the SAME committed 2x2 texel layout): a wrap-addressed axis
    // selects by parity (`coordinate & 1`), so floor(-1) (odd) and
    // trunc(0) (even) address genuinely different, non-collapsing
    // corners.
    //
    // At raw S10.5 point (s=-1, t=0): `relative_axis_coordinate` gives
    // `base_s = (-1).div_euclid(32) = -1`, `frac_s =
    // (-1).rem_euclid(32) = 31`; `base_t = 0`, `frac_t = 0`. Wrap
    // addressing (`mask=1`, period 2): `s0 = (-1) & 1 = 1`, `s1 = 0 & 1
    // = 0`, `t0 = 0 & 1 = 0`, `t1 = 1 & 1 = 1`. Corners: `c00 =
    // color(s0=1,t0=0)` = green `(0,255,0,255)`, `c10 =
    // color(s1=0,t0=0)` = red `(255,0,0,255)`, `c01 =
    // color(s0=1,t1=1)` = white `(255,255,255,255)`. `sf=31, tf=0,
    // sf+tf=31<=32` selects `filter_three_nearest`'s first branch:
    // `value = c00*32 + 31*(c10-c00) + 0*(c01-c00)` per channel:
    // R: 0*32 + 31*(255-0) + 0 = 7905 -> round((7905+16)/32) = 247
    // G: 255*32 + 31*(0-255) + 0 = 8160-7905 = 255 -> round((255+16)/32) = 8
    // B: 0*32 + 31*(0-0) + 0 = 0 -> 0
    // A: 255*32 + 31*(255-255) + 0 = 8160 -> 255
    // This CPU oracle result (the ONLY correct answer, since this
    // module's own `TextureCoordinateS10_5`/`address_axis_texel` chain
    // has always used `div_euclid`/`rem_euclid`, never truncation) is
    // asserted exactly below, with zero tolerance. This is the
    // discriminating value the GPU-side differential test also samples,
    // where the pre-repair `i32(uv.x)` bug would instead have addressed
    // `base_texel = 0` (truncating -1.0's neighborhood toward zero) and
    // produced pure red `(255,0,0,255)` -- a different, wrong result
    // this exact assertion rules out.
    let negative_coordinate = cpu_oracle_sample_with_tile(
        backend.physical_tmem(),
        fixture_wrap_tile_descriptor(),
        fixture_tile_size(),
        -1,
        0,
    );
    assert_eq!(negative_coordinate, [247, 8, 0, 255]);
}

/// Published committed-TMEM textured-draw card §4/§7 (mandatory exit
/// gate): the new fragment-callable WGSL TMEM addressing/filter chain
/// (`shaders/tmem_sample.wgsl`, wired through
/// `triangle_pipeline_fragment.wgsl`'s `fs_main`) must agree with the
/// CPU oracle chain, both computed independently -- CPU side via
/// `cpu_oracle_sample` above (no GPU, see
/// `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`), GPU
/// side through the real fragment pipeline on a host-GPU adapter
/// (`WgpuBackend::execute_raw_dpc` for the TMEM commit,
/// `TrianglePipelineRenderer::submit_admitted_triangle` -- reached via
/// the `#[cfg(test)]`-only `triangle_pipeline_for_test` accessor -- for
/// the textured triangle draw itself). UVs are chosen so the
/// rasterizer's own per-fragment interpolation (not a per-triangle
/// constant) produces two genuinely different sample points, each
/// algebraically derived below from the real vertex UVs and the actual
/// pixel-center sampling convention, not copied magic numbers.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }

    // Real production TMEM-load path: SetTextureImage/SetTile(0)/
    // SetTileSize(0)/LoadSync/LoadBlock, admitted and executed through
    // `WgpuBackend::execute_raw_dpc` exactly like every other TMEM-load
    // test in this module. SetTextureImage's own width is the SOURCE
    // IMAGE'S width (4 texels, see this test's own doc above) -- not
    // the 2x2 TILE's width, which `SetTileSize` below states
    // separately.
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    // SetTileSize: low_s=0, low_t=0, high_s=4 (raw S10.2; `integer() ==
    // 1`), high_t=4 -- a 2x2-texel tile (`high.integer() -
    // low.integer() + 1 == 2` on each axis), matching
    // `fixture_tile_size()` exactly.
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(load_sync());
    let source_bytes = fixture_load_block_source_bytes();
    // LoadBlock w0: source_s=0, source_t=0 (top-left of the 4-wide
    // source image). w1: tile=0, high_s=7 (eight texels, 0..=7
    // inclusive, spanning both rows of the 4-wide image --
    // `decode_load_block`'s `texels = high_s - source_s + 1 == 8`),
    // dxt=0 (pure-linear mode: no row-interleave -- correct here
    // because the source image's own natural row width, 4 texels * 2
    // bytes = 8 bytes, is already exactly one TMEM word, so a flat
    // linear copy already lands row 1 at the same word-aligned offset
    // `line_words=1`'s read-side formula expects).
    words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

    let (planned, _unused_deterministic_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture's TMEM-only load stays inside the admitted subset");
    // Unlike a bare `execute_raw_dpc` discard, this test needs the
    // TMEM write to be durably visible: `physical_tmem()` only reflects
    // the coordinator's ACTIVE slot after publish (see
    // `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
    // doc) -- skipping commit/seal/publish here left stale/invalid TMEM
    // active, which is what real-Metal execution caught
    // (`InvalidTexelByte` at the fixture's own addressed footprint).
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);

    let tmem = project_committed_tmem(backend.physical_tmem());
    let tile_binding = TileBindingParams::bound(fixture_tile_descriptor(), fixture_tile_size());

    // Screen geometry matches `covering_triangle_fixture`'s own right
    // triangle exactly: vertex0=(0,0), vertex1=(8,0), vertex2=(0,8),
    // all w=1 (perspective divide is a no-op) -- so the rasterizer's
    // barycentric UV interpolation at pixel-center `(px, py) = (x+0.5,
    // y+0.5)` is the plain affine formula `uv = uv0 + wx*(uv1-uv0) +
    // wy*(uv2-uv0)`, `wx = px/8`, `wy = py/8`, matching this file's own
    // `expected_interpolated_rgba8` color-interpolation precedent
    // (`targets/triangle_pipeline/tests.rs`) applied to UV instead of
    // color. UV0=(16,16), UV1=(112,16), UV2=(16,112): a genuinely
    // varying UV gradient (not constant) sweeping well past the 2x2
    // tile's own 64-unit extent, so pixel-center interpolation lands at
    // real, hand-computed points inside the tile -- computed
    // algebraically below, not guessed or copied from the card.
    let uv0 = (16.0f32, 16.0f32);
    let uv1 = (112.0f32, 16.0f32);
    let uv2 = (16.0f32, 112.0f32);
    let interpolated_uv = |x: u32, y: u32| -> (i16, i16) {
        let wx = (x as f32 + 0.5) / 8.0;
        let wy = (y as f32 + 0.5) / 8.0;
        let s = uv0.0 + wx * (uv1.0 - uv0.0) + wy * (uv2.0 - uv0.0);
        let t = uv0.1 + wx * (uv1.1 - uv0.1) + wy * (uv2.1 - uv0.1);
        (s.round() as i16, t.round() as i16)
    };
    // Pixel (0,0)'s center (px=py=0.5, wx=wy=0.0625): s=t=16+0.0625*96=22.
    let pixel_a = (0u32, 0u32);
    // Pixel (2,2)'s center (px=py=2.5, wx=wy=0.3125): s=t=16+0.3125*96=46.
    let pixel_b = (2u32, 2u32);
    let (a_s, a_t) = interpolated_uv(pixel_a.0, pixel_a.1);
    let (b_s, b_t) = interpolated_uv(pixel_b.0, pixel_b.1);
    assert_eq!((a_s, a_t), (22, 22));
    assert_eq!((b_s, b_t), (46, 46));

    let assertion_points: [(i16, i16); 2] = [(a_s, a_t), (b_s, b_t)];
    let expected: Vec<[u8; 4]> = assertion_points
        .iter()
        .map(|&(s, t)| cpu_oracle_sample(backend.physical_tmem(), s, t))
        .collect();

    let vertices = [
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv0.0, uv0.1],
        },
        fn64_render::NeutralTriangleVertex {
            x: 8.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv1.0, uv1.1],
        },
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 8.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv2.0, uv2.1],
        },
    ];
    // TEXEL0 passthrough SetCombine: (A-B)*C+D with A=TEXEL0(1), B=0
    // (COMBINED), C=ONE-equivalent... instead use the simplest faithful
    // identity available in the common table: D=TEXEL0 with A=B=
    // COMBINED(0) (zeroing the (A-B)*C term), matching this file's own
    // established SHADE-passthrough idiom but selecting TEXEL0 (index 1)
    // for D instead of SHADE (index 4).
    let color_a: u32 = 0;
    let color_b: u32 = 0;
    let color_c: u32 = 0;
    let color_d: u32 = 1; // TEXEL0
    let alpha_a: u32 = 0;
    let alpha_b: u32 = 0;
    let alpha_c: u32 = 1;
    let alpha_d: u32 = 1; // TEXEL0
    let low = (color_a << 5) | color_c;
    let high = (color_b << 24)
        | (color_d << 6)
        | (alpha_a << 21)
        | (alpha_b << 3)
        | (alpha_c << 18)
        | alpha_d;
    let combine_params = CombineParams::from_wire(low, high);

    let renderer = backend.triangle_pipeline_for_test();
    let raster_params = TriangleRasterParams {
        resolution: [8.0, 8.0],
        screen_scale: [1.0, 1.0],
        screen_offset: [0.0, 0.0],
    };
    let output = renderer
        .submit_admitted_triangle(
            vertices,
            OtherMode::from_wire(0, 0),
            combine_params,
            raster_params,
            TriangleTargetExtent {
                width: 8,
                height: 8,
            },
            tmem,
            tile_binding,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::default(),
            ResolvedFragmentBlendParams::NO_OP,
            false,
        )
        .expect("textured triangle draw must submit cleanly")
        .complete()
        .expect("textured triangle draw must complete cleanly");

    assert!(
        output
            .tmem_sample_status
            .iter()
            .all(|&status| status == TMEM_SAMPLE_STATUS_OK),
        "every fragment's tmem_sample.wgsl status must be OK for this fixture"
    );

    let observed_at = |x: u32, y: u32| -> [u8; 4] {
        let index = (y as usize * 8 + x as usize) * 4;
        [
            output.color_rgba8[index],
            output.color_rgba8[index + 1],
            output.color_rgba8[index + 2],
            output.color_rgba8[index + 3],
        ]
    };
    // Both assertion points: the real GPU fragment output at each
    // pixel, sourced through the actual per-fragment
    // `sample_committed_rgba16_three_nearest_bound` WGSL call, compared
    // against the CPU oracle chain computed above at the SAME
    // algebraically-derived interpolated UV -- the card's mandatory
    // CPU-vs-WGSL differential (§4/§7), both sides independent.
    assert_close_rgba8_channels(observed_at(pixel_a.0, pixel_a.1), expected[0], 2);
    assert_close_rgba8_channels(observed_at(pixel_b.0, pixel_b.1), expected[1], 2);
}

/// Negative-coordinate floor-vs-truncation repair, GPU half (independent
/// adversarial-review finding; CPU half is
/// `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
/// `negative_coordinate` assertion -- see that assertion's doc comment
/// for the full discrimination argument and the wrap-vs-clamp addressing
/// reasoning). `triangle_pipeline_fragment.wgsl` used to compute
/// `i32(uv.x)`/`i32(uv.y)` directly on the interpolated `f32` raw S10.5
/// coordinate -- truncating toward zero -- before calling
/// `sample_committed_rgba16_three_nearest_bound`. This test's geometry
/// makes the rasterizer's own per-fragment interpolation land exactly on
/// a fractional NEGATIVE raw S coordinate at pixel (0,0), so a real GPU
/// run exercises the actual bug site (the fragment shader's own
/// `f32`->`i32` conversion of an interpolated value), not just the
/// integer-only WGSL addressing chain downstream of it -- unlike the
/// CPU-only half above, which starts from an already-integer raw
/// coordinate and cannot observe this specific conversion-site defect by
/// itself.
///
/// This fixture uses `fixture_wrap_tile_descriptor` (mask=1 wrap, not
/// `fixture_tile_descriptor`'s clamp) over the SAME committed 2x2 RGBA16
/// texel layout -- required because the clamp fixture cannot expose this
/// bug at all (see that function's own doc). Vertex UVs: `uv0=(-1,0)`,
/// `uv1=(-1,0)` (S constant across X, so the X-gradient term vanishes),
/// `uv2=(7,0)` (S varies across Y only). At pixel (0,0)'s center
/// (`wx=wy=1/16`): `s = -1 + (1/16)*(-1-(-1)) + (1/16)*(7-(-1)) = -1 +
/// 0 + 0.5 = -0.5` exactly (`0.0625*8 = 0.5` has an exact `f32`
/// representation, no rounding drift); `t = 0` exactly (T is literally
/// constant `0` across all three vertices -- this test isolates the
/// negative-S repair alone; T's own varying-UV requirement is already
/// covered by `required_host_textured_triangle_wgsl_sampling_matches_
/// the_cpu_tmem_oracle`, unmodified by this test). `floor(-0.5) = -1`
/// (correct) vs. `i32(-0.5) = 0` truncated toward zero (the pre-repair
/// bug) -- see the CPU-side assertion's doc for the full corner/
/// fraction arithmetic this produces under wrap addressing, and why the
/// two paths' final blended colors provably differ, not just their
/// intermediate fractions.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn required_host_negative_uv_floors_toward_negative_infinity_not_truncation() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!(
                "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
            );
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }

    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(load_sync());
    let source_bytes = fixture_load_block_source_bytes();
    words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

    let (planned, _unused_deterministic_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture's TMEM-only load stays inside the admitted subset");
    // Unlike a bare `execute_raw_dpc` discard, this test needs the
    // TMEM write to be durably visible: `physical_tmem()` only reflects
    // the coordinator's ACTIVE slot after publish (see
    // `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
    // doc) -- skipping commit/seal/publish here left stale/invalid TMEM
    // active, which is what real-Metal execution caught
    // (`InvalidTexelByte` at the fixture's own addressed footprint).
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);

    let tmem = project_committed_tmem(backend.physical_tmem());
    // Wrap tile (mask=1), not the clamp fixture -- see this test's own
    // doc for why clamp addressing cannot expose the floor-vs-truncation
    // bug.
    let tile_binding =
        TileBindingParams::bound(fixture_wrap_tile_descriptor(), fixture_tile_size());

    let uv0 = (-1.0f32, 0.0f32);
    let uv1 = (-1.0f32, 0.0f32);
    let uv2 = (7.0f32, 0.0f32);
    // Pixel (0,0)'s center (px=py=0.5, wx=wy=0.0625): s = -1 +
    // 0.0625*(-1-(-1)) + 0.0625*(7-(-1)) = -0.5, t = 0.
    let (expected_s, expected_t): (i16, i16) = (-1, 0);
    let expected = cpu_oracle_sample_with_tile(
        backend.physical_tmem(),
        fixture_wrap_tile_descriptor(),
        fixture_tile_size(),
        expected_s,
        expected_t,
    );
    // Cross-check against the CPU-only test's own independently
    // hand-derived literal, not just self-consistency with
    // `cpu_oracle_sample_with_tile`.
    assert_eq!(expected, [247, 8, 0, 255]);

    let vertices = [
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv0.0, uv0.1],
        },
        fn64_render::NeutralTriangleVertex {
            x: 8.0,
            y: 0.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv1.0, uv1.1],
        },
        fn64_render::NeutralTriangleVertex {
            x: 0.0,
            y: 8.0,
            z: 0.5,
            w: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            texcoord: [uv2.0, uv2.1],
        },
    ];
    // Same TEXEL0-passthrough SetCombine as the other GPU differential
    // test.
    let color_a: u32 = 0;
    let color_b: u32 = 0;
    let color_c: u32 = 0;
    let color_d: u32 = 1; // TEXEL0
    let alpha_a: u32 = 0;
    let alpha_b: u32 = 0;
    let alpha_c: u32 = 1;
    let alpha_d: u32 = 1; // TEXEL0
    let low = (color_a << 5) | color_c;
    let high = (color_b << 24)
        | (color_d << 6)
        | (alpha_a << 21)
        | (alpha_b << 3)
        | (alpha_c << 18)
        | alpha_d;
    let combine_params = CombineParams::from_wire(low, high);

    let renderer = backend.triangle_pipeline_for_test();
    let raster_params = TriangleRasterParams {
        resolution: [8.0, 8.0],
        screen_scale: [1.0, 1.0],
        screen_offset: [0.0, 0.0],
    };
    let output = renderer
        .submit_admitted_triangle(
            vertices,
            OtherMode::from_wire(0, 0),
            combine_params,
            raster_params,
            TriangleTargetExtent {
                width: 8,
                height: 8,
            },
            tmem,
            tile_binding,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::default(),
            ResolvedFragmentBlendParams::NO_OP,
            false,
        )
        .expect("textured triangle draw must submit cleanly")
        .complete()
        .expect("textured triangle draw must complete cleanly");

    assert!(
        output
            .tmem_sample_status
            .iter()
            .all(|&status| status == TMEM_SAMPLE_STATUS_OK),
        "every fragment's tmem_sample.wgsl status must be OK for this fixture"
    );

    let observed = [
        output.color_rgba8[0],
        output.color_rgba8[1],
        output.color_rgba8[2],
        output.color_rgba8[3],
    ];
    // Exact agreement required, not the ±2 tolerance the other GPU
    // differential test uses: the pre-repair truncation bug's wrong
    // answer at this exact point (`[255, 0, 0, 255]`, pure red -- see
    // the CPU-side assertion's doc) differs from the correct floored
    // answer (`[247, 8, 0, 255]`) by up to 8 in the green channel, well
    // outside a ±2 tolerance, but this assertion holds GPU float
    // interpolation to the CPU oracle's own exact integer result with
    // zero slack -- this specific point was chosen so the interpolated
    // `f32` UV (`-0.5`) has an exact binary representation (no rounding
    // drift into the fixed-point filter math), so exact agreement is
    // the correct bar here, not a concession to interpolation noise.
    assert_eq!(observed, expected);
}

/// `create()`'s success stores `triangle_pipeline`/`triangle_target_extent`
/// atomically, together -- a repeated `create()` call with a changed
/// `RenderConfig` extent updates both, never one without the other.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn repeated_create_with_a_changed_extent_updates_pipeline_and_extent_together() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("first create() must succeed on a real adapter");
    assert_eq!(
        backend.triangle_target_extent,
        Some(TriangleTargetExtent {
            width: 8,
            height: 8
        })
    );

    let changed_config = fn64_render::RenderConfig {
        width: 16,
        height: 16,
        tv_type: fn64_runtime::TvType::default(),
    };
    backend
        .create_inner(&changed_config)
        .expect("a second create() call with a different extent must also succeed");
    assert!(backend.triangle_pipeline.is_some());
    assert_eq!(
        backend.triangle_target_extent,
        Some(TriangleTargetExtent {
            width: 16,
            height: 16
        }),
        "a repeated create() call must update the stored extent to match its own \
         RenderConfig, not retain the first call's value"
    );
}

/// `last_triangle_draw()` update timing (§1e): a failed
/// `draw_admitted_triangles` call leaves whatever prior successful
/// output was already stored completely untouched -- never cleared,
/// never partially overwritten. Calls `draw_admitted_triangles`
/// directly with a deliberately failing second triangle so the
/// failure is deterministic, rather than relying on a real pipeline
/// error.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn a_failed_triangle_draw_leaves_the_prior_successful_output_untouched() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("create() must succeed on a real adapter");

    let good_triangle = RetrievedTriangleDraw {
        vertices: [
            fixture_vertex(0.0),
            fixture_vertex(1.0),
            fixture_vertex(2.0),
        ],
        source: TriangleSource::RawTriangle,
        viewport: None,
        other_mode: OtherMode::from_wire(0, 0),
        combine_params: CombineParams::from_wire(0, 0),
        tile_binding: TileBindingParams::unbound(),
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::default(),
        fog_color: Color4::from_wire(0),
        // This fixture drives the GPU triangle path, which reads no
        // scissor; the texrect executor is the only consumer today.
        scissor: None,
        prim_depth: None,
    };
    backend
        .draw_admitted_triangles(vec![Ok(good_triangle)], None, true)
        .expect("a single valid triangle must draw successfully");
    let first_output_extent = backend
        .last_triangle_draw()
        .expect("the first successful draw must populate last_triangle_draw")
        .extent;

    let failing_triangles = vec![
        Ok(good_triangle),
        Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 1 }),
    ];
    let result = backend.draw_admitted_triangles(failing_triangles, None, true);
    assert!(
        result.is_err(),
        "a batch containing a MissingTriangleDrawState entry must fail, not silently skip it"
    );

    let output_after_failure = backend
        .last_triangle_draw()
        .expect("the prior successful output must still be present after a later failure");
    assert_eq!(
        output_after_failure.extent, first_output_extent,
        "a failed draw_admitted_triangles call must leave the prior successful \
         last_triangle_draw() value completely untouched, never cleared"
    );
}

/// Hostile regression for the clear-per-draw batching defect this card
/// fixes: two ordinary `RawTriangle`s with disjoint pixel coverage
/// (left half / right half of an 8x8 target), submitted together in
/// one `draw_admitted_triangles` call, must BOTH be visible in the
/// single resulting `last_triangle_draw()` output. Before this fix,
/// `draw_admitted_triangles` called `submit_admitted_triangle` once per
/// triangle, and each call's own `submit_triangles(&[fixture])`
/// re-cleared the shared target -- so only the second (last) triangle
/// would ever survive, and the first triangle's left half would read
/// back as the Clear color even though it drew without error. This is
/// the same underlying defect a `TextureRectangle`'s two-triangle
/// admission exposes (see
/// `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`),
/// isolated here for a plain two-`RawTriangle` sequence with no rect
/// involvement at all -- proving the fix is general to
/// `draw_admitted_triangles`'s batching, not specific to `is_rect`.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn two_ordinary_triangles_in_one_call_both_survive_into_one_output() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("create() must succeed on a real adapter");

    let left_triangle = half_covering_triangle(0.0, 4.0, 1.0);
    let right_triangle = half_covering_triangle(4.0, 8.0, 1.0);
    backend
        .draw_admitted_triangles(vec![Ok(left_triangle), Ok(right_triangle)], None, true)
        .expect("two well-formed triangles in one call must draw successfully");

    let output = backend
        .last_triangle_draw()
        .expect("a successful draw_admitted_triangles call must populate last_triangle_draw");
    let width = output.extent.width;
    let pixel = |x: u32, y: u32| {
        let index = (y * width + x) as usize * 4;
        [
            output.color_rgba8[index],
            output.color_rgba8[index + 1],
            output.color_rgba8[index + 2],
            output.color_rgba8[index + 3],
        ]
    };
    assert_ne!(
        pixel(1, 4),
        [0, 0, 0, 0],
        "the left triangle's own half must be covered -- if the second draw re-cleared \
         the target, this pixel would still read back as the Clear color"
    );
    assert_ne!(
        pixel(6, 4),
        [0, 0, 0, 0],
        "the right (later) triangle's own half must also be covered"
    );
}

/// Same hostile shape as
/// `a_failed_triangle_draw_leaves_the_prior_successful_output_untouched`,
/// but with a real two-triangle batch preceding the invalid draw:
/// proves batch submission failure atomicity holds even once
/// `draw_admitted_triangles` collects multiple fixtures before
/// submitting -- an invalid THIRD draw appended to an otherwise-valid
/// two-triangle batch must fail the whole call and leave the prior
/// output completely untouched, not partially apply the first two
/// triangles.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn an_invalid_draw_after_two_valid_triangles_preserves_the_prior_output() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("create() must succeed on a real adapter");

    let good_triangle = RetrievedTriangleDraw {
        vertices: [
            fixture_vertex(0.0),
            fixture_vertex(1.0),
            fixture_vertex(2.0),
        ],
        source: TriangleSource::RawTriangle,
        viewport: None,
        other_mode: OtherMode::from_wire(0, 0),
        combine_params: CombineParams::from_wire(0, 0),
        tile_binding: TileBindingParams::unbound(),
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::default(),
        fog_color: Color4::from_wire(0),
        // This fixture drives the GPU triangle path, which reads no
        // scissor; the texrect executor is the only consumer today.
        scissor: None,
        prim_depth: None,
    };
    backend
        .draw_admitted_triangles(vec![Ok(good_triangle)], None, true)
        .expect("a single valid triangle must draw successfully");
    let prior_color = backend
        .last_triangle_draw()
        .expect("the first successful draw must populate last_triangle_draw")
        .color_rgba8
        .clone();

    let batch_with_trailing_failure = vec![
        Ok(half_covering_triangle(0.0, 4.0, 1.0)),
        Ok(half_covering_triangle(4.0, 8.0, 1.0)),
        Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 2 }),
    ];
    let result = backend.draw_admitted_triangles(batch_with_trailing_failure, None, true);
    assert!(
        result.is_err(),
        "a batch whose last entry is a MissingTriangleDrawState must fail as a whole, even \
         though the two preceding entries were individually valid"
    );

    let output_after_failure = backend
        .last_triangle_draw()
        .expect("the prior successful output must still be present after a later failure");
    assert_eq!(
        output_after_failure.color_rgba8, prior_color,
        "a batch that fails during mapping must never submit any of its fixtures, leaving \
         last_triangle_draw() byte-identical to the value before the failed call"
    );
}

/// Positive: `raw_dpc_ir_capability` reports the real TMEM-plus-fill-
/// plus-FullSync-site capability, not the trait's `Unsupported` default
/// and not either older value -- a caller must be able to tell this
/// backend apart from a non-raw-DPC-capable one, from one that admits no
/// guest-visible write, and from one that rejects every FullSync,
/// without attempting a submission.
#[test]
fn raw_dpc_ir_capability_reports_transactional_tmem_fill_full_sync_site_only() {
    let (backend, _session) = WgpuBackend::try_new().unwrap();
    assert_eq!(
        backend.raw_dpc_ir_capability(),
        RawDpcIrCapability::TransactionalTmemFillFullSyncSiteOnly
    );
    assert_ne!(
        backend.raw_dpc_ir_capability(),
        RawDpcIrCapability::TransactionalTmemNoFullSync,
        "the older TMEM-only value would tell a caller this backend declares zero \
         guest-visible writes, which is no longer true"
    );
    assert_ne!(
        backend.raw_dpc_ir_capability(),
        RawDpcIrCapability::TransactionalTmemFillNoFullSync,
        "the fill-only value would tell a caller this backend rejects every FullSync, \
         which is no longer true"
    );
}

/// Hostile: a FullSync whose capture carries no boundary record -- a
/// producer that never took the reserve half -- must be surfaced as a
/// loud `RenderError`, never a silently truncated plan and never an
/// admitted site.
#[test]
fn plan_raw_dpc_rejects_an_unreserved_full_sync_command_loudly() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let mut words = one_load_block_words();
    words.extend([word(FULL_SYNC, 0), 0]);

    // `capture` builds through `OwnedRawDpcCapture::new`, so its boundary
    // list is empty.
    let request = session.plan_request(capture(words));
    let result = backend.plan_raw_dpc(request);
    assert!(
        result.is_err(),
        "an unreserved FullSync must be rejected, not silently admitted into the plan"
    );
}

/// Positive: the same stream, with the boundary record a reserving
/// producer supplies, plans cleanly -- FullSync is no longer blanket-
/// rejected at the production seam.
///
/// The boundary supplied here is exactly what
/// `try_dispatch_raw_dpc_via_session` supplies in production: both
/// interrupt states `Clear`, because reserving the DP completion slot
/// observes no interrupt. This test therefore also pins the nonclaim --
/// admission does not require, and does not produce, an `Asserted`
/// value.
#[test]
fn plan_raw_dpc_admits_a_reserved_full_sync_site() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let mut words = one_load_block_words();
    words.extend([word(FULL_SYNC, 0), 0]);

    let request = session.plan_request(full_sync_capture(words));
    let planned = backend.plan_raw_dpc(request);
    assert!(
        planned.is_ok(),
        "a FullSync site whose capture carries its boundary must plan cleanly: {:?}",
        planned.err()
    );
}

/// T4 characterization: `plan_raw_dpc` must accept a genuinely
/// XBUS-sourced capture (`RawDpcSource::XbusDmem`), not only the
/// RDRAM-sourced captures every other fixture in this module exercises.
/// Regression coverage for the bug this task found and fixed:
/// `single_source_probe_journal`'s command-decode access previously
/// always declared an RDRAM `RawCommands` region, which
/// `validate_one_to_one_command_reads` (fn64-render-ir) rejects for an
/// XBUS-sourced stream with `MissingCommandReadDeclaration` -- meaning
/// every ABI XBUS producer (MMIO XBUS, RSP XBUS) would have panicked on
/// its first `plan_raw_dpc` call the moment a T4 session was
/// registered, despite `WgpuBackend`'s own capability advertising
/// a raw-DPC capability with no source-kind carve-out.
#[test]
fn plan_raw_dpc_accepts_a_genuinely_xbus_sourced_capture() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let request = session.plan_request(xbus_capture(one_load_block_words()));
    let planned = backend.plan_raw_dpc(request);
    assert!(
        planned.is_ok(),
        "an admitted TMEM-only XBUS capture must plan cleanly: {:?}",
        planned.err()
    );
}

/// Hostile (nonmutation): dropping the sealed capsule before
/// `prepare_publication` cancels -- the coordinator's active physical
/// slot must be completely unchanged, exactly like T0's own
/// `seal_publication_advances_to_fabric_prepare`/cancellation tests.
#[test]
fn dropping_the_capsule_before_prepare_publication_does_not_mutate_active_physical_state() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    drop(capsule);

    assert_eq!(
        backend.physical_tmem().identity(),
        initial_identity,
        "a dropped, never-published capsule must leave the coordinator's active slot \
         completely untouched"
    );
}

/// Hostile (abandoned-ready): `complete_execution` records a ready
/// physical candidate in the coordinator's inactive slot, but if that
/// ordinal's `ReadyPublication` is never obtained (e.g. the caller
/// abandons the flow after `execute_raw_dpc` without ever publishing),
/// the active slot must remain the original value -- there is no route
/// from "executed" alone to a physical-state flip.
#[test]
fn executing_without_publishing_never_flips_the_active_physical_slot() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let _prepared = backend.execute_raw_dpc(bound).unwrap();

    assert_eq!(
        backend.physical_tmem().identity(),
        initial_identity,
        "execute_raw_dpc alone (no publish_raw_dpc) must never flip the active slot"
    );
}

/// Joint-publication: a successful `publish_raw_dpc` call is the one
/// place the physical-slot flip, the concrete fabric commit, and the
/// `Published` terminal outcome all happen together, in the same
/// non-`Result`, callback-free call -- proven by observing all three
/// facts change atomically across that single call (none change before
/// it, all three have changed by the time it returns).
#[test]
fn publish_raw_dpc_jointly_commits_physical_slot_fabric_and_published_outcome() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    // Before publish_raw_dpc: neither the physical slot nor the fabric
    // has moved yet.
    assert_eq!(backend.physical_tmem().identity(), initial_identity);
    assert_eq!(fabric.rsp_execution_state().dpc_current, 0x100);

    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);

    assert_eq!(outcome.submission(), submission);
    assert_ne!(backend.physical_tmem().identity(), initial_identity);
    assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
}

/// No-route-to-fabric-only-publication: `WgpuBackend::publish_raw_dpc`
/// is the object-safe trait method a caller uses; its own body is
/// exactly `self.coordinator.prepare_publication(publication).commit()`
/// (source below), and `fn64_render::ReadyRawDpcCommitCapsule` itself
/// exposes no bare `commit`/`CommittedRawDpcOutcome`-returning method
/// (enforced by T0's own colocated source-shape sweep in
/// `fn64-render`). This test asserts the source-level shape on the
/// `fn64-render-wgpu` side: `publish_raw_dpc`'s body reaches
/// `Published` through exactly that one unaltered expression -- no
/// fabric-only path exists that could reach `Published` without also
/// flipping this backend's own physical slot.
///
/// The body is no longer a single statement: the deferred color-target
/// publication takes its submission-keyed token before the commit and
/// redeems it after. That addition is deliberately held to the same
/// invariant, and this test now proves both halves of it -- the
/// terminal expression is still character-for-character intact, and
/// the token `take` that precedes it does not touch the capsule, the
/// coordinator, or the fabric.
#[test]
fn publish_raw_dpc_source_is_exactly_prepare_publication_then_commit() {
    let source = include_str!("../../production.rs");
    let body_start = source
        .find("fn publish_raw_dpc(")
        .expect("publish_raw_dpc must exist in this file");
    let body_end = source[body_start..]
        .find("\n    }\n")
        .expect("publish_raw_dpc must have a closing brace")
        + body_start;
    let body = &source[body_start..body_end];

    assert!(
        body.contains("self.coordinator.prepare_publication(publication).commit()"),
        "publish_raw_dpc must still reach Published through exactly \
         `self.coordinator.prepare_publication(publication).commit()` -- \
         one non-Result, callback-free terminal path"
    );
    assert_eq!(
        body.matches("prepare_publication(publication)").count(),
        1,
        "publish_raw_dpc must call the coordinator's prepare_publication exactly once"
    );
    assert_eq!(
        body.matches(".commit()").count(),
        1,
        "publish_raw_dpc must reach exactly one terminal commit"
    );

    // Nothing between obtaining the capsule and committing it may read
    // or alter the capsule, the coordinator, or the fabric. The only
    // statements permitted before the commit are the submission-keyed
    // token take and the capsule's own submission read -- neither of
    // which can change what is published.
    let before_commit = &body[..body
        .find("self.coordinator.prepare_publication(publication).commit()")
        .expect("checked above")];
    let executable_before: Vec<&str> = before_commit
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .filter(|line| {
            !line.starts_with("fn publish_raw_dpc(")
                && !line.starts_with("&mut self,")
                && !line.starts_with("publication:")
                && !line.starts_with(") ->")
        })
        .collect();
    assert_eq!(
        executable_before,
        vec![
            "let submission = publication.submission();",
            "let pending = if self",
            ".task_batch_pending_fill_publications",
            ".front()",
            ".is_some_and(|pending| pending.submission == submission)",
            "{",
            "self.task_batch_pending_fill_publications.pop_front()",
            "} else {",
            "self.pending_fill_publication.take()",
            "};",
            "let outcome =",
        ],
        "no step other than the submission-keyed fill token take and the capsule's own \
         submission read may run before publish_raw_dpc's terminal commit"
    );
}

/// Multi-load coverage: a plan with two independent `LoadBlock`s must
/// execute both -- `into_pending`'s destination-coverage check (backed
/// by `PhysicalTmemPacketTransaction::expected_destination_accesses`,
/// which `PhysicalTmemState::stage_neutral_transfer` seeds for the first
/// load and `stage_neutral_transfer_next` extends for every load after
/// it) must see every load's destinations, not just the first one's.
/// Regression coverage for the exact shape a prior version of
/// `stage_neutral_transfer`/`stage_neutral_transfer_next` got wrong:
/// either freezing coverage at load one (any second-and-later load's
/// destinations silently uncounted) or double-counting load one's own
/// destinations against itself.
#[test]
fn plan_execute_publish_completes_with_two_chained_tmem_loads() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let (planned, per_read_bytes) = plan_with_deterministic_reads_for_every_load(
        &mut backend,
        &session,
        two_load_block_words(),
    );
    assert_eq!(
        planned.guest_read_plan().reads().len(),
        2,
        "fixture must actually declare two independent TmemLoadSource reads"
    );
    let guest_capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();

    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);
    assert_ne!(
        backend.physical_tmem().identity(),
        initial_identity,
        "a two-load plan must still complete and flip the active physical slot"
    );
}

/// State continuity, source-shape half: the TMEM-only admitted subset
/// has no command that behaviorally *depends* on carried-over
/// `RdpState`, so a black-box test cannot distinguish "state is threaded
/// through" from "state is discarded but happens to look populated"
/// purely by observing `plan_raw_dpc`'s success/failure. This test
/// instead pins down the source-level facts that make state threading
/// real: exactly one typed planning decode receives `durable_state`, and
/// neither a second decode nor `RdpState::default()` appears. It mirrors
/// `publish_raw_dpc_source_is_exactly_prepare_publication_then_commit`'s
/// source-shape idiom.
///
/// This test previously asserted the *opposite* for the probe --
/// `RdpState::default()` exactly once, "only the throwaway single-source
/// probe decode is allowed to use it" -- on the stated premise that "the
/// one command that reads `state.color_image()` back, `FillRectangle`,
/// is out of v11's admitted TMEM-only scope". That premise expired when
/// `plan_texture_rectangle` began reading `color_image()` too, and the
/// stale assertion was pinning the WM2000 `FN64_RENDER=wgpu` blocker in
/// place: a probe blind to durable state derives a shorter access list
/// than the real pass, and the real pass then fails `JournalMismatch`
/// against the journal the probe built. See `plan_raw_dpc_inner`'s own
/// doc. The companion behavioral tests
/// (`plan_raw_dpc_carries_durable_rdp_state_across_submissions` and
/// `plan_raw_dpc_plans_a_texrect_against_a_color_image_an_earlier_submission_staged`)
/// prove the state accumulates; this one proves the sole derivation
/// actually consults it instead of a hardcoded default.
#[test]
fn plan_raw_dpc_inner_decodes_once_against_durable_state_not_default() {
    let source = include_str!("../../production.rs");
    let body_start = source
        .find("fn plan_raw_dpc_inner(")
        .expect("plan_raw_dpc_inner must exist in this file");
    let next_fn = source[body_start + 1..]
        .find("\nfn ")
        .map(|offset| body_start + 1 + offset)
        .unwrap_or(source.len());
    let body = &source[body_start..next_fn];
    assert!(
        body.contains("crate::raw_dpc::decode_raw_dpc_for_planning(ticket, durable_state)"),
        "the one planning decode must derive commands and journal against durable state",
    );
    assert_eq!(body.matches("decode_raw_dpc_for_planning(").count(), 1);
    assert!(!body.contains("crate::decode_raw_dpc("));
    let default_state_appearances = body.matches("RdpState::default()").count();
    assert_eq!(
        default_state_appearances, 0,
        "RdpState::default() must not replace the durable predecessor during derivation"
    );
}

/// State continuity, behavioral half: `WgpuBackend` must carry durable
/// `RdpState` (specifically its `tmem()` field here -- `SetTile`
/// stages into `TmemState`, the one durable-state field the admitted
/// TMEM-only command subset actually populates; `SetColorImage` is the
/// distinct command that would populate `color_image()`, and it is not
/// part of this admitted subset) forward from one `plan_raw_dpc` call to
/// the next, rather than re-decoding every submission against a fresh
/// default. Proven here by observing `backend.rdp_state().tmem()`
/// actually change away from default after a plan that issues a real
/// `SetTile`, then staying at that value (not reverting to default)
/// once a second, independent submission plans after it.
#[test]
fn plan_raw_dpc_carries_durable_rdp_state_across_submissions() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    assert_eq!(
        backend.rdp_state(),
        &RdpState::default(),
        "a fresh backend starts at default RDP state"
    );

    let tile = crate::tmem::TileIndex::try_new(7).unwrap();

    let request_one = session.plan_request(capture(one_load_block_words()));
    backend
        .plan_raw_dpc(request_one)
        .expect("first submission plans cleanly");
    let epoch_after_first = backend.rdp_state().tmem().tile(tile).last_load_epoch();
    assert!(
        epoch_after_first.is_some(),
        "the first submission's SetTile/LoadBlock must be reflected in durable RDP state"
    );

    // The identical fixture again: if durable state reset to default
    // between submissions, this second call would derive the exact same
    // first epoch as the first call did -- `TmemState`'s own
    // `next_load_epoch` counter is what actually distinguishes "state
    // carried forward" from "state silently reset and looks populated
    // only because this submission repopulated it itself".
    let request_two = session.plan_request(capture(one_load_block_words()));
    backend
        .plan_raw_dpc(request_two)
        .expect("second submission plans cleanly against the carried-forward state");
    let epoch_after_second = backend.rdp_state().tmem().tile(tile).last_load_epoch();
    assert!(
        epoch_after_second.map(|epoch| epoch.get())
            > epoch_after_first.map(|epoch| epoch.get()),
        "a second submission's load epoch ({epoch_after_second:?}) must strictly advance \
         past the first submission's ({epoch_after_first:?}) -- if durable state had reset \
         to default between submissions, both would report the same first epoch"
    );
}

/// Regression, cross-submission planning: a texrect whose destination
/// `SetColorImage` was staged by an **earlier** submission must plan
/// with the same declared `ColorFramebuffer` write run the real decode
/// derives -- not the empty run the probe derives against a fresh
/// `RdpState::default()`.
///
/// This is the WM2000 `FN64_RENDER=wgpu` blocker in miniature. The
/// title's attract loop stages its color image once and then keeps
/// submitting texrect-only XBUS runs against it, so the *third*
/// coalesced run is the first whose durable state is non-default and
/// therefore the first where `plan_raw_dpc_inner`'s two passes can
/// disagree. `plan_texture_rectangle`'s `let Some(image) =
/// state.color_image() else { return Ok(()) }` makes that disagreement
/// silent-but-fatal: the probe declares zero `RenderTarget` accesses,
/// the real decode declares one per covered row, and
/// `ExactRawDpcPlanWriter::finish` refuses the mismatch by name.
///
/// Submissions one and two only exist to populate durable state; the
/// assertion is entirely about submission three, which carries no
/// `SetColorImage` of its own.
#[test]
fn plan_raw_dpc_plans_a_texrect_against_a_color_image_an_earlier_submission_staged() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Submission one: stage the color image (and a whole-target fill,
    // which `admit_completed_initialization` requires before any
    // partial write into a fresh target).
    let request_one = session.plan_request(capture(whole_target_fill_words()));
    backend
        .plan_raw_dpc(request_one)
        .expect("the color-image-staging submission plans cleanly");

    // Submission two: a TMEM load, so durable state is non-default in
    // more than one field by the time submission three plans.
    let request_two = session.plan_request(capture(one_load_block_words()));
    backend
        .plan_raw_dpc(request_two)
        .expect("the TMEM-load submission plans cleanly");

    assert!(
        backend.rdp_state().color_image().is_some(),
        "positive control: durable state must actually carry a color image into \
         submission three -- without this the test would pass vacuously against a \
         default state the probe happens to agree with"
    );

    // Submission three: a texrect with NO SetColorImage of its own. Its
    // destination image can only come from durable state.
    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_in_target(7));
    let request_three = session.plan_request(capture(words));
    let planned_three = backend
        .plan_raw_dpc(request_three)
        .expect("a texrect against an earlier submission's color image must plan");

    // The sealed plan exposes no journal accessor, so the declared
    // write run is counted where it is derived: `plan_raw_dpc_inner`'s
    // own journal, rebuilt here against the same durable state the
    // backend now holds. `finish`'s access-for-access check above
    // already proved the two agree; this pins the count they agree on.
    //
    // Hand-derived from RT64's own `FixedRect`, not captured from this
    // port's output. `texrect_words_in_target`'s wire fields are 10.2
    // fixed point: `uly = 2 << 2 = 8`, `lry = 4 << 2 = 16`. The staged
    // `set_other_mode(0, 0)` is 1-cycle, so neither the copy-mode
    // `lry |= 3` nor the fill/copy `uly &= !3` applies. `FixedRect`'s
    // edges both ceil (`RDP::drawRect` passes `ceil = true` to
    // `height(true, true)`): `top = (8 + 3) >> 2 = 2`,
    // `bottom = (16 + 3) >> 2 = 4`. `bottom` is *exclusive*
    // (`plan_texture_rectangle` takes `y1 = bottom - 1 = 3`), so the
    // covered rows are y 2..=3 -- **2 rows**, not the 3 an inclusive
    // reading of the wire `lry` would suggest. Likewise x: `left =
    // (16 + 3) >> 2 = 4`, `right = (44 + 3) >> 2 = 11`, so x 4..=10.
    // x0 != 0, so `plan_render_target_rows` takes its per-row branch
    // and declares one `RenderTarget` write per row -- 2 writes.
    let mut probe_words = Vec::new();
    probe_words.extend(set_other_mode(0, 0));
    probe_words.extend(set_combine(0, 0));
    probe_words.extend(texrect_words_in_target(7));
    let probe_capture = capture(probe_words);
    let probe_submission = probe_capture.submission().clone();
    let probe_layout = probe_capture.memory_layout();
    let probe_journal = single_source_probe_journal(&probe_submission, probe_layout).unwrap();
    let probe_decoded = finalize_with_zero_reads(
        probe_layout,
        probe_capture.transaction_sequence(),
        probe_submission,
        probe_capture.cmd_end(),
        probe_capture.full_sync_boundaries().to_vec(),
        probe_journal,
    )
    .unwrap();
    let probe_ticket = submit_locally(probe_decoded).unwrap();
    let against_durable = match crate::decode_raw_dpc(probe_ticket, backend.rdp_state()) {
        Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
        other => panic!("probe decode must disagree with the single-source journal: {other:?}"),
    };
    let render_target_writes = against_durable
        .iter()
        .filter(|access| {
            access.purpose() == AccessPurpose::RenderTarget
                && access.mode() == AccessMode::Write
        })
        .count();
    assert_eq!(
        render_target_writes, 2,
        "the texrect covers 2 rows (y 2..=3, RT64's bottom edge being exclusive) at \
         nonzero x0, so the real decode declares exactly 2 per-row ColorFramebuffer \
         writes -- and the plan above sealed cleanly only because the probe that built \
         its journal saw the same 2, not the 0 a default-state probe sees"
    );
    drop(planned_three);
}

/// Two `LoadBlock`s in one submission whose destination TMEM ranges
/// actually collide (both target `tmem=0`, unlike
/// `two_load_block_words`'s disjoint `0`/`0x100` split) is legitimate
/// RDP hardware behavior -- a later load may correctly overwrite bytes
/// an earlier one just wrote, and physical-TMEM overlap resolution is
/// exactly what `PhysicalTmemState`'s transaction machinery already
/// proves at the unit level
/// (`overlapping_loads_snapshot_intermediate_effect_and_publish_final_postimage`
/// in `tmem::physical::tests`, and TLUT's own
/// `back_to_back_loads_to_overlapping_destinations_each_produce_independent_plans`).
/// This test proves the *full* `WgpuBackend` seam -- `plan_raw_dpc`
/// through `execute_raw_dpc` through `publish_raw_dpc` -- carries that
/// same last-write-wins overlap resolution end to end without rejecting
/// it, complementing the disjoint-destination coverage in
/// `plan_execute_publish_completes_with_two_chained_tmem_loads`.
#[test]
fn plan_execute_publish_completes_with_two_loads_to_overlapping_tmem_destinations() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let initial_identity = backend.physical_tmem().identity();

    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
    words.extend(set_tile(6, 2, 0)); // same tmem=0 as the first load above
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 6 << 24 | 9 << 12 | 0x0800]);

    let (planned, per_read_bytes) =
        plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
    let guest_capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();

    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("overlapping TMEM destinations must complete, not reject");
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);
    assert_ne!(
        backend.physical_tmem().identity(),
        initial_identity,
        "an overlapping-destination plan must still complete and flip the active slot, \
         exactly like the disjoint-destination case"
    );
}

/// Current-identity base: `execute_raw_dpc` always stages against
/// `coordinator.physical()` -- the currently *active* slot, re-read
/// fresh on every call -- which only ever changes via a completed
/// `publish_raw_dpc`, never via `execute_raw_dpc` alone (see
/// `executing_without_publishing_never_flips_the_active_physical_slot`).
/// A second, independent `execute_raw_dpc` against that same
/// still-current active base (no publish between the two calls) is
/// therefore legitimate, not stale, and must succeed -- proven here by
/// executing the identical fixture twice in a row against one backend.
/// (An initial version of this test wrongly expected the second call to
/// reject as "stale"; it does not, because nothing about the active
/// slot's identity changed between the two calls, and each call's own
/// decoded epoch legitimately advances via the durable `RdpState`
/// continuity `plan_raw_dpc_carries_durable_rdp_state_across_submissions`
/// proves -- there is no route to a genuinely stale base through this
/// backend's public, sequential API without a publish in between, and a
/// publish is exactly what turns "current" into "stale for the old
/// base".)
#[test]
fn executing_the_same_fixture_twice_against_the_same_current_active_base_both_succeed() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    let (planned_one, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture_one = guest_read_capture(&planned_one, &source_bytes);
    let bound_one = session
        .finalize_and_submit(planned_one, guest_capture_one)
        .unwrap();
    backend
        .execute_raw_dpc(bound_one)
        .expect("first execution against the current active base succeeds");

    let (planned_two, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture_two = guest_read_capture(&planned_two, &source_bytes);
    let bound_two = session
        .finalize_and_submit(planned_two, guest_capture_two)
        .unwrap();

    backend.execute_raw_dpc(bound_two).expect(
        "a second execution against the same still-current active base (no publish \
         between the two calls) must also succeed, not be rejected as stale",
    );
}

/// Production blend wiring slice 1: `SetFogColor(A)` -> triangle A ->
/// `SetFogColor(B)` -> triangle B must collect two distinct snapshots
/// through `PlanCollector`, mirroring `plan_collector_snapshots_
/// distinct_env_and_prim_colors_through_a_and_b_triangles` below exactly
/// for the new `current_fog_color` field.
#[test]
fn plan_collector_snapshots_distinct_fog_colors_through_a_and_b_triangles() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );

    let fog_a = fixture_set_fog_color(0x7777_7777);
    collector.command(RawDpcSemanticCommandRef::State(&fog_a));
    let triangle_a = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));

    let fog_b = fixture_set_fog_color(0x8888_8888);
    collector.command(RawDpcSemanticCommandRef::State(&fog_b));
    let triangle_b = fixture_triangle(10.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

    assert_eq!(collector.triangles.len(), 2);
    let first = collector.triangles[0].draw.as_ref().unwrap();
    let second = collector.triangles[1].draw.as_ref().unwrap();
    assert_eq!(first.fog_color, Color4::from_wire(0x7777_7777));
    assert_eq!(second.fog_color, Color4::from_wire(0x8888_8888));
    assert_ne!(
        first.fog_color, second.fog_color,
        "triangle A must NOT be retroactively affected by a SetFogColor after it in plan \
         order"
    );
}

/// The fallback's two axes come from their OWN extent dimensions.
///
/// A DELIBERATELY NON-SQUARE extent: 320x240 is the classic N64 frame,
/// and its two axes give different quarter-pixel bounds (1280 vs 960),
/// so a fallback deriving the height from the width -- or transposing
/// the two -- fails here. A square fixture would pass either way, which
/// is exactly the coincidence this case exists to avoid.
///
/// Hand-derived: 320 pixels * 4 = 1280 quarter-pixels;
/// 240 * 4 = 960. Origin is (0, 0) on both axes.
#[test]
fn the_texrect_scissor_fallback_is_the_full_target_on_each_axis_separately() {
    let extent = crate::targets::ColorTargetExtent::try_new(320, 240).unwrap();
    let scissor = texrect_scissor_or_full_target(None, extent);
    assert_eq!(scissor.mode(), 0);
    assert_eq!(scissor.upper_left_x(), 0);
    assert_eq!(scissor.upper_left_y(), 0);
    assert_eq!(scissor.lower_right_x(), 1280);
    assert_eq!(scissor.lower_right_y(), 960);
    assert_ne!(
        scissor.lower_right_x(),
        scissor.lower_right_y(),
        "the two axes must not collapse onto one dimension"
    );
}

/// A latched rect wins over the fallback outright -- the fallback is
/// only for a plan that issued no `SetScissor` at all. The latched rect
/// here is deliberately looser on one axis and tighter on the other
/// than the extent's fallback would be, so neither "always fallback"
/// nor "min of the two" passes.
#[test]
fn a_latched_scissor_wins_over_the_full_target_fallback() {
    let extent = crate::targets::ColorTargetExtent::try_new(320, 240).unwrap();
    let latched = crate::targets::RdpScissorRect::from_wire_quarter_pixels(1, 8, 12, 400, 2000);
    assert_eq!(
        texrect_scissor_or_full_target(Some(latched), extent),
        latched
    );
}

/// **`SetScissor` is a per-triangle snapshot, not the walk's running
/// final value.**
///
/// One packet can carry several rectangles under different scissors.
/// Pinned RT64 intersects its current scissor with each draw rectangle
/// (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`); fn64 therefore
/// snapshots the value current when each primitive arrives. Collecting a
/// single running value would clip earlier rectangles with the later
/// one's rect. The exact per-command relatch rule is fn64's own reading
/// and is not independently confirmed against an allowed hardware
/// reference.
///
/// Mirrors `plan_collector_snapshots_fog_color_per_triangle` exactly,
/// and uses two rects sharing NO coordinate so a snapshot that mixed
/// fields from both cannot pass.
#[test]
fn plan_collector_snapshots_scissor_per_triangle() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );

    let scissor_a = fixture_set_scissor(0, 4, 8, 12, 16);
    collector.command(RawDpcSemanticCommandRef::State(&scissor_a));
    let triangle_a = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));

    let scissor_b = fixture_set_scissor(1, 20, 24, 28, 32);
    collector.command(RawDpcSemanticCommandRef::State(&scissor_b));
    let triangle_b = fixture_triangle(10.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

    assert_eq!(collector.triangles.len(), 2);
    let first = collector.triangles[0]
        .draw
        .as_ref()
        .unwrap()
        .scissor
        .expect("triangle A saw scissor A");
    let second = collector.triangles[1]
        .draw
        .as_ref()
        .unwrap()
        .scissor
        .expect("triangle B saw scissor B");
    assert_eq!(
        (
            first.mode(),
            first.upper_left_x(),
            first.upper_left_y(),
            first.lower_right_x(),
            first.lower_right_y()
        ),
        (0, 4, 8, 12, 16)
    );
    assert_eq!(
        (
            second.mode(),
            second.upper_left_x(),
            second.upper_left_y(),
            second.lower_right_x(),
            second.lower_right_y()
        ),
        (1, 20, 24, 28, 32)
    );
    assert_ne!(
        first, second,
        "triangle A must NOT be retroactively re-scissored by a SetScissor after it in plan \
         order"
    );
}

/// A collector seeded with an EARLIER packet's rect hands that rect to
/// a triangle in a packet that issues no `SetScissor` of its own.
///
/// `SetScissor` is durable RDP state: a display list commonly sets the
/// scissor once per frame and then submits several packets under it, so
/// a per-packet reset would silently unscissor every packet after the
/// first.
#[test]
fn plan_collector_carries_a_seeded_scissor_into_a_packet_that_sets_none() {
    let seeded = crate::targets::RdpScissorRect::from_wire_quarter_pixels(2, 40, 44, 48, 52);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(OtherMode::from_wire(0, 0)),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        Some(seeded),
        None,
        [(None, None); 8],
    );
    let triangle = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    assert_eq!(
        collector.triangles[0].draw.as_ref().unwrap().scissor,
        Some(seeded)
    );
}

#[test]
fn scheduled_raw_triangle_carrier_preserves_mutated_wire_coefficients_and_routing() {
    const MUTATED_DXHDY: u32 = 0x89ab_cdef;
    let span = fn64_render::TriangleAccessSpan {
        first_access_index: 17,
        access_count: 3,
    };
    let mut triangle = fixture_raw_triangle_naming_tile(0);
    triangle.raw_words[5] = MUTATED_DXHDY;
    triangle.texrect_accesses = Some(span);

    let mut collector = fixture_raw_triangle_collector();
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let [scheduled] = collector.raw_triangle_commands.as_slice() else {
        panic!("one declared raw triangle must produce one scheduled carrier")
    };
    assert_eq!(scheduled.span, span);
    assert_eq!(scheduled.triangle_index, TriangleIndex::new(0));
    assert_eq!(scheduled.command_index, 0);
    assert_eq!(
        scheduled.decoded.unwrap().dxhdy(),
        MUTATED_DXHDY as i32,
        "the carrier must decode the command's own mutated wire slope"
    );
    assert_eq!(
        triangle.raw_words[5], MUTATED_DXHDY,
        "the neutral command remains the authoritative owner of its raw words"
    );
}

#[test]
fn scheduled_raw_triangle_carrier_preserves_decode_failure_until_named_error_route() {
    let mut triangle = fixture_raw_triangle_naming_tile(0);
    triangle.raw_words = Box::new([triangle.raw_words[0]]);
    triangle.texrect_accesses = Some(fn64_render::TriangleAccessSpan {
        first_access_index: 0,
        access_count: 1,
    });

    let mut collector = fixture_raw_triangle_collector();
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let [scheduled] = collector.raw_triangle_commands.as_slice() else {
        panic!("one declared raw triangle must produce one scheduled carrier")
    };
    assert_eq!(
        scheduled.decoded,
        Err(ScheduledRawTriangleDecodeError::Decode(
            crate::raw_dpc::TriangleDecodeError::UnexpectedLength {
                expected: 32,
                actual: 4,
            }
        )),
        "the carrier must retain the concrete decode failure"
    );
    assert!(matches!(
        decoded_scheduled_raw_triangle(scheduled),
        Err(WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable { triangle_index: 0 })
    ));
}

#[test]
fn scheduled_raw_triangle_carrier_preserves_missing_opcode_until_named_error_route() {
    let mut triangle = fixture_raw_triangle_naming_tile(0);
    triangle.raw_words = Box::new([]);
    triangle.texrect_accesses = Some(fn64_render::TriangleAccessSpan {
        first_access_index: 0,
        access_count: 1,
    });

    let mut collector = fixture_raw_triangle_collector();
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let [scheduled] = collector.raw_triangle_commands.as_slice() else {
        panic!("one declared raw triangle must produce one scheduled carrier")
    };
    assert_eq!(
        scheduled.decoded,
        Err(ScheduledRawTriangleDecodeError::MissingOpcode)
    );
    assert!(matches!(
        decoded_scheduled_raw_triangle(scheduled),
        Err(WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable { triangle_index: 0 })
    ));
}

/// **A raw triangle's GPU tile binding comes from its OWN wire field.**
///
/// `PlanCollector` froze `bound_tile_index` to 0 for every
/// `TriangleSource::RawTriangle`, with a comment claiming the triangle
/// "carries no tile field of its own to read". That claim is false:
/// wire word 0 bits 18:16 are the tile index, `RawTriangle::decode`
/// reads them, and `execute_scheduled_raw_triangle` (the CPU path)
/// already binds from them. The GPU uniform path silently sampled tile
/// 0's texture for any triangle naming another tile.
///
/// The wire word here is `0x080d_0000`. Derived by hand: opcode `0x08`
/// occupies bits 29:24, so `0x08 << 24 = 0x0800_0000`; the LEVEL field
/// starts at bit 19, so its low bit set contributes `1 << 19 =
/// 0x0008_0000`; the tile field is bits 18:16, so tile 5 contributes
/// `5 << 16 = 0x0005_0000`. Summed: `0x080d_0000`, and the expected
/// index is `(0x080d_0000 >> 16) & 0x7 == (0xd & 0x7) == 5` -- while a
/// decode masking `0xf` would read `0xd == 13`, off the 8-entry table.
///
/// Tile 5 and tile 0 are seeded with DIFFERENT `tmem_word_address`
/// values, so "read the named tile" and "read tile 0" are two
/// distinguishable answers -- every other raw-triangle fixture in this
/// file uses tile 0, where the two coincide, which is why a frozen 0
/// survived all of them.
#[test]
fn plan_collector_binds_the_tile_a_raw_triangle_s_own_wire_word_names() {
    const TILE_ZERO_TMEM: u16 = 0x010;
    const TILE_FIVE_TMEM: u16 = 0x100;

    let mut tiles = [(None, None); 8];
    let (descriptor_zero, size_zero) = fixture_neutral_tile(TILE_ZERO_TMEM);
    tiles[0] = (Some(descriptor_zero), Some(size_zero));
    let (descriptor_five, size_five) = fixture_neutral_tile(TILE_FIVE_TMEM);
    tiles[5] = (Some(descriptor_five), Some(size_five));

    let mut collector = PlanCollector::seeded_from_parts(
        Some(OtherMode::from_wire(0, 0)),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        tiles,
    );

    let triangle = fixture_raw_triangle_naming_tile(5);
    assert_eq!(
        triangle.raw_words[0], 0x080d_0000,
        "the fixture's wire word must be the hand-derived one this test reasons about"
    );
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let draw = collector.triangles[0].draw.as_ref().unwrap();
    assert_eq!(
        draw.tile_binding.tmem_word_address,
        u32::from(TILE_FIVE_TMEM),
        "the triangle names tile 5 in wire word 0 bits 18:16, so the GPU uniform must bind \
         tile 5's TMEM address, not tile 0's {TILE_ZERO_TMEM:#x}"
    );
    assert_ne!(
        TILE_FIVE_TMEM, TILE_ZERO_TMEM,
        "the two seeded tiles must differ in the field this test reads, or a frozen 0 would \
         pass"
    );
    assert_eq!(
        draw.tile_binding.bound, 1,
        "tile 5 was seeded with both halves present, so the binding must be bound"
    );
}

/// The companion arm: a raw triangle whose wire word names tile 0 must
/// still bind tile 0. Keeps the fix from degenerating into "always read
/// some other tile" -- the arm kept unchanged needs its own test, not
/// just the arm that changed.
#[test]
fn plan_collector_binds_tile_zero_when_a_raw_triangle_s_wire_word_names_it() {
    const TILE_ZERO_TMEM: u16 = 0x010;
    const TILE_FIVE_TMEM: u16 = 0x100;

    let mut tiles = [(None, None); 8];
    let (descriptor_zero, size_zero) = fixture_neutral_tile(TILE_ZERO_TMEM);
    tiles[0] = (Some(descriptor_zero), Some(size_zero));
    let (descriptor_five, size_five) = fixture_neutral_tile(TILE_FIVE_TMEM);
    tiles[5] = (Some(descriptor_five), Some(size_five));

    let mut collector = PlanCollector::seeded_from_parts(
        Some(OtherMode::from_wire(0, 0)),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        tiles,
    );

    // `0x0808_0000`: opcode 0x08 in bits 29:24, LEVEL's low bit set at
    // bit 19, tile field bits 18:16 all zero -- `(0x0808_0000 >> 16) &
    // 0x7 == (0x8 & 0x7) == 0` by hand, while a `0xf` mask would read
    // `0x8 == 8`, off the 8-entry table.
    let triangle = fixture_raw_triangle_naming_tile(0);
    assert_eq!(triangle.raw_words[0], 0x0808_0000);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let draw = collector.triangles[0].draw.as_ref().unwrap();
    assert_eq!(
        draw.tile_binding.tmem_word_address,
        u32::from(TILE_ZERO_TMEM),
        "the triangle names tile 0, so it must bind tile 0 -- not tile 5's {TILE_FIVE_TMEM:#x}"
    );
}

/// Framebuffer-blend admission split (Slice B): a triangle whose
/// resolved blend cycle selects `BlendBInput::FramebufferAlpha` on an
/// active cycle must still be rejected before GPU submission with a
/// named `BlendRequiresFramebuffer` error -- the coverage-count half of
/// the framebuffer-memory dependency, which this crate still does not
/// implement. A plain `BlendColorInput::Framebuffer` selector on `P`/`M`
/// alone (the destination-*color* half) is no longer this test's
/// fixture, since that subset is now admitted and rendered -- see
/// `draw_admitted_triangles_admits_a_framebuffer_color_only_blend_cycle`
/// below for that coverage. Mirrors `a_failed_triangle_draw_leaves_the_
/// prior_successful_output_untouched`'s own `create_inner`/
/// `RetrievedTriangleDraw` literal fixture pattern exactly.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn draw_admitted_triangles_rejects_a_blend_cycle_that_reads_the_framebuffer() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("create() must succeed on a real adapter");

    // OneCycle (high bits 20:21 == 0, the default) with cycle 1's `B`
    // selector (`blender_cycle_1().alpha_b`, low bits 18:19) = 1
    // (FramebufferAlpha, `BlendBInput::from_wire`): the coverage-count
    // sub-case this crate still does not implement.
    let framebuffer_alpha_blend_other_mode = OtherMode::from_wire(0, 1 << 18);
    let triangle = RetrievedTriangleDraw {
        vertices: [
            fixture_vertex(0.0),
            fixture_vertex(1.0),
            fixture_vertex(2.0),
        ],
        source: TriangleSource::RawTriangle,
        viewport: None,
        other_mode: framebuffer_alpha_blend_other_mode,
        combine_params: CombineParams::from_wire(0, 0),
        tile_binding: TileBindingParams::unbound(),
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::default(),
        fog_color: Color4::from_wire(0),
        // This fixture drives the GPU triangle path, which reads no
        // scissor; the texrect executor is the only consumer today.
        scissor: None,
        prim_depth: None,
    };
    let error = backend
        .draw_admitted_triangles(vec![Ok(triangle)], None, true)
        .expect_err(
            "a framebuffer-alpha-dependent blend cycle must be rejected before submission",
        );
    assert!(
        matches!(
            error,
            WgpuRawDpcExecutionError::BlendRequiresFramebuffer { triangle_index: 0 }
        ),
        "unexpected error variant: {error:?}"
    );
}

/// `ResolvedBlendCycle::requires_framebuffer_alpha` table: `true` exactly
/// when `alpha_b` (`B` selector) decodes to `FramebufferAlpha` (wire
/// value `1`), independent of `color_a`/`color_b` (`P`/`M`) -- composed
/// directly against `BlendBInput::from_wire`'s own decode, no new
/// arithmetic oracle.
#[test]
fn requires_framebuffer_alpha_matches_only_the_b_selector() {
    for color_a in 0u8..4 {
        for color_b in 0u8..4 {
            for alpha_b in 0u8..4 {
                let cycle = ResolvedBlendCycle::from_wire(BlenderCycle {
                    color_a,
                    alpha_a: 0,
                    color_b,
                    alpha_b,
                });
                let expected = BlendBInput::from_wire(alpha_b) == BlendBInput::FramebufferAlpha;
                assert_eq!(
                    cycle.requires_framebuffer_alpha(),
                    expected,
                    "color_a={color_a} color_b={color_b} alpha_b={alpha_b}"
                );
            }
        }
    }
}

/// Admission split (Slice B): a triangle whose resolved blend cycle
/// selects `BlendColorInput::Framebuffer` on `P` (color only, no
/// `FramebufferAlpha`) is now admitted -- not rejected -- and the
/// resulting fixture's `blend_params.reads_framebuffer_color` is `true`.
#[cfg(feature = "host-gpu-tests")]
#[test]
fn draw_admitted_triangles_admits_a_framebuffer_color_only_blend_cycle() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    backend
        .create_inner(&test_render_config())
        .expect("create() must succeed on a real adapter");

    // OneCycle (high bits 20:21 == 0, the default) with cycle 1's `P`
    // selector (`blender_cycle_1().color_a`, low bits 30:31) = 1
    // (Framebuffer, `BlendColorInput::from_wire`), `B` selector left at
    // its default (`0` = `OneMinusA`, not `FramebufferAlpha`) -- the
    // destination-color-only subset this card admits.
    let framebuffer_color_only_other_mode = OtherMode::from_wire(0, 1 << 30);
    let triangle = RetrievedTriangleDraw {
        vertices: [
            fixture_vertex(0.0),
            fixture_vertex(1.0),
            fixture_vertex(2.0),
        ],
        source: TriangleSource::RawTriangle,
        viewport: None,
        other_mode: framebuffer_color_only_other_mode,
        combine_params: CombineParams::from_wire(0, 0),
        tile_binding: TileBindingParams::unbound(),
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::default(),
        fog_color: Color4::from_wire(0),
        // This fixture drives the GPU triangle path, which reads no
        // scissor; the texrect executor is the only consumer today.
        scissor: None,
        prim_depth: None,
    };
    backend
        .draw_admitted_triangles(vec![Ok(triangle)], None, true)
        .expect("a color-only framebuffer blend cycle must be admitted, not rejected");
}

/// Command-time capture seam (card): `SetEnvColor(A)`/`SetPrimColor(A)`
/// -> triangle A -> `SetEnvColor(B)`/`SetPrimColor(B)` -> triangle B
/// must collect two distinct snapshots through `PlanCollector`,
/// mirroring `plan_collector_snapshots_each_triangle_at_its_own_
/// stream_position_not_the_final_value` above and
/// `triangle_draw_data.rs`'s identical `TriangleDrawStateCollector`
/// characterization test for the same new fields.
#[test]
fn plan_collector_snapshots_distinct_env_and_prim_colors_through_a_and_b_triangles() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );

    let env_a = fixture_set_env_color(0x1111_1111);
    let prim_a = fixture_set_prim_color(10, 5, 0x2222_2222);
    collector.command(RawDpcSemanticCommandRef::State(&env_a));
    collector.command(RawDpcSemanticCommandRef::State(&prim_a));
    let triangle_a = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));

    let env_b = fixture_set_env_color(0x3333_3333);
    let prim_b = fixture_set_prim_color(20, 10, 0x4444_4444);
    collector.command(RawDpcSemanticCommandRef::State(&env_b));
    collector.command(RawDpcSemanticCommandRef::State(&prim_b));
    let triangle_b = fixture_triangle(10.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

    assert_eq!(collector.triangles.len(), 2);
    let first = collector.triangles[0].draw.as_ref().unwrap();
    let second = collector.triangles[1].draw.as_ref().unwrap();
    assert_eq!(first.env_color, Color4::from_wire(0x1111_1111));
    assert_eq!(
        first.prim_color,
        PrimColor::from_wire(10 | (5 << 8), 0x2222_2222)
    );
    assert_eq!(second.env_color, Color4::from_wire(0x3333_3333));
    assert_eq!(
        second.prim_color,
        PrimColor::from_wire(20 | (10 << 8), 0x4444_4444)
    );
    assert_ne!(
        first.env_color, second.env_color,
        "triangle A must NOT be retroactively affected by a SetEnvColor after it in plan \
         order"
    );
    assert_ne!(
        first.prim_color, second.prim_color,
        "triangle A must NOT be retroactively affected by a SetPrimColor after it in plan \
         order"
    );
}

/// Durable cross-submission seed behavior for `env_color`/`prim_color`:
/// a triangle with no in-plan `SetEnvColor`/`SetPrimColor` of its own
/// still resolves those fields from `PlanCollector::seeded`'s durable
/// value, exactly mirroring `plan_collector_seeded_resolves_a_triangle_
/// with_no_in_plan_state_of_its_own` above for `other_mode`/`combine`.
#[test]
fn plan_collector_seeded_env_and_prim_color_resolve_a_triangle_with_no_in_plan_state() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let seed_env_color = Color4::from_wire(0x5555_5555);
    let seed_prim_color = PrimColor::from_wire(15 | (7 << 8), 0x6666_6666);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        seed_env_color,
        seed_prim_color,
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let triangle = fixture_triangle(1.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    assert_eq!(collector.triangles.len(), 1);
    let retrieved = collector.triangles[0]
        .draw
        .as_ref()
        .expect("a triangle with durably-seeded state must resolve, not reject");
    assert_eq!(retrieved.env_color, seed_env_color);
    assert_eq!(retrieved.prim_color, seed_prim_color);
}

/// A triangle visited with no `SetOtherMode`/`SetCombine` anywhere --
/// neither seeded nor in-plan -- must be a loud, named rejection, not
/// a silent default. Proves `PlanCollector::seeded_from_parts(None, None)`
/// (unseeded) genuinely leaves `current_other_mode`/`current_combine`
/// at `None` rather than defaulting them.
#[test]
fn plan_collector_rejects_a_triangle_visited_with_no_state_established_at_all() {
    let mut collector = PlanCollector::seeded_from_parts(
        None,
        None,
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let triangle = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    assert_eq!(collector.triangles.len(), 1);
    assert!(
        matches!(
            collector.triangles[0].draw,
            Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 0 })
        ),
        "expected NoOtherMode at triangle_index 0, got {:?}",
        collector.triangles[0].draw
    );
}

/// `PlanCollector::seeded` with a real durable value closes the
/// cross-submission carry-in gap: a triangle with no in-plan
/// `SetOtherMode`/`SetCombine` of its own still resolves cleanly when
/// seeded from a durable value, mirroring
/// `production_adapter.rs`'s own
/// `raw_triangle_is_admitted_using_durable_other_mode_carried_from_a_prior_submission`
/// at the retrieval layer instead of the admission layer.
#[test]
fn plan_collector_seeded_resolves_a_triangle_with_no_in_plan_state_of_its_own() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let triangle = fixture_triangle(1.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    assert_eq!(collector.triangles.len(), 1);
    let retrieved = collector.triangles[0]
        .draw
        .as_ref()
        .expect("a triangle with durably-seeded state must resolve, not reject");
    assert_eq!(retrieved.vertices, triangle.vertices);
    assert_eq!(retrieved.other_mode, seed_other_mode);
    assert_eq!(retrieved.combine_params, seed_combine);
}

/// Two triangles separated by an intervening `SetCombine` change must
/// collect **two different** snapshots, not one collapsed
/// whole-plan-final value -- the exact regression this design avoids
/// (see `production_adapter.rs`'s own `TriangleDrawStateCollector`
/// module doc, which independent review found and fixed this same
/// defect for). The first triangle sees the seeded value; the second
/// sees the value after the intervening `SetCombine`.
#[test]
fn plan_collector_snapshots_each_triangle_at_its_own_stream_position_not_the_final_value() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let seed_combine = CombineParams::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(seed_combine),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );

    let first_triangle = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&first_triangle));

    let changed_combine = fixture_set_combine(0, 1);
    collector.command(RawDpcSemanticCommandRef::State(&changed_combine));

    let second_triangle = fixture_triangle(10.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&second_triangle));

    assert_eq!(collector.triangles.len(), 2);
    let first_retrieved = collector.triangles[0]
        .draw
        .as_ref()
        .expect("first triangle resolves against the seeded value");
    let second_retrieved = collector.triangles[1]
        .draw
        .as_ref()
        .expect("second triangle resolves against the post-SetCombine value");
    assert_eq!(
        first_retrieved.combine_params, seed_combine,
        "the first triangle must NOT be retroactively affected by a SetCombine that comes \
         after it in plan order"
    );
    assert_ne!(
        second_retrieved.combine_params, first_retrieved.combine_params,
        "the second triangle must see the changed combine, proving per-triangle snapshots \
         are not collapsed onto one shared value"
    );
}

/// A real `SetOtherMode` visited before a triangle overrides the seed
/// -- the seed is only a starting value, never a fixed override, per
/// this design's own documented ordering semantics.
#[test]
fn plan_collector_lets_an_in_plan_set_other_mode_override_the_seed() {
    let seed_other_mode = OtherMode::from_wire(0, 0);
    let mut collector = PlanCollector::seeded_from_parts(
        Some(seed_other_mode),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );

    let changed_other_mode = fixture_set_other_mode(1 << 19, 0);
    collector.command(RawDpcSemanticCommandRef::State(&changed_other_mode));

    let triangle = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

    let retrieved = collector.triangles[0].draw.as_ref().unwrap();
    assert_ne!(
        retrieved.other_mode, seed_other_mode,
        "an in-plan SetOtherMode must override the seed, not be shadowed by it"
    );
    assert_eq!(retrieved.other_mode, OtherMode::from_wire(1 << 19, 0));
}

/// A plan with a triangle and no TMEM load must walk cleanly (no
/// panic) -- `PlanCollector` is now exhaustive over
/// `RawDpcSemanticCommandRef`'s real variant set instead of treating
/// `Triangle` as `unreachable!()`.
#[test]
fn plan_collector_walks_a_triangle_only_plan_without_panicking() {
    let mut collector = PlanCollector::seeded_from_parts(
        Some(OtherMode::from_wire(0, 0)),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let triangle = fixture_triangle(0.0);
    collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    assert!(collector.loads.is_empty());
    assert_eq!(collector.triangles.len(), 1);
}
