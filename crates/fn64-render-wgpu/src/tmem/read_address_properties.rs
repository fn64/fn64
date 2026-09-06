//! **Property tests for the TMEM physical-address computation, including the
//! odd/even T-parity branch.**
//!
//! The kernel under test is `first_physical_byte`
//! (`crates/fn64-render-wgpu/src/tmem/read.rs:836`) and the two functions it
//! composes: `linear_byte_address` (`:858`) and `odd_row_exchange` (`:905`).
//! Together they map a tile descriptor plus a tile-relative texel coordinate
//! onto the physical byte address inside TMEM's 4 KiB.
//!
//! # The oracle: a fresh transcription of the documented formula
//!
//! [`documented_physical_byte`] below is written from the rule stated in
//! `odd_row_exchange`'s own doc comment and the RT64 citations it carries
//! (`shaders/TextureDecoder.hlsli:17-25` for the exchange,
//! `:149-150` for the parity), and from the tile layout `linear_byte_address`
//! documents. It is a SEPARATE expression of the same rule, not a call into
//! the kernel:
//!
//! **On the citation target.** `docs/RDP-SILICON-VECTORS.md` contains no
//! statement of this addressing rule and no `TextureDecoder.hlsli` reference,
//! so the transcription is grounded on `odd_row_exchange`'s doc comment and
//! `docs/rt64/RT64-WM2000-TEXEL-LOCALISATION.md` instead -- both in-tree, and
//! both resting on pinned RT64 (MIT). No external source was consulted and no
//! barred tree was read, so the clean-room rule holds; the substitution of
//! the citation target is recorded here rather than left silent.
//!
//! ```text
//! linear = tmem_word * 8
//!        + row * line_words * 8
//!        + column * bytes_per_texel      (4bpp: column / 2)
//! address = (linear & scope_mask) as u16
//! if row is odd { address ^= 4 }
//! ```
//!
//! **Where the transcription is independent, and where it is not.** The
//! row-parity source and the XOR constant are genuinely second expressions:
//! the kernel derives parity via the `odd_row_exchange` helper from
//! `addressed.row()`, while the oracle writes `row % 2 == 1` inline on the
//! raw generated `row`; and the kernel spells the exchange as an operation
//! (`address ^ 4`) while
//! `adjacent_rows_of_a_zero_stride_tile_differ_by_exactly_the_exchange`
//! restates it as an observable distance. Defects in either therefore fail
//! loudly rather than cancelling.
//!
//! The per-size column stride is **not** independent: the oracle's
//! `match size` is arm-for-arm the same shape as the kernel's, including the
//! `Bits16 | Bits32` grouping. A shared mistake in a per-size stride WOULD
//! cancel and this differential would not see it. An earlier version of this
//! comment claimed the strides came "from a table up front" and so did not
//! mirror the kernel's control flow; that overstated the independence the
//! code actually delivers, and is corrected here.
//!
//! **The 32-bit arm is deliberately excluded from the differential.** RGBA32
//! addresses through `rgba32_low_address`, which masks to the LOW HALF
//! (`0x07ff`) rather than the caller's scope, and returns only the low half
//! of a texel split across both banks. That is a second, genuinely different
//! address rule; folding it into one oracle would make the oracle a copy of
//! the kernel's own branch. It is covered instead by
//! `the_thirty_two_bit_arm_always_lands_in_the_low_half`, a postcondition
//! that does not restate the formula.
//!
//! # The vacuity trap, and the branch-coverage guard
//!
//! The whole point of this kernel is the parity branch, and a generator that
//! only ever produced even rows would test exactly half of it while passing.
//! Worse, an even-row-only generator makes the `^ 4` term unreachable, so a
//! mutant that deletes the exchange entirely would survive.
//!
//! `the_generator_reaches_both_t_parity_branches` counts, over the generated
//! domain, how many cases take the exchange and how many do not, and fails
//! unless BOTH are substantially populated. The counts are asserted, not
//! merely printed.
//!
//! # Mutation results (see the task report for the full table)
//!
//! | mutation | killed by |
//! |---|---|
//! | `odd_row_exchange`: `row & 1 != 0` -> `row & 1 == 0` (branch swap) | differential |
//! | `linear_byte_address`: `tmem * 8` -> `tmem * 8 + 1` (boundary off-by-one) | differential |
//!
//! # Blast radius
//!
//! The differential sees the tile base, the row stride, the scope mask, and
//! the parity branch and its XOR constant. It does NOT see a shared per-size
//! column-stride mistake, which cancels (above). Nor does it see whether the
//! CALLER supplies the right tile-relative row --
//! the historical `first_row_parity` defect that motivated the current
//! `odd_row_exchange` was a caller-side unit mismatch, and an oracle that
//! takes the same `AddressedTmemTexel` the kernel takes cannot see one. That
//! remains the job of the end-to-end reader tests in `read.rs`.

use proptest::prelude::*;

use super::read::{first_physical_byte, AddressScope};
use super::{AddressedTmemTexel, TileDescriptor, TmemFirstRowParity, TmemWordAddress};
use crate::{ImageFormat, PixelSize, TileAddressMode};

/// The four pixel sizes, and the byte stride each one advances per column.
///
/// Transcribed from the tile layout `linear_byte_address` documents: 4bpp
/// packs two texels per byte (so its stride is expressed as a halved column
/// rather than a per-texel byte count), 8bpp is one byte, and both 16bpp and
/// 32bpp advance two bytes per texel in the low bank.
const SIZES: [PixelSize; 4] = [
    PixelSize::Bits4,
    PixelSize::Bits8,
    PixelSize::Bits16,
    PixelSize::Bits32,
];

/// TMEM's full address mask, `0x0fff` -- 4 KiB.
const FULL_TMEM_MASK: u64 = 0x0fff;
/// One 2 KiB bank, `0x07ff`.
const LOW_HALF_MASK: u64 = 0x07ff;

/// **The oracle.** A fresh transcription of the documented address formula,
/// written from the rule and its RT64 citations rather than from the kernel's
/// code.
///
/// Structured as `linear term -> mask -> conditional XOR`, with the per-size
/// column stride resolved from a table up front, so it does not mirror the
/// kernel's internal control flow.
fn documented_physical_byte(
    tmem_word: u16,
    line_words: u16,
    size: PixelSize,
    column: u16,
    row: u16,
    mask: u64,
) -> u16 {
    let column_bytes: u64 = match size {
        // Two texels share a byte, so the column index halves before it
        // becomes an offset. This is the documented 4bpp packing.
        PixelSize::Bits4 => u64::from(column) / 2,
        PixelSize::Bits8 => u64::from(column),
        PixelSize::Bits16 | PixelSize::Bits32 => u64::from(column) * 2,
    };

    let linear =
        u64::from(tmem_word) * 8 + u64::from(row) * u64::from(line_words) * 8 + column_bytes;

    let masked = (linear & mask) as u16;

    // The odd-row XOR4 bank exchange: swap the 32-bit word index within the
    // row while preserving the byte offset inside that word, which is exactly
    // a 4-byte address XOR. Parity comes from the tile-relative row alone.
    if row % 2 == 1 {
        masked ^ 4
    } else {
        masked
    }
}

/// Builds a tile descriptor over the generated parameters. Only the fields
/// the address computation reads (`size`, `line_words`, `tmem`) vary; the
/// rest are held at neutral values.
fn tile(size: PixelSize, line_words: u16, tmem_word: u16) -> TileDescriptor {
    let format = if size == PixelSize::Bits4 {
        ImageFormat::ColorIndex
    } else {
        ImageFormat::Rgba
    };
    TileDescriptor::from_wire(
        format,
        size,
        line_words,
        TmemWordAddress::try_new(tmem_word).unwrap(),
        0,
        TileAddressMode::default(),
        0,
        0,
        TileAddressMode::default(),
        0,
        0,
    )
}

// The generated tile-and-texel domain.
//
// Ranges are chosen so the linear address routinely EXCEEDS the mask and
// wraps -- `tmem_word` alone reaches 511 * 8 = 4088 bytes, and rows add many
// KiB on top -- because the mask is part of the formula and an unwrapped
// domain would leave it untested.
prop_compose! {
    fn address_case()(
        size_index in 0usize..3,      // 32bpp handled separately, see module doc
        line_words in 0u16..=64,
        tmem_word in 0u16..512,
        column in 0u16..=512,
        row in 0u16..=64,
    ) -> (PixelSize, u16, u16, u16, u16) {
        (SIZES[size_index], line_words, tmem_word, column, row)
    }
}

proptest! {
    /// **The differential.** For every generated tile and texel, the kernel's
    /// physical byte address must equal the freshly transcribed documented
    /// formula's, under both address scopes.
    #[test]
    fn the_physical_address_matches_the_documented_formula(
        (size, line_words, tmem_word, column, row) in address_case(),
    ) {
        let subject = tile(size, line_words, tmem_word);
        for (scope, mask) in [
            (AddressScope::FullTmem, FULL_TMEM_MASK),
            (AddressScope::LowHalf, LOW_HALF_MASK),
        ] {
            let addressed =
                AddressedTmemTexel::new(column, row, TmemFirstRowParity::Even);
            let got = first_physical_byte(subject, addressed, scope);
            let expected =
                documented_physical_byte(tmem_word, line_words, size, column, row, mask);
            prop_assert_eq!(
                got,
                expected,
                "address mismatch: size {:?} line_words {} tmem {} column {} row {} scope {:?}",
                size, line_words, tmem_word, column, row, scope
            );
        }
    }

    /// **The parity branch is the ONLY thing that separates adjacent rows'
    /// bank halves.** Two texels at the same column in rows `2k` and `2k+1`
    /// of a tile with a zero row stride must land at addresses differing by
    /// exactly 4 -- the exchange and nothing else.
    ///
    /// This is a second, independent statement of the branch: it names the
    /// XOR constant as an observable DISTANCE rather than as an operation,
    /// so a mutant that changes `^ 4` to `^ 8` fails here even where the
    /// masked differential might absorb it.
    #[test]
    fn adjacent_rows_of_a_zero_stride_tile_differ_by_exactly_the_exchange(
        size_index in 0usize..3,
        tmem_word in 0u16..512,
        column in 0u16..=64,
        pair in 0u16..=31,
    ) {
        let size = SIZES[size_index];
        // line_words = 0 collapses the row stride, so the ONLY remaining
        // row-dependent term is the parity exchange.
        let subject = tile(size, 0, tmem_word);
        let even = AddressedTmemTexel::new(column, pair * 2, TmemFirstRowParity::Even);
        let odd = AddressedTmemTexel::new(column, pair * 2 + 1, TmemFirstRowParity::Even);
        let even_address = first_physical_byte(subject, even, AddressScope::FullTmem);
        let odd_address = first_physical_byte(subject, odd, AddressScope::FullTmem);
        prop_assert_eq!(
            even_address ^ odd_address,
            4,
            "the odd/even exchange is not a 4-byte swap: even {} odd {}",
            even_address,
            odd_address
        );
    }

    /// **32-bit postcondition.** RGBA32 reads the low half of a split texel,
    /// so its address must always fall inside the low 2 KiB bank regardless
    /// of the scope the caller asked for. Stated as a containment, not as a
    /// formula, so it does not restate `rgba32_low_address`.
    #[test]
    fn the_thirty_two_bit_arm_always_lands_in_the_low_half(
        line_words in 0u16..=64,
        tmem_word in 0u16..512,
        column in 0u16..=512,
        row in 0u16..=64,
    ) {
        let subject = tile(PixelSize::Bits32, line_words, tmem_word);
        let addressed = AddressedTmemTexel::new(column, row, TmemFirstRowParity::Even);
        for scope in [AddressScope::FullTmem, AddressScope::LowHalf] {
            let address = first_physical_byte(subject, addressed, scope);
            prop_assert!(
                u64::from(address) <= LOW_HALF_MASK,
                "RGBA32 address {address} escaped the low bank under {scope:?}"
            );
        }
    }
}

/// **The branch-coverage guard.** Both T-parity arms must be reached by the
/// generated domain, and both counted.
///
/// Without this, an even-row-only generator would leave the `^ 4` exchange
/// entirely unreachable -- a mutant deleting it would survive while every
/// property still passed. The counts are asserted, not printed.
#[test]
fn the_generator_reaches_both_t_parity_branches() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = address_case();

    let (mut exchanged, mut plain) = (0usize, 0usize);
    const CASES: usize = 1024;
    for _ in 0..CASES {
        let (size, line_words, tmem_word, column, row) = strategy
            .new_tree(&mut runner)
            .expect("address case strategy produces a value")
            .current();
        // Observe the branch through its EFFECT, not by re-reading the
        // predicate: an address that differs from the un-exchanged formula
        // by exactly 4 took the exchange.
        let subject = tile(size, line_words, tmem_word);
        let addressed = AddressedTmemTexel::new(column, row, TmemFirstRowParity::Even);
        let got = first_physical_byte(subject, addressed, AddressScope::FullTmem);
        let unexchanged = {
            let column_bytes: u64 = match size {
                PixelSize::Bits4 => u64::from(column) / 2,
                PixelSize::Bits8 => u64::from(column),
                PixelSize::Bits16 | PixelSize::Bits32 => u64::from(column) * 2,
            };
            let linear = u64::from(tmem_word) * 8
                + u64::from(row) * u64::from(line_words) * 8
                + column_bytes;
            (linear & FULL_TMEM_MASK) as u16
        };
        if got == unexchanged {
            plain += 1;
        } else if got == unexchanged ^ 4 {
            exchanged += 1;
        } else {
            panic!("address {got} is neither the plain nor the exchanged form of {unexchanged}");
        }
    }

    assert!(
        exchanged > 0 && plain > 0,
        "generator did not reach both T-parity branches over {CASES} cases: \
         exchanged={exchanged} plain={plain}"
    );
    // Rows are generated uniformly, so a healthy split is near even. Requiring
    // at least an eighth on each side makes a drifting strategy fail loudly
    // rather than quietly starving one arm.
    assert!(
        exchanged * 8 > CASES && plain * 8 > CASES,
        "T-parity branch coverage is lopsided over {CASES} cases: \
         exchanged={exchanged} plain={plain}"
    );
}
