//! Validate one private, independently repeated executable-image group.
//!
//! This is a narrow CLI over the canonical parser in `trace`; it deliberately
//! emits no path, ROM digest, or captured word.

use fn64_discover::rom::normalize;
use fn64_discover::trace::{
    parse_reproducible_executable_image_group, ExecutableImageCapture, ExecutableImageLineage,
    NormalizedRomDigest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Component, PathBuf};

#[derive(Debug)]
struct Inputs {
    rom: PathBuf,
    group_name: String,
    image_id: String,
    capture_pc: u32,
    first_pc: u32,
    start: u32,
    word_count: u32,
    captures: Vec<PathBuf>,
}

#[derive(Serialize)]
struct Receipt<'a> {
    schema: &'static str,
    status: &'static str,
    group_name: &'a str,
    capture_count: usize,
    image_id: &'a str,
    lineage: &'static str,
    generation: u64,
    capture_pc: u32,
    first_executed_pc: u32,
    va_start: u32,
    byte_len: u32,
    image_sha256: &'a str,
    authority_sha256: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("executable-image group validation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let inputs = parse_args(std::env::args().skip(1))?;
    let rom_bytes = std::fs::read(&inputs.rom).map_err(|_| "reading ROM failed")?;
    let rom =
        normalize(&rom_bytes).map_err(|error| format!("normalizing ROM failed: {error:?}"))?;
    let expected = NormalizedRomDigest::try_from(rom.sha256).map_err(str::to_owned)?;
    let documents = inputs
        .captures
        .iter()
        .enumerate()
        .map(|(index, path)| {
            std::fs::read(path).map_err(|_| format!("reading capture {} failed", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let capture = parse_reproducible_executable_image_group(&documents, &expected, 3)
        .map_err(|error| format!("canonical reproducibility check rejected the group: {error}"))?;
    validate_requested_capture(&inputs, &capture)?;
    let expected_byte_len = inputs.word_count * 4;
    debug_assert_eq!(capture.byte_len, expected_byte_len);

    let authority = format!(
        "fn64:executable-image-group-receipt:v1:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        expected.as_str(),
        inputs.group_name,
        capture.image_id,
        capture.generation,
        capture.capture_pc,
        capture.first_executed_pc,
        capture.va_start,
        capture.byte_len,
        capture.sha256
    );
    let receipt = Receipt {
        schema: "fn64.executable-image-group-receipt.v1",
        status: "validated",
        group_name: &inputs.group_name,
        capture_count: documents.len(),
        image_id: &capture.image_id,
        lineage: "cpu_produced",
        generation: capture.generation,
        capture_pc: capture.capture_pc,
        first_executed_pc: capture.first_executed_pc,
        va_start: capture.va_start,
        byte_len: capture.byte_len,
        image_sha256: &capture.sha256,
        authority_sha256: format!("{:x}", Sha256::digest(authority.as_bytes())),
    };
    println!(
        "{}",
        serde_json::to_string(&receipt).map_err(|_| "serializing path-free receipt failed")?
    );
    Ok(())
}

fn validate_requested_capture(
    inputs: &Inputs,
    capture: &ExecutableImageCapture,
) -> Result<(), String> {
    let expected_byte_len = inputs
        .word_count
        .checked_mul(4)
        .ok_or_else(|| "requested image byte length overflows".to_owned())?;
    if capture.image_id != inputs.image_id
        || capture.lineage != ExecutableImageLineage::CpuProduced
        || capture.generation != 0
        || capture.capture_pc != inputs.capture_pc
        || capture.first_executed_pc != inputs.first_pc
        || capture.va_start != inputs.start
        || capture.byte_len != expected_byte_len
    {
        return Err(
            "capture group does not match the requested image identity and geometry".into(),
        );
    }
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Inputs, String> {
    let mut args = args.into_iter();
    let mut rom = None;
    let mut group_name = None;
    let mut image_id = None;
    let mut capture_pc = None;
    let mut first_pc = None;
    let mut start = None;
    let mut word_count = None;
    let mut captures = Vec::new();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_str() {
            "--rom" => set_once(&mut rom, PathBuf::from(value), "--rom")?,
            "--group-name" => set_once(&mut group_name, value, "--group-name")?,
            "--image-id" => set_once(&mut image_id, value, "--image-id")?,
            "--capture-pc" => set_once(&mut capture_pc, parse_u32(&value)?, "--capture-pc")?,
            "--first-pc" => set_once(&mut first_pc, parse_u32(&value)?, "--first-pc")?,
            "--start" => set_once(&mut start, parse_u32(&value)?, "--start")?,
            "--word-count" => set_once(
                &mut word_count,
                value
                    .parse::<u32>()
                    .map_err(|_| "--word-count must be an integer")?,
                "--word-count",
            )?,
            "--capture" => captures.push(PathBuf::from(value)),
            _ => return Err("unknown argument".into()),
        }
    }
    let inputs = Inputs {
        rom: rom.ok_or("missing --rom")?,
        group_name: group_name.ok_or("missing --group-name")?,
        image_id: image_id.ok_or("missing --image-id")?,
        capture_pc: capture_pc.ok_or("missing --capture-pc")?,
        first_pc: first_pc.ok_or("missing --first-pc")?,
        start: start.ok_or("missing --start")?,
        word_count: word_count.ok_or("missing --word-count")?,
        captures,
    };
    if !valid_group_name(&inputs.group_name) {
        return Err("group name must be an FN64_EXECUTABLE_IMAGE_* token".into());
    }
    if inputs.image_id.is_empty()
        || inputs.image_id.len() > 128
        || !inputs
            .image_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err("image ID must be a 1-128 character portable identifier".into());
    }
    if inputs.captures.len() < 3 {
        return Err("at least three --capture paths are required".into());
    }
    for path in std::iter::once(&inputs.rom).chain(inputs.captures.iter()) {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("all input paths must be absolute without parent traversal".into());
        }
    }
    Ok(inputs)
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{option} supplied more than once"));
    }
    Ok(())
}

fn parse_u32(value: &str) -> Result<u32, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
            .map_err(|_| "address must be a 32-bit decimal or 0x value".into())
    } else {
        value
            .parse::<u32>()
            .map_err(|_| "address must be a 32-bit decimal or 0x value".into())
    }
}

fn valid_group_name(value: &str) -> bool {
    value.starts_with("FN64_EXECUTABLE_IMAGE_")
        && value.len() > "FN64_EXECUTABLE_IMAGE_".len()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_args() -> Vec<String> {
        [
            "--rom",
            "/private/rom.z64",
            "--group-name",
            "FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION",
            "--image-id",
            "general-exception-preamble",
            "--capture-pc",
            "0x80000180",
            "--first-pc",
            "0x80000180",
            "--start",
            "0x80000180",
            "--word-count",
            "4",
            "--capture",
            "/private/capture-1.json",
            "--capture",
            "/private/capture-2.json",
            "--capture",
            "/private/capture-3.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn inputs() -> Inputs {
        parse_args(valid_args()).expect("valid bounded CLI fixture")
    }

    fn capture() -> ExecutableImageCapture {
        ExecutableImageCapture {
            schema: "fn64.executable-image.v1".into(),
            producer: "public-debugger-test".into(),
            normalized_rom_sha256: NormalizedRomDigest::try_from("11".repeat(32)).unwrap(),
            image_id: "general-exception-preamble".into(),
            lineage: ExecutableImageLineage::CpuProduced,
            generation: 0,
            capture_pc: 0x8000_0180,
            first_executed_pc: 0x8000_0180,
            retired_instructions: 7,
            va_start: 0x8000_0180,
            byte_len: 16,
            sha256: "22".repeat(32),
            words: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn cli_accepts_exact_bounded_shape() {
        let parsed = inputs();
        assert_eq!(parsed.capture_pc, 0x8000_0180);
        assert_eq!(parsed.word_count, 4);
        assert_eq!(parsed.captures.len(), 3);
    }

    #[test]
    fn cli_rejects_too_few_captures_and_invalid_group() {
        let mut too_few = valid_args();
        too_few.truncate(too_few.len() - 2);
        assert!(parse_args(too_few).unwrap_err().contains("at least three"));

        let mut invalid_group = valid_args();
        invalid_group[3] = "FN64_EXECUTABLE_IMAGE_".into();
        assert!(parse_args(invalid_group)
            .unwrap_err()
            .contains("group name"));
    }

    #[test]
    fn requested_identity_and_geometry_binding_rejects_every_mismatch() {
        let inputs = inputs();
        let matching = capture();
        validate_requested_capture(&inputs, &matching).unwrap();

        let mut mismatches = Vec::new();
        let mut candidate = matching.clone();
        candidate.image_id = "other-image".into();
        mismatches.push(candidate);
        let mut candidate = matching.clone();
        candidate.lineage = ExecutableImageLineage::SelfModifiedGeneration;
        mismatches.push(candidate);
        let mut candidate = matching.clone();
        candidate.generation = 1;
        mismatches.push(candidate);
        let mut candidate = matching.clone();
        candidate.capture_pc += 4;
        mismatches.push(candidate);
        let mut candidate = matching.clone();
        candidate.first_executed_pc += 4;
        mismatches.push(candidate);
        let mut candidate = matching.clone();
        candidate.va_start += 4;
        mismatches.push(candidate);
        let mut candidate = matching;
        candidate.byte_len += 4;
        mismatches.push(candidate);

        for mismatch in &mismatches {
            assert!(validate_requested_capture(&inputs, mismatch).is_err());
        }
    }
}
