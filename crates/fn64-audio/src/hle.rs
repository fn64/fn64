//! Clean-room audio Binary Interface (ABI) command-list substrate.
//!
//! The public N64 manuals define an audio task's command list as a sequence of
//! 64-bit ABI commands. Different audio microcode families assign different
//! meanings to the opcode and payload fields, so this module deliberately
//! separates family-neutral framing from family-specific execution. An
//! unrecognized family must never be decoded using a convenient opcode table.
//!
//! Provenance: public Nintendo 64 Introductory Manual, “Using Microcode”
//! (audio commands are 64-bit ABI commands), and the public audio-coprocessor
//! descriptions in US 6,342,892 and US 6,331,856. No GPL runtime or generated
//! audio-microcode implementation was read.

use core::fmt;

use fn64_runtime::{RdramAddr, RdramView};

use crate::hle_outcome::{AudioHleFamily, AudioHleSelection, AudioMicrocodeIdentity};
use crate::standard_abi::{DecodedStandardAbiPacket, StandardAbiPacket, UnknownStandardAbiOpcode};

/// Every public audio ABI command occupies two 32-bit words.
pub const ABI_COMMAND_BYTES: u32 = 8;

/// One family-neutral 64-bit audio ABI command.
///
/// Field interpretation beyond the leading opcode byte belongs to an exact
/// admitted microcode family. Retaining both words prevents a common decoder
/// bug where a shared-looking opcode silently discards family-specific bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiCommand {
    pub w0: u32,
    pub w1: u32,
}

impl AbiCommand {
    pub const fn new(w0: u32, w1: u32) -> Self {
        Self { w0, w1 }
    }

    /// The command selector common to the public 64-bit ABI framing.
    pub const fn opcode(self) -> u8 {
        (self.w0 >> 24) as u8
    }
}

/// Structural failures found before any family-specific command executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandListError {
    UnalignedAddress {
        address: u32,
    },
    PartialCommand {
        byte_len: u32,
    },
    AddressOverflow {
        address: u32,
        byte_len: u32,
    },
    OutOfBounds {
        address: u32,
        byte_len: u32,
        rdram_len: usize,
    },
}

impl fmt::Display for CommandListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnalignedAddress { address } => write!(
                f,
                "audio ABI command list address {address:#010x} is not 8-byte aligned"
            ),
            Self::PartialCommand { byte_len } => write!(
                f,
                "audio ABI command list length {byte_len:#x} is not a multiple of 8 bytes"
            ),
            Self::AddressOverflow { address, byte_len } => write!(
                f,
                "audio ABI command list {address:#010x}+{byte_len:#x} overflows physical addressing"
            ),
            Self::OutOfBounds {
                address,
                byte_len,
                rdram_len,
            } => write!(
                f,
                "audio ABI command list {address:#010x}..{:#010x} exceeds {rdram_len:#x} bytes of RDRAM",
                address.saturating_add(byte_len)
            ),
        }
    }
}

impl std::error::Error for CommandListError {}

/// One exact identity-to-family mapping in an embedder-owned HLE catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioHleCatalogEntry {
    pub identity: AudioMicrocodeIdentity,
    pub family: AudioHleFamily,
    pub implementation_revision: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioHleCatalogError {
    DuplicateIdentity { identity: AudioMicrocodeIdentity },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownAudioMicrocodeIdentity {
    pub identity: AudioMicrocodeIdentity,
}

impl fmt::Display for UnknownAudioMicrocodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "audio HLE catalog has no exact task-entry identity match"
        )
    }
}

impl std::error::Error for UnknownAudioMicrocodeIdentity {}

/// A validated immutable catalog. Fn64 ships no guessed family mappings.
#[derive(Clone, Copy)]
pub struct AudioHleCatalog<'a> {
    entries: &'a [AudioHleCatalogEntry],
}

impl<'a> AudioHleCatalog<'a> {
    pub fn new(entries: &'a [AudioHleCatalogEntry]) -> Result<Self, AudioHleCatalogError> {
        for (index, entry) in entries.iter().enumerate() {
            if entries[index + 1..]
                .iter()
                .any(|candidate| candidate.identity == entry.identity)
            {
                return Err(AudioHleCatalogError::DuplicateIdentity {
                    identity: entry.identity,
                });
            }
        }
        Ok(Self { entries })
    }

    pub fn admit(
        self,
        identity: AudioMicrocodeIdentity,
    ) -> Result<AdmittedAudioMicrocode, UnknownAudioMicrocodeIdentity> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.identity == identity)
            .ok_or(UnknownAudioMicrocodeIdentity { identity })?;
        Ok(AdmittedAudioMicrocode {
            selection: AudioHleSelection {
                microcode: identity,
                family: entry.family,
                implementation_revision: entry.implementation_revision,
            },
        })
    }
}

/// Non-forgeable proof that a complete task-entry identity matched a catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmittedAudioMicrocode {
    selection: AudioHleSelection,
}

impl AdmittedAudioMicrocode {
    pub const fn identity(self) -> AudioMicrocodeIdentity {
        self.selection.microcode
    }

    pub const fn family(self) -> AudioHleFamily {
        self.selection.family
    }

    pub const fn implementation_revision(self) -> u32 {
        self.selection.implementation_revision
    }

    pub const fn selection(self) -> AudioHleSelection {
        self.selection
    }

    /// Decode a packet only through the family selected by exact admission.
    pub fn decode_standard_abi(
        self,
        command: AbiCommand,
    ) -> Result<DecodedStandardAbiPacket, UnknownStandardAbiOpcode> {
        match self.selection.family {
            AudioHleFamily::StandardAbi => StandardAbiPacket::from(command).decode(),
        }
    }
}

/// A validated, allocation-free view of an audio ABI command list.
#[derive(Clone, Copy)]
pub struct CommandList<'a> {
    rdram: RdramView<'a>,
    address: u32,
    command_count: u32,
}

impl<'a> CommandList<'a> {
    /// Validate the complete command range before exposing any command.
    pub fn new(
        rdram: RdramView<'a>,
        address: u32,
        byte_len: u32,
    ) -> Result<Self, CommandListError> {
        if !address.is_multiple_of(ABI_COMMAND_BYTES) {
            return Err(CommandListError::UnalignedAddress { address });
        }
        if !byte_len.is_multiple_of(ABI_COMMAND_BYTES) {
            return Err(CommandListError::PartialCommand { byte_len });
        }
        let end = address
            .checked_add(byte_len)
            .ok_or(CommandListError::AddressOverflow { address, byte_len })?;
        if end as usize > rdram.len() {
            return Err(CommandListError::OutOfBounds {
                address,
                byte_len,
                rdram_len: rdram.len(),
            });
        }
        Ok(Self {
            rdram,
            address,
            command_count: byte_len / ABI_COMMAND_BYTES,
        })
    }

    pub const fn len(self) -> usize {
        self.command_count as usize
    }

    pub const fn is_empty(self) -> bool {
        self.command_count == 0
    }

    pub fn iter(self) -> CommandIter<'a> {
        CommandIter {
            list: self,
            next: 0,
        }
    }
}

impl<'a> IntoIterator for CommandList<'a> {
    type Item = AbiCommand;
    type IntoIter = CommandIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct CommandIter<'a> {
    list: CommandList<'a>,
    next: u32,
}

impl Iterator for CommandIter<'_> {
    type Item = AbiCommand;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.list.command_count {
            return None;
        }
        let offset = self
            .next
            .checked_mul(ABI_COMMAND_BYTES)
            .and_then(|offset| self.list.address.checked_add(offset))
            .expect("validated audio ABI command-list offset overflowed");
        self.next += 1;
        let w0 = self.list.rdram.read_u32(RdramAddr::from_offset(offset));
        let w1 = self.list.rdram.read_u32(RdramAddr::from_offset(offset + 4));
        Some(AbiCommand::new(w0, w1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.list.command_count - self.next) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CommandIter<'_> {}
impl std::iter::FusedIterator for CommandIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle_outcome::RSP_BANK_BYTES;

    fn put_word(storage: &mut [u8], offset: usize, word: u32) {
        storage[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
    }

    fn identity(imem_byte: u8, data: &[u8]) -> AudioMicrocodeIdentity {
        AudioMicrocodeIdentity::from_task_entry(&[imem_byte; RSP_BANK_BYTES], data).unwrap()
    }

    #[test]
    fn command_list_reads_native_word_storage_without_byte_reinterpretation() {
        let mut storage = [0u8; 32];
        put_word(&mut storage, 8, 0x04AA_1234);
        put_word(&mut storage, 12, 0x89AB_CDEF);
        put_word(&mut storage, 16, 0x0600_5678);
        put_word(&mut storage, 20, 0x0123_4567);

        let commands: Vec<_> = CommandList::new(RdramView::from_storage(&storage), 8, 16)
            .unwrap()
            .into_iter()
            .collect();

        assert_eq!(
            commands,
            [
                AbiCommand::new(0x04AA_1234, 0x89AB_CDEF),
                AbiCommand::new(0x0600_5678, 0x0123_4567),
            ]
        );
        assert_eq!(commands[0].opcode(), 0x04);
        assert_eq!(commands[1].opcode(), 0x06);
    }

    #[test]
    fn empty_command_list_is_valid_at_the_end_of_rdram() {
        let storage = [0u8; 16];
        let list = CommandList::new(RdramView::from_storage(&storage), 16, 0).unwrap();
        assert!(list.is_empty());
        assert_eq!(list.iter().len(), 0);
    }

    #[test]
    fn malformed_command_list_geometry_is_typed_and_side_effect_free() {
        let storage = [0u8; 32];
        let rdram = RdramView::from_storage(&storage);

        assert_eq!(
            CommandList::new(rdram, 4, 8).err(),
            Some(CommandListError::UnalignedAddress { address: 4 })
        );
        assert_eq!(
            CommandList::new(rdram, 8, 12).err(),
            Some(CommandListError::PartialCommand { byte_len: 12 })
        );
        assert_eq!(
            CommandList::new(rdram, u32::MAX - 7, 16).err(),
            Some(CommandListError::AddressOverflow {
                address: u32::MAX - 7,
                byte_len: 16,
            })
        );
        assert_eq!(
            CommandList::new(rdram, 24, 16).err(),
            Some(CommandListError::OutOfBounds {
                address: 24,
                byte_len: 16,
                rdram_len: 32,
            })
        );
    }

    #[test]
    fn exact_catalog_admission_is_required_before_standard_decode() {
        let admitted_identity = identity(1, &[2, 3, 4]);
        let unknown_identity = identity(9, &[2, 3, 4]);
        let catalog_entries = [AudioHleCatalogEntry {
            identity: admitted_identity,
            family: AudioHleFamily::StandardAbi,
            implementation_revision: 1,
        }];
        let catalog = AudioHleCatalog::new(&catalog_entries).unwrap();

        assert_eq!(
            catalog.admit(unknown_identity),
            Err(UnknownAudioMicrocodeIdentity {
                identity: unknown_identity,
            })
        );
        let admission = catalog.admit(admitted_identity).unwrap();
        assert_eq!(admission.identity(), admitted_identity);
        assert_eq!(admission.family(), AudioHleFamily::StandardAbi);
        assert_eq!(admission.implementation_revision(), 1);
        assert_eq!(
            admission
                .decode_standard_abi(AbiCommand::new(0x0200_0240, 0x0000_0060))
                .unwrap()
                .opcode as u8,
            0x02
        );
    }

    #[test]
    fn catalog_rejects_duplicate_identity_even_when_family_matches() {
        let duplicate = identity(1, &[2, 3, 4]);
        let entries = [
            AudioHleCatalogEntry {
                identity: duplicate,
                family: AudioHleFamily::StandardAbi,
                implementation_revision: 1,
            },
            AudioHleCatalogEntry {
                identity: duplicate,
                family: AudioHleFamily::StandardAbi,
                implementation_revision: 2,
            },
        ];
        assert_eq!(
            AudioHleCatalog::new(&entries).err(),
            Some(AudioHleCatalogError::DuplicateIdentity {
                identity: duplicate,
            })
        );
    }
}
