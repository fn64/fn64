//! The rdram buffer, its `MEM_*`-equivalent accessors, and the `RdramAddr`
//! translation newtype. See `docs/DESIGN.md` section 3.
//!
//! Semantics (byte-lane XOR, sign extension, KSEG0 base subtraction) are
//! transcribed from `aki-recomp/runtime/ABI-SURFACE.md` section (c), which
//! mechanically extracted them from N64Recomp-generated C (MIT-licensed
//! recompiler output; no vendor runtime implementation was read).

/// Default N64 RDRAM capacity (8 MB, the common console configuration both
/// ported games in `aki-recomp` target). A future multi-console config
/// point, not a magic constant scattered through call sites.
pub const DEFAULT_RDRAM_SIZE: usize = 8 * 1024 * 1024;

/// The KSEG0 base subtracted by every generated `MEM_*` macro. Per
/// ABI-SURFACE.md section (c): "The subtraction of 0xFFFFFFFF80000000 (not
/// 0x80000000) is deliberate 64-bit-safe translation math... correctly
/// cancels sign extension for both a plain unsigned 32-bit vram value...
/// and its 64-bit-sign-extended form, landing on the same rdram-relative
/// byte offset either way."
const KSEG0_BASE_SIGN_EXTENDED: u64 = 0xFFFF_FFFF_8000_0000;

/// An N64 vram/KSEG0 address as MIPS code computes it, already translated
/// to an rdram-relative byte offset. See `docs/DESIGN.md` section 3's
/// `RdramAddr` writeup for why this must be a newtype rather than a bare
/// `u32`/`u64` passed around ad hoc: the KSEG0-base translation is easy to
/// get subtly wrong for exactly half of its inputs (a 64-bit
/// sign-extended `gpr` vs. a plain 32-bit vram value) if hand-rolled at a
/// second call site.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct RdramAddr(u32);

impl RdramAddr {
    /// Construct directly from an already-resolved rdram-relative byte
    /// offset (e.g. a constant known to already be in this form).
    pub const fn from_offset(offset: u32) -> Self {
        RdramAddr(offset)
    }

    /// Construct from a MIPS `gpr` (per ABI-SURFACE.md section (b), a
    /// `recomp_context` register field is `uint64_t`, and generated code
    /// may carry a sign-extended 64-bit KSEG0 address in it). Replicates
    /// the generated `MEM_*` macros' own base-subtraction math exactly
    /// (section (c)) so this is correct for both a plain 32-bit vram value
    /// and its 64-bit sign-extended form.
    pub fn from_gpr(reg: u64) -> Self {
        RdramAddr(reg.wrapping_sub(KSEG0_BASE_SIGN_EXTENDED) as u32)
    }

    pub const fn offset(self) -> u32 {
        self.0
    }
}

/// Owns the single rdram allocation. Every consumer (`fn64-abi` shims, the
/// executor, `fn64-rt64`'s gfx task marshalling) borrows this; nothing
/// allocates a second copy. See `docs/DESIGN.md` section 3's "rdram buffer
/// ownership" writeup: this matches the ABI's own contract of "one shared
/// buffer, passed by reference to every RECOMP_FUNC," not a per-caller view.
pub struct Rdram {
    bytes: Box<[u8]>,
}

impl Rdram {
    pub fn new(size: usize) -> Self {
        Rdram {
            bytes: vec![0u8; size].into_boxed_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `MEM_W`: int32_t, word-aligned, no byte-lane XOR, sign-extended.
    pub fn read_w(&self, addr: RdramAddr) -> i32 {
        let o = addr.offset() as usize;
        i32::from_be_bytes(self.bytes[o..o + 4].try_into().unwrap())
    }

    pub fn write_w(&mut self, addr: RdramAddr, value: i32) {
        let o = addr.offset() as usize;
        self.bytes[o..o + 4].copy_from_slice(&value.to_be_bytes());
    }

    /// `MEM_H`: int16_t, byte-lane XOR `offset ^ 2`, sign-extended.
    pub fn read_h(&self, addr: RdramAddr) -> i16 {
        let o = (addr.offset() ^ 2) as usize;
        i16::from_be_bytes(self.bytes[o..o + 2].try_into().unwrap())
    }

    pub fn write_h(&mut self, addr: RdramAddr, value: i16) {
        let o = (addr.offset() ^ 2) as usize;
        self.bytes[o..o + 2].copy_from_slice(&value.to_be_bytes());
    }

    /// `MEM_HU`: uint16_t, byte-lane XOR `offset ^ 2`, zero-extended.
    pub fn read_hu(&self, addr: RdramAddr) -> u16 {
        let o = (addr.offset() ^ 2) as usize;
        u16::from_be_bytes(self.bytes[o..o + 2].try_into().unwrap())
    }

    pub fn write_hu(&mut self, addr: RdramAddr, value: u16) {
        let o = (addr.offset() ^ 2) as usize;
        self.bytes[o..o + 2].copy_from_slice(&value.to_be_bytes());
    }

    /// `MEM_B`: int8_t, byte-lane XOR `offset ^ 3`, sign-extended.
    pub fn read_b(&self, addr: RdramAddr) -> i8 {
        let o = (addr.offset() ^ 3) as usize;
        self.bytes[o] as i8
    }

    pub fn write_b(&mut self, addr: RdramAddr, value: i8) {
        let o = (addr.offset() ^ 3) as usize;
        self.bytes[o] = value as u8;
    }

    /// `MEM_BU`: uint8_t, byte-lane XOR `offset ^ 3`, zero-extended.
    pub fn read_bu(&self, addr: RdramAddr) -> u8 {
        let o = (addr.offset() ^ 3) as usize;
        self.bytes[o]
    }

    pub fn write_bu(&mut self, addr: RdramAddr, value: u8) {
        let o = (addr.offset() ^ 3) as usize;
        self.bytes[o] = value;
    }

    /// Raw pointer to the start of the buffer, for `fn64-abi` to hand to
    /// generated C's `uint8_t* rdram` parameter. The only sanctioned
    /// escape hatch for the "one shared buffer" rule in `docs/DESIGN.md`
    /// section 3 — generated code's own calling convention requires a raw
    /// pointer, not a Rust reference.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }
}

impl Default for Rdram {
    fn default() -> Self {
        Rdram::new(DEFAULT_RDRAM_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdram_addr_from_gpr_matches_plain_32_bit() {
        // A plain 32-bit vram value with no sign extension.
        let plain = RdramAddr::from_gpr(0x8000_1234);
        assert_eq!(plain.offset(), 0x1234);
    }

    #[test]
    fn rdram_addr_from_gpr_matches_sign_extended_64_bit() {
        // The same logical address, sign-extended to 64 bits the way a
        // gpr may carry it (per ABI-SURFACE.md section (b)).
        let extended = RdramAddr::from_gpr(0xFFFF_FFFF_8000_1234);
        assert_eq!(extended.offset(), 0x1234);
    }

    #[test]
    fn word_read_write_roundtrip() {
        let mut rdram = Rdram::new(64);
        let addr = RdramAddr::from_offset(0x10);
        rdram.write_w(addr, -1);
        assert_eq!(rdram.read_w(addr), -1);
        assert_eq!(&rdram.bytes[0x10..0x14], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn halfword_byte_lane_xor() {
        let mut rdram = Rdram::new(64);
        // offset 0 XOR 2 = 2: the halfword at word-offset 0 within a
        // big-endian word lands at byte offset 2, not 0.
        rdram.write_h(RdramAddr::from_offset(0), 0x1234);
        assert_eq!(&rdram.bytes[2..4], &[0x12, 0x34]);
        assert_eq!(rdram.read_h(RdramAddr::from_offset(0)), 0x1234);
    }

    #[test]
    fn byte_lane_xor_and_zero_extension() {
        let mut rdram = Rdram::new(64);
        // offset 0 XOR 3 = 3: the byte at word-offset 0 lands at byte 3.
        rdram.write_bu(RdramAddr::from_offset(0), 0xFE);
        assert_eq!(rdram.bytes[3], 0xFE);
        assert_eq!(rdram.read_bu(RdramAddr::from_offset(0)), 0xFE);
        // read_b sign-extends; read_bu zero-extends -- same bits, different result.
        assert_eq!(rdram.read_b(RdramAddr::from_offset(0)), -2i8);
    }
}
