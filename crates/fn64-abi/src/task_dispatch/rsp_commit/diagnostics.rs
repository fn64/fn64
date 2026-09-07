use super::*;
use fn64_audio::rsp::runtime::RspDpCommandSource;

pub(super) struct XbusStreamDump {
    pub(super) directory: std::path::PathBuf,
    pub(super) skip: u64,
    pub(super) rdram_index: Option<u64>,
}

pub(super) struct XbusDiagnostics {
    pub(super) stream_dump: Option<XbusStreamDump>,
    pub(super) diff_trace: bool,
}

struct SessionStreamDump {
    directory: std::path::PathBuf,
    skip: u64,
    count: u64,
    rdram_index: Option<u64>,
}

fn session_stream_dump() -> Option<&'static SessionStreamDump> {
    static CONFIG: std::sync::OnceLock<Option<SessionStreamDump>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            crate::diag_env::diag_env("FN64_RAW_DPC_STREAM_DUMP_DIR").map(|directory| {
                let parse = |name: &'static str, default: Option<u64>| {
                    crate::diag_env::diag_env(name).map_or(default, |raw| {
                        Some(
                            raw.parse::<u64>()
                                .unwrap_or_else(|_| panic!("{name} must be a u64, got {raw:?}")),
                        )
                    })
                };
                SessionStreamDump {
                    directory: directory.into(),
                    skip: parse("FN64_RAW_DPC_STREAM_DUMP_SKIP", Some(0))
                        .expect("raw-DPC dump skip has a default"),
                    count: parse("FN64_RAW_DPC_STREAM_DUMP_COUNT", Some(16))
                        .expect("raw-DPC dump count has a default"),
                    rdram_index: parse("FN64_RAW_DPC_STREAM_DUMP_RDRAM", None),
                }
            })
        })
        .as_ref()
}

pub(crate) const fn session_stream_dump_selected(index: u64, skip: u64, count: u64) -> bool {
    index >= skip && index - skip < count
}

pub(crate) fn raw_dpc_stream_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_be_bytes()).collect()
}

pub(super) fn maybe_dump_session_raw_dpc(
    submission: &fn64_render::OwnedRawDpcSubmission,
    words: &[u32],
    rdram: &[u8],
) {
    let Some(dump) = session_stream_dump() else {
        return;
    };
    thread_local! {
        static SESSION_DUMP_INDEX: Cell<u64> = const { Cell::new(0) };
    }
    let index = SESSION_DUMP_INDEX.with(|cell| {
        let index = cell.get();
        cell.set(index + 1);
        index
    });
    if !session_stream_dump_selected(index, dump.skip, dump.count) {
        return;
    }

    std::fs::create_dir_all(&dump.directory).unwrap_or_else(|error| {
        panic!("FN64_RAW_DPC_STREAM_DUMP_DIR {:?}: {error}", dump.directory)
    });
    let source = match submission.source() {
        fn64_render::RawDpcSource::Rdram => "rdram",
        fn64_render::RawDpcSource::XbusDmem => "xbus",
    };
    let stem = format!("raw-dpc-{index:06}-{source}");
    let stream_path = dump.directory.join(format!("{stem}.bin"));
    std::fs::write(&stream_path, raw_dpc_stream_bytes(words))
        .unwrap_or_else(|error| panic!("writing raw-DPC stream dump {stream_path:?}: {error}"));
    let metadata_path = dump.directory.join(format!("{stem}.txt"));
    let metadata = format!(
        "index={index}\nsource={source}\nstart={:#010x}\nend={:#010x}\nbytes={}\n",
        submission.start(),
        submission.end(),
        words.len() * 4
    );
    std::fs::write(&metadata_path, metadata)
        .unwrap_or_else(|error| panic!("writing raw-DPC metadata {metadata_path:?}: {error}"));
    if dump.rdram_index == Some(index) {
        let rdram_path = dump
            .directory
            .join(format!("raw-dpc-{index:06}-rdram-image.bin"));
        std::fs::write(&rdram_path, rdram)
            .unwrap_or_else(|error| panic!("writing RDRAM dump {rdram_path:?}: {error}"));
    }
}

pub(super) fn xbus_diagnostics() -> &'static XbusDiagnostics {
    static CONFIG: std::sync::OnceLock<XbusDiagnostics> = std::sync::OnceLock::new();
    CONFIG.get_or_init(|| {
        let stream_dump = crate::diag_env::diag_env("FN64_XBUS_STREAM_DUMP_DIR").map(|directory| {
            let parse_index = |name: &'static str, default: Option<u64>| {
                crate::diag_env::diag_env(name).map_or(default, |raw| {
                    Some(
                        raw.parse::<u64>()
                            .unwrap_or_else(|_| panic!("{name} must be a u64, got {raw:?}")),
                    )
                })
            };
            XbusStreamDump {
                directory: directory.into(),
                skip: parse_index("FN64_XBUS_STREAM_DUMP_SKIP", Some(0))
                    .expect("XBUS dump skip has a default"),
                rdram_index: parse_index("FN64_XBUS_STREAM_DUMP_RDRAM", None),
            }
        });
        XbusDiagnostics {
            stream_dump,
            diff_trace: crate::diag_env::diag_env_present("FN64_XBUS_DIFF_TRACE"),
        }
    })
}

pub(super) fn rsp_trace_dpc_words_limit() -> Option<usize> {
    static LIMIT: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        crate::diag_env::diag_env("RSP_TRACE_DPC_WORDS").map(|raw| {
            raw.parse::<usize>()
                .unwrap_or_else(|_| panic!("RSP_TRACE_DPC_WORDS must be decimal, got {raw:?}"))
        })
    })
}

fn rsp_dpc_task_census_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            crate::diag_env::diag_env("FN64_RSP_DPC_TASK_CENSUS").as_deref(),
            Some("1")
        )
    })
}

/// Report the natural transaction boundaries of one completed RSP task.
///
/// This is deliberately outside the per-run dispatch loop: the question the
/// census answers is whether physical DMEM-ring runs can share one renderer
/// lifetime ending at FullSync, so observing each run after it has already
/// become an independent transaction would lose the task-level grouping.
pub(super) fn maybe_report_rsp_dpc_task_shape(
    task_addr: Option<RdramAddr>,
    raw_submissions: usize,
    runs: &[CoalescedDpRun],
) {
    if !rsp_dpc_task_census_enabled() {
        return;
    }
    thread_local! {
        static TASK_INDEX: Cell<u64> = const { Cell::new(0) };
    }
    let task_index = TASK_INDEX.with(|cell| {
        let index = cell.get();
        cell.set(index.saturating_add(1));
        index
    });
    let bytes = runs
        .iter()
        .map(|run| run.words.len().saturating_mul(4))
        .sum::<usize>();
    let xbus_runs = runs.iter().filter(|run| run.xbus).count();
    let scans = runs
        .iter()
        .map(|run| {
            fn64_render::count_raw_rdp_full_sync_sites(&run.words)
                .unwrap_or_else(|error| panic!("RSP DPC task census scan rejected: {error:?}"))
        })
        .collect::<Vec<_>>();
    let full_sync_sites = scans
        .iter()
        .map(|scan| match scan {
            fn64_render::RawRdpScan::Complete(sites)
            | fn64_render::RawRdpScan::Incomplete {
                complete_prefix: sites,
                ..
            } => *sites,
        })
        .collect::<Vec<_>>();
    let incomplete_runs = scans.iter().filter(|scan| scan.is_incomplete()).count();
    let full_sync_total = full_sync_sites.iter().copied().sum::<usize>();
    let full_sync_runs = full_sync_sites.iter().filter(|sites| **sites != 0).count();
    let final_run_full_sync = full_sync_sites.last().is_some_and(|sites| *sites != 0);
    eprintln!(
        "[rsp-dpc-task-census] task={task_index} addr={task_addr:?} raw_submissions={raw_submissions} \
         runs={} xbus_runs={xbus_runs} bytes={bytes} full_sync_total={full_sync_total} \
         full_sync_runs={full_sync_runs} final_run_full_sync={final_run_full_sync} \
         incomplete_runs={incomplete_runs} \
         run_words={:?} run_full_sync_sites={full_sync_sites:?}",
        runs.len(),
        runs.iter().map(|run| run.words.len()).collect::<Vec<_>>(),
    );
}

pub(crate) fn commit_rsp_memory_state(
    dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    overlays: u64,
    execution_state: fn64_runtime::RspExecutionState,
) {
    with_host(|host| {
        let memory = host.device_fabric.rsp_memory_mut();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
                dmem,
            )
            .expect("RSP DMEM commit failed");
        for _ in 0..overlays {
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    imem,
                )
                .expect("RSP IMEM generation commit failed");
        }
        host.device_fabric
            .commit_complete_rsp_execution_state_preserving_live_dpc(execution_state)
            .unwrap_or_else(|error| panic!("RSP interpreter-state commit rejected: {error}"));
    });
}

/// One hardware command stream assembled from consecutive DPC submissions.
///
/// `start..end` is the source range the stream was fetched from, and its
/// length always equals the payload's, for both sources. Downstream capture
/// (`OwnedRawDpcSubmission::from_xbus_payload`,
/// `OwnedRawDpcSubmission::from_rdram_words`) rejects any run where it does
/// not.
pub(crate) struct CoalescedDpRun {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) xbus: bool,
    pub(crate) words: Vec<u32>,
    pub(crate) read_epoch_boundaries: Vec<CommandReadEpochBoundary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommandReadEpochBoundary {
    pub(crate) command_end_byte_offset: u32,
    pub(crate) read_epoch: fn64_audio::rsp::runtime::RspRdramReadEpoch,
    pub(crate) dp_end_step: Option<fn64_audio::rsp::runtime::RspDpEndStep>,
}

pub(super) fn validate_temporal_guest_read_route(
    before_image_count: usize,
    run_count: usize,
    session_registered: bool,
    task_batch: bool,
) {
    if before_image_count == 0 {
        return;
    }
    assert!(
        session_registered,
        "RSP DPC commands with post-END RDRAM mutations require the temporal raw-DPC session; legacy final-task RDRAM capture is not authoritative"
    );
    assert!(
        run_count == 1 || task_batch,
        "a multi-run RSP task with temporal guest reads requires transactional raw-DPC task batching so every source is captured before renderer copyback"
    );
}

/// Group consecutive DPC submissions into hardware command streams.
///
/// Consecutive END extensions against a single unmoved START are one stream:
/// F3DEX xbus 2.08 extends a run 8 bytes per END write, so a 16-byte command
/// straddles two submissions and per-submission decode would trap on a
/// truncation hardware simply stalls through (`7ef65d54`).
///
/// **Adjacency governs both sources, and for the same reason.** A submission
/// continues the open run only when its `start` equals the run's current
/// `end`; anything else is a new START and therefore a new stream. XBUS was
/// briefly exempted from that test on the theory that a DMEM-sourced range
/// means something weaker than a physical one. It does not: the producer
/// (`fn64_audio::rsp::runtime`'s CP0 `DP_END` handler) derives XBUS bytes from
/// exactly `start & 0x0fff .. end & 0x0fff`, so an XBUS range names its bytes
/// as precisely as an RDRAM range names its words. Measured on WM2000's first
/// graphics task: 365 XBUS submissions form four address-contiguous runs over
/// a DMEM ring at `[0x0ba8, 0x0f20)`, wrapping to the ring base three times.
/// Coalescing across a wrap concatenated all 3400 bytes while `start..end`
/// still described only 752 of them, and the capture correctly refused it
/// (`XbusPayloadLength { expected: 752, actual: 3400 }`). Each of the four runs
/// decodes cleanly on its own against the RDP width table
/// (`fn64_render_ir`'s `raw_rdp_command_width`), so the ring wrap is a real
/// stream boundary and not a straddle the coalescer must bridge.
pub(crate) fn coalesce_dp_submissions(
    submissions: Vec<fn64_audio::rsp::runtime::RspDpSubmission>,
) -> Vec<CoalescedDpRun> {
    let mut runs = Vec::new();
    let mut pending = submissions.into_iter().peekable();
    while let Some(first) = pending.next() {
        let first_read_epoch = first.read_epoch();
        let first_dp_end_step = first.dp_end_step();
        let (start, mut end, source) = first.into_parts();
        let mut read_epoch_boundaries = vec![CommandReadEpochBoundary {
            command_end_byte_offset: end
                .checked_sub(start)
                .expect("one DPC submission END precedes its START"),
            read_epoch: first_read_epoch,
            dp_end_step: first_dp_end_step,
        }];
        let (xbus, words) = match source {
            RspDpCommandSource::XbusBytes(mut stream) => {
                while pending
                    .peek()
                    .is_some_and(|submission| submission.is_xbus() && submission.start == end)
                {
                    let next = pending.next().expect("peeked XBUS submission disappeared");
                    let read_epoch = next.read_epoch();
                    let dp_end_step = next.dp_end_step();
                    let (_, next_end, next_source) = next.into_parts();
                    let RspDpCommandSource::XbusBytes(bytes) = next_source else {
                        unreachable!("XBUS predicate and owned command source diverged")
                    };
                    stream.extend_from_slice(&bytes);
                    end = next_end;
                    read_epoch_boundaries.push(CommandReadEpochBoundary {
                        command_end_byte_offset: end
                            .checked_sub(start)
                            .expect("coalesced XBUS END precedes its START"),
                        read_epoch,
                        dp_end_step,
                    });
                }
                let words = stream
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().expect("four XBUS bytes")))
                    .collect::<Vec<_>>();
                (true, words)
            }
            RspDpCommandSource::RdramWords(mut words) => {
                while pending
                    .peek()
                    .is_some_and(|submission| !submission.is_xbus() && submission.start == end)
                {
                    let next = pending.next().expect("peeked RDRAM submission disappeared");
                    let read_epoch = next.read_epoch();
                    let dp_end_step = next.dp_end_step();
                    let (_, next_end, next_source) = next.into_parts();
                    let RspDpCommandSource::RdramWords(next_words) = next_source else {
                        unreachable!("RDRAM predicate and owned command source diverged")
                    };
                    words.extend(next_words);
                    end = next_end;
                    read_epoch_boundaries.push(CommandReadEpochBoundary {
                        command_end_byte_offset: end
                            .checked_sub(start)
                            .expect("coalesced RDRAM END precedes its START"),
                        read_epoch,
                        dp_end_step,
                    });
                }
                (false, words)
            }
        };
        // The invariant every downstream capture depends on, checked at the
        // one site that can violate it rather than at the four that observe
        // it. A coalescing bug shows up here, naming the run, instead of as
        // a length mismatch several layers away.
        assert_eq!(
            u64::from(end).checked_sub(u64::from(start)),
            Some(words.len() as u64 * 4),
            "coalesced DPC run [{start:#010x}, {end:#010x}) (xbus={xbus}) does not describe its \
             own {} command bytes",
            words.len() * 4
        );
        runs.push(CoalescedDpRun {
            start,
            end,
            xbus,
            words,
            read_epoch_boundaries,
        });
    }
    runs
}
