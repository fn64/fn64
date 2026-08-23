//! The parity differential: fn64's shipping wgpu backend against RT64, the
//! oracle. The reference backend is run and reported alongside, but it is
//! NOT an authority and never enters a verdict.
//!
//! **`fn64-render-reference` is a second fn64 implementation, not a third
//! opinion.** It is unproven against hardware, and it has been WRONG on this
//! corpus's own cases: on every textured case here it returns a third set of
//! values that matches neither the hand-derived key nor RT64
//! (`RT64-HANDOFF.md` §3f records the one-cycle case where wgpu was right and
//! the reference was wrong). Read its column as a hint about where to look,
//! never as evidence for or against wgpu. Every verdict below is computed
//! from the wgpu/RT64 pair and the key alone.
//!
//! # Why this binary exists
//!
//! `fn64-render-conformance-wgpu-runner sweep` already compares wgpu against
//! the reference backend. That answers "do fn64's two Rust backends agree",
//! which is a useful consistency check and is NOT a parity measurement. The
//! port's stated purpose is matching RT64. So the number worth reporting is
//! wgpu-vs-RT64 over one replay input, and that is what this binary computes.
//!
//! It is a separate binary rather than a subcommand of the wgpu runner
//! because it needs the `rt64` feature, which drags in a C++ build and is
//! macOS-only. Keeping it separate leaves the wgpu runner buildable
//! everywhere.
//!
//! # RT64 is NOT authoritative everywhere, and the metric must say so
//!
//! `docs/RT64-GUARD-AUDIT.md` established that RT64 stops modelling the
//! hardware downstream of coverage: memory alpha is hardcoded to `1.0f` under
//! the comment "Coverage is not emulated" (`hle/rt64_blender.h:355-357`),
//! there is no hidden-bits sidecar, and `AA_EN` / `ALPHA_CVG_SEL` reach only a
//! debugger text line. angrylion is the sole authority there.
//!
//! A parity percentage that silently included coverage-dependent cases would
//! be measuring the wrong thing: a wgpu-vs-RT64 difference in such a case is
//! evidence about RT64's modelling gap, not about wgpu. So every case
//! declares an [`Authority`], the two partitions are counted separately, and
//! they are never added together into one percentage.
//!
//! # The answer key is a third authority, not either backend
//!
//! Every case carries a hand-derived `expected` function computed as
//! arithmetic over its own display list from public RDP semantics. It exists
//! so that when two backends disagree, the row can say which one the key
//! blesses -- attribution, not merely a count. No key is ever captured from a
//! backend's output.

#![cfg(target_os = "macos")]
#![allow(unsafe_code)]

use std::{
    fs::File,
    io::{self, Write},
    os::fd::FromRawFd,
};

use fn64_render::{
    AspectTarget, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRuntimeSettings,
};
use fn64_render_reference::ReferenceBackend;
use fn64_render_rt64::Rt64Backend;
use fn64_render_wgpu::conformance::{ConformanceReplay, ConformanceSession};
use fn64_runtime::{RdramAddr, RdramViewMut};
use serde_json::{json, Value};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMAND_START: u32 = 0x100;
const FRAMEBUFFER: u32 = 0x10_0000;

/// RT64 configures a real swap chain and a real Metal device. The 8x4 target
/// the wgpu runner's sweep uses is below anything RT64 will render into, so
/// the parity corpus uses a full 320x240 NTSC target -- the same extent the
/// RT64 deferred-history runner already proves RT64 renders at.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const PIXEL_COUNT: u32 = WIDTH * HEIGHT;
const FRAMEBUFFER_BYTES: u32 = PIXEL_COUNT * 2;

const RED: u16 = 0xf801;
const GREEN: u16 = 0x07c1;
const BLUE: u16 = 0x003f;
const STALE: u16 = 0xffff;
const GUARD: u16 = 0x4211;

/// Whether RT64's answer for a case is evidence about the hardware.
///
/// This is the partition the whole metric turns on. It is declared per case
/// from what the case's commands actually exercise, and it is checked against
/// the command words by `authority_matches_the_commands` so a case cannot
/// claim authority it does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Authority {
    /// RT64 models this faithfully: command semantics, geometry, combiner and
    /// texture behaviour. A wgpu-vs-RT64 difference here is a wgpu finding.
    Rt64Authoritative,
    /// RT64 does not model this. Anti-aliasing, coverage-dependent blending,
    /// and dither. A difference here is NOT evidence against wgpu, because
    /// the oracle is the one not modelling the hardware.
    /// `docs/RT64-GUARD-AUDIT.md` C4-C6, U1-U3.
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
    const fn wire(self) -> &'static str {
        match self {
            Self::Rt64Authoritative => "rt64-authoritative",
            Self::CoverageDependentRt64NotAuthoritative => "rt64-not-authoritative-coverage",
            Self::RawTrianglePlaneScaleDisagreement => "raw-triangle-plane-scale-disagreement",
        }
    }
}

struct Case {
    name: &'static str,
    /// Why this case is in the corpus at all -- what a disagreement here
    /// would mean.
    intent: &'static str,
    authority: Authority,
    commands: Vec<(u32, u32)>,
    /// Hand-derived expected pixel by linear index. Never captured from a
    /// backend.
    expected: fn(u32) -> u16,
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
const fn set_scissor(ulx: u32, uly: u32, lrx: u32, lry: u32) -> (u32, u32) {
    (
        0xed00_0000 | ((ulx * 4) << 12) | (uly * 4),
        ((lrx * 4) << 12) | (lry * 4),
    )
}

const fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

/// `SetOtherModes` for fill cycle with no AA, no coverage read, no dither.
/// This is the word every RT64-authoritative FILL case uses, and the thing
/// the coverage-dependent cases deliberately change.
const OTHER_MODES_FILL_NO_AA: (u32, u32) = (0xef30_00f0, 0);

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
const OTHER_MODES_ONE_CYCLE_TEXTURED: (u32, u32) = (0xef00_00f0, 0);

/// The two-cycle textured twin: only public `G_CYC_2CYCLE` bit 20 differs.
/// Point sampling, disabled dithering, and disabled coverage modes remain
/// identical to [`OTHER_MODES_ONE_CYCLE_TEXTURED`].
const OTHER_MODES_TWO_CYCLE_TEXTURED: (u32, u32) = (0xef10_00f0, 0);

/// The perspective-textured twin: only public `G_TP_PERSP` bit 19 differs.
/// Point sampling, disabled dithering, and disabled coverage modes remain
/// identical to [`OTHER_MODES_ONE_CYCLE_TEXTURED`].
const OTHER_MODES_ONE_CYCLE_TEXTURED_PERSPECTIVE: (u32, u32) =
    (OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 19), 0);

/// `SetOtherModes` for a ONE-CYCLE fill: byte-identical to
/// [`OTHER_MODES_FILL_NO_AA`] except the cycle-type field.
///
/// The two words differ only in w0 bits 21:20 (`G_MDSFT_CYCLETYPE`), which
/// carry `G_CYC_FILL = 3` in the fill constant and `G_CYC_1CYCLE = 0` here.
/// Every other property -- no AA, no dither, no coverage read -- is
/// unchanged, so this case stays in the RT64-authoritative partition for
/// exactly the reasons [`OTHER_MODES_FILL_NO_AA`] documents.
const OTHER_MODES_ONE_CYCLE_NO_AA: (u32, u32) = (0xef00_00f0, 0);

/// One-cycle, forced general blending with `P = M = Combined`,
/// `A = CombinedAlpha`, and `B = One`.
///
/// The low word is derived from the public `G_BL_*` selector packing:
/// `B = One` is selector 2 in cycle 1's bits 18:19 and `FORCE_BL` is bit 14;
/// every other selector is zero. This is deliberately not RT64's duplicate
/// `P == M && B == OneMinusA` passthrough: `B == One` makes the numerator
/// overflow classification true.
const OTHER_MODES_ONE_CYCLE_BLEND_OVERFLOW: (u32, u32) =
    (0xef00_00f0, (2 << 18) | (1 << 14));

/// One-cycle forced blending that selects BlendColor for P, input alpha for
/// A, combiner output for M, and zero for B. With opaque combiner alpha and
/// B=zero the M term vanishes, so the blender outputs BlendColor directly.
/// M=clr_in (0) avoids clr_mem (1) which would require IM_RD.
const OTHER_MODES_ONE_CYCLE_BLEND_COLOR: (u32, u32) = (0xef00_00f0, 0x800c_4000);

/// The FogColor twin of [`OTHER_MODES_ONE_CYCLE_BLEND_COLOR`], selecting
/// FogColor for P while retaining the same opaque-alpha and zero-B terms.
const OTHER_MODES_ONE_CYCLE_FOG_COLOR: (u32, u32) = (0xef00_00f0, 0xc00c_4000);

/// The colour the one-cycle band is asked to paint, and the seed it must
/// replace. Deliberately not `STALE`, so "the band did nothing" and "the
/// band worked" are different pictures.
const BAND_FILL_COLOR: u16 = 0xf801;

/// The seed the band must overwrite. Deliberately NOT white: the combiner
/// WM2000 stages resolves to white, so a white seed would make this case
/// pass even if the command were dropped entirely.
const BAND_SEED: u16 = 0x0843;

/// What the measured combiner produces with `shade = texel0 = texel1 = 0`.
/// Both RT64 and the reference backend were observed to write this.
const BAND_COMBINED_OUTPUT: u16 = 0xffff;

/// The band's own rows, chosen inside the target and away from every edge so
/// a clipping defect cannot be mistaken for a dropped command.
const BAND_TOP: u32 = 64;
const BAND_BOTTOM: u32 = 127;

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
fn one_cycle_fill_band() -> Vec<(u32, u32)> {
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
/// fixture-cannot-detect-the-bug trap `docs/RT64-WM2000-HARNESS-TRAPS.md`
/// records; seeding a distinct colour is what gives this case teeth.
fn one_cycle_fill_band_expected(index: u32) -> u16 {
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

fn one_fill(color: u16, ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
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
// `docs/RT64-WM2000-TEXTURE-STATE.md` bounded WM2000's remaining defect to,
// and localising it took a 20-minute ROM run plus a human reading a PNG.
//
// Every expected texel below is derived BY HAND from the RGBA16 wire layout
// and the TMEM addressing rule, never from any fn64 implementation. See
// `TEXTURE_TEXELS` for the derivation.

/// Where the texture's source pixels live in the staged RDRAM image. Clear of
/// the command stream (`0x100`) and the colour target (`0x10_0000`).
const TEXTURE_SOURCE: u32 = 0x2000;

/// The texture is 4x2 RGBA16 texels, so one row is 8 bytes = one 64-bit TMEM
/// word, and `SetTile`'s `line` field is 1.
const TEXTURE_WIDTH: u32 = 4;
const TEXTURE_HEIGHT: u32 = 2;
const TEXTURE_LINE_WORDS: u32 = 1;

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
const TEXTURE_TEXELS: [u16; 8] = [
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
const WIDE_SOURCE: u32 = 0x3000;
const WIDE_WIDTH: u32 = 8;
const WIDE_HEIGHT: u32 = 2;
const WIDE_LINE_WORDS: u32 = 2;

/// Sixteen distinct RGBA16 texels, row-major.
///
/// **Row 1 is what this case is for.** Every row-1 texel has bit 0x0040 set
/// (green's low bit) and no row-0 texel does, so reading row 0 where row 1
/// was meant is visible in one bit even if the columns happen to line up.
/// Within a row every texel differs from every other, so a wrong column is
/// visible too, and no texel is a byte-swap of another.
const WIDE_TEXELS: [u16; 16] = [
    // row 0: bit 0x0040 clear in every entry. 0x7fff rather than 0xffff for
    // the same anti-alias reason `TEXTURE_TEXELS` states.
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01,
    // row 1: bit 0x0040 set in every entry.
    0xf841, 0x0641, 0x0079, 0xffbf, 0x8461, 0xc671, 0x4251, 0xfc41,
];

// Independent, hand-derived keys. Keep these separate from `WIDE_TEXELS` so
// mutating an expected entry cannot also mutate the staged texture source.
const LOAD_BLOCK_LINEAR_EXPECTED: [u16; 8] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01,
];
const LOAD_BLOCK_DXT_EXPECTED: [u16; 16] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0xf841, 0x0641, 0x0079, 0xffbf,
    0x8461, 0xc671, 0x4251, 0xfc41,
];
const TEXRECT_FLIP_EXPECTED: [u16; 16] = [
    0xf801, 0x8421, 0xf841, 0x8461, 0x07c1, 0xc631, 0x0641, 0xc671, 0x003f, 0x4211, 0x0079, 0x4251,
    0x7fff, 0xfc01, 0xffbf, 0xfc41,
];

/// A tall RGBA16 strip reproducing WM2000's measured texrect state without
/// carrying any game content. The 64-texel source row occupies 16 TMEM
/// words, while the base tile deliberately declares the measured
/// `line = 17`, leaving one word of padding between rows. Fourteen rows make
/// a two-pixel-per-row displacement impossible to mistake for a boundary
/// tie-break.
const SKEW_SOURCE: u32 = 0x5000;
const SKEW_WIDTH: u32 = 64;
const SKEW_HEIGHT: u32 = 14;
const SKEW_LOW_T_ODD: u32 = 95;
const SKEW_LINE_WORDS: u32 = 17;
const SKEW_BAR_LEFT: u32 = 8;
const SKEW_BAR_RIGHT: u32 = 56;

const fn skew_texel(x: u32) -> u16 {
    if x >= SKEW_BAR_LEFT && x < SKEW_BAR_RIGHT {
        RED
    } else {
        BLUE
    }
}

fn skew_expected(index: u32) -> u16 {
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
fn wide_expected(index: u32) -> u16 {
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
fn wide_textured_rect() -> Vec<(u32, u32)> {
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
const TEXRECT_ULX: u32 = 0;
const TEXRECT_ULY: u32 = 0;
const TEXRECT_LRX: u32 = TEXTURE_WIDTH;
const TEXRECT_LRY: u32 = TEXTURE_HEIGHT;

/// The expected pixel for a textured case, by linear target index.
///
/// Inside the rectangle a pixel reads its own texel; outside it the seeded
/// `STALE` survives. This is the whole key, and it is arithmetic over
/// [`TEXTURE_TEXELS`] -- no backend is consulted.
fn textured_expected(index: u32) -> u16 {
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
const fn set_texture_image(width: u32, address: u32) -> (u32, u32) {
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
const fn set_tile(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word, 0)
}

/// Tile 0 with explicit S/T clamp and a two-bit S mask for a four-texel row.
/// Public `gDPSetTile` places T mode at 19:18, S mode at 9:8 and S mask at
/// 7:4. `G_TX_CLAMP = 2`; mask 2 preserves columns 0..3 after clamping.
const fn set_tile_clamped_four_texels(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (
        0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word,
        (2 << 18) | (2 << 8) | (2 << 4),
    )
}

/// `SetTileSize` for tile 0 covering the whole texture.
///
/// All four coordinates are S10.2 and both high edges are INCLUSIVE, so a
/// `w`-texel wide tile has `high_s = (w - 1) << 2`.
const fn set_tile_size(width: u32, height: u32) -> (u32, u32) {
    (0xf200_0000, (((width - 1) * 4) << 12) | ((height - 1) * 4))
}

/// `LoadTile` for tile 0 covering the whole texture, in the same S10.2
/// inclusive form as `SetTileSize`.
///
/// LoadTile rather than LoadBlock deliberately: LoadBlock's row advance is
/// driven by DXT and its `line` interacts with it, so a multi-row LoadBlock
/// whose rows stay contiguous is not expressible at `line = 1`. LoadTile
/// states its rows directly, which is what a fixture wants.
const fn load_tile(width: u32, height: u32) -> (u32, u32) {
    (0xf400_0000, (((width - 1) * 4) << 12) | ((height - 1) * 4))
}

/// `TextureRectangle` sampling tile 0, one texel per pixel on both axes.
///
/// Wire: word 0 carries `lrx` at 23:12 and `lry` at 11:0 in S10.2; word 1
/// carries `tile` at 26:24, `ulx` at 23:12 and `uly` at 11:0. Words 2 and 3
/// carry the S/T origin in S10.5 and the per-pixel DsDx/DtDy in S5.10.
/// `1 << 10` is exactly one texel per pixel.
fn texture_rectangle(ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
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
const fn tile_bounds_at(opcode: u32, width: u32, height: u32, low_t: u32) -> (u32, u32) {
    (
        opcode | (low_t * 4),
        (((width - 1) * 4) << 12) | ((low_t + height - 1) * 4),
    )
}

const fn set_tile_size_at(width: u32, height: u32, low_t: u32) -> (u32, u32) {
    tile_bounds_at(0xf200_0000, width, height, low_t)
}

const fn load_tile_at(width: u32, height: u32, low_t: u32) -> (u32, u32) {
    tile_bounds_at(0xf400_0000, width, height, low_t)
}

/// The same one-texel-per-pixel texrect with its S10.5 T origin aligned to a
/// nonzero tile origin. Subtracting the tile's S10.2 `low_t` therefore starts
/// the draw on tile-relative row zero.
fn texture_rectangle_at_t(ulx: u32, uly: u32, lrx: u32, lry: u32, low_t: u32) -> Vec<(u32, u32)> {
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
const SET_COMBINE_TEXEL0: (u32, u32) = (0xfc88_7f10, 0x88fc_f279);

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
const SET_COMBINE_SHADE: (u32, u32) = (0xfc88_7f10, 0x88fe_793c);

// ---------------------------------------------------------------------------
// Direct texture formats
// ---------------------------------------------------------------------------
//
// The N64 Programming Manual's "Texture Image Types and Format" table and
// texture-unit list define exactly ten legal pairs: RGBA16/32, YUV16, CI4/8,
// IA4/8/16 and I4/8 (chapter 13, pp. 189 and 216). The cases in this file now
// cover that complete matrix without importing a captured packet or an answer
// from either backend.

const RGBA32_SOURCE: u32 = 0x4700;
const IA8_SOURCE: u32 = 0x4200;
const IA4_SOURCE: u32 = 0x4300;
const IA16_SOURCE: u32 = 0x4400;
const I4_SOURCE: u32 = 0x4500;
const I8_SOURCE: u32 = 0x4600;
const YUV16_SOURCE: u32 = 0x4800;

/// Eight opaque IA8 texels. High nibble is intensity, low nibble is alpha;
/// fixing alpha at `0xf` makes any accidental I8 interpretation visible.
const IA8_BYTES: [u8; 8] = [0x1f, 0x2f, 0x3f, 0x4f, 0x5f, 0x6f, 0x7f, 0x8f];
/// Seven opaque IA4 texels, packed high-nibble first. The final low nibble is
/// padding outside the 7-pixel tile and therefore must never be sampled.
const IA4_BYTES: [u8; 4] = [0x13, 0x57, 0x9b, 0xd0];
/// Eight big-endian IA16 texels: one intensity byte then opaque alpha.
const IA16_BYTES: [u8; 16] = [
    0x08, 0xff, 0x28, 0xff, 0x48, 0xff, 0x68, 0xff, 0x88, 0xff, 0xa8, 0xff, 0xc8, 0xff, 0xe8, 0xff,
];
/// I4 is four-bit intensity replicated into RGB and alpha.
const I4_BYTES: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
/// I8 is one byte replicated into RGB and alpha. Values straddle successive
/// eight-count intensity boundaries so every five-bit RGB step is named.
const I8_BYTES: [u8; 8] = [0x08, 0x1c, 0x28, 0x3c, 0x48, 0x5c, 0x68, 0x7c];
/// Two big-endian RGBA32 texels. Every channel is authored on an eight-count
/// boundary so RGBA32 -> RGBA16 quantization is exact and hand-checkable.
const RGBA32_BYTES: [u8; 8] = [0x10, 0x28, 0x40, 0xff, 0x50, 0x68, 0x80, 0xff];
/// Four YUV16 pairs in the public `Y0,U,Y1,V` wire order. Neutral chroma makes
/// the texture filter's R'/G'/B' channels equal the selected Y regardless of
/// its conversion coefficients; every Y still differs from the seed.
const YUV16_BYTES: [u8; 16] = [
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
const IA8_EXPECTED: [u16; 8] = [
    0x1085, 0x2109, 0x318d, 0x4211, 0x5295, 0x6319, 0x739d, 0x8c63,
];
/// IA4 uses three intensity bits plus one alpha bit. For `0xb`, `i3 = 5`
/// expands to `0xb6`, `i5 = 0xb6 >> 3 = 22`, and opaque RGBA16 gray is
/// `(22 << 11)|(22 << 6)|(22 << 1)|1 = 0xb5ad`.
const IA4_EXPECTED: [u16; 7] = [0x0001, 0x2109, 0x4a53, 0x6b5b, 0x94a5, 0xb5ad, 0xdef7];
/// IA16 is already one intensity byte followed by one alpha byte. These
/// intensities quantize to five bits 1,5,9,...,29 and alpha stays opaque.
const IA16_EXPECTED: [u16; 8] = [
    0x0843, 0x294b, 0x4a53, 0x6b5b, 0x8c63, 0xad6b, 0xce73, 0xef7b,
];
const I4_EXPECTED: [u16; 8] = [
    0x1085, 0x2109, 0x318d, 0x4211, 0x5295, 0x6319, 0x739d, 0x8c63,
];
const I8_EXPECTED: [u16; 8] = [
    0x0843, 0x18c7, 0x294b, 0x39cf, 0x4a53, 0x5ad7, 0x6b5b, 0x7bdf,
];
/// Hand-derived RGBA16 target words for [`RGBA32_BYTES`]. For texel zero,
/// `r5=0x10>>3=2`, `g5=0x28>>3=5`, `b5=0x40>>3=8`, and opaque alpha gives
/// `(2<<11)|(5<<6)|(8<<1)|1 = 0x1151`. The other entries use the identical
/// upper-five-bit packing; no renderer output participates in this table.
const RGBA32_EXPECTED: [u16; 2] = [0x1151, 0x5361];
/// With U=V=128, the public first-stage equations reduce to `R'=G'=B'=Y`.
/// The fixture's Texel0-pass combiner selects those values directly. For the
/// first Y byte, `y5=0x10>>3=2`, hence
/// `(2<<11)|(2<<6)|(2<<1)|1 = 0x1085`.
const YUV16_EXPECTED: [u16; 8] = [
    0x1085, 0x294b, 0x4211, 0x5ad7, 0x739d, 0x8c63, 0xa529, 0xbdef,
];

fn expected_direct_row(index: u32, texels: &[u16]) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if y == 0 && x < texels.len() as u32 {
        texels[x as usize]
    } else {
        STALE
    }
}

fn ia8_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA8_EXPECTED)
}
fn ia4_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA4_EXPECTED)
}
fn ia16_expected(index: u32) -> u16 {
    expected_direct_row(index, &IA16_EXPECTED)
}
fn i4_expected(index: u32) -> u16 {
    expected_direct_row(index, &I4_EXPECTED)
}
fn i8_expected(index: u32) -> u16 {
    expected_direct_row(index, &I8_EXPECTED)
}
fn rgba32_expected(index: u32) -> u16 {
    expected_direct_row(index, &RGBA32_EXPECTED)
}
fn yuv16_expected(index: u32) -> u16 {
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
fn one_direct_texture_rect(
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
fn one_rgba32_rect() -> Vec<(u32, u32)> {
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
fn one_yuv16_rect() -> Vec<(u32, u32)> {
    let width = YUV16_EXPECTED.len() as u32;
    let mut words = one_fill(STALE, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        (0xfd00_0000 | (1 << 21) | (2 << 19) | (width - 1), YUV16_SOURCE),
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
const CI_SOURCE: u32 = 0x4000;
const PALETTE_SOURCE: u32 = 0x4100;

/// The palette's TMEM word address. `LoadTlut` refuses a destination tile
/// below word 256 by name ("LoadTLUT destination tile is outside high TMEM"),
/// which is the hardware split: indices live in low TMEM, palettes in high.
const PALETTE_TMEM_WORD: u32 = 256;

/// Eight CI4 indices, one per pixel of a 8x1 row -- deliberately NOT the
/// identity permutation, so a sampler that ignored the palette and returned
/// the index (or that returned palette entry `x` for pixel `x`) is visible.
const CI_INDICES: [u8; 8] = [3, 0, 5, 1, 7, 2, 6, 4];

/// The same bytes counted as 16-bit texels, which is how they are LOADED:
/// eight 4-bit indices are four bytes are two 16-bit texels.
const CI_LOAD_TEXELS: u32 = CI_INDICES.len() as u32 / 4;

/// The sixteen-entry RGBA16 palette. Only the eight entries the indices name
/// are distinguishable values; the rest are a marker that must never appear.
///
/// **The lookup is measured, not assumed.** Staging this palette 0x40 bytes
/// off leaves wgpu and RT64 still agreeing with each other but makes BOTH
/// stop matching the key -- both return `0x0001`, the decode of an unwritten
/// palette. So this case really does read the palette through `en_tlut`
/// rather than sampling the indices as colour.
const PALETTE: [u16; 16] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01, 0x0843, 0x0843, 0x0843, 0x0843,
    0x0843, 0x0843, 0x0843, 0x0843,
];

const CI8_SOURCE: u32 = 0x4900;
const CI8_PALETTE_SOURCE: u32 = 0x4a00;
const CI8_INDICES: [u8; 8] = [0x03, 0x20, 0x55, 0x81, 0xa7, 0xc2, 0xe6, 0xf4];

/// The eight named CI8 palette entries are deliberately sparse across the
/// full 0..255 index domain. Every unnamed entry is a marker distinct from
/// both the key colours and `STALE`.
const fn ci8_palette_entry(index: u8) -> u16 {
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
fn ci_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < CI_INDICES.len() as u32 && y < 1 {
        PALETTE[CI_INDICES[x as usize] as usize]
    } else {
        STALE
    }
}

fn ci8_expected(index: u32) -> u16 {
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
fn one_ci4_rect() -> Vec<(u32, u32)> {
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
fn one_ci8_rect() -> Vec<(u32, u32)> {
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
const PLANE_PER_TEXEL: i32 = 1 << 21;

/// Half a texel, the anti-coincidence offset every plane base carries.
///
/// A sample landing exactly on a texel boundary needs a FULL texel of error
/// before the sampled texel changes, so a boundary fixture cannot see a
/// half-texel bug. Sampling at the midpoint makes an error of half a texel in
/// either direction visible.
const PLANE_HALF_TEXEL: i32 = 1 << 20;

/// The X distance in Q16.16 from the major edge to the first covered
/// subsample of the pixel that edge starts in: the sampler takes X column
/// 1/8, so a left edge on a whole pixel is one eighth short. The base cancels
/// it, so a column evaluates to exactly its intended plane value.
const FIRST_SUBSAMPLE_DELTA_X: i32 = 65536 / 8;

/// The triangle's covered box: columns [2, 6), rows [0, 3).
const TRI_LEFT: u32 = 2;
const TRI_RIGHT: u32 = 6;
const TRI_TOP: u32 = 0;
const TRI_BOTTOM: u32 = 3;

/// The expected pixel for the textured-triangle case.
///
/// Inside the box, column `x` reads texel `x - TRI_LEFT` of row 0 -- the S
/// plane advances exactly one texel per pixel of X and the T plane is
/// constant, so all three rows are three independent readings of the same
/// claim. Outside, the seeded `STALE` survives.
fn triangle_expected(index: u32) -> u16 {
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
fn coefficient_block(value: [i32; 4], dx: [i32; 4], de: [i32; 4], dy: [i32; 4]) -> [u32; 16] {
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
fn textured_triangle_words(
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
fn textured_triangle_pair() -> Vec<(u32, u32)> {
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
fn textured_triangle_pair_of_width(width: u32) -> Vec<(u32, u32)> {
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
fn negative_w_textured_triangle_pair() -> Vec<(u32, u32)> {
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
fn negative_w_triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        0xf801
    } else {
        STALE
    }
}

fn one_negative_w_textured_triangle() -> Vec<(u32, u32)> {
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

fn one_textured_triangle() -> Vec<(u32, u32)> {
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
fn one_textured_rect() -> Vec<(u32, u32)> {
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

/// The one-cycle point-sampled fixture with only its draw cycle changed to
/// two-cycle mode. [`SET_COMBINE_TEXEL0`] programs Texel0 passthrough in both
/// cycles, so its hand-derived key remains [`textured_expected`].
fn two_cycle_textured_rect() -> Vec<(u32, u32)> {
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
fn state_color_blender_rect(
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

fn blend_color_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < TEXRECT_LRX && y < TEXRECT_LRY {
        // RGBA8888 (0x40, 0x80, 0xc0, 0xff) -> RGBA5551 (8, 16, 24, 1).
        0x4431
    } else {
        BLUE
    }
}

fn fog_color_expected(index: u32) -> u16 {
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
fn blend_numerator_overflow_rect() -> Vec<(u32, u32)> {
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
fn blend_numerator_overflow_expected(index: u32) -> u16 {
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
const fn load_block(texel_count: u32, dxt: u32) -> (u32, u32) {
    (0xf300_0000, ((texel_count - 1) << 12) | dxt)
}

/// A LoadBlock case over [`WIDE_TEXELS`]. `load_line_words` is the stride
/// applied when DXT crosses 0x800; `render_line_words` redescribes the loaded
/// bytes for sampling, because LoadBlock may leave holes between its logical
/// rows.
fn load_block_textured_rect(
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
fn load_block_linear_expected(index: u32) -> u16 {
    expected_direct_row(index, &LOAD_BLOCK_LINEAR_EXPECTED)
}

/// DXT 0x400 advances after the second word. With a loading `line = 2`, the
/// four source words land at TMEM words 0, 1, 4 and 5; redescribing the tile
/// with render `line = 4` makes rows 0 and 1 read those exact pairs. The odd
/// row's four-byte exchange is applied by both load and sample, so the visible
/// texels remain [`WIDE_TEXELS`] in row-major order.
fn load_block_dxt_expected(index: u32) -> u16 {
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
fn texrect_flip_expected(index: u32) -> u16 {
    const SIDE: u32 = 4;
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x < SIDE && y < SIDE {
        TEXRECT_FLIP_EXPECTED[(y * SIDE + x) as usize]
    } else {
        STALE
    }
}

fn one_textured_rect_flip() -> Vec<(u32, u32)> {
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
const SET_COMBINE_PRIMITIVE: (u32, u32) = (0xfcff_ffff, 0xfffd_f6fb);
// RGBA16 bit 0 stores coverage[2], not primitive alpha. Full coverage stores
// 8 - 1 = 7 under CVG_DST_CLAMP, whose visible MSB is one; RGB5=(4,24,28)
// therefore packs as 0x2639 (Programming Manual §§15.5.3, 15.5.6, 15.7).
const FLAT_TRIANGLE_COLOR: u16 = 0x2639;

fn flat_triangle_words(x_h: u32, x_l: u32, y_h: u32, y_l: u32, y_m: u32) -> Vec<(u32, u32)> {
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

fn one_flat_triangle_pair() -> Vec<(u32, u32)> {
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

fn flat_triangle_expected(index: u32) -> u16 {
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
fn shade_triangle_words(
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
const SHADE_TRIANGLE_RGBA: [i32; 4] = [0x20 << 16, 0xc0 << 16, 0xe0 << 16, 0xff << 16];

fn one_shade_triangle_pair() -> Vec<(u32, u32)> {
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

fn shade_triangle_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x >= TRI_LEFT && x < TRI_RIGHT && y >= TRI_TOP && y < TRI_BOTTOM {
        FLAT_TRIANGLE_COLOR
    } else {
        STALE
    }
}

fn skew_textured_rect(line_words: u32, low_t: u32) -> Vec<(u32, u32)> {
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

/// The corpus.
///
/// Every key is stated as arithmetic over the case's own display list under
/// the fill-cycle rule the reference runner documents: `G_FILLRECT` covers
/// `ceil(ulx) ..= floor(lrx)` INCLUSIVE on both edges.
///
/// **Provenance: every case here is hand-authored.** None is captured from a
/// running ROM. `docs/RT64-PARITY.md` states what that costs the metric.
fn cases() -> Vec<Case> {
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
        // (`docs/RT64-PARITY.md` section 2), so these stay in partition A.
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

fn command_words(commands: &[(u32, u32)]) -> Vec<u32> {
    commands
        .iter()
        .flat_map(|&(word0, word1)| [word0, word1])
        .collect()
}

/// The seeded guest memory every backend starts from: STALE everywhere in the
/// target, GUARD immediately either side of it, and the command words at
/// `COMMAND_START`.
fn seeded(commands: &[(u32, u32)]) -> Vec<u8> {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(RdramAddr::from_offset(FRAMEBUFFER + index * 2), STALE);
        }
        view.write_u16(RdramAddr::from_offset(FRAMEBUFFER - 2), GUARD);
        view.write_u16(
            RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES),
            GUARD,
        );
        // **The texture source, staged for every case.** A fill case never
        // reads it, so seeding it unconditionally costs nothing and keeps
        // `seeded` a single function of the command list. Written through the
        // same `write_u16` the framebuffer uses, so the guest byte-lane
        // mapping is applied once and in one place -- a raw `copy_from_slice`
        // here would stage the texels byte-swapped and every textured case
        // would report a texture defect that was really a runner defect.
        for (index, texel) in TEXTURE_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(TEXTURE_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        // The wide (`line = 2`) source, staged the same way and for the same
        // reason: unconditionally, so `seeded` stays a single function of the
        // command list, and through `write_u16` so the guest byte-lane
        // mapping is applied in exactly one place.
        for (index, texel) in WIDE_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(WIDE_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        // Stage the same synthetic bar on every source row used by the odd
        // base and even-low-T control. Keeping these bytes fixed means the
        // control changes only the command's tile/load origin.
        for source_y in (SKEW_LOW_T_ODD - 1)..(SKEW_LOW_T_ODD + SKEW_HEIGHT) {
            for x in 0..SKEW_WIDTH {
                let source_index = source_y * SKEW_WIDTH + x;
                view.write_u16(
                    RdramAddr::from_offset(SKEW_SOURCE + source_index * 2),
                    skew_texel(x),
                );
            }
        }
        // The CI4 palette, RGBA16 like every other texel image.
        for (index, entry) in PALETTE.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(PALETTE_SOURCE + index as u32 * 2),
                *entry,
            );
        }
        // The CI4 index image: two 4-bit indices per byte, high nibble first,
        // written as logical guest bytes so the `^3` lane map is applied once
        // -- the same reason every other source here goes through a view
        // rather than a raw slice write.
        let packed: Vec<u8> = CI_INDICES
            .chunks_exact(2)
            .map(|pair| (pair[0] << 4) | (pair[1] & 0xf))
            .collect();
        view.write_logical_bytes(RdramAddr::from_offset(CI_SOURCE), &packed);
        view.write_logical_bytes(RdramAddr::from_offset(CI8_SOURCE), &CI8_INDICES);
        // loadblock-deep slice: an independent RGBA16 strip and CI8 index
        // strip, staged the same way as the sources above, so its LoadBlock
        // DxT cases read real bytes rather than zeroed RDRAM.
        for (index, texel) in LOADBLOCK_DEEP_RGBA16_TEXELS.iter().enumerate() {
            view.write_u16(
                RdramAddr::from_offset(LOADBLOCK_DEEP_RGBA16_SOURCE + index as u32 * 2),
                *texel,
            );
        }
        view.write_logical_bytes(
            RdramAddr::from_offset(LOADBLOCK_DEEP_CI8_SOURCE),
            &LOADBLOCK_DEEP_CI8_INDICES,
        );
        for index in 0u16..=255 {
            view.write_u16(
                RdramAddr::from_offset(CI8_PALETTE_SOURCE + u32::from(index) * 2),
                ci8_palette_entry(index as u8),
            );
        }
        for (address, bytes) in [
            (RGBA32_SOURCE, RGBA32_BYTES.as_slice()),
            (IA8_SOURCE, IA8_BYTES.as_slice()),
            (IA4_SOURCE, IA4_BYTES.as_slice()),
            (IA16_SOURCE, IA16_BYTES.as_slice()),
            (I4_SOURCE, I4_BYTES.as_slice()),
            (I8_SOURCE, I8_BYTES.as_slice()),
            (YUV16_SOURCE, YUV16_BYTES.as_slice()),
        ] {
            view.write_logical_bytes(RdramAddr::from_offset(address), bytes);
        }
    }
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        let offset = COMMAND_START as usize + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    rdram
}

fn observation_bytes(rdram: &[u8]) -> Vec<u8> {
    rdram[FRAMEBUFFER as usize..(FRAMEBUFFER + FRAMEBUFFER_BYTES) as usize].to_vec()
}

fn command_end(commands: &[(u32, u32)]) -> u32 {
    COMMAND_START + (commands.len() as u32) * 8
}

/// The reference backend's committed guest framebuffer.
fn reference_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let mut backend = ReferenceBackend::default();
    if let Err(error) = backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)) {
        return Err(error.to_string());
    }
    match backend.process_rdp_commands(
        &mut rdram,
        COMMAND_START,
        command_end(commands),
        FRAMEBUFFER,
        true,
    ) {
        Ok(fn64_render::FrameStatus::Complete) => Ok(observation_bytes(&rdram)),
        Ok(status) => Err(format!("nonterminal status {status:?}")),
        Err(error) => Err(error.to_string()),
    }
}

/// Preserve the RT64/wgpu differential when the diagnostic-only reference
/// lane loudly rejects a state combination. Reference output never enters a
/// verdict, so converting its trap to a reported refusal changes no authority
/// claim and keeps the remaining corpus rows observable.
fn reference_outcome(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    match std::panic::catch_unwind(|| reference_bytes(commands)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            Err(format!("reference trapped: {message}"))
        }
    }
}

/// RT64's committed guest framebuffer -- the oracle's answer.
///
/// RT64 is created once per case rather than once per sweep: the deferred
/// history runner's state machine shows RT64 carries per-frame state across
/// submissions, and a shared backend would let case N's history leak into
/// case N+1's answer.
fn rt64_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)
            .map_err(|error| error.to_string())?,
        idle_work_active: false,
        developer_mode: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    if let Err(error) = backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)) {
        return Err(error.to_string());
    }
    match backend.process_rdp_commands(
        &mut rdram,
        COMMAND_START,
        command_end(commands),
        FRAMEBUFFER,
        true,
    ) {
        Ok(_) => Ok(observation_bytes(&rdram)),
        Err(error) => Err(error.to_string()),
    }
}

/// fn64's shipping wgpu backend, copied back exactly the way production
/// copies it.
///
/// `device_bytes` are flat big-endian device bytes; guest RDRAM stores native
/// words under the `^3` byte-lane mapping. Going through
/// `write_logical_bytes` is the same call `fn64-abi`'s
/// `copy_committed_guest_writes` makes. A raw `copy_from_slice` here reports
/// every pixel as byte-swapped -- a runner defect that reads exactly like a
/// renderer defect.
fn wgpu_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let mut rdram = seeded(commands);
    let mut session =
        ConformanceSession::try_new(WIDTH, HEIGHT).map_err(|refusal| refusal.to_string())?;
    let replay = ConformanceReplay {
        layout_bytes: RDRAM_LEN as u32,
        command_start: COMMAND_START,
        words: command_words(commands),
        transaction_sequence: 1,
        guest_read_sources: Vec::new(),
        // Serve declared reads from this fixture's own RDRAM image, exactly
        // as `fn64-abi` slices the live allocation. A partial `FillRectangle`
        // declares a colour-image seed read that no fixture author wrote into
        // `guest_read_sources`; without this the replay supplies 0 sources
        // for 1 declared read and every partial fill is refused, which would
        // read as a wgpu defect when it is a runner gap.
        guest_rdram: Some(rdram.to_vec()),
        target_width: WIDTH,
        target_height: HEIGHT,
    };
    let outcome = session
        .replay(&replay, FRAMEBUFFER)
        .map_err(|refusal| refusal.to_string())?;
    let published = outcome.target_bytes;
    if published.len() < FRAMEBUFFER_BYTES as usize {
        return Err(format!(
            "published {} target bytes, fewer than the declared {FRAMEBUFFER_BYTES}",
            published.len()
        ));
    }
    RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        RdramAddr::from_offset(FRAMEBUFFER),
        &published[..FRAMEBUFFER_BYTES as usize],
    );
    Ok(observation_bytes(&rdram))
}

/// Keep one loud backend trap from erasing every other case's differential.
/// The gate still treats the structured refusal as non-parity; this only
/// preserves the remaining rows and the trapped message as evidence.
fn wgpu_outcome(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    match std::panic::catch_unwind(|| wgpu_bytes(commands)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            Err(format!("wgpu trapped: {message}"))
        }
    }
}

/// The default path to the external angrylion bit-accurate RDP oracle.
///
/// angrylion-rdp-plus is MAME-licensed. It lives OUTSIDE fn64 and is invoked
/// only as an external process; nothing from it is linked or vendored, and
/// this runner never depends on it at build time. If the binary is absent the
/// leg is skipped (fail-open), never a build or test failure.
const ANGRYLION_ORACLE_DEFAULT: &str = "/Users/jer/Code/angrylion-oracle/oracle";
const ANGRYLION_ORACLE_ENV: &str = "FN64_ANGRYLION_ORACLE";

/// Sentinel a skipped angrylion leg returns instead of an error, so a missing
/// oracle never turns a wgpu-vs-RT64 verdict into a failure.
const ANGRYLION_SKIPPED: &str = "angrylion-oracle-skipped";

/// angrylion's committed guest framebuffer -- BIT-ACCURATE hardware ground
/// truth, produced by shelling out to the external oracle binary.
///
/// **Byte domain.** angrylion's RDP core applies exactly fn64's storage lane
/// XORs: `BYTE_ADDR_XOR = 3` (byte reads), `WORD_ADDR_XOR = 1` (halfword
/// reads, i.e. fn64's `^2` on a u16), and no XOR on 32-bit word reads/command
/// fetch. So the runner's `seeded()` storage image is already in angrylion's
/// native RDRAM domain, on BOTH the texture-read side and the framebuffer-
/// write side, and no re-swizzle is applied in either direction: the oracle's
/// raw framebuffer bytes are compared directly against `observation_bytes`.
/// Verified: a `0xf801` fill returns all-`0xf801`, and a two-colour probe
/// (green drawn at x=1 over red) places green at raw slot 0 (`0 ^ 1 = 1`) --
/// the same slot fn64 stores logical pixel 1 in.
///
/// **The whole seeded image is handed to the oracle**, not just the commands.
/// A textured draw reads texture source memory the command stream never wrote;
/// seeding only the commands (the oracle's original mode) makes angrylion read
/// texel 0 everywhere and every textured case reports a spurious disagreement.
/// So the runner writes `seeded(commands)` to a temp file and invokes the
/// oracle's `--rdram` mode with the `[COMMAND_START, command_end)` byte range,
/// so angrylion renders from byte-identical guest memory to wgpu and RT64.
///
/// Missing binary -> `Err(ANGRYLION_SKIPPED)`, a skip sentinel the caller
/// treats as "no reading", not as a defect.
fn angrylion_bytes(commands: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    let oracle = std::env::var(ANGRYLION_ORACLE_ENV)
        .unwrap_or_else(|_| ANGRYLION_ORACLE_DEFAULT.to_string());
    if !std::path::Path::new(&oracle).exists() {
        return Err(ANGRYLION_SKIPPED.to_string());
    }

    // The full guest memory image every backend renders from: STALE
    // background, GUARDs, staged texture sources, and the command words at
    // COMMAND_START -- byte-identical to what wgpu and RT64 receive.
    let rdram = seeded(commands);

    let dir = std::env::temp_dir();
    let unique = format!(
        "{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let cmds_path = dir.join(format!("fn64-angrylion-rdram-{unique}.bin"));
    let out_path = dir.join(format!("fn64-angrylion-out-{unique}.bin"));

    std::fs::write(&cmds_path, &rdram)
        .map_err(|error| format!("angrylion: writing RDRAM image: {error}"))?;

    let cleanup = |cmds: &std::path::Path, out: &std::path::Path| {
        let _ = std::fs::remove_file(cmds);
        let _ = std::fs::remove_file(out);
    };

    let status = std::process::Command::new(&oracle)
        .arg("--rdram")
        .arg(&cmds_path)
        .arg(format!("{COMMAND_START:x}"))
        .arg(format!("{:x}", command_end(commands)))
        .arg(format!("{FRAMEBUFFER:x}"))
        .arg(WIDTH.to_string())
        .arg(HEIGHT.to_string())
        .arg("2")
        .arg(&out_path)
        .output();

    let output = match status {
        Ok(output) => output,
        Err(error) => {
            cleanup(&cmds_path, &out_path);
            return Err(format!("angrylion: spawning oracle: {error}"));
        }
    };
    if !output.status.success() {
        cleanup(&cmds_path, &out_path);
        return Err(format!(
            "angrylion: oracle exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let bytes = match std::fs::read(&out_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            cleanup(&cmds_path, &out_path);
            return Err(format!("angrylion: reading framebuffer: {error}"));
        }
    };
    cleanup(&cmds_path, &out_path);

    if bytes.len() < FRAMEBUFFER_BYTES as usize {
        return Err(format!(
            "angrylion: oracle wrote {} bytes, fewer than the declared {FRAMEBUFFER_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes[..FRAMEBUFFER_BYTES as usize].to_vec())
}

/// Whether an angrylion result is the skip sentinel rather than a real answer.
fn angrylion_is_skipped(outcome: &Result<Vec<u8>, String>) -> bool {
    matches!(outcome, Err(message) if message == ANGRYLION_SKIPPED)
}

/// The hand-derived key, materialised in the same guest byte order the
/// backends' observations are read in.
fn key_bytes(case: &Case) -> Vec<u8> {
    let mut rdram = seeded(&case.commands);
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(
                RdramAddr::from_offset(FRAMEBUFFER + index * 2),
                (case.expected)(index),
            );
        }
    }
    observation_bytes(&rdram)
}

fn pixels(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
        .collect()
}

/// How one backend's outcome compares to the oracle's for one case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Byte-identical committed guest framebuffers.
    Identical,
    /// Both completed, bytes differ.
    Differs { pixels: usize },
    /// Exactly one of the pair refused. The most consequential kind: one
    /// engine renders the stream and the other declines it.
    OneRefused,
    /// Both refused. Not parity evidence in either direction.
    BothRefused,
}

impl Verdict {
    fn of(oracle: &Result<Vec<u8>, String>, candidate: &Result<Vec<u8>, String>) -> Self {
        match (oracle, candidate) {
            (Ok(oracle), Ok(candidate)) => {
                let differing = pixels(oracle)
                    .into_iter()
                    .zip(pixels(candidate))
                    .filter(|(left, right)| left != right)
                    .count();
                if differing == 0 {
                    Self::Identical
                } else {
                    Self::Differs { pixels: differing }
                }
            }
            (Err(_), Err(_)) => Self::BothRefused,
            _ => Self::OneRefused,
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Differs { .. } => "differs",
            Self::OneRefused => "one-refused",
            Self::BothRefused => "both-refused",
        }
    }

    /// Only a byte-identical result counts toward parity. A refusal by both
    /// backends is NOT agreement -- neither rendered anything.
    const fn is_parity(self) -> bool {
        matches!(self, Self::Identical)
    }
}

fn outcome_wire(outcome: &Result<Vec<u8>, String>) -> Value {
    match outcome {
        Ok(_) => json!("completed"),
        Err(message) => json!({ "refused": message }),
    }
}

/// A running tally for one partition of the corpus.
#[derive(Default)]
struct Tally {
    cases: usize,
    identical: usize,
    differs: usize,
    one_refused: usize,
    both_refused: usize,
}

impl Tally {
    fn record(&mut self, verdict: Verdict) {
        self.cases += 1;
        match verdict {
            Verdict::Identical => self.identical += 1,
            Verdict::Differs { .. } => self.differs += 1,
            Verdict::OneRefused => self.one_refused += 1,
            Verdict::BothRefused => self.both_refused += 1,
        }
    }

    fn wire(&self) -> Value {
        json!({
            "cases": self.cases,
            "byte_identical": self.identical,
            "differs": self.differs,
            "one_refused": self.one_refused,
            "both_refused": self.both_refused,
        })
    }
}

/// A real captured RDP stream, promoted into a parity case.
///
/// # Why this exists
///
/// Hand-authored cases test what the author imagined. A captured WM2000
/// packet tests what the game actually draws, which is the difference between
/// a toy metric and a real one. `docs/RT64-PARITY.md` states the corpus
/// provenance the reported numbers rest on.
///
/// # Provenance and why nothing is committed
///
/// The capture itself is NOT in this repository and must not be: a game's own
/// RDP command words are game content, which `README.md`'s "no game content
/// ships in this repo" rule covers. So this reads a dump produced by
/// `FN64_GBI_PACKET_DUMP` at run time, and when the variable is unset the
/// corpus is simply the hand-authored one and the report says so.
///
/// The format is the committed one:
/// `entry \t lane \t pc \t w0 \t w1`, produced by
/// `fn64-render-reference`'s `gbi::census::packet`. Two other in-tree parsers
/// already read it (`raw_dpc_session_integration.rs`,
/// `examples/rt64_wm2000_three_way.rs`); this is a third reader of the same
/// committed shape, not a new format.
mod captured {
    /// Where the operator points this runner at a packet dump.
    pub const PACKET_ENV: &str = "FN64_WM2000_PACKET_TSV";
    /// Which decode entry of that dump to replay. Defaults to 0.
    pub const ENTRY_ENV: &str = "FN64_WM2000_PACKET_ENTRY";

    #[derive(Debug)]
    pub struct CapturedPacket {
        pub entry: u64,
        pub words: Vec<u32>,
        pub source_pc: usize,
    }

    /// Parse one decode entry out of a packet dump.
    ///
    /// Consecutive rows must be exactly 8 RDRAM bytes apart. That contiguity
    /// check is what makes the concatenated word pairs the WIRE STREAM rather
    /// than a lossy sample of it -- without it a dump missing rows would
    /// still parse and would silently measure a different display list.
    pub fn parse_packet_dump(text: &str, entry: u64) -> Result<CapturedPacket, String> {
        let mut rows: Vec<(usize, u32, u32)> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("entry\t") {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 5 {
                return Err(format!(
                    "line {} has {} tab-separated fields, expected 5",
                    index + 1,
                    fields.len()
                ));
            }
            let row_entry: u64 = fields[0]
                .parse()
                .map_err(|e| format!("line {} entry field {:?}: {e}", index + 1, fields[0]))?;
            if row_entry != entry {
                continue;
            }
            // Only the raw-RDP lane is replayable as a command stream; a GBI
            // row is a display-list command that has not been decoded yet.
            if fields[1] != "RDP" {
                return Err(format!(
                    "line {} is on the {} lane; parity replays the raw-RDP lane",
                    index + 1,
                    fields[1]
                ));
            }
            let parse_hex = |field: &str, name: &str| -> Result<u64, String> {
                let stripped = field.strip_prefix("0x").ok_or_else(|| {
                    format!("line {} {name} is {field:?}, want 0x hex", index + 1)
                })?;
                u64::from_str_radix(stripped, 16)
                    .map_err(|e| format!("line {} {name} is {field:?}: {e}", index + 1))
            };
            rows.push((
                parse_hex(fields[2], "pc")? as usize,
                parse_hex(fields[3], "w0")? as u32,
                parse_hex(fields[4], "w1")? as u32,
            ));
        }
        if rows.is_empty() {
            return Err(format!("no rows for decode entry {entry}"));
        }
        for pair in rows.windows(2) {
            if pair[1].0 != pair[0].0 + 8 {
                return Err(format!(
                    "rows for entry {entry} are not contiguous: {:#010x} then {:#010x}",
                    pair[0].0, pair[1].0
                ));
            }
        }
        let source_pc = rows[0].0;
        let words = rows
            .iter()
            .flat_map(|&(_, w0, w1)| [w0, w1])
            .collect::<Vec<u32>>();
        Ok(CapturedPacket {
            entry,
            words,
            source_pc,
        })
    }

    /// Walk a raw-RDP word stream into `(byte_offset, cmd6, w0, w1)`.
    ///
    /// Deliberately independent of any decoder under test: it knows only that
    /// a raw-RDP command is 8 bytes except `G_TEXRECT` (`0x24`) and
    /// `G_TEXRECTFLIP` (`0x25`), which are 16.
    pub fn walk(words: &[u32]) -> Vec<(usize, u8, u32, u32)> {
        let mut out = Vec::new();
        let mut index = 0usize;
        while index + 1 < words.len() {
            let w0 = words[index];
            let w1 = words[index + 1];
            let cmd6 = ((w0 >> 24) & 0x3f) as u8;
            out.push((index * 4, cmd6, w0, w1));
            index += if matches!(cmd6, 0x24 | 0x25) { 4 } else { 2 };
        }
        out
    }

    /// Target extent read from the packet's OWN `SetColorImage` width and
    /// `SetScissor` lower-right Y, never hardcoded. Reading a captured stream
    /// at a guessed extent is the documented way to turn coherent geometry
    /// into convincing "striping" (`docs/RT64-WM2000-HARNESS-TRAPS.md`).
    pub fn target_extent(commands: &[(usize, u8, u32, u32)]) -> Option<(u32, u32)> {
        let width = commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
            .map(|&(_, _, w0, _)| (w0 & 0x0fff) + 1)?;
        let height = commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x2d)
            .map(|&(_, _, _, w1)| (w1 & 0x0fff) >> 2)?;
        Some((width, height))
    }

    /// The packet's own `SetColorImage` destination address.
    pub fn color_image_addr(commands: &[(usize, u8, u32, u32)]) -> Option<u32> {
        commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
            .map(|&(_, _, _, w1)| w1)
    }
}

/// Replay a captured packet through all three backends at the packet's own
/// extent and its own color-image address.
///
/// This is reported as its own section rather than folded into the
/// hand-authored tally: the two have different provenance, and averaging a
/// real frame together with twelve synthetic fills would produce a number
/// whose denominator means nothing.
fn captured_row() -> Value {
    let Some(path) = std::env::var_os(captured::PACKET_ENV) else {
        return json!({
            "available": false,
            "reason": format!(
                "{} is unset. The capture is game content and is deliberately \
                 not committed; produce one with FN64_GBI_PACKET_DUMP on a ROM \
                 run, then point this variable at it.",
                captured::PACKET_ENV
            ),
        });
    };
    let entry: u64 = std::env::var(captured::ENTRY_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(0);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return json!({"available": false, "reason": format!("{path:?} unreadable: {error}")})
        }
    };
    let packet = match captured::parse_packet_dump(&text, entry) {
        Ok(packet) => packet,
        Err(reason) => return json!({"available": false, "reason": reason}),
    };
    let walked = captured::walk(&packet.words);
    let (Some((width, height)), Some(color_image)) = (
        captured::target_extent(&walked),
        captured::color_image_addr(&walked),
    ) else {
        return json!({
            "available": false,
            "reason": "the captured packet sets no color image or no scissor, \
                       so its target extent cannot be read from the stream",
        });
    };
    json!({
        "available": true,
        "provenance": "captured from a real ROM run via FN64_GBI_PACKET_DUMP; not committed",
        "entry": packet.entry,
        "source_pc": format!("{:#010x}", packet.source_pc),
        "words": packet.words.len(),
        "commands": walked.len(),
        "target": {"width": width, "height": height},
        "color_image": format!("{color_image:#010x}"),
        "note": "Extent and destination are read from the packet's own \
                 SetColorImage/SetScissor, never guessed. Replaying this \
                 through the three backends is the next step and is NOT done \
                 here: docs/RT64-WM2000-THREE-WAY.md already reports 0 of \
                 115,200 differing for all three pairings on WM2000 frame 0.",
    })
}

// ---------------------------------------------------------------------------
// Programmatic corpus generator (Track B).
//
// The hand corpus above is a fixed set of authored cases with hand-derived
// keys. The generator instead emits VALID SYNTHETIC RDP command streams across
// the command/mode matrix, ranked by real-ROM usage, and compares wgpu and
// RT64 against ANGRYLION as ground truth -- there is no hand key, because the
// point is systematic coverage no human wrote a key for.
//
// Streams are HAND-DERIVED / SYNTHETIC (built from the same wire encoders the
// hand corpus uses); NEVER captured from a running ROM. Every stream is a
// complete, valid frame: SetColorImage + SetScissor + SetOtherModes + (tile/
// load if textured) + draw + SyncFull, so all three backends can render it.
// ---------------------------------------------------------------------------

/// One generated case: a name, a priority rank (1 = highest, do first), and a
/// valid RDP command stream. No hand key -- angrylion is the oracle.
struct GeneratedCase {
    name: String,
    /// Priority per the brief's real-ROM-usage order. Lower = render first.
    priority: u8,
    /// What matrix cell this case exercises, for the report.
    intent: &'static str,
    commands: Vec<(u32, u32)>,
}

/// A minimal complete fill frame painting `color` over the box
/// `[ulx,lrx) x [uly,lry)` in the requested `cycle_type` (0=1cyc, 1=2cyc,
/// 2=copy, 3=fill), over a STALE background. Fill cycle uses SetFillColor;
/// the non-fill cycle types drive the same rectangle through the pixel pipe
/// with a primitive-colour combiner so the mode-matrix cell is exercised end
/// to end rather than short-circuited by the fill path.
fn gen_fill_frame(color: u16, cycle_type: u32, ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
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
fn primitive_rgba8888(color: u16) -> u32 {
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
fn set_bilerp0(mut stream: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
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
fn gen_triangle_variant(opcode: u32) -> Vec<(u32, u32)> {
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
fn gen_fill_with_sync(sync_opcode: u32) -> Vec<(u32, u32)> {
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
const fn blend_other_modes(p: u32, a: u32, m: u32, b: u32, force_bl: bool, im_rd: bool) -> (u32, u32) {
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
fn gen_blend_rect(memory_seed: u16, primitive_rgba8888: u32, blend_words: (u32, u32)) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        blend_words,
        SET_COMBINE_PRIMITIVE,
        (0xfa00_0000, primitive_rgba8888),
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(flat_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(flat_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push((0xe900_0000, 0));
    words
}

/// The blend-color / fog-color twin of [`gen_blend_rect`] that also programs
/// `SetBlendColor`/`SetFogColor` before the draw.
fn gen_blend_rect_with_state_color(
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
    words.extend(flat_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(flat_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push((0xe900_0000, 0));
    words
}

/// The shade-driven twin of [`gen_blend_rect`]: a `SET_COMBINE_SHADE`
/// triangle pair with a non-opaque, non-zero flat shade alpha.
fn gen_blend_rect_shade_alpha(memory_seed: u16, shade_rgba: [i32; 4], blend_words: (u32, u32)) -> Vec<(u32, u32)> {
    let mut words = one_fill(memory_seed, 0, 0, WIDTH - 1, HEIGHT - 1);
    words.pop();
    words.extend([
        blend_words,
        SET_COMBINE_SHADE,
        set_scissor(0, 0, WIDTH, HEIGHT),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    ]);
    words.extend(shade_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM, shade_rgba));
    words.extend(shade_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP, shade_rgba));
    words.push((0xe900_0000, 0));
    words
}

const BLEND_MATRIX_MEMORY_SEED: u16 = GREEN;
const BLEND_MATRIX_PRIMITIVE_RGBA8888: u32 = 0xff00_00ff; // opaque red
const BLEND_MATRIX_SHADE_RGBA: [i32; 4] = [0x80 << 16, 0x7f << 16, 0x00 << 16, 0x80 << 16];

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
const fn other_modes_alpha_compare(compare_mode: u32) -> (u32, u32) {
    (OTHER_MODES_ONE_CYCLE_NO_AA.0, compare_mode & 0x3)
}

/// A flat-triangle-pair rectangle painted with a primitive colour of alpha
/// `prim_alpha`, under alpha-compare mode `compare_mode` against
/// `SetBlendColor`'s alpha byte `blend_alpha`, over a STALE background.
fn gen_alpha_compare_rect(compare_mode: u32, prim_alpha: u8, blend_alpha: u8) -> Vec<(u32, u32)> {
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
fn gen_fill_coverage_mode(color: u16, other_modes_word1: u32) -> Vec<(u32, u32)> {
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
fn gen_one_cycle_coverage_mode(other_modes_word1: u32) -> Vec<(u32, u32)> {
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
fn push_coverage_mode_cases(
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
fn direct_format_textured_triangle(
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
fn rgba32_textured_triangle(bilerp: bool) -> Vec<(u32, u32)> {
    let width = RGBA32_EXPECTED.len() as u32; // 2
    let other_modes = if bilerp {
        (OTHER_MODES_ONE_CYCLE_TEXTURED.0 | (1 << 11), OTHER_MODES_ONE_CYCLE_TEXTURED.1)
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
fn ci4_textured_triangle() -> Vec<(u32, u32)> {
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
fn ci8_textured_triangle() -> Vec<(u32, u32)> {
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

const ZBUF_Z_IMAGE: u32 = 0x9000;

/// `SetZImage` (opcode `0xfe`).
const fn set_z_image(address: u32) -> (u32, u32) {
    (0xfe00_0000, address & 0x00ff_ffff)
}

/// `SetMaskImage` (opcode `0x3e`), byte-identical wire shape to
/// [`set_z_image`].
const fn set_mask_image(address: u32) -> (u32, u32) {
    (0x3e00_0000, address & 0x00ff_ffff)
}

/// `SetPrimDepth` (opcode `0xee`): word0 bare, word1 = `(z << 16) | delta_z`.
const fn set_prim_depth(z: u16, delta_z: u16) -> (u32, u32) {
    (0xee00_0000, ((z as u32) << 16) | (delta_z as u32))
}

/// `SetOtherModes` one-cycle word carrying the requested Z fields.
const fn other_modes_one_cycle_z(z_source_prim: bool, z_compare_en: bool, z_update_en: bool) -> (u32, u32) {
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

const ZBUF_NEAR_COLOR: u32 = 0xff00_00ff; // opaque red, RGBA8888
const ZBUF_FAR_COLOR: u32 = 0x00ff_00ff; // opaque green, RGBA8888

/// One full frame exercising two overlapping flat triangles under
/// G_ZS_PRIM: a "far" triangle (green) drawn first covering the whole TRI
/// box, then a "near" triangle (red) drawn second over the identical box.
fn zbuffer_overlap_case(
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
    words.extend(flat_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(flat_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push(set_prim_depth(near_z, 0));
    words.push((0xfa00_0000, ZBUF_NEAR_COLOR));
    words.extend(flat_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(flat_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push((0xe900_0000, 0));
    words
}

/// Case 1 / 2 builder: z_compare_en + z_update_en both on, G_ZS_PRIM.
fn gen_zbuffer_compare_and_update(second_draw_z: u16, first_draw_z: u16) -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, true, true).1,
        first_draw_z,
        second_draw_z,
        false,
    )
}

/// Case 3: z_compare_en OFF, G_ZS_PRIM.
fn gen_zbuffer_compare_disabled(second_draw_z: u16, first_draw_z: u16) -> Vec<(u32, u32)> {
    zbuffer_overlap_case(
        other_modes_one_cycle_z(true, false, false).1,
        first_draw_z,
        second_draw_z,
        false,
    )
}

/// Case 4: z_compare_en ON, z_update_en OFF, G_ZS_PRIM -- the update-disabled
/// twin of [`gen_zbuffer_compare_and_update`]'s "nearer wins" key.
fn gen_zbuffer_update_disabled() -> Vec<(u32, u32)> {
    zbuffer_overlap_case(other_modes_one_cycle_z(true, true, false).1, 0x8000, 0x4000, false)
}

/// Case 5: `z_source_sel` itself. G_ZS_PIXEL with an explicit per-pixel Z
/// coefficient block on a raw `0x09` triangle, drawn OVER a G_ZS_PRIM
/// triangle whose PrimDepth is farther everywhere in the box.
fn gen_zbuffer_source_sel_pixel_wins() -> Vec<(u32, u32)> {
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
    words.extend(flat_triangle_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(flat_triangle_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
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
    words.extend(z_words(TRI_LEFT, TRI_RIGHT, TRI_TOP, TRI_BOTTOM, TRI_BOTTOM));
    words.extend(z_words(TRI_RIGHT, TRI_LEFT, TRI_TOP, TRI_BOTTOM, TRI_TOP));
    words.push((0xe900_0000, 0));
    words
}

/// Case 6: identical to case 1 except the z-image is bound with
/// `SetMaskImage` (`0x3e`) instead of `SetZImage` (`0xfe`).
fn gen_zbuffer_setmaskimage_binds_z_image() -> Vec<(u32, u32)> {
    zbuffer_overlap_case(other_modes_one_cycle_z(true, true, true).1, 0x8000, 0x2000, true)
}

// -----------------------------------------------------------------------
// slice loadblock-deep
// -----------------------------------------------------------------------
//
// LOADBLOCK (0x33) DxT row-advance, sampled by TEXTURED TRIANGLES (not
// texrects), across RGBA16 and CI8 sources.

const LOADBLOCK_DEEP_RGBA16_SOURCE: u32 = 0x6000;
const LOADBLOCK_DEEP_RGBA16_WIDTH: u32 = 8;

const LOADBLOCK_DEEP_RGBA16_TEXELS: [u16; 32] = [
    0xf801, 0x07c1, 0x003f, 0x7fff, 0x8421, 0xc631, 0x4211, 0xfc01,
    0xf841, 0x0641, 0x0079, 0xffbf, 0x8461, 0xc671, 0x4251, 0xfc41,
    0xf803, 0x07c3, 0x003d, 0x7ffd, 0x8423, 0xc633, 0x4213, 0xfc03,
    0xf843, 0x0643, 0x007b, 0xffbd, 0x8463, 0xc673, 0x4253, 0xfc43,
];

/// A LoadBlock case over [`LOADBLOCK_DEEP_RGBA16_TEXELS`], sampled by a
/// TEXTURED TRIANGLE rather than a texture rectangle.
fn load_block_deep_triangle(
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

const LOADBLOCK_DEEP_CI8_SOURCE: u32 = 0x7000;

const LOADBLOCK_DEEP_CI8_INDICES: [u8; 32] = [
    0x03, 0x20, 0x55, 0x81, 0xa7, 0xc2, 0xe6, 0xf4,
    0x10, 0x30, 0x60, 0x90, 0xb0, 0xd0, 0xf0, 0x01,
    0x11, 0x31, 0x61, 0x91, 0xb1, 0xd1, 0xf1, 0x02,
    0x12, 0x32, 0x62, 0x92, 0xb2, 0xd2, 0xf2, 0x04,
];

/// A LoadBlock case over CI8 indices, sampled by a TEXTURED TRIANGLE.
fn load_block_ci8_deep_triangle(
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
        (0xf500_0000 | (2 << 21) | (1 << 19) | (render_line_words << 9), 0),
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
fn generated_cases() -> Vec<GeneratedCase> {
    let mut cases = Vec::new();
    let mut push = |priority: u8, name: String, intent: &'static str, commands: Vec<(u32, u32)>| {
        cases.push(GeneratedCase { name, priority, intent, commands });
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
    for (color, label) in [(0xf801u16, "red"), (0x07c1, "green"), (0x003f, "blue"), (0x7fff, "white")] {
        push(
            5,
            format!("gen-fill-{label}-fullwidth-band"),
            "fill-cycle rectangle, full-width band",
            gen_fill_frame(color, 3, 0, 100, WIDTH, 140),
        );
    }
    // Single-pixel and last-pixel fills -- rasteriser boundary conditions.
    push(5, "gen-fill-single-pixel".into(), "fill single pixel", gen_fill_frame(0xf801, 3, 10, 10, 11, 11));
    push(5, "gen-fill-last-pixel".into(), "fill last pixel", gen_fill_frame(0x07c1, 3, WIDTH - 1, HEIGHT - 1, WIDTH, HEIGHT));

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
    for (op, label) in [(0x27u32, "pipesync"), (0x28, "tilesync"), (0x26, "loadsync")] {
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
    push(1, "gen-loadblock-linear".into(), "LoadBlock linear row advance (bilerp corrected)", set_bilerp0(load_block_textured_rect(8, 0, 2, 8, 1, 2)));
    push(1, "gen-loadblock-dxt".into(), "LoadBlock DxT row advance (bilerp corrected)", set_bilerp0(load_block_textured_rect(16, 0x400, 2, 8, 2, 4)));
    push(4, "gen-texrect-flip".into(), "TexRectFlip S/T swap (bilerp corrected)", set_bilerp0(one_textured_rect_flip()));
    push(2, "gen-textured-triangle".into(), "textured triangle (bilerp corrected)", set_bilerp0(one_textured_triangle()));
    // Witness: the SAME loadblock WITHOUT the bilerp correction. Expected to
    // reproduce the wgpu==RT64 vs angrylion divergence — documents the finding
    // as a live, reproducible corpus row.
    push(1, "gen-loadblock-linear-missing-bilerp".into(), "LoadBlock WITHOUT BI_LERP_0 (bilerp-gap witness)", load_block_textured_rect(8, 0, 2, 8, 1, 2));

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


    cases
}

/// The triage classification for one generated case, per the brief's rubric.
fn triage(
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
fn first_diff(a: &Result<Vec<u8>, String>, b: &Result<Vec<u8>, String>) -> Value {
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
fn diff_count(a: &Result<Vec<u8>, String>, b: &Result<Vec<u8>, String>) -> Value {
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let (a, b) = (pixels(a), pixels(b));
            json!((0..PIXEL_COUNT as usize).filter(|&i| a[i] != b[i]).count())
        }
        _ => Value::Null,
    }
}

/// Run the synthetic generator corpus, three-way comparing every case against
/// angrylion ground truth and classifying each per the triage rubric.
fn run_generated() -> Value {
    let mut cases = generated_cases();
    cases.sort_by_key(|c| (c.priority, c.name.clone()));
    // Optional substring filter for fast, targeted triage of a single slice
    // (e.g. FN64_ONLY=loadblock-deep). Absent, the whole corpus runs.
    if let Ok(filter) = std::env::var("FN64_ONLY") {
        if !filter.is_empty() {
            cases.retain(|c| c.name.contains(&filter));
        }
    }

    let mut rows = Vec::new();
    let mut counts: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();

    for case in &cases {
        let angrylion = angrylion_bytes(&case.commands);
        let wgpu = wgpu_outcome(&case.commands);
        let rt64 = rt64_bytes(&case.commands);

        let classification = triage(&angrylion, &wgpu, &rt64);
        *counts.entry(classification).or_default() += 1;

        rows.push(json!({
            "case": case.name,
            "priority": case.priority,
            "intent": case.intent,
            "command_words": case.commands.len(),
            "classification": classification,
            "angrylion": if angrylion_is_skipped(&angrylion) {
                json!("skipped")
            } else {
                outcome_wire(&angrylion)
            },
            "wgpu": outcome_wire(&wgpu),
            "rt64": outcome_wire(&rt64),
            "wgpu_vs_angrylion_diff_pixels": diff_count(&angrylion, &wgpu),
            "rt64_vs_angrylion_diff_pixels": diff_count(&angrylion, &rt64),
            "wgpu_vs_angrylion_first_diff": first_diff(&angrylion, &wgpu),
            "rt64_vs_angrylion_first_diff": first_diff(&angrylion, &rt64),
        }));
    }

    json!({
        "schema": "fn64.render-conformance.parity.generated.v1",
        "oracle": "angrylion-rdp-plus (bit-accurate hardware ground truth)",
        "candidates": ["fn64-render-wgpu", "fn64-render-rt64"],
        "target": { "width": WIDTH, "height": HEIGHT, "format": "rgba16" },
        "corpus_provenance": "hand-derived synthetic streams; NO case captured from a running ROM",
        "triage_legend": {
            "pass-all-match-hardware": "wgpu==angrylion and rt64==angrylion",
            "fn64-defect": "wgpu != angrylion, rt64 == angrylion (fn64 wrong)",
            "rt64-hle-defect": "wgpu == angrylion, rt64 != angrylion (RT64 HLE wrong)",
            "shared-ported-bug": "wgpu != angrylion, rt64 != angrylion, wgpu == rt64",
            "all-three-differ-inspect-construction": "all three differ; suspect stream",
            "angrylion-skipped-fallback-wgpu-vs-rt64": "oracle missing; only wgpu-vs-rt64 known",
        },
        "triage_counts": counts,
        "case_count": cases.len(),
        "rows": rows,
    })
}

fn run() -> Value {
    // Debug hatch: FN64_DUMP_CASE=<name> writes that case's seeded RDRAM image
    // to <name>.rdram.bin and its command words to stderr, then exits. Used to
    // reproduce a single case through the standalone oracle while iterating on
    // the angrylion byte domain. Never on in normal runs.
    if let Ok(target) = std::env::var("FN64_DUMP_CASE") {
        for case in cases() {
            if case.name == target {
                let rdram = seeded(&case.commands);
                let path = format!("/tmp/{}.rdram.bin", case.name);
                std::fs::write(&path, &rdram).unwrap();
                eprintln!("wrote {path}");
                eprintln!("cmd_start=0x{COMMAND_START:x} cmd_end=0x{:x}", command_end(&case.commands));
                for (i, (w0, w1)) in case.commands.iter().enumerate() {
                    eprintln!("  [{i:2}] {w0:#010x} {w1:#010x}  op={:#04x}", w0 >> 24);
                }
                return json!({"dumped": case.name});
            }
        }
        return json!({"error": "case not found"});
    }

    // Generator mode: FN64_GENERATE=1 emits the synthetic corpus and compares
    // wgpu and RT64 against ANGRYLION as ground truth. There is no hand key.
    if std::env::var("FN64_GENERATE").as_deref() == Ok("1") {
        return run_generated();
    }

    let mut rows = Vec::new();
    let mut authoritative = Tally::default();
    let mut non_authoritative = Tally::default();

    for case in cases() {
        let key = pixels(&key_bytes(&case));
        let rt64 = rt64_bytes(&case.commands);
        let wgpu = wgpu_outcome(&case.commands);
        let reference = reference_outcome(&case.commands);
        let angrylion = angrylion_bytes(&case.commands);

        // angrylion is bit-accurate ground truth. Classify each backend's
        // agreement with it (when it produced a reading) so the report carries
        // the truth partition directly, not only the wgpu-vs-RT64 differential.
        let agrees_with_angrylion = |outcome: &Result<Vec<u8>, String>| -> Value {
            match (&angrylion, outcome) {
                (Ok(truth), Ok(bytes)) => json!(pixels(truth) == pixels(bytes)),
                _ => Value::Null,
            }
        };
        let angrylion_matches_key = match &angrylion {
            Ok(bytes) => json!(pixels(bytes) == key),
            Err(_) => Value::Null,
        };

        let verdict = Verdict::of(&rt64, &wgpu);
        match case.authority {
            Authority::Rt64Authoritative => authoritative.record(verdict),
            Authority::CoverageDependentRt64NotAuthoritative => non_authoritative.record(verdict),
            // Counted with the other non-authoritative partition for the same
            // reason: the oracle is the lane that is not modelling the
            // command, so a difference is not evidence against wgpu.
            Authority::RawTrianglePlaneScaleDisagreement => non_authoritative.record(verdict),
        }

        let matches_key = |outcome: &Result<Vec<u8>, String>| match outcome {
            Ok(bytes) => json!(pixels(bytes) == key),
            Err(_) => json!(null),
        };

        // First differing pixel, for attribution. The whole 320x240 delta is
        // far too large to emit and a count plus one located example is what
        // a reader actually acts on.
        let first_difference = match (&rt64, &wgpu) {
            (Ok(rt64), Ok(wgpu)) => {
                let (rt64, wgpu) = (pixels(rt64), pixels(wgpu));
                (0..PIXEL_COUNT as usize)
                    .find(|&index| rt64[index] != wgpu[index])
                    .map(|index| {
                        json!({
                            "pixel": index,
                            "x": index as u32 % WIDTH,
                            "y": index as u32 / WIDTH,
                            "key": format!("{:#06x}", key[index]),
                            "rt64": format!("{:#06x}", rt64[index]),
                            "wgpu": format!("{:#06x}", wgpu[index]),
                        })
                    })
                    .unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };

        // Every differing pixel, capped. `first_difference` alone cannot
        // distinguish a wrong-texel fetch (a permutation of TEXTURE_TEXELS)
        // from a wrong-colour computation (a value not in the table at all),
        // because that needs the PATTERN across pixels, not one example.
        // Capped at 16 so a whole-target disagreement cannot flood the report.
        let differences = match (&rt64, &wgpu) {
            (Ok(rt64), Ok(wgpu)) => {
                let (rt64, wgpu) = (pixels(rt64), pixels(wgpu));
                let listed: Vec<Value> = (0..PIXEL_COUNT as usize)
                    .filter(|&index| rt64[index] != wgpu[index])
                    .take(16)
                    .map(|index| {
                        json!({
                            "x": index as u32 % WIDTH,
                            "y": index as u32 / WIDTH,
                            "key": format!("{:#06x}", key[index]),
                            "rt64": format!("{:#06x}", rt64[index]),
                            "wgpu": format!("{:#06x}", wgpu[index]),
                        })
                    })
                    .collect();
                json!(listed)
            }
            _ => Value::Null,
        };

        // The texel-sized window, every backend, every pixel. A wrong-texel
        // fetch and a wrong-colour computation are told apart by the PATTERN
        // over the whole window, which neither a first-difference nor a
        // differences-only list can show.
        let window = {
            let read = |outcome: &Result<Vec<u8>, String>| match outcome {
                Ok(bytes) => {
                    let got = pixels(bytes);
                    let mut out = Vec::new();
                    for y in 0..WIDE_HEIGHT {
                        for x in 0..WIDE_WIDTH {
                            out.push(format!("{:#06x}", got[(y * WIDTH + x) as usize]));
                        }
                    }
                    json!(out)
                }
                Err(_) => Value::Null,
            };
            let mut expected = Vec::new();
            for y in 0..WIDE_HEIGHT {
                for x in 0..WIDE_WIDTH {
                    expected.push(format!("{:#06x}", key[(y * WIDTH + x) as usize]));
                }
            }
            json!({
                "key": expected,
                "rt64": read(&rt64),
                "wgpu": read(&wgpu),
                "reference": read(&reference),
                "angrylion": read(&angrylion),
            })
        };

        rows.push(json!({
            "case": case.name,
            "window": window,
            "differences": differences,
            "intent": case.intent,
            "authority": case.authority.wire(),
            "verdict": verdict.wire(),
            "differing_pixels": match verdict {
                Verdict::Differs { pixels } => json!(pixels),
                Verdict::Identical => json!(0),
                _ => Value::Null,
            },
            "first_difference": first_difference,
            "rt64": outcome_wire(&rt64),
            "wgpu": outcome_wire(&wgpu),
            "reference": outcome_wire(&reference),
            "angrylion": if angrylion_is_skipped(&angrylion) {
                json!("skipped")
            } else {
                outcome_wire(&angrylion)
            },
            "rt64_matches_key": matches_key(&rt64),
            "wgpu_matches_key": matches_key(&wgpu),
            "reference_matches_key": matches_key(&reference),
            "angrylion_matches_key": angrylion_matches_key,
            "wgpu_agrees_with_angrylion": agrees_with_angrylion(&wgpu),
            "rt64_agrees_with_angrylion": agrees_with_angrylion(&rt64),
        }));
    }

    json!({
        "schema": "fn64.render-conformance.parity.v1",
        "oracle": "fn64-render-rt64",
        "candidate": "fn64-render-wgpu",
        "third_reading": "fn64-render-reference",
        "target": { "width": WIDTH, "height": HEIGHT, "format": "rgba16" },
        "corpus_provenance": "hand-authored; no case is captured from a running ROM",
        "parity": {
            "rt64_authoritative": authoritative.wire(),
            "rt64_not_authoritative_coverage": non_authoritative.wire(),
        },
        "captured_corpus": captured_row(),
        "note": "The two partitions are never summed. RT64 does not model \
                 coverage, AA or dither (docs/RT64-GUARD-AUDIT.md), so a \
                 difference in the second partition is evidence about RT64's \
                 modelling gap, not about wgpu.",
        "rows": rows,
    })
}

/// RT64's native side writes device diagnostics ("Device Name: Apple M5
/// Pro") straight to the process C stdout stream, which the C runtime flushes
/// at exit -- i.e. AFTER this runner has written its JSON. That appends
/// non-JSON lines to the document and makes the output unparseable, which is
/// exactly what happened on the first run of this binary.
///
/// The RT64 deferred-history runner solved this before us and this is its
/// approach, verbatim: duplicate the real stdout to a private descriptor to
/// write the protocol on, then point fd 1 at stderr so every native
/// diagnostic lands on the already-captured stderr pipe.
fn redirect_native_stdout() -> Result<File, Box<dyn std::error::Error>> {
    io::stdout().lock().flush()?;
    // SAFETY: flushing the process C stream before duplicating descriptors
    // prevents buffered native diagnostics from crossing the redirection.
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }
    // SAFETY: dup returns a new owned descriptor or -1. No Rust owner exists
    // until the success branch constructs exactly one File below.
    let protocol_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if protocol_fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: both descriptors are valid process descriptors. Native stdout
    // is sent to the already-captured stderr pipe before RT64 starts.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        let error = io::Error::last_os_error();
        // SAFETY: protocol_fd is the still-unowned successful dup above.
        unsafe {
            libc::close(protocol_fd);
        }
        return Err(error.into());
    }
    // SAFETY: protocol_fd is a unique owned descriptor after successful dup.
    Ok(unsafe { File::from_raw_fd(protocol_fd) })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut protocol = redirect_native_stdout()?;
    let value = run();
    serde_json::to_writer_pretty(&mut protocol, &value)?;
    writeln!(protocol)?;
    protocol.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
                    (word1 >> 12) & 0xfff >= (word0 >> 12) & 0xfff
                        && word1 & 0xfff >= word0 & 0xfff,
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
                .filter_map(|(index, (left, right))| {
                    (left != right).then_some((index, *left, *right))
                })
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
    /// times (docs/RT64-WM2000-HARNESS-TRAPS.md).
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
}
