//! The PI/ROM seam: a read-only byte source (`RomStorage`) plus the DMA
//! completion model `osEPiStartDma`/`osCartRomInit`-family shims drive.
//!
//! ## Provenance
//!
//! Semantics below come from: the public libultra manual's PI Manager /
//! Cartridge Domain sections (`osCartRomInit`, `osEPiStartDma`,
//! `osCreatePiManager`, `osVirtualToPhysical`'s documented KSEG0/1
//! translation role); `aki-recomp/runtime/ABI-SURFACE.md`'s mechanically
//! extracted call-site evidence (section (a)'s `_recomp` shim inventory);
//! and `aki-recomp/runtime/M1-WORKLIST.md`'s rung-cited call shapes (rung
//! 9's `osCreatePiManager` identification, rung 10b's `osCartRomInit`
//! correction and its direct `osEPiStartDma` unblock). No GPL runtime PI/DMA
//! implementation was read -- this is a fresh design against the documented
//! libultra contract plus our own byte-cited call-site evidence.
//!
//! ## Design: async-looking API, synchronous-in-this-model completion
//!
//! Real N64 `osEPiStartDma` is asynchronous: it kicks off a PI DMA and
//! returns immediately, with completion signaled later via a message posted
//! to a caller-supplied `OSMesgQueue` (the libultra manual's documented
//! "PI manager" pattern -- a dedicated thread owns the PI command queue and
//! posts completion messages, which is exactly `osCreatePiManager`'s
//! `cmdQ`/rung 9's identified role). `docs/DESIGN.md` section 2's "SI/PI
//! completion messages" design point says this directly: "DMA completion is
//! host-driven... and the correct model is 'post the completion message to
//! the registered OSMesgQueue, let the next coroutine-resume decision (not a
//! new host thread) pick up the woken thread.'"
//!
//! This module's `PiDma::start_dma` performs the byte copy immediately (a
//! host file read is not meaningfully slower than emulating multi-tick DMA
//! latency for this milestone, and `docs/DESIGN.md`'s explicit "no wall-clock
//! in core" rule means there is no virtual-time cost to model without
//! inventing one with no evidence behind it) but does NOT itself decide
//! when the completion message is posted -- it returns a `DmaCompletion`
//! value the caller (an `fn64-abi` shim) feeds to
//! `Executor::inject_event(ExternalEvent::DirectPost { .. })`, the exact
//! "ONE explicit host-side injection point" `docs/DESIGN.md` section 2
//! already establishes for every other completion source (VI, timers). This
//! keeps `fn64-runtime`'s rom module free of any `Executor` dependency
//! (matching this crate's existing "pure, standalone core" shape -- see
//! `timer.rs`'s identical split between "what changed" and "who acts on
//! it") while still routing every DMA completion through the same queue
//! machinery a blocking guest `osSendMesg` uses, closing the same
//! "asymmetry between guest and host senders" rung-18b-derived design point
//! that `docs/DESIGN.md` calls out for VI/timer events.

use crate::rdram::RdramAddr;
use crate::save::SaveStorage;
use crate::trace::DmaDirection;

/// Base of the PI **domain-2 address space** (the SRAM/save cartridge
/// domain). A PI DMA whose `devAddr` is `>= this` is a SAVE access, not a
/// cartridge-ROM read, and must route to `SaveStorage` (offset = `devAddr -
/// SRAM_DOMAIN2_BASE`), NOT the ROM image.
///
/// Byte-cited: OoT decomp `include/ultra64/rcp.h:714`
/// `#define PI_DOM2_ADDR2 0x08000000 /* to 0x0FFFFFFF */` -- the domain-2
/// address space (SRAM cartridge base). OoT's `SsSram_ReadWrite` passes
/// `OS_K1_TO_PHYSICAL(0xA8000000)` = `0xA8000000 - 0xA0000000` = `0x08000000`
/// as the DMA `devAddr` (decomp `z_sram.c:672`, recomp
/// `games/OOTU/RecompiledFuncs/funcs_34.c:10632` `a0 = 0x800 << 16`). This is
/// distinct from the domain-2 *register* base `0x05000000`
/// (`PI_BSD_DOM2_LAT_REG`); the SRAM *data* transfer targets `0x08000000`.
pub const SRAM_DOMAIN2_BASE: u32 = 0x0800_0000;

/// Is this PI-DMA `devAddr` a domain-2 (SRAM/save) access rather than a
/// cartridge-ROM read? See `SRAM_DOMAIN2_BASE`.
pub fn is_sram_dev_addr(dev_addr: u32) -> bool {
    dev_addr >= SRAM_DOMAIN2_BASE
}

/// A read-only byte source for cartridge-domain PI reads. `fn64-shell` (or a
/// test) supplies the real implementation (an mmap'd/loaded ROM file);
/// `fn64-runtime` has no file I/O of its own, matching this crate's existing
/// "pure Rust core, no OS/file dependency" shape (`lib.rs`'s module doc: this
/// crate "has zero knowledge of fn64-abi's extern C surface... it is
/// deliberately the independently-testable core").
///
/// Read-only by design: real cartridge ROM is physically read-only memory;
/// there is no `write` method here to accidentally add, and no future caller
/// can bypass that by reaching for a different trait -- this is the only
/// ROM-reading seam in the crate.
pub trait RomStorage {
    /// Total ROM size in bytes.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read `buf.len()` bytes starting at ROM byte offset `rom_offset` into
    /// `buf`. Panics (loud trap, not a silent short-read) if the requested
    /// range exceeds the ROM's real length -- a real cartridge DMA
    /// requesting an out-of-range address is a bug in the caller (or a
    /// mis-decoded `osPiHandle`), not a condition to paper over with zeros.
    fn read_into(&self, rom_offset: u32, buf: &mut [u8]);
}

/// A `RomStorage` backed by an in-memory byte slice (owned `Vec<u8>`) --
/// what `fn64-shell` uses once it has loaded the user's own ROM file (per
/// `README.md`'s "no game content ships in this repo" rule: the bytes come
/// from the user's own file, never checked in), and what this crate's own
/// tests use directly with synthetic bytes.
pub struct InMemoryRom {
    bytes: Vec<u8>,
}

impl InMemoryRom {
    pub fn new(bytes: Vec<u8>) -> Self {
        InMemoryRom { bytes }
    }
}

impl RomStorage for InMemoryRom {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_into(&self, rom_offset: u32, buf: &mut [u8]) {
        let start = rom_offset as usize;
        let end = start + buf.len();
        assert!(
            end <= self.bytes.len(),
            "InMemoryRom::read_into: range {start:#x}..{end:#x} exceeds ROM length {:#x} -- a \
             real cartridge DMA requesting past the end of ROM is a caller bug (mis-decoded PI \
             handle/offset), not something to silently truncate",
            self.bytes.len()
        );
        buf.copy_from_slice(&self.bytes[start..end]);
    }
}

// Direction of a PI DMA transfer, per `osEPiStartDma`'s documented `flag`
// argument (`OS_READ`/`OS_WRITE` in the public libultra manual). This
// module reuses `trace::DmaDirection` (imported above) rather than
// declaring a second, competing enum -- per `AGENTS.md`'s "mechanism over
// patch," a `DmaCompletion` value handed to `Executor::inject_event`/traced
// via `TraceKind::Dma` needs exactly one direction type, not two
// `DmaDirection`s that happen to share variant names and require a
// conversion at the call site. Cartridge ROM is read-only, so `ToRdram` (a
// ROM read) is the only direction this milestone's shims exercise;
// `FromRdram` is modeled for completeness against the documented API shape
// (a future EEPROM/flash `_recomp` shim would need it) but has no real
// backing store to write to yet -- see `PiDma::start_dma`'s
// `unimplemented!` for that direction.

/// The result of a completed PI DMA, carrying exactly what the caller needs
/// to post a completion message through `Executor::inject_event` -- see
/// module doc's "async-looking API" design note. Deliberately does NOT post
/// the message itself (no `Executor` dependency in this crate).
pub struct DmaCompletion {
    pub direction: DmaDirection,
    pub dram_addr: RdramAddr,
    pub dev_addr: u32,
    pub len: u32,
}

/// The PI-manager-owned DMA engine. Exactly one exists per running game
/// (owned by whatever holds the `RomStorage`, e.g. `fn64-shell`'s top-level
/// state), mirroring real hardware's single PI bus -- there is only ever one
/// PI DMA in flight at a time on real N64 (the manual documents
/// `osCreatePiManager`'s command queue as serializing concurrent
/// `osEPiStartDma` requests onto one PI channel), which this module reflects
/// by taking `&mut self` for `start_dma` rather than allowing concurrent
/// calls to reason about interleaved transfers.
pub struct PiDma<R: RomStorage> {
    rom: R,
    /// The save-backing store domain-2 (SRAM) DMAs route to, `None` until a
    /// caller registers one via `set_save`. A domain-2 DMA with no save
    /// registered is a loud trap (see `sram_read_into`/`sram_write_from`),
    /// not a silent ROM read past its end -- the old bug this fix removes.
    save: Option<Box<dyn SaveStorage>>,
}

impl<R: RomStorage> PiDma<R> {
    pub fn new(rom: R) -> Self {
        PiDma { rom, save: None }
    }

    /// Register the domain-2 (SRAM/EEPROM/Flash) save-backing store this PI
    /// engine routes `devAddr >= SRAM_DOMAIN2_BASE` DMAs to. Mirrors how the
    /// ROM is installed at construction -- the harness/`fn64-shell` supplies
    /// an `InMemorySaveStorage`/`FileSaveStorage` of the game's save size.
    pub fn set_save(&mut self, save: Box<dyn SaveStorage>) {
        self.save = Some(save);
    }

    pub fn has_save(&self) -> bool {
        self.save.is_some()
    }

    fn save_mut(&mut self, dir: &str) -> &mut dyn SaveStorage {
        self.save.as_deref_mut().unwrap_or_else(|| {
            panic!(
                "PiDma: a domain-2 (SRAM, devAddr >= {SRAM_DOMAIN2_BASE:#x}) {dir} DMA arrived \
                 but no save store is registered -- call PiDma::set_save(..) with the game's \
                 save-backing store before any save-domain DMA (see set_save's doc comment). \
                 This is a harness wiring bug, not something to silently route to the ROM image."
            )
        })
    }

    /// Read `buf.len()` SRAM bytes at `sram_offset` (already `devAddr -
    /// SRAM_DOMAIN2_BASE`) into `buf` as FLAT bytes -- the save chip is a
    /// plain byte buffer. The caller word-swizzles into rdram (see
    /// `osEPiStartDma_recomp`), exactly like a ROM DMA-in, because the guest
    /// reads the destination back via `MEM_BU` (`^3` byte-lane XOR, recomp.h)
    /// -- a flat rdram write would read back byte-swapped within each word.
    pub fn sram_read_into(&mut self, sram_offset: u32, buf: &mut [u8]) {
        self.save_mut("read (device->RDRAM)")
            .read_into(sram_offset as usize, buf);
    }

    /// Write FLAT `data` bytes to the save chip at `sram_offset` (already
    /// `devAddr - SRAM_DOMAIN2_BASE`). The caller un-swizzles rdram's
    /// native-word bytes back to flat save order first (see
    /// `osEPiStartDma_recomp`'s FromRdram arm).
    pub fn sram_write_from(&mut self, sram_offset: u32, data: &[u8]) {
        self.save_mut("write (RDRAM->device)")
            .write_from(sram_offset as usize, data);
    }

    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// Read raw ROM bytes directly, bypassing `start_dma`'s rdram-copy step
    /// -- for a caller (like `fn64-abi`'s shims) that only ever borrows a
    /// raw `rdram` pointer rather than owning an `Rdram` instance and so
    /// cannot call `start_dma` (which takes `&mut Rdram`). Same underlying
    /// `RomStorage::read_into` contract (loud panic on an out-of-range
    /// read, never a silent short-read).
    pub fn read_rom_bytes(&self, dev_addr: u32, buf: &mut [u8]) {
        self.rom.read_into(dev_addr, buf);
    }

    /// `osEPiStartDma(handle, mb, direction)`'s core transfer, once the
    /// caller (`fn64-abi`'s shim) has already resolved the `OSIoMesg`'s
    /// `dramAddr`/`devAddr`/`size` fields and the handle's cartridge-domain
    /// timing (`osCartRomInit`'s role, per rung 10b -- validated but not
    /// modeled numerically here: this milestone's shims don't yet need PI
    /// timing-register values, only a valid handle to have existed before
    /// the first real DMA, matching the rung's actual failure mode being
    /// "reads zero garbage from an unnamed handle," not a timing bug).
    ///
    /// Copies `len` ROM bytes starting at `dev_addr` into `rdram` at
    /// `dram_addr`, matching real cartridge-domain PI DMA's actual effect --
    /// see module doc for why this happens synchronously here even though
    /// the real hardware/ABI models it as async (the completion-message
    /// timing is the caller's job via the returned `DmaCompletion`, not
    /// this function's).
    pub fn start_dma(
        &mut self,
        rdram: &mut crate::rdram::Rdram,
        direction: DmaDirection,
        dram_addr: RdramAddr,
        dev_addr: u32,
        len: u32,
    ) -> DmaCompletion {
        let base = dram_addr.offset() as usize;
        match (direction, is_sram_dev_addr(dev_addr)) {
            // Cartridge-ROM read: big-endian ROM bytes swizzled into rdram's
            // native-word storage (dma_write_bytes does the swizzle).
            (DmaDirection::ToRdram, false) => {
                let mut buf = vec![0u8; len as usize];
                self.rom.read_into(dev_addr, &mut buf);
                rdram.dma_write_bytes(base, &buf);
            }
            // Domain-2 SRAM READ (device -> RDRAM): flat save bytes swizzled
            // into rdram the SAME way as a ROM DMA, because the guest reads the
            // destination via MEM_BU (`^3` XOR) -- see sram_read_into's doc.
            (DmaDirection::ToRdram, true) => {
                let sram_offset = dev_addr - SRAM_DOMAIN2_BASE;
                let mut buf = vec![0u8; len as usize];
                self.sram_read_into(sram_offset, &mut buf);
                rdram.dma_write_bytes(base, &buf);
            }
            // Domain-2 SRAM WRITE (RDRAM -> device): rdram holds native-word-
            // swizzled bytes; un-swizzle back to flat save order (the inverse
            // of dma_write_bytes) before writing the save chip.
            (DmaDirection::FromRdram, true) => {
                assert!(
                    base % 4 == 0 && (len as usize) % 4 == 0,
                    "PI DMA must be word-aligned (dram={base:#x} len={len:#x})"
                );
                let swz = rdram.read_bytes(base, len as usize);
                let mut flat = vec![0u8; len as usize];
                for (i, word) in swz.chunks_exact(4).enumerate() {
                    let o = i * 4;
                    flat[o..o + 4].copy_from_slice(&[word[3], word[2], word[1], word[0]]);
                }
                let sram_offset = dev_addr - SRAM_DOMAIN2_BASE;
                self.sram_write_from(sram_offset, &flat);
            }
            // A FromRdram write to the *cartridge-ROM* domain (not domain-2)
            // is genuinely nonsensical -- ROM is read-only. Keep the loud trap.
            (DmaDirection::FromRdram, false) => {
                unimplemented!(
                    "PiDma::start_dma: FromRdram to the cartridge-ROM domain (devAddr {dev_addr:#x} \
                     < SRAM_DOMAIN2_BASE) -- ROM is read-only. A domain-2 (SRAM/save) write uses \
                     devAddr >= SRAM_DOMAIN2_BASE and routes to the save store; a ROM-domain write \
                     is a caller bug, not a backing-store gap."
                );
            }
        }
        DmaCompletion {
            direction,
            dram_addr,
            dev_addr,
            len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdram::Rdram;

    #[test]
    fn dma_to_rdram_reads_back_through_mem_accessors_unswapped() {
        // The big-endian cartridge word `DE AD BE EF` must read back through
        // the guest's own MEM_* accessors as the SAME word/bytes it was on the
        // cart -- that is the whole contract. rdram is native-endian-WORD
        // storage, so start_dma stores it byte-reversed (`EF BE AD DE`); the
        // test asserts the SEMANTIC outcome (what MEM_W/MEM_BU return), not the
        // raw storage bytes. A flat DMA copy (the OoT-Locale_Init-hanging bug)
        // would make MEM_W read `0xEFBEADDE` and MEM_BU(+2) read `0xAD`.
        let mut rom_bytes = vec![0u8; 0x100];
        rom_bytes[0x10..0x14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut dma = PiDma::new(InMemoryRom::new(rom_bytes));
        let mut rdram = Rdram::new(64);

        let completion = dma.start_dma(
            &mut rdram,
            DmaDirection::ToRdram,
            RdramAddr::from_offset(0x20),
            0x10,
            4,
        );

        assert_eq!(completion.len, 4);
        // MEM_W reads the big-endian cart word back intact.
        assert_eq!(
            rdram.read_w(RdramAddr::from_offset(0x20)) as u32,
            0xDEAD_BEEF
        );
        // MEM_BU(base+k) returns cart byte k (0xDE,0xAD,0xBE,0xEF), NOT 3-k.
        assert_eq!(rdram.read_bu(RdramAddr::from_offset(0x20)), 0xDE);
        assert_eq!(rdram.read_bu(RdramAddr::from_offset(0x22)), 0xBE);
    }

    #[test]
    #[should_panic(expected = "exceeds ROM length")]
    fn out_of_range_read_panics_loudly_not_silently_truncated() {
        let rom = InMemoryRom::new(vec![0u8; 0x10]);
        let mut buf = [0u8; 4];
        rom.read_into(0x20, &mut buf);
    }

    use crate::save::{InMemorySaveStorage, SaveType};

    #[test]
    fn sram_dma_round_trips_through_rdram_with_correct_byte_order() {
        // Write a known pattern to SRAM via a FromRdram (save-write) DMA, then
        // read it back via a ToRdram (save-read) DMA and assert the bytes come
        // back through the guest's MEM_BU/MEM_W accessors in the SAME order --
        // that is the whole contract. The SRAM chip holds FLAT bytes; rdram
        // stores them word-swizzled (native-word), and the swizzle must cancel
        // on the round trip.
        let mut dma = PiDma::new(InMemoryRom::new(vec![0u8; 0x100]));
        dma.set_save(Box::new(InMemorySaveStorage::for_device(
            SaveType::SramBanked,
        )));
        let mut rdram = Rdram::new(0x1000);

        // Guest lays out 8 distinct bytes in rdram at 0x40 the normal way
        // (byte k via MEM_BU write), then DMAs them OUT to SRAM offset 0x10.
        let src = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        for (k, &b) in src.iter().enumerate() {
            rdram.write_bu(RdramAddr::from_offset(0x40 + k as u32), b);
        }
        dma.start_dma(
            &mut rdram,
            DmaDirection::FromRdram,
            RdramAddr::from_offset(0x40),
            SRAM_DOMAIN2_BASE + 0x10,
            8,
        );

        // Zero a DIFFERENT rdram region, DMA the save back IN there, and read
        // it via MEM_BU -- must match the original per-byte order exactly.
        dma.start_dma(
            &mut rdram,
            DmaDirection::ToRdram,
            RdramAddr::from_offset(0x80),
            SRAM_DOMAIN2_BASE + 0x10,
            8,
        );
        for (k, &b) in src.iter().enumerate() {
            assert_eq!(
                rdram.read_bu(RdramAddr::from_offset(0x80 + k as u32)),
                b,
                "SRAM round-trip byte {k} mismatched -- swizzle didn't cancel"
            );
        }
        // And MEM_W over the first word reads the guest's own word intact.
        assert_eq!(
            rdram.read_w(RdramAddr::from_offset(0x80)) as u32,
            rdram.read_w(RdramAddr::from_offset(0x40)) as u32
        );
    }

    #[test]
    fn sram_dma_reads_save_store_not_rom() {
        // A domain-2 devAddr must NOT read the ROM image. Fill ROM and SRAM
        // with distinguishable patterns; a ToRdram at SRAM_DOMAIN2_BASE must
        // deliver the SRAM byte, never the ROM byte at offset 0.
        let mut rom_bytes = vec![0u8; 0x100];
        rom_bytes[0] = 0xAA; // ROM byte at offset 0
        let mut dma = PiDma::new(InMemoryRom::new(rom_bytes));
        let mut save = InMemorySaveStorage::for_device(SaveType::SramBanked);
        save.write_from(0, &[0xC1, 0xC2, 0xC3, 0xC4]);
        dma.set_save(Box::new(save));
        let mut rdram = Rdram::new(0x1000);

        dma.start_dma(
            &mut rdram,
            DmaDirection::ToRdram,
            RdramAddr::from_offset(0x40),
            SRAM_DOMAIN2_BASE, // offset 0 in the SRAM domain
            4,
        );
        // MEM_BU(0x40) is the FIRST SRAM byte (0xC1), not the ROM byte (0xAA).
        assert_eq!(rdram.read_bu(RdramAddr::from_offset(0x40)), 0xC1);
        assert_ne!(rdram.read_bu(RdramAddr::from_offset(0x40)), 0xAA);
    }

    #[test]
    #[should_panic(expected = "no save store is registered")]
    fn sram_dma_without_registered_save_traps_loudly() {
        let mut dma = PiDma::new(InMemoryRom::new(vec![0u8; 0x100]));
        let mut rdram = Rdram::new(0x1000);
        dma.start_dma(
            &mut rdram,
            DmaDirection::ToRdram,
            RdramAddr::from_offset(0x40),
            SRAM_DOMAIN2_BASE,
            4,
        );
    }
}
