//! Consuming standard-family HLE execution frontier.
//!
//! Exact catalog admission authorizes this module to decode the standard ABI
//! wire format. It does not authorize undocumented DSP, memory, terminal, or
//! cycle semantics. The executor therefore consumes the sole same-entry lane,
//! applies only state transitions already proved by the public-contract
//! memory-command layer, and stops at the first typed evidence frontier.
//! Nothing in this module can mutate live RDRAM or publish DPC work.

use core::fmt;

use crate::hle_memory::{
    execute_standard_memory_command, StandardAbiMemoryError, StandardAbiMemoryState,
};
use crate::hle_outcome::AudioHleSelection;
use crate::hle_transaction::OwnedDmem;
use crate::standard_abi::{DecodedStandardAbiPacket, StandardAbiOpcode, UnknownStandardAbiOpcode};
use crate::whole_task::{
    NoDpcSubmissionWholeAudioTaskReference, PreparedWholeAudioTaskDifferential,
    WholeAudioTaskHleLane,
};
use fn64_runtime::RspMemoryBank;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StandardAudioHleFrontier {
    UnknownOpcode {
        command_index: usize,
        source: UnknownStandardAbiOpcode,
    },
    UnsupportedMemorySemantics {
        command_index: usize,
        opcode: StandardAbiOpcode,
        source: StandardAbiMemoryError,
    },
    UnsupportedDspSemantics {
        command_index: usize,
        opcode: StandardAbiOpcode,
    },
    UnsupportedCompletionSemantics {
        command_count: usize,
    },
}

impl fmt::Display for StandardAudioHleFrontier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownOpcode {
                command_index,
                source,
            } => write!(f, "standard audio command {command_index}: {source}"),
            Self::UnsupportedMemorySemantics {
                command_index,
                opcode,
                source,
            } => write!(
                f,
                "standard audio command {command_index} ({opcode:?}) reached an uncharacterized memory frontier: {source}"
            ),
            Self::UnsupportedDspSemantics {
                command_index,
                opcode,
            } => write!(
                f,
                "standard audio command {command_index} ({opcode:?}) requires uncharacterized DSP semantics"
            ),
            Self::UnsupportedCompletionSemantics { command_count } => write!(
                f,
                "standard audio task with {command_count} commands requires characterized terminal state and completion work"
            ),
        }
    }
}

impl std::error::Error for StandardAudioHleFrontier {}

/// Consumed same-snapshot execution stopped before any guessed behavior.
///
/// The authoritative LLE reference and typed frontier are retained for
/// diagnostics. Lane-local DMEM, transaction, and decoder state are discarded
/// on the frontier and therefore make no claim to be a replayable partial HLE
/// trace or commit candidate.
///
/// ```compile_fail
/// use fn64_audio::hle_executor::UnsupportedStandardWholeAudioTask;
///
/// fn duplicate(attempt: UnsupportedStandardWholeAudioTask) {
///     let _replayed = attempt.clone();
/// }
/// ```
///
/// ```compile_fail
/// use fn64_audio::hle_executor::UnsupportedStandardWholeAudioTask;
/// use fn64_audio::hle_outcome::AudioTaskOutcome;
///
/// fn promote_frontier(attempt: UnsupportedStandardWholeAudioTask) -> AudioTaskOutcome {
///     attempt.into_outcome()
/// }
/// ```
#[derive(Debug)]
pub struct UnsupportedStandardWholeAudioTask {
    selection: AudioHleSelection,
    decoded_commands: usize,
    frontier: StandardAudioHleFrontier,
    reference: NoDpcSubmissionWholeAudioTaskReference,
}

impl UnsupportedStandardWholeAudioTask {
    pub const fn selection(&self) -> AudioHleSelection {
        self.selection
    }

    pub const fn decoded_commands(&self) -> usize {
        self.decoded_commands
    }

    pub const fn frontier(&self) -> &StandardAudioHleFrontier {
        &self.frontier
    }

    pub const fn reference(&self) -> &NoDpcSubmissionWholeAudioTaskReference {
        &self.reference
    }
}

/// Consume the paired lane/reference owner and advance only through semantics
/// already established by allowed public sources.
///
/// This function cannot return a candidate `AudioTaskOutcome`: exact terminal
/// state and completion work are themselves characterization results. A future
/// completed result must be privately sealed by consuming this exact lane
/// before whole-task comparison or commit authority is reintroduced.
pub fn execute_standard_whole_audio_task(
    prepared: PreparedWholeAudioTaskDifferential,
) -> UnsupportedStandardWholeAudioTask {
    let (lane, reference) = prepared.into_parts();
    execute_standard_lane(lane, reference)
}

fn execute_standard_lane(
    lane: WholeAudioTaskHleLane,
    reference: NoDpcSubmissionWholeAudioTaskReference,
) -> UnsupportedStandardWholeAudioTask {
    let lane = lane.into_inner();
    let selection = lane.selection();
    let snapshot = lane.snapshot();
    let admission = snapshot.admission();
    let command_bytes = snapshot.entry().command_bytes();
    let mut dmem = OwnedDmem::new(*snapshot.entry().rsp_memory().bank(RspMemoryBank::Dmem));
    let mut transaction = lane.hle_transaction();
    let mut memory_state = StandardAbiMemoryState::default();

    for (command_index, bytes) in command_bytes.chunks_exact(8).enumerate() {
        let command = crate::hle::AbiCommand::new(
            u32::from_be_bytes(bytes[..4].try_into().expect("four-byte command word")),
            u32::from_be_bytes(bytes[4..].try_into().expect("four-byte command word")),
        );
        let packet = match admission.decode_standard_abi(command) {
            Ok(packet) => packet,
            Err(source) => {
                return unsupported(
                    selection,
                    command_index,
                    StandardAudioHleFrontier::UnknownOpcode {
                        command_index,
                        source,
                    },
                    reference,
                );
            }
        };

        if is_memory_command(packet) {
            if let Err(source) = execute_standard_memory_command(
                &mut memory_state,
                &mut dmem,
                &mut transaction,
                packet,
            ) {
                return unsupported(
                    selection,
                    command_index + 1,
                    StandardAudioHleFrontier::UnsupportedMemorySemantics {
                        command_index,
                        opcode: packet.opcode(),
                        source,
                    },
                    reference,
                );
            }
            continue;
        }

        return unsupported(
            selection,
            command_index + 1,
            StandardAudioHleFrontier::UnsupportedDspSemantics {
                command_index,
                opcode: packet.opcode(),
            },
            reference,
        );
    }

    unsupported(
        selection,
        command_bytes.len() / 8,
        StandardAudioHleFrontier::UnsupportedCompletionSemantics {
            command_count: command_bytes.len() / 8,
        },
        reference,
    )
}

fn unsupported(
    selection: AudioHleSelection,
    decoded_commands: usize,
    frontier: StandardAudioHleFrontier,
    reference: NoDpcSubmissionWholeAudioTaskReference,
) -> UnsupportedStandardWholeAudioTask {
    UnsupportedStandardWholeAudioTask {
        selection,
        decoded_commands,
        frontier,
        reference,
    }
}

fn is_memory_command(packet: DecodedStandardAbiPacket) -> bool {
    matches!(
        packet.opcode(),
        StandardAbiOpcode::ClearBuffer
            | StandardAbiOpcode::LoadBuffer
            | StandardAbiOpcode::SaveBuffer
            | StandardAbiOpcode::Segment
            | StandardAbiOpcode::SetBuffer
            | StandardAbiOpcode::DmemMove
            | StandardAbiOpcode::LoadAdpcm
            | StandardAbiOpcode::SetLoop
    )
}
