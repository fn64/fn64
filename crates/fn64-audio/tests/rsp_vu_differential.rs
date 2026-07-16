//! Instruction-boundary differential oracle for the RSP vector unit.
//!
//! The production side is fn64's typed-Rust dispatcher.  `ReferenceVu` is a
//! deliberately separate, test-only semantic model: it represents each ACC
//! lane as one signed 48-bit integer and evaluates the equations from the SGI
//! *Nintendo 64 RSP Programmer's Guide* appendix directly.  Algorithm shape
//! was independently checked against CEN64's BSD-3-Clause vector helpers
//! (`arch/x86_64/rsp/*.h`, `vrcpsq.c`, and `common/reciprocal.c`).  No fn64 VU
//! helper (clamp, element selection, ROM lookup, or op body) is used by the
//! reference side.
#![forbid(unsafe_code)]

use fn64_audio::rsp::decode::{VLoadOp, VStoreOp};
use fn64_audio::rsp::ops::{dispatch, OpInvocation, OpStatus, VuOp, ALL_VU_OPS};
use fn64_audio::rsp::runtime::RspMachine;
use fn64_audio::rsp::{Flags, VuState};

const MASK48: i64 = 0xFFFF_FFFF_FFFF;
const LANES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReferenceVu {
    regs: [[i16; LANES]; 32],
    acc: [i64; LANES],
    flags: Flags,
    div_in: u16,
    div_in_loaded: bool,
    div_out: u16,
}

impl ReferenceVu {
    fn capture(actual: &VuState) -> Self {
        let mut acc = [0; LANES];
        for (lane, slot) in acc.iter_mut().enumerate() {
            *slot = actual.acc.signed(lane);
        }
        Self {
            regs: actual.regs.r,
            acc,
            flags: actual.flags,
            div_in: actual.div_in,
            div_in_loaded: actual.div_in_loaded,
            div_out: actual.div_out,
        }
    }

    fn assert_matches(&self, actual: &VuState, label: &str) {
        assert_eq!(actual.regs.r, self.regs, "{label}: vector registers");
        for lane in 0..LANES {
            assert_eq!(
                actual.acc.signed(lane),
                self.acc[lane],
                "{label}: ACC lane {lane}"
            );
        }
        assert_eq!(actual.flags, self.flags, "{label}: VCO/VCC/VCE");
        assert_eq!(actual.div_in, self.div_in, "{label}: divider input latch");
        assert_eq!(
            actual.div_in_loaded, self.div_in_loaded,
            "{label}: divider input-valid latch"
        );
        assert_eq!(
            actual.div_out, self.div_out,
            "{label}: divider output latch"
        );
    }

    fn set_acc(&mut self, lane: usize, value: i64) {
        self.acc[lane] = sign48(value);
    }

    fn add_acc(&mut self, lane: usize, value: i64) {
        self.set_acc(lane, self.acc[lane].wrapping_add(value));
    }

    fn write_lo(&mut self, lane: usize, value: u16) {
        self.set_acc(lane, (self.acc[lane] & !0xFFFF) | i64::from(value));
    }

    fn exec(&mut self, op: VuOp, inv: OpInvocation) {
        let vs = self.regs[inv.vs];
        let vt = selected(self.regs[inv.vt], inv.e);
        match op {
            VuOp::Vmulf
            | VuOp::Vmulu
            | VuOp::Vmulq
            | VuOp::Vmudh
            | VuOp::Vmudm
            | VuOp::Vmudn
            | VuOp::Vmudl => {
                for lane in 0..LANES {
                    let s = i64::from(vs[lane]);
                    let t = i64::from(vt[lane]);
                    let (acc, result) = match op {
                        VuOp::Vmulf | VuOp::Vmulu => {
                            let acc = s * t * 2 + 0x8000;
                            let md = sign48(acc) >> 16;
                            let result = if op == VuOp::Vmulf {
                                signed_clamp(md) as u16
                            } else {
                                fractional_unsigned_clamp(md)
                            };
                            (acc, result)
                        }
                        VuOp::Vmulq => {
                            let product = s * t;
                            let acc = (product + if product < 0 { 31 } else { 0 }) << 16;
                            let result = signed_clamp(sign48(acc) >> 17) as u16 & 0xFFF0;
                            (acc, result)
                        }
                        VuOp::Vmudh => {
                            let acc = (s * t) << 16;
                            (acc, signed_clamp(sign48(acc) >> 16) as u16)
                        }
                        VuOp::Vmudm => {
                            let acc = s * i64::from(vt[lane] as u16);
                            (acc, signed_clamp(sign48(acc) >> 16) as u16)
                        }
                        VuOp::Vmudn => {
                            let acc = i64::from(vs[lane] as u16) * t;
                            (acc, acc as u16)
                        }
                        VuOp::Vmudl => {
                            let product = u32::from(vs[lane] as u16) * u32::from(vt[lane] as u16);
                            let acc = i64::from(product >> 16);
                            (acc, acc as u16)
                        }
                        _ => unreachable!(),
                    };
                    self.set_acc(lane, acc);
                    self.regs[inv.vd][lane] = result as i16;
                }
            }
            VuOp::Vmacf | VuOp::Vmacu | VuOp::Vmadh | VuOp::Vmadm | VuOp::Vmadn | VuOp::Vmadl => {
                for lane in 0..LANES {
                    let delta = match op {
                        VuOp::Vmacf | VuOp::Vmacu => i64::from(vs[lane]) * i64::from(vt[lane]) * 2,
                        VuOp::Vmadh => (i64::from(vs[lane]) * i64::from(vt[lane])) << 16,
                        VuOp::Vmadm => i64::from(vs[lane]) * i64::from(vt[lane] as u16),
                        VuOp::Vmadn => i64::from(vs[lane] as u16) * i64::from(vt[lane]),
                        VuOp::Vmadl => i64::from(
                            (u32::from(vs[lane] as u16) * u32::from(vt[lane] as u16)) >> 16,
                        ),
                        _ => unreachable!(),
                    };
                    self.add_acc(lane, delta);
                    let result = match op {
                        VuOp::Vmacu => fractional_unsigned_clamp(self.acc[lane] >> 16),
                        VuOp::Vmadn | VuOp::Vmadl => low_slice_clamp(self.acc[lane]),
                        _ => signed_clamp(self.acc[lane] >> 16) as u16,
                    };
                    self.regs[inv.vd][lane] = result as i16;
                }
            }
            VuOp::Vmacq => {
                for lane in 0..LANES {
                    let acc = self.acc[lane];
                    if (acc >> 21) != 0 && acc & (1 << 21) == 0 {
                        self.add_acc(lane, if acc < 0 { 32 << 16 } else { -(32 << 16) });
                    }
                    self.regs[inv.vd][lane] =
                        (signed_clamp(self.acc[lane] >> 17) as u16 & 0xFFF0) as i16;
                }
            }
            VuOp::Vadd | VuOp::Vsub | VuOp::Vaddc | VuOp::Vsubc | VuOp::Vabs => {
                for lane in 0..LANES {
                    let s = i32::from(vs[lane]);
                    let t = i32::from(vt[lane]);
                    let carry = i32::from(flag(self.flags.vco, lane));
                    let (raw, result) = match op {
                        VuOp::Vadd => {
                            let raw = s + t + carry;
                            clear_vco_lane(&mut self.flags, lane);
                            (raw, signed_clamp(i64::from(raw)))
                        }
                        VuOp::Vsub => {
                            let raw = s - t - carry;
                            clear_vco_lane(&mut self.flags, lane);
                            (raw, signed_clamp(i64::from(raw)))
                        }
                        VuOp::Vaddc => {
                            let sum = u32::from(vs[lane] as u16) + u32::from(vt[lane] as u16);
                            set_flag(&mut self.flags.vco, lane, sum > 0xFFFF);
                            set_flag(&mut self.flags.vco, lane + 8, false);
                            (sum as i32, sum as u16 as i16)
                        }
                        VuOp::Vsubc => {
                            let su = vs[lane] as u16;
                            let tu = vt[lane] as u16;
                            set_flag(&mut self.flags.vco, lane, su < tu);
                            set_flag(&mut self.flags.vco, lane + 8, su != tu);
                            (i32::from(su.wrapping_sub(tu)), su.wrapping_sub(tu) as i16)
                        }
                        VuOp::Vabs => {
                            let raw = if s < 0 {
                                -t
                            } else if s > 0 {
                                t
                            } else {
                                0
                            };
                            (raw, signed_clamp(i64::from(raw)))
                        }
                        _ => unreachable!(),
                    };
                    self.write_lo(lane, raw as u16);
                    self.regs[inv.vd][lane] = result;
                }
            }
            VuOp::Vand | VuOp::Vnand | VuOp::Vor | VuOp::Vnor | VuOp::Vxor | VuOp::Vnxor => {
                for lane in 0..LANES {
                    let s = vs[lane] as u16;
                    let t = vt[lane] as u16;
                    let value = match op {
                        VuOp::Vand => s & t,
                        VuOp::Vnand => !(s & t),
                        VuOp::Vor => s | t,
                        VuOp::Vnor => !(s | t),
                        VuOp::Vxor => s ^ t,
                        VuOp::Vnxor => !(s ^ t),
                        _ => unreachable!(),
                    };
                    self.write_lo(lane, value);
                    self.regs[inv.vd][lane] = value as i16;
                }
            }
            VuOp::Vlt | VuOp::Veq | VuOp::Vne | VuOp::Vge | VuOp::Vmrg => {
                for lane in 0..LANES {
                    let equal = vs[lane] == vt[lane];
                    let carry = flag(self.flags.vco, lane);
                    let ne = flag(self.flags.vco, lane + 8);
                    let select_vs = match op {
                        VuOp::Vlt => vs[lane] < vt[lane] || (equal && carry && ne),
                        VuOp::Veq => equal && !ne,
                        VuOp::Vne => !equal || ne,
                        VuOp::Vge => vs[lane] > vt[lane] || (equal && !(carry && ne)),
                        VuOp::Vmrg => flag(self.flags.vcc, lane),
                        _ => unreachable!(),
                    };
                    if op != VuOp::Vmrg {
                        set_flag(&mut self.flags.vcc, lane, select_vs);
                        set_flag(&mut self.flags.vcc, lane + 8, false);
                        clear_vco_lane(&mut self.flags, lane);
                    }
                    let result = if select_vs { vs[lane] } else { vt[lane] };
                    self.write_lo(lane, result as u16);
                    self.regs[inv.vd][lane] = result;
                }
            }
            VuOp::Vch => self.vch(inv, vs, vt),
            VuOp::Vcl => self.vcl(inv, vs, vt),
            VuOp::Vcr => self.vcr(inv, vs, vt),
            VuOp::Vsar => {
                for lane in 0..LANES {
                    self.regs[inv.vd][lane] = match inv.e {
                        8 => (self.acc[lane] >> 32) as u16 as i16,
                        9 => (self.acc[lane] >> 16) as u16 as i16,
                        10 => self.acc[lane] as u16 as i16,
                        _ => 0,
                    };
                }
            }
            VuOp::Vmov => {
                for (lane, &value) in vt.iter().enumerate() {
                    self.write_lo(lane, value as u16);
                }
                self.regs[inv.vd][inv.de & 7] = self.regs[inv.vt][inv.e & 7];
            }
            VuOp::Vrndn | VuOp::Vrndp => {
                for (lane, &value) in vt.iter().enumerate() {
                    let add = if inv.vs_index & 1 == 0 {
                        i64::from(value)
                    } else {
                        i64::from(value) << 16
                    };
                    let selected_sign = if op == VuOp::Vrndn {
                        self.acc[lane] < 0
                    } else {
                        self.acc[lane] >= 0
                    };
                    if selected_sign {
                        self.add_acc(lane, add);
                    }
                    self.regs[inv.vd][lane] = signed_clamp(self.acc[lane] >> 16);
                }
            }
            VuOp::Vrcp | VuOp::Vrcpl | VuOp::Vrsq | VuOp::Vrsql => {
                let source = self.regs[inv.vt][inv.e & 7];
                let use_latch = matches!(op, VuOp::Vrcpl | VuOp::Vrsql) && self.div_in_loaded;
                let input = if use_latch {
                    (((u32::from(self.div_in)) << 16) | u32::from(source as u16)) as i32
                } else {
                    i32::from(source)
                };
                let result = divide(input, matches!(op, VuOp::Vrsq | VuOp::Vrsql));
                for (lane, &value) in vt.iter().enumerate() {
                    self.write_lo(lane, value as u16);
                }
                self.div_out = (result >> 16) as u16;
                self.div_in_loaded = false;
                self.regs[inv.vd][inv.de & 7] = result as u16 as i16;
            }
            VuOp::Vrcph | VuOp::Vrsqh => {
                for (lane, &value) in vt.iter().enumerate() {
                    self.write_lo(lane, value as u16);
                }
                self.div_in = self.regs[inv.vt][inv.e & 7] as u16;
                self.div_in_loaded = true;
                self.regs[inv.vd][inv.de & 7] = self.div_out as i16;
            }
            VuOp::Vnop => {}
        }
    }

    fn vch(&mut self, inv: OpInvocation, vs: [i16; LANES], vt: [i16; LANES]) {
        for lane in 0..LANES {
            let s = i32::from(vs[lane]);
            let t = i32::from(vt[lane]);
            let opposite = (s ^ t) < 0;
            let result = if opposite {
                let sum = s + t;
                set_flag(&mut self.flags.vco, lane, true);
                set_flag(&mut self.flags.vco, lane + 8, sum != 0 && sum != -1);
                set_flag(&mut self.flags.vce as &mut u8, lane, sum == -1);
                set_flag(&mut self.flags.vcc, lane, sum <= 0);
                set_flag(&mut self.flags.vcc, lane + 8, t < 0);
                if sum <= 0 {
                    (vt[lane]).wrapping_neg()
                } else {
                    vs[lane]
                }
            } else {
                let diff = s - t;
                set_flag(&mut self.flags.vco, lane, false);
                set_flag(&mut self.flags.vco, lane + 8, diff != 0);
                set_flag(&mut self.flags.vce as &mut u8, lane, false);
                set_flag(&mut self.flags.vcc, lane, t < 0);
                set_flag(&mut self.flags.vcc, lane + 8, diff >= 0);
                if diff >= 0 {
                    vt[lane]
                } else {
                    vs[lane]
                }
            };
            self.write_lo(lane, result as u16);
            self.regs[inv.vd][lane] = result;
        }
    }

    fn vcl(&mut self, inv: OpInvocation, vs: [i16; LANES], vt: [i16; LANES]) {
        for lane in 0..LANES {
            let sign = flag(self.flags.vco, lane);
            let ne = flag(self.flags.vco, lane + 8);
            if sign && !ne {
                let sum = u32::from(vs[lane] as u16) + u32::from(vt[lane] as u16);
                let zero = sum as u16 == 0;
                let no_carry = sum <= 0xFFFF;
                let le = if flag(u16::from(self.flags.vce), lane) {
                    zero || no_carry
                } else {
                    zero && no_carry
                };
                set_flag(&mut self.flags.vcc, lane, le);
            } else if !sign && !ne {
                set_flag(
                    &mut self.flags.vcc,
                    lane + 8,
                    (vs[lane] as u16) >= (vt[lane] as u16),
                );
            }
            let select = flag(self.flags.vcc, lane + if sign { 0 } else { 8 });
            let result = if select {
                if sign {
                    vt[lane].wrapping_neg()
                } else {
                    vt[lane]
                }
            } else {
                vs[lane]
            };
            self.write_lo(lane, result as u16);
            self.regs[inv.vd][lane] = result;
            clear_vco_lane(&mut self.flags, lane);
            set_flag(&mut self.flags.vce as &mut u8, lane, false);
        }
    }

    fn vcr(&mut self, inv: OpInvocation, vs: [i16; LANES], vt: [i16; LANES]) {
        for lane in 0..LANES {
            let s = i32::from(vs[lane]);
            let t = i32::from(vt[lane]);
            let opposite = (s ^ t) < 0;
            let result = if opposite {
                let le = s + t < 0;
                set_flag(&mut self.flags.vcc, lane, le);
                set_flag(&mut self.flags.vcc, lane + 8, t < 0);
                if le {
                    !vt[lane]
                } else {
                    vs[lane]
                }
            } else {
                let ge = s - t >= 0;
                set_flag(&mut self.flags.vcc, lane, t < 0);
                set_flag(&mut self.flags.vcc, lane + 8, ge);
                if ge {
                    vt[lane]
                } else {
                    vs[lane]
                }
            };
            self.write_lo(lane, result as u16);
            self.regs[inv.vd][lane] = result;
            clear_vco_lane(&mut self.flags, lane);
            set_flag(&mut self.flags.vce as &mut u8, lane, false);
        }
    }
}

fn sign48(value: i64) -> i64 {
    ((value & MASK48) << 16) >> 16
}

fn selected(vt: [i16; LANES], e: usize) -> [i16; LANES] {
    let mut result = [0; LANES];
    for (lane, slot) in result.iter_mut().enumerate() {
        let source = match e & 15 {
            0 | 1 => lane,
            2 | 3 => (lane & 6) | (e & 1),
            4..=7 => (lane & 4) | (e & 3),
            _ => e & 7,
        };
        *slot = vt[source];
    }
    result
}

fn signed_clamp(value: i64) -> i16 {
    value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn fractional_unsigned_clamp(value: i64) -> u16 {
    if value < 0 {
        0
    } else if value > i64::from(i16::MAX) {
        0xFFFF
    } else {
        value as u16
    }
}

fn low_slice_clamp(acc: i64) -> u16 {
    let hi = (acc >> 32) as u16 as i16;
    let mid = (acc >> 16) as u16 as i16;
    if hi == mid >> 15 {
        acc as u16
    } else if hi < 0 {
        0
    } else {
        0xFFFF
    }
}

fn flag<T: Into<u16>>(bits: T, lane: usize) -> bool {
    (bits.into() >> lane) & 1 != 0
}

trait FlagWord {
    fn get(&self) -> u16;
    fn set(&mut self, value: u16);
}

impl FlagWord for u16 {
    fn get(&self) -> u16 {
        *self
    }
    fn set(&mut self, value: u16) {
        *self = value;
    }
}

impl FlagWord for u8 {
    fn get(&self) -> u16 {
        u16::from(*self)
    }
    fn set(&mut self, value: u16) {
        *self = value as u8;
    }
}

fn set_flag(bits: &mut impl FlagWord, lane: usize, value: bool) {
    let mask = 1u16 << lane;
    let word = if value {
        bits.get() | mask
    } else {
        bits.get() & !mask
    };
    bits.set(word);
}

fn clear_vco_lane(flags: &mut Flags, lane: usize) {
    set_flag(&mut flags.vco, lane, false);
    set_flag(&mut flags.vco, lane + 8, false);
}

fn integer_sqrt(value: u64) -> u64 {
    let mut low = 0u64;
    let mut high = 1u64 << 24;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if mid <= value / mid {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn reference_seed(index: usize, rsq: bool) -> u16 {
    if !rsq {
        if index == 0 {
            return 0xFFFF;
        }
        let denominator = index as u64 + 512;
        return ((((1u64 << 34) / denominator + 1) >> 8) & 0xFFFF) as u16;
    }
    let denominator = (index as u64 >> 1) + 256;
    let numerator = 1u64 << (47 + (index & 1));
    let raw = ((integer_sqrt(numerator / denominator) >> 3) & 0xFFFF) as u16;
    if index == 1 {
        0xFFFF
    } else {
        raw
    }
}

fn divide(input: i32, rsq: bool) -> i32 {
    let sign_mask = input >> 31;
    let mut data = input ^ sign_mask;
    if input > -32768 {
        data -= sign_mask;
    }
    if data == 0 {
        return 0x7FFF_FFFF;
    }
    if input == -32768 {
        return 0xFFFF_0000u32 as i32;
    }
    let shift = (data as u32).leading_zeros();
    let normalized = (data as u32) << shift;
    let index = if rsq {
        (((normalized >> 22) & 0x1FE) | (shift & 1)) as usize
    } else {
        ((normalized >> 22) & 0x1FF) as usize
    };
    let mantissa = (0x1_0000u64 | u64::from(reference_seed(index, rsq))) << 14;
    let denormalize = if rsq { (31 - shift) >> 1 } else { 31 - shift };
    ((mantissa >> denormalize) as i32) ^ sign_mask
}

fn seeded_state(seed: usize) -> VuState {
    const VECTORS: [[i16; LANES]; 3] = [
        [i16::MIN, i16::MAX, -1, 0, 1, 0x4000, -0x4000, 0x1234],
        [1, -1, 0x7FFE, -0x7FFF, 2, 4, 0x0101, -0x0101],
        [0x00FF, 0x7F00, -256, -2, 3, 0x5555, -0x5555, i16::MIN],
    ];
    const ACC: [i64; LANES] = [
        -(1i64 << 47),
        (1i64 << 47) - 1,
        -0x8000_0001,
        -1,
        0,
        0x7FFF_FFFF,
        0x8000_0000,
        0x1234_5678_9ABC,
    ];
    let mut state = VuState::new();
    state.regs.r[1] = VECTORS[seed % VECTORS.len()];
    state.regs.r[2] = VECTORS[(seed + 1) % VECTORS.len()];
    state.regs.r[3] = VECTORS[(seed + 2) % VECTORS.len()];
    for lane in 0..LANES {
        state.acc.set(lane, ACC[(lane + seed) & 7]);
    }
    state.flags = Flags {
        vco: [0xA55A, 0x5AA5, 0xFFFF][seed % 3],
        vcc: [0x3CC3, 0xC33C, 0x6996][seed % 3],
        vce: [0x96, 0x69, 0xFF][seed % 3],
    };
    state.div_in = [0x8000, 0x1234, 0xFFFF][seed % 3];
    state.div_in_loaded = seed & 1 != 0;
    state.div_out = [0x7FFF, 0xA5A5, 0x0001][seed % 3];
    state
}

fn invocation(e: usize) -> OpInvocation {
    OpInvocation {
        vd: 3,
        vs: 1,
        vt: 2,
        e,
        de: (e * 5 + 3) & 7,
        vs_index: e & 1,
    }
}

#[test]
fn all_44_compute_ops_match_reference_at_instruction_boundaries() {
    for op in ALL_VU_OPS {
        let elements: Vec<usize> = match op {
            VuOp::Vmacq | VuOp::Vnop => vec![0],
            _ => (0..16).collect(),
        };
        for seed in 0..3 {
            for &e in &elements {
                let mut actual = seeded_state(seed);
                let mut reference = ReferenceVu::capture(&actual);
                let inv = invocation(e);
                assert_eq!(dispatch(&mut actual, op, inv), OpStatus::Executed);
                reference.exec(op, inv);
                reference.assert_matches(&actual, &format!("{op:?} seed={seed} e={e}"));
            }
        }
    }
}

#[test]
fn reciprocal_and_rsqrt_match_at_signed_16_and_32_bit_boundaries() {
    let scalar_inputs = [i16::MIN, i16::MIN + 1, -1, 0, 1, i16::MAX];
    for (case, input) in scalar_inputs.into_iter().enumerate() {
        for op in [VuOp::Vrcp, VuOp::Vrsq] {
            let mut actual = seeded_state(case % 3);
            actual.regs.r[2][0] = input;
            let mut reference = ReferenceVu::capture(&actual);
            let inv = invocation(0);
            assert_eq!(dispatch(&mut actual, op, inv), OpStatus::Executed);
            reference.exec(op, inv);
            reference.assert_matches(&actual, &format!("{op:?} input={input}"));
        }
    }

    let paired_inputs = [
        i32::MIN,
        i32::MIN + 1,
        -0x1_0001,
        -0x1_0000,
        -1,
        0,
        1,
        0xFFFF,
        0x1_0000,
        i32::MAX,
    ];
    for (case, input) in paired_inputs.into_iter().enumerate() {
        for (high_op, low_op) in [(VuOp::Vrcph, VuOp::Vrcpl), (VuOp::Vrsqh, VuOp::Vrsql)] {
            let mut actual = seeded_state(case % 3);
            actual.regs.r[2][0] = (input >> 16) as i16;
            let mut reference = ReferenceVu::capture(&actual);
            let inv = invocation(0);
            assert_eq!(dispatch(&mut actual, high_op, inv), OpStatus::Executed);
            reference.exec(high_op, inv);
            reference.assert_matches(&actual, &format!("{high_op:?} input={input}"));

            actual.regs.r[2][0] = input as i16;
            reference.regs[2][0] = input as i16;
            assert_eq!(dispatch(&mut actual, low_op, inv), OpStatus::Executed);
            reference.exec(low_op, inv);
            reference.assert_matches(&actual, &format!("{low_op:?} input={input}"));
        }
    }
}

#[test]
fn dependent_flag_accumulator_and_divider_streams_match_after_every_step() {
    let streams: &[&[(VuOp, OpInvocation)]] = &[
        &[
            (VuOp::Vsubc, invocation(0)),
            (VuOp::Vlt, invocation(3)),
            (VuOp::Vch, invocation(8)),
            (VuOp::Vcl, invocation(8)),
            (VuOp::Vcr, invocation(15)),
            (VuOp::Vmrg, invocation(4)),
        ],
        &[
            (VuOp::Vmulf, invocation(2)),
            (VuOp::Vmacf, invocation(7)),
            (VuOp::Vmadh, invocation(12)),
            (VuOp::Vmacq, invocation(0)),
            (VuOp::Vsar, invocation(8)),
            (VuOp::Vsar, invocation(9)),
            (VuOp::Vsar, invocation(10)),
        ],
        &[
            (VuOp::Vrcp, invocation(8)),
            (VuOp::Vrcph, invocation(9)),
            (VuOp::Vrcpl, invocation(10)),
            (VuOp::Vrsqh, invocation(11)),
            (VuOp::Vrsql, invocation(15)),
            (VuOp::Vrsq, invocation(12)),
        ],
    ];
    for (stream_index, stream) in streams.iter().enumerate() {
        let mut actual = seeded_state(stream_index);
        let mut reference = ReferenceVu::capture(&actual);
        for (step, &(op, inv)) in stream.iter().enumerate() {
            assert_eq!(dispatch(&mut actual, op, inv), OpStatus::Executed);
            reference.exec(op, inv);
            reference.assert_matches(
                &actual,
                &format!("stream={stream_index} step={step} {op:?}"),
            );
        }
    }
}

#[derive(Clone)]
struct ReferenceMemory {
    regs: [[i16; LANES]; 32],
    dmem: [u8; 0x1000],
}

impl ReferenceMemory {
    fn byte(&self, address: u32) -> u8 {
        self.dmem[address as usize & 0xFFF]
    }

    fn write_byte(&mut self, address: u32, value: u8) {
        self.dmem[address as usize & 0xFFF] = value;
    }

    fn load(&mut self, op: VLoadOp, vt: usize, e: usize, base: u32, off: i16) {
        let scale = match op {
            VLoadOp::Lbv => 1,
            VLoadOp::Lsv => 2,
            VLoadOp::Llv => 4,
            VLoadOp::Ldv => 8,
            VLoadOp::Lpv | VLoadOp::Luv => 8,
            _ => 16,
        };
        let address = base.wrapping_add((i32::from(off) * scale) as u32);
        let mut bytes = vector_bytes(self.regs[vt]);
        match op {
            VLoadOp::Lbv | VLoadOp::Lsv | VLoadOp::Llv | VLoadOp::Ldv => {
                let count = match op {
                    VLoadOp::Lbv => 1,
                    VLoadOp::Lsv => 2,
                    VLoadOp::Llv => 4,
                    VLoadOp::Ldv => 8,
                    _ => unreachable!(),
                };
                for index in 0..count {
                    bytes[(e + index) & 15] = self.byte(address + index as u32);
                }
                self.regs[vt] = vector_from_bytes(bytes);
            }
            VLoadOp::Lqv => {
                let end = (e + 16 - (address as usize & 15)).min(16);
                for (index, destination) in (e..end).enumerate() {
                    bytes[destination] = self.byte(address + index as u32);
                }
                self.regs[vt] = vector_from_bytes(bytes);
            }
            VLoadOp::Lrv => {
                let mut destination = 16usize.wrapping_sub((address as usize & 15).wrapping_sub(e));
                let mut source = address & !15;
                while destination < 16 {
                    bytes[destination] = self.byte(source);
                    destination += 1;
                    source += 1;
                }
                self.regs[vt] = vector_from_bytes(bytes);
            }
            VLoadOp::Lpv | VLoadOp::Luv => {
                for lane in 0..LANES {
                    let source = address + ((16 - e + lane) & 15) as u32;
                    self.regs[vt][lane] =
                        i16::from(self.byte(source)) << if op == VLoadOp::Lpv { 8 } else { 7 };
                }
            }
            VLoadOp::Lhv => {
                for lane in 0..LANES {
                    let source = address + ((16 - e + lane * 2) & 15) as u32;
                    self.regs[vt][lane] = i16::from(self.byte(source)) << 7;
                }
            }
            VLoadOp::Lfv => {
                for index in 0..4 {
                    self.regs[vt][e / 2 + index] =
                        i16::from(self.byte(address + (index * 4) as u32)) << 7;
                }
            }
            VLoadOp::Ltv => {
                let mut source = address.wrapping_add(8) & !15;
                for register in vt..(vt + LANES).min(32) {
                    let element = ((8usize.wrapping_sub(e / 2) + register - vt) * 2) & 15;
                    let mut destination = vector_bytes(self.regs[register]);
                    destination[element] = self.byte(source);
                    destination[(element + 1) & 15] = self.byte(source + 1);
                    self.regs[register] = vector_from_bytes(destination);
                    source += 2;
                }
            }
        }
    }

    fn store(&mut self, op: VStoreOp, vt: usize, e: usize, base: u32, off: i16) {
        let scale = match op {
            VStoreOp::Sbv => 1,
            VStoreOp::Ssv => 2,
            VStoreOp::Slv => 4,
            VStoreOp::Sdv => 8,
            VStoreOp::Spv | VStoreOp::Suv => 8,
            _ => 16,
        };
        let address = base.wrapping_add((i32::from(off) * scale) as u32);
        let bytes = vector_bytes(self.regs[vt]);
        match op {
            VStoreOp::Sbv | VStoreOp::Ssv | VStoreOp::Slv | VStoreOp::Sdv => {
                let count = match op {
                    VStoreOp::Sbv => 1,
                    VStoreOp::Ssv => 2,
                    VStoreOp::Slv => 4,
                    VStoreOp::Sdv => 8,
                    _ => unreachable!(),
                };
                for index in 0..count {
                    self.write_byte(address + index as u32, bytes[(e + index) & 15]);
                }
            }
            VStoreOp::Sqv => {
                for index in 0..16 - (address as usize & 15) {
                    self.write_byte(address + index as u32, bytes[(e + index) & 15]);
                }
            }
            VStoreOp::Srv => {
                let count = address as usize & 15;
                let source = (16 - count + e) & 15;
                let row = address & !15;
                for index in 0..count {
                    self.write_byte(row + index as u32, bytes[(source + index) & 15]);
                }
            }
            VStoreOp::Spv | VStoreOp::Suv => {
                for index in 0..LANES {
                    let element = (e + index) & 15;
                    let lane = self.regs[vt][element & 7] as u16;
                    let high = matches!(
                        (op, element < 8),
                        (VStoreOp::Spv, true) | (VStoreOp::Suv, false)
                    );
                    self.write_byte(
                        address + index as u32,
                        if high {
                            (lane >> 8) as u8
                        } else {
                            (lane >> 7) as u8
                        },
                    );
                }
            }
            VStoreOp::Shv => {
                for index in 0..LANES {
                    let first = bytes[(e + index * 2) & 15];
                    let second = bytes[(e + index * 2 + 1) & 15];
                    self.write_byte(address + (index * 2) as u32, (first << 1) | (second >> 7));
                }
            }
            VStoreOp::Sfv => {
                let row = address & !15;
                let mut row_offset = address & 15;
                for lane in e / 2..e / 2 + 4 {
                    self.write_byte(
                        row + (row_offset & 15),
                        (self.regs[vt][lane] as u16 >> 7) as u8,
                    );
                    row_offset += 4;
                }
            }
            VStoreOp::Swv => {
                let row = address & !15;
                let row_offset = address & 15;
                for index in 0..16 {
                    self.write_byte(
                        row + ((row_offset + index) & 15),
                        bytes[(e + index as usize) & 15],
                    );
                }
            }
            VStoreOp::Stv => {
                let row = address & !15;
                let first_element = 8usize.wrapping_sub(e / 2);
                let mut row_offset = (address & 15) + (first_element * 2) as u32;
                for (offset, register) in (vt..(vt + LANES).min(32)).enumerate() {
                    let element = first_element + offset;
                    let value = self.regs[register][element & 7] as u16;
                    self.write_byte(row + (row_offset & 15), (value >> 8) as u8);
                    self.write_byte(row + ((row_offset + 1) & 15), value as u8);
                    row_offset += 2;
                }
            }
        }
    }
}

fn vector_bytes(vector: [i16; LANES]) -> [u8; 16] {
    let mut bytes = [0; 16];
    for lane in 0..LANES {
        bytes[lane * 2] = (vector[lane] as u16 >> 8) as u8;
        bytes[lane * 2 + 1] = vector[lane] as u8;
    }
    bytes
}

fn vector_from_bytes(bytes: [u8; 16]) -> [i16; LANES] {
    let mut vector = [0; LANES];
    for lane in 0..LANES {
        vector[lane] = i16::from_be_bytes([bytes[lane * 2], bytes[lane * 2 + 1]]);
    }
    vector
}

fn memory_fixture() -> ReferenceMemory {
    let mut reference = ReferenceMemory {
        regs: [[0; LANES]; 32],
        dmem: [0; 0x1000],
    };
    for (address, byte) in reference.dmem.iter_mut().enumerate() {
        *byte = (address as u8).wrapping_mul(73).wrapping_add(0x5D);
    }
    for register in 0..32 {
        for lane in 0..LANES {
            reference.regs[register][lane] =
                ((register as u16) << 11 | (lane as u16) << 8 | (0xA5 ^ lane as u16)) as i16;
        }
    }
    reference
}

fn actual_machine<'a>(reference: &ReferenceMemory, rdram: &'a mut [u8]) -> RspMachine<'a> {
    let mut machine = RspMachine::new(rdram);
    machine.ctx.rsp.regs.r = reference.regs;
    for address in 0..0x1000u32 {
        machine.dmem.write_bu(address, reference.byte(address));
    }
    machine
}

fn assert_memory_matches(machine: &RspMachine<'_>, reference: &ReferenceMemory, label: &str) {
    assert_eq!(
        machine.ctx.rsp.regs.r, reference.regs,
        "{label}: register file"
    );
    for address in 0..0x1000u32 {
        assert_eq!(
            machine.dmem.read_bu(address),
            reference.byte(address),
            "{label}: DMEM {address:#05x}"
        );
    }
}

#[test]
fn all_23_vector_load_store_ops_match_reference_at_alignment_boundaries() {
    let loads = [
        VLoadOp::Lbv,
        VLoadOp::Lsv,
        VLoadOp::Llv,
        VLoadOp::Ldv,
        VLoadOp::Lqv,
        VLoadOp::Lrv,
        VLoadOp::Lpv,
        VLoadOp::Luv,
        VLoadOp::Lhv,
        VLoadOp::Lfv,
        VLoadOp::Ltv,
    ];
    let stores = [
        VStoreOp::Sbv,
        VStoreOp::Ssv,
        VStoreOp::Slv,
        VStoreOp::Sdv,
        VStoreOp::Sqv,
        VStoreOp::Srv,
        VStoreOp::Spv,
        VStoreOp::Suv,
        VStoreOp::Shv,
        VStoreOp::Sfv,
        VStoreOp::Swv,
        VStoreOp::Stv,
    ];
    let addresses = [0x100, 0x101, 0x107, 0x108, 0x10F, 0xFFF];
    for op in loads {
        let elements: &[usize] = match op {
            VLoadOp::Lbv => &[0, 7, 15],
            VLoadOp::Lsv => &[0, 7, 14],
            VLoadOp::Llv => &[0, 6, 12],
            VLoadOp::Ldv => &[0, 4, 8],
            VLoadOp::Lfv => &[0, 8],
            VLoadOp::Ltv => &[0, 2, 8, 14],
            _ => &[0, 1, 7, 8, 15],
        };
        let registers: &[usize] = if op == VLoadOp::Ltv { &[8, 29] } else { &[8] };
        for &vt in registers {
            for &e in elements {
                for &address in &addresses {
                    let mut reference = memory_fixture();
                    let mut rdram = [0; 1];
                    let mut actual = actual_machine(&reference, &mut rdram);
                    actual.vload(op, vt as u8, e as u8, address, 0);
                    reference.load(op, vt, e, address, 0);
                    assert_memory_matches(
                        &actual,
                        &reference,
                        &format!("{op:?} vt={vt} e={e} addr={address:#x}"),
                    );
                }
            }
        }
    }
    for op in stores {
        let elements: &[usize] = match op {
            VStoreOp::Sbv => &[0, 7, 15],
            VStoreOp::Ssv => &[0, 7, 14],
            VStoreOp::Slv => &[0, 6, 12],
            VStoreOp::Sdv => &[0, 4, 8],
            VStoreOp::Sfv => &[0, 8],
            VStoreOp::Stv => &[0, 2, 8, 14],
            _ => &[0, 1, 7, 8, 15],
        };
        let registers: &[usize] = if op == VStoreOp::Stv { &[8, 29] } else { &[8] };
        for &vt in registers {
            for &e in elements {
                for &address in &addresses {
                    let mut reference = memory_fixture();
                    let mut rdram = [0; 1];
                    let mut actual = actual_machine(&reference, &mut rdram);
                    actual.vstore(op, vt as u8, e as u8, address, 0);
                    reference.store(op, vt, e, address, 0);
                    assert_memory_matches(
                        &actual,
                        &reference,
                        &format!("{op:?} vt={vt} e={e} addr={address:#x}"),
                    );
                }
            }
        }
    }
}
