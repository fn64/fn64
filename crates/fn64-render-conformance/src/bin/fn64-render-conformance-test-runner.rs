#![forbid(unsafe_code)]

use std::{
    env, fs,
    io::{self, Read},
    process,
};

use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, ContentDigest, DecodedTicket,
    DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
    GuestCommitEffectReport, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
    TicketAuthoritySet, WorkloadAdmission, WorkloadPacket, WorkloadRecord,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

#[path = "../wire.rs"]
#[allow(dead_code)]
// Shared binary-private protocol; this test binary owns issuance, not evaluation.
mod wire;

use wire::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureOptions {
    row_id: String,
    capture_layer: String,
    guest_visible: bool,
}

fn stdin_json<T: DeserializeOwned>() -> Result<T, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn test_packet(guest_visible: bool) -> WorkloadPacket {
    let layout = PhysicalMemoryLayout::try_new(0x2000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let stream = RawCommandStream::Dram(
        DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            command_range,
            vec![0xe9_u32 << 24, 0],
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                2,
                3,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap()])
        .unwrap(),
    );
    let mut accesses = vec![ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap()];
    if guest_visible {
        accesses.push(
            ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range: layout.range(0x400, 0x408).unwrap(),
                },
            )
            .unwrap(),
        );
    }
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(accesses.len(), 16).unwrap(),
        accesses,
    )
    .unwrap();
    WorkloadPacket::try_new(
        layout,
        WorkloadAdmission::RawDpc {
            transaction_sequence: 7,
        },
        vec![stream],
        journal,
    )
    .unwrap()
}

fn deterministic_effects(replay: &ValidatedReplay) -> Vec<EffectWire> {
    replay
        .slots
        .iter()
        .map(|(slot, access)| {
            let needed = access.region().declared_bytes() as usize;
            let mut bytes = Vec::with_capacity(needed);
            let mut counter = 0_u64;
            while bytes.len() < needed {
                bytes.extend_from_slice(
                    ContentDigest::hash(
                        b"fn64.render-conformance.test-effect.v2\0",
                        &[
                            replay.identity.as_ref(),
                            slot.as_bytes(),
                            &counter.to_be_bytes(),
                        ],
                    )
                    .as_ref(),
                );
                counter += 1;
            }
            bytes.truncate(needed);
            EffectWire {
                slot: slot.clone(),
                bytes_hex: encode_hex(&bytes),
            }
        })
        .collect()
}

fn deterministic_observation(replay: &ValidatedReplay) -> ObservationWire {
    let bytes = ContentDigest::hash(
        b"fn64.render-conformance.test-observation.v1\0",
        &[replay.identity.as_ref(), &[replay.capture_layer as u8]],
    );
    ObservationWire {
        layer: replay.capture_layer.wire_name().into(),
        bytes_hex: encode_hex(bytes.as_ref()),
    }
}

fn completed_writes(
    replay: &ValidatedReplay,
    effects: &[EffectWire],
) -> Result<Vec<CompletedWrite>, Box<dyn std::error::Error>> {
    replay
        .slots
        .iter()
        .zip(effects)
        .map(|((slot, access), effect)| {
            if effect.slot != *slot {
                return Err("effect slot mismatch".into());
            }
            let bytes = decode_hex(&effect.bytes_hex, "test effect")?;
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

fn guest_proof(
    replay: ValidatedReplay,
    effects: &[EffectWire],
    challenge: &str,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    if !replay
        .slots
        .iter()
        .any(|(_, access)| access.region().is_guest_visible())
    {
        return Ok(None);
    }
    let writes = completed_writes(&replay, effects)?;
    let guest_writes = writes
        .iter()
        .copied()
        .filter(|write| write.access().region().is_guest_visible())
        .collect();
    let (mut queue, mut backend, mut guest) = TicketAuthoritySet::try_new()?.into_roles();
    let submitted = queue.submit(DecodedTicket::new(replay.packet))?;
    let backend_report = BackendEffectReport::try_new(submitted.packet(), writes)?;
    let backend_receipt = backend.issue(&submitted, backend_report)?;
    let completed = submitted.gpu_complete(backend_receipt)?;
    let guest_report = GuestCommitEffectReport::try_new(&completed, guest_writes)?;
    let guest_receipt = guest.issue(&completed, guest_report)?;
    let committed = completed.commit_guest(guest_receipt)?;
    let workload = encode_hex(committed.packet().identity().digest().as_ref());
    let backend_effect = encode_hex(committed.backend_effect_identity().digest().as_ref());
    let guest_effect = encode_hex(committed.guest_effect_identity().digest().as_ref());
    let challenge_bytes = decode_hex(challenge, "challenge")?;
    if challenge_bytes.len() != 32 {
        return Err("challenge must contain exactly 32 bytes".into());
    }
    let proof_identity = encode_hex(
        ContentDigest::hash(
            b"fn64.render-conformance.guest-proof.v2\0",
            &[
                &challenge_bytes,
                &decode_hex(&workload, "workload")?,
                &decode_hex(&backend_effect, "backend")?,
                &decode_hex(&guest_effect, "guest")?,
            ],
        )
        .as_ref(),
    );
    Ok(Some(json!({
        "schema": GUEST_PROOF_SCHEMA, "challenge": challenge,
        "workload_identity": workload,
        "backend_effect_identity": backend_effect, "guest_effect_identity": guest_effect,
        "proof_identity": proof_identity,
    })))
}

fn fixture_bundle(options: FixtureOptions) -> Result<Value, Box<dyn std::error::Error>> {
    let packet = test_packet(options.guest_visible);
    let record = WorkloadRecord::from_packet(&packet);
    let payloads = packet
        .streams()
        .iter()
        .map(|stream| match stream {
            RawCommandStream::Dram(stream) => stream
                .chunks()
                .iter()
                .flat_map(|chunk| chunk.words().iter().flat_map(|word| word.to_be_bytes()))
                .collect::<Vec<_>>(),
            RawCommandStream::Xbus(stream) => stream
                .chunks()
                .iter()
                .flat_map(|chunk| chunk.bytes().iter().copied())
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();
    let replay = ReplayFixture {
        schema: REPLAY_SCHEMA.into(),
        row_id: options.row_id,
        record_hex: encode_hex(&record.encode()),
        payload_streams_hex: payloads.iter().map(|bytes| encode_hex(bytes)).collect(),
        capture_layer: options.capture_layer,
    };
    let validated = validate_replay(&replay)?;
    let expected_backend_effects = deterministic_effects(&validated);
    let expected_guest_effects = expected_backend_effects
        .iter()
        .filter(|effect| {
            validated
                .slots
                .iter()
                .find(|(slot, _)| slot == &effect.slot)
                .unwrap()
                .1
                .region()
                .is_guest_visible()
        })
        .cloned()
        .collect::<Vec<_>>();
    // Authority records only the typed lifecycle's guest-effect identity. A
    // per-run wire proof is issued later and is bound to the verifier challenge.
    let proof = guest_proof(
        validate_replay(&replay)?,
        &expected_backend_effects,
        &"00".repeat(32),
    )?;
    let expected_guest_effect_identity = proof
        .as_ref()
        .map(|value| value["guest_effect_identity"].as_str().unwrap().to_owned());
    let authority = json!({
        "schema": AUTHORITY_SCHEMA, "row_id": replay.row_id,
        "replay_identity": encode_hex(validated.identity.as_ref()),
        "expected_observation": deterministic_observation(&validated),
        "expected_backend_effects": expected_backend_effects,
        "expected_guest_effects": expected_guest_effects,
        "expected_guest_effect_identity": expected_guest_effect_identity,
        "expected_delegate_identity": null,
    });
    Ok(json!({"replay": replay, "authority": authority}))
}

fn run(request: RunnerRequest, behavior: &str) -> Result<Value, Box<dyn std::error::Error>> {
    if request.schema != REQUEST_SCHEMA {
        return Err("wrong runner request schema".into());
    }
    if behavior == "environment-sentinel" && env::var_os("FN64_CONFORMANCE_ENV_SENTINEL").is_some()
    {
        return Err("checker leaked its ambient environment into the runner".into());
    }
    let validated = validate_replay(&request.replay)?;
    let mut observation = deterministic_observation(&validated);
    let mut effects = deterministic_effects(&validated);
    let mut guest = guest_proof(validated, &effects, &request.challenge)?;
    match behavior {
        "honest" | "wrong-pid" | "stdout-hostile" | "environment-sentinel" => {}
        "echo" => observation.bytes_hex = request.replay.record_hex,
        "arbitrary-effects" => {
            if let Some(effect) = effects.first_mut() {
                effect.bytes_hex = "00".repeat(effect.bytes_hex.len() / 2);
            }
        }
        "fake-guest" => {
            if let Some(proof) = guest.as_mut() {
                proof["proof_identity"] = Value::String("00".repeat(32));
            }
        }
        "stale-guest-challenge" => {
            if let Some(proof) = guest.as_mut() {
                let stale = "00".repeat(32);
                let workload = decode_hex(
                    proof["workload_identity"]
                        .as_str()
                        .ok_or("missing workload")?,
                    "workload",
                )?;
                let backend = decode_hex(
                    proof["backend_effect_identity"]
                        .as_str()
                        .ok_or("missing backend")?,
                    "backend",
                )?;
                let guest_effect = decode_hex(
                    proof["guest_effect_identity"]
                        .as_str()
                        .ok_or("missing guest effect")?,
                    "guest effect",
                )?;
                proof["challenge"] = Value::String(stale.clone());
                proof["proof_identity"] = Value::String(encode_hex(
                    ContentDigest::hash(
                        b"fn64.render-conformance.guest-proof.v2\0",
                        &[
                            &decode_hex(&stale, "stale challenge")?,
                            &workload,
                            &backend,
                            &guest_effect,
                        ],
                    )
                    .as_ref(),
                ));
            }
        }
        _ => return Err("unknown runner behavior".into()),
    }
    Ok(json!({
        "schema": RESULT_SCHEMA, "challenge": request.challenge,
        "pid": if behavior == "wrong-pid" { process::id().wrapping_add(1) } else { process::id() },
        "execution_status": "completed", "observation": observation,
        "backend_effects": effects, "guest_commit_proof": guest,
        "delegate_identity": null,
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some("emit-test-fixture") => print!(
            "{}",
            serde_json::to_string(&fixture_bundle(stdin_json::<FixtureOptions>()?)?)?
        ),
        Some("run") => {
            let behavior = arguments.get(1).map(String::as_str).unwrap_or("honest");
            let request = stdin_json::<RunnerRequest>()?;
            if behavior == "clone" {
                let path = arguments.get(2).ok_or("clone mode needs a state path")?;
                if let Ok(encoded) = fs::read(path) {
                    print!("{}", String::from_utf8(encoded)?);
                } else {
                    let encoded = serde_json::to_vec(&run(request, "honest")?)?;
                    fs::write(path, &encoded)?;
                    print!("{}", String::from_utf8(encoded)?);
                }
            } else {
                if behavior == "stdout-hostile" {
                    print!("native diagnostic before JSON\n");
                }
                print!("{}", serde_json::to_string(&run(request, behavior)?)?);
            }
        }
        _ => return Err("usage: test runner emit-test-fixture|run [behavior]".into()),
    }
    Ok(())
}
