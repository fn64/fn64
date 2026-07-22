//! The typed runtime that emitted Rust targets: [`RecompContext`] (the CPU
//! register file) and [`Rdram`] (a checked memory view).
//!
//! # Why this exists (the whole point of `-rs`)
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
//! - Zero- or sign-extended KSEG0/KSEG1 addresses in the physical RDRAM
//!   window map through their shared low-29-bit physical offset. Unsupported
//!   mapped addresses retain the generated C lane's sparse/failing behavior;
//!   this runtime does not silently invent TLB translation.
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
/// One raw TLB entry as staged by the COP0 registers at `tlbwi` time.
///
/// The entry participates in the public `TLBR` and `TLBP` management
/// operations, but guest address translation through it remains a separate
/// loud frontier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TlbEntryRaw {
    pub page_mask: u32,
    pub entry_hi: u32,
    pub entry_lo0: u32,
    pub entry_lo1: u32,
}

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
    /// Writes are handed to the live CPU clock authority at the next block
    /// boundary. Options retain same-value writes, which are observable for
    /// Compare because every write acknowledges IP7.
    cop0_count_write: Option<u32>,
    cop0_compare_write: Option<u32>,
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
    /// COP0 EPC (register 14), written on precise exception entry when EXL was
    /// clear. Branch-delay exceptions hold the branch PC, not the delay PC.
    pub cop0_epc: u32,
    /// COP0 ErrorEPC (register 30), selected by ERET while Status.ERL is set.
    pub cop0_error_epc: u32,
    /// COP0 BadVAddr (register 8). The arbitrary-PC lane populates it for
    /// instruction-fetch AdEL and aligned-memory AdEL/AdES; TLB exception
    /// paths remain open.
    pub cop0_badvaddr: u32,
    /// COP0 TLB registers (Index 0, EntryLo0/1 2/3, PageMask 5, Wired 6,
    /// EntryHi 10). Stored round-trip state only: boot-time unmap-all loops
    /// save/clear these and `tlbwi` records entries; address translation
    /// through recorded entries is not modeled (use faults at the memory
    /// path).
    pub cop0_index: u32,
    /// Raw recorded TLB entries (see `tlbwi_record`, `tlbr_read`, and
    /// `tlbp_probe`).
    pub tlb_entries: [TlbEntryRaw; 32],
    pub cop0_entry_lo0: u32,
    pub cop0_entry_lo1: u32,
    pub cop0_page_mask: u32,
    pub cop0_wired: u32,
    pub cop0_entry_hi: u32,
    /// COP0 WatchLo/WatchHi (registers 18/19). Stored round-trip state only:
    /// SDK boot code writes 0 to disarm the watchpoint on the way up, and
    /// nothing in this runtime models the watch exception itself (a set
    /// watchpoint simply never fires).
    pub cop0_watch_lo: u32,
    pub cop0_watch_hi: u32,
    /// Libultra's combined CPU/RCP interrupt mask associated with this
    /// OSThread. CPU gating is mirrored into `cop0_status`; the packed value
    /// is retained so `osSetIntMask` returns this context's prior mask rather
    /// than another coroutine's last hardware setting.
    os_interrupt_mask: u32,
    /// Explicit host-installed return sentinel for an OSThread entry. A
    /// generated `jr`/`jalr` may finish the coroutine only when its captured
    /// target equals this value; address zero or an unmapped PC remains a
    /// loud guest fault.
    thread_return_pc: Option<u32>,
}

impl RecompContext {
    /// A fresh context with all registers zeroed.
    pub fn new() -> Self {
        RecompContext::default()
    }

    pub fn set_thread_return_pc(&mut self, pc: Option<u32>) {
        self.thread_return_pc = pc;
    }

    pub fn is_thread_return(&self, pc: u32) -> bool {
        self.thread_return_pc == Some(pc)
    }

    pub fn os_interrupt_mask(&self) -> u32 {
        self.os_interrupt_mask
    }

    pub fn replace_os_interrupt_mask(&mut self, mask: u32) -> u32 {
        std::mem::replace(&mut self.os_interrupt_mask, mask)
    }

    /// Refresh the block-local view from the live CPU clock without
    /// fabricating an architectural MTC0 write.
    pub fn synchronize_cop0_timing(&mut self, count: u32, compare: u32) {
        self.cop0_count = count;
        self.cop0_compare = compare;
    }

    /// Drain MTC0 Count/Compare writes for the live CPU clock authority.
    pub fn take_cop0_timing_writes(&mut self) -> (Option<u32>, Option<u32>) {
        (self.cop0_count_write.take(), self.cop0_compare_write.take())
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

    /// Snapshot all architectural GPRs for the audited rs/C ABI adapter.
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
            _ => trap_unsupported(format!("reserved COP1 control register FCR{idx}")),
        }
    }

    /// Write FCR31. Writes to FCR0/reserved FCRs have no architectural
    /// effect. Reserved bits are discarded rather than becoming hidden state.
    #[inline]
    pub fn write_fcr(&mut self, idx: u8, value: u32) {
        if idx != 31 {
            trap_unsupported(format!(
                "write to read-only/reserved COP1 control register FCR{idx}"
            ));
        }
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

    /// Apply the VR4300 ERET state transition and return its virtual target.
    /// User's Manual section 6.3 specifies ErrorEPC/ERL precedence over
    /// EPC/EXL and clearing the architectural LLbit on exception return.
    #[inline]
    pub fn exception_return_pc(&mut self) -> u32 {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;

        self.ll_reservation = None;
        if self.cop0_status & STATUS_ERL != 0 {
            self.cop0_status &= !STATUS_ERL;
            self.cop0_error_epc
        } else {
            self.cop0_status &= !STATUS_EXL;
            self.cop0_epc
        }
    }

    /// `tlbwi`: record the indexed entry from the staged COP0 TLB registers.
    /// Address TRANSLATION through recorded entries is not modeled -- KSEG0/
    /// KSEG1 code never needs it, and an actual load/store through a mapped
    /// segment (e.g. libultra's osInitialize page at 0xC0000000) faults
    /// loudly at the memory path if a title ever dereferences one. Recording
    /// instead of trapping lets boot-time TLB setup/unmap loops run exactly
    /// as on hardware.
    pub fn tlbwi_record(&mut self) {
        let index = (self.cop0_index & 31) as usize;
        self.tlb_entries[index] = TlbEntryRaw {
            page_mask: self.cop0_page_mask,
            entry_hi: self.cop0_entry_hi,
            entry_lo0: self.cop0_entry_lo0,
            entry_lo1: self.cop0_entry_lo1,
        };
    }

    /// `tlbr`: load the staged COP0 registers from the indexed TLB entry.
    ///
    /// VR4300 User's Manual section 5.4.11 names exactly these four
    /// destinations. Index bit 5 and the probe-failure bit do not participate
    /// in the 32-entry array index.
    pub fn tlbr_read(&mut self) {
        let entry = self.tlb_entries[(self.cop0_index & 31) as usize];
        self.cop0_page_mask = entry.page_mask;
        self.cop0_entry_hi = entry.entry_hi;
        self.cop0_entry_lo0 = entry.entry_lo0;
        self.cop0_entry_lo1 = entry.entry_lo1;
    }

    /// `tlbp`: probe all recorded entries using VPN2/PageMask plus ASID or the
    /// entry's paired Global bits, and publish the result in COP0 Index.
    ///
    /// Valid and Dirty do not participate in a tag match. More than one match
    /// is architecturally undefined, so the deterministic runtime traps rather
    /// than selecting an arbitrary entry. On a miss the architecture leaves
    /// the low Index field unpredictable; fn64's bounded deterministic policy
    /// clears that field and sets only the probe-failure bit.
    pub fn tlbp_probe(&mut self) {
        const VPN2_MASK: u32 = 0xffff_e000;
        const PAGE_MASK: u32 = 0x01ff_e000;
        const ASID_MASK: u32 = 0x0000_00ff;
        const GLOBAL: u32 = 1;

        let probe = self.cop0_entry_hi;
        let mut matched = None;
        for (index, entry) in self.tlb_entries.iter().enumerate() {
            let compared_vpn = VPN2_MASK & !(entry.page_mask & PAGE_MASK);
            let vpn_matches = (probe ^ entry.entry_hi) & compared_vpn == 0;
            let global = entry.entry_lo0 & GLOBAL != 0 && entry.entry_lo1 & GLOBAL != 0;
            let asid_matches = (probe ^ entry.entry_hi) & ASID_MASK == 0;
            if vpn_matches && (global || asid_matches) && matched.replace(index).is_some() {
                trap_unsupported(
                    "TLBP found multiple matching entries; VR4300 behavior is undefined",
                );
            }
        }
        self.cop0_index = matched.map_or(1 << 31, |index| index as u32);
    }

    #[inline]
    pub fn read_cop0(&self, reg: u8) -> u32 {
        match reg {
            8 => self.cop0_badvaddr,
            9 => self.cop0_count,
            11 => self.cop0_compare,
            12 => self.cop0_status,
            13 => self.cop0_cause,
            14 => self.cop0_epc,
            0 => self.cop0_index,
            2 => self.cop0_entry_lo0,
            3 => self.cop0_entry_lo1,
            5 => self.cop0_page_mask,
            6 => self.cop0_wired,
            10 => self.cop0_entry_hi,
            18 => self.cop0_watch_lo,
            19 => self.cop0_watch_hi,
            30 => self.cop0_error_epc,
            _ => trap_unsupported(format!("unsupported MFC0 from COP0 register {reg}")),
        }
    }

    /// Write a modeled 32-bit COP0 register for MTC0. Cause permits only the
    /// two software-pending bits; hardware pending lines remain owned by the
    /// device/clock layer. Status is context state and is replaced as one
    /// architectural register so interrupt gating changes at the next block
    /// boundary.
    #[inline]
    pub fn write_cop0(&mut self, reg: u8, value: u32) {
        match reg {
            9 => {
                self.cop0_count = value;
                self.cop0_count_write = Some(value);
            }
            11 => {
                self.cop0_compare = value;
                self.cop0_compare_write = Some(value);
                self.cop0_cause &= !crate::execution::CpuInterruptLine::TIMER.cause_bit();
            }
            12 => self.cop0_status = value,
            13 => {
                const SOFTWARE_IP: u32 = 0b11 << 8;
                self.cop0_cause = (self.cop0_cause & !SOFTWARE_IP) | (value & SOFTWARE_IP);
            }
            14 => self.cop0_epc = value,
            0 => self.cop0_index = value,
            2 => self.cop0_entry_lo0 = value,
            3 => self.cop0_entry_lo1 = value,
            5 => self.cop0_page_mask = value,
            6 => self.cop0_wired = value,
            10 => self.cop0_entry_hi = value,
            18 => self.cop0_watch_lo = value,
            19 => self.cop0_watch_hi = value,
            30 => self.cop0_error_epc = value,
            _ => trap_unsupported(format!("unsupported MTC0 to COP0 register {reg}")),
        }
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
            trap_unsupported(format!("enabled COP1 exception {exception}"));
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
/// Optional raw word-MMIO read. `None` means the address is ordinary memory.
pub type MmioRead = fn(u64) -> Option<u32>;
/// Optional raw word-MMIO write. `true` means the device consumed the write.
pub type MmioWrite = fn(u64, u32) -> bool;
/// One post-commit guest write. Only aligned CPU halfword stores carry a
/// value because public RDRAM hidden-bit behavior assigns semantics to that
/// exact operation; byte/word/DMA effects remain unclaimed ranges.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GuestWriteEvent {
    Range { physical_offset: u32, len: u32 },
    NonRdpWrite16 { logical_offset: u32, value: u16 },
}

impl GuestWriteEvent {
    pub const fn range(self) -> (u32, u32) {
        match self {
            Self::Range {
                physical_offset,
                len,
            } => (physical_offset, len),
            Self::NonRdpWrite16 { logical_offset, .. } => (logical_offset, 2),
        }
    }
}

/// Post-commit physical RDRAM write observer. Executable invalidation and
/// renderer notification are multiplexed by the host callback.
pub type WriteObserver = fn(GuestWriteEvent);

/// Host callback reached immediately before translated code preserves a loud
/// panic for an instruction shape this runtime does not model.
pub type UnsupportedObserver = fn(&str);

/// Stable identity emitted at the first statement of every translated
/// whole-function body. The enclosing artifact identity remains host-owned;
/// `(vram, symbol)` distinguishes functions within that artifact without
/// depending on native addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TranslatedFunctionIdentity {
    pub vram: u32,
    pub symbol: &'static str,
}

impl TranslatedFunctionIdentity {
    pub const fn new(vram: u32, symbol: &'static str) -> Self {
        Self { vram, symbol }
    }
}

/// Opaque version marker exported by newly generated whole-function modules.
/// Passing a generated module's marker to the ABI is the explicit assertion
/// that every callable in that artifact contains the entry hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionEntryObservationSchema(u32);

/// Entry-observation schema implemented by this emitter/runtime pair.
pub const FUNCTION_ENTRY_OBSERVATION_SCHEMA: FunctionEntryObservationSchema =
    FunctionEntryObservationSchema(1);

/// Host callback reached by an emitted body before its first translated
/// instruction executes.
pub type FunctionEntryObserver = fn(TranslatedFunctionIdentity);

thread_local! {
    /// Recompiled execution is single-threaded by design (`docs/DESIGN.md` section
    /// 2), so the override belongs to the executing host thread. A
    /// thread-local `Cell` also lets tests install an isolated resolver
    /// without unsafe global mutation or cross-test serialization.
    static HOST_LOOKUP: std::cell::Cell<Option<HostLookup>> = const {
        std::cell::Cell::new(None)
    };
    static HOST_PAUSE: std::cell::Cell<Option<HostPause>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_READ: std::cell::Cell<Option<MmioRead>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_WRITE: std::cell::Cell<Option<MmioWrite>> = const {
        std::cell::Cell::new(None)
    };
    static WRITE_OBSERVER: std::cell::Cell<Option<WriteObserver>> = const {
        std::cell::Cell::new(None)
    };
    static UNSUPPORTED_OBSERVER: std::cell::Cell<Option<UnsupportedObserver>> = const {
        std::cell::Cell::new(None)
    };
    static FUNCTION_ENTRY_OBSERVER: std::cell::Cell<Option<FunctionEntryObserver>> = const {
        std::cell::Cell::new(None)
    };
}

/// Install (or clear) the current thread's host-function resolver, returning
/// the previous resolver.
///
/// Generated dispatchers consult this hook before their sorted recompiled table.
/// A host can therefore bind a vram to a safe typed adapter over an fn64 shim;
/// vrams deliberately omitted from the recompiled table fail loudly if the host
/// has not installed their adapter. The function-pointer seam itself is
/// entirely safe Rust: no `transmute`, raw pointer, or ABI cast is involved.
pub fn set_host_lookup(resolver: Option<HostLookup>) -> Option<HostLookup> {
    HOST_LOOKUP.with(|slot| slot.replace(resolver))
}

/// Install the host's cooperative-yield adapter for translated self-loops.
pub fn set_host_pause(pause: Option<HostPause>) -> Option<HostPause> {
    HOST_PAUSE.with(|slot| slot.replace(pause))
}

/// Install the raw word-MMIO boundary used by emitted `lw`/`sw` operations.
/// The hooks are thread-local like host lookup because guest execution is
/// single-threaded; ordinary RDRAM accesses remain direct checked slice I/O.
pub fn set_mmio_hooks(
    read: Option<MmioRead>,
    write: Option<MmioWrite>,
) -> (Option<MmioRead>, Option<MmioWrite>) {
    let previous_read = MMIO_READ.with(|slot| slot.replace(read));
    let previous_write = MMIO_WRITE.with(|slot| slot.replace(write));
    (previous_read, previous_write)
}

pub fn set_write_observer(observer: Option<WriteObserver>) -> Option<WriteObserver> {
    WRITE_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the host's unsupported-instruction evidence sink. The translated
/// lane remains independently usable: without a sink, the same named panic
/// still fires.
pub fn set_unsupported_observer(
    observer: Option<UnsupportedObserver>,
) -> Option<UnsupportedObserver> {
    UNSUPPORTED_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the current thread's translated-function entry observer.
pub fn set_function_entry_observer(
    observer: Option<FunctionEntryObserver>,
) -> Option<FunctionEntryObserver> {
    FUNCTION_ENTRY_OBSERVER.with(|slot| slot.replace(observer))
}

/// Record entry into one emitted whole-function body. Generated code places
/// this call before initializing its local dispatch PC, so direct calls,
/// lookup-resolved calls, tail calls, and root entry share one boundary.
#[inline]
pub fn notify_function_entry(identity: TranslatedFunctionIdentity) {
    FUNCTION_ENTRY_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(identity);
        }
    });
}

/// Record and preserve the loud endpoint for unsupported translated CPU
/// behavior. Generated bodies use this instead of open-coded panics so the
/// fixed-cycle journal cannot miss an early abort.
#[cold]
#[inline(never)]
pub fn trap_unsupported(context: impl Into<String>) -> ! {
    let context = context.into();
    UNSUPPORTED_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(&context);
        }
    });
    panic!("{context}")
}

/// Notify the installed observer after bytes at a physical RDRAM range have
/// committed. DMA and generated-C adapters use this same seam as typed stores.
pub fn notify_guest_write(offset: u32, len: u32) {
    if len != 0 {
        WRITE_OBSERVER.with(|slot| {
            if let Some(observer) = slot.get() {
                observer(GuestWriteEvent::Range {
                    physical_offset: offset,
                    len,
                });
            }
        });
    }
}

/// Notify one aligned CPU halfword store after the visible bytes commit.
pub fn notify_non_rdp_write16(logical_offset: u32, value: u16) {
    WRITE_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(GuestWriteEvent::NonRdpWrite16 {
                logical_offset,
                value,
            });
        }
    });
}

/// Yield the active emulated thread at an unconditional branch-to-self.
pub fn pause_self() {
    HOST_PAUSE.with(|slot| {
        slot.get()
            .unwrap_or_else(|| panic!("pause_self: rs host installed no coroutine-yield adapter"))(
        )
    });
}

/// Resolve `vram` through the current thread's host-function resolver.
#[inline]
pub fn resolve_host_function(vram: u32) -> Option<RecompFunc> {
    HOST_LOOKUP.with(|slot| slot.get().and_then(|resolver| resolver(vram)))
}

/// Invoke a statically-known recompiled target unless the host resolver overrides
/// its vram. This is the direct-JAL counterpart of generated `lookup(vram)`:
/// libultra functions whose bodies contain no privileged instruction still
/// must enter the executor-backed host shim rather than bypassing it merely
/// because the Rust recompiler could translate their machine code.
#[inline]
pub fn call_host_or_recompiled(
    vram: u32,
    recompiled: RecompFunc,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) {
    resolve_host_function(vram).unwrap_or(recompiled)(ctx, mem);
}

impl<'a> Rdram<'a> {
    /// Wrap a byte buffer as rdram. The buffer should be [`RDRAM_LEN`] bytes;
    /// shorter buffers simply make more addresses fall out of bounds (a loud
    /// panic on access) rather than corrupting host memory.
    pub fn new(mem: &'a mut [u8]) -> Self {
        Rdram { mem }
    }

    /// Borrow the shared backing allocation at the runtime ABI seam. Normal
    /// emitted code has no reason to use this; fn64's rs-lane host adapters use
    /// it to call the existing, audited `*_recomp` marshalling layer without
    /// allocating or copying a second RDRAM image.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.mem
    }

    /// Translate a canonical KSEG0/KSEG1 address to its generated-code backing
    /// offset. Physical RDRAM aliases share the low 29-bit device prefix;
    /// non-RDRAM direct windows retain N64Recomp's sparse `address - KSEG0`
    /// layout. Modeled RCP/PIF words are excluded because their installed hook
    /// is the sole device authority.
    #[inline]
    fn direct_storage_offset(vaddr: u64) -> Option<usize> {
        if Self::is_word_only_mmio(vaddr) {
            return None;
        }

        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        if !canonical_32 || !direct_segment {
            return None;
        }

        let physical = low & 0x1fff_ffff;
        Some(if physical < RDRAM_LEN as u32 {
            physical as usize
        } else {
            low.wrapping_sub(0x8000_0000) as usize
        })
    }

    #[inline]
    fn backing_offset(vaddr: u64) -> usize {
        if let Some(offset) = Self::direct_storage_offset(vaddr) {
            return offset;
        }
        let physical = (vaddr as u32) & 0x1fff_ffff;
        let reason = if Self::is_word_only_mmio(vaddr) {
            "modeled word-only device access was not consumed by the installed hook"
        } else {
            "only zero- or sign-extended KSEG0/KSEG1 are modeled"
        };
        trap_unsupported(format!(
            "Rdram: unsupported mapped address {vaddr:#018x} resolves to physical {physical:#x}; {reason}"
        ))
    }

    /// Canonical physical RDRAM offset for cached or uncached CPU aliases.
    /// Only the direct segments are accepted: masking KUSEG would silently
    /// add unsupported TLB behavior. Device/renderer observations are named
    /// in the same physical RDRAM space as the visible backing bytes.
    #[inline]
    fn physical_rdram_offset(vaddr: u64) -> Option<u32> {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        let physical = low & 0x1fff_ffff;
        (canonical_32 && direct_segment && physical < RDRAM_LEN as u32).then_some(physical)
    }

    /// Generated-C's proxy exposes RCP registers and PIF RAM only as modeled
    /// word accesses. Keep the typed lane on that identical boundary instead
    /// of letting a subword operation fall through to sparse host storage.
    #[inline]
    fn is_word_only_mmio(vaddr: u64) -> bool {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        if canonical_32 && (0xa400_0000..0xa490_0000).contains(&low) {
            return true;
        }
        let physical = low & 0x1fff_ffff;
        canonical_32
            && (0x8000_0000..0xc000_0000).contains(&low)
            && (0x1fc0_07c0..0x1fc0_0800).contains(&physical)
    }

    #[inline]
    fn reject_nonword_mmio(vaddr: u64, width: u32, is_write: bool) {
        if Self::is_word_only_mmio(vaddr) {
            let operation = if is_write { "write" } else { "read" };
            trap_unsupported(format!(
                "Rdram: raw MMIO {operation} at {vaddr:#018x} used unsupported {width}-byte access; RCP/PIF registers require modeled word semantics"
            ));
        }
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
    ///
    /// Perf: read the 4 bytes as ONE slice range (`self.mem[p..p+4]`) rather
    /// than four `self.mem[p+i]` indexes. The range form does a SINGLE bounds
    /// check and lets the compiler emit one aligned 32-bit load; the byte-at-
    /// a-time form did 4 bounds checks + a byte-assemble in the hot loop
    /// (millions of accesses in collision init). Same value, safe indexing,
    /// still `#![forbid(unsafe_code)]`.
    #[inline]
    pub fn load_w(&self, vaddr: u64) -> i32 {
        assert_eq!(vaddr & 3, 0, "unaligned LW at {vaddr:#018x}");
        if let Some(value) = MMIO_READ.with(|slot| slot.get().and_then(|read| read(vaddr))) {
            return value as i32;
        }
        let p = Self::backing_offset(vaddr);
        i32::from_ne_bytes(self.mem[p..p + 4].try_into().unwrap())
    }

    /// Load a sign-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_h(&self, vaddr: u64) -> i16 {
        Self::reject_nonword_mmio(vaddr, 2, false);
        assert_eq!(vaddr & 1, 0, "unaligned LH at {vaddr:#018x}");
        let p = Self::backing_offset(vaddr) ^ 2;
        i16::from_ne_bytes(self.mem[p..p + 2].try_into().unwrap())
    }

    /// Load a zero-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_hu(&self, vaddr: u64) -> u16 {
        self.load_h(vaddr) as u16
    }

    /// Load a sign-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_b(&self, vaddr: u64) -> i8 {
        Self::reject_nonword_mmio(vaddr, 1, false);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] as i8
    }

    /// Load a zero-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_bu(&self, vaddr: u64) -> u8 {
        Self::reject_nonword_mmio(vaddr, 1, false);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p]
    }

    // --- Aligned stores ---

    /// Store the low word of `val`.
    #[inline]
    pub fn store_w(&mut self, vaddr: u64, val: u32) {
        assert_eq!(vaddr & 3, 0, "unaligned SW at {vaddr:#018x}");
        if MMIO_WRITE.with(|slot| slot.get().is_some_and(|write| write(vaddr, val))) {
            return;
        }
        let p = Self::backing_offset(vaddr);
        self.mem[p..p + 4].copy_from_slice(&val.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_guest_write(offset, 4);
        }
    }

    /// Store the low halfword of `val` (byte offset XOR 2).
    #[inline]
    pub fn store_h(&mut self, vaddr: u64, val: u16) {
        Self::reject_nonword_mmio(vaddr, 2, true);
        assert_eq!(vaddr & 1, 0, "unaligned SH at {vaddr:#018x}");
        let p = Self::backing_offset(vaddr) ^ 2;
        self.mem[p..p + 2].copy_from_slice(&val.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_non_rdp_write16(offset, val);
        }
    }

    /// Store the low byte of `val` (byte offset XOR 3).
    #[inline]
    pub fn store_b(&mut self, vaddr: u64, val: u8) {
        Self::reject_nonword_mmio(vaddr, 1, true);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] = val;
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_guest_write(offset, 1);
        }
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
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_word_only_mmio(word_addr) {
            if misalign != 0 {
                Self::reject_nonword_mmio(vaddr, 4 - misalign, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    /// Store-word-right.
    #[inline]
    pub fn store_wr(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_word_only_mmio(word_addr) {
            if misalign != 3 {
                Self::reject_nonword_mmio(vaddr, misalign + 1, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
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
        Self::reject_nonword_mmio(vaddr, 8, false);
        assert_eq!(vaddr & 7, 0, "unaligned LD at {vaddr:#018x}");
        let hi = self.load_w(vaddr) as u32 as u64;
        let lo = self.load_w(vaddr.wrapping_add(4)) as u32 as u64;
        (hi << 32) | lo
    }

    /// Store a 64-bit doubleword: the high word to `vaddr+0`, the low word to
    /// `vaddr+4`, followed by one post-commit eight-byte write range.
    #[inline]
    pub fn store_d(&mut self, vaddr: u64, val: u64) {
        Self::reject_nonword_mmio(vaddr, 8, true);
        assert_eq!(vaddr & 7, 0, "unaligned SD at {vaddr:#018x}");
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            let high = Self::backing_offset(vaddr);
            let low = Self::backing_offset(vaddr.wrapping_add(4));
            // Match N64Recomp's low-word then high-word commit order. The
            // observer runs only after both halves are coherent.
            self.mem[low..low + 4].copy_from_slice(&(val as u32).to_ne_bytes());
            self.mem[high..high + 4].copy_from_slice(&((val >> 32) as u32).to_ne_bytes());
            notify_guest_write(offset, 8);
        } else {
            self.store_w(vaddr.wrapping_add(4), val as u32);
            self.store_w(vaddr, (val >> 32) as u32);
        }
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

    // --- Checked accessors for the bank/sparse block-runner lane (U4) ---
    //
    // The historical whole-function lane calls the unchecked accessors above:
    // an access outside backed generated-code storage is a host panic there,
    // and that panicking semantics is deliberately preserved. The block-runner
    // lane instead needs
    // a typed VR4300 memory fault it can turn into `BlockExit::Fault`, so it
    // calls these `try_` variants. On success they perform the identical access
    // as their unchecked twin; on an out-of-bounds effective address they
    // return `Err(vaddr)` carrying the faulting guest virtual address, and
    // touch no memory. This models "access outside supplied backing storage";
    // it is not full VR4300 address-error/TLB semantics (see U4 in
    // `docs/UNIVERSAL-RUNTIME-PLAN.md`).

    /// True iff the `width`-byte range beginning at storage offset `p` lies
    /// wholly inside backed storage. Every checked accessor reduces to
    /// this after applying its own swizzle so the admitted set matches exactly
    /// which unchecked accesses would not panic.
    #[inline]
    fn storage_range_backed(&self, p: usize, width: usize) -> bool {
        p.checked_add(width)
            .is_some_and(|end| end <= self.mem.len())
    }

    /// Translate and bound a checked-lane access without entering the
    /// unchecked lane's loud unsupported-address trap. `try_*` callers must
    /// return the original virtual address as a typed fault for every
    /// non-direct segment, including opt-in MMIO windows with no installed port.
    #[inline]
    fn virtual_range_backed(&self, vaddr: u64, lane_xor: usize, width: usize) -> bool {
        Self::direct_storage_offset(vaddr)
            .is_some_and(|p| self.storage_range_backed(p ^ lane_xor, width))
    }

    /// Whether the aligned-word effective address is backed (LW/LWU/LL/…).
    #[inline]
    fn word_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 4)
    }

    /// Whether the aligned-doubleword effective address is backed (LD/SD/…).
    #[inline]
    fn dword_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 8)
    }

    /// Checked LW/LWU (aligned word). See the module note on the block lane.
    #[inline]
    pub fn try_load_w(&self, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr) {
            Ok(self.load_w(vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LH (aligned, sign-extended halfword).
    #[inline]
    pub fn try_load_h(&self, vaddr: u64) -> Result<i16, u64> {
        if self.virtual_range_backed(vaddr, 2, 2) {
            Ok(self.load_h(vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LHU (aligned, zero-extended halfword).
    #[inline]
    pub fn try_load_hu(&self, vaddr: u64) -> Result<u16, u64> {
        self.try_load_h(vaddr).map(|v| v as u16)
    }

    /// Checked LB (sign-extended byte).
    #[inline]
    pub fn try_load_b(&self, vaddr: u64) -> Result<i8, u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_b(vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LBU (zero-extended byte).
    #[inline]
    pub fn try_load_bu(&self, vaddr: u64) -> Result<u8, u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_bu(vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LWL (the aligned word it merges from must be backed).
    #[inline]
    pub fn try_load_wl(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LWR.
    #[inline]
    pub fn try_load_wr(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LD/LLD (aligned doubleword).
    #[inline]
    pub fn try_load_d(&self, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr) {
            Ok(self.load_d(vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LDL.
    #[inline]
    pub fn try_load_dl(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked LDR.
    #[inline]
    pub fn try_load_dr(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    /// Checked SW.
    #[inline]
    pub fn try_store_w(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr) {
            self.store_w(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SH.
    #[inline]
    pub fn try_store_h(&mut self, vaddr: u64, val: u16) -> Result<(), u64> {
        if self.virtual_range_backed(vaddr, 2, 2) {
            self.store_h(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SB.
    #[inline]
    pub fn try_store_b(&mut self, vaddr: u64, val: u8) -> Result<(), u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            self.store_b(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SWL (reads and writes the aligned word it straddles).
    #[inline]
    pub fn try_store_wl(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SWR.
    #[inline]
    pub fn try_store_wr(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SD/SCD (aligned doubleword).
    #[inline]
    pub fn try_store_d(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr) {
            self.store_d(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SDL.
    #[inline]
    pub fn try_store_dl(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    /// Checked SDR.
    #[inline]
    pub fn try_store_dr(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        set_unsupported_observer, trap_unsupported, GuestWriteEvent, Rdram, RecompContext,
        RDRAM_LEN,
    };

    type RdramOperation = for<'a> fn(&mut Rdram<'a>);

    thread_local! {
        static OBSERVED_WRITES: std::cell::RefCell<Vec<GuestWriteEvent>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static MMIO_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static UNSUPPORTED_CONTEXTS: std::cell::RefCell<Vec<String>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn observe_write(event: GuestWriteEvent) {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().push(event));
    }

    fn consume_mmio(_vaddr: u64, _value: u32) -> bool {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        true
    }

    fn read_mmio(_vaddr: u64) -> Option<u32> {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        Some(0)
    }

    fn observe_unsupported(context: &str) {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().push(context.to_owned()));
    }

    #[test]
    fn unsupported_observer_runs_before_the_named_panic() {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
        let previous = set_unsupported_observer(Some(observe_unsupported));
        let panic = std::panic::catch_unwind(|| trap_unsupported("unsupported COP0 register 7"));
        set_unsupported_observer(previous);

        assert!(panic.is_err());
        UNSUPPORTED_CONTEXTS.with(|contexts| {
            assert_eq!(
                contexts.borrow().as_slice(),
                ["unsupported COP0 register 7"]
            );
        });
    }

    #[test]
    fn exception_return_prefers_error_epc_and_preserves_exl_under_erl() {
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 2);
        ctx.cop0_epc = 0x8000_1000;
        ctx.cop0_error_epc = 0xBFC0_0200;
        ctx.set_ll_reservation(0x8000_0040, 4);

        assert_eq!(ctx.exception_return_pc(), 0xBFC0_0200);
        assert_eq!(ctx.cop0_status & (1 << 2), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert!(!ctx.take_ll_reservation(0x8000_0040, 4));
    }

    #[test]
    fn cop0_status_and_software_interrupt_writes_preserve_hardware_pending() {
        let mut ctx = RecompContext::new();
        ctx.write_cop0(12, 0x3400_FF01);
        assert_eq!(ctx.read_cop0(12), 0x3400_FF01);

        ctx.cop0_cause = (1 << 10) | (9 << 2) | (1 << 31);
        ctx.write_cop0(13, 0b10 << 8);
        assert_eq!(ctx.cop0_cause & (0b11 << 8), 0b10 << 8);
        assert_ne!(ctx.cop0_cause & (1 << 10), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn cop0_timing_writes_retain_same_value_compare_acknowledgements() {
        let mut ctx = RecompContext::new();
        ctx.synchronize_cop0_timing(7, 9);
        ctx.cop0_cause = 1 << 15;
        ctx.write_cop0(9, 7);
        ctx.write_cop0(11, 9);

        assert_eq!(ctx.cop0_cause & (1 << 15), 0);
        assert_eq!(ctx.take_cop0_timing_writes(), (Some(7), Some(9)));
        assert_eq!(ctx.take_cop0_timing_writes(), (None, None));
    }

    #[test]
    fn rdram_write_observer_runs_after_committed_logical_ranges() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous = super::set_write_observer(Some(observe_write));
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_8000_0000, 0x1122_3344);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_b(0xFFFF_FFFF_8000_0006, 0x77);
        mem.store_d(0xFFFF_FFFF_A000_0008, 0x8899_aabb_ccdd_eeff);

        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, 0x1122_3344);
        assert_eq!(mem.load_hu(0xFFFF_FFFF_8000_0004), 0x5566);
        assert_eq!(mem.load_bu(0xFFFF_FFFF_8000_0006), 0x77);
        assert_eq!(mem.load_d(0xFFFF_FFFF_8000_0008), 0x8899_aabb_ccdd_eeff);
        assert_eq!(
            OBSERVED_WRITES.with(|writes| writes.borrow().clone()),
            vec![
                GuestWriteEvent::Range {
                    physical_offset: 0,
                    len: 4,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::Range {
                    physical_offset: 6,
                    len: 1,
                },
                GuestWriteEvent::Range {
                    physical_offset: 8,
                    len: 8,
                },
            ]
        );
        super::set_write_observer(previous);
    }

    #[test]
    fn write_events_canonicalize_cached_and_uncached_rdram_aliases() {
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_a440_0000), None);
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0000_0000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_c000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0001_8000_1234), None);
    }

    #[test]
    fn sparse_direct_windows_share_one_classifier_across_canonical_forms() {
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0x0000_0000_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_8600_0000),
            Some(0x0600_0000)
        );
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_a460_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0x0000_0001_a600_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_c600_0000), None);

        let mut bytes = vec![0u8; RDRAM_LEN + 4];
        bytes[RDRAM_LEN..RDRAM_LEN + 4].copy_from_slice(&0x1234_5678u32.to_ne_bytes());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xffff_ffff_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0x0000_0000_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.try_load_w(0xffff_ffff_8080_0000), Ok(0x1234_5678));
    }

    #[test]
    fn kseg0_and_kseg1_loads_and_stores_share_visible_bytes() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let kseg0 = 0xffff_ffff_8000_0000;
        let kseg1 = 0xffff_ffff_a000_0000;

        mem.store_w(kseg1, 0x1122_3344);
        assert_eq!(mem.load_w(kseg0) as u32, 0x1122_3344);
        mem.store_h(kseg0 + 4, 0x8567);
        assert_eq!(mem.load_hu(kseg1 + 4), 0x8567);
        mem.store_b(kseg1 + 6, 0xa9);
        assert_eq!(mem.load_bu(kseg0 + 6), 0xa9);

        mem.store_w(0x0000_0000_8000_0008, 0xdead_beef);
        assert_eq!(mem.load_w(0x0000_0000_a000_0008) as u32, 0xdead_beef);
    }

    #[test]
    fn mapped_low_physical_addresses_trap_instead_of_aliasing_rdram() {
        let mut bytes = [0u8; 4];
        let mem = Rdram::new(&mut bytes);
        for address in [
            0x0000_0000_0000_0000,
            0xffff_ffff_c000_0000,
            0x0000_0001_8000_0000,
        ] {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = mem.load_w(address);
            }));
            assert!(
                panic.is_err(),
                "mapped address {address:#018x} did not trap"
            );
        }
    }

    #[test]
    fn checked_accessors_return_typed_faults_for_non_rdram_segments() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let mmio = 0xffff_ffff_a460_0010;

        assert_eq!(mem.try_load_w(mmio), Err(mmio));
        assert_eq!(mem.try_load_h(mmio), Err(mmio));
        assert_eq!(mem.try_load_hu(mmio), Err(mmio));
        assert_eq!(mem.try_load_b(mmio), Err(mmio));
        assert_eq!(mem.try_load_bu(mmio), Err(mmio));
        assert_eq!(mem.try_load_wl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_wr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_load_d(mmio), Err(mmio));
        assert_eq!(mem.try_load_dl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_dr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_store_w(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_h(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_b(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_wl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_wr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.try_store_d(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_dl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_dr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.as_mut_slice(), [0; 16]);
    }

    #[test]
    fn nonword_rcp_and_pif_accesses_trap_before_any_side_effect() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        MMIO_CALLS.with(|calls| calls.set(0));
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(Some(read_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        let operations: [RdramOperation; 8] = [
            |mem| {
                let _ = mem.load_h(0xffff_ffff_a400_0000);
            },
            |mem| {
                let _ = mem.load_b(0xffff_ffff_9fc0_07c0);
            },
            |mem| mem.store_h(0xffff_ffff_a440_0000, 1),
            |mem| mem.store_b(0xffff_ffff_bfc0_07c0, 1),
            |mem| {
                let _ = mem.load_d(0xffff_ffff_a400_0000);
            },
            |mem| mem.store_d(0xffff_ffff_bfc0_07c0, 1),
            |mem| mem.store_wl(0xffff_ffff_a440_0001, 1),
            |mem| mem.store_wr(0xffff_ffff_a440_0002, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "non-word MMIO access did not trap");
        }

        assert_eq!(MMIO_CALLS.with(std::cell::Cell::get), 0);
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(mem.as_mut_slice(), [0; 4]);

        mem.store_wl(0xffff_ffff_a440_0000, 0x1122_3344);
        mem.store_wr(0xffff_ffff_a440_0003, 0x5566_7788);
        assert_eq!(
            MMIO_CALLS.with(std::cell::Cell::get),
            2,
            "full-selector SWL/SWR must issue one write each with no MMIO pre-read"
        );
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn misaligned_aligned_accessors_trap_before_bytes_or_events_change() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let mut bytes = [0x5au8; 16];
        let before = bytes;
        let mut mem = Rdram::new(&mut bytes);
        let operations: [RdramOperation; 6] = [
            |mem| {
                let _ = mem.load_h(0xffff_ffff_8000_0001);
            },
            |mem| mem.store_h(0xffff_ffff_a000_0001, 1),
            |mem| {
                let _ = mem.load_w(0xffff_ffff_8000_0002);
            },
            |mem| mem.store_w(0xffff_ffff_a000_0002, 1),
            |mem| {
                let _ = mem.load_d(0xffff_ffff_8000_0004);
            },
            |mem| mem.store_d(0xffff_ffff_a000_0004, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "misaligned access did not trap");
        }
        assert_eq!(bytes, before);
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn consumed_mmio_store_does_not_report_an_rdram_write() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(None, Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A460_0000, 0x1234_5678);

        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(bytes, [0; 4]);
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }
}
