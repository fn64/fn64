//! Bounded dynamic-trace interchange for discovery.
//!
//! A trace is a JSONL stream bound to the SHA-256 of the normalized
//! big-endian ROM. Records are strictly sequenced so truncation, duplication,
//! and accidental concatenation fail loudly. Dynamic events are observations:
//! an observed target proves that one transfer happened, never that the target
//! set is complete. A completed producer may separately state bounded
//! instrumentation guarantees in the final record.
//!
//! Bank identity is deliberately explicit at every address. `Unknown` is
//! preserved through ingestion; this module never assigns a bank from a VA.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::io::BufRead;

pub const TRACE_SCHEMA_VERSION: u32 = 1;
pub const MAX_JSONL_RECORD_BYTES: usize = 1024 * 1024;
const PI_DRAM_ADDRESS_SPACE_BYTES: u32 = 0x0100_0000;
const PI_MAX_TRANSFER_BYTES: u32 = 0x0100_0000;

/// SHA-256 of the Phase-1 normalized, big-endian ROM bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NormalizedRomDigest(String);

impl NormalizedRomDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NormalizedRomDigest {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64 {
            return Err("normalized ROM SHA-256 must contain exactly 64 hex digits");
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("normalized ROM SHA-256 must use lowercase hexadecimal");
        }
        Ok(Self(value))
    }
}

impl From<NormalizedRomDigest> for String {
    fn from(value: NormalizedRomDigest) -> Self {
        value.0
    }
}

impl Serialize for NormalizedRomDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NormalizedRomDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// The producer's bank identity at one observation. Activation distinguishes
/// two lifetimes of the same named overlay without inventing new bank names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BankContext {
    Unknown,
    Known { bank: String, activation: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObservedAddress {
    pub address: u32,
    pub bank: BankContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiDmaDirection {
    CartToRdram,
    RdramToCart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectTransferKind {
    Call,
    Jump,
    Return,
    ExceptionReturn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchedValueWidth {
    U8,
    U16,
    U32,
    U64,
}

impl WatchedValueWidth {
    fn bytes(self) -> u32 {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U32 => 4,
            Self::U64 => 8,
        }
    }

    fn accepts(self, value: u64) -> bool {
        match self {
            Self::U8 => value <= u8::MAX.into(),
            Self::U16 => value <= u16::MAX.into(),
            Self::U32 => value <= u32::MAX.into(),
            Self::U64 => true,
        }
    }
}

/// What an instrumentation guarantee covers. Such a guarantee means the
/// producer recorded every event of this class during its sequence interval;
/// it does not claim all possible program behavior was exercised.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "domain", rename_all = "snake_case")]
pub enum CoverageDomain {
    PiDma,
    ExecutedPc,
    IndirectTransfer,
    WatchedTableWrite { watch_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExhaustivenessClaim {
    #[serde(flatten)]
    pub domain: CoverageDomain,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceCompletion {
    Completed,
    Aborted,
}

/// One input line. The header must be sequence zero and the end record must be
/// last. Every intervening sequence number must increase by exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TraceRecord {
    Header {
        sequence: u64,
        schema_version: u32,
        normalized_rom_sha256: NormalizedRomDigest,
        trace_id: String,
        producer: String,
    },
    PiDma {
        sequence: u64,
        direction: PiDmaDirection,
        cart_address: u32,
        dram_address: u32,
        byte_len: u32,
        active_bank: BankContext,
    },
    ExecutedPc {
        sequence: u64,
        pc: ObservedAddress,
    },
    IndirectTransfer {
        sequence: u64,
        kind: IndirectTransferKind,
        site: ObservedAddress,
        target: ObservedAddress,
    },
    WatchedTableWrite {
        sequence: u64,
        watch_id: String,
        address: u32,
        width: WatchedValueWidth,
        value: u64,
        active_bank: BankContext,
    },
    End {
        sequence: u64,
        completion: TraceCompletion,
        exhaustiveness: Vec<ExhaustivenessClaim>,
    },
}

impl TraceRecord {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Header { sequence, .. }
            | Self::PiDma { sequence, .. }
            | Self::ExecutedPc { sequence, .. }
            | Self::IndirectTransfer { sequence, .. }
            | Self::WatchedTableWrite { sequence, .. }
            | Self::End { sequence, .. } => *sequence,
        }
    }
}

/// Typed observations ready for a later adapter into `FactDb`. None of these
/// variants asserts that an observed set is exhaustive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case")]
pub enum ObservedTraceFact {
    PiDma {
        sequence: u64,
        direction: PiDmaDirection,
        cart_address: u32,
        dram_address: u32,
        byte_len: u32,
        active_bank: BankContext,
    },
    ExecutedPc {
        sequence: u64,
        pc: ObservedAddress,
    },
    IndirectTransfer {
        sequence: u64,
        kind: IndirectTransferKind,
        site: ObservedAddress,
        target: ObservedAddress,
    },
    WatchedTableWrite {
        sequence: u64,
        watch_id: String,
        address: u32,
        width: WatchedValueWidth,
        value: u64,
        active_bank: BankContext,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEventCounts {
    pub pi_dma: u64,
    pub executed_pc: u64,
    pub indirect_transfer: u64,
    pub watched_table_write: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceHeader {
    pub schema_version: u32,
    pub normalized_rom_sha256: NormalizedRomDigest,
    pub trace_id: String,
    pub producer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestReport {
    pub header: TraceHeader,
    pub completion: TraceCompletion,
    pub final_sequence: u64,
    pub counts: TraceEventCounts,
    pub observations_with_unknown_bank: u64,
    pub facts: Vec<ObservedTraceFact>,
    pub exhaustiveness: Vec<ExhaustivenessClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceIngestError {
    pub line: usize,
    pub message: String,
}

impl TraceIngestError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for TraceIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "trace: {}", self.message)
        } else {
            write!(formatter, "trace line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for TraceIngestError {}

fn validate_nonempty(line: usize, label: &str, value: &str) -> Result<(), TraceIngestError> {
    if value.trim().is_empty() {
        Err(TraceIngestError::at(
            line,
            format!("{label} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

fn validate_bank(line: usize, bank: &BankContext) -> Result<bool, TraceIngestError> {
    match bank {
        BankContext::Unknown => Ok(true),
        BankContext::Known { bank, .. } => {
            validate_nonempty(line, "bank", bank)?;
            Ok(false)
        }
    }
}

fn validate_instruction_address(
    line: usize,
    label: &str,
    address: &ObservedAddress,
) -> Result<u64, TraceIngestError> {
    if address.address & 3 != 0 {
        return Err(TraceIngestError::at(
            line,
            format!("{label} 0x{:08x} is not four-byte aligned", address.address),
        ));
    }
    Ok(u64::from(validate_bank(line, &address.bank)?))
}

/// Ingest one complete trace, rejecting records bound to another normalized
/// ROM. Output ordering is input sequence ordering and uses only ordered
/// collections for validation, so repeated ingestion is byte-deterministic
/// under `serde_json` serialization.
pub fn ingest_jsonl<R: BufRead>(
    mut reader: R,
    expected_rom: &NormalizedRomDigest,
) -> Result<IngestReport, TraceIngestError> {
    let mut raw = String::new();
    let mut line_number = 0usize;
    let mut header = None;
    let mut next_sequence = 0u64;
    let mut facts = Vec::new();
    let mut counts = TraceEventCounts::default();
    let mut unknown_bank_count = 0u64;
    let mut end = None;

    loop {
        raw.clear();
        let bytes = reader
            .read_line(&mut raw)
            .map_err(|error| TraceIngestError::at(line_number + 1, error.to_string()))?;
        if bytes == 0 {
            break;
        }
        line_number += 1;
        if bytes > MAX_JSONL_RECORD_BYTES {
            return Err(TraceIngestError::at(
                line_number,
                format!("record exceeds {MAX_JSONL_RECORD_BYTES} bytes"),
            ));
        }
        let trimmed = raw.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Err(TraceIngestError::at(
                line_number,
                "blank records are not permitted",
            ));
        }
        if end.is_some() {
            return Err(TraceIngestError::at(
                line_number,
                "record appears after end",
            ));
        }

        let record: TraceRecord = serde_json::from_str(trimmed)
            .map_err(|error| TraceIngestError::at(line_number, error.to_string()))?;
        if record.sequence() != next_sequence {
            return Err(TraceIngestError::at(
                line_number,
                format!(
                    "expected sequence {next_sequence}, found {}",
                    record.sequence()
                ),
            ));
        }
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or_else(|| TraceIngestError::at(line_number, "sequence overflow after u64::MAX"))?;

        match record {
            TraceRecord::Header {
                sequence: _,
                schema_version,
                normalized_rom_sha256,
                trace_id,
                producer,
            } => {
                if header.is_some() || !facts.is_empty() {
                    return Err(TraceIngestError::at(line_number, "header is not first"));
                }
                if schema_version != TRACE_SCHEMA_VERSION {
                    return Err(TraceIngestError::at(
                        line_number,
                        format!(
                            "unsupported schema version {schema_version}; expected {TRACE_SCHEMA_VERSION}"
                        ),
                    ));
                }
                if &normalized_rom_sha256 != expected_rom {
                    return Err(TraceIngestError::at(
                        line_number,
                        "normalized ROM digest does not match the requested ROM",
                    ));
                }
                validate_nonempty(line_number, "trace_id", &trace_id)?;
                validate_nonempty(line_number, "producer", &producer)?;
                header = Some(TraceHeader {
                    schema_version,
                    normalized_rom_sha256,
                    trace_id,
                    producer,
                });
            }
            other if header.is_none() => {
                let _ = other;
                return Err(TraceIngestError::at(
                    line_number,
                    "first record must be a header",
                ));
            }
            TraceRecord::PiDma {
                sequence,
                direction,
                cart_address,
                dram_address,
                byte_len,
                active_bank,
            } => {
                if byte_len == 0 || byte_len > PI_MAX_TRANSFER_BYTES {
                    return Err(TraceIngestError::at(
                        line_number,
                        format!(
                            "PI DMA byte_len {byte_len} is outside 1..={PI_MAX_TRANSFER_BYTES}"
                        ),
                    ));
                }
                let dram_end = dram_address.checked_add(byte_len).ok_or_else(|| {
                    TraceIngestError::at(line_number, "PI DMA DRAM interval overflows u32")
                })?;
                if dram_address >= PI_DRAM_ADDRESS_SPACE_BYTES
                    || dram_end > PI_DRAM_ADDRESS_SPACE_BYTES
                {
                    return Err(TraceIngestError::at(
                        line_number,
                        "PI DMA DRAM interval is outside the 24-bit PI DRAM address space",
                    ));
                }
                cart_address.checked_add(byte_len).ok_or_else(|| {
                    TraceIngestError::at(line_number, "PI DMA cartridge interval overflows u32")
                })?;
                unknown_bank_count += u64::from(validate_bank(line_number, &active_bank)?);
                counts.pi_dma += 1;
                facts.push(ObservedTraceFact::PiDma {
                    sequence,
                    direction,
                    cart_address,
                    dram_address,
                    byte_len,
                    active_bank,
                });
            }
            TraceRecord::ExecutedPc { sequence, pc } => {
                unknown_bank_count += validate_instruction_address(line_number, "PC", &pc)?;
                counts.executed_pc += 1;
                facts.push(ObservedTraceFact::ExecutedPc { sequence, pc });
            }
            TraceRecord::IndirectTransfer {
                sequence,
                kind,
                site,
                target,
            } => {
                unknown_bank_count +=
                    validate_instruction_address(line_number, "transfer site", &site)?;
                unknown_bank_count +=
                    validate_instruction_address(line_number, "transfer target", &target)?;
                counts.indirect_transfer += 1;
                facts.push(ObservedTraceFact::IndirectTransfer {
                    sequence,
                    kind,
                    site,
                    target,
                });
            }
            TraceRecord::WatchedTableWrite {
                sequence,
                watch_id,
                address,
                width,
                value,
                active_bank,
            } => {
                validate_nonempty(line_number, "watch_id", &watch_id)?;
                let width_bytes = width.bytes();
                if address % width_bytes != 0 {
                    return Err(TraceIngestError::at(
                        line_number,
                        format!(
                            "watched write address 0x{address:08x} is not {width_bytes}-byte aligned"
                        ),
                    ));
                }
                address.checked_add(width_bytes).ok_or_else(|| {
                    TraceIngestError::at(line_number, "watched write interval overflows u32")
                })?;
                if !width.accepts(value) {
                    return Err(TraceIngestError::at(
                        line_number,
                        format!("value 0x{value:x} does not fit {width:?}"),
                    ));
                }
                unknown_bank_count += u64::from(validate_bank(line_number, &active_bank)?);
                counts.watched_table_write += 1;
                facts.push(ObservedTraceFact::WatchedTableWrite {
                    sequence,
                    watch_id,
                    address,
                    width,
                    value,
                    active_bank,
                });
            }
            TraceRecord::End {
                sequence,
                completion,
                exhaustiveness,
            } => {
                if completion == TraceCompletion::Aborted && !exhaustiveness.is_empty() {
                    return Err(TraceIngestError::at(
                        line_number,
                        "an aborted trace cannot claim exhaustive instrumentation",
                    ));
                }
                let mut unique = BTreeSet::new();
                for claim in &exhaustiveness {
                    if claim.first_sequence == 0
                        || claim.first_sequence > claim.last_sequence
                        || claim.last_sequence >= sequence
                    {
                        return Err(TraceIngestError::at(
                            line_number,
                            format!(
                                "invalid exhaustiveness interval {}..={} for end sequence {sequence}",
                                claim.first_sequence, claim.last_sequence
                            ),
                        ));
                    }
                    if let CoverageDomain::WatchedTableWrite { watch_id } = &claim.domain {
                        validate_nonempty(line_number, "exhaustiveness watch_id", watch_id)?;
                    }
                    if !unique.insert(claim.clone()) {
                        return Err(TraceIngestError::at(
                            line_number,
                            "duplicate exhaustiveness claim",
                        ));
                    }
                }
                end = Some((sequence, completion, exhaustiveness));
            }
        }
    }

    let header = header.ok_or_else(|| TraceIngestError::at(0, "missing header"))?;
    let (final_sequence, completion, mut exhaustiveness) =
        end.ok_or_else(|| TraceIngestError::at(line_number, "missing end record"))?;
    exhaustiveness.sort();
    Ok(IngestReport {
        header,
        completion,
        final_sequence,
        counts,
        observations_with_unknown_bank: unknown_bank_count,
        facts,
        exhaustiveness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn digest() -> NormalizedRomDigest {
        NormalizedRomDigest::try_from(DIGEST.to_string()).unwrap()
    }

    fn ingest(lines: &[&str]) -> Result<IngestReport, TraceIngestError> {
        ingest_jsonl(Cursor::new(lines.join("\n")), &digest())
    }

    fn header() -> &'static str {
        r#"{"event":"header","sequence":0,"schema_version":1,"normalized_rom_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","trace_id":"test-1","producer":"synthetic-test"}"#
    }

    #[test]
    fn ingests_all_event_classes_without_inventing_unknown_banks() {
        let report = ingest(&[
            header(),
            r#"{"event":"pi_dma","sequence":1,"direction":"cart_to_rdram","cart_address":268439552,"dram_address":1024,"byte_len":64,"active_bank":{"status":"unknown"}}"#,
            r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}}}"#,
            r#"{"event":"indirect_transfer","sequence":3,"kind":"call","site":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}},"target":{"address":2148532224,"bank":{"status":"unknown"}}}"#,
            r#"{"event":"watched_table_write","sequence":4,"watch_id":"overlay-slots","address":2149580800,"width":"u32","value":2148532224,"active_bank":{"status":"known","bank":"loader","activation":7}}"#,
            r#"{"event":"end","sequence":5,"completion":"completed","exhaustiveness":[{"domain":"pi_dma","first_sequence":1,"last_sequence":4},{"domain":"indirect_transfer","first_sequence":1,"last_sequence":4}]}"#,
        ])
        .unwrap();

        assert_eq!(report.final_sequence, 5);
        assert_eq!(report.facts.len(), 4);
        assert_eq!(report.counts.pi_dma, 1);
        assert_eq!(report.counts.executed_pc, 1);
        assert_eq!(report.counts.indirect_transfer, 1);
        assert_eq!(report.counts.watched_table_write, 1);
        assert_eq!(report.observations_with_unknown_bank, 2);
        assert!(matches!(
            &report.facts[0],
            ObservedTraceFact::PiDma {
                active_bank: BankContext::Unknown,
                ..
            }
        ));
    }

    #[test]
    fn rejects_digest_mismatch_before_events() {
        let other = NormalizedRomDigest::try_from("f".repeat(64)).unwrap();
        let error = ingest_jsonl(Cursor::new(header()), &other).unwrap_err();
        assert!(error.message.contains("digest does not match"));
    }

    #[test]
    fn rejects_sequence_gaps_and_records_after_end() {
        let gap = ingest(&[
            header(),
            r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
        ])
        .unwrap_err();
        assert!(gap.message.contains("expected sequence 1"));

        let after_end = ingest(&[
            header(),
            r#"{"event":"end","sequence":1,"completion":"completed","exhaustiveness":[]}"#,
            r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
        ])
        .unwrap_err();
        assert!(after_end.message.contains("after end"));
    }

    #[test]
    fn validates_instruction_dma_and_table_write_ranges() {
        let unaligned_pc = ingest(&[
            header(),
            r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484673,"bank":{"status":"unknown"}}}"#,
        ])
        .unwrap_err();
        assert!(unaligned_pc.message.contains("four-byte aligned"));

        let dma_overrun = ingest(&[
            header(),
            r#"{"event":"pi_dma","sequence":1,"direction":"cart_to_rdram","cart_address":268435456,"dram_address":16777200,"byte_len":32,"active_bank":{"status":"unknown"}}"#,
        ])
        .unwrap_err();
        assert!(dma_overrun.message.contains("24-bit PI DRAM"));

        let unaligned_write = ingest(&[
            header(),
            r#"{"event":"watched_table_write","sequence":1,"watch_id":"table","address":4098,"width":"u32","value":1,"active_bank":{"status":"unknown"}}"#,
        ])
        .unwrap_err();
        assert!(unaligned_write.message.contains("not 4-byte aligned"));

        let wide_value = ingest(&[
            header(),
            r#"{"event":"watched_table_write","sequence":1,"watch_id":"table","address":4096,"width":"u8","value":256,"active_bank":{"status":"unknown"}}"#,
        ])
        .unwrap_err();
        assert!(wide_value.message.contains("does not fit"));
    }

    #[test]
    fn exhaustive_claims_require_completed_bounded_intervals() {
        let aborted = ingest(&[
            header(),
            r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
            r#"{"event":"end","sequence":2,"completion":"aborted","exhaustiveness":[{"domain":"executed_pc","first_sequence":1,"last_sequence":1}]}"#,
        ])
        .unwrap_err();
        assert!(aborted.message.contains("aborted trace"));

        let outside = ingest(&[
            header(),
            r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
            r#"{"event":"end","sequence":2,"completion":"completed","exhaustiveness":[{"domain":"executed_pc","first_sequence":1,"last_sequence":2}]}"#,
        ])
        .unwrap_err();
        assert!(outside.message.contains("invalid exhaustiveness interval"));
    }

    #[test]
    fn requires_footer_and_valid_nonempty_known_bank() {
        let missing_end = ingest(&[header()]).unwrap_err();
        assert!(missing_end.message.contains("missing end"));

        let empty_bank = ingest(&[
            header(),
            r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"known","bank":" ","activation":1}}}"#,
        ])
        .unwrap_err();
        assert!(empty_bank.message.contains("bank must not be empty"));
    }

    #[test]
    fn report_serialization_is_deterministic() {
        let input = [
            header(),
            r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
            r#"{"event":"end","sequence":2,"completion":"completed","exhaustiveness":[]}"#,
        ];
        let first = serde_json::to_string(&ingest(&input).unwrap()).unwrap();
        let second = serde_json::to_string(&ingest(&input).unwrap()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn digest_rejects_ambiguous_encodings() {
        assert!(NormalizedRomDigest::try_from("0".repeat(63)).is_err());
        assert!(NormalizedRomDigest::try_from("A".repeat(64)).is_err());
        assert!(NormalizedRomDigest::try_from("g".repeat(64)).is_err());
    }
}
