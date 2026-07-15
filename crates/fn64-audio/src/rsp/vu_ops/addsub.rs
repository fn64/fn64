//! The RSP VU **add/subtract** op family: `VADD`, `VADDC`, `VSUB`, `VSUBC`,
//! `VABS`, and the (recompiler-unused-but-defined) `VZERO` pseudo-op
//! (RSP-VU-ISA.md §6.3, §6.4, §6.5).
//!
//! Portable scalar Rust against the [`super::super::vu`] `VuState` API —
//! per-lane `i16` math with `i32` intermediates, no SIMD. Each op follows the
//! spec exactly, in particular the split between the *wrapped* value written to
//! `acc_lo` and the *clamped* value written to `vd`, the VCO carry/ne
//! production and clearing, and the `-0x8000` negate quirk in VABS.
//!
//! ## Contract with the spec
//!
//! - **`VADD`** (§6.3): `sum = vs[i] + vt_e[i] + carry_in` with
//!   `carry_in = VCO.carry[i]`. `acc_lo[i]` = the raw 16-bit wrapped sum;
//!   `vd[i] = clamp_signed(sum)`. Both VCO bytes (carry + ne) are cleared for
//!   every lane afterward.
//! - **`VADDC`** (§6.3): `sum = (u16)vs[i] + (u16)vt_e[i]` (17-bit unsigned).
//!   `vd[i] = acc_lo[i] = sum & 0xFFFF` (no clamp). `VCO.carry[i] = bit16`,
//!   `VCO.ne[i] = 0`.
//! - **`VSUB`** (§6.3): `diff = vs[i] - vt_e[i] - carry_in`. `acc_lo[i]` = the
//!   wrapped 16-bit diff; `vd[i] = clamp_signed(diff)`. Both VCO bytes cleared.
//! - **`VSUBC`** (§6.3): `diff = (u16)vs[i] - (u16)vt_e[i]`.
//!   `vd[i] = acc_lo[i] = diff & 0xFFFF`. `VCO.carry[i] = borrow` (1 iff
//!   `vs < vt` unsigned), `VCO.ne[i] = (diff != 0)`.
//! - **`VABS`** (§6.4): apply the sign of `vs[i]` to `vt_e[i]`. `vs>0` →
//!   `vt_e`; `vs<0` → `clamp_signed(-vt_e)` in `vd` while `acc_lo` gets the
//!   *unclamped* wrapped `(-vt_e) & 0xFFFF` (so negating `0x8000` yields
//!   `0x7FFF` in `vd` but `0x8000` in `acc_lo`); `vs==0` → `0` in both. No
//!   flags.
//! - **`VZERO`** (§6.5): `acc_lo[i] = vs[i] + vt_e[i]` (truncated, wrapped) and
//!   `vd[i] = acc_lo[i]`; no clamp, no flags. The recompiler never emits it,
//!   but it is defined for completeness.

use super::super::ops::{OpInvocation, OpStatus, VuOp};
use super::super::vu::{clamp_signed, element_select, VuState};

/// Attempt to execute one of the add/sub-family ops. Returns `Some(Executed)`
/// if `op` belongs to this family, or `None` if it does not (so the top-level
/// dispatcher can try the next family). Mirrors the `dispatch_mac` pattern.
pub fn dispatch_addsub(state: &mut VuState, op: VuOp, inv: &OpInvocation) -> Option<OpStatus> {
    match op {
        VuOp::Vadd => vadd(state, inv),
        VuOp::Vaddc => vaddc(state, inv),
        VuOp::Vsub => vsub(state, inv),
        VuOp::Vsubc => vsubc(state, inv),
        VuOp::Vabs => vabs(state, inv),
        _ => return None,
    }
    Some(OpStatus::Executed)
}

/// `VADD` — signed add with carry-in, signed-clamp result, wrapped acc_lo.
/// Clears both VCO bytes (RSP-VU-ISA.md §6.3).
pub fn vadd(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let carry_in = state.flags.vco_carry(i) as i32;
        let sum = vs[i] as i32 + vt_e[i] as i32 + carry_in;
        // acc_lo receives the raw 16-bit wrapped sum (NOT the clamped result).
        state.acc.write_lo(i, (sum & 0xFFFF) as u16);
        // vd receives the signed-clamped sum.
        vd[i] = clamp_signed(sum as i64);
    }
    for i in 0..8 {
        state.flags.clear_vco_lane(i);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VADDC` — unsigned add producing carry-out, truncated (no clamp) result.
/// Sets VCO.carry to the 17th bit, clears VCO.ne (RSP-VU-ISA.md §6.3).
pub fn vaddc(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let sum = (vs[i] as u16) as u32 + (vt_e[i] as u16) as u32;
        let low = (sum & 0xFFFF) as u16;
        state.acc.write_lo(i, low);
        vd[i] = low as i16;
        state.flags.set_vco_carry(i, (sum >> 16) & 1 != 0);
        state.flags.set_vco_ne(i, false);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VSUB` — signed subtract with borrow-in, signed-clamp result, wrapped
/// acc_lo. Clears both VCO bytes (RSP-VU-ISA.md §6.3).
pub fn vsub(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let carry_in = state.flags.vco_carry(i) as i32;
        let diff = vs[i] as i32 - vt_e[i] as i32 - carry_in;
        state.acc.write_lo(i, (diff & 0xFFFF) as u16);
        vd[i] = clamp_signed(diff as i64);
    }
    for i in 0..8 {
        state.flags.clear_vco_lane(i);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VSUBC` — unsigned subtract producing borrow + not-equal, truncated (no
/// clamp) result (RSP-VU-ISA.md §6.3).
pub fn vsubc(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        // 17-bit unsigned subtract; the borrow is 1 iff vs < vt unsigned.
        let a = (vs[i] as u16) as u32;
        let b = (vt_e[i] as u16) as u32;
        let raw = a.wrapping_sub(b);
        let low = (raw & 0xFFFF) as u16;
        state.acc.write_lo(i, low);
        vd[i] = low as i16;
        state.flags.set_vco_carry(i, a < b);
        // ne: set when the operands differ (equivalently low != 0).
        state.flags.set_vco_ne(i, low != 0);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VABS` — apply the sign of `vs[i]` to `vt_e[i]` (RSP-VU-ISA.md §6.4).
/// The `-0x8000` negate overflows: `vd` gets the signed-clamped `0x7FFF`, but
/// `acc_lo` gets the raw wrapped `0x8000`. No flags.
pub fn vabs(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let (lo, res): (u16, i16) = if vs[i] > 0 {
            (vt_e[i] as u16, vt_e[i])
        } else if vs[i] < 0 {
            // Negate with the -0x8000 quirk. -(vt) computed in i32 so the
            // 0x8000 case is +32768 before clamp; wrapped low is 0x8000.
            let neg = -(vt_e[i] as i32);
            ((neg & 0xFFFF) as u16, clamp_signed(neg as i64))
        } else {
            (0, 0)
        };
        state.acc.write_lo(i, lo);
        vd[i] = res;
    }
    state.regs.r[inv.vd] = vd;
}

/// `VZERO` — the recompiler-unused "add-and-discard-clamp" pseudo (RSP-VU-ISA.md
/// §6.5): `acc_lo[i] = vs[i] + vt_e[i]` wrapped, `vd[i] = acc_lo[i]`; no clamp,
/// no flags. Not dispatched (the recompiler never emits it), but defined for
/// completeness and unit-tested.
pub fn vzero(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let sum = vs[i] as i32 + vt_e[i] as i32;
        let low = (sum & 0xFFFF) as u16;
        state.acc.write_lo(i, low);
        vd[i] = low as i16;
    }
    state.regs.r[inv.vd] = vd;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(vd: usize, vs: usize, vt: usize, e: usize) -> OpInvocation {
        OpInvocation {
            vd,
            vs,
            vt,
            e,
            ..Default::default()
        }
    }

    // --- VADD -------------------------------------------------------------

    /// VADD must (a) add the VCO carry-in, (b) signed-CLAMP the result into vd,
    /// and (c) write the UNCLAMPED wrapped sum into acc_lo. Chosen so a wrong
    /// clamp mode (truncate instead of clamp) or a dropped carry-in gives a
    /// visibly different vd/acc_lo.
    #[test]
    fn vadd_clamps_vd_wraps_acc_and_uses_carry_in() {
        let mut st = VuState::new();
        // lane0: 0x7000 + 0x7000 = 0xE000 = 57344, clamps to 0x7FFF; wrapped
        //        low is 0xE000. Truncate-instead-of-clamp would give 0xE000.
        // lane1: 0x7FFF + 1 + carry(1) = 0x8001 -> clamp 0x7FFF; acc_lo 0x8001.
        //        Dropping the carry-in makes acc_lo 0x8000 not 0x8001 — so the
        //        carry-in is observable in acc_lo even though vd clamps either
        //        way.
        // lane2: -0x8000 + -1 + 0 = -0x8001 -> clamp -0x8000; acc_lo 0x7FFF.
        st.regs.r[2] = [0x7000, 0x7FFF, -0x8000, 0, 0, 0, 0, 0];
        st.regs.r[3] = [0x7000, 0x0001, -0x0001, 0, 0, 0, 0, 0];
        st.flags.set_vco_carry(1, true);
        vadd(&mut st, &inv(1, 2, 3, 0));
        // vd is the CLAMPED result.
        assert_eq!(st.regs.r[1][0], 0x7FFF, "lane0 vd clamps high");
        assert_eq!(st.regs.r[1][1], 0x7FFF, "lane1 vd clamps high");
        assert_eq!(st.regs.r[1][2], -0x8000, "lane2 vd clamps low");
        // acc_lo is the WRAPPED sum, distinct from vd on every clamped lane.
        assert_eq!(st.acc.read_lo(0), 0xE000, "lane0 acc_lo wraps (not clamp)");
        assert_eq!(st.acc.read_lo(1), 0x8001, "lane1 acc_lo carries the carry-in");
        assert_eq!(st.acc.read_lo(2), 0x7FFF, "lane2 acc_lo wraps");
        // Distinguishing assertions: vd != acc_lo on the clamped lanes proves
        // we did NOT green-against-a-truncate bug.
        assert_ne!(st.regs.r[1][0] as u16, st.acc.read_lo(0));
        assert_ne!(st.regs.r[1][1] as u16, st.acc.read_lo(1));
        // VCO must be fully cleared.
        assert_eq!(st.flags.vco, 0, "VADD clears both VCO bytes");
    }

    /// The carry-in itself changes the numeric result — pin it directly.
    #[test]
    fn vadd_carry_in_changes_result() {
        let mut st = VuState::new();
        st.regs.r[2] = [10, 10, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [20, 20, 0, 0, 0, 0, 0, 0];
        st.flags.set_vco_carry(1, true); // only lane1 has carry-in
        vadd(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 30, "lane0 no carry: 10+20");
        assert_eq!(st.regs.r[1][1], 31, "lane1 carry-in adds 1: 10+20+1");
    }

    // --- VADDC ------------------------------------------------------------

    /// VADDC produces the carry-out in VCO.carry and truncates (no clamp).
    /// Chosen so a signed-clamp bug would corrupt the result.
    #[test]
    fn vaddc_carry_out_and_truncates() {
        let mut st = VuState::new();
        // lane0: 0xFFFF + 0x0002 = 0x1_0001 -> low 0x0001, carry 1.
        //        A signed clamp of 65537 would give 0x7FFF: wrong.
        // lane1: 0x8000 + 0x8000 = 0x1_0000 -> low 0x0000, carry 1.
        // lane2: 0x0001 + 0x0002 = 0x0003 -> low 3, carry 0.
        st.regs.r[2] = [-1i16, -0x8000, 1, 0, 0, 0, 0, 0]; // 0xFFFF, 0x8000, 1
        st.regs.r[3] = [2, -0x8000, 2, 0, 0, 0, 0, 0]; // 2, 0x8000, 2
        // pre-set an ne bit to confirm VADDC clears it.
        st.flags.set_vco_ne(0, true);
        vaddc(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0] as u16, 0x0001, "lane0 truncated low");
        assert_eq!(st.regs.r[1][1] as u16, 0x0000, "lane1 truncated low");
        assert_eq!(st.regs.r[1][2] as u16, 0x0003, "lane2 low");
        assert!(st.flags.vco_carry(0), "lane0 carry-out");
        assert!(st.flags.vco_carry(1), "lane1 carry-out");
        assert!(!st.flags.vco_carry(2), "lane2 no carry");
        assert!(!st.flags.vco_ne(0), "VADDC clears ne");
        assert_eq!(st.acc.read_lo(0), 0x0001, "acc_lo mirrors vd");
    }

    // --- VSUB -------------------------------------------------------------

    /// VSUB signed-clamps vd, wraps acc_lo, subtracts the borrow-in. Chosen so
    /// wrong clamp mode / dropped borrow-in diverges.
    #[test]
    fn vsub_clamps_vd_wraps_acc_and_uses_borrow_in() {
        let mut st = VuState::new();
        // lane0: -0x8000 - 0x0001 - 0 = -0x8001 -> clamp -0x8000; acc_lo 0x7FFF.
        //        Truncate-instead-of-clamp would put 0x7FFF in vd — wrong.
        // lane1: 0x7FFF - (-1) - 0 = 0x8000 -> clamp 0x7FFF; acc_lo 0x8000.
        // lane2: 100 - 40 - borrow(1) = 59; without borrow-in it'd be 60.
        st.regs.r[2] = [-0x8000, 0x7FFF, 100, 0, 0, 0, 0, 0];
        st.regs.r[3] = [0x0001, -1, 40, 0, 0, 0, 0, 0];
        st.flags.set_vco_carry(2, true); // borrow-in on lane2 only
        vsub(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], -0x8000, "lane0 vd clamps low");
        assert_eq!(st.regs.r[1][1], 0x7FFF, "lane1 vd clamps high");
        assert_eq!(st.regs.r[1][2], 59, "lane2 borrow-in subtracts 1");
        assert_eq!(st.acc.read_lo(0), 0x7FFF, "lane0 acc_lo wraps -0x8001");
        assert_eq!(st.acc.read_lo(1), 0x8000, "lane1 acc_lo wraps 0x8000");
        // vd != acc_lo on the clamped lanes -> not green-against-truncate.
        assert_ne!(st.regs.r[1][0] as u16, st.acc.read_lo(0));
        assert_ne!(st.regs.r[1][1] as u16, st.acc.read_lo(1));
        assert_eq!(st.flags.vco, 0, "VSUB clears both VCO bytes");
    }

    // --- VSUBC ------------------------------------------------------------

    /// VSUBC produces borrow (carry) + ne, truncates. Chosen so the UNSIGNED
    /// borrow rule and the ne=(diff!=0) rule are both distinguishable from the
    /// signed alternatives.
    #[test]
    fn vsubc_borrow_ne_and_truncates() {
        let mut st = VuState::new();
        // lane0: 0x0003 - 0x0005 = -2 -> low 0xFFFE, borrow 1 (3<5), ne 1.
        // lane1: 0x0005 - 0x0005 = 0 -> low 0x0000, borrow 0, ne 0 (equal!).
        // lane2: 0x8000 - 0x0001 = 0x7FFF -> low 0x7FFF, borrow 0
        //        (0x8000 > 1 UNSIGNED). A SIGNED compare would wrongly say
        //        0x8000(-32768) < 1 -> borrow 1: this lane distinguishes
        //        unsigned-vs-signed borrow.
        st.regs.r[2] = [3, 5, -0x8000, 0, 0, 0, 0, 0]; // 3,5,0x8000
        st.regs.r[3] = [5, 5, 1, 0, 0, 0, 0, 0];
        vsubc(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0] as u16, 0xFFFE, "lane0 low");
        assert_eq!(st.regs.r[1][1] as u16, 0x0000, "lane1 low");
        assert_eq!(st.regs.r[1][2] as u16, 0x7FFF, "lane2 low");
        assert!(st.flags.vco_carry(0), "lane0 borrow (3<5)");
        assert!(!st.flags.vco_carry(1), "lane1 no borrow (equal)");
        assert!(!st.flags.vco_carry(2), "lane2 no borrow: 0x8000 > 1 UNSIGNED");
        assert!(st.flags.vco_ne(0), "lane0 ne (differ)");
        assert!(!st.flags.vco_ne(1), "lane1 not-ne (equal)");
        assert!(st.flags.vco_ne(2), "lane2 ne (differ)");
    }

    // --- VABS -------------------------------------------------------------

    /// VABS applies vs's sign to vt_e, with the -0x8000 clamp/acc split.
    #[test]
    fn vabs_sign_apply_and_0x8000_quirk() {
        let mut st = VuState::new();
        // lane0: vs>0 -> passthrough vt_e = 0x1234.
        // lane1: vs<0 -> negate vt_e(5) = -5; acc_lo 0xFFFB.
        // lane2: vs<0 and vt_e=0x8000 -> negate overflows: vd = 0x7FFF
        //        (clamped) but acc_lo = 0x8000 (raw wrapped). THE quirk.
        // lane3: vs==0 -> 0 in both regardless of vt_e.
        st.regs.r[2] = [7, -3, -1, 0, 0, 0, 0, 0];
        st.regs.r[3] = [0x1234, 5, -0x8000, 0x4444, 0, 0, 0, 0];
        vabs(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 0x1234, "lane0 passthrough");
        assert_eq!(st.regs.r[1][1], -5, "lane1 negate");
        assert_eq!(st.regs.r[1][2], 0x7FFF, "lane2 vd clamps -(-0x8000)");
        assert_eq!(st.regs.r[1][3], 0, "lane3 vs==0 -> 0");
        assert_eq!(st.acc.read_lo(0), 0x1234, "lane0 acc_lo");
        assert_eq!(st.acc.read_lo(1), 0xFFFB, "lane1 acc_lo = -5 wrapped");
        assert_eq!(
            st.acc.read_lo(2),
            0x8000,
            "lane2 acc_lo is RAW 0x8000, NOT the clamped 0x7FFF"
        );
        assert_eq!(st.acc.read_lo(3), 0x0000, "lane3 acc_lo");
        // The distinguishing assertion for the quirk: acc_lo != vd on lane2.
        assert_ne!(
            st.acc.read_lo(2),
            st.regs.r[1][2] as u16,
            "quirk: acc_lo(0x8000) must differ from vd(0x7FFF)"
        );
    }

    // --- VZERO ------------------------------------------------------------

    /// VZERO adds without clamp and writes acc_lo = vd. A signed-clamp bug
    /// would corrupt the overflowing lane.
    #[test]
    fn vzero_adds_without_clamp() {
        let mut st = VuState::new();
        // 0x7000 + 0x7000 = 0xE000 -> low 0xE000 (a clamp would give 0x7FFF).
        st.regs.r[2] = [0x7000, 1, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [0x7000, 2, 0, 0, 0, 0, 0, 0];
        vzero(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0] as u16, 0xE000, "no clamp: raw wrapped sum");
        assert_eq!(st.regs.r[1][1], 3);
        assert_eq!(st.acc.read_lo(0), 0xE000, "acc_lo = vd");
        assert_eq!(st.acc.read_lo(1), 3);
    }

    // --- Element modifier plumbing + dispatch -----------------------------

    /// Confirm the `e` broadcast is actually applied to vt in this family.
    #[test]
    fn vadd_applies_element_broadcast() {
        let mut st = VuState::new();
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0, 0, 0, 0, 0, 0, 0, 42];
        // e=15 -> whole-broadcast lane 7 (=42) to every lane.
        vadd(&mut st, &inv(1, 2, 3, 15));
        assert_eq!(st.regs.r[1], [42; 8], "e=15 broadcasts vt lane7");
    }

    /// The family dispatcher routes our five ops and declines the rest.
    #[test]
    fn dispatch_addsub_routes_family_only() {
        let mut st = VuState::new();
        let i = inv(1, 2, 3, 0);
        for op in [VuOp::Vadd, VuOp::Vaddc, VuOp::Vsub, VuOp::Vsubc, VuOp::Vabs] {
            assert_eq!(
                dispatch_addsub(&mut st, op, &i),
                Some(OpStatus::Executed),
                "{op:?} should be handled by addsub"
            );
        }
        // A foreign op is declined.
        assert_eq!(dispatch_addsub(&mut st, VuOp::Vmulf, &i), None);
    }
}
