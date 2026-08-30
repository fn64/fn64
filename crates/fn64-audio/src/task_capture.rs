//! Versioned private evidence for exact audio-rspboot task replay.
//!
//! The sidecar intentionally excludes RDRAM, but binds one external 8 MiB
//! image by length and SHA-256. It contains complete DMEM/IMEM banks, the
//! exact IMEM generation, PC, and pointer-free scalar/VU/device state at the
//! common boundary before rspboot's first instruction. Initial journal counts
//! are explicit and must be zero; a capture from a later phase is rejected.
//!
//! These bytes include game-derived DMEM and IMEM and must remain private.
//! This module provides evidence reconstruction only: it performs no digest
//! selection, registration, production admission, or execution-policy change.
//! A future frame version returns its typed error and records the reached
//! unsupported format when the process unsupported-event source is armed.

use fn64_runtime::rsp::RspMemorySnapshot;
use fn64_runtime::{OsTaskHeader, RdramAddr, RspMemoryBank, RSP_MEMORY_BANK_SIZE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hle_rspboot::{AudioRspbootError, AudioRspbootInput};
use crate::rsp::runtime::{RspMachineCaptureV1, RspMachineState};

const MAGIC: &[u8; 16] = b"FN64AUDRSPBOOT1\0";
const VERSION: u32 = 1;
const FRAME_HEADER_BYTES: usize = 16 + 4 + 8 + 32;
const MAX_PAYLOAD_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioTaskCaptureError {
    TruncatedFrame,
    BadMagic,
    UnsupportedVersion(u32),
    PayloadTooLarge(usize),
    FramedLengthMismatch { declared: usize, actual: usize },
    PayloadDigestMismatch,
    MalformedPayload(String),
    RdramLengthMismatch { captured: usize, supplied: usize },
    RdramDigestMismatch,
    RspBankLength { bank: &'static str, actual: usize },
    MachineState(&'static str),
    RspbootInput(AudioRspbootError),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureV1 {
    schema: String,
    task_addr: u32,
    loaded_header: [u32; 16],
    external_rdram_len: usize,
    external_rdram_sha256: Vec<u8>,
    dmem: Vec<u8>,
    imem: Vec<u8>,
    imem_generation: u64,
    initial_pc_low12: u32,
    machine: RspMachineCaptureV1,
}

pub fn encode_audio_rspboot_capture(
    input: &AudioRspbootInput,
) -> Result<Vec<u8>, AudioTaskCaptureError> {
    let payload = CaptureV1 {
        schema: "fn64.audio-rspboot-input.v1".into(),
        task_addr: input.task_addr().offset(),
        loaded_header: header_words(input.loaded_header()),
        external_rdram_len: input.rdram_storage().len(),
        external_rdram_sha256: Sha256::digest(input.rdram_storage()).to_vec(),
        dmem: input.rsp_memory().bank(RspMemoryBank::Dmem).to_vec(),
        imem: input.rsp_memory().bank(RspMemoryBank::Imem).to_vec(),
        imem_generation: input.rsp_memory().imem_generation(),
        initial_pc_low12: input.initial_pc_low12(),
        machine: input
            .initial_machine_state()
            .to_capture_v1()
            .map_err(AudioTaskCaptureError::MachineState)?,
    };
    let payload = serde_json::to_vec(&payload)
        .map_err(|error| AudioTaskCaptureError::MalformedPayload(error.to_string()))?;
    assert!(
        payload.len() <= MAX_PAYLOAD_BYTES,
        "private audio task capture payload exceeded fixed format bound"
    );
    let mut framed = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    framed.extend_from_slice(MAGIC);
    framed.extend_from_slice(&VERSION.to_le_bytes());
    framed.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    framed.extend_from_slice(&Sha256::digest(&payload));
    framed.extend_from_slice(&payload);
    Ok(framed)
}

pub fn decode_audio_rspboot_capture(
    framed: &[u8],
    external_rdram: Vec<u8>,
) -> Result<AudioRspbootInput, AudioTaskCaptureError> {
    if framed.len() < FRAME_HEADER_BYTES {
        return Err(AudioTaskCaptureError::TruncatedFrame);
    }
    if &framed[..16] != MAGIC {
        return Err(AudioTaskCaptureError::BadMagic);
    }
    let version = u32::from_le_bytes(framed[16..20].try_into().expect("four version bytes"));
    if version != VERSION {
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Audio,
            "audio.task-capture.frame-version",
            format!("audio task capture version {version} is unsupported; expected {VERSION}"),
            None,
            fn64_runtime::UnsupportedDisposition::ReturnedError,
        );
        return Err(AudioTaskCaptureError::UnsupportedVersion(version));
    }
    let declared_u64 = u64::from_le_bytes(
        framed[20..28]
            .try_into()
            .expect("eight payload-length bytes"),
    );
    let declared = usize::try_from(declared_u64)
        .map_err(|_| AudioTaskCaptureError::PayloadTooLarge(usize::MAX))?;
    if declared > MAX_PAYLOAD_BYTES {
        return Err(AudioTaskCaptureError::PayloadTooLarge(declared));
    }
    let actual = framed.len() - FRAME_HEADER_BYTES;
    if declared != actual {
        return Err(AudioTaskCaptureError::FramedLengthMismatch { declared, actual });
    }
    let payload = &framed[FRAME_HEADER_BYTES..];
    if framed[28..60] != Sha256::digest(payload)[..] {
        return Err(AudioTaskCaptureError::PayloadDigestMismatch);
    }
    let capture: CaptureV1 = serde_json::from_slice(payload)
        .map_err(|error| AudioTaskCaptureError::MalformedPayload(error.to_string()))?;
    if capture.schema != "fn64.audio-rspboot-input.v1" {
        return Err(AudioTaskCaptureError::MalformedPayload(format!(
            "unexpected schema {:?}",
            capture.schema
        )));
    }
    if capture.external_rdram_len != external_rdram.len() {
        return Err(AudioTaskCaptureError::RdramLengthMismatch {
            captured: capture.external_rdram_len,
            supplied: external_rdram.len(),
        });
    }
    if capture.external_rdram_sha256.len() != 32
        || capture.external_rdram_sha256 != Sha256::digest(&external_rdram)[..]
    {
        return Err(AudioTaskCaptureError::RdramDigestMismatch);
    }
    let dmem: [u8; RSP_MEMORY_BANK_SIZE] =
        capture
            .dmem
            .try_into()
            .map_err(|bytes: Vec<u8>| AudioTaskCaptureError::RspBankLength {
                bank: "DMEM",
                actual: bytes.len(),
            })?;
    let imem: [u8; RSP_MEMORY_BANK_SIZE] =
        capture
            .imem
            .try_into()
            .map_err(|bytes: Vec<u8>| AudioTaskCaptureError::RspBankLength {
                bank: "IMEM",
                actual: bytes.len(),
            })?;
    let machine = RspMachineState::from_capture_v1(capture.machine)
        .map_err(AudioTaskCaptureError::MachineState)?;
    AudioRspbootInput::new(
        RdramAddr::from_offset(capture.task_addr),
        header_from_words(capture.loaded_header),
        external_rdram,
        RspMemorySnapshot::from_complete_banks(dmem, imem, capture.imem_generation),
        capture.initial_pc_low12,
        machine,
    )
    .map_err(AudioTaskCaptureError::RspbootInput)
}

fn header_words(header: OsTaskHeader) -> [u32; 16] {
    [
        header.task_type,
        header.flags,
        header.ucode_boot,
        header.ucode_boot_size,
        header.ucode,
        header.ucode_size,
        header.ucode_data,
        header.ucode_data_size,
        header.dram_stack,
        header.dram_stack_size,
        header.output_buff,
        header.output_buff_size,
        header.data_ptr,
        header.data_size,
        header.yield_data_ptr,
        header.yield_data_size,
    ]
}

fn header_from_words(words: [u32; 16]) -> OsTaskHeader {
    OsTaskHeader {
        task_type: words[0],
        flags: words[1],
        ucode_boot: words[2],
        ucode_boot_size: words[3],
        ucode: words[4],
        ucode_size: words[5],
        ucode_data: words[6],
        ucode_data_size: words[7],
        dram_stack: words[8],
        dram_stack_size: words[9],
        output_buff: words[10],
        output_buff_size: words[11],
        data_ptr: words[12],
        data_size: words[13],
        yield_data_ptr: words[14],
        yield_data_size: words[15],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    use fn64_runtime::M_AUDTASK;

    fn fixture() -> AudioRspbootInput {
        let header = OsTaskHeader {
            task_type: M_AUDTASK,
            ucode_boot: 0x100,
            ucode_boot_size: 0x80,
            ucode: 0x200,
            ucode_size: 0x800,
            ucode_data: 0x1000,
            ucode_data_size: 0x40,
            data_ptr: 0x2000,
            data_size: 8,
            ..OsTaskHeader::default()
        };
        let mut dmem = std::array::from_fn(|index| (index as u8).wrapping_mul(3));
        for (slot, word) in dmem[0xfc0..].chunks_exact_mut(4).zip(header_words(header)) {
            slot.copy_from_slice(&word.to_be_bytes());
        }
        let imem = std::array::from_fn(|index| (index as u8).wrapping_mul(5));
        let mut empty = [];
        let machine = crate::rsp::runtime::RspMachine::new(&mut empty).snapshot_state();
        AudioRspbootInput::new(
            RdramAddr::from_offset(0x3000),
            header,
            vec![0; DEFAULT_RDRAM_SIZE],
            RspMemorySnapshot::from_complete_banks(dmem, imem, 17),
            0,
            machine,
        )
        .unwrap()
    }

    #[test]
    fn private_capture_round_trips_complete_preboot_input() {
        let input = fixture();
        let encoded = encode_audio_rspboot_capture(&input).unwrap();
        let decoded =
            decode_audio_rspboot_capture(&encoded, input.rdram_storage().to_vec()).unwrap();

        assert_eq!(decoded.task_addr(), input.task_addr());
        assert_eq!(decoded.loaded_header(), input.loaded_header());
        assert_eq!(decoded.rdram_storage(), input.rdram_storage());
        assert_eq!(decoded.rsp_memory(), input.rsp_memory());
        assert_eq!(decoded.initial_pc_low12(), input.initial_pc_low12());
        assert_eq!(
            decoded.initial_machine_state(),
            input.initial_machine_state()
        );
    }

    #[test]
    fn every_truncated_prefix_and_trailing_byte_is_rejected() {
        let input = fixture();
        let encoded = encode_audio_rspboot_capture(&input).unwrap();
        for end in 0..encoded.len() {
            assert!(
                decode_audio_rspboot_capture(&encoded[..end], input.rdram_storage().to_vec())
                    .is_err(),
                "accepted truncated prefix ending at byte {end}"
            );
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode_audio_rspboot_capture(&trailing, input.rdram_storage().to_vec()).is_err());
    }

    #[test]
    fn payload_and_external_rdram_mutation_are_rejected() {
        let input = fixture();
        let mut encoded = encode_audio_rspboot_capture(&input).unwrap();
        *encoded.last_mut().unwrap() ^= 1;
        assert!(matches!(
            decode_audio_rspboot_capture(&encoded, input.rdram_storage().to_vec()),
            Err(AudioTaskCaptureError::PayloadDigestMismatch)
        ));

        let encoded = encode_audio_rspboot_capture(&input).unwrap();
        let mut wrong_rdram = input.rdram_storage().to_vec();
        wrong_rdram[0] ^= 1;
        assert!(matches!(
            decode_audio_rspboot_capture(&encoded, wrong_rdram),
            Err(AudioTaskCaptureError::RdramDigestMismatch)
        ));
    }

    #[test]
    fn unsupported_frame_version_enters_the_armed_event_source() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let input = fixture();
        let mut encoded = encode_audio_rspboot_capture(&input).unwrap();
        encoded[16..20].copy_from_slice(&2_u32.to_le_bytes());

        assert!(matches!(
            decode_audio_rspboot_capture(&encoded, input.rdram_storage().to_vec()),
            Err(AudioTaskCaptureError::UnsupportedVersion(2))
        ));
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Audio);
        assert_eq!(events[0].operation, "audio.task-capture.frame-version");
        assert_eq!(events[0].guest_cycle, None);
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        fn64_runtime::complete_unsupported_observation(
            fn64_runtime::Cycles::ZERO,
            &"0".repeat(64),
        );
    }
}
