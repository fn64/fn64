//! The RSP Vector Unit register/accumulator/flag model, element-select, and
//! clamp helpers — the FOUNDATION every VU op builds on.
//!
//! This module is the stable interface the ops-phase agents call. It owns:
//! - the 32×8-lane `i16` vector register file (`VuRegs` / `VuState.regs`),
//! - the per-lane 48-bit signed accumulator (`Accumulator`, one `i64` per
//!   lane keeping the low 48 bits meaningful),
//! - the `VCO` / `VCC` / `VCE` flag registers (`Flags`),
//! - the reciprocal/inverse-sqrt div latch (`div_in` / `div_out`) VRCP*/VRSQ*
//!   thread through,
//! - `element_select` (the `e` broadcast/rotate shuffle), and
//! - the three clamp modes (`clamp_signed`, `clamp_unsigned`,
//!   `clamp_unsigned_low`) and accumulator slice read/write helpers.
//!
//! See `RSP-VU-ISA.md` §1–§5 for the behavioral spec these implement. All of
//! it is portable scalar Rust — `[i16; 8]` lanes, `i32`/`i64` intermediates,
//! no SIMD.

/// One RSP vector register: 8 lanes of signed 16-bit, lane 0 = most-
/// significant halfword (big-endian lane order, RSP-VU-ISA.md §1).
pub type Vec8 = [i16; 8];

/// Number of vector registers (V0..V31).
pub const NUM_VREGS: usize = 32;
/// Lanes per vector register.
pub const LANES: usize = 8;

/// The 32-entry vector register file. Ops read `regs.r[vs]` / `regs.r[vt]`
/// and write `regs.r[vd]`, matching the RSPRecomp generated call shape
/// `rsp.VOP<e>(rsp.vpu.r[vd], rsp.vpu.r[vs], rsp.vpu.r[vt])`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VuRegs {
    /// `r[0..32]`, each an 8-lane `i16` vector. Public + named `r` so the
    /// generated-call analogue `vpu.r[n]` reads naturally at the op layer.
    pub r: [Vec8; NUM_VREGS],
}

impl Default for VuRegs {
    fn default() -> Self {
        VuRegs {
            r: [[0i16; LANES]; NUM_VREGS],
        }
    }
}

/// The 48-bit signed accumulator: one independent 48-bit lane per vector lane,
/// modeled as an `i64` keeping only the low 48 bits meaningful (RSP-VU-ISA.md
/// §2). Slices HI (bits 47..32), MD (bits 31..16), LO (bits 15..0) are read
/// via `read_hi`/`read_mid`/`read_lo` (each returns the raw 16-bit slice) and
/// the whole signed value via `signed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Accumulator {
    /// Per-lane accumulator, low 48 bits meaningful, sign-extended when read
    /// as a signed value. Stored as `i64` so add/accumulate wrap naturally in
    /// two's complement; callers must mask/sign-extend to 48 bits on read.
    lanes: [i64; LANES],
}

/// Sign-extend the low 48 bits of an `i64` to a full signed `i64`.
#[inline]
fn sign_extend_48(v: i64) -> i64 {
    // Shift the 48-bit value up to the top of the i64 and arithmetic-shift
    // back, so bit 47 becomes the sign.
    (v << 16) >> 16
}

impl Accumulator {
    /// The signed 48-bit value of a lane (sign-extended from bit 47).
    #[inline]
    pub fn signed(&self, lane: usize) -> i64 {
        sign_extend_48(self.lanes[lane])
    }

    /// Overwrite a lane's full 48-bit accumulator (used by `VMULx`/`VMUDx`
    /// which SET the accumulator). Only the low 48 bits are retained.
    #[inline]
    pub fn set(&mut self, lane: usize, value: i64) {
        self.lanes[lane] = value & 0xFFFF_FFFF_FFFF;
    }

    /// Add into a lane's 48-bit accumulator (used by `VMACx`/`VMADx` which
    /// ACCUMULATE). Wraps within 48 bits.
    #[inline]
    pub fn add(&mut self, lane: usize, delta: i64) {
        self.lanes[lane] = (self.lanes[lane].wrapping_add(delta)) & 0xFFFF_FFFF_FFFF;
    }

    /// The LO slice (bits 15..0) of a lane, as a raw 16-bit value.
    #[inline]
    pub fn read_lo(&self, lane: usize) -> u16 {
        (self.lanes[lane] & 0xFFFF) as u16
    }

    /// The MD slice (bits 31..16) of a lane, as a raw 16-bit value.
    #[inline]
    pub fn read_mid(&self, lane: usize) -> u16 {
        ((self.lanes[lane] >> 16) & 0xFFFF) as u16
    }

    /// The HI slice (bits 47..32) of a lane, as a raw 16-bit value.
    #[inline]
    pub fn read_hi(&self, lane: usize) -> u16 {
        ((self.lanes[lane] >> 32) & 0xFFFF) as u16
    }

    /// Write the LO slice of a lane, leaving MD/HI untouched. Used by the
    /// ALU/compare ops that write `acc_lo` as a side effect (RSP-VU-ISA.md
    /// §2, §6.3–§6.6).
    #[inline]
    pub fn write_lo(&mut self, lane: usize, value: u16) {
        let hi_mid = self.lanes[lane] & !0xFFFF;
        self.lanes[lane] = hi_mid | (value as i64);
    }

    /// The signed 32-bit value `acc[47..16]` (the top 32 bits of the lane,
    /// i.e. HI:MD after dropping LO), used by the unsigned clamps
    /// (RSP-VU-ISA.md §4 mode 2, §6.2 VMADN/VMADL). Sign-extended from bit 47.
    #[inline]
    pub fn read_hi_mid_signed(&self, lane: usize) -> i32 {
        (self.signed(lane) >> 16) as i32
    }
}

/// The three RSP flag registers: VCO (carry+ne), VCC (compare/clip lo+hi),
/// VCE (compare extension). See RSP-VU-ISA.md §3. Each is a small bitfield,
/// one bit per lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Flags {
    /// VCO: low byte = carry/borrow (one bit per lane), high byte = "not
    /// equal" (ne). Represented as `u16`, low byte carry, high byte ne.
    pub vco: u16,
    /// VCC: low byte = primary compare/clip-low, high byte = secondary
    /// compare/clip-high (one bit per lane each).
    pub vcc: u16,
    /// VCE: 8 bits, one per lane, the compare-extension bit set by VCH and
    /// consumed by VCL.
    pub vce: u8,
}

/// Read/write helpers for a single lane's bit in a byte-packed flag field.
#[inline]
fn get_bit(field: u16, lane: usize) -> bool {
    (field >> lane) & 1 != 0
}
#[inline]
fn set_bit(field: &mut u16, lane: usize, value: bool) {
    let mask = 1u16 << lane;
    if value {
        *field |= mask;
    } else {
        *field &= !mask;
    }
}

impl Flags {
    // --- VCO: carry (low byte) / ne (high byte) ---

    /// VCO carry bit for `lane` (low byte). Written by `VADDC`/`VSUBC`, read
    /// as carry-in by `VADD`/`VSUB` and by the compares/clips.
    #[inline]
    pub fn vco_carry(&self, lane: usize) -> bool {
        get_bit(self.vco, lane)
    }
    #[inline]
    pub fn set_vco_carry(&mut self, lane: usize, value: bool) {
        set_bit(&mut self.vco, lane, value);
    }

    /// VCO "not equal" bit for `lane` (high byte, so lane + 8).
    #[inline]
    pub fn vco_ne(&self, lane: usize) -> bool {
        get_bit(self.vco, lane + 8)
    }
    #[inline]
    pub fn set_vco_ne(&mut self, lane: usize, value: bool) {
        set_bit(&mut self.vco, lane + 8, value);
    }

    // --- VCC: compare/clip low (low byte) / high (high byte) ---

    /// VCC low bit for `lane` (primary compare / clip-low).
    #[inline]
    pub fn vcc_low(&self, lane: usize) -> bool {
        get_bit(self.vcc, lane)
    }
    #[inline]
    pub fn set_vcc_low(&mut self, lane: usize, value: bool) {
        set_bit(&mut self.vcc, lane, value);
    }

    /// VCC high bit for `lane` (secondary compare / clip-high).
    #[inline]
    pub fn vcc_high(&self, lane: usize) -> bool {
        get_bit(self.vcc, lane + 8)
    }
    #[inline]
    pub fn set_vcc_high(&mut self, lane: usize, value: bool) {
        set_bit(&mut self.vcc, lane + 8, value);
    }

    // --- VCE: 8-bit compare extension ---

    /// VCE bit for `lane`.
    #[inline]
    pub fn vce(&self, lane: usize) -> bool {
        (self.vce >> lane) & 1 != 0
    }
    #[inline]
    pub fn set_vce(&mut self, lane: usize, value: bool) {
        let mask = 1u8 << lane;
        if value {
            self.vce |= mask;
        } else {
            self.vce &= !mask;
        }
    }

    /// Clear both VCO bytes for a lane (compares/clips do this after
    /// consuming the flags — RSP-VU-ISA.md §6.3, §6.6, §6.7).
    #[inline]
    pub fn clear_vco_lane(&mut self, lane: usize) {
        self.set_vco_carry(lane, false);
        self.set_vco_ne(lane, false);
    }
}

/// The complete VU state the op impls operate on: register file, accumulator,
/// flags, and the reciprocal/inverse-sqrt div latch. This is the single
/// `&mut self` an op receives (plus the operand register references the
/// generated call passes).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct VuState {
    /// The 32-register vector file. Named `regs` here; the generated-call
    /// analogue accesses `vpu.r[n]` — an adapter layer maps `rsp.vpu.r` onto
    /// `state.regs.r` in the ops/dispatch phase.
    pub regs: VuRegs,
    /// The 48-bit-per-lane accumulator.
    pub acc: Accumulator,
    /// VCO/VCC/VCE flags.
    pub flags: Flags,
    /// The 16-bit "high input" latch for the two-instruction VRCP/VRSQ
    /// sequence: `VRCPH`/`VRSQH` write it (the high 16 of the 32-bit
    /// operand), the following `VRCPL`/`VRSQL` consumes it. See
    /// RSP-VU-ISA.md §6.12.
    pub div_in: u16,
    /// Whether `div_in` currently holds a latched high half (set by a
    /// preceding `…H`, cleared after the paired `…L`/single op consumes it).
    /// The single-precision `VRCP`/`VRSQ` form treats the high half as 0.
    pub div_in_loaded: bool,
    /// The 16-bit "high output" latch: the high 16 of the last computed
    /// reciprocal/inverse-sqrt result, emitted by the next `VRCPH`/`VRSQH`.
    pub div_out: u16,
}

impl VuState {
    /// Fresh zeroed VU state.
    pub fn new() -> Self {
        VuState::default()
    }
}

// ---------------------------------------------------------------------------
// Element selection (RSP-VU-ISA.md §5)
// ---------------------------------------------------------------------------

/// The source lane `vt` reads for destination lane `i` under element modifier
/// `e` (0..15). Reproduces the shuffle table in RSP-VU-ISA.md §5:
/// - `e = 0 | 1`  → identity (`src = i`)
/// - `e = 2 | 3`  → quarter broadcast (keep the pair, pick lo/hi by `e & 1`)
/// - `e = 4..=7`  → half broadcast (keep the group of 4, pick by `e & 3`)
/// - `e = 8..=15` → whole broadcast of lane `e - 8`
#[inline]
pub fn element_source(e: usize, i: usize) -> usize {
    match e & 0xF {
        0 | 1 => i,
        2 | 3 => (i & !1) | (e & 1),
        4..=7 => (i & !3) | (e & 3),
        _ => (e & 0xF) - 8, // 8..=15
    }
}

/// Produce the element-selected view of `vt` for modifier `e`: for each lane
/// `i`, `out[i] = vt[element_source(e, i)]`. This is the ONE helper every
/// `Vt`-consuming op calls (RSP-VU-ISA.md §5).
#[inline]
pub fn element_select(vt: &Vec8, e: usize) -> Vec8 {
    let mut out = [0i16; LANES];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = vt[element_source(e, i)];
    }
    out
}

/// The single source lane a scalar op (VRCP/VRSQ/VMOV) reads from `vt` for a
/// given element field `e`. Scalar VMOV/VRCP/VRSQ instructions interpret the
/// field as a direct element number, independent of the destination element
/// (`de`), so the source is simply `e & 7` rather than the arithmetic-op
/// element-selection shuffle.
#[inline]
pub fn scalar_source_lane(e: usize, _de: usize) -> usize {
    e & 7
}

// ---------------------------------------------------------------------------
// Clamp / saturation modes (RSP-VU-ISA.md §4)
// ---------------------------------------------------------------------------

/// Signed clamp of a wide value to `i16` range `[-32768, 32767]`
/// (RSP-VU-ISA.md §4 mode 1). Returns the clamped value as `i16`.
#[inline]
pub fn clamp_signed(value: i64) -> i16 {
    if value > i16::MAX as i64 {
        i16::MAX
    } else if value < i16::MIN as i64 {
        i16::MIN
    } else {
        value as i16
    }
}

/// Unsigned fractional clamp used by `VMULU`/`VMACU`. Their signed product is
/// interpreted as an unsigned fraction: negative accumulators clamp to zero,
/// positive values whose MD sign bit is set clamp to `0xFFFF`, and the
/// remaining positive range returns MD. This is the HI/MD sign test in
/// CEN64's `rsp_vmacf_vmacu`, not an ordinary 0..=65535 integer clamp.
#[inline]
pub fn clamp_unsigned(value: i64) -> u16 {
    if value < 0 {
        0x0000
    } else if value > i16::MAX as i64 {
        0xFFFF
    } else {
        (value & 0xFFFF) as u16
    }
}

/// The low-slice clamp used by `VMADN`/`VMADL`. LO is returned only when HI is
/// a sign extension of MD. Otherwise negative overflow clamps to zero and
/// positive overflow to `0xFFFF` (SGI guide's `Clamp_Signed(ACC15..0)`,
/// independently structured as CEN64's `rsp_uclamp_acc`).
#[inline]
pub fn clamp_unsigned_low(acc: &Accumulator, lane: usize) -> u16 {
    let hi = acc.read_hi(lane) as i16;
    let mid = acc.read_mid(lane) as i16;
    if hi == (mid >> 15) {
        acc.read_lo(lane)
    } else if hi < 0 {
        0
    } else {
        0xFFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_identity_e0_and_e1() {
        let vt = [10, 20, 30, 40, 50, 60, 70, 80];
        assert_eq!(element_select(&vt, 0), vt);
        assert_eq!(element_select(&vt, 1), vt);
    }

    #[test]
    fn element_quarter_broadcast() {
        let vt = [0, 1, 2, 3, 4, 5, 6, 7];
        // e=2 (0q): pairs (0,0,2,2,4,4,6,6)
        assert_eq!(element_select(&vt, 2), [0, 0, 2, 2, 4, 4, 6, 6]);
        // e=3 (1q): pairs (1,1,3,3,5,5,7,7)
        assert_eq!(element_select(&vt, 3), [1, 1, 3, 3, 5, 5, 7, 7]);
    }

    #[test]
    fn element_half_broadcast() {
        let vt = [0, 1, 2, 3, 4, 5, 6, 7];
        // e=4 (0h): (0,0,0,0,4,4,4,4)
        assert_eq!(element_select(&vt, 4), [0, 0, 0, 0, 4, 4, 4, 4]);
        // e=5 (1h): (1,1,1,1,5,5,5,5)
        assert_eq!(element_select(&vt, 5), [1, 1, 1, 1, 5, 5, 5, 5]);
        // e=6 (2h): (2,2,2,2,6,6,6,6)
        assert_eq!(element_select(&vt, 6), [2, 2, 2, 2, 6, 6, 6, 6]);
        // e=7 (3h): (3,3,3,3,7,7,7,7)
        assert_eq!(element_select(&vt, 7), [3, 3, 3, 3, 7, 7, 7, 7]);
    }

    #[test]
    fn element_whole_broadcast() {
        let vt = [0, 1, 2, 3, 4, 5, 6, 7];
        for lane in 0..8 {
            let e = 8 + lane;
            assert_eq!(element_select(&vt, e), [vt[lane]; 8], "e={e}");
        }
    }

    #[test]
    fn accumulator_set_and_slices() {
        let mut acc = Accumulator::default();
        // 0x1234_5678_9ABC across HI:MD:LO
        acc.set(0, 0x1234_5678_9ABC);
        assert_eq!(acc.read_hi(0), 0x1234);
        assert_eq!(acc.read_mid(0), 0x5678);
        assert_eq!(acc.read_lo(0), 0x9ABC);
        assert_eq!(acc.signed(0), 0x1234_5678_9ABC);
    }

    #[test]
    fn accumulator_negative_sign_extends() {
        let mut acc = Accumulator::default();
        // -1 in 48-bit two's complement is 0xFFFF_FFFF_FFFF.
        acc.set(0, -1);
        assert_eq!(acc.read_hi(0), 0xFFFF);
        assert_eq!(acc.read_mid(0), 0xFFFF);
        assert_eq!(acc.read_lo(0), 0xFFFF);
        assert_eq!(acc.signed(0), -1);
    }

    #[test]
    fn accumulator_add_wraps_within_48_bits() {
        let mut acc = Accumulator::default();
        acc.set(0, 0x7FFF_FFFF_FFFF); // max positive 48-bit
        acc.add(0, 1); // overflow to min negative
        assert_eq!(acc.signed(0), -(1i64 << 47));
    }

    #[test]
    fn accumulator_write_lo_preserves_hi_mid() {
        let mut acc = Accumulator::default();
        acc.set(0, 0x1111_2222_3333);
        acc.write_lo(0, 0xBEEF);
        assert_eq!(acc.read_hi(0), 0x1111);
        assert_eq!(acc.read_mid(0), 0x2222);
        assert_eq!(acc.read_lo(0), 0xBEEF);
    }

    #[test]
    fn clamp_signed_saturates() {
        assert_eq!(clamp_signed(0x7FFF), 0x7FFF);
        assert_eq!(clamp_signed(0x8000), i16::MAX);
        assert_eq!(clamp_signed(-0x8000), i16::MIN);
        assert_eq!(clamp_signed(-0x8001), i16::MIN);
        assert_eq!(clamp_signed(100), 100);
    }

    #[test]
    fn clamp_unsigned_rule() {
        assert_eq!(clamp_unsigned(-1), 0x0000);
        assert_eq!(clamp_unsigned(0x1234), 0x1234);
        assert_eq!(clamp_unsigned(0x7FFF), 0x7FFF);
        assert_eq!(clamp_unsigned(0x8000), 0xFFFF);
    }

    #[test]
    fn clamp_unsigned_low_decides_by_hi_mid() {
        let mut acc = Accumulator::default();
        // A sign-extended negative accumulator is in range and returns LO.
        acc.set(0, -1);
        assert_eq!(clamp_unsigned_low(&acc, 0), 0xFFFF);
        // Positive and negative HI/MD sign mismatches clamp outward.
        acc.set(0, 0x1_0000_0000); // top32 = 0x1_0000
        assert_eq!(clamp_unsigned_low(&acc, 0), 0xFFFF);
        acc.set(0, -(0x8001i64 << 16));
        assert_eq!(clamp_unsigned_low(&acc, 0), 0x0000);
        // in range -> raw acc_lo
        acc.set(0, 0x0000_1234_5678); // top32 = 0x1234, lo = 0x5678
        assert_eq!(clamp_unsigned_low(&acc, 0), 0x5678);
    }

    #[test]
    fn flags_vco_carry_and_ne_are_independent_bytes() {
        let mut f = Flags::default();
        f.set_vco_carry(3, true);
        f.set_vco_ne(3, true);
        assert!(f.vco_carry(3));
        assert!(f.vco_ne(3));
        assert!(!f.vco_carry(4));
        f.clear_vco_lane(3);
        assert!(!f.vco_carry(3));
        assert!(!f.vco_ne(3));
    }

    #[test]
    fn flags_vcc_low_high_and_vce() {
        let mut f = Flags::default();
        f.set_vcc_low(0, true);
        f.set_vcc_high(7, true);
        f.set_vce(2, true);
        assert!(f.vcc_low(0));
        assert!(f.vcc_high(7));
        assert!(!f.vcc_high(0));
        assert!(f.vce(2));
        assert!(!f.vce(3));
    }
}
