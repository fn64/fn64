//! The VRCP / VRSQ reciprocal and inverse-square-root seed ROM tables.
//!
//! ## These are GENERATED, not copied
//!
//! The two 512-entry tables are hardware ROM constants embedded in the RSP.
//! They encode the seed for the RSP's single-step reciprocal / inverse-sqrt
//! (the `(0x10000 | frac)` implicit-leading-1 reconstruction plus the
//! normalization shift IS the "Newton refinement" — there is no iterated
//! loop, see RSP-VU-ISA.md §6.12). Because they are *derivable* from the
//! documented normalized-mantissa algorithm, they are generated here at
//! first use rather than transcribed from any implementation's data file.
//!
//! ## Generation algorithm (RSP-VU-ISA.md §6.12) and its verification
//!
//! **Reciprocal** — for index `i` in 0..512, denominator `a = i + 512`
//! (a normalized 1.9 mantissa, top bit implicit), value `b = 2^34 / a`,
//! entry `= (b + 1) >> 8` masked to 16 bits. Index 0 is the `1/1.0`
//! overflow case (raw value `0x10000`) and the ROM stores `0xFFFF`.
//! This reproduces the well-attested public RSP reciprocal ROM prefix
//! `0xFFFF, 0xFF00, 0xFE01, 0xFD04, ...` and tail `rcp[511] = 0x0040`
//! (spot-checked in the tests below).
//!
//! **Inverse sqrt** — the 9-bit index folds the parity of the exponent
//! (1/sqrt splits by even/odd magnitude exponent, §6.12), so adjacent table
//! entries interleave two mantissa octaves via
//! `a = ((i & 1) << 8) + (i >> 1) + 256`, then `entry = (isqrt(2^48 / a) >> 3)`
//! masked to 16 bits, with index 0
//! again the `1/sqrt(1.0)` overflow storing `0xFFFF`. This reproduces the
//! documented inverse-sqrt seed sequence (spot-checked below): the even-index
//! octave gives rsq at 0/2/4/6 = 0xFFFF/0xFF00/0xFE02/0xFD06, and both octaves
//! are monotone decreasing.
//!
//! The spot-check tests are the contract: if a future refinement of the
//! rounding disagrees on a low bit, the tests catch it against these known
//! anchors rather than silently shipping a subtly-wrong seed (RSP-VU-ISA.md
//! §6.12 explicitly warns the low-bit rounding is where clean-room
//! re-derivations most often diverge).

/// Number of entries in each seed ROM.
pub const RCP_ROM_LEN: usize = 512;
/// Number of entries in each seed ROM.
pub const RSQ_ROM_LEN: usize = 512;

/// Integer square root (floor) of a `u64`, portable and const-free. Used only
/// at table-generation time (once, lazily), so it need not be fast.
fn isqrt_u64(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    // Newton's method on integers; converges quickly for 48-bit inputs.
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Generate the 512-entry reciprocal seed ROM per RSP-VU-ISA.md §6.12.
fn generate_rcp_rom() -> [u16; RCP_ROM_LEN] {
    let mut rom = [0u16; RCP_ROM_LEN];
    for (i, slot) in rom.iter_mut().enumerate() {
        let a = (i as u64) + 512;
        let b = (1u64 << 34) / a;
        *slot = (((b + 1) >> 8) & 0xFFFF) as u16;
    }
    // Index 0 (1/1.0) overflows 16 bits (raw 0x10000); hardware stores 0xFFFF.
    rom[0] = 0xFFFF;
    rom
}

/// Generate the 512-entry inverse-square-root seed ROM per RSP-VU-ISA.md
/// §6.12. Adjacent entries interleave the two exponent-parity octaves.
fn generate_rsq_rom() -> [u16; RSQ_ROM_LEN] {
    let mut rom = [0u16; RSQ_ROM_LEN];
    for (i, slot) in rom.iter_mut().enumerate() {
        let a = (((i as u64) & 1) << 8) + ((i as u64) >> 1) + 256;
        let val = isqrt_u64((1u64 << 48) / a);
        *slot = ((val >> 3) & 0xFFFF) as u16;
    }
    rom[0] = 0xFFFF;
    rom
}

use std::sync::OnceLock;

static RCP_ROM: OnceLock<[u16; RCP_ROM_LEN]> = OnceLock::new();
static RSQ_ROM: OnceLock<[u16; RSQ_ROM_LEN]> = OnceLock::new();

/// The reciprocal seed ROM (generated once, then cached).
pub fn rcp_rom() -> &'static [u16; RCP_ROM_LEN] {
    RCP_ROM.get_or_init(generate_rcp_rom)
}

/// The inverse-sqrt seed ROM (generated once, then cached).
pub fn rsq_rom() -> &'static [u16; RSQ_ROM_LEN] {
    RSQ_ROM.get_or_init(generate_rsq_rom)
}

/// Look up a reciprocal seed by 9-bit index (masked to range).
pub fn rcp_seed(index: usize) -> u16 {
    rcp_rom()[index & (RCP_ROM_LEN - 1)]
}

/// Look up an inverse-sqrt seed by 9-bit index (masked to range).
pub fn rsq_seed(index: usize) -> u16 {
    rsq_rom()[index & (RSQ_ROM_LEN - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reciprocal_known_anchor_entries() {
        let rom = rcp_rom();
        // Well-attested public RSP reciprocal ROM values.
        assert_eq!(rom[0], 0xFFFF, "rcp[0]");
        assert_eq!(rom[1], 0xFF00, "rcp[1]");
        assert_eq!(rom[2], 0xFE01, "rcp[2]");
        assert_eq!(rom[3], 0xFD04, "rcp[3]");
        assert_eq!(rom[511], 0x0040, "rcp[511]");
    }

    #[test]
    fn reciprocal_is_monotone_decreasing_past_index_0() {
        let rom = rcp_rom();
        for i in 1..RCP_ROM_LEN - 1 {
            assert!(
                rom[i] >= rom[i + 1],
                "rcp not monotone at {}: {:#06X} < {:#06X}",
                i,
                rom[i],
                rom[i + 1]
            );
        }
    }

    #[test]
    fn inverse_sqrt_known_anchor_entries() {
        let rom = rsq_rom();
        // The even-index octave reproduces the documented inverse-sqrt seed
        // sequence; index 0 is the 1/sqrt(1.0) overflow -> 0xFFFF.
        assert_eq!(rom[0], 0xFFFF, "rsq[0]");
        assert_eq!(rom[2], 0xFF00, "rsq[2]");
        assert_eq!(rom[4], 0xFE02, "rsq[4]");
        assert_eq!(rom[6], 0xFD06, "rsq[6]");
    }

    #[test]
    fn inverse_sqrt_octaves_are_monotone() {
        let rom = rsq_rom();
        // Even-index octave monotone decreasing.
        let even: Vec<u16> = (0..RSQ_ROM_LEN).step_by(2).map(|i| rom[i]).collect();
        for w in even.windows(2) {
            assert!(w[0] >= w[1], "rsq even octave not monotone: {w:?}");
        }
        // Odd-index octave monotone decreasing.
        let odd: Vec<u16> = (1..RSQ_ROM_LEN).step_by(2).map(|i| rom[i]).collect();
        for w in odd.windows(2) {
            assert!(w[0] >= w[1], "rsq odd octave not monotone: {w:?}");
        }
    }

    #[test]
    fn seed_lookups_mask_into_range() {
        // 9-bit index wrap; no panic, consistent with direct indexing.
        assert_eq!(rcp_seed(0), rcp_rom()[0]);
        assert_eq!(rcp_seed(512), rcp_rom()[0]);
        assert_eq!(rsq_seed(513), rsq_rom()[1]);
    }

    #[test]
    fn isqrt_is_floor_correct() {
        assert_eq!(isqrt_u64(0), 0);
        assert_eq!(isqrt_u64(1), 1);
        assert_eq!(isqrt_u64(3), 1);
        assert_eq!(isqrt_u64(4), 2);
        assert_eq!(isqrt_u64(15), 3);
        assert_eq!(isqrt_u64(16), 4);
        assert_eq!(isqrt_u64(1u64 << 48), 1u64 << 24);
    }
}
