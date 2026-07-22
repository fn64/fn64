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
//! ## Design: synchronous transfer primitive under a timed device fabric
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
//! `PiDma::start_dma` is the byte-transfer primitive invoked only when a
//! scheduled device event matures. `DeviceFabric` owns PI busy state and the
//! guest-cycle deadline, then calls this primitive while committing bytes,
//! clearing busy, raising MI, and producing a `DmaCompletion` in one ordered
//! transition. The ABI layer posts that completion through the executor's
//! single external-event path before a coroutine can resume. Keeping the
//! primitive here avoids an Executor dependency while no longer pretending
//! that an asynchronous public API completed at its call site.

use crate::device::Cycles;
use crate::rdram::RdramAddr;
use crate::save::{
    EepromError, EepromKind, EepromStatus, SaveOperationEvent, SaveOperationKind, SaveStorage,
    SaveType, EEPROM_BLOCK_SIZE, EEPROM_WRITE_CYCLES,
};
use crate::trace::DmaDirection;

/// The logical-byte operations a PI transfer needs from the process's one
/// RDRAM allocation. Both the owning [`crate::rdram::Rdram`] and a checked
/// borrowed [`crate::rdram::RdramViewMut`] implement this interface, so an
/// ABI adapter never needs to fabricate a second RDRAM object.
pub trait DmaMemory {
    fn dma_write_bytes(&mut self, offset: usize, data: &[u8]);
    fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8>;
}

impl DmaMemory for crate::rdram::Rdram {
    fn dma_write_bytes(&mut self, offset: usize, data: &[u8]) {
        Self::dma_write_bytes(self, offset, data);
    }

    fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8> {
        Self::dma_read_bytes_flat(self, offset, len)
    }
}

impl DmaMemory for crate::rdram::RdramViewMut<'_> {
    fn dma_write_bytes(&mut self, offset: usize, data: &[u8]) {
        Self::dma_write_bytes(self, offset, data);
    }

    fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8> {
        Self::dma_read_bytes_flat(self, offset, len)
    }
}

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
// conversion at the call site. Cartridge ROM is read-only, while domain-2
// SRAM is writable. `try_start_dma` therefore returns a typed rejection for
// a ROM-domain `FromRdram` request and performs save-domain writes through
// the installed `SaveStorage`; callers never need to infer this distinction.

/// The result of a completed PI DMA, carrying exactly what the caller needs
/// to post a completion message through `Executor::inject_event` -- see
/// module doc's "async-looking API" design note. Deliberately does NOT post
/// the message itself (no `Executor` dependency in this crate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaCompletion {
    pub direction: DmaDirection,
    pub dram_addr: RdramAddr,
    pub dev_addr: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiDmaError {
    ReadOnlyDevice { dev_addr: u32 },
}

impl std::fmt::Display for PiDmaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::ReadOnlyDevice { dev_addr } => write!(
                f,
                "PI write targets read-only cartridge ROM at device address {dev_addr:#010x}"
            ),
        }
    }
}

impl std::error::Error for PiDmaError {}

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
    pending_eeprom_write: Option<PendingEepromWrite>,
    /// Successful EEPROM storage actions and timed SRAM DMA commits observed
    /// at their authoritative boundaries. This history is release evidence,
    /// not future device state, so it is intentionally absent from snapshots.
    save_operations: Vec<SaveOperationEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingEepromWrite {
    offset: usize,
    data: [u8; EEPROM_BLOCK_SIZE],
    ready_at: Cycles,
}

/// Immutable view of an EEPROM programming operation that has been accepted
/// but has not reached its guest-cycle completion deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingEepromWriteSnapshot {
    pub offset: u32,
    pub data: [u8; EEPROM_BLOCK_SIZE],
    pub ready_at: Cycles,
}

impl<R: RomStorage> PiDma<R> {
    pub fn new(rom: R) -> Self {
        PiDma {
            rom,
            save: None,
            pending_eeprom_write: None,
            save_operations: Vec::new(),
        }
    }

    /// Register the domain-2 (SRAM/EEPROM/Flash) save-backing store this PI
    /// engine routes `devAddr >= SRAM_DOMAIN2_BASE` DMAs to. Mirrors how the
    /// ROM is installed at construction -- the harness/`fn64-shell` supplies
    /// an `InMemorySaveStorage`/`FileSaveStorage` of the game's save size.
    pub fn set_save(&mut self, save: Box<dyn SaveStorage>) {
        assert!(
            self.pending_eeprom_write.is_none(),
            "PiDma::set_save cannot replace a save store while an EEPROM write is pending"
        );
        self.save = Some(save);
    }

    pub fn has_save(&self) -> bool {
        self.save.is_some()
    }

    /// Installed save-device capacity, used by protocol probes to distinguish
    /// EEPROM types without duplicating ownership of the backing store.
    pub fn save_len(&self) -> Option<usize> {
        self.save.as_deref().map(SaveStorage::len)
    }

    /// Successful EEPROM reads, matured programming operations, and SRAM DMA
    /// commits in exact guest-cycle order. Raw and high-level callers enter
    /// through these primitives, so neither path needs shim-name heuristics.
    pub fn save_operations(&self) -> &[SaveOperationEvent] {
        &self.save_operations
    }

    /// Move completed EEPROM observations to the ABI's unified release log.
    /// Draining at each host boundary preserves same-cycle order relative to
    /// PFS, FlashRAM, and SRAM operations owned above this storage primitive.
    pub fn take_save_operations(&mut self) -> Vec<SaveOperationEvent> {
        std::mem::take(&mut self.save_operations)
    }

    pub(crate) fn record_sram_dma_commit(&mut self, at: Cycles, completion: DmaCompletion) {
        if !is_sram_dev_addr(completion.dev_addr)
            || self.save_len() != Some(SaveType::SramBanked.byte_len())
        {
            return;
        }
        self.save_operations.push(SaveOperationEvent {
            at,
            device: SaveType::SramBanked,
            operation: match completion.direction {
                DmaDirection::ToRdram => SaveOperationKind::Read,
                DmaDirection::FromRdram => SaveOperationKind::Write,
            },
            offset: completion.dev_addr - SRAM_DOMAIN2_BASE,
            len: completion.len,
        });
    }

    /// Copy the complete installed save image without changing device bytes.
    pub fn save_snapshot_bytes(&mut self) -> Option<Vec<u8>> {
        self.save.as_deref_mut().map(SaveStorage::snapshot_bytes)
    }

    pub fn pending_eeprom_write_snapshot(&self) -> Option<PendingEepromWriteSnapshot> {
        self.pending_eeprom_write
            .map(|pending| PendingEepromWriteSnapshot {
                offset: u32::try_from(pending.offset).expect("EEPROM pending offset exceeds u32"),
                data: pending.data,
                ready_at: pending.ready_at,
            })
    }

    fn eeprom_kind(&self) -> Result<EepromKind, EepromError> {
        self.save_len()
            .and_then(EepromKind::from_byte_len)
            .ok_or(EepromError::NoDevice)
    }

    /// Commit a matured EEPROM programming operation. The pending payload is
    /// retained separately from the backing store so reads before the exact
    /// deadline cannot observe bytes hardware has only latched, not written.
    pub fn advance_eeprom_to(&mut self, now: Cycles) {
        let Some(pending) = self.pending_eeprom_write else {
            return;
        };
        if pending.ready_at > now {
            return;
        }
        let kind = self
            .eeprom_kind()
            .expect("a pending EEPROM write exists without an EEPROM store");
        self.pending_eeprom_write = None;
        self.save_mut("EEPROM write completion")
            .write_from(pending.offset, &pending.data);
        self.save_operations.push(SaveOperationEvent {
            at: pending.ready_at,
            device: kind.save_type(),
            operation: SaveOperationKind::Write,
            offset: u32::try_from(pending.offset).expect("EEPROM offset exceeds u32"),
            len: u32::try_from(EEPROM_BLOCK_SIZE).expect("EEPROM block size exceeds u32"),
        });
    }

    pub fn eeprom_status(&mut self, now: Cycles) -> Option<EepromStatus> {
        self.advance_eeprom_to(now);
        let kind = self.eeprom_kind().ok()?;
        Some(EepromStatus {
            kind,
            busy: self.pending_eeprom_write.is_some(),
        })
    }

    pub fn eeprom_busy_until(&mut self, now: Cycles) -> Option<Cycles> {
        self.advance_eeprom_to(now);
        self.pending_eeprom_write.map(|pending| pending.ready_at)
    }

    /// Read one physical Joybus block. A 4-Kbit part ignores the upper two
    /// block-address bits; callers implementing libultra's stricter API range
    /// contract validate before entering this hardware-level operation.
    pub fn eeprom_read_block(
        &mut self,
        now: Cycles,
        block: u8,
    ) -> Result<[u8; EEPROM_BLOCK_SIZE], EepromError> {
        self.advance_eeprom_to(now);
        let kind = self.eeprom_kind()?;
        if let Some(pending) = self.pending_eeprom_write {
            return Err(EepromError::Busy {
                ready_at: pending.ready_at,
            });
        }
        let offset = usize::from(kind.normalize_hardware_block(block)) * EEPROM_BLOCK_SIZE;
        let mut data = [0; EEPROM_BLOCK_SIZE];
        self.save_mut("EEPROM read").read_into(offset, &mut data);
        self.save_operations.push(SaveOperationEvent {
            at: now,
            device: kind.save_type(),
            operation: SaveOperationKind::Read,
            offset: u32::try_from(offset).expect("EEPROM offset exceeds u32"),
            len: u32::try_from(EEPROM_BLOCK_SIZE).expect("EEPROM block size exceeds u32"),
        });
        Ok(data)
    }

    /// Latch one Joybus block and begin background programming. The backing
    /// store changes only when [`Self::advance_eeprom_to`] reaches the returned
    /// deadline. A write attempted while busy is rejected with the same
    /// deadline so the Joybus layer can return its public `0x80` status.
    pub fn start_eeprom_write(
        &mut self,
        now: Cycles,
        block: u8,
        data: [u8; EEPROM_BLOCK_SIZE],
    ) -> Result<Cycles, EepromError> {
        self.advance_eeprom_to(now);
        let kind = self.eeprom_kind()?;
        if let Some(pending) = self.pending_eeprom_write {
            return Err(EepromError::Busy {
                ready_at: pending.ready_at,
            });
        }
        let ready_at = now
            .checked_add(EEPROM_WRITE_CYCLES)
            .expect("EEPROM write deadline overflows guest cycle clock");
        self.pending_eeprom_write = Some(PendingEepromWrite {
            offset: usize::from(kind.normalize_hardware_block(block)) * EEPROM_BLOCK_SIZE,
            data,
            ready_at,
        });
        Ok(ready_at)
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
        self.save_read_into(sram_offset as usize, buf);
    }

    /// Write FLAT `data` bytes to the save chip at `sram_offset` (already
    /// `devAddr - SRAM_DOMAIN2_BASE`). The caller un-swizzles rdram's
    /// native-word bytes back to flat save order first (see
    /// `osEPiStartDma_recomp`'s FromRdram arm).
    pub fn sram_write_from(&mut self, sram_offset: u32, data: &[u8]) {
        self.save_write_from(sram_offset as usize, data);
    }

    /// Protocol-neutral save read. EEPROM, FlashRAM, Controller Pak, and PI
    /// domain-2 SRAM all converge on the one installed backing store.
    pub fn save_read_into(&mut self, offset: usize, buf: &mut [u8]) {
        self.save_mut("read").read_into(offset, buf);
    }

    /// Protocol-neutral save write; see [`Self::save_read_into`].
    pub fn save_write_from(&mut self, offset: usize, data: &[u8]) {
        assert!(
            self.pending_eeprom_write.is_none(),
            "protocol-neutral save write cannot bypass a pending EEPROM programming operation"
        );
        self.save_mut("write").write_from(offset, data);
    }

    /// Protocol-neutral save erase, preserving the backing store's erased
    /// byte value and durability policy.
    pub fn save_erase(&mut self, offset: usize, len: usize) {
        assert!(
            self.pending_eeprom_write.is_none(),
            "protocol-neutral save erase cannot bypass a pending EEPROM programming operation"
        );
        self.save_mut("erase").erase(offset, len);
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
    /// caller (`fn64-abi`'s shim) has decoded the `OSIoMesg`, formed the
    /// public `handle->baseAddress | devAddr`, normalized the supported
    /// Game Pak/SRAM physical spaces into this engine's address convention,
    /// and applied the handle timing to `DeviceFabric`'s raw PI registers.
    /// Keeping handle parsing above this storage primitive lets managed/raw
    /// EPI and programmed I/O share one authority without teaching the
    /// runtime core about guest C-struct layout.
    ///
    /// Copies `len` ROM bytes starting at `dev_addr` into `rdram` at
    /// `dram_addr`, matching real cartridge-domain PI DMA's actual effect --
    /// This primitive runs synchronously only after `DeviceFabric` reaches
    /// the scheduled deadline; callers must not use it as the public start
    /// operation.
    pub fn start_dma<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &mut M,
        direction: DmaDirection,
        dram_addr: RdramAddr,
        dev_addr: u32,
        len: u32,
    ) -> DmaCompletion {
        self.try_start_dma(rdram, direction, dram_addr, dev_addr, len)
            .unwrap_or_else(|error| panic!("PiDma::start_dma: {error}"))
    }

    /// Typed counterpart to [`Self::start_dma`]. Device fabrics use this so
    /// a guest-visible PI rejection cannot escape as a host-language panic.
    pub fn try_start_dma<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &mut M,
        direction: DmaDirection,
        dram_addr: RdramAddr,
        dev_addr: u32,
        len: u32,
    ) -> Result<DmaCompletion, PiDmaError> {
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
                // Un-swizzle native-word rdram back to flat save order via the
                // per-byte inverse (`flat[k] = rdram[(base+k)^3]`), correct for
                // any offset/length -- no word-alignment requirement.
                let flat = rdram.dma_read_bytes_flat(base, len as usize);
                let sram_offset = dev_addr - SRAM_DOMAIN2_BASE;
                self.sram_write_from(sram_offset, &flat);
            }
            (DmaDirection::FromRdram, false) => {
                return Err(PiDmaError::ReadOnlyDevice { dev_addr });
            }
        }
        Ok(DmaCompletion {
            direction,
            dram_addr,
            dev_addr,
            len,
        })
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
    fn dma_to_rdram_handles_non_word_aligned_length() {
        // OoT's DmaMgr_DmaRomToRam issues sub-word-length DMAs (e.g. len=0x86);
        // the per-byte swizzle must place EVERY byte on its correct MEM_BU lane,
        // including the trailing partial word. A word-chunk-only loop (the old
        // code) would panic on the alignment assert or drop the tail bytes.
        let mut rom_bytes = vec![0u8; 0x100];
        // 6 distinguishable bytes at ROM 0x10 (1.5 words -- crosses a word edge).
        let src = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        rom_bytes[0x10..0x16].copy_from_slice(&src);
        let mut dma = PiDma::new(InMemoryRom::new(rom_bytes));
        let mut rdram = Rdram::new(64);

        dma.start_dma(
            &mut rdram,
            DmaDirection::ToRdram,
            RdramAddr::from_offset(0x20),
            0x10,
            6, // NON-word-aligned length
        );

        // The guest reads each byte via MEM_BU(base+k); every one must recover
        // the original cart byte k, including the two tail bytes (k=4,5).
        for (k, &b) in src.iter().enumerate() {
            assert_eq!(
                rdram.read_bu(RdramAddr::from_offset(0x20 + k as u32)),
                b,
                "MEM_BU(base+{k}) should recover cart byte {b:#x}"
            );
        }
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
    fn eeprom_write_commits_at_exact_typed_deadline_and_rejects_overlap() {
        let mut dma = PiDma::new(InMemoryRom::new(vec![]));
        dma.set_save(Box::new(InMemorySaveStorage::for_device(
            SaveType::Eeprom4k,
        )));
        let start = Cycles::new(10);
        let first = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let deadline = dma.start_eeprom_write(start, 3, first).unwrap();
        assert_eq!(deadline, start.checked_add(EEPROM_WRITE_CYCLES).unwrap());
        assert_eq!(
            dma.eeprom_status(start),
            Some(EepromStatus {
                kind: EepromKind::Eeprom4k,
                busy: true,
            })
        );

        let second = [0xA5; EEPROM_BLOCK_SIZE];
        assert_eq!(
            dma.start_eeprom_write(Cycles::new(deadline.get() - 1), 4, second),
            Err(EepromError::Busy { ready_at: deadline })
        );
        let bypass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dma.save_write_from(0, &[0]);
        }))
        .expect_err("protocol-neutral write must not bypass EEPROM busy state");
        let bypass_message = bypass
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| bypass.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(bypass_message.contains("cannot bypass a pending EEPROM"));
        let replacement = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dma.set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        }))
        .expect_err("save replacement must not discard EEPROM busy state");
        let replacement_message = replacement
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| replacement.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(replacement_message.contains("while an EEPROM write is pending"));
        let mut physical = [0; EEPROM_BLOCK_SIZE];
        dma.save_read_into(3 * EEPROM_BLOCK_SIZE, &mut physical);
        assert_eq!(physical, [0xFF; EEPROM_BLOCK_SIZE]);

        dma.advance_eeprom_to(Cycles::new(deadline.get() - 1));
        dma.save_read_into(3 * EEPROM_BLOCK_SIZE, &mut physical);
        assert_eq!(physical, [0xFF; EEPROM_BLOCK_SIZE]);
        assert!(dma.save_operations().is_empty());
        dma.advance_eeprom_to(deadline);
        dma.save_read_into(3 * EEPROM_BLOCK_SIZE, &mut physical);
        assert_eq!(physical, first);
        assert_eq!(dma.eeprom_busy_until(deadline), None);
        assert_eq!(
            dma.save_operations(),
            &[SaveOperationEvent {
                at: deadline,
                device: SaveType::Eeprom4k,
                operation: SaveOperationKind::Write,
                offset: 3 * EEPROM_BLOCK_SIZE as u32,
                len: EEPROM_BLOCK_SIZE as u32,
            }]
        );
    }

    #[test]
    fn physical_4k_eeprom_ignores_top_two_block_address_bits() {
        let mut dma = PiDma::new(InMemoryRom::new(vec![]));
        dma.set_save(Box::new(InMemorySaveStorage::for_device(
            SaveType::Eeprom4k,
        )));
        let data = [0x5A; EEPROM_BLOCK_SIZE];
        let deadline = dma.start_eeprom_write(Cycles::ZERO, 0xC2, data).unwrap();
        dma.advance_eeprom_to(deadline);
        assert_eq!(dma.eeprom_read_block(deadline, 0x02).unwrap(), data);
        assert_eq!(
            dma.save_operations(),
            &[
                SaveOperationEvent {
                    at: deadline,
                    device: SaveType::Eeprom4k,
                    operation: SaveOperationKind::Write,
                    offset: 2 * EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
                SaveOperationEvent {
                    at: deadline,
                    device: SaveType::Eeprom4k,
                    operation: SaveOperationKind::Read,
                    offset: 2 * EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
            ]
        );
    }

    #[test]
    fn replacing_an_idle_save_store_preserves_append_only_release_history() {
        let mut dma = PiDma::new(InMemoryRom::new(vec![]));
        dma.set_save(Box::new(InMemorySaveStorage::for_device(
            SaveType::Eeprom4k,
        )));
        let first_deadline = dma
            .start_eeprom_write(Cycles::new(7), 1, [0x3c; EEPROM_BLOCK_SIZE])
            .unwrap();
        dma.advance_eeprom_to(first_deadline);

        dma.set_save(Box::new(InMemorySaveStorage::for_device(
            SaveType::Eeprom16k,
        )));
        let second_deadline = dma
            .start_eeprom_write(first_deadline, 2, [0xa5; EEPROM_BLOCK_SIZE])
            .unwrap();
        dma.advance_eeprom_to(second_deadline);

        assert_eq!(
            dma.save_operations(),
            &[
                SaveOperationEvent {
                    at: first_deadline,
                    device: SaveType::Eeprom4k,
                    operation: SaveOperationKind::Write,
                    offset: EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
                SaveOperationEvent {
                    at: second_deadline,
                    device: SaveType::Eeprom16k,
                    operation: SaveOperationKind::Write,
                    offset: 2 * EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
            ]
        );
    }

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
    fn cartridge_write_is_a_typed_device_error() {
        let mut dma = PiDma::new(InMemoryRom::new(vec![0u8; 0x100]));
        let mut rdram = Rdram::new(0x100);
        assert_eq!(
            dma.try_start_dma(
                &mut rdram,
                DmaDirection::FromRdram,
                RdramAddr::from_offset(0x20),
                0x10,
                4,
            ),
            Err(PiDmaError::ReadOnlyDevice { dev_addr: 0x10 })
        );
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
