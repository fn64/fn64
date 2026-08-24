//! Offline timing replay for one production raw-DPC XBUS receipt.
//!
//! The command stream and RDRAM image are runtime inputs and must remain
//! outside git. The replay drives the same public plan -> execute -> guest
//! commit -> publish lifecycle as the ABI production route. Each iteration
//! starts from the same RDRAM bytes, and every committed payload must remain
//! byte-identical across the run.
//!
//! Usage:
//! `cargo run --release -p fn64-render-wgpu --features host-gpu-tests \
//!     --example raw_dpc_replay -- stream.bin rdram-image.bin`

use std::{
    path::Path,
    time::{Duration, Instant},
};

use fn64_render::{
    count_raw_rdp_full_sync_sites, ir_effect_content_digest, OwnedRawDpcCapture,
    OwnedRawDpcSubmission, RawDpcAbiSession, RenderBackend, RenderConfig,
};
use fn64_render_ir::{
    CapturedGuestRead, DeferredGuestReadCapture, DpInterruptState, FullSyncBoundary,
    PhysicalMemoryLayout, ResourceRegion, TemporalBoundary,
};
use fn64_render_wgpu::WgpuBackend;
use fn64_runtime::{
    rom::{InMemoryRom, PiDma},
    Cycles, DeviceFabric, DpcSubmissionSource, FixedPiTiming, RdramAddr, RdramView, RdramViewMut,
    TvType,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Default)]
struct Timings {
    plan: Duration,
    reads: Duration,
    finalize: Duration,
    execute: Duration,
    compute_probe: Duration,
    compute_probe_submissions: u32,
    compute_probe_batches: u32,
    compute_probe_draws: u32,
    compute_probe_pixels: u32,
    compute_replace: Duration,
    compute_replace_admission: Duration,
    compute_replace_effects: Duration,
    compute_replace_submissions: u32,
    compute_replace_batches: u32,
    compute_replace_draws: u32,
    compute_replace_pixels: u32,
    commit: Duration,
    copyback: Duration,
    publish: Duration,
    total: Duration,
}

impl Timings {
    fn add(&mut self, other: Self) {
        self.plan += other.plan;
        self.reads += other.reads;
        self.finalize += other.finalize;
        self.execute += other.execute;
        self.compute_probe += other.compute_probe;
        self.compute_probe_submissions += other.compute_probe_submissions;
        self.compute_probe_batches += other.compute_probe_batches;
        self.compute_probe_draws += other.compute_probe_draws;
        self.compute_probe_pixels += other.compute_probe_pixels;
        self.compute_replace += other.compute_replace;
        self.compute_replace_admission += other.compute_replace_admission;
        self.compute_replace_effects += other.compute_replace_effects;
        self.compute_replace_submissions += other.compute_replace_submissions;
        self.compute_replace_batches += other.compute_replace_batches;
        self.compute_replace_draws += other.compute_replace_draws;
        self.compute_replace_pixels += other.compute_replace_pixels;
        self.commit += other.commit;
        self.copyback += other.copyback;
        self.publish += other.publish;
        self.total += other.total;
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let stream_path = args
        .next()
        .expect("usage: raw_dpc_replay <stream.bin|stream-dir> <rdram-image.bin>");
    let rdram_path = args
        .next()
        .expect("usage: raw_dpc_replay <stream.bin> <rdram-image.bin>");
    assert!(
        args.next().is_none(),
        "unexpected third positional argument"
    );

    let mut streams = load_streams(Path::new(&stream_path));
    let terminal_index = env_u32(
        "FN64_RAW_DPC_REPLAY_PACKET",
        u32::try_from(streams.len() - 1).expect("stream count exceeds u32"),
    ) as usize;
    assert!(
        terminal_index < streams.len(),
        "FN64_RAW_DPC_REPLAY_PACKET {terminal_index} is outside {} captured packets",
        streams.len()
    );
    streams.truncate(terminal_index + 1);
    let window = env_u32("FN64_RAW_DPC_REPLAY_WINDOW", 1) as usize;
    assert!(window > 0, "FN64_RAW_DPC_REPLAY_WINDOW must be nonzero");
    assert!(
        window <= streams.len(),
        "FN64_RAW_DPC_REPLAY_WINDOW {window} exceeds {} available packets",
        streams.len()
    );
    let benchmark_start = streams.len() - window;
    let selected_window_packets = window;
    let combine_window = std::env::var_os("FN64_RAW_DPC_REPLAY_COMBINE_WINDOW")
        .is_some_and(|value| value == "1");
    if combine_window {
        // This is a diagnostic control, not a candidate transport contract:
        // concatenation asks whether execution can tolerate a larger lifetime.
        // Production grouping must retain each submission's journal and
        // architectural publication boundary.
        let combined = ReplayStream::from_bytes(
            streams[benchmark_start..]
                .iter()
                .flat_map(|stream| stream.bytes.iter().copied())
                .collect(),
            ReplaySource::Rdram,
        );
        streams.truncate(benchmark_start);
        streams.push(combined);
    }
    let (prefix, benchmark) = streams.split_at(benchmark_start);
    let terminal = benchmark.last().expect("nonempty benchmark window");
    let pristine =
        std::fs::read(&rdram_path).unwrap_or_else(|error| panic!("reading {rdram_path}: {error}"));
    let layout_bytes = u32::try_from(pristine.len()).expect("RDRAM image exceeds u32");
    let start = env_u32("FN64_RAW_DPC_REPLAY_START", 0);
    let end = start
        .checked_add(u32::try_from(terminal.bytes.len()).expect("stream exceeds u32"))
        .expect("XBUS stream end overflows u32");
    if terminal.source == ReplaySource::Xbus {
        assert!(
            end <= 0x1000,
            "XBUS stream [{start:#x}, {end:#x}) exceeds DMEM"
        );
    }
    let width = env_u32("FN64_RAW_DPC_REPLAY_WIDTH", 320);
    let height = env_u32("FN64_RAW_DPC_REPLAY_HEIGHT", 240);
    let warmup = env_u32("FN64_RAW_DPC_REPLAY_WARMUP", 10);
    let repeat = env_u32("FN64_RAW_DPC_REPLAY_REPEAT", 100);
    let detail = std::env::var_os("FN64_RAW_DPC_REPLAY_DETAIL").is_some();
    let compute_probe_requested =
        std::env::var_os("FN64_COMPUTE_RASTER_PROBE").is_some_and(|value| value == "1");
    let compute_chain_probe_requested =
        std::env::var_os("FN64_COMPUTE_RASTER_CHAIN_PROBE").is_some_and(|value| value == "1");
    let compute_replace_requested =
        std::env::var_os("FN64_COMPUTE_RASTER_REPLACE").is_some_and(|value| value == "1");
    let compute_replace_ab =
        std::env::var_os("FN64_COMPUTE_RASTER_REPLACE_AB").is_some_and(|value| value == "1");
    let compute_first = std::env::var_os("FN64_COMPUTE_RASTER_REPLACE_AB_COMPUTE_FIRST")
        .is_some_and(|value| value == "1");
    assert!(repeat > 0, "FN64_RAW_DPC_REPLAY_REPEAT must be nonzero");
    assert!(
        !compute_replace_ab || (!compute_replace_requested && repeat % 2 == 0),
        "replacement A/B requires an even repeat and must not be combined with fixed replacement"
    );

    let (mut backend, mut session) = WgpuBackend::try_new().expect("construct wgpu backend");
    backend
        .create(&RenderConfig {
            width,
            height,
            tv_type: TvType::default(),
        })
        .expect("create wgpu backend");
    backend.set_compute_raster_probe_enabled(false);
    backend.set_compute_raster_replace_enabled(false);

    let mut postimage = pristine.clone();
    for (index, stream) in prefix.iter().enumerate() {
        let prefix_end = u32::try_from(stream.bytes.len()).expect("prefix stream exceeds u32");
        replay_once(
            &mut backend,
            &mut session,
            &stream.words,
            &stream.bytes,
            stream.source,
            &pristine,
            &mut postimage,
            layout_bytes,
            0,
            prefix_end,
            u64::try_from(index).expect("prefix index exceeds u64") + 1,
        );
    }
    if !prefix.is_empty() {
        println!(
            "primed durable renderer state with {} packet(s)",
            prefix.len()
        );
    }
    backend.set_compute_raster_probe_enabled(compute_probe_requested);
    backend.set_compute_raster_chain_probe_enabled(compute_chain_probe_requested);
    backend.set_compute_raster_replace_enabled(compute_replace_requested);

    let mut samples = Vec::with_capacity(repeat as usize);
    let mut cpu_samples = Vec::with_capacity((repeat / 2) as usize);
    let mut replacement_samples = Vec::with_capacity((repeat / 2) as usize);
    let mut packet_samples = vec![Vec::with_capacity(repeat as usize); benchmark.len()];
    let mut expected_digest = None;
    let mut expected_committed_bytes = None;
    for iteration in 0..warmup + repeat {
        let replacement_iteration = if compute_replace_ab {
            (iteration % 2 == 0) == compute_first
        } else {
            compute_replace_requested
        };
        backend.set_compute_raster_replace_enabled(replacement_iteration);
        let mut timings = Timings::default();
        let mut digest = 0xcbf2_9ce4_8422_2325_u64;
        let mut committed_bytes = Vec::new();
        for (window_index, stream) in benchmark.iter().enumerate() {
            let packet_start = if window_index + 1 == benchmark.len() {
                start
            } else {
                0
            };
            let packet_end = packet_start
                + u32::try_from(stream.bytes.len()).expect("benchmark stream exceeds u32");
            let sequence = u64::try_from(prefix.len()).expect("prefix length exceeds u64")
                + u64::from(iteration)
                    * u64::try_from(benchmark.len()).expect("window exceeds u64")
                + u64::try_from(window_index).expect("window index exceeds u64")
                + 1;
            let (packet_timings, packet_digest, packet_bytes) = replay_once(
                &mut backend,
                &mut session,
                &stream.words,
                &stream.bytes,
                stream.source,
                &pristine,
                &mut postimage,
                layout_bytes,
                packet_start,
                packet_end,
                sequence,
            );
            timings.add(packet_timings);
            committed_bytes.extend_from_slice(&packet_bytes);
            if iteration >= warmup {
                packet_samples[window_index].push(packet_timings);
            }
            for byte in packet_digest.to_le_bytes() {
                digest ^= u64::from(byte);
                digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        match expected_digest {
            None => expected_digest = Some(digest),
            Some(expected) => assert_eq!(
                digest, expected,
                "committed guest bytes changed across identical replay iterations"
            ),
        }
        match &expected_committed_bytes {
            None => expected_committed_bytes = Some(committed_bytes),
            Some(expected) => assert_eq!(
                committed_bytes, *expected,
                "committed guest bytes changed across identical replay iterations"
            ),
        }
        if iteration >= warmup {
            samples.push(timings);
            if compute_replace_ab {
                if replacement_iteration {
                    replacement_samples.push(timings);
                } else {
                    cpu_samples.push(timings);
                }
            }
        }
    }

    println!(
        "selected_window_packets={} replay_packets={} combined_window={} \
         terminal_stream_bytes={} warmup={} repeat={} \
         committed_fnv1a={:016x} postimage_sha256={:x}",
        selected_window_packets,
        benchmark.len(),
        combine_window,
        terminal.bytes.len(),
        warmup,
        repeat,
        expected_digest.expect("at least one replay"),
        Sha256::digest(&postimage),
    );
    report("plan", &samples, |sample| sample.plan);
    report("guest_reads", &samples, |sample| sample.reads);
    report("finalize", &samples, |sample| sample.finalize);
    report("execute", &samples, |sample| sample.execute);
    report("compute_probe", &samples, |sample| sample.compute_probe);
    report("compute_replace", &samples, |sample| sample.compute_replace);
    report("replace_admit", &samples, |sample| {
        sample.compute_replace_admission
    });
    report("replace_effects", &samples, |sample| {
        sample.compute_replace_effects
    });
    report("commit", &samples, |sample| sample.commit);
    report("copyback", &samples, |sample| sample.copyback);
    report("publish", &samples, |sample| sample.publish);
    report("total", &samples, |sample| sample.total);
    if compute_replace_ab {
        report("ab_cpu_execute", &cpu_samples, |sample| sample.execute);
        report("ab_cpu_total", &cpu_samples, |sample| sample.total);
        report("ab_gpu_execute", &replacement_samples, |sample| {
            sample.execute
        });
        report("ab_gpu_work", &replacement_samples, |sample| {
            sample.compute_replace
        });
        report("ab_gpu_total", &replacement_samples, |sample| sample.total);
    }
    let probe_batches: u32 = samples
        .iter()
        .map(|sample| sample.compute_probe_batches)
        .sum();
    let probe_submissions: u32 = samples
        .iter()
        .map(|sample| sample.compute_probe_submissions)
        .sum();
    let probe_draws: u32 = samples
        .iter()
        .map(|sample| sample.compute_probe_draws)
        .sum();
    let probe_pixels: u32 = samples
        .iter()
        .map(|sample| sample.compute_probe_pixels)
        .sum();
    println!(
        "compute_probe_submissions={} batches={} draws={} target_pixels={} across_repeats={}",
        probe_submissions, probe_batches, probe_draws, probe_pixels, repeat
    );
    let replace_batches: u32 = samples
        .iter()
        .map(|sample| sample.compute_replace_batches)
        .sum();
    let replace_submissions: u32 = samples
        .iter()
        .map(|sample| sample.compute_replace_submissions)
        .sum();
    let replace_draws: u32 = samples
        .iter()
        .map(|sample| sample.compute_replace_draws)
        .sum();
    let replace_pixels: u32 = samples
        .iter()
        .map(|sample| sample.compute_replace_pixels)
        .sum();
    println!(
        "compute_replace_submissions={} batches={} draws={} target_pixels={} across_repeats={}",
        replace_submissions, replace_batches, replace_draws, replace_pixels, repeat
    );
    if detail {
        for (window_index, packet) in packet_samples.iter().enumerate() {
            let packet_index = prefix.len() + window_index;
            report(
                &format!("packet_{packet_index}_execute"),
                packet,
                |sample| sample.execute,
            );
            report(&format!("packet_{packet_index}_total"), packet, |sample| {
                sample.total
            });
            report(
                &format!("packet_{packet_index}_compute_probe"),
                packet,
                |sample| sample.compute_probe,
            );
            report(
                &format!("packet_{packet_index}_compute_replace"),
                packet,
                |sample| sample.compute_replace,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn replay_once(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: &[u32],
    stream: &[u8],
    source: ReplaySource,
    pristine: &[u8],
    postimage: &mut [u8],
    layout_bytes: u32,
    start: u32,
    end: u32,
    sequence: u64,
) -> (Timings, u64, Vec<u8>) {
    let total_started = Instant::now();
    let capture = capture(words, stream, source, layout_bytes, start, end, sequence);

    let started = Instant::now();
    let planned = backend
        .plan_raw_dpc(session.plan_request(capture))
        .expect("plan captured packet");
    let plan = started.elapsed();

    let started = Instant::now();
    let reads = planned
        .guest_read_plan()
        .reads()
        .iter()
        .map(|read| {
            let mut bytes = vec![0; read.range().len() as usize];
            RdramView::from_storage(pristine).copy_logical_bytes(
                RdramAddr::from_offset(read.range().start().get()),
                &mut bytes,
            );
            CapturedGuestRead::try_new(*read, bytes).expect("capture declared guest read")
        })
        .collect();
    let deferred = DeferredGuestReadCapture::new(reads);
    let reads = started.elapsed();

    let started = Instant::now();
    let bound = session
        .finalize_and_submit(planned, deferred)
        .expect("finalize captured packet");
    let finalize = started.elapsed();
    let submission = bound.submission();

    let started = Instant::now();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("execute captured packet");
    let execute = started.elapsed();
    let compute_probe = backend.take_compute_raster_probe_receipt();
    let compute_replace = backend.take_compute_raster_replace_receipt();

    let staged = backend.staged_guest_render_target_writes(submission);
    let payloads = backend.committed_guest_render_target_bytes(submission);
    assert_eq!(staged.len(), payloads.len(), "one payload per staged write");
    for (write, payload) in staged.iter().zip(&payloads) {
        assert_eq!(payload.len() as u32, write.byte_count());
        assert_eq!(ir_effect_content_digest(payload), write.content());
    }

    let started = Instant::now();
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .expect("commit captured packet guest writes");
    let commit = started.elapsed();

    let started = Instant::now();
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    let mut committed_bytes = Vec::new();
    for (write, payload) in staged.iter().zip(&payloads) {
        let ResourceRegion::Rdram { range, .. } = write.access().region() else {
            panic!("guest render-target write does not name RDRAM");
        };
        RdramViewMut::from_storage(postimage)
            .write_logical_bytes(RdramAddr::from_offset(range.start().get()), payload);
        for &byte in payload {
            committed_bytes.push(byte);
            digest ^= u64::from(byte);
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    let copyback = started.elapsed();

    let started = Instant::now();
    let mut fabric = DeviceFabric::new(
        PiDma::new(InMemoryRom::new(Vec::new())),
        FixedPiTiming(Cycles::new(0)),
    );
    fabric
        .request_dpc_submission(source.fabric_source(), start, end)
        .expect("request replay DPC submission")
        .expect("fresh replay fabric is not frozen");
    let token = fabric
        .pending_dpc_submission()
        .expect("replay fabric has one pending submission")
        .token;
    let ready = fabric
        .prepare_dpc_commit(token)
        .expect("prepare replay commit");
    let capsule = session
        .seal_publication(committed, ready)
        .expect("seal replay publication");
    backend.publish_raw_dpc(capsule);
    let publish = started.elapsed();

    (
        Timings {
            plan,
            reads,
            finalize,
            execute,
            compute_probe: compute_probe
                .map(|receipt| receipt.elapsed())
                .unwrap_or_default(),
            compute_probe_submissions: compute_probe
                .map(|receipt| receipt.submission_count())
                .unwrap_or_default(),
            compute_probe_batches: compute_probe
                .map(|receipt| receipt.batch_count())
                .unwrap_or_default(),
            compute_probe_draws: compute_probe
                .map(|receipt| receipt.draw_count())
                .unwrap_or_default(),
            compute_probe_pixels: compute_probe
                .map(|receipt| receipt.target_pixels())
                .unwrap_or_default(),
            compute_replace: compute_replace
                .map(|receipt| receipt.elapsed())
                .unwrap_or_default(),
            compute_replace_admission: compute_replace
                .map(|receipt| receipt.admission_elapsed())
                .unwrap_or_default(),
            compute_replace_effects: compute_replace
                .map(|receipt| receipt.effects_elapsed())
                .unwrap_or_default(),
            compute_replace_submissions: compute_replace
                .map(|receipt| receipt.submission_count())
                .unwrap_or_default(),
            compute_replace_batches: compute_replace
                .map(|receipt| receipt.batch_count())
                .unwrap_or_default(),
            compute_replace_draws: compute_replace
                .map(|receipt| receipt.draw_count())
                .unwrap_or_default(),
            compute_replace_pixels: compute_replace
                .map(|receipt| receipt.target_pixels())
                .unwrap_or_default(),
            commit,
            copyback,
            publish,
            total: total_started.elapsed(),
        },
        digest,
        committed_bytes,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplaySource {
    Xbus,
    Rdram,
}

impl ReplaySource {
    const fn fabric_source(self) -> DpcSubmissionSource {
        match self {
            Self::Xbus => DpcSubmissionSource::Dmem,
            Self::Rdram => DpcSubmissionSource::Rdram,
        }
    }
}

struct ReplayStream {
    bytes: Vec<u8>,
    words: Vec<u32>,
    source: ReplaySource,
}

impl ReplayStream {
    fn from_bytes(bytes: Vec<u8>, source: ReplaySource) -> Self {
        assert!(
            !bytes.is_empty() && bytes.len().is_multiple_of(8),
            "replay stream length {:#x} must be nonzero and 8-byte aligned",
            bytes.len()
        );
        let words = bytes
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte word")))
            .collect();
        Self {
            bytes,
            words,
            source,
        }
    }
}

fn load_streams(path: &Path) -> Vec<ReplayStream> {
    let paths = if path.is_dir() {
        let mut paths: Vec<_> = std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
            .map(|entry| entry.expect("read stream-directory entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("raw-dpc-") && name.ends_with("-xbus.bin"))
            })
            .collect();
        paths.sort();
        paths
    } else {
        vec![path.to_path_buf()]
    };
    assert!(
        !paths.is_empty(),
        "no raw-dpc XBUS streams found at {}",
        path.display()
    );
    paths
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
            assert!(
                !bytes.is_empty() && bytes.len().is_multiple_of(8),
                "stream {} length {:#x} must be nonzero and 8-byte aligned",
                path.display(),
                bytes.len()
            );
            ReplayStream::from_bytes(bytes, ReplaySource::Xbus)
        })
        .collect()
}

fn capture(
    words: &[u32],
    stream: &[u8],
    source: ReplaySource,
    layout_bytes: u32,
    start: u32,
    end: u32,
    sequence: u64,
) -> OwnedRawDpcCapture {
    let submission = match source {
        ReplaySource::Xbus => {
            OwnedRawDpcSubmission::from_xbus_payload(start, end, stream.to_vec())
        }
        ReplaySource::Rdram => {
            OwnedRawDpcSubmission::from_rdram_words(start, end, words.to_vec())
        }
    }
    .expect("construct replay submission");
    let layout = PhysicalMemoryLayout::try_new(layout_bytes).expect("construct RDRAM layout");
    let sites = count_raw_rdp_full_sync_sites(words)
        .expect("scan replay FullSync sites")
        .complete()
        .expect("captured stream ends at a command boundary");
    let cmd_end = TemporalBoundary::new(1, DpInterruptState::Clear);
    if sites == 0 {
        return OwnedRawDpcCapture::new(submission, layout, sequence, cmd_end);
    }
    let boundaries = (0..sites as u64)
        .map(|ordinal| {
            FullSyncBoundary::new(
                2 + ordinal * 2,
                3 + ordinal * 2,
                DpInterruptState::Clear,
                DpInterruptState::Clear,
            )
        })
        .collect();
    OwnedRawDpcCapture::with_full_sync_boundaries(submission, layout, sequence, cmd_end, boundaries)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name).map_or(default, |raw| {
        let raw = raw.trim();
        if let Some(hex) = raw.strip_prefix("0x") {
            u32::from_str_radix(hex, 16)
                .unwrap_or_else(|_| panic!("{name} must be a u32, got {raw:?}"))
        } else {
            raw.parse()
                .unwrap_or_else(|_| panic!("{name} must be a u32, got {raw:?}"))
        }
    })
}

fn report(name: &str, samples: &[Timings], select: impl Fn(&Timings) -> Duration) {
    let mut values: Vec<f64> = samples
        .iter()
        .map(|sample| select(sample).as_secs_f64() * 1_000.0)
        .collect();
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let percentile = |fraction: f64| values[((values.len() - 1) as f64 * fraction) as usize];
    println!(
        "{name:>11} mean_ms={mean:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
        percentile(0.50),
        percentile(0.95),
        percentile(0.99),
        values[values.len() - 1]
    );
}
