import re
p = "crates/fn64-render-wgpu/src/production.rs"
s = open(p).read()

anchor = """    /// Two `LoadBlock`s in one submission whose destination TMEM ranges
    /// actually collide"""
assert s.count(anchor) == 1, s.count(anchor)

new = '''    /// Regression, cross-submission planning: a texrect whose destination
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
        let (planned, source_bytes) =
            plan_with_deterministic_reads_or_none(&mut backend, &session, whole_target_fill_words());
        drop((planned, source_bytes));

        // Submission two: a TMEM load, so durable state is non-default in
        // more than one field by the time submission three plans.
        let request_two = session.plan_request(capture(one_load_block_words()));
        backend
            .plan_raw_dpc(request_two)
            .expect("the TMEM-load submission plans cleanly");

        assert!(
            backend.rdp_state().color_image().is_some(),
            "positive control: durable state must actually carry a color image into \\
             submission three -- without this the test would pass vacuously against a \\
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

        // Hand-derived, not captured: `texrect_words_in_target` covers
        // x 4..=11, y 2..=4 in the 16-wide RGBA16 target. x0 != 0, so
        // `plan_render_target_rows` takes its per-row branch and declares
        // one `RenderTarget` write per row -- 3 rows, 3 writes.
        let render_target_writes = planned_three
            .journal()
            .accesses()
            .iter()
            .filter(|access| {
                access.purpose() == AccessPurpose::RenderTarget
                    && access.mode() == AccessMode::Write
            })
            .count();
        assert_eq!(
            render_target_writes, 3,
            "the texrect covers 3 rows (y 2..=4) at nonzero x0, so the sealed plan must \\
             declare exactly 3 per-row ColorFramebuffer writes -- 0 would mean the plan was \\
             sealed with the probe's default-state journal, which declares no destination \\
             at all"
        );
    }

'''
s = s.replace(anchor, new + anchor, 1)
open(p, "w").write(s)
print("ok")
