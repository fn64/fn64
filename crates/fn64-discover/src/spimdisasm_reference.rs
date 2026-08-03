//! Strict per-bank normalization for adapter-owned spimdisasm reference output.
//!
//! This module does not invoke spimdisasm, inspect its implementation, read a
//! ROM, or promote a tool observation into a native fact. An out-of-process
//! adapter supplies one small JSON metadata document plus JSONL candidate
//! records. The normalizer binds that output to pinned tool/config/source
//! identities and exact bank VA/VROM geometry, then returns canonical
//! candidate records and a content-free cache receipt.

use crate::tool_adapter::Sha256Digest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SPIMDISASM_REFERENCE_SCHEMA: &str = "fn64.spimdisasm-reference";
pub const SPIMDISASM_REFERENCE_SCHEMA_VERSION: u32 = 1;
pub const SPIMDISASM_REFERENCE_ALGORITHM: &str = "fn64.spimdisasm-reference.normalize.v1";
pub const MAX_SPIMDISASM_REFERENCE_METADATA_BYTES: usize = 16 * 1024;
pub const MAX_SPIMDISASM_REFERENCE_JSONL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SPIMDISASM_REFERENCE_LINE_BYTES: usize = 4 * 1024;
pub const MAX_SPIMDISASM_REFERENCE_RECORDS: usize = 100_000;

/// Exact identity and linear VA/VROM mapping of one immutable analysis bank.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpimdisasmReferenceBankV1 {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
    pub vrom_start: u32,
    pub vrom_end: u32,
}

/// Caller-owned pins. Provider metadata must match every field exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpimdisasmReferenceExpectationV1 {
    pub tool_version: String,
    pub tool_build_sha256: Sha256Digest,
    pub tool_source_sha256: Sha256Digest,
    pub config_sha256: Sha256Digest,
    pub bank_input_sha256: Sha256Digest,
    pub bank: SpimdisasmReferenceBankV1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpimdisasmReferencePointV1 {
    pub bank: String,
    pub va: u32,
    pub vrom: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpimdisasmDataCandidateKindV1 {
    Pointer,
    String,
    Float,
    Object,
}

/// Canonical candidate-only observations. No variant can express proof.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpimdisasmReferenceCandidateV1 {
    BlockStart {
        at: SpimdisasmReferencePointV1,
    },
    DirectReference {
        source: SpimdisasmReferencePointV1,
        target: SpimdisasmReferencePointV1,
    },
    HiLoPair {
        hi: SpimdisasmReferencePointV1,
        lo: SpimdisasmReferencePointV1,
        target: SpimdisasmReferencePointV1,
    },
    DataCandidate {
        kind: SpimdisasmDataCandidateKindV1,
        bank: String,
        va_start: u32,
        va_end: u32,
        vrom_start: u32,
        vrom_end: u32,
    },
}

/// Cache identity only. It intentionally contains no raw records, names,
/// paths, diagnostics, or provider text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpimdisasmReferenceReceiptV1 {
    pub schema_version: u32,
    pub algorithm: &'static str,
    pub tool_version: String,
    pub tool_build_sha256: Sha256Digest,
    pub tool_source_sha256: Sha256Digest,
    pub config_sha256: Sha256Digest,
    pub bank_input_sha256: Sha256Digest,
    pub provider_output_sha256: Sha256Digest,
    pub canonical_candidates_sha256: Sha256Digest,
    pub cache_key_sha256: Sha256Digest,
    pub bank: SpimdisasmReferenceBankV1,
    pub candidate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedSpimdisasmReferencesV1 {
    pub candidates: Vec<SpimdisasmReferenceCandidateV1>,
    pub receipt: SpimdisasmReferenceReceiptV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpimdisasmReferenceError {
    MetadataTooLarge {
        bytes: usize,
        limit: usize,
    },
    JsonlTooLarge {
        bytes: usize,
        limit: usize,
    },
    LineTooLarge {
        line: usize,
        bytes: usize,
        limit: usize,
    },
    TooManyRecords {
        records: usize,
        limit: usize,
    },
    InvalidMetadata(String),
    InvalidJsonl {
        line: usize,
        detail: String,
    },
    IdentityMismatch(&'static str),
    InvalidBankGeometry,
    InvalidRecord {
        line: usize,
        detail: &'static str,
    },
    DuplicateRecord {
        line: usize,
    },
    InconsistentRecord {
        line: usize,
        detail: &'static str,
    },
}

impl std::fmt::Display for SpimdisasmReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MetadataTooLarge { bytes, limit } => {
                write!(f, "metadata is {bytes} bytes, exceeding limit {limit}")
            }
            Self::JsonlTooLarge { bytes, limit } => {
                write!(f, "JSONL is {bytes} bytes, exceeding limit {limit}")
            }
            Self::LineTooLarge { line, bytes, limit } => {
                write!(
                    f,
                    "JSONL line {line} is {bytes} bytes, exceeding limit {limit}"
                )
            }
            Self::TooManyRecords { records, limit } => {
                write!(f, "JSONL has {records} records, exceeding limit {limit}")
            }
            Self::InvalidMetadata(detail) => write!(f, "invalid metadata: {detail}"),
            Self::InvalidJsonl { line, detail } => {
                write!(f, "invalid JSONL line {line}: {detail}")
            }
            Self::IdentityMismatch(field) => write!(f, "metadata {field} does not match its pin"),
            Self::InvalidBankGeometry => write!(f, "invalid bank VA/VROM geometry"),
            Self::InvalidRecord { line, detail } => write!(f, "invalid record {line}: {detail}"),
            Self::DuplicateRecord { line } => write!(f, "record {line} is repeated"),
            Self::InconsistentRecord { line, detail } => {
                write!(f, "record {line} is inconsistent: {detail}")
            }
        }
    }
}

impl std::error::Error for SpimdisasmReferenceError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMetadataV1 {
    schema: String,
    schema_version: u32,
    algorithm: String,
    tool: String,
    tool_version: String,
    tool_build_sha256: Sha256Digest,
    tool_source_sha256: Sha256Digest,
    config_sha256: Sha256Digest,
    bank_input_sha256: Sha256Digest,
    provider_output_sha256: Sha256Digest,
    bank: SpimdisasmReferenceBankV1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireReferenceV1 {
    BlockStart {
        bank: String,
        va: u32,
        vrom: u32,
    },
    DirectReference {
        bank: String,
        source_va: u32,
        source_vrom: u32,
        target_va: u32,
        target_vrom: u32,
    },
    HiLoPair {
        bank: String,
        hi_va: u32,
        hi_vrom: u32,
        lo_va: u32,
        lo_vrom: u32,
        target_va: u32,
        target_vrom: u32,
    },
    DataCandidate {
        bank: String,
        data_kind: SpimdisasmDataCandidateKindV1,
        va_start: u32,
        va_end: u32,
        vrom_start: u32,
        vrom_end: u32,
    },
}

/// Normalize adapter-owned JSON metadata and JSONL records without invoking a
/// tool or reading bank bytes. Output records are sorted and unique.
pub fn normalize_spimdisasm_references_v1(
    metadata_json: &[u8],
    records_jsonl: &[u8],
    expected: &SpimdisasmReferenceExpectationV1,
) -> Result<NormalizedSpimdisasmReferencesV1, SpimdisasmReferenceError> {
    if metadata_json.len() > MAX_SPIMDISASM_REFERENCE_METADATA_BYTES {
        return Err(SpimdisasmReferenceError::MetadataTooLarge {
            bytes: metadata_json.len(),
            limit: MAX_SPIMDISASM_REFERENCE_METADATA_BYTES,
        });
    }
    if records_jsonl.len() > MAX_SPIMDISASM_REFERENCE_JSONL_BYTES {
        return Err(SpimdisasmReferenceError::JsonlTooLarge {
            bytes: records_jsonl.len(),
            limit: MAX_SPIMDISASM_REFERENCE_JSONL_BYTES,
        });
    }
    validate_bank(&expected.bank)?;
    validate_token(&expected.tool_version).map_err(SpimdisasmReferenceError::InvalidMetadata)?;

    let metadata: WireMetadataV1 = serde_json::from_slice(metadata_json)
        .map_err(|error| SpimdisasmReferenceError::InvalidMetadata(error.to_string()))?;
    validate_metadata(&metadata, expected)?;
    if Sha256Digest::of(records_jsonl) != metadata.provider_output_sha256 {
        return Err(SpimdisasmReferenceError::IdentityMismatch(
            "provider_output_sha256",
        ));
    }
    if !records_jsonl.is_empty() && !records_jsonl.ends_with(b"\n") {
        return Err(SpimdisasmReferenceError::InvalidJsonl {
            line: records_jsonl.split(|byte| *byte == b'\n').count(),
            detail: "final record is not LF-terminated".into(),
        });
    }

    let mut unique_wire = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut direct_sources = BTreeMap::new();
    let mut pair_instructions = BTreeSet::new();
    let mut data_ranges = BTreeMap::new();

    let record_bytes = &records_jsonl[..records_jsonl.len().saturating_sub(1)];
    let record_limit = if records_jsonl.is_empty() {
        0
    } else {
        usize::MAX
    };
    for (zero_index, line) in record_bytes
        .split(|byte| *byte == b'\n')
        .enumerate()
        .take(record_limit)
    {
        let line_number = zero_index + 1;
        if line.is_empty() || line.contains(&b'\r') {
            return Err(SpimdisasmReferenceError::InvalidJsonl {
                line: line_number,
                detail: "blank lines and CR bytes are forbidden".into(),
            });
        }
        if line.len() > MAX_SPIMDISASM_REFERENCE_LINE_BYTES {
            return Err(SpimdisasmReferenceError::LineTooLarge {
                line: line_number,
                bytes: line.len(),
                limit: MAX_SPIMDISASM_REFERENCE_LINE_BYTES,
            });
        }
        if candidates.len() == MAX_SPIMDISASM_REFERENCE_RECORDS {
            return Err(SpimdisasmReferenceError::TooManyRecords {
                records: candidates.len() + 1,
                limit: MAX_SPIMDISASM_REFERENCE_RECORDS,
            });
        }
        let wire: WireReferenceV1 = serde_json::from_slice(line).map_err(|error| {
            SpimdisasmReferenceError::InvalidJsonl {
                line: line_number,
                detail: error.to_string(),
            }
        })?;
        if !unique_wire.insert(wire.clone()) {
            return Err(SpimdisasmReferenceError::DuplicateRecord { line: line_number });
        }
        let candidate = normalize_record(
            wire,
            line_number,
            &expected.bank,
            &mut direct_sources,
            &mut pair_instructions,
            &mut data_ranges,
        )?;
        candidates.push(candidate);
    }
    candidates.sort();
    candidates.dedup();

    let canonical_candidates_sha256 = canonical_candidates_sha256(&candidates);
    let cache_key_sha256 = spimdisasm_reference_cache_key_v1(expected)?;
    Ok(NormalizedSpimdisasmReferencesV1 {
        receipt: SpimdisasmReferenceReceiptV1 {
            schema_version: SPIMDISASM_REFERENCE_SCHEMA_VERSION,
            algorithm: SPIMDISASM_REFERENCE_ALGORITHM,
            tool_version: expected.tool_version.clone(),
            tool_build_sha256: expected.tool_build_sha256,
            tool_source_sha256: expected.tool_source_sha256,
            config_sha256: expected.config_sha256,
            bank_input_sha256: expected.bank_input_sha256,
            provider_output_sha256: metadata.provider_output_sha256,
            canonical_candidates_sha256,
            cache_key_sha256,
            bank: expected.bank.clone(),
            candidate_count: candidates.len(),
        },
        candidates,
    })
}

fn validate_metadata(
    metadata: &WireMetadataV1,
    expected: &SpimdisasmReferenceExpectationV1,
) -> Result<(), SpimdisasmReferenceError> {
    if metadata.schema != SPIMDISASM_REFERENCE_SCHEMA {
        return Err(SpimdisasmReferenceError::IdentityMismatch("schema"));
    }
    if metadata.schema_version != SPIMDISASM_REFERENCE_SCHEMA_VERSION {
        return Err(SpimdisasmReferenceError::IdentityMismatch("schema_version"));
    }
    if metadata.algorithm != SPIMDISASM_REFERENCE_ALGORITHM {
        return Err(SpimdisasmReferenceError::IdentityMismatch("algorithm"));
    }
    if metadata.tool != "spimdisasm" {
        return Err(SpimdisasmReferenceError::IdentityMismatch("tool"));
    }
    validate_token(&metadata.tool_version).map_err(SpimdisasmReferenceError::InvalidMetadata)?;
    let checks = [
        (
            metadata.tool_version == expected.tool_version,
            "tool_version",
        ),
        (
            metadata.tool_build_sha256 == expected.tool_build_sha256,
            "tool_build_sha256",
        ),
        (
            metadata.tool_source_sha256 == expected.tool_source_sha256,
            "tool_source_sha256",
        ),
        (
            metadata.config_sha256 == expected.config_sha256,
            "config_sha256",
        ),
        (
            metadata.bank_input_sha256 == expected.bank_input_sha256,
            "bank_input_sha256",
        ),
        (metadata.bank == expected.bank, "bank"),
    ];
    for (matches, field) in checks {
        if !matches {
            return Err(SpimdisasmReferenceError::IdentityMismatch(field));
        }
    }
    Ok(())
}

fn validate_bank(bank: &SpimdisasmReferenceBankV1) -> Result<(), SpimdisasmReferenceError> {
    validate_token(&bank.bank).map_err(SpimdisasmReferenceError::InvalidMetadata)?;
    let va_len = bank.va_end.checked_sub(bank.va_start);
    let vrom_len = bank.vrom_end.checked_sub(bank.vrom_start);
    if va_len.is_none()
        || va_len == Some(0)
        || va_len != vrom_len
        || !bank.va_start.is_multiple_of(4)
        || !bank.va_end.is_multiple_of(4)
    {
        return Err(SpimdisasmReferenceError::InvalidBankGeometry);
    }
    Ok(())
}

fn validate_token(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    {
        return Err("identity tokens must be 1..=128 portable ASCII characters".into());
    }
    Ok(())
}

fn normalize_record(
    wire: WireReferenceV1,
    line: usize,
    bank: &SpimdisasmReferenceBankV1,
    direct_sources: &mut BTreeMap<u32, u32>,
    pair_instructions: &mut BTreeSet<u32>,
    data_ranges: &mut BTreeMap<u32, u32>,
) -> Result<SpimdisasmReferenceCandidateV1, SpimdisasmReferenceError> {
    match wire {
        WireReferenceV1::BlockStart { bank: id, va, vrom } => {
            let at = point(id, va, vrom, line, bank, true)?;
            Ok(SpimdisasmReferenceCandidateV1::BlockStart { at })
        }
        WireReferenceV1::DirectReference {
            bank: id,
            source_va,
            source_vrom,
            target_va,
            target_vrom,
        } => {
            let source = point(id.clone(), source_va, source_vrom, line, bank, true)?;
            let target = point(id, target_va, target_vrom, line, bank, false)?;
            if let Some(prior) = direct_sources.insert(source_va, target_va) {
                if prior != target_va {
                    return Err(SpimdisasmReferenceError::InconsistentRecord {
                        line,
                        detail: "one source instruction has multiple direct targets",
                    });
                }
            }
            Ok(SpimdisasmReferenceCandidateV1::DirectReference { source, target })
        }
        WireReferenceV1::HiLoPair {
            bank: id,
            hi_va,
            hi_vrom,
            lo_va,
            lo_vrom,
            target_va,
            target_vrom,
        } => {
            let hi = point(id.clone(), hi_va, hi_vrom, line, bank, true)?;
            let lo = point(id.clone(), lo_va, lo_vrom, line, bank, true)?;
            let target = point(id, target_va, target_vrom, line, bank, false)?;
            if hi_va >= lo_va {
                return Err(SpimdisasmReferenceError::InvalidRecord {
                    line,
                    detail: "HI instruction must precede LO instruction",
                });
            }
            if !pair_instructions.insert(hi_va) || !pair_instructions.insert(lo_va) {
                return Err(SpimdisasmReferenceError::InconsistentRecord {
                    line,
                    detail: "an instruction belongs to more than one HI/LO pair",
                });
            }
            Ok(SpimdisasmReferenceCandidateV1::HiLoPair { hi, lo, target })
        }
        WireReferenceV1::DataCandidate {
            bank: id,
            data_kind,
            va_start,
            va_end,
            vrom_start,
            vrom_end,
        } => {
            validate_bank_id(&id, line, bank)?;
            if va_start >= va_end
                || va_start < bank.va_start
                || va_end > bank.va_end
                || vrom_start < bank.vrom_start
                || vrom_end > bank.vrom_end
                || mapped_vrom(va_start, bank) != Some(vrom_start)
                || mapped_vrom(va_end, bank) != Some(vrom_end)
            {
                return Err(SpimdisasmReferenceError::InvalidRecord {
                    line,
                    detail: "data range does not match bank VA/VROM geometry",
                });
            }
            let overlaps_predecessor = data_ranges
                .range(..=va_start)
                .next_back()
                .is_some_and(|(_, end)| *end > va_start);
            let overlaps_successor = data_ranges
                .range(va_start..)
                .next()
                .is_some_and(|(start, _)| *start < va_end);
            if overlaps_predecessor || overlaps_successor {
                return Err(SpimdisasmReferenceError::InconsistentRecord {
                    line,
                    detail: "data candidates overlap",
                });
            }
            data_ranges.insert(va_start, va_end);
            Ok(SpimdisasmReferenceCandidateV1::DataCandidate {
                kind: data_kind,
                bank: id,
                va_start,
                va_end,
                vrom_start,
                vrom_end,
            })
        }
    }
}

fn point(
    id: String,
    va: u32,
    vrom: u32,
    line: usize,
    bank: &SpimdisasmReferenceBankV1,
    instruction: bool,
) -> Result<SpimdisasmReferencePointV1, SpimdisasmReferenceError> {
    validate_bank_id(&id, line, bank)?;
    if va < bank.va_start
        || va >= bank.va_end
        || mapped_vrom(va, bank) != Some(vrom)
        || (instruction && !va.is_multiple_of(4))
    {
        return Err(SpimdisasmReferenceError::InvalidRecord {
            line,
            detail: "point does not match bank VA/VROM geometry",
        });
    }
    Ok(SpimdisasmReferencePointV1 { bank: id, va, vrom })
}

fn validate_bank_id(
    id: &str,
    line: usize,
    bank: &SpimdisasmReferenceBankV1,
) -> Result<(), SpimdisasmReferenceError> {
    if id != bank.bank {
        return Err(SpimdisasmReferenceError::InvalidRecord {
            line,
            detail: "record BankId does not match the selected bank",
        });
    }
    Ok(())
}

fn mapped_vrom(va: u32, bank: &SpimdisasmReferenceBankV1) -> Option<u32> {
    bank.vrom_start.checked_add(va.checked_sub(bank.va_start)?)
}

/// Compute the lookup key before running the provider. The key binds every
/// input that can change normalized semantics, but not the not-yet-produced
/// provider output.
pub fn spimdisasm_reference_cache_key_v1(
    expected: &SpimdisasmReferenceExpectationV1,
) -> Result<Sha256Digest, SpimdisasmReferenceError> {
    validate_bank(&expected.bank)?;
    validate_token(&expected.tool_version).map_err(SpimdisasmReferenceError::InvalidMetadata)?;
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, SPIMDISASM_REFERENCE_ALGORITHM.as_bytes());
    hash_field(&mut hasher, expected.tool_version.as_bytes());
    hash_field(&mut hasher, &expected.tool_build_sha256.0);
    hash_field(&mut hasher, &expected.tool_source_sha256.0);
    hash_field(&mut hasher, &expected.config_sha256.0);
    hash_field(&mut hasher, &expected.bank_input_sha256.0);
    hash_field(&mut hasher, expected.bank.bank.as_bytes());
    for value in [
        expected.bank.va_start,
        expected.bank.va_end,
        expected.bank.vrom_start,
        expected.bank.vrom_end,
    ] {
        hasher.update(value.to_be_bytes());
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

fn canonical_candidates_sha256(candidates: &[SpimdisasmReferenceCandidateV1]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"fn64.spimdisasm-reference.candidates.v1");
    for candidate in candidates {
        match candidate {
            SpimdisasmReferenceCandidateV1::BlockStart { at } => {
                hasher.update([0]);
                hash_point(&mut hasher, at);
            }
            SpimdisasmReferenceCandidateV1::DirectReference { source, target } => {
                hasher.update([1]);
                hash_point(&mut hasher, source);
                hash_point(&mut hasher, target);
            }
            SpimdisasmReferenceCandidateV1::HiLoPair { hi, lo, target } => {
                hasher.update([2]);
                hash_point(&mut hasher, hi);
                hash_point(&mut hasher, lo);
                hash_point(&mut hasher, target);
            }
            SpimdisasmReferenceCandidateV1::DataCandidate {
                kind,
                bank,
                va_start,
                va_end,
                vrom_start,
                vrom_end,
            } => {
                hasher.update([3]);
                hasher.update([match kind {
                    SpimdisasmDataCandidateKindV1::Pointer => 0,
                    SpimdisasmDataCandidateKindV1::String => 1,
                    SpimdisasmDataCandidateKindV1::Float => 2,
                    SpimdisasmDataCandidateKindV1::Object => 3,
                }]);
                hash_field(&mut hasher, bank.as_bytes());
                for value in [va_start, va_end, vrom_start, vrom_end] {
                    hasher.update(value.to_be_bytes());
                }
            }
        }
    }
    Sha256Digest(hasher.finalize().into())
}

fn hash_point(hasher: &mut Sha256, point: &SpimdisasmReferencePointV1) {
    hash_field(hasher, point.bank.as_bytes());
    hasher.update(point.va.to_be_bytes());
    hasher.update(point.vrom.to_be_bytes());
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Sha256Digest {
        Sha256Digest([byte; 32])
    }

    fn expectation() -> SpimdisasmReferenceExpectationV1 {
        SpimdisasmReferenceExpectationV1 {
            tool_version: "1.42.2".into(),
            tool_build_sha256: digest(1),
            tool_source_sha256: digest(2),
            config_sha256: digest(3),
            bank_input_sha256: digest(4),
            bank: SpimdisasmReferenceBankV1 {
                bank: "resident".into(),
                va_start: 0x8000_0400,
                va_end: 0x8000_0500,
                vrom_start: 0x1000,
                vrom_end: 0x1100,
            },
        }
    }

    fn metadata(records: &[u8], extra: &str) -> Vec<u8> {
        let expected = expectation();
        format!(
            "{{\"schema\":\"{}\",\"schema_version\":1,\"algorithm\":\"{}\",\"tool\":\"spimdisasm\",\"tool_version\":\"1.42.2\",\"tool_build_sha256\":\"{}\",\"tool_source_sha256\":\"{}\",\"config_sha256\":\"{}\",\"bank_input_sha256\":\"{}\",\"provider_output_sha256\":\"{}\",\"bank\":{{\"bank\":\"resident\",\"va_start\":2147484672,\"va_end\":2147484928,\"vrom_start\":4096,\"vrom_end\":4352}}{extra}}}",
            SPIMDISASM_REFERENCE_SCHEMA,
            SPIMDISASM_REFERENCE_ALGORITHM,
            expected.tool_build_sha256.to_hex(),
            expected.tool_source_sha256.to_hex(),
            expected.config_sha256.to_hex(),
            expected.bank_input_sha256.to_hex(),
            Sha256Digest::of(records).to_hex(),
        )
        .into_bytes()
    }

    #[test]
    fn normalizes_all_views_in_canonical_order_and_separates_receipt() {
        let records = concat!(
            "{\"kind\":\"data_candidate\",\"bank\":\"resident\",\"data_kind\":\"string\",\"va_start\":2147484800,\"va_end\":2147484804,\"vrom_start\":4224,\"vrom_end\":4228}\n",
            "{\"kind\":\"hi_lo_pair\",\"bank\":\"resident\",\"hi_va\":2147484688,\"hi_vrom\":4112,\"lo_va\":2147484696,\"lo_vrom\":4120,\"target_va\":2147484800,\"target_vrom\":4224}\n",
            "{\"kind\":\"direct_reference\",\"bank\":\"resident\",\"source_va\":2147484684,\"source_vrom\":4108,\"target_va\":2147484800,\"target_vrom\":4224}\n",
            "{\"kind\":\"block_start\",\"bank\":\"resident\",\"va\":2147484672,\"vrom\":4096}\n",
        )
        .as_bytes();
        let normalized =
            normalize_spimdisasm_references_v1(&metadata(records, ""), records, &expectation())
                .unwrap();
        assert_eq!(normalized.candidates.len(), 4);
        assert!(normalized
            .candidates
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert_eq!(normalized.receipt.candidate_count, 4);
        assert_eq!(normalized.receipt.algorithm, SPIMDISASM_REFERENCE_ALGORITHM);
        assert_eq!(normalized.receipt.bank.bank, "resident");
    }

    #[test]
    fn rejects_unknown_metadata_and_record_fields() {
        let record = b"{\"kind\":\"block_start\",\"bank\":\"resident\",\"va\":2147484672,\"vrom\":4096,\"path\":\"/tmp/leak\"}\n";
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(record, ""), record, &expectation()),
            Err(SpimdisasmReferenceError::InvalidJsonl { .. })
        ));
        let empty = b"";
        assert!(matches!(
            normalize_spimdisasm_references_v1(
                &metadata(empty, ",\"content\":\"leak\""),
                empty,
                &expectation()
            ),
            Err(SpimdisasmReferenceError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn rejects_stale_output_and_bank_geometry() {
        let record =
            b"{\"kind\":\"block_start\",\"bank\":\"other\",\"va\":2147484672,\"vrom\":4096}\n";
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(record, ""), record, &expectation()),
            Err(SpimdisasmReferenceError::InvalidRecord { .. })
        ));
        let changed = b"{\"kind\":\"block_start\"}\n";
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(record, ""), changed, &expectation()),
            Err(SpimdisasmReferenceError::IdentityMismatch(
                "provider_output_sha256"
            ))
        ));
    }

    #[test]
    fn rejects_duplicates_conflicts_and_overlaps() {
        let duplicate = concat!(
            "{\"kind\":\"block_start\",\"bank\":\"resident\",\"va\":2147484672,\"vrom\":4096}\n",
            "{\"kind\":\"block_start\",\"bank\":\"resident\",\"va\":2147484672,\"vrom\":4096}\n",
        )
        .as_bytes();
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(duplicate, ""), duplicate, &expectation()),
            Err(SpimdisasmReferenceError::DuplicateRecord { .. })
        ));

        let overlap = concat!(
            "{\"kind\":\"data_candidate\",\"bank\":\"resident\",\"data_kind\":\"object\",\"va_start\":2147484800,\"va_end\":2147484816,\"vrom_start\":4224,\"vrom_end\":4240}\n",
            "{\"kind\":\"data_candidate\",\"bank\":\"resident\",\"data_kind\":\"string\",\"va_start\":2147484808,\"va_end\":2147484820,\"vrom_start\":4232,\"vrom_end\":4244}\n",
        )
        .as_bytes();
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(overlap, ""), overlap, &expectation()),
            Err(SpimdisasmReferenceError::InconsistentRecord { .. })
        ));

        let conflicting_reference = concat!(
            "{\"kind\":\"direct_reference\",\"bank\":\"resident\",\"source_va\":2147484684,\"source_vrom\":4108,\"target_va\":2147484800,\"target_vrom\":4224}\n",
            "{\"kind\":\"direct_reference\",\"bank\":\"resident\",\"source_va\":2147484684,\"source_vrom\":4108,\"target_va\":2147484804,\"target_vrom\":4228}\n",
        )
        .as_bytes();
        assert!(matches!(
            normalize_spimdisasm_references_v1(
                &metadata(conflicting_reference, ""),
                conflicting_reference,
                &expectation()
            ),
            Err(SpimdisasmReferenceError::InconsistentRecord { .. })
        ));

        let reused_hi = concat!(
            "{\"kind\":\"hi_lo_pair\",\"bank\":\"resident\",\"hi_va\":2147484688,\"hi_vrom\":4112,\"lo_va\":2147484696,\"lo_vrom\":4120,\"target_va\":2147484800,\"target_vrom\":4224}\n",
            "{\"kind\":\"hi_lo_pair\",\"bank\":\"resident\",\"hi_va\":2147484688,\"hi_vrom\":4112,\"lo_va\":2147484700,\"lo_vrom\":4124,\"target_va\":2147484804,\"target_vrom\":4228}\n",
        )
        .as_bytes();
        assert!(matches!(
            normalize_spimdisasm_references_v1(&metadata(reused_hi, ""), reused_hi, &expectation()),
            Err(SpimdisasmReferenceError::InconsistentRecord { .. })
        ));
    }

    #[test]
    fn enforces_byte_and_line_limits_and_exact_vrom_mapping() {
        let oversized_metadata = vec![b' '; MAX_SPIMDISASM_REFERENCE_METADATA_BYTES + 1];
        assert!(matches!(
            normalize_spimdisasm_references_v1(&oversized_metadata, b"", &expectation()),
            Err(SpimdisasmReferenceError::MetadataTooLarge { .. })
        ));

        let mut long_line = vec![b' '; MAX_SPIMDISASM_REFERENCE_LINE_BYTES + 1];
        long_line.push(b'\n');
        assert!(matches!(
            normalize_spimdisasm_references_v1(
                &metadata(&long_line, ""),
                &long_line,
                &expectation()
            ),
            Err(SpimdisasmReferenceError::LineTooLarge { .. })
        ));

        let wrong_vrom =
            b"{\"kind\":\"block_start\",\"bank\":\"resident\",\"va\":2147484676,\"vrom\":4096}\n";
        assert!(matches!(
            normalize_spimdisasm_references_v1(
                &metadata(wrong_vrom, ""),
                wrong_vrom,
                &expectation()
            ),
            Err(SpimdisasmReferenceError::InvalidRecord { .. })
        ));
    }

    #[test]
    fn cache_key_changes_with_bank_input_and_geometry() {
        let records = b"";
        let first =
            normalize_spimdisasm_references_v1(&metadata(records, ""), records, &expectation())
                .unwrap();
        let mut changed = expectation();
        changed.bank_input_sha256 = digest(9);
        let mut changed_metadata = metadata(records, "");
        let old = digest(4).to_hex();
        let new = digest(9).to_hex();
        let text = String::from_utf8(changed_metadata)
            .unwrap()
            .replace(&old, &new);
        changed_metadata = text.into_bytes();
        let second =
            normalize_spimdisasm_references_v1(&changed_metadata, records, &changed).unwrap();
        assert_ne!(
            first.receipt.cache_key_sha256,
            second.receipt.cache_key_sha256
        );

        let mut changed_geometry = expectation();
        changed_geometry.bank.va_start += 4;
        changed_geometry.bank.va_end += 4;
        changed_geometry.bank.vrom_start += 4;
        changed_geometry.bank.vrom_end += 4;
        assert_ne!(
            spimdisasm_reference_cache_key_v1(&expectation()).unwrap(),
            spimdisasm_reference_cache_key_v1(&changed_geometry).unwrap()
        );
    }
}
