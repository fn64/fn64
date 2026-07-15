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
//! - Sub-word accesses XOR the byte offset: halfword `^2`, byte `^3`. This is
//!   the N64's big-endian-in-a-little-endian-buffer convention. It is applied
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
    /// COP0 register 9, `Count`: the free-running cycle counter that backs
    /// `osGetCount`. It is the one COP0 read a recompiled body legitimately
    /// performs (`MFC0 rt, $9`); the host advances it. Modeled as real state
    /// rather than trapped, unlike the libultra-managed Status/Cause/EPC.
    pub cop0_count: u32,
    /// COP0 register 11, `Compare`: the timer-interrupt threshold written via
    /// `MTC0 rt, $11` on the `osSetTimer` path. Stored so the write round-trips;
    /// the interrupt it would schedule is the host's concern.
    pub cop0_compare: u32,
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

    /// Write a 32-bit result into GPR `idx`, sign-extending into the 64-bit
    /// register (the universal MIPS III rule for 32-bit ops: the result's
    /// bit 31 fills bits 63..32). This is the typed replacement for the C
    /// `S32(...)`/`ADD32(...)` casts.
    #[inline]
    pub fn set_r32(&mut self, idx: u8, val: i32) {
        self.set_r(idx, val as i64 as u64);
    }

    // ================================================================
    // COP1 / FPU register file.
    //
    // # The FR=0 even/odd pairing (the whole reason these aren't 32 plain f32s)
    //
    // libultra boots every OSThread with the FPU in FR=0 (32-register) mode.
    // In that mode a 64-bit value (a double, or a `ldc1`/`sdc1`/`dmtc1` slot)
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
        self.fpr[idx as usize]
    }

    /// Write raw 64 bits into doubleword FPR `idx`.
    #[inline]
    pub fn set_d_bits(&mut self, idx: u8, bits: u64) {
        self.fpr[idx as usize] = bits;
    }

    /// Read double-precision FPR `idx` as an `f64`.
    #[inline]
    pub fn f_d(&self, idx: u8) -> f64 {
        f64::from_bits(self.fpr[idx as usize])
    }

    /// Write an `f64` into double-precision FPR `idx`.
    #[inline]
    pub fn set_f_d(&mut self, idx: u8, val: f64) {
        self.fpr[idx as usize] = val.to_bits();
    }
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

impl<'a> Rdram<'a> {
    /// Wrap a byte buffer as rdram. The buffer should be [`RDRAM_LEN`] bytes;
    /// shorter buffers simply make more addresses fall out of bounds (a loud
    /// panic on access) rather than corrupting host memory.
    pub fn new(mem: &'a mut [u8]) -> Self {
        Rdram { mem }
    }

    /// Translate a sign-extended KSEG0/KSEG1 virtual address (the `reg + off`
    /// sum the MIPS code computes) to a physical rdram byte offset.
    #[inline]
    fn phys(vaddr: u64) -> usize {
        // Wrapping sub mirrors the C `(addr) - 0xFFFFFFFF80000000`; the result
        // is then range-checked by the slice index in each accessor.
        vaddr.wrapping_sub(RDRAM_VBASE) as usize
    }

    /// Effective virtual address of a `off(base)` operand: 32-bit-wrapping add
    /// of the base register's low word and the sign-extended offset, then
    /// sign-extended back to 64 bits (matches how the C forms `reg + offset`).
    #[inline]
    pub fn eff_addr(base_val: u64, off: i16) -> u64 {
        (base_val as u32).wrapping_add(off as i32 as u32) as i32 as i64 as u64
    }

    // --- Aligned loads ---

    /// Load a sign-extended word. Returns the `i32` the caller sign-extends
    /// into a GPR.
    #[inline]
    pub fn load_w(&self, vaddr: u64) -> i32 {
        let p = Self::phys(vaddr);
        i32::from_be_bytes([self.mem[p], self.mem[p + 1], self.mem[p + 2], self.mem[p + 3]])
    }

    /// Load a sign-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_h(&self, vaddr: u64) -> i16 {
        let p = Self::phys(vaddr) ^ 2;
        i16::from_be_bytes([self.mem[p], self.mem[p + 1]])
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
        self.mem[p..p + 4].copy_from_slice(&val.to_be_bytes());
    }

    /// Store the low halfword of `val` (byte offset XOR 2).
    #[inline]
    pub fn store_h(&mut self, vaddr: u64, val: u16) {
        let p = Self::phys(vaddr) ^ 2;
        self.mem[p..p + 2].copy_from_slice(&val.to_be_bytes());
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
    // half goes through the ordinary word path (`load_w`/`store_w`) — word
    // accesses carry NO sub-word swizzle — so the doubleword byte order comes
    // out big-endian, high word first, exactly as the game's memory image
    // holds it.

    /// Load a 64-bit doubleword: `(hi_word << 32) | lo_word` where `hi_word` is
    /// at `vaddr+0` and `lo_word` at `vaddr+4`.
    #[inline]
    pub fn load_d(&self, vaddr: u64) -> u64 {
        let hi = self.load_w(vaddr) as u32 as u64;
        let lo = self.load_w(vaddr.wrapping_add(4)) as u32 as u64;
        (hi << 32) | lo
    }

    /// Store a 64-bit doubleword: the high word to `vaddr+0`, the low word to
    /// `vaddr+4`.
    #[inline]
    pub fn store_d(&mut self, vaddr: u64, val: u64) {
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
