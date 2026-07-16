//! Parser for mupen64plus's `.m64p`-format savestate files (the `.stN`
//! files the faki-tools oracle/mupen64plus GUI produce), used as the
//! "reference RDRAM snapshot" input to state transplant.
//!
//! ## Provenance (clean-room protocol)
//!
//! This is a **file-format parser**, not a runtime reimplementation: the
//! byte layout below is transcribed directly from mupen64plus-core's own
//! `savestates_load_m64p` (`third_party/mupen64plus-core/src/main/
//! savestates.c` in the sibling `faki-tools` checkout this session was
//! granted read+run access to, lines 188-483 as of 2026-07-14). Reading a
//! file FORMAT that a tool this session is explicitly allowed to run
//! (the oracle, which links this exact mupen64plus-core) emits and
//! consumes is not "reading GPL runtime implementation code" in the sense
//! `fn64/AGENTS.md`'s clean-room protocol means to forbid (no runtime
//! BEHAVIOR was read or ported -- only the on-disk struct layout, needed
//! to parse the reference tool's own output). No mupen64plus CODE is
//! copied or executed here.
//!
//! ## Key finding this parser exists to capture
//!
//! The oracle's own `breakpoint`/`loadstate` commands read the CPU's LIVE
//! PC via `DebugGetCPUDataPtr` immediately after `M64CMD_STATE_LOAD`, but
//! that PC is consistently `0x80000180` (a dynarec/debugger dispatch stub)
//! across every fixture tested (see this crate's `bin/dump_snapshot.rs`
//! verification run), NOT the savestate's own saved resume PC. Register
//! contents (GPRs, CP0) DO match the savestate's real saved values (cross-
//! checked byte-for-byte against `oracle breakpoint`'s printed GPRs for
//! the same fixture) -- only the debugger's live PC read is stale/wrong
//! immediately after a state load. The true resume point is
//! `Cp0::epc` (Exception Program Counter): every fixture sampled has
//! `cp0_cause == 0` (no pending exception) and a raw `pc` field sitting at
//! a small, fixed set of dispatch addresses, which is the textbook
//! "paused right before ERET" shape -- ERET loads PC from EPC. Callers
//! transplanting a snapshot into a fresh executor should resume at
//! `cp0.epc`, not `Snapshot::pc`.
use std::io::Read;

use flate2::read::GzDecoder;
use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as RDRAM_MAX_SIZE;

const MAGIC: &[u8; 8] = b"M64+SAVE";
const MD5_LEN: usize = 32;
const SP_MEM_SIZE: usize = 0x2000;
const PIF_RAM_SIZE: usize = 0x40;
const TLB_LUT_ELEMS: usize = 0x0010_0000; // count of u32 elements, NOT bytes (savestates.c:431-432).
const CP0_REGS_COUNT: usize = 32;
const GPR_COUNT: usize = 32;
const FPR_COUNT: usize = 32;
const TLB_ENTRY_COUNT: usize = 32;
/// Packed size (bytes) of one `tlb_entry` as stored in the savestate,
/// mirroring the exact field order/sizes in `savestates.c`'s TLB-entry load
/// loop (approx. lines 447-472): i16 mask, u32 vpn2, u8 g, u8 asid, +2 pad,
/// u32 pfn_even, u8 c_even, u8 d_even, u8 v_even, +1 pad, u32 pfn_odd, u8
/// c_odd, u8 d_odd, u8 v_odd, u8 r, u32 start_even, u32 end_even, u32
/// phys_even, u32 start_odd, u32 end_odd, u32 phys_odd.
const TLB_ENTRY_PACKED_SIZE: usize =
    2 + 4 + 1 + 1 + 2 + 4 + 1 + 1 + 1 + 1 + 4 + 1 + 1 + 1 + 1 + 4 + 4 + 4 + 4 + 4 + 4;

/// GPR/CP0 register names in R4300/o32 order, matching the oracle's own
/// `GPR_NAMES` (`roms/NW4E/gates/oracle/src/main.rs`) for cross-checking
/// printed output by eye.
pub const GPR_NAMES: [&str; 32] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra",
];

/// CP0 register indices this crate names explicitly (the ones state
/// transplant / diagnostics need), per `device/r4300/cp0.h`'s enum order.
pub const CP0_BADVADDR: usize = 8;
pub const CP0_COUNT: usize = 9;
pub const CP0_STATUS: usize = 12;
pub const CP0_CAUSE: usize = 13;
pub const CP0_EPC: usize = 14;

#[derive(Debug)]
pub enum ParseError {
    Io(std::io::Error),
    BadMagic,
    UnexpectedEof(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "io error: {e}"),
            ParseError::BadMagic => write!(f, "not an M64+SAVE savestate (bad magic)"),
            ParseError::UnexpectedEof(field) => {
                write!(f, "unexpected EOF reading field: {field}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        ParseError::Io(e)
    }
}

/// The subset of a parsed savestate this crate's state-transplant path
/// needs: full RDRAM, all GPRs/CP0/hi/lo, and the header's version/ROM MD5
/// (for a sanity cross-check against the ROM the caller intends to run
/// against -- mupen64plus itself refuses to load a savestate whose MD5
/// doesn't match the currently-loaded ROM, `savestates.c` line ~245).
pub struct Snapshot {
    pub version: u32,
    pub rom_md5: String,
    /// Raw PC field as stored in the savestate (`savestates_load_set_pc`'s
    /// argument, `savestates.c:474`). See module doc: this is often NOT
    /// the real resume point -- prefer `cp0[CP0_EPC]` when `cp0[CP0_CAUSE]`
    /// is 0.
    pub pc: u32,
    pub gprs: [u64; GPR_COUNT],
    pub cp0: [u32; CP0_REGS_COUNT],
    pub mult_lo: u64,
    pub mult_hi: u64,
    /// Full 8 MiB RDRAM contents, N64 (big-endian-instruction-stream) byte
    /// order exactly as `DebugMemRead8`/the oracle's dumps present it --
    /// NOT yet converted to fn64_runtime::Rdram's native-word-order
    /// convention. See `to_fn64_rdram` for that conversion.
    pub rdram: Vec<u8>,
}

impl Snapshot {
    /// The resume PC a state-transplant caller should actually use: EPC
    /// when there's no pending exception (the paused-before-ERET shape
    /// this module doc describes), else the raw `pc` field. Falls back to
    /// `pc` if `cause` has an unexpected shape rather than guessing.
    pub fn resume_pc(&self) -> u32 {
        if self.cp0[CP0_CAUSE] == 0 {
            self.cp0[CP0_EPC]
        } else {
            self.pc
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn take(&mut self, len: usize, field: &'static str) -> Result<&'a [u8], ParseError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(ParseError::UnexpectedEof(field))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(ParseError::UnexpectedEof(field))?;
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, len: usize) {
        self.pos += len;
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, ParseError> {
        let bytes = self.take(4, field)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn i64_as_u64(&mut self, field: &'static str) -> Result<u64, ParseError> {
        let bytes = self.take(8, field)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }
}

/// Parse an on-disk `.m64p` savestate (gzip-compressed) into a [`Snapshot`].
///
/// Layout transcribed from `savestates_load_m64p` (see module doc for the
/// exact citation). `GETDATA`/`COPYARRAY` in the C source route through
/// `to_little_endian_buffer`, which is a no-op unless mupen64plus was built
/// with `M64P_BIG_ENDIAN` defined (it is not, on any desktop build this
/// project uses) -- so every multi-byte field in the on-disk buffer is
/// already host-native little-endian, not N64 big-endian; only the RDRAM
/// PAYLOAD bytes themselves carry N64 (effectively big-endian-instruction-
/// stream) byte order, because that's a raw memory-image copy
/// (`COPYARRAY(dev->rdram.dram, ...)`), not a per-field endian-converted
/// struct member.
pub fn parse(bytes: &[u8]) -> Result<Snapshot, ParseError> {
    let mut gz = GzDecoder::new(bytes);
    let mut data = Vec::new();
    gz.read_to_end(&mut data)?;

    let mut r = Reader::new(&data);

    let magic = r.take(8, "magic")?;
    if &magic[..MAGIC.len()] != MAGIC {
        return Err(ParseError::BadMagic);
    }
    // version: four big-shifted bytes in the C source (`version = (version
    // << 8) | *curr++` x4) -- i.e. big-endian, unlike everything after it.
    let version_bytes = r.take(4, "version")?;
    let version = u32::from_be_bytes(version_bytes.try_into().unwrap());

    let md5_bytes = r.take(MD5_LEN, "rom_md5")?;
    let rom_md5 = String::from_utf8_lossy(md5_bytes).into_owned();

    // --- fixed-size device-register prefix (savestates.c ~308-420) ---
    for _ in 0..10 {
        r.u32("rdram_regs")?;
    }
    r.skip(4); // padding
    r.u32("mi_init_mode")?;
    r.skip(4); // dup
    r.u32("mi_version")?;
    r.u32("mi_intr")?;
    r.u32("mi_intr_mask")?;
    r.skip(4); // padding
    r.skip(8); // dup + padding

    for _ in 0..13 {
        r.u32("pi_regs")?;
    }

    for _ in 0..4 {
        r.u32("sp_regs")?;
    }
    r.skip(4); // padding
    r.u32("sp_status")?;
    r.skip(16); // duplicated SP flags + padding
    r.u32("sp_dma_full")?;
    r.u32("sp_dma_busy")?;
    r.u32("sp_semaphore")?;
    r.u32("sp_pc")?;
    r.u32("sp_ibist")?;

    for _ in 0..4 {
        r.u32("si_regs")?;
    }
    for _ in 0..14 {
        r.u32("vi_regs")?;
    }
    r.u32("vi_delay")?;
    for _ in 0..8 {
        r.u32("ri_regs")?;
    }
    for _ in 0..6 {
        r.u32("ai_regs")?;
    }
    for _ in 0..4 {
        r.u32("ai_fifo")?;
    }

    r.u32("dpc_start")?;
    r.u32("dpc_end")?;
    r.u32("dpc_current")?;
    r.skip(4); // padding
    r.u32("dpc_status")?;
    r.skip(12); // duplicated DPC flags + padding
    r.u32("dpc_clock")?;
    r.u32("dpc_bufbusy")?;
    r.u32("dpc_pipebusy")?;
    r.u32("dpc_tmem")?;
    r.u32("dps_tbist")?;
    r.u32("dps_test_mode")?;
    r.u32("dps_buftest_addr")?;
    r.u32("dps_buftest_data")?;

    // --- the payload this crate actually wants ---
    let rdram = r.take(RDRAM_MAX_SIZE, "rdram")?.to_vec();
    r.skip(SP_MEM_SIZE);
    r.skip(PIF_RAM_SIZE);

    r.u32("use_flashram")?;
    r.skip(4 + 8 + 4 + 4); // old flashram state

    r.skip(TLB_LUT_ELEMS * 4); // LUT_r: elements of u32, not bytes.
    r.skip(TLB_LUT_ELEMS * 4); // LUT_w

    r.u32("llbit")?;

    let mut gprs = [0u64; GPR_COUNT];
    for slot in gprs.iter_mut() {
        *slot = r.i64_as_u64("gpr")?;
    }

    let mut cp0 = [0u32; CP0_REGS_COUNT];
    for slot in cp0.iter_mut() {
        *slot = r.u32("cp0")?;
    }

    let mult_lo = r.i64_as_u64("mult_lo")?;
    let mult_hi = r.i64_as_u64("mult_hi")?;

    r.skip(FPR_COUNT * 8); // fprs -- not needed by this crate yet.
    r.u32("fcr0")?;
    r.u32("fcr31")?;

    r.skip(TLB_ENTRY_COUNT * TLB_ENTRY_PACKED_SIZE);

    let pc = r.u32("pc")?;

    Ok(Snapshot {
        version,
        rom_md5,
        pc,
        gprs,
        cp0,
        mult_lo,
        mult_hi,
        rdram,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic savestate built with the exact field layout `parse`
    /// expects, so this test is independent of any real fixture file (no
    /// game content in this crate/repo). Verifies the parser's offset
    /// arithmetic end-to-end, not just against one golden file.
    fn synthetic_savestate(pc: u32, gprs: &[u64; 32], cp0: &[u32; 32]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&0x0001_0900u32.to_be_bytes());
        buf.extend_from_slice(b"00000000000000000000000000000000".as_slice()[..32].as_ref());

        // device-register prefix, all zeroed, sized to match `parse`'s skips.
        buf.extend(std::iter::repeat_n(0u8, 10 * 4 + 4)); // rdram_regs + pad
        buf.extend(std::iter::repeat_n(0u8, 4 + 4 + 4 + 4 + 4 + 4 + 8)); // mi block
        buf.extend(std::iter::repeat_n(0u8, 13 * 4)); // pi
        buf.extend(std::iter::repeat_n(
            0u8,
            4 * 4 + 4 + 4 + 16 + 4 + 4 + 4 + 4 + 4,
        )); // sp
        buf.extend(std::iter::repeat_n(0u8, 4 * 4)); // si
        buf.extend(std::iter::repeat_n(0u8, 14 * 4 + 4)); // vi
        buf.extend(std::iter::repeat_n(0u8, 8 * 4)); // ri
        buf.extend(std::iter::repeat_n(0u8, 6 * 4 + 4 * 4)); // ai
        buf.extend(std::iter::repeat_n(
            0u8,
            4 + 4 + 4 + 4 + 4 + 12 + 4 + 4 + 4 + 4 + 4 + 4 + 4 + 4,
        )); // dpc/dps

        buf.extend(std::iter::repeat_n(0xABu8, RDRAM_MAX_SIZE));
        buf.extend(std::iter::repeat_n(0u8, SP_MEM_SIZE));
        buf.extend(std::iter::repeat_n(0u8, PIF_RAM_SIZE));
        buf.extend(std::iter::repeat_n(0u8, 4 + 4 + 8 + 4 + 4)); // flashram block
        buf.extend(std::iter::repeat_n(0u8, TLB_LUT_ELEMS * 4 * 2)); // LUT_r + LUT_w
        buf.extend(std::iter::repeat_n(0u8, 4)); // llbit

        for g in gprs {
            buf.extend_from_slice(&g.to_le_bytes());
        }
        for c in cp0 {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&0u64.to_le_bytes()); // mult_lo
        buf.extend_from_slice(&0u64.to_le_bytes()); // mult_hi
        buf.extend(std::iter::repeat_n(0u8, FPR_COUNT * 8)); // fprs
        buf.extend_from_slice(&0u32.to_le_bytes()); // fcr0
        buf.extend_from_slice(&0u32.to_le_bytes()); // fcr31
        buf.extend(std::iter::repeat_n(
            0u8,
            TLB_ENTRY_COUNT * TLB_ENTRY_PACKED_SIZE,
        ));

        buf.extend_from_slice(&pc.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // next_interrupt
        buf.extend_from_slice(&0u32.to_le_bytes()); // old next_vi
        buf.extend_from_slice(&0u32.to_le_bytes()); // vi_field
        buf.extend_from_slice(&[0u8; 1024]); // queue
        buf.extend_from_slice(&[0u8; 4]); // using_tlb_data

        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&buf).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn round_trips_pc_gprs_cp0_and_rdram() {
        let mut gprs = [0u64; 32];
        gprs[29] = 0x8005_6ff0; // sp
        gprs[31] = 0x8011_87ac; // ra
        let mut cp0 = [0u32; 32];
        cp0[CP0_STATUS] = 0x2000_ff01;
        cp0[CP0_CAUSE] = 0; // no pending exception -> resume_pc() should use EPC
        cp0[CP0_EPC] = 0x8012_ff04;

        let bytes = synthetic_savestate(0x8000_1000, &gprs, &cp0);
        let snap = parse(&bytes).expect("parse should succeed");

        assert_eq!(snap.pc, 0x8000_1000);
        assert_eq!(snap.gprs[29], 0x8005_6ff0);
        assert_eq!(snap.gprs[31], 0x8011_87ac);
        assert_eq!(snap.cp0[CP0_STATUS], 0x2000_ff01);
        assert_eq!(snap.cp0[CP0_EPC], 0x8012_ff04);
        assert_eq!(snap.rdram.len(), RDRAM_MAX_SIZE);
        assert!(snap.rdram.iter().all(|&b| b == 0xAB));

        // The headline finding this parser exists to capture: with
        // cause==0, resume_pc() prefers EPC over the raw (often-stale, per
        // the oracle's own live-PC-read quirk) pc field.
        assert_eq!(snap.resume_pc(), 0x8012_ff04);
    }

    #[test]
    fn resume_pc_falls_back_to_raw_pc_when_an_exception_is_pending() {
        let gprs = [0u64; 32];
        let mut cp0 = [0u32; 32];
        cp0[CP0_CAUSE] = 0x0000_0400; // nonzero: a real pending exception code
        cp0[CP0_EPC] = 0x8099_9999;

        let bytes = synthetic_savestate(0x8000_1000, &gprs, &cp0);
        let snap = parse(&bytes).unwrap();
        assert_eq!(snap.resume_pc(), 0x8000_1000);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let bytes = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        use std::io::Write;
        let mut enc = bytes;
        enc.write_all(b"NOTASAVE").unwrap();
        let compressed = enc.finish().unwrap();
        assert!(matches!(parse(&compressed), Err(ParseError::BadMagic)));
    }
}
