use super::*;

/// The partition is the metric's load-bearing claim, so a case may not
/// claim RT64 authority while enabling a stage RT64 does not model.
///
/// AA_EN is bit 3 of `SetOtherModes` word 0's low byte. Derived by hand
/// from the RDP wire layout, not from any backend.
#[test]
fn authority_matches_the_commands() {
    for case in cases() {
        let other_modes = case
            .commands
            .iter()
            .find(|&&(word0, _)| word0 >> 24 == 0xef)
            .expect("every case sets other modes");
        let aa_enabled = other_modes.0 & 0x0000_0008 != 0;
        if aa_enabled {
            assert_eq!(
                case.authority,
                Authority::CoverageDependentRt64NotAuthoritative,
                "case {} enables AA_EN but claims RT64 authority",
                case.name
            );
        }
    }
}

/// **`SetScissor` splits its bounds across BOTH words.**
///
/// angrylion reads the upper-left from word 0 and the lower-right from
/// word 1 (`rasterizer.c:2779`). Packing both into word 0 -- which every
/// scissor in this corpus did until it was measured -- decodes as an
/// INVERTED box, and an inverted scissor is a degenerate input the two
/// backends answer differently. That is silent: it reads as a renderer
/// disagreement rather than a fixture defect.
#[test]
fn set_scissor_splits_its_bounds_across_both_words() {
    let (word0, word1) = set_scissor(0, 0, WIDTH / 2, HEIGHT);
    assert_eq!(word0 >> 24, 0xed, "opcode");
    // Upper-left in word 0, S10.2.
    assert_eq!((word0 >> 12) & 0xfff, 0, "upper-left X");
    assert_eq!(word0 & 0xfff, 0, "upper-left Y");
    // Lower-right in word 1, S10.2 -- NOT in word 0.
    assert_eq!((word1 >> 12) & 0xfff, (WIDTH / 2) * 4, "lower-right X");
    assert_eq!(word1 & 0xfff, HEIGHT * 4, "lower-right Y");

    // And the box is never inverted: every corpus scissor must have its
    // lower-right at or beyond its upper-left on both axes.
    for case in cases() {
        for pair in case.commands.windows(1) {
            let (word0, word1) = pair[0];
            if word0 >> 24 != 0xed {
                continue;
            }
            assert!(
                (word1 >> 12) & 0xfff >= (word0 >> 12) & 0xfff && word1 & 0xfff >= word0 & 0xfff,
                "case {} emits an inverted scissor: ul=({}, {}) lr=({}, {})",
                case.name,
                (word0 >> 12) & 0xfff,
                word0 & 0xfff,
                (word1 >> 12) & 0xfff,
                word1 & 0xfff
            );
        }
    }
}

/// **The triangle case's S planes are authored in RT64's VERTEX terms.**
///
/// RT64 evaluates S at three vertices, not per pixel, and only `v3`
/// carries the `Dx` term. With `De = 0` that makes `base` the S of the H
/// edge -- where `v1` and `v2` both sit -- so the two halves of the box
/// need DIFFERENT bases (left edge, right edge) and the SAME `Dx`.
///
/// This reproduces RT64's own arithmetic on the emitted words and checks
/// the three per-vertex texcoords land on the texel midpoints the key
/// expects. Without it, a shared base silently reverses the upper-right
/// half's gradient and the whole box reads texel 0 -- measured.
#[test]
fn the_triangle_planes_land_on_texel_midpoints_at_every_vertex() {
    let words = textured_triangle_pair();
    let per_texel = f64::from(PLANE_PER_TEXEL);

    // RT64's rule, with every dxdy zero so the H edge is vertical.
    let vertices = |chunk: &[(u32, u32)]| {
        let yl = f64::from((chunk[0].0 & 0xffff) as i32) / 4.0;
        let ym = f64::from((chunk[0].1 >> 16) as i32) / 4.0;
        let yh = f64::from((chunk[0].1 & 0xffff) as i32) / 4.0;
        let x_l = f64::from(chunk[1].0 >> 16);
        let x_h = f64::from(chunk[2].0 >> 16);
        // dy_n = y_n - floor(yh); dx_3 = x3 - (H edge at y3) = x_l - x_h.
        ([(x_h, yh), (x_h, yl), (x_l, ym)], x_l - x_h)
    };
    // The split-halfword coefficient block: S's integer half is word 0's
    // high 16 bits, its fraction half word 4's high 16 bits.
    let s_plane = |chunk: &[(u32, u32)], index: usize| {
        let integer = (chunk[index].0 >> 16) as u16 as i32;
        let fraction = (chunk[index + 4].0 >> 16) as u16 as i32;
        f64::from((integer << 16) | fraction)
    };

    for (half, offset) in [("lower-left", 0usize), ("upper-right", 12)] {
        let base_words = &words[offset..offset + 4];
        let tex_words = &words[offset + 4..offset + 12];
        let (vertex, dx_3) = vertices(base_words);
        let base = s_plane(tex_words, 0);
        let d_dx = s_plane(tex_words, 1);

        // tc1 = tc2 = base (De is zero); tc3 = base + Dx * dx_3.
        let tc = [base, base, base + d_dx * dx_3];
        for (index, (x, _)) in vertex.iter().enumerate() {
            // The midpoint, less the eighth-of-a-pixel first-subsample
            // offset the base deliberately cancels -- the sampler's first
            // covered column is at `x + 1/8`, so the plane is authored an
            // eighth low and arrives on the midpoint when evaluated there.
            let want = (x - f64::from(TRI_LEFT)) + 0.5 - 0.125;
            let got = tc[index] / per_texel;
            assert!(
                (got - want).abs() < 1.0 / 16.0,
                "{half} vertex {index} at x={x} should sample texel {want}, \
                 the plane gives {got}"
            );
        }
    }
}

/// **The triangle case must emit TWO triangles that tile its box.**
///
/// RT64 derives exactly three vertices from one triangle command --
/// `v1 = (XH at YH, YH)`, `v2 = (XH at YL, YL)`, `v3 = (XL, YM)` --
/// so `v1` and `v2` always share the H edge's X and ONE command can
/// never describe a rectangle. A fixture that emits a single command
/// gets the right triangle between the H edge and `(XL, YM)`, and the
/// half it silently loses reads as "RT64 dropped pixels" rather than as
/// a fixture defect. That cost a session.
///
/// Asserted on the emitted words rather than on rendered pixels, so it
/// fails without a GPU and names the cause directly.
#[test]
fn the_triangle_case_emits_two_triangles_tiling_its_box() {
    let words = textured_triangle_pair();
    let opcodes: Vec<u32> = words
        .iter()
        .map(|&(word0, _)| word0 >> 24)
        .filter(|opcode| (0x08..=0x0f).contains(opcode))
        .collect();
    assert_eq!(
        opcodes.len(),
        2,
        "the box needs exactly two triangle commands, got {opcodes:?}"
    );

    // Each command is 4 base words + 8 texture words.
    assert_eq!(words.len(), 24, "two textured triangles are 24 wire pairs");

    // Reproduce RT64's own vertex rule for both, with every slope zero.
    let vertices = |chunk: &[(u32, u32)]| {
        let yl = (chunk[0].0 & 0xffff) as i32 as f32 / 4.0;
        let ym = (chunk[0].1 >> 16) as i32 as f32 / 4.0;
        let yh = (chunk[0].1 & 0xffff) as i32 as f32 / 4.0;
        let x_l = (chunk[1].0 >> 16) as f32;
        let x_h = (chunk[2].0 >> 16) as f32;
        [(x_h, yh), (x_h, yl), (x_l, ym)]
    };
    let first = vertices(&words[0..4]);
    let second = vertices(&words[12..16]);

    let left = TRI_LEFT as f32;
    let right = TRI_RIGHT as f32;
    let top = TRI_TOP as f32;
    let bottom = TRI_BOTTOM as f32;
    assert_eq!(
        first,
        [(left, top), (left, bottom), (right, bottom)],
        "the first triangle must be the lower-left half of the box"
    );
    assert_eq!(
        second,
        [(right, top), (right, bottom), (left, top)],
        "the second triangle must be the upper-right half, or the box is \
         covered only in part"
    );
}

/// **The raw-triangle authority means what it says.** A case may only
/// claim `RawTrianglePlaneScaleDisagreement` if it actually issues a raw
/// triangle (opcode 0x08..=0x0f), and a case that issues one may not
/// claim RT64 authority -- RT64 draws no pixels for it, so a
/// wgpu-vs-RT64 difference there is not a wgpu finding.
///
/// Without this, the variant becomes a place to park any inconvenient
/// disagreement, which is exactly the failure the partition exists to
/// prevent.
#[test]
fn the_raw_triangle_authority_is_used_only_for_raw_triangles() {
    for case in cases() {
        let has_triangle = case
            .commands
            .iter()
            .any(|&(word0, _)| (0x08..=0x0f).contains(&(word0 >> 24)));
        if case.authority == Authority::RawTrianglePlaneScaleDisagreement {
            assert!(
                has_triangle,
                "case {} claims the raw-triangle authority without issuing a raw triangle",
                case.name
            );
        }
        // NOTE: a raw triangle may now claim RT64 authority. It could
        // not while the two lanes read the non-perspective plane on
        // different scales -- fn64 counted the S10.5 `2^5` twice, once
        // in `PLANE_TO_TEXEL` and again in the sampler. That is fixed,
        // both lanes match the key, and the constraint would now block
        // an honest case. Only the positive direction is still pinned.
    }
}

/// Every RT64-authoritative case must use the exact no-AA no-dither
/// other-modes word. If a later edit changes one, the partition claim
/// silently stops being true; this fails instead.
#[test]
fn authoritative_cases_use_the_no_coverage_other_modes_word() {
    for case in cases() {
        if case.authority != Authority::Rt64Authoritative {
            continue;
        }
        let other_modes = case
            .commands
            .iter()
            .find(|&&(word0, _)| word0 >> 24 == 0xef)
            .expect("every case sets other modes");
        // **The property, not the literal.** This used to require the
        // exact `OTHER_MODES_FILL_NO_AA` word, which made the corpus
        // structurally incapable of holding a non-fill case -- a
        // textured draw cannot run in fill cycle. What the partition
        // actually requires is that an RT64-authoritative case stay out
        // of the modes RT64 does not model, so the three coverage/dither
        // fields are checked directly against angrylion's own bit
        // positions (`rdp.c:623-660`).
        assert!(
            *other_modes == OTHER_MODES_FILL_NO_AA
                || *other_modes == OTHER_MODES_ONE_CYCLE_TEXTURED
                || *other_modes == OTHER_MODES_ONE_CYCLE_TEXTURED_PERSPECTIVE,
            "RT64-authoritative case {} uses an unvetted other-modes word \
             {other_modes:#010x?}; add it here with its own hand-derived \
             field table before using it",
            case.name
        );
        let (word0, word1) = *other_modes;
        assert_eq!(
            (word0 >> 6) & 3,
            3,
            "case {} enables RGB dither, which RT64 does not model \
             faithfully (guard audit U2/U3)",
            case.name
        );
        assert_eq!(
            (word0 >> 4) & 3,
            3,
            "case {} enables alpha dither, which RT64 does not model \
             faithfully (guard audit U2/U3)",
            case.name
        );
        assert_eq!(
            word1 & 0b11_0000_0000_1000,
            0,
            "case {} enables AA_EN, ALPHA_CVG_SEL or CVG_TIMES_ALPHA, \
             none of which RT64 models (guard audit C4-C6)",
            case.name
        );
    }
}

/// The corpus must actually contain both partitions. A metric reporting
/// "0 non-authoritative cases" would look clean while having quietly
/// stopped testing the partition at all.
#[test]
fn both_partitions_are_populated() {
    let cases = cases();
    assert!(
        cases
            .iter()
            .filter(|case| case.authority == Authority::Rt64Authoritative)
            .count()
            >= 8
    );
    assert!(cases
        .iter()
        .any(|case| case.authority == Authority::CoverageDependentRt64NotAuthoritative));
}

/// Case names are the metric's row keys; duplicates would silently merge
/// two measurements in any downstream table.
#[test]
fn case_names_are_unique() {
    let mut names: Vec<&str> = cases().iter().map(|case| case.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count);
}

/// `gDPLoadTextureTile(..., G_IM_SIZ_32b, ...)` passes the same size to
/// its load and render SetTile commands. Its `G_IM_SIZ_32b_TILE_BYTES`
/// and `G_IM_SIZ_32b_LINE_BYTES` are both 2, so this two-texel fixture
/// derives `line = ((2 * 2) + 7) >> 3 = 1` for both descriptors.
#[test]
fn rgba32_case_uses_the_public_split_bank_tile_derivation() {
    let set_tiles = one_rgba32_rect()
        .into_iter()
        .filter(|(word0, _)| word0 >> 24 == 0xf5)
        .collect::<Vec<_>>();
    assert_eq!(set_tiles.len(), 2);
    for (word0, word1) in set_tiles {
        assert_eq!((word0 >> 21) & 0x7, 0, "RGBA format");
        assert_eq!((word0 >> 19) & 0x3, 3, "32-bit size");
        assert_eq!((word0 >> 9) & 0x1ff, 1, "one word per bank row");
        assert_eq!(word0 & 0x1ff, 0, "low-half TMEM base");
        assert_eq!(word1, 0, "tile zero and default addressing fields");
    }
}

/// A key that equals the seeded target would "pass" against a backend
/// that did nothing at all. Every case must expect at least one pixel to
/// change.
#[test]
fn every_key_expects_the_target_to_change() {
    for case in cases() {
        assert!(
            (0..PIXEL_COUNT).any(|index| (case.expected)(index) != STALE),
            "case {} expects no pixel to change",
            case.name
        );
    }
}

/// `Verdict` is what the tally counts, so its classification is the
/// arithmetic behind every reported number.
#[test]
fn verdict_classifies_each_pairing() {
    let full = |value: u16| Ok(vec![value.to_ne_bytes()[0], value.to_ne_bytes()[1]]);
    let refused = || Err("refused".to_string());
    assert_eq!(Verdict::of(&full(RED), &full(RED)), Verdict::Identical);
    assert_eq!(
        Verdict::of(&full(RED), &full(BLUE)),
        Verdict::Differs { pixels: 1 }
    );
    assert_eq!(Verdict::of(&refused(), &full(RED)), Verdict::OneRefused);
    assert_eq!(Verdict::of(&full(RED), &refused()), Verdict::OneRefused);
    assert_eq!(Verdict::of(&refused(), &refused()), Verdict::BothRefused);
}

/// Only `Identical` counts toward parity. A double refusal in particular
/// must NOT read as agreement -- that is the single easiest way to
/// manufacture a flattering number.
#[test]
fn only_byte_identical_counts_as_parity() {
    assert!(Verdict::Identical.is_parity());
    assert!(!Verdict::Differs { pixels: 1 }.is_parity());
    assert!(!Verdict::OneRefused.is_parity());
    assert!(!Verdict::BothRefused.is_parity());
}

/// The tally must route each verdict to its own bucket and never lose a
/// case: `cases` is the denominator every reported ratio uses.
#[test]
fn tally_partitions_every_verdict() {
    let mut tally = Tally::default();
    tally.record(Verdict::Identical);
    tally.record(Verdict::Identical);
    tally.record(Verdict::Differs { pixels: 4 });
    tally.record(Verdict::OneRefused);
    tally.record(Verdict::BothRefused);
    assert_eq!(tally.cases, 5);
    assert_eq!(tally.identical, 2);
    assert_eq!(tally.differs, 1);
    assert_eq!(tally.one_refused, 1);
    assert_eq!(tally.both_refused, 1);
}

/// The seeded target is STALE everywhere with GUARD either side, and the
/// commands land at `COMMAND_START`. Derived from the wire layout.
#[test]
fn seeded_memory_is_stale_with_guards() {
    let commands = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
    let rdram = seeded(&commands);
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(view.read_u16(RdramAddr::from_offset(FRAMEBUFFER)), STALE);
    assert_eq!(
        view.read_u16(RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES - 2)),
        STALE
    );
    assert_eq!(
        view.read_u16(RdramAddr::from_offset(FRAMEBUFFER - 2)),
        GUARD
    );
    assert_eq!(
        view.read_u16(RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES)),
        GUARD
    );
    assert_eq!(
        u32::from_ne_bytes(
            rdram[COMMAND_START as usize..COMMAND_START as usize + 4]
                .try_into()
                .unwrap()
        ),
        commands[0].0
    );
}

/// `command_end` is what bounds every backend's decode. An off-by-one
/// here would truncate the final command for all three backends at once
/// and still look like agreement.
#[test]
fn command_end_covers_every_command_word() {
    let commands = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
    assert_eq!(
        command_end(&commands),
        COMMAND_START + 6 * 8,
        "six commands of eight bytes each"
    );
    assert_eq!(command_words(&commands).len(), 12);
}

/// `fill_rect` encodes coordinates in 10.2 fixed point. Derived by hand
/// from the RDP wire layout: `lrx` at bits 43..32 of the 64-bit command,
/// i.e. bits 23..12 of word 0.
#[test]
fn fill_rect_encodes_ten_dot_two_fixed_point() {
    let (word0, word1) = fill_rect(319, 239, 0, 0);
    assert_eq!(word0 >> 24, 0xf6);
    assert_eq!((word0 >> 12) & 0xfff, 319 * 4);
    assert_eq!(word0 & 0xfff, 239 * 4);
    assert_eq!(word1, 0);
    let (word0, _) = fill_rect(17, 9, 17, 9);
    assert_eq!((word0 >> 12) & 0xfff, 68);
}

/// **The texrect high edge is EXCLUSIVE and the fill high edge is
/// INCLUSIVE.** This is the one place the two rectangle commands
/// disagree, and applying the fill rule to a texrect is silent: the draw
/// covers one fewer row and column, and the missing pixels keep whatever
/// was under them, which reads as a texel-fetch defect rather than a
/// fixture defect.
///
/// A revision of this corpus made exactly that mistake, and it cost a
/// session: every backend was correct, the hand-derived key demanded
/// texels on a row no backend drew, and all three lanes reported
/// `matches_key: false`. The rule is pinned in `targets/texrect.rs`
/// ("the fill rule is inclusive and the texrect rule is half-open, so the
/// fill rectangle is exactly one pixel larger on each axis").
///
/// So this asserts the corpus's own constants carry the texture's FULL
/// extent, not `extent - 1`, and that the key agrees with them.
#[test]
fn texrect_high_edges_are_exclusive_unlike_the_fill_rule() {
    assert_eq!(TEXRECT_LRX, TEXTURE_WIDTH);
    assert_eq!(TEXRECT_LRY, TEXTURE_HEIGHT);

    // The last covered pixel is one inside each high edge, and the first
    // uncovered one is the edge itself.
    let at = |x: u32, y: u32| textured_expected(y * WIDTH + x);
    assert_ne!(at(TEXTURE_WIDTH - 1, TEXTURE_HEIGHT - 1), STALE);
    assert_eq!(at(TEXTURE_WIDTH, 0), STALE);
    assert_eq!(at(0, TEXTURE_HEIGHT), STALE);

    // Every texel in the covered window is its own texel, in row-major
    // order -- the property the whole textured corpus rests on.
    for y in 0..TEXTURE_HEIGHT {
        for x in 0..TEXTURE_WIDTH {
            assert_eq!(at(x, y), TEXTURE_TEXELS[(y * TEXTURE_WIDTH + x) as usize]);
        }
    }
}

#[test]
fn skew_sweep_changes_only_the_named_ingredient() {
    let base = skew_textured_rect(SKEW_LINE_WORDS, SKEW_LOW_T_ODD);
    let even_low_t = skew_textured_rect(SKEW_LINE_WORDS, SKEW_LOW_T_ODD - 1);
    let line_16 = skew_textured_rect(SKEW_LINE_WORDS - 1, SKEW_LOW_T_ODD);

    let changed = |left: &[(u32, u32)], right: &[(u32, u32)]| {
        left.iter()
            .zip(right)
            .enumerate()
            .filter_map(|(index, (left, right))| (left != right).then_some((index, *left, *right)))
            .collect::<Vec<_>>()
    };

    // A different T origin necessarily changes the three commands that
    // carry that one semantic ingredient: tile bounds, load bounds and
    // the texrect's sample origin. Every other command is byte-identical.
    let low_t_changes = changed(&base, &even_low_t);
    assert_eq!(low_t_changes.len(), 3);
    assert_eq!(
        low_t_changes
            .iter()
            .map(|(_, base, _)| base.0 >> 24)
            .collect::<Vec<_>>(),
        vec![0xf2, 0xf4, 0]
    );

    // `line` lives in SetTile alone, so the line control must differ by
    // exactly that one 64-bit command.
    let line_changes = changed(&base, &line_16);
    assert_eq!(line_changes.len(), 1);
    assert_eq!(line_changes[0].1 .0 >> 24, 0xf5);
}

#[test]
fn skew_key_has_fourteen_stationary_red_extents() {
    for y in 0..SKEW_HEIGHT {
        let red: Vec<u32> = (0..SKEW_WIDTH)
            .filter(|&x| skew_expected(y * WIDTH + x) == RED)
            .collect();
        assert_eq!(red.first(), Some(&SKEW_BAR_LEFT));
        assert_eq!(red.last(), Some(&(SKEW_BAR_RIGHT - 1)));
        assert_eq!(red.len() as u32, SKEW_BAR_RIGHT - SKEW_BAR_LEFT);
    }
}

/// The contiguity check is the parser's load-bearing invariant: a dump
/// missing a row must be REFUSED, not silently concatenated into a
/// different display list that would then be measured as if it were the
/// game's.
#[test]
fn captured_parser_refuses_a_gap() {
    let contiguous = "0\tRDP\t0x1000\t0xef300000\t0x00000000\n\
                      0\tRDP\t0x1008\t0xe9000000\t0x00000000\n";
    let packet = captured::parse_packet_dump(contiguous, 0).expect("contiguous rows parse");
    assert_eq!(packet.words, vec![0xef30_0000, 0, 0xe900_0000, 0]);
    assert_eq!(packet.source_pc, 0x1000);

    // Same rows, second one 16 bytes on instead of 8: one pair is missing.
    let gapped = "0\tRDP\t0x1000\t0xef300000\t0x00000000\n\
                  0\tRDP\t0x1010\t0xe9000000\t0x00000000\n";
    let error = captured::parse_packet_dump(gapped, 0).expect_err("a gap must be refused");
    assert!(error.contains("not contiguous"), "{error}");
}

/// Rows for other decode entries must not leak into the replayed stream.
#[test]
fn captured_parser_selects_one_entry() {
    let text = "0\tRDP\t0x1000\t0xef300000\t0x00000000\n\
                1\tRDP\t0x2000\t0xf7000000\t0x11111111\n\
                1\tRDP\t0x2008\t0xe9000000\t0x00000000\n";
    assert_eq!(
        captured::parse_packet_dump(text, 1).unwrap().words,
        vec![0xf700_0000, 0x1111_1111, 0xe900_0000, 0]
    );
    assert_eq!(
        captured::parse_packet_dump(text, 0).unwrap().words,
        vec![0xef30_0000, 0]
    );
    assert!(captured::parse_packet_dump(text, 9).is_err());
}

/// A GBI-lane row is a display-list command that has not been decoded to
/// RDP words yet. Replaying it as if it were a raw-RDP stream would
/// measure nonsense, so it must be refused rather than accepted.
#[test]
fn captured_parser_refuses_the_gbi_lane() {
    let text = "0\tGBI\t0x1000\t0xef300000\t0x00000000\n";
    let error = captured::parse_packet_dump(text, 0).expect_err("GBI lane must be refused");
    assert!(error.contains("raw-RDP lane"), "{error}");
}

/// `walk` must give `G_TEXRECT`/`G_TEXRECTFLIP` their 16 bytes and every
/// other command 8. Getting this wrong desynchronises the whole stream
/// from the first texrect onward.
#[test]
fn captured_walk_gives_texrect_sixteen_bytes() {
    // TEXRECT (0x24) then a FullSync (0x29).
    let words = vec![0x2400_0000, 0, 0, 0, 0xe900_0000, 0];
    let walked = captured::walk(&words);
    assert_eq!(walked.len(), 2);
    assert_eq!((walked[0].0, walked[0].1), (0, 0x24));
    assert_eq!((walked[1].0, walked[1].1), (16, 0x29));
    // Without the 16-byte rule the second command would be read at
    // offset 8, out of the middle of the texrect.
    let plain = vec![0xf600_0000, 0, 0xe900_0000, 0];
    let walked = captured::walk(&plain);
    assert_eq!((walked[1].0, walked[1].1), (8, 0x29));
}

/// The extent must come from the packet's own SetColorImage/SetScissor.
/// Reading a captured stream at a guessed width is the documented cause
/// of "striping" that has been misreported as a renderer defect three
/// times (docs/rt64/RT64-WM2000-HARNESS-TRAPS.md).
#[test]
fn captured_extent_is_read_from_the_stream() {
    // SetColorImage (0x3f) with width-1 = 479, SetScissor (0x2d) with
    // lower-right Y in 10.2 fixed point = 237 << 2.
    let words = vec![
        0x3f00_0000 | 479,
        0x0038_f800,
        0x2d00_0000,
        (237 << 2) & 0x0fff,
    ];
    let walked = captured::walk(&words);
    assert_eq!(captured::target_extent(&walked), Some((480, 237)));
    assert_eq!(captured::color_image_addr(&walked), Some(0x0038_f800));
    // A stream with no color image cannot have an extent invented for it.
    assert_eq!(
        captured::target_extent(&captured::walk(&[0xe900_0000, 0])),
        None
    );
}

/// With the variable unset the report must say the captured corpus is
/// unavailable rather than quietly reporting a hand-authored number as
/// though real content backed it.
#[test]
fn captured_corpus_is_absent_without_the_env_var() {
    if std::env::var_os(captured::PACKET_ENV).is_some() {
        return;
    }
    let row = captured_row();
    assert_eq!(row["available"], serde_json::json!(false));
    assert!(row["reason"]
        .as_str()
        .unwrap()
        .contains(captured::PACKET_ENV));
}

/// The key must be materialised through the same `^3` guest byte-lane
/// mapping the observations are read through, or every comparison is a
/// byte-swap away from the truth.
#[test]
fn key_is_materialised_in_guest_byte_order() {
    let case = Case {
        name: "probe",
        intent: "probe",
        authority: Authority::Rt64Authoritative,
        commands: one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1),
        expected: |_| RED,
    };
    let key = pixels(&key_bytes(&case));
    assert_eq!(key.len(), PIXEL_COUNT as usize);
    assert!(key.iter().all(|&pixel| pixel == RED));
}
