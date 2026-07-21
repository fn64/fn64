//! Semantic Controller Pak file system used by the public libultra PFS API.
//!
//! Provenance: public libultra Controller Pak manuals define 128 physical
//! 256-byte pages, five reserved management pages, 123 data pages, sixteen
//! directory entries, 32-byte I/O granularity, and the metadata/search rules.
//! This model owns those observable semantics without copying any runtime
//! implementation. Its 32 KiB physical image is authoritative for note data
//! and raw Joybus block I/O; the redundant label/inode/directory encoding in
//! management pages 0-4 remains a separate closure item.

pub const PFS_PAGE_SIZE: usize = 256;
pub const PFS_BLOCK_SIZE: usize = 32;
pub const PFS_TOTAL_PAGES: usize = 128;
pub const PFS_MANAGEMENT_PAGES: usize = 5;
pub const PFS_DATA_PAGES: usize = 123;
pub const PFS_CAPACITY: usize = PFS_PAGE_SIZE * PFS_DATA_PAGES;
pub const PFS_RAW_CAPACITY: usize = PFS_PAGE_SIZE * PFS_TOTAL_PAGES;
pub const PFS_MAX_FILES: usize = 16;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PfsError {
    Invalid,
    DataFull,
    DirectoryFull,
    Exists,
}

#[derive(Clone, Debug)]
struct Note {
    key: PfsKey,
    pages: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ControllerPak {
    notes: [Option<Note>; PFS_MAX_FILES],
    raw: Box<[u8; PFS_RAW_CAPACITY]>,
}

impl Default for ControllerPak {
    fn default() -> Self {
        Self {
            notes: std::array::from_fn(|_| None),
            raw: Box::new([0xFF; PFS_RAW_CAPACITY]),
        }
    }
}

impl ControllerPak {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn free_bytes(&self) -> usize {
        let used: usize = self
            .notes
            .iter()
            .flatten()
            .map(|note| note.pages.len() * PFS_PAGE_SIZE)
            .sum();
        PFS_CAPACITY - used
    }

    pub fn allocate(&mut self, key: PfsKey, requested_len: usize) -> Result<usize, PfsError> {
        if requested_len == 0 {
            return Err(PfsError::Invalid);
        }
        if self.find(key).is_ok() {
            return Err(PfsError::Exists);
        }
        let Some(slot) = self.notes.iter().position(Option::is_none) else {
            return Err(PfsError::DirectoryFull);
        };
        let rounded = requested_len
            .checked_add(PFS_PAGE_SIZE - 1)
            .ok_or(PfsError::Invalid)?
            / PFS_PAGE_SIZE
            * PFS_PAGE_SIZE;
        if rounded > self.free_bytes() {
            return Err(PfsError::DataFull);
        }
        let page_count = rounded / PFS_PAGE_SIZE;
        let mut used = [false; PFS_TOTAL_PAGES];
        used[..PFS_MANAGEMENT_PAGES].fill(true);
        for note in self.notes.iter().flatten() {
            for page in &note.pages {
                used[usize::from(*page)] = true;
            }
        }
        let pages = used
            .iter()
            .enumerate()
            .filter_map(|(page, used)| (!used).then_some(page as u8))
            .take(page_count)
            .collect::<Vec<_>>();
        if pages.len() != page_count {
            return Err(PfsError::DataFull);
        }
        for page in &pages {
            let start = usize::from(*page) * PFS_PAGE_SIZE;
            self.raw[start..start + PFS_PAGE_SIZE].fill(0);
        }
        self.notes[slot] = Some(Note { key, pages });
        Ok(slot)
    }

    pub fn find(&self, key: PfsKey) -> Result<usize, PfsError> {
        self.notes
            .iter()
            .position(|note| note.as_ref().is_some_and(|note| note.key == key))
            .ok_or(PfsError::Invalid)
    }

    pub fn delete(&mut self, key: PfsKey) -> Result<(), PfsError> {
        let slot = self.find(key)?;
        self.notes[slot] = None;
        Ok(())
    }

    pub fn state(&self, file_no: usize) -> Result<PfsState, PfsError> {
        let note = self
            .notes
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
        let note = self
            .notes
            .get(file_no)
            .and_then(Option::as_ref)
            .ok_or(PfsError::Invalid)?;
        validate_io_range(note.pages.len() * PFS_PAGE_SIZE, offset, destination.len())?;
        copy_note_bytes(&self.raw[..], &note.pages, offset, destination);
        Ok(())
    }

    pub fn write(&mut self, file_no: usize, offset: usize, source: &[u8]) -> Result<(), PfsError> {
        let (notes, raw) = (&self.notes, &mut self.raw);
        let note = notes
            .get(file_no)
            .and_then(Option::as_ref)
            .ok_or(PfsError::Invalid)?;
        validate_io_range(note.pages.len() * PFS_PAGE_SIZE, offset, source.len())?;
        write_note_bytes(&mut raw[..], &note.pages, offset, source);
        Ok(())
    }

    /// Read one physical 32-byte Controller Pak block. The public Joybus
    /// protocol exposes the full 0x0000..=0x7fff image in aligned blocks.
    pub fn raw_read_block(
        &self,
        address: usize,
        destination: &mut [u8; PFS_BLOCK_SIZE],
    ) -> Result<(), PfsError> {
        validate_raw_block(address)?;
        destination.copy_from_slice(&self.raw[address..address + PFS_BLOCK_SIZE]);
        Ok(())
    }

    /// Write one physical 32-byte Controller Pak block. Writes to data pages
    /// immediately affect any semantic note mapped over those bytes; raw
    /// management-page decoding is not yet claimed.
    pub fn raw_write_block(
        &mut self,
        address: usize,
        source: &[u8; PFS_BLOCK_SIZE],
    ) -> Result<(), PfsError> {
        validate_raw_block(address)?;
        self.raw[address..address + PFS_BLOCK_SIZE].copy_from_slice(source);
        Ok(())
    }
}

fn copy_note_bytes(raw: &[u8], pages: &[u8], offset: usize, destination: &mut [u8]) {
    for (index, byte) in destination.iter_mut().enumerate() {
        let note_offset = offset + index;
        let page = usize::from(pages[note_offset / PFS_PAGE_SIZE]);
        let page_offset = note_offset % PFS_PAGE_SIZE;
        *byte = raw[page * PFS_PAGE_SIZE + page_offset];
    }
}

fn write_note_bytes(raw: &mut [u8], pages: &[u8], offset: usize, source: &[u8]) {
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
    (end <= PFS_RAW_CAPACITY)
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
            game_code: id as u32,
            game_name: [id; 16],
            ext_name: [id, 0, 0, 0],
        }
    }

    #[test]
    fn allocation_rounds_to_pages_and_reclaims_space() {
        let mut pak = ControllerPak::new();
        let file = pak.allocate(key(1), 257).unwrap();
        assert_eq!(pak.state(file).unwrap().file_size, 512);
        assert_eq!(pak.free_bytes(), PFS_CAPACITY - 512);
        pak.delete(key(1)).unwrap();
        assert_eq!(pak.free_bytes(), PFS_CAPACITY);
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
    fn raw_blocks_and_semantic_notes_share_authoritative_data_pages() {
        let mut pak = ControllerPak::new();
        let file = pak.allocate(key(3), PFS_PAGE_SIZE).unwrap();
        let first_data_address = PFS_MANAGEMENT_PAGES * PFS_PAGE_SIZE;
        let raw_payload = [0x3C; PFS_BLOCK_SIZE];
        pak.raw_write_block(first_data_address, &raw_payload)
            .unwrap();
        let mut semantic = [0; PFS_BLOCK_SIZE];
        pak.read(file, 0, &mut semantic).unwrap();
        assert_eq!(semantic, raw_payload);

        let semantic_payload = [0xC3; PFS_BLOCK_SIZE];
        pak.write(file, PFS_BLOCK_SIZE, &semantic_payload).unwrap();
        let mut raw = [0; PFS_BLOCK_SIZE];
        pak.raw_read_block(first_data_address + PFS_BLOCK_SIZE, &mut raw)
            .unwrap();
        assert_eq!(raw, semantic_payload);
    }
}
