//! The "mul-hi" RSP VU multiply family that SETS the accumulator:
//! `VMULF`, `VMULQ`, `VMUDH`, `VMUDM`, `VMUDN`, `VMUDL`.
//!
//! These all overwrite the 48-bit accumulator with a freshly-computed product
//! (contrast the `VMACx`/`VMADx` accumulate family, which add into it), then
//! extract the destination lane from a specific accumulator slice with a
//! specific clamp mode. See `RSP-VU-ISA.md` §6.1 and §7.
//!
//! Every op here is portable scalar Rust: `i16` source lanes, `i32`/`i64`
//! intermediates for the multiply and the 48-bit accumulator, no SIMD. The
//! per-op math (operand signedness, pre-add shift/round, which ACC slice the
//! result comes from, and the clamp mode) is exactly the part that must be
//! bit-exact; each op has a dedicated fail-against-reference test that would
//! turn red if the clamp mode or the shift were swapped.

use crate::rsp::ops::OpInvocation;
use crate::rsp::vu::{clamp_signed, clamp_unsigned, element_select, VuState, LANES};

/// `VMULF` — signed fractional multiply, round, signed-clamp (§6.1).
///
/// `p = vs[i] * vt_e[i]` (signed×signed, 32-bit). `ACC = (p << 1) + 0x8000`
/// (the `<<1` is the `.15 × .15 → .15` fixed-point scale, `+0x8000` is
/// round-to-nearest at bit 15). `vd[i] = clamp_signed(ACC >> 16)`.
///
/// The `-1.0 × -1.0` case (`vs = vt = 0x8000`) naturally rounds-and-clamps to
/// `0x7FFF` rather than wrapping.
pub fn vmulf(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        let p = vs[i] as i32 as i64 * vt_e[i] as i32 as i64;
        let acc_val = (p << 1) + 0x8000;
        state.acc.set(i, acc_val);
        vd[i] = clamp_signed(state.acc.signed(i) >> 16);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMULU` — the same signed fractional product and rounding as `VMULF`,
/// extracted with the RSP's unsigned clamp (Programmer's Guide, Table 3-4).
pub fn vmulu(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        let p = vs[i] as i32 as i64 * vt_e[i] as i32 as i64;
        let acc_val = (p << 1) + 0x8000;
        state.acc.set(i, acc_val);
        vd[i] = clamp_unsigned(state.acc.signed(i) >> 16) as i16;
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMULQ` — MPEG inverse-quantization multiply. The signed product is placed
/// at accumulator bit 16 and receives the manual's `(31 << 16)` negative
/// rounding bias. The result field is accumulator bits 32..17, signed-clamped,
/// with its low nibble cleared (Programmer's Guide, pp. 61-62 / Table 3-4).
pub fn vmulq(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        let p = vs[i] as i32 as i64 * vt_e[i] as i32 as i64;
        let acc_val = (p << 16) + if p < 0 { 31i64 << 16 } else { 0 };
        state.acc.set(i, acc_val);
        let extracted = clamp_signed(state.acc.signed(i) >> 17);
        vd[i] = (extracted as i32 & !0xF) as i16;
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMUDH` — signed × signed, high (integer) part (§6.1).
///
/// `p = vs[i] * vt_e[i]` (signed 32-bit). `ACC = p << 16` (product occupies
/// acc_mid:acc_hi, acc_lo = 0). `vd[i] = clamp_signed(ACC >> 16)`, i.e. the
/// signed-clamp of the raw 32-bit product.
pub fn vmudh(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        let p = vs[i] as i32 as i64 * vt_e[i] as i32 as i64;
        let acc_val = p << 16;
        state.acc.set(i, acc_val);
        vd[i] = clamp_signed(state.acc.signed(i) >> 16);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMUDM` — signed(vs) × unsigned(vt), middle (§6.1).
///
/// `p = (vs[i] as i16) * (vt_e[i] as u16)`. `ACC = sign_extend(p)` (product in
/// bits 31..0, sign-extended above). `vd[i] = clamp_signed(ACC >> 16)` (the
/// high half of the signed×unsigned product).
pub fn vmudm(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        // vs signed, vt unsigned.
        let p = vs[i] as i32 as i64 * (vt_e[i] as u16) as i64;
        state.acc.set(i, p);
        vd[i] = clamp_signed(state.acc.signed(i) >> 16);
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMUDN` — unsigned(vs) × signed(vt), low, NO clamp (§6.1).
///
/// `p = (vs[i] as u16) * (vt_e[i] as i16)`. `ACC = sign_extend(p)`.
/// `vd[i] = acc_lo` (the low 16 bits, **truncated, no clamp**).
///
/// Contrast `VMADN`, which applies the unsigned-low clamp — the §7 trap. This
/// op truncates.
pub fn vmudn(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        // vs unsigned, vt signed.
        let p = (vs[i] as u16) as i64 * vt_e[i] as i32 as i64;
        state.acc.set(i, p);
        vd[i] = state.acc.read_lo(i) as i16;
    }
    state.regs.r[inv.vd] = vd;
}

/// `VMUDL` — unsigned × unsigned, low fractional, NO clamp (§6.1).
///
/// `p = (vs[i] as u16) * (vt_e[i] as u16)` (unsigned 32-bit). `ACC = p >> 16`
/// (only the high 16 bits of the unsigned product survive, into acc_lo;
/// acc_mid/acc_hi = 0). `vd[i] = acc_lo` (**truncated, no clamp**).
pub fn vmudl(state: &mut VuState, inv: &OpInvocation) {
    let vs = state.regs.r[inv.vs];
    let vt_e = element_select(&state.regs.r[inv.vt], inv.e);
    let mut vd = [0i16; LANES];
    for i in 0..LANES {
        let p = (vs[i] as u16) as u32 * (vt_e[i] as u16) as u32;
        let acc_val = (p >> 16) as i64;
        state.acc.set(i, acc_val);
        vd[i] = state.acc.read_lo(i) as i16;
    }
    state.regs.r[inv.vd] = vd;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsp::vu::VuState;

    /// Build an invocation with identity element select (e = 0) reading vs=1,
    /// vt=2 into vd=3, the natural per-lane pairing.
    fn inv() -> OpInvocation {
        OpInvocation {
            vd: 3,
            vs: 1,
            vt: 2,
            e: 0,
            de: 0,
            vs_index: 0,
        }
    }

    fn state_with(vs: [i16; 8], vt: [i16; 8]) -> VuState {
        let mut s = VuState::new();
        s.regs.r[1] = vs;
        s.regs.r[2] = vt;
        s
    }

    // ---- VMULF ----------------------------------------------------------

    #[test]
    fn vmulf_rounds_and_signed_clamps() {
        // Lane 0: -1.0 * -1.0 = 0x8000 * 0x8000. p = 0x4000_0000,
        // (p<<1)+0x8000 = 0x8000_8000 (positive as signed 48-bit),
        // >>16 = 0x8000 = 32768 -> signed-clamped to 0x7FFF.
        // Lane 1: 0x4000 (0.5) * 0x4000 (0.5) = 0x1000_0000; <<1 = 0x2000_0000;
        // +0x8000 = 0x2000_8000; >>16 = 0x2000 -> 0x2000 (0.25), no clamp.
        // Lane 2: rounding is observable: 0x0001 * 0x4000 = 0x4000; <<1 = 0x8000;
        // +0x8000 = 0x1_0000; >>16 = 1.  Without the +0x8000 round it would be 0.
        let mut s = state_with(
            [0x0001, 0x4000, 0x0001, 0, 0, 0, 0, 0],
            [0x4000, 0x4000, 0x4000, 0, 0, 0, 0, 0],
        );
        // Overwrite lane 0 with the -1.0 case (0x8000 = i16::MIN).
        s.regs.r[1][0] = i16::MIN; // 0x8000
        s.regs.r[2][0] = i16::MIN; // 0x8000
        vmulf(&mut s, &inv());
        assert_eq!(
            s.regs.r[3][0], 0x7FFF,
            "VMULF -1*-1 must saturate to 0x7FFF"
        );
        assert_eq!(s.regs.r[3][1], 0x2000, "VMULF 0.5*0.5 = 0.25");
        assert_eq!(
            s.regs.r[3][2], 1,
            "VMULF rounding: +0x8000 must carry this to 1, not 0"
        );
        // ACC lane 2 check: (0x4000<<1)+0x8000 = 0x1_0000.
        assert_eq!(s.acc.signed(2), 0x1_0000);
    }

    #[test]
    fn vmulu_uses_unsigned_result_clamp_and_preserves_fractional_accumulator() {
        let mut st = VuState::new();
        st.regs.r[1] = [0x4000, -0x4000, 0, 0, 0, 0, 0, 0];
        st.regs.r[2] = [0x4000, 0x4000, 0, 0, 0, 0, 0, 0];
        vmulu(&mut st, &inv());
        assert_eq!(st.regs.r[3][0] as u16, 0x2000);
        assert_eq!(
            st.regs.r[3][1] as u16, 0x0000,
            "negative result clamps to zero"
        );
        assert_eq!(st.acc.signed(0), 0x2000_8000);
        assert_eq!(st.acc.signed(1), -0x1FFF_8000);
    }

    #[test]
    fn vmulf_round_bias_is_required_not_a_bug_green() {
        // A wrong impl that drops the +0x8000 round would give 0 here; the
        // spec-correct value is 1. Pick inputs where the difference shows.
        let mut s = state_with([0x0001, 0, 0, 0, 0, 0, 0, 0], [0x4000, 0, 0, 0, 0, 0, 0, 0]);
        vmulf(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 1);
        // Demonstrate distinguishability: without rounding, acc>>16 would be 0.
        let no_round = clamp_signed((0x4000i64 << 1) >> 16);
        assert_eq!(
            no_round, 0,
            "sanity: the un-rounded result differs (0 vs 1)"
        );
    }

    // ---- VMUDH ----------------------------------------------------------

    #[test]
    fn vmudh_is_signed_clamp_of_integer_product() {
        // Lane 0: 0x1000 * 0x0010 = 0x10000 -> signed-clamp -> 0x7FFF (overflow).
        // Lane 1: 0x0002 * 0x0003 = 6 -> 6.
        // Lane 2: -0x1000 * 0x0010 = -0x10000 -> signed-clamp -> i16::MIN.
        let mut s = state_with(
            [0x1000, 0x0002, -0x1000, 0, 0, 0, 0, 0],
            [0x0010, 0x0003, 0x0010, 0, 0, 0, 0, 0],
        );
        vmudh(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 0x7FFF, "VMUDH must SIGNED-clamp overflow");
        assert_eq!(s.regs.r[3][1], 6);
        assert_eq!(s.regs.r[3][2], i16::MIN);
        // acc = p<<16 -> acc>>16 = p; lane1 acc = 6<<16.
        assert_eq!(s.acc.signed(1), 6i64 << 16);
        assert_eq!(s.acc.read_lo(1), 0, "VMUDH leaves acc_lo = 0");
    }

    #[test]
    fn vmudh_signed_clamp_not_truncate() {
        // If VMUDH truncated acc_lo instead of signed-clamping acc>>16, this
        // 0x10000 product would yield 0 (low 16 of acc_mid = 0); spec = 0x7FFF.
        let mut s = state_with([0x1000, 0, 0, 0, 0, 0, 0, 0], [0x0010, 0, 0, 0, 0, 0, 0, 0]);
        vmudh(&mut s, &inv());
        assert_ne!(s.regs.r[3][0], 0, "truncate-bug would give 0 here");
        assert_eq!(s.regs.r[3][0], 0x7FFF);
    }

    // ---- VMUDM ----------------------------------------------------------

    #[test]
    fn vmudm_signed_vs_times_unsigned_vt() {
        // vs signed, vt UNSIGNED. Lane 0: vs=-1 (0xFFFF), vt=0x8000 (unsigned
        // 32768). p = -1 * 32768 = -32768. acc = sign_extend(-32768).
        // acc>>16 = -1 -> signed-clamp -> -1.
        // If vt were (wrongly) treated as signed -0x8000, p = (-1)*(-32768) =
        // +32768, acc>>16 = 0 -> result 0. So the signedness is distinguishable.
        let mut s = state_with(
            [-1, 0x0100, 0, 0, 0, 0, 0, 0],
            [i16::MIN, 0x0200, 0, 0, 0, 0, 0, 0],
        );
        vmudm(&mut s, &inv());
        assert_eq!(
            s.regs.r[3][0], -1,
            "VMUDM: vt must be UNSIGNED; signed-vt bug would give 0"
        );
        // Lane 1: 0x0100 * 0x0200 = 0x20000; acc>>16 = 2.
        assert_eq!(s.regs.r[3][1], 2);
        assert_eq!(s.acc.signed(0), -32768i64);
    }

    #[test]
    fn vmudm_high_half_extraction() {
        // vs=0x0001, vt=0xFFFF (unsigned 65535). p = 65535. acc>>16 = 0.
        // acc_lo would be 0xFFFF; the result comes from acc>>16 (=0), NOT acc_lo.
        let mut s = state_with([0x0001, 0, 0, 0, 0, 0, 0, 0], [-1, 0, 0, 0, 0, 0, 0, 0]);
        vmudm(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 0, "VMUDM extracts acc>>16, not acc_lo");
        assert_eq!(s.acc.read_lo(0), 0xFFFF);
    }

    // ---- VMUDN ----------------------------------------------------------

    #[test]
    fn vmudn_unsigned_vs_times_signed_vt_truncates_lo() {
        // vs UNSIGNED, vt signed. Lane 0: vs=0xFFFF (65535), vt=-1. p = -65535.
        // acc = sign_extend(-65535) = 0x...FFFF_0001. acc_lo = 0x0001.
        // vd = acc_lo truncated = 1 (0x0001 as i16). NO clamp.
        let mut s = state_with(
            [-1, 0x0002, 0, 0, 0, 0, 0, 0],
            [-1, 0x4000, 0, 0, 0, 0, 0, 0],
        );
        vmudn(&mut s, &inv());
        assert_eq!(
            s.regs.r[3][0], 0x0001,
            "VMUDN: unsigned vs; truncate acc_lo (no clamp)"
        );
        // Lane 1: vs=2 (unsigned), vt=0x4000. p = 0x8000. acc_lo = 0x8000.
        assert_eq!(
            s.regs.r[3][1],
            i16::MIN,
            "0x8000 low word, truncated (no clamp)"
        );
    }

    #[test]
    fn vmudn_truncates_does_not_signed_clamp() {
        // p = -65535: acc[47..16] is negative, so a (wrong) unsigned-low clamp
        // would give 0x0000, and a signed clamp of acc>>16 would give i16::MIN.
        // The spec value (truncate acc_lo) is 0x0001 -- distinct from both.
        let mut s = state_with([-1, 0, 0, 0, 0, 0, 0, 0], [-1, 0, 0, 0, 0, 0, 0, 0]);
        vmudn(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 0x0001);
        assert_ne!(
            s.regs.r[3][0], 0x0000,
            "unsigned-low-clamp bug would give 0"
        );
        assert_ne!(s.regs.r[3][0], i16::MIN, "signed-clamp bug would give MIN");
    }

    // ---- VMUDL ----------------------------------------------------------

    #[test]
    fn vmudl_unsigned_times_unsigned_high_bits_to_lo() {
        // Both UNSIGNED. Lane 0: vs=0xFFFF (65535), vt=0xFFFF (65535).
        // p = 0xFFFE_0001. p>>16 = 0xFFFE. acc_lo = 0xFFFE. vd = 0xFFFE (=-2 i16).
        // If vs/vt were treated as SIGNED (-1 * -1 = 1), p>>16 = 0 -> result 0.
        let mut s = state_with(
            [-1, 0x0001, 0, 0, 0, 0, 0, 0],
            [-1, i16::MIN, 0, 0, 0, 0, 0, 0],
        );
        vmudl(&mut s, &inv());
        assert_eq!(
            s.regs.r[3][0] as u16, 0xFFFE,
            "VMUDL: unsigned*unsigned, high 16 of product; signed bug would give 0"
        );
        // Lane 1: vs=1, vt=0x8000 (unsigned 32768). p = 0x8000. p>>16 = 0.
        assert_eq!(s.regs.r[3][1], 0);
        assert_eq!(s.acc.signed(0), 0xFFFE, "acc holds only the >>16 high bits");
    }

    #[test]
    fn vmudl_shift_is_right_16_not_left() {
        // p = 0x0001_0000 (vs=0x0100, vt=0x0100 unsigned). p>>16 = 1.
        // A wrong <<16 or no-shift would give 0 or 0x0000 in acc_lo.
        let mut s = state_with([0x0100, 0, 0, 0, 0, 0, 0, 0], [0x0100, 0, 0, 0, 0, 0, 0, 0]);
        vmudl(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 1, "VMUDL must >>16 the unsigned product");
    }

    // ---- VMULQ ----------------------------------------------------------

    #[test]
    fn vmulq_masks_low_nibble_and_biases_negative() {
        // Positive lane: p=0x1_0000; ACC>>17=0x8000, signed-clamped and masked
        // to 0x7ff0. Negative lane: p=-256, manual bias makes p+31=-225;
        // ACC>>17=-113 and the low-nibble mask produces -128 (0xff80).
        let mut s = state_with(
            [0x0100, -0x0100, 0, 0, 0, 0, 0, 0],
            [0x0100, 0x0001, 0, 0, 0, 0, 0, 0],
        );
        vmulq(&mut s, &inv());
        assert_eq!(s.regs.r[3][0], 0x7FF0, "VMULQ clears the low nibble");
        assert_eq!(
            s.regs.r[3][1] as u16, 0xFF80,
            "VMULQ uses ACC bits 32..17 after the +31 negative bias"
        );
        // Distinguishability: without the low-nibble mask, lane 0 would be 0x7FFF.
        assert_ne!(s.regs.r[3][0], 0x7FFF, "unmasked bug would leave 0x7FFF");
        // Without the negative bias, ACC>>17=-128 happens to mask identically
        // for this input; ACC itself proves the bias was applied.
        assert_eq!(s.acc.signed(1), (-225i64) << 16);
    }
}
