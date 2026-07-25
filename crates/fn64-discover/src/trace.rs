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

/// One `ExecutedPc` observation landing on a word static analysis had
/// already called `ProvenData`. Dynamic evidence (this word ran) and static
/// evidence (this word is proven non-code) genuinely disagree; per
/// `facts.rs`'s monotonic-fact invariant, neither side may silently
/// overwrite the other, so this is a typed finding, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticDataConflict {
    pub site: crate::facts::BankAddr,
    pub trace_id: String,
    pub sequence: u64,
}

/// The measured effect of folding one trace's `ExecutedPc` observations into
/// a [`crate::facts::FactDb`] as bank-scoped dynamic code-existence evidence
/// ([`crate::facts::Fact::ObservedExecutedCode`]). Every count here is a
/// direct measurement of what [`fold_executed_pcs_into_fact_db`] actually did
/// to `db` -- never an estimate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactDbFoldReport {
    /// Total `Fact::ObservedExecutedCode` records appended to the database
    /// (one per known-bank `ExecutedPc` observation folded).
    pub facts_added: u64,
    /// `(bank, pc)` words that had no prior code-existence conclusion of any
    /// kind before this fold and now carry one from this evidence class.
    pub new_code_existence: BTreeSet<crate::facts::BankAddr>,
    /// `(bank, pc)` words that already carried a code-existence conclusion
    /// (from this or an earlier fold, static or dynamic) and this
    /// observation corroborates -- a second-or-later independent sighting of
    /// the same word, not new evidence of existence.
    pub corroborated: BTreeSet<crate::facts::BankAddr>,
    /// Observations that landed on a word static analysis had already
    /// proven `ProvenData`. Reported, never resolved by fiat.
    pub conflicts: Vec<StaticDataConflict>,
    /// `ExecutedPc` observations skipped because their bank was
    /// `BankContext::Unknown`. Never promoting, per this module's mandate
    /// that unknown-bank identity is preserved rather than guessed.
    pub unknown_bank_skipped: u64,
}

/// Fold every known-bank `ExecutedPc` observation in `facts` into `db` as
/// [`crate::facts::Fact::ObservedExecutedCode`] evidence, and return the
/// exact measured delta.
///
/// This is deliberately the *only* class of trace fact this adapter
/// promotes: an executed PC in a known bank proves that word is code that
/// ran once -- weaker than a proven owner, weaker than CFG-reachability
/// proof, but a real, distinct, auditable evidence class (`observed_executed`
/// / dynamic code existence). `IndirectTransfer`, `PiDma`, and
/// `WatchedTableWrite` facts are untouched here; they remain, as `trace.rs`'s
/// module doc says, observations without an admission rule of their own yet.
///
/// `static_word_class` looks up whatever static classification the caller
/// already has for `(bank, va)` (e.g. from a `cfg::Cfg::word_class` built by
/// `resolve::build_cfg_closed_with_facts`/`build_cfg_exploratory_with_candidates`).
/// It is a caller-supplied lookup, not a CFG this module builds itself, so
/// `trace.rs` stays decoupled from bank-materialization and CFG
/// construction. Returning `None` means "no static code/data claim at this
/// word" and is never treated as a conflict.
///
/// Idempotent per `(bank, pc)`: observing the same PC twice inserts two
/// `Fact::ObservedExecutedCode` records (distinct provenance, exact-duplicate
/// facts are harmless per `FactDb::insert`'s contract) but only ever
/// concludes the `(bank, pc)` subject once, growing its `justified_by` list
/// -- "one fact[-conclusion] with two provenance records."
pub fn fold_executed_pcs_into_fact_db(
    db: &mut crate::facts::FactDb,
    trace_id: &str,
    facts: &[ObservedTraceFact],
    static_word_class: impl Fn(&str, u32) -> Option<crate::cfg::WordClass>,
) -> FactDbFoldReport {
    use crate::cfg::WordClass;
    use crate::facts::{observed_executed_code_subject, BankAddr, Fact, ProofState};

    let mut report = FactDbFoldReport::default();
    for observed in facts {
        let ObservedTraceFact::ExecutedPc { sequence, pc } = observed else {
            continue;
        };
        let BankContext::Known { bank, .. } = &pc.bank else {
            report.unknown_bank_skipped += 1;
            continue;
        };
        let site = BankAddr::new(bank.clone(), pc.address);
        let subject = observed_executed_code_subject(&site.bank, site.pc);
        let already_concluded = db.conclusion(&subject).is_some();

        let fact_index = db.insert(Fact::ObservedExecutedCode {
            site: site.clone(),
            trace: trace_id.to_string(),
            sequence: *sequence,
        });
        report.facts_added += 1;

        let mut justified_by = db
            .conclusion(&subject)
            .map(|conclusion| conclusion.justified_by.clone())
            .unwrap_or_default();
        justified_by.push(fact_index);
        db.conclude(
            subject,
            ProofState::Supported,
            justified_by,
            "trace_observed_executed_pc",
        )
        .expect(
            "ObservedExecutedCode never proposes Proven, so it can never fail to supersede \
             an existing conclusion for the same subject",
        );

        if already_concluded {
            report.corroborated.insert(site.clone());
        } else {
            report.new_code_existence.insert(site.clone());
        }

        if matches!(
            static_word_class(&site.bank, site.pc),
            Some(WordClass::ProvenData)
        ) {
            report.conflicts.push(StaticDataConflict {
                site,
                trace_id: trace_id.to_string(),
                sequence: *sequence,
            });
        }
    }
    report
}

/// The measured effect of folding one trace's `IndirectTransfer`
/// observations into a [`crate::facts::FactDb`] as
/// [`crate::facts::Fact::ObservedIndirectTarget`] existence evidence.
/// Every count is a direct measurement of what
/// [`fold_indirect_targets_into_fact_db`] did to `db`, never an estimate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectFoldReport {
    /// `Fact::ObservedIndirectTarget` records appended (one per known-bank
    /// site+target observation folded).
    pub facts_added: u64,
    /// `(site -> target)` edges observed for the first time in this fold.
    pub new_edges: BTreeSet<(crate::facts::BankAddr, crate::facts::BankAddr)>,
    /// Edges already concluded (this or an earlier fold) that this
    /// observation corroborates -- a repeat sighting, not a new edge.
    pub corroborated: BTreeSet<(crate::facts::BankAddr, crate::facts::BankAddr)>,
    /// Observations whose target word static analysis had already proven
    /// `ProvenData`. Reported, never resolved by fiat -- an indirect edge
    /// into proven data means the static class or the trace bank identity
    /// is wrong, and that must surface loudly.
    pub target_conflicts: Vec<StaticDataConflict>,
    /// Observations skipped because the site OR target bank was
    /// `BankContext::Unknown`. An edge is only sound when BOTH endpoints
    /// have a known bank; a target in an unknown bank cannot become a
    /// bank-qualified `ObservedIndirectTarget` without inventing identity.
    pub unknown_bank_skipped: u64,
}

/// Fold every known-bank `IndirectTransfer` observation in `facts` into
/// `db` as [`crate::facts::Fact::ObservedIndirectTarget`] existence
/// evidence, and return the exact measured delta.
///
/// This is the trace lane's admission rule for indirect edges, the
/// counterpart of [`fold_executed_pcs_into_fact_db`] for executed PCs. An
/// observed `jr`/`jalr` transfer proves the edge `site -> target`
/// EXISTED at runtime -- exactly the evidence static value-set analysis
/// cannot supply for a load-derived or input-dependent dispatch (the MM
/// audio/camera handler tables the static lanes leave open). It is
/// deliberately weaker than a static `IndirectTransferAnalysis` with
/// `Exhaustive` state: an observation is existence, never exhaustiveness,
/// so it concludes at `ProofState::Supported` and NEVER contributes an
/// exhaustive successor set. Downstream owner admission may treat the
/// target as a proven callable entry (it demonstrably ran), but the
/// site's frontier stays open unless a separate exhaustive proof closes
/// it.
///
/// BOTH endpoints must have a known bank: an edge whose target bank is
/// `Unknown` is skipped (`unknown_bank_skipped`), never promoted, per
/// this module's mandate that unknown-bank identity is preserved rather
/// than guessed. `static_word_class` is the same caller-supplied lookup
/// as the executed-PC fold; a target landing on `ProvenData` is recorded
/// as a conflict, never silently resolved.
///
/// Idempotent per `(site -> target)` edge: the same edge observed twice
/// inserts two facts (distinct provenance) but concludes the edge once,
/// growing its `justified_by` list.
pub fn fold_indirect_targets_into_fact_db(
    db: &mut crate::facts::FactDb,
    trace_id: &str,
    facts: &[ObservedTraceFact],
    static_word_class: impl Fn(&str, u32) -> Option<crate::cfg::WordClass>,
) -> IndirectFoldReport {
    use crate::cfg::WordClass;
    use crate::facts::{observed_indirect_target_subject, BankAddr, Fact, ProofState};

    let mut report = IndirectFoldReport::default();
    for observed in facts {
        let ObservedTraceFact::IndirectTransfer {
            sequence,
            site,
            target,
            ..
        } = observed
        else {
            continue;
        };
        let (BankContext::Known { bank: site_bank, .. }, BankContext::Known { bank: target_bank, .. }) =
            (&site.bank, &target.bank)
        else {
            report.unknown_bank_skipped += 1;
            continue;
        };
        let site_addr = BankAddr::new(site_bank.clone(), site.address);
        let target_addr = BankAddr::new(target_bank.clone(), target.address);
        let subject = observed_indirect_target_subject(
            &site_addr.bank,
            site_addr.pc,
            &target_addr.bank,
            target_addr.pc,
        );
        let already_concluded = db.conclusion(&subject).is_some();

        let fact_index = db.insert(Fact::ObservedIndirectTarget {
            site: site_addr.clone(),
            target: target_addr.clone(),
            trace: trace_id.to_string(),
        });
        report.facts_added += 1;

        let mut justified_by = db
            .conclusion(&subject)
            .map(|conclusion| conclusion.justified_by.clone())
            .unwrap_or_default();
        justified_by.push(fact_index);
        db.conclude(
            subject,
            ProofState::Supported,
            justified_by,
            "trace_observed_indirect_target",
        )
        .expect(
            "ObservedIndirectTarget never proposes Proven, so it can never fail to supersede \
             an existing conclusion for the same subject",
        );

        let edge = (site_addr.clone(), target_addr.clone());
        if already_concluded {
            report.corroborated.insert(edge);
        } else {
            report.new_edges.insert(edge);
        }

        if matches!(
            static_word_class(&target_addr.bank, target_addr.pc),
            Some(WordClass::ProvenData)
        ) {
            report.target_conflicts.push(StaticDataConflict {
                site: target_addr,
                trace_id: trace_id.to_string(),
                sequence: *sequence,
            });
        }
    }
    report
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

    mod fold_into_fact_db {
        use super::*;
        use crate::cfg::WordClass;
        use crate::facts::{observed_executed_code_subject, BankAddr, Fact, FactDb, ProofState};

        fn known_pc(bank: &str, address: u32, activation: u64) -> ObservedAddress {
            ObservedAddress {
                address,
                bank: BankContext::Known {
                    bank: bank.to_string(),
                    activation,
                },
            }
        }

        fn unknown_pc(address: u32) -> ObservedAddress {
            ObservedAddress {
                address,
                bank: BankContext::Unknown,
            }
        }

        fn no_static_class(_bank: &str, _va: u32) -> Option<WordClass> {
            None
        }

        #[test]
        fn known_bank_observation_adds_a_code_existence_fact() {
            let mut db = FactDb::new();
            let facts = vec![ObservedTraceFact::ExecutedPc {
                sequence: 1,
                pc: known_pc("boot", 0x8000_0400, 0),
            }];

            let report =
                fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

            assert_eq!(report.facts_added, 1);
            let site = BankAddr::new("boot", 0x8000_0400);
            assert_eq!(report.new_code_existence, [site.clone()].into());
            assert!(report.corroborated.is_empty());
            assert!(report.conflicts.is_empty());
            assert_eq!(report.unknown_bank_skipped, 0);

            assert_eq!(db.facts().len(), 1);
            assert!(matches!(
                &db.facts()[0],
                Fact::ObservedExecutedCode { site: s, trace, sequence: 1 }
                    if *s == site && trace == "trace-a"
            ));
            let conclusion = db
                .conclusion(&observed_executed_code_subject("boot", 0x8000_0400))
                .unwrap();
            assert_eq!(conclusion.state, ProofState::Supported);
            assert_eq!(conclusion.justified_by, vec![0]);
        }

        #[test]
        fn repeated_observation_of_the_same_pc_adds_one_conclusion_two_provenance_records() {
            let mut db = FactDb::new();
            let facts = vec![
                ObservedTraceFact::ExecutedPc {
                    sequence: 1,
                    pc: known_pc("boot", 0x8000_0400, 0),
                },
                ObservedTraceFact::ExecutedPc {
                    sequence: 2,
                    pc: known_pc("boot", 0x8000_0400, 0),
                },
            ];

            let report =
                fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

            assert_eq!(report.facts_added, 2);
            let site = BankAddr::new("boot", 0x8000_0400);
            // First sighting is new evidence; the second is a corroboration
            // of that same word, not a second new-evidence word.
            assert_eq!(report.new_code_existence, [site.clone()].into());
            assert_eq!(report.corroborated, [site.clone()].into());

            // Two distinct Fact::ObservedExecutedCode records (provenance)...
            assert_eq!(db.facts().len(), 2);
            let sequences: Vec<u64> = db
                .facts()
                .iter()
                .map(|f| match f {
                    Fact::ObservedExecutedCode { sequence, .. } => *sequence,
                    _ => panic!("unexpected fact variant"),
                })
                .collect();
            assert_eq!(sequences, vec![1, 2]);

            // ...but exactly one conclusion for the (bank, pc) subject, whose
            // justified_by names both provenance facts.
            assert_eq!(db.conclusions().count(), 1);
            let conclusion = db
                .conclusion(&observed_executed_code_subject("boot", 0x8000_0400))
                .unwrap();
            assert_eq!(conclusion.justified_by, vec![0, 1]);
        }

        #[test]
        fn observed_executed_word_that_is_statically_proven_data_raises_a_conflict() {
            let mut db = FactDb::new();
            let facts = vec![ObservedTraceFact::ExecutedPc {
                sequence: 7,
                pc: known_pc("boot", 0x8000_0800, 0),
            }];
            let static_class = |bank: &str, va: u32| -> Option<WordClass> {
                (bank == "boot" && va == 0x8000_0800).then_some(WordClass::ProvenData)
            };

            let report = fold_executed_pcs_into_fact_db(&mut db, "trace-b", &facts, static_class);

            assert_eq!(report.facts_added, 1);
            assert_eq!(report.conflicts.len(), 1);
            assert_eq!(report.conflicts[0].site, BankAddr::new("boot", 0x8000_0800));
            assert_eq!(report.conflicts[0].trace_id, "trace-b");
            assert_eq!(report.conflicts[0].sequence, 7);
            // The conflict is surfaced, not resolved: the observation still
            // becomes Supported evidence rather than being dropped, and
            // nothing here mutates or removes the static ProvenData claim
            // (which this test's closure stands in for -- it is never
            // touched by the adapter at all).
            let conclusion = db
                .conclusion(&observed_executed_code_subject("boot", 0x8000_0800))
                .unwrap();
            assert_eq!(conclusion.state, ProofState::Supported);
        }

        #[test]
        fn unknown_bank_observation_adds_nothing() {
            let mut db = FactDb::new();
            let facts = vec![ObservedTraceFact::ExecutedPc {
                sequence: 1,
                pc: unknown_pc(0x8000_0400),
            }];

            let report =
                fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

            assert_eq!(report.facts_added, 0);
            assert_eq!(report.unknown_bank_skipped, 1);
            assert!(report.new_code_existence.is_empty());
            assert!(report.corroborated.is_empty());
            assert!(report.conflicts.is_empty());
            assert!(db.facts().is_empty());
            assert_eq!(db.conclusions().count(), 0);
        }

        #[test]
        fn non_executed_pc_facts_are_left_untouched() {
            let mut db = FactDb::new();
            let facts = vec![ObservedTraceFact::PiDma {
                sequence: 1,
                direction: PiDmaDirection::CartToRdram,
                cart_address: 0x1000_0000,
                dram_address: 0x400,
                byte_len: 64,
                active_bank: BankContext::Unknown,
            }];

            let report =
                fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

            assert_eq!(report.facts_added, 0);
            assert_eq!(report.unknown_bank_skipped, 0);
            assert!(db.facts().is_empty());
        }
    }

    mod pi_dma_fold {
        use super::*;
        use crate::facts::{Fact, FactDb, ProofState, RomAddressSpace};

        /// `rom_offset` is a ROM file offset; the observation carries the
        /// cart-BUS address the hardware saw, so it is offset into PI cart
        /// domain 1. Keeping the test inputs in bus space is the point -- the
        /// translation is what a live capture proved was missing.
        fn dma(sequence: u64, rom_offset: u32, dram: u32, len: u32) -> ObservedTraceFact {
            ObservedTraceFact::PiDma {
                sequence,
                direction: PiDmaDirection::CartToRdram,
                cart_address: PI_CART_DOMAIN1_BASE + rom_offset,
                dram_address: dram,
                byte_len: len,
                active_bank: BankContext::Unknown,
            }
        }

        fn prove_mapping(db: &mut FactDb, bank: &str, rom_start: u32, va_start: u32, len: u32) {
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.to_string(),
                rom_space: RomAddressSpace::Physical,
                rom_start,
                rom_end: rom_start + len,
                va_start,
                va_end: va_start + len,
            });
            db.conclude(format!("bank:{bank}"), ProofState::Proven, vec![mapping], "test")
                .unwrap();
        }

        #[test]
        fn a_cart_to_rdram_transfer_becomes_a_supported_mapping() {
            let mut db = FactDb::new();
            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(1, 0x10_0000, 0x40_0000, 0x2000)]);

            assert_eq!(report.facts_added, 1);
            assert_eq!(report.new_mappings.len(), 1);
            let bank = report.new_mappings.iter().next().unwrap();
            let conclusion = db.conclusion(&format!("bank:{bank}")).unwrap();
            // The whole point of the evidence class: observed, never proven.
            assert_eq!(conclusion.state, ProofState::Supported);
            assert!(
                db.proven_rom_mappings().is_empty(),
                "an observation must not appear as a proven mapping"
            );
            let mapping = db
                .facts()
                .iter()
                .find_map(|fact| match fact {
                    Fact::RomMapping { va_start, rom_start, .. } => Some((*rom_start, *va_start)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(mapping, (0x10_0000, 0x8040_0000), "KSEG0 destination");
        }

        #[test]
        fn a_reloaded_overlay_concludes_once_and_counts_the_repeat() {
            // Games reload the same overlay constantly. That is one mapping and
            // N sightings, not N mappings.
            let mut db = FactDb::new();
            let facts = vec![
                dma(1, 0x10_0000, 0x40_0000, 0x2000),
                dma(9, 0x10_0000, 0x40_0000, 0x2000),
                dma(17, 0x10_0000, 0x40_0000, 0x2000),
            ];
            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

            assert_eq!(report.facts_added, 1);
            assert_eq!(report.repeated, 2);
            assert_eq!(report.new_mappings.len(), 1);
        }

        #[test]
        fn an_observation_matching_a_proven_mapping_corroborates_instead_of_duplicating() {
            // This is the case worth having: an independent producer agreeing
            // with static composition is corroboration static analysis cannot
            // give itself.
            let mut db = FactDb::new();
            prove_mapping(&mut db, "code", 0x10_0000, 0x8040_0000, 0x2000);

            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(1, 0x10_0000, 0x40_0000, 0x2000)]);

            assert_eq!(report.facts_added, 0, "no duplicate mapping was added");
            assert!(report.new_mappings.is_empty());
            assert_eq!(
                report.corroborated.iter().cloned().collect::<Vec<_>>(),
                vec!["code".to_string()]
            );
            assert!(report.conflicts.is_empty());
        }

        #[test]
        fn an_observation_contradicting_a_proven_mapping_is_reported_not_resolved() {
            let mut db = FactDb::new();
            prove_mapping(&mut db, "code", 0x10_0000, 0x8040_0000, 0x2000);

            // Same VA, different ROM source: one of the two is wrong.
            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(4, 0x99_0000, 0x40_0000, 0x2000)]);

            assert_eq!(report.conflicts.len(), 1);
            let conflict = &report.conflicts[0];
            assert_eq!(conflict.va_start, 0x8040_0000);
            assert_eq!(conflict.observed_rom_start, 0x99_0000);
            assert_eq!(conflict.proven_rom_start, 0x10_0000);
            assert_eq!(conflict.proven_bank, "code");
            assert_eq!(report.facts_added, 0, "a conflict must not be silently admitted");
        }

        #[test]
        fn a_transfer_from_another_pi_device_is_counted_not_mistranslated() {
            // 0x0500_0000 is the 64DD, 0x1FC0_0000 is PIF ROM. Neither is this
            // cartridge, so subtracting the domain-1 base would invent a ROM
            // offset for bytes that never came from the ROM.
            let mut db = FactDb::new();
            let facts = vec![
                ObservedTraceFact::PiDma {
                    sequence: 1,
                    direction: PiDmaDirection::CartToRdram,
                    cart_address: 0x0500_0000,
                    dram_address: 0x40_0000,
                    byte_len: 0x1000,
                    active_bank: BankContext::Unknown,
                },
                ObservedTraceFact::PiDma {
                    sequence: 2,
                    direction: PiDmaDirection::CartToRdram,
                    cart_address: 0x1FC0_0000,
                    dram_address: 0x40_0000,
                    byte_len: 0x1000,
                    active_bank: BankContext::Unknown,
                },
            ];
            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

            assert_eq!(report.off_cartridge_skipped, 2);
            assert_eq!(report.facts_added, 0);
        }

        #[test]
        fn the_cart_bus_address_is_translated_to_a_rom_offset() {
            // The live-capture bug: the IPL3 boot copy reads cart 0x10001000,
            // which is ROM offset 0x1000. Folding the bus address verbatim put
            // every mapping outside the image and made corroboration with the
            // proven boot bank impossible.
            let mut db = FactDb::new();
            prove_mapping(&mut db, "boot", 0x1000, 0x8000_0400, 0x10_0000);

            let report = fold_pi_dmas_into_fact_db(
                &mut db,
                "t",
                &[dma(1, 0x1000, 0x400, 0x10_0000)],
            );

            assert!(
                report.conflicts.is_empty(),
                "the boot copy must corroborate, not conflict: {:?}",
                report.conflicts
            );
            assert_eq!(
                report.corroborated.iter().cloned().collect::<Vec<_>>(),
                vec!["boot".to_string()]
            );
        }

        #[test]
        fn write_back_and_degenerate_transfers_are_skipped_and_counted() {
            let mut db = FactDb::new();
            let facts = vec![
                ObservedTraceFact::PiDma {
                    sequence: 1,
                    direction: PiDmaDirection::RdramToCart,
                    cart_address: 0x10_0000,
                    dram_address: 0x40_0000,
                    byte_len: 0x2000,
                    active_bank: BankContext::Unknown,
                },
                dma(2, 0x10_0000, 0x40_0000, 0),
                // Already at the top of the address space: end overflows u32.
                ObservedTraceFact::PiDma {
                    sequence: 3,
                    direction: PiDmaDirection::CartToRdram,
                    cart_address: 0x1FBF_FFFF,
                    dram_address: 0x40_0000,
                    byte_len: 0xFFFF_FFFF,
                    active_bank: BankContext::Unknown,
                },
            ];
            let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

            assert_eq!(report.non_load_skipped, 1, "a write-back is not a load image");
            assert_eq!(report.degenerate_skipped, 2);
            assert_eq!(report.facts_added, 0);
        }
    }

    mod indirect_fold {
        use super::*;
        use crate::cfg::WordClass;
        use crate::facts::{
            observed_indirect_target_subject, BankAddr, Fact, FactDb, ProofState,
        };

        fn known(bank: &str, address: u32) -> ObservedAddress {
            ObservedAddress {
                address,
                bank: BankContext::Known {
                    bank: bank.to_string(),
                    activation: 0,
                },
            }
        }

        fn unknown(address: u32) -> ObservedAddress {
            ObservedAddress {
                address,
                bank: BankContext::Unknown,
            }
        }

        fn edge(site: ObservedAddress, target: ObservedAddress) -> ObservedTraceFact {
            ObservedTraceFact::IndirectTransfer {
                sequence: 1,
                kind: IndirectTransferKind::Call,
                site,
                target,
            }
        }

        fn no_class(_bank: &str, _va: u32) -> Option<WordClass> {
            None
        }

        #[test]
        fn known_edge_becomes_a_supported_observed_indirect_target() {
            let mut db = FactDb::new();
            let facts = vec![edge(
                known("code", 0x8019_3efc),
                known("code", 0x8019_4000),
            )];
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);

            assert_eq!(report.facts_added, 1);
            let site = BankAddr::new("code", 0x8019_3efc);
            let target = BankAddr::new("code", 0x8019_4000);
            assert_eq!(report.new_edges, [(site.clone(), target.clone())].into());
            assert!(report.corroborated.is_empty());
            assert!(report.target_conflicts.is_empty());
            assert_eq!(report.unknown_bank_skipped, 0);
            assert!(matches!(
                &db.facts()[0],
                Fact::ObservedIndirectTarget { site: s, target: t, trace }
                    if *s == site && *t == target && trace == "trace-a"
            ));
            let subject = observed_indirect_target_subject("code", 0x8019_3efc, "code", 0x8019_4000);
            assert_eq!(db.conclusion(&subject).unwrap().state, ProofState::Supported);
        }

        #[test]
        fn same_edge_twice_corroborates_one_conclusion_with_two_facts() {
            let mut db = FactDb::new();
            let facts = vec![
                edge(known("code", 0x100), known("code", 0x200)),
                edge(known("code", 0x100), known("code", 0x200)),
            ];
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);

            assert_eq!(report.facts_added, 2);
            assert_eq!(report.new_edges.len(), 1);
            assert_eq!(report.corroborated.len(), 1);
            let subject = observed_indirect_target_subject("code", 0x100, "code", 0x200);
            assert_eq!(db.conclusion(&subject).unwrap().justified_by, vec![0, 1]);
        }

        #[test]
        fn one_site_two_targets_are_two_distinct_edges() {
            let mut db = FactDb::new();
            let facts = vec![
                edge(known("code", 0x100), known("code", 0x200)),
                edge(known("code", 0x100), known("code", 0x300)),
            ];
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
            // Existence, not exhaustiveness: both edges kept, never merged.
            assert_eq!(report.new_edges.len(), 2);
            assert_eq!(report.facts_added, 2);
        }

        #[test]
        fn unknown_target_bank_is_skipped_not_invented() {
            let mut db = FactDb::new();
            let facts = vec![edge(known("code", 0x100), unknown(0x8020_0000))];
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
            assert_eq!(report.facts_added, 0);
            assert_eq!(report.unknown_bank_skipped, 1);
            assert!(db.facts().is_empty());
        }

        #[test]
        fn unknown_site_bank_is_also_skipped() {
            let mut db = FactDb::new();
            let facts = vec![edge(unknown(0x100), known("code", 0x200))];
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
            assert_eq!(report.facts_added, 0);
            assert_eq!(report.unknown_bank_skipped, 1);
        }

        #[test]
        fn target_on_proven_data_is_reported_as_conflict() {
            let mut db = FactDb::new();
            let facts = vec![edge(known("code", 0x100), known("code", 0x200))];
            let class = |bank: &str, va: u32| {
                (bank == "code" && va == 0x200).then_some(WordClass::ProvenData)
            };
            let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, class);
            // The edge is still recorded (the observation happened); the
            // conflict is surfaced, never silently resolved.
            assert_eq!(report.facts_added, 1);
            assert_eq!(report.target_conflicts.len(), 1);
            assert_eq!(report.target_conflicts[0].site, BankAddr::new("code", 0x200));
        }
    }
}

/// A proven static mapping and an observed DMA disagree about what backs a VA.
///
/// Reported, never resolved by fiat: one of the two is wrong, and which one is
/// a question this adapter has no standing to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedMappingConflict {
    pub trace_id: String,
    pub sequence: u64,
    pub va_start: u32,
    pub observed_rom_start: u32,
    pub proven_bank: String,
    pub proven_rom_start: u32,
}

/// The measured effect of folding one trace's `PiDma` observations into a
/// [`crate::facts::FactDb`] as observed load-image mappings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiDmaFoldReport {
    /// `Fact::RomMapping` records appended (one per distinct transfer).
    pub facts_added: u64,
    /// Banks concluded from a transfer no proven mapping already described.
    pub new_mappings: BTreeSet<String>,
    /// Transfers whose geometry a proven static mapping already asserts. These
    /// add no reach, and they are the valuable ones: an independent producer
    /// agreeing with static composition is corroboration static analysis
    /// cannot give itself.
    pub corroborated: BTreeSet<String>,
    /// Transfers that land on a VA a proven mapping backs from a DIFFERENT ROM
    /// offset.
    pub conflicts: Vec<ObservedMappingConflict>,
    /// Distinct transfers seen more than once (an overlay reloaded). Counted,
    /// not re-concluded.
    pub repeated: u64,
    /// `FromRdram` transfers. A write-back is not a load image.
    pub non_load_skipped: u64,
    /// Zero-length transfers, or ones whose end address overflows `u32`.
    pub degenerate_skipped: u64,
    /// Transfers whose source is not PI cartridge domain 1 -- the 64DD or PIF
    /// ROM, which are different devices, not this cartridge. Counted so they
    /// are visibly excluded rather than silently mistranslated.
    pub off_cartridge_skipped: u64,
}

/// Fold every cart-to-RDRAM `PiDma` observation into `db` as an observed
/// load-image mapping, and return the exact measured delta.
///
/// **Why this is the one universal composition mechanism.** Every static
/// strategy has to recognise a structure -- a dmadata-shaped file table, an
/// AKI descriptor table -- and is therefore bound to the engine families whose
/// structure it knows. Static PI-DMA operand slicing does not escape that: at
/// the interesting call sites the vrom/size operands are READ FROM the very
/// table the scan is trying to avoid needing, so it cannot bootstrap (measured
/// negative, see `DISCOVER-PLAN`). An observed DMA has no such circularity. It
/// does not matter whether the address came from a table, a decompressor, a
/// TLB-mapped pointer, or a custom IPL3 -- by the time the transfer completes,
/// the geometry is a fact.
///
/// **Evidence class.** `Supported`, never `Proven`. A completed transfer shows
/// bytes moved cart->RDRAM at one moment of one run; it does not show the
/// destination is code, that the mapping is stable, or that the set of
/// transfers observed is exhaustive. `headless.rs` requires a genuinely
/// completed DMA for this record -- a register write or a DMA-start
/// notification is explicitly not sufficient -- so the observation is sound
/// even though the conclusion drawn from it is bounded.
///
/// Deduplicated by `(cart, dram, len)`: a game reloading the same overlay
/// yields one mapping and a `repeated` count, not N conclusions.
/// PI cartridge domain 1: the address window a game's ROM is visible at on the
/// PI bus. An observation carries the address the hardware saw, which is a
/// cart-bus address; a `RomMapping` wants a ROM file offset. Domain 2
/// (0x0500_0000, 64DD) and PIF ROM (0x1FC0_0000) are different devices and are
/// NOT this ROM, so a transfer from them is reported rather than translated.
const PI_CART_DOMAIN1_BASE: u32 = 0x1000_0000;
const PI_CART_DOMAIN1_END: u32 = 0x1FC0_0000;

/// Translate a PI cart-bus address to a ROM file offset, or `None` when the
/// transfer's source is not this cartridge.
fn cart_address_to_rom_offset(cart_address: u32) -> Option<u32> {
    (PI_CART_DOMAIN1_BASE..PI_CART_DOMAIN1_END)
        .contains(&cart_address)
        .then(|| cart_address - PI_CART_DOMAIN1_BASE)
}

pub fn fold_pi_dmas_into_fact_db(
    db: &mut crate::facts::FactDb,
    trace_id: &str,
    facts: &[ObservedTraceFact],
) -> PiDmaFoldReport {
    use crate::facts::{BankAddr, Fact, ProofState, RomAddressSpace};
    use std::collections::BTreeMap;

    let mut report = PiDmaFoldReport::default();
    let mut seen: BTreeSet<(u32, u32, u32)> = BTreeSet::new();

    // Proven VA -> (bank, rom_start), so an observation can be checked against
    // static composition instead of silently duplicating or contradicting it.
    let proven: BTreeMap<u32, (String, u32)> = db
        .proven_rom_mappings()
        .iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                va_start,
                ..
            } => Some((*va_start, (bank.clone(), *rom_start))),
            _ => None,
        })
        .collect();

    for observed in facts {
        let ObservedTraceFact::PiDma {
            sequence,
            direction,
            cart_address,
            dram_address,
            byte_len,
            ..
        } = observed
        else {
            continue;
        };
        if *direction != PiDmaDirection::CartToRdram {
            report.non_load_skipped += 1;
            continue;
        }
        let (Some(_), Some(dram_end)) = (
            cart_address.checked_add(*byte_len),
            dram_address.checked_add(*byte_len),
        ) else {
            report.degenerate_skipped += 1;
            continue;
        };
        if *byte_len == 0 {
            report.degenerate_skipped += 1;
            continue;
        }
        // The observation records a cart-BUS address; a mapping wants a ROM
        // file offset. Without this the boot copy reads as ROM 0x10001000
        // instead of 0x1000 and every mapping lands outside the image --
        // caught by a live capture conflicting with the proven boot bank.
        let Some(rom_offset) = cart_address_to_rom_offset(*cart_address) else {
            report.off_cartridge_skipped += 1;
            continue;
        };
        if !seen.insert((rom_offset, *dram_address, *byte_len)) {
            report.repeated += 1;
            continue;
        }

        // KSEG0 is the address the guest executes a loaded image at; the DMA
        // destination is the physical RDRAM offset.
        let va_start = 0x8000_0000 | dram_address;
        let va_end = 0x8000_0000 | dram_end;
        let rom_end = rom_offset + byte_len;
        let bank = format!("observed_dma_{rom_offset:08x}_{dram_address:08x}_{byte_len:x}");

        match proven.get(&va_start) {
            Some((proven_bank, proven_rom_start)) if *proven_rom_start == rom_offset => {
                report.corroborated.insert(proven_bank.clone());
                continue;
            }
            Some((proven_bank, proven_rom_start)) => {
                report.conflicts.push(ObservedMappingConflict {
                    trace_id: trace_id.to_string(),
                    sequence: *sequence,
                    va_start,
                    observed_rom_start: rom_offset,
                    proven_bank: proven_bank.clone(),
                    proven_rom_start: *proven_rom_start,
                });
                continue;
            }
            None => {}
        }

        let mapping = db.insert(Fact::RomMapping {
            bank: bank.clone(),
            rom_space: RomAddressSpace::Physical,
            rom_start: rom_offset,
            rom_end,
            va_start,
            va_end,
        });
        let evidence = db.insert(Fact::Evidence {
            subject: BankAddr::new(&bank, va_start),
            note: format!(
                "observed PI DMA (trace {trace_id}, seq {sequence}): cart \
                 0x{cart_address:x} (ROM 0x{rom_offset:x})+0x{byte_len:x} -> RDRAM 0x{dram_address:x} \
                 (VA 0x{va_start:x}). One completed transfer in one run; Supported, \
                 not Proven -- neither exhaustive nor evidence the image is code."
            ),
        });
        report.facts_added += 1;
        db.conclude(
            format!("bank:{bank}"),
            ProofState::Supported,
            vec![mapping, evidence],
            "trace_observed_pi_dma",
        )
        .expect("observed-DMA bank names encode their own geometry and never collide");
        report.new_mappings.insert(bank);
    }
    report
}
