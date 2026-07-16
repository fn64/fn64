//! The "recip" VU op family: the reciprocal / inverse-sqrt scalar table ops
//! (`VRCP`, `VRCPH`, `VRSQ`, `VRSQH`), the single-lane move (`VMOV`), and the
//! accumulator-rounding pair (`VRNDN`/`VRNDP`).
//!
//! Clean-room from `RSP-VU-ISA.md` §6.10–§6.12. No GPL implementation
//! (`rsp_vu_impl.hpp`) was read. Everything is portable scalar Rust over the
//! shared [`VuState`]/[`Accumulator`]/[`Flags`] API in [`crate::rsp::vu`]; no
//! SIMD.
//!
//! Every op writes its result lane(s) into `state.regs.r[vd]` and mutates
//! `state.acc` / the div latch in place, matching the `dispatch` seam in
//! [`crate::rsp::ops`].
//!
//! The two Q-format oddballs from the task's original "recip" grouping,
//! `VMACQ` and `VMULQ`, are NOT here: `VMACQ` is owned by the [`super::mac`]
//! multiply-accumulate group (it is the accumulate step, §6.2) and `VMULQ` by
//! the [`super::mul_hi`] set-accumulator group (§6.1). This module does not
//! re-implement or re-wire them, to avoid duplicate dispatch arms.

use crate::rsp::ops::{OpInvocation, OpStatus, VuOp};
use crate::rsp::tables::{rcp_seed, rsq_seed};
use crate::rsp::vu::{clamp_signed, element_select, scalar_source_lane, VuState};

// ---------------------------------------------------------------------------
// §6.10 VMOV — move one element/lane
// ---------------------------------------------------------------------------

/// `VMOV` (§6.10): write the element-selected `vt` lane `src(e, de)` into the
/// single destination lane `de`, and broadcast the whole element-selected `vt`
/// into `acc_lo` (the hardware side effect). Flags: none.
pub fn vmov(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    let de = de & 7;
    // Hardware loads the whole broadcasted vt_e into acc_lo across all lanes.
    let vt_e = element_select(&state.regs.r[vt], e);
    for (i, &v) in vt_e.iter().enumerate() {
        state.acc.write_lo(i, v as u16);
    }
    // Only the de lane of vd is written.
    state.regs.r[vd][de] = vt_e[de];
}

// ---------------------------------------------------------------------------
// §6.11 VRNDN / VRNDP — accumulator rounding by sign
// ---------------------------------------------------------------------------

/// Shared body for the VRND pair. `round_positive == true` is `VRNDP` (add when
/// current ACC >= 0), `false` is `VRNDN` (add when current ACC < 0). `vs_index`
/// (0/1) selects whether the addend is shifted left 16. Then every lane gets
/// `vd = clamp_signed(acc >> 16)`. Flags: none.
fn vrnd(state: &mut VuState, vd: usize, vs_index: usize, vt: usize, e: usize, round_positive: bool) {
    let vt_e = element_select(&state.regs.r[vt], e);
    for (i, &vt_lane) in vt_e.iter().enumerate() {
        let mut prod = vt_lane as i64;
        if (vs_index & 1) != 0 {
            prod <<= 16;
        }
        let acc_signed = state.acc.signed(i);
        let do_add = if round_positive {
            acc_signed >= 0
        } else {
            acc_signed < 0
        };
        if do_add {
            state.acc.add(i, prod);
        }
        state.regs.r[vd][i] = clamp_signed(state.acc.signed(i) >> 16);
    }
}

/// `VRNDN` (§6.11): round-negative — add the (optionally <<16) addend into ACC
/// on lanes whose current ACC is negative, then extract `clamp_signed(acc>>16)`.
pub fn vrndn(state: &mut VuState, vd: usize, vs_index: usize, vt: usize, e: usize) {
    vrnd(state, vd, vs_index, vt, e, false);
}

/// `VRNDP` (§6.11): round-positive — add into ACC on lanes whose current ACC is
/// >= 0, then extract `clamp_signed(acc>>16)`.
pub fn vrndp(state: &mut VuState, vd: usize, vs_index: usize, vt: usize, e: usize) {
    vrnd(state, vd, vs_index, vt, e, true);
}

// ---------------------------------------------------------------------------
// §6.12 Reciprocal / inverse-sqrt scalar table ops
// ---------------------------------------------------------------------------

/// The shared normalize step (RSP-VU-ISA.md §6.12 steps 3–4). Given the signed
/// 32-bit `input`, return `(sign_negative, shift, normalized_index_base)` where
/// `shift = clz(magnitude)` and the magnitude has had its INT_MIN guard applied.
/// The caller derives the ROM index from `shift`/`magnitude`.
struct Normalized {
    negative: bool,
    shift: u32,
    magnitude: u32,
}

fn normalize(input: i32) -> Normalized {
    let negative = input < 0;
    let magnitude: u32 = if input < 0 {
        if input == i32::MIN {
            0x7FFF_FFFF
        } else {
            (-input) as u32
        }
    } else {
        input as u32
    };
    // clz of a zero magnitude: hardware treats specially; use 32 so the shift
    // math below produces the documented degenerate result rather than UB.
    let shift = magnitude.leading_zeros();
    Normalized {
        negative,
        shift,
        magnitude,
    }
}

/// Assemble the 32-bit operand for a `…L`/plain op from the 16-bit source and
/// the div latch, per §6.12 step 2.
fn assemble_input(state: &VuState, in16: i16, use_latch: bool) -> i32 {
    if use_latch && state.div_in_loaded {
        (((state.div_in as u32) << 16) | (in16 as u16 as u32)) as i32
    } else {
        in16 as i32 // sign-extend the 16-bit source
    }
}

/// Core reciprocal computation (§6.12 steps 4–7): given the 32-bit `input`,
/// produce the full 32-bit `result`. `is_rsq` selects the inverse-sqrt table +
/// index folding + halved denormalize shift.
fn recip_core(input: i32, is_rsq: bool) -> i32 {
    if input == 0 {
        // 1/0 -> saturated. Hardware yields 0x7FFF_FFFF.
        return 0x7FFF_FFFF;
    }
    let n = normalize(input);
    let shift = n.shift;
    let normalized = n.magnitude << shift; // top bit now set (bit 31)
    let (index, frac, denorm_shift) = if is_rsq {
        let idx = (((normalized >> 23) & 0x1FE) | (shift & 1)) as usize;
        let frac = rsq_seed(idx);
        (idx, frac, (31 - shift) >> 1)
    } else {
        let idx = ((normalized >> 22) & 0x1FF) as usize;
        let frac = rcp_seed(idx);
        (idx, frac, 31 - shift)
    };
    let _ = index;
    // Reconstruct with the implicit leading 1, place, then denormalize.
    let mantissa = (0x1_0000u64 | frac as u64) << 14;
    let mut result = (mantissa >> denorm_shift) as i32;
    if n.negative {
        result = !result;
    }
    result
}

/// `VRCP` (§6.12): single-precision reciprocal. Operates on the sign-extended
/// 16-bit `vt` lane selected by `e`; writes the low 16 of the result to `vd[de]`
/// and latches the high 16 into `div_out`. Broadcasts `vt_e` into acc_lo.
pub fn vrcp(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    recip_scalar(state, vd, de, vt, e, /*is_rsq=*/ false, /*use_latch=*/ false);
}

/// `VRSQ` (§6.12): single-precision inverse-sqrt. Same shape as VRCP with the
/// rsq table/shift.
pub fn vrsq(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    recip_scalar(state, vd, de, vt, e, /*is_rsq=*/ true, /*use_latch=*/ false);
}

/// Shared body for the single/low reciprocal-family ops. Reads the selected
/// source lane, assembles the 32-bit operand (using the div latch if
/// `use_latch`), runs the table core, writes `vd[de]` = low 16, latches the
/// high 16 into `div_out`, clears the latch, and broadcasts `vt_e` into acc_lo.
fn recip_scalar(
    state: &mut VuState,
    vd: usize,
    de: usize,
    vt: usize,
    e: usize,
    is_rsq: bool,
    use_latch: bool,
) {
    let de = de & 7;
    let src = scalar_source_lane(e, de);
    let in16 = state.regs.r[vt][src];
    let input = assemble_input(state, in16, use_latch);
    let result = recip_core(input, is_rsq);
    // Broadcast vt_e into acc_lo (VMOV-like side effect).
    let vt_e = element_select(&state.regs.r[vt], e);
    for (i, &v) in vt_e.iter().enumerate() {
        state.acc.write_lo(i, v as u16);
    }
    state.div_out = ((result as u32) >> 16) as u16;
    state.div_in_loaded = false;
    state.regs.r[vd][de] = (result & 0xFFFF) as u16 as i16;
}

/// `VRCPH` (§6.12): high half. Does NOT run the table. It (a) latches the high
/// 16 bits of the *input* (`div_in = vt_e[src]`) for the next `VRCPL`, and (b)
/// writes the previously-computed high result (`vd[de] = div_out`). Broadcasts
/// `vt_e` into acc_lo.
pub fn vrcph(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    recip_high(state, vd, de, vt, e);
}

/// `VRSQH` (§6.12): high half of inverse-sqrt — identical latch/emit role to
/// VRCPH (the high-half op runs no table for either family).
pub fn vrsqh(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    recip_high(state, vd, de, vt, e);
}

/// Shared VRCPH/VRSQH body.
fn recip_high(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize) {
    let de = de & 7;
    let src = scalar_source_lane(e, de);
    let in16 = state.regs.r[vt][src];
    // Latch the high 16 of the input for the following …L.
    state.div_in = in16 as u16;
    state.div_in_loaded = true;
    // Broadcast vt_e into acc_lo.
    let vt_e = element_select(&state.regs.r[vt], e);
    for (i, &v) in vt_e.iter().enumerate() {
        state.acc.write_lo(i, v as u16);
    }
    // Emit the previously-computed high result.
    state.regs.r[vd][de] = state.div_out as i16;
}

/// Helper used by tests / a future dispatch of the paired low ops (`VRCPL` /
/// `VRSQL`) — consumes the latch. Exposed here so the recip family's latch
/// contract is exercised even though VRCPL/VRSQL proper are wired by another
/// group. Not part of this group's dispatch arms.
#[allow(dead_code)]
fn recip_low(state: &mut VuState, vd: usize, de: usize, vt: usize, e: usize, is_rsq: bool) {
    recip_scalar(state, vd, de, vt, e, is_rsq, /*use_latch=*/ true);
}

/// Route a "recip"-family opcode to its body. Returns `Some(Executed)` for the
/// ops this module owns (`VMOV`, `VRNDN`/`VRNDP`, `VRCP`/`VRCPH`/`VRSQ`/
/// `VRSQH`), `None` otherwise so the central dispatcher can try the next
/// family. Wired into [`crate::rsp::ops::dispatch`] for this group's opcodes
/// only — `VMACQ`/`VMULQ`/`VRCPL`/`VRSQL` are handled by other groups.
pub fn try_dispatch(state: &mut VuState, op: VuOp, inv: &OpInvocation) -> Option<OpStatus> {
    match op {
        VuOp::Vmov => vmov(state, inv.vd, inv.de, inv.vt, inv.e),
        VuOp::Vrndn => vrndn(state, inv.vd, inv.vs_index, inv.vt, inv.e),
        VuOp::Vrndp => vrndp(state, inv.vd, inv.vs_index, inv.vt, inv.e),
        VuOp::Vrcp => vrcp(state, inv.vd, inv.de, inv.vt, inv.e),
        VuOp::Vrcph => vrcph(state, inv.vd, inv.de, inv.vt, inv.e),
        VuOp::Vrsq => vrsq(state, inv.vd, inv.de, inv.vt, inv.e),
        VuOp::Vrsqh => vrsqh(state, inv.vd, inv.de, inv.vt, inv.e),
        _ => return None,
    }
    Some(OpStatus::Executed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsp::vu::VuState;

    // -- VMOV -----------------------------------------------------------------

    #[test]
    fn vmov_moves_selected_lane_only_and_loads_acc_lo() {
        let mut st = VuState::new();
        st.regs.r[2] = [10, 11, 12, 13, 14, 15, 16, 17];
        st.regs.r[5] = [0; 8];
        // e=8 => whole-broadcast lane 0 (=10); de=3.
        vmov(&mut st, 5, 3, 2, 8);
        // Only lane 3 written, = broadcast value 10.
        assert_eq!(st.regs.r[5], [0, 0, 0, 10, 0, 0, 0, 0]);
        // acc_lo broadcast to 10 across all lanes.
        for i in 0..8 {
            assert_eq!(st.acc.read_lo(i), 10u16, "acc_lo lane {i}");
        }
    }

    #[test]
    fn vmov_identity_element_picks_de_lane() {
        let mut st = VuState::new();
        st.regs.r[2] = [0, 1, 2, 3, 4, 5, 6, 7];
        // e=0 identity => src(0, de) = de. de=6 -> value 6.
        vmov(&mut st, 5, 6, 2, 0);
        assert_eq!(st.regs.r[5][6], 6);
        // Wrong-lane guard: if source selection were broadcast-lane-0 we'd get 0.
        assert_ne!(st.regs.r[5][6], 0);
    }

    // -- VRNDN / VRNDP --------------------------------------------------------

    #[test]
    fn vrndp_adds_only_on_nonnegative_acc() {
        let mut st = VuState::new();
        // lane0 acc >=0, lane1 acc <0. Addend = 0x100 (vs_index=0 -> no <<16).
        st.acc.set(0, 0x0000_0000_1000);
        st.acc.set(1, -(0x0000_0000_1000));
        st.regs.r[3] = [0x100; 8];
        vrndp(&mut st, 7, 0, 3, 0);
        // lane0: acc>=0 so add 0x100 -> 0x1100; vd = clamp(acc>>16) = 0.
        assert_eq!(st.acc.signed(0), 0x1100);
        // lane1: acc<0 so NOT added; unchanged.
        assert_eq!(st.acc.signed(1), -0x1000);
    }

    #[test]
    fn vrndn_adds_only_on_negative_acc_and_shift_applies() {
        let mut st = VuState::new();
        st.acc.set(0, 0x1000); // >=0 -> no add for VRNDN
        st.acc.set(1, -0x1000); // <0 -> add
        st.regs.r[3] = [0x2; 8];
        // vs_index=1 -> addend <<16 = 0x2_0000.
        vrndn(&mut st, 7, 1, 3, 0);
        assert_eq!(st.acc.signed(0), 0x1000, "lane0 must be untouched");
        assert_eq!(st.acc.signed(1), -0x1000 + 0x2_0000, "lane1 gets shifted add");
        // vd lane1 = clamp_signed(acc>>16) = clamp((0x1F000)>>16)=1.
        assert_eq!(st.regs.r[7][1], 1);
    }

    #[test]
    fn vrnd_shift_flag_is_distinguishable() {
        // Same setup, vs_index 0 vs 1 must give different ACC — guards the <<16.
        let build = |vs_index: usize| {
            let mut st = VuState::new();
            st.acc.set(0, -1); // negative -> VRNDN adds
            st.regs.r[3] = [0x3; 8];
            vrndn(&mut st, 7, vs_index, 3, 0);
            st.acc.signed(0)
        };
        assert_eq!(build(0), -1 + 0x3);
        assert_eq!(build(1), -1 + 0x3_0000);
        assert_ne!(build(0), build(1));
    }

    // -- VRCP -----------------------------------------------------------------

    #[test]
    fn vrcp_of_one_matches_table_reconstruction() {
        let mut st = VuState::new();
        st.regs.r[4] = [1, 0, 0, 0, 0, 0, 0, 0];
        // e=0 identity, de=0 -> source lane 0 = 1.
        vrcp(&mut st, 6, 0, 4, 0);
        // Recompute expected independently:
        // input=1, shift=clz(1)=31, normalized=0x8000_0000, index=(>>22)&0x1FF=0,
        // frac=rcp[0]=0xFFFF, mantissa=(0x1FFFF<<14), denorm=31-31=0 -> result.
        let frac = rcp_seed(0) as u64;
        let mantissa = (0x1_0000u64 | frac) << 14;
        // denorm shift is 31 - shift = 0 here, so mantissa is placed as-is.
        let expected = mantissa as i32;
        assert_eq!(st.div_out, ((expected as u32) >> 16) as u16, "div_out hi");
        assert_eq!(st.regs.r[6][0], (expected & 0xFFFF) as u16 as i16, "vd lo");
    }

    #[test]
    fn vrcp_negative_input_ones_complements_result() {
        let mut posst = VuState::new();
        posst.regs.r[4] = [4, 0, 0, 0, 0, 0, 0, 0];
        vrcp(&mut posst, 6, 0, 4, 0);
        let pos_full =
            ((posst.div_out as u32) << 16) | (posst.regs.r[6][0] as u16 as u32);

        let mut negst = VuState::new();
        negst.regs.r[4] = [-4, 0, 0, 0, 0, 0, 0, 0];
        vrcp(&mut negst, 6, 0, 4, 0);
        let neg_full =
            ((negst.div_out as u32) << 16) | (negst.regs.r[6][0] as u16 as u32);

        // Negative reciprocal is the ones-complement of the positive one.
        assert_eq!(neg_full, !pos_full);
        // Distinguisher: it is NOT the two's-complement (that would be off by 1).
        assert_ne!(neg_full, pos_full.wrapping_neg());
    }

    #[test]
    fn vrcp_larger_magnitude_gives_smaller_reciprocal() {
        // Monotonicity sanity: 1/2 > 1/4 in the high word.
        let recip_hi = |x: i16| {
            let mut st = VuState::new();
            st.regs.r[4] = [x, 0, 0, 0, 0, 0, 0, 0];
            vrcp(&mut st, 6, 0, 4, 0);
            st.div_out
        };
        assert!(recip_hi(2) > recip_hi(4));
        assert!(recip_hi(4) > recip_hi(8));
    }

    // -- VRCPH / VRCPL latch --------------------------------------------------

    #[test]
    fn vrcph_latches_input_and_emits_prior_high_result() {
        let mut st = VuState::new();
        st.div_out = 0x1234; // pretend a prior op computed this high result
        st.regs.r[4] = [0x00AB, 0, 0, 0, 0, 0, 0, 0];
        vrcph(&mut st, 6, 0, 4, 0);
        // Emits the prior div_out as vd[de].
        assert_eq!(st.regs.r[6][0], 0x1234);
        // Latches the input high half.
        assert_eq!(st.div_in, 0x00AB);
        assert!(st.div_in_loaded);
    }

    #[test]
    fn vrcph_then_vrcpl_uses_full_32bit_input() {
        // 32-bit input assembled across VRCPH (hi=0x0001) + VRCPL (lo=0x0000)
        // must equal the single-op VRCP of 0x0001_0000 (=65536), NOT of 0.
        let mut st = VuState::new();
        st.regs.r[4] = [0x0001, 0, 0, 0, 0, 0, 0, 0];
        vrcph(&mut st, 6, 0, 4, 0); // latch hi = 0x0001
        st.regs.r[4] = [0x0000, 0, 0, 0, 0, 0, 0, 0];
        recip_low(&mut st, 6, 0, 4, 0, false); // lo = 0x0000

        // Independent expectation via the core on the full 32-bit input.
        let expected = recip_core(0x0001_0000, false);
        let got = ((st.div_out as u32) << 16) | (st.regs.r[6][0] as u16 as u32);
        assert_eq!(got, expected as u32);
        // Distinguisher: this must differ from reciprocal-of-zero-latch (0x0000).
        let recip_of_lo_only = recip_core(0x0000, false);
        assert_ne!(got, recip_of_lo_only as u32);
    }

    // -- VRSQ -----------------------------------------------------------------

    #[test]
    fn vrsq_uses_rsq_table_not_rcp() {
        // For the same input, VRSQ and VRCP must differ (different ROM + shift).
        let mut a = VuState::new();
        a.regs.r[4] = [16, 0, 0, 0, 0, 0, 0, 0];
        vrcp(&mut a, 6, 0, 4, 0);
        let rcp_hi = a.div_out;

        let mut b = VuState::new();
        b.regs.r[4] = [16, 0, 0, 0, 0, 0, 0, 0];
        vrsq(&mut b, 6, 0, 4, 0);
        let rsq_hi = b.div_out;

        assert_ne!(rcp_hi, rsq_hi, "rsq must not equal rcp for the same input");
        // Independent recompute of rsq high word.
        let expected = recip_core(16, true);
        assert_eq!(rsq_hi, ((expected as u32) >> 16) as u16);
    }

    #[test]
    fn vrsq_shift_parity_folds_into_index() {
        // Inputs whose magnitude differs by one exponent (2 vs 4) select the two
        // parity octaves; results must differ.
        let rsq_hi = |x: i16| {
            let mut st = VuState::new();
            st.regs.r[4] = [x, 0, 0, 0, 0, 0, 0, 0];
            vrsq(&mut st, 6, 0, 4, 0);
            st.div_out
        };
        assert_ne!(rsq_hi(2), rsq_hi(4));
        // Larger magnitude -> smaller 1/sqrt.
        assert!(rsq_hi(2) >= rsq_hi(4));
    }

    #[test]
    fn vrsqh_latches_and_emits_like_vrcph() {
        let mut st = VuState::new();
        st.div_out = 0x5678;
        st.regs.r[4] = [0x00CD, 0, 0, 0, 0, 0, 0, 0];
        vrsqh(&mut st, 6, 0, 4, 0);
        assert_eq!(st.regs.r[6][0], 0x5678);
        assert_eq!(st.div_in, 0x00CD);
        assert!(st.div_in_loaded);
    }

    #[test]
    fn recip_core_denorm_shift_differs_between_rcp_and_rsq() {
        // Guard the shift: rsq halves the denormalize shift, so for a mid-range
        // input the rcp and rsq results must not coincide.
        let input = 0x0000_5A82; // arbitrary mid value
        assert_ne!(recip_core(input, false), recip_core(input, true));
    }
}
