//! Shared fixtures for `production`'s unit tests.
//!
//! Moved verbatim out of `production.rs`'s trailing `#[cfg(test)] mod
//! tests` block. The tests themselves live in the four sibling concern
//! modules below; every helper, fixture constant and wire-word builder
//! they share stays here so each child can reach it through `use
//! super::*`.

mod census;
mod color;
mod execute;
mod plan;

fn completed_cpu_accumulator(
    candidate: crate::targets::CandidateColorTarget,
    bytes: Vec<u8>,
    coverage: crate::targets::ColorCoverageState,
) -> crate::InitializedCandidateColorTarget {
    let key = candidate.key();
    let generation = candidate.generation();
    let rectangle = crate::targets::TargetRectangle::try_new(
        0,
        0,
        key.extent().width(),
        key.extent().height(),
    )
    .unwrap();
    let device =
        crate::targets::DeviceColorBytes::new_for_fill(key, generation, key.format(), bytes)
            .unwrap();
    candidate
        .admit_completed_initialization(
            crate::targets::CompletedColorTargetWrite::new_for_fill(
                key,
                generation,
                key.range(),
                rectangle,
                device,
            )
            .with_coverage(coverage),
        )
        .unwrap()
}

use fn64_render::OwnedRawDpcSubmission;

use fn64_render_ir::{
    CapturedGuestRead, DeferredGuestReadCapture, DpInterruptState, TemporalBoundary,
};

use crate::{
    BlendBInput, BlenderCycle, ImageFormat, PixelSize, TileAddressMode, TileCoordinate,
    TileDescriptor, TileSize, TmemWordAddress,
};

use super::*;

const LAYOUT_BYTES: u32 = 0x4000;

const COMMAND_START: u32 = 0x1000;

const SET_TEXTURE_IMAGE: u8 = 0x3d;

const SET_TILE: u8 = 0x35;

const SET_TILE_SIZE_OPCODE: u8 = 0x32;

const LOAD_SYNC: u8 = 0x26;

const LOAD_BLOCK: u8 = 0x33;

const FULL_SYNC: u8 = 0x29;

const SET_OTHER_MODE: u8 = 0x2f;

// False positive (dead_code): only read from shaded_covering_triangle_words,
// which is itself only called from #[cfg(feature = "host-gpu-tests")] tests.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
const SET_COMBINE: u8 = 0x3c;

const SET_ENV_COLOR: u8 = 0x3b;

const RAW_TRIANGLE_BASE_EDGE: u8 = 0x08;

use crate::wire_words::word;

fn indexed_source_rows(
    first_access: u32,
    rows: &[Vec<u8>],
) -> (CapturedGuestReadAuthority, Vec<ResourceAccess>) {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let mut authority = CapturedGuestReadAuthority::default();
    authority
        .by_access
        .resize_with(first_access as usize + rows.len(), || None);
    let mut accesses = Vec::with_capacity(rows.len());
    for (ordinal, bytes) in rows.iter().enumerate() {
        let access_index = first_access + ordinal as u32;
        let start = ordinal as u32 * bytes.len() as u32;
        let access = ResourceAccess::try_new(
            fn64_render_ir::OperationId::new(access_index),
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: fn64_render_ir::RdramResource::Buffer,
                range: layout.range(start, start + bytes.len() as u32).unwrap(),
            },
        )
        .unwrap();
        authority.by_access[access_index as usize] = Some(IndexedCapturedGuestRead {
            access,
            bytes: CapturedGuestReadBytes::copied(bytes),
        });
        accesses.push(access);
    }
    (authority, accesses)
}

use crate::wire_words::set_other_mode;

use crate::wire_words::set_combine;

/// Mirrors `raw_dpc::production_adapter::tests::set_env_color` exactly
/// (that helper is private to its own module's tests, so this is a
/// local, identical copy, not a shared import -- same convention as
/// `triangle_base_edge_words` above).
fn set_env_color(color: u32) -> [u32; 2] {
    [word(SET_ENV_COLOR, 0), color]
}

use crate::wire_words::set_prim_color;

/// One base-edge (non-shaded, non-textured, non-Z) triangle command's
/// eight raw wire words, from the crate's shared `wire_words` builder.
fn triangle_base_edge_words(tile: u32, level: u32, yl: u16) -> [u32; 8] {
    let mut words = crate::wire_words::EdgeWords {
        tile,
        level,
        yl: yl as i16,
        ..crate::wire_words::EdgeWords::zeroed()
    }
    .words(0, RAW_TRIANGLE_BASE_EDGE);
    // This fixture's own edge payload, unchanged: an arbitrary but fixed
    // set of slopes that exercises decode without naming a footprint.
    words[2..].copy_from_slice(&[0x0010_0000, 0, 0x0020_0000, 0x0000_8000, 0x0005_0000, 0]);
    words
}

fn set_texture_image(format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
    [
        word(SET_TEXTURE_IMAGE, format << 21 | size << 19 | (width - 1)),
        address,
    ]
}

fn set_tile(tile: u32, line: u32, tmem: u32) -> [u32; 2] {
    [word(SET_TILE, 2 << 19 | line << 9 | tmem), tile << 24]
}

/// One `SetTileSize` command's two wire words.
///
/// Field placement is `tmem::wire`'s own `tile_size` decode read
/// backwards: **low** S/T live in w0 (bits 23:12 and 11:0), **high**
/// S/T in w1 (same two positions), and the tile index in w1 bits 26:24.
/// All four are raw 10.2 fixed-point fields. Getting this pair the
/// wrong way round is not a silent error -- it produced a
/// `ReversedClampExtent` refusal naming `low 0x01c, high 0x000`, which
/// is how the swap was caught.
fn set_tile_size_words(tile: u32, high_s: u32, high_t: u32) -> [u32; 2] {
    [
        word(SET_TILE_SIZE_OPCODE, 0),
        tile << 24 | high_s << 12 | high_t,
    ]
}

fn load_sync() -> [u32; 2] {
    [word(LOAD_SYNC, 0), 0]
}

/// One admitted, TMEM-only raw-DPC command stream: SetTextureImage,
/// SetTile, LoadSync, LoadBlock -- the same admitted TMEM/state subset
/// v11 freezes, and the same fixture shape T1's own
/// `production_adapter::tests` module uses.
fn one_load_block_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
    words
}

/// A mixed plan: one admitted `TmemLoad` (identical shape to
/// `one_load_block_words`) PLUS `SetOtherMode`/`SetCombine`/one
/// admitted `RawTriangle` -- proves the loads+triangle branch-selection
/// rule (§1c): a plan with at least one TMEM load must always take the
/// real successor route (`complete_execution`), never the
/// preserving-physical route, regardless of the triangle's presence.
// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn mixed_load_and_triangle_words() -> Vec<u32> {
    let mut words = one_load_block_words();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(triangle_base_edge_words(7, 2, 0));
    words
}

/// A triangle-only plan: `SetOtherMode`/`SetCombine`/one admitted
/// `RawTriangle`, zero TMEM loads -- exercises `stage_and_report`'s
/// `StagedOutcome::NoPhysicalSuccessor` arm and
/// `RawDpcCoordinator::complete_execution_preserving_physical`.
fn triangle_only_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(triangle_base_edge_words(7, 2, 0));
    words
}

// False positive (dead_code): only read from
// shaded_covering_triangle_words below, itself only called from
// #[cfg(feature = "host-gpu-tests")] tests.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
const RAW_TRIANGLE_SHADED: u8 = 0x0c;

/// A shaded (0x0c), non-textured, non-Z triangle covering the whole
/// 8x8 target with a FLAT uniform shade color -- mirrors
/// `targets::triangle_pipeline::tests::host_gpu_tests::
/// shaded_covering_triangle_words` exactly (see that function's own
/// doc for the full field-by-field derivation); duplicated here, not
/// imported, since that helper is private to its own module's tests.
// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn shaded_covering_triangle_words(color_255: [u32; 4]) -> Vec<u32> {
    let mut words = vec![
        word(RAW_TRIANGLE_SHADED, 32u32),
        0,
        (8i32 << 16) as u32,
        0,
        0,
        0,
        0,
        0,
    ];
    let base_w0 = (color_255[0] << 16) | (color_255[1] & 0xffff);
    let base_w1 = (color_255[2] << 16) | (color_255[3] & 0xffff);
    words.extend([
        base_w0, base_w1, // shade[0]
        0, 0, // shade[1] (dx)
        0, 0, // shade[2] (base low half, zero)
        0, 0, // shade[3] (dx low half)
        0, 0, // shade[4] (de)
        0, 0, // shade[5] (unused by decode_shade)
        0, 0, // shade[6] (de low half)
        0, 0, // shade[7] (unused)
    ]);
    words
}

/// Two independent `LoadBlock`s in one submission, each preceded by its
/// own `SetTile`/`LoadSync` (a fresh `LoadSync` mints a strictly newer
/// `TmemLoadEpoch`, satisfying `neutral_validate_transfer`'s
/// `EpochNotNewer` ordering check) and targeting disjoint TMEM word
/// offsets (`tmem=0` vs `tmem=0x100`) so their destination ranges never
/// overlap. This is the only fixture in this module that exercises
/// `PhysicalTmemPacketTransaction::stage_neutral_transfer_next` -- every
/// other fixture here has exactly one load, which never chains past
/// `PhysicalTmemState::stage_neutral_transfer`'s first-load path.
fn two_load_block_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
    words.extend(set_tile(6, 2, 0x100));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 6 << 24 | 9 << 12 | 0x0800]);
    words
}

fn capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
    let submission =
        OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
    fn64_render::OwnedRawDpcCapture::new(
        submission,
        layout,
        7,
        TemporalBoundary::new(1, DpInterruptState::Clear),
    )
}

/// Same fixture shape as `capture`, but carrying one `FullSyncBoundary`
/// per `SYNC_FULL` opcode in `words` -- what a producer that took the
/// nonmutating `preflight_dp_full_sync` reserve half supplies.
///
/// Both interrupt states are `Clear`. That mirrors the real ABI producer
/// exactly: a reservation observes no interrupt, and the device fabric
/// raises the DP line only on a later `advance_to`, strictly after this
/// capture would have been built.
fn full_sync_capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
    // `Complete` specifically: a test fixture that ends inside a command
    // is a broken fixture, not a stall to tolerate.
    let sites = fn64_render::count_raw_rdp_full_sync_sites(&words)
        .unwrap()
        .complete()
        .expect("test fixture command words must form whole commands");
    let submission =
        OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
    let boundaries = (0..sites as u64)
        .map(|ordinal| {
            fn64_render_ir::FullSyncBoundary::new(
                2 + ordinal * 2,
                3 + ordinal * 2,
                DpInterruptState::Clear,
                DpInterruptState::Clear,
            )
        })
        .collect();
    fn64_render::OwnedRawDpcCapture::with_full_sync_boundaries(
        submission,
        layout,
        7,
        TemporalBoundary::new(1, DpInterruptState::Clear),
        boundaries,
    )
}

/// Same fixture shape as `capture`, but sourced from XBUS/DMEM instead
/// of RDRAM -- T4's second ABI producer shape (MMIO XBUS, RSP XBUS).
/// `RawDpcSource::XbusDmem` bounds ranges to the 4 KiB DMEM bank
/// (`OwnedRawDpcSubmission::validate_range`), unlike the RDRAM-bounded
/// `capture` helper above, so this starts at DMEM offset 0.
fn xbus_capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let start = 0u32;
    let end = u32::try_from(words.len() * 4).unwrap();
    let payload: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
    let submission = OwnedRawDpcSubmission::from_xbus_payload(start, end, payload).unwrap();
    fn64_render::OwnedRawDpcCapture::new(
        submission,
        layout,
        7,
        TemporalBoundary::new(1, DpInterruptState::Clear),
    )
}

/// Drives `backend.plan_raw_dpc` for real (through the two-pass probe
/// internal to `plan_raw_dpc_inner`), fills the plan's own deferred
/// guest-read plan with deterministic bytes, and returns the sealed
/// `PlannedRawDpcSubmission` plus the bytes used (so a hostile test can
/// assert on the physical postimage those bytes should produce).
fn plan_with_deterministic_reads(
    backend: &mut WgpuBackend,
    session: &RawDpcAbiSession,
    words: Vec<u32>,
) -> (PlannedRawDpcSubmission, Vec<u8>) {
    let request = session.plan_request(capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("fixture plans cleanly");
    let source_bytes: Vec<u8> = (0..planned.guest_read_plan().reads()[0].range().len())
        .map(|index| index as u8)
        .collect();
    (planned, source_bytes)
}

/// Same as `plan_with_deterministic_reads`, but for a fixture that
/// declares no TMEM-load reads -- `plan_with_deterministic_reads`'s own
/// `reads()[0]` indexing would panic on an empty guest-read plan.
///
/// **It no longer asserts the plan declares NO reads at all.** That
/// assertion held while `TmemLoadSource` meant "a TMEM load", and a
/// partial `FillRectangle` now declares one too, for its colour-image
/// seed (`raw_dpc::plan_fill`). The two are indistinguishable by
/// purpose, which is a real cost of reusing `TmemLoadSource` for the
/// seed and is recorded in `docs/RT64-FILL-PARTIAL-SEED.md`.
///
/// What it asserts instead is the fact the callers actually depend on:
/// no read of a TEXTURE buffer. A seed read names
/// `RdramResource::ColorFramebuffer`, so the two remain separable by
/// resource even though the purpose no longer separates them.
fn plan_with_no_reads(
    backend: &mut WgpuBackend,
    session: &RawDpcAbiSession,
    words: Vec<u32>,
) -> PlannedRawDpcSubmission {
    let request = session.plan_request(capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("fixture plans cleanly");
    assert!(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .all(|read| read.resource() == fn64_render_ir::RdramResource::ColorFramebuffer),
        "this fixture must declare no TMEM-load source reads; only colour-image seeds"
    );
    planned
}

/// Plans a multi-load fixture and fills *every* read the resulting
/// `guest_read_plan` declares (one `TmemLoadSource` per load) with its
/// own deterministic byte pattern, keyed by read index so two reads of
/// equal length still get distinguishable content -- unlike
/// `plan_with_deterministic_reads`/`guest_read_capture` above, which
/// only ever fill (and only ever need to fill) a single load's one read.
fn plan_with_deterministic_reads_for_every_load(
    backend: &mut WgpuBackend,
    session: &RawDpcAbiSession,
    words: Vec<u32>,
) -> (PlannedRawDpcSubmission, Vec<Vec<u8>>) {
    let request = session.plan_request(capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("fixture plans cleanly");
    let per_read_bytes: Vec<Vec<u8>> = planned
        .guest_read_plan()
        .reads()
        .iter()
        .enumerate()
        .map(|(read_index, read)| {
            (0..read.range().len())
                .map(|byte_index| (read_index as u8).wrapping_add(byte_index as u8))
                .collect()
        })
        .collect();
    (planned, per_read_bytes)
}

fn guest_read_capture_per_read(
    planned: &PlannedRawDpcSubmission,
    per_read_bytes: &[Vec<u8>],
) -> DeferredGuestReadCapture {
    DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .zip(per_read_bytes)
            .map(|(read, bytes)| CapturedGuestRead::try_new(*read, bytes.clone()).unwrap())
            .collect(),
    )
}

/// Every declared read gets `source_bytes`, resized to that read's own
/// declared length.
///
/// The resize is not cosmetic. This used to hand the same slice to
/// every read, which held while all of them were TMEM loads of one
/// fixture texture. A partial fill's colour-image seed is declared
/// alongside those and is sized by the target, not the texture, so the
/// unresized version failed `GuestReadByteCountMismatch` (expected 256,
/// actual 48).
///
/// Padding is zero, which is safe HERE and nowhere near a seed
/// assertion: these fixtures assert on texels the fill overwrites, not
/// on seeded pixels. A fixture that asserts seed content must state its
/// own bytes -- see `capture_declared_reads`.
fn guest_read_capture(
    planned: &PlannedRawDpcSubmission,
    source_bytes: &[u8],
) -> DeferredGuestReadCapture {
    DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                let mut bytes = source_bytes.to_vec();
                bytes.resize(read.range().len() as usize, 0);
                CapturedGuestRead::try_new(*read, bytes).unwrap()
            })
            .collect(),
    )
}

fn captured_binding_fixture() -> (DeferredGuestRead, Vec<ResourceAccess>, Vec<u8>) {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let (planned, bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let read = planned.guest_read_plan().reads()[0];
    let access = ResourceAccess::try_new(
        read.operation(),
        AccessMode::Read,
        AccessPurpose::TmemLoadSource,
        ResourceRegion::Rdram {
            resource: read.resource(),
            range: read.range(),
        },
    )
    .unwrap();
    let layout = read.range().layout();
    let mut accesses: Vec<ResourceAccess> = (0..=read.access_index())
        .map(|index| {
            ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(index),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::RawCommands,
                    range: layout.range(0, 4).unwrap(),
                },
            )
            .unwrap()
        })
        .collect();
    accesses[read.access_index() as usize] = access;
    (read, accesses, bytes)
}

fn admitted_fabric(
) -> fn64_runtime::DeviceFabric<fn64_runtime::rom::InMemoryRom, fn64_runtime::FixedPiTiming>
{
    let mut fabric = fn64_runtime::DeviceFabric::new(
        fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
        fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
    );
    fabric
        .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
        .unwrap()
        .expect("fresh fabric is never frozen");
    fabric
}

// False positive (dead_code): only read from texrect_words, itself only
// called from #[cfg(feature = "host-gpu-tests")] tests.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
const TEXRECT: u8 = 0x24;

const TEXRECT_FLIP: u8 = 0x25;

/// One `TextureRectangle`/`TextureRectangleFlip` command's 4-word wire
/// payload -- same bit layout as `raw_dpc::production_adapter`'s own
/// `texrect_words`, but this fixture's `ulx=8, uly=8, lrx=24, lry=24`
/// (2.0/2.0/6.0/6.0px, `.2` fixed point) places a 4x4-pixel rectangle
/// entirely inside `test_render_config`'s 8x8 target, at `[2, 6) x
/// [2, 6)`, unlike `production_adapter.rs`'s own fixture (which targets
/// a much larger, offscreen-for-8x8 render target). `dsdx=dtdy=0`
/// (constant `uls=ult=0` texcoord for every vertex) keeps every covered
/// fragment's sample well inside the 2x2 tile's interior, including the
/// 3-nearest filter's `+1` neighbor read -- this fixture's job is to
/// prove the rectangle's real pixel POSITION, not to exercise a UV
/// gradient (`required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`
/// already covers gradient/interpolation correctness for a `RawTriangle`).
// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn texrect_words(opcode: u8, tile: u32) -> [u32; 4] {
    let ulx: u32 = 8;
    let uly: u32 = 8;
    let lrx: u32 = 24;
    let lry: u32 = 24;
    let uls: u32 = 0;
    let ult: u32 = 0;
    let dsdx: u32 = 0x0000;
    let dtdy: u32 = 0x0000;
    [
        word(opcode, (lrx << 12) | lry),
        (tile & 0x7) << 24 | (ulx << 12) | uly,
        (uls << 16) | ult,
        (dsdx << 16) | dtdy,
    ]
}

/// Loads this module's frozen 2x2 RGBA16 texel fixture, commits, and
/// publishes it, exactly like
/// `required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`'s
/// own load-then-draw split: `project_committed_tmem` only reflects the
/// coordinator's ACTIVE (already-published) physical slot, never a load
/// still pending within the same `execute_raw_dpc` call -- so a
/// texture-sampling draw must be a SEPARATE, later `execute_raw_dpc`
/// from its own load, not batched into one command stream with it.
// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn load_and_publish_fixture_texture(backend: &mut WgpuBackend, session: &mut RawDpcAbiSession) {
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(load_sync());
    let source_bytes = fixture_load_block_source_bytes();
    words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

    let (planned, _unused_deterministic_bytes) =
        plan_with_deterministic_reads(backend, session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture's TMEM-only load stays inside the admitted subset");
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
}

#[cfg(feature = "host-gpu-tests")]
fn load_and_publish_full_tmem_fixture(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
) {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 32;
    const LINE_WORDS: u32 = WIDTH * 2 / 8;
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, WIDTH, 0x200));
    words.extend(set_tile(0, LINE_WORDS, 0));
    words.extend(set_tile_size_words(0, (WIDTH - 1) << 2, (HEIGHT - 1) << 2));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 0), (WIDTH * HEIGHT - 1) << 12]);

    let (planned, source_bytes) = plan_with_deterministic_reads(backend, session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
}

/// Published committed-TMEM textured-draw card §4: the frozen literal
/// texel values (four fully-saturated primary/neutral colors, one per
/// corner), corrected against this crate's own real
/// `LoadBlock`/tile-addressing implementation for the *source image
/// width and byte layout*, rather than copied blind from the card's
/// prose. See this function's own doc below for the one correction.
///
/// RGBA16, 2x2 TILE (the card's own frozen extent). Texel `(0,0)` = red
/// `0xF801` -> RGBA8 `(255,0,0,255)`; `(1,0)` = green `0x07C1` ->
/// `(0,255,0,255)`; `(0,1)` = blue `0x003F` -> `(0,0,255,255)`; `(1,1)`
/// = white `0xFFFF` -> `(255,255,255,255)`.
///
/// **Source-image-width correction (this slice's own verification, not
/// a copy of the card's literal byte string):** `tmem/read.rs`'s
/// `linear_byte_address` computes each row's TMEM start as `row *
/// tile.line_words() * 8` -- always a whole-8-byte-word multiple.
/// `tmem/wire.rs`'s `transfer_shape` for `LoadBlock` transfers exactly
/// `source.total_bytes()` bytes as ONE flat linear run (`dxt=0` mode,
/// no row-interleave), so if the card's own literal 2-texel-wide
/// SOURCE IMAGE were used, row 1 (texels `(0,1)`/`(1,1)`) would land at
/// source/TMEM byte 4 -- not a whole-word multiple, so no `line_words`
/// value can make the READ side find it there (`line_words=1` looks
/// for row 1 at byte 8; `line_words=0` aliases every row to byte 0).
/// This fixture instead uses a 4-texel-wide SOURCE IMAGE (so one row is
/// naturally exactly one 8-byte TMEM word: `4 texels * 2 bytes = 8
/// bytes`), `LoadBlock`s the top 2 rows x 4 columns (8 texels, still
/// one linear `dxt=0` transfer, still admitted), and the 2x2 TILE's own
/// `SetTile`/`SetTileSize` addresses only that image's left 2x2
/// sub-region (columns 0-1, `mask_s`/`mask_t` left at 0 so clamp mode
/// bounds the tile exactly to `high.integer()-low.integer()+1 == 2`).
/// Columns 2-3 of each row are filler, never addressed by this tile.
/// `line_words=1` (one whole word/row) now correctly finds row 1 at
/// byte 8, matching `LoadBlock`'s own real linear placement. The three
/// assertion points' expected colors are computed by literally calling
/// this crate's own `address_texture_cell`/`gather_committed_texture_cell`/
/// `filter_three_nearest_committed_cell` chain against this corrected
/// layout -- not hand-derived arithmetic copied from the card, which is
/// exactly the kind of mismatch this verification step exists to catch.
const FIXTURE_TMEM_WORD_ADDRESS: u16 = 0;

const FIXTURE_LINE_WORDS: u16 = 1;

const FIXTURE_SOURCE_IMAGE_WIDTH: u32 = 4;

/// **Odd-row XOR4 correction (this slice's own verification against
/// the real read path, not assumed from the card's prose):**
/// `LoadBlock` writes its whole transfer as ONE linear run
/// (`tmem/wire.rs`'s `transfer_shape` `Block` arm always reports
/// `row_count = 1`, so its own `odd_row_exchange` never fires) --
/// hardware treats a block load as texel-address-agnostic bytes, not
/// discrete tile rows. But the READ side (`tmem_rgba16_texel_address`/
/// `tmem/read.rs`'s `linear_byte_address`+`odd_row_exchange`) DOES
/// apply the XOR4 swap to any texel whose TILE-relative row is odd,
/// because this fixture's tile has an EVEN T origin (`low_t == 0`, so
/// `TmemFirstRowParity::Even` is the parity the tile itself derives --
/// it is not a frozen constant; `tmem_sample.wgsl`'s
/// `tmem_first_row_parity_odd` and `targets/texrect.rs`'s own
/// derivation both read `low_t.integer() & 1`): row
/// 1 (odd) XORs its computed address by 4. Since the write never
/// exchanged but the read always will for row 1, this fixture's source
/// bytes for row-1 texels must be placed at their POST-XOR4 TMEM
/// offsets directly: texel (0,1) reads from address `8 XOR 4 = 12`,
/// texel (1,1) reads from address `10 XOR 4 = 14`. Bytes 8-11 (row 1's
/// un-exchanged half) are filler, never read by this tile's own two
/// column addresses under the exchange.
fn fixture_load_block_source_bytes() -> Vec<u8> {
    vec![
        0xf8, 0x01, // (0,0) red -- row 0 (even), no exchange
        0x07, 0xc1, // (1,0) green -- row 0 (even), no exchange
        0x00, 0x00, // (2,0) filler, never addressed by the 2x2 tile
        0x00, 0x00, // (3,0) filler
        0x00, 0x00, // byte 8-9: row-1 UN-exchanged half, never read
        0x00, 0x00, // byte 10-11: row-1 UN-exchanged half, never read
        0x00, 0x3f, // byte 12-13: (0,1) blue's real post-XOR4 address
        0xff, 0xff, // byte 14-15: (1,1) white's real post-XOR4 address
    ]
}

fn fixture_tile_descriptor() -> TileDescriptor {
    TileDescriptor::from_wire(
        ImageFormat::Rgba,
        PixelSize::Bits16,
        FIXTURE_LINE_WORDS,
        TmemWordAddress::try_new(FIXTURE_TMEM_WORD_ADDRESS).unwrap(),
        0,
        TileAddressMode::default(),
        0,
        0,
        TileAddressMode::default(),
        0,
        0,
    )
}

fn fixture_tile_size() -> TileSize {
    // S10.2 raw units: `TileCoordinate::integer() = raw >> 2`, so
    // `high - low + 1 == 2` texels wide/tall needs `high.integer() ==
    // 1` (raw `4`) with `low.integer() == 0` (raw `0`).
    TileSize::from_wire(
        TileCoordinate::try_new(0).unwrap(),
        TileCoordinate::try_new(0).unwrap(),
        TileCoordinate::try_new(4).unwrap(),
        TileCoordinate::try_new(4).unwrap(),
    )
}

/// CPU oracle side of the differential: the real
/// `address_texture_cell`/`gather_committed_texture_cell`/
/// `filter_three_nearest_committed_cell` chain, invoked directly with
/// no GPU involved (card §4/§7 requirement).
fn cpu_oracle_sample(physical: &PhysicalTmemState, raw_s: i16, raw_t: i16) -> [u8; 4] {
    cpu_oracle_sample_with_tile(
        physical,
        fixture_tile_descriptor(),
        fixture_tile_size(),
        raw_s,
        raw_t,
    )
}

/// Same CPU oracle chain as `cpu_oracle_sample`, parameterized over the
/// tile descriptor/size -- used by the negative-coordinate repair test
/// below, which needs a wrap-addressed (not clamp-addressed) tile: under
/// this crate's frozen clamp fixture (`fixture_tile_descriptor`'s own
/// `mask_s`/`mask_t == 0`, which forces `clamps = true` unconditionally
/// per `address_axis_texel`), any negative `base_texel` clamps to column/
/// row 0 on BOTH the correct-floor and buggy-truncate paths, and the
/// resulting blended color is provably identical either way (the clamp
/// formula's two branches agree exactly at that boundary) -- so a clamp
/// fixture cannot discriminate floor from truncation for a negative
/// coordinate. A `mask=1` wrap tile (non-clamp, non-mirror) instead
/// addresses each axis by parity (`coordinate & 1`), so a negative
/// `base_texel` of differing parity under floor vs. truncation selects
/// genuinely different corners, not a saturated boundary.
fn cpu_oracle_sample_with_tile(
    physical: &PhysicalTmemState,
    tile: TileDescriptor,
    size: TileSize,
    raw_s: i16,
    raw_t: i16,
) -> [u8; 4] {
    let request = crate::PointSampleRequest::new(
        crate::PointSampleCoordinates::new(
            crate::TextureCoordinateS10_5::from_raw(raw_s),
            crate::TextureCoordinateS10_5::from_raw(raw_t),
        ),
        crate::TmemFirstRowParity::Even,
    );
    let cell = crate::gather_committed_texture_cell(
        physical,
        tile,
        size,
        request,
        crate::TextureLutMode::Disabled,
    )
    .expect("fixture's assertion points stay inside the addressed footprint");
    crate::filter_three_nearest_committed_cell(cell)
}

/// Wrap-mode (`mask=1`, non-clamp, non-mirror) sibling of
/// `fixture_tile_descriptor` over the exact same committed 2x2 RGBA16
/// texel layout -- see `cpu_oracle_sample_with_tile`'s doc for why
/// wrap (not clamp) addressing is required to discriminate floor from
/// truncation at a negative coordinate.
fn fixture_wrap_tile_descriptor() -> TileDescriptor {
    TileDescriptor::from_wire(
        ImageFormat::Rgba,
        PixelSize::Bits16,
        FIXTURE_LINE_WORDS,
        TmemWordAddress::try_new(FIXTURE_TMEM_WORD_ADDRESS).unwrap(),
        0,
        TileAddressMode::from_wire(0), // t: mirror=false, clamp=false
        1,                             // mask_t = 1 (2-texel wrap period)
        0,
        TileAddressMode::from_wire(0), // s: mirror=false, clamp=false
        1,                             // mask_s = 1 (2-texel wrap period)
        0,
    )
}

#[cfg(feature = "host-gpu-tests")]
fn assert_close_rgba8_channels(observed: [u8; 4], expected: [u8; 4], tolerance: i32) {
    for channel in 0..4 {
        let diff = i32::from(observed[channel]) - i32::from(expected[channel]);
        assert!(
            diff.abs() <= tolerance,
            "channel {channel}: observed={observed:?} expected={expected:?} \
             tolerance={tolerance}"
        );
    }
}

/// One flat-shaded, non-textured, non-Z `RawTriangle` covering exactly
/// the left half (`x` in `[0, width/2)`) of an 8x8 target, built from
/// literal `NeutralTriangleVertex` positions (raw RDP screen-pixel
/// space, matching `shaders/triangle_pipeline_vertex.wgsl`'s own
/// module doc) rather than a wire-decoded fixture -- this test only
/// needs two draws with disjoint, independently-checkable pixel
/// coverage, not a real command-stream decode.
// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn half_covering_triangle(left: f32, right: f32, shade: f32) -> RetrievedTriangleDraw {
    RetrievedTriangleDraw {
        vertices: [
            fn64_render::NeutralTriangleVertex {
                x: left,
                y: 0.0,
                z: 0.0,
                w: 1.0,
                color: [shade, shade, shade, 1.0],
                texcoord: [0.0, 0.0],
            },
            fn64_render::NeutralTriangleVertex {
                x: right,
                y: 0.0,
                z: 0.0,
                w: 1.0,
                color: [shade, shade, shade, 1.0],
                texcoord: [0.0, 0.0],
            },
            fn64_render::NeutralTriangleVertex {
                x: (left + right) / 2.0,
                y: 8.0,
                z: 0.0,
                w: 1.0,
                color: [shade, shade, shade, 1.0],
                texcoord: [0.0, 0.0],
            },
        ],
        source: TriangleSource::RawTriangle,
        viewport: None,
        other_mode: OtherMode::from_wire(0, 0),
        // SHADE passthrough: `run_one_cycle` always evaluates the
        // second-cycle bit positions (`color_combiner.wgsl`'s
        // `run_one_cycle` hardcodes `second_cycle = true`), so this
        // uses the same second-cycle color_d/alpha_d=SHADE encoding as
        // `targets::triangle_pipeline::tests::shade_passthrough_combine_params`
        // -- color_a=color_b=0 (COMBINED) makes `(A-B)*C` zero, so
        // `(A-B)*C+D` collapses to D (SHADE), and this triangle's own
        // per-vertex `color` is what reaches the fragment, not the
        // all-zero default `CombineParams::from_wire(0, 0)` would
        // otherwise produce (transparent black everywhere,
        // indistinguishable from an uncovered/cleared pixel).
        combine_params: CombineParams::from_wire(0, (4 << 6) | 4),
        tile_binding: TileBindingParams::unbound(),
        blend_color: Color4::from_wire(0),
        env_color: Color4::from_wire(0),
        prim_color: PrimColor::from_wire(0, 0),
        fog_color: Color4::from_wire(0),
        // A raw-triangle fixture: this path does not read the scissor
        // (only `execute_texture_rectangle` clips against it today),
        // so an unset rect is the honest value rather than a fabricated
        // full-frame one.
        scissor: None,
        prim_depth: None,
    }
}

fn fixture_location(command_index: u32) -> fn64_render::RawDpcCommandLocation {
    fn64_render::RawDpcCommandLocation {
        command_index,
        stream_index: 0,
        chunk_index: 0,
        source_address: fn64_render_ir::PhysicalAddress::try_new(0x1000)
            .expect("fixture address is in-bounds"),
        source_byte_offset: 0,
        source_byte_len: 8,
        wire_opcode: 0x08,
    }
}

fn fixture_vertex(seed: f32) -> fn64_render::NeutralTriangleVertex {
    fn64_render::NeutralTriangleVertex {
        x: seed,
        y: seed + 1.0,
        z: seed + 2.0,
        w: 1.0,
        color: [seed, seed, seed, 1.0],
        texcoord: [0.0, 0.0],
    }
}

fn fixture_triangle(seed: f32) -> RdpTriangleCommand {
    RdpTriangleCommand {
        location: fixture_location(0),
        raw_words: Box::new([]),
        vertices: core::array::from_fn(|index| fixture_vertex(seed + index as f32)),
        source: TriangleSource::RawTriangle,
        viewport: None,
        texrect_accesses: None,
    }
}

fn fixture_set_other_mode(high: u32, low: u32) -> RdpStateCommand {
    RdpStateCommand::SetOtherMode {
        location: fixture_location(0),
        raw_words: Box::new([0, 0]),
        other_mode: fn64_render::NeutralOtherMode { high, low },
        before: None,
        after: fn64_render::RdpStateIdentity::of_other_mode(fn64_render::NeutralOtherMode {
            high,
            low,
        }),
    }
}

fn fixture_set_combine(low: u32, high: u32) -> RdpStateCommand {
    RdpStateCommand::SetCombine {
        location: fixture_location(0),
        raw_words: Box::new([0, 0]),
        combine: fn64_render::NeutralCombineParams { low, high },
        before: None,
        after: fn64_render::RdpStateIdentity::of_combine(fn64_render::NeutralCombineParams {
            low,
            high,
        }),
    }
}

fn fixture_set_env_color(value: u32) -> RdpStateCommand {
    RdpStateCommand::SetEnvColor {
        location: fixture_location(0),
        raw_words: Box::new([0]),
        color: fn64_render::NeutralColor4 { value },
        before: None,
        after: fn64_render::RdpStateIdentity::of_env_color(fn64_render::NeutralColor4 {
            value,
        }),
    }
}

fn fixture_set_prim_color(lod_frac: u8, lod_min: u8, color: u32) -> RdpStateCommand {
    let neutral = fn64_render::NeutralPrimColor {
        lod_frac,
        lod_min,
        color,
    };
    RdpStateCommand::SetPrimColor {
        location: fixture_location(0),
        raw_words: Box::new([0, 0]),
        color: neutral,
        before: None,
        after: fn64_render::RdpStateIdentity::of_prim_color(neutral),
    }
}

fn fixture_set_fog_color(value: u32) -> RdpStateCommand {
    RdpStateCommand::SetFogColor {
        location: fixture_location(0),
        raw_words: Box::new([0]),
        color: fn64_render::NeutralColor4 { value },
        before: None,
        after: fn64_render::RdpStateIdentity::of_fog_color(fn64_render::NeutralColor4 {
            value,
        }),
    }
}

fn fixture_set_scissor(mode: u8, ulx: u16, uly: u16, lrx: u16, lry: u16) -> RdpStateCommand {
    let scissor = fn64_render::NeutralScissor {
        mode,
        upper_left_x: ulx,
        upper_left_y: uly,
        lower_right_x: lrx,
        lower_right_y: lry,
    };
    RdpStateCommand::SetScissor {
        location: fixture_location(0),
        raw_words: Box::new([0]),
        scissor,
        before: None,
        after: fn64_render::RdpStateIdentity::of_scissor(scissor),
    }
}

/// A neutral tile descriptor whose `tmem_word_address` is the caller's,
/// so two tiles seeded into `PlanCollector` are distinguishable in the
/// GPU uniform by a field the uniform actually carries.
fn fixture_neutral_tile(
    tmem_word_address: u16,
) -> (
    fn64_render::NeutralTileDescriptor,
    fn64_render::NeutralTileSize,
) {
    (
        fn64_render::NeutralTileDescriptor {
            format: fn64_render::NeutralImageFormat::Rgba,
            size: fn64_render::NeutralPixelSize::Bits16,
            line_words: 2,
            tmem_word_address,
            palette: 0,
            s_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: false,
            },
            mask_s: 0,
            shift_s: 0,
            t_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: false,
            },
            mask_t: 0,
            shift_t: 0,
        },
        fn64_render::NeutralTileSize {
            low_s: 0,
            low_t: 0,
            high_s: 7 << 2,
            high_t: 7 << 2,
        },
    )
}

/// One raw triangle command carrying real wire words whose word 0 names
/// `tile` in bits 18:16 -- the field `RawTriangle::decode` reads as
/// `((w0 >> 16) & 0x7)` and the CPU executor already honours.
fn fixture_raw_triangle_naming_tile(tile: u32) -> RdpTriangleCommand {
    // Opcode 0x08 (flat, untextured) in bits 29:24 of word 0, the tile
    // index in bits 18:16, everything else zero. Four 64-bit words = 8
    // u32 words for a `triangleBaseWords` command.
    // Bit 19 is the LEVEL field's low bit, deliberately SET here: it
    // is the bit immediately above the 3-bit tile field, so a decode
    // that widens the mask past `0x7` reads it as part of the tile
    // index and lands on a different table entry. Without a set bit
    // there, `& 0x7` and `& 0xf` agree for every tile 0..=7 and a
    // widened-mask mutant survives.
    let word0 = (0x08u32 << 24) | (1 << 19) | (tile << 16);
    RdpTriangleCommand {
        location: fixture_location(0),
        raw_words: Box::new([word0, 0, 0, 0, 0, 0, 0, 0]),
        vertices: core::array::from_fn(|index| fixture_vertex(index as f32)),
        source: TriangleSource::RawTriangle,
        viewport: None,
        texrect_accesses: None,
    }
}

fn fixture_raw_triangle_collector() -> PlanCollector {
    PlanCollector::seeded_from_parts(
        Some(OtherMode::from_wire(0, 0)),
        Some(CombineParams::from_wire(0, 0)),
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    )
}

// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn test_render_config() -> fn64_render::RenderConfig {
    fn64_render::RenderConfig {
        width: 8,
        height: 8,
        tv_type: fn64_runtime::TvType::default(),
    }
}

/// The color-image height every fill fixture in this module configures.
const FILL_TARGET_HEIGHT: u32 = 8;

/// The color-image width every fill fixture in this module stages.
const FILL_TARGET_WIDTH: u32 = 16;

/// The physical address every fill fixture's `SetColorImage` names.
/// Chosen clear of `COMMAND_START` (0x1000) so the target's byte range
/// never overlaps the command stream, and inside `LAYOUT_BYTES`
/// (0x4000) so `plan_fill`'s installed-RDRAM check passes.
const FILL_TARGET_ADDRESS: u32 = 0x2000;

const SET_COLOR_IMAGE: u8 = 0x3f;

const SET_FILL_COLOR: u8 = 0x37;

const FILL_RECTANGLE: u8 = 0x36;

/// Records the host-configured framebuffer extent without requiring a
/// GPU adapter.
///
/// `create_inner` stores `configured_target_extent` *before* it requests
/// a device (see its own comment), precisely so an admitted
/// `FillRectangle` -- a CPU-side executor with no adapter dependency --
/// can execute on an adapterless host. A `NoAdapter` result is therefore
/// expected and ignored here; any *other* create failure still panics,
/// because that would mean the extent was not recorded for the reason
/// this helper assumes.
fn configure_fill_target_height(backend: &mut WgpuBackend) {
    match backend.create_inner(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    }) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(_)) => {
            backend.disable_adapterless_gpu_diagnostic();
        }
        Err(other) => panic!("create_inner failed for an unexpected reason: {other}"),
    }
    assert!(
        backend.configured_target_extent.is_some(),
        "create_inner must record the host-configured extent even with no GPU adapter"
    );
}

/// `SetOtherMode` staging Fill cycle (`cycle_type == 3`) with no
/// Z-compare/Z-update/image-read bit set -- the only `OtherMode`
/// `execute_fill_rectangle` admits (`require_safe_fill_cycle_bypass`).
fn fill_cycle_other_mode(low: u32) -> [u32; 2] {
    [word(SET_OTHER_MODE, 3 << 20), low]
}

/// `SetColorImage` staging an RGBA16 image of `FILL_TARGET_WIDTH` at
/// `FILL_TARGET_ADDRESS`. Wire `format` is 0 (`Rgba`), wire `size` is 2
/// (`Bits16`), and the wire `width` field is width-1 (the decoder adds
/// one back). `FILL_TARGET_ADDRESS` is 64-byte aligned, which
/// `SetColorImage`'s own decode requires.
fn set_color_image_rgba16() -> [u32; 2] {
    [
        word(SET_COLOR_IMAGE, 2 << 19 | (FILL_TARGET_WIDTH - 1)),
        FILL_TARGET_ADDRESS,
    ]
}

fn set_fill_color(value: u32) -> [u32; 2] {
    [word(SET_FILL_COLOR, 0), value]
}

/// One `FillRectangle` at whole-pixel coordinates. The wire fields are
/// 10.2 fixed point, so each coordinate is shifted left by 2.
fn fill_rectangle(x0: u32, y0: u32, x1: u32, y1: u32) -> [u32; 2] {
    [
        word(FILL_RECTANGLE, ((x1 << 2) << 12) | (y1 << 2)),
        ((x0 << 2) << 12) | (y0 << 2),
    ]
}

/// The headline fixture: a partial-width, three-row fill.
///
/// `x0 = 4` is deliberately nonzero, so `plan_fill` takes its per-row
/// branch (`x0 == 0 && x1 + 1 == width` is false) and declares **three**
/// disjoint, width-strided write accesses rather than one collapsed
/// range. 11 pixels wide (x 4..=14) x 3 rows (y 2..=4) in an RGBA16
/// image: 22 bytes per row, 66 bytes total, spanning 22 + 2*32 = 86
/// bytes -- so a single collapsed range would falsely claim 20 untouched
/// inter-row bytes as written.
fn partial_width_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(4, 2, 14, 4));
    words
}

/// Same target and rectangle height, but spanning the image's full
/// width -- so `plan_fill` takes its `planned_rows == 1` branch and
/// declares exactly one contiguous access.
fn full_width_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(0, 2, FILL_TARGET_WIDTH - 1, 4));
    words
}

/// A whole-target fill: every pixel of the 16x8 image.
///
/// Required as the *first* fill against a fresh color target.
/// `CandidateColorTarget::admit_completed_initialization` rejects a
/// partial rectangle on a target with no predecessor
/// (`PartialNewTargetInitialization`), because a brand-new target has no
/// prior device-byte content for the untouched rows and admitting one
/// would publish fabricated zeros as if they were real content. Filling
/// the whole target first establishes generation 1 honestly; a
/// subsequent partial fill then patches into that real buffer.
///
/// This is also the real-world order: a title clears its framebuffer
/// before filling sub-rectangles into it.
fn whole_target_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x0842_1085));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        FILL_TARGET_HEIGHT - 1,
    ));
    words
}

/// Three fills of one target in one packet. The first establishes every
/// byte; the next two patch disjoint rectangles, leaving observable
/// pixels from all three commands in the final resident image.
fn three_fill_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(2, 2, 5, 5));
    words.extend(set_fill_color(0x6319_7bdf));
    words.extend(fill_rectangle(10, 1, 13, 6));
    words
}

/// `finalize_and_submit`, with the declared-read capture built before
/// the plan is moved.
///
/// A free function rather than an inline expression because
/// `capture_declared_reads` borrows the plan and `finalize_and_submit`
/// consumes it, and Rust evaluates arguments left to right -- so the
/// obvious one-liner is a borrow-after-move.
fn finalize_and_submit_pair(
    session: &mut RawDpcAbiSession,
    planned: PlannedRawDpcSubmission,
) -> Result<BoundSubmittedRawDpc, fn64_render_ir::ValidationError> {
    let capture = capture_declared_reads(&planned);
    session.finalize_and_submit(planned, capture)
}

/// A capture that satisfies every guest read the plan declared, with
/// deterministic bytes keyed by access index.
///
/// Fixtures used to pass an empty capture, which was correct while the
/// only declared reads were TMEM loads that fill fixtures never issue.
/// A partial `FillRectangle` now also declares one -- its colour-image
/// seed -- so an empty capture fails `GuestReadCountMismatch` rather
/// than testing anything.
///
/// The bytes are a per-access constant rather than zeros, deliberately:
/// zero is the fabricated value the seed exists to displace, so a
/// fixture seeded with zeros could not tell a working seed from a
/// missing one.
fn capture_declared_reads(planned: &PlannedRawDpcSubmission) -> DeferredGuestReadCapture {
    DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                let fill = (read.access_index() as u8).wrapping_mul(17).wrapping_add(3);
                CapturedGuestRead::try_new(*read, vec![fill; read.range().len() as usize])
                    .expect("a capture sized to its own declared read is well formed")
            })
            .collect(),
    )
}

/// Runs one fill capture all the way through plan -> execute -> commit
/// -> seal -> publish, returning the staged writes it committed.
fn publish_one_fill(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> Vec<CompletedWrite> {
    publish_one_fill_with_submission(backend, session, words).0
}

fn publish_one_fill_with_submission(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> (Vec<CompletedWrite>, fn64_render_ir::SubmissionIdentity) {
    let request = session.plan_request(capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("fixture plans cleanly");
    let bound = finalize_and_submit_pair(session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture executes cleanly");
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    (staged, submission)
}

fn planned_submission_identity(words: Vec<u32>) -> fn64_render_ir::SubmissionIdentity {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let request = session.plan_request(capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("identity fixture plans cleanly");
    finalize_and_submit_pair(&mut session, planned)
        .expect("identity fixture submits cleanly")
        .submission()
}

// False positive (dead_code): only called from
// #[cfg(feature = "host-gpu-tests")] tests, invisible to a default
// check/test run.
#[cfg_attr(not(feature = "host-gpu-tests"), allow(dead_code))]
fn whole_target_cpu_triangle_words() -> Vec<u32> {
    let (combine_low, combine_high) =
        crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_PRIMITIVE);
    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(combine_low, combine_high));
    words.extend(set_prim_color(0, 0, TRIANGLE_PRIM_WIRE));
    words.extend(set_color_image_rgba16());
    words.extend(
        crate::wire_words::EdgeWords {
            lft: true,
            yl: crate::wire_words::line(8),
            ym: crate::wire_words::line(8),
            yh: 0,
            xl: crate::wire_words::px(FILL_TARGET_WIDTH as i32),
            xh: crate::wire_words::px(0),
            xm: crate::wire_words::px(FILL_TARGET_WIDTH as i32),
            ..crate::wire_words::EdgeWords::zeroed()
        }
        .words(0, RAW_TRIANGLE_BASE_EDGE),
    );
    words
}

/// The ordered `RenderTarget` write accesses one word stream's decode
/// declares in its own resource journal.
///
/// Read from `crate::decode_raw_dpc`'s resource plan -- the same list
/// `plan_fill` pushed and the same list `ExactRawDpcPlanWriter::finish`
/// proves the sealed plan equals one for one. `PlannedRawDpcSubmission`
/// exposes no journal accessor of its own, so this re-decodes the same
/// capture rather than reaching into a sealed value.
fn declared_render_target_writes(words: Vec<u32>) -> Vec<(u32, u32)> {
    let capture = capture(words);
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
    let decoded = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission.clone(),
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        probe_journal,
    )
    .unwrap();
    let ticket = submit_locally(decoded).unwrap();
    let accesses = match crate::decode_raw_dpc(ticket, &RdpState::default()) {
        Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
        Ok(decoded) => decoded.resource_plan().accesses().to_vec(),
        Err(error) => panic!("probe decode must report the real access list, got {error:?}"),
    };
    accesses
        .iter()
        .filter(|access| {
            access.mode() == AccessMode::Write
                && access.purpose() == AccessPurpose::RenderTarget
        })
        .map(|access| match access.region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                (range.start().get(), range.len())
            }
            other => panic!("a fill access is always an RDRAM region, got {other:?}"),
        })
        .collect()
}

/// Drives plan -> execute for a fill fixture, which declares zero
/// `TmemLoadSource` reads.
fn plan_and_execute_fill(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> (
    fn64_render_ir::SubmissionIdentity,
    Result<BackendPreparedRawDpc, RenderError>,
) {
    let planned = plan_with_no_reads(backend, session, words);
    let bound = finalize_and_submit_pair(session, planned).unwrap();
    let submission = bound.submission();
    (submission, backend.execute_raw_dpc(bound))
}

/// A `FillRectangle` followed by a `RawTriangle` in one packet: the
/// ordinary N64 idiom of clearing a framebuffer and then drawing into
/// it. Both halves plan cleanly on their own, so nothing upstream
/// refuses this; the refusal has to live at execution.
///
/// `set_combine` is required before the triangle -- `PlanCollector`
/// rejects a triangle visited with no combiner state established
/// (see `plan_collector_rejects_a_triangle_visited_with_no_state_
/// established_at_all`). `set_other_mode` is deliberately NOT re-issued
/// after the fill: reverting to a non-Fill cycle would be a second,
/// unrelated reason for the packet to be interesting, and the fill's
/// own Fill-cycle `OtherMode` is what `plan_fill` admitted against.
fn fill_then_triangle_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(set_combine(0, 0));
    words.extend(triangle_base_edge_words(7, 2, 0));
    words
}

#[cfg(feature = "host-gpu-tests")]
mod host_gpu_tests {
    use super::*;

    /// Required host evidence: a real adapter request succeeds and
    /// `WgpuBackend::create` stores a real `TrianglePipelineRenderer`,
    /// specifically a Metal adapter (asserted below via
    /// `adapter_info().backend`, not merely "some adapter, whatever
    /// it is" -- `host-gpu-tests` is this crate's real-Metal
    /// qualification gate). `create_inner` (not `create`) is called
    /// directly so a `NoAdapter` outcome is distinguishable, by type,
    /// from any other failure -- not to make it non-panicking: a
    /// `NoAdapter` here is still a loud, named panic (`required host
    /// GPU evidence unavailable`), matching this crate's own existing
    /// convention for required host-GPU test evidence
    /// (`device/mod.rs`'s `host_gpu_tests` module panics identically
    /// on its own `HeadlessDeviceOutcome::NoAdapter`). The value of
    /// the typed `WgpuCreateError` here is that this panic message
    /// names exactly which failure occurred, instead of an opaque
    /// `RenderError::Backend` string a caller would have to parse.
    #[test]
    fn create_requests_a_real_metal_adapter_and_stores_the_triangle_pipeline() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        match backend.create_inner(&test_render_config()) {
            Ok(()) => {
                let renderer = backend
                    .triangle_pipeline
                    .as_ref()
                    .expect("a successful create() must store a real TrianglePipelineRenderer");
                assert_eq!(
                    renderer.adapter_info().backend,
                    wgpu::Backend::Metal,
                    "this test qualifies real Metal execution specifically, not merely \
                     some adapter -- got {:?}",
                    renderer.adapter_info()
                );
            }
            Err(WgpuCreateError::NoAdapter(no_adapter)) => {
                panic!(
                    "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
                );
            }
            Err(other) => panic!("create() failed for an unexpected reason: {other}"),
        }
    }

    /// Repeated `create()` calls are an explicit full reset (card
    /// §1a): re-requesting a device from scratch must succeed again,
    /// not error as "already initialized" and not silently no-op.
    #[test]
    fn repeated_create_calls_reset_the_triangle_pipeline_each_time() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("first create() must succeed on a real adapter");
        assert!(backend.triangle_pipeline.is_some());
        backend
            .create_inner(&test_render_config())
            .expect("a second create() call must also succeed, not error or no-op");
        assert!(backend.triangle_pipeline.is_some());
    }
}

// -----------------------------------------------------------------
// Composed fill + TMEM in one packet.
//
// Census `docs/RT64-WM2000-CENSUS.md` §4a measures the former
// `MixedFillAndTmemLoadPacket` refusal firing on 218/218 WM2000 frames.
// These tests are the unit-level evidence for the composition that
// replaced it; `fn64-abi`'s `raw_dpc_session_integration` carries the
// end-to-end half through the real producer seam.
// -----------------------------------------------------------------

/// A composed packet: the TMEM load first, then the whole-target fill.
/// Both halves are the existing single-source fixtures verbatim, so a
/// composed packet's halves are provably the same commands the
/// single-source tests already pin.
fn tmem_then_fill_words() -> Vec<u32> {
    let mut words = one_load_block_words();
    words.extend(whole_target_fill_words());
    words
}

/// The same two halves, swapped: the fill first, then the TMEM load.
fn fill_then_tmem_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(one_load_block_words());
    words
}

/// Every write access one word stream's decode declares, as
/// `(operation_id, purpose)` in the resource journal's own order.
///
/// Reuses `declared_render_target_writes`'s probe-decode technique --
/// `PlannedRawDpcSubmission` exposes no journal accessor -- but keeps
/// the purpose tag rather than filtering to `RenderTarget`, because the
/// interleaving of the two purposes IS the fact under test.
/// Every `RenderTarget` write access one word stream's decode declares,
/// as `(start, end)` guest byte ranges in the journal's own order.
///
/// Same probe-decode technique as `declared_write_purposes`
/// (`PlannedRawDpcSubmission` exposes no journal accessor), but keeps the
/// RDRAM *range* rather than the purpose tag: a count alone cannot tell a
/// correctly-placed rectangle from one shifted by a row, which is exactly
/// the mutation that survived a count-only assertion.
fn declared_render_target_ranges(words: Vec<u32>) -> Vec<(u32, u32)> {
    let capture = capture(words);
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
    let decoded = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission.clone(),
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        probe_journal,
    )
    .unwrap();
    let ticket = submit_locally(decoded).unwrap();
    let accesses = match crate::decode_raw_dpc(ticket, &RdpState::default()) {
        Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
        Ok(decoded) => decoded.resource_plan().accesses().to_vec(),
        Err(error) => panic!("probe decode must report the real access list, got {error:?}"),
    };
    accesses
        .iter()
        .filter(|access| {
            access.mode() == AccessMode::Write
                && access.purpose() == AccessPurpose::RenderTarget
        })
        .filter_map(|access| match access.region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                Some((range.start().get(), range.end()))
            }
            _ => None,
        })
        .collect()
}

fn declared_write_purposes(words: Vec<u32>) -> Vec<(u32, AccessPurpose)> {
    let capture = capture(words);
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
    let decoded = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission.clone(),
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        probe_journal,
    )
    .unwrap();
    let ticket = submit_locally(decoded).unwrap();
    let accesses = match crate::decode_raw_dpc(ticket, &RdpState::default()) {
        Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
        Ok(decoded) => decoded.resource_plan().accesses().to_vec(),
        Err(error) => panic!("probe decode must report the real access list, got {error:?}"),
    };
    accesses
        .iter()
        .filter(|access| access.mode() == AccessMode::Write)
        .map(|access| (access.operation().get(), access.purpose()))
        .collect()
}

/// Drives plan -> execute for a composed fixture, supplying the one
/// `TmemLoadSource` read its TMEM half declares.
fn plan_and_execute_composed(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> (
    fn64_render_ir::SubmissionIdentity,
    Result<BackendPreparedRawDpc, RenderError>,
) {
    let (planned, source_bytes) = plan_with_deterministic_reads(backend, session, words);
    let capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let submission = bound.submission();
    (submission, backend.execute_raw_dpc(bound))
}

/// Drives plan -> execute -> guest commit -> publish for a composed
/// fixture, the full conveyor `publish_one_fill` drives for a fill-only
/// one, with the TMEM half's declared read supplied.
///
/// Publication matters here and cannot be skipped: `physical_tmem()`
/// reads the coordinator's *active* slot, and `complete_execution`
/// installs its successor into the *inactive* one. Only `commit` flips
/// them. So the TMEM half's effect is unobservable until publish, which
/// is exactly why the composed test drives all the way through.
fn publish_composed(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> Vec<CompletedWrite> {
    let (planned, source_bytes) = plan_with_deterministic_reads(backend, session, words);
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("a composed fill+TMEM packet must execute");
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    staged
}

// --- TextureRectangle composition frontier (this card's measurement) ---

/// One `TextureRectangle` command's 4-word wire payload, mirroring
/// `raw_dpc::production_adapter::tests::texrect_words` exactly (that
/// helper is private to its own module's tests, so this is a local,
/// identical copy -- the same convention `triangle_base_edge_words`
/// above already follows).
///
/// Deliberately sized to land inside the 16x8 `FILL_TARGET_*` image
/// this module's fill fixtures use, rather than reusing the sibling's
/// 48x48 rectangle: a rectangle larger than the target would confound
/// "declares no write" with "declares a write outside the target".
fn texrect_words_in_target(tile: u32) -> [u32; 4] {
    // 10.2 fixed point, matching `fill_rectangle` above: x 4..=11,
    // y 2..=4, wholly inside the 16x8 RGBA16 target.
    let ulx: u32 = 4 << 2;
    let uly: u32 = 2 << 2;
    let lrx: u32 = 11 << 2;
    let lry: u32 = 4 << 2;
    let dsdx: u32 = 0x0100;
    let dtdy: u32 = 0x0100;
    [
        word(0x24, (lrx << 12) | lry),
        (tile & 0x7) << 24 | (ulx << 12) | uly,
        0,
        (dsdx << 16) | dtdy,
    ]
}

/// `texrect_words_in_target`'s stepping sibling: identical rectangle,
/// but `dsdx`/`dtdy` of `0x0400` (one texel per pixel in S5.10) instead
/// of `0x0100`.
///
/// The step matters and was determined by measurement. Copy mode
/// halves the S step twice (`dsdx >>= 2`), so
/// `lrs = (0 + 0x100 * (8 << 2)) >> 7 = 64` in S10.5 -- **2 texels
/// across the 8-pixel row**. `dtdy` is not shifted, so
/// `lrt = (0 + 0x400 * (3 << 2)) >> 7 = 96` -- **3 texels over the 3
/// rows**, one per row. At `0x0100` the S span is half a texel and
/// every pixel in a row samples the same texel, which makes an
/// "S is actually read" assertion unsatisfiable; the sibling keeps
/// `0x0100` because its own tests never sample.
fn texrect_words_in_target_stepping(tile: u32) -> [u32; 4] {
    let mut words = texrect_words_in_target(tile);
    words[3] = (0x0400u32 << 16) | 0x0400;
    words
}

/// A TMEM load, then a `TextureRectangle` sampling the tile it loaded --
/// the WM2000-title-screen shape this card was dispatched to admit.
fn tmem_then_texrect_words() -> Vec<u32> {
    let mut words = one_load_block_words();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_in_target(7));
    words
}

/// The composed shape: whole-target fill, a TMEM load, then a
/// `TextureRectangle` sampling it.
fn fill_tmem_and_texrect_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(one_load_block_words());
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_in_target(7));
    words
}

/// The number of `TriangleSource::TextureRectangle` triangles this
/// stream admits, measured through the same plan walk execution uses --
/// not re-derived from the wire words, which would be a second
/// independent model of the same fact.
fn admitted_texture_rectangle_triangles(words: Vec<u32>) -> usize {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &mut session, words);
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();

    let mut plan_visitor = PlanCollector::seeded_from_parts(
        None,
        None,
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        plan: PlanCollector::seeded_from_parts(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            None,
            [(None, None); 8],
        ),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        physical: coordinator.physical(),
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    view.plan
        .triangles
        .iter()
        .filter(|planned| {
            planned
                .draw
                .as_ref()
                .map(|draw| draw.source == TriangleSource::TextureRectangle)
                .unwrap_or(false)
        })
        .count()
}

/// The WM2000-title-screen-shaped stream this card exists to admit: a
/// whole-target `FillRectangle` in Fill cycle, a `LoadBlock` filling
/// tile 7, then a `TextureRectangle` in **Copy** cycle sampling that
/// tile.
///
/// The cycle-type switch between the two halves is not incidental: a
/// fill is only admitted in Fill cycle and this texrect executor is
/// only admitted in Copy cycle (it evaluates no color combiner), so a
/// real stream must set each. `fill_tmem_and_texrect_words` above keeps
/// its single `set_other_mode(0, 0)` and is used only by the
/// declared-write and admission tests, which never execute.
fn fill_load_and_copy_texrect_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    // A wider, UNSKEWED LoadBlock than `one_load_block_words`': 24
    // texels (`uls=0, lrs=23`) with `dxt = 0`, so TMEM bytes 0..48 are
    // contiguously valid -- three complete rows at this tile's
    // `line_words = 2` (16 bytes = 8 RGBA16 texels per row).
    //
    // Both changes were forced by measurement. `one_load_block_words`'
    // 8-texel load fills row 0 only, and `dxt = 0x800` skews so hard
    // that its 8 texels land in bytes 0..8 and 24..32 with a hole
    // between -- a texrect whose T advances one texel per row hits the
    // hole and is refused as `physical TMEM texel byte 0x014 is
    // invalid`. That refusal is how these values were determined.
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
    // High S/T of 7 texels, in the 10.2 wire encoding (`<< 2`); low
    // S/T are the field's own zero. Mirrors
    // `raw_dpc::production_adapter::tests::set_tile_size` exactly (that
    // helper is private to its own module's tests), same local-copy
    // convention as `set_other_mode` above.
    words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    // Copy cycle (2), so the texrect executor admits it.
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_in_target_stepping(7));
    words
}

/// **WM2000's measured mixed shape: texrects and a raw triangle in one
/// packet, the triangle strictly last.**
///
/// Modelled on the packet the all-Rust lane actually aborted on
/// (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`, instrumented at the refusal
/// site): **texrects, TMEM loads, one raw triangle at the END, zero
/// fills**. The real packet carried 6 texrects and 9 loads; the shape
/// that matters is the *pairing*, so this fixture carries one of each
/// plus the trailing raw triangle. A fill is deliberately absent -- the
/// real packet had none, and adding one would instead exercise the
/// separate `MixedFillAndTrianglePacket` refusal, which is kept.
///
/// Built by taking `fill_load_and_copy_texrect_words`' load-and-texrect
/// half verbatim (its `SetColorImage` supplied by
/// `set_color_image_rgba16` instead of a whole-target fill, so the
/// texrect still declares its journal write) and appending one
/// `RawTriangle`. The trailing `set_other_mode(0, 0)` is not decoration:
/// the texrect ran in Copy cycle (2) and a raw triangle is not admitted
/// there, so a real stream switches back exactly as this one does.
fn load_texrect_and_trailing_raw_triangle_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_color_image_rgba16());
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
    words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_in_target_stepping(7));
    // Back out of Copy cycle for the raw triangle, then the triangle
    // itself -- the last command in the packet, exactly as measured.
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words.extend(triangle_base_edge_words(7, 2, 0));
    words
}

/// **The admission this card exists for.** A packet carrying both an
/// admitted `TextureRectangle` and an admitted `RawTriangle` executes,
/// and the texrect's guest-visible pixels survive.
///
/// This packet was `MixedTexrectAndRawTrianglePacket` -- refused on the
/// reasoning that "the two have no defined ordering". Measuring the
/// packet showed the ordering was never missing: the raw triangle
/// contributes no `ResourceAccess` and no staged `CompletedWrite`, so
/// the journal it must be ordered against is the one the texrect alone
/// produces, and `stage_color_commands` already derives that order from
/// the decoder's own `command_index`.
///
/// The load-bearing assertion is the second one: the texrect's declared
/// write is present in the staged writes, so admitting the triangle did
/// not cost the packet its guest-visible half. The refusal did exactly
/// that -- it dropped six real rectangles to withhold one triangle that
/// reaches only `triangle_draw_output`, which `present` refuses to scan
/// out and nothing copies into RDRAM.
/// The flat (opcode 0x08) triangle this card's end-to-end tests draw:
/// vertical edges at x = 2 and x = 6, scanlines 0..3, `lft` set.
///
/// Hand-derived footprint against the 16x8 RGBA16 fill target at
/// `FILL_TARGET_ADDRESS`:
///   yh = 0, yl = 3<<2 = 12 (S11.2) -> rows 0, 1, 2
///   left edge  x = 2.0  -> x0 = ceil(2 - 7/8)  = 2
///   right edge x = 6.0  -> x1 = ceil(6 - 1/8)  = 6
/// So each row writes pixels 2..6 = 4 pixels = 8 bytes, at
/// 0x2000 + (16y + 2)*2 = 0x2004, 0x2024, 0x2044.
fn flat_triangle_in_target_words() -> [u32; 8] {
    crate::wire_words::EdgeWords {
        lft: true,
        yl: crate::wire_words::line(3),
        ym: crate::wire_words::line(3),
        yh: 0,
        xl: crate::wire_words::px(6),
        xh: crate::wire_words::px(2),
        xm: crate::wire_words::px(6),
        ..crate::wire_words::EdgeWords::zeroed()
    }
    .words(0, RAW_TRIANGLE_BASE_EDGE)
}

/// The primitive colour every flat-triangle end-to-end test writes, and
/// its RGBA16 encoding, both derived by hand and from nothing else.
///
///   PRIM = 0x80FF4080 -> R 0x80, G 0xFF, B 0x40, A 0x80
///   RGBA16 5/5/5/1 = (0x80>>3 << 11) | (0xFF>>3 << 6) | (0x40>>3 << 1)
///                    | 1
///                  = 0x8000 | 0x07C0 | 0x0010 | 1 = 0x87D1
const TRIANGLE_PRIM_WIRE: u32 = 0x80FF_4080;

const TRIANGLE_PRIM_RGBA16: u16 = 0x87D1;

/// A packet staging one-cycle mode, the flat
/// `(Zero - Zero) * Zero + Primitive` combiner program, a primitive
/// colour, the RGBA16 colour image, then one flat raw triangle.
///
/// The combiner program's wire words are packed from the same field
/// layout `targets::raw_triangle::tests` derives: color A/B/C/D =
/// 8/8/16/3 and alpha A/B/C/D = 7/7/7/3 in the SECOND bitfield slice,
///   low  = (A << 5) | C
///   high = (B << 24) | (D << 6) | (aA << 21) | (aB << 3) | (aC << 18)
///          | aD
fn flat_triangle_packet_words() -> Vec<u32> {
    let (low, high) =
        crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_PRIMITIVE);
    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(low, high));
    words.extend(set_prim_color(0, 0, TRIANGLE_PRIM_WIRE));
    words.extend(set_color_image_rgba16());
    words.extend(flat_triangle_in_target_words());
    words
}

/// A ROM-independent instance of the exact program admitted by the first
/// task-scoped compute kernel. The state words are the census-selected
/// program key; the shaded+textured wire triangle and published fixture
/// TMEM force this test through the same typed texture dependency path as
/// the live WM2000 packets without embedding any game bytes.
fn textured_triangle_packet_words(
    combine_low: u32,
    combine_high: u32,
    other_mode_high: u32,
    other_mode_low: u32,
) -> Vec<u32> {
    use crate::rdp_harness::Tri;

    let mut words = Vec::new();
    words.extend(crate::wire_words::set_other_mode_bits(
        0,
        other_mode_high,
        other_mode_low,
    ));
    words.extend(set_combine(combine_low, combine_high));
    words.extend(set_tile(
        0,
        FIXTURE_LINE_WORDS as u32,
        FIXTURE_TMEM_WORD_ADDRESS as u32,
    ));
    words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
    words.extend(set_color_image_rgba16());
    words.extend(
        Tri::flat()
            .left_major()
            .edges(2.0, 6.0)
            .rows(0..3)
            .shade(
                [0x0080_0000, 0x00ff_0000, 0x0040_0000, 0x00ff_0000],
                [0; 4],
                [0; 4],
                [0; 4],
            )
            .texture_planes(
                // With W=2^20, perspective conversion is
                // `(S/W)*2^15`; S=T=2^9 therefore lands at raw S10.5
                // coordinate 16, the centre of fixture texel (0,0).
                [1 << 9, 1 << 9, 1 << 20, 0],
                [0; 4],
                [0; 4],
                [0; 4],
            )
            .words(),
    );
    words
}

#[cfg(feature = "host-gpu-tests")]
fn hot_textured_triangle_packet_words() -> Vec<u32> {
    use crate::targets::{
        HOT_COMBINE_HIGH, HOT_COMBINE_LOW, HOT_OTHER_MODE_HIGH, HOT_OTHER_MODE_LOW,
    };

    textured_triangle_packet_words(
        HOT_COMBINE_LOW,
        HOT_COMBINE_HIGH,
        HOT_OTHER_MODE_HIGH,
        HOT_OTHER_MODE_LOW,
    )
}

/// The `SET_FILL_COLOR` word `whole_target_fill_words` stages.
///
/// Named here rather than repeated as a literal so the fill-half
/// expectation and the fixture cannot drift apart.
const COMPOSED_FILL_COLOR: u32 = 0x0842_1085;

/// The RGBA16 halfword a fill of `fill_color` writes at column `x`.
///
/// The RDP's fill cycle writes the 32-bit fill color as two halfwords
/// per 32-bit word, so an RGBA16 target takes the HIGH halfword on even
/// columns and the LOW halfword on odd ones. Mirrors
/// `fn64-abi`'s `raw_dpc_session_integration::expected_fill_halfword`
/// exactly (that helper is private to its own test module, so this is a
/// local, identical copy -- the same convention `set_other_mode` above
/// already follows). Fill mode writes that register value while the RDP
/// arithmetic pipeline is largely unused (Programming Manual Chapter 12
/// "Fill Mode" and §12.8.2 "Fill Color"), so bit 0 remains verbatim.
fn expected_fill_halfword(fill_color: u32, x: u32) -> u16 {
    if x % 2 == 0 {
        (fill_color >> 16) as u16
    } else {
        fill_color as u16
    }
}

/// The typed tile the composed fixture's texrect samples through,
/// rebuilt from the SAME wire fields `set_tile`/`set_tile_size_words`
/// wrote.
///
/// Deliberately constructed from the fixture's own literals rather than
/// read back out of the plan: an oracle built from the code under
/// test's own state snapshot would agree with it by construction. The
/// fields are `set_tile(7, 2, 0)` -- RGBA (format 0), Bits16 (size code
/// 2), 2 line words, TMEM word 0, palette 0, both address modes clear
/// (wrap), masks and shifts zero -- and
/// `set_tile_size_words(7, 7 << 2, 7 << 2)` -- low S/T zero, high S/T
/// 7 texels in 10.2.
fn composed_fixture_tile() -> crate::TexrectTileBinding {
    crate::TexrectTileBinding::try_from_neutral(
        fn64_render::NeutralTileDescriptor {
            format: fn64_render::NeutralImageFormat::Rgba,
            size: fn64_render::NeutralPixelSize::Bits16,
            line_words: 2,
            tmem_word_address: 0,
            palette: 0,
            s_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: false,
            },
            mask_s: 0,
            shift_s: 0,
            t_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: false,
            },
            mask_t: 0,
            shift_t: 0,
        },
        fn64_render::NeutralTileSize {
            low_s: 0,
            low_t: 0,
            high_s: 7 << 2,
            high_t: 7 << 2,
        },
    )
    .expect("the fixture's tile fields are all inside their public field widths")
}

/// The composed fixture's texrect draw, rebuilt from RT64's own
/// `texture_rectangle_vertices` on the fixture's raw wire words.
///
/// This is the oracle's S/T stepping source. It goes through
/// `texture_rectangle_vertices` -- the same ported geometry the decoder
/// and the executor both use -- because the alternative is a third
/// independent model of copy-mode `dsdx >>= 2` and `lrx |= 3`, whose
/// disagreements would be its own bugs rather than findings. What the
/// oracle keeps independent is the TMEM image it reads (committed, not
/// pending) and the reader entry point (`sample_committed_point`, not
/// `sample_point` over a post-image).
fn composed_fixture_draw() -> crate::TexrectDraw {
    let words = texrect_words_in_target_stepping(7);
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
    let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
        .expect("the fixture's texrect words decode");
    let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::Copy)
        .expect("the fixture's rectangle is non-empty in copy cycle");
    crate::TexrectDraw::try_from_viewport_and_texcoords(
        vertices.viewport,
        // Vertex 0 is `(u1, v1)` and vertex **3** is `(u2, v2)` -- the
        // two opposite corners in `texture_rectangle_vertices`' own
        // six-vertex texcoord order. Vertex 5 is `(u1, v2)`, the
        // lower-LEFT corner, and using it collapses the S span to zero.
        vertices.vertex(0).texcoord(),
        vertices.vertex(3).texcoord(),
    )
    .expect("the fixture's texcoords recover integer S10.5 endpoints")
}

/// The rectangle this fixture's texrect covers, derived **twice** and
/// reconciled, in Copy cycle.
///
/// Derivation 1, RT64's own path (`texture_rectangle_vertices`): the
/// wire fields are `ulx=4<<2=16, uly=2<<2=8, lrx=11<<2=44, lry=4<<2=16`.
/// Copy cycle applies `lrx |= 3` and `lry |= 3` -> `47, 19`, then
/// fill/copy UL round-down `ulx &= !3` / `uly &= !3` leaves `16, 8`
/// unchanged (both already multiples of 4). `FixedRect::left/top/right/
/// bottom(ceil=true)` is `(coord + 3) >> 2` on all four: `(16+3)>>2=4`,
/// `(8+3)>>2=2`, `(47+3)>>2=12`, `(19+3)>>2=5`. Half-open, so the
/// covered pixels are **x 4..=11, y 2..=4** -- 8 wide, 3 tall.
///
/// Derivation 2, independent: `ceil(coord / 4)` on the four
/// copy-mutated values `16, 8, 47, 19` gives `4, 2, 12, 5`. Same.
///
/// The naive reading of the wire corners -- "x 4..=11, y 2..=4 because
/// the fields say 4 and 11 and 2 and 4" -- happens to give the same
/// x-range here by coincidence and the WRONG y-range (it would give 3
/// rows only if you also guessed the copy-mode `|= 3`). Under
/// **one-cycle** the identical wire words give 7x2, not 8x3, which is
/// why the extent must come from the ported geometry and not the wire
/// fields: the same command means different footprints in different
/// cycle types.
const TEXRECT_X0: u32 = 4;

const TEXRECT_Y0: u32 = 2;

const TEXRECT_WIDTH: u32 = 8;

const TEXRECT_HEIGHT: u32 = 3;

// --- N fills and N texrects in one packet (the multiplicity card) ---

/// A `TextureRectangle` at an arbitrary whole-pixel rectangle, sampling
/// `tile` with `texrect_words_in_target_stepping`'s one-texel-per-pixel
/// step.
///
/// The parameterized sibling of `texrect_words_in_target_stepping`,
/// which is fixed at one rectangle. Needed because the whole point of
/// this card is several texrects at *different* rectangles in one
/// packet, and a fixture that could only produce one rectangle could
/// not express an overlap.
fn texrect_words_at(tile: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> [u32; 4] {
    [
        word(0x24, ((x1 << 2) << 12) | (y1 << 2)),
        (tile & 0x7) << 24 | ((x0 << 2) << 12) | (y0 << 2),
        0,
        (0x0400u32 << 16) | 0x0400,
    ]
}

/// The `SetTextureImage`/`SetTile`/`SetTileSize`/`LoadSync`/`LoadBlock`
/// run `fill_load_and_copy_texrect_words` uses, factored out so a
/// multi-command fixture stages TMEM exactly once and every texrect in
/// it samples the same loaded tile.
///
/// One load, not one per texrect: the pending post-image is sealed once
/// per packet from every load in it, so N loads would be composed into
/// one image anyway and would only obscure which texels a texrect read.
fn composed_tmem_load_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, 8, 0x200));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
    words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
    words
}

// --- The WM2000 sprite strip: N loads and N texrects in strict
// --- alternation, every load writing the SAME TMEM range.
//
// Measured on the real ROM through the all-Rust stack, WM2000's sixth
// gfx packet is one TLUT load followed by seven `LoadTile`/texrect
// pairs whose seven loads all write TMEM from word 0, overwriting each
// other. That is the shape a once-per-packet post-image gets maximally
// wrong -- every texrect would draw the LAST load's texels -- so it is
// the shape the fixtures below reproduce.

/// How many load+texrect pairs the sprite-strip fixture stages. Seven,
/// the count WM2000's own sixth packet carries.
const SPRITE_STRIP_PAIRS: usize = 7;

const SPRITE_STRIP_Y0: u32 = 2;

const SPRITE_STRIP_Y1: u32 = 3;

/// The inclusive wire width of each sprite in pixels - 1. Narrow enough
/// that seven of them fit side by side across the 16-pixel target with
/// no overlap, so each texrect's pixels are attributable to exactly one
/// pair rather than to whichever pair painted last.
const SPRITE_STRIP_SPAN: u32 = 1;

/// The x origin of sprite `index`. Disjoint by construction: pair `i`
/// owns columns `2i..=2i+1` and no other pair touches them.
fn sprite_strip_x0(index: usize) -> u32 {
    index as u32 * (SPRITE_STRIP_SPAN + 1)
}

/// **The sprite strip: `SPRITE_STRIP_PAIRS` `LoadBlock`/texrect pairs in
/// strict alternation, every load writing TMEM from word 0.**
///
/// Each load reads a DIFFERENT guest address, so
/// `plan_with_deterministic_reads_for_every_load` gives each one
/// distinguishable source bytes and the seven post-images genuinely
/// differ. Each texrect draws at a disjoint x range, so which load's
/// texels reached which pixels is readable off the published buffer
/// without disentangling overlaps.
///
/// Opened with a whole-target fill because a fresh color target admits
/// nothing else (`PartialNewTargetInitialization`); every later command
/// patches into the buffer that fill established.
fn sprite_strip_words(pairs: usize) -> Vec<u32> {
    let mut words = whole_target_fill_words();
    for index in 0..pairs {
        // A different source address per load, so the loads' contents
        // differ. Byte-aligned well clear of the fill's own target.
        words.extend(set_texture_image(0, 2, 8, 0x200 + (index as u32) * 0x100));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        // Same TMEM destination every time -- tile 7 is bound at TMEM
        // word 0 by the `set_tile` above, and 24 texels at dxt=0 fill
        // bytes 0..48. Load i+1 overwrites load i exactly.
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        // Copy cycle (2), the mode the texrect executor admits here.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        let x0 = sprite_strip_x0(index);
        words.extend(texrect_words_in_target_stepping_at(
            7,
            x0,
            SPRITE_STRIP_Y0,
            x0 + SPRITE_STRIP_SPAN,
            SPRITE_STRIP_Y1,
        ));
    }
    words
}

/// `texrect_words_at` with the unit S/T step
/// `texrect_words_in_target_stepping` uses, so the texrect walks one
/// texel per pixel instead of holding texel (0,0).
fn texrect_words_in_target_stepping_at(
    tile: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> [u32; 4] {
    let mut words = texrect_words_at(tile, x0, y0, x1, y1);
    words[3] = (0x0400u32 << 16) | 0x0400;
    words
}

/// Drives the sprite strip all the way through publication with
/// per-load source bytes, and returns the published color-target
/// buffer.
fn publish_sprite_strip(pairs: usize) -> Vec<u8> {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let words = sprite_strip_words(pairs);
    let (planned, per_read_bytes) =
        plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
    assert_eq!(
        per_read_bytes.len(),
        pairs,
        "the sprite strip must declare exactly one TmemLoadSource read per load, or the \
         per-load source bytes below name the wrong loads"
    );
    let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("the sprite strip must execute");
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged)
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    published_target_bytes(&backend)
}

/// Sprite `index`'s own published pixels, read off its disjoint column
/// range of a `publish_sprite_strip` buffer.
fn sprite_strip_pixels(resident: &[u8], index: usize) -> Vec<u16> {
    let x0 = sprite_strip_x0(index);
    let mut pixels = Vec::new();
    for y in SPRITE_STRIP_Y0..=SPRITE_STRIP_Y1 {
        for x in x0..=(x0 + SPRITE_STRIP_SPAN) {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            pixels.push(u16::from_be_bytes([resident[offset], resident[offset + 1]]));
        }
    }
    pixels
}

/// The `plan.triangle_commands` one word stream's decode produces,
/// read through the same plan walk execution uses rather than
/// re-derived from the wire words.
fn plan_triangle_commands(words: Vec<u32>) -> Vec<u32> {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, per_read_bytes) =
        plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
    let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let mut plan_visitor = PlanCollector::seeded_from_parts(
        None,
        None,
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        physical: coordinator.physical(),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::seeded_from_parts(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            None,
            [(None, None); 8],
        ),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    view.plan.triangle_commands
}

/// **The scale fixture: three fills and three texrects interleaved in
/// one packet, against one color image.**
///
/// Command order, which is the whole subject of this card:
///
/// | # | command | rectangle |
/// |---|---|---|
/// | 0 | fill `0x0842_1085` | whole target (16x8) |
/// | 1 | texrect | x 0..=3, y 0..=1 |
/// | 2 | fill `0x1084_2109` | x 8..=15, y 0..=3 |
/// | 3 | texrect | x 4..=11, y 2..=4 |
/// | 4 | fill `0x2108_4211` | x 0..=7, y 5..=7 |
/// | 5 | texrect | x 12..=15, y 6..=7 |
///
/// The first fill is whole-target because a fresh color target admits
/// nothing else (`PartialNewTargetInitialization`); every later command
/// patches into the buffer that fill established. The interleaving is
/// deliberate: a fill *between* two texrects is the case that a
/// "fills first, then texrects" implementation would get wrong while
/// still passing a test whose commands happened to be grouped.
///
/// The cycle-type switches are load-bearing, not noise. A fill is
/// admitted only in Fill cycle and a texrect only in Copy cycle, so
/// each command sets its own -- and `PlanCollector` snapshots the mode
/// at each command's stream position, which is what makes a fill after
/// a texrect still see Fill cycle.
fn three_fills_and_three_texrects_words() -> Vec<u32> {
    let mut words = Vec::new();
    // Command 0: the whole-target fill that establishes the buffer.
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(MULTI_FILL_COLORS[0]));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        FILL_TARGET_HEIGHT - 1,
    ));
    // The single TMEM load every texrect below samples.
    words.extend(composed_tmem_load_words());
    // Command 1: a texrect in the top-left corner.
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_at(7, 0, 0, 3, 1));
    // Command 2: a fill on the right half of the top rows.
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_fill_color(MULTI_FILL_COLORS[1]));
    words.extend(fill_rectangle(8, 0, 15, 3));
    // Command 3: the middle texrect.
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_at(7, 4, 2, 11, 4));
    // Command 4: a fill across the bottom-left.
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_fill_color(MULTI_FILL_COLORS[2]));
    words.extend(fill_rectangle(0, 5, 7, 7));
    // Command 5: a texrect in the bottom-right corner.
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_at(7, 12, 6, 15, 7));
    words
}

/// The three fill colors `three_fills_and_three_texrects_words` stages,
/// in command order. Named so the fixture and every expectation read
/// the same values.
///
/// All three differ in their high AND low halfwords, so a pixel can be
/// attributed to the fill that wrote it on either column parity -- two
/// fills sharing a halfword would make an "the later fill won" assertion
/// unfalsifiable on half the columns.
const MULTI_FILL_COLORS: [u32; 3] = [0x0842_1085, 0x1084_2109, 0x2108_4211];

/// The six commands' **wire** rectangles, in command order, as
/// `(x0, y0, x1, y1)` inclusive whole-pixel bounds -- exactly the
/// literals the fixture's wire words carry, and nothing more.
///
/// **A texrect's rasterized extent is NOT these corners.** Copy cycle
/// applies `lrx |= 3` / `lry |= 3` and RT64's `FixedRect` ceil is
/// `(coord + 3) >> 2` on all four, so the footprint of a texrect whose
/// wire `lry` is `4 << 2 = 16` is five rows' worth of `lry` (`19`),
/// ceil'd to `5` -- three rows, not the two a naive
/// `y1 - y0 + 1` reading of `(2, 4)` would give for the same command
/// under one cycle. These bounds are therefore the fixture's *input*;
/// the extents the ownership map uses are derived through
/// `texture_rectangle_vertices` in `multi_command_extents`, which is
/// the same ported geometry the decoder and executor use.
///
/// A fill's extent, by contrast, IS its wire corners inclusive: the
/// fill executor's `resolve_fill_pixel_rectangle` refuses a fractional
/// edge outright, so a whole-pixel fill covers exactly `x0..=x1`.
const MULTI_RECTS: [(u32, u32, u32, u32); 6] = [
    (0, 0, 15, 7),
    (0, 0, 3, 1),
    (8, 0, 15, 3),
    (4, 2, 11, 4),
    (0, 5, 7, 7),
    (12, 6, 15, 7),
];

/// Which of the six commands are texrects, in command order.
const MULTI_IS_TEXRECT: [bool; 6] = [false, true, false, true, false, true];

/// Each command's half-open rasterized pixel extent
/// `(x, y, width, height)`, in command order.
///
/// A fill's comes from its wire corners inclusive; a texrect's comes
/// from `texture_rectangle_vertices` -- RT64's own geometry, never the
/// wire corners, for the copy-cycle rounding reason `MULTI_RECTS`
/// states. This is the one place the two kinds' extents are derived,
/// so the ownership map and the per-pixel oracle cannot disagree about
/// where a command drew.
fn multi_command_extents() -> Vec<(u32, u32, u32, u32)> {
    MULTI_RECTS
        .iter()
        .enumerate()
        .map(|(command, (x0, y0, x1, y1))| {
            if MULTI_IS_TEXRECT[command] {
                let draw = texrect_draw_at(*x0, *y0, *x1, *y1);
                (draw.left(), draw.top(), draw.width(), draw.height())
            } else {
                (*x0, *y0, x1 - x0 + 1, y1 - y0 + 1)
            }
        })
        .collect()
}

/// Which command last wrote each pixel of the 16x8 target under
/// `three_fills_and_three_texrects_words`, hand-derived by replaying
/// `MULTI_RECTS` in command order.
///
/// **This is derivation 1 of two.** It is a painter's-algorithm replay
/// -- for each command in order, stamp its rectangle -- which is the
/// semantics this card claims, written independently of the executor's
/// accumulation loop. Derivation 2 is the per-pixel value check in the
/// test itself, which asks the fill oracle or the committed-TMEM oracle
/// for the value that owner should have produced. The two are
/// reconciled by construction: this map says *who*, the oracles say
/// *what*, and a disagreement in either direction fails.
fn multi_command_owner_map() -> Vec<usize> {
    let mut owner = vec![usize::MAX; (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT) as usize];
    for (command, (x, y, width, height)) in multi_command_extents().iter().enumerate() {
        for row in *y..*y + *height {
            for column in *x..*x + *width {
                owner[(row * FILL_TARGET_WIDTH + column) as usize] = command;
            }
        }
    }
    assert!(
        owner.iter().all(|command| *command != usize::MAX),
        "command #0 is a whole-target fill, so every pixel must have an owner"
    );
    owner
}

/// The `PlanCollector` a stream decodes to, walked exactly the way
/// execution walks it.
///
/// Not re-derived from the wire words: the point of a positive control
/// is to measure what the real decoder produced, and a second wire
/// parser here would be a different model that could agree with the
/// fixture while disagreeing with execution.
/// The exact packet WM2000 aborts this backend on: one
/// `G_RDPFULLSYNC` wire command and nothing else. `word(FULL_SYNC, 0)`
/// is `0x29 << 24`, and the trailing zero is the command's second
/// word -- every RDP command in this module's fixtures is two words.
fn sync_only_words() -> Vec<u32> {
    vec![word(FULL_SYNC, 0), 0]
}

/// A packet of nothing but durable RDP register writes: `SetOtherMode`
/// and `SetCombine`, which `PlanCollector` folds into
/// `current_other_mode`/`current_combine` and pushes onto no command
/// list. Two real wire commands, zero completable ones -- the only
/// shape `NoCompletedLoads` still refuses.
fn state_only_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(0, 0));
    words
}

/// [`plan_of`] for a fixture that declares no `TmemLoadSource` reads
/// and carries its own `FullSyncBoundary` records -- the sync-only
/// shape, which `plan_with_deterministic_reads` cannot plan (it fills
/// a load's read, and there is no load).
fn plan_of_no_reads(words: Vec<u32>) -> PlanCollector {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let request = session.plan_request(full_sync_capture(words));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("a reserved sync-only capture must plan cleanly");
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    let mut plan_visitor = PlanCollector::seeded_from_parts(
        None,
        None,
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        plan: PlanCollector::seeded_from_parts(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            None,
            [(None, None); 8],
        ),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        physical: coordinator.physical(),
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    view.plan
}

fn plan_of(words: Vec<u32>) -> PlanCollector {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &mut session, words);
    let read_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, read_capture).unwrap();

    let mut plan_visitor = PlanCollector::seeded_from_parts(
        None,
        None,
        Color4::from_wire(0),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
        Color4::from_wire(0),
        None,
        None,
        [(None, None); 8],
    );
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        plan: PlanCollector::seeded_from_parts(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            None,
            [(None, None); 8],
        ),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        physical: coordinator.physical(),
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    view.plan
}

/// The published resident's full-extent bytes, with the extent asserted.
fn published_target_bytes(backend: &WgpuBackend) -> Vec<u8> {
    let resident = backend
        .color_targets()
        .expect("a composed packet must have built the color-target registry")
        .residents()
        .first()
        .expect("the composed packet must have published exactly one resident")
        .device_bytes()
        .device_bytes()
        .to_vec();
    assert_eq!(
        resident.len() as u32,
        FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
        "the published buffer must be the target's full extent"
    );
    resident
}

/// The `TexrectDraw` a texrect at these whole-pixel bounds produces,
/// rebuilt the way `composed_fixture_draw` rebuilds the single-texrect
/// fixture's: through RT64's own `texture_rectangle_vertices`, never
/// from the wire corners.
fn texrect_draw_at(x0: u32, y0: u32, x1: u32, y1: u32) -> crate::TexrectDraw {
    let words = texrect_words_at(7, x0, y0, x1, y1);
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
    let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
        .expect("the fixture's texrect words decode");
    let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::Copy)
        .expect("the fixture's rectangle is non-empty in copy cycle");
    crate::TexrectDraw::try_from_viewport_and_texcoords(
        vertices.viewport,
        vertices.vertex(0).texcoord(),
        vertices.vertex(3).texcoord(),
    )
    .expect("the fixture's texcoords recover integer S10.5 endpoints")
}

/// The RGBA16 halfword the committed-TMEM oracle says a texrect writes
/// at column/row `(column, row)` of its own rectangle.
///
/// Reads durable state through `sample_committed_point` -- a different
/// function over a different image than the pending post-image the
/// executor sampled -- and asserts the snapshot really is `Committed`,
/// so an oracle that had silently become the implementation would fail
/// rather than agree with itself.
fn expected_texel_halfword(
    committed: &PhysicalTmemState,
    tile: crate::targets::TexrectTileBinding,
    draw: crate::TexrectDraw,
    column: u32,
    row: u32,
) -> u16 {
    let request = crate::PointSampleRequest::new(
        crate::PointSampleCoordinates::new(
            crate::TextureCoordinateS10_5::from_raw(draw.s_at(column)),
            crate::TextureCoordinateS10_5::from_raw(draw.t_at(row)),
        ),
        crate::TmemFirstRowParity::Even,
    );
    let texel = crate::sample_committed_point(
        committed,
        tile.descriptor(),
        tile.size(),
        request,
        crate::TextureLutMode::Disabled,
    )
    .expect("the committed oracle must be able to sample the same texel");
    assert!(
        texel.snapshot().is_committed(),
        "the ORACLE reads durable state, so its snapshot must be Committed -- if this is \
         Proposed the oracle is not independent of the executor"
    );
    let [red, green, blue, _alpha] = texel.texel().rgba8888();
    (u16::from(red >> 3) << 11) | (u16::from(green >> 3) << 6) | (u16::from(blue >> 3) << 1) | 1
}

/// A whole-target fill, one TMEM load, then two texrects whose
/// rectangles **overlap**: x 0..=7 and x 4..=11, both over y 2..=4.
///
/// The 4-pixel offset is what makes the overlap observable. Both
/// texrects sample the same tile, but a pixel in the intersection is a
/// different column of each -- and S advances two texels across an
/// 8-pixel row, so the two columns sample different texels wherever
/// they fall on opposite sides of the row's midpoint.
fn two_overlapping_texrects_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(composed_tmem_load_words());
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(texrect_words_at(7, 0, 2, 7, 4));
    words.extend(texrect_words_at(7, 4, 2, 11, 4));
    words
}

/// How many fill+texrect pairs the scale fixture stages. 16 of each --
/// 33 color commands in one packet, an order of magnitude past the
/// "exactly one of each" the removed refusals enforced, and the same
/// order of magnitude as WM2000 frame 0's 60 + 60.
const SCALE_COMMAND_PAIRS: usize = 16;

const SCALE_TEXRECT_Y0: u32 = 2;

const SCALE_TEXRECT_Y1: u32 = 4;

/// The inclusive wire width of each scale texrect, in pixels - 1.
const SCALE_TEXRECT_SPAN: u32 = 3;

/// The x origin of scale texrect `index`, walked across the target so
/// successive texrects overlap their neighbours rather than stacking.
fn scale_texrect_x0(index: usize) -> u32 {
    (index as u32 * 3) % (FILL_TARGET_WIDTH - SCALE_TEXRECT_SPAN)
}

/// `pairs` fills and `pairs` texrects, interleaved, after one
/// whole-target fill and one TMEM load.
fn many_fills_and_texrects_words(pairs: usize) -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(composed_tmem_load_words());
    for index in 0..pairs {
        // A fill, at a rectangle that moves down the target.
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_fill_color(
            0x0842_1085u32.wrapping_add(index as u32 * 0x0421),
        ));
        let y0 = (index as u32) % FILL_TARGET_HEIGHT;
        words.extend(fill_rectangle(0, y0, FILL_TARGET_WIDTH - 1, y0));
        // A texrect, at a rectangle that moves across it.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        let x0 = scale_texrect_x0(index);
        words.extend(texrect_words_at(
            7,
            x0,
            SCALE_TEXRECT_Y0,
            x0 + SCALE_TEXRECT_SPAN,
            SCALE_TEXRECT_Y1,
        ));
    }
    words
}

// --- One-cycle texrects: the mode WM2000 actually uses ---
//
// `docs/RT64-WM2000-CYCLE-MODES.md` measured 2,520 of 2,520 WM2000
// texrects as one-cycle, zero as Copy, running exactly two combiner
// programs. Everything below executes that shape end to end.

/// The two measured programs' `SetCombine` wire words, packed from
/// `CombineParams`' own **second-cycle** bit positions -- the slice
/// one-cycle mode reads. Deliberately re-derived here from the field
/// layout rather than imported from `targets::texrect`'s own test
/// module: a fixture built from the code under test's helper would
/// agree with it by construction.
///
/// color A `low >> 5 & 0xF`, B `high >> 24 & 0xF`, C `low & 0x1F`,
/// D `high >> 6 & 0x7`; alpha A `high >> 21 & 0x7`, B `high >> 3 & 0x7`,
/// C `high >> 18 & 0x7`, D `high & 0x7`.
fn one_cycle_combine_words(color: [u32; 4], alpha: [u32; 4]) -> [u32; 2] {
    let [ca, cb, cc, cd] = color;
    let [aa, ab, ac, ad] = alpha;
    let low = (ca << 5) | cc;
    let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
    set_combine(low, high)
}

/// Program 1: RGB `(Environment - Texel0) * Primitive + Texel0`,
/// Alpha `(Texel0 - Zero) * Primitive + Zero`. 2,100 of 2,520.
const ENV_LERP_COLOR: [u32; 4] = [5, 1, 3, 1];

const ENV_LERP_ALPHA: [u32; 4] = [1, 7, 3, 7];

/// Program 2: both channels `(Zero - Zero) * Zero + Primitive`. 420 of
/// 2,520. Each slot's ZERO index is its OWN out-of-table value.
const FLAT_PRIM_COLOR: [u32; 4] = [8, 8, 16, 3];

const FLAT_PRIM_ALPHA: [u32; 4] = [7, 7, 7, 3];

const ONE_CYCLE_ENV_WIRE: u32 = 0xFF00_80FF;

const ONE_CYCLE_PRIM_WIRE: u32 = 0x80FF_4080;

/// `fill_load_and_copy_texrect_words` with the cycle switched to
/// **one-cycle** and a real `SetCombine`/`SetEnvColor`/`SetPrimColor`
/// staged before the rectangle.
///
/// Everything else -- the fill, the `LoadBlock`, the tile, the
/// rectangle's own wire words -- is byte-identical to the Copy fixture,
/// so the only difference between the two executions is the cycle type
/// and the combiner program. That is what makes the Copy regression
/// guard and this test a controlled pair rather than two unrelated
/// fixtures.
fn fill_load_and_one_cycle_texrect_words(color: [u32; 4], alpha: [u32; 4]) -> Vec<u32> {
    let mut words = whole_target_fill_words();
    // The tip's own load run, reused rather than re-inlined: a second
    // copy of the same five commands would be free to drift from the
    // fixture every other texrect test samples.
    words.extend(composed_tmem_load_words());
    // One-cycle (0), where Copy is 2.
    words.extend(set_other_mode(0, 0));
    words.extend(one_cycle_combine_words(color, alpha));
    words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
    // `lod_frac`/`lod_min` deliberately non-zero: neither measured
    // program reads `prim_lod_frac`, so a leak into a color channel
    // shows up as a wrong pixel here.
    words.extend(set_prim_color(0x40, 0x05, ONE_CYCLE_PRIM_WIRE));
    words.extend(texrect_words_in_target_stepping(7));
    words
}

/// The one-cycle rectangle, derived **twice** and reconciled.
///
/// Derivation 1, RT64's own `texture_rectangle_vertices`: the wire
/// fields are `ulx=16, uly=8, lrx=44, lry=16`. One-cycle applies
/// **neither** Copy's `lrx |= 3`/`lry |= 3` **nor** fill/copy's
/// `ulx &= !3` -- both are cycle-gated -- so the four values are
/// unchanged. `(coord + 3) >> 2` on each gives `4, 2, 11, 4`.
/// Half-open: pixels **x 4..=10, y 2..=3** -- 7 wide, 2 tall.
///
/// Derivation 2, independent: `ceil(coord / 4)` on `16, 8, 44, 16` is
/// `4, 2, 11, 4`. Same.
///
/// **This differs from the Copy fixture's 8x3 for the identical wire
/// words**, which is precisely why the extent must come from the ported
/// geometry and never from the wire corners. `the_one_cycle_extent_
/// differs_from_the_copy_extent_for_identical_wire_words` asserts that
/// difference rather than leaving it as a comment.
const ONE_CYCLE_X0: u32 = 4;

const ONE_CYCLE_Y0: u32 = 2;

const ONE_CYCLE_WIDTH: u32 = 7;

const ONE_CYCLE_HEIGHT: u32 = 2;

/// The one-cycle draw, through `texture_rectangle_vertices` -- the same
/// ported geometry the decoder and executor both use, for the reason
/// `composed_fixture_draw` states.
fn one_cycle_fixture_draw() -> crate::TexrectDraw {
    let words = texrect_words_in_target_stepping(7);
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
    let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
        .expect("the fixture's texrect words decode");
    let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::OneCycle)
        .expect("the fixture's rectangle is non-empty in one-cycle");
    crate::TexrectDraw::try_from_viewport_and_texcoords(
        vertices.viewport,
        vertices.vertex(0).texcoord(),
        vertices.vertex(3).texcoord(),
    )
    .expect("the fixture's texcoords recover integer S10.5 endpoints")
}

/// The hand-derived combined RGBA16 halfword for one pixel of the
/// env-lerp program, computed from the committed-TMEM oracle's texel.
///
/// This mirrors the executor's quantization -- normalize by `/ 255.0`,
/// `run_one_cycle`, `* 255.0` and `round`, then `write_pixel`'s RGBA16
/// color truncation followed by this fixture's full stored-coverage bit --
/// and is deliberately written out here rather than
/// calling a shared helper, so the two are independently authored
/// statements of the same rule that must reconcile.
fn expected_one_cycle_halfword(texel: [u8; 4], color: [u32; 4], alpha: [u32; 4]) -> u16 {
    expected_one_cycle_halfword_with_prim(texel, color, alpha, ONE_CYCLE_PRIM_WIRE)
}

/// [`expected_one_cycle_halfword`] with the primitive register named
/// explicitly, for the multi-texrect fixture where each command stages
/// its own.
fn expected_one_cycle_halfword_with_prim(
    texel: [u8; 4],
    color: [u32; 4],
    alpha: [u32; 4],
    prim_wire: u32,
) -> u16 {
    let combine_words = one_cycle_combine_words(color, alpha);
    let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
    let inputs = crate::combiner::combiner_inputs_from_fragment_registers(
        crate::combiner::CombinerInputs {
            tex_val0: [
                f32::from(texel[0]) / 255.0,
                f32::from(texel[1]) / 255.0,
                f32::from(texel[2]) / 255.0,
                f32::from(texel[3]) / 255.0,
            ],
            tex_val1: [0.0; 4],
            prim_color: [0.0; 4],
            shade_color: [0.0; 4],
            env_color: [0.0; 4],
            key_center: [0.0; 3],
            key_scale: [0.0; 3],
            lod_fraction: 0.0,
            prim_lod_frac: 0.0,
            noise: 0.0,
            k4: 0.0,
            k5: 0.0,
        },
        crate::state::Color4::from_wire(ONE_CYCLE_ENV_WIRE),
        crate::state::PrimColor::from_wire(0x05 << 8 | 0x40, prim_wire),
    );
    let (combined, _alpha_compare) = crate::combiner::run_one_cycle(params, inputs);
    let [red, green, blue, _alpha] = combined.map(|channel| (channel * 255.0).round() as u8);
    (u16::from(red >> 3) << 11) | (u16::from(green >> 3) << 6) | (u16::from(blue >> 3) << 1) | 1
}

/// The three primitive colours the multi-texrect one-cycle fixture
/// stages, one per texrect, in command order.
///
/// All three differ in **both** RGBA16 halves after the `>> 3` / `>> 7`
/// pack (`0x87D1`, `0xFA21`, `0x443F`), so every pixel can be
/// attributed to the texrect that wrote it. Two texrects sharing a
/// packed value would make "the later one won the overlap"
/// unfalsifiable exactly where it matters.
const MULTI_ONE_CYCLE_PRIM: [u32; 3] = [0x80FF_4080, 0xFF40_8080, 0x4080_FF80];

/// **Three one-cycle texrects in one packet, each running the combiner
/// against the accumulated buffer.**
///
/// The shape WM2000 actually issues -- its early frames carry 60 flat
/// rectangles plus 25 tinted ones per entry
/// (`docs/RT64-WM2000-CYCLE-MODES.md` §3) -- and a shape that could not
/// be expressed before the N-command accumulation seam landed.
///
/// | # | command | wire rectangle | primitive |
/// |---|---|---|---|
/// | 0 | fill | whole target | `0x0842_1085` |
/// | 1 | one-cycle texrect | x 0..=4, y 0..=2 | `0x80FF_4080` |
/// | 2 | one-cycle texrect | x 3..=8, y 1..=4 | `0xFF40_8080` |
/// | 3 | one-cycle texrect | x 10..=15, y 5..=7 | `0x4080_FF80` |
///
/// **Texrects 1 and 2 deliberately overlap.** Under one cycle the
/// extents are 4x2 at (0,0) and 5x3 at (3,1), which share the single
/// pixel (3, 1). That pixel must carry texrect 2's colour, and it is
/// the only assertion in this file that can distinguish "the loop
/// composed in journal order" from "the loop composed in some order".
///
/// Each texrect stages its **own** `SetPrimColor` before its own
/// rectangle, which is what makes this a per-command test rather than
/// three copies of one draw: the executor must read the register
/// latched at each texrect's own stream position, not the walk's final
/// value. If it read the final value every rectangle would be
/// `0x4080_FF80` and the first two assertions would fail.
///
/// All three run the flat-primitive program, for a measured reason
/// rather than convenience: it references no texel, so
/// `references_texels_in_first_cycle` is false and the GPU fragment
/// shader short-circuits past the pre-existing committed-vs-pending
/// TMEM projection defect that
/// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_tmem_projection`
/// pins. It is still a genuine per-fragment combiner evaluation --
/// `run_one_cycle` runs on every pixel of all three rectangles.
fn three_one_cycle_texrects_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(COMPOSED_FILL_COLOR));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        FILL_TARGET_HEIGHT - 1,
    ));
    words.extend(composed_tmem_load_words());
    for (index, (x0, y0, x1, y1)) in [(0u32, 0u32, 4u32, 2u32), (3, 1, 8, 4), (10, 5, 15, 7)]
        .into_iter()
        .enumerate()
    {
        // One-cycle (0), re-stated per command: `PlanCollector`
        // snapshots the mode at each command's own stream position.
        words.extend(set_other_mode(0, 0));
        words.extend(one_cycle_combine_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA));
        words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
        words.extend(set_prim_color(0x40, 0x05, MULTI_ONE_CYCLE_PRIM[index]));
        words.extend(texrect_words_at(7, x0, y0, x1, y1));
    }
    words
}

/// The one-cycle rasterized extent of one wire rectangle, through
/// RT64's own `texture_rectangle_vertices` -- never the wire corners.
///
/// One cycle applies neither Copy's `lrx |= 3` nor fill/copy's
/// `ulx &= !3`, so `(coord + 3) >> 2` runs on the raw 10.2 fields.
/// Returned half-open as `(left, top, right, bottom)`.
fn one_cycle_extent_of(x0: u32, y0: u32, x1: u32, y1: u32) -> (u32, u32, u32, u32) {
    let words = texrect_words_at(7, x0, y0, x1, y1);
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
    let raw = crate::RawTextureRectangle::decode(0x24, &bytes).expect("the words decode");
    let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::OneCycle)
        .expect("the rectangle is non-empty in one cycle");
    let viewport = vertices.viewport;
    (
        viewport.left as u32,
        viewport.top as u32,
        viewport.right as u32,
        viewport.bottom as u32,
    )
}

/// `a_second_packet_composes_into_the_color_image_the_first_one_declared`'s
/// second packet, as its own function so the positive control measures
/// the identical word stream the test executes rather than a retyped
/// copy of it.
fn second_words_for_control() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_other_mode(2, 0));
    words.extend(set_combine(0, 0));
    words.extend(set_tile(7, 1, 0));
    words.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
    words.extend(texrect_words_in_target(7));
    words
}

/// Build an `RspMemory` whose IMEM holds `text`, zero-padded, so the
/// digest `process_task` reports is a value this test chose rather than
/// whatever a default bank happens to hash to.
fn rsp_memory_with_imem(text: &[u8]) -> fn64_runtime::RspMemory {
    let mut memory = fn64_runtime::RspMemory::new();
    memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            text,
        )
        .expect("the fixture microcode fits in the IMEM bank");
    memory
}
