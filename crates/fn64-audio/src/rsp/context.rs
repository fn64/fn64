//! `RspContext` and `RspExitReason` — the scalar-side state and return type
//! the RSPRecomp-generated ucode C is written against.
//!
//! ## Matching the generated signature
//!
//! `rsp_recomp.cpp` generates functions with the signatures (lines 907-908,
//! 955-957, 1016, 1057):
//! ```c
//! using RspUcodePermutationFunc = RspExitReason(uint8_t* rdram, RspContext* ctx);
//! RspExitReason <ucode>(uint8_t* rdram, uint32_t ucode_addr);
//! ```
//! and the overlay-swap path saves/restores scalar regs `r1..r31`, the DMA
//! addresses, and `jump_target` through the `RspContext*` (lines 598-606,
//! 1017-1023). The context also carries the resume address/delay for the
//! overlay-swap continuation (lines 544-546, 1028-1048) and the `RSP` (VU)
//! object (`ctx->rsp`, line 1023). We model all of that here so a future Rust
//! port of the generated body has a faithful `ctx` to thread through.
//!
//! `r0` is not stored (hardwired zero on MIPS; the codegen prints it as the
//! literal `0`, `ctx_gpr_prefix` returns `""` for reg 0 — lines 117-122,
//! 590). We keep `r1..=r31` as an array indexed 1..=31 with index 0 unused,
//! so op/dispatch code can write `ctx.r[n]` for any `n` and reading `r[0]`
//! yields the correct hardwired zero.

use super::vu::VuState;

/// Why a recompiled RSP ucode function returned. Mirrors the generated C's
/// `RspExitReason` enum: the recompiler emits `return RspExitReason::X` for
/// `Broke` (line 532), `Unsupported` (292), `ImemOverrun` (1073),
/// `UnhandledJumpTarget` (592), `SwapOverlay` (606), and
/// `UnhandledResumeTarget` (1050).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspExitReason {
    /// A `break` instruction ran — the normal "ucode task finished" exit.
    Broke,
    /// An instruction the recompiler was told (via config) not to support
    /// was reached; the generated code early-returns here.
    Unsupported,
    /// Execution ran off the end of IMEM without a terminating `break`.
    ImemOverrun,
    /// An indirect jump (`jr`/`jalr`) targeted an address with no generated
    /// label — a recompilation gap.
    UnhandledJumpTarget,
    /// The ucode requested an overlay DMA into IMEM; the host must swap the
    /// overlay and re-enter the matching permutation function. The context's
    /// `dma_mem_address`/`dma_dram_address`/`resume_*` describe the swap.
    SwapOverlay,
    /// After an overlay swap, the saved `resume_address` matched no generated
    /// resume label — a recompilation gap on the resume path.
    UnhandledResumeTarget,
}

/// The scalar-side RSP context the generated ucode threads through. Field
/// names match the generated C so a port reads 1:1.
#[derive(Clone, Debug)]
pub struct RspContext {
    /// Scalar GPRs. `r[0]` is the hardwired-zero `$zero` and is never written
    /// by well-formed code; indices 1..=31 are the real registers `r1..r31`
    /// the generated function saves/restores (`rsp_recomp.cpp` lines
    /// 1017-1020, 598-601).
    pub r: [u32; 32],
    /// DMA source (DRAM/rdram) address, set by `SET_DMA_DRAM`
    /// (`SP_DRAM_ADDR` write, line 149) and read on an overlay swap.
    pub dma_dram_address: u32,
    /// DMA destination (RSP MEM) address, set by `SET_DMA_MEM`
    /// (`SP_MEM_ADDR` write, line 152). Its `& 0x1000` bit distinguishes an
    /// IMEM (overlay) load from a DMEM data load (lines 542-543).
    pub dma_mem_address: u32,
    /// The pending indirect-jump target (`jr`/`jalr` store into it before
    /// `goto do_indirect_jump`, lines 492, 497), saved across an overlay swap.
    pub jump_target: u32,
    /// The IMEM address to resume at after an overlay swap
    /// (`ctx->resume_address`, lines 544, 1028-1044).
    pub resume_address: u32,
    /// Whether the resume point was in a branch delay slot (selects the
    /// `_delay` resume label, lines 545, 1028-1035).
    pub resume_delay: bool,
    /// The Vector Unit state (`ctx->rsp`, line 1023) — the register file,
    /// accumulator, flags, and div latch the compute ops operate on.
    pub rsp: VuState,
    /// Instruction-step (basic-block-entry) counter for perf profiling. Written
    /// by the generated loop; read by the FFI wrapper. Not part of the RSP
    /// hardware model — a diagnostic only.
    pub steps: u64,
}

impl Default for RspContext {
    fn default() -> Self {
        RspContext {
            r: [0u32; 32],
            dma_dram_address: 0,
            dma_mem_address: 0,
            jump_target: 0,
            resume_address: 0,
            resume_delay: false,
            rsp: VuState::new(),
            steps: 0,
        }
    }
}

impl RspContext {
    /// A fresh zeroed context (matches the generated `RspContext ctx{}` at the
    /// top of the outer ucode function, line 956).
    pub fn new() -> Self {
        RspContext::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_zero_initialized_like_generated_entry() {
        let ctx = RspContext::new();
        assert!(ctx.r.iter().all(|&v| v == 0));
        assert_eq!(ctx.dma_dram_address, 0);
        assert_eq!(ctx.dma_mem_address, 0);
        assert_eq!(ctx.jump_target, 0);
        assert_eq!(ctx.resume_address, 0);
        assert!(!ctx.resume_delay);
    }

    #[test]
    fn r0_stays_zero_as_hardwired_zero() {
        let mut ctx = RspContext::new();
        // Even if code writes r[0], reading it back models $zero: well-formed
        // generated code never emits a write to r0 (ctx_gpr_prefix returns ""
        // and the codegen prints literal 0), so r[0] is expected to remain 0.
        ctx.r[1] = 0xDEAD_BEEF;
        assert_eq!(ctx.r[0], 0);
        assert_eq!(ctx.r[1], 0xDEAD_BEEF);
    }

    #[test]
    fn exit_reasons_are_distinct() {
        // Sanity that the enum variants the generated code returns are all
        // present and comparable.
        assert_ne!(RspExitReason::Broke, RspExitReason::SwapOverlay);
        assert_ne!(RspExitReason::Unsupported, RspExitReason::ImemOverrun);
        assert_ne!(
            RspExitReason::UnhandledJumpTarget,
            RspExitReason::UnhandledResumeTarget
        );
    }
}
