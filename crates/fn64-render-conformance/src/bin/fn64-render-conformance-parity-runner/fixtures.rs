//! Hand-authored case fixtures, backend/type vocabulary (Authority, Case).

use super::*;

pub(crate) const RDRAM_LEN: usize = 8 * 1024 * 1024;
pub(crate) const COMMAND_START: u32 = 0x100;
pub(crate) const FRAMEBUFFER: u32 = 0x10_0000;

/// RT64 configures a real swap chain and a real Metal device. The 8x4 target
/// the wgpu runner's sweep uses is below anything RT64 will render into, so
/// the parity corpus uses a full 320x240 NTSC target -- the same extent the
/// RT64 deferred-history runner already proves RT64 renders at.
pub(crate) const WIDTH: u32 = 320;
pub(crate) const HEIGHT: u32 = 240;
pub(crate) const PIXEL_COUNT: u32 = WIDTH * HEIGHT;
pub(crate) const FRAMEBUFFER_BYTES: u32 = PIXEL_COUNT * 2;

pub(crate) const RED: u16 = 0xf801;
pub(crate) const GREEN: u16 = 0x07c1;
pub(crate) const BLUE: u16 = 0x003f;
pub(crate) const STALE: u16 = 0xffff;
pub(crate) const GUARD: u16 = 0x4211;

/// Whether RT64's answer for a case is evidence about the hardware.
///
/// This is the partition the whole metric turns on. It is declared per case
/// from what the case's commands actually exercise, and it is checked against
/// the command words by `authority_matches_the_commands` so a case cannot
/// claim authority it does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Authority {
    /// RT64 models this faithfully: command semantics, geometry, combiner and
    /// texture behaviour. A wgpu-vs-RT64 difference here is a wgpu finding.
    Rt64Authoritative,
    /// RT64 does not model this. Anti-aliasing, coverage-dependent blending,
    /// and dither. A difference here is NOT evidence against wgpu, because
    /// the oracle is the one not modelling the hardware.
    /// `docs/rt64/RT64-GUARD-AUDIT.md` C4-C6, U1-U3.
    CoverageDependentRt64NotAuthoritative,
    /// **HISTORICAL: the raw-triangle plane-scale disagreement, now fixed.**
    ///
    /// Kept as a named partition because the defect it describes was real,
    /// shipped, and is exactly the kind of thing that comes back. No case
    /// currently claims it -- `textured-triangle-point-sampled` is
    /// `Rt64Authoritative` and both lanes match its key.
    ///
    /// What it was: fn64's `texture_coordinates_s10_5` divided the plane by
    /// `PLANE_TO_TEXEL = 2^21` and returned a value its caller consumed as
    /// S10.5 through `TextureCoordinateS10_5::from_raw`, so the sampler
    /// applied its own `>>5` on top and the S10.5 `2^5` was counted TWICE --
    /// a plane of `2^26` per texel where hardware and RT64 use `2^21`.
    ///
    /// Hardware, from angrylion: `ss = s >> 16` to S10.5
    /// (`rasterizer.c:479`), `tcdiv_nopersp` applies no scale
    /// (`tcoord.c:1024`), `*S = locs >> 5` to whole texels
    /// (`tcoord.c:143`). The corpus found it: at `2^21` RT64 reproduced the
    /// key and wgpu read texel 0 everywhere; at `2^26` they swapped. Fixing
    /// `PLANE_TO_TEXEL` to `2^16` -- the plane->S10.5 divisor, leaving the
    /// `>>5` to the sampler where it belongs -- made all three agree.
    RawTrianglePlaneScaleDisagreement,
}

impl Authority {
    pub(crate) const fn wire(self) -> &'static str {
        match self {
            Self::Rt64Authoritative => "rt64-authoritative",
            Self::CoverageDependentRt64NotAuthoritative => "rt64-not-authoritative-coverage",
            Self::RawTrianglePlaneScaleDisagreement => "raw-triangle-plane-scale-disagreement",
        }
    }
}

pub(crate) struct Case {
    pub(crate) name: &'static str,
    /// Why this case is in the corpus at all -- what a disagreement here
    /// would mean.
    pub(crate) intent: &'static str,
    pub(crate) authority: Authority,
    pub(crate) commands: Vec<(u32, u32)>,
    /// Hand-derived expected pixel by linear index. Never captured from a
    /// backend.
    pub(crate) expected: fn(u32) -> u16,
}

/// `SetScissor` over a whole-pixel box, in the wire's own field order.
///
/// **The bounds are SPLIT ACROSS BOTH WORDS**, and getting that wrong is
/// silent. The public libultra macro `gDPSetScissor`
/// (`ultra64/gbi.h:4794-4817`) packs the UPPER-LEFT into word 0 and the
/// LOWER-RIGHT into word 1, each bound as `(int)((float)(coord) * 4.0f)`
/// -- quarter-pixel units in two 12-bit fields. A decoder therefore reads:
///
/// ```text
/// clip.xh = (w0 >> 12) & 0xfff     clip.xl = (w1 >> 12) & 0xfff
/// clip.yh = (w0 >>  0) & 0xfff     clip.yl = (w1 >>  0) & 0xfff
/// ```
///
/// All four are S10.2, so a whole pixel is `<< 2`.
///
/// Every scissor in this corpus previously packed the LOWER-RIGHT into word
/// 0 and left word 1 zero, which decodes as an inverted box -- upper-left
/// `(160, 240)` to lower-right `(0, 0)` for the half-width case. That is a
/// degenerate input, and the two backends answered it differently:
/// `scissor-narrower-than-rect` reported RT64 painting 38,400 pixels the key
/// excluded, which read as an RT64 finding and was a fixture defect.
pub(crate) const fn set_scissor(ulx: u32, uly: u32, lrx: u32, lry: u32) -> (u32, u32) {
    (
        0xed00_0000 | ((ulx * 4) << 12) | (uly * 4),
        ((lrx * 4) << 12) | (lry * 4),
    )
}

pub(crate) const fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

/// `SetOtherModes` for fill cycle with no AA, no coverage read, no dither.
/// This is the word every RT64-authoritative FILL case uses, and the thing
/// the coverage-dependent cases deliberately change.
pub(crate) const OTHER_MODES_FILL_NO_AA: (u32, u32) = (0xef30_00f0, 0);

/// `SetOtherModes` for a ONE-CYCLE textured draw, carrying exactly the same
/// no-AA / no-dither / no-coverage properties [`OTHER_MODES_FILL_NO_AA`]
/// does, so a textured case stays inside the RT64-authoritative partition.
///
/// Derived from the public libultra encoding, not from any emulator:
/// `gDPSetOtherMode` packs `w0 = G_RDPSETOTHERMODE << 24 | mode0`, and the
/// `G_*` field constants in `ultra64/gbi.h` supply every position below.
/// With `G_CYC_1CYCLE`, `G_TP_NONE`, `G_TT_NONE`, `G_TF_POINT` all zero and
/// `G_CD_DISABLE = 3 << 6`, `G_AD_DISABLE = 3 << 4`, `mode0 = 0xf0`, giving
/// exactly the `(0xef00_00f0, 0)` below. **Re-derived and confirmed to
/// reproduce bit-for-bit from the public header alone**, so this literal is
/// independently obtainable:
///
/// | field | bits | value | meaning |
/// |---|---|---|---|
/// | `cycle_type` | w0 21:20 | 0 | one cycle |
/// | `persp_tex_en` | w0 19 | 0 | non-perspective: the `/2^21` plane path |
/// | `en_tlut` | w0 15 | 0 | TLUT off; the tile's own format decodes |
/// | `sample_type` | w0 13 | 0 | POINT sample, no bilerp |
/// | `rgb_dither_sel` | w0 7:6 | 3 | dither disabled |
/// | `alpha_dither_sel` | w0 5:4 | 3 | dither disabled |
/// | `antialias_en` | w1 3 | 0 | AA off |
/// | `alpha_cvg_select` | w1 13 | 0 | coverage not substituted for alpha |
/// | `cvg_times_alpha` | w1 12 | 0 | no coverage multiply |
///
/// Point sampling is the load-bearing choice: it makes the expected pixel a
/// single named texel rather than a filter of four, so the key stays
/// hand-derivable. It also keeps the case clear of the three-nearest filter,
/// whose tie-break this repo records as a preserved convention rather than a
/// verified hardware fact (`tmem/sample.rs`'s
/// `filter_three_nearest_committed_cell`).
pub(crate) const OTHER_MODES_ONE_CYCLE_TEXTURED: (u32, u32) = (0xef00_00f0, 0);

/// The two-cycle textured twin: only public `G_CYC_2CYCLE` bit 20 differs.
/// Point sampling, disabled dithering, and disabled coverage modes remain
/// identical to [`OTHER_MODES_ONE_CYCLE_TEXTURED`].
pub(crate) const OTHER_MODES_TWO_CYCLE_TEXTURED: (u32, u32) = (0xef10_00f0, 0);

/// The perspective-textured twin: only public `G_TP_PERSP` bit 19 differs.
/// Point sampling, disabled dithering, and disabled coverage modes remain
/// identical to [`OTHER_MODES_ONE_CYCLE_TEXTURED`].
pub(crate) const OTHER_MODES_ONE_CYCLE_TEXTURED_PERSPECTIVE: (u32, u32) =
    (OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 19), 0);

/// `SetOtherModes` for a ONE-CYCLE fill: byte-identical to
/// [`OTHER_MODES_FILL_NO_AA`] except the cycle-type field.
///
/// The two words differ only in w0 bits 21:20 (`G_MDSFT_CYCLETYPE`), which
/// carry `G_CYC_FILL = 3` in the fill constant and `G_CYC_1CYCLE = 0` here.
/// Every other property -- no AA, no dither, no coverage read -- is
/// unchanged, so this case stays in the RT64-authoritative partition for
/// exactly the reasons [`OTHER_MODES_FILL_NO_AA`] documents.
pub(crate) const OTHER_MODES_ONE_CYCLE_NO_AA: (u32, u32) = (0xef00_00f0, 0);

/// One-cycle, forced general blending with `P = M = Combined`,
/// `A = CombinedAlpha`, and `B = One`.
///
/// The low word is derived from the public `G_BL_*` selector packing:
/// `B = One` is selector 2 in cycle 1's bits 18:19 and `FORCE_BL` is bit 14;
/// every other selector is zero. This is deliberately not RT64's duplicate
/// `P == M && B == OneMinusA` passthrough: `B == One` makes the numerator
/// overflow classification true.
pub(crate) const OTHER_MODES_ONE_CYCLE_BLEND_OVERFLOW: (u32, u32) =
    (0xef00_00f0, (2 << 18) | (1 << 14));

/// One-cycle forced blending that selects BlendColor for P, input alpha for
/// A, combiner output for M, and zero for B. With opaque combiner alpha and
/// B=zero the M term vanishes, so the blender outputs BlendColor directly.
/// M=clr_in (0) avoids clr_mem (1) which would require IM_RD.
pub(crate) const OTHER_MODES_ONE_CYCLE_BLEND_COLOR: (u32, u32) = (0xef00_00f0, 0x800c_4000);

/// The FogColor twin of [`OTHER_MODES_ONE_CYCLE_BLEND_COLOR`], selecting
/// FogColor for P while retaining the same opaque-alpha and zero-B terms.
pub(crate) const OTHER_MODES_ONE_CYCLE_FOG_COLOR: (u32, u32) = (0xef00_00f0, 0xc00c_4000);

/// The colour the one-cycle band is asked to paint, and the seed it must
/// replace. Deliberately not `STALE`, so "the band did nothing" and "the
/// band worked" are different pictures.
pub(crate) const BAND_FILL_COLOR: u16 = 0xf801;

/// The seed the band must overwrite. Deliberately NOT white: the combiner
/// WM2000 stages resolves to white, so a white seed would make this case
/// pass even if the command were dropped entirely.
pub(crate) const BAND_SEED: u16 = 0x0843;

/// What the measured combiner produces with `shade = texel0 = texel1 = 0`.
/// Both RT64 and the reference backend were observed to write this.
pub(crate) const BAND_COMBINED_OUTPUT: u16 = 0xffff;

/// The band's own rows, chosen inside the target and away from every edge so
/// a clipping defect cannot be mistaken for a dropped command.
pub(crate) const BAND_TOP: u32 = 64;
pub(crate) const BAND_BOTTOM: u32 = 127;

/// A **one-cycle** `G_FILLRECT` band over a `STALE`-seeded target.
///
/// **What this case is for.** WM2000 clears its framebuffer with roughly
/// sixty full-width `G_FILLRECT` bands per frame issued in ONE-CYCLE mode,
/// not fill cycle. fn64's fill executor is reached only for
/// `CycleType::Fill`, so those bands stage no framebuffer write at all: ABI
/// dispatch then takes `commit_zero_guest_writes`, whose RDRAM copyback is
/// guarded by `if !commit_writes.is_empty()`, and VI scans out the
/// untouched framebuffer. The visible result on the AKI, THQ, JAKKS and
/// Asmik logo screens is stale content surviving wherever a later primitive
/// does not happen to overwrite it.
///
/// RT64 does not treat cycle type as a gate on whether to draw:
/// `RDP::fillRect` calls `drawRect` unconditionally and the cycle check only
/// ORs `lrx |= 3` for COPY/FILL (`rt64_rdp.cpp:1043`), so the rectangle
/// enters the ordinary draw pipeline as two triangles with zero vertex
/// colour (`rectColorFloats`, `rt64_rdp.cpp:1253`).
///
/// **The key is deliberately NOT asserted here.** What RT64's zero-shade
/// rectangle resolves to under this combiner is exactly the open question,
/// so this case is authored to expose the DIFFERENCE between the two
/// backends rather than to encode a predicted answer. The `expected`
/// function below states the seed, which is what a backend that drops the
/// command produces -- so wgpu matching the key while RT64 differs is
/// itself the finding.
pub(crate) fn one_cycle_fill_band() -> Vec<(u32, u32)> {
    // The Fill-cycle seed. This half is already proven: `full-target-red`
    // and the textured cases all rely on it.
    let mut words = one_fill(BAND_SEED, 0, 0, WIDTH - 1, HEIGHT - 1);
    // Drop the seed's own FullSync; one closes the whole packet.
    words.pop();
    words.extend([
        // The ONLY difference from a working fill: the cycle type.
        OTHER_MODES_ONE_CYCLE_NO_AA,
        // **The combined path needs the combiner state a fill cycle never
        // reads.** These three words are lifted verbatim from the measured
        // WM2000 packet in `docs/WM2000-FILLRECT-EVIDENCE.txt`, which stages
        // exactly this before its sixty one-cycle bands: `SetPrimColor`,
        // `SetEnvColor`, then the `SetCombine` immediately preceding them.
        // Using the ROM's own words keeps the fixture honest about what the
        // game actually programs, rather than inventing a combiner that
        // happens to be convenient.
        (0xfa00_0000, 0xffff_ffef),
        (0xfb00_0000, 0xffff_ffff),
        (0xfcff_ffff, 0xfffd_f6fb),
        (0xf700_0000, (BAND_FILL_COLOR as u32) * 0x1_0001),
        fill_rect(WIDTH - 1, BAND_BOTTOM, 0, BAND_TOP),
        (0xe900_0000, 0),
    ]);
    words
}

/// The hand-derived key.
///
/// Outside the band the seed survives. INSIDE it, both engines were measured
/// to resolve WM2000's own combiner to white with `shade = texel0 = texel1 =
/// 0`, and the band uses the EXCLUSIVE lower/right edge that one-/two-cycle
/// mode takes (`crates/fn64-render-reference/src/raster/draw.rs:113-135`),
/// so the final column and row keep the seed.
///
/// **The seed is deliberately not white.** An earlier revision seeded
/// `STALE` (0xffff), which is exactly what this combiner paints -- so the
/// case passed whether or not the command executed at all, and could not
/// tell "wrote white" from "wrote nothing". That is the
/// fixture-cannot-detect-the-bug trap `docs/rt64/RT64-WM2000-HARNESS-TRAPS.md`
/// records; seeding a distinct colour is what gives this case teeth.
pub(crate) fn one_cycle_fill_band_expected(index: u32) -> u16 {
    let y = index / WIDTH;
    let x = index % WIDTH;
    let inside_rows = y >= BAND_TOP && y < BAND_BOTTOM;
    let inside_columns = x < WIDTH - 1;
    if inside_rows && inside_columns {
        BAND_COMBINED_OUTPUT
    } else {
        BAND_SEED
    }
}

pub(crate) fn one_fill(color: u16, ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
    vec![
        OTHER_MODES_FILL_NO_AA,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xf700_0000, (color as u32) * 0x1_0001),
        fill_rect(lrx, lry, ulx, uly),
        (0xe900_0000, 0),
    ]
}

// ---------------------------------------------------------------------------
// Textured cases
// ---------------------------------------------------------------------------
//
// **Why these exist.** The corpus was fill-rectangles only, which means it
// could not see any defect in the path that turns a texture coordinate into a
// texel: TMEM addressing, the tile descriptor's format/size/line fields, the
// palette, or the byte-lane mapping. That is exactly the layer
// `docs/rt64/RT64-WM2000-TEXTURE-STATE.md` bounded WM2000's remaining defect to,
// and localising it took a 20-minute ROM run plus a human reading a PNG.
//
// Every expected texel below is derived BY HAND from the RGBA16 wire layout
// and the TMEM addressing rule, never from any fn64 implementation. See
// `TEXTURE_TEXELS` for the derivation.

/// Where the texture's source pixels live in the staged RDRAM image. Clear of
/// the command stream (`0x100`) and the colour target (`0x10_0000`).
pub(crate) const TEXTURE_SOURCE: u32 = 0x2000;

/// The texture is 4x2 RGBA16 texels, so one row is 8 bytes = one 64-bit TMEM
/// word, and `SetTile`'s `line` field is 1.
pub(crate) const TEXTURE_WIDTH: u32 = 4;
pub(crate) const TEXTURE_HEIGHT: u32 = 2;
pub(crate) const TEXTURE_LINE_WORDS: u32 = 1;

/// The eight texels staged into TMEM, row-major.
///
/// **Chosen so every one is distinguishable from every other**, and so a
/// wrong TMEM address, a wrong row, a swapped 4-byte bank or a wrong byte
/// lane each produce a DIFFERENT visible answer rather than coinciding. In
/// particular the two rows differ in every texel, so reading row 0 where row
/// 1 was meant is visible; and no texel is a byte-swap of another, so a
/// lane error cannot alias onto a correct value.
///
/// Values are RGBA16 (5/5/5/1) and are stated as the literal 16-bit words the
/// guest writes big-endian into RDRAM.
pub(crate) const TEXTURE_TEXELS: [u16; 8] = [
    0xf801, // row 0, col 0: r=31 g=0  b=0  a=1
    0x07c1, // row 0, col 1: r=0  g=31 b=0  a=1
    0x003f, // row 0, col 2: r=0  g=0  b=31 a=1
    // **Deliberately NOT 0xffff.** `STALE` is 0xffff, so a texel equal to it
    // would make "drew this pixel correctly" and "did not draw this pixel at
    // all" the same observation -- a backend that skipped the column would
    // pass. Measured: with 0xffff here, the textured-triangle case reported
    // only 9 differing pixels of 12 because column 4's texel aliased the
    // background. 0x7fff keeps the all-high-channel shape without the alias.
    0x7fff, // row 0, col 3: r=15 g=31 b=31 a=1
    0x8421, // row 1, col 0: r=16 g=16 b=16 a=1
    0xc631, // row 1, col 1: r=24 g=24 b=24 a=1
    0x4211, // row 1, col 2: r=8  g=8  b=8  a=1
    0xfc01, // row 1, col 3: r=31 g=0  b=0  a=1 with g's top bit set
];

/// **The wide texture, for the `line > 1` case.** Its own source address so
/// the 4x2 image above is untouched and the two committed textured cases
/// cannot regress when this one changes.
///
/// 8x2 RGBA16 texels: one row is 16 bytes = TWO 64-bit TMEM words, so
/// `SetTile`'s `line` is 2. That is the field every case above leaves at 1,
/// and it is the multiplier in angrylion's own row address
/// (`tile->line * (t & 0xff)`, `tmem.c:65`) -- a stride defect is invisible
/// while `line` is 1, because a wrong multiplier times one row index is
/// still the right address on row 0.
pub(crate) const WIDE_SOURCE: u32 = 0x3000;
pub(crate) const WIDE_WIDTH: u32 = 8;
pub(crate) const WIDE_HEIGHT: u32 = 2;
pub(crate) const WIDE_LINE_WORDS: u32 = 2;

/// Sixteen distinct RGBA16 texels, row-major.
///
/// **Row 1 is what this case is for.** Every row-1 texel has bit 0x0040 set
/// (green's low bit) and no row-0 texel does, so reading row 0 where row 1
/// was meant is visible in one bit even if the columns happen to line up.
/// Within a row every texel differs from every other, so a wrong column is
/// visible too, and no texel is a byte-swap of another.
pub(crate) const WIDE_TEXELS: [u16; 16] = [
    // row 0: bit 0x0040 clear in every entry. 0x7fff rather than 0xffff for
    // the same anti-alias reason `TEXTURE_TEXELS` states.
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01,
    // row 1: bit 0x0040 set in every entry.
    0xf841, 0x0641, 0x0079, 0xffbf, 0x8461, 0xc671, 0x4251, 0xfc41,
];

// Independent, hand-derived keys. Keep these separate from `WIDE_TEXELS` so
// mutating an expected entry cannot also mutate the staged texture source.
pub(crate) const LOAD_BLOCK_LINEAR_EXPECTED: [u16; 8] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01,
];
pub(crate) const LOAD_BLOCK_DXT_EXPECTED: [u16; 16] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0xf841, 0x0641, 0x0079, 0xffbf,
    0x8461, 0xc671, 0x4251, 0xfc41,
];
pub(crate) const TEXRECT_FLIP_EXPECTED: [u16; 16] = [
    0xf801, 0x8421, 0xf841, 0x8461, 0x07c1, 0xc631, 0x0641, 0xc671, 0x003f, 0x4211, 0x0079, 0x4251,
    0x7fff, 0xfc01, 0xffbf, 0xfc41,
];

/// A tall RGBA16 strip reproducing WM2000's measured texrect state without
/// carrying any game content. The 64-texel source row occupies 16 TMEM
/// words, while the base tile deliberately declares the measured
/// `line = 17`, leaving one word of padding between rows. Fourteen rows make
/// a two-pixel-per-row displacement impossible to mistake for a boundary
/// tie-break.
pub(crate) const SKEW_SOURCE: u32 = 0x5000;
pub(crate) const SKEW_WIDTH: u32 = 64;
pub(crate) const SKEW_HEIGHT: u32 = 14;
pub(crate) const SKEW_LOW_T_ODD: u32 = 95;
pub(crate) const SKEW_LINE_WORDS: u32 = 17;
pub(crate) const SKEW_BAR_LEFT: u32 = 8;
pub(crate) const SKEW_BAR_RIGHT: u32 = 56;

pub(crate) const fn skew_texel(x: u32) -> u16 {
    if x >= SKEW_BAR_LEFT && x < SKEW_BAR_RIGHT {
        RED
    } else {
        BLUE
    }
}

pub(crate) fn skew_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < SKEW_WIDTH && y < SKEW_HEIGHT {
        skew_texel(x)
    } else {
        STALE
    }
}

/// The expected pixel for the wide case, by linear target index.
///
/// Half-open on both axes, the texrect rule. Inside, pixel `(x, y)` reads
/// texel `(x, y)` of the 8x2 image; outside, the seeded `STALE` survives.
/// Arithmetic over [`WIDE_TEXELS`] -- no backend is consulted.
pub(crate) fn wide_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < WIDE_WIDTH && y < WIDE_HEIGHT {
        WIDE_TEXELS[(y * WIDE_WIDTH + x) as usize]
    } else {
        STALE
    }
}

/// The command list for the wide (`line = 2`) textured case.
///
/// Same shape as [`one_textured_rect`] -- seed fill, state, load, draw, sync
/// -- but every texture parameter comes from the WIDE constants, so the tile
/// carries `line = 2` and the rectangle is 8 columns wide.
pub(crate) fn wide_textured_rect() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(WIDE_WIDTH, WIDE_SOURCE),
        set_tile(WIDE_LINE_WORDS, 0),
        set_tile_size(WIDE_WIDTH, WIDE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(WIDE_WIDTH, WIDE_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle(0, 0, WIDE_WIDTH, WIDE_HEIGHT));
    words.push((0xe900_0000, 0));
    words
}

/// Where the textured rectangle lands on the target: 4 columns by 2 rows at
/// the origin, so the rectangle steps exactly one texel per pixel on both
/// axes and pixel `(x, y)` samples texel `(x, y)` with no filtering
/// ambiguity.
///
/// **The high edges are EXCLUSIVE, unlike `G_FILLRECT`'s.** This is the one
/// place the two rectangle commands disagree, and getting it wrong is silent:
/// the draw simply covers one fewer row and column, which reads as a texel
/// defect rather than a fixture defect. fn64 pins the rule in
/// `targets/texrect.rs` -- "the fill rule is inclusive and the texrect rule
/// is half-open, so the fill rectangle is exactly one pixel larger on each
/// axis" -- and a test there asserts the two extents never coincide.
///
/// An earlier revision of this fixture wrote `TEXTURE_HEIGHT - 1` here, by
/// analogy with the fill cases above. That covered row 0 only, so every
/// backend correctly left row 1 as `STALE` while the key demanded texels
/// there, and all three lanes reported `matches_key: false` against a key
/// that was itself wrong. RT64 agreeing with wgpu and the reference about
/// row 1 is what exposed it.
pub(crate) const TEXRECT_ULX: u32 = 0;
pub(crate) const TEXRECT_ULY: u32 = 0;
pub(crate) const TEXRECT_LRX: u32 = TEXTURE_WIDTH;
pub(crate) const TEXRECT_LRY: u32 = TEXTURE_HEIGHT;

/// The expected pixel for a textured case, by linear target index.
///
/// Inside the rectangle a pixel reads its own texel; outside it the seeded
/// `STALE` survives. This is the whole key, and it is arithmetic over
/// [`TEXTURE_TEXELS`] -- no backend is consulted.
pub(crate) fn textured_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    // Half-open, matching the wire rule in `TEXRECT_LRX`'s own doc.
    if x < TEXRECT_LRX && y < TEXRECT_LRY {
        TEXTURE_TEXELS[(y * TEXTURE_WIDTH + x) as usize]
    } else {
        STALE
    }
}

/// `SetTextureImage` naming the staged source as RGBA16.
///
/// Wire: `format` 0 (RGBA) at bits 23:21, `size` 2 (16-bit) at 20:19, and a
/// width field of `width - 1` at 11:0 -- the public libultra encoding, from
/// `gDPSetTextureImage`/`gSetImage` and the `G_IM_FMT_*`/`G_IM_SIZ_*`
/// constants in `ultra64/gbi.h`. Re-derived and confirmed to reproduce
/// bit-for-bit from the header alone.
pub(crate) const fn set_texture_image(width: u32, address: u32) -> (u32, u32) {
    (0xfd00_0000 | (2 << 19) | (width - 1), address)
}

/// `SetTile` for tile 0, RGBA16, at TMEM word 0.
///
/// Wire, from the public libultra `gDPSetTile` encoding (`ultra64/gbi.h`):
/// `format` 23:21, `size` 20:19, `line` 17:9, `tmem` 8:0 in word 0; `tile`
/// 26:24, `palette` 23:20, and the S/T clamp/mirror/mask/shift fields in
/// word 1. Re-derived and confirmed to reproduce bit-for-bit from the header
/// alone. Everything not named here is zero: no palette and no mirror.
///
/// **Correction.** This comment used to say the zero `mask_s`/`mask_t`
/// "forces the CLAMP arm". It does not: `ultra64/gbi.h:323-326` defines
/// `G_TX_WRAP = 0 << 1` and `G_TX_CLAMP = 1 << 1`, so the zero encoding is
/// WRAP. A zero mask still pins addressing -- with no mask bits the wrapped
/// coordinate cannot move -- so the fixture's intent survives, but the
/// stated reason was wrong. Found while re-grounding this file's citations
/// on allowed sources.
pub(crate) const fn set_tile(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word, 0)
}

/// Tile 0 with explicit S/T clamp and a two-bit S mask for a four-texel row.
/// Public `gDPSetTile` places T mode at 19:18, S mode at 9:8 and S mask at
/// 7:4. `G_TX_CLAMP = 2`; mask 2 preserves columns 0..3 after clamping.
pub(crate) const fn set_tile_clamped_four_texels(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (
        0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word,
        (2 << 18) | (2 << 8) | (2 << 4),
    )
}

/// `SetTileSize` for tile 0 covering the whole texture.
///
/// All four coordinates are S10.2 and both high edges are INCLUSIVE, so a
/// `w`-texel wide tile has `high_s = (w - 1) << 2`.
pub(crate) const fn set_tile_size(width: u32, height: u32) -> (u32, u32) {
    (0xf200_0000, (((width - 1) * 4) << 12) | ((height - 1) * 4))
}

/// `LoadTile` for tile 0 covering the whole texture, in the same S10.2
/// inclusive form as `SetTileSize`.
///
/// LoadTile rather than LoadBlock deliberately: LoadBlock's row advance is
/// driven by DXT and its `line` interacts with it, so a multi-row LoadBlock
/// whose rows stay contiguous is not expressible at `line = 1`. LoadTile
/// states its rows directly, which is what a fixture wants.
pub(crate) const fn load_tile(width: u32, height: u32) -> (u32, u32) {
    (0xf400_0000, (((width - 1) * 4) << 12) | ((height - 1) * 4))
}

/// `TextureRectangle` sampling tile 0, one texel per pixel on both axes.
///
/// Wire: word 0 carries `lrx` at 23:12 and `lry` at 11:0 in S10.2; word 1
/// carries `tile` at 26:24, `ulx` at 23:12 and `uly` at 11:0. Words 2 and 3
/// carry the S/T origin in S10.5 and the per-pixel DsDx/DtDy in S5.10.
/// `1 << 10` is exactly one texel per pixel.
pub(crate) fn texture_rectangle(ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
    vec![
        (
            0xe400_0000 | ((lrx * 4) << 12) | (lry * 4),
            ((ulx * 4) << 12) | (uly * 4),
        ),
        (0, (1 << 26) | (1 << 10)),
    ]
}

/// `SetTileSize`/`LoadTile` bounds with a nonzero T origin. Coordinates are
/// S10.2 and the high edge remains inclusive.
pub(crate) const fn tile_bounds_at(opcode: u32, width: u32, height: u32, low_t: u32) -> (u32, u32) {
    (
        opcode | (low_t * 4),
        (((width - 1) * 4) << 12) | ((low_t + height - 1) * 4),
    )
}

pub(crate) const fn set_tile_size_at(width: u32, height: u32, low_t: u32) -> (u32, u32) {
    tile_bounds_at(0xf200_0000, width, height, low_t)
}

pub(crate) const fn load_tile_at(width: u32, height: u32, low_t: u32) -> (u32, u32) {
    tile_bounds_at(0xf400_0000, width, height, low_t)
}

/// The same one-texel-per-pixel texrect with its S10.5 T origin aligned to a
/// nonzero tile origin. Subtracting the tile's S10.2 `low_t` therefore starts
/// the draw on tile-relative row zero.
pub(crate) fn texture_rectangle_at_t(
    ulx: u32,
    uly: u32,
    lrx: u32,
    lry: u32,
    low_t: u32,
) -> Vec<(u32, u32)> {
    let mut words = texture_rectangle(ulx, uly, lrx, lry);
    words[1].0 = low_t << 5;
    words
}

/// `SetCombine` selecting `(Zero - Zero) * Zero + Texel0` in BOTH the colour
/// and the alpha pipe, for both cycles.
///
/// A rectangle command carries no shade attributes, so the reset combiner --
/// which selects SHADE -- is not a legal program for one; the reference lane
/// refuses it by name. Texel0 passthrough is what makes the drawn pixel the
/// sampled texel and nothing else, which is what lets the key below be a
/// single named texel.
///
/// **BOTH cycles are set**, because in one-cycle mode the RDP evaluates the
/// SECOND cycle's fields; leaving cycle 1 at its reset value makes it select
/// `Combined` before any first-cycle result exists, which the reference lane
/// refuses by name.
///
/// Packed by hand from angrylion's `rdp_set_combine`
/// (`src/core/n64video/rdp/combiner.c:522-539`), which is the authority for
/// every bit position:
///
/// | field | word | bits |
/// |---|---|---|
/// | `sub_a_rgb0` / `sub_a_rgb1` | w0 | 23:20 / 8:5 |
/// | `mul_rgb0` / `mul_rgb1` | w0 | 19:15 / 4:0 |
/// | `sub_a_a0` / `mul_a0` | w0 | 14:12 / 11:9 |
/// | `sub_b_rgb0` / `sub_b_rgb1` | w1 | 31:28 / 27:24 |
/// | `sub_a_a1` / `mul_a1` | w1 | 23:21 / 20:18 |
/// | `add_rgb0` / `add_rgb1` | w1 | 17:15 / 8:6 |
/// | `sub_b_a0` / `sub_b_a1` | w1 | 14:12 / 5:3 |
/// | `add_a0` / `add_a1` | w1 | 11:9 / 2:0 |
///
/// and the `Zero` encodings from the same file's input tables (`:6-100`):
/// sub_a RGB and sub_b RGB take `Zero` at code >= 8, mul RGB at code >= 16,
/// add RGB at code 7, and every alpha input at code 7. `Texel0` is code 1 in
/// the add-RGB and add-alpha tables alike.
pub(crate) const SET_COMBINE_TEXEL0: (u32, u32) = (0xfc88_7f10, 0x88fc_f279);

/// `SetCombine` selecting `(Zero - Zero) * Zero + Shade` in BOTH the colour
/// and the alpha pipe, for both cycles -- the shade-passthrough twin of
/// [`SET_COMBINE_TEXEL0`].
///
/// Word 0 (`sub_a`/`mul` for cycle 0, `sub_b` for both cycles) is untouched:
/// none of those fields differ between the two programs. Only the four
/// "add" fields in word 1 change, from Texel0's code `1` to Shade's code
/// `4`, at the same bit positions the table on [`SET_COMBINE_TEXEL0`]
/// documents (`add_rgb0` 17:15, `add_rgb1` 8:6, `add_a0` 11:9, `add_a1`
/// 2:0). Re-deriving `SET_COMBINE_TEXEL0`'s own word 1 confirms all four
/// fields read `1` there, so flipping just those nibbles from `1` to `4`
/// gives `0x88fe_793c`.
pub(crate) const SET_COMBINE_SHADE: (u32, u32) = (0xfc88_7f10, 0x88fe_793c);

// ---------------------------------------------------------------------------
// Direct texture formats
// ---------------------------------------------------------------------------
//
// The N64 Programming Manual's "Texture Image Types and Format" table and
// texture-unit list define exactly ten legal pairs: RGBA16/32, YUV16, CI4/8,
// IA4/8/16 and I4/8 (chapter 13, pp. 189 and 216). The cases in this file now
// cover that complete matrix without importing a captured packet or an answer
// from either backend.

pub(crate) const RGBA32_SOURCE: u32 = 0x4700;
pub(crate) const IA8_SOURCE: u32 = 0x4200;
pub(crate) const IA4_SOURCE: u32 = 0x4300;
pub(crate) const IA16_SOURCE: u32 = 0x4400;
pub(crate) const I4_SOURCE: u32 = 0x4500;
pub(crate) const I8_SOURCE: u32 = 0x4600;
pub(crate) const YUV16_SOURCE: u32 = 0x4800;

/// Eight opaque IA8 texels. High nibble is intensity, low nibble is alpha;
/// fixing alpha at `0xf` makes any accidental I8 interpretation visible.
pub(crate) const IA8_BYTES: [u8; 8] = [0x1f, 0x2f, 0x3f, 0x4f, 0x5f, 0x6f, 0x7f, 0x8f];
/// Seven opaque IA4 texels, packed high-nibble first. The final low nibble is
/// padding outside the 7-pixel tile and therefore must never be sampled.
pub(crate) const IA4_BYTES: [u8; 4] = [0x13, 0x57, 0x9b, 0xd0];
/// Eight big-endian IA16 texels: one intensity byte then opaque alpha.
pub(crate) const IA16_BYTES: [u8; 16] = [
    0x08, 0xff, 0x28, 0xff, 0x48, 0xff, 0x68, 0xff, 0x88, 0xff, 0xa8, 0xff, 0xc8, 0xff, 0xe8, 0xff,
];
/// I4 is four-bit intensity replicated into RGB and alpha.
pub(crate) const I4_BYTES: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
/// I8 is one byte replicated into RGB and alpha. Values straddle successive
/// eight-count intensity boundaries so every five-bit RGB step is named.
pub(crate) const I8_BYTES: [u8; 8] = [0x08, 0x1c, 0x28, 0x3c, 0x48, 0x5c, 0x68, 0x7c];
/// Two big-endian RGBA32 texels. Every channel is authored on an eight-count
/// boundary so RGBA32 -> RGBA16 quantization is exact and hand-checkable.
pub(crate) const RGBA32_BYTES: [u8; 8] = [0x10, 0x28, 0x40, 0xff, 0x50, 0x68, 0x80, 0xff];
/// Four YUV16 pairs in the public `Y0,U,Y1,V` wire order. Neutral chroma makes
/// the texture filter's R'/G'/B' channels equal the selected Y regardless of
/// its conversion coefficients; every Y still differs from the seed.
pub(crate) const YUV16_BYTES: [u8; 16] = [
    0x10, 0x80, 0x28, 0x80, 0x40, 0x80, 0x58, 0x80, 0x70, 0x80, 0x88, 0x80, 0xa0, 0x80, 0xb8, 0x80,
];

/// Hand-derived RGBA16 target words for [`IA8_BYTES`].
///
/// For the first texel, wire byte `0x1f` splits into intensity `i4 = 1` and
/// alpha `a4 = 15`. Nibble replication gives `i8 = (1 << 4) | 1 = 17` and
/// `a8 = (15 << 4) | 15 = 255`. With dither disabled, RGBA16 keeps the upper
/// five intensity bits: `i5 = 17 >> 3 = 2`. Replicating that gray value into
/// R/G/B and retaining opaque coverage gives
/// `(2 << 11) | (2 << 6) | (2 << 1) | 1 = 0x1085`. The remaining entries use
/// the same arithmetic for intensity nibbles 2 through 8.
pub(crate) const IA8_EXPECTED: [u16; 8] = [
    0x1085, 0x2109, 0x318d, 0x4211, 0x5295, 0x6319, 0x739d, 0x8c63,
];
/// IA4 uses three intensity bits plus one alpha bit. For `0xb`, `i3 = 5`
/// expands to `0xb6`, `i5 = 0xb6 >> 3 = 22`, and opaque RGBA16 gray is
/// `(22 << 11)|(22 << 6)|(22 << 1)|1 = 0xb5ad`.
pub(crate) const IA4_EXPECTED: [u16; 7] = [0x0001, 0x2109, 0x4a53, 0x6b5b, 0x94a5, 0xb5ad, 0xdef7];
/// IA16 is already one intensity byte followed by one alpha byte. These
/// intensities quantize to five bits 1,5,9,...,29 and alpha stays opaque.
pub(crate) const IA16_EXPECTED: [u16; 8] = [
    0x0843, 0x294b, 0x4a53, 0x6b5b, 0x8c63, 0xad6b, 0xce73, 0xef7b,
];
pub(crate) const I4_EXPECTED: [u16; 8] = [
    0x1085, 0x2109, 0x318d, 0x4211, 0x5295, 0x6319, 0x739d, 0x8c63,
];
pub(crate) const I8_EXPECTED: [u16; 8] = [
    0x0843, 0x18c7, 0x294b, 0x39cf, 0x4a53, 0x5ad7, 0x6b5b, 0x7bdf,
];
/// Hand-derived RGBA16 target words for [`RGBA32_BYTES`]. For texel zero,
/// `r5=0x10>>3=2`, `g5=0x28>>3=5`, `b5=0x40>>3=8`, and opaque alpha gives
/// `(2<<11)|(5<<6)|(8<<1)|1 = 0x1151`. The other entries use the identical
/// upper-five-bit packing; no renderer output participates in this table.
pub(crate) const RGBA32_EXPECTED: [u16; 2] = [0x1151, 0x5361];
/// With U=V=128, the public first-stage equations reduce to `R'=G'=B'=Y`.
/// The fixture's Texel0-pass combiner selects those values directly. For the
/// first Y byte, `y5=0x10>>3=2`, hence
/// `(2<<11)|(2<<6)|(2<<1)|1 = 0x1085`.
pub(crate) const YUV16_EXPECTED: [u16; 8] = [
    0x1085, 0x294b, 0x4211, 0x5ad7, 0x739d, 0x8c63, 0xa529, 0xbdef,
];

pub(crate) fn expected_direct_row(index: u32, texels: &[u16]) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if y == 0 && x < texels.len() as u32 {
        texels[x as usize]
    } else {
        STALE
    }
}

pub(crate) fn ia8_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA8_EXPECTED)
}
pub(crate) fn ia4_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA4_EXPECTED)
}
pub(crate) fn ia16_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA16_EXPECTED)
}
pub(crate) fn i4_expected(index: u32) -> u16 {
    expected_direct_row(index, &I4_EXPECTED)
}
pub(crate) fn i8_expected(index: u32) -> u16 {
    expected_direct_row(index, &I8_EXPECTED)
}
pub(crate) fn rgba32_expected(index: u32) -> u16 {
    expected_direct_row(index, &RGBA32_EXPECTED)
}
pub(crate) fn yuv16_expected(index: u32) -> u16 {
    expected_direct_row(index, &YUV16_EXPECTED)
}

/// Seed, load through the public 16-bit transfer form, redescribe the same
/// low-TMEM bytes with the direct format under test, then point-sample row 0.
///
/// `format` and `size` occupy SetTile bits 23:21 and 20:19. `line` is the
/// row stride in 64-bit TMEM words. All rows start at TMEM byte zero, so texel
/// x addresses byte `(x << size) >> 1`; 4-bit texels select the high nibble
/// for even x and low nibble for odd x. Loading via RGBA16 is a byte transfer
/// only and is required for 4-bit rows, whose direct load form is not public.
pub(crate) fn one_direct_texture_rect(
    source: u32,
    width: u32,
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
        set_tile_size(width, 1),
    ]);
    words.extend(texture_rectangle(0, 0, width, 1));
    words.push((0xe900_0000, 0));
    words
}

/// A true split-bank RGBA32 load and point-sampled draw. Unlike the smaller
/// direct formats, RGBA32 must be loaded with size 32 on both the image and
/// tile descriptors. The public `gDPLoadTextureTile` macro passes `siz`
/// unchanged to both SetTile commands, while `G_IM_SIZ_32b_TILE_BYTES` and
/// `G_IM_SIZ_32b_LINE_BYTES` are both 2: each texel advances two bytes in
/// each half-bank, and `line = 1` is one padded 64-bit row per bank here.
/// Programming Manual section 13.8.1 Figure 13-15 supplies the paired-bank
/// layout; `gbi.h` supplies the independently checkable command derivation.
pub(crate) fn one_rgba32_rect() -> Vec<(u32, u32)> {
    let width = RGBA32_EXPECTED.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
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
    words.extend(texture_rectangle(0, 0, width, 1));
    words.push((0xe900_0000, 0));
    words
}

/// A legal YUV16 load in the public even-S, paired-chroma form. Neutral
/// chroma removes every conversion-coefficient term before the Texel0-pass
/// combiner, so the hand-derived key needs no unstated matrix assumption.
pub(crate) fn one_yuv16_rect() -> Vec<(u32, u32)> {
    let width = YUV16_EXPECTED.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (
            0xfd00_0000 | (1 << 21) | (2 << 19) | (width - 1),
            YUV16_SOURCE,
        ),
        (0xf500_0000 | (1 << 21) | (2 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
        (0xe600_0000, 0),
        load_tile(width, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (1 << 21) | (2 << 19) | (1 << 9), 0),
        set_tile_size(width, 1),
    ]);
    words.extend(texture_rectangle(0, 0, width, 1));
    words.push((0xe900_0000, 0));
    words
}

// ---------------------------------------------------------------------------
// Colour-indexed (CI4) with a TLUT
// ---------------------------------------------------------------------------
//
// **Why this exists.** Every textured case above is direct-colour RGBA16: the
// texel bytes ARE the colour. A colour-indexed texture is a different path --
// the tile holds 4-bit INDICES, a palette is loaded separately into high TMEM
// by `LoadTlut`, and `en_tlut` in other-modes switches the sampler onto the
// lookup. None of that is reachable from an RGBA16 case, and
// `RT64-WM2000-TEXTURE-STATE.md` names the palette as one of the suspects it
// could not rule out for the blocky-glyph symptom.

/// Where the CI4 index image and its palette live in staged RDRAM.
pub(crate) const CI_SOURCE: u32 = 0x4000;
pub(crate) const PALETTE_SOURCE: u32 = 0x4100;

/// The palette's TMEM word address. `LoadTlut` refuses a destination tile
/// below word 256 by name ("LoadTLUT destination tile is outside high TMEM"),
/// which is the hardware split: indices live in low TMEM, palettes in high.
pub(crate) const PALETTE_TMEM_WORD: u32 = 256;

/// Eight CI4 indices, one per pixel of a 8x1 row -- deliberately NOT the
/// identity permutation, so a sampler that ignored the palette and returned
/// the index (or that returned palette entry `x` for pixel `x`) is visible.
pub(crate) const CI_INDICES: [u8; 8] = [3, 0, 5, 1, 7, 2, 6, 4];

/// The same bytes counted as 16-bit texels, which is how they are LOADED:
/// eight 4-bit indices are four bytes are two 16-bit texels.
pub(crate) const CI_LOAD_TEXELS: u32 = CI_INDICES.len() as u32 / 4;

/// The sixteen-entry RGBA16 palette. Only the eight entries the indices name
/// are distinguishable values; the rest are a marker that must never appear.
///
/// **The lookup is measured, not assumed.** Staging this palette 0x40 bytes
/// off leaves wgpu and RT64 still agreeing with each other but makes BOTH
/// stop matching the key -- both return `0x0001`, the decode of an unwritten
/// palette. So this case really does read the palette through `en_tlut`
/// rather than sampling the indices as colour.
pub(crate) const PALETTE: [u16; 16] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0x0843, 0x0843, 0x0843, 0x0843,
    0x0843, 0x0843, 0x0843, 0x0843,
];

pub(crate) const CI8_SOURCE: u32 = 0x4900;
pub(crate) const CI8_PALETTE_SOURCE: u32 = 0x4a00;
pub(crate) const CI8_INDICES: [u8; 8] = [0x03, 0x20, 0x55, 0x81, 0xa7, 0xc2, 0xe6, 0xf4];

/// The eight named CI8 palette entries are deliberately sparse across the
/// full 0..255 index domain. Every unnamed entry is a marker distinct from
/// both the key colours and `STALE`.
pub(crate) const fn ci8_palette_entry(index: u8) -> u16 {
    match index {
        0x03 => 0xf801,
        0x20 => 0x07c1,
        0x55 => 0x003f,
        0x81 => 0x7fff,
        0xa7 => 0x8421,
        0xc2 => 0xc631,
        0xe6 => 0x4211,
        0xf4 => 0xfc01,
        _ => 0x0843,
    }
}

/// The expected pixel for the CI4 case: pixel `x` reads index
/// `CI_INDICES[x]`, which selects `PALETTE[that]`.
pub(crate) fn ci_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < CI_INDICES.len() as u32 && y < 1 {
        PALETTE[CI_INDICES[x as usize] as usize]
    } else {
        STALE
    }
}

pub(crate) fn ci8_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < CI8_INDICES.len() as u32 && y == 0 {
        ci8_palette_entry(CI8_INDICES[x as usize])
    } else {
        STALE
    }
}

/// The CI4 command list: seed fill, state, palette load, index load, draw.
///
/// Wire notes, each from the same field positions the RGBA16 helpers above
/// cite: `SetTextureImage`/`SetTile` carry format CI (2) at bits 23:21 and
/// size 4-bit (0) at 20:19 for the index image, while the PALETTE is loaded
/// through a second tile that is RGBA16 -- libultra's own `gDPLoadTLUT`
/// macros emit `SetTextureImage(G_IM_FMT_RGBA, G_IM_SIZ_16b, ...)` for the
/// palette regardless of the indexed tile's format.
///
/// `en_tlut` is other-modes w0 bit 15, the one field that switches the
/// sampler from "the texel bytes are the colour" to "the texel bytes are an
/// index into high TMEM".
pub(crate) fn one_ci4_rect() -> Vec<(u32, u32)> {
    let entries = PALETTE.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        // One-cycle textured, TLUT ENABLED.
        (
            OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 15),
            OTHER_MODES_ONE_CYCLE_TEXTURED.1,
        ),
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        // -- the palette, into high TMEM through tile 1 (RGBA16 source).
        (0xfd00_0000 | (2 << 19) | (entries - 1), PALETTE_SOURCE),
        (0xf500_0000 | (2 << 19) | PALETTE_TMEM_WORD, 1 << 24),
        (0xe600_0000, 0),
        (0xf000_0000, (1 << 24) | ((entries - 1) << 14)),
        (0xe600_0000, 0),
        // -- the CI4 index image, into low TMEM through tile 0.
        //
        // **Loaded through a 16-bit image, described as CI4.** fn64 refuses a
        // direct four-bit load by name ("direct four-bit TMEM loads are
        // unsupported; load through a public 16-bit form", `tmem/wire.rs`),
        // and that is what real N64 code does anyway: the load moves bytes,
        // and only the TILE descriptor says how to read them. Eight 4-bit
        // indices are four bytes, so the loading tile is TWO 16-bit texels.
        (0xfd00_0000 | (2 << 19) | (CI_LOAD_TEXELS - 1), CI_SOURCE),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(CI_LOAD_TEXELS, 1),
        (0xe600_0000, 0),
        load_tile(CI_LOAD_TEXELS, 1),
        (0xe600_0000, 0),
        // Now redescribe the SAME TMEM words as a CI4 tile: format CI (2) at
        // bits 23:21, size 4-bit (0) at 20:19. Nothing is reloaded -- this is
        // a descriptor change over bytes already in TMEM.
        (0xf500_0000 | (2 << 21) | (0 << 19) | (1 << 9), 0),
        set_tile_size(CI_INDICES.len() as u32, 1),
    ]);
    words.extend(texture_rectangle(0, 0, CI_INDICES.len() as u32, 1));
    words.push((0xe900_0000, 0));
    words
}

/// CI8 uses all eight index bits and a 256-entry high-TMEM TLUT. The index
/// bytes are loaded through four public 16-bit transfer texels, then the same
/// low-TMEM bytes are redescribed as CI8 without reloading.
pub(crate) fn one_ci8_rect() -> Vec<(u32, u32)> {
    let entries = 256u32;
    let load_texels_16b = CI8_INDICES.len() as u32 / 2;
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
        (0xfd00_0000 | (2 << 19) | (load_texels_16b - 1), CI8_SOURCE),
        (0xf500_0000 | (2 << 19) | (1 << 9), 0),
        set_tile_size(load_texels_16b, 1),
        (0xe600_0000, 0),
        load_tile(load_texels_16b, 1),
        (0xe600_0000, 0),
        (0xf500_0000 | (2 << 21) | (1 << 19) | (1 << 9), 0),
        set_tile_size(CI8_INDICES.len() as u32, 1),
    ]);
    words.extend(texture_rectangle(0, 0, CI8_INDICES.len() as u32, 1));
    words.push((0xe900_0000, 0));
    words
}

// ---------------------------------------------------------------------------
// Textured raw triangle
// ---------------------------------------------------------------------------
//
// **Why this exists.** Every case above draws with `TextureRectangle`, and
// WM2000 does not: its packets carry raw TRIANGLES, nine TMEM loads each.
// `production.rs` dispatches triangles through their own arm with their own
// coefficient decode, plane evaluation and span walk -- none of which a
// texrect case can reach. A defect there is invisible to the whole corpus so
// far.

/// **One texel of S, in the non-perspective plane's own units.**
///
/// Derived from the cited scale, not read back from any implementation:
/// `G_TP_NONE` converts an s15.16 plane value to S10.5 by dividing by `2^21`,
/// and one whole texel is 32 in S10.5, so one texel is `32 * 2^21 = 2^26`.
///
/// **Non-perspective deliberately.** The perspective path's own scale carries
/// a documented history of having been fitted circularly against fn64's
/// constant before being re-derived; `2^21` is the independent one. A
/// perspective case is worth adding but must derive its expectation from
/// angrylion, not from either renderer.
pub(crate) const PLANE_PER_TEXEL: i32 = 1 << 21;

/// Half a texel, the anti-coincidence offset every plane base carries.
///
/// A sample landing exactly on a texel boundary needs a FULL texel of error
/// before the sampled texel changes, so a boundary fixture cannot see a
/// half-texel bug. Sampling at the midpoint makes an error of half a texel in
/// either direction visible.
pub(crate) const PLANE_HALF_TEXEL: i32 = 1 << 20;

/// The X distance in Q16.16 from the major edge to the first covered
/// subsample of the pixel that edge starts in: the sampler takes X column
/// 1/8, so a left edge on a whole pixel is one eighth short. The base cancels
/// it, so a column evaluates to exactly its intended plane value.
pub(crate) const FIRST_SUBSAMPLE_DELTA_X: i32 = 65536 / 8;

/// The triangle's covered box: columns [2, 6), rows [0, 3).
pub(crate) const TRI_LEFT: u32 = 2;
pub(crate) const TRI_RIGHT: u32 = 6;
pub(crate) const TRI_TOP: u32 = 0;
pub(crate) const TRI_BOTTOM: u32 = 3;

/// The expected pixel for the textured-triangle case.
///
/// Inside the box, column `x` reads texel `x - TRI_LEFT` of row 0 -- the S
/// plane advances exactly one texel per pixel of X and the T plane is
/// constant, so all three rows are three independent readings of the same
/// claim. Outside, the seeded `STALE` survives.
pub(crate) fn triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        TEXTURE_TEXELS[(x - TRI_LEFT) as usize]
    } else {
        STALE
    }
}

/// One 8-word coefficient block from four Q16.16 component groups.
///
/// The block is NOT sixteen consecutive Q16.16 values. As sixteen `u32`
/// halves (half `n` is byte `4n`), each component's HIGH 16 bits sit at its
/// integer offset and its LOW 16 bits sixteen bytes later; components 0 and 2
/// occupy their word's high half, 1 and 3 the low half. Byte offsets:
/// value (0, 16), d/dx (8, 24), d/de (32, 48), d/dy (40, 56).
pub(crate) fn coefficient_block(
    value: [i32; 4],
    dx: [i32; 4],
    de: [i32; 4],
    dy: [i32; 4],
) -> [u32; 16] {
    let mut halves = [0u32; 16];
    let mut put = |integer_byte: usize, fraction_byte: usize, components: [i32; 4]| {
        for (index, component) in components.iter().enumerate() {
            let high = integer_byte / 4 + index / 2;
            let low = fraction_byte / 4 + index / 2;
            let shift = if index % 2 == 0 { 16 } else { 0 };
            halves[high] |= ((((*component >> 16) as u32) & 0xffff) << shift) as u32;
            halves[low] |= (((*component as u32) & 0xffff) << shift) as u32;
        }
    };
    put(0, 16, value);
    put(8, 24, dx);
    put(32, 48, de);
    put(40, 56, dy);
    halves
}

/// A textured, unshaded, depthless raw triangle (opcode `0x0a`) covering
/// [`TRI_LEFT`, `TRI_RIGHT`) x [`TRI_TOP`, `TRI_BOTTOM`).
///
/// Wire, from the triangle decoder's own field reads: word 0 carries `lft` at
/// bit 23, `level` 21:19, `tile` 18:16 and YL in its low half; word 0's
/// second half is YM high / YH low, all three S11.2. Words 1..=3 are
/// XL/dXLdy, XH/dXHdy, XM/dXMdy as Q16.16 pairs. Then the eight-word texture
/// coefficient block.
///
/// A vertical-sided box rather than a sloped triangle: every dXdy is zero and
/// the left and right edges are constant, so the covered set is exactly the
/// rectangle above and the key is arithmetic rather than a rasterization
/// argument. The point of this case is the TEXTURE path, not edge walking.
/// One textured raw triangle (opcode `0x0a`) from an explicit H/L edge pair.
///
/// **RT64 emits exactly three vertices from these words**
/// (`rt64_gbi_rdp.cpp:352-406`), and reproducing its own arithmetic is what
/// makes this fixture predictable rather than guessed:
///
/// * `v1 = (XH evaluated at YH, YH)`
/// * `v2 = (XH evaluated at YL, YL)`
/// * `v3 = (XL, YM)`
///
/// So `v1` and `v2` always share the H edge's X. **A single triangle command
/// therefore cannot describe a rectangle** -- with every `dxdy` zero it
/// describes the right triangle between the H edge and the point `(XL, YM)`.
/// That is not a defect in either renderer; it is what the wire encoding
/// means, and it is why [`one_textured_triangle`] emits TWO of these.
///
/// `x_h`/`x_l` are whole pixels; every slope is zero, so both non-major
/// edges are vertical and the two triangles below tile exactly.
pub(crate) fn textured_triangle_words(
    x_h: u32,
    x_l: u32,
    y_h: u32,
    y_l: u32,
    y_m: u32,
    s_base: i32,
) -> Vec<(u32, u32)> {
    // All three Y bounds are S11.2. YL is the last covered scanline's LOWER
    // bound -- a triangle spanning rows 0..3 covers 0, 1 and 2, so YL is
    // line 3 and the raster's `y < yl` bound stops after row 2.
    let yl = ((y_l as i32) << 2) as u16 as u32;
    let ym = ((y_m as i32) << 2) as u16 as u32;
    let yh = ((y_h as i32) << 2) as u16 as u32;
    let word0 = 0x0a00_0000 | (1 << 23) | yl;
    let base = [
        (word0, (ym << 16) | yh),
        // Word order is XL/dXLdy, XH/dXHdy, XM/dXMdy -- the H edge is the
        // MAJOR one that `v1`/`v2` both sit on, and XM is unused here
        // because YM is pinned to an endpoint rather than a crossover row.
        ((x_l << 16), 0),
        ((x_h << 16), 0),
        ((x_l << 16), 0),
    ];
    // **Authored in RT64's VERTEX terms, which is what makes this fixture
    // readable by both lanes.**
    //
    // RT64 does not evaluate the plane per pixel; it evaluates S at three
    // vertices and lets the GPU interpolate (`decodeTriangles`):
    //
    // ```text
    // tc1 = base + De*dy_1                     dy_n = y_n - floor(yh)
    // tc2 = base + De*dy_2
    // tc3 = base + De*dy_3 + Dx*dx_3           dx_3 = x3 - (H edge at y3)
    // ```
    //
    // Only `tc3` carries the `Dx` term, so with `De = 0` -- which a texcoord
    // depending on X alone wants -- `tc1` and `tc2` BOTH take `base`.
    // Therefore **`base` must be the S of the H edge**, which is where those
    // two vertices sit, and `Dx` supplies the step out to `v3`.
    //
    // Both halves of the box want the SAME `Dx` of one texel per pixel of X:
    // the upper-right half's `dx_3` is NEGATIVE (its `v3` is to the left of
    // its H edge), so the sign cancels and no negative gradient is needed.
    // Only `base` differs between them. An earlier attempt that negated `Dx`
    // for that half double-counted the sign and read as texel 0 everywhere.
    //
    // T is constant, so every row reads TMEM row 0. W is 1 and unused:
    // `G_TP_NONE` never divides by it.
    let texture = coefficient_block(
        [s_base, PLANE_HALF_TEXEL, 1, 0],
        [PLANE_PER_TEXEL, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let mut words: Vec<(u32, u32)> = base.to_vec();
    for pair in texture.chunks_exact(2) {
        words.push((pair[0], pair[1]));
    }
    words
}

/// The two triangles that tile [`TRI_LEFT`, `TRI_RIGHT`) x [`TRI_TOP`,
/// `TRI_BOTTOM`) exactly, derived from RT64's own vertex rule above.
///
/// | | H edge | L edge | YM | vertices |
/// |---|---|---|---|---|
/// | lower-left | `TRI_LEFT` | `TRI_RIGHT` | `TRI_BOTTOM` | `(l,t) (l,b) (r,b)` |
/// | upper-right | `TRI_RIGHT` | `TRI_LEFT` | `TRI_TOP` | `(r,t) (r,b) (l,t)` |
///
/// Their union is the closed rectangle and their interiors are disjoint, so
/// the pair covers every pixel of the box exactly once.
pub(crate) fn textured_triangle_pair() -> Vec<(u32, u32)> {
    textured_triangle_pair_of_width(TRI_RIGHT - TRI_LEFT)
}

/// Generalization of [`textured_triangle_pair`] to an arbitrary texel
/// `width`, still anchored at `TRI_LEFT`/`TRI_TOP`/`TRI_BOTTOM` so every
/// existing case that calls the fixed-width wrapper is unchanged.
///
/// **Why this exists.** [`textured_triangle_pair`]'s box is hardcoded to
/// `[TRI_LEFT, TRI_RIGHT)`, four texels wide. A texture source staged with
/// FEWER texels than that (RGBA32's two, CI4/CI8's eight-INDEX-but-narrower-
/// after-redescribe strips) samples past its own staged data when drawn
/// through the fixed-width pair -- the S plane keeps advancing one texel per
/// pixel of X regardless of how many texels the tile actually holds, so
/// pixels beyond the real width silently wrap/clamp onto a neighboring texel
/// instead of failing loudly. Parameterizing the box width to the source's
/// own texel count is what closes that gap.
pub(crate) fn textured_triangle_pair_of_width(width: u32) -> Vec<(u32, u32)> {
    let right = TRI_LEFT + width;
    // Each half's `base` is the S at its OWN H edge, per the vertex rule in
    // `textured_triangle_words`: the lower-left half's H edge is the left
    // side, the upper-right half's is the right side.
    let left_s = PLANE_HALF_TEXEL - PLANE_PER_TEXEL / 8;
    let right_s = left_s + PLANE_PER_TEXEL * width as i32;
    let mut words =
        textured_triangle_words(TRI_LEFT, right, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM, left_s);
    words.extend(textured_triangle_words(
        right, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP, right_s,
    ));
    words
}

/// The command list for the textured-triangle case: seed fill, state, load,
/// triangle, sync. The texture staging is the 4x2 image the texrect cases
/// use, so a disagreement here against those is a triangle-path difference
/// and not a different texture.
/// Two constant-plane perspective triangles covering the standard triangle
/// box. The texture block is hand-authored as Q16.16 `[S,T,W] =
/// [65536,0,-262144]`, with all derivatives zero.
pub(crate) fn negative_w_textured_triangle_pair() -> Vec<(u32, u32)> {
    let triangle = |x_h: u32, x_l: u32, y_m: u32| {
        let yl = ((TRI_BOTTOM as i32) << 2) as u16 as u32;
        let ym = ((y_m as i32) << 2) as u16 as u32;
        let yh = ((TRI_TOP as i32) << 2) as u16 as u32;
        let base = [
            (0x0a00_0000 | (1 << 23) | yl, (ym << 16) | yh),
            (x_l << 16, 0),
            (x_h << 16, 0),
            (x_l << 16, 0),
        ];
        let texture = coefficient_block(
            [1 << 16, 0, -(4 << 16), 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        );
        let mut words = base.to_vec();
        for pair in texture.chunks_exact(2) {
            words.push((pair[0], pair[1]));
        }
        words
    };

    let mut words = triangle(TRI_LEFT, TRI_RIGHT, TRI_BOTTOM);
    words.extend(triangle(TRI_RIGHT, TRI_LEFT, TRI_TOP));
    words
}

/// Signed division gives `(1 / -4) * 1024 = -256` texels. Point sampling
/// floors that coordinate and the explicit four-texel S clamp selects column
/// zero, whose independently staged RGBA16 wire word is red (`0xf801`). The
/// full-target seed is `STALE` (`0xffff`), which this draw cannot produce.
pub(crate) fn negative_w_triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        0xf801
    } else {
        STALE
    }
}

pub(crate) fn one_negative_w_textured_triangle() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED_PERSPECTIVE,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile_clamped_four_texels(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, 1),
        (0xe600_0000, 0),
    ]);
    words.extend(negative_w_textured_triangle_pair());
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn one_textured_triangle() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
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

/// The command list for a textured case: seed fill, state, load, draw, sync.
///
/// **Why it opens with a full-target fill of `STALE`.** A texrect writes a
/// sub-region, so every pixel outside it must come from real prior content.
/// `execute_scheduled_texrect` takes that content from the packet's
/// accumulated buffer and refuses with `MissingResidentBytes` when there is
/// none -- a legitimate guard: treating a resident target as if it had no
/// prior content would silently discard everything outside the rectangle.
///
/// The fill lane has a second rung the texrect lane does not: a fill with no
/// accumulated buffer falls back to its declared colour-image seed read, the
/// guest's own framebuffer bytes. `seed_access_index` exists only on the fill
/// IR node, so a texrect that is the FIRST command against a resident target
/// has nothing to seed from and cannot complete.
///
/// Opening the list with a full-extent fill answers that from inside the
/// packet: the fill needs no seed itself (it covers the whole target), and it
/// leaves an accumulated buffer the texrect then composes into. This is the
/// same in-packet composition `nested-second-fill` already exercises.
///
/// The fill paints `STALE`, which is exactly what `seeded` already writes
/// across the framebuffer and exactly what [`textured_expected`] already
/// requires outside the rectangle -- so the hand-derived key is unchanged by
/// this fill, and the pixels outside the rectangle still assert that the
/// texrect wrote nothing it should not have.
pub(crate) fn one_textured_rect() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle(
        TEXRECT_ULX,
        TEXRECT_ULY,
        TEXRECT_LRX,
        TEXRECT_LRY,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// `SetTile` for tile 0, RGBA16, with an explicit **S mask** and the WRAP
/// (non-clamp, non-mirror) S mode, for the right-edge over-read reproduction.
///
/// The `set_tile` above leaves `mask_s == 0`, which `tmem/sample.rs` treats
/// as a forced clamp regardless of mode. To exercise the WRAP arm this case
/// must carry a nonzero mask. Public `gDPSetTile` (`ultra64/gbi.h`) places
/// S mode at bits 9:8 (0 = WRAP) and S mask at 7:4; `mask_s = 2` addresses a
/// four-texel row (`0..3`), so a coordinate at texel 4 wraps to texel 0.
pub(crate) const fn set_tile_wrap_s(line_words: u32, tmem_word: u32, mask_s: u32) -> (u32, u32) {
    (
        0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word,
        mask_s << 4,
    )
}

/// A one-cycle texrect whose right edge lands ONE pixel past the tile's
/// loaded S extent, so the rightmost destination column samples texel index
/// `TEXTURE_WIDTH` -- one texel beyond the `[0, TEXTURE_WIDTH-1]` the
/// `LoadTile` actually wrote. This is the exact shape #35 inferred but the
/// then-existing corpus never constructed: the intersection of a textured
/// rectangle, its right screen column, and a tile whose valid texels end at
/// that column.
///
/// The rectangle is `TEXTURE_WIDTH + 1` pixels wide over a `TEXTURE_WIDTH`-
/// texel tile at one texel per pixel (`dsdx = 1<<10`), so `s_at(column)`
/// equals `column` texels and the last column (index `TEXTURE_WIDTH`) lands
/// on the first out-of-extent texel. `addressing` (`wrap` vs `clamp`) is the
/// only thing that varies between the two variants, and the two produce a
/// DIFFERENT rightmost-column texel -- so a right-edge over-read is visible
/// no matter which addressing the sampler is asked for.
pub(crate) fn right_edge_overread_rect(addressing: RightEdgeAddressing) -> Vec<(u32, u32)> {
    // Draw one pixel wider than the loaded tile so the last column samples
    // texel TEXTURE_WIDTH.
    let draw_width = TEXTURE_WIDTH + 1;
    let mut words = set_bilerp0(one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1));
    words.pop();
    let tile_word = match addressing {
        // mask_s == 0 forces clamp: texel TEXTURE_WIDTH clamps to
        // TEXTURE_WIDTH-1 (the last loaded texel).
        RightEdgeAddressing::Clamp => set_tile(TEXTURE_LINE_WORDS, 0),
        // mask_s == 2 + WRAP mode: texel TEXTURE_WIDTH wraps to texel 0.
        RightEdgeAddressing::Wrap => set_tile_wrap_s(TEXTURE_LINE_WORDS, 0, 2),
    };
    words.extend([
        set_bilerp0(vec![OTHER_MODES_ONE_CYCLE_TEXTURED])[0],
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        tile_word,
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle(0, 0, draw_width, TEXTURE_HEIGHT));
    words.push((0xe900_0000, 0));
    words
}

#[derive(Clone, Copy)]
pub(crate) enum RightEdgeAddressing {
    Clamp,
    Wrap,
}

/// The one-cycle point-sampled fixture with only its draw cycle changed to
/// two-cycle mode. [`SET_COMBINE_TEXEL0`] programs Texel0 passthrough in both
/// cycles, so its hand-derived key remains [`textured_expected`].
pub(crate) fn two_cycle_textured_rect() -> Vec<(u32, u32)> {
    let mut words = one_textured_rect();
    let draw_modes = words
        .iter_mut()
        .find(|word| **word == OTHER_MODES_ONE_CYCLE_TEXTURED)
        .expect("one_textured_rect must set one-cycle textured draw modes");
    *draw_modes = OTHER_MODES_TWO_CYCLE_TEXTURED;
    words
}

/// A texrect whose primitive combiner supplies opaque alpha while the
/// blender's P selector reads either BlendColor or FogColor. The seeded blue
/// target and red primitive colour are both distinct from the state colour,
/// so a dropped state command or combiner passthrough cannot satisfy the key.
pub(crate) fn state_color_blender_rect(
    set_state_color: (u32, u32),
    other_modes: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut words = one_fill(BLUE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        set_state_color,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, 0xff00_00ff),
        other_modes,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle(
        TEXRECT_ULX,
        TEXRECT_ULY,
        TEXRECT_LRX,
        TEXRECT_LRY,
    ));
    words.push((0xe900_0000, 0));
    words
}

pub(crate) fn blend_color_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < TEXRECT_LRX && y < TEXRECT_LRY {
        // RGBA8888 (0x40, 0x80, 0xc0, 0xff) -> RGBA5551 (8, 16, 24, 1).
        0x4431
    } else {
        BLUE
    }
}

pub(crate) fn fog_color_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < TEXRECT_LRX && y < TEXRECT_LRY {
        // RGBA8888 (0x20, 0x60, 0xa0, 0xff) -> RGBA5551 (4, 12, 20, 1).
        0x2329
    } else {
        BLUE
    }
}

/// A texrect whose combiner emits opaque white and whose blender evaluates
/// `(white * 1 + white * 1) / (1 + 1)` through RT64's overflow path.
///
/// The target is seeded blue, which this all-white blend program cannot
/// produce, so a dropped draw cannot accidentally satisfy the key. The
/// texture setup is retained from [`one_textured_rect`] even though the
/// Primitive combiner does not sample it, keeping the draw on the corpus's
/// already-proven texrect command shape.
pub(crate) fn blend_numerator_overflow_rect() -> Vec<(u32, u32)> {
    let mut words = one_fill(BLUE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_BLEND_OVERFLOW,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, 0xffff_ffff),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle(
        TEXRECT_ULX,
        TEXRECT_ULY,
        TEXRECT_LRX,
        TEXRECT_LRY,
    ));
    words.push((0xe900_0000, 0));
    words
}

/// Hand-derived key for [`blend_numerator_overflow_rect`]. In normalized
/// units the wrapped channel is `(2 mod (1 + 8/255)) / 2 = 247/510`, which
/// quantizes to RGB5 value 15 in every channel; opaque RGBA16 is therefore
/// `(15 << 11) | (15 << 6) | (15 << 1) | 1 = 0x7bdf`.
pub(crate) fn blend_numerator_overflow_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < TEXRECT_LRX && y < TEXRECT_LRY {
        0x7bdf
    } else {
        BLUE
    }
}

// ---------------------------------------------------------------------------
// Measured-opcode gap cases
// ---------------------------------------------------------------------------

/// `LoadBlock` transfers `texel_count` consecutive RGBA16 texels beginning at
/// `(uls, ult)`. Unlike [`load_tile`], its last twelve bits are DXT rather
/// than a lower-right T coordinate.
///
/// Wire, from public libultra `gDPLoadBlock`: ULS/ULT occupy word 0's two
/// twelve-bit coordinate fields, while word 1 holds tile, inclusive LRS and
/// DXT. These cases start at `(0, 0)`, so only LRS and DXT are nonzero.
pub(crate) const fn load_block(texel_count: u32, dxt: u32) -> (u32, u32) {
    (0xf300_0000, ((texel_count - 1) << 12) | dxt)
}

/// A LoadBlock case over [`WIDE_TEXELS`]. `load_line_words` is the stride
/// applied when DXT crosses 0x800; `render_line_words` redescribes the loaded
/// bytes for sampling, because LoadBlock may leave holes between its logical
/// rows.
pub(crate) fn load_block_textured_rect(
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
        set_texture_image(WIDE_WIDTH, WIDE_SOURCE),
        // Public gDPLoadTextureBlock orders TileSync before the loading tile.
        (0xe800_0000, 0),
        set_tile(load_line_words, 0),
        (0xe600_0000, 0),
        load_block(texel_count, dxt),
        // The render descriptor is not installed until the load is complete.
        (0xe700_0000, 0),
        set_tile(render_line_words, 0),
        set_tile_size(render_width, render_height),
    ]);
    words.extend(texture_rectangle(0, 0, render_width, render_height));
    words.push((0xe900_0000, 0));
    words
}

/// DXT zero never crosses the 0x800 row threshold. The first two 64-bit
/// words therefore land at TMEM words 0 and 1, so the one-row render reads
/// source texels 0 through 7 in order.
pub(crate) fn load_block_linear_expected(index: u32) -> u16 {
    expected_direct_row(index, &LOAD_BLOCK_LINEAR_EXPECTED)
}

/// DXT 0x400 advances after the second word. With a loading `line = 2`, the
/// four source words land at TMEM words 0, 1, 4 and 5; redescribing the tile
/// with render `line = 4` makes rows 0 and 1 read those exact pairs. The odd
/// row's four-byte exchange is applied by both load and sample, so the visible
/// texels remain [`WIDE_TEXELS`] in row-major order.
pub(crate) fn load_block_dxt_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < WIDE_WIDTH && y < WIDE_HEIGHT {
        LOAD_BLOCK_DXT_EXPECTED[(y * WIDE_WIDTH + x) as usize]
    } else {
        STALE
    }
}

/// Opcode 0x25 uses the same destination rectangle as opcode 0x24 and swaps
/// the coordinate axes: pixel `(x, y)` reads source `(s, t) = (y, x)`.
/// [`WIDE_TEXELS`] is redescribed as a 4x4 image so the transpose is square,
/// in-bounds, and every transposed position has a distinct value.
pub(crate) fn texrect_flip_expected(index: u32) -> u16 {
    const SIDE: u32 = 4;
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < SIDE && y < SIDE {
        TEXRECT_FLIP_EXPECTED[(y * SIDE + x) as usize]
    } else {
        STALE
    }
}

pub(crate) fn one_textured_rect_flip() -> Vec<(u32, u32)> {
    const SIDE: u32 = 4;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(SIDE, WIDE_SOURCE),
        set_tile(1, 0),
        set_tile_size(SIDE, SIDE),
        (0xe600_0000, 0),
        load_tile(SIDE, SIDE),
        (0xe700_0000, 0),
    ]);
    let mut rectangle = texture_rectangle(0, 0, SIDE, SIDE);
    rectangle[0].0 = (rectangle[0].0 & 0x00ff_ffff) | 0xe500_0000;
    words.extend(rectangle);
    words.push((0xe900_0000, 0));
    words
}

/// Public libultra `G_CC_PRIMITIVE` in both cycles. Its token `0` maps to the
/// dedicated zero mux encodings (RGB 31, narrowed to 15 in A/B; alpha 7),
/// while primitive is D=3. Applying `GCCc0w0`/`GCCc1w0` gives `0x00ff_ffff`;
/// applying `GCCc0w1`/`GCCc1w1` gives `0xfffd_f6fb`.
pub(crate) const SET_COMBINE_PRIMITIVE: (u32, u32) = (0xfcff_ffff, 0xfffd_f6fb);
// RGBA16 bit 0 stores coverage[2], not primitive alpha. Full coverage stores
// 8 - 1 = 7 under CVG_DST_CLAMP, whose visible MSB is one; RGB5=(4,24,28)
// therefore packs as 0x2639 (Programming Manual §§15.5.3, 15.5.6, 15.7).
pub(crate) const FLAT_TRIANGLE_COLOR: u16 = 0x2639;

pub(crate) fn flat_triangle_words(
    x_h: u32,
    x_l: u32,
    y_h: u32,
    y_l: u32,
    y_m: u32,
) -> Vec<(u32, u32)> {
    let yl = ((y_l as i32) << 2) as u16 as u32;
    let ym = ((y_m as i32) << 2) as u16 as u32;
    let yh = ((y_h as i32) << 2) as u16 as u32;
    vec![
        (0x0800_0000 | (1 << 23) | yl, (ym << 16) | yh),
        (x_l << 16, 0),
        (x_h << 16, 0),
        (x_l << 16, 0),
    ]
}

pub(crate) fn one_flat_triangle_pair() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_NO_AA,
        SET_COMBINE_PRIMITIVE,
        // Primitive RGBA8888 = (0x20, 0xc0, 0xe0, 0xff). With dither off,
        // the target keeps RGB5=(4,24,28), A1=1: 0x2000+0x0600+0x0038+1.
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

pub(crate) fn flat_triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        FLAT_TRIANGLE_COLOR
    } else {
        STALE
    }
}

/// A shade-only raw triangle (opcode `0x0c` = `G_RDPTRI_BASE | Shaded`)
/// covering [`TRI_LEFT`, `TRI_RIGHT`) x [`TRI_TOP`, `TRI_BOTTOM`), built from
/// the same edge words [`flat_triangle_words`] uses plus one 8-word shade
/// coefficient block.
///
/// **RT64's own field layout is the authority for the shade block**
/// (`rt64_gbi_rdp.cpp` `decodeTriangles`, shaded branch): `curData[0]`/
/// `curData[2]` supply the base RGBA (word 0 = R:G, word 1 = B:A, each split
/// integer-high/fraction-low across the pair), `curData[1]`/`curData[3]`
/// supply d/dx, and `curData[4]`/`curData[6]` supply d/de -- exactly
/// [`coefficient_block`]'s `(value, dx, de, dy)` grouping, so that helper
/// (already proven correct for the S/T/W block) packs the shade block too.
///
/// Every derivative is zero: FLAT shade, so every covered pixel reads the
/// same RGBA color regardless of where it falls in the triangle, and the
/// key is a single named value rather than a per-pixel interpolation.
pub(crate) fn shade_triangle_words(
    x_h: u32,
    x_l: u32,
    y_h: u32,
    y_l: u32,
    y_m: u32,
    rgba: [i32; 4],
) -> Vec<(u32, u32)> {
    let yl = ((y_l as i32) << 2) as u16 as u32;
    let ym = ((y_m as i32) << 2) as u16 as u32;
    let yh = ((y_h as i32) << 2) as u16 as u32;
    let base = [
        (0x0c00_0000 | (1 << 23) | yl, (ym << 16) | yh),
        (x_l << 16, 0),
        (x_h << 16, 0),
        (x_l << 16, 0),
    ];
    let shade = coefficient_block(rgba, [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]);
    let mut words: Vec<(u32, u32)> = base.to_vec();
    for pair in shade.chunks_exact(2) {
        words.push((pair[0], pair[1]));
    }
    words
}

/// Shade RGBA8888 = (0x20, 0xc0, 0xe0, 0xff), the same base color
/// [`one_flat_triangle_pair`] uses via `SET_COMBINE_PRIMITIVE`, packed here
/// as Q16.16 integers for the shade coefficient block. With every
/// derivative zero the interpolated color is this constant everywhere, and
/// with dither off the quantization arithmetic is identical to the
/// primitive case: RGB5=(4,24,28) with the coverage bit set gives the same
/// [`FLAT_TRIANGLE_COLOR`] (`0x2639`).
pub(crate) const SHADE_TRIANGLE_RGBA: [i32; 4] = [0x20 << 16, 0xc0 << 16, 0xe0 << 16, 0xff << 16];

pub(crate) fn one_shade_triangle_pair() -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_NO_AA,
        SET_COMBINE_SHADE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
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

pub(crate) fn shade_triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        FLAT_TRIANGLE_COLOR
    } else {
        STALE
    }
}

pub(crate) fn skew_textured_rect(line_words: u32, low_t: u32) -> Vec<(u32, u32)> {
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(SKEW_WIDTH, SKEW_SOURCE),
        set_tile(line_words, 0),
        set_tile_size_at(SKEW_WIDTH, SKEW_HEIGHT, low_t),
        (0xe600_0000, 0),
        load_tile_at(SKEW_WIDTH, SKEW_HEIGHT, low_t),
        (0xe600_0000, 0),
    ]);
    words.extend(texture_rectangle_at_t(0, 0, SKEW_WIDTH, SKEW_HEIGHT, low_t));
    words.push((0xe900_0000, 0));
    words
}
