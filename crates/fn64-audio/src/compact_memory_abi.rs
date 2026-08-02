//! Exact-image-characterized compact audio memory commands.
//!
//! This is not a family detector. A private same-snapshot LLE matrix for one
//! exact task-entry identity established the load/save shapes below across
//! every such occurrence in one real 164-command task. A separately seeded,
//! truncated same-image matrix established CLEAR's minimum block and rounding;
//! an adjacent matrix established DMEMMOVE's same rounding plus ascending
//! 16-byte block behavior under overlap. An address-redirection and count
//! matrix established LOADADPCM's fixed DMEM destination and raw DMA length.
//! The public SGI RSP Programmer's Guide supplies the SP DMA address and length
//! masking rules. Catalog admission must still bind the complete microcode
//! identity before a caller may select this grammar.

use core::fmt;

use fn64_runtime::RdramAddr;

use crate::hle::AbiCommand;
use crate::hle_transaction::{
    AudioHleTaskTransaction, AudioHleTransactionError, DmemByteRange, DmemRangeError,
    DmemWriteError, OwnedDmem,
};

const DMEM_MEMORY_BASE: u16 = 0x04f0;
const ADPCM_TABLE_DMEM_START: u16 = 0x03f0;
const DMA_ADDRESS_MASK: u32 = 0x00ff_fff8;
const DMA_TRANSFER_QUANTUM_BYTES: u32 = 8;
const TRANSFER_QUANTUM_BYTES: u16 = 16;
const ZERO_DMEM: [u8; crate::hle_outcome::RSP_BANK_BYTES] = [0; crate::hle_outcome::RSP_BANK_BYTES];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompactMemoryOpcode {
    ClearBuffer,
    LoadBuffer,
    SaveBuffer,
    DmemMove,
    LoadAdpcm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompactMemoryCommand {
    ClearBuffer {
        dmem: DmemByteRange,
    },
    LoadBuffer {
        rdram: RdramAddr,
        dmem: DmemByteRange,
    },
    SaveBuffer {
        rdram: RdramAddr,
        dmem: DmemByteRange,
    },
    DmemMove {
        input: DmemByteRange,
        output: DmemByteRange,
    },
    LoadAdpcm {
        rdram: RdramAddr,
        table: DmemByteRange,
    },
}

impl CompactMemoryCommand {
    pub const fn opcode(self) -> CompactMemoryOpcode {
        match self {
            Self::ClearBuffer { .. } => CompactMemoryOpcode::ClearBuffer,
            Self::LoadBuffer { .. } => CompactMemoryOpcode::LoadBuffer,
            Self::SaveBuffer { .. } => CompactMemoryOpcode::SaveBuffer,
            Self::DmemMove { .. } => CompactMemoryOpcode::DmemMove,
            Self::LoadAdpcm { .. } => CompactMemoryOpcode::LoadAdpcm,
        }
    }

    pub const fn rdram(self) -> Option<RdramAddr> {
        match self {
            Self::ClearBuffer { .. } | Self::DmemMove { .. } => None,
            Self::LoadBuffer { rdram, .. } | Self::SaveBuffer { rdram, .. } => Some(rdram),
            Self::LoadAdpcm { rdram, .. } => Some(rdram),
        }
    }

    pub const fn memory_range(self) -> Option<DmemByteRange> {
        match self {
            Self::ClearBuffer { dmem }
            | Self::LoadBuffer { dmem, .. }
            | Self::SaveBuffer { dmem, .. } => Some(dmem),
            Self::LoadAdpcm { table, .. } => Some(table),
            Self::DmemMove { .. } => None,
        }
    }

    pub const fn move_ranges(self) -> Option<(DmemByteRange, DmemByteRange)> {
        match self {
            Self::DmemMove { input, output } => Some((input, output)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactMemoryDecodeError {
    UnsupportedOpcode {
        opcode: u8,
    },
    OutsideCharacterizedShape {
        opcode: CompactMemoryOpcode,
        reserved_bits: u32,
    },
    ZeroLengthUncharacterized {
        opcode: CompactMemoryOpcode,
    },
    DmemAddressOverflow {
        offset: u16,
    },
    DmemRange(DmemRangeError),
}

impl fmt::Display for CompactMemoryDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnsupportedOpcode { opcode } => {
                write!(
                    f,
                    "compact audio DMA opcode {opcode:#04x} is not characterized"
                )
            }
            Self::OutsideCharacterizedShape {
                opcode,
                reserved_bits,
            } => write!(
                f,
                "compact audio memory {opcode:?} reserved bits {reserved_bits:#010x} are not characterized"
            ),
            Self::ZeroLengthUncharacterized { opcode } => {
                write!(
                    f,
                    "compact audio DMA {opcode:?} zero-length behavior is not characterized"
                )
            }
            Self::DmemAddressOverflow { offset } => write!(
                f,
                "compact audio DMA DMEM base plus wire offset {offset:#06x} overflows"
            ),
            Self::DmemRange(source) => write!(f, "compact audio DMA range is invalid: {source:?}"),
        }
    }
}

impl std::error::Error for CompactMemoryDecodeError {}

pub fn decode_compact_memory(
    command: AbiCommand,
) -> Result<CompactMemoryCommand, CompactMemoryDecodeError> {
    let opcode = match command.opcode() {
        0x02 => CompactMemoryOpcode::ClearBuffer,
        0x04 => CompactMemoryOpcode::LoadBuffer,
        0x06 => CompactMemoryOpcode::SaveBuffer,
        0x0a => CompactMemoryOpcode::DmemMove,
        0x0b => CompactMemoryOpcode::LoadAdpcm,
        opcode => {
            fn64_runtime::record_unsupported_event(
                fn64_runtime::UnsupportedSubsystem::Audio,
                "audio.hle.compact-memory-unsupported-opcode",
                format!("compact audio DMA reached uncharacterized selector {opcode:#04x}"),
                None,
                fn64_runtime::UnsupportedDisposition::ReturnedError,
            );
            return Err(CompactMemoryDecodeError::UnsupportedOpcode { opcode });
        }
    };
    if matches!(
        opcode,
        CompactMemoryOpcode::ClearBuffer | CompactMemoryOpcode::DmemMove
    ) {
        let reserved_bits = command.w0 & 0x00ff_0000;
        let reserved_bits = if opcode == CompactMemoryOpcode::ClearBuffer {
            reserved_bits | (command.w1 & 0xffff_0000)
        } else {
            reserved_bits
        };
        if reserved_bits != 0 {
            return Err(CompactMemoryDecodeError::OutsideCharacterizedShape {
                opcode,
                reserved_bits,
            });
        }
        let count = command.w1 as u16;
        let byte_len = u32::from(count)
            .max(1)
            .div_ceil(u32::from(TRANSFER_QUANTUM_BYTES))
            * u32::from(TRANSFER_QUANTUM_BYTES);
        let byte_len = u16::try_from(byte_len).map_err(|_| {
            CompactMemoryDecodeError::DmemRange(DmemRangeError::OutOfBounds {
                start: command.w0 as u16,
                byte_len: u16::MAX,
            })
        })?;
        let input = compact_dmem_range(command.w0 as u16, byte_len)?;
        return Ok(if opcode == CompactMemoryOpcode::ClearBuffer {
            CompactMemoryCommand::ClearBuffer { dmem: input }
        } else {
            let output = compact_dmem_range((command.w1 >> 16) as u16, byte_len)?;
            CompactMemoryCommand::DmemMove { input, output }
        });
    }
    if opcode == CompactMemoryOpcode::LoadAdpcm {
        let count = command.w0 & 0x00ff_ffff;
        if count == 0 {
            return Err(CompactMemoryDecodeError::ZeroLengthUncharacterized { opcode });
        }
        let byte_len = count
            .checked_add(DMA_TRANSFER_QUANTUM_BYTES - 1)
            .expect("24-bit wire count plus seven cannot overflow")
            & !(DMA_TRANSFER_QUANTUM_BYTES - 1);
        let byte_len = u16::try_from(byte_len).map_err(|_| {
            CompactMemoryDecodeError::DmemRange(DmemRangeError::OutOfBounds {
                start: ADPCM_TABLE_DMEM_START,
                byte_len: u16::MAX,
            })
        })?;
        let table = DmemByteRange::new(ADPCM_TABLE_DMEM_START, byte_len)
            .map_err(CompactMemoryDecodeError::DmemRange)?;
        return Ok(CompactMemoryCommand::LoadAdpcm {
            rdram: RdramAddr::from_offset(command.w1 & DMA_ADDRESS_MASK),
            table,
        });
    }
    let transfer_quanta = ((command.w0 >> 16) & 0xff) as u16;
    if transfer_quanta == 0 {
        return Err(CompactMemoryDecodeError::ZeroLengthUncharacterized { opcode });
    }
    let byte_len = transfer_quanta * TRANSFER_QUANTUM_BYTES;
    let offset = command.w0 as u16;
    let dmem = compact_dmem_range(offset, byte_len)?;
    let rdram = RdramAddr::from_offset(command.w1 & DMA_ADDRESS_MASK);
    Ok(match opcode {
        CompactMemoryOpcode::LoadBuffer => CompactMemoryCommand::LoadBuffer { rdram, dmem },
        CompactMemoryOpcode::SaveBuffer => CompactMemoryCommand::SaveBuffer { rdram, dmem },
        CompactMemoryOpcode::ClearBuffer | CompactMemoryOpcode::DmemMove => {
            unreachable!("non-DMA memory command returned above")
        }
        CompactMemoryOpcode::LoadAdpcm => unreachable!("LOADADPCM returned above"),
    })
}

fn compact_dmem_range(
    offset: u16,
    byte_len: u16,
) -> Result<DmemByteRange, CompactMemoryDecodeError> {
    let dmem_start = DMEM_MEMORY_BASE
        .checked_add(offset)
        .ok_or(CompactMemoryDecodeError::DmemAddressOverflow { offset })?;
    let dmem =
        DmemByteRange::new(dmem_start, byte_len).map_err(CompactMemoryDecodeError::DmemRange)?;
    Ok(dmem)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompactMemoryExecutionError {
    Transaction(AudioHleTransactionError),
    DmemWrite(DmemWriteError),
}

impl fmt::Display for CompactMemoryExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transaction(source) => {
                write!(f, "compact audio DMA RDRAM access failed: {source:?}")
            }
            Self::DmemWrite(source) => write!(f, "compact audio DMA DMEM write failed: {source:?}"),
        }
    }
}

impl std::error::Error for CompactMemoryExecutionError {}

/// Execute one fully decoded transfer against speculative state only.
///
/// Decode proves the complete DMEM range. Transaction methods preflight the
/// complete physical RDRAM range before staging any byte, so every error is
/// side-effect free.
pub fn execute_compact_memory(
    command: CompactMemoryCommand,
    dmem: &mut OwnedDmem,
    transaction: &mut AudioHleTaskTransaction<'_>,
) -> Result<(), CompactMemoryExecutionError> {
    match command {
        CompactMemoryCommand::ClearBuffer { dmem: range } => dmem
            .write_range(range, &ZERO_DMEM[..usize::from(range.byte_len())])
            .map_err(CompactMemoryExecutionError::DmemWrite),
        CompactMemoryCommand::LoadBuffer { rdram, dmem: range } => {
            let bytes = transaction
                .read_bytes(rdram, u32::from(range.byte_len()))
                .map_err(CompactMemoryExecutionError::Transaction)?;
            dmem.write_range(range, &bytes)
                .map_err(CompactMemoryExecutionError::DmemWrite)
        }
        CompactMemoryCommand::LoadAdpcm {
            rdram,
            table: range,
        } => {
            let bytes = transaction
                .read_bytes(rdram, u32::from(range.byte_len()))
                .map_err(CompactMemoryExecutionError::Transaction)?;
            dmem.write_range(range, &bytes)
                .map_err(CompactMemoryExecutionError::DmemWrite)
        }
        CompactMemoryCommand::SaveBuffer { rdram, dmem: range } => transaction
            .write_bytes(rdram, dmem.read_range(range))
            .map_err(CompactMemoryExecutionError::Transaction),
        CompactMemoryCommand::DmemMove { input, output } => {
            for byte_offset in (0..input.byte_len()).step_by(usize::from(TRANSFER_QUANTUM_BYTES)) {
                let source =
                    DmemByteRange::new(input.start() + byte_offset, TRANSFER_QUANTUM_BYTES)
                        .expect("decoded blockwise move source stays in its proven range");
                let destination =
                    DmemByteRange::new(output.start() + byte_offset, TRANSFER_QUANTUM_BYTES)
                        .expect("decoded blockwise move destination stays in its proven range");
                let mut block = [0; TRANSFER_QUANTUM_BYTES as usize];
                block.copy_from_slice(dmem.read_range(source));
                dmem.write_range(destination, &block)
                    .map_err(CompactMemoryExecutionError::DmemWrite)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    use fn64_runtime::{RdramView, RdramViewMut};

    fn packet(opcode: u8, quanta: u8, dmem_offset: u16, rdram: u32) -> AbiCommand {
        AbiCommand::new(
            (u32::from(opcode) << 24) | (u32::from(quanta) << 16) | u32::from(dmem_offset),
            rdram,
        )
    }

    #[test]
    fn decode_matches_the_characterized_compact_memory_wire() {
        let load = decode_compact_memory(packet(4, 23, 0, 0x0170_0007)).unwrap();
        assert_eq!(load.opcode(), CompactMemoryOpcode::LoadBuffer);
        assert_eq!(load.rdram().unwrap().offset(), 0x0070_0000);
        assert_eq!(
            load.memory_range().unwrap(),
            DmemByteRange::new(0x04f0, 368).unwrap()
        );

        let save = decode_compact_memory(packet(6, 46, 368, 0x8070_0005)).unwrap();
        assert_eq!(save.opcode(), CompactMemoryOpcode::SaveBuffer);
        assert_eq!(save.rdram().unwrap().offset(), 0x0070_0000);
        assert_eq!(
            save.memory_range().unwrap(),
            DmemByteRange::new(0x0660, 736).unwrap()
        );

        for (count, expected_bytes) in [(0, 16), (1, 16), (16, 16), (17, 32), (368, 368)] {
            let clear = decode_compact_memory(packet(2, 0, 2352, count)).unwrap();
            assert_eq!(clear.opcode(), CompactMemoryOpcode::ClearBuffer);
            assert_eq!(clear.rdram(), None);
            assert_eq!(
                clear.memory_range().unwrap(),
                DmemByteRange::new(0x0e20, expected_bytes).unwrap()
            );
        }

        let moved =
            decode_compact_memory(AbiCommand::new((10 << 24) | 2352, (1984 << 16) | 369)).unwrap();
        assert_eq!(moved.opcode(), CompactMemoryOpcode::DmemMove);
        assert_eq!(
            moved.move_ranges(),
            Some((
                DmemByteRange::new(0x0e20, 384).unwrap(),
                DmemByteRange::new(0x0cb0, 384).unwrap(),
            ))
        );

        let table = decode_compact_memory(packet(0x0b, 0, 32, 0x8070_0007)).unwrap();
        assert_eq!(table.opcode(), CompactMemoryOpcode::LoadAdpcm);
        assert_eq!(table.rdram().unwrap().offset(), 0x0070_0000);
        assert_eq!(
            table.memory_range().unwrap(),
            DmemByteRange::new(0x03f0, 32).unwrap()
        );
        let rounded_table = decode_compact_memory(packet(0x0b, 0, 33, 0x100)).unwrap();
        assert_eq!(
            rounded_table.memory_range().unwrap(),
            DmemByteRange::new(0x03f0, 40).unwrap()
        );
    }

    #[test]
    fn unsupported_zero_and_out_of_range_shapes_are_loud() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        assert_eq!(
            decode_compact_memory(packet(3, 1, 0, 0)).unwrap_err(),
            CompactMemoryDecodeError::UnsupportedOpcode { opcode: 3 }
        );
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].operation,
            "audio.hle.compact-memory-unsupported-opcode"
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(
            decode_compact_memory(packet(4, 0, 0, 0)).unwrap_err(),
            CompactMemoryDecodeError::ZeroLengthUncharacterized {
                opcode: CompactMemoryOpcode::LoadBuffer
            }
        );
        assert_eq!(
            decode_compact_memory(packet(0x0b, 0, 0, 0)).unwrap_err(),
            CompactMemoryDecodeError::ZeroLengthUncharacterized {
                opcode: CompactMemoryOpcode::LoadAdpcm
            }
        );
        assert!(matches!(
            decode_compact_memory(packet(6, 255, u16::MAX, 0)),
            Err(CompactMemoryDecodeError::DmemAddressOverflow { .. })
        ));
        assert!(matches!(
            decode_compact_memory(packet(4, 255, 0, 0)),
            Err(CompactMemoryDecodeError::DmemRange(_))
        ));
        assert!(matches!(
            decode_compact_memory(packet(2, 1, 0, 16)),
            Err(CompactMemoryDecodeError::OutsideCharacterizedShape { .. })
        ));
    }

    #[test]
    fn memory_commands_use_logical_bytes_and_transactional_writes() {
        let mut storage = vec![0; DEFAULT_RDRAM_SIZE];
        let source: Vec<u8> = (0..32).map(|index| 0x40 | index).collect();
        RdramViewMut::from_storage(&mut storage)
            .write_logical_bytes(RdramAddr::from_offset(0x200), &source);
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        let mut dmem = OwnedDmem::default();

        let load = decode_compact_memory(packet(4, 2, 0, 0x207)).unwrap();
        execute_compact_memory(load, &mut dmem, &mut transaction).unwrap();
        assert_eq!(dmem.read_range(load.memory_range().unwrap()), source);
        assert_eq!(transaction.written_byte_count(), 0);

        let clear = decode_compact_memory(packet(2, 0, 0, 17)).unwrap();
        execute_compact_memory(clear, &mut dmem, &mut transaction).unwrap();
        assert_eq!(dmem.read_range(clear.memory_range().unwrap()), &[0; 32]);

        let table_source: Vec<u8> = (0..40).map(|index| 0x80 | index).collect();
        let mut table_storage = vec![0; DEFAULT_RDRAM_SIZE];
        RdramViewMut::from_storage(&mut table_storage)
            .write_logical_bytes(RdramAddr::from_offset(0x600), &table_source);
        let mut table_transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&table_storage)).unwrap();
        let table = decode_compact_memory(packet(0x0b, 0, 33, 0x607)).unwrap();
        execute_compact_memory(table, &mut dmem, &mut table_transaction).unwrap();
        assert_eq!(dmem.read_range(table.memory_range().unwrap()), table_source);

        let save = decode_compact_memory(packet(6, 2, 0, 0x407)).unwrap();
        execute_compact_memory(save, &mut dmem, &mut transaction).unwrap();
        let patches = transaction.canonical_patches().unwrap();
        assert_eq!(patches.as_slice().len(), 1);
        assert_eq!(patches.as_slice()[0].range().start(), 0x400);
        assert_eq!(patches.as_slice()[0].bytes(), &[0; 32]);
    }

    #[test]
    fn rejected_rdram_range_cannot_mutate_dmem_or_stage_a_prefix() {
        let storage = vec![0; DEFAULT_RDRAM_SIZE];
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        let mut dmem = OwnedDmem::default();
        let before = dmem.clone();
        let load = decode_compact_memory(packet(4, 2, 0, 0x00ff_fff8)).unwrap();

        assert!(matches!(
            execute_compact_memory(load, &mut dmem, &mut transaction),
            Err(CompactMemoryExecutionError::Transaction(_))
        ));
        assert_eq!(dmem, before);
        assert_eq!(transaction.written_byte_count(), 0);
    }

    #[test]
    fn dmem_move_copies_ascending_sixteen_byte_blocks_under_forward_overlap() {
        let storage = vec![0; DEFAULT_RDRAM_SIZE];
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        let mut dmem = OwnedDmem::default();
        let source = DmemByteRange::new(0x0e20, 32).unwrap();
        let pattern: Vec<u8> = (0..32).collect();
        dmem.write_range(source, &pattern).unwrap();
        let command =
            decode_compact_memory(AbiCommand::new((10 << 24) | 2352, (2360 << 16) | 32)).unwrap();

        execute_compact_memory(command, &mut dmem, &mut transaction).unwrap();

        let (_, output) = command.move_ranges().unwrap();
        let mut expected = Vec::from(&pattern[..16]);
        expected.extend_from_slice(&pattern[8..16]);
        expected.extend_from_slice(&pattern[24..32]);
        assert_eq!(dmem.read_range(output), expected);
        assert_eq!(transaction.written_byte_count(), 0);
    }
}
