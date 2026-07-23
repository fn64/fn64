//! Side-effect-free memory transactions for audio HLE task preflight.
//!
//! RDRAM accesses are expressed only as logical guest byte addresses and pass
//! through [`fn64_runtime::RdramView`], preserving the native-word lane mapping
//! owned by `fn64-runtime`. Writes remain in a sparse overlay until a caller
//! extracts canonical patches; this module never mutates live RDRAM.

use std::collections::BTreeMap;

use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use fn64_runtime::{RdramAddr, RdramView};

use crate::hle_outcome::{
    CanonicalRdramError, CanonicalRdramPatches, RdramByteRange, RdramPatch, RdramPatchError,
    RdramRangeError, RSP_BANK_BYTES,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioHleTransactionError {
    PhysicalRdramTooSmall {
        storage_bytes: usize,
        required_bytes: usize,
    },
    RdramRange(RdramRangeError),
    Patch(RdramPatchError),
    CanonicalPatches(CanonicalRdramError),
}

/// A read-only live RDRAM view plus sparse logical-byte writes.
///
/// The view may cover a larger generated-code allocation, but this transaction
/// admits only the console's physical RDRAM range. The sparse map records write
/// intent even when a byte equals its live value, which is required to compare
/// the exact mutation ranges produced by HLE and LLE.
pub struct AudioHleTaskTransaction<'a> {
    live: RdramView<'a>,
    writes: BTreeMap<u32, u8>,
}

impl<'a> AudioHleTaskTransaction<'a> {
    pub fn new(live: RdramView<'a>) -> Result<Self, AudioHleTransactionError> {
        if live.len() < DEFAULT_RDRAM_SIZE {
            return Err(AudioHleTransactionError::PhysicalRdramTooSmall {
                storage_bytes: live.len(),
                required_bytes: DEFAULT_RDRAM_SIZE,
            });
        }
        Ok(Self {
            live,
            writes: BTreeMap::new(),
        })
    }

    pub fn read_u8(&self, addr: RdramAddr) -> Result<u8, AudioHleTransactionError> {
        let range =
            RdramByteRange::new(addr.offset(), 1).map_err(AudioHleTransactionError::RdramRange)?;
        Ok(self.read_checked_byte(range.start()))
    }

    pub fn read_bytes(
        &self,
        addr: RdramAddr,
        byte_len: u32,
    ) -> Result<Vec<u8>, AudioHleTransactionError> {
        if byte_len == 0 {
            return Ok(Vec::new());
        }
        let range = RdramByteRange::new(addr.offset(), byte_len)
            .map_err(AudioHleTransactionError::RdramRange)?;
        Ok((range.start()..range.end())
            .map(|offset| self.read_checked_byte(offset))
            .collect())
    }

    pub fn write_u8(&mut self, addr: RdramAddr, value: u8) -> Result<(), AudioHleTransactionError> {
        let range =
            RdramByteRange::new(addr.offset(), 1).map_err(AudioHleTransactionError::RdramRange)?;
        self.writes.insert(range.start(), value);
        Ok(())
    }

    /// Stage one complete logical-byte write after preflighting its full range.
    /// A rejected range cannot leave a partial prefix in the transaction.
    pub fn write_bytes(
        &mut self,
        addr: RdramAddr,
        bytes: &[u8],
    ) -> Result<(), AudioHleTransactionError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let byte_len = u32::try_from(bytes.len()).map_err(|_| {
            AudioHleTransactionError::Patch(RdramPatchError::ByteLengthExceedsU32 {
                byte_len: bytes.len(),
            })
        })?;
        let range = RdramByteRange::new(addr.offset(), byte_len)
            .map_err(AudioHleTransactionError::RdramRange)?;
        for (offset, &byte) in (range.start()..range.end()).zip(bytes) {
            self.writes.insert(offset, byte);
        }
        Ok(())
    }

    pub fn written_byte_count(&self) -> usize {
        self.writes.len()
    }

    pub fn canonical_patches(&self) -> Result<CanonicalRdramPatches, AudioHleTransactionError> {
        let mut patches = Vec::new();
        let mut start = None;
        let mut previous = 0u32;
        let mut bytes = Vec::new();

        for (&offset, &byte) in &self.writes {
            if start.is_some() && offset != previous + 1 {
                patches.push(
                    RdramPatch::new(start.expect("nonempty patch has a start"), bytes)
                        .map_err(AudioHleTransactionError::Patch)?,
                );
                start = None;
                bytes = Vec::new();
            }
            if start.is_none() {
                start = Some(offset);
            }
            previous = offset;
            bytes.push(byte);
        }
        if let Some(start) = start {
            patches.push(RdramPatch::new(start, bytes).map_err(AudioHleTransactionError::Patch)?);
        }

        CanonicalRdramPatches::new(patches).map_err(AudioHleTransactionError::CanonicalPatches)
    }

    fn read_checked_byte(&self, offset: u32) -> u8 {
        self.writes
            .get(&offset)
            .copied()
            .unwrap_or_else(|| self.live.read_u8(RdramAddr::from_offset(offset)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmemRangeError {
    Empty,
    OutOfBounds { start: u16, byte_len: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmemAddr(u16);

impl DmemAddr {
    pub fn new(offset: u16) -> Result<Self, DmemRangeError> {
        if usize::from(offset) >= RSP_BANK_BYTES {
            return Err(DmemRangeError::OutOfBounds {
                start: offset,
                byte_len: 1,
            });
        }
        Ok(Self(offset))
    }

    pub const fn offset(self) -> u16 {
        self.0
    }
}

/// A nonempty half-open range wholly contained in the 4 KiB DMEM bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmemByteRange {
    start: u16,
    byte_len: u16,
}

impl DmemByteRange {
    pub fn new(start: u16, byte_len: u16) -> Result<Self, DmemRangeError> {
        if byte_len == 0 {
            return Err(DmemRangeError::Empty);
        }
        let end = u32::from(start) + u32::from(byte_len);
        if end > RSP_BANK_BYTES as u32 {
            return Err(DmemRangeError::OutOfBounds { start, byte_len });
        }
        Ok(Self { start, byte_len })
    }

    pub const fn start(self) -> u16 {
        self.start
    }

    pub const fn byte_len(self) -> u16 {
        self.byte_len
    }

    pub const fn end(self) -> u16 {
        // The constructor proves this sum is at most 0x1000.
        self.start + self.byte_len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DmemWriteError {
    LengthMismatch {
        range_bytes: u16,
        supplied_bytes: usize,
    },
}

/// Owned architectural DMEM bytes with checked address/range access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedDmem {
    bytes: [u8; RSP_BANK_BYTES],
}

impl Default for OwnedDmem {
    fn default() -> Self {
        Self::new([0; RSP_BANK_BYTES])
    }
}

impl OwnedDmem {
    pub const fn new(bytes: [u8; RSP_BANK_BYTES]) -> Self {
        Self { bytes }
    }

    pub fn read_u8(&self, addr: DmemAddr) -> u8 {
        self.bytes[usize::from(addr.offset())]
    }

    pub fn write_u8(&mut self, addr: DmemAddr, value: u8) {
        self.bytes[usize::from(addr.offset())] = value;
    }

    pub fn read_range(&self, range: DmemByteRange) -> &[u8] {
        &self.bytes[usize::from(range.start())..usize::from(range.end())]
    }

    /// Replace a complete checked range. A length mismatch is rejected before
    /// any byte is changed.
    pub fn write_range(
        &mut self,
        range: DmemByteRange,
        bytes: &[u8],
    ) -> Result<(), DmemWriteError> {
        if bytes.len() != usize::from(range.byte_len()) {
            return Err(DmemWriteError::LengthMismatch {
                range_bytes: range.byte_len(),
                supplied_bytes: bytes.len(),
            });
        }
        self.bytes[usize::from(range.start())..usize::from(range.end())].copy_from_slice(bytes);
        Ok(())
    }

    pub const fn image(&self) -> &[u8; RSP_BANK_BYTES] {
        &self.bytes
    }

    pub const fn into_image(self) -> [u8; RSP_BANK_BYTES] {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::RdramViewMut;

    fn physical_storage() -> Vec<u8> {
        vec![0; DEFAULT_RDRAM_SIZE]
    }

    #[test]
    fn logical_byte_reads_preserve_native_word_lane_semantics() {
        let mut storage = physical_storage();
        RdramViewMut::from_storage(&mut storage).write_u32(RdramAddr::from_offset(0), 0x1122_3344);
        let transaction = AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();

        assert_eq!(
            transaction
                .read_bytes(RdramAddr::from_offset(0), 4)
                .unwrap(),
            [0x11, 0x22, 0x33, 0x44]
        );
    }

    #[test]
    fn staged_writes_read_through_and_do_not_mutate_live_rdram() {
        let mut storage = physical_storage();
        RdramViewMut::from_storage(&mut storage)
            .write_u32(RdramAddr::from_offset(0x20), 0x1020_3040);
        let live = RdramView::from_storage(&storage);
        let mut transaction = AudioHleTaskTransaction::new(live).unwrap();

        transaction
            .write_bytes(RdramAddr::from_offset(0x21), &[0xaa, 0xbb])
            .unwrap();
        assert_eq!(
            transaction
                .read_bytes(RdramAddr::from_offset(0x20), 4)
                .unwrap(),
            [0x10, 0xaa, 0xbb, 0x40]
        );
        assert_eq!(
            live.copy_logical_test_bytes(RdramAddr::from_offset(0x20), 4),
            [0x10, 0x20, 0x30, 0x40],
            "the transaction must retain writes only in its sparse overlay"
        );
    }

    #[test]
    fn adjacent_out_of_order_writes_extract_one_canonical_patch() {
        let storage = physical_storage();
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        transaction
            .write_bytes(RdramAddr::from_offset(0x104), &[5, 6])
            .unwrap();
        transaction
            .write_bytes(RdramAddr::from_offset(0x100), &[1, 2, 3, 4])
            .unwrap();
        transaction
            .write_u8(RdramAddr::from_offset(0x200), 9)
            .unwrap();

        let patches = transaction.canonical_patches().unwrap();
        assert_eq!(patches.as_slice().len(), 2);
        assert_eq!(
            patches.as_slice()[0].range(),
            RdramByteRange::new(0x100, 6).unwrap()
        );
        assert_eq!(patches.as_slice()[0].bytes(), &[1, 2, 3, 4, 5, 6]);
        assert_eq!(patches.as_slice()[1].bytes(), &[9]);
    }

    #[test]
    fn rdram_bounds_fail_before_changing_the_overlay() {
        let storage = physical_storage();
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        transaction
            .write_u8(RdramAddr::from_offset(0x100), 7)
            .unwrap();
        let before = transaction.canonical_patches().unwrap();

        assert!(matches!(
            transaction.write_bytes(
                RdramAddr::from_offset(DEFAULT_RDRAM_SIZE as u32 - 1),
                &[1, 2],
            ),
            Err(AudioHleTransactionError::RdramRange(
                RdramRangeError::OutOfBounds { .. }
            ))
        ));
        assert!(matches!(
            transaction.read_bytes(RdramAddr::from_offset(u32::MAX), 2),
            Err(AudioHleTransactionError::RdramRange(
                RdramRangeError::AddressOverflow { .. }
            ))
        ));
        assert_eq!(transaction.canonical_patches().unwrap(), before);
    }

    #[test]
    fn short_rdram_is_rejected_and_empty_access_is_a_no_op() {
        let short = vec![0; DEFAULT_RDRAM_SIZE - 1];
        assert!(matches!(
            AudioHleTaskTransaction::new(RdramView::from_storage(&short)),
            Err(AudioHleTransactionError::PhysicalRdramTooSmall { .. })
        ));

        let storage = physical_storage();
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        assert_eq!(
            transaction
                .read_bytes(RdramAddr::from_offset(u32::MAX), 0)
                .unwrap(),
            []
        );
        transaction
            .write_bytes(RdramAddr::from_offset(u32::MAX), &[])
            .unwrap();
        assert_eq!(transaction.written_byte_count(), 0);
    }

    #[test]
    fn dmem_addresses_and_ranges_are_checked() {
        assert!(matches!(
            DmemAddr::new(RSP_BANK_BYTES as u16),
            Err(DmemRangeError::OutOfBounds { .. })
        ));
        assert!(matches!(
            DmemByteRange::new(0xfff, 2),
            Err(DmemRangeError::OutOfBounds { .. })
        ));
        assert_eq!(DmemByteRange::new(0, 0), Err(DmemRangeError::Empty));

        let mut dmem = OwnedDmem::default();
        let last = DmemAddr::new(0xfff).unwrap();
        dmem.write_u8(last, 0x7f);
        assert_eq!(dmem.read_u8(last), 0x7f);
    }

    #[test]
    fn dmem_range_write_is_atomic_on_length_mismatch() {
        let mut initial = [0u8; RSP_BANK_BYTES];
        initial[0x20..0x24].copy_from_slice(&[1, 2, 3, 4]);
        let mut dmem = OwnedDmem::new(initial);
        let range = DmemByteRange::new(0x20, 4).unwrap();

        assert_eq!(
            dmem.write_range(range, &[9, 9]),
            Err(DmemWriteError::LengthMismatch {
                range_bytes: 4,
                supplied_bytes: 2,
            })
        );
        assert_eq!(dmem.read_range(range), &[1, 2, 3, 4]);
        dmem.write_range(range, &[5, 6, 7, 8]).unwrap();
        assert_eq!(dmem.read_range(range), &[5, 6, 7, 8]);
    }

    trait LogicalTestBytes {
        fn copy_logical_test_bytes(self, addr: RdramAddr, byte_len: usize) -> Vec<u8>;
    }

    impl LogicalTestBytes for RdramView<'_> {
        fn copy_logical_test_bytes(self, addr: RdramAddr, byte_len: usize) -> Vec<u8> {
            let mut bytes = vec![0; byte_len];
            self.copy_logical_bytes(addr, &mut bytes);
            bytes
        }
    }
}
