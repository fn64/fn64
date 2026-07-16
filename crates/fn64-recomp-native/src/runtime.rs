//! The typed runtime that emitted Rust targets: [`RecompContext`] (the CPU
//! register file) and [`Rdram`] (a checked memory view).
//!
//! # Why this exists (the whole point of `-native`)
//!
//! The N64Recomp C output reaches memory through raw macros like
//! `*(int16_t*)(rdram + (((reg + off) ^ 2) - 0x…80000000))` — a pointer cast
//! and a hand-written byte swizzle at every access. That is exactly the
//! byte-reinterpret bug class this project has been paying for. Here the
//! swizzle lives in ONE place, expressed as safe indexing on a `&mut [u8]`,
//! and every emitted access goes through a *typed method* (`load_w`,
//! `store_h`, …). No emitted code ever casts a pointer. `#![forbid(unsafe_code)]`
//! at the crate root makes that structural, not merely a convention.
//!
//! # Semantic model (matches N64Recomp's `recomp.h`, clean-room from the ISA)
//!
//! - A GPR is a 64-bit value (`gpr = uint64_t` in the C). 32-bit results are
//!   sign-extended into it (that is what `S32`/`ADD32` do). We store GPRs as
//!   `u64` and expose typed read/write helpers.
//! - A KSEG0/KSEG1 virtual address `v` maps to rdram byte offset
//!   `v - 0xFFFF_FFFF_8000_0000` (i.e. strip the sign-extended `0x8000_0000`
//!   base). This is the `- 0xFFFFFFFF80000000` in the C macros.
//! - Word accesses use the host-native representation used by the ABI buffer;
//!   sub-word accesses XOR the byte offset: halfword `^2`, byte `^3`. This is
//!   the N64's big-endian view over a little-endian host buffer. It is applied
//!   here in one spot, in [`Rdram`].

/// The recompiled-CPU register context: 32 general-purpose registers plus the
/// HI/LO multiply-divide pair. `$zero` (index 0) reads as 0 and ignores writes.
///
/// GPRs are stored as `u64` to hold the sign-extended 64-bit values MIPS
/// keeps; the typed accessors ([`RecompContext::r`], [`RecompContext::set_r32`],
/// …) enforce the sign/zero-extension contract so emitted code never open-codes
/// a cast.
#[derive(Clone, Debug, Default)]
pub struct RecompContext {
    /// r[0] is `$zero`; kept in the array for uniform indexing but never
    /// observably nonzero (writes go through [`RecompContext::set_r`], which
    /// drops index 0).
    r: [u64; 32],
    /// The HI result register of MULT/DIV.
    pub hi: u64,
    /// The LO result register of MULT/DIV.
    pub lo: u64,

    /// The COP1 (FPU) register file, stored as 32 raw 64-bit slots. See the
    /// [`RecompContext`] FPU accessors for how the FR=0 even/odd pairing maps a
    /// single-precision register index onto these slots — this mirrors
    /// `fn64-abi`'s `f_odd` model (an odd `$fN` aliases the HIGH 32-bit word of
    /// its even partner `$f(N-1)`) so the byte layout matches the recompiled C
    /// exactly. We keep raw bits (not `f32`/`f64`) so a bit-copy `MOV`/`MTC1`
    /// never perturbs a NaN payload and the aliasing is pure integer indexing.
    fpr: [u64; 32],
    /// The FPU condition flag (FCSR bit 23). Set by the `C.cond.fmt` compares,
    /// tested by `BC1T`/`BC1F`. This is N64Recomp's per-function `c1cs`
    /// promoted to context state (equivalent: a compare always precedes the
    /// branch that reads it, so lifetime is irrelevant to the result).
    pub fpu_cond: bool,
    /// FCSR bits other than condition bit 23, which is kept in `fpu_cond` so
    /// generated branch code can read it directly. VR4300 User's Manual
    /// section 6.3.2.2 defines FS(24), Cause(17:12), Enables(11:7),
    /// Flags(6:2), and RM(1:0); reserved bits read as zero.
    fcsr: u32,
    /// Address/width of the most recent LL/LLD reservation. There is only one
    /// architectural LLbit. A mismatched SC/SCD must fail and clear it.
    ll_reservation: Option<(u64, u8)>,
    /// COP0 register 9, `Count`: the free-running cycle counter that backs
    /// `osGetCount`. It is the one COP0 read a recompiled body legitimately
    /// performs (`MFC0 rt, $9`); the host advances it. Modeled as real state
    /// rather than trapped, unlike the libultra-managed Status/Cause/EPC.
    pub cop0_count: u32,
    /// COP0 register 11, `Compare`: the timer-interrupt threshold written via
    /// `MTC0 rt, $11` on the `osSetTimer` path. Stored so the write round-trips;
    /// the interrupt it would schedule is the host's concern.
    pub cop0_compare: u32,
    /// COP0 condition bit used by BC0*. On VR4300 this reflects Status.CH.
    /// CACHE tag operations are host-modeled, so callers that exercise BC0
    /// explicitly supply the observed condition through this field.
    pub cop0_cond: bool,
    /// COP0 Status (register 12). Privileged libultra entry points are
    /// host-bound, but their typed adapters still need per-OSThread status
    /// state for `__osGetSR`/`__osSetSR` and interrupt-mask round trips.
    pub cop0_status: u32,
    /// COP0 Cause (register 13). The coroutine executor delivers events at
    /// explicit yield points rather than synthesizing CPU exceptions, so the
    /// normal value is zero; keeping the field makes `__osGetCause` an honest
    /// state read instead of a fabricated constant.
    pub cop0_cause: u32,
}

impl RecompContext {
    /// A fresh context with all registers zeroed.
    pub fn new() -> Self {
        RecompContext::default()
    }

    /// Read GPR `idx` as a full 64-bit value. `$zero` reads 0.
    #[inline]
    pub fn r(&self, idx: u8) -> u64 {
        self.r[idx as usize]
    }

    /// Read GPR `idx` as a signed 32-bit value (the low word).
    #[inline]
    pub fn r_s32(&self, idx: u8) -> i32 {
        self.r[idx as usize] as u32 as i32
    }

    /// Read GPR `idx` as an unsigned 32-bit value (the low word).
    #[inline]
    pub fn r_u32(&self, idx: u8) -> u32 {
        self.r[idx as usize] as u32
    }

    /// Read GPR `idx` as a signed 64-bit value. This is the `SIGNED(reg)` /
    /// `ToS64` operand of the C oracle — MIPS III compares (SLT/SLTI, and the
    /// single-operand branches) operate on the full 64-bit register.
    #[inline]
    pub fn r_s64(&self, idx: u8) -> i64 {
        self.r[idx as usize] as i64
    }

    /// Read GPR `idx` as an unsigned 64-bit value (`ToU64`, for SLTU/SLTIU).
    #[inline]
    pub fn r_u64(&self, idx: u8) -> u64 {
        self.r[idx as usize]
    }

    /// Write a raw 64-bit value into GPR `idx`. Writes to `$zero` are dropped,
    /// upholding the hardwired-zero contract.
    #[inline]
    pub fn set_r(&mut self, idx: u8, val: u64) {
        if idx != 0 {
            self.r[idx as usize] = val;
        }
    }

    /// Snapshot all architectural GPRs for the audited native/C ABI adapter.
    /// The returned copy preserves `$zero == 0` without exposing the backing
    /// array for unchecked mutation.
    pub fn gprs(&self) -> [u64; 32] {
        self.r
    }

    /// Restore a GPR snapshot after an fn64 host shim returns. `$zero` is
    /// forced back to zero even if a foreign ABI context contained garbage.
    pub fn set_gprs(&mut self, mut regs: [u64; 32]) {
        regs[0] = 0;
        self.r = regs;
    }

    /// Write a 32-bit result into GPR `idx`, sign-extending into the 64-bit
    /// register (the universal MIPS III rule for 32-bit ops: the result's
    /// bit 31 fills bits 63..32). This is the typed replacement for the C
    /// `S32(...)`/`ADD32(...)` casts.
    #[inline]
    pub fn set_r32(&mut self, idx: u8, val: i32) {
        self.set_r(idx, val as i64 as u64);
    }

    /// Read FCR0/FCR31. The VR4300 implements only those two control
    /// registers (User's Manual section 6.3.2); reserved FCRs read as zero.
    #[inline]
    pub fn read_fcr(&self, idx: u8) -> u32 {
        match idx {
            // VR4300 implementation number 0x0B, revision zero.
            0 => 0x0000_0B00,
            31 => (self.fcsr & !(1 << 23)) | ((self.fpu_cond as u32) << 23),
            _ => panic!("reserved COP1 control register FCR{idx}"),
        }
    }

    /// Write FCR31. Writes to FCR0/reserved FCRs have no architectural
    /// effect. Reserved bits are discarded rather than becoming hidden state.
    #[inline]
    pub fn write_fcr(&mut self, idx: u8, value: u32) {
        assert_eq!(
            idx, 31,
            "write to read-only/reserved COP1 control register FCR{idx}"
        );
        const WRITABLE: u32 = (1 << 24) | (1 << 23) | 0x0003_FFFF;
        self.fpu_cond = value & (1 << 23) != 0;
        self.fcsr = value & WRITABLE & !(1 << 23);
    }

    /// Establish the single architectural LLbit reservation.
    #[inline]
    pub fn set_ll_reservation(&mut self, vaddr: u64, width: u8) {
        self.ll_reservation = Some((vaddr, width));
    }

    /// Test and clear the LLbit for SC/SCD. The VR4300 User's Manual LL/SC
    /// descriptions require the linked access to target the same block; this
    /// typed runtime uses the stricter same-address/same-width condition.
    #[inline]
    pub fn take_ll_reservation(&mut self, vaddr: u64, width: u8) -> bool {
        self.ll_reservation.take() == Some((vaddr, width))
    }

    /// VR4300 signed word division, including the implementation-defined
    /// divide-by-zero results documented in User's Manual appendix D.2.
    pub fn div_s32(&mut self, dividend: i32, divisor: i32) {
        if divisor == 0 {
            self.lo = if dividend < 0 {
                0xFFFF_FFFF_8000_0001
            } else {
                0x0000_0000_7FFF_FFFF
            };
            self.hi = dividend as i64 as u64;
        } else {
            self.lo = dividend.wrapping_div(divisor) as i64 as u64;
            self.hi = dividend.wrapping_rem(divisor) as i64 as u64;
        }
    }

    /// VR4300 unsigned word division. Word HI/LO results are sign-extended,
    /// including the all-ones quotient on divide by zero.
    pub fn div_u32(&mut self, dividend: u32, divisor: u32) {
        if let Some(quotient) = dividend.checked_div(divisor) {
            self.lo = quotient as i32 as i64 as u64;
            self.hi = (dividend % divisor) as i32 as i64 as u64;
        } else {
            self.lo = u64::MAX;
            self.hi = (dividend as i32) as i64 as u64;
        }
    }

    /// Signed doubleword division. INT64_MIN/-1 produces the architectural
    /// wrapped quotient and zero remainder. The public VR4300 appendix prints
    /// only word-sized divide-by-zero results, so DDIV-by-zero traps loudly
    /// rather than inventing a 64-bit constant.
    pub fn div_s64(&mut self, dividend: i64, divisor: i64) {
        assert_ne!(
            divisor, 0,
            "DDIV by zero: result is not specified by the public VR4300 manual"
        );
        if dividend == i64::MIN && divisor == -1 {
            self.lo = dividend as u64;
            self.hi = 0;
        } else {
            self.lo = dividend.wrapping_div(divisor) as u64;
            self.hi = dividend.wrapping_rem(divisor) as u64;
        }
    }

    /// Unsigned doubleword division. See [`RecompContext::div_s64`] for why a
    /// zero divisor is a loud uncertainty trap.
    pub fn div_u64(&mut self, dividend: u64, divisor: u64) {
        assert_ne!(
            divisor, 0,
            "DDIVU by zero: result is not specified by the public VR4300 manual"
        );
        self.lo = dividend / divisor;
        self.hi = dividend % divisor;
    }

    /// Convert a floating value to an integer using FCSR.RM (or a fixed mode
    /// for ROUND/TRUNC/CEIL/FLOOR). Cause bits are per-operation and Flag bits
    /// accumulate as specified by VR4300 User's Manual section 6.3.2.2.
    pub fn fpu_to_i32(&mut self, value: f64, fixed_mode: Option<u8>) -> i32 {
        let rounded = self.round_for_mode(value, fixed_mode);
        if !rounded.is_finite() || !(-2_147_483_648.0..2_147_483_648.0).contains(&rounded) {
            self.raise_fpu(4);
            i32::MIN
        } else {
            if rounded != value {
                self.raise_fpu(0);
            }
            rounded as i32
        }
    }

    /// 64-bit counterpart of [`RecompContext::fpu_to_i32`].
    pub fn fpu_to_i64(&mut self, value: f64, fixed_mode: Option<u8>) -> i64 {
        let rounded = self.round_for_mode(value, fixed_mode);
        // i64::MAX is not exactly representable as f64; 2^63 itself is the
        // exclusive upper bound, while -2^63 is representable and valid.
        if !rounded.is_finite()
            || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&rounded)
        {
            self.raise_fpu(4);
            i64::MIN
        } else {
            if rounded != value {
                self.raise_fpu(0);
            }
            rounded as i64
        }
    }

    #[inline]
    fn round_for_mode(&mut self, value: f64, fixed_mode: Option<u8>) -> f64 {
        // Cause is rewritten by every arithmetic/conversion operation.
        self.fcsr &= !(0x3F << 12);
        match fixed_mode.unwrap_or((self.fcsr & 3) as u8) {
            0 => value.round_ties_even(),
            1 => value.trunc(),
            2 => value.ceil(),
            3 => value.floor(),
            _ => unreachable!("FCSR.RM and fixed rounding modes are two bits"),
        }
    }

    #[inline]
    fn raise_fpu(&mut self, exception: u8) {
        // exception 0..4 = Inexact, Underflow, Overflow, Divide-by-zero,
        // Invalid. Cause adds bit 12; sticky Flag adds bit 2.
        self.fcsr |= 1 << (12 + exception);
        self.fcsr |= 1 << (2 + exception);
        if self.fcsr & (1 << (7 + exception)) != 0 {
            panic!("enabled COP1 exception {}", exception);
        }
    }

    /// Evaluate any of the sixteen C.cond.fmt predicates. The low three funct
    /// bits select unordered/equal/less participation; bit 3 selects signaling
    /// behavior. Quiet compares still signal on an SNaN.
    pub fn fpu_compare(&mut self, lhs: f64, rhs: f64, lhs_snan: bool, rhs_snan: bool, cond: u8) {
        self.fcsr &= !(0x3F << 12);
        let unordered = lhs.is_nan() || rhs.is_nan();
        if (unordered && cond & 0x8 != 0) || lhs_snan || rhs_snan {
            self.raise_fpu(4);
        }
        self.fpu_cond = (unordered && cond & 1 != 0)
            || (!unordered && lhs == rhs && cond & 2 != 0)
            || (!unordered && lhs < rhs && cond & 4 != 0);
    }

    #[inline]
    pub fn fpu_compare_s(&mut self, fs: u8, ft: u8, cond: u8) {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        self.fpu_compare(
            f32::from_bits(a) as f64,
            f32::from_bits(b) as f64,
            is_snan32(a),
            is_snan32(b),
            cond,
        );
    }

    #[inline]
    pub fn fpu_compare_d(&mut self, fs: u8, ft: u8, cond: u8) {
        let a = self.d_bits(fs);
        let b = self.d_bits(ft);
        self.fpu_compare(
            f64::from_bits(a),
            f64::from_bits(b),
            is_snan64(a),
            is_snan64(b),
            cond,
        );
    }

    // ================================================================
    // COP1 / FPU register file.
    //
    // # The FR=0 even/odd pairing (the whole reason these aren't 32 plain f32s)
    //
    // libultra boots every OSThread with the FPU in FR=0 mode: 16 paired
    // 64-bit FGRs addressed by even register numbers. In that mode a 64-bit
    // value (a double, or a `ldc1`/`sdc1`/`dmtc1` slot)
    // lives in an *even* register `$f(2k)`, and the odd register `$f(2k+1)`
    // is NOT independent — it aliases the HIGH 32 bits of that same 64-bit
    // slot. A single-precision `$f(2k+1)` therefore reads/writes the top word
    // of `$f(2k)`. This is exactly the `f_odd[(N-1)*2]` addressing the
    // recompiled C uses (`fn64-abi::RecompContext::arm_fpr_alias`).
    //
    // We store the file as 32 `u64` slots and resolve a single-precision index
    // to (slot, is_high_word). Even N -> (N, low word); odd N -> (N-1, high
    // word). Double/64-bit ops address the even slot directly. All accessors
    // move raw *bits* (`f32::from_bits`/`to_bits`), never a lossy cast, so a
    // `MOV.S`/`MTC1` of a signalling-NaN pattern is preserved bit-exactly.
    // ================================================================

    /// The 64-bit slot and whether the low (false) or high (true) 32-bit word
    /// holds single-precision register `idx` under FR=0.
    #[inline]
    fn fpr_single_slot(idx: u8) -> (usize, bool) {
        if idx & 1 == 0 {
            (idx as usize, false)
        } else {
            ((idx - 1) as usize, true)
        }
    }

    /// Read single-precision FPR `idx` as raw 32 bits.
    #[inline]
    pub fn f_bits(&self, idx: u8) -> u32 {
        let (slot, high) = Self::fpr_single_slot(idx);
        if high {
            (self.fpr[slot] >> 32) as u32
        } else {
            self.fpr[slot] as u32
        }
    }

    /// Write raw 32 bits into single-precision FPR `idx` (leaving the paired
    /// word of the even slot untouched).
    #[inline]
    pub fn set_f_bits(&mut self, idx: u8, bits: u32) {
        let (slot, high) = Self::fpr_single_slot(idx);
        if high {
            self.fpr[slot] = (self.fpr[slot] & 0x0000_0000_FFFF_FFFF) | ((bits as u64) << 32);
        } else {
            self.fpr[slot] = (self.fpr[slot] & 0xFFFF_FFFF_0000_0000) | (bits as u64);
        }
    }

    /// Read single-precision FPR `idx` as an `f32`.
    #[inline]
    pub fn f_s(&self, idx: u8) -> f32 {
        f32::from_bits(self.f_bits(idx))
    }

    /// Write an `f32` into single-precision FPR `idx`.
    #[inline]
    pub fn set_f_s(&mut self, idx: u8, val: f32) {
        self.set_f_bits(idx, val.to_bits());
    }

    /// Read a doubleword (double / `dmtc1` / `ldc1`) FPR `idx` as raw 64 bits.
    /// Under FR=0 these use even registers; we index the slot directly.
    #[inline]
    pub fn d_bits(&self, idx: u8) -> u64 {
        assert_eq!(idx & 1, 0, "FR=0 doubleword read from odd FPR f{idx}");
        self.fpr[idx as usize]
    }

    /// Write raw 64 bits into doubleword FPR `idx`.
    #[inline]
    pub fn set_d_bits(&mut self, idx: u8, bits: u64) {
        assert_eq!(idx & 1, 0, "FR=0 doubleword write to odd FPR f{idx}");
        self.fpr[idx as usize] = bits;
    }

    /// Read double-precision FPR `idx` as an `f64`.
    #[inline]
    pub fn f_d(&self, idx: u8) -> f64 {
        f64::from_bits(self.d_bits(idx))
    }

    /// Write an `f64` into double-precision FPR `idx`.
    #[inline]
    pub fn set_f_d(&mut self, idx: u8, val: f64) {
        self.set_d_bits(idx, val.to_bits());
    }
}

#[inline]
fn is_snan32(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x007F_FFFF != 0 && bits & 0x0040_0000 == 0
}

#[inline]
fn is_snan64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000
        && bits & 0x000F_FFFF_FFFF_FFFF != 0
        && bits & 0x0008_0000_0000_0000 == 0
}

/// Round an `f32` to the nearest integer, ties to even — the FPU's default
/// (FCSR round-to-nearest) rounding mode, which every OoT thread boots into.
/// This is the `CVT.W.S`/`CVT.L.S` rounding: N64Recomp routes it through
/// `lrintf` under the C default rounding mode (round-to-nearest-even). Rust's
/// [`f32::round_ties_even`] is exactly that, with no global FP-environment
/// dependency. Returned as `f64` so the caller's `as i32`/`as i64` truncation
/// of an already-integral value is exact.
#[inline]
pub fn round_ties_even_f32(v: f32) -> f64 {
    v.round_ties_even() as f64
}

/// Round an `f64` to the nearest integer, ties to even (the `CVT.W.D`/
/// `CVT.L.D` rounding; see [`round_ties_even_f32`]).
#[inline]
pub fn round_ties_even_f64(v: f64) -> f64 {
    v.round_ties_even()
}

/// Number of bytes of rdram the N64 exposes (8 MiB with the Expansion Pak,
/// which is what recompiled titles assume). The checked accessors bound every
/// access against this.
pub const RDRAM_LEN: usize = 8 * 1024 * 1024;

/// The base virtual address that maps to rdram offset 0 (KSEG0). Sign-extended
/// to 64 bits, this is the `0xFFFF_FFFF_8000_0000` the C macros subtract.
pub const RDRAM_VBASE: u64 = 0xFFFF_FFFF_8000_0000;

/// A checked view over rdram. All emitted memory accesses go through these
/// typed methods; the address translation and the big-endian sub-word swizzle
/// live here and nowhere else.
pub struct Rdram<'a> {
    mem: &'a mut [u8],
}

/// The common signature of every typed-Rust recompiled function.
///
/// This is the safe-Rust equivalent of N64Recomp's MIT-licensed
/// `recomp_func_t = void(uint8_t *rdram, recomp_context *ctx)`
/// (`refs/N64RecompSource/include/recomp.h:443-451`). The three explicit
/// higher-ranked lifetimes keep the context borrow, the `Rdram` view borrow,
/// and the underlying byte-slice borrow independent; no pointer conversion or
/// lifetime erasure is involved.
pub type RecompFunc =
    for<'ctx, 'view, 'rdram> fn(&'ctx mut RecompContext, &'view mut Rdram<'rdram>);

/// Host lookup hook used for functions that must be supplied by the runtime
/// instead of executing a recompiled body (libultra shims, exception/TLB
/// handling, and other host-owned boundaries).
pub type HostLookup = fn(u32) -> Option<RecompFunc>;
/// Cooperative-yield hook for the N64Recomp `pause_self` self-loop rule.
pub type HostPause = fn();

thread_local! {
    /// Native execution is single-threaded by design (`docs/DESIGN.md` section
    /// 2), so the override belongs to the executing host thread. A
    /// thread-local `Cell` also lets tests install an isolated resolver
    /// without unsafe global mutation or cross-test serialization.
    static HOST_LOOKUP: std::cell::Cell<Option<HostLookup>> = const {
        std::cell::Cell::new(None)
    };
    static HOST_PAUSE: std::cell::Cell<Option<HostPause>> = const {
        std::cell::Cell::new(None)
    };
}

/// Install (or clear) the current thread's host-function resolver, returning
/// the previous resolver.
///
/// Generated dispatchers consult this hook before their sorted native table.
/// A host can therefore bind a vram to a safe typed adapter over an fn64 shim;
/// vrams deliberately omitted from the native table fail loudly if the host
/// has not installed their adapter. The function-pointer seam itself is
/// entirely safe Rust: no `transmute`, raw pointer, or ABI cast is involved.
pub fn set_host_lookup(resolver: Option<HostLookup>) -> Option<HostLookup> {
    HOST_LOOKUP.with(|slot| slot.replace(resolver))
}

/// Install the host's cooperative-yield adapter for translated self-loops.
pub fn set_host_pause(pause: Option<HostPause>) -> Option<HostPause> {
    HOST_PAUSE.with(|slot| slot.replace(pause))
}

/// Yield the active emulated thread at an unconditional branch-to-self.
pub fn pause_self() {
    HOST_PAUSE.with(|slot| {
        slot.get().unwrap_or_else(|| {
            panic!("pause_self: native host installed no coroutine-yield adapter")
        })()
    });
}

/// Resolve `vram` through the current thread's host-function resolver.
#[inline]
pub fn resolve_host_function(vram: u32) -> Option<RecompFunc> {
    HOST_LOOKUP.with(|slot| slot.get().and_then(|resolver| resolver(vram)))
}

/// Invoke a statically-known native target unless the host resolver overrides
/// its vram. This is the direct-JAL counterpart of generated `lookup(vram)`:
/// libultra functions whose bodies contain no privileged instruction still
/// must enter the executor-backed host shim rather than bypassing it merely
/// because the native recompiler could translate their machine code.
#[inline]
pub fn call_host_or_native(
    vram: u32,
    native: RecompFunc,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) {
    resolve_host_function(vram).unwrap_or(native)(ctx, mem);
}

impl<'a> Rdram<'a> {
    /// Wrap a byte buffer as rdram. The buffer should be [`RDRAM_LEN`] bytes;
    /// shorter buffers simply make more addresses fall out of bounds (a loud
    /// panic on access) rather than corrupting host memory.
    pub fn new(mem: &'a mut [u8]) -> Self {
        Rdram { mem }
    }

    /// Borrow the shared backing allocation at the runtime ABI seam. Normal
    /// emitted code has no reason to use this; fn64's native host adapters use
    /// it to call the existing, audited `*_recomp` marshalling layer without
    /// allocating or copying a second RDRAM image.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.mem
    }

    /// Translate a sign-extended KSEG0/KSEG1 virtual address (the `reg + off`
    /// sum the MIPS code computes) to a physical rdram byte offset.
    #[inline]
    fn phys(vaddr: u64) -> usize {
        // Wrapping sub mirrors the C `(addr) - 0xFFFFFFFF80000000`; the result
        // is then range-checked by the slice index in each accessor.
        vaddr.wrapping_sub(RDRAM_VBASE) as usize
    }

    /// Effective virtual address of a `off(base)` operand: full-width MIPS III
    /// addition of the 64-bit base and sign-extended 16-bit offset.
    #[inline]
    pub fn eff_addr(base_val: u64, off: i16) -> u64 {
        base_val.wrapping_add(off as i64 as u64)
    }

    // --- Aligned loads ---

    /// Load a sign-extended word. Returns the `i32` the caller sign-extends
    /// into a GPR.
    #[inline]
    pub fn load_w(&self, vaddr: u64) -> i32 {
        let p = Self::phys(vaddr);
        assert_eq!(p & 3, 0, "unaligned LW at {vaddr:#018x}");
        i32::from_ne_bytes([
            self.mem[p],
            self.mem[p + 1],
            self.mem[p + 2],
            self.mem[p + 3],
        ])
    }

    /// Load a sign-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_h(&self, vaddr: u64) -> i16 {
        assert_eq!(vaddr & 1, 0, "unaligned LH at {vaddr:#018x}");
        let p = Self::phys(vaddr) ^ 2;
        i16::from_ne_bytes([self.mem[p], self.mem[p + 1]])
    }

    /// Load a zero-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_hu(&self, vaddr: u64) -> u16 {
        self.load_h(vaddr) as u16
    }

    /// Load a sign-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_b(&self, vaddr: u64) -> i8 {
        let p = Self::phys(vaddr) ^ 3;
        self.mem[p] as i8
    }

    /// Load a zero-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_bu(&self, vaddr: u64) -> u8 {
        let p = Self::phys(vaddr) ^ 3;
        self.mem[p]
    }

    // --- Aligned stores ---

    /// Store the low word of `val`.
    #[inline]
    pub fn store_w(&mut self, vaddr: u64, val: u32) {
        let p = Self::phys(vaddr);
        assert_eq!(p & 3, 0, "unaligned SW at {vaddr:#018x}");
        self.mem[p..p + 4].copy_from_slice(&val.to_ne_bytes());
    }

    /// Store the low halfword of `val` (byte offset XOR 2).
    #[inline]
    pub fn store_h(&mut self, vaddr: u64, val: u16) {
        assert_eq!(vaddr & 1, 0, "unaligned SH at {vaddr:#018x}");
        let p = Self::phys(vaddr) ^ 2;
        self.mem[p..p + 2].copy_from_slice(&val.to_ne_bytes());
    }

    /// Store the low byte of `val` (byte offset XOR 3).
    #[inline]
    pub fn store_b(&mut self, vaddr: u64, val: u8) {
        let p = Self::phys(vaddr) ^ 3;
        self.mem[p] = val;
    }

    // --- Unaligned word loads/stores (LWL/LWR/SWL/SWR) ---
    //
    // Semantics clean-roomed from the MIPS III ISA: the pair of instructions
    // together load/store a full word straddling an alignment boundary. We
    // mirror N64Recomp's `do_lwl`/`do_lwr`/`do_swl`/`do_swr` helper math,
    // which is itself the ISA definition.

    /// Load-word-left: merge the high bytes of the addressed word into the
    /// high end of `initial` (the current register value).
    #[inline]
    pub fn load_wl(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 << (misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded << (misalign * 8))) as i32
    }

    /// Load-word-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_wr(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 >> (24 - misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded >> (24 - misalign * 8))) as i32
    }

    /// Store-word-left.
    #[inline]
    pub fn store_wl(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let initial = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    /// Store-word-right.
    #[inline]
    pub fn store_wr(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let initial = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 << (24 - misalign * 8));
        let shifted = val << (24 - misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    // --- 64-bit doubleword loads/stores (LD/SD/LLD/SCD) ---
    //
    // Clean-roomed from the MIPS III ISA and matching N64Recomp's
    // `load_doubleword`/`SD` macros exactly: a doubleword is the two 32-bit
    // words at `vaddr+0` (the high half) and `vaddr+4` (the low half). Each
    // half goes through the ordinary native-endian word path
    // (`load_w`/`store_w`) with no sub-word swizzle. Logically, the high guest
    // word remains at `vaddr+0` and the low guest word at `vaddr+4`.

    /// Load a 64-bit doubleword: `(hi_word << 32) | lo_word` where `hi_word` is
    /// at `vaddr+0` and `lo_word` at `vaddr+4`.
    #[inline]
    pub fn load_d(&self, vaddr: u64) -> u64 {
        assert_eq!(vaddr & 7, 0, "unaligned LD at {vaddr:#018x}");
        let hi = self.load_w(vaddr) as u32 as u64;
        let lo = self.load_w(vaddr.wrapping_add(4)) as u32 as u64;
        (hi << 32) | lo
    }

    /// Store a 64-bit doubleword: the high word to `vaddr+0`, the low word to
    /// `vaddr+4`.
    #[inline]
    pub fn store_d(&mut self, vaddr: u64, val: u64) {
        assert_eq!(vaddr & 7, 0, "unaligned SD at {vaddr:#018x}");
        self.store_w(vaddr, (val >> 32) as u32);
        self.store_w(vaddr.wrapping_add(4), val as u32);
    }

    // --- Unaligned doubleword loads/stores (LDL/LDR/SDL/SDR) ---
    //
    // The 64-bit analogue of LWL/LWR/SWL/SWR: the pair together moves a full
    // doubleword straddling an 8-byte boundary. Math mirrors N64Recomp's
    // `do_ldl`/`do_ldr`/`do_sdl`/`do_sdr`, which is the ISA definition. The
    // aligned dword the shift operates on is at `vaddr & !7`, and the shift
    // distances use the 3-bit misalignment (0..7).

    /// Load-doubleword-left: merge the high bytes of the addressed doubleword
    /// into the high end of `initial` (the current register value).
    #[inline]
    pub fn load_dl(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (misalign * 8));
        masked | (loaded << (misalign * 8))
    }

    /// Load-doubleword-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_dr(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (56 - misalign * 8));
        masked | (loaded >> (56 - misalign * 8))
    }

    /// Store-doubleword-left.
    #[inline]
    pub fn store_dl(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }

    /// Store-doubleword-right.
    #[inline]
    pub fn store_dr(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (56 - misalign * 8));
        let shifted = val << (56 - misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }
}
