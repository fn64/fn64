//! Black-box headless-emulator bridge for dynamic discovery.
//!
//! fn64 exports a content-bound run bundle from a validated [`ProbePlan`],
//! then accepts a deliberately small JSONL observation stream from an
//! out-of-process wrapper. The wrapper names the probe that admitted every
//! event; this module checks the event against that probe before translating
//! it into the canonical trace schema. Emulator-specific commands, device
//! tags, and textual log parsing stay outside generic discovery.
//!
//! This is an observation boundary, not an emulator behavior model. In
//! particular, the PI event is named `PiDmaCompleted`: register writes or a
//! DMA-start notification are not sufficient input for that record.

use crate::probe::{AddressRange, BankScope, Probe, ProbeTarget, ValidatedProbePlan};
use crate::trace::{
    BankContext, IndirectTransferKind, ObservedAddress, PiDmaDirection, TraceCompletion,
    TraceRecord, WatchedValueWidth, MAX_JSONL_RECORD_BYTES, TRACE_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{BufRead, Cursor, Write};

pub const HEADLESS_BRIDGE_SCHEMA_VERSION: u32 = 1;

/// SHA-256 identity for an external executable, settings snapshot, or other
/// non-ROM artifact. This is intentionally distinct from
/// [`NormalizedRomDigest`], so an artifact hash cannot be passed where the
/// normalized game identity is required.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeadlessArtifactDigest(String);

impl TryFrom<String> for HeadlessArtifactDigest {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64 {
            return Err("artifact SHA-256 must contain exactly 64 hex digits");
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("artifact SHA-256 must use lowercase hexadecimal");
        }
        Ok(Self(value))
    }
}

impl Serialize for HeadlessArtifactDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessProducerIdentity {
    pub adapter_id: String,
    pub adapter_version: String,
    pub emulator: String,
    pub emulator_version: String,
    pub executable_sha256: HeadlessArtifactDigest,
    pub settings_sha256: HeadlessArtifactDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessResetKind {
    PowerOn,
    HardReset,
    SoftReset,
    StateRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessRegion {
    Ntsc,
    Pal,
    Mpal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessLaunchIdentity {
    pub reset: HeadlessResetKind,
    pub region: HeadlessRegion,
    pub initial_state_sha256: Option<HeadlessArtifactDigest>,
}

/// The exact, deterministic input an emulator-specific wrapper consumes.
/// ROM and input-timeline paths are intentionally absent; callers supply
/// those out of tree and verify them against the included digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessRunBundle<'a> {
    pub schema_version: u32,
    pub trace_schema_version: u32,
    pub trace_id: &'a str,
    pub producer: &'a HeadlessProducerIdentity,
    pub launch: &'a HeadlessLaunchIdentity,
    pub probe_plan: &'a ValidatedProbePlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedHeadlessRun<'a> {
    bundle: HeadlessRunBundle<'a>,
    bundle_sha256: String,
}

impl<'a> PreparedHeadlessRun<'a> {
    pub fn bundle(&self) -> &HeadlessRunBundle<'a> {
        &self.bundle
    }

    pub fn bundle_sha256(&self) -> &str {
        &self.bundle_sha256
    }

    pub fn write_json<W: Write>(&self, mut writer: W) -> Result<(), HeadlessBridgeError> {
        serde_json::to_writer(&mut writer, &self.bundle)
            .map_err(|error| HeadlessBridgeError::new(0, error.to_string()))?;
        writer
            .write_all(b"\n")
            .map_err(|error| HeadlessBridgeError::new(0, error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessBudgetKind {
    Instructions,
    Events,
    EmulatedTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum HeadlessStopReason {
    ScenarioComplete,
    BudgetReached { budget: HeadlessBudgetKind },
    ProducerAbort { detail: String },
}

/// JSONL emitted by an emulator-specific black-box wrapper.
///
/// Sequence zero is the header. Event sequence numbers are preserved in the
/// canonical trace so a raw log and its normalized form can be compared
/// without an implicit reorder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum HeadlessObservationRecord {
    Header {
        sequence: u64,
        schema_version: u32,
        trace_id: String,
        run_bundle_sha256: String,
    },
    ExecutedPc {
        sequence: u64,
        probe_id: String,
        pc: ObservedAddress,
    },
    IndirectTransfer {
        sequence: u64,
        probe_id: String,
        kind: IndirectTransferKind,
        site: ObservedAddress,
        target: ObservedAddress,
    },
    PiDmaCompleted {
        sequence: u64,
        probe_id: String,
        direction: PiDmaDirection,
        cart_address: u32,
        dram_address: u32,
        byte_len: u32,
        active_bank: BankContext,
    },
    WatchedTableWrite {
        sequence: u64,
        probe_id: String,
        watch_id: String,
        address: u32,
        width: WatchedValueWidth,
        value: u64,
        active_bank: BankContext,
    },
    End {
        sequence: u64,
        stop_reason: HeadlessStopReason,
        instructions_executed: u64,
        emulated_time_ns: u64,
    },
}

impl HeadlessObservationRecord {
    fn sequence(&self) -> u64 {
        match self {
            Self::Header { sequence, .. }
            | Self::ExecutedPc { sequence, .. }
            | Self::IndirectTransfer { sequence, .. }
            | Self::PiDmaCompleted { sequence, .. }
            | Self::WatchedTableWrite { sequence, .. }
            | Self::End { sequence, .. } => *sequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedHeadlessTrace {
    pub records: Vec<TraceRecord>,
}

impl NormalizedHeadlessTrace {
    pub fn write_jsonl<W: Write>(&self, mut writer: W) -> Result<(), HeadlessBridgeError> {
        for record in &self.records {
            serde_json::to_writer(&mut writer, record)
                .map_err(|error| HeadlessBridgeError::new(0, error.to_string()))?;
            writer
                .write_all(b"\n")
                .map_err(|error| HeadlessBridgeError::new(0, error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessBridgeError {
    pub line: usize,
    pub message: String,
}

impl HeadlessBridgeError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for HeadlessBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "headless bridge: {}", self.message)
        } else {
            write!(
                formatter,
                "headless bridge line {}: {}",
                self.line, self.message
            )
        }
    }
}

impl std::error::Error for HeadlessBridgeError {}

fn validate_identifier(label: &str, value: &str) -> Result<(), HeadlessBridgeError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(HeadlessBridgeError::new(
            0,
            format!("{label} must be nonempty canonical text"),
        ));
    }
    Ok(())
}

pub fn prepare_headless_run<'a>(
    trace_id: &'a str,
    producer: &'a HeadlessProducerIdentity,
    launch: &'a HeadlessLaunchIdentity,
    probe_plan: &'a ValidatedProbePlan,
) -> Result<PreparedHeadlessRun<'a>, HeadlessBridgeError> {
    validate_identifier("trace_id", trace_id)?;
    validate_identifier("adapter_id", &producer.adapter_id)?;
    validate_identifier("adapter_version", &producer.adapter_version)?;
    validate_identifier("emulator", &producer.emulator)?;
    validate_identifier("emulator_version", &producer.emulator_version)?;
    if matches!(launch.reset, HeadlessResetKind::StateRestore)
        != launch.initial_state_sha256.is_some()
    {
        return Err(HeadlessBridgeError::new(
            0,
            "state_restore requires exactly one initial-state digest, and other resets forbid it",
        ));
    }

    let bundle = HeadlessRunBundle {
        schema_version: HEADLESS_BRIDGE_SCHEMA_VERSION,
        trace_schema_version: TRACE_SCHEMA_VERSION,
        trace_id,
        producer,
        launch,
        probe_plan,
    };
    let canonical = serde_json::to_vec(&bundle)
        .map_err(|error| HeadlessBridgeError::new(0, error.to_string()))?;
    let bundle_sha256 = format!("{:x}", Sha256::digest(canonical));
    Ok(PreparedHeadlessRun {
        bundle,
        bundle_sha256,
    })
}

fn bank_matches(scope: &BankScope, observed: &BankContext) -> bool {
    match (scope, observed) {
        (BankScope::Any, _) => true,
        (BankScope::Known { bank: expected }, BankContext::Known { bank, .. }) => expected == bank,
        (BankScope::Known { .. }, BankContext::Unknown) => false,
    }
}

fn range_contains(range: AddressRange, start: u32, byte_len: u32) -> bool {
    byte_len > 0
        && start >= range.start
        && start
            .checked_add(byte_len)
            .is_some_and(|end| end <= range.end)
}

fn width_bytes(width: WatchedValueWidth) -> u32 {
    match width {
        WatchedValueWidth::U8 => 1,
        WatchedValueWidth::U16 => 2,
        WatchedValueWidth::U32 => 4,
        WatchedValueWidth::U64 => 8,
    }
}

fn probe_by_id<'a>(plan: &'a ValidatedProbePlan, id: &str) -> Option<&'a Probe> {
    plan.as_plan()
        .probes
        .iter()
        .find(|probe| probe.probe_id == id)
}

fn require_probe<'a>(
    line: usize,
    plan: &'a ValidatedProbePlan,
    probe_id: &str,
) -> Result<&'a Probe, HeadlessBridgeError> {
    probe_by_id(plan, probe_id)
        .ok_or_else(|| HeadlessBridgeError::new(line, format!("unknown probe_id `{probe_id}`")))
}

fn require_event_matches_probe(
    line: usize,
    plan: &ValidatedProbePlan,
    record: &HeadlessObservationRecord,
) -> Result<(), HeadlessBridgeError> {
    let (probe_id, matches) = match record {
        HeadlessObservationRecord::ExecutedPc { probe_id, pc, .. } => {
            let probe = require_probe(line, plan, probe_id)?;
            let matches = matches!(
                &probe.target,
                ProbeTarget::ExecutedPcRange { bank, range }
                    if bank_matches(bank, &pc.bank) && range_contains(*range, pc.address, 4)
            );
            (probe_id, matches)
        }
        HeadlessObservationRecord::IndirectTransfer { probe_id, site, .. } => {
            let probe = require_probe(line, plan, probe_id)?;
            let matches = matches!(
                &probe.target,
                ProbeTarget::IndirectSite { site: expected }
                    if expected.pc == site.address && bank_matches(&expected.bank, &site.bank)
            );
            (probe_id, matches)
        }
        HeadlessObservationRecord::PiDmaCompleted {
            probe_id,
            direction,
            cart_address,
            dram_address,
            byte_len,
            ..
        } => {
            let probe = require_probe(line, plan, probe_id)?;
            let matches = matches!(
                &probe.target,
                ProbeTarget::PiDma { direction: expected_direction, cart_range, dram_range }
                    if expected_direction.is_none_or(|expected| expected == *direction)
                        && cart_range.is_none_or(|range| range_contains(range, *cart_address, *byte_len))
                        && dram_range.is_none_or(|range| range_contains(range, *dram_address, *byte_len))
            );
            (probe_id, matches)
        }
        HeadlessObservationRecord::WatchedTableWrite {
            probe_id,
            watch_id,
            address,
            width,
            active_bank,
            ..
        } => {
            let probe = require_probe(line, plan, probe_id)?;
            let matches = matches!(
                &probe.target,
                ProbeTarget::WatchedWrite { watch_id: expected_watch, bank, range, widths }
                    if expected_watch == watch_id
                        && bank_matches(bank, active_bank)
                        && widths.contains(width)
                        && range_contains(*range, *address, width_bytes(*width))
            );
            (probe_id, matches)
        }
        HeadlessObservationRecord::Header { .. } | HeadlessObservationRecord::End { .. } => {
            return Ok(())
        }
    };
    if !matches {
        return Err(HeadlessBridgeError::new(
            line,
            format!("event does not satisfy probe `{probe_id}`"),
        ));
    }
    Ok(())
}

fn validate_stop(
    line: usize,
    plan: &ValidatedProbePlan,
    stop_reason: &HeadlessStopReason,
    instructions_executed: u64,
    observed_events: u64,
    emulated_time_ns: u64,
) -> Result<TraceCompletion, HeadlessBridgeError> {
    let budget = plan.as_plan().budget;
    if instructions_executed > budget.max_instructions {
        return Err(HeadlessBridgeError::new(
            line,
            "producer exceeded the instruction budget",
        ));
    }
    if observed_events > budget.max_events {
        return Err(HeadlessBridgeError::new(
            line,
            "producer exceeded the event budget",
        ));
    }
    if emulated_time_ns > budget.max_emulated_time_ns {
        return Err(HeadlessBridgeError::new(
            line,
            "producer exceeded the emulated-time budget",
        ));
    }

    match stop_reason {
        HeadlessStopReason::ScenarioComplete => Ok(TraceCompletion::Completed),
        HeadlessStopReason::ProducerAbort { detail } => {
            if detail.is_empty() || detail.trim() != detail || detail.chars().any(char::is_control)
            {
                return Err(HeadlessBridgeError::new(
                    line,
                    "producer-abort detail must be nonempty canonical text",
                ));
            }
            Ok(TraceCompletion::Aborted)
        }
        HeadlessStopReason::BudgetReached { budget: reached } => {
            let at_limit = match reached {
                HeadlessBudgetKind::Instructions => {
                    instructions_executed == budget.max_instructions
                }
                HeadlessBudgetKind::Events => observed_events == budget.max_events,
                HeadlessBudgetKind::EmulatedTime => emulated_time_ns == budget.max_emulated_time_ns,
            };
            if !at_limit {
                return Err(HeadlessBridgeError::new(
                    line,
                    format!("stop reason names {reached:?} budget before its exact limit"),
                ));
            }
            Ok(TraceCompletion::Completed)
        }
    }
}

/// Normalize one wrapper observation stream into the canonical trace schema.
///
/// No exhaustiveness claims are generated yet. Existing trace coverage
/// domains do not encode the range/bank filters from a probe, so translating
/// a filtered instrumentation guarantee would accidentally broaden it.
pub fn normalize_headless_jsonl<R: BufRead>(
    mut reader: R,
    prepared: &PreparedHeadlessRun<'_>,
) -> Result<NormalizedHeadlessTrace, HeadlessBridgeError> {
    let plan = prepared.bundle.probe_plan;
    let mut raw = String::new();
    let mut line = 0usize;
    let mut next_sequence = 0u64;
    let mut saw_header = false;
    let mut saw_end = false;
    let mut observed_events = 0u64;
    let producer = format!(
        "headless:{}:{}:{}",
        prepared.bundle.producer.adapter_id,
        prepared.bundle.producer.emulator,
        prepared.bundle_sha256
    );
    let mut records = Vec::new();

    loop {
        raw.clear();
        let bytes = reader
            .read_line(&mut raw)
            .map_err(|error| HeadlessBridgeError::new(line + 1, error.to_string()))?;
        if bytes == 0 {
            break;
        }
        line += 1;
        if bytes > MAX_JSONL_RECORD_BYTES {
            return Err(HeadlessBridgeError::new(
                line,
                format!("record exceeds {MAX_JSONL_RECORD_BYTES} bytes"),
            ));
        }
        let trimmed = raw.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Err(HeadlessBridgeError::new(line, "blank record"));
        }
        if saw_end {
            return Err(HeadlessBridgeError::new(line, "record appears after end"));
        }
        let record: HeadlessObservationRecord = serde_json::from_str(trimmed)
            .map_err(|error| HeadlessBridgeError::new(line, error.to_string()))?;
        if record.sequence() != next_sequence {
            return Err(HeadlessBridgeError::new(
                line,
                format!(
                    "expected sequence {next_sequence}, found {}",
                    record.sequence()
                ),
            ));
        }
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or_else(|| HeadlessBridgeError::new(line, "sequence overflow after u64::MAX"))?;

        match record {
            HeadlessObservationRecord::Header {
                sequence,
                schema_version,
                trace_id,
                run_bundle_sha256,
            } => {
                if sequence != 0 || saw_header {
                    return Err(HeadlessBridgeError::new(line, "header is not first"));
                }
                if schema_version != HEADLESS_BRIDGE_SCHEMA_VERSION {
                    return Err(HeadlessBridgeError::new(
                        line,
                        format!(
                            "unsupported schema version {schema_version}; expected {HEADLESS_BRIDGE_SCHEMA_VERSION}"
                        ),
                    ));
                }
                if trace_id != prepared.bundle.trace_id {
                    return Err(HeadlessBridgeError::new(line, "trace_id mismatch"));
                }
                if run_bundle_sha256 != prepared.bundle_sha256 {
                    return Err(HeadlessBridgeError::new(line, "run bundle digest mismatch"));
                }
                saw_header = true;
                records.push(TraceRecord::Header {
                    sequence,
                    schema_version: TRACE_SCHEMA_VERSION,
                    normalized_rom_sha256: plan.as_plan().normalized_rom_sha256.clone(),
                    trace_id,
                    producer: producer.clone(),
                });
            }
            other if !saw_header => {
                let _ = other;
                return Err(HeadlessBridgeError::new(
                    line,
                    "first record must be a header",
                ));
            }
            event @ HeadlessObservationRecord::ExecutedPc { .. }
            | event @ HeadlessObservationRecord::IndirectTransfer { .. }
            | event @ HeadlessObservationRecord::PiDmaCompleted { .. }
            | event @ HeadlessObservationRecord::WatchedTableWrite { .. } => {
                require_event_matches_probe(line, plan, &event)?;
                observed_events = observed_events
                    .checked_add(1)
                    .ok_or_else(|| HeadlessBridgeError::new(line, "event count overflow"))?;
                let normalized = match event {
                    HeadlessObservationRecord::ExecutedPc { sequence, pc, .. } => {
                        TraceRecord::ExecutedPc { sequence, pc }
                    }
                    HeadlessObservationRecord::IndirectTransfer {
                        sequence,
                        kind,
                        site,
                        target,
                        ..
                    } => TraceRecord::IndirectTransfer {
                        sequence,
                        kind,
                        site,
                        target,
                    },
                    HeadlessObservationRecord::PiDmaCompleted {
                        sequence,
                        direction,
                        cart_address,
                        dram_address,
                        byte_len,
                        active_bank,
                        ..
                    } => TraceRecord::PiDma {
                        sequence,
                        direction,
                        cart_address,
                        dram_address,
                        byte_len,
                        active_bank,
                    },
                    HeadlessObservationRecord::WatchedTableWrite {
                        sequence,
                        watch_id,
                        address,
                        width,
                        value,
                        active_bank,
                        ..
                    } => TraceRecord::WatchedTableWrite {
                        sequence,
                        watch_id,
                        address,
                        width,
                        value,
                        active_bank,
                    },
                    HeadlessObservationRecord::Header { .. }
                    | HeadlessObservationRecord::End { .. } => unreachable!(),
                };
                records.push(normalized);
            }
            HeadlessObservationRecord::End {
                sequence,
                stop_reason,
                instructions_executed,
                emulated_time_ns,
            } => {
                let completion = validate_stop(
                    line,
                    plan,
                    &stop_reason,
                    instructions_executed,
                    observed_events,
                    emulated_time_ns,
                )?;
                records.push(TraceRecord::End {
                    sequence,
                    completion,
                    exhaustiveness: Vec::new(),
                });
                saw_end = true;
            }
        }
    }

    if !saw_header {
        return Err(HeadlessBridgeError::new(0, "missing header"));
    }
    if !saw_end {
        return Err(HeadlessBridgeError::new(line, "missing end record"));
    }
    let normalized = NormalizedHeadlessTrace { records };
    let mut canonical_jsonl = Vec::new();
    normalized.write_jsonl(&mut canonical_jsonl)?;
    crate::trace::ingest_jsonl(
        Cursor::new(canonical_jsonl),
        &plan.as_plan().normalized_rom_sha256,
    )
    .map_err(|error| {
        HeadlessBridgeError::new(0, format!("normalized trace failed validation: {error}"))
    })?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{
        ExpectedInformationGain, InputTimelineDigest, ProbeAddress, ProbeBudget, ProbePlan,
        ScenarioIdentity,
    };
    use crate::trace::{ingest_jsonl, NormalizedRomDigest, ObservedTraceFact};
    use std::io::Cursor;

    const ROM_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TIMELINE_DIGEST: &str =
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn probe(id: &str, target: ProbeTarget) -> Probe {
        Probe {
            probe_id: id.to_string(),
            target,
            expected_information_gain: ExpectedInformationGain {
                priority: 1,
                unresolved_question: format!("resolve {id}"),
            },
        }
    }

    fn plan() -> ValidatedProbePlan {
        ProbePlan {
            normalized_rom_sha256: NormalizedRomDigest::try_from(ROM_DIGEST.to_string()).unwrap(),
            scenario: ScenarioIdentity {
                scenario_id: "boot-neutral".to_string(),
                input_timeline_id: "neutral-v1".to_string(),
                input_timeline_sha256: InputTimelineDigest::try_from(TIMELINE_DIGEST.to_string())
                    .unwrap(),
                start_emulated_time_ns: 0,
            },
            budget: ProbeBudget {
                max_instructions: 100,
                max_events: 4,
                max_emulated_time_ns: 1_000,
            },
            probes: vec![
                probe(
                    "pc",
                    ProbeTarget::ExecutedPcRange {
                        bank: BankScope::Known {
                            bank: "resident".to_string(),
                        },
                        range: AddressRange {
                            start: 0x8000_0400,
                            end: 0x8000_0500,
                        },
                    },
                ),
                probe(
                    "indirect",
                    ProbeTarget::IndirectSite {
                        site: ProbeAddress {
                            pc: 0x8000_0600,
                            bank: BankScope::Any,
                        },
                    },
                ),
                probe(
                    "dma",
                    ProbeTarget::PiDma {
                        direction: Some(PiDmaDirection::CartToRdram),
                        cart_range: Some(AddressRange {
                            start: 0x1000_1000,
                            end: 0x1000_2000,
                        }),
                        dram_range: Some(AddressRange {
                            start: 0x0010_0000,
                            end: 0x0011_0000,
                        }),
                    },
                ),
                probe(
                    "write",
                    ProbeTarget::WatchedWrite {
                        watch_id: "callbacks".to_string(),
                        bank: BankScope::Any,
                        range: AddressRange {
                            start: 0x8000_1000,
                            end: 0x8000_1100,
                        },
                        widths: vec![WatchedValueWidth::U32],
                    },
                ),
            ],
        }
        .validate()
        .unwrap()
    }

    fn producer() -> HeadlessProducerIdentity {
        HeadlessProducerIdentity {
            adapter_id: "synthetic-neutral-jsonl".to_string(),
            adapter_version: "1".to_string(),
            emulator: "synthetic-emulator".to_string(),
            emulator_version: "1.0".to_string(),
            executable_sha256: HeadlessArtifactDigest::try_from("a".repeat(64)).unwrap(),
            settings_sha256: HeadlessArtifactDigest::try_from("b".repeat(64)).unwrap(),
        }
    }

    fn launch() -> HeadlessLaunchIdentity {
        HeadlessLaunchIdentity {
            reset: HeadlessResetKind::PowerOn,
            region: HeadlessRegion::Ntsc,
            initial_state_sha256: None,
        }
    }

    fn header(prepared: &PreparedHeadlessRun<'_>) -> String {
        format!(
            r#"{{"event":"header","sequence":0,"schema_version":1,"trace_id":"trace-1","run_bundle_sha256":"{}"}}"#,
            prepared.bundle_sha256()
        )
    }

    #[test]
    fn bundle_is_byte_deterministic_and_digest_bound() {
        let plan = plan();
        let producer = producer();
        let launch = launch();
        let first = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();
        let second = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();
        let mut first_json = Vec::new();
        let mut second_json = Vec::new();
        first.write_json(&mut first_json).unwrap();
        second.write_json(&mut second_json).unwrap();
        assert_eq!(first_json, second_json);
        assert_eq!(first.bundle_sha256(), second.bundle_sha256());
        assert!(String::from_utf8(first_json).unwrap().contains(ROM_DIGEST));
    }

    #[test]
    fn normalizes_every_event_class_then_passes_canonical_ingest() {
        let plan = plan();
        let producer = producer();
        let launch = launch();
        let prepared = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();
        let lines = [
            header(&prepared),
            r#"{"event":"executed_pc","sequence":1,"probe_id":"pc","pc":{"address":2147484672,"bank":{"status":"known","bank":"resident","activation":0}}}"#.to_string(),
            r#"{"event":"indirect_transfer","sequence":2,"probe_id":"indirect","kind":"call","site":{"address":2147485184,"bank":{"status":"unknown"}},"target":{"address":2147485696,"bank":{"status":"unknown"}}}"#.to_string(),
            r#"{"event":"pi_dma_completed","sequence":3,"probe_id":"dma","direction":"cart_to_rdram","cart_address":268439552,"dram_address":1048576,"byte_len":64,"active_bank":{"status":"unknown"}}"#.to_string(),
            r#"{"event":"watched_table_write","sequence":4,"probe_id":"write","watch_id":"callbacks","address":2147487744,"width":"u32","value":2147485696,"active_bank":{"status":"unknown"}}"#.to_string(),
            r#"{"event":"end","sequence":5,"stop_reason":{"reason":"budget_reached","budget":"events"},"instructions_executed":80,"emulated_time_ns":900}"#.to_string(),
        ];
        let normalized =
            normalize_headless_jsonl(Cursor::new(lines.join("\n")), &prepared).unwrap();
        let mut jsonl = Vec::new();
        normalized.write_jsonl(&mut jsonl).unwrap();
        let expected = NormalizedRomDigest::try_from(ROM_DIGEST.to_string()).unwrap();
        let report = ingest_jsonl(Cursor::new(jsonl), &expected).unwrap();
        assert_eq!(report.facts.len(), 4);
        assert_eq!(report.counts.pi_dma, 1);
        assert!(matches!(
            report.facts[0],
            ObservedTraceFact::ExecutedPc { .. }
        ));
        assert!(report.exhaustiveness.is_empty());
    }

    #[test]
    fn rejects_identity_sequence_and_unknown_fields() {
        let plan = plan();
        let producer = producer();
        let launch = launch();
        let prepared = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();

        let wrong_digest = header(&prepared).replace(prepared.bundle_sha256(), &"f".repeat(64));
        let error = normalize_headless_jsonl(Cursor::new(wrong_digest), &prepared).unwrap_err();
        assert!(error.message.contains("digest mismatch"));

        let unknown = format!(
            "{}\n{}",
            header(&prepared),
            r#"{"event":"end","sequence":2,"stop_reason":{"reason":"scenario_complete"},"instructions_executed":1,"emulated_time_ns":1}"#
        );
        let error = normalize_headless_jsonl(Cursor::new(unknown), &prepared).unwrap_err();
        assert!(error.message.contains("expected sequence 1"));

        let extra = header(&prepared).replace("}", ",\"path\":\"/tmp/rom.z64\"}");
        let error = normalize_headless_jsonl(Cursor::new(extra), &prepared).unwrap_err();
        assert!(error.message.contains("unknown field"));
    }

    #[test]
    fn rejects_events_outside_the_named_probe() {
        let plan = plan();
        let producer = producer();
        let launch = launch();
        let prepared = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();
        let input = format!(
            "{}\n{}",
            header(&prepared),
            r#"{"event":"executed_pc","sequence":1,"probe_id":"pc","pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#
        );
        let error = normalize_headless_jsonl(Cursor::new(input), &prepared).unwrap_err();
        assert!(error.message.contains("does not satisfy probe"));

        let input = format!(
            "{}\n{}",
            header(&prepared),
            r#"{"event":"pi_dma_completed","sequence":1,"probe_id":"dma","direction":"cart_to_rdram","cart_address":268439552,"dram_address":1114080,"byte_len":64,"active_bank":{"status":"unknown"}}"#
        );
        let error = normalize_headless_jsonl(Cursor::new(input), &prepared).unwrap_err();
        assert!(error.message.contains("does not satisfy probe"));
    }

    #[test]
    fn budgets_are_enforced_and_abort_maps_without_exhaustiveness() {
        let plan = plan();
        let producer = producer();
        let launch = launch();
        let prepared = prepare_headless_run("trace-1", &producer, &launch, &plan).unwrap();
        let early_budget = format!(
            "{}\n{}",
            header(&prepared),
            r#"{"event":"end","sequence":1,"stop_reason":{"reason":"budget_reached","budget":"instructions"},"instructions_executed":99,"emulated_time_ns":10}"#
        );
        let error = normalize_headless_jsonl(Cursor::new(early_budget), &prepared).unwrap_err();
        assert!(error.message.contains("before its exact limit"));

        let aborted = format!(
            "{}\n{}",
            header(&prepared),
            r#"{"event":"end","sequence":1,"stop_reason":{"reason":"producer_abort","detail":"watchdog"},"instructions_executed":20,"emulated_time_ns":50}"#
        );
        let normalized = normalize_headless_jsonl(Cursor::new(aborted), &prepared).unwrap();
        assert!(matches!(
            normalized.records.last(),
            Some(TraceRecord::End {
                completion: TraceCompletion::Aborted,
                exhaustiveness,
                ..
            }) if exhaustiveness.is_empty()
        ));
    }

    #[test]
    fn artifact_digest_cannot_use_ambiguous_hex() {
        assert!(HeadlessArtifactDigest::try_from("a".repeat(63)).is_err());
        assert!(HeadlessArtifactDigest::try_from("A".repeat(64)).is_err());
        assert!(HeadlessArtifactDigest::try_from("g".repeat(64)).is_err());
    }

    #[test]
    fn restored_state_is_explicitly_digest_bound() {
        let plan = plan();
        let producer = producer();
        let invalid = HeadlessLaunchIdentity {
            reset: HeadlessResetKind::StateRestore,
            region: HeadlessRegion::Pal,
            initial_state_sha256: None,
        };
        let error = prepare_headless_run("trace-1", &producer, &invalid, &plan).unwrap_err();
        assert!(error.message.contains("initial-state digest"));

        let invalid = HeadlessLaunchIdentity {
            reset: HeadlessResetKind::PowerOn,
            region: HeadlessRegion::Ntsc,
            initial_state_sha256: Some(HeadlessArtifactDigest::try_from("c".repeat(64)).unwrap()),
        };
        let error = prepare_headless_run("trace-1", &producer, &invalid, &plan).unwrap_err();
        assert!(error.message.contains("other resets forbid"));
    }
}
