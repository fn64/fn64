//! The typed runtime substrate the recompiled Rust calls into: the scalar
//! register file, the DMA engine, and the CP2 vector load/store + scalar
//! transfer semantics that the [`crate::rsp::vu_ops`] compute layer does NOT
//! cover.
//!
//! ## Why this is type-safe
//!
//! The generated code never does a raw byte reinterpret. It calls named
//! methods here (`vload`, `vstore`, `mfc2`, `dma_read`, …) whose signatures
//! are `u32`/`i16`/`[i16; 8]`/`&mut VuState` — the byte-lane swizzle,
//! sign-extension, and element rotation live in ONE audited place, so the
//! whole class of "reinterpret a `u8*` at the wrong offset" bug that plagues
//! C recomp cannot appear in the emitter's output.
//!
//! ## Provenance
//!
//! The vector load/store element semantics follow the public SGI *Nintendo 64
//! RSP Programmer's Guide*, Chapter 3 and instruction appendix: a
//! quad load (`LQV`) fills the vector from the addressed 16-byte-aligned
//! window; `LDV`/`LLV`/`LSV`/`LBV` fill 8/4/2/1 bytes starting at element
//! `e`; the packed loads (`LPV`/`LUV`/`LHV`/`LFV`) spread bytes across lanes
//! with a shift. No GPL `rsp_vu_impl.hpp` was read. The DMA engine mirrors
//! `rsp_recomp.cpp`'s `SET_DMA_*`/`DO_DMA_*` macro contract (copy between a
//! DRAM window in `rdram` and the RSP DMEM, byte-for-byte).

use crate::rsp::context::{RspContext, RspExitReason};
use crate::rsp::decode::{VLoadOp, VStoreOp};
use crate::rsp::dmem::{Dmem, DMEM_SIZE};
use crate::rsp::vu::{Vec8, VuState, LANES};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

static DMA_TRACE_SEQ: AtomicU64 = AtomicU64::new(0);

/// One diagnostic observation of an SP DMA command at its execution point.
///
/// This journal is intentionally absent from [`RspArchitecturalState`] and
/// [`RspMachineState`]. It can characterize a microcode as a black box, but
/// cannot influence execution, comparison, or commit authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspDmaJournalEntry {
    pub direction: RspDmaDirection,
    pub effective_dram_address: u32,
    pub sp_mem_address: u32,
    pub raw_length_descriptor: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspDmaDirection {
    Read,
    Write,
}

/// One RDP command-DMA range submitted through the RSP's DPC CP0 registers.
/// `xbus` records whether the command source is RSP DMEM rather than RDRAM.
/// XBUS bytes are captured at submission time because the ucode reuses its
/// small DMEM command ring before deferred host rendering consumes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspDpSubmission {
    pub start: u32,
    pub end: u32,
    pub xbus: bool,
    /// Logical big-endian XBUS bytes; empty for RDRAM-backed submissions.
    pub payload: Vec<u8>,
    /// Logical command words captured when CMD_END admits the range.
    pub words: Vec<u32>,
}

/// The logical IMEM byte range replaced by one pending SP read-DMA.
///
/// RSP DMA wraps within the 4 KiB memory bank, so membership is expressed as
/// circular distance rather than a non-wrapping host range. The descriptor's
/// rectangular lines are contiguous on the RSP-memory side; DRAM `skip`
/// changes only the source stride. This follows the public SGI *Nintendo 64
/// RSP Programmer's Guide*, DMA engine length/count/skip register semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImemDmaSpan {
    start: usize,
    byte_len: usize,
}

/// Pointer-free RSP architectural state carried across an HLE/LLE boundary.
///
/// DMEM, IMEM, and RDRAM remain in their typed owners. This value owns every
/// future-visible non-memory register and queued DPC submission, but excludes
/// the interpreter's diagnostic instruction counter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspArchitecturalState {
    gprs: [u32; 32],
    dma_dram_address: u32,
    dma_mem_address: u32,
    jump_target: u32,
    resume_address: u32,
    resume_delay: bool,
    vu: VuState,
    sp_status: u32,
    sp_semaphore: bool,
    dma_read_length: u32,
    dma_write_length: u32,
    dp_start: u32,
    dp_end: u32,
    dp_current: u32,
    dp_status: u32,
    dp_clock: u32,
    dp_busy: u32,
    dp_pipe_busy: u32,
    dp_tmem_busy: u32,
    dp_submissions: Vec<RspDpSubmission>,
}

impl RspArchitecturalState {
    pub const fn gprs(&self) -> &[u32; 32] {
        &self.gprs
    }

    pub const fn dma_dram_address(&self) -> u32 {
        self.dma_dram_address
    }

    pub const fn dma_mem_address(&self) -> u32 {
        self.dma_mem_address
    }

    pub const fn jump_target(&self) -> u32 {
        self.jump_target
    }

    pub const fn resume_address(&self) -> u32 {
        self.resume_address
    }

    pub const fn resume_delay(&self) -> bool {
        self.resume_delay
    }

    pub const fn vu(&self) -> &VuState {
        &self.vu
    }

    pub const fn sp_status(&self) -> u32 {
        self.sp_status
    }

    pub const fn sp_semaphore(&self) -> bool {
        self.sp_semaphore
    }

    pub const fn dma_read_length(&self) -> u32 {
        self.dma_read_length
    }

    pub const fn dma_write_length(&self) -> u32 {
        self.dma_write_length
    }

    pub const fn dp_start(&self) -> u32 {
        self.dp_start
    }

    pub const fn dp_end(&self) -> u32 {
        self.dp_end
    }

    pub const fn dp_current(&self) -> u32 {
        self.dp_current
    }

    pub const fn dp_status(&self) -> u32 {
        self.dp_status
    }

    pub const fn dp_clock(&self) -> u32 {
        self.dp_clock
    }

    pub const fn dp_busy(&self) -> u32 {
        self.dp_busy
    }

    pub const fn dp_pipe_busy(&self) -> u32 {
        self.dp_pipe_busy
    }

    pub const fn dp_tmem_busy(&self) -> u32 {
        self.dp_tmem_busy
    }

    pub fn dp_submissions(&self) -> &[RspDpSubmission] {
        &self.dp_submissions
    }
}

/// Complete non-memory interpreter state. Architectural state is kept as an
/// owned value so an HLE implementation can transfer it without inheriting
/// the diagnostic instruction count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspMachineState {
    architectural: RspArchitecturalState,
    diagnostic_steps: u64,
}

impl RspMachineState {
    /// Wrap architectural state for APIs that traffic in complete machine
    /// snapshots. The new diagnostic counter starts at zero.
    pub fn from_architectural_state(architectural: RspArchitecturalState) -> Self {
        Self {
            architectural,
            diagnostic_steps: 0,
        }
    }

    pub const fn architectural_state(&self) -> &RspArchitecturalState {
        &self.architectural
    }

    pub fn into_architectural_state(self) -> RspArchitecturalState {
        self.architectural
    }

    pub const fn diagnostic_steps(&self) -> u64 {
        self.diagnostic_steps
    }

    /// Convert an rspboot-to-ucode handoff into the bounded state retained
    /// after an optimized HLE backend reports success.
    ///
    /// The backend does not expose the ucode's post-execution scalar/VU
    /// image, so this preserves rspboot-entry registers while consuming the
    /// overlay resume latch that only the represented ucode phase could use.
    /// Callers must label this as compatibility evidence, never as exact LLE
    /// post-task state.
    pub fn into_hle_compatibility_architectural_state(mut self) -> RspArchitecturalState {
        self.architectural.resume_address = 0;
        self.architectural.resume_delay = false;
        self.architectural
    }
}

impl ImemDmaSpan {
    /// Whether `pc` selects an instruction byte replaced by this DMA.
    pub fn contains_pc(self, pc: u32) -> bool {
        let offset = pc as usize & (DMEM_SIZE - 1);
        let distance = offset.wrapping_sub(self.start) & (DMEM_SIZE - 1);
        distance < self.byte_len.min(DMEM_SIZE)
    }
}

/// The full RSP machine state the recompiled ucode runs against: scalar regs
/// + DMA context (in [`RspContext`]), the 4 KiB DMEM, and the VU state.
///
/// `rdram` is borrowed for the lifetime of a single ucode run so the DMA
/// engine can move bytes between main memory and DMEM. It is `&mut [u8]`
/// (a checked slice) — never a raw pointer inside this type — so every DMA
/// access is bounds-checked.
pub struct RspMachine<'a> {
    /// Scalar regs, DMA addresses, jump/resume, and the VU state.
    pub ctx: RspContext,
    /// The 4 KiB data memory the compute ops read/write.
    pub dmem: Dmem,
    /// Main memory (rdram), for DMA. Bounds-checked on every access.
    pub rdram: &'a mut [u8],
    /// Explicit RDRAM/host-alias ranges admitted to the SP DMA engine. The
    /// default is the complete supplied slice; whole-game adapters narrow it
    /// to physical RDRAM plus loaded static-overlay aliases.
    dma_rdram_ranges: Vec<Range<usize>>,
    /// RSP status bits as observed through CP0 $c4. DMA is synchronous in the
    /// recompiled runtime, so BUSY/FULL never remain set across instructions.
    sp_status: u32,
    sp_semaphore: bool,
    dma_read_length: u32,
    dma_write_length: u32,
    dp_start: u32,
    dp_end: u32,
    dp_current: u32,
    dp_status: u32,
    dp_clock: u32,
    dp_busy: u32,
    dp_pipe_busy: u32,
    dp_tmem_busy: u32,
    dp_submissions: Vec<RspDpSubmission>,
    /// Half-open RDRAM byte ranges written by SP DMA since the previous
    /// drain. The admitted-range check above the copy remains authoritative;
    /// this log lets callers commit only bytes the RSP could have changed.
    rdram_writes: Vec<(usize, usize)>,
    dma_journal: Vec<RspDmaJournalEntry>,
}

impl<'a> RspMachine<'a> {
    /// A fresh machine bound to `rdram`, with the standard RSP boot state
    /// (`r1 = 0xFC0`, the initial stack pointer the recompiler seeds —
    /// `rsp_recomp.cpp` `r1 = 0xFC0;`).
    pub fn new(rdram: &'a mut [u8]) -> Self {
        let rdram_len = rdram.len();
        let mut ctx = RspContext::new();
        ctx.r[1] = 0xFC0;
        RspMachine {
            ctx,
            dmem: Dmem::new(),
            rdram,
            dma_rdram_ranges: std::iter::once(0..rdram_len).collect(),
            sp_status: 0,
            sp_semaphore: false,
            dma_read_length: 0,
            dma_write_length: 0,
            dp_start: 0,
            dp_end: 0,
            dp_current: 0,
            dp_status: 0,
            dp_clock: 0,
            dp_busy: 0,
            dp_pipe_busy: 0,
            dp_tmem_busy: 0,
            dp_submissions: Vec::new(),
            rdram_writes: Vec::new(),
            dma_journal: Vec::new(),
        }
    }

    /// Capture every future-visible non-memory register and pending DPC
    /// submission, excluding diagnostic interpreter accounting.
    pub fn snapshot_architectural_state(&self) -> RspArchitecturalState {
        RspArchitecturalState {
            gprs: self.ctx.r,
            dma_dram_address: self.ctx.dma_dram_address,
            dma_mem_address: self.ctx.dma_mem_address,
            jump_target: self.ctx.jump_target,
            resume_address: self.ctx.resume_address,
            resume_delay: self.ctx.resume_delay,
            vu: self.ctx.rsp.clone(),
            sp_status: self.sp_status,
            sp_semaphore: self.sp_semaphore,
            dma_read_length: self.dma_read_length,
            dma_write_length: self.dma_write_length,
            dp_start: self.dp_start,
            dp_end: self.dp_end,
            dp_current: self.dp_current,
            dp_status: self.dp_status,
            dp_clock: self.dp_clock,
            dp_busy: self.dp_busy,
            dp_pipe_busy: self.dp_pipe_busy,
            dp_tmem_busy: self.dp_tmem_busy,
            dp_submissions: self.dp_submissions.clone(),
        }
    }

    /// Restore future-visible non-memory state while preserving this
    /// interpreter's diagnostic instruction count. RDRAM and its write-effect
    /// journal remain paired and unchanged; callers that need a clean effect
    /// boundary must restore into a freshly constructed lane.
    pub fn restore_architectural_state(&mut self, state: RspArchitecturalState) {
        self.ctx.r = state.gprs;
        self.ctx.dma_dram_address = state.dma_dram_address;
        self.ctx.dma_mem_address = state.dma_mem_address;
        self.ctx.jump_target = state.jump_target;
        self.ctx.resume_address = state.resume_address;
        self.ctx.resume_delay = state.resume_delay;
        self.ctx.rsp = state.vu;
        self.sp_status = state.sp_status;
        self.sp_semaphore = state.sp_semaphore;
        self.dma_read_length = state.dma_read_length;
        self.dma_write_length = state.dma_write_length;
        self.dp_start = state.dp_start;
        self.dp_end = state.dp_end;
        self.dp_current = state.dp_current;
        self.dp_status = state.dp_status;
        self.dp_clock = state.dp_clock;
        self.dp_busy = state.dp_busy;
        self.dp_pipe_busy = state.dp_pipe_busy;
        self.dp_tmem_busy = state.dp_tmem_busy;
        self.dp_submissions = state.dp_submissions;
    }

    /// Capture complete non-memory state, including diagnostic accounting.
    pub fn snapshot_state(&self) -> RspMachineState {
        RspMachineState {
            architectural: self.snapshot_architectural_state(),
            diagnostic_steps: self.ctx.steps,
        }
    }

    /// Restore a snapshot while retaining this machine's checked RDRAM slice,
    /// admitted DMA ranges, and separately imported DMEM image.
    pub fn restore_state(&mut self, state: RspMachineState) {
        let RspMachineState {
            architectural,
            diagnostic_steps,
        } = state;
        self.restore_architectural_state(architectural);
        self.ctx.steps = diagnostic_steps;
    }

    /// Restrict SP DMA to explicitly admitted ranges in the supplied backing
    /// slice. Adjacent and overlapping ranges are merged so a rectangular row
    /// may cross a boundary only when the union is continuous.
    pub fn set_dma_rdram_ranges(&mut self, mut ranges: Vec<Range<usize>>) {
        assert!(
            !ranges.is_empty(),
            "RSP DMA requires at least one admitted RDRAM range"
        );
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            assert!(
                range.start < range.end && range.end <= self.rdram.len(),
                "RSP DMA admitted range {range:?} is invalid for backing length {:#x}",
                self.rdram.len()
            );
            if let Some(last) = merged.last_mut() {
                if range.start <= last.end {
                    last.end = last.end.max(range.end);
                    continue;
                }
            }
            merged.push(range);
        }
        self.dma_rdram_ranges = merged;
    }

    /// Replace DMEM from an architectural, big-endian byte image.
    pub fn load_dmem_logical(&mut self, bytes: &[u8; DMEM_SIZE]) {
        for (address, byte) in bytes.iter().copied().enumerate() {
            self.dmem.write_bu(address as u32, byte);
        }
    }

    /// Export DMEM in architectural byte-address order.
    pub fn dmem_logical(&self) -> [u8; DMEM_SIZE] {
        core::array::from_fn(|address| self.dmem.read_bu(address as u32))
    }

    /// Install the persistent SP status image used when this interpreter run
    /// begins. BUSY/FULL remain owned by the outer device fabric.
    pub fn set_sp_status_raw(&mut self, status: u32) {
        self.sp_status = status;
    }

    /// Overlay the registers owned authoritatively by the outer device
    /// fabric while retaining scalar, vector, and continuation state.
    ///
    /// CPU MMIO and `osSpTaskLoad` may change these latches between two
    /// interpreter entries. Restoring their stale duplicate from the prior
    /// task would make the next CP0 read depend on which execution lane ran
    /// last. Pending interpreter-produced DPC submissions must be drained by
    /// the prior commit before the fabric image can replace their register
    /// context.
    pub fn overlay_device_execution_state(&mut self, state: fn64_runtime::RspExecutionState) {
        assert!(
            self.dp_submissions.is_empty(),
            "cannot overlay device-owned RSP registers with queued interpreter DPC submissions"
        );
        self.ctx.dma_mem_address = u32::from(state.sp_dma_mem_addr.get());
        self.ctx.dma_dram_address = state.sp_dma_dram_addr.offset();
        self.sp_status = state.sp_status;
        self.sp_semaphore = state.sp_semaphore;
        self.dma_read_length = state.sp_dma_read_length;
        self.dma_write_length = state.sp_dma_write_length;
        self.dp_start = state.dpc_start;
        self.dp_end = state.dpc_end;
        self.dp_current = state.dpc_current;
        self.dp_status = state.dpc_status;
        self.dp_clock = state.dpc_clock;
        self.dp_busy = state.dpc_busy;
        self.dp_pipe_busy = state.dpc_pipe_busy;
        self.dp_tmem_busy = state.dpc_tmem_busy;
    }

    pub const fn sp_status(&self) -> u32 {
        self.sp_status
    }

    /// Inspect the semaphore without performing CP0's architectural
    /// read-and-set. This accessor is diagnostic-only.
    pub const fn sp_semaphore_latch(&self) -> bool {
        self.sp_semaphore
    }

    /// Drain the DPC ranges produced since the last call.
    pub fn take_dp_submissions(&mut self) -> Vec<RspDpSubmission> {
        core::mem::take(&mut self.dp_submissions)
    }

    // -- Scalar register access (r0 hardwired zero) --

    /// Read scalar reg `n` (`r0` reads as 0).
    #[inline]
    pub fn reg(&self, n: u8) -> u32 {
        if n == 0 {
            0
        } else {
            self.ctx.r[n as usize]
        }
    }

    /// Write scalar reg `n` (writes to `r0` are ignored, modeling `$zero`).
    #[inline]
    pub fn set_reg(&mut self, n: u8, v: u32) {
        if n != 0 {
            self.ctx.r[n as usize] = v;
        }
    }

    // -- Scalar DMEM loads/stores (delegating to the swizzled accessors) --

    #[inline]
    pub fn load_b(&self, addr: u32) -> u32 {
        self.dmem.read_b(addr) as i32 as u32
    }
    #[inline]
    pub fn load_bu(&self, addr: u32) -> u32 {
        self.dmem.read_bu(addr) as u32
    }
    #[inline]
    pub fn load_h(&self, addr: u32) -> u32 {
        self.dmem.read_h(addr) as i32 as u32
    }
    #[inline]
    pub fn load_hu(&self, addr: u32) -> u32 {
        self.dmem.read_hu(addr) as u32
    }
    #[inline]
    pub fn load_w(&self, addr: u32) -> u32 {
        self.dmem.read_w(addr) as u32
    }
    #[inline]
    pub fn store_b(&mut self, addr: u32, v: u32) {
        self.dmem.write_b(addr, v as u8 as i8);
    }
    #[inline]
    pub fn store_h(&mut self, addr: u32, v: u32) {
        self.dmem.write_h(addr, v as u16 as i16);
    }
    #[inline]
    pub fn store_w(&mut self, addr: u32, v: u32) {
        self.dmem.write_w(addr, v as i32);
    }

    // -- CP2 scalar transfers --

    /// `mfc2 rt, vs[e]`: read the big-endian byte pair at byte-element `e`
    /// of vector `vs`, sign-extended into a 32-bit scalar. Element `e` is a
    /// BYTE index into the 16-byte register; the two bytes at `e` and `e+1`
    /// (wrapping) form the halfword (n64brew MFC2).
    pub fn mfc2(&self, vs: u8, e: u8) -> u32 {
        let bytes = vec_to_bytes(&self.ctx.rsp.regs.r[vs as usize]);
        let hi = bytes[(e as usize) & 0xF];
        let lo = bytes[(e as usize + 1) & 0xF];
        let h = ((hi as u16) << 8) | lo as u16;
        h as i16 as i32 as u32
    }

    /// `mtc2 rt, vs[e]`: write the low 16 bits of `val` into the byte pair at
    /// byte-element `e` of vector `vs` (n64brew MTC2).
    pub fn mtc2(&mut self, vs: u8, e: u8, val: u32) {
        let mut bytes = vec_to_bytes(&self.ctx.rsp.regs.r[vs as usize]);
        let hi = (val >> 8) as u8;
        let lo = val as u8;
        bytes[(e as usize) & 0xF] = hi;
        bytes[(e as usize + 1) & 0xF] = lo;
        self.ctx.rsp.regs.r[vs as usize] = bytes_to_vec(&bytes);
    }

    /// `cfc2 rt, cd`: read a VU control register (0=VCO, 1=VCC, 2=VCE),
    /// sign-extended to 32 bits (the control regs read as signed 16-bit).
    pub fn cfc2(&self, cd: u8) -> u32 {
        let v: u16 = match cd {
            0 => self.ctx.rsp.flags.vco,
            1 => self.ctx.rsp.flags.vcc,
            2 => self.ctx.rsp.flags.vce as u16,
            _ => panic!("invalid RSP CFC2 control register c{cd}"),
        };
        v as i16 as i32 as u32
    }

    /// `ctc2 rt, cd`: write a VU control register from a scalar reg.
    pub fn ctc2(&mut self, cd: u8, val: u32) {
        match cd {
            0 => self.ctx.rsp.flags.vco = val as u16,
            1 => self.ctx.rsp.flags.vcc = val as u16,
            2 => self.ctx.rsp.flags.vce = val as u8,
            _ => panic!("invalid RSP CTC2 control register c{cd}"),
        }
    }

    // -- CP0 control registers (Programmer's Guide, Table 4-1) --

    /// Read one of the 16 RSP-view CP0 registers. Reading the semaphore
    /// returns its previous value and atomically sets it, as on hardware.
    pub fn read_cp0(&mut self, reg: u8) -> u32 {
        match reg {
            0 => self.ctx.dma_mem_address,
            1 => self.ctx.dma_dram_address,
            2 => self.dma_read_length,
            3 => self.dma_write_length,
            4 => self.sp_status,
            5 | 6 => 0, // synchronous DMA: FULL and BUSY are clear at boundaries
            7 => {
                let previous = u32::from(self.sp_semaphore);
                self.sp_semaphore = true;
                previous
            }
            8 => self.dp_start,
            9 => self.dp_end,
            10 => self.dp_current,
            11 => self.dp_status,
            12 => self.dp_clock,
            13 => self.dp_busy,
            14 => self.dp_pipe_busy,
            15 => self.dp_tmem_busy,
            _ => panic!("invalid RSP MFC0 register c{reg}"),
        }
    }

    /// Write one RSP-view CP0 register. A read-DMA into IMEM returns
    /// `SwapOverlay`; all other writes complete synchronously.
    pub fn write_cp0(&mut self, reg: u8, value: u32) -> Option<RspExitReason> {
        if !super::content_safe_diagnostics() && std::env::var_os("RSP_TRACE_CP0").is_some() {
            eprintln!("[fn64-rsp-cp0] write c{reg}={value:#010x}");
        }
        match reg {
            0 => self.set_dma_mem(value),
            1 => self.set_dma_dram(value),
            2 => {
                self.dma_read_length = value;
                return self.dma_read(value);
            }
            3 => {
                self.dma_write_length = value;
                self.dma_write(value);
            }
            4 => self.write_sp_status(value),
            5 | 6 | 10 | 12 | 13 | 14 | 15 => {
                // Architecturally read-only registers ignore writes.
            }
            7 => self.sp_semaphore = false,
            8 => {
                self.dp_start = value & 0x00FF_FFF8;
                // The synchronous model is idle at every instruction
                // boundary, so a newly accepted START also becomes CURRENT.
                self.dp_current = self.dp_start;
            }
            9 => {
                self.dp_end = value & 0x00FF_FFF8;
                if self.dp_current != self.dp_end {
                    let start = self.dp_current;
                    let end = self.dp_end;
                    assert!(
                        start < end && start.is_multiple_of(8) && end.is_multiple_of(8),
                        "RSP DPC range [{start:#010x}, {end:#010x}) must be ordered and 8-byte aligned"
                    );
                    let xbus = self.dp_status & 1 != 0;
                    // The renderer runs after the RSP reaches BREAK, while
                    // hardware begins fetching at CMD_END. Capture here to
                    // close the interleaving where a later SP DMA or an
                    // earlier framebuffer write aliases and overwrites this
                    // command range before deferred host rendering begins.
                    let payload = if xbus {
                        (start..end)
                            .map(|address| self.dmem.read_bu(address & 0x0fff))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let words = if xbus {
                        let start = (start & 0x0fff) as usize;
                        let end = (end & 0x0fff) as usize;
                        assert!(
                            start < end && end <= DMEM_SIZE,
                            "RSP XBUS DPC range [{start:#05x}, {end:#05x}) exceeds DMEM"
                        );
                        (start..end)
                            .step_by(4)
                            .map(|offset| self.dmem.read_w(offset as u32) as u32)
                            .collect()
                    } else {
                        assert!(
                            end as usize <= self.rdram.len(),
                            "RSP DPC range end {end:#010x} exceeds RDRAM length {:#x}",
                            self.rdram.len()
                        );
                        (start..end)
                            .step_by(4)
                            .map(|address| {
                                u32::from_ne_bytes(
                                    self.rdram[address as usize..address as usize + 4]
                                        .try_into()
                                        .expect("four RDRAM command bytes"),
                                )
                            })
                            .collect()
                    };
                    self.dp_submissions.push(RspDpSubmission {
                        start,
                        end,
                        xbus,
                        payload,
                        words,
                    });
                }
                // No RDP execution engine lives in fn64-audio. Model the
                // command DMA as instant so polling loops observe idle.
                self.dp_current = self.dp_end;
            }
            11 => self.write_dp_status(value),
            _ => panic!("invalid RSP MTC0 register c{reg}"),
        }
        None
    }

    /// BREAK sets HALT and BROKE (Programmer's Guide, BREAK and Table 4-2).
    pub fn break_rsp(&mut self) {
        self.sp_status |= 0x3;
    }

    fn write_sp_status(&mut self, command: u32) {
        set_clear_pair(&mut self.sp_status, command, 0, 1, 0); // HALT
        if command & (1 << 2) != 0 {
            self.sp_status &= !(1 << 1); // clear BROKE; no set command exists
        }
        set_clear_pair(&mut self.sp_status, command, 5, 6, 5); // single step
        set_clear_pair(&mut self.sp_status, command, 7, 8, 6); // interrupt on break
        for signal in 0..8 {
            let clear = 9 + signal * 2;
            set_clear_pair(&mut self.sp_status, command, clear, clear + 1, 7 + signal);
        }
    }

    fn write_dp_status(&mut self, command: u32) {
        // RDP status write commands (Programmer's Guide, Table 4-5).
        set_clear_pair(&mut self.dp_status, command, 0, 1, 0); // XBUS DMEM DMA
        set_clear_pair(&mut self.dp_status, command, 2, 3, 1); // freeze
        set_clear_pair(&mut self.dp_status, command, 4, 5, 2); // flush
        if command & (1 << 6) != 0 {
            self.dp_tmem_busy = 0;
        }
        if command & (1 << 7) != 0 {
            self.dp_pipe_busy = 0;
        }
        if command & (1 << 8) != 0 {
            self.dp_busy = 0;
        }
        if command & (1 << 9) != 0 {
            self.dp_clock = 0;
        }
    }

    // -- CP2 vector loads --

    /// Execute a vector load into register `vt`. `base_val` is the value of
    /// the base scalar reg; `off` is the raw (unscaled) element offset from
    /// the instruction. The DMEM address is `base_val + off*scale` per op.
    pub fn vload(&mut self, op: VLoadOp, vt: u8, e: u8, base_val: u32, off: i16) {
        let mut bytes = vec_to_bytes(&self.ctx.rsp.regs.r[vt as usize]);
        let e = e as usize & 0xF;
        match op {
            // Elementwise byte-count loads: fill `n` bytes starting at element
            // `e`, from DMEM starting at addr. Address = base + off*n.
            VLoadOp::Lbv => self.load_bytes(&mut bytes, e, 1, base_val, off, 1),
            VLoadOp::Lsv => self.load_bytes(&mut bytes, e, 2, base_val, off, 2),
            VLoadOp::Llv => self.load_bytes(&mut bytes, e, 4, base_val, off, 4),
            VLoadOp::Ldv => self.load_bytes(&mut bytes, e, 8, base_val, off, 8),
            VLoadOp::Lqv => {
                // Load up to the end of the current 16-byte DMEM row into the
                // register starting at element e.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let end = (addr & !0xF).wrapping_add(16);
                let mut a = addr;
                let mut idx = e;
                while a < end && idx < 16 {
                    bytes[idx] = self.dmem.read_bu(a);
                    a = a.wrapping_add(1);
                    idx += 1;
                }
            }
            VLoadOp::Lrv => {
                // Load the bytes from the row start up to the addressed byte
                // into the tail of the register.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let start = addr & !0xF;
                let count = (addr - start) as usize;
                let mut a = start;
                let first = 16i32 - count as i32 + e as i32;
                for byte in bytes.iter_mut().skip(first.max(0) as usize) {
                    *byte = self.dmem.read_bu(a);
                    a = a.wrapping_add(1);
                }
            }
            VLoadOp::Lpv | VLoadOp::Luv => {
                // Packed load: 8 bytes -> 8 lanes, each byte placed in the
                // high (LPV) or << 7 (LUV) part of the lane.
                let addr = base_val.wrapping_add((off as i32 * 8) as u32);
                let mut lanes = [0i16; LANES];
                for (i, lane) in lanes.iter_mut().enumerate() {
                    let source = ((16 - e + i) & 0xF) as u32;
                    let b = self.dmem.read_bu(addr.wrapping_add(source)) as i16;
                    *lane = match op {
                        VLoadOp::Lpv => b << 8,
                        _ => b << 7, // LUV
                    };
                }
                self.ctx.rsp.regs.r[vt as usize] = lanes;
                return;
            }
            VLoadOp::Lhv => {
                // Half load: 8 bytes at stride 2 -> lanes << 7.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let mut lanes = [0i16; LANES];
                for (i, lane) in lanes.iter_mut().enumerate() {
                    let source = ((16 - e + i * 2) & 0xF) as u32;
                    let b = self.dmem.read_bu(addr.wrapping_add(source)) as i16;
                    *lane = b << 7;
                }
                self.ctx.rsp.regs.r[vt as usize] = lanes;
                return;
            }
            VLoadOp::Lfv => {
                // Fourth load: partial packed load (elements 0..4 filled).
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let mut lanes = self.ctx.rsp.regs.r[vt as usize];
                for i in 0..4usize {
                    let lane = ((e >> 1) + i) & 7;
                    let b = self.dmem.read_bu(addr.wrapping_add((i as u32) * 4)) as i16;
                    lanes[lane] = b << 7;
                }
                self.ctx.rsp.regs.r[vt as usize] = lanes;
                return;
            }
            VLoadOp::Ltv => {
                // SGI Guide pp. 54-55 / MAME `LTV`: load at most eight
                // consecutive registers, rotating the destination element.
                // The +8 alignment rule is observable when the effective
                // address is in the upper half of a 16-byte row.
                let effective = base_val.wrapping_add((off as i32 * 16) as u32);
                let mut addr = effective.wrapping_add(8) & !0xF;
                for reg in vt as usize..(vt as usize + LANES).min(32) {
                    let element = ((8usize.wrapping_sub(e >> 1) + reg - vt as usize) << 1) & 0xF;
                    let mut reg_bytes = vec_to_bytes(&self.ctx.rsp.regs.r[reg]);
                    reg_bytes[element] = self.dmem.read_bu(addr);
                    reg_bytes[(element + 1) & 0xF] = self.dmem.read_bu(addr.wrapping_add(1));
                    self.ctx.rsp.regs.r[reg] = bytes_to_vec(&reg_bytes);
                    addr = addr.wrapping_add(2);
                }
                return;
            }
        }
        self.ctx.rsp.regs.r[vt as usize] = bytes_to_vec(&bytes);
    }

    /// Helper for the elementwise byte-count loads (LBV/LSV/LLV/LDV): copy
    /// `n` bytes from DMEM `base+off*scale` into the register's byte image
    /// starting at byte-element `e` (wrapping within the 16-byte register).
    fn load_bytes(
        &self,
        bytes: &mut [u8; 16],
        e: usize,
        n: usize,
        base_val: u32,
        off: i16,
        scale: i32,
    ) {
        let addr = base_val.wrapping_add((off as i32 * scale) as u32);
        for i in 0..n {
            bytes[(e + i) & 0xF] = self.dmem.read_bu(addr.wrapping_add(i as u32));
        }
    }

    // -- CP2 vector stores --

    /// Execute a vector store from register `vt`.
    pub fn vstore(&mut self, op: VStoreOp, vt: u8, e: u8, base_val: u32, off: i16) {
        let reg = self.ctx.rsp.regs.r[vt as usize];
        let bytes = vec_to_bytes(&reg);
        let e = e as usize & 0xF;
        match op {
            VStoreOp::Sbv => self.store_bytes(&bytes, e, 1, base_val, off, 1),
            VStoreOp::Ssv => self.store_bytes(&bytes, e, 2, base_val, off, 2),
            VStoreOp::Slv => self.store_bytes(&bytes, e, 4, base_val, off, 4),
            VStoreOp::Sdv => self.store_bytes(&bytes, e, 8, base_val, off, 8),
            VStoreOp::Sqv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let end = (addr & !0xF).wrapping_add(16);
                let mut a = addr;
                let mut idx = e;
                while a < end {
                    self.dmem.write_bu(a, bytes[idx & 0xF]);
                    a = a.wrapping_add(1);
                    idx += 1;
                }
            }
            VStoreOp::Srv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let start = addr & !0xF;
                let count = (addr - start) as usize;
                let first = (16 - count + e) & 0xF;
                for (i, idx) in (first..).take(count).enumerate() {
                    self.dmem
                        .write_bu(start.wrapping_add(i as u32), bytes[idx & 0xF]);
                }
            }
            VStoreOp::Spv | VStoreOp::Suv => {
                // Packed store: 8 lanes -> 8 bytes.
                let addr = base_val.wrapping_add((off as i32 * 8) as u32);
                for i in 0..LANES {
                    let index = (e + i) & 0xF;
                    let lane = reg[index & 7] as u16;
                    let b = match (op, index < 8) {
                        (VStoreOp::Spv, true) | (VStoreOp::Suv, false) => (lane >> 8) as u8,
                        _ => (lane >> 7) as u8,
                    };
                    self.dmem.write_bu(addr.wrapping_add(i as u32), b);
                }
            }
            VStoreOp::Shv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                for i in 0..LANES {
                    let hi = bytes[(e + i * 2) & 0xF];
                    let lo = bytes[(e + i * 2 + 1) & 0xF];
                    let b = (hi << 1) | (lo >> 7);
                    self.dmem.write_bu(addr.wrapping_add((i as u32) * 2), b);
                }
            }
            VStoreOp::Sfv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let row = addr & !0xF;
                let mut row_offset = addr & 0xF;
                for i in 0..4usize {
                    let lane = ((e >> 1) + i) & 7;
                    let b = (reg[lane] as u16 >> 7) as u8;
                    self.dmem.write_bu(row + (row_offset & 0xF), b);
                    row_offset += 4;
                }
            }
            VStoreOp::Swv => {
                // Wrapped store remains within the addressed aligned DMEM row.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let row = addr & !0xF;
                let row_offset = addr & 0xF;
                for i in 0..16usize {
                    self.dmem
                        .write_bu(row + ((row_offset + i as u32) & 0xF), bytes[(e + i) & 0xF]);
                }
            }
            VStoreOp::Stv => {
                // SGI Guide pp. 54-55 / MAME `STV`: walk consecutive vector
                // registers while rotating both the source element and byte
                // position within the addressed 16-byte row.
                let effective = base_val.wrapping_add((off as i32 * 16) as u32);
                let row = effective & !0xF;
                let first_element = 8usize.wrapping_sub(e >> 1);
                let mut row_offset = (effective & 0xF).wrapping_add((first_element * 2) as u32);
                for (offset, reg_idx) in (vt as usize..(vt as usize + LANES).min(32)).enumerate() {
                    let element = first_element + offset;
                    let value = self.ctx.rsp.regs.r[reg_idx][element & 7] as u16;
                    self.dmem
                        .write_bu(row + (row_offset & 0xF), (value >> 8) as u8);
                    self.dmem
                        .write_bu(row + ((row_offset + 1) & 0xF), value as u8);
                    row_offset += 2;
                }
            }
        }
    }

    fn store_bytes(
        &mut self,
        bytes: &[u8; 16],
        e: usize,
        n: usize,
        base_val: u32,
        off: i16,
        scale: i32,
    ) {
        let addr = base_val.wrapping_add((off as i32 * scale) as u32);
        for i in 0..n {
            self.dmem
                .write_bu(addr.wrapping_add(i as u32), bytes[(e + i) & 0xF]);
        }
    }

    // -- DMA engine (SET_DMA_* / DO_DMA_*) --

    /// `SET_DMA_DRAM` — set the DRAM (rdram) DMA address.
    #[inline]
    pub fn set_dma_dram(&mut self, addr: u32) {
        self.ctx.dma_dram_address = addr;
    }
    /// `SET_DMA_MEM` — set the RSP MEM (DMEM/IMEM) DMA address.
    #[inline]
    pub fn set_dma_mem(&mut self, addr: u32) {
        self.ctx.dma_mem_address = addr;
    }

    /// `DO_DMA_READ` (`SP_RD_LEN` write): copy `len+1` bytes from rdram at
    /// `dma_dram_address` into DMEM at `dma_mem_address & 0xFFF`. If the MEM
    /// address targets IMEM (`& 0x1000`), this is an overlay swap the caller
    /// must resolve — returns `Some(RspExitReason::SwapOverlay)`; the recomp
    /// loop propagates it. Otherwise returns `None`.
    ///
    /// `len` is the raw length/count/skip encoding. The hardware forces the
    /// low three address bits to zero and the low three length bits to one.
    pub fn dma_read(&mut self, len: u32) -> Option<RspExitReason> {
        self.dma_read_length = len;
        let (line_len, lines, skip) = decode_dma_length(len);
        let mut dram = self.ctx.dma_dram_address as usize & 0x00FF_FFF8;
        let mut mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 8);
        assert_dma_rdram_range(
            "read",
            self.rdram.len(),
            &self.dma_rdram_ranges,
            dram,
            DmaTransfer {
                descriptor: len,
                line_len,
                lines,
                skip,
            },
        );
        self.dma_journal.push(RspDmaJournalEntry {
            direction: RspDmaDirection::Read,
            effective_dram_address: dram as u32,
            sp_mem_address: self.ctx.dma_mem_address & 0x1ff8,
            raw_length_descriptor: len,
        });
        trace_dma(DmaTrace {
            direction: if self.ctx.dma_mem_address & 0x1000 != 0 {
                "read-imem"
            } else {
                "read-dmem"
            },
            dram,
            mem,
            descriptor: len,
            line_len,
            lines,
            skip,
            checksum: checksum_rdram(self.rdram, dram, line_len, lines, skip),
        });
        trace_dma_words(self.rdram, dram, line_len, lines, skip);
        if self.ctx.dma_mem_address & 0x1000 != 0 {
            return Some(RspExitReason::SwapOverlay);
        }
        for _ in 0..lines {
            for i in 0..line_len {
                self.dmem.as_bytes_mut()[(mem + i) & (DMEM_SIZE - 1)] = self.rdram[dram + i];
            }
            dram = dram.wrapping_add(line_len + skip);
            mem = (mem + line_len) & (DMEM_SIZE - 1);
        }
        self.ctx.dma_dram_address = dram as u32;
        self.ctx.dma_mem_address = mem as u32;
        None
    }

    /// Describe the IMEM destination of the read-DMA which returned
    /// [`RspExitReason::SwapOverlay`]. The caller uses this before completing
    /// the DMA to identify the first instruction belonging to the new ucode.
    pub fn pending_imem_dma_span(&self) -> ImemDmaSpan {
        assert!(
            self.ctx.dma_mem_address & 0x1000 != 0,
            "pending_imem_dma_span called without an IMEM destination"
        );
        let (line_len, lines, _) = decode_dma_length(self.dma_read_length);
        ImemDmaSpan {
            start: self.ctx.dma_mem_address as usize & (DMEM_SIZE - 8),
            byte_len: line_len
                .checked_mul(lines)
                .expect("IMEM DMA destination length overflow"),
        }
    }

    /// Complete the IMEM DMA which caused [`RspExitReason::SwapOverlay`].
    ///
    /// The interpreter pauses before replacing executable memory. The outer
    /// runtime supplies its persistent architectural IMEM image here; this
    /// method applies the pending rectangular DMA in the native-word backing
    /// layout shared by RDRAM and RSP memory, then converts it back to logical
    /// byte order and advances the CP0 DMA address registers.
    pub fn complete_imem_dma(&mut self, imem: &mut [u8; DMEM_SIZE]) {
        assert!(
            self.ctx.dma_mem_address & 0x1000 != 0,
            "complete_imem_dma called without an IMEM destination"
        );
        let (line_len, lines, skip) = decode_dma_length(self.dma_read_length);
        let mut dram = self.ctx.dma_dram_address as usize & 0x00FF_FFF8;
        let mut mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 8);
        assert_dma_rdram_range(
            "read-imem completion",
            self.rdram.len(),
            &self.dma_rdram_ranges,
            dram,
            DmaTransfer {
                descriptor: self.dma_read_length,
                line_len,
                lines,
                skip,
            },
        );
        let mut backing = [0u8; DMEM_SIZE];
        for logical in 0..DMEM_SIZE {
            backing[logical ^ 3] = imem[logical];
        }
        for _ in 0..lines {
            for i in 0..line_len {
                backing[(mem + i) & (DMEM_SIZE - 1)] = self.rdram[dram + i];
            }
            dram = dram.wrapping_add(line_len + skip);
            mem = (mem + line_len) & (DMEM_SIZE - 1);
        }
        for logical in 0..DMEM_SIZE {
            imem[logical] = backing[logical ^ 3];
        }
        self.ctx.dma_dram_address = dram as u32;
        self.ctx.dma_mem_address = 0x1000 | mem as u32;
    }

    /// `DO_DMA_WRITE` (`SP_WR_LEN` write): copy `len+1` bytes from DMEM at
    /// `dma_mem_address & 0xFFF` back to rdram at `dma_dram_address`.
    pub fn dma_write(&mut self, len: u32) {
        let (line_len, lines, skip) = decode_dma_length(len);
        let mut dram = self.ctx.dma_dram_address as usize & 0x00FF_FFF8;
        let mut mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 8);
        assert_dma_rdram_range(
            "write",
            self.rdram.len(),
            &self.dma_rdram_ranges,
            dram,
            DmaTransfer {
                descriptor: len,
                line_len,
                lines,
                skip,
            },
        );
        self.dma_journal.push(RspDmaJournalEntry {
            direction: RspDmaDirection::Write,
            effective_dram_address: dram as u32,
            sp_mem_address: self.ctx.dma_mem_address & 0x1ff8,
            raw_length_descriptor: len,
        });
        trace_dma(DmaTrace {
            direction: "write",
            dram,
            mem,
            descriptor: len,
            line_len,
            lines,
            skip,
            checksum: checksum_dmem(&self.dmem, mem, line_len, lines),
        });
        for _ in 0..lines {
            for i in 0..line_len {
                self.rdram[dram + i] = self.dmem.as_bytes()[(mem + i) & (DMEM_SIZE - 1)];
            }
            self.record_rdram_write(dram, line_len);
            dram = dram.wrapping_add(line_len + skip);
            mem = (mem + line_len) & (DMEM_SIZE - 1);
        }
        self.ctx.dma_dram_address = dram as u32;
        self.ctx.dma_mem_address = mem as u32;
    }

    fn record_rdram_write(&mut self, start: usize, len: usize) {
        let mut merged_start = start;
        let mut merged_end = start
            .checked_add(len)
            .expect("validated RSP DMA write span overflow");

        // DMA descriptors may move backward or revisit an earlier range. Keep
        // the effect journal canonical here so later JIT invalidation and
        // speculative outcome extraction cannot lose a non-monotonic write.
        let first = self
            .rdram_writes
            .partition_point(|&(_, existing_end)| existing_end < merged_start);
        let mut after = first;
        while let Some(&(existing_start, existing_end)) = self.rdram_writes.get(after) {
            if existing_start > merged_end {
                break;
            }
            merged_start = merged_start.min(existing_start);
            merged_end = merged_end.max(existing_end);
            after += 1;
        }
        self.rdram_writes
            .splice(first..after, [(merged_start, merged_end)]);
    }

    /// Drain the sorted, disjoint RDRAM write coverage produced by SP DMA.
    pub fn take_rdram_writes(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.rdram_writes)
    }

    /// Drain diagnostic observations without changing architectural state.
    pub fn take_dma_journal(&mut self) -> Vec<RspDmaJournalEntry> {
        std::mem::take(&mut self.dma_journal)
    }

    /// Convenience accessor for the VU state the compute-op dispatcher wants.
    #[inline]
    pub fn vu(&mut self) -> &mut VuState {
        &mut self.ctx.rsp
    }
}

fn set_clear_pair(state: &mut u32, command: u32, clear_cmd: u32, set_cmd: u32, bit: u32) {
    if command & (1 << clear_cmd) != 0 {
        *state &= !(1 << bit);
    }
    if command & (1 << set_cmd) != 0 {
        *state |= 1 << bit;
    }
}

fn decode_dma_length(value: u32) -> (usize, usize, usize) {
    let line_len = (((value & 0x0FFF) | 7) + 1) as usize;
    let lines = (((value >> 12) & 0xFF) + 1) as usize;
    let skip = ((value >> 20) & 0x0FFF) as usize;
    (line_len, lines, skip)
}

#[derive(Clone, Copy)]
struct DmaTransfer {
    descriptor: u32,
    line_len: usize,
    lines: usize,
    skip: usize,
}

fn assert_dma_rdram_range(
    direction: &str,
    rdram_len: usize,
    admitted_ranges: &[Range<usize>],
    dram: usize,
    transfer: DmaTransfer,
) {
    let DmaTransfer {
        descriptor,
        line_len,
        lines,
        skip,
    } = transfer;
    let stride = line_len
        .checked_add(skip)
        .expect("RSP DMA line stride overflow");
    for line in 0..lines {
        let line_start = line
            .checked_mul(stride)
            .and_then(|offset| dram.checked_add(offset))
            .unwrap_or_else(|| {
                panic!(
                    "RSP DMA {direction} address overflow: DRAM {dram:#08x}, descriptor \
                     {descriptor:#010x}, line_len {line_len}, lines {lines}, skip {skip}"
                )
            });
        let line_end = line_start.checked_add(line_len).unwrap_or_else(|| {
            panic!(
                "RSP DMA {direction} range overflow: DRAM {dram:#08x}, descriptor \
                 {descriptor:#010x}, line_len {line_len}, lines {lines}, skip {skip}"
            )
        });
        assert!(
            line_end <= rdram_len,
            "RSP DMA validator produced range [{line_start:#08x}, {line_end:#08x}) beyond backing \
             length {rdram_len:#x}"
        );
        assert!(
            admitted_ranges
                .iter()
                .any(|range| line_start >= range.start && line_end <= range.end),
            "RSP DMA {direction} line {line} range [{line_start:#08x}, {line_end:#08x}) is outside \
             admitted RDRAM ranges {admitted_ranges:?} (backing length {rdram_len:#x}): descriptor \
             {descriptor:#010x}, line_len {line_len}, lines {lines}, skip {skip}"
        );
    }
}

struct DmaTrace {
    direction: &'static str,
    dram: usize,
    mem: usize,
    descriptor: u32,
    line_len: usize,
    lines: usize,
    skip: usize,
    checksum: u64,
}

fn trace_dma(trace: DmaTrace) {
    if super::content_safe_diagnostics() || std::env::var_os("RSP_TRACE_DMA").is_none() {
        return;
    }
    let seq = DMA_TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
    let limit = std::env::var("RSP_TRACE_DMA_LIMIT")
        .ok()
        .map(|raw| {
            raw.parse::<u64>()
                .unwrap_or_else(|_| panic!("RSP_TRACE_DMA_LIMIT must be an integer, got {raw:?}"))
        })
        .unwrap_or(u64::MAX);
    if seq >= limit {
        return;
    }
    let DmaTrace {
        direction,
        dram,
        mem,
        descriptor,
        line_len,
        lines,
        skip,
        checksum,
    } = trace;
    eprintln!(
        "[fn64-audio/rsp] dma#{} {direction} dram=0x{dram:06x} mem=0x{mem:03x} desc=0x{descriptor:08x} line_len={line_len} lines={lines} skip={skip} checksum=0x{checksum:016x}",
        seq + 1
    );
}

fn trace_dma_words(rdram: &[u8], mut dram: usize, line_len: usize, lines: usize, skip: usize) {
    if super::content_safe_diagnostics() {
        return;
    }
    let Some(limit) = std::env::var("RSP_TRACE_DMA_WORDS").ok().map(|raw| {
        raw.parse::<usize>()
            .unwrap_or_else(|_| panic!("RSP_TRACE_DMA_WORDS must be an integer, got {raw:?}"))
    }) else {
        return;
    };
    let mut words = Vec::new();
    for _ in 0..lines {
        for bytes in rdram[dram..dram + line_len].chunks(4) {
            let mut native = [0u8; 4];
            native[..bytes.len()].copy_from_slice(bytes);
            words.push(u32::from_ne_bytes(native));
            if words.len() == limit {
                break;
            }
        }
        if words.len() == limit {
            break;
        }
        dram += line_len + skip;
    }
    eprintln!("[fn64-audio/rsp] dma source words={words:08x?}");
}

fn checksum_rdram(
    rdram: &[u8],
    mut dram: usize,
    line_len: usize,
    lines: usize,
    skip: usize,
) -> u64 {
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..lines {
        for i in 0..line_len {
            checksum ^= u64::from(rdram.get(dram + i).copied().unwrap_or(0));
            checksum = checksum.wrapping_mul(0x100_0000_01b3);
        }
        dram = dram.wrapping_add(line_len + skip);
    }
    checksum
}

fn checksum_dmem(dmem: &Dmem, mut mem: usize, line_len: usize, lines: usize) -> u64 {
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..lines {
        for i in 0..line_len {
            checksum ^= u64::from(dmem.as_bytes()[(mem + i) & (DMEM_SIZE - 1)]);
            checksum = checksum.wrapping_mul(0x100_0000_01b3);
        }
        mem = (mem + line_len) & (DMEM_SIZE - 1);
    }
    checksum
}

/// Convert an 8-lane big-endian vector register into its 16-byte image (lane
/// 0 is the high halfword → bytes 0,1; lane 1 → bytes 2,3; …). This is the
/// ONE place the lane↔byte mapping is defined, so loads/stores and MFC2 all
/// agree.
#[inline]
fn vec_to_bytes(v: &Vec8) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (i, &lane) in v.iter().enumerate() {
        let u = lane as u16;
        b[i * 2] = (u >> 8) as u8;
        b[i * 2 + 1] = u as u8;
    }
    b
}

/// Inverse of [`vec_to_bytes`].
#[inline]
fn bytes_to_vec(b: &[u8; 16]) -> Vec8 {
    let mut v = [0i16; LANES];
    for (i, lane) in v.iter_mut().enumerate() {
        *lane = (((b[i * 2] as u16) << 8) | b[i * 2 + 1] as u16) as i16;
    }
    v
}

#[cfg(test)]
mod tests;
