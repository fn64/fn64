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
use crate::trace::DmaDirection;

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
}

impl<R: RomStorage> PiDma<R> {
    pub fn new(rom: R) -> Self {
        PiDma { rom }
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
        match direction {
            DmaDirection::ToRdram => {
                let mut buf = vec![0u8; len as usize];
                self.rom.read_into(dev_addr, &mut buf);
                let base = dram_addr.offset() as usize;
                rdram.write_bytes(base, &buf);
            }
            DmaDirection::FromRdram => {
                unimplemented!(
                    "PiDma::start_dma: FromRdram (a write to cartridge domain, e.g. EEPROM/flash) \
                     has no backing store in this milestone -- RomStorage is read-only cartridge \
                     ROM only (see that trait's doc comment). A future osEepromWrite/osFlash* \
                     shim needs a separate writable-backing-store seam, not silently succeeding \
                     as a no-op write here."
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
    fn dma_to_rdram_copies_real_rom_bytes() {
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
        assert_eq!(rdram.read_bytes(0x20, 4), &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    #[should_panic(expected = "exceeds ROM length")]
    fn out_of_range_read_panics_loudly_not_silently_truncated() {
        let rom = InMemoryRom::new(vec![0u8; 0x10]);
        let mut buf = [0u8; 4];
        rom.read_into(0x20, &mut buf);
    }
}
