//! Controller Pak file system over one authoritative physical image.
//!
//! Provenance: the public libultra Controller Pak manuals define 256-byte
//! pages, sixteen directory entries, 32-byte I/O granularity, note metadata/
//! search rules, and `PFS_ERR_INCONSISTENT` for a damaged management area.
//! The public N64brew Controller Pak and filesystem hardware documentation
//! (filesystem revision 5639) defines the physical image encoding used here:
//! byte 0x1a of the ID block is the 32 KiB bank count; each bank contributes
//! one checksum-protected FAT page plus one backup; the directory follows the
//! FAT copies; each FAT page describes 128 physical pages but its first entry
//! is occupied by the checksum; and FAT values 0/1/3/>=5 mean reserved/end/
//! free/next. Writes at 0x8000..=0xffff select a bank and reads there return
//! zero. Both high-level file operations and raw Joybus block I/O therefore
//! observe and mutate this same physical image.

pub const PFS_PAGE_SIZE: usize = 256;
pub const PFS_BLOCK_SIZE: usize = 32;
pub const PFS_PAGES_PER_BANK: usize = 128;
pub const PFS_BANK_CAPACITY: usize = PFS_PAGE_SIZE * PFS_PAGES_PER_BANK;
pub const PFS_MAX_BANKS: u8 = 62;
/// Standard one-bank geometry retained for callers which need the original
/// public 32 KiB constants.
pub const PFS_TOTAL_PAGES: usize = 128;
pub const PFS_MANAGEMENT_PAGES: usize = 5;
pub const PFS_DATA_PAGES: usize = 123;
pub const PFS_CAPACITY: usize = PFS_PAGE_SIZE * PFS_DATA_PAGES;
pub const PFS_RAW_CAPACITY: usize = PFS_PAGE_SIZE * PFS_TOTAL_PAGES;
pub const PFS_MAX_FILES: usize = 16;

const PFS_ID_PRIMARY_OFFSET: usize = 0x20;
const PFS_ID_BACKUP_OFFSETS: [usize; 3] = [0x60, 0x80, 0xc0];
const PFS_FAT_PRIMARY_PAGE: usize = 1;
#[cfg(test)]
const PFS_FAT_BACKUP_PAGE: usize = 2;
const PFS_DIRECTORY_ENTRY_SIZE: usize = 32;
const PFS_NOTE_STATUS_OCCUPIED: u8 = 0x02;
const PFS_FAT_END: u16 = 1;
const PFS_FAT_FREE: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PfsKey {
    pub company_code: u16,
    pub game_code: u32,
    pub game_name: [u8; 16],
    pub ext_name: [u8; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PfsState {
    pub file_size: u32,
    pub key: PfsKey,
}

/// One decoded note in the release-evidence projection. This is derived from
/// the physical FAT and directory rather than retained as a second authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PfsNoteEvidenceSnapshot {
    pub key: PfsKey,
    pub pages: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerPakEvidenceSnapshot {
    pub bank_count: u8,
    pub active_bank: u8,
    pub notes: [Option<PfsNoteEvidenceSnapshot>; PFS_MAX_FILES],
    pub raw: Vec<u8>,
}

/// Validated physical capacity for one linear bank-switched Controller Pak.
/// The public filesystem format stores this value in one byte, while the
/// published SDK geometry caps it at 62 banks so all management pages fit in
/// bank zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerPakBankCount(u8);

impl ControllerPakBankCount {
    pub const STANDARD: Self = Self(1);

    pub const fn new(value: u8) -> Option<Self> {
        if value >= 1 && value <= PFS_MAX_BANKS {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PfsError {
    Invalid,
    Inconsistent,
    DataFull,
    DirectoryFull,
    Exists,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Note {
    key: PfsKey,
    pages: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct ControllerPak {
    raw: Box<[u8]>,
    bank_count: ControllerPakBankCount,
    active_bank: u8,
}

impl Default for ControllerPak {
    fn default() -> Self {
        Self::with_bank_count(ControllerPakBankCount::STANDARD)
    }
}

impl ControllerPak {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bank_count(bank_count: ControllerPakBankCount) -> Self {
        let mut raw =
            vec![0xff; usize::from(bank_count.get()) * PFS_BANK_CAPACITY].into_boxed_slice();
        format_image(&mut raw, bank_count);
        Self {
            raw,
            bank_count,
            active_bank: 0,
        }
    }

    pub const fn bank_count(&self) -> ControllerPakBankCount {
        self.bank_count
    }

    pub const fn active_bank(&self) -> u8 {
        self.active_bank
    }

    pub fn evidence_snapshot(&self) -> ControllerPakEvidenceSnapshot {
        let notes = self.decode_notes().unwrap_or_else(|error| {
            panic!(
                "Controller Pak release evidence encountered damaged management pages: {error:?}"
            )
        });
        ControllerPakEvidenceSnapshot {
            bank_count: self.bank_count.get(),
            active_bank: self.active_bank,
            notes: notes.map(|note| {
                note.map(|note| PfsNoteEvidenceSnapshot {
                    key: note.key,
                    pages: note.pages,
                })
            }),
            raw: self.raw.to_vec(),
        }
    }

    /// Validate the management area and perform only the repair that
    /// the public redundant-FAT contract determines unambiguously: if exactly
    /// one FAT copy has a valid checksum, reproduce it into the damaged copy.
    /// Two valid-but-different copies or semantic chain damage are ambiguous
    /// and remain `Inconsistent` rather than selecting or inventing data.
    pub fn check_and_repair(&mut self) -> Result<(), PfsError> {
        let bank_count = usize::from(self.bank_count.get());
        let mut fat = vec![0u16; self.total_pages()];
        let mut repairs = Vec::new();
        for bank in 0..bank_count {
            let primary_page = PFS_FAT_PRIMARY_PAGE + bank;
            let backup_page = PFS_FAT_PRIMARY_PAGE + bank_count + bank;
            let primary = decode_fat_page(&self.raw, primary_page);
            let backup = decode_fat_page(&self.raw, backup_page);
            let (entries, repair) = match (primary, backup) {
                (Ok(primary), Ok(backup)) if primary == backup => (primary, None),
                (Ok(_), Ok(_)) => return Err(PfsError::Inconsistent),
                (Ok(primary), Err(())) => (primary, Some((primary_page, backup_page))),
                (Err(()), Ok(backup)) => (backup, Some((backup_page, primary_page))),
                (Err(()), Err(())) => return Err(PfsError::Inconsistent),
            };
            let start = bank * PFS_PAGES_PER_BANK;
            fat[start..start + PFS_PAGES_PER_BANK].copy_from_slice(&entries);
            repairs.extend(repair);
        }
        self.decode_notes_from_fat(&fat)?;
        for (source, destination) in repairs {
            self.copy_fat_page(source, destination);
        }
        Ok(())
    }

    pub fn free_bytes(&self) -> Result<usize, PfsError> {
        self.decode_notes()?;
        let fat = self.decode_fat()?;
        Ok(fat
            .iter()
            .enumerate()
            .filter(|&(page, &entry)| self.is_data_page(page) && entry == PFS_FAT_FREE)
            .count()
            * PFS_PAGE_SIZE)
    }

    pub fn allocate(&mut self, key: PfsKey, requested_len: usize) -> Result<usize, PfsError> {
        if requested_len == 0 {
            return Err(PfsError::Invalid);
        }
        let notes = self.decode_notes()?;
        if notes.iter().flatten().any(|existing| existing.key == key) {
            return Err(PfsError::Exists);
        }
        let Some(slot) = notes.iter().position(Option::is_none) else {
            return Err(PfsError::DirectoryFull);
        };
        let rounded = requested_len
            .checked_add(PFS_PAGE_SIZE - 1)
            .ok_or(PfsError::Invalid)?
            / PFS_PAGE_SIZE
            * PFS_PAGE_SIZE;
        let page_count = rounded / PFS_PAGE_SIZE;
        let mut fat = self.decode_fat()?;
        let pages = fat
            .iter()
            .enumerate()
            .filter_map(|(page, &entry)| {
                (self.is_data_page(page) && entry == PFS_FAT_FREE).then_some(page as u16)
            })
            .take(page_count)
            .collect::<Vec<_>>();
        if pages.len() != page_count {
            return Err(PfsError::DataFull);
        }
        for (index, &page) in pages.iter().enumerate() {
            fat[usize::from(page)] = pages.get(index + 1).map_or(PFS_FAT_END, |next| *next);
            let start = usize::from(page) * PFS_PAGE_SIZE;
            self.raw[start..start + PFS_PAGE_SIZE].fill(0);
        }
        self.write_fat_copies(&fat);
        self.write_directory_entry(slot, key, pages[0]);
        Ok(slot)
    }

    pub fn find(&self, key: PfsKey) -> Result<usize, PfsError> {
        self.decode_notes()?
            .iter()
            .position(|note| note.as_ref().is_some_and(|note| note.key == key))
            .ok_or(PfsError::Invalid)
    }

    pub fn delete(&mut self, key: PfsKey) -> Result<(), PfsError> {
        let notes = self.decode_notes()?;
        let slot = notes
            .iter()
            .position(|note| note.as_ref().is_some_and(|note| note.key == key))
            .ok_or(PfsError::Invalid)?;
        let pages = notes[slot]
            .as_ref()
            .expect("found Controller Pak note disappeared")
            .pages
            .clone();
        let mut fat = self.decode_fat()?;
        for page in pages {
            fat[usize::from(page)] = PFS_FAT_FREE;
        }
        self.write_fat_copies(&fat);
        let start = self.directory_entry_offset(slot);
        self.raw[start..start + PFS_DIRECTORY_ENTRY_SIZE].fill(0);
        Ok(())
    }

    pub fn state(&self, file_no: usize) -> Result<PfsState, PfsError> {
        let notes = self.decode_notes()?;
        let note = notes
            .get(file_no)
            .and_then(Option::as_ref)
            .ok_or(PfsError::Invalid)?;
        Ok(PfsState {
            file_size: u32::try_from(note.pages.len() * PFS_PAGE_SIZE)
                .expect("Controller Pak note exceeds u32"),
            key: note.key,
        })
    }

    pub fn read(
        &self,
        file_no: usize,
        offset: usize,
        destination: &mut [u8],
    ) -> Result<(), PfsError> {
        let notes = self.decode_notes()?;
        let note = notes
            .get(file_no)
            .and_then(Option::as_ref)
            .ok_or(PfsError::Invalid)?;
        validate_io_range(note.pages.len() * PFS_PAGE_SIZE, offset, destination.len())?;
        copy_note_bytes(&self.raw[..], &note.pages, offset, destination);
        Ok(())
    }

    pub fn write(&mut self, file_no: usize, offset: usize, source: &[u8]) -> Result<(), PfsError> {
        let notes = self.decode_notes()?;
        let note = notes
            .get(file_no)
            .and_then(Option::as_ref)
            .ok_or(PfsError::Invalid)?;
        validate_io_range(note.pages.len() * PFS_PAGE_SIZE, offset, source.len())?;
        write_note_bytes(&mut self.raw[..], &note.pages, offset, source);
        Ok(())
    }

    /// Read one 32-byte Controller Pak block. The public Joybus protocol maps
    /// 0x0000..=0x7fff through the active 32 KiB bank; the upper half reads as
    /// zero and is reserved for bank switching writes.
    pub fn raw_read_block(
        &self,
        address: usize,
        destination: &mut [u8; PFS_BLOCK_SIZE],
    ) -> Result<(), PfsError> {
        validate_raw_block(address)?;
        if address >= PFS_BANK_CAPACITY {
            destination.fill(0);
            return Ok(());
        }
        let physical = usize::from(self.active_bank) * PFS_BANK_CAPACITY + address;
        destination.copy_from_slice(&self.raw[physical..physical + PFS_BLOCK_SIZE]);
        Ok(())
    }

    /// Write one physical 32-byte Controller Pak block. Management writes are
    /// accepted as physical transactions; the next semantic operation
    /// validates the complete image and reports inconsistent intermediate or
    /// corrupt metadata rather than consulting a stale shadow directory.
    pub fn raw_write_block(
        &mut self,
        address: usize,
        source: &[u8; PFS_BLOCK_SIZE],
    ) -> Result<(), PfsError> {
        validate_raw_block(address)?;
        if address >= PFS_BANK_CAPACITY {
            // Public traces establish uniform 32-byte accessory writes, while
            // the hardware description only names one written bank value. A
            // nonuniform payload therefore has no clean-room-supported latch
            // interpretation.
            if !source.iter().all(|&byte| byte == source[0]) {
                return Err(PfsError::Invalid);
            }
            let requested = source[0];
            let count = self.bank_count.get();
            self.active_bank = if requested < count {
                requested
            } else if count.is_power_of_two() {
                // N64brew's observed four-bank example maps 5 to 1 and states
                // that the written value's low bits select the bank.
                requested & (count - 1)
            } else {
                // Mirroring for nonexistent banks is explicitly described as
                // incompletely investigated; do not invent an odd-size map.
                return Err(PfsError::Invalid);
            };
            return Ok(());
        }
        let physical = usize::from(self.active_bank) * PFS_BANK_CAPACITY + address;
        self.raw[physical..physical + PFS_BLOCK_SIZE].copy_from_slice(source);
        Ok(())
    }

    fn total_pages(&self) -> usize {
        usize::from(self.bank_count.get()) * PFS_PAGES_PER_BANK
    }

    fn management_pages(&self) -> usize {
        3 + 2 * usize::from(self.bank_count.get())
    }

    fn directory_offset(&self) -> usize {
        (1 + 2 * usize::from(self.bank_count.get())) * PFS_PAGE_SIZE
    }

    fn is_data_page(&self, page: usize) -> bool {
        page < self.total_pages()
            && page >= self.management_pages()
            && !page.is_multiple_of(PFS_PAGES_PER_BANK)
    }

    fn decode_fat(&self) -> Result<Vec<u16>, PfsError> {
        let bank_count = usize::from(self.bank_count.get());
        let primary = decode_fat_copy(&self.raw, PFS_FAT_PRIMARY_PAGE, bank_count)
            .map_err(|()| PfsError::Inconsistent)?;
        let backup = decode_fat_copy(&self.raw, PFS_FAT_PRIMARY_PAGE + bank_count, bank_count)
            .map_err(|()| PfsError::Inconsistent)?;
        (primary == backup)
            .then_some(primary)
            .ok_or(PfsError::Inconsistent)
    }

    fn decode_notes(&self) -> Result<[Option<Note>; PFS_MAX_FILES], PfsError> {
        let fat = self.decode_fat()?;
        self.decode_notes_from_fat(&fat)
    }

    fn decode_notes_from_fat(
        &self,
        fat: &[u16],
    ) -> Result<[Option<Note>; PFS_MAX_FILES], PfsError> {
        if fat.len() != self.total_pages() {
            return Err(PfsError::Inconsistent);
        }

        let mut notes: [Option<Note>; PFS_MAX_FILES] = std::array::from_fn(|_| None);
        let mut owned = vec![false; self.total_pages()];
        for slot in 0..PFS_MAX_FILES {
            let start = self.directory_entry_offset(slot);
            let entry = &self.raw[start..start + PFS_DIRECTORY_ENTRY_SIZE];
            if entry.iter().all(|&byte| byte == 0) {
                continue;
            }
            if entry[8] & PFS_NOTE_STATUS_OCCUPIED == 0 {
                return Err(PfsError::Inconsistent);
            }
            let game_code = u32::from_be_bytes(entry[0..4].try_into().unwrap());
            let company_code = u16::from_be_bytes(entry[4..6].try_into().unwrap());
            if game_code == 0 || company_code == 0 {
                return Err(PfsError::Inconsistent);
            }
            let first_page = u16::from_be_bytes(entry[6..8].try_into().unwrap());
            let pages = self.decode_chain(fat, first_page, &mut owned)?;
            let key = PfsKey {
                company_code,
                game_code,
                ext_name: entry[0x0c..0x10].try_into().unwrap(),
                game_name: entry[0x10..0x20].try_into().unwrap(),
            };
            if notes[..slot].iter().flatten().any(|note| note.key == key) {
                return Err(PfsError::Inconsistent);
            }
            notes[slot] = Some(Note { key, pages });
        }
        for page in 0..self.total_pages() {
            if !self.is_data_page(page) {
                if fat[page] != 0 {
                    return Err(PfsError::Inconsistent);
                }
            } else if owned[page] {
                let entry = usize::from(fat[page]);
                if fat[page] != PFS_FAT_END && !self.is_data_page(entry) {
                    return Err(PfsError::Inconsistent);
                }
            } else if fat[page] != PFS_FAT_FREE {
                return Err(PfsError::Inconsistent);
            }
        }
        Ok(notes)
    }

    fn write_fat_copies(&mut self, fat: &[u16]) {
        let bank_count = usize::from(self.bank_count.get());
        encode_fat_copy(&mut self.raw, PFS_FAT_PRIMARY_PAGE, bank_count, fat);
        encode_fat_copy(
            &mut self.raw,
            PFS_FAT_PRIMARY_PAGE + bank_count,
            bank_count,
            fat,
        );
    }

    fn copy_fat_page(&mut self, source_page: usize, destination_page: usize) {
        let source = source_page * PFS_PAGE_SIZE;
        let destination = destination_page * PFS_PAGE_SIZE;
        let bytes: [u8; PFS_PAGE_SIZE] = self.raw[source..source + PFS_PAGE_SIZE]
            .try_into()
            .expect("fixed Controller Pak FAT page");
        self.raw[destination..destination + PFS_PAGE_SIZE].copy_from_slice(&bytes);
    }

    fn directory_entry_offset(&self, slot: usize) -> usize {
        self.directory_offset() + slot * PFS_DIRECTORY_ENTRY_SIZE
    }

    fn write_directory_entry(&mut self, slot: usize, key: PfsKey, first_page: u16) {
        let start = self.directory_entry_offset(slot);
        let entry = &mut self.raw[start..start + PFS_DIRECTORY_ENTRY_SIZE];
        entry.fill(0);
        entry[0..4].copy_from_slice(&key.game_code.to_be_bytes());
        entry[4..6].copy_from_slice(&key.company_code.to_be_bytes());
        entry[6..8].copy_from_slice(&first_page.to_be_bytes());
        entry[8] = PFS_NOTE_STATUS_OCCUPIED;
        entry[0x0c..0x10].copy_from_slice(&key.ext_name);
        entry[0x10..0x20].copy_from_slice(&key.game_name);
    }

    fn decode_chain(
        &self,
        fat: &[u16],
        first_page: u16,
        owned: &mut [bool],
    ) -> Result<Vec<u16>, PfsError> {
        let mut page = usize::from(first_page);
        let mut local = vec![false; self.total_pages()];
        let mut pages = Vec::new();
        loop {
            if !self.is_data_page(page) || local[page] || owned[page] {
                return Err(PfsError::Inconsistent);
            }
            local[page] = true;
            owned[page] = true;
            pages.push(page as u16);
            match fat[page] {
                PFS_FAT_END => return Ok(pages),
                next => {
                    let next = usize::from(next);
                    if !self.is_data_page(next) {
                        return Err(PfsError::Inconsistent);
                    }
                    page = next;
                }
            }
        }
    }
}

fn format_image(raw: &mut [u8], bank_count: ControllerPakBankCount) {
    raw[..PFS_BLOCK_SIZE].fill(0);
    let mut id = [0u8; PFS_BLOCK_SIZE];
    id[0x19] = 1;
    id[0x1a] = bank_count.get();
    let checksum = id[..0x1c]
        .chunks_exact(2)
        .map(|word| u16::from_be_bytes([word[0], word[1]]))
        .fold(0u16, u16::wrapping_add);
    id[0x1c..0x1e].copy_from_slice(&checksum.to_be_bytes());
    id[0x1e..0x20].copy_from_slice(&0xfff2u16.wrapping_sub(checksum).to_be_bytes());
    raw[PFS_ID_PRIMARY_OFFSET..PFS_ID_PRIMARY_OFFSET + PFS_BLOCK_SIZE].copy_from_slice(&id);
    for offset in PFS_ID_BACKUP_OFFSETS {
        raw[offset..offset + PFS_BLOCK_SIZE].copy_from_slice(&id);
    }

    let banks = usize::from(bank_count.get());
    let total_pages = banks * PFS_PAGES_PER_BANK;
    let management_pages = 3 + 2 * banks;
    let mut fat = vec![PFS_FAT_FREE; total_pages];
    fat[..management_pages].fill(0);
    for bank in 1..banks {
        fat[bank * PFS_PAGES_PER_BANK] = 0;
    }
    encode_fat_copy(raw, PFS_FAT_PRIMARY_PAGE, banks, &fat);
    encode_fat_copy(raw, PFS_FAT_PRIMARY_PAGE + banks, banks, &fat);
    let directory_offset = (1 + 2 * banks) * PFS_PAGE_SIZE;
    raw[directory_offset..management_pages * PFS_PAGE_SIZE].fill(0);
}

fn decode_fat_copy(raw: &[u8], first_page: usize, bank_count: usize) -> Result<Vec<u16>, ()> {
    let mut fat = vec![0u16; bank_count * PFS_PAGES_PER_BANK];
    for bank in 0..bank_count {
        let entries = decode_fat_page(raw, first_page + bank)?;
        let start = bank * PFS_PAGES_PER_BANK;
        fat[start..start + PFS_PAGES_PER_BANK].copy_from_slice(&entries);
    }
    Ok(fat)
}

fn decode_fat_page(raw: &[u8], page: usize) -> Result<[u16; PFS_PAGES_PER_BANK], ()> {
    let start = page * PFS_PAGE_SIZE;
    let bytes = &raw[start..start + PFS_PAGE_SIZE];
    if bytes[0] != 0 || bytes[1] != bytes[2..].iter().copied().fold(0u8, u8::wrapping_add) {
        return Err(());
    }
    let mut fat = [0u16; PFS_PAGES_PER_BANK];
    for (page, entry) in fat.iter_mut().enumerate().skip(1) {
        let offset = page * 2;
        *entry = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    }
    Ok(fat)
}

fn encode_fat_copy(raw: &mut [u8], first_page: usize, bank_count: usize, fat: &[u16]) {
    assert_eq!(fat.len(), bank_count * PFS_PAGES_PER_BANK);
    for bank in 0..bank_count {
        let start = (first_page + bank) * PFS_PAGE_SIZE;
        let bytes = &mut raw[start..start + PFS_PAGE_SIZE];
        bytes.fill(0);
        let fat_start = bank * PFS_PAGES_PER_BANK;
        for local_page in 1..PFS_PAGES_PER_BANK {
            let offset = local_page * 2;
            bytes[offset..offset + 2].copy_from_slice(&fat[fat_start + local_page].to_be_bytes());
        }
        bytes[1] = bytes[2..].iter().copied().fold(0u8, u8::wrapping_add);
    }
}

fn copy_note_bytes(raw: &[u8], pages: &[u16], offset: usize, destination: &mut [u8]) {
    for (index, byte) in destination.iter_mut().enumerate() {
        let note_offset = offset + index;
        let page = usize::from(pages[note_offset / PFS_PAGE_SIZE]);
        let page_offset = note_offset % PFS_PAGE_SIZE;
        *byte = raw[page * PFS_PAGE_SIZE + page_offset];
    }
}

fn write_note_bytes(raw: &mut [u8], pages: &[u16], offset: usize, source: &[u8]) {
    for (index, byte) in source.iter().copied().enumerate() {
        let note_offset = offset + index;
        let page = usize::from(pages[note_offset / PFS_PAGE_SIZE]);
        let page_offset = note_offset % PFS_PAGE_SIZE;
        raw[page * PFS_PAGE_SIZE + page_offset] = byte;
    }
}

fn validate_raw_block(address: usize) -> Result<(), PfsError> {
    if !address.is_multiple_of(PFS_BLOCK_SIZE) {
        return Err(PfsError::Invalid);
    }
    let end = address
        .checked_add(PFS_BLOCK_SIZE)
        .ok_or(PfsError::Invalid)?;
    (end <= 2 * PFS_BANK_CAPACITY)
        .then_some(())
        .ok_or(PfsError::Invalid)
}

fn validate_io_range(file_len: usize, offset: usize, len: usize) -> Result<(), PfsError> {
    if !offset.is_multiple_of(PFS_BLOCK_SIZE) || !len.is_multiple_of(PFS_BLOCK_SIZE) {
        return Err(PfsError::Invalid);
    }
    let end = offset.checked_add(len).ok_or(PfsError::Invalid)?;
    if end > file_len {
        return Err(PfsError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u8) -> PfsKey {
        PfsKey {
            company_code: 0x1234,
            game_code: 0x4142_4300 | u32::from(id),
            game_name: [id; 16],
            ext_name: [id, 0, 0, 0],
        }
    }

    fn read_page(pak: &ControllerPak, page: usize) -> [u8; PFS_PAGE_SIZE] {
        let mut output = [0u8; PFS_PAGE_SIZE];
        for block in 0..PFS_PAGE_SIZE / PFS_BLOCK_SIZE {
            let address = page * PFS_PAGE_SIZE + block * PFS_BLOCK_SIZE;
            pak.raw_read_block(
                address,
                (&mut output[block * PFS_BLOCK_SIZE..(block + 1) * PFS_BLOCK_SIZE])
                    .try_into()
                    .unwrap(),
            )
            .unwrap();
        }
        output
    }

    fn write_page(pak: &mut ControllerPak, page: usize, bytes: &[u8; PFS_PAGE_SIZE]) {
        for block in 0..PFS_PAGE_SIZE / PFS_BLOCK_SIZE {
            let address = page * PFS_PAGE_SIZE + block * PFS_BLOCK_SIZE;
            pak.raw_write_block(
                address,
                (&bytes[block * PFS_BLOCK_SIZE..(block + 1) * PFS_BLOCK_SIZE])
                    .try_into()
                    .unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn allocation_rounds_to_pages_and_reclaims_space() {
        let mut pak = ControllerPak::new();
        let file = pak.allocate(key(1), 257).unwrap();
        assert_eq!(pak.state(file).unwrap().file_size, 512);
        assert_eq!(pak.free_bytes().unwrap(), PFS_CAPACITY - 512);
        pak.delete(key(1)).unwrap();
        assert_eq!(pak.free_bytes().unwrap(), PFS_CAPACITY);
    }

    #[test]
    fn block_io_round_trips_and_rejects_unaligned_ranges() {
        let mut pak = ControllerPak::new();
        let file = pak.allocate(key(2), 256).unwrap();
        let source = [0xA5; PFS_BLOCK_SIZE];
        pak.write(file, 32, &source).unwrap();
        let mut destination = [0; PFS_BLOCK_SIZE];
        pak.read(file, 32, &mut destination).unwrap();
        assert_eq!(destination, source);
        assert_eq!(pak.write(file, 1, &source), Err(PfsError::Invalid));
    }

    #[test]
    fn high_level_allocation_encodes_exact_fat_directory_and_data_bytes() {
        let mut pak = ControllerPak::new();
        let note_key = key(3);
        assert_eq!(pak.allocate(note_key, 512), Ok(0));

        let primary = read_page(&pak, PFS_FAT_PRIMARY_PAGE);
        let backup = read_page(&pak, PFS_FAT_BACKUP_PAGE);
        assert_eq!(primary, backup);
        assert_eq!(primary[5 * 2..5 * 2 + 2], 6u16.to_be_bytes());
        assert_eq!(primary[6 * 2..6 * 2 + 2], PFS_FAT_END.to_be_bytes());
        assert_eq!(
            primary[1],
            primary[2..].iter().copied().fold(0u8, u8::wrapping_add)
        );

        let directory = read_page(&pak, 3);
        assert_eq!(directory[0..4], note_key.game_code.to_be_bytes());
        assert_eq!(directory[4..6], note_key.company_code.to_be_bytes());
        assert_eq!(directory[6..8], 5u16.to_be_bytes());
        assert_eq!(directory[8], PFS_NOTE_STATUS_OCCUPIED);
        assert_eq!(directory[0x0c..0x10], note_key.ext_name);
        assert_eq!(directory[0x10..0x20], note_key.game_name);
        assert!(read_page(&pak, 5).iter().all(|&byte| byte == 0));
        assert!(read_page(&pak, 6).iter().all(|&byte| byte == 0));
    }

    #[test]
    fn raw_management_and_data_pages_define_high_level_note() {
        let source_key = key(4);
        let mut source = ControllerPak::new();
        source.allocate(source_key, 512).unwrap();
        let payload = [0x5a; PFS_BLOCK_SIZE];
        source.write(0, PFS_PAGE_SIZE, &payload).unwrap();

        let mut raw = ControllerPak::new();
        for page in 1..=6 {
            write_page(&mut raw, page, &read_page(&source, page));
        }
        assert_eq!(raw.find(source_key), Ok(0));
        assert_eq!(raw.state(0).unwrap().file_size, 512);
        assert_eq!(raw.free_bytes().unwrap(), PFS_CAPACITY - 512);
        let mut observed = [0u8; PFS_BLOCK_SIZE];
        raw.read(0, PFS_PAGE_SIZE, &mut observed).unwrap();
        assert_eq!(observed, payload);
        assert_eq!(raw.evidence_snapshot(), source.evidence_snapshot());
    }

    #[test]
    fn one_bad_fat_checksum_repairs_from_the_unambiguous_copy() {
        for (damaged_page, good_page) in [
            (PFS_FAT_PRIMARY_PAGE, PFS_FAT_BACKUP_PAGE),
            (PFS_FAT_BACKUP_PAGE, PFS_FAT_PRIMARY_PAGE),
        ] {
            let mut pak = ControllerPak::new();
            pak.allocate(key(5), PFS_PAGE_SIZE).unwrap();
            let good = read_page(&pak, good_page);
            let mut damaged = read_page(&pak, damaged_page);
            damaged[1] ^= 1;
            write_page(&mut pak, damaged_page, &damaged);
            assert_eq!(pak.free_bytes(), Err(PfsError::Inconsistent));
            assert_eq!(pak.check_and_repair(), Ok(()));
            assert_eq!(read_page(&pak, damaged_page), good);
            assert_eq!(pak.state(0).unwrap().file_size, PFS_PAGE_SIZE as u32);
        }
    }

    #[test]
    fn ambiguous_or_structurally_corrupt_management_data_is_inconsistent() {
        let mut mismatch = ControllerPak::new();
        let mut primary = read_page(&mismatch, PFS_FAT_PRIMARY_PAGE);
        primary[5 * 2..5 * 2 + 2].copy_from_slice(&PFS_FAT_END.to_be_bytes());
        primary[1] = primary[2..].iter().copied().fold(0u8, u8::wrapping_add);
        write_page(&mut mismatch, PFS_FAT_PRIMARY_PAGE, &primary);
        assert_eq!(mismatch.check_and_repair(), Err(PfsError::Inconsistent));

        let mut cycle = ControllerPak::new();
        cycle.allocate(key(6), 2 * PFS_PAGE_SIZE).unwrap();
        let mut fat = decode_fat_copy(&cycle.raw, PFS_FAT_PRIMARY_PAGE, 1).unwrap();
        fat[6] = 5;
        cycle.write_fat_copies(&fat);
        assert_eq!(cycle.state(0), Err(PfsError::Inconsistent));

        let mut orphan = ControllerPak::new();
        let mut fat = decode_fat_copy(&orphan.raw, PFS_FAT_PRIMARY_PAGE, 1).unwrap();
        fat[7] = PFS_FAT_END;
        orphan.write_fat_copies(&fat);
        assert_eq!(orphan.free_bytes(), Err(PfsError::Inconsistent));

        let mut out_of_range = ControllerPak::new();
        out_of_range.allocate(key(9), PFS_PAGE_SIZE).unwrap();
        let mut fat = decode_fat_copy(&out_of_range.raw, PFS_FAT_PRIMARY_PAGE, 1).unwrap();
        fat[5] = PFS_TOTAL_PAGES as u16;
        out_of_range.write_fat_copies(&fat);
        assert_eq!(out_of_range.state(0), Err(PfsError::Inconsistent));
    }

    #[test]
    fn shared_chain_and_invalid_directory_fields_are_inconsistent() {
        let mut pak = ControllerPak::new();
        pak.allocate(key(7), PFS_PAGE_SIZE).unwrap();
        let mut directory = read_page(&pak, 3);
        let first = directory[..PFS_DIRECTORY_ENTRY_SIZE].to_vec();
        directory[PFS_DIRECTORY_ENTRY_SIZE..2 * PFS_DIRECTORY_ENTRY_SIZE].copy_from_slice(&first);
        directory[PFS_DIRECTORY_ENTRY_SIZE] ^= 1;
        write_page(&mut pak, 3, &directory);
        assert_eq!(pak.state(1), Err(PfsError::Inconsistent));

        let mut bad_status = ControllerPak::new();
        bad_status.allocate(key(8), PFS_PAGE_SIZE).unwrap();
        let mut directory = read_page(&bad_status, 3);
        directory[8] = 0;
        write_page(&mut bad_status, 3, &directory);
        assert_eq!(bad_status.find(key(8)), Err(PfsError::Inconsistent));
    }

    #[test]
    fn bank_count_type_enforces_published_geometry_limit() {
        assert_eq!(ControllerPakBankCount::new(0), None);
        assert_eq!(ControllerPakBankCount::new(1).unwrap().get(), 1);
        assert_eq!(
            ControllerPakBankCount::new(PFS_MAX_BANKS).unwrap().get(),
            62
        );
        assert_eq!(ControllerPakBankCount::new(PFS_MAX_BANKS + 1), None);

        let maximum =
            ControllerPak::with_bank_count(ControllerPakBankCount::new(PFS_MAX_BANKS).unwrap());
        assert_eq!(maximum.raw[PFS_ID_PRIMARY_OFFSET + 0x1a], PFS_MAX_BANKS);
        assert_eq!(maximum.free_bytes().unwrap(), 7_748 * PFS_PAGE_SIZE);
    }

    #[test]
    fn two_bank_format_and_allocation_cross_the_reserved_bank_boundary() {
        let banks = ControllerPakBankCount::new(2).unwrap();
        let mut pak = ControllerPak::with_bank_count(banks);
        assert_eq!(pak.bank_count(), banks);
        assert_eq!(pak.raw.len(), 2 * PFS_BANK_CAPACITY);
        assert_eq!(pak.raw[PFS_ID_PRIMARY_OFFSET + 0x1a], 2);
        assert_eq!(pak.free_bytes().unwrap(), 248 * PFS_PAGE_SIZE);

        let file = pak.allocate(key(10), 122 * PFS_PAGE_SIZE).unwrap();
        let snapshot = pak.evidence_snapshot();
        let pages = &snapshot.notes[file].as_ref().unwrap().pages;
        assert_eq!(pages[0], 7);
        assert_eq!(pages[120], 127);
        assert_eq!(pages[121], 129);
        assert!(!pages.contains(&128));

        let primary_bank_zero = read_page(&pak, 1);
        let primary_bank_one = read_page(&pak, 2);
        assert_eq!(primary_bank_zero[254..256], 129u16.to_be_bytes());
        assert_eq!(primary_bank_one[2..4], PFS_FAT_END.to_be_bytes());
        assert_eq!(read_page(&pak, 1), read_page(&pak, 3));
        assert_eq!(read_page(&pak, 2), read_page(&pak, 4));
    }

    #[test]
    fn raw_bank_latch_selects_storage_and_high_window_reads_zero() {
        let mut pak = ControllerPak::with_bank_count(ControllerPakBankCount::new(4).unwrap());
        let select_bank_one = [1; PFS_BLOCK_SIZE];
        pak.raw_write_block(PFS_BANK_CAPACITY, &select_bank_one)
            .unwrap();
        assert_eq!(pak.active_bank(), 1);

        let payload = [0x5a; PFS_BLOCK_SIZE];
        pak.raw_write_block(PFS_PAGE_SIZE, &payload).unwrap();
        let mut observed = [0; PFS_BLOCK_SIZE];
        pak.raw_read_block(PFS_PAGE_SIZE, &mut observed).unwrap();
        assert_eq!(observed, payload);

        let select_bank_zero = [0; PFS_BLOCK_SIZE];
        pak.raw_write_block(PFS_BANK_CAPACITY, &select_bank_zero)
            .unwrap();
        pak.raw_read_block(PFS_PAGE_SIZE, &mut observed).unwrap();
        assert_ne!(observed, payload);

        let select_mirrored_bank = [5; PFS_BLOCK_SIZE];
        pak.raw_write_block(0xffe0, &select_mirrored_bank).unwrap();
        assert_eq!(pak.active_bank(), 1);
        pak.raw_read_block(PFS_PAGE_SIZE, &mut observed).unwrap();
        assert_eq!(observed, payload);

        observed.fill(0xff);
        pak.raw_read_block(PFS_BANK_CAPACITY, &mut observed)
            .unwrap();
        assert_eq!(observed, [0; PFS_BLOCK_SIZE]);

        let mut nonuniform = [0; PFS_BLOCK_SIZE];
        nonuniform[PFS_BLOCK_SIZE - 1] = 1;
        assert_eq!(
            pak.raw_write_block(PFS_BANK_CAPACITY, &nonuniform),
            Err(PfsError::Invalid)
        );

        let mut odd_sized = ControllerPak::with_bank_count(ControllerPakBankCount::new(3).unwrap());
        assert_eq!(
            odd_sized.raw_write_block(PFS_BANK_CAPACITY, &[3; PFS_BLOCK_SIZE]),
            Err(PfsError::Invalid)
        );
    }

    #[test]
    fn high_level_data_crossing_banks_is_visible_through_raw_latch() {
        let mut pak = ControllerPak::with_bank_count(ControllerPakBankCount::new(2).unwrap());
        let file = pak.allocate(key(11), 122 * PFS_PAGE_SIZE).unwrap();
        let payload = [0xa6; PFS_BLOCK_SIZE];
        pak.write(file, 121 * PFS_PAGE_SIZE, &payload).unwrap();

        pak.raw_write_block(PFS_BANK_CAPACITY, &[1; PFS_BLOCK_SIZE])
            .unwrap();
        let mut observed = [0; PFS_BLOCK_SIZE];
        pak.raw_read_block(PFS_PAGE_SIZE, &mut observed).unwrap();
        assert_eq!(observed, payload);

        let mut high_level = [0; PFS_BLOCK_SIZE];
        pak.read(file, 121 * PFS_PAGE_SIZE, &mut high_level)
            .unwrap();
        assert_eq!(high_level, payload);
    }

    #[test]
    fn repair_applies_to_each_banked_fat_pair_only_after_full_validation() {
        let mut pak = ControllerPak::with_bank_count(ControllerPakBankCount::new(2).unwrap());
        pak.allocate(key(12), 122 * PFS_PAGE_SIZE).unwrap();
        let good = read_page(&pak, 4);
        let mut damaged = read_page(&pak, 2);
        damaged[1] ^= 1;
        write_page(&mut pak, 2, &damaged);
        assert_eq!(pak.check_and_repair(), Ok(()));
        assert_eq!(read_page(&pak, 2), good);
    }
}
