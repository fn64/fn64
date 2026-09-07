//! The generated-corpus collector and its triage/report helpers.

use super::*;

pub(crate) fn generated_cases() -> Vec<GeneratedCase> {
    let mut cases = Vec::new();
    let mut push = |priority: u8, name: String, intent: &'static str, commands: Vec<(u32, u32)>| {
        cases.push(GeneratedCase {
            name,
            priority,
            intent,
            commands,
        });
    };

    // (5) Mode matrix -- cycle type x fill box. Fill/copy through the fill
    // path, 1cyc/2cyc through the pixel pipe. These are texture-source-
    // independent, so they give clean angrylion signal today.
    for (cycle, label) in [(3u32, "fill"), (0, "one-cycle"), (1, "two-cycle")] {
        push(
            5,
            format!("gen-modematrix-cycle-{label}-red-box"),
            "cycle type x rectangle fill",
            gen_fill_frame(0xf801, cycle, 0, 0, 80, 60),
        );
    }
    // Fill-cycle boxes at varied colours and extents -- edge/coverage of the
    // fill rasteriser, the most-used real-ROM primitive.
    for (color, label) in [
        (0xf801u16, "red"),
        (0x07c1, "green"),
        (0x003f, "blue"),
        (0x7fff, "white"),
    ] {
        push(
            5,
            format!("gen-fill-{label}-fullwidth-band"),
            "fill-cycle rectangle, full-width band",
            gen_fill_frame(color, 3, 0, 100, WIDTH, 140),
        );
    }
    // Single-pixel and last-pixel fills -- rasteriser boundary conditions.
    push(
        5,
        "gen-fill-single-pixel".into(),
        "fill single pixel",
        gen_fill_frame(0xf801, 3, 10, 10, 11, 11),
    );
    push(
        5,
        "gen-fill-last-pixel".into(),
        "fill last pixel",
        gen_fill_frame(0x07c1, 3, WIDTH - 1, HEIGHT - 1, WIDTH, HEIGHT),
    );

    // (2) Triangle variants 0x08..0x0f. Only the flat (0x08) and shade (0x0c)
    // are in the hand corpus; the rest of the family is untested. Texture
    // variants (0x0a/0x0e) reuse the proven textured builder.
    for opcode in [0x08u32, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f] {
        push(
            2,
            format!("gen-triangle-opcode-{opcode:#04x}"),
            "raw triangle opcode family",
            gen_triangle_variant(opcode),
        );
    }

    // (3) Syncs: PIPESYNC (0x27), TILESYNC (0x28), LOADSYNC (0x26),
    // FULLSYNC is already every frame's closer. Insert each into a valid fill
    // and confirm the raster is unperturbed.
    for (op, label) in [
        (0x27u32, "pipesync"),
        (0x28, "tilesync"),
        (0x26, "loadsync"),
    ] {
        push(
            3,
            format!("gen-sync-{label}-in-fill"),
            "sync opcode inside a fill frame",
            gen_fill_with_sync(op),
        );
    }

    // (1) LOADBLOCK + (4) TexRectFlip come from the proven textured builders
    // that stage RGBA16 source. Their angrylion reading depends on the RGBA16
    // texture-source staging domain (under investigation), so they are the
    // highest priority but their triage waits on that resolution.
    // Textured rects/blocks: correct the missing BI_LERP_0 so angrylion
    // samples the full RGBA texel instead of collapsing it to the blue
    // channel. Also emit the UNCORRECTED loadblock-linear as an explicit
    // regression witness for the bilerp finding.
    push(
        1,
        "gen-loadblock-linear".into(),
        "LoadBlock linear row advance (bilerp corrected)",
        set_bilerp0(load_block_textured_rect(8, 0, 2, 8, 1, 2)),
    );
    push(
        1,
        "gen-loadblock-dxt".into(),
        "LoadBlock DxT row advance (bilerp corrected)",
        set_bilerp0(load_block_textured_rect(16, 0x400, 2, 8, 2, 4)),
    );
    push(
        4,
        "gen-texrect-flip".into(),
        "TexRectFlip S/T swap (bilerp corrected)",
        set_bilerp0(one_textured_rect_flip()),
    );
    push(
        2,
        "gen-textured-triangle".into(),
        "textured triangle (bilerp corrected)",
        set_bilerp0(one_textured_triangle()),
    );
    // Witness: the SAME loadblock WITHOUT the bilerp correction. Expected to
    // reproduce the wgpu==RT64 vs angrylion divergence — documents the finding
    // as a live, reproducible corpus row.
    push(
        1,
        "gen-loadblock-linear-missing-bilerp".into(),
        "LoadBlock WITHOUT BI_LERP_0 (bilerp-gap witness)",
        load_block_textured_rect(8, 0, 2, 8, 1, 2),
    );

    // Right-edge texel over-read (task 36 reproduction). A one-cycle texrect
    // drawn one pixel wider than the loaded tile, so the rightmost column
    // samples texel TEXTURE_WIDTH -- one past the loaded S extent. The two
    // addressing modes give a DIFFERENT rightmost-column texel, so whichever
    // the sampler produces, a right-edge over-read shows up as a wgpu-vs-
    // angrylion divergence on the last column.
    push(1, "gen-texrect-right-edge-overread-clamp".into(), "texrect right edge one past loaded S extent, CLAMP (mask_s==0): last column must clamp to the last loaded texel", right_edge_overread_rect(RightEdgeAddressing::Clamp));
    push(1, "gen-texrect-right-edge-overread-wrap".into(), "texrect right edge one past loaded S extent, WRAP (mask_s==2): last column must wrap to texel 0", right_edge_overread_rect(RightEdgeAddressing::Wrap));

    // -------------------------------------------------------------------
    // Track-B fan-out pass 1: designed slices.
    // -------------------------------------------------------------------

    // (5) Mode matrix -- BLENDER. P/A/M/B mux across the common real-ROM
    // configurations: passthrough, alpha-blend over clr_mem, blend-color,
    // fog-color, shade-alpha-driven, and coverage-driven blends. All draw a
    // primitive- or shade-combined triangle pair over a GREEN memory seed
    // with an opaque RED primitive/shade, so the visible outcome always
    // distinguishes "used P" from "used M" from "mixed the two".
    push(
        5,
        "gen-blender-passthrough".into(),
        "blender P=Combined A=Combined M=Combined B=Zero (b==0 bypass): pure clr_in*1 passthrough, memory untouched by the blend math",
        gen_blend_rect(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            blend_other_modes(0, 0, 0, 3, true, false),
        ),
    );
    push(
        5,
        "gen-blender-alpha-blend-over-mem".into(),
        "blender P=Combined A=CombinedAlpha M=Framebuffer B=1-A: opaque-alpha combined color alpha-composited over clr_mem (opaque primitive -> full replace, exercises the clr_mem read path)",
        gen_blend_rect(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            blend_other_modes(0, 0, 1, 0, true, true),
        ),
    );
    push(
        5,
        "gen-blender-blend-color-over-mem".into(),
        "blender P=BlendColor A=CombinedAlpha M=Framebuffer B=1-A: SetBlendColor supplies P, composited over clr_mem with opaque combiner alpha",
        gen_blend_rect_with_state_color(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            (0xf900_0000, 0x4080_c0ff),
            blend_other_modes(2, 0, 1, 0, true, true),
        ),
    );
    push(
        5,
        "gen-blender-fog-color-over-mem".into(),
        "blender P=FogColor A=CombinedAlpha M=Framebuffer B=1-A: SetFogColor supplies P, composited over clr_mem with opaque combiner alpha",
        gen_blend_rect_with_state_color(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            (0xf800_0000, 0x2060_a0ff),
            blend_other_modes(3, 0, 1, 0, true, true),
        ),
    );
    push(
        5,
        "gen-blender-shade-alpha-driven".into(),
        "blender P=Combined(shade) A=ShadeAlpha M=Framebuffer B=1-A: a genuine fractional shade alpha (0x80/0xff) drives a real P/M mix rather than the 0/1 extremes",
        gen_blend_rect_shade_alpha(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_SHADE_RGBA,
            blend_other_modes(0, 2, 1, 0, true, true),
        ),
    );
    push(
        5,
        "gen-blender-coverage-driven".into(),
        "blender P=Combined A=CombinedAlpha M=Framebuffer B=FramebufferCoverage/8: the AA/coverage-substituted-for-B path -- full interior coverage makes B=1, mixing P and M through the general divisor rather than either short-circuit branch",
        gen_blend_rect(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            blend_other_modes(0, 0, 1, 1, true, true),
        ),
    );
    push(
        5,
        "gen-blender-force-bl-off-selects-p".into(),
        "FORCE_BL=0 (bit 14 clear): the last blend stage is bypassed and unconditionally selects P (BlendColor here), independent of A/M/B, exercising the non-blended one-cycle default every WM2000 opaque draw actually uses",
        gen_blend_rect_with_state_color(
            BLEND_MATRIX_MEMORY_SEED,
            BLEND_MATRIX_PRIMITIVE_RGBA8888,
            (0xf900_0000, 0x4080_c0ff),
            blend_other_modes(2, 0, 0, 0, false, false),
        ),
    );

    // (5) Alpha compare matrix -- threshold-compare and dither-compare, plus
    // a disabled-bits control that proves the bits gate the test rather
    // than the primitive alpha value alone suppressing output.
    push(
        5,
        "gen-alpha-compare-threshold-pass".into(),
        "alpha_compare_en=threshold (mode word1 bits1:0=1); prim alpha 0xff \
         exceeds SetBlendColor's threshold 0x80, so the compare passes and \
         the rectangle is written",
        gen_alpha_compare_rect(1, 0xff, 0x80),
    );
    push(
        5,
        "gen-alpha-compare-threshold-reject".into(),
        "alpha_compare_en=threshold; prim alpha 0x20 is below SetBlendColor's \
         threshold 0x80, so every covered pixel fails the compare and the \
         STALE background survives the whole rectangle",
        gen_alpha_compare_rect(1, 0x20, 0x80),
    );
    push(
        5,
        "gen-alpha-compare-threshold-boundary-equal".into(),
        "alpha_compare_en=threshold with prim alpha EQUAL to SetBlendColor's \
         threshold (0x80==0x80), isolating hardware's exact boundary \
         predicate (strictly-less-than rejects vs less-or-equal rejects) \
         rather than assuming either",
        gen_alpha_compare_rect(1, 0x80, 0x80),
    );
    push(
        5,
        "gen-alpha-compare-dither-forced-pass".into(),
        "alpha_compare_en=dither (mode word1 bits1:0=3): compares combined \
         alpha against a per-pixel pseudorandom noise value in [0,255] \
         instead of a fixed threshold. Prim alpha is forced to the maximum \
         0xff, which no possible noise sample in [0,255] exceeds, so the \
         compare is deterministically a pass everywhere regardless of the \
         dither seed a backend implements",
        gen_alpha_compare_rect(3, 0xff, 0x00),
    );
    push(
        5,
        "gen-alpha-compare-dither-forced-reject".into(),
        "alpha_compare_en=dither with prim alpha forced to the minimum \
         0x00: any nonzero noise sample exceeds it, so the compare rejects \
         almost everywhere. The single-noise-value-of-zero corner is the \
         one pixel-level case a dither implementation's exact PRNG can \
         disagree on; every other covered pixel is a deterministic reject",
        gen_alpha_compare_rect(3, 0x00, 0x00),
    );
    push(
        5,
        "gen-alpha-compare-disabled-control".into(),
        "alpha_compare_en=NONE (mode word1 bits1:0=0) with the SAME prim \
         alpha (0x20) and threshold (0x80) as the threshold-reject case: \
         proves the compare bits themselves gate rejection, not the low \
         alpha value alone suppressing output -- with the unit disabled \
         the rectangle must be written in full",
        gen_alpha_compare_rect(0, 0x20, 0x80),
    );

    // (6) Coverage-modes matrix -- cvg_dest x color_on_cvg x cvg_x_alpha x
    // force_blend, across fill and one-cycle rects. RT64-non-authoritative
    // per guard-audit C4-C6 ("Coverage is not emulated"); angrylion is the
    // sole judge for these rows.
    push_coverage_mode_cases(&mut push);

    // (6) Formats-deep -- direct/CI texture formats sampled by a TEXTURED
    // TRIANGLE instead of a texture rectangle, reusing each format's proven
    // texrect staging. IA/I formats are immune to the BI_LERP_0 collapse
    // (their value already lives in the blue channel); RGBA32/CI4/CI8 all
    // set BI_LERP_0.
    push(
        6,
        "gen-triangle-ia8".into(),
        "IA8 as a textured-triangle source (format=3,size=1,line=1)",
        direct_format_textured_triangle(IA8_SOURCE, 8, 3, 1, 1),
    );
    push(
        6,
        "gen-triangle-ia4".into(),
        "IA4 as a textured-triangle source (format=3,size=0,line=1), packed-nibble addressing through triangles",
        direct_format_textured_triangle(IA4_SOURCE, 7, 3, 0, 1),
    );
    push(
        6,
        "gen-triangle-ia16".into(),
        "IA16 as a textured-triangle source (format=3,size=2,line=2)",
        direct_format_textured_triangle(IA16_SOURCE, 8, 3, 2, 2),
    );
    push(
        6,
        "gen-triangle-i4".into(),
        "I4 as a textured-triangle source (format=4,size=0,line=1), packed-nibble intensity replication via triangles",
        direct_format_textured_triangle(I4_SOURCE, 8, 4, 0, 1),
    );
    push(
        6,
        "gen-triangle-i8".into(),
        "I8 as a textured-triangle source (format=4,size=1,line=1), byte-addressed intensity replication via triangles",
        direct_format_textured_triangle(I8_SOURCE, 8, 4, 1, 1),
    );
    push(
        6,
        "gen-triangle-rgba32-bilerp".into(),
        "RGBA32 as a textured-triangle source (bilerp corrected: BI_LERP_0 set so RGBA is not collapsed to blue)",
        rgba32_textured_triangle(true),
    );
    push(
        1,
        "gen-triangle-rgba32-missing-bilerp".into(),
        "RGBA32 textured triangle WITHOUT BI_LERP_0 (bilerp-gap witness, triangle path)",
        rgba32_textured_triangle(false),
    );
    push(
        6,
        "gen-triangle-ci4-bilerp".into(),
        "CI4+16-entry TLUT as a textured-triangle source (bilerp corrected), en_tlut bit set; expects palette lookup via triangle decode",
        ci4_textured_triangle(),
    );
    push(
        6,
        "gen-triangle-ci8-bilerp".into(),
        "CI8+256-entry TLUT as a textured-triangle source (bilerp corrected); expects sparse palette lookup via triangle decode",
        ci8_textured_triangle(),
    );

    // (5) Z-buffer matrix -- z_compare_en / z_update_en / z_source_sel
    // deciding which of two overlapping flat triangles survives, plus the
    // SetMaskImage alternate z-image binding (priority 6, per the brief's
    // own "(6) convert/key/maskimage" bucket).
    push(
        5,
        "gen-zbuffer-nearer-wins".into(),
        "z_compare_en+z_update_en on, G_ZS_PRIM: nearer (smaller Z) triangle drawn \
         second over a farther one must win and paint red",
        gen_zbuffer_compare_and_update(0x1000, 0x8000),
    );
    push(
        5,
        "gen-zbuffer-farther-loses".into(),
        "z_compare_en+z_update_en on, G_ZS_PRIM: farther (larger Z) triangle drawn \
         second over a nearer one must be REJECTED -- the first (nearer, green) \
         triangle's colour survives",
        gen_zbuffer_compare_and_update(0x8000, 0x1000),
    );
    push(
        5,
        "gen-zbuffer-compare-disabled".into(),
        "z_compare_en off, G_ZS_PRIM: depth never gates the write, so the \
         second-drawn triangle (nominally farther, Z=0x8000) wins purely by \
         draw order over the first (Z=0x1000) -- painter's-order behaviour",
        gen_zbuffer_compare_disabled(0x8000, 0x1000),
    );
    push(
        5,
        "gen-zbuffer-update-disabled".into(),
        "z_compare_en on, z_update_en off, G_ZS_PRIM: the twin of \
         gen-zbuffer-nearer-wins with the same far/near Z pair but update \
         disabled, so the first triangle's depth is never committed and the \
         second compares against the freshly-staged (zeroed) z-image instead -- \
         a backend that ignores z_update_en renders this identically to the \
         compare-and-update twin, which is the defect signal",
        gen_zbuffer_update_disabled(),
    );
    push(
        5,
        "gen-zbuffer-source-sel-pixel-wins".into(),
        "z_source_sel: a G_ZS_PIXEL raw triangle (opcode 0x09, explicit per-pixel \
         Z coefficient block) drawn over a farther G_ZS_PRIM triangle must win \
         under z_compare_en, proving compare read the coefficient-block Z and \
         not a stale PrimDepth register",
        gen_zbuffer_source_sel_pixel_wins(),
    );
    push(
        6,
        "gen-zbuffer-setmaskimage-binds-z-image".into(),
        "SetMaskImage (0x3e) as an alternate z-image binding, otherwise identical \
         to gen-zbuffer-nearer-wins. EXPECTED wgpu-refused: wgpu's raw-DPC \
         decoder has no dispatch arm for 0x3e (own unit test asserts \
         UnsupportedCommand) -- a real fn64 gap, logged here rather than fixed, \
         since angrylion and RT64 both treat 0x3e as a plain SetZImage alias.",
        gen_zbuffer_setmaskimage_binds_z_image(),
    );

    // (1) Loadblock-deep -- LOADBLOCK DxT row-advance sampled by TEXTURED
    // TRIANGLES (not texrects), across RGBA16 and CI8 sources, including
    // non-power-of-two DXT values that cross the 0x800 accumulator on a
    // fractional word boundary.
    push(
        1,
        "gen-loadblock-deep-rgba16-dxt400-triangle".into(),
        "LOADBLOCK 0x33 RGBA16, DXT=0x400 crossing 0x800 three times over four \
         rows, sampled by a textured TRIANGLE (bilerp corrected)",
        set_bilerp0(load_block_deep_triangle(32, 0x400, 2, 8, 4, 2)),
    );
    push(
        1,
        "gen-loadblock-deep-rgba16-dxt800-triangle".into(),
        "LOADBLOCK 0x33 RGBA16, DXT=0x800 advances every word (max stride), \
         sampled by a textured TRIANGLE (bilerp corrected)",
        set_bilerp0(load_block_deep_triangle(16, 0x800, 1, 4, 4, 1)),
    );
    push(
        1,
        "gen-loadblock-deep-rgba16-dxt-fractional-triangle".into(),
        "LOADBLOCK 0x33 RGBA16, DXT=0x300 crosses the 0x800 accumulator on a \
         fractional-word boundary, sampled by a textured TRIANGLE (bilerp \
         corrected)",
        set_bilerp0(load_block_deep_triangle(24, 0x300, 3, 8, 3, 2)),
    );
    push(
        1,
        "gen-loadblock-deep-ci8-dxt400-triangle".into(),
        "LOADBLOCK 0x33 CI8, DXT=0x400 row-advance at the CI8 (8 texels/word) \
         cadence, sampled by a textured TRIANGLE (bilerp corrected)",
        set_bilerp0(load_block_ci8_deep_triangle(
            LOADBLOCK_DEEP_CI8_SOURCE,
            &LOADBLOCK_DEEP_CI8_INDICES,
            32,
            0x400,
            2,
            16,
            2,
            2,
        )),
    );
    push(
        1,
        "gen-loadblock-deep-ci8-dxt800-triangle".into(),
        "LOADBLOCK 0x33 CI8, DXT=0x800 advances every word (8 texels/row), \
         sampled by a textured TRIANGLE (bilerp corrected)",
        set_bilerp0(load_block_ci8_deep_triangle(
            LOADBLOCK_DEEP_CI8_SOURCE,
            &LOADBLOCK_DEEP_CI8_INDICES,
            16,
            0x800,
            1,
            8,
            2,
            1,
        )),
    );
    push(
        1,
        "gen-loadblock-deep-ci8-dxt-fractional-triangle".into(),
        "LOADBLOCK 0x33 CI8, DXT=0x600 crosses the 0x800 accumulator on a \
         fractional-word boundary at the CI8 cadence, sampled by a textured \
         TRIANGLE (bilerp corrected)",
        set_bilerp0(load_block_ci8_deep_triangle(
            LOADBLOCK_DEEP_CI8_SOURCE,
            &LOADBLOCK_DEEP_CI8_INDICES,
            24,
            0x600,
            2,
            12,
            2,
            2,
        )),
    );

    // -------------------------------------------------------------------
    // Track-B fan-out pass 2: designed slices.
    // -------------------------------------------------------------------

    // slice two-cycle-combine
    push(
        2,
        "gen-two-cycle-texel0-combined-texel1-select".into(),
        "two-cycle: cycle 0 forms Combined := Texel0; cycle 1 blends toward \
         Texel1 with weight PrimitiveAlpha = 1.0, so the visible pixel is \
         exactly Texel1 -- proves COMBINED feeds cycle 1 and TEXEL1 samples tile+1",
        two_cycle_two_tile_rect(SET_COMBINE_TEXEL0_INTO_TEXEL1_SELECT, 0xff00_00ff),
    );
    push(
        2,
        "gen-two-cycle-texel1-direct".into(),
        "two-cycle: cycle 1 reads Texel1 directly, never touching Combined -- \
         isolates raw TEXEL1 TMEM addressing at tile+1 from COMBINED chaining",
        two_cycle_two_tile_rect(SET_COMBINE_TEXEL1_DIRECT, 0xff00_00ff),
    );
    push(
        2,
        "gen-two-cycle-wm2000-fog-program".into(),
        "the REAL WM2000 combine word 0xfc15fea3/0xf00ff23f (73,925 measured \
         draws), applied with Primitive=0 to isolate cycle 0's chain",
        wm2000_fog_triangle(),
    );
    push(
        2,
        "gen-two-cycle-wm2000-shade-fog-program".into(),
        "the REAL WM2000 combine word 0xfc45fea3/0xf00ff83f (7,762 draws), the \
         SHADE-driven near-twin of the dominant fog program",
        wm2000_shade_fog_triangle(),
    );
    push(
        2,
        "gen-two-cycle-combined-alpha-chain".into(),
        "two-cycle: cycle 0's alpha output (CombinedAlpha := PrimitiveAlpha) is \
         read by cycle 1's RGB C-term; two bands switch visible colour between \
         Primitive and black",
        combined_alpha_chain_bands(),
    );
    push(
        1,
        "gen-two-cycle-lod-fraction-gap".into(),
        "two-cycle: cycle 1 reads LodFraction as its RGB multiplier. NOT \
         expected to pass: wgpu hardcodes lod_fraction=0.0 while non-mipmap \
         hardware/RT64 use 1.0 -- a NEW gap distinct from two-cycle-textured",
        two_cycle_two_tile_rect(SET_COMBINE_LOD_FRACTION_GAP, 0xff00_00ff),
    );

    // slice blend-deep
    push_blend_deep_cases(&mut push);

    // slice tlut-palette-deep
    push(
        6,
        "gen-tlut-ci4-palette-bank-0".into(),
        "CI4 textured triangle, tile palette field = 0 (bank 0) into a 32-entry \
         two-bank TLUT; baseline reading. EXPECTED shared-ported-bug (#20).",
        ci4_palette_bank_textured_triangle(0),
    );
    push(
        6,
        "gen-tlut-ci4-palette-bank-1".into(),
        "SAME CI4 image and TLUT but tile palette field = bank 1: selects 16 \
         DIFFERENT entries despite identical low-TMEM bytes. EXPECTED \
         shared-ported-bug (#20); bank-select exonerated if delta is consistent.",
        ci4_palette_bank_textured_triangle(1),
    );
    push(
        6,
        "gen-tlut-ci8-full-range-ramp".into(),
        "CI8 textured triangle sampling 32 texels across a contiguous index \
         ramp crossing every high nibble 0x0..0x7. EXPECTED shared-ported-bug (#20).",
        ci8_full_range_textured_triangle(),
    );
    push(
        6,
        "gen-tlut-type-rgba16".into(),
        "tlut_type = G_TT_RGBA16 on a CI8 textured triangle: entry decodes as \
         RGBA5551. Paired with gen-tlut-type-ia16. EXPECTED shared-ported-bug (#20).",
        tlut_type_textured_triangle(2),
    );
    push(
        6,
        "gen-tlut-type-ia16".into(),
        "IA16 twin of gen-tlut-type-rgba16: same bytes, only tlut_type differs; \
         entries must decode as [hi,hi,hi,lo]. EXPECTED shared-ported-bug (#20).",
        tlut_type_textured_triangle(3),
    );
    push(
        6,
        "gen-tlut-loadtlut-nonzero-origin".into(),
        "LoadTlut whose SetTextureImage address points 40 texels into its source \
         array; entries before the offset are a marker. EXPECTED shared-ported-bug (#20).",
        palette_origin_textured_triangle(),
    );

    // slice lod-mip
    push(
        3,
        "gen-lod-texture-lod-en-single-tile".into(),
        "texture_lod_en set on a one-cycle textured triangle with ONE tile. \
         EXPECTED wgpu-refused/divergent: wgpu never calls compute_lod.",
        one_tile_textured_triangle(OTHER_MODES_ONE_CYCLE_TEXTURED_LOD),
    );
    push(
        3,
        "gen-lod-two-tile-mip-chain".into(),
        "texture_lod_en on, TWO tile descriptors resident. EXPECTED \
         wgpu-refused/divergent: RT64 picks between tiles, wgpu samples tile 0.",
        two_level_textured_triangle(OTHER_MODES_ONE_CYCLE_TEXTURED_LOD),
    );
    push(
        3,
        "gen-lod-two-tile-mip-chain-disabled".into(),
        "Control twin: identical staging, texture_lod_en OFF (G_TL_TILE). \
         EXPECTED pass-all-match -- a second tile alone is not the defect.",
        two_level_textured_triangle(OTHER_MODES_ONE_CYCLE_TEXTURED),
    );
    push(
        4,
        "gen-lod-fraction-combiner-disabled".into(),
        "LOD_FRACTION as two-cycle combiner output, texture_lod_en OFF: \
         lodFraction=1.0 unconditionally. EXPECTED pass-all-match (control).",
        lod_fraction_combiner_case(OTHER_MODES_TWO_CYCLE_TEXTURED),
    );
    push(
        3,
        "gen-lod-fraction-combiner-enabled".into(),
        "LOD_FRACTION combiner output with texture_lod_en ON: a real derivative \
         function. EXPECTED wgpu-refused/divergent (compute_lod unwired).",
        lod_fraction_combiner_case(OTHER_MODES_TWO_CYCLE_TEXTURED_LOD),
    );
    push(
        4,
        "gen-lod-detail-sharpen-two-tile".into(),
        "texture_detail = G_TD_SHARPEN on the two-tile LOD case. EXPECTED \
         wgpu-refused/divergent: exercises computeLOD's lodSharpen sub-path.",
        two_level_textured_triangle(with_texture_detail(OTHER_MODES_ONE_CYCLE_TEXTURED_LOD, 1)),
    );

    // slice zmode-deep
    push(
        6,
        "gen-zbuffer-zmode-inter-tie".into(),
        "ZMODE_INTER, tied Z on both draws: hardware's nearer relation passes \
         the tie (red wins), fn64's strict in_front rejects it (green). \
         EXPECTED wgpu diverges from angrylion==RT64.",
        gen_zbuffer_zmode_inter_tie(),
    );
    push(
        6,
        "gen-zbuffer-zmode-translucent-control".into(),
        "ZMODE_XLU control: mode_passes(Translucent,_) IS the in_front relation \
         fn64 always uses. EXPECTED pass-all-match.",
        gen_zbuffer_zmode_translucent_control(),
    );
    push(
        6,
        "gen-zbuffer-zmode-decal-correlated-wins".into(),
        "ZMODE_DECAL, second draw coplanar (same Z): hardware passes the decal \
         (red wins), fn64's strict compare rejects it (green). EXPECTED wgpu \
         diverges from angrylion==RT64.",
        gen_zbuffer_zmode_decal_correlated_wins(),
    );
    push(
        6,
        "gen-zbuffer-zmode-decal-uncorrelated-control".into(),
        "ZMODE_DECAL alone over a zero z-image at large Z: both relations reject. \
         EXPECTED pass-all-match; isolates the correlated-acceptance path.",
        gen_zbuffer_zmode_decal_uncorrelated_control(),
    );
    push(
        6,
        "gen-zbuffer-primdepth-three-tier-chain".into(),
        "G_ZS_PRIM through 3 SetPrimDepth draws (far, mid, near), ZMODE_OPAQUE: \
         a strictly-decreasing chain can't distinguish nearer from in_front. \
         EXPECTED pass-all-match.",
        gen_zbuffer_primdepth_three_tier_chain(),
    );
    push(
        6,
        "gen-zbuffer-pixel-zplane-gradient-split".into(),
        "G_ZS_PIXEL raw triangle with nonzero dzdx overlapped by a flat \
         G_ZS_PRIM triangle at a straddling Z: hardware shows a two-region \
         split, fn64 flattens the gradient. EXPECTED wgpu diverges from angrylion==RT64.",
        gen_zbuffer_pixel_zplane_gradient_split(),
    );

    // slice formats-wider
    push_formats_wider_cases(&mut push);

    cases
}

/// The triage classification for one generated case, per the brief's rubric.
pub(crate) fn triage(
    angrylion: &Result<Vec<u8>, String>,
    wgpu: &Result<Vec<u8>, String>,
    rt64: &Result<Vec<u8>, String>,
) -> &'static str {
    if angrylion_is_skipped(angrylion) {
        return "angrylion-skipped-fallback-wgpu-vs-rt64";
    }
    let (a, w, r) = match (angrylion, wgpu, rt64) {
        (Ok(a), Ok(w), Ok(r)) => (pixels(a), pixels(w), pixels(r)),
        // A backend refused: not a pixel verdict. Name which lane failed.
        _ => {
            return match (angrylion.is_ok(), wgpu.is_ok(), rt64.is_ok()) {
                (false, _, _) => "angrylion-error",
                (_, false, _) => "wgpu-refused",
                (_, _, false) => "rt64-refused",
                _ => "unknown-refusal",
            };
        }
    };
    let wgpu_ok = w == a;
    let rt64_ok = r == a;
    match (wgpu_ok, rt64_ok) {
        (true, true) => "pass-all-match-hardware",
        (true, false) => "rt64-hle-defect", // wgpu matches truth, RT64 diverges
        (false, true) => "fn64-defect",     // fn64 wrong, RT64 matches truth
        (false, false) => {
            if w == r {
                "shared-ported-bug" // both ported engines share a bug angrylion exposes
            } else {
                "all-three-differ-inspect-construction"
            }
        }
    }
}

/// First differing pixel between two readings, as a JSON object or null.
pub(crate) fn first_diff(a: &Result<Vec<u8>, String>, b: &Result<Vec<u8>, String>) -> Value {
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let (a, b) = (pixels(a), pixels(b));
            (0..PIXEL_COUNT as usize)
                .find(|&i| a[i] != b[i])
                .map(|i| {
                    json!({
                        "pixel": i,
                        "x": i as u32 % WIDTH,
                        "y": i as u32 / WIDTH,
                        "angrylion": format!("{:#06x}", a[i]),
                        "other": format!("{:#06x}", b[i]),
                    })
                })
                .unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// Count of differing pixels between two readings, or null if either refused.
pub(crate) fn diff_count(a: &Result<Vec<u8>, String>, b: &Result<Vec<u8>, String>) -> Value {
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let (a, b) = (pixels(a), pixels(b));
            json!((0..PIXEL_COUNT as usize).filter(|&i| a[i] != b[i]).count())
        }
        _ => Value::Null,
    }
}
