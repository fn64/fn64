//! Exact-image-characterized compact audio command dispatcher.
//!
//! This module composes the independently characterized memory and DSP
//! commands. It does not detect or admit a microcode family; callers must hold
//! exact catalog admission before selecting this grammar.

use core::fmt;

use crate::compact_dsp_abi::{
    decode_compact_dsp, execute_compact_dsp, CompactDspCommand, CompactDspDecodeError,
};
use crate::compact_memory_abi::{
    decode_compact_memory, execute_compact_memory, CompactMemoryCommand, CompactMemoryDecodeError,
    CompactMemoryExecutionError,
};
use crate::hle::{AbiCommand, AdmittedCompactAbiDecodeError};
use crate::hle_outcome::{AudioHleSelection, CanonicalRdramPatches};
use crate::hle_snapshot::AudioHleLane;
use crate::hle_transaction::{
    AudioHleTaskTransaction, AudioHleTransactionError, DmemByteRange, DmemWriteError, OwnedDmem,
};
use fn64_runtime::RspMemoryBank;

const COMPACT_COMMAND_DMEM_START: u16 = 0x02b0;
const COMPACT_COMMAND_BATCH_BYTES: usize = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompactAbiCommand {
    Memory(CompactMemoryCommand),
    Dsp(CompactDspCommand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactAbiDecodeError {
    Memory(CompactMemoryDecodeError),
    Dsp(CompactDspDecodeError),
}

impl fmt::Display for CompactAbiDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(source) => source.fmt(f),
            Self::Dsp(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for CompactAbiDecodeError {}

pub fn decode_compact_abi(command: AbiCommand) -> Result<CompactAbiCommand, CompactAbiDecodeError> {
    match command.opcode() {
        0x02 | 0x04 | 0x06 | 0x0a | 0x0b => decode_compact_memory(command)
            .map(CompactAbiCommand::Memory)
            .map_err(CompactAbiDecodeError::Memory),
        0x0c..=0x0e => decode_compact_dsp(command)
            .map(CompactAbiCommand::Dsp)
            .map_err(CompactAbiDecodeError::Dsp),
        _ => decode_compact_memory(command)
            .map(CompactAbiCommand::Memory)
            .map_err(CompactAbiDecodeError::Memory),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompactAbiExecutionError {
    Memory(CompactMemoryExecutionError),
    Dsp(DmemWriteError),
}

impl fmt::Display for CompactAbiExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(source) => source.fmt(f),
            Self::Dsp(source) => write!(f, "compact audio DSP DMEM write failed: {source:?}"),
        }
    }
}

impl std::error::Error for CompactAbiExecutionError {}

pub fn execute_compact_abi(
    command: CompactAbiCommand,
    dmem: &mut OwnedDmem,
    transaction: &mut AudioHleTaskTransaction<'_>,
) -> Result<(), CompactAbiExecutionError> {
    match command {
        CompactAbiCommand::Memory(command) => execute_compact_memory(command, dmem, transaction)
            .map_err(CompactAbiExecutionError::Memory),
        CompactAbiCommand::Dsp(command) => {
            execute_compact_dsp(command, dmem).map_err(CompactAbiExecutionError::Dsp)
        }
    }
}

#[derive(Debug)]
pub struct CompactAudioTaskExecution {
    selection: AudioHleSelection,
    decoded_commands: usize,
    dmem: OwnedDmem,
    rdram_patches: CanonicalRdramPatches,
}

impl CompactAudioTaskExecution {
    pub const fn selection(&self) -> AudioHleSelection {
        self.selection
    }

    pub const fn decoded_commands(&self) -> usize {
        self.decoded_commands
    }

    pub const fn dmem(&self) -> &OwnedDmem {
        &self.dmem
    }

    pub const fn rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.rdram_patches
    }
}

#[derive(Debug)]
pub enum CompactAudioTaskError {
    Decode {
        command_index: usize,
        source: AdmittedCompactAbiDecodeError,
    },
    Execute {
        command_index: usize,
        source: CompactAbiExecutionError,
    },
    Patches(AudioHleTransactionError),
    CommandStaging(DmemWriteError),
}

impl fmt::Display for CompactAudioTaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode {
                command_index,
                source,
            } => write!(f, "compact audio command {command_index}: {source}"),
            Self::Execute {
                command_index,
                source,
            } => write!(
                f,
                "compact audio command {command_index} execution failed: {source}"
            ),
            Self::Patches(source) => {
                write!(f, "compact audio patch extraction failed: {source:?}")
            }
            Self::CommandStaging(source) => {
                write!(f, "compact audio command staging failed: {source:?}")
            }
        }
    }
}

impl std::error::Error for CompactAudioTaskError {}

/// Execute one exactly admitted compact command list without live mutation.
pub fn execute_compact_audio_lane(
    lane: AudioHleLane,
) -> Result<CompactAudioTaskExecution, CompactAudioTaskError> {
    let snapshot = lane.snapshot();
    let admission = snapshot.admission();
    let selection = snapshot.selection();
    let command_bytes = snapshot.entry().command_bytes();
    let mut dmem = OwnedDmem::new(*snapshot.entry().rsp_memory().bank(RspMemoryBank::Dmem));
    let mut transaction = lane.hle_transaction();

    let mut command_index = 0;
    for batch in command_bytes.chunks(COMPACT_COMMAND_BATCH_BYTES) {
        let staging = DmemByteRange::new(
            COMPACT_COMMAND_DMEM_START,
            u16::try_from(batch.len()).expect("compact command batch fits u16"),
        )
        .expect("nonempty compact command batch fits its staging window");
        dmem.write_range(staging, batch)
            .map_err(CompactAudioTaskError::CommandStaging)?;
        for bytes in batch.chunks_exact(8) {
            let wire = AbiCommand::new(
                u32::from_be_bytes(bytes[..4].try_into().expect("four-byte command word")),
                u32::from_be_bytes(bytes[4..].try_into().expect("four-byte command word")),
            );
            let command = admission.decode_compact_abi(wire).map_err(|source| {
                CompactAudioTaskError::Decode {
                    command_index,
                    source,
                }
            })?;
            execute_compact_abi(command, &mut dmem, &mut transaction).map_err(|source| {
                CompactAudioTaskError::Execute {
                    command_index,
                    source,
                }
            })?;
            command_index += 1;
        }
    }
    let rdram_patches = transaction
        .canonical_patches()
        .map_err(CompactAudioTaskError::Patches)?;
    Ok(CompactAudioTaskExecution {
        selection,
        decoded_commands: command_index,
        dmem,
        rdram_patches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    use fn64_runtime::RdramView;

    #[test]
    fn dispatcher_covers_every_characterized_selector_and_traps_the_rest() {
        for opcode in [2, 4, 6, 10, 11, 12, 13, 14] {
            let command = match opcode {
                2 => AbiCommand::new(2 << 24, 1),
                4 | 6 => AbiCommand::new((opcode << 24) | (1 << 16), 0),
                10 => AbiCommand::new(10 << 24, 1),
                11 => AbiCommand::new((11 << 24) | 8, 0),
                12 => AbiCommand::new((12 << 24) | 0x4000, 0),
                13 => AbiCommand::new(13 << 24, 0),
                14 => AbiCommand::new(14 << 24, 0x1234),
                _ => unreachable!(),
            };
            assert!(decode_compact_abi(command).is_ok(), "opcode {opcode:#x}");
        }
        assert!(decode_compact_abi(AbiCommand::new(3 << 24, 0)).is_err());
    }

    #[test]
    fn composed_executor_shares_one_transaction_and_dmem_owner() {
        let storage = vec![0; DEFAULT_RDRAM_SIZE];
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        let mut dmem = OwnedDmem::default();
        let set = decode_compact_abi(AbiCommand::new(14 << 24, 0x1234)).unwrap();
        execute_compact_abi(set, &mut dmem, &mut transaction).unwrap();
        assert_eq!(&dmem.image()[0x0fea..0x0fec], &[0x12, 0x34]);
        assert_eq!(transaction.written_byte_count(), 0);
    }
}
