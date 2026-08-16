#![cfg(target_os = "macos")]

use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::FromRawFd,
    process,
};

use fn64_render::{
    ActiveRenderGraphicsApi, AspectTarget, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, ContentDigest, DecodedTicket,
    DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
    GuestCommitEffectReport, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
    TicketAuthoritySet, WorkloadAdmission, WorkloadPacket, WorkloadRecord,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64DeferredWorkloadEvidence, Rt64DeferredWorkloadSnapshot, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

#[path = "../wire.rs"]
#[allow(dead_code)]
mod wire;

use wire::*;

const ROW_ID: &str = "feature::deferred-frame-history";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMAND_START: u32 = 0x100;
const COMMAND_END: u32 = 0x158;
const FRAMEBUFFER_A: u32 = 0x10_0000;
const FRAMEBUFFER_B: u32 = 0x14_0000;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const PIXEL_COUNT: u32 = WIDTH * HEIGHT;
const FRAMEBUFFER_BYTES: u32 = PIXEL_COUNT * 2;
const RED: u16 = 0xf801;
const GREEN: u16 = 0x07c1;
const BLUE: u16 = 0x003f;
const STALE: u16 = 0xffff;
const GUARD: u16 = 0x4211;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_OVERLAY: &str = "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3";
const CONTENT_DIGEST: u64 = 0xe330_7aad_eb40_0f41;
const IDENTITY_DIGEST: u64 = 0x9be9_6c04_3b6c_016e;
const ENABLED_FEATURES: [&str; 2] = [
    "fn64-render-conformance/rt64-deferred-history-runner",
    "fn64-render-rt64/rt64",
];

const fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

const COMMANDS: [(u32, u32); 11] = [
    (0xef30_00f0, 0),
    (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER_A),
    (0xf700_0000, (RED as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
    (0xf700_0000, (BLUE as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, WIDTH / 2, 0),
    (0xe700_0000, 0),
    (0xff10_0000 | (WIDTH - 1), FRAMEBUFFER_B),
    (0xf700_0000, (GREEN as u32) * 0x1_0001),
    fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
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
                range: layout.range(FRAMEBUFFER_A, FRAMEBUFFER_A + FRAMEBUFFER_BYTES)?,
            },
        )?,
        ResourceAccess::try_new(
            OperationId::new(2),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(FRAMEBUFFER_B, FRAMEBUFFER_B + FRAMEBUFFER_BYTES)?,
            },
        )?,
    ];
    let declared_bytes = command_range.len() + FRAMEBUFFER_BYTES * 2;
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
        capture_layer: "full_sync_timeline".into(),
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
        || validated.capture_layer.wire_name() != "full_sync_timeline"
        || validated.packet != packet_for_commands(&COMMANDS)?
    {
        return Err(
            "RT64 deferred-history runner accepts only the exact reviewed 0x100..0x158 fixture"
                .into(),
        );
    }
    Ok(validated)
}

fn initial_rdram(commands: &[(u32, u32)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for target in [FRAMEBUFFER_A, FRAMEBUFFER_B] {
            for index in 0..PIXEL_COUNT {
                view.write_u16(RdramAddr::from_offset(target + index * 2), STALE);
            }
            view.write_u16(RdramAddr::from_offset(target - 2), GUARD);
            view.write_u16(RdramAddr::from_offset(target + FRAMEBUFFER_BYTES), GUARD);
        }
    }
    for (index, &(word0, word1)) in commands.iter().enumerate() {
        let offset = COMMAND_START as usize + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    Ok(rdram)
}

fn require_exact_rdram(rdram: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let view = RdramView::from_storage(rdram);
    for index in 0..PIXEL_COUNT {
        let x = index % WIDTH;
        let expected_a = if x < WIDTH / 2 { RED } else { BLUE };
        let actual_a = view.read_u16(RdramAddr::from_offset(FRAMEBUFFER_A + index * 2));
        let actual_b = view.read_u16(RdramAddr::from_offset(FRAMEBUFFER_B + index * 2));
        if (actual_a, actual_b) != (expected_a, GREEN) {
            return Err(format!(
                "RT64 deferred fixture RDRAM differs at pixel {index}: A={actual_a:#06x}, B={actual_b:#06x}"
            )
            .into());
        }
    }
    for address in [
        FRAMEBUFFER_A - 2,
        FRAMEBUFFER_A + FRAMEBUFFER_BYTES,
        FRAMEBUFFER_B - 2,
        FRAMEBUFFER_B + FRAMEBUFFER_BYTES,
    ] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(
                format!("RT64 deferred fixture escaped at {address:#010x}: {actual:#06x}").into(),
            );
        }
    }
    Ok(())
}

fn expected_rdram() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut rdram = initial_rdram(&COMMANDS)?;
    let mut view = RdramViewMut::from_storage(&mut rdram);
    for index in 0..PIXEL_COUNT {
        let x = index % WIDTH;
        view.write_u16(
            RdramAddr::from_offset(FRAMEBUFFER_A + index * 2),
            if x < WIDTH / 2 { RED } else { BLUE },
        );
        view.write_u16(RdramAddr::from_offset(FRAMEBUFFER_B + index * 2), GREEN);
    }
    require_exact_rdram(&rdram)?;
    Ok(rdram)
}

fn audited_snapshot() -> Rt64DeferredWorkloadSnapshot {
    Rt64DeferredWorkloadSnapshot {
        workload_id: 1,
        present_id: 0,
        submission_frame: 0,
        content_digest: CONTENT_DIGEST,
        identity_digest: IDENTITY_DIGEST,
        framebuffer_pair_count: 2,
        projection_count: 2,
        game_call_count: 3,
        triangle_count: 6,
        vertex_count: 0,
        face_index_count: 0,
        rdp_param_count: 3,
        load_operation_count: 0,
        selected_framebuffer_index: -1,
        selected_draw_call_index: -1,
        selected_framebuffer_address: 0,
        paused: false,
        pair_color_addresses: [FRAMEBUFFER_A, FRAMEBUFFER_B, 0, 0],
        pair_game_call_counts: [2, 1, 0, 0],
        pair_projection_counts: [1, 1, 0, 0],
        call_uids: [0; 16],
        call_fill_colors: {
            let mut values = [0; 16];
            values[0] = u32::from(RED) * 0x1_0001;
            values[1] = u32::from(BLUE) * 0x1_0001;
            values[2] = u32::from(GREEN) * 0x1_0001;
            values
        },
        call_triangle_counts: {
            let mut values = [0; 16];
            values[..3].copy_from_slice(&[2, 2, 2]);
            values
        },
    }
}

fn require_exact_evidence(
    evidence: &Rt64DeferredWorkloadEvidence,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = audited_snapshot();
    if evidence.pre_submission != expected || evidence.current != expected {
        return Err(format!(
            "RT64 deferred snapshot is not the exact FullSync-owned queue slot: {evidence:#?}"
        )
        .into());
    }
    Ok(())
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn snapshot_observation(snapshot: &Rt64DeferredWorkloadSnapshot) -> Vec<u8> {
    let mut output = Vec::with_capacity(104);
    for value in [
        snapshot.workload_id,
        snapshot.present_id,
        snapshot.submission_frame,
        snapshot.content_digest,
        snapshot.identity_digest,
    ] {
        put_u64(&mut output, value);
    }
    for value in [
        snapshot.framebuffer_pair_count,
        snapshot.projection_count,
        snapshot.game_call_count,
        snapshot.triangle_count,
        snapshot.vertex_count,
        snapshot.face_index_count,
        snapshot.rdp_param_count,
        snapshot.load_operation_count,
    ] {
        put_u32(&mut output, value);
    }
    put_i32(&mut output, snapshot.selected_framebuffer_index);
    put_i32(&mut output, snapshot.selected_draw_call_index);
    put_u32(&mut output, snapshot.selected_framebuffer_address);
    put_u32(&mut output, u32::from(snapshot.paused));
    for value in snapshot.pair_color_addresses {
        put_u32(&mut output, value);
    }
    assert_eq!(output.len(), 104);
    output
}

fn actual_effects(
    replay: &ValidatedReplay,
    rdram: &[u8],
) -> Result<Vec<EffectWire>, Box<dyn std::error::Error>> {
    if replay.slots.len() != 2
        || replay.slots[0].0 != "effect-0000"
        || replay.slots[1].0 != "effect-0001"
    {
        return Err("RT64 deferred fixture lost its exact two ordered write slots".into());
    }
    Ok([
        (FRAMEBUFFER_A, FRAMEBUFFER_A + FRAMEBUFFER_BYTES),
        (FRAMEBUFFER_B, FRAMEBUFFER_B + FRAMEBUFFER_BYTES),
    ]
    .into_iter()
    .zip(&replay.slots)
    .map(|((start, end), (slot, _))| EffectWire {
        slot: slot.clone(),
        bytes_hex: encode_hex(&rdram[start as usize..end as usize]),
    })
    .collect())
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
            let bytes = decode_hex(&effect.bytes_hex, "RT64 framebuffer effect")?;
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
    let effects = actual_effects(&validated, &expected_rdram()?)?;
    let proof = guest_proof(validate_exact_replay(&replay)?, &effects, &"00".repeat(32))?;
    let guest_effect_identity = proof["guest_effect_identity"]
        .as_str()
        .ok_or("typed guest proof omitted its effect identity")?;
    let authority = json!({
        "schema": AUTHORITY_SCHEMA,
        "row_id": ROW_ID,
        "replay_identity": encode_hex(validated.identity.as_ref()),
        "expected_observation": {
            "layer": "full_sync_timeline",
            "bytes_hex": encode_hex(&snapshot_observation(&audited_snapshot())),
        },
        "expected_backend_effects": effects.clone(),
        "expected_guest_effects": effects,
        "expected_guest_effect_identity": guest_effect_identity,
        "expected_delegate_identity": delegate_identity_from_observed_api(
            ActiveRenderGraphicsApi::Metal,
        )?,
    });
    Ok(json!({"replay": replay, "authority": authority}))
}

fn delegate_identity_from_observed_api(
    graphics_api: ActiveRenderGraphicsApi,
) -> Result<DelegateIdentityWire, Box<dyn std::error::Error>> {
    if graphics_api != ActiveRenderGraphicsApi::Metal {
        return Err(format!(
            "RT64 deferred-history evidence requires a live Metal device, observed {graphics_api:?}"
        )
        .into());
    }
    let identity = Rt64Backend::release_identity_for_api(graphics_api);
    let value = DelegateIdentityWire {
        delegate_kind: "rt64".into(),
        adapter: identity.adapter.into(),
        adapter_source_sha256: identity.adapter_source_sha256.into(),
        source_id: identity.source_id.into(),
        source_provenance: match identity.source_provenance {
            Rt64SourceProvenance::GitClean => "git-clean",
            Rt64SourceProvenance::GitDirty => "git-dirty",
            Rt64SourceProvenance::Declared => "declared",
        }
        .into(),
        source_overlay_id: identity.source_overlay_id.into(),
        post_vi_api: identity.post_vi_api.into(),
        enabled_features: ENABLED_FEATURES
            .iter()
            .map(|value| (*value).into())
            .collect(),
    };
    require_delegate_identity(&value)?;
    Ok(value)
}

fn require_delegate_identity(
    identity: &DelegateIdentityWire,
) -> Result<(), Box<dyn std::error::Error>> {
    if identity.delegate_kind != "rt64"
        || identity.adapter != "fn64-render-rt64/rt64"
        || identity.adapter_source_sha256.len() != 64
        || !identity
            .adapter_source_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || identity.source_id != PINNED_SOURCE
        || identity.source_provenance != "git-clean"
        || identity.source_overlay_id != PINNED_OVERLAY
        || identity.post_vi_api != "metal-bgra8-unorm"
        || identity.enabled_features
            != ENABLED_FEATURES
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
    {
        return Err(format!(
            "RT64 runner identity is outside the exact reviewed build: {identity:?}"
        )
        .into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapturePhase {
    Created,
    Armed,
    Submitted,
    Read,
}

#[derive(Clone, Copy)]
enum CaptureEvent {
    Arm,
    Submit,
    Read,
}

fn transition(phase: &mut CapturePhase, event: CaptureEvent) -> Result<(), &'static str> {
    let next = match (*phase, event) {
        (CapturePhase::Created, CaptureEvent::Arm) => CapturePhase::Armed,
        (CapturePhase::Armed, CaptureEvent::Submit) => CapturePhase::Submitted,
        (CapturePhase::Submitted, CaptureEvent::Read) => CapturePhase::Read,
        _ => return Err("deferred capture lifecycle is out of order"),
    };
    *phase = next;
    Ok(())
}

struct CreatedBackend {
    backend: Rt64Backend,
    phase: CapturePhase,
    identity: DelegateIdentityWire,
}

struct ArmedBackend {
    backend: Rt64Backend,
    phase: CapturePhase,
    identity: DelegateIdentityWire,
}

struct SubmittedBackend {
    backend: Rt64Backend,
    phase: CapturePhase,
    identity: DelegateIdentityWire,
    rdram: Vec<u8>,
}

impl CreatedBackend {
    fn create() -> Result<Self, Box<dyn std::error::Error>> {
        let runtime = RenderRuntimeSettings {
            graphics_api: RenderGraphicsApi::Metal,
            filtering: RenderFiltering::Nearest,
            aspect_ratio: RenderAspectRatio::Manual,
            aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
            idle_work_active: false,
            developer_mode: false,
            ..RenderRuntimeSettings::default()
        };
        let mut backend = Rt64Backend::new().with_runtime_settings(runtime.clone());
        let configured_policy = backend.configured_runtime_policy();
        backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
        if backend.active_settings() != Some(&runtime) {
            return Err("RT64 did not activate the exact deferred-history settings".into());
        }
        if backend.active_runtime_policy() != Some(configured_policy) {
            return Err("RT64 did not activate the complete configured runtime policy".into());
        }
        let replacements = backend
            .active_replacement_settings()
            .ok_or("RT64 did not expose active replacement policy")?;
        if !replacements.packs.is_empty() {
            return Err("RT64 deferred-history evidence forbids replacement packs".into());
        }
        let live_graphics_api = backend.live_device_graphics_api_for_evidence()?;
        let identity = delegate_identity_from_observed_api(live_graphics_api)?;
        Ok(Self {
            backend,
            phase: CapturePhase::Created,
            identity,
        })
    }

    fn arm(mut self) -> Result<ArmedBackend, Box<dyn std::error::Error>> {
        transition(&mut self.phase, CaptureEvent::Arm)?;
        self.backend
            .enable_deferred_workload_capture_for_evidence()?;
        Ok(ArmedBackend {
            backend: self.backend,
            phase: self.phase,
            identity: self.identity,
        })
    }
}

impl ArmedBackend {
    fn submit(
        mut self,
        mut rdram: Vec<u8>,
    ) -> Result<SubmittedBackend, Box<dyn std::error::Error>> {
        transition(&mut self.phase, CaptureEvent::Submit)?;
        // The return status contains fn64's preflight FullSync classification.
        // This runner intentionally derives no observation from it.
        let _ = self.backend.process_rdp_commands(
            &mut rdram,
            COMMAND_START,
            COMMAND_END,
            FRAMEBUFFER_B,
            true,
        )?;
        Ok(SubmittedBackend {
            backend: self.backend,
            phase: self.phase,
            identity: self.identity,
            rdram,
        })
    }
}

impl SubmittedBackend {
    fn read(
        mut self,
    ) -> Result<
        (Rt64DeferredWorkloadEvidence, Vec<u8>, DelegateIdentityWire),
        Box<dyn std::error::Error>,
    > {
        transition(&mut self.phase, CaptureEvent::Read)?;
        let evidence = self.backend.deferred_workload_evidence()?;
        Ok((evidence, self.rdram, self.identity))
    }
}

fn run(request: RunnerRequest) -> Result<Value, Box<dyn std::error::Error>> {
    if request.schema != REQUEST_SCHEMA
        || request.ordinal >= fn64_render_conformance::REQUIRED_CLEAN_RUNS
    {
        return Err("invalid RT64 deferred-history request schema or ordinal".into());
    }
    let replay = validate_exact_replay(&request.replay)?;
    let rdram = initial_rdram(&COMMANDS)?;
    let (evidence, rdram, identity) = CreatedBackend::create()?.arm()?.submit(rdram)?.read()?;
    require_exact_evidence(&evidence)?;
    require_exact_rdram(&rdram)?;
    let effects = actual_effects(&replay, &rdram)?;
    let proof = guest_proof(replay, &effects, &request.challenge)?;
    Ok(json!({
        "schema": RESULT_SCHEMA,
        "challenge": request.challenge,
        "pid": process::id(),
        "execution_status": "completed",
        "observation": {
            "layer": "full_sync_timeline",
            "bytes_hex": encode_hex(&snapshot_observation(&evidence.pre_submission)),
        },
        "backend_effects": effects,
        "guest_commit_proof": proof,
        "delegate_identity": identity,
    }))
}

fn redirect_native_stdout() -> Result<File, Box<dyn std::error::Error>> {
    io::stdout().lock().flush()?;
    // SAFETY: flushing the process C stream before duplicating descriptors
    // prevents buffered native diagnostics from crossing the redirection.
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }
    // SAFETY: dup returns a new owned descriptor or -1. No Rust owner exists
    // until the success branch constructs exactly one File below.
    let protocol_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if protocol_fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: both descriptors are valid process descriptors. Native stdout
    // is sent to the already-captured stderr pipe before RT64/SDL starts.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        let error = io::Error::last_os_error();
        // SAFETY: protocol_fd is the still-unowned successful dup above.
        unsafe {
            libc::close(protocol_fd);
        }
        return Err(error.into());
    }
    // SAFETY: protocol_fd is a unique owned descriptor after successful dup.
    Ok(unsafe { File::from_raw_fd(protocol_fd) })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("emit-replay") => stdout_json(io::stdout().lock(), &exact_replay()?)?,
        Some("emit-fixture") => stdout_json(io::stdout().lock(), &fixture_bundle()?)?,
        Some("run") => {
            let request = stdin_json::<RunnerRequest>()?;
            let output = redirect_native_stdout()?;
            stdout_json(output, &run(request)?)?;
        }
        _ => {
            return Err(
                "usage: fn64-render-conformance-rt64-deferred-history-runner emit-replay|emit-fixture|run"
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
    fn pipe_sync_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[6] = (0, 0);
        rejects(&commands);
    }

    #[test]
    fn double_full_sync_is_rejected() {
        let mut commands = COMMANDS.to_vec();
        commands.insert(10, (0xe900_0000, 0));
        rejects(&commands);
    }

    #[test]
    fn fill_value_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[2].1 ^= 1;
        rejects(&commands);
    }

    #[test]
    fn framebuffer_target_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands[7].1 = FRAMEBUFFER_B + 8;
        rejects(&commands);
    }

    #[test]
    fn command_order_is_part_of_the_exact_fixture() {
        let mut commands = COMMANDS;
        commands.swap(4, 6);
        rejects(&commands);
    }

    #[test]
    fn no_arm_and_read_before_submit_are_rejected() {
        let mut phase = CapturePhase::Created;
        assert!(transition(&mut phase, CaptureEvent::Submit).is_err());
        assert!(transition(&mut phase, CaptureEvent::Read).is_err());
        transition(&mut phase, CaptureEvent::Arm).unwrap();
        assert!(transition(&mut phase, CaptureEvent::Read).is_err());
    }

    #[test]
    fn queue_slot_identity_must_stay_current() {
        let pre = audited_snapshot();
        let mut current = pre;
        current.workload_id += 1;
        assert!(require_exact_evidence(&Rt64DeferredWorkloadEvidence {
            pre_submission: pre,
            current,
        })
        .is_err());
    }

    #[test]
    fn observation_is_104_bytes_and_fieldwise_big_endian() {
        let snapshot = audited_snapshot();
        let bytes = snapshot_observation(&snapshot);
        assert_eq!(bytes.len(), 104);
        assert_eq!(&bytes[0..8], &1_u64.to_be_bytes());
        assert_eq!(&bytes[8..16], &0_u64.to_be_bytes());
        assert_eq!(&bytes[16..24], &0_u64.to_be_bytes());
        assert_eq!(&bytes[24..32], &CONTENT_DIGEST.to_be_bytes());
        assert_eq!(&bytes[32..40], &IDENTITY_DIGEST.to_be_bytes());
        assert_eq!(
            &bytes[40..72],
            &[2_u32, 2, 3, 6, 0, 0, 3, 0].map(u32::to_be_bytes).concat()
        );
        assert_eq!(&bytes[72..76], &(-1_i32).to_be_bytes());
        assert_eq!(&bytes[76..80], &(-1_i32).to_be_bytes());
        assert_eq!(&bytes[80..88], &[0; 8]);
        assert_eq!(&bytes[88..92], &FRAMEBUFFER_A.to_be_bytes());
        assert_eq!(&bytes[92..96], &FRAMEBUFFER_B.to_be_bytes());
        assert_eq!(&bytes[96..104], &[0; 8]);
    }

    #[test]
    fn framebuffer_effects_and_guards_are_exact() {
        let mut rdram = initial_rdram(&COMMANDS).unwrap();
        {
            let mut view = RdramViewMut::from_storage(&mut rdram);
            for index in 0..PIXEL_COUNT {
                let x = index % WIDTH;
                view.write_u16(
                    RdramAddr::from_offset(FRAMEBUFFER_A + index * 2),
                    if x < WIDTH / 2 { RED } else { BLUE },
                );
                view.write_u16(RdramAddr::from_offset(FRAMEBUFFER_B + index * 2), GREEN);
            }
        }
        require_exact_rdram(&rdram).unwrap();
        RdramViewMut::from_storage(&mut rdram)
            .write_u16(RdramAddr::from_offset(FRAMEBUFFER_A + 4), STALE);
        assert!(require_exact_rdram(&rdram).is_err());
        let mut rdram = initial_rdram(&COMMANDS).unwrap();
        RdramViewMut::from_storage(&mut rdram)
            .write_u16(RdramAddr::from_offset(FRAMEBUFFER_A - 2), 0);
        assert!(require_exact_rdram(&rdram).is_err());
    }

    #[test]
    fn challenge_must_be_fresh_shape() {
        let replay = validate_exact_replay(&exact_replay().unwrap()).unwrap();
        let rdram = initial_rdram(&COMMANDS).unwrap();
        let effects = actual_effects(&replay, &rdram).unwrap();
        assert!(guest_proof(replay, &effects, "00").is_err());
    }

    #[test]
    fn source_and_feature_mutations_are_rejected() {
        let mut identity =
            delegate_identity_from_observed_api(ActiveRenderGraphicsApi::Metal).unwrap();
        identity.source_id = "git:0000000000000000000000000000000000000000".into();
        assert!(require_delegate_identity(&identity).is_err());
        let mut identity =
            delegate_identity_from_observed_api(ActiveRenderGraphicsApi::Metal).unwrap();
        identity.enabled_features.push("unreviewed".into());
        assert!(require_delegate_identity(&identity).is_err());
    }

    #[test]
    fn live_graphics_api_must_be_metal() {
        assert!(delegate_identity_from_observed_api(ActiveRenderGraphicsApi::Metal).is_ok());
        assert!(delegate_identity_from_observed_api(ActiveRenderGraphicsApi::D3d12).is_err());
        assert!(delegate_identity_from_observed_api(ActiveRenderGraphicsApi::Vulkan).is_err());
    }

    #[test]
    fn private_authority_binds_exact_observation_effects_and_delegate() {
        let fixture = fixture_bundle().unwrap();
        let replay: ReplayFixture = serde_json::from_value(fixture["replay"].clone()).unwrap();
        let validated = validate_exact_replay(&replay).unwrap();
        let effects = actual_effects(&validated, &expected_rdram().unwrap()).unwrap();
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
                    "layer": "full_sync_timeline",
                    "bytes_hex": encode_hex(&snapshot_observation(&audited_snapshot())),
                },
                "backend_effects": effects,
                "guest_commit_proof": proof,
                "delegate_identity": delegate_identity_from_observed_api(
                    ActiveRenderGraphicsApi::Metal,
                )
                .unwrap(),
            },
        });
        let request: EvaluationRequest = serde_json::from_value(request_value.clone()).unwrap();
        let evaluation = evaluate(&request).unwrap();
        assert_eq!(
            serde_json::to_value(evaluation).unwrap()["classification"],
            "pass"
        );

        let mut wrong_source = request_value.clone();
        wrong_source["result"]["delegate_identity"]["source_id"] =
            Value::String("git:0000000000000000000000000000000000000000".into());
        let wrong_source: EvaluationRequest = serde_json::from_value(wrong_source).unwrap();
        assert!(matches!(
            evaluate(&wrong_source),
            Err(WireError::DelegateIdentity)
        ));

        let mut wrong_features = request_value;
        wrong_features["result"]["delegate_identity"]["enabled_features"] =
            json!(["fn64-render-rt64/rt64"]);
        let wrong_features: EvaluationRequest = serde_json::from_value(wrong_features).unwrap();
        assert!(matches!(
            evaluate(&wrong_features),
            Err(WireError::DelegateIdentity)
        ));
    }
}
