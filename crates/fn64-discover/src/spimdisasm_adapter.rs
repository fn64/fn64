//! Candidate-only adapter for spimdisasm's `--function-info` CSV output.
//!
//! The external process boundary is deliberately absent: a runner may invoke
//! a pinned spimdisasm installation in an out-of-tree workspace, then pass
//! the resulting CSV and exact bank identity here. This module validates the
//! version-specific CSV contract, checks its ROM-offset/VA geometry, and
//! exports only function-entry and function-extent candidates through
//! [`crate::tool_adapter`]'s strict JSONL schema.

use crate::facts::BankAddr;
use crate::tool_adapter::{
    export_complete_tool_jsonl, AdapterError, BankInputIdentity, BankRange, CompleteToolRun,
    Sha256Digest, ToolAdapterExpectation, ToolCandidateKind, ToolClaimRecord, ToolIdentity,
    ToolLineageRef, ToolLineageRole, ToolResourceDiagnostics, ToolRunRole,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const FUNCTION_INFO_HEADER: [&str; 9] = [
    "vrom",
    "address",
    "name",
    "file",
    "length",
    "hash of top bits of words",
    "functions called by this function",
    "non-jal function calls",
    "referenced functions",
];

/// Hard limit for one provider-native CSV artifact. The generic JSONL parser
/// has separate limits after normalization.
pub const MAX_FUNCTION_INFO_CSV_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpimdisasmRunDiagnostics {
    pub elapsed_millis: u64,
    pub peak_memory_bytes: Option<u64>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpimdisasmExportRequest {
    /// Must name `spimdisasm`; version and build digest identify the pinned
    /// package/executable used by the out-of-process runner.
    pub tool: ToolIdentity,
    pub input: BankInputIdentity,
    pub parent_lineage: Vec<ToolLineageRef>,
    /// Value passed as spimdisasm's input-file start offset. A materialized
    /// bank normally uses zero; analyzing a bank slice in a larger normalized
    /// ROM uses that slice's physical start.
    pub vrom_start: u32,
    pub diagnostics: SpimdisasmRunDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpimdisasmExport {
    pub jsonl: String,
    pub expectation: ToolAdapterExpectation,
    /// Order-independent digest of all provider CSV fields after strict CSV
    /// decoding. It is attached as provider-output lineage.
    pub provider_output_sha256: Sha256Digest,
    pub function_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpimdisasmAdapterError {
    CsvTooLarge {
        bytes: usize,
        limit: usize,
    },
    InvalidCsv(String),
    UnexpectedHeader,
    WrongToolName(String),
    InvalidHex {
        row: usize,
        field: &'static str,
    },
    InvalidFunctionName {
        row: usize,
    },
    NonPortableFileIdentity {
        row: usize,
    },
    EmptyFunction {
        row: usize,
    },
    UnalignedFunction {
        row: usize,
        address: u32,
        length: u32,
    },
    FunctionOutsideBank {
        row: usize,
        address: u32,
        length: u32,
    },
    VromGeometryMismatch {
        row: usize,
        expected: u32,
        actual: u32,
    },
    DuplicateFunctionEntry(u32),
    OverlappingFunctions {
        previous_end: u32,
        next_start: u32,
    },
    ToolAdapter(AdapterError),
}

impl std::fmt::Display for SpimdisasmAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CsvTooLarge { bytes, limit } => {
                write!(f, "spimdisasm CSV is {bytes} bytes, exceeding limit {limit}")
            }
            Self::InvalidCsv(detail) => write!(f, "invalid spimdisasm function-info CSV: {detail}"),
            Self::UnexpectedHeader => write!(f, "unexpected spimdisasm function-info CSV header"),
            Self::WrongToolName(name) => write!(f, "spimdisasm adapter received tool {name:?}"),
            Self::InvalidHex { row, field } => {
                write!(f, "spimdisasm CSV row {row} has invalid hexadecimal {field}")
            }
            Self::InvalidFunctionName { row } => {
                write!(f, "spimdisasm CSV row {row} has an invalid function name")
            }
            Self::NonPortableFileIdentity { row } => write!(
                f,
                "spimdisasm CSV row {row} has a non-portable file identity"
            ),
            Self::EmptyFunction { row } => write!(f, "spimdisasm CSV row {row} has zero length"),
            Self::UnalignedFunction { row, address, length } => write!(
                f,
                "spimdisasm CSV row {row} function 0x{address:08x}+0x{length:x} is not word-aligned"
            ),
            Self::FunctionOutsideBank { row, address, length } => write!(
                f,
                "spimdisasm CSV row {row} function 0x{address:08x}+0x{length:x} is outside the bank"
            ),
            Self::VromGeometryMismatch { row, expected, actual } => write!(
                f,
                "spimdisasm CSV row {row} VROM 0x{actual:08x} does not match mapped 0x{expected:08x}"
            ),
            Self::DuplicateFunctionEntry(address) => {
                write!(f, "spimdisasm CSV repeats function entry 0x{address:08x}")
            }
            Self::OverlappingFunctions {
                previous_end,
                next_start,
            } => write!(
                f,
                "spimdisasm functions overlap at 0x{next_start:08x} before prior end 0x{previous_end:08x}"
            ),
            Self::ToolAdapter(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SpimdisasmAdapterError {}

impl From<AdapterError> for SpimdisasmAdapterError {
    fn from(value: AdapterError) -> Self {
        Self::ToolAdapter(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FunctionRow {
    vrom: u32,
    address: u32,
    length: u32,
    fields: Vec<String>,
}

/// Convert one complete `--function-info` CSV into fn64's candidate-only
/// tool-adapter JSONL. This function never invokes spimdisasm or reads a ROM.
pub fn export_function_info_csv(
    csv_bytes: &[u8],
    request: SpimdisasmExportRequest,
) -> Result<SpimdisasmExport, SpimdisasmAdapterError> {
    if request.tool.name != "spimdisasm" {
        return Err(SpimdisasmAdapterError::WrongToolName(request.tool.name));
    }
    if csv_bytes.len() > MAX_FUNCTION_INFO_CSV_BYTES {
        return Err(SpimdisasmAdapterError::CsvTooLarge {
            bytes: csv_bytes.len(),
            limit: MAX_FUNCTION_INFO_CSV_BYTES,
        });
    }

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(csv_bytes);
    let headers = reader
        .headers()
        .map_err(|error| SpimdisasmAdapterError::InvalidCsv(error.to_string()))?;
    if headers.len() != FUNCTION_INFO_HEADER.len()
        || headers
            .iter()
            .zip(FUNCTION_INFO_HEADER)
            .any(|(actual, expected)| actual != expected)
    {
        return Err(SpimdisasmAdapterError::UnexpectedHeader);
    }

    let mut rows = Vec::new();
    for (zero_index, record) in reader.records().enumerate() {
        let row_number = zero_index + 2;
        let record =
            record.map_err(|error| SpimdisasmAdapterError::InvalidCsv(error.to_string()))?;
        let fields: Vec<String> = record.iter().map(str::to_owned).collect();
        let vrom = parse_hex(&fields[0]).ok_or(SpimdisasmAdapterError::InvalidHex {
            row: row_number,
            field: "vrom",
        })?;
        let address = parse_hex(&fields[1]).ok_or(SpimdisasmAdapterError::InvalidHex {
            row: row_number,
            field: "address",
        })?;
        if fields[2].is_empty() || fields[2].chars().any(char::is_control) {
            return Err(SpimdisasmAdapterError::InvalidFunctionName { row: row_number });
        }
        if !is_portable_file_identity(&fields[3]) {
            return Err(SpimdisasmAdapterError::NonPortableFileIdentity { row: row_number });
        }
        let length = parse_hex(&fields[4]).ok_or(SpimdisasmAdapterError::InvalidHex {
            row: row_number,
            field: "length",
        })?;
        validate_geometry(row_number, vrom, address, length, &request)?;
        rows.push(FunctionRow {
            vrom,
            address,
            length,
            fields,
        });
    }
    rows.sort_by_key(|row| (row.address, row.length, row.vrom));
    validate_partition(&rows)?;

    let provider_output_sha256 = canonical_provider_output_digest(&rows);
    let config_sha256 = config_digest(request.vrom_start);
    let mut lineage = request.parent_lineage;
    lineage.push(ToolLineageRef {
        role: ToolLineageRole::ToolConfiguration,
        source_sha256: config_sha256,
    });
    lineage.push(ToolLineageRef {
        role: ToolLineageRole::ProviderOutput,
        source_sha256: provider_output_sha256,
    });
    lineage.sort();
    lineage.dedup();

    let mut claims = Vec::with_capacity(rows.len() * 2);
    for row in &rows {
        let entry_sequence = claims.len() as u64;
        claims.push(ToolClaimRecord {
            sequence: entry_sequence,
            provider_claim_id: format!("spimdisasm-entry-{:08x}", row.address),
            claim: ToolCandidateKind::FunctionEntry {
                address: BankAddr::new(&request.input.bank, row.address),
            },
        });
        let extent_sequence = claims.len() as u64;
        claims.push(ToolClaimRecord {
            sequence: extent_sequence,
            provider_claim_id: format!(
                "spimdisasm-extent-{:08x}-{:08x}",
                row.address,
                row.address + row.length
            ),
            claim: ToolCandidateKind::FunctionExtent {
                range: BankRange {
                    bank: request.input.bank.clone(),
                    va_start: row.address,
                    va_end: row.address + row.length,
                },
            },
        });
    }

    let expectation = ToolAdapterExpectation {
        input: request.input.clone(),
        role: ToolRunRole::FunctionBoundaryCandidates,
        lineage: lineage.clone(),
        limits: Default::default(),
    };
    let jsonl = export_complete_tool_jsonl(CompleteToolRun {
        tool: request.tool,
        role: ToolRunRole::FunctionBoundaryCandidates,
        input: request.input,
        lineage,
        claims,
        resources: ToolResourceDiagnostics {
            input_bytes: u64::from(expectation.input.va_end - expectation.input.va_start),
            elapsed_millis: request.diagnostics.elapsed_millis,
            peak_memory_bytes: request.diagnostics.peak_memory_bytes,
            limit_hit: false,
            warnings: request.diagnostics.warnings,
        },
    })?;
    Ok(SpimdisasmExport {
        jsonl,
        expectation,
        provider_output_sha256,
        function_count: rows.len(),
    })
}

fn parse_hex(value: &str) -> Option<u32> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

fn is_portable_file_identity(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with('/')
        || value.starts_with('~')
        || value
            .chars()
            .any(|character| matches!(character, '\\' | ':'))
        || value.chars().any(char::is_control)
    {
        return false;
    }
    value
        .split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn validate_geometry(
    row: usize,
    vrom: u32,
    address: u32,
    length: u32,
    request: &SpimdisasmExportRequest,
) -> Result<(), SpimdisasmAdapterError> {
    if length == 0 {
        return Err(SpimdisasmAdapterError::EmptyFunction { row });
    }
    if !address.is_multiple_of(4) || !length.is_multiple_of(4) {
        return Err(SpimdisasmAdapterError::UnalignedFunction {
            row,
            address,
            length,
        });
    }
    let Some(end) = address.checked_add(length) else {
        return Err(SpimdisasmAdapterError::FunctionOutsideBank {
            row,
            address,
            length,
        });
    };
    if address < request.input.va_start || end > request.input.va_end {
        return Err(SpimdisasmAdapterError::FunctionOutsideBank {
            row,
            address,
            length,
        });
    }
    let expected = request
        .vrom_start
        .checked_add(address - request.input.va_start)
        .ok_or(SpimdisasmAdapterError::FunctionOutsideBank {
            row,
            address,
            length,
        })?;
    if vrom != expected {
        return Err(SpimdisasmAdapterError::VromGeometryMismatch {
            row,
            expected,
            actual: vrom,
        });
    }
    Ok(())
}

fn validate_partition(rows: &[FunctionRow]) -> Result<(), SpimdisasmAdapterError> {
    let mut entries = BTreeSet::new();
    let mut previous_end = None;
    for row in rows {
        if !entries.insert(row.address) {
            return Err(SpimdisasmAdapterError::DuplicateFunctionEntry(row.address));
        }
        if let Some(end) = previous_end {
            if row.address < end {
                return Err(SpimdisasmAdapterError::OverlappingFunctions {
                    previous_end: end,
                    next_start: row.address,
                });
            }
        }
        previous_end = Some(row.address + row.length);
    }
    Ok(())
}

fn config_digest(vrom_start: u32) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.spimdisasm-function-info.config.v1\0");
    hasher.update(vrom_start.to_le_bytes());
    Sha256Digest(hasher.finalize().into())
}

fn canonical_provider_output_digest(rows: &[FunctionRow]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.spimdisasm-function-info.output.v1\0");
    hasher.update((rows.len() as u64).to_le_bytes());
    for row in rows {
        hasher.update((row.fields.len() as u64).to_le_bytes());
        for field in &row.fields {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
    }
    Sha256Digest(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_adapter::{ingest_tool_jsonl, CandidateProofCeiling};

    fn digest(byte: u8) -> Sha256Digest {
        Sha256Digest([byte; 32])
    }

    fn request(bank: &str) -> SpimdisasmExportRequest {
        SpimdisasmExportRequest {
            tool: ToolIdentity {
                name: "spimdisasm".to_string(),
                version: "1.42.2".to_string(),
                build_sha256: digest(1),
            },
            input: BankInputIdentity {
                normalized_rom_sha256: digest(2),
                bank: bank.to_string(),
                bank_bytes_sha256: digest(3),
                mapping_sha256: digest(4),
                va_start: 0x8000_0400,
                va_end: 0x8000_0800,
            },
            parent_lineage: vec![ToolLineageRef {
                role: ToolLineageRole::DiscoverySnapshot,
                source_sha256: digest(5),
            }],
            vrom_start: 0x1000,
            diagnostics: SpimdisasmRunDiagnostics {
                elapsed_millis: 12,
                peak_memory_bytes: Some(4096),
                warnings: Vec::new(),
            },
        }
    }

    fn csv(rows: &str) -> String {
        format!("{}\n{rows}", FUNCTION_INFO_HEADER.join(","))
    }

    #[test]
    fn exports_bank_qualified_candidate_only_jsonl() {
        let csv = csv(concat!(
            "0x001040,0x80000440,func_80000440,\"unit, one\",0x20,abc,[],[],[]\n",
            "0x001000,0x80000400,func_80000400,,0x40,def,[],[],[]"
        ));
        let export = export_function_info_csv(csv.as_bytes(), request("resident")).unwrap();
        assert_eq!(export.function_count, 2);
        let output = ingest_tool_jsonl(&export.jsonl, &export.expectation).unwrap();
        assert_eq!(output.candidates().len(), 4);
        assert!(output.candidates().iter().all(|candidate| {
            candidate.proof_ceiling == CandidateProofCeiling::Candidate
                && match &candidate.kind {
                    ToolCandidateKind::FunctionEntry { address } => address.bank == "resident",
                    ToolCandidateKind::FunctionExtent { range } => range.bank == "resident",
                    _ => false,
                }
        }));
        assert!(output.source().lineage.iter().any(|lineage| {
            lineage.role == ToolLineageRole::ProviderOutput
                && lineage.source_sha256 == export.provider_output_sha256
        }));
    }

    #[test]
    fn row_order_and_csv_quoting_do_not_change_canonical_export() {
        let first = csv(concat!(
            "0x001000,0x80000400,func_80000400,,0x20,abc,[],[],[]\n",
            "0x001020,0x80000420,func_80000420,,0x20,def,[],[],[]"
        ));
        let second = csv(concat!(
            "0x001020,0x80000420,func_80000420,\"\",0x20,def,[],[],[]\r\n",
            "0x001000,0x80000400,func_80000400,\"\",0x20,abc,[],[],[]"
        ));
        let first = export_function_info_csv(first.as_bytes(), request("resident")).unwrap();
        let second = export_function_info_csv(second.as_bytes(), request("resident")).unwrap();
        assert_eq!(first.provider_output_sha256, second.provider_output_sha256);
        assert_eq!(first.jsonl, second.jsonl);
        for _ in 0..10 {
            assert_eq!(
                export_function_info_csv(first_csv().as_bytes(), request("resident"))
                    .unwrap()
                    .jsonl,
                first.jsonl
            );
        }
    }

    fn first_csv() -> String {
        csv(concat!(
            "0x001000,0x80000400,func_80000400,,0x20,abc,[],[],[]\n",
            "0x001020,0x80000420,func_80000420,,0x20,def,[],[],[]"
        ))
    }

    #[test]
    fn geometry_duplicate_overlap_and_schema_drift_fail_closed() {
        let bad_vrom = csv("0x001004,0x80000400,func_80000400,,0x20,abc,[],[],[]");
        assert!(matches!(
            export_function_info_csv(bad_vrom.as_bytes(), request("resident")),
            Err(SpimdisasmAdapterError::VromGeometryMismatch { .. })
        ));

        let overlap = csv(concat!(
            "0x001000,0x80000400,func_a,,0x40,abc,[],[],[]\n",
            "0x001020,0x80000420,func_b,,0x20,def,[],[],[]"
        ));
        assert!(matches!(
            export_function_info_csv(overlap.as_bytes(), request("resident")),
            Err(SpimdisasmAdapterError::OverlappingFunctions { .. })
        ));

        let duplicate = csv(concat!(
            "0x001000,0x80000400,func_a,,0x20,abc,[],[],[]\n",
            "0x001000,0x80000400,func_b,,0x20,def,[],[],[]"
        ));
        assert!(matches!(
            export_function_info_csv(duplicate.as_bytes(), request("resident")),
            Err(SpimdisasmAdapterError::DuplicateFunctionEntry(0x8000_0400))
        ));

        let drifted = "vrom,address,name,file,length\n0x1000,0x80000400,func,,,";
        assert_eq!(
            export_function_info_csv(drifted.as_bytes(), request("resident")).unwrap_err(),
            SpimdisasmAdapterError::UnexpectedHeader
        );
    }

    #[test]
    fn materialization_paths_cannot_enter_provider_identity() {
        for file in [
            "/absolute/workspace/bank/text",
            "../bank/text",
            "bank/../text",
            r"C:\\temp\\bank",
            "file:bank",
        ] {
            let csv = csv(&format!(
                "0x001000,0x80000400,func_80000400,{file},0x20,abc,[],[],[]"
            ));
            assert!(matches!(
                export_function_info_csv(csv.as_bytes(), request("resident")),
                Err(SpimdisasmAdapterError::NonPortableFileIdentity { .. })
            ));
        }

        let portable = csv("0x001000,0x80000400,func_80000400,overlay/text,0x20,abc,[],[],[]");
        export_function_info_csv(portable.as_bytes(), request("resident")).unwrap();
    }

    #[test]
    fn same_va_in_different_banks_has_distinct_jsonl_identity() {
        let csv = csv("0x001000,0x80000400,func_80000400,,0x20,abc,[],[],[]");
        let first = export_function_info_csv(csv.as_bytes(), request("overlay-a")).unwrap();
        let second = export_function_info_csv(csv.as_bytes(), request("overlay-b")).unwrap();
        assert_ne!(first.jsonl, second.jsonl);
        let first = ingest_tool_jsonl(&first.jsonl, &first.expectation).unwrap();
        let second = ingest_tool_jsonl(&second.jsonl, &second.expectation).unwrap();
        assert_ne!(first.source().source_sha256, second.source().source_sha256);
    }
}
