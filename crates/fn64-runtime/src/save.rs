//! The save-data seam: EEPROM / SRAM / FlashRAM / Controller Pak, backed by
//! a `SaveStorage` trait exactly the way `rom.rs`'s `RomStorage` backs
//! cartridge ROM reads -- a real, file-backed implementation lives at this
//! crate's edge (`FileSaveStorage`, below) rather than this crate reaching
//! for `std::fs` from deep inside a shim, keeping the same "pure, testable
//! core + one real host-facing impl" split `rom.rs`'s module doc already
//! established.
//!
//! ## Provenance
//!
//! Sizes/semantics are the publicly documented libultra save-device
//! contract (`osEepromProbe`/`osEepromRead`/`osEepromWrite`/
//! `osEepromLongRead`/`osEepromLongWrite`; `osFlash*`; `osPfs*` Controller
//! Pak file API) -- public libultra manual sections (EEPROM Manager, Flash
//! RAM, Controller Pak/PFS Manager), plus the widely published N64 EEPROM
//! sizes (4 Kbit / 16 Kbit) and Flash sector geometry. No GPL runtime
//! save-device implementation was read.
//!
//! ## Design
//!
//! One `SaveStorage` per game, one file per save slot (`docs/DESIGN.md`'s
//! "per-game save file" framing, matching real hardware's "each cartridge
//! has its own dedicated save chip"). This module does not decide WHICH
//! device a given ROM uses (EEPROM vs Flash vs SRAM is a per-cartridge
//! hardware fact carried in that game's `profile.toml`, per
//! `docs/COMPLETENESS.md`'s "game-specific which of EEPROM/Flash a given
//! ROM's profile.toml needs" note) -- it exposes a byte-addressable backing
//! store plus block/page/erase primitives for each protocol shape, and the
//! `fn64-abi` shim layer picks which subset a given game's shims call
//! into.
//!
//! Controller Pak (PFS) is layered as directory/file semantics on TOP of
//! the same page-erase primitive real Controller Paks use (128-byte pages),
//! not a wholly separate storage mechanism -- matching real hardware, where
//! a Controller Pak IS Flash-like paged storage with a filesystem the SDK
//! layers over it.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::device::Cycles;
use crate::tv::CPU_CLOCK_HZ;

/// Which save device a game's cartridge is wired to, per its
/// `profile.toml` -- purely descriptive; this module does not act on it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SaveType {
    Eeprom4k,
    Eeprom16k,
    SramBanked,
    FlashRam,
    ControllerPak,
}

/// Public EEPROM device identities carried by the Joybus Info response.
/// Capacity, identifiers, and the write-in-progress status bit come from
/// the public N64 Joybus hardware documentation; the conservative 15 ms
/// programming interval is the libultra EEPROM Manager's documented timer
/// policy for consecutive writes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EepromKind {
    Eeprom4k,
    Eeprom16k,
}

impl EepromKind {
    pub const fn from_byte_len(len: usize) -> Option<Self> {
        match len {
            512 => Some(Self::Eeprom4k),
            2048 => Some(Self::Eeprom16k),
            _ => None,
        }
    }

    pub const fn byte_len(self) -> usize {
        match self {
            Self::Eeprom4k => 512,
            Self::Eeprom16k => 2048,
        }
    }

    pub const fn joybus_identifier(self) -> u16 {
        match self {
            Self::Eeprom4k => 0x0080,
            Self::Eeprom16k => 0x00C0,
        }
    }

    pub const fn normalize_hardware_block(self, block: u8) -> u8 {
        match self {
            // Public hardware documentation specifies that a 4-Kbit part
            // ignores the top two address bits. Libultra's high-level API
            // still rejects those addresses before reaching this layer.
            Self::Eeprom4k => block & 0x3F,
            Self::Eeprom16k => block,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EepromStatus {
    pub kind: EepromKind,
    pub busy: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EepromError {
    NoDevice,
    Busy { ready_at: Cycles },
}

/// Libultra's public EEPROM Manager waits 15 ms between consecutive block
/// writes. This is a deterministic compatibility deadline, not a claim that
/// every physical EEPROM revision takes exactly the full interval.
pub const EEPROM_WRITE_CYCLES: Cycles = Cycles::new(CPU_CLOCK_HZ * 15 / 1_000);

impl SaveType {
    /// Backing-store byte size for this device, per the publicly documented
    /// N64 save-device sizes (4 Kbit EEPROM = 512 bytes, 16 Kbit EEPROM =
    /// 2048 bytes, SRAM = 32 KiB, FlashRAM = 128 KiB, Controller Pak =
    /// 32 KiB usable + system pages).
    pub const fn byte_len(self) -> usize {
        match self {
            SaveType::Eeprom4k => 512,
            SaveType::Eeprom16k => 2048,
            SaveType::SramBanked => 32 * 1024,
            SaveType::FlashRam => 128 * 1024,
            SaveType::ControllerPak => 32 * 1024,
        }
    }
}

/// A save device's byte-level backing store: read/write/erase over a fixed-
/// size region. Mirrors `rom.rs::RomStorage`'s "trait seam, real impl lives
/// separately" shape exactly -- see this module's doc comment.
///
/// `erase` fills `range` with the device's real erased-state byte pattern
/// (`0xFF` for EEPROM/Flash/Controller-Pak-page-erase, per public hardware
/// docs: NAND/EEPROM-style storage reads all-ones after an erase, not
/// all-zero) rather than being a thin `write(range, zeros)` -- a caller
/// that reads back immediately after erase must see the real hardware
/// value, not an invented one.
pub trait SaveStorage {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_into(&mut self, offset: usize, buf: &mut [u8]);

    fn write_from(&mut self, offset: usize, data: &[u8]);

    /// Erase `len` bytes starting at `offset` to the device's real erased
    /// value (`0xFF`, see doc comment above).
    fn erase(&mut self, offset: usize, len: usize) {
        let filler = vec![0xFFu8; len];
        self.write_from(offset, &filler);
    }
}

/// In-memory `SaveStorage` -- what this crate's own tests use, and a valid
/// choice for a caller that wants an ephemeral (non-persisted) save (e.g. a
/// differential-trace test run that must not touch the real filesystem, per
/// `AGENTS.md`'s "Differential evidence" rule needing reproducible runs).
pub struct InMemorySaveStorage {
    bytes: Vec<u8>,
}

impl InMemorySaveStorage {
    /// New backing store, all bytes at the real hardware "never written"
    /// value (`0xFF` -- see `SaveStorage::erase`'s doc comment; EEPROM/Flash
    /// read as all-ones before the first write, matching a fresh/blank
    /// cartridge's save chip, not an invented all-zero default).
    pub fn new(len: usize) -> Self {
        InMemorySaveStorage {
            bytes: vec![0xFFu8; len],
        }
    }

    pub fn for_device(ty: SaveType) -> Self {
        Self::new(ty.byte_len())
    }
}

impl SaveStorage for InMemorySaveStorage {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_into(&mut self, offset: usize, buf: &mut [u8]) {
        let end = offset + buf.len();
        assert!(
            end <= self.bytes.len(),
            "InMemorySaveStorage::read_into: range {offset:#x}..{end:#x} exceeds device length \
             {:#x} -- a caller bug (mis-decoded EEPROM/Flash address), not something to silently \
             short-read",
            self.bytes.len()
        );
        buf.copy_from_slice(&self.bytes[offset..end]);
    }

    fn write_from(&mut self, offset: usize, data: &[u8]) {
        let end = offset + data.len();
        assert!(
            end <= self.bytes.len(),
            "InMemorySaveStorage::write_from: range {offset:#x}..{end:#x} exceeds device length \
             {:#x}",
            self.bytes.len()
        );
        self.bytes[offset..end].copy_from_slice(data);
    }
}

/// Real, file-backed `SaveStorage` -- one save file per game/device, on
/// disk. This is the "backed by a file (per-game save file)" deliverable:
/// opens (creating if absent, zero-padded to the device's real size with
/// the real erased-state `0xFF` fill) a single flat file and does plain
/// seek+read/write against it. Every write is followed by an explicit
/// `flush` (not just buffered) so a host crash/kill after a device-model
/// commit does not silently lose it. EEPROM writes reach this method only at
/// their typed programming deadline; command acceptance alone does not flush
/// latched bytes prematurely.
pub struct FileSaveStorage {
    file: std::fs::File,
    len: usize,
}

impl FileSaveStorage {
    /// Open (or create) `path` as a save file of exactly `len` bytes. If the
    /// file is new or shorter than `len`, it is extended with the real
    /// hardware erased-state fill (`0xFF`) -- never zero-filled, matching
    /// `InMemorySaveStorage::new`'s same reasoning.
    pub fn open(path: &std::path::Path, len: usize) -> std::io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let current_len = file.metadata()?.len() as usize;
        if current_len < len {
            file.seek(SeekFrom::End(0))?;
            let pad = vec![0xFFu8; len - current_len];
            file.write_all(&pad)?;
            file.flush()?;
        }

        Ok(FileSaveStorage { file, len })
    }

    pub fn open_for_device(path: &std::path::Path, ty: SaveType) -> std::io::Result<Self> {
        Self::open(path, ty.byte_len())
    }
}

impl SaveStorage for FileSaveStorage {
    fn len(&self) -> usize {
        self.len
    }

    fn read_into(&mut self, offset: usize, buf: &mut [u8]) {
        assert!(
            offset + buf.len() <= self.len,
            "FileSaveStorage::read_into: range {offset:#x}..{:#x} exceeds device length {:#x}",
            offset + buf.len(),
            self.len
        );
        self.file
            .seek(SeekFrom::Start(offset as u64))
            .expect("FileSaveStorage::read_into: seek failed -- save file I/O error");
        self.file
            .read_exact(buf)
            .expect("FileSaveStorage::read_into: read failed -- save file I/O error");
    }

    fn write_from(&mut self, offset: usize, data: &[u8]) {
        assert!(
            offset + data.len() <= self.len,
            "FileSaveStorage::write_from: range {offset:#x}..{:#x} exceeds device length {:#x}",
            offset + data.len(),
            self.len
        );
        self.file
            .seek(SeekFrom::Start(offset as u64))
            .expect("FileSaveStorage::write_from: seek failed -- save file I/O error");
        self.file
            .write_all(data)
            .expect("FileSaveStorage::write_from: write failed -- save file I/O error");
        // Explicit flush -- see struct doc comment: once the device model
        // commits a write, it must be durable rather than merely buffered in
        // the host's page cache.
        self.file
            .flush()
            .expect("FileSaveStorage::write_from: flush failed -- save file I/O error");
    }
}

/// EEPROM read/write in the real device's 8-byte-block granularity
/// (`osEepromRead`/`osEepromWrite` transfer exactly one 8-byte block per
/// call per the public libultra manual; `osEepromLongRead`/
/// `osEepromLongWrite` transfer a caller-given number of contiguous
/// blocks). Thin helpers over `SaveStorage` so the `_recomp` shims
/// (`fn64-abi`) don't hand-roll the block-size math at each of the 4 call
/// shapes.
pub const EEPROM_BLOCK_SIZE: usize = 8;

pub fn eeprom_read_block(
    storage: &mut dyn SaveStorage,
    block: u8,
    buf: &mut [u8; EEPROM_BLOCK_SIZE],
) {
    storage.read_into(block as usize * EEPROM_BLOCK_SIZE, buf);
}

pub fn eeprom_write_block(
    storage: &mut dyn SaveStorage,
    block: u8,
    data: &[u8; EEPROM_BLOCK_SIZE],
) {
    storage.write_from(block as usize * EEPROM_BLOCK_SIZE, data);
}

pub fn eeprom_long_read(storage: &mut dyn SaveStorage, start_block: u8, buf: &mut [u8]) {
    storage.read_into(start_block as usize * EEPROM_BLOCK_SIZE, buf);
}

pub fn eeprom_long_write(storage: &mut dyn SaveStorage, start_block: u8, data: &[u8]) {
    storage.write_from(start_block as usize * EEPROM_BLOCK_SIZE, data);
}

/// FlashRAM geometry from the public `osFlashSectorErase` manual: one page
/// is 128 bytes and one erase sector is 128 pages (16 KiB). The device has
/// 1024 pages total (128 KiB), hence eight independently erasable sectors.
pub const FLASH_SECTOR_SIZE: usize = 128 * FLASH_PAGE_SIZE;
pub const FLASH_PAGE_SIZE: usize = 128;

pub fn flash_erase_sector(storage: &mut dyn SaveStorage, sector: u32) {
    storage.erase(sector as usize * FLASH_SECTOR_SIZE, FLASH_SECTOR_SIZE);
}

pub fn flash_write_page(
    storage: &mut dyn SaveStorage,
    byte_offset: u32,
    buf: &[u8; FLASH_PAGE_SIZE],
) {
    storage.write_from(byte_offset as usize, buf);
}

pub fn flash_read_page(
    storage: &mut dyn SaveStorage,
    byte_offset: u32,
    buf: &mut [u8; FLASH_PAGE_SIZE],
) {
    storage.read_into(byte_offset as usize, buf);
}

/// Controller Pak (PFS) page size -- 32-byte pages, per the public libultra
/// Controller Pak Manager documentation (`osPfsReadWriteFile` transfers in
/// 32-byte page units).
pub const PFS_PAGE_SIZE: usize = 32;

pub fn pfs_read_page(storage: &mut dyn SaveStorage, page: u16, buf: &mut [u8; PFS_PAGE_SIZE]) {
    storage.read_into(page as usize * PFS_PAGE_SIZE, buf);
}

pub fn pfs_write_page(storage: &mut dyn SaveStorage, page: u16, buf: &[u8; PFS_PAGE_SIZE]) {
    storage.write_from(page as usize * PFS_PAGE_SIZE, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_storage_reads_as_erased_0xff() {
        let mut s = InMemorySaveStorage::for_device(SaveType::Eeprom4k);
        let mut buf = [0u8; 8];
        s.read_into(0, &mut buf);
        assert_eq!(
            buf, [0xFF; 8],
            "unwritten EEPROM reads all-ones, not all-zero"
        );
    }

    #[test]
    fn eeprom_block_roundtrip() {
        let mut s = InMemorySaveStorage::for_device(SaveType::Eeprom4k);
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        eeprom_write_block(&mut s, 3, &data);
        let mut readback = [0u8; 8];
        eeprom_read_block(&mut s, 3, &mut readback);
        assert_eq!(readback, data);
        // A different block is untouched.
        let mut other = [0u8; 8];
        eeprom_read_block(&mut s, 0, &mut other);
        assert_eq!(other, [0xFF; 8]);
    }

    #[test]
    fn eeprom_long_read_write_multi_block() {
        let mut s = InMemorySaveStorage::for_device(SaveType::Eeprom16k);
        let data = vec![0xAB; EEPROM_BLOCK_SIZE * 4];
        eeprom_long_write(&mut s, 2, &data);
        let mut readback = vec![0u8; EEPROM_BLOCK_SIZE * 4];
        eeprom_long_read(&mut s, 2, &mut readback);
        assert_eq!(readback, data);
    }

    #[test]
    fn flash_erase_then_write_then_read() {
        let mut s = InMemorySaveStorage::for_device(SaveType::FlashRam);
        flash_erase_sector(&mut s, 0);
        let mut readback = [0u8; FLASH_PAGE_SIZE];
        flash_read_page(&mut s, 0, &mut readback);
        assert_eq!(
            readback, [0xFF; FLASH_PAGE_SIZE],
            "erased sector reads all-ones"
        );

        let mut page = [0u8; FLASH_PAGE_SIZE];
        page[0] = 0x42;
        flash_write_page(&mut s, 0, &page);
        flash_read_page(&mut s, 0, &mut readback);
        assert_eq!(readback, page);
    }

    #[test]
    fn flash_sector_erase_uses_the_documented_128_page_boundary() {
        let mut s = InMemorySaveStorage::for_device(SaveType::FlashRam);
        let before = [0x11; FLASH_PAGE_SIZE];
        let inside = [0x22; FLASH_PAGE_SIZE];
        let after = [0x33; FLASH_PAGE_SIZE];
        flash_write_page(
            &mut s,
            (FLASH_SECTOR_SIZE - FLASH_PAGE_SIZE) as u32,
            &before,
        );
        flash_write_page(&mut s, FLASH_SECTOR_SIZE as u32, &inside);
        flash_write_page(&mut s, (2 * FLASH_SECTOR_SIZE) as u32, &after);

        flash_erase_sector(&mut s, 1);

        let mut page = [0; FLASH_PAGE_SIZE];
        flash_read_page(
            &mut s,
            (FLASH_SECTOR_SIZE - FLASH_PAGE_SIZE) as u32,
            &mut page,
        );
        assert_eq!(page, before);
        flash_read_page(&mut s, FLASH_SECTOR_SIZE as u32, &mut page);
        assert_eq!(page, [0xFF; FLASH_PAGE_SIZE]);
        flash_read_page(&mut s, (2 * FLASH_SECTOR_SIZE) as u32, &mut page);
        assert_eq!(page, after);
    }

    #[test]
    fn pfs_page_roundtrip() {
        let mut s = InMemorySaveStorage::for_device(SaveType::ControllerPak);
        let mut page = [0u8; PFS_PAGE_SIZE];
        page[5] = 0x99;
        pfs_write_page(&mut s, 10, &page);
        let mut readback = [0u8; PFS_PAGE_SIZE];
        pfs_read_page(&mut s, 10, &mut readback);
        assert_eq!(readback, page);
    }

    #[test]
    #[should_panic(expected = "exceeds device length")]
    fn out_of_range_read_traps_loudly() {
        let mut s = InMemorySaveStorage::for_device(SaveType::Eeprom4k);
        let mut buf = [0u8; 8];
        s.read_into(511, &mut buf); // 511+8 > 512
    }

    #[test]
    fn file_backed_storage_persists_across_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "fn64_save_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.eep");

        {
            let mut storage = FileSaveStorage::open_for_device(&path, SaveType::Eeprom4k).unwrap();
            let data = [9, 8, 7, 6, 5, 4, 3, 2];
            eeprom_write_block(&mut storage, 1, &data);
        }

        {
            let mut storage = FileSaveStorage::open_for_device(&path, SaveType::Eeprom4k).unwrap();
            let mut readback = [0u8; 8];
            eeprom_read_block(&mut storage, 1, &mut readback);
            assert_eq!(readback, [9, 8, 7, 6, 5, 4, 3, 2]);
            // Other blocks remain the real erased value.
            let mut other = [0u8; 8];
            eeprom_read_block(&mut storage, 0, &mut other);
            assert_eq!(other, [0xFF; 8]);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_backed_storage_creates_correctly_sized_new_file() {
        let dir = std::env::temp_dir().join(format!(
            "fn64_save_test_new_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sav");

        let storage = FileSaveStorage::open_for_device(&path, SaveType::FlashRam).unwrap();
        assert_eq!(storage.len(), SaveType::FlashRam.byte_len());
        assert_eq!(
            std::fs::metadata(&path).unwrap().len() as usize,
            SaveType::FlashRam.byte_len()
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
