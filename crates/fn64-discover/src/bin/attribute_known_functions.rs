//! Grade a sealed cold-training workspace against a known-function dump.
//!
//! The answer key is not opened until the entire ROM-only workspace has
//! validated and its compact attribution index has been finalized.

use fn64_discover::grade_candidates::ScopedCandidateIdentitiesV3;
use fn64_discover::missed_function_attribution::{
    attribute_known_functions, validate_attribution_envelope_json_v2,
    validate_attribution_report_against_cold_v1, AnswerAttributionStatusV1, AnswerFunctionV1,
    AnswerRowKind, AnswerSectionV1, AttributionEnvelopeBindingsV2, AttributionEnvelopeV2,
    ColdAttributionIndexBuilder, ExecutionDomain, MissReasonV1,
    KNOWN_FUNCTION_ATTRIBUTION_ALGORITHM_V2, KNOWN_FUNCTION_ATTRIBUTION_ENVELOPE_SCHEMA_V2,
};
use fn64_discover::snapshot_workspace::{
    validate_snapshot_workspace_streaming, SnapshotWorkspaceError,
};
use fn64_discover::tool_adapter::Sha256Digest;
use fn64_discover::workspace_artifacts::{publish_new, validate_output_path, validate_workspace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MIB: u64 = 1024 * 1024;
const MAX_DUMP_BYTES: u64 = 32 * MIB;
const MAX_OUTPUT_BYTES: usize = 128 * MIB as usize;
const MAX_ANSWER_SECTIONS: usize = 8_192;
const MAX_ANSWER_ROWS: usize = 250_000;
const MAX_NAME_BYTES: usize = 4_096;
const OUTPUT_NAME: &str = "known-function-attribution.json";
const VALIDATION_RECEIPT_SCHEMA_V1: &str = "fn64.known-function-attribution-validation.v1";

#[derive(Serialize)]
struct ValidationReceiptV1<'a> {
    schema: &'static str,
    schema_version: u32,
    report_sha256: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dump {
    #[serde(rename = "section", default)]
    sections: Vec<DumpSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DumpSection {
    name: String,
    rom: u32,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<DumpFunction>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DumpFunction {
    name: String,
    vram: u32,
    size: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("attribute-known-functions: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let first = required_arg(&mut args, "COLD_WORKSPACE or --validate-report")?;
    if first == Path::new("--validate-report") {
        return validate_report_command(&mut args);
    }
    let workspace = first;
    let dump_path = required_arg(&mut args, "DUMP_TOML")?;
    let expected_rom = parse_digest_arg(required_arg(&mut args, "EXPECTED_ROM_SHA256")?)?;
    let expected_dump = parse_digest_arg(required_arg(&mut args, "EXPECTED_DUMP_SHA256")?)?;
    let output_workspace = required_arg(&mut args, "OUTPUT_WORKSPACE")?;
    if args.next().is_some() {
        return Err(usage());
    }

    let output_workspace = validate_workspace(&output_workspace)?;
    if workspace == output_workspace {
        return Err("output workspace must be separate from the sealed cold workspace".into());
    }
    let output_path = output_workspace.join(OUTPUT_NAME);
    validate_output_path(&output_workspace, &output_path)?;

    let started = Instant::now();
    let mut identities: Option<ScopedCandidateIdentitiesV3> = None;
    let mut index_builder = Some(ColdAttributionIndexBuilder::new());
    let identity = validate_snapshot_workspace_streaming(
        &workspace,
        |value| {
            identities = Some(value.clone());
            Ok(())
        },
        |bank| {
            index_builder
                .as_mut()
                .ok_or_else(|| SnapshotWorkspaceError::visitor("cold index already finalized"))?
                .ingest_snapshot(bank.snapshot)
                .map_err(|error| SnapshotWorkspaceError::visitor(error.to_string()))
        },
    )
    .map_err(|error| format!("cold workspace rejected before answer key admission: {error}"))?;
    if identity.normalized_rom_sha256 != expected_rom {
        return Err(format!(
            "cold workspace ROM digest {}, expected {}",
            identity.normalized_rom_sha256.to_hex(),
            expected_rom.to_hex()
        ));
    }
    let index = index_builder
        .take()
        .ok_or_else(|| "cold attribution index missing".to_string())?
        .finalize()
        .map_err(|error| format!("finalizing cold attribution index: {error}"))?;
    let identities = identities.ok_or_else(|| "cold candidate receipt missing".to_string())?;

    // This is the first operation that opens the grading key.
    let dump_bytes = read_bounded_stable_regular(&dump_path, "answer-key dump", MAX_DUMP_BYTES)?;
    let actual_dump = Sha256Digest::of(&dump_bytes);
    if actual_dump != expected_dump {
        return Err(format!(
            "answer-key digest {}, expected {}",
            actual_dump.to_hex(),
            expected_dump.to_hex()
        ));
    }
    let dump: Dump = toml::from_str(
        std::str::from_utf8(&dump_bytes)
            .map_err(|error| format!("answer-key dump is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("parsing answer-key dump: {error}"))?;
    let (sections, functions) = normalize_dump(dump)?;
    let report = attribute_known_functions(&index, &identities, &sections, &functions)
        .map_err(|error| format!("attributing known functions: {error}"))?;
    let output = AttributionEnvelopeV2 {
        schema_version: KNOWN_FUNCTION_ATTRIBUTION_ENVELOPE_SCHEMA_V2,
        algorithm: KNOWN_FUNCTION_ATTRIBUTION_ALGORITHM_V2.into(),
        normalized_rom_sha256: identity.normalized_rom_sha256.to_hex(),
        cold_workspace_manifest_sha256: identity.manifest_sha256.to_hex(),
        cold_candidate_identities_v3_sha256: identity
            .scoped_candidate_identities_v3_sha256
            .to_hex(),
        answer_key_sha256: actual_dump.to_hex(),
        answer_key_execution_domain: ExecutionDomain::Unknown,
        report,
    };
    let mut encoded = BoundedVecWriter::new(MAX_OUTPUT_BYTES);
    serde_json::to_writer_pretty(&mut encoded, &output)
        .map_err(|error| format!("serializing bounded attribution report: {error}"))?;
    encoded
        .write_all(b"\n")
        .map_err(|error| format!("finishing bounded attribution report: {error}"))?;
    let encoded = encoded.into_inner();
    publish_new(&output_path, &encoded)?;
    println!(
        "attribute-known-functions: rows={} bodies={} candidate_matched_rows={} missed_rows={} candidate_denominator={} candidate_matches={} candidate_ungradable={} combined_candidates={} per_detector_only={} elapsed_ms={} report_sha256={} output={}",
        output.report.totals.raw_rows,
        output.report.totals.distinct_bodies,
        output.report.totals.candidate_matched_rows,
        output.report.totals.missed_rows,
        output.report.candidate_totals.denominator,
        output.report.candidate_totals.candidate_matched,
        output.report.candidate_totals.ungradable,
        output.report.candidate_totals.combined,
        output.report.candidate_totals.per_detector_only,
        started.elapsed().as_millis(),
        Sha256Digest::of(&encoded).to_hex(),
        output_path.display(),
    );
    let mut miss_reasons = std::collections::BTreeMap::<MissReasonV1, u64>::new();
    for row in &output.report.rows {
        if let AnswerAttributionStatusV1::Missed { primary_reason } = &row.status {
            let count = miss_reasons.entry(primary_reason.to_owned()).or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| "miss-reason count overflow".to_string())?;
        }
    }
    println!(
        "attribute-known-functions: miss_reasons={}",
        miss_reasons
            .into_iter()
            .map(|(reason, count)| format!("{reason:?}:{count}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}

fn validate_report_command(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), String> {
    let report_path = required_arg(args, "REPORT")?;
    let cold_workspace = required_arg(args, "COLD_WORKSPACE")?;
    let dump_path = required_arg(args, "DUMP_TOML")?;
    let normalized_rom_sha256 =
        parse_digest_arg(required_arg(args, "EXPECTED_ROM_SHA256")?)?.to_hex();
    let cold_workspace_manifest_sha256 =
        parse_digest_arg(required_arg(args, "EXPECTED_COLD_MANIFEST_SHA256")?)?.to_hex();
    let cold_candidate_identities_v3_sha256 =
        parse_digest_arg(required_arg(args, "EXPECTED_CANDIDATE_V3_SHA256")?)?.to_hex();
    let answer_key_sha256 =
        parse_digest_arg(required_arg(args, "EXPECTED_ANSWER_SHA256")?)?.to_hex();
    if args.next().is_some() {
        return Err(usage());
    }
    let mut identities: Option<ScopedCandidateIdentitiesV3> = None;
    let mut index_builder = Some(ColdAttributionIndexBuilder::new());
    let identity = validate_snapshot_workspace_streaming(
        &cold_workspace,
        |value| {
            identities = Some(value.clone());
            Ok(())
        },
        |bank| {
            index_builder
                .as_mut()
                .ok_or_else(|| SnapshotWorkspaceError::visitor("cold index already finalized"))?
                .ingest_snapshot(bank.snapshot)
                .map_err(|error| SnapshotWorkspaceError::visitor(error.to_string()))
        },
    )
    .map_err(|error| format!("cold workspace rejected during report validation: {error}"))?;
    if identity.normalized_rom_sha256.to_hex() != normalized_rom_sha256
        || identity.manifest_sha256.to_hex() != cold_workspace_manifest_sha256
        || identity.scoped_candidate_identities_v3_sha256.to_hex()
            != cold_candidate_identities_v3_sha256
    {
        return Err("cold workspace identity differs from report bindings".into());
    }
    let index = index_builder
        .take()
        .ok_or_else(|| "cold attribution index missing".to_string())?
        .finalize()
        .map_err(|error| format!("finalizing cold attribution index: {error}"))?;

    // Admit answer-derived artifacts only after the cold workspace has
    // independently validated, exactly as the producer does.
    let bytes =
        read_bounded_stable_regular(&report_path, "attribution report", MAX_OUTPUT_BYTES as u64)?;
    let envelope = validate_attribution_envelope_json_v2(
        &bytes,
        AttributionEnvelopeBindingsV2 {
            normalized_rom_sha256: &normalized_rom_sha256,
            cold_workspace_manifest_sha256: &cold_workspace_manifest_sha256,
            cold_candidate_identities_v3_sha256: &cold_candidate_identities_v3_sha256,
            answer_key_sha256: &answer_key_sha256,
        },
    )
    .map_err(|error| error.to_string())?;
    let dump_bytes = read_bounded_stable_regular(&dump_path, "answer-key dump", MAX_DUMP_BYTES)?;
    let actual_dump = Sha256Digest::of(&dump_bytes);
    if actual_dump.to_hex() != answer_key_sha256 {
        return Err(format!(
            "answer-key digest {}, expected {}",
            actual_dump.to_hex(),
            answer_key_sha256
        ));
    }
    let dump: Dump = toml::from_str(
        std::str::from_utf8(&dump_bytes)
            .map_err(|error| format!("answer-key dump is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("parsing answer-key dump: {error}"))?;
    let (sections, functions) = normalize_dump(dump)?;
    validate_attribution_report_against_cold_v1(
        &envelope.report,
        &index,
        &identities.ok_or_else(|| "cold candidate receipt missing".to_string())?,
        &sections,
        &functions,
    )
    .map_err(|error| error.to_string())?;
    let report_sha256 = Sha256Digest::of(&bytes).to_hex();
    let stdout = std::io::stdout();
    write_validation_receipt(stdout.lock(), &report_sha256)
}

fn write_validation_receipt(mut output: impl Write, report_sha256: &str) -> Result<(), String> {
    serde_json::to_writer(
        &mut output,
        &ValidationReceiptV1 {
            schema: VALIDATION_RECEIPT_SCHEMA_V1,
            schema_version: 1,
            report_sha256,
        },
    )
    .map_err(|error| format!("serializing validation receipt: {error}"))?;
    output
        .write_all(b"\n")
        .map_err(|error| format!("finishing validation receipt: {error}"))
}

fn required_arg(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    _label: &str,
) -> Result<PathBuf, String> {
    args.next().map(PathBuf::from).ok_or_else(usage)
}

fn parse_digest_arg(path: PathBuf) -> Result<Sha256Digest, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "digest argument must be UTF-8".to_string())?;
    Sha256Digest::from_hex(value).map_err(str::to_string)
}

fn usage() -> String {
    "usage: attribute_known_functions COLD_WORKSPACE DUMP_TOML EXPECTED_ROM_SHA256 EXPECTED_DUMP_SHA256 OUTPUT_WORKSPACE\n       attribute_known_functions --validate-report REPORT COLD_WORKSPACE DUMP_TOML EXPECTED_ROM_SHA256 EXPECTED_COLD_MANIFEST_SHA256 EXPECTED_CANDIDATE_V3_SHA256 EXPECTED_ANSWER_SHA256".into()
}

fn normalize_dump(dump: Dump) -> Result<(Vec<AnswerSectionV1>, Vec<AnswerFunctionV1>), String> {
    if dump.sections.is_empty() {
        return Err("answer-key dump contains no sections".into());
    }
    if dump.sections.len() > MAX_ANSWER_SECTIONS {
        return Err(format!(
            "answer-key dump contains {} sections, limit is {MAX_ANSWER_SECTIONS}",
            dump.sections.len()
        ));
    }
    let mut sections = Vec::with_capacity(dump.sections.len());
    let mut functions = Vec::new();
    let mut seen_bodies = BTreeSet::new();
    for (section_index, section) in dump.sections.into_iter().enumerate() {
        if section.name.len() > MAX_NAME_BYTES {
            return Err(format!(
                "answer-key section {section_index} name exceeds {MAX_NAME_BYTES} bytes"
            ));
        }
        let section_ordinal = u64::try_from(section_index)
            .map_err(|_| "answer-key section count exceeds u64".to_string())?;
        sections.push(AnswerSectionV1 {
            raw_ordinal: section_ordinal,
            name: section.name,
            execution_domain: ExecutionDomain::Unknown,
            rom_start: section.rom,
            vram_start: section.vram,
            size: section.size,
        });
        for function in section.functions {
            if functions.len() == MAX_ANSWER_ROWS {
                return Err(format!(
                    "answer-key dump exceeds {MAX_ANSWER_ROWS} function rows"
                ));
            }
            if function.name.len() > MAX_NAME_BYTES {
                return Err(format!(
                    "answer-key function {} name exceeds {MAX_NAME_BYTES} bytes",
                    functions.len()
                ));
            }
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} starts before section {section_ordinal}",
                    function.name
                )
            })?;
            let raw_rom = section
                .rom
                .checked_add(offset)
                .ok_or_else(|| format!("function {:?} ROM coordinate overflows", function.name))?;
            let coordinate = (raw_rom, function.vram);
            let kind = if function.size == 0 {
                AnswerRowKind::ZeroSizeMarker
            } else if !seen_bodies.insert(coordinate) {
                AnswerRowKind::Alias
            } else {
                AnswerRowKind::Function
            };
            let raw_ordinal = u64::try_from(functions.len())
                .map_err(|_| "answer-key function count exceeds u64".to_string())?;
            functions.push(AnswerFunctionV1 {
                raw_ordinal,
                section_raw_ordinal: section_ordinal,
                name: function.name,
                vram: function.vram,
                size: function.size,
                kind,
            });
        }
    }
    if functions.is_empty() {
        return Err("answer-key dump contains no function rows".into());
    }
    Ok((sections, functions))
}

struct BoundedVecWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedVecWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedVecWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let next_len = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| std::io::Error::other("attribution output length overflow"))?;
        if next_len > self.limit {
            return Err(std::io::Error::other(format!(
                "attribution output exceeds {} bytes",
                self.limit
            )));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_bounded_stable_regular(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    let initial = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting {label} {}: {error}", path.display()))?;
    if !initial.file_type().is_file() {
        return Err(format!("{label} is not a regular file"));
    }
    if initial.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| format!("opening {label} {}: {error}", path.display()))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("inspecting opened {label}: {error}"))?;
    if !opened.is_file() {
        return Err(format!("opened {label} is not a regular file"));
    }
    ensure_same_metadata(&initial, &opened, label)?;
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading {label}: {error}"))?;
    if bytes.len() as u64 > limit || bytes.len() as u64 != opened.len() {
        return Err(format!(
            "{label} changed length or exceeded its bound while reading"
        ));
    }
    let after_open = file
        .metadata()
        .map_err(|error| format!("reinspecting opened {label}: {error}"))?;
    let after_path = fs::symlink_metadata(path)
        .map_err(|error| format!("reinspecting {label} path: {error}"))?;
    ensure_same_metadata(&opened, &after_open, label)?;
    ensure_same_metadata(&opened, &after_path, label)?;
    Ok(bytes)
}

fn ensure_same_metadata(
    expected: &fs::Metadata,
    actual: &fs::Metadata,
    label: &str,
) -> Result<(), String> {
    let changed = expected.len() != actual.len();
    #[cfg(unix)]
    let changed = changed
        || expected.dev() != actual.dev()
        || expected.ino() != actual.ino()
        || expected.mtime() != actual.mtime()
        || expected.mtime_nsec() != actual.mtime_nsec();
    if changed {
        return Err(format!("{label} changed while being admitted"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_markers_and_labels_alias_coordinates() {
        let dump = Dump {
            sections: vec![DumpSection {
                name: "text".into(),
                rom: 0x1000,
                vram: 0x8000_0000,
                size: 0x20,
                functions: vec![
                    DumpFunction {
                        name: "a".into(),
                        vram: 0x8000_0000,
                        size: 8,
                    },
                    DumpFunction {
                        name: "a_alias".into(),
                        vram: 0x8000_0000,
                        size: 8,
                    },
                    DumpFunction {
                        name: "marker".into(),
                        vram: 0x8000_0008,
                        size: 0,
                    },
                ],
            }],
        };
        let (_, functions) = normalize_dump(dump).unwrap();
        assert_eq!(functions[0].kind, AnswerRowKind::Function);
        assert_eq!(functions[1].kind, AnswerRowKind::Alias);
        assert_eq!(functions[2].kind, AnswerRowKind::ZeroSizeMarker);
    }

    #[test]
    fn normalization_rejects_unbounded_names_before_report_construction() {
        let dump = Dump {
            sections: vec![DumpSection {
                name: "x".repeat(MAX_NAME_BYTES + 1),
                rom: 0,
                vram: 0x8000_0000,
                size: 4,
                functions: Vec::new(),
            }],
        };
        assert!(normalize_dump(dump)
            .unwrap_err()
            .contains("section 0 name exceeds"));
    }

    #[test]
    fn bounded_writer_rejects_before_growing_past_limit() {
        let mut writer = BoundedVecWriter::new(4);
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        assert_eq!(writer.into_inner(), b"1234");
    }

    #[test]
    fn validation_receipt_is_one_exact_compact_json_line() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let mut output = Vec::new();
        write_validation_receipt(&mut output, digest).unwrap();
        assert_eq!(
            output,
            format!(
                "{{\"schema\":\"fn64.known-function-attribution-validation.v1\",\"schema_version\":1,\"report_sha256\":\"{digest}\"}}\n"
            )
            .into_bytes()
        );
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
    }
}
