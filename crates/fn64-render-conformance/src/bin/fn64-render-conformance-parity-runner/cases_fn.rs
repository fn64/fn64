//! The hand corpus collector.

use super::*;

/// The corpus.
///
/// Every key is stated as arithmetic over the case's own display list under
/// the fill-cycle rule the reference runner documents: `G_FILLRECT` covers
/// `ceil(ulx) ..= floor(lrx)` INCLUSIVE on both edges.
///
/// **Provenance: every case here is hand-authored.** None is captured from a
/// running ROM. `docs/rt64/RT64-PARITY.md` states what that costs the metric.
pub(crate) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "full-target-red",
            intent: "the degenerate case: one fill covering the whole target. \
                     A disagreement here is a broken backend, not a subtlety.",
            authority: Authority::Rt64Authoritative,
            commands: one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1),
            expected: |_| RED,
        },
        Case {
            name: "right-half-blue-over-red",
            intent: "two fills, the second overlapping the right half. Tests \
                     command ORDER: a backend that reordered or merged them \
                     paints the whole target one colour.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                words.pop();
                words.push((0xf700_0000, (BLUE as u32) * 0x1_0001));
                words.push(fill_rect(WIDTH - 1, HEIGHT - 1, WIDTH / 2, 0));
                words.push((0xe900_0000, 0));
                words
            },
            expected: |index| {
                if index % WIDTH < WIDTH / 2 {
                    RED
                } else {
                    BLUE
                }
            },
        },
        Case {
            name: "top-left-quadrant",
            intent: "a partial fill. Both edges inclusive, so columns 0..=159 \
                     and rows 0..=119 are covered and the rest keeps the \
                     seeded bytes. A backend that cannot express partial \
                     target initialisation shows it here.",
            authority: Authority::Rt64Authoritative,
            commands: one_fill(RED, 0, 0, WIDTH / 2 - 1, HEIGHT / 2 - 1),
            expected: |index| {
                if index % WIDTH < WIDTH / 2 && index / WIDTH < HEIGHT / 2 {
                    RED
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "single-pixel",
            intent: "`ulx == lrx` is ONE column wide under the inclusive rule, \
                     not zero. This is the case a half-open reading drops \
                     entirely.",
            authority: Authority::Rt64Authoritative,
            commands: one_fill(BLUE, 17, 9, 17, 9),
            expected: |index| {
                if index == 9 * WIDTH + 17 {
                    BLUE
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "last-column-last-row",
            intent: "where an off-by-one in either direction shows up first.",
            authority: Authority::Rt64Authoritative,
            commands: one_fill(RED, WIDTH - 1, HEIGHT - 1, WIDTH - 1, HEIGHT - 1),
            expected: |index| {
                if index == PIXEL_COUNT - 1 {
                    RED
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "even-color-lsb-clear",
            intent: "a colour whose LSB is CLEAR. RED/BLUE/GREEN all have \
                     theirs set, which makes their 5->8->5 round trip exact. \
                     This distinguishes a backend that preserves the wire \
                     value from one that round-trips through 8 bits.",
            authority: Authority::Rt64Authoritative,
            commands: one_fill(0xf800, 0, 0, WIDTH - 1, HEIGHT - 1),
            expected: |_| 0xf800,
        },
        Case {
            name: "nested-second-fill",
            intent: "the second fill is fully CONTAINED in the first. Order \
                     matters and containment is where a merge optimisation \
                     would hide.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                words.pop();
                words.push((0xf700_0000, (BLUE as u32) * 0x1_0001));
                words.push(fill_rect(WIDTH - 2, HEIGHT - 2, 1, 1));
                words.push((0xe900_0000, 0));
                words
            },
            expected: |index| {
                let (x, y) = (index % WIDTH, index / WIDTH);
                if (1..=WIDTH - 2).contains(&x) && (1..=HEIGHT - 2).contains(&y) {
                    BLUE
                } else {
                    RED
                }
            },
        },
        Case {
            name: "scissor-narrower-than-rect",
            intent: "the scissor admits only the left half while the fill asks \
                     for the whole target. A backend that ignores the scissor \
                     paints the right half too. MEASURED: RT64 paints it -- \
                     all 38,400 excluded pixels -- and wgpu does not, so wgpu \
                     matches the key here. The scissor's own encoding was \
                     verified against angrylion's `rdp_set_scissor` first: \
                     the bounds split across BOTH words, and this corpus \
                     packed them into word 0 until that was measured, which \
                     made every scissor an inverted box. Correcting it did \
                     NOT change the outcome, so this is a real difference on \
                     a correctly encoded command and not a fixture defect.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                words[1] = set_scissor(0, 0, WIDTH / 2, HEIGHT);
                words
            },
            expected: |index| {
                if index % WIDTH < WIDTH / 2 {
                    RED
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "scissor-top-rows-only",
            intent: "the vertical counterpart of the scissor case. A backend \
                     that applied the scissor on X only would pass the \
                     previous case and fail this one.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(GREEN, 0, 0, WIDTH - 1, HEIGHT - 1);
                words[1] = set_scissor(0, 0, WIDTH, HEIGHT / 2);
                words
            },
            expected: |index| {
                if index / WIDTH < HEIGHT / 2 {
                    GREEN
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "three-fills-strict-order",
            intent: "three overlapping fills where only strict submission \
                     order yields the key. Any reordering produces a \
                     different picture, so this is an order probe that a \
                     two-fill case cannot be.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                words.pop();
                words.push((0xf700_0000, (GREEN as u32) * 0x1_0001));
                words.push(fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0));
                words.push((0xf700_0000, (BLUE as u32) * 0x1_0001));
                words.push(fill_rect(WIDTH / 2 - 1, HEIGHT - 1, 0, 0));
                words.push((0xe900_0000, 0));
                words
            },
            // RED is fully overpainted by GREEN, then the left half by BLUE.
            expected: |index| {
                if index % WIDTH < WIDTH / 2 {
                    BLUE
                } else {
                    GREEN
                }
            },
        },
        // ------------------------------------------------------------------
        // Textured cases. RT64 IS the oracle for texture behaviour
        // (`docs/rt64/RT64-PARITY.md` section 2), so these stay in partition A.
        // ------------------------------------------------------------------
        Case {
            name: "textured-rect-point-sampled",
            intent: "the corpus's first textured case, and the reason the \
                     rest of this block exists. A 4x2 RGBA16 tile is loaded \
                     with LoadTile and drawn one texel per pixel, so pixel \
                     (x, y) must be texel (x, y) exactly. This is the case \
                     that can see a wrong TMEM address, a wrong tile line, a \
                     swapped 4-byte bank, or a wrong byte lane -- none of \
                     which any fill-rectangle case can reach.",
            authority: Authority::Rt64Authoritative,
            commands: one_textured_rect(),
            expected: textured_expected,
        },
        Case {
            name: "textured-rect-second-row-only",
            intent: "the same tile drawn one row DOWN, so every pixel reads \
                     TMEM row 1 -- the row that carries the odd-row XOR4 bank \
                     exchange. A reader and writer that disagree about that \
                     exchange return the right texel's neighbour four bytes \
                     away, which is wrong colour at correct coordinates: \
                     exactly the signature in RT64-WM2000-TEXTURE-STATE.md. \
                     The first case above cannot see it, because its row 0 \
                     never exchanges.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_textured_rect();
                // Move the rectangle's T origin one texel down. The texrect
                // words are the last two before FullSync; word 1 of the pair
                // carries the S/T origin in S10.5, so one texel is `1 << 5`.
                let texrect_s_t = words.len() - 2;
                words[texrect_s_t].0 = 1 << 5;
                words
            },
            // Every pixel row now reads TMEM row 1. The rectangle is still
            // two rows tall, but the tile clamps at its own last row
            // (`mask_t == 0` forces the clamp arm), so both target rows read
            // TMEM row 1.
            expected: |index| {
                let x = index % WIDTH;
                let y = index / WIDTH;
                if x < TEXRECT_LRX && y < TEXRECT_LRY {
                    TEXTURE_TEXELS[(TEXTURE_WIDTH + x) as usize]
                } else {
                    STALE
                }
            },
        },
        Case {
            name: "textured-rect-ci4-tlut",
            intent: "the first COLOUR-INDEXED case. Every textured case above \
                     is direct-colour RGBA16, where the texel bytes ARE the \
                     colour. CI4 is a different path: the tile holds 4-bit \
                     indices, a palette is loaded separately into HIGH TMEM \
                     by LoadTlut, and other-modes `en_tlut` switches the \
                     sampler onto the lookup. RT64-WM2000-TEXTURE-STATE.md \
                     names the palette as a suspect it could not rule out for \
                     the blocky glyphs. The indices are a non-identity \
                     permutation, so a sampler that returned the index \
                     itself, or palette entry x for pixel x, is visible.",
            authority: Authority::Rt64Authoritative,
            commands: one_ci4_rect(),
            expected: ci_expected,
        },
        Case {
            name: "textured-rect-rgba32",
            intent: "two opaque RGBA32 texels exercise one complete split-bank TMEM \
                     layout and 8/8/8/8 channel decode. The seed is 0xffff, \
                     which no authored texel can quantize to, and each \
                     RGBA16 key word is packed directly from its wire bytes.",
            authority: Authority::Rt64Authoritative,
            commands: one_rgba32_rect(),
            expected: rgba32_expected,
        },
        Case {
            name: "textured-rect-ci8-tlut",
            intent: "eight sparse full-byte indices exercise CI8 addressing \
                     and all 256 high-TMEM palette entries. The index set \
                     crosses every nibble range, so truncating CI8 to CI4 or \
                     selecting palette entry x cannot match the key.",
            authority: Authority::Rt64Authoritative,
            commands: one_ci8_rect(),
            expected: ci8_expected,
        },
        Case {
            name: "textured-rect-yuv16",
            intent: "four even-S YUV16 pairs exercise the only legal YUV \
                     cell and its Y0,U,Y1,V wire layout. Neutral chroma \
                     reduces the public first-stage equations to gray=Y, \
                     selected by the explicit Texel0-pass combiner without \
                     borrowing an answer from either renderer.",
            authority: Authority::Rt64Authoritative,
            commands: one_yuv16_rect(),
            expected: yuv16_expected,
        },
        Case {
            name: "textured-rect-ia8",
            intent: "an opaque IA8 row exercises the ROM's most-used missing \
                     direct format: each byte must split into a high intensity \
                     nibble and low alpha nibble. A disagreement means the \
                     tile format/size dispatch, byte address, or IA8 channel \
                     expansion differs from RT64; treating each byte as I8 \
                     cannot reproduce this hand-derived key.",
            authority: Authority::Rt64Authoritative,
            commands: one_direct_texture_rect(IA8_SOURCE, 8, 4, 3, 1, 1),
            expected: ia8_expected,
        },
        Case {
            name: "textured-rect-ia4",
            intent: "a packed IA4 row exercises the other format measured \
                     heavily in the decoded frame: high-nibble-first TMEM \
                     addressing followed by a 3-bit intensity/1-bit alpha \
                     split. A disagreement identifies packed-nibble address \
                     selection or IA4 expansion, not filtering.",
            authority: Authority::Rt64Authoritative,
            commands: one_direct_texture_rect(IA4_SOURCE, 7, 2, 3, 0, 1),
            expected: ia4_expected,
        },
        Case {
            name: "textured-rect-ia16",
            intent: "an IA16 row names separate big-endian intensity and alpha \
                     bytes per texel across a two-word TMEM stride. A \
                     disagreement means the 16-bit direct decoder, byte \
                     order, or line=2 address calculation differs from RT64.",
            authority: Authority::Rt64Authoritative,
            commands: one_direct_texture_rect(IA16_SOURCE, 8, 8, 3, 2, 2),
            expected: ia16_expected,
        },
        Case {
            name: "textured-rect-i4",
            intent: "a packed I4 row verifies that each high-nibble-first \
                     intensity value feeds RGB and alpha together. A \
                     disagreement means packed TMEM addressing or the I4 \
                     replication path differs from RT64.",
            authority: Authority::Rt64Authoritative,
            commands: one_direct_texture_rect(I4_SOURCE, 8, 2, 4, 0, 1),
            expected: i4_expected,
        },
        Case {
            name: "textured-rect-i8",
            intent: "an I8 row verifies one-byte TMEM addressing and the \
                     intensity-to-RGBA replication path across successive \
                     five-bit quantization steps. A disagreement means \
                     I8 was decoded as another direct format or addressed at \
                     the wrong byte.",
            authority: Authority::Rt64Authoritative,
            commands: one_direct_texture_rect(I8_SOURCE, 8, 4, 4, 1, 1),
            expected: i8_expected,
        },
        Case {
            name: "textured-rect-loadblock-linear",
            intent: "the corpus's first LoadBlock. DXT=0 loads two consecutive \
                     64-bit words into TMEM words 0 and 1, then an 8x1 \
                     point-sampled rectangle reads all eight distinct RGBA16 \
                     texels. This isolates opcode 0x33's linear placement \
                     from LoadTile's per-row addressing.",
            authority: Authority::Rt64Authoritative,
            commands: load_block_textured_rect(8, 0, 2, 8, 1, 2),
            expected: load_block_linear_expected,
        },
        Case {
            name: "textured-rect-loadblock-dxt-row-advance",
            intent: "LoadBlock with DXT=0x400 crosses the 0x800 accumulator \
                     after word 1. Loading line=2 therefore maps four source \
                     words to TMEM 0,1,4,5; render line=4 reads them as two \
                     rows and exposes both the DXT stride and odd-row \
                     four-byte exchange.",
            authority: Authority::Rt64Authoritative,
            commands: load_block_textured_rect(16, 0x400, 2, 8, 2, 4),
            expected: load_block_dxt_expected,
        },
        Case {
            name: "textured-rect-flip-point-sampled",
            intent: "opcode 0x25 keeps a 4x4 rectangle's destination fixed \
                     while transposing its S/T sample axes. Every source \
                     texel is distinct, so treating TEXRECTFLIP as ordinary \
                     TEXRECT produces a different hand-derived 4x4 key.",
            authority: Authority::Rt64Authoritative,
            commands: one_textured_rect_flip(),
            expected: texrect_flip_expected,
        },
        Case {
            name: "flat-triangle-primitive",
            intent: "the first opcode 0x08 triangle, with no shade, texture \
                     or depth coefficients. Two explicit edge pairs tile the \
                     same 4x3 box as the textured control, while a public \
                     G_CC_PRIMITIVE combiner makes every covered pixel one \
                     hand-derived RGBA16 value. This isolates base edge-walk \
                     and coverage from the texture pipeline.",
            authority: Authority::Rt64Authoritative,
            commands: one_flat_triangle_pair(),
            expected: flat_triangle_expected,
        },
        Case {
            name: "shade-only-triangle",
            intent: "the first opcode 0x0c triangle (`G_RDPTRI_BASE | \
                     Shaded`), carrying an 8-word shade coefficient block and \
                     no texture coefficients. A public G_CC_SHADE combiner \
                     makes every covered pixel the shade colour, isolating \
                     the shade-interpolation path from both the base \
                     edge-walk (`flat-triangle-primitive`, opcode 0x08) and \
                     the texture pipeline (`textured-triangle-point-sampled`, \
                     opcode 0x0e). Every shade derivative is zero, so the \
                     covered rectangle is one flat colour and the key stays \
                     arithmetic rather than a per-pixel interpolation.",
            authority: Authority::Rt64Authoritative,
            commands: one_shade_triangle_pair(),
            expected: shade_triangle_expected,
        },
        Case {
            name: "perspective-textured-triangle-negative-w",
            intent: "a perspective raw triangle with constant Q16.16 planes \
                     [S,T,W] = [65536,0,-262144]. Signed RT64 division gives \
                     S=-256 texels, which point sampling floors and the \
                     explicit four-texel clamp maps to the FIRST texel. The \
                     old |W| divide gives +256 and maps to the LAST texel, so \
                     this row kills the confirmed sign-loss defect.",
            authority: Authority::Rt64Authoritative,
            commands: one_negative_w_textured_triangle(),
            expected: negative_w_triangle_expected,
        },
        Case {
            name: "textured-triangle-point-sampled",
            intent: "the first RAW TRIANGLE in the corpus, and the first case \
                     on the path WM2000 actually draws through. Every case \
                     above uses TextureRectangle; a triangle carries its own \
                     coefficient decode, plane evaluation and span walk, none \
                     of which a texrect can reach. Vertical-sided so the \
                     covered set is exactly a rectangle and the key stays \
                     arithmetic -- this measures the TEXTURE path, not edge \
                     walking. S advances one texel per pixel of X and T is \
                     constant, so the three covered rows are three \
                     independent readings of the same claim.",
            authority: Authority::Rt64Authoritative,
            commands: one_textured_triangle(),
            expected: triangle_expected,
        },
        Case {
            name: "textured-rect-wide-line-two",
            intent: "the first case with a tile `line` other than 1. An 8x2 \
                     RGBA16 texture puts TWO 64-bit words in each TMEM row, \
                     so the row stride is `line * t` rather than just `t` -- \
                     angrylion's own `tile->line * (t & 0xff)`. A wrong \
                     multiplier is INVISIBLE at line 1 (any multiplier times \
                     row 0 is still row 0), which is exactly why every case \
                     above can be green while a stride defect ships. Row 1 \
                     carries a bit no row-0 texel has, so reading the wrong \
                     row shows up even if the columns coincide.",
            authority: Authority::Rt64Authoritative,
            commands: wide_textured_rect(),
            expected: wide_expected,
        },
        Case {
            name: "textured-rect-line17-low-t95",
            intent: "the measured WM2000 texrect state reduced to a synthetic \
                     64x14 RGBA16 bar: LoadTile, tmem 0, line 17 and odd \
                     low_t 95. Every source row has identical red extents, \
                     so a two-texel XOR4 displacement appears directly as a \
                     shifted red edge and fourteen rows expose any cumulative \
                     two-pixel-per-row skew.",
            authority: Authority::Rt64Authoritative,
            commands: skew_textured_rect(SKEW_LINE_WORDS, SKEW_LOW_T_ODD),
            expected: skew_expected,
        },
        Case {
            name: "textured-rect-line17-low-t94",
            intent: "one-variable control for textured-rect-line17-low-t95. \
                     Only the SetTileSize, LoadTile and texrect T origins \
                     change from odd low_t 95 to even low_t 94; line 17, \
                     LoadTile, RGBA16, tmem 0, source pixels and 64x14 draw \
                     geometry stay fixed.",
            authority: Authority::Rt64Authoritative,
            commands: skew_textured_rect(SKEW_LINE_WORDS, SKEW_LOW_T_ODD - 1),
            expected: skew_expected,
        },
        Case {
            name: "textured-rect-line16-low-t95",
            intent: "one-variable control for textured-rect-line17-low-t95. \
                     Only SetTile's line field changes from the measured 17 \
                     words to the tightly packed 16 words occupied by each \
                     64-texel RGBA16 source row; odd low_t 95, LoadTile, \
                     tmem 0, source pixels and 64x14 draw geometry stay fixed.",
            authority: Authority::Rt64Authoritative,
            commands: skew_textured_rect(SKEW_LINE_WORDS - 1, SKEW_LOW_T_ODD),
            expected: skew_expected,
        },
        Case {
            name: "one-cycle-fill-band",
            intent: "a G_FILLRECT band issued in ONE-CYCLE mode over a \
                     STALE-seeded target. WM2000 clears its framebuffer with \
                     ~60 such bands per frame; dropping those writes leaves the \
                     stale framebuffer at VI -- the measured cause of \
                     the foreign content on the AKI/THQ/JAKKS/Asmik logo \
                     screens. RT64 calls drawRect unconditionally \
                     (rt64_rdp.cpp:1043). The key requires the measured white \
                     combiner result across exactly the exclusive 319x63 \
                     extent, with the distinct seed surviving outside it.",
            authority: Authority::Rt64Authoritative,
            commands: one_cycle_fill_band(),
            expected: one_cycle_fill_band_expected,
        },
        Case {
            name: "blend-numerator-overflow-wrap",
            intent: "opaque white enters both P and M with A = B = 1, so each \
                     general-path RGB numerator is 2. RT64 wraps it modulo \
                     1 + 8/255 before dividing by 2; clamp-before-divide or \
                     divide-without-wrap produces white instead of the \
                     hand-derived 0x7bdf. The untouched target is blue, a \
                     colour this all-white blend program cannot produce.",
            authority: Authority::Rt64Authoritative,
            commands: blend_numerator_overflow_rect(),
            expected: blend_numerator_overflow_expected,
        },
        Case {
            name: "blend-color-blender-passthrough",
            intent: "SetBlendColor supplies the forced blender's P input while \
                     an opaque red primitive supplies only the alpha factor. \
                     The covered 4x2 texrect must therefore resolve to the \
                     distinct blue-purple blend colour, proving opcode 0xf9 \
                     reaches the blender rather than the combiner.",
            authority: Authority::Rt64Authoritative,
            commands: state_color_blender_rect(
                (0xf900_0000, 0x4080_c0ff),
                OTHER_MODES_ONE_CYCLE_BLEND_COLOR,
            ),
            expected: blend_color_expected,
        },
        Case {
            name: "fog-color-blender",
            intent: "SetFogColor supplies the forced blender's P input while \
                     the same opaque primitive and zero memory factor isolate \
                     that selector. The covered 4x2 texrect must resolve to \
                     the distinct fog colour, proving opcode 0xf8 reaches \
                     the blender without relying on an impossible combiner \
                     FogColor input.",
            authority: Authority::Rt64Authoritative,
            commands: state_color_blender_rect(
                (0xf800_0000, 0x2060_a0ff),
                OTHER_MODES_ONE_CYCLE_FOG_COLOR,
            ),
            expected: fog_color_expected,
        },
        Case {
            name: "two-cycle-textured",
            intent: "the point-sampled 4x2 RGBA16 control with only cycle type \
                     changed to G_CYC_2CYCLE. Both combiner cycles select \
                     Texel0 passthrough, so every covered pixel must remain \
                     identical to textured-rect-point-sampled while exercising \
                     the previously absent two-cycle texture path.",
            authority: Authority::Rt64Authoritative,
            commands: two_cycle_textured_rect(),
            expected: textured_expected,
        },
        // ------------------------------------------------------------------
        // The partition boundary. Everything below exercises a stage RT64
        // does not model, so RT64's answer is NOT evidence about wgpu.
        // ------------------------------------------------------------------
        Case {
            name: "coverage-aa-enabled-fill",
            intent: "identical to full-target-red except AA_EN is SET in \
                     SetOtherModes. RT64 hardcodes memory alpha to 1.0f under \
                     'Coverage is not emulated' (rt64_blender.h:355-357) and \
                     routes AA_EN only to a debugger string, so RT64's answer \
                     here is not evidence about the hardware. Reported \
                     separately; angrylion is the authority.",
            authority: Authority::CoverageDependentRt64NotAuthoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                // AA_EN is bit 3 of the low half of SetOtherModes word 1.
                words[0] = (0xef30_00f8, 0);
                words
            },
            expected: |_| RED,
        },
        Case {
            name: "coverage-alpha-dither-enabled",
            intent: "alpha dither enabled. The guard audit's U2/U3 record that \
                     angrylion and RT64 apply dither at different stages with \
                     different arithmetic and the authority question is \
                     UNSETTLED, so neither reference can bless this. Counted \
                     in the non-authoritative partition.",
            authority: Authority::CoverageDependentRt64NotAuthoritative,
            commands: {
                let mut words = one_fill(BLUE, 0, 0, WIDTH - 1, HEIGHT - 1);
                // Select an RGB/alpha dither mode rather than the
                // no-dither encoding the other cases use.
                words[0] = (0xef30_00f0, 0x0000_0000);
                words[0].0 = 0xef20_00f0;
                words
            },
            expected: |_| BLUE,
        },
    ]
}
