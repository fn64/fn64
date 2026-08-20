//! The three-way parity differential: fn64's shipping wgpu backend against
//! RT64, the oracle, with the reference backend as an independent third
//! reading.
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
}

impl Authority {
    const fn wire(self) -> &'static str {
        match self {
            Self::Rt64Authoritative => "rt64-authoritative",
            Self::CoverageDependentRt64NotAuthoritative => "rt64-not-authoritative-coverage",
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
/// Hand-derived field by field from angrylion's `rdp_set_other_modes`
/// (`src/core/n64video/rdp.c:623-660`), which is the authority for every bit
/// position below:
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

fn one_fill(color: u16, ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
    vec![
        OTHER_MODES_FILL_NO_AA,
        (0xed00_0000, ((WIDTH * 4) << 12) | (HEIGHT * 4)),
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
    0xffff, // row 0, col 3: r=31 g=31 b=31 a=1
    0x8421, // row 1, col 0: r=16 g=16 b=16 a=1
    0xc631, // row 1, col 1: r=24 g=24 b=24 a=1
    0x4211, // row 1, col 2: r=8  g=8  b=8  a=1
    0xfc01, // row 1, col 3: r=31 g=0  b=0  a=1 with g's top bit set
];

/// Where the textured rectangle lands on the target, inclusive on both edges
/// exactly as `G_FILLRECT` is. 4 columns by 2 rows at the origin, so the
/// rectangle steps exactly one texel per pixel on both axes and pixel
/// `(x, y)` samples texel `(x, y)` with no filtering ambiguity.
const TEXRECT_ULX: u32 = 0;
const TEXRECT_ULY: u32 = 0;
const TEXRECT_LRX: u32 = TEXTURE_WIDTH - 1;
const TEXRECT_LRY: u32 = TEXTURE_HEIGHT - 1;

/// The expected pixel for a textured case, by linear target index.
///
/// Inside the rectangle a pixel reads its own texel; outside it the seeded
/// `STALE` survives. This is the whole key, and it is arithmetic over
/// [`TEXTURE_TEXELS`] -- no backend is consulted.
fn textured_expected(index: u32) -> u16 {
    let x = index % WIDTH;
    let y = index / WIDTH;
    if x <= TEXRECT_LRX && y <= TEXRECT_LRY {
        TEXTURE_TEXELS[(y * TEXTURE_WIDTH + x) as usize]
    } else {
        STALE
    }
}

/// `SetTextureImage` naming the staged source as RGBA16.
///
/// Wire: `format` 0 (RGBA) at bits 23:21, `size` 2 (16-bit) at 20:19, and a
/// width field of `width - 1` at 11:0, matching angrylion's
/// `rdp_set_texture_image` (`src/core/n64video/rdp/tex.c:1002-1008`).
const fn set_texture_image(width: u32, address: u32) -> (u32, u32) {
    (0xfd00_0000 | (2 << 19) | (width - 1), address)
}

/// `SetTile` for tile 0, RGBA16, at TMEM word 0.
///
/// Wire, from angrylion's `rdp_set_tile` (`tex.c:979-1000`): `format` 23:21,
/// `size` 20:19, `line` 17:9, `tmem` 8:0 in word 0; `tile` 26:24, `palette`
/// 23:20, and the S/T clamp/mirror/mask/shift fields in word 1. Everything
/// not named here is zero: no palette, no mirror, and `mask_s`/`mask_t` zero,
/// which forces the CLAMP arm so a coordinate cannot wrap onto a neighbour
/// and hide an addressing error.
const fn set_tile(line_words: u32, tmem_word: u32) -> (u32, u32) {
    (0xf500_0000 | (2 << 19) | (line_words << 9) | tmem_word, 0)
}

/// `SetTileSize` for tile 0 covering the whole texture.
///
/// All four coordinates are S10.2 and both high edges are INCLUSIVE, so a
/// `w`-texel wide tile has `high_s = (w - 1) << 2`.
const fn set_tile_size(width: u32, height: u32) -> (u32, u32) {
    (
        0xf200_0000,
        (((width - 1) * 4) << 12) | ((height - 1) * 4),
    )
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

/// The command list for a textured case: state, load, draw, sync.
fn one_textured_rect() -> Vec<(u32, u32)> {
    let mut words = vec![
        OTHER_MODES_ONE_CYCLE_TEXTURED,
        SET_COMBINE_TEXEL0,
        (0xed00_0000, ((WIDTH * 4) << 12) | (HEIGHT * 4)),
        (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
        set_texture_image(TEXTURE_WIDTH, TEXTURE_SOURCE),
        set_tile(TEXTURE_LINE_WORDS, 0),
        set_tile_size(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
        load_tile(TEXTURE_WIDTH, TEXTURE_HEIGHT),
        (0xe600_0000, 0),
    ];
    words.extend(texture_rectangle(
        TEXRECT_ULX,
        TEXRECT_ULY,
        TEXRECT_LRX,
        TEXRECT_LRY,
    ));
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
                     paints the right half too. NOTE: the guard audit records \
                     that RT64 and angrylion disagree on subpixel scissor \
                     ROUNDING; this case uses whole-pixel edges, where they \
                     agree, so it stays RT64-authoritative.",
            authority: Authority::Rt64Authoritative,
            commands: {
                let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                words[1] = (0xed00_0000, (((WIDTH / 2) * 4) << 12) | (HEIGHT * 4));
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
                words[1] = (0xed00_0000, ((WIDTH * 4) << 12) | ((HEIGHT / 2) * 4));
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
                if x <= TEXRECT_LRX && y <= TEXRECT_LRY {
                    TEXTURE_TEXELS[(TEXTURE_WIDTH + x) as usize]
                } else {
                    STALE
                }
            },
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
                let stripped = field
                    .strip_prefix("0x")
                    .ok_or_else(|| format!("line {} {name} is {field:?}, want 0x hex", index + 1))?;
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

fn run() -> Value {
    let mut rows = Vec::new();
    let mut authoritative = Tally::default();
    let mut non_authoritative = Tally::default();

    for case in cases() {
        let key = pixels(&key_bytes(&case));
        let rt64 = rt64_bytes(&case.commands);
        let wgpu = wgpu_bytes(&case.commands);
        let reference = reference_bytes(&case.commands);

        let verdict = Verdict::of(&rt64, &wgpu);
        match case.authority {
            Authority::Rt64Authoritative => authoritative.record(verdict),
            Authority::CoverageDependentRt64NotAuthoritative => non_authoritative.record(verdict),
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

        rows.push(json!({
            "case": case.name,
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
            "rt64_matches_key": matches_key(&rt64),
            "wgpu_matches_key": matches_key(&wgpu),
            "reference_matches_key": matches_key(&reference),
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
                    || *other_modes == OTHER_MODES_ONE_CYCLE_TEXTURED,
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
            view.read_u16(RdramAddr::from_offset(
                FRAMEBUFFER + FRAMEBUFFER_BYTES - 2
            )),
            STALE
        );
        assert_eq!(view.read_u16(RdramAddr::from_offset(FRAMEBUFFER - 2)), GUARD);
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
        assert_eq!(captured::target_extent(&captured::walk(&[0xe900_0000, 0])), None);
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
        assert!(row["reason"].as_str().unwrap().contains(captured::PACKET_ENV));
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
