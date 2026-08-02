//! Exact-image-characterized compact audio DSP commands.
//!
//! This is not a family detector. The fixed INTERLEAVE geometry below comes
//! from a private same-snapshot LLE dependency sweep of one exact task-entry
//! identity. Catalog admission must still bind the complete microcode identity
//! before a caller may select this grammar.

use core::fmt;

use crate::hle::AbiCommand;
use crate::hle_transaction::{DmemByteRange, DmemWriteError, OwnedDmem};

const DMEM_AUDIO_BASE: u16 = 0x04f0;
const DSP_BUFFER_BYTES: u16 = 368;
const INTERLEAVE_CHANNEL_BYTES: u16 = DSP_BUFFER_BYTES;
const INTERLEAVE_OUTPUT_BYTES: u16 = INTERLEAVE_CHANNEL_BYTES * 2;
const INTERLEAVE_LEFT_OFFSET: u16 = 1248;
const INTERLEAVE_RIGHT_OFFSET: u16 = 1616;
const SCALAR_STATE_DMEM: u16 = 0x0fea;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompactInterleaveCommand {
    output: DmemByteRange,
    left: DmemByteRange,
    right: DmemByteRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompactMixerCommand {
    gain: i16,
    input: DmemByteRange,
    output: DmemByteRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompactSetScalarCommand {
    value: u16,
}

impl CompactSetScalarCommand {
    pub const fn value(self) -> u16 {
        self.value
    }
}

impl CompactMixerCommand {
    pub const fn gain(self) -> i16 {
        self.gain
    }

    pub const fn ranges(self) -> (DmemByteRange, DmemByteRange) {
        (self.input, self.output)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompactDspCommand {
    Mixer(CompactMixerCommand),
    Interleave(CompactInterleaveCommand),
    SetScalar(CompactSetScalarCommand),
}

impl CompactInterleaveCommand {
    pub const fn output(self) -> DmemByteRange {
        self.output
    }

    pub const fn channels(self) -> (DmemByteRange, DmemByteRange) {
        (self.left, self.right)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactDspDecodeError {
    WrongOpcode {
        opcode: u8,
    },
    NonzeroOperand {
        word0_payload: u32,
        word1: u32,
    },
    MixerReservedBits {
        reserved_bits: u32,
    },
    MixerRange {
        offset: u16,
        source: crate::hle_transaction::DmemRangeError,
    },
}

impl fmt::Display for CompactDspDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::WrongOpcode { opcode } => {
                write!(f, "compact INTERLEAVE decoder received opcode {opcode:#04x}")
            }
            Self::NonzeroOperand {
                word0_payload,
                word1,
            } => write!(
                f,
                "compact INTERLEAVE nonzero operands are not characterized: word0 payload {word0_payload:#08x}, word1 {word1:#010x}"
            ),
            Self::MixerReservedBits { reserved_bits } => write!(
                f,
                "compact MIXER reserved bits {reserved_bits:#08x} are not characterized"
            ),
            Self::MixerRange { offset, source } => write!(
                f,
                "compact MIXER buffer at offset {offset:#06x} is invalid: {source:?}"
            ),
        }
    }
}

impl std::error::Error for CompactDspDecodeError {}

pub fn decode_compact_dsp(command: AbiCommand) -> Result<CompactDspCommand, CompactDspDecodeError> {
    match command.opcode() {
        0x0c => decode_compact_mixer(command).map(CompactDspCommand::Mixer),
        0x0d => decode_compact_interleave(command).map(CompactDspCommand::Interleave),
        0x0e => Ok(CompactDspCommand::SetScalar(CompactSetScalarCommand {
            value: command.w1 as u16,
        })),
        opcode => Err(CompactDspDecodeError::WrongOpcode { opcode }),
    }
}

pub fn decode_compact_mixer(
    command: AbiCommand,
) -> Result<CompactMixerCommand, CompactDspDecodeError> {
    if command.opcode() != 0x0c {
        return Err(CompactDspDecodeError::WrongOpcode {
            opcode: command.opcode(),
        });
    }
    let reserved_bits = command.w0 & 0x00ff_0000;
    if reserved_bits != 0 {
        return Err(CompactDspDecodeError::MixerReservedBits { reserved_bits });
    }
    let input_offset = (command.w1 >> 16) as u16;
    let output_offset = command.w1 as u16;
    Ok(CompactMixerCommand {
        gain: command.w0 as u16 as i16,
        input: checked_range(input_offset, DSP_BUFFER_BYTES)?,
        output: checked_range(output_offset, DSP_BUFFER_BYTES)?,
    })
}

pub fn decode_compact_interleave(
    command: AbiCommand,
) -> Result<CompactInterleaveCommand, CompactDspDecodeError> {
    if command.opcode() != 0x0d {
        return Err(CompactDspDecodeError::WrongOpcode {
            opcode: command.opcode(),
        });
    }
    let word0_payload = command.w0 & 0x00ff_ffff;
    if word0_payload != 0 || command.w1 != 0 {
        return Err(CompactDspDecodeError::NonzeroOperand {
            word0_payload,
            word1: command.w1,
        });
    }
    Ok(CompactInterleaveCommand {
        output: exact_range(0, INTERLEAVE_OUTPUT_BYTES),
        left: exact_range(INTERLEAVE_LEFT_OFFSET, INTERLEAVE_CHANNEL_BYTES),
        right: exact_range(INTERLEAVE_RIGHT_OFFSET, INTERLEAVE_CHANNEL_BYTES),
    })
}

fn exact_range(offset: u16, byte_len: u16) -> DmemByteRange {
    DmemByteRange::new(
        DMEM_AUDIO_BASE
            .checked_add(offset)
            .expect("characterized compact DSP offset fits DMEM address"),
        byte_len,
    )
    .expect("characterized compact DSP range fits DMEM")
}

fn checked_range(offset: u16, byte_len: u16) -> Result<DmemByteRange, CompactDspDecodeError> {
    let start = DMEM_AUDIO_BASE
        .checked_add(offset)
        .ok_or(CompactDspDecodeError::MixerRange {
            offset,
            source: crate::hle_transaction::DmemRangeError::OutOfBounds {
                start: offset,
                byte_len,
            },
        })?;
    DmemByteRange::new(start, byte_len)
        .map_err(|source| CompactDspDecodeError::MixerRange { offset, source })
}

pub fn execute_compact_dsp(
    command: CompactDspCommand,
    dmem: &mut OwnedDmem,
) -> Result<(), DmemWriteError> {
    match command {
        CompactDspCommand::Mixer(command) => execute_compact_mixer(command, dmem),
        CompactDspCommand::Interleave(command) => execute_compact_interleave(command, dmem),
        CompactDspCommand::SetScalar(command) => {
            let range = DmemByteRange::new(SCALAR_STATE_DMEM, 2)
                .expect("characterized compact scalar state fits DMEM");
            dmem.write_range(range, &command.value.to_be_bytes())
        }
    }
}

pub fn execute_compact_mixer(
    command: CompactMixerCommand,
    dmem: &mut OwnedDmem,
) -> Result<(), DmemWriteError> {
    let mut input = [0; DSP_BUFFER_BYTES as usize];
    input.copy_from_slice(dmem.read_range(command.input));
    let mut output = [0; DSP_BUFFER_BYTES as usize];
    output.copy_from_slice(dmem.read_range(command.output));
    for (input_sample, output_sample) in input.chunks_exact(2).zip(output.chunks_exact_mut(2)) {
        let input = i16::from_be_bytes(input_sample.try_into().expect("two-byte input sample"));
        let prior = i16::from_be_bytes(
            (&*output_sample)
                .try_into()
                .expect("two-byte output sample"),
        );
        let accumulator =
            i64::from(prior) * 32_767 * 2 + 0x8000 + i64::from(input) * i64::from(command.gain) * 2;
        let mixed = (accumulator >> 16).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        output_sample.copy_from_slice(&mixed.to_be_bytes());
    }
    dmem.write_range(command.output, &output)
}

pub fn execute_compact_interleave(
    command: CompactInterleaveCommand,
    dmem: &mut OwnedDmem,
) -> Result<(), DmemWriteError> {
    let mut left = [0; INTERLEAVE_CHANNEL_BYTES as usize];
    left.copy_from_slice(dmem.read_range(command.left));
    let mut right = [0; INTERLEAVE_CHANNEL_BYTES as usize];
    right.copy_from_slice(dmem.read_range(command.right));
    let mut output = [0; INTERLEAVE_OUTPUT_BYTES as usize];
    for ((destination, left_sample), right_sample) in output
        .chunks_exact_mut(4)
        .zip(left.chunks_exact(2))
        .zip(right.chunks_exact(2))
    {
        destination[..2].copy_from_slice(left_sample);
        destination[2..].copy_from_slice(right_sample);
    }
    dmem.write_range(command.output, &output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_zero_operand_packet_decodes_to_characterized_ranges() {
        let command = decode_compact_interleave(AbiCommand::new(0x0d00_0000, 0)).unwrap();
        assert_eq!(command.output(), DmemByteRange::new(0x04f0, 736).unwrap());
        assert_eq!(
            command.channels(),
            (
                DmemByteRange::new(0x09d0, 368).unwrap(),
                DmemByteRange::new(0x0b40, 368).unwrap(),
            )
        );
    }

    #[test]
    fn other_opcodes_and_nonzero_operands_are_rejected() {
        assert_eq!(
            decode_compact_interleave(AbiCommand::new(0x0c00_0000, 0)),
            Err(CompactDspDecodeError::WrongOpcode { opcode: 0x0c })
        );
        assert!(matches!(
            decode_compact_interleave(AbiCommand::new(0x0d00_0001, 0)),
            Err(CompactDspDecodeError::NonzeroOperand { .. })
        ));
        assert!(matches!(
            decode_compact_interleave(AbiCommand::new(0x0d00_0000, 1)),
            Err(CompactDspDecodeError::NonzeroOperand { .. })
        ));
    }

    #[test]
    fn mixer_decodes_signed_gain_and_compact_ranges() {
        let command = decode_compact_mixer(AbiCommand::new(0x0c00_d99a, 0x0000_0170)).unwrap();
        assert_eq!(command.gain(), -9830);
        assert_eq!(
            command.ranges(),
            (
                DmemByteRange::new(0x04f0, 368).unwrap(),
                DmemByteRange::new(0x0660, 368).unwrap(),
            )
        );
        assert!(matches!(
            decode_compact_mixer(AbiCommand::new(0x0c01_0000, 0)),
            Err(CompactDspDecodeError::MixerReservedBits { .. })
        ));
    }

    #[test]
    fn mixer_uses_fractional_output_initialization_then_unrounded_mac() {
        let command = decode_compact_mixer(AbiCommand::new(0x0c00_2666, 0x0170_0000)).unwrap();
        let mut dmem = OwnedDmem::default();
        let (input, output) = command.ranges();
        let input_bytes: Vec<u8> = (17u8..=32).cycle().take(368).collect();
        let output_bytes: Vec<u8> = (1u8..=16).cycle().take(368).collect();
        dmem.write_range(input, &input_bytes).unwrap();
        dmem.write_range(output, &output_bytes).unwrap();

        execute_compact_mixer(command, &mut dmem).unwrap();

        assert_eq!(
            &dmem.read_range(output)[..16],
            &[
                0x06, 0x21, 0x08, 0xbd, 0x0b, 0x59, 0x0d, 0xf5, 0x10, 0x92, 0x13, 0x2e, 0x15, 0xca,
                0x18, 0x66
            ]
        );
    }

    #[test]
    fn opcode_fourteen_ignores_other_fields_and_sets_one_scalar_halfword() {
        let command = decode_compact_dsp(AbiCommand::new(0x0eff_ffff, 0xabcd_4770)).unwrap();
        assert_eq!(
            command,
            CompactDspCommand::SetScalar(CompactSetScalarCommand { value: 0x4770 })
        );
        let mut dmem = OwnedDmem::default();
        execute_compact_dsp(command, &mut dmem).unwrap();
        assert_eq!(
            dmem.read_range(DmemByteRange::new(0x0fea, 2).unwrap()),
            &[0x47, 0x70]
        );
    }

    #[test]
    fn execution_interleaves_big_endian_halfword_images_and_nothing_else() {
        let command = decode_compact_interleave(AbiCommand::new(0x0d00_0000, 0)).unwrap();
        let mut dmem = OwnedDmem::default();
        let left: Vec<u8> = (0..184u16)
            .flat_map(|sample| sample.to_be_bytes())
            .collect();
        let right: Vec<u8> = (0..184u16)
            .flat_map(|sample| (0x8000 | sample).to_be_bytes())
            .collect();
        let (left_range, right_range) = command.channels();
        dmem.write_range(left_range, &left).unwrap();
        dmem.write_range(right_range, &right).unwrap();

        execute_compact_interleave(command, &mut dmem).unwrap();

        let output = dmem.read_range(command.output());
        for (sample, frame) in output.chunks_exact(4).enumerate() {
            assert_eq!(&frame[..2], &left[sample * 2..sample * 2 + 2]);
            assert_eq!(&frame[2..], &right[sample * 2..sample * 2 + 2]);
        }
        assert_eq!(dmem.read_range(left_range), left);
        assert_eq!(dmem.read_range(right_range), right);
    }
}
