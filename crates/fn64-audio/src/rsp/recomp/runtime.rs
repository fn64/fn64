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
use std::sync::atomic::{AtomicU64, Ordering};

static DMA_TRACE_SEQ: AtomicU64 = AtomicU64::new(0);

/// One RDP command-DMA range submitted through the RSP's DPC CP0 registers.
/// `xbus` records whether the command source is RSP DMEM rather than RDRAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspDpSubmission {
    pub start: u32,
    pub end: u32,
    pub xbus: bool,
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
    /// Half-open `[start, end)` rdram byte ranges written by `dma_write`
    /// since construction (or the last [`Self::take_rdram_writes`]),
    /// clamped to `rdram.len()` and coalesced when contiguous/overlapping.
    /// The union of these spans covers every rdram byte this machine may
    /// have mutated: `dma_write` is the ONLY rdram store path in this type.
    rdram_writes: Vec<(usize, usize)>,
}

impl<'a> RspMachine<'a> {
    /// A fresh machine bound to `rdram`, with the standard RSP boot state
    /// (`r1 = 0xFC0`, the initial stack pointer the recompiler seeds —
    /// `rsp_recomp.cpp` `r1 = 0xFC0;`).
    pub fn new(rdram: &'a mut [u8]) -> Self {
        let mut ctx = RspContext::new();
        ctx.r[1] = 0xFC0;
        RspMachine {
            ctx,
            dmem: Dmem::new(),
            rdram,
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
        }
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

    pub const fn sp_status(&self) -> u32 {
        self.sp_status
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
                // Writing DPC_START also latches DPC_CURRENT (hardware: the
                // command DMA engine begins the next run at START).
                self.dp_start = value & 0x00FF_FFF8;
                self.dp_current = self.dp_start;
            }
            9 => {
                // Writing DPC_END runs the RDP from CURRENT to END -- NOT
                // from START: a ucode that grows one command run with
                // repeated END writes (F3DEX xbus 2.08 does exactly this
                // against its DMEM ring) must submit each new tail once,
                // never re-submit from START. `END == CURRENT` (how the xbus
                // ucode opens its stream before any command exists) runs
                // zero commands and is not a submission at all.
                self.dp_end = value & 0x00FF_FFF8;
                if self.dp_end != self.dp_current {
                    self.dp_submissions.push(RspDpSubmission {
                        start: self.dp_current,
                        end: self.dp_end,
                        xbus: self.dp_status & 1 != 0,
                    });
                    // No RDP execution engine lives in fn64-audio. Model the
                    // command DMA as instant so polling loops observe idle.
                    self.dp_current = self.dp_end;
                }
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
        if self.ctx.dma_mem_address & 0x1000 != 0 {
            return Some(RspExitReason::SwapOverlay);
        }
        let (line_len, lines, skip) = decode_dma_length(len);
        let mut dram = self.ctx.dma_dram_address as usize & 0x00FF_FFF8;
        let mut mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 8);
        trace_dma(DmaTrace {
            direction: "read",
            dram,
            mem,
            descriptor: len,
            line_len,
            lines,
            skip,
            checksum: checksum_rdram(self.rdram, dram, line_len, lines, skip),
        });
        for _ in 0..lines {
            for i in 0..line_len {
                if let Some(&b) = self.rdram.get(dram + i) {
                    self.dmem.as_bytes_mut()[(mem + i) & (DMEM_SIZE - 1)] = b;
                }
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
        let mut backing = [0u8; DMEM_SIZE];
        for logical in 0..DMEM_SIZE {
            backing[logical ^ 3] = imem[logical];
        }
        for _ in 0..lines {
            for i in 0..line_len {
                if let Some(&byte) = self.rdram.get(dram + i) {
                    backing[(mem + i) & (DMEM_SIZE - 1)] = byte;
                }
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
                if let Some(dst) = self.rdram.get_mut(dram + i) {
                    *dst = self.dmem.as_bytes()[(mem + i) & (DMEM_SIZE - 1)];
                }
            }
            self.record_rdram_write(dram, line_len);
            dram = dram.wrapping_add(line_len + skip);
            mem = (mem + line_len) & (DMEM_SIZE - 1);
        }
        self.ctx.dma_dram_address = dram as u32;
        self.ctx.dma_mem_address = mem as u32;
    }

    /// Record one DMA-write line's rdram byte range, clamped exactly the way
    /// the copy loop above clamps (per-byte `get_mut` beyond `rdram.len()` is
    /// a dropped store). Contiguous/overlapping ranges coalesce with the most
    /// recent entry so consecutive lines (skip = 0) stay one span.
    fn record_rdram_write(&mut self, start: usize, len: usize) {
        let end = start.saturating_add(len).min(self.rdram.len());
        let start = start.min(self.rdram.len());
        if start >= end {
            return;
        }
        if let Some(last) = self.rdram_writes.last_mut() {
            if start <= last.1 && end >= last.0 {
                last.0 = last.0.min(start);
                last.1 = last.1.max(end);
                return;
            }
        }
        self.rdram_writes.push((start, end));
    }

    /// Take the accumulated half-open rdram write spans (see `rdram_writes`'s
    /// field doc). After this call the machine's write log is empty again.
    pub fn take_rdram_writes(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.rdram_writes)
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
    if std::env::var_os("RSP_TRACE_DMA").is_none() {
        return;
    }
    let seq = DMA_TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
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
        "[fn64-audio/rsp] dma#{seq} {direction} dram=0x{dram:06x} mem=0x{mem:03x} desc=0x{descriptor:08x} line_len={line_len} lines={lines} skip={skip} checksum=0x{checksum:016x}"
    );
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
mod tests {
    use super::*;

    #[test]
    fn lqv_sqv_roundtrip_full_quad() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        // Write a known 16-byte pattern into DMEM at 0x00.
        for i in 0..16u32 {
            m.dmem.write_bu(i, (i as u8) + 1);
        }
        // LQV v3, 0(r0): load the whole quad into v3.
        m.vload(VLoadOp::Lqv, 3, 0, 0, 0);
        // Lane 0 = bytes 1,2 -> 0x0102.
        assert_eq!(m.ctx.rsp.regs.r[3][0], 0x0102);
        assert_eq!(m.ctx.rsp.regs.r[3][7], 0x0F10);
        // SQV v3, 0(r0) to a fresh offset (0x20), then read back bytes.
        m.vstore(VStoreOp::Sqv, 3, 0, 0x20, 0);
        for i in 0..16u32 {
            assert_eq!(m.dmem.read_bu(0x20 + i), (i as u8) + 1);
        }
    }

    #[test]
    fn ldv_loads_eight_bytes_at_element() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        for i in 0..8u32 {
            m.dmem.write_bu(0x40 + i, 0xA0 + i as u8);
        }
        // LDV v5[0], 0(base=r0 with base_val 0x40)
        m.vload(VLoadOp::Ldv, 5, 0, 0x40, 0);
        assert_eq!(m.ctx.rsp.regs.r[5][0], 0xA0A1u16 as i16);
        assert_eq!(m.ctx.rsp.regs.r[5][3], 0xA6A7u16 as i16);
        // Upper lanes untouched (still zero).
        assert_eq!(m.ctx.rsp.regs.r[5][4], 0);
    }

    #[test]
    fn mtc2_mfc2_roundtrip_lane() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.mtc2(7, 4, 0x1234); // write element 4 (lane 2) of v7
        assert_eq!(m.ctx.rsp.regs.r[7][2], 0x1234);
        assert_eq!(m.mfc2(7, 4) as u16, 0x1234);
    }

    #[test]
    fn mtc2_and_mfc2_wrap_byte_element_15() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.mtc2(7, 15, 0x1234);
        assert_eq!(m.mfc2(7, 15) as u16, 0x1234);
        let bytes = vec_to_bytes(&m.ctx.rsp.regs.r[7]);
        assert_eq!(bytes[15], 0x12);
        assert_eq!(bytes[0], 0x34);
    }

    #[test]
    fn cfc2_ctc2_roundtrip_vcc() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.ctc2(1, 0x00AB);
        assert_eq!(m.ctc2_read_vcc(), 0x00AB);
        assert_eq!(m.cfc2(1) as u16, 0x00AB);
    }

    #[test]
    fn dma_read_copies_rdram_into_dmem() {
        let mut rdram = vec![0u8; 0x1000];
        for i in 0..64usize {
            rdram[0x200 + i] = i as u8;
        }
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_dram(0x200);
        m.set_dma_mem(0x080);
        let swap = m.dma_read(63); // 63+1 = 64 bytes
        assert!(swap.is_none());
        // DMA copies FLAT bytes into DMEM (no swizzle at the DMA layer —
        // the ^3/^2 swizzle is imposed only by the sub-word RSP_MEM_*
        // accessors). So compare the flat backing store byte-for-byte.
        for i in 0..64usize {
            assert_eq!(m.dmem.as_bytes()[0x080 + i], i as u8);
        }
    }

    #[test]
    fn dma_read_preserves_guest_byte_order_between_native_word_stores() {
        let mut rdram = vec![0u8; 0x1000];
        write_rdram_word(&mut rdram, 0x200, 0xAABB_CCDD);
        write_rdram_word(&mut rdram, 0x204, 0x1122_3344);

        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_dram(0x200);
        m.set_dma_mem(0x080);
        assert_eq!(m.dma_read(7), None);

        assert_eq!(m.load_w(0x080), 0xAABB_CCDD);
        assert_eq!(m.load_hu(0x080), 0xAABB);
        assert_eq!(m.load_hu(0x082), 0xCCDD);
        assert_eq!(
            [0x080, 0x081, 0x082, 0x083].map(|addr| m.load_bu(addr) as u8),
            [0xAA, 0xBB, 0xCC, 0xDD],
            "raw DMA is correct only if RSP DMEM and RDRAM expose the same logical byte order"
        );
    }

    #[test]
    fn dma_write_preserves_guest_byte_order_for_rdram_consumers() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.store_w(0x080, 0x7F01_80FF);
        m.store_w(0x084, 0x1234_FEDC);
        m.set_dma_mem(0x080);
        m.set_dma_dram(0x200);
        m.dma_write(7);

        assert_eq!(read_rdram_i16(m.rdram, 0x200), 0x7F01);
        assert_eq!(read_rdram_i16(m.rdram, 0x202) as u16, 0x80FF);
        assert_eq!(
            [0x200, 0x201, 0x202, 0x203].map(|addr| read_rdram_u8(m.rdram, addr)),
            [0x7F, 0x01, 0x80, 0xFF],
            "AI/RDRAM readers must observe the PCM bytes in guest order after RSP DMA write"
        );
    }

    #[test]
    fn dma_read_into_imem_signals_overlay_swap() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_mem(0x1000); // IMEM bit set
        assert_eq!(m.dma_read(0), Some(RspExitReason::SwapOverlay));
    }

    #[test]
    fn imem_overlay_completion_replaces_logical_words_and_advances_dma() {
        let mut rdram = vec![0u8; 0x1000];
        write_rdram_word(&mut rdram, 0x200, 0x3C01_1234);
        write_rdram_word(&mut rdram, 0x204, 0x3421_5678);
        let mut m = RspMachine::new(&mut rdram);
        let mut imem = [0xAA; DMEM_SIZE];
        m.set_dma_dram(0x200);
        m.set_dma_mem(0x1020);
        assert_eq!(m.dma_read(7), Some(RspExitReason::SwapOverlay));

        m.complete_imem_dma(&mut imem);

        assert_eq!(
            &imem[0x20..0x28],
            &[0x3C, 0x01, 0x12, 0x34, 0x34, 0x21, 0x56, 0x78]
        );
        assert_eq!(m.ctx.dma_mem_address, 0x1028);
        assert_eq!(m.ctx.dma_dram_address, 0x208);
    }

    #[test]
    fn pending_imem_dma_span_tracks_aligned_rectangular_and_wrapped_destinations() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_mem(0x1ffb);
        let descriptor = 7 | (1 << 12) | (8 << 20);
        assert_eq!(m.dma_read(descriptor), Some(RspExitReason::SwapOverlay));

        let span = m.pending_imem_dma_span();
        assert!(span.contains_pc(0x1ff8));
        assert!(span.contains_pc(0x1ffc));
        assert!(span.contains_pc(0x1000));
        assert!(span.contains_pc(0x1004));
        assert!(!span.contains_pc(0x1008));
        assert!(!span.contains_pc(0x1ff4));
    }

    #[test]
    fn pending_imem_dma_span_covering_a_bank_contains_every_pc() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_mem(0x1180);
        let descriptor = 0x0fff;
        assert_eq!(m.dma_read(descriptor), Some(RspExitReason::SwapOverlay));
        let span = m.pending_imem_dma_span();
        assert!([0x1000, 0x117c, 0x1180, 0x1ffc]
            .into_iter()
            .all(|pc| span.contains_pc(pc)));
    }

    #[test]
    fn dma_applies_eight_byte_alignment_count_and_skip() {
        let mut rdram = vec![0u8; 0x1000];
        for i in 0..8usize {
            rdram[0x100 + i] = 0x10 + i as u8;
            rdram[0x110 + i] = 0x20 + i as u8;
        }
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_dram(0x103);
        m.set_dma_mem(0x023);
        // length=8 bytes, count=2 lines, skip=8 bytes.
        let descriptor = 7 | (1 << 12) | (8 << 20);
        assert_eq!(m.dma_read(descriptor), None);
        assert_eq!(
            &m.dmem.as_bytes()[0x20..0x28],
            &(0x10u8..0x18).collect::<Vec<_>>()
        );
        assert_eq!(
            &m.dmem.as_bytes()[0x28..0x30],
            &(0x20u8..0x28).collect::<Vec<_>>()
        );
        assert_eq!(m.ctx.dma_mem_address, 0x30);
        assert_eq!(m.ctx.dma_dram_address, 0x120);
    }

    #[test]
    fn cp0_status_break_semaphore_and_dp_registers_are_observable() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);

        // SP_STATUS write commands: set HALT, set SIG0, then clear HALT.
        assert_eq!(m.write_cp0(4, (1 << 1) | (1 << 10)), None);
        assert_eq!(m.read_cp0(4) & ((1 << 0) | (1 << 7)), (1 << 0) | (1 << 7));
        m.write_cp0(4, 1 << 0);
        assert_eq!(m.read_cp0(4) & 1, 0);
        m.break_rsp();
        assert_eq!(m.read_cp0(4) & 3, 3, "BREAK sets HALT and BROKE");

        assert_eq!(m.read_cp0(7), 0, "first semaphore read returns clear");
        assert_eq!(m.read_cp0(7), 1, "read atomically sets semaphore");
        m.write_cp0(7, 0xFFFF_FFFF);
        assert_eq!(m.read_cp0(7), 0, "any semaphore write clears it");

        m.write_cp0(8, 0x12345F);
        m.write_cp0(9, 0x23456F);
        assert_eq!(m.read_cp0(8), 0x123458);
        assert_eq!(m.read_cp0(9), 0x234568);
        assert_eq!(
            m.read_cp0(10),
            0x234568,
            "RDP command DMA completes synchronously"
        );
        assert_eq!(m.read_cp0(5), 0);
        assert_eq!(m.read_cp0(6), 0);
        assert_eq!(
            m.take_dp_submissions(),
            vec![RspDpSubmission {
                start: 0x123458,
                end: 0x234568,
                xbus: false,
            }]
        );
        m.write_cp0(11, 1 << 1);
        m.write_cp0(8, 0x80);
        m.write_cp0(9, 0x100);
        assert!(m.take_dp_submissions()[0].xbus);
    }

    fn write_rdram_word(rdram: &mut [u8], offset: usize, value: u32) {
        rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    fn read_rdram_i16(rdram: &[u8], offset: usize) -> i16 {
        let o = offset ^ 2;
        i16::from_ne_bytes([rdram[o], rdram[o + 1]])
    }

    fn read_rdram_u8(rdram: &[u8], offset: usize) -> u8 {
        rdram[offset ^ 3]
    }

    #[test]
    fn all_fixed_width_vector_load_store_sizes_are_element_addressed() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        let cases = [
            (VLoadOp::Lbv, VStoreOp::Sbv, 1usize),
            (VLoadOp::Lsv, VStoreOp::Ssv, 2),
            (VLoadOp::Llv, VStoreOp::Slv, 4),
            (VLoadOp::Ldv, VStoreOp::Sdv, 8),
        ];
        for (load, store, count) in cases {
            for i in 0..count {
                m.dmem.write_bu(0x100 + i as u32, 0x80 + i as u8);
            }
            m.ctx.rsp.regs.r[3] = [0; 8];
            m.vload(load, 3, 4, 0x100, 0);
            m.vstore(store, 3, 4, 0x180, 0);
            for i in 0..count {
                assert_eq!(m.dmem.read_bu(0x180 + i as u32), 0x80 + i as u8);
            }
        }
    }

    #[test]
    fn quad_and_rest_pair_crosses_an_unaligned_boundary() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        for i in 0..16u32 {
            m.dmem.write_bu(0x105 + i, i as u8);
        }
        m.vload(VLoadOp::Lqv, 4, 0, 0x105, 0);
        m.vload(VLoadOp::Lrv, 4, 0, 0x115, 0);
        assert_eq!(
            vec_to_bytes(&m.ctx.rsp.regs.r[4]),
            core::array::from_fn(|i| i as u8)
        );

        m.vstore(VStoreOp::Sqv, 4, 0, 0x305, 0);
        m.vstore(VStoreOp::Srv, 4, 0, 0x315, 0);
        for i in 0..16u32 {
            assert_eq!(m.dmem.read_bu(0x305 + i), i as u8);
        }
    }

    #[test]
    fn quad_and_rest_stores_wrap_nonzero_byte_elements() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.ctx.rsp.regs.r[4] = bytes_to_vec(&core::array::from_fn(|i| i as u8));

        m.vstore(VStoreOp::Sqv, 4, 14, 0x305, 0);
        for i in 0..11u32 {
            assert_eq!(m.dmem.read_bu(0x305 + i), ((14 + i) & 15) as u8);
        }

        m.vstore(VStoreOp::Srv, 4, 14, 0x32F, 0);
        for i in 0..15u32 {
            assert_eq!(m.dmem.read_bu(0x320 + i), ((15 + i) & 15) as u8);
        }
    }

    #[test]
    fn packed_half_and_fourth_vector_transfers_match_bit_positions() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        for i in 0..8u32 {
            m.dmem.write_bu(0x100 + i, 0x10 + i as u8);
            m.dmem.write_bu(0x200 + i * 2, 0x20 + i as u8);
        }
        m.vload(VLoadOp::Lpv, 1, 0, 0x100, 0);
        m.vstore(VStoreOp::Spv, 1, 0, 0x140, 0);
        m.vload(VLoadOp::Luv, 2, 0, 0x100, 0);
        m.vstore(VStoreOp::Suv, 2, 0, 0x150, 0);
        for i in 0..8u32 {
            assert_eq!(
                m.ctx.rsp.regs.r[1][i as usize] as u16,
                (0x10 + i as u16) << 8
            );
            assert_eq!(
                m.ctx.rsp.regs.r[2][i as usize] as u16,
                (0x10 + i as u16) << 7
            );
            assert_eq!(m.dmem.read_bu(0x140 + i), 0x10 + i as u8);
            assert_eq!(m.dmem.read_bu(0x150 + i), 0x10 + i as u8);
        }

        m.vload(VLoadOp::Lhv, 3, 0, 0x200, 0);
        m.vstore(VStoreOp::Shv, 3, 0, 0x240, 0);
        for i in 0..8u32 {
            assert_eq!(
                m.ctx.rsp.regs.r[3][i as usize] as u16,
                (0x20 + i as u16) << 7
            );
            assert_eq!(m.dmem.read_bu(0x240 + i * 2), 0x20 + i as u8);
        }

        for i in 0..4u32 {
            m.dmem.write_bu(0x280 + i * 4, 0x30 + i as u8);
        }
        m.vload(VLoadOp::Lfv, 4, 8, 0x280, 0);
        m.vstore(VStoreOp::Sfv, 4, 8, 0x2C0, 0);
        for i in 0..4u32 {
            assert_eq!(
                m.ctx.rsp.regs.r[4][4 + i as usize] as u16,
                (0x30 + i as u16) << 7
            );
            assert_eq!(m.dmem.read_bu(0x2C0 + i * 4), 0x30 + i as u8);
        }
    }

    #[test]
    fn transpose_and_wrapped_store_cover_register_and_row_rotation() {
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        for i in 0..8u32 {
            m.dmem.write_h(0x300 + i * 2, (0x4000 + i) as i16);
        }
        m.vload(VLoadOp::Ltv, 8, 4, 0x300, 0);
        for i in 0..8usize {
            assert_eq!(m.ctx.rsp.regs.r[8 + i][(6 + i) & 7], (0x4000 + i) as i16);
        }
        m.vstore(VStoreOp::Stv, 8, 4, 0x340, 0);
        for i in 0..8u32 {
            assert_eq!(
                m.dmem.read_hu(0x340 + ((12 + i * 2) & 0xF)),
                0x4000 + i as u16
            );
        }

        m.ctx.rsp.regs.r[5] =
            core::array::from_fn(|i| u16::from_be_bytes([i as u8 * 2, i as u8 * 2 + 1]) as i16);
        let source = vec_to_bytes(&m.ctx.rsp.regs.r[5]);
        m.vstore(VStoreOp::Swv, 5, 3, 0x385, 0);
        for i in 0..16usize {
            assert_eq!(
                m.dmem.read_bu(0x380 + ((5 + i) & 0xF) as u32),
                source[(3 + i) & 0xF]
            );
        }
    }

    #[test]
    fn boot_sets_stack_pointer() {
        let mut rdram = vec![0u8; 16];
        let m = RspMachine::new(&mut rdram);
        assert_eq!(m.reg(1), 0xFC0);
        assert_eq!(m.reg(0), 0); // r0 hardwired zero
    }

    impl RspMachine<'_> {
        // test-only helper
        fn ctc2_read_vcc(&self) -> u16 {
            self.ctx.rsp.flags.vcc
        }
    }
}
