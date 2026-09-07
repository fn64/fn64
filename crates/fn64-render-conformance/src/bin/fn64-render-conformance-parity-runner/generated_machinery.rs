//! Synthetic generated-corpus fixture builders (Track B).

use super::*;

/// One generated case: a name, a priority rank (1 = highest, do first), and a
/// valid RDP command stream. No hand key -- angrylion is the oracle.
pub(crate) struct GeneratedCase {
    pub(crate) name: String,
    /// Priority per the brief's real-ROM-usage order. Lower = render first.
    pub(crate) priority: u8,
    /// What matrix cell this case exercises, for the report.
    pub(crate) intent: &'static str,
    pub(crate) commands: Vec<(u32, u32)>,
}

/// A minimal complete fill frame painting `color` over the box
/// `[ulx,lrx) x [uly,lry)` in the requested `cycle_type` (0=1cyc, 1=2cyc,
/// 2=copy, 3=fill), over a STALE background. Fill cycle uses SetFillColor;
/// the non-fill cycle types drive the same rectangle through the pixel pipe
/// with a primitive-colour combiner so the mode-matrix cell is exercised end
/// to end rather than short-circuited by the fill path.
pub(crate) fn gen_fill_frame(
    color: u16,
    cycle_type: u32,
    ulx: u32,
    uly: u32,
    lrx: u32,
    lry: u32,
) -> Vec<(u32, u32)> {
    // SetOtherModes: base no-AA/no-dither word with the cycle-type field set.
    // Bits 21:20 carry cycle type; the fill constant is 0xef30_00f0 (cycle=3).
    let other_modes = (0xef00_00f0 | (cycle_type << 20), 0u32);
    if cycle_type == 3 {
        // True fill path: SetFillColor + FillRectangle. `fill_rect`'s
        // lower-right is INCLUSIVE (the hand corpus passes WIDTH-1/HEIGHT-1
        // for a full-target fill), while this function takes an EXCLUSIVE
        // `lrx`/`lry`, so convert. A rectangle whose inclusive lower-right
        // reached the exclusive extent would exceed the staged color-image
        // width and every backend that validates the extent refuses it.
        let (incl_lrx, incl_lry) = (lrx.saturating_sub(1), lry.saturating_sub(1));
        return vec![
            other_modes,
            set_scissor(0, 0, WIDTH, HEIGHT),
            (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
            (0xf700_0000, (color as u32) * 0x1_0001),
            fill_rect(incl_lrx, incl_lry, ulx, uly),
            (0xe900_0000, 0),
        ];
    }
    // Non-fill: paint the rectangle through the pixel pipe with a primitive
    // colour selected straight through the combiner. FillRectangle is only
    // legal in fill/copy; a pixel-pipe rectangle is a TextureRectangle with a
    // combiner that ignores the texel. We keep it simple with a flat triangle
    // pair covering the box and a primitive-colour combiner (the same shape
    // `one_flat_triangle_pair` proves renders on all three backends).
    let mut words = vec![
        other_modes,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888(color)),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ];
    // Seed the background first so uncovered pixels are STALE, not zero.
    let mut seeded_bg = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    seeded_bg.pop(); // drop its SyncFull; ours closes the frame
    let mut frame = seeded_bg;
    frame.extend(words.drain(..));
    frame.extend(flat_triangle_words(ulx, lrx, uly, lry, lry));
    frame.extend(flat_triangle_words(lrx, ulx, uly, lry, uly));
    frame.push((0xe900_0000, 0));
    frame
}

/// Approximate an RGBA16 colour as the RGBA8888 word a primitive-colour
/// combiner needs so the pixel pipe reproduces it. 5-bit channels are
/// left-justified into 8 bits; alpha is forced opaque.
pub(crate) fn primitive_rgba8888(color: u16) -> u32 {
    let r5 = ((color >> 11) & 0x1f) as u32;
    let g5 = ((color >> 6) & 0x1f) as u32;
    let b5 = ((color >> 1) & 0x1f) as u32;
    let expand = |c: u32| (c << 3) | (c >> 2);
    (expand(r5) << 24) | (expand(g5) << 16) | (expand(b5) << 8) | 0xff
}

/// Set `BI_LERP_0` (SetOtherModes word0 bit 11) on every one-cycle textured
/// SetOtherModes in the stream.
///
/// **This corrects a corpus-wide fixture gap the angrylion leg surfaced.** The
/// hand corpus's `OTHER_MODES_ONE_CYCLE_TEXTURED = 0xef0000f0` leaves bit 11
/// clear. Bit-accurate hardware (angrylion) then routes an RGBA texel through
/// the colour-convert/YUV unit — with zero SetConvert coefficients that
/// collapses every channel to the texel's blue channel (grayscale). wgpu and
/// RT64 both ignore the missing bit and pass the full RGBA texel through, so
/// they agree with the hand key yet diverge from hardware. IA/I textures are
/// unaffected because their value already lives in the blue channel. Proven by
/// instrumenting angrylion: setting bit 11 makes all three backends agree.
///
/// SetOtherModes is opcode `0xef` in word0's top byte; bit 11 is the mode-word
/// `bi_lerp0`. The fill/copy other-modes (cycle-type 3/2) are left untouched —
/// bilerp is meaningless there — by only touching one-cycle/two-cycle words.
pub(crate) fn set_bilerp0(mut stream: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    for (word0, _) in stream.iter_mut() {
        if *word0 >> 24 == 0xef {
            let cycle_type = (*word0 >> 20) & 0x3;
            if cycle_type == 0 || cycle_type == 1 {
                *word0 |= 1 << 11;
            }
        }
    }
    stream
}

/// A raw triangle of the given opcode (0x08..=0x0f) covering the standard TRI
/// box with a primitive-colour combiner. Only the opcode's feature bits
/// (shade/texture/zbuffer) differ; the geometry is the flat pair. Texture and
/// zbuffer variants still emit valid coefficient blocks so the command is
/// well-formed even where the combiner ignores them.
pub(crate) fn gen_triangle_variant(opcode: u32) -> Vec<(u32, u32)> {
    // Feature bits in the opcode low nibble: bit0=shade? Actually the RDP
    // triangle opcodes are 0x08 base | 0x04 shade | 0x02 texture | 0x01 zbuf.
    let shade = opcode & 0x04 != 0;
    let texture = opcode & 0x02 != 0;
    let _zbuf = opcode & 0x01 != 0;
    if texture {
        // A textured triangle needs a loaded tile; reuse the proven textured
        // triangle builder, which emits the S/T/W coefficient block. Correct
        // its missing BI_LERP_0 so angrylion samples the full RGBA texel.
        return set_bilerp0(one_textured_triangle());
    }
    if shade {
        return one_shade_triangle_pair();
    }
    // Flat, zbuffer-or-not: the flat pair. The zbuffer bit adds a Z
    // coefficient block on hardware; a flat non-shaded triangle with the bit
    // set but no depth image is still a valid command to compare.
    one_flat_triangle_pair()
}

/// Insert a sync opcode into an otherwise-valid fill frame at the position a
/// ROM would emit it, to check sync handling does not perturb the raster.
pub(crate) fn gen_fill_with_sync(sync_opcode: u32) -> Vec<(u32, u32)> {
    let mut words = gen_fill_frame(0xf801, 3, 0, 0, 64, 48);
    // Insert the sync just before the draw (index 4: after SetFillColor).
    words.insert(4, (sync_opcode << 24, 0));
    words
}

// =============================================================================
// Track-B fan-out pass 1: designed slices (blend-modes, alpha-compare,
// coverage-modes, formats-deep, zbuffer, loadblock-deep) integrated into the
// generator corpus below. Each slice's builders precede `generated_cases()`;
// each slice's `push(...)` calls are inside it.
// =============================================================================

// -----------------------------------------------------------------------
// slice blend-modes
// -----------------------------------------------------------------------
//
// Mode matrix -- BLENDER mode. One-cycle P/A/M/B selector matrix.
//
// GBI selector semantics (verified against this crate's own reference
// decoder, `fn64-render-reference/src/gbi/types.rs:511-525` and
// `raster/blend.rs:242-292`, which is itself sourced from public
// `ultra64/gbi.h:612-627`):
//
//   P / M (color, 2 bits): 0=Combined(clr_in) 1=Framebuffer(clr_mem)
//                          2=BlendColor       3=FogColor
//   A     (alpha, 2 bits): 0=CombinedAlpha 1=FogAlpha 2=ShadeAlpha 3=Zero
//   B     (alpha, 2 bits): 0=1-A 1=FramebufferCoverage/8 2=One 3=Zero
//
// `SetOtherModes` word1 (low) packs the ACTIVE cycle -- cycle 2's slot, which
// is what one-cycle mode evaluates -- at bits 31:30 (P), 29:28 (A), 27:26
// (M), 25:24 (B); confirmed against `OtherMode::blender_cycle_2` in the same
// file. `FORCE_BL` is bit 14 (`0x4000`); without it the last blend stage is
// bypassed and simply selects P. `IM_RD` (framebuffer-read enable) is word1
// bit 6 (`0x0040`) and must be set whenever P, A, or M reads memory/coverage.

/// Pack a one-cycle blender word (SetOtherModes word1) from its four GBI
/// selectors plus FORCE_BL/IM_RD flags, per the bit table above.
pub(crate) const fn blend_other_modes(
    p: u32,
    a: u32,
    m: u32,
    b: u32,
    force_bl: bool,
    im_rd: bool,
) -> (u32, u32) {
    let mut low = (p << 30) | (a << 28) | (m << 26) | (b << 24);
    if force_bl {
        low |= 1 << 14;
    }
    if im_rd {
        low |= 1 << 6;
    }
    (0xef00_00f0, low)
}

/// A flat-shaded triangle pair covering the standard TRI box, drawn with a
/// primitive-colour combiner (`SET_COMBINE_PRIMITIVE`, opaque alpha) over a
/// distinct memory seed, under the given one-cycle blender word.
pub(crate) fn gen_blend_rect(
    memory_seed: u16,
    primitive_rgba8888: u32,
    blend_words: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        blend_words,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// The blend-color / fog-color twin of [`gen_blend_rect`] that also programs
/// `SetBlendColor`/`SetFogColor` before the draw.
pub(crate) fn gen_blend_rect_with_state_color(
    memory_seed: u16,
    primitive_rgba8888: u32,
    set_state_color: (u32, u32),
    blend_words: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_state_color,
        blend_words,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// The shade-driven twin of [`gen_blend_rect`]: a `SET_COMBINE_SHADE`
/// triangle pair with a non-opaque, non-zero flat shade alpha.
pub(crate) fn gen_blend_rect_shade_alpha(
    memory_seed: u16,
    shade_rgba: [i32; 4],
    blend_words: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        blend_words,
        SET_COMBINE_SHADE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(shade_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM, shade_rgba,
    ));
    words.extend(shade_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP, shade_rgba,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const BLEND_MATRIX_MEMORY_SEED: u16 = GREEN;
pub(crate) const BLEND_MATRIX_PRIMITIVE_RGBA8888: u32 = 0xff00_00ff; // opaque red
pub(crate) const BLEND_MATRIX_SHADE_RGBA: [i32; 4] =
    [0x80 << 16, 0x7f << 16, 0x00 << 16, 0x80 << 16];

// -----------------------------------------------------------------------
// slice alpha-compare
// -----------------------------------------------------------------------
//
// Alpha compare matrix -- alpha_compare_en / dither_alpha, threshold vs
// dither compare mode, plus a disabled-bits control.
//
// `gDPSetOtherMode`'s low mode word carries `G_MDSFT_ALPHACOMPARE` at bits
// 1:0 (`ultra64/gbi.h`): `G_AC_NONE = 0`, `G_AC_THRESHOLD = 1` (compare
// combined alpha against `SetBlendColor`'s alpha byte), `G_AC_DITHER = 3`
// (compare against per-pixel noise in [0,255]; value 2 is reserved).
//
// `SetBlendColor` is opcode `0xf9`; its low byte is the alpha channel the
// compare tests against. Every case runs `SET_COMBINE_PRIMITIVE`, so
// `SetPrimColor`'s low byte is the alpha the compare unit evaluates.

/// One-cycle `SetOtherModes` with `alpha_compare_en` set to `compare_mode`
/// (0=disabled, 1=threshold, 3=dither) in word 1 bits 1:0.
pub(crate) const fn other_modes_alpha_compare(compare_mode: u32) -> (u32, u32) {
    (OTHER_MODES_ONE_CYCLE_NO_AA.0, compare_mode & 0x3)
}

/// A flat-triangle-pair rectangle painted with a primitive colour of alpha
/// `prim_alpha`, under alpha-compare mode `compare_mode` against
/// `SetBlendColor`'s alpha byte `blend_alpha`, over a STALE background.
pub(crate) fn gen_alpha_compare_rect(
    compare_mode: u32,
    prim_alpha: u8,
    blend_alpha: u8,
) -> Vec<(u32, u32)> {
    const ULX: u32 = 0;
    const ULY: u32 = 0;
    const LRX: u32 = 80;
    const LRY: u32 = 60;

    let mut seeded_bg = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    seeded_bg.pop();

    let mut frame = seeded_bg;
    frame.extend([
        other_modes_alpha_compare(compare_mode),
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, 0x20c0_e000 | prim_alpha as u32),
        (0xf900_0000, 0x0000_0000 | blend_alpha as u32),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    frame.extend(flat_triangle_words(ULX, LRX, ULY, LRY, LRY));
    frame.extend(flat_triangle_words(LRX, ULX, ULY, LRY, ULY));
    frame.push((0xe900_0000, 0));
    frame
}

// -----------------------------------------------------------------------
// slice coverage-modes
// -----------------------------------------------------------------------
//
// Coverage-modes matrix: cvg_dest x color_on_cvg x cvg_x_alpha x
// force_blend, across fill and one-cycle rects. All eight fields live in
// SetOtherModes' LOW word (word1), per public libultra `gbi.h`
// (`G_MDSFT_RENDERMODE` field group):
//
//   bit  3        AA_EN           0x8
//   bit  6        IM_RD           0x40
//   bit  7        CLR_ON_CVG      0x80
//   bits 9:8      CVG_DST select  0x000/0x100/0x200/0x300
//   bit  12       CVG_X_ALPHA     0x1000
//   bit  13       ALPHA_CVG_SEL   0x2000
//   bit  14       FORCE_BL        0x4000
//
// These are switches the RT64 guard audit names as unmodeled
// ("Coverage is not emulated" in `rt64_blender.h`). Every case here routes
// through angrylion as the oracle, so wgpu-vs-RT64 disagreement on these
// rows is evidence about RT64's own modelling gap, not a wgpu defect.

/// A fill-cycle full-target box with `word1` set directly, everything else
/// identical to [`gen_fill_frame`] at `cycle_type = 3`.
pub(crate) fn gen_fill_coverage_mode(color: u16, other_modes_word1: u32) -> Vec<(u32, u32)> {
    let other_modes = (0xef30_00f0, other_modes_word1);
    vec![
        other_modes,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xf700_0000, (color as u32) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
        (0xe900_0000, 0),
    ]
}

/// A one-cycle flat-shaded triangle pair with `other_modes_word1` set
/// directly, over a `STALE`-seeded target.
pub(crate) fn gen_one_cycle_coverage_mode(other_modes_word1: u32) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (0xef00_00f0, other_modes_word1),
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, 0x20c0_e0ff),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// Register the coverage-modes slice's cases into the generator corpus.
pub(crate) fn push_coverage_mode_cases(
    push: &mut impl FnMut(u8, String, &'static str, Vec<(u32, u32)>),
) {
    for (bits, label) in [
        (0x000u32, "clamp"),
        (0x100, "wrap"),
        (0x200, "zap"),
        (0x300, "save"),
    ] {
        push(
            6,
            format!("gen-coverage-cvgdest-{label}-fill"),
            "cvg_dest selector x fill-cycle rectangle (fill bypasses cvg_dest; \
             non-authoritative for RT64 per guard-audit C4-C6)",
            gen_fill_coverage_mode(0xf801, bits),
        );
    }

    for (bits, label) in [
        (0x000u32, "clamp"),
        (0x100, "wrap"),
        (0x200, "zap"),
        (0x300, "save"),
    ] {
        push(
            6,
            format!("gen-coverage-cvgdest-{label}-one-cycle"),
            "cvg_dest selector x one-cycle flat triangle, AA off (full \
             coverage in, destination-write policy under test)",
            gen_one_cycle_coverage_mode(bits),
        );
    }

    push(
        6,
        "gen-coverage-color-on-cvg-one-cycle".into(),
        "CLR_ON_CVG with CVG_DST_WRAP: color write gated on coverage \
         reaching full",
        gen_one_cycle_coverage_mode(0x080 /* CLR_ON_CVG */ | 0x100 /* CVG_DST_WRAP */),
    );

    push(
        6,
        "gen-coverage-cvg-x-alpha-aa-one-cycle".into(),
        "CVG_X_ALPHA with AA_EN: coverage-weighted alpha on an antialiased \
         triangle edge",
        gen_one_cycle_coverage_mode(0x1000 /* CVG_X_ALPHA */ | 0x8 /* AA_EN */),
    );

    push(
        6,
        "gen-coverage-force-blend-one-cycle".into(),
        "FORCE_BL with IM_RD + CVG_DST_WRAP: general blender forced on over \
         a one-cycle triangle",
        gen_one_cycle_coverage_mode(
            0x4000 /* FORCE_BL */ | 0x40 /* IM_RD */ | 0x100 /* CVG_DST_WRAP */
                | (2 << 18), /* cycle-1 B = One */
        ),
    );

    push(
        6,
        "gen-coverage-all-modes-combined-one-cycle".into(),
        "AA_EN + CVG_DST_WRAP + CLR_ON_CVG + FORCE_BL together (matches the \
         public G_RM_AA_XLU_SURF bit combination) over a one-cycle triangle",
        gen_one_cycle_coverage_mode(
            0x8 /* AA_EN */ | 0x100 /* CVG_DST_WRAP */ | 0x80 /* CLR_ON_CVG */
                | 0x4000 /* FORCE_BL */ | 0x40 /* IM_RD, required for FORCE_BL's M/B reads */
                | (2 << 18), /* cycle-1 B = One */
        ),
    );
}

// -----------------------------------------------------------------------
// slice formats-deep
// -----------------------------------------------------------------------
//
// The direct/CI texture formats sampled through a TEXTURED TRIANGLE rather
// than a texture rectangle, reusing each format's proven texrect staging
// (`one_direct_texture_rect`, `one_ci4_rect`, `one_ci8_rect`) but drawing
// with [`textured_triangle_pair`] instead. Every case sets BI_LERP_0 except
// IA/I formats, which are immune (their value already lives in the blue
// channel the color-convert collapse preserves).

/// A direct-format (IA8/IA4/IA16/I4/I8) texture sampled by a textured
/// triangle instead of a texrect. Mirrors [`one_direct_texture_rect`]'s
/// staging exactly; only the final draw command differs.
pub(crate) fn direct_format_textured_triangle(
    source: u32,
    load_texels_16b: u32,
    format: u32,
    size: u32,
    line_words: u32,
) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (2 << 19) | (load_texels_16b - 1), source),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (
            0xf500_0000 | (format << 21) | (size << 19) | (line_words << 9),
            0,
        ),
        set_tile_size(TRI_RIGHT - TRI_LEFT, 1),
    ]);
    words.extend(textured_triangle_pair());
    words.push((0xe900_0000, 0));
    words
}

/// RGBA32 as a textured-triangle source. `bilerp`: whether BI_LERP_0 (mode
/// word bit 11) is set. **The corrected case (`bilerp = true`) is the one to
/// trust**; the `false` variant is kept only as the corpus's documented
/// bilerp-gap witness for the triangle path, mirroring
/// `gen-loadblock-linear-missing-bilerp`'s texrect-path witness.
pub(crate) fn rgba32_textured_triangle(bilerp: bool) -> Vec<(u32, u32)> {
    let width = RGBA32_EXPECTED.len() as u32; // 2
    let other_modes = if bilerp {
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        )
    } else {
        OTHER_MODES_ONE_CYCLE_TEXTURED
    };
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        other_modes,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (3 << 19) | (width - 1), RGBA32_SOURCE),
        (0xf500_0000 | (3 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
        (0xe600_0000, 0),
        load_tile(width, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (3 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

/// CI4+16-entry TLUT as a textured-triangle source, mirroring
/// [`one_ci4_rect`]'s staging exactly; only the final draw differs. Sampled
/// at the FULL `CI_INDICES` width (not the fixed 4-texel `TRI_LEFT..
/// TRI_RIGHT` box): a narrower box would advance the S plane past its own
/// staged indices and silently clamp/wrap onto a neighbor instead of
/// reading what was actually loaded.
pub(crate) fn ci4_textured_triangle() -> Vec<(u32, u32)> {
    let entries = PALETTE.len() as u32;
    let width = CI_INDICES.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (2 << 19) | (entries - 1), PALETTE_SOURCE),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (0xfd00_0000 | (2 << 19) | (CI_LOAD_TEXELS - 1), CI_SOURCE),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(CI_LOAD_TEXELS, 1),
        (0xe600_0000, 0),
        load_tile(CI_LOAD_TEXELS, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (0 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

/// CI8+256-entry TLUT as a textured-triangle source, mirroring
/// [`one_ci8_rect`]'s staging exactly; only the final draw differs. Sampled
/// at the FULL `CI8_INDICES` width for the same reason as
/// [`ci4_textured_triangle`].
pub(crate) fn ci8_textured_triangle() -> Vec<(u32, u32)> {
    let entries = 256u32;
    let load_texels_16b = CI8_INDICES.len() as u32 / 2;
    let width = CI8_INDICES.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (2 << 19) | (entries - 1), CI8_PALETTE_SOURCE),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (0xfd00_0000 | (2 << 19) | (load_texels_16b - 1), CI8_SOURCE),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (1 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

// -----------------------------------------------------------------------
// slice zbuffer
// -----------------------------------------------------------------------
//
// Z-BUFFER matrix: SetOtherModes z_compare_en/z_update_en/z_source_sel,
// SetMaskImage(0x3e) as an alternate z-image binding, and two overlapping
// flat triangles at different depths so z-compare/z-update actually decide
// which one's colour survives.
//
// Every case uses G_ZS_PRIM (`SetOtherModes` low-word bit 2) except case 5,
// which uses G_ZS_PIXEL with an explicit per-triangle Z coefficient block.
//
// SetOtherModes bit layout (public libultra `gbi.h`):
//   word1 bit 2       G_MDSFT_ZSRCSEL   0 = G_ZS_PIXEL, 1 = G_ZS_PRIM
//   word1 bit 4       Z_CMP             z_compare_en
//   word1 bit 5       Z_UPD             z_update_en
//
// SetZImage (0xfe) / SetMaskImage (0x3e): opcode in top byte of word0,
// address in low 24 bits of word1 -- byte-identical wire handling in
// angrylion (`rdp_set_depth_image` / `rdp_set_mask_image` both do
// `wstate->zb_address = args[1] & 0x00ffffff`).
//
// **fn64 gap under test.** wgpu's raw-DPC decoder has no dispatch arm for
// opcode 0x3e (own unit test asserts `UnsupportedCommand`). Case 6 below is
// expected to make wgpu REFUSE while angrylion and RT64 accept and agree.

pub(crate) const ZBUF_Z_IMAGE: u32 = 0x9000;

/// `SetZImage` (opcode `0xfe`).
pub(crate) const fn set_z_image(address: u32) -> (u32, u32) {
    (0xfe00_0000, address & 0x00ff_ffff)
}

/// `SetMaskImage` (opcode `0x3e`), byte-identical wire shape to
/// [`set_z_image`].
pub(crate) const fn set_mask_image(address: u32) -> (u32, u32) {
    (0x3e00_0000, address & 0x00ff_ffff)
}

/// `SetPrimDepth` (opcode `0xee`): word0 bare, word1 = `(z << 16) | delta_z`.
pub(crate) const fn set_prim_depth(z: u16, delta_z: u16) -> (u32, u32) {
    (0xee00_0000, ((z as u32) << 16) | (delta_z as u32))
}

/// `SetOtherModes` one-cycle word carrying the requested Z fields.
pub(crate) const fn other_modes_one_cycle_z(
    z_source_prim: bool,
    z_compare_en: bool,
    z_update_en: bool,
) -> (u32, u32) {
    let mut w1 = 0u32;
    if z_source_prim {
        w1 |= 1 << 2;
    }
    if z_compare_en {
        w1 |= 1 << 4;
    }
    if z_update_en {
        w1 |= 1 << 5;
    }
    (0xef00_00f0, w1)
}

pub(crate) const ZBUF_NEAR_COLOR: u32 = 0xff00_00ff; // opaque red, RGBA8888
pub(crate) const ZBUF_FAR_COLOR: u32 = 0x00ff_00ff; // opaque green, RGBA8888

/// One full frame exercising two overlapping flat triangles under
/// G_ZS_PRIM: a "far" triangle (green) drawn first covering the whole TRI
/// box, then a "near" triangle (red) drawn second over the identical box.
pub(crate) fn zbuffer_overlap_case(
    other_modes_word1: u32,
    far_z: u16,
    near_z: u16,
    mask_image_instead_of_setzimage: bool,
) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        if mask_image_instead_of_setzimage {
            set_mask_image(ZBUF_Z_IMAGE)
        } else {
            set_z_image(ZBUF_Z_IMAGE)
        },
        (0xef00_00f0, other_modes_word1),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(set_prim_depth(far_z, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(near_z, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// Case 1 / 2 builder: z_compare_en + z_update_en both on, G_ZS_PRIM.
pub(crate) fn gen_zbuffer_compare_and_update(
    second_draw_z: u16,
    first_draw_z: u16,
) -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, true, true).1,
        first_draw_z,
        second_draw_z,
        false,
    )
}

/// Case 3: z_compare_en OFF, G_ZS_PRIM.
pub(crate) fn gen_zbuffer_compare_disabled(
    second_draw_z: u16,
    first_draw_z: u16,
) -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, false, false).1,
        first_draw_z,
        second_draw_z,
        false,
    )
}

/// Case 4: z_compare_en ON, z_update_en OFF, G_ZS_PRIM -- the update-disabled
/// twin of [`gen_zbuffer_compare_and_update`]'s "nearer wins" key.
pub(crate) fn gen_zbuffer_update_disabled() -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, true, false).1,
        0x8000,
        0x4000,
        false,
    )
}

/// Case 5: `z_source_sel` itself. G_ZS_PIXEL with an explicit per-pixel Z
/// coefficient block on a raw `0x09` triangle, drawn OVER a G_ZS_PRIM
/// triangle whose PrimDepth is farther everywhere in the box.
pub(crate) fn gen_zbuffer_source_sel_pixel_wins() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push((0xef00_00f0, other_modes_one_cycle_z(true, true, true).1));
    words.push(set_prim_depth(0x8000, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xef00_00f0, other_modes_one_cycle_z(false, true, true).1));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    let z_words = |x_h: u32, x_l: u32, y_h: u32, y_l: u32, y_m: u32| -> Vec<(u32, u32)> {
        let yl = ((y_l as i32) << 2) as u16 as u32;
        let ym = ((y_m as i32) << 2) as u16 as u32;
        let yh = ((y_h as i32) << 2) as u16 as u32;
        vec![
            (0x0900_0000 | (1 << 23) | yl, (ym << 16) | yh),
            (x_l << 16, 0),
            (x_h << 16, 0),
            (x_l << 16, 0),
            (2 << 16, 0), // z = 0x0002_0000, dzdx = 0
            (0, 0),       // dzde = 0, dzdy = 0
        ]
    };
    words.extend(z_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(z_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push((0xe900_0000, 0));
    words
}

/// Case 6: identical to case 1 except the z-image is bound with
/// `SetMaskImage` (`0x3e`) instead of `SetZImage` (`0xfe`).
pub(crate) fn gen_zbuffer_setmaskimage_binds_z_image() -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, true, true).1,
        0x8000,
        0x2000,
        true,
    )
}

// -----------------------------------------------------------------------
// slice loadblock-deep
// -----------------------------------------------------------------------
//
// LOADBLOCK (0x33) DxT row-advance, sampled by TEXTURED TRIANGLES (not
// texrects), across RGBA16 and CI8 sources.

pub(crate) const LOADBLOCK_DEEP_RGBA16_SOURCE: u32 = 0x6000;
pub(crate) const LOADBLOCK_DEEP_RGBA16_WIDTH: u32 = 8;

pub(crate) const LOADBLOCK_DEEP_RGBA16_TEXELS: [u16; 32] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0xf841, 0x0641, 0x0079, 0xffbf,
    0x8461, 0xc671, 0x4251, 0xfc41, 0xf803, 0x07c3, 0x003d, 0x7ffd, 0x8423, 0xc633, 0x4213, 0xfc03,
    0xf843, 0x0643, 0x007b, 0xffbd, 0x8463, 0xc673, 0x4253, 0xfc43,
];

/// A LoadBlock case over [`LOADBLOCK_DEEP_RGBA16_TEXELS`], sampled by a
/// TEXTURED TRIANGLE rather than a texture rectangle.
pub(crate) fn load_block_deep_triangle(
    texel_count: u32,
    dxt: u32,
    load_line_words: u32,
    render_width: u32,
    render_height: u32,
    render_line_words: u32,
) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(LOADBLOCK_DEEP_RGBA16_WIDTH, LOADBLOCK_DEEP_RGBA16_SOURCE),
        (0xe800_0000, 0),
        set_tile(load_line_words, 0),
        (0xe600_0000, 0),
        load_block(texel_count, dxt),
        (0xe700_0000, 0),
        set_tile(render_line_words, 0),
        set_tile_size(render_width, render_height),
    ]);

    let s_base = PLANE_HALF_TEXEL;
    let t_base = PLANE_HALF_TEXEL;
    let x_left = 0u32;
    let x_right = render_width;
    let y_top = 0u32;
    let y_bottom = render_height;

    let triangle = |x_h: u32, x_l: u32, y_m: u32, s_at_h: i32| {
        let yl = ((y_bottom as i32) << 2) as u16 as u32;
        let ym = ((y_m as i32) << 2) as u16 as u32;
        let yh = ((y_top as i32) << 2) as u16 as u32;
        let base = [
            (0x0a00_0000 | (1 << 23) | yl, (ym << 16) | yh),
            (x_l << 16, 0),
            (x_h << 16, 0),
            (x_l << 16, 0),
        ];
        let texture = coefficient_block(
            [s_at_h, t_base, 1, 0],
            [PLANE_PER_TEXEL, 0, 0, 0],
            [0, PLANE_PER_TEXEL, 0, 0],
            [0, 0, 0, 0],
        );
        let mut w = base.to_vec();
        for pair in texture.chunks_exact(2) {
            w.push((pair[0], pair[1]));
        }
        w
    };

    words.extend(triangle(x_left, x_right, y_bottom, s_base));
    let right_s = s_base + PLANE_PER_TEXEL * (x_right - x_left) as i32;
    words.extend(triangle(x_right, x_left, y_top, right_s));

    words.push((0xe900_0000, 0));
    words
}

pub(crate) const LOADBLOCK_DEEP_CI8_SOURCE: u32 = 0x7000;

pub(crate) const LOADBLOCK_DEEP_CI8_INDICES: [u8; 32] = [
    0x03, 0x20, 0x55, 0x81, 0xa7, 0xc2, 0xe6, 0xf4, 0x10, 0x30, 0x60, 0x90, 0xb0, 0xd0, 0xf0, 0x01,
    0x11, 0x31, 0x61, 0x91, 0xb1, 0xd1, 0xf1, 0x02, 0x12, 0x32, 0x62, 0x92, 0xb2, 0xd2, 0xf2, 0x04,
];

/// A LoadBlock case over CI8 indices, sampled by a TEXTURED TRIANGLE.
pub(crate) fn load_block_ci8_deep_triangle(
    source_addr: u32,
    indices: &[u8],
    texel_count: u32,
    dxt: u32,
    load_line_words: u32,
    render_width: u32,
    render_height: u32,
    render_line_words: u32,
) -> Vec<(u32, u32)> {
    let entries = 256u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (2 << 19) | (entries - 1), CI8_PALETTE_SOURCE),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (0xe800_0000, 0),
        set_texture_image(texel_count / 2, source_addr),
        set_tile(load_line_words, 0),
        (0xe600_0000, 0),
        load_block(texel_count, dxt),
        (0xe700_0000, 0),
        (
            0xf500_0000 | (2 << 21) | (1 << 19) | (render_line_words << 9),
            0,
        ),
        set_tile_size(render_width, render_height),
    ]);

    let _ = indices;

    let s_base = PLANE_HALF_TEXEL;
    let t_base = PLANE_HALF_TEXEL;
    let x_right = render_width;
    let y_bottom = render_height;

    let triangle = |x_h: u32, x_l: u32, y_m: u32, s_at_h: i32| {
        let yl = ((y_bottom as i32) << 2) as u16 as u32;
        let ym = ((y_m as i32) << 2) as u16 as u32;
        let yh = 0u32;
        let base = [
            (0x0a00_0000 | (1 << 23) | yl, (ym << 16) | yh),
            (x_l << 16, 0),
            (x_h << 16, 0),
            (x_l << 16, 0),
        ];
        let texture = coefficient_block(
            [s_at_h, t_base, 1, 0],
            [PLANE_PER_TEXEL, 0, 0, 0],
            [0, PLANE_PER_TEXEL, 0, 0],
            [0, 0, 0, 0],
        );
        let mut w = base.to_vec();
        for pair in texture.chunks_exact(2) {
            w.push((pair[0], pair[1]));
        }
        w
    };

    words.extend(triangle(0, x_right, y_bottom, s_base));
    let right_s = s_base + PLANE_PER_TEXEL * x_right as i32;
    words.extend(triangle(x_right, 0, 0, right_s));

    words.push((0xe900_0000, 0));
    words
}

/// The first batch: ~30 highest-priority cases across the matrix.
///
/// Priority order (brief): (1) LOADBLOCK, (2) triangle variants, (3) syncs,
/// (4) SetPrimDepth/SetBlendColor/TexRectFlip, (5) mode matrix, (6) convert/
/// key/maskimage. The batch is capped so results can be triaged before
/// expanding.
// =====================================================================
// Track-B fan-out pass 2 -- designed slice builders.
// =====================================================================

// -----------------------------------------------------------------------
// slice two-cycle-combine
// -----------------------------------------------------------------------

pub(crate) const fn set_tile_1(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (
        0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word,
        1 << 24,
    )
}

pub(crate) const fn set_tile_size_1(width: u32, height: u32) -> (u32, u32) {
    (
        0xf200_0000,
        (1 << 24) | (((width - 1) * 4) << 12) | ((height - 1) * 4),
    )
}

pub(crate) const fn load_tile_1(width: u32, height: u32) -> (u32, u32) {
    (
        0xf400_0000,
        (1 << 24) | (((width - 1) * 4) << 12) | ((height - 1) * 4),
    )
}

pub(crate) fn stage_and_declare_two_tiles() -> Vec<(u32, u32)> {
    vec![
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        set_texture_image(WIDE_WIDTH, WIDE_SOURCE),
        set_tile_1(WIDE_LINE_WORDS, 8),
        set_tile_size_1(WIDE_WIDTH, WIDE_HEIGHT),
        (0xe600_0000, 0),
        load_tile_1(WIDE_WIDTH, WIDE_HEIGHT),
        (0xe600_0000, 0),
        set_tile_size_1(TEXTURE_WIDTH, TEXTURE_HEIGHT),
    ]
}

pub(crate) fn two_cycle_two_tile_rect(combine: (u32, u32), prim_rgba8888: u32) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.push((
        OTHER_MODES_TWO_CYCLE_TEXTURED.0 | (1 << 11),
        OTHER_MODES_TWO_CYCLE_TEXTURED.1,
    ));
    words.push(combine);
    words.push((0xfa00_0000, prim_rgba8888));
    words.push(set_scissor(0, 0, WIDTH, HEIGHT));
    words.push((0xff10_0000 | (WIDTH - 1), FRAMEBUFFER));
    words.extend(stage_and_declare_two_tiles());
    words.extend(texture_rectangle(
        TEXRECT_ULX,
        TEXRECT_ULY,
        TEXRECT_LRX,
        TEXRECT_LRY,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const SET_COMBINE_TEXEL0_INTO_TEXEL1_SELECT: (u32, u32) = (0xfc88_7e4a, 0x80fc_f238);
pub(crate) const SET_COMBINE_TEXEL1_DIRECT: (u32, u32) = (0xfc88_7f10, 0x88fc_f2ba);
pub(crate) const SET_COMBINE_LOD_FRACTION_GAP: (u32, u32) = (0xfc88_7e4d, 0x81fc_f27a);
pub(crate) const SET_COMBINE_WM2000_FOG: (u32, u32) = (0xfc15_fea3, 0xf00f_f23f);
pub(crate) const WM2000_FOG_SHADE_RGBA: [i32; 4] = [0, 0, 0, 0xff << 16];

pub(crate) fn shade_and_textured_triangle_pair(shade_rgba: [i32; 4]) -> Vec<(u32, u32)> {
    let opcode = 0x0e00_0000;
    let half = |x_h: u32, x_l: u32, y_m: u32, s_base: i32| -> Vec<(u32, u32)> {
        let yl = ((TRI_BOTTOM as i32) << 2) as u16 as u32;
        let ym = ((y_m as i32) << 2) as u16 as u32;
        let yh = ((TRI_TOP as i32) << 2) as u16 as u32;
        let mut w = vec![
            (opcode | (1 << 23) | yl, (ym << 16) | yh),
            (x_l << 16, 0),
            (x_h << 16, 0),
            (x_l << 16, 0),
        ];
        let shade = coefficient_block(shade_rgba, [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]);
        for pair in shade.chunks_exact(2) {
            w.push((pair[0], pair[1]));
        }
        let texture = coefficient_block(
            [s_base, PLANE_HALF_TEXEL, 1, 0],
            [PLANE_PER_TEXEL, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        );
        for pair in texture.chunks_exact(2) {
            w.push((pair[0], pair[1]));
        }
        w
    };
    let left_s = PLANE_HALF_TEXEL - PLANE_PER_TEXEL / 8;
    let right_s = left_s + PLANE_PER_TEXEL * (TRI_RIGHT - TRI_LEFT) as i32;
    let mut words = half(TRI_LEFT, TRI_RIGHT, TRI_BOTTOM, left_s);
    words.extend(half(TRI_RIGHT, TRI_LEFT, TRI_TOP, right_s));
    words
}

pub(crate) fn wm2000_fog_triangle() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_TWO_CYCLE_TEXTURED.0 | (1 << 11),
            OTHER_MODES_TWO_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_WM2000_FOG,
        (0xfa00_0000, 0x0000_0000),
        (0xfb00_0000, 0x0000_ffff),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
    ]);
    words.extend(shade_and_textured_triangle_pair(WM2000_FOG_SHADE_RGBA));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const SET_COMBINE_WM2000_SHADE_FOG: (u32, u32) = (0xfc45_fea3, 0xf00f_f83f);

pub(crate) fn wm2000_shade_fog_triangle() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (0xef00_00f0 | (1 << 20), 0u32),
        SET_COMBINE_WM2000_SHADE_FOG,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfa00_0000, 0x0000_0000),
        (0xfb00_0000, 0x0000_ffff),
    ]);
    words.extend(shade_triangle_words(
        TRI_LEFT,
        TRI_RIGHT,
        TRI_TOP,
        TRI_BOTTOM,
        TRI_BOTTOM,
        SHADE_TRIANGLE_RGBA,
    ));
    words.extend(shade_triangle_words(
        TRI_RIGHT,
        TRI_LEFT,
        TRI_TOP,
        TRI_BOTTOM,
        TRI_TOP,
        SHADE_TRIANGLE_RGBA,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const SET_COMBINE_PRIMITIVE_ALPHA_CHAIN: (u32, u32) = (0xfc88_7e67, 0x881e_f7f8);

pub(crate) fn combined_alpha_chain_bands() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.push((OTHER_MODES_TWO_CYCLE_TEXTURED.0 & !(1 << 11), 0));
    words.push(SET_COMBINE_PRIMITIVE_ALPHA_CHAIN);
    words.push((0xfb00_0000, 0x2020_20ff));
    words.push(set_scissor(0, 0, WIDTH, HEIGHT));
    words.push((0xff10_0000 | (WIDTH - 1), FRAMEBUFFER));
    words.push((0xfa00_0000, 0x4080_c0ff));
    words.extend(flat_triangle_words(0, 40, 0, 8, 8));
    words.extend(flat_triangle_words(40, 0, 0, 8, 0));
    words.push((0xfa00_0000, 0x4080_c000));
    words.extend(flat_triangle_words(40, 80, 0, 8, 8));
    words.extend(flat_triangle_words(80, 40, 0, 8, 0));
    words.push((0xe900_0000, 0));
    words
}

// -----------------------------------------------------------------------
// slice blend-deep (Pass 2)
// -----------------------------------------------------------------------

pub(crate) const fn blend_deep_other_modes(
    cycle_type: u32,
    p1: u32,
    a1: u32,
    m1: u32,
    b1: u32,
    p2: u32,
    a2: u32,
    m2: u32,
    b2: u32,
    force_bl: bool,
    im_rd: bool,
    aa_en: bool,
) -> (u32, u32) {
    let high = 0xef00_00f0 | (cycle_type << 20);
    let mut low = (p1 << 30)
        | (a1 << 26)
        | (m1 << 22)
        | (b1 << 18)
        | (p2 << 28)
        | (a2 << 24)
        | (m2 << 20)
        | (b2 << 16);
    if force_bl {
        low |= 1 << 14;
    }
    if im_rd {
        low |= 1 << 6;
    }
    if aa_en {
        low |= 1 << 3;
    }
    (high, low)
}

pub(crate) fn gen_blend_deep_rect(
    memory_seed: u16,
    primitive_rgba8888: u32,
    blend_color_rgba8888: u32,
    fog_color_rgba8888: u32,
    other_modes: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (0xf900_0000, blend_color_rgba8888),
        (0xf800_0000, fog_color_rgba8888),
        other_modes,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

// `gen_blend_deep_textured` was removed with its only caller,
// `gen-blend-deep-two-cycle-textured-bilerp` (dropped in task 21 as
// all-three-differ-inspect-construction; see the drop note at the push site).

pub(crate) fn sloped_triangle_words(dxhdy_q16: i32) -> Vec<(u32, u32)> {
    let yl = ((TRI_BOTTOM as i32) << 2) as u16 as u32;
    let ym = yl;
    let yh = ((TRI_TOP as i32) << 2) as u16 as u32;
    vec![
        (0x0800_0000 | (1 << 23) | yl, (ym << 16) | yh),
        ((TRI_LEFT << 16), 0),
        ((TRI_LEFT << 16), dxhdy_q16 as u32),
        ((TRI_LEFT << 16), 0),
    ]
}

pub(crate) fn gen_blend_aa_edge_rect(primitive_rgba8888: u32, dxhdy_q16: i32) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        blend_deep_other_modes(0, 0, 0, 1, 0, 0, 0, 0, 0, true, true, true),
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(sloped_triangle_words(dxhdy_q16));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn seed_striped_background(
    stripe_a: u16,
    stripe_b: u16,
    stripe_width: u32,
) -> Vec<(u32, u32)> {
    let mut words = vec![
        (0xef30_00f0, 0u32),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ];
    let mut x = 0u32;
    let mut which_a = true;
    while x < WIDTH {
        let next = (x + stripe_width).min(WIDTH);
        words.push((
            0xf700_0000,
            (if which_a { stripe_a } else { stripe_b } as u32) * 0x1_0001,
        ));
        words.push(fill_rect(next - 1, HEIGHT - 1, x, 0));
        x = next;
        which_a = !which_a;
    }
    words
}

pub(crate) fn gen_blend_im_rd_over_striped(
    primitive_rgba8888: u32,
    stripe_a: u16,
    stripe_b: u16,
) -> Vec<(u32, u32)> {
    const STRIPE_WIDTH: u32 = 16;
    // Draw a small 32x8 rect (NOT the whole framebuffer): x 0..32 crosses the
    // stripe boundary at 16 so both the `stripe_a` and `stripe_b` memory phases
    // are read-modify-written, while keeping the differing-pixel count bounded
    // (<=256) instead of the whole-framebuffer 38,400. The IM_RD read-address
    // and blend-rounding divergence appears identically over the small region.
    const DRAW_RIGHT: u32 = 32;
    const DRAW_BOTTOM: u32 = 8;
    let mut words = seed_striped_background(stripe_a, stripe_b, STRIPE_WIDTH);
    words.extend([
        blend_deep_other_modes(0, 0, 0, 1, 0, 0, 0, 0, 0, true, true, false),
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(
        0,
        DRAW_RIGHT,
        0,
        DRAW_BOTTOM,
        DRAW_BOTTOM,
    ));
    words.extend(flat_triangle_words(DRAW_RIGHT, 0, 0, DRAW_BOTTOM, 0));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn push_blend_deep_cases(
    push: &mut impl FnMut(u8, String, &'static str, Vec<(u32, u32)>),
) {
    const OPAQUE_RED: u32 = 0xff00_00ff;
    const HALF_ALPHA_RED: u32 = 0xff00_0080;
    const BLEND_COLOR_CYAN: u32 = 0x00ff_ffff;
    const FOG_COLOR_YELLOW: u32 = 0xffff_00ff;

    push(
        7,
        "gen-blend-deep-two-cycle-both-stages".into(),
        "two-cycle blender, BOTH stages doing real work: stage1 alpha-blend \
         primitive over clr_mem, stage2 blend that result over BlendColor; \
         proves both stages execute in sequence",
        gen_blend_deep_rect(
            GREEN,
            HALF_ALPHA_RED,
            BLEND_COLOR_CYAN,
            FOG_COLOR_YELLOW,
            blend_deep_other_modes(1, 0, 0, 1, 0, 0, 0, 2, 0, true, true, false),
        ),
    );

    push(
        7,
        "gen-blend-deep-blend-color-as-m-mux".into(),
        "two-cycle blender, stage1 M=BlendColor (M-mux read), stage2 B=Zero \
         passthrough of stage1 output -- isolates the M-mux BlendColor path",
        gen_blend_deep_rect(
            GREEN,
            OPAQUE_RED,
            BLEND_COLOR_CYAN,
            FOG_COLOR_YELLOW,
            blend_deep_other_modes(1, 0, 0, 2, 0, 0, 3, 0, 3, true, true, false),
        ),
    );

    push(
        7,
        "gen-blend-deep-im-rd-striped-framebuffer".into(),
        "IM_RD read-modify-write: one-cycle alpha-blend of a semi-transparent \
         primitive over a PRE-SEEDED, non-uniform (16px vertical stripes) \
         framebuffer -- catches a wrong read address, not just a wrong blend",
        gen_blend_im_rd_over_striped(HALF_ALPHA_RED, 0xf801, 0x003f),
    );

    // DROPPED (task 21): `gen-blend-deep-two-cycle-textured-bilerp` classified
    // all-three-differ-inspect-construction (wgpu 0x07ff, RT64 0xf801, angrylion
    // 0x66f7; wgpu_d=9, rt64_d=12 -- wgpu != RT64). It layered a two-cycle
    // combiner over a TEXTURED triangle without a valid two-cycle combine word,
    // so cycle 1 fell into the unsettled second-cycle-texel path that wgpu and
    // RT64 model differently (the same axis the wgpu-refused
    // `gen-two-cycle-texel1-*` cases already isolate). It tested no single clean
    // cause; the two-cycle blender axis is covered by
    // `gen-blend-deep-two-cycle-both-stages` (shared-ported-bug) and
    // `gen-two-cycle-wm2000-*` instead.

    push(
        7,
        "gen-blend-aa-sloped-edge".into(),
        "antialiased partial-coverage edge: a SLOPED triangle (nonzero dXHdy) \
         with AA_EN, one-cycle alpha-blend over a STALE background -- \
         hypotenuse column coverage varies by row",
        gen_blend_aa_edge_rect(HALF_ALPHA_RED, 65536 / 2),
    );

    push(
        7,
        "gen-blend-aa-coverage-driven-edge".into(),
        "B=FramebufferCoverage/8 on the SAME sloped AA edge as \
         gen-blend-aa-sloped-edge -- the partial-coverage column is where this \
         selector can differ from a hardcoded B=1",
        {
            let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
            words.pop();
            words.extend([
                blend_deep_other_modes(0, 0, 0, 1, 1, 0, 0, 0, 0, true, true, true),
                SET_COMBINE_PRIMITIVE,
                (0xfa00_0000, HALF_ALPHA_RED),
                set_scissor(0, 0, WIDTH, HEIGHT),
                (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
            ]);
            words.extend(sloped_triangle_words(65536 / 2));
            words.push((0xe900_0000, 0));
            words
        },
    );
}

// -----------------------------------------------------------------------
// slice tlut-palette-deep
// -----------------------------------------------------------------------

pub(crate) const PALETTE_BANK0: [u16; 16] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0x0843, 0x0843, 0x0843, 0x0843,
    0x0843, 0x0843, 0x0843, 0x0843,
];
pub(crate) const PALETTE_BANK1: [u16; 16] = [
    0x8001, 0xf7c1, 0x783f, 0x07ff, 0x4421, 0x2631, 0xc211, 0x1c01, 0x2222, 0x2222, 0x2222, 0x2222,
    0x2222, 0x2222, 0x2222, 0x2222,
];

pub(crate) const PALETTE_BANK_CI4_SOURCE: u32 = 0xc000;
pub(crate) const PALETTE_BANK_TLUT_SOURCE: u32 = 0xc400;
pub(crate) const PALETTE_BANK_CI4_INDICES: [u8; 8] = [3, 0, 5, 1, 7, 2, 6, 4];

pub(crate) fn ci4_palette_bank_textured_triangle(bank: u32) -> Vec<(u32, u32)> {
    let entries = (PALETTE_BANK0.len() + PALETTE_BANK1.len()) as u32;
    let width = PALETTE_BANK_CI4_INDICES.len() as u32;
    let load_texels_16b = width / 4;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (
            0xfd00_0000 | (2 << 19) | (entries - 1),
            PALETTE_BANK_TLUT_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (
            0xfd00_0000 | (2 << 19) | (load_texels_16b - 1),
            PALETTE_BANK_CI4_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (0 << 19) | (1 << 9), bank << 20),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const CI8_FULL_RANGE_SOURCE: u32 = 0xc800;
pub(crate) const CI8_FULL_RANGE_TLUT_SOURCE: u32 = 0xcc00;
pub(crate) const CI8_FULL_RANGE_WIDTH: u32 = 32;

pub(crate) fn ci8_full_range_index(i: u32) -> u8 {
    (i * 4) as u8
}

pub(crate) fn ci8_full_range_palette_entry(index: u8) -> u16 {
    0x8000 | (index as u16)
}

pub(crate) fn ci8_full_range_textured_triangle() -> Vec<(u32, u32)> {
    let entries = 256u32;
    let width = CI8_FULL_RANGE_WIDTH;
    let load_texels_16b = width / 2;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (
            0xfd00_0000 | (2 << 19) | (entries - 1),
            CI8_FULL_RANGE_TLUT_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (
            0xfd00_0000 | (2 << 19) | (load_texels_16b - 1),
            CI8_FULL_RANGE_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (1 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const TLUT_TYPE_CI8_SOURCE: u32 = 0xd000;
pub(crate) const TLUT_TYPE_TLUT_SOURCE: u32 = 0xd400;
pub(crate) const TLUT_TYPE_INDICES: [u8; 4] = [0x10, 0x40, 0x90, 0xe0];
pub(crate) const TLUT_TYPE_ENTRIES: [u16; 4] = [0x8421, 0xc631, 0x4a5c, 0x93b7];

pub(crate) fn tlut_type_textured_triangle(tlut_type: u32) -> Vec<(u32, u32)> {
    let entries = 256u32;
    let width = TLUT_TYPE_INDICES.len() as u32;
    let load_texels_16b = width / 2;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (tlut_type << 14) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (
            0xfd00_0000 | (2 << 19) | (entries - 1),
            TLUT_TYPE_TLUT_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (
            0xfd00_0000 | (2 << 19) | (load_texels_16b - 1),
            TLUT_TYPE_CI8_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (1 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const PALETTE_ORIGIN_SOURCE: u32 = 0xd800;
pub(crate) const PALETTE_ORIGIN_CI8_SOURCE: u32 = 0xdc00;
pub(crate) const PALETTE_ORIGIN_TEXEL_OFFSET: u32 = 40;
pub(crate) const PALETTE_ORIGIN_ENTRIES: u32 = 4;
pub(crate) const PALETTE_ORIGIN_CI8_INDICES: [u8; 4] = [0, 1, 2, 3];

pub(crate) fn palette_origin_textured_triangle() -> Vec<(u32, u32)> {
    let entries = PALETTE_ORIGIN_ENTRIES;
    let width = entries;
    let load_texels_16b = width / 2;
    let source_addr = PALETTE_ORIGIN_SOURCE + PALETTE_ORIGIN_TEXEL_OFFSET * 2;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15) | (1 << 11),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (2 << 19) | (entries - 1), source_addr),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        (
            0xfd00_0000 | (2 << 19) | (load_texels_16b - 1),
            PALETTE_ORIGIN_CI8_SOURCE,
        ),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (1 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(textured_triangle_pair_of_width(width));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const PALETTE_ORIGIN_TLUT_ENTRIES: [u16; 4] = [0xf801, 0x07c1, 0x003f, 0x7fff];

// -----------------------------------------------------------------------
// slice lod-mip
// -----------------------------------------------------------------------

pub(crate) const OTHER_MODES_ONE_CYCLE_TEXTURED_LOD: (u32, u32) =
    (OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 11) | (1 << 16), 0);

pub(crate) const OTHER_MODES_TWO_CYCLE_TEXTURED_LOD: (u32, u32) =
    (OTHER_MODES_ONE_CYCLE_TEXTURED_LOD.0 | (1 << 20), 0);

pub(crate) const fn with_texture_detail(other_modes: (u32, u32), detail: u32) -> (u32, u32) {
    (other_modes.0 | (detail << 17), other_modes.1)
}

pub(crate) const MIP1_SOURCE: u32 = 0xb000;
pub(crate) const MIP1_TMEM_WORD: u32 = 8;
pub(crate) const MIP1_WIDTH: u32 = 2;
pub(crate) const MIP1_HEIGHT: u32 = 2;
pub(crate) const MIP1_LINE_WORDS: u32 = 1;
pub(crate) const MIP1_TEXELS: [u16; 4] = [0x0421, 0x2529, 0x4a4a, 0x6bde];

pub(crate) const fn set_tile_indexed(tile: u32, line_words: u32, tmem_word: u32) -> (u32, u32) {
    let (w0, _) = set_tile(line_words, tmem_word);
    (w0, tile << 24)
}

pub(crate) const fn set_tile_size_indexed(tile: u32, width: u32, height: u32) -> (u32, u32) {
    let (w0, w1) = set_tile_size(width, height);
    (w0, (tile << 24) | w1)
}

pub(crate) const fn load_tile_indexed(tile: u32, width: u32, height: u32) -> (u32, u32) {
    let (w0, w1) = load_tile(width, height);
    (w0, (tile << 24) | w1)
}

pub(crate) fn two_level_textured_triangle(other_modes: (u32, u32)) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        other_modes,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile_indexed(0, TEXTURE_LINE_WORDS, 0),
        set_tile_size_indexed(0, TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        set_texture_image(MIP1_WIDTH, MIP1_SOURCE),
        set_tile_indexed(1, MIP1_LINE_WORDS, MIP1_TMEM_WORD),
        set_tile_size_indexed(1, MIP1_WIDTH, MIP1_HEIGHT),
        (0xe600_0000, 0),
        load_tile_indexed(1, MIP1_WIDTH, MIP1_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(textured_triangle_pair());
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const SET_COMBINE_LOD_FRACTION: (u32, u32) = (0xfc16_902d, 0x8823_ffff);

pub(crate) fn lod_fraction_combiner_case(other_modes: (u32, u32)) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        other_modes,
        SET_COMBINE_LOD_FRACTION,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
    ]);
    words.extend(textured_triangle_pair());
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn one_tile_textured_triangle(other_modes: (u32, u32)) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        other_modes,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
    ]);
    words.extend(textured_triangle_pair());
    words.push((0xe900_0000, 0));
    words
}

// -----------------------------------------------------------------------
// slice zmode-deep (Pass 2)
// -----------------------------------------------------------------------

pub(crate) const fn other_modes_one_cycle_zmode(
    z_source_prim: bool,
    z_compare_en: bool,
    z_update_en: bool,
    z_mode: u32,
) -> (u32, u32) {
    let mut w1 = 0u32;
    if z_source_prim {
        w1 |= 1 << 2;
    }
    if z_compare_en {
        w1 |= 1 << 4;
    }
    if z_update_en {
        w1 |= 1 << 5;
    }
    w1 |= (z_mode & 0x3) << 10;
    (0xef00_00f0, w1)
}

pub(crate) const ZBUF_TIE_Z: u16 = 0x4000;

pub(crate) fn gen_zbuffer_zmode_inter_tie() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        other_modes_one_cycle_zmode(true, true, true, 1),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(set_prim_depth(ZBUF_TIE_Z, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(ZBUF_TIE_Z, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn gen_zbuffer_zmode_translucent_control() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        other_modes_one_cycle_zmode(true, true, true, 2),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(set_prim_depth(0x8000, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(0x1000, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const DECAL_BASE_Z: u16 = 0x2000;

pub(crate) fn gen_zbuffer_zmode_decal_correlated_wins() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        other_modes_one_cycle_zmode(true, true, true, 3),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(set_prim_depth(DECAL_BASE_Z, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(DECAL_BASE_Z, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const DECAL_ISOLATED_Z: u16 = 0x7000;

pub(crate) fn gen_zbuffer_zmode_decal_uncorrelated_control() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        other_modes_one_cycle_zmode(true, true, true, 3),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_prim_depth(DECAL_ISOLATED_Z, 0),
        (0xfa00_0000, ZBUF_NEAR_COLOR),
    ]);
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const PRIMDEPTH_CHAIN_FAR: u16 = 0xc000;
pub(crate) const PRIMDEPTH_CHAIN_MID: u16 = 0x6000;
pub(crate) const PRIMDEPTH_CHAIN_NEAR: u16 = 0x0800;
pub(crate) const ZBUF_MID_COLOR: u32 = 0x0000_ffff;

pub(crate) fn gen_zbuffer_primdepth_three_tier_chain() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        other_modes_one_cycle_zmode(true, true, true, 0),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(set_prim_depth(PRIMDEPTH_CHAIN_FAR, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(PRIMDEPTH_CHAIN_MID, 0));
    words.push((0xfa00_0000, ZBUF_MID_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push(set_prim_depth(PRIMDEPTH_CHAIN_NEAR, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) const GRADIENT_Z_LEFT: i32 = 0x4000 << 16;
pub(crate) const GRADIENT_DZDX: i32 = 0x0c00 << 16;
pub(crate) const PRIM_MID_Z: u16 = 0x5800;

pub(crate) fn gen_zbuffer_pixel_zplane_gradient_split() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_z_image(ZBUF_Z_IMAGE),
        SET_COMBINE_PRIMITIVE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.push(other_modes_one_cycle_zmode(false, true, true, 0));
    words.push((0xfa00_0000, ZBUF_FAR_COLOR));
    let z_words =
        |x_h: u32, x_l: u32, y_h: u32, y_l: u32, y_m: u32, dzdx: i32| -> Vec<(u32, u32)> {
            let yl = ((y_l as i32) << 2) as u16 as u32;
            let ym = ((y_m as i32) << 2) as u16 as u32;
            let yh = ((y_h as i32) << 2) as u16 as u32;
            vec![
                (0x0900_0000 | (1 << 23) | yl, (ym << 16) | yh),
                (x_l << 16, 0),
                (x_h << 16, 0),
                (x_l << 16, 0),
                (GRADIENT_Z_LEFT as u32, dzdx as u32),
                (0, 0),
            ]
        };
    words.extend(z_words(
        TRI_LEFT,
        TRI_RIGHT,
        TRI_TOP,
        TRI_BOTTOM,
        TRI_BOTTOM,
        GRADIENT_DZDX,
    ));
    words.extend(z_words(
        TRI_RIGHT,
        TRI_LEFT,
        TRI_TOP,
        TRI_BOTTOM,
        TRI_TOP,
        GRADIENT_DZDX,
    ));
    words.push(other_modes_one_cycle_zmode(true, true, true, 0));
    words.push(set_prim_depth(PRIM_MID_Z, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(
        TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM,
    ));
    words.extend(flat_triangle_words(
        TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP,
    ));
    words.push((0xe900_0000, 0));
    words
}

// -----------------------------------------------------------------------
// slice formats-wider (Pass 2)
// -----------------------------------------------------------------------

// NOTE: the four RGBA32-DESTINATION cases this slice originally proposed
// (fill / one-cycle / two-cycle / IA16 drawn onto an RGBA32 SetColorImage at
// a separate address) are NOT integrated. This runner's observation window is
// fixed at `FRAMEBUFFER` (RGBA16, `FRAMEBUFFER_BYTES` = pixels*2) and every
// backend is handed that same target address. An RGBA32 color image at a
// different address is never observed: RT64 and angrylion both leave the
// observed window untouched and "pass" vacuously (0 diff on all-STALE), while
// wgpu refuses on the STALE-prelude/color-image mismatch. Neither is a valid
// RGBA32 signal, so the cases are dropped rather than shipped as a vacuous
// pass masking a refusal. The remaining formats-wider cases below (I8
// fill-cycle control, CI8 two-cycle) draw into the observed window and give a
// real verdict.

pub(crate) fn i8_fill_vs_one_cycle_pair() -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let one_cycle = direct_format_textured_triangle(I8_SOURCE, 8, 4, 1, 1);
    let fill = gen_fill_frame(I8_EXPECTED[0], 3, TRI_LEFT, TRI_TOP, TRI_RIGHT, TRI_BOTTOM);
    (fill, one_cycle)
}

// `ci8_textured_triangle_two_cycle` was removed with its only caller,
// `gen-widerformats-ci8-triangle-two-cycle` (dropped in task 21 as
// all-three-differ-inspect-construction; see the drop note at the push site).

pub(crate) fn push_formats_wider_cases(
    push: &mut impl FnMut(u8, String, &'static str, Vec<(u32, u32)>),
) {
    // The four RGBA32-destination cases are intentionally omitted -- see the
    // note above `i8_fill_vs_one_cycle_pair`: this runner's fixed RGBA16
    // observation window cannot observe an RGBA32 color image at a separate
    // address, so those cases would be vacuous passes / wgpu refusals rather
    // than valid RGBA32 signal.
    {
        // Only the fill-cycle half is registered here: the one-cycle half of
        // `i8_fill_vs_one_cycle_pair` is byte-identical to the already-present
        // `gen-triangle-i8` case, so it is dropped per the no-duplicate rule.
        let (fill, _one_cycle) = i8_fill_vs_one_cycle_pair();
        push(
            7,
            "gen-widerformats-i8-fill-cycle-control".into(),
            "fill-cycle rectangle in I8's first-texel decoded colour, over the \
             same box the one-cycle I8 triangle (gen-triangle-i8) covers -- \
             fill-vs-one-cycle contrast isolated from TMEM/decode",
            fill,
        );
    }

    // DROPPED (task 21): `gen-widerformats-ci8-triangle-two-cycle` classified
    // all-three-differ-inspect-construction (wgpu 0x0001, RT64 0xf801, angrylion
    // 0x0843; wgpu_d=12, rt64_d=24 -- wgpu != RT64), refuting its own EXPECTED
    // "wgpu==RT64" note. It flipped a one-cycle CI8-TLUT triangle to two-cycle
    // WITHOUT supplying a matching two-cycle combine, so cycle 1 read the
    // unsettled second-cycle-texel path where wgpu and RT64 diverge -- masking,
    // not widening, the #20 CI8+TLUT S-plane divergence. That divergence is
    // already covered cleanly one-cycle by `gen-triangle-ci8-bilerp`
    // (shared-ported-bug, wgpu==RT64, d=24).
}
