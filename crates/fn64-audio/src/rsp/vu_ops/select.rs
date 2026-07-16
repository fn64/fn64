//! The RSP VU "select" op family: the signed/equality compares (`VLT`, `VEQ`,
//! `VNE`, `VGE`), the VCC-driven merge (`VMRG`), the clip ops (`VCH`, `VCL`,
//! `VCR`), and the named VCC/VCO/VCE register-slice accessors
//! (`VCCL`/`VCCH`/`VCOL`/`VCOH`/`VCE`).
//!
//! Implemented per `RSP-VU-ISA.md` §6.6 (compares + merge), §6.7 (clip), and
//! §6.8 (the register-slice names). Everything is portable scalar Rust over the
//! `[i16; 8]` lane model and the `VuState`/`Accumulator`/`Flags` API in
//! [`crate::rsp::vu`]; no SIMD, no GPL implementation read.
//!
//! ## Shared skeleton (compares + merge, §6.6)
//!
//! Each compare sets `VCC.low[i]` per lane, writes the selected source into
//! both `vd[i]` and `acc_lo[i]`, and — for `VLT/VEQ/VNE/VGE` — clears both VCO
//! bytes for the lane afterward. `VCC.high[i]` is forced to 0 by the compares.
//! `VMRG` selects by the existing `VCC.low` and touches no flags.
//!
//! ## Clip skeleton (§6.7)
//!
//! `VCH` sets VCO(carry+ne), both VCC bytes, and VCE in one shot with a
//! sign-dependent branch and the `s == -t-1` extension. `VCL` consumes the
//! flags VCH left and refines the result via an unsigned 16-bit compare plus the
//! VCE extension, then clears VCO and VCE. `VCR` is a one-shot signed-range clip
//! (note the `~t` = `-t-1` ones-complement selection) that also clears VCO/VCE.
//! These are the subtlest ops in the ISA (§7); each branch is pinned by a
//! dedicated test below.

use crate::rsp::ops::{OpInvocation, OpStatus, VuOp};
use crate::rsp::vu::{element_select, Flags, VuState};

// ---------------------------------------------------------------------------
// §6.8 — VCC/VCO/VCE register-slice named accessors
//
// These are NOT distinct RSP opcodes; per RSP-VU-ISA.md §6.8 the `-L`/`-H`
// suffixes just name the low/high byte of VCC/VCO (and VCE the whole 8-bit
// extension register). They carry no per-lane arithmetic — they are byte-level
// read/write accessors onto the `Flags` struct. `CFC2`/`CTC2` (the scalar-side
// control-register moves) use these to read/write the flag registers.
// ---------------------------------------------------------------------------

/// The VCO low byte — the 8 carry/borrow bits (one per lane). "VCOL".
#[inline]
pub fn read_vcol(flags: &Flags) -> u8 {
    (flags.vco & 0xFF) as u8
}
/// Write the VCO low byte (carry/borrow bits), leaving the ne byte intact.
#[inline]
pub fn write_vcol(flags: &mut Flags, value: u8) {
    flags.vco = (flags.vco & 0xFF00) | value as u16;
}

/// The VCO high byte — the 8 "not-equal" (ne) bits (one per lane). "VCOH".
#[inline]
pub fn read_vcoh(flags: &Flags) -> u8 {
    (flags.vco >> 8) as u8
}
/// Write the VCO high byte (ne bits), leaving the carry byte intact.
#[inline]
pub fn write_vcoh(flags: &mut Flags, value: u8) {
    flags.vco = (flags.vco & 0x00FF) | ((value as u16) << 8);
}

/// The VCC low byte — the 8 primary compare/clip-low bits. "VCCL".
#[inline]
pub fn read_vccl(flags: &Flags) -> u8 {
    (flags.vcc & 0xFF) as u8
}
/// Write the VCC low byte (primary compare/clip-low), leaving the high byte.
#[inline]
pub fn write_vccl(flags: &mut Flags, value: u8) {
    flags.vcc = (flags.vcc & 0xFF00) | value as u16;
}

/// The VCC high byte — the 8 secondary compare/clip-high bits. "VCCH".
#[inline]
pub fn read_vcch(flags: &Flags) -> u8 {
    (flags.vcc >> 8) as u8
}
/// Write the VCC high byte (secondary compare/clip-high), leaving the low byte.
#[inline]
pub fn write_vcch(flags: &mut Flags, value: u8) {
    flags.vcc = (flags.vcc & 0x00FF) | ((value as u16) << 8);
}

/// The whole 8-bit VCE (compare-extension) register. "VCE".
#[inline]
pub fn read_vce(flags: &Flags) -> u8 {
    flags.vce
}
/// Write the whole 8-bit VCE register.
#[inline]
pub fn write_vce(flags: &mut Flags, value: u8) {
    flags.vce = value;
}

// ---------------------------------------------------------------------------
// §6.6 — Compares (VLT/VEQ/VNE/VGE) and merge (VMRG)
// ---------------------------------------------------------------------------

/// Shared body for the four compares: compute `cond` per lane, set `VCC.low`,
/// clear `VCC.high`, write `cond ? vs : vt_e` into `vd` and `acc_lo`, and clear
/// both VCO bytes for the lane. `cond_fn` receives `(vs, vt_e, eq, ne, carry)`.
#[inline]
fn compare<F>(state: &mut VuState, inv: &OpInvocation, cond_fn: F)
where
    F: Fn(i16, i16, bool, bool, bool) -> bool,
{
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let a = vs[i];
        let b = vt_e[i];
        let eq = a == b;
        let ne = state.flags.vco_ne(i);
        let carry = state.flags.vco_carry(i);
        let cond = cond_fn(a, b, eq, ne, carry);
        let result = if cond { a } else { b };
        vd[i] = result;
        state.acc.write_lo(i, result as u16);
        state.flags.set_vcc_low(i, cond);
        state.flags.set_vcc_high(i, false);
        // Compares consume and then clear both VCO bytes for the lane.
        state.flags.clear_vco_lane(i);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VLT` — set-if-less-than (signed). §6.6.
/// `cond = (vs < vt) || (eq && ne && carry)` — the equal-but-flagged case
/// reproduces the RSP's exact behavior after a preceding `VSUBC`.
pub fn vlt(state: &mut VuState, inv: &OpInvocation) {
    compare(state, inv, |a, b, eq, ne, carry| {
        a < b || (eq && ne && carry)
    });
}

/// `VEQ` — set-if-equal. §6.6. `cond = eq && !ne`.
pub fn veq(state: &mut VuState, inv: &OpInvocation) {
    compare(state, inv, |_, _, eq, ne, _| eq && !ne);
}

/// `VNE` — set-if-not-equal. §6.6. `cond = !eq || ne`.
pub fn vne(state: &mut VuState, inv: &OpInvocation) {
    compare(state, inv, |_, _, eq, ne, _| !eq || ne);
}

/// `VGE` — set-if-greater-or-equal (signed). §6.6.
/// `cond = (vs > vt) || (eq && !(ne && carry))`.
pub fn vge(state: &mut VuState, inv: &OpInvocation) {
    compare(state, inv, |a, b, eq, ne, carry| {
        a > b || (eq && !(ne && carry))
    });
}

/// `VMRG` — merge/select by `VCC.low`. §6.6.
/// `vd[i] = acc_lo[i] = VCC.low[i] ? vs[i] : vt_e[i]`. Touches no flags.
pub fn vmrg(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let result = if state.flags.vcc_low(i) {
            vs[i]
        } else {
            vt_e[i]
        };
        vd[i] = result;
        state.acc.write_lo(i, result as u16);
    }
    state.regs.r[inv.vd] = vd;
}

// ---------------------------------------------------------------------------
// §6.7 — Clip ops (VCH / VCL / VCR)
//
// These follow the documented RSP clip algorithm. `s`/`t` are the signed 16-bit
// operands; the sign-dependent branch is keyed on whether the operands have
// opposite signs (`(s ^ t) < 0`). The intermediate sums are computed in `i32`
// so the ±0x8000 boundary cases never overflow.
// ---------------------------------------------------------------------------

/// `VCH` — clip, high half. Sets VCO(carry+ne), both VCC bytes, and VCE. §6.7.
pub fn vch(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let s = vs[i] as i32;
        let t = vt_e[i] as i32;
        let opposite = (s ^ t) < 0; // operands have opposite signs
        let result: i32 = if opposite {
            // carry set; the "clip to -t" branch.
            state.flags.set_vco_carry(i, true);
            let vce = s == (-t - 1);
            state.flags.set_vce(i, vce);
            // ne set unless s == -t or s == -t-1 (the two exact-boundary cases).
            let ne = !(s == -t || s == (-t - 1));
            state.flags.set_vco_ne(i, ne);
            let clip_low = (s + t) <= 0;
            state.flags.set_vcc_low(i, clip_low);
            state.flags.set_vcc_high(i, t < 0);
            if clip_low {
                -t
            } else {
                s
            }
        } else {
            // same signs; carry clear, VCE clear, "clip to t" branch.
            state.flags.set_vco_carry(i, false);
            state.flags.set_vce(i, false);
            let ne = s != t;
            state.flags.set_vco_ne(i, ne);
            let clip_high = (s - t) >= 0;
            state.flags.set_vcc_high(i, clip_high);
            state.flags.set_vcc_low(i, t < 0);
            if clip_high {
                t
            } else {
                s
            }
        };
        let r16 = result as i16;
        vd[i] = r16;
        state.acc.write_lo(i, r16 as u16);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VCL` — clip, low half. Consumes the VCO/VCC/VCE state a prior `VCH` left,
/// refines VCC via an unsigned 16-bit compare + the VCE extension, then clears
/// VCO and VCE. §6.7 / §7.
pub fn vcl(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let s = vs[i];
        let t = vt_e[i];
        let su = s as u16;
        let tu = t as u16;
        let carry = state.flags.vco_carry(i);
        let ne = state.flags.vco_ne(i);
        let vce = state.flags.vce(i);

        let result: i16 = if carry {
            // Opposite-sign path from VCH: decides "clip to -t" via unsigned
            // compare of the low 16 bits, extended by VCE for the -t-1 boundary.
            if !ne {
                // ge = (u16)s >= (u16)(-t); the sum s+t as a 17-bit unsigned add
                // carrying out tells us s >= -t (unsigned). Recompute directly.
                let sum = su as u32 + tu as u32; // low-16 unsigned add
                let low = sum as u16;
                let sum_zero = low == 0;
                // The hardware comparison is equivalent to comparing the
                // wrapping sum with unsigned-saturating(s+t). This is true
                // without carry and for the special 0x1ffff sum, but false for
                // the other overflowing sums. VCE selects OR versus AND with
                // the zero test (Programmer's Guide pp. 72-73; independently
                // cross-checked against CEN64's vector algorithm).
                let no_carry = sum <= 0xFFFF;
                let new_low = if vce {
                    sum_zero || no_carry
                } else {
                    sum_zero && no_carry
                };
                state.flags.set_vcc_low(i, new_low);
            }
            // ne set: keep VCC.low as VCH left it.
            let clip_low = state.flags.vcc_low(i);
            if clip_low {
                (-(t as i32)) as i16
            } else {
                s
            }
        } else {
            // Same-sign path from VCH: decides "clip to t" via unsigned compare.
            if !ne {
                let ge = su >= tu;
                state.flags.set_vcc_high(i, ge);
            }
            // ne set: keep VCC.high unchanged.
            let clip_high = state.flags.vcc_high(i);
            if clip_high {
                t
            } else {
                s
            }
        };
        vd[i] = result;
        state.acc.write_lo(i, result as u16);
        // VCL clears VCO and VCE afterward.
        state.flags.clear_vco_lane(i);
        state.flags.set_vce(i, false);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VCR` — clip range (one-shot signed; no VCE, no VCO). §6.7.
/// Note the `~t` (`-t-1`, ones-complement) selection — distinct from VCH's
/// `-t`. Sets both VCC bytes; clears VCO and VCE.
pub fn vcr(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; 8];
    for i in 0..8 {
        let s = vs[i] as i32;
        let t = vt_e[i] as i32;
        let opposite = (s ^ t) < 0;
        let result: i32 = if opposite {
            let clip_low = (s + t + 1) <= 0; // s <= -t - 1
            state.flags.set_vcc_low(i, clip_low);
            state.flags.set_vcc_high(i, t < 0);
            if clip_low {
                !t // ~t = -t-1
            } else {
                s
            }
        } else {
            let clip_high = (s - t) >= 0;
            state.flags.set_vcc_high(i, clip_high);
            state.flags.set_vcc_low(i, t < 0);
            if clip_high {
                t
            } else {
                s
            }
        };
        let r16 = result as i16;
        vd[i] = r16;
        state.acc.write_lo(i, r16 as u16);
        // VCR clears VCO and VCE.
        state.flags.clear_vco_lane(i);
        state.flags.set_vce(i, false);
    }
    state.regs.r[inv.vd] = vd;
}

/// Dispatch the "select" op family. Returns `Some(Executed)` for the ops this
/// module owns (`Vlt/Veq/Vne/Vge/Vmrg/Vch/Vcl/Vcr`), `None` for any other op so
/// the central dispatcher can try the next family. Wired into
/// [`crate::rsp::ops::dispatch`] for this group's opcodes only.
pub fn try_dispatch(state: &mut VuState, op: VuOp, inv: &OpInvocation) -> Option<OpStatus> {
    match op {
        VuOp::Vlt => vlt(state, inv),
        VuOp::Veq => veq(state, inv),
        VuOp::Vne => vne(state, inv),
        VuOp::Vge => vge(state, inv),
        VuOp::Vmrg => vmrg(state, inv),
        VuOp::Vch => vch(state, inv),
        VuOp::Vcl => vcl(state, inv),
        VuOp::Vcr => vcr(state, inv),
        _ => return None,
    }
    Some(OpStatus::Executed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsp::vu::VuState;

    fn inv(vd: usize, vs: usize, vt: usize) -> OpInvocation {
        OpInvocation {
            vd,
            vs,
            vt,
            e: 0,
            de: 0,
            vs_index: 0,
        }
    }

    // ---- §6.8 register-slice accessors ----

    #[test]
    fn vco_vcc_byte_accessors_roundtrip_and_are_independent() {
        let mut f = Flags::default();
        write_vcol(&mut f, 0xA5);
        write_vcoh(&mut f, 0x3C);
        write_vccl(&mut f, 0x0F);
        write_vcch(&mut f, 0xF0);
        write_vce(&mut f, 0x99);
        assert_eq!(read_vcol(&f), 0xA5);
        assert_eq!(read_vcoh(&f), 0x3C);
        assert_eq!(read_vccl(&f), 0x0F);
        assert_eq!(read_vcch(&f), 0xF0);
        assert_eq!(read_vce(&f), 0x99);
        // Writing the low byte must not disturb the high byte and vice-versa.
        write_vcol(&mut f, 0x11);
        assert_eq!(read_vcoh(&f), 0x3C, "VCOL write clobbered VCOH");
        write_vcch(&mut f, 0x22);
        assert_eq!(read_vccl(&f), 0x0F, "VCCH write clobbered VCCL");
    }

    // ---- §6.6 compares ----

    #[test]
    fn vlt_signed_less_than_and_selects_source() {
        // Lane 0: vs<vt -> cond true, picks vs. Lane 1: vs>vt -> false, picks vt.
        // Lane 2: signed-ness matters: -1 (0xFFFF) < 1, must be treated signed.
        let mut st = VuState::new();
        st.regs.r[1] = [3, 9, -1, 0, 0, 0, 0, 0]; // vs
        st.regs.r[2] = [5, 4, 1, 0, 0, 0, 0, 0]; // vt
        vlt(&mut st, &inv(3, 1, 2));
        assert_eq!(st.regs.r[3][0], 3, "3<5: pick vs");
        assert!(st.flags.vcc_low(0));
        assert_eq!(st.regs.r[3][1], 4, "9<4 false: pick vt");
        assert!(!st.flags.vcc_low(1));
        // If -1 were compared UNSIGNED (0xFFFF), it would be > 1 and cond false;
        // signed it is < 1 so cond true and picks vs (-1). This distinguishes
        // the signed compare from an unsigned one.
        assert_eq!(st.regs.r[3][2], -1, "signed -1 < 1: pick vs");
        assert!(st.flags.vcc_low(2), "signed compare, not unsigned");
        // acc_lo mirrors the result.
        assert_eq!(st.acc.read_lo(2), 0xFFFF);
    }

    #[test]
    fn vlt_equal_but_flagged_case_uses_vco() {
        // vs == vt. With ne+carry set, VLT's cond is true (picks vs); without,
        // cond is false (picks vt). Distinguishes the (eq && ne && carry) term.
        let mut st = VuState::new();
        st.regs.r[1] = [7, 7, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [7, 7, 0, 0, 0, 0, 0, 0];
        // Lane 0: ne+carry set -> cond true.
        st.flags.set_vco_ne(0, true);
        st.flags.set_vco_carry(0, true);
        // Lane 1: only ne set (no carry) -> cond false.
        st.flags.set_vco_ne(1, true);
        vlt(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_low(0), "eq&&ne&&carry -> true");
        assert!(!st.flags.vcc_low(1), "eq&&ne&&!carry -> false");
        // VCO must be cleared afterward.
        assert!(!st.flags.vco_ne(0));
        assert!(!st.flags.vco_carry(0));
    }

    #[test]
    fn veq_uses_ne_flag() {
        // Equal lanes: cond = eq && !ne. Lane with ne set must NOT compare equal.
        let mut st = VuState::new();
        st.regs.r[1] = [5, 5, 6, 0, 0, 0, 0, 0];
        st.regs.r[2] = [5, 5, 7, 0, 0, 0, 0, 0];
        st.flags.set_vco_ne(1, true); // lane1 equal but ne set
        veq(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_low(0), "equal, no ne -> true");
        assert!(!st.flags.vcc_low(1), "equal but ne -> false");
        assert!(!st.flags.vcc_low(2), "not equal -> false");
    }

    #[test]
    fn vne_uses_ne_flag() {
        // cond = !eq || ne. Equal lane with ne set is TRUE; equal without is false.
        let mut st = VuState::new();
        st.regs.r[1] = [5, 5, 6, 0, 0, 0, 0, 0];
        st.regs.r[2] = [5, 5, 7, 0, 0, 0, 0, 0];
        st.flags.set_vco_ne(1, true);
        vne(&mut st, &inv(3, 1, 2));
        assert!(!st.flags.vcc_low(0), "equal, no ne -> false");
        assert!(st.flags.vcc_low(1), "equal but ne -> true");
        assert!(st.flags.vcc_low(2), "not equal -> true");
    }

    #[test]
    fn vge_signed_greater_or_equal() {
        let mut st = VuState::new();
        // Lane0: 9>=4 true. Lane1: 2>=8 false. Lane2: equal with ne&carry ->
        // cond = eq && !(ne&&carry) = false.
        st.regs.r[1] = [9, 2, 5, 0, 0, 0, 0, 0];
        st.regs.r[2] = [4, 8, 5, 0, 0, 0, 0, 0];
        st.flags.set_vco_ne(2, true);
        st.flags.set_vco_carry(2, true);
        vge(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_low(0));
        assert_eq!(st.regs.r[3][0], 9);
        assert!(!st.flags.vcc_low(1));
        assert_eq!(st.regs.r[3][1], 8);
        assert!(!st.flags.vcc_low(2), "equal but ne&&carry -> GE false");
    }

    #[test]
    fn vmrg_selects_by_vcc_low_without_touching_flags() {
        let mut st = VuState::new();
        st.regs.r[1] = [10, 11, 12, 13, 14, 15, 16, 17]; // vs
        st.regs.r[2] = [20, 21, 22, 23, 24, 25, 26, 27]; // vt
                                                         // VCC.low pattern: pick vs on even lanes.
        for i in 0..8 {
            st.flags.set_vcc_low(i, i % 2 == 0);
        }
        let vcc_before = st.flags.vcc;
        let vco_before = st.flags.vco;
        vmrg(&mut st, &inv(3, 1, 2));
        assert_eq!(st.regs.r[3], [10, 21, 12, 23, 14, 25, 16, 27]);
        assert_eq!(st.acc.read_lo(0), 10);
        assert_eq!(st.acc.read_lo(1), 21);
        assert_eq!(st.flags.vcc, vcc_before, "VMRG must not change VCC");
        assert_eq!(st.flags.vco, vco_before, "VMRG must not change VCO");
    }

    #[test]
    fn compares_apply_element_modifier() {
        // e=8 broadcasts vt lane 0. VEQ vs=[5,7,..] vt lane0=5.
        let mut st = VuState::new();
        st.regs.r[1] = [5, 7, 5, 0, 0, 0, 0, 0];
        st.regs.r[2] = [5, 9, 9, 0, 0, 0, 0, 0];
        let mut i = inv(3, 1, 2);
        i.e = 8; // broadcast vt[0] = 5 to all lanes
        veq(&mut st, &i);
        assert!(st.flags.vcc_low(0), "5==5");
        assert!(!st.flags.vcc_low(1), "7!=5");
        assert!(st.flags.vcc_low(2), "5==5 (broadcast)");
    }

    // ---- §6.7 clip ops ----

    #[test]
    fn vch_opposite_sign_branch() {
        // s=3, t=-5 (opposite signs). sign path:
        //   carry=1; vce=(s==-t-1)=(3==4)=false; ne=!(s==-t||s==-t-1)=!(3==5||3==4)=true
        //   clip_low=(s+t<=0)=(-2<=0)=true -> vd=-t=5; vcc_high=(t<0)=true
        let mut st = VuState::new();
        st.regs.r[1] = [3, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [-5i16, 0, 0, 0, 0, 0, 0, 0];
        vch(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vco_carry(0), "opposite signs -> carry");
        assert!(!st.flags.vce(0));
        assert!(st.flags.vco_ne(0));
        assert!(st.flags.vcc_low(0), "s+t<=0");
        assert!(st.flags.vcc_high(0), "t<0");
        assert_eq!(st.regs.r[3][0], 5, "clip to -t");
    }

    #[test]
    fn vch_vce_extension_boundary() {
        // s=4, t=-5: s == -t-1 (4 == 4) -> vce true, ne false, clip_low(s+t=-1<=0)=true,
        // vd = -t = 5.
        let mut st = VuState::new();
        st.regs.r[1] = [4, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [-5i16, 0, 0, 0, 0, 0, 0, 0];
        vch(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vce(0), "s==-t-1 -> VCE");
        assert!(!st.flags.vco_ne(0), "boundary -> ne clear");
        assert_eq!(st.regs.r[3][0], 5);
    }

    #[test]
    fn vch_same_sign_branch() {
        // s=7, t=3 (same sign). same-sign path:
        //   carry=0; vce=0; ne=(s!=t)=true; clip_high=(s-t>=0)=(4>=0)=true -> vd=t=3
        //   vcc_low=(t<0)=false
        let mut st = VuState::new();
        st.regs.r[1] = [7, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [3, 0, 0, 0, 0, 0, 0, 0];
        vch(&mut st, &inv(3, 1, 2));
        assert!(!st.flags.vco_carry(0), "same sign -> no carry");
        assert!(!st.flags.vce(0));
        assert!(st.flags.vco_ne(0));
        assert!(st.flags.vcc_high(0), "s-t>=0");
        assert!(!st.flags.vcc_low(0), "t>=0");
        assert_eq!(st.regs.r[3][0], 3, "clip to t");
    }

    #[test]
    fn vcl_same_sign_unsigned_compare() {
        // Prime a same-sign VCH (carry clear), then VCL refines VCC.high via
        // UNSIGNED compare. s=0x0002, t=0x0003 -> (u16)s < (u16)t -> ge false ->
        // vd = s. Flip clip via unsigned boundary distinguishes signed vs unsigned.
        let mut st = VuState::new();
        st.regs.r[1] = [2, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [3, 0, 0, 0, 0, 0, 0, 0];
        // Emulate VCH same-sign leftovers: carry=0, ne=0 (so VCL recomputes).
        st.flags.set_vco_carry(0, false);
        st.flags.set_vco_ne(0, false);
        vcl(&mut st, &inv(3, 1, 2));
        assert!(!st.flags.vcc_high(0), "2 >= 3 unsigned is false");
        assert_eq!(st.regs.r[3][0], 2, "clip_high false -> s");
        // VCO/VCE cleared.
        assert!(!st.flags.vco_carry(0));
        assert!(!st.flags.vce(0));
    }

    #[test]
    fn vcl_ne_set_keeps_prior_vcc() {
        // carry clear, ne SET: VCL must keep VCC.high as-is, not recompute.
        // Set vcc_high true beforehand; s(=5) >= t(=2) unsigned is also true, so
        // to prove "keep, not recompute" set a case where recompute would DIFFER:
        // s=2,t=5 -> recompute would give false, but ne-set must keep prior true.
        let mut st = VuState::new();
        st.regs.r[1] = [2, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [5, 0, 0, 0, 0, 0, 0, 0];
        st.flags.set_vco_carry(0, false);
        st.flags.set_vco_ne(0, true);
        st.flags.set_vcc_high(0, true); // prior VCH decision
        vcl(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_high(0), "ne set -> keep prior VCC.high");
        assert_eq!(st.regs.r[3][0], 5, "clip_high kept true -> t");
    }

    #[test]
    fn vcl_opposite_sign_uses_vce_extension() {
        // carry set (opposite sign), ne clear. s=0x0003, t=0xFFFD (=-3).
        // sum = 3 + 0xFFFD = 0x10000 -> carry_out=1, sum_zero=(low16==0)=true.
        // Without VCE: new_low = sum_zero && !carry_out = true && false = false.
        // With VCE:    new_low = sum_zero || carry_out = true. This is exactly
        // where the VCE extension flips the clip decision.
        let mut st_novce = VuState::new();
        st_novce.regs.r[1] = [3, 0, 0, 0, 0, 0, 0, 0];
        st_novce.regs.r[2] = [-3i16, 0, 0, 0, 0, 0, 0, 0];
        st_novce.flags.set_vco_carry(0, true);
        st_novce.flags.set_vco_ne(0, false);
        st_novce.flags.set_vce(0, false);
        vcl(&mut st_novce, &inv(3, 1, 2));
        assert!(!st_novce.flags.vcc_low(0), "no VCE -> clip_low false");
        assert_eq!(st_novce.regs.r[3][0], 3, "clip_low false -> s");

        let mut st_vce = VuState::new();
        st_vce.regs.r[1] = [3, 0, 0, 0, 0, 0, 0, 0];
        st_vce.regs.r[2] = [-3i16, 0, 0, 0, 0, 0, 0, 0];
        st_vce.flags.set_vco_carry(0, true);
        st_vce.flags.set_vco_ne(0, false);
        st_vce.flags.set_vce(0, true);
        vcl(&mut st_vce, &inv(3, 1, 2));
        assert!(st_vce.flags.vcc_low(0), "VCE -> clip_low true");
        assert_eq!(st_vce.regs.r[3][0], 3, "clip to -t = 3");
    }

    #[test]
    fn vcl_vce_distinguishes_wrapped_zero_from_next_sum() {
        let mut st = VuState::new();
        st.regs.r[1] = [0x7FFF, 0x7FFF, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [-0x7FFF, -0x7FFE, 0, 0, 0, 0, 0, 0];
        for lane in 0..2 {
            st.flags.set_vco_carry(lane, true);
            st.flags.set_vco_ne(lane, false);
            st.flags.set_vce(lane, true);
        }
        vcl(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_low(0), "0x7fff + 0x8001 wraps exactly to zero");
        assert!(!st.flags.vcc_low(1), "0x7fff + 0x8002 wraps to one");
        assert_eq!(st.regs.r[3][0], 0x7FFF);
        assert_eq!(st.regs.r[3][1], 0x7FFF);
    }

    #[test]
    fn vcr_uses_ones_complement_selection() {
        // Opposite sign: s=-10, t=5. clip_low = (s+t+1<=0) = (-4<=0) = true ->
        // vd = ~t = ~5 = -6 (NOT -t = -5). This distinguishes VCR's ~t from
        // VCH's -t: with -t the result would be -5.
        let mut st = VuState::new();
        st.regs.r[1] = [-10i16, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [5, 0, 0, 0, 0, 0, 0, 0];
        vcr(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_low(0), "s<=-t-1");
        assert_eq!(st.regs.r[3][0], -6, "VCR selects ~t = -t-1, not -t");
        assert!(!st.flags.vcc_high(0), "vcc_high = (t < 0); t=5 so false");
    }

    #[test]
    fn vcr_same_sign_branch_and_clears_vco() {
        // Same sign: s=8, t=3. clip_high = (s-t>=0)=true -> vd=t=3. VCO/VCE cleared.
        let mut st = VuState::new();
        st.regs.r[1] = [8, 0, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [3, 0, 0, 0, 0, 0, 0, 0];
        st.flags.set_vco_carry(0, true); // should be cleared by VCR
        st.flags.set_vce(0, true);
        vcr(&mut st, &inv(3, 1, 2));
        assert!(st.flags.vcc_high(0));
        assert_eq!(st.regs.r[3][0], 3, "clip to t");
        assert!(!st.flags.vco_carry(0), "VCR clears VCO");
        assert!(!st.flags.vce(0), "VCR clears VCE");
    }

    // ---- guard: confirm the tests FAIL against the wrong mode (no green-on-bug) ----

    #[test]
    fn guard_vcr_ones_complement_vs_negate_are_distinguishable() {
        // Prove ~t (=-6) and -t (=-5) differ for the chosen inputs, so the
        // vcr_uses_ones_complement_selection test would fail if VCR used -t.
        let t: i16 = 5;
        assert_ne!(!t, -t, "test inputs must distinguish ~t from -t");
        assert_eq!(!t, -6);
        assert_eq!(-t, -5);
    }

    #[test]
    fn guard_vlt_signed_vs_unsigned_distinguishable() {
        // -1 as signed < 1, as unsigned (0xFFFF) > 1 — the lane-2 assertion in
        // vlt_signed_less_than_and_selects_source would flip if compared unsigned.
        let a: i16 = -1;
        let b: i16 = 1;
        assert!(a < b, "signed");
        assert!((a as u16) > (b as u16), "unsigned differs");
    }
}
