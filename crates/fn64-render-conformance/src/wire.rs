//! Canonical replay and verifier-private authority wire protocol.
//!
//! A runner receives [`RunnerRequest`], which deliberately excludes authority
//! expectations. Replay decoding and final evaluation happen in this Rust
//! crate so the Python orchestrator cannot invent renderer-IR evidence.

use fn64_render_ir::{
    BackendEffectReport, CompletedWrite, ContentDigest, DmemRange, DramCommandChunk,
    DramCommandStream, FullSyncBoundary, RawCommandStream, RawStreamKind, ResourceAccess,
    TemporalBoundary, WorkloadPacket, WorkloadRecord, XbusCommandChunk, XbusCommandStream,
};
use serde::{Deserialize, Serialize};

use fn64_render_conformance::{ContractError, ObservableLayer, RowId, MAX_OBSERVABLE_BYTES};

pub const REPLAY_SCHEMA: &str = "fn64.render-conformance.replay.v1";
pub const AUTHORITY_SCHEMA: &str = "fn64.render-conformance.private-authority.v1";
pub const REQUEST_SCHEMA: &str = "fn64.render-conformance.runner-request.v2";
pub const RESULT_SCHEMA: &str = "fn64.render-conformance.process-result.v3";
pub const INSPECTION_SCHEMA: &str = "fn64.render-conformance.replay-inspection.v1";
pub const EVALUATION_SCHEMA: &str = "fn64.render-conformance.evaluation.v1";
pub const GUEST_PROOF_SCHEMA: &str = "fn64.render-conformance.guest-commit-proof.v2";

#[derive(Debug)]
pub enum WireError {
    Contract(ContractError),
    Json(serde_json::Error),
    InvalidHex(&'static str),
    InvalidSchema(&'static str),
    InvalidLayer,
    PayloadCount,
    PayloadLength,
    PayloadAlignment,
    EffectCount,
    EffectSlot,
    ObservationLayer,
    GuestProof,
    ExecutionStatus,
    Ir(fn64_render_ir::ValidationError),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(out, "renderer conformance wire rejected: {self:?}")
    }
}

impl std::error::Error for WireError {}

impl From<ContractError> for WireError {
    fn from(value: ContractError) -> Self {
        Self::Contract(value)
    }
}
impl From<serde_json::Error> for WireError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<fn64_render_ir::ValidationError> for WireError {
    fn from(value: fn64_render_ir::ValidationError) -> Self {
        Self::Ir(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayFixture {
    pub schema: String,
    pub row_id: String,
    pub record_hex: String,
    pub payload_streams_hex: Vec<String>,
    pub capture_layer: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservationWire {
    pub layer: String,
    pub bytes_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectWire {
    pub slot: String,
    pub bytes_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateAuthority {
    schema: String,
    row_id: String,
    replay_identity: String,
    expected_observation: ObservationWire,
    expected_backend_effects: Vec<EffectWire>,
    expected_guest_effects: Vec<EffectWire>,
    expected_guest_effect_identity: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerRequest {
    pub schema: String,
    pub ordinal: usize,
    pub challenge: String,
    pub replay: ReplayFixture,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GuestCommitProof {
    schema: String,
    challenge: String,
    workload_identity: String,
    backend_effect_identity: String,
    guest_effect_identity: String,
    proof_identity: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessResult {
    pub schema: String,
    pub challenge: String,
    pub pid: u32,
    pub execution_status: String,
    pub observation: ObservationWire,
    pub backend_effects: Vec<EffectWire>,
    pub guest_commit_proof: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplayInspection {
    pub schema: String,
    pub row_id: String,
    pub replay_identity: String,
    pub record_identity: String,
    pub workload_identity: String,
    pub capture_layer: String,
    pub effect_slots: Vec<EffectSlot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSlot {
    pub id: String,
    pub guest_visible: bool,
    pub byte_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRequest {
    pub replay: ReplayFixture,
    pub authority: PrivateAuthority,
    pub result: ProcessResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    schema: String,
    classification: String,
    semantic_identity: String,
    observation_identity: String,
    backend_effect_identity: String,
    guest_effect_identity: Option<String>,
}

pub struct ValidatedReplay {
    pub row_id: RowId,
    pub packet: WorkloadPacket,
    pub record: WorkloadRecord,
    pub capture_layer: ObservableLayer,
    pub identity: ContentDigest,
    pub slots: Vec<(String, ResourceAccess)>,
}

pub fn decode_hex(value: &str, field: &'static str) -> Result<Vec<u8>, WireError> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(WireError::InvalidHex(field));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| WireError::InvalidHex(field))
        })
        .collect()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("String writes cannot fail");
    }
    result
}

fn digest_hex(value: ContentDigest) -> String {
    encode_hex(value.as_ref())
}

fn layer_from_wire(value: &str) -> Result<ObservableLayer, WireError> {
    ObservableLayer::ORDERED
        .into_iter()
        .find(|layer| layer.wire_name() == value)
        .ok_or(WireError::InvalidLayer)
}

fn reconstruct_streams(
    record: &WorkloadRecord,
    payloads: &[Vec<u8>],
) -> Result<Vec<RawCommandStream>, WireError> {
    if record.streams().len() != payloads.len() {
        return Err(WireError::PayloadCount);
    }
    record
        .streams()
        .iter()
        .zip(payloads)
        .map(|(stream, payload)| {
            if payload.len() != stream.byte_len() as usize {
                return Err(WireError::PayloadLength);
            }
            let mut prior = stream.start();
            match stream.kind() {
                RawStreamKind::Dram => {
                    let layout = record.memory_layout();
                    let mut chunks = Vec::new();
                    for boundary in stream.cmd_end_occurrences() {
                        let start = (prior - stream.start()) as usize;
                        let end = (boundary.source_address - stream.start()) as usize;
                        let bytes = &payload[start..end];
                        if bytes.len() % 4 != 0 {
                            return Err(WireError::PayloadAlignment);
                        }
                        let words = bytes
                            .chunks_exact(4)
                            .map(|word| u32::from_be_bytes(word.try_into().expect("exact chunk")))
                            .collect();
                        let full_syncs = stream
                            .full_sync_occurrences()
                            .iter()
                            .filter(|sync| sync.chunk_index == boundary.chunk_index)
                            .map(|sync| {
                                FullSyncBoundary::new(
                                    sync.sequence,
                                    sync.interrupt_sequence,
                                    sync.interrupt_before,
                                    sync.interrupt_after,
                                )
                            })
                            .collect();
                        chunks.push(DramCommandChunk::try_new(
                            layout.range(prior, boundary.source_address)?,
                            words,
                            TemporalBoundary::new(boundary.sequence, boundary.interrupt),
                            full_syncs,
                        )?);
                        prior = boundary.source_address;
                    }
                    Ok(RawCommandStream::Dram(DramCommandStream::try_new(chunks)?))
                }
                RawStreamKind::Xbus => {
                    let mut chunks = Vec::new();
                    for boundary in stream.cmd_end_occurrences() {
                        let start = (prior - stream.start()) as usize;
                        let end = (boundary.source_address - stream.start()) as usize;
                        let full_syncs = stream
                            .full_sync_occurrences()
                            .iter()
                            .filter(|sync| sync.chunk_index == boundary.chunk_index)
                            .map(|sync| {
                                FullSyncBoundary::new(
                                    sync.sequence,
                                    sync.interrupt_sequence,
                                    sync.interrupt_before,
                                    sync.interrupt_after,
                                )
                            })
                            .collect();
                        chunks.push(XbusCommandChunk::try_new(
                            DmemRange::try_new(prior, boundary.source_address)?,
                            payload[start..end].to_vec(),
                            TemporalBoundary::new(boundary.sequence, boundary.interrupt),
                            full_syncs,
                        )?);
                        prior = boundary.source_address;
                    }
                    Ok(RawCommandStream::Xbus(XbusCommandStream::try_new(chunks)?))
                }
            }
        })
        .collect()
}

pub fn validate_replay(replay: &ReplayFixture) -> Result<ValidatedReplay, WireError> {
    if replay.schema != REPLAY_SCHEMA {
        return Err(WireError::InvalidSchema("replay"));
    }
    let row_id = RowId::new(replay.row_id.clone())?;
    let record_bytes = decode_hex(&replay.record_hex, "record_hex")?;
    let record = WorkloadRecord::decode(&record_bytes)?;
    let payloads = replay
        .payload_streams_hex
        .iter()
        .map(|value| decode_hex(value, "payload_streams_hex"))
        .collect::<Result<Vec<_>, _>>()?;
    let streams = reconstruct_streams(&record, &payloads)?;
    let packet = record.replay(streams)?;
    let capture_layer = layer_from_wire(&replay.capture_layer)?;
    let layer_tag = [capture_layer as u8];
    let mut fields: Vec<&[u8]> = vec![row_id.as_str().as_bytes(), &record_bytes, &layer_tag];
    fields.extend(payloads.iter().map(Vec::as_slice));
    let identity = ContentDigest::hash(b"fn64.render-conformance.replay.v1\0", &fields);
    let slots = packet
        .journal()
        .accesses()
        .iter()
        .copied()
        .filter(|access| access.mode().writes())
        .enumerate()
        .map(|(index, access)| (format!("effect-{index:04}"), access))
        .collect();
    Ok(ValidatedReplay {
        row_id,
        packet,
        record,
        capture_layer,
        identity,
        slots,
    })
}

pub fn inspect(replay: &ReplayFixture) -> Result<ReplayInspection, WireError> {
    let value = validate_replay(replay)?;
    Ok(ReplayInspection {
        schema: INSPECTION_SCHEMA.into(),
        row_id: value.row_id.as_str().into(),
        replay_identity: digest_hex(value.identity),
        record_identity: digest_hex(value.record.record_identity().digest()),
        workload_identity: digest_hex(value.record.workload_identity().digest()),
        capture_layer: value.capture_layer.wire_name().into(),
        effect_slots: value
            .slots
            .iter()
            .map(|(id, access)| EffectSlot {
                id: id.clone(),
                guest_visible: access.region().is_guest_visible(),
                byte_count: access.region().declared_bytes(),
            })
            .collect(),
    })
}

fn parse_effects(
    replay: &ValidatedReplay,
    values: &[EffectWire],
) -> Result<Vec<CompletedWrite>, WireError> {
    if values.len() != replay.slots.len() {
        return Err(WireError::EffectCount);
    }
    replay
        .slots
        .iter()
        .zip(values)
        .map(|((slot, access), value)| {
            if &value.slot != slot {
                return Err(WireError::EffectSlot);
            }
            let bytes = decode_hex(&value.bytes_hex, "effect bytes")?;
            if bytes.len() != access.region().declared_bytes() as usize {
                return Err(WireError::PayloadLength);
            }
            let content = ContentDigest::hash(
                b"fn64.render-conformance.effect-content.v1\0",
                &[slot.as_bytes(), &bytes],
            );
            Ok(CompletedWrite::try_new(
                *access,
                bytes.len() as u32,
                content,
            )?)
        })
        .collect()
}

fn guest_proof_identity(proof: &GuestCommitProof) -> Result<String, WireError> {
    let challenge = decode_hex(&proof.challenge, "challenge")?;
    let workload = decode_hex(&proof.workload_identity, "workload identity")?;
    let backend = decode_hex(&proof.backend_effect_identity, "backend effect identity")?;
    let guest = decode_hex(&proof.guest_effect_identity, "guest effect identity")?;
    if [challenge.len(), workload.len(), backend.len(), guest.len()] != [32, 32, 32, 32] {
        return Err(WireError::GuestProof);
    }
    Ok(digest_hex(ContentDigest::hash(
        b"fn64.render-conformance.guest-proof.v2\0",
        &[&challenge, &workload, &backend, &guest],
    )))
}

pub fn evaluate(request: &EvaluationRequest) -> Result<Evaluation, WireError> {
    let replay = validate_replay(&request.replay)?;
    let authority = &request.authority;
    if authority.schema != AUTHORITY_SCHEMA
        || authority.row_id != replay.row_id.as_str()
        || authority.replay_identity != digest_hex(replay.identity)
    {
        return Err(WireError::InvalidSchema("authority"));
    }
    if request.result.schema != RESULT_SCHEMA || request.result.execution_status != "completed" {
        return Err(WireError::ExecutionStatus);
    }
    if request.result.observation.layer != replay.capture_layer.wire_name()
        || authority.expected_observation.layer != replay.capture_layer.wire_name()
    {
        return Err(WireError::ObservationLayer);
    }
    let observed = decode_hex(&request.result.observation.bytes_hex, "observation")?;
    let expected = decode_hex(
        &authority.expected_observation.bytes_hex,
        "expected observation",
    )?;
    if observed.len() > MAX_OBSERVABLE_BYTES || expected.len() > MAX_OBSERVABLE_BYTES {
        return Err(WireError::PayloadLength);
    }
    let actual_writes = parse_effects(&replay, &request.result.backend_effects)?;
    let expected_writes = parse_effects(&replay, &authority.expected_backend_effects)?;
    let actual_backend = BackendEffectReport::try_new(&replay.packet, actual_writes)?;
    let expected_backend = BackendEffectReport::try_new(&replay.packet, expected_writes)?;
    let guest_slots: Vec<_> = replay
        .slots
        .iter()
        .filter(|(_, access)| access.region().is_guest_visible())
        .collect();
    let expected_guest_values: Vec<_> = authority
        .expected_backend_effects
        .iter()
        .filter(|value| guest_slots.iter().any(|(slot, _)| slot == &value.slot))
        .cloned()
        .collect();
    if expected_guest_values != authority.expected_guest_effects {
        return Err(WireError::GuestProof);
    }
    let guest_identity = if guest_slots.is_empty() {
        if request.result.guest_commit_proof.is_some()
            || authority.expected_guest_effect_identity.is_some()
        {
            return Err(WireError::GuestProof);
        }
        None
    } else {
        let proof: GuestCommitProof = serde_json::from_value(
            request
                .result
                .guest_commit_proof
                .clone()
                .ok_or(WireError::GuestProof)?,
        )
        .map_err(|_| WireError::GuestProof)?;
        if proof.schema != GUEST_PROOF_SCHEMA
            || proof.challenge != request.result.challenge
            || proof.workload_identity != digest_hex(replay.packet.identity().digest())
            || proof.backend_effect_identity != digest_hex(actual_backend.identity().digest())
            || Some(&proof.guest_effect_identity)
                != authority.expected_guest_effect_identity.as_ref()
            || proof.proof_identity != guest_proof_identity(&proof)?
        {
            return Err(WireError::GuestProof);
        }
        Some(proof.guest_effect_identity.clone())
    };
    let effects_match = request.result.backend_effects == authority.expected_backend_effects
        && actual_backend.identity() == expected_backend.identity()
        && (!guest_slots.is_empty() || authority.expected_guest_effects.is_empty());
    let classification = if observed == expected && effects_match {
        "pass"
    } else {
        "diverges"
    };
    let observation_identity = ContentDigest::hash(
        b"fn64.render-conformance.observable.v2\0",
        &[&[replay.capture_layer as u8], &observed],
    );
    let guest_tag = guest_identity.as_deref().unwrap_or("");
    let semantic = ContentDigest::hash(
        b"fn64.render-conformance.semantic-result.v1\0",
        &[
            replay.row_id.as_str().as_bytes(),
            replay.identity.as_ref(),
            classification.as_bytes(),
            observation_identity.as_ref(),
            actual_backend.identity().digest().as_ref(),
            guest_tag.as_bytes(),
        ],
    );
    Ok(Evaluation {
        schema: EVALUATION_SCHEMA.into(),
        classification: classification.into(),
        semantic_identity: digest_hex(semantic),
        observation_identity: digest_hex(observation_identity),
        backend_effect_identity: digest_hex(actual_backend.identity().digest()),
        guest_effect_identity: guest_identity,
    })
}
