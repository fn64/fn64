//! Characterization boundary for standard-ABI memory commands.
//!
//! Public libultra `abi.h` proves the packet shape. It does not establish
//! whether pointer-looking words are physical or segmented addresses, the
//! `SEGMENT` selector width, the state selected by `SETBUFF | A_AUX`, transfer
//! count units, DMA alignment/rounding, zero-count behavior, DMEM wrapping,
//! `DMEMMOVE` overlap behavior, or `LOADADPCM` layout and lifetime. This
//! module retains those wire values and emits typed characterization requests
//! instead of turning field names into guessed microcode behavior.
//!
//! Provenance: public libultra v5.2 `abi.h`, the public SGI RSP Programmer's
//! Guide, and `AUDIO-HLE.md`'s clean-room differential boundary. No runtime
//! implementation is an input.

use core::fmt;

use crate::hle_transaction::{AudioHleTaskTransaction, OwnedDmem};
use crate::standard_abi::{DecodedStandardAbiPacket, StandardAbiCommand, StandardAbiOpcode};

/// Raw buffer fields installed by the canonical flag-zero `SETBUFF` form.
///
/// The public macro proves their placement, but not the count unit or any
/// DMEM wrap behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardBufferDescriptor {
    pub input: u16,
    pub output: u16,
    pub count: u16,
}

/// State established without interpreting a disputed flag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StandardAbiMemoryState {
    canonical_buffer: Option<StandardBufferDescriptor>,
}

impl StandardAbiMemoryState {
    pub fn canonical_buffer(&self) -> Option<StandardBufferDescriptor> {
        self.canonical_buffer
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardMemoryCommand {
    SetBuffer,
    LoadBuffer,
    SaveBuffer,
    ClearBuffer,
    DmemMove,
    Segment,
    LoadAdpcm,
    SetLoop,
}

impl StandardMemoryCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::SetBuffer => "SETBUFF",
            Self::LoadBuffer => "LOADBUFF",
            Self::SaveBuffer => "SAVEBUFF",
            Self::ClearBuffer => "CLEARBUFF",
            Self::DmemMove => "DMEMMOVE",
            Self::Segment => "SEGMENT",
            Self::LoadAdpcm => "LOADADPCM",
            Self::SetLoop => "SETLOOP",
        }
    }
}

/// Context retained beside one uninterpreted pointer-shaped wire word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawAddressContext {
    BufferTransfer { buffer: StandardBufferDescriptor },
    LoadAdpcm { count: u32 },
    SetLoop,
}

/// Design-only request for evidence that the public header cannot supply.
///
/// These values are suitable inputs to same-snapshot LLE characterization.
/// They do not authorize HLE execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardAbiCharacterizationRequest {
    AddressInterpretation {
        command: StandardMemoryCommand,
        raw_address: u32,
        context: RawAddressContext,
    },
    SegmentSelectorWidth {
        raw_w1: u32,
    },
    SetBufferDisposition {
        flags: u8,
        fields: StandardBufferDescriptor,
    },
    ClearBufferSemantics {
        dmem: u32,
        count: u32,
    },
    DmemMoveSemantics {
        input: u32,
        output: u16,
        count: u16,
    },
}

impl StandardAbiCharacterizationRequest {
    pub const fn unresolved_rules(self) -> &'static str {
        match self {
            Self::AddressInterpretation { command, .. } => match command {
                StandardMemoryCommand::LoadBuffer | StandardMemoryCommand::SaveBuffer => {
                    "address interpretation, count units, zero-count behavior, DMA alignment/rounding, and DMEM wrap"
                }
                StandardMemoryCommand::LoadAdpcm => {
                    "address interpretation, count units, DMA alignment/rounding, table layout, and table lifetime"
                }
                StandardMemoryCommand::SetLoop => {
                    "address interpretation and loop-state lifetime"
                }
                _ => "address interpretation",
            },
            Self::SegmentSelectorWidth { .. } => {
                "SEGMENT selector width, table initialization, and table persistence"
            }
            Self::SetBufferDisposition { .. } => {
                "nonzero SETBUFF flag disposition and affected buffer state"
            }
            Self::ClearBufferSemantics { .. } => {
                "CLEARBUFF count units, zero-count behavior, and DMEM wrap"
            }
            Self::DmemMoveSemantics { .. } => {
                "DMEMMOVE count units, zero-count behavior, DMEM wrap, and overlap behavior"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardAbiMemoryError {
    NotMemoryCommand {
        opcode: StandardAbiOpcode,
    },
    /// The packet is not one emitted by the documented public macro shape.
    /// This is a characterization boundary, not a claim that the ucode rejects
    /// the packet.
    OutsidePublicMacroShape {
        command: StandardMemoryCommand,
        raw_w0_payload: u32,
    },
    CanonicalBufferUnknown {
        command: StandardMemoryCommand,
    },
    EvidenceFrontier(StandardAbiCharacterizationRequest),
}

impl fmt::Display for StandardAbiMemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotMemoryCommand { opcode } => {
                write!(f, "{opcode:?} is not a standard ABI memory command")
            }
            Self::OutsidePublicMacroShape {
                command,
                raw_w0_payload,
            } => write!(
                f,
                "{} packet payload {raw_w0_payload:#08x} is outside the documented public macro shape and requires characterization",
                command.name()
            ),
            Self::CanonicalBufferUnknown { command } => write!(
                f,
                "{} requires a preceding canonical flag-zero SETBUFF",
                command.name()
            ),
            Self::EvidenceFrontier(request) => write!(
                f,
                "standard audio memory behavior requires differential evidence: {}",
                request.unresolved_rules()
            ),
        }
    }
}

impl std::error::Error for StandardAbiMemoryError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedCanonicalBuffer(StandardBufferDescriptor);

/// Execute only a public-contract state change or return characterization.
///
/// Preflight is pure. Every `Err` leaves `state`, `dmem`, and `transaction`
/// byte-for-byte unchanged, including preexisting staged RDRAM writes.
pub fn execute_standard_memory_command(
    state: &mut StandardAbiMemoryState,
    dmem: &mut OwnedDmem,
    transaction: &mut AudioHleTaskTransaction<'_>,
    packet: DecodedStandardAbiPacket,
) -> Result<(), StandardAbiMemoryError> {
    let prepared = preflight_standard_memory_command(state, dmem, transaction, packet)?;
    state.canonical_buffer = Some(prepared.0);
    Ok(())
}

fn preflight_standard_memory_command(
    state: &StandardAbiMemoryState,
    _dmem: &OwnedDmem,
    _transaction: &AudioHleTaskTransaction<'_>,
    packet: DecodedStandardAbiPacket,
) -> Result<PreparedCanonicalBuffer, StandardAbiMemoryError> {
    match packet.command() {
        StandardAbiCommand::SetBuffer {
            flags,
            input,
            output,
            count,
        } => {
            let fields = StandardBufferDescriptor {
                input,
                output,
                count,
            };
            if flags == 0 {
                Ok(PreparedCanonicalBuffer(fields))
            } else {
                Err(StandardAbiMemoryError::EvidenceFrontier(
                    StandardAbiCharacterizationRequest::SetBufferDisposition { flags, fields },
                ))
            }
        }
        StandardAbiCommand::Segment { raw_w1 } => {
            require_public_macro_shape(
                StandardMemoryCommand::Segment,
                packet.raw().w0 & 0x00ff_ffff,
            )?;
            Err(StandardAbiMemoryError::EvidenceFrontier(
                StandardAbiCharacterizationRequest::SegmentSelectorWidth { raw_w1 },
            ))
        }
        StandardAbiCommand::SetLoop { raw_state } => {
            require_public_macro_shape(
                StandardMemoryCommand::SetLoop,
                packet.raw().w0 & 0x00ff_ffff,
            )?;
            Err(address_request(
                StandardMemoryCommand::SetLoop,
                raw_state,
                RawAddressContext::SetLoop,
            ))
        }
        StandardAbiCommand::LoadBuffer { raw_source } => {
            require_public_macro_shape(
                StandardMemoryCommand::LoadBuffer,
                packet.raw().w0 & 0x00ff_ffff,
            )?;
            let buffer = require_canonical_buffer(state, StandardMemoryCommand::LoadBuffer)?;
            Err(address_request(
                StandardMemoryCommand::LoadBuffer,
                raw_source,
                RawAddressContext::BufferTransfer { buffer },
            ))
        }
        StandardAbiCommand::SaveBuffer { raw_destination } => {
            require_public_macro_shape(
                StandardMemoryCommand::SaveBuffer,
                packet.raw().w0 & 0x00ff_ffff,
            )?;
            let buffer = require_canonical_buffer(state, StandardMemoryCommand::SaveBuffer)?;
            Err(address_request(
                StandardMemoryCommand::SaveBuffer,
                raw_destination,
                RawAddressContext::BufferTransfer { buffer },
            ))
        }
        StandardAbiCommand::ClearBuffer { dmem, count } => {
            Err(StandardAbiMemoryError::EvidenceFrontier(
                StandardAbiCharacterizationRequest::ClearBufferSemantics { dmem, count },
            ))
        }
        StandardAbiCommand::DmemMove {
            input,
            output,
            count,
        } => Err(StandardAbiMemoryError::EvidenceFrontier(
            StandardAbiCharacterizationRequest::DmemMoveSemantics {
                input,
                output,
                count,
            },
        )),
        StandardAbiCommand::LoadAdpcm { count, raw_table } => Err(address_request(
            StandardMemoryCommand::LoadAdpcm,
            raw_table,
            RawAddressContext::LoadAdpcm { count },
        )),
        _ => Err(StandardAbiMemoryError::NotMemoryCommand {
            opcode: packet.opcode(),
        }),
    }
}

fn require_public_macro_shape(
    command: StandardMemoryCommand,
    raw_w0_payload: u32,
) -> Result<(), StandardAbiMemoryError> {
    if raw_w0_payload == 0 {
        Ok(())
    } else {
        Err(StandardAbiMemoryError::OutsidePublicMacroShape {
            command,
            raw_w0_payload,
        })
    }
}

fn require_canonical_buffer(
    state: &StandardAbiMemoryState,
    command: StandardMemoryCommand,
) -> Result<StandardBufferDescriptor, StandardAbiMemoryError> {
    state
        .canonical_buffer
        .ok_or(StandardAbiMemoryError::CanonicalBufferUnknown { command })
}

fn address_request(
    command: StandardMemoryCommand,
    raw_address: u32,
    context: RawAddressContext,
) -> StandardAbiMemoryError {
    StandardAbiMemoryError::EvidenceFrontier(
        StandardAbiCharacterizationRequest::AddressInterpretation {
            command,
            raw_address,
            context,
        },
    )
}

#[cfg(test)]
mod tests {
    use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    use fn64_runtime::{RdramAddr, RdramView};

    use super::*;
    use crate::standard_abi::{StandardAbiPacket, A_AUX};

    fn packet(opcode: StandardAbiOpcode, w0_payload: u32, w1: u32) -> DecodedStandardAbiPacket {
        StandardAbiPacket::new(((opcode as u32) << 24) | (w0_payload & 0x00ff_ffff), w1)
            .decode()
            .unwrap()
    }

    fn fixture() -> (StandardAbiMemoryState, OwnedDmem, Vec<u8>) {
        (
            StandardAbiMemoryState::default(),
            OwnedDmem::default(),
            vec![0; DEFAULT_RDRAM_SIZE],
        )
    }

    fn execute(
        state: &mut StandardAbiMemoryState,
        dmem: &mut OwnedDmem,
        storage: &[u8],
        packet: DecodedStandardAbiPacket,
    ) -> Result<(), StandardAbiMemoryError> {
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(storage)).unwrap();
        execute_standard_memory_command(state, dmem, &mut transaction, packet)
    }

    fn canonical_buffer() -> StandardBufferDescriptor {
        StandardBufferDescriptor {
            input: 0x100,
            output: 0x200,
            count: 0x30,
        }
    }

    fn install_canonical_buffer(
        state: &mut StandardAbiMemoryState,
        dmem: &mut OwnedDmem,
        storage: &[u8],
    ) {
        execute(
            state,
            dmem,
            storage,
            packet(StandardAbiOpcode::SetBuffer, 0x0000_0100, 0x0200_0030),
        )
        .unwrap();
    }

    #[test]
    fn flag_zero_setbuffer_retains_only_raw_canonical_fields() {
        let (mut state, mut dmem, storage) = fixture();
        install_canonical_buffer(&mut state, &mut dmem, &storage);
        assert_eq!(state.canonical_buffer(), Some(canonical_buffer()));
    }

    #[test]
    fn aux_setbuffer_is_a_disposition_request_without_state_mutation() {
        let (mut state, mut dmem, storage) = fixture();
        install_canonical_buffer(&mut state, &mut dmem, &storage);
        let before = state.clone();
        let fields = StandardBufferDescriptor {
            input: 0x1111,
            output: 0x2222,
            count: 0x3333,
        };

        assert_eq!(
            execute(
                &mut state,
                &mut dmem,
                &storage,
                packet(
                    StandardAbiOpcode::SetBuffer,
                    (u32::from(A_AUX) << 16) | 0x1111,
                    0x2222_3333,
                ),
            ),
            Err(StandardAbiMemoryError::EvidenceFrontier(
                StandardAbiCharacterizationRequest::SetBufferDisposition {
                    flags: A_AUX,
                    fields,
                }
            ))
        );
        assert_eq!(state, before);
    }

    #[test]
    fn segment_retains_raw_word_and_requests_selector_width() {
        let (mut state, mut dmem, storage) = fixture();
        assert_eq!(
            execute(
                &mut state,
                &mut dmem,
                &storage,
                packet(StandardAbiOpcode::Segment, 0, 0xa5_123456),
            ),
            Err(StandardAbiMemoryError::EvidenceFrontier(
                StandardAbiCharacterizationRequest::SegmentSelectorWidth {
                    raw_w1: 0xa5_123456,
                }
            ))
        );
        assert_eq!(state, StandardAbiMemoryState::default());
    }

    #[test]
    fn pointer_shaped_fields_remain_raw_address_requests() {
        let (mut state, mut dmem, storage) = fixture();
        install_canonical_buffer(&mut state, &mut dmem, &storage);
        let cases = [
            (
                packet(StandardAbiOpcode::LoadBuffer, 0, 0x0212_3456),
                StandardAbiCharacterizationRequest::AddressInterpretation {
                    command: StandardMemoryCommand::LoadBuffer,
                    raw_address: 0x0212_3456,
                    context: RawAddressContext::BufferTransfer {
                        buffer: canonical_buffer(),
                    },
                },
            ),
            (
                packet(StandardAbiOpcode::SaveBuffer, 0, 0x89ab_cdef),
                StandardAbiCharacterizationRequest::AddressInterpretation {
                    command: StandardMemoryCommand::SaveBuffer,
                    raw_address: 0x89ab_cdef,
                    context: RawAddressContext::BufferTransfer {
                        buffer: canonical_buffer(),
                    },
                },
            ),
            (
                packet(StandardAbiOpcode::LoadAdpcm, 0x40, 0xfedc_ba98),
                StandardAbiCharacterizationRequest::AddressInterpretation {
                    command: StandardMemoryCommand::LoadAdpcm,
                    raw_address: 0xfedc_ba98,
                    context: RawAddressContext::LoadAdpcm { count: 0x40 },
                },
            ),
            (
                packet(StandardAbiOpcode::SetLoop, 0, 0x7654_3210),
                StandardAbiCharacterizationRequest::AddressInterpretation {
                    command: StandardMemoryCommand::SetLoop,
                    raw_address: 0x7654_3210,
                    context: RawAddressContext::SetLoop,
                },
            ),
        ];

        for (packet, request) in cases {
            let before = state.clone();
            assert_eq!(
                execute(&mut state, &mut dmem, &storage, packet),
                Err(StandardAbiMemoryError::EvidenceFrontier(request))
            );
            assert_eq!(state, before);
        }
    }

    #[test]
    fn transfer_requires_a_canonical_flag_zero_buffer() {
        let (mut state, mut dmem, storage) = fixture();
        assert_eq!(
            execute(
                &mut state,
                &mut dmem,
                &storage,
                packet(StandardAbiOpcode::LoadBuffer, 0, 0x1234_5678),
            ),
            Err(StandardAbiMemoryError::CanonicalBufferUnknown {
                command: StandardMemoryCommand::LoadBuffer,
            })
        );
    }

    #[test]
    fn outside_macro_shape_requires_characterization_without_claiming_rejection() {
        let (mut state, mut dmem, storage) = fixture();
        let before = state.clone();
        assert_eq!(
            execute(
                &mut state,
                &mut dmem,
                &storage,
                packet(StandardAbiOpcode::Segment, 1, 0x0512_3456),
            ),
            Err(StandardAbiMemoryError::OutsidePublicMacroShape {
                command: StandardMemoryCommand::Segment,
                raw_w0_payload: 1,
            })
        );
        assert_eq!(state, before);
    }

    #[test]
    fn clear_and_move_retain_raw_operands_as_semantic_requests() {
        let (mut state, mut dmem, storage) = fixture();
        let cases = [
            (
                packet(StandardAbiOpcode::ClearBuffer, 0x123, 0x4567),
                StandardAbiCharacterizationRequest::ClearBufferSemantics {
                    dmem: 0x123,
                    count: 0x4567,
                },
            ),
            (
                packet(StandardAbiOpcode::DmemMove, 0x100, 0x0200_0030),
                StandardAbiCharacterizationRequest::DmemMoveSemantics {
                    input: 0x100,
                    output: 0x200,
                    count: 0x30,
                },
            ),
        ];
        for (packet, request) in cases {
            assert_eq!(
                execute(&mut state, &mut dmem, &storage, packet),
                Err(StandardAbiMemoryError::EvidenceFrontier(request))
            );
        }
    }

    #[test]
    fn zero_counts_are_characterized_instead_of_becoming_silent_noops() {
        let (mut state, mut dmem, storage) = fixture();
        for packet in [
            packet(StandardAbiOpcode::ClearBuffer, 0x100, 0),
            packet(StandardAbiOpcode::DmemMove, 0x100, 0x0200_0000),
            packet(StandardAbiOpcode::LoadAdpcm, 0, 0xffff_ffff),
        ] {
            assert!(matches!(
                execute(&mut state, &mut dmem, &storage, packet),
                Err(StandardAbiMemoryError::EvidenceFrontier(_))
            ));
        }
    }

    #[test]
    fn failed_preflight_preserves_state_dmem_and_existing_rdram_overlay() {
        let (mut state, mut dmem, storage) = fixture();
        install_canonical_buffer(&mut state, &mut dmem, &storage);
        dmem.write_u8(crate::hle_transaction::DmemAddr::new(0x20).unwrap(), 0x7f);
        let mut transaction =
            AudioHleTaskTransaction::new(RdramView::from_storage(&storage)).unwrap();
        transaction
            .write_bytes(RdramAddr::from_offset(0x40), &[1, 2, 3])
            .unwrap();
        let before_state = state.clone();
        let before_dmem = dmem.clone();
        let before_patches = transaction.canonical_patches().unwrap();

        assert!(matches!(
            execute_standard_memory_command(
                &mut state,
                &mut dmem,
                &mut transaction,
                packet(StandardAbiOpcode::LoadBuffer, 0, 0xffff_ffff),
            ),
            Err(StandardAbiMemoryError::EvidenceFrontier(_))
        ));
        assert_eq!(state, before_state);
        assert_eq!(dmem, before_dmem);
        assert_eq!(transaction.canonical_patches().unwrap(), before_patches);
    }

    #[test]
    fn non_memory_commands_trap_by_typed_opcode() {
        let (mut state, mut dmem, storage) = fixture();
        assert_eq!(
            execute(
                &mut state,
                &mut dmem,
                &storage,
                packet(StandardAbiOpcode::Mixer, 0, 0),
            ),
            Err(StandardAbiMemoryError::NotMemoryCommand {
                opcode: StandardAbiOpcode::Mixer,
            })
        );
    }
}
