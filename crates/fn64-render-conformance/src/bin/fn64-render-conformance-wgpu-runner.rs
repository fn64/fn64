//! wgpu-backend conformance runner for `feature::native-renderer-rdram-sync`.
//!
//! **This is the third engine adapter in the harness, and the first for the
//! backend fn64 actually ships.** It replays the *identical* fixture
//! `fn64-render-conformance-reference-runner` replays -- the same `ROW_ID`,
//! the same eight-command display list, the same 8x4 RGBA16 native target,
//! the same seeded bytes and the same region guards -- through
//! `fn64-render-wgpu`'s raw-DPC seam instead of `ReferenceBackend`. Two
//! runners answering one row against one independently-derived answer key is
//! what makes this a differential rather than two unrelated tests.
//!
//! Like the reference runner it is a non-RT64 runner, so it reports
//! `delegate_identity: None` (`tools/check_rt64_port_parity.py` requires
//! exactly that for a non-RT64 runner), and the entire pinned-source /
//! overlay / Metal identity block does not exist here.
//!
//! ## Adapterless
//!
//! No GPU adapter is requested or required. `ConformanceSession::try_new`
//! records the host-configured target extent and tolerates
//! `WgpuCreateError::NoAdapter`, and the bytes this runner observes come from
//! `ColorTargetRegistry`'s `device_bytes`, which is a CPU `Vec<u8>`.
//! Fill-cycle `FillRectangle` execution in this backend is CPU-side work.
//!
//! ## The answer key is the reference runner's, verbatim and independent
//!
//! `expected_pixel` is the same per-pixel arithmetic over the display list
//! the reference runner derives by hand from public RDP semantics, restated
//! here rather than imported so that a change to one runner's key cannot
//! silently move the other's. `expected_answer_matches_the_reference_runners_key`
//! pins that the two agree. Neither key is captured from any backend.
//!
//! ## What "diverges" means for this runner
//!
//! The runner reports what the engine produced; it never asserts equality
//! against the key. When `fn64-render-wgpu` refuses the fixture outright, the
//! refusal is reported as a `refused` execution status carrying the named
//! guard, which the verifier classifies as a non-`completed` result rather
//! than a pass. That is deliberate: a refusal is a real, reportable
//! disagreement with a backend that renders the same stream, and hiding it
//! behind a fabricated observation would be exactly the failure mode this
//! harness exists to prevent.

use std::{
    io::{self, Read, Write},
    process,
};

use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, ContentDigest, DecodedTicket,
    DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
    GuestCommitEffectReport, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
    TicketAuthoritySet, WorkloadAdmission, WorkloadPacket, WorkloadRecord,
};
use fn64_render_wgpu::conformance::{ConformanceReplay, ConformanceRefusal, ConformanceSession};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

#[path = "../wire.rs"]
#[allow(dead_code)]
mod wire;

use wire::*;

/// The identical row the reference runner answers. Same row, two engines.
const ROW_ID: &str = "feature::native-renderer-rdram-sync";
const CAPTURE_LAYER: &str = "resource_journal_guest_memory_effects";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMAND_START: u32 = 0x100;
const FRAMEBUFFER: u32 = 0x10_0000;
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;
const PIXEL_COUNT: u32 = WIDTH * HEIGHT;
const FRAMEBUFFER_BYTES: u32 = PIXEL_COUNT * 2;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const STALE: u16 = 0xffff;
const GUARD: u16 = 0x4211;

const fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

/// Byte-for-byte the reference runner's `COMMANDS`. Any drift between the two
/// arrays is caught by `command_stream_matches_the_reference_runners`.
const COMMANDS: [(u32, u32); 8] = [
    (0xef30_00f0, 0),
    (0xed00_0000, ((WIDTH * 4) << 12) | (HEIGHT * 4)),
    (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
    (0xf700_0000, (RED as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
    (0xf700_0000, (BLUE as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, WIDTH / 2, 0),
    (0xe900_0000, 0),
];

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
            "wgpu RDRAM-sync runner accepts only the exact reviewed native-target fixture".into(),
        );
    }
    Ok(validated)
}

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

/// The independently derived answer key, restated from the same public RDP
/// semantics the reference runner documents: fill-cycle `G_FILLRECT` covers
/// `ceil(ulx) ..= floor(lrx)` inclusive on both edges, so the first fill
/// covers every pixel with `RED` and the second covers exactly the right half
/// with `BLUE`. The renderer is not consulted.
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

fn observation_bytes(rdram: &[u8]) -> Vec<u8> {
    rdram[FRAMEBUFFER as usize..(FRAMEBUFFER + FRAMEBUFFER_BYTES) as usize].to_vec()
}

fn require_bounded_write(rdram: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let view = RdramView::from_storage(rdram);
    for address in [FRAMEBUFFER - 2, FRAMEBUFFER + FRAMEBUFFER_BYTES] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(format!(
                "wgpu RDRAM-sync write escaped its region at {address:#010x}: {actual:#06x}"
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
        return Err("wgpu RDRAM-sync fixture lost its single ordered write slot".into());
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
            let bytes = decode_hex(&effect.bytes_hex, "wgpu framebuffer effect")?;
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
        // A non-RT64 runner reports no delegate identity.
        "expected_delegate_identity": Value::Null,
    });
    Ok(json!({"replay": replay, "authority": authority}))
}

/// What the wgpu backend did with the fixture: either the framebuffer bytes
/// it published, or the named guard that refused it.
enum WgpuOutcome {
    Rendered(Vec<u8>),
    Refused(ConformanceRefusal),
}

/// Drive `fn64-render-wgpu`'s raw-DPC seam over the fixture's own command
/// stream, adapterlessly, and splice the published target bytes back into a
/// copy of the fixture's guest RDRAM.
///
/// The target bytes are the CPU-side `ColorTargetRegistry` resident, which is
/// exactly what `fn64-abi`'s `copy_committed_guest_writes` copies back into
/// guest memory in production. Splicing them here is that copy, performed by
/// the runner because no ABI is in the loop -- it copies the resident's own
/// bytes verbatim into the declared framebuffer range and touches nothing
/// else, so the guard halfwords either side stay whatever the seed left them.
fn render(rdram: &mut [u8]) -> Result<WgpuOutcome, Box<dyn std::error::Error>> {
    let mut session = match ConformanceSession::try_new(WIDTH, HEIGHT) {
        Ok(session) => session,
        Err(refusal) => return Ok(WgpuOutcome::Refused(refusal)),
    };
    let replay = ConformanceReplay {
        layout_bytes: RDRAM_LEN as u32,
        command_start: COMMAND_START,
        words: command_words(&COMMANDS),
        transaction_sequence: 1,
        guest_read_sources: Vec::new(),
        guest_rdram: Some(rdram.to_vec()),
        target_width: WIDTH,
        target_height: HEIGHT,
    };
    let outcome = match session.replay(&replay, FRAMEBUFFER) {
        Ok(outcome) => outcome,
        Err(refusal) => return Ok(WgpuOutcome::Refused(refusal)),
    };
    let published = outcome.target_bytes;
    if published.len() < FRAMEBUFFER_BYTES as usize {
        return Err(format!(
            "wgpu published {} target bytes, fewer than the {FRAMEBUFFER_BYTES} the fixture \
             declares",
            published.len()
        )
        .into());
    }
    // **Through `write_logical_bytes`, not a raw slice copy.** This is the
    // exact call `fn64-abi`'s `copy_committed_guest_writes` makes
    // (`task_dispatch/rsp_commit.rs`), and the difference is load-bearing:
    // `device_bytes` are flat big-endian device bytes, while guest RDRAM is
    // stored in native words under the `^3` byte-lane mapping `write_u8`
    // applies. A raw `copy_from_slice` here reported every pixel as
    // byte-swapped against the reference backend -- a runner defect that
    // would have been read as a renderer defect. Copying the way production
    // copies is what makes the two backends' observations comparable at all.
    RdramViewMut::from_storage(rdram).write_logical_bytes(
        RdramAddr::from_offset(FRAMEBUFFER),
        &published[..FRAMEBUFFER_BYTES as usize],
    );
    Ok(WgpuOutcome::Rendered(published))
}

fn run(request: RunnerRequest) -> Result<Value, Box<dyn std::error::Error>> {
    if request.schema != REQUEST_SCHEMA
        || request.ordinal >= fn64_render_conformance::REQUIRED_CLEAN_RUNS
    {
        return Err("invalid wgpu RDRAM-sync request schema or ordinal".into());
    }
    let replay = validate_exact_replay(&request.replay)?;
    let mut rdram = initial_rdram(&COMMANDS)?;
    // A refusal is reported, not hidden: the result carries `execution_status:
    // "refused"` and the guard's own message, which the verifier treats as a
    // non-`completed` result. Fabricating an observation to reach `completed`
    // is precisely the failure mode this harness exists to prevent.
    if let WgpuOutcome::Refused(refusal) = render(&mut rdram)? {
        return Ok(json!({
            "schema": RESULT_SCHEMA,
            "challenge": request.challenge,
            "pid": process::id(),
            "execution_status": "refused",
            "refusal": refusal.to_string(),
            "observation": {
                "layer": CAPTURE_LAYER,
                "bytes_hex": "",
            },
            "backend_effects": Vec::<EffectWire>::new(),
            "guest_commit_proof": Value::Null,
            "delegate_identity": Value::Null,
        }));
    }
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

/// Replay the row through this backend and report the diff against the
/// derived answer key, without any wire encoding. This is the differential
/// itself: `diff` names every disagreeing pixel, or the guard that refused.
fn diff() -> Result<Value, Box<dyn std::error::Error>> {
    let mut rdram = initial_rdram(&COMMANDS)?;
    let expected = expected_rdram()?;
    match render(&mut rdram)? {
        WgpuOutcome::Refused(refusal) => Ok(json!({
            "row_id": ROW_ID,
            "backend": "fn64-render-wgpu",
            "status": "refused",
            "refusal": refusal.to_string(),
        })),
        WgpuOutcome::Rendered(_) => {
            let observed = observation_bytes(&rdram);
            let key = observation_bytes(&expected);
            let disagreements: Vec<Value> = (0..PIXEL_COUNT as usize)
                .filter_map(|index| {
                    let actual = u16::from_be_bytes([observed[index * 2], observed[index * 2 + 1]]);
                    let want = u16::from_be_bytes([key[index * 2], key[index * 2 + 1]]);
                    (actual != want).then(|| {
                        json!({
                            "pixel": index,
                            "x": index as u32 % WIDTH,
                            "y": index as u32 / WIDTH,
                            "expected": format!("{want:#06x}"),
                            "actual": format!("{actual:#06x}"),
                        })
                    })
                })
                .collect();
            Ok(json!({
                "row_id": ROW_ID,
                "backend": "fn64-render-wgpu",
                "status": if disagreements.is_empty() { "agrees" } else { "diverges" },
                "pixels_compared": PIXEL_COUNT,
                "disagreeing_pixels": disagreements.len(),
                "disagreements": disagreements,
            }))
        }
    }
}


/// The differential proper: one family of fill fixtures, both backends, every
/// disagreement reported.
///
/// `diff` answers one row. This answers many, which is what converts "which
/// hypothesis explains the difference" into "here are the fixtures where the
/// two backends disagree, ranked". Each case varies exactly one thing about
/// the same display list -- rectangle geometry, fill colour, or the
/// scissor -- so a disagreement names its own cause.
///
/// **Neither backend is the authority here.** The sweep reports where they
/// differ from each other AND, for each, what the hand-derived key says, so a
/// disagreement can be attributed rather than merely counted.
mod sweep {
    use super::*;
    use fn64_render::{RenderBackend, RenderConfig};
    use fn64_render_reference::ReferenceBackend;

    /// One swept fixture: a display list, and the key that says what its
    /// committed framebuffer must contain.
    struct Case {
        name: &'static str,
        commands: Vec<(u32, u32)>,
        /// Hand-derived expected pixel, by linear index. Never captured.
        expected: fn(u32) -> u16,
    }

    fn one_fill(color: u16, ulx: u32, uly: u32, lrx: u32, lry: u32) -> Vec<(u32, u32)> {
        vec![
            (0xef30_00f0, 0),
            (0xed00_0000, ((WIDTH * 4) << 12) | (HEIGHT * 4)),
            (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER),
            (0xf700_0000, (color as u32) * 0x1_0001),
            fill_rect(lrx, lry, ulx, uly),
            (0xe900_0000, 0),
        ]
    }

    /// Every case's key is stated as arithmetic over its own display list,
    /// following the same fill-cycle rule the reference runner documents:
    /// `G_FILLRECT` covers `ceil(ulx) ..= floor(lrx)` INCLUSIVE on both
    /// edges, so a rectangle stated as `(0,0)..(3,3)` covers four columns and
    /// four rows, not three.
    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "full-target-red",
                commands: one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1),
                expected: |_| RED,
            },
            Case {
                name: "right-half-blue-over-red",
                commands: COMMANDS.to_vec(),
                expected: expected_pixel,
            },
            Case {
                // Inclusive on BOTH edges: columns 0..=3 and rows 0..=1 are
                // covered, everything else keeps the seeded bytes.
                name: "top-left-quadrant",
                commands: one_fill(RED, 0, 0, WIDTH / 2 - 1, HEIGHT / 2 - 1),
                expected: |index| {
                    if index % WIDTH < WIDTH / 2 && index / WIDTH < HEIGHT / 2 {
                        RED
                    } else {
                        STALE
                    }
                },
            },
            Case {
                // A single pixel. `ulx == lrx` is one column wide under the
                // inclusive rule, not zero -- the case a half-open reading
                // would drop entirely.
                name: "single-pixel",
                commands: one_fill(BLUE, 3, 2, 3, 2),
                expected: |index| {
                    if index == 2 * WIDTH + 3 {
                        BLUE
                    } else {
                        STALE
                    }
                },
            },
            Case {
                // The last column and last row, which is where an off-by-one
                // in either direction shows up first.
                name: "last-column-last-row",
                commands: one_fill(RED, WIDTH - 1, HEIGHT - 1, WIDTH - 1, HEIGHT - 1),
                expected: |index| {
                    if index == PIXEL_COUNT - 1 {
                        RED
                    } else {
                        STALE
                    }
                },
            },
            Case {
                // A colour whose LSB is CLEAR. RED and BLUE both have theirs
                // set, which is what makes their 5->8->5 round trip exact and
                // their stored coverage full. This one distinguishes a
                // backend that preserves the wire value from one that
                // round-trips it through 8 bits per channel.
                name: "even-color-lsb-clear",
                commands: one_fill(0xf800, 0, 0, WIDTH - 1, HEIGHT - 1),
                expected: |_| 0xf800,
            },
            Case {
                // Two fills where the SECOND is fully contained in the first.
                // Order matters; a backend that merged or reordered them
                // would show it here.
                name: "nested-second-fill",
                commands: {
                    let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                    words.pop();
                    words.push((0xf700_0000, (BLUE as u32) * 0x1_0001));
                    words.push(fill_rect(WIDTH - 2, HEIGHT - 2, 1, 1));
                    words.push((0xe900_0000, 0));
                    words
                },
                expected: |index| {
                    let (x, y) = (index % WIDTH, index / WIDTH);
                    if (1..=WIDTH - 2).contains(&x) && (1..=HEIGHT - 2).contains(&y) {
                        BLUE
                    } else {
                        RED
                    }
                },
            },
            Case {
                // A scissor NARROWER than the rectangle. The fill asks for
                // the whole target; the scissor admits only the left half.
                // A backend that ignores the scissor paints the right half
                // too.
                name: "scissor-narrower-than-rect",
                commands: {
                    let mut words = one_fill(RED, 0, 0, WIDTH - 1, HEIGHT - 1);
                    words[1] = (0xed00_0000, (((WIDTH / 2) * 4) << 12) | (HEIGHT * 4));
                    words
                },
                expected: |index| {
                    if index % WIDTH < WIDTH / 2 {
                        RED
                    } else {
                        STALE
                    }
                },
            },
        ]
    }

    fn seeded(commands: &[(u32, u32)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        initial_rdram(commands)
    }

    /// The reference backend's committed guest framebuffer for one case.
    fn reference_bytes(
        commands: &[(u32, u32)],
    ) -> Result<Result<Vec<u8>, String>, Box<dyn std::error::Error>> {
        let mut rdram = seeded(commands)?;
        let mut backend = ReferenceBackend::default();
        if let Err(error) = backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)) {
            return Ok(Err(error.to_string()));
        }
        let end = COMMAND_START + (commands.len() as u32) * 8;
        match backend.process_rdp_commands(&mut rdram, COMMAND_START, end, FRAMEBUFFER, true) {
            Ok(fn64_render::FrameStatus::Complete) => Ok(Ok(observation_bytes(&rdram))),
            Ok(status) => Ok(Err(format!("nonterminal status {status:?}"))),
            Err(error) => Ok(Err(error.to_string())),
        }
    }

    /// The wgpu backend's committed guest framebuffer for one case, copied
    /// back exactly the way production copies it.
    fn wgpu_bytes(
        commands: &[(u32, u32)],
    ) -> Result<Result<Vec<u8>, String>, Box<dyn std::error::Error>> {
        let mut rdram = seeded(commands)?;
        let mut session = match ConformanceSession::try_new(WIDTH, HEIGHT) {
            Ok(session) => session,
            Err(refusal) => return Ok(Err(refusal.to_string())),
        };
        let replay = ConformanceReplay {
            layout_bytes: RDRAM_LEN as u32,
            command_start: COMMAND_START,
            words: command_words(commands),
            transaction_sequence: 1,
            guest_read_sources: Vec::new(),
        guest_rdram: Some(rdram.to_vec()),
            target_width: WIDTH,
            target_height: HEIGHT,
        };
        let outcome = match session.replay(&replay, FRAMEBUFFER) {
            Ok(outcome) => outcome,
            Err(refusal) => return Ok(Err(refusal.to_string())),
        };
        let published = outcome.target_bytes;
        if published.len() < FRAMEBUFFER_BYTES as usize {
            return Ok(Err(format!(
                "published {} target bytes, fewer than the declared {FRAMEBUFFER_BYTES}",
                published.len()
            )));
        }
        RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            RdramAddr::from_offset(FRAMEBUFFER),
            &published[..FRAMEBUFFER_BYTES as usize],
        );
        Ok(Ok(observation_bytes(&rdram)))
    }

    /// The hand-derived key for one case, in the same guest byte order both
    /// backends' observations are read in.
    fn key_bytes(case: &Case) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut rdram = seeded(&case.commands)?;
        {
            let mut view = RdramViewMut::from_storage(&mut rdram);
            for index in 0..PIXEL_COUNT {
                view.write_u16(
                    RdramAddr::from_offset(FRAMEBUFFER + index * 2),
                    (case.expected)(index),
                );
            }
        }
        Ok(observation_bytes(&rdram))
    }

    fn pixels(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect()
    }

    /// Compare both backends against each other and against the key.
    pub fn run() -> Result<Value, Box<dyn std::error::Error>> {
        let mut rows = Vec::new();
        let mut disagreements = 0usize;
        for case in cases() {
            let key = pixels(&key_bytes(&case)?);
            let reference = reference_bytes(&case.commands)?;
            let wgpu = wgpu_bytes(&case.commands)?;
            let row = match (&reference, &wgpu) {
                (Ok(reference), Ok(wgpu)) => {
                    let (reference, wgpu) = (pixels(reference), pixels(wgpu));
                    let differing: Vec<Value> = (0..PIXEL_COUNT as usize)
                        .filter(|&index| reference[index] != wgpu[index])
                        .map(|index| {
                            json!({
                                "pixel": index,
                                "x": index as u32 % WIDTH,
                                "y": index as u32 / WIDTH,
                                "key": format!("{:#06x}", key[index]),
                                "reference": format!("{:#06x}", reference[index]),
                                "wgpu": format!("{:#06x}", wgpu[index]),
                            })
                        })
                        .collect();
                    if !differing.is_empty() {
                        disagreements += 1;
                    }
                    json!({
                        "case": case.name,
                        "status": if differing.is_empty() { "agree" } else { "disagree" },
                        "reference_matches_key": reference == key,
                        "wgpu_matches_key": wgpu == key,
                        "differing_pixels": differing.len(),
                        "differences": differing,
                    })
                }
                // A refusal by exactly one backend IS a disagreement, and the
                // most consequential kind: one engine renders the stream and
                // the other declines it.
                (reference, wgpu) => {
                    disagreements += 1;
                    json!({
                        "case": case.name,
                        "status": "disagree",
                        "kind": "one-backend-refused",
                        "reference": match reference {
                            Ok(_) => json!("completed"),
                            Err(message) => json!({"refused": message}),
                        },
                        "wgpu": match wgpu {
                            Ok(_) => json!("completed"),
                            Err(message) => json!({"refused": message}),
                        },
                    })
                }
            };
            rows.push(row);
        }
        Ok(json!({
            "backends": ["fn64-render-reference", "fn64-render-wgpu"],
            "cases": rows.len(),
            "disagreeing_cases": disagreements,
            "rows": rows,
        }))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("emit-replay") => stdout_json(io::stdout().lock(), &exact_replay()?)?,
        Some("emit-fixture") => stdout_json(io::stdout().lock(), &fixture_bundle()?)?,
        Some("diff") => stdout_json(io::stdout().lock(), &diff()?)?,
        Some("sweep") => stdout_json(io::stdout().lock(), &sweep::run()?)?,
        Some("run") => {
            let request = stdin_json::<RunnerRequest>()?;
            stdout_json(io::stdout().lock(), &run(request)?)?;
        }
        _ => {
            return Err(
                "usage: fn64-render-conformance-wgpu-runner emit-replay|emit-fixture|diff|run"
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

    /// The differential is only meaningful if both runners replay the SAME
    /// stream. Derived from the wire layout here, independently of the
    /// reference runner's array, and compared word by word.
    #[test]
    fn command_stream_matches_the_wire_layout_the_reference_runner_documents() {
        // G_RDPSETOTHERMODE with cycle_type = Fill.
        assert_eq!(COMMANDS[0], (0xef30_00f0, 0));
        // G_SETSCISSOR, lower-right exclusive, in 10.2 fixed point.
        assert_eq!(COMMANDS[1], (0xed00_0000, (32 << 12) | 16));
        // G_SETCIMG RGBA16, width-1 in the low bits, at FRAMEBUFFER.
        assert_eq!(COMMANDS[2], (0xff10_0007, 0x10_0000));
        // Two G_SETFILLCOLOR words, each halfword the same colour.
        assert_eq!(COMMANDS[3], (0xf700_0000, 0xf801_f801));
        assert_eq!(COMMANDS[5], (0xf700_0000, 0x003f_003f));
        // Two G_FILLRECT commands: whole target, then the right half.
        assert_eq!(COMMANDS[4], (0xf600_0000 | (28 << 12) | 12, 0));
        assert_eq!(COMMANDS[6], (0xf600_0000 | (28 << 12) | 12, 16 << 12));
        // G_RDPFULLSYNC.
        assert_eq!(COMMANDS[7], (0xe900_0000, 0));
    }

    /// The answer key must be independent of the engine: literal per-pixel
    /// arithmetic over the display list, no call into any backend.
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

    /// **The acceptance test for this runner.** It replays the row through
    /// `fn64-render-wgpu` adapterlessly and records what happened. It asserts
    /// only that the runner reached a *definite* answer -- rendered bytes or
    /// a named refusal -- because asserting agreement would make the
    /// differential unable to report a disagreement.
    #[test]
    fn wgpu_backend_reaches_a_definite_answer_on_the_shared_row() {
        let report = diff().unwrap();
        let status = report["status"].as_str().unwrap();
        assert!(
            matches!(status, "agrees" | "diverges" | "refused"),
            "the wgpu runner must reach a definite answer, got {report}"
        );
        if status == "refused" {
            assert!(
                !report["refusal"].as_str().unwrap().is_empty(),
                "a refusal must name its guard"
            );
        } else {
            assert_eq!(report["pixels_compared"], PIXEL_COUNT);
        }
    }

    /// A wrong answer must classify as `diverges`, not silently pass, on this
    /// runner's own wire path exactly as it does on the reference runner's.
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

    /// A runner that invented a delegate identity must be rejected: this
    /// runner is non-RT64 and the authority pins `None`.
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
                    "adapter": "fn64-render-wgpu",
                    "adapter_source_sha256": "0".repeat(64),
                    "source_id": "git:0000000000000000000000000000000000000000",
                    "source_provenance": "declared",
                    "source_overlay_id": "none",
                    "post_vi_api": "none",
                    "enabled_features": ["fn64-render-conformance/wgpu-runner"],
                },
            },
        });
        let request: EvaluationRequest = serde_json::from_value(request_value).unwrap();
        assert!(matches!(
            evaluate(&request),
            Err(WireError::DelegateIdentity)
        ));
    }
}
