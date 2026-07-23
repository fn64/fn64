//! Typed wire decoder for the standard libultra audio binary interface.
//!
//! ## Provenance and boundary
//!
//! The opcode numbers, packet fields, flag values, and state sizes below come
//! only from the public libultra `abi.h` published with the N64 OS 2.0K/v5.2
//! manuals:
//! <https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/header/abi.htm>.
//!
//! This module describes a wire grammar, not a microcode classifier. A caller
//! may select it only after an exact admitted microcode text/data identity has
//! been mapped to this ABI family. An eight-byte stream containing these
//! opcode values is not identity evidence. The public header does not specify
//! the DSP arithmetic, rounding, saturation, filter coefficients, state
//! transitions, cycle costs, or malformed-packet behavior, so none is
//! implemented or inferred here.

use core::convert::TryFrom;
use core::fmt;

/// One standard audio command occupies two big-endian 32-bit words.
pub const STANDARD_ABI_COMMAND_BYTES: usize = 8;

/// Standard libultra audio-command flags.
///
/// Several names intentionally share a bit because `abi.h` assigns the bit a
/// command-specific meaning.
pub const A_INIT: u8 = 0x01;
pub const A_CONTINUE: u8 = 0x00;
pub const A_LOOP: u8 = 0x02;
pub const A_OUT: u8 = 0x02;
pub const A_LEFT: u8 = 0x02;
pub const A_RIGHT: u8 = 0x00;
pub const A_VOL: u8 = 0x04;
pub const A_RATE: u8 = 0x00;
pub const A_AUX: u8 = 0x08;
pub const A_NOAUX: u8 = 0x00;
pub const A_MAIN: u8 = 0x00;
pub const A_MIX: u8 = 0x10;

/// `ADPCMVSIZE` from the public header.
pub const ADPCM_VECTOR_I16_COUNT: usize = 8;
/// `ADPCMFSIZE` and therefore the length of `ADPCM_STATE`.
pub const ADPCM_STATE_I16_COUNT: usize = 16;
pub const ADPCM_STATE_BYTES: usize = ADPCM_STATE_I16_COUNT * size_of::<i16>();

pub const POLE_FILTER_STATE_I16_COUNT: usize = 4;
pub const POLE_FILTER_STATE_BYTES: usize = POLE_FILTER_STATE_I16_COUNT * size_of::<i16>();

pub const RESAMPLE_STATE_I16_COUNT: usize = 16;
pub const RESAMPLE_STATE_BYTES: usize = RESAMPLE_STATE_I16_COUNT * size_of::<i16>();

pub const ENV_MIX_STATE_I16_COUNT: usize = 40;
pub const ENV_MIX_STATE_BYTES: usize = ENV_MIX_STATE_I16_COUNT * size_of::<i16>();

pub const UNITY_PITCH: u16 = 0x8000;

/// The sixteen opcodes assigned by the standard libultra ABI header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StandardAbiOpcode {
    SpNoop = 0x00,
    Adpcm = 0x01,
    ClearBuffer = 0x02,
    EnvMixer = 0x03,
    LoadBuffer = 0x04,
    Resample = 0x05,
    SaveBuffer = 0x06,
    Segment = 0x07,
    SetBuffer = 0x08,
    SetVolume = 0x09,
    DmemMove = 0x0a,
    LoadAdpcm = 0x0b,
    Mixer = 0x0c,
    Interleave = 0x0d,
    PoleFilter = 0x0e,
    SetLoop = 0x0f,
}

/// A packet whose high command byte is not assigned by standard `abi.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownStandardAbiOpcode {
    pub opcode: u8,
}

impl fmt::Display for UnknownStandardAbiOpcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown standard audio ABI opcode {:#04x}", self.opcode)
    }
}

impl std::error::Error for UnknownStandardAbiOpcode {}

impl TryFrom<u8> for StandardAbiOpcode {
    type Error = UnknownStandardAbiOpcode;

    fn try_from(opcode: u8) -> Result<Self, Self::Error> {
        match opcode {
            0x00 => Ok(Self::SpNoop),
            0x01 => Ok(Self::Adpcm),
            0x02 => Ok(Self::ClearBuffer),
            0x03 => Ok(Self::EnvMixer),
            0x04 => Ok(Self::LoadBuffer),
            0x05 => Ok(Self::Resample),
            0x06 => Ok(Self::SaveBuffer),
            0x07 => Ok(Self::Segment),
            0x08 => Ok(Self::SetBuffer),
            0x09 => Ok(Self::SetVolume),
            0x0a => Ok(Self::DmemMove),
            0x0b => Ok(Self::LoadAdpcm),
            0x0c => Ok(Self::Mixer),
            0x0d => Ok(Self::Interleave),
            0x0e => Ok(Self::PoleFilter),
            0x0f => Ok(Self::SetLoop),
            opcode => Err(UnknownStandardAbiOpcode { opcode }),
        }
    }
}

/// The exact two-word representation retained alongside typed field decoding.
///
/// Retaining both words makes padding and currently uninterpreted bits
/// observable without assigning them behavior the public header does not
/// specify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StandardAbiPacket {
    pub w0: u32,
    pub w1: u32,
}

impl StandardAbiPacket {
    pub const fn new(w0: u32, w1: u32) -> Self {
        Self { w0, w1 }
    }

    pub fn from_be_bytes(bytes: [u8; STANDARD_ABI_COMMAND_BYTES]) -> Self {
        Self {
            w0: u32::from_be_bytes(bytes[0..4].try_into().expect("four command bytes")),
            w1: u32::from_be_bytes(bytes[4..8].try_into().expect("four command bytes")),
        }
    }

    pub fn to_be_bytes(self) -> [u8; STANDARD_ABI_COMMAND_BYTES] {
        let mut bytes = [0; STANDARD_ABI_COMMAND_BYTES];
        bytes[0..4].copy_from_slice(&self.w0.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.w1.to_be_bytes());
        bytes
    }

    pub const fn opcode_byte(self) -> u8 {
        (self.w0 >> 24) as u8
    }

    /// Decode fields defined by the public header without interpreting their
    /// arithmetic meaning.
    ///
    /// This operation does not classify the microcode family. Selection must
    /// already have been authorized by exact text/data identity admission.
    pub fn decode(self) -> Result<DecodedStandardAbiPacket, UnknownStandardAbiOpcode> {
        let opcode = StandardAbiOpcode::try_from(self.opcode_byte())?;
        let flags = ((self.w0 >> 16) & 0xff) as u8;
        let low16 = self.w0 as u16;
        let high16_w1 = (self.w1 >> 16) as u16;
        let low16_w1 = self.w1 as u16;

        let command = match opcode {
            StandardAbiOpcode::SpNoop => StandardAbiCommand::SpNoop,
            StandardAbiOpcode::Adpcm => StandardAbiCommand::Adpcm {
                flags,
                gain: low16,
                state_addr: self.w1,
            },
            StandardAbiOpcode::ClearBuffer => StandardAbiCommand::ClearBuffer {
                dmem: self.w0 & 0x00ff_ffff,
                count: self.w1,
            },
            StandardAbiOpcode::EnvMixer => StandardAbiCommand::EnvMixer {
                flags,
                state_addr: self.w1,
            },
            StandardAbiOpcode::LoadBuffer => StandardAbiCommand::LoadBuffer {
                source_addr: self.w1,
            },
            StandardAbiOpcode::Resample => StandardAbiCommand::Resample {
                flags,
                pitch: low16,
                state_addr: self.w1,
            },
            StandardAbiOpcode::SaveBuffer => StandardAbiCommand::SaveBuffer {
                destination_addr: self.w1,
            },
            StandardAbiOpcode::Segment => StandardAbiCommand::Segment {
                segment: (self.w1 >> 24) as u8,
                base: self.w1 & 0x00ff_ffff,
            },
            StandardAbiOpcode::SetBuffer => StandardAbiCommand::SetBuffer {
                flags,
                input: low16,
                output: high16_w1,
                count: low16_w1,
            },
            StandardAbiOpcode::SetVolume => StandardAbiCommand::SetVolume {
                flags,
                volume: low16,
                target: high16_w1,
                rate: low16_w1,
            },
            StandardAbiOpcode::DmemMove => StandardAbiCommand::DmemMove {
                input: self.w0 & 0x00ff_ffff,
                output: high16_w1,
                count: low16_w1,
            },
            StandardAbiOpcode::LoadAdpcm => StandardAbiCommand::LoadAdpcm {
                count: self.w0 & 0x00ff_ffff,
                table_addr: self.w1,
            },
            StandardAbiOpcode::Mixer => StandardAbiCommand::Mixer {
                flags,
                gain: low16,
                input: high16_w1,
                output: low16_w1,
            },
            StandardAbiOpcode::Interleave => StandardAbiCommand::Interleave {
                left: high16_w1,
                right: low16_w1,
            },
            StandardAbiOpcode::PoleFilter => StandardAbiCommand::PoleFilter {
                flags,
                gain: low16,
                state_addr: self.w1,
            },
            StandardAbiOpcode::SetLoop => StandardAbiCommand::SetLoop {
                state_addr: self.w1,
            },
        };

        Ok(DecodedStandardAbiPacket {
            raw: self,
            opcode,
            command,
        })
    }
}

impl From<crate::hle::AbiCommand> for StandardAbiPacket {
    fn from(command: crate::hle::AbiCommand) -> Self {
        Self::new(command.w0, command.w1)
    }
}

/// Fields named by the standard `abi.h` packet structures and macros.
///
/// Values remain unsigned wire fields. Their DSP signedness and arithmetic
/// interpretation are outside the documented grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardAbiCommand {
    SpNoop,
    Adpcm {
        flags: u8,
        gain: u16,
        state_addr: u32,
    },
    ClearBuffer {
        dmem: u32,
        count: u32,
    },
    EnvMixer {
        flags: u8,
        state_addr: u32,
    },
    LoadBuffer {
        source_addr: u32,
    },
    Resample {
        flags: u8,
        pitch: u16,
        state_addr: u32,
    },
    SaveBuffer {
        destination_addr: u32,
    },
    Segment {
        segment: u8,
        base: u32,
    },
    SetBuffer {
        flags: u8,
        input: u16,
        output: u16,
        count: u16,
    },
    SetVolume {
        flags: u8,
        volume: u16,
        target: u16,
        rate: u16,
    },
    DmemMove {
        input: u32,
        output: u16,
        count: u16,
    },
    LoadAdpcm {
        count: u32,
        table_addr: u32,
    },
    Mixer {
        flags: u8,
        gain: u16,
        input: u16,
        output: u16,
    },
    Interleave {
        left: u16,
        right: u16,
    },
    PoleFilter {
        flags: u8,
        gain: u16,
        state_addr: u32,
    },
    SetLoop {
        state_addr: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedStandardAbiPacket {
    pub raw: StandardAbiPacket,
    pub opcode: StandardAbiOpcode,
    pub command: StandardAbiCommand,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(opcode: StandardAbiOpcode, w0_payload: u32, w1: u32) -> StandardAbiPacket {
        StandardAbiPacket::new(((opcode as u32) << 24) | (w0_payload & 0x00ff_ffff), w1)
    }

    fn decode(opcode: StandardAbiOpcode, w0_payload: u32, w1: u32) -> DecodedStandardAbiPacket {
        packet(opcode, w0_payload, w1).decode().unwrap()
    }

    #[test]
    fn opcode_table_is_exact_and_exhaustive() {
        let expected = [
            StandardAbiOpcode::SpNoop,
            StandardAbiOpcode::Adpcm,
            StandardAbiOpcode::ClearBuffer,
            StandardAbiOpcode::EnvMixer,
            StandardAbiOpcode::LoadBuffer,
            StandardAbiOpcode::Resample,
            StandardAbiOpcode::SaveBuffer,
            StandardAbiOpcode::Segment,
            StandardAbiOpcode::SetBuffer,
            StandardAbiOpcode::SetVolume,
            StandardAbiOpcode::DmemMove,
            StandardAbiOpcode::LoadAdpcm,
            StandardAbiOpcode::Mixer,
            StandardAbiOpcode::Interleave,
            StandardAbiOpcode::PoleFilter,
            StandardAbiOpcode::SetLoop,
        ];
        for (raw, expected) in (0u8..=0x0f).zip(expected) {
            assert_eq!(StandardAbiOpcode::try_from(raw), Ok(expected));
            assert_eq!(expected as u8, raw);
        }
    }

    #[test]
    fn unknown_opcode_is_a_typed_error() {
        let raw = StandardAbiPacket::new(0x10ab_cdef, 0x0123_4567);
        assert_eq!(raw.decode(), Err(UnknownStandardAbiOpcode { opcode: 0x10 }));
        assert_eq!(
            raw.decode().unwrap_err().to_string(),
            "unknown standard audio ABI opcode 0x10"
        );
    }

    #[test]
    fn packet_bytes_are_big_endian_and_round_trip() {
        let bytes = [0x05, 0x11, 0x80, 0x00, 0x01, 0x23, 0x45, 0x67];
        let packet = StandardAbiPacket::from_be_bytes(bytes);
        assert_eq!(packet, StandardAbiPacket::new(0x0511_8000, 0x0123_4567));
        assert_eq!(packet.to_be_bytes(), bytes);
    }

    #[test]
    fn decodes_spnoop_macro_shape() {
        let decoded = decode(StandardAbiOpcode::SpNoop, 0, 0);
        assert_eq!(decoded.command, StandardAbiCommand::SpNoop);
        assert_eq!(decoded.raw, StandardAbiPacket::new(0, 0));
    }

    #[test]
    fn decodes_adpcm_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::Adpcm,
            ((A_INIT | A_LOOP) as u32) << 16 | 0x2468,
            0x0012_3400,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::Adpcm {
                flags: A_INIT | A_LOOP,
                gain: 0x2468,
                state_addr: 0x0012_3400,
            }
        );
    }

    #[test]
    fn decodes_clear_buffer_macro_shape() {
        let decoded = decode(StandardAbiOpcode::ClearBuffer, 0x00a2_0240, 0x1234_0060);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::ClearBuffer {
                dmem: 0x00a2_0240,
                count: 0x1234_0060,
            }
        );
    }

    #[test]
    fn decodes_env_mixer_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::EnvMixer,
            (A_AUX as u32) << 16,
            0x0008_1200,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::EnvMixer {
                flags: A_AUX,
                state_addr: 0x0008_1200,
            }
        );
    }

    #[test]
    fn decodes_load_buffer_macro_shape() {
        let decoded = decode(StandardAbiOpcode::LoadBuffer, 0, 0x0020_4000);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::LoadBuffer {
                source_addr: 0x0020_4000,
            }
        );
    }

    #[test]
    fn decodes_resample_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::Resample,
            ((A_INIT | A_MIX) as u32) << 16 | UNITY_PITCH as u32,
            0x0004_2000,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::Resample {
                flags: A_INIT | A_MIX,
                pitch: UNITY_PITCH,
                state_addr: 0x0004_2000,
            }
        );
    }

    #[test]
    fn decodes_save_buffer_macro_shape() {
        let decoded = decode(StandardAbiOpcode::SaveBuffer, 0, 0x0030_8000);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::SaveBuffer {
                destination_addr: 0x0030_8000,
            }
        );
    }

    #[test]
    fn decodes_segment_macro_shape() {
        let decoded = decode(StandardAbiOpcode::Segment, 0, (5 << 24) | 0x0034_5678);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::Segment {
                segment: 5,
                base: 0x0034_5678,
            }
        );
    }

    #[test]
    fn decodes_set_buffer_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::SetBuffer,
            (A_AUX as u32) << 16 | 0x0110,
            0x0660_0170,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::SetBuffer {
                flags: A_AUX,
                input: 0x0110,
                output: 0x0660,
                count: 0x0170,
            }
        );
    }

    #[test]
    fn decodes_set_volume_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::SetVolume,
            ((A_LEFT | A_VOL) as u32) << 16 | 0x7fff,
            0x4000_1234,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::SetVolume {
                flags: A_LEFT | A_VOL,
                volume: 0x7fff,
                target: 0x4000,
                rate: 0x1234,
            }
        );
    }

    #[test]
    fn decodes_dmem_move_macro_shape() {
        let decoded = decode(StandardAbiOpcode::DmemMove, 0x00a3_0320, 0x0780_0080);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::DmemMove {
                input: 0x00a3_0320,
                output: 0x0780,
                count: 0x0080,
            }
        );
    }

    #[test]
    fn decodes_load_adpcm_macro_shape() {
        let decoded = decode(StandardAbiOpcode::LoadAdpcm, 0x00b0_0080, 0x0018_2000);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::LoadAdpcm {
                count: 0x00b0_0080,
                table_addr: 0x0018_2000,
            }
        );
    }

    #[test]
    fn decodes_mixer_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::Mixer,
            (A_MIX as u32) << 16 | 0x6000,
            0x0220_0880,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::Mixer {
                flags: A_MIX,
                gain: 0x6000,
                input: 0x0220,
                output: 0x0880,
            }
        );
    }

    #[test]
    fn decodes_interleave_macro_shape() {
        let decoded = decode(StandardAbiOpcode::Interleave, 0, 0x04e0_0650);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::Interleave {
                left: 0x04e0,
                right: 0x0650,
            }
        );
    }

    #[test]
    fn decodes_pole_filter_macro_shape() {
        let decoded = decode(
            StandardAbiOpcode::PoleFilter,
            (A_INIT as u32) << 16 | 0x5000,
            0x0009_1000,
        );
        assert_eq!(
            decoded.command,
            StandardAbiCommand::PoleFilter {
                flags: A_INIT,
                gain: 0x5000,
                state_addr: 0x0009_1000,
            }
        );
    }

    #[test]
    fn decodes_set_loop_macro_shape() {
        let decoded = decode(StandardAbiOpcode::SetLoop, 0, 0x0014_0800);
        assert_eq!(
            decoded.command,
            StandardAbiCommand::SetLoop {
                state_addr: 0x0014_0800,
            }
        );
    }

    #[test]
    fn raw_words_retain_padding_the_header_does_not_assign() {
        let raw = StandardAbiPacket::new(0x00ab_cdef, 0x89ab_cdef);
        let decoded = raw.decode().unwrap();
        assert_eq!(decoded.raw, raw);
        assert_eq!(decoded.command, StandardAbiCommand::SpNoop);
    }

    #[test]
    fn published_flag_aliases_and_state_sizes_are_exact() {
        assert_eq!(A_CONTINUE, A_RIGHT);
        assert_eq!(A_RIGHT, A_RATE);
        assert_eq!(A_RATE, A_NOAUX);
        assert_eq!(A_NOAUX, A_MAIN);
        assert_eq!(A_LOOP, A_OUT);
        assert_eq!(A_OUT, A_LEFT);
        assert_eq!(ADPCM_VECTOR_I16_COUNT, 8);
        assert_eq!(ADPCM_STATE_BYTES, 32);
        assert_eq!(POLE_FILTER_STATE_BYTES, 8);
        assert_eq!(RESAMPLE_STATE_BYTES, 32);
        assert_eq!(ENV_MIX_STATE_BYTES, 80);
    }
}
