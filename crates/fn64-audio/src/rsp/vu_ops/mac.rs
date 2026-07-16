//! The RSP VU "mac" op family: the multiply-**accumulate** ops plus the
//! accumulator-slice reader `VSAR`.
//!
//! Ops implemented here (RSP-VU-ISA.md §6.2, §6.9):
//! - `VMACF` — signed fractional multiply-accumulate, signed clamp.
//! - `VMACQ` — accumulate step of `VMULQ` (Q rounding), no `vs`/`vt`.
//! - `VMADH` — signed×signed high, `ACC += p << 16`, signed clamp.
//! - `VMADM` — signed(vs)×unsigned(vt), `ACC += sext(p)`, signed clamp.
//! - `VMADN` — unsigned(vs)×signed(vt), `ACC += sext(p)`, **unsigned-low clamp**.
//! - `VMADL` — unsigned×unsigned, `ACC += p >> 16`, **unsigned-low clamp**.
//! - `VSAR`  — read one accumulator slice (`e = 8/9/10` → HI/MD/LO) into `vd`.
//!
//! Every op writes all 8 lanes of `vd` and (except `VSAR`) mutates `state.acc`.
//! None of these ops touch VCO/VCC/VCE. All math is portable scalar Rust:
//! `i16` source lanes, `i32`/`i64` intermediates, no SIMD.
//!
//! The distinguishing traps (RSP-VU-ISA.md §7) this module is careful about:
//! - `VMACF` adds `p << 1` (NO `+0x8000` round bias — that is a `VMULF`-only
//!   feature).
//! - `VMADN`/`VMADL` extract the low word with the **unsigned-low clamp**
//!   (`clamp_unsigned_low`), NOT a plain truncate — the accumulate-into-low
//!   path can overflow acc_lo into acc_mid, and the clamp decides by the sign
//!   of `acc[47..16]`. Their `VMUDN`/`VMUDL` set-accumulator cousins truncate.
//! - `VMADH`/`VMADM` extract with the **signed clamp** of `acc[47..16]`.

use super::super::ops::{OpInvocation, OpStatus, VuOp};
use super::super::vu::{clamp_signed, clamp_unsigned, clamp_unsigned_low, element_select, VuState};

/// Attempt to execute one of the "mac" family ops. Returns `Some(status)` if
/// `op` belongs to this family (and it was executed), or `None` if it does not
/// — so the shared dispatcher can fall through to the other op groups without
/// this module claiming ops it doesn't own.
pub fn dispatch_mac(state: &mut VuState, op: VuOp, inv: &OpInvocation) -> Option<OpStatus> {
    match op {
        VuOp::Vmacf => vmacf(state, inv),
        VuOp::Vmacu => vmacu(state, inv),
        VuOp::Vmacq => vmacq(state, inv),
        VuOp::Vmadh => vmadh(state, inv),
        VuOp::Vmadm => vmadm(state, inv),
        VuOp::Vmadn => vmadn(state, inv),
        VuOp::Vmadl => vmadl(state, inv),
        VuOp::Vsar => vsar(state, inv),
        _ => return None,
    }
    Some(OpStatus::Executed)
}

/// `VMACU` — signed fractional multiply-accumulate with unsigned result
/// clamping (Programmer's Guide, Table 3-4). Unlike `VMULU`, the accumulate
/// step adds no new rounding constant.
fn vmacu(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        let p = (vs[i] as i32) * (vt_e[i] as i32);
        state.acc.add(i, (p as i64) << 1);
        state.regs.r[inv.vd][i] = clamp_unsigned(state.acc.signed(i) >> 16) as i16;
    }
}

/// `VMACF` — like `VMULF` but ACCUMULATE: `ACC += (p << 1)` with the signed
/// 32-bit product `p = vs[i] * vt_e[i]`, then `vd[i] = clamp_signed(ACC >> 16)`.
/// No `+0x8000` round bias on the accumulate step (that is VMULF-only). §6.2.
fn vmacf(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        let p = (vs[i] as i32) * (vt_e[i] as i32); // signed × signed
        let delta = (p as i64) << 1; // 2*p, NOT +0x8000
        state.acc.add(i, delta);
        state.regs.r[inv.vd][i] = clamp_signed(state.acc.signed(i) >> 16);
    }
}

/// `VMACQ` — the accumulate step of `VMULQ`. Takes no `vs`/`vt`; it nudges the
/// existing ACC by `32 << 16` when bits 47..21 are nonzero and bit 21 is
/// clear: add for a negative accumulator and subtract for a positive one.
/// Result extraction matches `VMULQ`: signed-clamp ACC bits 32..17, then clear
/// the low nibble (Programmer's Guide, p. 62).
fn vmacq(state: &mut VuState, inv: &OpInvocation) {
    for i in 0..8 {
        let acc = state.acc.signed(i);
        let upper_nonzero = (acc >> 21) != 0;
        let bit_21_clear = acc & (1 << 21) == 0;
        if upper_nonzero && bit_21_clear {
            state
                .acc
                .add(i, if acc < 0 { 32i64 << 16 } else { -(32i64 << 16) });
        }
        let clamped = clamp_signed(state.acc.signed(i) >> 17);
        state.regs.r[inv.vd][i] = (clamped as u16 & !0xF) as i16;
    }
}

/// `VMADH` — like `VMUDH` but ACCUMULATE: `ACC += (p << 16)` with signed
/// product `p = vs[i] * vt_e[i]` (product into acc_mid:acc_hi), then
/// `vd[i] = clamp_signed(ACC >> 16)`. §6.2.
fn vmadh(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        let p = (vs[i] as i32) * (vt_e[i] as i32); // signed × signed
        let delta = (p as i64) << 16;
        state.acc.add(i, delta);
        state.regs.r[inv.vd][i] = clamp_signed(state.acc.signed(i) >> 16);
    }
}

/// `VMADM` — like `VMUDM` (signed vs × unsigned vt) but ACCUMULATE:
/// `ACC += sign_extend(p)`, `p = (vs[i] as i16) * (vt_e[i] as u16)`, then
/// `vd[i] = clamp_signed(ACC >> 16)`. §6.2.
fn vmadm(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        // vs signed, vt unsigned. Product is signed (vs may be negative).
        let p = (vs[i] as i32) * (vt_e[i] as u16 as i32);
        state.acc.add(i, p as i64); // i32 sign-extends into i64
        state.regs.r[inv.vd][i] = clamp_signed(state.acc.signed(i) >> 16);
    }
}

/// `VMADN` — like `VMUDN` (unsigned vs × signed vt) but ACCUMULATE:
/// `ACC += sign_extend(p)`, `p = (vs[i] as u16) * (vt_e[i] as i16)`, then
/// `vd[i] = clamp_unsigned_low(ACC)`. **The low-part clamp — NOT truncate.**
/// §6.2, §7.
fn vmadn(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        // vs unsigned, vt signed. Product is signed (vt may be negative).
        let p = (vs[i] as u16 as i32) * (vt_e[i] as i32);
        state.acc.add(i, p as i64);
        state.regs.r[inv.vd][i] = clamp_unsigned_low(&state.acc, i) as i16;
    }
}

/// `VMADL` — like `VMUDL` (unsigned × unsigned, `>>16`) but ACCUMULATE:
/// `ACC += (p >> 16)`, `p = (vs[i] as u16) * (vt_e[i] as u16)`, then
/// `vd[i] = clamp_unsigned_low(ACC)` (same unsigned-low clamp as VMADN). §6.2.
fn vmadl(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    for i in 0..8 {
        let p = (vs[i] as u16 as u32) * (vt_e[i] as u16 as u32); // unsigned × unsigned
        let delta = (p >> 16) as i64;
        state.acc.add(i, delta);
        state.regs.r[inv.vd][i] = clamp_unsigned_low(&state.acc, i) as i16;
    }
}

/// `VSAR` — read one accumulator slice into `vd`. `e` selects: `8` → HI
/// (bits 47..32), `9` → MD (bits 31..16), `10` → LO (bits 15..0), any other
/// `e` → 0. Does not modify ACC or any flag; `vs`/`vt` ignored. §6.9.
fn vsar(state: &mut VuState, inv: &OpInvocation) {
    for i in 0..8 {
        let slice = match inv.e {
            8 => state.acc.read_hi(i),
            9 => state.acc.read_mid(i),
            10 => state.acc.read_lo(i),
            _ => 0,
        };
        state.regs.r[inv.vd][i] = slice as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::vu::Accumulator;
    use super::*;

    /// Build an invocation with the common register wiring.
    fn inv(vd: usize, vs: usize, vt: usize, e: usize) -> OpInvocation {
        OpInvocation {
            vd,
            vs,
            vt,
            e,
            ..Default::default()
        }
    }

    // -- VMACF ---------------------------------------------------------------

    #[test]
    fn vmacf_accumulates_p_shl_1_without_round_bias() {
        // Distinguishing input: pick vs*vt so that the (WRONG) VMULF-style
        // `+0x8000` round bias would flip the >>16 result, but the correct
        // VMACF (just `<<1`) does not.
        //
        // vs=vt=0x4000 (=16384). p = 0x1000_0000. p<<1 = 0x2000_0000.
        // Starting ACC = 0. After: ACC = 0x2000_0000. ACC>>16 = 0x2000.
        // If we WRONGLY added +0x8000: ACC = 0x2000_8000, >>16 still 0x2000 —
        // not distinguishing. So choose a value where +0x8000 crosses a
        // >>16 boundary: make ACC just below a boundary so the bias tips it.
        //
        // Set ACC initial = 0x0000_8000 (via a prior add), vs=vt=0 so p=0.
        // Then p<<1 = 0; correct ACC stays 0x8000, >>16 = 0.
        // Wrong (+0x8000) would make 0x1_0000, >>16 = 1. Distinguishable.
        let mut st = VuState::new();
        st.acc.set(0, 0x0000_8000);
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        vmacf(&mut st, &inv(1, 2, 3, 0));
        // Correct: ACC unchanged (added 0), lane0 = clamp(0x8000>>16)=0.
        assert_eq!(
            st.regs.r[1][0], 0,
            "VMACF must NOT add a +0x8000 round bias"
        );
        assert_eq!(st.acc.read_lo(0), 0x8000);
    }

    #[test]
    fn vmacu_accumulates_then_unsigned_clamps() {
        let mut st = VuState::new();
        st.acc.set(0, 0x0001_0000_0000);
        st.acc.set(1, -0x0000_0001_0000);
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        vmacu(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(
            st.regs.r[1][0] as u16, 0xFFFF,
            "positive overflow clamps high"
        );
        assert_eq!(st.regs.r[1][1] as u16, 0, "negative accumulator clamps low");
    }

    #[test]
    fn vmacf_signed_clamps_high_positive() {
        // vs=vt=0x7FFF, twice, to overflow past i16 in acc>>16 and force a
        // signed clamp to 0x7FFF (distinguishes signed clamp from truncate).
        let mut st = VuState::new();
        st.regs.r[2] = [0x7FFF; 8];
        st.regs.r[3] = [0x7FFF; 8];
        // p = 0x7FFF*0x7FFF = 0x3FFF_0001. p<<1 = 0x7FFE_0002.
        // After one MACF: ACC = 0x7FFE_0002; >>16 = 0x7FFE (fits i16).
        vmacf(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 0x7FFE);
        // Accumulate again: ACC = 0xFFFC_0004; >>16 = 0xFFFC as i32 = 0xFFFC
        // -> that's 65532, exceeds i16::MAX, must clamp to 0x7FFF.
        vmacf(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(
            st.regs.r[1][0], 0x7FFF,
            "acc>>16 exceeds i16 range, must signed-clamp not truncate"
        );
    }

    #[test]
    #[should_panic]
    fn vmacf_bug_check_round_bias_would_differ() {
        // Mirror of vmacf_accumulates_p_shl_1: assert the value a +0x8000
        // (VMULF-round) BUG would produce, and confirm the correct impl does
        // NOT produce it (so this panics = the test can tell them apart).
        let mut st = VuState::new();
        st.acc.set(0, 0x0000_8000);
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        vmacf(&mut st, &inv(1, 2, 3, 0));
        // The BUGGY expected value:
        assert_eq!(st.regs.r[1][0], 1);
    }

    // -- VMADH ---------------------------------------------------------------

    #[test]
    fn vmadh_shifts_product_by_16_and_accumulates() {
        let mut st = VuState::new();
        st.regs.r[2] = [3, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [5, 0, 0, 0, 0, 0, 0, 0];
        // p = 15. ACC += 15<<16 = 0xF_0000. >>16 = 0xF.
        vmadh(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 15);
        assert_eq!(st.acc.read_mid(0), 15);
        assert_eq!(st.acc.read_lo(0), 0);
        // Distinguish from a WRONG `<<1` or `<<0` shift: with <<0 the acc_lo
        // would be 15, not 0, and vd would be 0.
        assert_ne!(st.regs.r[1][0], 0);
    }

    #[test]
    fn vmadh_negative_product_signextends() {
        let mut st = VuState::new();
        st.regs.r[2] = [-2, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [3, 0, 0, 0, 0, 0, 0, 0];
        // p = -6. ACC += -6<<16 = -0x6_0000. >>16 = -6.
        vmadh(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], -6);
        assert_eq!(st.acc.signed(0), -0x6_0000);
    }

    // -- VMADM ---------------------------------------------------------------

    #[test]
    fn vmadm_signed_vs_unsigned_vt() {
        let mut st = VuState::new();
        // vs = -1 (signed), vt = 0x8000 (unsigned = 32768).
        st.regs.r[2] = [-1, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [-0x8000i16, 0, 0, 0, 0, 0, 0, 0]; // bits 0x8000 = u16 32768
                                                          // Correct (vt unsigned): p = -1 * 32768 = -32768 = -0x8000.
                                                          // ACC += -0x8000. >>16 = -1 (0xFFFF...). clamp_signed(-1) = -1.
        vmadm(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.acc.signed(0), -0x8000);
        assert_eq!(st.regs.r[1][0], clamp_signed(-0x8000 >> 16));
        assert_eq!(st.regs.r[1][0], -1);
    }

    #[test]
    #[should_panic]
    fn vmadm_bug_check_if_vt_were_signed() {
        // If vt were WRONGLY treated as signed, 0x8000 = -32768, and
        // p = -1 * -32768 = +32768 = +0x8000, giving acc = +0x8000 and a
        // DIFFERENT sign. Assert the buggy result to prove distinguishability.
        let mut st = VuState::new();
        st.regs.r[2] = [-1, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [-0x8000i16, 0, 0, 0, 0, 0, 0, 0];
        vmadm(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.acc.signed(0), 0x8000); // the signed-vt BUG value
    }

    // -- VMADN ---------------------------------------------------------------

    #[test]
    fn vmadn_unsigned_vs_signed_vt_low_clamp() {
        let mut st = VuState::new();
        // vs = 0x8000 (unsigned = 32768), vt = 2 (signed).
        st.regs.r[2] = [-0x8000i16, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [2, 0, 0, 0, 0, 0, 0, 0];
        // Correct (vs unsigned): p = 32768 * 2 = 65536 = 0x1_0000.
        // ACC += 0x1_0000. acc_lo = 0, acc[47..16] = 1 (in range) -> vd = acc_lo = 0.
        vmadn(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.acc.signed(0), 0x1_0000);
        assert_eq!(st.regs.r[1][0], 0);
    }

    #[test]
    fn vmadn_low_clamp_saturates_on_negative_hi_mid() {
        // Set ACC so acc[47..16] is negative -> unsigned-low clamp gives 0,
        // NOT the raw acc_lo (which would be nonzero). This is the MADN-clamps
        // vs MUDN-truncates distinguishing case.
        let mut st = VuState::new();
        st.acc.set(0, -0x1_0000 | 0x1234); // top32 = -1 (negative), lo = 0x1234
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        // p = 0, so ACC unchanged. Correct clamp: acc[47..16] < 0 -> 0x0000.
        vmadn(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(
            st.regs.r[1][0], 0,
            "VMADN clamps (not truncates): negative hi-mid -> 0x0000"
        );
        // The raw acc_lo is 0x1234 — a truncate BUG would return that.
        assert_eq!(st.acc.read_lo(0), 0x1234);
    }

    #[test]
    #[should_panic]
    fn vmadn_bug_check_truncate_would_differ() {
        // Prove the clamp is distinguishable from a truncate: with negative
        // hi-mid, a truncating (VMUDN-style) BUG returns raw acc_lo = 0x1234.
        let mut st = VuState::new();
        st.acc.set(0, -0x1_0000 | 0x1234);
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        vmadn(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 0x1234u16 as i16); // the truncate BUG value
    }

    #[test]
    fn vmadn_low_clamp_saturates_high() {
        // acc[47..16] > 0xFFFF -> clamp to 0xFFFF.
        let mut st = VuState::new();
        st.acc.set(0, 0x1_0000_9999); // top32 = 0x1_0000 (>0xFFFF), lo=0x9999
        st.regs.r[2] = [0; 8];
        st.regs.r[3] = [0; 8];
        vmadn(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 0xFFFFu16 as i16);
    }

    // -- VMADL ---------------------------------------------------------------

    #[test]
    fn vmadl_unsigned_unsigned_shift16_low_clamp() {
        let mut st = VuState::new();
        // vs = 0xFFFF, vt = 0xFFFF (both unsigned = 65535).
        st.regs.r[2] = [-1i16, 0, 0, 0, 0, 0, 0, 0]; // 0xFFFF
        st.regs.r[3] = [-1i16, 0, 0, 0, 0, 0, 0, 0]; // 0xFFFF
                                                     // p = 65535*65535 = 0xFFFE_0001. p>>16 = 0xFFFE.
                                                     // ACC += 0xFFFE. acc[47..16] = 0 (in range) -> vd = acc_lo = 0xFFFE.
        vmadl(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.acc.read_lo(0), 0xFFFE);
        assert_eq!(st.regs.r[1][0], 0xFFFEu16 as i16);
    }

    #[test]
    #[should_panic]
    fn vmadl_bug_check_signed_operands_would_differ() {
        // If operands were WRONGLY signed: -1 * -1 = 1, p>>16 = 0, acc_lo = 0.
        // Assert the buggy value to prove the unsigned path is distinguishable.
        let mut st = VuState::new();
        st.regs.r[2] = [-1i16, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[3] = [-1i16, 0, 0, 0, 0, 0, 0, 0];
        vmadl(&mut st, &inv(1, 2, 3, 0));
        assert_eq!(st.regs.r[1][0], 0); // the signed-operand BUG value
    }

    // -- VSAR ----------------------------------------------------------------

    #[test]
    fn vsar_selects_correct_slice_by_e() {
        let mut st = VuState::new();
        for i in 0..8 {
            st.acc.set(i, 0x1234_5678_9ABC + i as i64);
        }
        // e=8 -> HI (bits 47..32) = 0x1234
        vsar(&mut st, &inv(1, 0, 0, 8));
        assert_eq!(st.regs.r[1][0], 0x1234u16 as i16);
        // e=9 -> MD (bits 31..16) = 0x5678
        vsar(&mut st, &inv(1, 0, 0, 9));
        assert_eq!(st.regs.r[1][0], 0x5678u16 as i16);
        // e=10 -> LO (bits 15..0) = 0x9ABC (+i for later lanes)
        vsar(&mut st, &inv(1, 0, 0, 10));
        assert_eq!(st.regs.r[1][0], 0x9ABCu16 as i16);
        assert_eq!(st.regs.r[1][1], 0x9ABDu16 as i16);
        // other e -> 0
        vsar(&mut st, &inv(1, 0, 0, 0));
        assert_eq!(st.regs.r[1][0], 0);
    }

    #[test]
    fn vsar_does_not_modify_acc_or_flags() {
        let mut st = VuState::new();
        st.acc.set(3, 0x0BAD_F00D_CAFE);
        let acc_before = st.acc;
        let flags_before = st.flags;
        vsar(&mut st, &inv(1, 0, 0, 9));
        assert_eq!(st.acc, acc_before, "VSAR must not modify ACC");
        assert_eq!(st.flags, flags_before, "VSAR must not modify flags");
    }

    #[test]
    #[should_panic]
    fn vsar_bug_check_wrong_slice_would_differ() {
        // Prove HI (e=8) and MD (e=9) give distinguishable values so a
        // wrong-slice BUG is caught: assert HI's value equals MD's (false).
        let mut acc = Accumulator::default();
        acc.set(0, 0x1234_5678_9ABC);
        assert_eq!(acc.read_hi(0), acc.read_mid(0)); // 0x1234 != 0x5678
    }

    // -- VMACQ ---------------------------------------------------------------

    #[test]
    fn vmacq_clears_low_nibble_of_result() {
        let mut st = VuState::new();
        // VMACQ extracts ACC bits 32..17, then clears the low nibble.
        st.acc.set(0, 0x1237_0000);
        vmacq(&mut st, &inv(1, 0, 0, 0));
        assert_eq!(
            st.regs.r[1][0] as u16 & 0xF,
            0,
            "VMACQ must clear the low nibble of the result"
        );
        assert_eq!(st.regs.r[1][0], 0x0910u16 as i16);
    }

    #[test]
    fn vmacq_oddifies_at_accumulator_bit_21() {
        let mut positive = VuState::new();
        positive.acc.set(0, 0x0040_0000); // positive, bit 21 clear
        vmacq(&mut positive, &inv(1, 0, 0, 0));
        assert_eq!(positive.acc.signed(0), 0x0020_0000);
        assert_eq!(positive.regs.r[1][0], 0x0010);

        let mut negative = VuState::new();
        negative.acc.set(0, -0x0040_0000); // negative, bit 21 clear
        vmacq(&mut negative, &inv(1, 0, 0, 0));
        assert_eq!(negative.acc.signed(0), -0x0020_0000);
        assert_eq!(negative.regs.r[1][0], -0x0010);
    }

    #[test]
    fn vmacq_element_field_is_ignored() {
        // VMACQ takes no vs/vt and ignores e; two different e values must
        // produce the same result for the same ACC.
        let mut a = VuState::new();
        let mut b = VuState::new();
        a.acc.set(0, 0x0055_0000);
        b.acc.set(0, 0x0055_0000);
        vmacq(&mut a, &inv(1, 0, 0, 0));
        vmacq(&mut b, &inv(1, 0, 0, 12));
        assert_eq!(a.regs.r[1], b.regs.r[1]);
    }
}
