//! Strict candidate-only interchange for external discovery tools.
//!
//! This module parses an already-produced JSONL stream. It never launches a
//! process and never accepts ROM bytes. The caller supplies the exact
//! bank-local input identity and lineage it expected the tool to consume; a
//! stale, partial, unqualified, out-of-bank, resource-truncated, or unknown
//! stream is rejected rather than weakened into a guess.
//!
//! External tools are candidate providers only. The output type has a
//! one-variant [`CandidateProofCeiling`], so this adapter cannot express a
//! supported or proven conclusion even if an input attempts to claim one
//! (unknown JSON fields are rejected).

use crate::facts::BankAddr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const TOOL_ADAPTER_SCHEMA: &str = "fn64.tool-adapter";
pub const TOOL_ADAPTER_SCHEMA_VERSION: u32 = 1;
pub const TOOL_ADAPTER_SCHEMA_VERSION_V2: u32 = 2;
pub const TOOL_ADAPTER_SCHEMA_VERSION_V3: u32 = 3;

/// A SHA-256 value serialized as exactly 64 lowercase hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Sha256Digest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
        out
    }

    pub fn from_hex(value: &str) -> Result<Self, &'static str> {
        if value.len() != 64 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
            return Err("SHA-256 must contain exactly 64 hexadecimal digits");
        }
        if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err("SHA-256 must use canonical lowercase hexadecimal");
        }
        let mut out = [0u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            out[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
        }
        Ok(Self(out))
    }
}

fn hex_nibble(byte: u8) -> Result<u8, &'static str> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("invalid lowercase hexadecimal digit"),
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}

/// Exact identity of the immutable bank slice supplied to a tool. The bank
/// name distinguishes mutually-exclusive images that occupy the same VAs;
/// the mapping digest distinguishes revisions of that bank's load mapping.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BankInputIdentity {
    pub normalized_rom_sha256: Sha256Digest,
    pub bank: String,
    pub bank_bytes_sha256: Sha256Digest,
    pub mapping_sha256: Sha256Digest,
    pub va_start: u32,
    pub va_end: u32,
}

impl BankInputIdentity {
    fn validate(&self) -> Result<(), AdapterError> {
        validate_token("bank", &self.bank, 128)?;
        if self.va_start >= self.va_end
            || !self.va_start.is_multiple_of(4)
            || !self.va_end.is_multiple_of(4)
        {
            return Err(AdapterError::InvalidBankIdentity);
        }
        Ok(())
    }

    fn byte_len(&self) -> u64 {
        u64::from(self.va_end - self.va_start)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRunRole {
    FunctionBoundaryCandidates,
    ControlFlowCandidates,
    RegionCandidates,
    SymbolCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLineageRole {
    DiscoverySnapshot,
    EvidenceManifest,
    ParentToolRun,
    ToolConfiguration,
    /// Digest of the provider-native output after format-level canonical
    /// decoding but before conversion into fn64 claims. This preserves the
    /// external artifact's lineage without making it authoritative.
    ProviderOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolLineageRef {
    pub role: ToolLineageRole,
    pub source_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolIdentity {
    pub name: String,
    pub version: String,
    /// Digest of the executable/script/package content when available. This
    /// is data lineage, not permission to execute it.
    pub build_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BankRange {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
}

/// Ghidra and similar providers can recover concrete targets for a computed
/// transfer, but their absence of additional references is not an
/// exhaustiveness proof. Keeping this enum one-variant makes that ceiling a
/// wire-level invariant instead of a convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputedFlowCompleteness {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCandidateKind {
    FunctionEntry {
        address: BankAddr,
    },
    FunctionExtent {
        range: BankRange,
    },
    /// One exact contiguous component of a provider's function body. Unlike
    /// `FunctionExtent`, this does not claim that the component is the whole
    /// function or that gaps between components are executable body bytes.
    FunctionBodyRange {
        entry: BankAddr,
        range: BankRange,
    },
    ExecutableRange {
        range: BankRange,
    },
    DataRange {
        range: BankRange,
    },
    SymbolAlias {
        address: BankAddr,
        alias: String,
    },
    /// One bank-local computed-control-flow site and the targets recovered by
    /// the provider. An empty target vector still records the independently
    /// observed site. Targets are candidates and never imply completeness.
    ComputedControlFlow {
        site: BankAddr,
        via_call: bool,
        targets: Vec<BankAddr>,
        completeness: ComputedFlowCompleteness,
    },
}

/// One provider claim record before canonical semantic deduplication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolClaimRecord {
    pub sequence: u64,
    pub provider_claim_id: String,
    pub claim: ToolCandidateKind,
}

/// Resource use reported by the provider. A set `limit_hit` makes the run
/// partial and therefore rejects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResourceDiagnostics {
    pub input_bytes: u64,
    pub elapsed_millis: u64,
    pub peak_memory_bytes: Option<u64>,
    pub limit_hit: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRunSummary {
    pub complete: bool,
    pub analyzed_range: BankRange,
    pub skipped_ranges: Vec<BankRange>,
    pub claim_records: u64,
    /// Digest returned by [`canonical_claim_records_sha256`].
    pub claims_sha256: Sha256Digest,
    pub resources: ToolResourceDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRunHeader {
    schema: String,
    schema_version: u32,
    tool: ToolIdentity,
    role: ToolRunRole,
    input: BankInputIdentity,
    lineage: Vec<ToolLineageRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
enum WireRecord {
    Header {
        schema: String,
        schema_version: u32,
        tool: ToolIdentity,
        role: ToolRunRole,
        input: BankInputIdentity,
        lineage: Vec<ToolLineageRef>,
    },
    Claim {
        sequence: u64,
        provider_claim_id: String,
        claim: ToolCandidateKind,
    },
    Summary {
        complete: bool,
        analyzed_range: BankRange,
        skipped_ranges: Vec<BankRange>,
        claim_records: u64,
        claims_sha256: Sha256Digest,
        resources: ToolResourceDiagnostics,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateProofCeiling {
    Candidate,
}

/// A canonical semantic candidate. Duplicate provider records are merged but
/// every provider claim ID and sequence remains attached as lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCandidate {
    pub kind: ToolCandidateKind,
    pub proof_ceiling: CandidateProofCeiling,
    pub provider_claim_ids: Vec<String>,
    pub source_sequences: Vec<u64>,
}

/// Immutable lineage for a successfully ingested run. `source_sha256` is
/// computed from canonical header/lineage/candidate semantics; JSONL line
/// order and duplicate claim records do not change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRunSource {
    pub source_sha256: Sha256Digest,
    pub schema_version: u32,
    pub tool: ToolIdentity,
    pub role: ToolRunRole,
    pub input: BankInputIdentity,
    pub lineage: Vec<ToolLineageRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAdapterOutput {
    source: ToolRunSource,
    candidates: Vec<ToolCandidate>,
    summary: ToolRunSummary,
}

impl ToolAdapterOutput {
    pub fn source(&self) -> &ToolRunSource {
        &self.source
    }

    pub fn candidates(&self) -> &[ToolCandidate] {
        &self.candidates
    }

    pub fn summary(&self) -> &ToolRunSummary {
        &self.summary
    }
}

/// Exact expectations and parser resource bounds supplied by fn64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAdapterExpectation {
    pub input: BankInputIdentity,
    pub role: ToolRunRole,
    /// Required lineage is exact after canonical sort/dedup. This prevents a
    /// tool result from an older discovery snapshot being accepted merely
    /// because it analyzed the same byte slice.
    pub lineage: Vec<ToolLineageRef>,
    pub limits: AdapterLimits,
}

/// Complete candidate-provider run ready for deterministic JSONL export.
/// Provider-specific adapters construct this value only after validating
/// their native output. The generic exporter validates its own stream through
/// [`ingest_tool_jsonl`] before returning it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteToolRun {
    pub tool: ToolIdentity,
    pub role: ToolRunRole,
    pub input: BankInputIdentity,
    pub lineage: Vec<ToolLineageRef>,
    pub claims: Vec<ToolClaimRecord>,
    pub resources: ToolResourceDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterLimits {
    pub max_total_bytes: usize,
    pub max_line_bytes: usize,
    pub max_claim_records: usize,
    pub max_lineage_entries: usize,
    pub max_warnings: usize,
}

impl Default for AdapterLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 64 * 1024 * 1024,
            max_line_bytes: 1024 * 1024,
            max_claim_records: 1_000_000,
            max_lineage_entries: 1024,
            max_warnings: 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    InvalidLimit(&'static str),
    InputTooLarge {
        bytes: usize,
        limit: usize,
    },
    LineTooLarge {
        line: usize,
        bytes: usize,
        limit: usize,
    },
    BlankLine {
        line: usize,
    },
    InvalidJson {
        line: usize,
        detail: String,
    },
    HeaderNotFirst {
        line: usize,
    },
    DuplicateHeader {
        line: usize,
    },
    MissingHeader,
    SummaryNotLast {
        line: usize,
    },
    DuplicateSummary {
        line: usize,
    },
    MissingSummary,
    UnknownSchema {
        schema: String,
        version: u32,
    },
    InvalidBankIdentity,
    InvalidToolIdentity,
    StaleInput,
    UnexpectedRole {
        expected: ToolRunRole,
        actual: ToolRunRole,
    },
    StaleLineage,
    TooMuchLineage {
        count: usize,
        limit: usize,
    },
    TooManyClaims {
        count: usize,
        limit: usize,
    },
    DuplicateSequence(u64),
    MissingSequence {
        expected: u64,
        actual: u64,
    },
    DuplicateProviderClaimId(String),
    InvalidProviderClaimId {
        sequence: u64,
    },
    BodyRangeWithoutFunctionEntry {
        sequence: u64,
        bank: String,
        entry: u32,
    },
    WrongClaimRole {
        sequence: u64,
        role: ToolRunRole,
    },
    UnqualifiedOrWrongBank {
        sequence: u64,
        bank: String,
    },
    OutOfBank {
        sequence: u64,
        start: u32,
        end: u32,
    },
    UnalignedCodeClaim {
        sequence: u64,
        start: u32,
        end: u32,
    },
    InvalidAlias {
        sequence: u64,
    },
    NonCanonicalComputedTargets {
        sequence: u64,
    },
    PartialRun,
    IncompleteAnalyzedRange,
    ClaimCountMismatch {
        summary: u64,
        actual: usize,
    },
    ClaimDigestMismatch,
    ResourceLimitHit,
    ResourceInputMismatch {
        summary: u64,
        expected: u64,
    },
    TooManyWarnings {
        count: usize,
        limit: usize,
    },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimit(name) => write!(f, "invalid tool-adapter limit: {name}"),
            Self::InputTooLarge { bytes, limit } => {
                write!(f, "tool JSONL is {bytes} bytes, exceeding limit {limit}")
            }
            Self::LineTooLarge { line, bytes, limit } => write!(
                f,
                "tool JSONL line {line} is {bytes} bytes, exceeding limit {limit}"
            ),
            Self::BlankLine { line } => write!(f, "blank tool JSONL line {line}"),
            Self::InvalidJson { line, detail } => {
                write!(f, "invalid tool JSONL record at line {line}: {detail}")
            }
            Self::HeaderNotFirst { line } => write!(f, "tool header is not line one (line {line})"),
            Self::DuplicateHeader { line } => write!(f, "duplicate tool header at line {line}"),
            Self::MissingHeader => write!(f, "tool JSONL has no header"),
            Self::SummaryNotLast { line } => {
                write!(f, "record follows tool summary at line {line}")
            }
            Self::DuplicateSummary { line } => write!(f, "duplicate tool summary at line {line}"),
            Self::MissingSummary => write!(f, "tool JSONL has no summary"),
            Self::UnknownSchema { schema, version } => {
                write!(f, "unknown tool schema {schema:?} version {version}")
            }
            Self::InvalidBankIdentity => write!(f, "invalid bank-local input identity"),
            Self::InvalidToolIdentity => write!(f, "invalid tool identity"),
            Self::StaleInput => write!(f, "tool header input does not match expected bank identity"),
            Self::UnexpectedRole { expected, actual } => {
                write!(f, "tool role {actual:?} does not match expected {expected:?}")
            }
            Self::StaleLineage => write!(f, "tool lineage does not match expected lineage"),
            Self::TooMuchLineage { count, limit } => {
                write!(f, "tool lineage has {count} entries, exceeding limit {limit}")
            }
            Self::TooManyClaims { count, limit } => {
                write!(f, "tool emitted {count} claims, exceeding limit {limit}")
            }
            Self::DuplicateSequence(sequence) => write!(f, "duplicate claim sequence {sequence}"),
            Self::MissingSequence { expected, actual } => write!(
                f,
                "claim sequence is incomplete: expected {expected}, found {actual}"
            ),
            Self::DuplicateProviderClaimId(id) => {
                write!(f, "duplicate provider claim ID {id:?}")
            }
            Self::InvalidProviderClaimId { sequence } => {
                write!(f, "invalid provider claim ID at sequence {sequence}")
            }
            Self::BodyRangeWithoutFunctionEntry {
                sequence,
                bank,
                entry,
            } => write!(
                f,
                "function-body range at sequence {sequence} has no matching function-entry claim for {bank}:0x{entry:08x}"
            ),
            Self::WrongClaimRole { sequence, role } => {
                write!(f, "claim sequence {sequence} is invalid for role {role:?}")
            }
            Self::UnqualifiedOrWrongBank { sequence, bank } => write!(
                f,
                "claim sequence {sequence} names unexpected or empty bank {bank:?}"
            ),
            Self::OutOfBank { sequence, start, end } => write!(
                f,
                "claim sequence {sequence} range [0x{start:08x},0x{end:08x}) is outside the expected bank"
            ),
            Self::UnalignedCodeClaim { sequence, start, end } => write!(
                f,
                "claim sequence {sequence} code range [0x{start:08x},0x{end:08x}) is not word-aligned"
            ),
            Self::InvalidAlias { sequence } => {
                write!(f, "claim sequence {sequence} has an invalid symbol alias")
            }
            Self::NonCanonicalComputedTargets { sequence } => write!(
                f,
                "computed-flow claim sequence {sequence} targets are not strictly sorted and unique"
            ),
            Self::PartialRun => write!(f, "tool reports a partial run or skipped ranges"),
            Self::IncompleteAnalyzedRange => {
                write!(f, "tool did not analyze the exact expected bank range")
            }
            Self::ClaimCountMismatch { summary, actual } => write!(
                f,
                "tool summary claims {summary} records but stream contains {actual}"
            ),
            Self::ClaimDigestMismatch => write!(f, "tool claim-record digest does not match summary"),
            Self::ResourceLimitHit => write!(f, "tool reports hitting a resource limit"),
            Self::ResourceInputMismatch { summary, expected } => write!(
                f,
                "tool reports {summary} input bytes but expected bank has {expected}"
            ),
            Self::TooManyWarnings { count, limit } => {
                write!(f, "tool reports {count} warnings, exceeding limit {limit}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

/// Parse and validate one complete external-tool JSONL result.
pub fn ingest_tool_jsonl(
    jsonl: &str,
    expectation: &ToolAdapterExpectation,
) -> Result<ToolAdapterOutput, AdapterError> {
    validate_limits(expectation.limits)?;
    expectation.input.validate()?;
    if jsonl.len() > expectation.limits.max_total_bytes {
        return Err(AdapterError::InputTooLarge {
            bytes: jsonl.len(),
            limit: expectation.limits.max_total_bytes,
        });
    }
    let expected_lineage = canonical_lineage(&expectation.lineage);
    if expected_lineage.len() > expectation.limits.max_lineage_entries {
        return Err(AdapterError::TooMuchLineage {
            count: expected_lineage.len(),
            limit: expectation.limits.max_lineage_entries,
        });
    }

    let mut header: Option<ToolRunHeader> = None;
    let mut claims = Vec::new();
    let mut summary: Option<ToolRunSummary> = None;
    let mut nonempty_records = 0usize;

    for (zero_line, line) in jsonl.lines().enumerate() {
        let line_number = zero_line + 1;
        if line.is_empty() || line.trim().is_empty() {
            return Err(AdapterError::BlankLine { line: line_number });
        }
        if line.len() > expectation.limits.max_line_bytes {
            return Err(AdapterError::LineTooLarge {
                line: line_number,
                bytes: line.len(),
                limit: expectation.limits.max_line_bytes,
            });
        }
        if summary.is_some() {
            return Err(AdapterError::SummaryNotLast { line: line_number });
        }
        let record: WireRecord =
            serde_json::from_str(line).map_err(|error| AdapterError::InvalidJson {
                line: line_number,
                detail: error.to_string(),
            })?;
        nonempty_records += 1;
        match record {
            WireRecord::Header {
                schema,
                schema_version,
                tool,
                role,
                input,
                lineage,
            } => {
                if nonempty_records != 1 {
                    return Err(AdapterError::HeaderNotFirst { line: line_number });
                }
                if header.is_some() {
                    return Err(AdapterError::DuplicateHeader { line: line_number });
                }
                if schema != TOOL_ADAPTER_SCHEMA
                    || (schema_version != TOOL_ADAPTER_SCHEMA_VERSION
                        && schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V2
                        && schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3)
                {
                    return Err(AdapterError::UnknownSchema {
                        schema,
                        version: schema_version,
                    });
                }
                validate_token("tool name", &tool.name, 128)
                    .and_then(|_| validate_token("tool version", &tool.version, 128))
                    .map_err(|_| AdapterError::InvalidToolIdentity)?;
                input.validate()?;
                if input != expectation.input {
                    return Err(AdapterError::StaleInput);
                }
                if role != expectation.role {
                    return Err(AdapterError::UnexpectedRole {
                        expected: expectation.role.clone(),
                        actual: role,
                    });
                }
                let lineage = canonical_lineage(&lineage);
                if lineage.len() > expectation.limits.max_lineage_entries {
                    return Err(AdapterError::TooMuchLineage {
                        count: lineage.len(),
                        limit: expectation.limits.max_lineage_entries,
                    });
                }
                if lineage != expected_lineage {
                    return Err(AdapterError::StaleLineage);
                }
                header = Some(ToolRunHeader {
                    schema: TOOL_ADAPTER_SCHEMA.to_string(),
                    schema_version,
                    tool,
                    role: expectation.role.clone(),
                    input: expectation.input.clone(),
                    lineage,
                });
            }
            WireRecord::Claim {
                sequence,
                provider_claim_id,
                claim,
            } => {
                let Some(header) = &header else {
                    return Err(AdapterError::HeaderNotFirst { line: line_number });
                };
                if claims.len() >= expectation.limits.max_claim_records {
                    return Err(AdapterError::TooManyClaims {
                        count: claims.len() + 1,
                        limit: expectation.limits.max_claim_records,
                    });
                }
                validate_token("provider claim ID", &provider_claim_id, 256)
                    .map_err(|_| AdapterError::InvalidProviderClaimId { sequence })?;
                validate_claim(
                    sequence,
                    &claim,
                    header.schema_version,
                    &header.role,
                    &header.input,
                )?;
                claims.push(ToolClaimRecord {
                    sequence,
                    provider_claim_id,
                    claim,
                });
            }
            WireRecord::Summary {
                complete,
                analyzed_range,
                skipped_ranges,
                claim_records,
                claims_sha256,
                resources,
            } => {
                if header.is_none() {
                    return Err(AdapterError::HeaderNotFirst { line: line_number });
                }
                if summary.is_some() {
                    return Err(AdapterError::DuplicateSummary { line: line_number });
                }
                summary = Some(ToolRunSummary {
                    complete,
                    analyzed_range,
                    skipped_ranges,
                    claim_records,
                    claims_sha256,
                    resources,
                });
            }
        }
    }

    let header = header.ok_or(AdapterError::MissingHeader)?;
    let summary = summary.ok_or(AdapterError::MissingSummary)?;
    validate_record_identity(&mut claims)?;
    validate_function_body_associations(&claims)?;
    validate_summary(&summary, &claims, &header.input, expectation.limits)?;
    let candidates = canonical_candidates(&claims);
    let source = ToolRunSource {
        source_sha256: source_digest(&header, &candidates),
        schema_version: header.schema_version,
        tool: header.tool,
        role: header.role,
        input: header.input,
        lineage: header.lineage,
    };
    Ok(ToolAdapterOutput {
        source,
        candidates,
        summary,
    })
}

/// Serialize one complete provider run into the strict v1 JSONL schema.
///
/// Claim sequence numbers and provider IDs remain provider lineage, but rows
/// are emitted in sequence order. The result is accepted only if the generic
/// ingester can validate it against the exact identity and lineage supplied
/// by the provider-specific adapter.
pub fn export_complete_tool_jsonl(run: CompleteToolRun) -> Result<String, AdapterError> {
    export_complete_tool_jsonl_version(run, TOOL_ADAPTER_SCHEMA_VERSION)
}

/// Serialize a complete provider run into schema v2. V2 adds exact,
/// entry-associated `function_body_range` candidates for discontiguous bodies;
/// it does not change v1's existing wire encodings or canonical digest.
pub fn export_complete_tool_jsonl_v2(run: CompleteToolRun) -> Result<String, AdapterError> {
    export_complete_tool_jsonl_version(run, TOOL_ADAPTER_SCHEMA_VERSION_V2)
}

/// Serialize a complete provider run into schema v3. V3 adds bank-qualified,
/// explicitly non-exhaustive computed-control-flow candidates.
pub fn export_complete_tool_jsonl_v3(run: CompleteToolRun) -> Result<String, AdapterError> {
    export_complete_tool_jsonl_version(run, TOOL_ADAPTER_SCHEMA_VERSION_V3)
}

fn export_complete_tool_jsonl_version(
    run: CompleteToolRun,
    schema_version: u32,
) -> Result<String, AdapterError> {
    let mut claims = run.claims;
    validate_record_identity(&mut claims)?;

    let lineage = canonical_lineage(&run.lineage);
    let header = WireRecord::Header {
        schema: TOOL_ADAPTER_SCHEMA.to_string(),
        schema_version,
        tool: run.tool,
        role: run.role.clone(),
        input: run.input.clone(),
        lineage: lineage.clone(),
    };
    let summary = WireRecord::Summary {
        complete: true,
        analyzed_range: BankRange {
            bank: run.input.bank.clone(),
            va_start: run.input.va_start,
            va_end: run.input.va_end,
        },
        skipped_ranges: Vec::new(),
        claim_records: claims.len() as u64,
        claims_sha256: canonical_claim_records_sha256(&claims),
        resources: run.resources,
    };

    let mut records = Vec::with_capacity(claims.len() + 2);
    records.push(serde_json::to_string(&header).expect("wire header is serializable"));
    records.extend(claims.iter().map(|claim| {
        serde_json::to_string(&WireRecord::Claim {
            sequence: claim.sequence,
            provider_claim_id: claim.provider_claim_id.clone(),
            claim: claim.claim.clone(),
        })
        .expect("wire claim is serializable")
    }));
    records.push(serde_json::to_string(&summary).expect("wire summary is serializable"));
    let jsonl = records.join("\n");

    ingest_tool_jsonl(
        &jsonl,
        &ToolAdapterExpectation {
            input: run.input,
            role: run.role,
            lineage,
            limits: AdapterLimits::default(),
        },
    )?;
    Ok(jsonl)
}

/// Canonical digest used by the required summary record. Record order in the
/// JSONL stream does not matter; sequence and provider claim ID remain part of
/// the digest and are validated independently.
pub fn canonical_claim_records_sha256(records: &[ToolClaimRecord]) -> Sha256Digest {
    let mut sorted = records.to_vec();
    sorted.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.tool-adapter.claim-records.v1\0");
    hash_u64(&mut hasher, sorted.len() as u64);
    for record in &sorted {
        hash_u64(&mut hasher, record.sequence);
        hash_str(&mut hasher, &record.provider_claim_id);
        hash_claim(&mut hasher, &record.claim);
    }
    Sha256Digest(hasher.finalize().into())
}

fn validate_limits(limits: AdapterLimits) -> Result<(), AdapterError> {
    for (name, value) in [
        ("max_total_bytes", limits.max_total_bytes),
        ("max_line_bytes", limits.max_line_bytes),
        ("max_claim_records", limits.max_claim_records),
        ("max_lineage_entries", limits.max_lineage_entries),
        ("max_warnings", limits.max_warnings),
    ] {
        if value == 0 {
            return Err(AdapterError::InvalidLimit(name));
        }
    }
    Ok(())
}

fn validate_token(_field: &str, value: &str, max_len: usize) -> Result<(), AdapterError> {
    if value.is_empty()
        || value.len() > max_len
        || value.chars().any(|character| character.is_control())
    {
        return Err(AdapterError::InvalidToolIdentity);
    }
    Ok(())
}

fn canonical_lineage(lineage: &[ToolLineageRef]) -> Vec<ToolLineageRef> {
    let mut lineage = lineage.to_vec();
    lineage.sort();
    lineage.dedup();
    lineage
}

fn validate_claim(
    sequence: u64,
    claim: &ToolCandidateKind,
    schema_version: u32,
    role: &ToolRunRole,
    input: &BankInputIdentity,
) -> Result<(), AdapterError> {
    let role_ok = matches!(
        (role, claim),
        (
            ToolRunRole::FunctionBoundaryCandidates,
            ToolCandidateKind::FunctionEntry { .. }
                | ToolCandidateKind::FunctionExtent { .. }
                | ToolCandidateKind::FunctionBodyRange { .. }
        ) | (
            ToolRunRole::ControlFlowCandidates,
            ToolCandidateKind::ComputedControlFlow { .. }
        ) | (
            ToolRunRole::RegionCandidates,
            ToolCandidateKind::ExecutableRange { .. } | ToolCandidateKind::DataRange { .. }
        ) | (
            ToolRunRole::SymbolCandidates,
            ToolCandidateKind::SymbolAlias { .. }
        )
    );
    if !role_ok {
        return Err(AdapterError::WrongClaimRole {
            sequence,
            role: role.clone(),
        });
    }
    if matches!(claim, ToolCandidateKind::FunctionBodyRange { .. })
        && schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V2
        && schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
    {
        return Err(AdapterError::UnknownSchema {
            schema: TOOL_ADAPTER_SCHEMA.to_string(),
            version: schema_version,
        });
    }
    if matches!(claim, ToolCandidateKind::ComputedControlFlow { .. })
        && schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
    {
        return Err(AdapterError::UnknownSchema {
            schema: TOOL_ADAPTER_SCHEMA.to_string(),
            version: schema_version,
        });
    }

    match claim {
        ToolCandidateKind::FunctionEntry { address } => {
            validate_address(sequence, address, input, true)
        }
        ToolCandidateKind::FunctionExtent { range }
        | ToolCandidateKind::ExecutableRange { range } => {
            validate_range(sequence, range, input, true)
        }
        ToolCandidateKind::FunctionBodyRange { entry, range } => {
            validate_address(sequence, entry, input, true)?;
            validate_range(sequence, range, input, true)
        }
        ToolCandidateKind::DataRange { range } => validate_range(sequence, range, input, false),
        ToolCandidateKind::SymbolAlias { address, alias } => {
            // Symbol providers may name byte-granular data as well as code.
            // Function-entry alignment belongs to the function role, not to
            // bank-qualified identity in general.
            validate_address(sequence, address, input, false)?;
            if alias.is_empty()
                || alias.len() > 256
                || alias.chars().any(|character| character.is_control())
            {
                return Err(AdapterError::InvalidAlias { sequence });
            }
            Ok(())
        }
        ToolCandidateKind::ComputedControlFlow {
            site,
            targets,
            completeness: ComputedFlowCompleteness::Unknown,
            ..
        } => {
            validate_address(sequence, site, input, true)?;
            if !targets.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(AdapterError::NonCanonicalComputedTargets { sequence });
            }
            for target in targets {
                validate_address(sequence, target, input, true)?;
            }
            Ok(())
        }
    }
}

fn validate_address(
    sequence: u64,
    address: &BankAddr,
    input: &BankInputIdentity,
    aligned: bool,
) -> Result<(), AdapterError> {
    if address.bank.is_empty() || address.bank != input.bank {
        return Err(AdapterError::UnqualifiedOrWrongBank {
            sequence,
            bank: address.bank.clone(),
        });
    }
    if address.pc < input.va_start || address.pc >= input.va_end {
        return Err(AdapterError::OutOfBank {
            sequence,
            start: address.pc,
            end: address.pc.saturating_add(1),
        });
    }
    if aligned && !address.pc.is_multiple_of(4) {
        return Err(AdapterError::UnalignedCodeClaim {
            sequence,
            start: address.pc,
            end: address.pc.saturating_add(1),
        });
    }
    Ok(())
}

fn validate_range(
    sequence: u64,
    range: &BankRange,
    input: &BankInputIdentity,
    aligned: bool,
) -> Result<(), AdapterError> {
    if range.bank.is_empty() || range.bank != input.bank {
        return Err(AdapterError::UnqualifiedOrWrongBank {
            sequence,
            bank: range.bank.clone(),
        });
    }
    if range.va_start >= range.va_end
        || range.va_start < input.va_start
        || range.va_end > input.va_end
    {
        return Err(AdapterError::OutOfBank {
            sequence,
            start: range.va_start,
            end: range.va_end,
        });
    }
    if aligned && (!range.va_start.is_multiple_of(4) || !range.va_end.is_multiple_of(4)) {
        return Err(AdapterError::UnalignedCodeClaim {
            sequence,
            start: range.va_start,
            end: range.va_end,
        });
    }
    Ok(())
}

fn validate_record_identity(claims: &mut [ToolClaimRecord]) -> Result<(), AdapterError> {
    claims.sort_by_key(|claim| claim.sequence);
    let mut ids = BTreeSet::new();
    for (expected, claim) in claims.iter().enumerate() {
        let expected = expected as u64;
        if claim.sequence != expected {
            if expected > 0 && claims[expected as usize - 1].sequence == claim.sequence {
                return Err(AdapterError::DuplicateSequence(claim.sequence));
            }
            return Err(AdapterError::MissingSequence {
                expected,
                actual: claim.sequence,
            });
        }
        if !ids.insert(claim.provider_claim_id.clone()) {
            return Err(AdapterError::DuplicateProviderClaimId(
                claim.provider_claim_id.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_function_body_associations(claims: &[ToolClaimRecord]) -> Result<(), AdapterError> {
    let entries: BTreeSet<_> = claims
        .iter()
        .filter_map(|claim| match &claim.claim {
            ToolCandidateKind::FunctionEntry { address } => Some(address),
            _ => None,
        })
        .collect();
    for claim in claims {
        if let ToolCandidateKind::FunctionBodyRange { entry, .. } = &claim.claim {
            if !entries.contains(entry) {
                return Err(AdapterError::BodyRangeWithoutFunctionEntry {
                    sequence: claim.sequence,
                    bank: entry.bank.clone(),
                    entry: entry.pc,
                });
            }
        }
    }
    Ok(())
}

fn validate_summary(
    summary: &ToolRunSummary,
    claims: &[ToolClaimRecord],
    input: &BankInputIdentity,
    limits: AdapterLimits,
) -> Result<(), AdapterError> {
    if !summary.complete || !summary.skipped_ranges.is_empty() {
        return Err(AdapterError::PartialRun);
    }
    let expected_range = BankRange {
        bank: input.bank.clone(),
        va_start: input.va_start,
        va_end: input.va_end,
    };
    if summary.analyzed_range != expected_range {
        return Err(AdapterError::IncompleteAnalyzedRange);
    }
    if summary.claim_records != claims.len() as u64 {
        return Err(AdapterError::ClaimCountMismatch {
            summary: summary.claim_records,
            actual: claims.len(),
        });
    }
    if summary.claims_sha256 != canonical_claim_records_sha256(claims) {
        return Err(AdapterError::ClaimDigestMismatch);
    }
    if summary.resources.limit_hit {
        return Err(AdapterError::ResourceLimitHit);
    }
    if summary.resources.input_bytes != input.byte_len() {
        return Err(AdapterError::ResourceInputMismatch {
            summary: summary.resources.input_bytes,
            expected: input.byte_len(),
        });
    }
    if summary.resources.warnings.len() > limits.max_warnings {
        return Err(AdapterError::TooManyWarnings {
            count: summary.resources.warnings.len(),
            limit: limits.max_warnings,
        });
    }
    for warning in &summary.resources.warnings {
        if warning.len() > 1024 || warning.chars().any(|character| character.is_control()) {
            return Err(AdapterError::TooManyWarnings {
                count: summary.resources.warnings.len(),
                limit: limits.max_warnings,
            });
        }
    }
    Ok(())
}

fn canonical_candidates(records: &[ToolClaimRecord]) -> Vec<ToolCandidate> {
    let mut grouped: BTreeMap<ToolCandidateKind, (Vec<String>, Vec<u64>)> = BTreeMap::new();
    for record in records {
        let (ids, sequences) = grouped.entry(record.claim.clone()).or_default();
        ids.push(record.provider_claim_id.clone());
        sequences.push(record.sequence);
    }
    grouped
        .into_iter()
        .map(|(kind, (mut provider_claim_ids, mut source_sequences))| {
            provider_claim_ids.sort();
            provider_claim_ids.dedup();
            source_sequences.sort_unstable();
            source_sequences.dedup();
            ToolCandidate {
                kind,
                proof_ceiling: CandidateProofCeiling::Candidate,
                provider_claim_ids,
                source_sequences,
            }
        })
        .collect()
}

fn source_digest(header: &ToolRunHeader, candidates: &[ToolCandidate]) -> Sha256Digest {
    source_digest_parts(
        header.schema_version,
        &header.tool,
        &header.role,
        &header.input,
        &header.lineage,
        candidates.iter().map(|candidate| &candidate.kind),
    )
}

pub(crate) fn recompute_tool_run_source_sha256(
    source: &ToolRunSource,
    candidate_kinds: &[ToolCandidateKind],
) -> Sha256Digest {
    let mut candidate_kinds = candidate_kinds.to_vec();
    candidate_kinds.sort();
    candidate_kinds.dedup();
    source_digest_parts(
        source.schema_version,
        &source.tool,
        &source.role,
        &source.input,
        &source.lineage,
        candidate_kinds.iter(),
    )
}

fn source_digest_parts<'a>(
    schema_version: u32,
    tool: &ToolIdentity,
    role: &ToolRunRole,
    input: &BankInputIdentity,
    lineage: &[ToolLineageRef],
    candidate_kinds: impl ExactSizeIterator<Item = &'a ToolCandidateKind>,
) -> Sha256Digest {
    let mut hasher = Sha256::new();
    if schema_version == TOOL_ADAPTER_SCHEMA_VERSION {
        // Preserve every established v1 source identity byte-for-byte.
        hasher.update(b"fn64.tool-adapter.source.v1\0");
    } else if schema_version == TOOL_ADAPTER_SCHEMA_VERSION_V2 {
        hasher.update(b"fn64.tool-adapter.source.v2\0");
        hasher.update(schema_version.to_le_bytes());
    } else {
        hasher.update(b"fn64.tool-adapter.source.v3\0");
        hasher.update(schema_version.to_le_bytes());
    }
    hash_str(&mut hasher, &tool.name);
    hash_str(&mut hasher, &tool.version);
    hasher.update(tool.build_sha256.0);
    hasher.update([role_tag(role)]);
    hash_input(&mut hasher, input);
    hash_u64(&mut hasher, lineage.len() as u64);
    for lineage in lineage {
        hasher.update([lineage_tag(&lineage.role)]);
        hasher.update(lineage.source_sha256.0);
    }
    hash_u64(&mut hasher, candidate_kinds.len() as u64);
    for kind in candidate_kinds {
        hash_claim(&mut hasher, kind);
    }
    Sha256Digest(hasher.finalize().into())
}

fn hash_input(hasher: &mut Sha256, input: &BankInputIdentity) {
    hasher.update(input.normalized_rom_sha256.0);
    hash_str(hasher, &input.bank);
    hasher.update(input.bank_bytes_sha256.0);
    hasher.update(input.mapping_sha256.0);
    hasher.update(input.va_start.to_le_bytes());
    hasher.update(input.va_end.to_le_bytes());
}

fn hash_claim(hasher: &mut Sha256, claim: &ToolCandidateKind) {
    match claim {
        ToolCandidateKind::FunctionEntry { address } => {
            hasher.update([1]);
            hash_address(hasher, address);
        }
        ToolCandidateKind::FunctionExtent { range } => {
            hasher.update([2]);
            hash_range(hasher, range);
        }
        ToolCandidateKind::FunctionBodyRange { entry, range } => {
            hasher.update([6]);
            hash_address(hasher, entry);
            hash_range(hasher, range);
        }
        ToolCandidateKind::ExecutableRange { range } => {
            hasher.update([3]);
            hash_range(hasher, range);
        }
        ToolCandidateKind::DataRange { range } => {
            hasher.update([4]);
            hash_range(hasher, range);
        }
        ToolCandidateKind::SymbolAlias { address, alias } => {
            hasher.update([5]);
            hash_address(hasher, address);
            hash_str(hasher, alias);
        }
        ToolCandidateKind::ComputedControlFlow {
            site,
            via_call,
            targets,
            completeness: ComputedFlowCompleteness::Unknown,
        } => {
            hasher.update([7]);
            hash_address(hasher, site);
            hasher.update([u8::from(*via_call)]);
            hash_u64(hasher, targets.len() as u64);
            for target in targets {
                hash_address(hasher, target);
            }
            hasher.update([0]);
        }
    }
}

fn hash_address(hasher: &mut Sha256, address: &BankAddr) {
    hash_str(hasher, &address.bank);
    hasher.update(address.pc.to_le_bytes());
}

fn hash_range(hasher: &mut Sha256, range: &BankRange) {
    hash_str(hasher, &range.bank);
    hasher.update(range.va_start.to_le_bytes());
    hasher.update(range.va_end.to_le_bytes());
}

fn hash_str(hasher: &mut Sha256, value: &str) {
    hash_u64(hasher, value.len() as u64);
    hasher.update(value.as_bytes());
}

fn hash_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_le_bytes());
}

fn role_tag(role: &ToolRunRole) -> u8 {
    match role {
        ToolRunRole::FunctionBoundaryCandidates => 1,
        ToolRunRole::RegionCandidates => 2,
        ToolRunRole::SymbolCandidates => 3,
        ToolRunRole::ControlFlowCandidates => 4,
    }
}

fn lineage_tag(role: &ToolLineageRole) -> u8 {
    match role {
        ToolLineageRole::DiscoverySnapshot => 1,
        ToolLineageRole::EvidenceManifest => 2,
        ToolLineageRole::ParentToolRun => 3,
        ToolLineageRole::ToolConfiguration => 4,
        ToolLineageRole::ProviderOutput => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Sha256Digest {
        Sha256Digest([byte; 32])
    }

    fn input(bank: &str) -> BankInputIdentity {
        BankInputIdentity {
            normalized_rom_sha256: digest(1),
            bank: bank.to_string(),
            bank_bytes_sha256: digest(2),
            mapping_sha256: digest(3),
            va_start: 0x8000_0000,
            va_end: 0x8000_1000,
        }
    }

    fn tool() -> ToolIdentity {
        ToolIdentity {
            name: "fake-provider".to_string(),
            version: "1.2.3".to_string(),
            build_sha256: digest(4),
        }
    }

    fn lineage() -> Vec<ToolLineageRef> {
        vec![
            ToolLineageRef {
                role: ToolLineageRole::DiscoverySnapshot,
                source_sha256: digest(5),
            },
            ToolLineageRef {
                role: ToolLineageRole::ToolConfiguration,
                source_sha256: digest(6),
            },
        ]
    }

    fn function_claim(bank: &str, sequence: u64, pc: u32) -> ToolClaimRecord {
        ToolClaimRecord {
            sequence,
            provider_claim_id: format!("claim-{sequence}"),
            claim: ToolCandidateKind::FunctionEntry {
                address: BankAddr::new(bank, pc),
            },
        }
    }

    fn function_body_range_claim(
        bank: &str,
        sequence: u64,
        entry: u32,
        start: u32,
        end: u32,
    ) -> ToolClaimRecord {
        ToolClaimRecord {
            sequence,
            provider_claim_id: format!("body-range-{sequence}"),
            claim: ToolCandidateKind::FunctionBodyRange {
                entry: BankAddr::new(bank, entry),
                range: BankRange {
                    bank: bank.to_string(),
                    va_start: start,
                    va_end: end,
                },
            },
        }
    }

    fn schema_v2(jsonl: String) -> String {
        jsonl.replacen("\"schema_version\":1", "\"schema_version\":2", 1)
    }

    fn schema_v3(jsonl: String) -> String {
        jsonl.replacen("\"schema_version\":1", "\"schema_version\":3", 1)
    }

    fn computed_claim(
        bank: &str,
        sequence: u64,
        site: u32,
        via_call: bool,
        targets: &[u32],
    ) -> ToolClaimRecord {
        ToolClaimRecord {
            sequence,
            provider_claim_id: format!("computed-{sequence}"),
            claim: ToolCandidateKind::ComputedControlFlow {
                site: BankAddr::new(bank, site),
                via_call,
                targets: targets
                    .iter()
                    .map(|target| BankAddr::new(bank, *target))
                    .collect(),
                completeness: ComputedFlowCompleteness::Unknown,
            },
        }
    }

    fn stream(
        input: &BankInputIdentity,
        role: ToolRunRole,
        lineage: Vec<ToolLineageRef>,
        claims: &[ToolClaimRecord],
        complete: bool,
    ) -> String {
        let header = WireRecord::Header {
            schema: TOOL_ADAPTER_SCHEMA.to_string(),
            schema_version: TOOL_ADAPTER_SCHEMA_VERSION,
            tool: tool(),
            role,
            input: input.clone(),
            lineage,
        };
        let summary = WireRecord::Summary {
            complete,
            analyzed_range: BankRange {
                bank: input.bank.clone(),
                va_start: input.va_start,
                va_end: input.va_end,
            },
            skipped_ranges: Vec::new(),
            claim_records: claims.len() as u64,
            claims_sha256: canonical_claim_records_sha256(claims),
            resources: ToolResourceDiagnostics {
                input_bytes: input.byte_len(),
                elapsed_millis: 7,
                peak_memory_bytes: Some(4096),
                limit_hit: false,
                warnings: Vec::new(),
            },
        };
        let mut records = vec![serde_json::to_string(&header).unwrap()];
        records.extend(claims.iter().map(|claim| {
            serde_json::to_string(&WireRecord::Claim {
                sequence: claim.sequence,
                provider_claim_id: claim.provider_claim_id.clone(),
                claim: claim.claim.clone(),
            })
            .unwrap()
        }));
        records.push(serde_json::to_string(&summary).unwrap());
        records.join("\n")
    }

    fn expectation(bank: &str) -> ToolAdapterExpectation {
        ToolAdapterExpectation {
            input: input(bank),
            role: ToolRunRole::FunctionBoundaryCandidates,
            lineage: lineage(),
            limits: AdapterLimits::default(),
        }
    }

    #[test]
    fn fake_provider_success_is_sorted_deduplicated_and_candidate_only() {
        let expected = expectation("bank-a");
        let claims = vec![
            function_claim("bank-a", 0, 0x8000_0080),
            function_claim("bank-a", 1, 0x8000_0040),
            function_claim("bank-a", 2, 0x8000_0080),
        ];
        let output = ingest_tool_jsonl(
            &stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &claims,
                true,
            ),
            &expected,
        )
        .unwrap();
        assert_eq!(output.candidates.len(), 2);
        assert!(output
            .candidates
            .iter()
            .all(|candidate| candidate.proof_ceiling == CandidateProofCeiling::Candidate));
        assert_eq!(output.candidates[1].source_sequences, vec![0, 2]);
        assert_eq!(output.source.lineage, canonical_lineage(&lineage()));
    }

    #[test]
    fn same_va_in_distinct_banks_remains_distinct_identity() {
        let expected_a = expectation("bank-a");
        let expected_b = expectation("bank-b");
        let output_a = ingest_tool_jsonl(
            &stream(
                &expected_a.input,
                expected_a.role.clone(),
                expected_a.lineage.clone(),
                &[function_claim("bank-a", 0, 0x8000_0040)],
                true,
            ),
            &expected_a,
        )
        .unwrap();
        let output_b = ingest_tool_jsonl(
            &stream(
                &expected_b.input,
                expected_b.role.clone(),
                expected_b.lineage.clone(),
                &[function_claim("bank-b", 0, 0x8000_0040)],
                true,
            ),
            &expected_b,
        )
        .unwrap();
        assert_ne!(output_a.candidates[0].kind, output_b.candidates[0].kind);
        assert_ne!(output_a.source.source_sha256, output_b.source.source_sha256);
    }

    #[test]
    fn schema_v2_preserves_discontiguous_body_ranges_without_claiming_the_gap() {
        let expected = expectation("bank-a");
        let entry = 0x8000_0040;
        let claims = vec![
            function_claim("bank-a", 0, entry),
            function_body_range_claim("bank-a", 1, entry, entry, entry + 8),
            function_body_range_claim("bank-a", 2, entry, entry + 0x20, entry + 0x28),
        ];
        let jsonl = schema_v2(stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            &claims,
            true,
        ));
        let output = ingest_tool_jsonl(&jsonl, &expected).unwrap();
        let v1_entry_only = ingest_tool_jsonl(
            &stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &[function_claim("bank-a", 0, entry)],
                true,
            ),
            &expected,
        )
        .unwrap();
        let v2_entry_only = ingest_tool_jsonl(
            &schema_v2(stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &[function_claim("bank-a", 0, entry)],
                true,
            )),
            &expected,
        )
        .unwrap();

        assert_eq!(
            output.source().schema_version,
            TOOL_ADAPTER_SCHEMA_VERSION_V2
        );
        assert_ne!(
            v1_entry_only.source().source_sha256,
            v2_entry_only.source().source_sha256
        );
        assert_eq!(output.candidates().len(), 3);
        assert!(output.candidates().iter().any(|candidate| {
            candidate.kind
                == ToolCandidateKind::FunctionBodyRange {
                    entry: BankAddr::new("bank-a", entry),
                    range: BankRange {
                        bank: "bank-a".to_string(),
                        va_start: entry + 0x20,
                        va_end: entry + 0x28,
                    },
                }
        }));
        assert!(!output.candidates().iter().any(|candidate| matches!(
            candidate.kind,
            ToolCandidateKind::FunctionExtent { ref range }
                if range.va_start <= entry + 8 && range.va_end >= entry + 0x20
        )));
        let mut reordered = claims.clone();
        reordered.reverse();
        assert_eq!(
            canonical_claim_records_sha256(&claims),
            canonical_claim_records_sha256(&reordered)
        );
    }

    #[test]
    fn schema_v1_rejects_body_ranges_and_v2_validates_both_addresses() {
        let expected = expectation("bank-a");
        let entry = 0x8000_0040;
        let valid = [function_body_range_claim(
            "bank-a",
            0,
            entry,
            entry + 0x20,
            entry + 0x28,
        )];
        assert!(matches!(
            ingest_tool_jsonl(
                &stream(
                    &expected.input,
                    expected.role.clone(),
                    expected.lineage.clone(),
                    &valid,
                    true,
                ),
                &expected
            ),
            Err(AdapterError::UnknownSchema { version: 1, .. })
        ));

        let missing_entry = schema_v2(stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            &valid,
            true,
        ));
        assert!(matches!(
            ingest_tool_jsonl(&missing_entry, &expected),
            Err(AdapterError::BodyRangeWithoutFunctionEntry { sequence: 0, .. })
        ));

        for invalid in [
            function_body_range_claim("bank-b", 0, entry, entry + 0x20, entry + 0x28),
            function_body_range_claim("bank-a", 0, entry + 2, entry + 0x20, entry + 0x28),
            function_body_range_claim("bank-a", 0, entry, entry + 0x22, entry + 0x28),
            function_body_range_claim("bank-a", 0, entry, entry + 0x20, 0x8000_2000),
        ] {
            let jsonl = schema_v2(stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &[invalid],
                true,
            ));
            assert!(ingest_tool_jsonl(&jsonl, &expected).is_err());
        }
    }

    #[test]
    fn schema_v3_computed_flow_is_bank_local_sorted_and_never_exhaustive() {
        let mut expected = expectation("bank-a");
        expected.role = ToolRunRole::ControlFlowCandidates;
        let claim = computed_claim("bank-a", 0, 0x8000_0040, true, &[0x8000_0080, 0x8000_00c0]);
        let jsonl = schema_v3(stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            std::slice::from_ref(&claim),
            true,
        ));
        let output = ingest_tool_jsonl(&jsonl, &expected).unwrap();
        assert_eq!(
            output.source().schema_version,
            TOOL_ADAPTER_SCHEMA_VERSION_V3
        );
        assert_eq!(output.candidates().len(), 1);
        assert_eq!(
            output.candidates()[0].proof_ceiling,
            CandidateProofCeiling::Candidate
        );

        let v2 = schema_v2(stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            std::slice::from_ref(&claim),
            true,
        ));
        assert!(matches!(
            ingest_tool_jsonl(&v2, &expected),
            Err(AdapterError::UnknownSchema { version: 2, .. })
        ));

        for invalid in [
            computed_claim("bank-a", 0, 0x8000_0040, false, &[0x8000_00c0, 0x8000_0080]),
            computed_claim("bank-a", 0, 0x8000_0040, false, &[0x8000_0080, 0x8000_0080]),
        ] {
            let jsonl = schema_v3(stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &[invalid],
                true,
            ));
            assert!(matches!(
                ingest_tool_jsonl(&jsonl, &expected),
                Err(AdapterError::NonCanonicalComputedTargets { sequence: 0 })
            ));
        }

        for invalid in [
            computed_claim("bank-b", 0, 0x8000_0040, false, &[]),
            computed_claim("bank-a", 0, 0x8000_0042, false, &[]),
            computed_claim("bank-a", 0, 0x8000_0040, false, &[0x8000_2000]),
        ] {
            let jsonl = schema_v3(stream(
                &expected.input,
                expected.role.clone(),
                expected.lineage.clone(),
                &[invalid],
                true,
            ));
            assert!(ingest_tool_jsonl(&jsonl, &expected).is_err());
        }
    }

    #[test]
    fn stale_input_and_lineage_are_rejected() {
        let expected = expectation("bank-a");
        let mut stale_input = expected.input.clone();
        stale_input.bank_bytes_sha256 = digest(99);
        let jsonl = stream(
            &stale_input,
            expected.role.clone(),
            expected.lineage.clone(),
            &[],
            true,
        );
        assert_eq!(
            ingest_tool_jsonl(&jsonl, &expected).unwrap_err(),
            AdapterError::StaleInput
        );

        let jsonl = stream(
            &expected.input,
            expected.role.clone(),
            vec![ToolLineageRef {
                role: ToolLineageRole::DiscoverySnapshot,
                source_sha256: digest(77),
            }],
            &[],
            true,
        );
        assert_eq!(
            ingest_tool_jsonl(&jsonl, &expected).unwrap_err(),
            AdapterError::StaleLineage
        );
    }

    #[test]
    fn wrong_bank_out_of_bank_partial_and_unknown_fields_fail_closed() {
        let expected = expectation("bank-a");
        let wrong_bank = [function_claim("bank-b", 0, 0x8000_0040)];
        assert!(matches!(
            ingest_tool_jsonl(
                &stream(
                    &expected.input,
                    expected.role.clone(),
                    expected.lineage.clone(),
                    &wrong_bank,
                    true,
                ),
                &expected
            ),
            Err(AdapterError::UnqualifiedOrWrongBank { .. })
        ));

        let outside = [function_claim("bank-a", 0, 0x8000_2000)];
        assert!(matches!(
            ingest_tool_jsonl(
                &stream(
                    &expected.input,
                    expected.role.clone(),
                    expected.lineage.clone(),
                    &outside,
                    true,
                ),
                &expected
            ),
            Err(AdapterError::OutOfBank { .. })
        ));

        assert_eq!(
            ingest_tool_jsonl(
                &stream(
                    &expected.input,
                    expected.role.clone(),
                    expected.lineage.clone(),
                    &[],
                    false,
                ),
                &expected
            )
            .unwrap_err(),
            AdapterError::PartialRun
        );

        let unknown = format!(
            "{{\"record\":\"header\",\"schema\":\"{}\",\"schema_version\":1,\"tool\":{},\"role\":\"function_boundary_candidates\",\"input\":{},\"lineage\":[],\"proof_state\":\"proven\"}}",
            TOOL_ADAPTER_SCHEMA,
            serde_json::to_string(&tool()).unwrap(),
            serde_json::to_string(&expected.input).unwrap()
        );
        assert!(matches!(
            ingest_tool_jsonl(&unknown, &expected),
            Err(AdapterError::InvalidJson { .. })
        ));
    }

    #[test]
    fn claim_line_order_does_not_change_canonical_output() {
        let expected = expectation("bank-a");
        let claims = vec![
            function_claim("bank-a", 0, 0x8000_0040),
            function_claim("bank-a", 1, 0x8000_0080),
        ];
        let first = stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            &claims,
            true,
        );
        let mut lines: Vec<_> = first.lines().map(str::to_owned).collect();
        lines.swap(1, 2);
        let reordered = lines.join("\n");
        let first = ingest_tool_jsonl(&first, &expected).unwrap();
        let second = ingest_tool_jsonl(&reordered, &expected).unwrap();
        assert_eq!(first, second);
        for _ in 0..20 {
            assert_eq!(ingest_tool_jsonl(&reordered, &expected).unwrap(), first);
        }
    }

    #[test]
    fn incomplete_sequence_and_digest_mismatch_are_rejected() {
        let expected = expectation("bank-a");
        let gap = [function_claim("bank-a", 1, 0x8000_0040)];
        assert!(matches!(
            ingest_tool_jsonl(
                &stream(
                    &expected.input,
                    expected.role.clone(),
                    expected.lineage.clone(),
                    &gap,
                    true,
                ),
                &expected
            ),
            Err(AdapterError::MissingSequence {
                expected: 0,
                actual: 1
            })
        ));

        let valid = stream(
            &expected.input,
            expected.role.clone(),
            expected.lineage.clone(),
            &[function_claim("bank-a", 0, 0x8000_0040)],
            true,
        );
        let mut lines: Vec<String> = valid.lines().map(str::to_owned).collect();
        let last = lines.len() - 1;
        let mut summary: serde_json::Value = serde_json::from_str(&lines[last]).unwrap();
        summary["claims_sha256"] = serde_json::Value::String(digest(9).to_hex());
        lines[last] = serde_json::to_string(&summary).unwrap();
        assert_eq!(
            ingest_tool_jsonl(&lines.join("\n"), &expected).unwrap_err(),
            AdapterError::ClaimDigestMismatch
        );
    }
}
