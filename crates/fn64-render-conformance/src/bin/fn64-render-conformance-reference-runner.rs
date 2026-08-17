//! Pure-Rust reference-backend conformance runner for
//! `feature::native-renderer-rdram-sync`.
//!
//! This is the second engine adapter in the harness and the first non-RT64
//! one. It speaks the identical `RunnerRequest`/`ProcessResult` wire the RT64
//! deferred-history runner speaks, and is deliberately much smaller: a
//! non-RT64 runner reports `delegate_identity: None`
//! (`tools/check_rt64_port_parity.py` requires exactly that), so the entire
//! pinned-source / overlay / Metal identity block does not exist here. There
//! is no GPU, no FFI, and no native stdout to redirect -- `ReferenceBackend`
//! is synchronous CPU work under `#![forbid(unsafe_code)]`.
//!
//! The row's public claim is that a native-resolution render leaves the exact
//! game-visible bytes in RDRAM after FullSync, region-bounded. The observable
//! is therefore `resource_journal_guest_memory_effects` and the observation is
//! literally the committed guest framebuffer bytes -- no engine-internal
//! debugger projection is involved, which is precisely why this row can be
//! answered by more than one engine.
//!
//! ## How the expected answer is derived (this is the whole point)
//!
//! `expected_rdram` never calls the renderer. It recomputes the committed
//! framebuffer from the display list by hand, using only the public RDP
//! semantics:
//!
//! * `G_SETCIMG` latches an RGBA16 target of `WIDTH` pixels at `FRAMEBUFFER`.
//! * `G_SETFILLCOLOR` loads a 32-bit register whose two halfwords alternate
//!   across even/odd pixels. Both halves are programmed to the same value
//!   here, so the alternation is a no-op and each fill is one flat color.
//! * Fill-cycle `G_FILLRECT` covers `ceil(ulx) ..= floor(lrx)` inclusive on
//!   both edges (SGI RDP Command Summary; `raster/draw.rs`), so the first
//!   rectangle covers every pixel and the second covers exactly the right
//!   half.
//! * The RGBA5551 write-back is exact for these two constants: `0xf801` and
//!   `0x003f` both survive the 5->8->5 expand/truncate round trip unchanged,
//!   and both have their LSB set, which makes stored coverage full so the
//!   committed visible LSB is 1 again.
//!
//! So the expected bytes are `RED` for `x < WIDTH/2` and `BLUE` otherwise,
//! for every row, with the two `GUARD` halfwords either side untouched. That
//! is arithmetic over the command stream, not a capture of backend output.

use std::{
    io::{self, Read, Write},
    process,
};

use fn64_render::{RenderBackend, RenderConfig};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, ContentDigest, DecodedTicket,
    DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
    GuestCommitEffectReport, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
    TicketAuthoritySet, WorkloadAdmission, WorkloadPacket, WorkloadRecord,
};
use fn64_render_reference::ReferenceBackend;
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

#[path = "../wire.rs"]
#[allow(dead_code)]
mod wire;

use wire::*;

const ROW_ID: &str = "feature::native-renderer-rdram-sync";
const CAPTURE_LAYER: &str = "resource_journal_guest_memory_effects";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMAND_START: u32 = 0x100;
const FRAMEBUFFER: u32 = 0x10_0000;
/// Native resolution. Eight by four is thirty-two guest-visible pixels, the
/// exact pixel count the row's public closure claims transitions from seeded
/// bytes to rendered bytes.
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;
const PIXEL_COUNT: u32 = WIDTH * HEIGHT;
const FRAMEBUFFER_BYTES: u32 = PIXEL_COUNT * 2;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
/// Seeded guest content. Every expected pixel differs from it, so a backend
/// that writes nothing cannot satisfy this row.
const STALE: u16 = 0xffff;
/// Region bound. Both guards differ from every rendered value and from
/// `STALE`, so an over-wide or misaligned write is visible.
const GUARD: u16 = 0x4211;

const fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

/// The exact reviewed display list. Fill cycle, full-target scissor, one
/// RGBA16 native target, two flat fills, one FullSync.
const COMMANDS: [(u32, u32); 8] = [
    // G_RDPSETOTHERMODE: cycle_type = Fill, no depth/image-read hazards.
    (0xef30_00f0, 0),
    // G_SETSCISSOR over the whole native target (exclusive lower-right).
    (0xed00_0000, ((WIDTH * 4) << 12) | (HEIGHT * 4)),
    // G_SETCIMG RGBA16, width = WIDTH, at FRAMEBUFFER.
    (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    (0xf700_0000, (RED as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
    (0xf700_0000, (BLUE as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, WIDTH / 2, 0),
    // G_RDPFULLSYNC.
    (0xe900_0000, 0),
];

const COMMAND_END: u32 = COMMAND_START + (COMMANDS.len() as u32) * 8;

fn stdin_json<T: DeserializeOwned>() -> Result<T, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn stdout_json<T: Serialize>(
    mut output: impl Write,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer(&mut output, value)?;
    output.flush()?;
    Ok(())
}

fn command_words(commands: &[(u32, u32)]) -> Vec<u32> {
    commands
        .iter()
        .flat_map(|&(word0, word1)| [word0, word1])
        .collect()
}

fn packet_for_commands(
    commands: &[(u32, u32)],
) -> Result<WorkloadPacket, Box<dyn std::error::Error>> {
    let layout = PhysicalMemoryLayout::try_new(RDRAM_LEN as u32)?;
    let end = COMMAND_START
        .checked_add(u32::try_from(commands.len() * 8)?)
        .ok_or("command end overflow")?;
    let command_range = layout.range(COMMAND_START, end)?;
    let stream = RawCommandStream::Dram(DramCommandStream::try_new(vec![
        DramCommandChunk::try_new(
            command_range,
            command_words(commands),
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                2,
                3,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )?,
    ])?);
    let accesses = vec![
        ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: command_range,
            },
        )?,
        ResourceAccess::try_new(
            OperationId::new(1),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(FRAMEBUFFER, FRAMEBUFFER + FRAMEBUFFER_BYTES)?,
            },
        )?,
    ];
    let declared_bytes = command_range.len() + FRAMEBUFFER_BYTES;
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(accesses.len(), declared_bytes)?,
        accesses,
    )?;
    Ok(WorkloadPacket::try_new(
        layout,
        WorkloadAdmission::RawDpc {
            transaction_sequence: 1,
        },
        vec![stream],
        journal,
    )?)
}

fn replay_for_commands(
    commands: &[(u32, u32)],
) -> Result<ReplayFixture, Box<dyn std::error::Error>> {
    let packet = packet_for_commands(commands)?;
    let record = WorkloadRecord::from_packet(&packet);
    let payload = command_words(commands)
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
    Ok(ReplayFixture {
        schema: REPLAY_SCHEMA.into(),
        row_id: ROW_ID.into(),
        record_hex: encode_hex(&record.encode()),
        payload_streams_hex: vec![encode_hex(&payload)],
        capture_layer: CAPTURE_LAYER.into(),
    })
}

fn exact_replay() -> Result<ReplayFixture, Box<dyn std::error::Error>> {
    replay_for_commands(&COMMANDS)
}

fn validate_exact_replay(
    replay: &ReplayFixture,
) -> Result<ValidatedReplay, Box<dyn std::error::Error>> {
    let validated = validate_replay(replay)?;
    if validated.row_id.as_str() != ROW_ID
        || validated.capture_layer.wire_name() != CAPTURE_LAYER
        || validated.packet != packet_for_commands(&COMMANDS)?
    {
        return Err(
            "reference RDRAM-sync runner accepts only the exact reviewed native-target fixture"
                .into(),
        );
    }
    Ok(validated)
}

/// Guest RDRAM before the render: seeded framebuffer, two guards, the display
/// list itself.
fn initial_rdram(commands: &[(u32, u32)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(RdramAddr::from_offset(FRAMEBUFFER + index * 2), STALE);
        }
        view.write_u16(RdramAddr::from_offset(FRAMEBUFFER - 2), GUARD);
        view.write_u16(
            RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES),
            GUARD,
        );
    }
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        let offset = COMMAND_START as usize + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    Ok(rdram)
}

/// The independently derived answer key. Left half of every row is the first
/// fill's color, the right half is the second fill's color, and both guards
/// are unchanged. Derived from the RDP command semantics documented at the
/// top of this file; the renderer is not consulted.
fn expected_pixel(index: u32) -> u16 {
    if index % WIDTH < WIDTH / 2 {
        RED
    } else {
        BLUE
    }
}

fn expected_rdram() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut rdram = initial_rdram(&COMMANDS)?;
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(
                RdramAddr::from_offset(FRAMEBUFFER + index * 2),
                expected_pixel(index),
            );
        }
    }
    Ok(rdram)
}

/// The observation for this row is the committed guest framebuffer itself:
/// the exact bytes the game would read. No engine-internal projection.
fn observation_bytes(rdram: &[u8]) -> Vec<u8> {
    rdram[FRAMEBUFFER as usize..(FRAMEBUFFER + FRAMEBUFFER_BYTES) as usize].to_vec()
}

/// Region bound. The runner refuses to report a result whose guards moved,
/// independently of whether the pixels happen to match.
fn require_bounded_write(rdram: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let view = RdramView::from_storage(rdram);
    for address in [FRAMEBUFFER - 2, FRAMEBUFFER + FRAMEBUFFER_BYTES] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(format!(
                "reference RDRAM-sync write escaped its region at {address:#010x}: {actual:#06x}"
            )
            .into());
        }
    }
    Ok(())
}

fn actual_effects(
    replay: &ValidatedReplay,
    rdram: &[u8],
) -> Result<Vec<EffectWire>, Box<dyn std::error::Error>> {
    if replay.slots.len() != 1 || replay.slots[0].0 != "effect-0000" {
        return Err("reference RDRAM-sync fixture lost its single ordered write slot".into());
    }
    Ok(vec![EffectWire {
        slot: replay.slots[0].0.clone(),
        bytes_hex: encode_hex(&observation_bytes(rdram)),
    }])
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
            let bytes = decode_hex(&effect.bytes_hex, "reference framebuffer effect")?;
            let content = ContentDigest::hash(
                b"fn64.render-conformance.effect-content.v1\0",
                &[slot.as_bytes(), &bytes],
            );
            Ok(CompletedWrite::try_new(
                *access,
                u32::try_from(bytes.len())?,
                content,
            )?)
        })
        .collect()
}

fn guest_proof(
    replay: ValidatedReplay,
    effects: &[EffectWire],
    challenge: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let challenge_bytes = decode_hex(challenge, "challenge")?;
    if challenge_bytes.len() != 32 {
        return Err("challenge must contain exactly 32 bytes".into());
    }
    let writes = completed_writes(&replay, effects)?;
    let guest_writes = writes.clone();
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
    Ok(json!({
        "schema": GUEST_PROOF_SCHEMA,
        "challenge": challenge,
        "workload_identity": workload,
        "backend_effect_identity": backend_effect,
        "guest_effect_identity": guest_effect,
        "proof_identity": proof_identity,
    }))
}

/// Emit the public replay plus the verifier-private authority. The authority's
/// expected observation and effects come from `expected_rdram`, which is
/// derived arithmetically above and never from a backend run.
fn fixture_bundle() -> Result<Value, Box<dyn std::error::Error>> {
    let replay = exact_replay()?;
    let validated = validate_exact_replay(&replay)?;
    let expected = expected_rdram()?;
    let effects = actual_effects(&validated, &expected)?;
    let proof = guest_proof(validate_exact_replay(&replay)?, &effects, &"00".repeat(32))?;
    let guest_effect_identity = proof["guest_effect_identity"]
        .as_str()
        .ok_or("typed guest proof omitted its effect identity")?;
    let authority = json!({
        "schema": AUTHORITY_SCHEMA,
        "row_id": ROW_ID,
        "replay_identity": encode_hex(validated.identity.as_ref()),
        "expected_observation": {
            "layer": CAPTURE_LAYER,
            "bytes_hex": encode_hex(&observation_bytes(&expected)),
        },
        "expected_backend_effects": effects.clone(),
        "expected_guest_effects": effects,
        "expected_guest_effect_identity": guest_effect_identity,
        // A non-RT64 runner reports no delegate identity; the checker requires
        // exactly this (`check_rt64_port_parity.py`, non-RT64 branch).
        "expected_delegate_identity": Value::Null,
    });
    Ok(json!({"replay": replay, "authority": authority}))
}

/// Run the pure-Rust reference engine over the fixture's own guest memory.
fn render(rdram: &mut [u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut backend = ReferenceBackend::default();
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    let status =
        backend.process_rdp_commands(rdram, COMMAND_START, COMMAND_END, FRAMEBUFFER, true)?;
    if status != fn64_render::FrameStatus::Complete {
        return Err(format!("reference backend returned nonterminal status {status:?}").into());
    }
    Ok(())
}

fn run(request: RunnerRequest) -> Result<Value, Box<dyn std::error::Error>> {
    if request.schema != REQUEST_SCHEMA
        || request.ordinal >= fn64_render_conformance::REQUIRED_CLEAN_RUNS
    {
        return Err("invalid reference RDRAM-sync request schema or ordinal".into());
    }
    let replay = validate_exact_replay(&request.replay)?;
    let mut rdram = initial_rdram(&COMMANDS)?;
    render(&mut rdram)?;
    // Deliberately NOT an equality check against the expected answer: this
    // runner reports what the engine produced. Only the region bound is
    // enforced, because an out-of-region write makes the reported effect
    // meaningless rather than merely wrong.
    require_bounded_write(&rdram)?;
    let effects = actual_effects(&replay, &rdram)?;
    let proof = guest_proof(replay, &effects, &request.challenge)?;
    Ok(json!({
        "schema": RESULT_SCHEMA,
        "challenge": request.challenge,
        "pid": process::id(),
        "execution_status": "completed",
        "observation": {
            "layer": CAPTURE_LAYER,
            "bytes_hex": encode_hex(&observation_bytes(&rdram)),
        },
        "backend_effects": effects,
        "guest_commit_proof": proof,
        "delegate_identity": Value::Null,
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("emit-replay") => stdout_json(io::stdout().lock(), &exact_replay()?)?,
        Some("emit-fixture") => stdout_json(io::stdout().lock(), &fixture_bundle()?)?,
        Some("run") => {
            let request = stdin_json::<RunnerRequest>()?;
            stdout_json(io::stdout().lock(), &run(request)?)?;
        }
        _ => {
            return Err(
                "usage: fn64-render-conformance-reference-runner emit-replay|emit-fixture|run"
                    .into(),
            )
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(commands: &[(u32, u32)]) {
        let accepted =
            replay_for_commands(commands).and_then(|replay| validate_exact_replay(&replay));
        assert!(accepted.is_err());
    }

    #[test]
    fn fill_value_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[3].1 ^= 1;
        rejects(&commands);
    }

    #[test]
    fn framebuffer_target_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[2].1 = FRAMEBUFFER + 8;
        rejects(&commands);
    }

    #[test]
    fn full_sync_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[7] = (0, 0);
        rejects(&commands);
    }

    /// The answer key must be independent of the engine: it is stated here
    /// as literal per-pixel arithmetic over the display list, with no call
    /// into `ReferenceBackend`.
    #[test]
    fn expected_answer_is_derived_not_captured() {
        let expected = expected_rdram().unwrap();
        let view = RdramView::from_storage(&expected);
        for index in 0..PIXEL_COUNT {
            let actual = view.read_u16(RdramAddr::from_offset(FRAMEBUFFER + index * 2));
            let derived = if (index % WIDTH) < 4 { 0xf801 } else { 0x003f };
            assert_eq!(actual, derived, "pixel {index}");
            assert_ne!(actual, STALE, "pixel {index} must differ from seeded bytes");
        }
        assert_eq!(
            view.read_u16(RdramAddr::from_offset(FRAMEBUFFER - 2)),
            GUARD
        );
        assert_eq!(
            view.read_u16(RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES)),
            GUARD
        );
    }

    #[test]
    fn region_guards_are_enforced() {
        let expected = expected_rdram().unwrap();
        require_bounded_write(&expected).unwrap();
        let mut escaped = expected;
        RdramViewMut::from_storage(&mut escaped)
            .write_u16(RdramAddr::from_offset(FRAMEBUFFER + FRAMEBUFFER_BYTES), 0);
        assert!(require_bounded_write(&escaped).is_err());
    }

    #[test]
    fn challenge_must_be_fresh_shape() {
        let replay = validate_exact_replay(&exact_replay().unwrap()).unwrap();
        let rdram = initial_rdram(&COMMANDS).unwrap();
        let effects = actual_effects(&replay, &rdram).unwrap();
        assert!(guest_proof(replay, &effects, "00").is_err());
    }

    /// The end-to-end comparison, run in-process: the engine renders, the
    /// independently derived authority judges it, and the classification is
    /// whatever it is.
    #[test]
    fn reference_engine_is_compared_against_the_derived_authority() {
        let fixture = fixture_bundle().unwrap();
        let replay: ReplayFixture = serde_json::from_value(fixture["replay"].clone()).unwrap();
        let validated = validate_exact_replay(&replay).unwrap();
        let mut rdram = initial_rdram(&COMMANDS).unwrap();
        render(&mut rdram).unwrap();
        require_bounded_write(&rdram).unwrap();
        let effects = actual_effects(&validated, &rdram).unwrap();
        let challenge = "ab".repeat(32);
        let proof = guest_proof(
            validate_exact_replay(&replay).unwrap(),
            &effects,
            &challenge,
        )
        .unwrap();
        let request_value = json!({
            "replay": replay,
            "authority": fixture["authority"],
            "result": {
                "schema": RESULT_SCHEMA,
                "challenge": challenge,
                "pid": 1,
                "execution_status": "completed",
                "observation": {
                    "layer": CAPTURE_LAYER,
                    "bytes_hex": encode_hex(&observation_bytes(&rdram)),
                },
                "backend_effects": effects,
                "guest_commit_proof": proof,
                "delegate_identity": Value::Null,
            },
        });
        let request: EvaluationRequest = serde_json::from_value(request_value).unwrap();
        let evaluation = evaluate(&request).unwrap();
        assert_eq!(
            serde_json::to_value(evaluation).unwrap()["classification"],
            "pass",
            "the reference engine's committed RDRAM diverged from the derived answer key"
        );
    }

    /// A runner that invented a delegate identity must be rejected: the
    /// authority pins `None`.
    #[test]
    fn invented_delegate_identity_is_rejected() {
        let fixture = fixture_bundle().unwrap();
        let replay: ReplayFixture = serde_json::from_value(fixture["replay"].clone()).unwrap();
        let validated = validate_exact_replay(&replay).unwrap();
        let expected = expected_rdram().unwrap();
        let effects = actual_effects(&validated, &expected).unwrap();
        let challenge = "cd".repeat(32);
        let proof = guest_proof(
            validate_exact_replay(&replay).unwrap(),
            &effects,
            &challenge,
        )
        .unwrap();
        let request_value = json!({
            "replay": replay,
            "authority": fixture["authority"],
            "result": {
                "schema": RESULT_SCHEMA,
                "challenge": challenge,
                "pid": 1,
                "execution_status": "completed",
                "observation": {
                    "layer": CAPTURE_LAYER,
                    "bytes_hex": encode_hex(&observation_bytes(&expected)),
                },
                "backend_effects": effects,
                "guest_commit_proof": proof,
                "delegate_identity": {
                    "delegate_kind": "rust_port",
                    "adapter": "fn64-render-reference",
                    "adapter_source_sha256": "0".repeat(64),
                    "source_id": "git:0000000000000000000000000000000000000000",
                    "source_provenance": "declared",
                    "source_overlay_id": "none",
                    "post_vi_api": "none",
                    "enabled_features": ["fn64-render-conformance/reference-runner"],
                },
            },
        });
        let request: EvaluationRequest = serde_json::from_value(request_value).unwrap();
        assert!(matches!(
            evaluate(&request),
            Err(WireError::DelegateIdentity)
        ));
    }

    /// A wrong answer must classify as `diverges`, not silently pass. The
    /// effects stay correct here so evaluation reaches classification rather
    /// than failing earlier at the guest-proof binding; the observation alone
    /// is wrong.
    #[test]
    fn a_wrong_observation_diverges() {
        let fixture = fixture_bundle().unwrap();
        let replay: ReplayFixture = serde_json::from_value(fixture["replay"].clone()).unwrap();
        let validated = validate_exact_replay(&replay).unwrap();
        let expected = expected_rdram().unwrap();
        let mut wrong = expected.clone();
        RdramViewMut::from_storage(&mut wrong)
            .write_u16(RdramAddr::from_offset(FRAMEBUFFER), STALE);
        let effects = actual_effects(&validated, &expected).unwrap();
        let challenge = "ef".repeat(32);
        let proof = guest_proof(
            validate_exact_replay(&replay).unwrap(),
            &effects,
            &challenge,
        )
        .unwrap();
        let request_value = json!({
            "replay": replay,
            "authority": fixture["authority"],
            "result": {
                "schema": RESULT_SCHEMA,
                "challenge": challenge,
                "pid": 1,
                "execution_status": "completed",
                "observation": {
                    "layer": CAPTURE_LAYER,
                    "bytes_hex": encode_hex(&observation_bytes(&wrong)),
                },
                "backend_effects": effects,
                "guest_commit_proof": proof,
                "delegate_identity": Value::Null,
            },
        });
        let request: EvaluationRequest = serde_json::from_value(request_value).unwrap();
        let evaluation = evaluate(&request).unwrap();
        assert_eq!(
            serde_json::to_value(evaluation).unwrap()["classification"],
            "diverges"
        );
    }
}
