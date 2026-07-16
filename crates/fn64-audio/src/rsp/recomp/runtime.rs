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
//! The vector load/store element semantics are the public, community-
//! documented RSP behavior (n64brew "RSP" / "Vector loads and stores"): a
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
        }
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
        // Only e+1 within the register is written; e=15 writes just the one.
        if (e as usize) + 1 < 16 {
            bytes[(e as usize) + 1] = lo;
        }
        self.ctx.rsp.regs.r[vs as usize] = bytes_to_vec(&bytes);
    }

    /// `cfc2 rt, cd`: read a VU control register (0=VCO, 1=VCC, 2=VCE),
    /// sign-extended to 32 bits (the control regs read as signed 16-bit).
    pub fn cfc2(&self, cd: u8) -> u32 {
        let v: u16 = match cd & 3 {
            0 => self.ctx.rsp.flags.vco,
            1 => self.ctx.rsp.flags.vcc,
            _ => self.ctx.rsp.flags.vce as u16,
        };
        v as i16 as i32 as u32
    }

    /// `ctc2 rt, cd`: write a VU control register from a scalar reg.
    pub fn ctc2(&mut self, cd: u8, val: u32) {
        match cd & 3 {
            0 => self.ctx.rsp.flags.vco = val as u16,
            1 => self.ctx.rsp.flags.vcc = val as u16,
            _ => self.ctx.rsp.flags.vce = val as u8,
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
                let mut idx = 16 - count;
                for _ in 0..count {
                    if idx < 16 {
                        bytes[idx] = self.dmem.read_bu(a);
                    }
                    a = a.wrapping_add(1);
                    idx += 1;
                }
            }
            VLoadOp::Lpv | VLoadOp::Luv => {
                // Packed load: 8 bytes -> 8 lanes, each byte placed in the
                // high (LPV) or << 7 (LUV) part of the lane.
                let addr = base_val.wrapping_add((off as i32 * 8) as u32);
                let mut lanes = [0i16; LANES];
                for (i, lane) in lanes.iter_mut().enumerate() {
                    let b = self.dmem.read_bu(addr.wrapping_add(i as u32)) as i16;
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
                    let b = self.dmem.read_bu(addr.wrapping_add((i as u32) * 2)) as i16;
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
                    let b = self.dmem.read_bu(addr.wrapping_add((i as u32) * 4)) as i16;
                    lanes[i] = b << 7;
                }
                self.ctx.rsp.regs.r[vt as usize] = lanes;
                return;
            }
            VLoadOp::Ltv => {
                // Transpose load: load a row of 8 halfwords into 8 registers
                // at a rotating element. `vt` is the base of an 8-register
                // group; `e` selects the starting lane.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let vt_base = (vt & !7) as usize;
                for i in 0..LANES {
                    let a = addr.wrapping_add((i as u32) * 2);
                    let h = self.dmem.read_hu(a) as i16;
                    let reg = vt_base + ((i + (e >> 1)) & 7);
                    let lane = i;
                    self.ctx.rsp.regs.r[reg][lane] = h;
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
                while a < end && idx < 16 {
                    self.dmem.write_bu(a, bytes[idx & 0xF]);
                    a = a.wrapping_add(1);
                    idx += 1;
                }
            }
            VStoreOp::Srv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let start = addr & !0xF;
                let count = (addr - start) as usize;
                let mut a = start;
                let mut idx = 16 - count;
                for _ in 0..count {
                    self.dmem.write_bu(a, bytes[idx & 0xF]);
                    a = a.wrapping_add(1);
                    idx += 1;
                }
            }
            VStoreOp::Spv | VStoreOp::Suv => {
                // Packed store: 8 lanes -> 8 bytes.
                let addr = base_val.wrapping_add((off as i32 * 8) as u32);
                for i in 0..LANES {
                    let lane = reg[i] as u16;
                    let b = match op {
                        VStoreOp::Spv => (lane >> 8) as u8,
                        _ => (lane >> 7) as u8, // SUV
                    };
                    self.dmem.write_bu(addr.wrapping_add(i as u32), b);
                }
            }
            VStoreOp::Shv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                for i in 0..LANES {
                    let b = (reg[i] as u16 >> 7) as u8;
                    self.dmem.write_bu(addr.wrapping_add((i as u32) * 2), b);
                }
            }
            VStoreOp::Sfv => {
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                for i in 0..4usize {
                    let b = (reg[i] as u16 >> 7) as u8;
                    self.dmem.write_bu(addr.wrapping_add((i as u32) * 4), b);
                }
            }
            VStoreOp::Swv => {
                // Wrapping store of all 16 bytes starting at element e.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                for i in 0..16usize {
                    self.dmem
                        .write_bu(addr.wrapping_add(i as u32), bytes[(e + i) & 0xF]);
                }
            }
            VStoreOp::Stv => {
                // Transpose store: 8 registers' rotating lanes -> a row.
                let addr = base_val.wrapping_add((off as i32 * 16) as u32);
                let vt_base = (vt & !7) as usize;
                for i in 0..LANES {
                    let reg_idx = vt_base + ((i + (e >> 1)) & 7);
                    let h = self.ctx.rsp.regs.r[reg_idx][i] as u16;
                    self.dmem
                        .write_h(addr.wrapping_add((i as u32) * 2), h as i16);
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
    /// `len` is the raw `SP_RD_LEN` value; the real hardware length is the low
    /// 12 bits + 1 (skip/count fields ignored for these simple single-block
    /// audio DMAs — the audio ucode uses count=0/skip=0 blocks).
    pub fn dma_read(&mut self, len: u32) -> Option<RspExitReason> {
        if self.ctx.dma_mem_address & 0x1000 != 0 {
            return Some(RspExitReason::SwapOverlay);
        }
        let count = ((len & 0xFFF) + 1) as usize;
        let dram = self.ctx.dma_dram_address as usize & 0x00FF_FFFF;
        let mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 1);
        for i in 0..count {
            let src = dram + i;
            if src >= self.rdram.len() {
                break;
            }
            let b = self.rdram[src];
            self.dmem.as_bytes_mut()[(mem + i) & (DMEM_SIZE - 1)] = b;
        }
        None
    }

    /// `DO_DMA_WRITE` (`SP_WR_LEN` write): copy `len+1` bytes from DMEM at
    /// `dma_mem_address & 0xFFF` back to rdram at `dma_dram_address`.
    pub fn dma_write(&mut self, len: u32) {
        let count = ((len & 0xFFF) + 1) as usize;
        let dram = self.ctx.dma_dram_address as usize & 0x00FF_FFFF;
        let mem = self.ctx.dma_mem_address as usize & (DMEM_SIZE - 1);
        for i in 0..count {
            let dst = dram + i;
            if dst >= self.rdram.len() {
                break;
            }
            self.rdram[dst] = self.dmem.as_bytes()[(mem + i) & (DMEM_SIZE - 1)];
        }
    }

    /// Convenience accessor for the VU state the compute-op dispatcher wants.
    #[inline]
    pub fn vu(&mut self) -> &mut VuState {
        &mut self.ctx.rsp
    }
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
    fn dma_read_into_imem_signals_overlay_swap() {
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_dma_mem(0x1000); // IMEM bit set
        assert_eq!(m.dma_read(0), Some(RspExitReason::SwapOverlay));
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
