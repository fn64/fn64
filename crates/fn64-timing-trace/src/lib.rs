//! Producer-neutral cycle-stamped device-event trace interchange for the differential timing
//! oracle (design spec `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`).
//!
//! This is a sibling of fn64-discover's PC trace, not an extension of it. That
//! trace carries *what code ran and what banks moved* (`executed_pc`, `pi_dma`
//! observations for discovery); this crate carries *when devices did work*
//! (cycle-stamped PI/AI/SI DMA start/complete, MI raise/ack, VI retrace) for
//! reference-emulator timing parity. The two schemas are versioned
//! independently through [`DEVICE_TRACE_SCHEMA_VERSION`].
//!
//! ## Producer-neutrality is the load-bearing constraint
//!
//! The whole point of the oracle is to compare fn64's device-event stream to
//! an INDEPENDENT reference emulator's (a C program reading a different core's
//! registers — see the design spec's `mupen_devtrace.c` component). So this
//! schema is deliberately decoupled from every fn64 internal type: a record is
//! plain wire enums and integers. PI DMA records include direction plus an
//! explicit ROM/SRAM variant and device-relative offset; other records leave
//! those PI-only fields null. There is no
//! `RdramAddr`, no `Cycles`, no `DeviceTraceKind` on the wire. A C producer
//! emits the exact same JSONL; the [`capture`] tap
//! (Rust) emits it from fn64's fabric. Neither is privileged.
//!
//! ## `wrong == 0`
//!
//! A record must faithfully represent an event the producer actually observed.
//! The fn64 tap serializes exactly what [`DeviceFabric`](fn64_runtime::device::DeviceFabric)
//! already recorded in its cycle-stamped trace log — it never fabricates a
//! cycle or an event, and it never guesses a completion cycle the fabric did
//! not stamp. Start and completion are DISTINCT records with their own cycles,
//! because the fabric stamps them at distinct CPU master cycles (a
//! `PiDmaStarted` event at the start cycle, a `PiBytesCommitted` event at the
//! completion cycle). The oracle compares those cycles; it must never see an
//! interpolated one.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::BufRead;

/// The device-timing schema evolves independently of discovery's PC trace.
pub const DEVICE_TRACE_SCHEMA_VERSION: u32 = 3;

/// A single JSONL device record is tiny, so a generous cap still fails
/// loudly on a corrupt or concatenated stream.
pub const MAX_JSONL_RECORD_BYTES: usize = 1024 * 1024;

/// Which N64 device a timing event belongs to. Serialized as a lowercase
/// string so an independent C producer can emit the same token without
/// sharing a Rust enum layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingDevice {
    Pi,
    Ai,
    Si,
    Sp,
    Dp,
    Vi,
    Mi,
}

/// Unit carried by every `cycle` field in a device-timing trace. CP0 Count is
/// deliberately not a wire unit: it advances once per two CPU master cycles
/// and is guest-writable, while device deadlines use the monotonic master
/// clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingCycleUnit {
    R4300MasterCycle,
}

/// The zero point shared by all cycle stamps in one trace. Anchoring the first
/// aligned event avoids pretending two independently launched producers began
/// observation at the same hardware instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingCycleOrigin {
    FirstEvent,
}

/// Typed clock contract for a timing stream. `quantum` is the producer's
/// timestamp resolution in master cycles: fn64 stamps exact cycles (`1`),
/// while a CP0 Count observer can resolve only even-cycle boundaries (`2`).
/// Every event stamp must be a multiple of `quantum`; its represented hardware
/// instant therefore has at most `quantum - 1` cycles of resolution
/// uncertainty. A comparator must account for both producers' uncertainty
/// rather than treating a coarse stamp as exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingTraceClock {
    pub unit: TimingCycleUnit,
    pub hz: u64,
    pub origin: TimingCycleOrigin,
    pub quantum: u32,
}

impl TimingTraceClock {
    pub const fn exact_master_cycles() -> Self {
        Self {
            unit: TimingCycleUnit::R4300MasterCycle,
            hz: fn64_runtime::CPU_CLOCK_HZ,
            origin: TimingCycleOrigin::FirstEvent,
            quantum: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingDmaDirection {
    ToRdram,
    FromRdram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingPiDevice {
    Rom,
    Sram,
}

/// The class of cycle-stamped device event. Serialized as a lowercase string,
/// producer-neutral. The payload two fields (`addr_or_source`,
/// `value_or_len`) are interpreted per-kind — see [`TimingEventKind::payload_meaning`].
///
/// This is the union of what BOTH producers can emit and that matters for
/// timing parity, drawn from fn64's
/// [`DeviceTraceKind`](fn64_runtime::device::DeviceTraceKind) but named for the
/// wire, not the Rust type:
/// - PI/AI/SI/SP DMA *start* (payload = a device/DRAM address + byte length),
/// - PI/AI/SI/SP DMA *complete* (the busy-clear / bytes-committed cycle),
/// - MI interrupt *raise* / *ack* (payload = the interrupt source bit),
/// - VI *retrace* (a field/interrupt tick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingEventKind {
    /// A DMA (PI/AI/SI/SP) began. `addr_or_source` = the primary device or
    /// DRAM byte address of the transfer; `value_or_len` = byte length.
    DmaStart,
    /// A DMA (PI/AI/SI/SP) completed and its busy flag cleared.
    /// `addr_or_source` = the same primary address; `value_or_len` = byte
    /// length. This record's `cycle` is the completion cycle the producer
    /// observed, never an interpolation.
    DmaComplete,
    /// An MI interrupt line was raised. `addr_or_source` = the `MI_INTR`
    /// source bit (`InterruptSource::bit()`); `value_or_len` = 0.
    MiRaise,
    /// An MI interrupt line was acknowledged/cleared. `addr_or_source` = the
    /// `MI_INTR` source bit; `value_or_len` = 0.
    MiAck,
    /// A VI retrace/field interrupt fired. `addr_or_source` = 0;
    /// `value_or_len` = 0. The `cycle` is the retrace cycle.
    ViRetrace,
}

impl TimingEventKind {
    /// Human-readable note on how the two payload fields are used for this
    /// kind. Documentation for a comparator author; not serialized.
    pub const fn payload_meaning(self) -> (&'static str, &'static str) {
        match self {
            Self::DmaStart | Self::DmaComplete => ("primary device/DRAM address", "byte length"),
            Self::MiRaise | Self::MiAck => ("MI_INTR source bit", "unused (0)"),
            Self::ViRetrace => ("unused (0)", "unused (0)"),
        }
    }
}

/// One line of a device-timing trace. The header must be ordinal zero and the
/// end record last; every intervening ordinal must increase by exactly one —
/// strict sequencing makes truncation, duplication, and accidental
/// concatenation fail loudly.
///
/// The `ordinal` field IS the sequence number for a `DeviceEvent` record; a
/// separate `ordinal` accessor returns it for every variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum DeviceTraceRecord {
    Header {
        ordinal: u64,
        schema_version: u32,
        clock: TimingTraceClock,
        /// Canonical, duplicate-free set of devices this producer observed.
        /// A completed subset trace makes no claim about omitted devices.
        observed_devices: Vec<TimingDevice>,
        /// Free-form producer identity, e.g. `"fn64-device-fabric"` or
        /// `"mupen-devtrace"`. Lets the comparator label divergences by side.
        producer: String,
        /// Opaque run identity so two traces of the same ROM+input can be
        /// paired without inspecting their contents.
        trace_id: String,
    },
    /// A cycle-stamped device event. This is the only payload-bearing variant;
    /// its `ordinal` is both the record's sequence position AND the event's
    /// stable ordering key (mirrors the fabric's per-cycle `sequence`).
    DeviceEvent {
        ordinal: u64,
        event_kind: TimingEventKind,
        device: TimingDevice,
        /// Relative CPU master cycle at which the event was observed. The
        /// header binds the unit, frequency, origin, and producer quantum.
        cycle: u64,
        /// Per-kind: a device/DRAM address, an MI source bit, or 0.
        addr_or_source: u32,
        /// Per-kind: a byte length, or 0.
        value_or_len: u32,
        /// Required only for PI DMA events.
        dma_direction: Option<TimingDmaDirection>,
        /// Required only for PI DMA events; distinguishes equal ROM/SRAM offsets.
        pi_device: Option<TimingPiDevice>,
        /// Required only for PI DMA events; relative to the selected device.
        pi_offset: Option<u32>,
    },
    End {
        ordinal: u64,
        completion: DeviceTraceCompletion,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTraceCompletion {
    Completed,
    Aborted,
}

impl DeviceTraceRecord {
    pub fn ordinal(&self) -> u64 {
        match self {
            Self::Header { ordinal, .. }
            | Self::DeviceEvent { ordinal, .. }
            | Self::End { ordinal, .. } => *ordinal,
        }
    }
}

/// Parsed header of a device-timing trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTraceHeader {
    pub schema_version: u32,
    pub clock: TimingTraceClock,
    pub observed_devices: Vec<TimingDevice>,
    pub producer: String,
    pub trace_id: String,
}

/// A single validated device event, lifted out of the JSONL envelope. This is
/// what a comparator aligns across the two producers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEvent {
    pub ordinal: u64,
    pub event_kind: TimingEventKind,
    pub device: TimingDevice,
    pub cycle: u64,
    pub addr_or_source: u32,
    pub value_or_len: u32,
    pub dma_direction: Option<TimingDmaDirection>,
    pub pi_device: Option<TimingPiDevice>,
    pub pi_offset: Option<u32>,
}

/// The result of ingesting one complete device-timing trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTraceIngest {
    pub header: DeviceTraceHeader,
    pub completion: DeviceTraceCompletion,
    pub final_ordinal: u64,
    pub events: Vec<DeviceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTraceIngestError {
    pub line: usize,
    pub message: String,
}

impl DeviceTraceIngestError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for DeviceTraceIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "device trace: {}", self.message)
        } else {
            write!(
                formatter,
                "device trace line {}: {}",
                self.line, self.message
            )
        }
    }
}

impl std::error::Error for DeviceTraceIngestError {}

fn validate_nonempty(line: usize, label: &str, value: &str) -> Result<(), DeviceTraceIngestError> {
    if value.trim().is_empty() {
        Err(DeviceTraceIngestError::at(
            line,
            format!("{label} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

fn validate_pi_payload(
    line: usize,
    event_kind: TimingEventKind,
    device: TimingDevice,
    dma_direction: Option<TimingDmaDirection>,
    pi_device: Option<TimingPiDevice>,
    pi_offset: Option<u32>,
) -> Result<(), DeviceTraceIngestError> {
    let is_pi_dma = device == TimingDevice::Pi
        && matches!(
            event_kind,
            TimingEventKind::DmaStart | TimingEventKind::DmaComplete
        );
    let complete = dma_direction.is_some() && pi_device.is_some() && pi_offset.is_some();
    let absent = dma_direction.is_none() && pi_device.is_none() && pi_offset.is_none();
    if (is_pi_dma && complete) || (!is_pi_dma && absent) {
        Ok(())
    } else {
        Err(DeviceTraceIngestError::at(
            line,
            "PI DMA direction/device/offset must be present together only on PI DMA events",
        ))
    }
}

/// Ingest one complete device-timing trace from JSONL. Ordinals must start at
/// zero (the header) and increase by exactly one; the stream must end with an
/// `end` record; nothing may follow it. Output ordering is input ordering, so
/// re-ingestion is byte-deterministic under `serde_json`.
pub fn ingest_jsonl<R: BufRead>(
    mut reader: R,
) -> Result<DeviceTraceIngest, DeviceTraceIngestError> {
    let mut raw = String::new();
    let mut line_number = 0usize;
    let mut header = None;
    let mut next_ordinal = 0u64;
    let mut events: Vec<DeviceEvent> = Vec::new();
    let mut end = None;

    loop {
        raw.clear();
        let bytes = reader
            .read_line(&mut raw)
            .map_err(|error| DeviceTraceIngestError::at(line_number + 1, error.to_string()))?;
        if bytes == 0 {
            break;
        }
        line_number += 1;
        if bytes > MAX_JSONL_RECORD_BYTES {
            return Err(DeviceTraceIngestError::at(
                line_number,
                format!("record exceeds {MAX_JSONL_RECORD_BYTES} bytes"),
            ));
        }
        let trimmed = raw.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Err(DeviceTraceIngestError::at(
                line_number,
                "blank records are not permitted",
            ));
        }
        if end.is_some() {
            return Err(DeviceTraceIngestError::at(
                line_number,
                "record appears after end",
            ));
        }

        let record: DeviceTraceRecord = serde_json::from_str(trimmed)
            .map_err(|error| DeviceTraceIngestError::at(line_number, error.to_string()))?;
        if record.ordinal() != next_ordinal {
            return Err(DeviceTraceIngestError::at(
                line_number,
                format!(
                    "expected ordinal {next_ordinal}, found {}",
                    record.ordinal()
                ),
            ));
        }
        next_ordinal = next_ordinal.checked_add(1).ok_or_else(|| {
            DeviceTraceIngestError::at(line_number, "ordinal overflow after u64::MAX")
        })?;

        match record {
            DeviceTraceRecord::Header {
                ordinal: _,
                schema_version,
                clock,
                observed_devices,
                producer,
                trace_id,
            } => {
                if header.is_some() || !events.is_empty() {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        "header is not first",
                    ));
                }
                if schema_version != DEVICE_TRACE_SCHEMA_VERSION {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        format!(
                            "unsupported schema version {schema_version}; expected {DEVICE_TRACE_SCHEMA_VERSION}"
                        ),
                    ));
                }
                validate_nonempty(line_number, "producer", &producer)?;
                validate_nonempty(line_number, "trace_id", &trace_id)?;
                if clock.unit != TimingCycleUnit::R4300MasterCycle
                    || clock.hz != fn64_runtime::CPU_CLOCK_HZ
                    || clock.origin != TimingCycleOrigin::FirstEvent
                    || clock.quantum == 0
                {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        format!("unsupported timing clock contract: {clock:?}"),
                    ));
                }
                if observed_devices.is_empty()
                    || observed_devices.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        "observed_devices must be nonempty, canonical, and duplicate-free",
                    ));
                }
                header = Some(DeviceTraceHeader {
                    schema_version,
                    clock,
                    observed_devices,
                    producer,
                    trace_id,
                });
            }
            other if header.is_none() => {
                let _ = other;
                return Err(DeviceTraceIngestError::at(
                    line_number,
                    "first record must be a header",
                ));
            }
            DeviceTraceRecord::DeviceEvent {
                ordinal,
                event_kind,
                device,
                cycle,
                addr_or_source,
                value_or_len,
                dma_direction,
                pi_device,
                pi_offset,
            } => {
                if !header
                    .as_ref()
                    .expect("header was validated before device events")
                    .observed_devices
                    .contains(&device)
                {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        format!(
                            "event device {device:?} is outside the declared observation scope"
                        ),
                    ));
                }
                if events.is_empty() && cycle != 0 {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        format!("first device event must define cycle zero, found {cycle}"),
                    ));
                }
                let quantum = header
                    .as_ref()
                    .expect("header was validated before device events")
                    .clock
                    .quantum;
                if cycle % u64::from(quantum) != 0 {
                    return Err(DeviceTraceIngestError::at(
                        line_number,
                        format!(
                            "device event cycle {cycle} is not aligned to producer quantum {quantum}"
                        ),
                    ));
                }
                if let Some(previous) = events.last() {
                    if cycle < previous.cycle {
                        return Err(DeviceTraceIngestError::at(
                            line_number,
                            format!(
                                "device event cycle regressed from {} to {cycle}",
                                previous.cycle
                            ),
                        ));
                    }
                }
                validate_pi_payload(
                    line_number,
                    event_kind,
                    device,
                    dma_direction,
                    pi_device,
                    pi_offset,
                )?;
                events.push(DeviceEvent {
                    ordinal,
                    event_kind,
                    device,
                    cycle,
                    addr_or_source,
                    value_or_len,
                    dma_direction,
                    pi_device,
                    pi_offset,
                });
            }
            DeviceTraceRecord::End {
                ordinal,
                completion,
            } => {
                end = Some((ordinal, completion));
            }
        }
    }

    let header = header.ok_or_else(|| DeviceTraceIngestError::at(0, "missing header"))?;
    let (final_ordinal, completion) =
        end.ok_or_else(|| DeviceTraceIngestError::at(line_number, "missing end record"))?;
    Ok(DeviceTraceIngest {
        header,
        completion,
        final_ordinal,
        events,
    })
}

/// Serialize a full record sequence to JSONL (one record per line, trailing
/// newline). The inverse of [`ingest_jsonl`]; a round-trip through both is the
/// identity on the event stream.
pub fn to_jsonl(records: &[DeviceTraceRecord]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for record in records {
        out.push_str(&serde_json::to_string(record)?);
        out.push('\n');
    }
    Ok(out)
}

fn pi_timing_payload(
    event_kind: TimingEventKind,
    request: fn64_runtime::PiDmaRequest,
) -> (
    TimingEventKind,
    TimingDevice,
    u32,
    u32,
    Option<TimingDmaDirection>,
    Option<TimingPiDevice>,
    Option<u32>,
) {
    let direction = match request.direction {
        fn64_runtime::DmaDirection::ToRdram => TimingDmaDirection::ToRdram,
        fn64_runtime::DmaDirection::FromRdram => TimingDmaDirection::FromRdram,
    };
    let (device, offset) = match request.device {
        fn64_runtime::PiDeviceAddress::RomOffset(offset) => (TimingPiDevice::Rom, offset),
        fn64_runtime::PiDeviceAddress::SramOffset(offset) => (TimingPiDevice::Sram, offset),
    };
    (
        event_kind,
        TimingDevice::Pi,
        request.dram_addr.offset(),
        request.len,
        Some(direction),
        Some(device),
        Some(offset),
    )
}

/// The fn64-side capture tap: turn one run of a
/// [`DeviceFabric`](fn64_runtime::device::DeviceFabric) into the
/// producer-neutral device-timing schema above.
///
/// This is the counterpart of the (later increment) C `mupen_devtrace.c`
/// producer: both emit the SAME [`DeviceTraceRecord`] JSONL, so the comparator
/// aligns them without knowing which core produced which stream.
///
/// ## It consumes what the fabric already recorded — no new instrumentation
///
/// `DeviceFabric` already IS a cycle-stamped event log: every state
/// transition calls its private `record()`, which pushes a
/// [`DeviceTraceEvent`](fn64_runtime::device::DeviceTraceEvent) carrying the
/// guest cycle (`at`), a monotonic `sequence`, and a
/// [`DeviceTraceKind`](fn64_runtime::device::DeviceTraceKind). The tap reads
/// that log verbatim via [`DeviceFabric::trace`](fn64_runtime::device::DeviceFabric::trace)
/// — it adds ZERO instrumentation to the fabric's hot path and never invents a
/// cycle. The event's cycle IS the fabric's stamp; the event's `ordinal` IS
/// the fabric's `sequence`.
///
/// ## What it keeps vs. drops
///
/// It keeps the events that matter for timing parity and that an independent
/// register-reading producer can also observe: DMA start/complete for PI, AI,
/// SI, and SP; direct PIF control projected through the shared SI busy edges;
/// MI raise/ack; VI retrace. It drops fabric-internal bookkeeping
/// a foreign core does not expose the same way — `NotificationReady` (an
/// fn64 OS-work signal, redundant with the completion it echoes), the
/// `RcpTask*` and `SpTaskAdmitted` records (RSP scheduling, out of this
/// increment's device-timing scope), and the intermediate `*BusyCleared`
/// records (the same completion cycle as the bytes-committed record they
/// follow; the completion payload lives on the bytes-committed variant).
///
/// The returned `Vec` is a complete, valid record stream (header +
/// device-events + end) ready for [`to_jsonl`] / [`ingest_jsonl`]. Events
/// before `capture_start` are outside the requested window and are omitted;
/// the first retained wire event defines relative cycle zero.
pub fn capture(
    fabric_trace: &[fn64_runtime::device::DeviceTraceEvent],
    capture_start: fn64_runtime::Cycles,
    observed_devices: &[TimingDevice],
    producer: impl Into<String>,
    trace_id: impl Into<String>,
    completion: DeviceTraceCompletion,
) -> Vec<DeviceTraceRecord> {
    use fn64_runtime::device::{DeviceTraceKind, InterruptSource};

    // Local: the fabric event's own `sequence` is the natural ordinal, but the
    // envelope requires a gap-free 0,1,2,... run (header is 0, end is last),
    // and the tap drops some fabric records — so we re-number densely here.
    let mut records = Vec::with_capacity(fabric_trace.len() + 2);
    let mut ordinal = 0u64;

    assert!(
        !observed_devices.is_empty() && !observed_devices.windows(2).any(|pair| pair[0] >= pair[1]),
        "device timing capture scope must be nonempty, canonical, and duplicate-free"
    );
    records.push(DeviceTraceRecord::Header {
        ordinal,
        schema_version: DEVICE_TRACE_SCHEMA_VERSION,
        clock: TimingTraceClock::exact_master_cycles(),
        observed_devices: observed_devices.to_vec(),
        producer: producer.into(),
        trace_id: trace_id.into(),
    });
    ordinal += 1;

    let source_bit = |source: InterruptSource| -> u32 { source.bit() };

    let mut first_event_at = None;
    for event in fabric_trace {
        let Some(absolute_cycle) = event.at.get().checked_sub(capture_start.get()) else {
            continue;
        };
        // (event kind, device, address, length, PI direction/device/offset)
        let mapped: Option<(
            TimingEventKind,
            TimingDevice,
            u32,
            u32,
            Option<TimingDmaDirection>,
            Option<TimingPiDevice>,
            Option<u32>,
        )> = match event.kind {
            DeviceTraceKind::PiDmaStarted(request) => {
                Some(pi_timing_payload(TimingEventKind::DmaStart, request))
            }
            // Bytes-committed carries the request payload and is stamped at the
            // completion cycle; the following PiBusyCleared shares that cycle
            // and is dropped as redundant.
            DeviceTraceKind::PiBytesCommitted(request) => {
                Some(pi_timing_payload(TimingEventKind::DmaComplete, request))
            }
            DeviceTraceKind::PiBusyCleared => None,
            DeviceTraceKind::AiDmaStarted(request) => Some((
                TimingEventKind::DmaStart,
                TimingDevice::Ai,
                request.dram_addr.offset(),
                request.len,
                None,
                None,
                None,
            )),
            DeviceTraceKind::AiDmaComplete(request) => Some((
                TimingEventKind::DmaComplete,
                TimingDevice::Ai,
                request.dram_addr.offset(),
                request.len,
                None,
                None,
                None,
            )),
            DeviceTraceKind::SiDmaStarted(request) => Some((
                TimingEventKind::DmaStart,
                TimingDevice::Si,
                request.dram_addr.offset(),
                // SI transfers are the fixed 64-byte PIF window; length is not
                // carried per-request, so 0 marks "SI's fixed window".
                0,
                None,
                None,
                None,
            )),
            DeviceTraceKind::SiBytesCommitted(request) => Some((
                TimingEventKind::DmaComplete,
                TimingDevice::Si,
                request.dram_addr.offset(),
                0,
                None,
                None,
                None,
            )),
            // The v3 black-box producer observes the shared SI busy edge and
            // therefore spells a direct PIF control transaction as an SI DMA
            // with no RDRAM address/length. Preserve that wire shape while the
            // runtime retains the truthful semantic distinction internally.
            DeviceTraceKind::PifControlStarted(_) => Some((
                TimingEventKind::DmaStart,
                TimingDevice::Si,
                0,
                0,
                None,
                None,
                None,
            )),
            DeviceTraceKind::PifControlComplete(_) => Some((
                TimingEventKind::DmaComplete,
                TimingDevice::Si,
                0,
                0,
                None,
                None,
                None,
            )),
            DeviceTraceKind::SiBusyCleared => None,
            DeviceTraceKind::SpDmaStarted(request) => Some((
                TimingEventKind::DmaStart,
                TimingDevice::Sp,
                request.dram_addr.offset(),
                request.total_bytes() as u32,
                None,
                None,
                None,
            )),
            DeviceTraceKind::SpDmaBytesCommitted(request) => Some((
                TimingEventKind::DmaComplete,
                TimingDevice::Sp,
                request.dram_addr.offset(),
                request.total_bytes() as u32,
                None,
                None,
                None,
            )),
            // SP DMA queue admission and busy-clear are scheduling bookkeeping,
            // not a distinct timed transfer edge.
            DeviceTraceKind::SpDmaQueued(_) | DeviceTraceKind::SpDmaBusyCleared => None,
            DeviceTraceKind::MiInterruptRaised(source) => Some((
                TimingEventKind::MiRaise,
                TimingDevice::Mi,
                source_bit(source),
                0,
                None,
                None,
                None,
            )),
            DeviceTraceKind::MiInterruptCleared(source) => Some((
                TimingEventKind::MiAck,
                TimingDevice::Mi,
                source_bit(source),
                0,
                None,
                None,
                None,
            )),
            DeviceTraceKind::ViInterrupt => Some((
                TimingEventKind::ViRetrace,
                TimingDevice::Vi,
                0,
                0,
                None,
                None,
                None,
            )),
            // Out-of-scope-for-this-increment fabric bookkeeping.
            DeviceTraceKind::SpTaskAdmitted { .. }
            | DeviceTraceKind::RcpTaskStarted { .. }
            | DeviceTraceKind::RcpTaskComplete(_)
            | DeviceTraceKind::NotificationReady(_) => None,
        };

        if let Some((
            event_kind,
            device,
            addr_or_source,
            value_or_len,
            dma_direction,
            pi_device,
            pi_offset,
        )) = mapped
        {
            if !observed_devices.contains(&device) {
                continue;
            }
            let origin = *first_event_at.get_or_insert(absolute_cycle);
            records.push(DeviceTraceRecord::DeviceEvent {
                ordinal,
                event_kind,
                device,
                cycle: absolute_cycle - origin,
                addr_or_source,
                value_or_len,
                dma_direction,
                pi_device,
                pi_offset,
            });
            ordinal += 1;
        }
    }

    records.push(DeviceTraceRecord::End {
        ordinal,
        completion,
    });
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header() -> DeviceTraceRecord {
        DeviceTraceRecord::Header {
            ordinal: 0,
            schema_version: DEVICE_TRACE_SCHEMA_VERSION,
            clock: TimingTraceClock::exact_master_cycles(),
            observed_devices: vec![
                TimingDevice::Pi,
                TimingDevice::Ai,
                TimingDevice::Si,
                TimingDevice::Sp,
                TimingDevice::Vi,
                TimingDevice::Mi,
            ],
            producer: "synthetic-test".to_string(),
            trace_id: "device-1".to_string(),
        }
    }

    #[test]
    fn jsonl_round_trip_is_the_identity_on_the_event_stream() {
        let records = vec![
            header(),
            DeviceTraceRecord::DeviceEvent {
                ordinal: 1,
                event_kind: TimingEventKind::DmaStart,
                device: TimingDevice::Pi,
                cycle: 0,
                addr_or_source: 0x20,
                value_or_len: 64,
                dma_direction: Some(TimingDmaDirection::ToRdram),
                pi_device: Some(TimingPiDevice::Rom),
                pi_offset: Some(0x10),
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 2,
                event_kind: TimingEventKind::DmaComplete,
                device: TimingDevice::Pi,
                cycle: 12,
                addr_or_source: 0x20,
                value_or_len: 64,
                dma_direction: Some(TimingDmaDirection::ToRdram),
                pi_device: Some(TimingPiDevice::Rom),
                pi_offset: Some(0x10),
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 3,
                event_kind: TimingEventKind::MiRaise,
                device: TimingDevice::Mi,
                cycle: 12,
                addr_or_source: 1 << 4, // MI_INTR PI source bit
                value_or_len: 0,
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
            DeviceTraceRecord::End {
                ordinal: 4,
                completion: DeviceTraceCompletion::Completed,
            },
        ];

        let jsonl = to_jsonl(&records).unwrap();
        let ingest = ingest_jsonl(Cursor::new(&jsonl)).unwrap();

        assert_eq!(ingest.header.producer, "synthetic-test");
        assert_eq!(ingest.header.trace_id, "device-1");
        assert_eq!(ingest.completion, DeviceTraceCompletion::Completed);
        assert_eq!(ingest.final_ordinal, 4);
        assert_eq!(ingest.events.len(), 3);
        assert_eq!(ingest.events[0].event_kind, TimingEventKind::DmaStart);
        assert_eq!(ingest.events[0].cycle, 0);
        assert_eq!(ingest.events[1].cycle, 12);
        assert_eq!(ingest.events[2].device, TimingDevice::Mi);

        // Re-serialization of the ingest is byte-deterministic.
        let first = serde_json::to_string(&ingest).unwrap();
        let second = serde_json::to_string(&ingest_jsonl(Cursor::new(&jsonl)).unwrap()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_ambiguous_pre_v3_and_invalid_clock_contracts() {
        let mut pre_v3 = header();
        if let DeviceTraceRecord::Header { schema_version, .. } = &mut pre_v3 {
            *schema_version = 2;
        }
        let error = ingest_jsonl(Cursor::new(
            to_jsonl(&[
                pre_v3,
                DeviceTraceRecord::End {
                    ordinal: 1,
                    completion: DeviceTraceCompletion::Completed,
                },
            ])
            .unwrap(),
        ))
        .unwrap_err();
        assert!(error.message.contains("unsupported schema version 2"));

        let mut zero_quantum = header();
        if let DeviceTraceRecord::Header { clock, .. } = &mut zero_quantum {
            clock.quantum = 0;
        }
        let error = ingest_jsonl(Cursor::new(
            to_jsonl(&[
                zero_quantum,
                DeviceTraceRecord::End {
                    ordinal: 1,
                    completion: DeviceTraceCompletion::Completed,
                },
            ])
            .unwrap(),
        ))
        .unwrap_err();
        assert!(error.message.contains("unsupported timing clock contract"));
    }

    #[test]
    fn first_event_defines_zero_and_cycles_never_regress() {
        let event = |ordinal, cycle| DeviceTraceRecord::DeviceEvent {
            ordinal,
            event_kind: TimingEventKind::ViRetrace,
            device: TimingDevice::Vi,
            cycle,
            addr_or_source: 0,
            value_or_len: 0,
            dma_direction: None,
            pi_device: None,
            pi_offset: None,
        };
        let nonzero = to_jsonl(&[
            header(),
            event(1, 1),
            DeviceTraceRecord::End {
                ordinal: 2,
                completion: DeviceTraceCompletion::Completed,
            },
        ])
        .unwrap();
        let error = ingest_jsonl(Cursor::new(nonzero)).unwrap_err();
        assert!(error.message.contains("must define cycle zero"));

        let regressing = to_jsonl(&[
            header(),
            event(1, 0),
            event(2, 12),
            event(3, 11),
            DeviceTraceRecord::End {
                ordinal: 4,
                completion: DeviceTraceCompletion::Completed,
            },
        ])
        .unwrap();
        let error = ingest_jsonl(Cursor::new(regressing)).unwrap_err();
        assert!(error.message.contains("cycle regressed"));
    }

    #[test]
    fn event_cycles_must_lie_on_the_declared_quantum() {
        let mut quantized = header();
        if let DeviceTraceRecord::Header { clock, .. } = &mut quantized {
            clock.quantum = 2;
        }
        let error = ingest_jsonl(Cursor::new(
            to_jsonl(&[
                quantized,
                DeviceTraceRecord::DeviceEvent {
                    ordinal: 1,
                    event_kind: TimingEventKind::ViRetrace,
                    device: TimingDevice::Vi,
                    cycle: 0,
                    addr_or_source: 0,
                    value_or_len: 0,
                    dma_direction: None,
                    pi_device: None,
                    pi_offset: None,
                },
                DeviceTraceRecord::DeviceEvent {
                    ordinal: 2,
                    event_kind: TimingEventKind::ViRetrace,
                    device: TimingDevice::Vi,
                    cycle: 3,
                    addr_or_source: 0,
                    value_or_len: 0,
                    dma_direction: None,
                    pi_device: None,
                    pi_offset: None,
                },
                DeviceTraceRecord::End {
                    ordinal: 3,
                    completion: DeviceTraceCompletion::Completed,
                },
            ])
            .unwrap(),
        ))
        .unwrap_err();
        assert!(error.message.contains("not aligned to producer quantum 2"));
    }

    #[test]
    fn rejects_noncanonical_scopes_and_out_of_scope_events() {
        let mut duplicate = header();
        if let DeviceTraceRecord::Header {
            observed_devices, ..
        } = &mut duplicate
        {
            *observed_devices = vec![TimingDevice::Vi, TimingDevice::Vi];
        }
        let error = ingest_jsonl(Cursor::new(
            to_jsonl(&[
                duplicate,
                DeviceTraceRecord::End {
                    ordinal: 1,
                    completion: DeviceTraceCompletion::Completed,
                },
            ])
            .unwrap(),
        ))
        .unwrap_err();
        assert!(error.message.contains("canonical"));

        let mut vi_only = header();
        if let DeviceTraceRecord::Header {
            observed_devices, ..
        } = &mut vi_only
        {
            *observed_devices = vec![TimingDevice::Vi];
        }
        let error = ingest_jsonl(Cursor::new(
            to_jsonl(&[
                vi_only,
                DeviceTraceRecord::DeviceEvent {
                    ordinal: 1,
                    event_kind: TimingEventKind::DmaStart,
                    device: TimingDevice::Pi,
                    cycle: 0,
                    addr_or_source: 0,
                    value_or_len: 1,
                    dma_direction: Some(TimingDmaDirection::ToRdram),
                    pi_device: Some(TimingPiDevice::Rom),
                    pi_offset: Some(0),
                },
                DeviceTraceRecord::End {
                    ordinal: 2,
                    completion: DeviceTraceCompletion::Completed,
                },
            ])
            .unwrap(),
        ))
        .unwrap_err();
        assert!(error
            .message
            .contains("outside the declared observation scope"));
    }

    #[test]
    fn schema_v3_distinguishes_equal_pi_offsets_and_rejects_partial_payloads() {
        let records = |pi_device| {
            vec![
                header(),
                DeviceTraceRecord::DeviceEvent {
                    ordinal: 1,
                    event_kind: TimingEventKind::DmaStart,
                    device: TimingDevice::Pi,
                    cycle: 0,
                    addr_or_source: 0x20,
                    value_or_len: 4,
                    dma_direction: Some(TimingDmaDirection::ToRdram),
                    pi_device,
                    pi_offset: Some(0x10),
                },
                DeviceTraceRecord::End {
                    ordinal: 2,
                    completion: DeviceTraceCompletion::Completed,
                },
            ]
        };
        let rom = to_jsonl(&records(Some(TimingPiDevice::Rom))).unwrap();
        let sram = to_jsonl(&records(Some(TimingPiDevice::Sram))).unwrap();
        assert_ne!(rom, sram);
        assert_ne!(
            ingest_jsonl(Cursor::new(rom)).unwrap().events,
            ingest_jsonl(Cursor::new(sram)).unwrap().events
        );

        let partial = to_jsonl(&records(None)).unwrap();
        let error = ingest_jsonl(Cursor::new(partial)).unwrap_err();
        assert!(error.message.contains("must be present together"));
    }

    #[test]
    fn rejects_ordinal_gaps_and_records_after_end() {
        let gap = to_jsonl(&[
            header(),
            DeviceTraceRecord::DeviceEvent {
                ordinal: 2,
                event_kind: TimingEventKind::ViRetrace,
                device: TimingDevice::Vi,
                cycle: 5,
                addr_or_source: 0,
                value_or_len: 0,
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
        ])
        .unwrap();
        let err = ingest_jsonl(Cursor::new(gap)).unwrap_err();
        assert!(err.message.contains("expected ordinal 1"), "{}", err);

        let after_end = to_jsonl(&[
            header(),
            DeviceTraceRecord::End {
                ordinal: 1,
                completion: DeviceTraceCompletion::Completed,
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 2,
                event_kind: TimingEventKind::ViRetrace,
                device: TimingDevice::Vi,
                cycle: 5,
                addr_or_source: 0,
                value_or_len: 0,
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
        ])
        .unwrap();
        let err = ingest_jsonl(Cursor::new(after_end)).unwrap_err();
        assert!(err.message.contains("after end"), "{}", err);
    }

    #[test]
    fn rejects_missing_header_and_missing_end() {
        let no_header = to_jsonl(&[DeviceTraceRecord::DeviceEvent {
            ordinal: 0,
            event_kind: TimingEventKind::ViRetrace,
            device: TimingDevice::Vi,
            cycle: 1,
            addr_or_source: 0,
            value_or_len: 0,
            dma_direction: None,
            pi_device: None,
            pi_offset: None,
        }])
        .unwrap();
        let err = ingest_jsonl(Cursor::new(no_header)).unwrap_err();
        assert!(
            err.message.contains("first record must be a header"),
            "{}",
            err
        );

        let no_end = to_jsonl(&[header()]).unwrap();
        let err = ingest_jsonl(Cursor::new(no_end)).unwrap_err();
        assert!(err.message.contains("missing end"), "{}", err);
    }

    // --- The fabric capture tap, driven through a real DeviceFabric. ---

    mod fabric_tap {
        use super::super::*;
        use fn64_runtime::device::{DeviceFabric, FixedPiTiming, PiDmaRequest};
        use fn64_runtime::{
            Cycles, DmaDirection, InMemoryRom, InterruptSource, PiDma, Rdram, RdramAddr,
        };
        use std::io::Cursor;

        /// A fabric whose PI transfers always takes 12 CPU master cycles, so
        /// the completion cycle is a known constant the test can assert.
        fn fabric() -> DeviceFabric<InMemoryRom, FixedPiTiming> {
            // 0x100 bytes of cartridge, a recognizable word at 0x10.
            let mut rom = vec![0u8; 0x100];
            rom[0x10..0x14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
            DeviceFabric::new(
                PiDma::new(InMemoryRom::new(rom)),
                FixedPiTiming(Cycles::new(12)),
            )
        }

        #[test]
        fn tap_emits_the_real_cycle_stamped_pi_dma_and_mi_stream() {
            let mut fabric = fabric();
            let mut rdram = Rdram::new(0x100);

            fabric
                .advance_to(fn64_runtime::EmulatedInstant::new(100), &mut rdram)
                .unwrap();

            // Drive one PI DMA (cart 0x10 -> DRAM 0x20, 4 bytes). start_pi_dma
            // stamps PiDmaStarted at absolute cycle 100; advancing to 112 fires
            // the completion. The timing wire projects both relative to the
            // typed capture window below; the first event is the wire origin,
            // yielding cycles 0 and 12.
            let request = PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                device: fn64_runtime::PiDeviceAddress::RomOffset(0x10),
                len: 4,
            };
            fabric.start_pi_dma(request).unwrap();
            fabric
                .advance_to(fn64_runtime::EmulatedInstant::new(112), &mut rdram)
                .unwrap();

            // Also raise (and then ack) an independent MI source at cycle 12,
            // exercising both MI record kinds from a source other than PI.
            fabric.raise_interrupt(InterruptSource::Vi);
            fabric.clear_interrupt(InterruptSource::Vi);

            let records = capture(
                fabric.trace(),
                Cycles::new(100),
                &[
                    TimingDevice::Pi,
                    TimingDevice::Ai,
                    TimingDevice::Si,
                    TimingDevice::Sp,
                    TimingDevice::Vi,
                    TimingDevice::Mi,
                ],
                "fn64-device-fabric",
                "pi-dma-smoke",
                DeviceTraceCompletion::Completed,
            );

            // Round-trip proves the emitted records are a valid, gap-free,
            // header/events/end stream.
            let jsonl = to_jsonl(&records).unwrap();
            let ingest = ingest_jsonl(Cursor::new(&jsonl)).unwrap();
            assert_eq!(ingest.header.producer, "fn64-device-fabric");
            assert_eq!(ingest.completion, DeviceTraceCompletion::Completed);

            // The exact expected cycle-stamped device-event stream. This is
            // the whole point: the tap must reproduce the fabric's real
            // ordering and cycles, never fabricate.
            type EventTuple = (
                TimingEventKind,
                TimingDevice,
                u64,
                u32,
                u32,
                Option<TimingDmaDirection>,
                Option<TimingPiDevice>,
                Option<u32>,
            );
            let expected: Vec<EventTuple> = vec![
                // PI DMA started at cycle 0, DRAM 0x20, 4 bytes.
                (
                    TimingEventKind::DmaStart,
                    TimingDevice::Pi,
                    0,
                    0x20,
                    4,
                    Some(TimingDmaDirection::ToRdram),
                    Some(TimingPiDevice::Rom),
                    Some(0x10),
                ),
                // PI DMA completed at cycle 12 (start + fixed 12-cycle latency).
                (
                    TimingEventKind::DmaComplete,
                    TimingDevice::Pi,
                    12,
                    0x20,
                    4,
                    Some(TimingDmaDirection::ToRdram),
                    Some(TimingPiDevice::Rom),
                    Some(0x10),
                ),
                // The PI completion raised the PI MI line, at the same cycle 12.
                (
                    TimingEventKind::MiRaise,
                    TimingDevice::Mi,
                    12,
                    InterruptSource::Pi.bit(),
                    0,
                    None,
                    None,
                    None,
                ),
                // The independent VI raise/ack, both at cycle 12.
                (
                    TimingEventKind::MiRaise,
                    TimingDevice::Mi,
                    12,
                    InterruptSource::Vi.bit(),
                    0,
                    None,
                    None,
                    None,
                ),
                (
                    TimingEventKind::MiAck,
                    TimingDevice::Mi,
                    12,
                    InterruptSource::Vi.bit(),
                    0,
                    None,
                    None,
                    None,
                ),
            ];

            let actual: Vec<EventTuple> = ingest
                .events
                .iter()
                .map(|e| {
                    (
                        e.event_kind,
                        e.device,
                        e.cycle,
                        e.addr_or_source,
                        e.value_or_len,
                        e.dma_direction,
                        e.pi_device,
                        e.pi_offset,
                    )
                })
                .collect();

            assert_eq!(
                actual, expected,
                "tap must reproduce the fabric's exact cycle-stamped device-event stream"
            );

            // Ordinals are dense and gap-free: header 0, five events 1..=5, end 6.
            assert_eq!(ingest.final_ordinal, 6);
            let ordinals: Vec<u64> = ingest.events.iter().map(|e| e.ordinal).collect();
            assert_eq!(ordinals, vec![1, 2, 3, 4, 5]);
        }

        #[test]
        fn tap_maps_direct_pif_control_to_the_reference_si_busy_wire_shape() {
            let mut fabric = fabric();
            let mut rdram = Rdram::new(0);
            fabric.set_pif_control_latency(Cycles::new(5));
            assert!(fabric.advance_clock_if_idle(fn64_runtime::EmulatedInstant::new(100)));
            fabric.pif_ram_cpu_write_w(60, 0x08).unwrap();
            fabric
                .advance_to(fn64_runtime::EmulatedInstant::new(105), &mut rdram)
                .unwrap();

            let records = capture(
                fabric.trace(),
                Cycles::new(100),
                &[
                    TimingDevice::Pi,
                    TimingDevice::Ai,
                    TimingDevice::Si,
                    TimingDevice::Sp,
                    TimingDevice::Vi,
                    TimingDevice::Mi,
                ],
                "fn64-device-fabric",
                "direct-pif-smoke",
                DeviceTraceCompletion::Completed,
            );
            let ingest = ingest_jsonl(Cursor::new(to_jsonl(&records).unwrap())).unwrap();
            assert_eq!(
                ingest
                    .events
                    .iter()
                    .map(|event| (
                        event.event_kind,
                        event.device,
                        event.cycle,
                        event.addr_or_source,
                        event.value_or_len,
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (TimingEventKind::DmaStart, TimingDevice::Si, 0, 0, 0),
                    (TimingEventKind::DmaComplete, TimingDevice::Si, 5, 0, 0),
                    (
                        TimingEventKind::MiRaise,
                        TimingDevice::Mi,
                        5,
                        InterruptSource::Si.bit(),
                        0,
                    ),
                ]
            );
        }

        #[test]
        fn tap_is_deterministic_across_repeated_identical_runs() {
            let run = || {
                let mut fabric = fabric();
                let mut rdram = Rdram::new(0x100);
                let request = PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: RdramAddr::from_offset(0x20),
                    device: fn64_runtime::PiDeviceAddress::RomOffset(0x10),
                    len: 4,
                };
                fabric.start_pi_dma(request).unwrap();
                fabric
                    .advance_to(fn64_runtime::EmulatedInstant::new(12), &mut rdram)
                    .unwrap();
                let records = capture(
                    fabric.trace(),
                    Cycles::ZERO,
                    &[
                        TimingDevice::Pi,
                        TimingDevice::Ai,
                        TimingDevice::Si,
                        TimingDevice::Sp,
                        TimingDevice::Vi,
                        TimingDevice::Mi,
                    ],
                    "fn64-device-fabric",
                    "determinism",
                    DeviceTraceCompletion::Completed,
                );
                to_jsonl(&records).unwrap()
            };
            assert_eq!(run(), run());
        }
    }
}
