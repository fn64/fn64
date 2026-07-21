//! RSP DMEM (data memory) — the 0x1000-byte scratchpad the audio ucode reads
//! voice/envelope/ADPCM data out of and writes finished PCM into, plus the
//! `RSP_MEM_*` byte-lane-swizzled accessors the RSPRecomp-generated C emits.
//!
//! ## Why the byte-lane XOR (and why it matches fn64-runtime's `MEM_*`)
//!
//! The RSPRecomp codegen emits sub-word memory accesses as `RSP_MEM_B(off,
//! base)`, `RSP_MEM_H_LOAD(off, base)`, `RSP_MEM_W_LOAD(off, base)` etc.
//! Per the MIT-licensed RSPRecomp-generated C ABI and fn64-runtime's RDRAM
//! contract, word storage is host-native-endian, so a big-endian guest
//! byte/halfword address must be XOR-corrected onto the right lane within the
//! little-endian-stored word — exactly the `^2` (halfword) / `^3` (byte)
//! trick fn64-runtime's `rdram.rs` already documents and uses for main-RDRAM
//! `MEM_*`. We replicate the SAME lane math here for the 0x1000-byte DMEM so
//! the generated audio-ucode entry points observe identical bytes.
//!
//! Words (`RSP_MEM_W_*`) are native-endian with NO lane XOR (word-aligned,
//! `from_ne_bytes`); halfwords carry `^2`; bytes carry `^3` — the same three
//! rules as `rdram.rs`, just scoped to the 4 KiB DMEM address space instead
//! of 8 MiB RDRAM. The RSP address space wraps at 0x1000 (`rsp_mem_mask =
//! 0x1FFF` in the codegen masks IMEM+DMEM; DMEM proper is the low 0x1000),
//! so every offset is masked into range before the lane XOR.

/// RSP DMEM size: 4 KiB. The RSP has 4 KiB DMEM + 4 KiB IMEM; only DMEM holds
/// the data the compute ops touch (IMEM is instructions, handled by the
/// recompiler at generation time, not at run time).
pub const DMEM_SIZE: usize = 0x1000;

/// The DMEM address mask. RSP data addresses wrap within the 4 KiB DMEM.
pub const DMEM_MASK: u32 = 0x0FFF;

/// The 4 KiB RSP data memory scratchpad, with native-endian-word storage and
/// the `^2`/`^3` byte-lane-swizzled sub-word accessors the generated ucode C
/// links against. Fixed-size (never grows), unlike main RDRAM.
#[derive(Clone)]
pub struct Dmem {
    bytes: Box<[u8; DMEM_SIZE]>,
}

impl Default for Dmem {
    fn default() -> Self {
        Dmem::new()
    }
}

impl Dmem {
    /// A zeroed DMEM.
    pub fn new() -> Self {
        Dmem {
            bytes: Box::new([0u8; DMEM_SIZE]),
        }
    }

    /// Raw view of the whole 4 KiB, for bulk DMA fill/drain (the ucode's DMA
    /// engine loads voice data in and stores PCM out through here). No
    /// swizzle — this is the flat backing store; sub-word structure is
    /// imposed only by the `RSP_MEM_*` accessors.
    pub fn as_bytes(&self) -> &[u8; DMEM_SIZE] {
        &self.bytes
    }

    /// Mutable raw view of the whole 4 KiB, for bulk DMA fill.
    pub fn as_bytes_mut(&mut self) -> &mut [u8; DMEM_SIZE] {
        &mut self.bytes
    }

    /// `RSP_MEM_W_LOAD` / scalar `lw`: 32-bit load, sign is inherent in
    /// `i32`. The RSP scalar unit is byte-addressed with NO alignment
    /// restriction (public SGI RSP Programmer's Guide; the wm2000 audio ucode
    /// does `sw`/`lw` at DMEM 0xE to stash a DRAM pointer across the unused
    /// jump-table slots), so an unaligned word is the big-endian assembly of
    /// the four logical bytes at `offset..offset+4`, each on its `^3` lane.
    /// The aligned case keeps the native-word fast path (bit-identical to the
    /// byte-lane assembly). Mirrors `rdram.rs::read_w`.
    pub fn read_w(&self, offset: u32) -> i32 {
        let o = (offset & DMEM_MASK) as usize;
        if o & 3 == 0 {
            return i32::from_ne_bytes([
                self.bytes[o],
                self.bytes[(o + 1) & (DMEM_SIZE - 1)],
                self.bytes[(o + 2) & (DMEM_SIZE - 1)],
                self.bytes[(o + 3) & (DMEM_SIZE - 1)],
            ]);
        }
        let mut value = 0u32;
        for k in 0..4 {
            value = (value << 8) | u32::from(self.read_bu(offset.wrapping_add(k)));
        }
        value as i32
    }

    /// `RSP_MEM_W_STORE` / scalar `sw`: 32-bit store at any byte address (see
    /// `read_w` for the unaligned RSP semantics). An unaligned store places
    /// the value's big-endian bytes at logical `offset..offset+4` without
    /// touching neighbouring lanes -- the previous aligned-only native store
    /// scattered them across the wrong lanes, which corrupted the wm2000
    /// audio ucode's jump table (entry 6 at DMEM 0xC) when the ucode stored
    /// its segment pointer at DMEM 0xE.
    pub fn write_w(&mut self, offset: u32, value: i32) {
        let o = (offset & DMEM_MASK) as usize;
        if o & 3 == 0 {
            let b = value.to_ne_bytes();
            for (k, &byte) in b.iter().enumerate() {
                self.bytes[(o + k) & (DMEM_SIZE - 1)] = byte;
            }
            return;
        }
        for (k, &byte) in (value as u32).to_be_bytes().iter().enumerate() {
            self.write_bu(offset.wrapping_add(k as u32), byte);
        }
    }

    /// `RSP_MEM_H_LOAD` / scalar `lh`: 16-bit load, sign-extended, any byte
    /// address (halfword-aligned keeps the `^ 2` lane fast path; odd
    /// addresses assemble the two logical bytes). Mirrors `rdram.rs::read_h`.
    pub fn read_h(&self, offset: u32) -> i16 {
        self.read_hu(offset) as i16
    }

    /// `RSP_MEM_H_STORE` / scalar `sh`: 16-bit store at any byte address.
    pub fn write_h(&mut self, offset: u32, value: i16) {
        if offset & 1 == 0 {
            let o = ((offset ^ 2) & DMEM_MASK) as usize;
            let b = value.to_ne_bytes();
            self.bytes[o] = b[0];
            self.bytes[(o + 1) & (DMEM_SIZE - 1)] = b[1];
            return;
        }
        let b = (value as u16).to_be_bytes();
        self.write_bu(offset, b[0]);
        self.write_bu(offset.wrapping_add(1), b[1]);
    }

    /// `RSP_MEM_HU_LOAD` / scalar `lhu`: 16-bit load, zero-extended, any byte
    /// address.
    pub fn read_hu(&self, offset: u32) -> u16 {
        if offset & 1 == 0 {
            let o = ((offset ^ 2) & DMEM_MASK) as usize;
            return u16::from_ne_bytes([self.bytes[o], self.bytes[(o + 1) & (DMEM_SIZE - 1)]]);
        }
        (u16::from(self.read_bu(offset)) << 8) | u16::from(self.read_bu(offset.wrapping_add(1)))
    }

    /// `RSP_MEM_B`: int8_t, byte-lane XOR `offset ^ 3`, sign-extended.
    pub fn read_b(&self, offset: u32) -> i8 {
        let o = ((offset ^ 3) & DMEM_MASK) as usize;
        self.bytes[o] as i8
    }

    /// `RSP_MEM_B` store: byte-lane XOR `offset ^ 3`.
    pub fn write_b(&mut self, offset: u32, value: i8) {
        let o = ((offset ^ 3) & DMEM_MASK) as usize;
        self.bytes[o] = value as u8;
    }

    /// `RSP_MEM_BU`: uint8_t, byte-lane XOR `offset ^ 3`, zero-extended.
    pub fn read_bu(&self, offset: u32) -> u8 {
        let o = ((offset ^ 3) & DMEM_MASK) as usize;
        self.bytes[o]
    }

    /// `RSP_MEM_BU` store: byte-lane XOR `offset ^ 3`.
    pub fn write_bu(&mut self, offset: u32, value: u8) {
        let o = ((offset ^ 3) & DMEM_MASK) as usize;
        self.bytes[o] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmem_is_4kib_and_zeroed() {
        let d = Dmem::new();
        assert_eq!(d.as_bytes().len(), DMEM_SIZE);
        assert!(d.as_bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn word_roundtrips_native_no_swizzle() {
        let mut d = Dmem::new();
        d.write_w(0x40, 0x1234_5678);
        assert_eq!(d.read_w(0x40), 0x1234_5678);
    }

    #[test]
    fn byte_lane_swizzle_matches_word_layout() {
        // Store a word, then read its individual bytes via the ^3-swizzled
        // byte accessor: byte k of the big-endian value must come back at
        // word_off+k, exactly as fn64-runtime's rdram MEM_* contract.
        let mut d = Dmem::new();
        // A native-word store of 0xAABBCCDD lays bytes [DD CC BB AA] in memory
        // (little-endian host). MEM_BU(word+0) with ^3 reads lane 0^3=3 = 0xAA.
        d.write_w(0x00, 0xAABB_CCDDu32 as i32);
        assert_eq!(d.read_bu(0x00), 0xAA); // most-significant byte first
        assert_eq!(d.read_bu(0x01), 0xBB);
        assert_eq!(d.read_bu(0x02), 0xCC);
        assert_eq!(d.read_bu(0x03), 0xDD);
    }

    #[test]
    fn halfword_swizzle_matches_word_layout() {
        let mut d = Dmem::new();
        d.write_w(0x10, 0x1122_3344);
        // MEM_HU(word+0) ^2 -> high halfword 0x1122; +2 -> low 0x3344.
        assert_eq!(d.read_hu(0x10), 0x1122);
        assert_eq!(d.read_hu(0x12), 0x3344);
    }

    #[test]
    fn signed_byte_and_halfword_sign_extend() {
        let mut d = Dmem::new();
        d.write_b(0x20, -1);
        assert_eq!(d.read_b(0x20), -1);
        assert_eq!(d.read_bu(0x20), 0xFF);
        d.write_h(0x24, -2);
        assert_eq!(d.read_h(0x24), -2);
        assert_eq!(d.read_hu(0x24), 0xFFFE);
    }

    #[test]
    fn unaligned_word_store_preserves_neighbouring_halfwords() {
        // The wm2000 audio ucode's jump table lives at DMEM 0x0..0x20 with
        // two unused halfword slots at 0xE/0x10; command 0x0F stores a DRAM
        // segment pointer there with `sw r1, 0xe(r0)`. Entry 6 (0xC) and
        // entry 9 (0x12) must survive, and the pointer must read back.
        let mut d = Dmem::new();
        d.write_h(0x0c, 0x1208); // entry 6
        d.write_h(0x12, 0x127c_u16 as i16); // entry 9
        d.write_w(0x0e, 0x000d_5fc0);
        assert_eq!(d.read_hu(0x0c), 0x1208, "entry 6 clobbered by sw at 0xE");
        assert_eq!(d.read_hu(0x12), 0x127c, "entry 9 clobbered by sw at 0xE");
        assert_eq!(d.read_w(0x0e), 0x000d_5fc0, "sw/lw at 0xE must roundtrip");
        assert_eq!(d.read_hu(0x0e), 0x000d);
        assert_eq!(d.read_hu(0x10), 0x5fc0);
    }

    #[test]
    fn unaligned_word_access_is_big_endian_byte_addressed() {
        let mut d = Dmem::new();
        for k in 0..8u32 {
            d.write_bu(0x30 + k, 0x10 + k as u8);
        }
        // lw at +1/+2/+3 assembles the logical big-endian bytes.
        assert_eq!(d.read_w(0x31) as u32, 0x1112_1314);
        assert_eq!(d.read_w(0x32) as u32, 0x1213_1415);
        assert_eq!(d.read_w(0x33) as u32, 0x1314_1516);
        // sw at an odd address places big-endian bytes at logical positions.
        d.write_w(0x41, 0xAABB_CCDDu32 as i32);
        assert_eq!(
            [0x41, 0x42, 0x43, 0x44].map(|a| d.read_bu(a)),
            [0xAA, 0xBB, 0xCC, 0xDD]
        );
    }

    #[test]
    fn odd_address_halfword_access_is_byte_addressed() {
        let mut d = Dmem::new();
        d.write_bu(0x51, 0x7F);
        d.write_bu(0x52, 0x80);
        assert_eq!(d.read_hu(0x51), 0x7F80);
        assert_eq!(d.read_h(0x51), 0x7F80);
        d.write_h(0x61, 0x8001u16 as i16);
        assert_eq!(d.read_bu(0x61), 0x80);
        assert_eq!(d.read_bu(0x62), 0x01);
        assert_eq!(d.read_h(0x61) as u16, 0x8001);
    }

    #[test]
    fn address_wraps_within_4kib() {
        let mut d = Dmem::new();
        // 0x1000 wraps to 0x000; verify the mask is applied, not a panic.
        d.write_bu(0x1000, 0x5A);
        assert_eq!(d.read_bu(0x0000), 0x5A);
    }
}
