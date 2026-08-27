//! FPU and 64-bit integer arithmetic methods of [`RecompContext`].
//!
//! Split from the runtime module body purely by size; this is the same
//! inherent impl continued in a child module, so field access and privacy
//! are unchanged.

use super::*;

#[inline]
fn is_snan32(bits: u32) -> bool {
    // VR4300 User's Manual p.151 uses the legacy convention: fraction MSB 1
    // denotes signaling NaN, opposite the modern IEEE host convention.
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x007F_FFFF != 0 && bits & 0x0040_0000 != 0
}

#[inline]
fn is_qnan32(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x003F_FFFF != 0
}

#[inline]
fn is_subnormal32(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0 && bits & 0x007F_FFFF != 0
}

#[inline]
fn is_snan64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000
        && bits & 0x000F_FFFF_FFFF_FFFF != 0
        && bits & 0x0008_0000_0000_0000 != 0
}

#[inline]
fn is_qnan64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000 && bits & 0x0007_FFFF_FFFF_FFFF != 0
}

#[inline]
fn is_subnormal64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0 && bits & 0x000F_FFFF_FFFF_FFFF != 0
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

impl RecompContext {
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

    fn try_fixed_to_float_raw(
        &mut self,
        value: i64,
        format: FixedFloatFormat,
        signed_56_source: bool,
    ) -> Result<u64, FpuException> {
        const SIGNED_56_MIN: i64 = -(1i64 << 55);
        const SIGNED_56_MAX: i64 = (1i64 << 55) - 1;

        self.fcsr &= !(0x3F << 12);
        if signed_56_source && !(SIGNED_56_MIN..=SIGNED_56_MAX).contains(&value) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let (bits, inexact) = encode_fixed_float(value, format, (self.fcsr & 3) as u8);
        if inexact {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(bits)
    }

    /// Exact typed CVT.S.W result. The source is the low word of `fs`; `fd`
    /// remains unmodified until the caller commits this immutable bit result.
    pub fn try_cvt_s_w_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        self.try_fixed_to_float_raw(
            i64::from(self.f_bits(fs) as i32),
            FixedFloatFormat::Single,
            false,
        )
        .map(|bits| bits as u32)
    }

    /// Exact typed CVT.D.W result.
    pub fn try_cvt_d_w_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        self.try_fixed_to_float_raw(
            i64::from(self.f_bits(fs) as i32),
            FixedFloatFormat::Double,
            false,
        )
    }

    /// Exact typed CVT.S.L result. VR4300's L-format conversion accepts only
    /// values representable as a signed 56-bit integer; other sources raise
    /// the always-enabled Unimplemented Operation cause.
    pub fn try_cvt_s_l_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        self.try_fixed_to_float_raw(self.d_bits(fs) as i64, FixedFloatFormat::Single, true)
            .map(|bits| bits as u32)
    }

    /// Exact typed CVT.D.L result with the same signed-56 admission rule.
    pub fn try_cvt_d_l_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        self.try_fixed_to_float_raw(self.d_bits(fs) as i64, FixedFloatFormat::Double, true)
    }

    fn whole_function_fixed_to_float<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => {
                trap_unsupported("enabled COP1 fixed-to-float exception in whole-function lane")
            }
        }
    }

    pub fn cvt_s_w_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_w_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_d_w_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_w_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_s_l_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_l_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_d_l_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_l_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    fn fpu_unimplemented(&mut self) -> FpuException {
        self.fcsr &= !(0x3F << 12);
        self.fcsr |= 1 << 17;
        FpuException
    }

    /// Exact CVT.D.S result. VR4300 treats a denormal or legacy QNaN operand
    /// as Unimplemented; an SNaN raises Invalid and otherwise produces the
    /// MIPS-IV canonical double QNaN. Every finite normal single widens
    /// exactly, so FCSR.RM is immaterial.
    pub fn try_cvt_d_s_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        const D_QNAN: u64 = 0x7FF7_FFFF_FFFF_FFFF;
        let bits = self.f_bits(fs);
        let sign = u64::from(bits >> 31) << 63;
        let exponent = (bits >> 23) & 0xFF;
        let fraction = bits & 0x007F_FFFF;
        self.fcsr &= !(0x3F << 12);

        if exponent == 0 {
            return if fraction == 0 {
                Ok(sign)
            } else {
                Err(self.fpu_unimplemented())
            };
        }
        if exponent == 0xFF {
            return if fraction == 0 {
                Ok(sign | 0x7FF0_0000_0000_0000)
            } else if is_snan32(bits) {
                self.record_fpu_exception(SingleFpuCause::Invalid)?;
                Ok(D_QNAN)
            } else {
                Err(self.fpu_unimplemented())
            };
        }

        let double_exponent = u64::from(exponent + (1023 - 127)) << 52;
        Ok(sign | double_exponent | (u64::from(fraction) << 29))
    }

    /// Exact CVT.S.D result using integer IEEE decoding and FCSR.RM. VR4300
    /// detects tininess after rounding. A denormal result is supported only
    /// when FS is set and U/I are both disabled; that path flushes to signed
    /// zero or signed minimum-normal and raises U+I together.
    pub fn try_cvt_s_d_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        const S_QNAN: u32 = 0x7FBF_FFFF;
        const S_MAX: u32 = 0x7F7F_FFFF;
        const S_INFINITY: u32 = 0x7F80_0000;
        const S_MIN_NORMAL: u32 = 0x0080_0000;
        const FCSR_FS: u32 = 1 << 24;
        let bits = self.d_bits(fs);
        let negative = bits >> 63 != 0;
        let sign = (bits >> 32) as u32 & 0x8000_0000;
        let exponent = ((bits >> 52) & 0x7FF) as u32;
        let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
        self.fcsr &= !(0x3F << 12);

        if exponent == 0 {
            return if fraction == 0 {
                Ok(sign)
            } else {
                Err(self.fpu_unimplemented())
            };
        }
        if exponent == 0x7FF {
            return if fraction == 0 {
                Ok(sign | S_INFINITY)
            } else if is_snan64(bits) {
                self.record_fpu_exception(SingleFpuCause::Invalid)?;
                Ok(S_QNAN)
            } else {
                Err(self.fpu_unimplemented())
            };
        }

        let mode = (self.fcsr & 3) as u8;
        let unbiased = exponent as i32 - 1023;
        let significand = (1u64 << 52) | fraction;
        if unbiased > 127 {
            self.record_fpu_exceptions(FPU_CAUSE_O | FPU_CAUSE_I)?;
            return Ok(sign | overflowed_single(mode, negative, S_MAX, S_INFINITY));
        }

        if unbiased >= -126 {
            let (mut rounded, inexact) = round_shift_right(significand, 29, mode, negative);
            let mut output_exponent = unbiased;
            if rounded == 1 << 24 {
                rounded >>= 1;
                output_exponent += 1;
            }
            if output_exponent > 127 {
                self.record_fpu_exceptions(FPU_CAUSE_O | FPU_CAUSE_I)?;
                return Ok(sign | overflowed_single(mode, negative, S_MAX, S_INFINITY));
            }
            if inexact {
                self.record_fpu_exception(SingleFpuCause::Inexact)?;
            }
            return Ok(sign
                | (((output_exponent + 127) as u32) << 23)
                | (rounded as u32 & 0x007F_FFFF));
        }

        let shift = (-unbiased - 97) as u32;
        let (rounded, inexact) = round_shift_right(significand, shift, mode, negative);
        if rounded == 1 << 23 {
            if inexact {
                self.record_fpu_exception(SingleFpuCause::Inexact)?;
            }
            return Ok(sign | S_MIN_NORMAL);
        }

        let enables = ((self.fcsr >> 7) & 0x1F) as u8;
        if self.fcsr & FCSR_FS == 0 || enables & (FPU_CAUSE_U | FPU_CAUSE_I) != 0 {
            return Err(self.fpu_unimplemented());
        }
        self.record_fpu_exceptions(FPU_CAUSE_U | FPU_CAUSE_I)?;
        let magnitude = match (mode, negative) {
            (2, false) | (3, true) => S_MIN_NORMAL,
            _ => 0,
        };
        Ok(sign | magnitude)
    }

    fn whole_function_float_to_float<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => trap_unsupported("COP1 float-to-float exception in whole-function lane"),
        }
    }

    pub fn cvt_d_s_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_s_bits(fs);
        Self::whole_function_float_to_float(result)
    }

    pub fn cvt_s_d_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_d_bits(fs);
        Self::whole_function_float_to_float(result)
    }

    fn try_fpu_to_i32_raw(
        &mut self,
        value: f64,
        signaling_nan: bool,
        unimplemented_operand: bool,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        self.fcsr &= !(0x3F << 12);
        if signaling_nan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
            return Ok(i32::MAX);
        }
        if unimplemented_operand {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let rounded = self.rounded_for_mode(value, fixed_mode);
        if !(-2_147_483_648.0..2_147_483_648.0).contains(&rounded) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        if rounded != value {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(rounded as i32)
    }

    fn try_fpu_to_i64_raw(
        &mut self,
        value: f64,
        signaling_nan: bool,
        unimplemented_operand: bool,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        self.fcsr &= !(0x3F << 12);
        if signaling_nan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
            return Ok(i64::MAX);
        }
        if unimplemented_operand {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let rounded = self.rounded_for_mode(value, fixed_mode);
        if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&rounded) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        if rounded != value {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(rounded as i64)
    }

    /// Convert a raw single-precision operand to a W result. The immutable
    /// typed result is returned before the caller commits `fd`.
    pub fn try_fpu_to_i32_s(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        let bits = self.f_bits(fs);
        self.try_fpu_to_i32_raw(
            f32::from_bits(bits) as f64,
            is_snan32(bits),
            is_qnan32(bits) || is_subnormal32(bits) || f32::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Double-precision counterpart of [`RecompContext::try_fpu_to_i32_s`].
    pub fn try_fpu_to_i32_d(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        let bits = self.d_bits(fs);
        self.try_fpu_to_i32_raw(
            f64::from_bits(bits),
            is_snan64(bits),
            is_qnan64(bits) || is_subnormal64(bits) || f64::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Convert a raw single-precision operand to an L result.
    pub fn try_fpu_to_i64_s(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        let bits = self.f_bits(fs);
        self.try_fpu_to_i64_raw(
            f32::from_bits(bits) as f64,
            is_snan32(bits),
            is_qnan32(bits) || is_subnormal32(bits) || f32::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Double-precision counterpart of [`RecompContext::try_fpu_to_i64_s`].
    pub fn try_fpu_to_i64_d(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        let bits = self.d_bits(fs);
        self.try_fpu_to_i64_raw(
            f64::from_bits(bits),
            is_snan64(bits),
            is_qnan64(bits) || is_subnormal64(bits) || f64::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    fn whole_function_conversion<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => {
                trap_unsupported("enabled COP1 float-to-fixed exception in whole-function lane")
            }
        }
    }

    pub fn fpu_to_i32_s(&mut self, fs: u8, fixed_mode: Option<u8>) -> i32 {
        let result = self.try_fpu_to_i32_s(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i32_d(&mut self, fs: u8, fixed_mode: Option<u8>) -> i32 {
        let result = self.try_fpu_to_i32_d(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i64_s(&mut self, fs: u8, fixed_mode: Option<u8>) -> i64 {
        let result = self.try_fpu_to_i64_s(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i64_d(&mut self, fs: u8, fixed_mode: Option<u8>) -> i64 {
        let result = self.try_fpu_to_i64_d(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    fn rounded_for_mode(&self, value: f64, fixed_mode: Option<u8>) -> f64 {
        match fixed_mode.unwrap_or((self.fcsr & 3) as u8) {
            0 => value.round_ties_even(),
            1 => value.trunc(),
            2 => value.ceil(),
            3 => value.floor(),
            _ => unreachable!("FCSR.RM and fixed rounding modes are two bits"),
        }
    }

    /// Record one precise IEEE exception after the operation has cleared its
    /// per-operation Cause field. VR4300 User's Manual section 6.3.2.2: Cause
    /// is set in either case; an enabled exception traps without changing Flag,
    /// while a disabled exception completes and accumulates the sticky Flag.
    /// This helper is single-cause-only. A future operation with several
    /// simultaneous causes must set every Cause, test all matching Enables,
    /// and set no new Flags if any cause is enabled; sequential calls here
    /// would incorrectly commit a disabled cause's Flag before a later enabled
    /// cause is observed.
    #[inline]
    fn record_fpu_exception(&mut self, exception: SingleFpuCause) -> Result<(), FpuException> {
        self.record_fpu_exceptions(1 << exception.index())
    }

    fn record_fpu_exceptions(&mut self, exceptions: u8) -> Result<(), FpuException> {
        assert_eq!(exceptions & !0x1F, 0, "IEEE FPU cause mask exceeds VZOUI");
        self.fcsr |= u32::from(exceptions) << 12;
        if ((self.fcsr >> 7) as u8) & exceptions != 0 {
            Err(FpuException)
        } else {
            self.fcsr |= u32::from(exceptions) << 2;
            Ok(())
        }
    }

    /// Fold the IEEE conditions the soft-float shim reported into FCSR and decide
    /// whether the op traps, returning `true` when an ENABLED exception fired.
    ///
    /// The VR4300 (User's Manual section 6.6, "Floating-Point Exceptions")
    /// distinguishes two outcomes per operation, and the FCSR update differs:
    ///
    /// * **Trapped** — any raised condition whose FCSR Enable bit is set. The
    ///   FPU writes the FCSR **Cause** field (so the handler can read which
    ///   condition trapped) but leaves the sticky **Flags** field and the
    ///   destination register **unchanged**, then vectors to the ExcCode-15
    ///   general exception. The caller must NOT commit the computed result.
    /// * **Not trapped** — no enabled condition fired. The FPU writes both the
    ///   Cause field and ORs the sticky Flags bits, and the destination register
    ///   takes the computed result.
    ///
    /// The Cause field is fully rewritten every operation (cleared first, exactly
    /// as [`RecompContext::round_for_mode`] does for conversions); the sticky
    /// Flags bits are only OR-ed in on the not-trapped path.
    #[inline]
    fn apply_fpu_flags(&mut self, flags: crate::fpu::Flags) -> bool {
        // Assemble the Cause bits this op signalled. The five IEEE conditions
        // occupy Cause 16:12 (index 0..4); the Unimplemented Operation (E) bit
        // is Cause bit 17 (index 5). The full 6-bit Cause field is 17:12.
        let ieee = u32::from(flags.inexact)
            | (u32::from(flags.underflow) << 1)
            | (u32::from(flags.overflow) << 2)
            | (u32::from(flags.divbyzero) << 3)
            | (u32::from(flags.invalid) << 4);
        let cause = ieee | (u32::from(flags.unimplemented) << 5);

        // A trap fires iff any IEEE condition whose Enable bit (FCSR 11:7) is
        // set was signalled, OR the Unimplemented Operation bit is set. E has
        // NO Enable bit and is UNMASKABLE — it always vectors to ExcCode 15
        // (VR4300 User's Manual section 7.5). Enables never gate it.
        let enables = (self.fcsr >> 7) & 0x1F;
        let trapped = (ieee & enables != 0) || flags.unimplemented;

        // Cause is rewritten unconditionally (clear the old 17:12 field, install
        // the freshly signalled conditions) so the handler sees exactly what
        // this op raised, on both the trapped and untrapped paths.
        self.fcsr = (self.fcsr & !(0x3F << 12)) | (cause << 12);

        if !trapped {
            // No exception fired: accumulate the sticky Flag bits (6:2). E has
            // no sticky Flag bit (only bits 6:2 exist), and it always traps, so
            // it is never reached here — the IEEE bits are the only sticky ones.
            self.fcsr |= ieee << 2;
        }
        trapped
    }

    /// Read the two-bit FCSR rounding mode (RM) field.
    #[inline]
    fn fcsr_rm(&self) -> u8 {
        (self.fcsr & 3) as u8
    }

    // --- COP1 arithmetic routed through the IEEE soft-float shim (`fpu`). ---
    //
    // Each reads the operand bits, performs the op under FCSR.RM in `crate::fpu`
    // (host-independent, IEEE-exact), then folds the returned IEEE flags into
    // FCSR via `apply_fpu_flags`, which reports whether an ENABLED exception
    // fired.
    //
    // # Result-commit ordering (the enabled-exception rule)
    //
    // On an enabled FP exception the VR4300 traps BEFORE writing the destination
    // register (User's Manual section 6.6): the result is discarded and only the
    // FCSR Cause field records the condition. So these methods compute first,
    // fold the flags, and write the destination ONLY when no trap fired. Each
    // returns `true` when it trapped — the emitted block lane checks that return
    // and exits to the ExcCode-15 fault handler (exactly as the integer-overflow
    // lane checks `checked_add` and exits to the IntegerOverflow handler). The
    // straight-line / whole-function lane instead panics loudly on a trap
    // (mirroring the `.expect("MIPS ADD integer overflow")` shape), since that
    // lane has no exception-return ABI yet.
    //
    // Note the operand bits are sampled BEFORE the destination is written, so an
    // in-place `fd == fs`/`fd == ft` op reads the original inputs even on the
    // committed (non-trapping) path.

    /// ADD.S: `fd = fs + ft` honoring FCSR.RM, with IEEE flags. Returns `true` if
    /// an enabled exception trapped (destination left unwritten).
    #[inline]
    #[must_use]
    pub fn fpu_add_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        self.fpu_add_s_admitted(
            fd,
            fs,
            ft,
            self.rn_finite_fast_active,
            self.rn_finite_fast_selected,
        )
    }

    #[cfg(test)]
    pub(super) fn fpu_add_s_selected(&mut self, fd: u8, fs: u8, ft: u8, allow_fast: bool) -> bool {
        self.fpu_add_s_admitted(fd, fs, ft, allow_fast && self.fcsr_rm() == 0, allow_fast)
    }

    #[inline(always)]
    fn fpu_add_s_admitted(
        &mut self,
        fd: u8,
        fs: u8,
        ft: u8,
        admit_fast: bool,
        _selector_enabled: bool,
    ) -> bool {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        let fast = admit_fast
            .then(|| crate::fpu::try_add_s_rn_finite(a, b))
            .flatten();
        let (bits, flags) = fast.unwrap_or_else(|| crate::fpu::add_s(a, b, self.fcsr_rm()));
        #[cfg(feature = "cop1-fast-receipt")]
        self.note_rn_finite_selection(1, fast.is_some(), _selector_enabled);
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// SUB.S: `fd = fs - ft`.
    #[inline]
    #[must_use]
    pub fn fpu_sub_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        self.fpu_sub_s_admitted(
            fd,
            fs,
            ft,
            self.rn_finite_fast_active,
            self.rn_finite_fast_selected,
        )
    }

    #[cfg(test)]
    pub(super) fn fpu_sub_s_selected(&mut self, fd: u8, fs: u8, ft: u8, allow_fast: bool) -> bool {
        self.fpu_sub_s_admitted(fd, fs, ft, allow_fast && self.fcsr_rm() == 0, allow_fast)
    }

    #[inline(always)]
    fn fpu_sub_s_admitted(
        &mut self,
        fd: u8,
        fs: u8,
        ft: u8,
        admit_fast: bool,
        _selector_enabled: bool,
    ) -> bool {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        let fast = admit_fast
            .then(|| crate::fpu::try_sub_s_rn_finite(a, b))
            .flatten();
        let (bits, flags) = fast.unwrap_or_else(|| crate::fpu::sub_s(a, b, self.fcsr_rm()));
        #[cfg(feature = "cop1-fast-receipt")]
        self.note_rn_finite_selection(2, fast.is_some(), _selector_enabled);
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// MUL.S: `fd = fs * ft`.
    #[inline]
    #[must_use]
    pub fn fpu_mul_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        self.fpu_mul_s_admitted(
            fd,
            fs,
            ft,
            self.rn_finite_fast_active,
            self.rn_finite_fast_selected,
        )
    }

    #[cfg(test)]
    pub(super) fn fpu_mul_s_selected(&mut self, fd: u8, fs: u8, ft: u8, allow_fast: bool) -> bool {
        self.fpu_mul_s_admitted(fd, fs, ft, allow_fast && self.fcsr_rm() == 0, allow_fast)
    }

    #[inline(always)]
    fn fpu_mul_s_admitted(
        &mut self,
        fd: u8,
        fs: u8,
        ft: u8,
        admit_fast: bool,
        _selector_enabled: bool,
    ) -> bool {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        let fast = admit_fast
            .then(|| crate::fpu::try_mul_s_rn_finite(a, b))
            .flatten();
        let (bits, flags) = fast.unwrap_or_else(|| crate::fpu::mul_s(a, b, self.fcsr_rm()));
        #[cfg(feature = "cop1-fast-receipt")]
        self.note_rn_finite_selection(4, fast.is_some(), _selector_enabled);
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    #[cfg(feature = "cop1-fast-receipt")]
    #[inline]
    fn note_rn_finite_selection(&mut self, op_bit: u8, fast: bool, selected: bool) {
        if !selected {
            return;
        }
        let seen = if fast {
            &mut self.rn_finite_fast_seen
        } else {
            &mut self.rn_finite_fallback_seen
        };
        if *seen & op_bit == 0 {
            *seen |= op_bit;
            crate::fpu::note_experimental_single_selection_once(op_bit, fast);
        }
    }

    /// DIV.S: `fd = fs / ft`.
    #[inline]
    #[must_use]
    pub fn fpu_div_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::div_s(self.f_bits(fs), self.f_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// SQRT.S: `fd = sqrt(fs)`, correctly rounded under FCSR.RM.
    #[inline]
    #[must_use]
    pub fn fpu_sqrt_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::sqrt_s(self.f_bits(fs), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// ABS.S: `fd = |fs|` (sign-bit op; Invalid only on an SNaN operand).
    #[inline]
    #[must_use]
    pub fn fpu_abs_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::abs_s(self.f_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// NEG.S: `fd = -fs` (sign-bit op; Invalid only on an SNaN operand).
    #[inline]
    #[must_use]
    pub fn fpu_neg_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::neg_s(self.f_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// ADD.D: `fd = fs + ft`.
    #[inline]
    #[must_use]
    pub fn fpu_add_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::add_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// SUB.D: `fd = fs - ft`.
    #[inline]
    #[must_use]
    pub fn fpu_sub_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::sub_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// MUL.D: `fd = fs * ft`.
    #[inline]
    #[must_use]
    pub fn fpu_mul_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::mul_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// DIV.D: `fd = fs / ft`.
    #[inline]
    #[must_use]
    pub fn fpu_div_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::div_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// SQRT.D: `fd = sqrt(fs)`, correctly rounded under FCSR.RM.
    #[inline]
    #[must_use]
    pub fn fpu_sqrt_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::sqrt_d(self.d_bits(fs), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// ABS.D: `fd = |fs|`.
    #[inline]
    #[must_use]
    pub fn fpu_abs_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::abs_d(self.d_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// NEG.D: `fd = -fs`.
    #[inline]
    #[must_use]
    pub fn fpu_neg_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::neg_d(self.d_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    // --- FP conditional moves (MOVF/MOVT/MOVZ/MOVN.fmt). ---
    //
    // These copy the source FPR to the destination FPR only when a predicate
    // holds; when it does not, the destination is left UNCHANGED. They are pure
    // bit copies — no rounding, no IEEE exception, no FCSR effect (VR4300
    // User's Manual, MOVF/MOVT/MOVZ/MOVN.fmt). The move width follows the format
    // (single copies 32 bits through the FR-aware single accessor; double copies
    // 64 bits through the double accessor), so the FR even/odd model applies
    // uniformly.

    /// `MOVF.S`/`MOVT.S`: `fd = fs` (single) iff `fpu_cond == tf`.
    #[inline]
    pub fn fpu_movcf_s(&mut self, fd: u8, fs: u8, tf: bool) {
        if self.fpu_cond == tf {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVF.D`/`MOVT.D`: `fd = fs` (double) iff `fpu_cond == tf`.
    #[inline]
    pub fn fpu_movcf_d(&mut self, fd: u8, fs: u8, tf: bool) {
        if self.fpu_cond == tf {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// `MOVZ.S`: `fd = fs` (single) iff GPR `rt` reads zero (full 64 bits).
    #[inline]
    pub fn fpu_movz_s(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) == 0 {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVN.S`: `fd = fs` (single) iff GPR `rt` reads nonzero.
    #[inline]
    pub fn fpu_movn_s(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) != 0 {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVZ.D`: `fd = fs` (double) iff GPR `rt` reads zero.
    #[inline]
    pub fn fpu_movz_d(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) == 0 {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// `MOVN.D`: `fd = fs` (double) iff GPR `rt` reads nonzero.
    #[inline]
    pub fn fpu_movn_d(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) != 0 {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// Evaluate any of the sixteen C.cond.fmt predicates. The low three funct
    /// bits select unordered/equal/less participation; bit 3 selects signaling
    /// behavior. Quiet compares still signal on an SNaN.
    pub fn try_fpu_compare(
        &mut self,
        lhs: f64,
        rhs: f64,
        lhs_snan: bool,
        rhs_snan: bool,
        cond: u8,
    ) -> Result<(), FpuException> {
        assert!(cond < 16, "COP1 compare predicate exceeds four bits");
        self.fcsr &= !(0x3F << 12);
        let unordered = lhs.is_nan() || rhs.is_nan();
        let condition = (unordered && cond & 1 != 0)
            || (!unordered && lhs == rhs && cond & 2 != 0)
            || (!unordered && lhs < rhs && cond & 4 != 0);
        if (unordered && cond & 0x8 != 0) || lhs_snan || rhs_snan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
        }
        self.fpu_cond = condition;
        Ok(())
    }

    #[inline]
    pub fn try_fpu_compare_s(&mut self, fs: u8, ft: u8, cond: u8) -> Result<(), FpuException> {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        self.try_fpu_compare(
            f32::from_bits(a) as f64,
            f32::from_bits(b) as f64,
            is_snan32(a),
            is_snan32(b),
            cond,
        )
    }

    #[inline]
    pub fn try_fpu_compare_d(&mut self, fs: u8, ft: u8, cond: u8) -> Result<(), FpuException> {
        let a = self.d_bits(fs);
        let b = self.d_bits(ft);
        self.try_fpu_compare(
            f64::from_bits(a),
            f64::from_bits(b),
            is_snan64(a),
            is_snan64(b),
            cond,
        )
    }

    /// Whole-function compatibility boundary. Arbitrary-PC lanes use the
    /// typed `try_` form so they can enter the guest exception vector.
    #[inline]
    pub fn fpu_compare_s(&mut self, fs: u8, ft: u8, cond: u8) {
        if self.try_fpu_compare_s(fs, ft, cond).is_err() {
            trap_unsupported("enabled COP1 compare exception in whole-function lane");
        }
    }

    /// Double-precision counterpart of [`RecompContext::fpu_compare_s`].
    #[inline]
    pub fn fpu_compare_d(&mut self, fs: u8, ft: u8, cond: u8) {
        if self.try_fpu_compare_d(fs, ft, cond).is_err() {
            trap_unsupported("enabled COP1 compare exception in whole-function lane");
        }
    }

    // ================================================================
    // COP1 / FPU register file.
    //
    // The VR4300 manual sections 5.2 and 5.3 define 32 physical FGRs. In FR=0
    // each contributes one 32-bit word and an even doubleword FPR joins the
    // adjacent even/odd words. In FR=1 each FPR is one independent 64-bit FGR.
    // Keeping that physical shape means toggling Status.FR never rearranges or
    // discards state. All typed operations route through these raw accessors.
    // ================================================================

    /// Snapshot every physical FGR without applying the active FR view.
    ///
    /// This compatibility accessor retains the legacy differential-test name,
    /// but its entries are physical registers rather than FR-shaped slots.
    pub fn fpr_slots(&self) -> [u64; 32] {
        self.fpr.physical_state().into_words()
    }

    /// Whether Status.FR selects 32 independent 64-bit FPRs.
    #[inline]
    pub fn fpu_fr(&self) -> bool {
        self.cop0_status & COP0_STATUS_FR != 0
    }

    /// Read the low word of physical FGR `idx`. Under FR=0 these 32 words are
    /// the complete FGR file; under FR=1 this is the single/W view of the same
    /// independent 64-bit register.
    #[inline]
    pub fn f_bits(&self, idx: u8) -> u32 {
        self.fpr.word(idx)
    }

    /// Write the low word of physical FGR `idx`, preserving the upper word
    /// that is latent in FR=0 and independently visible in FR=1.
    #[inline]
    pub fn set_f_bits(&mut self, idx: u8, bits: u32) {
        self.fpr.set_word(idx, bits);
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

    /// Read a doubleword FPR. FR=0 joins the low words of adjacent even/odd
    /// FGRs; FR=1 reads one complete FGR and permits odd indices.
    #[inline]
    pub fn d_bits(&self, idx: u8) -> u64 {
        self.fpr.doubleword(idx, self.fpu_fr())
    }

    /// Write a doubleword through the active FR view. An FR=0 paired write
    /// preserves both physical FGR upper words so a later FR=1 view recovers
    /// them unchanged.
    #[inline]
    pub fn set_d_bits(&mut self, idx: u8, bits: u64) {
        let fr = self.fpu_fr();
        self.fpr.set_doubleword(idx, bits, fr);
    }

    /// Complete physical FGR state for deterministic state/evidence snapshots.
    /// This is view-independent: unlike 32 single reads it retains every upper
    /// word that FR=0 makes temporarily inaccessible.
    pub fn physical_fgr_state(&self) -> PhysicalFgrState {
        self.fpr.physical_state()
    }

    /// Replace the complete physical FGR file without interpreting the active
    /// FR view. ABI adapters use this only after validating that their packed
    /// C context mode agrees with CP0.Status.FR.
    pub fn replace_physical_fgr_state(&mut self, state: PhysicalFgrState) {
        self.fpr.replace_physical_state(state);
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
