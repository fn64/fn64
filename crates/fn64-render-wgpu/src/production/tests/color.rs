//! Colour-side unit tests: texture rectangles, combiner programs,
//! one-cycle mode, the per-triangle TMEM GPU projections and the
//! committed/pending TMEM identity crossing.

use super::*;

/// Positive control for the two tests below: these fixtures really do
/// carry an admitted `TextureRectangle`, admitted as exactly two
/// `TriangleSource::TextureRectangle` triangles.
///
/// Without this, "appending a texrect changes no declared write" and
/// "fill + texrect is refused" would both still pass against a fixture
/// whose texrect had silently vanished from the stream -- measured, not
/// hypothesised: deleting the `texrect_words_in_target` line from
/// `fill_tmem_and_texrect_words` left both of those tests green until
/// this control existed (mutant D in this card's report).
#[test]
fn the_texrect_fixtures_really_do_admit_a_texture_rectangle() {
    assert_eq!(
        admitted_texture_rectangle_triangles(one_load_block_words()),
        0,
        "the TMEM-only control must admit no texture-rectangle triangles"
    );
    for (label, words) in [
        ("tmem_then_texrect", tmem_then_texrect_words()),
        ("fill_tmem_and_texrect", fill_tmem_and_texrect_words()),
    ] {
        assert_eq!(
            admitted_texture_rectangle_triangles(words),
            2,
            "{label} must admit exactly two TextureRectangle-sourced triangles -- one \
             rectangle is two triangles, and zero would mean the fixture lost its texrect"
        );
    }
}

/// **A `TextureRectangle` now declares a journal write access for its
/// destination rectangle** -- the inversion of this card's own starting
/// measurement.
///
/// At `92affbee` this test asserted the opposite, and its failure message
/// named the condition under which it should be rewritten: "if this ever
/// fails, a texrect has gained a journal write access and the composition
/// this card was dispatched to build becomes tractable". That is what
/// happened. `raw_dpc::mod`'s `plan_texture_rectangle` derives the
/// rectangle's rasterized pixel extent from
/// `texture_rectangle_vertices` (the ported RT64 `drawTexRect`/`drawRect`)
/// and declares the same per-row `ColorFramebuffer` writes `plan_fill`
/// does, through the same `plan_render_target_rows`.
///
/// The counts here are hand-derived, not captured. `texrect_words_in_target`
/// is `ulx=4<<2, uly=2<<2, lrx=11<<2, lry=4<<2`; RT64's
/// `left/top/right/bottom = (coord + 3) >> 2` gives `4, 2, 11, 4`, a
/// half-open extent, so the covered pixels are x `4..=10`, y `2..=3`.
/// That is 7 pixels wide in a 16-wide image -- a *partial*-width
/// rectangle -- so it declares one access per row: **2**. Cross-checked
/// independently: `ceil(coord / 4)` for each of the four wire values
/// yields the same `4, 2, 11, 4`.
#[test]
fn a_texture_rectangle_declares_a_render_target_write_access() {
    // A TMEM load alone declares its TMEM destination write.
    let tmem_only = declared_write_purposes(one_load_block_words());
    assert!(
        !tmem_only.is_empty(),
        "the TMEM-only control must declare at least one write, or this test cannot \
         discriminate 'texrect declares nothing' from 'the probe sees nothing'"
    );
    assert!(
        tmem_only
            .iter()
            .all(|(_, purpose)| *purpose == AccessPurpose::TmemLoadDestination),
        "the TMEM-only control must declare only TMEM destination writes, got {tmem_only:?}"
    );

    // `tmem_then_texrect_words` stages no `SetColorImage`, so its texrect
    // has no destination image and declares no write -- the documented
    // "declaring nothing is not a silent no-op" case in
    // `plan_texture_rectangle`'s contract. Pinned so that case cannot
    // silently start declaring a range.
    let with_texrect = declared_write_purposes(tmem_then_texrect_words());
    assert_eq!(
        with_texrect, tmem_only,
        "a texrect with no staged SetColorImage has no destination image, so it must \
         declare no write"
    );

    // With a color image staged (by the fill), the SAME texrect declares
    // its own RenderTarget writes on top of the fill's.
    let composed = declared_write_purposes(fill_tmem_and_texrect_words());
    let render_target_writes = composed
        .iter()
        .filter(|(_, purpose)| *purpose == AccessPurpose::RenderTarget)
        .count();
    let fill_only_render_target_writes = declared_write_purposes(whole_target_fill_words())
        .iter()
        .filter(|(_, purpose)| *purpose == AccessPurpose::RenderTarget)
        .count();
    assert_eq!(
        fill_only_render_target_writes, 1,
        "the whole-target fill is full-image-width, so it collapses to exactly one \
         contiguous access -- if this moves, the derivation below is measuring \
         something else"
    );
    assert_eq!(
        render_target_writes,
        fill_only_render_target_writes + 2,
        "the texrect must contribute exactly 2 RenderTarget writes of its own -- one per \
         covered row (y 2..=3), because at 7 pixels wide in a 16-wide image its rows are \
         disjoint and must not collapse"
    );

    // A count alone cannot tell a correctly-placed rectangle from one
    // shifted by a row (measured: that mutation survived a count-only
    // assertion). Assert the exact declared byte ranges.
    //
    // Hand-derived: the fill is the whole 16x8 RGBA16 target at
    // `0x2000`, so it declares `0x2000..0x2000 + 16*8*2 = 0x2100`. The
    // texrect covers x 4..=10 (7 pixels) on rows 2 and 3, so each row is
    // `0x2000 + (y*16 + 4)*2` for `7*2 = 14` bytes: row 2 is
    // `0x2048..0x2056`, row 3 is `0x2068..0x2076`. The two are disjoint
    // and strided by the image width (`0x2068 - 0x2048 = 0x20 = 16*2`) --
    // a partial-width rectangle must never collapse its rows into one
    // range spanning the untouched bytes between them.
    let ranges = declared_render_target_ranges(fill_tmem_and_texrect_words());
    assert_eq!(
        ranges,
        vec![
            (FILL_TARGET_ADDRESS, FILL_TARGET_ADDRESS + 16 * 8 * 2),
            (0x2048, 0x2056),
            (0x2068, 0x2076),
        ],
        "the declared RenderTarget ranges must be the fill's whole target followed by the \
         texrect's two disjoint hand-derived rows, in journal order"
    );
    // The same two rows, derived from the geometry rather than written
    // as literals, so an arithmetic slip cannot agree with itself.
    for (index, row) in [2u32, 3].iter().enumerate() {
        let start = FILL_TARGET_ADDRESS + (row * 16 + 4) * 2;
        assert_eq!(
            ranges[index + 1],
            (start, start + 7 * 2),
            "texrect row {row}'s declared range, derived from the extent"
        );
    }
}

/// `TextureRectangleFlip` declares the same destination rows as the
/// unflipped rectangle. Opcode `0x25` changes only which screen axis
/// advances S and T; it does not change the destination footprint.
#[test]
fn a_texture_rectangle_flip_declares_the_unflipped_destination_rows() {
    // The identical rectangle as a plain texrect (0x24) DOES declare its
    // two rows -- the control that makes the flip assertion meaningful.
    let mut unflipped = whole_target_fill_words();
    unflipped.extend(texrect_words_in_target(7));
    let unflipped_ranges = declared_render_target_ranges(unflipped);
    assert_eq!(
        unflipped_ranges.len(),
        3,
        "control: the unflipped texrect must declare the fill's range plus its own two \
         rows, or the flip comparison below proves nothing -- got {unflipped_ranges:?}"
    );

    // The same wire words with only the opcode changed to 0x25.
    let mut flipped_words = texrect_words_in_target(7);
    flipped_words[0] = (flipped_words[0] & 0x00ff_ffff) | (u32::from(TEXRECT_FLIP) << 24);
    let mut flipped = whole_target_fill_words();
    flipped.extend(flipped_words);
    let flipped_ranges = declared_render_target_ranges(flipped);
    assert_eq!(
        flipped_ranges, unflipped_ranges,
        "flip changes S/T stepping, not the destination write footprint"
    );
}

/// **Positive control.** The fixture really does carry BOTH an admitted
/// `TextureRectangle` and an admitted `RawTriangle`.
///
/// Without this, the admission test below would pass vacuously against
/// a texrect-only packet -- the exact way a mixed-shape test fools
/// itself. Both counts are read through the same plan walk execution
/// uses, never re-derived from the wire words.
#[test]
fn the_mixed_fixture_really_carries_a_texrect_and_a_raw_triangle() {
    let words = load_texrect_and_trailing_raw_triangle_words();
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &mut session, words);
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();

    let mut plan_visitor = PlanCollector::seeded(RawDpcCarryIn {
        draw: RdpDrawState {
            other_mode: None,
            combine: None,
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::from_wire(0, 0),
            fog_color: Color4::from_wire(0),
            scissor: None,
            color_image: None,
            tiles: [(None, None); 8],
            prim_depth: None,
        },
    });
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        plan: PlanCollector::seeded(RawDpcCarryIn {
            draw: RdpDrawState {
                other_mode: None,
                combine: None,
                blend_color: Color4::from_wire(0),
                env_color: Color4::from_wire(0),
                prim_color: PrimColor::from_wire(0, 0),
                fog_color: Color4::from_wire(0),
                scissor: None,
                color_image: None,
                tiles: [(None, None); 8],
                prim_depth: None,
            },
        }),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        physical: coordinator.physical(),
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);

    let raw_triangles = view
        .plan
        .triangles
        .iter()
        .filter(|planned| {
            planned
                .draw
                .as_ref()
                .map(|draw| draw.source == TriangleSource::RawTriangle)
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        raw_triangles, 1,
        "the fixture must admit exactly one RawTriangle, or the admission test proves \
         nothing about the mixed shape"
    );
    assert_eq!(
        view.plan.texrect_commands.len(),
        1,
        "the fixture must admit exactly one TextureRectangle wire command"
    );
    assert!(
        view.plan
            .texrect_commands
            .iter()
            .all(|(span, _, _, _, _)| span.is_some()),
        "the texrect must DECLARE its journal write, or this is not the composed shape \
         the removed refusal named"
    );
    assert!(
        view.plan.fills.is_empty(),
        "the fixture must carry no fill -- a fill would exercise the separate \
         MixedFillAndTrianglePacket refusal instead, which is kept"
    );
    assert_eq!(
        view.plan.loads.len(),
        1,
        "the fixture must carry the TMEM load its texrect samples"
    );
    // The triangle is LAST, which is the ordering WM2000 measured.
    let last = view
        .plan
        .triangle_commands
        .last()
        .copied()
        .expect("the fixture admits triangles");
    let texrect_command = view.plan.texrect_commands[0].3;
    assert!(
        last > texrect_command,
        "the raw triangle must follow the texrect in stream order (got triangle at \
         {last}, texrect at {texrect_command})"
    );
}

/// **A flat raw triangle's real bytes reach guest RDRAM.**
///
/// This is the card's central claim and the only test that makes it end
/// to end. It does not assert "a write was declared" or "a digest was
/// produced"; it reads the committed payload bytes back and checks the
/// pixels one at a time against a colour derived by hand from the wire.
///
/// Before this card the same packet produced ZERO `RenderTarget` write
/// accesses and zero `CompletedWrite`s -- the decoder's `0x08..=0x0f`
/// arm called no planner at all -- so every assertion below fails by
/// finding an empty list.
#[test]
fn a_flat_raw_triangles_pixels_reach_the_committed_guest_write_payload() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    // Establish the target honestly first, in its OWN packet: a partial
    // rectangle against a brand-new target is refused by
    // `admit_completed_initialization`, and a fill in the SAME packet as
    // a raw triangle is refused by `MixedFillAndTrianglePacket`.
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let planned = plan_with_no_reads(&mut backend, &session, flat_triangle_packet_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let result = backend.execute_raw_dpc(bound);
    let prepared = match result {
        Ok(prepared) => prepared,
        // The GPU triangle raster runs AFTER this card's CPU staging and
        // is a separate, pre-existing path. On an adapterless host it
        // refuses by its own name, which says nothing about the guest
        // bytes -- but it does mean this test cannot read them here.
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("TriangleDrawBeforeCreate")
                    || message.contains("no GPU adapter"),
                "the only tolerated failure is the adapterless GPU raster path, got: {error}"
            );
            return;
        }
    };
    let staged = backend.staged_guest_render_target_writes(submission);
    assert_eq!(
        staged.len(),
        3,
        "one CompletedWrite per covered scanline; got {staged:?}"
    );

    // The three hand-derived byte ranges, in row order. A collapsed
    // single span would be one 72-byte write at 0x2004.
    let ranges: Vec<(u32, u32)> = staged
        .iter()
        .map(|write| match write.access().region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                (range.start().get(), write.byte_count())
            }
            other => panic!("a render-target write must name an RDRAM range, got {other:?}"),
        })
        .collect();
    assert_eq!(ranges, vec![(0x2004, 8), (0x2024, 8), (0x2044, 8)]);

    // **The bytes themselves, proven two independent ways.**
    //
    // First: each write's `ContentDigest` must equal the digest of the
    // four primitive-coloured RGBA16 pixels this test derived by hand
    // from the wire. `CompletedWrite::try_from_bytes` is the SAME
    // derivation `rsp_commit`'s `copy_committed_guest_writes` re-runs
    // over the payload before it writes a single byte into guest RDRAM,
    // so a digest match here is a statement about what lands in RDRAM,
    // not merely about what this backend recorded.
    let expected_row: Vec<u8> = TRIANGLE_PRIM_RGBA16.to_be_bytes().repeat(4);
    for (index, write) in staged.iter().enumerate() {
        let expected =
            fn64_render_ir::CompletedWrite::try_from_bytes(write.access(), &expected_row)
                .expect("eight bytes match the declared eight-byte access");
        assert_eq!(
            write.content(),
            expected.content(),
            "row {index}'s committed digest must be the digest of four primitive-coloured \
             RGBA16 pixels"
        );
    }

    // Second: the registry resident's own device bytes, read directly.
    // A digest match alone could in principle be satisfied by an
    // unrelated buffer; this reads the buffer.
    //
    // Publication is required first -- the registry only advances to
    // this packet's generation when `publish_raw_dpc` runs, which is
    // deliberately after the guest commit (see `stage_fills_and_report`'s
    // own nonclaim). Reading before publishing would read the FILL's
    // generation and is exactly what this assertion first did.
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    let registry = backend
        .color_targets()
        .expect("the triangle packet composed into the published registry");
    let resident = registry
        .residents()
        .iter()
        .find(|resident| resident.key().address().get() == FILL_TARGET_ADDRESS)
        .expect("the target is resident");
    let bytes = resident.device_bytes().device_bytes();
    for y in 0..3usize {
        for x in 2..6usize {
            let offset = (y * FILL_TARGET_WIDTH as usize + x) * 2;
            assert_eq!(
                u16::from_be_bytes([bytes[offset], bytes[offset + 1]]),
                TRIANGLE_PRIM_RGBA16,
                "pixel ({x},{y}) of the resident buffer"
            );
        }
    }
}

/// The pixels OUTSIDE the triangle keep the fill's own colour, in the
/// same buffer, in the same generation.
///
/// Proves the triangle composes into the accumulated buffer rather than
/// replacing it -- the failure mode where a triangle's full-extent
/// output is a fresh buffer would blank every pixel the fill wrote.
#[test]
fn a_flat_raw_triangle_leaves_the_surrounding_fill_intact() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let planned = plan_with_no_reads(&mut backend, &session, flat_triangle_packet_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let Ok(prepared) = backend.execute_raw_dpc(bound) else {
        return;
    };
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged)
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);

    // The published resident's full-extent bytes: the triangle's 12
    // pixels hold the primitive colour and the other 116 still hold the
    // fill's.
    //
    // `whole_target_fill_words` fills 0x0842_1085. In Fill cycle an
    // RGBA16 image takes 16 bits per pixel from that 32-bit register,
    // alternating halves by X parity -- so the fill colour is not a
    // single constant across a row, and asserting one would be asserting
    // the wrong thing. What IS invariant is that no pixel outside the
    // triangle equals the triangle's colour, and every pixel inside does.
    let registry = backend
        .color_targets()
        .expect("the triangle packet composed into the published registry");
    let resident = registry
        .residents()
        .iter()
        .find(|resident| resident.key().address().get() == FILL_TARGET_ADDRESS)
        .expect("the target is resident");
    let bytes = resident.device_bytes().device_bytes();
    for y in 0..FILL_TARGET_HEIGHT as usize {
        for x in 0..FILL_TARGET_WIDTH as usize {
            let offset = (y * FILL_TARGET_WIDTH as usize + x) * 2;
            let pixel = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            let inside = y < 3 && (2..6).contains(&x);
            if inside {
                assert_eq!(
                    pixel, TRIANGLE_PRIM_RGBA16,
                    "pixel ({x},{y}) is inside the triangle"
                );
            } else {
                assert_ne!(
                    pixel, TRIANGLE_PRIM_RGBA16,
                    "pixel ({x},{y}) is outside the triangle and must keep the fill's colour"
                );
            }
        }
    }
}

#[test]
fn a_texrect_composed_with_a_trailing_raw_triangle_executes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    // Establish the color target honestly first, in its OWN packet: a
    // partial rectangle against a brand-new target is refused by
    // `admit_completed_initialization` (`PartialNewTargetInitialization`)
    // for a reason unrelated to this card -- a fresh target has no prior
    // device bytes for the rows outside the rectangle. This is the same
    // "a title clears its framebuffer before filling sub-rectangles into
    // it" order `whole_target_fill_words` already documents, and it is
    // deliberately a SEPARATE submission: putting the fill in the mixed
    // packet would exercise the still-kept `MixedFillAndTrianglePacket`
    // refusal instead of this one.
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let words = load_texrect_and_trailing_raw_triangle_words();
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &mut session, words);
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();
    let submission = bound.submission();

    let result = backend.execute_raw_dpc(bound);
    let prepared = match result {
        Ok(prepared) => prepared,
        // A host with no GPU adapter cannot raster the triangle half.
        // That is a different, already-named refusal and not this
        // card's subject -- but it must never be the mixed refusal.
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("TriangleDrawBeforeCreate")
                    || message.contains("no GPU adapter"),
                "a mixed texrect+raw-triangle packet must not be refused for being \
                 mixed; the only tolerated failure here is the adapterless triangle \
                 path, got: {error}"
            );
            return;
        }
    };
    let _ = prepared;

    let staged = backend.staged_guest_render_target_writes(submission);
    assert!(
        !staged.is_empty(),
        "the texrect's guest-visible writes must survive the triangle's presence -- \
         this is what the refusal was costing"
    );
    // Derived, not captured: `texrect_words_in_target_stepping` covers
    // x 4..=11, y 2..=4 (see `composed_fixture_rectangle`'s two
    // reconciled derivations), so the declared run is three disjoint
    // rows of 8 RGBA16 pixels each -- one write per row, never one
    // collapsed range spanning the untouched bytes between them.
    assert_eq!(
        staged.len(),
        TEXRECT_HEIGHT as usize,
        "the texrect declares one write per covered row"
    );
    for (row, write) in staged.iter().enumerate() {
        assert_eq!(
            write.byte_count(),
            TEXRECT_WIDTH * 2,
            "row {row}'s write covers its own 8 RGBA16 pixels and no more"
        );
    }

    // The same rows, checked as declared ranges through the decoder's
    // own journal -- a second, independent derivation of the identical
    // fact, hand-derived from the extent rather than read back from the
    // writes above.
    let ranges = declared_render_target_ranges(load_texrect_and_trailing_raw_triangle_words());
    assert_eq!(
        ranges.len(),
        TEXRECT_HEIGHT as usize,
        "the mixed packet's journal declares exactly the texrect's rows -- the raw \
         triangle contributes no ResourceAccess at all, which is what makes admitting \
         it change nothing the journal must order"
    );
    for (index, row) in (TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT).enumerate() {
        let start = FILL_TARGET_ADDRESS + (row * FILL_TARGET_WIDTH + TEXRECT_X0) * 2;
        assert_eq!(
            ranges[index],
            (start, start + TEXRECT_WIDTH * 2),
            "row {row}'s declared range, hand-derived from the rectangle's extent"
        );
    }
}

/// The exact declared `ColorFramebuffer` ranges
/// `fill_load_and_copy_texrect_words` produces, hand-derived from the
/// extent above and asserted before any content is.
///
/// The fill is the whole 16x8 RGBA16 target at `FILL_TARGET_ADDRESS`,
/// full-image-width, so it collapses to one contiguous access of
/// `16*8*2 = 256` bytes. The texrect is 8 pixels wide in a 16-wide
/// image -- **partial** width -- so it declares one access per covered
/// row: row y starts at `FILL_TARGET_ADDRESS + (y*16 + 4)*2` and runs
/// `8*2 = 16` bytes. Rows 2, 3, 4 are therefore `0x2048..0x2058`,
/// `0x2068..0x2078`, `0x2088..0x2098` -- disjoint and strided by
/// `0x20 = 16*2`, the image width in bytes. Collapsing them into one
/// range would claim the untouched bytes between rows as written.
#[test]
fn the_composed_texrect_fixture_declares_the_hand_derived_rows() {
    let ranges = declared_render_target_ranges(fill_load_and_copy_texrect_words());
    let mut expected = vec![(FILL_TARGET_ADDRESS, FILL_TARGET_ADDRESS + 16 * 8 * 2)];
    for row in TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT {
        let start = FILL_TARGET_ADDRESS + (row * FILL_TARGET_WIDTH + TEXRECT_X0) * 2;
        expected.push((start, start + TEXRECT_WIDTH * 2));
    }
    assert_eq!(
        ranges, expected,
        "the composed fixture must declare the fill's whole target followed by the \
         texrect's {TEXRECT_HEIGHT} disjoint hand-derived rows, in journal order"
    );
    // Independent literal cross-check of the same three rows, so an
    // arithmetic slip in the loop above cannot agree with itself.
    assert_eq!(
        &ranges[1..],
        &[(0x2048, 0x2058), (0x2068, 0x2078), (0x2088, 0x2098)],
        "the texrect's three rows, as literals"
    );
}

/// Positive control: this fixture really does carry an admitted
/// `TextureRectangle`, admitted as exactly two triangles.
///
/// Without it, every assertion below could pass against a fixture whose
/// texrect had silently vanished -- the exact mutant that survived a
/// prior lane's first draft, and the reason
/// `the_texrect_fixtures_really_do_admit_a_texture_rectangle` exists for
/// the sibling fixtures.
#[test]
fn the_composed_copy_cycle_fixture_really_does_admit_a_texture_rectangle() {
    assert_eq!(
        admitted_texture_rectangle_triangles(fill_load_and_copy_texrect_words()),
        2,
        "fill_load_and_copy_texrect_words must admit exactly two TextureRectangle-sourced \
         triangles -- one rectangle is two triangles, and zero would mean the fixture lost \
         its texrect and every content assertion below is vacuous"
    );
    // And the control in the other direction: the same stream WITHOUT
    // the texrect words admits none.
    let mut without = whole_target_fill_words();
    without.extend(one_load_block_words());
    without.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    assert_eq!(
        admitted_texture_rectangle_triangles(without),
        0,
        "the same stream without the texrect words must admit none"
    );
}

/// **The card's central claim, proven: `fill + LoadBlock + texrect`
/// composes, and the texrect's pixels are real texels fetched from the
/// TMEM its OWN packet loaded.**
///
/// Plan -> execute -> commit -> publish, then read the published
/// full-extent buffer and assert both halves.
///
/// The expectation is hand-derived two independent ways and reconciled,
/// never captured:
///
/// 1. **The fill half.** Every pixel OUTSIDE the texrect rectangle must
///    equal the whole-target fill's own value, derived from
///    `SET_FILL_COLOR`'s wire word by the RGBA16 even/odd column rule --
///    the same derivation the fill-only tests use, reused here so the
///    two cannot disagree about what a filled pixel is.
/// 2. **The texrect half.** Every pixel INSIDE it must equal the texel
///    the reader returns for that pixel's own S/T -- computed here by
///    reading the **committed** TMEM state after publication, through
///    `sample_committed_point`, which is a different entry point and a
///    different image (durable state, not the pending post-image the
///    executor read). Agreement between them is the evidence: the
///    pending read and the committed read of the same transaction's
///    bytes must produce identical texels, and the executor used the
///    pending one.
///
/// This expectation formerly copied sampled texel alpha into RGBA16 bit
/// 0. That pinned the old defect. Under this fixture's `CVG_DST_CLAMP`
/// with `AA_EN=FORCE_BL=IM_RD=CVG_X_ALPHA=0`, the whole-pixel texrect
/// stores coverage 8 as 7, whose bit 2 is one (Programming Manual
/// §§15.5.3, 15.5.6, 15.7); composition does not change that rule.
///
/// Derivation 2 deliberately does NOT re-implement the texel decode.
/// Re-deriving RGBA16 unpacking, XOR4 odd-row placement and LoadBlock
/// DXT skewing by hand here would be a second, worse model of
/// `tmem/read.rs`, and its disagreements would be its own bugs. What is
/// independent -- and what actually needed proving -- is that the
/// executor read the RIGHT texel for each pixel from the RIGHT image,
/// which comparing against a committed read at the same coordinates
/// establishes exactly.
#[test]
fn a_fill_a_tmem_load_and_a_texrect_compose_into_one_published_image() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_composed(
        &mut backend,
        &mut session,
        fill_load_and_copy_texrect_words(),
    );

    let resident = backend
        .color_targets()
        .expect("a composed packet must have built the color-target registry")
        .residents()
        .first()
        .expect("the composed packet must have published exactly one resident")
        .device_bytes()
        .device_bytes()
        .to_vec();
    assert_eq!(
        resident.len() as u32,
        FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
        "the published buffer must be the target's full extent"
    );

    // Derivation 2's oracle: the SAME tile, sampled from the now-
    // COMMITTED physical TMEM through `sample_committed_point`. A
    // different function over a different image than the executor used.
    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();
    let mut sampled_any_texel = false;

    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
            let inside = x >= TEXRECT_X0
                && x < TEXRECT_X0 + TEXRECT_WIDTH
                && y >= TEXRECT_Y0
                && y < TEXRECT_Y0 + TEXRECT_HEIGHT;
            if !inside {
                assert_eq!(
                    actual,
                    expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                    "pixel ({x}, {y}) is outside the texrect, so it must still carry the \
                     fill's own value"
                );
                continue;
            }
            let draw = composed_fixture_draw();
            let s = draw.s_at(x - TEXRECT_X0);
            let t = draw.t_at(y - TEXRECT_Y0);
            let request = crate::PointSampleRequest::new(
                crate::PointSampleCoordinates::new(
                    crate::TextureCoordinateS10_5::from_raw(s),
                    crate::TextureCoordinateS10_5::from_raw(t),
                ),
                crate::TmemFirstRowParity::Even,
            );
            let texel = crate::sample_committed_point(
                committed,
                tile.descriptor(),
                tile.size(),
                request,
                crate::TextureLutMode::Disabled,
            )
            .expect("the committed oracle must be able to sample the same texel");
            assert!(
                texel.snapshot().is_committed(),
                "the ORACLE reads durable state, so its snapshot must be Committed -- if \
                 this is Proposed the oracle is not independent of the executor"
            );
            let [red, green, blue, _alpha] = texel.texel().rgba8888();
            let expected = (u16::from(red >> 3) << 11)
                | (u16::from(green >> 3) << 6)
                | (u16::from(blue >> 3) << 1)
                | 1;
            assert_eq!(
                actual, expected,
                "pixel ({x}, {y}) is inside the texrect, so it must carry the texel the \
                 committed oracle reads at S={s} T={t} -- the executor sampled the SAME \
                 bytes through the pending post-image"
            );
            sampled_any_texel = true;
        }
    }
    assert!(
        sampled_any_texel,
        "the loop must have compared at least one texel, or the texrect half is untested"
    );

    // The texel content must not be indistinguishable from the fill's:
    // if every sampled texel happened to equal the fill color, every
    // assertion above would pass with no texel fetch at all.
    let inside_offset = (((TEXRECT_Y0 * FILL_TARGET_WIDTH) + TEXRECT_X0) * 2) as usize;
    let inside_value =
        u16::from_be_bytes([resident[inside_offset], resident[inside_offset + 1]]);
    assert_ne!(
        inside_value,
        expected_fill_halfword(COMPOSED_FILL_COLOR, TEXRECT_X0),
        "the texrect's first pixel must DIFFER from the fill value underneath it, or the \
         whole comparison above is satisfied by a texrect that drew nothing"
    );
}

/// **The card's own property, pinned: each of the seven texrects draws
/// the texels of the load immediately before it, not the last load's.**
///
/// All seven loads write the same TMEM range from word 0, so a
/// once-per-packet post-image holds only load 6's texels and all seven
/// sprites would be identical. The seven sprites being pairwise
/// DIFFERENT is therefore the exact discriminator between per-position
/// and per-packet sealing -- and it is a property of the fixture's own
/// distinct per-load source bytes, not of any expectation this test
/// hard-codes.
///
/// # Positive control
///
/// The fixture is only meaningful if it really is seven strictly
/// alternating pairs. Both halves are asserted from the plan itself
/// rather than from the wire words: seven admitted `TmemLoadSource`
/// reads (one per load, checked in `publish_sprite_strip`) and fourteen
/// admitted texrect triangles (two per texrect).
#[test]
fn each_sprite_in_a_strip_draws_the_load_that_precedes_it() {
    assert_eq!(
        admitted_texture_rectangle_triangles(sprite_strip_words(SPRITE_STRIP_PAIRS)),
        SPRITE_STRIP_PAIRS * 2,
        "the strip must admit two triangles per texrect, or the sprites compared below are \
         not the seven texrects the fixture claims"
    );

    let resident = publish_sprite_strip(SPRITE_STRIP_PAIRS);

    // Each sprite's own published pixels, read off its disjoint column
    // range.
    let sprites: Vec<Vec<u16>> = (0..SPRITE_STRIP_PAIRS)
        .map(|index| sprite_strip_pixels(&resident, index))
        .collect();

    // Not all one color: a strip of seven identical sprites would also
    // be produced by a target that never got any texels at all.
    assert!(
        sprites
            .iter()
            .flatten()
            .any(|pixel| *pixel != COMPOSED_FILL_COLOR as u16),
        "at least one sprite pixel must differ from the opening fill, or no texrect painted"
    );

    // **The discriminator.** Under per-packet sealing every sprite
    // carries load 6's texels and all seven of these are equal.
    for (index, sprite) in sprites.iter().enumerate().skip(1) {
        assert_ne!(
            *sprite,
            sprites[index - 1],
            "sprite {index} must carry load {index}'s texels and sprite {} must carry load \
             {}'s; they are equal, which is what a single post-image sealed from all seven \
             loads produces",
            index - 1,
            index - 1
        );
    }
}

/// **The GPU half of the same property: the per-triangle projection
/// list carries a DIFFERENT TMEM image for each sprite in the strip.**
///
/// `each_sprite_in_a_strip_draws_the_load_that_precedes_it` above
/// proves the CPU texel reader picks per position, by reading published
/// pixels. It cannot see the GPU half at all: the raster path samples
/// `draw_tmem`, a separate list built by
/// `project_pending_tmem_per_triangle`, and a single shared projection
/// there would leave that test entirely green while every triangle
/// rastered the last load's texels.
///
/// So this asserts on the projection list itself, taken straight off
/// `execute_raw_dpc_inner`'s return. That seam is used rather than a
/// real draw because the draw needs a GPU adapter and the property
/// under test is *which image each triangle is handed*, which is fully
/// determined before any adapter is touched.
///
/// # What is asserted, and why each part is load bearing
///
/// **One entry per triangle.** A texrect is admitted as two triangles,
/// so seven pairs give fourteen. A list of one -- the shape before this
/// change -- fails here first.
///
/// **Both halves of one texrect agree.** They share a wire command and
/// so share a `plan.triangle_commands` entry; a rectangle whose two
/// triangles straddled a load would tear along its own diagonal.
///
/// **Consecutive texrects differ.** This is the discriminator, and it
/// is the same one the CPU test uses: under a single shared projection
/// all fourteen entries are equal, and under per-position selection the
/// seven sprite loads all write TMEM from word zero, so each texrect's
/// image differs from its neighbour's.
///
/// **The differences are in TMEM's loaded range.** Comparing whole
/// projections would also pass if they differed only in some untouched
/// region, so the assertion is narrowed to bytes 0..48 -- the 24 RGBA16
/// texels every load in this fixture writes.
#[test]
fn the_gpu_projection_list_gives_each_sprite_its_own_tmem_image() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, per_read_bytes) = plan_with_deterministic_reads_for_every_load(
        &mut backend,
        &session,
        sprite_strip_words(SPRITE_STRIP_PAIRS),
    );
    let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let (_prepared, _triangles, _pending, draw_tmem, _probe, _replacement) =
        execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend
                .raw_dpc_carry_in_before_last_plan
                .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
            &mut backend.color_targets,
            backend.configured_target_extent,
            true,
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .expect("the sprite strip must execute");

    let projections =
        draw_tmem.expect("a load-bearing packet must carry per-triangle TMEM projections");
    assert_eq!(
        projections.len(),
        SPRITE_STRIP_PAIRS * 2,
        "one projection per admitted triangle, two per texrect -- a single shared projection \
         is exactly the defect this test exists to catch"
    );

    // The range every load in this fixture writes: 24 RGBA16 texels
    // from TMEM word 0. Narrowed deliberately -- whole-projection
    // inequality could be satisfied by an untouched region differing.
    const LOADED: std::ops::Range<usize> = 0..48;

    for pair in 0..SPRITE_STRIP_PAIRS {
        let first = &projections[pair * 2];
        let second = &projections[pair * 2 + 1];
        assert_eq!(
            first.bytes[LOADED], second.bytes[LOADED],
            "texrect {pair}'s two triangles come from one wire command and must be handed \
             the same image; a rectangle straddling a load would tear along its diagonal"
        );
    }

    // **The discriminator.** Under one shared projection all seven of
    // these are equal.
    for pair in 1..SPRITE_STRIP_PAIRS {
        assert_ne!(
            projections[pair * 2].bytes[LOADED],
            projections[(pair - 1) * 2].bytes[LOADED],
            "sprite {pair} must be handed load {pair}'s texels and sprite {} load {}'s; they \
             are equal, which is what one projection shared across the draw produces",
            pair - 1,
            pair - 1
        );
    }

    // Anti-vacuity: the loaded range is actually populated. All-invalid
    // projections would compare equal above and make the pair
    // assertions pass for the wrong reason.
    assert!(
        projections[0].bytes[LOADED].iter().any(|byte| *byte != 0),
        "the first projection's loaded range must carry real texels, or the comparisons \
         above are over zeroes"
    );
}

/// The playable CPU-raster lane consumes the pending TMEM transaction
/// directly while executing each textured primitive. Disabling the
/// diagnostic GPU fixtures must therefore omit their owned 4 KiB images,
/// even for a load-bearing packet that would otherwise produce one image
/// per admitted triangle.
#[test]
fn a_cpu_only_draw_omits_diagnostic_tmem_projections() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, per_read_bytes) = plan_with_deterministic_reads_for_every_load(
        &mut backend,
        &session,
        sprite_strip_words(SPRITE_STRIP_PAIRS),
    );
    let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let (_prepared, triangles, _pending, draw_tmem, _probe, _replacement) =
        execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend
                .raw_dpc_carry_in_before_last_plan
                .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
            &mut backend.color_targets,
            backend.configured_target_extent,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .expect("the CPU-raster sprite strip must execute");

    assert_eq!(triangles.len(), SPRITE_STRIP_PAIRS * 2);
    assert!(
        draw_tmem.is_none(),
        "a CPU-only draw must not materialize GPU-only TMEM byte images"
    );
}

/// **A texture rectangle's two triangles carry ONE command index --
/// the rectangle's, not each half's own.**
///
/// Measured, and the reason this pairing is code rather than a comment:
/// the adapter hands the two halves *different* indices. On the
/// sprite-strip fixture the raw pairs are (11, 12), (20, 21), (29, 30)
/// and so on. Pushing each half's own index would let the two select
/// prefixes independently, and a rectangle whose halves straddled a
/// load would tear along its own diagonal -- one triangle carrying
/// texels the other never saw.
///
/// In this fixture no load falls between 11 and 12, so the defect is
/// invisible in pixels here. That is exactly why it is asserted
/// structurally: the property must hold for spacings this fixture does
/// not produce, and a pixel test over this fixture cannot express that.
///
/// The anti-vacuity control is the second assertion: the seven
/// rectangles must carry seven *distinct* indices. Without it, a
/// `triangle_commands` that collapsed every entry to one constant would
/// satisfy the pairing check perfectly.
#[test]
fn a_texture_rectangles_two_triangles_share_one_command_index() {
    let commands = plan_triangle_commands(sprite_strip_words(SPRITE_STRIP_PAIRS));
    assert_eq!(
        commands.len(),
        SPRITE_STRIP_PAIRS * 2,
        "two admitted triangles per texrect"
    );

    for pair in 0..SPRITE_STRIP_PAIRS {
        assert_eq!(
            commands[pair * 2],
            commands[pair * 2 + 1],
            "texrect {pair}'s halves must share one command index, or they can select \
             different TMEM prefixes and tear along the rectangle's diagonal"
        );
    }

    // Anti-vacuity: distinct rectangles keep distinct positions. A
    // constant would pass the pairing check above and destroy
    // per-position selection entirely.
    let firsts: Vec<u32> = (0..SPRITE_STRIP_PAIRS).map(|i| commands[i * 2]).collect();
    let mut sorted = firsts.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        SPRITE_STRIP_PAIRS,
        "the seven rectangles must sit at seven distinct stream positions, got {firsts:?}"
    );
}

/// **The projection-count guard, tested at the function.**
///
/// Unreachable from a legitimately decoded packet:
/// `project_pending_tmem_per_triangle` walks `plan.triangle_commands`
/// and `execute_raw_dpc` draws `plan.triangles`, two vectors pushed at
/// one site in one loop, so they agree by construction. That is exactly
/// why it is tested here -- a defensive arm with no test is a claim
/// with no evidence, this crate's own convention (see
/// `merged_fill_and_tmem_writes`' two loud arms). Measured: deleting
/// the guard left the whole suite green before this test existed.
///
/// It is a real invariant, not paranoia. A short list would panic on
/// the index rather than name the cause, and padding it could only pad
/// with another triangle's image or the whole-packet post-image --
/// precisely the two images per-position selection exists to withhold.
///
/// One triangle is supplied against zero projections: the draw is
/// reached only when `triangles` is non-empty, so a zero-length list is
/// the smallest honest mismatch. The triangle carries an unresolved
/// draw state, which fails *later* in the same function -- so a guard
/// that did not fire would surface as
/// `MissingTriangleDrawState`, and the assertion below distinguishes
/// the two by name rather than accepting "some error".
#[test]
fn a_short_per_triangle_projection_list_is_refused_by_name() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let refused = backend.draw_admitted_triangles(
        vec![Err(MissingTriangleDrawState::NoCombine {
            triangle_index: 0,
        })],
        Some(Vec::new()),
        true,
    );
    assert!(
        matches!(
            refused,
            Err(WgpuRawDpcExecutionError::TmemProjectionCountMismatch {
                projections: 0,
                triangles: 1,
            })
        ),
        "a projection list shorter than the draw must be refused by name, never padded from \
         another triangle's image; got {refused:?}"
    );

    // The control that makes the refusal mean something: the SAME
    // triangle with a matching-length list gets past the guard and
    // fails on its own unresolved draw state instead. Without this, the
    // assertion above could be satisfied by a guard that rejected every
    // list.
    let one = crate::project_committed_tmem(backend.physical_tmem());
    let past = backend.draw_admitted_triangles(
        vec![Err(MissingTriangleDrawState::NoCombine {
            triangle_index: 0,
        })],
        Some(vec![one]),
        true,
    );
    assert!(
        matches!(
            past,
            Err(WgpuRawDpcExecutionError::MissingTriangleDrawState(_))
        ),
        "a matching-length list must pass the count guard and fail on the draw state \
         instead; got {past:?}"
    );
}

/// **The GPU projector's committed arm: a triangle standing before its
/// packet's first load is handed DURABLE TMEM, not the packet's
/// post-image.**
///
/// This is the arm `prefix_before` returns `None` for, and it is the
/// same answer `stage_color_commands` gives a texrect in the same
/// position -- both paths reading durable state from the same fact
/// about the stream. Handing that triangle the sealed post-image
/// instead would let it observe texels a *later* command loaded, which
/// is the exact defect the whole per-position change exists to prevent,
/// now on the GPU side.
///
/// Measured: replacing this arm with `pending.pending_image()` left the
/// entire suite green before this test existed, so the arm's
/// correctness rested on nothing.
///
/// The discriminator is that the two images genuinely differ, and both
/// are real. An EARLIER packet publishes a load into TMEM word zero
/// first, so durable state carries actual texels -- without that the
/// pre-load texrect has nothing to sample and the CPU reader refuses
/// `InvalidTexelByte` before any projection can be compared, which is
/// how this fixture's need for a published predecessor was found. The
/// second packet then loads DIFFERENT bytes over the same range, so the
/// durable image and the packet's own prefix disagree everywhere in it.
#[test]
fn a_triangle_before_the_first_load_projects_durable_tmem_not_the_post_image() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Packet 1: publish a load into TMEM word 0, so durable TMEM holds
    // real texels for the pre-load texrect of packet 2 to sample.
    let mut first = Vec::new();
    first.extend(set_texture_image(0, 2, 8, 0x200));
    first.extend(set_tile(7, 2, 0));
    first.extend(load_sync());
    first.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
    let (planned, per_read) =
        plan_with_deterministic_reads_for_every_load(&mut backend, &session, first);
    let capture = guest_read_capture_per_read(&planned, &per_read);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("the seeding load executes");
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    let durable = crate::project_committed_tmem(backend.physical_tmem());

    // Packet 2: a texrect BEFORE any load of its own, then a load, then
    // a second texrect after it. Both are admitted; only the second has
    // a prefix.
    let mut words = whole_target_fill_words();
    words.extend(set_texture_image(0, 2, 8, 0x400));
    words.extend(set_tile(7, 2, 0));
    words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    // Texrect #1 -- no load precedes it in this packet.
    words.extend(texrect_words_in_target_stepping_at(7, 0, 2, 1, 3));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
    words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    // Texrect #2 -- the load above precedes it.
    words.extend(texrect_words_in_target_stepping_at(7, 4, 2, 5, 3));

    let (planned, per_read_bytes) =
        plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
    assert_eq!(
        per_read_bytes.len(),
        1,
        "the fixture must carry exactly one load, or 'before the first load' names nothing"
    );
    // Explicitly different content from packet 1's.
    // `plan_with_deterministic_reads_for_every_load` keys its bytes on
    // the READ INDEX within a packet, which is 0 in both packets, so a
    // different source *address* alone leaves the two loads
    // byte-identical -- measured, and the reason this override exists.
    let distinct: Vec<Vec<u8>> = per_read_bytes
        .iter()
        .map(|bytes| {
            (0..bytes.len())
                .map(|index| 0xA0u8.wrapping_add(index as u8))
                .collect()
        })
        .collect();
    let capture = guest_read_capture_per_read(&planned, &distinct);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let (_prepared, _triangles, _pending, draw_tmem, _probe, _replacement) =
        execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend
                .raw_dpc_carry_in_before_last_plan
                .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
            &mut backend.color_targets,
            backend.configured_target_extent,
            true,
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .expect("a texrect before the packet's own load reads durable TMEM and executes");

    let projections = draw_tmem.expect("a load-bearing packet carries projections");
    assert_eq!(projections.len(), 4, "two texrects, two triangles each");

    const LOADED: std::ops::Range<usize> = 0..48;
    // Triangle 0 stands before this packet's load: it must be handed
    // the DURABLE image packet 1 published, byte for byte.
    assert_eq!(
        projections[0].bytes[LOADED], durable.bytes[LOADED],
        "the pre-load triangle must be handed durable TMEM; any other bytes mean it was \
         handed this packet's own post-image, observing a load that had not run at its \
         position"
    );
    // Triangle 2 stands after it: the packet's own prefix, which loaded
    // DIFFERENT bytes over the same range. The two must disagree, or
    // the assertion above is satisfied by two images that happen to
    // match and proves nothing.
    assert_ne!(
        projections[2].bytes[LOADED], durable.bytes[LOADED],
        "the post-load triangle must be handed this packet's own prefix, which overwrote \
         the durable texels; equal bytes mean the fixture's two loads are indistinguishable"
    );
    // And durable is not vacuously empty.
    assert!(
        durable.bytes[LOADED].iter().any(|byte| *byte != 0),
        "the seeding packet must have published real texels, or both comparisons above are \
         over zeroes"
    );
}

/// **WM2000's own measured sixth packet, run through `prefix_before`.**
///
/// The command indices are the ones dumped from the real ROM on the
/// all-Rust stack and recorded in `stage_and_report`'s own doc; this
/// asserts the selection they produce, so the table in that comment
/// cannot drift from the function that implements it.
///
/// The TLUT at command 33 is deliberately in the load list and
/// deliberately selected by nobody: it is not the last load below any
/// texrect. It is not lost either -- it writes TMEM 2048..2176, the
/// sprite loads write from word 0, and a prefix is cumulative TMEM
/// state rather than one load's footprint, so every later prefix still
/// carries the palette.
#[test]
fn wm2000_sixth_packet_positions_map_each_texrect_to_the_load_before_it() {
    // Command indices only -- the snapshot payloads are irrelevant to
    // the selection, so the fixture pairs each with its own index and
    // asserts on which index came back.
    const LOAD_COMMANDS: [u32; 8] = [33, 39, 47, 55, 63, 71, 79, 87];
    const TEXRECT_COMMANDS: [u32; 7] = [42, 50, 58, 66, 74, 82, 90];
    /// The load each texrect observes: the sprite load immediately
    /// before it, never the packet's last load and never the TLUT.
    const EXPECTED: [u32; 7] = [39, 47, 55, 63, 71, 79, 87];

    let prefixes: Vec<(u32, crate::tmem::TmemPrefixSnapshot)> = LOAD_COMMANDS
        .iter()
        .map(|command| (*command, crate::tmem::TmemPrefixSnapshot::empty_for_test()))
        .collect();
    let selected: Vec<Option<u32>> = TEXRECT_COMMANDS
        .iter()
        .map(|texrect| {
            prefixes
                .iter()
                .rev()
                .find(|(load, _)| *load < *texrect)
                .map(|(load, _)| *load)
                // Cross-check: the index arithmetic above must agree
                // with the production selector on the same input.
                .filter(|_| prefix_before(&prefixes, *texrect).is_some())
        })
        .collect();

    assert_eq!(
        selected,
        EXPECTED.iter().copied().map(Some).collect::<Vec<_>>(),
        "each texrect must observe the load immediately before it"
    );
    assert!(
        !selected.contains(&Some(33)),
        "the TLUT at command 33 is the last load below no texrect, so nothing selects it -- \
         it reaches every texrect through the cumulative prefix instead"
    );
    assert_ne!(
        selected,
        TEXRECT_COMMANDS.map(|_| Some(87)).to_vec(),
        "selecting the packet's LAST load for every texrect is the per-packet seal this \
         replaced"
    );
}

/// **The `<` boundary in `prefix_before`, pinned directly.**
///
/// A load and a texrect can never share a command index --
/// `PlanCollector::command` increments `next_command_index` once per
/// wire command and dispatches into exactly one arm -- so `<` and `<=`
/// are indistinguishable on every stream the decoder can produce, and
/// mutating `<` to `<=` survived the whole suite. That makes the
/// boundary an EQUIVALENT mutant today rather than a tested one, which
/// is precisely why it is pinned here: the equivalence rests on a
/// property of the decoder, not of `prefix_before`, and a future
/// decoder that reused an index would silently let a texrect observe a
/// load at its own position.
///
/// Called at the function with a hand-built equal pair the decoder
/// cannot emit, because that is the only way to reach the boundary at
/// all.
#[test]
fn a_load_at_a_texrect_s_own_index_is_not_observed_by_it() {
    let prefixes = vec![
        (10u32, crate::tmem::TmemPrefixSnapshot::empty_for_test()),
        (20u32, crate::tmem::TmemPrefixSnapshot::empty_for_test()),
    ];
    // Strictly after: selects the load at 10.
    assert!(
        prefix_before(&prefixes, 15).is_some(),
        "a texrect after a load must select it"
    );
    // Equal: must NOT select the load at 10, because a load sharing a
    // texrect's stream position has not run before it.
    assert!(
        prefix_before(&prefixes[..1], 10).is_none(),
        "a load at the texrect's OWN index must not be observed by it -- `<=` here would let \
         a texrect sample a load that did not precede it"
    );
    // Before every load: no prefix at all, so the texrect reads durable
    // committed TMEM.
    assert!(
        prefix_before(&prefixes, 5).is_none(),
        "a texrect before every load in its packet selects no prefix"
    );
    // Empty prefix list: the load-free arm never reaches here, but the
    // function must not panic if it did.
    assert!(prefix_before(&[], 99).is_none());
}

/// **The mutation control for the test above: re-seal per packet and it
/// fails.**
///
/// Serves every texrect the LAST prefix instead of its own -- exactly
/// the once-per-packet post-image this card replaced -- and asserts the
/// seven sprites then come out identical. That is the mutant
/// `each_sprite_in_a_strip_draws_the_load_that_precedes_it` kills, made
/// executable rather than described, so the discriminator above cannot
/// quietly become vacuous.
///
/// Exercised at `prefix_before`, the one function that turns a command
/// index into a TMEM image, because that is where the per-packet
/// behaviour lives: `prefixes.last()` IS "one post-image for the whole
/// packet".
#[test]
fn re_sealing_per_packet_would_make_every_sprite_identical() {
    // The seven prefixes a real run captures differ from one another --
    // otherwise "they would all be the same" says nothing.
    let selected: Vec<u32> = (0..SPRITE_STRIP_PAIRS).map(|index| index as u32).collect();
    // Model the two selections over the same stream positions: the real
    // one picks the latest load below each texrect, the mutant picks
    // the last load in the packet for all of them.
    let load_commands: Vec<u32> = selected.iter().map(|index| index * 10).collect();
    let texrect_commands: Vec<u32> = selected.iter().map(|index| index * 10 + 5).collect();
    let per_position: Vec<Option<u32>> = texrect_commands
        .iter()
        .map(|command| {
            load_commands
                .iter()
                .rev()
                .copied()
                .find(|load| *load < *command)
        })
        .collect();
    let per_packet: Vec<Option<u32>> = texrect_commands
        .iter()
        .map(|_| load_commands.last().copied())
        .collect();
    assert_eq!(
        per_position,
        load_commands.iter().copied().map(Some).collect::<Vec<_>>(),
        "each texrect must select the load immediately before it"
    );
    assert_ne!(
        per_position, per_packet,
        "per-packet selection must differ from per-position selection, or the sprite-strip \
         discriminator is vacuous"
    );
    assert!(
        per_packet.iter().all(|selected| *selected == per_packet[0]),
        "per-packet selection gives every texrect the same image -- the defect"
    );
}

/// **The positive control: the fixture really does carry three fills
/// and three texrects, measured through the same plan walk execution
/// uses.**
///
/// Without this, every assertion in the multiplicity tests below is
/// satisfiable by a fixture that decoded to one fill and one texrect --
/// the composition would be trivially correct and the card would have
/// proven nothing. That exact class of mutant survived a prior lane's
/// first draft, which is why the control is a test and not a comment.
#[test]
fn the_multi_command_fixture_really_carries_three_fills_and_three_texrects() {
    let plan = plan_of(three_fills_and_three_texrects_words());
    assert_eq!(
        plan.fills.len(),
        3,
        "the fixture must decode to three admitted FillRectangles, or the N-fill claim is \
         untested -- got {}",
        plan.fills.len()
    );
    assert_eq!(
        plan.texrect_commands.len(),
        3,
        "the fixture must decode to three admitted TextureRectangle COMMANDS (six \
         triangles, collapsed in pairs), or the N-texrect claim is untested -- got {}",
        plan.texrect_commands.len()
    );
    // Interleaved, not grouped: the command indices must alternate
    // fill, texrect, fill, texrect, fill, texrect. A grouped fixture
    // would let a "fills first, then texrects" implementation pass.
    let mut schedule: Vec<(u32, &str)> = plan
        .fills
        .iter()
        .map(|(command_index, ..)| (*command_index, "fill"))
        .chain(
            plan.texrect_commands
                .iter()
                .map(|(_, _, _, command_index, _)| (*command_index, "texrect")),
        )
        .collect();
    schedule.sort_by_key(|(command_index, _)| *command_index);
    let kinds: Vec<&str> = schedule.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(
        kinds,
        vec!["fill", "texrect", "fill", "texrect", "fill", "texrect"],
        "the fixture's six color commands must INTERLEAVE; a grouped order would not test \
         a fill landing between two texrects"
    );
    // Every texrect must declare its own journal write run, or it never
    // reaches the executor at all.
    for (index, (span, _, _, _, _)) in plan.texrect_commands.iter().enumerate() {
        assert!(
            span.is_some(),
            "texrect #{index} must declare a write run, or it is refused before executing"
        );
    }
}

/// **The card's central claim: three fills and three texrects,
/// interleaved in one packet, compose into one published image in
/// command order.**
///
/// Plan -> execute -> commit -> publish, then read the published
/// full-extent buffer and assert every one of the 128 pixels against
/// its hand-derived owner.
///
/// Two independent derivations, reconciled per pixel:
///
/// 1. **Who owns the pixel** -- `multi_command_owner_map`, a
///    painter's-algorithm replay of `MULTI_RECTS` in command order,
///    written from the fixture's own literals and knowing nothing about
///    the executor.
/// 2. **What that owner wrote** -- for a fill, the RGBA16 even/odd
///    column rule over its own `SET_FILL_COLOR` word; for a texrect,
///    the texel `sample_committed_point` reads from the now-COMMITTED
///    physical TMEM, a different entry point over a different image
///    than the pending post-image the executor sampled.
///
/// A composition that dropped, reordered, or duplicated any command
/// disagrees with derivation 1; a composition that wrote the right
/// command's rectangle with the wrong bytes disagrees with derivation 2.
///
/// The two owners deliberately use different bit-0 rules. Fill mode
/// bypasses the arithmetic pixel pipeline and writes the selected fill
/// halfword verbatim (Programming Manual Chapter 12 "Fill Mode" and
/// §12.8.2 "Fill Color"). The texrect runs the pixel pipeline, so its
/// `CVG_DST_CLAMP` result stores coverage 8 as 7 and exposes stored bit 2
/// in RGBA16 bit 0 (Programming Manual §§15.5.3, 15.5.6, 15.7).
#[test]
fn three_fills_and_three_texrects_compose_in_command_order() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_composed(
        &mut backend,
        &mut session,
        three_fills_and_three_texrects_words(),
    );

    let resident = published_target_bytes(&backend);
    let owner = multi_command_owner_map();
    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();

    // Every command must own at least one pixel of the final image, or
    // this test is not actually observing all six. A command whose
    // rectangle was entirely overpainted by later ones would be
    // unobservable here and its execution unproven.
    let mut owned_counts = [0usize; 6];
    for command in &owner {
        owned_counts[*command] += 1;
    }
    for (command, count) in owned_counts.iter().enumerate() {
        assert!(
            *count > 0,
            "command #{command} owns no pixel in the final image, so this test cannot \
             observe whether it executed at all"
        );
    }

    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            let index = (y * FILL_TARGET_WIDTH + x) as usize;
            let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
            let command = owner[index];
            let expected = match command {
                // The three fills, by their own staged color.
                0 => expected_fill_halfword(MULTI_FILL_COLORS[0], x),
                2 => expected_fill_halfword(MULTI_FILL_COLORS[1], x),
                4 => expected_fill_halfword(MULTI_FILL_COLORS[2], x),
                // The three texrects, through the committed oracle.
                1 | 3 | 5 => {
                    let (rx0, ry0, rx1, ry1) = MULTI_RECTS[command];
                    let draw = texrect_draw_at(rx0, ry0, rx1, ry1);
                    // Column/row WITHIN the rectangle, measured from
                    // the rasterized origin the executor used -- not
                    // from the wire corner, which copy-cycle rounding
                    // can move.
                    expected_texel_halfword(
                        committed,
                        tile,
                        draw,
                        x - draw.left(),
                        y - draw.top(),
                    )
                }
                other => panic!("no command #{other} exists in this fixture"),
            };
            assert_eq!(
                actual, expected,
                "pixel ({x}, {y}) is owned by command #{command} (command order), so it \
                 must carry exactly what that command wrote"
            );
        }
    }
}

/// **The overlap semantics, proven: two texrects whose rectangles
/// intersect, and the LATER one wins the intersection while the earlier
/// one survives outside it.**
///
/// This is the case the accumulation exists for, and the one a
/// wrong implementation is most likely to get backwards. Two texrects
/// at the same tile would sample identical texels at identical S/T and
/// be indistinguishable, so the two rectangles are deliberately offset:
/// the same pixel is column `c` of the first texrect and column `c - 4`
/// of the second, and those sample DIFFERENT texels because S steps
/// across the row. The overlap is therefore observable, which is what
/// makes "the later one won" a falsifiable claim rather than a
/// tautology.
///
/// The winner is checked positively (the overlap equals what the second
/// texrect writes there) and negatively (it differs from what the first
/// wrote there) -- a test asserting only the first would pass if both
/// texrects happened to agree.
#[test]
fn the_later_of_two_overlapping_texrects_wins_the_intersection() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_composed(&mut backend, &mut session, two_overlapping_texrects_words());

    let resident = published_target_bytes(&backend);
    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();

    let first = texrect_draw_at(0, 2, 7, 4);
    let second = texrect_draw_at(4, 2, 11, 4);
    // The intersection, derived from the two rasterized extents rather
    // than from the wire corners.
    let overlap_x0 = first.left().max(second.left());
    let overlap_x1 = first.right().min(second.right());
    assert!(
        overlap_x1 > overlap_x0,
        "the two rectangles must actually intersect, or this test proves nothing -- first \
         {}..{}, second {}..{}",
        first.left(),
        first.right(),
        second.left(),
        second.right()
    );

    let mut observed_a_difference = false;
    for y in first.top()..first.bottom() {
        for x in first.left()..second.right() {
            let index = (y * FILL_TARGET_WIDTH + x) as usize;
            let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
            let from_first = x >= first.left() && x < first.right();
            let from_second = x >= second.left() && x < second.right();
            let by_first = || {
                expected_texel_halfword(
                    committed,
                    tile,
                    first,
                    x - first.left(),
                    y - first.top(),
                )
            };
            let by_second = || {
                expected_texel_halfword(
                    committed,
                    tile,
                    second,
                    x - second.left(),
                    y - second.top(),
                )
            };
            if from_second {
                assert_eq!(
                    actual,
                    by_second(),
                    "pixel ({x}, {y}) is inside the SECOND texrect, so the second must have \
                     won it -- in the overlap this is the whole claim"
                );
                if from_first && by_first() != by_second() {
                    // The pixel is in the overlap AND the two texrects
                    // disagree there, so the winner is observable.
                    assert_ne!(
                        actual,
                        by_first(),
                        "pixel ({x}, {y}) is in the overlap and the two texrects write \
                         different texels there, so carrying the FIRST one's value means \
                         the earlier command won -- the exact inversion this test exists \
                         to catch"
                    );
                    observed_a_difference = true;
                }
            } else {
                assert!(from_first, "the loop only covers the two rectangles");
                assert_eq!(
                    actual,
                    by_first(),
                    "pixel ({x}, {y}) is inside the FIRST texrect and OUTSIDE the second, \
                     so the first's pixels must survive there -- a later command that \
                     overwrote the whole buffer instead of its own rectangle fails here"
                );
            }
        }
    }
    assert!(
        observed_a_difference,
        "no pixel in the overlap distinguished the two texrects, so 'the later one wins' \
         was never actually observed -- the fixture must make the two disagree somewhere"
    );
}

/// **The scale test: a frame-0-shaped packet -- tens of fills and
/// texrects in one submission -- executes rather than refusing.**
///
/// WM2000's frame 0 is 60 `G_TEXRECT` plus 60 `G_FILLRECT` with zero
/// triangles. This approximates that shape at the target size this
/// module's fixtures use: 16 fills and 16 texrects, interleaved, all
/// into one 16x8 color image. The claim is about **multiplicity**, not
/// about WM2000's own geometry -- the rectangles here are this
/// fixture's, and no pixel-level parity with a real frame is asserted.
///
/// What it proves is exactly what the two refusals this card removed
/// used to prevent: a packet with many of each executes end to end,
/// publishes one resident, and the last command's pixels are the ones
/// visible where it drew. A packet that still refused would fail at
/// `execute_raw_dpc`.
#[test]
fn a_frame_zero_shaped_packet_of_sixteen_fills_and_sixteen_texrects_executes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let words = many_fills_and_texrects_words(SCALE_COMMAND_PAIRS);

    // The positive control, again and locally: this fixture really does
    // carry the counts its name claims.
    let plan = plan_of(words.clone());
    // `SCALE_COMMAND_PAIRS` fills PLUS the leading whole-target fill
    // that establishes the buffer -- a fresh target admits nothing
    // else, so the +1 is structural, not padding.
    assert_eq!(
        plan.fills.len(),
        SCALE_COMMAND_PAIRS + 1,
        "the scale fixture must decode to {} fills",
        SCALE_COMMAND_PAIRS + 1
    );
    assert_eq!(
        plan.texrect_commands.len(),
        SCALE_COMMAND_PAIRS,
        "the scale fixture must decode to {SCALE_COMMAND_PAIRS} texrect commands"
    );

    publish_composed(&mut backend, &mut session, words);
    let resident = published_target_bytes(&backend);

    // The LAST command is a texrect at a known rectangle; its pixels
    // must be the ones visible there. Everything earlier at those
    // pixels has been overpainted, so this is the accumulation's own
    // end-to-end signature at scale.
    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();
    let last = texrect_draw_at(
        scale_texrect_x0(SCALE_COMMAND_PAIRS - 1),
        SCALE_TEXRECT_Y0,
        scale_texrect_x0(SCALE_COMMAND_PAIRS - 1) + SCALE_TEXRECT_SPAN,
        SCALE_TEXRECT_Y1,
    );
    let mut compared = 0usize;
    for y in last.top()..last.bottom() {
        for x in last.left()..last.right() {
            let index = (y * FILL_TARGET_WIDTH + x) as usize;
            let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
            assert_eq!(
                actual,
                expected_texel_halfword(committed, tile, last, x - last.left(), y - last.top()),
                "pixel ({x}, {y}) is inside the LAST of {} commands, so it must carry that \
                 command's texel -- publishing an intermediate buffer fails here",
                SCALE_COMMAND_PAIRS * 2 + 1
            );
            compared += 1;
        }
    }
    assert!(
        compared > 0,
        "the last command must cover at least one pixel, or the scale test asserts nothing"
    );
}

/// **Constraint 3, proven rather than asserted: the same wire words
/// cover a different footprint in one-cycle than in Copy.**
///
/// If these were equal, taking the extent from the wire corners would
/// be harmless and the whole "derive it from
/// `texture_rectangle_vertices`" rule would be unfalsifiable here.
#[test]
fn the_one_cycle_extent_differs_from_the_copy_extent_for_identical_wire_words() {
    let copy = composed_fixture_draw();
    let one_cycle = one_cycle_fixture_draw();
    assert_eq!(
        (copy.width(), copy.height()),
        (TEXRECT_WIDTH, TEXRECT_HEIGHT),
        "the Copy extent must be the hand-derived 8x3"
    );
    assert_eq!(
        (one_cycle.width(), one_cycle.height()),
        (ONE_CYCLE_WIDTH, ONE_CYCLE_HEIGHT),
        "the one-cycle extent must be the hand-derived 7x2"
    );
    assert_ne!(
        (copy.width(), copy.height()),
        (one_cycle.width(), one_cycle.height()),
        "identical wire words must cover DIFFERENT footprints in the two cycle types, or              the wire corners would have been a safe extent source after all"
    );
}

/// **Positive control: the one-cycle fixtures really do carry an
/// admitted `TextureRectangle`, and really do carry a combiner program
/// that is not the identity.**
///
/// The first half is the control a prior lane's mutant survived without
/// (deleting the texrect line left the content tests green). The second
/// half is this card's own addition: a fixture whose `SetCombine` was
/// silently all-zero would still admit a texrect, and a pixel test
/// against it would be checking a program nobody measured.
#[test]
fn the_one_cycle_fixtures_really_do_admit_a_combining_texture_rectangle() {
    for (label, color, alpha) in [
        ("env-lerp", ENV_LERP_COLOR, ENV_LERP_ALPHA),
        ("flat-primitive", FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
    ] {
        assert_eq!(
            admitted_texture_rectangle_triangles(fill_load_and_one_cycle_texrect_words(
                color, alpha
            )),
            2,
            "{label} must admit exactly two TextureRectangle-sourced triangles"
        );
        // The program the fixture actually stages, decoded through the
        // same accessor the executor's gate uses.
        let combine_words = one_cycle_combine_words(color, alpha);
        let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
        let selectors = [
            params.decode_color(crate::combiner::ColorInputSlot::A, true),
            params.decode_color(crate::combiner::ColorInputSlot::B, true),
            params.decode_color(crate::combiner::ColorInputSlot::C, true),
            params.decode_color(crate::combiner::ColorInputSlot::D, true),
        ];
        let expected = if label == "env-lerp" {
            [
                crate::combiner::ColorInput::Environment,
                crate::combiner::ColorInput::Texel0,
                crate::combiner::ColorInput::Primitive,
                crate::combiner::ColorInput::Texel0,
            ]
        } else {
            [
                crate::combiner::ColorInput::Zero,
                crate::combiner::ColorInput::Zero,
                crate::combiner::ColorInput::Zero,
                crate::combiner::ColorInput::Primitive,
            ]
        };
        assert_eq!(
            selectors, expected,
            "{label}'s staged SetCombine must decode to the measured program, or the pixel \
             assertions below check arithmetic nobody measured"
        );
    }
    // And the same stream WITHOUT the texrect admits none.
    let mut without = whole_target_fill_words();
    without.extend(one_load_block_words());
    without.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    assert_eq!(admitted_texture_rectangle_triangles(without), 0);
}

/// **The inversion: a texrect whose latched `SetCombine` references
/// `TEXEL0` now executes through `execute_raw_dpc` on an
/// adapter-equipped host, and its pixels are the real combined output.**
///
/// # What this replaces, and why the replacement is the record
///
/// Its predecessor,
/// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_
/// tmem_projection`, asserted the opposite -- that this exact packet was
/// blocked by name -- and was correct when written. It pinned a
/// PRE-EXISTING defect its own card could not close: `execute_raw_dpc`
/// ran two paths over one packet that read **different TMEM images**.
/// `draw_admitted_triangles` projected `coordinator.physical()`, the
/// already-**published** slot, while the CPU texel reader sampled the
/// packet's own **pending** post-image -- the only image a packet's own
/// `LoadBlock` exists in before publication. That predecessor ended with
/// an explicit instruction: "the day the projection is fixed, it fails
/// and is rewritten to assert pixels." It did fail, by its own named
/// panic, and this is that rewrite. This paragraph is the supersession
/// record.
///
/// # Why it was invisible for so long
///
/// Every prior texrect fixture latched `SetCombine(0, 0)`, whose
/// selectors reference no texel, so
/// `CombineParams::references_texels_in_first_cycle` is false, the
/// `texture_referenced` uniform is 0, and the fragment shader
/// short-circuits to `TMEM_SAMPLE_STATUS_OK` without sampling at all.
/// **The control passing was never evidence the GPU sampled correctly;
/// it was evidence it never sampled at all.** The GPU path had
/// therefore never actually fetched a texrect's texels.
///
/// # The measurement
///
/// At the untouched baseline `87b2f5b0`, the composed Copy fixture with
/// only its `set_combine(0, 0)` swapped for the env-lerp program failed
/// with `TmemSampleFailed { status: 2 }`
/// (`TMEM_SAMPLE_STATUS_INVALID_BYTE`) -- the shader read addresses the
/// published projection reported invalid. The cycle type was never the
/// variable; the texel reference was.
///
/// # What is asserted now
///
/// Both measured programs, in one loop, so the texel-referencing and
/// texel-free cases stay a controlled pair rather than two unrelated
/// tests. Each must execute, and the env-lerp arm's pixels are
/// reconciled against `expected_one_cycle_halfword` over the texel a
/// **committed** oracle reads -- a different image and a different
/// entry point than the executor used, so agreement is real evidence
/// rather than a transcription.
#[test]
fn a_texel_referencing_combine_executes_and_carries_its_combined_pixels() {
    for (label, color, alpha) in [
        ("env-lerp", ENV_LERP_COLOR, ENV_LERP_ALPHA),
        ("flat-primitive", FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
    ] {
        let combine_words = one_cycle_combine_words(color, alpha);
        let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
        let references_texel = params.references_texels_in_first_cycle();
        // **Positive control**, asserted rather than assumed: the
        // env-lerp arm must genuinely reference TEXEL0 and the
        // flat-primitive arm must genuinely not. Without this a fixture
        // that silently stopped referencing a texel would pass the whole
        // loop while proving nothing about texel sampling.
        assert_eq!(
            references_texel,
            label == "env-lerp",
            "{label}'s texel reference must match the census program it names"
        );

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        if backend.triangle_pipeline.is_none() {
            // No adapter: the triangle path cannot run at all, so
            // nothing about the projection is observable here.
            continue;
        }
        publish_composed(
            &mut backend,
            &mut session,
            fill_load_and_one_cycle_texrect_words(color, alpha),
        );

        let resident = backend
            .color_targets()
            .expect("a composed packet must have built the color-target registry")
            .residents()
            .first()
            .expect("the composed packet must have published exactly one resident")
            .device_bytes()
            .device_bytes()
            .to_vec();

        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();
        let draw = one_cycle_fixture_draw();
        let mut combined_values = std::collections::BTreeSet::new();
        let mut compared = 0usize;

        for y in 0..ONE_CYCLE_HEIGHT {
            for x in 0..ONE_CYCLE_WIDTH {
                let target_x = ONE_CYCLE_X0 + x;
                let target_y = ONE_CYCLE_Y0 + y;
                let offset = ((target_y * FILL_TARGET_WIDTH + target_x) * 2) as usize;
                let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                let request = crate::PointSampleRequest::new(
                    crate::PointSampleCoordinates::new(
                        crate::TextureCoordinateS10_5::from_raw(draw.s_at(x)),
                        crate::TextureCoordinateS10_5::from_raw(draw.t_at(y)),
                    ),
                    crate::TmemFirstRowParity::Even,
                );
                let sampled = crate::sample_committed_point(
                    committed,
                    tile.descriptor(),
                    tile.size(),
                    request,
                    crate::TextureLutMode::Disabled,
                )
                .expect("the committed oracle must sample the same texel");
                assert!(
                    sampled.snapshot().is_committed(),
                    "the ORACLE reads durable state, so its snapshot must be Committed -- if \
                     this is Proposed the oracle is not independent of the executor"
                );
                assert_eq!(
                    actual,
                    expected_one_cycle_halfword(sampled.texel().rgba8888(), color, alpha),
                    "{label}: pixel ({target_x}, {target_y}) must be the combiner's own \
                     output over the texel the committed oracle reads at this position"
                );
                assert_ne!(
                    actual,
                    expected_fill_halfword(COMPOSED_FILL_COLOR, target_x),
                    "{label}: pixel ({target_x}, {target_y}) must differ from the fill \
                     underneath, or the texrect drew nothing"
                );
                combined_values.insert(actual);
                compared += 1;
            }
        }
        assert_eq!(
            compared,
            (ONE_CYCLE_WIDTH * ONE_CYCLE_HEIGHT) as usize,
            "{label}: the loop must have compared exactly the hand-derived rectangle"
        );
        // **The claim that separates the two programs**, and the one
        // that could only be made once the projection was fixed: the
        // env-lerp output VARIES across the rectangle because it reads
        // the texel, while the flat-primitive output is constant
        // because it does not. A stale or empty projection would make
        // the env-lerp arm constant too, satisfying every assertion
        // above -- this is what catches that.
        if references_texel {
            assert!(
                combined_values.len() >= 2,
                "{label} reads TEXEL0, so its output must VARY across the rectangle -- a \
                 constant image means the projection carried empty or stale bytes rather \
                 than this packet's own load: got {combined_values:?}"
            );
        } else {
            assert_eq!(
                combined_values.len(),
                1,
                "{label} reads no texel, so its output must be constant: got \
                 {combined_values:?}"
            );
        }
    }
}

/// **The flat-primitive program, executed end to end into the published
/// image.** This is the half of WM2000's texrect work that the blocker
/// above does not touch, and it is a real one-cycle combiner
/// evaluation: 420 of the title's 2,520 texrects run exactly this
/// program.
///
/// `(Zero - Zero) * Zero + Primitive` reads no texel, so
/// `references_texels_in_first_cycle` is false, the GPU fragment shader
/// short-circuits, and the packet reaches `stage_texrect` -- where the
/// CPU executor runs `run_one_cycle` per pixel exactly as it would for
/// the env-lerp program.
///
/// The expectation is hand-derived twice and reconciled:
///
/// 1. Algebraically, the program is the primitive colour in every
///    channel, independent of the texel: `0x80FF4080` ->
///    `(128, 255, 64, 128)` -> RGBA16 `(128>>3)<<11 | (255>>3)<<6 |
///    (64>>3)<<1 | 1` = `0x87D1`. The final one is stored coverage bit
///    2, not `(128>>7)`: this whole-pixel texrect uses `CVG_DST_CLAMP`
///    with `AA_EN=FORCE_BL=IM_RD=CVG_X_ALPHA=0`, so coverage 8 stores as
///    7 (Programming Manual §§15.5.3, 15.5.6, 15.7).
/// 2. Independently, through `expected_one_cycle_halfword`, which runs
///    the real `run_one_cycle` over the real decoded `CombineParams`.
///
/// Both are asserted, and they must agree.
///
/// **The positive controls that make it non-vacuous**, each named:
/// the combined pixel must differ from the fill underneath (or nothing
/// drew), it must differ from the raw texel (or the combiner was
/// bypassed -- mutant (a)), and it must be texel-INDEPENDENT while the
/// underlying texels genuinely vary (or the program was not the one
/// staged -- mutant (e)).
#[test]
fn the_flat_primitive_one_cycle_program_composes_into_the_published_image() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    if backend.triangle_pipeline.is_none() {
        // No adapter: a triangle-bearing packet cannot execute at all.
        // Skipping is honest here and the crate's own
        // `configure_fill_target_height` already tolerates the case.
        return;
    }
    publish_composed(
        &mut backend,
        &mut session,
        fill_load_and_one_cycle_texrect_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
    );

    let resident = backend
        .color_targets()
        .expect("a composed packet must have built the color-target registry")
        .residents()
        .first()
        .expect("the composed packet must have published exactly one resident")
        .device_bytes()
        .device_bytes()
        .to_vec();

    // Derivation 1: the primitive colour, packed by hand.
    let [red, green, blue, _alpha_byte] = ONE_CYCLE_PRIM_WIRE.to_be_bytes();
    let expected_literal = (u16::from(red >> 3) << 11)
        | (u16::from(green >> 3) << 6)
        | (u16::from(blue >> 3) << 1)
        | 1;
    assert_eq!(
        expected_literal, 0x87D1,
        "the hand-packed literal must match the digit-by-digit derivation in this test's doc"
    );

    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();
    let draw = one_cycle_fixture_draw();
    let mut texels = std::collections::BTreeSet::new();
    let mut compared = 0usize;

    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
            let inside = x >= ONE_CYCLE_X0
                && x < ONE_CYCLE_X0 + ONE_CYCLE_WIDTH
                && y >= ONE_CYCLE_Y0
                && y < ONE_CYCLE_Y0 + ONE_CYCLE_HEIGHT;
            if !inside {
                assert_eq!(
                    actual,
                    expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                    "pixel ({x}, {y}) is outside the texrect, so it must still carry the \
                     fill's own value"
                );
                continue;
            }
            // Derivation 1, the literal.
            assert_eq!(
                actual, expected_literal,
                "pixel ({x}, {y}) must be the primitive colour the flat program selects"
            );
            // Derivation 2, through the real combiner over the real
            // texel the committed oracle reads.
            let request = crate::PointSampleRequest::new(
                crate::PointSampleCoordinates::new(
                    crate::TextureCoordinateS10_5::from_raw(draw.s_at(x - ONE_CYCLE_X0)),
                    crate::TextureCoordinateS10_5::from_raw(draw.t_at(y - ONE_CYCLE_Y0)),
                ),
                crate::TmemFirstRowParity::Even,
            );
            let sampled = crate::sample_committed_point(
                committed,
                tile.descriptor(),
                tile.size(),
                request,
                crate::TextureLutMode::Disabled,
            )
            .expect("the committed oracle must sample the same texel");
            assert!(
                sampled.snapshot().is_committed(),
                "the ORACLE reads durable state, so its snapshot must be Committed"
            );
            let texel = sampled.texel().rgba8888();
            assert_eq!(
                actual,
                expected_one_cycle_halfword(texel, FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
                "the two independent derivations must reconcile at pixel ({x}, {y})"
            );
            // **Mutant (a) control**: the raw texel must NOT equal the
            // combined output, or bypassing the combiner is invisible.
            let raw = (u16::from(texel[0] >> 3) << 11)
                | (u16::from(texel[1] >> 3) << 6)
                | (u16::from(texel[2] >> 3) << 1)
                | 1;
            assert_ne!(
                actual, raw,
                "pixel ({x}, {y})'s combined value must differ from the raw texel, or the \
                 combiner could have been bypassed undetectably"
            );
            texels.insert(raw);
            compared += 1;
        }
    }

    assert_eq!(
        compared,
        (ONE_CYCLE_WIDTH * ONE_CYCLE_HEIGHT) as usize,
        "the loop must have compared exactly the hand-derived 7x2 rectangle"
    );
    // **The texel-independence control.** The output is constant, which
    // is only meaningful evidence if the INPUT texels varied. If every
    // sampled texel were identical, "texel-independent" would be
    // trivially satisfied and mutant (e) -- running the env-lerp
    // program here instead -- could survive.
    assert!(
        texels.len() >= 2,
        "the sampled texels must genuinely vary across the rectangle, or the flat program's \
         texel-independence is vacuous -- got {texels:?}"
    );
    // And the texrect drew over the fill.
    let inside_offset = (((ONE_CYCLE_Y0 * FILL_TARGET_WIDTH) + ONE_CYCLE_X0) * 2) as usize;
    assert_ne!(
        u16::from_be_bytes([resident[inside_offset], resident[inside_offset + 1]]),
        expected_fill_halfword(COMPOSED_FILL_COLOR, ONE_CYCLE_X0),
        "the texrect's first pixel must differ from the fill underneath it"
    );
}

/// **The regression guard: Copy-cycle texrects still work, and Copy
/// still writes the RAW texel.**
///
/// The Copy path's full content assertions live in
/// `a_fill_a_tmem_load_and_a_texrect_compose_into_one_published_image`,
/// unchanged by this card. What this adds is the discrimination that
/// only matters once one-cycle is admitted: Copy must **not** consult
/// the combiner program, even though one is latched.
///
/// The program staged here is the flat-primitive one, chosen because it
/// references no texel and so is not blocked by the GPU-path defect
/// pinned above -- and because it would change **every** pixel had Copy
/// consulted it, which the positive control at the end asserts rather
/// than assumes. A program that happened to be the identity would make
/// this guard unable to detect Copy accidentally combining.
#[test]
fn a_copy_cycle_texrect_still_writes_the_raw_texel_with_a_program_staged() {
    let mut words = whole_target_fill_words();
    words.extend(composed_tmem_load_words());
    // Copy cycle (2), with a real combiner program latched.
    words.extend(set_other_mode(2, 0));
    words.extend(one_cycle_combine_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA));
    words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
    words.extend(set_prim_color(0x40, 0x05, ONE_CYCLE_PRIM_WIRE));
    words.extend(texrect_words_in_target_stepping(7));

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    if backend.triangle_pipeline.is_none() {
        return;
    }
    publish_composed(&mut backend, &mut session, words);

    let resident = backend
        .color_targets()
        .unwrap()
        .residents()
        .first()
        .unwrap()
        .device_bytes()
        .device_bytes()
        .to_vec();
    let committed = backend.physical_tmem();
    let tile = composed_fixture_tile();
    let draw = composed_fixture_draw();
    let mut compared = 0usize;
    let mut would_have_differed = 0usize;

    for y in TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT {
        for x in TEXRECT_X0..TEXRECT_X0 + TEXRECT_WIDTH {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
            let request = crate::PointSampleRequest::new(
                crate::PointSampleCoordinates::new(
                    crate::TextureCoordinateS10_5::from_raw(draw.s_at(x - TEXRECT_X0)),
                    crate::TextureCoordinateS10_5::from_raw(draw.t_at(y - TEXRECT_Y0)),
                ),
                crate::TmemFirstRowParity::Even,
            );
            let texel = crate::sample_committed_point(
                committed,
                tile.descriptor(),
                tile.size(),
                request,
                crate::TextureLutMode::Disabled,
            )
            .expect("the committed oracle must sample")
            .texel()
            .rgba8888();
            let raw = (u16::from(texel[0] >> 3) << 11)
                | (u16::from(texel[1] >> 3) << 6)
                | (u16::from(texel[2] >> 3) << 1)
                | u16::from(texel[3] >> 7);
            assert_eq!(
                actual, raw,
                "Copy cycle must write the RAW texel at ({x}, {y}), not a combined one, \
                 even with a SetCombine and both colour registers staged"
            );
            if expected_one_cycle_halfword(texel, FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA) != raw {
                would_have_differed += 1;
            }
            compared += 1;
        }
    }
    assert_eq!(
        compared,
        (TEXRECT_WIDTH * TEXRECT_HEIGHT) as usize,
        "the Copy rectangle is still the hand-derived 8x3"
    );
    // **The positive control.** The staged program is one that WOULD
    // have changed every pixel had Copy consulted it. Without this,
    // "Copy wrote the raw texel" would also pass against an identity
    // program and prove nothing about the gate.
    assert_eq!(
        would_have_differed, compared,
        "the staged program must be one that would have changed every pixel, or this \
         regression guard cannot detect Copy accidentally combining"
    );
}

/// **The post-merge claim: N one-cycle texrects compose in one packet,
/// each running the combiner against the accumulated buffer, in
/// journal order.**
///
/// Every pixel is attributed to exactly one writer by an ownership map
/// built in command order -- later commands overwrite earlier ones in
/// the map exactly as the accumulation loop overwrites them in the
/// buffer -- and then asserted against that writer's own hand-derived
/// value. The fill's pixels come from the RGBA16 even/odd column rule;
/// each texrect's come from its own primitive colour.
///
/// Both derivations of a texrect pixel are asserted and must agree:
/// the packed literal from `MULTI_ONE_CYCLE_PRIM`, and
/// `expected_one_cycle_halfword` running the real `run_one_cycle` over
/// the real decoded `CombineParams`.
///
/// The fill and texrect expectations deliberately differ at bit 0. Fill
/// mode bypasses the arithmetic pixel pipeline and retains the fill
/// register bit verbatim (Programming Manual Chapter 12 "Fill Mode" and
/// §12.8.2 "Fill Color"). Each one-cycle texrect instead stores its
/// `CVG_DST_CLAMP` result: coverage 8 as 7, with stored bit 2 visible in
/// RGBA16 bit 0 (Programming Manual §§15.5.3, 15.5.6, 15.7).
#[test]
fn three_one_cycle_texrects_compose_in_journal_order_against_the_accumulated_buffer() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    if backend.triangle_pipeline.is_none() {
        return;
    }
    publish_composed(&mut backend, &mut session, three_one_cycle_texrects_words());

    let resident = backend
        .color_targets()
        .expect("the packet must have built the registry")
        .residents()
        .first()
        .expect("exactly one resident")
        .device_bytes()
        .device_bytes()
        .to_vec();

    let extents: Vec<(u32, u32, u32, u32)> =
        [(0u32, 0u32, 4u32, 2u32), (3, 1, 8, 4), (10, 5, 15, 7)]
            .into_iter()
            .map(|(x0, y0, x1, y1)| one_cycle_extent_of(x0, y0, x1, y1))
            .collect();
    assert_eq!(
        extents,
        vec![(0, 0, 4, 2), (3, 1, 8, 4), (10, 5, 15, 7)],
        "the three one-cycle extents, derived through texture_rectangle_vertices and \
         cross-checked against the hand derivation (ceil(coord/4) on the raw 10.2 fields, \
         with neither Copy's |= 3 nor fill/copy's &= !3)"
    );

    // The ownership map, built in COMMAND order: a later texrect
    // overwrites an earlier one, which is the accumulation loop's own
    // rule expressed independently of it.
    let mut owner: Vec<Option<usize>> =
        vec![None; (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT) as usize];
    for (index, &(left, top, right, bottom)) in extents.iter().enumerate() {
        for y in top..bottom {
            for x in left..right {
                owner[(y * FILL_TARGET_WIDTH + x) as usize] = Some(index);
            }
        }
    }
    // The overlap really exists, or "the later texrect won" is vacuous.
    assert_eq!(
        owner[(1 * FILL_TARGET_WIDTH + 3) as usize],
        Some(1),
        "pixel (3, 1) lies in BOTH texrect 0 and texrect 1, so the map must award it to \
         the later one -- if this is Some(0) the two rectangles stopped overlapping and \
         the journal-order assertion below proves nothing"
    );

    let mut per_texrect = [0usize; 3];
    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
            match owner[(y * FILL_TARGET_WIDTH + x) as usize] {
                None => assert_eq!(
                    actual,
                    expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                    "pixel ({x}, {y}) is covered by no texrect, so it must still carry the \
                     whole-target fill's own value"
                ),
                Some(index) => {
                    let [red, green, blue, _alpha] = MULTI_ONE_CYCLE_PRIM[index].to_be_bytes();
                    let literal = (u16::from(red >> 3) << 11)
                        | (u16::from(green >> 3) << 6)
                        | (u16::from(blue >> 3) << 1)
                        | 1;
                    assert_eq!(
                        actual, literal,
                        "pixel ({x}, {y}) belongs to texrect {index}, so it must carry that \
                         texrect's OWN primitive colour -- a wrong value here means the \
                         executor read a register latched at another command's position"
                    );
                    per_texrect[index] += 1;
                }
            }
        }
    }

    // Every texrect contributed surviving pixels, so none was skipped
    // and none was wholly overwritten -- three commands really ran.
    for (index, count) in per_texrect.iter().enumerate() {
        assert!(
            *count > 0,
            "texrect {index} must own at least one surviving pixel, or the packet did not \
             execute all three: {per_texrect:?}"
        );
    }
    // The three packed colours are pairwise distinct, so attributing a
    // pixel to a texrect is a real discrimination.
    let packed: std::collections::BTreeSet<u16> = MULTI_ONE_CYCLE_PRIM
        .iter()
        .map(|wire| {
            let [r, g, b, _a] = wire.to_be_bytes();
            (u16::from(r >> 3) << 11) | (u16::from(g >> 3) << 6) | (u16::from(b >> 3) << 1) | 1
        })
        .collect();
    assert_eq!(
        packed.len(),
        3,
        "the three primitive colours must pack to three distinct RGBA16 values, or a pixel \
         cannot be attributed to the texrect that wrote it"
    );

    // **Derivation 2**, through the real combiner rather than the
    // packed literal: `expected_one_cycle_halfword` runs `run_one_cycle`
    // over the real decoded `CombineParams` for each texrect's own
    // registers. It must agree with the literal at texrect 1's own
    // first owned pixel.
    let probe = expected_one_cycle_halfword_with_prim(
        [0x18, 0x40, 0xC8, 0xFF],
        FLAT_PRIM_COLOR,
        FLAT_PRIM_ALPHA,
        MULTI_ONE_CYCLE_PRIM[1],
    );
    let [r1, g1, b1, _a1] = MULTI_ONE_CYCLE_PRIM[1].to_be_bytes();
    assert_eq!(
        probe,
        (u16::from(r1 >> 3) << 11) | (u16::from(g1 >> 3) << 6) | (u16::from(b1 >> 3) << 1) | 1,
        "the real combiner over texrect 1's own primitive register must reconcile with the \
         packed literal this test asserted against the published buffer"
    );
}

/// **A texrect that declared no write stays on the triangle path.**
///
/// `stage_and_report` routes a load-free packet to the color-target
/// accumulation seam, and that seam needs a `ColorTargetKey`. A texrect
/// with no staged `SetColorImage` declares no `ColorFramebuffer` access
/// at all -- `raw_dpc::plan_texture_rectangle` returns early -- so
/// there is no key to build and no target to compose into. It belongs
/// on the GPU triangle path, exactly where it went before this file
/// learned about texrects.
///
/// Routing on the mere *presence* of a texrect sent it to the seam
/// instead and refused it with `NoStagedColorImage`: measured, that
/// broke both `..._texture_rectangle_at_its_own_wire_position` fixtures.
/// This pins the same rule without needing a host GPU, so the guard
/// survives on an adapterless machine.
#[test]
fn a_texrect_that_declared_no_write_is_not_routed_to_the_color_target_seam() {
    // No `SetColorImage` anywhere in the stream, so the decoder
    // declares no RenderTarget write for the texrect.
    let mut words = Vec::new();
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(set_tile(7, 1, 0));
    words.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
    words.extend(texrect_words_in_target(7));

    // Positive control, both halves. The stream must really carry a
    // texrect (a stream with none would also pass the assertion below,
    // vacuously), and that texrect must really declare no write.
    //
    // Measured against the SAME stream with a `SetColorImage` spliced
    // in: that variant declares writes, this one declares none, and the
    // only difference between them is the register. So the emptiness
    // here is the decoder's early return on a missing color image, not
    // a fixture that failed to carry a texrect at all.
    let mut with_image = whole_target_fill_words();
    with_image.extend(words.iter().copied());
    assert!(
        !declared_render_target_ranges(with_image).is_empty(),
        "the same texrect must declare writes once a color image is staged, or this \
         fixture carries no texrect and the test is vacuous"
    );
    assert!(
        declared_render_target_ranges(words.clone()).is_empty(),
        "the fixture's texrect must declare NO write, or it is not the shape under test"
    );

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, result) = plan_and_execute_fill(&mut backend, &mut session, words);

    // On an adapterless host the triangle path refuses with
    // `TriangleDrawBeforeCreate`; with an adapter it draws. Either way
    // the packet must NOT have been routed to the color-target seam,
    // which is what `NoStagedColorImage` would prove.
    if let Some(error) = result.err() {
        assert!(
            !error.to_string().contains("no SetColorImage current"),
            "a texrect declaring no write must stay on the triangle path, never reach \
             the color-target key derivation: {error}"
        );
    }
}

/// **The identity crossing refuses in BOTH directions, by name.**
///
/// `verify_tmem_identity` is the one site where a texrect's TMEM image
/// is checked against the identity its caller selected. The pending
/// direction has been checked since commit `99bde6a3`; the committed
/// direction is new with the load-free texrect admission, and without a
/// test it would be decorative -- measured, deleting it left the entire
/// suite green.
///
/// Both real impls are correct, so no real image can reach either arm.
/// Sources that lie are the only way to prove either refusal is wired,
/// and the honest pair below is what makes the lying pair mean
/// something: the check must discriminate on the identity, not refuse
/// everything.
#[test]
fn the_tmem_identity_crossing_refuses_a_forgery_in_either_direction() {
    // Honest durable state -- a real `PhysicalTmemState`, exactly the
    // source `TexrectTmemSource::Committed` hands a load-free packet.
    let committed = PhysicalTmemState::try_new().unwrap();

    // The committed arm accepts it, so the check discriminates on the
    // identity rather than refusing everything.
    verify_tmem_identity(&committed, false, 0)
        .expect("durable state must pass the arm that selected it");

    // The pending arm refuses that same honest committed image. This is
    // the defect a load-bearing packet would suffer: its texrect
    // silently missing its own packet's loads, which commit `3a1a6a73`
    // measured as `TMEM_SAMPLE_STATUS_INVALID_BYTE`.
    let error = verify_tmem_identity(&committed, true, 5)
        .expect_err("durable state must not satisfy the pending arm");
    assert!(
        matches!(
            error,
            WgpuRawDpcExecutionError::PendingTmemImageClaimedCommitted { triangle_index: 5 }
        ),
        "the refusal must be the named variant carrying its own triangle index: {error:?}"
    );

    // The mirror direction, which the load-free admission introduced.
    // The source lies about its identity while returning durable bytes
    // -- precisely the forgery shape, and the only way to reach the arm
    // at all, since both real impls are correct.
    //
    // The `Proposed` identity is a REAL one, produced by
    // `tmem::read`'s own test constructor rather than synthesized here,
    // so this test cannot pass against a variant no real image could
    // produce.
    struct ForgedProposed<'a>(&'a PhysicalTmemState);
    impl crate::TmemByteSource for ForgedProposed<'_> {
        fn snapshot(&self) -> crate::TmemSnapshotIdentity {
            crate::tmem::proposed_identity_for_test()
        }
        fn valid_byte(&self, address: u16) -> Option<u8> {
            crate::TmemByteSource::valid_byte(self.0, address)
        }
    }
    assert!(
        !crate::tmem::proposed_identity_for_test().is_committed(),
        "the identity borrowed for the forgery must really be Proposed, or the refusal \
         below fires for the wrong reason"
    );

    let forged = ForgedProposed(&committed);
    let error = verify_tmem_identity(&forged, false, 3)
        .expect_err("a proposal must not satisfy the committed arm");
    assert!(
        matches!(
            error,
            WgpuRawDpcExecutionError::CommittedTmemImageClaimedProposed { triangle_index: 3 }
        ),
        "the refusal must be the named variant carrying its own triangle index: {error:?}"
    );
    // And the pending arm accepts that same identity, so this direction
    // discriminates on the identity too.
    verify_tmem_identity(&forged, true, 0)
        .expect("a Proposed identity must pass the arm that selected it");
}

/// **The GPU projection refuses a pending image that claims to be
/// committed, by name.**
///
/// The sibling of `execute_scheduled_texrect`'s
/// `PendingTmemImageClaimedCommitted` check, at the other place a
/// pending post-image is consumed. Both exist because the type system
/// cannot enforce this: `Committed` and `Proposed` inhabit one enum, so
/// a wrong `snapshot()` impl compiles and passes.
///
/// Measured, which is why this test exists: deleting the refusal left
/// the env-lerp pixel test, the projection unit tests and the
/// guest-RDRAM end-to-end test all green. No real `PendingTmemImage`
/// can reach the arm -- its own impl is correct -- so a source that lies
/// is the only way to prove the refusal is wired rather than decorative.
#[test]
fn the_gpu_projection_refuses_a_pending_image_claiming_to_be_committed() {
    /// A byte source with a pending image's bytes and a durable
    /// image's *claim* -- the forgery the split exists to catch.
    struct ForgedCommitted;
    impl crate::TmemByteSource for ForgedCommitted {
        fn snapshot(&self) -> crate::TmemSnapshotIdentity {
            let state = PhysicalTmemState::try_new().unwrap();
            crate::TmemByteSource::snapshot(&state)
        }
        fn valid_byte(&self, address: u16) -> Option<u8> {
            Some(address as u8)
        }
    }

    let error = project_proposed_image(&ForgedCommitted)
        .expect_err("a source claiming Committed must be refused, not projected");
    assert!(
        matches!(
            error,
            WgpuRawDpcExecutionError::PendingTmemProjectionClaimedCommitted
        ),
        "the refusal must be the named variant, not some other error: {error:?}"
    );
    assert!(
        error.to_string().contains("Committed snapshot identity"),
        "the refusal must name what went wrong: {error}"
    );

    // The contrast that makes the claim mean something: the SAME bytes
    // behind an honest `Proposed` identity project successfully, so the
    // refusal discriminates on the identity, not on the content. The
    // honest identity comes from a real sealed transaction driven
    // through the composed execution path -- the only route to one in
    // this file, since `PendingTmemTransaction` is move-only.
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, result) = plan_and_execute_composed(
        &mut backend,
        &mut session,
        fill_load_and_copy_texrect_words(),
    );
    assert!(
        result.is_ok(),
        "a composed packet must execute, which requires its own pending post-image to have \
         projected successfully through this same function: {result:?}"
    );
}

/// **A later packet's triangles never sample an earlier packet's pending
/// projection.**
///
/// The pending post-image belongs to the packet that sealed it. A second
/// `execute_raw_dpc` carrying triangles but no TMEM load of its own must
/// project the *published* slot -- which by then does contain the first
/// packet's load, because publication ran between them -- and must never
/// reuse a retained projection from the earlier call.
///
/// This is the cross-packet half of the same invariant per-load prefix
/// selection enforces within a packet (`prefix_before`): a draw may only
/// observe TMEM already established at its own position in the stream,
/// never state from a different transaction.
///
/// Measured: caching the projection on the backend and reusing it when a
/// later packet supplied none passed the env-lerp pixel test, the
/// projection unit tests and the guest-RDRAM end-to-end test. Only this
/// test kills that mutant, which is why it asserts on the retained
/// *state* rather than on pixels -- the leaked and correct projections
/// happen to agree on content here, so a pixel comparison cannot
/// separate them, but the leak is still a real cross-transaction read.
#[test]
fn a_later_packet_does_not_reuse_an_earlier_packets_pending_projection() {
    let source = include_str!("../../production/state.rs");
    let struct_start = source
        .find("pub struct WgpuBackend {")
        .expect("WgpuBackend must exist in this file");
    let struct_end = source[struct_start..]
        .find("\n}\n")
        .expect("WgpuBackend must have a closing brace")
        + struct_start;
    let fields = &source[struct_start..struct_end];
    assert!(
        !fields.contains("TmemGpuProjection"),
        "WgpuBackend must hold no TmemGpuProjection field -- a retained projection is a \
         pending post-image outliving the packet that sealed it, which is exactly the \
         cross-transaction read the committed/pending split exists to prevent. Fields: \
         {fields}"
    );
}

/// **The committed/pending distinction, tested rather than assumed: a
/// pending post-image read reports `Proposed`, a durable read reports
/// `Committed`, and the two carry different identity types.**
///
/// This is what the whole `TmemSnapshotIdentity` split exists for. A
/// pending transaction has no durable `(state, generation)` pair --
/// `binding.state` is the BASE state's identity and
/// `binding.next_generation` names a generation that will not exist if
/// publication is rejected -- so answering a pending read with a
/// `PhysicalTmemSnapshotIdentity` would mint a receipt for a snapshot
/// nothing ever published, indistinguishable downstream from a real one.
///
/// Measured, not assumed: forging `Committed` inside
/// `PendingTmemImage`'s own `snapshot()` impl passed the entire
/// 5021-test suite before this test and `stage_texrect`'s matching
/// runtime check existed (mutant K in this card's report). Both landed
/// for that reason, and the runtime check is a check rather than a type
/// guarantee because both variants inhabit one enum: a wrong impl
/// compiles.
///
/// The pending image is reached through the composed execution path,
/// which is the only route to a real sealed transaction in this file --
/// the type is move-only and `submitted_packet` is the one callback
/// where the `WorkloadPacket` it needs is in scope.
#[test]
fn a_pending_tmem_read_reports_a_proposal_and_a_committed_read_reports_a_snapshot() {
    // The pending side, observed from inside execution: `stage_texrect`
    // asserts `!snapshot.is_committed()` on the live post-image and
    // refuses `PendingTmemImageClaimedCommitted` otherwise, so a
    // successful composed execution IS the pending-side assertion.
    // Running it here rather than only relying on the composed test
    // keeps the claim attached to the property being made.
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, result) = plan_and_execute_composed(
        &mut backend,
        &mut session,
        fill_load_and_copy_texrect_words(),
    );
    assert!(
        result.is_ok(),
        "the composed packet must execute, which requires its pending post-image to have \
         reported a Proposed snapshot: {result:?}"
    );

    // The durable side, for the contrast that makes the claim mean
    // something: the SAME reader over durable state reports Committed,
    // with the state's own real identity and generation.
    let committed = backend.physical_tmem();
    let durable = crate::TmemByteSource::snapshot(committed);
    assert!(
        durable.is_committed(),
        "a read of durable PhysicalTmemState must report Committed, got {durable:?}"
    );
    let snapshot = durable
        .committed()
        .expect("a Committed identity must expose its snapshot");
    assert_eq!(snapshot.state(), committed.identity());
    assert_eq!(snapshot.generation(), committed.generation());
    assert!(
        durable.proposed().is_none(),
        "a Committed identity must not also present itself as a proposal"
    );

    // A fresh durable state reports a DIFFERENT identity, so the
    // assertion above is pinning a real value rather than a constant.
    let other = PhysicalTmemState::try_new().unwrap();
    assert_ne!(
        crate::TmemByteSource::snapshot(&other)
            .committed()
            .expect("durable")
            .state(),
        snapshot.state(),
        "two distinct durable states must report distinct identities"
    );
}

/// **Invariant 2, proven: ordering within a packet is semantics, and
/// the reverse order observably differs.**
///
/// Forward (`LoadBlock` then texrect) and reversed (texrect then
/// `LoadBlock`) both execute -- both are legal RDP streams -- and they
/// produce **different** texrect pixels. Same commands, same wire
/// words, same fill; only the order changed.
///
/// # This test found a real defect, and records it
///
/// The first draft asserted only that the reversed order "must not
/// execute", and it **failed**: the reversed stream executed cleanly
/// and produced texrect rows with byte-identical `CompletedWrite`
/// content digests to the forward stream's. The cause was structural,
/// not a slip: `stage_and_report` sealed ONE pending post-image from
/// every load before any texrect executed, so a texrect's position in
/// the command stream had no effect on what it saw. Ordering was not
/// semantics; it was ignored.
///
/// A `TexrectBeforeItsOwnLoad` refusal named that honestly while there
/// was no per-position image to serve. Per-load prefix views replaced
/// it: the texrect in the reversed stream now samples what TMEM held at
/// its own position -- durable committed state, because no load in its
/// packet precedes it -- and the load that follows it still stages, so
/// the stream executes and the two orders differ in output.
///
/// The identical-digest observation is what keeps this load bearing
/// rather than defensive: without a position-aware image the two orders
/// are genuinely indistinguishable in their output, which is precisely
/// the invariant violation. Asserting a *difference* is strictly
/// stronger than asserting a refusal -- a refusal proves only that one
/// order was rejected, while this proves the accepted orders disagree.
#[test]
fn a_texrect_before_its_load_observably_differs_from_one_after_it() {
    // Forward order: fill, load, texrect. Executes.
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let forward_words = fill_load_and_copy_texrect_words();
    let (_, forward) =
        plan_and_execute_composed(&mut backend, &mut session, forward_words.clone());
    assert!(
        forward.is_ok(),
        "the forward order (load, then texrect) must execute: {forward:?}"
    );

    // Reversed: the texrect comes BEFORE the load. Same commands, same
    // wire words, only the order changed.
    let mut reversed = whole_target_fill_words();
    reversed.extend(set_texture_image(0, 2, 8, 0x200));
    reversed.extend(set_tile(7, 2, 0));
    reversed.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    reversed.extend(set_other_mode(2, 0));
    reversed.extend(set_combine(0, 0));
    reversed.extend(texrect_words_in_target_stepping(7));
    reversed.extend(load_sync());
    reversed.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);

    // Control: the reversed stream really does still carry both an
    // admitted texrect and an admitted load. Without this, a difference
    // below could mean the reordering broke the decode instead of
    // moving the texrect to a position with different texels.
    assert_eq!(
        admitted_texture_rectangle_triangles(reversed.clone()),
        2,
        "the reversed stream must still admit its texture rectangle, or the difference below \
         proves nothing about ordering"
    );

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, backward) = plan_and_execute_composed(&mut backend, &mut session, reversed.clone());
    let error = backward
        .expect_err(
            "the reversed texrect samples TMEM at its own position, where this packet's load \
             has not run and nothing was ever published -- it must not produce texels",
        )
        .to_string();
    assert!(
        error.contains("physical TMEM texel byte") && error.contains("is invalid"),
        "the reversed order must be refused by the TEXEL READER finding nothing valid at \
         this stream position -- not by a shape gate, and never by inventing a zero texel. \
         Got: {error}"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a refused reversed packet must leave no redeemable fill token"
    );

    // **The observable difference, and the mutation this test kills.**
    //
    // The refusal above alone would still pass under a once-per-packet
    // post-image if that image happened to be invalid too, so the
    // strip below carries the discriminating half: seven loads writing
    // the same TMEM range, seven texrects, and the requirement that
    // consecutive sprites DIFFER. Measured: with `prefix_before`
    // mutated to `prefixes.last()` -- which is exactly re-sealing once
    // per packet -- the forward and reversed publications produce
    // byte-identical `CompletedWrite` digest lists here, the same
    // indistinguishability the original defect had.
    let forward_digests: Vec<_> = {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_composed(&mut backend, &mut session, forward_words)
            .iter()
            .map(|write| write.content())
            .collect()
    };
    let strip = publish_sprite_strip(SPRITE_STRIP_PAIRS);
    let first_sprite = sprite_strip_pixels(&strip, 0);
    let last_sprite = sprite_strip_pixels(&strip, SPRITE_STRIP_PAIRS - 1);
    assert_ne!(
        first_sprite, last_sprite,
        "the first and last texrect of a strip whose loads all overwrite one TMEM range must \
         carry DIFFERENT texels; equal ones mean every texrect observed the last load, which \
         is the ordering violation this test exists to catch"
    );
    assert!(
        !forward_digests.is_empty(),
        "the forward order must publish writes, or its execution above proved nothing"
    );
}

/// **The second inversion of this test, and the disproof that drove
/// it.**
///
/// At `be6ed65c` this was
/// `a_fill_composed_with_a_texture_rectangle_is_refused_by_name`,
/// asserting `MixedFillAndTrianglePacket` on the reasoning that "a
/// texrect *is* two triangles by the time `stage_and_report` sees it".
/// That premise fell: a texrect declares its own journal
/// `ColorFramebuffer` writes where a raw triangle declares none.
///
/// It then asserted `TexrectWithoutTmemLoad`, justified by a census
/// reading "0 of WM2000's 219 decode entries carry a texrect without a
/// load in the same entry". **That premise has now fallen too, and the
/// count is not what was wrong with it.** The census window was 219
/// decode entries of boot/attract and its own doc supersedes it twice
/// (383 -> 1,056 -> 4,454 VI fields, 219 -> 2,219 -> 5,792 entries).
/// Re-measured on the real ROM through the shell's `FN64_RENDER=wgpu`
/// seam, the fourth packet WM2000 issues is 46 texrects, 0 loads and 0
/// fills -- the shape the refusal declared impossible, from the game.
///
/// So this test now pins the **admission**: a texrect in a load-free
/// packet samples durable committed TMEM, which is not a stale
/// substitute for a proposal but the published result of every earlier
/// packet's loads -- the only thing hardware TMEM could hold at this
/// stream position. What kept the old refusal honest is kept by other
/// means, and they are asserted here too: the read goes through the
/// same `sample_point` path a pending read uses, so an invalid TMEM
/// byte is still a named refusal rather than a fabricated texel.
///
/// The fill+**raw triangle** refusal is unchanged and still named
/// `MixedFillAndTrianglePacket`; it is asserted separately below.
#[test]
fn a_fill_composed_with_a_texture_rectangle_and_no_tmem_load_samples_committed_tmem() {
    let mut fill_and_texrect = whole_target_fill_words();
    fill_and_texrect.extend(set_other_mode(2, 0));
    fill_and_texrect.extend(set_combine(0, 0));
    // A bound tile, so the texrect reaches the SAMPLER rather than
    // stopping at `TexrectUnboundTile` one step earlier. Without this
    // the test would pass on a refusal that says nothing about what a
    // load-free texrect reads, and the invalid-byte assertion below
    // would be vacuous.
    fill_and_texrect.extend(set_tile(7, 1, 0));
    fill_and_texrect.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
    fill_and_texrect.extend(texrect_words_in_target(7));

    // Positive control: this stream really does carry the texrect.
    // Measured through the journal rather than the plan walk, because
    // `admitted_texture_rectangle_triangles` needs a TMEM read to
    // capture and this fixture deliberately has no TMEM load at all.
    // Two `RenderTarget` ranges beyond the fill's own single
    // whole-target one means the texrect declared its own rows; the
    // extent is the one-cycle 7x2 derivation (this fixture's
    // `set_other_mode(2, 0)` is Copy, so 8x3 -- three rows).
    let ranges = declared_render_target_ranges(fill_and_texrect.clone());
    assert_eq!(
        ranges.len(),
        1 + TEXRECT_HEIGHT as usize,
        "the fixture must declare the fill's range plus the texrect's {TEXRECT_HEIGHT}              rows, or the refusal below is vacuous -- got {ranges:?}"
    );

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, result) = plan_and_execute_fill(&mut backend, &mut session, fill_and_texrect);

    // **No refusal, and specifically not the deleted one.** Nothing in
    // this backend may name `TexrectWithoutTmemLoad` any more: the
    // variant is gone, so a reintroduction is a compile error rather
    // than a string this assertion has to chase.
    //
    // The packet's TMEM is entirely unwritten here -- this fixture
    // stages no load in this packet and publishes none before it -- so
    // every texel the texrect asks for is an INVALID byte. That is the
    // load-bearing half of the assertion: admitting the shape must not
    // mean fabricating texels for it. The reader refuses by name, from
    // the same `sample_point` path a pending read uses, and this test
    // pins that the refusal is about the *bytes*, not about the packet
    // shape.
    let error = result.expect_err(
        "an unwritten TMEM must still refuse the texel fetch by name, or this admission \
         would be producing plausible pixels from nothing",
    );
    let message = error.to_string();
    assert!(
        !message.contains("completed no TMEM load"),
        "the packet SHAPE must no longer be the refusal -- got: {message}"
    );
    assert!(
        message.contains("invalid") || message.contains("Invalid"),
        "the refusal must name the invalid TMEM byte the sampler actually hit, got: {message}"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a refused composition must leave no redeemable fill token behind"
    );
}

/// **Durable cross-packet carry-in for `SetColorImage`, measured as the
/// defect it closes.**
///
/// The RDP's color-image register survives a submission boundary, so a
/// packet may compose into a target it never re-declares -- and WM2000
/// does exactly that: its texrect packet carries 14 texrects, 4 loads
/// and zero fills, every texrect declaring a real write run derived by
/// the decoder from the *previous* packet's `SetColorImage`.
///
/// `color_target_key` used to read the image off `plan.fills.first()`,
/// so that packet aborted the run. This pins the fix at the seam: the
/// second packet declares no color image of its own and must still
/// resolve one.
///
/// The positive control is the first packet's own success -- if it did
/// not establish a target, the second packet's admission would prove
/// nothing about carry-in.
#[test]
fn a_second_packet_composes_into_the_color_image_the_first_one_declared() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Packet one: declares the color image and fills the whole target.
    let (_, first) =
        plan_and_execute_fill(&mut backend, &mut session, whole_target_fill_words());
    first.expect("the declaring packet must execute, or carry-in is untested");
    assert!(
        backend.rdp_state.color_image().is_some(),
        "the first packet must leave a durable color image behind, or this test is vacuous"
    );

    // Packet two: a **texrect and no fill at all**, and no
    // `SetColorImage` of its own. The absence of a fill is the whole
    // point: with the key derived from `plan.fills.first()` there is
    // nothing to derive it from, which is exactly the shape WM2000
    // aborted on. A second fill would leave the old derivation working
    // and this test asserting nothing.
    let (_, second) =
        plan_and_execute_fill(&mut backend, &mut session, second_words_for_control());

    // **The positive control IS the refusal, and it names the derived
    // key.** This packet still fails -- the first packet's fill was
    // staged but never published, so the resident bytes a texrect must
    // compose over do not exist yet. What matters is *which* refusal:
    // `MissingResidentBytes` is raised by `execute_scheduled_texrect`,
    // strictly downstream of `color_target_key`, and it prints the key
    // that was derived. So a key genuinely was built for a packet
    // carrying no fill to build one from.
    //
    // Asserting the address makes it non-vacuous: it is the *first*
    // packet's `SetColorImage` address, carried across the submission
    // boundary. Excluding the "no SetColorImage" message alone would
    // also pass if some earlier gate refused first.
    let error = second.expect_err("the unpublished target still refuses for resident bytes");
    let message = error.to_string();
    assert!(
        !message.contains("no SetColorImage current"),
        "the second packet must resolve the durable register, not refuse for its \
         absence: {message}"
    );
    let carried = backend
        .rdp_state
        .color_image()
        .expect("checked above")
        .address()
        .get();
    assert!(
        message.contains("requires resident_bytes")
            && message.contains(&format!("address: {carried}")),
        "the refusal must be the downstream resident-bytes one, naming a key at the \
         first packet's own color-image address {carried} -- that key is the proof the \
         durable register was read. Got: {message}"
    );
}

/// Packet A establishes `(Zero - Zero) * Zero + Primitive` with a white
/// primitive. Packet B draws a CI4 texrect, changes to black primitive and
/// Texel0 passthrough, then draws again. State must not travel backwards.
///
/// For texel `[32, 64, 128, 255]`, expected channels come directly from
/// `(A-B)*C+D`: first `(0-0)*0+1`, then `(0-0)*0+Texel0`. No fn64 output
/// is captured as the oracle.
#[test]
fn a_raw_dpc_packet_does_not_apply_later_combiner_state_retroactively() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let flat_primitive = one_cycle_combine_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA);
    let texel0_passthrough = one_cycle_combine_words([8, 8, 16, 1], [7, 7, 7, 1]);

    let mut first = Vec::new();
    first.extend(set_other_mode(0, 0));
    // SetTile: format=CI (2), size=4-bit (0), line=1, tile=7.
    first.extend([word(SET_TILE, 2 << 21 | 1 << 9), 7 << 24]);
    first.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    first.extend(flat_primitive);
    first.extend(set_prim_color(0, 0, 0xffff_ffff));
    backend
        .plan_raw_dpc(session.plan_request(capture(first)))
        .expect("packet A establishes the carry-in state");

    let mut second = Vec::new();
    second.extend(texrect_words_at(7, 0, 0, 3, 1));
    second.extend(set_prim_color(0, 0, 0x0000_00ff));
    second.extend(texel0_passthrough);
    second.extend(texrect_words_at(7, 4, 0, 7, 1));
    let planned = backend
        .plan_raw_dpc(session.plan_request(capture(second)))
        .expect("packet B plans cleanly");
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    // The production execute_raw_dpc seeding shape: one complete
    // pre-delta snapshot, never per-register reads from mixed times.
    let mut plan_visitor = PlanCollector::seeded(
        backend
            .raw_dpc_carry_in_before_last_plan
            .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
    );
    let mut color_targets = None;
    let mut view = ExecutionCollector {
        plan: PlanCollector::seeded(RawDpcCarryIn {
            draw: RdpDrawState {
                other_mode: None,
                combine: None,
                blend_color: Color4::default(),
                env_color: Color4::default(),
                prim_color: PrimColor::default(),
                fog_color: Color4::default(),
                scissor: None,
                color_image: None,
                tiles: [(None, None); 8],
                prim_depth: None,
            },
        }),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        physical: backend.coordinator.physical(),
        color_targets: &mut color_targets,
        configured_target_extent: backend.configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    backend
        .coordinator
        .execution_view(&bound, &mut plan_visitor, &mut view);

    assert_eq!(
        view.plan.triangles.len(),
        4,
        "two texrects produce two triangles each"
    );
    let draws: Vec<_> = view
        .plan
        .triangles
        .iter()
        .map(|planned| planned.draw.as_ref().expect("each texrect retrieves draw state"))
        .collect();
    let texel = [32.0 / 255.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
    for (index, draw) in draws.iter().enumerate() {
        let inputs = crate::combiner::combiner_inputs_from_fragment_registers(
            crate::combiner::CombinerInputs {
                tex_val0: texel,
                tex_val1: [0.0; 4],
                prim_color: [0.0; 4],
                shade_color: [0.0; 4],
                env_color: [0.0; 4],
                key_center: [0.0; 3],
                key_scale: [0.0; 3],
                lod_fraction: 0.0,
                prim_lod_frac: 0.0,
                noise: 0.0,
                k4: 0.0,
                k5: 0.0,
            },
            draw.env_color,
            draw.prim_color,
        );
        let (actual, _) = crate::combiner::run_one_cycle(draw.combine_params, inputs);
        let expected = if index < 2 { [1.0; 4] } else { texel };
        for channel in 0..4 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1.0e-6,
                "draw {index} channel {channel}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            draw.combine_params.references_texels_in_first_cycle(),
            index >= 2,
            "only the second rectangle may read Texel0"
        );
    }
}

/// **The physical -> buffer-relative subtraction in
/// `committed_guest_render_target_bytes` refuses a write below its own
/// target's base instead of wrapping.**
///
/// The arithmetic is `range.start().get() - base` on two `u32`s. Before
/// this test it was a bare subtraction: a staged write starting below
/// `base` wrapped to a near-`u32::MAX` offset, which then sliced far
/// past the buffer. In release that is a silent wrong read, and this
/// method's own doc forbids exactly that ("never copy some other
/// submission's pixels into guest memory").
///
/// `fill_completed_writes` rejects such a write when it builds the list,
/// so the guard is unreachable through the public packet path -- which
/// is why the malformed write is staged directly onto the `pub(super)`
/// staging field here. That is the only way to reach the boundary, and
/// leaving it unproven is what let the wrap survive.
///
/// Mutation-checked: restoring the bare `-` makes this test fail (the
/// wrapped offset slices out of bounds and trips the buffer `expect`
/// with a different message than the base guard's).
#[test]
#[should_panic(expected = "starts at or after its own color target's base")]
fn a_staged_write_below_the_target_base_is_refused_not_wrapped() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Execute but deliberately do NOT publish: `publish_raw_dpc` takes the
    // pending fill publication, and this test must reach into it while it
    // is still staged. This is exactly `publish_composed`'s own prefix.
    let (planned, source_bytes) = plan_with_deterministic_reads(
        &mut backend,
        &mut session,
        three_fills_and_three_texrects_words(),
    );
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();
    backend
        .execute_raw_dpc(bound)
        .expect("a composed fill+TMEM packet must execute");

    // Rewrite one staged guest write's range to start below the target
    // base -- the exact shape `fill_completed_writes` would have rejected.
    let pending = backend
        .pending_fill_publication
        .as_mut()
        .expect("executing the composed packet must leave a pending fill publication");
    let base = pending.color.full().key().address().get();
    assert!(base >= 2, "the fixture target must not sit at address 0");
    let victim = pending.guest_writes[0].access();
    let fn64_render_ir::ResourceRegion::Rdram { resource, range } = victim.region() else {
        panic!("a staged guest render-target write names an RDRAM region");
    };
    let below = fn64_render_ir::ResourceAccess::try_new(
        victim.operation(),
        victim.mode(),
        victim.purpose(),
        fn64_render_ir::ResourceRegion::Rdram {
            resource,
            range: range.layout().range(base - 2, base).unwrap(),
        },
    )
    .unwrap();
    pending.guest_writes[0] = CompletedWrite::try_from_bytes(below, &[0u8; 2]).unwrap();
    let submission = pending.submission;

    let _ = backend.committed_guest_render_target_bytes(submission);
}
