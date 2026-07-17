//! The RSP VU **logical** op family: `VAND` `VNAND` `VOR` `VNOR` `VXOR`
//! `VNXOR` and `VNOP` (RSP-VU-ISA.md §6.5).
//!
//! These are the simplest VU ops: pure per-lane bitwise combinations of
//! `vs[i]` and the element-selected `vt_e[i]`, with the result written both to
//! the destination register lane and into `acc_lo` as a side effect. They
//! apply **no clamp** (§4 mode 3, "no clamp / truncate") and touch **no**
//! VCO/VCC/VCE flag. `VNOP` touches nothing at all.
//!
//! ## Bit-exactness notes (why these are simple but not trivial)
//!
//! - The combine is done on the raw 16-bit lane bit patterns. We operate on
//!   `u16` (`vs[i] as u16`, `vt_e[i] as u16`) so the complement ops (`VNAND`,
//!   `VNOR`, `VNXOR`) invert **all 16 bits** with a `u16` `!`, then reinterpret
//!   as `i16` for the register write — exactly the RSP's "write the 16-bit
//!   result pattern" behavior. Doing the `!` on an `i16` would produce the same
//!   two's-complement bit pattern here, but `u16` makes the "these are opaque
//!   bit patterns, not signed magnitudes" intent explicit and keeps `acc_lo`
//!   (a `u16`) a direct copy with no sign games.
//! - `acc_lo[i]` receives the **same** 16-bit result pattern as `vd[i]` (§6.5:
//!   "All six also set `acc_lo[i] = vd[i]`"). `acc_mid`/`acc_hi` are left
//!   untouched (`write_lo` preserves them).
//! - The element modifier `e` is applied to `vt` (and only `vt`) via
//!   [`element_select`], the one shared shuffle helper — a broadcast `e`
//!   changes *which* `vt` lane each destination lane reads, so a logic op with
//!   `e != 0` is emphatically not the identity even for `VOR x, x`.
//!
//! Everything here is portable scalar Rust — `[i16; 8]` lanes, per-lane `u16`
//! bit math, no SIMD.

use super::super::vu::{element_select, Vec8, VuState, LANES};

/// The specific bitwise combination a logic op performs on the two raw 16-bit
/// lane patterns. Kept as one enum + one combine function so all six ops share
/// exactly one code path (only the operator differs), which is what makes the
/// "flip the operator and the test goes red" guarantee meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicKind {
    /// `vd = vs & vt_e`.
    And,
    /// `vd = !(vs & vt_e)`.
    Nand,
    /// `vd = vs | vt_e`.
    Or,
    /// `vd = !(vs | vt_e)`.
    Nor,
    /// `vd = vs ^ vt_e`.
    Xor,
    /// `vd = !(vs ^ vt_e)`.
    Nxor,
}

impl LogicKind {
    /// Combine two raw 16-bit lane patterns per this op. Operates on `u16` so
    /// the complement variants invert the full 16-bit pattern with no
    /// sign-magnitude interpretation (RSP-VU-ISA.md §6.5).
    #[inline]
    fn combine(self, a: u16, b: u16) -> u16 {
        match self {
            LogicKind::And => a & b,
            LogicKind::Nand => !(a & b),
            LogicKind::Or => a | b,
            LogicKind::Nor => !(a | b),
            LogicKind::Xor => a ^ b,
            LogicKind::Nxor => !(a ^ b),
        }
    }
}

/// Execute one logical op (`VAND`/`VNAND`/`VOR`/`VNOR`/`VXOR`/`VNXOR`).
///
/// For each of the 8 lanes: `res = kind.combine(vs[i], vt_e[i])`, written to
/// `vd[i]` and into `acc_lo[i]`. No clamp, no flag change (RSP-VU-ISA.md §6.5).
///
/// `vs` / `vt` are register indices into `state.regs.r`; `vd` the destination
/// register index; `e` the element modifier applied to `vt` only.
pub fn exec_logic(state: &mut VuState, kind: LogicKind, vd: usize, vs: usize, vt: usize, e: usize) {
    // Snapshot the sources before writing vd — vd may alias vs and/or vt, and
    // element_select already returns an owned copy of the shuffled vt.
    let vs_v: Vec8 = state.regs.r[vs];
    let vt_e: Vec8 = element_select(&state.regs.r[vt], e);

    let mut result: Vec8 = [0i16; LANES];
    for i in 0..LANES {
        let res = kind.combine(vs_v[i] as u16, vt_e[i] as u16);
        result[i] = res as i16;
        // acc_lo[i] = vd[i] (same 16-bit pattern); acc_mid/acc_hi untouched.
        state.acc.write_lo(i, res);
    }
    state.regs.r[vd] = result;
}

/// `VNOP` — no operation. Touches nothing: no register, no accumulator, no
/// flag (RSP-VU-ISA.md §6.5). `e` is ignored. Present as a real function so the
/// dispatcher routes `VNOP` here rather than through an `Unimplemented` trap.
#[inline]
pub fn exec_vnop(_state: &mut VuState) {
    // Intentionally empty: VNOP is architecturally a no-op.
}

#[cfg(test)]
mod tests {
    use super::super::super::vu::VuState;
    use super::*;

    /// Build a VuState with vs in reg 1 and vt in reg 2, run `kind` with
    /// element modifier `e` into reg 3, and return (vd, acc_lo array).
    fn run(kind: LogicKind, vs: Vec8, vt: Vec8, e: usize) -> (Vec8, [u16; LANES]) {
        let mut st = VuState::new();
        st.regs.r[1] = vs;
        st.regs.r[2] = vt;
        exec_logic(&mut st, kind, 3, 1, 2, e);
        let acc_lo: [u16; LANES] = core::array::from_fn(|i| st.acc.read_lo(i));
        (st.regs.r[3], acc_lo)
    }

    // Distinguishable bit patterns: every pair (a,b) below yields a DIFFERENT
    // result under each of AND/NAND/OR/NOR/XOR/NXOR, so a test asserting one
    // op's output fails if the dispatcher routes to the wrong logic operator.
    //
    // Take a = 0xF0F0, b = 0x0FF0:
    //   AND  = 0x00F0   OR   = 0xFFF0   XOR  = 0xFF00
    //   NAND = 0xFF0F   NOR  = 0x000F   NXOR = 0x00FF
    // All six distinct. i16-reinterpretations follow from the bit patterns.

    const A: u16 = 0xF0F0;
    const B: u16 = 0x0FF0;

    fn broadcast(x: u16) -> Vec8 {
        [x as i16; LANES]
    }

    #[test]
    fn vand_exact_and_distinct_from_other_ops() {
        let (vd, acc) = run(LogicKind::And, broadcast(A), broadcast(B), 0);
        // AND(0xF0F0, 0x0FF0) = 0x00F0
        assert_eq!(vd, [0x00F0i16; LANES], "VAND result");
        assert_eq!(acc, [0x00F0u16; LANES], "VAND acc_lo mirrors vd");
        // Guard: this must NOT equal what a wrong operator would produce.
        assert_ne!(vd, [0xFFF0u16 as i16; LANES], "would be OR, not AND");
        assert_ne!(vd, [0xFF00u16 as i16; LANES], "would be XOR, not AND");
        assert_ne!(vd, [0xFF0Fu16 as i16; LANES], "would be NAND, not AND");
    }

    #[test]
    fn vnand_exact_is_complement_of_and() {
        let (vd, acc) = run(LogicKind::Nand, broadcast(A), broadcast(B), 0);
        // NAND(0xF0F0, 0x0FF0) = !0x00F0 = 0xFF0F
        assert_eq!(vd, [0xFF0Fu16 as i16; LANES], "VNAND result");
        assert_eq!(acc, [0xFF0Fu16; LANES], "VNAND acc_lo mirrors vd");
        // Must differ from plain AND (the un-complemented op).
        assert_ne!(vd, [0x00F0i16; LANES], "would be AND, not NAND");
    }

    #[test]
    fn vor_exact_and_distinct() {
        let (vd, acc) = run(LogicKind::Or, broadcast(A), broadcast(B), 0);
        // OR(0xF0F0, 0x0FF0) = 0xFFF0
        assert_eq!(vd, [0xFFF0u16 as i16; LANES], "VOR result");
        assert_eq!(acc, [0xFFF0u16; LANES], "VOR acc_lo mirrors vd");
        assert_ne!(vd, [0x00F0i16; LANES], "would be AND, not OR");
        assert_ne!(vd, [0xFF00u16 as i16; LANES], "would be XOR, not OR");
    }

    #[test]
    fn vnor_exact_is_complement_of_or() {
        let (vd, acc) = run(LogicKind::Nor, broadcast(A), broadcast(B), 0);
        // NOR(0xF0F0, 0x0FF0) = !0xFFF0 = 0x000F
        assert_eq!(vd, [0x000Fi16; LANES], "VNOR result");
        assert_eq!(acc, [0x000Fu16; LANES], "VNOR acc_lo mirrors vd");
        assert_ne!(vd, [0xFFF0u16 as i16; LANES], "would be OR, not NOR");
    }

    #[test]
    fn vxor_exact_and_distinct() {
        let (vd, acc) = run(LogicKind::Xor, broadcast(A), broadcast(B), 0);
        // XOR(0xF0F0, 0x0FF0) = 0xFF00
        assert_eq!(vd, [0xFF00u16 as i16; LANES], "VXOR result");
        assert_eq!(acc, [0xFF00u16; LANES], "VXOR acc_lo mirrors vd");
        assert_ne!(vd, [0x00F0i16; LANES], "would be AND, not XOR");
        assert_ne!(vd, [0xFFF0u16 as i16; LANES], "would be OR, not XOR");
    }

    #[test]
    fn vnxor_exact_is_complement_of_xor() {
        let (vd, acc) = run(LogicKind::Nxor, broadcast(A), broadcast(B), 0);
        // NXOR(0xF0F0, 0x0FF0) = !0xFF00 = 0x00FF
        assert_eq!(vd, [0x00FFi16; LANES], "VNXOR result");
        assert_eq!(acc, [0x00FFu16; LANES], "VNXOR acc_lo mirrors vd");
        assert_ne!(vd, [0xFF00u16 as i16; LANES], "would be XOR, not NXOR");
    }

    #[test]
    fn complement_ops_invert_all_16_bits() {
        // A pattern where a mistaken i16-vs-u16 complement, or clamping, would
        // change the result. NAND of 0x0000 & anything -> 0xFFFF (all ones).
        let (vd, _) = run(LogicKind::Nand, broadcast(0x0000), broadcast(0x1234), 0);
        assert_eq!(
            vd, [0xFFFFu16 as i16; LANES],
            "NAND with a zero operand is all ones (-1)"
        );
        // NOR of two zeros -> 0xFFFF as well; confirms full-width inversion.
        let (vd2, _) = run(LogicKind::Nor, broadcast(0x0000), broadcast(0x0000), 0);
        assert_eq!(
            vd2, [-1i16; LANES],
            "NOR(0,0) = 0xFFFF = -1, full 16-bit invert"
        );
    }

    #[test]
    fn element_modifier_selects_vt_lane_not_identity() {
        // vt distinct per lane; e=8 broadcasts vt lane 0 to every dest lane.
        // OR(vs[i], vt[0]) must use vt[0] for every lane, proving element
        // selection is applied to vt (a wrong "identity" impl would OR vs[i]
        // with vt[i] and give a different vector).
        let vs = [
            0x0001, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020, 0x0040, 0x0080,
        ];
        let vt = [
            0x8000u16 as i16,
            0x0100,
            0x0200,
            0x0400,
            0x0800,
            0x1000,
            0x2000,
            0x4000,
        ];
        let (vd, _) = run(LogicKind::Or, vs, vt, 8); // broadcast vt lane 0 = 0x8000
        let expected: Vec8 = core::array::from_fn(|i| (vs[i] as u16 | 0x8000u16) as i16);
        assert_eq!(vd, expected, "e=8 must OR each lane with vt[0]");
        // A naive identity (OR vs[i] with vt[i]) would differ in lanes 1..7.
        let identity: Vec8 = core::array::from_fn(|i| (vs[i] as u16 | vt[i] as u16) as i16);
        assert_ne!(vd, identity, "must NOT be the per-lane identity pairing");
    }

    #[test]
    fn logic_ops_do_not_touch_flags_or_acc_mid_hi() {
        let mut st = VuState::new();
        st.regs.r[1] = broadcast(A);
        st.regs.r[2] = broadcast(B);
        // Pre-seed acc mid/hi and flags to non-zero; the op must leave them.
        for i in 0..LANES {
            st.acc.set(i, 0x1111_2222_0000);
        }
        st.flags.vco = 0xBEEF;
        st.flags.vcc = 0xCAFE;
        st.flags.vce = 0x5A;
        exec_logic(&mut st, LogicKind::And, 3, 1, 2, 0);
        for i in 0..LANES {
            assert_eq!(st.acc.read_hi(i), 0x1111, "acc_hi preserved lane {i}");
            assert_eq!(st.acc.read_mid(i), 0x2222, "acc_mid preserved lane {i}");
            assert_eq!(st.acc.read_lo(i), 0x00F0, "acc_lo = result lane {i}");
        }
        assert_eq!(st.flags.vco, 0xBEEF, "VCO untouched");
        assert_eq!(st.flags.vcc, 0xCAFE, "VCC untouched");
        assert_eq!(st.flags.vce, 0x5A, "VCE untouched");
    }

    #[test]
    fn vnop_touches_nothing() {
        let mut st = VuState::new();
        st.regs.r[3] = broadcast(0x1234);
        for i in 0..LANES {
            st.acc.set(i, 0x0001_0002_0003);
        }
        st.flags.vco = 0x1234;
        st.flags.vcc = 0x5678;
        st.flags.vce = 0x9A;
        let before = st.clone();
        exec_vnop(&mut st);
        assert_eq!(st, before, "VNOP must not mutate any VU state");
    }
}
